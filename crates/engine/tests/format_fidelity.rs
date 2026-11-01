//! Round-trip fidelity harness: a corpus of synthetic documents, one per feature of the document
//! model, each saved in every editable format and opened again. For each feature and format it
//! records whether the feature survives:
//!
//! - **save**: the document saved as X and opened again;
//! - **via DOCX**: the document saved as DOCX, opened, saved as X and opened again;
//! - **stable**: X opened and saved again as X gives the same feature (a second save loses nothing
//!   more).
//!
//! What a format loses must be what the save prompt names (`io::lossy_features`), and nothing it
//! keeps may be named there. Run with `--nocapture` (`cargo test -p wordcraft-engine --test
//! format_fidelity -- --nocapture`) to print the matrix.
#![cfg(not(target_arch = "wasm32"))]

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde_json::json;
use wordcraft_doc::para::InlineObject;
use wordcraft_doc::props::{CharProps, TextColor, VMerge, VertAlign};
use wordcraft_doc::table::Table;
use wordcraft_doc::{Block, Blocks, Document, Paragraph, PartKind, Revision, RevisionKind, para_block};
use wordcraft_engine::{Session, io};

/// Formats that hold a whole document (as against plain text). `wordml2003` is an `.xml` file
/// written as a Word 2003 XML Document (`format: "wordml2003"`, and a save over such a file).
const FORMATS: &[&str] = &["docx", "json", "odt", "rtf", "html", "md", "wordml2003"];

/// The file name and options a save in `ext` (one of [`FORMATS`]) is written with.
fn save_as(ext: &str) -> (String, io::ExportOptions) {
    match ext {
        "wordml2003" => ("fidelity.xml".into(), io::ExportOptions { word2003: true, ..Default::default() }),
        _ => (format!("fidelity.{ext}"), io::ExportOptions::default()),
    }
}

// ---------------------------------------------------------------------------------------------
// Fingerprints: what a feature looks like in a document, independent of how a format spells it.

/// Deepest table nesting looked into.
const DEPTH: usize = 6;

fn clean(s: &str) -> String {
    s.chars().filter(|c| *c != wordcraft_doc::para::OBJ).collect::<String>().trim().to_string()
}

/// Every paragraph of `blocks`, tables included, in order.
fn paras(blocks: &Blocks) -> Vec<&Paragraph> {
    fn go<'a>(bl: &'a Blocks, out: &mut Vec<&'a Paragraph>, d: usize) {
        for b in bl {
            match &**b {
                Block::Para(p) => out.push(p),
                Block::Table(t) if d < DEPTH => {
                    for r in &t.rows {
                        for c in &r.cells {
                            go(&c.blocks, out, d + 1);
                        }
                    }
                }
                Block::Table(_) => {}
            }
        }
    }
    let mut out = Vec::new();
    go(blocks, &mut out, 0);
    out
}

/// Non-empty paragraph texts of `blocks`.
fn texts(blocks: &Blocks) -> Vec<String> {
    paras(blocks).iter().map(|p| clean(&p.plain_text())).filter(|t| !t.is_empty()).collect()
}

/// Text of the runs of `blocks` whose properties satisfy `f`, with what `f` says about them.
fn runs_where(blocks: &Blocks, f: &dyn Fn(&CharProps) -> Option<String>) -> Vec<String> {
    let mut out = Vec::new();
    for p in paras(blocks) {
        for (r, props) in p.run_ranges() {
            let t = clean(p.text.get(r).unwrap_or(""));
            if t.is_empty() {
                continue;
            }
            if let Some(v) = f(props) {
                out.push(format!("{t}={v}"));
            }
        }
    }
    out
}

/// The inline objects of `blocks`.
fn objects(blocks: &Blocks) -> Vec<&InlineObject> {
    paras(blocks).into_iter().flat_map(|p| p.objects.iter()).collect()
}

fn part_texts(doc: &Document, kind: PartKind) -> Vec<String> {
    doc.parts.values().filter(|p| p.kind == kind).map(|p| texts(&p.blocks).join(" / ")).filter(|t| !t.is_empty()).collect()
}

fn rgb(c: Option<TextColor>) -> Option<String> {
    match c {
        Some(TextColor::Rgb(c)) => Some(c.hex()),
        _ => None,
    }
}

fn tables(blocks: &Blocks, d: usize, out: &mut Vec<String>) {
    for b in blocks {
        if let Block::Table(t) = &**b {
            let mut s = format!("T{d}[");
            for r in &t.rows {
                for c in &r.cells {
                    let span = if c.props.span > 1 { format!("~{}", c.props.span) } else { String::new() };
                    let vm = match c.props.vmerge {
                        VMerge::Restart => "v+",
                        VMerge::Continue => "v-",
                        VMerge::None => "",
                    };
                    let _ = write!(s, "{}{span}{vm}|", texts(&c.blocks).join(" "));
                }
                s.push(';');
            }
            s.push(']');
            out.push(s);
            if d < DEPTH {
                for r in &t.rows {
                    for c in &r.cells {
                        tables(&c.blocks, d + 1, out);
                    }
                }
            }
        }
    }
}

