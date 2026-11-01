//! The shared document limits (`wordcraft_doc::limits`) end to end: what the editor lets a
//! person make, every lossless format saves and opens again, whole.

use std::sync::Arc;

use serde_json::json;
use wordcraft_doc::limits::{Limits, Measure, Tally};

use crate::Session;
use wordcraft_doc::para::NoteKind;
use wordcraft_doc::props::{CharProps, TextColor, Underline};
use wordcraft_doc::{Block, Document, InlineObject, Paragraph, PartKind, Revision, RevisionKind};

/// A synthetic document of the given shape, built straight in the model (fast enough for
/// documents at the limits). `spec` is `key=value` pairs separated by commas:
/// `paras` (paragraphs in all, every story counted), `runs` and `chars` (a body paragraph's runs
/// and the characters in each), `heavy` (a font, colour and underline on every run), `light`
/// (every other run bold, nothing else), `notes`,
/// `comments`, `headers` (stories of one paragraph each), `styles` (custom paragraph styles),
/// `lists` (lists of their own), `sections`, `revisions` (tracked insertions, each of its own
/// author), `sources` (bibliography sources), `pictures` and `picbytes` (distinct pictures of that
/// many bytes).
pub(crate) fn generated(spec: &str) -> Document {
    let get = |k: &str| -> usize {
        spec.split(',').filter_map(|kv| kv.split_once('=')).find(|(key, _)| key.trim() == k).and_then(|(_, v)| v.trim().parse().ok()).unwrap_or(0)
    };
    let (paras, runs, chars) = (get("paras").max(1), get("runs").max(1), get("chars").max(1));
    let heavy = get("heavy") != 0;
    let light = get("light") != 0;
    // Every paragraph in one of the lists, in turn (not only the first `lists` paragraphs).
    let lists_everywhere = get("lists_everywhere") != 0;
    let (notes, comments, headers) = (get("notes"), get("comments"), get("headers"));
    let (styles, lists, sections, revisions) = (get("styles"), get("lists"), get("sections"), get("revisions"));
    let (pictures, picbytes) = (get("pictures"), get("picbytes").max(64));
    let mut doc = Document::new();
    doc.body.clear();
    for i in 0..get("sources") {
        doc.sources.push(wordcraft_doc::Source {
            tag: format!("Src{i}"),
            kind: "book".into(),
            author: "Rivera, Alex".into(),
            title: format!("Synthetic {i}"),
            year: "2026".into(),
            ..Default::default()
        });
    }
    {
        let sheet = wordcraft_doc::Shared::make_mut(&mut doc.styles);
        for i in 0..styles {
            sheet.styles.push(wordcraft_doc::styles::Style {
                id: format!("Gen{i}"),
                name: format!("Generated {i}"),
                based_on: Some("Normal".into()),
                para: wordcraft_doc::props::ParaProps { space_after: Some((i % 24) as f32), ..Default::default() },
                ..Default::default()
            });
        }
    }
    let mut list_ids = Vec::with_capacity(lists);
    {
        let numbering = wordcraft_doc::Shared::make_mut(&mut doc.numbering);
        for _ in 0..lists {
            list_ids.push(numbering.add_list(wordcraft_doc::numbering::ListKind::Numbered));
        }
    }
    {
        let revs = wordcraft_doc::Shared::make_mut(&mut doc.revisions);
        for i in 0..revisions {
            revs.push(Revision::new(RevisionKind::Insert, format!("Reviewer {i}"), "2026-10-17T12:00:00Z"));
        }
    }
    {
        let media = wordcraft_doc::Shared::make_mut(&mut doc.media);
        for i in 0..pictures {
            media.insert(format!("gen{i}.png"), Arc::new(picture(i, picbytes)));
        }
    }
    let word: String = "abcdefghijklmnopqrstuvwxyz".chars().cycle().take(chars).collect();
    let story_paras = notes + comments + headers;
    let body = paras.saturating_sub(story_paras).max(1);
    let run_props = |i: usize, j: usize| {
        let mut props = CharProps {
            bold: Some(j.is_multiple_of(2)),
            italic: Some(j.is_multiple_of(3)),
            size: Some(if j.is_multiple_of(2) { 12.0 } else { 11.0 }),
            ..Default::default()
        };
        if heavy {
            props.font = Some(if j.is_multiple_of(2) { "Source Serif 4" } else { "Source Sans 3" }.into());
            props.color = Some(TextColor::Rgb(wordcraft_doc::props::Rgb(10, 20, ((i + j) * 20) as u8)));
            props.underline = Some(Underline::Single);
        }
        if runs == 1 {
            props = CharProps::default();
        }
        // Light: every other run bold, the least that keeps runs apart.
        if light {
            props = CharProps { bold: j.is_multiple_of(2).then_some(true), ..Default::default() };
        }
        props
    };
    for i in 0..body {
        let mut p = Paragraph::new();
        p.text = word.repeat(runs);
        p.runs = (0..runs).map(|j| wordcraft_doc::Run { len: word.len(), props: run_props(i, j) }).collect();
        if let Some(r) = (i < revisions).then_some(i as u32)
            && let Some(first) = p.runs.first_mut()
        {
            first.props.ins = Some(r);
        }
        if styles > 0 {
            p.props.style = Some(format!("Gen{}", i % styles));
        }
        if let Some(num) = list_ids.get(i % lists.max(1)).filter(|_| i < lists || lists_everywhere) {
            p.props.numbering = Some(wordcraft_doc::props::NumRef { num: *num, level: 0 });
        }
        if i > 0 && i <= sections.saturating_sub(1) {
            p.section = Some(Box::new(wordcraft_doc::SectionProps::default()));
        }
        doc.body.push(Arc::new(Block::Para(p)));
    }
    // Pictures, notes and comments go into the body's paragraphs, one each, from the start.
    let mut at = 0usize;
    let mut next_para = |doc: &mut Document, obj: InlineObject| {
        let i = at % doc.body.len();
        at += 1;
        if let Some(Block::Para(p)) = doc.body.get_mut(i).map(Arc::make_mut) {
            let end = p.len();
            let props = CharProps::default();
            let _ = p.insert_object(end, obj, &props);
        }
    };
    for i in 0..pictures {
        next_para(
            &mut doc,
            InlineObject::Image {
                media: format!("gen{i}.png"),
                w: 12.0,
                h: 12.0,
                alt: String::new(),
                float: Default::default(),
                crop: [0.0; 4],
                ole: None,
            },
        );
    }
    for i in 0..notes {
        let id = doc.add_part(PartKind::Footnote, vec![wordcraft_doc::para_block(Paragraph::with_text(&format!("Note {i}."), CharProps::default()))]);
        next_para(&mut doc, InlineObject::NoteRef { kind: NoteKind::Footnote, id, custom: String::new() });
    }
    for i in 0..comments {
        let part =
            doc.add_part(PartKind::Comment, vec![wordcraft_doc::para_block(Paragraph::with_text(&format!("Comment {i}."), CharProps::default()))]);
        let cid = i as u32;
        wordcraft_doc::Shared::make_mut(&mut doc.comments).insert(
            cid,
            wordcraft_doc::Comment {
                author: "Reviewer".into(),
                initials: "R".into(),
                date: "2026-10-17T12:00:00Z".into(),
                parent: None,
                resolved: false,
                part,
                durable_id: None,
                date_utc: None,
            },
        );
        next_para(&mut doc, InlineObject::CommentStart { id: cid });
        next_para(&mut doc, InlineObject::CommentEnd { id: cid });
    }
    // Headers: one a section (the first section's three, then one each for the others).
    for i in 0..headers {
        let id = doc.add_part(PartKind::Header, vec![wordcraft_doc::para_block(Paragraph::with_text(&format!("Header {i}."), CharProps::default()))]);
        let mut n = 0usize;
        let mut set = None;
        for b in doc.body.iter_mut() {
            if let Block::Para(p) = &**b
                && p.section.is_some()
            {
                if n == i {
                    set = Some(b);
                    break;
                }
                n += 1;
            }
        }
        match set {
            Some(b) => {
                if let Block::Para(p) = Arc::make_mut(b)
                    && let Some(s) = p.section.as_mut()
                {
                    s.headers.default = Some(id);
                }
            }
            None => doc.last_section.headers.default = Some(id),
        }
    }
    doc
}

