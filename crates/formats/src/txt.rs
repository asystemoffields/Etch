//! Plain text: UTF-8 (with or without BOM), UTF-16 LE/BE (BOM or sniffed), else the legacy
//! encoding the bytes look like (GBK/GB18030, Big5, Shift_JIS, EUC-KR, Windows code pages…;
//! #778), Windows-1252 when nothing fits better.
//!
//! A file read here keeps its form: the encoding, byte order mark and line ends it was read
//! with are recorded on the document ([`TextFormat`], under [`TEXT_FORMAT_KEY`] in
//! `Document::passthrough`, which only plain text reads) and written again when it is saved as
//! text, so saving a file someone else made doesn't rewrite every line of it. A document that
//! wasn't read from text is written as UTF-8 with CRLF line ends. List paragraphs get their label
//! and a tab, table cells are separated by tabs.

use std::sync::Arc;

use wordcraft_doc::{CharProps, Document, Paragraph, para_block};

pub use wordcraft_doc::encoding::cp1252;

use crate::model::{FBlock, ListCounter, from_doc};

fn utf16(b: &[u8], le: bool) -> String {
    let units: Vec<u16> = b.as_chunks::<2>().0.iter().map(|c| if le { u16::from_le_bytes(*c) } else { u16::from_be_bytes(*c) }).collect();
    String::from_utf16_lossy(&units)
}

/// UTF-16 without a BOM, told by its zero bytes: `Some(true)` little-endian, `Some(false)`
/// big-endian.
fn unmarked_utf16(b: &[u8]) -> Option<bool> {
    if b.len() < 4 || !b.len().is_multiple_of(2) {
        return None;
    }
    let sample = b.get(..b.len().min(512))?;
    let even_zero = sample.iter().step_by(2).filter(|x| **x == 0).count();
    let odd_zero = sample.iter().skip(1).step_by(2).filter(|x| **x == 0).count();
    let half = sample.len() / 2;
    if odd_zero * 10 >= half * 4 && even_zero * 10 < half {
        return Some(true);
    }
    if even_zero * 10 >= half * 4 && odd_zero * 10 < half {
        return Some(false);
    }
    None
}

/// Does `b` look like plain text, so a file with an extension WordCraft doesn't know (a
/// playlist, a log, a config file…) can open as text the way Word's text import opens it?
///
/// Text has a BOM, or reads as UTF-16 by its zero bytes, or has no NUL byte and next to no
/// other control characters in its first 64 KB. PDFs are refused by their signature (they
/// start as text). An empty file is text.
pub fn is_text(b: &[u8]) -> bool {
    if b.starts_with(&[0xEF, 0xBB, 0xBF]) || b.starts_with(&[0xFF, 0xFE]) || b.starts_with(&[0xFE, 0xFF]) {
        return true;
    }
    if b.starts_with(b"%PDF-") {
        return false;
    }
    let sample = b.get(..b.len().min(64 << 10)).unwrap_or(b);
    // Tab, line feed, form feed, carriage return, DOS end-of-file (^Z) and escape appear in text;
    // other controls in more than 1 of 100 characters mean binary.
    let odd = |c: u32| c == 0 || (c < 0x20 && !matches!(c, 0x09 | 0x0A | 0x0C | 0x0D | 0x1A | 0x1B)) || c == 0x7F;
    if let Some(le) = unmarked_utf16(sample) {
        let units = sample.as_chunks::<2>().0;
        let n = units.iter().filter(|c| odd(u32::from(if le { u16::from_le_bytes(**c) } else { u16::from_be_bytes(**c) }))).count();
        return n.saturating_mul(100) <= units.len();
    }
    if sample.contains(&0) {
        return false;
    }
    sample.iter().filter(|c| odd(u32::from(**c))).count().saturating_mul(100) <= sample.len()
}

/// The `Document::passthrough` key holding the [`TextFormat`] a plain-text file was read with.
pub const TEXT_FORMAT_KEY: &str = "wordcraft:textFormat";

/// The character encoding of a plain-text file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Encoding {
    #[default]
    Utf8,
    Utf16Le,
    Utf16Be,
    Windows1252,
    /// Another legacy encoding the bytes looked like (#778): GBK, Big5, Shift_JIS, EUC-KR, a
    /// Windows or ISO code page…
    Legacy(&'static encoding_rs::Encoding),
}

/// The line ends of a plain-text file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Eol {
    #[default]
    Crlf,
    Lf,
    Cr,
}

impl Eol {
    fn as_str(self) -> &'static str {
        match self {
            Eol::Crlf => "\r\n",
            Eol::Lf => "\n",
            Eol::Cr => "\r",
        }
    }
}