/// The fingerprint of feature `f` in `doc`.
fn fingerprint(f: &str, doc: &Document) -> String {
    let body = &doc.body;
    let v: Vec<String> = match f {
        "text" => texts(body),
        "lineBreakTab" => paras(body).iter().map(|p| clean(&p.text)).filter(|t| !t.is_empty()).collect(),
        "bold" => runs_where(body, &|c| c.bold.filter(|b| *b).map(|_| "b".into())),
        "italic" => runs_where(body, &|c| c.italic.filter(|b| *b).map(|_| "i".into())),
        "underline" => runs_where(body, &|c| c.underline.filter(|u| *u != wordcraft_doc::props::Underline::None).map(|_| "u".into())),
        "strike" => runs_where(body, &|c| c.strike.filter(|b| *b).map(|_| "s".into())),
        "superscript" => runs_where(body, &|c| (c.vert_align == Some(VertAlign::Superscript)).then(|| "sup".into())),
        "subscript" => runs_where(body, &|c| (c.vert_align == Some(VertAlign::Subscript)).then(|| "sub".into())),
        "color" => runs_where(body, &|c| rgb(c.color)),
        "highlight" => runs_where(body, &|c| c.highlight.and_then(|h| h.rgb()).or(c.shading).map(|c| c.hex())),
        "font" => runs_where(body, &|c| c.font.clone().filter(|f| f == "Georgia")),
        "size" => runs_where(body, &|c| c.size.filter(|s| (*s - 18.0).abs() < 0.01).map(|s| s.to_string())),
        "alignment" => paras(body).iter().filter_map(|p| p.props.align.map(|a| format!("{}={a:?}", clean(&p.plain_text())))).collect(),
        "headings" => paras(body)
            .iter()
            .filter_map(|p| doc.styles.resolve_para(&p.props).outline_level.map(|l| format!("H{}:{}", l + 1, clean(&p.plain_text()))))
            .collect(),
        "lists" => paras(body)
            .iter()
            .filter_map(|p| {
                let n = doc.styles.resolve_para(&p.props).numbering?;
                let lv = doc.numbering.level(n.num, n.level)?;
                let bullet = matches!(lv.format, wordcraft_doc::section::NumFormat::Bullet);
                Some(format!("{}{}:{}", if bullet { "•" } else { "1." }, n.level, clean(&p.plain_text())))
            })
            .collect(),
        "table" | "nestedTable" | "mergedCells" => {
            let mut out = Vec::new();
            tables(body, 0, &mut out);
            out
        }
        "picture" => objects(body)
            .iter()
            .filter_map(|o| match o {
                InlineObject::Image { media, w, h, .. } => {
                    Some(format!("{}x{}:{}", w.round(), h.round(), doc.media.get(media).map_or(0, |m| m.len().min(1))))
                }
                _ => None,
            })
            .collect(),
        "hyperlink" => runs_where(body, &|c| c.link.clone()),
        // The macro project, kept as opaque bytes (only `.docm`/`.dotm` and the JSON hold it).
        "macros" => doc.passthrough.get("word/vbaProject.bin").map(|b| format!("vba:{}", b.len())).into_iter().collect(),
        "bookmark" => objects(body)
            .iter()
            .filter_map(|o| match o {
                InlineObject::BookmarkStart { name } if name != "_GoBack" => Some(name.clone()),
                _ => None,
            })
            .collect(),
        "pageBreak" => paras(body)
            .iter()
            .filter(|p| p.props.page_break_before == Some(true) || p.text.contains(wordcraft_doc::para::PAGE_BREAK))
            .map(|_| "break".to_string())
            .collect(),
        "footnote" | "endnote" => {
            let kind = if f == "footnote" { PartKind::Footnote } else { PartKind::Endnote };
            let refs = objects(body)
                .iter()
                .filter(
                    |o| matches!(o, InlineObject::NoteRef { kind: k, .. } if (*k == wordcraft_doc::para::NoteKind::Footnote) == (f == "footnote")),
                )
                .count();
            let mut v = part_texts(doc, kind);
            v.push(format!("refs={refs}"));
            v
        }
        "comment" => doc
            .comments
            .values()
            .map(|c| {
                let t = doc.parts.get(&c.part).map(|p| texts(&p.blocks).join(" / ")).unwrap_or_default();
                format!("{}:{}:{}", c.author, t, c.parent.is_some())
            })
            .collect(),
        "commentRange" => {
            // The text each comment is on.
            let mut v = Vec::new();
            for p in paras(body) {
                let mut open: Vec<(u32, usize)> = Vec::new();
                let mut k = 0;
                for (i, ch) in p.text.char_indices() {
                    if ch != wordcraft_doc::para::OBJ {
                        continue;
                    }
                    match p.objects.get(k) {
                        Some(InlineObject::CommentStart { .. }) => open.push((k as u32, i)),
                        Some(InlineObject::CommentEnd { .. }) => {
                            if let Some((_, s)) = open.pop() {
                                v.push(clean(p.text.get(s..i).unwrap_or("")));
                            }
                        }
                        _ => {}
                    }
                    k += 1;
                }
            }
            v
        }
        "header" => part_texts(doc, PartKind::Header),
        "footer" => part_texts(doc, PartKind::Footer),
        "pageField" => doc
            .parts
            .values()
            .flat_map(|p| objects(&p.blocks))
            .filter_map(|o| match o {
                InlineObject::Field { instr, .. } | InlineObject::FieldStart { instr, .. } => {
                    Some(instr.trim().split(' ').next().unwrap_or("").to_uppercase())
                }
                _ => None,
            })
            .collect(),
        "nestedField" => objects(body)
            .into_iter()
            .filter_map(|o| match o {
                InlineObject::Field { instr, nested, .. } if !nested.is_empty() => Some(wordcraft_layout::fields::nested_code_text(instr, nested)),
                _ => None,
            })
            .collect(),
        "trackedChanges" => {
            let mut v = Vec::new();
            for p in paras(body) {
                for (r, c) in p.run_ranges() {
                    let t = clean(p.text.get(r).unwrap_or(""));
                    for (idx, kind) in [(c.ins, "ins"), (c.del, "del")] {
                        if let Some(rev) = idx.and_then(|i| doc.revisions.get(i as usize)) {
                            let want = if kind == "ins" { RevisionKind::Insert } else { RevisionKind::Delete };
                            if rev.kind == want && !t.is_empty() {
                                v.push(format!("{kind}:{t}:{}", rev.author));
                            }
                        }
                    }
                }
            }
            v
        }
        "textBox" => part_texts(doc, PartKind::TextBox),
        "equation" => objects(body)
            .iter()
            .filter_map(|o| match o {
                InlineObject::Equation { linear, .. } => Some(linear.replace(' ', "")),
                _ => None,
            })
            .collect(),
        "contentControl" => objects(body).iter().filter(|o| matches!(o, InlineObject::ControlStart { .. })).map(|_| "cc".to_string()).collect(),
        "drawing" => objects(body)
            .iter()
            .filter(|o| matches!(o, InlineObject::Shape { story: None, .. } | InlineObject::Group { .. }))
            .map(|_| "shape".to_string())
            .collect(),
        "sections" => doc.sections().iter().map(|(_, s)| format!("{}x{}", s.page_w.round(), s.page_h.round())).collect(),
        "pageSetup" => {
            let s = &doc.last_section;
            vec![format!(
                "{}x{} m{} {} {} {}",
                s.page_w.round(),
                s.page_h.round(),
                s.margin_left.round(),
                s.margin_right.round(),
                s.margin_top.round(),
                s.margin_bottom.round()
            )]
        }
        "properties" => vec![doc.core.title.clone(), doc.core.creator.clone()],
        // Which header each kind of page shows, section by section (links followed).
        "firstPageHeader" | "evenPageHeader" | "sectionHeaders" => {
            let mut v = Vec::new();
            let (mut d, mut first, mut even) = (None, None, None);
            for (_, sec) in doc.sections() {
                d = sec.headers.default.or(d);
                first = sec.headers.first.or(first);
                even = sec.headers.even.or(even);
                let t = |id: Option<u32>| id.and_then(|i| doc.parts.get(&i)).map(|p| texts(&p.blocks).join(" ")).unwrap_or_default();
                let f = if sec.title_page { t(first) } else { String::new() };
                let e = if doc.settings.even_odd_headers { t(even) } else { String::new() };
                v.push(format!("default={} first={f} even={e}", t(d)));
            }
            v
        }
        "commentReply" => {
            let author_of = |id: Option<u32>| id.and_then(|i| doc.comments.get(&i)).map(|c| c.author.clone()).unwrap_or_default();
            doc.comments
                .values()
                .map(|c| {
                    let t = doc.parts.get(&c.part).map(|p| texts(&p.blocks).join(" / ")).unwrap_or_default();
                    format!("{}:{}:reply-to={}", c.author, t, author_of(c.parent))
                })
                .collect()
        }
        "noteInTable" | "noteFormatting" => {
            let mut v: Vec<String> = doc
                .parts
                .values()
                .filter(|p| p.kind == PartKind::Footnote)
                .flat_map(|p| runs_where(&p.blocks, &|c| c.bold.filter(|b| *b).map(|_| "b".into())))
                .collect();
            v.extend(part_texts(doc, PartKind::Footnote));
            let mut t = Vec::new();
            tables(body, 0, &mut t);
            v.extend(t);
            v
        }
        "customNoteMark" => objects(body)
            .iter()
            .filter_map(|o| match o {
                InlineObject::NoteRef { custom, .. } => Some(format!("mark={custom}")),
                _ => None,
            })
            .collect(),
        "moves" => doc.revisions.iter().filter(|r| r.move_name.is_some()).map(|r| format!("{:?}", r.kind)).collect(),
        "smartTag" => runs_where(body, &|c| c.wrap.as_ref().map(|w| w.contains("w:smartTag").to_string())),
        "specVanish" => runs_where(body, &|c| c.spec_vanish.map(|v| v.to_string())),
        "ptab" => paras(body)
            .iter()
            .flat_map(|p| p.run_ranges().filter_map(|(r, c)| c.ptab.as_ref().map(|t| format!("{}:{t}", r.len()))).collect::<Vec<_>>())
            .collect(),
        "oleObject" => objects(body)
            .iter()
            .filter_map(|o| match o {
                InlineObject::Image { ole: Some(e), .. } => Some(e.refs.len().to_string()),
                _ => None,
            })
            .collect(),
        // Right-to-left paragraphs (Word's `w:bidi`) and their text.
        "rtl" => paras(body).iter().filter(|p| doc.styles.resolve_para(&p.props).bidi).map(|p| format!("rtl:{}", clean(&p.plain_text()))).collect(),
        // East Asian text: the characters themselves, ideographic spaces and full-width
        // punctuation included (`texts` trims ASCII white space only).
        "eastAsian" => paras(body).iter().map(|p| clean(&p.plain_text())).filter(|t| !t.is_empty()).collect(),
        // The label each list paragraph shows ("1.", "a.", "•"), its level and text: a restarted
        // list counts from 1 again, a continued one counts on.
        "listRestart" | "listContinue" | "nestedLists" => {
            let mut counters = wordcraft_doc::numbering::Counters::default();
            paras(body)
                .iter()
                .filter_map(|p| {
                    let n = doc.styles.resolve_para(&p.props).numbering?;
                    let (label, _) = counters.next_label(&doc.numbering, n.num, n.level)?;
                    Some(format!("{label}{}:{}", n.level, clean(&p.plain_text())))
                })
                .collect()
        }
        // Each table's column widths (points, rounded to 2: readers convert through other units).
        "tableWidths" => {
            fn widths(blocks: &Blocks, out: &mut Vec<String>) {
                for b in blocks {
                    if let Block::Table(t) = &**b {
                        out.push(t.grid.iter().map(|w| ((w / 2.0).round() * 2.0).to_string()).collect::<Vec<_>>().join(","));
                    }
                }
            }
            let mut v = Vec::new();
            widths(body, &mut v);
            v
        }
        // Shaded cells: their text and colour.
        "cellShading" => {
            let mut v = Vec::new();
            for b in body {
                if let Block::Table(t) = &**b {
                    for c in t.rows.iter().flat_map(|r| r.cells.iter()) {
                        if let Some(s) = c.props.shading {
                            v.push(format!("{}={}", texts(&c.blocks).join(" "), s.hex()));
                        }
                    }
                }
            }
            v
        }
        // Paragraph spacing and indents a paragraph states itself (points).
        "paraSpacing" => paras(body)
            .iter()
            .filter_map(|p| {
                let pp = &p.props;
                let any = [pp.space_before, pp.space_after, pp.indent_left, pp.indent_first].iter().any(Option::is_some) || pp.line_spacing.is_some();
                any.then(|| {
                    let r = |v: Option<f32>| v.map(|v| v.round().to_string()).unwrap_or_default();
                    let ls = match pp.line_spacing {
                        Some(wordcraft_doc::props::LineSpacing::Multiple(m)) => format!("x{m:.2}"),
                        Some(wordcraft_doc::props::LineSpacing::AtLeast(v)) => format!(">={}", v.round()),
                        Some(wordcraft_doc::props::LineSpacing::Exactly(v)) => format!("={}", v.round()),
                        None => String::new(),
                    };
                    format!(
                        "{}:sb{} sa{} li{} fi{} {ls}",
                        clean(&p.plain_text()),
                        r(pp.space_before),
                        r(pp.space_after),
                        r(pp.indent_left),
                        r(pp.indent_first)
                    )
                })
            })
            .collect(),
        // A field in the body: its kind (a simple field or a field range, as the format spells
        // it; the result isn't compared, since opening a file updates a DATE).
        "bodyField" => objects(body)
            .iter()
            .filter_map(|o| match o {
                InlineObject::Field { instr, .. } | InlineObject::FieldStart { instr, .. } => {
                    Some(instr.trim().split(' ').next().unwrap_or("").to_uppercase())
                }
                _ => None,
            })
            .collect(),
        _ => vec![format!("unknown feature {f}")],
    };
    v.join(" | ")
}

