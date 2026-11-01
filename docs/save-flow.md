# How saving works

The promises Etch's save flow keeps, and the few places where it deliberately or knowingly doesn't.
The transitions are pinned by `crates/ui-egui/src/tests_save_flow.rs`; recovery copies are described
in [recovery.md](recovery.md).

## The promises

1. **A failed write never marks the document saved.** "Saved" is set only after the file is on
   disk, and only if the document hasn't changed since the bytes were made.
2. **No recovery copy is deleted before the real save succeeds.** A recovery copy is removed only
   once the document is clean, which only a successful save makes it. The exceptions are the
   person's own choices: Don't Save, and protecting with a password (below).
3. **A second Save while one is in flight never races.** On the desktop every explicit save waits
   for an AutoSave write under way. Two writes to one file never overlap.
4. **Closing with changes always asks or saves.** The close button, File › Close, New, Open,
   Recover › Restore, Envelopes and Labels ask Save / Don't Save / Cancel first when there are
   unsaved changes, an unconfirmed browser save, or a comment draft the document refuses. A failed
   or cancelled save cancels the close.

Supporting rules:

- AutoSave writes only to a file the person saved to in this session, in a format that keeps
  everything, never in the browser, and never to a file a confined control connection that changed
  the document may not write.
- A save over a file someone else changed, or over a read-only file, is refused and offers Save
  Copy. Nothing is overwritten.
- A save in a format that can't keep everything asks first, once per document and file, and again
  only if the document would lose more.
- Scripts and the control channel never see a prompt: they save and report.

## Exceptions

- **Browser:** a Save marks the document saved when its bytes go to the browser's save picker,
  before the picker writes; a cancel or failure puts "unsaved" back, and New, Open and Close ask
  first meanwhile. A second Save while the first is still in the picker starts a second save; if
  both target the same file, the last to finish wins.
- **Saving with a password** deletes the unencrypted recovery copy before the encrypted write, so
  no unencrypted copy outlives it. If the write fails, the next recovery copy comes within 10 s.
- **Don't Save** discards the recovery copy before the held-back command runs; if that command then
  fails (opening an unreadable file), the document stays open without a copy until the next cycle.
- **A hung AutoSave write** (an unresponsive network mount) makes Close and File commands wait,
  with no timeout. Nothing is lost: the recovery copy is at most 10 s old, and the file is never
  written in place.
