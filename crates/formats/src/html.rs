//! HTML.
//!
//! Import is a tolerant tag-soup reader (no DOM): a tokenizer plus a stack of open elements with
//! HTML's implicit closes for `p`, `li`, `tr`, `td`/`th` and headings. It understands `p`, `div`,
//! `h1`–`h6`, `b`/`strong`, `i`/`em`, `u`, `s`/`strike`/`del`, `sup`/`sub`, `code`/`kbd`, `a`
//! (links and `id`/`name` anchors), `br`, `hr`, `ul`/`ol`/`li` (nested), `table`/`tr`/`td`/`th`
//! (colspan/rowspan), `img` (`data:` URIs, other sources through a caller's [`ImageLoader`]), `blockquote`, `pre`, `font`, and the CSS
//! properties `color`, `background-color`, `font-weight`, `font-style`, `font-size`,
//! `font-family`, `text-decoration`, `vertical-align`, `text-align`, `white-space` and page
//! breaks. Scripts and styles are skipped; nesting is capped.
//!
//! Export writes clean semantic HTML with inline CSS and pictures as `data:` URIs.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use wordcraft_doc::section::NumFormat;
use wordcraft_doc::{Align, Document};

use crate::model::{
    self, Cell, FBlock, FTable, Flow, Fmt, Inline, Kind, ListInfo, Meta, Note, Para, Spacing, base64_encode, data_uri, make_img, mime_of, parse_color,
};

/// Open elements kept at most (deeper start tags are ignored).
const MAX_STACK: usize = 256;

// ---------------------------------------------------------------------------------------------
// Entities and decoding

/// Decode an entity body (`amp`, `#38`, `#x26`) to its text.
pub fn decode_entity(name: &str) -> Option<String> {
    if let Some(num) = name.strip_prefix('#') {
        let v = if let Some(h) = num.strip_prefix(['x', 'X']) { u32::from_str_radix(h, 16).ok()? } else { num.parse::<u32>().ok()? };
        let c = match v {
            0 => '\u{FFFD}',
            0x80..=0x9F => crate::txt::cp1252(v as u8),
            _ => char::from_u32(v).unwrap_or('\u{FFFD}'),
        };
        return Some(c.to_string());
    }
    let c = match name {
        "amp" | "AMP" => '&',
        "lt" | "LT" => '<',
        "gt" | "GT" => '>',
        "quot" | "QUOT" => '"',
        "apos" | "APOS" => '\'',
        "nbsp" | "NBSP" => '\u{00A0}',
        "ensp" => '\u{2002}',
        "emsp" => '\u{2003}',
        "thinsp" => '\u{2009}',
        "shy" => '\u{00AD}',
        "copy" | "COPY" => '©',
        "reg" | "REG" => '®',
        "trade" | "TRADE" => '™',
        "hellip" => '…',
        "mdash" => '—',
        "ndash" => '–',
        "minus" => '−',
        "lsquo" => '‘',
        "rsquo" => '’',
        "sbquo" => '‚',
        "ldquo" => '“',
        "rdquo" => '”',
        "bdquo" => '„',
        "laquo" => '«',
        "raquo" => '»',
        "bull" => '•',
        "middot" => '·',
        "deg" => '°',
        "plusmn" => '±',
        "times" => '×',
        "divide" => '÷',
        "frac12" => '½',
        "frac14" => '¼',
        "frac34" => '¾',
        "frac13" => '⅓',
        "frac23" => '⅔',
        "frac18" => '⅛',
        "frac38" => '⅜',
        "frac58" => '⅝',
        "frac78" => '⅞',
        "sup1" => '¹',
        "sup2" => '²',
        "sup3" => '³',
        "micro" => 'µ',
        "permil" => '‰',
        "prime" => '′',
        "Prime" => '″',
        "oline" => '‾',
        "frasl" => '⁄',
        "sect" => '§',
        "para" => '¶',
        "euro" | "EURO" => '€',
        "pound" | "POUND" => '£',
        "yen" | "YEN" => '¥',
        "cent" | "CENT" => '¢',
        "curren" => '¤',
        "dagger" => '†',
        "Dagger" => '‡',
        "larr" => '←',
        "rarr" => '→',
        "uarr" => '↑',
        "darr" => '↓',
        "harr" => '↔',
        "lArr" => '⇐',
        "rArr" => '⇒',
        "uArr" => '⇑',
        "dArr" => '⇓',
        "hArr" => '⇔',
        "crarr" => '↵',
        "forall" => '∀',
        "part" => '∂',
        "exist" => '∃',
        "empty" => '∅',
        "nabla" => '∇',
        "isin" => '∈',
        "notin" => '∉',
        "ni" => '∋',
        "prod" => '∏',
        "sum" => '∑',
        "radic" => '√',
        "prop" => '∝',
        "ang" => '∠',
        "and" => '∧',
        "or" => '∨',
        "cap" => '∩',
        "cup" => '∪',
        "int" => '∫',
        "there4" => '∴',
        "sim" => '∼',
        "cong" => '≅',
        "asymp" | "approx" => '≈',
        "ne" => '≠',
        "equiv" => '≡',
        "le" => '≤',
        "ge" => '≥',
        "sub" => '⊂',
        "sup" => '⊃',
        "nsub" => '⊄',
        "sube" => '⊆',
        "supe" => '⊇',
        "oplus" => '⊕',
        "otimes" => '⊗',
        "perp" => '⊥',
        "sdot" => '⋅',
        "infin" => '∞',
        "loz" => '◊',
        "spades" => '♠',
        "clubs" => '♣',
        "hearts" => '♥',
        "diams" => '♦',
        "check" => '✓',
        "iexcl" => '¡',
        "iquest" => '¿',
        "szlig" => 'ß',
        "auml" => 'ä',
        "ouml" => 'ö',
        "uuml" => 'ü',
        "Auml" => 'Ä',
        "Ouml" => 'Ö',
        "Uuml" => 'Ü',
        "eacute" => 'é',
        "egrave" => 'è',
        "ecirc" => 'ê',
        "euml" => 'ë',
        "Eacute" => 'É',
        "Egrave" => 'È',
        "Ecirc" => 'Ê',
        "Euml" => 'Ë',
        "aacute" => 'á',
        "agrave" => 'à',
        "acirc" => 'â',
        "atilde" => 'ã',
        "Agrave" => 'À',
        "Aacute" => 'Á',
        "Acirc" => 'Â',
        "Atilde" => 'Ã',
        "iacute" => 'í',
        "igrave" => 'ì',
        "icirc" => 'î',
        "iuml" => 'ï',
        "Iacute" => 'Í',
        "Igrave" => 'Ì',
        "Icirc" => 'Î',
        "Iuml" => 'Ï',
        "oacute" => 'ó',
        "ograve" => 'ò',
        "ocirc" => 'ô',
        "otilde" => 'õ',
        "Oacute" => 'Ó',
        "Ograve" => 'Ò',
        "Ocirc" => 'Ô',
        "Otilde" => 'Õ',
        "uacute" => 'ú',
        "ugrave" => 'ù',
        "ucirc" => 'û',
        "Uacute" => 'Ú',
        "Ugrave" => 'Ù',
        "Ucirc" => 'Û',
        "yacute" => 'ý',
        "yuml" => 'ÿ',
        "Yacute" => 'Ý',
        "ntilde" => 'ñ',
        "Ntilde" => 'Ñ',
        "ccedil" => 'ç',
        "Ccedil" => 'Ç',
        "aring" => 'å',
        "Aring" => 'Å',
        "oslash" => 'ø',
        "Oslash" => 'Ø',
        "aelig" => 'æ',
        "AElig" => 'Æ',
        "alpha" => 'α',
        "beta" => 'β',
        "gamma" => 'γ',
        "delta" => 'δ',
        "epsilon" => 'ε',
        "zeta" => 'ζ',
        "eta" => 'η',
        "theta" => 'θ',
        "iota" => 'ι',
        "kappa" => 'κ',
        "lambda" => 'λ',
        "mu" => 'μ',
        "nu" => 'ν',
        "xi" => 'ξ',
        "pi" => 'π',
        "rho" => 'ρ',
        "sigma" => 'σ',
        "tau" => 'τ',
        "upsilon" => 'υ',
        "phi" => 'φ',
        "chi" => 'χ',
        "psi" => 'ψ',
        "omega" => 'ω',
        "Gamma" => 'Γ',
        "Delta" => 'Δ',
        "Theta" => 'Θ',
        "Lambda" => 'Λ',
        "Xi" => 'Ξ',
        "Pi" => 'Π',
        "Sigma" => 'Σ',
        "Upsilon" => 'Υ',
        "Phi" => 'Φ',
        "Psi" => 'Ψ',
        "Omega" => 'Ω',
        "zwj" => '\u{200D}',
        "zwnj" => '\u{200C}',
        _ => return None,
    };
    Some(c.to_string())
}

/// Replace entity references in `s`.
pub fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(k) = rest.find('&') {
        out.push_str(&rest[..k]);
        let after = &rest[k + 1..];
        // An entity name is short; looking further would rescan the whole text for every stray `&`.
        let mut cut = after.len().min(33);
        while !after.is_char_boundary(cut) {
            cut -= 1;
        }
        let end = after.get(..cut).and_then(|w| w.find(';'));
        match end.and_then(|e| decode_entity(&after[..e]).map(|t| (t, e))) {
            Some((t, e)) => {
                out.push_str(&t);
                rest = &after[e + 1..];
            }
            None => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn sniff_charset(head: &str) -> Option<&str> {
    let (i, n) = head.find("charset").map(|i| (i, 7)).or_else(|| head.find("encoding").map(|i| (i, 8)))?;
    let after = head.get(i + n..)?.trim_start().strip_prefix('=')?;
    let trimmed = after.trim_start().trim_start_matches(['"', '\'']);
    let end = trimmed.find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ';' | '>' | '/')).unwrap_or(trimmed.len());
    let label = trimmed[..end].trim();
    if label.is_empty() { None } else { Some(label) }
}

/// Decode HTML bytes: BOM, then a `charset` in the first 2 KB, else UTF-8 with a legacy fallback.
pub fn decode(b: &[u8]) -> String {
    if b.starts_with(&[0xFF, 0xFE]) || b.starts_with(&[0xFE, 0xFF]) || b.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return crate::txt::decode(b);
    }
    let head = String::from_utf8_lossy(b.get(..b.len().min(2048)).unwrap_or(b)).to_ascii_lowercase();
    if let Some(label) = sniff_charset(&head)
        && let Some(s) = crate::txt::decode_as(b, label)
    {
        return s;
    }
    if let Ok(s) = std::str::from_utf8(b) {
        return s.to_string();
    }
    crate::txt::decode(b)
}

// ---------------------------------------------------------------------------------------------
// Tokenizer

#[derive(Debug)]
enum Tok {
    Text(String),
    Start { name: String, attrs: Vec<(String, String)>, self_close: bool },
    End(String),
}

struct Lexer<'a> {
    s: &'a str,
    pos: usize,
}

impl Lexer<'_> {
    fn rest(&self) -> &str {
        self.s.get(self.pos..).unwrap_or("")
    }

    /// Skip past the next occurrence of `pat` (ASCII case-insensitive), or to the end.
    fn skip_past(&mut self, pat: &str) -> usize {
        let start = self.pos;
        let hay = self.rest().as_bytes();
        let p = pat.as_bytes();
        let found = hay.windows(p.len().max(1)).position(|w| w.eq_ignore_ascii_case(p));
        match found {
            Some(k) => self.pos += k + p.len(),
            None => self.pos = self.s.len(),
        }
        found.map(|k| start + k).unwrap_or(self.s.len())
    }

    /// Skip past the next matching end tag `</tag...>` (ASCII case-insensitive), or to the end.
    fn skip_past_end_tag(&mut self, tag: &str) {
        let prefix = format!("</{tag}");
        let p = prefix.as_bytes();
        while self.pos < self.s.len() {
            let hay = self.rest().as_bytes();
            let found = hay.windows(p.len().max(1)).position(|w| w.eq_ignore_ascii_case(p));
            let Some(k) = found else {
                self.pos = self.s.len();
                return;
            };
            let after_tag = k + p.len();
            let next_byte = hay.get(after_tag);
            if next_byte.is_none_or(|b| b.is_ascii_whitespace() || *b == b'>' || *b == b'/') {
                self.pos += after_tag;
                self.skip_past(">");
                return;
            } else {
                self.pos += k + 2;
            }
        }
    }

    fn next(&mut self) -> Option<Tok> {
        let rest = self.rest();
        if rest.is_empty() {
            return None;
        }
        if !rest.starts_with('<') {
            let end = rest.find('<').unwrap_or(rest.len());
            let t = unescape(&rest[..end]);
            self.pos += end;
            return Some(Tok::Text(t));
        }
        let b = rest.as_bytes();
        if rest.starts_with("<!--") {
            self.pos += 4;
            self.skip_past("-->");
            return Some(Tok::Text(String::new()));
        }
        if rest.starts_with("<!") || rest.starts_with("<?") {
            self.skip_past(">");
            return Some(Tok::Text(String::new()));
        }
        if rest.starts_with("</") {
            let name: String = rest
                .strip_prefix("</")
                .unwrap_or("")
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == ':' || *c == '-')
                .map(|c| c.to_ascii_lowercase())
                .collect();
            if name.is_empty() {
                self.pos += 2;
                return Some(Tok::Text("</".into()));
            }
            self.skip_past(">");
            return Some(Tok::End(name));
        }
        if !b.get(1).is_some_and(|c| c.is_ascii_alphabetic()) {
            self.pos += 1;
            return Some(Tok::Text("<".into()));
        }
        // Start tag.
        let name: String =
            rest[1..].chars().take_while(|c| c.is_ascii_alphanumeric() || *c == ':' || *c == '-').map(|c| c.to_ascii_lowercase()).collect();
        let mut i = 1 + name.len();
        let mut attrs = Vec::new();
        let mut self_close = false;
        loop {
            while b.get(i).is_some_and(|c| c.is_ascii_whitespace()) {
                i += 1;
            }
            match b.get(i) {
                None => break,
                Some(b'>') => {
                    i += 1;
                    break;
                }
                Some(b'/') => {
                    if b.get(i + 1) == Some(&b'>') {
                        self_close = true;
                        i += 2;
                        break;
                    }
                    i += 1;
                    continue;
                }
                _ => {}
            }
            let a0 = i;
            while b.get(i).is_some_and(|c| !c.is_ascii_whitespace() && !matches!(c, b'=' | b'>' | b'/')) {
                i += 1;
            }
            let an = rest.get(a0..i).unwrap_or("").to_ascii_lowercase();
            if an.is_empty() {
                i += 1;
                continue;
            }
            while b.get(i).is_some_and(|c| c.is_ascii_whitespace()) {
                i += 1;
            }
            let mut val = String::new();
            if b.get(i) == Some(&b'=') {
                i += 1;
                while b.get(i).is_some_and(|c| c.is_ascii_whitespace()) {
                    i += 1;
                }
                match b.get(i) {
                    Some(&q) if q == b'"' || q == b'\'' => {
                        let v0 = i + 1;
                        let e = rest.get(v0..).and_then(|r| r.find(q as char)).map(|k| v0 + k).unwrap_or(rest.len());
                        val = unescape(rest.get(v0..e).unwrap_or(""));
                        i = e + 1;
                    }
                    _ => {
                        let v0 = i;
                        while b.get(i).is_some_and(|c| !c.is_ascii_whitespace() && *c != b'>') {
                            i += 1;
                        }
                        val = unescape(rest.get(v0..i).unwrap_or(""));
                    }
                }
            }
            if attrs.len() < 64 {
                attrs.push((an, val));
            }
        }
        self.pos += i.min(rest.len());
        Some(Tok::Start { name, attrs, self_close })
    }
}

// ---------------------------------------------------------------------------------------------
// Builder

#[derive(Clone, Copy, PartialEq, Debug)]
enum ElKind {
    Inline,
    Block,
    Heading(u8),
    Quote,
    Pre,
    List(bool),
    Li,
    Table,
    Tr,
    Cell,
    Head,
}