/// A distinct, valid 1×1 PNG padded with `bytes` in all: the pad follows the image's end, where
/// decoders stop reading.
fn picture(i: usize, bytes: usize) -> Vec<u8> {
    const PNG: [u8; 67] = [
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
        0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00, 0x01,
        0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];
    let mut v = PNG.to_vec();
    let mut x = (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    while v.len() < bytes {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        v.extend_from_slice(&x.to_le_bytes());
    }
    v.truncate(bytes.max(PNG.len()));
    v
}

/// A document at every limit at once, as [`generated`] builds it: 250,000 paragraphs, 735,000
/// runs (every other one bold, the least formatting that keeps them apart), 25,000 stories
/// (10,000 footnotes, 10,000 comments, 5,000 headers), 10,000 styles (with the built-in ones),
/// lists and sources, 5,000 sections, 100,000 revisions, 5,000 pictures of 52 KB (256 MB), and
/// the saved size within a tenth of its limit.
pub(crate) const AT_LIMITS: &str = "paras=250000,runs=3,chars=1,light=1,notes=10000,comments=10000,headers=5000,styles=9946,lists=10000,sections=5000,revisions=100000,sources=10000,pictures=5000,picbytes=53687";

/// Writes [`generated`] documents for measuring: `WC_GEN` is the spec, `WC_GEN_OUT` the files
/// (comma-separated; each extension picks its format).
#[test]
#[ignore = "a generator for measurements, run by hand"]
fn generate_documents() {
    let (Ok(spec), Ok(out)) = (std::env::var("WC_GEN"), std::env::var("WC_GEN_OUT")) else { return };
    let t = std::time::Instant::now();
    let doc = generated(&spec);
    eprintln!("built in {:.1} s: {} paragraphs, {} parts", t.elapsed().as_secs_f64(), crate::session::paragraph_total(&doc), doc.parts.len());
    eprintln!("{:?}; over: {:?}", Tally::of(&doc), Tally::of(&doc).over(&Limits::DOCUMENT));
    for path in out.split(',').filter(|p| !p.is_empty()) {
        let t = std::time::Instant::now();
        let bytes = crate::io::save_bytes(path, &doc).unwrap();
        std::fs::write(path, &bytes).unwrap();
        eprintln!("{path}: {} bytes in {:.1} s", bytes.len(), t.elapsed().as_secs_f64());
    }
}

/// What the Word writer emits for `doc`: (every part's bytes together, the largest part that
/// isn't a picture or kept part), as the reader inflates them.
pub(crate) fn written_sizes(doc: &Document) -> (u64, u64) {
    let bytes = wordcraft_docx::write(doc).unwrap();
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let kept: std::collections::HashSet<usize> = doc.media.values().chain(doc.passthrough.values()).map(|b| b.len()).collect();
    let (mut total, mut largest) = (0u64, 0u64);
    for i in 0..z.len() {
        let f = z.by_index(i).unwrap();
        let (name, size) = (f.name().to_string(), f.size());
        total += size;
        let media = name.starts_with("word/media/") || kept.contains(&(size as usize)) && !name.ends_with(".rels");
        if !media {
            largest = largest.max(size);
        }
    }
    (total, largest)
}

/// The text and structure of `doc` as a recovery copy holds them, beyond what the copy allows
/// each side file's entry (`PART_LISTING`): compact JSON without the pictures and kept parts
/// (they are side files), and their names in the listing.
pub(crate) fn recovery_json_bytes(doc: &Document) -> u64 {
    let mut d = doc.clone();
    d.media = Default::default();
    d.passthrough = Default::default();
    let names: u64 = doc.media.keys().chain(doc.passthrough.keys()).map(|k| k.len() as u64).sum();
    serde_json::to_vec(&d).map_or(u64::MAX, |v| v.len() as u64 + names)
}

/// The saved-size estimate holds what the writers of every lossless format emit, for every
/// sample document and for generated ones of every shape: the Word writer's parts together within
/// the estimate plus the pictures, and each part within the estimate, and what a document's
/// content adds to an empty document's parts within what it adds to the estimate (so the fixed
/// parts' allowance doesn't hide a piece the estimate undercounts); Word XML (Flat OPC, the
/// pictures in base64) within the estimate plus a third more pictures; a recovery copy's text and
/// structure (compact JSON) within the estimate; WordCraft's JSON (indented, the pictures in
/// base64) within twice the estimate and a third more pictures. So the readers' byte limits,
/// which follow from the estimate's limit, open every file a document within the limits saves.
#[test]
fn the_saved_size_estimate_bounds_what_the_writer_emits() {
    let mut docs: Vec<(String, Document)> = vec![
        ("sample".into(), crate::sample::sample_document()),
        ("letter".into(), crate::sample::letter()),
        ("resume".into(), crate::sample::resume()),
        ("report".into(), crate::sample::report()),
    ];
    for spec in [
        "paras=2000,runs=1,chars=5",
        "paras=500,runs=10,chars=5",
        "paras=300,runs=10,chars=5,heavy=1",
        "paras=600,runs=1,chars=20,notes=200,comments=200,headers=50,sections=50",
        "paras=300,runs=2,chars=10,styles=100,lists=100,revisions=300",
        "paras=100,runs=1,chars=20,pictures=50,picbytes=2000",
        // One kind of content at a time, many of it, to see what each costs.
        "paras=20000,runs=1,chars=1",
        "paras=20000,runs=3,chars=4",
        "paras=20000,runs=3,chars=1,light=1",
        "paras=2100,notes=2000",
        "paras=2100,comments=2000",
        "paras=2000,sections=2000",
        "paras=4000,sections=2000,headers=2000",
        "paras=2000,pictures=2000,picbytes=64",
        "paras=2000,lists=2000",
        "paras=100,styles=2000",
        "paras=2000,runs=2,chars=4,revisions=2000",
        "paras=100,sources=2000",
    ] {
        docs.push((spec.into(), generated(spec)));
    }
    let empty = Document::new();
    let (empty_written, empty_estimate) = (written_sizes(&empty).0, Tally::of(&empty).saved_bytes);
    for (name, doc) in &docs {
        let t = wordcraft_doc::limits::Tally::of(doc);
        let (total, largest) = written_sizes(doc);
        let added = total.saturating_sub(t.media_bytes).saturating_sub(empty_written);
        let estimated = t.saved_bytes.saturating_sub(empty_estimate);
        eprintln!(
            "{name:70} estimate {:>9}  written {:>9} ({:.2}x)  added {:.2}x  largest part {:>9}",
            t.saved_bytes,
            total.saturating_sub(t.media_bytes),
            t.saved_bytes as f64 / total.saturating_sub(t.media_bytes).max(1) as f64,
            estimated as f64 / added.max(1) as f64,
            largest
        );
        assert!(added <= estimated, "{name}: the content adds {added} bytes written, {estimated} estimated");
        assert!(total <= t.saved_bytes + t.media_bytes, "{name}: {total} written, {} + {} estimated", t.saved_bytes, t.media_bytes);
        assert!(largest <= t.saved_bytes, "{name}: a part of {largest} bytes, {} estimated", t.saved_bytes);
        let base64 = t.media_bytes.div_ceil(3) * 4;
        let flat = crate::io::save_bytes("limits.xml", doc).unwrap().len() as u64;
        let recovery = recovery_json_bytes(doc);
        let json = crate::io::save_bytes("limits.json", doc).unwrap().len() as u64;
        eprintln!(
            "{:70} flat {:.2}x  recovery {:.2}x  json {:.2}x",
            "",
            t.saved_bytes as f64 / flat.saturating_sub(base64).max(1) as f64,
            t.saved_bytes as f64 / recovery.max(1) as f64,
            2.0 * t.saved_bytes as f64 / json.saturating_sub(base64).max(1) as f64
        );
        assert!(flat <= t.saved_bytes + base64, "{name}: Word XML of {flat} bytes, {} + {base64} estimated", t.saved_bytes);
        assert!(recovery <= t.saved_bytes, "{name}: a recovery copy's JSON of {recovery} bytes, {} estimated", t.saved_bytes);
        assert!(json <= 2 * t.saved_bytes + base64, "{name}: JSON of {json} bytes, 2 x {} + {base64} estimated", t.saved_bytes);
    }
}

/// Peak resident memory so far (`VmHWM`), MB; 0 where `/proc` has none.
pub(crate) fn peak_mb() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("VmHWM:")).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse::<f64>().ok()))
        .map_or(0.0, |kb| kb / 1024.0)
}

