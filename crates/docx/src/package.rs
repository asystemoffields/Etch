//! OPC package access: bounded zip reading, relationships, part names.

use std::collections::{BTreeMap, HashMap};
use std::io::{Cursor, Read, Write};
use std::sync::Arc;

use crate::DocxError;
use crate::xml::{self, El, NodeBudget};

/// Largest single part we decompress: more than any part WordCraft writes for a document within
/// the limits it is held to ([`wordcraft_doc::limits::MAX_XML_PART_BYTES`]).
pub const MAX_PART: u64 = wordcraft_doc::limits::MAX_XML_PART_BYTES;
/// Largest total of all decompressed parts: a document within the limits, its pictures and kept
/// parts included ([`wordcraft_doc::limits::MAX_PACKAGE_BYTES`]).
pub const MAX_TOTAL: u64 = wordcraft_doc::limits::MAX_PACKAGE_BYTES;
/// Most zip entries a package may have: one a header or footer, one a picture, the kept parts
/// and the fixed ones ([`wordcraft_doc::limits::MAX_PACKAGE_PARTS`]).
pub const MAX_ENTRIES: usize = wordcraft_doc::limits::MAX_PACKAGE_PARTS;
/// Most relationships one `.rels` part may have (Word's own files have a few hundred; a
/// document within the limits at most [`wordcraft_doc::limits::MAX_PART_RELATIONSHIPS`]). A file
/// with more is refused.
pub const MAX_RELS: usize = wordcraft_doc::limits::MAX_PART_RELATIONSHIPS;

/// Relationship type suffixes (after `.../relationships/`).
pub mod rt {
    pub const OFFICE_DOC: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument";
    pub const CORE: &str = "http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties";
    pub const APP: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties";
    pub const CUSTOM: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/custom-properties";
    pub const CUSTOM_XML_PROPS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/customXmlProps";
    pub const STYLES: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles";
    pub const NUMBERING: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering";
    pub const SETTINGS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings";
    pub const ATTACHED_TEMPLATE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/attachedTemplate";
    pub const FONT_TABLE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/fontTable";
    pub const THEME: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme";
    pub const FOOTNOTES: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/footnotes";
    pub const ENDNOTES: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/endnotes";
    pub const COMMENTS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/comments";
    pub const COMMENTS_EX: &str = "http://schemas.microsoft.com/office/2011/relationships/commentsExtended";
    pub const COMMENTS_IDS: &str = "http://schemas.microsoft.com/office/2016/09/relationships/commentsIds";
    pub const COMMENTS_EXTENSIBLE: &str = "http://schemas.microsoft.com/office/2018/08/relationships/commentsExtensible";
    pub const HEADER: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/header";
    pub const FOOTER: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/footer";
    pub const IMAGE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image";
    pub const HYPERLINK: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink";
    pub const VBA_PROJECT: &str = "http://schemas.microsoft.com/office/2006/relationships/vbaProject";
    pub const CHART: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart";
    pub const DIAGRAM_DATA: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/diagramData";
    pub const DIAGRAM_DRAWING: &str = "http://schemas.microsoft.com/office/2007/relationships/diagramDrawing";
    pub const DIAGRAM_LAYOUT: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/diagramLayout";
    pub const DIAGRAM_QUICK_STYLE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/diagramQuickStyle";
    pub const DIAGRAM_COLORS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/diagramColors";
    pub const CUSTOM_XML: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/customXml";
    pub const GLOSSARY: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/glossaryDocument";
}

/// Content type of a glossary document part (ECMA-376 Part 1 §11.3.8).
pub const GLOSSARY_CONTENT_TYPE: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document.glossary+xml";
/// `Document::passthrough` key: a fingerprint of `Document::glossary` as read, so a save can tell
/// an untouched glossary (written back verbatim) from an edited one (written from the model).
/// Not a package part: WordCraft's own bookkeeping.
pub const GLOSSARY_READ: &str = "wordcraft:glossary.read";
/// Most building blocks read from one glossary.
pub const MAX_GLOSSARY_ENTRIES: usize = 10_000;
/// The glossary entry types `w:type` takes (ST_DocPartType); others aren't kept.
pub const GLOSSARY_TYPES: [&str; 7] = ["none", "normal", "autoExp", "toolbar", "speller", "formFld", "bbPlcHdr"];
/// The insertion behaviours `w:behavior` takes (ST_DocPartBehavior); others aren't kept.
pub const GLOSSARY_BEHAVIORS: [&str; 3] = ["content", "p", "pg"];

