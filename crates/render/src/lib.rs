//! WordCraft rasteriser: draws a page's display list with vello_cpu.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use vello_cpu::kurbo::{self, Affine, BezPath, Shape};
use vello_cpu::peniko;
use vello_cpu::{Pixmap, RenderContext, Resources};
use wordcraft_doc::Document;
use wordcraft_doc::para::ShapeKind;
use wordcraft_doc::props::Rgb;
use wordcraft_fonts::FontDb;
use wordcraft_layout::Page;
use wordcraft_layout::display::{DisplayOptions, Draw, Stroke, page_display};
use wordcraft_layout::wordart::ArtPaint;
use wordcraft_metafile::{Item, Picture, PlacedItem, Seg};

mod color_glyph;
pub mod icon_lib;

/// Worker threads for rasterising (0 on the web, where there are no threads).
pub fn default_threads() -> u16 {
    #[cfg(target_arch = "wasm32")]
    {
        0
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::thread::available_parallelism().map(|n| (n.get().saturating_sub(1)).clamp(1, 8) as u16).unwrap_or(2)
    }
}

/// Largest raster side vello_cpu handles comfortably.
pub const MAX_SIDE: u32 = 16_000;
/// Largest raster area (64 Mpx, 256 MB of RGBA): a raster that would be larger is drawn smaller,
/// keeping its shape, instead of allocating gigabytes for a hostile scale or page size.
pub const MAX_PIXELS: u64 = 64_000_000;

/// The raster size for a `w`×`h` pixel request and the factor (≤ 1) the drawing is scaled by to
/// fit [`MAX_SIDE`] and [`MAX_PIXELS`]. Non-finite or tiny sides become 1.
fn fit_raster(w: f64, h: f64) -> (u16, u16, f64) {
    let side = |v: f64| if v.is_finite() { v.max(1.0) } else { 1.0 };
    let (w, h) = (side(w), side(h));
    let max = MAX_SIDE as f64;
    let k = (max / w).min(max / h).min((MAX_PIXELS as f64 / (w * h)).sqrt()).min(1.0);
    // Shrunk sides round down so the area stays within the budget. Others round up, but a side
    // that is a whole number of pixels up to float noise (612 pt × 4/3, from an `f32` scale, is
    // 816.00002) stays that number: an extra pixel column made the page's screen rect wider than
    // its scale, so drags and hit tests drifted from the pointer.
    let px = |v: f64| (if k < 1.0 { (v * k).floor() } else { (v - 1e-3).ceil() }).clamp(1.0, max) as u16;
    (px(w), px(h), k)
}

/// A rendered image (premultiplied RGBA8, row-major).
#[derive(Clone, Default)]
pub struct Rendered {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Rendered {
    pub fn to_straight(&self) -> Vec<u8> {
        let mut out = self.pixels.clone();
        for px in out.as_chunks_mut::<4>().0 {
            let a = px[3] as u32;
            if a != 0 && a != 255 {
                for c in px.iter_mut().take(3) {
                    *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
        out
    }
    /// PNG bytes. An image that can't be encoded (no pixels, a size the encoder refuses) is an
    /// error, never an empty file reported as written.
    pub fn to_png(&self) -> Result<Vec<u8>, String> {
        if self.width == 0 || self.height == 0 {
            return Err(format!("can't make a PNG of an empty {}×{} image", self.width, self.height));
        }
        let len = (self.width as usize).checked_mul(self.height as usize).and_then(|n| n.checked_mul(4));
        let Some(len) = len else {
            return Err(format!("PNG encode: the {}×{} image is too large", self.width, self.height));
        };
        let mut px = self.to_straight();
        px.resize(len, 0);
        let Some(img) = image::RgbaImage::from_raw(self.width, self.height, px) else {
            return Err(format!("PNG encode: bad size {}×{}", self.width, self.height));
        };
        let mut buf = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png).map_err(|e| format!("PNG encode: {e}"))?;
        if buf.is_empty() {
            return Err("PNG encode: the encoder wrote nothing".into());
        }
        Ok(buf)
    }
    pub fn to_jpeg(&self, quality: u8) -> Vec<u8> {
        let rgba = self.to_straight();
        let rgb: Vec<u8> = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| {
                let a = p[3] as u32;
                let mix = |c: u8| ((c as u32 * a + 255 * (255 - a)) / 255) as u8;
                [mix(p[0]), mix(p[1]), mix(p[2])]
            })
            .collect();
        let mut buf = Vec::new();
        let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality.clamp(1, 100));
        let _ = image::ImageEncoder::write_image(enc, &rgb, self.width, self.height, image::ExtendedColorType::Rgb8);
        buf
    }
    /// Straight RGBA at (x, y) (transparent outside).
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        if x >= self.width || y >= self.height {
            return [0; 4];
        }
        let i = ((y as usize * self.width as usize) + x as usize) * 4;
        let Some(p) = self.pixels.get(i..i + 4) else { return [0; 4] };
        let a = p[3] as u32;
        if a == 0 {
            return [0; 4];
        }
        let un = |c: u8| ((c as u32 * 255 + a / 2) / a).min(255) as u8;
        [un(p[0]), un(p[1]), un(p[2]), p[3]]
    }
}

fn color(c: Rgb, alpha: f32) -> peniko::Color {
    peniko::Color::from_rgba8(c.0, c.1, c.2, (alpha.clamp(0.0, 1.0) * 255.0) as u8)
}

/// Upper bound on the bytes the two decoded-media caches keep together (about 96 MB), half each;
/// past its half a cache starts over.
const CACHE_BYTES: usize = 96 * 1024 * 1024 / 2;
/// Largest decoded picture kept (8 Mi pixels, 32 MB): more than a full page needs at 300 dpi. A
/// larger picture is scaled down once, keeping its shape, so it fits the cache and is not decoded
/// again at every render.
const IMAGE_PIXELS: u64 = 8 * 1024 * 1024;
/// Largest picture decoded at all (100 Mpx), checked from the file header before decoding.
const DECODE_PIXELS: u64 = 100_000_000;
/// Upper bound on cached entries per cache.
const CACHE_ENTRIES: usize = 256;

/// Decoded media by media key, shared across renders, with an approximate byte budget. An entry holds
/// the source bytes it was decoded from, so a hit needs the same allocation (`Arc::ptr_eq`), and that
/// allocation cannot be freed and its address reused while the entry lives.
struct MediaCache<T> {
    map: HashMap<String, Entry<T>>,
    bytes: usize,
}

struct Entry<T> {
    src: Arc<Vec<u8>>,
    val: T,
    size: usize,
}

impl<T: Clone> MediaCache<T> {
    fn get(&self, key: &str, src: &Arc<Vec<u8>>) -> Option<T> {
        let e = self.map.get(key)?;
        Arc::ptr_eq(&e.src, src).then(|| e.val.clone())
    }

    /// Stores `val` (about `size` bytes) with its source, whose bytes the entry keeps alive and so counts
    /// too. Clears the cache first when the budget or entry cap is hit; an entry larger than the whole
    /// budget is not kept.
    fn insert(&mut self, key: &str, src: &Arc<Vec<u8>>, val: T, size: usize) {
        let size = size.saturating_add(src.len());
        if size > CACHE_BYTES {
            return;
        }
        if let Some(old) = self.map.remove(key) {
            self.bytes = self.bytes.saturating_sub(old.size);
        }
        if self.map.len() >= CACHE_ENTRIES || self.bytes.saturating_add(size) > CACHE_BYTES {
            self.map.clear();
            self.bytes = 0;
        }
        self.map.insert(key.to_string(), Entry { src: src.clone(), val, size });
        self.bytes = self.bytes.saturating_add(size);
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn images() -> &'static Mutex<MediaCache<Option<Arc<Pixmap>>>> {
    static C: std::sync::OnceLock<Mutex<MediaCache<Option<Arc<Pixmap>>>>> = std::sync::OnceLock::new();
    C.get_or_init(|| Mutex::new(MediaCache { map: HashMap::new(), bytes: 0 }))
}

fn vectors() -> &'static Mutex<MediaCache<Option<Arc<Vector>>>> {
    static C: std::sync::OnceLock<Mutex<MediaCache<Option<Arc<Vector>>>>> = std::sync::OnceLock::new();
    C.get_or_init(|| Mutex::new(MediaCache { map: HashMap::new(), bytes: 0 }))
}

/// Largest side (pixels) a document picture may decode to: what a `Pixmap` can hold (`u16`
/// sides, which [`premul_pixmap`] enforces after decoding). Applying it as a decode limit rejects
/// a small file that declares a bigger image at header time, before anything is allocated.
const MAX_DECODE_SIDE: u32 = u16::MAX as u32;

/// Decode an encoded image to a premultiplied pixmap. The size is read from the header first:
/// pictures over 100 Mpx are not decoded, and pictures over [`IMAGE_PIXELS`] are scaled down.
pub fn decode_pixmap(bytes: &[u8]) -> Option<Pixmap> {
    decode_pixmap_within(bytes, IMAGE_PIXELS)
}

fn decode_pixmap_within(bytes: &[u8], budget: u64) -> Option<Pixmap> {
    let probe = wordcraft_geom::picture::probe(bytes).ok()?;
    let (w, h) = (probe.width, probe.height);
    let area = u64::from(w).checked_mul(u64::from(h))?;
    if area == 0 || area > DECODE_PIXELS {
        log::warn!("picture of {w}×{h} pixels is too large to draw");
        return None;
    }
    if w > MAX_DECODE_SIDE || h > MAX_DECODE_SIDE {
        return None;
    }
    let mut img = probe.decode(wordcraft_geom::MAX_DECODED_BYTES).ok()?;
    if area > budget.max(1) {
        // Scale down in the decoded colour type, before the RGBA copy is made.
        let k = (budget.max(1) as f64 / area as f64).sqrt();
        let fit = |v: u32| ((v as f64 * k).floor() as u32).max(1);
        img = img.thumbnail_exact(fit(w), fit(h));
    }
    let img = img.into_rgba8();
    let (w, h) = img.dimensions();
    premul_pixmap(w, h, img.as_raw())
}

/// A premultiplied pixmap from straight RGBA8 rows. None for empty, oversized or mis-sized input
/// (the pixmap constructor asserts the length).
fn premul_pixmap(w: u32, h: u32, rgba: &[u8]) -> Option<Pixmap> {
    if w == 0 || h == 0 || w > u16::MAX as u32 || h > u16::MAX as u32 {
        return None;
    }
    if Some(rgba.len()) != (w as usize).checked_mul(h as usize).and_then(|n| n.checked_mul(4)) {
        return None;
    }
    let data: Vec<vello_cpu::color::PremulRgba8> = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| {
            let a = p[3] as u16;
            let m = |c: u8| ((c as u16 * a + 127) / 255) as u8;
            vello_cpu::color::PremulRgba8 { r: m(p[0]), g: m(p[1]), b: m(p[2]), a: p[3] }
        })
        .collect();
    Some(Pixmap::from_parts(data, w as u16, h as u16))
}

/// Pixel size of an encoded image.
pub fn image_size(bytes: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?.into_dimensions().ok()
}

/// The natural size in points of a Windows Metafile or Enhanced Metafile picture, and the file
/// extension it is stored under (`"wmf"` or `"emf"`), or None for anything else (#802). Very large
/// frames are scaled down to 22 inches on the longer side, keeping the shape.
pub fn metafile_size(bytes: &[u8]) -> Option<(f32, f32, &'static str)> {
    if !wordcraft_metafile::is_metafile(bytes) {
        return None;
    }
    let pic = wordcraft_metafile::parse(bytes).ok()?;
    let (w, h) = (pic.width, pic.height);
    if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0) {
        return None;
    }
    let k = (1584.0 / w.max(h)).min(1.0);
    // An EMF starts with its header record, type 1.
    let ext = if bytes.get(..4) == Some(&[1, 0, 0, 0]) { "emf" } else { "wmf" };
    Some((w * k, h * k, ext))
}

/// A metafile (WMF/EMF) picture, parsed once. The geometry comes placed by the metafile crate; the
/// bitmaps are pixmaps, one per picture item (None for paths and undecodable bitmaps).
struct Vector {
    pic: Picture,
    pixmaps: Vec<Option<Arc<Pixmap>>>,
}

impl Vector {
    /// Approximate memory held: pixels and path segments.
    fn bytes(&self) -> usize {
        let px: usize = self.pixmaps.iter().flatten().map(|p| p.width() as usize * p.height() as usize * 4).sum();
        let segs: usize = self
            .pic
            .items
            .iter()
            .map(|it| match it {
                Item::Path { segs, .. } => segs.len() * std::mem::size_of::<Seg>(),
                Item::Bitmap { .. } => 0,
            })
            .sum();
        px + segs
    }
}

/// The metafile picture for a media key. None when the bytes are not a metafile or do not parse, so
/// the caller falls back to the raster path (and the grey box).
fn vector_for(doc: &Document, key: &str) -> Option<Arc<Vector>> {
    let bytes = doc.media.get(key)?;
    if !wordcraft_metafile::is_metafile(bytes) {
        return None;
    }
    if let Some(v) = lock(vectors()).get(key, bytes) {
        return v;
    }
    let v = build_vector(bytes).map(Arc::new);
    if v.is_none() {
        log::warn!("picture `{key}` is a metafile that could not be read");
    }
    let size = v.as_ref().map_or(0, |v| v.bytes());
    lock(vectors()).insert(key, bytes, v.clone(), size);
    v
}

/// Parses a metafile and decodes its bitmaps. None when the picture has no usable size.
fn build_vector(bytes: &[u8]) -> Option<Vector> {
    let mut pic = wordcraft_metafile::parse(bytes).ok()?;
    if !pic.has_size() {
        return None;
    }
    // The pixels move into the pixmaps; the picture keeps only the geometry.
    let pixmaps = pic.items.iter_mut().map(|it| it.take_pixels().and_then(|(w, h, px)| premul_pixmap(w, h, &px)).map(Arc::new)).collect();
    Some(Vector { pic, pixmaps })
}

fn rgba_color(c: [u8; 4]) -> peniko::Color {
    peniko::Color::from_rgba8(c[0], c[1], c[2], c[3])
}

/// Draws a metafile picture into `r` (page points): fills and strokes in page space (so strokes keep
/// their width under non-uniform scaling), bitmaps stretched, all clipped to `r` and faded by `alpha`.
/// Strokes have round caps and joins, as GDI draws them, and are at least one device pixel wide, so
/// cosmetic (width 0) pens show as hairlines at any zoom.
fn draw_vector(ctx: &mut RenderContext, v: &Vector, r: &kurbo::Rect, crop: &[f32; 4], alpha: f32, view: Affine) {
    ctx.set_transform(view);
    let alpha = if alpha.is_finite() { alpha.clamp(0.0, 1.0) } else { 1.0 };
    let faded = alpha < 1.0;
    if faded {
        ctx.push_opacity_layer(alpha);
    }
    ctx.push_clip_layer(&r.to_path(0.1));
    let pixel = device_pixel(view);
    for it in v.pic.place(*r, *crop) {
        match it {
            PlacedItem::Path { path, fill, stroke, even_odd } => {
                if let Some(f) = fill {
                    ctx.set_paint(rgba_color(f));
                    ctx.set_fill_rule(if even_odd { peniko::Fill::EvenOdd } else { peniko::Fill::NonZero });
                    ctx.fill_path(&path);
                }
                if let Some((s, w)) = stroke {
                    ctx.set_paint(rgba_color(s));
                    ctx.set_stroke(kurbo::Stroke::new(w.max(pixel)).with_caps(kurbo::Cap::Round).with_join(kurbo::Join::Round));
                    ctx.stroke_path(&path);
                }
            }
            PlacedItem::Bitmap { index, rect } => {
                let Some(Some(pm)) = v.pixmaps.get(index) else { continue };
                let (pw, ph) = (pm.width().max(1) as f64, pm.height().max(1) as f64);
                ctx.set_paint(vello_cpu::Image {
                    image: vello_cpu::ImageSource::Pixmap(pm.clone()),
                    sampler: peniko::ImageSampler::default().with_quality(peniko::ImageQuality::Medium),
                });
                ctx.set_paint_transform(Affine::translate((rect.x0, rect.y0)) * Affine::scale_non_uniform(rect.width() / pw, rect.height() / ph));
                ctx.fill_rect(&rect);
                ctx.reset_paint_transform();
            }
        }
    }
    ctx.set_fill_rule(peniko::Fill::NonZero);
    ctx.pop_layer();
    if faded {
        ctx.pop_layer();
    }
}

