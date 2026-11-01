//! Custom document properties (`docProps/custom.xml`, ECMA-376 Part 1 §22.3). Citation
//! managers keep their document preferences here (Zotero: `ZOTERO_PREF_1`…), so they must
//! survive a round trip intact.

use wordcraft_doc::CustomProp;

use crate::xml::{El, MAX_DEPTH, Node, W, esc, is_qname};

/// The format identifier every custom property carries (§22.3.2.2).
const FMTID: &str = "{D5CDD505-2E9C-101B-9397-08002B2CF9AE}";
/// Most properties we keep from one file.
const MAX_PROPS: usize = 10_000;

/// Simple variant types kept as text.
const SIMPLE: &[&str] = &[
    "lpwstr", "lpstr", "bstr", "i1", "i2", "i4", "i8", "int", "ui1", "ui2", "ui4", "ui8", "uint", "r4", "r8", "decimal", "bool", "date", "filetime",
    "cy", "error", "clsid",
];

/// Properties of a parsed `docProps/custom.xml` root.
pub fn read(root: &El) -> Vec<CustomProp> {
    let mut out = Vec::new();
    if root.els().filter(|e| e.local() == "property").nth(MAX_PROPS).is_some() {
        crate::read::refuse_here(|| format!("more than {MAX_PROPS} custom document properties"));
    }
    for p in root.els().filter(|e| e.local() == "property").take(MAX_PROPS) {
        let Some(name) = p.attr("name").filter(|n| !n.is_empty()) else { continue };
        let Some(v) = p.els().next() else { continue };
        let kind = v.local();
        let prop = if SIMPLE.contains(&kind) {
            CustomProp { name: name.to_string(), kind: kind.to_string(), value: v.text() }
        } else {
            let mut xml = String::new();
            el_xml(v, &mut xml, 0);
            CustomProp { name: name.to_string(), kind: "raw".into(), value: xml }
        };
        out.push(prop);
    }
    out
}

/// `docProps/custom.xml` for `props`.
pub fn write(props: &[CustomProp]) -> Vec<u8> {
    let mut w = W::new();
    w.open(
        "Properties",
        &[
            ("xmlns", "http://schemas.openxmlformats.org/officeDocument/2006/custom-properties"),
            ("xmlns:vt", "http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes"),
        ],
    );
    // Property ids start at 2 (§22.3.2.2).
    for (pid, p) in (2u32..).zip(props.iter().filter(|p| !p.name.is_empty())) {
        if p.kind == "raw" && !raw_is_wellformed(&p.value) {
            continue;
        }
        w.open("property", &[("fmtid", FMTID), ("pid", &pid.to_string()), ("name", &p.name)]);
        if p.kind == "raw" {
            w.raw(&p.value);
        } else {
            let kind = if SIMPLE.contains(&p.kind.as_str()) { p.kind.as_str() } else { "lpwstr" };
            w.leaf(&format!("vt:{kind}"), &[], &p.value);
        }
        w.close("property");
    }
    w.close("Properties");
    w.into_bytes()
}

/// A raw value is written only if it is one strictly well-formed element where `vt` is declared.
fn raw_is_wellformed(xml: &str) -> bool {
    crate::xml::fragment_roots(xml, &[("vt", "http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes")]) == Some(1)
}

/// Serialise an element with its canonical prefixes.
/// Elements and attributes whose names aren't XML names (or are in an unknown namespace) are left out.
fn el_xml(e: &El, out: &mut String, depth: usize) {
    if depth > MAX_DEPTH || !is_qname(&e.name) {
        return;
    }
    out.push('<');
    out.push_str(&e.name);
    for (k, v) in &e.attrs {
        if k.starts_with("xmlns") || !is_qname(k) {
            continue;
        }
        out.push(' ');
        out.push_str(k);
        out.push_str("=\"");
        out.push_str(&esc(v));
        out.push('"');
    }
    if e.kids.is_empty() {
        out.push_str("/>");
        return;
    }
    out.push('>');
    for k in &e.kids {
        match k {
            Node::El(c) => el_xml(c, out, depth + 1),
            Node::Text(t) => out.push_str(&esc(t)),
        }
    }
    out.push_str("</");
    out.push_str(&e.name);
    out.push('>');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_raw_values_are_left_out() {
        let prop = |value: &str| CustomProp { name: "p".into(), kind: "raw".into(), value: value.into() };
        let props = [prop("<vt:vector>"), prop("<vt:a></vt:b>"), prop("<vt:a/><vt:b/>"), prop("<x:a/>"), prop("<vt:lpwstr>ok</vt:lpwstr>")];
        let out = String::from_utf8(write(&props)).unwrap();
        assert_eq!(out.matches("<property ").count(), 1, "{out}");
        assert!(out.contains("<vt:lpwstr>ok</vt:lpwstr>"));
        assert!(crate::xml::fragment_ok(&out[out.find("<Properties").unwrap()..], &[]));
    }

    #[test]
    fn raw_values_with_bad_names_keep_only_xml_names() {
        let mut v = El { name: "vt:vector".into(), attrs: vec![("size".into(), "1".into()), ("ba&d".into(), "x".into())], kids: Vec::new() };
        v.kids.push(Node::El(El { name: "vt:lp&wstr".into(), ..El::default() }));
        v.kids.push(Node::El(El { name: "?:unknown".into(), ..El::default() }));
        v.kids.push(Node::El(El { name: "vt:lpwstr".into(), kids: vec![Node::Text("a".into())], ..El::default() }));
        let mut xml = String::new();
        el_xml(&v, &mut xml, 0);
        assert_eq!(xml, r#"<vt:vector size="1"><vt:lpwstr>a</vt:lpwstr></vt:vector>"#);
    }
}
