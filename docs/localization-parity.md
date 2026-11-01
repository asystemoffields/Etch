# Localization parity with Microsoft Word

Interface languages, script support and proofing per language. How translation works (catalog
format, adding a language, clean-room rule): `crates/ui-egui/src/i18n/mod.rs`.

**Dimension: ~29% (estimated)** across the twelve key languages.

## What Word ships (measured by listing the bundle)

- **30 interface languages** (31 `.lproj` folders incl. Base): ar, cs, da, de, el, en, en-GB, es,
  es-MX, fi, fr, fr-CA, he, hu, id, it, ja, ko, nl, no, pl, pt (BR), pt-PT, ru, sk, sv, th, tr,
  zh-CN, zh-TW. Arabic and Hebrew get a mirrored interface. No Hindi or Vietnamese interface on Mac.
- **120 proofing tools** (`SharedSupport/Proofing Tools`): spellers for ~50 languages, grammar
  checkers for ~20, hyphenators for ~16, thesauri for ~20.

## What Etch ships (measured)

- **23 interface languages**: English plus `zh-hans`, `zh-hant`, `ja`, Korean `ko`, `uk`, `es`, `pt-br`,
  Serbian `sr`/`sr-latn`, Estonian `et`, Turkish `tr`, Russian `ru`, Italian `it`, German `de`, Polish `pl`, Czech `cs`, Hungarian `hu`, French `fr`, Bulgarian `bg`, Vietnamese `vi`, Swedish `sv` and Croatian `hr`. Catalogs have 2,061–2,504 entries
  each; Polish has every key of every other catalog, including the built-in style names and the equation gallery, except the citation style names only Hungarian, Bulgarian and Swedish translate so far (a test checks it). A test (`every_tab_and_command_is_translated`) enforces that every ribbon tab, command
  label and ribbon location is translated in every language.
  `pl`, `pl-PL` and `pl_PL.UTF-8` system locales pick Polish automatically.
- Of the 675 English strings a scan extracts from `tl!(…)` calls and command labels/locations, each
  catalog covers **661 (98%)**. Some newer dialog and pane strings (Equation tab tooltips, Zotero,
  Read Aloud, Backstage greetings) aren't wrapped in `tl!` yet, so they show in English in every
  language: the true share of visible strings is **~93–95%** (estimated).