/// How a plain-text file was written: what a save as text writes again.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextFormat {
    pub encoding: Encoding,
    /// The file starts with a byte order mark.
    pub bom: bool,
    /// The line end most of its lines have (a file with mixed line ends is written with this one).
    pub eol: Eol,
    /// The file had vertical tabs (U+000B, Word's line break in text): line breaks are written
    /// as vertical tabs again rather than as line ends.
    pub vt_breaks: bool,
}

impl TextFormat {
    /// The format `doc` was read with, if it was read from plain text.
    pub fn of(doc: &Document) -> Option<TextFormat> {
        doc.passthrough.get(TEXT_FORMAT_KEY).and_then(|b| std::str::from_utf8(b).ok()).and_then(TextFormat::parse)
    }

    /// Record this format on `doc`.
    pub fn record(self, doc: &mut Document) {
        doc.passthrough.insert(TEXT_FORMAT_KEY.into(), Arc::new(self.to_string().into_bytes()));
    }

    fn parse(s: &str) -> Option<TextFormat> {
        let mut f = TextFormat::default();
        for item in s.split(';') {
            let (k, v) = item.split_once('=')?;
            match k {
                "encoding" => {
                    f.encoding = match v {
                        "utf-8" => Encoding::Utf8,
                        "utf-16le" => Encoding::Utf16Le,
                        "utf-16be" => Encoding::Utf16Be,
                        "windows-1252" => Encoding::Windows1252,
                        other => Encoding::Legacy(encoding_rs::Encoding::for_label(other.as_bytes())?),
                    }
                }
                "bom" => f.bom = v == "1",
                "eol" => {
                    f.eol = match v {
                        "crlf" => Eol::Crlf,
                        "lf" => Eol::Lf,
                        "cr" => Eol::Cr,
                        _ => return None,
                    }
                }
                "vt" => f.vt_breaks = v == "1",
                _ => {}
            }
        }
        Some(f)
    }

    /// `text` in this encoding, with the byte order mark when the file had one. Text that the
    /// encoding can't hold (a letter Windows-1252 has no byte for) is written as UTF-8 with a
    /// byte order mark rather than losing those letters.
    pub fn encode(&self, text: &str) -> Vec<u8> {
        let mut out = Vec::with_capacity(text.len() + 3);
        match self.encoding {
            Encoding::Utf8 => {
                if self.bom {
                    out.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
                }
                out.extend_from_slice(text.as_bytes());
            }
            Encoding::Utf16Le | Encoding::Utf16Be => {
                let le = self.encoding == Encoding::Utf16Le;
                if self.bom {
                    out.extend_from_slice(if le { &[0xFF, 0xFE] } else { &[0xFE, 0xFF] });
                }
                for u in text.encode_utf16() {
                    out.extend_from_slice(&if le { u.to_le_bytes() } else { u.to_be_bytes() });
                }
            }
            Encoding::Windows1252 => match text.chars().map(cp1252_byte).collect::<Option<Vec<u8>>>() {
                Some(bytes) => out = bytes,
                None => {
                    out.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
                    out.extend_from_slice(text.as_bytes());
                }
            },
            Encoding::Legacy(enc) => {
                let (bytes, _, lost) = enc.encode(text);
                if lost || enc.output_encoding() != enc {
                    out.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
                    out.extend_from_slice(text.as_bytes());
                } else {
                    out = bytes.into_owned();
                }
            }
        }
        out
    }
}

impl std::fmt::Display for TextFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let encoding = match self.encoding {
            Encoding::Utf8 => "utf-8",
            Encoding::Utf16Le => "utf-16le",
            Encoding::Utf16Be => "utf-16be",
            Encoding::Windows1252 => "windows-1252",
            // WHATWG names have no `;` or `=`.
            Encoding::Legacy(enc) => enc.name(),
        };
        let eol = match self.eol {
            Eol::Crlf => "crlf",
            Eol::Lf => "lf",
            Eol::Cr => "cr",
        };
        write!(f, "encoding={encoding};bom={};eol={eol};vt={}", u8::from(self.bom), u8::from(self.vt_breaks))
    }
}

/// The Windows-1252 byte for `c`, if it has one ([`cp1252`] read backwards: the five bytes
/// Windows-1252 leaves unassigned read as the C1 controls of the same number, and write back so).
fn cp1252_byte(c: char) -> Option<u8> {
    let u = u32::from(c);
    if u < 0x80 || (0xA0..=0xFF).contains(&u) {
        return u8::try_from(u).ok();
    }
    (0x80..0xA0u8).find(|b| cp1252(*b) == c)
}

