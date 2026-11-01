# Where Etch falls short of Word

What's missing, ranked by how much it stops a Word user from switching. Each entry links to the
parity document that has the detail.

| # | Gap | Who it affects | Detail |
|---|---|---|---|
| 1 | **Real-world .docx files.** Reading and writing are tested on synthetic documents, not yet on a broad set of files from Word, Google Docs, LibreOffice and Pages compared with Word page by page. | Everyone who opens files they receive | [file-format-parity.md](file-format-parity.md) |
| 2 | **Platforms.** Tested on Linux. The macOS, Windows, BSD and browser builds compile but haven't been tried; there are no published binaries yet. | Everyone not on Linux | [hardware-parity.md](hardware-parity.md) |
| 3 | **Pagination differs from Word** in places: Aptos has no metric-matched open substitute, Word's legacy compatibility options are only partly covered, and there is no page-by-page comparison with Word. | Forms, legal and academic work | [layout-parity.md](layout-parity.md) |
| 4 | **Word's ink and OLE objects.** Ink from Word shows as its picture rather than as editable strokes; OLE objects show only their picture and can't be opened. | Pen notes; documents with embedded objects | [file-format-parity.md](file-format-parity.md) |
| 5 | **Review depth.** Content controls, move tracking, formatting revisions and tracked table rows and cells work. Missing: drag-and-drop moves, tracked style-definition changes, an Original view, and tracked merges, splits and sorts. Custom XML bindings aren't evaluated. | Legal and editorial review | [file-format-parity.md](file-format-parity.md) |
| 6 | **Printing.** Printer choice, copies, collation, duplex and grayscale work; driver trays and options and pages per sheet don't yet. | Everyone who prints | [hardware-parity.md](hardware-parity.md) |
| 7 | **Dialog depth.** 59 dialogs against Word's ~100. | Power users | [ui-parity.md](ui-parity.md) |
| 8 | **Proofing languages.** Spelling, grammar and hyphenation cover English, Polish, Portuguese, Italian, Croatian, Turkish, Russian, Czech and Indonesian; text in other languages is left unmarked. | Writers in other languages | [localization-parity.md](localization-parity.md) |
| 9 | **Objects.** Text in a freely rotated text box, shape effects, WordArt 3-D effects, connector routing around shapes, and dragging members inside a Drawing Canvas. | Newsletters, flyers, diagrams | [target-app-parity.md](target-app-parity.md) |
| 10 | **Interface languages.** Hindi, Arabic, Indonesian and European Portuguese are missing, there's no mirrored right-to-left interface, and about 5% of strings in the translated catalogs are still English. | Most of the world's writers | [localization-parity.md](localization-parity.md) |
| 11 | **Draw tab.** Pens, highlighter, eraser, selection, Ink to Shape and Ink Replay work; Ink to Math and pen pressure don't yet. | Pen and tablet users | [hardware-parity.md](hardware-parity.md) |
| 12 | **Charts and SmartArt.** Etch's own charts work; editing charts and SmartArt from Word, and more chart types and formatting, don't yet. | Reports and business documents | [target-app-parity.md](target-app-parity.md) |
| 13 | **.doc can't be written**, and Word 6/95 files don't open. | Exchanging files with old systems | [file-format-parity.md](file-format-parity.md) |
| 14 | **References.** Zotero works; EndNote and Mendeley desktop don't. | Students and researchers | [target-app-parity.md](target-app-parity.md) |
| 15 | **Right-to-left completeness.** Kashida justification, and right-to-left text in HTML, ODT and RTF. | Arabic, Persian and Hebrew writers | [layout-parity.md](layout-parity.md) |
| 16 | **East Asian typography.** Some Asian Typography options. | Chinese, Japanese and Korean documents | [layout-parity.md](layout-parity.md) |
| 17 | **Screen readers.** The document canvas is exposed to assistive technology but hasn't been tried with real screen readers. | Blind and low-vision users | [ui-parity.md](ui-parity.md) |
| 18 | **RTF and ODT depth.** Some formatting revisions, RTF section export, and right-to-left text. | Exchanging files with LibreOffice and older tools | [file-format-parity.md](file-format-parity.md) |
| 19 | **Large real-world documents.** Performance is measured on synthetic documents (500+ pages, many pictures). | Thesis and book authors | [layout-parity.md](layout-parity.md) |
| 20 | **Ecosystem.** No VBA (macros are kept, not run), Office add-ins, cloud storage or co-authoring. | Teams; macro templates | [target-app-parity.md](target-app-parity.md) |
| 21 | **Default shortcuts.** About 160 of Word's ~250 (Customize Keyboard can add more). | Power users | [ui-parity.md](ui-parity.md) |
| 22 | **PDF Reflow** (opening a PDF as an editable document). | Users who edit PDFs they receive | [file-format-parity.md](file-format-parity.md) |
| 23 | **View windows.** Side by Side, Synchronous Scrolling, Arrange All, Switch Windows. | Comparing documents | [ui-parity.md](ui-parity.md) |
| 24 | **Minor formats.** MHT, Works, WordPerfect. | Rare | [file-format-parity.md](file-format-parity.md) |
| 25 | **Dictate.** | Dictation users | [hardware-parity.md](hardware-parity.md) |
| 26 | **Equations.** Ink equations (Ink to Math). | Maths-heavy documents | [layout-parity.md](layout-parity.md) |

Not planned: mail merge (Envelopes and Labels are there, and merge fields in documents you
receive are kept) and AI features.

How saving protects your work is described in [save-flow.md](save-flow.md) and
[recovery.md](recovery.md).
