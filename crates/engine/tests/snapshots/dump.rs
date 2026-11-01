//! Text dumps of what the snapshots pin: a layout (pages, items, lines), a document model, a
//! saved file (its parts, one tag a line, volatile values normalised), a PDF (its objects and
//! content streams) and the fonts a layout used.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io::Read as _;

use sha2::Digest as _;

use wordcraft_doc::{Block, Blocks, Document, Paragraph, Path, StoryRef};
use wordcraft_engine::Selection;
use wordcraft_geom::Rect;
use wordcraft_layout::para::{ClKind, Line, ParaLayout};
use wordcraft_layout::{DocLayout, Page, Placed};

// ---------------------------------------------------------------------------------------------
// Numbers and text

/// A length to a tenth of a point (`-0.0` is `0.0`).
pub fn pt(v: f32) -> String {
    let r = (v * 10.0).round() / 10.0;
    let r = if r == 0.0 { 0.0 } else { r };
    format!("{r:.1}")
}

fn rect(r: &Rect) -> String {
    format!("[{} {} {}x{}]", pt(r.x), pt(r.y), pt(r.w), pt(r.h))
}

fn story(s: StoryRef) -> String {
    match s {
        StoryRef::Body => "body".into(),
        StoryRef::Part(id) => format!("part:{id}"),
    }
}

fn path(p: &Path) -> String {
    let v: Vec<String> = p.0.iter().map(|i| i.to_string()).collect();
    format!("[{}]", v.join(","))
}

fn rgb(c: wordcraft_doc::props::Rgb) -> String {
    format!("#{}", c.hex().to_lowercase())
}

/// Text as a quoted string: control characters escaped, the object placeholder shown as `{obj}`.
pub fn quoted(s: &str) -> String {
    let s = s.replace(wordcraft_doc::para::OBJ, "{obj}");
    format!("{s:?}")
}

pub fn sha256(bytes: &[u8]) -> String {
    let h = sha2::Sha256::digest(bytes);
    h.iter().map(|b| format!("{b:02x}")).collect()
}

// ---------------------------------------------------------------------------------------------
// Layout

/// The text a line draws: its clusters' text, with tabs, breaks and objects (field results,
/// note numbers) spelled out.
fn line_text(p: Option<&Paragraph>, pl: &ParaLayout, l: &Line) -> String {
    let mut s = String::new();
    for ci in l.c0..l.c1 {
        let Some(c) = pl.clusters.get(ci) else { continue };
        if let Ok(k) = pl.shown.binary_search_by_key(&ci, |e| e.0) {
            if let Some(e) = pl.shown.get(k) {
                s.push_str(&e.1);
            }
            continue;
        }
        match c.kind {
            ClKind::Text | ClKind::Space => {
                let t = p.and_then(|p| p.text.get(c.start as usize..c.end as usize)).unwrap_or("?");
                s.push_str(&t.replace(wordcraft_doc::para::OBJ, "{obj}"));
            }
            ClKind::Tab => s.push('\t'),
            ClKind::LineBreak => s.push('⏎'),
            ClKind::PageBreak => s.push('⇟'),
            ClKind::ColumnBreak => s.push('⇥'),
            _ => s.push_str("{obj}"),
        }
    }
    s
}