/// A fingerprint of `glossary` (FNV-1a over its debug form): stable across runs and builds that
/// keep the model's shape, and cheap (nothing is allocated).
pub fn glossary_fingerprint(glossary: &[wordcraft_doc::BuildingBlock]) -> String {
    struct Fnv(u64);
    impl std::fmt::Write for Fnv {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            for b in s.bytes() {
                self.0 = (self.0 ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
            }
            Ok(())
        }
    }
    let mut h = Fnv(0xcbf2_9ce4_8422_2325);
    // Writing into `Fnv` never fails.
    let _ = std::fmt::Write::write_fmt(&mut h, format_args!("{glossary:?}"));
    format!("{:016x}", h.0)
}

/// `Document::passthrough` key for a macro project, kept as opaque bytes (never parsed or run).
pub const VBA_PROJECT_PART: &str = "word/vbaProject.bin";
/// `Document::passthrough` key listing the parts the project relates to (VBA data, signatures…),
/// one `relationship type \t part name \t content type` line each; each part's bytes are kept
/// under its part name. Not a package part: WordCraft's own bookkeeping.
pub const VBA_RELATED: &str = "wordcraft:vbaProject.related";
/// Most parts we carry along with a macro project.
pub const MAX_VBA_RELATED: usize = 64;

/// `Document::passthrough` key listing the parts that objects read from a file refer to (charts,
/// SmartArt diagrams, OLE objects: see `wordcraft_doc::graphic::Embedded`), with their content
/// types and own relationships, as [`EmbeddedManifest`] lines. Each part's bytes are kept under its
/// part name. Not a package part: WordCraft's own bookkeeping.
pub const EMBEDDED_PARTS: &str = "wordcraft:embedded.parts";
/// The font table kept from a file (read by `Reader::keep_part_tree`), with the fonts it embeds.
pub const FONT_TABLE_PART: &str = "word/fontTable.xml";
/// Custom XML roots kept by the package (P) or main document (D): `owner\ttype\tpart`.
/// Their opaque bytes and related parts use the same bounded manifest as embedded objects.
pub const CUSTOM_XML_ROOTS: &str = "wordcraft:customXml.roots";
/// The original `word/settings.xml` when it holds settings the model doesn't (merged into on
/// save by `write::settings_merge`). Not a package part: WordCraft's own bookkeeping.
pub const SETTINGS_ORIG: &str = "wordcraft:settings.orig";
/// The relationships of that original settings part, for the children a save keeps from it (a
/// mail merge's data source and header source, its recipient data part…): `id\ttype\ttarget\tE|I`
/// lines (`E`: `target` is an external URL; `I`: a part kept with its own relationships in the
/// [`EmbeddedManifest`]). The attached template's isn't listed: the model holds it. Not a package
/// part: WordCraft's own bookkeeping.
pub const SETTINGS_RELS: &str = "wordcraft:settings.rels";
/// Extended properties (`docProps/app.xml`) the model doesn't hold but a save keeps
/// ([`APP_KEPT_PROPS`]): `name\tvalue` lines. Not a package part: WordCraft's own bookkeeping.
pub const APP_KEPT: &str = "wordcraft:app.kept";
/// The `docProps/app.xml` properties carried over from the file. Counts (pages, lines,
/// characters, editing time) and the producing application's version are not: they describe
/// the file Word saved, not the one WordCraft writes.
pub const APP_KEPT_PROPS: &[&str] =
    &["Template", "Manager", "Company", "HyperlinkBase", "ScaleCrop", "LinksUpToDate", "SharedDoc", "HyperlinksChanged"];
/// Parts WordCraft doesn't model, carried verbatim: `owner\ttype\tpart` lines (owner `P` for
/// the package, `D` for the main document), with their bytes and relationships in the
/// [`EmbeddedManifest`]. Not a package part: WordCraft's own bookkeeping.
pub const UNMODELLED_ROOTS: &str = "wordcraft:unmodelled.roots";
/// Most parts we carry along for a file's embedded objects.
pub const MAX_EMBEDDED_PARTS: usize = 4096;
/// Most relationships we keep for one such part.
pub const MAX_EMBEDDED_RELS: usize = 1024;
/// Longest markup we keep for one embedded object.
pub const MAX_EMBEDDED_XML: usize = 1024 * 1024;

