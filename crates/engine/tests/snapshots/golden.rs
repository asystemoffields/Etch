//! Golden files: where they live, how a snapshot is compared with its golden, how a difference
//! is reported, and how a golden is blessed.
//!
//! - `WC_BLESS=1` rewrites the goldens of every snapshot the test run reaches (a test filter
//!   picks the family, `WC_SNAPSHOT_DOC=a,b` the documents).
//! - A failure prints every differing hunk (the first few in full, with context and the page,
//!   part or step it falls in), the counts, where the actual output was written, and the bless
//!   command for exactly that family and document.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Where the goldens are checked in.
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("goldens")
}

/// `WC_BLESS` is set (and not `0`): write the goldens instead of checking them.
pub fn blessing() -> bool {
    std::env::var("WC_BLESS").is_ok_and(|v| !v.is_empty() && v != "0")
}

/// `WC_SNAPSHOT_DOC=a,b`: only these documents (or scripts) are checked or blessed.
pub fn wanted(name: &str) -> bool {
    match std::env::var("WC_SNAPSHOT_DOC") {
        Ok(v) if !v.trim().is_empty() => v.split(',').map(str::trim).any(|w| w == name || name.starts_with(&format!("{w}."))),
        _ => true,
    }
}

/// Where the actual output of a failing (or every, with `WC_SNAPSHOT_OUT`) snapshot is written,
/// so it can be diffed with a tool or looked at.
pub fn out_dir() -> PathBuf {
    match std::env::var_os("WC_SNAPSHOT_OUT") {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => std::env::temp_dir().join("wordcraft-snapshots"),
    }
}

fn write_out(family: &str, file: &str, bytes: &[u8]) -> Option<PathBuf> {
    let dir = out_dir().join(family);
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(file);
    std::fs::write(&path, bytes).ok()?;
    Some(path)
}

/// The failures of one family, reported together at the end so one run shows every document
/// that changed.
#[derive(Default)]
pub struct Failures {
    pub items: Vec<String>,
    pub checked: usize,
    pub blessed: usize,
}

impl Failures {
    pub fn push(&mut self, s: String) {
        self.items.push(s);
    }

    /// Panics with every failure when there is one; prints what was checked otherwise.
    pub fn finish(self, family: &str) {
        if self.blessed > 0 {
            eprintln!("snapshots/{family}: blessed {} golden file(s) under {}", self.blessed, root().display());
        }
        if self.items.is_empty() {
            eprintln!("snapshots/{family}: {} snapshot(s) match their goldens", self.checked);
            return;
        }
        let mut msg = format!("\n{} {family} snapshot(s) differ from their goldens:\n", self.items.len());
        for it in &self.items {
            msg.push_str(it);
            msg.push('\n');
        }
        let _ = writeln!(
            msg,
            "To accept every change in this family: WC_BLESS=1 cargo test -p wordcraft-engine --test 'snapshots*' {family}_snapshots\n\
             (then read `git diff crates/engine/tests/goldens/` before committing)."
        );
        panic!("{msg}");
    }
}

/// The bless command for one snapshot of a family.
fn bless_cmd(family: &str, name: &str) -> String {
    format!("WC_BLESS=1 WC_SNAPSHOT_DOC={name} cargo test -p wordcraft-engine --test 'snapshots*' {family}_snapshots")
}

