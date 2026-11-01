//! File format dispatch by extension. Format crates plug in here.

use wordcraft_doc::Document;

#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) mod fault;
mod fidelity;
#[cfg(not(target_arch = "wasm32"))]
mod file_version;
#[cfg(not(target_arch = "wasm32"))]
mod safe_save;
pub use fidelity::{can_save, drops_macros, keeps_everything, lossy_features, lossy_features_with};
#[cfg(not(target_arch = "wasm32"))]
pub use file_version::FileVersion;

/// A refused native save. The caller retains edits and can choose a separate destination.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}: file changed or is unavailable; save a copy to keep your edits", path.display())]
pub struct FileConflict {
    pub path: std::path::PathBuf,
}

/// Formats WordCraft opens.
pub const OPEN_EXTS: &[&str] = &[
    "docx",
    "docm",
    "dotx",
    "dotm",
    "doc",
    "dot",
    "txt",
    "md",
    "markdown",
    "html",
    "htm",
    "rtf",
    "odt",
    "fodt",
    "tex",
    "latex",
    "ltx",
    "xml",
    "wcraft.json",
    "json",
];
/// Formats WordCraft saves (Save As).
pub const SAVE_EXTS: &[&str] = &["docx", "docm", "dotx", "dotm", "pdf", "txt", "md", "html", "rtf", "odt", "tex", "png", "json", "xml"];

/// Magic number for ZIP package signatures (used by .docx/.docm/.dotx/.dotm).
const ZIP_SIG: &[u8] = b"PK\x03\x04";
/// Magic number for OLE compound document signatures (used by .doc/.dot).
const OLE_SIG: &[u8] = &[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

/// The extension of a file name or path, lower-case; empty when the file name has none (a dot
/// in a folder name doesn't count).
fn ext_of(name: &str) -> String {
    let file = name.rsplit(['/', '\\']).next().unwrap_or(name);
    file.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase()).unwrap_or_default()
}

/// Is `ext` one of Word's package formats (.docx/.docm/.dotx/.dotm), which save without loss?
pub fn is_word_package(ext: &str) -> bool {
    wordcraft_docx::Flavor::from_ext(ext).is_some()
}

/// How a copy in another format is written.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExportOptions {
    /// Print Markup: PDFs show tracked changes marked up and, with `comments`, each page scaled
    /// down beside a markup area with the comments in balloons (page images get the area too);
    /// HTML, Markdown and text copies end with the comments.
    pub markup: bool,
    /// With `markup`, show the comments (Track Changes Options › Comments).
    pub comments: bool,
    /// Print hidden text (File › Options › Display): PDFs, prints and page images include text
    /// formatted as hidden; off, as in Word, they leave it out whatever the screen shows.
    pub hidden_text: bool,
    /// An `.xml` copy is a Word 2003 XML Document (WordprocessingML 2003) instead of a Word XML
    /// Document (Flat OPC): `format: "wordml2003"` on the save commands.
    pub word2003: bool,
}

/// A document password: wiped from memory when dropped, never printed (`Debug` shows `***`).
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Password(zeroize::Zeroizing<String>);