fn item(out: &mut String, doc: &Document, it: &Placed, indent: &str) {
    match it {
        Placed::Lines { story: st, path: p, para, l0, l1, x, y, turn } => {
            let para_ref = doc.para(*st, p);
            let mut head = format!("{indent}lines {} {} l{l0}..{l1} x={} y={}", story(*st), path(p), pt(*x), pt(*y));
            if *turn != wordcraft_doc::props::TextDirection::Horizontal {
                let _ = write!(head, " turn={turn:?}");
            }
            if let Some(lab) = &para.label
                && *l0 == 0
            {
                let _ = write!(head, " label={} lx={} lw={}", quoted(&lab.text), pt(lab.x), pt(lab.width));
            }
            let _ = writeln!(out, "{head}");
            for (i, l) in para.lines.iter().enumerate().take(*l1).skip(*l0) {
                let mut flags = String::new();
                if l.hyphen.is_some() {
                    flags.push_str(" hyph");
                }
                if l.beside {
                    flags.push_str(" beside");
                }
                if l.rtl {
                    flags.push_str(" rtl");
                }
                let _ = writeln!(
                    out,
                    "{indent}  {i}: top={} h={} base={} left={} right={} end={:?}{flags} {}",
                    pt(l.top),
                    pt(l.height),
                    pt(l.baseline),
                    pt(l.left),
                    pt(l.right),
                    l.end,
                    quoted(&line_text(para_ref, para, l))
                );
            }
        }
        Placed::Fill { rect: r, color } => {
            let _ = writeln!(out, "{indent}fill {} {}", rect(r), rgb(*color));
        }
        Placed::Rule { x0, y0, x1, y1, border } => {
            let _ = writeln!(
                out,
                "{indent}rule ({} {})-({} {}) {:?} w={} {}",
                pt(*x0),
                pt(*y0),
                pt(*x1),
                pt(*y1),
                border.style,
                pt(border.width),
                border.color.map(rgb).unwrap_or_else(|| "auto".into())
            );
        }
        Placed::Image { rect: r, media, crop, story: st, path: p, off, spin } => {
            let _ = writeln!(
                out,
                "{indent}image {} media={media} crop=[{} {} {} {}] at {} {}#{off} spin={}",
                rect(r),
                pt(crop[0]),
                pt(crop[1]),
                pt(crop[2]),
                pt(crop[3]),
                story(*st),
                path(p),
                spin_text(spin)
            );
        }
        Placed::Shape { rect: r, kind, fill, stroke, stroke_width, freeform, spin, .. } => {
            let _ = writeln!(
                out,
                "{indent}shape {kind:?} {} fill={} stroke={} sw={} spin={}{}",
                rect(r),
                fill.map(rgb).unwrap_or_else(|| "none".into()),
                stroke.map(rgb).unwrap_or_else(|| "none".into()),
                pt(*stroke_width),
                spin_text(spin),
                if freeform.is_some() { " freeform" } else { "" }
            );
        }
        Placed::Graphic { rect: r, story: st, path: p, off, spin, .. } => {
            let _ = writeln!(out, "{indent}graphic {} at {} {}#{off} spin={}", rect(r), story(*st), path(p), spin_text(spin));
        }
        Placed::Clip { rect: r, spin, items } => {
            let _ = writeln!(out, "{indent}clip {} spin={} {{", rect(r), spin_text(spin));
            let deeper = format!("{indent}  ");
            for it in items {
                item(out, doc, it, &deeper);
            }
            let _ = writeln!(out, "{indent}}}");
        }
        Placed::Cell { rect: r, covered, table, row, cell, story: st, clip } => {
            let _ = writeln!(
                out,
                "{indent}cell {} table={} r{row} c{cell} {}{}{}",
                rect(r),
                path(table),
                story(*st),
                if *covered { " covered" } else { "" },
                if *clip { " clip" } else { "" }
            );
        }
        Placed::Object { rect: r, spin, story: st, path: p, off, text_box, wrap, origin } => {
            let _ = writeln!(
                out,
                "{indent}object {} at {} {}#{off} box={} wrap={wrap:?} origin=({} {}) spin={}",
                rect(r),
                story(*st),
                path(p),
                text_box.map(|b| b.to_string()).unwrap_or_else(|| "-".into()),
                pt(origin.x),
                pt(origin.y),
                spin_text(spin)
            );
        }
    }
}

fn spin_text(s: &wordcraft_geom::Spin) -> String {
    if s.deg == 0.0 && !s.flip_h && !s.flip_v {
        return "0".into();
    }
    format!("{}{}{}", pt(s.deg), if s.flip_h { "h" } else { "" }, if s.flip_v { "v" } else { "" })
}

