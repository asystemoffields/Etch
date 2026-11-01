//! Format crates registered with the I/O dispatcher.

use wordcraft_doc::Document;

/// Open a format other than plain text / JSON; files that give no page size (Markdown, HTML)
/// start on `page`.
pub fn open(ext: &str, bytes: &[u8], page: &wordcraft_doc::SectionProps) -> Option<Result<Document, String>> {
    match ext {
        "docx" | "docm" | "dotx" | "dotm" => Some(wordcraft_docx::read(bytes).map_err(|e| e.to_string())),
        "doc" | "dot" => Some(wordcraft_docbin::read(bytes).map_err(|e| e.to_string())),
        // Word XML Document (Flat OPC) or Word 2003 XML, told apart by the root element.
        "xml" => Some(wordcraft_docx::read_xml(bytes).map_err(|e| e.to_string())),
        // HTML, Markdown, ODT, RTF…: pictures fit the text width (see `io::fit_pictures`).
        other => wordcraft_formats::import_on(other, bytes, page).map(|r| {
            r.map(|mut doc| {
                crate::io::fit_pictures(&mut doc);
                doc
            })
        }),
    }
}

/// Save to a format other than plain text / JSON.
pub fn save(ext: &str, doc: &Document, opts: &crate::io::ExportOptions) -> Option<Result<Vec<u8>, String>> {
    // .docx/.docm/.dotx/.dotm: the extension picks the package flavour Word expects.
    if let Some(flavor) = wordcraft_docx::Flavor::from_ext(ext) {
        return Some(wordcraft_docx::write_as(doc, flavor).map_err(|e| e.to_string()));
    }
    match ext {
        "xml" if opts.word2003 => Some(wordcraft_docx::write_wordml2003(doc).map_err(|e| e.to_string())),
        "xml" => Some(wordcraft_docx::write_flat(doc).map_err(|e| e.to_string())),
        "png" => Some(render_png(doc, 0, 2.0, opts)),
        "pdf" => {
            let pdf = pdf_options(opts);
            // The window's layout when a save of the session's document lends it (it is the
            // print layout), else one from scratch.
            let pdf = match crate::print::lent_layout(doc, &wordcraft_pdf::layout_options(&pdf)) {
                Some(lay) => wordcraft_pdf::export_layout(doc, &lay, &pdf),
                None => {
                    crate::print::count_fresh();
                    wordcraft_pdf::export(doc, &pdf)
                }
            };
            Some(pdf.map_err(|e| e.to_string()))
        }
        other => wordcraft_formats::export_with(other, doc, &wordcraft_formats::ExportOptions { comments: opts.markup && opts.comments }),
    }
}

/// The PDF options of a copy saved with `opts`: Print Markup's tracked changes and comments.
pub fn pdf_options(opts: &crate::io::ExportOptions) -> wordcraft_pdf::PdfOptions {
    wordcraft_pdf::PdfOptions {
        include_markup: opts.markup,
        comments: opts.markup && opts.comments,
        show_hidden: opts.hidden_text,
        ..Default::default()
    }
}

/// Render page `page` as PNG at `scale` px/pt. With Print Markup's comments the page has its
/// markup area with the comments in balloons (page images always show tracked changes).
pub fn render_png(doc: &Document, page: usize, scale: f32, opts: &crate::io::ExportOptions) -> Result<Vec<u8>, String> {
    let lo = wordcraft_layout::LayoutOptions { comment_balloons: opts.markup && opts.comments, show_hidden: opts.hidden_text, ..Default::default() };
    let l = wordcraft_layout::layout(doc, &mut wordcraft_layout::LayoutCache::new(), &lo);
    let p = l.pages.get(page).ok_or_else(|| format!("no page {}", page + 1))?;
    let img = wordcraft_render::render_page(doc, p, scale, &Default::default());
    img.to_png()
}