#[derive(Clone, Debug)]
struct El {
    name: String,
    kind: ElKind,
    fmt: Fmt,
    align: Option<Align>,
    preserve: bool,
    page_break: bool,
    page_break_after: bool,
    /// Spacing and indents from the element's own CSS (`margin-top`, `text-indent`…).
    spacing: Spacing,
    /// Li: a paragraph of this item was already emitted.
    used: bool,
    /// List: which list element this is (1-based), so items of one `<ol>` are one list.
    list: u32,
    /// List: `<ol start="N">`, given to its first item.
    start: Option<u32>,
    /// `dir="rtl"` (`Some(true)`) or `dir="ltr"` on this element.
    rtl: Option<bool>,
    /// The part this element plays in a document's footnote and endnote markup ([`NoteEl`]).
    note: NoteEl,
}

/// The footnote and endnote markup the reader takes, two vocabularies:
///
/// - Word's, as its web pages and its HTML clipboard carry it: a reference in the text is a
///   link `<a style="mso-footnote-id:ftn1" href="#_ftn1" name="_ftnref1">` around the mark; the
///   notes come after the body in `<div style="mso-element:footnote-list">`, each in
///   `<div style="mso-element:footnote" id="ftn1">` whose first paragraph (class
///   `MsoFootnoteText`) starts with the link back, `<a href="#_ftnref1" name="_ftn1">` around
///   the mark again. Endnotes the same with `endnote`, `edn` and `MsoEndnoteText`.
/// - Etch's own, plain HTML with DPUB-ARIA roles ([`notes_html`] writes it): a reference
///   `<a href="#fn1" id="fnref1" role="doc-noteref"><sup>1</sup></a>`; the notes in a
///   `<section role="doc-endnotes">` (class `endnotes` for endnotes), each an `li` with the id
///   the reference points at, holding the note's blocks and an `<a role="doc-backlink">` back
///   to the reference.
#[derive(Clone, Debug, PartialEq)]
enum NoteEl {
    None,
    /// The list of notes: what it holds besides the notes (a rule, Word's `<br>`) is dropped.
    /// `Some(endnotes)` for Etch's `<section role="doc-endnotes">`, whose `li` with an id are
    /// the notes; `None` for Word's list, whose notes say what they are.
    List(Option<bool>),
    /// A note's text, by its id (`ftn1`, `fn1`); whether it is an endnote.
    Def(String, bool),
    /// A reference in the text: the note's id, whether the link says it is an endnote, and the
    /// mark the reference shows, collected from the text inside the link.
    Ref {
        key: String,
        endnote: bool,
        mark: String,
    },
    /// The link back from a note's text to its reference: dropped with the mark inside it.
    Back,
}

/// Where an element being read sits in the note markup ([`note_element`]).
#[derive(Clone, Copy, Debug, PartialEq)]
enum NoteCtx {
    /// Outside any note and any list of notes.
    Body,
    /// Inside a note's text, where a note link is the link back.
    Def,
    /// In Etch's list of notes, outside any note's text: whether they are endnotes.
    List(bool),
}

/// Whether `attrs` give the element the ARIA role `role` (one of the tokens of `role`).
fn has_role(attrs: &[(String, String)], role: &str) -> bool {
    attr(attrs, "role").is_some_and(|v| v.split_ascii_whitespace().any(|t| t.eq_ignore_ascii_case(role)))
}

/// The part `name` with `attrs` plays in the note markup ([`NoteEl`]), sitting in `ctx`.
fn note_element(name: &str, attrs: &[(String, String)], ctx: NoteCtx) -> NoteEl {
    let in_def = ctx == NoteCtx::Def;
    // Etch's markup, by its DPUB-ARIA roles.
    if has_role(attrs, "doc-endnotes") {
        let endnotes = attr(attrs, "class").is_some_and(|c| c.split_ascii_whitespace().any(|t| t.eq_ignore_ascii_case("endnotes")));
        return NoteEl::List(Some(endnotes));
    }
    if let NoteCtx::List(endnote) = ctx
        && name == "li"
        && let Some(id) = attr(attrs, "id").map(str::trim).filter(|s| !s.is_empty())
    {
        return NoteEl::Def(id.to_ascii_lowercase(), endnote);
    }
    if name == "a" && in_def && has_role(attrs, "doc-backlink") {
        return NoteEl::Back;
    }
    if name == "a"
        && has_role(attrs, "doc-noteref")
        && let Some(key) = attr(attrs, "href").map(str::trim).and_then(|h| h.strip_prefix('#')).filter(|k| !k.is_empty())
    {
        if in_def {
            return NoteEl::Back;
        }
        let key = key.to_ascii_lowercase();
        let endnote = key.starts_with("en");
        return NoteEl::Ref { key, endnote, mark: String::new() };
    }
    let style = attr(attrs, "style").unwrap_or("");
    let decl = |prop: &str| -> Option<String> {
        style.split(';').find_map(|d| {
            let (k, v) = d.split_once(':')?;
            k.trim().eq_ignore_ascii_case(prop).then(|| v.trim().to_ascii_lowercase())
        })
    };
    if let Some(el) = decl("mso-element") {
        match el.as_str() {
            "footnote-list" | "endnote-list" => return NoteEl::List(None),
            "footnote" | "endnote" => {
                if let Some(id) = attr(attrs, "id").or_else(|| attr(attrs, "name")).map(str::trim).filter(|s| !s.is_empty()) {
                    return NoteEl::Def(id.to_ascii_lowercase(), el == "endnote");
                }
            }
            _ => {}
        }
    }
    if name != "a" {
        return NoteEl::None;
    }
    // The link's target, `#_ftn1` (a reference) or `#_ftnref1` (the link back), as the note's
    // id, whether it is an endnote, and whether it points at a reference.
    let target = attr(attrs, "href").map(str::trim).and_then(|h| h.strip_prefix("#_")).and_then(|t| {
        let t = t.to_ascii_lowercase();
        let (pfx, rest) = t.split_at_checked(3)?;
        if !matches!(pfx, "ftn" | "edn") {
            return None;
        }
        let (back, n) = match rest.strip_prefix("ref") {
            Some(n) => (true, n),
            None => (false, rest),
        };
        (!n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())).then(|| (format!("{pfx}{n}"), pfx == "edn", back))
    });
    let own = attr(attrs, "name").or_else(|| attr(attrs, "id")).map(|s| s.trim().to_ascii_lowercase());
    let styled = decl("mso-footnote-id").map(|k| (k, false)).or_else(|| decl("mso-endnote-id").map(|k| (k, true)));
    if styled.is_none() && target.is_none() {
        return NoteEl::None;
    }
    let is_back = in_def
        || target.as_ref().is_some_and(|t| t.2)
        || own.as_deref().is_some_and(|o| {
            o.strip_prefix("_ftn").or_else(|| o.strip_prefix("_edn")).is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        });
    if is_back {
        return NoteEl::Back;
    }
    let (key, endnote) = match (styled, target) {
        (Some((k, e)), _) if !k.is_empty() => (k, e),
        (_, Some((k, e, _))) => (k, e),
        _ => return NoteEl::None,
    };
    NoteEl::Ref { key, endnote, mark: String::new() }
}

/// The mark a note of its own has, from the mark its reference showed: Word's numbers (`1`,
/// `[1]`, `i`, `a`) are the note's numbering and give none; anything else (`*`, `†`) is a custom
/// mark.
fn custom_mark(mark: &str) -> String {
    let m: String = mark.chars().filter(|c| !c.is_whitespace() && !c.is_control() && !matches!(c, '[' | ']')).take(10).collect();
    if m.chars().all(|c| c.is_ascii_alphanumeric()) { String::new() } else { m }
}

struct TableB {
    rows: Vec<Vec<Cell>>,
    row: Option<Vec<Cell>>,
    /// The table or one of its cells asks for borders.
    bordered: bool,
    /// Column widths from `<col>` elements (points), in order.
    widths: Vec<f32>,
}

/// Whether an element's `border` attribute or `style` declares a visible border.
fn declares_border(attrs: &[(String, String)], is_table: bool) -> bool {
    if is_table && attr(attrs, "border").is_some_and(|b| b.trim().parse::<u32>().map(|n| n > 0).unwrap_or(b.trim().is_empty())) {
        return true;
    }
    let Some(style) = attr(attrs, "style") else { return false };
    style.split(';').any(|decl| {
        let Some((name, value)) = decl.split_once(':') else { return false };
        let name = name.trim().to_ascii_lowercase();
        if !matches!(name.as_str(), "border" | "border-top" | "border-right" | "border-bottom" | "border-left") {
            return false;
        }
        let value = value.trim().to_ascii_lowercase();
        !value.is_empty() && !value.split_whitespace().any(|tok| matches!(tok, "none" | "hidden" | "0" | "0px" | "0pt"))
    })
}

/// Loads the bytes of an image an `img` names by a source other than a `data:` URI (a path
/// relative to the HTML file). `None` when it can't be found. A picture used more than once can
/// share one buffer.
pub type ImageLoader<'a> = &'a dyn Fn(&str) -> Option<Arc<Vec<u8>>>;

struct Builder<'r> {
    images: ImageLoader<'r>,
    containers: Vec<Vec<FBlock>>,
    cells: Vec<Cell>,
    tables: Vec<TableB>,
    stack: Vec<El>,
    para: Option<Para>,
    /// The current paragraph continues a list item (joins the previous paragraph).
    para_cont: bool,
    /// Pending page break from preceding elements with page-break-after or unconsumed break.
    pending_page_break: bool,
    /// Whether the current paragraph's first / last text was read with preserved white space
    /// (CSS `white-space: pre…`): those edge spaces are kept when the paragraph ends.
    pre_lead: Option<bool>,
    pre_tail: bool,
    meta: Meta,
    title: Option<String>,
    /// Paragraphs (and empty table cells, which get one) still allowed
    /// ([`wordcraft_doc::MAX_PARAGRAPHS`]); once spent no more are made.
    paras_left: usize,
    too_many_paras: bool,
    /// List elements seen ([`El::list`]).
    lists_seen: u32,
    /// The mark each note reference showed, by the note's id ([`NoteEl::Ref`]).
    note_refs: HashMap<String, String>,
    /// Each note's text by its id ([`NoteEl::Def`]): whether it is an endnote, and its blocks.
    note_defs: BTreeMap<String, (bool, Vec<FBlock>)>,
}

fn is_void(n: &str) -> bool {
    matches!(
        n,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
            | "basefont"
            | "frame"
    )
}

fn block_kind(n: &str) -> Option<ElKind> {
    Some(match n {
        "p" | "div" | "section" | "article" | "header" | "footer" | "main" | "nav" | "aside" | "figure" | "figcaption" | "address" | "dl" | "dt"
        | "dd" | "center" | "form" | "fieldset" | "body" | "html" | "caption" | "thead" | "tbody" | "tfoot" | "details" | "summary" | "legend"
        | "hgroup" => ElKind::Block,
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => ElKind::Heading(n.get(1..2).and_then(|d| d.parse().ok()).unwrap_or(1)),
        "blockquote" => ElKind::Quote,
        "pre" | "listing" | "xmp" | "plaintext" => ElKind::Pre,
        "ul" | "menu" | "dir" => ElKind::List(false),
        "ol" => ElKind::List(true),
        "li" => ElKind::Li,
        "table" => ElKind::Table,
        "tr" => ElKind::Tr,
        "td" | "th" => ElKind::Cell,
        "head" => ElKind::Head,
        _ => return None,
    })
}

/// Closes an open `p` (HTML's "close a p element" on block starts).
fn closes_p(n: &str) -> bool {
    matches!(
        n,
        "address"
            | "article"
            | "aside"
            | "blockquote"
            | "div"
            | "dl"
            | "fieldset"
            | "footer"
            | "form"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "header"
            | "hr"
            | "main"
            | "nav"
            | "ol"
            | "p"
            | "pre"
            | "section"
            | "table"
            | "ul"
            | "figure"
            | "center"
            | "dd"
            | "dt"
            | "li"
    )
}

/// A CSS length (or font-size keyword) in points; `None` when it is not one.
pub fn css_len_pt(v: &str) -> Option<f32> {
    let v = v.trim().to_ascii_lowercase();
    let num = |s: &str| s.trim().parse::<f32>().ok().filter(|x| x.is_finite());
    if let Some(x) = v.strip_suffix("pt") {
        num(x)
    } else if let Some(x) = v.strip_suffix("px") {
        num(x).map(|p| p * 0.75)
    } else if let Some(x) = v.strip_suffix("rem").or_else(|| v.strip_suffix("em")) {
        num(x).map(|p| p * 12.0)
    } else if let Some(x) = v.strip_suffix("in") {
        num(x).map(|p| p * 72.0)
    } else if let Some(x) = v.strip_suffix("cm") {
        num(x).map(|p| p * 72.0 / 2.54)
    } else if let Some(x) = v.strip_suffix("mm") {
        num(x).map(|p| p * 72.0 / 25.4)
    } else if let Some(x) = v.strip_suffix('%') {
        num(x).map(|p| p * 0.12)
    } else {
        match v.as_str() {
            "xx-small" => Some(7.0),
            "x-small" => Some(7.5),
            "small" => Some(10.0),
            "medium" => Some(12.0),
            "large" => Some(13.5),
            "x-large" => Some(18.0),
            "xx-large" => Some(24.0),
            _ => num(&v).map(|p| p * 0.75),
        }
    }
}

/// Apply a `style` attribute to formatting / block properties.
/// A CSS `line-height`: a number is a multiple, a length is exact, `normal` nothing.
fn css_line_height(v: &str) -> Option<wordcraft_doc::props::LineSpacing> {
    use wordcraft_doc::props::LineSpacing;
    let v = v.trim();
    if let Some(p) = v.strip_suffix('%') {
        return p.trim().parse::<f32>().ok().filter(|p| p.is_finite() && *p > 0.0).map(|p| LineSpacing::Multiple((p / 100.0).clamp(0.06, 132.0)));
    }
    if let Ok(m) = v.parse::<f32>() {
        return (m.is_finite() && m > 0.0).then(|| LineSpacing::Multiple(m.clamp(0.06, 132.0)));
    }
    css_len_pt(v).filter(|pt| *pt > 0.0).map(|pt| LineSpacing::Exactly(pt.clamp(1.0, 1584.0)))
}

/// Inline CSS onto the formatting, alignment, white-space handling, page break and paragraph
/// spacing and indents of an element.
fn apply_style(
    css: &str,
    f: &mut Fmt,
    align: &mut Option<Align>,
    preserve: &mut bool,
    page_break: &mut bool,
    page_break_after: &mut bool,
    spacing: &mut Spacing,
) {
    for decl in css.split(';') {
        let Some((k, v)) = decl.split_once(':') else { continue };
        let pt = |v: &str| css_len_pt(v).map(|p| p.clamp(-1584.0, 1584.0));
        match k.trim().to_ascii_lowercase().as_str() {
            "margin-top" => spacing.before = pt(v).map(|p| p.max(0.0)),
            "margin-bottom" => spacing.after = pt(v).map(|p| p.max(0.0)),
            "margin-left" => spacing.left = pt(v),
            "margin-right" => spacing.right = pt(v),
            "text-indent" => spacing.first = pt(v),
            "line-height" => spacing.line = css_line_height(v),
            _ => {}
        }
    }
    for decl in css.split(';').take(64) {
        let Some((k, v)) = decl.split_once(':') else { continue };
        let k = k.trim().to_ascii_lowercase();
        let v = v.trim().trim_end_matches("!important").trim();
        let lv = v.to_ascii_lowercase();
        match k.as_str() {
            "font-weight" => f.bold = lv.starts_with("bold") || lv.parse::<u32>().is_ok_and(|w| w >= 600),
            "font-style" => f.italic = lv == "italic" || lv == "oblique",
            "text-decoration" | "text-decoration-line" => {
                if lv.contains("none") {
                    f.underline = false;
                    f.strike = false;
                }
                if lv.contains("underline") {
                    f.underline = true;
                }
                if lv.contains("line-through") {
                    f.strike = true;
                }
            }
            "color" => {
                if let Some(c) = parse_color(v) {
                    f.color = Some(c);
                }
            }
            "background-color" | "background" => {
                if let Some(c) = v.split_whitespace().find_map(parse_color) {
                    f.background = Some(c);
                }
            }
            "font-size" => {
                if let Some(p) = css_len_pt(v).filter(|p| *p >= 1.0 && *p <= 1638.0) {
                    f.size = Some(p);
                }
            }
            "font-family" => {
                let fam = v.split(',').next().unwrap_or("").trim().trim_matches(['"', '\'']).to_string();
                if model::is_mono(&fam) || fam.eq_ignore_ascii_case("monospace") {
                    f.code = true;
                } else if !fam.is_empty() && !matches!(fam.to_ascii_lowercase().as_str(), "serif" | "sans-serif" | "inherit" | "initial") {
                    f.font = Some(fam);
                }
            }
            "vertical-align" => {
                f.sup = lv == "super";
                f.sub = lv == "sub";
            }
            "text-align" => {
                *align = match lv.as_str() {
                    "center" => Some(Align::Center),
                    "right" | "end" => Some(Align::Right),
                    "justify" => Some(Align::Justify),
                    "left" | "start" => Some(Align::Left),
                    _ => *align,
                }
            }
            "white-space" => *preserve = lv.starts_with("pre"),
            "page-break-before" | "break-before" => *page_break = lv == "always" || lv == "page",
            "page-break-after" | "break-after" => *page_break_after = lv == "always" || lv == "page",
            _ => {}
        }
    }
}

