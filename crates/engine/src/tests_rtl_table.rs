//! A synthetic look-alike of a long right-to-left AutoFit table (an Arabic report with
//! one table of about 1,800 rows by 17 columns, set in Microsoft Sans Serif and Times New Roman),
//! built as WordprocessingML from a fixed seed. No real document is used.
//!
//! `WC_PERF_SAVE=dir cargo test --release -p wordcraft-engine --lib rtl_lookalike_pages -- --ignored --nocapture`
//! prints the page count of each variant and writes them to `dir` for the CLI.

use std::fmt::Write as _;

const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// Arabic words for the cells (ordinary vocabulary, our own lists).
const WORDS: &[&str] = &[
    "التقرير",
    "السنوي",
    "الميزانية",
    "المحافظة",
    "المشروع",
    "الإجمالي",
    "عدد",
    "المدارس",
    "الطلاب",
    "المعلمين",
    "نسبة",
    "الإنجاز",
    "المرحلة",
    "الأولى",
    "الثانية",
    "قيد",
    "التنفيذ",
    "مكتمل",
    "ملاحظات",
    "التمويل",
    "الحكومي",
    "الخاص",
    "الشمالية",
    "الجنوبية",
    "الوسطى",
    "إدارة",
    "الصحة",
    "التعليم",
    "الزراعة",
    "المياه",
    "الطرق",
    "الكهرباء",
    "مركز",
    "مدينة",
    "قرية",
    "منطقة",
];

/// Latin words of about the same length, for [`Shape::latin`].
const LATIN: &[&str] = &[
    "report", "annual", "budget", "province", "project", "total", "number", "schools", "students", "teachers", "rate", "progress", "stage", "first",
    "second", "pending", "works", "complete", "notes", "funding", "public", "private", "northern", "southern", "central", "office", "health",
    "learning", "farming", "water", "roads", "power", "centre", "city", "village", "region",
];

/// A small xorshift generator: the same seed gives the same document.
struct Rng(u64, &'static [&'static str]);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
    fn words(&mut self, n: usize) -> String {
        let words = self.1;
        (0..n).map(|_| words[self.below(words.len() as u64) as usize]).collect::<Vec<_>>().join(" ")
    }
}

/// What a variant of the look-alike changes.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Shape {
    pub rows: usize,
    pub cols: usize,
    /// `w:tblLayout w:type="fixed"`: the stored grid is kept as it is.
    pub fixed: bool,
    /// Latin words in place of the Arabic ones (the same fonts): separates font fallback for
    /// Arabic from the rest.
    pub latin: bool,
    pub seed: u64,
}

impl Default for Shape {
    fn default() -> Self {
        Shape { rows: 1800, cols: 17, fixed: false, latin: false, seed: 18 }
    }
}

const SERIF: &str = "Times New Roman";
const SANS: &str = "Microsoft Sans Serif";

fn run(out: &mut String, text: &str, font: &str, bold: bool) {
    let b = if bold { "<w:b/><w:bCs/>" } else { "" };
    let _ = write!(
        out,
        r#"<w:r><w:rPr><w:rFonts w:ascii="{font}" w:hAnsi="{font}" w:cs="{font}"/>{b}<w:sz w:val="20"/><w:szCs w:val="20"/><w:rtl/></w:rPr><w:t xml:space="preserve">{text}</w:t></w:r>"#
    );
}

