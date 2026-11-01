# Installing spelling dictionaries

English spelling works out of the box. Polish, Russian, Portuguese, Czech, Italian, Croatian and
Turkish spelling use a standard **Hunspell** dictionary (an `.aff` and a `.dic` file) that
Etch reads at runtime. These dictionaries are not bundled: they are large, and several are
GPL or LGPL licensed. Without one, the language isn't checked. The status bar then shows the
language with **(no dictionary)**, for example *Português (Brasil) (no dictionary)*.

If a dictionary is installed but can't be read (an unsupported `SET` encoding, a file over the
size limit, a parse error), the language isn't checked either, and the status bar shows
**(dictionary unreadable)** instead. Its tooltip gives the `.dic` file and the reason, and
**File › Options › Proofing** lists the same reason next to the file.

Install a dictionary once, restart Etch, and the language is checked as you type.

## Which files each language needs

Etch looks for the first of these file names (each name means a `<name>.aff` and
`<name>.dic` pair in the same folder):

| Language | File names | Common source |
|---|---|---|
| Polish | `pl_PL`, `pl-PL`, `pl` | SJP.PL |
| Russian | `ru_RU`, `ru-RU`, `ru` | LibreOffice `dict-ru` |
| Portuguese (Brazil and Portugal) | `pt_BR`, `pt-BR`, `pt` | VERO (LibreOffice `dict-pt-BR`) |
| Czech | `cs_CZ`, `cs-CZ`, `cs` | LibreOffice `dict-cs` |
| Italian | `it_IT`, `it_CH`, `it-IT`, `it` | LibreOffice `dict-it` |
| Croatian | `hr_HR`, `hr-HR`, `hr` | LibreOffice `dict-hr` |
| Turkish | `tr_TR`, `tr-TR`, `tr` | LibreOffice `dict-tr` |

Every Portuguese region uses the same `pt_BR` dictionary, so text marked *Português (Portugal)*
is checked with it too.

The files may be UTF-8 or the encoding named on the `.aff`'s `SET` line (ISO 8859-1, ISO 8859-2, KOI8-R
and Windows-1251 are read as well; the original VERO `pt_BR` is ISO 8859-1).

## Where Etch looks

The first folder holding a matching pair wins:

1. The folder set in **File › Options › Proofing › Dictionaries folder**. This is the easiest
   way on every system, and it works for an app started from the Start menu, Finder or the Dock.
   The same pane lists the dictionary file each language found.
2. The folder in the `WORDCRAFT_DICTIONARIES` environment variable.
3. The system's usual places:

| System | Folders |
|---|---|
| Windows | `%LOCALAPPDATA%\Etch\dictionaries`, `%LOCALAPPDATA%\Etch\dictionaries` (earlier builds), `%LOCALAPPDATA%\hunspell`, `%APPDATA%\Etch\dictionaries`, `%APPDATA%\Etch\dictionaries`, `%APPDATA%\hunspell`, LibreOffice's `share\extensions\dict-*` under `Program Files` (on any drive) |
| macOS | `~/Library/Spelling`, `/Library/Spelling`, `/opt/homebrew/share/hunspell`, `/usr/local/share/hunspell`, `/Applications/LibreOffice.app/Contents/Resources/extensions/dict-*` |
| Linux and BSD | `~/.local/share/hunspell`, `/usr/share/hunspell`, `/usr/share/myspell`, `/usr/share/myspell/dicts`, `/usr/local/share/hunspell`, `/usr/local/share/myspell`, LibreOffice's `share/extensions/dict-*` under `/usr/lib/libreoffice` or `/opt/libreoffice` |

The web version can't read these folders, so these languages aren't spell-checked there.

## Example: Brazilian Portuguese (`pt_BR`)

VERO, the Brazilian Portuguese dictionary LibreOffice ships, is available from the
[LibreOffice dictionaries repository](https://github.com/LibreOffice/dictionaries/tree/master/pt_BR)
(LGPLv3 / MPL). Any of these ways works.

### If LibreOffice is installed

Nothing to do: Etch finds LibreOffice's `dict-pt-BR` extension on its own. Restart
Etch after installing LibreOffice.

### Windows

In PowerShell:

```powershell
$d = "$env:LOCALAPPDATA\Etch\dictionaries"
New-Item -ItemType Directory -Force $d | Out-Null
$base = "https://raw.githubusercontent.com/LibreOffice/dictionaries/master/pt_BR"
Invoke-WebRequest "$base/pt_BR.aff" -OutFile "$d\pt_BR.aff" -UseBasicParsing
Invoke-WebRequest "$base/pt_BR.dic" -OutFile "$d\pt_BR.dic" -UseBasicParsing
```

### macOS

```sh
mkdir -p ~/Library/Spelling
cd ~/Library/Spelling
curl -fLO https://raw.githubusercontent.com/LibreOffice/dictionaries/master/pt_BR/pt_BR.aff
curl -fLO https://raw.githubusercontent.com/LibreOffice/dictionaries/master/pt_BR/pt_BR.dic
```

### Linux

From the distribution's packages:

```sh
sudo apt install hunspell-pt-br   # Debian, Ubuntu
```

Or, without root, into your own folder:

```sh
mkdir -p ~/.local/share/hunspell
cd ~/.local/share/hunspell
curl -fLO https://raw.githubusercontent.com/LibreOffice/dictionaries/master/pt_BR/pt_BR.aff
curl -fLO https://raw.githubusercontent.com/LibreOffice/dictionaries/master/pt_BR/pt_BR.dic
```

Other languages work the same way: replace `pt_BR` with the language's folder and file names
from the LibreOffice repository (for example `it_IT/it_IT.aff`).

## Checking that it works

- Restart Etch. The status bar no longer says **(no dictionary)**, and a misspelled word
  such as `impramir` is underlined, with `imprimir` among its suggestions.
- **File › Options › Proofing** lists the file each language found.
- From the command line:

  ```sh
  etch-cli run --cmd 'text.insert={"text":"Vou impramir o documento."}' --cmd select.all --cmd 'review.language={"lang":"pt-BR"}' --cmd review.issues --print
  ```

  The `review.issues` line lists `impramir` as a spelling issue with its suggestions.
- The log file (`%APPDATA%\Etch\logs\etch.log` on Windows) says
  `Portuguese spelling: no Hunspell pt_BR dictionary installed` when none was found.

Never commit dictionary files to this repository: they stay on the user's system.