impl Password {
    pub fn new(s: &str) -> Password {
        Password(zeroize::Zeroizing::new(s.to_string()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// The text, for a password box to edit in place.
    pub fn as_mut_string(&mut self) -> &mut String {
        &mut self.0
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Debug for Password {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Password(***)")
    }
}

/// Does this open error mean the file is password-protected and no password was given?
pub fn needs_password(err: &str) -> bool {
    err.contains(wordcraft_docx::PASSWORD_REQUIRED)
}

/// Does this open error mean the password given was wrong?
pub fn wrong_password(err: &str) -> bool {
    err.contains(wordcraft_docx::WRONG_PASSWORD)
}

/// Is the file at `path` empty, zero bytes long? File Explorer's New › Microsoft Word Document
/// makes one, and `file.open` opens it as a new blank document (#1384). False for a file that
/// can't be read: opening it says why.
#[cfg(not(target_arch = "wasm32"))]
pub fn is_empty_file(path: &std::path::Path) -> bool {
    crate::file_access::check_read(&path.to_string_lossy()).is_ok() && std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() == 0)
}

/// [`is_empty_file`]: on the web files come through the browser, never by path.
#[cfg(target_arch = "wasm32")]
pub fn is_empty_file(_path: &std::path::Path) -> bool {
    false
}

/// Can files with this extension be saved with a password (Word's packages)?
pub fn can_encrypt(name: &str) -> bool {
    is_word_package(&ext_of(name))
}

/// Finish a document just read from a file: never leave it empty, and bring its unlocked
/// `DATE`/`TIME` fields up to now the way Word does on open (see
/// [`crate::cmd::references::refresh_date_fields`]).
fn opened(doc: &mut Document) {
    doc.ensure_nonempty();
    crate::cmd::references::refresh_date_fields(doc, &crate::clock::Clock::default());
}

/// Parse a document from bytes; `name` gives the format by extension.
pub fn open_bytes(name: &str, bytes: &[u8]) -> Result<Document, String> {
    open_bytes_with(name, bytes, None).map(|(doc, _)| doc)
}

/// Open a Word 97-2003 binary file, decrypting it with `password` when it is password-protected
/// (XOR, RC4, RC4 CryptoAPI). The errors for a missing or wrong password are the same texts the
/// `.docx` reader gives, so [`needs_password`] and [`wrong_password`] recognise them.
fn open_binary_doc(bytes: &[u8], password: Option<&str>) -> Result<(Document, bool), String> {
    use wordcraft_docbin::DocbinError;
    let encrypted = wordcraft_docbin::is_encrypted(bytes);
    match wordcraft_docbin::read_with_password(bytes, password) {
        Ok(doc) => Ok((doc, encrypted)),
        Err(DocbinError::PasswordRequired) => Err(wordcraft_docx::PASSWORD_REQUIRED.to_string()),
        Err(DocbinError::WrongPassword) => Err(wordcraft_docx::WRONG_PASSWORD.to_string()),
        Err(e) => Err(e.to_string()),
    }
}

/// [`open_bytes`] for a file that may be password-protected: a Word document encrypted with a
/// password ([MS-OFFCRYPTO] agile or Standard encryption, under any Word extension, or the
/// Word 97-2003 schemes) is decrypted with
/// `password` first. Also says
/// whether the file was encrypted. Without the password needed the error satisfies
/// [`needs_password`]; with a wrong one, [`wrong_password`].
pub fn open_bytes_with(name: &str, bytes: &[u8], password: Option<&str>) -> Result<(Document, bool), String> {
    open_bytes_in(name, bytes, password, &wordcraft_geom::Region::default())
}

/// The page a file that gives no page size (plain text, Markdown, HTML) opens on: `region`'s
/// paper, as a new blank document (#838, #1381), with the default margins.
pub fn region_page(region: &wordcraft_geom::Region) -> wordcraft_doc::SectionProps {
    let (_, w, h) = region.paper();
    wordcraft_doc::SectionProps { page_w: w, page_h: h, ..Default::default() }
}

/// [`open_bytes_with`] in `region`: a file that gives no page size opens on the region's paper
/// (see [`region_page`]); a document with its own page size keeps it.
pub fn open_bytes_in(name: &str, bytes: &[u8], password: Option<&str>, region: &wordcraft_geom::Region) -> Result<(Document, bool), String> {
    let page = region_page(region);
    let (doc, encrypted) = read_bytes(name, bytes, password, &page)?;
    within_limits(name, &doc)?;
    load_embedded_fonts(&doc);
    Ok((doc, encrypted))
}

/// A document past one of the limits a document is held to (`wordcraft_doc::limits`) is refused,
/// naming what is over, whatever format it came in: the editor couldn't keep it within them, and
/// a file saved from it might not open again. Each reader refuses what it can before building
/// the document (a file of a million empty paragraphs never becomes one); this catches the rest
/// (WordCraft's JSON, a saved size past the budget, more runs than a document may have).
pub fn within_limits(name: &str, doc: &Document) -> Result<(), String> {
    within_limits_of(name, doc, &wordcraft_doc::limits::Limits::DOCUMENT)
}

/// [`within_limits`] against `limits` (tests scale them down).
pub(crate) fn within_limits_of(name: &str, doc: &Document, limits: &wordcraft_doc::limits::Limits) -> Result<(), String> {
    if let Some(over) = wordcraft_doc::limits::Tally::of(doc).over(limits) {
        return Err(format!("{name}: limit exceeded: {over}"));
    }
    match wordcraft_doc::limits::tables_over(doc) {
        Some(what) => Err(format!("{name}: limit exceeded: {what}")),
        None => Ok(()),
    }
}

/// Keys `key` in JSON text `bytes` (a quoted name followed by a colon), counted without parsing:
/// at least as many as the objects that have it (a string holding exactly the name followed by a
/// colon would count too, which only makes the count larger).
fn json_keys(bytes: &[u8], key: &str) -> usize {
    let pat = format!("\"{key}\"");
    let pat = pat.as_bytes();
    let mut n = 0usize;
    let mut i = 0usize;
    while let Some(at) = bytes.get(i..).and_then(|rest| rest.windows(pat.len()).position(|w| w == pat)) {
        let start = i + at;
        let end = start + pat.len();
        let escaped = start > 0 && bytes.get(start - 1) == Some(&b'\\');
        let colon = bytes.get(end..).and_then(|rest| rest.iter().find(|b| !b.is_ascii_whitespace())) == Some(&b':');
        if !escaped && colon {
            n += 1;
        }
        i = end;
    }
    n
}

/// What [`json_within_limits`] lets past the limits: the keys it counts also name what the
/// document's count leaves out (the note separators' paragraphs, a picture named `len`), so a
/// document at the limits opens; the count after parsing is exact.
pub(crate) const JSON_KEY_SLACK: usize = 1024;

/// WordCraft's JSON with more paragraphs or runs than a document may have is refused before it is
/// parsed (each costs some hundreds of bytes in memory however little JSON it takes).
pub(crate) fn json_within_limits(name: &str, bytes: &[u8]) -> Result<(), String> {
    use wordcraft_doc::limits::{Measure, PARAGRAPHS, RUNS};
    for (key, limit, measure) in [("runs", PARAGRAPHS, Measure::Paragraphs), ("len", RUNS, Measure::Runs)] {
        if json_keys(bytes, key) > limit.saturating_add(JSON_KEY_SLACK) {
            return Err(format!("{name}: limit exceeded: more than {limit} {}", measure.what()));
        }
    }
    Ok(())
}

/// Load the fonts `doc` embeds (#1171) for this run, in memory only: text set in a font that isn't
/// installed then lays out and exports with it instead of a fallback. Faces already available
/// stay as they are.
fn load_embedded_fonts(doc: &Document) {
    let files: Vec<(Vec<u8>, String)> = wordcraft_docx::embedded_fonts(doc)
        .into_iter()
        .filter_map(|f| wordcraft_fonts::decode_font_file(&f.data).ok().map(|data| (data, f.name)))
        .collect();
    if !files.is_empty() {
        wordcraft_fonts::FontDb::global().add_document_fonts(files);
    }
}

fn read_bytes(name: &str, bytes: &[u8], password: Option<&str>, page: &wordcraft_doc::SectionProps) -> Result<(Document, bool), String> {
    let ext = ext_of(name);
    if matches!(ext.as_str(), "docx" | "docm" | "dotx" | "dotm" | "doc" | "dot") && wordcraft_docx::is_encrypted(bytes) {
        let mut doc = wordcraft_docx::read_with_password(bytes, password).map_err(|e| e.to_string())?;
        opened(&mut doc);
        return Ok((doc, true));
    }
    if ext.is_empty() || signature_contradicts(&ext, bytes) {
        return open_by_content(name, &ext, bytes, password, page);
    }
    let mut doc = match ext.as_str() {
        "doc" | "dot" => {
            let (mut doc, encrypted) = open_binary_doc(bytes, password)?;
            opened(&mut doc);
            return Ok((doc, encrypted));
        }
        // Plain text goes through the formats importer: BOMs, unmarked UTF-16, Windows-1252.
        "txt" | "text" => wordcraft_formats::txt::import_on(bytes, page)?,
        "json" => {
            json_within_limits(name, bytes)?;
            let mut d = serde_json::from_slice::<Document>(bytes).map_err(|e| format!("{name}: {e}"))?;
            d.normalize_runs();
            d
        }
        other => match crate::io_ext::open(other, bytes, page) {
            Some(r) => r?,
            None => return open_by_content(name, other, bytes, password, page),
        },
    };
    opened(&mut doc);
    Ok((doc, false))
}

/// The narrowest text width of the document's sections, points: what a picture must fit.
pub(crate) fn text_width(doc: &Document) -> f32 {
    doc.sections().iter().map(|(_, sp)| sp.text_width()).fold(f32::INFINITY, f32::min).clamp(36.0, 4000.0)
}

/// Shrink inline pictures wider than `max_w` to it, keeping their proportions. Layout keeps a
/// picture's size (as Word does), so formats whose pictures come at their pixel size (HTML,
/// Markdown, ODT, RTF) are fitted when they are read, as Insert › Pictures does.
pub(crate) fn fit_block_pictures(b: &mut wordcraft_doc::Block, max_w: f32) {
    crate::cmd::paste::each_para(b, 0, &mut |p| {
        let mut changed = false;
        for o in &mut p.objects {
            if let wordcraft_doc::InlineObject::Image { w, h, float, .. } = o
                && float.wrap == wordcraft_doc::para::Wrap::Inline
                && w.is_finite()
                && *w > max_w
            {
                let k = max_w / *w;
                *w = max_w;
                *h = (*h * k).max(1.0);
                changed = true;
            }
        }
        if changed {
            p.touch();
        }
    });
}

/// [`fit_block_pictures`] for a whole document read from a file, against its own text width.
pub(crate) fn fit_pictures(doc: &mut Document) {
    let max_w = text_width(doc);
    for b in &mut doc.body {
        fit_block_pictures(std::sync::Arc::make_mut(b), max_w);
    }
}

/// Does the file hold a different document format than its Word or RTF extension says? An RTF
/// file saved as `.doc` (as accounting systems and report generators often save their
/// documents), a `.docx` package renamed `.doc`, a Word 97-2003 file renamed `.docx`, a Word XML
/// Document (2003 or Flat OPC) or a web page saved as `.doc` (#2853): Word opens
/// each by what it holds, so they go through [`open_by_content`] instead of failing.
fn signature_contradicts(ext: &str, bytes: &[u8]) -> bool {
    let expected = match ext {
        "docx" | "docm" | "dotx" | "dotm" => "zip",
        "doc" | "dot" => "ole",
        "rtf" => "rtf",
        _ => return false,
    };
    let held = if bytes.starts_with(ZIP_SIG) {
        "zip"
    } else if bytes.starts_with(OLE_SIG) {
        "ole"
    } else if bytes.starts_with(b"{\\rtf") {
        "rtf"
    } else if is_word_xml(bytes) {
        "xml"
    } else if looks_like_web_page(bytes) {
        "html"
    } else {
        return false;
    };
    held != expected
}

/// The bytes after a UTF-8 byte-order mark and leading white space.
fn after_bom(bytes: &[u8]) -> &[u8] {
    let b = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(bytes);
    let start = b.iter().position(|c| !c.is_ascii_whitespace()).unwrap_or(b.len());
    b.get(start..).unwrap_or_default()
}

/// Is this a Word XML Document: Word 2003 XML (`w:wordDocument`) or Flat OPC (`pkg:package`)?
/// Only text that starts with markup is parsed (UTF-16 has its byte-order mark).
fn is_word_xml(bytes: &[u8]) -> bool {
    let utf16 = bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]);
    (utf16 || after_bom(bytes).starts_with(b"<")) && !matches!(wordcraft_docx::sniff_xml(bytes), wordcraft_docx::XmlKind::Other)
}

/// Does the file start like a web page (`<!DOCTYPE html …>` or `<html …>`, after an XML
/// declaration and comments)? Word's Save as Web Page and many report generators write HTML
/// under a `.doc` name, and Word opens it as a web page.
fn looks_like_web_page(bytes: &[u8]) -> bool {
    let head = wordcraft_formats::txt::decode(bytes.get(..bytes.len().min(4096)).unwrap_or(bytes)).to_ascii_lowercase();
    let mut t = head.trim_start_matches(['\u{FEFF}', ' ', '\t', '\r', '\n']);
    for _ in 0..64 {
        let skip = if t.starts_with("<!--") {
            t.find("-->").map(|i| i + 3)
        } else if t.starts_with("<?") {
            t.find("?>").map(|i| i + 2)
        } else {
            break;
        };
        let Some(rest) = skip.and_then(|i| t.get(i..)) else { return false };
        t = rest.trim_start();
    }
    // The tag name ends there: `<htmlish>` is some other XML, not a web page.
    let tag = |name: &str| {
        t.strip_prefix(name).is_some_and(|rest| rest.is_empty() || rest.starts_with(|c: char| c.is_ascii_whitespace() || c == '>' || c == '/'))
    };
    tag("<!doctype html") || tag("<html")
}

/// A file whose extension names no format WordCraft knows (`.m3u`, `.log`, `.ini`, none…),
/// opened by what it holds, as Word does: a Word document or RTF file by its signature, a Word
/// XML Document by its root element, a web page under a Word or RTF name as one
/// (#2853), any other text through the plain-text import with encoding detection (#569).
/// Anything else is refused with a clear error rather than shown as garbage.
fn open_by_content(
    name: &str,
    ext: &str,
    bytes: &[u8],
    password: Option<&str>,
    page: &wordcraft_doc::SectionProps,
) -> Result<(Document, bool), String> {
    let (mut doc, encrypted) = if bytes.starts_with(OLE_SIG) && wordcraft_docx::is_encrypted(bytes) {
        (wordcraft_docx::read_with_password(bytes, password).map_err(|e| e.to_string())?, true)
    } else if bytes.starts_with(ZIP_SIG) {
        (wordcraft_docx::read(bytes).map_err(|e| format!("{name}: not a document Etch can open ({e})"))?, false)
    } else if bytes.starts_with(OLE_SIG) {
        open_binary_doc(bytes, password)
            .map_err(|e| if needs_password(&e) || wrong_password(&e) { e } else { format!("{name}: not a document Etch can open ({e})") })?
    } else if bytes.starts_with(b"{\\rtf") {
        (wordcraft_formats::rtf::import(bytes)?, false)
    } else if is_word_xml(bytes) {
        (wordcraft_docx::read_xml(bytes).map_err(|e| format!("{name}: {e}"))?, false)
    } else if matches!(ext, "doc" | "dot" | "docx" | "docm" | "dotx" | "dotm" | "rtf") && looks_like_web_page(bytes) {
        let opened = crate::io_ext::open("html", bytes, page).unwrap_or_else(|| Err(format!("{name}: can't open web pages")));
        (opened?, false)
    } else if wordcraft_formats::txt::is_text(bytes) {
        (wordcraft_formats::txt::import_on(bytes, page)?, false)
    } else if ext.is_empty() {
        return Err(format!("{name}: not a document or text file Etch can open"));
    } else {
        return Err(format!("{name}: unsupported format `.{ext}` (not a document or text file)"));
    };
    opened(&mut doc);
    Ok((doc, encrypted))
}

/// Serialise a document; `name` gives the format by extension.
pub fn save_bytes(name: &str, doc: &Document) -> Result<Vec<u8>, String> {
    save_bytes_opts(name, doc, &ExportOptions::default())
}

/// [`save_bytes`] with export options (Print Markup).
pub fn save_bytes_opts(name: &str, doc: &Document, opts: &ExportOptions) -> Result<Vec<u8>, String> {
    let ext = ext_of(name);
    match ext.as_str() {
        // List paragraphs get their label and a tab, as in Word's plain-text save. A file read
        // as text is written in the encoding, BOM and line ends it was read with.
        "txt" | "text" => Ok(wordcraft_formats::txt::export_with(doc, opts.markup && opts.comments)),
        "json" => serde_json::to_vec_pretty(doc).map_err(|e| e.to_string()),
        other => match crate::io_ext::save(other, doc, opts) {
            Some(r) => r,
            None => Err(format!("{name}: can't save as `.{other}`")),
        },
    }
}

/// [`save_bytes`], encrypted with `password` when there is one and the format is a Word package
/// (other formats are copies and are written as they are).
pub fn save_bytes_with(name: &str, doc: &Document, password: Option<&str>, opts: &ExportOptions) -> Result<Vec<u8>, String> {
    let bytes = save_bytes_opts(name, doc, opts)?;
    match password {
        Some(pw) if can_encrypt(name) => wordcraft_docx::encrypt(&bytes, pw).map_err(|e| e.to_string()),
        _ => Ok(bytes),
    }
}

pub fn open_path(path: &std::path::Path) -> Result<Document, String> {
    open_path_with(path, None).map(|(doc, _)| doc)
}

/// [`open_path`] for a file that may be password-protected (see [`open_bytes_with`]).
#[cfg(not(target_arch = "wasm32"))]
pub fn open_path_with(path: &std::path::Path, password: Option<&str>) -> Result<(Document, bool), String> {
    open_path_in(path, password, &wordcraft_geom::Region::default())
}

/// [`open_path_with`] in `region` (see [`open_bytes_in`]).
#[cfg(not(target_arch = "wasm32"))]
pub fn open_path_in(path: &std::path::Path, password: Option<&str>, region: &wordcraft_geom::Region) -> Result<(Document, bool), String> {
    match open_path_versioned(path, password, region)? {
        (Some(doc), encrypted, _) => Ok((doc, encrypted)),
        // An empty file is no document of any format.
        (None, ..) => open_bytes_in(&path.to_string_lossy(), &[], password, region),
    }
}

/// [`open_path_in`], recording exactly the bytes read, including encrypted package bytes. `None`
/// for an empty file (#1384: the caller decides what it opens as), with its version all the same.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn open_path_versioned(
    path: &std::path::Path,
    password: Option<&str>,
    region: &wordcraft_geom::Region,
) -> Result<(Option<Document>, bool, FileVersion), String> {
    crate::file_access::check_read(&path.to_string_lossy())?;
    // The same capped, regular-files-only read as every other open; the identity is taken from
    // the handle and the bytes it read.
    let (bytes, file) = read_capped_file(path, MAX_OPEN_BYTES, "file")?;
    let version = FileVersion::of_read(path, file, &bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.is_empty() {
        return Ok((None, false, version));
    }
    let name = path.to_string_lossy();
    if matches!(ext_of(&name).as_str(), "html" | "htm" | "xhtml") {
        // Pictures referenced by a relative path load from beside the HTML file.
        let images = LocalImages::new(path.parent().unwrap_or(std::path::Path::new("")));
        let html = wordcraft_formats::html::decode(&bytes);
        let mut doc = wordcraft_formats::html::import_file(&html, &|src| images.load(src), &region_page(region))?;
        fit_pictures(&mut doc);
        opened(&mut doc);
        return Ok((Some(doc), false, version));
    }
    let (doc, encrypted) = open_bytes_in(&name, &bytes, password, region)?;
    Ok((Some(doc), encrypted, version))
}

/// The pictures an HTML file names by a path relative to its folder.
///
/// Only regular files inside the folder (after resolving `..` and symlinks) are read. The folder is
/// the HTML file's own, which `open_path_with` has already checked against the automation read root. URLs
/// (`http:`, `file:`…) and absolute paths are not followed. Each file is read at most once and
/// at most `max_file` bytes of it; every reference, cached or not, counts against a budget for
/// the whole document, so one large picture can't be multiplied by referencing it many times.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct LocalImages {
    /// The folder, canonical; `None` when it can't be resolved (nothing loads then).
    dir: Option<std::path::PathBuf>,
    max_file: u64,
    /// Bytes the document may still load.
    left: std::cell::Cell<u64>,
    cache: std::cell::RefCell<Vec<(std::path::PathBuf, std::sync::Arc<Vec<u8>>)>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl LocalImages {
    /// Largest picture file read.
    const MAX_FILE: u64 = 256 << 20;
    /// Picture bytes one document may load, counting every reference.
    const BUDGET: u64 = 1 << 30;
    /// Files whose bytes are kept for further references.
    const CACHE: usize = 64;

    pub(crate) fn new(dir: &std::path::Path) -> LocalImages {
        LocalImages::with_limits(dir, LocalImages::MAX_FILE, LocalImages::BUDGET)
    }

    pub(crate) fn with_limits(dir: &std::path::Path, max_file: u64, budget: u64) -> LocalImages {
        let dir = if dir.as_os_str().is_empty() { std::path::Path::new(".") } else { dir };
        LocalImages { dir: std::fs::canonicalize(dir).ok(), max_file, left: std::cell::Cell::new(budget), cache: Default::default() }
    }

    /// Bytes of the file `src` names, or `None` when it isn't a regular file inside the folder,
    /// is too large, or the budget is spent.
    pub(crate) fn load(&self, src: &str) -> Option<std::sync::Arc<Vec<u8>>> {
        let path = self.resolve(src)?;
        let cached = self.cache.borrow().iter().find(|(p, _)| *p == path).map(|(_, d)| d.clone());
        if let Some(data) = cached {
            self.charge(data.len())?;
            return Some(data);
        }
        let limit = self.max_file.min(self.left.get());
        let bytes = read_regular_file(&path, limit)?;
        // What was read is spent even when it's over the limit.
        self.charge(bytes.len())?;
        if bytes.len() as u64 > limit {
            return None;
        }
        let data = std::sync::Arc::new(bytes);
        let mut cache = self.cache.borrow_mut();
        if cache.len() < LocalImages::CACHE {
            cache.push((path, data.clone()));
        }
        Some(data)
    }

    /// Take `n` bytes from the budget; `None` (and the budget emptied) when there aren't enough.
    fn charge(&self, n: usize) -> Option<()> {
        let left = self.left.get().checked_sub(n as u64);
        self.left.set(left.unwrap_or(0));
        left.map(|_| ())
    }

    /// The canonical path `src` names, if it's inside the folder.
    fn resolve(&self, src: &str) -> Option<std::path::PathBuf> {
        let src = src.split(['#', '?']).next().unwrap_or("");
        if src.is_empty() || src.starts_with("//") || src.split_once(':').is_some_and(|(scheme, _)| !scheme.contains('/')) {
            return None;
        }
        let rel = String::from_utf8(wordcraft_formats::model::percent_decode(src)).ok()?;
        let rel = std::path::Path::new(&rel);
        if rel.components().any(|c| matches!(c, std::path::Component::Prefix(_) | std::path::Component::RootDir)) {
            return None;
        }
        let dir = self.dir.as_ref()?;
        // Canonical paths have `..` and symlinks resolved, so this keeps both inside the folder.
        let path = std::fs::canonicalize(dir.join(rel)).ok()?;
        path.starts_with(dir).then_some(path)
    }
}

/// Up to `limit + 1` bytes of the regular file at `path` (more than `limit` means too large), or
/// `None` for anything else. A named pipe blocks when opened and a device reports a length of 0
/// and may never end, so the path is checked before opening, the opened handle is checked again,
/// and the read is bounded whatever length the file reports.
#[cfg(not(target_arch = "wasm32"))]
fn read_regular_file(path: &std::path::Path, limit: u64) -> Option<Vec<u8>> {
    read_regular(path, limit).map(|(bytes, _)| bytes)
}

/// [`read_regular_file`], and the open handle it read from.
#[cfg(not(target_arch = "wasm32"))]
fn read_regular(path: &std::path::Path, limit: u64) -> Option<(Vec<u8>, std::fs::File)> {
    use std::io::Read;
    let (file, meta) = open_regular(path).ok()?;
    let mut out = Vec::with_capacity(usize::try_from(meta.len().min(limit)).unwrap_or(0));
    (&file).take(limit.saturating_add(1)).read_to_end(&mut out).ok()?;
    Some((out, file))
}

/// The regular file at `path`, open, with its handle's metadata. The path is checked before
/// opening (a named pipe blocks `open` until a writer appears, #3666) and the handle after (the
/// path may have been swapped in between); anything but a regular file is "not a file".
#[cfg(not(target_arch = "wasm32"))]
fn open_regular(path: &std::path::Path) -> Result<(std::fs::File, std::fs::Metadata), String> {
    let not_a_file = || format!("{}: not a file", path.display());
    let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !meta.is_file() {
        return Err(not_a_file());
    }
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let meta = file.metadata().map_err(|e| format!("{}: {e}", path.display()))?;
    if !meta.is_file() {
        return Err(not_a_file());
    }
    Ok((file, meta))
}

/// The largest document [`open_path_with`] reads.
pub const MAX_OPEN_BYTES: u64 = 2 << 30;
/// The largest file [`read_file`] reads (a picture, mail-merge data).
pub const MAX_READ_BYTES: u64 = 256 << 20;

/// The bytes of the regular file at `path`, at most `max` of them. Anything else (a directory,
/// a named pipe that would block, a device such as `/dev/zero` that never ends) and anything
/// larger is an error, found without reading more than `max + 1` bytes.
#[cfg(not(target_arch = "wasm32"))]
fn read_capped(path: &std::path::Path, max: u64, what: &str) -> Result<Vec<u8>, String> {
    read_capped_file(path, max, what).map(|(bytes, _)| bytes)
}

/// [`read_capped`], and the open handle it read from.
#[cfg(not(target_arch = "wasm32"))]
fn read_capped_file(path: &std::path::Path, max: u64, what: &str) -> Result<(Vec<u8>, std::fs::File), String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !meta.is_file() {
        return Err(format!("{}: not a file", path.display()));
    }
    let too_large = || format!("{}: the {what} is larger than {} MB", path.display(), max >> 20);
    if meta.len() > max {
        return Err(too_large());
    }
    let (bytes, file) = read_regular(path, max).ok_or_else(|| format!("{}: can't read this file", path.display()))?;
    if bytes.len() as u64 > max {
        return Err(too_large());
    }
    Ok((bytes, file))
}