/// One device pixel in page points under `view` (its mean axis scale); 1 for a degenerate transform.
fn device_pixel(view: Affine) -> f64 {
    let c = view.as_coeffs();
    let scale = (c[0].hypot(c[1]) + c[2].hypot(c[3])) / 2.0;
    if scale.is_finite() && scale > 1e-9 { 1.0 / scale } else { 1.0 }
}

/// Largest page-background picture decoded, in pixels (hostile files).
const MAX_BACKGROUND_PIXELS: u64 = 64 * 1024 * 1024;

/// Draw the page's picture or texture fill (Design › Page Color › Fill Effects) behind the content:
/// tiled at its natural size (96 dpi) for a `tile` fill, stretched over the page otherwise.
fn draw_page_picture(ctx: &mut RenderContext, doc: &Document, page: &Page, view: Affine) {
    let Some(pic) = &doc.settings.page_picture else { return };
    let Some(bytes) = doc.media.get(&pic.media) else { return };
    match image_size(bytes) {
        Some((w, h)) if w > 0 && h > 0 && (w as u64) * (h as u64) <= MAX_BACKGROUND_PIXELS => {}
        _ => return,
    }
    let Some(pm) = pixmap_for(doc, &pic.media) else { return };
    let (pw, ph) = (pm.width().max(1) as f64, pm.height().max(1) as f64);
    let (w, h) = (page.w as f64, (page.h as f64).min(1e6));
    if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0) {
        return;
    }
    let tile = pic.fill_type.eq_ignore_ascii_case("tile");
    let (sx, sy) = if tile { (0.75, 0.75) } else { (w / pw, h / ph) };
    let extend = if tile { peniko::Extend::Repeat } else { peniko::Extend::Pad };
    ctx.set_transform(view);
    ctx.set_paint(vello_cpu::Image {
        image: vello_cpu::ImageSource::Pixmap(pm),
        sampler: peniko::ImageSampler::default().with_quality(peniko::ImageQuality::Medium).with_extend(extend),
    });
    ctx.set_paint_transform(Affine::scale_non_uniform(sx, sy));
    ctx.fill_rect(&kurbo::Rect::new(0.0, 0.0, w, h));
    ctx.reset_paint_transform();
}

fn pixmap_for(doc: &Document, key: &str) -> Option<Arc<Pixmap>> {
    let bytes = doc.media.get(key)?;
    if let Some(v) = lock(images()).get(key, bytes) {
        return v;
    }
    let pm = decode_pixmap(bytes).map(Arc::new);
    let size = pm.as_ref().map_or(0, |p| (p.width() as usize).saturating_mul(p.height() as usize).saturating_mul(4));
    lock(images()).insert(key, bytes, pm.clone(), size);
    pm
}

/// Options for a page raster.
#[derive(Clone, Debug)]
pub struct RenderOptions {
    pub display: DisplayOptions,
    /// Paper colour (page colour from Design › Page Color, else white).
    pub paper: Rgb,
    /// Colour for formatting marks.
    pub mark_color: Rgb,
    /// Thicken glyphs slightly the way macOS draws text on screen ("font smoothing"), so pages
    /// look like they do in other Mac apps. For on-screen rasters only: exports stay exact.
    pub text_darkening: bool,
    /// Dark page (View › Switch Modes): white paper turns [`RenderOptions::dark_paper`] grey and
    /// automatic (black) text white; highlights, shading and page colours become dark versions of
    /// their hue, and text colours keep their hue but are lightened until they read at 4.5:1
    /// against what is behind them. Pictures are untouched. Screen only: print, export and PDF
    /// never set it.
    pub dark: bool,
    /// The grey that black becomes on a dark page (white paper turns this, not pure black).
    pub dark_paper: u8,
    /// Worker threads that rasterise (`None`: [`default_threads`]). Each worker costs CPU for
    /// coordination: a page drawn after every keystroke takes about 2.5 times the CPU time on
    /// eight workers that it takes on none, for half the wait.
    pub threads: Option<u16>,
}

/// Default grey for a dark page's paper.
pub const DARK_PAPER: u8 = 0x33;

impl RenderOptions {
    /// The on-screen colour for a document colour (see [`RenderOptions::dark`]).
    pub fn ink(&self, c: Rgb) -> Rgb {
        if !self.dark {
            return c;
        }
        // Invert, then lift the blacks to the paper grey so the page isn't a black hole.
        let Rgb(r, g, b) = invert_lightness(c);
        let p = self.dark_paper as u32;
        let lift = |v: u8| (p + v as u32 * (255 - p) / 255).min(255) as u8;
        Rgb(lift(r), lift(g), lift(b))
    }
}

/// The WCAG contrast a dark page's text keeps against what is behind it.
pub const DARK_TEXT_CONTRAST: f64 = 4.5;
/// The WCAG contrast a dark page's thin strokes (underlines, borders, rules) keep against the paper.
const DARK_LINE_CONTRAST: f64 = 3.0;
/// The lightest a dark page's background (highlight, shading, page colour) gets, as WCAG relative
/// luminance: white text on it reads at 7:1.
const DARK_BACKGROUND_LUMINANCE: f64 = 0.10;

