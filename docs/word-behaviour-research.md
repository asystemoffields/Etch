# Word behaviour research

Word's behaviour on questions Etch needed answered, researched from public sources: Microsoft
Support and Learn articles, the ECMA-376 spec and [MS-OI29500] implementation notes, Word MVP and
reputable tutorial sites, and forum answers. Everything is paraphrased; no Microsoft code, assets or
long text were copied. Code comments cite the items by number.

**Confidence:** **high** means a Microsoft document or the spec says it. **Medium** means several
consistent reputable secondary sources. **Low** means one forum post or an inference.
**Unresolved** means nothing usable was found; those questions have manual test steps below.

## Answers in detail

### Track changes, protection and undo

**1. Deleting across paragraphs (low).** ECMA-376 says a deleted paragraph mark merges that
paragraph's content into the following paragraph. So when a tracked deletion that starts at the
very start of paragraph 1 is accepted, the last paragraph's mark and formatting should survive,
which agrees with Etch. For an untracked delete, an older Word book says the merged paragraph takes
the deleted mark's formatting. But Word's smart paragraph selection may behave differently when the
whole first paragraph is selected, and current Word was not confirmed.
Sources: https://learn.microsoft.com/lb-lu/dotnet/api/documentformat.openxml.wordprocessing.deleted?view=openxml-2.13.1 ;
https://www.oreilly.com/library/view/special-edition-using/0789725134/0789725134_ch15lev1sec6.html ;
https://www.libreoffice.org/bugzilla/show_bug.cgi?id=125311

**2. Table columns and rows under tracking (medium; highlighting high).** Deleting a row is
tracked. Deleting a column is not: Word shows a warning that the action won't be marked as a change,
and Microsoft Q&A answers call this a known Microsoft 365 limitation. No source showed column
insertion tracked or a recent change to this. The Advanced Track Changes Options dialog has table
cell highlighting colours for inserted, deleted, merged and split cells. Etch tracks columns, which
is more than Word does, and doesn't highlight cells.
Sources: https://support.microsoft.com/en-au/office/advanced-track-changes-options-in-word-2580cd04-1ea2-461b-98b3-c2f9c365f1f2 ;
https://learn.microsoft.com/en-us/answers/questions/1161757/can-t-keep-track-of-changes-done-to-a-table-in-wor ;
https://bz.apache.org/ooo/show_bug.cgi?id=76935

**3. Merge/split, Convert, Sort under tracking (medium for merge and split).** Merging and
splitting cells are not tracked, and Word warns before doing them (a Microsoft Word 2007 note and
later Q&A threads). Nothing specific was found for Convert Text to Table, Convert Table to Text or
Sort.
Sources: https://learn.microsoft.com/en-us/answers/a/10147163 ;
https://learn.microsoft.com/en-us/archive/blogs/fixit4me/fix-revision-tracking-for-word-tables ;
https://www.ericwhite.com/blog/?p=4019

**4. Track Changes under tracked-changes protection (medium).** When this protection is enforced,
Track Changes is locked on and the user can't turn it off; guides describe the button as greyed out.
Sources: https://www.journalofaccountancy.com/issues/2008/aug/putalockontrackchanges/ ;
https://ionos.co.uk/digitalguide/online-marketing/online-sales/word-track-changes

**5. Formatting-only protection (high).** It blocks direct formatting and styles that aren't
allowed, and optionally switching themes or style sets. Typing and editing text are not restricted.
Zotero users report that citations still insert and update, but their italics and superscript can't
be applied, so they take the paragraph style. Inserting a TOC can fail unless its styles are allowed.
For Etch: check whether a field update applying direct formatting should be blocked under this
protection.
Sources: https://support.microsoft.com/en-US/Word/restrict-or-permit-formatting-changes ;
https://forums.zotero.org/discussion/comment/307141/ ;
https://techcommunity.microsoft.com/discussions/word/restrict-editing-of-styles-allow-new-breaks-tocs-etc-/4530989

**6. Change Case under tracking (low).** No Microsoft source was found. A LibreOffice bug commenter
reports that Word shows a case change as a formatting revision (a change bar, with no insertion or
deletion). That fits the observation recorded in `typing-parity.md`.
Source: https://www.libreoffice.org/bugzilla/show_bug.cgi?id=130028 (seen only as a search snippet)

**7. Undo grouping of Backspace/Delete (unresolved).** No Word-specific source was found. The
Windows rich-edit control, which is not Word, groups backspaces with typing and starts a new group
at Delete.
Source: https://learn.microsoft.com/en-us/windows/win32/controls/em-stopgrouptyping