fn attr<'a>(attrs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

impl<'r> Builder<'r> {
    fn new(images: ImageLoader<'r>, para_budget: usize) -> Builder<'r> {
        Builder {
            paras_left: para_budget,
            too_many_paras: false,
            lists_seen: 0,
            images,
            containers: vec![Vec::new()],
            cells: Vec::new(),
            tables: Vec::new(),
            stack: Vec::new(),
            para: None,
            para_cont: false,
            pending_page_break: false,
            pre_lead: None,
            pre_tail: false,
            meta: Meta::default(),
            title: None,
            note_refs: HashMap::new(),
            note_defs: BTreeMap::new(),
        }
    }

    /// Where the element about to open sits in the note markup ([`NoteCtx`]).
    fn note_ctx(&self) -> NoteCtx {
        if self.stack.iter().any(|e| matches!(e.note, NoteEl::Def(..))) {
            return NoteCtx::Def;
        }
        match self.stack.iter().rev().find_map(|e| if let NoteEl::List(l) = e.note { Some(l) } else { None }) {
            Some(Some(endnotes)) => NoteCtx::List(endnotes),
            _ => NoteCtx::Body,
        }
    }

    fn fmt(&self) -> Fmt {
        self.stack.last().map(|e| e.fmt.clone()).unwrap_or_default()
    }

    /// Count one more paragraph against the budget; false once it is spent.
    fn take_para(&mut self) -> bool {
        match self.paras_left.checked_sub(1) {
            Some(left) => {
                self.paras_left = left;
                true
            }
            None => {
                self.too_many_paras = true;
                false
            }
        }
    }

    fn preserve(&self) -> bool {
        self.stack.last().is_some_and(|e| e.preserve)
    }

    fn in_head(&self) -> bool {
        self.stack.iter().any(|e| e.kind == ElKind::Head)
    }

    /// Paragraph context from the open elements (innermost first, up to the enclosing cell).
    fn start_para(&mut self) -> &mut Para {
        if self.para.is_none() {
            let mut p = Para::default();
            let mut list_level: Option<(bool, usize)> = None;
            let mut li_at: Option<usize> = None;
            let mut lists = 0usize;
            p.rtl = self.stack.iter().rev().find_map(|e| e.rtl).unwrap_or(false);
            for (i, e) in self.stack.iter().enumerate().rev() {
                match e.kind {
                    ElKind::Cell => {
                        // CSS inherits `text-align` from the cell, its row, row group and table
                        // (#1381); the paragraph's own (found above) wins.
                        if p.align.is_none() {
                            p.align = self.stack.get(..=i).and_then(|outer| {
                                outer
                                    .iter()
                                    .rev()
                                    .take_while(|o| o.kind != ElKind::Table)
                                    .chain(outer.iter().rev().find(|o| o.kind == ElKind::Table))
                                    .find_map(|o| o.align)
                            });
                        }
                        break;
                    }
                    ElKind::Table => break,
                    ElKind::Heading(n) if p.kind == Kind::Normal => p.kind = Kind::Heading(n.clamp(1, 6)),
                    ElKind::Pre if p.kind == Kind::Normal => p.kind = Kind::Code,
                    ElKind::Quote if p.kind == Kind::Normal => p.kind = Kind::Quote,
                    ElKind::Li if li_at.is_none() => li_at = Some(i),
                    ElKind::List(o) => {
                        if list_level.is_none() {
                            list_level = Some((o, i));
                        }
                        lists += 1;
                    }
                    _ => {}
                }
                if p.align.is_none() && e.align.is_some() && e.kind != ElKind::Inline {
                    p.align = e.align;
                }
                if p.spacing == Spacing::default() && e.spacing != Spacing::default() && e.kind != ElKind::Inline {
                    p.spacing = e.spacing;
                }
            }
            if let (Some((o, at)), Some(li)) = (list_level, li_at) {
                let list = self.stack.get(at).map(|e| e.list);
                // `start` goes to the list's first item.
                let start = self.stack.get_mut(at).and_then(|e| e.start.take()).filter(|_| o);
                p.list = Some(ListInfo { ordered: o, level: lists.saturating_sub(1).min(8) as u8, list, start });
                self.para_cont = self.stack.get(li).is_some_and(|e| e.used);
                if let Some(e) = self.stack.get_mut(li) {
                    e.used = true;
                }
            } else {
                self.para_cont = false;
            }
            let pb_before = self.stack.iter_mut().rev().take_while(|e| e.kind != ElKind::Cell).any(|e| std::mem::take(&mut e.page_break));
            p.page_break = pb_before || std::mem::take(&mut self.pending_page_break);
            self.para = Some(p);
        }
        self.para.get_or_insert_with(Para::default)
    }

    fn container(&mut self) -> &mut Vec<FBlock> {
        if self.containers.is_empty() {
            self.containers.push(Vec::new());
        }
        let n = self.containers.len() - 1;
        &mut self.containers[n]
    }

    fn flush(&mut self) {
        let Some(mut p) = self.para.take() else { return };
        let (pre_lead, pre_tail) = (self.pre_lead.take().unwrap_or(false), std::mem::take(&mut self.pre_tail));
        if p.kind != Kind::Code {
            // Preserved white space keeps its spaces and tabs at the edges; line breaks there
            // (source formatting) still go.
            const ALL: &[char] = &[' ', '\t', '\r', '\n'];
            const BREAKS: &[char] = &['\r', '\n'];
            p.trim_with(if pre_lead { BREAKS } else { ALL }, if pre_tail { BREAKS } else { ALL });
            if p.text().trim().is_empty() {
                // Only white space (indentation between blocks): nothing to keep.
                p.trim();
            }
        }
        if p.inlines.len() == 1 && p.text() == "\u{00A0}" {
            // `<p>&nbsp;</p>`: an empty paragraph on purpose.
            p.inlines.clear();
            self.para_cont = false;
            if self.take_para() {
                self.container().push(FBlock::Para(p));
            }
            return;
        }
        if p.is_empty() && p.kind != Kind::Code && p.list.is_none() {
            // Whitespace-only text between blocks.
            return;
        }
        let cont = std::mem::take(&mut self.para_cont);
        let level = p.list.map(|l| l.level);
        let c = self.container();
        if cont
            && let Some(FBlock::Para(prev)) = c.last_mut()
            && prev.list.is_some()
            && prev.list.map(|l| l.level) == level
        {
            prev.push_text("\n", &Fmt::default());
            prev.inlines.extend(p.inlines);
            return;
        }
        if self.take_para() {
            self.container().push(FBlock::Para(p));
        }
    }

    fn text(&mut self, t: &str) {
        if self.in_head() {
            if let Some(title) = &mut self.title {
                title.push_str(t);
            }
            return;
        }
        if t.is_empty() {
            return;
        }
        // Inside a note's reference the text is the mark it shows; inside the link back, the
        // mark again (the note's own number starts its text when the document is made).
        if let Some(e) = self.stack.iter_mut().rev().find(|e| matches!(e.note, NoteEl::Ref { .. } | NoteEl::Back)) {
            if let NoteEl::Ref { mark, .. } = &mut e.note
                && mark.len() < 64
            {
                mark.push_str(t);
            }
            return;
        }
        let inside_table_only = matches!(
            self.stack.iter().rev().find(|e| matches!(e.kind, ElKind::Table | ElKind::Tr | ElKind::Cell)).map(|e| e.kind),
            Some(ElKind::Table | ElKind::Tr)
        );
        if inside_table_only && t.trim().is_empty() {
            return;
        }
        let code = self.stack.iter().any(|e| e.kind == ElKind::Pre);
        let f = self.fmt();
        if code {
            let mut first = true;
            for line in t.split('\n') {
                if !first {
                    // A newline right after <pre> is ignored.
                    let empty_start = self.para.as_ref().is_none_or(|p| p.inlines.is_empty()) && self.pre_fresh();
                    if !empty_start {
                        self.start_para();
                        self.flush();
                    }
                }
                first = false;
                let line = line.trim_end_matches('\r');
                if !line.is_empty() {
                    self.start_para().push_text(line, &Fmt { code: false, ..f.clone() });
                    self.mark_pre_used();
                }
            }
            return;
        }
        if self.preserve() {
            self.note_edge(true);
            self.start_para().push_text(t, &f);
            return;
        }
        // Collapse whitespace.
        let mut s = String::with_capacity(t.len());
        let mut prev_space = self.para.as_ref().is_none_or(|p| match p.inlines.last() {
            Some(Inline::Text(x, _)) => x.ends_with([' ', '\n']),
            Some(_) => false,
            None => true,
        });
        for c in t.chars() {
            if matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{000C}') {
                if !prev_space {
                    s.push(' ');
                    prev_space = true;
                }
            } else {
                s.push(c);
                prev_space = false;
            }
        }
        if s.is_empty() {
            return;
        }
        self.note_edge(false);
        self.start_para().push_text(&s, &f);
    }

    /// Record whether text about to be added keeps its white space (see `pre_lead`).
    fn note_edge(&mut self, preserved: bool) {
        if self.para.as_ref().is_none_or(|p| p.inlines.is_empty()) {
            self.pre_lead = Some(preserved);
        }
        self.pre_tail = preserved;
    }

    fn pre_fresh(&self) -> bool {
        self.stack.iter().rev().find(|e| e.kind == ElKind::Pre).is_some_and(|e| !e.used)
    }

    fn mark_pre_used(&mut self) {
        if let Some(e) = self.stack.iter_mut().rev().find(|e| e.kind == ElKind::Pre) {
            e.used = true;
        }
    }

    /// Pop elements down to and including index `k`.
    fn pop_to(&mut self, k: usize) {
        while self.stack.len() > k {
            let Some(e) = self.stack.pop() else { break };
            self.close(&e);
        }
    }

    fn close(&mut self, e: &El) {
        if e.page_break_after || e.page_break {
            self.pending_page_break = true;
        }
        if e.kind != ElKind::Inline {
            self.flush();
        }
        match &e.note {
            NoteEl::Ref { key, mark, .. } => {
                self.note_refs.entry(key.clone()).or_insert_with(|| mark.clone());
            }
            // (Both are block elements, so the paragraph in them was flushed above.)
            NoteEl::Def(key, endnote) => {
                let blocks = if self.containers.len() > 1 { self.containers.pop().unwrap_or_default() } else { Vec::new() };
                self.note_defs.entry(key.clone()).or_insert((*endnote, blocks));
            }
            NoteEl::List(_) => {
                if self.containers.len() > 1 {
                    self.containers.pop();
                }
            }
            NoteEl::Back | NoteEl::None => {}
        }
        match e.kind {
            ElKind::Cell => {
                self.flush();
                let blocks = if self.containers.len() > 1 { self.containers.pop().unwrap_or_default() } else { Vec::new() };
                let mut cell = self.cells.pop().unwrap_or_default();
                // An empty cell gets a paragraph of its own: it counts as one.
                if blocks.is_empty() && !self.take_para() {
                    return;
                }
                cell.blocks = blocks;
                if let Some(t) = self.tables.last_mut() {
                    t.row.get_or_insert_with(Vec::new).push(cell);
                }
            }
            ElKind::Tr => {
                if let Some(t) = self.tables.last_mut()
                    && let Some(r) = t.row.take()
                {
                    t.rows.push(r);
                }
            }
            ElKind::Table => {
                if let Some(mut t) = self.tables.pop() {
                    if let Some(r) = t.row.take() {
                        t.rows.push(r);
                    }
                    let borderless = !t.bordered;
                    // Keep at most MAX_ROWS rows (the model drops the rest anyway): bounds the
                    // O(rows × rowspan) work of `insert_covered` on a table of millions of `<tr>`.
                    let rows: Vec<_> = t.rows.into_iter().filter(|r| !r.is_empty()).collect();
                    if rows.len() > wordcraft_doc::table::MAX_ROWS {
                        refuse(|| format!("a table with more than {} rows", wordcraft_doc::table::MAX_ROWS));
                    }
                    let rows: Vec<_> = rows.into_iter().take(wordcraft_doc::table::MAX_ROWS).collect();
                    // Column widths count only when every column has one.
                    let widths = if t.widths.iter().all(|w| w.is_finite() && *w > 1.0) { std::mem::take(&mut t.widths) } else { Vec::new() };
                    let mut ft = FTable { rows, widths, borderless };
                    if !ft.rows.is_empty() {
                        ft.insert_covered();
                        self.container().push(FBlock::Table(ft));
                    }
                }
            }
            ElKind::Head => {}
            _ => {}
        }
    }

    /// Index of the innermost open element named `name`, not looking past a table boundary
    /// unless `cross_tables`.
    fn find_open(&self, pred: impl Fn(&El) -> bool, stop: impl Fn(&El) -> bool) -> Option<usize> {
        for (i, e) in self.stack.iter().enumerate().rev() {
            if pred(e) {
                return Some(i);
            }
            if stop(e) {
                return None;
            }
        }
        None
    }

    fn close_head_for_start(&mut self, name: &str) {
        // A body-content start tag implicitly ends an omitted </head>.
        if !matches!(
            name,
            "html" | "head" | "base" | "basefont" | "bgsound" | "link" | "meta" | "noframes" | "noscript" | "script" | "style" | "template" | "title"
        ) && let Some(k) = self.find_open(|e| e.kind == ElKind::Head, |_| false)
        {
            self.pop_to(k);
        }
    }