/// A text snapshot: `tests/goldens/<family>/<name>.txt`.
pub fn check_text(fails: &mut Failures, family: &str, name: &str, actual: &str) {
    let path = root().join(family).join(format!("{name}.txt"));
    if blessing() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, actual).unwrap();
        fails.blessed += 1;
        return;
    }
    fails.checked += 1;
    let rel = format!("{family}/{name}.txt");
    let expected = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => {
            let out = write_out(family, &format!("{name}.txt"), actual.as_bytes());
            fails.push(format!(
                "--- {rel}: no golden file yet ({} lines of output). Actual written to {}.\n    bless: {}",
                actual.lines().count(),
                out.map(|p| p.display().to_string()).unwrap_or_default(),
                bless_cmd(family, name)
            ));
            return;
        }
    };
    if std::env::var_os("WC_SNAPSHOT_OUT").is_some() {
        write_out(family, &format!("{name}.txt"), actual.as_bytes());
    }
    if expected == actual {
        return;
    }
    let out = write_out(family, &format!("{name}.txt"), actual.as_bytes());
    let mut msg = text_diff(&rel, &expected, actual);
    let _ = writeln!(msg, "    actual output: {}", out.map(|p| p.display().to_string()).unwrap_or_default());
    let _ = write!(msg, "    bless: {}", bless_cmd(family, name));
    fails.push(msg);
}

/// A line of a dump that names the region the lines after it belong to (a layout page, a saved
/// part, a script step, a model section).
fn is_heading(line: &str) -> bool {
    line.starts_with("page ") || line.starts_with("== ") || line.starts_with("step ") || line.starts_with("# ")
}

fn heading_before(lines: &[&str], i: usize) -> Option<String> {
    lines.get(..i.min(lines.len()))?.iter().rev().find(|l| is_heading(l)).map(|l| l.trim_end().to_string())
}

/// Hunks the report shows in full.
const SHOWN_HUNKS: usize = 4;
/// Lines of context around a hunk.
const CONTEXT: usize = 2;
/// Longest lines the report prints whole.
const LINE_CAP: usize = 220;
/// The diff falls back to a prefix/suffix report above this many changed lines on a side.
const DIFF_CAP: usize = 6000;

fn clip(s: &str) -> String {
    if s.chars().count() <= LINE_CAP {
        return s.to_string();
    }
    let cut: String = s.chars().take(LINE_CAP).collect();
    format!("{cut}…")
}

/// One edit: lines `a0..a1` of the expected text became lines `b0..b1` of the actual one.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Hunk {
    a0: usize,
    a1: usize,
    b0: usize,
    b1: usize,
}

/// The hunks between two line lists: common prefix and suffix dropped, then a Myers diff of
/// what is left (or one hunk for the whole middle when it is too long to diff cheaply).
fn hunks(a: &[&str], b: &[&str]) -> Vec<Hunk> {
    let pre = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let suf = a[pre..].iter().rev().zip(b[pre..].iter().rev()).take_while(|(x, y)| x == y).count();
    let (am, bm) = (&a[pre..a.len() - suf], &b[pre..b.len() - suf]);
    if am.is_empty() && bm.is_empty() {
        return Vec::new();
    }
    if am.len() > DIFF_CAP || bm.len() > DIFF_CAP {
        return vec![Hunk { a0: pre, a1: pre + am.len(), b0: pre, b1: pre + bm.len() }];
    }
    let mut out = Vec::new();
    for h in myers(am, bm) {
        out.push(Hunk { a0: h.a0 + pre, a1: h.a1 + pre, b0: h.b0 + pre, b1: h.b1 + pre });
    }
    out
}