**8. Tracked section-break insertion (unresolved).** In the spec, `w:sectPrChange` records changed
section properties. An inserted break is most likely a tracked inserted paragraph mark that carries
the `sectPr`, so rejecting it removes the break. Which section's setup the joined text keeps was
not found.
Sources: https://webapp.docx4java.org/OnlineDemo/ecma376/WordML/sectPr_1.html ;
https://bugs.libreoffice.org/show_bug.cgi?id=120336

### Fields

**11. Insert Field (medium).** Microsoft's articles treat inserting a field and updating fields as
separate steps. Inserting computes only the new field. The TOC changes only on Update Field,
Ctrl+A then F9, or the "update fields before printing" option. No source says inserting updates
other fields.
Sources: https://support.microsoft.com/en-us/word/insert-edit-and-view-fields-in-word ;
https://support.microsoft.com/en-us/word/update-fields

**12. SEQ outside the body (high for headers, footers and notes; medium for text boxes).**
Microsoft's SEQ article says SEQ fields in headers, footers, comments and footnotes don't affect the
body's numbering. An MVP answer says SEQ fields in text boxes do count, ordered by where each text
box is anchored. Ctrl+A then F9 also doesn't reach headers, footers or text boxes.
Sources: https://support.microsoft.com/en-us/office/field-codes-seq-sequence-field-062a387b-dfc9-4ef8-8235-29ee113d59be ;
https://learn.microsoft.com/en-us/answers/a/11464150 ;
https://addbalance.com/articles/Dealing%20with%20Fields%20in%20Microsoft%20Word.htm

**13. Locked SEQ (unresolved).** Locking stops a field recalculating itself. No source says whether
later SEQ fields count a locked one, or continue from the number it shows.
Sources: https://learn.microsoft.com/en-us/answers/a/10797652 ;
https://reference.aspose.com/words/python-net/aspose.words.fields/fieldseq

**14. PRINTDATE (medium).** A document never printed shows an all-zero date. Printing, including to
a PDF printer, records the date; Save As or Export to PDF does not. The date is presumably stored in
`cp:lastPrinted` in `core.xml`, but no source confirms it.
Sources: https://reference.aspose.com/words/net/aspose.words.fields/fieldprintdate ;
https://community.adobe.com/t5/acrobat-discussions/how-can-i-keep-microsoft-word-s-printdate-field-code-from-becoming-plain-text-when-converting-my/m-p/9295619 ;
https://www.libreoffice.org/bugzilla/show_bug.cgi?id=134901

**15. Table of figures and index page-number format (medium for the table of figures, low for the
index).** No Microsoft statement was found. Thesis guides treat the list of figures like the TOC:
fixing the section's page-number format fixes its numbers. Word's page references generally use the
format of the section that contains the target. A table of figures is a TOC field with `\c`.
Sources: https://www.k-state.edu/grad/academics/etdr/template/word/word13/pagenumbers.html ;
https://techcommunity.microsoft.com/discussions/word/toc-page-numbers-not-working-correctly/3484555

**16. Bibliography tag on edit (unresolved).** The Tag name is a field of its own in the source
dialog. Word makes new internal GUIDs when a source is edited, but nothing says the visible tag is
rewritten.
Sources: https://learn.microsoft.com/nb-no/office/vba/word/concepts/working-with-word/working-with-bibliographies ;
https://support.microsoft.com/en-us/office/add-citations-in-a-word-document-ab9322bb-a8d3-47f4-80c8-63c06779f127 ;
https://office-watch.com/2019/source-manager-for-citations-in-word/

**17. Zone-less created/modified dates (unresolved).** LibreOffice discussions suggest reading a
value with no zone as local time, and confirm that Word writes `Z`. How Word reads such a value is
not documented.
Sources: https://www.libreoffice.org/bugzilla/show_bug.cgi?id=134589 ;
https://ask.libreoffice.org/t/timezone-of-creation-date-for-docx-file/54489

**18. System time-zone change while running (unresolved).** Nothing found.

**19. Nested fields on update (low).** Sources describe nesting and say inner fields are evaluated
first, but none states the update scope. The prevailing understanding is that Word updates every
nested field of any kind before the outer one, so a nested SEQ increments and a nested PAGEREF is
recomputed.
Sources: https://addbalance.com/articles/Dealing%20with%20Fields%20in%20Microsoft%20Word.htm ;
https://support.microsoft.com/en-us/word/insert-edit-and-view-fields-in-word

