# Etch — instructions for agents and contributors

Etch is a clean-room, open-source word processor in Rust that works the way Microsoft Word does. It has no AI or agent features; the JSON control channel exists only in test builds. It runs on Linux, macOS, Windows and the BSDs, and in the browser through WebAssembly.

Where things stand: [`ROADMAP.md`](ROADMAP.md) (stage, priorities, known limitations) and [`docs/gaps.md`](docs/gaps.md) (the ranked work list).

## Never crash
People trust Etch with their writing; a crash loses their work. **This outranks feature work**: never ship a feature through a panic path, and fix a crash before building on top of it.
- **No panics in non-test code:** no `unwrap()`, `expect()`, `panic!`, `unreachable!`, `todo!`, `unimplemented!`; no `unsafe` (`unsafe_code = "forbid"`).
- **Errors are `Result<T, E>`** through the crate's error type and `?`. An unfinished feature returns an error or is disabled; it never panics.
- **Input-derived numbers are hostile** (files, commands, control params): `get()` instead of `[i]`, slice strings only at char boundaries, checked/saturating arithmetic, clamp sizes, cap allocations, bound recursion.
- **Last-resort guard:** `Session::run` turns an escaped panic into an error and restores the document. It's a safety net, not a licence.
- **Prove it:** every crash fix lands with a regression test. `crates/engine/src/tests.rs::hostile_params_never_panic` runs every command with junk params — keep it green.
- Production crate roots carry `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]`; new crates start with it. Tests may `unwrap` (`clippy.toml`).

## Assets: absolutely no Microsoft (or Adobe/Avid/Autodesk) iconography or images
**This rule is non-negotiable.** Breaking it is the worst mistake you can make in this repo.
- Never copy, trace, screenshot-crop or recreate icons, images, templates, themes, clip art, fonts, cursors, sounds or any other asset from Microsoft Word/Office or any Microsoft, Adobe, Avid or Autodesk product. Not even "temporarily".
- Every asset must be **original** (drawn in code — prefer this: `crates/ui-egui/src/icons.rs`), **public domain / CC0**, **OSI open source**, or **Creative Commons that allows redistribution**, or licensed open source by a contributor who made it.
- **Every asset file has a row in [`ATTRIBUTION.md`](ATTRIBUTION.md)** (author, source, licence). `cargo xtask assets` (part of `cargo xtask ci`) fails otherwise. Add the row in the same commit as the file.
- Fonts: no new font files in this repo (optional extra fonts come from `CRAFT_FONTS_DIR` at build time). Documents asking for proprietary fonts (Calibri, Aptos, Cambria…) are rendered with installed system fonts or metric-compatible open substitutes (`crates/fonts/src/word.rs`) — never bundle proprietary fonts.
- Templates, sample documents and themes are our own text and colours (`crates/engine/src/sample.rs`, `cmd/design.rs`). Demo images are public domain, with sources recorded.

## Clean room
- Microsoft Word may be **observed** black-box: run it and use it with synthetic documents you made. Never read, disassemble or copy anything inside Word's installation (file names only). Never commit files produced by Word, or screenshots of it. Never copy Microsoft wording beyond feature names.
- No downloaded documents: test with synthetic documents built in code, never with files from bug trackers or the web.
- File formats come from public specs: ECMA-376 / ISO 29500 (OOXML), the published RTF spec, OASIS ODF 1.3. Never copy GPL/LGPL/AGPL code (LibreOffice, AbiWord, pandoc…), not even for names, and nothing from leaked source.

## Architecture (see [`docs/architecture.md`](docs/architecture.md))
| Layer | Crate | Job |
|---|---|---|
| L0 | `geom` | units, rects, measurement parsing (`tz` and `control-key` are standalone) |
| L1 | `doc`, `fonts`, `metafile`, `proof` | document model + edit primitives; font DB, shaping, Word font substitution; WMF/EMF reader; proofing |
| L2 | `layout`, `docx`, `formats` | line breaking, pagination, tables, headers/footers, hit testing; OOXML I/O; TXT/MD/HTML/RTF/ODT |
| L3 | `render`, `pdf` | vello_cpu rasteriser; krilla PDF |
| L4 | `engine` | Session, undo, **command registry**, Word feature catalog, samples, I/O dispatch |
| L5 | `zotero` | Zotero citation protocol |
| L6 | `ui-egui` | Word-style egui front end (swappable) |
| apps | `apps/wordcraft`, `apps/wordcraft-cli`, `apps/wordcraft-web` | binaries |

- **Everything is a command.** User-visible behaviour = a `CommandSpec` in `crates/engine/src/cmd/*.rs` (id, label, ribbon location, shortcut, params doc, `enabled`, `run`) + tests. UI-only commands (`ui.*`) live in `crates/ui-egui/src/lib.rs`. The ribbon, shortcuts, command search, CLI and control channel all dispatch by id. Programmatic calls never open dialogs.
- **Parity is measured:** `crates/engine/src/catalog.rs` lists Word's ribbon/menu features with command ids. `cargo xtask parity` writes `docs/parity-checklist.md`; a test enforces a floor that only rises. Implement missing ids to raise it.
- **Layering** is enforced by `cargo xtask layers`. Nothing below L6 depends on egui/eframe/winit/rfd. The UI is thin and reads `Session` state; colours come from `theme::Tokens`.
- **Rust only** (no handwritten JS/TS). **Never break wasm** (`cargo xtask wasm`).

