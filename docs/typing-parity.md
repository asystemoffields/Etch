# Typing parity with Word

What Word does when you type, observed black-box (same keystrokes typed into Word and
Etch, results read back paragraph by paragraph) and pinned by tests in
`crates/engine/src/tests_typing.rs`. When typing behaviour changes, change it here and in the
tests together. Line breaking and pagination are in [`layout-parity.md`](layout-parity.md); the
overall picture is in [`target-app-parity.md`](target-app-parity.md).

## Sample text

Enter at the end of a paragraph containing exactly `=rand` or `=lorem` replaces
the formula with five paragraphs of three sentences and leaves the caret in a following
empty paragraph. `=rand(p)` / `=lorem(p)` set the paragraph count; `(p,s)` sets both
counts. Etch accepts integers from 1 to 200 for each count; if either parameter
is invalid or outside that range, both revert to 5/3. Other text, a selection or Enter
inside the formula follows ordinary Enter behaviour. The English text is original to
Etch; lorem ipsum uses traditional pseudo-Latin. This implements the issue's
requested defaults; the exact numerical limits have not been compared in Word.

The replacement and its paragraph breaks are one undo step. Typing and paragraph
formatting use the existing editing primitives. Tests: `sample_text_formulas_expand_on_enter`,
`sample_text_invalid_parameters_use_defaults`, `sample_text_requires_an_exact_paragraph_and_caret_at_end`,
`sample_text_preserves_typing_and_paragraph_formatting`, `sample_text_replacement_tracks_changes`.

## Lists

