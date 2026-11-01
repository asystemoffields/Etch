//! Picture safety ports from upstream d7429d21 and c99eb500, through the actual insert command.
use serde_json::json;
use wordcraft_doc::Document;
use wordcraft_engine::Session;

#[test]
fn inserting_a_tiff_with_jpeg_strips_is_refused_without_changing_the_document() {
    let mut good = Vec::new();
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(5, 3, image::Rgb([9, 8, 7])))
        .write_to(&mut std::io::Cursor::new(&mut good), image::ImageFormat::Tiff)
        .unwrap();
    // The encoder writes a little-endian classic TIFF. Change only the compression tag.
    assert_eq!(&good[..4], b"II*\0");
    let ifd = u32::from_le_bytes(good[4..8].try_into().unwrap()) as usize;
    let count = u16::from_le_bytes(good[ifd..ifd + 2].try_into().unwrap()) as usize;
    let at = (0..count).map(|i| ifd + 2 + 12 * i).find(|at| u16::from_le_bytes(good[*at..*at + 2].try_into().unwrap()) == 259).unwrap();
    let mut bad = good.clone();
    bad[at + 8..at + 10].copy_from_slice(&7u16.to_le_bytes());
    let mut s = Session::new(Document::new());
    let before = s.doc.clone();
    let depth = s.undo_depth();
    let encoded = wordcraft_engine::cmd::insert::base64_encode(&bad);
    let error = s.run("insert.picture", &json!({"data": encoded})).unwrap_err().to_string();
    assert!(error.contains("TIFF") && error.contains("JPEG"), "{error}");
    assert_eq!(s.doc, before);
    assert_eq!(s.undo_depth(), depth);
    // A normal TIFF can still be inserted and drawn.
    let encoded = wordcraft_engine::cmd::insert::base64_encode(&good);
    s.run("insert.picture", &json!({"data": encoded})).unwrap();
    assert_eq!(s.doc.media.len(), 1);
    assert!(wordcraft_render::decode_pixmap(&good).is_some());
}