/// Resident memory now (`VmRSS`), MB.
pub(crate) fn rss_mb() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("VmRSS:")).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse::<f64>().ok()))
        .map_or(0.0, |kb| kb / 1024.0)
}

/// Opens `WC_READ` in stages, printing the peak memory after each (for measurements).
#[test]
#[ignore = "a probe for measurements, run by hand"]
fn probe_open_stages() {
    let Ok(path) = std::env::var("WC_READ") else { return };
    let bytes = std::fs::read(&path).unwrap();
    eprintln!("read file: {:.0} MB", peak_mb());
    let t = std::time::Instant::now();
    // `WC_RAW`: the Word reader alone, without the limits checked on opening (to measure a
    // document past them).
    let doc = if std::env::var("WC_RAW").is_ok() { wordcraft_docx::read(&bytes).unwrap() } else { crate::io::open_bytes(&path, &bytes).unwrap() };
    eprintln!("open_bytes: {:.0} MB, {:.1} s", peak_mb(), t.elapsed().as_secs_f64());
    drop(bytes);
    eprintln!("held after open: {:.0} MB", rss_mb());
    let t = std::time::Instant::now();
    let mut s = crate::Session::new(doc);
    eprintln!("session: {:.0} MB, {:.1} s", peak_mb(), t.elapsed().as_secs_f64());
    let t = std::time::Instant::now();
    let _ = s.doc.word_count();
    eprintln!("word count: {:.0} MB, {:.1} s", peak_mb(), t.elapsed().as_secs_f64());
    if std::env::var("WC_LAYOUT").is_ok() {
        let t = std::time::Instant::now();
        let pages = s.layout().pages.len();
        eprintln!("layout ({pages} pages): {:.0} MB, {:.1} s", peak_mb(), t.elapsed().as_secs_f64());
    }
}