| Keys | Word's result | Test |
|---|---|---|
| `* item` Enter Enter `text` | the second Enter ends the list; `text` is a **Normal** paragraph (no list indent left behind) | `enter_twice_after_a_list_item_ends_the_list`, `the_paragraph_after_a_list_is_normal_text` |
| … Enter ×3, ×4 | each further Enter adds a blank Normal paragraph (never swallowed) | `more_enters_after_a_list_make_blank_paragraphs` |
| Enter on an empty **nested** item | moves it up one level; only a top-level empty item leaves the list. The same when the list comes from the paragraph style (List Number 2 in a Word file) | `enter_on_an_empty_nested_item_moves_it_up_a_level_first`, `enter_on_an_empty_item_numbered_by_its_style_moves_it_up_a_level` |
| `* ` Enter (nothing typed) | list cancelled, Normal paragraph | `enter_right_after_starting_a_list_cancels_it` |
| Tab / Shift+Tab at the start of an item | demote / promote | `shift_tab_and_tab_change_list_levels` |
| Backspace at the start of an item | 1st: number removed, indent kept; 2nd: Normal paragraph — **not** joined to the one above | `backspace_at_the_start_of_an_item_removes_the_number_then_the_indent` |
| Enter mid-item / at item start | splits the item / inserts an empty item above | `enter_in_the_middle_or_at_the_start_of_an_item` |
| Shift+Enter in an item | line break inside the same item | `shift_enter_stays_inside_the_list_item` |
| Paste: Keep Text Only `A`, empty line, `B` into an empty item | three paragraphs `A`, empty, `B`: pasted paragraph breaks are text, not Enter (they never end the list) | `plain_text_paste_keeps_an_empty_paragraph_in_a_list` |
| `* ` `- ` `1. ` `1) ` `a. ` `a) ` `A. ` `i. ` `I. ` + text | bullet, dash bullet, 1. 1) a. a) A. i. I. lists | `list_autoformat_triggers` |
| `> ` `-> ` `=> ` + text | bulleted list with an arrow bullet (➢ ➔ ⇨; Word takes them from a symbol font, we use the Unicode arrows) ¹ | `arrow_bullets` |
| `1. ` after other text | a new list restarting at 1 (never continues an earlier list) | `typing_1_dot_starts_numbering_again` |
| Heading 1 `One`, Heading 2 `Two`, Heading 1 `Three`, the caret in `Three`, Multilevel List › the "1 / 1.1 / 1.1.1" heading list, then at the end Enter, Heading 2, `Four` Enter `Body` | One = 1, Two = 1.1, Three = 2, styles kept; the new Heading 2 is numbered 2.1 by itself (the heading styles carry the list); `Body` (Normal) stays unnumbered | `a_new_heading_after_a_heading_linked_list_is_numbered` |
| Numbering on a Heading 1 | "1. One", still Heading 1 | `numbering_a_heading_keeps_the_heading_style` |
| The "1 / 1.1 / 1.1.1" heading list with a caret or a selection | paragraphs already in a Heading style keep their style and level; every other paragraph style (Normal, Quote, Title…) becomes Heading 1. One H1 / Body / Two H2 → 1, Body H1 "2", Two H2 "2.1"; Two H2 + Three H3 with no Heading 1 above → 1.1, 1.1.1; caret in a Heading 2 → stays Heading 2 "1.1"; Normal alone → Heading 1 "1" (checked in Word for Mac, 2026-10-16) | `a_heading_preset_keeps_headings_and_makes_other_paragraphs_heading_1`, `a_heading_preset_on_a_lone_normal_paragraph_makes_it_heading_1` |
| Undo after a heading list | one step: paragraph styles and the heading styles' numbers come back | `undo_takes_a_heading_preset_back_in_one_step` |
| A paragraph whose numbering was switched off, then Heading 2 from a heading-linked list | numbered (1.1) by the style | `applying_a_numbered_heading_style_numbers_the_paragraph` |
| `1. ` typed in a Heading 1 (also `a) ` in a Heading 2, `1. ` in a Subtitle) | numbered, the style kept; only body text becomes List Paragraph (checked in Word for Mac, 2026-10-15) | `typing_1_dot_in_a_heading_keeps_the_heading_style` |
| ⌘Z right after an AutoFormat | undoes only the AutoFormat (`* ` comes back as text) | `undo_right_after_autoformat_undoes_only_the_autoformat` |
| `1. Lorem` Enter `Ipsum` Enter Tab `Latin` Enter `Nonsense` Enter Enter | the empty `c.` moves up a level and numbers on: `3.` | `tests_tables_lists::enter_on_an_empty_nested_item_moves_it_up_and_numbers_on` |
| Numbering (Home) on the paragraph just after a table that follows a numbered list | goes on with that list (`3.`): a table between items doesn't end it ² | `tests_tables_lists::numbering_continues_across_a_table` |
| Copy list items, paste as plain text elsewhere | each item keeps its number or bullet and the separator after it: `1.<tab>Item 1`; an item copied from part-way through, or a paragraph only reached at its start, has none | `tests_tables_lists::copied_list_items_keep_their_numbers_in_plain_text` |
| Undo / Redo of numbering changes (an item taken out, Restart at 1, Set Numbering Value) | the labels follow exactly, as a fresh layout numbers them | `tests_tables_lists::list_numbering_after_undo`, `set_numbering_value_renumbers_from_the_item_on` |
| Continue Numbering (the item's context menu) after Restart at 1 or Set Numbering Value, on a list split by a table or a section break | the item and the items after it in its list number on from the nearest list before them with the same definition; refused when the item already continues it or no such list comes before; a table or a section break in a plain paragraph between items doesn't end the list; Undo and Redo give the labels back ² | `tests_clipboard::restart_set_value_and_continue_numbering_across_a_table_and_a_section_break` |
| Enter / Backspace / Tab / Shift+Tab on items, then Undo | as the rows above; a run of Backspaces is one undo step (Undo table), Shift+Tab and the typing before it their own | `tests_clipboard::enter_backspace_tab_on_list_items_follow_word_and_undo` |
| A list item from a web page (`<ul><li>…`) pasted on a blank line / into a paragraph with text | an item (the empty unformatted paragraph takes the pasted paragraph's formatting, as the first paragraph of a longer paste does) / the text joins the paragraph, which keeps its own formatting ¹ | `tests_clipboard::an_html_list_item_pasted_on_a_blank_line_is_a_list_item` |

## Tables

| Keys | Word's result | Test |
|---|---|---|
| Tab / Shift+Tab | the next / previous cell, passing over the cells a vertically merged cell covers (their text isn't shown, so the caret never goes there); Tab in the last cell adds a row of single cells | `tests_tables_lists::tab_and_shift_tab_skip_cells_a_merge_covers` |
| Table selected, Backspace / Delete | Backspace deletes the table, selected whole rows or columns; Delete empties them and keeps the table. Some cells of a row or column (or one cell) are emptied by either | `tests_tables_lists::backspace_deletes_a_selected_table_rows_or_columns_and_delete_empties_them` |
| Cells selected, Copy, Paste | Copy takes a table of the selected cells (the whole table, rows, or the columns of a block); pasted between paragraphs it is a table, pasted into a cell a nested table; the plain text has cells separated by tabs. Nesting stays capped at 16 tables | `tests_tables_lists::copying_cells_copies_a_table`, `copying_rows_from_inside_a_merged_cell_makes_a_valid_table`, `tests::table_nesting_is_capped` |
| Merge Cells | the rectangle of grid columns the selected cells cover, grown until every cell inside is whole (a cell index is a different column in rows split differently) | `tests_tables_lists::merge_uses_grid_columns_not_cell_indexes` |
| Insert Above / Below beside a vertically merged cell | a row inside the merged cell joins it; one above or below it stands alone; the merged cell stays whole ² | `tests_tables_lists::inserting_rows_around_and_inside_a_merged_cell_keeps_it_whole` |
| Delete Rows through the top of a vertically merged cell | the rest of it stays merged and keeps its text ² | `tests_tables_lists::deleting_the_top_row_of_a_merged_cell_keeps_the_rest_merged_with_its_text` |
| Split Cells (columns) in a vertically merged cell | every row of it splits alike, so each part stays merged down the same rows ² | `tests_tables_lists::split_a_merged_cell_into_columns_splits_every_row_of_it` |
| Split Table, Delete Cells next to a merged cell | each part keeps whole cells: no cell is left continuing a merge that doesn't start above it | `tests_tables_lists::split_table_through_a_merged_cell_gives_each_part_a_whole_cell`, `delete_cells_beside_a_merged_cell_keeps_the_merges_valid`, `the_caret_never_lands_in_a_covered_cell` |
| Sort a table with vertically merged cells | refused with a message, nothing changes ² | `tests_tables_lists::sort_refuses_a_table_with_vertically_merged_cells` |
| Convert to Text / Convert Text to Table | the text keeps its formatting (and pictures and fields); a nested table stays a table ² | `tests_tables_lists::convert_to_text_keeps_formatting_and_nested_tables`, `convert_text_to_table_keeps_formatting` |
| A table as the whole document | row, column and table deletes, Backspace and Shift+Tab at its edges keep a paragraph after it; deleting it leaves one empty paragraph | `tests_tables_lists::a_table_as_the_only_content` |
| any table and list commands, Undo it all, Redo it all | the document is exactly as it was and stays valid throughout: cells within the grid, every vertical merge starting above where it continues, list paragraphs in lists that exist, the caret never in a covered cell, list labels as a fresh layout gives them | `tests_tables_lists::random_table_and_list_commands_keep_the_document_valid_and_undo_redo_exact` |
| Ctrl+A / ⌘A in a table cell (or with cells, a row or a column selected) | the whole document (the caret's story), as Word's shortcut lists for Windows and Mac say: Ctrl/⌘+A selects all of the document, with no table special case (`docs/word-behaviour-research.md`, item 21 / checklist 17). Before, Etch stepped cell → row → table → document, which is LibreOffice's way. Table › Select Table selects the table; Word's Alt+Numpad 5 isn't bound (egui reports keypad digits like top-row ones, and Alt+digit composes character codes) | `tests::select_all_in_a_cell_selects_the_whole_document`, `select_all_from_a_column_selects_the_whole_document`, `select_all_keeps_the_whole_story_when_it_ends_in_a_table` |

² Not yet compared with Word: chosen so nothing is lost and every table stays valid.

## Undo

| Keys | Word's result | Test |
|---|---|---|
| type, select some of it, type over it | two undo steps: the first ⌘Z brings the selected text back (still selected), the second removes the typing. Uninterrupted typing stays one step | `typing_then_selecting_then_typing_are_two_undo_steps` |
| ⌘B with nothing selected, ⌘Z, type | the text isn't bold (Undo takes the waiting caret formatting back too); ⌘Y brings it back | `undoing_bold_at_the_caret_turns_bold_typing_off_again` |
| type, then Backspace ×3 (or Delete ×n) | the run of Backspaces is one undo step ("Backspace"), apart from the typing; a run of Deletes is one "Delete" step. Typing, a caret move, a selection or any other edit ends the run; deleting a selection is a step of its own. **Not yet compared with Word** (Etch's own grouping, chosen to match typing) | `backspace_and_delete_runs_are_one_undo_step_each` |
| Backspace at the start of the document or a cell, Delete at the end | nothing to delete, so no undo step (Undo takes back the edit before it, Redo stays) and the document isn't marked edited | `backspace_or_delete_with_nothing_to_delete_leaves_no_undo_step` |
| any edit, Undo it all, Redo it all | the document is exactly as it was (serialized), whatever the keys, and stays well formed throughout: random sequences of typing, Enter, Backspace, Delete, cut, paste, formatting, lists, tables, Replace All, Change Case, Track Changes, table rows and columns, Accept and Reject, and comments; also other programs' HTML, RTF and text pastes, Paste Special and Merge Formatting, the Clipboard pane, Find, Replace and Replace All with match case, whole words, wildcards and special codes, Restart / Continue / Set Numbering Value / list levels / Define New Multilevel List, footnotes and endnotes, headers, footers, text boxes, section and page breaks, Move Paragraph. Throughout, a section break is only ever in a top-level body paragraph, every list paragraph is in a list the document defines, every note reference points at a note of its kind and no two at the same note, every text box at a text box story | `tests_editing::random_edits_keep_the_document_valid_and_undo_redo_exact`, `check_structure` |
| a typing run; Replace All; a paste; Paste Special; AutoCorrect right after typing | one undo step each (the correction is a step of its own before the typing); Undo keeps working across a save, and undoing past it marks the document edited again | `tests_clipboard::undo_steps_group_typing_runs_replace_all_and_pastes`, `undo_works_across_a_save` |

## Paste

| Keys | Word's result | Test |
|---|---|---|
| Paste (⌘V) of plain text with several lines from another program, in the middle of a right-to-left paragraph | pasted as Keep Text Only pastes it: every line becomes a paragraph formatted like the paragraph it lands in (direction, style, alignment, indents, list), and so does the text after the caret. One ⌘Z takes the whole paste out ² | `plain_text_paste_gives_every_line_the_formatting_of_its_paragraph` |
| … with empty lines, or a line break at the end | each empty line is an empty paragraph with that formatting too, in a list an empty item. The text takes the formatting before the caret (bold there makes it bold) ² | `plain_text_paste_keeps_empty_lines_as_paragraphs`, `plain_text_paste_keeps_an_empty_paragraph_in_a_list` |

² From Microsoft's description of the paste options: Word offers no options for text without
formatting, and Keep Text Only gives text the style of the paragraph where it is pasted and the
formatting of the text before the cursor. Not yet compared side by side in Word.

## Track Changes

| Keys | Word's result | Test |
|---|---|---|
| Enter (tracking on) | the new paragraph mark is a tracked insertion; Reject All joins the paragraphs again, Accept All keeps them | `tracked_enter_and_backspace_track_the_paragraph_mark`, `tests::reject_all_removes_a_tracked_paragraph_break` |
| Paste of several paragraphs (tracking on) | every paragraph mark the paste adds is a tracked insertion; Reject All restores the one paragraph, Accept All keeps the pasted ones | `reject_all_takes_out_the_paragraph_breaks_a_tracked_paste_inserted` |
| Backspace at a paragraph start / Delete at a paragraph end (tracking on) | the paragraph mark is marked deleted (still shown); the caret moves before it (Backspace) or past it (Delete). Accept joins the paragraphs, Reject keeps them. A mark you inserted yourself is just removed | `tracked_enter_and_backspace_track_the_paragraph_mark`, `tests::tracked_backspace_and_delete_mark_a_paragraph_break_deleted` |
| Delete a range from the end of one paragraph into a later one (tracking on) | the paragraph marks and text in the range are tracked deletions. Accept All removes exactly those, so the paragraph after the range stays a paragraph of its own; Reject All restores everything. Typing into an empty paragraph and rejecting leaves that paragraph | `tests::accept_all_after_a_tracked_delete_from_a_paragraph_end_keeps_the_next_paragraph`, `tests::reject_all_after_typing_into_an_empty_paragraph_keeps_the_paragraph` |
| Backspace just after a section break / Delete just before it | the break goes and the two paragraphs join; the text before it takes the following section's page setup (Word's documented behaviour: a break holds the setup of the section before it); headers and footers the following section showed by linking to the previous one stay. Delete at the end of the last paragraph removes a break on its mark too, with no paragraph to join (with Track Changes on it still does nothing there). Undo brings the break back. Breaks read from a .docx and breaks inserted here alike | `backspace_and_delete_remove_a_section_break`, `delete_removes_a_section_break_on_the_last_paragraph` |
| Select All, then Delete or Backspace | the whole document goes, tables included, also when it starts with a table; one empty paragraph is left | `select_all_then_delete_removes_tables` |
| A space, punctuation or Enter after existing text AutoCorrect changes (`teh`, a lowercase sentence start) (tracking on) | the correction is tracked: the old text a tracked deletion, the new a tracked insertion; Reject All restores it. Text you just typed is corrected in place, as before | `tracked_autocorrect_of_existing_text_is_a_tracked_change` |
| Overtype (tracking on) | each typed character replaces the next character not already marked deleted | `tracked_overtype_replaces_the_next_live_characters` |
| Cut a whole paragraph, then Enter (tracking on) | the new paragraph mark is an insertion only, never also deleted because the mark it was split from is: Backspace removes it again and Reject All restores the paragraphs | `tracked_enter_in_a_paragraph_with_a_deleted_mark_inserts_a_new_mark` |
| Paste paragraphs into a paragraph whose mark is marked deleted (tracking on) | the deletion stays on the mark that ends the pasted text (the original mark); Reject All restores the paragraphs. Any edits with tracking on reject back to the original text | `tracked_paste_keeps_the_deletion_of_the_paragraph_mark_it_lands_in`, `tests_editing::random_tracked_edits_reject_back_to_the_original` |
| Table Layout › Delete Table / Rows / Columns / Cells (tracking on) | the rows are tracked row deletions (`w:trPr/w:del`), the columns' and cells' cells tracked cell deletions (`w:cellDel`), their text marked deleted; the table stays until accepted, when the rows, columns or cells go (as Word's Accept removes deleted rows); Reject All restores them. Rows and cells you inserted yourself just go. Pressed again over what is already marked deleted, nothing changes and a message says to accept or reject first. Delete Columns leaves a merged cell that also spans a kept column alone (it narrows untracked; tracked it stays as it is). A content control locked against deletion, or content locked against editing, refuses the delete with tracking on or off | `tests_revision_checks::tracked_table_delete_marks_the_text_deleted_and_reject_all_restores_it`, `tracked_row_column_and_cell_deletes_mark_their_text_deleted`, `tracked_table_delete_over_tracked_insertions_rejects_back_to_the_original`, `table_deletes_refuse_a_control_locked_against_deletion`, `tracked_delete_table_marks_its_rows_and_accept_all_removes_it`, `delete_columns_leaves_a_merged_cell_that_keeps_a_column_alone`, `tracked_row_and_column_deletes_over_tracked_changes_reject_back_to_the_original`, `tests_review::tracked_row_and_column_edits_accept_like_untracked_and_reject_back` |
| Table Layout › Insert Above / Below / Left / Right, Tab in the last cell, Insert Table, a pasted table (tracking on) | the new rows are tracked row insertions (`w:trPr/w:ins`), a new column's cells tracked cell insertions (`w:cellIns`); Reject All takes them out again (a rejected column gives the other columns back the width it took), Accept All keeps them. A new row next to a deleted or inserted cell copies that cell's revision, so it goes with its column. Saved to .docx and read back. Accept or Reject with the caret in such a row or column takes it, and the Reviewing Pane lists it | `tests_review::tracked_row_and_column_edits_accept_like_untracked_and_reject_back`, `accept_and_reject_at_the_caret_take_one_row_or_column`, `tracked_rows_and_cells_are_revisions_in_the_reviewing_pane`, `tracked_rows_survive_a_docx_round_trip` |
| A selection over whole tables deleted (tracking on) | the tables' rows are tracked deletions too, and the paragraph mark before them, so Accept All gives what the deletion gives untracked (the paragraphs around the tables join) | `tests_review::tracked_row_and_column_edits_accept_like_untracked_and_reject_back` |
| Any edits with tracking on, then Accept All / Reject All | Accept All gives what the same edits give untracked (text, formatting, lists, tables, rows and columns); Reject All gives the document back: paragraph styles, lists, direction and borders that typing, AutoFormat and pastes changed come back too, as tracked formatting changes. Not compared: the formatting of paragraph marks (see Known gaps) | `tests_editing::random_tracked_edits_accept_like_the_same_edits_untracked`, `random_tracked_edits_reject_back_to_the_original`, `tests_review::reject_all_takes_back_what_these_tracked_edits_changed` |
| Accept All / Reject All for one reviewer (`author`) | only that reviewer's changes are accepted or rejected, like Word's Accept/Reject All Changes Shown with only that reviewer shown; the others stay tracked | `tests_review::accept_all_and_reject_all_can_take_one_reviewers_changes` |
| Delete text holding a whole comment range / one end of it | the comment goes with its replies (as in Word); over one end, the comment stays on what is left. With tracking on the comment stays until the deletion is accepted. Cut and Paste moves a comment, Copy and Paste makes a second one; no comment is left with one end | `tests_review::deleting_commented_text_leaves_no_dangling_anchor`, `accepting_a_deletion_of_commented_text_deletes_the_comment`, `cut_moves_a_comment_and_copy_makes_another` |
| Backspace / Delete with a whole table selected (Select Table, or every cell dragged over) | Backspace removes the table, like Delete Table (one undo step; with tracking on its text is marked deleted and the structure stays); the caret goes to the paragraph after it. Delete clears the cells and keeps the table. A nested table goes on its own and leaves the cell's paragraph; a table that was the only block leaves one empty paragraph | `backspace_with_the_whole_table_selected_removes_the_table`, `delete_with_the_whole_table_selected_clears_the_cells_and_keeps_the_table`, `backspace_on_a_selected_nested_table_removes_only_it`, `backspace_on_a_selected_table_that_is_the_only_block_leaves_an_empty_paragraph`, `tracked_backspace_on_a_selected_table_marks_its_text_deleted_like_delete_table` |
| Backspace / Delete with a table and the paragraph before or after it selected | both remove the table, whatever end the selection starts in; the text around joins. Any table the selection covers whole goes, at any nesting level | `delete_or_backspace_with_the_table_and_the_paragraph_after_selected_removes_the_table`, `delete_or_backspace_with_the_paragraph_before_and_the_table_selected_removes_the_table` |
| Backspace / Delete with whole rows or columns selected | Backspace deletes the rows or columns (Delete Rows / Delete Columns); Delete clears their cells. One selected cell is just that cell, also in a one-row or one-column table: Backspace clears it. **Differs:** in a 1×1 table, Select Cell then Backspace removes the table (Etch has no end-of-row mark to tell the cell from the whole table; the end-of-row caret position is still missing) | `backspace_deletes_selected_rows_and_delete_clears_them`, `backspace_deletes_selected_columns_and_delete_clears_them`, `backspace_on_one_selected_cell_of_a_one_row_or_one_column_table_clears_it`; `delete_range` edge cases: `edit::tests::a_range_ending_at_the_end_of_a_cell_that_isnt_the_last_keeps_the_table` and five more (other tables, nested tables, story ends, headers and footnotes, content controls, nesting past the paragraph walk) |
| Change Case (Shift+F3, Home › Aa) (tracking on) | Word shows no insertion or deletion markup (checked with Formatting markup set to none, so Word may record it as a hidden formatting revision). **Differs:** Etch records the changed letters as a tracked deletion of the old and insertion of the new, since case isn't a run property a formatting revision could hold; Reject All restores the old case, Accept All keeps the new. Your own tracked insertion is rewritten in place; already-deleted text is left alone and doesn't count (with tracking off too), so changing case again (or Shift+F3) works from the visible text. Refused in content locked against editing | `tests_revision_checks::tracked_change_case_rejects_back_to_the_original_case`, `tracked_change_case_of_own_insertion_stays_an_insertion`, `tracked_change_case_twice_reads_only_the_live_text`, `tracked_change_case_handles_sharp_s_and_mixed_formatting`, `change_case_refuses_content_locked_against_editing` |
| Unlink Fields (Ctrl+Shift+F9) (tracking on) | not tracked, like Word: the field's result stays as ordinary text. Refused in content locked against editing | `tests_revision_checks::tracked_unlink_keeps_the_result_as_plain_text`, `unlink_refuses_a_field_in_content_locked_against_editing` |
| Insert Footnote / Endnote, a picture, a shape or a text box (tracking on) | a tracked insertion, as typed text is: Reject All takes the reference and its note, or the object (and a box's text), out again; Accept All keeps them with no marks left. Before, these were inserted untracked and stayed after Reject All (found by `random_tracked_edits_reject_back_to_the_original`) | `tests_clipboard::a_footnote_inserted_under_track_changes_is_a_tracked_insertion`, `objects_inserted_under_track_changes_are_tracked_insertions` |
| Replace All (tracking on) | every match a tracked deletion and insertion in one undo step; Reject All gives the text back, Accept All the replaced text | `tests_clipboard::find_and_replace_options_special_characters_and_stories` |
| Replace All, from the body | every story, as Word's dialog with Search: All: the body, headers and footers, footnotes and endnotes, text boxes and comments, but not a text box inside a header or footer (https://wordmvp.com/FAQs/Customization/ReplaceAnywhere.htm ; https://wordmvp.com/FAQs/MacrosVBA/FindReplaceAllWithVBA.htm); one undo step, tracked under Track Changes in every story, the caret stays in its story. `edit.replaceAll {"in": "story"}` keeps to the caret's story, as a recorded macro does (`docs/word-behaviour-research.md`) | `tests_word_parity::replace_all_covers_every_story_but_text_boxes_in_headers` |
| Inserted and deleted cells and rows (tracking on), All Markup | the cells are filled in Word's default colours: inserted light blue, deleted (and every cell of a deleted row) pink, under their text, in the window, page images and Print Markup PDFs; on the dark page the fills are dimmed like other backgrounds; No Markup shows plain cells. Track Changes Options › Table cell highlighting takes other colours (`review.trackingOptions {"cellInsertColor", "cellDeleteColor"}`, `auto` for the defaults). Merged and split cells have no colour: neither Word nor Etch records them (https://support.microsoft.com/en-au/office/advanced-track-changes-options-in-word-2580cd04-1ea2-461b-98b3-c2f9c365f1f2) | `tests_word_parity::tracked_cells_are_highlighted_in_the_markup_views`; `wordcraft-render tests::tracked_cells_are_highlighted_in_markup_light_and_dark` |
| Merge Cells / Split Cells (tracking on) | not tracked, as in Word, which warns first that the action won't be marked as a change (https://learn.microsoft.com/en-us/answers/a/10147163 ; https://www.ericwhite.com/blog/?p=4019): the window asks (OK / Cancel, `ui.untrackedAction`) before merging or splitting, and the engine's result says `"untracked": true`; `confirmed: true` skips the prompt. Column insertions and deletions stay tracked as `w:cellIns`/`w:cellDel` (LibreOffice writes and Word reads them, `docs/word-behaviour-research.md` Resolution) | `tests_word_parity::tracked_cells_are_highlighted_in_the_markup_views`; `wordcraft-ui-egui tests_untracked_action::merge_and_split_with_tracking_on_ask_first_and_stay_untracked` |

## Editing keys

Word's documented keyboard behaviour (Microsoft's shortcut list, feature names only); the
Windows/Linux keys are given first, macOS second.

| Keys | Word's result | Test |
|---|---|---|
| select text right to left (from its end), type | the new text takes the first selected character's formatting (font, size…), as with a selection made left to right; not the text's before it | `typing_over_backward_selection_keeps_its_font` |
| Down / Up (↓ / ↑) from a line beside a table | into the table's nearest row: the cell under the caret, or the nearest one when the caret is left or right of the cells' text (the start of the line above a table is left of the first cell's text); never past the table to the paragraph beyond it | `down_and_up_from_a_line_start_beside_a_table_go_into_it` |
| Alt+Shift+Up / Down (⌃⇧↑ / ⌃⇧↓) | the paragraphs the selection touches swap with the paragraph above / below, keeping style and list membership; the selection moves with them; one undo step. A selection ending at the start of a paragraph doesn't take it along. Refused at the top or bottom of the story or cell, next to a table and across a section break | `alt_shift_down_then_up_moves_a_paragraph_and_back_and_undoes` |
| Alt+Shift+Left / Right (⌃⇧← / ⌃⇧→) | Promote / Demote: a heading goes up or down one level (Heading 1 and Heading 9 are the ends); promoted body text becomes a heading at the level of the heading before it; a list item changes level like Shift+Tab / Tab | `alt_shift_left_and_right_promote_and_demote_headings_and_list_items` |
| Outline view: Alt+Shift+Up / Down, Left / Right | as above, but a collapsed heading takes what is hidden under it along: moving passes whole shown paragraphs with their hidden content; promote/demote shifts the hidden subheadings too | `outline_collapse_hides_descendants_and_expand_restores` |
| Outline view: Alt+Shift+Plus / Minus, Alt+Shift+1–9, Alt+Shift+A | Expand / Collapse the heading (one level at a time); Show Level 1–9; show all levels. The caret never stays in hidden text: it moves on to the next shown paragraph | `outline_show_level_filters_paragraphs` |
| Ctrl+Left / Right (⌥← / ⌥→); Home / End (⌘← / ⌘→) | one word left / right; the start / end of the line. On macOS ⌘← / ⌘→ go to the line's ends, as in every Mac text field, and ⌥ moves by word; Shift extends the selection | `word_default_shortcuts_resolve` |
| Shift+F5 | Go Back: the caret cycles through the last four places edited, most recent first | `shift_f5_goes_back_through_the_last_edits` |
| Alt+X (no macOS default) | the hex code before the caret (up to six digits, `U+` optional) or the selected code becomes its character; otherwise the character before the caret becomes its code (`é` ↔ `00E9`); one undo step | `alt_x_toggles_between_a_character_and_its_code` |
| Shift+Backspace / Shift+Delete | Shift+Backspace erases like Backspace. Shift+Delete cuts the selection to the clipboard and does nothing without one (Word for Windows, observed 2026-10-14). Windows delivers Shift+Delete as a Cut; macOS and Linux deliver the key, which did nothing | `shift_delete_cuts_the_selection_and_shift_backspace_erases` |
| Alt+numpad code (Windows and Linux; not macOS) | hold Alt, type a decimal code, release Alt: the character appears at the caret as typed text (one undo step). With a leading 0 the code is the ANSI code page — Windows-1252 by default (`0233` → é, `0128` → €); without one, 1–255 is the OEM code page — 437 by default and in US English (`233` → Θ, `1` → ☺, `7` → •), 850 for Western European locales and English outside the US (`155` → ø; `233` is Ú there, per the CP850 mapping file — é is `130` in both). On Windows the system locale picks both pages (`text.typeCode` `ansi`/`oem`): Central European 1250/852 (Polish `0185` and `165` → ą), Cyrillic 1251/866, Greek 1253/737, Turkish 1254/857, and for codes up to 255 the character Windows composes itself replaces ours (it knows every code page, double-byte ones too); Linux keeps 1252 and 437/850. Above 255, with or without the 0, the Unicode character with that decimal value (`8364` → €). Control characters, 0, unassigned bytes and values past Unicode type nothing; auto-repeat of a held digit adds nothing, and switching windows drops a half-typed code. Word counts only keypad digits; egui reports keypad and top-row digits alike, so top-row digits with Alt alone compose codes too. An Alt code never opens KeyTips | `alt_numpad_codes_map_like_word_on_windows`, `alt_numpad_code_types_one_character`, ui-egui `alt_numpad_code_types_its_character_without_keytips` |
| Backspace at the start of a paragraph after an empty one; Delete in an empty paragraph before another | the empty paragraph goes and the other keeps its own formatting (an empty Heading 1 above body text leaves body text), as Accept does with that mark deleted under tracking. Two paragraphs with text join and keep the first one's formatting. Not yet keystroke-compared with Word | `joining_after_an_empty_paragraph_keeps_this_paragraphs_formatting` |
| Backspace / Delete over a paragraph mark inside a content control locked against editing | refused, like any other deletion there | `joining_paragraphs_inside_locked_content_is_refused` |
| Backspace after a section break, Delete before one, or deleting a selection over one or more | the break goes: its text joins the section after it, which keeps its own page setup (Word: the combined section takes the formatting of the section that followed the break); the headers and footers that section linked to (showed from) a deleted section stay. Delete at the end of a last paragraph that ends a section (a file can put a break there) removes that break. Undo brings the breaks back (the joined paragraph kept the first section's break, so it only moved down) | `tests_styles::deleting_a_section_break_joins_the_section_after_it`, `…_keeps_the_headers_the_next_section_showed`, `…delete_at_the_end_removes_a_last_paragraph_section_break` |
| Paste text in a style the document doesn't have (deleted since the copy, or from another document) | the style comes with it, with the styles it is based on, linked to or followed by; a style of the same id the document has keeps its own definition | `tests_styles::pasted_text_brings_the_styles_it_uses` |
| Enter at the end of a paragraph whose style's next style doesn't exist (a file can name one) | the new paragraph keeps the paragraph's style | `tests_styles::enter_keeps_the_style_when_the_next_style_is_missing` |
| Bold (or any font format) with nothing selected, then Select All, Find or Select Word | the waiting formatting is dropped, as when the caret moves: typing over the selection or Replace takes the replaced text's formatting | `formatting_waiting_at_the_caret_is_dropped_by_a_new_selection` |
| Format Painter | copies formatting only: never the source's hyperlink or its tracked insertion or deletion; the target keeps its own link and tracked changes | `tests_editing::format_painter_copies_formatting_not_revisions_or_links` |
| Shift+Right (⇧→) at the end of a cell's text | the whole cell is selected first (its end-of-cell mark); the next Shift+Right selects the next cell with it. In an empty cell the first press selects the cell | `shift_right_at_the_end_of_a_cell_selects_the_cell_first` |
| Whole paragraphs selected (from the first one's start to the start of the paragraph after them), then Delete, Backspace or Cut | the paragraphs go with their marks; the paragraph after keeps its own formatting (three list items deleted don't make it an item, bold text in it stays bold). A deletion from inside the first paragraph joins as Backspace does, keeping its formatting. Not yet compared: a deletion from a paragraph's start that ends inside a later paragraph (it keeps the first paragraph's formatting here) | `tests_clipboard::deleting_whole_paragraphs_leaves_the_next_paragraphs_formatting_alone` |
| Copy text holding a footnote reference, paste it (in this document, or into another through its own copy) | the pasted reference is a footnote of its own with the same text, as in Word, never a second reference to the same note; editing one leaves the other; a cut pasted back keeps its note; saved to .docx each note is written | `tests_clipboard::pasting_a_copied_footnote_makes_a_note_of_its_own` |
| Copy or cut in one window, paste in another (the system clipboard's text and HTML) | the text, bold, the list with its numbers or bullets, the picture and the table arrive and survive a save; each window's Undo takes back its half. Not footnotes: the HTML writer drops them (ignored test `copy_to_another_document_keeps_footnotes`) | `tests_clipboard::copy_to_another_document_keeps_text_formatting_lists_pictures_and_tables`, `cut_and_paste_between_two_documents_both_ways` |
| Paste a copied range holding a section break into a table cell, a header, a footnote or a text box | the break goes and the paragraphs stay (a break can't live there); pasted in the body it stays a section break ¹ | `tests_clipboard::pasting_a_section_break_into_a_cell_header_or_note_drops_it` |
| Paste (Keep Text Only, or another program's HTML) over a selection spanning cells, rows, paragraphs, into or out of a table, or over a field | the table stays valid, the document well formed, and Undo gives everything back | `tests_clipboard::paste_over_selections_spanning_cells_paragraphs_and_fields_stays_valid` |
| Find & Replace: match case, whole words, wildcards (`c?t`, `^&`), `^p`, `^t`, `^l`, `^m`, a match across a paragraph mark, in a table, in a note (with the caret there) | as Word's dialog: options stay set between searches, so a search names its own; `^t` → `^p` splits the paragraph, a match over a mark joins them; the caret's story is searched (body with its tables, or the note) | `tests_clipboard::find_and_replace_options_special_characters_and_stories` |

## AutoCorrect and AutoFormat as you type

The `-->` replacement produces `→` when completed with a space, including
at paragraph start without creating a list. It respects AutoCorrect preferences, user
entries and undo. The existing `-> ` paragraph-start arrow bullet is unchanged.

Applied when a word is finished by a space, punctuation **or Enter** (`enter_finishes_a_word_for_autocorrect`):

- First letter of a sentence (paragraph or list item start, after `. ! ?`) is capitalised; not
  after abbreviations (`e.g.`, `Dr.`, …) or initials (`J.`), and not for mixed-case words
  (`iPhone`) or single-letter labels (`a.`). `teh`→`the`, `i`→`I`, `(c)`→`©`, `(tm)`→`™`,
  `wait...`→`wait…`, smart quotes (`autocorrect_as_you_type`); a quote opens after a space,
  an opening bracket, a dash, `«` `‹` `„` `‚`, or the other kind of opening quote, so nested
  quotes curl correctly (`smart_quotes_nested_and_opening_delimiters`).
  Smart quotes in Hebrew (and other right-to-left text) are chosen from the characters around
  the quote in logical order, whatever the display direction: `"` after a space or at the start
  is “ (U+201C) even though it shows on the right of the word, after a letter ” (U+201D); in an
  acronym (`צה"ל`) it becomes ” as in Word for Mac 16.113.4, not gershayim (״); `'` after a letter
  (`ג'ירפה`) is ’ as after any letter, with no geresh conversion (not yet checked in Word; `smart_quotes_in_hebrew_use_logical_order`).
- Correct TWo INitial CApitals: a finished word of three or more letters with two capitals and
  the rest lowercase loses its second capital (`WOrd`→`Word`, `TEh`→`The`); not the exceptions
  (`IDs`, `CDs`, `PCs`, `TVs`, … plus the user's, `tools.autocorrect {"addInitialCapsException"}`)
  (`two_initial_capitals_are_corrected`).
- Reversing an automatic capital makes an exception (Word's "Automatically add words to list",
  observed on the Mac): Undo of the sentence capital's own step adds the word before it
  to the First Letter list (`there.`; only words ending in a full stop, the one form seen in
  Word), and so does Backspacing over the capitalised word and typing it again; the word is then
  not capitalised after `there.` any more. Any other Undo (of later typing, of a paste) adds
  nothing, and the capital is forgotten once another Undo runs, a new document is opened, or the
  undo history is cleared. Each list has its own switch
  (`tools.autocorrect {"autoAddFirstLetter", "autoAddInitialCaps"}`, on by default); words over
  255 characters or with spaces are never added. Undoing a replacement (`teh`→`the`) adds
  nothing: Word doesn't either. Undoing a TWo INitial CApitals fix adds the word to the INitial
  CAps list (as in Word, checked on the Mac: `TWo ` typed again isn't corrected; same switch rule as First Letter). The first word of a
  paragraph has no word before it, so nothing is added there; Word also left the retyped first
  word ("hello") lowercase in that paragraph, which Etch doesn't (paragraph-level
  suppression, only for the paragraph where the Undo happened: Word capitalises the first word
  of a new paragraph again; a separate follow-up). Word adds nothing in bulleted-list paragraphs; not covered
  (`undoing_a_sentence_capital_adds_a_first_letter_exception`,
  `backspacing_a_sentence_capital_and_retyping_adds_a_first_letter_exception`,
  `automatic_exceptions_can_be_turned_off`, `undoing_a_replacement_adds_no_exception`,
  `undoing_an_initial_caps_fix_adds_an_initial_caps_exception`,
  `undoing_other_edits_after_a_capital_adds_no_exception`,
  `a_full_undo_history_does_not_match_later_undos`, `a_new_document_forgets_the_capital`,
  `an_overlong_word_is_not_added_as_an_exception`).
- Correct accidental usage of cAPS LOCK: `wORD`→`Word` (three or more letters; not `iOS`,
  `mRNA`, …). Word also switches Caps Lock off; we only correct the word
  (`accidental_caps_lock_is_corrected`).
- Capitalize first letter of table cells: the first word typed in a cell is capitalised like a
  sentence start, under its own option (`first_letter_of_a_table_cell_is_capitalised`).
- Each fix is one undo step right after it, and each has its own switch (`twoInitialCaps`,
  `capsLock`, `capCells` on `tools.autocorrect`; on by default)
  (`capitalisation_fixes_undo_and_turn_off`).
- `1/2 1/4 3/4` → `½ ¼ ¾`; `1st 22nd 13th` get superscript suffixes (`ordinals_become_superscript`).
- `word -- word` → en dash, `word - word` → en dash, `word--word` → em dash (`dashes`).
- Web addresses become links; following text isn't linked (`web_addresses_become_links`).
- A hyperlink covers only its own text: typing at either end of it, or after Enter at its end,
  isn't linked; typing inside it is. Linking a whole paragraph doesn't link its mark, so the
  paragraphs Enter makes from it aren't links (`a_link_covers_only_its_own_text`,
  `typing_at_either_end_of_a_link_is_not_linked`).
- A field covers only its own result: text typed or pasted with the caret just after a field
  (a Zotero citation at the end of a paragraph, the caret put there with a click) goes after
  the field, not into its result (`typing_after_a_citation_goes_outside_its_field`).
- Enter after `---` `===` `***` `___` `~~~` `###` → a bottom border on the paragraph above
  (single, double, dotted, thick, wave, triple) (`border_line_autoformat`). ⌘Z right after brings the
  characters back and removes the border; the Enter stays (one undo step) ¹
  (`undo_right_after_a_border_line_brings_the_characters_back`).
- `+---+------+` Enter (plus signs around runs of hyphens, the whole paragraph) → a one-row
  table across the text width, each column as wide as its share of the hyphens; the caret goes
  into the first cell. ⌘Z right after brings the characters back. More than 63 columns stay
  text ¹ (`plus_and_hyphen_line_becomes_a_table`).
- Smileys `:)` `:-)` `:(` `:-(` `:|` → ☺ ☹ 😐, also straight after a word (`ok:)` → `ok☺`) ¹
  (`smileys_are_replaced_inside_words_too`).

- Turkish, Azerbaijani and Crimean Tatar text (the word's language, else the editing language)
  maps `i` → `İ` and `ı` → `I` when a sentence or day is capitalised, and `I` → `ı` when
  TWo INitial CApitals / cAPS LOCK lowercase a letter (`istanbul` → `İstanbul`, `İNgilizce` →
  `İngilizce`, `ılık` → `Ilık`; observed in Word for Mac with Turkish proofing). Lowercasing `İ`
  gives the single letter `i` in every language, never `i` + combining dot. Other languages keep
  the Unicode default (`istanbul` → `Istanbul` in English)
  (`autocorrect_capitalisation_follows_turkic_case_mapping`). Format > Change Case is not yet
  Turkish-aware.
- Names of days are capitalised (`monday` → `Monday`). AutoCorrect Options (`tools.autocorrect`, saved per user) switch each of these off: replacements, sentence and
  table-cell capitals, days, smart quotes, fractions, ordinals, dashes, links, bulleted and
  numbered lists, border lines; user entries are added, replaced or deleted (built-in ones too),
  and "don't capitalize after" exceptions extend the abbreviation list. A replacement of several
  words takes a sentence capital on its first (`autocorrect_options_change_what_typing_does`).
- The built-in replacement list is English: its words (`teh`, `i`, `dont`…) are replaced only
  in English text (the run's `w:lang`, then its style, then the document default; text with no
  language at all is in the editing language, so with a Polish interface a `.docx` without
  `w:lang` is Polish). English is decided as proofing decides it (`ProofLang`: `en`, `eng`).
  Polish `Ala i kot` keeps its `i` ("and"); symbols (`(c)`, `->`, `1/2`) and the user's own
  entries apply in every language (`autocorrect_replacement_list_follows_the_texts_language`).
  Croatian, Bosnian and Serbian `i` is kept only when the text is tagged with that language: those
  interfaces don't set an editing language yet. Per-language lists for other languages aren't
  there yet.

¹ From Word's documented AutoFormat and AutoCorrect options; not yet keystroke-compared with
Word.
## Spelling as you type

Word draws no red squiggle under the word being typed. The squiggle comes when the word ends
(space, punctuation, Enter, Tab) or when the caret leaves it any other way (arrow keys, Home,
End, a click, a selection). Only the word being typed is exempt: clicking into a misspelled word
that is already there does not hide its squiggle, and typing into it makes it the word being
typed again. F7 and the Spelling pane report the word all the while. Grammar marks are not held
back.

| Keys | Result | Test |
|---|---|---|
| `The helo` | no squiggle under `helo`; `review.issues` and F7 (`review.spelling`) still list it | `the_word_being_typed_is_not_marked_until_it_ends` |
| `The helo` then space, `.`, Enter or Tab | `helo` underlined | same |
| `The helo` then Left, Home, Ctrl+Left, Ctrl+Home or Select All; End after that | `helo` underlined, and still underlined when the caret comes back | `moving_the_caret_out_of_the_typed_word_marks_it` |
| `The helo` in one paragraph, a click into another, a click back, `o` typed into it | underlined on leaving; still underlined on return ¹; typing into it hides the mark again until the next space ¹ | `a_click_elsewhere_marks_the_typed_word_and_a_click_back_keeps_the_mark` |
| `The helo ` then Backspace | the mark goes while the word is edited again ¹ | `backspace_into_a_finished_word_edits_it_again` |
| `I typed teh ` (AutoCorrect); a phrase by one `text.insert`; Undo | AutoCorrect's result is checked as usual; of inserted text only the word the caret ends in is exempt; Undo moves the caret, so what is left is marked | `autocorrect_and_inserted_text_are_marked_as_usual` |
| `It doesn'` (smart quotes make it `doesn’`); `The nonn-` then `exist` | no squiggle until the word ends: the apostrophe and the hyphen stay part of the word, as in the spelling check ¹ | `apostrophes_and_hyphens_stay_part_of_the_word_being_typed` |
| A click at the end of `helo`, then Insert, Delete with nothing after the caret, or Shift+Tab | `helo` stays underlined: only an edit to the text makes it the word being typed ¹ | `text_commands_that_edit_nothing_leave_an_existing_word_marked` |

¹ Follows the rule above; not yet keystroke-compared with Word.

The exemption is a layout input (`LayoutOptions::typing_word`, filtered in
`crates/layout/src/para.rs`). The Session records the word after a `text.*` command that changes
the caret's paragraph, and drops it once the caret is anywhere else or the word changed under it
(`crates/engine/src/session.rs`), so a caret move without an edit lays only that paragraph out
again.

## Editing options (File › Options › Advanced)

| Keys / setting | Result | Test |
|---|---|---|
| Typing over a selection | replaces it (Word's default); with "Typing replaces selected text" off, the text goes in front of the selection, which stays | `typing_over_a_selection_follows_the_replace_selection_option` |
| Overtype on, typing | each character replaces the one after the caret, never the paragraph mark or an object; Insert switches overtype only with the Insert-key option on (off by default, as in Word) | `overtype_replaces_the_characters_after_the_caret` |

## Text direction

- Typing Arabic, Hebrew or another right-to-left script as the first letters of a paragraph
  whose direction nothing sets (not the paragraph, its style or the document defaults) makes it
  a right-to-left paragraph, so the caret starts at the right margin. A paragraph that already
  has letters keeps its direction, and so do opened documents: a paragraph without `w:bidi` stays
  left-to-right whatever its text (`typing_rtl_letters_into_an_empty_paragraph_makes_it_rtl`).
  Word takes the direction from the keyboard language instead; not yet compared side by side.
- A table inserted in a right-to-left paragraph is a right-to-left table (its first cell at the
  right), as in Word, so Tab moves to the cell on the left (the next one in reading order) and
  Shift+Tab back; in a left-to-right table Tab still moves right (`tab_in_a_table_inserted_in_right_to_left_text_moves_left`).
- Text typed or inserted (links, fields, symbols, pasted text with merged formatting) takes its
  direction from its own letters, not from the run around the caret, keeping only that run's
  bidi language: Latin typed into Hebrew from a reopened file is saved as left-to-right text, and
  the saved file marks the right-to-left pieces `w:rtl` itself (`latin_typed_into_reopened_hebrew_is_saved_left_to_right`). Known display difference: spaces,
  digits and punctuation typed into reopened Hebrew text draw in the Latin font, not the
  complex-script font, until the file is saved and reopened.

## Layout

- No line break right after a slash (`and/or`, URLs) unless the word can't fit a line
  (`layout::tests::no_line_break_right_after_a_slash`). A slash that opens a word after a
  space (`Kazakh /Russian` captions) travels with it: the break is before the slash, so
  `/заполняется` moves to the next line whole and is never split between its letters (`layout::tests::a_word_after_a_slash_wraps_whole`, also in a fixed table cell and not after a no-break space).

## Complex scripts

- Myanmar, Khmer and the Indic scripts are typed in memory order (consonant, medials, then
  vowel signs such as ေ, which is drawn before its consonant); the caret steps over whole
  syllables (Myanmar vowel signs and stacked consonants never split off), Backspace removes one
  character (vowel sign, medial, stacked consonant), Delete the whole syllable — the platforms'
  standard behaviour, not yet compared against Word (`myanmar_typing_steps_by_syllable_and_backspaces_by_sign`,
  `indic_and_khmer_backspace_removes_one_character`).

## Date and time

Insert › Date & Time, the DATE, TIME, CREATEDATE, SAVEDATE and PRINTDATE fields, a cover page's
date and the date picker's Today show local civil time: at noon on 31 October in Auckland
(UTC+13) the date inserted is 31 October, not the UTC date (the 30th). A recorded instant
(CREATEDATE, SAVEDATE) is shown with the offset in force at that instant, so a document created in
January and shown in July shows its winter time. What is written into files stays UTC with `Z`
(document properties, tracked changes, comments). The zone comes from the system: `TZ` or
`/etc/localtime` (TZif files and POSIX TZ rules, read by our own reader, `crates/tz`) on Linux,
macOS and the BSDs, the browser's on the web, Windows' own on Windows. Pinned by
`tests_fields::insert_date_and_time_is_the_local_date`,
`tests_fields::created_and_saved_dates_are_local_with_the_offset_of_their_instant` and
`tests_fields::date_fields_opened_and_nested_are_local`, with fixed zones.

## Input methods

- An input method can ask to erase characters before and after the caret (`DeleteSurrounding`)
  and send replacement text. The web build's hidden text field does this whenever its contents
  change other than by adding to the end: Backspace and corrections from phone keyboards, and
  the Chrome caret jump below. The desktop builds never receive it (egui-winit 0.36 doesn't
  send it). Etch erases those characters as Backspace erases a selection, stopping at the
  paragraph's ends, then types the new text. The new text keeps the erased text's formatting
  and never overtypes what follows. Erasing and retyping stay in the open typing undo step, and
  typing after them joins it too (`keys::tests::delete_surrounding_from_the_web_text_agent_does_not_repeat_text`,
  `delete_surrounding_erases_characters_on_both_sides_of_the_caret`,
  `delete_surrounding_stays_inside_the_paragraph`, `delete_surrounding_joins_the_typing_undo_step`,
  `delete_surrounding_keeps_the_replaced_texts_formatting`,
  `delete_surrounding_text_does_not_overtype`, `delete_surrounding_rewrite_ends_at_a_key_press`).
- After AutoCorrect or AutoFormat changes the text as you type, the web build tells its hidden
  text field to start afresh. Its later counts then can't reach past the change
  (`automatic_change_asks_the_input_method_to_start_afresh`). The reset itself happens in eframe
  and hasn't been checked in a browser.

## Fields, printing and section breaks

Where `docs/word-behaviour-research.md` found Word's behaviour from public sources (its item
numbers in brackets), pinned by `crates/engine/src/tests_word_parity.rs`:

| Action | Word (source) | Test |
|---|---|---|
| Insert › Quick Parts › Field (`insert.field`) | computes the new field only: a SEQ takes the number it has in the sequence (the other SEQ fields keep theirs), a PAGEREF its bookmark's page, a date today; the table of contents and the other fields change only on Update Fields, which then settles the document (https://support.microsoft.com/en-us/word/insert-edit-and-view-fields-in-word ; https://support.microsoft.com/en-us/word/update-fields) | `insert_field_computes_only_the_new_field` |
| SEQ fields in a text box | count in the body's sequence, at the place the box is anchored; SEQ fields in headers, footers and notes don't affect the body's numbering and are left alone (https://support.microsoft.com/en-us/office/field-codes-seq-sequence-field-062a387b-dfc9-4ef8-8235-29ee113d59be ; https://learn.microsoft.com/en-us/answers/a/11464150) | `seq_fields_in_text_boxes_count_at_their_anchor` |
| PRINTDATE | never printed: the all-zero date (`0/0/0000`, `0:00:00 AM` with a time picture), not today (ECMA-376 §17.16.5.49). File › Print and Print to PDF record the print in the document's properties (`cp:lastPrinted`) and the field shows it, on the printed pages too; Export and Save As PDF don't print (https://office-watch.com/2018/other-word-document-date-and-time-options/ ; https://reference.aspose.com/words/net/aspose.words.fields/fieldprintdate) | `printdate_is_the_zero_date_until_a_print_records_it` |
| Table of figures page numbers | in each caption's section's page-number format (roman front matter stays roman), as the table of contents does (medium; https://www.k-state.edu/grad/academics/etdr/template/word/word13/pagenumbers.html). The index keeps decimal pages: nothing found for Word | `table_of_figures_pages_follow_the_section_number_format` |
| Print hidden text (File › Options › Display, off) | decides whether hidden text prints and goes into PDFs and page images; the ¶ display and the formatting-marks option never do (https://support.microsoft.com/en-US/Word/word-options-display). `file.options {"printHiddenText": bool}` | `print_hidden_text_decides_prints_and_pdfs_not_the_screen` |
| Next Page break at the end of a numbered item | the item's own paragraph ends the section (its `w:pPr/w:sectPr`, ECMA-376 §17.6.17); no empty item is made to hold the break. The new paragraph after the break is an empty item (low confidence for Word; pinned as it is) | `a_section_break_at_the_end_of_a_list_item_is_held_by_the_item` |
| Fields under formatting-only protection | insert and update (their results' direct formatting with them); direct formatting is refused (https://support.microsoft.com/en-US/Word/restrict-or-permit-formatting-changes) | `formatting_only_protection_lets_fields_insert_and_update` |

## Known gaps

- Web: Chrome has been seen moving the caret of eframe's hidden text field to its
  start after the first non-Latin character. Later letters then go in before what was typed
  since the field was last cleared, so `hello` then `мир` gives `риhelloм`. The text is no
  longer repeated, but the cause of the caret jump hasn't been found and isn't worked around.
- Word's body font Aptos isn't installed outside Office; our substitute (Carlito, else Helvetica
  Neue) is wider, so lines wrap earlier than in Word. Needs a metric-matched open substitute.
- Reject All doesn't always give a paragraph mark its formatting back: emptying a paragraph
  gives its mark the formatting of the text deleted, and Enter leaves the mark's formatting on
  the first part while its revisions go with the second, neither a tracked change. Only the
  height of an empty line and what is typed into it show it.
- Merging and splitting cells, Convert Text to Table / Table to Text and Sort aren't tracked
  (not compared with Word).
- Moving paragraphs with Track Changes on isn't recorded as a tracked move; whole table rows
  don't move yet (a selection across cells is refused). Go Back's places aren't mapped through
  later edits (they are clamped to the document).
- Not yet compared: keystroke-by-keystroke comparison of Correct TWo INitial CApitals, cAPS LOCK
  and table-cell capitals (implemented from Word's AutoCorrect options; Word's exception list
  not compared); the rows marked ¹ above; how Word groups runs of Backspace and Delete into undo
  steps (Undo table).
- Footnotes don't cross the clipboard to another window: the HTML a copy puts beside its text
  drops notes (and fields and text boxes), so a note copied into another document is lost.
- Plain text from another program pastes as unformatted paragraphs after the first line; in Word
  each line splits the paragraph as Enter does (the ignored test
  `plain_text_pasted_from_another_program_keeps_the_paragraphs_formatting` pins that).
- Ctrl+Home, Ctrl+End and Select All with the caret in a note stay in the note (Select All:
  medium; Home/End: unobserved). The empty numbered item after a section break at the end of a
  list item is Word's probable behaviour, not observed. How Word itself shows `w:cellIns` /
  `w:cellDel` written by another program is inferred from LibreOffice's interoperability work.
- A page break at the end of a paragraph that ends a Next Page section: Etch keeps the
  paragraph mark on the break's page, so "A", Ctrl+Enter, a Next Page break, "B" is two pages;
  `docs/word-behaviour-research.md` item 27 expects a blank page 2 (three pages) at low
  confidence. Left as it is until observed.

## How to compare

- Etch: the headless `ui_shot` example (no window, no focus) with `UI_SHOT_FULL=1` prints
  the whole `document.inspect`; drive it with `ui.text` / `ui.key` lines so keys take the real
  input path.
- Word: prefer scripting that doesn't take focus (open, read paragraphs, export PDF). Keystroke
  runs steal focus and race the person at the keyboard: keep them short, read the document back
  to verify, and ask first. Screenshots of Word stay local and are never committed.
