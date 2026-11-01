//! Workspace tooling: `cargo xtask <command>`.
//!
//! Pure Rust (std + serde_json). External tools (`cargo`, `curl`, `tar`) are
//! invoked through `std::process::Command`.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod assets;
mod ico;
mod layers;
mod parity;
mod stats;
mod version;
mod visual_diff;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const USAGE: &str = "\
usage: cargo xtask <command>

commands:
  assets          check that every icon/image/font/asset is attributed in ATTRIBUTION.md
  layers          enforce the crate dependency layering (docs/architecture.md)
  wasm            cargo check --target wasm32-unknown-unknown for the wasm-safe crates
  ci              fmt --check, clippy -D warnings, test, assets, layers, wasm (stops at first failure)
  quick [--base REF]
                  the checks a small change needs: fmt, assets, layers, then clippy and tests of
                  the crates changed since REF (default: the closest of hardening, main
                  and the branch's upstream), the snapshots when
                  a crate below the engine changed, wasm and automation only where they apply
  corpus [--download]
                  show where test corpora live; --download fetches PngSuite into corpus/pngsuite
  stats [--exact] count tests and lines per crate (--exact: ask the test harness via `-- --list`)
  parity          recompute the Word feature-parity summary in docs/parity-checklist.md
  visual-diff <ref.pdf> <test.pdf> <outdir> [--private] [--dpi N]
                  compare two PDFs of a document page by page (SSIM, line breaks, offsets);
                  needs poppler (pdftoppm, pdftotext)
  ico <out.ico> <in.png>...
                  pack square PNGs (<= 256 px) into a Windows .ico (see packaging/icons.sh)
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();
    let result = match args.first().map(String::as_str) {
        Some("assets") => assets::run(&root()),
        Some("layers") => cmd_layers(),
        Some("wasm") => cmd_wasm(),
        Some("ci") => cmd_ci(),
        Some("quick") => cmd_quick(&rest),
        Some("corpus") => cmd_corpus(rest.contains(&"--download")),
        Some("stats") => stats::run(&root(), rest.contains(&"--exact")),
        Some("parity") => parity::run(&root()),
        Some("ico") => ico::run(&rest),
        Some("visual-diff") => visual_diff::run(&rest),
        Some("version") => version::run(&root(), &rest),
        Some("-h" | "--help" | "help") | None => {
            print!("{USAGE}");
            Ok(())
        }
        Some(other) => Err(format!("unknown command `{other}`\n\n{USAGE}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Workspace root (parent of the xtask crate).
pub fn root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest.parent().unwrap_or(manifest).to_path_buf()
}

pub fn cargo() -> Command {
    let mut c = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    c.current_dir(root());
    c
}

fn run(mut cmd: Command, what: &str) -> Result<(), String> {
    eprintln!("$ {what}");
    let status = cmd.status().map_err(|e| format!("{what}: failed to spawn: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("{what}: exited with {status}")) }
}

/// `cargo metadata` with the resolved dependency graph (the layering rules walk it for UI crates
/// reached indirectly).
pub fn metadata() -> Result<serde_json::Value, String> {
    let out = cargo().args(["metadata", "--format-version", "1"]).output().map_err(|e| format!("cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(format!("cargo metadata failed:\n{}", String::from_utf8_lossy(&out.stderr)));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo metadata: bad JSON: {e}"))
}

fn cmd_layers() -> Result<(), String> {
    let crates = layers::from_metadata(&metadata()?)?;
    println!("Dependency layering (docs/architecture.md)\n");
    println!("{:<28} {:<14} workspace deps", "crate", "layer");
    for c in &crates {
        let ws: Vec<String> = c
            .deps
            .iter()
            .filter(|d| d.workspace)
            .map(|d| {
                let k = match d.kind {
                    layers::DepKind::Normal => "",
                    layers::DepKind::Dev => " (dev)",
                    layers::DepKind::Build => " (build)",
                };
                format!("{}{k}", layers::short_name(&d.name))
            })
            .collect();
        println!("{:<28} {:<14} {}", c.name, layers::describe(layers::classify(&c.name)), ws.join(", "));
    }
    let violations = layers::check(&crates);
    println!();
    if violations.is_empty() {
        println!("OK: {} crates, no layering violations.", crates.len());
        Ok(())
    } else {
        println!("{} violation(s):", violations.len());
        for v in &violations {
            println!("  - {v}");
        }
        Err(format!("{} layering violation(s)", violations.len()))
    }
}

/// Workspace packages that must build for wasm32: all L0–L5 crates plus
/// the egui shell.
fn wasm_set() -> Result<Vec<String>, String> {
    let crates = layers::from_metadata(&metadata()?)?;
    Ok(crates
        .into_iter()
        .filter(|c| match layers::classify(&c.name) {
            Some(layers::Class::Layer(l)) => l <= 5 || layers::short_name(&c.name) == "ui-egui",
            Some(layers::Class::Standalone) => true,
            _ => false,
        })
        .map(|c| c.name)
        .collect())
}

fn cmd_wasm() -> Result<(), String> {
    let set = wasm_set()?;
    let mut results = Vec::new();
    for pkg in &set {
        let mut c = cargo();
        c.args(["check", "--target", "wasm32-unknown-unknown", "-p", pkg]);
        let ok = run(c, &format!("cargo check --target wasm32-unknown-unknown -p {pkg}")).is_ok();
        results.push((pkg.clone(), ok));
    }
    println!("\nwasm32-unknown-unknown check:");
    for (p, ok) in &results {
        println!("  {:<6} {p}", if *ok { "ok" } else { "FAIL" });
    }
    let failed = results.iter().filter(|r| !r.1).count();
    if failed == 0 { Ok(()) } else { Err(format!("{failed} crate(s) failed the wasm check")) }
}

fn cmd_ci() -> Result<(), String> {
    type Step = (&'static str, Box<dyn Fn() -> Result<(), String>>);
    let steps: Vec<Step> = vec![
        (
            "fmt",
            Box::new(|| {
                let mut c = cargo();
                c.args(["fmt", "--all", "--", "--check"]);
                run(c, "cargo fmt --all -- --check")
            }),
        ),
        (
            "clippy",
            Box::new(|| {
                let mut c = cargo();
                c.args(["clippy", "--workspace", "--all-targets", "--", "-D", "warnings"]);
                run(c, "cargo clippy --workspace --all-targets -- -D warnings")
            }),
        ),
        (
            "test",
            Box::new(|| {
                let mut c = cargo();
                c.args(["test", "--workspace"]);
                run(c, "cargo test --workspace")
            }),
        ),
        // The desktop app's control channel is a non-default feature (test and development builds
        // only); lint and test it too, so the control server and its confinement stay covered.
        (
            "automation",
            Box::new(|| {
                let mut c = cargo();
                c.args(["clippy", "-p", "wordcraft", "--all-targets", "--features", "automation", "--", "-D", "warnings"]);
                run(c, "cargo clippy -p wordcraft --all-targets --features automation -- -D warnings")?;
                let mut c = cargo();
                c.args(["test", "-p", "wordcraft", "--features", "automation"]);
                run(c, "cargo test -p wordcraft --features automation")
            }),
        ),
        ("assets", Box::new(|| assets::run(&root()))),
        ("layers", Box::new(cmd_layers)),
        ("wasm", Box::new(cmd_wasm)),
    ];
    let mut done = Vec::new();
    for (name, f) in &steps {
        eprintln!("\n=== ci: {name} ===");
        if let Err(e) = f() {
            println!("\nCI summary:");
            for d in &done {
                println!("  ok    {d}");
            }
            println!("  FAIL  {name}: {e}");
            for (n, _) in steps.iter().skip(done.len() + 1) {
                println!("  skip  {n}");
            }
            return Err(format!("ci failed at `{name}`"));
        }
        done.push(*name);
    }
    println!("\nCI summary: all {} steps passed ({})", done.len(), done.join(", "));
    Ok(())
}

/// Files changed since `base` (committed on this branch or not), relative to the root.
fn changed_files(base: &str) -> Result<Vec<String>, String> {
    let git = |args: &[&str]| -> Result<String, String> {
        let out = Command::new("git").args(args).current_dir(root()).output().map_err(|e| format!("git: {e}"))?;
        if !out.status.success() {
            return Err(format!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let since = git(&["merge-base", "HEAD", base])?;
    let mut files: Vec<String> = git(&["diff", "--name-only", since.trim()])?.lines().map(str::to_string).collect();
    files.extend(git(&["ls-files", "--others", "--exclude-standard"])?.lines().map(str::to_string));
    files.sort();
    files.dedup();
    Ok(files)
}

/// The branch a change is measured against: of `hardening`, `main` and the current branch's
/// upstream, the one whose common ancestor with HEAD is fewest commits back (a stale local
/// `main` left from before a fork loses to the branch the work started from).
fn default_base() -> String {
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(root())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    ["hardening", "main", "@{upstream}"]
        .into_iter()
        .filter_map(|b| {
            let base = git(&["merge-base", "HEAD", b])?;
            let behind: u64 = git(&["rev-list", "--count", &format!("{base}..HEAD")])?.parse().ok()?;
            Some((behind, b.to_string()))
        })
        .min()
        .map_or_else(|| "HEAD".to_string(), |(_, b)| b)
}

/// `cargo xtask quick`: the checks a small change needs, not the whole gate (see `USAGE`).
fn cmd_quick(rest: &[&str]) -> Result<(), String> {
    let base = rest.iter().position(|a| *a == "--base").and_then(|i| rest.get(i + 1)).map_or_else(default_base, |b| b.to_string());
    let files = changed_files(&base)?;
    let meta = metadata()?;
    let root = root();
    // Each workspace package with its folder, relative to the root.
    let mut packages: Vec<(String, String)> = Vec::new();
    for p in meta["packages"].as_array().into_iter().flatten() {
        let (Some(name), Some(manifest)) = (p["name"].as_str(), p["manifest_path"].as_str()) else { continue };
        let dir = Path::new(manifest).parent().and_then(|d| d.strip_prefix(&root).ok()).map(|d| d.to_string_lossy().into_owned());
        if let Some(dir) = dir.filter(|d| !d.is_empty()) {
            packages.push((name.to_string(), format!("{dir}/")));
        }
    }
    let mut changed: Vec<String> =
        packages.iter().filter(|(_, dir)| files.iter().any(|f| f.starts_with(dir.as_str()))).map(|(n, _)| n.clone()).collect();
    changed.sort();
    let below_engine = changed.iter().any(|n| matches!(layers::classify(n), Some(layers::Class::Layer(0..=3) | layers::Class::Standalone)));
    let workspace_wide =
        files.iter().any(|f| f == "Cargo.toml" || f == "Cargo.lock" || f.starts_with(".cargo/") || f == "clippy.toml" || f == "rustfmt.toml");
    eprintln!("changed since {base}: {} file(s); crates: {}", files.len(), if changed.is_empty() { "none".to_string() } else { changed.join(", ") });
    if workspace_wide {
        eprintln!("workspace manifests or lint settings changed: running the full gate");
        return cmd_ci();
    }
    let mut steps: Vec<String> = Vec::new();
    let mut step = |name: &str, f: &dyn Fn() -> Result<(), String>| -> Result<(), String> {
        eprintln!("\n=== quick: {name} ===");
        f().map_err(|e| format!("quick failed at `{name}`: {e}"))?;
        steps.push(name.to_string());
        Ok(())
    };
    step("fmt", &|| run_args(&["fmt", "--all", "--", "--check"]))?;
    step("assets", &|| assets::run(&root))?;
    step("layers", &cmd_layers)?;
    if !changed.is_empty() {
        let mut clippy = vec!["clippy".to_string()];
        let mut test = vec!["test".to_string()];
        for n in &changed {
            clippy.extend(["-p".into(), n.clone()]);
            test.extend(["-p".into(), n.clone()]);
        }
        clippy.extend(["--all-targets", "--", "-D", "warnings"].map(String::from));
        step("clippy", &|| run_args(&clippy.iter().map(String::as_str).collect::<Vec<_>>()))?;
        step("test", &|| run_args(&test.iter().map(String::as_str).collect::<Vec<_>>()))?;
        if below_engine && !changed.iter().any(|n| n == "wordcraft-engine") {
            // Layout, formats and fonts show up first in the engine's golden outputs.
            step("snapshots", &|| run_args(&["test", "-p", "wordcraft-engine", "--test", "snapshots*"]))?;
        }
        if changed.iter().any(|n| n == "wordcraft") {
            step("automation", &|| {
                run_args(&["clippy", "-p", "wordcraft", "--all-targets", "--features", "automation", "--", "-D", "warnings"])?;
                run_args(&["test", "-p", "wordcraft", "--features", "automation"])
            })?;
        }
        let wasm: Vec<String> = wasm_set()?.into_iter().filter(|n| changed.contains(n)).collect();
        if !wasm.is_empty() {
            step("wasm", &|| {
                for n in &wasm {
                    run_args(&["check", "--target", "wasm32-unknown-unknown", "-p", n])?;
                }
                Ok(())
            })?;
        }
    }
    println!("\nquick: {} passed ({}). Not the full gate: `cargo xtask ci` before publishing.", steps.len(), steps.join(", "));
    Ok(())
}

/// Runs `cargo <args>` from the root.
fn run_args(args: &[&str]) -> Result<(), String> {
    let mut c = cargo();
    c.args(args);
    run(c, &format!("cargo {}", args.join(" ")))
}

const PNGSUITE_URL: &str = "http://www.schaik.com/pngsuite/PngSuite-2017jul19.tgz";

fn cmd_corpus(download: bool) -> Result<(), String> {
    let corpus = root().join("corpus");
    println!(
        "Test corpora live under {} (git-ignored, never committed).
Tests that use a corpus skip cleanly when it is absent.

  corpus/pngsuite/   PngSuite (public domain) — wordcraft-codecs compares every file
                     against the `image` crate. Fetch: cargo xtask corpus --download
  corpus/psd/        PSD samples from MIT/BSD projects (ag-psd, psd-tools test data).
                     Copy files in manually; licences must be MIT/BSD/CC0.
  corpus/tiff/       libtiff pics (optional)
  corpus/exr/        OpenEXR sample images (optional)
  corpus/raw/        raw.pixls.us samples, CC0 (optional)
",
        corpus.display()
    );
    if !download {
        return Ok(());
    }
    let dest = corpus.join("pngsuite");
    std::fs::create_dir_all(&dest).map_err(|e| format!("create {}: {e}", dest.display()))?;
    let tgz = corpus.join("PngSuite-2017jul19.tgz");
    let mut curl = Command::new("curl");
    curl.args(["-fsSL", "-o"]).arg(&tgz).arg(PNGSUITE_URL);
    run(curl, &format!("curl {PNGSUITE_URL}"))?;
    let mut tar = Command::new("tar");
    tar.arg("-xzf").arg(&tgz).arg("-C").arg(&dest);
    run(tar, "tar -xzf PngSuite-2017jul19.tgz")?;
    let _ = std::fs::remove_file(&tgz);
    let n = std::fs::read_dir(&dest).map(|d| d.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "png")).count()).unwrap_or(0);
    println!("PngSuite: {n} PNG files in {}", dest.display());
    if n == 0 { Err("no PNG files extracted".into()) } else { Ok(()) }
}
