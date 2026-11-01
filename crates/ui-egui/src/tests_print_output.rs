//! Output the window makes besides File › Export (the File › Print preview, the page-orientation
//! probe before a native print, the web build's PDF downloads) takes the window's layout when it
//! is the print layout ([`wordcraft_engine::Session::print_layout`]), as Print and Export do, and
//! is the same as from a layout from scratch: the preview's pixels, the probe's answer, the
//! downloaded PDF's bytes. The count of layouts made from scratch for output
//! ([`wordcraft_engine::print::fresh_layouts`]) tells the two paths apart.

use std::cell::RefCell;
use std::rc::Rc;

use serde_json::json;
use wordcraft_engine::print::fresh_layouts;

use crate::{Services, WordApp};

/// A document of a few pages with a misspelt word on every line (the window's layout holds
/// squiggles for them), a fixed creation date (PDFs are stamped with it, else with the time).
fn app() -> WordApp {
    let mut doc = wordcraft_doc::Document::from_text(&"Teh quick brown fox jumpd over the lazzy dog. ".repeat(200));
    doc.core.created = "2026-01-01T00:00:00Z".into();
    let mut a = WordApp::new(wordcraft_engine::Session::new(doc), Services::default());
    a.ui.language = "en".into();
    a
}

/// The preview's layout is the window's (no layout of its own), and its pages draw the same
/// pixels as those of a layout from scratch: the squiggles the window's layout holds are left out.
#[test]
fn the_print_preview_takes_the_window_layout_and_draws_the_printed_page() {
    let mut a = app();
    let window = a.session.layout();
    assert!(window.pages.iter().any(|p| !p.items.is_empty()));
    let ctx = egui::Context::default();
    let before = fresh_layouts();
    let lay = crate::print_page::layout_of(&mut a, &ctx);
    assert_eq!(fresh_layouts() - before, 0, "no layout of its own");
    assert!(std::sync::Arc::ptr_eq(&lay, &window), "the window's layout");
    let scratch = a.session.export_layout();
    assert_eq!(lay.pages.len(), scratch.pages.len());
    for (i, (p, q)) in lay.pages.iter().zip(&scratch.pages).enumerate().take(2) {
        let shown = wordcraft_render::render_page(&a.session.doc, p, 1.0, &crate::print_page::preview_options());
        let printed = wordcraft_render::render_page(&a.session.doc, q, 1.0, &Default::default());
        assert!(shown.pixels == printed.pixels, "page {i}: the preview draws the printed page");
    }
    // With squiggles drawn the window's page differs: the comparison above can tell.
    let squiggly = wordcraft_render::render_page(&a.session.doc, &lay.pages[0], 1.0, &Default::default());
    let printed = wordcraft_render::render_page(&a.session.doc, &scratch.pages[0], 1.0, &Default::default());
    assert!(squiggly.pixels != printed.pixels, "the window's layout holds squiggles");
    // ¶ shown: the window's layout isn't the print layout, so the preview has one of its own.
    a.run("view.marks", json!({"value": true})).unwrap();
    let _ = a.session.layout();
    a.run("text.insert", json!({"text": "More. "})).unwrap();
    let _ = a.session.layout();
    let before = fresh_layouts();
    let lay = crate::print_page::layout_of(&mut a, &ctx);
    assert_eq!(fresh_layouts() - before, 1, "laid out for print");
    assert_eq!(format!("{:?}", lay.pages), format!("{:?}", a.session.export_layout().pages));
}

/// Before a native print with automatic orientation, the first page's shape comes from the
/// window's layout: the job costs no layout besides the window's, and a landscape page still
/// asks for landscape.
#[test]
fn the_orientation_probe_takes_the_window_layout() {
    for landscape in [false, true] {
        let mut a = app();
        if landscape {
            a.run("layout.orientation", json!({"value": "landscape"})).unwrap();
        }
        let _ = a.session.layout();
        a.printing.printers.push(crate::printing::Printer { name: "Queue".into(), default: true });
        a.run("ui.printSettings", json!({"printer": "Queue"})).unwrap();
        let jobs: Rc<RefCell<Vec<Vec<(String, String)>>>> = Rc::default();
        let seen = jobs.clone();
        a.services.print_native = Some(Box::new(move |_, _, _, options| {
            seen.borrow_mut().push(options);
            let (sender, receiver) = std::sync::mpsc::channel();
            let _ = sender.send(Ok("queued".into()));
            receiver
        }));
        let before = fresh_layouts();
        a.run("ui.printSubmit", json!({})).unwrap();
        assert_eq!(fresh_layouts() - before, 0, "landscape {landscape}: no layout but the window's");
        let jobs = jobs.borrow();
        let wanted = if landscape { "4" } else { "3" };
        assert!(jobs.len() == 1 && jobs[0].iter().any(|(k, v)| k == "orientation-requested" && v == wanted), "{jobs:?}");
    }
}

/// The web build's PDF downloads (Save As PDF, Export PDF) are written from the window's layout,
/// to the same bytes as from a layout from scratch; other formats lay nothing out.
#[test]
fn web_pdf_downloads_take_the_window_layout() {
    let mut a = app();
    let got: Rc<RefCell<Vec<(String, Vec<u8>)>>> = Rc::default();
    let sink = got.clone();
    a.services.download = Some(Box::new(move |n: &str, b: &[u8]| {
        sink.borrow_mut().push((n.to_string(), b.to_vec()));
        Ok(())
    }));
    let _ = a.session.layout();
    let mut made = 0;
    let mut run = |a: &mut WordApp, id: &str, path: &str| {
        let before = fresh_layouts();
        a.execute(id, json!({"path": path})).unwrap();
        made += fresh_layouts() - before;
    };
    // A PDF from scratch (a session never laid out): what the downloads must hold.
    let scratch = |a: &WordApp| wordcraft_engine::io::save_bytes("scratch.pdf", &a.session.doc).unwrap();
    run(&mut a, "file.exportPdf", "novel.pdf");
    let exported = scratch(&a);
    run(&mut a, "file.saveAs", "novel.docx");
    // (A save stamps the document: the PDF saved next is of the stamped document.)
    run(&mut a, "file.saveAs", "novel.PDF");
    let saved = scratch(&a);
    assert_eq!(made, 0, "no layout but the window's");
    let got = got.borrow();
    let names: Vec<&str> = got.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["novel.pdf", "novel.docx", "novel.PDF"]);
    assert!(got[0].1 == exported, "Export PDF: the bytes a layout from scratch writes");
    assert!(got[2].1 == saved, "Save As PDF: the bytes a layout from scratch writes");
}