- Pending PRs: Norwegian Bokmål and Nynorsk, Hebrew with RTL.
- **Proofing: English, Polish, Portuguese, Italian, Russian, Czech and Indonesian**, chosen per run from its language (`w:lang`, resolved through
  the styles; Review › Language and the status bar set it: a short list, plus Set Proofing
  Language… for any of 30 languages and Set As Default for the document). Languages without proofing tools get
  the styles; Review › Language and the status bar set it). Languages without proofing tools get
  no spelling or grammar marks (as in Word without the language's tools) instead of being flagged
  as English. Detail in [Proofing per language](#proofing-per-language).
- Document scripts: shaping via HarfRust (HarfBuzz port) for every script; bidi (UAX) and Arabic
  shaping tested; IME on all desktop platforms (winit fixes pending).

## The twelve key languages

Status: `full` = every user-visible string translated and the script renders and edits correctly;
`partial`; `menus only`; `none`.

| Language | Code | UI strings translated | Dialogs / tooltips / help | Script support | Native review | Status |
|---|---|---|---|---|---|---|
| English | en | source (100%) | ✅ | Latin ✅ | yes | **full** |
| Simplified Chinese (Mandarin) | zh-hans | 929 entries; 98% of extracted, ~95% of visible | most; newer panes English | CJK ✅, IME ✅; Windows UI font fixed; interface font chain covers every catalog character, installed CJK font after the embedded ones when it doesn't, PingFang found on macOS 12; font list by Chinese names (宋体, 微软雅黑); no vertical text | no | partial |
| Spanish | es | ~930; 98% / ~95% | most | Latin ✅ | no | partial |
| Hindi | hi | 0 | ❌ | Devanagari shaping via HarfRust, untested; no hyphenation | no | none |
| Arabic | ar | 0 | ❌ | shaping, joining, lam-alef, bidi ✅; no kashida; **no mirrored UI** | no | none |
| French | fr | 2,615 entries (complete catalog) | all | Latin ✅ | no | partial (no proofing) |
| Portuguese | pt-br (pt-PT none) | ~930; 98% / ~95% (pt-BR) | most | Latin ✅ | no | partial |
| Indonesian | id | 0 | ❌ | Latin ✅ | no | none |
| Japanese | ja | ~930; 98% / ~95% | most | CJK ✅, IME ✅; no vertical text, ruby or document grid | no | partial |
| German | de | 2,021 entries; same key set as zh-hans; 98% of extracted, ~95% of visible | most | Latin ✅ | no | partial |
| Korean | ko | 2,463 entries (all catalog keys and preview context) | most; newer panes English | Hangul ✅ (Korean-first embedded/system UI font fallback); macOS Korean IME fixes pending | no | partial |
| Vietnamese | vi | 2,448 entries; every key of the reference catalogs; 98% of extracted, ~95% of visible | most | Latin ✅ | no | partial |

Other shipped languages: **16** — Traditional Chinese (`zh-hant`), Ukrainian (`uk`), Serbian
Cyrillic and Latin (`sr`, `sr-latn`), Estonian (`et`), Turkish (`tr`), Russian (`ru`), Italian (`it`), Polish (`pl`), Czech (`cs`), Hungarian (`hu`), French (`fr`), Bulgarian (`bg`), Vietnamese (`vi`), Swedish (`sv`) and Croatian (`hr`), all partial.
Swedish (`sv-SE` and `sv-FI` system locales, `{"value":"sv"}`) has 2,504 entries. Counts use
count-neutral wording (`{words} ord`: the noun is the same in the singular and the plural). The
catalog is an AI-assisted draft whose terminology and running app were reviewed by a native
speaker. Swedish proofing is not included; terminology: [`localization-sv.md`](localization-sv.md).
Croatian (`hr-HR` system locales, `{"value":"hr"}`) has 2,507 entries: every key of the reference catalog, built-in style names and the equation gallery. Counts use count-neutral wording (`Riječi: 21`) because Croatian numerals take three noun forms. The catalog was written and checked by a native speaker; Croatian proofing is a separate change.
Estonian uses the existing fonts, follows `et-EE` system locales, and is available in Options or
through `ui.language` with `{"value":"et"}`. Its 1,307 entries cover the existing interface catalog.
Estonian spelling, grammar, dates and document templates are not added by the interface catalog.
Turkish works the same way (`tr-TR` and `tr-CY` system locales, `{"value":"tr"}`) with all 2,325
entries, the same keys as the Russian catalog. Command search folds the Turkish dotted and dotless I (`italik` finds İtalik, `YAZI TİPİ`
finds Yazı Tipi) while English labels and ASCII command ids keep their usual matching. Turkish
proofing is not included; terminology and checks: [`localization-tr.md`](localization-tr.md).
Bulgarian (`bg-BG` system locales, `{"value":"bg"}`) has 2,419 entries: every key of the reference
catalog, the equation gallery's labels and everything the interface audit finds (the audit checks
Bulgarian as it does Russian). Counts use count-neutral wording (`Думи: 21`). Bulgarian text is
hyphenated by the Bulgarian syllable rules; Bulgarian spelling and grammar are not included.
Hungarian follows `hu-HU` system locales and `{"value":"hu"}`, with a complete catalog of 2,419 entries: every key of
the Russian catalog, built-in style names shown translated, plus the citation style names. Hungarian proofing is not included; terminology and conventions:
[`localization-hu.md`](localization-hu.md).

Czech follows `cs-CZ` system locales and `{"value":"cs"}`, with 2,447 entries: every key of the
Russian and Spanish catalogs, including built-in style names and numbered families, the equation
gallery, watermarks, paper sizes and preview tiles. The Czech coverage test
enforces both reference catalogs and retains the Chinese catalog coverage; counts keep the existing
count-neutral wording (`Počet slov: 21`). Czech proofing is not included.

Polish, Portuguese and Czech interfaces write new documents and opened TXT/Markdown text
in their proofing languages (`pl-PL`, `pt-BR`, `cs-CZ`).

**Interface audit (runs in CI).** `untranslated_interface_text` lays the app out in a pseudo-language
(every tab, File page, dialog, pane, the right-click menu, and a hover and a click on every ribbon
control) and fails on visible text that skipped the catalog; then Russian, Hungarian, Turkish and
Bulgarian must have every string it looked up. Proper names (citation styles such as APA or
GOST, LaTeX, Unicode) and `bad parameters:` errors, which name a command's JSON parameters for
scripts, stay English. Size labels (Envelopes, Labels, Layout › Size) are translated
patterns (`{name}   {height} × {width} in`) with the region's decimal separator; paper names
other than envelopes (Letter, Legal, A4, B5 (JIS)) stay as they are.

## Proofing per language

| Language | Spelling | Suggestions | Grammar | Hyphenation |
|---|---|---|---|---|
| English (`en`, any region; text with no language) | Moby word list (public domain, ~160k words) with inflection rules | edit distance | repeated word, `a`/`an`, sentence capital, space before punctuation, double space | Moby dictionary, our Liang patterns, heuristic |
| Russian (`ru`) | A Russian Hunspell dictionary installed on the system (`ru_RU` from the OS, a package manager, LibreOffice's `dict-ru` or the `WORDCRAFT_DICTIONARIES` folder; UTF-8, KOI8-R or Windows-1251; not bundled, so no Russian spelling marks without it), read by the same affix engine; `ё` optional (`елка`/`ёлка`, `еще`/`ещё`); hyphenated words whole, then by parts (`кто-то`, `по-русски`, `кое-где`, `северо-западный`, `ИИ-консультант`); solid compounds (`высокоэластичный`), `не` with adjectives, participles and verbal nouns (not verbs), productive prefixes (`ультра-`, `эко-`) and prefixed verbs; abbreviations with endings (`ГИПа`) and camel case left alone; our supplement of ~200 modern words with their declensions (`assets/spelling/ru-extra.txt`: `маркетплейс`, `промокод`, `интеграция`, `колеровать`) | `ё`/`е` first, then slips within letter groups (unstressed vowels, voiced and voiceless pairs, `ь`/`ъ`), the common confusions (`жы`, `чя`, `тся`/`ться`, doubled consonants, unpronounced consonants: `здраствуй` → `здравствуй`), a missing space (`незнаю` → `не знаю`; not a preposition before a verb), one typo, then a stem scan | Russian messages: repeated word, space before punctuation, double space, sentence capital (knows `т. е.`, `г.`, `руб.`, `ул.`, initials, numbers, dialogue dashes), possibly missing comma before `что`/`чтобы`/`если`/`хотя`/`поскольку`/`который` and before `потому что` (not after conjunctions and particles such as `и`, `только`, `даже`, `так`, `для того`; not after a preposition; not in `хотя бы`, `что-то`), doubled punctuation | The rules of Russian word division as code (no pattern file): a vowel on each side, two letters at least, `ь`/`ъ`/`й` kept with the letter before, a break after `ъ`, a consonant cluster divided after its first consonant |
| Polish (`pl`) | The Hunspell `pl_PL` dictionary installed on the system (OS package, Homebrew, LibreOffice, the folder set in File › Options › Proofing › Dictionaries folder, or `WORDCRAFT_DICTIONARIES`; not bundled, so no Polish spelling marks without it; the same pane shows which file each language found), read at runtime by a Hunspell-style affix engine (`crates/proof/src/affix.rs`); names capitalized, compounds part by part, `Kennedy'ego` | missing diacritics first (`zolw` → `żółw`), the dictionary's confusion table (`żeka` → `rzeka`, `chuśtawka` → `huśtawka`), missing space (`napewno` → `na pewno`), one typo, then a stem scan within two typos | Polish messages: repeated word, space before punctuation, double space, sentence capital (knows `np.`, `r.`, initials, ordinals, dialogue dashes), missing comma before `że`/`który`/`ponieważ`/`gdyż`/`aby`/`żeby`/`jeśli`/`jeżeli`/`gdyby`/`lecz` (not after `mimo`, `chyba`, `tak`, prepositions…), `we`/`ze` (`we wtorek`, `ze szkoły`), doubled punctuation | TeX Polish patterns (Kołodziejska, Jackowski, Ryćko; MIT option) with their 20 exception words; a Polish syllable heuristic for letters outside the alphabet |
| Portuguese (`pt`, any region) | The Hunspell `pt_BR` dictionary installed on the system — VERO and its derivatives (same places as Polish; not bundled, LGPL/MPL), read at runtime by the same affix engine, which reads its `FLAG UTF-8`, 106 flags, continuation classes and `FORBIDDENWORD` spellings; names capitalized, compounds part by part (`guarda-roupa`, `ex-marido`) | missing accents first (`coraçao` → `coração`, `saude` → `saúde`), the dictionary's confusion table (`seje` → `seja`, `excessão` → `exceção`), missing space (`nãoé` → `não é`), one typo, then an accent-folding stem scan | Portuguese messages: repeated word (the emphatic doubles are allowed), space before punctuation, double space, sentence capital (knows `etc.`, `pág.`, `Dr.`, initials, ordinals, dialogue dashes), doubled punctuation; no comma rule — `que` opens a clause only sometimes | TeX Portuguese patterns (Rezende, Almeida, Araujo, Benevides; BSD 3-clause) with their 2 exception words |
| Czech (`cs`, `cs-CZ`, `ces`, `cze`) | The system Hunspell `cs_CZ` dictionary, read at runtime (Debian/Ubuntu: `hunspell-cs`; the same system, LibreOffice and `WORDCRAFT_DICTIONARIES` search paths as Polish). No dictionary data is bundled; without an installed dictionary spelling stays off | suggestions from the loaded dictionary's MAP/REP/TRY/KEY tables, missing spaces and a stem scan; short-word splits rank after deleting an extra letter; NOSUGGEST stems and their derived forms are accepted when typed but excluded from suggestions | Czech messages (including Czech quotation marks) for repeated words, space before punctuation and double spaces only; digit-space-colon-space-digit ratios keep their spaces; no Czech-specific grammar rules | existing English fallback, as for `Other`; no Czech patterns bundled (the TeX/LibreOffice Czech patterns are GPL-2) |
| Italian (`it`, any region) | The Hunspell `it_IT` dictionary installed on the system (same places as Polish and Portuguese; not bundled — the upstream file is GPL-licensed), read at runtime by the same affix engine; elided forms check the word after the apostrophe (`l'amico`, `un'idea`, `c'è`, `dov'è`, `senz'altro`), truncations keep their apostrophe (`po'`, `mo'`), hyphenated compounds part by part | missing accents first (`caffe` → `caffè`, `perche` → `perché`), the dictionary's confusion table, missing space (`perme` → `per me`), one typo, then an accent-folding stem scan | Italian messages: repeated word (emphatic doubles like `piano piano` are allowed), space before punctuation, double space, sentence capital (knows `ecc.`, `dott.`, `prof.`, initials, ordinals), doubled punctuation, `un pò` → `un po'`, `qual'è` → `qual è`, `E'` → `È`, `un'altro` → `un altro` and `un altra` → `un'altra` (gender from the ending, masculine `-ista`/`-ma`/`-cida`… left alone); these work without a dictionary; no comma rule | TeX Italian patterns (Beccari; MIT option), no exception words |
| Turkish (`tr`, any region) | The Hunspell `tr_TR` dictionary installed on the system (same places as Polish; not bundled), read at runtime; proper nouns capitalized, apostrophe suffixes (`Ankara'da`, `Etch'ta`) and hyphenated compounds | missing diacritics / ASCII restoration first (`turkce` → `Türkçe`, `isik` → `ışık`), common confusion table (`yalnış` → `yanlış`, `herşey` → `her şey`), detached suffixes (`bende` → `ben de`, `geldinmi` → `geldi mi`), one typo | Turkish messages: repeated word (idiomatic reduplications allowed: `yavaş yavaş`), space before punctuation, double space, sentence capital (knows `vb.`, `vs.`, `prof.`, `dr.`), no comma before/after conjunctions (`ve`, `veya`, `de`), doubled punctuation | Turkish syllable rules (TDK rules); no external pattern file or word list needed |
| Indonesian (`id`, `id-ID`, `ind`, legacy `in`) | The system Hunspell `id_ID` dictionary, read at runtime (Debian/Ubuntu: `hunspell-id`; LibreOffice's `dict-id`; the same system, LibreOffice, Dictionaries folder and `WORDCRAFT_DICTIONARIES` search paths as Polish). No dictionary data is bundled; without an installed dictionary spelling stays off. Hyphenated reduplication is accepted whole (`kupu-kupu`) or by parts (`anak-anak`); hyphenated clitics after capitals and abbreviations (`KTP-nya`, `se-Indonesia`) are accepted | the same dictionary-driven ranking as Czech: MAP/REP/TRY/KEY tables of the loaded dictionary, missing spaces and a stem scan; nothing Indonesian hard-coded | Indonesian messages for repeated words (offering the hyphenated reduplication `anak-anak` first, then the single word), space before punctuation and double spaces only; no Indonesian-specific grammar rules | existing English fallback, as for `Other`; no Indonesian patterns bundled |
| Any other | — (no marks) | — | — | the English patterns, as before |

The dictionary file names each language looks for, the folders searched on each system and how to
install one (`pt_BR` as the example): [spelling-dictionaries.md](spelling-dictionaries.md).

Measured with the SJP.PL dictionary: every one of the ~4.5 million forms an independent expansion of its affix file
produces is accepted, and 200,000 one-edit mutations that aren't words are all rejected; a check
takes ~1 µs, a suggestion list 1–35 ms (release build). Not yet: Czech hyphenation patterns or Czech-specific grammar, other languages' dictionaries,
a thesaurus for Polish, Word's style and clarity refinements, Polish-specific AutoCorrect.

Measured with the LibreOffice `ru_RU` dictionary on a private corpus of 205 real-world Russian
business documents (354,025 Cyrillic words): the dictionary loads in ~0.1 s, the whole corpus is
checked in ~0.6 s, and 1.63% of the words are marked, almost all of them names of people, places
and companies (2.96% before the supplement and the compound rules). Not yet: a Russian thesaurus,
Russian AutoCorrect, agreement checks.

Italian (`it`) follows `it`, `it-IT` and `it-CH` system locales. Its catalog has every key of
the others plus 51 strings that are wrapped in `tl!` but were in no catalog yet (File › Info
properties, the File › Home greeting, list levels, Equation tab gallery and tooltips). Ribbon tabs,
groups and command names use the names Italian Word users know (feature names, as `AGENTS.md`
allows); all other text is written from the meaning of the English. The interface audit also
checks the Italian catalog (2026-10-14). Italian proofing is in the proofing table above.
allows); all other text is written from the meaning of the English. Italian proofing: see the table above.

A native speaker reviewed the catalog on 2026-10-14 (wording fixes, consistent terms).
## Sorting and numbers in the text's language

Sorts follow the alphabet of the text's language (`w:lang`, else the document default):
Table › Sort, Home › Sort, the index and its letter headings, the bibliography and the Table of
Authorities. Letters with diacritics sort with their base letter (`é` with `e`) unless the
language makes them letters of their own: Polish `ą ć ę ł ń ó ś ź ż`, Czech/Slovak `č ch ř š ž`
(and Slovak `ä ô`), Hungarian `cs dz dzs gy ly ny sz ty zs ö ü`, Swedish/Finnish `å ä ö` and
Danish/Norwegian `æ ø å` after `z`, Spanish `ñ`, Turkish, Croatian/Bosnian/Serbian Latin,
Romanian, Estonian, Latvian, Lithuanian, Slovenian and Icelandic (`crates/doc/src/collate.rs`,
a hand-written per-language alphabet table; full CLDR collation is not used). Table sort and
table formulas read numbers with the language's decimal and group separators (`1 250,50` in
Polish, `1.250,50` in German, `1,250.50` in English) and write formula results with its decimal
separator (`crates/doc/src/locale_number.rs`). Interface lists (styles, fonts) still sort by code
point.

## Region conventions

As in Word, a new blank document takes its paper from the system **region**, not the interface
language: US Letter in the territories CLDR lists for it (US, Canada, Mexico, the Philippines,
Chile, Colombia, Venezuela and a few Central American ones), A4 everywhere else. Lengths are shown
in inches in the US (and Liberia, Myanmar) and centimetres elsewhere, written with the region's
decimal separator (`2,54 cm` in Poland, Germany, France…); fields take either separator and any
unit (`1 in` in a centimetre field). An explicit File › Options › Advanced › Measurement units
choice still wins; its Automatic entry follows the region again.

Where the region comes from:
- **File › Options › Advanced › Region** (saved; agents: `file.options {"region": "pl-PL"}`, `auto`
  returns to the system's);
- else `WORDCRAFT_REGION` or `LC_ALL`;
- else, on Unix, each convention from its own locale: numbers from `LC_NUMERIC` (else `LANG`),
  paper from `LC_PAPER`, measurement from `LC_MEASUREMENT`;
- else the platform's preferred locale: the macOS preferred locale (`en-PL`), the browser's
  language on the web, and on Windows the home location (Settings › Time & language › Region)
  with the display language. Word follows Windows' regional format instead, so an en-US format
  set by someone living in Germany gives A4 and a decimal comma here; reading the regional format
  needs Win32 calls that are `unsafe` in Rust, which Etch forbids.

Runs without preferences (`WORDCRAFT_NO_PREFS`) stay `en-US` unless `WORDCRAFT_REGION` is set, so
agents and screenshots give the same result on every machine. The decimal separator belongs to
the language's own number format (CLDR): a decimal-comma language uses a comma everywhere except
where CLDR says otherwise (`de-CH`), and English, whose `en-XX` locales follow the territory, uses a
comma in decimal-comma territories (`en-DE`, `en-PL`). Other decimal-point languages keep the point
in any territory (`ja-PL` is `2.54 cm`), as CLDR's fallback to the language gives.

Templates (Letter, Resume, Report) keep their own size. Not yet: Word's "Default paper size"
preference of its own, and Word's locale-specific Normal margins (2,5 cm in some regions;
Etch keeps 2,54 cm).

## Shared work

| Item | Notes |
|---|---|
| Wrap the remaining English literals in `tl!` and translate them in every catalog | Makes the six partial languages full except script items |
| Mirrored (right-to-left) interface for Arabic and Hebrew | Ribbon, panes, dialogs, rulers |
| Vertical text, ruby, document grid (Chinese, Japanese, Korean) | |
| Proofing dictionaries, grammar and hyphenation for the 11 non-English key languages | Open dictionaries only (licence check per dictionary); issues. Per-run languages, the Hunspell-style affix engine and the TeX pattern reader are shared, so a language with an open Hunspell dictionary and hyphenation patterns is mostly data plus grammar rules |
| Native-speaker review of every catalog | Swedish: terminology and the running app reviewed (2026-10-13); Italian reviewed (2026-10-14); the others not yet |