/// One part an embedded object needs: its content type and its own relationships (internal
/// targets are part names).
#[derive(Clone, Debug, Default)]
pub struct EmbeddedPart {
    pub content_type: String,
    pub rels: Vec<Rel>,
}

/// The parts kept for embedded objects, by part name (see [`EMBEDDED_PARTS`]). Stored as text:
/// `P\tpart\tcontent type` per part, then `R\tpart\tid\ttype\ttarget\tI|E` per relationship of
/// that part (`E`: `target` is an external URL).
#[derive(Clone, Debug, Default)]
pub struct EmbeddedManifest {
    pub parts: BTreeMap<String, EmbeddedPart>,
}

/// Can `s` be a field of a manifest line?
pub fn manifest_field_ok(s: &str) -> bool {
    !s.is_empty() && !s.contains(['\t', '\r', '\n'])
}

impl EmbeddedManifest {
    pub fn parse(bytes: &[u8]) -> EmbeddedManifest {
        let mut m = EmbeddedManifest::default();
        for line in String::from_utf8_lossy(bytes).lines() {
            let f: Vec<&str> = line.split('\t').collect();
            match f.as_slice() {
                ["P", part, ct] if m.parts.len() < MAX_EMBEDDED_PARTS => {
                    m.parts.entry((*part).to_string()).or_insert_with(|| EmbeddedPart { content_type: (*ct).to_string(), rels: Vec::new() });
                }
                ["R", part, id, kind, target, mode] => {
                    if let Some(p) = m.parts.get_mut(*part)
                        && p.rels.len() < MAX_EMBEDDED_RELS
                        && !p.rels.iter().any(|r| r.id == *id)
                    {
                        p.rels.push(Rel { id: (*id).to_string(), kind: (*kind).to_string(), target: (*target).to_string(), external: *mode == "E" });
                    }
                }
                _ => {}
            }
        }
        m
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut s = String::new();
        for (part, p) in &self.parts {
            if !manifest_field_ok(part) || !manifest_field_ok(&p.content_type) {
                continue;
            }
            s.push_str(&format!("P\t{part}\t{}\n", p.content_type));
        }
        for (part, p) in &self.parts {
            for r in p.rels.iter().filter(|r| [&r.id, &r.kind, &r.target].iter().all(|f| manifest_field_ok(f))) {
                s.push_str(&format!("R\t{part}\t{}\t{}\t{}\t{}\n", r.id, r.kind, r.target, if r.external { "E" } else { "I" }));
            }
        }
        s.into_bytes()
    }
}

/// Does relationship type `t` end with `suffix` (ignoring the transitional/strict prefix)?
pub fn rel_is(t: &str, full: &str) -> bool {
    let tail = |s: &str| s.rsplit('/').next().unwrap_or(s).to_string();
    t == full || tail(t) == tail(full)
}

/// The decompressed parts of a package (bounded). Shared, so a part the document keeps as it is (a
/// picture, an embedded object) is handed over rather than copied.
pub struct Package {
    /// Shared, so a picture read into the document is the package's bytes, not a copy of them
    /// (a document at the limits holds 512 MB of pictures).
    files: BTreeMap<String, Arc<Vec<u8>>>,
    /// Lower-cased part name → stored name (the first in name order): OPC part names compare
    /// case-insensitively, so `WORD/Media/x.png` names the part stored as `word/media/x.png`.
    folded: HashMap<String, String>,
    /// XML nodes the parts may still be read into ([`xml::MAX_FILE_NODES`] for all of them).
    nodes: NodeBudget,
    /// The first limit a part was found past while reading it (more relationships than a part
    /// may have): the file is refused, naming it, rather than read without what is over.
    refused: std::cell::OnceCell<String>,
}

impl Package {
    pub fn open(bytes: &[u8]) -> Result<Package, DocxError> {
        Package::open_limited(bytes, MAX_PART, MAX_TOTAL)
    }