/// The bytes of the file at `path`, for commands that take a path (pictures, mail-merge data).
/// Every such read goes through here so the automation read root applies ([`crate::file_access`]).
/// Only a regular file of at most [`MAX_READ_BYTES`] is read.
#[cfg(not(target_arch = "wasm32"))]
pub fn read_file(path: &str) -> Result<Vec<u8>, String> {
    crate::file_access::check_read(path)?;
    read_capped(std::path::Path::new(path), MAX_READ_BYTES, "file")
}

#[cfg(target_arch = "wasm32")]
pub fn read_file(path: &str) -> Result<Vec<u8>, String> {
    Err(format!("{path}: files are opened through the browser on the web"))
}

/// [`read_file`] for a file that must be a regular file of at most `max` bytes, checked before
/// anything is read (a font named by the user, which could be any size).
#[cfg(not(target_arch = "wasm32"))]
pub fn read_file_capped(path: &str, max: u64, what: &str) -> Result<Vec<u8>, String> {
    crate::file_access::check_read(path)?;
    read_capped(std::path::Path::new(path), max, what)
}

#[cfg(target_arch = "wasm32")]
pub fn read_file_capped(path: &str, _max: u64, _what: &str) -> Result<Vec<u8>, String> {
    Err(format!("{path}: files are opened through the browser on the web"))
}

