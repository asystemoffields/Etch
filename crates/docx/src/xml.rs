//! A small, bounded XML tree on top of `quick-xml`, plus a string-building XML writer.
//!
//! Element and attribute names are rewritten to *canonical prefixes* by namespace URI (`w:p`,
//! `r:id`, `a:blip`…), so the reader doesn't care which prefixes a producer chose. Names in
//! unknown namespaces keep their local name behind a `?:` prefix.
//!
//! Memory: a tree costs about what its nodes need and no more. Each distinct name is allocated
//! once per part and shared ([`Name`]); child and attribute lists are cut to their length when
//! an element ends. The nodes of one part, and of every part of one file together
//! ([`NodeBudget`]), are capped while parsing, so a small file of millions of tiny elements is
//! refused before it costs much.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use quick_xml::XmlVersion;
use quick_xml::events::Event;
use quick_xml::name::{QName, ResolveResult};
use quick_xml::reader::NsReader;

use crate::DocxError;

/// Maximum element nesting accepted by the parser.
pub const MAX_DEPTH: usize = 256;
/// Most nodes in one part read on its own (see [`MAX_FILE_NODES`] for what counts).
pub const MAX_ELEMENTS: usize = MAX_FILE_NODES;
/// Most nodes the XML trees of one file hold at once ([`NodeBudget`]): 12 million (twelve for
/// each of a million paragraphs, which the document limits once were; not tied to them since:
/// the budget bounds memory, not a document's length). An element or attribute counts one,
/// a text node two, and a name past [`MAX_SHARED_NAMES`] one more, so that each count stands for
/// at most about 72 bytes of tree whatever the markup (under 900 MB in all, what one part could
/// take before this budget); text and attribute values longer than a few bytes add what they
/// hold, which the package's size limits bound.
///
/// The parts that grow with a document (its stories: the body, headers and footers, notes and
/// comments; and its lists) are read a block at a time as they are parsed ([`Chunks`]), so what
/// they hold at once is one block (a paragraph, a table row, a block of a cell) and a document
/// of any length fits. WordCraft writes about 7 nodes for a paragraph of prose, 11 for a table
/// cell and 720 for a paragraph of ten runs with every formatting it has (60 a run more): only
/// one paragraph of some 200,000 such runs, or a text box of some 16,000 such paragraphs, would
/// pass the budget.
/// Other parts are held whole while they are read (charts and diagrams for the whole read).
pub const MAX_FILE_NODES: usize = 12_000_000;
/// Most attributes kept on one element (the schemas define a few dozen at most); later ones
/// are ignored.
pub const MAX_ATTRS: usize = 1024;
/// Most distinct names shared in one part (a schema has a few hundred); names past them are
/// allocated per node, so a file of millions of different names can't grow the table.
const MAX_SHARED_NAMES: usize = 8192;

/// An element or attribute name with its canonical prefix (`w:p`, `r:id`). The nodes of one
/// name in a parsed part share one allocation, so a name costs a node 16 bytes.
#[derive(Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Name(Arc<str>);

impl Name {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::ops::Deref for Name {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for Name {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::borrow::Borrow<str> for Name {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl From<&str> for Name {
    fn from(s: &str) -> Name {
        Name(Arc::from(s))
    }
}

impl From<String> for Name {
    fn from(s: String) -> Name {
        Name(Arc::from(s))
    }
}

impl From<&String> for Name {
    fn from(s: &String) -> Name {
        Name(Arc::from(s.as_str()))
    }
}

impl From<&Name> for String {
    fn from(n: &Name) -> String {
        n.as_str().to_string()
    }
}

impl PartialEq<str> for Name {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for Name {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl PartialEq<String> for Name {
    fn eq(&self, other: &String) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<Name> for str {
    fn eq(&self, other: &Name) -> bool {
        self == other.as_str()
    }
}

impl PartialEq<Name> for &str {
    fn eq(&self, other: &Name) -> bool {
        *self == other.as_str()
    }
}

impl PartialEq<Name> for String {
    fn eq(&self, other: &Name) -> bool {
        self == other.as_str()
    }
}

impl std::fmt::Display for Name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::fmt::Debug for Name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&*self.0, f)
    }
}

/// What's left of one file's node budget ([`MAX_FILE_NODES`]), shared by every part parsed from
/// it: the trees a file's parts are held in at once ([`Tree`], and what a part read a chunk at a
/// time holds while it is read) can't add up to more than one part may have. Once a part doesn't
/// fit, the budget stays spent, so a reader that skips a broken part can still tell the file was
/// refused rather than read in part.
#[derive(Debug)]
pub struct NodeBudget {
    left: std::cell::Cell<usize>,
    spent: std::cell::Cell<bool>,
}

impl NodeBudget {
    pub fn new(nodes: usize) -> NodeBudget {
        NodeBudget { left: std::cell::Cell::new(nodes), spent: std::cell::Cell::new(false) }
    }

    /// The budget of one file.
    pub fn file() -> NodeBudget {
        NodeBudget::new(MAX_FILE_NODES)
    }

    /// Nodes still allowed.
    #[cfg(test)]
    pub fn left(&self) -> usize {
        self.left.get()
    }

    /// Did a part not fit?
    pub fn spent(&self) -> bool {
        self.spent.get()
    }

    /// Take `n` nodes; false (nothing taken) when fewer are left, or once it is spent (what
    /// was given back since doesn't let a refused file read on).
    fn take(&self, n: usize) -> bool {
        if self.spent.get() {
            return false;
        }
        match self.left.get().checked_sub(n) {
            Some(left) => {
                self.left.set(left);
                true
            }
            None => false,
        }
    }