/// Should bytes that aren't all valid UTF-8 still be read as UTF-8? Yes when the valid
/// multi-byte sequences (é, —, “…”) at least match the invalid ones: a UTF-8 file with a stray
/// byte or two (a truncated copy, one pasted Windows-1252 letter) keeps its letters, the bad
/// bytes becoming U+FFFD. Windows-1252 text almost never forms valid UTF-8 sequences (each
/// accented letter is a lone high byte), so it still reads as Windows-1252.
fn mostly_utf8(b: &[u8]) -> bool {
    let mut valid = 0usize;
    let mut invalid = 0usize;
    for chunk in b.utf8_chunks() {
        valid = valid.saturating_add(chunk.valid().chars().filter(|c| c.len_utf8() > 1).count());
        if !chunk.invalid().is_empty() {
            invalid = invalid.saturating_add(1);
        }
    }
    invalid == 0 || (valid > 0 && valid >= invalid)
}

/// Decode text bytes and say how they were encoded (and whether they had a BOM): BOMs first,
/// then UTF-16 without BOM (by its zero bytes), UTF-8 (allowing a few invalid bytes,
/// [`mostly_utf8`]), and the legacy encoding the bytes look like ([`legacy`]) as the last resort.
pub fn decode_detect(b: &[u8]) -> (String, Encoding, bool) {
    if let Some(rest) = b.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return (String::from_utf8_lossy(rest).into_owned(), Encoding::Utf8, true);
    }
    if let Some(rest) = b.strip_prefix(&[0xFF, 0xFE]) {
        return (utf16(rest, true), Encoding::Utf16Le, true);
    }
    if let Some(rest) = b.strip_prefix(&[0xFE, 0xFF]) {
        return (utf16(rest, false), Encoding::Utf16Be, true);
    }
    if let Some(le) = unmarked_utf16(b) {
        return (utf16(b, le), if le { Encoding::Utf16Le } else { Encoding::Utf16Be }, false);
    }
    if mostly_utf8(b) {
        return (String::from_utf8_lossy(b).into_owned(), Encoding::Utf8, false);
    }
    let (text, encoding) = legacy_detect(b);
    (text, encoding, false)
}

/// Decode text bytes: BOMs first, then UTF-16 without BOM (by its zero bytes), UTF-8, and the
/// legacy encoding the bytes look like ([`legacy`]) as the last resort ([`decode_detect`]).
pub fn decode(b: &[u8]) -> String {
    decode_detect(b).0
}

/// The line end most of `text`'s lines have; CRLF when it has none, or on a tie.
fn dominant_eol(text: &str) -> Eol {
    let crlf = text.matches("\r\n").count();
    let lf = text.matches('\n').count().saturating_sub(crlf);
    let cr = text.matches('\r').count().saturating_sub(crlf);
    if lf > crlf && lf >= cr {
        Eol::Lf
    } else if cr > crlf && cr > lf {
        Eol::Cr
    } else {
        Eol::Crlf
    }
}

/// Most bytes looked at to guess a legacy encoding.
const SNIFF: usize = 1 << 20;

/// Bytes that aren't UTF-8 in the legacy encoding they look like (chardetng, the detector
/// Firefox uses for unlabelled pages): GBK/GB18030 for Simplified Chinese, Big5, Shift_JIS or
/// EUC-JP, EUC-KR, or a single-byte Windows / ISO code page; Windows-1252 as before when the
/// guess is a single-byte Western one.
pub fn legacy(b: &[u8]) -> String {
    legacy_detect(b).0
}

/// [`legacy`], and the encoding it read the bytes in.
fn legacy_detect(b: &[u8]) -> (String, Encoding) {
    let mut det = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Deny);
    det.feed(b.get(..SNIFF).unwrap_or(b), b.len() <= SNIFF);
    let enc = det.guess(None, chardetng::Utf8Detection::Deny);
    if enc == encoding_rs::WINDOWS_1252 {
        return (b.iter().map(|c| cp1252(*c)).collect(), Encoding::Windows1252);
    }
    (enc.decode_without_bom_handling(b).0.into_owned(), Encoding::Legacy(enc))
}

/// Decode bytes in a named encoding (a WHATWG label such as `gbk`, `gb18030`, `big5`,
/// `shift_jis`, `euc-kr`, `utf-16le`, `windows-1251`); `None` when the label isn't known. HTML
/// import uses it for a page's declared `charset` (#2180).
pub fn decode_as(b: &[u8], label: &str) -> Option<String> {
    let enc = encoding_rs::Encoding::for_label(label.trim().as_bytes())?;
    Some(enc.decode(b).0.into_owned())
}

/// Is `c` kept in a paragraph read from text? Tabs, page breaks (form feed) and NUL are; so is
/// every printable character and the C1 controls Windows-1252's unassigned bytes read as (they
/// write back to the same bytes). Other C0 controls (and U+000E, the model's column break) and
/// DEL are dropped.
fn kept(c: char) -> bool {
    !matches!(c, '\u{0001}'..='\u{0008}' | '\u{000D}'..='\u{001F}' | '\u{007F}')
}