/// The first `max` bytes of the file at `path` (fewer when it is shorter), inside the automation
/// read root when there is one: enough to tell a file's format by its start. Only a regular file
/// is opened: a named pipe would block the open until a writer appeared, and this runs on the
/// save path (is the file saved over a Word 2003 XML document?), so a pipe named `.xml` hung
/// Save (#3666, upstream 4095cc34).
#[cfg(not(target_arch = "wasm32"))]
pub fn read_file_head(path: &str, max: u64) -> Result<Vec<u8>, String> {
    use std::io::Read;
    crate::file_access::check_read(path)?;
    let (f, _) = open_regular(std::path::Path::new(path))?;
    let mut head = Vec::new();
    f.take(max).read_to_end(&mut head).map_err(|e| format!("{path}: {e}"))?;
    Ok(head)
}

/// Rename without replacing another file, respecting both automation access roots.
#[cfg(not(target_arch = "wasm32"))]
pub fn rename_file(source: &std::path::Path, destination: &std::path::Path) -> Result<(), String> {
    crate::file_access::check_read(&source.to_string_lossy())?;
    crate::file_access::check_write(&source.to_string_lossy())?;
    crate::file_access::check_write(&destination.to_string_lossy())?;
    // Creating a sibling hard link refuses collisions atomically and keeps the original bytes.
    if let Err(e) = std::fs::hard_link(source, destination) {
        if e.kind() == std::io::ErrorKind::AlreadyExists && only_case_differs(source, destination) {
            return std::fs::rename(source, destination).map_err(|e| format!("Could not rename document: {e}"));
        }
        return Err(format!("Could not rename document: {e}"));
    }
    if let Err(e) = std::fs::remove_file(source) {
        let _ = std::fs::remove_file(destination);
        return Err(format!("Could not rename document: {e}"));
    }
    Ok(())
}