/// Myers' O(ND) line diff, returned as hunks of changed line ranges (neighbouring edits merged).
fn myers(a: &[&str], b: &[&str]) -> Vec<Hunk> {
    let (n, m) = (a.len(), b.len());
    let max = n + m;
    let off = max;
    let mut v = vec![0usize; 2 * max + 2];
    let mut trace: Vec<Vec<usize>> = Vec::new();
    let mut found = false;
    for d in 0..=max {
        trace.push(v.clone());
        let mut k = -(d as isize);
        while k <= d as isize {
            let idx = (k + off as isize) as usize;
            let mut x = if k == -(d as isize) || (k != d as isize && v[idx - 1] < v[idx + 1]) { v[idx + 1] } else { v[idx - 1] + 1 };
            let mut y = (x as isize - k) as usize;
            while x < n && y < m && a[x] == b[y] {
                x += 1;
                y += 1;
            }
            v[idx] = x;
            if x >= n && y >= m {
                found = true;
                break;
            }
            k += 2;
        }
        if found {
            break;
        }
    }
    // Walk the trace back to list the edits as (kind, a index, b index).
    let (mut x, mut y) = (n, m);
    let mut edits: Vec<(char, usize, usize)> = Vec::new();
    for d in (1..trace.len()).rev() {
        let v = &trace[d];
        let k = x as isize - y as isize;
        let idx = (k + off as isize) as usize;
        let prev_k = if k == -(d as isize) || (k != d as isize && v[idx - 1] < v[idx + 1]) { k + 1 } else { k - 1 };
        let pidx = (prev_k + off as isize) as usize;
        let prev_x = v[pidx];
        let prev_y = (prev_x as isize - prev_k) as usize;
        while x > prev_x && y > prev_y {
            x -= 1;
            y -= 1;
        }
        if x == prev_x {
            edits.push(('+', x, prev_y));
        } else {
            edits.push(('-', prev_x, y));
        }
        x = prev_x;
        y = prev_y;
    }
    edits.reverse();
    // Merge runs of edits into hunks.
    let mut out: Vec<Hunk> = Vec::new();
    let (mut ai, mut bi) = (0usize, 0usize);
    for (kind, ex, ey) in edits {
        // Common lines before this edit.
        let (ca, cb) = (ex.saturating_sub(ai), ey.saturating_sub(bi));
        let common = ca.min(cb);
        ai += common;
        bi += common;
        let (da, db) = if kind == '-' { (1, 0) } else { (0, 1) };
        match out.last_mut() {
            Some(h) if h.a1 == ai && h.b1 == bi => {
                h.a1 += da;
                h.b1 += db;
            }
            _ => out.push(Hunk { a0: ai, a1: ai + da, b0: bi, b1: bi + db }),
        }
        ai += da;
        bi += db;
    }
    out
}

/// The lines that changed from `before` to `after`, as `- ` and `+ ` lines in order (what the
/// command snapshots record for each step).
pub fn changed_lines(before: &str, after: &str) -> String {
    let a: Vec<&str> = before.lines().collect();
    let b: Vec<&str> = after.lines().collect();
    let mut out = String::new();
    for h in hunks(&a, &b) {
        for l in a.iter().take(h.a1).skip(h.a0) {
            let _ = writeln!(out, "  - {l}");
        }
        for l in b.iter().take(h.b1).skip(h.b0) {
            let _ = writeln!(out, "  + {l}");
        }
    }
    out
}

