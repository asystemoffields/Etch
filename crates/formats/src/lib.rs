//! WordCraft import/export for the "other" text formats: plain text, Markdown (CommonMark subset
//! plus GFM tables and strikethrough), HTML, RTF, OpenDocument Text and LaTeX — and, read-only,
//! comma-separated values for tables ([`csv`]).
//!
//! Every format maps to a small flow model ([`model::Flow`]: paragraphs with a kind, list
//! membership, alignment and formatted spans; tables of cells), which [`model::to_doc`] and
//! [`model::from_doc`] convert to and from the full [`Document`]. All parsers are written from
//! the public specifications, are lenient by design and never panic on hostile input: nesting is
//! depth-limited and sizes are capped.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod comments;
pub mod csv;
pub mod html;
pub mod latex;
pub mod markdown;
pub mod mathml;
pub mod model;
pub mod odt;
pub mod rtf;
pub mod txt;

use wordcraft_doc::Document;

/// Extensions this crate handles.
pub const EXTENSIONS: &[&str] = &["txt", "text", "md", "markdown", "html", "htm", "xhtml", "rtf", "odt", "fodt", "tex", "latex", "ltx"];

/// Import `bytes` in the format named by `ext` (no dot, any case). `None` = not our format.
pub fn import(ext: &str, bytes: &[u8]) -> Option<Result<Document, String>> {
    let ext = ext.trim_start_matches('.').to_ascii_lowercase();
    let r = match ext.as_str() {
        "txt" | "text" => txt::import(bytes),
        "md" | "markdown" => markdown::import(&txt::decode(bytes)),
        "html" | "htm" | "xhtml" => html::import_file(&html::decode(bytes), &|_| None, &wordcraft_doc::SectionProps::default()),
        "rtf" => rtf::import(bytes),
        "odt" | "fodt" => odt::import(bytes),
        "tex" | "latex" | "ltx" => latex::import(bytes),
        _ => return None,
    };
    Some(r.map(|mut d| {
        d.ensure_nonempty();
        d
    }))
}

/// [`import`] onto `page` (its size and margins) for the formats whose files give no page size:
/// plain text, Markdown and HTML start on it, and their tables size to it (#1381). The other
/// formats keep the page their file gives.
pub fn import_on(ext: &str, bytes: &[u8], page: &wordcraft_doc::SectionProps) -> Option<Result<Document, String>> {
    let ext = ext.trim_start_matches('.').to_ascii_lowercase();
    let mut doc = match ext.as_str() {
        "txt" | "text" => match txt::import_on(bytes, page) {
            Ok(doc) => doc,
            Err(e) => return Some(Err(e)),
        },
        "md" | "markdown" => match markdown::import_on(&txt::decode(bytes), page) {
            Ok(doc) => doc,
            Err(e) => return Some(Err(e)),
        },
        "html" | "htm" | "xhtml" => match html::import_file(&html::decode(bytes), &|_| None, page) {
            Ok(doc) => doc,
            Err(e) => return Some(Err(e)),
        },
        _ => return import(&ext, bytes),
    };
    doc.ensure_nonempty();
    Some(Ok(doc))
}

/// Export `doc` in the format named by `ext`. `None` = not our format.
pub fn export(ext: &str, doc: &Document) -> Option<Result<Vec<u8>, String>> {
    let ext = ext.trim_start_matches('.').to_ascii_lowercase();
    Some(match ext.as_str() {
        "txt" | "text" => Ok(txt::export(doc)),
        "md" | "markdown" => Ok(markdown::export(doc).into_bytes()),
        "html" | "htm" | "xhtml" => Ok(html::export(doc).into_bytes()),
        "rtf" => Ok(rtf::export(doc).into_bytes()),
        "odt" => odt::export(doc),
        "tex" | "latex" | "ltx" => Ok(latex::export(doc).into_bytes()),
        _ => return None,
    })
}

/// How [`export_with`] writes a copy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExportOptions {
    /// Print Markup: HTML, Markdown and plain text end with the document's comments (who wrote
    /// each, the text it comments on, what it says, its replies). Other formats keep comments
    /// their own way or not at all.
    pub comments: bool,
}

/// [`export`] with options.
pub fn export_with(ext: &str, doc: &Document, opts: &ExportOptions) -> Option<Result<Vec<u8>, String>> {
    let out = export(ext, doc)?;
    if !opts.comments {
        return Some(out);
    }
    let ext = ext.trim_start_matches('.').to_ascii_lowercase();
    // Plain text adds them before encoding, in the file's own encoding and line ends.
    if matches!(ext.as_str(), "txt" | "text") {
        return Some(Ok(txt::export_with(doc, true)));
    }
    Some(out.map(|bytes| comments::append(&ext, doc, bytes)))
}

#[cfg(test)]
mod tests;
