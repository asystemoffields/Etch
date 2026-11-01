//! Seeded mutation fuzzing of the RTF, ODT (package and flat `.fodt`), HTML and Markdown readers
//! (and the writers on what they read), from seeds that use notes, annotations, text boxes,
//! fields, nested tables, lists, sections and headers. Never-crash: every mutant opens or is
//! refused without a panic, in bounded time, and what opens saves and opens again. The mutator
//! is a fixed-seed xorshift, so a failure reproduces; `FUZZ_ROUNDS=n` runs each fuzz n times
//! longer (the seed sequence continues, so round 1 is always the default run).
//!
//! Mutations: bit flips, bytes removed or duplicated, the file cut short, tokens of the format
//! spliced in (once or hundreds of times), deep nesting (an opener repeated thousands of times,
//! with or without its closer), huge counts (a number replaced by one far past any limit),
//! bad encodings (invalid UTF-8, a UTF-16 mark, NUL bytes) and unbalanced groups or tags.

use std::io::Write;
use std::time::{Duration, Instant};

/// Longest one mutant may take to open and save again.
const BUDGET: Duration = Duration::from_secs(5);

/// Largest file a writer may make of a mutant (a few KB of input): more is a blow-up.
const MAX_OUTPUT: usize = 64 << 20;

fn rounds() -> usize {
    std::env::var("FUZZ_ROUNDS").ok().and_then(|v| v.parse().ok()).unwrap_or(1).clamp(1, 1000)
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
}

/// What a format's mutations are made of: tokens to splice in, nesting pairs, and the numbers
/// huge counts replace.
struct Shape {
    tokens: &'static [&'static str],
    /// (opener, closer) pairs for deep nesting.
    nests: &'static [(&'static str, &'static str)],
}

const HUGE: &[&str] = &["999999999", "4294967295", "-1", "0", "18446744073709551616", "1e308", "99999999999999999999"];

const BAD_BYTES: &[&[u8]] =
    &[b"\xff\xfe", b"\xfe\xff", b"\xc0\x80", b"\xed\xa0\x80", b"\xf4\x90\x80\x80", b"\x00", b"\xef\xbb\xbf", b"\x80\x80\x80", b"\xe6\x97"];

/// `seed` with a few random edits.
fn mutate(seed: &[u8], shape: &Shape, rng: &mut Rng) -> Vec<u8> {
    let mut b = seed.to_vec();
    for _ in 0..1 + rng.below(6) {
        let at = rng.below(b.len() + 1);
        match rng.below(12) {
            0 => {
                if let Some(x) = b.get_mut(at) {
                    *x ^= 1 << rng.below(8);
                }
            }
            1 => {
                let end = (at + rng.below(64)).min(b.len());
                b.drain(at..end);
            }
            2 => {
                let end = (at + rng.below(256)).min(b.len());
                let chunk = b.get(at..end).unwrap_or(&[]).to_vec();
                let to = rng.below(b.len() + 1);
                b.splice(to..to, chunk);
            }
            // Cut short.
            3 => b.truncate(at),
            // Deep nesting: an opener thousands deep, closed or not.
            4 => {
                if let Some((open, close)) = shape.nests.get(rng.below(shape.nests.len())) {
                    let deep = rng.below(4) == 0;
                    let n = 1 + rng.below(if deep { 3000 } else { 40 });
                    let closed = rng.below(3) != 0;
                    let end = (at + rng.below(200)).min(b.len());
                    if closed {
                        b.splice(end..end, close.repeat(n).into_bytes());
                    }
                    b.splice(at..at, open.repeat(n).into_bytes());
                }
            }
            // Huge counts: a number in the file replaced.
            5 => {
                let digits = b.get(at..).and_then(|rest| rest.iter().position(|c| c.is_ascii_digit())).map(|p| at + p);
                if let Some(s) = digits {
                    let e = s + b.get(s..).map(|r| r.iter().take_while(|c| c.is_ascii_digit()).count()).unwrap_or(0);
                    let huge = HUGE[rng.below(HUGE.len())];
                    b.splice(s..e, huge.bytes());
                }
            }
            // Bad encodings.
            6 => {
                let bad = BAD_BYTES[rng.below(BAD_BYTES.len())];
                b.splice(at..at, bad.iter().copied());
            }
            // An unbalanced opener or closer.
            7 => {
                if let Some((open, close)) = shape.nests.get(rng.below(shape.nests.len())) {
                    let t = if rng.below(2) == 0 { open } else { close };
                    b.splice(at..at, t.bytes());
                }
            }
            _ => {
                let t = shape.tokens[rng.below(shape.tokens.len())];
                let long = rng.below(8) == 0;
                let n = 1 + rng.below(if long { 200 } else { 3 });
                let s = t.repeat(n);
                b.splice(at..at, s.into_bytes());
            }
        }
    }
    b
}

