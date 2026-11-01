# Control protocol

The control channel is for tests and development only. It exists only in builds of the desktop app
with the non-default `automation` feature:
`cargo run --release -p etch --features automation -- --sample --control 7981`. A default or
release build opens no socket and writes no key file, and refuses `--control`, the automation root
flags and `WORDCRAFT_CONTROL_PORT` with `built without automation support` and exit status 2.

Start the app with a control port: `etch --control 7981` (or `WORDCRAFT_CONTROL_PORT=7981`).
A missing or invalid port after `--control` (not 1–65535) prints an error and exits with status 2.
It listens on `127.0.0.1` only. Send one JSON object per line, with the window's key (see
[Keys](#keys)); each gets one reply line:

```json
{"id": 1, "key": "…", "method": "engine.execute", "params": {"command": "text.insert", "params": {"text": "Hello"}}}
{"id": 1, "ok": true, "result": {"anchor": {...}, "focus": {...}}}
```

Errors come back as `{"id": …, "ok": false, "error": "message"}`. A failed command leaves the
document unchanged. A line that isn't valid JSON gets one error reply, then the server closes the
connection (so an HTTP request sent to the port can't run a command); reconnect to continue.

## Keys

Any local program (and possibly a web page) can reach a loopback port, so every request needs the
window's key. Once the port is bound, the app writes a random key (256 bits, as hex) to
`<settings>/control-key.<instance>` and removes the file when the window closes; a new start
writes a new key. `ETCH_PROFILE_DIR` (or `WORDCRAFT_PROFILE_DIR`) overrides the whole profile folder for isolated runs,
including recovery copies. See [local recovery](recovery.md). `<settings>` is the folder that holds the preferences: on Linux
`$XDG_CONFIG_HOME/etch` (by default `~/.config/etch`), on macOS
`~/Library/Application Support/Etch`, on Windows `%APPDATA%\Etch` (or, for a run where copying
the old Etch folder there failed, that old folder: the log says which). `<instance>` is the
Flatpak instance id inside Flatpak, and `local-<port>` otherwise. On Unix the file has mode 0600.

```sh
cat ~/.config/etch/control-key.local-7981   # Linux, outside Flatpak, port 7981
```

- Send the key as top-level `"key"` with every request, or once per connection with
  `{"method": "auth", "key": "…"}` (reply `{"ok": true, "result": {"confined": false, "readRoot":
  null, "writeRoot": null}}`, see [Confinement](#confinement)); later requests on that
  connection may leave `key` out. Only `auth` reads `params.key`; for other methods `params.key`
  is an ordinary parameter (`ui.key {"key": "Enter"}` presses Enter).
- The key is checked on every request. A missing or wrong key gets
  `{"ok": false, "error": "unauthorized"}`, and the app closes the connection.
- A line over 1 MiB gets `{"ok": false, "error": "line too long"}`, and the app closes the
  connection. Until the connection has sent an authorised request the limit is 64 KiB, and the
  error comes as soon as that much has arrived, so a client without the key can't make the app hold
  a large line. Send `auth` (or a short request with the key) first.
- A connection that sends no authorised request within 30 s of being accepted is closed. That is
  one limit: blank lines and bad JSON do not extend it. After an authorised request the
  connection may stay idle.
- At most 16 connections are open at once. A connection past that gets
  `{"ok": false, "error": "too many connections"}` and is closed; a slot frees when a connection
  ends.

## Confinement

A connection can confine the files its commands read and write to folders:

```json
{"id": 1, "method": "auth", "key": "…", "params": {"readRoot": "/work/project", "writeRoot": "/work/project/out"}}
{"id": 1, "ok": true, "result": {"confined": true, "readRoot": "/work/project", "writeRoot": "/work/project/out"}}
```

- `auth` (or `confine`, later on the connection, with the same params) takes `readRoot` and
  `writeRoot`: absolute paths of existing folders, or null. Naming either confines the connection;
  a root left out or null grants none of its access. The result gives the roots in force, links
  followed (`confined: false`: none).
- From then on every request on that connection runs inside the roots: `engine.execute` and the
  bare command ids (`file.open`, `file.saveAs`, `file.exportPdf`, `insert.picture`,
  `insert.textFromFile`, `review.compare`, `ui.render`… every command that takes a path) and
  `ui.screenshot`'s `path`. A path outside is refused with an ordinary error, for example
  `automation path rejected: outside the write root /work/project/out: /etc/x.docx`. The checks
  (`crates/engine/src/file_access.rs`): links followed, `..` resolved, compared by whole folder names, case
  ignored on Windows, device names refused. Relative paths are resolved against the app's working
  directory.
- The clicks and keys the connection sends (`ui.click`, `ui.key`, `ui.text`…) play inside its
  roots too, with the next few frames, so they can't drive File › Open or the built-in file browser
  past them. Everything else the person does in the window is not confined.
- AutoSave carries the confinement of the edits it writes. It writes on a worker thread, so the app
  records the roots of every confined connection that changed (or replaced) the document since the
  person last saved it, and AutoSave writes the file only if each of them may write it; otherwise
  it stops, says why in the status bar, and resumes once the person saves. `file.autosave` with
  `{"value": true}` from a confined connection is refused for a file outside its write root.
- A confined connection never discards unsaved work: with unsaved changes, `app.quit` and
  `file.close` are refused even with `"force": true`, and so are the commands that replace the
  document (`file.new`, `file.open` of a path, `file.recover` restore, Envelopes and Labels in a
  new document) and Don't Save in the person's Save Changes prompt
  (`ui.saveChanges`). Save first (inside the write root), or leave it to the person.
- Recovery copies ([local recovery](recovery.md)): a confined connection restores
  (`file.recover {"restore": id}`) only a copy whose original file is inside its read root, and
  discards one (`{"discard": id}`, `ui.recoveryDiscard`) only inside its write root; a copy of an
  untitled document counts as outside. The listing leaves out `path` for files outside its read root.
- Roots only narrow: a later `auth` or `confine` must name folders inside the roots in force (a
  subfolder, or the same), and can't grant a kind of access they don't. A request that would widen
  them, names a relative or missing folder, or gives something other than a string or null, gets
  an error and the app closes the connection, so a client that asked to be confined never carries
  on unconfined. `auth` without `readRoot`/`writeRoot` keeps the roots as they are.
- `etch --control 7981 --automation-read-root DIR --automation-write-root DIR` (each flag
  once, `--flag DIR` or `--flag=DIR`; a missing folder is an error) confines every connection from
  its start; connections can narrow those roots, never leave them. Without the flags a connection
  is unconfined until it asks. A flag, or a flag's value, that isn't valid Unicode (in either
  spelling, also `--control`) is an error (exit status 2), never a file name to open.
- A client that needs the confinement should check `confined` in the reply: an app from before
  this answers `auth` with `{}` and confines nothing.

## Methods

| Method | Params | Result |
|---|---|---|
| `engine.execute` | `command`, `params` | the command's result (any id from `engine.commands`, including UI commands `ui.tab`, `ui.dialog` (`{"name": dialog, "part"?: table style part}`; `part` opens New / Modify Table Style (`newTableStyle` / `modifyTableStyle`) on that part: `wholeTable`, `headerRow`, `firstColumn`, `bandedRows`, `nwCell`… as `table.modifyStyle` names them), `ui.backstage`, `ui.zotero.*` (see `docs/zotero.md`), `ui.language` (`{"value": "auto"|"en"|"zh-hans"|"zh-hant"|"ja"|"uk"|"es"|"pt-br"|"sr"|"sr-latn"|"pl"|"cs"|"et"|"de"|"ru"|"tr"|"it"|"hu"|"fr"|"bg"|"vi"|"hr"|"ko"|"sv"}`), `ui.theme` (`{"value": "system"|"light"|"dark"}`; `system` follows the OS appearance, light when it reports none; also `ui.theme.light`, `ui.theme.dark`, `ui.theme.system` without params, which command search lists), `ui.pageInDark` (Page colour in dark theme, `{"value": "follow"|"light"|"dark"}`; no value reads it; `follow`, the default, draws the pages dark while the interface theme is dark, `light` never, `dark` always; returns `{"pageInDark", "darkPage"}`, `darkPage` being whether pages are dark now; setting it ends a `view.darkMode` (Switch Modes) override, which lasts the session and isn't saved; also `ui.pageInDark.follow`, `ui.pageInDark.light`, `ui.pageInDark.dark` without params), `ui.interfaceSize` (`{"value": "auto"}` or `{"percent": 50–300}`, clamped; no value reads it; `auto` keeps the system display scale, a percentage is absolute, so 100 is one screen pixel per point; the result's `percent` is the size drawn at and `auto` whether it follows the system; document zoom stays `view.zoom`), `ui.animations` (`{"value": "system"|"on"|"off"}`; no value reads it; `system` follows the system's animation setting, which the desktop app reads on Windows, macOS, Linux and the BSDs and the web app from `prefers-reduced-motion`, and means on where the system doesn't say; the result's `enabled` is what the setting comes to and `system` what the system reports, `null` when it doesn't say), `ui.fileBrowser` (desktop: `{"purpose": "document"|"picture"|"save"}` opens Etch's own file browser, for when the system file dialog doesn't show up), `ui.caretAnimation` (`{"value": true|false}`; no value reads it; off, the caret jumps and blinks on and off instead of gliding and fading), `ui.startScreen` (`{"value": true|false}`; no value reads it; on, the default, the desktop and web apps start on File › Home when launched with nothing to open), `ui.saveConflict` (`{"answer": "copy"|"cancel"}`; answers the Save conflict dialog that opens when a native Save or AutoSave finds the file changed, moved or unreadable since it was opened or last saved: `cancel` keeps editing, `copy` asks for a new file name (`<name> copy.<ext>`) and saves there with `file.saveAs`, returning `{"pending": "saveCopy"}` while the picker is open; the external version is never overwritten; errors when no such dialog is open or the document has been replaced), `ui.saveFormat` (`{"answer": "keep"|"docx"|"cancel"}`; answers the compatibility prompt the person's first Save over a file in a format that can't keep everything shows, or a Save As, Export or Save Copy to a file that would leave out the document's macro project (see [Saving](#saving)): `keep` saves in the file's format (to the file the Save As picked), `docx` opens Save As with a `.docx` suggested (`.docm`, or `.dotm` for a template, when the prompt named the macros), `cancel` saves nothing; errors when no such prompt is open or the document has been replaced), `ui.contextMenu` (opens the page's context menu below the caret, as Shift+F10 does)…) |
| `auth` / `confine` | `readRoot?`, `writeRoot?` (`auth` also `key`) | the roots in force: `confined`, `readRoot`, `writeRoot` ([Confinement](#confinement)) |
| `engine.execute` | `command`, `params` | the command's result (any id from `engine.commands`, including UI commands `ui.tab`, `ui.dialog` (`{"name": dialog, "part"?: table style part}`; `part` opens New / Modify Table Style (`newTableStyle` / `modifyTableStyle`) on that part: `wholeTable`, `headerRow`, `firstColumn`, `bandedRows`, `nwCell`… as `table.modifyStyle` names them), `ui.backstage`, `ui.zotero.*` (see `docs/zotero.md`), `ui.language` (`{"value": "auto"|"en"|"zh-hans"|"zh-hant"|"ja"|"pt-br"|"es"|"uk"|"sr"|"sr-latn"|"et"|"tr"|"ru"|"pl"|"de"|"it"|"cs"|"hu"|"fr"}`), `ui.theme` (`{"value": "system"|"light"|"dark"}`; `system` follows the OS appearance, light when it reports none), `ui.interfaceSize` (`{"value": "auto"}` or `{"percent": 50–300}`, clamped; no value reads it; `auto` keeps the system display scale, a percentage is absolute, so 100 is one screen pixel per point; the result's `percent` is the size drawn at and `auto` whether it follows the system; document zoom stays `view.zoom`), `ui.animations` (`{"value": "system"|"on"|"off"}`; no value reads it; `system` follows the system's animation setting, which the desktop app reads on Windows, macOS, Linux and the BSDs and the web app from `prefers-reduced-motion`, and means on where the system doesn't say; the result's `enabled` is what the setting comes to and `system` what the system reports, `null` when it doesn't say), `ui.fileBrowser` (desktop: `{"purpose": "document"|"picture"|"save"}` opens Etch's own file browser, for when the system file dialog doesn't show up), `ui.caretAnimation` (`{"value": true|false}`; no value reads it; off, the caret jumps and blinks on and off instead of gliding and fading), `ui.startScreen` (`{"value": true|false}`; no value reads it; on, the default, the desktop and web apps start on File › Home when launched with nothing to open), `ui.alignmentGuides` (`{"value": true|false}`; no value reads it; on, the default, moving a picture or shape snaps it to the page, the margins and other objects, with guide lines while dragging; Alt moves freely), `ui.quickAccessToolbar` (`{"value": true|false}`; no value toggles it; false takes the Quick Access Toolbar out of the title bar, as its menu's Hide Quick Access Toolbar does, and true brings it back)…) |
| `engine.commands` | — | every command: id, label, location, shortcut, params, enabled |
| `document.inspect` | `text?` | blocks (text, style, runs, lists, tables), parts, sections, selection, pages |
| `ui.inspect` | — | UI state, view, dialog, window size, page rects on screen, caret, comment balloons (`balloons`: id + screen rect; `balloon`: the selected one), perf |
| `ui.click` | `x`, `y`, `button?`, `count?`, `shift?`, `cmd?`, `alt?` | real pointer input at window coordinates |
| `ui.clickText` | `page` (0-based), `x`, `y` (points from the page's top-left), `count?` | click inside a page |
| `ui.move` / `ui.drag` | `x`,`y` / `x`,`y`,`toX`,`toY`,`steps?` | pointer move / drag |
| `ui.press` / `ui.release` | `x`,`y`, modifiers | primary button down / up there (with `ui.move` between: a drag you can screenshot halfway) |
| `ui.wheel` | `x`, `y`, `dy`, `dx?`, `unit?` (`point`/`line`/`page`), `phase?` (`start`/`move`/`end`), modifiers | mouse-wheel or trackpad scroll at window coordinates (`dy` < 0 scrolls down) |
| `ui.key` | `key` (`B`, `Enter`, `Left`…), `shift?`, `alt?`, `cmd?` | key press through egui (shortcuts apply) |
| `ui.text` | `text` | typed text through egui |
| `ui.screenshot` | `path?` | PNG of the window: written to `path`, or without one returned as base64 `png` in the reply |
| `ui.render` | `path?`, `page?` (1-based), `scale?` | PNG of a page, rendered by the engine: written to `path`, or without one returned as base64 `png` in the reply (never a fixed name in the shared temporary folder) |
| `ui.parity` | — | feature-catalog parity |
| `ui.resize` / `ui.focus` / `app.quit` | | window control |

Any other method name is treated as a command id: `{"method": "format.bold"}` works.

The `CharProps` and `ParaProps` that `format.set`, `para.set` and `styles.modify` take are listed
in [Character and paragraph properties](#character-and-paragraph-properties).

Positions are `{"story": "body" | {"part": n}, "path": [block, row, cell, block, …], "off": byteOffset}`.

### Saving

`file.save` and `file.saveAs` never open a dialog for a script. Saving in a format that can't
keep the whole document (anything but `.docx`/`.docm`/`.dotx`/`.dotm`, Word XML `.xml` and
Etch `.json`: ODT, RTF, HTML, Markdown, LaTeX, plain text, PDF, PNG) still saves, since the
script asked for it, and the result says what was lost:
`{"saved": true, "path": "…", "encrypted": false, "lossy": ["comments", "footnotes"]}`. The
`lossy` ids are `comments`, `footnotes`, `endnotes`, `headersFooters`, `trackedChanges`,
`textBoxes`, `equations`, `contentControls`, `drawings` (shapes, charts, SmartArt),
`nestedTables` (RTF, Markdown, text), `pictures` (text), `nestedFields` and `macros` (a macro
project read from a `.docm`/`.dotm`, which only `.docm`, `.dotm` and JSON keep: also reported for
a `.docx`, `.dotx` or Word XML file); the array is empty when the format can't keep everything but
this document uses none of them, and `lossy` is absent for formats that keep everything. A save over the document's own file in such a format leaves it saved, as Word's
Keep format does (the person is asked first, `ui.saveFormat`; AutoSave never writes such a file).
A format Etch opens but can't write (Word 97-2003 `.doc`/`.dot`) is an error naming the
alternative: `file.saveAs {"path": "<name>.docx"}`.

### Keyboard and text input

`ui.key` and `ui.text` go to this window's focused widget even when another app is in front. A focused Find field receives the text; a document receives it only while its canvas
owns keyboard focus. Shortcuts, dialogs and document protection still apply. These methods do
not activate the native window. `ui.focus` separately requests native activation.

The `{"queued": true}` reply acknowledges the input queue, not a completed edit. Input is applied
on subsequent UI frames. Inspect the resulting document or widget state before continuing with
an operation that depends on it; do not replay an edit simply because its effect is not immediate.

### Closing the document

`engine.execute` with `file.close` never shows the Save / Don't Save / Cancel prompt. On a
document with unsaved changes it replies with an error (the app keeps running and the edits
stay); save first (`file.save`), or pass `{"force": true}` to discard the changes and close.
The bare form `{"method": "file.close", "force": true}` (a method name that is a command id) is
guarded the same way, and only a boolean `true` counts as `force`. A document without unsaved
changes closes at once. Closing closes the document, not the app: the window stays and
shows no page, and `file.new` or `file.open` goes on from there. A
[confined](#confinement) connection can't discard unsaved changes at all: `force` is refused
there too.

`app.quit` follows the same contract: with unsaved changes it replies with an error and the
window stays open, unless `params` has `"force": true` (only a boolean `true` counts), which
quits and discards them (not from a confined connection). With nothing unsaved it quits at once.

### File access

Unless confined ([Confinement](#confinement)), commands sent to the app read and write any path
the app's user can.

### Native printing

`engine.execute` can call `ui.printSettings` with `{}` to inspect the printer list, selected
printer, copies, duplex, grayscale, discovery/submission status and backend receipt/error.
`{"refresh":true}` starts background discovery; poll the same command until `discovering` is
false. Set `printer` to a discovered name, integer `copies` from 1 to 999, `duplex` to
`printerDefault`, `off`, `longEdge` or `shortEdge`, and boolean `grayscale`. Invalid updates are
rejected without changing earlier settings. `ui.printSubmit` explicitly submits the current
print-ready PDF without opening a dialog; inspect `ui.printSettings` until `busy` becomes false
and check `status`/`detail`. The browser keeps its own print hook. PDF export remains separate.

`ui.printSubmit` / `ui.print` also accept the PDF page settings (`range`, `pages`,
`parity`, `copies`, `collate`). Native printing renders the selected pages once and passes
copies/collation to the spooler; the browser and `file.printPdf` keep copies in the PDF.
Omitting native `copies` uses the configured `ui.printSettings` value. `collate` defaults to
File › Print's setting; invalid ranges submit no job. `paper` (`none`, or a Layout › Size name
such as `Letter`, `Legal`, `A4`) is Scale to paper size: every page is scaled onto that
paper and centred, and native jobs ask the spooler for that `media`; the document's page size is
unchanged. `file.printPdf` takes it too; an unknown paper submits no job.

`engine.commands {"search":"print"}` lists the engine print commands and the app-owned
`ui.printSettings`, `ui.printSubmit`, and legacy `ui.print` alias, with current enablement
and their parameter documentation. Search is case-insensitive across IDs, labels, locations
and parameter descriptions. Listing commands never discovers printers, submits a job or opens
a dialog. Settings inspection is always available; submission becomes enabled when the native
printer is ready, or when the browser host provides its print flow.

## Character and paragraph properties

`format.set {"props": CharProps}`, `para.set {"props": ParaProps}`,
`styles.modify {"style": …, "chr"?: CharProps, "para"?: ParaProps}` and the `para` of
`table.newStyle` / `table.modifyStyle` take these two objects. Every field is optional: a field
left out keeps what the style chain gives. A field name the object doesn't know is ignored, not
refused, so check the spelling. Lengths are points. A colour is a hex string `"RRGGBB"` or
`"#RRGGBB"`, or `[r, g, b]` (three numbers 0–255); it reads back as `[r, g, b]`.

```json
{"command": "styles.modify", "params": {"style": "Heading1",
  "chr": {"font": "Georgia", "size": 18, "bold": true, "color": "1B1611"},
  "para": {"spaceBefore": 12, "lineSpacing": {"rule": "multiple", "value": 1.15}, "keepNext": true}}}
```

### CharProps

| Field | Type | Values |
|---|---|---|
| `style` | string | character style id |
| `font` | string | font name |
| `size` | number | points |
| `bold`, `italic`, `strike`, `doubleStrike`, `caps`, `smallCaps`, `hidden`, `outline`, `shadow`, `emboss`, `engrave`, `noProof` | bool | |
| `underline` | string | `none`, `single`, `words`, `double`, `thick`, `dotted`, `dash`, `dotDash`, `dotDotDash`, `wave`, `doubleWave` |
| `underlineColor` | colour | |
| `color` | `"Auto"`, a colour, or `{"Rgb": colour}` | `Auto` (also `"auto"`) is black on light backgrounds, white on dark; read back as `"Auto"` or `{"Rgb": …}` |
| `highlight` | string | `none`, `yellow`, `brightGreen`, `turquoise`, `pink`, `blue`, `red`, `darkBlue`, `teal`, `green`, `violet`, `darkRed`, `darkYellow`, `gray50`, `gray25`, `black`, `white` |
| `shading` | colour | background fill |
| `border` | Border | character border; `"style": "none"` is explicitly no border |
| `vertAlign` | string | `baseline`, `superscript`, `subscript` |
| `spacing` | number | character spacing, points: positive expands, negative condenses |
| `scale` | number | horizontal scale, percent (100 = normal) |
| `position` | number | raised (positive) or lowered (negative), points |
| `kern` | number | kern fonts at or above this size, points; 0 = off |
| `textEffects` | object | WordArt `fill`, `outline`, `shadow`, `glow`, `reflection` (see `crates/doc/src/wordart.rs`) |
| `lang` | string | BCP 47 language tag (proofing) |
| `rtl` | bool | right-to-left run |
| `cs` | bool | format the whole run with the complex-script properties |
| `fontCs`, `fontEa` | string | complex-script font; East Asian font |
| `sizeCs` | number | complex-script size, points |
| `boldCs`, `italicCs` | bool | complex-script bold, italic |
| `langBidi` | string | BCP 47 language of complex-script text, e.g. `fa-IR` |
| `link` | string | hyperlink target: a URL, or `#bookmark` |
| `ins`, `del`, `fmtChange` | | Track Changes bookkeeping (revision indexes) |

`format.set` also applies `size`, `bold` and `italic` to `sizeCs`, `boldCs` and `italicCs` when
those aren't given; `styles.modify` sets only what it is given.

A **Border** is `{"style": …, "width": pt, "color": colour | null, "space": pt}` (`space` is
the distance from the text). Missing fields are `none`, 0, no colour and 0, so a border needs a
`width` above 0 to show. `style` is one of `none`, `single`, `double`, `dotted`, `dashed`,
`thick`, `triple`, `dotDash`, `wave`.

### ParaProps

| Field | Type | Values |
|---|---|---|
| `style` | string | paragraph style id |
| `align` | string | `left`, `center`, `right`, `justify`, `distribute` (`left` is the start edge: the right in a right-to-left paragraph) |
| `indentLeft`, `indentRight` | number | points |
| `indentFirst` | number | first-line indent, points; negative = hanging |
| `indentLeftChars`, `indentRightChars`, `indentFirstChars` | number | the same indents in characters; a non-zero value wins over the one in points, 0 cancels an inherited one |
| `spaceBefore`, `spaceAfter` | number | points |
| `lineSpacing` | object | `{"rule": "multiple", "value": 1.15}` (1 = single, 2 = double), `{"rule": "atLeast", "value": pt}`, `{"rule": "exactly", "value": pt}` |
| `contextualSpacing` | bool | ignore space before/after between paragraphs of the same style |
| `mirrorIndents` | bool | left and right indents swap on even pages |
| `keepNext`, `keepLines`, `pageBreakBefore`, `widowControl` | bool | |
| `outlineLevel` | integer | 0–8 = Level 1–9, 9 = body text |
| `numbering` | object | `{"num": n, "level": n}`, both required; `level` counts from 0; `num` 0 removes numbering a style gives |
| `tabs` | array | `[{"pos": pt, "align"?: "left\|center\|right\|decimal\|bar\|clear", "leader"?: "none\|dot\|hyphen\|underscore\|middleDot"}]`; replaces the inherited stops, `clear` removes an inherited one at `pos` |
| `shading` | colour | background fill |
| `borders` | object | `{"top", "left", "bottom", "right", "between"}`, each a Border (`between`: between paragraphs; `inside_v` is used by tables) |
| `suppressHyphens`, `suppressLineNumbers` | bool | |
| `bidi` | bool | right-to-left paragraph |
| `dropCap` | integer | lines to drop, 0 = none |
| `kinsoku`, `wordWrap`, `overflowPunct`, `autoSpaceDE`, `autoSpaceDN`, `snapToGrid` | bool | East Asian typography; unset = on |
| `topLinePunct` | bool | East Asian typography; unset = off |
| `numChange`, `fmtChange` | | Track Changes bookkeeping |

`para.set` also takes `"onPage": {"align"?: "left|center|right|justify|distribute", "left"?: pt,
"right"?: pt}`: the alignment and indents as seen on the page, which each selected paragraph turns
into its own start and end once `props` (a `bidi` in it, say) is applied. The Paragraph dialog sends
only the fields that changed this way, as one command and so one undo step.