    fn start(&mut self, name: &str, attrs: &[(String, String)], self_close: bool) {
        // Head metadata.
        match name {
            "title" => {
                self.title = Some(String::new());
            }
            "meta" => {
                let n = attr(attrs, "name").unwrap_or("").to_ascii_lowercase();
                let c = attr(attrs, "content").unwrap_or("").to_string();
                match n.as_str() {
                    "author" => self.meta.author = c,
                    "description" => self.meta.description = c,
                    "keywords" => self.meta.keywords = c,
                    "subject" => self.meta.subject = c,
                    _ => {}
                }
                return;
            }
            _ => {}
        }
        let is_table_part = matches!(name, "tr" | "td" | "th");
        let in_table = self.tables.len() < model::MAX_DEPTH;
        // Implicit closes.
        if closes_p(name)
            && let Some(k) = self.find_open(|e| e.name == "p", |e| matches!(e.kind, ElKind::Cell | ElKind::Table | ElKind::Li | ElKind::Quote))
        {
            self.pop_to(k);
        }
        if matches!(name, "h1" | "h2" | "h3" | "h4" | "h5" | "h6") && self.stack.last().is_some_and(|e| matches!(e.kind, ElKind::Heading(_))) {
            let k = self.stack.len() - 1;
            self.pop_to(k);
        }
        if name == "li"
            && let Some(k) = self.find_open(|e| e.kind == ElKind::Li, |e| matches!(e.kind, ElKind::List(_) | ElKind::Table | ElKind::Cell))
        {
            self.pop_to(k);
        }
        if matches!(name, "dt" | "dd")
            && let Some(k) =
                self.find_open(|e| e.name == "dt" || e.name == "dd", |e| matches!(e.kind, ElKind::Table | ElKind::Cell) || e.name == "dl")
        {
            self.pop_to(k);
        }
        if is_table_part && !self.tables.is_empty() {
            if matches!(name, "td" | "th")
                && let Some(k) = self.find_open(|e| e.kind == ElKind::Cell, |e| matches!(e.kind, ElKind::Table | ElKind::Tr))
            {
                self.pop_to(k);
            }
            if name == "tr"
                && let Some(k) = self.find_open(|e| matches!(e.kind, ElKind::Tr | ElKind::Cell), |e| e.kind == ElKind::Table)
            {
                // Pop to the outermost of cell/row inside the current table.
                let mut kk = k;
                while kk > 0 && self.stack.get(kk - 1).is_some_and(|e| matches!(e.kind, ElKind::Tr | ElKind::Cell)) {
                    kk -= 1;
                }
                self.pop_to(kk);
            }
        }
        let ctx = if self.in_head() { NoteCtx::Body } else { self.note_ctx() };
        let note = if self.in_head() { NoteEl::None } else { note_element(name, attrs, ctx) };
        if name == "a"
            && note == NoteEl::None
            && !self.in_head()
            && let Some(id) = attr(attrs, "id").or_else(|| attr(attrs, "name")).filter(|s| !s.is_empty())
        {
            self.start_para().inlines.push(Inline::Anchor(id.to_string()));
        }
        if let NoteEl::Ref { key, endnote, .. } = &note {
            // Filled in when the document is finished ([`Builder::finish`]); the key is the
            // note's id until then.
            let placeholder = Note { endnote: *endnote, custom: key.clone(), blocks: Vec::new() };
            self.start_para().inlines.push(Inline::Note(Box::new(placeholder)));
        }
        // Element-specific work.
        match name {
            "br" => {
                if self.stack.iter().any(|e| e.kind == ElKind::Pre) {
                    self.start_para();
                    self.flush();
                } else {
                    let f = self.fmt();
                    self.start_para().push_text("\n", &f);
                }
                return;
            }
            "hr" => {
                self.flush();
                if self.take_para() {
                    self.container().push(FBlock::Para(Para::new(Kind::Rule)));
                }
                return;
            }
            "wbr" => {
                let f = self.fmt();
                self.start_para().push_text("\u{200B}", &f);
                return;
            }
            // `<col width>` (an attribute or CSS) gives the column's width.
            "col" => {
                if let Some(t) = self.tables.last_mut()
                    && t.widths.len() < wordcraft_doc::table::MAX_COLS
                {
                    let css = attr(attrs, "style").and_then(|st| {
                        st.split(';')
                            .find_map(|d| d.split_once(':').filter(|(k, _)| k.trim().eq_ignore_ascii_case("width")).and_then(|(_, v)| css_len_pt(v)))
                    });
                    let w = css.or_else(|| attr(attrs, "width").and_then(css_len_pt)).unwrap_or(0.0);
                    let span = attr(attrs, "span").and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(1).clamp(1, 63);
                    for _ in 0..span {
                        t.widths.push(w);
                    }
                }
                return;
            }
            "img" => {
                let alt = attr(attrs, "alt").unwrap_or("").to_string();
                let dim = |k: &str| attr(attrs, k).and_then(|v| v.trim().trim_end_matches("px").trim().parse::<f32>().ok()).map(|p| p * 0.75);
                let mut w = dim("width");
                let mut h = dim("height");
                if let Some(st) = attr(attrs, "style") {
                    for decl in st.split(';') {
                        if let Some((k, v)) = decl.split_once(':') {
                            match k.trim().to_ascii_lowercase().as_str() {
                                "width" => w = css_len_pt(v).or(w),
                                "height" => h = css_len_pt(v).or(h),
                                _ => {}
                            }
                        }
                    }
                }
                let img = attr(attrs, "src")
                    .and_then(|src| if src.trim_start().starts_with("data:") { data_uri(src).map(Arc::new) } else { (self.images)(src.trim()) })
                    .and_then(|d| make_img(d, w, h, &alt));
                let f = self.fmt();
                match img {
                    Some(im) => self.start_para().inlines.push(Inline::Image(im)),
                    None if !alt.is_empty() => self.start_para().push_text(&alt, &f),
                    None => {}
                }
                return;
            }
            _ => {}
        }
        if is_void(name) || self_close && block_kind(name).is_none() && !matches!(name, "a" | "span") {
            return;
        }
        if self.stack.len() >= MAX_STACK {
            return;
        }
        let mut kind = block_kind(name).unwrap_or(ElKind::Inline);
        if matches!(kind, ElKind::Table | ElKind::Tr | ElKind::Cell) && !in_table {
            kind = ElKind::Block;
        }
        if matches!(kind, ElKind::Tr | ElKind::Cell) && self.tables.is_empty() {
            kind = ElKind::Block;
        }
        // A list of notes and a note's text hold blocks of their own (Etch's note is an `li`,
        // and the `ol` holding Etch's notes is not one of the document's lists).
        if matches!(note, NoteEl::List(_) | NoteEl::Def(..)) && matches!(kind, ElKind::Inline | ElKind::Li) {
            kind = ElKind::Block;
        }
        if matches!(ctx, NoteCtx::List(_)) && matches!(kind, ElKind::List(_)) {
            kind = ElKind::Block;
        }
        if kind != ElKind::Inline {
            self.flush();
        }
        if matches!(note, NoteEl::List(_) | NoteEl::Def(..)) {
            self.containers.push(Vec::new());
        }
        let mut fmt = self.fmt();
        let mut align = None;
        let mut preserve = self.preserve();
        let mut page_break = false;
        let mut page_break_after = false;
        match name {
            // A header cell is bold, as in browsers and Word, unless its own CSS says otherwise.
            "b" | "strong" | "th" => fmt.bold = true,
            "i" | "em" | "cite" | "var" | "dfn" | "address" => fmt.italic = true,
            "u" | "ins" => fmt.underline = true,
            "s" | "strike" | "del" => fmt.strike = true,
            "code" | "kbd" | "tt" | "samp" => fmt.code = true,
            "sup" => fmt.sup = true,
            "sub" => fmt.sub = true,
            "mark" => fmt.background = Some(wordcraft_doc::Rgb(255, 255, 0)),
            "font" => {
                if let Some(c) = attr(attrs, "color").and_then(parse_color) {
                    fmt.color = Some(c);
                }
                if let Some(face) = attr(attrs, "face") {
                    let fam = face.split(',').next().unwrap_or("").trim().to_string();
                    if model::is_mono(&fam) {
                        fmt.code = true;
                    } else if !fam.is_empty() {
                        fmt.font = Some(fam);
                    }
                }
                if let Some(sz) = attr(attrs, "size") {
                    let s = sz.trim();
                    let val = if let Some(rest) = s.strip_prefix('+') {
                        rest.parse::<i32>().ok().map(|d| 3 + d)
                    } else if let Some(rest) = s.strip_prefix('-') {
                        rest.parse::<i32>().ok().map(|d| 3 - d)
                    } else {
                        s.parse::<i32>().ok()
                    };
                    if let Some(sz) = val {
                        fmt.size = Some([7.5, 10.0, 12.0, 13.5, 18.0, 24.0, 36.0].get((sz.clamp(1, 7) - 1) as usize).copied().unwrap_or(12.0));
                    }
                }
            }
            "a" => {
                if let Some(h) = attr(attrs, "href").filter(|h| wordcraft_doc::link::is_safe_href(h)) {
                    fmt.link = Some(h.trim().to_string());
                }
            }
            _ => {}
        }
        // `align` on a table is about where the table sits (not imported); it doesn't align the
        // text in its cells.
        if let Some(a) = attr(attrs, "align").filter(|_| kind != ElKind::Table) {
            align = match a.to_ascii_lowercase().as_str() {
                "center" | "middle" => Some(Align::Center),
                "right" => Some(Align::Right),
                "justify" => Some(Align::Justify),
                "left" => Some(Align::Left),
                _ => None,
            };
        }
        if name == "center" {
            align = Some(Align::Center);
        }
        let mut spacing = Spacing::default();
        if let Some(st) = attr(attrs, "style") {
            apply_style(st, &mut fmt, &mut align, &mut preserve, &mut page_break, &mut page_break_after, &mut spacing);
        }
        if kind == ElKind::Pre {
            preserve = true;
        }
        match kind {
            ElKind::Table => self.tables.push(TableB { rows: Vec::new(), row: None, bordered: declares_border(attrs, true), widths: Vec::new() }),
            ElKind::Tr => {
                if let Some(t) = self.tables.last_mut()
                    && let Some(r) = t.row.take()
                {
                    t.rows.push(r);
                }
            }
            ElKind::Cell => {
                let tb = self.find_open(|e| e.kind == ElKind::Tr, |e| e.kind == ElKind::Table).is_none();
                if let Some(t) = self.tables.last_mut()
                    && declares_border(attrs, false)
                {
                    t.bordered = true;
                }
                if tb && let Some(t) = self.tables.last_mut() {
                    // A cell without a row: start one.
                    if let Some(r) = t.row.take() {
                        t.rows.push(r);
                    }
                    t.row = Some(Vec::new());
                }
                let num = |k: &str| attr(attrs, k).and_then(|v| v.trim().parse::<u32>().ok()).map(|v| v.clamp(1, 63));
                let shading = attr(attrs, "bgcolor").and_then(parse_color).or(fmt.background);
                self.cells.push(Cell {
                    blocks: Vec::new(),
                    colspan: num("colspan").unwrap_or(1),
                    rowspan: attr(attrs, "rowspan").and_then(|v| v.trim().parse::<u32>().ok()).map(|v| v.clamp(1, 1000)).unwrap_or(1),
                    covered: false,
                    header: name == "th",
                    shading,
                    borders: None,
                });
                fmt.background = None;
                self.containers.push(Vec::new());
            }
            _ => {}
        }
        let rtl = attr(attrs, "dir").and_then(|d| match d.trim().to_ascii_lowercase().as_str() {
            "rtl" => Some(true),
            "ltr" => Some(false),
            _ => None,
        });
        let (list, start) = match kind {
            ElKind::List(ordered) => {
                self.lists_seen = self.lists_seen.saturating_add(1);
                let start = attr(attrs, "start").filter(|_| ordered).and_then(|v| v.trim().parse::<u32>().ok()).filter(|s| *s != 1);
                (self.lists_seen, start.map(|s| s.min(32_767)))
            }
            _ => (0, None),
        };
        self.stack.push(El {
            name: name.to_string(),
            kind,
            fmt,
            align,
            preserve,
            page_break,
            page_break_after,
            spacing,
            used: false,
            list,
            start,
            rtl,
            note,
        });
    }

    fn end(&mut self, name: &str) {
        if name == "title" {
            if let Some(t) = self.title.take() {
                self.meta.title = t.split_whitespace().collect::<Vec<_>>().join(" ");
            }
            return;
        }
        let crosses = matches!(name, "table");
        let target = if matches!(name, "td" | "th") {
            self.find_open(|e| e.kind == ElKind::Cell, |e| e.kind == ElKind::Table)
        } else if name == "tr" {
            self.find_open(|e| e.kind == ElKind::Tr, |e| e.kind == ElKind::Table)
        } else {
            self.find_open(|e| e.name == name, |e| !crosses && matches!(e.kind, ElKind::Table | ElKind::Cell))
        };
        if let Some(k) = target {
            self.pop_to(k);
        }
    }

    fn finish(mut self) -> (Flow, bool) {
        self.pop_to(0);
        self.flush();
        // Unclosed cells: fold remaining containers into the body.
        while self.containers.len() > 1 {
            let c = self.containers.pop().unwrap_or_default();
            self.container().extend(c);
        }
        let mut blocks = self.containers.pop().unwrap_or_default();
        if let Some(t) = self.title.take() {
            self.meta.title = t;
        }
        // Each reference gets its note's text; one whose note is missing keeps the mark it
        // showed, as text.
        let refs = std::mem::take(&mut self.note_refs);
        let mut defs = std::mem::take(&mut self.note_defs);
        model::resolve_notes(&mut blocks, &mut |n| {
            let key = std::mem::take(&mut n.custom);
            let mark = refs.get(&key).cloned().unwrap_or_default();
            match defs.remove(&key) {
                Some((endnote, text)) => {
                    n.endnote = endnote;
                    n.blocks = text;
                    n.custom = custom_mark(&mark);
                    None
                }
                None => Some(Inline::Text(mark.trim().to_string(), Fmt { sup: true, ..Default::default() })),
            }
        });
        // A note nothing refers to: its text goes after the body rather than being lost.
        for (_, (_, text)) in defs {
            blocks.extend(text);
        }
        (Flow { blocks, meta: self.meta, ..Default::default() }, self.too_many_paras)
    }
}

