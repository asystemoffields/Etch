//! The save and command snapshot families (see `snapshots.rs` for the whole suite). They run in
//! a binary of their own because the command scripts load system faces by name (a bullet's
//! "Arial"), and a face once loaded would serve as fallback for every later layout in the
//! process, which would make the layout, PDF and render goldens depend on test order.
#![cfg(not(target_arch = "wasm32"))]

#[path = "snapshots/mod.rs"]
mod snapshots;

use std::collections::BTreeMap;
use std::fmt::Write as _;

use snapshots::corpus::{self, Doc};
use snapshots::dump;
use snapshots::golden::{self, Failures};
use wordcraft_engine::io;

fn selected(docs: Vec<Doc>) -> Vec<Doc> {
    docs.into_iter().filter(|d| golden::wanted(d.name)).collect()
}

/// The formats the save snapshots cover: a Word package, an ODT package and the text formats.
const SAVE_EXTS: &[&str] = &["docx", "odt", "rtf", "html", "md", "txt"];

#[test]
fn save_snapshots() {
    let mut fails = Failures::default();
    let docs = corpus::corpus();
    // The blank document's parts, so each golden holds only the parts that differ from them.
    let mut blank: BTreeMap<&str, BTreeMap<String, String>> = BTreeMap::new();
    if let Some(b) = docs.iter().find(|d| d.name == "blank") {
        for ext in ["docx", "odt"] {
            match io::save_bytes(&format!("blank.{ext}"), &b.doc).and_then(|bytes| dump::package_parts_text(&bytes)) {
                Ok(parts) => {
                    blank.insert(ext, parts);
                }
                Err(e) => fails.push(format!("--- save/blank.{ext}: can't save the blank document: {e}")),
            }
        }
    }
    for d in selected(docs) {
        for ext in SAVE_EXTS {
            let name = format!("{}.{ext}", d.name);
            let save = || io::save_bytes_opts(&name, &d.doc, &d.export);
            let bytes = match save() {
                Ok(b) => b,
                Err(e) => {
                    fails.push(format!("--- save/{name}: the save failed: {e}"));
                    continue;
                }
            };
            // (The blank document's own golden holds every part in full.)
            let reference = if d.name == "blank" { None } else { blank.get(ext) };
            let text = match *ext {
                "docx" | "odt" => match dump::package_text(&bytes, reference) {
                    Ok(t) => t,
                    Err(e) => {
                        fails.push(format!("--- save/{name}: not a readable package: {e}"));
                        continue;
                    }
                },
                _ => dump::text_file_text(ext, &bytes),
            };
            // Saving twice must give the same bytes: a writer that iterates a hash map, or reads
            // the clock where the dump doesn't blank it, would make the golden flaky.
            if let Ok(again) = save()
                && again != bytes
            {
                let second = match *ext {
                    "docx" | "odt" => dump::package_text(&again, reference).unwrap_or_default(),
                    _ => dump::text_file_text(ext, &again),
                };
                if second != text {
                    let mut msg = format!("--- save/{name}: two saves of the same document differ (the writer is not deterministic):\n");
                    msg.push_str(&golden::text_diff(&format!("save/{name} (first save vs second save)"), &text, &second));
                    fails.push(msg);
                }
            }
            golden::check_text(&mut fails, "save", &name, &text);
        }
    }
    fails.finish("save");
}

#[test]
fn command_snapshots() {
    let mut fails = Failures::default();
    for script in corpus::scripts() {
        if !golden::wanted(script.name) {
            continue;
        }
        let mut s = corpus::session(script.start);
        let mut out = format!("# command snapshot: {} ({} steps)\n", script.name, script.steps.len());
        let mut prev = dump::model_text(&s.doc, Some(&s.sel));
        let _ = writeln!(out, "step 0: start\n{prev}");
        for (i, (id, params)) in script.steps.iter().enumerate() {
            let result = s.run(id, params);
            // The layout after each command, as the window makes it: a command whose result
            // can't be laid out fails here.
            let _ = s.layout();
            let outcome = match &result {
                Ok(v) => format!("ok {}", clip(&v.to_string())),
                Err(e) => format!("error {}", clip(&e.to_string())),
            };
            let _ = writeln!(out, "step {}: {id} {params} -> {outcome}", i + 1);
            let now = dump::model_text(&s.doc, Some(&s.sel));
            let changes = golden::changed_lines(&prev, &now);
            if changes.is_empty() {
                out.push_str("  (no change)\n");
            } else {
                out.push_str(&changes);
            }
            if std::env::var_os("WC_SNAPSHOT_OUT").is_some() {
                let dir = golden::out_dir().join("commands").join(script.name);
                let _ = std::fs::create_dir_all(&dir);
                let _ = std::fs::write(dir.join(format!("step-{:02}.txt", i + 1)), &now);
            }
            prev = now;
        }
        let _ = writeln!(out, "# final state\n{prev}");
        golden::check_text(&mut fails, "commands", script.name, &out);
    }
    fails.finish("commands");
}

fn clip(s: &str) -> String {
    if s.chars().count() <= 160 { s.to_string() } else { format!("{}…", s.chars().take(160).collect::<String>()) }
}
