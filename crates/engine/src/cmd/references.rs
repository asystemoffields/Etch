//! References tab: table of contents, footnotes/endnotes, captions; field updates.

use serde_json::{Value, json};
use wordcraft_doc::numbering::{Counters, LevelSuffix};
use wordcraft_doc::para::{CodePiece, InlineObject, MAX_NESTED_DEPTH, NestedField, NoteKind, OBJ, code_pieces};
use wordcraft_doc::props::{CharProps, TabAlign, TabLeader, TabStop};
use wordcraft_doc::section::NumFormat;
use wordcraft_doc::{Block, Paragraph, PartKind, Path, Pos, StoryRef, para_block};

use super::{delete_selection, sel_result};
use crate::{CmdError, CmdResult, CommandSpec, Selection, Session, p};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("references.toc", "Table of Contents", "References › Table of Contents", insert_toc).params(
            r#"{"levels"?: 1-9, "title"?: string, "pageNumbers"?: bool, "rightAlign"?: bool, "tabLeader"?: "dot|hyphen|underscore|none", "hyperlinks"?: bool, "headingStyles"?: bool, "outlineLevels"?: bool, "tcFields"?: bool, "styleLevels"?: {style name: 1-9}} (replaces an existing table; {} updates it)"#,
        ),
        CommandSpec::new("references.updateToc", "Update Table", "References › Table of Contents", |s, _| {
            update_toc(s)?;
            sel_result(s)
        }),
        CommandSpec::new("references.removeToc", "Remove Table of Contents", "References › Table of Contents", |s, _| {
            // A TOC read as a range field is the engine's TOC first, as Update Table makes it.
            range_toc_to_field(s)?;
            if let Some((start, end)) = toc_range(s) {
                // With the building-block control around it (#2427), as Word removes the control.
                let (start, end) = toc_control_span(s, start, end).unwrap_or((start, end));
                for i in (start..=end).rev() {
                    s.doc.remove_block(StoryRef::Body, &Path::top(i))?;
                }
                s.clamp_selection();
            }
            sel_result(s)
        }),
        CommandSpec::new("references.addText", "Add Text", "References › Table of Contents", |s, v| {
            let level = p::u64(v, "level").unwrap_or(0).min(9) as u8;
            super::para::fmt(s, &|pp| pp.outline_level = if level == 0 { Some(9) } else { Some(level - 1) })
        })
        .params(r#"{"level": 0 (do not show) | 1-9}"#),
        CommandSpec::new("references.footnote", "Insert Footnote", "References › Footnotes", |s, v| note(s, v, NoteKind::Footnote))
            .key("Mod+Alt+F")
            .params(r#"{"text"?: string, "mark"?: string (a custom mark instead of the number)}"#),
        CommandSpec::new("references.endnote", "Insert Endnote", "References › Footnotes", |s, v| note(s, v, NoteKind::Endnote))
            // macOS takes ⌘⌥D to show and hide the Dock; Word for Mac uses ⌘⌥E.
            .key(super::mac_or("Mod+Alt+E", "Mod+Alt+D"))
            .params(r#"{"text"?: string, "mark"?: string (a custom mark instead of the number)}"#),
        CommandSpec::new("references.convertNotes", "Convert Notes", "References › Footnotes", |s, v| {
            let (foot, end) = match p::req_str(v, "to")? {
                "endnote" | "endnotes" => (true, false),
                "footnote" | "footnotes" => (false, true),
                "swap" => (true, true),
                t => return Err(CmdError::Params(format!("unknown `to` {t:?}; use endnote, footnote or swap"))),
            };
            let n = s.doc.convert_notes(foot, end);
            Ok(json!({"converted": n}))
        })
        .params(r#"{"to": "endnote" (footnotes become endnotes) | "footnote" (endnotes become footnotes) | "swap"}"#),
        CommandSpec::new("references.nextFootnote", "Next Footnote", "References › Footnotes", |s, _| {
            let caret = s.sel.focus.clone();
            let refs = note_refs(s, Some(NoteKind::Footnote));
            if let Some(p) = refs.iter().find(|p| **p > caret).or(refs.first()) {
                s.sel = Selection::caret(p.clone());
            }
            sel_result(s)
        })
        .key(super::non_mac("Alt+Shift+> / Alt+Shift+."))
        .pure(),
        CommandSpec::new("references.notes", "Show Notes", "References › Footnotes", |s, _| {
            let mut out = Vec::new();
            for (id, part) in &s.doc.parts {
                if matches!(part.kind, PartKind::Footnote | PartKind::Endnote) {
                    out.push(json!({"id": id, "kind": part.kind, "text": s.doc.plain_text(StoryRef::Part(*id))}));
                }
            }
            Ok(Value::Array(out))
        })
        .pure(),
        CommandSpec::new("references.caption", "Insert Caption", "References › Captions", caption)
            .params(r#"{"label"?: "Figure|Table|Equation|any label", "text"?: string, "position"?: "below|above", "excludeLabel"?: bool, "format"?: "ARABIC|alphabetic|ALPHABETIC|roman|ROMAN", "chapter"?: heading level 1-9, "separator"?: "-|.|:|—|–"}"#),
        // F9: update fields; the selected ones that showed their code show their result again.
        CommandSpec::new("references.updateFields", "Update Field", "References", |s, v| {
            update_fields_in(s, p::bool(v, "all").unwrap_or(false))?;
            super::fields::show_results(s)?;
            sel_result(s)
        })
        .key("F9")
        .params(r#"{"all"?: bool (every story: headers, footers, notes, comments and text boxes too, as before printing)}"#),
    ]
}

/// A field instruction split into its name, positional arguments and switches (ECMA-376
/// §17.16.1): `TOC \o "1-3" \h` → name `TOC`, switches `o` = `1-3` and `h`. Quoted text stays
/// whole (without the quotes; `\"` and `\\` inside it are a quote and a backslash). A switch
/// takes the word after it as its argument unless that word is another switch.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FieldCode {
    pub name: String,
    pub args: Vec<String>,
    /// Switch letter (lower-cased; `*`, `@`, `#` for the general switches) and argument.
    pub switches: Vec<(char, Option<String>)>,
}

impl FieldCode {
    pub fn parse(instr: &str) -> FieldCode {
        let mut words: Vec<(String, bool)> = Vec::new();
        let mut chars = instr.chars().peekable();
        while let Some(&c) = chars.peek() {
            if c.is_whitespace() {
                chars.next();
            } else if c == '"' {
                chars.next();
                let mut w = String::new();
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' if matches!(chars.peek(), Some('"' | '\\')) => w.extend(chars.next()),
                        c => w.push(c),
                    }
                }
                words.push((w, true));
            } else {
                let mut w = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_whitespace() || (c == '"' && !w.is_empty()) {
                        break;
                    }
                    w.push(c);
                    chars.next();
                }
                words.push((w, false));
            }
        }
        let mut words = words.into_iter().peekable();
        let mut f =
            FieldCode { name: words.next().map(|(w, _)| w.trim_start_matches('=').to_ascii_uppercase()).unwrap_or_default(), ..Default::default() };
        while let Some((w, quoted)) = words.next() {
            let sw = if quoted { None } else { w.strip_prefix('\\').and_then(|r| r.chars().next()) };
            match sw {
                Some(c) => {
                    let arg = match words.peek() {
                        Some((n, q)) if *q || !n.starts_with('\\') => words.next().map(|(n, _)| n),
                        _ => None,
                    };
                    f.switches.push((c.to_ascii_lowercase(), arg));
                }
                None => f.args.push(w),
            }
        }
        f
    }

    pub fn has(&self, c: char) -> bool {
        self.switches.iter().any(|(k, _)| *k == c)
    }

    /// The argument of switch `c`, if the switch is there with one.
    pub fn arg(&self, c: char) -> Option<&str> {
        self.switches.iter().find(|(k, _)| *k == c).and_then(|(_, a)| a.as_deref())
    }
}

/// A level range such as `1-3` (or one level, `2`), clamped to 1–9.
pub fn level_range(s: &str) -> Option<(u8, u8)> {
    let mut it = s.split('-').map(|x| x.trim().parse::<u8>().ok());
    let a = it.next().flatten()?.clamp(1, 9);
    let b = match it.next() {
        Some(b) => b?.clamp(1, 9),
        None => a,
    };
    Some((a.min(b), a.max(b)))
}

/// The tab leader named in a command parameter.
pub fn leader(name: &str) -> TabLeader {
    match name {
        "none" => TabLeader::None,
        "hyphen" => TabLeader::Hyphen,
        "underscore" => TabLeader::Underscore,
        "middleDot" => TabLeader::MiddleDot,
        _ => TabLeader::Dot,
    }
}

/// The TOC field instruction for `references.toc`'s parameters (ECMA-376 §17.16.5.68): `\o`
/// heading styles, `\h` hyperlinks, `\z` hide tab and page numbers in Web view, `\u` outline
/// levels, `\f`/`\l` TC fields, `\n` without page numbers, `\p` separator, `\t` styles with levels.
pub fn toc_instr(v: &Value) -> String {
    let levels = p::u64(v, "levels").unwrap_or(3).clamp(1, 9);
    let on = |k: &str, d: bool| p::bool(v, k).unwrap_or(d);
    let mut s = String::from("TOC");
    if on("headingStyles", true) {
        s.push_str(&format!(" \\o \"1-{levels}\""));
    }
    if on("hyperlinks", true) {
        s.push_str(" \\h");
    }
    s.push_str(" \\z");
    if on("outlineLevels", true) {
        s.push_str(" \\u");
    }
    if on("tcFields", false) {
        s.push_str(&format!(" \\f \\l \"1-{levels}\""));
    }
    if !on("pageNumbers", true) {
        s.push_str(&format!(" \\n \"1-{levels}\""));
    } else if !on("rightAlign", true) {
        s.push_str(" \\p \" \"");
    }
    let clean = |n: &str| n.chars().filter(|c| !matches!(c, '"' | ',' | '\\')).take(64).collect::<String>().trim().to_string();
    let pairs: Vec<String> = match v.get("styleLevels") {
        Some(Value::Object(m)) => m
            .iter()
            .take(64)
            .filter_map(|(name, l)| Some((clean(name), l.as_u64()?.clamp(1, 9))))
            .filter(|(n, _)| !n.is_empty())
            .map(|(n, l)| format!("{n},{l}"))
            .collect(),
        _ => Vec::new(),
    };
    if !pairs.is_empty() {
        s.push_str(&format!(" \\t \"{}\"", pairs.join(",")));
    }
    s
}

/// A TOC entry: the paragraph it points at, its level (0-based), its text, and the leading page
/// breaks of that paragraph (its page is where the text after them lands).
struct TocEntry {
    path: Path,
    level: u8,
    text: String,
    lead: usize,
}

/// The headings of the body, in document order, as (body block, level 1–9, text, leading page
/// breaks): what Insert › Link › Place in This Document lists.
pub(crate) fn headings(s: &Session) -> Vec<(usize, u8, String, usize)> {
    let f = FieldCode::parse("TOC \\o \"1-9\" \\u");
    toc_entries(s, &f).into_iter().filter(|e| e.path.depth() == 0).map(|e| (e.path.last(), e.level.saturating_add(1), e.text, e.lead)).collect()
}

/// Visit the paragraphs of `story` in document order, each with its path (the components a
/// [`Path`] holds), without making a `Path` for each: in a long document most paragraphs have
/// nothing to update. Tables nest as deep as [`wordcraft_doc::Document::para_paths`] goes.
pub(crate) fn visit_paras(doc: &wordcraft_doc::Document, story: StoryRef, f: &mut dyn FnMut(&[u32], &Paragraph)) {
    fn walk(bl: &wordcraft_doc::Blocks, prefix: &mut Vec<u32>, depth: usize, f: &mut dyn FnMut(&[u32], &Paragraph)) {
        if depth > 16 {
            return;
        }
        for (i, b) in bl.iter().enumerate() {
            prefix.push(u32::try_from(i).unwrap_or(u32::MAX));
            match &**b {
                Block::Para(p) => f(prefix, p),
                Block::Table(t) => {
                    for (r, row) in t.rows.iter().enumerate() {
                        for (c, cell) in row.cells.iter().enumerate() {
                            prefix.push(u32::try_from(r).unwrap_or(u32::MAX));
                            prefix.push(u32::try_from(c).unwrap_or(u32::MAX));
                            walk(&cell.blocks, prefix, depth + 1, f);
                            prefix.pop();
                            prefix.pop();
                        }
                    }
                }
            }
            prefix.pop();
        }
    }
    if let Some(bl) = doc.story(story) {
        walk(bl, &mut Vec::new(), 0, f);
    }
}

/// The paths of the paragraphs of `story` that `keep` picks, in document order.
pub(crate) fn paras_where(doc: &wordcraft_doc::Document, story: StoryRef, keep: &dyn Fn(&Paragraph) -> bool) -> Vec<Path> {
    let mut out = Vec::new();
    visit_paras(doc, story, &mut |path, p| {
        if keep(p) {
            out.push(Path(path.to_vec()));
        }
    });
    out
}

/// Whether `p` holds an unlocked simple field.
fn has_unlocked_field(p: &Paragraph) -> bool {
    p.objects.iter().any(|o| matches!(o, InlineObject::Field { locked: false, .. }))
}

/// The heading level (1–9) of a built-in heading style.
fn heading_style_level(p: &Paragraph) -> Option<u8> {
    p.props.style.as_deref()?.strip_prefix("Heading")?.trim().parse::<u8>().ok().filter(|l| (1..=9).contains(l))
}