    pub fn open_limited(bytes: &[u8], max_part: u64, max_total: u64) -> Result<Package, DocxError> {
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| DocxError::Zip(e.to_string()))?;
        if zip.len() > MAX_ENTRIES {
            return Err(DocxError::Limit(format!("{} zip entries", zip.len())));
        }
        let mut files = BTreeMap::new();
        let mut total: u64 = 0;
        for i in 0..zip.len() {
            let mut f = match zip.by_index(i) {
                Ok(f) => f,
                Err(e) => {
                    log::warn!("docx: skipping unreadable zip entry {i}: {e}");
                    continue;
                }
            };
            if f.is_dir() {
                continue;
            }
            let name = f.name().trim_start_matches('/').to_string();
            if f.size() > max_part {
                return Err(DocxError::Limit(format!("part {name} declares {} bytes", f.size())));
            }
            let room = max_part.min(max_total.saturating_sub(total));
            let mut buf = Vec::with_capacity(f.size().min(16 * 1024 * 1024) as usize);
            let read = (&mut f).take(room + 1).read_to_end(&mut buf);
            if let Err(e) = read {
                // A corrupt entry: keep going unless it's a part we can't do without.
                log::warn!("docx: zip entry {name} unreadable: {e}");
                continue;
            }
            if buf.len() as u64 > room {
                return Err(DocxError::Limit(format!("part {name} decompresses past the size limit")));
            }
            total = total.saturating_add(buf.len() as u64);
            files.insert(name, buf);
        }
        Ok(Package::from_parts(files))
    }

    /// A package from parts already in memory (a Flat OPC file, a converted document), by part
    /// name without the leading `/`.
    pub fn from_parts(files: BTreeMap<String, Vec<u8>>) -> Package {
        let mut folded = HashMap::with_capacity(files.len());
        for k in files.keys() {
            folded.entry(k.to_ascii_lowercase()).or_insert_with(|| k.clone());
        }
        let files = files.into_iter().map(|(k, v)| (k, Arc::new(v))).collect();
        Package { files, folded, nodes: NodeBudget::file(), refused: std::cell::OnceCell::new() }
    }

    /// A package WordCraft has just written from a document it holds (to convert it): only each
    /// part's own node cap applies, not the file's. The parts come from a document already in
    /// memory, so converting it isn't held to less than saving it as DOCX was.
    pub fn written(files: BTreeMap<String, Vec<u8>>) -> Package {
        Package { nodes: NodeBudget::new(usize::MAX), ..Package::from_parts(files) }
    }

    /// The first limit a part was found past, if any: the file is then refused with it.
    pub fn refused(&self) -> Option<&str> {
        self.refused.get().map(String::as_str)
    }

    /// Did a part not fit the file's XML node budget? The file is then refused, even when the
    /// part was one a reader can do without.
    pub fn nodes_spent(&self) -> bool {
        self.nodes.spent()
    }

    /// XML nodes the parts may still be read into.
    #[cfg(test)]
    pub fn nodes_left(&self) -> usize {
        self.nodes.left()
    }

    /// The package with a node budget of `nodes` (tests use a small one).
    #[cfg(test)]
    pub fn with_node_budget(mut self, nodes: usize) -> Package {
        self.nodes = NodeBudget::new(nodes);
        self
    }

    /// The stored name of the part `name` refers to (leading `/` ignored; an exact match first,
    /// then a case-insensitive one). Caches of per-part work key on this, so case variants of
    /// one name share a single entry.
    pub fn canonical(&self, name: &str) -> Option<&str> {
        let n = name.trim_start_matches('/');
        if let Some((k, _)) = self.files.get_key_value(n) {
            return Some(k);
        }
        self.folded.get(&n.to_ascii_lowercase()).map(String::as_str)
    }

    /// Part bytes by name (leading `/` ignored; case-insensitive fallback).
    pub fn get(&self, name: &str) -> Option<&[u8]> {
        self.files.get(self.canonical(name)?).map(|v| v.as_slice())
    }

    /// Part bytes by name, as [`Package::get`], shared rather than borrowed: a picture the
    /// document keeps holds the package's bytes.
    pub fn get_shared(&self, name: &str) -> Option<Arc<Vec<u8>>> {
        self.files.get(self.canonical(name)?).cloned()
    }

    /// The tree of part `name` (`None` without one); its nodes count against the file's
    /// budget while it is held.
    pub fn xml(&self, name: &str) -> Result<Option<xml::Tree<'_>>, DocxError> {
        match self.get(name) {
            Some(b) => xml::parse_budgeted(b, &[], &self.nodes).map(Some),
            None => Ok(None),
        }
    }

    /// [`Package::xml`] for the main document part: malformed XML (a mismatched end tag, an
    /// element left open by a cut-off part) is an error, never a shorter document (#3599). Only
    /// the unit tests' whole-tree reading takes the main part whole; the reader streams it
    /// ([`Package::xml_chunks_strict`]).
    #[cfg(test)]
    pub fn xml_strict(&self, name: &str) -> Result<Option<xml::Tree<'_>>, DocxError> {
        match self.get(name) {
            Some(b) => xml::parse_budgeted_strict(b, &self.nodes).map(Some),
            None => Ok(None),
        }
    }

    /// [`Package::xml`] handing `chunks` the elements it asks for as each is read (see
    /// [`xml::Chunks`]); the tree returned holds the rest.
    pub fn xml_chunks(&self, name: &str, chunks: &mut dyn xml::Chunks) -> Result<Option<xml::Tree<'_>>, DocxError> {
        match self.get(name) {
            Some(b) => xml::parse_chunks(b, &[], &self.nodes, chunks).map(Some),
            None => Ok(None),
        }
    }

    /// [`Package::xml_chunks`] as strict as [`Package::xml_strict`] (the main document part).
    pub fn xml_chunks_strict(&self, name: &str, chunks: &mut dyn xml::Chunks) -> Result<Option<xml::Tree<'_>>, DocxError> {
        match self.get(name) {
            Some(b) => xml::parse_chunks_strict(b, &self.nodes, chunks).map(Some),
            None => Ok(None),
        }
    }

    /// [`Package::xml`] handing each child of the root to `each` as soon as it has been parsed
    /// (and letting it go), so the part's tree is never held whole; the tree returned is the root
    /// alone.
    pub fn xml_children(&self, name: &str, each: &mut dyn FnMut(El) -> Result<(), DocxError>) -> Result<Option<xml::Tree<'_>>, DocxError> {
        struct Children<'f>(&'f mut dyn FnMut(El) -> Result<(), DocxError>);
        impl xml::Chunks for Children<'_> {
            fn is_chunk(&mut self, open: &[El], _: &El) -> bool {
                open.len() == 1
            }
            fn holds_chunks(&self, open: &[El]) -> bool {
                open.len() == 1
            }
            fn chunk(&mut self, _: &[El], el: El) -> Result<(), DocxError> {
                (self.0)(el)
            }
        }
        self.xml_chunks(name, &mut Children(each))
    }

    /// Relationships of `part` (from `dir/_rels/name.rels`).
    pub fn rels(&self, part: &str) -> Rels {
        self.rels_limited(part, MAX_RELS)
    }

    /// [`Package::rels`] holding at most `max` (tests use a small one): past it the file is
    /// refused ([`Package::refused`]).
    fn rels_limited(&self, part: &str, max: usize) -> Rels {
        let (dir, file) = split_dir(part);
        let path = if dir.is_empty() { format!("_rels/{file}.rels") } else { format!("{dir}/_rels/{file}.rels") };
        let mut rels = Rels::default();
        let root = match self.xml(&path) {
            Ok(Some(root)) => root,
            Ok(None) => return rels,
            Err(e) => {
                log::warn!("docx: ignoring unreadable relationships {path}: {e}");
                return rels;
            }
        };
        for r in root.els().filter(|e| e.local() == "Relationship") {
            if rels.list.len() >= max {
                let _ = self.refused.set(format!("more than {max} relationships in {path}"));
                break;
            }
            let (Some(id), Some(target)) = (r.attr("Id"), r.attr("Target")) else { continue };
            let external = r.attr("TargetMode").is_some_and(|m| m.eq_ignore_ascii_case("External"));
            // An internal target names a part by its stored name, whatever case the file used.
            let resolved = if external {
                target.to_string()
            } else {
                let t = resolve(dir, target);
                self.canonical(&t).map(str::to_string).unwrap_or(t)
            };
            rels.push(Rel { id: id.to_string(), kind: r.attr("Type").unwrap_or("").to_string(), target: resolved, external });
        }
        rels
    }
}