// ---------------------------------------------------------------------------------------------
// The editor at each limit (scaled down, one limit at a time).

/// What a document is, to compare one read back with the one saved: every count but the saved
/// size (the reader may write it a few bytes differently) and runs (it may merge runs of the
/// same formatting), and the text of every story in order.
fn fingerprint(doc: &Document) -> (Vec<u64>, Vec<String>) {
    let t = Tally::of(doc);
    let counts = Measure::ALL.iter().filter(|m| !matches!(m, Measure::SavedBytes | Measure::Runs)).map(|m| t.get(*m)).collect();
    let mut texts = vec![doc.plain_text(wordcraft_doc::StoryRef::Body)];
    texts.extend(doc.parts.values().map(|p| p.blocks.iter().filter_map(|b| b.as_para()).map(Paragraph::plain_text).collect::<Vec<_>>().join("\n")));
    (counts, texts)
}

/// `doc` saved as `name` and opened again equals it ([`fingerprint`]).
pub(crate) fn round_trips(name: &str, doc: &Document) {
    let bytes = crate::io::save_bytes(name, doc).unwrap_or_else(|e| panic!("{name}: {e}"));
    let back = crate::io::open_bytes(name, &bytes).unwrap_or_else(|e| panic!("{name}: a document within the limits doesn't open again: {e}"));
    assert_eq!(fingerprint(&back), fingerprint(doc), "{name}: what was saved isn't what opened");
}

