//! Which formats keep the whole document, and what a save in another format would lose.
//!
//! Only formats whose round trip is proven by tests count as keeping everything: Word's packages
//! (`.docx`/`.docm`/`.dotx`/`.dotm`, `crates/docx/tests/roundtrip.rs`), Word XML (`.xml`, Flat
//! OPC, written by the same writer and tested equal to `.docx`: `flat_opc_round_trip_equals_docx`)
//! and WordCraft's own JSON, except that only `.docm`, `.dotm` and the JSON hold a macro project
//! ([`drops_macros`]: a save in any other format names it as `macros`). Every other format (ODT, RTF, HTML, Markdown, LaTeX, plain text, and
//! the PDF and PNG copies) goes through a small flow model of paragraphs, lists and tables. ODT
//! and RTF also carry notes, comments, headers and footers, tracked changes, text boxes, fields
//! and sections through it; the other text formats don't. What each format still loses is
//! measured by the fidelity harness (`crates/engine/tests/format_fidelity.rs`), which checks
//! [`lossy_features`] against it. A save in one of them is a compatibility save (Word's "Keep
//! current format"), never an AutoSave.

use wordcraft_doc::props::VertAlign;
use wordcraft_doc::{Block, Blocks, Document, InlineObject, PartKind};

/// Does saving in the format of `name` (by its extension) keep the whole document?
pub fn keeps_everything(name: &str) -> bool {
    let ext = super::ext_of(name);
    super::is_word_package(&ext) || matches!(ext.as_str(), "json" | "xml")
}

/// Does a save of `doc` as `name` leave out its macro project? Only macro-enabled Word packages
/// (`.docm`, `.dotm`) and WordCraft's JSON keep one; a `.docx`/`.dotx`, Word XML and every other
/// format are written without it (Word warns before saving a macro-free document too). Cheap: one
/// lookup, so AutoSave can ask every frame.
pub fn drops_macros(name: &str, doc: &Document) -> bool {
    wordcraft_docx::has_macro_project(doc) && !matches!(super::ext_of(name).as_str(), "docm" | "dotm" | "json")
}

/// Can WordCraft write the format of `name` at all? (It opens Word 97-2003 `.doc`/`.dot` but
/// doesn't write them.)
pub fn can_save(name: &str) -> bool {
    let ext = super::ext_of(name);
    super::SAVE_EXTS.contains(&ext.as_str()) || matches!(ext.as_str(), "text" | "htm" | "xhtml" | "markdown" | "latex" | "ltx")
}

