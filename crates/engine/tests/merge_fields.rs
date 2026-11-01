//! Etch has no mail merge. Documents people receive may still be mail merge
//! main documents, and their merge data must survive: `MERGEFIELD` fields (nested in `IF` and
//! `SKIPIF` codes too), the other merge fields, and Word's merge settings (`w:mailMerge`) open,
//! show the results they were saved with, survive editing and Update Fields (F9, and before
//! printing), and save back unchanged. Packages are hand-written from ECMA-376 Part 1 (§17.16
//! fields, §17.14 mail merge); no file produced by Word.
#![cfg(not(target_arch = "wasm32"))]

use std::collections::BTreeMap;
use std::io::{Read, Write};

use serde_json::json;
use wordcraft_doc::para::InlineObject;
use wordcraft_doc::{Document, PartKind, StoryRef};
use wordcraft_engine::{Session, io};

const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

fn zip(entries: &[(&str, &str)]) -> Vec<u8> {
    let mut zw = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, body) in entries {
        zw.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
        zw.write_all(body.as_bytes()).unwrap();
    }
    zw.finish().unwrap().into_inner()
}

fn unzip(bytes: &[u8]) -> BTreeMap<String, String> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut out = BTreeMap::new();
    for i in 0..z.len() {
        let mut f = z.by_index(i).unwrap();
        let mut b = Vec::new();
        f.read_to_end(&mut b).unwrap();
        out.insert(f.name().to_string(), String::from_utf8_lossy(&b).into_owned());
    }
    out
}

/// A complex field: `begin`, its code (`code` may hold nested fields' markup), `separate`, the
/// result runs, `end`.
fn field(code: &str, result: &str) -> String {
    format!(
        r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r>{code}<w:r><w:fldChar w:fldCharType="separate"/></w:r>{result}<w:r><w:fldChar w:fldCharType="end"/></w:r>"#
    )
}