/// The entries a TOC field collects from the body, in document order: built-in heading styles
/// in the `\o` range, paragraphs with an outline level (`\u`), styles listed in `\t`, and TC
/// fields (`\f`, levels `\l`).
fn toc_entries(s: &Session, f: &FieldCode) -> Vec<TocEntry> {
    let o = f.has('o').then(|| f.arg('o').and_then(level_range).unwrap_or((1, 9)));
    let max = o.map(|r| r.1).unwrap_or(9);
    let styles: Vec<(String, u8)> = f
        .arg('t')
        .map(|t| {
            let parts: Vec<&str> = t.split([',', ';']).map(str::trim).collect();
            parts.chunks(2).filter_map(|c| Some((c.first()?.to_string(), c.get(1)?.parse::<u8>().ok()?.clamp(1, 9)))).collect()
        })
        .unwrap_or_default();
    let tc = f.has('f').then(|| f.arg('f').unwrap_or("").to_ascii_uppercase());
    let tc_levels = f.arg('l').and_then(level_range).unwrap_or((1, 9));
    let mut v = Vec::new();
    // Every numbered paragraph advances the list counters, so an entry carries the number its
    // heading shows (`1.1`), followed by the level's separator, as Word writes it.
    let mut counters = Counters::default();
    // What each paragraph style gives (its numbering, its outline level), resolved once.
    let mut by_style_props: std::collections::HashMap<Option<String>, wordcraft_doc::resolve::ResolvedPara> = Default::default();
    visit_paras(&s.doc, StoryRef::Body, &mut |prefix, p| {
        let style_props = by_style_props
            .entry(p.props.style.clone())
            .or_insert_with(|| s.doc.styles.resolve_para(&wordcraft_doc::ParaProps { style: p.props.style.clone(), ..Default::default() }));
        let number = p
            .props
            .numbering
            .or(style_props.numbering)
            .filter(|n| n.num != 0)
            .and_then(|n| counters.next_label(&s.doc.numbering, n.num, n.level))
            .filter(|(label, def)| !label.is_empty() && !matches!(def.format, NumFormat::Bullet | NumFormat::None))
            .map(|(label, def)| match def.suffix {
                LevelSuffix::Tab => format!("{label}\t"),
                LevelSuffix::Space => format!("{label} "),
                LevelSuffix::Nothing => label,
            });
        // Headings in table cells are entries too (#2482).
        let path = Path(prefix.to_vec());
        let style = p.props.style.as_deref().unwrap_or("Normal");
        if style.starts_with("TOC") {
            return;
        }
        let lead = p.text.bytes().take_while(|b| *b == 0x0C).count();
        let by_style =
            styles.iter().find(|(n, _)| s.doc.styles.get(style).is_some_and(|st| st.name.eq_ignore_ascii_case(n)) || n == style).map(|(_, l)| *l);
        // `Some(9)` direct outline level is body text: "Do Not Show in Table of Contents".
        let by_outline = f.has('u').then_some(p.props.outline_level).flatten().filter(|l| *l < 9).map(|l| l + 1).filter(|l| *l <= max);
        let hidden = f.has('u') && p.props.outline_level == Some(9);
        let style_outline = style_props.outline_level;
        let by_heading = o.and_then(|(a, b)| {
            let l = heading_style_level(p).or_else(|| style_outline.map(|l| l + 1))?;
            (a..=b).contains(&l).then_some(l)
        });
        if let Some(l) = by_style.or(by_outline).or(if hidden { None } else { by_heading }) {
            let t = p.plain_text().replace(['\u{000C}', '\n', '\t'], " ").trim().to_string();
            if !t.is_empty() {
                let text = number.map_or_else(|| t.clone(), |n| format!("{n}{t}"));
                v.push(TocEntry { path: path.clone(), level: l - 1, text, lead });
            }
        }
        if let Some(id) = &tc {
            for off in p.object_offsets() {
                let Some(InlineObject::Field { instr, .. }) = p.object_at(off) else { continue };
                let c = FieldCode::parse(instr);
                if c.name != "TC" {
                    continue;
                }
                let kind = c.arg('f').unwrap_or("C").to_ascii_uppercase();
                let l = c.arg('l').and_then(|l| l.trim().parse::<u8>().ok()).unwrap_or(1).clamp(1, 9);
                let text = c.args.first().map(|t| t.trim().to_string()).unwrap_or_default();
                if (id.is_empty() || *id == kind) && (tc_levels.0..=tc_levels.1).contains(&l) && !text.is_empty() {
                    v.push(TocEntry { path: path.clone(), level: l - 1, text, lead: off });
                }
            }
        }
    });
    v
}

/// The TOC block range in the body: the paragraph holding the TOC field and the TOC entries after
/// it. A TOC with `\c` is a table of figures, kept by the generated lists.
fn toc_range(s: &Session) -> Option<(usize, usize)> {
    let start = s.doc.body.iter().position(|b| b.as_para().is_some_and(|p| toc_field(p).is_some()))?;
    let mut end = start;
    while let Some(Block::Para(p)) = s.doc.body.get(end + 1).map(|b| &**b) {
        if p.props.style.as_deref().is_some_and(|st| st.starts_with("TOC") && st != "TOCHeading") {
            end += 1;
        } else {
            break;
        }
    }
    Some((start, end))
}

/// The body blocks of the TOC building-block control (`w:docPartObj` "Table of Contents", read
/// from a file and kept, #2427) around the TOC at blocks `start..=end`: from the block where it
/// opens, at the start of a paragraph, to the block where it closes, at the end of one. `None`
/// when no such control is around it, or it doesn't hold whole blocks.
fn toc_control_span(s: &Session, start: usize, end: usize) -> Option<(usize, usize)> {
    use wordcraft_doc::control::{ControlEvent, ControlKind, control_events, pair_events};
    let events = control_events(&s.doc.body);
    let (pairs, _) = pair_events(&events);
    pairs.iter().find_map(|(o, c, _)| {
        let (Some(ControlEvent::Open { block: a, off, control }), Some(close)) = (events.get(*o), events.get(*c)) else { return None };
        let toc = matches!(&control.kind, ControlKind::Gallery { list: false, gallery, .. } if gallery == "Table of Contents");
        let b = close.block();
        let whole_start = off.is_none_or(|off| off == 0);
        let whole_end = match close.off() {
            None => true,
            Some(off) => {
                s.doc.body.get(b).and_then(|blk| blk.as_para()).is_some_and(|p| off.checked_add(OBJ.len_utf8()).is_some_and(|e| e == p.len()))
            }
        };
        (toc && whole_start && whole_end && *a <= start && b >= end).then_some((*a, b))
    })
}

/// The caret is in the table of contents (its heading or an entry), where Word's context menu
/// offers Update Field for the whole table.
pub fn caret_in_toc(s: &Session) -> bool {
    let at = &s.sel.focus;
    if at.story != StoryRef::Body || at.path.depth() != 0 {
        return false;
    }
    if toc_range(s).is_some_and(|(start, end)| (start..=end).contains(&at.path.last())) {
        return true;
    }
    // A saved or imported TOC is a range field spanning its entries until Update Table turns it
    // into the engine's TOC (#2182, a save and reopen; not the flow in the report). Only the
    // range Update Table would convert counts, so the menu never refreshes a different TOC.
    s.doc.field_ranges(StoryRef::Body).into_iter().find(|r| convertible_toc_range(s, r)).is_some_and(|r| {
        // The caret at the field's own marker (offset 0 of the first entry) and at the end of
        // the last entry belong to the range as well.
        (r.start.path.last()..=r.end.path.last()).contains(&at.path.last())
    })
}

/// The range field `range_toc_to_field` converts: an unlocked, top-level TOC (not a table of
/// figures) when the body has no engine TOC yet.
fn convertible_toc_range(s: &Session, r: &wordcraft_doc::FieldRange) -> bool {
    let field = FieldCode::parse(&r.instr);
    toc_range(s).is_none()
        && field.name == "TOC"
        && !field.has('c')
        && r.depth == 0
        && !r.locked
        && r.start.path.depth() == 0
        && r.end.path.depth() == 0
}

/// A TOC imported as a range field (`FieldStart`…`FieldEnd`: one without a TOC heading, or with
/// content around it the engine's TOC can't reproduce) becomes the engine's TOC before an update,
/// so Update Table refreshes it rather than adding a second one: an empty `TOC` field where the
/// range started, its old result gone, and the regenerated entries after that paragraph (text
/// that followed the range moves below them). Only an unlocked, top-level TOC that isn't inside
/// another range field, and only when the body has no engine TOC already.
fn range_toc_to_field(s: &mut Session) -> Result<(), CmdError> {
    if toc_range(s).is_some() {
        return Ok(());
    }
    let Some(r) = s.doc.field_ranges(StoryRef::Body).into_iter().find(|r| convertible_toc_range(s, r)) else {
        return Ok(());
    };
    let Some(after_end) = r.end.off.checked_add(OBJ.len_utf8()) else { return Ok(()) };
    let end = Pos { off: after_end, ..r.end.clone() };
    let props = s.doc.para_at(&r.start).map(|p| p.props_of_char(r.start.off).clone()).unwrap_or_default();
    let mut at = s.doc.delete_range(&r.start, &end)?;
    // A range that filled its own paragraphs right under a TOC heading (as Word and other writers
    // lay it out) leaves an empty paragraph behind, which would show as a blank line above the
    // entries. The field goes at the end of the heading instead, as `insert_toc` places it. A
    // paragraph left holding only the end of a content control around the TOC (its building-block
    // control, #2427) stays: as an entry, the new entries take its place and carry the marker
    // (`replace_entries`).
    // The same when the paragraph before is an empty one: what the writer leaves where the
    // engine's TOC field was when there is no TOC heading (it writes the field in the first
    // entry); a new field paragraph each time added a blank line above the TOC at every reopen
    // and update.
    let i = at.path.last();
    // (Empty but for the start of the TOC's building-block control, as a reopened TOC is: #2427.)
    let heading = |p: &Paragraph| p.props.style.as_deref() == Some("TOCHeading") || markers_only(p) && p.section.is_none();
    if at.path.depth() == 0
        && i > 0
        && s.doc.para_at(&at).is_some_and(markers_only)
        && s.doc.body.get(i - 1).and_then(|b| b.as_para()).is_some_and(heading)
    {
        if s.doc.para_at(&at).is_some_and(|p| p.is_empty()) {
            s.doc.remove_block(StoryRef::Body, &at.path)?;
        }
        let heading = Path::top(i - 1);
        let len = s.doc.para(StoryRef::Body, &heading).map_or(0, |p| p.len());
        at = Pos { story: StoryRef::Body, path: heading, off: len };
    }
    // The code as the range had it, nested fields too (their places counted from the trimmed code).
    let lead = r.instr.len() - r.instr.trim_start().len();
    let nested = r.nested.iter().filter(|n| n.at >= lead).map(|n| NestedField { at: n.at - lead, ..n.clone() }).collect();
    let field = InlineObject::Field {
        instr: r.instr.trim().to_string(),
        result: String::new(),
        locked: false,
        dirty: false,
        code: false,
        form: String::new(),
        nested,
    };
    let after = s.doc.insert_object(&at, field, &props)?;
    if s.doc.para_at(&after).is_some_and(|p| after.off < p.len()) {
        s.doc.split_paragraph(&after)?;
    }
    s.clamp_selection();
    Ok(())
}

/// The TOC field instruction in `p` (not a table of figures).
fn toc_field(p: &Paragraph) -> Option<String> {
    p.objects.iter().find_map(|o| match o {
        InlineObject::Field { instr, .. } => {
            let f = FieldCode::parse(instr);
            (f.name == "TOC" && !f.has('c')).then(|| instr.clone())
        }
        _ => None,
    })
}

/// Whether the TOC field in `p` is locked (`w:fldLock`): its entries stay as they are.
fn toc_locked(p: &Paragraph) -> bool {
    p.objects.iter().any(|o| matches!(o, InlineObject::Field { instr, locked: true, .. } if is_toc_instr(instr)))
}

/// The first cached number-separator tab formatting at each TOC level.
fn cached_toc_tabs(s: &Session) -> Vec<(String, Vec<TabStop>)> {
    let span = toc_range(s).map(|(start, end)| (start.saturating_add(1), end)).or_else(|| {
        s.doc.field_ranges(StoryRef::Body).into_iter().find_map(|r| {
            let f = FieldCode::parse(&r.instr);
            (r.depth == 0 && !r.locked && r.start.path.depth() == 0 && r.end.path.depth() == 0 && f.name == "TOC" && !f.has('c'))
                .then_some((r.start.path.last(), r.end.path.last()))
        })
    });
    let Some(blocks) = span.and_then(|(start, end)| s.doc.body.get(start..=end)) else { return Vec::new() };
    let mut tabs: Vec<(String, Vec<TabStop>)> = Vec::new();
    for p in blocks.iter().filter_map(|b| b.as_para()) {
        let Some(style) = &p.props.style else { continue };
        let Some(direct) = &p.props.tabs else { continue };
        if !direct.iter().any(|t| matches!(t.align, TabAlign::Left | TabAlign::Center | TabAlign::Decimal)) {
            continue;
        }
        if !style.starts_with("TOC") || style == "TOCHeading" || tabs.iter().any(|(id, _)| id == style) {
            continue;
        }
        tabs.push((style.clone(), direct.clone()));
    }
    tabs
}