/// What a save of `doc` as `name` would lose: `None` when the format keeps everything
/// ([`keeps_everything`]) and the document has no macro project it would leave out; otherwise
/// the features `doc` has that the format can't hold, as camelCase ids (`comments`, `footnotes`,
/// `endnotes`, `headersFooters`, `trackedChanges`, `textBoxes`, `equations`, `contentControls`,
/// `drawings`, `nestedTables`, `mergedCells`, `pictures`, `fields`, `nestedFields`, `sections`,
/// `pageSetup`, `properties`, `formatting`, `macros`), which may be empty when the document uses
/// none of them. A `.docx`, `.dotx` or Word XML file keeps everything but a macro project
/// ([`drops_macros`]): `Some(["macros"])` for a document that has one.
///
/// `formatting` is character, paragraph and table formatting the format has no way to write
/// (Markdown and plain text: underline, superscript and subscript, colours, fonts and sizes,
/// alignment, right-to-left paragraphs, spacing and indents, tabs, page breaks, column widths
/// and cell shading; every text format: absolute position tabs, hidden separators and smart
/// tags), as measured by the fidelity harness.
pub fn lossy_features(name: &str, doc: &Document) -> Option<Vec<&'static str>> {
    let macros = drops_macros(name, doc);
    if keeps_everything(name) {
        return macros.then(|| vec!["macros"]);
    }
    let ext = super::ext_of(name);
    let found = Found::of(doc);
    // ODT and RTF keep notes, comments, headers and footers, tracked changes and text boxes,
    // but not comment threads (a reply is a comment of its own), moves (an insertion and a
    // deletion) or, in RTF, a note's own mark. HTML keeps footnotes and endnotes (as Word's web
    // pages do); Markdown keeps them as footnotes, without a note's own mark.
    let rich = matches!(ext.as_str(), "odt" | "rtf");
    let web = matches!(ext.as_str(), "html" | "htm" | "xhtml");
    let markdown = matches!(ext.as_str(), "md" | "markdown");
    let mut lost = Vec::new();
    let mut add = |id: &'static str, present: bool| {
        if present {
            lost.push(id);
        }
    };
    let replies = doc.comments.values().any(|c| c.parent.is_some());
    add("comments", if rich { replies } else { found.comments || !doc.comments.is_empty() });
    add(
        "footnotes",
        if rich || web {
            found.custom_marks && ext == "rtf"
        } else if markdown {
            found.custom_marks
        } else {
            found.footnotes
        },
    );
    add("endnotes", found.endnotes && !rich && !web);
    add("headersFooters", found.headers_footers && !rich);
    let moves = doc.revisions.iter().any(|r| r.move_name.is_some());
    // Nor tracked table rows and cells (#1998): only the text in them stays tracked.
    add("trackedChanges", if rich { found.tracked && (moves || found.table_revisions) } else { found.tracked });
    add("textBoxes", found.text_boxes && !rich);
    // LaTeX writes equations as math and HTML as MathML; the other formats as their linear text.
    add("equations", found.equations && !matches!(ext.as_str(), "tex" | "latex" | "ltx" | "html" | "htm" | "xhtml"));
    add("contentControls", found.controls);
    add("drawings", found.drawings);
    // Markdown and plain text flatten a table inside a table into its cell; the other text
    // formats one nested deeper than their flow keeps.
    add(
        "nestedTables",
        found.nested_tables && matches!(ext.as_str(), "md" | "markdown" | "txt" | "text") || found.deep_tables && ext != "pdf" && ext != "png",
    );
    // Markdown links pictures it can't embed.
    let plain = matches!(ext.as_str(), "txt" | "text" | "md" | "markdown");
    add("pictures", found.pictures && plain);
    // Markdown's tables can't merge cells.
    add("mergedCells", found.merged_cells && plain);
    // The text formats keep a field's last result as text, not the field; ODT and RTF keep
    // the simple fields their vocabularies have, and write the rest as their results.
    let kept: &[&str] = match ext.as_str() {
        "odt" => wordcraft_formats::odt::KEPT_FIELDS,
        "rtf" => wordcraft_formats::rtf::SIMPLE_FIELDS,
        _ => &[],
    };
    add("fields", found.fields.iter().any(|kind| !kept.iter().any(|k| k.eq_ignore_ascii_case(kind))));
    // A field nested in another's code is kept only by Word documents and WordCraft's JSON:
    // ODT and RTF write the outer code with the inner field's result in its place (#1671).
    add("nestedFields", found.nested_fields);
    // HTML, Markdown and plain text have no pages: sections with page setups of their own are
    // lost; HTML keeps one page setup (`@page`), the others none.
    add("sections", doc.sections().len() > 1 && !rich);
    add("pageSetup", found.page_setup && plain);
    // Markdown and plain text have nowhere for the document's properties.
    add("properties", found.properties && plain);
    // Formatting the format has no way to write.
    let formatting = found.ptab || found.spec_vanish || found.smart_tag || (plain && found.plain_formatting);
    add("formatting", formatting && !matches!(ext.as_str(), "pdf" | "png"));
    add("macros", macros);
    Some(lost)
}