/// `[Content_Types].xml`: Override by part name, then Default by extension (both case-insensitive).
#[derive(Default)]
pub struct ContentTypes {
    overrides: HashMap<String, String>,
    defaults: HashMap<String, String>,
}

impl ContentTypes {
    pub fn read(pkg: &Package) -> ContentTypes {
        let mut ct = ContentTypes::default();
        let root = match pkg.xml("[Content_Types].xml") {
            Ok(Some(root)) => root,
            Ok(None) => return ct,
            Err(e) => {
                log::warn!("docx: ignoring unreadable [Content_Types].xml: {e}");
                return ct;
            }
        };
        for e in root.els() {
            let (Some(key), Some(t)) = (e.attr("PartName").or_else(|| e.attr("Extension")), e.attr("ContentType")) else { continue };
            let map = if e.local() == "Override" { &mut ct.overrides } else { &mut ct.defaults };
            map.entry(key.trim_start_matches('/').to_ascii_lowercase()).or_insert_with(|| t.to_string());
        }
        ct
    }

    /// Content type of `part` (a package path without the leading `/`).
    pub fn of(&self, part: &str) -> Option<&str> {
        let part = part.trim_start_matches('/').to_ascii_lowercase();
        if let Some(t) = self.overrides.get(&part) {
            return Some(t);
        }
        let ext = part.rsplit_once('.').map(|(_, e)| e)?;
        self.defaults.get(ext).map(String::as_str)
    }
}