    /// Give back `n` nodes a parse took (a tree dropped, a chunk read and let go).
    fn give(&self, n: usize) {
        self.left.set(self.left.get().saturating_add(n));
    }
}

/// What a parse hands on as soon as it is read instead of keeping it in the tree. A reader
/// reads a story a block at a time (and a long table a row at a time) this way, so the memory
/// a story takes while it is read is that of one block, however long the story is, and the
/// budget is charged only for the nodes held at once.
pub trait Chunks {
    /// Is element `el` (just started: its name and attributes), whose open ancestors are `open`
    /// (the root first), handed on when it ends rather than added to its parent?
    fn is_chunk(&mut self, open: &[El], el: &El) -> bool;
    /// Are the children of the innermost element of `open` handed on? Text directly inside it is
    /// dropped (a reader of chunks never sees it).
    fn holds_chunks(&self, open: &[El]) -> bool;
    /// An element handed on, with its open ancestors (what has been read of them so far).
    fn chunk(&mut self, open: &[El], el: El) -> Result<(), DocxError>;
}

/// Known namespaces and the canonical prefix we use for them (read and write).
pub const NAMESPACES: &[(&str, &str)] = &[
    ("w", "http://schemas.openxmlformats.org/wordprocessingml/2006/main"),
    ("r", "http://schemas.openxmlformats.org/officeDocument/2006/relationships"),
    ("wp", "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"),
    ("a", "http://schemas.openxmlformats.org/drawingml/2006/main"),
    ("c", "http://schemas.openxmlformats.org/drawingml/2006/chart"),
    ("pic", "http://schemas.openxmlformats.org/drawingml/2006/picture"),
    ("mc", "http://schemas.openxmlformats.org/markup-compatibility/2006"),
    ("w14", "http://schemas.microsoft.com/office/word/2010/wordml"),
    ("wpi", "http://schemas.microsoft.com/office/word/2010/wordprocessingInk"),
    ("w15", "http://schemas.microsoft.com/office/word/2012/wordml"),
    ("wp14", "http://schemas.microsoft.com/office/word/2010/wordprocessingDrawing"),
    ("wps", "http://schemas.microsoft.com/office/word/2010/wordprocessingShape"),
    ("wpg", "http://schemas.microsoft.com/office/word/2010/wordprocessingGroup"),
    ("wpc", "http://schemas.microsoft.com/office/word/2010/wordprocessingCanvas"),
    ("dgm", "http://schemas.openxmlformats.org/drawingml/2006/diagram"),
    ("dsp", "http://schemas.microsoft.com/office/drawing/2008/diagram"),
    ("m", "http://schemas.openxmlformats.org/officeDocument/2006/math"),
    ("v", "urn:schemas-microsoft-com:vml"),
    ("o", "urn:schemas-microsoft-com:office:office"),
    ("w10", "urn:schemas-microsoft-com:office:word"),
    // Office 2016 charts (`cx:chart`), and the namespaces their `mc:Choice` requires.
    ("cx", "http://schemas.microsoft.com/office/drawing/2014/chartex"),
    ("cx1", "http://schemas.microsoft.com/office/drawing/2015/9/8/chartex"),
    ("cx2", "http://schemas.microsoft.com/office/drawing/2015/10/21/chartex"),
    ("cx3", "http://schemas.microsoft.com/office/drawing/2016/5/9/chartex"),
    ("cx4", "http://schemas.microsoft.com/office/drawing/2016/5/10/chartex"),
    ("cx5", "http://schemas.microsoft.com/office/drawing/2016/5/11/chartex"),
    ("cx6", "http://schemas.microsoft.com/office/drawing/2016/5/12/chartex"),
    ("cx7", "http://schemas.microsoft.com/office/drawing/2016/5/13/chartex"),
    ("cx8", "http://schemas.microsoft.com/office/drawing/2016/5/14/chartex"),
];

/// The `a:ext` URI under which a drawing is marked decorative ([MS-ODRAWXML]), and the
/// namespace of its `adec:decorative` element.
pub const DECORATIVE_EXT: &str = "{C183D7F6-B498-43B3-948B-1728B52AA6E4}";
pub const DECORATIVE_NS: &str = "http://schemas.microsoft.com/office/drawing/2017/decorative";

/// Namespaces that only appear in non-body parts.
const OTHER_NAMESPACES: &[(&str, &str)] = &[
    ("b", "http://schemas.openxmlformats.org/officeDocument/2006/bibliography"),
    ("rel", "http://schemas.openxmlformats.org/package/2006/relationships"),
    ("ct", "http://schemas.openxmlformats.org/package/2006/content-types"),
    ("cp", "http://schemas.openxmlformats.org/package/2006/metadata/core-properties"),
    ("dc", "http://purl.org/dc/elements/1.1/"),
    ("dcterms", "http://purl.org/dc/terms/"),
    ("xsi", "http://www.w3.org/2001/XMLSchema-instance"),
    ("xml", "http://www.w3.org/XML/1998/namespace"),
    ("ep", "http://schemas.openxmlformats.org/officeDocument/2006/extended-properties"),
    ("op", "http://schemas.openxmlformats.org/officeDocument/2006/custom-properties"),
    ("vt", "http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes"),
];

/// ISO/IEC 29500 Strict namespaces map onto the transitional prefixes.
const STRICT: &[(&str, &str)] = &[
    ("w", "http://purl.oclc.org/ooxml/wordprocessingml/main"),
    ("r", "http://purl.oclc.org/ooxml/officeDocument/relationships"),
    ("wp", "http://purl.oclc.org/ooxml/drawingml/wordprocessingDrawing"),
    ("a", "http://purl.oclc.org/ooxml/drawingml/main"),
    ("pic", "http://purl.oclc.org/ooxml/drawingml/picture"),
    ("m", "http://purl.oclc.org/ooxml/officeDocument/math"),
];

fn prefix_for(uri: &[u8], extra: &[(&'static str, &str)]) -> &'static str {
    if let Some((p, _)) = extra.iter().find(|(_, u)| u.as_bytes() == uri) {
        return p;
    }
    for (p, u) in NAMESPACES.iter().chain(OTHER_NAMESPACES).chain(STRICT) {
        if u.as_bytes() == uri {
            return p;
        }
    }
    "?"
}

/// Compatibility feature namespaces whose elements are not otherwise handled by this tree.
/// Keep the reader's existing feature set when canonicalizing `mc:Choice/@Requires` aliases.
fn requirement_prefix(uri: &[u8]) -> &'static str {
    let prefix = prefix_for(uri, &[]);
    if prefix != "?" {
        return prefix;
    }
    match uri {
        b"http://schemas.microsoft.com/office/drawing/2010/main" => "a14",
        b"http://schemas.microsoft.com/office/word/2015/wordml/symex" => "w16se",
        b"http://schemas.microsoft.com/office/word/2016/wordml/cid" => "w16cid",
        b"http://schemas.microsoft.com/office/word/2018/wordml" => "w16",
        b"http://schemas.microsoft.com/office/word/2018/wordml/cex" => "w16cex",
        b"http://schemas.microsoft.com/office/word/2020/wordml/sdtdatahash" => "w16sdtdh",
        _ => "?",
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    El(El),
    Text(String),
}

/// An element with canonical-prefixed name and attributes.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct El {
    pub name: Name,
    pub attrs: Vec<(Name, String)>,
    pub kids: Vec<Node>,
}

impl El {
    /// Attribute by canonical name (`w:val`); falls back to an unqualified attribute with the
    /// same local name (some producers omit the prefix).
    pub fn attr(&self, name: &str) -> Option<&str> {
        if let Some((_, v)) = self.attrs.iter().find(|(k, _)| k == name) {
            return Some(v);
        }
        let local = name.rsplit(':').next().unwrap_or(name);
        if local != name {
            return self.attrs.iter().find(|(k, _)| k == local).map(|(_, v)| v.as_str());
        }
        None
    }
    /// Child elements.
    pub fn els(&self) -> impl Iterator<Item = &El> {
        self.kids.iter().filter_map(|n| match n {
            Node::El(e) => Some(e),
            Node::Text(_) => None,
        })
    }
    pub fn child(&self, name: &str) -> Option<&El> {
        self.els().find(|e| e.name == name)
    }
    pub fn children<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a El> + 'a {
        self.els().filter(move |e| e.name == name)
    }
    /// `w:val` of a child.
    pub fn child_val(&self, name: &str) -> Option<&str> {
        self.child(name).and_then(|c| c.attr("w:val"))
    }
    /// Direct text content.
    pub fn text(&self) -> String {
        let mut s = String::new();
        for n in &self.kids {
            if let Node::Text(t) = n {
                s.push_str(t);
            }
        }
        s
    }
    /// All descendant text (depth-bounded by the parser).
    pub fn deep_text(&self) -> String {
        let mut s = String::new();
        fn go(e: &El, s: &mut String, depth: usize) {
            if depth > MAX_DEPTH {
                return;
            }
            for n in &e.kids {
                match n {
                    Node::Text(t) => s.push_str(t),
                    Node::El(c) => go(c, s, depth + 1),
                }
            }
        }
        go(self, &mut s, 0);
        s
    }
    /// First descendant with this name (depth-first, bounded).
    pub fn find(&self, name: &str) -> Option<&El> {
        fn go<'a>(e: &'a El, name: &str, depth: usize) -> Option<&'a El> {
            if depth > MAX_DEPTH {
                return None;
            }
            for c in e.els() {
                if c.name == name {
                    return Some(c);
                }
                if let Some(f) = go(c, name, depth + 1) {
                    return Some(f);
                }
            }
            None
        }
        go(self, name, 0)
    }
    /// The element as XML with canonical prefixes (for re-emitting kept content). Elements and
    /// attributes in unknown namespaces, or whose names aren't XML names, are dropped.
    pub fn to_xml(&self) -> String {
        fn go(e: &El, s: &mut String, depth: usize) {
            if depth > MAX_DEPTH || !is_qname(&e.name) {
                return;
            }
            s.push('<');
            s.push_str(&e.name);
            for (k, v) in &e.attrs {
                if !is_qname(k) {
                    continue;
                }
                s.push(' ');
                s.push_str(k);
                s.push_str("=\"");
                s.push_str(&esc(v));
                s.push('"');
            }
            if e.kids.is_empty() {
                s.push_str("/>");
                return;
            }
            s.push('>');
            for n in &e.kids {
                match n {
                    Node::Text(t) => s.push_str(&esc(t)),
                    Node::El(c) => go(c, s, depth + 1),
                }
            }
            s.push_str("</");
            s.push_str(&e.name);
            s.push('>');
        }
        let mut s = String::new();
        go(self, &mut s, 0);
        s
    }
    /// Local part of the name.
    pub fn local(&self) -> &str {
        self.name.rsplit(':').next().unwrap_or(&self.name)
    }
}

/// UTF-16 LE/BE text with a byte-order mark as UTF-8 (`None` without one). Lossy: an unpaired
/// surrogate becomes U+FFFD and a trailing odd byte is dropped. The result is at most 1.5× the input.
pub(crate) fn utf16_to_utf8(bytes: &[u8]) -> Option<String> {
    let (rest, le) = match bytes.get(..2) {
        Some([0xFF, 0xFE]) => (bytes.get(2..)?, true),
        Some([0xFE, 0xFF]) => (bytes.get(2..)?, false),
        _ => return None,
    };
    let units = rest.as_chunks::<2>().0.iter().map(|c| if le { u16::from_le_bytes(*c) } else { u16::from_be_bytes(*c) });
    Some(char::decode_utf16(units).map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER)).collect())
}

/// Parse XML bytes into the root element.
pub fn parse(bytes: &[u8]) -> Result<El, DocxError> {
    parse_ns(bytes, &[])
}

/// [`parse`] with `extra` namespaces (prefix, URI) mapped to canonical prefixes first, so a
/// related vocabulary (WordprocessingML 2003) reads with the prefixes the reader knows.
pub fn parse_ns(bytes: &[u8], extra: &[(&'static str, &str)]) -> Result<El, DocxError> {
    parse_limited(bytes, extra, MAX_ELEMENTS).map(|(root, _)| root)
}

/// [`parse_ns`] of a part of a file whose parts share `budget`: the nodes read are taken from
/// it while the tree is held, and a part that doesn't fit what's left is refused and spends it.
pub fn parse_budgeted<'b>(bytes: &[u8], extra: &[(&'static str, &str)], budget: &'b NodeBudget) -> Result<Tree<'b>, DocxError> {
    parse_charged(bytes, extra, budget, None, Strictness::Lenient)
}

/// [`parse_budgeted`] for a part whose loss would be silent data loss (the main document): a
/// mismatched end tag or an element left open at the end of the data is an error, not a cue to
/// stop reading (#3599). The other parts stay lenient on purpose: a broken style, numbering or
/// settings part costs formatting, never text.
#[cfg(test)]
pub fn parse_budgeted_strict<'b>(bytes: &[u8], budget: &'b NodeBudget) -> Result<Tree<'b>, DocxError> {
    parse_charged(bytes, &[], budget, None, Strictness::Strict)
}