fn insert_toc(s: &mut Session, v: &Value) -> CmdResult {
    let tabs = cached_toc_tabs(s);
    range_toc_to_field(s)?;
    let replace = v.as_object().is_some_and(|m| !m.is_empty());
    // Replacing a table puts the new one where the old one was, in place of an empty paragraph.
    let mut placeholder = false;
    let at = match toc_range(s) {
        // `{}` updates the table there; settings replace it where it is.
        Some(_) if !replace => {
            regenerate_toc(s, None, &tabs)?;
            return sel_result(s);
        }
        Some((start, end)) => {
            for i in (start..=end).rev() {
                s.doc.remove_block(StoryRef::Body, &Path::top(i))?;
            }
            s.doc.insert_block(StoryRef::Body, &Path::top(start), Block::Para(Paragraph::new()))?;
            placeholder = true;
            s.sel = Selection::caret(Pos { story: StoryRef::Body, path: Path::top(start), off: 0 });
            s.sel.focus.clone()
        }
        None => delete_selection(s)?,
    };
    let title = p::str(v, "title").unwrap_or("Contents").to_string();
    if at.story != StoryRef::Body || at.path.depth() > 0 {
        return Err(CmdError::Failed("a table of contents goes in the main text".into()));
    }
    // Title paragraph carrying the TOC field marker.
    let mut head = Paragraph::with_text(&title, CharProps::default()).styled("TOCHeading");
    head.insert_object(
        head.len(),
        InlineObject::Field {
            instr: toc_instr(v),
            result: String::new(),
            locked: false,
            dirty: false,
            code: false,
            form: String::new(),
            nested: Vec::new(),
        },
        &CharProps::default(),
    )?;
    let i = if s.doc.para_at(&at).is_some_and(|p| p.is_empty()) {
        if placeholder {
            s.doc.remove_block(StoryRef::Body, &at.path)?;
        }
        at.path.last()
    } else {
        s.doc.split_paragraph(&at)?.path.last()
    };
    s.doc.insert_block(StoryRef::Body, &Path::top(i), Block::Para(head))?;
    let leader = leader(p::str(v, "tabLeader").unwrap_or("dot"));
    regenerate_toc(s, Some(leader), &tabs)?;
    s.clamp_selection();
    sel_result(s)
}

/// A hidden `_Toc` bookmark around the heading at body block `i` (after its leading page breaks),
/// reusing one already there; its name.
pub(crate) fn toc_bookmark(s: &mut Session, i: usize, lead: usize) -> Result<String, CmdError> {
    toc_bookmark_from(s, &Path::top(i), lead, &mut None)
}

/// [`toc_bookmark`] for the heading paragraph at `path` (top-level or in a table cell, #2482),
/// numbering a new bookmark from `next` (found from the document's `_Toc` bookmarks when `None`,
/// then counted on), so a table of many headings looks once.
fn toc_bookmark_from(s: &mut Session, path: &Path, lead: usize, next: &mut Option<u64>) -> Result<String, CmdError> {
    const MARK: usize = wordcraft_doc::para::OBJ.len_utf8();
    let para = s.doc.para(StoryRef::Body, path).ok_or_else(|| CmdError::Failed("heading is gone".into()))?;
    if let Some(InlineObject::BookmarkStart { name }) = para.object_at(lead)
        && name.starts_with("_Toc")
    {
        return Ok(name.clone());
    }
    let n = *next.get_or_insert_with(|| {
        s.doc.bookmarks().iter().filter_map(|(n, _)| n.strip_prefix("_Toc").and_then(|d| d.parse::<u64>().ok())).max().unwrap_or(0).saturating_add(1)
    });
    *next = Some(n.saturating_add(1));
    let name = format!("_Toc{n:09}");
    let end = para.len();
    let props = CharProps::default();
    let pos = |off| Pos { story: StoryRef::Body, path: path.clone(), off };
    s.doc.insert_object(&pos(end), InlineObject::BookmarkEnd { name: name.clone() }, &props)?;
    s.doc.insert_object(&pos(lead), InlineObject::BookmarkStart { name: name.clone() }, &props)?;
    // The caret in that paragraph stays on the same text.
    let shift = |p: &mut Pos| {
        if p.story == StoryRef::Body && &p.path == path {
            if p.off > end {
                p.off = p.off.saturating_add(2 * MARK);
            } else if p.off >= lead && p.off > 0 {
                p.off = p.off.saturating_add(MARK);
            }
        }
    };
    shift(&mut s.sel.anchor);
    shift(&mut s.sel.focus);
    Ok(name)
}

/// Regenerate the TOC entries with page numbers, laying the document out again until they hold
/// (the entries themselves change pagination).
pub fn update_toc(s: &mut Session) -> Result<(), CmdError> {
    let tabs = cached_toc_tabs(s);
    regenerate_toc(s, None, &tabs)
}

/// Rebuild the entries the TOC field's switches ask for. The tab leader is `leader`, or the one
/// the entries had (Word keeps it in the entries' tab stops, not in the field).
fn regenerate_toc(s: &mut Session, leader: Option<TabLeader>, tabs: &[(String, Vec<TabStop>)]) -> Result<(), CmdError> {
    let mut leader = leader;
    for _ in 0..PAGE_ROUNDS {
        if !toc_pass(s, leader.take(), tabs)? {
            break;
        }
    }
    Ok(())
}

/// Whether `instr` is a table of contents (not a table of figures).
fn is_toc_instr(instr: &str) -> bool {
    let f = FieldCode::parse(instr);
    f.name == "TOC" && !f.has('c')
}

/// One pass over the TOC: its entries as the headings and the current layout give them (the
/// layout already has the entries there, so their own length counts), replacing only those that
/// come out different. Returns whether anything changed.
fn toc_pass(s: &mut Session, leader: Option<TabLeader>, tabs: &[(String, Vec<TabStop>)]) -> Result<bool, CmdError> {
    let before = s.doc.body.clone();
    range_toc_to_field(s)?;
    let Some((start, end)) = toc_range(s) else { return Ok(body_changed(&s.doc.body, &before)) };
    let Some(instr) = s.doc.body.get(start).and_then(|b| b.as_para()).and_then(toc_field) else { return Ok(body_changed(&s.doc.body, &before)) };
    if s.doc.body.get(start).and_then(|b| b.as_para()).is_some_and(toc_locked) {
        return Ok(false);
    }
    // A TOC field imported with an inline placeholder/cached result must have its result cleared so
    // that the engine's TOC shape matches `toc_span` and the DOCX writer spans the field across entries.
    let cached = |o: &InlineObject| matches!(o, InlineObject::Field { instr, result, .. } if !result.is_empty() && is_toc_instr(instr));
    if s.doc.body.get(start).and_then(|b| b.as_para()).is_some_and(|p| p.objects.iter().any(cached)) {
        let p = s.doc.para_mut(StoryRef::Body, &Path::top(start))?;
        for o in &mut p.objects {
            if let InlineObject::Field { instr, result, .. } = o
                && is_toc_instr(instr)
            {
                result.clear();
            }
        }
        p.touch();
    }
    let f = FieldCode::parse(&instr);
    let leader = leader.unwrap_or_else(|| {
        s.doc
            .body
            .get(start + 1)
            .filter(|_| end > start)
            .and_then(|b| b.as_para())
            .and_then(|p| p.props.tabs.as_ref()?.last().map(|t| t.leader))
            .unwrap_or(TabLeader::Dot)
    });
    // The text width of the TOC's own section, not the caret's: after Select All the caret sits
    // at the end of the document, possibly in a wider (landscape) section.
    let section = s.doc.section_index_of(start);
    let width = s.doc.sections().get(section).map_or_else(|| super::page::sect(s).text_width(), |(_, x)| x.text_width());
    let mut entries = toc_entries(s, &f);
    let links: Vec<Option<String>> = if f.has('h') {
        let mut v = Vec::new();
        let mut next = None;
        for e in &entries {
            // TC fields link to their paragraph's start.
            let lead = s.doc.para(StoryRef::Body, &e.path).map_or(0, |p| p.text.bytes().take_while(|b| *b == 0x0C).count());
            v.push(Some(format!("#{}", toc_bookmark_from(s, &e.path, lead, &mut next)?)));
        }
        // A new bookmark moves the text after it: TC fields are found again where they are now.
        entries = toc_entries(s, &f);
        v
    } else {
        vec![None; entries.len()]
    };
    let no_pages = f.has('n').then(|| f.arg('n').and_then(level_range).unwrap_or((1, 9)));
    let sep = f.arg('p').map(str::to_string).unwrap_or_else(|| "\t".into());
    // The pages come from the document as it is, the entries there: paths and layout agree.
    // (A conversion or new bookmarks above changed it since the layout was last read.)
    if body_changed(&s.doc.body, &before) {
        s.touch();
    }
    let l = s.layout();
    let sections = s.doc.sections();
    let mut paras = Vec::with_capacity(entries.len());
    for (k, e) in entries.iter().enumerate() {
        let (n, format) = page_label(&l, &sections, &Pos { story: StoryRef::Body, path: e.path.clone(), off: e.lead });
        let show_page = no_pages.is_none_or(|(a, b)| !(a..=b).contains(&(e.level + 1)));
        let text = if show_page { format!("{}{sep}{}", e.text, format.format(n)) } else { e.text.clone() };
        let props = CharProps { link: links.get(k).cloned().flatten(), ..Default::default() };
        let mut para = Paragraph::with_text(&text, props).styled(&format!("TOC{}", e.level + 1));
        para.mark = CharProps::default();
        // An entry whose text has a tab (a heading number's) keeps the number tabs the entries of
        // its level had (#2475).
        if e.text.contains('\t') {
            para.props.tabs = tabs.iter().find(|(style, _)| Some(style) == para.props.style.as_ref()).map(|(_, tabs)| tabs.clone());
        }
        if show_page && sep.contains('\t') {
            let right = TabStop { pos: width - 0.5, align: TabAlign::Right, leader };
            para.props.tabs = Some(entry_tabs(s, &para.props, &e.text, right));
        }
        paras.push(para);
    }
    replace_entries(s, start + 1, end - start, paras);
    let changed = body_changed(&s.doc.body, &before);
    if changed {
        s.touch();
    }
    Ok(changed)
}

/// Whether `p` holds nothing but content control markers (or nothing at all).
fn markers_only(p: &Paragraph) -> bool {
    p.text.chars().all(|c| c == OBJ) && p.objects.iter().all(|o| matches!(o, InlineObject::ControlStart { .. } | InlineObject::ControlEnd))
}

/// The content control markers of `old` (the blocks a regenerated list replaces) that the new
/// paragraphs carry on, with their formatting: (those that go before the first new paragraph's
/// text, those that go after the last one's). A control around the list (a TOC's building-block
/// control read from a file, #2427) keeps both its ends where they were: before the first
/// entry's text, after the last one's; a marker in the middle whose partner is outside the list
/// goes to the nearer end (a start to the front, an end to the back). Controls opened and closed
/// within the old entries (around generated text) go with them. Every start or end of a control
/// outside the list is carried, so its markers stay balanced.
fn carried_markers(old: &[std::sync::Arc<Block>]) -> (Vec<(InlineObject, CharProps)>, Vec<(InlineObject, CharProps)>) {
    #[derive(Clone, Copy, PartialEq)]
    enum Place {
        Lead,
        Mid,
        Trail,
    }
    let is_marker = |o: &InlineObject| matches!(o, InlineObject::ControlStart { .. } | InlineObject::ControlEnd);
    let last = old.len().saturating_sub(1);
    let mut seq: Vec<(InlineObject, CharProps, Place)> = Vec::new();
    for (bi, b) in old.iter().enumerate() {
        let Some(p) = b.as_para() else { continue };
        let offsets = p.object_offsets();
        // Where the paragraph's own content (anything but markers) starts and ends.
        let content = |off: usize, c: char| c != OBJ || p.object_at(off).is_none_or(|o| !is_marker(o));
        let first = p.text.char_indices().find(|&(i, c)| content(i, c)).map_or(p.len(), |(i, _)| i);
        let past = p.text.char_indices().rev().find(|&(i, c)| content(i, c)).map_or(0, |(i, c)| i + c.len_utf8());
        for (off, o) in offsets.iter().zip(&p.objects) {
            if !is_marker(o) {
                continue;
            }
            let lead = bi == 0 && *off < first;
            let trail = bi == last && *off >= past;
            let place = match (lead, trail) {
                // A paragraph of markers only, alone: starts in front, ends behind.
                (true, true) if matches!(o, InlineObject::ControlEnd) => Place::Trail,
                (true, _) => Place::Lead,
                (_, true) => Place::Trail,
                _ => Place::Mid,
            };
            seq.push((o.clone(), p.props_of_char(*off).clone(), place));
        }
    }
    // Pairs with an end among the generated text go with it.
    let mut keep = vec![true; seq.len()];
    let mut open: Vec<usize> = Vec::new();
    for (i, (o, _, place)) in seq.iter().enumerate() {
        match o {
            InlineObject::ControlStart { .. } => open.push(i),
            _ => {
                if let Some(j) = open.pop()
                    && (*place == Place::Mid || seq.get(j).is_some_and(|m| m.2 == Place::Mid))
                {
                    for k in [i, j] {
                        if let Some(slot) = keep.get_mut(k) {
                            *slot = false;
                        }
                    }
                }
            }
        }
    }
    let kept = |want: &dyn Fn(&InlineObject, Place) -> bool| -> Vec<(InlineObject, CharProps)> {
        seq.iter().zip(&keep).filter(|((o, _, place), k)| **k && want(o, *place)).map(|((o, props, _), _)| (o.clone(), props.clone())).collect()
    };
    let mut front = kept(&|_, place| place == Place::Lead);
    front.extend(kept(&|o, place| place == Place::Mid && matches!(o, InlineObject::ControlStart { .. })));
    let mut back = kept(&|o, place| place == Place::Mid && matches!(o, InlineObject::ControlEnd));
    back.extend(kept(&|_, place| place == Place::Trail));
    (front, back)
}

