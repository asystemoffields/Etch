//! Regression tests for review findings: hostile piece tables, table row ends, fields,
//! bookmarks, lists and formatting toggles.

use super::*;

#[test]
fn aliased_pieces_claiming_more_text_than_the_stream_are_rejected() {
    // 10k pieces of 10k CPs each all point at the same stored bytes: each alone fits the
    // stream, but together they alias the same bytes far more often than the stream holds.
    let n = 10_000u32;
    let cps: Vec<u32> = (0..=n).map(|i| i * 10_000).collect();
    let fcs: Vec<u32> = (0..n).map(|_| (2 * TEXT_FC) | 0x4000_0000).collect();
    let text = vec![b'\r'; 10_000];
    let err = read(&raw_piece_doc(&cps, &fcs, &text, n * 10_000)).expect_err("rejected");
    assert!(matches!(err, crate::DocbinError::Limit(_)), "{err:?}");
}

/// The process's peak resident memory so far (Linux `VmHWM`), in bytes.
fn peak_rss() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let kb = status.lines().find_map(|l| l.strip_prefix("VmHWM:"))?.trim().trim_end_matches("kB").trim().parse::<u64>().ok()?;
    Some(kb * 1024)
}

#[test]
fn paragraph_count_is_capped_before_paragraphs_are_built() {
    // More paragraph marks than the cap in distinct bytes (no aliasing), about 1 MB of them:
    // refused from the piece table's text, before a paragraph is built. Counting while
    // building stopped only once the cap's worth of paragraphs (about 2 KB each) existed.
    let n = crate::MAX_PARAGRAPHS as u32 + 1_000;
    let text = vec![b'\r'; n as usize];
    let fc = (2 * TEXT_FC) | 0x4000_0000;
    let file = raw_piece_doc(&[0, n], &[fc], &text, n);
    let before = peak_rss();
    // (`err()`, not `expect_err`: printing a document of a million paragraphs takes gigabytes.)
    let err = read(&file).err();
    let after = peak_rss();
    assert!(matches!(&err, Some(crate::DocbinError::Limit(m)) if m.contains("paragraph marks")), "{err:?}");
    if let (Some(a), Some(b)) = (before, after) {
        assert!(b.saturating_sub(a) < 64 << 20, "reading raised the peak by {} MB", b.saturating_sub(a) >> 20);
    }
    // The same marks in UTF-16 count the same.
    let utf16: Vec<u8> = text.iter().flat_map(|&c| [c, 0]).collect();
    let err = read(&raw_piece_doc(&[0, n], &[TEXT_FC], &utf16, n)).err();
    assert!(matches!(&err, Some(crate::DocbinError::Limit(m)) if m.contains("paragraph marks")), "{err:?}");
}

#[test]
fn paragraphs_up_to_the_cap_still_open() {
    // Cell marks count with paragraph marks, and a document with fewer of them than the cap
    // reads every one (the count refuses nothing a walk would read).
    let text = b"a\rb\x07c\x07\rd\r";
    let n = text.len() as u32;
    let fc = (2 * TEXT_FC) | 0x4000_0000;
    let doc = read(&raw_piece_doc(&[0, n], &[fc], text, n)).expect("opens");
    assert!(doc.paragraph_count() >= 4, "{}", doc.paragraph_count());
}

/// A paragraph read keeps no room to spare (a run is about 350 bytes; one pushed into an empty
/// list made room for four).
#[test]
fn paragraphs_read_hold_no_spare_room() {
    let text = b"one\rtwo words\rthree\r";
    let n = text.len() as u32;
    let fc = (2 * TEXT_FC) | 0x4000_0000;
    let doc = read(&raw_piece_doc(&[0, n], &[fc], text, n)).expect("opens");
    for b in &doc.body {
        let p = b.as_para().expect("a paragraph");
        assert_eq!(p.runs.capacity(), p.runs.len(), "{:?}", p.text);
        assert_eq!(p.text.capacity(), p.text.len(), "{:?}", p.text);
    }
}

/// A table spec: `text` with the given (start CP, grpprl) paragraph entries.
fn table_spec(text: &'static str, papx: Vec<(u32, Vec<u8>)>) -> StructSpec {
    StructSpec {
        text,
        header_stories: vec![],
        sections: vec![(text.chars().count() as u32, Vec::new())],
        chpx_runs: vec![(0, Vec::new())],
        papx_entries: papx.into_iter().map(|(cp, g)| (cp, 0, g)).collect(),
        plf_lst: Vec::new(),
        plf_lfo: Vec::new(),
        note_stories: vec![],
        endnote_stories: vec![],
        extra_pairs: Vec::new(),
        data_stream: Vec::new(),
    }
}