thread_local! {
    /// The first limit the HTML being read was found past: a file (or a paste) is refused with it
    /// ([`import_file`], [`import_whole`]), rather than read without what is over.
    static REFUSED: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

fn refuse(what: impl FnOnce() -> String) {
    REFUSED.with(|r| {
        let mut r = r.borrow_mut();
        if r.is_none() {
            *r = Some(what());
        }
    });
}

fn take_refused() -> Option<String> {
    REFUSED.with(|r| r.borrow_mut().take())
}

/// Parse HTML text into the flow model.
pub fn parse(s: &str) -> Flow {
    parse_with(s, &|_| None)
}

/// [`parse`], loading pictures that aren't `data:` URIs through `images`. Past
/// [`wordcraft_doc::MAX_PARAGRAPHS`] paragraphs no more are made (a file is refused instead,
/// see [`import_file`]).
pub fn parse_with(s: &str, images: ImageLoader) -> Flow {
    parse_budgeted(s, images, wordcraft_doc::MAX_PARAGRAPHS).0
}

/// [`parse_with`] making at most `para_budget` paragraphs, and whether the source had more.
fn parse_budgeted(s: &str, images: ImageLoader, para_budget: usize) -> (Flow, bool) {
    let _ = take_refused();
    let mut lx = Lexer { s, pos: 0 };
    let mut b = Builder::new(images, para_budget);
    let mut guard = 0usize;
    while let Some(t) = lx.next() {
        guard += 1;
        if guard > 50_000_000 {
            refuse(|| "more than 50,000,000 tags and text pieces".to_string());
            break;
        }
        match t {
            Tok::Text(t) => b.text(&t),
            Tok::Start { name, attrs, self_close } => {
                b.close_head_for_start(&name);
                if crate::mathml::local(&name) == "math" && !self_close {
                    let el = read_math(&mut lx, name, attrs);
                    if !b.in_head()
                        && let Some((linear, display)) = crate::mathml::read(&el)
                    {
                        b.start_para().inlines.push(Inline::Equation { linear, display });
                    }
                    continue;
                }
                if matches!(name.as_str(), "script" | "style" | "noscript" | "template" | "textarea") && !self_close {
                    lx.skip_past_end_tag(&name);
                    continue;
                }
                b.start(&name, &attrs, self_close);
            }
            Tok::End(name) => b.end(&name),
        }
    }
    b.finish()
}

/// An HTML file opened as a document onto `page`: [`import_on`], but a file of more paragraphs
/// than a document may have ([`wordcraft_doc::MAX_PARAGRAPHS`]) is refused, after making no more
/// than that many (each costs about 2.5 KB while read however short it is).
pub fn import_file(s: &str, images: ImageLoader, page: &wordcraft_doc::SectionProps) -> Result<Document, String> {
    import_file_budgeted(s, images, page, wordcraft_doc::MAX_PARAGRAPHS)
}

/// [`import_file`] making at most `para_budget` paragraphs (tests use a small budget).
pub(crate) fn import_file_budgeted(s: &str, images: ImageLoader, page: &wordcraft_doc::SectionProps, para_budget: usize) -> Result<Document, String> {
    let (flow, too_many) = parse_budgeted(s, images, para_budget);
    if too_many {
        return Err(format!("HTML: more than {para_budget} paragraphs"));
    }
    if let Some(what) = take_refused() {
        return Err(format!("HTML: limit exceeded: {what}"));
    }
    let mut page = page.clone();
    apply_page_rule(s, &mut page);
    Ok(model::to_doc_in(&flow, page))
}

/// The first `@page` rule's `size` and `margin` (CSS Paged Media), onto `page`: a named size
/// (`A4`, `letter`…) or two lengths, `landscape`/`portrait`, and one to four margin lengths.
/// Only the rule's declarations are looked at, and only when they are well formed.
fn apply_page_rule(s: &str, page: &mut wordcraft_doc::SectionProps) {
    // Only a style sheet's rule counts, not text that mentions one.
    let lower = s.to_ascii_lowercase();
    let mut from = 0;
    let mut rule = None;
    while let Some(st) = lower.get(from..).and_then(|r| r.find("<style")) {
        let start = from + st;
        let Some(gt) = lower.get(start..).and_then(|r| r.find('>')) else { break };
        let body_at = start + gt + 1;
        let end = lower.get(body_at..).and_then(|r| r.find("</style")).map(|e| body_at + e).unwrap_or(lower.len());
        if let Some(at) = lower.get(body_at..end).and_then(|css| css.find("@page")) {
            rule = Some(body_at + at);
            break;
        }
        from = end.max(body_at);
    }
    let Some(at) = rule else { return };
    let rest = s.get(at + 5..).unwrap_or("");
    let Some(open) = rest.find('{') else { return };
    // Nothing but white space or a selector between `@page` and the block.
    if rest.get(..open).is_some_and(|sel| sel.contains(['}', ';', '<'])) {
        return;
    }
    let body = rest.get(open + 1..).unwrap_or("");
    let Some(close) = body.find('}') else { return };
    let body = body.get(..close.min(2048)).unwrap_or("");
    for decl in body.split(';') {
        let Some((k, v)) = decl.split_once(':') else { continue };
        let words: Vec<&str> = v.split_whitespace().collect();
        match k.trim().to_ascii_lowercase().as_str() {
            "size" => {
                let mut landscape = None;
                let mut lengths = Vec::new();
                for w in &words {
                    match w.to_ascii_lowercase().as_str() {
                        "landscape" => landscape = Some(true),
                        "portrait" => landscape = Some(false),
                        "a3" => lengths = vec![841.89, 1190.55],
                        "a4" => lengths = vec![595.28, 841.89],
                        "a5" => lengths = vec![419.53, 595.28],
                        "b4" => lengths = vec![708.66, 1000.63],
                        "b5" => lengths = vec![498.9, 708.66],
                        "letter" => lengths = vec![612.0, 792.0],
                        "legal" => lengths = vec![612.0, 1008.0],
                        "ledger" => lengths = vec![792.0, 1224.0],
                        "auto" => {}
                        _ => {
                            if let Some(pt) = css_len_pt(w).filter(|p| (36.0..=4000.0).contains(p)) {
                                lengths.push(pt);
                            }
                        }
                    }
                }
                let (w, h) = match lengths.as_slice() {
                    [w] => (*w, *w),
                    [w, h] => (*w, *h),
                    _ => continue,
                };
                // A landscape page is written wide; `landscape` on a portrait size turns it.
                let (w, h) = if landscape == Some(true) && w < h { (h, w) } else { (w, h) };
                page.page_w = w;
                page.page_h = h;
                page.landscape = landscape.unwrap_or(w > h);
            }
            "margin" => {
                let v: Vec<f32> = words.iter().filter_map(|w| css_len_pt(w)).filter(|p| (0.0..=2000.0).contains(p)).collect();
                if v.len() != words.len() {
                    continue;
                }
                let (t, r, b, l) = match v.as_slice() {
                    [a] => (*a, *a, *a, *a),
                    [a, b] => (*a, *b, *a, *b),
                    [a, b, c] => (*a, *b, *c, *b),
                    [a, b, c, d] => (*a, *b, *c, *d),
                    _ => continue,
                };
                page.margin_top = t;
                page.margin_right = r;
                page.margin_bottom = b;
                page.margin_left = l;
            }
            _ => {}
        }
    }
}

/// [`import`] of HTML that must be taken whole (a paste): past a limit (more paragraphs than a
/// document may have, a table longer than a table may be, an equation too large) it is refused,
/// naming it, rather than taken in part.
pub fn import_whole(s: &str) -> Result<Document, String> {
    let (flow, too_many) = parse_budgeted(s, &|_| None, wordcraft_doc::MAX_PARAGRAPHS);
    if too_many {
        return Err(format!("HTML: more than {} paragraphs", wordcraft_doc::MAX_PARAGRAPHS));
    }
    if let Some(what) = take_refused() {
        return Err(format!("HTML: limit exceeded: {what}"));
    }
    Ok(model::to_doc(&flow))
}

/// A `<math>` element (#816): its tokens up to the matching end tag, as a tree. Nesting and size
/// are bounded; an unclosed element ends with the input.
fn read_math(lx: &mut Lexer, name: String, attrs: Vec<(String, String)>) -> crate::mathml::El {
    use crate::mathml::{El, Item, MAX_NODES, local};
    let mut stack: Vec<El> = vec![El { name, attrs, children: Vec::new() }];
    let mut count = 0usize;
    let close = |stack: &mut Vec<El>| {
        if stack.len() > 1
            && let Some(done) = stack.pop()
            && let Some(parent) = stack.last_mut()
        {
            parent.children.push(Item::El(done));
        }
    };
    while let Some(t) = lx.next() {
        count += 1;
        if count > MAX_NODES * 4 {
            // Over the cap: the file is refused (the rest of this element is skipped meanwhile,
            // so its tokens aren't read as HTML text).
            refuse(|| format!("an equation of more than {} parts", MAX_NODES * 4));
            skip_math(lx, t);
            break;
        }
        match t {
            Tok::Text(t) => {
                if let Some(top) = stack.last_mut()
                    && !t.is_empty()
                {
                    top.children.push(Item::Text(t));
                }
            }
            Tok::Start { name, attrs, self_close } => {
                let el = El { name, attrs, children: Vec::new() };
                if self_close || stack.len() > wordcraft_doc::math::MAX_DEPTH {
                    if let Some(top) = stack.last_mut() {
                        top.children.push(Item::El(el));
                    }
                } else {
                    stack.push(el);
                }
            }
            Tok::End(name) => {
                let name = local(&name).to_string();
                // Close up to the matching open element; a stray end tag is ignored.
                if let Some(k) = stack.iter().rposition(|e| local(&e.name) == name) {
                    if k == 0 {
                        break;
                    }
                    while stack.len() > k {
                        close(&mut stack);
                    }
                }
            }
        }
    }
    while stack.len() > 1 {
        close(&mut stack);
    }
    stack.pop().unwrap_or_default()
}

/// Skip tokens (starting with `first`) up to and including the `</math>` that closes the element
/// being read; nested `<math>` elements are counted. An unclosed element ends with the input.
fn skip_math(lx: &mut Lexer, first: Tok) {
    use crate::mathml::local;
    let mut depth = 0usize;
    let mut t = Some(first);
    while let Some(tok) = t {
        match tok {
            Tok::Start { name, self_close: false, .. } if local(&name) == "math" => depth = depth.saturating_add(1),
            Tok::End(name) if local(&name) == "math" => {
                if depth == 0 {
                    return;
                }
                depth = depth.saturating_sub(1);
            }
            _ => {}
        }
        t = lx.next();
    }
}

/// Parse HTML into a document.
pub fn import(s: &str) -> Document {
    model::to_doc(&parse(s))
}

/// [`import`], loading pictures that aren't `data:` URIs through `images`.
pub fn import_with(s: &str, images: ImageLoader) -> Document {
    model::to_doc(&parse_with(s, images))
}

/// [`import_with`] onto `page` (its size and margins), which tables size to: HTML gives no page
/// size, so the caller picks it (the region's paper, #1381).
pub fn import_on(s: &str, images: ImageLoader, page: &wordcraft_doc::SectionProps) -> Document {
    model::to_doc_in(&parse_with(s, images), page.clone())
}

// ---------------------------------------------------------------------------------------------
// Export

/// Escape text for HTML content and attribute values.
pub fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\u{000C}' | '\u{000E}' => {}
            _ => o.push(c),
        }
    }
    o
}

fn align_css(a: Option<Align>) -> Option<&'static str> {
    match a? {
        Align::Center => Some("text-align:center"),
        Align::Right => Some("text-align:right"),
        Align::Justify | Align::Distribute => Some("text-align:justify"),
        Align::Left => None,
    }
}

fn text_html(t: &str) -> String {
    let mut o = String::new();
    let mut prev_space = false;
    for c in t.chars() {
        match c {
            '\n' => {
                o.push_str("<br>");
                prev_space = true;
                continue;
            }
            '\t' => o.push_str("<span style=\"white-space:pre\">\t</span>"),
            ' ' if prev_space => o.push_str("&nbsp;"),
            _ => o.push_str(&esc(&c.to_string())),
        }
        prev_space = c == ' ';
    }
    o
}

/// What the writer keeps while it writes a document's blocks: the notes referenced so far,
/// written after the body ([`notes_html`]), and their numbers.
struct Writer {
    /// The notes referenced, in order.
    notes: Vec<NoteOut>,
    /// Footnotes and endnotes numbered so far (a note with a mark of its own takes no number).
    foot_numbered: u32,
    end_numbered: u32,
    /// How footnotes and endnotes are numbered: the document's settings, else the defaults.
    formats: [NumFormat; 2],
    /// Inside a note's text, where a reference writes only its mark (no note in a note).
    in_note: bool,
}

impl Default for Writer {
    fn default() -> Self {
        Writer { notes: Vec::new(), foot_numbered: 0, end_numbered: 0, formats: DEFAULT_NOTE_FORMATS, in_note: false }
    }
}

/// The default numbering of footnotes (1, 2, 3) and endnotes (i, ii, iii).
const DEFAULT_NOTE_FORMATS: [NumFormat; 2] = [NumFormat::Decimal, NumFormat::LowerRoman];

/// A note referenced in what was written.
struct NoteOut {
    endnote: bool,
    /// The note's place among its kind, notes with their own marks included: its ids (`fn3`,
    /// `fnref3`) and its item in the list of notes.
    id: u32,
    /// The mark shown: the number in the document's format, or the note's own mark.
    mark: String,
    /// The note has a mark of its own (no number).
    custom: bool,
    note: Note,
}

/// The document's numbering of footnotes and endnotes, for [`Writer::formats`].
fn note_formats(doc: &Document) -> [NumFormat; 2] {
    [doc.settings.footnote_format, doc.settings.endnote_format]
}

/// The ids of a note and of its reference: `fn3`/`fnref3`, endnotes `en3`/`enref3`.
fn note_ids(endnote: bool, id: u32) -> (String, String) {
    let pfx = if endnote { "en" } else { "fn" };
    (format!("{pfx}{id}"), format!("{pfx}ref{id}"))
}

/// The notes referenced in what was written, after the body, as plain HTML5 with the DPUB-ARIA
/// 1.1 roles (W3C) for notes; the reader takes it back ([`NoteEl`]). The footnotes, then the
/// endnotes, each kind in a section of its own:
///
/// ```html
/// <section role="doc-endnotes" class="footnotes" aria-label="Footnotes">
/// <hr>
/// <ol>
/// <li id="fn1">
/// <p>The note's text. <a href="#fnref1" role="doc-backlink" aria-label="Back to text">↩︎</a></p>
/// </li>
/// </ol>
/// </section>
/// ```
///
/// - `doc-endnotes` is DPUB-ARIA's role for a collection of notes after the text, on a section
///   holding a list; the notes are its list items, which take no role of their own (DPUB-ARIA
///   says not to put `doc-footnote` inside `doc-endnotes`, and ARIA in HTML allows no note role
///   on an `li`). The class tells footnotes from endnotes; the label names the landmark.
/// - Each item's id (`fn1`, endnotes `en1`) is the one its reference links to; its number is
///   the note's place among its kind. When the mark shown differs from the list's own number (a
///   mark of the note's own, endnotes in roman numerals, a number after a custom-marked note)
///   the item's `list-style-type` is that mark as a CSS string, so the list shows the mark the
///   text shows.
/// - The note's blocks follow as written in the body. The link back (`doc-backlink`, a hooked
///   arrow with the text variation selector, so it is not drawn as an emoji, and a label to
///   name it) ends the note's last paragraph after a space; when the note ends in something
///   else (a list, a table, a rule) or is empty, it is a paragraph of its own.
/// - A document without notes writes nothing here, and its style sheet stays as it was.
fn notes_html(w: &mut Writer, out: &mut String) {
    let notes = std::mem::take(&mut w.notes);
    w.in_note = true;
    for endnote in [false, true] {
        let kind: Vec<&NoteOut> = notes.iter().filter(|n| n.endnote == endnote).collect();
        if kind.is_empty() {
            continue;
        }
        let (class, label) = if endnote { ("endnotes", "Endnotes") } else { ("footnotes", "Footnotes") };
        out.push_str(&format!("<section role=\"doc-endnotes\" class=\"{class}\" aria-label=\"{label}\">\n<hr>\n<ol>\n"));
        for n in kind {
            let (note_id, ref_id) = note_ids(endnote, n.id);
            let style = if n.custom {
                format!(" style=\"list-style-type:{}\"", esc(&css_string(&format!("{} ", n.mark))))
            } else if n.mark != n.id.to_string() {
                format!(" style=\"list-style-type:{}\"", esc(&css_string(&format!("{}. ", n.mark))))
            } else {
                String::new()
            };
            out.push_str(&format!("<li id=\"{note_id}\"{style}>\n"));
            let back = format!("<a href=\"#{ref_id}\" role=\"doc-backlink\" aria-label=\"Back to text\">\u{21A9}\u{FE0E}</a>");
            let mut blocks = n.note.blocks.clone();
            // The note's text starts after its own mark in the document: no space before it.
            if let Some(FBlock::Para(first)) = blocks.first_mut() {
                first.trim_with(&[' ', '\t', '\n', '\u{00A0}'], &[]);
            }
            match blocks.split_last() {
                Some((FBlock::Para(last), rest)) if last.list.is_none() && !matches!(last.kind, Kind::Code | Kind::Quote | Kind::Rule) => {
                    blocks_html(rest, out, 0, w);
                    let html = para_html(last, w);
                    match html.rfind("</") {
                        Some(at) if !last.inlines.is_empty() => {
                            out.push_str(html.get(..at).unwrap_or(""));
                            out.push(' ');
                            out.push_str(&back);
                            out.push_str(html.get(at..).unwrap_or(""));
                        }
                        _ => {
                            out.push_str(&html);
                            out.push_str(&format!("\n<p>{back}</p>"));
                        }
                    }
                    out.push('\n');
                }
                _ => {
                    blocks_html(&blocks, out, 0, w);
                    out.push_str(&format!("<p>{back}</p>\n"));
                }
            }
            out.push_str("</li>\n");
        }
        out.push_str("</ol>\n</section>\n");
    }
    w.in_note = false;
}

/// `s` as a CSS string (`'…'`), quotes and backslashes escaped, control characters left out.
fn css_string(s: &str) -> String {
    let mut o = String::from("'");
    for c in s.chars() {
        match c {
            '\'' | '\\' => {
                o.push('\\');
                o.push(c);
            }
            c if c.is_control() => {}
            c => o.push(c),
        }
    }
    o.push('\'');
    o
}

/// Inline spans → HTML (a note reference as its mark; the notes themselves are not written).
pub fn inlines_html(inlines: &[Inline]) -> String {
    inlines_html_in(inlines, &mut Writer::default())
}