**20. Mail merge main document (high).** Microsoft KB 825765 says opening a merge main document
linked to a data source shows a prompt that it will run an SQL command; this is by design, for
security. Yes keeps the document attached to its data. No opens it detached. On Windows, the DWORD
`SQLSecurityCheck`=0 under `HKCU\Software\Microsoft\Office\16.0\Word\Options` suppresses the prompt.
If the data source path doesn't resolve, Word asks the user to find the file. Keeping `w:mailMerge`
unchanged is what keeps this working. No Mac equivalent of the registry setting was found.
Sources: https://support.microsoft.com/en-au/kb/825765 ;
https://learn.microsoft.com/en-us/answers/questions/5040678/how-to-add-the-dword-value-sqlsecuritycheck-in-wor?page=2

### Tables, selection, limits

**21. Ctrl+A (high).** Word's keyboard-shortcut articles for Windows and Mac list Ctrl/Cmd+A only
as selecting all of the document's content. To select a table on Windows, Alt+Numpad 5 (Num Lock
off) or Layout › Select › Table; the Mac list has no table shortcut. A LibreOffice bug comment calls
LibreOffice's cell-first Ctrl+A an advantage over Word. Ctrl+A in a header, text box or footnote
probably selects all of that story, but that wasn't confirmed.
Sources: https://support.microsoft.com/en-us/office/keyboard-shortcuts-in-word-95ef89dd-7142-4b50-afb2-f762f663ceb2 ;
https://support.microsoft.com/en-us/office/keyboard-shortcuts-in-word-for-mac-3256d48a-7967-475d-be81-a6e3e1284b25 ;
https://learn.microsoft.com/en-us/answers/questions/5123899/selecting-all-text-in-table ;
https://www.libreoffice.org/bugzilla/show_bug.cgi?id=72486

**22. Document size limits (high).** Microsoft's operating-parameter article gives:
- 32 MB for a document's text alone (graphics not counted);
- 512 MB as the largest file Word 2007 and later opens, with a specific error above it;
- no paragraph or page limit;
- styles 4,079, lists 2,047, and about 2.1 billion bookmarks, comments or fields.

In practice memory decides.
Sources: https://learn.microsoft.com/en-us/office/troubleshoot/word/operating-parameter-limitation ;
https://learn.microsoft.com/en-us/office/troubleshoot/word/file-larger-than-512-megabytes ;
https://office-watch.com/2023/microsoft-word-document-size-limit-explained/

**23. Refusing edits at a limit (low).** No designed message is documented. Users report
out-of-memory or disk-space errors when saving, "not enough memory" on Mac, and saving that stops
working beyond a certain size.
Sources: https://techcommunity.microsoft.com/discussions/word/unable-to-make-changes-to-an-85mb-word-doc-and-save-it/3976300 ;
https://learn.microsoft.com/en-us/answers/questions/5922829/out-of-memory-for-a-large-file ;
https://learn.microsoft.com/en-us/answers/a/10321431

**24. AutoFit widening per keystroke (unresolved).** Secondary sources only say that AutoFit to
Contents resizes columns as content changes, and that a long word can push a table past the margin.
Sources: https://office-watch.com/2020/autofit-table-columns-in-word/ ;
https://office-watch.com/2023/word-autofit-table-columns-fix/

### Styles, sections, headers

**25. Linked character style toggling (medium).** Under the spec's toggle rule (§17.7.3), a
character style's bold or italic flips the value from the paragraph style. So Heading 1 Char inside
Heading 1 shows as not bold, and Emphasis inside Quote shows upright. LibreOffice bug 130610 says
Word displays it that way. [MS-OI29500] notes that within paragraph styles a toggle value resets
rather than flips.
Sources: https://learn.microsoft.com/en-us/openspecs/office_standards/ms-oi29500/f7130225-2368-48f3-acae-a9d278d0fb25 ;
https://learn.microsoft.com/en-us/openspecs/office_standards/ms-oi29500/bd62641d-7786-473f-872d-4b3205e91a82 ;
https://www.libreoffice.org/bugzilla/show_bug.cgi?id=130610

**26. Based-on chain (medium).** Microsoft's implementation notes on §17.7.3 treat a "level" as a
style type (table, numbering, paragraph, character), not as each style in a based-on chain. Within
the chain, values are inherited and the most derived one wins, so two bolds don't cancel. The note's
example is a paragraph style; character styles are assumed to work the same way. python-docx's
analysis suggests toggling at each level of the chain, but the author hedges and it conflicts with
Microsoft's note.
Sources: https://learn.microsoft.com/en-us/openspecs/office_standards/ms-oi29500/f7130225-2368-48f3-acae-a9d278d0fb25 ;
https://python-docx-oss.readthedocs.io/en/latest/dev/analysis/features/text/font.html