/// The formats that keep everything ([`crate::io::fidelity::keeps_everything`]).
const LOSSLESS: [&str; 3] = ["limits.docx", "limits.xml", "limits.json"];

/// A distinct small PNG of about `bytes` bytes (a valid image with padding after its end).
fn png(i: usize, bytes: usize) -> String {
    crate::cmd::insert::base64_encode(&picture(i, bytes))
}

/// One limit: how to make the document grow by one of what it counts (the `i`th time), and
/// what to do first: once (`setup`) and before each step (`ready`: a command that sets up the
/// next edit without growing the document).
struct Edge {
    measure: Measure,
    setup: fn(&mut Session),
    ready: fn(&mut Session, usize),
    grow: fn(&mut Session, usize) -> crate::CmdResult,
    /// How many the limit allows past what the setup leaves.
    room: u64,
}

fn edges() -> Vec<Edge> {
    vec![
        Edge { measure: Measure::Paragraphs, setup: |_| {}, ready: |_, _| {}, grow: |s, _| s.run("text.newParagraph", &json!({})), room: 5 },
        Edge {
            measure: Measure::Runs,
            setup: |s| {
                s.run("text.insert", &json!({"text": "Synthetic runs "})).unwrap();
            },
            // A letter in the other weight: a run of its own.
            ready: |s, _| {
                s.run("format.bold", &json!({})).unwrap();
            },
            grow: |s, _| s.run("text.insert", &json!({"text": "x"})),
            room: 6,
        },
        Edge {
            measure: Measure::Stories,
            setup: |_| {},
            ready: |_, _| {},
            grow: |s, i| s.run("references.footnote", &json!({"text": format!("Synthetic note {i}")})),
            room: 4,
        },
        Edge {
            measure: Measure::Styles,
            setup: |_| {},
            ready: |_, _| {},
            grow: |s, i| s.run("styles.create", &json!({"name": format!("Synthetic {i}")})),
            room: 4,
        },
        Edge {
            measure: Measure::Lists,
            setup: |s| {
                s.run("text.insert", &json!({"text": "Item"})).unwrap();
                s.run("para.numbering", &json!({})).unwrap();
            },
            ready: |_, _| {},
            grow: |s, _| s.run("para.restartNumbering", &json!({})),
            room: 3,
        },
        Edge {
            measure: Measure::Sections,
            setup: |_| {},
            ready: |_, _| {},
            grow: |s, _| s.run("layout.break", &json!({"kind": "nextPage"})),
            room: 4,
        },
        Edge {
            measure: Measure::Revisions,
            setup: |s| {
                s.run("review.trackChanges", &json!({"on": true})).unwrap();
            },
            // Each author's typing is a revision of its own.
            ready: |s, i| {
                s.run("file.setAuthor", &json!({"name": format!("Reviewer {i}")})).unwrap();
            },
            grow: |s, _| s.run("text.insert", &json!({"text": "x"})),
            room: 4,
        },
        Edge {
            measure: Measure::Sources,
            setup: |_| {},
            ready: |_, _| {},
            grow: |s, i| {
                s.run("references.sources", &json!({"add": {"tag": format!("T{i}"), "title": format!("Synthetic {i}"), "author": "Rivera, Alex"}}))
            },
            room: 3,
        },
        Edge {
            measure: Measure::Pictures,
            setup: |_| {},
            ready: |_, _| {},
            grow: |s, i| s.run("insert.picture", &json!({"data": png(i, 200)})),
            room: 3,
        },
        Edge {
            measure: Measure::MediaBytes,
            setup: |_| {},
            ready: |_, _| {},
            grow: |s, i| s.run("insert.picture", &json!({"data": png(i, 4000)})),
            room: 3 * 4000 + 100,
        },
        Edge {
            measure: Measure::SavedBytes,
            setup: |_| {},
            ready: |_, _| {},
            grow: |s, _| s.run("text.insert", &json!({"text": "synthetic ".repeat(100)})),
            room: 3 * 1000 + 10,
        },
    ]
}