fn instr(text: &str) -> String {
    let esc = text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;");
    format!(r#"<w:r><w:instrText xml:space="preserve">{esc}</w:instrText></w:r>"#)
}

fn run(text: &str) -> String {
    format!(r#"<w:r><w:t xml:space="preserve">{text}</w:t></w:r>"#)
}

/// The merge settings Word writes in a main document (data source, query, field mapping, the
/// recipients left out), by relationships of the settings part.
const MAIL_MERGE: &str = r#"<w:mailMerge><w:mainDocumentType w:val="formLetters"/><w:linkToQuery/><w:dataType w:val="native"/><w:connectString w:val="Provider=Example.OLEDB;Data Source=C:\Lists\guests.xlsx"/><w:query w:val="SELECT * FROM `Guests$`"/><w:dataSource r:id="rId1"/><w:odso><w:table w:val="Guests$"/><w:src r:id="rId1"/><w:fHdr/><w:fieldMapData><w:type w:val="dbColumn"/><w:name w:val="City"/><w:mappedName w:val="City"/><w:column w:val="2"/><w:lid w:val="en-US"/></w:fieldMapData><w:recipientData r:id="rId2"/></w:odso></w:mailMerge>"#;

const RECIPIENTS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:recipients xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:recipientData><w:active w:val="1"/><w:column w:val="0"/><w:uniqueTag>QWRh</w:uniqueTag></w:recipientData></w:recipients>"#;

/// A letter as a mail merge main document: a merge field with a placeholder result, one with an
/// already merged result in two runs (kept as its text: like any field's, the result takes the
/// field's own formatting), an IF whose code nests a MERGEFIELD, a SKIPIF, NEXT, MERGEREC,
/// ADDRESSBLOCK and GREETINGLINE, a simple-field MERGEFIELD, and one in the header.
fn letter() -> Vec<u8> {
    let p = |inner: String| format!("<w:p>{inner}</w:p>");
    let body = [
        p(format!("{}{}{}", run("Dear "), field(&instr(r#" MERGEFIELD "First Name" \* MERGEFORMAT "#), &run("«First Name»")), run(","))),
        p(field(
            &format!(
                "{}{}{}",
                instr(" IF "),
                field(&instr(" MERGEFIELD City "), &run("Oslo")),
                instr(r#" = "Oslo" "See you at home" "See you in Oslo" "#)
            ),
            &run("See you at home"),
        )),
        p(field(&instr(" MERGEFIELD Company "), r#"<w:r><w:rPr><w:b/></w:rPr><w:t>Acme</w:t></w:r><w:r><w:t xml:space="preserve"> Ltd</w:t></w:r>"#)),
        p(format!(
            "{}{}{}{}",
            field(&format!("{}{}{}", instr(" SKIPIF "), field(&instr(" MERGEFIELD Status "), &run("Active")), instr(r#" = "Moved" "#)), ""),
            field(&instr(" MERGEREC "), &run("1")),
            field(&instr(" NEXT "), ""),
            field(&instr(r#" ADDRESSBLOCK \f "<<_FIRST0_>> <<_LAST0_>>" "#), &run("«AddressBlock»")),
        )),
        p(field(&instr(r#" GREETINGLINE \f "<<_GREETING_>> <<_FIRST0_>>," \l 1033 \e "Dear Sir or Madam," "#), &run("Dear Ada,"))),
        p(format!(r#"<w:fldSimple w:instr=" MERGEFIELD Title "><w:r><w:t>Dr</w:t></w:r></w:fldSimple>{}"#, run(" and team"))),
    ]
    .concat();
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="{W}" xmlns:r="{R}"><w:body>{body}<w:sectPr><w:headerReference w:type="default" r:id="rIdH"/></w:sectPr></w:body></w:document>"#
    );
    let header = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:hdr xmlns:w="{W}" xmlns:r="{R}"><w:p>{}{}</w:p></w:hdr>"#,
        run("For "),
        field(&instr(" MERGEFIELD Name "), &run("«Name»"))
    );
    let settings = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:settings xmlns:w="{W}" xmlns:r="{R}">{MAIL_MERGE}<w:defaultTabStop w:val="720"/></w:settings>"#
    );
    let settings_rels = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{R}/mailMergeSource" Target="file:///C:\Lists\guests.xlsx" TargetMode="External"/><Relationship Id="rId2" Type="{R}/recipientData" Target="recipientData.xml"/></Relationships>"#
    );
    let doc_rels = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdS" Type="{R}/settings" Target="settings.xml"/><Relationship Id="rIdH" Type="{R}/header" Target="header1.xml"/></Relationships>"#
    );
    let root_rels = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{R}/officeDocument" Target="word/document.xml"/></Relationships>"#
    );
    let ct = r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/header1.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml"/><Override PartName="/word/settings.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml"/><Override PartName="/word/recipientData.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.mailMergeRecipientData+xml"/></Types>"#;
    zip(&[
        ("[Content_Types].xml", ct),
        ("_rels/.rels", &root_rels),
        ("word/document.xml", &document),
        ("word/_rels/document.xml.rels", &doc_rels),
        ("word/header1.xml", &header),
        ("word/settings.xml", &settings),
        ("word/_rels/settings.xml.rels", &settings_rels),
        ("word/recipientData.xml", RECIPIENTS),
    ])
}

/// The header story of `d`.
fn header(d: &Document) -> StoryRef {
    let id = d.parts.iter().find(|(_, p)| p.kind == PartKind::Header).map(|(id, _)| *id).expect("a header");
    StoryRef::Part(id)
}

/// Each paragraph of `story` with a field: its text and its field objects (code, result, nested
/// fields, lock), in order.
fn field_paras(d: &Document, story: StoryRef) -> Vec<(String, Vec<InlineObject>)> {
    d.para_paths(story)
        .into_iter()
        .filter_map(|path| d.para(story, &path))
        .map(|p| {
            (
                p.plain_text(),
                p.objects
                    .iter()
                    .filter(|o| matches!(o, InlineObject::Field { .. } | InlineObject::FieldStart { .. } | InlineObject::FieldEnd))
                    .cloned()
                    .collect::<Vec<_>>(),
            )
        })
        .filter(|(_, f)| !f.is_empty())
        .collect()
}

/// The codes of every field (nested ones too) in `d`'s body and header.
fn codes(d: &Document) -> Vec<String> {
    fn walk(n: &[wordcraft_doc::para::NestedField], out: &mut Vec<String>) {
        for f in n {
            out.push(f.instr.trim().to_string());
            walk(&f.nested, out);
        }
    }
    let mut out = Vec::new();
    for story in [StoryRef::Body, header(d)] {
        for (_, objects) in field_paras(d, story) {
            for o in objects {
                if let InlineObject::Field { instr, nested, .. } | InlineObject::FieldStart { instr, nested, .. } = o {
                    out.push(instr.trim().to_string());
                    walk(&nested, &mut out);
                }
            }
        }
    }
    out
}

/// A merge main document opens with every merge field showing the result it was saved with,
/// keeps them through typing, Update Fields (the body's, and every story's as before printing),
/// toggling field codes and a save, and reopens with the same fields, nested ones included.
#[test]
fn merge_fields_keep_their_results_through_open_edit_update_save_and_reopen() {
    let opened = io::open_bytes("letter.docx", &letter()).expect("opens");
    let body = field_paras(&opened, StoryRef::Body);
    let texts: Vec<&str> = body.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(
        texts,
        ["Dear «First Name»,", "See you at home", "Acme Ltd", "1«AddressBlock»", "Dear Ada,", "Dr and team"],
        "each field shows its stored result"
    );
    assert_eq!(field_paras(&opened, header(&opened)).first().map(|(t, _)| t.as_str()), Some("For «Name»"));
    let all = codes(&opened);
    for code in [
        r#"MERGEFIELD "First Name" \* MERGEFORMAT"#,
        "MERGEFIELD City",
        "MERGEFIELD Company",
        "MERGEFIELD Status",
        "MERGEREC",
        "NEXT",
        "MERGEFIELD Title",
        "MERGEFIELD Name",
    ] {
        assert!(all.iter().any(|c| c == code), "{code} in {all:?}");
    }
    assert!(all.iter().any(|c| c.starts_with("IF Oslo = ")), "the IF's code holds its nested field's result: {all:?}");
    let header_before = field_paras(&opened, header(&opened));

    let mut s = Session::new(opened);
    // Editing: typing before the first field, and a paragraph after the last.
    s.run("caret.docStart", &json!({})).unwrap();
    s.run("text.insert", &json!({"text": "Hello. "})).unwrap();
    s.run("caret.docEnd", &json!({})).unwrap();
    s.run("text.newParagraph", &json!({})).unwrap();
    s.run("text.insert", &json!({"text": "Kind regards"})).unwrap();
    let mut want = body.clone();
    if let Some(first) = want.first_mut() {
        first.0 = format!("Hello. {}", first.0);
    }
    let unchanged = |d: &Document, when: &str| {
        assert_eq!(field_paras(d, StoryRef::Body), want, "{when}: the body's fields are as they were");
        assert_eq!(field_paras(d, header(d)), header_before, "{when}: the header's");
    };
    unchanged(&s.doc, "edited");
    // F9 in the body, then in every story (Update fields before printing): there is nothing to
    // compute for a merge field, so each keeps its result; none is blanked or shown as «Name».
    s.run("caret.docStart", &json!({})).unwrap();
    s.run("references.updateFields", &json!({})).unwrap();
    unchanged(&s.doc, "F9");
    s.run("references.updateFields", &json!({"all": true})).unwrap();
    unchanged(&s.doc, "update before printing");
    // Field codes shown and hidden again (Alt+F9 twice), then F9 again.
    s.run("select.all", &json!({})).unwrap();
    s.run("fields.toggleCode", &json!({})).unwrap();
    s.run("fields.toggleCode", &json!({})).unwrap();
    s.run("references.updateFields", &json!({})).unwrap();
    unchanged(&s.doc, "codes toggled");

    // Saved and opened again: the same fields, results and nested codes.
    let dir = std::env::temp_dir().join(format!("wordcraft-merge-fields-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("letter.docx");
    s.run("file.save", &json!({"path": path.to_string_lossy()})).unwrap();
    let saved = std::fs::read(&path).unwrap();
    let back = io::open_bytes("letter.docx", &saved).unwrap();
    unchanged(&back, "reopened");
    assert_eq!(codes(&back), all);
    // The package holds the codes and stored results Word reads.
    let files = unzip(&saved);
    let xml = files.get("word/document.xml").unwrap();
    for needle in
        ["MERGEFIELD City", "MERGEFIELD Company", "MERGEFIELD Status", "MERGEREC", "NEXT", "ADDRESSBLOCK", "GREETINGLINE", "MERGEFIELD Title"]
    {
        assert!(xml.contains(needle), "{needle} written");
    }
    for result in ["«First Name»", ">Oslo<", "See you at home", ">Acme Ltd<", "«AddressBlock»", "Dear Ada,", ">Dr<"] {
        assert!(xml.contains(result), "{result} written: {xml}");
    }
    assert!(files.values().any(|x| x.contains("MERGEFIELD Name") && x.contains("«Name»")), "the header's merge field");
    // Saved again unchanged: the same document part.
    let again = io::save_bytes("letter.docx", &back).unwrap();
    assert_eq!(unzip(&again).get("word/document.xml"), files.get("word/document.xml"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Merge fields typed in WordCraft (Insert › Field) are ordinary fields: an update keeps
/// whatever result they have, a placeholder, a merged value or nothing at all.
#[test]
fn update_fields_keeps_a_merge_fields_result_whatever_it_is() {
    let mut s = Session::new(Document::new());
    for (code, result) in [("MERGEFIELD Name", "«Name»"), ("MERGEFIELD City", "Oslo"), ("MERGEFIELD Empty", ""), ("MERGEREC", "7")] {
        s.run("insert.field", &json!({"instr": code, "result": result})).unwrap();
        s.run("text.insert", &json!({"text": " "})).unwrap();
    }
    let results = |s: &Session| -> Vec<String> {
        field_paras(&s.doc, StoryRef::Body)
            .into_iter()
            .flat_map(|(_, f)| f)
            .filter_map(|o| if let InlineObject::Field { result, .. } = o { Some(result) } else { None })
            .collect()
    };
    assert_eq!(results(&s), ["«Name»", "Oslo", "", "7"]);
    s.run("references.updateFields", &json!({"all": true})).unwrap();
    assert_eq!(results(&s), ["«Name»", "Oslo", "", "7"]);
    assert_eq!(s.doc.plain_text(StoryRef::Body), "«Name» Oslo  7 ");
}

/// Word's merge settings ride along through the engine's open, edit and save, byte for byte,
/// with the data source and recipient data they name.
#[test]
fn mail_merge_settings_survive_an_edit_and_a_save() {
    let mut s = Session::new(io::open_bytes("letter.docx", &letter()).unwrap());
    s.run("caret.docEnd", &json!({})).unwrap();
    s.run("text.insert", &json!({"text": " Thanks."})).unwrap();
    let files = unzip(&io::save_bytes("letter.docx", &s.doc).unwrap());
    let settings = files.get("word/settings.xml").unwrap();
    assert!(settings.contains(MAIL_MERGE), "{settings}");
    let rels = files.get("word/_rels/settings.xml.rels").unwrap();
    assert!(
        rels.contains(r#"Id="rId1""#)
            && rels.contains("mailMergeSource")
            && rels.contains(r"file:///C:\Lists\guests.xlsx")
            && rels.contains(r#"TargetMode="External""#),
        "{rels}"
    );
    assert!(rels.contains(r#"Id="rId2""#) && rels.contains("recipientData"), "{rels}");
    assert_eq!(files.get("word/recipientData.xml").map(String::as_str), Some(RECIPIENTS));
}

/// The other formats write a merge field as they write any field, unchanged by the removal: its
/// stored result as text (ODT, HTML, Markdown, plain text), or the code and result (RTF).
#[test]
fn other_formats_write_merge_fields_as_before() {
    let d = io::open_bytes("letter.docx", &letter()).unwrap();
    let text = |ext: &str| String::from_utf8_lossy(&io::save_bytes(&format!("letter.{ext}"), &d).unwrap()).into_owned();
    let odt = unzip(&io::save_bytes("letter.odt", &d).unwrap()).get("content.xml").cloned().unwrap();
    for out in [odt, text("html"), text("md"), text("txt")] {
        for result in ["«First Name»", "See you at home", "Dear Ada,", "«AddressBlock»"] {
            assert!(out.contains(result), "{result} in {out}");
        }
    }
    let rtf = text("rtf");
    assert!(rtf.contains(r"{\field{\*\fldinst {MERGEFIELD Company}}{\fldrslt {Acme Ltd}}}"), "{rtf}");
    assert!(rtf.contains(r"{\field{\*\fldinst {MERGEFIELD Name}}{\fldrslt {\u171?Name\u187?}}}"), "the header's: {rtf}");
    // RTF has no nested fields here: the IF's code carries its nested field's result, as before.
    assert!(rtf.contains(r#"{\field{\*\fldinst {IF Oslo = "Oslo" "See you at home" "See you in Oslo"}}{\fldrslt {See you at home}}}"#), "{rtf}");
    // Read back, the RTF's merge field is a field again, with its result.
    let back = io::open_bytes("letter.rtf", rtf.as_bytes()).unwrap();
    assert!(back.plain_text(StoryRef::Body).contains("See you at home"));
}

/// The merge settings, their relationships and recipient data in the files `files` holds.
fn assert_mail_merge_kept(files: &BTreeMap<String, String>, what: &str) {
    let settings = files.get("word/settings.xml").map(String::as_str).unwrap_or_default();
    assert!(settings.contains(MAIL_MERGE), "{what}: {settings}");
    let rels = files.get("word/_rels/settings.xml.rels").map(String::as_str).unwrap_or_default();
    assert!(rels.contains(r#"Id="rId1""#) && rels.contains("mailMergeSource") && rels.contains(r#"TargetMode="External""#), "{what}: {rels}");
    assert!(rels.contains(r#"Id="rId2""#) && rels.contains("recipientData"), "{what}: {rels}");
    // A Word XML Document holds each part as XML, without its own XML declaration.
    let body = |x: &str| x.strip_prefix(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#).unwrap_or(x).to_string();
    assert_eq!(files.get("word/recipientData.xml").map(|x| body(x)), Some(body(RECIPIENTS)), "{what}");
}

/// The merge settings survive every way the document is written: Save As to another `.docx`
/// (and a `.docm`), the copy AutoSave writes on its worker, a recovery copy restored at the next
/// start and saved, an encrypted recovery copy of the document with a password, and a Word XML
/// Document (Flat OPC) opened and saved again.
#[test]
fn mail_merge_settings_survive_save_as_autosave_and_recovery() {
    let dir = std::env::temp_dir().join(format!("wordcraft-merge-save-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let letter_path = dir.join("letter.docx");
    std::fs::write(&letter_path, letter()).unwrap();
    let mut s = Session::new(Document::new());
    s.run("file.open", &json!({"path": letter_path})).unwrap();
    s.run("caret.docEnd", &json!({})).unwrap();
    s.run("text.insert", &json!({"text": " Thanks."})).unwrap();
    for name in ["copy.docx", "copy.docm"] {
        let out = dir.join(name);
        s.run("file.saveAs", &json!({"path": out})).unwrap();
        assert_mail_merge_kept(&unzip(&std::fs::read(&out).unwrap()), name);
    }
    // AutoSave: the document copied and written on a worker thread.
    s.run("text.insert", &json!({"text": " More."})).unwrap();
    let (doc, expected, target) = (s.doc.clone(), s.file_version().cloned(), dir.join("copy.docm"));
    std::thread::spawn(move || io::save_path_versioned(&target, &doc, None, &io::ExportOptions::default(), expected.as_ref()))
        .join()
        .unwrap()
        .unwrap();
    assert_mail_merge_kept(&unzip(&std::fs::read(dir.join("copy.docm")).unwrap()), "AutoSave");
    // Flat OPC, saved and opened again.
    let flat = dir.join("letter.xml");
    s.run("file.saveAs", &json!({"path": flat})).unwrap();
    let back = io::open_path(&flat).unwrap();
    assert_mail_merge_kept(&unzip(&io::save_bytes("again.docx", &back).unwrap()), "Flat OPC");

    // Recovery, plain and encrypted: a copy restored at the next start keeps them.
    for password in [None, Some("synthetic-merge-pass")] {
        let root = dir.join(format!("recovery-{}", password.is_some()));
        let mut s = Session::new(Document::new());
        s.enable_recovery(root.clone(), std::sync::Arc::new(|| {})).unwrap();
        s.run("file.open", &json!({"path": letter_path})).unwrap();
        s.run("caret.docEnd", &json!({})).unwrap();
        s.run("text.insert", &json!({"text": " Unsaved words."})).unwrap();
        if let Some(pw) = password {
            s.run("file.encrypt", &json!({"password": pw})).unwrap();
        }
        assert_eq!(s.emergency_snapshot(std::time::Duration::from_secs(10)), Ok(true));
        drop(s);
        let mut s = Session::new(Document::new());
        s.enable_recovery(root, std::sync::Arc::new(|| {})).unwrap();
        let settle = |s: &mut Session| {
            let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while !(s.recovery_ready() && !s.recovery_busy()) {
                assert!(std::time::Instant::now() < end, "recovery did not finish");
                s.recovery_tick();
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        };
        settle(&mut s);
        let id = s.recovery_documents()[0].id.clone();
        let mut params = json!({"restore": id});
        if let Some(pw) = password {
            params["password"] = json!(pw);
        }
        s.run("file.recover", &params).unwrap();
        settle(&mut s);
        assert!(s.doc.plain_text(StoryRef::Body).contains("Unsaved words."));
        let out = dir.join(format!("restored-{}.docx", password.is_some()));
        s.run("file.saveAs", &json!({"path": out})).unwrap();
        let bytes = std::fs::read(&out).unwrap();
        let bytes = match password {
            // A protected document saves encrypted: open it with its password to look inside.
            Some(_) => io::save_bytes("plain.docx", &io::open_bytes_with("x.docx", &bytes, password).unwrap().0).unwrap(),
            None => bytes,
        };
        assert_mail_merge_kept(&unzip(&bytes), &format!("recovery, protected: {}", password.is_some()));
    }
    let _ = std::fs::remove_dir_all(&dir);
}