**27. Page break then an empty section-ending paragraph (low).** `splitPgBreakAndParaMark` (off by
default in Word 2013 and later) only keeps a page break's *own* paragraph mark on the break's page.
A separate following paragraph that holds the Next Page break still starts the new page alone, so a
blank page is expected. Forum advice for exactly this blank page is to delete the page break or make
the section break Continuous.
Sources: https://learn.microsoft.com/en-us/dotnet/api/documentformat.openxml.wordprocessing.compatibility.splitpagebreakandparagraphmark?view=openxml-3.0.1 ;
https://learn.microsoft.com/en-us/answers/a/9932990 ;
https://learn.microsoft.com/en-us/answers/a/9582006

**31. Page Number galleries (medium).** Top of Page and Bottom of Page replace the existing header
or footer. Current Position, or a PAGE field inserted by hand (Alt+Shift+P on Windows), keeps it.
Sources: https://edu.gcfglobal.org/en/word/page-numbers/1/ ;
https://learn.microsoft.com/en-us/answers/a/11760464 ;
https://learn.microsoft.com/en-us/answers/a/12741451 ;
https://helpdesk.commons.gc.cuny.edu/?p=12248

**32. Blank (Three Columns) (low).** It is described as three placeholders aligned by the Header
style's built-in centre and right tab stops; a table comes up only as a workaround when the left text
wraps. The placeholders may be content controls; that wasn't confirmed.
Sources: https://support.microsoft.com/en-us/office/header-page-numbering-tips-49f07430-ca24-444d-a034-21c71fc43fb0 ;
https://www.techrepublic.com/article/align-header-footer-margins-word/

**9. Missing next style (high for Enter).** The spec (§17.7.4.10, quoted on Microsoft's
NextParagraphStyle page) says a `w:next` naming a missing or non-paragraph style is ignored, so the
new paragraph keeps the current style. What Word writes into other styles' `w:next` when the style
they name is deleted was not found.
Source: https://learn.microsoft.com/en-us/dotnet/api/documentformat.openxml.wordprocessing.nextparagraphstyle?view=openxml-3.0.1

### Saving, printing, AutoCorrect

**28. Hidden text and PDF (medium; the display part is high).** Microsoft's Display options page
says the hidden-text display setting only affects the screen. "Print hidden text" decides printing
and is off by default. Users report that PDFs included hidden text until they cleared that option.
On Mac the setting is under Preferences › Print › Include in Print Version › Hidden text; nothing
was found on whether Mac PDF export follows it.
Sources: https://support.microsoft.com/en-US/Word/word-options-display ;
https://community.adobe.com/t5/acrobat-discussions/how-to-prevent-word-hidden-text-from-being-visible-in-pdf-output/m-p/14274752 ;
https://learn.microsoft.com/en-us/answers/questions/5645764/micro-office-2024-is-printing-hidden-text-that-doe

**29. `=rand` / `=lorem` (medium for the default; low for the limits).** Microsoft's article
says the feature needs "Replace text as you type" on. The article still gives an older default, but
Word 2016 and later are reported to produce 5 paragraphs of 3 sentences. Office Watch reports
maximums of 99 paragraphs and 200 sentences; past them (for example `=rand(201,100)`) nothing
happens and the text stays as typed.
Sources: https://support.microsoft.com/en-us/word/how-to-insert-sample-text-in-microsoft-office-word ;
https://office-watch.com/?p=723 ;
https://helpdeskgeek.com/microsoft-word-generate-random-text/

**30. AutoFormat details (medium).**
- (a) Microsoft says `:)` and `:-)` become an emoji, but not whether one fires straight after a
  word. Unresolved.
- (b) `>`, `*` and `-` followed by a space start bullets. Office Watch says the `->` bullet and the
  `-->` arrow are Wingdings characters. Etch uses Unicode on purpose.
- (c) The six border characters are documented: `-` single, `=` double, `_` thick, `*` dotted, `#`
  triple, `~` wavy. That matches Etch's `border_line_autoformat`.

Sources: https://support.microsoft.com/en-US/Word/autoformat-as-you-type ;
https://support.microsoft.com/en-us/office/autocorrect-emojis-in-documents-and-email-601cad94-1a54-41e2-8cf8-4b09a6eadfe1 ;
https://office-watch.com/2022/right-arrow-symbol-word-excel-powerpoint-outlook/ ;
https://learn.microsoft.com/en-us/archive/blogs/hub/adding-and-removing-fancy-dividers-in-word

