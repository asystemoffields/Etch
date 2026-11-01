//! Undo, clipboard, find & replace, Go To, Format Painter.

use serde_json::{Value, json};
use wordcraft_doc::edit::Fragment;
use wordcraft_doc::{PartKind, Pos, StoryRef};

use super::{delete_selection, find_pattern, pos_json, sel_result};
use crate::{CmdError, CmdResult, CommandSpec, Selection, Session, p};

/// Undo several steps at once: the Undo button's list (#701). `count` is validated here and
/// clamped to the undo depth, so hostile numbers never reach the stack.
fn undo_to(s: &mut Session, v: &Value) -> CmdResult {
    let n = p::u64(v, "count").ok_or_else(|| CmdError::Params("`count` (whole number, 1 or more) is required".into()))?;
    if n == 0 {
        return Err(CmdError::Params("`count` must be 1 or more".into()));
    }
    let n = usize::try_from(n).unwrap_or(usize::MAX).min(s.undo_depth());
    if s.step_lifts_protection(false) {
        return Err(CmdError::Disabled("edit.undoTo: use Stop Protection to lift the document's protection".into()));
    }
    let undone = s.undo_steps(n);
    let mut r = sel_result(s)?;
    if let Some(m) = r.as_object_mut() {
        m.insert("undone".into(), json!(undone));
        m.insert("depth".into(), json!(s.undo_depth()));
    }
    Ok(r)
}