/// Make the `count` body blocks from `at` the paragraphs `new`, keeping each block that comes
/// out the same (its layout is reused, and Undo shares it). A section break that ended one of
/// the old blocks ends the last new one (or stays in an empty paragraph when there is none), and
/// so do the markers of a content control around them ([`carried_markers`]).
pub(crate) fn replace_entries(s: &mut Session, at: usize, count: usize, mut new: Vec<Paragraph>) {
    let body = &mut s.doc.body;
    let end = at.saturating_add(count).min(body.len());
    let at = at.min(end);
    let (front, back) = carried_markers(body.get(at..end).unwrap_or_default());
    if !(front.is_empty() && back.is_empty()) {
        if new.is_empty() {
            new.push(Paragraph::new());
        }
        if let Some(first) = new.first_mut() {
            for (k, (o, props)) in front.into_iter().enumerate() {
                let _ = first.insert_object(k * OBJ.len_utf8(), o, &props);
            }
        }
        if let Some(last) = new.last_mut() {
            for (o, props) in back {
                let _ = last.insert_object(last.len(), o, &props);
            }
        }
    }
    let mut section = None;
    for b in body.get(at..end).unwrap_or_default() {
        if let Some(p) = b.as_para()
            && p.section.is_some()
        {
            section = p.section.clone();
        }
    }
    if section.is_some() {
        if new.is_empty() {
            new.push(Paragraph::new());
        }
        if let Some(last) = new.last_mut() {
            last.section = section;
        }
    }
    let same = |b: &std::sync::Arc<Block>, p: &Paragraph| b.as_para() == Some(p);
    if end - at == new.len() {
        for (b, p) in body.iter_mut().skip(at).zip(new) {
            if !same(b, &p) {
                *b = std::sync::Arc::new(Block::Para(p));
            }
        }
        return;
    }
    // A different number: keep the entries that start and end the run alike.
    let old = end - at;
    let mut head = 0;
    while head < old.min(new.len()) && body.get(at + head).zip(new.get(head)).is_some_and(|(b, p)| same(b, p)) {
        head += 1;
    }
    let mut tail = 0;
    while tail < (old - head).min(new.len() - head) && body.get(end - 1 - tail).zip(new.get(new.len() - 1 - tail)).is_some_and(|(b, p)| same(b, p)) {
        tail += 1;
    }
    let keep = new.len() - tail;
    let fresh: Vec<std::sync::Arc<Block>> = new.into_iter().take(keep).skip(head).map(|p| std::sync::Arc::new(Block::Para(p))).collect();
    body.splice(at + head..end - tail, fresh);
}

/// The tab stops of a TOC entry: the right-aligned stop for its page number, and, when the
/// entry text itself has no tab, Clear stops for the left, centre and decimal stops its style
/// sets before that. Those stops are for a number set off by a tab (`1.1<tab>Title`); with the
/// number set off by a space, or no number, a short entry's page tab would stop at one of them
/// (without the leader), and so would the tab on the last line of an entry that wraps.
fn entry_tabs(s: &Session, props: &wordcraft_doc::ParaProps, text: &str, right: TabStop) -> Vec<TabStop> {
    let mut tabs = props.tabs.clone().unwrap_or_default();
    tabs.retain(|t| t.pos < right.pos - 0.5 && !matches!(t.align, TabAlign::Right | TabAlign::Bar));
    if !text.contains('\t') {
        let style = s.doc.styles.resolve_para(&wordcraft_doc::ParaProps { style: props.style.clone(), ..Default::default() });
        for t in style.tabs {
            if t.pos < right.pos - 0.5 && !matches!(t.align, TabAlign::Right | TabAlign::Bar) {
                tabs.push(TabStop { pos: t.pos, align: TabAlign::Clear, leader: TabLeader::None });
            }
        }
    }
    tabs.push(right);
    tabs
}

fn note(s: &mut Session, v: &Value, kind: NoteKind) -> CmdResult {
    let text = p::str(v, "text").unwrap_or("").to_string();
    // A custom mark: a few characters, as Word's dialog takes.
    let custom: String = p::str(v, "mark").unwrap_or("").trim().chars().filter(|c| !c.is_control()).take(10).collect();
    let (pk, style, refstyle) = match kind {
        NoteKind::Footnote => (PartKind::Footnote, "FootnoteText", "FootnoteReference"),
        NoteKind::Endnote => (PartKind::Endnote, "EndnoteText", "EndnoteReference"),
    };
    // With Track Changes on the note is a tracked insertion, reference and text, so Reject All
    // takes it out again and Accept All keeps it.
    let ins = s.doc.settings.track_changes.then(|| super::new_revision(s, wordcraft_doc::RevisionKind::Insert));
    let mut para = Paragraph::new().styled(style);
    let id = s.doc.try_add_part(pk, Vec::new())?;
    para.insert_object(
        0,
        InlineObject::NoteRef { kind, id, custom: custom.clone() },
        &CharProps { style: Some(refstyle.into()), ins, ..Default::default() },
    )?;
    let len = para.len();
    para.insert_text(len, &format!(" {text}"), &CharProps { ins, ..Default::default() })?;
    s.doc.set_story(StoryRef::Part(id), vec![para_block(para)])?;
    let at = delete_selection(s)?;
    s.doc.insert_object(&at, InlineObject::NoteRef { kind, id, custom }, &CharProps { style: Some(refstyle.into()), ins, ..Default::default() })?;
    // Like Word, the caret moves into the new note.
    s.sel = Selection::caret(s.doc.end_of(StoryRef::Part(id)));
    Ok(json!({"id": id}))
}

/// `references.noteOptions`: footnote or endnote placement, number format, start and restart,
/// for the whole document (each section then follows it) or the selection's sections.
pub(super) fn note_options(s: &mut Session, v: &Value) -> CmdResult {
    use wordcraft_doc::section::{MAX_NOTE_START, NotePos, NoteProps, NoteRestart, NumFormat};
    let fmt = |k: &str| -> Result<Option<NumFormat>, CmdError> {
        match p::str(v, k) {
            None => Ok(None),
            Some(f) if NumFormat::from_ooxml(f).ooxml() == f => Ok(Some(NumFormat::from_ooxml(f))),
            Some(f) => Err(CmdError::Params(format!("unknown number format {f:?}"))),
        }
    };
    // The earlier form: the document's number formats.
    let (old_foot, old_end) = (fmt("footnoteFormat")?, fmt("endnoteFormat")?);
    let endnote = match p::str(v, "kind") {
        None | Some("footnote") => false,
        Some("endnote") => true,
        Some(k) => return Err(CmdError::Params(format!("unknown `kind` {k:?}; use footnote or endnote"))),
    };
    let pos = match p::str(v, "pos") {
        None => None,
        Some(x) => match NotePos::from_ooxml(x) {
            Some(p @ (NotePos::PageBottom | NotePos::BeneathText)) if !endnote => Some(p),
            Some(p @ (NotePos::SectEnd | NotePos::DocEnd)) if endnote => Some(p),
            _ => {
                let ok = if endnote { "sectEnd or docEnd" } else { "pageBottom or beneathText" };
                return Err(CmdError::Params(format!("`pos` {x:?} doesn't apply here; use {ok}")));
            }
        },
    };
    let num_restart = match p::str(v, "numRestart") {
        None => None,
        Some(r) => match NoteRestart::from_ooxml(r) {
            Some(NoteRestart::EachPage) if endnote => return Err(CmdError::Params("endnotes can't start again on each page".into())),
            Some(r) => Some(r),
            None => return Err(CmdError::Params(format!("unknown `numRestart` {r:?}; use continuous, eachSect or eachPage"))),
        },
    };
    let change = NoteProps {
        pos,
        num_fmt: fmt("numFmt")?,
        num_start: p::f32(v, "numStart").map(|n| n.round().clamp(1.0, MAX_NOTE_START as f32) as u32),
        num_restart,
    };
    let whole = match p::str(v, "scope") {
        None | Some("document") => true,
        Some("section") => false,
        Some(x) => return Err(CmdError::Params(format!("unknown `scope` {x:?}; use document or section"))),
    };
    // Set `from`'s fields on `to`; `clear` instead unsets the fields `from` sets.
    let merge = |to: &mut NoteProps, from: &NoteProps, clear: bool| {
        if from.pos.is_some() {
            to.pos = if clear { None } else { from.pos };
        }
        if from.num_fmt.is_some() {
            to.num_fmt = if clear { None } else { from.num_fmt };
        }
        if from.num_start.is_some() {
            to.num_start = if clear { None } else { from.num_start };
        }
        if from.num_restart.is_some() {
            to.num_restart = if clear { None } else { from.num_restart };
        }
    };
    fn pick(sect: &mut wordcraft_doc::SectionProps, endnote: bool) -> &mut NoteProps {
        if endnote { &mut sect.endnote_pr } else { &mut sect.footnote_pr }
    }
    if change.is_empty() {
        // Nothing to change: the options in effect.
    } else if whole {
        let st = &mut s.doc.settings;
        if let Some(f) = change.num_fmt {
            *(if endnote { &mut st.endnote_format } else { &mut st.footnote_format }) = f;
        }
        merge(if endnote { &mut st.endnote_pr } else { &mut st.footnote_pr }, &NoteProps { num_fmt: None, ..change }, false);
        // Every section follows the document now.
        let ends: Vec<usize> = s.doc.sections().iter().map(|(e, _)| *e).collect();
        for e in ends {
            merge(pick(s.doc.section_mut(e), endnote), &change, true);
        }
    } else {
        super::page::with_sect(s, |sect| merge(pick(sect, endnote), &change, false))?;
    }
    if let Some(f) = old_foot {
        s.doc.settings.footnote_format = f;
    }
    if let Some(f) = old_end {
        s.doc.settings.endnote_format = f;
    }
    let sect = super::page::sect(s);
    Ok(json!({
        "footnote": s.doc.note_options(&sect, false),
        "endnote": s.doc.note_options(&sect, true),
        "footnoteFormat": s.doc.settings.footnote_format,
        "endnoteFormat": s.doc.settings.endnote_format,
    }))
}

fn note_refs(s: &Session, kind: Option<NoteKind>) -> Vec<Pos> {
    let mut v = Vec::new();
    for path in s.doc.para_paths(StoryRef::Body) {
        let Some(p) = s.doc.para(StoryRef::Body, &path) else { continue };
        for o in p.object_offsets() {
            if let Some(InlineObject::NoteRef { kind: k, .. }) = p.object_at(o)
                && (kind.is_none() || kind == Some(*k))
            {
                v.push(Pos { story: StoryRef::Body, path: path.clone(), off: o });
            }
        }
    }
    v
}

/// The `\*` format switch value for a caption's number format (OOXML names work too).
pub fn seq_format(name: &str) -> &'static str {
    match name {
        "alphabetic" | "lowerLetter" => "alphabetic",
        "ALPHABETIC" | "upperLetter" => "ALPHABETIC",
        "roman" | "lowerRoman" => "roman",
        "ROMAN" | "upperRoman" => "ROMAN",
        _ => "ARABIC",
    }
}

/// Separators Word offers between a chapter number and a caption number.
pub const CHAPTER_SEPARATORS: [&str; 5] = ["-", ".", ":", "\u{2014}", "\u{2013}"];

/// Insert a caption paragraph below (or above) the caret's paragraph, table or object: the label,
/// a chapter number (`STYLEREF n \s` and a separator) and a `SEQ label \* format \s n` number
/// (ECMA-376 §17.16.5.56, §17.16.5.59).
fn caption(s: &mut Session, v: &Value) -> CmdResult {
    let label: String = p::str(v, "label").unwrap_or("Figure").chars().filter(|c| !matches!(c, '"' | '\\') && !c.is_control()).take(40).collect();
    let label = if label.trim().is_empty() { "Figure".to_string() } else { label.trim().to_string() };
    // SEQ identifiers are one word: Word joins a multi-word label with underscores.
    let ident = label.split_whitespace().collect::<Vec<_>>().join("_");
    let text = p::str(v, "text").unwrap_or("").to_string();
    let above = p::str(v, "position") == Some("above");
    let format = seq_format(p::str(v, "format").unwrap_or("ARABIC"));
    let chapter = p::u64(v, "chapter").filter(|l| (1..=9).contains(l));
    let sep = p::str(v, "separator").and_then(|x| CHAPTER_SEPARATORS.iter().find(|s| **s == x)).copied().unwrap_or("-");
    let mut para = Paragraph::new().styled("Caption");
    let field = |instr: String| InlineObject::Field {
        instr,
        result: String::new(),
        locked: false,
        dirty: false,
        code: false,
        form: String::new(),
        nested: Vec::new(),
    };
    if !p::bool(v, "excludeLabel").unwrap_or(false) {
        para.insert_text(0, &format!("{label} "), &CharProps::default())?;
    }
    let mut seq = format!("SEQ {ident} \\* {format}");
    if let Some(l) = chapter {
        para.insert_object(para.len(), field(format!("STYLEREF {l} \\s")), &CharProps::default())?;
        para.insert_text(para.len(), sep, &CharProps::default())?;
        seq.push_str(&format!(" \\s {l}"));
    }
    para.insert_object(para.len(), field(seq), &CharProps::default())?;
    if !text.is_empty() {
        para.insert_text(para.len(), &format!(": {text}"), &CharProps::default())?;
    }
    // A selected picture, shape or chart: next to the paragraph holding it, in the same cell or
    // text box (#1230). Otherwise next to the caret's top-level block: a paragraph, or the whole
    // table it is in.
    let f = s.sel.focus.clone();
    let at = match super::objects::object_selection(s) {
        Some((pos, _)) => {
            let k = pos.path.last();
            pos.path.with_last(if above { k } else { k.saturating_add(1) })
        }
        None => {
            let top = f.path.0.first().copied().unwrap_or(0) as usize;
            Path::top(if above { top } else { top.saturating_add(1) })
        }
    };
    s.doc.insert_block(f.story, &at, Block::Para(para))?;
    update_seq(s)?;
    let off = s.doc.para(f.story, &at).map_or(0, |p| p.len());
    s.sel = Selection::caret(Pos { story: f.story, path: at, off });
    sel_result(s)
}