/// Every limit, scaled down, through the editor's own commands: edits take the document to the
/// limit; at it, the document saves in every format that keeps everything (DOCX, Word XML,
/// WordCraft's JSON) and opens again the same; one more is refused, naming the limit, and leaves
/// the document and the undo history exactly as they were.
#[test]
fn every_limit_holds_edits_up_to_it_and_refuses_one_past() {
    for e in edges() {
        let what = e.measure.what();
        let mut s = Session::new(Document::new());
        (e.setup)(&mut s);
        let base = s.tally();
        let mut limits = Limits::DOCUMENT;
        limits.set(e.measure, base.get(e.measure) + e.room);
        s.limits = limits;
        let mut i = 0;
        let err = loop {
            (e.ready)(&mut s, i);
            let (doc, undo, redo) = (s.doc.clone(), s.undo_labels(), s.redo_label().map(str::to_string));
            match (e.grow)(&mut s, i) {
                Ok(_) => {
                    assert!(s.tally().get(e.measure) <= limits.get(e.measure), "{what}: past the limit");
                    i += 1;
                    assert!(i < 1000, "{what}: never refused");
                }
                Err(err) => {
                    assert_eq!(s.doc, doc, "{what}: the refused edit left something behind");
                    assert_eq!(
                        (s.undo_labels(), s.redo_label().map(str::to_string)),
                        (undo, redo),
                        "{what}: the refused edit changed the undo history"
                    );
                    break err;
                }
            }
        };
        assert!(i > 0, "{what}: refused before reaching the limit: {err}");
        assert!(matches!(err, crate::CmdError::Disabled(_)) && err.to_string().contains(what), "{what}: {err}");
        // At the limit (one more step would have passed it).
        let at = s.tally().get(e.measure);
        assert!(at > base.get(e.measure) && at <= limits.get(e.measure), "{what}: {at} of {}", limits.get(e.measure));
        assert_eq!(s.tally, Some(Tally::of(&s.doc)), "{what}: the count kept from what changed isn't the document's");
        for name in LOSSLESS {
            round_trips(name, &s.doc);
        }
        // Undo still goes back past the limit's edits.
        s.run("edit.undo", &json!({})).unwrap();
        assert!(s.tally().get(e.measure) <= at, "{what}");
    }
}

// ---------------------------------------------------------------------------------------------
// The real limits (ignored: a minute or more each and gigabytes).