/// [`parse_budgeted`] handing `chunks` the elements it asks for as soon as each is read. Those
/// are charged to `budget` only while they are read; the tree returned holds the rest.
pub fn parse_chunks<'b>(
    bytes: &[u8],
    extra: &[(&'static str, &str)],
    budget: &'b NodeBudget,
    chunks: &mut dyn Chunks,
) -> Result<Tree<'b>, DocxError> {
    parse_charged(bytes, extra, budget, Some(chunks), Strictness::Lenient)
}

/// [`parse_chunks`] as strict as [`parse_budgeted_strict`].
pub fn parse_chunks_strict<'b>(bytes: &[u8], budget: &'b NodeBudget, chunks: &mut dyn Chunks) -> Result<Tree<'b>, DocxError> {
    parse_charged(bytes, &[], budget, Some(chunks), Strictness::Strict)
}

/// What a parse does with markup that isn't well formed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Strictness {
    /// An end tag closes the innermost open element whatever its name, and elements still
    /// open at the end of the data are closed: the part reads as far as it goes.
    Lenient,
    /// A mismatched end tag or an element left open is an error.
    Strict,
}

/// A part's tree, its nodes charged to the file's budget while it is held (given back when it
/// is dropped), so the budget bounds the nodes a file holds at once.
#[derive(Debug)]
pub struct Tree<'b> {
    el: El,
    charge: usize,
    budget: &'b NodeBudget,
}

impl Tree<'_> {
    /// The tree to hold for good (its nodes stay charged).
    pub fn keep(mut self) -> El {
        self.charge = 0;
        std::mem::take(&mut self.el)
    }
}

impl std::ops::Deref for Tree<'_> {
    type Target = El;
    fn deref(&self) -> &El {
        &self.el
    }
}

impl Drop for Tree<'_> {
    fn drop(&mut self) {
        self.budget.give(self.charge);
    }
}

fn parse_charged<'b>(
    bytes: &[u8],
    extra: &[(&'static str, &str)],
    budget: &'b NodeBudget,
    chunks: Option<&mut dyn Chunks>,
    strictness: Strictness,
) -> Result<Tree<'b>, DocxError> {
    match parse_nodes(bytes, extra, budget, chunks, strictness) {
        Ok((el, charge)) => Ok(Tree { el, charge, budget }),
        Err(Refused::TooManyNodes) => {
            budget.left.set(0);
            budget.spent.set(true);
            Err(too_many_nodes())
        }
        Err(Refused::Other(e)) => Err(e),
    }
}

/// The error a file refused for its node count gives.
pub fn too_many_nodes() -> DocxError {
    DocxError::Limit(format!("the file's XML needs more than {MAX_FILE_NODES} elements, attributes and text nodes at once"))
}

/// Why a parse stopped.
enum Refused {
    /// More nodes than allowed.
    TooManyNodes,
    Other(DocxError),
}

impl From<DocxError> for Refused {
    fn from(e: DocxError) -> Refused {
        Refused::Other(e)
    }
}