/// A readable report of how `actual` differs from `expected`.
pub fn text_diff(what: &str, expected: &str, actual: &str) -> String {
    let a: Vec<&str> = expected.lines().collect();
    let b: Vec<&str> = actual.lines().collect();
    let hs = hunks(&a, &b);
    let removed: usize = hs.iter().map(|h| h.a1 - h.a0).sum();
    let added: usize = hs.iter().map(|h| h.b1 - h.b0).sum();
    let mut msg = format!("--- {what}: {} hunk(s), -{removed} +{added} lines (golden {} lines, actual {} lines)\n", hs.len(), a.len(), b.len());
    for (i, h) in hs.iter().enumerate() {
        let region = heading_before(&b, h.b0).or_else(|| heading_before(&a, h.a0));
        if i >= SHOWN_HUNKS {
            let _ = writeln!(
                msg,
                "    @@ golden {}-{} / actual {}-{} @@{}",
                h.a0 + 1,
                h.a1,
                h.b0 + 1,
                h.b1,
                region.map(|r| format!("  in: {r}")).unwrap_or_default()
            );
            continue;
        }
        let _ = writeln!(
            msg,
            "    @@ golden {}-{} / actual {}-{} @@{}",
            h.a0 + 1,
            h.a1,
            h.b0 + 1,
            h.b1,
            region.map(|r| format!("  in: {r}")).unwrap_or_default()
        );
        let c0 = h.b0.saturating_sub(CONTEXT);
        for (j, l) in b.iter().enumerate().take(h.b0).skip(c0) {
            let _ = writeln!(msg, "      {:>5}  {}", j + 1, clip(l));
        }
        let shown_a = (h.a1 - h.a0).min(40);
        for (j, l) in a.iter().enumerate().take(h.a0 + shown_a).skip(h.a0) {
            let _ = writeln!(msg, "    - {:>5}  {}", j + 1, clip(l));
        }
        if h.a1 - h.a0 > shown_a {
            let _ = writeln!(msg, "    - … {} more golden lines", h.a1 - h.a0 - shown_a);
        }
        let shown_b = (h.b1 - h.b0).min(40);
        for (j, l) in b.iter().enumerate().take(h.b0 + shown_b).skip(h.b0) {
            let _ = writeln!(msg, "    + {:>5}  {}", j + 1, clip(l));
        }
        if h.b1 - h.b0 > shown_b {
            let _ = writeln!(msg, "    + … {} more actual lines", h.b1 - h.b0 - shown_b);
        }
        for (j, l) in b.iter().enumerate().take((h.b1 + CONTEXT).min(b.len())).skip(h.b1) {
            let _ = writeln!(msg, "      {:>5}  {}", j + 1, clip(l));
        }
    }
    if hs.len() > SHOWN_HUNKS {
        let _ = writeln!(msg, "    ({} hunks not shown in full)", hs.len() - SHOWN_HUNKS);
    }
    msg
}

/// Channel difference a pixel may have before it counts as changed: rasterisation rounding, not
/// a change in what is drawn (a missing or moved line changes pixels by far more).
pub const PIXEL_TOLERANCE: u8 = 3;

/// The page as a PNG with the slowest, smallest encoding (straight RGBA).
fn small_png(image: &wordcraft_render::Rendered) -> Option<Vec<u8>> {
    use image::ImageEncoder as _;
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    let mut buf = Vec::new();
    let enc = PngEncoder::new_with_quality(&mut buf, CompressionType::Best, FilterType::Adaptive);
    enc.write_image(&image.to_straight(), image.width, image.height, image::ExtendedColorType::Rgba8).ok()?;
    Some(buf)
}