/// Whether `destination` names `source` in the same directory spelled with different case, and
/// the directory holds no entry spelled exactly as `destination`.
#[cfg(not(target_arch = "wasm32"))]
fn only_case_differs(source: &std::path::Path, destination: &std::path::Path) -> bool {
    let (Some(dir), Some(from), Some(to)) = (source.parent(), source.file_name(), destination.file_name()) else {
        return false;
    };
    if destination.parent() != Some(dir) || from == to || from.to_string_lossy().to_lowercase() != to.to_string_lossy().to_lowercase() {
        return false;
    }
    let dir = if dir.as_os_str().is_empty() { std::path::Path::new(".") } else { dir };
    std::fs::read_dir(dir).is_ok_and(|entries| entries.filter_map(Result::ok).all(|entry| entry.file_name() != to))
}

/// Write `bytes` to the file at `path`, inside the automation write root when there is one.
/// Written like a save (a temporary file renamed over the target), so a failed write leaves any
/// existing file whole.
#[cfg(not(target_arch = "wasm32"))]
pub fn write_file(path: &str, bytes: &[u8]) -> Result<(), String> {
    crate::file_access::check_write(path)?;
    // Like a document save: the old file is replaced only once the new bytes are fully written.
    safe_save::write(std::path::Path::new(path), bytes).map_err(|e| format!("{path}: {e}"))
}