/// [`parse_ns`] allowing at most `max_nodes` elements, attributes and text nodes together; also
/// gives the number of nodes read.
pub(crate) fn parse_limited(bytes: &[u8], extra: &[(&'static str, &str)], max_nodes: usize) -> Result<(El, usize), DocxError> {
    parse_nodes(bytes, extra, &NodeBudget::new(max_nodes), None, Strictness::Lenient).map_err(|e| match e {
        Refused::TooManyNodes => DocxError::Limit(format!("more than {max_nodes} XML elements, attributes and text nodes")),
        Refused::Other(e) => e,
    })
}

/// Names met in one part, each allocated once ([`MAX_SHARED_NAMES`] of them at most).
#[derive(Default)]
struct Names {
    /// Element names by prefix, `:` and raw local name (or the raw local name alone, when not
    /// in a namespace).
    elements: HashMap<Box<[u8]>, Name>,
    /// Attribute names the same way; `None`: not an XML name, so the attribute is dropped.
    attributes: HashMap<Box<[u8]>, Option<Name>>,
    key: Vec<u8>,
}

impl Names {
    fn set_key(&mut self, prefix: Option<&str>, local: &[u8]) {
        self.key.clear();
        if let Some(p) = prefix {
            self.key.extend_from_slice(p.as_bytes());
            self.key.push(b':');
        }
        self.key.extend_from_slice(local);
    }

    /// An element's name, and whether it is shared (false: a name of its own, past
    /// [`MAX_SHARED_NAMES`]). A local name that isn't an XML name is put in the unknown
    /// namespace: nothing reads it and it is never written back.
    fn element(&mut self, prefix: Option<&str>, local: &[u8]) -> (Name, bool) {
        self.set_key(prefix, local);
        if let Some(n) = self.elements.get(self.key.as_slice()) {
            return (n.clone(), true);
        }
        let local = String::from_utf8_lossy(local);
        let name = Name::from(match prefix {
            _ if !is_ncname(&local) => format!("?:{local}"),
            Some(p) => format!("{p}:{local}"),
            None => local.into_owned(),
        });
        let shared = self.elements.len() < MAX_SHARED_NAMES;
        if shared {
            self.elements.insert(self.key.as_slice().into(), name.clone());
        }
        (name, shared)
    }

    /// An attribute's name (`None` when its local name isn't an XML name), and whether it is
    /// shared, as [`Names::element`].
    fn attribute(&mut self, prefix: Option<&str>, local: &[u8]) -> (Option<Name>, bool) {
        self.set_key(prefix, local);
        if let Some(n) = self.attributes.get(self.key.as_slice()) {
            return (n.clone(), true);
        }
        let local = String::from_utf8_lossy(local);
        let name = is_ncname(&local).then(|| {
            Name::from(match prefix {
                Some(p) => format!("{p}:{local}"),
                None => local.into_owned(),
            })
        });
        let shared = self.attributes.len() < MAX_SHARED_NAMES;
        if shared {
            self.attributes.insert(self.key.as_slice().into(), name.clone());
        }
        (name, shared)
    }
}

/// The nodes one parse has taken from its budget and holds.
struct Charge<'b> {
    budget: &'b NodeBudget,
    taken: usize,
}

impl Charge<'_> {
    fn take(&mut self, n: usize) -> Result<(), Refused> {
        if !self.budget.take(n) {
            return Err(Refused::TooManyNodes);
        }
        self.taken += n;
        Ok(())
    }

    /// Give back what was taken since `mark` (a chunk read and let go).
    fn give_back_to(&mut self, mark: usize) {
        let n = self.taken.saturating_sub(mark);
        self.budget.give(n);
        self.taken -= n;
    }
}

/// The reader set up as every tree parse reads: text kept as it is, empty elements left empty,
/// and end tags checked against their start tags only for a strict parse.
fn reader(bytes: &[u8], strictness: Strictness) -> NsReader<&[u8]> {
    let mut r = NsReader::from_reader(bytes);
    let c = r.config_mut();
    c.trim_text_start = false;
    c.trim_text_end = false;
    c.expand_empty_elements = false;
    c.check_end_names = strictness == Strictness::Strict;
    r
}

/// `bytes` as UTF-8 text: UTF-16 (with a BOM) transcoded, a UTF-8 BOM stripped.
fn utf8(bytes: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    match utf16_to_utf8(bytes) {
        Some(s) => std::borrow::Cow::Owned(s.into_bytes()),
        None => std::borrow::Cow::Borrowed(bytes.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(bytes)),
    }
}

