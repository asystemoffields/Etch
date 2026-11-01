//! Objects kept for writing back: charts, SmartArt diagrams and OLE objects.
//!
//! WordCraft draws charts and diagrams and shows an OLE object's picture, but doesn't edit them.
//! So that saving doesn't lose them, the reader keeps each one's own markup (an `a:graphic` or a
//! `w:object`) as a [`wordcraft_doc::graphic::Embedded`] on the object, and every part that markup
//! reaches through relationships (chart, embedded workbook, colours and style; diagram data, layout,
//! quick style, colours and drawing; OLE embedding and its picture…) as opaque bytes in
//! `Document::passthrough`, with their content types and relationships in an
//! [`EmbeddedManifest`](crate::package::EmbeddedManifest). The font table and the fonts it embeds
//! are kept the same way.

use std::collections::HashSet;
use std::sync::Arc;

use wordcraft_doc::graphic::{Embedded, EmbeddedRef};

use super::Reader;
use crate::package::{
    CUSTOM_XML_ROOTS, ContentTypes, MAX_EMBEDDED_PARTS, MAX_EMBEDDED_RELS, MAX_EMBEDDED_XML, Rel, Rels, SETTINGS_RELS, manifest_field_ok, rel_is, rt,
};
use crate::xml::{El, MAX_DEPTH, NAMESPACES, esc};

/// Relationship types (last path segment) the reader models or the writer produces itself, so
/// they are never carried verbatim: carrying them would duplicate or contradict the written part.
/// Types the main document refers to by id (headers, pictures, objects…) are listed too.
pub const HANDLED_RELS: &[&str] = &[
    "officeDocument",
    "core-properties",
    "extended-properties",
    "custom-properties",
    "styles",
    "numbering",
    "settings",
    "theme",
    "fontTable",
    "footnotes",
    "endnotes",
    "comments",
    "commentsExtended",
    "commentsIds",
    "commentsExtensible",
    "customXml",
    "vbaProject",
    "header",
    "footer",
    "image",
    "hyperlink",
    "oleObject",
    "package",
    "chart",
    "chartEx",
    "diagramData",
    "diagramDrawing",
    "diagramLayout",
    "diagramQuickStyle",
    "diagramColors",
    "video",
    "audio",
    "media",
    "control",
    "subDocument",
    "ink",
];

/// Most relationship ids one object's markup may use.
const MAX_IDS: usize = 64;

