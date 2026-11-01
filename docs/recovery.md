# Local document recovery

Desktop Etch periodically keeps a local copy of unsaved work, including new drafts.
File › Recover lists available copies. The page opens at startup when copies are found.
Restore opens an unsaved document with no file path. Save As chooses where to keep it.
Recovery never writes to the original document. AutoSave's permission to write to an earlier
file is cleared on restore.

Continue without restoring leaves copies available. Discard asks for confirmation and deletes
the selected copy, including its retained generation. It does not delete the editing document.
After discarding that document's copy, another copy is made only after a further edit.
A successful save removes that document's recovery copy on the next worker cycle while it
remains open; if the saved document is replaced first (Save, then New or Open), its copy is
removed as soon as the worker is free, and closing the window on a saved document removes it
before the app exits. Don't Save at the Save Changes prompt (New, Open, Close, quitting)
removes the abandoned document's copy at once. A document with unsaved changes replaced without
that choice (an agent's `file.open`, a forced `app.quit`) keeps its copy: it may be the only one.

## Password-protected documents

A document with a password (opened with one, or given one by Encrypt with Password or Save with
a password) gets an encrypted recovery copy. Nothing of it is written to disk in plain form, not
even in temporary files: the snapshot is encrypted in memory with the document's password, in
the same container as a password-protected Word document (`wordcraft_docx::encrypt`: agile
encryption, AES-256-CBC, a SHA-512 key derived with 100,000 iterations and a random salt, and an
HMAC that detects damage). Its pictures are encrypted the same way, each in its own side file,
named by a keyed hash so the names say nothing about the pictures. The copy's first line holds
only what File › Recover lists: its id, when it was made, and that it is protected. It is listed
as "Protected document", without its file name or folder (either could say what it is about).
For confined automation it counts as an untitled document: outside every root.

Restore asks for the password, the same prompt as opening a protected file. A wrong password is
an error that asks again; the copy stays as it was. The restored document keeps the password, so
it saves encrypted and its later copies stay encrypted. Discovery can't check an encrypted
generation without the password, so a damaged newest one restores the retained earlier one. `file.recover {"restore": id}` without
`"password"` fails with the same "protected with a password" error as `file.open`.

Applying a password (or a different one) waits for any pending snapshot, removes the current
document's copies, and fails if that cleanup fails: no plaintext copy, and no copy under an old
password, survives protection. Removing the password makes plain copies again from the next edit.
Tested: `protected_documents_get_an_encrypted_copy_restored_with_their_password`,
`a_new_password_purges_copies_under_the_old_one`, `password_barrier_*` and
`opening_protected_file_*` in `recovery/desktop.rs` scan every byte of the recovery folder for
known plaintext markers (text, picture bytes, the file name) and find no plain generation or side
file; `a_protected_copy_asks_for_its_password` (ui-egui) drives the prompt.

## Pictures and large documents

Pictures and other binary parts are kept beside the snapshot, each in a side file in the record
named by its SHA-256 (`part-<hash>.bin`; `.enc` for a protected document), written once: later
generations refer to the same file, so an unchanged picture isn't written or hashed again (the
hash is remembered while the picture's bytes are unchanged). The snapshot's JSON holds the text
and structure and refers to each part by file, length and hash; a restore checks both. A side
file that is missing or damaged costs that picture, not the text: the restore says which
pictures it couldn't bring back. Side files no retained generation uses are removed.

Before anything is written, the snapshot is planned against the budget: the text and structure
first, then the pictures that fit, smallest first. Pictures that don't fit are left out of that
copy and named; the text is still kept. When even the text doesn't fit, the snapshot fails
before writing anything. Nothing is written and then deleted for being too large.

When recovery can't cover the document's unsaved changes, the status bar says so in red, for as
long as it lasts: "Not protected by recovery: pictures too large to keep (n)" (the tooltip names
them and suggests saving) or "…: no copy could be written" (the tooltip gives the reason).
Clicking it opens File › Recover. Saving the document clears it. `file.recover` reports the same
as `gap`: `{"omitted": [part names]}`, `{"failed": "reason"}`, or null. Tested:
`over_budget_keeps_the_text_and_names_the_pictures_left_out`,
`the_session_reports_what_recovery_cannot_cover`, `pictures_are_side_files_written_once`,
`a_damaged_picture_file_leaves_out_the_picture_not_the_text`, `hostile_part_names_are_refused`
and (ui-egui) `the_status_bar_says_when_recovery_cannot_cover_the_document`.

## Storage and timing

Copies live in `recovery/` under the app's profile folder: on Linux `$XDG_CONFIG_HOME/etch` (by
default `~/.config/etch`), on macOS `~/Library/Application Support/Etch`, on Windows
`%APPDATA%\Etch`. `ETCH_PROFILE_DIR` (or `WORDCRAFT_PROFILE_DIR`; Etch's wins when both are set)
selects a separate profile for isolated runs, including preferences, logs and control keys.

Earlier builds, named Etch, used `wordcraft` / `Etch` folders in the same places. When
only the old folder exists, the first start copies it, recovery copies included, before anything
reads or writes the folder (`apps/wordcraft/src/settings.rs`): into a temporary sibling folder,
every file synchronized to disk, then renamed into place in one step, so the new folder is either
complete or absent. The old folder is never moved, changed or deleted; it stays as a fallback. If
the copy fails, the temporary folder is removed and that run uses the old folder, so its recovery
copies and settings are still found; the next start tries again. Links inside the old folder are
not followed, and files that can't be copied (special files, unreadable ones) are skipped; both
are logged and never stop the start. Two instances starting at once end up sharing one copy. No
migration happens with a profile folder override or with `WORDCRAFT_NO_PREFS` (which then uses
the old folder while it is the only one).
`WORDCRAFT_NO_PREFS` disables recovery as well as preferences. Headless engine/CLI sessions
and the browser do not create recovery files implicitly.

The desktop host checks for a dirty document every frame but snapshots at most once per ten
seconds, and only when its revision changes. A slow snapshot spaces out the next: after one
that took `t`, the next waits at least `20 t` (at most five minutes), so recovery uses no more
than about 5% of one core while typing. It clones the shared document model for a bounded
worker queue. Encoding, file I/O, full-document validation and discovery run on the worker.
An unchanged document does not schedule repeated recovery repaints. When the session ends
normally, the snapshot under way is finished and the worker releases its record locks first.

Measured (Linux, Ryzen 5 7530U, release build, synthetic document of 6,000 paragraphs and a
27 MB picture, `cargo test --release -p wordcraft-engine recovery_cost -- --ignored
--nocapture`): the UI thread spends ~0.1 ms cloning the document and ~0.08 ms queueing the
snapshot, so typing never waits; the worker takes ~0.95 s to encode, write, synchronize, read
back and compare a 39 MB copy (1.2 s and 99 MB when bytes were arrays of numbers); listing one
such copy at start-up takes ~0.7 s on the worker. These numbers predate side files.

A document with 300 MB of pictures (ten synthetic 30 MB pictures, `cargo test -p
wordcraft-engine recovery_cost_300mb -- --ignored --nocapture`, test profile, same machine,
2026-10-15). Before side files, every attempt took ~1.4 s, wrote 240 MiB to a temporary file,
deleted it and failed (no copy at all), again at every interval. Now:

| Snapshot | Time | Written |
|---|---|---|
| First copy | 0.50 s | 300 MB (the pictures once, the text) |
| After a text edit | 4 ms | under 1 MB (the text only) |
| First copy after Encrypt with Password | 2.3 s | 302 MB (all encrypted) |
| Protected, after a text edit | 35 ms | under 1 MB (one key derivation) |

The most recent edits can be lost if a crash happens before the next snapshot or while its
write is still pending, except when the app is terminated or panics (below).

## Termination and panics

Being told to quit by the system no longer loses the edits since the last periodic copy. On
Unix, SIGTERM (logout, shutdown, `kill`), SIGINT (Ctrl+C in the terminal that started the app)
and SIGHUP (that terminal closed) are caught by a watcher thread (`signal-hook`), which wakes the
window: on its next frame it writes an up-to-date copy of a document with unsaved changes
synchronously (`Session::emergency_snapshot`, waiting for the worker for at most 10 s) and quits
without the Save Changes prompt. The copy is offered at the next start. A second signal, or a
window that hasn't quit within 20 s, ends the process at once (exit status 128 + the signal), as
the signal would have. Closing the app with unsaved changes by any other route that skips the
prompt (an agent's forced quit) also writes the copy as the app is dropped.

A panic asks for the same copy. The panic hook only sets a flag: it takes no lock and touches no
session (the panicking thread may hold any lock, a poisoned one would panic again, and a panic
inside the hook aborts). The UI thread, which owns the session, writes the copy on its next
frame (a panic `Session::run` caught, or one on another thread) or, when the panic unwinds the
UI thread itself, as the app is dropped. Not covered: a panic that aborts instead of unwinding
(macOS AppKit callbacks), SIGKILL and power loss, which leave the last periodic copy. Windows'
session end (`WM_QUERYENDSESSION`/`WM_ENDSESSION`) isn't passed on by winit, so logging off there
relies on the periodic copy too. A password-protected document's emergency copy is encrypted,
like its periodic ones. Pictures the periodic copy already wrote aren't written again, so an
emergency copy of a document with large pictures usually writes only its text. New pictures are
written only while at least 1.5 s of the 10 s bound remain; after that the text is written
without them, and they are named as left out.
Tested: `crates/engine/tests/emergency_snapshot.rs` (the copy is on disk when the call returns,
before any periodic tick, and restores at the next start; none for saved documents; a protected
document's copy is encrypted, holds no plaintext marker and restores with its password; an
8 MB picture already on disk isn't rewritten, and with 1 s left the text is kept and the new
picture named) and `apps/wordcraft/src/termination.rs` (a SIGHUP sent to the test process asks the
window to quit; the app's copy then restores).

Version 2 snapshots wrap the native JSON document with an id, document name, timestamp and
monotonic generation, and list its pictures and opaque preserved parts as side files (above);
chart/SmartArt/OLE source markup and relationships are in the JSON. Version 1 snapshots, with
media inline (one base64 string per part, or the older arrays of numbers), still open, and the
next copy moves their pictures to side files. Older JSON without these fields still opens.
Paragraph cache revisions are transient and regenerated on load; undo history and selection are
not part of the document snapshot. Recovery preserves the current in-memory model. It cannot
recover content already lost during import or make later export more faithful than the exporter.

Each generation is encoded in memory, decoded and compared with the source document, written
exclusively to a temporary file, synchronized, read back and compared, then renamed to a new
generation. Side files are written the same way before the generation that uses them. The last valid
snapshot is retained until its successor is valid. Discovery falls back to an older valid copy
if the newest is corrupt and reports unreadable files.

A plain generation carries a checksum of its own (side files have their SHA-256, protected
copies the encryption container's HMAC): it starts with the line `Etch recovery copy`, then
`sha256=<hex> length=<bytes>` for the JSON that follows. Listing and restoring verify both, so a
copy damaged on disk in a way that keeps its JSON valid (a bit flip in the text) is refused like an
unreadable one instead of restoring a silently different document. When the newest generation is
refused (damaged, or a protected one that doesn't decrypt), File › Recover lists the one saved
before it, marked "The newest copy was damaged: this is the one saved before it." (`earlier` in
`file.recover`), and restoring it says on the status bar that the copy before it was restored and
the most recent edits may be missing; a generation damaged after it was listed falls back the
same way at Restore. Copies written before checksums (the JSON alone) are listed and restore as
before, unchecked; their next generation has a checksum. The checksum is SHA-256, already used
for the side files (`sha2`). Tested: `tests_save_safety::a_recovery_copy_damaged_on_disk_restores_the_one_before_it_and_says_so`,
`a_recovery_copy_written_before_checksums_still_restores` (a fixture in the old format),
`a_damaged_recovery_copy_is_refused_or_restored_never_a_panic` (checksummed copies: every damage
refused; old copies: refused or restored, never a panic) and (ui-egui)
`a_damaged_recovery_copy_says_the_one_before_it_is_offered_and_restored` (in German).

File › Recover marks a copy "Older than the saved file" (`olderThanFile`) when the file it was
made from was changed after the copy was made (saved again here before the copy was removed, or
by another program): restoring it opens it as an untitled document and never touches the file,
which may hold newer work (`file_recover_says_when_a_copy_is_older_than_its_saved_file`,
`a_copy_older_than_the_saved_file_never_overwrites_it`). Abandoned temporary files are cleaned
only after acquiring that record's lock. Native file locks prevent other running instances from
restoring or deleting a live record. Unix folders/files use modes 0700/0600, and an existing
`recovery/` folder that others could read is made 0700 before anything is written to it;
Windows uses the profile's inherited permissions. Directory synchronization runs on Unix; Windows synchronizes
the file but does not yet explicitly synchronize directory metadata. Power-loss durability on
Windows is therefore unverified.

Limits are 272 MiB per generation file (the text and structure; a protected copy may be larger
by what encryption adds), 1 GiB per record (its generations and side files together), 2 GiB for
the profile's recovery folder, 128 records, 16 generations and 65,536 side files per record
during discovery. Ordinarily only two valid generations remain. The sizes follow from the limits
every document is held to (`wordcraft_doc::limits`): a generation file holds a document's saved
size (256 MB, which bounds its JSON as well as its Word XML) and the listing of its side files,
and a record two generations and the 256 MB of pictures a document may have, so the copy of a
document at every limit is written whole and loads back
(`recovery::desktop::tests::recovery_budgets_follow_from_the_document_limits`; at the real
limits, `a_recovery_copy_at_the_limits_loads_back`, ignored for its size). Copies are retained until saved or explicitly discarded; there is no
automatic age-based deletion. A snapshot over the limits keeps the text without the pictures
that don't fit (above); one whose text doesn't fit reports an error and retains the previous
valid copy. Side files hold the bytes as they are (not base64) and nothing is compressed, so a
copy can be larger than the DOCX. The interface is translated into every interface language. There is no cloud,
version-history or browser recovery.

## Commands and verification

`file.recover={}` reads cached `{enabled,busy,documents,error,gap}`. Each document has `id`, `name`
and `savedAt`, and `path`, the file it was opened from or saved to when the copy was made
(none for an untitled document, a protected one, or a file name that isn't valid Unicode);
`protected: true` for a protected document's copy, and `omitted`, the parts it couldn't keep;
`earlier: true` when the newest generation was damaged and this is the one before it, and
`olderThanFile: true` when its file was changed after the copy was made.
`{"restore": id, "password": "…"}` restores a protected copy. `{"refresh":true}`, `{"restore":"id"}` and `{"discard":"id"}` queue worker
operations. Poll until `busy` is false; inspect `error` for failure. A restore is cancelled if the
document id or revision changes while it loads. Programmatic restores do not open dialogs;
user restores over dirty work use the existing Save Changes prompt.
`ui.recoveryDiscard={"id":"id"}` opens confirmation; `{"answer":"discard"}` or
`{"answer":"cancel"}` answers it.

Focused tests are `cargo test -p wordcraft-engine recovery`,
`cargo test -p wordcraft-engine --test recovery_format`, and
`cargo test -p wordcraft-ui-egui recovery`. They cover lifecycle, preservation, original-file
protection, failed writes, corruption, locks, password cleanup and the actual recovery buttons.
Faults are injected at each durable step (`crates/engine/src/io/fault.rs`, test builds only):
creating the temporary file, writing it (also half of it, a disk filling up), synchronizing it,
reading it back, renaming it and synchronizing the folder, of a picture's side file and of the
generation. After each, the last good copy restores whole at the next start, the session reports
"no copy could be written", no temporary file is left and the next copy succeeds
(`tests_save_safety::a_recovery_copy_that_fails_at_any_step_keeps_the_last_good_one`,
`a_recovery_folder_that_fails_to_synchronize_keeps_the_copy_before`). A generation damaged on
disk (digits changed, numbers made huge, bytes flipped, the file cut short) is refused or restores
a document that lays out, edits and saves, never a panic
(`a_damaged_recovery_copy_is_refused_or_restored_never_a_panic`); copies near the record and
folder budgets, plain and protected, restore whole less exactly the pictures they name
(`copies_near_the_budgets_restore_whole_or_name_what_they_left_out`).
The contribution also uses a native SIGKILL/restart replay in temporary profiles for both new
drafts and edited existing files. It uses synthetic inputs and hashes the original before and
after. No Microsoft Word operation is needed for this reliability test.

## Maintenance validation

The 2026-10-12 rebase retains main’s editable graphic models, native font setup, current
options module, fixed-width Backstage and filename-aware Save As. The SIGKILL/restart
observations above belong to the original contribution, not a rerun on this branch. Current
validation executes the focused lifecycle and preservation tests in temporary profiles and
all repository CI gates; no daily-driver recovery folder or native app is operated.

The desktop host chooses the private recovery profile. Recovery commands accept known
snapshot IDs, never file paths or a profile override. This internal store is separate from
the automation roots that constrain opening and saving user documents.

A confined control connection ([confinement](control-protocol.md#confinement)) restores a copy
only when its original file (`path`) is inside the connection's read root, and discards one
(`file.recover {"discard": id}`, `ui.recoveryDiscard`) only inside its write root. A copy of an
untitled document counts as outside every root. Its listing leaves out `path` for files outside
its read root. The person's own File › Recover is not restricted.

Discovery skips a record it can't use (more generations than a record holds, a folder that can't
be listed or cleaned) with a warning in the log and lists the others; `error` then says some
recovery files could not be read. The skipped record is left on disk as it was.
