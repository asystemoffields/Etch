//! Decoded-media cache: hits need the same allocation, and the byte budget bounds what is kept.

use super::*;

#[test]
fn media_cache_only_hits_the_same_allocation() {
    let mut c: MediaCache<u32> = MediaCache { map: HashMap::new(), bytes: 0 };
    let a = Arc::new(vec![1u8, 2, 3]);
    let b = Arc::new(vec![1u8, 2, 3]);
    c.insert("pic", &a, 7, 3);
    assert_eq!(c.get("pic", &a), Some(7));
    assert_eq!(c.get("pic", &b), None, "equal bytes in another allocation are another picture");
}

#[test]
fn media_cache_is_bounded_by_bytes() {
    let mut c: MediaCache<u32> = MediaCache { map: HashMap::new(), bytes: 0 };
    let src = Arc::new(vec![0u8]);
    let big = CACHE_BYTES / 2 + 1;
    c.insert("a", &src, 1, big);
    c.insert("b", &src, 2, big);
    assert_eq!(c.get("a", &src), None, "the first entry is evicted to make room");
    assert_eq!(c.get("b", &src), Some(2));
    assert!(c.bytes <= CACHE_BYTES);
    c.insert("huge", &src, 3, CACHE_BYTES + 1);
    assert_eq!(c.get("huge", &src), None, "a value over the whole budget is not kept");
}

#[test]
fn media_cache_counts_the_source_bytes_it_holds() {
    let mut c: MediaCache<Option<u32>> = MediaCache { map: HashMap::new(), bytes: 0 };
    let src = Arc::new(vec![0u8; 1000]);
    c.insert("broken", &src, None, 0);
    assert_eq!(c.bytes, 1000, "an entry that decoded to nothing still holds its source");
    c.insert("pic", &src, Some(1), 24);
    assert_eq!(c.bytes, 2024);
    // A source over the whole budget is not kept, whatever it decoded to (calloc'd, so cheap).
    let huge = Arc::new(vec![0u8; CACHE_BYTES + 1]);
    c.insert("huge", &huge, None, 0);
    assert!(!c.map.contains_key("huge"));
    assert_eq!(CACHE_BYTES * 2, 96 * 1024 * 1024, "the image and vector caches share about 96 MB");
}

/// A wave stroke starting around 1e20 never advanced (`x + step == x`), pushing curves until memory ran out.
#[test]
fn wave_stroke_at_a_huge_coordinate_ends() {
    assert!(wave_path(1e20, 0.0, 2e20, 0.0, 1.0).elements().len() < 3);
    assert!(wave_path(0.0, 0.0, 1e20, 0.0, 1.0).elements().len() < 20_100);
    assert!(wave_path(0.0, 0.0, 100.0, 0.0, 1.0).elements().len() > 10);
    assert!(wave_path(0.0, 0.0, 0.0, 100.0, 1.0).elements().len() > 10, "vertical wave strokes produce elements");
}

/// The largest picture kept, plus a generous source, fits a cache: big pictures are decoded once.
#[test]
fn the_largest_kept_picture_fits_the_cache() {
    assert!(IMAGE_PIXELS as usize * 4 + 8 * 1024 * 1024 <= CACHE_BYTES);
}

fn png(w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::new();
    image::RgbaImage::from_pixel(w, h, image::Rgba([10, 20, 30, 255]))
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out
}

#[test]
fn oversize_pictures_are_scaled_down_keeping_their_shape() {
    let pm = decode_pixmap_within(&png(200, 100), 5000).unwrap();
    assert!(u64::from(pm.width()) * u64::from(pm.height()) <= 5000);
    assert_eq!((pm.width(), pm.height()), (100, 50));
    let small = decode_pixmap_within(&png(20, 10), 5000).unwrap();
    assert_eq!((small.width(), small.height()), (20, 10), "small pictures keep their size");
}