impl Reader<'_> {
    /// Keep custom XML stores reached from the main document or package, without evaluating
    /// bindings. A bibliography already loaded into the source model belongs to its writer.
    pub(super) fn keep_custom_xml(&mut self, package: &Rels, document: &Rels) {
        let mut roots = String::new();
        for (owner, rels) in [("P", package), ("D", document)] {
            if rels.list().iter().filter(|r| !r.external && rel_is(&r.kind, rt::CUSTOM_XML)).count() > MAX_EMBEDDED_RELS {
                self.refuse(|| format!("more than {MAX_EMBEDDED_RELS} custom XML parts"));
            }
            for rel in rels.list().iter().filter(|r| !r.external && rel_is(&r.kind, rt::CUSTOM_XML)).take(MAX_EMBEDDED_RELS) {
                if ![&rel.kind, &rel.target].iter().all(|s| manifest_field_ok(s)) {
                    continue;
                }
                if !self.doc.sources.is_empty() && self.xml(&rel.target).ok().flatten().is_some_and(|r| r.local() == "Sources") {
                    continue;
                }
                // Native ink shares the custom XML relationship type, but its part belongs to the
                // drawing that refers to it: kept with it, or dropped with it (no orphan part).
                let pkg = self.pkg;
                if self.content_types.get_or_insert_with(|| ContentTypes::read(pkg)).of(&rel.target) == Some("application/inkml+xml") {
                    continue;
                }
                if self.keep_part_tree(&rel.target) {
                    roots.push_str(&format!("{owner}\t{}\t{}\n", rel.kind, rel.target));
                }
            }
        }
        if !roots.is_empty() {
            self.doc.passthrough.insert(CUSTOM_XML_ROOTS.into(), Arc::new(roots.into_bytes()));
        }
    }

    /// Keep the parts the package and the main document relate to that WordCraft neither models
    /// nor writes itself (web settings, styles-with-effects, add-in task panes, the thumbnail, a
    /// glossary…), with every part they reach, so saving carries them unchanged. Skipped: the
    /// relationship types the writer produces or the reader models ([`HANDLED_RELS`]) and any
    /// relationship the main document's markup refers to by id (its target goes with the
    /// element, or not at all).
    ///
    /// `stories` names the story parts the writer writes afresh (`(owner, part)`: a header or
    /// footer `H<part id>`, the footnotes `F`, endnotes `E` and comments `C`); their unmodelled
    /// relationships are kept the same way, for their owner.
    pub(super) fn keep_unmodelled(&mut self, package: &Rels, document: &Rels, stories: &[(String, String)]) {
        let mut owners: Vec<(String, String, Rels)> =
            vec![("P".into(), String::new(), package.clone()), ("D".into(), self.main.clone(), document.clone())];
        // Every story's: a document has at most as many as a file may make (`MAX_PARTS`).
        for (owner, path) in stories {
            let rels = self.pkg.rels(path);
            if !rels.list().is_empty() {
                owners.push((owner.clone(), path.clone(), rels));
            }
        }
        // The part's bytes are looked at where they are (not copied: a main part can be hundreds
        // of megabytes), once for all the relationships it might refer to.
        let pkg = self.pkg;
        let mut roots = String::new();
        // One glossary per main document (its first relationship; see `read_glossary`).
        let mut glossary = false;
        for (owner, path, rels) in &owners {
            let owner = owner.as_str();
            // The relationships the reader doesn't handle itself (pictures, headers, footers…
            // are written afresh): past the limit the file is refused, not read without them.
            let mut candidates: Vec<&Rel> =
                rels.list().iter().filter(|r| !r.external).filter(|r| !HANDLED_RELS.contains(&r.kind.rsplit('/').next().unwrap_or(""))).collect();
            if candidates.len() > MAX_EMBEDDED_RELS {
                self.refused.get_or_insert_with(|| format!("more than {MAX_EMBEDDED_RELS} parts it doesn't model related to one part ({path})"));
                candidates.truncate(MAX_EMBEDDED_RELS);
            }
            let referenced = if owner == "P" || path.is_empty() || candidates.is_empty() {
                HashSet::new()
            } else {
                quoted_ids(pkg.get(path).unwrap_or_default(), candidates.iter().map(|r| r.id.as_str()))
            };
            for rel in candidates {
                if referenced.contains(rel.id.as_str())
                    || (owner == "D" && rel.kind.rsplit('/').next() == Some("glossaryDocument") && std::mem::replace(&mut glossary, true))
                    || ![&rel.kind, &rel.target].iter().all(|s| manifest_field_ok(s))
                {
                    continue;
                }
                if self.keep_part_tree(&rel.target) {
                    roots.push_str(&format!("{owner}\t{}\t{}\n", rel.kind, rel.target));
                }
            }
        }
        if !roots.is_empty() {
            self.doc.passthrough.insert(crate::package::UNMODELLED_ROOTS.into(), Arc::new(roots.into_bytes()));
        }
    }

    /// Keep the relationships of the settings part at `path` (but the attached template's, which
    /// the model holds) for the settings a save keeps from it ([`SETTINGS_RELS`]): a mail merge's
    /// data source and header source (external), its recipient data (a part, kept with every
    /// part it reaches).
    pub(super) fn keep_settings_rels(&mut self, path: &str) {
        let mut lines = String::new();
        let mut ids: HashSet<String> = HashSet::new();
        let all = self.pkg.rels(path).into_list();
        if all.len() > MAX_EMBEDDED_RELS {
            self.refuse(|| format!("more than {MAX_EMBEDDED_RELS} relationships of its settings"));
        }
        for rel in all.into_iter().take(MAX_EMBEDDED_RELS) {
            let tail = rel.kind.rsplit('/').next().unwrap_or("");
            if rel_is(&rel.kind, rt::ATTACHED_TEMPLATE)
                || (!rel.external && HANDLED_RELS.contains(&tail))
                || ![&rel.id, &rel.kind, &rel.target].iter().all(|f| manifest_field_ok(f))
                || ids.contains(&rel.id)
            {
                continue;
            }
            if !rel.external && !self.keep_part_tree(&rel.target) {
                continue;
            }
            lines.push_str(&format!("{}\t{}\t{}\t{}\n", rel.id, rel.kind, rel.target, if rel.external { "E" } else { "I" }));
            ids.insert(rel.id);
        }
        if !lines.is_empty() {
            self.doc.passthrough.insert(SETTINGS_RELS.into(), Arc::new(lines.into_bytes()));
        }
    }

    /// Keep `el` (an object's `a:graphic` or `w:object`, read with `rels`) and the parts it refers
    /// to, so the object can be written back. `extra` names relationships the object's parts use
    /// by id although its markup doesn't (a SmartArt drawing, named in the diagram's data part).
    /// `None` when it can't be written back faithfully: a relationship it uses is missing, a part
    /// is missing, or it's too big.
    pub(super) fn keep_embedded(&mut self, el: &El, rels: &Rels, extra: &[String]) -> Option<Arc<Embedded>> {
        let mut ids = Vec::new();
        if rel_ids(el, &mut ids, 0).is_none() {
            self.refuse(|| format!("an embedded object that uses more than {MAX_IDS} relationships"));
            return None;
        }
        let mut refs = Vec::new();
        for id in &ids {
            let rel = rels.by_id(id)?;
            if !rel.external && !self.keep_part_tree(&rel.target) {
                return None;
            }
            refs.push(EmbeddedRef { id: id.clone(), kind: rel.kind.clone(), target: rel.target.clone(), external: rel.external });
        }
        for id in extra.iter().filter(|id| !ids.contains(id)) {
            if let Some(rel) = rels.by_id(id).filter(|r| !r.external)
                && self.keep_part_tree(&rel.target)
            {
                refs.push(EmbeddedRef { id: id.clone(), kind: rel.kind.clone(), target: rel.target.clone(), external: false });
            }
        }
        let xml = standalone_xml(el)?;
        if xml.len() > MAX_EMBEDDED_XML {
            self.refuse(|| format!("an embedded object's markup longer than {} MB", MAX_EMBEDDED_XML >> 20));
            return None;
        }
        Some(Arc::new(Embedded { xml, refs }))
    }

    /// Keep part `root` and every part it reaches through its relationships. False when `root`
    /// itself can't be kept.
    pub(super) fn keep_part_tree(&mut self, root: &str) -> bool {
        let root = self.canon(root);
        if self.embedded.parts.contains_key(&root) {
            return true;
        }
        if !self.keep_part(&root) {
            return false;
        }
        let mut todo = vec![root];
        while let Some(part) = todo.pop() {
            let mut kept: Vec<Rel> = Vec::new();
            let mut kept_ids: HashSet<String> = HashSet::new();
            for r in self.pkg.rels(&part).into_list() {
                if kept.len() >= MAX_EMBEDDED_RELS {
                    self.refuse(|| format!("a part of an embedded object ({part}) with more than {MAX_EMBEDDED_RELS} relationships"));
                    break;
                }
                if ![&r.id, &r.kind, &r.target].iter().all(|f| manifest_field_ok(f)) || kept_ids.contains(&r.id) {
                    continue;
                }
                if !r.external && !self.embedded.parts.contains_key(&r.target) {
                    if !self.keep_part(&r.target) {
                        continue;
                    }
                    todo.push(r.target.clone());
                }
                kept_ids.insert(r.id.clone());
                kept.push(r);
            }
            if let Some(p) = self.embedded.parts.get_mut(&part) {
                p.rels = kept;
            }
        }
        true
    }

    /// Keep one part's bytes and content type (not its relationships).
    fn keep_part(&mut self, part: &str) -> bool {
        let lower = part.to_ascii_lowercase();
        if !manifest_field_ok(part) || lower.ends_with(".rels") || lower == "[content_types].xml" || part.eq_ignore_ascii_case(&self.main) {
            log::warn!("docx: not carrying part {part:?} for an embedded object");
            return false;
        }
        if self.embedded.parts.len() >= MAX_EMBEDDED_PARTS {
            self.refuse(|| format!("more than {MAX_EMBEDDED_PARTS} parts kept for its objects, macros and unmodelled content"));
            return false;
        }
        let Some(bytes) = self.pkg.get_shared(part) else {
            log::warn!("docx: part {part} of an embedded object is missing");
            return false;
        };
        if !self.doc.passthrough.contains_key(part) && !self.take_copy(bytes.len(), part) {
            return false;
        }
        let pkg = self.pkg;
        let ct = self.content_types.get_or_insert_with(|| ContentTypes::read(pkg)).of(part).unwrap_or("application/octet-stream");
        let content_type = if manifest_field_ok(ct) { ct.to_string() } else { "application/octet-stream".to_string() };
        self.doc.passthrough.entry(part.to_string()).or_insert(bytes);
        self.embedded.parts.insert(part.to_string(), crate::package::EmbeddedPart { content_type, rels: Vec::new() });
        true
    }
}