// ---------------------------------------------------------------------------------------------
// Corpus

fn run(s: &mut Session, id: &str, v: serde_json::Value) {
    if let Err(e) = s.run(id, &v) {
        panic!("{id} {v}: {e:?}");
    }
}

fn select(s: &mut Session, text: &str) {
    run(s, "select.text", json!({ "text": text }));
}

/// A session on `text`, caret at the end.
fn session(text: &str) -> Session {
    let mut s = Session::new(Document::from_text(text));
    let end = s.doc.end_of(wordcraft_doc::StoryRef::Body);
    s.sel = wordcraft_engine::Selection::caret(end);
    s
}

fn formatted(word: &str, cmd: &str, v: serde_json::Value) -> Document {
    let mut s = session(&format!("Plain words then {word} and more plain words."));
    select(&mut s, word);
    run(&mut s, cmd, v);
    s.doc
}

fn cell_text(t: &mut Table, r: usize, c: usize, text: &str) {
    if let Some(cell) = t.rows.get_mut(r).and_then(|row| row.cells.get_mut(c)) {
        cell.blocks = vec![para_block(Paragraph::with_text(text, CharProps::default()))];
    }
}

fn table_doc(t: Table) -> Document {
    let mut d = Document::from_text("Before the table.");
    d.body.push(std::sync::Arc::new(Block::Table(t)));
    d.body.push(para_block(Paragraph::with_text("After the table.", CharProps::default())));
    d
}