#[cfg(target_arch = "wasm32")]
pub fn write_file(path: &str, _bytes: &[u8]) -> Result<(), String> {
    Err(format!("{path}: files are saved through the browser on the web"))
}

#[cfg(target_arch = "wasm32")]
pub fn open_path_with(path: &std::path::Path, _password: Option<&str>) -> Result<(Document, bool), String> {
    Err(format!("{}: files are opened through the browser on the web", path.display()))
}

#[cfg(target_arch = "wasm32")]
pub fn open_path_in(path: &std::path::Path, password: Option<&str>, _region: &wordcraft_geom::Region) -> Result<(Document, bool), String> {
    open_path_with(path, password)
}

pub fn save_path(path: &std::path::Path, doc: &Document) -> Result<(), String> {
    save_path_with(path, doc, None, &ExportOptions::default())
}

/// [`save_path`], encrypted with `password` as [`save_bytes_with`] does.
#[cfg(not(target_arch = "wasm32"))]
pub fn save_path_with(path: &std::path::Path, doc: &Document, password: Option<&str>, opts: &ExportOptions) -> Result<(), String> {
    crate::file_access::check_write(&path.to_string_lossy())?;
    let bytes = save_bytes_with(&path.to_string_lossy(), doc, password, opts)?;
    safe_save::write(path, &bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// Nothing is at `path`, not even a dangling link, and the roots let it be written: where a
/// Save As may put the session's file back once it is gone (`cmd::file::save`).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn nothing_at(path: &std::path::Path) -> bool {
    crate::file_access::check_write(&path.to_string_lossy()).is_ok()
        && std::fs::symlink_metadata(path).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
}

/// Refuse an already conflicting password-save request before any recovery purge barrier.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn check_save_destination(path: &std::path::Path, expected: Option<&FileVersion>) -> Result<(), crate::CmdError> {
    crate::file_access::check_write(&path.to_string_lossy()).map_err(crate::CmdError::Failed)?;
    if let Some(expected) = expected
        && expected.owns(path).map_err(crate::CmdError::FileConflict)?
    {
        expected.verify(path).map_err(crate::CmdError::FileConflict)?;
    }
    // A read-only file is refused before a password save changes anything (as the save would).
    if safe_save::target(path).ok().and_then(|t| std::fs::metadata(t).ok()).is_some_and(|m| m.is_file() && m.permissions().readonly()) {
        return Err(crate::CmdError::ReadOnly(path.to_path_buf()));
    }
    Ok(())
}