/// A page image snapshot: `tests/goldens/<family>/<name>.png` (straight RGBA; compared pixel by
/// pixel against the golden decoded, so the PNG encoder's choices don't matter).
pub fn check_png(fails: &mut Failures, family: &str, name: &str, image: &wordcraft_render::Rendered) {
    let path = root().join(family).join(format!("{name}.png"));
    let png = match image.to_png() {
        Ok(p) => p,
        Err(e) => {
            fails.push(format!("--- {family}/{name}.png: can't encode the page: {e}"));
            return;
        }
    };
    if blessing() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        // The golden is compared by its pixels, so it is written as small as the encoder can.
        std::fs::write(&path, small_png(image).unwrap_or(png)).unwrap();
        fails.blessed += 1;
        return;
    }
    fails.checked += 1;
    let rel = format!("{family}/{name}.png");
    let golden = match std::fs::read(&path) {
        Ok(b) => b,
        Err(_) => {
            let out = write_out(family, &format!("{name}.actual.png"), &png);
            fails.push(format!(
                "--- {rel}: no golden image yet ({}x{}). Actual written to {}.\n    bless: {} (and add a row for the new file to ATTRIBUTION.md)",
                image.width,
                image.height,
                out.map(|p| p.display().to_string()).unwrap_or_default(),
                bless_cmd(family, name)
            ));
            return;
        }
    };
    if std::env::var_os("WC_SNAPSHOT_OUT").is_some() {
        write_out(family, &format!("{name}.actual.png"), &png);
    }
    let decoded = match image::load_from_memory(&golden) {
        Ok(i) => i.to_rgba8(),
        Err(e) => {
            fails.push(format!("--- {rel}: the golden isn't a readable PNG: {e}\n    bless: {}", bless_cmd(family, name)));
            return;
        }
    };
    let actual = image.to_straight();
    if (decoded.width(), decoded.height()) != (image.width, image.height) {
        let out = write_out(family, &format!("{name}.actual.png"), &png);
        fails.push(format!(
            "--- {rel}: size changed: golden {}x{}, actual {}x{} (the page size or the scale changed).\n    actual image: {}\n    bless: {}",
            decoded.width(),
            decoded.height(),
            image.width,
            image.height,
            out.map(|p| p.display().to_string()).unwrap_or_default(),
            bless_cmd(family, name)
        ));
        return;
    }
    let (w, h) = (image.width as usize, image.height as usize);
    let exp = decoded.as_raw();
    let mut changed = 0usize;
    let mut max = 0u8;
    let mut first: Option<(usize, usize)> = None;
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
    let mut diff = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 4;
            let d = (0..4).map(|c| exp[i + c].abs_diff(actual[i + c])).max().unwrap_or(0);
            if d > max {
                max = d;
            }
            let ch = d > PIXEL_TOLERANCE;
            // The diff image: changed pixels red on a faint copy of the actual page.
            let (r, g, b) = if ch { (255, 0, 0) } else { (192 + actual[i] / 4, 192 + actual[i + 1] / 4, 192 + actual[i + 2] / 4) };
            diff[i] = r;
            diff[i + 1] = g;
            diff[i + 2] = b;
            diff[i + 3] = 255;
            if ch {
                changed += 1;
                first.get_or_insert((x, y));
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    if changed == 0 {
        return;
    }
    let out = write_out(family, &format!("{name}.actual.png"), &png);
    let diff_img = wordcraft_render::Rendered { width: image.width, height: image.height, pixels: diff };
    let diff_out = diff_img.to_png().ok().and_then(|p| write_out(family, &format!("{name}.diff.png"), &p));
    let (fx, fy) = first.unwrap_or((0, 0));
    let fi = (fy * w + fx) * 4;
    let hex = |p: &[u8]| format!("#{:02x}{:02x}{:02x}", p[0], p[1], p[2]);
    fails.push(format!(
        "--- {rel}: {changed} of {} pixels differ by more than {PIXEL_TOLERANCE} (max channel difference {max}) inside x {x0}-{x1}, y {y0}-{y1} of {w}x{h}; \
         first at ({fx},{fy}): golden {} actual {}\n    actual image: {}\n    changed pixels in red: {}\n    bless: {}",
        w * h,
        hex(&exp[fi..fi + 4]),
        hex(&actual[fi..fi + 4]),
        out.map(|p| p.display().to_string()).unwrap_or_default(),
        diff_out.map(|p| p.display().to_string()).unwrap_or_default(),
        bless_cmd(family, name)
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_finds_changed_and_inserted_lines() {
        let a = "page 1\na\nb\nc\nd\n";
        let b = "page 1\na\nB\nc\nd\ne\n";
        let hs = hunks(&a.lines().collect::<Vec<_>>(), &b.lines().collect::<Vec<_>>());
        assert_eq!(hs, vec![Hunk { a0: 2, a1: 3, b0: 2, b1: 3 }, Hunk { a0: 5, a1: 5, b0: 5, b1: 6 }]);
        let msg = text_diff("x", a, b);
        assert!(msg.contains("in: page 1"), "{msg}");
        assert!(msg.contains("-     3  b") && msg.contains("+     3  B") && msg.contains("+     6  e"), "{msg}");
    }

    #[test]
    fn identical_texts_have_no_hunks() {
        assert!(hunks(&["a", "b"], &["a", "b"]).is_empty());
        assert!(hunks(&[], &[]).is_empty());
    }
}