/// [`lossy_features`] of a save written with `opts`. An `.xml` file written as a Word 2003 XML
/// Document (`opts.word2003`: `format: "wordml2003"`, or a save over such a file) keeps less than
/// a Word XML Document (Flat OPC): the 2003 vocabulary has no drawings other than pictures, text
/// boxes, content controls or equations, comment threads, moves, formatting changes or a note's
/// own mark, and its writer leaves them out (`wordml2003_write`), and the even pages' headers.
/// Measured by the fidelity harness's `wordml2003` column.
pub fn lossy_features_with(name: &str, doc: &Document, opts: &super::ExportOptions) -> Option<Vec<&'static str>> {
    if !(opts.word2003 && super::ext_of(name) == "xml") {
        return lossy_features(name, doc);
    }
    let found = Found::of(doc);
    let replies = doc.comments.values().any(|c| c.parent.is_some());
    // Moves become an insertion and a deletion, formatting changes aren't tracked; tracked table
    // rows and cells are named too (not measured by the harness, so named to be safe).
    let tracked = doc.revisions.iter().any(|r| r.move_name.is_some() || r.kind == wordcraft_doc::RevisionKind::Format) || found.table_revisions;
    // Its pictures are those of the formats `w:binData` holds; others (WebP, SVG…) are left out.
    let odd_pictures = doc.media.keys().any(|k| {
        let ext = k.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
        !matches!(ext.as_str(), "png" | "jpeg" | "jpg" | "gif" | "bmp" | "emf" | "wmf" | "tif" | "tiff")
    });
    let mut lost = Vec::new();
    for (id, present) in [
        ("comments", replies),
        ("footnotes", found.custom_marks),
        // Different odd and even pages: the even pages' headers and footers.
        ("headersFooters", found.headers_footers && doc.settings.even_odd_headers),
        ("trackedChanges", found.tracked && tracked),
        ("textBoxes", found.text_boxes),
        ("equations", found.equations),
        ("contentControls", found.controls),
        ("drawings", found.drawings),
        ("pictures", found.pictures && odd_pictures),
        // Absolute position tabs and smart tags have no 2003 form.
        ("formatting", found.ptab || found.smart_tag),
        // `w:macrosPresent="no"`: no macro project is written.
        ("macros", drops_macros(name, doc)),
    ] {
        if present {
            lost.push(id);
        }
    }
    Some(lost)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Session;
    use serde_json::json;

    /// Only formats proven lossless by round-trip tests keep everything; ODT and RTF (which drop
    /// content controls, drawings, equations and comment threads) don't, nor do the text formats.
    #[test]
    fn only_word_documents_and_wordcraft_json_keep_everything() {
        for name in ["a.docx", "A.DOCX", "a.docm", "a.dotx", "a.dotm", "a.json", "a.wcraft.json", "a.xml", "/dir.odt/a.docx"] {
            assert!(keeps_everything(name), "{name}");
        }
        for name in ["a.odt", "a.rtf", "a.html", "a.htm", "a.md", "a.txt", "a.tex", "a.pdf", "a.png", "a.doc", "a", "/dir.docx/a.odt"] {
            assert!(!keeps_everything(name), "{name}");
        }
        assert!(can_save("a.odt") && can_save("a.txt") && can_save("a.docx") && can_save("a.text"));
        assert!(!can_save("a.doc") && !can_save("a.dot") && !can_save("a"));
    }

    /// What a lossy save would lose is reported by feature, only for what the document has.
    #[test]
    fn lossy_features_name_what_the_document_would_lose() {
        let mut s = Session::new(Document::from_text("Synthetic body"));
        assert_eq!(lossy_features("a.docx", &s.doc), None);
        assert_eq!(lossy_features("a.odt", &s.doc), Some(vec![]), "lossy, but nothing here to lose");
        s.run("references.footnote", &json!({"text": "Synthetic footnote"})).unwrap();
        s.run("references.endnote", &json!({"text": "Synthetic endnote"})).unwrap();
        s.run("review.newComment", &json!({"text": "Synthetic comment"})).unwrap();
        s.run("insert.header", &json!({"text": "Synthetic header"})).unwrap();
        s.run("insert.equation", &json!({"linear": "E=mc^2"})).unwrap();
        s.run("developer.plainText", &json!({"tag": "synthetic"})).unwrap();
        s.run("insert.textBox", &json!({"text": "Synthetic box"})).unwrap();
        s.run("review.trackChanges", &json!({"value": true})).unwrap();
        s.run("text.insert", &json!({"text": "tracked"})).unwrap();
        let odt = lossy_features("letter.odt", &s.doc).unwrap();
        for f in ["equations", "contentControls"] {
            assert!(odt.contains(&f), "odt: {f} missing from {odt:?}");
        }
        assert!(!odt.contains(&"nestedTables"));
        // ODT and RTF keep notes, comments, headers, tracked changes and text boxes.
        let rtf = lossy_features("letter.rtf", &s.doc).unwrap();
        for f in ["comments", "footnotes", "endnotes", "headersFooters", "trackedChanges", "textBoxes"] {
            assert!(!rtf.contains(&f), "rtf: {f} in {rtf:?}");
        }
        assert!(rtf.contains(&"equations") && rtf.contains(&"contentControls"), "{rtf:?}");
        // HTML keeps notes (DPUB-ARIA note markup); Markdown keeps footnotes, and endnotes as
        // footnotes.
        let html = lossy_features("letter.html", &s.doc).unwrap();
        for f in ["comments", "headersFooters", "trackedChanges", "textBoxes", "contentControls"] {
            assert!(html.contains(&f), "html: {f} missing from {html:?}");
        }
        assert!(!html.contains(&"footnotes") && !html.contains(&"endnotes"), "html keeps notes: {html:?}");
        let md = lossy_features("letter.md", &s.doc).unwrap();
        assert!(!md.contains(&"footnotes") && md.contains(&"endnotes"), "markdown keeps footnotes only: {md:?}");
        s.run("references.footnote", &json!({"text": "Starred", "mark": "*"})).unwrap();
        assert!(lossy_features("letter.md", &s.doc).unwrap().contains(&"footnotes"), "a note's own mark is lost in Markdown");
        assert!(!lossy_features("letter.html", &s.doc).unwrap().contains(&"footnotes"), "HTML keeps a note's own mark");
        // LaTeX keeps equations as math.
        assert!(!lossy_features("letter.tex", &s.doc).unwrap().contains(&"equations"));
        // ODT and RTF don't keep tracked table rows (#1998).
        s.run("insert.table", &json!({"rows": 1, "cols": 1})).unwrap();
        for name in ["letter.odt", "letter.rtf"] {
            assert!(lossy_features(name, &s.doc).unwrap().contains(&"trackedChanges"), "{name}");
        }
        assert_eq!(lossy_features("letter.docx", &s.doc), None);
    }

    /// Markdown and text flatten a table nested in a table; ODT, RTF and HTML keep it.
    #[test]
    fn nested_tables_are_reported_for_the_formats_that_flatten_them() {
        let mut s = Session::new(Document::from_text("Synthetic"));
        s.run("insert.table", &json!({"rows": 2, "cols": 2})).unwrap();
        assert!(!lossy_features("a.rtf", &s.doc).unwrap().contains(&"nestedTables"));
        s.run("insert.table", &json!({"rows": 1, "cols": 1})).unwrap();
        for (name, lost) in [("a.rtf", false), ("a.md", true), ("a.txt", true), ("a.odt", false), ("a.html", false)] {
            assert_eq!(lossy_features(name, &s.doc).unwrap().contains(&"nestedTables"), lost, "{name}");
        }
    }
}