/// Renumber the body's SEQ fields in document order, honouring `\* format`, `\s level` (restart
/// after each heading of that level), `\r n` (reset to n), `\c` (repeat) and `\h` (hide), and
/// fill body STYLEREF fields ([`wordcraft_layout::styleref`]): chapter numbers (`STYLEREF n \s`,
/// the last level-n heading's number, or its count when the heading isn't numbered) and the
/// text or number of the nearest paragraph of a style.
///
/// SEQ fields in text boxes count in the body's sequence, at the place each box is anchored
/// (Word; `docs/word-behaviour-research.md` item 12). Headers, footers, notes and comments
/// don't: their SEQ fields are left alone.
pub fn update_seq(s: &mut Session) -> Result<(), CmdError> {
    update_seq_only(s, None)
}

/// One field to compute alone (Insert Field): the story and paragraph it is in and its index
/// among the paragraph's objects.
#[derive(Clone, Copy)]
pub(crate) struct Only<'a> {
    pub story: StoryRef,
    pub path: &'a Path,
    pub k: usize,
}

impl Only<'_> {
    /// Whether object `k` of the paragraph at `path` of `story` is the one.
    fn is(&self, story: StoryRef, path: &Path, k: usize) -> bool {
        self.story == story && *self.path == *path && self.k == k
    }
}

/// Text boxes nested in text boxes deeper than this are left out of the numbering.
const MAX_BOX_DEPTH: usize = 8;

/// [`update_seq`], writing the result of every field, or of `only` that one (the others keep
/// theirs: the numbers are still counted from the whole document, so the field takes the
/// number it has in it).
pub(crate) fn update_seq_only(s: &mut Session, only: Option<Only>) -> Result<(), CmdError> {
    let mut headings = [0u32; 9];
    let mut seq: std::collections::HashMap<String, (u32, u32)> = Default::default();
    // Built on the first STYLEREF (field results don't change what it finds).
    let mut styles: Option<wordcraft_layout::styleref::StyleIndex> = None;
    // The headings (they restart `\s` numbering), the paragraphs with fields and those with
    // text boxes (whose fields count where the box is anchored).
    let paths = paras_where(&s.doc, StoryRef::Body, &|p| heading_style_level(p).is_some() || has_unlocked_field(p) || has_text_box(p));
    let mut pending: Vec<(StoryRef, Path, Vec<(usize, String)>)> = Vec::new();
    for path in paths {
        let Some(p) = s.doc.para(StoryRef::Body, &path) else { continue };
        if let Some(l) = heading_style_level(p)
            && let Some(k) = (l as usize).checked_sub(1)
            && let Some(count) = headings.get_mut(k)
        {
            *count = count.saturating_add(1);
        }
        seq_para(&s.doc, StoryRef::Body, &path, &headings, &mut seq, &mut styles, 0, &mut pending);
    }
    for (story, path, mut updates) in pending {
        if let Some(o) = only {
            updates.retain(|(k, _)| o.is(story, &path, *k));
        }
        if !updates.is_empty() {
            set_results(s, story, &path, updates)?;
        }
    }
    Ok(())
}

/// Whether `instr` has the one-letter switch `\sw`, also run together with another (`\p\h`).
fn has_switch(instr: &str, sw: char) -> bool {
    instr.split('\\').skip(1).any(|s| {
        let mut chars = s.chars();
        chars.next().is_some_and(|c| c.eq_ignore_ascii_case(&sw)) && chars.next().is_none_or(|c| !c.is_alphanumeric())
    })
}

/// Whether `p` holds a shape with a text box (or a group with one).
fn has_text_box(p: &Paragraph) -> bool {
    p.objects.iter().any(|o| !o.text_boxes().is_empty())
}

/// The SEQ and STYLEREF results of the paragraph at `path` of `story`, counted on from `seq`
/// (see [`update_seq`]), and of the text boxes anchored in it, in order, each at its anchor;
/// pushed to `out`. STYLEREF is computed in the body only.
#[allow(clippy::too_many_arguments)]
fn seq_para(
    doc: &wordcraft_doc::Document,
    story: StoryRef,
    path: &Path,
    headings: &[u32; 9],
    seq: &mut std::collections::HashMap<String, (u32, u32)>,
    styles: &mut Option<wordcraft_layout::styleref::StyleIndex>,
    depth: usize,
    out: &mut Vec<(StoryRef, Path, Vec<(usize, String)>)>,
) {
    let Some(p) = doc.para(story, path) else { return };
    let mut updates: Vec<(usize, String)> = Vec::new();
    for (k, o) in p.objects.iter().enumerate() {
        let InlineObject::Field { instr, locked: false, .. } = o else {
            // A text box anchored here: its fields count now, before what follows the anchor.
            if depth < MAX_BOX_DEPTH {
                for id in o.text_boxes() {
                    if !doc.parts.get(&id).is_some_and(|part| part.kind == PartKind::TextBox) {
                        continue;
                    }
                    for bp in doc.para_paths(StoryRef::Part(id)) {
                        seq_para(doc, StoryRef::Part(id), &bp, headings, seq, styles, depth + 1, out);
                    }
                }
            }
            continue;
        };
        let f = FieldCode::parse(instr);
        match f.name.as_str() {
            "SEQ" => {
                let Some(id) = f.args.first() else { continue };
                let restart = f.arg('s').and_then(|l| l.parse::<usize>().ok()).and_then(|l| l.checked_sub(1)).and_then(|l| headings.get(l)).copied();
                let n = seq.entry(id.clone()).or_insert((0, 0));
                if let Some(epoch) = restart
                    && n.1 != epoch
                {
                    *n = (0, epoch);
                }
                if let Some(r) = f.arg('r').and_then(|r| r.parse::<u32>().ok()) {
                    n.0 = r;
                } else if !f.has('c') {
                    n.0 = n.0.saturating_add(1);
                }
                let fmt = wordcraft_layout::fields::format_switch(instr).unwrap_or_default();
                updates.push((k, if f.has('h') { String::new() } else { fmt.format(n.0) }));
            }
            "STYLEREF" if story == StoryRef::Body => {
                let ix = styles.get_or_insert_with(|| wordcraft_layout::styleref::StyleIndex::build(doc));
                if let Some(i) = ix.index_of(path) {
                    updates.push((k, ix.eval(instr, wordcraft_layout::styleref::At::Body(i))));
                }
            }
            _ => {}
        }
    }
    if !updates.is_empty() {
        out.push((story, path.clone(), updates));
    }
}

/// Compute the result of the one field at `at` (its U+FFFC), as Word's Insert Field does:
/// nothing else in the document changes (the table of contents and the other fields keep their
/// results until Update Fields; `docs/word-behaviour-research.md` item 11). A SEQ or STYLEREF
/// field takes the number it has in the document as it is; a PAGEREF its bookmark's page; a
/// table of contents or generated list is built. A document so left is still settled by the
/// next Update Fields, which computes every field again.
pub(crate) fn compute_field(s: &mut Session, at: &Pos) -> Result<(), CmdError> {
    let Some(p) = s.doc.para_at(at) else { return Ok(()) };
    let Some(InlineObject::Field { instr, locked: false, .. }) = p.object_at(at.off) else { return Ok(()) };
    let instr = instr.clone();
    let k = p.text.get(..at.off).map_or(0, |t| t.chars().filter(|c| *c == OBJ).count());
    let (story, path) = (at.story, at.path.clone());
    let t = instr.trim_start();
    if is_toc_instr(&instr) && story == StoryRef::Body {
        let tabs = cached_toc_tabs(s);
        toc_pass(s, None, &tabs)?;
        return Ok(());
    }
    if story == StoryRef::Body && (t.starts_with("INDEX") || t.starts_with("BIBLIOGRAPHY") || t.starts_with("TOA") || t.starts_with("TOC \\c")) {
        return super::citations::update_generated(s);
    }
    let only = Only { story, path: &path, k };
    match wordcraft_layout::fields::field_name(&instr).as_str() {
        "SEQ" | "STYLEREF" => update_seq_only(s, Some(only)),
        "PAGEREF" => update_page_fields_only(s, story, Some(only)).map(|_| ()),
        _ => update_text_fields_only(s, story, Some(only)).map(|_| ()),
    }
}

/// How many times an update lays the document out again for page numbers to settle: a table of
/// contents that grows moves the headings after it, and their new pages can change it again.
pub(crate) const PAGE_ROUNDS: usize = 4;

/// Update fields (F9) in the body: SEQ numbering and STYLEREF, dates, document properties,
/// cross-references, citations, generated lists and the table of contents.
///
/// The results depend on each other: page numbers on where the generated entries push the
/// text, a REF on the fields inside its bookmark, SEQ numbers on the fields the regenerated
/// entries no longer hold. So every kind is computed again, from the document and layout as
/// the round before left them, until a round changes nothing (bounded): a second update then
/// changes nothing either. A field whose result comes out the same is left as it was, its
/// paragraph shared with the undo step and its layout reused.
pub fn update_fields(s: &mut Session) -> Result<(), CmdError> {
    update_fields_in(s, false)
}

/// [`update_fields`] of the story being edited, as Word's F9 does (#1706): the body, or the
/// header, footer, note, comment or text box the selection is in (the body's fields stay as they
/// are then); with `all`, the body and every story (Update fields before printing). SEQ
/// numbering, STYLEREF, citations, generated lists and the TOC are the body's: they run only
/// when the body is updated.
pub fn update_fields_in(s: &mut Session, all: bool) -> Result<(), CmdError> {
    let caret_part = match s.sel.focus.story {
        StoryRef::Part(id) if s.doc.parts.contains_key(&id) => Some(StoryRef::Part(id)),
        _ => None,
    };
    let in_body = all || caret_part.is_none();
    let parts: Vec<StoryRef> = if all { s.doc.parts.keys().map(|k| StoryRef::Part(*k)).collect() } else { caret_part.into_iter().collect() };
    // The number tabs the TOC entries have now, kept by its regenerated entries (#2475).
    let tabs = if in_body { cached_toc_tabs(s) } else { Vec::new() };
    for _ in 0..PAGE_ROUNDS {
        let before = s.doc.body.clone();
        let mut changed = false;
        if in_body {
            update_seq(s)?;
            changed |= update_nested_codes(s, StoryRef::Body)?;
            changed |= update_text_fields(s, StoryRef::Body)?;
            super::citations::refresh_citations(s)?;
            super::citations::generated_pass(s)?;
            toc_pass(s, None, &tabs)?;
            update_page_fields(s, StoryRef::Body)?;
        }
        for story in &parts {
            changed |= update_nested_codes(s, *story)?;
            changed |= update_text_fields(s, *story)?;
            changed |= update_page_fields(s, *story)?;
        }
        if !changed && !body_changed(&s.doc.body, &before) {
            break;
        }
    }
    Ok(())
}

/// Whether `body` holds other blocks than `before` (an unchanged block is the same allocation).
pub(crate) fn body_changed(body: &wordcraft_doc::Blocks, before: &wordcraft_doc::Blocks) -> bool {
    body.len() != before.len() || body.iter().zip(before).any(|(a, b)| !std::sync::Arc::ptr_eq(a, b))
}

/// Put back the blocks of `before` that `body` has again, rebuilt but equal: one for one when
/// there are as many, else from the start and from the end. Their layouts are reused.
pub(crate) fn keep_same_blocks(body: &mut wordcraft_doc::Blocks, before: &wordcraft_doc::Blocks) {
    let same = |a: &std::sync::Arc<Block>, b: &std::sync::Arc<Block>| std::sync::Arc::ptr_eq(a, b) || **a == **b;
    if body.len() == before.len() {
        for (a, b) in body.iter_mut().zip(before) {
            if !std::sync::Arc::ptr_eq(a, b) && **a == **b {
                *a = b.clone();
            }
        }
        return;
    }
    let n = body.len().min(before.len());
    let mut head = 0;
    while head < n && body.get(head).zip(before.get(head)).is_some_and(|(a, b)| same(a, b)) {
        if let (Some(a), Some(b)) = (body.get_mut(head), before.get(head)) {
            *a = b.clone();
        }
        head += 1;
    }
    let (bl, ol) = (body.len(), before.len());
    let mut tail = 0;
    while tail < n - head && body.get(bl - 1 - tail).zip(before.get(ol - 1 - tail)).is_some_and(|(a, b)| same(a, b)) {
        if let (Some(a), Some(b)) = (body.get_mut(bl - 1 - tail), before.get(ol - 1 - tail)) {
            *a = b.clone();
        }
        tail += 1;
    }
}