fn has_sel(s: &Session) -> Option<&'static str> {
    if s.sel.is_collapsed() { Some("nothing is selected") } else { None }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec::new("edit.undo", "Undo", "Quick Access Toolbar", |s, _| {
            if s.step_lifts_protection(false) {
                return Err(CmdError::Disabled("edit.undo: use Stop Protection to lift the document's protection".into()));
            }
            s.undo();
            sel_result(s)
        })
        .key("Mod+Z")
        .pure(),
        CommandSpec::new("edit.undoTo", "Undo Actions", "Quick Access Toolbar", undo_to)
            .params(r#"{"count": n (1 = the newest step, like edit.undo; n undoes the newest n steps at once; at most the undo depth)}"#)
            .when(|s| if s.can_undo() { None } else { Some("there is nothing to undo") })
            .pure(),
        CommandSpec::new("edit.undoHistory", "Undo History", "Quick Access Toolbar", |s, _| {
            Ok(json!({"undo": s.undo_history(), "depth": s.undo_depth(), "redo": s.redo_label()}))
        })
        .pure(),
        CommandSpec::new("edit.redo", "Redo", "Quick Access Toolbar", |s, _| {
            if s.step_lifts_protection(true) {
                return Err(CmdError::Disabled("edit.redo: use Stop Protection to lift the document's protection".into()));
            }
            s.redo();
            sel_result(s)
        })
        .key("Mod+Y / Mod+Shift+Z")
        .pure(),
        CommandSpec::new("edit.cut", "Cut", "Home › Clipboard", cut).key("Mod+X").when(has_sel),
        CommandSpec::new("edit.copy", "Copy", "Home › Clipboard", copy).key("Mod+C / Mod+Insert").pure(),
        // Word's Spike (Ctrl+F3, Ctrl+Shift+F3; ⌘ on the Mac): the AutoText entry "Spike", so Quick
        // Parts and AutoText insert it without emptying it, as in Word.
        CommandSpec::new("edit.spike", "Spike", "Editing", spike).key("Mod+F3").when(has_sel),
        CommandSpec::new("edit.insertSpike", "Insert Spike", "Editing", insert_spike).key("Mod+Shift+F3").when(|s| {
            if s.math.is_some() {
                Some("leave the equation first")
            } else if s.building_blocks.contains_key(SPIKE) {
                None
            } else {
                Some("the Spike is empty")
            }
        }),
        CommandSpec::new("edit.paste", "Paste", "Home › Clipboard", paste).key("Mod+V / Shift+Insert").params(r#"{"text"?: string, "html"?: string, "rtf"?: string}"#),
        CommandSpec::new("edit.pasteText", "Paste: Keep Text Only", "Home › Clipboard › Paste", paste_text)
            .params(r#"{"text"?: string}"#)
            .key("Mod+Shift+Alt+V"),
        CommandSpec::new("edit.pasteMerge", "Paste: Merge Formatting", "Home › Clipboard › Paste", paste_merge)
            .params(r#"{"text"?: string, "html"?: string, "rtf"?: string}"#),
        CommandSpec::new("edit.clipboardPane", "Clipboard Pane", "Home › Clipboard", |s, v| {
            let on = p::bool(v, "value").unwrap_or(!s.view.clipboard_pane);
            s.view.clipboard_pane = on;
            Ok(json!({"value": on, "count": s.clip_history.len()}))
        })
        .params(r#"{"value"?: bool}"#)
        .pure(),
        CommandSpec::new("edit.clipboardItems", "Clipboard Items", "Home › Clipboard › Clipboard Pane", |s, _| Ok(s.clip_history.describe())).pure(),
        CommandSpec::new("edit.pasteClipboardItem", "Paste Clipboard Item", "Home › Clipboard › Clipboard Pane", paste_clip_item)
            .params(r#"{"index": n (0 = newest)}"#)
            .when(has_clips),
        CommandSpec::new("edit.pasteAllClipboard", "Paste All", "Home › Clipboard › Clipboard Pane", paste_all_clips).when(has_clips),
        CommandSpec::new("edit.deleteClipboardItem", "Delete Clipboard Item", "Home › Clipboard › Clipboard Pane", |s, v| {
            let i = clip_index(s, v)?;
            s.clip_history.items.remove(i);
            Ok(s.clip_history.describe())
        })
        .params(r#"{"index": n (0 = newest)}"#)
        .when(has_clips)
        .pure(),
        CommandSpec::new("edit.clearClipboard", "Clear All", "Home › Clipboard › Clipboard Pane", |s, _| {
            s.clip_history.items.clear();
            Ok(s.clip_history.describe())
        })
        .pure(),
        CommandSpec::new("edit.find", "Find", "Home › Editing", find)
            .key("Mod+F")
            .params(r#"{"text": string, "matchCase"?: bool, "wholeWord"?: bool, "matchPrefix"?: bool, "matchSuffix"?: bool, "ignorePunct"?: bool, "ignoreSpace"?: bool, "wildcards"?: bool, "special"?: bool (^p ^t ^# … codes, default true), "regex"?: bool}"#)
            .pure(),
        CommandSpec::new("edit.advancedFind", "Advanced Find", "Home › Editing › Find", advanced_find)
            .params(r#"{"text"?: string, …options of edit.find, "in"?: "main|selection", "highlight"?: bool}"#)
            .pure(),
        CommandSpec::new("edit.findNext", "Find Next", "Home › Editing › Find", |s, v| {
            read_opts(s, v)?;
            step(s, 1)
        })
        .key("Mod+G / F3 / Shift+F4 / Mod+PageDown")
        .params(r#"{"text"?: string, "matchCase"?: bool, "wholeWord"?: bool, "regex"?: bool}"#)
        .pure(),
        CommandSpec::new("edit.findPrevious", "Find Previous", "Home › Editing › Find", |s, v| {
            read_opts(s, v)?;
            step(s, -1)
        })
        // Elsewhere Ctrl+Shift+G is Word Count; Ctrl+Page Up/Down browse the find results.
        .key(super::mac_or("Mod+Shift+G / Mod+PageUp", "Mod+PageUp"))
        .params(r#"{"text"?: string, "matchCase"?: bool, "wholeWord"?: bool, "regex"?: bool}"#)
        .pure(),
        CommandSpec::new("edit.replace", "Replace", "Home › Editing", replace).params(r#"{"text": string, "with": string, …options of edit.replaceAll}"#),
        // Ctrl+H, Home › Replace: the Find and Replace dialog on its Replace tab (#712). Opening
        // it changes nothing, so it is its own command and never marks the document edited.
        CommandSpec::new("edit.findReplace", "Find and Replace", "Home › Editing", |s, v| {
            let tab = match p::str(v, "tab").unwrap_or("replace") {
                t @ ("find" | "replace" | "goto") => t,
                other => return Err(CmdError::Params(format!("`tab` is find, replace or goto, not `{other}`"))),
            };
            s.ui_requests.push(json!({"open": tab}));
            Ok(json!({"open": tab}))
        })
        // Word for Mac's own key is ⌃H.
        .key(super::mac_or("Mod+H / Ctrl+H", "Mod+H"))
        .params(r#"{"tab"?: "find|replace|goto" (default replace)}"#)
        .pure(),
        CommandSpec::new("edit.replaceAll", "Replace All", "Home › Editing › Replace", replace_all)
            .params(r#"{"text": string, "with": string (^p ^t ^& ^c …; \\1 with wildcards), "in"?: "all" (every story: body, headers, footers, notes, text boxes and comments, as Word's dialog; default) | "story" (the caret's story only), …options of edit.find}"#),
        CommandSpec::new("edit.goto", "Go To", "Home › Editing › Find", goto)
            .key("Mod+Alt+G / F5")
            .params(r#"{"page"?: n, "pageBy"?: int (pages forward, negative back, from the caret's page), "bookmark"?: string (selects its text), "paragraph"?: n}; none opens Find and Replace on its Go To tab"#)
            .pure(),
        CommandSpec::new("edit.goBack", "Go Back", "Navigation", |s, _| {
            let p = s.go_back().ok_or_else(|| CmdError::Failed("no edits to go back to yet".into()))?;
            s.sel = Selection::caret(p);
            s.goal_x = None;
            sel_result(s)
        })
        .key("Shift+F5 / Mod+Alt+Z")
        .pure(),
        CommandSpec::new("edit.formatPainter", "Format Painter", "Home › Clipboard", painter).params(r#"{"sticky"?: bool}"#).pure(),
        CommandSpec::new("edit.copyFormat", "Copy Formatting", "Home › Clipboard", |s, _| {
            painter_pick(s);
            sel_result(s)
        })
        .key("Mod+Shift+C")
        .pure(),
        CommandSpec::new("edit.pasteFormat", "Paste Formatting", "Home › Clipboard", paste_format).key("Mod+Shift+V"),
    ]
}

pub(crate) fn copy(s: &mut Session, _: &Value) -> CmdResult {
    let (a, b) = s.sel.ordered();
    if a == b {
        return Ok(json!({"text": ""}));
    }
    // Whole cells copy as a table of those cells (#1744), not as their paragraphs.
    let f = super::table::copy_cells(s).unwrap_or_else(|| s.doc.copy_range(&a, &b));
    s.clipboard_text = labelled_text(s, &a, &b, &f).unwrap_or_else(|| f.clipboard_text());
    s.clip_history.push(&f, &s.clipboard_text);
    s.clipboard = Some(f);
    Ok(copy_result(s))
}

/// The plain text of copied list items with their numbers or bullets (#2015): `1.<tab>Item`, as
/// another program pasting plain text shows the list. An item copied from part-way through has
/// no number. `None` when no copied paragraph shows one, or the copy isn't paragraphs side by
/// side (whole cells, or a range in and out of a table), whose text stays as it was.
fn labelled_text(s: &Session, a: &Pos, b: &Pos, f: &Fragment) -> Option<String> {
    use wordcraft_doc::numbering::{Counters, LevelSuffix};
    if a.story != b.story || a.path.parent() != b.path.parent() || f.blocks.iter().any(|x| x.as_para().is_none()) {
        return None;
    }
    let first = a.path.last();
    let last = b.path.last();
    if f.blocks.len() != last.checked_sub(first)? + 1 {
        return None;
    }
    // Every numbered paragraph up to the copy advances the counters, as on the page.
    let mut counters = Counters::default();
    let mut labels = vec![None; f.blocks.len()];
    for path in s.doc.para_paths(a.story) {
        if path > b.path {
            break;
        }
        let Some(p) = s.doc.para(a.story, &path) else { continue };
        let Some(n) = p.props.numbering.or_else(|| s.doc.styles.resolve_para(&p.props).numbering).filter(|n| n.num != 0) else { continue };
        let Some((label, def)) = counters.next_label(&s.doc.numbering, n.num, n.level) else { continue };
        if path.parent() == a.path.parent() && (first..=last).contains(&path.last()) && !label.is_empty() {
            // A bullet from a symbol font's private-use characters reads as a plain bullet.
            let label: String = label.chars().map(|c| if ('\u{F000}'..='\u{F0FF}').contains(&c) { '•' } else { c }).collect();
            let sep = match def.suffix {
                LevelSuffix::Tab => "\t",
                LevelSuffix::Space => " ",
                LevelSuffix::Nothing => "",
            };
            if let Some(slot) = labels.get_mut(path.last() - first) {
                *slot = Some(format!("{label}{sep}"));
            }
        }
    }
    // The first item copied from part-way through shows no number.
    if a.off > 0
        && let Some(slot) = labels.first_mut()
    {
        *slot = None;
    }
    // Nor does the paragraph a selection ends at the start of (only its paragraph mark is copied).
    if b.off == 0
        && last > first
        && let Some(slot) = labels.last_mut()
    {
        *slot = None;
    }
    if labels.iter().all(Option::is_none) {
        return None;
    }
    let lines: Vec<String> = f
        .blocks
        .iter()
        .zip(labels)
        .filter_map(|(blk, label)| blk.as_para().map(|p| format!("{}{}", label.unwrap_or_default(), p.shown_text())))
        .collect();
    Some(lines.join("\n"))
}

/// What Copy and Cut return: the clipboard's text, and the copy as HTML (equations as MathML,
/// #816) for the host to put on the system clipboard too, so other programs keep the formatting;
/// and for a copy of one equation alone, its MathML (#1297).
pub(crate) fn copy_result(s: &mut Session) -> Value {
    s.clipboard_html = match &s.clipboard {
        Some(f) => {
            // The lists and pictures the copy refers to are the document's: the HTML needs their
            // numbering and bytes, or another window pastes plain paragraphs without the
            // pictures (both are shared, so this copies nothing).
            let mut d = f.to_document();
            d.numbering = s.doc.numbering.clone();
            d.media = s.doc.media.clone();
            wordcraft_formats::html::export_fragment(&d)
        }
        None => String::new(),
    };
    let mut r = json!({"text": s.clipboard_text});
    if !s.clipboard_html.is_empty() {
        r["html"] = json!(s.clipboard_html);
    }
    // A copy of one equation alone, as MathML for the clipboard formats equation editors read
    // ("MathML" on Windows, `public.mathml.presentation` on macOS; #1297). Word looks for them
    // before anything else when it pastes; a copy with text around the equation doesn't offer
    // them, so pasting it keeps the text.
    if let Some((linear, display)) = s.clipboard.as_ref().and_then(|f| f.sole_equation()) {
        r["mathml"] = json!(wordcraft_formats::mathml::linear_to_mathml(linear, display));
    }
    r
}

fn cut(s: &mut Session, v: &Value) -> CmdResult {
    let r = copy(s, v)?;
    // A tracked cut of a paragraph or sentence may become a move when pasted.
    if !super::moves::cut_tracked(s)? {
        delete_selection(s)?;
    }
    super::controls::restore_placeholder(s)?;
    Ok(r)
}

/// The AutoText entry the Spike collects into.
const SPIKE: &str = "Spike";

/// Ctrl+F3: cut the selection to the end of the Spike. The clipboard is left alone.
fn spike(s: &mut Session, _: &Value) -> CmdResult {
    let (a, b) = s.sel.ordered();
    // Column selection collapses the selection before the command runs.
    if a == b {
        return Err(CmdError::Disabled("edit.spike: nothing is selected".into()));
    }
    let piece = s.doc.copy_range(&a, &b);
    if piece.blocks.is_empty() {
        return Err(CmdError::Disabled("edit.spike: this selection can't be collected".into()));
    }
    delete_selection(s)?;
    super::controls::restore_placeholder(s)?;
    s.building_blocks.entry(SPIKE.to_string()).or_default().append_piece(piece);
    sel_result(s)
}

/// Ctrl+Shift+F3: insert everything the Spike collected, in order, and empty it.
fn insert_spike(s: &mut Session, _: &Value) -> CmdResult {
    let frag = s.building_blocks.get(SPIKE).cloned().ok_or_else(|| CmdError::Disabled("edit.insertSpike: the Spike is empty".into()))?;
    super::paste::insert(s, frag)?;
    s.building_blocks.remove(SPIKE);
    sel_result(s)
}

/// Paste keeps the source formatting: our own copy while the system clipboard still holds it,
/// else another program's HTML or RTF (#687). The system clipboard's text has none, so it pastes
/// as Keep Text Only does.
fn paste(s: &mut Session, v: &Value) -> CmdResult {
    match super::paste::paste_content(s, v)? {
        super::paste::Pasted::Fragment(frag) => super::paste::insert(s, frag)?,
        super::paste::Pasted::Text(text) => super::paste::insert_text(s, &text)?,
    }
    sel_result(s)
}

/// Merge Formatting: the pasted text takes the destination's formatting, keeping its emphasis.
fn paste_merge(s: &mut Session, v: &Value) -> CmdResult {
    let mut frag = super::paste::paste_fragment(s, v)?;
    super::paste::merge_formatting(s, &mut frag);
    super::paste::insert(s, frag)?;
    sel_result(s)
}

fn has_clips(s: &Session) -> Option<&'static str> {
    if s.clip_history.is_empty() { Some("the Clipboard pane is empty") } else { None }
}

fn clip_index(s: &Session, v: &Value) -> Result<usize, CmdError> {
    let i = p::u64(v, "index").ok_or_else(|| CmdError::Params("`index` (number, 0 = newest) is required".into()))?;
    usize::try_from(i)
        .ok()
        .filter(|i| *i < s.clip_history.len())
        .ok_or_else(|| CmdError::Params(format!("no clipboard item {i} (there are {})", s.clip_history.len())))
}

/// Paste a collected item through the normal paste path. The clipboard itself stays as it was.
fn paste_clip(s: &mut Session, f: Fragment) -> CmdResult {
    let saved = s.clipboard.replace(f);
    let r = paste(s, &json!({}));
    s.clipboard = saved;
    r
}

fn paste_clip_item(s: &mut Session, v: &Value) -> CmdResult {
    let i = clip_index(s, v)?;
    let f = s.clip_history.items.get(i).map(|x| x.fragment.clone()).ok_or_else(|| CmdError::Params("no such clipboard item".into()))?;
    paste_clip(s, f)
}

/// Every collected item, in the order they were copied (one undo step).
fn paste_all_clips(s: &mut Session, _: &Value) -> CmdResult {
    let all: Vec<Fragment> = s.clip_history.items.iter().rev().map(|x| x.fragment.clone()).collect();
    let mut r = sel_result(s)?;
    for f in all {
        r = paste_clip(s, f)?;
    }
    Ok(r)
}

fn paste_text(s: &mut Session, v: &Value) -> CmdResult {
    let text = match p::str(v, "text") {
        Some(t) => t.to_string(),
        None => s.clipboard.as_ref().map(Fragment::plain_text).unwrap_or_default(),
    };
    if text.is_empty() {
        return Err(CmdError::Failed("the clipboard is empty".into()));
    }
    super::paste_text(s, &text.replace("\r\n", "\r").replace('\n', "\r"))?;
    sel_result(s)
}

/// One match: its range and, when asked for, the Replace with text filled in for it.
pub(crate) struct Found {
    pub a: Pos,
    pub b: Pos,
    pub rep: Option<String>,
}

/// A stretch of searched text: `len` bytes from `hay` are paragraph `para`'s bytes from `raw`.
/// A paragraph mark is a one-byte stretch at the paragraph's end (`mark`).
struct Seg {
    hay: usize,
    para: usize,
    raw: usize,
    len: usize,
    mark: bool,
}

/// All matches of the find state in the current story. Text marked deleted by
/// Track Changes is no longer part of the document, so the search reads each
/// paragraph without it: a match lies within one stretch of live text, and word
/// boundaries and anchors see the neighbouring live text, not the deleted text.
/// Otherwise Replace would find its own deleted text again.
pub(crate) fn search(s: &Session, story: StoryRef) -> Result<Vec<(Pos, Pos)>, CmdError> {
    Ok(search_full(s, story, false)?.into_iter().map(|f| (f.a, f.b)).collect())
}

fn find_opts(f: &crate::session::FindState) -> find_pattern::Opts {
    find_pattern::Opts {
        match_case: f.match_case,
        whole_word: f.whole_word,
        prefix: f.match_prefix,
        suffix: f.match_suffix,
        ignore_punct: f.ignore_punct,
        ignore_space: f.ignore_space,
        wildcards: f.wildcards,
        regex: f.regex,
        literal: f.literal,
    }
}

/// [`search`], with the Replace with text of each match when `with_rep` (special codes, `^&`,
/// wildcard groups). A pattern that can match a paragraph mark (`^p`) reads runs of sibling
/// paragraphs as one text with a mark between them; otherwise each paragraph is read alone.
pub(crate) fn search_full(s: &Session, story: StoryRef, with_rep: bool) -> Result<Vec<Found>, CmdError> {
    use regex_automata::{Input, meta, util::syntax};
    let f = &s.find;
    if f.query.is_empty() {
        return Ok(Vec::new());
    }
    let opts = find_opts(f);
    let c = find_pattern::compile(&f.query, &opts).map_err(CmdError::Params)?;
    let re = meta::Regex::builder()
        .syntax(syntax::Config::new().case_insensitive(c.case_insensitive))
        .configure(meta::Config::new().nfa_size_limit(Some(1 << 20)))
        .build(&c.pattern)
        .map_err(|e| CmdError::Params(format!("bad pattern: {e}")))?;
    let pieces = with_rep.then(|| find_pattern::parse_replace(&f.replace, &opts));
    let paths = s.doc.para_paths(story);
    // Paragraphs read as one text: consecutive siblings when the pattern spans paragraphs.
    let mut chunks: Vec<std::ops::Range<usize>> = Vec::new();
    for i in 0..paths.len() {
        let joins =
            c.spans_paras && i > 0 && paths.get(i - 1).zip(paths.get(i)).is_some_and(|(x, y)| x.parent() == y.parent() && x.last() + 1 == y.last());
        match chunks.last_mut() {
            Some(r) if joins => r.end = i + 1,
            _ => chunks.push(i..i + 1),
        }
    }
    let mut out = Vec::new();
    let mut rep_bytes = 0usize;
    let mut caps = re.create_captures();
    for chunk in chunks {
        let mut hay = String::new();
        let mut segs: Vec<Seg> = Vec::new();
        // Where the live text breaks (tracked-deleted text, or a paragraph edge when not spanning).
        let mut cuts: Vec<usize> = Vec::new();
        let last = chunk.end.saturating_sub(1);
        for i in chunk {
            let Some(p) = paths.get(i).and_then(|path| s.doc.para(story, path)) else {
                cuts.push(hay.len());
                continue;
            };
            let mut expect = 0;
            for (r, ch) in p.run_ranges() {
                let Some(t) = p.text.get(r.clone()).filter(|_| ch.del.is_none()) else { continue };
                if t.is_empty() {
                    continue;
                }
                if r.start != expect {
                    cuts.push(hay.len());
                }
                match segs.last_mut() {
                    Some(sg) if !sg.mark && sg.para == i && sg.raw + sg.len == r.start => sg.len += t.len(),
                    _ => segs.push(Seg { hay: hay.len(), para: i, raw: r.start, len: t.len(), mark: false }),
                }
                hay.push_str(t);
                expect = r.end;
            }
            if i < last {
                if expect != p.text.len() {
                    cuts.push(hay.len());
                }
                segs.push(Seg { hay: hay.len(), para: i, raw: p.text.len(), len: 1, mark: true });
                hay.push(if p.section.is_some() { find_pattern::SECT_MARK } else { find_pattern::PARA_MARK });
            }
        }
        let pos_of = |x: usize, end: bool| -> Option<Pos> {
            let k = segs.partition_point(|sg| if end { sg.hay + sg.len < x } else { sg.hay + sg.len <= x });
            let sg = segs.get(k)?;
            if (end && sg.hay >= x) || (!end && sg.hay > x) {
                return None;
            }
            if sg.mark && x > sg.hay {
                let path = paths.get(sg.para)?;
                return Some(Pos { story, path: path.with_last(path.last() + 1), off: 0 });
            }
            Some(Pos { story, path: paths.get(sg.para)?.clone(), off: sg.raw + (x - sg.hay) })
        };
        cuts.push(hay.len());
        let mut from = 0;
        for cut in cuts {
            if cut <= from {
                continue;
            }
            let mut input = Input::new(&hay).range(from..cut);
            from = cut;
            loop {
                re.search_captures(&input, &mut caps);
                let Some(m) = caps.get_match() else { break };
                // Step past an empty match so the search moves on.
                let next = if m.is_empty() {
                    hay.get(m.end()..).and_then(|t| t.chars().next()).map_or(m.end() + 1, |c| m.end() + c.len_utf8())
                } else {
                    m.end()
                };
                if next > input.end() {
                    break;
                }
                input.set_start(next);
                if m.is_empty() {
                    continue;
                }
                let (Some(a), Some(b)) = (pos_of(m.start(), false), pos_of(m.end(), true)) else { continue };
                let rep = pieces.as_ref().map(|pc| {
                    let group = |n: usize| caps.get_group(n).and_then(|g| hay.get(g.range())).map(str::to_string);
                    find_pattern::expand(pc, hay.get(m.range()).unwrap_or(""), &group, &s.clipboard_text)
                });
                rep_bytes = rep_bytes.saturating_add(rep.as_ref().map_or(0, String::len));
                if rep_bytes > MAX_REPLACE_BYTES {
                    return Err(CmdError::Disabled(format!("the replacements are too large (over {} MB)", MAX_REPLACE_BYTES >> 20)));
                }
                out.push(Found { a, b, rep });
                if out.len() >= 100_000 {
                    return Ok(out);
                }
            }
        }
    }
    Ok(out)
}

/// The longest Replace with text, in characters (Word's box holds 255).
pub const MAX_REPLACE_CHARS: usize = 255;
/// The most bytes the replacements of one search may add up to (`^c` and `^&` expand per match).
pub const MAX_REPLACE_BYTES: usize = 64 << 20;

fn read_opts(s: &mut Session, v: &Value) -> Result<(), CmdError> {
    if p::str(v, "with").is_some_and(|t| t.chars().count() > MAX_REPLACE_CHARS) {
        return Err(CmdError::Params(format!("Replace with is at most {MAX_REPLACE_CHARS} characters")));
    }
    // A new search changes what Reading Highlight marks.
    s.find.highlights = None;
    if let Some(t) = p::str(v, "text") {
        s.find.query = t.to_string();
    }
    if let Some(b) = p::bool(v, "matchCase") {
        s.find.match_case = b;
    }
    if let Some(b) = p::bool(v, "wholeWord") {
        s.find.whole_word = b;
    }
    if let Some(b) = p::bool(v, "regex") {
        s.find.regex = b;
    }
    if let Some(b) = p::bool(v, "wildcards") {
        s.find.wildcards = b;
    }
    if let Some(b) = p::bool(v, "special") {
        s.find.literal = !b;
    }
    if let Some(b) = p::bool(v, "matchPrefix") {
        s.find.match_prefix = b;
    }
    if let Some(b) = p::bool(v, "matchSuffix") {
        s.find.match_suffix = b;
    }
    if let Some(b) = p::bool(v, "ignorePunct") {
        s.find.ignore_punct = b;
    }
    if let Some(b) = p::bool(v, "ignoreSpace") {
        s.find.ignore_space = b;
    }
    if let Some(t) = p::str(v, "with") {
        s.find.replace = t.to_string();
    }
    Ok(())
}

fn find(s: &mut Session, v: &Value) -> CmdResult {
    read_opts(s, v)?;
    if p::str(v, "text").is_none() {
        // Word's Ctrl+F: the Navigation pane with its search box ready to type in (#712); the
        // Find and Replace dialog is Advanced Find.
        s.view.nav_pane = true;
        s.ui_requests.push(json!({"focus": "navSearch"}));
        return Ok(json!({"count": s.find.results.len()}));
    }
    let story = s.sel.focus.story;
    s.find.results = search(s, story)?;
    // Select the first match after the caret.
    let caret = s.sel.focus.clone();
    let idx = s.find.results.iter().position(|(a, _)| *a >= caret).unwrap_or(0);
    s.find.current = idx;
    if let Some((a, b)) = s.find.results.get(idx).cloned() {
        s.sel = Selection { anchor: a, focus: b };
    }
    Ok(json!({
        "count": s.find.results.len(),
        "matches": s.find.results.iter().take(200).map(|(a, b)| json!({"start": pos_json(a), "end": pos_json(b)})).collect::<Vec<_>>(),
    }))
}

/// Find in the classic dialog: every match in the body or the selection, optionally highlighted while you read.
fn advanced_find(s: &mut Session, v: &Value) -> CmdResult {
    if p::str(v, "text").is_none() && v.get("highlight").is_none() {
        s.ui_requests.push(json!({"open": "find"}));
        return Ok(json!({"count": s.find.results.len()}));
    }
    read_opts(s, v)?;
    let within = match p::str(v, "in").unwrap_or("main") {
        "main" => None,
        "selection" if !s.sel.is_collapsed() => Some(s.sel.ordered()),
        "selection" => return Err(CmdError::Params("`in: selection` needs a selection".into())),
        other => return Err(CmdError::Params(format!("`in` is main or selection, not `{other}`"))),
    };
    let story = within.as_ref().map(|(a, _)| a.story).unwrap_or(StoryRef::Body);
    let mut found = search(s, story)?;
    if let Some((a, b)) = &within {
        found.retain(|(x, y)| x >= a && y <= b);
    }
    if let Some(h) = p::bool(v, "highlight") {
        s.find.highlight = h;
        s.find.highlights = None;
    }
    let n = found.len();
    let matches: Vec<Value> = found.iter().take(200).map(|(a, b)| json!({"start": pos_json(a), "end": pos_json(b)})).collect();
    s.find.current = 0;
    s.find.results = found;
    Ok(json!({"count": n, "matches": matches, "highlight": s.find.highlight}))
}

fn step(s: &mut Session, dir: i64) -> CmdResult {
    let story = s.sel.focus.story;
    s.find.results = search(s, story)?;
    let n = s.find.results.len();
    if n == 0 {
        return Err(CmdError::Failed("no matches".into()));
    }
    let caret = if dir > 0 { s.sel.ordered().1 } else { s.sel.ordered().0 };
    let idx = if dir > 0 {
        s.find.results.iter().position(|(a, _)| *a >= caret).unwrap_or(0)
    } else {
        s.find.results.iter().rposition(|(_, b)| *b <= caret).unwrap_or(n - 1)
    };
    s.find.current = idx;
    if let Some((a, b)) = s.find.results.get(idx).cloned() {
        s.sel = Selection { anchor: a, focus: b };
    }
    Ok(json!({"index": idx, "count": n}))
}

fn replace(s: &mut Session, v: &Value) -> CmdResult {
    read_opts(s, v)?;
    // The replacement takes the found text's formatting, never formatting waiting at a caret.
    s.pending = None;
    let story = s.sel.focus.story;
    let results = search_full(s, story, true)?;
    let (a, b) = s.sel.ordered();
    // Replace the current match if selected, then move to the next.
    if let Some(f) = results.into_iter().find(|f| f.a == a && f.b == b) {
        super::paste_text(s, &f.rep.unwrap_or_default())?;
    }
    let _ = step(s, 1);
    Ok(json!({"remaining": search(s, story)?.len()}))
}

/// Every match replaced, last first so earlier positions stay valid (one undo step). A `\r` in
/// the replacement (`^p`) splits the paragraph.
///
/// As Word's Replace All with Search: All, it covers every story: the body, headers and footers,
/// footnotes and endnotes, text boxes and comments, but not text boxes inside headers and
/// footers (`docs/word-behaviour-research.md`, Replace All scope). `"in": "story"` keeps to
/// the caret's story, as a recorded macro does. With Track Changes on every replacement is a
/// tracked deletion and insertion, in every story alike.
fn replace_all(s: &mut Session, v: &Value) -> CmdResult {
    read_opts(s, v)?;
    let stories = match p::str(v, "in").unwrap_or("all") {
        "all" => replace_stories(&s.doc),
        "story" => vec![s.sel.focus.story],
        other => return Err(CmdError::Params(format!("`in` is all or story, not `{other}`"))),
    };
    // Every match found, and checked against content-control locks, before anything changes.
    let mut found: Vec<Vec<Found>> = Vec::with_capacity(stories.len());
    for story in stories {
        let results = search_full(s, story, true)?;
        super::controls::check_deletions(s, story, results.iter().map(|f| (&f.a, &f.b)))?;
        found.push(results);
    }
    let n: usize = found.iter().map(Vec::len).sum();
    // Each replacement takes its match's formatting, never formatting waiting at a caret.
    s.pending = None;
    let old = s.sel.clone();
    for Found { a, b, rep } in found.into_iter().flat_map(|r| r.into_iter().rev()) {
        let with = rep.unwrap_or_default();
        if s.doc.settings.track_changes {
            // Same path as a single replace, so the change is a reviewable revision.
            s.sel = Selection { anchor: a, focus: b };
            super::paste_text(s, &with)?;
            continue;
        }
        // Replacing a symbol gives the new text the symbol's formatting, not its own font.
        let props = s.doc.para_at(&a).map(|p| p.props_of_char(a.off).clone().without_symbol()).unwrap_or_default();
        // A match inside one paragraph with no objects in it (so no content control marker to
        // keep) is edited in that paragraph alone: `delete_range` looks over the story each time,
        // which made Replace All quadratic.
        if a.path == b.path && a.story == b.story && !with.contains(['\r', '\u{2029}']) {
            let para = s.doc.para_mut(a.story, &a.path)?;
            if para.text.get(a.off..b.off).is_some_and(|t| !t.contains(wordcraft_doc::para::OBJ)) {
                para.delete(a.off, b.off)?;
                para.insert_text(a.off, &with, &props)?;
                continue;
            }
        }
        let mut at = s.doc.delete_range(&a, &b)?;
        for (k, part) in with.split(['\r', '\u{2029}']).enumerate() {
            if k > 0 {
                at = s.doc.split_paragraph(&at)?;
            }
            if !part.is_empty() {
                at = s.doc.insert_text(&at, part, &props)?;
            }
        }
    }
    // Tracked replacements select each match as they go: the caret stays in its own story.
    if s.sel.focus.story != old.focus.story {
        s.sel = old;
        s.clamp_selection();
    }
    s.status = format!("All done. We made {n} replacements.");
    Ok(json!({"replaced": n}))
}

/// The stories Replace All covers, the body first: every part but a text box anchored in a
/// header or footer (which Word's Find and Replace doesn't reach).
fn replace_stories(doc: &wordcraft_doc::Document) -> Vec<StoryRef> {
    let in_header_or_footer = |id: u32| {
        doc.text_box_anchor(id).is_some_and(|at| match at.story {
            StoryRef::Part(h) => doc.parts.get(&h).is_some_and(|p| matches!(p.kind, PartKind::Header | PartKind::Footer)),
            StoryRef::Body => false,
        })
    };
    std::iter::once(StoryRef::Body)
        .chain(doc.parts.iter().filter(|(id, part)| part.kind != PartKind::TextBox || !in_header_or_footer(**id)).map(|(id, _)| StoryRef::Part(*id)))
        .collect()
}

fn goto(s: &mut Session, v: &Value) -> CmdResult {
    // Relative pages, Go To's "+2" / "-1": from the page the caret is on.
    let by = v.get("pageBy").and_then(Value::as_i64);
    let rel = by.map(|by| {
        let here = s.layout().caret_on(&s.sel.focus, s.page_hint).map_or(0, |c| c.page) as i64;
        (here + 1).saturating_add(by).max(1) as u64
    });
    if let Some(pg) = rel.or_else(|| p::u64(v, "page")) {
        let l = s.layout();
        let idx = (pg.max(1) as usize - 1).min(l.pages.len().saturating_sub(1));
        let target = l.pages.get(idx).and_then(|page| {
            page.items.iter().find_map(|it| match it {
                wordcraft_layout::Placed::Lines { story, path, para, l0, .. } if *story == StoryRef::Body => {
                    para.lines.get(*l0).map(|ln| Pos { story: *story, path: path.clone(), off: ln.start })
                }
                _ => None,
            })
        });
        if let Some(t) = target {
            s.sel = Selection::caret(t);
            s.page_hint = idx;
        }
        s.ui_requests.push(json!({"scrollTo": "caret"}));
        return sel_result(s);
    }
    if let Some(name) = p::str(v, "bookmark") {
        // Word's built-in bookmark: the top of the document.
        if name == "_top" {
            s.sel = Selection::caret(Pos::body(0, 0));
            return sel_result(s);
        }
        // As in Word, the bookmark's text is selected (a caret for an empty bookmark).
        let (from, to) = super::insert::bookmark_range(&s.doc, name).ok_or_else(|| CmdError::Failed(format!("no bookmark `{name}`")))?;
        s.sel = Selection { anchor: from, focus: to };
        return sel_result(s);
    }
    if let Some(n) = p::u64(v, "paragraph") {
        let paths = s.doc.para_paths(StoryRef::Body);
        if let Some(path) = paths.get((n.max(1) - 1) as usize) {
            s.sel = Selection::caret(Pos { story: StoryRef::Body, path: path.clone(), off: 0 });
        }
        return sel_result(s);
    }
    s.ui_requests.push(json!({"open": "goto"}));
    sel_result(s)
}

fn painter_pick(s: &mut Session) {
    let f = s.sel.ordered().0;
    // Formatting only: never the source's link or its tracked insertion, deletion or change, and
    // not a symbol's own font.
    let chr = s.doc.para_at(&f).map(|p| p.props_of_char(f.off).formatting().without_symbol()).unwrap_or_default();
    let para = s.doc.para_at(&f).map(|p| p.props.formatting()).unwrap_or_default();
    s.painter = Some((chr, para, false));
}

fn painter(s: &mut Session, v: &Value) -> CmdResult {
    if s.painter.is_some() {
        s.painter = None;
        return Ok(json!({"active": false}));
    }
    painter_pick(s);
    if let Some((_, _, sticky)) = s.painter.as_mut() {
        *sticky = p::bool(v, "sticky").unwrap_or(false);
    }
    Ok(json!({"active": true}))
}

/// The painted paragraph formatting, keeping the target paragraph's own tracked changes.
fn keep_revisions(target: &wordcraft_doc::props::ParaProps, painted: &wordcraft_doc::props::ParaProps) -> wordcraft_doc::props::ParaProps {
    wordcraft_doc::props::ParaProps { num_change: target.num_change.clone(), fmt_change: target.fmt_change.clone(), ..painted.clone() }
}

/// Apply the format painter's formatting to the selection.
fn paste_format(s: &mut Session, _: &Value) -> CmdResult {
    let Some((chr, para, sticky)) = s.painter.clone() else { return Err(CmdError::Failed("no formatting copied".into())) };
    let (a, b) = s.sel.ordered();
    if a == b {
        // No selection: apply paragraph formatting (and word formatting like Word).
        let f = s.sel.focus.clone();
        if let Some(pp) = s.doc.para_at(&f) {
            let (x, y) = pp.word_at(f.off);
            s.doc.format_range(&Pos { off: x, ..f.clone() }, &Pos { off: y, ..f.clone() }, &|c| *c = c.cleared().overlaid(&chr))?;
        }
        s.doc.format_paragraphs(&a, &b, &|p| *p = keep_revisions(p, &para))?;
    } else {
        // The target keeps its own link and tracked changes (`cleared` keeps them).
        s.doc.format_range(&a, &b, &|c| *c = c.cleared().overlaid(&chr))?;
        if a.path != b.path || a.off == 0 {
            s.doc.format_paragraphs(&a, &b, &|p| *p = keep_revisions(p, &para))?;
        }
    }
    if !sticky {
        s.painter = None;
    }
    sel_result(s)
}