/// The features of a document a lossy format can drop.
#[derive(Default)]
struct Found {
    comments: bool,
    footnotes: bool,
    endnotes: bool,
    headers_footers: bool,
    tracked: bool,
    /// A table row or cell that is a tracked insertion or deletion.
    table_revisions: bool,
    text_boxes: bool,
    equations: bool,
    controls: bool,
    drawings: bool,
    nested_tables: bool,
    /// Tables nested deeper than the text formats' flow keeps ([`wordcraft_formats::model::MAX_DEPTH`]):
    /// their writers flatten the deepest into paragraphs.
    deep_tables: bool,
    pictures: bool,
    /// A footnote or endnote with a mark of its own.
    custom_marks: bool,
    /// A field with fields nested in its code (`IF { REF amount } …`).
    nested_fields: bool,
    /// The kinds of field in the text (the first word of each field's code, upper case).
    fields: Vec<String>,
    /// A cell merged across columns or rows.
    merged_cells: bool,
    /// A page size, orientation or margins other than the default.
    page_setup: bool,
    /// A title, author, subject, keywords or description.
    properties: bool,
    /// Formatting Markdown and plain text can't write (see [`lossy_features`]).
    plain_formatting: bool,
    /// Formatting no text format writes: absolute position tabs, hidden separators (which
    /// Word 2003 XML keeps), smart tags.
    ptab: bool,
    spec_vanish: bool,
    smart_tag: bool,
}

/// Deepest table nesting looked into (hostile documents can nest deeper).
const MAX_DEPTH: usize = 32;

impl Found {
    fn of(doc: &Document) -> Found {
        let mut f = Found::default();
        for part in doc.parts.values() {
            match part.kind {
                PartKind::Header | PartKind::Footer => f.headers_footers |= !part.blocks.is_empty(),
                PartKind::Footnote => f.footnotes = true,
                PartKind::Endnote => f.endnotes = true,
                PartKind::Comment => f.comments = true,
                PartKind::TextBox => f.text_boxes = true,
            }
            f.blocks(&part.blocks, 0);
        }
        f.blocks(&doc.body, 0);
        let (d, s) = (wordcraft_doc::SectionProps::default(), &doc.last_section);
        f.page_setup = [(s.page_w, d.page_w), (s.page_h, d.page_h), (s.margin_left, d.margin_left), (s.margin_right, d.margin_right)]
            .iter()
            .chain([(s.margin_top, d.margin_top), (s.margin_bottom, d.margin_bottom)].iter())
            .any(|(a, b)| (a - b).abs() > 0.5)
            || s.landscape != d.landscape;
        let c = &doc.core;
        f.properties = [&c.title, &c.creator, &c.subject, &c.keywords, &c.description].iter().any(|v| !v.trim().is_empty());
        // A text box is kept as one, not as a drawing.
        f.drawings = false;
        f.drawings_in(doc, &doc.body, 0);
        for part in doc.parts.values() {
            f.drawings_in(doc, &part.blocks, 0);
        }
        f
    }