#[test]
fn row_end_mark_is_not_a_cell_but_legacy_ttp_cells_still_count() {
    let in_tbl = grpprl(&[(0x2416, &[0x01])]);
    let ttp = grpprl(&[(0x2416, &[0x01]), (0x2417, &[0x01])]);
    // Real layout: "a\x07 \x07 \x07|ttp" is one row of two cells, the second empty.
    let spec = table_spec("a\u{7}\u{7}\u{7}x\r", vec![(0, in_tbl.clone()), (2, in_tbl.clone()), (3, ttp.clone()), (4, Vec::new())]);
    let doc = read(&struct_doc(&spec)).expect("opens");
    let tbl = doc.body.iter().find_map(|b| b.as_table()).expect("table");
    assert_eq!(tbl.rows.len(), 1);
    assert_eq!(tbl.rows[0].cells.len(), 2);
    // Tolerated older shape: the TTP sits on the last cell's own (non-empty) mark.
    let spec = table_spec("a\u{7}b\u{7}x\r", vec![(0, in_tbl), (2, ttp), (4, Vec::new())]);
    let doc = read(&struct_doc(&spec)).expect("opens");
    let tbl = doc.body.iter().find_map(|b| b.as_table()).expect("table");
    assert_eq!(tbl.rows.len(), 1);
    assert_eq!(tbl.rows[0].cells.len(), 2);
}

fn paras(text: &'static str) -> Vec<wordcraft_doc::para::Paragraph> {
    let ccp = text.chars().count() as u32;
    let doc = read(&struct_doc(&structured_spec(text, ccp))).expect("opens");
    doc.body.iter().filter_map(|b| b.as_para().cloned()).collect()
}

#[test]
fn multi_paragraph_field_result_keeps_one_paragraph_per_entry() {
    // A TOC: its result spans paragraphs, each entry a nested HYPERLINK whose instruction
    // must not leak into the text.
    let p = paras("\u{13} TOC \\o \u{14}\u{13} HYPERLINK \\l \"_Toc1\" \u{14}One\u{15}\rTwo\r\u{15}tail\r");
    let texts: Vec<&str> = p.iter().map(|p| p.text.as_str()).collect();
    assert_eq!(texts, ["One", "Two", "tail"]);
    assert!(p.iter().all(|p| p.objects.is_empty()));
}

#[test]
fn nested_field_instruction_does_not_leak_into_the_result() {
    let p = paras("\u{13}IF 1 = 1 \u{14}A\u{13}PAGE\u{14}7\u{15}B\u{15}\r");
    let wordcraft_doc::para::InlineObject::Field { result, .. } = p[0].objects.first().expect("field") else { panic!("{:?}", p[0].objects) };
    assert_eq!(result, "A7B");
}

#[test]
fn unterminated_field_keeps_the_following_text() {
    // No separator or end: the instruction text stays visible and the next paragraph
    // is untouched.
    let texts: Vec<String> = paras("Go \u{13}PAGE\rnext\r").into_iter().map(|p| p.text).collect();
    assert_eq!(texts, ["Go PAGE", "next"]);
    // A result left open at the end of the text is kept as plain text.
    let texts: Vec<String> = paras("a\r\u{13}PAGE\u{14}3").into_iter().map(|p| p.text).collect();
    assert_eq!(texts, ["a", "3"]);
}