/// The rows of each table on a page, from its cell areas: `rows [path]: 0:y+h 1:y+h …`.
fn rows_summary(out: &mut String, items: &[Placed]) {
    let mut rows: BTreeMap<(Vec<u32>, usize), (f32, f32)> = BTreeMap::new();
    fn walk(items: &[Placed], rows: &mut BTreeMap<(Vec<u32>, usize), (f32, f32)>) {
        for it in items {
            match it {
                Placed::Cell { rect, table, row, .. } => {
                    let e = rows.entry((table.0.clone(), *row)).or_insert((f32::MAX, f32::MIN));
                    e.0 = e.0.min(rect.y);
                    e.1 = e.1.max(rect.y + rect.h);
                }
                Placed::Clip { items, .. } => walk(items, rows),
                _ => {}
            }
        }
    }
    walk(items, &mut rows);
    let mut by_table: BTreeMap<Vec<u32>, Vec<(usize, f32, f32)>> = BTreeMap::new();
    for ((t, r), (y0, y1)) in rows {
        by_table.entry(t).or_default().push((r, y0, y1 - y0));
    }
    for (t, rs) in by_table {
        let s: Vec<String> = rs.iter().map(|(r, y, h)| format!("{r}:{}+{}", pt(*y), pt(*h))).collect();
        let _ = writeln!(out, "  rows {}: {}", path(&Path(t)), s.join(" "));
    }
}

fn page_text(out: &mut String, doc: &Document, n: usize, p: &Page) {
    let _ = writeln!(
        out,
        "page {}: {}x{} number={} section={} body={} slot={:?} first_block={} markup={} header={} footer={}",
        n + 1,
        pt(p.w),
        pt(p.h),
        p.number,
        p.section,
        rect(&p.body),
        p.hf_slot,
        p.first_block,
        pt(p.markup),
        p.header_story.map(|s| format!("part:{s}")).unwrap_or_else(|| "-".into()),
        p.footer_story.map(|s| format!("part:{s}")).unwrap_or_else(|| "-".into()),
    );
    if !p.header.is_empty() {
        let _ = writeln!(out, "  header ({} items):", p.header.len());
        for it in &p.header {
            item(out, doc, it, "    ");
        }
    }
    let items: &[Placed] = &p.items;
    let _ = writeln!(out, "  items ({}):", items.len());
    for it in items {
        item(out, doc, it, "    ");
    }
    rows_summary(out, items);
    if !p.footer.is_empty() {
        let _ = writeln!(out, "  footer ({} items):", p.footer.len());
        for it in &p.footer {
            item(out, doc, it, "    ");
        }
    }
}

/// The families of every face a layout used (text runs and list labels), sorted.
pub fn fonts_used(lay: &DocLayout) -> BTreeSet<String> {
    fn walk(items: &[Placed], out: &mut BTreeSet<String>) {
        for it in items {
            match it {
                Placed::Lines { para, .. } => {
                    for st in &para.styles {
                        out.insert(st.face.get().family.clone());
                    }
                    if let Some(l) = &para.label {
                        for r in &l.runs {
                            if let Some(st) = para.styles.get(r.style as usize) {
                                out.insert(st.face.get().family.clone());
                            }
                        }
                    }
                }
                Placed::Clip { items, .. } => walk(items, out),
                _ => {}
            }
        }
    }
    let mut out = BTreeSet::new();
    for p in &lay.pages {
        walk(&p.header, &mut out);
        walk(&p.items, &mut out);
        walk(&p.footer, &mut out);
    }
    out
}

/// The layout dump: a header, then every page with its header, items (lines with their text,
/// position and height; fills, rules, cells, images, shapes, objects) and footer.
pub fn layout_text(doc: &Document, lay: &DocLayout, name: &str, variant: &str, fonts: &BTreeSet<String>) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# layout snapshot: {name} ({variant})");
    let fonts: Vec<&str> = fonts.iter().map(String::as_str).collect();
    let _ = writeln!(out, "# fonts: {}", fonts.join(", "));
    let _ = writeln!(out, "# pages: {}  text boxes laid out: {}", lay.pages.len(), lay.text_boxes);
    for (n, p) in lay.pages.iter().enumerate() {
        page_text(&mut out, doc, n, p);
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Model

fn json<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_else(|e| format!("<unserialisable: {e}>"))
}