fn cell(out: &mut String, width: u32, text: &str, font: &str, bold: bool) {
    let _ = write!(out, r#"<w:tc><w:tcPr><w:tcW w:w="{width}" w:type="dxa"/></w:tcPr><w:p><w:pPr><w:bidi/><w:jc w:val="center"/></w:pPr>"#);
    if !text.is_empty() {
        run(out, text, font, bold);
    }
    out.push_str("</w:p></w:tc>");
}

/// The text of one body cell: a serial number, a name, numbers, short words, and now and then a
/// long note.
fn cell_text(rng: &mut Rng, row: usize, col: usize, cols: usize) -> String {
    match col {
        0 => (row + 1).to_string(),
        1 => {
            let n = 2 + rng.below(3) as usize;
            rng.words(n)
        }
        c if c + 1 == cols => match rng.below(10) {
            0 => {
                let n = 8 + rng.below(12) as usize;
                rng.words(n)
            }
            1..=4 => {
                let n = 1 + rng.below(3) as usize;
                rng.words(n)
            }
            _ => String::new(),
        },
        c if c % 4 == 3 => {
            let n = 1 + rng.below(2) as usize;
            rng.words(n)
        }
        c if c % 4 == 2 => format!("{}.{}%", rng.below(100), rng.below(10)),
        _ => rng.below(100_000).to_string(),
    }
}

/// The look-alike's `word/document.xml`.
pub(crate) fn document_xml(shape: Shape) -> String {
    let mut rng = Rng(shape.seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1, if shape.latin { LATIN } else { WORDS });
    let cols = shape.cols.max(2);
    // A4 portrait, 2.54 cm margins: 9,026 twips of text width. The stored grid is narrower than
    // the content asks for: equal columns over the text width, as a table inserted at its
    // default width and then filled.
    let col_w = 9026 / cols as u32;
    let mut out = String::with_capacity(shape.rows * cols * 300);
    let layout = if shape.fixed { r#"<w:tblLayout w:type="fixed"/>"# } else { "" };
    let _ = write!(out, r#"<w:p><w:pPr><w:bidi/><w:jc w:val="center"/></w:pPr>"#);
    run(&mut out, &rng.words(5), SERIF, true);
    out.push_str("</w:p>");
    let _ = write!(
        out,
        r#"<w:tbl><w:tblPr><w:bidiVisual/><w:tblStyle w:val="TableGrid"/><w:tblW w:w="0" w:type="auto"/>{layout}<w:tblLook w:val="04A0" w:firstRow="1" w:lastRow="0" w:firstColumn="1" w:lastColumn="0" w:noHBand="0" w:noVBand="1"/></w:tblPr><w:tblGrid>"#
    );
    for _ in 0..cols {
        let _ = write!(out, r#"<w:gridCol w:w="{col_w}"/>"#);
    }
    out.push_str("</w:tblGrid>");
    // The header row, repeated on every page.
    out.push_str(r#"<w:tr><w:trPr><w:tblHeader/></w:trPr>"#);
    for _ in 0..cols {
        let text = {
            let n = 1 + rng.below(3) as usize;
            rng.words(n)
        };
        cell(&mut out, col_w, &text, SERIF, true);
    }
    out.push_str("</w:tr>");
    for row in 0..shape.rows {
        out.push_str("<w:tr>");
        for c in 0..cols {
            let text = cell_text(&mut rng, row, c, cols);
            let font = if c % 2 == 0 { SANS } else { SERIF };
            cell(&mut out, col_w, &text, font, false);
        }
        out.push_str("</w:tr>");
    }
    out.push_str("</w:tbl><w:p><w:pPr><w:bidi/></w:pPr></w:p>");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="{W}" xmlns:r="{R}"><w:body>{out}<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="708" w:footer="708" w:gutter="0"/><w:bidi/></w:sectPr></w:body></w:document>"#
    )
}

/// Styles as Word 2010 writes them for an Arabic document: Normal with 10 pt after and 1.15
/// lines, and the Table Grid style, which takes both back to nothing and single.
const STYLES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:asciiTheme="minorHAnsi" w:eastAsiaTheme="minorHAnsi" w:hAnsiTheme="minorHAnsi" w:cstheme="minorBidi"/><w:sz w:val="22"/><w:szCs w:val="22"/><w:lang w:val="en-US" w:eastAsia="en-US" w:bidi="ar-SA"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="200" w:line="276" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:qFormat/><w:pPr><w:bidi/></w:pPr></w:style><w:style w:type="table" w:default="1" w:styleId="TableNormal"><w:name w:val="Normal Table"/><w:uiPriority w:val="99"/><w:semiHidden/><w:unhideWhenUsed/><w:tblPr><w:tblInd w:w="0" w:type="dxa"/><w:tblCellMar><w:top w:w="0" w:type="dxa"/><w:left w:w="108" w:type="dxa"/><w:bottom w:w="0" w:type="dxa"/><w:right w:w="108" w:type="dxa"/></w:tblCellMar></w:tblPr></w:style><w:style w:type="table" w:styleId="TableGrid"><w:name w:val="Table Grid"/><w:basedOn w:val="TableNormal"/><w:uiPriority w:val="59"/><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:tblPr><w:tblBorders><w:top w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:left w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:bottom w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:right w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:insideH w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:insideV w:val="single" w:sz="4" w:space="0" w:color="auto"/></w:tblBorders></w:tblPr></w:style></w:styles>"#;

/// The look-alike as a `.docx` package.
pub(crate) fn lookalike_docx(shape: Shape) -> Vec<u8> {
    use std::io::Write;
    let ct = r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/></Types>"#;
    let rels = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{R}/officeDocument" Target="word/document.xml"/></Relationships>"#
    );
    let doc_rels = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{R}/styles" Target="styles.xml"/></Relationships>"#
    );
    let document = document_xml(shape);
    let mut zw = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default();
    for (name, body) in [
        ("[Content_Types].xml", ct),
        ("_rels/.rels", rels.as_str()),
        ("word/_rels/document.xml.rels", doc_rels.as_str()),
        ("word/styles.xml", STYLES),
        ("word/document.xml", document.as_str()),
    ] {
        zw.start_file(name, opts).unwrap();
        zw.write_all(body.as_bytes()).unwrap();
    }
    zw.finish().unwrap().into_inner()
}

/// The page count, and on the second page: the body rows, line heights (least, most, mean) and
/// lines per cell paragraph.
fn measure(bytes: &[u8]) -> String {
    let doc = crate::io::open_bytes("lookalike.docx", bytes).unwrap();
    let mut s = crate::Session::new(doc);
    let l = s.layout();
    let mut rows = std::collections::BTreeSet::new();
    let (mut heights, mut paras) = (Vec::new(), 0usize);
    if let Some(page) = l.pages.get(1) {
        for it in page.items.iter() {
            if let wordcraft_layout::Placed::Lines { path, para, l0, l1, .. } = it {
                rows.insert(path.0.get(1).copied());
                paras += 1;
                heights.extend(para.lines.get(*l0..*l1).unwrap_or(&[]).iter().map(|x| x.height));
            }
        }
    }
    let n = heights.len().max(1) as f32;
    let lo = heights.iter().copied().fold(f32::MAX, f32::min);
    let hi = heights.iter().copied().fold(0.0, f32::max);
    format!(
        "pages {} | page 2: rows {} line h {lo:.2}..{hi:.2} mean {:.2} lines/para {:.2}",
        l.pages.len(),
        rows.len(),
        heights.iter().sum::<f32>() / n,
        heights.len() as f32 / paras.max(1) as f32
    )
}

/// Prints the look-alike's page count with Etch's AutoFit and with the stored grid kept.
#[test]
#[ignore = "slow: lays out about 1,800 table rows twice; run by hand for its page counts"]
fn rtl_lookalike_pages() {
    let d = Shape::default();
    for (name, shape) in [
        ("autofit", d),
        ("fixed", Shape { fixed: true, ..d }),
        ("latin-autofit", Shape { latin: true, ..d }),
        ("latin-fixed", Shape { latin: true, fixed: true, ..d }),
    ] {
        let bytes = lookalike_docx(shape);
        if let Ok(dir) = std::env::var("WC_PERF_SAVE") {
            let _ = std::fs::write(std::path::Path::new(&dir).join(format!("rtl-{name}.docx")), &bytes);
        }
        println!("LOOKALIKE {name}: {}", measure(&bytes));
    }
}

/// Prints, for the first rows of a small look-alike, each row's height (from where the next row
/// starts) against its tallest cell's lines, and whether the header row repeats on page 2.
#[test]
#[ignore = "diagnostic; run by hand"]
fn rtl_lookalike_rows() {
    for latin in [false, true] {
        let bytes = lookalike_docx(Shape { rows: 300, latin, ..Shape::default() });
        let doc = crate::io::open_bytes("lookalike.docx", &bytes).unwrap();
        let mut s = crate::Session::new(doc);
        let l = s.layout();
        // Row → (top, tallest cell's text height, its line count).
        let mut rows: std::collections::BTreeMap<u32, (f32, f32, usize)> = std::collections::BTreeMap::new();
        for it in l.pages[0].items.iter() {
            if let wordcraft_layout::Placed::Lines { path, para, l0, l1, y, .. } = it
                && let Some(&r) = path.0.get(1)
            {
                let h: f32 = para.lines.get(*l0..*l1).unwrap_or(&[]).iter().map(|x| x.height).sum();
                let e = rows.entry(r).or_insert((*y, 0.0, 0));
                e.0 = e.0.min(*y);
                if h > e.1 {
                    (e.1, e.2) = (h, l1 - l0);
                }
            }
        }
        let tops: Vec<(u32, (f32, f32, usize))> = rows.into_iter().take(8).collect();
        for w in tops.windows(2) {
            let ((r, (top, text, n)), (_, (next, _, _))) = (w[0], w[1]);
            println!("ROW latin={latin} row {r}: height {:.2} text {text:.2} ({n} lines) extra {:.2}", next - top, next - top - text);
        }
        let header_on_2 = l
            .pages
            .get(1)
            .is_some_and(|p| p.items.iter().any(|it| matches!(it, wordcraft_layout::Placed::Lines { path, .. } if path.0.get(1) == Some(&0))));
        println!("ROW latin={latin} header row on page 2: {header_on_2}");
    }
}