fn inlines_html_in(inlines: &[Inline], w: &mut Writer) -> String {
    let mut o = String::new();
    for i in inlines.iter() {
        match i {
            Inline::Text(t, f) => {
                let mut open = String::new();
                let mut close: Vec<&str> = Vec::new();
                // A script or other non-web target is written as plain text (`is_safe_href`).
                if let Some(l) = f.link.as_deref().filter(|l| wordcraft_doc::link::is_safe_href(l)) {
                    open.push_str(&format!("<a href=\"{}\">", esc(l)));
                    close.push("</a>");
                }
                let mut tag = |o: &str, c: &'static str| {
                    open.push_str(o);
                    close.push(c);
                };
                if f.bold {
                    tag("<strong>", "</strong>");
                }
                if f.italic {
                    tag("<em>", "</em>");
                }
                if f.underline {
                    tag("<u>", "</u>");
                }
                if f.strike {
                    tag("<s>", "</s>");
                }
                if f.sup {
                    tag("<sup>", "</sup>");
                } else if f.sub {
                    tag("<sub>", "</sub>");
                }
                if f.code {
                    tag("<code>", "</code>");
                }
                let mut css = Vec::new();
                if let Some(c) = f.color {
                    css.push(format!("color:#{}", c.hex()));
                }
                if let Some(c) = f.background {
                    css.push(format!("background-color:#{}", c.hex()));
                }
                if let Some(s) = f.size {
                    css.push(format!("font-size:{}pt", trim_num(s)));
                }
                if let Some(fam) = &f.font {
                    css.push(format!("font-family:'{}'", esc(&fam.replace('\'', ""))));
                }
                if !css.is_empty() {
                    open.push_str(&format!("<span style=\"{}\">", css.join(";")));
                    close.push("</span>");
                }
                o.push_str(&open);
                o.push_str(&text_html(t));
                for c in close.iter().rev() {
                    o.push_str(c);
                }
            }
            Inline::Image(img) => {
                o.push_str(&format!(
                    "<img src=\"data:{};base64,{}\" alt=\"{}\" width=\"{}\" height=\"{}\">",
                    mime_of(&img.ext),
                    base64_encode(&img.data),
                    esc(&img.alt),
                    (img.w / 0.75).round(),
                    (img.h / 0.75).round()
                ));
            }
            Inline::Anchor(a) => o.push_str(&format!("<a id=\"{}\"></a>", esc(a))),
            // No picture to show: the browser shows the alt text in its place.
            Inline::Figure(alt) => o.push_str(&format!("<img alt=\"{}\">", esc(alt))),
            // MathML, which browsers, Word and LibreOffice show as an equation (#816).
            Inline::Equation { linear, display } => o.push_str(&crate::mathml::linear_to_mathml(linear, *display)),
            // A reference in the text: its mark, linked to the note written after the body
            // ([`notes_html`]). Inside a note's text (a note has no notes) the mark alone.
            Inline::Note(n) => {
                if w.in_note {
                    if !n.custom.is_empty() {
                        o.push_str(&format!("<sup>{}</sup>", esc(&n.custom)));
                    }
                    continue;
                }
                let id = u32::try_from(w.notes.iter().filter(|x| x.endnote == n.endnote).count()).unwrap_or(u32::MAX).saturating_add(1);
                let custom = !n.custom.is_empty();
                let mark = if !custom {
                    let (numbered, fmt) = if n.endnote { (&mut w.end_numbered, w.formats[1]) } else { (&mut w.foot_numbered, w.formats[0]) };
                    *numbered = numbered.saturating_add(1);
                    fmt.format(*numbered)
                } else {
                    n.custom.clone()
                };
                let (note_id, ref_id) = note_ids(n.endnote, id);
                o.push_str(&format!("<a href=\"#{note_id}\" id=\"{ref_id}\" role=\"doc-noteref\"><sup>{}</sup></a>", esc(&mark)));
                w.notes.push(NoteOut { endnote: n.endnote, id, mark, custom, note: (**n).clone() });
            }
            // Only the ODT and RTF writers are given these (`model::from_doc_full`).
            Inline::CommentStart(_) | Inline::CommentEnd(_) | Inline::TextBox(_) | Inline::Field { .. } => {}
        }
    }
    o
}

