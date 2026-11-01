# Etch parity with Microsoft Word

How close Etch is to Word, area by area. Parity is depth and fidelity against Word, not just
presence, and is estimated. The target is Microsoft Word (Microsoft 365) for Mac 16.113, observed
black-box with synthetic documents, plus Word's public documentation; Windows-only features (PDF
Reflow, Works/WordPerfect import) count too. The ribbon and menu catalog is measured:
`cargo xtask parity` writes [parity-checklist.md](parity-checklist.md). Ranked shortfalls are in
[gaps.md](gaps.md).

## Feature areas

| Area | Parity | What works | Missing |
|---|---|---|---|
| Typing, selection, clipboard, undo, find/replace | 85% | Word's typing behaviour ([typing-parity.md](typing-parity.md)), Paste Special, Advanced Find, Column Selection | Clipboard pane |
| Character and paragraph formatting | 82% | Nearly every property, Font and Paragraph dialogs, character borders, kerning, Phonetic Guide, Enclose Characters | Asian Typography; ligature and stylistic-set options in the Font dialog |
| Lists | 78% | Bullets, numbering, multilevel lists, restart and set value, Word's list AutoFormat and Enter/Backspace behaviour | Define New Multilevel List depth, list styles, legal-numbering edge cases |
| Tables | 76% | Merge and split, table styles and custom styles saved in .docx, floating tables, rows split across pages, sort, formulas, Draw Table and Eraser, dragging borders and ruler markers, cell selection, insert buttons, AutoFit, right-click table commands, cell text direction | Splitting or erasing around vertically merged cells; nested-table polish |
| Equations | 75% | OMML read and write, OpenType MATH layout, an in-place editor with an Equation tab, long display equations wrap at operators | Ink equations |
| Styles and themes | 74% | Style gallery and pane, create and modify, style sets, themes, Style Inspector | Style separators, linked-style edge cases; a smaller set of style sets and themes than Word's |
| Review | 72% | Comments, track changes (insertions, deletions, formatting, moves, table rows and cells), balloons, Compare and Combine with settings, Restrict Editing with exceptions and a password | Drag-and-drop moves, Translate, Block Authors, modern comment threads and mentions |
| View | 72% | Print, web, draft, read and focus modes; zoom with pages side by side; Navigation pane; Outline view | Side by Side, Synchronous Scrolling, Arrange All, Switch Windows, Master Document |
| Page layout and sections | 70% | Margins, size, columns, breaks, page borders, line numbers, vertical alignment, drop caps, hyphenation, vertical sections | Column balancing ([layout-parity.md](layout-parity.md)) |
| Headers, footers, page numbers, fields | 70% | First, even and odd pages, link to previous, TOC as a field, cross-references; Update Fields settles every field from the layout; fields nested in field codes survive save, paste and undo | Many of Word's ~90 field codes; the Field dialog |
| References | 65% | TOC, captions, index, table of authorities, citations and bibliographies in all 12 of Word's styles, Zotero, Word's `Sources.xml` import | EndNote and Mendeley desktop integration |
| Footnotes and endnotes | 60% | Placed and editable, continued across pages, the Footnote and Endnote dialog with per-section options | — |
| Mailings | 60% | Envelopes and Labels | Envelope and Label Options depth, printing them directly. No mail merge, by design |
| Pictures, shapes, text boxes, WordArt | 60% | Insert, crop, recolour, styles, floating placement and wrap, free rotation and flips, text-box text direction and linked text boxes, WordArt fills, outlines, effects and warps, glued connectors, icons, Drawing Canvas | Text in a freely rotated text box, shape effects, WordArt 3-D, online pictures, screenshots |
| Backstage, printing, options | 58% | New and Home pages, save prompts, desktop printing with printer choice, copies, duplex and grayscale | Driver trays and options, pages per sheet; many of Word's Options panes |
| Right-to-left and complex scripts | 50% | Bidirectional text, Arabic shaping, right-to-left paragraphs, tables and sections | Kashida justification, a mirrored interface |
| Accessibility | 50% | Alt text for every object kind, the Accessibility checker with one-click fixes, the document canvas exposed to screen readers | Testing with real screen readers; reading-order tools |
| Proofing | 44% | Spelling, grammar and hyphenation in English, Polish, Portuguese, Italian, Croatian, Turkish, Russian, Czech and Indonesian, chosen per run from its language; thesaurus; custom dictionary | The other ~40 languages Word proofs; Editor's style refinements |
| East Asian typography | 25% | IME input, vertical text, Phonetic Guide, document grid | `vert` glyph alternates, two lines in one, Asian line-break options |
| Charts, SmartArt, Draw/ink, 3D models, icons | 3% | Etch's own charts and SmartArt, created and edited; charts and SmartArt from Word drawn and kept; Draw tab pens, Lasso Select, Ink to Shape, Ink Replay | Editing charts and SmartArt from Word, Ink to Math, pen pressure, Word's own ink, 3D models |
| Scripting | beyond Word | Every action is a command usable from `etch-cli` and macros | VBA and Office add-ins |
