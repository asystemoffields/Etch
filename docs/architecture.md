# Etch architecture

How Etch is built. Rules for working in the code are in [`AGENTS.md`](../AGENTS.md).

## Layers

~319,000 lines of Rust (≈198k production, ≈121k tests, measured 2026-10-25), 16 crates, 3 apps and `xtask`. Layering is enforced by `cargo xtask layers`:
a crate may only depend on crates in lower layers, and nothing below L6 depends on egui, eframe,
winit or rfd. `tz` and `control-key` are standalone (no workspace dependencies). Prod lines exclude
test files and trailing test modules (shell count, 2026-10-25); `cargo xtask stats` prints total lines and
tests per crate (engine 81.8k, ui-egui 65.6k, docx 37.5k, layout 35.2k in all).

| Layer | Crate | Prod lines | Job |
|---|---|---|---|
| L0 | `geom` | 0.6k | Units (points, twips, EMU, inches, cm), rectangles, measurement parsing |
| L0 | `tz` | 0.7k | Local time: the offset in force at an instant, from TZif zone files (RFC 8536) and POSIX TZ rules read by our own reader (Unix), the browser (web) or the Windows Runtime calendar (Windows); standalone, no dependencies on Unix |
| L1 | `doc` | 19.7k | Document model: stories, paragraphs, runs, tables, sections, styles, numbering, fields, notes, comments, revisions, equations (OMML-shaped math tree, linear/LaTeX input); edit primitives; bidi |
| L1 | `fonts` | 4.4k | Font database (bundled OFL + system fonts), metrics, outlines, shaping with HarfRust, Word font substitution (`word.rs`: Carlito ↔ Calibri, Caladea ↔ Cambria, Liberation ↔ Arial/Times/Courier), OpenType MATH |
| L1 | `metafile` | 2.0k | WMF and EMF reader into a neutral vector model (paths, RGBA bitmaps) per MS-WMF/MS-EMF; pure Rust over byte slices, every count capped; used by `render` |
| L1 | `proof` | 10.5k | Proofing per language (`ProofLang` from a run's `w:lang`): English (public-domain word list) and Polish (installed Hunspell dictionary through a Hunspell-style affix engine) spelling and suggestions, rule-based grammar for both, hyphenation dictionary and Liang patterns (English, Polish) |
| L2 | `layout` | 19.4k | Shaping runs, line breaking, tabs, lists, tables (row splitting, floating tables), pagination, headers/footers, notes, floating objects and wrap, math layout, fields, hit testing; produces a display list |
| L2 | `docx` | 23.4k | OOXML WordprocessingML reader and writer (Transitional and Strict namespaces, `.docx/.docm/.dotx/.dotm`), OMML, custom properties, VBA parts preserved |
| L2 | `docbin` | 4.9k | Word 97-2003 `.doc` reader ([MS-DOC]: FIB, piece table, FKPs, SPRMs, stylesheet, lists, tables, notes, pictures) over a CFB container |
| L2 | `formats` | 17.6k | TXT, Markdown, HTML, RTF, ODT and LaTeX import/export through a shared intermediate model |
| L3 | `render` | 2.6k | vello_cpu rasteriser: pages and thumbnails from the display list; PNG/JPEG |
| L3 | `pdf` | 1.8k | krilla PDF export: font subsets, selectable text, links, outline, tagging |
| L4 | `engine` | 41.1k | `Session` (document, selection, undo, layout cache, view state), **command registry** (617 `CommandSpec`s in `cmd/*.rs`), Word feature catalog (`catalog.rs`), samples, file I/O dispatch (`io.rs`), Read Aloud sentence model |
| L5 | `zotero` | 1.2k | Zotero's word-processor integration protocol answered against a `Session` |
| L4 | `control-key` | 0.1k | Per-window control-port key files (desktop app's `automation` feature only) |
| L6 | `ui-egui` | 42.7k | Word-style front end: ribbon, keytips, mini-toolbar, Backstage, canvas, rulers, panes, dialogs, Equation tab, i18n catalogs, themes, in-process control dispatcher, headless `ui_shot` example |
| apps | `wordcraft` (binary `etch`) | 3.0k | Desktop app (eframe + wgpu; logging, window geometry, file associations; the localhost control server only with the non-default `automation` feature) |
| apps | `wordcraft-cli` (binary `etch-cli`) | 0.8k | `convert`, `text`, `inspect`, `render`, `run`, `commands`, `parity`, `zotero` |
| apps | `wordcraft-web` | 0.8k | Browser build (eframe web runner, WebGPU with WebGL2 fallback) |

## Data flow

1. **Open:** `engine::io` picks a reader by extension (`docx`, `docbin`, `formats`, JSON) and
   produces a `doc::Document`.
2. **Edit:** every user-visible action is a command id run through `Session::run` (ribbon,
   shortcuts, command search, CLI and control channel all dispatch by id). `Session::run`
   snapshots for undo (blocks are `Arc`-shared, and styles, numbering, comments, revisions,
   secondary stories, media and table rows are `doc::Shared`, an `Arc` that copies on write, so
   snapshots are cheap and an edit copies only what it changes) and catches an escaped
   panic, restoring the document (never-crash standard).
3. **Layout:** `layout` turns the document into pages of positioned glyphs, boxes and images
   (display list), incrementally: paragraph layouts are cached by revision, and `layout_after`
   resumes pagination from a checkpoint on the page of the first changed body block, taking the
   earlier pages (shared, not copied) and, once the pagination is back in step, the later ones
   from the last layout (`crates/layout/src/resume.rs`); anything it can't vouch for is laid out
   whole.
4. **Draw:** the UI paints pages through `render` (CPU raster, cached per page, pixel-aligned) on
   an egui/wgpu canvas; PDF export walks the same display list through `pdf`.
5. **Save:** `engine::io` picks a writer by extension.

## Testing and gates

- 821 `#[test]` functions and 10 property-test blocks: model, layout, DOCX round-trip and hostile
  input, `.doc`, formats, typing parity (`engine/src/tests_typing.rs`), bidi, i18n catalogs, UI
  interaction (egui_kittest), control-channel confinement; `hostile_params_never_panic` runs every command with
  junk params.
- `cargo xtask ci`: fmt, clippy `-D warnings`, tests, the app's `automation` feature (clippy and
  tests), asset attribution, layering, wasm build.
- `cargo xtask parity` regenerates [`parity-checklist.md`](parity-checklist.md); a test keeps the
  catalog percentage above a floor that only rises.
- GitHub Actions on PRs: packaging lint, and FreeBSD / Windows ARM64 when build files change;
  release builds on pushes to `release`.