/// Which of `ids` appear in `part` quoted (`"rId5"`, as a relationship id is written in an
/// attribute): one pass over the part however many ids there are. An id that holds a quote
/// itself (not an XML ID) is never found.
fn quoted_ids<'a>(part: &[u8], ids: impl Iterator<Item = &'a str>) -> HashSet<&'a str> {
    let wanted: std::collections::HashMap<&'a [u8], &'a str> = ids.filter(|id| !id.contains('"')).map(|id| (id.as_bytes(), id)).collect();
    let longest = wanted.keys().map(|k| k.len()).max().unwrap_or(0);
    let mut found = HashSet::new();
    let mut prev: Option<usize> = None;
    for (i, _) in part.iter().enumerate().filter(|(_, b)| **b == b'"') {
        // Between two quotes in a row: what a quoted id would be.
        if let Some(p) = prev
            && i - p - 1 <= longest
            && let Some(id) = part.get(p + 1..i).and_then(|seg| wanted.get(seg))
        {
            found.insert(*id);
        }
        prev = Some(i);
    }
    found
}

/// The relationship ids `e` uses (attributes in the relationships namespace, and VML's
/// `o:relid`), in document order. `None` past [`MAX_IDS`].
fn rel_ids(e: &El, out: &mut Vec<String>, depth: usize) -> Option<()> {
    if depth > MAX_DEPTH {
        return Some(());
    }
    for (k, v) in &e.attrs {
        if (k.starts_with("r:") || k == "o:relid") && !out.contains(v) {
            if out.len() >= MAX_IDS {
                return None;
            }
            out.push(v.clone());
        }
    }
    for c in e.els() {
        rel_ids(c, out, depth + 1)?;
    }
    Some(())
}