/// Session-owned native save: compare content after serialization, before replacement. This
/// detects pre-existing changes; it is not an atomic conditional write against other writers.
#[cfg(not(target_arch = "wasm32"))]
pub fn save_path_versioned(
    path: &std::path::Path,
    doc: &Document,
    password: Option<&str>,
    opts: &ExportOptions,
    expected: Option<&FileVersion>,
) -> Result<FileVersion, crate::CmdError> {
    crate::file_access::check_write(&path.to_string_lossy()).map_err(crate::CmdError::Failed)?;
    let bytes = save_bytes_with(&path.to_string_lossy(), doc, password, opts).map_err(crate::CmdError::Failed)?;
    let expected = match expected {
        Some(v) if v.owns(path).map_err(crate::CmdError::FileConflict)? => Some(v),
        _ => None,
    };
    let prepared = match safe_save::prepare(path, &bytes) {
        Ok(prepared) => prepared,
        Err(e) => {
            // A preparation error may come from an inaccessible destination, not its parent.
            if let Some(expected) = expected {
                expected.verify(path).map_err(crate::CmdError::FileConflict)?;
            }
            // Read-only: refused like Word, and AutoSave too, with Save As suggested.
            if safe_save::is_read_only(&e) {
                return Err(crate::CmdError::ReadOnly(path.to_path_buf()));
            }
            return Err(crate::CmdError::Failed(format!("{}: {e}", path.display())));
        }
    };
    // Tests: another program may act here (`fault`), after the bytes are ready.
    #[cfg(test)]
    fault::at(path, fault::Step::Verify).map_err(|e| crate::CmdError::Failed(format!("{}: {e}", path.display())))?;
    if let Some(expected) = expected {
        expected.verify(path).map_err(crate::CmdError::FileConflict)?;
    }
    prepared.commit().map_err(|e| crate::CmdError::Failed(format!("{}: {e}", path.display())))?;
    Ok(FileVersion::saved(path, &bytes))
}

#[cfg(target_arch = "wasm32")]
pub fn save_path_with(path: &std::path::Path, _doc: &Document, _password: Option<&str>, _opts: &ExportOptions) -> Result<(), String> {
    Err(format!("{}: files are saved through the browser on the web", path.display()))
}