/// At every limit at once, a document saves in every format that keeps everything and opens
/// again the same; every count is within a tenth of its limit, none past it.
#[test]
#[ignore = "a document at every limit at once: minutes and some GB"]
fn a_document_at_every_limit_saves_and_opens_again_in_every_lossless_format() {
    let t0 = std::time::Instant::now();
    let doc = generated(AT_LIMITS);
    let t = Tally::of(&doc);
    eprintln!("built in {:.1} s, peak {:.0} MB: {t:?}", t0.elapsed().as_secs_f64(), peak_mb());
    assert_eq!(t.over(&Limits::DOCUMENT), None);
    for m in Measure::ALL {
        let (n, limit) = (t.get(m), Limits::DOCUMENT.get(m));
        assert!(n.saturating_mul(10) >= limit.saturating_mul(9), "{}: {n} of {limit}", m.what());
    }
    for name in LOSSLESS {
        let t0 = std::time::Instant::now();
        round_trips(name, &doc);
        eprintln!("{name}: saved and opened again in {:.1} s, peak {:.0} MB", t0.elapsed().as_secs_f64(), peak_mb());
    }
}

/// A file one past each limit, as WordCraft writes one, is refused on opening in every format
/// that keeps everything, naming the limit: never opened without what is over.
#[test]
#[ignore = "documents one past each limit: minutes"]
fn a_file_one_past_each_limit_is_refused() {
    use wordcraft_doc::limits as l;
    let built_in = Document::new().styles.styles.len();
    let cases = [
        (Measure::Paragraphs, format!("paras={}", l::PARAGRAPHS + 1)),
        (Measure::Runs, format!("paras={},runs=5,chars=1", l::RUNS / 5 + 1)),
        (Measure::Stories, format!("paras={},notes={}", l::STORIES + 101, l::STORIES + 1)),
        (Measure::Styles, format!("paras=100,styles={}", l::STYLES + 1 - built_in)),
        (Measure::Lists, format!("paras={},lists={}", l::LISTS + 1, l::LISTS + 1)),
        (Measure::Sections, format!("paras={},sections={}", l::SECTIONS + 1, l::SECTIONS + 1)),
        (Measure::Revisions, format!("paras={},runs=2,chars=4,revisions={}", l::REVISIONS + 1, l::REVISIONS + 1)),
        (Measure::Sources, format!("paras=100,sources={}", l::SOURCES + 1)),
        (Measure::Pictures, format!("paras={},pictures={}", l::PICTURES + 1, l::PICTURES + 1)),
        (Measure::MediaBytes, format!("paras=10,pictures=1,picbytes={}", l::MEDIA_BYTES + 1)),
        (Measure::SavedBytes, "paras=50000,runs=10,chars=300,heavy=1".to_string()),
    ];
    for (m, spec) in cases {
        let doc = generated(&spec);
        let t = Tally::of(&doc);
        assert!(t.get(m) > Limits::DOCUMENT.get(m), "{spec}: {} of {}", t.get(m), Limits::DOCUMENT.get(m));
        for name in LOSSLESS {
            let t0 = std::time::Instant::now();
            let bytes = crate::io::save_bytes(name, &doc).unwrap_or_else(|e| panic!("{name}: {e}"));
            let err = crate::io::open_bytes(name, &bytes).err().unwrap_or_else(|| panic!("{name}, {}: opened", m.what()));
            eprintln!("{} {name} ({:.1} s): {err}", m.what(), t0.elapsed().as_secs_f64());
            assert!(err.contains(m.what()) && (err.contains("limit exceeded") || err.contains("more than")), "{name}, {}: {err}", m.what());
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Opening: what no reader refuses by itself, the backstop does.

/// A document read from any format that is past one of the limits (counted as the editor counts
/// them) is refused on opening, naming the limit; at the limit it opens. Scaled down: the limits
/// set to what the document has, then one less.
#[test]
fn opening_refuses_a_document_past_any_limit_whatever_its_format() {
    let doc = generated("paras=30,runs=2,chars=6,notes=2,comments=2,styles=3,lists=2,sections=3,revisions=3,sources=2,pictures=2,picbytes=100");
    let t = Tally::of(&doc);
    for m in Measure::ALL {
        let mut at = Limits::DOCUMENT;
        at.set(m, t.get(m));
        assert_eq!(crate::io::within_limits_of("synthetic.odt", &doc, &at), Ok(()), "{}", m.what());
        assert!(t.get(m) > 0, "{}: the document has none", m.what());
        let mut under = at;
        under.set(m, t.get(m) - 1);
        let err = crate::io::within_limits_of("synthetic.odt", &doc, &under).unwrap_err();
        assert!(err.starts_with("synthetic.odt: limit exceeded: ") && err.contains(m.what()), "{err}");
    }
}

/// Tables nested deeper than the Word reader takes (`TABLE_NESTING_READ`), or with more rows or
/// cells than it takes, are refused on opening whatever the format (an RTF or HTML file could
/// hold one, and its DOCX wouldn't open again); at the limit they open.
#[test]
fn opening_refuses_tables_the_word_reader_would_refuse() {
    use wordcraft_doc::limits::TABLE_NESTING_READ;
    use wordcraft_doc::table::{MAX_COLS, MAX_ROWS, Table};
    fn nested(depth: usize) -> Document {
        let mut t = Table::new(1, 1, 100.0);
        for _ in 1..depth {
            let mut outer = Table::new(1, 1, 100.0);
            outer.rows[0].cells[0].blocks = vec![Arc::new(Block::Table(t))];
            t = outer;
        }
        let mut doc = Document::new();
        doc.body = vec![Arc::new(Block::Table(t)), wordcraft_doc::para_block(Paragraph::new())];
        doc
    }
    assert_eq!(crate::io::within_limits("deep.rtf", &nested(TABLE_NESTING_READ)), Ok(()));
    let err = crate::io::within_limits("deep.rtf", &nested(TABLE_NESTING_READ + 1)).unwrap_err();
    assert!(err.contains("nested more than 24 deep"), "{err}");
    // (`Table::new` makes no more than the limits: one more row or cell is added by hand.)
    let table = |rows: usize, cols: usize, more_rows: bool, more_cells: bool| {
        let mut t = Table::new(rows, cols, 100.0);
        if more_rows {
            let r = t.rows[0].clone();
            t.rows.push(r);
        }
        if more_cells {
            let c = t.rows[0].cells[0].clone();
            t.rows[0].cells.push(c);
        }
        let mut doc = Document::new();
        doc.body = vec![Arc::new(Block::Table(t)), wordcraft_doc::para_block(Paragraph::new())];
        doc
    };
    assert_eq!(crate::io::within_limits("wide.html", &table(2, MAX_COLS, false, false)), Ok(()));
    assert!(crate::io::within_limits("wide.html", &table(2, MAX_COLS, false, true)).unwrap_err().contains("more than 63 cells"));
    assert_eq!(crate::io::within_limits("long.html", &table(MAX_ROWS, 1, false, false)), Ok(()));
    assert!(crate::io::within_limits("long.html", &table(MAX_ROWS, 1, true, false)).unwrap_err().contains("more than 32767 rows"));
}

/// WordCraft's JSON with more paragraphs or runs than a document may have (and the separators'
/// and other keys the count can't tell apart) is refused before it is parsed (counted from its
/// keys); at the limit the count lets it through.
#[test]
fn json_past_the_paragraphs_or_runs_is_refused_before_it_is_parsed() {
    use wordcraft_doc::limits::{PARAGRAPHS, RUNS};
    let slack = crate::io::JSON_KEY_SLACK;
    for (key, limit, what) in [("runs", PARAGRAPHS + slack, "paragraphs"), ("len", RUNS + slack, "runs")] {
        let at = format!("[{}]", vec![format!("{{\"{key}\": 1}}"); limit].join(","));
        assert_eq!(crate::io::json_within_limits("at.json", at.as_bytes()), Ok(()), "{key}");
        let past = format!("[{}, {{\"{key}\":1}}]", at.trim_start_matches('[').trim_end_matches(']'));
        let err = crate::io::json_within_limits("past.json", past.as_bytes()).unwrap_err();
        assert!(err.contains(what), "{err}");
        // A key named in a string, escaped, isn't counted; nor is one not followed by a colon.
        let text = format!("[{}, \"\\\"{key}\\\": \", \"{key}\"]", at.trim_start_matches('[').trim_end_matches(']'));
        assert_eq!(crate::io::json_within_limits("text.json", text.as_bytes()), Ok(()), "{key}");
    }
    let err = crate::io::open_bytes("past.json", format!("[{}]", vec!["{\"len\":1}"; RUNS + slack + 1].join(",")).as_bytes()).unwrap_err();
    assert!(err.contains("limit exceeded"), "{err}");
}