fn png() -> Vec<u8> {
    let img = image::RgbaImage::from_fn(4, 3, |x, y| image::Rgba([x as u8 * 60, y as u8 * 80, 120, 255]));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

/// The corpus: (feature, the document showing it, the `lossy_features` id that names its loss).
fn corpus() -> Vec<(&'static str, Document, Option<&'static str>)> {
    let mut c: Vec<(&'static str, Document, Option<&'static str>)> = Vec::new();
    c.push(("text", Document::from_text("First paragraph of synthetic text.\nSecond paragraph, with punctuation: \"quotes\" & <angles>.\nThird — with non-ASCII: café, Ελληνικά, 日本語."), None));
    c.push((
        "lineBreakTab",
        {
            let mut s = session("Line one");
            run(&mut s, "text.lineBreak", json!({}));
            run(&mut s, "text.insert", json!({"text": "line two"}));
            run(&mut s, "text.tab", json!({}));
            run(&mut s, "text.insert", json!({"text": "after tab"}));
            s.doc
        },
        Some("formatting"),
    ));
    c.push(("bold", formatted("strong", "format.bold", json!({})), Some("formatting")));
    c.push(("italic", formatted("slanted", "format.italic", json!({})), Some("formatting")));
    c.push(("underline", formatted("lined", "format.underline", json!({})), Some("formatting")));
    c.push(("strike", formatted("struck", "format.strikethrough", json!({})), Some("formatting")));
    c.push(("superscript", formatted("raised", "format.superscript", json!({})), Some("formatting")));
    c.push(("subscript", formatted("lowered", "format.subscript", json!({})), Some("formatting")));
    c.push(("color", formatted("coloured", "format.color", json!({"color": "C00000"})), Some("formatting")));
    c.push(("highlight", formatted("marked", "format.highlight", json!({"color": "yellow"})), Some("formatting")));
    c.push(("font", formatted("serif", "format.font", json!({"name": "Georgia"})), Some("formatting")));
    c.push(("size", formatted("larger", "format.size", json!({"size": 18})), Some("formatting")));
    c.push((
        "alignment",
        {
            let mut s = session("Centred line\nRight line\nJustified line of text.");
            select(&mut s, "Centred line");
            run(&mut s, "para.alignCenter", json!({}));
            select(&mut s, "Right line");
            run(&mut s, "para.alignRight", json!({}));
            select(&mut s, "Justified line");
            run(&mut s, "para.justify", json!({}));
            s.doc
        },
        Some("formatting"),
    ));
    c.push((
        "headings",
        {
            let mut s = session("Main heading\nBody text.\nSub heading\nMore text.");
            select(&mut s, "Main heading");
            run(&mut s, "para.style", json!({"style": "Heading1"}));
            select(&mut s, "Sub heading");
            run(&mut s, "para.style", json!({"style": "Heading2"}));
            s.doc
        },
        None,
    ));
    c.push((
        "lists",
        {
            let mut s = session("Intro\nApples\nPears\nFirst step\nSecond step\nOutro");
            select(&mut s, "Apples");
            run(&mut s, "para.bullets", json!({}));
            select(&mut s, "Pears");
            run(&mut s, "para.bullets", json!({}));
            run(&mut s, "para.indent", json!({}));
            select(&mut s, "First step");
            run(&mut s, "para.numbering", json!({}));
            select(&mut s, "Second step");
            run(&mut s, "para.numbering", json!({}));
            s.doc
        },
        None,
    ));
    c.push((
        "table",
        {
            let mut t = Table::new(2, 2, 400.0);
            for (r, c, x) in [(0, 0, "A1"), (0, 1, "B1"), (1, 0, "A2"), (1, 1, "B2")] {
                cell_text(&mut t, r, c, x);
            }
            table_doc(t)
        },
        None,
    ));
    c.push((
        "nestedTable",
        {
            let mut inner = Table::new(1, 2, 150.0);
            cell_text(&mut inner, 0, 0, "inner one");
            cell_text(&mut inner, 0, 1, "inner two");
            let mut t = Table::new(1, 2, 400.0);
            cell_text(&mut t, 0, 0, "outer");
            if let Some(cell) = t.rows.get_mut(0).and_then(|r| r.cells.get_mut(1)) {
                cell.blocks = vec![std::sync::Arc::new(Block::Table(inner)), para_block(Paragraph::new())];
            }
            table_doc(t)
        },
        Some("nestedTables"),
    ));
    c.push((
        "mergedCells",
        {
            let mut t = Table::new(3, 3, 400.0);
            for r in 0..3 {
                for col in 0..3 {
                    cell_text(&mut t, r, col, &format!("r{r}c{col}"));
                }
            }
            // Row 0: the first two cells merged across; column 2: rows 1–2 merged down.
            if let Some(row) = t.rows.get_mut(0) {
                row.cells.remove(1);
                if let Some(c0) = row.cells.get_mut(0) {
                    c0.props.span = 2;
                    c0.props.width = Some(266.0);
                }
            }
            if let Some(c) = t.rows.get_mut(1).and_then(|r| r.cells.get_mut(2)) {
                c.props.vmerge = VMerge::Restart;
            }
            if let Some(c) = t.rows.get_mut(2).and_then(|r| r.cells.get_mut(2)) {
                c.props.vmerge = VMerge::Continue;
                c.blocks = vec![para_block(Paragraph::new())];
            }
            table_doc(t)
        },
        Some("mergedCells"),
    ));
    c.push((
        "picture",
        {
            let mut s = session("A picture: ");
            let data = png();
            let key = s.doc.add_media(data, "png");
            let at = s.doc.end_of(wordcraft_doc::StoryRef::Body);
            let img = InlineObject::Image {
                media: key,
                w: 48.0,
                h: 36.0,
                alt: "Synthetic picture".into(),
                float: Default::default(),
                crop: [0.0; 4],
                ole: None,
            };
            s.doc.insert_object(&at, img, &CharProps::default()).unwrap();
            s.doc
        },
        Some("pictures"),
    ));
    c.push(("hyperlink", formatted("a link", "insert.link", json!({"url": "https://example.com/page"})), None));
    c.push((
        "bookmark",
        {
            let mut s = session("Marked place in the text.");
            select(&mut s, "Marked place");
            run(&mut s, "insert.bookmark", json!({"name": "SyntheticMark"}));
            s.doc
        },
        None,
    ));
    c.push((
        "pageBreak",
        {
            let mut s = session("Page one text.");
            run(&mut s, "text.pageBreak", json!({}));
            run(&mut s, "text.insert", json!({"text": "Page two text."}));
            s.doc
        },
        Some("formatting"),
    ));
    c.push((
        "footnote",
        {
            let mut s = session("Text with a footnote");
            run(&mut s, "references.footnote", json!({"text": "Synthetic footnote text."}));
            s.doc
        },
        Some("footnotes"),
    ));
    c.push((
        "endnote",
        {
            let mut s = session("Text with an endnote");
            run(&mut s, "references.endnote", json!({"text": "Synthetic endnote text."}));
            s.doc
        },
        Some("endnotes"),
    ));
    c.push((
        "comment",
        {
            let mut s = session("Some commented words here.");
            s.author = "Synthetic Author".into();
            select(&mut s, "commented words");
            run(&mut s, "review.newComment", json!({"text": "A synthetic remark."}));
            s.doc
        },
        Some("comments"),
    ));
    c.push((
        "commentRange",
        {
            let mut s = session("Some commented words here.");
            select(&mut s, "commented words");
            run(&mut s, "review.newComment", json!({"text": "A synthetic remark."}));
            s.doc
        },
        Some("comments"),
    ));
    c.push((
        "header",
        {
            let mut s = session("Body text.");
            run(&mut s, "insert.header", json!({"text": "Synthetic header"}));
            s.doc
        },
        Some("headersFooters"),
    ));
    c.push((
        "footer",
        {
            let mut s = session("Body text.");
            run(&mut s, "insert.footer", json!({"text": "Synthetic footer"}));
            s.doc
        },
        Some("headersFooters"),
    ));
    c.push((
        "pageField",
        {
            let mut s = session("Body text.");
            run(&mut s, "insert.pageNumber", json!({"position": "bottom", "align": "center"}));
            s.doc
        },
        Some("headersFooters"),
    ));
    c.push((
        "nestedField",
        {
            // `IF { REF { DOCPROPERTY Target } } = 0 "zero" "other"`: two levels of nesting.
            let target =
                wordcraft_doc::para::NestedField { at: 4, instr: "DOCPROPERTY Target".into(), result: "amount".into(), ..Default::default() };
            let amount = wordcraft_doc::para::NestedField {
                at: 3,
                instr: "REF amount".into(),
                result: "0".into(),
                nested: vec![target],
                ..Default::default()
            };
            let mut p = Paragraph::with_text("Total: ", CharProps::default());
            let field = InlineObject::Field {
                instr: "IF 0 = 0 \"zero\" \"other\"".into(),
                result: "zero".into(),
                locked: false,
                dirty: false,
                code: false,
                form: String::new(),
                nested: vec![amount],
            };
            let end = p.len();
            p.insert_object(end, field, &CharProps::default()).unwrap();
            let mut d = Document::new();
            d.body = vec![para_block(p)];
            d
        },
        Some("nestedFields"),
    ));
    c.push((
        "trackedChanges",
        {
            let mut s = session("Original sentence stays. Removed words go.");
            s.author = "Synthetic Reviewer".into();
            run(&mut s, "review.trackChanges", json!({"value": true}));
            select(&mut s, "Removed words");
            run(&mut s, "text.delete", json!({}));
            select(&mut s, "stays.");
            run(&mut s, "select.collapse", json!({"end": true}));
            run(&mut s, "text.insert", json!({"text": " Added text."}));
            s.doc
        },
        Some("trackedChanges"),
    ));
    c.push((
        "textBox",
        {
            let mut s = session("Text before the box. ");
            run(&mut s, "insert.textBox", json!({"text": "Synthetic boxed text"}));
            s.doc
        },
        Some("textBoxes"),
    ));
    c.push((
        "equation",
        {
            let mut s = session("An equation: ");
            run(&mut s, "insert.equation", json!({"linear": "a^2+b^2=c^2", "edit": false}));
            s.doc
        },
        Some("equations"),
    ));
    c.push((
        "contentControl",
        {
            let mut s = session("A field to fill: ");
            run(&mut s, "developer.plainText", json!({"tag": "synthetic"}));
            s.doc
        },
        Some("contentControls"),
    ));
    c.push((
        "drawing",
        {
            let mut s = session("A shape: ");
            run(&mut s, "insert.shape", json!({"kind": "rectangle", "width": 72, "height": 36}));
            s.doc
        },
        Some("drawings"),
    ));
    c.push((
        "sections",
        {
            let mut s = session("Portrait section text.");
            run(&mut s, "layout.break", json!({"kind": "nextPage"}));
            run(&mut s, "text.insert", json!({"text": "Landscape section text."}));
            run(&mut s, "layout.orientation", json!({"value": "landscape"}));
            s.doc
        },
        Some("sections"),
    ));
    c.push((
        "pageSetup",
        {
            let mut s = session("Body on A4 with wide margins.");
            run(&mut s, "layout.size", json!({"name": "A4"}));
            run(&mut s, "layout.margins", json!({"top": 90, "bottom": 90, "left": 100, "right": 100}));
            s.doc
        },
        Some("pageSetup"),
    ));
    c.push((
        "properties",
        {
            let mut d = Document::from_text("Body.");
            d.core.title = "Synthetic Title".into();
            d.core.creator = "Synthetic Creator".into();
            d
        },
        Some("properties"),
    ));
    c.push((
        "commentReply",
        {
            let mut s = session("Some commented words here.");
            s.author = "First Author".into();
            select(&mut s, "commented words");
            run(&mut s, "review.newComment", json!({"text": "A synthetic remark."}));
            let id = *s.doc.comments.keys().next().unwrap();
            s.author = "Second Author".into();
            run(&mut s, "review.reply", json!({"id": id, "text": "A synthetic answer."}));
            s.doc
        },
        Some("comments"),
    ));
    c.push((
        "firstPageHeader",
        {
            let mut d = Document::from_text("Body text.");
            let def = d.add_part(PartKind::Header, vec![para_block(Paragraph::with_text("Every page", CharProps::default()))]);
            let first = d.add_part(PartKind::Header, vec![para_block(Paragraph::with_text("Title page", CharProps::default()))]);
            d.last_section.headers.default = Some(def);
            d.last_section.headers.first = Some(first);
            d.last_section.title_page = true;
            d
        },
        Some("headersFooters"),
    ));
    c.push((
        "evenPageHeader",
        {
            let mut d = Document::from_text("Body text.");
            let def = d.add_part(PartKind::Header, vec![para_block(Paragraph::with_text("Odd pages", CharProps::default()))]);
            let even = d.add_part(PartKind::Header, vec![para_block(Paragraph::with_text("Even pages", CharProps::default()))]);
            d.last_section.headers.default = Some(def);
            d.last_section.headers.even = Some(even);
            d.settings.even_odd_headers = true;
            d
        },
        Some("headersFooters"),
    ));
    c.push((
        "sectionHeaders",
        {
            let mut d = Document::from_text("First section.\nSecond section.");
            let one = d.add_part(PartKind::Header, vec![para_block(Paragraph::with_text("Header one", CharProps::default()))]);
            let two = d.add_part(PartKind::Header, vec![para_block(Paragraph::with_text("Header two", CharProps::default()))]);
            let mut first = d.last_section.clone();
            first.headers.default = Some(one);
            if let Some(Block::Para(p)) = d.body.first_mut().map(std::sync::Arc::make_mut) {
                p.section = Some(Box::new(first));
            }
            d.last_section.headers.default = Some(two);
            d
        },
        Some("headersFooters"),
    ));
    c.push((
        "noteInTable",
        {
            let mut t = Table::new(1, 2, 400.0);
            cell_text(&mut t, 0, 0, "Cell with a note");
            cell_text(&mut t, 0, 1, "Plain cell");
            let mut s = Session::new(table_doc(t));
            select(&mut s, "with a note");
            run(&mut s, "select.collapse", json!({"end": true}));
            run(&mut s, "references.footnote", json!({"text": "Note in a cell."}));
            s.doc
        },
        Some("footnotes"),
    ));
    c.push((
        "noteFormatting",
        {
            let mut s = session("Text with a formatted note");
            run(&mut s, "references.footnote", json!({"text": "Plain then strong words."}));
            select(&mut s, "strong words");
            run(&mut s, "format.bold", json!({}));
            s.doc
        },
        Some("footnotes"),
    ));
    c.push((
        "customNoteMark",
        {
            let mut s = session("Text with a starred note");
            run(&mut s, "references.footnote", json!({"text": "Starred note.", "mark": "*"}));
            s.doc
        },
        Some("footnotes"),
    ));
    c.push((
        "moves",
        {
            let mut d = Document::from_text("Moved words then the rest.\nThe end.");
            let from = Revision { move_name: Some("move1".into()), ..Revision::new(RevisionKind::Delete, "Synthetic Mover", "2026-01-01T00:00:00Z") };
            let to = Revision { move_name: Some("move1".into()), ..Revision::new(RevisionKind::Insert, "Synthetic Mover", "2026-01-01T00:00:00Z") };
            d.revisions.push(from);
            d.revisions.push(to);
            let props = |del: bool| CharProps { del: del.then_some(0), ins: (!del).then_some(1), ..Default::default() };
            if let Some(Block::Para(p)) = d.body.get_mut(0).map(std::sync::Arc::make_mut) {
                p.format(0, "Moved words".len(), &|c| *c = props(true)).unwrap();
            }
            if let Some(Block::Para(p)) = d.body.get_mut(1).map(std::sync::Arc::make_mut) {
                let end = p.len();
                p.insert_text(end, " Moved words", &props(false)).unwrap();
            }
            d
        },
        Some("trackedChanges"),
    ));
    c.push((
        "smartTag",
        {
            let mut d = Document::new();
            let mut p = Paragraph::with_text("Meet in ", CharProps::default());
            let wrap = CharProps { wrap: Some(r#"<w:smartTag w:uri="urn:example" w:element="place"></w:smartTag>"#.into()), ..Default::default() };
            let end = p.len();
            p.insert_text(end, "Springfield", &wrap).unwrap();
            let end = p.len();
            p.insert_text(end, " today.", &CharProps::default()).unwrap();
            d.body = vec![para_block(p)];
            d
        },
        Some("formatting"),
    ));
    c.push((
        "specVanish",
        {
            let mut d = Document::new();
            let mut p = Paragraph::with_text("Hidden separator ", CharProps { spec_vanish: Some(true), hidden: Some(true), ..Default::default() });
            let end = p.len();
            p.insert_text(end, "visible text", &CharProps::default()).unwrap();
            d.body = vec![para_block(p)];
            d
        },
        Some("formatting"),
    ));
    c.push((
        "ptab",
        {
            let mut d = Document::new();
            let mut p = Paragraph::with_text("Left", CharProps::default());
            let end = p.len();
            // A positional tab is an object since upstream #2367 (`InlineObject::PositionalTab`);
            // its run keeps the settings as read (`CharProps::ptab`), as the DOCX reader gives it.
            let tab = InlineObject::PositionalTab {
                align: wordcraft_doc::para::PTabAlign::Right,
                relative_to: wordcraft_doc::para::PTabBase::Margin,
                leader: wordcraft_doc::props::TabLeader::Dot,
            };
            p.insert_object(end, tab, &CharProps { ptab: Some("margin|right|dot".into()), ..Default::default() }).unwrap();
            let end = p.len();
            p.insert_text(end, "Right", &CharProps::default()).unwrap();
            d.body = vec![para_block(p)];
            d
        },
        Some("formatting"),
    ));
    // Right-to-left paragraphs (#2693): Hebrew and Arabic text set right to left.
    c.push((
        "rtl",
        {
            let mut s = session("Latin line first.\nעברית מימין לשמאל\nمرحبا بالعالم");
            select(&mut s, "עברית");
            run(&mut s, "para.rtl", json!({"value": true}));
            select(&mut s, "مرحبا");
            run(&mut s, "para.rtl", json!({"value": true}));
            s.doc
        },
        Some("formatting"),
    ));
    // East Asian text: Japanese, Chinese (with an ideographic space U+3000 and full-width
    // punctuation) and Korean.
    c.push(("eastAsian", Document::from_text("日本語の文章です。「引用」も含む。\n中文\u{3000}全角空格和标点，句号。\n한국어 문장입니다."), None));
    // A numbered list restarted at 1 (Restart at 1) right after another.
    c.push((
        "listRestart",
        {
            let mut s = session("Intro\nOne\nTwo\nThree\nFour\nOutro");
            for t in ["One", "Two", "Three", "Four"] {
                select(&mut s, t);
                run(&mut s, "para.numbering", json!({}));
            }
            select(&mut s, "Three");
            run(&mut s, "para.restartNumbering", json!({}));
            s.doc
        },
        Some("formatting"),
    ));
    // A numbered list that goes on (3., 4.) after a paragraph that isn't part of it.
    c.push((
        "listContinue",
        {
            let mut s = session("Intro\nOne\nTwo\nMiddle paragraph.\nThree\nFour\nOutro");
            for t in ["One", "Two"] {
                select(&mut s, t);
                run(&mut s, "para.numbering", json!({}));
            }
            let num = s.doc.body.get(1).and_then(|b| b.as_para()).and_then(|p| p.props.numbering).map(|n| n.num).unwrap();
            for i in [4usize, 5] {
                if let Some(Block::Para(p)) = s.doc.body.get_mut(i).map(std::sync::Arc::make_mut) {
                    p.props.numbering = Some(wordcraft_doc::props::NumRef { num, level: 0 });
                    p.props.style = Some("ListParagraph".into());
                    p.touch();
                }
            }
            s.doc
        },
        Some("formatting"),
    ));
    // Three levels of numbering (1., a., i.) and back to the first.
    c.push((
        "nestedLists",
        {
            let mut s = session("Level one\nLevel two\nLevel three\nLevel one again");
            for (t, indent) in [("Level one", 0), ("Level two", 1), ("Level three", 2), ("Level one again", 0)] {
                select(&mut s, t);
                run(&mut s, "para.numbering", json!({}));
                for _ in 0..indent {
                    run(&mut s, "para.indent", json!({}));
                }
            }
            s.doc
        },
        None,
    ));
    // A table whose columns have widths of their own.
    c.push((
        "tableWidths",
        {
            let mut t = Table::new(1, 3, 400.0);
            t.grid = vec![80.0, 120.0, 200.0];
            for (col, x) in ["narrow", "middle", "wide"].iter().enumerate() {
                cell_text(&mut t, 0, col, x);
                if let Some(c) = t.rows.get_mut(0).and_then(|r| r.cells.get_mut(col)) {
                    c.props.width = t.grid.get(col).copied();
                }
            }
            table_doc(t)
        },
        Some("formatting"),
    ));
    // A shaded table cell.
    c.push((
        "cellShading",
        {
            let mut t = Table::new(1, 2, 400.0);
            cell_text(&mut t, 0, 0, "shaded");
            cell_text(&mut t, 0, 1, "plain");
            if let Some(c) = t.rows.get_mut(0).and_then(|r| r.cells.get_mut(0)) {
                c.props.shading = Some(wordcraft_doc::Rgb(0xDD, 0xEE, 0xFF));
            }
            table_doc(t)
        },
        Some("formatting"),
    ));
    // Space before and after, a left and a first-line indent, and 1.5 line spacing.
    c.push((
        "paraSpacing",
        {
            let mut d = Document::from_text("Plain paragraph.\nSpaced and indented paragraph.\nAnother plain one.");
            if let Some(Block::Para(p)) = d.body.get_mut(1).map(std::sync::Arc::make_mut) {
                p.props.space_before = Some(12.0);
                p.props.space_after = Some(6.0);
                p.props.indent_left = Some(36.0);
                p.props.indent_first = Some(18.0);
                p.props.line_spacing = Some(wordcraft_doc::props::LineSpacing::Multiple(1.5));
            }
            d
        },
        Some("formatting"),
    ));
    // A DATE field in the body with its last result.
    c.push((
        "bodyField",
        {
            let mut p = Paragraph::with_text("Printed on ", CharProps::default());
            let field = InlineObject::Field {
                instr: "DATE \\@ \"yyyy-MM-dd\"".into(),
                result: "2026-01-02".into(),
                locked: false,
                dirty: false,
                code: false,
                form: String::new(),
                nested: Vec::new(),
            };
            let end = p.len();
            p.insert_object(end, field, &CharProps::default()).unwrap();
            let mut d = Document::new();
            d.body = vec![para_block(p)];
            d
        },
        Some("fields"),
    ));
    // A macro project as the reader keeps one from a `.docm` (opaque bytes, never run).
    c.push((
        "macros",
        {
            let mut d = Document::from_text("A document with macros.");
            d.passthrough.insert("word/vbaProject.bin".into(), std::sync::Arc::new(vec![0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1, 1, 2, 3]));
            d
        },
        Some("macros"),
    ));
    c
}

// ---------------------------------------------------------------------------------------------
// The harness

fn reopen(doc: &Document, ext: &str) -> Result<Document, String> {
    let (name, opts) = save_as(ext);
    let bytes = io::save_bytes_opts(&name, doc, &opts)?;
    io::open_bytes(&name, &bytes)
}

/// Per feature and format: (kept on save, kept via DOCX, stable on a second save).
type Matrix = BTreeMap<(&'static str, &'static str), (bool, bool, bool)>;

fn matrix() -> (Matrix, Vec<(&'static str, Document, Option<&'static str>)>) {
    let corpus = corpus();
    let mut m = Matrix::new();
    for (f, doc, _) in &corpus {
        let want = fingerprint(f, doc);
        assert!(!want.is_empty() && !want.contains("unknown"), "{f}: the corpus document doesn't show it: {want:?}");
        let via = reopen(doc, "docx").unwrap();
        // FIDELITY_DUMP=dir writes each saved file there, to look at with other programs.
        if let Ok(dir) = std::env::var("FIDELITY_DUMP") {
            for ext in FORMATS {
                if let Ok(b) = io::save_bytes(&format!("x.{ext}"), doc) {
                    let _ = std::fs::write(format!("{dir}/{f}.{ext}"), b);
                }
            }
        }
        for ext in FORMATS {
            let once = reopen(doc, ext).unwrap_or_else(|e| panic!("{f} as {ext}: {e}"));
            let kept = fingerprint(f, &once) == want;
            let via_docx = reopen(&via, ext).map(|d| fingerprint(f, &d) == want).unwrap_or(false);
            let stable = reopen(&once, ext).map(|d| fingerprint(f, &d) == fingerprint(f, &once)).unwrap_or(false);
            m.insert((f, ext), (kept, via_docx, stable));
        }
    }
    (m, corpus)
}

fn print(m: &Matrix, corpus: &[(&'static str, Document, Option<&'static str>)]) -> String {
    let mut s = String::from("| feature |");
    for ext in FORMATS {
        let _ = write!(s, " {ext} |");
    }
    s.push_str("\n|---|");
    for _ in FORMATS {
        s.push_str("---|");
    }
    s.push('\n');
    for (f, _, _) in corpus {
        let _ = write!(s, "| {f} |");
        for ext in FORMATS {
            let (a, b, c) = m.get(&(*f, *ext)).copied().unwrap_or_default();
            let cell = match (a, b, c) {
                (true, true, true) => "kept",
                (false, _, true) => "lost",
                (true, _, false) | (false, _, false) => "unstable",
                (true, false, true) => "kept (lost via DOCX)",
            };
            let _ = write!(s, " {cell} |");
        }
        s.push('\n');
    }
    s
}

/// Features each format is known still to lose (save → reopen). The list only shrinks: a
/// format that starts keeping a feature fails the test until it's taken off.
const STILL_LOST: &[(&str, &[&str])] = &[
    // A `.docx` can't hold a macro project (a `.docm` can: `macro_packages.rs`).
    ("docx", &["macros"]),
    ("json", &[]),
    ("odt", &["nestedField", "equation", "contentControl", "drawing", "commentReply", "moves", "smartTag", "specVanish", "ptab", "macros"]),
    (
        "rtf",
        &[
            "nestedField",
            "equation",
            "contentControl",
            "drawing",
            "commentReply",
            "customNoteMark",
            "moves",
            "smartTag",
            "specVanish",
            "ptab",
            "macros",
        ],
    ),
    (
        "wordml2003",
        &[
            "textBox",
            "equation",
            "contentControl",
            "drawing",
            "commentReply",
            "evenPageHeader",
            "customNoteMark",
            "moves",
            "smartTag",
            "ptab",
            "macros",
        ],
    ),
    (
        "html",
        &[
            "nestedField",
            "comment",
            "commentRange",
            "header",
            "footer",
            "pageField",
            "trackedChanges",
            "textBox",
            "contentControl",
            "drawing",
            "sections",
            "commentReply",
            "firstPageHeader",
            "evenPageHeader",
            "sectionHeaders",
            "moves",
            "smartTag",
            "specVanish",
            "ptab",
            "bodyField",
            "macros",
        ],
    ),
    (
        "md",
        &[
            "nestedField",
            "lineBreakTab",
            "underline",
            "superscript",
            "subscript",
            "color",
            "highlight",
            "font",
            "size",
            "alignment",
            "nestedTable",
            "mergedCells",
            "picture",
            "pageBreak",
            "endnote",
            "comment",
            "commentRange",
            "header",
            "footer",
            "pageField",
            "trackedChanges",
            "textBox",
            "equation",
            "contentControl",
            "drawing",
            "sections",
            "pageSetup",
            "properties",
            "commentReply",
            "firstPageHeader",
            "evenPageHeader",
            "sectionHeaders",
            "customNoteMark",
            "moves",
            "smartTag",
            "specVanish",
            "ptab",
            "rtl",
            "tableWidths",
            "cellShading",
            "paraSpacing",
            "bodyField",
            "macros",
        ],
    ),
];

#[test]
fn fidelity_matrix() {
    let (m, corpus) = matrix();
    let table = print(&m, &corpus);
    println!("{table}");
    let mut problems = Vec::new();
    for ext in FORMATS {
        let expected: &[&str] = STILL_LOST.iter().find(|(e, _)| e == ext).map(|(_, l)| *l).unwrap_or(&[]);
        for (f, doc, lossy_id) in &corpus {
            let (kept, _, stable) = m.get(&(*f, *ext)).copied().unwrap_or_default();
            let expect_lost = expected.contains(f);
            if kept == expect_lost {
                problems.push(format!("{ext}/{f}: {} (update STILL_LOST)", if kept { "now kept" } else { "lost" }));
            }
            if kept && !stable {
                problems.push(format!("{ext}/{f}: kept once but changes when saved again"));
            }
            // The save prompt names exactly what is lost.
            if let Some(id) = lossy_id {
                let (name, opts) = save_as(ext);
                let named = io::lossy_features_with(&name, doc, &opts).is_some_and(|l| l.contains(id));
                if named == kept {
                    problems.push(format!("{ext}/{f}: lossy_features {} `{id}`", if named { "names" } else { "doesn't name" }));
                }
            }
        }
    }
    assert!(problems.is_empty(), "{}\n{table}", problems.join("\n"));
}

/// Saves whose bytes are known to differ from the save of the file they made when it is opened
/// and saved again (the pair is (format, feature)); the list only shrinks. Each is one of:
/// the format drops the feature and the reopened document writes without it (text boxes,
/// drawings, moves, nested fields, pictures in Markdown, a custom note mark, smart tags and
/// position tabs in Word 2003 XML); a DATE field is updated when the file opens (`bodyField`);
/// RTF has no styles of its own, so a heading's spacing and a list's indents read back as the
/// paragraph's (`headings`, `lists`…); HTML header cells read back
/// bold (`table`), its `&nbsp;` as a space (`trackedChanges`), and a trailing space is trimmed
/// (`textBox`, `drawing`, `moves`); an ODT table width is rounded (`mergedCells`).
const REOPEN_DIFFERS: &[(&str, &str)] = &[
    ("rtf", "headings"),
    ("rtf", "nestedField"),
    ("rtf", "bodyField"),
    ("html", "table"),
    ("html", "mergedCells"),
    ("html", "trackedChanges"),
    ("html", "textBox"),
    ("html", "drawing"),
    ("html", "moves"),
    ("md", "picture"),
    ("md", "textBox"),
    ("md", "drawing"),
    ("md", "moves"),
    ("odt", "mergedCells"),
    ("odt", "drawing"),
    ("wordml2003", "textBox"),
    ("wordml2003", "drawing"),
    ("wordml2003", "customNoteMark"),
    ("wordml2003", "moves"),
    ("wordml2003", "smartTag"),
    ("wordml2003", "ptab"),
    ("odt", "bodyField"),
    ("wordml2003", "bodyField"),
    ("docx", "bodyField"),
    ("json", "bodyField"),
];

/// Saves known to change again at every reopen and save (the file grows). None since the
/// upstream merge brought 00cd3f33 (#2667: RTF list items no longer gain a space each round).
const RESAVE_GROWS: &[(&str, &str)] = &[];

/// Round-trip stability, byte for byte: saving the same document twice gives the same bytes
/// (nothing in a file depends on the clock: dates come from the document), the save of a file
/// opened again is the same as that file (so a second save changes nothing, except where
/// [`REOPEN_DIFFERS`] says), and the save of *that* opened again is the same again.
#[test]
fn every_format_saves_the_same_bytes_twice_and_after_a_reopen() {
    let mut problems = Vec::new();
    for (f, doc, _) in corpus() {
        for ext in FORMATS {
            let (name, opts) = save_as(ext);
            let a = io::save_bytes_opts(&name, &doc, &opts).unwrap();
            let b = io::save_bytes_opts(&name, &doc, &opts).unwrap();
            if a != b {
                problems.push(format!("{f} as {ext}: two saves of the document differ"));
            }
            let once = io::open_bytes(&name, &a).unwrap();
            let c = io::save_bytes_opts(&name, &once, &opts).unwrap();
            let twice = io::open_bytes(&name, &c).unwrap();
            let d = io::save_bytes_opts(&name, &twice, &opts).unwrap();
            let grows = RESAVE_GROWS.contains(&(ext, f));
            if (c != d) != grows {
                problems.push(format!(
                    "{f} as {ext}: the save of a reopened file {} when opened and saved again (update RESAVE_GROWS)",
                    if grows { "no longer changes" } else { "changes" }
                ));
            }
            let known = REOPEN_DIFFERS.contains(&(ext, f));
            if (a != c) != known {
                problems.push(format!(
                    "{f} as {ext}: the save after a reopen {} the save (update REOPEN_DIFFERS)",
                    if known { "equals" } else { "differs from" }
                ));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// Word documents and WordCraft's JSON keep the whole corpus: the document read back equals
/// the document read back after a second save.
#[test]
fn docx_and_json_round_trips_are_stable_for_the_whole_corpus() {
    for (f, doc, _) in corpus() {
        for ext in ["docx", "json"] {
            let once = reopen(&doc, ext).unwrap();
            let twice = reopen(&once, ext).unwrap();
            assert_eq!(once.body, twice.body, "{f} as {ext}: body");
            assert_eq!(*once.parts, *twice.parts, "{f} as {ext}: parts");
            assert_eq!(*once.comments, *twice.comments, "{f} as {ext}: comments");
            assert_eq!(once.last_section, twice.last_section, "{f} as {ext}: section");
        }
    }
}

/// Files another program wrote from the corpus (`FIDELITY_FOREIGN=dir` holding `<feature>.odt`
/// and `<feature>.rtf`): each opens with the feature the corpus document has. Not run by
/// default: the files aren't part of the repository.
#[test]
#[ignore]
fn foreign_files_keep_the_corpus_features() {
    let Ok(dir) = std::env::var("FIDELITY_FOREIGN") else { return };
    let mut problems = Vec::new();
    for (f, doc, _) in corpus() {
        for ext in ["odt", "rtf"] {
            let Ok(bytes) = std::fs::read(format!("{dir}/{f}.{ext}")) else { continue };
            match io::open_bytes(&format!("x.{ext}"), &bytes) {
                Ok(d) => {
                    let (want, got) = (fingerprint(f, &doc), fingerprint(f, &d));
                    println!("{f}.{ext}: {}", if want == got { "kept".to_string() } else { format!("want {want:?} got {got:?}") });
                    if want != got {
                        problems.push(format!("{f}.{ext}"));
                    }
                }
                Err(e) => problems.push(format!("{f}.{ext}: {e}")),
            }
        }
    }
    assert!(problems.is_empty(), "{problems:?}");
}

/// The shared document limits (`wordcraft_doc::limits`): the saved-size estimate bounds what the
/// Word writer emits for every feature of the corpus. What a document adds to an empty
/// document's parts is within what it adds to the estimate (so the estimate's allowance for the
/// fixed parts doesn't hide a feature it undercounts), and its JSON (what a recovery copy holds)
/// within the estimate. The readers' byte limits follow from the estimate's limit, so a document
/// within the limits saves into a file that opens again.
#[test]
fn the_saved_size_estimate_bounds_every_feature() {
    use wordcraft_doc::limits::Tally;
    fn written(doc: &Document) -> u64 {
        let bytes = wordcraft_docx::write(doc).unwrap();
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        (0..z.len()).map(|i| z.by_index(i).unwrap().size()).sum()
    }
    let empty = Document::new();
    let (empty_written, empty_estimate) = (written(&empty), Tally::of(&empty).saved_bytes);
    let mut problems = Vec::new();
    let mut tightest = (f64::MAX, "");
    for (f, doc, _) in corpus() {
        let t = Tally::of(&doc);
        let added = written(&doc).saturating_sub(t.media_bytes).saturating_sub(empty_written);
        let estimated = t.saved_bytes.saturating_sub(empty_estimate);
        if added > estimated {
            problems.push(format!("{f}: adds {added} bytes written, {estimated} estimated"));
        }
        if added > 0 && (estimated as f64 / added as f64) < tightest.0 {
            tightest = (estimated as f64 / added as f64, f);
        }
        let mut text = doc.clone();
        text.media = Default::default();
        text.passthrough = Default::default();
        let json = serde_json::to_vec(&text).unwrap().len() as u64;
        if json > t.saved_bytes {
            problems.push(format!("{f}: {json} bytes of JSON, {} estimated", t.saved_bytes));
        }
    }
    println!("tightest: {} at {:.2}x", tightest.1, tightest.0);
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// Writes each feature's first save, the save after a reopen and the one after a second reopen
/// into the folder `REOPEN_DUMP` names, to look at what differs (`<feature>.<ext>.a|c|d`). Not
/// run by default.
#[test]
#[ignore]
fn dump_reopen_saves() {
    let Ok(dir) = std::env::var("REOPEN_DUMP") else { return };
    for (f, doc, _) in corpus() {
        for ext in FORMATS {
            let (name, opts) = save_as(ext);
            let a = io::save_bytes_opts(&name, &doc, &opts).unwrap();
            let once = io::open_bytes(&name, &a).unwrap();
            let c = io::save_bytes_opts(&name, &once, &opts).unwrap();
            let twice = io::open_bytes(&name, &c).unwrap();
            let d = io::save_bytes_opts(&name, &twice, &opts).unwrap();
            for (tag, bytes) in [("a", &a), ("c", &c), ("d", &d)] {
                let _ = std::fs::write(format!("{dir}/{f}.{ext}.{tag}"), bytes);
            }
        }
    }
}