impl RenderOptions {
    /// The on-screen colour for a background in the document: a highlight, shading, a filled
    /// shape or the page colour. On a dark page it is a dark, low-lightness version of the same
    /// hue (white paper becomes the paper grey), so light text reads on it: inverting instead
    /// left bright hues such as yellow bright under white text (#1252); yellow turns a dim olive.
    pub fn background(&self, c: Rgb) -> Rgb {
        if !self.dark {
            return c;
        }
        let g = self.ink(c);
        if relative_luminance(g) <= DARK_BACKGROUND_LUMINANCE {
            return g;
        }
        let (h, s, l) = to_hsl(g);
        // Luminance grows with lightness at a fixed hue and saturation: find the lightest that fits.
        let (mut lo, mut hi) = (0.0, l);
        for _ in 0..24 {
            let mid = (lo + hi) / 2.0;
            if relative_luminance(from_hsl(h, s, mid)) <= DARK_BACKGROUND_LUMINANCE {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        from_hsl(h, s, lo)
    }

    /// The on-screen paper: the document's page colour (Design › Page Color) or the plain paper,
    /// as a [`RenderOptions::background`].
    pub fn page_background(&self, page_color: Option<Rgb>) -> Rgb {
        self.background(page_color.unwrap_or(self.paper))
    }

    /// The on-screen colour for text (or its underline) in `c`, on the on-screen `paper` or, when
    /// it sits on one, the on-screen `fill` behind it (a highlight, shading or filled shape, from
    /// [`RenderOptions::background`]). On a dark page the colour's lightness is inverted keeping
    /// its hue (black text turns white); on a fill it takes the lighter of that and its own
    /// colour, so white text on a dark highlight stays white. Then its lightness moves away from
    /// what is behind it until the two reach [`DARK_TEXT_CONTRAST`].
    pub fn text(&self, c: Rgb, paper: Rgb, fill: Option<Rgb>) -> Rgb {
        if !self.dark {
            return c;
        }
        let inked = self.ink(c);
        match fill {
            Some(f) => {
                let start = if relative_luminance(c) > relative_luminance(inked) { c } else { inked };
                self.readable_from(start, f, DARK_TEXT_CONTRAST)
            }
            None => self.readable_from(inked, paper, DARK_TEXT_CONTRAST),
        }
    }

    /// A thin stroke (rule, border, underline off any fill) in `c` over `behind`: as
    /// [`RenderOptions::text`] on the paper, at [`DARK_LINE_CONTRAST`].
    fn stroke(&self, c: Rgb, behind: Rgb) -> Rgb {
        if !self.dark {
            return c;
        }
        self.readable_from(self.ink(c), behind, DARK_LINE_CONTRAST)
    }

    /// `g` (already on-screen), with its lightness moved the least that reaches `target` against
    /// `behind`, keeping hue and saturation.
    fn readable_from(&self, g: Rgb, behind: Rgb, target: f64) -> Rgb {
        if contrast_ratio(g, behind) >= target {
            return g;
        }
        let (h, s, l) = to_hsl(g);
        let lighten = relative_luminance(behind) < 0.18;
        let end = if lighten { 1.0 } else { 0.0 };
        if contrast_ratio(from_hsl(h, s, end), behind) < target {
            return from_hsl(h, s, end);
        }
        // The smallest move from `l` toward `end` that reaches the target.
        let (mut near, mut far) = (l, end);
        for _ in 0..24 {
            let mid = (near + far) / 2.0;
            if contrast_ratio(from_hsl(h, s, mid), behind) >= target {
                far = mid;
            } else {
                near = mid;
            }
        }
        from_hsl(h, s, far)
    }
}

/// WCAG 2 relative luminance of an sRGB colour (0 for black, 1 for white).
pub fn relative_luminance(c: Rgb) -> f64 {
    let lin = |v: u8| {
        let v = v as f64 / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin(c.0) + 0.7152 * lin(c.1) + 0.0722 * lin(c.2)
}

/// WCAG 2 contrast ratio of two colours, 1 to 21.
pub fn contrast_ratio(a: Rgb, b: Rgb) -> f64 {
    let (x, y) = (relative_luminance(a), relative_luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// HSL of a colour: hue in turns (0..1), saturation and lightness 0..1.
fn to_hsl(c: Rgb) -> (f64, f64, f64) {
    let (r, g, b) = (c.0 as f64 / 255.0, c.1 as f64 / 255.0, c.2 as f64 / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.0;
    let d = max - min;
    if d <= 0.0 {
        return (0.0, 0.0, l);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs()).max(1e-9);
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h / 6.0, s.clamp(0.0, 1.0), l)
}

/// The colour with HSL `h` (in turns), `s`, `l`.
fn from_hsl(h: f64, s: f64, l: f64) -> Rgb {
    let l = l.clamp(0.0, 1.0);
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s.clamp(0.0, 1.0);
    let hp = h.rem_euclid(1.0) * 6.0;
    let x = c * (1.0 - (hp.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    let q = |v: f64| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    Rgb(q(r), q(g), q(b))
}

/// What lies behind a point of a page, on screen: the paper (page colour), then the backgrounds
/// (highlights, shading, filled shapes) in drawing order, in page points. Text colours on a dark
/// page are worked out against it, and the caret takes its colour from it (#299). Text,
/// pictures and turned or rotated content are left out.
struct Behind {
    page: Rgb,
    fills: Vec<Backing>,
}

/// One background: its area (clipped), on-screen colour and opacity, and for a shape its outline
/// (an ellipse's corners are not inside it).
struct Backing {
    rect: kurbo::Rect,
    color: Rgb,
    alpha: f32,
    shape: Option<(ShapeKind, Option<u32>)>,
}

impl Behind {
    /// The backgrounds of `items` over the on-screen `page`.
    fn new(items: &[Draw], page: Rgb, opts: &RenderOptions) -> Behind {
        let mut b = Behind { page, fills: Vec::new() };
        b.collect(items, None, opts, 0);
        b
    }

    fn collect(&mut self, items: &[Draw], clip: Option<kurbo::Rect>, opts: &RenderOptions, depth: u32) {
        if depth > 16 {
            return;
        }
        let clipped = |r: kurbo::Rect| clip.map_or(r, |c| r.intersect(c));
        for it in items {
            match it {
                Draw::Figure { draws, .. } => self.collect(draws, clip, opts, depth + 1),
                Draw::Clip { rect, items } => self.collect(items, Some(clipped(page_rect(rect))), opts, depth + 1),
                // A highlight or shading is always a background, however narrow (a highlighted
                // space); another fill only when it isn't a rule.
                Draw::Fill { rect, color, alpha, text_bg } if *text_bg || !is_thin(rect) => {
                    let alpha = if alpha.is_finite() { alpha.clamp(0.0, 1.0) } else { 1.0 };
                    self.fills.push(Backing { rect: clipped(page_rect(rect)), color: opts.background(*color), alpha, shape: None });
                }
                // Shape fills are drawn opaque (the model has no fill transparency).
                Draw::Shape { rect, kind, fill: Some(f), corner, .. } if *kind != ShapeKind::Line => {
                    let shape = Some((*kind, *corner));
                    self.fills.push(Backing { rect: clipped(page_rect(rect)), color: opts.background(*f), alpha: 1.0, shape });
                }
                _ => {}
            }
        }
    }

    /// The on-screen colour of the backgrounds over page point (`x`, `y`), blended in drawing
    /// order over the paper; `None` where there are none.
    fn fill_at(&self, x: f64, y: f64) -> Option<Rgb> {
        let p = kurbo::Point::new(x, y);
        let mut bg: Option<[f32; 3]> = None;
        for f in &self.fills {
            let inside = f.rect.contains(p) && f.shape.is_none_or(|(kind, corner)| kurbo::Shape::contains(&shape_path(kind, f.rect, corner), p));
            if !inside {
                continue;
            }
            let under = bg.get_or_insert([self.page.0 as f32, self.page.1 as f32, self.page.2 as f32]);
            for (v, n) in under.iter_mut().zip([f.color.0, f.color.1, f.color.2]) {
                *v += (n as f32 - *v) * f.alpha;
            }
        }
        let c = |v: f32| v.round().clamp(0.0, 255.0) as u8;
        bg.map(|b| Rgb(c(b[0]), c(b[1]), c(b[2])))
    }

    /// The on-screen colour behind page point (`x`, `y`): the backgrounds there, else the paper.
    fn at(&self, x: f64, y: f64) -> Rgb {
        self.fill_at(x, y).unwrap_or(self.page)
    }
}

fn page_rect(r: &wordcraft_geom::Rect) -> kurbo::Rect {
    kurbo::Rect::new(r.x as f64, r.y as f64, r.right() as f64, r.bottom() as f64)
}

/// A fill under 4 points tall (a rule, a fraction bar) or 1.5 points wide (a vertical rule) is a
/// stroke, not a background. Narrow but tall fills stay backgrounds: a highlighted space is one.
fn is_thin(r: &wordcraft_geom::Rect) -> bool {
    r.h.abs() < 4.0 || r.w.abs() < 1.5
}

/// Invert HSL lightness, keeping hue and saturation: shifting every channel by
/// `255 - max - min` maps the lightest channel to `255 - min` and the darkest to `255 - max`.
pub fn invert_lightness(c: Rgb) -> Rgb {
    let (r, g, b) = (c.0 as i32, c.1 as i32, c.2 as i32);
    let d = 255 - r.max(g).max(b) - r.min(g).min(b);
    let f = |v: i32| (v + d).clamp(0, 255) as u8;
    Rgb(f(r), f(g), f(b))
}

impl Default for RenderOptions {
    fn default() -> Self {
        RenderOptions {
            display: DisplayOptions::default(),
            paper: Rgb::WHITE,
            mark_color: Rgb(0x2B, 0x57, 0x9A),
            text_darkening: false,
            dark: false,
            dark_paper: DARK_PAPER,
            threads: None,
        }
    }
}

/// Render a whole page at `scale` pixels per point.
pub fn render_page(doc: &Document, page: &Page, scale: f32, opts: &RenderOptions) -> Rendered {
    let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let (w, h, k) = fit_raster(page.w as f64 * scale as f64, page.h.min(1e6) as f64 * scale as f64);
    render_region(doc, page, w as u32, h as u32, Affine::scale(scale as f64 * k), opts)
}

/// Render a page into a `w`×`h` raster with `view` (page points → pixels). A raster over
/// [`MAX_SIDE`] or [`MAX_PIXELS`] comes back smaller, with the drawing scaled to match.
pub fn render_region(doc: &Document, page: &Page, w: u32, h: u32, view: Affine, opts: &RenderOptions) -> Rendered {
    let (w16, h16, k) = fit_raster(w as f64, h as f64);
    let view = if k < 1.0 { Affine::scale(k) * view } else { view };
    let items = page_display(doc, page, &opts.display);
    let pixels = with_context(w16, h16, opts.threads.unwrap_or_else(default_threads), |ctx| {
        ctx.set_transform(Affine::IDENTITY);
        let paper = opts.page_background(doc.settings.page_color);
        ctx.set_paint(color(paper, 1.0));
        ctx.fill_rect(&kurbo::Rect::new(0.0, 0.0, w16 as f64, h16 as f64));
        draw_page_picture(ctx, doc, page, view);
        // Only a dark page changes colours by what is behind them.
        let behind = if opts.dark { Behind::new(&items, paper, opts) } else { Behind { page: paper, fills: Vec::new() } };
        let visible = view.inverse().transform_rect_bbox(kurbo::Rect::new(0.0, 0.0, w16 as f64, h16 as f64)).inflate(40.0, 40.0);
        if let Some(wm) = &doc.settings.watermark {
            draw_watermark(ctx, view, page, wm, opts);
        }
        for it in &items {
            draw(ctx, doc, it, view, &visible, opts, &behind);
        }
        ctx.flush();
        let mut pixels = vec![0u8; raster_bytes(w16, h16)];
        let mut res = Resources::new();
        if let Some(pm) = vello_cpu::PixmapMut::new(w16, h16, &mut pixels) {
            ctx.render(pm, &mut res);
        }
        pixels
    });
    Rendered { width: w16 as u32, height: h16 as u32, pixels }
}

/// Largest raster whose rasterising context is kept for the next render ([`with_context`]):
/// a page at 200% on a high-density screen (about 8 Mpx) and then some.
const KEEP_PIXELS: u64 = 16_000_000;

thread_local! {
    /// The rasterising context the last render on this thread used, and its worker threads.
    static CONTEXT: std::cell::RefCell<Option<(u16, RenderContext)>> = const { std::cell::RefCell::new(None) };
    /// Rasterising contexts made on this thread (tests).
    #[cfg(test)]
    static MADE: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Run `f` with a `w`×`h` rasterising context of `threads` worker threads, reset from the one
/// the last render on this thread used when it had as many threads. A fresh context starts a
/// pool of worker threads and allocates their buffers, which cost more than drawing a typical
/// page; reset, it draws exactly what a fresh one draws. Contexts for rasters over
/// [`KEEP_PIXELS`] aren't kept, so their buffers go with them.
fn with_context<R>(w: u16, h: u16, threads: u16, f: impl FnOnce(&mut RenderContext) -> R) -> R {
    let keep = u64::from(w) * u64::from(h) <= KEEP_PIXELS;
    // Taken out while in use: a render inside a render makes its own.
    let kept = if keep { CONTEXT.try_with(|c| c.try_borrow_mut().ok().and_then(|mut k| k.take())).ok().flatten() } else { None };
    let mut ctx = match kept {
        Some((t, mut ctx)) if t == threads => {
            ctx.reset_and_resize(w, h);
            ctx
        }
        _ => {
            #[cfg(test)]
            MADE.with(|m| m.set(m.get() + 1));
            RenderContext::new_with(w, h, vello_cpu::RenderSettings { num_threads: threads, ..Default::default() })
        }
    };
    let r = f(&mut ctx);
    if keep {
        let _ = CONTEXT.try_with(|c| {
            if let Ok(mut k) = c.try_borrow_mut() {
                *k = Some((threads, ctx));
            }
        });
    }
    r
}

/// Bytes of a `w`×`h` RGBA raster (sides from [`fit_raster`], so within [`MAX_PIXELS`]).
fn raster_bytes(w: u16, h: u16) -> usize {
    (w as usize).saturating_mul(h as usize).saturating_mul(4)
}

/// The on-screen colour behind page point (`x`, `y`): the paper (page colour), then any
/// shading, highlight or filled shape drawn over that point, blended in drawing order, all as
/// the page shows them (dark on a dark page). The caret takes its colour from this so it shows
/// against whatever lies behind it (#299); it is the same lookup text colours use.
pub fn background_at(doc: &Document, page: &Page, opts: &RenderOptions, x: f32, y: f32) -> Rgb {
    let paper = opts.page_background(doc.settings.page_color);
    if !(x.is_finite() && y.is_finite()) {
        return paper;
    }
    let items = page_display(doc, page, &opts.display);
    Behind::new(&items, paper, opts).at(x as f64, y as f64)
}

/// One line of `text` in `face` at `size` points, black on white, in a `w`×`h` point raster at
/// `scale` pixels per point, starting `margin` points in with the capitals centred in `h`: a font
/// list entry. Not the face's ascent: a font with a tall one (Amiri Quran's is 1.8 em) put the
/// name below the row (#2570). Outlines come straight from the face, not the shared outline
/// cache, so a face read just for the preview leaves nothing behind once dropped (#848).
pub fn render_text_line(face: &wordcraft_fonts::FontFace, text: &str, size: f32, w: f32, h: f32, margin: f32, scale: f32) -> Rendered {
    let fin = |v: f32, d: f32| if v.is_finite() && v >= 0.0 { v } else { d };
    let (size, w, h, margin) = (fin(size, 12.0).min(1000.0) as f64, fin(w, 1.0), fin(h, 1.0), fin(margin, 0.0).min(1000.0) as f64);
    let scale = if scale.is_finite() && scale > 0.0 { scale.min(16.0) } else { 1.0 };
    let (w16, h16, k) = fit_raster(w as f64 * scale as f64, h as f64 * scale as f64);
    let scale = scale * k as f32;
    let mut ctx = RenderContext::new_with(w16, h16, vello_cpu::RenderSettings { num_threads: 0, ..Default::default() });
    ctx.set_transform(Affine::IDENTITY);
    ctx.set_paint(color(Rgb(255, 255, 255), 1.0));
    ctx.fill_rect(&kurbo::Rect::new(0.0, 0.0, w16 as f64, h16 as f64));
    ctx.set_paint(color(Rgb(0, 0, 0), 1.0));
    let upem = if face.upem.is_finite() && face.upem > 0.0 { face.upem } else { 1000.0 };
    let k = size / upem;
    let view = Affine::scale(scale as f64);
    let cap = if face.cap_height.is_finite() { face.cap_height.clamp(upem * 0.3, upem) } else { upem * 0.7 };
    let baseline = (h as f64 + cap * k) / 2.0;
    let mut x = margin;
    let colour = face.has_color_glyphs();
    for g in wordcraft_fonts::shape(face, text, &[], |c| c) {
        let at = view * Affine::translate((x + g.x_offset as f64 * k, baseline - g.y_offset as f64 * k)) * Affine::scale(k);
        if colour && let Some(d) = color_glyph::drawable(face, g.gid, size * scale as f64, false) {
            color_glyph::draw(&mut ctx, &d, at, upem, color(Rgb(0, 0, 0), 1.0), 1.0);
            ctx.set_paint(color(Rgb(0, 0, 0), 1.0));
        } else {
            ctx.set_transform(at);
            ctx.fill_path(&face.glyph_outline(g.gid));
        }
        x += g.x_advance as f64 * k;
        if x > w as f64 {
            break;
        }
    }
    ctx.flush();
    let mut pixels = vec![0u8; raster_bytes(w16, h16)];
    let mut res = Resources::new();
    if let Some(pm) = vello_cpu::PixmapMut::new(w16, h16, &mut pixels) {
        ctx.render(pm, &mut res);
    }
    Rendered { width: w16 as u32, height: h16 as u32, pixels }
}

fn draw_watermark(ctx: &mut RenderContext, view: Affine, page: &Page, wm: &wordcraft_doc::Watermark, opts: &RenderOptions) {
    let r = wordcraft_fonts::word::resolve(&wm.font, false, false);
    let face = r.face;
    let glyphs = wordcraft_fonts::shape(&face, &wm.text, &[], |c| c);
    let upem = face.upem.max(1.0);
    let raw_w: f64 = glyphs.iter().map(|g| g.x_advance as f64).sum::<f64>() / upem;
    if raw_w <= 0.0 {
        return;
    }
    // The document page, not a Print Markup area beside it.
    let pw = (page.w - page.markup.max(0.0)).max(1.0) as f64;
    let diag = if wm.diagonal { (pw.powi(2) + (page.h.min(2000.0) as f64).powi(2)).sqrt() } else { pw };
    let size = (diag * 0.5 / raw_w).min(200.0);
    let k = size / upem;
    let angle = if wm.diagonal { -((page.h.min(2000.0) as f64) / pw).atan() } else { 0.0 };
    let base = Affine::translate((pw / 2.0, page.h.min(2000.0) as f64 / 2.0))
        * Affine::rotate(angle)
        * Affine::translate((-raw_w * size / 2.0, size * 0.35));
    ctx.set_paint(color(opts.ink(wm.color), if wm.semitransparent { 0.35 } else { 0.8 }));
    let db = FontDb::global();
    let mut x = 0.0;
    for g in &glyphs {
        let o = db.outline(&face, g.gid);
        ctx.set_transform(view * base * Affine::translate((x, 0.0)) * Affine::scale(k));
        ctx.fill_path(&o);
        x += g.x_advance as f64 * k;
    }
}

/// How far, in device pixels, macOS-style font smoothing pushes each side of a glyph outline out
/// at `ppem` device pixels per em. Values from Pathfinder (MIT/Apache-2.0), which measured them
/// against macOS: 1.21% of the pixel size horizontally and 1.25 times that vertically, each
/// capped at 0.3 px, and no darkening past 72 px per em. We stroke the outline, which grows it
/// evenly, so this is the mean of the two.
pub fn stem_darkening(ppem: f64) -> f64 {
    const FACTOR: [f64; 2] = [0.0121, 0.0121 * 1.25];
    const MAX_PX: f64 = 0.3;
    const MAX_PPEM: f64 = 72.0;
    if !ppem.is_finite() || ppem <= 0.0 || ppem > MAX_PPEM {
        return 0.0;
    }
    FACTOR.iter().map(|f| (ppem * f).min(MAX_PX)).sum::<f64>() / 2.0
}

/// A wavy line from `(x0, y0)` to `(x1, y1)`; at most 20 000 units long, so a huge extent can't make it endless.
fn wave_path(x0: f64, y0: f64, x1: f64, y1: f64, amp: f64) -> BezPath {
    let mut p = BezPath::new();
    let (dx, dy) = (x1 - x0, y1 - y0);
    let len = (dx * dx + dy * dy).sqrt();
    if !len.is_finite() || len < 1e-3 {
        return p;
    }
    let (ux, uy) = (dx / len, dy / len);
    let (nx, ny) = (-uy, ux);
    let step = amp * 2.0;
    p.move_to((x0, y0));
    // Past about 1e16 a step no longer changes coordinates: stop instead of pushing curves forever.
    if x0 + ux * step == x0 && y0 + uy * step == y0 {
        return p;
    }
    let mut t = 0.0;
    let mut up = true;
    while t < len && t < 20_000.0 {
        let nt = (t + step).min(len);
        if nt <= t {
            break;
        }
        let mid_t = (t + nt) / 2.0;
        let sign = if up { -1.0 } else { 1.0 };
        let cx = x0 + ux * mid_t + nx * amp * sign;
        let cy = y0 + uy * mid_t + ny * amp * sign;
        let ex = x0 + ux * nt;
        let ey = y0 + uy * nt;
        p.quad_to((cx, cy), (ex, ey));
        t = nt;
        up = !up;
    }
    p
}

fn draw(ctx: &mut RenderContext, doc: &Document, it: &Draw, view: Affine, visible: &kurbo::Rect, opts: &RenderOptions, behind: &Behind) {
    match it {
        Draw::Figure { draws, .. } => {
            for d in draws {
                draw(ctx, doc, d, view, visible, opts, behind);
            }
        }
        Draw::Fill { rect, color: c, alpha, text_bg } => {
            let r = kurbo::Rect::new(rect.x as f64, rect.y as f64, rect.right() as f64, rect.bottom() as f64);
            if !r.overlaps(*visible) {
                return;
            }
            ctx.set_transform(view);
            let c = if *text_bg || !is_thin(rect) { opts.background(*c) } else { opts.stroke(*c, behind.at(r.center().x, r.center().y)) };
            ctx.set_paint(color(c, *alpha));
            ctx.fill_rect(&r);
        }
        Draw::Line { x0, y0, x1, y1, width, color: c, stroke, alpha, text_bg } => {
            ctx.set_transform(view);
            // An underline or strike-through on a highlight reads like its text; other lines
            // keep 3:1 against what they cross.
            let ink = match text_bg {
                Some(bg) => opts.text(*c, behind.page, Some(opts.background(*bg))),
                None if opts.dark => opts.stroke(*c, behind.at(f64::from((*x0 + *x1) / 2.0), f64::from((*y0 + *y1) / 2.0))),
                None => *c,
            };
            ctx.set_paint(color(ink, *alpha));
            let w = *width as f64;
            let mut st = kurbo::Stroke::new(w);
            match stroke {
                Stroke::Dotted => st = st.with_dashes(0.0, [w, w * 2.0]),
                Stroke::Dashed => st = st.with_dashes(0.0, [w * 4.0, w * 2.0]),
                _ => {}
            }
            let (x0, y0, x1, y1) = (*x0 as f64, *y0 as f64, *x1 as f64, *y1 as f64);
            let line = |ctx: &mut RenderContext, ox: f64, oy: f64| {
                let mut p = BezPath::new();
                p.move_to((x0 + ox, y0 + oy));
                p.line_to((x1 + ox, y1 + oy));
                ctx.stroke_path(&p);
            };
            if *stroke == Stroke::Wave {
                let amp = (w * 1.2).max(0.8);
                let p = wave_path(x0, y0, x1, y1, amp);
                ctx.set_stroke(kurbo::Stroke::new(w * 0.8));
                ctx.stroke_path(&p);
                return;
            }
            if *stroke == Stroke::Double {
                let (dx, dy) = (x1 - x0, y1 - y0);
                let len = (dx * dx + dy * dy).sqrt().max(1e-3);
                let (nx, ny) = (-dy / len * w * 0.9, dx / len * w * 0.9);
                ctx.set_stroke(kurbo::Stroke::new(w * 0.6));
                line(ctx, nx, ny);
                line(ctx, -nx, -ny);
                return;
            }
            ctx.set_stroke(st);
            line(ctx, 0.0, 0.0);
        }
        Draw::Glyphs { face, size, glyphs, color: c, alpha, synth_bold, synth_italic, text_bg, .. } => {
            // Invisible glyphs (WordArt letters are drawn from their outlines) carry text only.
            if *alpha <= 0.0 {
                return;
            }
            let db = FontDb::global();
            let k = *size as f64 / face.upem.max(1.0);
            // What lies behind the run: its own highlight or shading, else whatever the page has
            // there (paragraph or cell shading, a filled shape), else the paper.
            let fill = match (text_bg, glyphs.first()) {
                _ if !opts.dark => None,
                (Some(bg), _) => Some(opts.background(*bg)),
                (None, Some((_, x, y))) => behind.fill_at(f64::from(*x) + 0.5, f64::from(*y) - f64::from(*size) * 0.3),
                (None, None) => None,
            };
            let ink = opts.text(*c, behind.page, fill);
            ctx.set_paint(color(ink, *alpha));
            let skew = if *synth_italic { Affine::new([1.0, 0.0, -0.21, 1.0, 0.0, 0.0]) } else { Affine::IDENTITY };
            // Outline strokes, in font units: synthetic bold, then on-screen stem darkening (a
            // stroke grows the outline by half its width on each side). Darkening is skipped for
            // translucent text, where the stroke overlapping the fill would show as a darker rim.
            let px_per_pt = view.determinant().abs().sqrt();
            let darken_px = if opts.text_darkening && *alpha >= 1.0 { stem_darkening(*size as f64 * px_per_pt) } else { 0.0 };
            let darken = if darken_px > 0.0 && k * px_per_pt > 0.0 { 2.0 * darken_px / (k * px_per_pt) } else { 0.0 };
            let outline_stroke = if *synth_bold { face.upem * 0.03 } else { 0.0 } + darken;
            if outline_stroke > 0.0 {
                let join = if darken > 0.0 { kurbo::Join::Round } else { kurbo::Join::Miter };
                ctx.set_stroke(kurbo::Stroke::new(outline_stroke).with_join(join));
            }
            // Emoji and other colour glyphs (#1269) are drawn from the font's colour data.
            let colour = face.has_color_glyphs();
            let ppem = *size as f64 * px_per_pt;
            for (gid, x, y) in glyphs {
                let (gx, gy) = (*x as f64, *y as f64);
                // Written so a NaN position is skipped too.
                let near = gx >= visible.x0 - 100.0 && gx <= visible.x1 && gy >= visible.y0 && gy <= visible.y1 + 200.0;
                if !near {
                    continue;
                }
                if colour && let Some(d) = color_glyph::drawable(face, *gid, ppem, true) {
                    let at = view * Affine::translate((gx, gy)) * skew * Affine::scale(k);
                    color_glyph::draw(ctx, &d, at, face.upem, color(ink, 1.0), *alpha);
                    ctx.set_paint(color(ink, *alpha));
                    continue;
                }
                let o = db.outline(face, *gid);
                if o.elements().is_empty() {
                    continue;
                }
                ctx.set_transform(view * Affine::translate((gx, gy)) * skew * Affine::scale(k));
                ctx.fill_path(&o);
                if outline_stroke > 0.0 {
                    ctx.stroke_path(&o);
                }
            }
        }
        Draw::Mark { x, baseline, size, ch, color: mark } => {
            let face = wordcraft_fonts::word::resolve("Source Sans 3", false, false).face;
            let gid = face.glyph_for(*ch);
            let face = if gid == 0 {
                match FontDb::global().fallback_for(*ch, face.id()) {
                    Some(f) => wordcraft_fonts::FaceRef::of(&f),
                    None => return,
                }
            } else {
                face
            };
            let gid = face.glyph_for(*ch);
            let o = FontDb::global().outline(&face, gid);
            let k = *size as f64 / face.upem.max(1.0);
            ctx.set_paint(color(opts.ink(mark.unwrap_or(opts.mark_color)), 0.9));
            ctx.set_transform(view * Affine::translate((*x as f64, *baseline as f64)) * Affine::scale(k));
            ctx.fill_path(&o);
        }
        Draw::MarkText { x, baseline, size, text } => {
            let base_face = wordcraft_fonts::word::resolve("Source Sans 3", false, false).face;
            ctx.set_paint(color(opts.ink(opts.mark_color), 0.9));
            let mut gx = *x as f64;
            for ch in text.chars() {
                let mut face = base_face;
                if face.glyph_for(ch) == 0
                    && let Some(f) = FontDb::global().fallback_for(ch, face.id())
                {
                    face = wordcraft_fonts::FaceRef::of(&f);
                }
                let gid = face.glyph_for(ch);
                let k = *size as f64 / face.upem.max(1.0);
                let o = FontDb::global().outline(&face, gid);
                ctx.set_transform(view * Affine::translate((gx, *baseline as f64)) * Affine::scale(k));
                ctx.fill_path(&o);
                gx += face.advance(gid) * k;
            }
        }
        Draw::Image { rect, media, crop, alpha, .. } => {
            let r = kurbo::Rect::new(rect.x as f64, rect.y as f64, rect.right() as f64, rect.bottom() as f64);
            if !r.overlaps(*visible) {
                return;
            }
            if let Some(v) = vector_for(doc, media) {
                draw_vector(ctx, &v, &r, crop, *alpha, view);
                return;
            }
            let Some(pm) = pixmap_for(doc, media) else {
                ctx.set_transform(view);
                ctx.set_paint(color(opts.ink(Rgb(0xD0, 0xD0, 0xD0)), 1.0));
                ctx.fill_rect(&r);
                return;
            };
            let (pw, ph) = (pm.width().max(1) as f64, pm.height().max(1) as f64);
            let [cl, ct, cr, cb] = crop.map(|v| if v.is_finite() { v.clamp(0.0, 0.95) as f64 } else { 0.0 });
            let vis_w = (1.0 - cl - cr).max(0.05);
            let vis_h = (1.0 - ct - cb).max(0.05);
            let sx = r.width() / (pw * vis_w);
            let sy = r.height() / (ph * vis_h);
            ctx.set_transform(view);
            // vello_cpu panics on image paints with sampler alpha != 1, so fade through a layer.
            let alpha = if alpha.is_finite() { alpha.clamp(0.0, 1.0) } else { 1.0 };
            let faded = alpha < 1.0;
            if faded {
                ctx.push_opacity_layer(alpha);
            }
            ctx.set_paint(vello_cpu::Image {
                image: vello_cpu::ImageSource::Pixmap(pm),
                sampler: peniko::ImageSampler::default().with_quality(peniko::ImageQuality::Medium),
            });
            ctx.set_paint_transform(Affine::translate((r.x0 - cl * pw * sx, r.y0 - ct * ph * sy)) * Affine::scale_non_uniform(sx, sy));
            ctx.fill_rect(&r);
            ctx.reset_paint_transform();
            if faded {
                ctx.pop_layer();
            }
        }
        Draw::Path { segs, fill, stroke, stroke_width, dash } => {
            let path = wordcraft_layout::display::seg_path(segs);
            if path.elements().is_empty() {
                return;
            }
            ctx.set_transform(view);
            if let Some(f) = fill {
                ctx.set_paint(color(opts.ink(*f), 1.0));
                ctx.fill_path(&path);
            }
            if let Some(s) = stroke {
                // Width 0 (or none) is a hairline.
                let w = if stroke_width.is_finite() && *stroke_width > 0.0 { stroke_width.clamp(0.25, 200.0) } else { 0.75 };
                ctx.set_paint(color(opts.ink(*s), 1.0));
                ctx.set_stroke(kurbo::Stroke::new(w as f64).with_dashes(0.0, dash.iter().map(|v| f64::from(*v))));
                ctx.stroke_path(&path);
            }
        }
        Draw::Art { path, fx, color: c, alpha, text_bg } => {
            let Some(b) = wordcraft_layout::wordart::path_box(path) else { return };
            if !b.inflate(200.0, 200.0).overlaps(*visible) {
                return;
            }
            ctx.set_transform(view);
            for l in wordcraft_layout::wordart::art_layers(path, fx, *c, *alpha) {
                // The letters read against their highlight like plain text (#1252); shadows don't.
                let ink = |c: Rgb| if l.letters { opts.text(c, behind.page, text_bg.map(|bg| opts.background(bg))) } else { opts.ink(c) };
                match &l.paint {
                    ArtPaint::Solid(c, a) => ctx.set_paint(color(ink(*c), *a)),
                    ArtPaint::Linear { p0, p1, stops } => {
                        let stops: Vec<peniko::ColorStop> =
                            stops.iter().map(|(o, c, a)| peniko::ColorStop::from((o.clamp(0.0, 1.0), color(ink(*c), *a)))).collect();
                        let g = peniko::Gradient::new_linear((f64::from(p0.0), f64::from(p0.1)), (f64::from(p1.0), f64::from(p1.1)))
                            .with_stops(stops.as_slice());
                        ctx.set_paint(g);
                    }
                }
                match l.stroke {
                    Some(w) => {
                        ctx.set_stroke(kurbo::Stroke::new(f64::from(w.clamp(0.1, 400.0))).with_join(kurbo::Join::Round));
                        ctx.stroke_path(&l.path);
                    }
                    None => ctx.fill_path(&l.path),
                }
            }
        }
        Draw::Ink { pts, color: c, width, alpha } => {
            let Some(path) = ink_path(pts) else { return };
            if !path.bounding_box().inflate(*width as f64, *width as f64).overlaps(*visible) {
                return;
            }
            ctx.set_transform(view);
            ctx.set_paint(color(opts.ink(*c), *alpha));
            let w = if width.is_finite() { width.clamp(0.25, 200.0) } else { 1.0 };
            ctx.set_stroke(kurbo::Stroke::new(w as f64).with_caps(kurbo::Cap::Round).with_join(kurbo::Join::Round));
            ctx.stroke_path(&path);
        }
        Draw::Shape { rect, kind, fill, stroke, stroke_width, effects, corner, dash } => {
            let r = kurbo::Rect::new(rect.x as f64, rect.y as f64, rect.right() as f64, rect.bottom() as f64);
            let path = shape_path(*kind, r, *corner);
            ctx.set_transform(view);
            let fx = effects.sanitized();
            let can_fill = *kind != ShapeKind::Line && fill.is_some();
            if !fx.is_empty() && (can_fill || stroke.is_some()) {
                let sw = if stroke_width.is_finite() { stroke_width.clamp(0.25, 200.0) as f64 } else { 0.75 };
                // The shape's silhouette grown by `g` points and moved by `d`, in the current paint.
                let silhouette = |ctx: &mut RenderContext, g: f32, d: kurbo::Vec2| {
                    let g = g as f64;
                    if can_fill {
                        let grow = g + if stroke.is_some() { sw / 2.0 } else { 0.0 };
                        let rr = r.inflate(grow, grow) + d;
                        if rr.width() > 0.0 && rr.height() > 0.0 {
                            ctx.fill_path(&shape_path(*kind, rr, *corner));
                        }
                    } else if sw + 2.0 * g > 0.05 {
                        ctx.set_stroke(kurbo::Stroke::new(sw + 2.0 * g));
                        ctx.stroke_path(&shape_path(*kind, r + d, *corner));
                    }
                };
                if let Some(sh) = fx.shadow {
                    let (dx, dy) = sh.offset();
                    for (g, a) in wordcraft_doc::effects::bands(-sh.blur / 2.0, sh.blur / 2.0, sh.opacity()) {
                        ctx.set_paint(color(opts.ink(sh.color), a));
                        silhouette(ctx, g, kurbo::Vec2::new(dx as f64, dy as f64));
                    }
                }
                if let Some(gl) = fx.glow {
                    for (g, a) in wordcraft_doc::effects::bands(0.0, gl.size, gl.opacity()) {
                        ctx.set_paint(color(opts.ink(gl.color), a));
                        silhouette(ctx, g, kurbo::Vec2::ZERO);
                    }
                }
            }
            // Soft edges: the fill fades out toward the outline (which fades with it).
            if let (Some(rad), Some(f), true) = (fx.soft_edge, fill, can_fill) {
                let rad = rad.min(rect.w.min(rect.h) / 2.0).max(0.0);
                for (g, a) in wordcraft_doc::effects::bands(-rad, 0.0, 1.0) {
                    let rr = r.inflate(g as f64, g as f64);
                    if rr.width() > 0.0 && rr.height() > 0.0 {
                        ctx.set_paint(color(opts.background(*f), a));
                        ctx.fill_path(&shape_path(*kind, rr, *corner));
                    }
                }
                return;
            }
            if let Some(f) = fill {
                ctx.set_paint(color(opts.background(*f), 1.0));
                ctx.fill_path(&path);
            }
            if let Some(s) = stroke {
                ctx.set_paint(color(opts.ink(*s), 1.0));
                ctx.set_stroke(
                    kurbo::Stroke::new(if stroke_width.is_nan() { 0.25 } else { stroke_width.clamp(0.25, 200.0) } as f64)
                        .with_dashes(0.0, dash.iter().map(|v| f64::from(*v))),
                );
                ctx.stroke_path(&path);
            }
        }
        Draw::Clip { rect, items } => {
            let r = kurbo::Rect::new(rect.x as f64, rect.y as f64, rect.right() as f64, rect.bottom() as f64);
            if !r.is_finite() || !r.overlaps(*visible) {
                return;
            }
            ctx.set_transform(view);
            ctx.push_clip_layer(&r.to_path(0.1));
            let local = r.intersect(*visible);
            for it in items {
                draw(ctx, doc, it, view, &local, opts, behind);
            }
            ctx.pop_layer();
        }
        Draw::Turned { x, y, turn, items } => {
            let m = Affine::new(Draw::turn_matrix(*turn, *x, *y).map(f64::from));
            let local = m.inverse().transform_rect_bbox(*visible);
            for it in items {
                draw(ctx, doc, it, view * m, &local, opts, &Behind { page: behind.page, fills: Vec::new() });
            }
        }
        Draw::Rotated { cx, cy, spin, items } => {
            let m = Affine::new(spin.matrix(*cx, *cy).map(f64::from));
            let local = m.inverse().transform_rect_bbox(*visible);
            for it in items {
                draw(ctx, doc, it, view * m, &local, opts, &Behind { page: behind.page, fills: Vec::new() });
            }
        }
    }
}

/// The line through an ink stroke's points (finite ones only); a single point is a dot.
fn ink_path(pts: &[(f32, f32)]) -> Option<BezPath> {
    let mut p = BezPath::new();
    let mut last = None;
    for &(x, y) in pts.iter().filter(|(x, y)| x.is_finite() && y.is_finite() && x.abs() < 1e6 && y.abs() < 1e6) {
        let pt = (x as f64, y as f64);
        if last.is_none() {
            p.move_to(pt);
        } else {
            p.line_to(pt);
        }
        last = Some(pt);
    }
    // Only sanitized points reach the path: a tap is drawn at the one usable point.
    let only = last?;
    if p.elements().len() == 1 {
        // A tap: a zero-length line, which round caps draw as a dot.
        p.line_to(only);
    }
    Some(p)
}

/// Outline of a basic shape in a rectangle; `corner` is a rounded rectangle's
/// ([`wordcraft_doc::connector::ShapeExtra::corner`]).
pub fn shape_path(kind: ShapeKind, r: kurbo::Rect, corner: Option<u32>) -> BezPath {
    let (cx, cy) = (r.center().x, r.center().y);
    let poly = |pts: &[(f64, f64)]| {
        let mut p = BezPath::new();
        for (i, (x, y)) in pts.iter().enumerate() {
            if i == 0 {
                p.move_to((*x, *y));
            } else {
                p.line_to((*x, *y));
            }
        }
        p.close_path();
        p
    };
    match kind {
        // A freeform is drawn from its own paths; without them, its frame.
        ShapeKind::Rectangle | ShapeKind::TextBox | ShapeKind::Freeform => r.to_path(0.1),
        ShapeKind::RoundedRectangle => {
            let radius = wordcraft_doc::connector::corner_radius(corner, r.width() as f32, r.height() as f32);
            kurbo::RoundedRect::from_rect(r, f64::from(radius)).to_path(0.1)
        }
        ShapeKind::Ellipse => kurbo::Ellipse::from_rect(r).to_path(0.1),
        ShapeKind::FlowchartOr => {
            let mut p = kurbo::Ellipse::from_rect(r).to_path(0.1);
            p.move_to((r.x0, cy));
            p.line_to((r.x1, cy));
            p.move_to((cx, r.y0));
            p.line_to((cx, r.y1));
            p
        }
        ShapeKind::Triangle => poly(&[(cx, r.y0), (r.x1, r.y1), (r.x0, r.y1)]),
        ShapeKind::RightTriangle => poly(&[(r.x0, r.y0), (r.x1, r.y1), (r.x0, r.y1)]),
        ShapeKind::Diamond => poly(&[(cx, r.y0), (r.x1, cy), (cx, r.y1), (r.x0, cy)]),
        // DrawingML's `line`: top-left to bottom-right (flips give the other directions).
        ShapeKind::Line => {
            let mut p = BezPath::new();
            p.move_to((r.x0, r.y0));
            p.line_to((r.x1, r.y1));
            p
        }
        // Connectors are drawn as paths (`Draw::Path`); this is their frame's diagonal.
        ShapeKind::StraightConnector | ShapeKind::ElbowConnector | ShapeKind::CurvedConnector => {
            let mut p = BezPath::new();
            p.move_to((r.x0, r.y0));
            p.line_to((r.x1, r.y1));
            p
        }
        ShapeKind::Arrow => {
            let h = r.height();
            poly(&[
                (r.x0, cy - h * 0.2),
                (r.x1 - h * 0.5, cy - h * 0.2),
                (r.x1 - h * 0.5, r.y0),
                (r.x1, cy),
                (r.x1 - h * 0.5, r.y1),
                (r.x1 - h * 0.5, cy + h * 0.2),
                (r.x0, cy + h * 0.2),
            ])
        }
        ShapeKind::Star => {
            let mut pts = Vec::new();
            for i in 0..10 {
                let a = std::f64::consts::PI * (i as f64) / 5.0 - std::f64::consts::FRAC_PI_2;
                let rad = if i % 2 == 0 { 1.0 } else { 0.4 };
                pts.push((cx + a.cos() * r.width() / 2.0 * rad, cy + a.sin() * r.height() / 2.0 * rad));
            }
            poly(&pts)
        }
        ShapeKind::Heart => {
            let (w, h) = (r.width(), r.height());
            let mut p = BezPath::new();
            p.move_to((cx, r.y0 + h * 0.3));
            p.curve_to((cx, r.y0), (r.x0, r.y0), (r.x0, r.y0 + h * 0.3));
            p.curve_to((r.x0, r.y0 + h * 0.6), (cx - w * 0.1, r.y0 + h * 0.75), (cx, r.y1));
            p.curve_to((cx + w * 0.1, r.y0 + h * 0.75), (r.x1, r.y0 + h * 0.6), (r.x1, r.y0 + h * 0.3));
            p.curve_to((r.x1, r.y0), (cx, r.y0), (cx, r.y0 + h * 0.3));
            p.close_path();
            p
        }
    }
}

/// Render the page area (`x`, `y`, `w`×`h` points) at `scale` px/pt — for previews and thumbnails.
pub fn render_area(doc: &Document, page: &Page, x: f32, y: f32, w: f32, h: f32, scale: f32, opts: &RenderOptions) -> Rendered {
    let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let pw = ((w * scale).ceil() as u32).clamp(1, MAX_SIDE);
    let ph = ((h * scale).ceil() as u32).clamp(1, MAX_SIDE);
    let view = Affine::scale(scale as f64) * Affine::translate((-x as f64, -y as f64));
    render_region(doc, page, pw, ph, view, opts)
}

/// Draw `items` (page points) alone, on nothing, over the page area (`x`, `y`, `w`×`h` points)
/// at `scale` px/pt: no paper, no watermark. A dragged object's preview.
pub fn render_items(doc: &Document, items: &[Draw], x: f32, y: f32, w: f32, h: f32, scale: f32, opts: &RenderOptions) -> Rendered {
    let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let (pw, ph) = (((w * scale).ceil() as u32).clamp(1, MAX_SIDE), ((h * scale).ceil() as u32).clamp(1, MAX_SIDE));
    let (cw, ch) = wordcraft_geom::fit_pixels(pw, ph, wordcraft_geom::MAX_RASTER_PIXELS);
    let (w16, h16) = (cw as u16, ch as u16);
    let view = Affine::scale_non_uniform(cw as f64 / pw as f64, ch as f64 / ph as f64)
        * Affine::scale(scale as f64)
        * Affine::translate((-x as f64, -y as f64));
    let mut ctx = RenderContext::new_with(w16, h16, vello_cpu::RenderSettings { num_threads: 0, ..Default::default() });
    let visible = view.inverse().transform_rect_bbox(kurbo::Rect::new(0.0, 0.0, w16 as f64, h16 as f64)).inflate(40.0, 40.0);
    // Drawn over the page, so its colours are worked out against the page's paper.
    let paper = opts.page_background(doc.settings.page_color);
    let behind = if opts.dark { Behind::new(items, paper, opts) } else { Behind { page: paper, fills: Vec::new() } };
    for it in items {
        draw(&mut ctx, doc, it, view, &visible, opts, &behind);
    }
    ctx.flush();
    let mut pixels = vec![0u8; w16 as usize * h16 as usize * 4];
    let mut res = Resources::new();
    if let Some(pm) = vello_cpu::PixmapMut::new(w16, h16, &mut pixels) {
        ctx.render(pm, &mut res);
    }
    Rendered { width: w16 as u32, height: h16 as u32, pixels }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wordcraft_doc::props::Highlight;
    use wordcraft_layout::{LayoutCache, LayoutOptions, layout};

    /// The first and last pixel rows with ink in a black-on-white [`render_text_line`] raster.
    fn ink_rows(img: &Rendered) -> Option<(usize, usize)> {
        let w = img.width as usize * 4;
        let rows: Vec<usize> = img.pixels.chunks(w).enumerate().filter(|(_, r)| r.chunks(4).any(|p| p[0] < 128)).map(|(i, _)| i).collect();
        Some((*rows.first()?, *rows.last()?))
    }

    /// #2570: a font with a very tall ascent (Amiri Quran's is 1.8 em, for stacked marks) put its
    /// font list entry's baseline below the row, so the name was cut in half, and every tall-ascent
    /// font sat lower than its neighbours. The entry's capitals are now centred in the row,
    /// whatever the ascent.
    #[test]
    fn a_tall_ascent_font_list_entry_sits_where_the_others_do() {
        // Source Sans 3 renamed (same length) with its ascent set to Amiri Quran's 1815/1000 em.
        let mut data = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts/SourceSans3-Regular.ttf")).unwrap();
        let utf16 = |s: &str| s.encode_utf16().flat_map(u16::to_be_bytes).collect::<Vec<u8>>();
        for (from, to) in [(utf16("Source Sans 3"), utf16("Talled Sans 3")), (b"Source Sans 3".to_vec(), b"Talled Sans 3".to_vec())] {
            let mut i = 0;
            while let Some(at) = data[i..].windows(from.len()).position(|w| w == from) {
                data[i + at..i + at + to.len()].copy_from_slice(&to);
                i += at + to.len();
            }
        }
        let table = |data: &[u8], tag: &[u8; 4]| {
            let n = u16::from_be_bytes([data[4], data[5]]) as usize;
            (0..n)
                .map(|k| 12 + k * 16)
                .find(|&r| &data[r..r + 4] == tag)
                .map(|r| u32::from_be_bytes(data[r + 8..r + 12].try_into().unwrap()) as usize)
                .unwrap()
        };
        let upem = u16::from_be_bytes(data[table(&data, b"head") + 18..][..2].try_into().unwrap()) as f64;
        let tall = ((upem * 1.815) as i16).to_be_bytes();
        let (hhea, os2) = (table(&data, b"hhea"), table(&data, b"OS/2"));
        for at in [hhea + 4, os2 + 68, os2 + 74] {
            data[at..at + 2].copy_from_slice(&tall);
        }
        assert!(wordcraft_fonts::FontDb::global().add_font(data) > 0);
        let db = wordcraft_fonts::FontDb::global();
        let tall_face = db.face("Talled Sans 3", "Regular");
        let normal = db.face(wordcraft_fonts::FALLBACK_FAMILY, "Regular");
        assert_eq!(tall_face.family, "Talled Sans 3");
        assert!(tall_face.ascent > normal.ascent * 1.5, "patched ascent {} vs {}", tall_face.ascent, normal.ascent);
        // The font list's entry: 13 pt text in a 244×22 pt row, 2 pt in.
        let entry = |f: &wordcraft_fonts::FontFace| ink_rows(&render_text_line(f, "Amiri Quran", 13.0, 244.0, 22.0, 2.0, 2.0));
        let (top, bottom) = entry(&normal).unwrap();
        let (tall_top, tall_bottom) = entry(&tall_face).unwrap();
        assert_eq!((tall_top, tall_bottom), (top, bottom), "same glyphs, same place in the row");
        assert!(top > 0 && bottom < 43, "the name isn't clipped: ink rows {top}..={bottom} of 44");
        // Centred: as much room above the capitals as below the descender, within a few pixels.
        let (above, below) = (top as i32, 43 - bottom as i32);
        assert!((above - below).abs() <= 6, "rows above {above}, below {below}");
    }

    /// #299: the caret's background is the page colour, a highlight, or the dark page.
    #[test]
    fn background_at_caret_follows_page_colour_highlight_and_dark_mode() {
        use wordcraft_doc::Pos;
        use wordcraft_doc::props::Highlight;
        let mut d = Document::from_text("Hello WordCraft");
        let mid = |d: &Document, off: usize| {
            let l = layout(d, &mut LayoutCache::new(), &LayoutOptions::default());
            let c = l.caret_on(&Pos::body(0, off), 0).unwrap();
            let page = l.pages[c.page].clone();
            (page, c.x, c.top + c.height / 2.0)
        };
        let light = RenderOptions::default();
        let dark = RenderOptions { dark: true, ..Default::default() };
        let (page, x, y) = mid(&d, 2);
        assert_eq!(background_at(&d, &page, &light, x, y), Rgb::WHITE);
        assert_eq!(background_at(&d, &page, &dark, x, y), Rgb(DARK_PAPER, DARK_PAPER, DARK_PAPER));
        assert_eq!(background_at(&d, &page, &light, f32::NAN, f32::INFINITY), Rgb::WHITE, "junk points fall back to the paper");
        d.settings.page_color = Some(Rgb(0x10, 0x10, 0x40));
        assert_eq!(background_at(&d, &page, &light, x, y), Rgb(0x10, 0x10, 0x40));
        d.settings.page_color = None;
        d.format_range(&Pos::body(0, 0), &Pos::body(0, 5), &|c| c.highlight = Some(Highlight::Black)).unwrap();
        let (page, x, y) = mid(&d, 2);
        assert_eq!(background_at(&d, &page, &light, x, y), Rgb::BLACK, "inside the highlight");
        let (page, x, y) = mid(&d, 9);
        assert_eq!(background_at(&d, &page, &light, x, y), Rgb::WHITE, "past the highlight");
    }

    /// #299: a filled shape colours the caret's background only inside its outline: an
    /// ellipse's corner shows what lies beneath, its centre the fill.
    #[test]
    fn background_follows_a_shapes_outline_not_its_box() {
        let opts = RenderOptions::default();
        let ellipse = Draw::Shape {
            rect: wordcraft_geom::Rect::new(100.0, 100.0, 200.0, 100.0),
            kind: ShapeKind::Ellipse,
            fill: Some(Rgb::BLACK),
            stroke: None,
            stroke_width: 0.0,
            effects: Default::default(),
            corner: None,
            dash: Vec::new(),
        };
        let behind = Behind::new(std::slice::from_ref(&ellipse), Rgb::WHITE, &opts);
        let at = |x: f64, y: f64| behind.at(x, y);
        assert_eq!(at(200.0, 150.0), Rgb::BLACK, "the centre is the fill");
        assert_eq!(at(103.0, 103.0), Rgb::WHITE, "the box's corner is outside the ellipse");
        assert_eq!(at(350.0, 150.0), Rgb::WHITE, "outside the box");
    }

    /// An image the encoder can't write is an error, never an empty PNG reported as written
    /// (`file.exportPng` wrote a 0-byte file and said it had saved it).
    #[test]
    fn a_png_that_cant_be_encoded_is_an_error_not_empty_bytes() {
        for (width, height) in [(0, 0), (0, 10), (10, 0)] {
            let img = Rendered { width, height, pixels: Vec::new() };
            assert!(img.to_png().is_err(), "{width}×{height}");
        }
        let png = Rendered { width: 2, height: 1, pixels: vec![255; 8] }.to_png().unwrap();
        assert!(png.starts_with(b"\x89PNG"));
    }

    #[test]
    fn renders_text_dark_pixels() {
        let d = Document::from_text("Hello WordCraft");
        let l = layout(&d, &mut LayoutCache::new(), &LayoutOptions::default());
        let img = render_page(&d, &l.pages[0], 1.0, &RenderOptions::default());
        assert_eq!((img.width, img.height), (612, 792));
        // Somewhere in the first text line there are dark pixels; the margins are white.
        let dark = (72..300).flat_map(|x| (72..100).map(move |y| (x, y))).filter(|(x, y)| img.pixel(*x, *y)[0] < 128).count();
        assert!(dark > 30, "dark {dark}");
        assert_eq!(img.pixel(5, 5), [255, 255, 255, 255]);
        assert!(!img.to_png().unwrap().is_empty());
    }

    /// Word's table cell highlighting in the markup views (its Advanced Track Changes Options;
    /// `docs/word-behaviour-research.md` item 2): a cell inserted with Track Changes on is filled
    /// light blue and a cell of a deleted row pink, under the text; No Markup shows plain cells;
    /// on a dark page the fills take the dark versions of their hues, as every background does.
    #[test]
    fn tracked_cells_are_highlighted_in_markup_light_and_dark() {
        use wordcraft_doc::{Revision, RevisionKind};
        use wordcraft_layout::display::{CELL_DELETED, CELL_INSERTED, DisplayOptions};
        let mut d = Document::from_text("before\nafter");
        d.revisions.push(Revision { kind: RevisionKind::Insert, author: "A".into(), date: String::new(), move_name: None });
        d.revisions.push(Revision { kind: RevisionKind::Delete, author: "A".into(), date: String::new(), move_name: None });
        let mut t = wordcraft_doc::Table::new(2, 2, 400.0);
        for r in 0..2 {
            for c in 0..2 {
                t.rows[r].cells[c].blocks = vec![wordcraft_doc::para_block(wordcraft_doc::Paragraph::with_text("x", Default::default()))];
            }
        }
        t.rows[0].cells[1].props.ins = Some(0);
        t.rows[1].props.del = Some(1);
        d.insert_block(wordcraft_doc::StoryRef::Body, &wordcraft_doc::Path::top(1), wordcraft_doc::Block::Table(t)).unwrap();
        let l = layout(&d, &mut LayoutCache::new(), &LayoutOptions::default());
        let page = &l.pages[0];
        let cells: Vec<(usize, usize, wordcraft_geom::Rect)> = page
            .items
            .iter()
            .filter_map(|it| match it {
                wordcraft_layout::Placed::Cell { rect, row, cell, .. } => Some((*row, *cell, *rect)),
                _ => None,
            })
            .collect();
        assert_eq!(cells.len(), 4, "{cells:?}");
        // A pixel in the lower part of each cell, clear of its text line and borders.
        let probe = |r: usize, c: usize| {
            let rect = cells.iter().find(|(x, y, _)| (*x, *y) == (r, c)).map(|x| x.2).unwrap();
            ((rect.x + rect.w / 2.0) as u32, (rect.y + rect.h - 4.0) as u32)
        };
        let render = |markup: bool, dark: bool| {
            let opts = RenderOptions { dark, display: DisplayOptions { markup, ..Default::default() }, ..Default::default() };
            let img = render_page(&d, page, 1.0, &opts);
            let px = |(x, y): (u32, u32)| {
                let p = img.pixel(x, y);
                Rgb(p[0], p[1], p[2])
            };
            (px(probe(0, 0)), px(probe(0, 1)), px(probe(1, 0)), px(probe(1, 1)), opts)
        };
        let white = Rgb(255, 255, 255);
        let (plain, inserted, deleted_a, deleted_b, _) = render(true, false);
        assert_eq!((plain, inserted), (white, CELL_INSERTED), "an inserted cell is filled light blue");
        assert_eq!((deleted_a, deleted_b), (CELL_DELETED, CELL_DELETED), "a deleted row's cells are filled pink");
        let (plain, inserted, deleted_a, _, _) = render(false, false);
        assert_eq!((plain, inserted, deleted_a), (white, white, white), "No Markup shows plain cells");
        let (plain, inserted, deleted_a, _, opts) = render(true, true);
        assert_eq!(plain, opts.page_background(None));
        assert_eq!((inserted, deleted_a), (opts.background(CELL_INSERTED), opts.background(CELL_DELETED)), "the dark page dims the fills");
        assert_ne!(inserted, CELL_INSERTED);
    }

    /// Table Layout › Text Direction (#226): a turned cell's text is painted running down the
    /// cell, not across it.
    #[test]
    fn turned_cell_text_is_painted_turned() {
        use wordcraft_doc::props::TextDirection;
        let ink = |dir: TextDirection| {
            let mut d = Document::from_text("before\nafter");
            let mut t = wordcraft_doc::Table::new(1, 2, 400.0);
            t.rows[0].cells[0].blocks = vec![wordcraft_doc::para_block(wordcraft_doc::Paragraph::with_text("WWWWWWWWWWWW", Default::default()))];
            t.rows[0].cells[0].props.text_direction = dir;
            d.insert_block(wordcraft_doc::StoryRef::Body, &wordcraft_doc::Path::top(1), wordcraft_doc::Block::Table(t)).unwrap();
            let l = layout(&d, &mut LayoutCache::new(), &LayoutOptions::default());
            let img = render_page(&d, &l.pages[0], 1.0, &RenderOptions::default());
            // Dark pixels inside the first cell (x 72..272, clear of its borders), well below
            // the first text line.
            (80..264).flat_map(|x| (140..190).map(move |y| (x, y))).filter(|(x, y)| img.pixel(*x, *y)[0] < 128).count()
        };
        assert_eq!(ink(TextDirection::Horizontal), 0, "horizontal text stays in its line");
        assert!(ink(TextDirection::Down) > 30, "down");
        assert!(ink(TextDirection::Up) > 30, "up");
    }

    /// Shape Effects (#275): an outer shadow paints dark pixels beside the shape on the side it is
    /// cast to, and none on the other; a glow surrounds it.
    #[test]
    fn shape_shadow_is_cast_offset_from_the_shape() {
        use wordcraft_doc::effects::{Glow, Shadow, ShapeEffects};
        use wordcraft_doc::para::InlineObject;
        let draw = |effects: ShapeEffects| {
            let mut d = Document::from_text("");
            let shape = InlineObject::Shape {
                kind: ShapeKind::Rectangle,
                w: 100.0,
                h: 60.0,
                fill: Some(Rgb(0, 0, 255)),
                stroke: None,
                stroke_width: 0.0,
                float: Default::default(),
                story: None,
                freeform: None,
                effects,
                extra: Default::default(),
            };
            let at = wordcraft_doc::Pos { story: wordcraft_doc::StoryRef::Body, path: wordcraft_doc::Path::top(0), off: 0 };
            d.insert_object(&at, shape, &Default::default()).unwrap();
            let l = layout(&d, &mut LayoutCache::new(), &LayoutOptions::default());
            render_page(&d, &l.pages[0], 1.0, &RenderOptions::default())
        };
        let plain = draw(ShapeEffects::default());
        let blue = |img: &Rendered| {
            let px: Vec<(u32, u32)> = (0..612)
                .flat_map(|x| (0..400).map(move |y| (x, y)))
                .filter(|(x, y)| matches!(img.pixel(*x, *y), [r, _, b, _] if r < 40 && b > 200))
                .collect();
            let (x0, x1) = (px.iter().map(|p| p.0).min().unwrap(), px.iter().map(|p| p.0).max().unwrap());
            let (y0, y1) = (px.iter().map(|p| p.1).min().unwrap(), px.iter().map(|p| p.1).max().unwrap());
            (x0, y0, x1, y1)
        };
        let (x0, y0, x1, y1) = blue(&plain);
        assert!(x1 - x0 > 90 && y1 - y0 > 50, "{:?}", (x0, y0, x1, y1));
        // Grey (not white, not blue) pixels in a strip just outside an edge.
        let grey = |img: &Rendered, xs: std::ops::Range<u32>, ys: std::ops::Range<u32>| {
            xs.flat_map(|x| ys.clone().map(move |y| (x, y)))
                .filter(|(x, y)| matches!(img.pixel(*x, *y), [r, g, b, _] if r < 200 && r == g && g == b))
                .count()
        };
        let right = |img: &Rendered| grey(img, x1 + 2..x1 + 6, y0 + 10..y1);
        let left = |img: &Rendered| grey(img, x0.saturating_sub(6)..x0.saturating_sub(2), y0 + 10..y1);
        assert_eq!((right(&plain), left(&plain)), (0, 0));
        let shadow = Shadow { color: Rgb::BLACK, transparency: 30.0, blur: 2.0, distance: 8.0, angle: 0.0, rot_with_shape: false };
        let img = draw(ShapeEffects { shadow: Some(shadow), ..Default::default() });
        assert_eq!(blue(&img), (x0, y0, x1, y1), "the shape itself is unchanged");
        assert!(right(&img) > 100, "shadow to the right: {}", right(&img));
        assert_eq!(left(&img), 0, "nothing on the left");
        let img = draw(ShapeEffects { glow: Some(Glow { color: Rgb(255, 0, 0), size: 6.0, transparency: 0.0 }), ..Default::default() });
        let reddish = |x: u32, y: u32| matches!(img.pixel(x, y), [r, g, _, _] if r > 200 && g < 200);
        assert!(reddish(x1 + 2, (y0 + y1) / 2) && reddish(x0 - 2, (y0 + y1) / 2), "glow on both sides");
    }

    #[test]
    fn stem_darkening_follows_the_macos_curve() {
        assert_eq!(stem_darkening(0.0), 0.0);
        assert_eq!(stem_darkening(f64::NAN), 0.0);
        assert_eq!(stem_darkening(-5.0), 0.0);
        // Small text grows in proportion to its pixel size...
        assert!((stem_darkening(10.0) - 10.0 * 0.0121 * 2.25 / 2.0).abs() < 1e-9);
        // ...body text on a Retina screen hits the 0.3 px cap...
        assert_eq!(stem_darkening(30.0), 0.3);
        assert_eq!(stem_darkening(72.0), 0.3);
        // ...and display sizes aren't darkened at all.
        assert_eq!(stem_darkening(72.5), 0.0);
    }

    /// Ink in a raster: the sum of how far each pixel is from white.
    fn ink(img: &Rendered) -> u64 {
        img.pixels.as_chunks::<4>().0.iter().map(|p| 255 - p[0] as u64).sum()
    }

    /// The number of rasterising workers changes how long a page takes and what CPU it costs,
    /// never its pixels (the screen draws pages on fewer workers than exports).
    #[test]
    fn the_number_of_workers_never_changes_the_pixels() {
        let text = (0..40).map(|i| format!("Line {i} of a page with some words, shading and marks.")).collect::<Vec<_>>().join("\n");
        let d = Document::from_text(&text);
        let l = layout(&d, &mut LayoutCache::new(), &LayoutOptions::default());
        let at = |threads: Option<u16>| render_page(&d, &l.pages[0], 1.5, &RenderOptions { threads, ..Default::default() }).pixels;
        let default = at(None);
        for threads in [0, 1, 2, 3, 8] {
            assert!(at(Some(threads)) == default, "{threads} workers");
        }
    }

    #[test]
    fn text_darkening_adds_ink_on_screen_only() {
        let d = Document::from_text("Marketing analytics lead with twelve years of measurement work.");
        let l = layout(&d, &mut LayoutCache::new(), &LayoutOptions::default());
        let plain = render_page(&d, &l.pages[0], 2.0, &RenderOptions::default());
        let dark = render_page(&d, &l.pages[0], 2.0, &RenderOptions { text_darkening: true, ..Default::default() });
        let (a, b) = (ink(&plain), ink(&dark));
        // About 0.3 px more on every edge of an 11 pt line at 2x: clearly more ink, not bold.
        assert!(b as f64 > a as f64 * 1.08 && (b as f64) < a as f64 * 1.6, "plain {a}, darkened {b}");
        // Off by default: exports and thumbnails are untouched.
        assert!(!RenderOptions::default().text_darkening);
    }

    #[test]
    fn page_picture_is_painted_behind_the_text() {
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(8, 8, image::Rgba([10, 200, 30, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let mut d = Document::new();
        d.media.insert("bg.png".into(), Arc::new(png));
        for kind in ["tile", "frame"] {
            d.settings.page_picture = Some(wordcraft_doc::PagePicture { media: "bg.png".into(), fill_type: kind.into(), title: String::new() });
            let l = layout(&d, &mut LayoutCache::new(), &LayoutOptions::default());
            let img = render_page(&d, &l.pages[0], 1.0, &RenderOptions::default());
            let i = ((300 * img.width + 300) * 4) as usize;
            assert_eq!(&img.pixels[i..i + 3], &[10, 200, 30], "{kind}");
        }
        // An undecodable picture leaves the paper alone.
        d.media.insert("bg.png".into(), Arc::new(b"junk".to_vec()));
        let l = layout(&d, &mut LayoutCache::new(), &LayoutOptions::default());
        let img = render_page(&d, &l.pages[0], 1.0, &RenderOptions::default());
        assert_eq!(&img.pixels[0..3], &[255, 255, 255]);
    }

    /// A PNG signature, an IHDR declaring `w`×`h` 8-bit RGBA and a short IDAT (all CRCs valid, so
    /// the header parses), with no real pixel data.
    fn ihdr_only_png(w: u32, h: u32) -> Vec<u8> {
        fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
            let mut body = kind.to_vec();
            body.extend_from_slice(data);
            let mut crc = 0xFFFF_FFFFu32;
            for b in &body {
                crc ^= *b as u32;
                for _ in 0..8 {
                    crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
                }
            }
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            out.extend_from_slice(&body);
            out.extend_from_slice(&(!crc).to_be_bytes());
        }
        let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        let mut ihdr = w.to_be_bytes().to_vec();
        ihdr.extend_from_slice(&h.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        chunk(&mut out, b"IHDR", &ihdr);
        chunk(&mut out, b"IDAT", &[0x78, 0x9c, 0, 0]);
        out
    }

    #[test]
    fn decode_budget_is_checked_from_the_header() {
        // A header (valid CRC) claiming 60000x60000 with no pixel data reads fine, then the
        // decode limit refuses it before anything is allocated.
        let hdr = ihdr_only_png(60_000, 60_000);
        assert_eq!(image_size(&hdr), Some((60_000, 60_000)));
        assert!(decode_pixmap(&hdr).is_none());
    }

    #[test]
    fn page_raster_is_capped_by_total_pixels() {
        let d = Document::new();
        let l = layout(&d, &mut LayoutCache::new(), &LayoutOptions::default());
        let opts = RenderOptions::default();
        let ok = render_region(&d, &l.pages[0], 300, 200, Affine::IDENTITY, &opts);
        assert_eq!((ok.width, ok.height), (300, 200));
        // Bigger requests shrink to fit `MAX_PIXELS` (`fit_raster`, tested in cache_tests).
        let (w, h, k) = fit_raster(12_000.0, 8_000.0);
        assert!(u64::from(w) * u64::from(h) <= MAX_PIXELS && k < 1.0);
    }

    #[test]
    fn hostile_scale_is_clamped() {
        let d = Document::new();
        let l = layout(&d, &mut LayoutCache::new(), &LayoutOptions::default());
        let img = render_page(&d, &l.pages[0], f32::NAN, &RenderOptions::default());
        assert_eq!(img.width, 612);
        let big = render_page(&d, &l.pages[0], 1e9, &RenderOptions::default());
        assert!(big.width <= MAX_SIDE);
        assert!(decode_pixmap(b"not an image").is_none());
    }

    #[test]
    fn dimmed_header_image_renders() {
        let mut d = Document::from_text("body");
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(4, 4, image::Rgba([200, 0, 0, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let media = d.add_media(png, "png");
        let mut hp = wordcraft_doc::Paragraph::new();
        let obj = wordcraft_doc::para::InlineObject::Image {
            media,
            w: 40.0,
            h: 40.0,
            alt: String::new(),
            float: Default::default(),
            crop: [0.0; 4],
            ole: None,
        };
        hp.insert_object(0, obj, &Default::default()).unwrap();
        let id = d.add_part(wordcraft_doc::PartKind::Header, vec![wordcraft_doc::para_block(hp)]);
        d.last_section.headers.default = Some(id);
        let l = layout(&d, &mut LayoutCache::new(), &LayoutOptions::default());
        let opts = RenderOptions::default();
        assert!(opts.display.dim_header);
        let img = render_page(&d, &l.pages[0], 1.0, &opts);
        assert!(!img.to_png().unwrap().is_empty());
    }

    #[test]
    fn dark_page_inverts_lightness() {
        assert_eq!(invert_lightness(Rgb::WHITE), Rgb::BLACK);
        assert_eq!(invert_lightness(Rgb::BLACK), Rgb::WHITE);
        // Pure hues keep their colour; dark blue turns light blue.
        assert_eq!(invert_lightness(Rgb(255, 0, 0)), Rgb(255, 0, 0));
        assert_eq!(invert_lightness(Rgb(0, 0, 0x80)), Rgb(0x7F, 0x7F, 0xFF));
        let d = Document::from_text("Hello WordCraft");
        let l = layout(&d, &mut LayoutCache::new(), &LayoutOptions::default());
        let opts = RenderOptions { dark: true, ..Default::default() };
        assert_eq!(opts.ink(Rgb::WHITE), Rgb(DARK_PAPER, DARK_PAPER, DARK_PAPER));
        assert_eq!(opts.ink(Rgb::BLACK), Rgb::WHITE);
        let img = render_page(&d, &l.pages[0], 1.0, &opts);
        assert_eq!(img.pixel(5, 5), [DARK_PAPER, DARK_PAPER, DARK_PAPER, 255]);
        let light = (72..300).flat_map(|x| (72..100).map(move |y| (x, y))).filter(|(x, y)| img.pixel(*x, *y)[0] > 128).count();
        assert!(light > 30, "light {light}");
    }

    /// Text colours the dark-page audit found unreadable, and others people use.
    const TEXT: [Rgb; 10] = [
        Rgb::BLACK,
        Rgb::WHITE,
        Rgb(0, 0, 255),
        Rgb(255, 0, 0),
        Rgb(0x1F, 0x38, 0x64),
        Rgb(0xC0, 0, 0),
        Rgb(0x05, 0x63, 0xC1),
        Rgb(0, 0xB0, 0x50),
        Rgb(255, 255, 0),
        Rgb(0x70, 0x30, 0xA0),
    ];
    /// Backgrounds: paper, highlights, shading, page colours.
    const BEHIND: [Rgb; 10] = [
        Rgb::WHITE,
        Rgb(255, 255, 0),
        Rgb(0, 255, 0),
        Rgb(0, 255, 255),
        Rgb(255, 0, 255),
        Rgb(0xD9, 0xD9, 0xD9),
        Rgb(0x1F, 0x38, 0x64),
        Rgb(0xFF, 0xF2, 0xCC),
        Rgb(0xDE, 0xEA, 0xF6),
        Rgb::BLACK,
    ];

    #[test]
    fn contrast_math_matches_wcag() {
        assert!((contrast_ratio(Rgb::WHITE, Rgb::BLACK) - 21.0).abs() < 1e-9);
        assert!((contrast_ratio(Rgb(0x77, 0x77, 0x77), Rgb::WHITE) - 4.48).abs() < 0.01);
        assert_eq!(contrast_ratio(Rgb(1, 2, 3), Rgb(1, 2, 3)), 1.0);
        for c in TEXT.into_iter().chain(BEHIND) {
            let (h, s, l) = to_hsl(c);
            let back = from_hsl(h, s, l);
            assert!((0..3).all(|i| [c.0, c.1, c.2][i].abs_diff([back.0, back.1, back.2][i]) <= 1), "{c:?} round-trips as {back:?}");
        }
    }

    #[test]
    fn dark_page_text_reads_on_page_highlight_and_shading() {
        let opts = RenderOptions { dark: true, ..Default::default() };
        // Automatic text and white paper swap.
        assert_eq!(opts.page_background(None), Rgb(DARK_PAPER, DARK_PAPER, DARK_PAPER));
        assert_eq!(opts.text(Rgb::BLACK, opts.page_background(None), None), Rgb::WHITE);
        for bg in BEHIND {
            let behind = opts.background(bg);
            // Highlights, shading and page colours turn dark, so light text reads on them...
            assert!(relative_luminance(behind) <= DARK_BACKGROUND_LUMINANCE + 1e-3, "{bg:?} → {behind:?}");
            assert!(contrast_ratio(Rgb::WHITE, behind) >= 7.0, "{bg:?} → {behind:?}");
            // ...keeping their hue.
            let ((h0, s0, _), (h1, s1, _)) = (to_hsl(bg), to_hsl(behind));
            if s0 > 0.2 && s1 > 0.2 {
                assert!(hue_gap(h0, h1) < 0.03, "{bg:?} → {behind:?} changed hue");
            }
            for c in TEXT {
                let ink = opts.text(c, opts.page_background(None), Some(behind));
                let r = contrast_ratio(ink, behind);
                assert!(r >= DARK_TEXT_CONTRAST, "{c:?} on {bg:?}: {ink:?} on {behind:?} is {r:.2}:1");
                // Coloured text keeps its hue.
                let ((h0, s0, _), (h1, s1, _)) = (to_hsl(c), to_hsl(ink));
                if s0 > 0.2 && s1 > 0.2 {
                    assert!(hue_gap(h0, h1) < 0.03, "{c:?} → {ink:?} changed hue");
                }
            }
        }
        // The audit's cases, by name: white-on-highlight was 1.07:1, blue 1.84:1, red 3.5:1.
        let paper = opts.page_background(None);
        for c in [Rgb(0, 0, 255), Rgb(255, 0, 0)] {
            assert!(contrast_ratio(opts.text(c, paper, None), paper) >= DARK_TEXT_CONTRAST);
        }
        for h in [Highlight::Yellow, Highlight::BrightGreen] {
            let bg = opts.background(h.rgb().unwrap());
            assert!(contrast_ratio(opts.text(Rgb::BLACK, paper, Some(bg)), bg) >= DARK_TEXT_CONTRAST);
        }
    }

    fn hue_gap(a: f64, b: f64) -> f64 {
        let d = (a - b).rem_euclid(1.0);
        d.min(1.0 - d)
    }

    /// A one-line document whose first `n` characters carry `f`.
    fn formatted(text: &str, n: usize, f: &dyn Fn(&mut wordcraft_doc::props::CharProps)) -> Document {
        let mut d = Document::from_text(text);
        d.format_range(&wordcraft_doc::Pos::body(0, 0), &wordcraft_doc::Pos::body(0, n), f).unwrap();
        d
    }

    /// The pixels of `img` whose colour is exactly `c`, as a bounding box (x0, y0, x1, y1).
    fn bbox_of(img: &Rendered, c: Rgb) -> Option<(u32, u32, u32, u32)> {
        let mut b: Option<(u32, u32, u32, u32)> = None;
        for y in 0..img.height {
            for x in 0..img.width {
                let p = img.pixel(x, y);
                if (p[0], p[1], p[2]) == (c.0, c.1, c.2) {
                    b = Some(b.map_or((x, y, x, y), |(a, bb, cc, dd)| (a.min(x), bb.min(y), cc.max(x), dd.max(y))));
                }
            }
        }
        b
    }

    #[test]
    fn dark_page_renders_highlighted_and_shaded_text_readably() {
        let opts = RenderOptions { dark: true, ..Default::default() };
        let cases: [(&str, Rgb, &dyn Fn(&mut wordcraft_doc::props::CharProps)); 4] = [
            ("yellow highlight", Highlight::Yellow.rgb().unwrap(), &|c| c.highlight = Some(Highlight::Yellow)),
            ("green highlight", Highlight::BrightGreen.rgb().unwrap(), &|c| c.highlight = Some(Highlight::BrightGreen)),
            ("grey shading", Rgb(0xD9, 0xD9, 0xD9), &|c| c.shading = Some(Rgb(0xD9, 0xD9, 0xD9))),
            ("navy shading, white text", Rgb(0x1F, 0x38, 0x64), &|c| {
                c.shading = Some(Rgb(0x1F, 0x38, 0x64));
                c.color = Some(wordcraft_doc::props::TextColor::Rgb(Rgb::WHITE));
            }),
        ];
        for (name, bg, f) in cases {
            let d = formatted("Marked words and the rest", 12, f);
            let l = layout(&d, &mut LayoutCache::new(), &LayoutOptions::default());
            let img = render_page(&d, &l.pages[0], 3.0, &opts);
            let behind = opts.background(bg);
            let Some((x0, y0, x1, y1)) = bbox_of(&img, behind) else { panic!("{name}: no {behind:?} background on the page") };
            // The lightest pixel inside the background is a glyph's stem, at full coverage.
            let brightest = (y0..=y1)
                .flat_map(|y| (x0..=x1).map(move |x| (x, y)))
                .map(|(x, y)| img.pixel(x, y))
                .map(|p| Rgb(p[0], p[1], p[2]))
                .max_by(|a, b| relative_luminance(*a).total_cmp(&relative_luminance(*b)))
                .unwrap();
            let r = contrast_ratio(brightest, behind);
            assert!(r >= DARK_TEXT_CONTRAST, "{name}: text {brightest:?} on {behind:?} is {r:.2}:1");
            // The highlight between words (a narrow fill) is a background too, not a bright stroke.
            let as_stroke = opts.stroke(bg, opts.page_background(None));
            if as_stroke != behind && name.contains("highlight") {
                assert!(bbox_of(&img, as_stroke).is_none(), "{name}: a gap drawn as a stroke in {as_stroke:?}");
            }
        }
        // Coloured text on the dark paper.
        for c in [Rgb(0, 0, 255), Rgb(255, 0, 0)] {
            let d = formatted("Coloured", 8, &|p| p.color = Some(wordcraft_doc::props::TextColor::Rgb(c)));
            let l = layout(&d, &mut LayoutCache::new(), &LayoutOptions::default());
            let img = render_page(&d, &l.pages[0], 3.0, &opts);
            let want = opts.text(c, opts.page_background(None), None);
            assert!(bbox_of(&img, want).is_some(), "{c:?} is drawn as {want:?}");
        }
        // A page colour turns into a dark version of itself.
        let mut d = Document::from_text("Page colour");
        d.settings.page_color = Some(Rgb(0xFF, 0xF2, 0xCC));
        let l = layout(&d, &mut LayoutCache::new(), &LayoutOptions::default());
        let img = render_page(&d, &l.pages[0], 1.0, &opts);
        let p = img.pixel(3, 3);
        assert_eq!(Rgb(p[0], p[1], p[2]), opts.page_background(Some(Rgb(0xFF, 0xF2, 0xCC))));
        assert!(relative_luminance(Rgb(p[0], p[1], p[2])) <= DARK_BACKGROUND_LUMINANCE + 1e-3);
    }

    #[test]
    fn dark_page_leaves_pictures_alone() {
        let mut d = Document::from_text("x");
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(8, 8, image::Rgba([255, 255, 0, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let media = d.add_media(png, "png");
        let obj = wordcraft_doc::para::InlineObject::Image {
            media,
            w: 60.0,
            h: 60.0,
            alt: String::new(),
            float: Default::default(),
            crop: [0.0; 4],
            ole: None,
        };
        d.insert_object(&wordcraft_doc::Pos::body(0, 0), obj, &Default::default()).unwrap();
        let l = layout(&d, &mut LayoutCache::new(), &LayoutOptions::default());
        let img = render_page(&d, &l.pages[0], 1.0, &RenderOptions { dark: true, ..Default::default() });
        assert!(bbox_of(&img, Rgb(255, 255, 0)).is_some(), "the picture keeps its yellow");
    }

    #[test]
    fn export_rendering_is_unchanged() {
        // Print, export, PDF and thumbnails render with the default options: never dark.
        let opts = RenderOptions::default();
        assert!(!opts.dark);
        for c in TEXT.into_iter().chain(BEHIND) {
            assert_eq!(opts.ink(c), c);
            assert_eq!(opts.background(c), c);
            for b in BEHIND {
                assert_eq!(opts.text(c, b, Some(b)), c);
                assert_eq!(opts.text(c, b, None), c);
            }
        }
        assert_eq!(opts.page_background(None), Rgb::WHITE);
        assert_eq!(opts.page_background(Some(Rgb(1, 2, 3))), Rgb(1, 2, 3));
        // Highlighted, coloured text renders in the document's own colours.
        let d = formatted("Marked words and the rest", 12, &|c| {
            c.highlight = Some(Highlight::Yellow);
            c.color = Some(wordcraft_doc::props::TextColor::Rgb(Rgb(0, 0, 255)));
        });
        let l = layout(&d, &mut LayoutCache::new(), &LayoutOptions::default());
        let img = render_page(&d, &l.pages[0], 3.0, &opts);
        assert_eq!(img.pixel(3, 3), [255, 255, 255, 255]);
        assert!(bbox_of(&img, Rgb(255, 255, 0)).is_some());
        assert!(bbox_of(&img, Rgb(0, 0, 255)).is_some());
    }

    /// #2534: legacy font effects are drawn: outlined letters are hollow; shadowed and embossed
    /// ones have a grey copy down-right, imprinted ones up-left.
    #[test]
    fn legacy_font_effects_are_drawn() {
        use wordcraft_doc::CharProps;
        // Dark and grey pixel counts, and the grey pixels' mean offset from the dark ones.
        let shot = |c: CharProps| {
            let mut d = Document::from_text("x");
            let cp = CharProps { size: Some(40.0), bold: Some(true), ..c };
            d.body = vec![wordcraft_doc::para_block(wordcraft_doc::Paragraph::with_text("HHHH", cp))];
            let l = layout(&d, &mut LayoutCache::new(), &LayoutOptions::default());
            let img = render_page(&d, &l.pages[0], 2.0, &RenderOptions::default());
            let (mut dark, mut grey) = ((0.0, 0.0, 0.0f64), (0.0, 0.0, 0.0f64));
            for (x, y) in (100..800).flat_map(|x| (100..300).map(move |y| (x, y))) {
                let v = img.pixel(x, y)[0];
                // The copy is solid RGB 128; a wider range would count the glyph's own
                // antialiasing, whose centroid hides the copy's offset on some fonts.
                let acc = match v {
                    0..60 => &mut dark,
                    120..=136 => &mut grey,
                    _ => continue,
                };
                *acc = (acc.0 + 1.0, acc.1 + f64::from(x), acc.2 + f64::from(y));
            }
            let mean = |a: (f64, f64, f64)| (a.1 / a.0.max(1.0), a.2 / a.0.max(1.0));
            let ((dx, dy), (gx, gy)) = (mean(dark), mean(grey));
            (dark.0, grey.0, (gx - dx, gy - dy))
        };
        let (plain_dark, plain_grey, _) = shot(CharProps::default());
        assert!(plain_dark > 2000.0, "plain {plain_dark}");
        let (dark, _, _) = shot(CharProps { outline: Some(true), ..Default::default() });
        assert!(dark < plain_dark / 3.0, "outline is hollow: {dark} vs {plain_dark}");
        for (c, sign) in [
            (CharProps { shadow: Some(true), ..Default::default() }, 1.0),
            (CharProps { emboss: Some(true), ..Default::default() }, 1.0),
            (CharProps { engrave: Some(true), ..Default::default() }, -1.0),
        ] {
            let (_, grey, (ox, oy)) = shot(c);
            assert!(grey > plain_grey * 2.0, "a grey copy: {grey} vs {plain_grey}");
            assert!(ox * sign > 1.0 && oy * sign > 1.0, "grey copy offset {ox} {oy}");
        }
    }

    /// A document whose only text is a yellow-highlighted run.
    fn yellow_highlight_doc() -> (Document, wordcraft_layout::DocLayout) {
        let cp = wordcraft_doc::CharProps { highlight: Some(wordcraft_doc::props::Highlight::Yellow), size: Some(24.0), ..Default::default() };
        let mut d = Document::from_text("x");
        d.body = vec![wordcraft_doc::para_block(wordcraft_doc::Paragraph::with_text("HHHH HHHH", cp))];
        let l = layout(&d, &mut LayoutCache::new(), &LayoutOptions::default());
        (d, l)
    }

    /// #1252: on a dark page a highlight is drawn dimmed (keeping its hue) so the white text on it
    /// reads; a light page is unchanged.
    #[test]
    fn dark_page_dims_a_highlight_raster() {
        let (d, l) = yellow_highlight_doc();
        let items = page_display(&d, &l.pages[0], &DisplayOptions::default());
        let r = items
            .iter()
            .find_map(|i| match i {
                Draw::Fill { rect, color, .. } if *color == Rgb(255, 255, 0) => Some(*rect),
                _ => None,
            })
            .unwrap();
        // Just inside the top-left corner of the highlight: above the capitals, so pure background.
        let (px, py) = ((r.x + 1.5) as u32, (r.y + 1.5) as u32);
        let light = render_page(&d, &l.pages[0], 1.0, &RenderOptions::default());
        assert_eq!(light.pixel(px, py), [255, 255, 0, 255], "light page keeps bright yellow");
        let dark = render_page(&d, &l.pages[0], 1.0, &RenderOptions { dark: true, ..Default::default() });
        let [r8, g8, b8, _] = dark.pixel(px, py);
        assert!(r8 < 0x60 && g8 < 0x60, "dark page highlight is dim, got {:?}", (r8, g8, b8));
        assert!(r8.abs_diff(g8) <= 4 && g8 > b8.saturating_add(0x20), "dark page highlight stays yellow in hue, got {:?}", (r8, g8, b8));
    }

    /// #1252: on a dark page every highlight colour is dimmed so the text drawn on it reads
    /// (Automatic and explicit black text both draw white); a light page is the identity.
    #[test]
    fn dark_page_text_backgrounds_keep_text_readable() {
        use wordcraft_doc::props::{Highlight, TextColor};
        use wordcraft_layout::display::text_color;
        let dark = RenderOptions { dark: true, ..Default::default() };
        let light = RenderOptions::default();
        let colours: Vec<Rgb> = Highlight::ALL.iter().filter_map(|h| h.rgb()).collect();
        assert_eq!(colours.len(), 16);
        let paper = dark.page_background(None);
        for c in colours {
            let bg = dark.background(c);
            for (what, tc) in [("automatic", text_color(&TextColor::Auto, Some(c))), ("black", Rgb::BLACK)] {
                let fg = dark.text(tc, paper, Some(bg));
                assert_eq!(fg, Rgb::WHITE, "{what} text on {c:?} draws white");
                let k = contrast_ratio(fg, bg);
                assert!(k >= 4.5, "{what} text on {c:?}: {fg:?} on {bg:?} has contrast {k:.2}");
            }
            assert_eq!(light.background(c), c, "light page keeps {c:?}");
            assert_eq!(light.text(c, Rgb::WHITE, Some(c)), c, "light page keeps text {c:?}");
        }
        // Yellow turns a dim olive, still yellow in hue.
        let Rgb(r, g, b) = dark.background(Rgb(255, 255, 0));
        assert!(r < 0x60 && relative_luminance(Rgb(r, g, b)) <= DARK_BACKGROUND_LUMINANCE + 1e-3, "yellow → {:?}", (r, g, b));
        assert!(r == g && g > b);
        // Text off a highlight starts from the plain inversion, then reaches 4.5:1 on the paper:
        // white text on white paper shows light grey rather than vanishing into the dark paper.
        let white = dark.text(Rgb::WHITE, paper, None);
        assert!(contrast_ratio(white, paper) >= 4.5 && white != Rgb::WHITE, "{white:?}");
        assert_eq!(dark.text(Rgb::BLACK, paper, None), dark.ink(Rgb::BLACK));
    }

    /// #1252 review: explicit coloured text (pure blue, red, every highlight colour) on every
    /// dimmed highlight still reaches 4.5:1 on a dark page.
    #[test]
    fn dark_page_coloured_text_on_highlights_reads() {
        use wordcraft_doc::props::Highlight;
        let dark = RenderOptions { dark: true, ..Default::default() };
        let colours: Vec<Rgb> = Highlight::ALL.iter().filter_map(|h| h.rgb()).collect();
        let texts: Vec<Rgb> = [Rgb(0, 0, 255), Rgb(255, 0, 0)].into_iter().chain(colours.iter().copied()).collect();
        let paper = dark.page_background(None);
        for &h in &colours {
            let bg = dark.background(h);
            for &tc in &texts {
                let fg = dark.text(tc, paper, Some(bg));
                let k = contrast_ratio(fg, bg);
                assert!(k >= 4.5, "text {tc:?} on {h:?}: {fg:?} on {bg:?} has contrast {k:.2}");
            }
        }
    }

    /// #1252 review: an Automatic underline on a dark highlight (black) draws light on a dark page,
    /// like the text it underlines, not the dark grey plain inversion gives white.
    #[test]
    fn dark_page_underline_on_a_dark_highlight_reads() {
        use wordcraft_doc::props::{Highlight, Underline};
        let cp =
            wordcraft_doc::CharProps { highlight: Some(Highlight::Black), underline: Some(Underline::Thick), size: Some(24.0), ..Default::default() };
        let mut d = Document::from_text("x");
        d.body = vec![wordcraft_doc::para_block(wordcraft_doc::Paragraph::with_text("HHHH HHHH", cp))];
        let l = layout(&d, &mut LayoutCache::new(), &LayoutOptions::default());
        let items = page_display(&d, &l.pages[0], &DisplayOptions::default());
        let (x0, x1, y, w) = items
            .iter()
            .find_map(|i| match i {
                Draw::Line { x0, x1, y0, width, .. } => Some((*x0, *x1, *y0, *width)),
                _ => None,
            })
            .unwrap();
        assert!(w >= 1.5, "thick underline {w}");
        let (px, py) = (((x0 + x1) / 2.0) as u32, y.round() as u32);
        let dark = render_page(&d, &l.pages[0], 4.0, &RenderOptions { dark: true, ..Default::default() });
        let [r8, g8, b8, _] = dark.pixel(px * 4, py * 4);
        assert!(r8 > 0xC0 && g8 > 0xC0 && b8 > 0xC0, "underline draws light, got {:?}", (r8, g8, b8));
    }

    /// #2534 review: Automatic text with a legacy shadow (drawn from its outlines) on a black
    /// highlight draws light on a dark page, like plain text does, not dark grey on near-black.
    #[test]
    fn dark_page_shadowed_text_on_a_dark_highlight_reads() {
        use wordcraft_doc::props::Highlight;
        let cp = wordcraft_doc::CharProps { highlight: Some(Highlight::Black), shadow: Some(true), size: Some(24.0), ..Default::default() };
        let mut d = Document::from_text("x");
        d.body = vec![wordcraft_doc::para_block(wordcraft_doc::Paragraph::with_text("HHHH HHHH", cp))];
        let l = layout(&d, &mut LayoutCache::new(), &LayoutOptions::default());
        let items = page_display(&d, &l.pages[0], &DisplayOptions::default());
        let b = items
            .iter()
            .find_map(|i| match i {
                Draw::Art { path, text_bg, .. } => {
                    assert!(text_bg.is_some(), "the run's highlight is the letters' background");
                    wordcraft_layout::wordart::path_box(path)
                }
                _ => None,
            })
            .unwrap();
        let dark = render_page(&d, &l.pages[0], 2.0, &RenderOptions { dark: true, ..Default::default() });
        let light = (b.x0 as u32 * 2..b.x1 as u32 * 2)
            .flat_map(|x| (b.y0 as u32 * 2..b.y1 as u32 * 2).map(move |y| (x, y)))
            .filter(|&(x, y)| dark.pixel(x, y)[..3].iter().all(|&v| v > 0xE0))
            .count();
        assert!(light > 200, "letters draw light, {light} light pixels");
    }

    /// One EMF record: type, size, payload (a multiple of 4 bytes).
    fn emf_rec(typ: u32, payload: &[u8]) -> Vec<u8> {
        let mut v = typ.to_le_bytes().to_vec();
        v.extend(((8 + payload.len()) as u32).to_le_bytes());
        v.extend(payload);
        v
    }

    fn le32(vals: &[i32]) -> Vec<u8> {
        vals.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    /// A 25 mm square EMF (device units 0.04 per 0.01 mm, so 100 device units) with a red rectangle over
    /// its right half.
    fn red_right_half_emf() -> Vec<u8> {
        let mut h = vec![0u8; 88];
        h[0..4].copy_from_slice(&1u32.to_le_bytes());
        h[4..8].copy_from_slice(&88u32.to_le_bytes());
        h[8..24].copy_from_slice(&le32(&[0, 0, 100, 100]));
        h[24..40].copy_from_slice(&le32(&[0, 0, 2500, 2500]));
        h[40..44].copy_from_slice(&0x464D_4520u32.to_le_bytes());
        h[72..88].copy_from_slice(&le32(&[1000, 1000, 250, 250]));
        let brush = emf_rec(39, &le32(&[1, 0, 0x0000_00FF, 0]));
        let select = emf_rec(37, &le32(&[1]));
        let rect = emf_rec(43, &le32(&[50, 0, 100, 100]));
        let eof = emf_rec(14, &[0; 12]);
        [h, brush, select, rect, eof].concat()
    }

    #[test]
    fn metafile_picture_is_drawn_as_vectors() {
        let mut d = Document::new();
        let media = d.add_media(red_right_half_emf(), "emf");
        let mut p = wordcraft_doc::Paragraph::new();
        let obj = wordcraft_doc::para::InlineObject::Image {
            media,
            w: 70.0,
            h: 70.0,
            alt: String::new(),
            float: Default::default(),
            crop: [0.0; 4],
            ole: None,
        };
        p.insert_object(0, obj, &Default::default()).unwrap();
        d.body = vec![wordcraft_doc::para_block(p)];
        let l = layout(&d, &mut LayoutCache::new(), &LayoutOptions::default());
        let opts = RenderOptions::default();
        let rect = page_display(&d, &l.pages[0], &opts.display)
            .iter()
            .find_map(|i| match i {
                Draw::Image { rect, .. } => Some(*rect),
                _ => None,
            })
            .expect("a picture on the page");
        let img = render_page(&d, &l.pages[0], 1.0, &opts);
        let y = (rect.y + rect.h / 2.0) as u32;
        let right = img.pixel((rect.x + rect.w * 0.75) as u32, y);
        let left = img.pixel((rect.x + rect.w * 0.25) as u32, y);
        assert!(right[0] > 200 && right[1] < 60 && right[2] < 60, "right half should be red: {right:?}");
        assert!(left[..3].iter().all(|c| *c > 200), "left half should be paper, not the grey box: {left:?}");
    }

    #[test]
    fn cosmetic_metafile_pen_is_one_device_pixel_wide() {
        // MOVETO (0, 50), LINETO (100, 50) with the default cosmetic black pen: a hairline across the middle.
        let mut emf = red_right_half_emf()[..88].to_vec();
        emf.extend([emf_rec(27, &le32(&[0, 50])), emf_rec(54, &le32(&[100, 50])), emf_rec(14, &[0; 12])].concat());
        let mut d = Document::new();
        let media = d.add_media(emf, "emf");
        let mut p = wordcraft_doc::Paragraph::new();
        let obj = wordcraft_doc::para::InlineObject::Image {
            media,
            w: 70.0,
            h: 70.0,
            alt: String::new(),
            float: Default::default(),
            crop: [0.0; 4],
            ole: None,
        };
        p.insert_object(0, obj, &Default::default()).unwrap();
        d.body = vec![wordcraft_doc::para_block(p)];
        let l = layout(&d, &mut LayoutCache::new(), &LayoutOptions::default());
        let opts = RenderOptions::default();
        let rect = page_display(&d, &l.pages[0], &opts.display)
            .iter()
            .find_map(|i| match i {
                Draw::Image { rect, .. } => Some(*rect),
                _ => None,
            })
            .expect("a picture on the page");
        let scale = 2.0;
        let img = render_page(&d, &l.pages[0], scale, &opts);
        let (x, y) = (((rect.x + rect.w / 2.0) * scale) as u32, ((rect.y + rect.h / 2.0) * scale) as u32);
        let darkest = (y - 3..=y + 3).map(|y| img.pixel(x, y)[0]).min().unwrap_or(255);
        // A 0.1 pt line would cover a fifth of a pixel (about 204); one device pixel covers at least half of one.
        assert!(darkest < 140, "the hairline should be clearly visible: {darkest}");
    }

    #[test]
    fn unreadable_metafile_keeps_the_grey_box() {
        let mut d = Document::new();
        // The header alone has no records, so the metafile does not parse.
        let media = d.add_media(red_right_half_emf()[..88].to_vec(), "emf");
        let mut p = wordcraft_doc::Paragraph::new();
        let obj = wordcraft_doc::para::InlineObject::Image {
            media,
            w: 70.0,
            h: 70.0,
            alt: String::new(),
            float: Default::default(),
            crop: [0.0; 4],
            ole: None,
        };
        p.insert_object(0, obj, &Default::default()).unwrap();
        d.body = vec![wordcraft_doc::para_block(p)];
        let l = layout(&d, &mut LayoutCache::new(), &LayoutOptions::default());
        let opts = RenderOptions::default();
        let rect = page_display(&d, &l.pages[0], &opts.display)
            .iter()
            .find_map(|i| match i {
                Draw::Image { rect, .. } => Some(*rect),
                _ => None,
            })
            .expect("a picture on the page");
        let img = render_page(&d, &l.pages[0], 1.0, &opts);
        assert_eq!(img.pixel((rect.x + rect.w / 2.0) as u32, (rect.y + rect.h / 2.0) as u32), [0xD0, 0xD0, 0xD0, 255]);
    }

    #[test]
    fn shapes_have_paths() {
        let r = kurbo::Rect::new(0.0, 0.0, 10.0, 10.0);
        for k in
            [ShapeKind::Rectangle, ShapeKind::Ellipse, ShapeKind::FlowchartOr, ShapeKind::Star, ShapeKind::Heart, ShapeKind::Arrow, ShapeKind::Line]
        {
            assert!(!shape_path(k, r, None).elements().is_empty());
        }
    }

    #[test]
    fn dashed_outline_has_gaps() {
        use wordcraft_doc::{
            connector::ShapeExtra,
            line_dash::LineDash,
            para::{Anchor, Float, InlineObject, Wrap},
        };
        let image = |dash| {
            let shape = InlineObject::Shape {
                kind: ShapeKind::Rectangle,
                w: 70.0,
                h: 28.0,
                fill: None,
                stroke: Some(Rgb(192, 0, 0)),
                stroke_width: 1.0,
                float: Float { wrap: Wrap::InFrontOfText, h_rel: Anchor::Page, v_rel: Anchor::Page, x: 100.0, y: 100.0, ..Default::default() },
                story: None,
                freeform: None,
                effects: Default::default(),
                extra: ShapeExtra { dash, ..Default::default() },
            };
            let mut p = wordcraft_doc::Paragraph::new();
            p.insert_object(0, shape, &Default::default()).unwrap();
            let mut d = Document::new();
            d.body = vec![wordcraft_doc::para_block(p)];
            let l = layout(&d, &mut LayoutCache::new(), &LayoutOptions::default());
            render_page(&d, &l.pages[0], 4.0, &RenderOptions::default())
        };
        let solid = image(LineDash::Solid);
        let dashed = image(LineDash::Dash);
        assert!(solid.pixel(422, 400)[1] < 80);
        assert_eq!(dashed.pixel(422, 400), [255, 255, 255, 255]);
        assert!(dashed.pixel(406, 400)[1] < 80);
    }

    #[test]
    fn zero_width_connector_dashes() {
        use wordcraft_doc::{
            connector::ShapeExtra,
            line_dash::LineDash,
            para::{Anchor, Float, InlineObject, Wrap},
        };
        let shape = InlineObject::Shape {
            kind: ShapeKind::ElbowConnector,
            w: 70.0,
            h: 28.0,
            fill: None,
            stroke: Some(Rgb(192, 0, 0)),
            stroke_width: 0.0,
            float: Float { wrap: Wrap::InFrontOfText, h_rel: Anchor::Page, v_rel: Anchor::Page, x: 100.0, y: 100.0, ..Default::default() },
            story: None,
            freeform: None,
            effects: Default::default(),
            extra: ShapeExtra { dash: LineDash::Dash, ..Default::default() },
        };
        let mut p = wordcraft_doc::Paragraph::new();
        p.insert_object(0, shape, &Default::default()).unwrap();
        let mut d = Document::new();
        d.body = vec![wordcraft_doc::para_block(p)];
        let l = layout(&d, &mut LayoutCache::new(), &LayoutOptions::default());
        let image = render_page(&d, &l.pages[0], 8.0, &RenderOptions::default());
        // A 0.75 pt stroke has a 3 pt dash followed by a 2.25 pt gap.
        assert!(image.pixel(812, 800)[1] < 80, "inside the first dash");
        assert_eq!(image.pixel(832, 800), [255, 255, 255, 255], "inside the first gap");
    }

    #[test]
    fn flowchart_or_spokes() {
        let r = kurbo::Rect::new(0.0, 0.0, 10.0, 10.0);
        let path = shape_path(ShapeKind::FlowchartOr, r, None);
        let elements = path.elements();
        for pair in [
            [kurbo::PathEl::MoveTo((0.0, 5.0).into()), kurbo::PathEl::LineTo((10.0, 5.0).into())],
            [kurbo::PathEl::MoveTo((5.0, 0.0).into()), kurbo::PathEl::LineTo((5.0, 10.0).into())],
        ] {
            assert!(elements.windows(2).any(|w| w == pair), "missing spoke {pair:?}");
        }
    }

    /// A rounded rectangle laid out and rendered: its corner pixel is painted when the corner is
    /// square, left white with the default corner; turned a quarter, the same.
    #[test]
    fn rendered_rounded_rectangles_follow_their_corner() {
        use wordcraft_doc::para::{Anchor, Float, ShapeKind, Wrap};
        let rendered = |corner: Option<u32>, rot: f32| {
            let float = Float { wrap: Wrap::InFrontOfText, h_rel: Anchor::Page, v_rel: Anchor::Page, x: 100.0, y: 100.0, rot, ..Default::default() };
            let shape = wordcraft_doc::para::InlineObject::Shape {
                kind: ShapeKind::RoundedRectangle,
                w: 200.0,
                h: 200.0,
                fill: Some(wordcraft_doc::props::Rgb(0, 0, 0)),
                stroke: None,
                stroke_width: 0.0,
                float,
                story: None,
                freeform: None,
                effects: Default::default(),
                extra: wordcraft_doc::connector::ShapeExtra { corner, ..Default::default() },
            };
            let mut p = wordcraft_doc::Paragraph::new();
            p.insert_object(0, shape, &Default::default()).unwrap();
            let mut d = Document::new();
            d.body = vec![wordcraft_doc::para_block(p)];
            let l = layout(&d, &mut LayoutCache::new(), &LayoutOptions::default());
            render_page(&d, &l.pages[0], 1.0, &RenderOptions::default())
        };
        for rot in [0.0, 90.0] {
            // 3 pt in from the frame's top-left corner, and its middle.
            let (corner, middle) = ((103, 103), (200, 200));
            let square = rendered(Some(0), rot);
            assert_eq!(square.pixel(corner.0, corner.1)[0], 0, "square corner, turned {rot}");
            let rounded = rendered(None, rot);
            assert_eq!(rounded.pixel(corner.0, corner.1)[0], 255, "default corner, turned {rot}");
            assert_eq!(rounded.pixel(middle.0, middle.1)[0], 0, "filled, turned {rot}");
        }
    }

    /// A rounded rectangle's corner is its file's (`roundRect`'s `adj`), not always 16%.
    #[test]
    fn rounded_rectangle_corners_follow_their_radius() {
        use kurbo::Shape;
        let r = kurbo::Rect::new(0.0, 0.0, 200.0, 100.0);
        let inside = |corner, pt: (f64, f64)| shape_path(ShapeKind::RoundedRectangle, r, corner).contains(pt.into());
        // Two points inside the top-left corner: 1 pt and 4 pt in from both edges.
        let (tip, near) = ((1.0, 1.0), (4.0, 4.0));
        assert!(inside(Some(0), tip), "square corner");
        assert!(!inside(Some(3_565), tip) && inside(Some(3_565), near), "3.565 pt corner");
        assert!(!inside(None, near), "16 pt corner by default");
        assert!(!inside(Some(u32::MAX), (14.0, 14.0)) && inside(Some(u32::MAX), (100.0, 1.0)), "round ends");
    }

    /// #611: a line runs as DrawingML's `line` preset does, top-left to bottom-right (Word's
    /// files flip it for the other diagonal), not bottom-left to top-right.
    #[test]
    fn a_line_runs_from_top_left_to_bottom_right() {
        let p = shape_path(ShapeKind::Line, kurbo::Rect::new(10.0, 20.0, 30.0, 60.0), None);
        assert_eq!(p.elements(), &[kurbo::PathEl::MoveTo((10.0, 20.0).into()), kurbo::PathEl::LineTo((30.0, 60.0).into())]);
    }
}

#[cfg(test)]
mod cache_tests;
#[cfg(test)]
mod tests_color_glyphs;