**10. Repeat warning after "Keep format" (low).** An old AskWoody thread (Word 2000) says Word
warned on every save as .txt. Microsoft KB 970020 describes the ODT compatibility prompt, which has
a "Don't show this message again" check box and a registry value behind it, so the ODT prompt can be
turned off for good. Nothing was found for RTF in current Word.
Sources: https://www.askwoody.com/?p=384281 ; https://support.microsoft.com/kb/970020

### Answered elsewhere

- **33.** Word keeps the VBA project in the open document after a macro-free save, until the
  document is closed, and doesn't prompt again.
- **34.** Word warns before saving to ODT or RTF.
- **35.** Remove Table of Contents removes the whole content control, heading included.
- **36.** Word warns before a macro-free save (Microsoft KB 926051): Yes saves without the macros,
  No goes back to choose a type.

Etch behaves the same on all four.
Sources: https://support.microsoft.com/en-us/kb/926051 ; https://support.microsoft.com/kb/970020

## More questions

Clean-room: public web pages only. No Word binaries, no GUI.

| # | Question | Word | Confidence | Etch |
|---|---|---|---|---|
| 1 | Replace All scope | Dialog (Search: All): every story: body, headers/footers, footnotes/endnotes, text boxes, comments. Exception: text boxes inside headers/footers. VBA `ActiveDocument.Content.Find`: main story only. A recorded macro: only the caret's story. | Medium-high | Differs |
| 2 | Delete from start of A into B, then the paragraphs join | B's formatting survives (B's paragraph mark is the one left; A's mark is deleted). Sources disagree in places. | Low | Probably differs |
| 3 | Ctrl+Home/End/A in a footnote | Ctrl+A selects all footnotes (the notes story), not the body. Ctrl+Home/End likely stay in the notes story too. | Ctrl+A: medium. Home/End: low | Ctrl+A matches; Home/End probably match |
| 4 | Next Page break at end of a numbered item | No empty numbered paragraph *holding* the break: the item's own paragraph ends the section (in OOXML, `sectPr` sits in that paragraph's `pPr`). The new paragraph after the break probably inherits the numbering (empty "n+1." on the next page). | Storage: high (spec). UI: low | Differs |
| 5 | Undo grouping of Backspace/Delete | Typing goes into large "Typing" undo actions, not one per keystroke or word. AutoCorrect/AutoFormat, formatting changes, Overtype and (by analogy) caret moves break a group. Exactly how Backspace/Delete runs group is undocumented. | Low | Unknown |

### 1. Replace All scope
- **Word:** Doug Robbins (wordmvp.com) says the built-in Find/Replace works "almost" anywhere. Text in a text box inside a header or footer "is outside the scope of the Find and Replace utility". A *recorded* macro "will only act on the part of the document that contains the insertion point". `Document.Content` is "the main document story", the same as `StoryRanges(wdMainTextStory)`, so `ActiveDocument.Content.Find.Execute Replace:=wdReplaceAll` only touches the body. Code that covers everything loops through `StoryRanges` plus `NextStoryRange`, and also the `ShapeRange` text frames in headers and footers. The scope covers comments, footnote, endnote, header/footer and text-frame stories (wordmvp story list). Comments in the dialog's scope comes from that story list; I didn't find a direct statement.
- **Confidence:** medium-high. That VBA's `Content` is main-story only comes from a Microsoft doc. The dialog's scope comes from one MVP source plus consistent forum reports.
- **URLs:** https://wordmvp.com/FAQs/Customization/ReplaceAnywhere.htm · https://wordmvp.com/FAQs/MacrosVBA/FindReplaceAllWithVBA.htm · https://learn.microsoft.com/en-us/office/vba/api/word.document.content · https://learn.microsoft.com/en-us/office/vba/api/word.storyranges
- **Etch:** differs. Etch replaces in the caret's story only, which matches a recorded macro, not the dialog.
- **Test:** put "zzz" in the body, header, a footnote, a text box and a comment. With the caret in the body, Ctrl+H, zzz→yyy, More > Search: All, Replace All. Check each place and the count Word reports.

