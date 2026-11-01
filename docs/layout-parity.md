# Layout and pagination parity with Microsoft Word

A document only "opens correctly" if its pages break where Word breaks them: the same lines, the
same page count, footnotes and pictures on the same pages. This checklist covers line breaking,
pagination and object placement in `crates/layout`. Typing behaviour is in
[`typing-parity.md`](typing-parity.md); the reading and writing of the properties is in
[`file-format-parity.md`](file-format-parity.md).

**Dimension: ~55% (estimated).** The engine implements most of Word's layout
rules, but nothing has been compared page-for-page with Word on real documents, and the default
font can't match Word's metrics.

Additive paragraph spacing retains the full before-space after a section page break; ordinary overlapping spacing is unchanged. Synthetic control: `layout::tests::section_start_summed_spacing`.

Exact line spacing keeps the run font baseline around tall equations; automatic and minimum spacing still grow for math. Synthetic control: `layout::tests::exact_math_keeps_run_baseline`.

## Checklist

✅ matches Word as far as our tests show · 🟡 implemented, differs or untested against Word · ❌ missing

| Feature | State | Evidence / gap |
|---|---|---|
| 2026-10-16 | trivial | Split-cell content retains its resolved top margin and border band on continuation pages, matching original Word Mac16.113.2 controls. Synthetic regressions cover margins, borders, line spacing and image frames. |
| Hint-reliant CJK glyph outlines (DFKai-SB / 標楷體) | 🟡 | Required TrueType interpreter runs at units-per-em size; local Windows glyph-bounds regression and synthetic DOCX page/window comparison pass. Other platforms and hint-reliant families remain unmeasured; no native Word comparison. |
| Subpoint table grid columns | 🟡 | Preserve positive grid widths rather than imposing a text-box minimum; ECMA-376 §17.4.16. Native Word for Mac 16.113.2 build26092012 confirms narrow trailing grid units. `layout::tests::subpoint_grid_keeps_widths`, `tiny_grid_scales_to_width`. Tiny totals normalize without overflowing the scale factor. Zero and invalid widths retain their fallback. Border painting remains separate. |
| Line breaking (UAX with Word's exceptions: no break after `/`) | ✅ | `layout::tests::no_line_break_right_after_a_slash` |
| A break before a slash that starts a word (" /word" wraps whole; not after a no-break space) | 🟡 | `layout::tests::a_word_after_a_slash_wraps_whole`, `layout::tests::a_no_break_space_before_a_slash_still_holds`; behaviour from the report, not compared with Word yet |
| Narrow table cells | ✅ | `layout::tests::narrow_cells_keep_their_width`: usable widths below 12 pt wrap without expansion; oversized glyphs still make progress |
| Justification, Word 2013+ space shrinking (compat mode 15) | ✅ | #109 |
| Spaces at the end of a line hang past the right margin instead of wrapping; the caret after them stays at the margin, in the body and in headers and footers | ✅ | `layout::tests::caret_after_hanging_spaces_stays_at_the_margin`: a letter typed after them wraps to the next line |
| Complex scripts (Myanmar, Khmer, Indic): shaped per script run with OpenType fallback fonts, one cluster per syllable, Myanmar wraps between syllables | 🟡 | `layout::tests_complex`. Khmer, Thai and Lao still wrap only at spaces (need a dictionary) |
| Kashida justification (Arabic) | ❌ | no kashida in `crates/layout` |
| Line height of Arabic and Hebrew in a missing Windows core font | 🟡 | Times New Roman, Arial and Courier New carry Arabic and Hebrew in their own file (Microsoft typography font list, script tags 'Arab' 'Hebr'), so their lines keep the font's one set of line metrics: with a metric-compatible substitute (Liberation, Tinos, Arimo, Cousine) the line takes the substitute's metrics while a fallback face draws the letters (`layout::tests_bidi::arabic_in_a_missing_times_new_roman_keeps_its_line_height`). Fonts with no metric-compatible open face here, such as Microsoft Sans Serif, keep the fallback face's metrics |
| Missing Thai document fonts (TH Sarabun New, Angsana New, Cordia New, Browallia New, the UPC families, Leelawadee): one Thai substitute face for the whole run, Latin letters, digits and ฿ included, so the run keeps one font at one size as Word's Cordia New substitution does | ✅ | `fonts::word::tests::a_missing_thai_font_resolves_to_an_installed_thai_face`, `fonts::fontdb::tests::a_thai_run_with_latin_falls_back_to_one_face`, `layout::tests_complex::a_missing_thai_font_draws_the_whole_run_in_one_thai_face`, `layout::tests_complex::a_thai_substitute_without_latin_hands_the_run_to_one_face_covering_it`. Windows: Cordia New, then Leelawadee UI, Tahoma, Noto Sans Thai; macOS: Thonburi (Sathu for the serif families); Linux: Noto Sans Thai, then the TLWG fonts. The first listed face with Latin letters and digits as well as Thai wins. Other Thai names (`TH …`, `…UPC`, names with `Thai`) use the same lists. When no listed face is installed, or the one found lacks the run's Thai or Latin letters, the run moves to one installed face covering its Thai and Latin text if there is one. Only Thai fonts: a missing Latin font keeps Latin text in its own substitute, and a run with its own complex-script font keeps it for the Thai letters, as Word does |
| Distributed / Thai distributed alignment | 🟡 | read; layout untested |
| Default font metrics (Aptos, Calibri, Cambria…) | 🟡 | Missing Arial Unicode MS tries Arial then the Arial substitutes, case-insensitively (`fonts::word::tests::unicode_arial_is_proportional`); glyph coverage and exact Word line height can differ. Carlito/Caladea/Liberation are metric-compatible for Calibri/Cambria/Arial/Times/Courier; **Aptos (Word's default since 2024) has no metric-compatible open substitute**, so new Word documents wrap differently ([typing-parity.md](typing-parity.md) Known gaps). Needs a metric-matched open font |
| Colour emoji and colour fonts on screen and in page renders: COLR v0/v1 paint graphs (solid fills, linear, radial and sweep gradients, composite modes) and bitmap strikes (Apple Color Emoji `sbix`, Noto Color Emoji `CBDT`) drawn in the font's colours at the layout's position and size; a dark page doesn't invert them, the same as pictures | 🟡 | `fonts::color` (skrifa), `render::color_glyph`; `render::tests_color_glyphs::emoji_are_drawn_in_colour` and `emoji_keep_their_colours_on_a_dark_page` (system font, skip without one), `render::color_glyph::tests`, `fonts::color::tests`. CBDT is verified on a real page with Noto Color Emoji; COLR v0/v1 and sbix are implemented to the OpenType spec and tested with synthetic data, but not yet seen on a real font (Segoe UI Emoji, COLRv1 Noto, Apple Color Emoji). PDF export already drew them through krilla |
| Full font names in document font requests | ✅ | Exact English (or first available) OpenType name ID4 lookup in loaded faces and the native lazy catalog; family and legacy-family matches keep priority. No style-suffix guessing; named variable-instance aliases remain unsupported |
| Symbol fonts (Symbol including its PCL6 alias, Wingdings, Wingdings 2/3, Webdings) without the proprietary font: each character, stored as U+F020–U+F0FF or a plain 8-bit code, is drawn as its Unicode equivalent from a face that has it; PDF text and copies read as those symbols | ✅ | `doc::symbol_fonts` tables (Adobe Symbol table and Unicode L2/12-368), `layout::tests::wingdings_text_draws_and_reads_as_its_unicode_symbols`. Shapes come from Noto Sans Symbols / Symbols 2, DejaVu Sans or Segoe UI Symbol, so they differ from Word's; Wingdings 0xFF (not in Unicode) still has no symbol |
| List bullets stored as a private-use character (U+E000–U+F8FF) in a font that isn't installed and isn't a mapped symbol-font code: drawn as the level's default bullet (• ○ ▪), not as whatever an installed icon font has at that code | ✅ | `layout::tests::private_use_bullet_no_font_draws_shows_the_levels_default_bullet` (U+F081 showed Font Awesome's Twitter logo) |
| Indents in characters (`w:firstLineChars`, `w:leftChars`…), measured in the paragraph mark's font size | ✅ | `layout::tests::character_indents_use_the_paragraph_mark_size`; spacing in lines (`w:beforeLines`/`w:afterLines`) uses the stored points (needs the document grid) |
| Line spacing (single, 1.5, double, at least, exactly, multiple), spacing before/after, contextual spacing | 🟡 | Contextual spacing measured in body paragraphs and headers (below), with a four-paragraph table-cell regression; list-label ascent overflow is unscaled by multiple spacing and uses descent for automatic and at-least spacing; other line-spacing combinations remain (#135 open) |
| Kerning only where Kerning for fonts asks for it (`w:kern`, at or above its size) | ✅ | Measured in Word for Windows: no `w:kern` or `w:kern="0"` is unkerned; Etch kerned every run, so its lines held more text than Word's. `layout::tests::kerning_follows_kerning_for_fonts_like_word` |
| OpenType ligatures as the document asks (`w14:ligatures`: standard `liga`, contextual `clig`, historical `hlig`, discretional `dlig`, any combination, in runs, styles and defaults) | 🟡 | `layout::tests::ligatures_follow_w14_ligatures`, `ligature_flags_map_to_opentype_features`. Without the property Etch keeps drawing standard and contextual ligatures, while Word draws none; the Font dialog's Ligatures choice is still missing |
| Automatic (HTML-style) paragraph spacing (`w:beforeAutospacing`, `w:afterAutospacing`) | ✅ | Measured in Word for Windows: 14 pt at any size, none at the top of the document or a cell or after a cell's last paragraph, the larger of two neighbouring spaces. `layout::tests::automatic_paragraph_spacing_like_word`, `…_drops_at_cell_edges` |
| Paragraph boundaries inside field instructions | ✅ | Synthetic DOCX observed in Word: instruction-only paragraphs add no height; the empty result keeps a line. `docx::fixtures::field_code_spans_paragraphs`, `layout::tests::hidden_marks_take_no_space`. Other field-code display modes remain unverified |
| Widow/orphan control, keep with next, keep lines together, page break before | ✅ | `lib.rs` keep_next/keep_lines; a line holding only a page break stays at the bottom of a full page instead of leaving a blank page; a paragraph ending with a page break keeps its space after on the old page, so the next page starts at the top (compatibility modes 12 to 15, `layout::tests::space_before_at_the_top_of_a_page_like_word`); a three-line paragraph with room for two moves whole (neither a widow nor an orphan is allowed), as in Word |
| Tabs (left, center, right, decimal, bar), leaders | 🟡 | Bar tabs untested. Positional tabs (`w:ptab`, ECMA-376 §17.3.3.23) go to the left edge, centre or right edge of the margins or indents whatever the tab stops, and to the next line when the text has passed that place (`layout::tests::right_positional_tab_ends_text_on_the_right_margin` and the three after it); not compared with Word on lines narrowed by floating objects or right-to-left paragraphs |
| Hyphenation (automatic, zone from the document, words in capitals, soft hyphens, consecutive limit, Manual Hyphenation) | 🟡 | English patterns only; other languages hyphenate wrongly or not at all |
| Drop caps (dropped and in margin, lines, distance, font; `w:framePr w:dropCap`) | ✅ | 2026-10-17 |
| All Caps and Small Caps: the full capital of each letter, so ß draws as SS (ﬀ as FF); the capitals drawn for one character are one cluster, so the caret, selection, hit testing and line breaking treat ß as one character | ✅ | `layout::tests::caps_draw_sharp_s_as_two_capitals_in_one_cluster`. The PDF text layer still reads the document's lowercase letters until |
| Columns, column breaks, separators, balancing | ✅ | Columns balance on a section's last page when the next section starts continuous (also at the document end, which Word only balances when the document ends with a continuous break); not after next-page/even/odd breaks or on a page with a column break; keep-together, keep-with-next, widow/orphan control and unsplittable rows can leave them slightly uneven |
| Section breaks (next page, continuous, even, odd), different first page, odd/even headers | ✅ | Section breaks don't show in Draft view. A break inserted with text selected goes before the selection and keeps it, as in Word. A deleted break joins its text to the section after it, whose setup wins; the headers and footers that section linked to the deleted one stay (`tests_styles::deleting_a_section_break_joins_the_section_after_it`). Link to Previous works on the header or footer being edited, in the section of the page it is edited on, for that kind (first page, even pages, default) only, and switches: unlinking copies what the section showed (`…link_to_previous_switches_the_edited_headers_section_and_kind`). Columns with text selected put it in a continuous section of its own. A continuous section after a page break that left the section before only its empty closing mark on the new page starts that page, with its own first-page header (`layout::tests_sections::a_continuous_section_after_a_page_break_starts_the_page_with_its_own_header`); a next-page section there still starts a page of its own after it (Word not compared) |
| Style precedence: document defaults → table style → paragraph style chain → character style chain → direct formatting (ECMA-376 §17.7.2) | 🟡 | `doc::resolve::tests::defaults_then_paragraph_style_then_character_style_then_direct`, `table_style_sits_between_defaults_and_paragraph_style`. Toggle properties (§17.7.3: bold, italic and their complex-script forms, caps, small caps, strike, hidden, outline, shadow, emboss, engrave): a character style turning one on flips the value below it (italic Emphasis in an italic Quote is upright), off leaves it; paragraph styles set them outright (MS-OI29500 2.1.258); within a character style's own chain the most derived value counts (`character_styles_toggle_bold_and_italic`, `style_chains_set_toggle_properties_outright`). From the specification; not yet compared with Word (linked character styles over their own paragraph style especially) |
| Section breaks (next page, continuous, even, odd), different first page, odd/even headers | ✅ | Section breaks don't show in Draft view. A break inserted with text selected goes before the selection and keeps it, as in Word. An otherwise empty section at page start retains its line (`empty_page_section_keeps_line`); a closing mark after text stays quiet. |
| Labels on page-break-only list paragraphs | ✅ | `layout::tests::break_only_list_has_no_label`: no bullet or number on the break, but the next number is still 3 after item 1 and a break. Plain empty items keep their labels. Synthetic DOCX observed through Adobe PDF conversion |
| List labels: number formats (decimal, letters, Russian letters, roman, ordinal text, Chinese/Japanese/Korean counting and legal, 一〇一 digits, 甲乙丙, ①…⑳, full-width), `%n` text such as `第%1章` / `（%1）`, legal numbering | ✅ | `doc::cjk_numbers` tests, `doc::section::tests::russian_letters_follow_word`, `layout::tests::chinese_counting_list_labels_and_their_font_runs`, `layout::tests::russian_letter_list_labels_draw_cyrillic`; a label mixing faces (digit + CJK comma) is drawn run by run; Russian letters skip ё, й, ъ and ь and then double (аа, бб), as observed in Word. Hebrew, Arabic, Hindi, Thai and Vietnamese formats show decimal The label stays attached to the start of the text when the paragraph is centred or aligned to the end, as in Word. |
| Body top below a tall header, footer pushes body up | ✅ | a footer taller than the bottom margin moves the body bottom up, and a header or footer linked to the previous section takes its room too |
| Tables: row border exceptions (`w:tblPrEx/w:tblBorders`) | ✅ | `row_borders_stay_on_row`, `row_nil_overrides_style`: explicit cell borders win; row exceptions precede table styles and table borders |
| Footnotes at the page bottom, endnotes at the end, note numbering | 🟡 | A long note continues onto the next page at a line boundary (its first line stays with the reference), first in that page's note area under a full-width continuation separator; the document's own separator, continuation separator and continuation notice are drawn (the notice under a note that continues); on a page with footnotes a paragraph ending there keeps its space after above the separator, or its last lines go to the next page with the note; a long endnote continues on the next column or page at a line boundary, and the caret finds note text on its continuation page; each section's note options: footnotes below the text, endnotes at the end of each section, start at, numbering restarted each section or each page (counted from where the notes land, at most three passes) |
| Tables: row heights (at least/exact), rows split across pages, header rows repeat, Can't Split | ✅ | 2026-10-06; omitted cell-margin edges inherit current table defaults; every cell in a row gets the row's largest top and bottom margin; an exact height covers the top border, top margin and text, the bottom margin comes on top, and an exact row never splits; a row split across pages keeps its cells' bottom margin, border and last space after on the first page, widow/orphan control applies in its cells, each cell continues at the top of the next page and an at-least row keeps its minimum; an at-least row with less room than its minimum on a page that already holds something starts the next page whole (Allow row to break on or off, vAlign centre too), as observed in Word for Mac 2026-10-15 (a row of about 2.6 cm); that repeated header rows go with it and that a row taller than a page splits from the top of the next page come from the issue and synthetic tests, not a Word check (`…_an_at_least_row_with_no_room_starts_the_next_page_whole_either_way_breaking_is_set`, `…_an_at_least_row_taller_than_a_page_splits_from_the_top_of_the_next_page`, `layout::tests::a_row_shares_its_largest_top_and_bottom_margins`, `…_a_split_row_keeps_its_bottom_margin_and_widow_control`, `…_split_row_cells_each_continue_at_the_top`) |
| Tables: autofit to contents/window, fixed widths, Word 2013 edge | 🟡 | #44: AutoFit Contents measures each column's narrowest and widest text and shares the width like Word, once (not live as you type); consistent a percentage width past 100% is kept, as Word does; cell widths override stale grid proportions when their totals agree (`cell_widths_override_grid`); conflicting widths and spans still differ |
| Floating tables (`w:tblpPr`) | 🟡 | one placed on the page or margin that would cover visible text before it and does not fit below it moves to the next page; empty line-break lines do not count as text; other overlap rules untested |
| Nested tables | 🟡 | render; polish missing |
| Floating pictures/shapes placement (relative to page/margin/column/paragraph/line/character) | ✅ | in a header or footer table cell, a float not laid out in its cell (`layoutInCell="0"`, VML `o:allowincell="f"`, or any VML shape in a header or footer: user-checked in Word for Mac) and placed relative to the page or margins is placed on the page, drawn with the header behind or in front of its text, on every page: Word's picture watermark in a header that starts with a table is centred behind the body text (`layout::tests::header_cell_float_placed_on_the_page`). Other cell floats are still placed relative to their cell |
| Text wrap: square, top-and-bottom, behind, in front | ✅ | 2026-10-06; beside an object, a line's first word that doesn't fit the strip goes on whole to the row's next strip or the next row, instead of being split (`layout::tests::a_word_too_wide_for_the_strip_beside_a_float_moves_on_whole`; inferred from a reporter's comparison, not yet measured in Word) a paragraph running on into the next column or page is broken again there around that column's shapes, its own floating objects left where they are. Text in an earlier column doesn't yet wrap around a shape anchored in a later one |
| Text wrap: tight, through (contour, wrap polygon) | 🟡 | lines follow the wrap polygon, or the outline worked out from the object (shape geometry; a picture's opaque pixels as a convex hull; otherwise the rectangle), turned and flipped with it, kept the left/right distance from text; Through also uses gaps inside the outline. Wrap side (`wrapText`: both sides, left only, right only, largest side per line) applies to square, tight and through wraps, turned objects included; picture outlines are convex; not compared page-for-page with Word |
| Text boxes, overflow, linked text boxes | ✅ | linked text boxes: the text runs on through each box in turn at that box's line length, overflow past the last box hidden; Align Text top / middle / bottom; the box's own internal margins, and a box set to Resize shape to fit text shows all its text |
| Rotated objects and text | 🟡 | pictures, shapes, charts and groups drawn turned and flipped about their centre (screen and PDF); square wrap and inline lines keep clear of the rotated bounds; hit testing on the turned shape. Text in a rotated text box stays upright |
| Page borders (from page edge or text), page colour, watermark | ✅ | |
| Line numbers (restart per page/section, count by) | ✅ | |
| Vertical page alignment | ✅ | Top, Center, Bottom and Justified: justified shares each page's spare room equally between its paragraphs and tables, first line on the top margin and last on the bottom one (above the footnotes), the last page and a page ending in a manual page break too; a page with one block stays at the top; the last paragraph's space after stays inside the justified height (Word for Mac, user-checked). A section's last page takes its own section's alignment (it took the next section's before). Multi-column justified pages stay at the top (not compared with Word) |
| Gutter, mirror margins, book fold, 2 pages per sheet | 🟡 | gutter and mirror margins (even pages put the inside margin and gutter on the right; page-relative floats stay put, and inside/outside alignment and margins follow the page side); book fold missing |
| Vertical text (`tbRl`) in sections and text boxes; cell text direction | ✅ | cell text direction; text-box text direction; vertical sections (`tbRl`/`btLr`): the page's text area turned, lines top to bottom stacking right to left, ideographs, kana and Hangul upright, Latin on its side, caret and clicks through the turn. Missing: OpenType `vert` alternates (punctuation is moved, not substituted), arrow keys along vertical lines, upright pictures take turned room |
| Document grid (`w:docGrid`, lines per page, characters per line) | 🟡 | line grid snaps line heights to whole pitches (text centred in them; `w:snapToGrid` off opts a paragraph out); character grid puts East Asian characters on the character pitch (`linesAndChars`) or all text (`snapToChars`), Latin text proportional; View › Gridlines draws it. Not yet: run-level `w:snapToGrid`, grid in headers/footers, paragraph spacing in grid lines, Word's exact glyph placement in a cell |
| Ruby / phonetic guide, enclose characters, combined characters | 🟡 | ruby (Phonetic Guide): ruby text over its base by alignment, line grows to fit, in PDF; enclose and combined characters missing; ruby not yet compared with Word page by page |
| Automatic space between Asian text and Latin text or digits (`w:autoSpaceDE`, `w:autoSpaceDN`) | 🟡 | a quarter of the Asian character's font size after the first of two touching characters, across runs (measured on Word's PDF of a Chinese document at 16 pt, as JIS X 4051 asks); not beside East Asian punctuation, Hangul, Latin punctuation or an existing space; a field is classed by the text it shows; none at a line's end, none on a character grid the paragraph snaps to (`layout::tests_autospace`). Not yet: justification widening the space as Word does, `autoSpaceLikeWord95` |
| Bidirectional paragraphs, mixed-direction lines; RTL tables and sections | 🟡 | RTL tables: grid columns from the right edge, start alignment and indent from the right margin, cell margins and left/right borders mirrored, merged and turned cells and nested tables included; RTL sections: newspaper columns right to left with the line between mirrored. Right-hand page gutter (`w:rtlGutter`) missing |
| Equations: display layout, numbering, wrapping long display equations | ✅ | long display equations break at top-level operators (`m:brkBin`, `m:brkBinSub`), manual breaks and `m:alnAt`, continuation lines indented by `m:wrapIndent` or set right (`m:wrapRight`); an n-ary operator, delimiter, accent or group character the math font lacks is drawn from a fallback font instead of a missing-glyph box |
| Hidden text excluded from breaking and hyphenation | ✅ | #173 |
| Hidden text in prints, PDFs and page images | ✅ | Word's Print hidden text (File › Options › Display, off by default) decides; the on-screen ¶ display doesn't (research item 28; `tests_word_parity::print_hidden_text_decides_prints_and_pdfs_not_the_screen`) |
| Table cell highlighting in the markup views (inserted / deleted cells and rows) | ✅ | Word's default colours (light blue, pink; configurable), drawn under the cell's content by `display::page_display`, dimmed on the dark page; merged and split cells aren't recorded by Word or Etch (research item 2; `wordcraft-render tests::tracked_cells_are_highlighted_in_markup_light_and_dark`) |
| Fields: PAGE, NUMPAGES, SECTIONPAGES, TOC page numbers | ✅ | totals in the body need a second pass once the page count is known; SECTIONPAGES includes shared continuous-section start pages |
| Compatibility modes (Word 2003/2007/2010 layout for old files: `w:compat` options) | 🟡 | `allowSpaceOfSameStyleInTable` supported in modes 11/12/14; mode 15 retains contextual suppression. Most other legacy compat options remain ignored |
| Fields: PAGE, NUMPAGES, SECTIONPAGES, TOC page numbers, PAGEREF in headers and footers | ✅ | totals in the body need a second pass once the page count is known; a PAGE field in a paragraph that runs onto a later page (a page break inside it, or a natural split) shows that page's number, the rest of the paragraph broken again for the wider result; PAGEREF in headers and footers shows its bookmark's page, filled in after pagination like their totals; in the body it keeps its result until fields are updated. Update Fields takes TOC, table-of-figures and PAGEREF pages from the layout with the generated entries in place, in the page's section number format, laying out again until they hold (`tests_fields`) |
| Fields: STYLEREF in headers and footers (running chapter titles) | ✅ | Evaluated per page at layout time: the page's first (or with `\l` last) paragraph of the style, else the nearest before it; style name, id or heading level (a level finds the style named `heading N` whatever its id, so localized documents match); character styles; `\n \r \w \s \t \p` (`\r` treated as `\w`); body STYLEREF updated with the other fields. Field results with right-to-left text (a Hebrew heading, "פרק א") are laid out with bidi reordering like ordinary text: the result is an isolate taking its first strong letter's direction (else the text's around the field), and each level run is shaped and mirrored in its own direction, so `{ PAGE }` in field-code view and `(Chapter 1)` keep their brackets in right-to-left paragraphs; this covers every field and note mark shown as text. Header paragraphs are cached on their per-page results, so typing in body text no STYLEREF shows doesn't lay the headers out again |
| Fields: PAGE, NUMPAGES, SECTIONPAGES, TOC page numbers, PAGEREF in headers and footers | ✅ | totals in the body need a second pass once the page count is known; SECTIONPAGES includes shared continuous-section start pages; a PAGE field in a paragraph that runs onto a later page (a page break inside it, or a natural split) shows that page's number, the rest of the paragraph broken again for the wider result; PAGEREF in headers and footers shows its bookmark's page, filled in after pagination like their totals; in the body it keeps its result until fields are updated |
| Page-by-page comparison harness against Word | ❌ | Run Word locally on our own synthetic documents, export PDF, compare line breaks; Word output stays under `plan/` (never committed) |

## Missing black-circle glyphs

When the requested face lacks U+25CF (●), try installed Times New Roman before the UI sans
fallback, checking glyph coverage. Existing primary glyphs, bold/italic selection and fallback
when that face is absent remain unchanged. This is an observed Word for Mac compatibility
preference, not an OOXML font requirement. Synthetic font-boundary regressions cover catalog
loading, coverage, exclusion, styles and disabled system lookup. A metric control retains tall
labels and pins the 0.682 pt ascent overflow of an 11 pt Source Sans 3 label over Arial text.
Other missing characters and paragraph wrapping are outside this change.
An original embedded-font control matches native Word inline fallback to Times while preserving
a covered primary glyph. A tall present-glyph label retains a 19.628 pt pitch (Word mean 19.632 pt).
The combined numbering-definition blocks are not used to infer their individual font choices.

## Floating tables across header and body

A body floating table with `tblOverlap=never` clears floating header tables.
Allowed overlap retains its requested position. Original native controls confirm
both cases. Earlier-body reflow around late tables remains separate.

## List-label line spacing

Controlled original DOCX probes against Word for Mac 16.113.3 (26092714), macOS 26.6.2,
compatibility mode 15, zero paragraph gaps, Roboto Regular 12 pt body text: a label's positive
ascent overflow adds to the text line pitch without being multiplied by automatic spacing.
Label descent does not enlarge automatic or AtLeast spacing. A label's ascent includes positive
hhea leading. The original base split text leading across ascent/descent; current main assigns
its full gap to ascent too, so labels now use that shared metric. The table records the original
measurements, not a new native comparison of this rebase. These are
observations of synthetic documents, not a claim about Word's internal implementation.

| Label / spacing | Word baseline gap | Fixed Etch | Before fix |
|---|---:|---:|---:|
| Roboto 24 pt / single | 25.20 pt | 25.1953 pt | 28.125 pt |
| Roboto 24 pt / 1.25 | 28.80 pt | 28.7109 pt | 35.1563 pt |
| Roboto 24 pt / double | 39.36 pt | 39.2578 pt | 56.25 pt |
| Apple Symbols 12 pt / AtLeast 12 pt | 13.92 pt | 14.0625 pt | 14.6338 pt |
| Apple Symbols 24 pt / AtLeast 12 pt | 20.88 pt | 20.9297 pt | 24.0 pt |

Actual font metrics and original-input provenance were checked locally; Word-derived exports
remain uncommitted. Tolerance is 0.25 pt; this evidence covers Latin text with the tested symbol
and decimal labels, automatic single/1.25/double spacing, AtLeast controls, Exactly 24 pt,
wrapped paragraphs and save/reopen. Regression tests inject the independently measured metrics
so CI does not require those installed fonts. Inline pictures retain their existing height floor.
Picture bullets are not implemented, and the private Linux Symbol substitution report has not
been reproduced. Absolute baseline-leading placement remains the separate contribution.
This narrows gap without raising the overall readiness estimate.

## Legacy table contextual spacing

Observed in Word for Mac 16.113.3 (build 26092714) on macOS 26.6.2, using
original synthetic DOCX files, Roboto Regular 12 pt, exactly 24 pt lines, equal paragraph
styles, both paragraphs contextual, first paragraph after-space 12 pt and before-space 0 pt.
The one-cell table had no borders or cell margins. Baseline separation was 36 pt with
`w:allowSpaceOfSameStyleInTable` enabled in compatibility modes 11, 12 and 14, versus
24 pt with it disabled. Mode 15 measured 24 pt in either state. Body and header controls
in modes 14 and 15 measured 24 pt with the flag enabled. Comparison tolerance: 0.25 pt.

The model and DOCX reader/writer preserve the flag. Cell layout bypasses contextual
suppression only for enabled modes 11/12/14, including cells without table-style formatting.
Tests: `legacy_table_contextual_spacing_flag_keeps_after_space` and
`legacy_table_contextual_spacing_flag_is_scoped_to_cells_and_legacy_mode`.
Etch original-open and save/reopen measurements match all 12 controls; reopening
its saved DOCX files in Word retains the same gaps. Word-derived exports stay local under
ignored `plan/word/`; no Word-generated fixture is committed.

The [published option](https://learn.microsoft.com/en-us/dotnet/api/documentformat.openxml.wordprocessing.allowspaceofsamestyleintable?view=openxml-3.0.1)
specifies an override inside tables. Its mode 15 exception is not explained by that source;
the guard follows this observation. [MS-DOCX compatibilityMode](https://learn.microsoft.com/en-us/openspecs/office_standards/ms-docx/90138c4d-eb18-4edc-aa6c-dfb799cb1d0d)
allows 11, 12, 14 and 15. Mode 13 is invalid and Word rejected its synthetic input.
Ordinary overlapping before/after space and one-sided contextual spacing remain separate
work in. Nested tables, rotated cells, other fonts and Word for Windows
were not reference-tested here. This closes one compatibility-option gap, not table parity.

## Contextual paragraph spacing

**Observed, 2026-10-11:** original synthetic DOCX probes in Word for Mac 16.113.3
(build 26092714), compatibility mode 15, Roboto Regular 12 pt, Exactly 24 pt line
spacing. With identical paragraph style IDs and contextual spacing enabled only
on the first paragraph, 12 pt after / 8 pt before adds 0 pt to baseline separation;
10 pt after / 12 pt before adds 1.92 pt, within the predeclared 0.25 pt tolerance
of 2 pt. Body paragraphs and headers give the same results. Enabling only the
second paragraph preserves the first paragraph's 12 pt after-space.

The first-only residual agrees with the worked example in
[ISO/IEC 29500-1 §17.3.1.9](https://learn.microsoft.com/en-us/dotnet/api/documentformat.openxml.wordprocessing.contextualspacing?view=openxml-3.0.1).
`contextual_after_removes_a_smaller_before_gap`,
`contextual_after_keeps_only_the_residual_before_gap`, and
`contextual_spacing_preserves_each_side_and_style_identity` pin those relative
gaps and controls in body and header layout. Different style IDs retain their
spacing even when their definitions are identical.

This fixes contextual suppression in `place_para` and `layout_box`. Ordinary
before/after overlap still needs; the uniform first-baseline offset, page
boundaries, tables, text boxes and notes were not compared with Word in this study.
No overall parity estimate changes from these probes.

## Performance

| Measure | Etch | Word | Kind |
|---|---|---|---|
| Cold layout, 188-page document | 61 ms | not measured | measured 2026-10-06 (not re-run this pass) |
| Relayout after an edit | 1.4 ms | not measured | measured 2026-10-06 |
| Relayout after an edit, synthetic: 2,000 paragraphs plain / with a Heading 1 every second paragraph; 600 paragraphs with a 4×4 table every second one | 7.2 / 10.4 / 33.1 ms (was 11.3 / 69.8 / 54.7 ms) | not measured | measured 2026-10-13, release, best of 5 edits (`tests_perf::perf_relayout`) |
| Incremental relayout after typing (`layout_after`), same documents plus one 8,000-row table: at the end / in the middle | 0.2 / 0.3, 0.3 / 0.3, 0.3 / 0.3, 0.2 ms (whole relayout 5.3, 7.5, 29.2, 223.5 ms); typing in a cell in the middle of the 8,000-row table 5.4 ms (only the changed row and its page are laid out again) | not measured | measured 2026-10-14, release, loaded machine, best of 5 edits (`tests_perf::perf_relayout`); equal to a layout from scratch after every edit (`tests_resume`, `engine::tests_relayout`) |
| A key that widens an AutoFit column of an 8,000-row table (every cell laid out again at the new widths; 05's `bigtable`, release, `tests_perf::perf_table_typing`) | 484 ms (was 540; 1,660 before 07), the rows' items made only when their page is read; a key that keeps the columns 8.9 ms (was 9.9) | not measured | measured 2026-10-19 on a shared 4-vCPU VM, three interleaved rounds, medians; layouts equal to from scratch and to rows made at once (`tests_autofit`) |
| Update Fields on a 996-page document (420 chapters, 4,200 tables with SEQ captions, 840 REF/PAGEREF, a table of contents; release, `tests_fields::perf_update_fields`): first update / nothing to change / after typing in the middle | 0.68 / 0.17 / 0.19 s (was 33 / 34 / 38 s at `ba7ed7b`), linear in the document | not measured | measured 2026-10-16 on a shared 4-vCPU VM |
| Relayout per keystroke where it used to be slow (median of 7, release `etch-cli`): Enter at the start of a 684-page document / of an 8,000-row table; typing in a footnote; adding a footnote near the end; typing in an 8,000-row table with numbered cells | 1.3 / 3.4 ms (was 8.0 / 41.0 ms); 0.2 ms (was 7.4); 0.5 ms (was 45.5); 4.7 ms (was 373) | not measured | measured 2026-10-14, before and after back to back; adding a note near the start numbers every later note again (37 ms, was 46); equal to a layout from scratch after every edit (`tests_resume`, `engine::tests_relayout`) |
| Font picker with many installed fonts | fixed freezes | instant | issue |
| Large real-world documents (500+ pages, many pictures) | not measured | | gap |
| Large synthetic documents, open and first layout (release `etch-cli info`, from process start): 630 pages of prose and headings / 319 pages of tables / one 8,000-row table (403 pages) / 300 pages with 3,000 footnotes / 300 pictures (151 pages) / 20 sections with their own headers (180 pages) | 0.70 / 1.73 / 2.25 / 0.59 / 0.12 / 0.24 s, of which layout 0.63 / 1.30 / 1.69 / 0.48 / 0.09 / 0.22 s | not measured | measured 2026-10-16, cloud VM, best of 3 (`engine::tests_perf::perf_profile`) |
| Typing in those documents, relayout per key (median of 30 keys in the middle) | prose 0.7, tables 0.4 (was 2.4), 8,000-row table 7.9 (was 11.1), footnotes 5.5 (was 19.0), pictures 0.5, sections 1.1 ms | not measured | measured 2026-10-16, cloud VM, same tool; equal to a layout from scratch after every edit (`tests_resume::edits_with_notes_waiting_match_layouts_from_scratch`) |
| Rasterising a page at 100% on a 96-dpi screen (vello_cpu) | 3.9–5.2 ms (was 5.1–6.5 ms); the page under the caret after a key 2.6–4.6 ms | not measured | measured 2026-10-16, cloud VM (4 vCPU), same tool; pixels equal to a fresh context's (`render::cache_tests::a_kept_context_draws_what_a_fresh_one_draws`) |
| A key that widens or narrows an AutoFit column (a long word typed or deleted in a cell) of an 8,000-row, 403-page table: cells with preferred widths / content-sized cells; the paragraphs shaped per key | 0.41 / 0.40 s (was 1.66 / 1.66 s); 1 shaped, the other 40,000 cells' lines broken again (was 40,000 shaped); RSS after the typing 465 MB (was 665) | not measured | measured 2026-10-17, cloud VM (load 1.0–3.3, median 1.5), interleaved with the base, three rounds, median of the keys (`engine::tests_perf::perf_table_typing`); equal to a layout from scratch after every key (`tests_autofit`, `engine::tests_relayout::keys_in_an_autofit_table_shape_only_the_cell_typed_in`) |
| PDF export (Print, File › Export › PDF, Save As PDF) of a document the window has laid out: 630 pages of prose / 319 of tables / the 8,000-row table / 300 pages of footnotes | 0.53 / 0.58 / 0.86 / 0.27 s (was 1.0 / 1.75 / 2.8 / 0.71 s): the window's layout is the print layout, so only the PDF is written | not measured | measured 2026-10-17, same; PDF and page-image bytes equal to those from a layout from scratch in 15 view states (`engine::tests_print_layout`) |
| Scrolling a 1,096-page document that is one 5,300-row table | ~2.4 ms idle, ~4.5 ms median scroll frame (was ~600 ms); opening still lays out for ~4–8 s | not measured | measured 2026-10-12, release, headless (`tests_perf::big_table_scroll_bench`) |

TOC refresh includes heading and outline paragraphs inside tables, including nested cells. Links and page numbers retain their full paragraph paths when entries move body blocks. The existing link picker still accepts top-level heading block indices.

Explicit DrawingML preset and custom outline dashes retain their stroke-width units on import
and save, in raster output and in PDF. Synthetic controls cover red rectangles, ellipses and
connectors. Custom lists are capped at 64 pairs; painted spans are bounded to 0.01–20000 pt.
Theme-inherited dash styles, line caps, ink and open freeform strokes are not covered by this change.
When content exceeds a preferred column width, AutoFit donors lose equal widths until they reach their content minima. Original synthetic Word probes cover donor reductions; cached field refresh and wrapping remain separate gaps.

Behind-text pictures in table cells participate in centre and bottom vertical alignment without increasing row flow height.

TOC refresh reuses direct tab formatting from the first cached entry with a number-separator stop at each level for numbered entries, including imported range fields. Page-number tabs still use the TOC section width. No new default number-tab position is inferred.

Explicit cell page breaks (Word for Mac 16.113.2, compatibility modes 14 and 15): leading, interior and trailing breaks take no line height. Ordinary line breaks retain their lines; body page breaks still paginate. Synthetic regressions preserve UTF-8 cluster boundaries, bookmark objects and source offsets, including body/cell cache reuse without table-style formatting. Width, border strokes and residual row metrics remain outside this change.

Footer flow-table strokes reserve their drawn top edge, including the half-width above the cell box. Original Word for Mac 16.113.2 mode-15 controls confirm the three footer cases move to the next page. The 3 pt row-end border case without a footer still differs. Tight row-end and continuation border fitting remain separate.

## Footer page tables

Page-relative footer tables keep their physical wrap bounds without adding to footer text height. Body text resumes below the table. Original native controls cover empty, text-bearing and minimum-height tables. Body textbox boundaries and nested footer painting remain separate gaps.