/// Give the fields at object indices `updates` of the paragraph at `path` their new results,
/// clearing `w:dirty` as Word's F9 does. Only a paragraph whose fields change is touched, so an
/// update that computes the same results leaves the document, and its layout, as they were.
pub(crate) fn set_results(s: &mut Session, story: StoryRef, path: &Path, updates: Vec<(usize, String)>) -> Result<bool, CmdError> {
    let differs = s.doc.para(story, path).is_some_and(|p| {
        updates.iter().any(|(k, r)| matches!(p.objects.get(*k), Some(InlineObject::Field { result, dirty, .. }) if result != r || *dirty))
    });
    if !differs {
        return Ok(false);
    }
    let para = s.doc.para_mut(story, path)?;
    for (k, r) in updates {
        if let Some(InlineObject::Field { result, dirty, .. }) = para.objects.get_mut(k) {
            *result = r;
            *dirty = false;
        }
    }
    para.touch();
    // The layout read next (page numbers) is of the document with the new results.
    s.touch();
    Ok(true)
}

/// The body's bookmarks by name: where each starts (the first, if a name is there twice).
fn bookmark_starts(doc: &wordcraft_doc::Document) -> std::collections::HashMap<String, Pos> {
    let mut out = std::collections::HashMap::new();
    visit_paras(doc, StoryRef::Body, &mut |path, p| {
        if !p.objects.iter().any(|o| matches!(o, InlineObject::BookmarkStart { .. })) {
            return;
        }
        for off in p.object_offsets() {
            if let Some(InlineObject::BookmarkStart { name }) = p.object_at(off) {
                out.entry(name.clone()).or_insert_with(|| Pos::new(StoryRef::Body, Path(path.to_vec()), off));
            }
        }
    });
    out
}

/// The bookmark a REF or PAGEREF field names: its first argument, without the switches that
/// may be run on to it (`PAGEREF _Toc1\h`).
fn ref_target(f: &FieldCode) -> Option<&str> {
    f.args.first().map(|a| a.split('\\').next().unwrap_or("")).filter(|a| !a.is_empty())
}

/// The fields of `story` whose results don't depend on pages: dates, document properties,
/// word counts, table formulas and REF (a bookmark's text, or above/below with `\p`).
fn update_text_fields(s: &mut Session, story: StoryRef) -> Result<bool, CmdError> {
    update_text_fields_only(s, story, None)
}

/// [`update_text_fields`], or `only` that one field of `story`.
fn update_text_fields_only(s: &mut Session, story: StoryRef, only: Option<Only>) -> Result<bool, CmdError> {
    // The bookmarks and the word count, found once and only when a field asks for them.
    let mut marks: Option<std::collections::HashMap<String, Pos>> = None;
    let mut words: Option<usize> = None;
    let mut changed = false;
    let paths = match only {
        Some(o) => vec![o.path.clone()],
        None => paras_where(&s.doc, story, &has_unlocked_field),
    };
    for path in paths {
        let Some(p) = s.doc.para(story, &path) else { continue };
        let mut updates: Vec<(usize, String)> = Vec::new();
        for (k, (off, o)) in p.object_offsets().into_iter().zip(&p.objects).enumerate() {
            if only.is_some_and(|o| !o.is(story, &path, k)) {
                continue;
            }
            let InlineObject::Field { instr, locked: false, .. } = o else { continue };
            // A table formula reads the cells around its own cell again.
            if instr.trim_start().starts_with('=') {
                if let Some((tp, r, c)) = path.cell()
                    && let Some(t) = s.doc.table(story, &tp)
                {
                    let default_lang = super::default_lang(s);
                    let text = match super::table_formula::evaluate_field_with(t, Some(&s.doc), default_lang.as_deref(), r, c, instr) {
                        Ok(text) => text,
                        Err(e) => format!("!{e}"),
                    };
                    updates.push((k, text));
                }
                continue;
            }
            if let Some(text) = text_result(&s.doc, &s.clock, &mut marks, &mut words, instr, &Pos::new(story, path.clone(), off)) {
                updates.push((k, text));
            }
        }
        if !updates.is_empty() {
            changed |= set_results(s, story, &path, updates)?;
        }
    }
    Ok(changed)
}

/// What a field that doesn't depend on pages shows now: dates, document properties, word
/// counts and REF (a bookmark's text, above/below with `\p`, or an error). `here` is where the
/// field is (for a field nested in another's code, where that one is). `None` for the kinds
/// computed elsewhere or not at all, and for a property Word doesn't know: they keep their
/// results. `marks` and `words` are found the first time a field needs them. Dates are in
/// `clock`'s local time (#1707).
fn text_result(
    doc: &wordcraft_doc::Document,
    clock: &crate::clock::Clock,
    marks: &mut Option<std::collections::HashMap<String, Pos>>,
    words: &mut Option<usize>,
    instr: &str,
    here: &Pos,
) -> Option<String> {
    let name = wordcraft_layout::fields::field_name(instr);
    match name.as_str() {
        "DATE" | "TIME" | "CREATEDATE" | "SAVEDATE" | "PRINTDATE" => {
            let pic = date_picture(instr).unwrap_or_else(|| if name == "TIME" { "h:mm am/pm".into() } else { "M/d/yyyy".into() });
            // When the document was created and last saved, as its properties record
            // (ECMA-376 §17.16.5.10, §17.16.5.55), in local time with the offset in force then
            // (#1707); now for a document never saved.
            let recorded = match name.as_str() {
                "CREATEDATE" => Some(doc.core.created.as_str()),
                "SAVEDATE" => Some(doc.core.modified.as_str()),
                // When it was last printed; never printed, Word's all-zero date, not today.
                "PRINTDATE" => return Some(print_date_text(clock, &doc.core.last_printed, &pic)),
                _ => None,
            };
            Some(recorded.and_then(|iso| clock.format_recorded(&pic, iso)).unwrap_or_else(|| clock.format_now(&pic)))
        }
        // A property Word doesn't know keeps the result it has.
        "DOCPROPERTY" => FieldCode::parse(instr).args.first().and_then(|n| doc_property(doc, n)),
        "REF" => {
            let f = FieldCode::parse(instr);
            let marks = marks.get_or_insert_with(|| bookmark_starts(doc));
            let Some((target, at)) = ref_target(&f).and_then(|n| Some((n, marks.get(n)?))) else {
                return Some(MISSING_BOOKMARK.to_string());
            };
            if has_switch(instr, 'p') {
                // Where the bookmark is from here: only within the same story.
                (at.story == here.story).then(|| if at < here { "above" } else { "below" }.to_string())
            } else if bookmark_holds(doc, target, at, here) {
                // Its own result would be part of what it shows, longer at each update.
                Some(SELF_REFERENCE.to_string())
            } else {
                bookmark_text_at(doc, target, at)
            }
        }
        "TITLE" => Some(doc.core.title.clone()),
        "AUTHOR" => Some(doc.core.creator.clone()),
        "NUMWORDS" => Some(words.get_or_insert_with(|| doc.word_count()).to_string()),
        _ => None,
    }
}

/// The longest code updating nested fields makes (their results take their places in it).
const MAX_CODE: usize = 1 << 20;

/// Bring the fields nested in the codes of `story`'s fields up to date (#1671), and the codes
/// with them: each nested field's new result stands where it is in the code, as Word reads the
/// code when it updates the field. The kinds [`text_result`] computes are updated, deepest first;
/// a locked nested field, and other kinds (SEQ, PAGEREF…), keep their results.
fn update_nested_codes(s: &mut Session, story: StoryRef) -> Result<bool, CmdError> {
    let has_nested = |p: &Paragraph| {
        p.objects.iter().any(|o| {
            matches!(o, InlineObject::Field { locked: false, nested, .. } | InlineObject::FieldStart { locked: false, nested, .. } if !nested.is_empty())
        })
    };
    let paths = paras_where(&s.doc, story, &has_nested);
    let (mut marks, mut words) = (None, None);
    let mut changed = false;
    for path in paths {
        let Some(p) = s.doc.para(story, &path) else { continue };
        let mut updates: Vec<(usize, String, Vec<NestedField>)> = Vec::new();
        for (k, (off, o)) in p.object_offsets().into_iter().zip(&p.objects).enumerate() {
            let (InlineObject::Field { instr, locked: false, nested, .. } | InlineObject::FieldStart { instr, locked: false, nested, .. }) = o else {
                continue;
            };
            if nested.is_empty() {
                continue;
            }
            let here = Pos::new(story, path.clone(), off);
            let (code, inner) = refreshed_code(&s.doc, &s.clock, &mut marks, &mut words, instr, nested, &here, 1);
            if code != *instr || inner != *nested {
                updates.push((k, code, inner));
            }
        }
        if updates.is_empty() {
            continue;
        }
        let para = s.doc.para_mut(story, &path)?;
        for (k, code, inner) in updates {
            if let Some(InlineObject::Field { instr, nested, .. } | InlineObject::FieldStart { instr, nested, .. }) = para.objects.get_mut(k) {
                *instr = code;
                *nested = inner;
            }
        }
        para.touch();
        s.touch();
        changed = true;
    }
    Ok(changed)
}

/// `instr` with the fields in `nested` brought up to date (see [`update_nested_codes`]).
fn refreshed_code(
    doc: &wordcraft_doc::Document,
    clock: &crate::clock::Clock,
    marks: &mut Option<std::collections::HashMap<String, Pos>>,
    words: &mut Option<usize>,
    instr: &str,
    nested: &[NestedField],
    here: &Pos,
    depth: usize,
) -> (String, Vec<NestedField>) {
    let mut code = String::with_capacity(instr.len());
    let mut out = Vec::new();
    for piece in code_pieces(instr, nested) {
        match piece {
            CodePiece::Text(t) => code.push_str(t),
            CodePiece::Field(f) => {
                let (inner_code, inner_nested) = if depth < MAX_NESTED_DEPTH && !f.nested.is_empty() {
                    refreshed_code(doc, clock, marks, words, &f.instr, &f.nested, here, depth + 1)
                } else {
                    (f.instr.clone(), f.nested.clone())
                };
                let computed = if f.locked { None } else { text_result(doc, clock, marks, words, &inner_code, here) };
                let fits = |r: &str| code.len().saturating_add(r.len()) <= MAX_CODE;
                let (result, dirty) = match computed {
                    Some(r) if fits(&r) => (r, false),
                    _ => (f.result.clone(), f.dirty),
                };
                out.push(NestedField { at: code.len(), instr: inner_code, result: result.clone(), locked: f.locked, dirty, nested: inner_nested });
                code.push_str(&result);
            }
        }
    }
    (code, out)
}

/// The number and number format of the page the text at `pos` is on (page 1 when it isn't
/// laid out).
pub(crate) fn page_label(l: &wordcraft_layout::DocLayout, sections: &[(usize, &wordcraft_doc::SectionProps)], pos: &Pos) -> (u32, NumFormat) {
    let page = l.caret(pos).and_then(|c| l.pages.get(c.page));
    let format = page.and_then(|p| sections.get(p.section)).map(|(_, x)| x.page_num_format).unwrap_or_default();
    (page.map_or(1, |p| p.number), format)
}

/// PAGEREF fields of `story`, from the current layout: the page their bookmark starts on, in
/// its section's page number format unless a `\*` switch says otherwise; with `\p`, "above" or
/// "below" on the bookmark's own page and "on page N" elsewhere (ECMA-376 §17.16.5.45).
fn update_page_fields(s: &mut Session, story: StoryRef) -> Result<bool, CmdError> {
    update_page_fields_only(s, story, None)
}

/// [`update_page_fields`], or `only` that one field of `story`.
fn update_page_fields_only(s: &mut Session, story: StoryRef, only: Option<Only>) -> Result<bool, CmdError> {
    let is_pageref =
        |o: &InlineObject| matches!(o, InlineObject::Field { instr, locked: false, .. } if wordcraft_layout::fields::field_name(instr) == "PAGEREF");
    let paths = match only {
        Some(o) => vec![o.path.clone()],
        None => paras_where(&s.doc, story, &|p| p.objects.iter().any(is_pageref)),
    };
    if paths.is_empty() {
        return Ok(false);
    }
    let marks = bookmark_starts(&s.doc);
    let l = s.layout();
    let sections = s.doc.sections();
    let mut all: Vec<(Path, Vec<(usize, String)>)> = Vec::new();
    for path in paths {
        let Some(p) = s.doc.para(story, &path) else { continue };
        let mut updates = Vec::new();
        for (k, (off, o)) in p.object_offsets().into_iter().zip(&p.objects).enumerate() {
            let InlineObject::Field { instr, .. } = o else { continue };
            if !is_pageref(o) || only.is_some_and(|x| !x.is(story, &path, k)) {
                continue;
            }
            let f = FieldCode::parse(instr);
            let Some(at) = ref_target(&f).and_then(|n| marks.get(n)) else {
                updates.push((k, MISSING_BOOKMARK.to_string()));
                continue;
            };
            let (n, format) = page_label(&l, &sections, at);
            let number = wordcraft_layout::fields::format_switch(instr).unwrap_or(format).format(n);
            if !has_switch(instr, 'p') {
                updates.push((k, number));
            } else if story == StoryRef::Body {
                let here = Pos::new(story, path.clone(), off);
                let text = if l.caret(&here).map(|c| c.page) != l.caret(at).map(|c| c.page) {
                    format!("on page {number}")
                } else if *at < here {
                    "above".to_string()
                } else {
                    "below".to_string()
                };
                updates.push((k, text));
            }
        }
        all.push((path, updates));
    }
    let mut changed = false;
    for (path, updates) in all {
        changed |= set_results(s, story, &path, updates)?;
    }
    Ok(changed)
}