fn trim_num(v: f32) -> String {
    let s = format!("{:.2}", v);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn para_attrs(p: &Para) -> String {
    let mut css: Vec<String> = Vec::new();
    if let Some(a) = align_css(p.align) {
        css.push(a.to_string());
    }
    if p.page_break {
        css.push("break-before:page;page-break-before:always".to_string());
    }
    let sp = &p.spacing;
    for (k, v) in
        [("margin-top", sp.before), ("margin-bottom", sp.after), ("margin-left", sp.left), ("margin-right", sp.right), ("text-indent", sp.first)]
    {
        if let Some(v) = v.filter(|v| v.is_finite()) {
            css.push(format!("{k}:{}pt", trim_num(v)));
        }
    }
    match sp.line {
        Some(wordcraft_doc::props::LineSpacing::Multiple(m)) if m.is_finite() => css.push(format!("line-height:{}", trim_num(m))),
        Some(wordcraft_doc::props::LineSpacing::Exactly(v)) | Some(wordcraft_doc::props::LineSpacing::AtLeast(v)) if v.is_finite() => {
            css.push(format!("line-height:{}pt", trim_num(v)))
        }
        _ => {}
    }
    let dir = if p.rtl { " dir=\"rtl\"" } else { "" };
    if css.is_empty() { dir.to_string() } else { format!("{dir} style=\"{}\"", css.join(";")) }
}

fn para_html(p: &Para, w: &mut Writer) -> String {
    para_html_as(p, p.kind, w)
}

/// [`para_html`] with the paragraph written as `kind`.
fn para_html_as(p: &Para, kind: Kind, w: &mut Writer) -> String {
    let body = inlines_html_in(&p.inlines, w);
    let attrs = para_attrs(p);
    match kind {
        Kind::Heading(n) => format!("<h{n}{attrs}>{body}</h{n}>", n = n.clamp(1, 6)),
        Kind::Title => format!("<h1{attrs}>{body}</h1>"),
        Kind::Rule => "<hr>".to_string(),
        _ if body.is_empty() => format!("<p{attrs}>&nbsp;</p>"),
        _ => format!("<p{attrs}>{body}</p>"),
    }
}

fn table_html(t: &FTable, out: &mut String, depth: usize, w: &mut Writer) {
    out.push_str("<table style=\"border-collapse:collapse\">\n");
    if !t.widths.is_empty() && t.widths.len() == t.cols() && t.widths.iter().all(|w| w.is_finite() && *w > 0.0) {
        out.push_str("<colgroup>");
        for w in &t.widths {
            out.push_str(&format!("<col style=\"width:{}pt\">", trim_num(*w)));
        }
        out.push_str("</colgroup>\n");
    }
    for row in &t.rows {
        out.push_str("<tr>");
        for c in row {
            if c.covered {
                continue;
            }
            let tag = if c.header { "th" } else { "td" };
            let mut attrs = String::new();
            if c.colspan > 1 {
                attrs.push_str(&format!(" colspan=\"{}\"", c.colspan));
            }
            if c.rowspan > 1 {
                attrs.push_str(&format!(" rowspan=\"{}\"", c.rowspan));
            }
            let mut css = vec!["padding:4px 6px".to_string(), "vertical-align:top".to_string()];
            if !t.borderless {
                css.insert(0, "border:1px solid #999".to_string());
            }
            if let Some(s) = c.shading {
                css.push(format!("background-color:#{}", s.hex()));
            }
            let single = match c.blocks.as_slice() {
                [FBlock::Para(p)] if p.kind == Kind::Normal && p.list.is_none() && !p.page_break => Some(p),
                _ => None,
            };
            if let Some(a) = single.and_then(|p| align_css(p.align)) {
                css.push(a.to_string());
            }
            out.push_str(&format!("<{tag}{attrs} style=\"{}\">", css.join(";")));
            match single {
                Some(p) => out.push_str(&inlines_html_in(&p.inlines, w)),
                None => {
                    out.push('\n');
                    blocks_html(&c.blocks, out, depth + 1, w);
                }
            }
            out.push_str(&format!("</{tag}>"));
        }
        out.push_str("</tr>\n");
    }
    out.push_str("</table>\n");
}

/// A table nested too deep to write as a table: its text, a paragraph per row with the cells'
/// paragraphs (and the tables inside them, read the same way, without recursing) separated by
/// spaces, so nothing is dropped.
fn flat_table_html(t: &FTable, out: &mut String) {
    for row in &t.rows {
        let mut line: Vec<String> = Vec::new();
        let mut stack: Vec<&FBlock> = row.iter().filter(|c| !c.covered).flat_map(|c| c.blocks.iter()).rev().collect();
        while let Some(b) = stack.pop() {
            match b {
                FBlock::Para(p) => {
                    let t = p.text();
                    if !t.trim().is_empty() {
                        line.push(t);
                    }
                }
                FBlock::Table(inner) => stack.extend(inner.rows.iter().flatten().filter(|c| !c.covered).flat_map(|c| c.blocks.iter()).rev()),
            }
        }
        if !line.is_empty() {
            out.push_str(&format!("<p>{}</p>\n", text_html(&line.join(" "))));
        }
    }
}

fn blocks_html(blocks: &[FBlock], out: &mut String, depth: usize, w: &mut Writer) {
    let mut i = 0;
    while let Some(b) = blocks.get(i) {
        match b {
            FBlock::Table(t) => {
                if depth < model::MAX_DEPTH {
                    table_html(t, out, depth, w);
                } else {
                    flat_table_html(t, out);
                }
                i += 1;
            }
            FBlock::Para(p) if p.list.is_some() => {
                // Nested lists from consecutive list paragraphs.
                let mut stack: Vec<(bool, bool)> = Vec::new(); // (ordered, li open)
                let mut cur: Option<u32> = None;
                while let Some(FBlock::Para(q)) = blocks.get(i) {
                    let Some(li) = q.list else { break };
                    let lv = li.level.min(8) as usize;
                    // A top-level item of another list ends this one (the next restarts), and an
                    // item restarting at a number of its own starts a list at its level.
                    if lv == 0 && !stack.is_empty() && li.list != cur {
                        while !stack.is_empty() {
                            close_list(&mut stack, out);
                        }
                    }
                    if li.start.is_some() {
                        while stack.len() > lv {
                            close_list(&mut stack, out);
                        }
                    }
                    if lv == 0 {
                        cur = li.list;
                    }
                    while stack.len() > lv + 1 {
                        close_list(&mut stack, out);
                    }
                    if stack.len() == lv + 1 && stack.last().is_some_and(|s| s.0 != li.ordered) {
                        close_list(&mut stack, out);
                    }
                    while stack.len() < lv + 1 {
                        if let Some(top) = stack.last_mut()
                            && !top.1
                        {
                            out.push_str("<li>");
                            top.1 = true;
                        }
                        match li.start.filter(|_| li.ordered && stack.len() == lv) {
                            Some(s) => out.push_str(&format!("<ol start=\"{s}\">")),
                            None => out.push_str(if li.ordered { "<ol>" } else { "<ul>" }),
                        }
                        stack.push((li.ordered, false));
                    }
                    if let Some(top) = stack.last_mut() {
                        if top.1 {
                            out.push_str("</li>\n");
                        }
                        top.1 = true;
                    }
                    let body = inlines_html_in(&q.inlines, w);
                    let attrs = para_attrs(q);
                    out.push_str(&format!("<li{attrs}>{body}"));
                    i += 1;
                }
                while !stack.is_empty() {
                    close_list(&mut stack, out);
                }
                out.push('\n');
            }
            FBlock::Para(p) if p.kind == Kind::Code => {
                out.push_str("<pre><code>");
                let mut first = true;
                while let Some(FBlock::Para(q)) = blocks.get(i) {
                    if q.kind != Kind::Code || q.list.is_some() {
                        break;
                    }
                    if !first {
                        out.push('\n');
                    }
                    first = false;
                    out.push_str(&esc(&q.text()));
                    i += 1;
                }
                out.push_str("</code></pre>\n");
            }
            FBlock::Para(p) if p.kind == Kind::Quote => {
                out.push_str("<blockquote>\n");
                while let Some(FBlock::Para(q)) = blocks.get(i) {
                    if q.kind != Kind::Quote || q.list.is_some() {
                        break;
                    }
                    out.push_str(&para_html_as(q, Kind::Normal, w));
                    out.push('\n');
                    i += 1;
                }
                out.push_str("</blockquote>\n");
            }
            FBlock::Para(p) => {
                out.push_str(&para_html(p, w));
                out.push('\n');
                i += 1;
            }
        }
    }
}

fn close_list(stack: &mut Vec<(bool, bool)>, out: &mut String) {
    if let Some((ordered, li_open)) = stack.pop() {
        if li_open {
            out.push_str("</li>");
        }
        out.push_str(if ordered { "</ol>" } else { "</ul>" });
    }
}

/// A complete HTML document, its footnotes and endnotes after the body ([`notes_html`]).
pub fn export(doc: &Document) -> String {
    let flow = model::from_doc_notes(doc);
    export_flow_with(&flow, doc.styles.default_chr.lang.as_deref().unwrap_or("en"), Some(&doc.last_section), note_formats(doc))
}

/// `doc`'s content as an HTML fragment (no head or style sheet): what a copy puts on the system
/// clipboard beside its text, equations as MathML (#816), the notes it references after the
/// blocks, so a paste in another window keeps them.
pub fn export_fragment(doc: &Document) -> String {
    let flow = model::from_doc_notes(doc);
    let mut w = Writer { formats: note_formats(doc), ..Default::default() };
    let mut out = String::from("<meta charset=\"utf-8\">");
    blocks_html(&flow.blocks, &mut out, 0, &mut w);
    notes_html(&mut w, &mut out);
    out
}

pub fn export_flow(flow: &Flow, lang: &str) -> String {
    export_flow_on(flow, lang, None)
}

/// [`export_flow`] with the document's page size and margins as an `@page` rule, which the
/// reader takes back ([`apply_page_rule`]).
pub fn export_flow_on(flow: &Flow, lang: &str, page: Option<&wordcraft_doc::SectionProps>) -> String {
    export_flow_with(flow, lang, page, DEFAULT_NOTE_FORMATS)
}

/// [`export_flow_on`] numbering the notes in `formats` (footnotes, endnotes).
fn export_flow_with(flow: &Flow, lang: &str, page: Option<&wordcraft_doc::SectionProps>, formats: [NumFormat; 2]) -> String {
    let mut w = Writer { formats, ..Default::default() };
    let mut body = String::new();
    blocks_html(&flow.blocks, &mut body, 0, &mut w);
    let mut notes = String::new();
    notes_html(&mut w, &mut notes);
    let m = &flow.meta;
    let mut out = String::new();
    out.push_str("<!DOCTYPE html>\n");
    out.push_str(&format!("<html lang=\"{}\">\n<head>\n<meta charset=\"utf-8\">\n", esc(lang)));
    out.push_str("<meta name=\"generator\" content=\"Etch\">\n");
    out.push_str(&format!("<title>{}</title>\n", esc(&m.title)));
    for (k, v) in [("author", &m.author), ("description", &m.description), ("keywords", &m.keywords), ("subject", &m.subject)] {
        if !v.is_empty() {
            out.push_str(&format!("<meta name=\"{k}\" content=\"{}\">\n", esc(v)));
        }
    }
    out.push_str(
        "<style>\nbody{font-family:Aptos,Calibri,'Segoe UI',Arial,sans-serif;font-size:12pt;line-height:1.15;max-width:48em;margin:2em auto;padding:0 1em}\n\
         table{border-collapse:collapse;margin:0.5em 0}\nblockquote{margin:0.5em 2em;font-style:italic;color:#404040}\n\
         pre{background:#f5f5f5;padding:0.5em;font-family:'Courier New',monospace;font-size:10pt}\n",
    );
    if !notes.is_empty() {
        out.push_str("section.footnotes,section.endnotes{font-size:10pt}\n");
    }
    if let Some(s) = page {
        out.push_str(&format!(
            "@page{{size:{}pt {}pt;margin:{}pt {}pt {}pt {}pt}}\n",
            trim_num(s.page_w),
            trim_num(s.page_h),
            trim_num(s.margin_top),
            trim_num(s.margin_right),
            trim_num(s.margin_bottom),
            trim_num(s.margin_left)
        ));
    }
    out.push_str("</style>\n</head>\n<body>\n");
    out.push_str(&body);
    out.push_str(&notes);
    out.push_str("</body>\n</html>\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(f: &Flow) -> Vec<String> {
        f.blocks
            .iter()
            .map(|b| match b {
                FBlock::Para(p) => format!("{:?}{:?}:{}", p.kind, p.list.map(|l| (l.ordered, l.level)), p.text()),
                FBlock::Table(t) => format!("table{}x{}", t.rows.len(), t.cols()),
            })
            .collect()
    }

    fn table_style(html: &str) -> Option<String> {
        let doc = crate::model::to_doc(&parse(html));
        doc.body.iter().find_map(|b| match &**b {
            wordcraft_doc::Block::Table(t) => Some(t.props.style.clone().unwrap_or_default()),
            _ => None,
        })
    }

    /// `dir="rtl"` on a paragraph, or inherited from an element around it, is a right-to-left
    /// paragraph (#2693); `dir="ltr"` inside turns it back; the writer puts `dir="rtl"` on the
    /// paragraph.
    #[test]
    fn right_to_left_paragraphs_round_trip() {
        let f = parse("<div dir=\"rtl\"><p>עברית</p><p dir=\"ltr\">Latin</p><ul><li>item</li></ul></div><p>after</p>");
        let rtl: Vec<bool> = f.blocks.iter().filter_map(|b| if let FBlock::Para(p) = b { Some(p.rtl) } else { None }).collect();
        assert_eq!(rtl, vec![true, false, true, false]);
        let html = export_flow(&f, "en");
        assert!(html.contains("<p dir=\"rtl\">עברית</p>") && html.contains("<li dir=\"rtl\">"), "{html}");
        let again = parse(&html);
        let rtl2: Vec<bool> = again.blocks.iter().filter_map(|b| if let FBlock::Para(p) = b { Some(p.rtl) } else { None }).collect();
        assert_eq!(rtl2, rtl);
        let doc = crate::model::to_doc(&again);
        assert_eq!(doc.body.first().and_then(|b| b.as_para()).and_then(|p| p.props.bidi), Some(true));
    }

    /// `<ol start>` starts a list there, two `<ol>`s in a row are two lists (the second restarts
    /// at 1), and a list continued after a paragraph is written with the number it goes on with.
    #[test]
    fn list_starts_and_restarts_round_trip() {
        let f = parse("<ol start=\"3\"><li>three</li><li>four</li></ol><ol><li>one</li></ol><p>x</p><ol start=\"5\"><li>five</li></ol>");
        let items: Vec<(Option<u32>, Option<u32>)> =
            f.blocks.iter().filter_map(|b| if let FBlock::Para(p) = b { p.list.map(|l| (l.list, l.start)) } else { None }).collect();
        assert_eq!(items.len(), 4);
        assert_eq!(items[0].1, Some(3));
        assert_eq!(items[1].1, None);
        assert_ne!(items[0].0, items[2].0, "two lists");
        assert_eq!(items[3].1, Some(5));
        let doc = crate::model::to_doc(&f);
        let mut c = wordcraft_doc::numbering::Counters::default();
        let labels: Vec<String> = doc
            .body
            .iter()
            .filter_map(|b| b.as_para())
            .filter_map(|p| {
                let n = p.props.numbering?;
                let (l, _) = c.next_label(&doc.numbering, n.num, n.level)?;
                Some(format!("{l}{}", p.plain_text()))
            })
            .collect();
        assert_eq!(labels, vec!["3.three", "4.four", "1.one", "5.five"]);
        let html = export(&doc);
        assert!(html.contains("<ol start=\"3\">") && html.contains("</ol><ol><li>one") && html.contains("<ol start=\"5\">"), "{html}");
    }

    /// The `@page` rule (size and margins) is the page the document opens on and is written
    /// back; named sizes and `landscape` are understood; a broken rule is ignored.
    #[test]
    fn page_rule_round_trips() {
        let page = wordcraft_doc::SectionProps::default();
        let html = "<style>p{} @page { size: A4 landscape; margin: 1in 2cm }</style><p>x</p>";
        let doc = import_file(html, &|_| None, &page).unwrap();
        let s = &doc.last_section;
        assert!((s.page_w - 841.89).abs() < 0.1 && (s.page_h - 595.28).abs() < 0.1 && s.landscape, "{s:?}");
        assert!((s.margin_top - 72.0).abs() < 0.01 && (s.margin_left - 56.69).abs() < 0.1, "{s:?}");
        let out = export(&doc);
        assert!(out.contains("@page{size:841.89pt 595.28pt;margin:72pt 56.69pt 72pt 56.69pt}"), "{out}");
        let again = import_file(&out, &|_| None, &page).unwrap();
        let t = &again.last_section;
        assert!((t.page_w - s.page_w).abs() < 0.01 && (t.page_h - s.page_h).abs() < 0.01 && t.landscape, "{t:?}");
        for bad in [
            "<style>@page { size: 1e308pt; margin: -5pt }</style>",
            "<style>@page { size: ; margin: 1pt 2pt 3pt 4pt 5pt }</style>",
            "<p>@page {size: A4}</p>",
        ] {
            let d = import_file(bad, &|_| None, &page).unwrap();
            assert_eq!((d.last_section.page_w, d.last_section.margin_top), (page.page_w, page.margin_top), "{bad}");
        }
    }

    /// Paragraph spacing, indents and line spacing from inline CSS are the paragraph's, and are
    /// written back.
    #[test]
    fn paragraph_spacing_round_trips() {
        let f = parse(
            "<p style=\"margin-top:12pt;margin-bottom:6pt;margin-left:36pt;text-indent:18pt;line-height:1.5\">x</p><p style=\"line-height:200%\">y</p>",
        );
        let Some(FBlock::Para(p)) = f.blocks.first() else { panic!() };
        let sp = &p.spacing;
        assert_eq!((sp.before, sp.after, sp.left, sp.first), (Some(12.0), Some(6.0), Some(36.0), Some(18.0)));
        assert_eq!(sp.line, Some(wordcraft_doc::props::LineSpacing::Multiple(1.5)));
        let Some(FBlock::Para(q)) = f.blocks.get(1) else { panic!() };
        assert_eq!(q.spacing.line, Some(wordcraft_doc::props::LineSpacing::Multiple(2.0)));
        let html = export_flow(&f, "en");
        assert!(html.contains("margin-top:12pt;margin-bottom:6pt;margin-left:36pt;text-indent:18pt;line-height:1.5"), "{html}");
        let again = parse(&html);
        let Some(FBlock::Para(p2)) = again.blocks.first() else { panic!() };
        assert_eq!(p2.spacing, p.spacing);
    }

    /// `<col>` widths (CSS or the attribute) are the table's column widths, and are written
    /// back as a `<colgroup>`.
    #[test]
    fn column_widths_round_trip() {
        let f = parse("<table><colgroup><col style=\"width:72pt\"><col width=\"150\"></colgroup><tr><td>a</td><td>b</td></tr></table>");
        let Some(FBlock::Table(t)) = f.blocks.first() else { panic!("{:?}", texts(&f)) };
        assert_eq!(t.widths.len(), 2, "{:?}", t.widths);
        assert_eq!(t.widths[0], 72.0);
        let html = export_flow(&f, "en");
        assert!(html.contains("<colgroup><col style=\"width:72pt\">"), "{html}");
        let again = parse(&html);
        let Some(FBlock::Table(t2)) = again.blocks.first() else { panic!() };
        assert_eq!(t2.widths, t.widths);
    }

    #[test]
    fn borderless_tables_get_no_grid_style() {
        assert_eq!(table_style("<table><tr><td>a<td>b</table>").as_deref(), Some(""));
        assert_eq!(table_style("<table style='border:none'><tr><td style='border:0'>a</table>").as_deref(), Some(""));
        assert_eq!(table_style("<table border=0><tr><td style='border: 0px none'>a</table>").as_deref(), Some(""));
        assert_eq!(table_style("<table border=1><tr><td>a</table>").as_deref(), Some("TableGrid"));
        assert_eq!(table_style("<table style='border:1px solid #000'><tr><td>a</table>").as_deref(), Some("TableGrid"));
        assert_eq!(table_style("<table><tr><td style='border:1px solid #999'>a</table>").as_deref(), Some("TableGrid"));
    }

    #[test]
    fn borderless_tables_export_without_cell_borders() {
        // #438: a table without grid style and borders writes no cell borders and reopens borderless.
        for (src, grid) in [
            ("<table style='border:none'><tr><td>A<td>B</table>", false),
            ("<table><tr><td>A<td>B</table>", false),
            ("<table border=1><tr><td>A<td>B</table>", true),
        ] {
            let out = export(&crate::model::to_doc(&parse(src)));
            assert_eq!(out.contains("border:1px"), grid, "{src}: {out}");
            assert_eq!(table_style(&out).as_deref(), Some(if grid { "TableGrid" } else { "" }), "{src}");
            assert!(out.contains(">A</td>") && out.contains(">B</td>"), "{out}");
        }
    }

    #[test]
    fn css_preformatted_paragraphs_keep_edge_spaces() {
        // #439: `white-space:pre` keeps leading and trailing spaces like `<pre>`; normal
        // paragraphs still collapse.
        let f = parse(
            "<p style=\"white-space:pre\">  Alpha  Beta  </p><p style=\"white-space:pre-wrap\">\n  Gamma  \n</p><pre>  Alpha  </pre><p>  Alpha  Beta  </p>",
        );
        assert_eq!(texts(&f), vec!["NormalNone:  Alpha  Beta  ", "NormalNone:  Gamma  ", "CodeNone:  Alpha  ", "NormalNone:Alpha Beta"]);
    }

    #[test]
    fn tag_soup() {
        let f = parse(
            "<html><head><title>T &amp; U</title><style>p{}</style><script>if(a<b)x</script></head><body><h1>Head</h1><p>One <b>bold<p>Two <i>it</i>\n  text<ul><li>a<li>b<ol><li>c</ol></ul><table><tr><td>x<td colspan=2>y<tr><th>z</table><blockquote>q</blockquote><pre>l1\nl2</pre>",
        );
        assert_eq!(f.meta.title, "T & U");
        assert_eq!(
            texts(&f),
            vec![
                "Heading(1)None:Head",
                "NormalNone:One bold",
                "NormalNone:Two it text",
                "NormalSome((false, 0)):a",
                "NormalSome((false, 0)):b",
                "NormalSome((true, 1)):c",
                "table2x3",
                "QuoteNone:q",
                "CodeNone:l1",
                "CodeNone:l2",
            ]
        );
        let FBlock::Para(p) = &f.blocks[1] else { panic!() };
        assert!(matches!(&p.inlines[1], Inline::Text(t, f) if t == "bold" && f.bold));
    }

    #[test]
    fn omitted_head_keeps_body_text() {
        let f = parse("<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>T</title>\n<body><p>Hello world</p><p>Second</p></body></html>");
        assert_eq!(f.meta.title, "T");
        assert_eq!(texts(&f), vec!["NormalNone:Hello world", "NormalNone:Second"]);

        for body in [
            "<p>Hello world</p>",
            "<div>Hello world</div>",
            "<span>Hello world</span>",
            "<a href='https://example.test'>Hello world</a>",
            "<custom>Hello world</custom>",
            "<textarea>ignored</textarea>Hello world",
        ] {
            let f = parse(&format!(
                "<html><head><title>T &amp; U</title><meta name='author' content='Ann'><style>p{{}}</style><script>ignored</script>{body}",
            ));
            assert_eq!(f.meta.title, "T & U");
            assert_eq!(f.meta.author, "Ann");
            assert_eq!(texts(&f), vec!["NormalNone:Hello world"], "{body}");
        }
    }

    #[test]
    fn styles_and_links() {
        let f = parse("<p style='text-align:center'><span style=\"color:#ff0000;font-weight:700\">red</span> <a href=\"http://x\">l</a><br>n</p>");
        let FBlock::Para(p) = &f.blocks[0] else { panic!() };
        assert_eq!(p.align, Some(Align::Center));
        assert!(matches!(&p.inlines[0], Inline::Text(t, f) if t == "red" && f.bold && f.color == Some(wordcraft_doc::Rgb(255,0,0))));
        assert!(p.inlines.iter().any(|i| matches!(i, Inline::Text(t, f) if t == "l" && f.link.as_deref() == Some("http://x"))));
        assert!(p.text().ends_with("l\nn"));
    }

    /// The paragraphs of every cell of the first table, row by row.
    fn cell_paras(f: &Flow) -> Vec<Vec<Vec<Para>>> {
        let Some(FBlock::Table(t)) = f.blocks.iter().find(|b| matches!(b, FBlock::Table(_))) else { panic!("no table") };
        t.rows
            .iter()
            .map(|r| {
                r.iter().map(|c| c.blocks.iter().filter_map(|b| if let FBlock::Para(p) = b { Some(p.clone()) } else { None }).collect()).collect()
            })
            .collect()
    }

    fn all_bold(p: &Para) -> bool {
        !p.inlines.is_empty() && p.inlines.iter().all(|i| matches!(i, Inline::Text(_, f) if f.bold))
    }

    fn any_bold(p: &Para) -> bool {
        p.inlines.iter().any(|i| matches!(i, Inline::Text(_, f) if f.bold))
    }

    #[test]
    fn header_cells_are_bold_unless_their_css_says_otherwise() {
        // #1381: a <th> is bold, as in browsers and Word; `font-weight:normal` on it or inside it wins.
        let f = parse(
            "<table><thead><tr><th>Head</th><th style='font-weight:normal'>Plain</th><th><span style='font-weight:400'>Inner</span></th></tr></thead><tbody><tr><td>a<td>b<td>c</tbody></table>",
        );
        let rows = cell_paras(&f);
        assert!(all_bold(&rows[0][0][0]), "{:?}", rows[0][0]);
        assert!(!any_bold(&rows[0][1][0]), "{:?}", rows[0][1]);
        assert!(!any_bold(&rows[0][2][0]), "{:?}", rows[0][2]);
        assert!(!any_bold(&rows[1][0][0]), "td stays regular");
    }

    #[test]
    fn cell_text_align_reaches_the_cell_paragraphs() {
        // #1381: `text-align` on a td/th, or inherited from its tr/thead/tbody/table, aligns the
        // cell's paragraphs; a paragraph's own text-align inside the cell wins.
        let f = parse(
            "<table style='text-align:center'><tbody style='text-align:justify'><tr style='text-align:right'>\
             <td style='text-align:right;'>r</td><td>inherited</td><td style='text-align:right'><p style='text-align:center'>own</p></td>\
             <td align=right>attr</td></tr><tr><td>body</td></tr></tbody><tr><td>table</td></tr></table>",
        );
        let rows = cell_paras(&f);
        assert_eq!(rows[0][0][0].align, Some(Align::Right), "td style");
        assert_eq!(rows[0][1][0].align, Some(Align::Right), "from tr");
        assert_eq!(rows[0][2][0].align, Some(Align::Center), "own paragraph wins");
        assert_eq!(rows[0][3][0].align, Some(Align::Right), "align attribute");
        assert_eq!(rows[1][0][0].align, Some(Align::Justify), "from tbody");
        assert_eq!(rows[2][0][0].align, Some(Align::Center), "from table");
        // `align` on a table places the table; it doesn't align the text in its cells.
        let f = parse("<table align=center><tr><td>x</td></tr></table>");
        assert_eq!(cell_paras(&f)[0][0][0].align, None);
    }

    #[test]
    fn reporter_sample_header_bold_and_right_aligned_cells() {
        // #1381: the reporter's table.
        let f = parse(
            "<table border=\"1\" style=\"width:100%;border-collapse:collapse;\">\n<thead><tr style=\"background-color:#e8f0fa;\"><th>Indicatore</th><th>Valore atteso</th></tr></thead>\n<tbody>\n<tr><td>Audit</td><td style=\"text-align:right;\">4</td></tr>\n</tbody></table>",
        );
        let rows = cell_paras(&f);
        assert!(all_bold(&rows[0][0][0]) && all_bold(&rows[0][1][0]), "{:?}", rows[0]);
        assert_eq!(rows[1][1][0].align, Some(Align::Right));
        assert_eq!(rows[1][0][0].align, None);
    }

    #[test]
    fn entities() {
        assert_eq!(unescape("a &lt;b&gt; &#65;&#x42; &bogus; &"), "a <b> AB &bogus; &");
        assert_eq!(unescape("&minus; &COPY; &EURO; &TRADE; &frac12; &frac34;"), "− © € ™ ½ ¾");
        assert_eq!(unescape("&theta; &lambda; &Delta; &nabla; &sum; &radic; &ne; &le; &ge; &hearts;"), "θ λ Δ ∇ ∑ √ ≠ ≤ ≥ ♥");
    }

    #[test]
    fn wbr_word_break_opportunity() {
        let f = parse("<p>Super<wbr>cali<wbr>fragilistic</p>");
        let Some(FBlock::Para(p)) = f.blocks.first() else { panic!("expected para") };
        assert_eq!(p.text(), "Super\u{200B}cali\u{200B}fragilistic");
    }

    #[test]
    fn page_break_after_and_before() {
        let f = parse("<p style='page-break-after: always'>First</p><p>Second</p>");
        let paras: Vec<_> = f.blocks.iter().filter_map(|b| if let FBlock::Para(p) = b { Some(p) } else { None }).collect();
        assert_eq!(paras.len(), 2);
        assert!(!paras[0].page_break);
        assert!(paras[1].page_break);

        let f2 = parse("<div style='break-after: page'><p>Part 1</p></div><p>Part 2</p>");
        let paras2: Vec<_> = f2.blocks.iter().filter_map(|b| if let FBlock::Para(p) = b { Some(p) } else { None }).collect();
        assert_eq!(paras2.len(), 2);
        assert!(!paras2[0].page_break);
        assert!(paras2[1].page_break);
    }

    #[test]
    fn font_size_relative_and_absolute() {
        let f = parse("<font size=\"+1\">larger</font><font size=\"-1\">smaller</font><font size=\"5\">huge</font>");
        let inls = match &f.blocks[0] {
            FBlock::Para(p) => &p.inlines,
            _ => panic!("expected para"),
        };
        let sizes: Vec<Option<f32>> = inls
            .iter()
            .map(|i| match i {
                Inline::Text(_, fmt) => fmt.size,
                _ => None,
            })
            .collect();
        assert_eq!(sizes, vec![Some(13.5), Some(10.0), Some(18.0)]);
    }

    #[test]
    fn decode_with_charset_declarations() {
        let iso = b"<html><head><meta charset=\"iso-8859-1\"></head><body>\xE9l\xE8ve</body></html>";
        assert!(decode(iso).contains("élève"));

        let win = b"<html><head><meta charset='windows-1252'></head><body>\x80100</body></html>";
        assert!(decode(win).contains("€100"));

        let gbk = b"<html><head><meta http-equiv=\"Content-Type\" content=\"text/html; charset=gbk\"></head><body>\xC4\xE3\xBA\xC3</body></html>";
        assert!(decode(gbk).contains("你好"));

        let xml = b"<?xml version=\"1.0\" encoding=\"utf-8\"?><html><body>hello</body></html>";
        assert!(decode(xml).contains("hello"));
    }

    #[test]
    fn raw_text_elements_skip_only_matching_end_tag() {
        let f = parse("<script>var a = \"</scripted>\"; var b = \"<b>malicious</b>\";</script><p>Hello</p>");
        assert_eq!(f.blocks.len(), 1);
        let p = match &f.blocks[0] {
            FBlock::Para(p) => p,
            b => panic!("expected para, got {b:?}"),
        };
        assert_eq!(p.text(), "Hello");

        let f = parse("<style>/* </styles> */ p { color: red; }</style><p>World</p>");
        assert_eq!(f.blocks.len(), 1);
        let p = match &f.blocks[0] {
            FBlock::Para(p) => p,
            b => panic!("expected para, got {b:?}"),
        };
        assert_eq!(p.text(), "World");
    }

    #[test]
    fn tables_nested_too_deep_keep_their_text() {
        let para = |t: &str| {
            let mut p = Para::default();
            p.push_text(t, &Fmt::default());
            FBlock::Para(p)
        };
        let mut inner = FTable {
            rows: vec![vec![
                Cell { blocks: vec![para("deepest a")], ..Default::default() },
                Cell { blocks: vec![para("deepest b")], ..Default::default() },
            ]],
            widths: Vec::new(),
            borderless: false,
        };
        for n in 0..12 {
            let cell = Cell { blocks: vec![para(&format!("level {n}")), FBlock::Table(inner)], ..Default::default() };
            inner = FTable { rows: vec![vec![cell]], widths: Vec::new(), borderless: false };
        }
        let html = export_flow(&Flow { blocks: vec![FBlock::Table(inner)], ..Default::default() }, "en");
        for t in ["level 11", "level 0", "deepest a deepest b"] {
            assert!(html.contains(t), "{t}: {html}");
        }
        assert_eq!(html.matches("<table").count(), model::MAX_DEPTH, "{html}");
    }

    /// Footnotes and endnotes are written as plain HTML5 with DPUB-ARIA roles (a `doc-noteref`
    /// link in the text, the notes after the body in `doc-endnotes` sections, each item linking
    /// back) and read back as notes: a numbered one, one with a mark of its own, an endnote, one
    /// of two paragraphs and one referenced from a table cell.
    #[test]
    fn footnotes_and_endnotes_round_trip_as_dpub_aria_notes() {
        use crate::model::test_notes::{noted_document, notes_of};
        use wordcraft_doc::StoryRef;
        use wordcraft_doc::para::InlineObject;
        let d = noted_document();
        let html = export(&d);
        let back = |r: &str| format!("<a href=\"#{r}\" role=\"doc-backlink\" aria-label=\"Back to text\">\u{21A9}\u{FE0E}</a>");
        for want in [
            "Alpha<a href=\"#fn1\" id=\"fnref1\" role=\"doc-noteref\"><sup>1</sup></a> beta.".to_string(),
            "<a href=\"#fn2\" id=\"fnref2\" role=\"doc-noteref\"><sup>*</sup></a>".to_string(),
            "Gamma<a href=\"#en1\" id=\"enref1\" role=\"doc-noteref\"><sup>i</sup></a> delta.".to_string(),
            "<td style=\"border:1px solid #999;padding:4px 6px;vertical-align:top\">cell<a href=\"#fn4\" id=\"fnref4\" role=\"doc-noteref\"><sup>3</sup></a> text</td>".to_string(),
            format!(
                "<section role=\"doc-endnotes\" class=\"footnotes\" aria-label=\"Footnotes\">\n<hr>\n<ol>\n\
                 <li id=\"fn1\">\n<p>The first note. {}</p>\n</li>\n\
                 <li id=\"fn2\" style=\"list-style-type:'* '\">\n<p>Starred note. {}</p>\n</li>\n\
                 <li id=\"fn3\" style=\"list-style-type:'2. '\">\n<p>Two paragraphs,</p>\n<p>the second one. {}</p>\n</li>\n\
                 <li id=\"fn4\" style=\"list-style-type:'3. '\">\n<p>In a cell. {}</p>\n</li>\n</ol>\n</section>\n",
                back("fnref1"),
                back("fnref2"),
                back("fnref3"),
                back("fnref4")
            ),
            format!(
                "<section role=\"doc-endnotes\" class=\"endnotes\" aria-label=\"Endnotes\">\n<hr>\n<ol>\n\
                 <li id=\"en1\" style=\"list-style-type:'i. '\">\n<p>An endnote. {}</p>\n</li>\n</ol>\n</section>\n</body>",
                back("enref1")
            ),
            "section.footnotes,section.endnotes{font-size:10pt}".to_string(),
        ] {
            assert!(html.contains(&want), "missing {want:?} in\n{html}");
        }
        // No Word vocabulary in what Etch writes.
        for word in ["mso-", "Mso", "_ftn", "_edn", "supportFootnotes", "supportEndnotes"] {
            assert!(!html.contains(word), "{word} in\n{html}");
        }
        // After the body's blocks, the footnotes before the endnotes.
        let (table, foot, end) =
            (html.find("</table>").unwrap(), html.find("class=\"footnotes\"").unwrap(), html.find("class=\"endnotes\"").unwrap());
        assert!(table < foot && foot < end, "{html}");
        let again = import(&html);
        assert_eq!(notes_of(&again), notes_of(&d));
        // (A read document gets a paragraph after its last table.)
        assert_eq!(again.plain_text(StoryRef::Body).trim_end(), d.plain_text(StoryRef::Body).trim_end());
        // Nothing of the list of notes (the rule, the arrows) is left in the body or the notes.
        assert!(!again.plain_text(StoryRef::Body).contains('\u{21A9}'));
        for (_, _, texts) in notes_of(&again) {
            assert!(texts.iter().all(|t| !t.contains('\u{21A9}') && t == t.trim()), "{texts:?}");
        }
        let bookmarks: Vec<String> = again
            .body
            .iter()
            .filter_map(|b| b.as_para())
            .flat_map(|p| &p.objects)
            .filter_map(|o| if let InlineObject::BookmarkStart { name } = o { Some(name.clone()) } else { None })
            .collect();
        assert!(bookmarks.is_empty(), "the note links aren't bookmarks: {bookmarks:?}");
        // And again: the second save is the first.
        assert_eq!(export(&again).replace("<p>&nbsp;</p>\n", ""), html.replace("<p>&nbsp;</p>\n", ""));
    }

    /// A document without notes writes no section of notes and no note style: its HTML is what
    /// it was before notes were written.
    #[test]
    fn a_document_without_notes_writes_no_note_markup() {
        let plain = export(&Document::from_text("No notes."));
        for no in ["doc-endnotes", "doc-noteref", "footnotes", "endnotes", "<section", "<hr>"] {
            assert!(!plain.contains(no), "{no} in\n{plain}");
        }
        let frag = export_fragment(&Document::from_text("No notes."));
        assert_eq!(frag, "<meta charset=\"utf-8\"><p>No notes.</p>\n");
    }

    /// The clipboard's fragment carries the notes as the file does, and reads back with them.
    #[test]
    fn the_clipboard_fragment_carries_the_notes() {
        use crate::model::test_notes::{noted_document, notes_of};
        let d = noted_document();
        let frag = export_fragment(&d);
        assert!(frag.contains("role=\"doc-noteref\"") && frag.contains("<section role=\"doc-endnotes\" class=\"endnotes\""), "{frag}");
        assert!(!frag.contains("<style"), "{frag}");
        assert_eq!(notes_of(&import(&frag)), notes_of(&d));
    }

    /// A note's text that doesn't end in a paragraph (a list here) gets the link back in a
    /// paragraph of its own, and reads back without it.
    #[test]
    fn a_note_ending_in_a_list_gets_its_link_back_in_a_paragraph_of_its_own() {
        use crate::model::test_notes::notes_of;
        let html = concat!(
            "<p>Text<a href=\"#fn1\" id=\"fnref1\" role=\"doc-noteref\"><sup>1</sup></a>.</p>",
            "<section role=\"doc-endnotes\" class=\"footnotes\"><hr><ol><li id=\"fn1\"><p>Intro.</p><ul><li>one</li><li>two</li></ul>",
            "<p><a href=\"#fnref1\" role=\"doc-backlink\">\u{21A9}</a></p></li></ol></section>",
        );
        let d = import(html);
        let notes = notes_of(&d);
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert_eq!(notes[0].2.iter().filter(|t| !t.is_empty()).cloned().collect::<Vec<_>>(), ["Intro.", "one", "two"]);
        let texts: Vec<String> = d.body.iter().map(|b| b.as_para().map(|p| p.plain_text()).unwrap_or_default()).collect();
        assert_eq!(texts, ["Text."]);
        // Written again, the list stays in the note and the link back follows it.
        let out = export(&d);
        let li = out.find("<li id=\"fn1\">").unwrap();
        let tail = &out[li..];
        assert!(tail.find("</ul>").unwrap() < tail.find("<p><a href=\"#fnref1\" role=\"doc-backlink\"").unwrap(), "{out}");
    }

    /// A page as Word saves it (its structure, with the conditional comments it writes for
    /// browsers, unquoted attributes, a note with a mark of its own) reads as a document with
    /// its notes; the marks, the rule and the `<br>` of the lists leave nothing behind.
    #[test]
    fn words_own_note_markup_reads_as_notes() {
        use crate::model::test_notes::notes_of;
        let html = concat!(
            "<html><body>",
            "<p class=MsoNormal>Body<a style='mso-footnote-id:ftn1' href=\"#_ftn1\" name=\"_ftnref1\" title=\"\"><span class=MsoFootnoteReference>",
            "<span style='mso-special-character:footnote'><![if !supportFootnotes]><span class=MsoFootnoteReference>[1]</span><![endif]></span></span></a>",
            " and an endnote<a style='mso-endnote-id:edn1' href=\"#_edn1\" name=\"_ednref1\" title=\"\"><span class=MsoEndnoteReference>",
            "<span style='mso-special-character:footnote'><![if !supportFootnotes]><span class=MsoEndnoteReference>[i]</span><![endif]></span></span></a>",
            " and a star<a style='mso-footnote-id:ftn2' href=\"#_ftn2\" name=\"_ftnref2\" title=\"\"><span class=MsoFootnoteReference>*</span></a>.</p>",
            "<div style='mso-element:footnote-list'><![if !supportFootnotes]><br clear=all><hr align=left size=1 width=\"33%\"><![endif]>",
            "<div style='mso-element:footnote' id=ftn1><p class=MsoFootnoteText><a style='mso-footnote-id:ftn1' href=\"#_ftnref1\" name=\"_ftn1\" title=\"\">",
            "<span class=MsoFootnoteReference><span style='mso-special-character:footnote'><![if !supportFootnotes]><span class=MsoFootnoteReference>[1]",
            "</span><![endif]></span></span></a> A Word footnote.</p></div>",
            "<div style='mso-element:footnote' id=ftn2><p class=MsoFootnoteText><a style='mso-footnote-id:ftn2' href=\"#_ftnref2\" name=\"_ftn2\" title=\"\">",
            "<span class=MsoFootnoteReference>*</span></a> A starred one.</p></div>",
            "</div>",
            "<div style='mso-element:endnote-list'><![if !supportEndnotes]><br clear=all><hr align=left size=1 width=\"33%\"><![endif]>",
            "<div style='mso-element:endnote' id=edn1><p class=MsoEndnoteText><a style='mso-endnote-id:edn1' href=\"#_ednref1\" name=\"_edn1\" title=\"\">",
            "<span class=MsoEndnoteReference><span style='mso-special-character:footnote'><![if !supportFootnotes]><span class=MsoEndnoteReference>[i]",
            "</span><![endif]></span></span></a> A Word endnote.</p></div>",
            "</div></body></html>"
        );
        let d = import(html);
        assert_eq!(
            notes_of(&d),
            vec![
                (false, String::new(), vec!["A Word footnote.".to_string()]),
                (true, String::new(), vec!["A Word endnote.".to_string()]),
                (false, "*".to_string(), vec!["A starred one.".to_string()]),
            ]
        );
        let texts: Vec<String> = d.body.iter().map(|b| b.as_para().map(|p| p.plain_text()).unwrap_or_default()).collect();
        assert_eq!(texts, ["Body and an endnote and a star."], "nothing but the body's paragraph");
    }

    /// A note link whose note is missing keeps the mark it showed, as superscript text; a note
    /// nothing refers to keeps its text after the body. Both vocabularies.
    #[test]
    fn a_note_link_without_its_note_keeps_its_mark_and_a_stray_note_its_text() {
        use crate::model::test_notes::notes_of;
        for html in [
            "<p>Text<a href=\"#_ftn7\" name=\"_ftnref7\"><sup>7</sup></a> here.</p>\
             <div style=\"mso-element:footnote\" id=\"ftn9\"><p>Orphan note.</p></div>",
            "<p>Text<a href=\"#fn7\" id=\"fnref7\" role=\"doc-noteref\"><sup>7</sup></a> here.</p>\
             <section role=\"doc-endnotes\" class=\"footnotes\"><hr><ol><li id=\"fn9\"><p>Orphan note. \
             <a href=\"#fnref9\" role=\"doc-backlink\">\u{21A9}</a></p></li></ol></section>",
        ] {
            let d = import(html);
            assert!(notes_of(&d).is_empty());
            let texts: Vec<String> = d.body.iter().map(|b| b.as_para().map(|p| p.plain_text().trim_end().to_string()).unwrap_or_default()).collect();
            assert_eq!(texts, ["Text7 here.", "Orphan note."], "{html}");
            let p = d.body[0].as_para().unwrap();
            assert_eq!(p.props_of_char(4).vert_align, Some(wordcraft_doc::props::VertAlign::Superscript));
        }
    }

    /// Lists, links and sections that aren't notes read as before: an ordinary `ol` stays a
    /// list, a link with an id stays a bookmark, a section without the role keeps its text, and
    /// a backlink outside a note is an ordinary link.
    #[test]
    fn ordinary_lists_links_and_sections_are_not_notes() {
        use crate::model::test_notes::notes_of;
        let d = import(
            "<section class=\"footnotes\"><ol><li id=\"fn1\">An item.</li></ol></section>\
             <p><a href=\"#top\" role=\"doc-backlink\">Up</a> and <a href=\"#fn1\">a link</a>.</p>",
        );
        assert!(notes_of(&d).is_empty());
        let texts: Vec<String> = d.body.iter().map(|b| b.as_para().map(|p| p.plain_text()).unwrap_or_default()).collect();
        assert_eq!(texts, ["An item.", "Up and a link."]);
        assert!(d.body[0].as_para().unwrap().props.numbering.is_some(), "the item is still in a list");
    }
}