/// A document with one paragraph per line; a vertical tab (U+000B, how Word writes a line break
/// in text) is a line break within the paragraph. The file's encoding, BOM and line ends are
/// recorded on the document ([`TextFormat`]) for saving it back the same way. A file of more
/// lines than [`wordcraft_doc::MAX_PARAGRAPHS`] is refused before any paragraph is made: each
/// line costs about 1 KB however short it is.
pub fn import(bytes: &[u8]) -> Result<Document, String> {
    import_budgeted(bytes, wordcraft_doc::MAX_PARAGRAPHS)
}

/// [`import`] allowing at most `max_lines` lines (tests use a small budget).
pub(crate) fn import_budgeted(bytes: &[u8], max_lines: usize) -> Result<Document, String> {
    let (text, encoding, bom) = decode_detect(bytes);
    let eol = dominant_eol(&text);
    let vt_breaks = text.contains('\u{000B}');
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let lines = text.bytes().filter(|&b| b == b'\n').count().saturating_add(1);
    if lines > max_lines {
        return Err(format!("the text has {lines} lines, more than the {max_lines} paragraphs a document can have"));
    }
    let mut doc = Document::new();
    doc.body = text
        .split('\n')
        .map(|line| {
            let clean: String =
                line.chars().map(|c| if c == '\u{000B}' { wordcraft_doc::para::LINE_BREAK } else { c }).filter(|c| *c == '\n' || kept(*c)).collect();
            para_block(Paragraph::with_text(&clean, CharProps::default()))
        })
        .collect();
    doc.ensure_nonempty();
    TextFormat { encoding, bom, eol, vt_breaks }.record(&mut doc);
    Ok(doc)
}

/// [`import`] onto `page`: plain text gives no page size (#1381).
pub fn import_on(bytes: &[u8], page: &wordcraft_doc::SectionProps) -> Result<Document, String> {
    let mut doc = import(bytes)?;
    doc.last_section = page.clone();
    Ok(doc)
}

/// How lines are separated in the text written.
struct Seps<'a> {
    eol: &'a str,
    line_break: &'a str,
    /// A page break is written as a form feed between its lines (documents read from text,
    /// whose form feeds became page breaks); otherwise it is a plain line end.
    form_feed: bool,
}

/// The document's lines of text, each with whether a page break comes before it.
fn blocks_text(blocks: &[FBlock], out: &mut Vec<(String, bool)>, lists: &mut ListCounter, depth: usize, seps: &Seps) {
    for b in blocks {
        match b {
            FBlock::Para(p) => {
                let mut line = String::new();
                match p.list {
                    Some(li) => {
                        line.push_str(&"\t".repeat(li.level as usize));
                        line.push_str(&lists.label(li));
                        line.push('\t');
                    }
                    None => lists.reset(),
                }
                line.push_str(&p.shown_text().replace('\n', seps.line_break).replace(['\u{000C}', '\u{000E}'], ""));
                out.push((line, p.page_break));
            }
            FBlock::Table(t) => {
                lists.reset();
                for row in &t.rows {
                    let cells: Vec<String> = row
                        .iter()
                        .map(|c| {
                            if depth > 8 {
                                return String::new();
                            }
                            let mut v = Vec::new();
                            blocks_text(&c.blocks, &mut v, &mut ListCounter::default(), depth + 1, seps);
                            v.into_iter().map(|(l, _)| l).collect::<Vec<_>>().join(" ")
                        })
                        .collect();
                    out.push((cells.join("\t"), false));
                }
            }
        }
    }
}

/// The document as text, in the form it was read with ([`TextFormat`]): its encoding, BOM and
/// line ends; a document not read from text is UTF-8 with CRLF line ends.
pub fn export(doc: &Document) -> Vec<u8> {
    export_with(doc, false)
}

/// [`export`], ending with the document's comments when `comments` (Print Markup).
pub fn export_with(doc: &Document, comments: bool) -> Vec<u8> {
    let format = TextFormat::of(doc);
    let f = format.unwrap_or_default();
    let eol = f.eol.as_str();
    let seps = Seps { eol, line_break: if f.vt_breaks { "\u{000B}" } else { eol }, form_feed: format.is_some() };
    let flow = from_doc(doc);
    let mut lines = Vec::new();
    blocks_text(&flow.blocks, &mut lines, &mut ListCounter::default(), 0, &seps);
    let mut text = String::new();
    for (i, (line, page_break)) in lines.iter().enumerate() {
        if i > 0 {
            text.push_str(if *page_break && seps.form_feed { "\u{000C}" } else { seps.eol });
        }
        text.push_str(line);
    }
    if comments {
        let notes = crate::comments::append("txt", doc, Vec::new());
        text.push_str(&String::from_utf8_lossy(&notes).replace("\r\n", eol));
    }
    f.encode(&text)
}