## Quality gate
- **Small, contained change** (a UI tweak, a one-crate fix, docs): `cargo xtask quick` before committing. It runs fmt, the asset and layering checks, then clippy and the tests of the crates changed since the branch the work started from, the engine snapshots when a crate below the engine changed, and the automation and wasm checks only where they apply; a change to the workspace manifests or lint settings runs the full gate instead.
- **Full gate** `cargo xtask ci` (fmt, clippy -D warnings, tests, automation, assets, layers, wasm): for changes to saving, file formats or recovery, and before a release.

## Snapshots
`crates/engine/tests/snapshots.rs` pins behaviour on a synthetic corpus (`tests/snapshots/corpus.rs`, bundled fonts only, stopped clock): layouts, saves (DOCX, ODT, RTF, HTML, Markdown, text), PDFs, rendered pages and two scripted command sequences, each against a golden under `crates/engine/tests/goldens/`. It runs with `cargo xtask ci`; alone: `cargo test -p wordcraft-engine --test 'snapshots*'`. A failure prints each differing hunk with the page, part or step it is in, where the actual output was written, and the bless command. **A pure refactor changes no golden.** For an intended change, bless one family at a time and read `git diff crates/engine/tests/goldens/` before committing: `WC_BLESS=1 cargo test -p wordcraft-engine --test 'snapshots*' layout_snapshots` (families `layout_`, `save_`, `pdf_`, `render_`, `command_`; `WC_SNAPSHOT_DOC=name[,name]` limits it to some documents; a new `render/*.png` golden needs an `ATTRIBUTION.md` row). Heavier, ignored by default: `cargo test -p wordcraft-engine --test snapshots render_every_page -- --ignored`.

## Typing behaves like Word
Typing, Enter, Backspace, Tab, lists, AutoCorrect and AutoFormat follow Word's observed behaviour in [`docs/typing-parity.md`](docs/typing-parity.md), pinned by `crates/engine/src/tests_typing.rs` (keys go through the same commands as the keyboard). Change behaviour, doc and tests together; a typing fix lands with a test there. Compare headlessly (`ui_shot`).

## Running and looking at the app
- `cargo run --release -p wordcraft --features automation -- --sample --control 7981` (sample document + control channel). The control channel exists only with the non-default `automation` feature (tests and development); a default or release build has no socket and no key file, and refuses `--control` / `WORDCRAFT_CONTROL_PORT` with exit code 2. `cargo xtask ci` lints and tests the feature too.
- Drive it with JSON lines on `127.0.0.1:7981`, each with the window's key from `~/.config/etch/control-key.local-7981` (macOS: `~/Library/Application Support/Etch/`), e.g. `{"id":1,"key":"…","method":"engine.execute","params":{"command":"text.insert","params":{"text":"Hello"}}}` then `{"id":2,"key":"…","method":"ui.screenshot","params":{"path":"/tmp/shot.png"}}`. Methods: `crates/ui-egui/src/control.rs`; docs: `docs/control-protocol.md` (Keys).
- **Headless window screenshots** (no focus stealing, works with a locked screen): `cargo run --release -p wordcraft-ui-egui --example ui_shot -- script.jsonl` (see the example's header).
- **For UI work, look at the result** (screenshot, read the PNG), and compare with Word where you can.
- CLI: `etch-cli convert in.docx out.pdf`, `etch-cli run --template sample --cmd 'select.text={"text":"Studio"}' --cmd format.bold --save out.docx`, `etch-cli commands`, `etch-cli parity`.
- Control connections can be confined to folders (`--automation-read-root DIR` / `--automation-write-root DIR`, or `auth` with roots): `docs/control-protocol.md` (Confinement).
- Parallel agents: separate `CARGO_TARGET_DIR` per agent; edit only the crates you own; delete your target dir when done (disk).

## Docs
`ROADMAP.md` (stage, priorities, known limitations), `docs/gaps.md` (ranked shortfalls), `docs/target-app-parity.md` (the assessment), `docs/architecture.md`, and the parity checklists `docs/file-format-parity.md`, `docs/layout-parity.md`, `docs/typing-parity.md`, `docs/ui-parity.md`, `docs/hardware-parity.md`, `docs/localization-parity.md`. They describe Etch as it is now: when a change closes or narrows a gap, update the gap and its parity doc in the same commit. No progress logs or revision histories.

## Contributor credits (About window)
About ▸ Contributors is compiled from `contributors/contributors.json` (generated, never hand-edit) and `contributors/people.toml` (names people chose for themselves; see [`docs/contributors.md`](docs/contributors.md)). Add only your own entry, never anyone else's.
