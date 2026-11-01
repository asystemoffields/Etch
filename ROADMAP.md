# Etch roadmap

**Stage: alpha.** The everyday workflows (write, format, styles, lists, tables, references,
review, open and save .docx) work end to end. Etch is tested on Linux.

## Priorities

1. **Durability.** Never lose or damage someone's writing.
2. **Correctness.** Behave like Word and keep documents faithful when they're opened and saved.
3. **Speed and weight.** Stay fast on long documents and light on memory.
4. **Dark mode** that is good, not just inverted.

New features come after these.

## Known limitations

- Tested on Linux only. The macOS, Windows, BSD and browser builds compile but haven't been tried,
  and there are no published binaries yet.
- .docx fidelity is tested on synthetic documents, not yet on a broad set of real-world files.
- Pagination can differ from Word, mostly where a document's fonts aren't installed (Aptos has no
  metric-matched open substitute).
- Charts and SmartArt from Word have limited editing; OLE objects show only their picture.
- Fewer dialogs and default shortcuts than Word.
- Proofing (spelling, grammar, hyphenation) covers nine languages.
- .doc files open but can't be saved; Word 6/95 files don't open.
- Password-protected .docx files open and save; the encryption code hasn't had an outside security
  review.
- No VBA, add-ins, cloud storage or co-authoring. No mail merge (Envelopes and Labels are there).

The full list, ranked: [docs/gaps.md](docs/gaps.md).

## Parity with Word

Estimated against Microsoft Word for Mac 16.113. The ribbon and menu catalog is measured by
`cargo xtask parity` ([docs/parity-checklist.md](docs/parity-checklist.md)).

### By dimension

| Dimension | Parity | Doc |
|---|---|---|
| Features (depth) | 68% | [target-app-parity.md](docs/target-app-parity.md#feature-areas) |
| UI/UX fidelity | 65% | [ui-parity.md](docs/ui-parity.md) · [typing-parity.md](docs/typing-parity.md) |
| File formats | 60% | [file-format-parity.md](docs/file-format-parity.md) |
| Layout and pagination fidelity | 55% | [layout-parity.md](docs/layout-parity.md) |
| Hardware | 40% | [hardware-parity.md](docs/hardware-parity.md) |
| Localization | 27% | [localization-parity.md](docs/localization-parity.md) |
| Performance | 75% | [layout-parity.md](docs/layout-parity.md#performance) |
| Stability | 50% | [gaps.md](docs/gaps.md) |
| Platforms | 95% (beyond Word on Linux, BSD, web) | this page |
| Ecosystem and plugins | 15% | [gaps.md](docs/gaps.md) |

### By feature area

| Area | Parity |
|---|---|
| Typing, selection, clipboard, undo, find/replace | 85% |
| Character and paragraph formatting | 82% |
| Lists | 78% |
| Equations | 75% |
| Tables | 76% |
| Styles and themes | 74% |
| Review (comments, track changes, compare, protect) | 72% |
| View | 72% |
| Page layout and sections | 70% |
| Headers, footers, page numbers, fields | 70% |
| References (TOC, citations, bibliography, captions, index) | 65% |
| Mailings (Envelopes and Labels; no mail merge) | 60% |
| Footnotes and endnotes | 60% |
| Pictures, shapes, text boxes, WordArt | 60% |
| Backstage, printing, options | 58% |
| Right-to-left and complex scripts | 50% |
| Proofing (spelling, grammar, thesaurus, languages) | 40% |
| Accessibility | 35% |
| East Asian typography | 25% |
| Charts, SmartArt, Draw/ink, 3D models, icons | 3% |
| Scripting (CLI, macros; control channel in test builds) | beyond Word |

Evidence per area: [docs/target-app-parity.md](docs/target-app-parity.md#feature-areas).