### 2. Which formatting survives the join
- **Word:** paragraph formatting lives in the paragraph mark. The selection removes A's ¶ and keeps B's ¶, so the classic rule says the joined paragraph has **B's** style, indents and list membership. The sources are weak and partly conflict. An O'Reilly book on Office XP says "the new paragraph takes on the formatting of the deleted paragraph mark". Bettersolutions says both "will then match that of the next paragraph" and "the paragraph mark associated with the first paragraph is applied to the second". A newsgroup expert says the result is "the first style in the combined paragraph". Forum lore also says newer Word versions keep the first paragraph's formatting when the join happens *mid-paragraph* (Delete at the end of A). I couldn't confirm that. Either way it shouldn't apply here, because the selection starts at the very start of A, so none of A's text is left.
- **Confidence:** low.
- **URLs:** https://bettersolutions.com/word/paragraphs/paragraph-marks.htm · https://www.oreilly.com/library/view/special-edition-using/0789725134/0789725134_ch15lev1sec6.html · https://groups.google.com/g/microsoft.public.word.newusers/c/EqAY241llEw
- **Etch:** probably differs (Etch keeps A's formatting).
- **Test:** A = Heading 1, B = a numbered Normal paragraph with a 1" indent. Select from the start of A to the middle of B, press Delete, and read the style box, ruler and numbering. Repeat with the selection starting mid-A.

### 3. Ctrl+Home / Ctrl+End / Ctrl+A in a footnote
- **Word:** Susan Harkins (TechRepublic, 2017) gives "click inside any footnote and press Ctrl+A" as a way to select **all footnotes and only footnotes**. In the body, Ctrl+A selects everything except headers, footers, footnotes and endnotes. So Select All is scoped to the story. I found no source for Ctrl+Home/End. By the same story scoping they should go to the start of the first note and the end of the last note, not into the body (inference).
- **Confidence:** Ctrl+A medium. Ctrl+Home/End low.
- **URLs:** https://www.techrepublic.com/article/office-q-a-three-ways-to-select-all-footnotes-in-a-document/
- **Etch:** Ctrl+A matches (it stays in the note), as long as Etch's "note" means the whole notes story. If Etch selects only the current footnote, it differs. Ctrl+Home/End: unknown, probably matches.
- **Test:** add 3 footnotes, click in footnote 2 (Print Layout). Ctrl+A: what is highlighted? Ctrl+Home, then Ctrl+End: where does the caret land?

### 4. Next Page break at the end of a numbered item
- **Word:** ECMA-376 Part 1 §17.6.17/§17.6.18 stores a section's properties in the `w:sectPr` of the *last paragraph's* `w:pPr`. The final section's properties go in `w:body`. So the numbered item's own paragraph can end section 1, and no empty paragraph is needed to carry the break. In the UI, the break mark shows right after the item's text, in place of its ¶. I didn't find a source describing the paragraph created after the break. By analogy with pressing Enter, it is likely an empty paragraph with the same list formatting (an empty "4." at the top of the next page).
- **Confidence:** how the break is stored: high (spec). What the UI creates: low.
- **URLs:** ECMA-376 Part 1, §17.6.17 sectPr and §17.6.18 sectPr (https://ecma-international.org/publications-and-standards/standards/ecma-376/)
- **Etch:** differs. Etch creates an extra empty numbered paragraph to hold the break. In Word the item itself ends the section, and any empty numbered paragraph comes *after* the break.
- **Test:** make a list 1–3, put the caret at the end of item 3, Layout > Breaks > Next Page, turn ¶ on. Is the break on item 3's line? Is there an empty "4." on page 2? Is there an empty numbered paragraph before the break?

### 5. Undo grouping of Backspace/Delete
- **Word:** Microsoft doesn't document the rules. Office Watch says typed characters are not separate actions: a whole paragraph or more can be undone at once. AutoCorrect changes, formatting changes and dialog changes are separate steps. Suzanne Barnhill (MVP, Microsoft Q&A) says Overtype mode makes undo character-by-character. Microsoft's RichEdit docs (the control family, not Word) group typing *including Backspace* into one action until the selection moves (arrow keys or mouse) or Delete is pressed. Word probably behaves similarly, but that's unconfirmed. Whether a run of Delete keypresses is one step or merges with Typing is unknown.
- **Confidence:** low.
- **URLs:** https://office-watch.com/2021/beyond-simple-undo-in-microsoft-office/ · https://learn.microsoft.com/en-us/answers/questions/4868709/undo-undoes-one-letter-at-a-time-instead-of-the-wh · https://learn.microsoft.com/en-us/windows/win32/controls/em-stopgrouptyping
- **Etch:** unknown.
- **Test:** type "hello world", press Backspace 5 times, and open the Undo dropdown to see the entries and their labels. Repeat with: Delete ×5 at mid-word; Backspace ×3, Left arrow, Backspace ×3; type then Backspace without pausing. Count the Ctrl+Z presses needed each time.

## Unresolved: manual test steps

Each takes about 2 minutes in Word, with ¶ (Show/Hide) on.

1. **item 7 Undo grouping.** Type `abcdef`, press Backspace three times, and open the Undo drop-down;
   note the entries and their names. Press Ctrl+Z once and note what comes back. Repeat with Delete
   (caret moved left first), then with type, Backspace, type.
2. **item 8 Tracked section break.** In a one-section portrait document, turn Track Changes on. Insert
   Layout › Breaks › Next Page mid-text and make the second section landscape. Save a copy (look in
   `document.xml` for `w:ins` in the break paragraph's mark and any `w:sectPrChange`). Reject the
   break and note whether the joined section is portrait or landscape.
3. **item 13 Locked SEQ.** Type three lines `{SEQ x}` (Ctrl+F9 for the braces) and press F9 to get
   1, 2, 3. Change the second code to `{SEQ x \r 10}` and press F9 (it shows 10). Lock it with
   Ctrl+F11, then remove `\r 10`. Press Ctrl+A, then F9. If the third shows 11, Word continues from
   the locked value; 3 means the locked field is counted; 2 means it is skipped.
4. **item 16 Bibliography tag.** References › Insert Citation › Add New Source, with author Smith and
   year 2020; note the tag. Manage Sources › Edit, change them to Jones and 2021, then OK. Compare
   the tag and the citation's code (Alt+F9).
5. **item 17 Zone-less dates.** On a machine not set to UTC, unzip a .docx and set
   `<dcterms:created xsi:type="dcterms:W3CDTF">2026-01-15T21:59:44</dcterms:created>`. Rezip and
   open it. Compare File › Info and `{CREATEDATE \@ "HH:mm"}` with 21:59: the same means local; your
   offset applied means UTC.
6. **item 18 Time-zone change.** Insert `{TIME \@ "HH:mm"}`. With Word open, change the system time
   zone, then select the field and press F9. Repeat after restarting Word.
7. **item 24 AutoFit widening.** Insert a 3×2 table and choose Layout › AutoFit › AutoFit Contents. In
   A1, type `x` twenty times without spaces, watching the column border: does it move on each key?
8. **#30a Smiley after a word.** Type `ok:) ` and then `ok :) `, and note which one becomes an emoji.
9. Partial answers worth confirming:
   - **item 1:** paragraph 1 centred Heading 1, paragraph 2 left Normal. Select from the start of
     paragraph 1 into the middle of paragraph 2 and delete, tracked off; then repeat tracked on and
     Accept All. Note the style each time.
   - **item 2:** with tracking on, insert and then delete a table column; note any warning and shading.
   - **item 6:** with tracking on, select `hello world`, press Shift+F3, and note the balloon: Formatted,
     or a deletion and an insertion?
   - **item 19:** `{IF 1 = 1 "{SEQ t}" "x"}` after two `{SEQ t}` fields. Add another SEQ t above, update
     only the IF field, and see whether its number changes.
   - **item 21:** Ctrl+A in a cell, then in a header, a text box and a footnote.
   - **item 27:** type `A`, press Ctrl+Enter, insert a Next Page break, type `B`; is it 2 pages or 3?
   - **item 29:** type `=rand(100,1)`, `=rand(99,201)`, `=rand(0,3)` and `=rand(a)`, pressing Enter after
     each.
   - **item 10:** save as .txt and Keep format; type a word and Ctrl+S, then Ctrl+S again with no
     change. Repeat for .rtf and .odt.
   - **item 9 part 2:** set style X's next style to Y, delete Y, then read X's "Style for following
     paragraph".

## What Etch does

Tests are in `crates/engine/src/tests_word_parity.rs` unless another file is named.

| Item | Etch | Test |
|---|---|---|
| item 21 Ctrl+A in a cell | Ctrl+A in a cell selects the whole document, as in Word. | `tests::select_all_in_a_cell_selects_the_whole_document` |
| item 4 tracked columns | **Left as it is**, column tracking kept: LibreOffice writes `w:cellIns`/`w:cellDel` for tracked table columns in DOCX since 7.6 (commit 85a47bbb, tdf#155328), its code comments that Word exports the same revision ids, and tdf#157187 describes the fix for Word losing such column changes, so Word reads them when they are well formed. Not a Microsoft statement: a question remains (below). | — |
| cell highlighting | **Changed:** the markup views fill inserted cells light blue and deleted cells (and the cells of deleted rows) pink, Word's defaults, under the text, theme-aware on the dark page; Track Changes Options take other colours (`cellInsertColor`, `cellDeleteColor`). Merged and split cells have no colour: neither Etch nor Word records them (item 3). | `tracked_cells_are_highlighted_in_the_markup_views`; `wordcraft-render tests::tracked_cells_are_highlighted_in_markup_light_and_dark` (pixels, light and dark) |
| item 11 Insert Field | **Changed:** `insert.field` computes only the new field (a SEQ takes its number in the sequence, a PAGEREF its page, a TOC or generated list is built); the next Update Fields settles the document. | `insert_field_computes_only_the_new_field` |
| item 12 SEQ in text boxes | **Changed:** SEQ fields in text boxes anchored in the body count in the body's sequence at their anchor; headers, footers and notes stay out. | `seq_fields_in_text_boxes_count_at_their_anchor` |
| item 14 PRINTDATE | **Changed:** never printed, the all-zero date (`0/0/0000`, ECMA-376 §17.16.5.49); File › Print and Print to PDF record `cp:lastPrinted` (Part 2 §11.1) and refresh PRINTDATE fields; Export and Save As PDF don't; the record round-trips DOCX and Word 2003 XML. | `printdate_is_the_zero_date_until_a_print_records_it` |
| item 15 table of figures / index pages | **Changed for the table of figures:** each caption's page in its section's page-number format, as the TOC. **Index left decimal:** the further search found nothing on Word's INDEX field and roman page numbers. | `table_of_figures_pages_follow_the_section_number_format` |
| item 28 Print hidden text | **Changed:** a new preference (`file.options {"printHiddenText"}`, Options › Display › Printing, off as in Word); prints, PDFs and page images follow it, never the ¶ display. | `print_hidden_text_decides_prints_and_pdfs_not_the_screen` |
| item 19 nested fields | **Left as it is** (low): the further search found only the fact that nested fields are evaluated inside out and Microsoft's "which fields are updated when" articles, none saying whether updating the outer field recomputes every nested kind. Sources: https://support.microsoft.com/en-us/topic/some-fields-are-updated-while-other-fields-are-not-eb3ad53a-328b-a3b4-91c6-28a84001855f ; https://support.microsoft.com/en-US/Word/which-fields-are-updated-when-you-open-repaginate-or-print-document ; https://addbalance.com/articles/Dealing%20with%20Fields%20in%20Microsoft%20Word.htm | — |
| item 29 `=rand` limits | **Left as it is** (low): the sources disagree: Office Watch says 99 paragraphs and 200 sentences, a French tutorial says up to 200 paragraphs of 99 sentences or 99 paragraphs of 200 sentences (https://office-watch.com/?p=723 ; https://www.moyens.net/?p=181546). | — |
| item 6 Change Case under tracking | **Left as it is** (low): nothing new; only LibreOffice bug reports about LibreOffice's own Change Case under tracking (https://www.libreoffice.org/bugzilla/show_bug.cgi?id=141198). `typing-parity.md` keeps the Differs row. | — |
| item 3 merge/split warning | **Changed:** with Track Changes on, Merge Cells and Split Cells ask first in the window (OK / Cancel; `ui.untrackedAction`), and the engine's result says `"untracked": true`; a script passes `confirmed`. Confirmed by a Microsoft Q&A thread quoting the dialog and Eric White's note (https://learn.microsoft.com/en-us/answers/a/10147163 ; https://www.ericwhite.com/blog/?p=4019). | `wordcraft-ui-egui tests_untracked_action::merge_and_split_with_tracking_on_ask_first_and_stay_untracked`; `tracked_cells_are_highlighted_in_the_markup_views` |
| Replace All scope | **Changed:** Replace All covers every story (body, headers, footers, notes, text boxes, comments; not text boxes in headers and footers), one undo step, tracked under Track Changes; `"in": "story"` keeps the caret's story. | `replace_all_covers_every_story_but_text_boxes_in_headers` |
| Section break at a list item's end | **Pinned, no change needed:** the item's own paragraph already holds the `sectPr`; the paragraph after the break is an empty item (low for Word, left as it is). | `a_section_break_at_the_end_of_a_list_item_is_held_by_the_item` |
| item 27 (blank page) | **Question, left as it is:** Etch gives two pages for "A", a page break, a Next Page break, "B" (the break's paragraph mark stays on page 1, and the new section starts page 2), where the research expects a blank page 2 at low confidence. Not changed without an observation. | — |
| item 5 (formatting-only protection) | **Pinned:** fields insert and update, direct formatting is refused. | `formatting_only_protection_lets_fields_insert_and_update` |

Open question left from item 2: how Word itself renders `w:cellIns`/`w:cellDel` written by another
program (highlighted cells, the Reviewing Pane, Accept/Reject) is still inferred from LibreOffice's
interoperability work, not seen.