/// Open `bytes` as `ext`; if it opens, save it as `ext` (when it is a format WordCraft writes),
/// RTF and ODT, and open those again.
fn exercise(ext: &str, bytes: &[u8], what: &str) {
    let t = Instant::now();
    if let Some(Ok(doc)) = wordcraft_formats::import(ext, bytes) {
        let outs: &[&str] = match ext {
            "html" => &["html", "rtf", "odt"],
            "md" => &["md", "rtf", "odt"],
            _ => &["rtf", "odt"],
        };
        for out in outs {
            if let Some(Ok(saved)) = wordcraft_formats::export(out, &doc) {
                assert!(saved.len() <= MAX_OUTPUT, "{what}: our {out} of it is {} bytes", saved.len());
                let again = wordcraft_formats::import(out, &saved);
                assert!(matches!(again, Some(Ok(_))), "{what}: our {out} of it doesn't open: {:?}", again.map(|r| r.err()));
            }
        }
    }
    assert!(t.elapsed() < BUDGET, "{what}: took {:?}", t.elapsed());
}

const RTF_SEED: &str = r"{\rtf1\ansi\deff0{\fonttbl{\f0 Arial;}{\f1\fcharset128 MS Mincho;}}{\colortbl;\red255\green0\blue0;}{\*\revtbl {Unknown;}{Ann;}}
{\*\listtable{\list\listtemplateid1{\listlevel\levelnfc0\levelstartat3{\leveltext\'02\'00.;}{\levelnumbers\'01;}}{\listname ;}\listid1}}{\*\listoverridetable{\listoverride\listid1\listoverridecount0\ls1}}
\facingp\fet2\paperw12240\paperh15840
\sectd\pgwsxn12240\pghsxn15840\titlepg
{\header \pard\plain Head {\field{\*\fldinst PAGE}{\fldrslt 1}}\par}
{\headerl \pard\plain Even head\par}{\headerf \pard\plain First head\par}
\pard\plain Body {\b bold}{\super\chftn}{\footnote\pard\plain{\super\chftn} Note {\i text}\par}.
{\*\atrfstart 1}commented{\*\atrfend 1}{{\*\atnid AB}{\*\atnauthor Ann}\chatn{\*\annotation{\*\atnref 1}{\*\atndate 1234567}\pard\plain Remark\par}}
{\super\chftn}{\footnote\ftnalt\pard\plain End note\par}
\pard\plain\rtlpar\qr \u1513?\u1500?\u1493?\u1501?\par
\pard\plain\ls1\ilvl0\sb120\sa60\li720\fi-360\sl360\slmult1 Item {\f1 \'93\'fa\'96\'7b}\par
{\shp{\*\shpinst\shpleft0\shptop0\shpright2000\shpbottom1000{\sp{\sn shapeType}{\sv 202}}{\shptxt\pard\plain Boxed\par}}{\shprslt Boxed}}
{\revised\revauth1 inserted}{\deleted\revauthdel1 gone}\sect
\sectd\lndscpsxn\pgwsxn15840\pghsxn12240\sbkpage
\trowd\cellx3000\cellx6000\pard\intbl outer\cell
\pard\intbl\itap2 in1\nestcell\pard\intbl\itap2 in2\nestcell{\*\nesttableprops\trowd\cellx1000\cellx2000\nestrow}{\nonesttables\par}
\pard\intbl\itap1 after\cell\row
\pard\plain Last\par}";

const RTF_SHAPE: Shape = Shape {
    tokens: &[
        "{",
        "}",
        "\\par ",
        "\\cell ",
        "\\row ",
        "\\nestcell ",
        "\\nestrow ",
        "\\itap2 ",
        "\\itap9 ",
        "\\intbl ",
        "{\\footnote ",
        "\\ftnalt ",
        "{\\*\\annotation ",
        "{\\*\\atrfstart 1}",
        "{\\*\\atrfend 1}",
        "{\\*\\atnref 7}",
        "{\\*\\atndate -5}",
        "{\\shp{\\*\\shpinst ",
        "{\\shptxt ",
        "\\shpright-99999 ",
        "{\\header ",
        "{\\headerl ",
        "\\sect ",
        "\\sectd ",
        "{\\field{\\*\\fldinst PAGE}{\\fldrslt ",
        "{\\*\\nesttableprops ",
        "\\trowd\\cellx100 ",
        "\\u-1? ",
        "\\'ff",
        "\\'93",
        "\\bin5 ",
        "{\\pict\\pngblip ab}",
        "\\ls1\\ilvl8 ",
        "\\ls99999 ",
        "\\levelstartat999999999 ",
        "\\rtlpar ",
        "\\ltrpar ",
        "\\sl-999999 ",
        "\\fi-999999999 ",
        "\\uc9 ",
        "\\f1 ",
        "\\ansicpg932 ",
        "\\fcharset128 ",
        "{\\*\\listtable",
        "{\\listlevel",
    ],
    nests: &[("{", "}"), ("{\\footnote ", "}"), ("{\\shp{\\*\\shpinst{\\shptxt ", "}}}"), ("\\trowd\\cellx100\\pard\\intbl\\itap2 ", "\\nestcell")],
};

fn zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let o = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, data) in entries {
        z.start_file(*name, o).unwrap();
        z.write_all(data).unwrap();
    }
    z.finish().unwrap().into_inner()
}

const NS: &str = r#"xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:meta="urn:oasis:names:tc:opendocument:xmlns:meta:1.0" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0" xmlns:xlink="http://www.w3.org/1999/xlink""#;

/// A 1×1 PNG, base64 (for a flat document's `office:binary-data`).
const PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

fn odt_styles() -> String {
    String::from(
        r#"<office:automatic-styles><style:page-layout style:name="pm1"><style:page-layout-properties fo:page-width="21cm" fo:page-height="29.7cm"/></style:page-layout><style:page-layout style:name="pm2"><style:page-layout-properties fo:page-width="29.7cm" fo:page-height="21cm"/></style:page-layout></office:automatic-styles><office:master-styles><style:master-page style:name="Standard" style:page-layout-name="pm1"><style:header><text:p>Head <text:page-number>1</text:page-number></text:p></style:header><style:header-left><text:p>Left</text:p></style:header-left><style:footer-first><text:p>First foot</text:p></style:footer-first></style:master-page><style:master-page style:name="Wide" style:page-layout-name="pm2"><style:footer><text:p>Wide foot</text:p></style:footer></style:master-page></office:master-styles>"#,
    )
}

fn odt_auto_and_body(image: &str) -> String {
    format!(
        r#"<office:automatic-styles><style:style style:name="PW" style:family="paragraph" style:master-page-name="Wide"/><style:style style:name="PR" style:family="paragraph"><style:paragraph-properties style:writing-mode="rl-tb" fo:margin-left="1cm" fo:text-indent="-0.5cm" fo:line-height="150%"/></style:style><text:list-style style:name="L1"><text:list-level-style-number text:level="1" style:num-format="1"/><text:list-level-style-bullet text:level="2" text:bullet-char="•"/></text:list-style><style:style style:name="T1.A" style:family="table-column"><style:table-column-properties style:column-width="3cm"/></style:style></office:automatic-styles><office:body><office:text><text:p>Body<text:note text:id="n1" text:note-class="footnote"><text:note-citation text:label="*">*</text:note-citation><text:note-body><text:p>Note <text:span>text</text:span></text:p></text:note-body></text:note> and <office:annotation office:name="a1"><dc:creator>Ann</dc:creator><dc:date>2026-01-01T00:00:00</dc:date><meta:creator-initials>A</meta:creator-initials><text:p>Remark</text:p></office:annotation>commented<office:annotation-end office:name="a1"/> words <draw:frame svg:width="3cm" svg:height="2cm"><draw:text-box><text:p>Boxed</text:p></draw:text-box></draw:frame>.</text:p><text:p text:style-name="PR">עברית</text:p><text:list text:style-name="L1"><text:list-item text:start-value="3"><text:p>Three</text:p><text:list><text:list-item><text:p>Bullet</text:p></text:list-item></text:list></text:list-item></text:list><text:list text:style-name="L1" text:continue-numbering="true"><text:list-item><text:p>Four</text:p></text:list-item></text:list><table:table><table:table-column table:style-name="T1.A" table:number-columns-repeated="2"/><table:table-row><table:table-cell><text:p>outer</text:p><table:table><table:table-row><table:table-cell><text:p>inner</text:p></table:table-cell></table:table-row></table:table></table:table-cell><table:table-cell><text:p>pic <draw:frame svg:width="1cm" svg:height="1cm">{image}</draw:frame></text:p></table:table-cell></table:table-row></table:table><text:p text:style-name="PW">Wide page<text:note text:note-class="endnote"><text:note-body><text:p>End</text:p></text:note-body></text:note></text:p></office:text></office:body>"#
    )
}

/// A package's `content.xml` and `styles.xml`.
fn odt_seed() -> (String, String) {
    let styles = format!("<office:document-styles {NS}>{}</office:document-styles>", odt_styles());
    let content =
        format!("<office:document-content {NS}>{}</office:document-content>", odt_auto_and_body(r#"<draw:image xlink:href="Pictures/p.png"/>"#));
    (content, styles)
}

/// The same document as one flat XML file (`.fodt`), its picture in `office:binary-data`.
fn fodt_seed() -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<office:document {NS} office:mimetype=\"application/vnd.oasis.opendocument.text\"><office:meta><dc:title>Flat</dc:title></office:meta>{}{}</office:document>",
        odt_styles(),
        odt_auto_and_body(&format!("<draw:image><office:binary-data>{PNG_B64}</office:binary-data></draw:image>"))
    )
}

fn png() -> Vec<u8> {
    wordcraft_formats::model::base64_decode(PNG_B64).unwrap()
}

const ODT_SHAPE: Shape = Shape {
    tokens: &[
        "<text:p>",
        "</text:p>",
        "<text:note>",
        "</text:note>",
        "<text:note-body>",
        "<office:annotation>",
        "</office:annotation>",
        "<office:annotation-end office:name=\"a1\"/>",
        "<dc:creator>",
        "<draw:frame>",
        "<draw:text-box>",
        "</draw:text-box>",
        "<draw:image><office:binary-data>AAAA</office:binary-data></draw:image>",
        "<office:binary-data>",
        "</office:binary-data>",
        "<text:page-number>",
        "<table:table>",
        "<table:table-cell>",
        "</table:table-cell>",
        "<table:table-column table:number-columns-repeated=\"999999999\"/>",
        "<text:list>",
        "<text:list-item text:start-value=\"4294967295\">",
        "<text:list text:continue-numbering=\"true\">",
        "<style:header>",
        "</style:header>",
        "<style:master-page style:name=\"X\">",
        "<text:s text:c=\"999\"/>",
        "<style:paragraph-properties style:writing-mode=\"rl-tb\" fo:line-height=\"1e308%\"/>",
        "\"",
        "<",
        ">",
        "&amp;",
        "&#x110000;",
        "</office:text>",
        "<office:text>",
        "<office:document>",
        "</office:document>",
    ],
    nests: &[
        ("<text:span>", "</text:span>"),
        ("<text:list><text:list-item>", "</text:list-item></text:list>"),
        ("<table:table><table:table-row><table:table-cell>", "</table:table-cell></table:table-row></table:table>"),
        ("<draw:frame><draw:text-box>", "</draw:text-box></draw:frame>"),
        ("<text:note><text:note-body>", "</text:note-body></text:note>"),
        ("<office:annotation>", "</office:annotation>"),
    ],
};

const HTML_SEED: &str = r#"<!DOCTYPE html><html lang="en" dir="ltr"><head><meta charset="utf-8"><title>Seed &amp; title</title><meta name="author" content="Ann"><style>body{font-family:serif} @page{size:A4 landscape;margin:2cm 1in}</style></head><body>
<h1 style="text-align:center">Heading</h1>
<p style="margin-top:12pt;text-indent:1em;line-height:1.5">Plain <b>bold</b> <i>it</i> <u>u</u> <s>s</s> <sup>2</sup> <code>c</code> <a href="https://e.test/x?y=1&amp;z=2">link</a> <span style="color:#c00;font-size:18pt;background:yellow">styled</span> &nbsp; &#x1F600; &lt;tag&gt;</p>
<p dir="rtl">עברית מימין לשמאל</p>
<ol start="3"><li>three<ol><li>nested a</li><li>nested b</li></ol></li><li>four</li></ol>
<!-- -->
<ul><li>bullet<p>second para</p></li></ul>
<table border="1"><colgroup><col style="width:72pt"><col width="150"></colgroup><tr><th colspan="2">head</th></tr><tr><td rowspan="2" style="background-color:#eef">a</td><td>b<table><tr><td>in</td></tr></table></td></tr><tr><td style="text-align:right">c</td></tr></table>
<blockquote><p>quoted</p></blockquote>
<pre>  code
    more</pre>
<p><img src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==" alt="dot" width="20" height="10"><br>after break<a id="mark"></a></p>
<p><math><mrow><msup><mi>x</mi><mn>2</mn></msup><mo>+</mo><mfrac><mn>1</mn><mi>y</mi></mfrac></mrow></math></p>
<hr><p style="page-break-before:always">last</p>
</body></html>"#;

const HTML_SHAPE: Shape = Shape {
    tokens: &[
        "<p>",
        "</p>",
        "<div dir=\"rtl\">",
        "</div>",
        "<table>",
        "</table>",
        "<tr>",
        "<td colspan=\"999999999\" rowspan=\"4294967295\">",
        "<th>",
        "</td>",
        "<col width=\"99999999999\">",
        "<col span=\"999999\">",
        "<ol start=\"4294967295\">",
        "<ol start=\"-1\">",
        "<li>",
        "</li>",
        "</ol>",
        "<ul>",
        "<br>",
        "<img src=\"data:image/png;base64,\">",
        "<img src=\"data:,\">",
        "<a href=\"javascript:alert(1)\">",
        "<a id=\"x\"></a>",
        "<math><mrow>",
        "</mrow></math>",
        "<mfrac>",
        "<style>",
        "</style>",
        "<script>",
        "</script>",
        "<!--",
        "-->",
        "&#x110000;",
        "&#99999999999;",
        "&amp;",
        "&",
        "<",
        ">",
        "\"",
        "=",
        "<p style=\"margin-left:1e308pt;line-height:-5;text-indent:99999999999pt\">",
        "@page{size:1e308pt;margin:-1pt}",
        "<h9>",
        "<pre>",
        "<blockquote>",
        "<font size=\"99999\" color=\"#zzz\">",
    ],
    nests: &[
        ("<div>", "</div>"),
        ("<ul><li>", "</li></ul>"),
        ("<table><tr><td>", "</td></tr></table>"),
        ("<blockquote>", "</blockquote>"),
        ("<span>", "</span>"),
        ("<b>", "</b>"),
        ("<math><mrow>", "</mrow></math>"),
    ],
};

const MD_SEED: &str = "# Heading *one*\n\nPlain **bold** _it_ ~~s~~ `code` [link](https://e.test/x \"t\") ![dot](data:image/png;base64,AAAA) <a id=\"m\"></a> a\\*b  \nhard break\n\n3. three\n4. four\n    1. nested\n    2. nested two\n\n<!-- -->\n\n1. restart\n\n- bullet\n  continued\n\n  second para\n- [ ] task\n\n> quote\n> more\n\n```rust\nfn x() {}\n```\n\n| a | b |\n|---|:-:|\n| 1 | 2 |\n\n---\n\n    indented code\n\n\u{3000}全角 and 日本語\n";

const MD_SHAPE: Shape = Shape {
    tokens: &[
        "- ",
        "* ",
        "1. ",
        "999999999. ",
        "4294967296) ",
        "> ",
        "#",
        "######## ",
        "```",
        "~~~",
        "|",
        "|---|",
        "[",
        "]",
        "(",
        ")",
        "*",
        "_",
        "`",
        "<!-- -->",
        "<!--",
        "\\",
        "  \n",
        "\n\n",
        "\t",
        "![",
        "<a id=\"x\">",
        "<br>",
        "&#99999999;",
        "&amp;",
        "---",
        "***",
        "    ",
        "\u{3000}",
        "\u{00a0}",
        "<https://e.test>",
        "<x@y.z>",
        "[x]: ",
        "~",
    ],
    nests: &[("> ", ""), ("- ", ""), ("    ", ""), ("*", "*"), ("[", "]"), ("`", "`"), ("<b>", "</b>")],
};

#[test]
fn mutated_rtf_never_panics() {
    let mut rng = Rng(0x5eed_0ff1_de11_7100);
    let seed = RTF_SEED.as_bytes();
    exercise("rtf", seed, "seed");
    // Our own RTF of the seed is a second seed.
    let ours = wordcraft_formats::export("rtf", &wordcraft_formats::import("rtf", seed).unwrap().unwrap()).unwrap().unwrap();
    for i in 0..3000 * rounds() {
        let base = if i % 2 == 0 { seed } else { &ours[..] };
        let m = mutate(base, &RTF_SHAPE, &mut rng);
        exercise("rtf", &m, &format!("rtf mutant {i}"));
    }
}

#[test]
fn mutated_odt_never_panics() {
    let mut rng = Rng(0x0d75_eedc_afef_00d0);
    let (content, styles) = odt_seed();
    let pack = |c: &[u8], s: &[u8]| {
        zip(&[("mimetype", b"application/vnd.oasis.opendocument.text"), ("content.xml", c), ("styles.xml", s), ("Pictures/p.png", &png())])
    };
    exercise("odt", &pack(content.as_bytes(), styles.as_bytes()), "seed");
    for i in 0..1500 * rounds() {
        let (c, s) = if i % 3 == 0 {
            (content.as_bytes().to_vec(), mutate(styles.as_bytes(), &ODT_SHAPE, &mut rng))
        } else {
            (mutate(content.as_bytes(), &ODT_SHAPE, &mut rng), styles.as_bytes().to_vec())
        };
        exercise("odt", &pack(&c, &s), &format!("odt mutant {i}"));
    }
    // The package itself mutated.
    let whole = pack(content.as_bytes(), styles.as_bytes());
    let zip_shape =
        Shape { tokens: &["PK", "\0\0\0\0", "\u{ff}\u{ff}", "PK\u{3}\u{4}", "PK\u{5}\u{6}", "content.xml", "mimetype"], nests: &[("PK", "")] };
    for i in 0..300 * rounds() {
        let m = mutate(&whole, &zip_shape, &mut rng);
        exercise("odt", &m, &format!("odt package mutant {i}"));
    }
}

#[test]
fn mutated_flat_odt_never_panics() {
    let mut rng = Rng(0xf1a7_0d7f_0d75_eed1);
    let seed = fodt_seed();
    exercise("fodt", seed.as_bytes(), "seed");
    for i in 0..1500 * rounds() {
        let m = mutate(seed.as_bytes(), &ODT_SHAPE, &mut rng);
        exercise("fodt", &m, &format!("fodt mutant {i}"));
    }
}

#[test]
fn mutated_html_never_panics() {
    let mut rng = Rng(0x47ad_1e5e_edb0_0b1e);
    let seed = HTML_SEED.as_bytes();
    exercise("html", seed, "seed");
    let ours = wordcraft_formats::export("html", &wordcraft_formats::import("html", seed).unwrap().unwrap()).unwrap().unwrap();
    for i in 0..2500 * rounds() {
        let base = if i % 2 == 0 { seed } else { &ours[..] };
        let m = mutate(base, &HTML_SHAPE, &mut rng);
        exercise("html", &m, &format!("html mutant {i}"));
    }
}

#[test]
fn mutated_markdown_never_panics() {
    let mut rng = Rng(0x3a2d_0c5e_edc0_ffee);
    let seed = MD_SEED.as_bytes();
    exercise("md", seed, "seed");
    let ours = wordcraft_formats::export("md", &wordcraft_formats::import("md", seed).unwrap().unwrap()).unwrap().unwrap();
    for i in 0..2500 * rounds() {
        let base = if i % 2 == 0 { seed } else { &ours[..] };
        let m = mutate(base, &MD_SHAPE, &mut rng);
        exercise("md", &m, &format!("md mutant {i}"));
    }
}

/// Deflate bombs and lying packages: a package whose XML inflates far past the budget is
/// refused before it is inflated whole; one whose entry is bigger than it says, one with
/// thousands of entries, and one whose `mimetype` is huge all open or are refused in bounded
/// time and memory.
#[test]
fn odt_zip_bombs_are_refused_or_bounded() {
    let (content, styles) = odt_seed();
    // 400 MB of spaces inside the content (deflated to under a megabyte).
    let bomb = {
        let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let o = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        z.start_file("mimetype", o).unwrap();
        z.write_all(b"application/vnd.oasis.opendocument.text").unwrap();
        z.start_file("content.xml", o).unwrap();
        z.write_all(content.split("<office:body>").next().unwrap().as_bytes()).unwrap();
        let spaces = vec![b' '; 1 << 20];
        for _ in 0..400 {
            z.write_all(&spaces).unwrap();
        }
        z.write_all(b"<office:body><office:text><text:p>x</text:p></office:text></office:body></office:document-content>").unwrap();
        z.start_file("styles.xml", o).unwrap();
        z.write_all(styles.as_bytes()).unwrap();
        z.finish().unwrap().into_inner()
    };
    assert!(bomb.len() < (2 << 20), "the bomb is {} bytes", bomb.len());
    let t = Instant::now();
    let r = wordcraft_formats::import("odt", &bomb).unwrap();
    assert!(r.is_err(), "a 400 MB content.xml opened");
    assert!(t.elapsed() < Duration::from_secs(10), "{:?}", t.elapsed());
    // Thousands of entries, pictures pointing at the same big entry many times.
    let many = {
        let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let o = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        z.start_file("mimetype", o).unwrap();
        z.write_all(b"application/vnd.oasis.opendocument.text").unwrap();
        for i in 0..5000 {
            z.start_file(format!("Pictures/{i}.png"), o).unwrap();
            z.write_all(&png()).unwrap();
        }
        let frames: String = (0..5000)
            .map(|i| format!(r#"<draw:frame svg:width="1cm" svg:height="1cm"><draw:image xlink:href="Pictures/{}.png"/></draw:frame>"#, i % 7))
            .collect();
        z.start_file("content.xml", o).unwrap();
        z.write_all(format!("<office:document-content {NS}><office:body><office:text><text:p>{frames}</text:p></office:text></office:body></office:document-content>").as_bytes()).unwrap();
        z.finish().unwrap().into_inner()
    };
    let t = Instant::now();
    let r = wordcraft_formats::import("odt", &many).unwrap();
    assert!(r.is_ok() || r.as_ref().unwrap_err().contains("limit"), "{r:?}");
    assert!(t.elapsed() < Duration::from_secs(20), "{:?}", t.elapsed());
    // A huge mimetype entry (only its first bytes are read).
    let big_mime = zip(&[("mimetype", &vec![b'a'; 8 << 20]), ("content.xml", content.as_bytes())]);
    let r = wordcraft_formats::import("odt", &big_mime).unwrap();
    assert!(r.is_err(), "a document with a bogus 8 MB mimetype opened");
}

/// The seeds read back with what they hold.
#[test]
fn seeds_read_their_stories() {
    let doc = wordcraft_formats::import("rtf", RTF_SEED.as_bytes()).unwrap().unwrap();
    let kinds: Vec<_> = doc.parts.values().map(|p| p.kind).collect();
    use wordcraft_doc::PartKind::*;
    for k in [Header, Footnote, Endnote, Comment, TextBox] {
        assert!(kinds.contains(&k), "rtf: no {k:?} in {kinds:?}");
    }
    assert_eq!(doc.sections().len(), 2);
    let text = doc.plain_text(wordcraft_doc::StoryRef::Body);
    assert!(text.contains("日本"), "the Shift_JIS text of the seed: {text}");
    assert!(doc.body.iter().filter_map(|b| b.as_para()).any(|p| p.props.bidi == Some(true)), "the \\rtlpar paragraph");
    let (content, styles) = odt_seed();
    let odt = zip(&[
        ("mimetype", b"application/vnd.oasis.opendocument.text"),
        ("content.xml", content.as_bytes()),
        ("styles.xml", styles.as_bytes()),
        ("Pictures/p.png", &png()),
    ]);
    for (what, bytes, ext) in [("odt", odt, "odt"), ("fodt", fodt_seed().into_bytes(), "fodt")] {
        let doc = wordcraft_formats::import(ext, &bytes).unwrap().unwrap();
        let kinds: Vec<_> = doc.parts.values().map(|p| p.kind).collect();
        for k in [Header, Footer, Footnote, Endnote, Comment, TextBox] {
            assert!(kinds.contains(&k), "{what}: no {k:?} in {kinds:?}");
        }
        assert_eq!(doc.sections().len(), 2, "{what}");
        assert!(doc.settings.even_odd_headers && doc.sections().first().is_some_and(|(_, s)| s.title_page), "{what}");
        assert_eq!(doc.media.len(), 1, "{what}: the picture");
        assert!(doc.body.iter().filter_map(|b| b.as_para()).any(|p| p.props.bidi == Some(true)), "{what}: the right-to-left paragraph");
    }
    let doc = wordcraft_formats::import("html", HTML_SEED.as_bytes()).unwrap().unwrap();
    assert!(doc.body.iter().filter_map(|b| b.as_para()).any(|p| p.props.bidi == Some(true)), "html: the dir=rtl paragraph");
    assert_eq!(doc.media.len(), 1, "html: the picture");
    let doc = wordcraft_formats::import("md", MD_SEED.as_bytes()).unwrap().unwrap();
    assert!(doc.plain_text(wordcraft_doc::StoryRef::Body).contains("全角 and 日本語"), "md: the East Asian text");
}

/// A Markdown paragraph that starts with an ideographic space (U+3000, a Japanese first-line
/// indent) keeps it (#2538). Fixed upstream in 75122c11, not here: the upstream merge should
/// make this pass.
#[test]
fn markdown_keeps_a_leading_ideographic_space() {
    let doc = wordcraft_formats::import("md", "\u{3000}全角で始まる段落。\n".as_bytes()).unwrap().unwrap();
    assert!(doc.plain_text(wordcraft_doc::StoryRef::Body).starts_with('\u{3000}'), "{:?}", doc.plain_text(wordcraft_doc::StoryRef::Body));
}

/// Text in a rectangle or custom shape (as another program writes a text box) opens as a text
/// box; a shape without text is still left out.
#[test]
fn odt_shape_text_opens_as_a_text_box() {
    let content = format!(
        r#"<office:document-content {NS}><office:body><office:text><text:p>Before <draw:custom-shape text:anchor-type="as-char" svg:width="2in" svg:height="1in"><text:p>Shape text</text:p><draw:enhanced-geometry draw:type="rect"/></draw:custom-shape><draw:rect svg:width="1in" svg:height="1in"/> after</text:p></office:text></office:body></office:document-content>"#
    );
    let odt = zip(&[("mimetype", b"application/vnd.oasis.opendocument.text"), ("content.xml", content.as_bytes())]);
    let doc = wordcraft_formats::import("odt", &odt).unwrap().unwrap();
    let boxes: Vec<_> = doc.parts.values().filter(|p| p.kind == wordcraft_doc::PartKind::TextBox).collect();
    assert_eq!(boxes.len(), 1);
    let p = doc.body.first().and_then(|b| b.as_para()).unwrap();
    assert_eq!(p.plain_text().replace(wordcraft_doc::para::OBJ, ""), "Before  after");
}