/// `mc:Choice`'s `Requires`: its namespace prefixes as canonical ones, resolved where the
/// element is (declarations aren't kept in the tree); `?` for one that isn't known.
fn requirements(r: &NsReader<&[u8]>, val: &str) -> String {
    let requirements: Vec<_> = val.split_whitespace().take(65).collect();
    if requirements.len() > 64 || requirements.iter().any(|prefix| prefix.len() > 128) {
        return "?".to_string();
    }
    requirements
        .into_iter()
        .map(|prefix| {
            let qualified = format!("{prefix}:requirement");
            match r.resolver().resolve_attribute(QName(qualified.as_bytes())).0 {
                ResolveResult::Bound(ns) => requirement_prefix(ns.as_ref()),
                _ => "?",
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The tree of `bytes` (less what `chunks` takes) and the nodes it holds, taken from `budget`.
/// What a parse that fails for another reason took is given back.
fn parse_nodes(
    bytes: &[u8],
    extra: &[(&'static str, &str)],
    budget: &NodeBudget,
    chunks: Option<&mut dyn Chunks>,
    strictness: Strictness,
) -> Result<(El, usize), Refused> {
    let text = utf8(bytes);
    let mut charge = Charge { budget, taken: 0 };
    match read_tree(&mut reader(&text, strictness), extra, &mut charge, chunks, strictness) {
        Ok(root) => Ok((root, charge.taken)),
        Err(e) => {
            charge.give_back_to(0);
            Err(e)
        }
    }
}

fn read_tree(
    r: &mut NsReader<&[u8]>,
    extra: &[(&'static str, &str)],
    charge: &mut Charge,
    mut chunks: Option<&mut dyn Chunks>,
    strictness: Strictness,
) -> Result<El, Refused> {
    let mut stack: Vec<El> = Vec::new();
    // For each open element: what had been taken before it, and whether it is handed on.
    let mut marks: Vec<(usize, bool)> = Vec::new();
    let mut root: Option<El> = None;
    let mut names = Names::default();
    let mut attrs: Vec<(Name, String)> = Vec::new();
    loop {
        // Events borrow from the input: nothing is copied but what the tree keeps.
        let (res, ev) = r.read_resolved_event().map_err(|e| DocxError::Xml(e.to_string()))?;
        let el_prefix = match &res {
            ResolveResult::Bound(ns) => Some(prefix_for(ns.as_ref(), extra)),
            ResolveResult::Unknown(_) if strictness == Strictness::Strict => {
                return Err(DocxError::Xml("the document uses an unbound namespace prefix".into()).into());
            }
            _ => None,
        };
        match ev {
            Event::Start(ref e) | Event::Empty(ref e) => {
                if strictness == Strictness::Strict && root.is_some() && stack.is_empty() {
                    return Err(DocxError::Xml("the document has more than one root element".into()).into());
                }
                let mark = charge.taken;
                let (name, shared) = names.element(el_prefix, e.local_name().as_ref());
                charge.take(node_cost(1, shared))?;
                attrs.clear();
                // Names already on a many-attribute element (a duplicate keeps the first value).
                let mut seen: Option<HashSet<Name>> = None;
                for a in e.attributes().with_checks(strictness == Strictness::Strict) {
                    if attrs.len() >= MAX_ATTRS {
                        break;
                    }
                    let a = match a {
                        Ok(a) => a,
                        Err(e) if strictness == Strictness::Strict => return Err(DocxError::Xml(e.to_string()).into()),
                        Err(_) => continue,
                    };
                    let key = a.key;
                    let raw_key = key.as_ref();
                    if raw_key == b"xmlns" || raw_key.starts_with(b"xmlns:") {
                        continue;
                    }
                    let (ares, alocal) = r.resolver().resolve_attribute(key);
                    let aprefix = match ares {
                        ResolveResult::Bound(ns) => Some(prefix_for(ns.as_ref(), extra)),
                        ResolveResult::Unknown(_) if strictness == Strictness::Strict => {
                            return Err(DocxError::Xml("an attribute uses an unbound namespace prefix".into()).into());
                        }
                        _ => None,
                    };
                    let (aname, shared) = names.attribute(aprefix, alocal.as_ref());
                    let Some(aname) = aname else { continue };
                    let duplicate = if attrs.len() < 16 {
                        attrs.iter().any(|(k, _)| *k == aname)
                    } else {
                        !seen.get_or_insert_with(|| attrs.iter().map(|(k, _)| k.clone()).collect()).insert(aname.clone())
                    };
                    if duplicate {
                        continue;
                    }
                    let val = match a.normalized_value(XmlVersion::Explicit1_0) {
                        Ok(v) => v.into_owned(),
                        Err(_) => String::from_utf8_lossy(&a.value).into_owned(),
                    };
                    // Requires contains namespace prefixes, not feature names. Resolve them while
                    // the namespace scope still exists; declarations are omitted from this tree.
                    let val = if name == "mc:Choice" && aname == "Requires" { requirements(r, &val) } else { val };
                    charge.take(node_cost(1, shared))?;
                    attrs.push((aname, val));
                }
                // Exactly as long as it is (most elements have one or two attributes); the
                // scratch list keeps its room for the next element.
                let mut own = Vec::with_capacity(attrs.len());
                own.append(&mut attrs);
                let el = El { name, attrs: own, kids: Vec::new() };
                let chunk = chunks.as_mut().is_some_and(|c| c.is_chunk(&stack, &el));
                if matches!(ev, Event::Start(_)) {
                    if stack.len() >= MAX_DEPTH {
                        return Err(DocxError::Limit("XML nesting too deep".into()).into());
                    }
                    stack.push(el);
                    marks.push((mark, chunk));
                } else {
                    finish(&mut stack, &mut root, el, (mark, chunk), &mut chunks, charge)?;
                }
            }
            Event::End(_) => {
                if let Some(el) = stack.pop() {
                    let mark = marks.pop().unwrap_or((charge.taken, false));
                    finish(&mut stack, &mut root, el, mark, &mut chunks, charge)?;
                }
            }
            Event::Text(t) => {
                if strictness == Strictness::Strict && stack.is_empty() && !t.decode().map_err(|e| DocxError::Xml(e.to_string()))?.trim().is_empty() {
                    return Err(DocxError::Xml("text outside the document root".into()).into());
                }
                if text_kept(&stack, &chunks)
                    && let Some(top) = stack.last_mut()
                {
                    let s = t.decode().map_err(|e| DocxError::Xml(e.to_string()))?;
                    push_text(top, &s, charge)?;
                }
            }
            Event::CData(t) => {
                if strictness == Strictness::Strict && stack.is_empty() {
                    return Err(DocxError::Xml("CDATA outside the document root".into()).into());
                }
                if text_kept(&stack, &chunks)
                    && let Some(top) = stack.last_mut()
                {
                    let s = String::from_utf8_lossy(&t);
                    push_text(top, &s, charge)?;
                }
            }
            Event::GeneralRef(g) => {
                if strictness == Strictness::Strict && stack.is_empty() {
                    return Err(DocxError::Xml("a character reference outside the document root".into()).into());
                }
                if text_kept(&stack, &chunks)
                    && let Some(top) = stack.last_mut()
                {
                    let ch = match g.resolve_char_ref() {
                        Ok(Some(c)) => Some(c.to_string()),
                        Ok(None) => {
                            let n = g.decode().map_err(|e| DocxError::Xml(e.to_string()))?;
                            quick_xml::escape::resolve_predefined_entity(&n).map(str::to_string)
                        }
                        Err(_) => None,
                    };
                    if let Some(c) = ch {
                        push_text(top, &c, charge)?;
                    } else if strictness == Strictness::Strict {
                        return Err(DocxError::Xml("an invalid or unknown character reference in the document".into()).into());
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
        if strictness == Strictness::Lenient && root.is_some() && stack.is_empty() {
            break;
        }
    }
    // Unclosed elements at EOF: an error for the main document (#3599: a cut-off part would
    // otherwise open as a silently shorter document); closed for the other parts (lenient).
    if strictness == Strictness::Strict
        && let Some(open) = stack.last()
    {
        return Err(DocxError::Xml(format!("the document ends inside an unclosed <{}> element", open.name)).into());
    }
    while let Some(el) = stack.pop() {
        let mark = marks.pop().unwrap_or((charge.taken, false));
        finish(&mut stack, &mut root, el, mark, &mut chunks, charge)?;
    }
    root.ok_or_else(|| DocxError::Xml("no root element".into()).into())
}

/// Is text inside the innermost open element kept (not inside an element whose children are
/// handed on)?
fn text_kept(stack: &[El], chunks: &Option<&mut dyn Chunks>) -> bool {
    !stack.is_empty() && !chunks.as_ref().is_some_and(|c| c.holds_chunks(stack))
}

/// What a node costs the budget: its own (an element, an attribute, or a text node at
/// [`TEXT_NODE_COST`]), and one more when its name isn't shared (a name allocated for it alone).
fn node_cost(own: usize, shared_name: bool) -> usize {
    own + usize::from(!shared_name)
}

/// A text node costs the budget two nodes: it takes an element's room in its parent's list,
/// and allocates its text besides.
const TEXT_NODE_COST: usize = 2;

/// Add text to the open element `top`: to its last text node, or as a new one taken from the
/// budget.
fn push_text(top: &mut El, s: &str, charge: &mut Charge) -> Result<(), Refused> {
    if let Some(Node::Text(prev)) = top.kids.last_mut() {
        prev.push_str(s);
    } else {
        charge.take(node_cost(TEXT_NODE_COST, true))?;
        top.kids.push(Node::Text(s.to_string()));
    }
    Ok(())
}

/// An element just ended: handed on (and what it took given back once it has been read), or
/// attached to its parent (or made the root), first cutting its lists to their length: a list
/// grown a node at a time keeps up to as much room again spare.
fn finish(
    stack: &mut [El],
    root: &mut Option<El>,
    mut el: El,
    (mark, chunk): (usize, bool),
    chunks: &mut Option<&mut dyn Chunks>,
    charge: &mut Charge,
) -> Result<(), Refused> {
    if chunk && let Some(c) = chunks.as_mut() {
        c.chunk(stack, el)?;
        charge.give_back_to(mark);
        return Ok(());
    }
    el.kids.shrink_to_fit();
    for k in &mut el.kids {
        if let Node::Text(t) = k {
            t.shrink_to_fit();
        }
    }
    match stack.last_mut() {
        Some(parent) => parent.kids.push(Node::El(el)),
        None => {
            if root.is_none() {
                *root = Some(el);
            }
        }
    }
    Ok(())
}

/// What [`scan`] reports: an element starts (its canonical name, and an `mc:Choice`'s resolved
/// `Requires`), or the innermost open one ends.
pub enum Scan<'a> {
    Start(&'a Name, Option<&'a str>),
    End,
}

/// Walk the elements of `bytes` as [`parse`] reads them (canonical names, `mc:Choice`
/// requirements resolved), building nothing: for counting what a part holds before reading it.
/// Nesting is limited as [`parse`] limits it.
pub fn scan(bytes: &[u8], f: &mut dyn FnMut(Scan<'_>)) -> Result<(), DocxError> {
    let text = utf8(bytes);
    let mut r = reader(&text, Strictness::Lenient);
    let mut names = Names::default();
    let mut depth = 0usize;
    let mut started = false;
    loop {
        let (res, ev) = r.read_resolved_event().map_err(|e| DocxError::Xml(e.to_string()))?;
        let prefix = match &res {
            ResolveResult::Bound(ns) => Some(prefix_for(ns.as_ref(), &[])),
            _ => None,
        };
        match ev {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let (name, _) = names.element(prefix, e.local_name().as_ref());
                let requires = if name == "mc:Choice" {
                    e.attributes().with_checks(false).flatten().find(|a| a.key.as_ref() == b"Requires").map(|a| {
                        let val = a.normalized_value(XmlVersion::Explicit1_0).map(|v| v.into_owned()).unwrap_or_default();
                        requirements(&r, &val)
                    })
                } else {
                    None
                };
                f(Scan::Start(&name, requires.as_deref()));
                started = true;
                if matches!(ev, Event::Start(_)) {
                    if depth >= MAX_DEPTH {
                        return Err(DocxError::Limit("XML nesting too deep".into()));
                    }
                    depth += 1;
                } else {
                    f(Scan::End);
                }
            }
            Event::End(_) if depth > 0 => {
                depth -= 1;
                f(Scan::End);
            }
            Event::Eof => break,
            _ => {}
        }
        if started && depth == 0 {
            break;
        }
    }
    for _ in 0..depth {
        f(Scan::End);
    }
    Ok(())
}

/// Can `c` start an XML name (XML 1.0 fifth edition `NameStartChar`, without `:`)?
fn name_start(c: char) -> bool {
    matches!(c,
        'A'..='Z' | '_' | 'a'..='z' | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}' | '\u{F8}'..='\u{2FF}' | '\u{370}'..='\u{37D}'
        | '\u{37F}'..='\u{1FFF}' | '\u{200C}'..='\u{200D}' | '\u{2070}'..='\u{218F}' | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}'
        | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}' | '\u{10000}'..='\u{EFFFF}')
}

/// Can `c` continue an XML name (`NameChar`, without `:`)?
fn name_char(c: char) -> bool {
    name_start(c) || matches!(c, '-' | '.' | '0'..='9' | '\u{B7}' | '\u{300}'..='\u{36F}' | '\u{203F}'..='\u{2040}')
}

/// Is `s` an XML namespace `NCName` (a name without a colon)?
pub fn is_ncname(s: &str) -> bool {
    let mut c = s.chars();
    c.next().is_some_and(name_start) && c.all(name_char)
}

/// Is `s` a `QName`: an `NCName`, or `prefix:local` with both parts `NCName`s?
pub fn is_qname(s: &str) -> bool {
    match s.split_once(':') {
        Some((p, l)) => is_ncname(p) && is_ncname(l),
        None => is_ncname(s),
    }
}

/// The number of top-level elements in `xml`, a markup fragment to be written verbatim inside an
/// element that declares the namespaces `ns` (prefix, URI); `None` unless it is strictly
/// well-formed there. Strict: end tags match, every element is closed, attribute names are unique
/// QNames with bound prefixes, references resolve to XML characters, no declarations, processing
/// instructions or DTDs, and only whitespace between top-level elements. Unlike [`parse`], nothing
/// is repaired: a fragment that fails is left out rather than written.
pub fn fragment_roots(xml: &str, ns: &[(&str, &str)]) -> Option<usize> {
    if !xml.chars().all(xml_char_ok) {
        return None;
    }
    const WRAP: &str = "wordcraft-fragment";
    let mut doc = format!("<{WRAP}");
    for (p, u) in ns {
        doc.push_str(&format!(" xmlns:{p}=\"{}\"", esc(u)));
    }
    doc.push('>');
    doc.push_str(xml);
    doc.push_str(&format!("</{WRAP}>"));
    let mut r = NsReader::from_str(&doc);
    {
        let c = r.config_mut();
        c.trim_text_start = false;
        c.trim_text_end = false;
        c.expand_empty_elements = false;
        c.check_end_names = true;
        c.check_comments = true;
    }
    let (mut depth, mut count, mut roots, mut closed) = (0usize, 0usize, 0usize, false);
    loop {
        let (res, ev) = r.read_resolved_event().ok()?;
        if closed {
            return matches!(ev, Event::Eof).then_some(roots);
        }
        match ev {
            Event::Start(ref e) | Event::Empty(ref e) => {
                count += 1;
                if count > MAX_ELEMENTS || depth > MAX_DEPTH {
                    return None;
                }
                let name = std::str::from_utf8(e.name().as_ref()).ok()?.to_string();
                if !is_qname(&name) || matches!(res, ResolveResult::Unknown(_)) {
                    return None;
                }
                for a in e.attributes() {
                    let a = a.ok()?;
                    let key = std::str::from_utf8(a.key.as_ref()).ok()?;
                    if !is_qname(key) || a.normalized_value(XmlVersion::Explicit1_0).is_err() {
                        return None;
                    }
                    if key != "xmlns" && !key.starts_with("xmlns:") && matches!(r.resolver().resolve_attribute(a.key).0, ResolveResult::Unknown(_)) {
                        return None;
                    }
                }
                if depth == 1 {
                    roots += 1;
                }
                if matches!(ev, Event::Start(_)) {
                    depth += 1;
                } else if depth == 0 {
                    return None;
                }
            }
            Event::End(_) => {
                depth = depth.checked_sub(1)?;
                closed = depth == 0;
            }
            Event::Text(t) => {
                let s = t.decode().ok()?;
                if depth <= 1 && !s.trim().is_empty() {
                    return None;
                }
            }
            Event::CData(_) => {
                if depth <= 1 {
                    return None;
                }
            }
            Event::GeneralRef(g) => {
                if depth <= 1 {
                    return None;
                }
                match g.resolve_char_ref() {
                    Ok(Some(c)) if xml_char_ok(c) => {}
                    Ok(None) => {
                        quick_xml::escape::resolve_predefined_entity(&g.decode().ok()?)?;
                    }
                    _ => return None,
                }
            }
            Event::Comment(_) => {}
            Event::Decl(_) | Event::PI(_) | Event::DocType(_) | Event::Eof => return None,
        }
    }
}

/// Is `xml` a strictly well-formed fragment where `ns` is declared (see [`fragment_roots`])?
pub fn fragment_ok(xml: &str, ns: &[(&str, &str)]) -> bool {
    fragment_roots(xml, ns).is_some()
}

/// Is `c` allowed in XML 1.0 text?
fn xml_char_ok(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}')
}

/// Escape text for element content or attribute values (drops characters XML can't carry).
pub fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&apos;"),
            '\t' => o.push_str("&#9;"),
            '\n' => o.push_str("&#10;"),
            '\r' => o.push_str("&#13;"),
            c if xml_char_ok(c) => o.push(c),
            _ => {}
        }
    }
    o
}

/// A string-building XML writer.
#[derive(Default)]
pub struct W {
    pub s: String,
}

impl W {
    pub fn new() -> W {
        W { s: String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n") }
    }
    fn attrs(&mut self, attrs: &[(&str, &str)]) {
        for (k, v) in attrs {
            self.s.push(' ');
            self.s.push_str(k);
            self.s.push_str("=\"");
            self.s.push_str(&esc(v));
            self.s.push('"');
        }
    }
    pub fn open(&mut self, name: &str, attrs: &[(&str, &str)]) {
        self.s.push('<');
        self.s.push_str(name);
        self.attrs(attrs);
        self.s.push('>');
    }
    pub fn empty(&mut self, name: &str, attrs: &[(&str, &str)]) {
        self.s.push('<');
        self.s.push_str(name);
        self.attrs(attrs);
        self.s.push_str("/>");
    }
    pub fn close(&mut self, name: &str) {
        self.s.push_str("</");
        self.s.push_str(name);
        self.s.push('>');
    }
    pub fn text(&mut self, t: &str) {
        self.s.push_str(&esc(t));
    }
    /// `<name>text</name>`.
    pub fn leaf(&mut self, name: &str, attrs: &[(&str, &str)], t: &str) {
        self.open(name, attrs);
        self.text(t);
        self.close(name);
    }
    /// `<name w:val="v"/>`.
    pub fn val(&mut self, name: &str, v: &str) {
        self.empty(name, &[("w:val", v)]);
    }
    pub fn raw(&mut self, s: &str) {
        self.s.push_str(s);
    }
    pub fn into_bytes(self) -> Vec<u8> {
        self.s.into_bytes()
    }
}

/// `xmlns:` declarations for the body-part namespaces, plus `mc:Ignorable`.
pub fn body_ns() -> Vec<(Name, String)> {
    let mut v: Vec<(Name, String)> = NAMESPACES.iter().map(|(p, u)| (Name::from(format!("xmlns:{p}")), (*u).to_string())).collect();
    v.push(("mc:Ignorable".into(), "w14 w15 wp14".into()));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_parts_parse_and_odd_ones_never_panic() {
        let src = r#"<?xml version="1.0" encoding="UTF-16"?><r:Relationships xmlns:r="urn:x"><r:Relationship Id="rId1" Target="café😀.xml"/></r:Relationships>"#;
        for le in [true, false] {
            let mut b = if le { vec![0xFF, 0xFE] } else { vec![0xFE, 0xFF] };
            b.extend(src.encode_utf16().flat_map(|u| if le { u.to_le_bytes() } else { u.to_be_bytes() }));
            let root = parse(&b).unwrap();
            assert_eq!(root.els().next().and_then(|e| e.attr("Target")), Some("café😀.xml"), "le {le}");
        }
        // Odd length, unpaired surrogates, a bare BOM: an error or a tree, never a panic.
        for b in
            [&[0xFF, 0xFE][..], &[0xFE, 0xFF, 0x00], &[0xFF, 0xFE, b'<', 0, 0x00, 0xD8, b'a', 0, b'/', 0, b'>', 0], &[0xFF, 0xFE, 0x00, 0xDC, 0x00]]
        {
            let _ = parse(b);
        }
        assert_eq!(utf16_to_utf8(&[0xFF, 0xFE, b'a', 0, 0x00, 0xD8, b'b']).as_deref(), Some("a\u{FFFD}"));
        assert_eq!(utf16_to_utf8(b"<a/>"), None);
    }

    #[test]
    fn parses_with_namespaces() {
        let x = br#"<?xml version="1.0"?><x:document xmlns:x="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:rr="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><x:p rr:id="r1" x:val="a&amp;b"><x:t xml:space="preserve"> hi &#65;&lt; </x:t></x:p></x:document>"#;
        let e = parse(x).unwrap();
        assert_eq!(e.name, "w:document");
        let p = e.child("w:p").unwrap();
        assert_eq!(p.attr("r:id"), Some("r1"));
        assert_eq!(p.attr("w:val"), Some("a&b"));
        let t = p.child("w:t").unwrap();
        assert_eq!(t.text(), " hi A< ");
        assert_eq!(t.attr("xml:space"), Some("preserve"));
    }

    #[test]
    fn choice_requirements_resolve_namespace_aliases_and_rebindings() {
        let x = br#"<mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:pen="http://schemas.microsoft.com/office/word/2010/wordprocessingInk"><mc:Choice Requires="pen"/><mc:Choice xmlns:pen="urn:future" Requires="pen"/><mc:Choice Requires="undeclared"/></mc:AlternateContent>"#;
        let e = parse(x).unwrap();
        let values: Vec<_> = e.children("mc:Choice").map(|c| c.attr("Requires")).collect();
        assert_eq!(values, [Some("wpi"), Some("?"), Some("?")]);
    }

    #[test]
    fn deep_nesting_is_error() {
        let mut s = String::new();
        for _ in 0..1000 {
            s.push_str("<a>");
        }
        assert!(parse(s.as_bytes()).is_err());
    }

    /// A start tag with thousands of `xmlns:` declarations is refused instead of allocated
    /// (RUSTSEC-2026-0195: unbounded namespace bindings in the resolver).
    #[test]
    fn too_many_namespace_declarations_is_error() {
        let mut s = String::from("<a xmlns:a0=\"urn:a0\"");
        for i in 1..10_000 {
            s.push_str(&format!(" xmlns:a{i}=\"urn:a{i}\""));
        }
        s.push_str("><b/></a>");
        assert!(parse(s.as_bytes()).is_err());
    }

    #[test]
    fn garbage_is_error_not_panic() {
        assert!(parse(b"").is_err());
        assert!(parse(b"\xff\xfe<").is_err());
        let _ = parse(b"<a><b></a>");
        let _ = parse(b"<a x='1' x='2'/>");
    }

    #[test]
    fn names_that_are_not_xml_names_are_never_written() {
        let x = br#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:foo&x w:v&y="1" w:ok="2"><w:b/></w:foo&x><w:p w:a]b="3" w:val="4"/></w:document>"#;
        let e = parse(x).unwrap();
        let out = e.to_xml();
        assert!(!out.contains('&') && !out.contains("?:"), "{out}");
        assert!(out.contains(r#"<w:p w:val="4"/>"#), "{out}");
        assert_eq!(e.child("w:p").map(|p| p.attrs.len()), Some(1));
        // A tree built in code with bad names writes only its good parts.
        let built = El {
            name: "w:r".into(),
            attrs: vec![("w:a b".into(), "1".into()), ("w:c".into(), "2".into())],
            kids: vec![Node::El(El { name: "1x".into(), ..El::default() })],
        };
        assert_eq!(built.to_xml(), r#"<w:r w:c="2"></w:r>"#);
        assert!(is_qname("w:val") && is_qname("val") && is_qname("é-1.x"));
        assert!(!is_qname("") && !is_qname("a:b:c") && !is_qname(":a") && !is_qname("1a") && !is_qname("a&b") && !is_qname("?:x"));
    }

    #[test]
    fn fragments_are_checked_strictly() {
        for good in [
            "",
            "  ",
            "<w:r/>",
            "<w:r><w:t xml:space=\"preserve\">a &amp; &#65;</w:t></w:r><w:r/>",
            "<x:y xmlns:x=\"urn:x\"/>",
            "<w:r><!-- c --></w:r>",
        ] {
            assert!(fragment_ok(good, NAMESPACES), "{good}");
        }
        for bad in [
            "<w:r>",                            // unclosed (the lenient parser auto-closes)
            "<w:r></w:t>",                      // mismatched end (the lenient parser ignores it)
            "<w:r/></wordcraft-fragment><w:p>", // breaks out of the wrapper
            "</wordcraft-fragment><wordcraft-fragment>",
            "<w:r w:a=\"1\" w:a=\"2\"/>", // duplicate attribute
            "<zz:r/>",                    // unbound prefix
            "<w:r zz:a=\"1\"/>",
            "<w:r>&nbsp;</w:r>", // undefined entity
            "<w:r>&#1;</w:r>",   // not an XML character
            "<w:r>\u{1}</w:r>",
            "text<w:r/>", // text where elements belong
            "<?pi x?>",
            "<!DOCTYPE x>",
            "<w:r><!-- a -- b --></w:r>",
            "<w:foo&x/>",
        ] {
            assert!(!fragment_ok(bad, NAMESPACES), "{bad}");
        }
        assert_eq!(fragment_roots("<w:r/> <w:r><w:t/></w:r>", NAMESPACES), Some(2));
    }

    #[test]
    fn attributes_are_capped_deduplicated_and_counted() {
        let mut s = String::from(r#"<a x="1" x="2" y="3" y="4""#);
        for i in 0..20 {
            s.push_str(&format!(r#" d{i}="a" d{i}="b""#));
        }
        for i in 0..MAX_ATTRS + 500 {
            s.push_str(&format!(r#" n{i}="v""#));
        }
        s.push_str("/>");
        let e = parse(s.as_bytes()).unwrap();
        assert_eq!(e.attrs.len(), MAX_ATTRS);
        assert_eq!((e.attr("x"), e.attr("y"), e.attr("d19")), (Some("1"), Some("3"), Some("a")));
        let names: std::collections::HashSet<&str> = e.attrs.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(names.len(), MAX_ATTRS);
        // Attributes count toward the node budget like elements do.
        assert!(parse_limited(br#"<a b="1" c="2" d="3"/>"#, &[], 4).is_ok());
        assert!(matches!(parse_limited(br#"<a b="1" c="2" d="3" e="4"/>"#, &[], 4), Err(DocxError::Limit(_))));
    }

    #[test]
    fn escapes() {
        assert_eq!(esc("a<b&\"\u{1}"), "a&lt;b&amp;&quot;");
    }

    /// The bytes a tree holds: its nodes' room in their lists, its strings, and each name
    /// allocation once however many nodes share it.
    fn tree_bytes(e: &El) -> usize {
        fn name(n: &Name, seen: &mut HashSet<*const u8>) -> usize {
            if seen.insert(n.as_str().as_ptr()) { 16 + n.len() } else { 0 }
        }
        fn go(e: &El, seen: &mut HashSet<*const u8>) -> usize {
            let mut b =
                name(&e.name, seen) + e.kids.capacity() * std::mem::size_of::<Node>() + e.attrs.capacity() * std::mem::size_of::<(Name, String)>();
            for (k, v) in &e.attrs {
                b += name(k, seen) + v.capacity();
            }
            for k in &e.kids {
                b += match k {
                    Node::El(c) => go(c, seen),
                    Node::Text(t) => t.capacity(),
                };
            }
            b
        }
        std::mem::size_of::<Node>() + go(e, &mut HashSet::new())
    }

    /// A tree costs what its nodes need: a name is allocated once however many elements have it,
    /// and lists and text keep no spare room. Before, each element allocated its own name and a
    /// list grown a node at a time kept room for up to twice as many (at least four): about
    /// 113 bytes an empty element, 360 a typical property element (`<w:sz w:val="24"/>`).
    #[test]
    fn trees_hold_no_spare_room_and_share_their_names() {
        assert!(std::mem::size_of::<Node>() <= 64, "{}", std::mem::size_of::<Node>());
        let n = 10_000;
        let x = format!(
            r#"<w:body xmlns:w="{}">{}</w:body>"#,
            NAMESPACES[0].1,
            r#"<w:p><w:pPr><w:jc w:val="center"/></w:pPr><w:r><w:t>a&amp;b</w:t></w:r></w:p>"#.repeat(n)
        );
        let root = parse(x.as_bytes()).unwrap();
        fn check(e: &El) {
            assert_eq!(e.kids.capacity(), e.kids.len(), "{}", e.name);
            assert_eq!(e.attrs.capacity(), e.attrs.len(), "{}", e.name);
            for k in &e.kids {
                match k {
                    Node::El(c) => check(c),
                    Node::Text(t) => assert_eq!(t.capacity(), t.len(), "{t}"),
                }
            }
        }
        check(&root);
        let (a, b) = (root.els().next().unwrap(), root.els().nth(1).unwrap());
        assert!(Arc::ptr_eq(&a.name.0, &b.name.0), "one allocation for every w:p");
        let (ja, jb) = (a.find("w:jc").unwrap(), b.find("w:jc").unwrap());
        assert!(Arc::ptr_eq(&ja.attrs[0].0.0, &jb.attrs[0].0.0), "one allocation for every w:val");
        assert_eq!(a.find("w:t").map(El::text).as_deref(), Some("a&b"));
        // Six elements, one attribute and one text node a paragraph: at most 480 bytes in all.
        let per_para = tree_bytes(&root) / n;
        assert!(per_para <= 480, "{per_para} bytes a paragraph");
        // An empty element: its room in its parent's list, no more.
        let empty = parse(format!("<a>{}</a>", "<x/>".repeat(n)).as_bytes()).unwrap();
        assert!(tree_bytes(&empty) / n <= std::mem::size_of::<Node>(), "{}", tree_bytes(&empty) / n);
    }

    /// Every node counts against the budget: text nodes as two (their room in the list and
    /// their text), and so does an element or attribute whose name isn't shared. Before, text
    /// nodes were free, so a part could hold twice the nodes the cap meant.
    #[test]
    fn text_nodes_and_names_of_their_own_count() {
        let fits = |x: &str, max: usize| parse_limited(x.as_bytes(), &[], max).map(|(_, n)| n);
        // Root, then four (element, text) pairs: 1 + 4 × 3.
        let x = "<a><x/>y<x/>y<x/>y<x/>y</a>";
        assert_eq!(fits(x, 13), Ok(13));
        assert!(matches!(fits(x, 12), Err(DocxError::Limit(_))));
        // Adjacent text (with a reference between) is one text node.
        assert_eq!(fits("<a>b&amp;c</a>", 3), Ok(3));
        // Names past the shared ones count one more each.
        let distinct: String = (0..MAX_SHARED_NAMES + 10).map(|i| format!("<n{i} a{i}=\"\"/>")).collect();
        let x = format!("<root>{distinct}</root>");
        // The root takes one of the shared element names.
        let own = 11 + 10;
        assert_eq!(fits(&x, usize::MAX), Ok(1 + 2 * (MAX_SHARED_NAMES + 10) + own));
    }

    /// The parts of one file share one budget for the trees held at once: parts that fit one by
    /// one are refused when held together, and the budget stays spent so a reader that skips a
    /// broken part can tell. A tree dropped gives its nodes back; one kept for good doesn't.
    #[test]
    fn the_parts_of_a_file_share_one_budget() {
        let budget = NodeBudget::new(10);
        let part = b"<a><b/><b/><b/></a>";
        let first = parse_budgeted(part, &[], &budget).unwrap();
        assert_eq!(budget.left(), 6);
        drop(parse_budgeted(part, &[], &budget).unwrap());
        assert_eq!(budget.left(), 6, "a tree dropped gives its nodes back");
        let kept = parse_budgeted(part, &[], &budget).unwrap().keep();
        assert_eq!(kept.els().count(), 3);
        assert_eq!(budget.left(), 2, "a tree kept doesn't");
        assert!(!budget.spent());
        assert!(matches!(parse_budgeted(part, &[], &budget), Err(DocxError::Limit(_))));
        assert!(budget.spent());
        drop(first);
        assert!(matches!(parse_budgeted(b"<a/>", &[], &budget), Err(DocxError::Limit(_))));
        assert!(budget.spent());
        // Other errors leave it as it was.
        let budget = NodeBudget::new(1000);
        assert!(parse_budgeted(b"", &[], &budget).is_err());
        assert!(!budget.spent());
        assert!(parse_budgeted("<a>".repeat(MAX_DEPTH + 1).as_bytes(), &[], &budget).is_err());
        assert!(!budget.spent());
    }

    /// Billion laughs: entities a DOCTYPE declares are never expanded (only the five XML
    /// predefines and character references are), so they cost nothing.
    #[test]
    fn declared_entities_are_not_expanded() {
        let mut dtd = String::from(r#"<!DOCTYPE a [<!ENTITY l0 "lol">"#);
        for i in 1..10 {
            dtd.push_str(&format!(r#"<!ENTITY l{i} "{}">"#, format!("&l{};", i - 1).repeat(10)));
        }
        let x = format!("{dtd}]><a>&l9;&lt;&#65;</a>");
        let (root, nodes) = parse_limited(x.as_bytes(), &[], usize::MAX).unwrap();
        assert_eq!(root.text(), "<A");
        assert_eq!(nodes, 3);
    }
}