fn blocks_text(out: &mut String, blocks: &Blocks, indent: &str, depth: usize) {
    for (i, b) in blocks.iter().enumerate() {
        match &**b {
            Block::Para(p) => {
                let _ = writeln!(out, "{indent}[{i}] para props={} mark={} {}", json(&p.props), json(&p.mark), quoted(&p.text));
                let runs: Vec<String> = p.runs.iter().map(|r| format!("{}:{}", r.len, json(&r.props))).collect();
                if !runs.is_empty() {
                    let _ = writeln!(out, "{indent}    runs {}", runs.join(" "));
                }
                for (k, o) in p.objects.iter().enumerate() {
                    let _ = writeln!(out, "{indent}    obj {k} {}", json(o));
                }
                if let Some(s) = &p.section {
                    let _ = writeln!(out, "{indent}    section {}", json(s));
                }
            }
            Block::Table(t) => {
                let grid: Vec<String> = t.grid.iter().map(|g| pt(*g)).collect();
                let _ = writeln!(out, "{indent}[{i}] table props={} grid=[{}]", json(&t.props), grid.join(" "));
                if depth > 8 {
                    let _ = writeln!(out, "{indent}    (too deep)");
                    continue;
                }
                for (r, row) in t.rows.iter().enumerate() {
                    let _ = writeln!(out, "{indent}    row {r} props={}", json(&row.props));
                    for (c, cell) in row.cells.iter().enumerate() {
                        let _ = writeln!(out, "{indent}      cell {c} props={}", json(&cell.props));
                        blocks_text(out, &cell.blocks, &format!("{indent}        "), depth + 1);
                    }
                }
            }
        }
    }
}

fn pos(p: &wordcraft_doc::Pos) -> String {
    format!("{} {}@{}", story(p.story), path(&p.path), p.off)
}

/// The document model as text: selection, properties, settings, the sections, the lists, the
/// styles that differ from the built-in set, every block of every story with its formatting,
/// the comments and revisions (without their dates: commands stamp them from the wall clock).
pub fn model_text(doc: &Document, sel: Option<&Selection>) -> String {
    let mut out = String::new();
    if let Some(sel) = sel {
        let _ = writeln!(out, "sel: {} .. {}", pos(&sel.anchor), pos(&sel.focus));
    }
    let _ = writeln!(out, "core: title={} creator={} subject={}", quoted(&doc.core.title), quoted(&doc.core.creator), quoted(&doc.core.subject));
    let _ = writeln!(out, "settings: {}", json(&doc.settings));
    let _ = writeln!(out, "last_section: {}", json(&doc.last_section));
    let builtin = wordcraft_doc::StyleSheet::builtin();
    let _ =
        writeln!(out, "styles: {} (defaults chr={} para={})", doc.styles.styles.len(), json(&doc.styles.default_chr), json(&doc.styles.default_para));
    for st in &doc.styles.styles {
        let same = builtin.styles.iter().any(|b| b.id == st.id && b == st);
        if !same {
            let _ = writeln!(out, "  style {}", json(st));
        }
    }
    for id in builtin.styles.iter().map(|b| &b.id) {
        if !doc.styles.styles.iter().any(|s| &s.id == id) {
            let _ = writeln!(out, "  style {id} removed");
        }
    }
    let _ = writeln!(out, "numbering: {} lists, {} abstracts", doc.numbering.nums.len(), doc.numbering.abstracts.len());
    for n in &doc.numbering.nums {
        let _ = writeln!(out, "  list {}", json(n));
    }
    for a in &doc.numbering.abstracts {
        let _ = writeln!(out, "  abstract {} name={:?}", a.id, a.name);
        for (i, l) in a.levels.iter().enumerate() {
            let _ = writeln!(out, "    L{i} {}", json(l));
        }
    }
    let _ = writeln!(out, "body ({} blocks):", doc.body.len());
    blocks_text(&mut out, &doc.body, "  ", 0);
    for (id, part) in doc.parts.iter() {
        let _ = writeln!(out, "part {id} {:?} body={} ({} blocks):", part.kind, json(&part.body), part.blocks.len());
        blocks_text(&mut out, &part.blocks, "  ", 0);
    }
    for (id, c) in doc.comments.iter() {
        let _ = writeln!(
            out,
            "comment {id} author={} initials={} parent={:?} resolved={} part={}",
            quoted(&c.author),
            quoted(&c.initials),
            c.parent,
            c.resolved,
            c.part
        );
    }
    for (i, r) in doc.revisions.iter().enumerate() {
        let _ = writeln!(out, "revision {i} {:?} author={} move={:?}", r.kind, quoted(&r.author), r.move_name);
    }
    for (k, m) in doc.media.iter() {
        let _ = writeln!(out, "media {k}: {} bytes sha256={}", m.len(), &sha256(m)[..16]);
    }
    let _ = writeln!(out, "model sha256={}", model_hash(doc));
    out
}