    /// Drawings other than text boxes in `blocks`.
    fn drawings_in(&mut self, doc: &Document, blocks: &Blocks, depth: usize) {
        if depth > MAX_DEPTH || self.drawings {
            return;
        }
        let text_box = |id: &Option<u32>| id.and_then(|i| doc.parts.get(&i)).is_some_and(|p| p.kind == PartKind::TextBox);
        for b in blocks {
            match &**b {
                Block::Para(p) => {
                    self.drawings |= p.objects.iter().any(|o| match o {
                        InlineObject::Shape { story, .. } => !text_box(story),
                        InlineObject::Graphic { .. } | InlineObject::Group { .. } => true,
                        _ => false,
                    });
                }
                Block::Table(t) => {
                    for c in t.rows.iter().flat_map(|r| r.cells.iter()) {
                        self.drawings_in(doc, &c.blocks, depth + 1);
                    }
                }
            }
        }
    }

    fn blocks(&mut self, blocks: &Blocks, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        for b in blocks {
            match &**b {
                Block::Para(p) => {
                    self.tracked |=
                        p.mark.ins.is_some() || p.mark.del.is_some() || p.runs.iter().any(|r| r.props.ins.is_some() || r.props.del.is_some());
                    let pp = &p.props;
                    self.plain_formatting |= pp.align.is_some_and(|a| a != wordcraft_doc::Align::Left)
                        || pp.bidi == Some(true)
                        || pp.page_break_before == Some(true)
                        || pp.space_before.is_some()
                        || pp.space_after.is_some()
                        || pp.line_spacing.is_some()
                        || pp.indent_first.is_some()
                        || pp.indent_right.is_some()
                        // List paragraphs are indented by their list; the text formats keep that.
                        || (pp.indent_left.is_some() && pp.numbering.is_none())
                        || p.text.contains(['\t', wordcraft_doc::para::PAGE_BREAK]);
                    for r in &p.runs {
                        let c = &r.props;
                        self.plain_formatting |= c.underline.is_some_and(|u| u != wordcraft_doc::props::Underline::None)
                            || c.vert_align.is_some_and(|v| v != VertAlign::Baseline)
                            || c.color.is_some()
                            || c.highlight.is_some()
                            || c.shading.is_some()
                            || c.font.is_some()
                            || c.size.is_some();
                        self.ptab |= c.ptab.is_some();
                        self.spec_vanish |= c.spec_vanish == Some(true);
                        self.smart_tag |= c.wrap.is_some();
                    }
                    for o in &p.objects {
                        match o {
                            InlineObject::Equation { .. } => self.equations = true,
                            InlineObject::ControlStart { .. } => self.controls = true,
                            InlineObject::CommentStart { .. } => self.comments = true,
                            InlineObject::Graphic { .. } | InlineObject::Shape { .. } | InlineObject::Group { .. } => self.drawings = true,
                            InlineObject::Image { .. } => self.pictures = true,
                            InlineObject::NoteRef { custom, .. } => self.custom_marks |= !custom.is_empty(),
                            InlineObject::Field { instr, nested, .. } | InlineObject::FieldStart { instr, nested, .. } => {
                                let kind = instr.split_whitespace().next().unwrap_or("").to_ascii_uppercase();
                                if !self.fields.contains(&kind) && self.fields.len() < 64 {
                                    self.fields.push(kind);
                                }
                                self.nested_fields |= !nested.is_empty();
                            }
                            _ => {}
                        }
                    }
                }
                Block::Table(t) => {
                    if depth > 0 {
                        self.nested_tables = true;
                    }
                    if depth >= wordcraft_formats::model::MAX_DEPTH {
                        self.deep_tables = true;
                    }
                    self.controls |= !t.controls.open.is_empty();
                    // Columns of different widths (Markdown's pipes have none).
                    let uneven = t.grid.windows(2).any(|w| w.first().zip(w.get(1)).is_some_and(|(a, b)| (a - b).abs() > 0.5));
                    self.plain_formatting |= uneven;
                    for row in &t.rows {
                        self.controls |= !row.controls.open.is_empty();
                        self.table_revisions |= row.props.ins.is_some() || row.props.del.is_some();
                        for cell in &row.cells {
                            self.controls |= !cell.controls.open.is_empty();
                            self.table_revisions |= cell.props.ins.is_some() || cell.props.del.is_some();
                            self.tracked |= self.table_revisions;
                            self.merged_cells |= cell.props.span > 1 || cell.props.vmerge != wordcraft_doc::props::VMerge::None;
                            self.plain_formatting |= cell.props.shading.is_some();
                            self.blocks(&cell.blocks, depth + 1);
                        }
                    }
                }
            }
        }
    }
}