/// A header claiming a 20000×20000 picture is refused from the header alone, before decoding.
#[test]
fn huge_picture_headers_are_refused_before_decoding() {
    let (w, h): (i32, i32) = (20_000, 20_000);
    let mut bmp = Vec::new();
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&(54u32 + 4).to_le_bytes());
    bmp.extend_from_slice(&[0; 4]);
    bmp.extend_from_slice(&54u32.to_le_bytes());
    bmp.extend_from_slice(&40u32.to_le_bytes());
    bmp.extend_from_slice(&w.to_le_bytes());
    bmp.extend_from_slice(&h.to_le_bytes());
    bmp.extend_from_slice(&1u16.to_le_bytes());
    bmp.extend_from_slice(&32u16.to_le_bytes());
    bmp.extend_from_slice(&[0; 24]);
    bmp.extend_from_slice(&[0; 4]);
    assert_eq!(image_size(&bmp), Some((20_000, 20_000)));
    assert!(decode_pixmap(&bmp).is_none());
}

#[test]
fn rasters_are_capped_by_area_keeping_their_shape() {
    let (w, h, k) = fit_raster(15_000.0, 15_000.0);
    assert!(u64::from(w) * u64::from(h) <= MAX_PIXELS, "{w}×{h}");
    assert!(k < 1.0 && w == h);
    assert_eq!(fit_raster(612.0, 792.0), (612, 792, 1.0));
    let (w, h, _) = fit_raster(1e12, 10.0);
    assert!(u32::from(w) <= MAX_SIDE && h >= 1);
    assert_eq!(fit_raster(f64::NAN, f64::INFINITY), (1, 1, 1.0));
    // A page at a huge scale: drawn smaller, never over the area budget.
    let d = wordcraft_doc::Document::from_text("x");
    let l = wordcraft_layout::layout(&d, &mut wordcraft_layout::LayoutCache::new(), &wordcraft_layout::LayoutOptions::default());
    let img = render_page(&d, &l.pages[0], 30.0, &RenderOptions::default());
    assert!(u64::from(img.width) * u64::from(img.height) <= MAX_PIXELS);
    assert_eq!(img.pixels.len(), img.width as usize * img.height as usize * 4);
    let ratio = img.width as f64 / img.height as f64;
    assert!((ratio - 612.0 / 792.0).abs() < 0.01, "{ratio}");
}

/// Rendering on a thread keeps its rasterising context (and its worker threads) for the next
/// render, reset: whatever was drawn before, at whatever size, a kept context draws the pixels a
/// fresh one does (each fresh render below runs on a new thread, which has no context yet).
#[test]
fn a_kept_context_draws_what_a_fresh_one_draws() {
    let mut d = wordcraft_doc::Document::from_text(&"Some text to draw, and some more of it to fill the lines.\n".repeat(120));
    let t = wordcraft_doc::Table::new(3, 3, 300.0);
    d.body.insert(4, std::sync::Arc::new(wordcraft_doc::Block::Table(t)));
    let l = wordcraft_layout::layout(&d, &mut wordcraft_layout::LayoutCache::new(), &wordcraft_layout::LayoutOptions::default());
    assert!(l.pages.len() >= 2);
    let dark = RenderOptions { dark: true, ..Default::default() };
    let cases: Vec<(usize, f32, RenderOptions)> = vec![
        (0, 1.0, RenderOptions::default()),
        (1, 2.0, RenderOptions::default()),
        (0, 1.5, dark),
        (1, 0.5, RenderOptions::default()),
        (0, 1.0, RenderOptions::default()),
    ];
    let d = std::sync::Arc::new(d);
    let l = std::sync::Arc::new(l);
    let made = MADE.with(|m| m.get());
    for (page, scale, opts) in cases {
        let kept = render_page(&d, &l.pages[page], scale, &opts);
        let (d2, l2, o2) = (d.clone(), l.clone(), opts.clone());
        let fresh = std::thread::spawn(move || render_page(&d2, &l2.pages[page], scale, &o2)).join().unwrap();
        assert_eq!((kept.width, kept.height), (fresh.width, fresh.height));
        assert!(kept.pixels == fresh.pixels, "page {page} at {scale}: a kept context drew other pixels");
    }
    assert!(MADE.with(|m| m.get()) <= made + 1, "five renders on one thread made one context at most");
}