/// Refresh every unlocked `DATE` and `TIME` field to now, as Word does whenever it shows,
/// prints or exports a document: a letter written from a template carries today's date, not the
/// date the template was last saved (#1672).
///
/// Only those two. `CREATEDATE`, `SAVEDATE` and `PRINTDATE` record when something happened and
/// are not "now"; `REF`, `PAGEREF`, `TOC` and citations are refreshed on an explicit update
/// (`fields.updateAll`) and not on open, again as Word does. A field carrying `w:fldLock` keeps
/// its stored result, which is what the lock is for. Now is `clock`'s, in its local time (#1707).
pub fn refresh_date_fields(doc: &mut wordcraft_doc::Document, clock: &crate::clock::Clock) {
    refresh_dates(doc, &|name, pic| matches!(name, "DATE" | "TIME").then(|| clock.format_now(pic)));
}

/// Refresh every unlocked `PRINTDATE` field, in every story, to the print the document records
/// (the all-zero date when it was never printed): what printing does after it records the date
/// (`Session::stamp_print`), so the pages printed carry it, as Word's do (ECMA-376 §17.16.5.49).
pub fn refresh_print_date_fields(doc: &mut wordcraft_doc::Document, clock: &crate::clock::Clock) {
    let printed = doc.core.last_printed.clone();
    refresh_dates(doc, &|name, pic| (name == "PRINTDATE").then(|| print_date_text(clock, &printed, pic)));
}

/// What a PRINTDATE field shows: the recorded print `printed` (W3CDTF) in local time with the
/// picture `pic`, or Word's all-zero date when the document was never printed.
fn print_date_text(clock: &crate::clock::Clock, printed: &str, pic: &str) -> String {
    if printed.is_empty() {
        return crate::clock::format_never(pic);
    }
    clock.format_recorded(pic, printed).unwrap_or_else(|| crate::clock::format_never(pic))
}

/// Give every unlocked date field of every story the result `result` makes of its name and
/// date picture (`None` leaves the field alone); see [`refresh_date_fields`].
fn refresh_dates(doc: &mut wordcraft_doc::Document, result: &dyn Fn(&str, &str) -> Option<String>) {
    let stories: Vec<StoryRef> = std::iter::once(StoryRef::Body).chain(doc.parts.keys().copied().map(StoryRef::Part)).collect();
    for story in stories {
        for path in doc.para_paths(story) {
            let Some(p) = doc.para(story, &path) else { continue };
            let mut updates: Vec<(usize, String)> = Vec::new();
            for (k, o) in p.objects.iter().enumerate() {
                let InlineObject::Field { instr, locked: false, .. } = o else { continue };
                let name = wordcraft_layout::fields::field_name(instr);
                // Word's default picture when the code carries no `\@` switch.
                let pic = date_picture(instr).unwrap_or_else(|| if name == "TIME" { "h:mm am/pm".into() } else { "M/d/yyyy".into() });
                if let Some(r) = result(&name, &pic) {
                    updates.push((k, r));
                }
            }
            if updates.is_empty() {
                continue;
            }
            let Ok(para) = doc.para_mut(story, &path) else { continue };
            for (k, r) in updates {
                if let Some(InlineObject::Field { result, .. }) = para.objects.get_mut(k) {
                    *result = r;
                }
            }
            para.touch();
        }
    }
}

/// The value of document property `name` for a DOCPROPERTY field: a custom property, else one
/// of the built-in ones Word names (names compare case-insensitively).
fn doc_property(doc: &wordcraft_doc::Document, name: &str) -> Option<String> {
    if let Some(v) = doc.custom_prop(name) {
        return Some(v.to_string());
    }
    let c = &doc.core;
    Some(match name.to_ascii_lowercase().as_str() {
        "title" => c.title.clone(),
        "subject" => c.subject.clone(),
        "author" => c.creator.clone(),
        "keywords" => c.keywords.clone(),
        "comments" => c.description.clone(),
        "category" => c.category.clone(),
        "lastsavedby" => c.last_modified_by.clone(),
        "revisionnumber" => c.revision.to_string(),
        _ => return None,
    })
}

/// A field's `\@` date picture: the quoted text after it (switches may follow), or one word.
pub fn date_picture(instr: &str) -> Option<String> {
    let rest = instr.split_once("\\@")?.1.trim_start();
    let pic = match rest.strip_prefix('"') {
        Some(q) => q.split('"').next().unwrap_or(""),
        None => rest.split_whitespace().next().unwrap_or(""),
    };
    (!pic.is_empty()).then(|| pic.to_string())
}

/// What a REF or PAGEREF field shows when the document has no bookmark of the name it gives.
pub const MISSING_BOOKMARK: &str = "Error! Bookmark missing.";

/// What a REF field shows when it is inside the text of the bookmark it names.
pub const SELF_REFERENCE: &str = "Error! The field is inside the bookmark it shows.";

/// The longest text a REF field takes from its bookmark: bookmarks that show each other through
/// REF fields grow at every update, and stop here.
const MAX_REF_TEXT: usize = 32 * 1024;

/// The end of the text [`bookmark_text_at`] takes for the bookmark `name` starting at `start`.
fn bookmark_text_end(para: &Paragraph, name: &str, start: &Pos) -> usize {
    para.object_offsets()
        .into_iter()
        .find(|o| *o > start.off && matches!(para.object_at(*o), Some(InlineObject::BookmarkEnd { name: n }) if n == name))
        .unwrap_or(para.len())
}

/// Whether `at` is inside the text of the bookmark `name` starting at `start` (as REF shows it).
fn bookmark_holds(doc: &wordcraft_doc::Document, name: &str, start: &Pos, at: &Pos) -> bool {
    at.story == start.story
        && at.path == start.path
        && at.off >= start.off
        && doc.para(start.story, &start.path).is_some_and(|p| at.off < bookmark_text_end(p, name, start))
}

/// The text a bookmark marks (field results included), up to its end in the same paragraph or
/// the paragraph's end. `None` when there is no such bookmark.
pub fn bookmark_text(doc: &wordcraft_doc::Document, name: &str) -> Option<String> {
    let (_, start) = doc.bookmarks().into_iter().find(|(n, _)| n == name)?;
    bookmark_text_at(doc, name, &start)
}