#[test]
fn lfo_for_unknown_list_keeps_later_overrides_in_step() {
    // Two LFOs: the first names a list that doesn't exist, the second restarts level 0 of
    // the real list at 5. The first LFOData's cp must be skipped too, or the second LFOLVL is
    // read four bytes early.
    let levels: Vec<Vec<u8>> = (0..9).map(|_| lvl(1, 0x00, &[0x0000, 0x002E], true)).collect();
    let mut lfo = 2u32.to_le_bytes().to_vec();
    for (lsid, clfolvl) in [(0x7777i32, 0u8), (0x1000, 1)] {
        lfo.extend_from_slice(&lsid.to_le_bytes());
        lfo.extend_from_slice(&[0u8; 8]);
        lfo.push(clfolvl);
        lfo.extend_from_slice(&[0u8; 3]);
    }
    lfo.extend_from_slice(&0u32.to_le_bytes()); // LFOData 1: cp, no LFOLVLs
    lfo.extend_from_slice(&0u32.to_le_bytes()); // LFOData 2: cp
    lfo.extend_from_slice(&5u32.to_le_bytes()); // LFOLVL: iStartAt = 5
    lfo.extend_from_slice(&[0x10, 0, 0, 0]); // level 0, fStartAt
    let text = "one\r";
    let spec = StructSpec { plf_lst: plf_lst(0x1000, false, &levels), plf_lfo: lfo, ..table_spec(text, vec![(0, Vec::new())]) };
    let doc = read(&struct_doc(&spec)).expect("opens");
    let num = doc.numbering.nums.iter().find(|n| n.id == 2).expect("second LFO");
    assert_eq!(num.start_overrides, vec![(0, 5)]);
}

#[test]
fn nested_bookmarks_pair_ends_through_ibkl() {
    // "outer" spans [0, 10), "inner" [2, 5). Ends are sorted by CP (5, 10), so outer's FBKF
    // points at end 1 and inner's at end 0; pairing by position swapped them.
    let text = "0123456789\r";
    let ccp = text.chars().count() as u32;
    let fbkf = [1u16.to_le_bytes(), 0u16.to_le_bytes(), 0u16.to_le_bytes(), 0u16.to_le_bytes()].concat();
    let spec = StructSpec {
        text,
        extra_pairs: vec![(21, sttbf_bkmk(&["outer", "inner"])), (22, plc(&[0, 2, ccp], &fbkf)), (23, plc(&[5, 10, ccp], &[]))],
        ..structured_spec(text, ccp)
    };
    let doc = read(&struct_doc(&spec)).expect("opens");
    let p0 = doc.body.first().and_then(|b| b.as_para()).expect("para");
    use wordcraft_doc::para::InlineObject::{BookmarkEnd, BookmarkStart};
    let seq: Vec<String> = p0
        .objects
        .iter()
        .map(|o| match o {
            BookmarkStart { name } => format!("+{name}"),
            BookmarkEnd { name } => format!("-{name}"),
            other => format!("{other:?}"),
        })
        .collect();
    assert_eq!(seq, ["+outer", "+inner", "-inner", "-outer"]);
    assert_eq!(p0.text, "\u{FFFC}01\u{FFFC}234\u{FFFC}56789\u{FFFC}");
}

#[test]
fn toggle_inverts_the_paragraph_style_value() {
    // sprmCFBold = 0x81 means "the opposite of the style": off in a bold style, on in Normal.
    // The style is only applied at the paragraph mark, so the toggle used to invert the
    // default (not bold) everywhere.
    let spec = RichSpec {
        text: "strong\rplain\r",
        styles: vec![(1, 0x0FFF, "Normal", Vec::new(), Vec::new()), (1, 0, "Loud", Vec::new(), grpprl(&[(0x0835, &[0x01])]))],
        fonts: vec!["Times New Roman"],
        chpx_runs: vec![(0, grpprl(&[(0x0835, &[0x81])]))],
        papx_entries: vec![(0, 1, Vec::new()), (7, 0, Vec::new())],
    };
    let doc = read(&rich_doc(&spec)).expect("opens");
    let p = |i: usize| doc.body.get(i).and_then(|b| b.as_para()).cloned().expect("para");
    assert_eq!(p(0).props.style.as_deref(), Some("Loud"));
    assert_eq!(p(0).runs.first().and_then(|r| r.props.bold), Some(false), "{:?}", p(0).runs);
    assert_eq!(p(1).runs.first().and_then(|r| r.props.bold), Some(true), "{:?}", p(1).runs);
}

#[test]
fn literal_object_replacement_char_in_text_is_not_an_anchor() {
    // A U+FFFC stored in the file's text (and in a field result) would claim an object that
    // isn't there and shift every later anchor.
    let p = paras("a\u{FFFC}b\u{13}PAGE\u{14}\u{FFFC}\u{15}\r");
    assert_eq!(p[0].text, "a\u{FFFD}b\u{FFFC}");
    assert_eq!(p[0].objects.len(), 1);
    let wordcraft_doc::para::InlineObject::Field { result, .. } = &p[0].objects[0] else { panic!() };
    assert_eq!(result, "\u{FFFD}");
}