/// `e` as a standalone element: its XML with the namespace declarations its canonical prefixes
/// need. `None` when too long, or in a namespace we can't name.
fn standalone_xml(e: &El) -> Option<String> {
    let xml = e.to_xml();
    let rest = xml.get(1 + e.name.len()..).filter(|_| !xml.is_empty())?;
    let mut s = format!("<{}", e.name);
    for (p, u) in NAMESPACES {
        s.push_str(&format!(" xmlns:{p}=\"{}\"", esc(u)));
    }
    s.push_str(rest);
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One pass finds the ids the naive search finds (a quoted id anywhere in the part), for
    /// ids next to each other, at the ends, empty or holding a quote.
    #[test]
    fn quoted_ids_are_those_a_search_finds() {
        let naive = |part: &[u8], id: &str| {
            let needle = format!("\"{id}\"");
            part.windows(needle.len()).any(|w| w == needle.as_bytes())
        };
        let parts: [&[u8]; 6] = [
            br#"<w:r r:id="rId1"/><v:shape o:relid="rId22" x="rId3x"/>"#,
            br#""rId4""rId5" "" rId6 "rId7"#,
            b"",
            br#"""""#,
            br#"<a b="r"c" d="rId1"/>"#,
            "<é a=\"rId9\" b=\"\u{FFFD}\"/>".as_bytes(),
        ];
        let ids = ["rId1", "rId22", "rId2", "rId3", "rId3x", "rId4", "rId5", "rId6", "rId7", "rId9", "", "r\"c", "\u{FFFD}"];
        for part in parts {
            let found = quoted_ids(part, ids.iter().copied());
            for id in ids {
                let want = naive(part, id) && !id.contains('"');
                assert_eq!(found.contains(id), want, "{id:?} in {}", String::from_utf8_lossy(part));
            }
        }
    }
}