/// [`bookmark_text`] for the bookmark `name` that starts at `start`, at most [`MAX_REF_TEXT`]
/// bytes of it.
fn bookmark_text_at(doc: &wordcraft_doc::Document, name: &str, start: &Pos) -> Option<String> {
    let para = doc.para(start.story, &start.path)?;
    let end = bookmark_text_end(para, name, start);
    // The text from the bookmark's start to its end, objects as their text (field results), as
    // the paragraph's plain text has it: read in place rather than copied out.
    let mut k = para.text.get(..start.off)?.chars().filter(|c| *c == OBJ).count();
    let mut text = String::new();
    for c in para.text.get(start.off..end)?.chars() {
        if c == OBJ {
            if let Some(o) = para.objects.get(k) {
                text.push_str(o.plain_text());
            }
            k += 1;
        } else {
            text.push(c);
        }
        if text.len() > MAX_REF_TEXT.saturating_mul(2) {
            break;
        }
    }
    let mut text = text.trim().to_string();
    if text.len() > MAX_REF_TEXT {
        let mut cut = MAX_REF_TEXT;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(s: &Session) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for path in s.doc.para_paths(StoryRef::Body) {
            for o in &s.doc.para(StoryRef::Body, &path).unwrap().objects {
                if let InlineObject::Field { instr, result, .. } = o {
                    out.push((instr.clone(), result.clone()));
                }
            }
        }
        out
    }

    /// #775: body STYLEREF fields show the nearest paragraph of the style before them.
    #[test]
    fn body_styleref_finds_the_heading_before_it() {
        let mut s = Session::new(wordcraft_doc::Document::from_text("Intro\nChapter One\ntext here\nChapter Two\nend"));
        for t in ["Chapter One", "Chapter Two"] {
            s.run("select.text", &json!({"text": t})).unwrap();
            s.run("styles.apply", &json!({"style": "Heading 1"})).unwrap();
        }
        s.run("select.text", &json!({"text": "text here"})).unwrap();
        s.run("caret.end", &json!({})).unwrap();
        s.run("insert.field", &json!({"instr": r#"STYLEREF "Heading 1" \* MERGEFORMAT"#})).unwrap();
        s.run("caret.docEnd", &json!({})).unwrap();
        s.run("insert.field", &json!({"instr": "STYLEREF 1 \\p"})).unwrap();
        s.run("caret.docStart", &json!({})).unwrap();
        s.run("insert.field", &json!({"instr": "STYLEREF \"Heading 1\" \\p"})).unwrap();
        s.run("insert.field", &json!({"instr": "STYLEREF \"No Such Style\""})).unwrap();
        let results: Vec<String> = fields(&s).into_iter().map(|f| f.1).collect();
        assert_eq!(results, ["below", wordcraft_layout::styleref::NO_TEXT, "Chapter One", "above"]);
    }

    /// #775: a template's running header `STYLEREF "heading 1"` (a complex field, opened from
    /// DOCX) shows each page's chapter, and keeps its code through a save.
    #[test]
    fn docx_styleref_header_shows_each_page_s_chapter() {
        let ns = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;
        let heading = |t: &str, brk: bool| {
            let b = if brk { "<w:pageBreakBefore/>" } else { "" };
            format!(
                r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/>{b}</w:pPr><w:r><w:t>{t}</w:t></w:r></w:p><w:p><w:r><w:t>Body of {t}.</w:t></w:r></w:p>"#
            )
        };
        let body = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><w:document {ns}><w:body>{}{}{}<w:sectPr><w:headerReference w:type="default" r:id="rIdH"/></w:sectPr></w:body></w:document>"#,
            heading("Rivers", false),
            heading("Mountains", true),
            heading("Deserts", true)
        );
        let header = format!(
            r#"<w:hdr {ns}><w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> STYLEREF  "heading 1" \* MERGEFORMAT </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>Cached</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p></w:hdr>"#
        );
        let styles = format!(
            r#"<w:styles {ns}><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/></w:style></w:styles>"#
        );
        let root = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;
        let rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdH" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/header" Target="header1.xml"/><Relationship Id="rIdS" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/></Relationships>"#;
        let mut zw = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, text) in [
            ("_rels/.rels", root),
            ("word/document.xml", body.as_str()),
            ("word/_rels/document.xml.rels", rels),
            ("word/header1.xml", header.as_str()),
            ("word/styles.xml", styles.as_str()),
        ] {
            zw.start_file(name, zip::write::SimpleFileOptions::default()).unwrap();
            std::io::Write::write_all(&mut zw, text.as_bytes()).unwrap();
        }
        let bytes = zw.finish().unwrap().into_inner();
        let headers = |s: &mut Session| -> Vec<String> {
            s.layout()
                .pages
                .iter()
                .map(|p| {
                    p.header
                        .iter()
                        .filter_map(|it| {
                            if let wordcraft_layout::Placed::Lines { para, .. } = it {
                                Some(para.shown.iter().map(|x| x.1.clone()).collect::<String>())
                            } else {
                                None
                            }
                        })
                        .collect::<String>()
                })
                .collect()
        };
        let mut s = Session::new(crate::io::open_bytes("chapters.docx", &bytes).unwrap());
        assert_eq!(headers(&mut s), ["Rivers", "Mountains", "Deserts"]);
        let saved = crate::io::save_bytes("chapters.docx", &s.doc).unwrap();
        let mut again = Session::new(crate::io::open_bytes("chapters.docx", &saved).unwrap());
        assert_eq!(headers(&mut again), ["Rivers", "Mountains", "Deserts"]);
        let hid = again.doc.last_section.headers.default.unwrap();
        let instr = again.doc.para_paths(StoryRef::Part(hid)).iter().find_map(|p| {
            again.doc.para(StoryRef::Part(hid), p).unwrap().objects.iter().find_map(|o| match o {
                InlineObject::Field { instr, .. } => Some(instr.clone()),
                _ => None,
            })
        });
        assert!(instr.is_some_and(|i| i.contains(r#"STYLEREF  "heading 1""#)));
    }

    /// #402: Custom Table of Contents settings become TOC switches, and updating honours them:
    /// levels (`\o`), a style with a level (`\t`), no hyperlinks, page numbers after a space
    /// (`\p`), or none at all (`\n`).
    #[test]
    fn toc_switches_shape_the_entries() {
        let mut s = Session::new(wordcraft_doc::Document::from_text("Intro\nPart\nDeep\nNote"));
        for (text, style) in [("Intro", "Heading 1"), ("Part", "Heading 2"), ("Deep", "Heading 3"), ("Note", "Quote")] {
            s.run("select.text", &json!({"text": text})).unwrap();
            s.run("styles.apply", &json!({"style": style})).unwrap();
        }
        s.run("caret.docStart", &json!({})).unwrap();
        s.run("references.toc", &json!({"levels": 2, "hyperlinks": false, "rightAlign": false, "styleLevels": {"Quote": 2}})).unwrap();
        assert_eq!(fields(&s)[0].0, "TOC \\o \"1-2\" \\z \\u \\p \" \" \\t \"Quote,2\"");
        let entries: Vec<(String, String)> = s
            .doc
            .body
            .iter()
            .filter_map(|b| b.as_para())
            .filter(|p| p.props.style.as_deref().is_some_and(|st| st.starts_with("TOC") && st != "TOCHeading"))
            .map(|p| (p.props.style.clone().unwrap(), p.plain_text()))
            .collect();
        assert_eq!(entries, [("TOC1", "Intro 1"), ("TOC2", "Part 1"), ("TOC2", "Note 1")].map(|(a, b)| (a.to_string(), b.to_string())));
        assert!(s.doc.bookmarks().is_empty(), "no hyperlinks, no _Toc bookmarks");
        // Settings again replace the table in place; hyperlinks bookmark the headings.
        s.run("references.toc", &json!({"levels": 3, "pageNumbers": false})).unwrap();
        assert_eq!(s.doc.plain_text(StoryRef::Body).matches("Contents").count(), 1);
        let toc: Vec<String> =
            s.doc.body.iter().filter_map(|b| b.as_para()).filter(|p| p.props.style.as_deref() == Some("TOC3")).map(|p| p.plain_text()).collect();
        assert_eq!(toc, ["Deep"]);
        let first = s.doc.body.iter().filter_map(|b| b.as_para()).find(|p| p.props.style.as_deref() == Some("TOC1")).unwrap();
        let link = first.runs[0].props.link.clone().unwrap();
        assert!(s.doc.bookmarks().iter().any(|(n, _)| format!("#{n}") == link), "{link}");
    }

    /// #402: Caption numbering — a custom label above the paragraph, letters, and chapter
    /// numbers that restart after each Heading 1 (`STYLEREF 1 \s`, `SEQ … \s 1`).
    #[test]
    fn caption_numbering_with_chapters() {
        let mut s = Session::new(wordcraft_doc::Document::from_text("One\nPic\nPic two\nTwo\nPic three"));
        for t in ["One", "Two"] {
            s.run("select.text", &json!({"text": t})).unwrap();
            s.run("styles.apply", &json!({"style": "Heading 1"})).unwrap();
        }
        let opts = json!({"label": "Photo Plate", "format": "upperLetter", "chapter": 1, "separator": "."});
        for t in ["Pic three", "Pic two", "Pic"] {
            s.run("select.text", &json!({"text": t})).unwrap();
            let mut v = opts.clone();
            v["position"] = json!("above");
            v["text"] = json!(t);
            s.run("references.caption", &v).unwrap();
        }
        let caps: Vec<String> =
            s.doc.body.iter().filter_map(|b| b.as_para()).filter(|p| p.props.style.as_deref() == Some("Caption")).map(|p| p.plain_text()).collect();
        assert_eq!(caps, ["Photo Plate 1.A: Pic", "Photo Plate 1.B: Pic two", "Photo Plate 2.A: Pic three"]);
        assert!(fields(&s).contains(&("SEQ Photo_Plate \\* ALPHABETIC \\s 1".to_string(), "A".to_string())), "{:?}", fields(&s));
        // The caption sits above its paragraph; excluding the label leaves the number.
        let i = s.doc.body.iter().position(|b| b.as_para().is_some_and(|p| p.plain_text() == "Pic")).unwrap();
        assert_eq!(s.doc.body[i - 1].as_para().unwrap().plain_text(), "Photo Plate 1.A: Pic");
        s.run("caret.docEnd", &json!({})).unwrap();
        s.run("references.caption", &json!({"label": "Table", "excludeLabel": true, "format": "ROMAN"})).unwrap();
        assert_eq!(s.doc.body.last().unwrap().as_para().unwrap().plain_text(), "I");
    }

    #[test]
    fn update_fields_ref_and_pageref() {
        let mut s = Session::new(wordcraft_doc::Document::from_text("Bookmark target text here.\nSecond paragraph."));
        s.run("select.text", &json!({"text": "Bookmark target text"})).unwrap();
        s.run("insert.bookmark", &json!({"name": "TargetMark"})).unwrap();
        s.run("caret.docEnd", &json!({})).unwrap();
        s.run("insert.field", &json!({"instr": "REF \"TargetMark\" \\h", "result": "old"})).unwrap();
        s.run("insert.field", &json!({"instr": "REF TargetMark\\h", "result": "old"})).unwrap();
        s.run("insert.field", &json!({"instr": "PAGEREF \"TargetMark\" \\* Roman", "result": "0"})).unwrap();
        s.run("insert.field", &json!({"instr": "PAGEREF TargetMark \\p", "result": "above"})).unwrap();
        s.run("insert.field", &json!({"instr": "REF TargetMark \\p", "result": "above"})).unwrap();

        update_fields(&mut s).unwrap();

        let res: Vec<(String, String)> = fields(&s);
        assert_eq!(res[0], ("REF \"TargetMark\" \\h".into(), "Bookmark target text".into()));
        assert_eq!(res[1], ("REF TargetMark\\h".into(), "Bookmark target text".into()));
        assert_eq!(res[2], ("PAGEREF \"TargetMark\" \\* Roman".into(), "I".into()));
        assert_eq!(res[3], ("PAGEREF TargetMark \\p".into(), "above".into()));
        assert_eq!(res[4], ("REF TargetMark \\p".into(), "above".into()));
    }

    /// A document with a body AUTHOR field and a header holding AUTHOR, NUMWORDS and a locked TITLE,
    /// all stale; the author is "Ada".
    fn doc_with_header_fields() -> (wordcraft_doc::Document, u32) {
        use wordcraft_doc::{CharProps, Paragraph, PartKind};
        let field = |instr: &str, locked: bool| InlineObject::Field {
            instr: instr.into(),
            result: "stale".into(),
            locked,
            dirty: false,
            code: false,
            form: String::new(),
            nested: Vec::new(),
        };
        let mut doc = wordcraft_doc::Document::from_text("one two three");
        doc.core.creator = "Ada".into();
        let mut p = Paragraph::new();
        for (instr, locked) in [("AUTHOR", false), ("NUMWORDS", false), ("TITLE", true)] {
            let off = p.len();
            p.insert_object(off, field(instr, locked), &CharProps::default()).unwrap();
        }
        let body_path = doc.para_paths(StoryRef::Body).remove(0);
        doc.para_mut(StoryRef::Body, &body_path).unwrap().insert_object(0, field("AUTHOR", false), &CharProps::default()).unwrap();
        let hdr = doc.add_part(PartKind::Header, vec![std::sync::Arc::new(wordcraft_doc::Block::Para(p))]);
        (doc, hdr)
    }

    fn story_fields(s: &Session, story: StoryRef) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for path in s.doc.para_paths(story) {
            for o in &s.doc.para(story, &path).unwrap().objects {
                if let InlineObject::Field { instr, result, .. } = o {
                    out.push((instr.clone(), result.clone()));
                }
            }
        }
        out
    }

    /// #1706 (Word check): Update Field with the caret in the body leaves a header's fields alone.
    #[test]
    fn update_fields_in_the_body_leaves_headers_alone() {
        let (doc, hdr) = doc_with_header_fields();
        let mut s = Session::new(doc);
        s.run("references.updateFields", &json!({})).unwrap();
        assert_eq!(story_fields(&s, StoryRef::Body)[0], ("AUTHOR".to_string(), "Ada".to_string()));
        assert!(story_fields(&s, StoryRef::Part(hdr)).iter().all(|(_, r)| r == "stale"), "{:?}", story_fields(&s, StoryRef::Part(hdr)));
    }

    /// #1706: with the caret in a header (or other part), Update Field refreshes that part's
    /// fields, but not locked ones and not the body's.
    #[test]
    fn update_fields_in_a_header_updates_that_header() {
        let (doc, hdr) = doc_with_header_fields();
        let mut s = Session::new(doc);
        let path = s.doc.para_paths(StoryRef::Part(hdr)).remove(0);
        s.sel = crate::session::Selection::caret(Pos::new(StoryRef::Part(hdr), path, 0));
        s.run("references.updateFields", &json!({})).unwrap();
        let got = story_fields(&s, StoryRef::Part(hdr));
        assert_eq!(got.len(), 3, "{got:?}");
        assert_eq!(got[0].1, "Ada");
        assert_eq!(got[1].1, s.doc.word_count().to_string());
        assert_eq!(got[2].1, "stale", "a locked field stays as it was");
        assert!(story_fields(&s, StoryRef::Body).iter().all(|(_, r)| r == "stale"), "the body is not touched");
    }

    /// #1706: `all: true` (printing) updates the whole document even with the caret in a header.
    #[test]
    fn update_fields_all_covers_every_story() {
        let (doc, hdr) = doc_with_header_fields();
        let mut s = Session::new(doc);
        let path = s.doc.para_paths(StoryRef::Part(hdr)).remove(0);
        s.sel = crate::session::Selection::caret(Pos::new(StoryRef::Part(hdr), path, 0));
        s.run("references.updateFields", &json!({"all": true})).unwrap();
        let got = story_fields(&s, StoryRef::Part(hdr));
        assert_eq!((got[0].1.as_str(), got[2].1.as_str()), ("Ada", "stale"), "{got:?}");
        assert_eq!(story_fields(&s, StoryRef::Body)[0].1, "Ada");
        // Junk values fall back to the caret's story.
        s.run("references.updateFields", &json!({"all": "yes"})).unwrap();
    }

    #[test]
    fn toc_tabs_ignore_nonentries() {
        let mut s = Session::new(wordcraft_doc::Document::from_text("Before\nContents\nOld\t9\nAfter"));
        let stop = |pos| TabStop { pos, align: TabAlign::Left, leader: TabLeader::None };
        for (i, pos) in [(0, 96.0), (1, 84.0), (2, 24.0)] {
            let p = s.doc.para_mut(StoryRef::Body, &Path::top(i)).unwrap();
            p.props.style = Some("TOC2".into());
            p.props.tabs = Some(vec![stop(pos)]);
        }
        s.doc
            .para_mut(StoryRef::Body, &Path::top(1))
            .unwrap()
            .insert_object(
                0,
                InlineObject::Field {
                    instr: "TOC".into(),
                    result: String::new(),
                    locked: false,
                    dirty: false,
                    code: false,
                    form: String::new(),
                    nested: Vec::new(),
                },
                &CharProps::default(),
            )
            .unwrap();
        assert_eq!(cached_toc_tabs(&s), vec![("TOC2".into(), vec![stop(24.0)])]);
    }

    /// #1230: a selected picture or shape in a table cell gets its caption in the cell, under
    /// (or over) the paragraph that holds it, not after the whole table; the number counts on.
    #[test]
    fn caption_for_an_object_in_a_cell_stays_in_the_cell() {
        let mut s = Session::new(wordcraft_doc::Document::new());
        s.run("insert.table", &json!({"rows": 1, "cols": 2})).unwrap();
        s.run("insert.shape", &json!({"kind": "ellipse", "width": 60, "height": 40})).unwrap();
        let (at, _) = crate::cmd::objects::selected(&s).expect("the shape is selected");
        let (table, r, c) = at.path.cell().expect("the shape is in a cell");
        s.run("references.caption", &json!({"label": "Figure", "text": "Kiln"})).unwrap();
        let cell = |s: &Session| s.doc.table(StoryRef::Body, &table).unwrap().rows[r].cells[c].blocks.clone();
        let texts: Vec<String> = cell(&s).iter().filter_map(|b| b.as_para()).map(|p| p.plain_text()).collect();
        assert_eq!(texts.len(), 2, "{texts:?}");
        assert_eq!(texts[1], "Figure 1: Kiln", "the caption is under the shape, in the cell");
        assert_eq!(s.doc.body.iter().filter(|b| b.as_para().is_some_and(|p| p.props.style.as_deref() == Some("Caption"))).count(), 0);
        // Above: over the shape's paragraph, and it is Figure 1 (the one below becomes 2).
        let end = Pos { off: at.off + wordcraft_doc::para::OBJ.len_utf8(), ..at.clone() };
        s.sel = Selection { anchor: at.clone(), focus: end };
        assert!(crate::cmd::objects::object_selection(&s).is_some());
        s.run("references.caption", &json!({"label": "Figure", "text": "Lid", "position": "above"})).unwrap();
        let texts: Vec<String> = cell(&s).iter().filter_map(|b| b.as_para()).map(|p| p.plain_text()).collect();
        assert_eq!(texts.len(), 3, "{texts:?}");
        assert_eq!((texts[0].as_str(), texts[2].as_str()), ("Figure 1: Lid", "Figure 2: Kiln"));
        // A picture outside a table: the caption goes right after its paragraph.
        let mut s = Session::new(wordcraft_doc::Document::from_text("Before\nAfter"));
        s.run("select.text", &json!({"text": "Before"})).unwrap();
        s.run("caret.end", &json!({})).unwrap();
        s.run("insert.shape", &json!({"kind": "ellipse", "width": 60, "height": 40})).unwrap();
        s.run("references.caption", &json!({})).unwrap();
        let texts: Vec<String> = s.doc.body.iter().filter_map(|b| b.as_para()).map(|p| p.plain_text()).collect();
        assert_eq!(texts.get(1).map(String::as_str), Some("Figure 1"), "{texts:?}");
    }
}