#[derive(Clone, Debug)]
pub struct Rel {
    pub id: String,
    pub kind: String,
    /// Package path (internal) or URL (external).
    pub target: String,
    pub external: bool,
}

/// A part's relationships in file order, indexed by id (the first of duplicate ids wins).
#[derive(Clone, Debug, Default)]
pub struct Rels {
    list: Vec<Rel>,
    by_id: HashMap<String, usize>,
}

impl Rels {
    fn push(&mut self, r: Rel) {
        self.by_id.entry(r.id.clone()).or_insert(self.list.len());
        self.list.push(r);
    }
    /// All relationships, in file order.
    pub fn list(&self) -> &[Rel] {
        &self.list
    }
    pub fn into_list(self) -> Vec<Rel> {
        self.list
    }
    pub fn by_id(&self, id: &str) -> Option<&Rel> {
        self.list.get(*self.by_id.get(id)?)
    }
    pub fn by_type(&self, full: &str) -> Option<&Rel> {
        self.list.iter().find(|r| rel_is(&r.kind, full))
    }
}

fn split_dir(part: &str) -> (&str, &str) {
    let p = part.trim_start_matches('/');
    match p.rfind('/') {
        Some(i) => (p.get(..i).unwrap_or(""), p.get(i + 1..).unwrap_or(p)),
        None => ("", p),
    }
}