/// A hash of the whole model (its JSON, the wall-clock dates blanked), so what the dump doesn't
/// spell out is pinned too.
pub fn model_hash(doc: &Document) -> String {
    let mut v = serde_json::to_value(doc).unwrap_or(serde_json::Value::Null);
    fn blank(v: &mut serde_json::Value) {
        match v {
            serde_json::Value::Object(m) => {
                for (k, x) in m.iter_mut() {
                    if matches!(k.as_str(), "date" | "dateUtc" | "date_utc" | "created" | "modified") && x.is_string() {
                        *x = serde_json::Value::String("<date>".into());
                    } else {
                        blank(x);
                    }
                }
            }
            serde_json::Value::Array(a) => a.iter_mut().for_each(blank),
            _ => {}
        }
    }
    blank(&mut v);
    sha256(v.to_string().as_bytes())
}

// ---------------------------------------------------------------------------------------------
// Saved files

/// Values a save takes from the clock or the build, blanked so the dumps don't change with
/// them: ISO timestamps, RTF date groups, PDF dates and ids, the crate version.
pub fn normalize(s: &str) -> String {
    use std::sync::OnceLock;
    static RES: OnceLock<Vec<(regex::Regex, &'static str)>> = OnceLock::new();
    let res = RES.get_or_init(|| {
        vec![
            (regex::Regex::new(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:?\d{2})?").unwrap(), "<timestamp>"),
            (regex::Regex::new(r"\\yr\d+\\mo\d+\\dy\d+\\hr\d+\\min\d+(\\sec\d+)?").unwrap(), "\\yr0\\mo0\\dy0\\hr0\\min0"),
            (regex::Regex::new(r"\\revdttm(del)?-?\d+").unwrap(), "\\revdttm${1}0"),
            (regex::Regex::new(r"\\atndate -?\d+").unwrap(), "\\atndate 0"),
            // Formatting revisions' dates (RTF 1.9.1 `\crdate`, `\prdate`), written since the upstream merge.
            (regex::Regex::new(r"\\(cr|pr)date-?\d+").unwrap(), "\\${1}date0"),
            (regex::Regex::new(r"D:\d{14}[^)]*").unwrap(), "D:<timestamp>"),
            (regex::Regex::new(r"/ID \[<[0-9A-Fa-f]+> <[0-9A-Fa-f]+>\]").unwrap(), "/ID [<id> <id>]"),
            (regex::Regex::new(r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}").unwrap(), "<uuid>"),
        ]
    });
    let mut out = s.replace(env!("CARGO_PKG_VERSION"), "<version>");
    for (re, rep) in res {
        out = re.replace_all(&out, *rep).into_owned();
    }
    out
}

/// XML one element a line: a newline between `>` and `<`, so a diff names the element.
pub fn xml_lines(s: &str) -> String {
    s.replace("><", ">\n<")
}

/// The entries of a zip package by name.
pub fn zip_parts(bytes: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let mut out = BTreeMap::new();
    for i in 0..z.len() {
        let mut f = z.by_index(i).map_err(|e| e.to_string())?;
        let mut v = Vec::new();
        f.read_to_end(&mut v).map_err(|e| e.to_string())?;
        out.insert(f.name().to_string(), v);
    }
    Ok(out)
}

/// A part's text for the dump: XML and text parts in full (normalised, one element a line);
/// anything else (a picture) as its size and hash.
pub fn part_text(name: &str, bytes: &[u8]) -> String {
    let textual = name.ends_with(".xml") || name.ends_with(".rels") || name.ends_with(".txt") || name == "mimetype";
    match std::str::from_utf8(bytes) {
        Ok(s) if textual => xml_lines(&normalize(s)),
        _ => format!("<binary {} bytes sha256={}>", bytes.len(), sha256(bytes)),
    }
}

/// A zip package (DOCX, ODT) as text: each part under a `== name` heading. Parts equal to the
/// blank document's (`blank`, by part name) are written as a one-line reference, so goldens
/// hold each boilerplate part once (in the blank document's own golden).
pub fn package_text(bytes: &[u8], blank: Option<&BTreeMap<String, String>>) -> Result<String, String> {
    let parts = zip_parts(bytes)?;
    let mut out = String::new();
    let _ = writeln!(out, "# {} parts", parts.len());
    for (name, data) in &parts {
        let text = part_text(name, data);
        match blank.and_then(|b| b.get(name)) {
            Some(b) if *b == text => {
                let _ = writeln!(
                    out,
                    "== {name}: same as the blank document's ({} lines, sha256={})",
                    text.lines().count(),
                    &sha256(text.as_bytes())[..16]
                );
            }
            _ => {
                let _ = writeln!(out, "== {name} ({} lines)", text.lines().count());
                out.push_str(&text);
                if !text.ends_with('\n') {
                    out.push('\n');
                }
            }
        }
    }
    Ok(out)
}

/// The parts of the blank document's package, by name, as [`part_text`] writes them.
pub fn package_parts_text(bytes: &[u8]) -> Result<BTreeMap<String, String>, String> {
    Ok(zip_parts(bytes)?.iter().map(|(n, d)| (n.clone(), part_text(n, d))).collect())
}

/// A text format (RTF, HTML, Markdown, plain text) as a dump: normalised, under one heading.
pub fn text_file_text(ext: &str, bytes: &[u8]) -> String {
    let s = String::from_utf8_lossy(bytes);
    let mut out = format!("== {ext} ({} bytes)\n", bytes.len());
    let body = normalize(&s);
    let body = if ext == "html" { body.replace("><", ">\n<") } else { body };
    out.push_str(&body);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Page content streams written in full; later pages are summarised (operator counts and a
/// hash), which keeps a long document's golden small while its first pages stay readable.
pub const PDF_PAGES_IN_FULL: usize = 2;

/// Text (UTF-8 with no control characters but tab, newline and return): a content stream, the
/// XMP metadata; a font or a picture is not.
fn is_text(data: &[u8]) -> bool {
    std::str::from_utf8(data).is_ok() && data.iter().all(|b| !matches!(b, 0..=8 | 11 | 12 | 14..=31 | 127))
}

/// What a page content stream draws, counted: text objects, text runs, rectangles, path
/// segments, images and form objects.
fn content_summary(s: &str) -> String {
    let mut counts = [("BT", 0usize), ("Tj", 0), ("TJ", 0), ("re", 0), ("l", 0), ("c", 0), ("Do", 0), ("cm", 0), ("rg", 0)];
    for tok in s.split(|c: char| c.is_whitespace() || c == ']' || c == ')') {
        for (op, n) in counts.iter_mut() {
            if tok == *op {
                *n += 1;
            }
        }
    }
    counts.iter().filter(|(_, n)| *n > 0).map(|(op, n)| format!("{op}={n}")).collect::<Vec<_>>().join(" ")
}

/// A PDF written without stream compression as text: every line of the file as it is, with
/// text streams (the page content: drawing operators and positions; the XMP metadata) in full
/// for the first [`PDF_PAGES_IN_FULL`] pages and summarised after, binary streams (fonts,
/// pictures) as their size and hash; dates and ids blanked.
pub fn pdf_text(bytes: &[u8]) -> String {
    let page_re = regex::Regex::new(r"/Type\s*/Page(?:[^s]|$)").unwrap_or_else(|e| panic!("{e}"));
    let id_re = regex::Regex::new(r"/ID\s*\[\([^)]*\)\s*\([^)]*\)\]").unwrap_or_else(|e| panic!("{e}"));
    let mut out = String::new();
    let mut stream: Option<Vec<u8>> = None;
    let mut pages = 0usize;
    let mut content_streams = 0usize;
    for raw in bytes.split(|b| *b == b'\n') {
        let line = raw.strip_suffix(b"\r").unwrap_or(raw);
        if let Some(buf) = stream.as_mut() {
            if line == b"endstream" {
                let data = buf.clone();
                stream = None;
                if is_text(&data) {
                    let s = String::from_utf8_lossy(&data);
                    // XMP metadata is never page content, whatever its ids happen to spell (`Tj`).
                    let is_content = !s.trim_start().starts_with("<?xpacket")
                        && (s.contains(" Tm") || s.contains(" cm") || s.contains(" re") || s.contains("Tj") || s.contains("TJ"));
                    if is_content {
                        content_streams += 1;
                    }
                    if is_content && content_streams > PDF_PAGES_IN_FULL {
                        let _ = writeln!(out, "  <page content {} bytes sha256={} {}>", data.len(), &sha256(&data)[..32], content_summary(&s));
                    } else {
                        for l in normalize(&s).lines() {
                            let _ = writeln!(out, "  {l}");
                        }
                    }
                } else {
                    let _ = writeln!(out, "  <binary stream {} bytes sha256={}>", data.len(), sha256(&data));
                }
                out.push_str("endstream\n");
            } else {
                if !buf.is_empty() {
                    buf.push(b'\n');
                }
                buf.extend_from_slice(line);
            }
            continue;
        }
        if line == b"stream" {
            out.push_str("stream\n");
            stream = Some(Vec::new());
            continue;
        }
        if is_text(line) {
            let s = String::from_utf8_lossy(line);
            if page_re.is_match(&s) {
                pages += 1;
            }
            let s = id_re.replace_all(&normalize(&s), "/ID[<id><id>]").into_owned();
            out.push_str(&s);
            out.push('\n');
        } else {
            let _ = writeln!(out, "<binary line {} bytes sha256={}>", line.len(), &sha256(line)[..16]);
        }
    }
    format!("# pdf: {} bytes, {pages} page object(s), {content_streams} content stream(s)\n{out}", bytes.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalisation_blanks_dates_and_ids() {
        let s = "a 2026-10-18T12:34:56Z b 2026-10-18T12:34:56.123+02:00 c \\yr2026\\mo10\\dy18\\hr12\\min34 d \\revdttm123 \\revdttmdel45 \
                 e (D:20261018123456+00'00') f /ID [<ABCDEF> <123456>] g 123e4567-e89b-12d3-a456-426614174000";
        let n = normalize(s);
        assert!(!n.contains("2026"), "{n}");
        assert!(n.contains("\\revdttm0 \\revdttmdel0"), "{n}");
        assert_eq!(normalize("{\\*\\atndate 132814279}"), "{\\*\\atndate 0}");
        assert_eq!(normalize("\\crauth1\\crdate669685872 \\prdate-12"), "\\crauth1\\crdate0 \\prdate0");
        assert!(n.contains("/ID [<id> <id>]") && n.contains("<uuid>") && n.contains("D:<timestamp>"), "{n}");
        // A plain date without a time (a DATE field's result) is content, not a stamp.
        assert_eq!(normalize("on 2026-10-18 we"), "on 2026-10-18 we");
    }

    #[test]
    fn pdf_dump_keeps_text_streams_and_hashes_binary_ones() {
        let pdf = b"%PDF-1.7\n1 0 obj\n<< /Length 9 >>\nstream\nBT ET q Q\nendstream\nendobj\n2 0 obj\nstream\n\x00\x01\x02\xff\nendstream\nendobj\n<< /Type /Page >>\n";
        let t = pdf_text(pdf);
        assert!(t.contains("  BT ET q Q\n"), "{t}");
        assert!(t.contains("<binary stream 4 bytes sha256="), "{t}");
        assert!(t.starts_with("# pdf: "), "{t}");
        assert!(t.contains("1 page object(s)"), "{t}");
    }

    #[test]
    fn lengths_round_to_a_tenth_without_negative_zero() {
        assert_eq!(pt(-0.01), "0.0");
        assert_eq!(pt(12.345), "12.3");
        assert_eq!(pt(72.0), "72.0");
    }
}