/// Resolve a relative target against a source directory into a package path.
pub fn resolve(dir: &str, target: &str) -> String {
    let target = target.split('#').next().unwrap_or(target);
    let mut parts: Vec<&str> = if target.starts_with('/') { Vec::new() } else { dir.split('/').filter(|s| !s.is_empty()).collect() };
    for seg in target.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

/// The relative reference from part `from` to part `to` (both package paths), as a
/// relationship target: `word/document.xml` → `word/charts/chart1.xml` is `charts/chart1.xml`.
pub fn relative(from: &str, to: &str) -> String {
    let from_dir: Vec<&str> = from.split('/').filter(|s| !s.is_empty()).collect();
    let from_dir = from_dir.get(..from_dir.len().saturating_sub(1)).unwrap_or(&[]);
    let to: Vec<&str> = to.split('/').filter(|s| !s.is_empty()).collect();
    let to_dir = to.get(..to.len().saturating_sub(1)).unwrap_or(&[]);
    let common = from_dir.iter().zip(to_dir).take_while(|(a, b)| a == b).count();
    let mut out: Vec<&str> = vec![".."; from_dir.len() - common];
    out.extend(to.get(common..).unwrap_or(&[]));
    let s = out.join("/");
    // A relative reference whose first segment has a colon would read as a URI scheme.
    if s.split('/').next().is_some_and(|seg| seg.contains(':')) { format!("./{s}") } else { s }
}

/// Build a zip from (name, bytes) entries.
pub fn zip_entries(entries: &[(String, Vec<u8>)]) -> Result<Vec<u8>, DocxError> {
    let mut zw = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).compression_level(Some(6));
    let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, bytes) in entries {
        // Already-compressed media gain nothing from deflate.
        let lower = name.to_ascii_lowercase();
        let o = if [".png", ".jpg", ".jpeg", ".gif", ".webp"].iter().any(|e| lower.ends_with(e)) { stored } else { opts };
        zw.start_file(name.as_str(), o).map_err(|e| DocxError::Zip(e.to_string()))?;
        zw.write_all(bytes).map_err(|e| DocxError::Zip(e.to_string()))?;
    }
    let c = zw.finish().map_err(|e| DocxError::Zip(e.to_string()))?;
    Ok(c.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zip_of(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let e: Vec<(String, Vec<u8>)> = entries.iter().map(|(n, b)| (n.to_string(), b.clone())).collect();
        zip_entries(&e).unwrap()
    }

    #[test]
    fn decompression_bomb_is_rejected() {
        let z = zip_of(&[("word/document.xml", vec![b' '; 2_000_000])]);
        assert!(z.len() < 100_000);
        assert!(matches!(Package::open_limited(&z, 1_000_000, 10_000_000), Err(DocxError::Limit(_))));
        assert!(matches!(Package::open_limited(&z, 10_000_000, 1_000_000), Err(DocxError::Limit(_))));
        assert!(Package::open_limited(&z, 10_000_000, 10_000_000).is_ok());
    }

    #[test]
    fn lying_declared_size_is_rejected_or_bounded() {
        let mut z = zip_of(&[("a.xml", b"<a/>".to_vec())]);
        // Patch the uncompressed size in the central directory and local header to ~4 GB.
        let mut patched = 0;
        for sig in [[0x50u8, 0x4B, 0x01, 0x02], [0x50, 0x4B, 0x03, 0x04]] {
            if let Some(pos) = z.windows(4).position(|w| w == sig) {
                let off = if sig[2] == 1 { pos + 24 } else { pos + 22 };
                z[off..off + 4].copy_from_slice(&0xFFFF_FFF0u32.to_le_bytes());
                patched += 1;
            }
        }
        assert_eq!(patched, 2);
        match Package::open(&z) {
            Err(_) => {}
            Ok(p) => assert!(p.get("a.xml").is_none_or(|b| b.len() < 1024)),
        }
    }

    #[test]
    fn case_variants_share_one_canonical_name() {
        let mut files = BTreeMap::new();
        files.insert("word/media/image1.png".to_string(), vec![1u8]);
        files.insert("word/_rels/document.xml.rels".to_string(), br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="a" Type="t" Target="MEDIA/Image1.PNG"/><Relationship Id="b" Type="t" Target="/Word/media/image1.png"/><Relationship Id="c" Type="t" Target="media/none.png"/></Relationships>"#.to_vec());
        let p = Package::from_parts(files);
        assert_eq!(p.canonical("/WORD/MEDIA/IMAGE1.png"), Some("word/media/image1.png"));
        assert_eq!(p.canonical("word/media/image1.png"), Some("word/media/image1.png"));
        assert_eq!(p.canonical("word/media/x.png"), None);
        assert_eq!(p.get("Word/Media/Image1.png"), Some(&[1u8][..]));
        let rels = p.rels("word/document.xml");
        let targets: Vec<&str> = rels.list().iter().map(|r| r.target.as_str()).collect();
        assert_eq!(targets, ["word/media/image1.png", "word/media/image1.png", "word/media/none.png"]);
    }

    #[test]
    fn rels_index_by_first_id_and_are_capped() {
        let mut x = String::from(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#);
        x.push_str(r#"<Relationship Id="dup" Type="t" Target="first.xml"/><Relationship Id="dup" Type="t" Target="second.xml"/>"#);
        const MAX: usize = 65_536;
        for i in 0..MAX + 10 {
            x.push_str(&format!(r#"<Relationship Id="r{i}" Type="t" Target="p{i}.xml"/>"#));
        }
        x.push_str("</Relationships>");
        let mut files = BTreeMap::new();
        files.insert("word/_rels/document.xml.rels".to_string(), x.into_bytes());
        let pkg = Package::from_parts(files);
        let rels = pkg.rels_limited("word/document.xml", MAX);
        assert_eq!(rels.list().len(), MAX);
        // Past the cap the file is refused, not read without the rest.
        assert!(pkg.refused().is_some_and(|w| w.contains("relationships")), "{:?}", pkg.refused());
        assert_eq!(rels.by_id("dup").map(|r| r.target.as_str()), Some("word/first.xml"));
        assert_eq!(rels.by_id("r100").map(|r| r.target.as_str()), Some("word/p100.xml"));
        assert!(rels.by_id(&format!("r{}", MAX)).is_none());
        // Every lookup is a hash probe: a pass over all ids is instant, not quadratic.
        assert!((0..MAX - 2).all(|i| rels.by_id(&format!("r{i}")).is_some()));
        // At the cap: nothing refused.
        assert!(Package::from_parts(BTreeMap::new()).refused().is_none());
    }

    /// A file's parts share one node budget; a package WordCraft wrote itself (to convert a
    /// document it holds, as Save As Word 2003 XML does) has only each part's cap.
    #[test]
    fn written_packages_have_no_file_node_budget() {
        let files = BTreeMap::from([("a.xml".to_string(), b"<a><b/></a>".to_vec())]);
        assert_eq!(Package::from_parts(files.clone()).nodes_left(), xml::MAX_FILE_NODES);
        let written = Package::written(files);
        let tree = written.xml("a.xml").unwrap();
        assert!(tree.is_some());
        assert_eq!(written.nodes_left(), usize::MAX - 2);
    }

    #[test]
    fn the_document_shares_its_pictures_with_the_package() {
        // Each picture was copied out of the package, so a picture document held its pictures
        // twice while it opened, and left the package's copies behind as freed memory.
        let ns = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture""#;
        let document = format!(
            r#"<w:document {ns}><w:body><w:p><w:r><w:drawing><wp:inline><wp:extent cx="635000" cy="635000"/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic><pic:blipFill><a:blip r:embed="rIdImg"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p></w:body></w:document>"#
        );
        let rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdImg" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/pic.png"/></Relationships>"#;
        let z = zip_of(&[
            ("word/document.xml", document.into_bytes()),
            ("word/_rels/document.xml.rels", rels.as_bytes().to_vec()),
            ("word/media/pic.png", b"\x89PNG picture bytes".to_vec()),
        ]);
        let pkg = Package::open(&z).unwrap();
        let doc = crate::read::read_opened(&pkg).unwrap();
        let held = doc.media.values().next().expect("the picture is read");
        assert!(Arc::ptr_eq(held, &pkg.get_shared("word/media/pic.png").unwrap()), "the same bytes, not a copy");
        assert_eq!(pkg.get("/WORD/media/PIC.png"), Some(&b"\x89PNG picture bytes"[..]), "lookups are unchanged");
    }

    #[test]
    fn resolves_targets() {
        assert_eq!(resolve("word", "media/image1.png"), "word/media/image1.png");
        assert_eq!(resolve("word", "../customXml/item1.xml"), "customXml/item1.xml");
        assert_eq!(resolve("word", "/word/styles.xml"), "word/styles.xml");
        assert_eq!(resolve("", "word/document.xml"), "word/document.xml");
        assert_eq!(resolve("word", "../../../x"), "x");
    }

    #[test]
    fn relative_targets_resolve_back() {
        for (from, to, want) in [
            ("word/document.xml", "word/charts/chart1.xml", "charts/chart1.xml"),
            ("word/charts/chart1.xml", "word/embeddings/book.xlsx", "../embeddings/book.xlsx"),
            ("word/charts/chart1.xml", "word/charts/colors1.xml", "colors1.xml"),
            ("word/document.xml", "customXml/item1.xml", "../customXml/item1.xml"),
            ("word/document.xml", "word/a:b.bin", "./a:b.bin"),
        ] {
            let r = relative(from, to);
            assert_eq!(r, want);
            assert_eq!(resolve(from.rsplit_once('/').map_or("", |(d, _)| d), &r), to);
        }
    }

    #[test]
    fn embedded_manifest_round_trips_and_ignores_junk() {
        let mut m = EmbeddedManifest::default();
        let rel = Rel { id: "rId1".into(), kind: "k".into(), target: "word/embeddings/x.xlsx".into(), external: false };
        let url = Rel { id: "rId2".into(), kind: "k".into(), target: "https://example.com/x".into(), external: true };
        m.parts.insert("word/charts/chart1.xml".into(), EmbeddedPart { content_type: "ct".into(), rels: vec![rel, url] });
        let back = EmbeddedManifest::parse(&m.to_bytes());
        let p = &back.parts["word/charts/chart1.xml"];
        assert_eq!((p.content_type.as_str(), p.rels.len(), p.rels[1].external), ("ct", 2, true));
        let junk = EmbeddedManifest::parse(b"R\tnope\trId1\tk\tt\tI\nP\tonly-two\n\t\t\n\xff\xfe");
        assert!(junk.parts.is_empty());
    }
}
