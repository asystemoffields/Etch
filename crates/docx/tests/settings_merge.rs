//! Settings a save used to drop: `word/settings.xml` is merged into the file's original, so
//! settings the model doesn't hold (compatibility options, rsids, document variables, proofing
//! state, "remove personal information", a mail merge's data source with its relationships…)
//! survive, and owned ones follow the model. Hand-written fixtures (from the spec), no files
//! produced by Word.

use std::collections::BTreeMap;
use std::io::{Read, Write};

const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const R_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

fn zip(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut zw = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        zw.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
        zw.write_all(bytes).unwrap();
    }
    zw.finish().unwrap().into_inner()
}

fn unzip(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut out = BTreeMap::new();
    for i in 0..z.len() {
        let mut f = z.by_index(i).unwrap();
        let mut b = Vec::new();
        f.read_to_end(&mut b).unwrap();
        assert!(out.insert(f.name().to_string(), b).is_none(), "duplicate part {}", f.name());
    }
    out
}

fn text(files: &BTreeMap<String, Vec<u8>>, name: &str) -> String {
    String::from_utf8(files.get(name).unwrap_or_else(|| panic!("no {name} in {:?}", files.keys())).clone()).unwrap()
}

fn rels(list: &[(&str, &str, &str)]) -> Vec<u8> {
    let mut s =
        String::from(r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#);
    for (id, ty, target) in list {
        s.push_str(&format!(r#"<Relationship Id="{id}" Type="{ty}" Target="{target}"/>"#));
    }
    s.push_str("</Relationships>");
    s.into_bytes()
}

/// A package: `body`, document relationships, root relationships after the main document's,
/// and extra parts.
fn package(body: &str, doc_rels: &[(&str, &str, &str)], root_rels: &[(&str, &str, &str)], extra: &[(&str, &str)]) -> Vec<u8> {
    let doc = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="{W}" xmlns:r="{R_NS}" xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml"><w:body>{body}</w:body></w:document>"#
    );
    let mut root = vec![("rIdMain", "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument", "word/document.xml")];
    root.extend_from_slice(root_rels);
    let mut entries: Vec<(&str, Vec<u8>)> =
        vec![("_rels/.rels", rels(&root)), ("word/document.xml", doc.into_bytes()), ("word/_rels/document.xml.rels", rels(doc_rels))];
    for (p, c) in extra {
        entries.push((p, c.as_bytes().to_vec()));
    }
    zip(&entries)
}

fn save(doc: &wordcraft_doc::Document) -> BTreeMap<String, Vec<u8>> {
    unzip(&wordcraft_docx::write(doc).expect("write"))
}

const SETTINGS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:settings xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml" xmlns:w15="http://schemas.microsoft.com/office/word/2012/wordml" mc:Ignorable="w14 w15"><w:zoom w:percent="120"/><w:removePersonalInformation/><w:removeDateAndTime/><w:proofState w:spelling="clean" w:grammar="clean"/><w:mailMerge><w:mainDocumentType w:val="formLetters"/><w:dataSource r:id="rId9"/></w:mailMerge><w:trackRevisions/><w:defaultTabStop w:val="708"/><w:hyphenationZone w:val="425"/><w:characterSpacingControl w:val="doNotCompress"/><w:compat><w:compatSetting w:name="compatibilityMode" w:uri="http://schemas.microsoft.com/office/word" w:val="15"/><w:compatSetting w:name="overrideTableStyleFontSizeAndJustification" w:uri="http://schemas.microsoft.com/office/word" w:val="1"/><w:compatSetting w:name="enableOpenTypeFeatures" w:uri="http://schemas.microsoft.com/office/word" w:val="1"/></w:compat><w:docVars><w:docVar w:name="ClauseSet" w:val="B"/></w:docVars><w:rsids><w:rsidRoot w:val="00A1B2C3"/><w:rsid w:val="00A1B2C3"/></w:rsids><m:mathPr><m:mathFont m:val="Cambria Math"/><m:smallFrac m:val="0"/></m:mathPr><w:themeFontLang w:val="pl-PL"/><w:decimalSymbol w:val=","/><w:listSeparator w:val=";"/><w14:docId w14:val="1A2B3C4D"/></w:settings>"#;

fn with_settings(settings: &str) -> Vec<u8> {
    package("<w:p><w:r><w:t>Text</w:t></w:r></w:p>", &[("rId1", &format!("{REL}/settings"), "settings.xml")], &[], &[("word/settings.xml", settings)])
}

fn pos(s: &str, needle: &str) -> usize {
    s.find(needle).unwrap_or_else(|| panic!("{needle} missing from {s}"))
}

#[test]
fn unmodelled_settings_survive_a_save_and_owned_ones_follow_the_model() {
    let mut doc = wordcraft_docx::read(&with_settings(SETTINGS)).unwrap();
    assert!(doc.settings.track_changes);
    doc.settings.track_changes = false;
    let files = save(&doc);
    let s = text(&files, "word/settings.xml");
    // Kept byte for byte: everything the model doesn't hold, the original root and zoom.
    for kept in [
        r#"<w:zoom w:percent="120"/>"#,
        r#"<w:proofState w:spelling="clean" w:grammar="clean"/>"#,
        r#"w:name="overrideTableStyleFontSizeAndJustification""#,
        r#"w:name="enableOpenTypeFeatures""#,
        r#"<w:docVar w:name="ClauseSet" w:val="B"/>"#,
        r#"<w:rsidRoot w:val="00A1B2C3"/>"#,
        r#"<m:mathFont m:val="Cambria Math"/>"#,
        r#"<w:themeFontLang w:val="pl-PL"/>"#,
        r#"<w:decimalSymbol w:val=","/>"#,
        r#"<w:listSeparator w:val=";"/>"#,
        r#"<w14:docId w14:val="1A2B3C4D"/>"#,
        r#"mc:Ignorable="w14 w15""#,
    ] {
        assert!(s.contains(kept), "{kept} lost: {s}");
    }
    // Owned: Track Changes was turned off; one compatibility mode; the mail merge source (a
    // relationship the file doesn't have) is dropped rather than left dangling.
    assert!(!s.contains("trackRevisions"), "{s}");
    assert_eq!(s.matches("compatibilityMode").count(), 1, "{s}");
    assert!(!s.contains("mailMerge") && !s.contains("rId9"), "{s}");
    // The privacy options are the model's (#1062): written once each, and gone when turned off.
    assert_eq!(s.matches("removePersonalInformation").count(), 1, "{s}");
    assert_eq!(s.matches("removeDateAndTime").count(), 1, "{s}");
    // CT_Settings order holds for kept and owned children.
    assert!(pos(&s, "removePersonalInformation") < pos(&s, "defaultTabStop"));
    assert!(pos(&s, "defaultTabStop") < pos(&s, "<w:compat>"));
    assert!(pos(&s, "<w:compat>") < pos(&s, "<w:rsids>"));
    // Turning Track Changes back on inserts it at its place.
    doc.settings.track_changes = true;
    let s = text(&save(&doc), "word/settings.xml");
    assert!(pos(&s, "proofState") < pos(&s, "<w:trackRevisions/>") && pos(&s, "<w:trackRevisions/>") < pos(&s, "defaultTabStop"), "{s}");
    let mut private = doc.clone();
    private.settings.remove_personal_info = false;
    private.settings.remove_date_time = false;
    let p = text(&save(&private), "word/settings.xml");
    assert!(!p.contains("removePersonalInformation") && !p.contains("removeDateAndTime"), "{p}");
    // A second generation is identical.
    let again = wordcraft_docx::read(&wordcraft_docx::write(&doc).unwrap()).unwrap();
    assert_eq!(text(&save(&again), "word/settings.xml"), s);
}

#[test]
fn settings_written_by_wordcraft_keep_no_original() {
    let doc = wordcraft_doc::Document::new();
    let back = wordcraft_docx::read(&wordcraft_docx::write(&doc).unwrap()).unwrap();
    assert!(!back.passthrough.contains_key("wordcraft:settings.orig"), "{:?}", back.passthrough.keys());
    assert_eq!(text(&save(&back), "word/settings.xml"), text(&save(&doc), "word/settings.xml"));
}

/// A mail merge main document as Word saves one (ECMA-376 Part 1 §17.14): the merge settings
/// name the data source and header source by external relationships of the settings part, and
/// the recipient data (which records are in the merge) by a part of its own.
const MAIL_MERGE: &str = r#"<w:mailMerge><w:mainDocumentType w:val="formLetters"/><w:linkToQuery/><w:dataType w:val="native"/><w:connectString w:val="Provider=Example.OLEDB;Data Source=C:\Lists\guests.xlsx;Mode=Read"/><w:query w:val="SELECT * FROM `Guests$` WHERE `City` = 'Oslo'"/><w:dataSource r:id="rId2"/><w:headerSource r:id="rId3"/><w:viewMergedData/><w:odso><w:udl w:val="Provider=Example.OLEDB;Data Source=C:\Lists\guests.xlsx"/><w:table w:val="Guests$"/><w:src r:id="rId2"/><w:colDelim w:val="9"/><w:fHdr/><w:fieldMapData><w:type w:val="dbColumn"/><w:name w:val="First Name"/><w:mappedName w:val="First Name"/><w:column w:val="0"/><w:lid w:val="en-US"/></w:fieldMapData><w:recipientData r:id="rId4"/></w:odso></w:mailMerge>"#;

const RECIPIENTS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:recipients xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:recipientData><w:active w:val="1"/><w:column w:val="0"/><w:uniqueTag>QWRh</w:uniqueTag></w:recipientData><w:recipientData><w:active w:val="0"/><w:column w:val="0"/><w:uniqueTag>QWxhbg==</w:uniqueTag></w:recipientData></w:recipients>"#;

fn mail_merge_package() -> Vec<u8> {
    let settings = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:settings xmlns:w="{W}" xmlns:r="{R_NS}"><w:zoom w:percent="100"/><w:attachedTemplate r:id="rId1"/>{MAIL_MERGE}<w:defaultTabStop w:val="720"/></w:settings>"#
    );
    let settings_rels = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="{REL}/attachedTemplate" Target="file:///C:/Templates/Letter.dotx" TargetMode="External"/><Relationship Id="rId2" Type="{REL}/mailMergeSource" Target="file:///C:\Lists\guests.xlsx" TargetMode="External"/><Relationship Id="rId3" Type="{REL}/mailMergeHeaderSource" Target="file:///C:\Lists\header.docx" TargetMode="External"/><Relationship Id="rId4" Type="{REL}/recipientData" Target="recipientData.xml"/></Relationships>"#
    );
    let ct = r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/settings.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml"/><Override PartName="/word/recipientData.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.mailMergeRecipientData+xml"/></Types>"#;
    let body = r#"<w:p><w:r><w:t xml:space="preserve">Dear </w:t></w:r><w:fldSimple w:instr=" MERGEFIELD &quot;First Name&quot; "><w:r><w:t>«First Name»</w:t></w:r></w:fldSimple><w:r><w:t>,</w:t></w:r></w:p>"#;
    package(
        body,
        &[("rId1", &format!("{REL}/settings"), "settings.xml")],
        &[],
        &[
            ("[Content_Types].xml", ct),
            ("word/settings.xml", &settings),
            ("word/_rels/settings.xml.rels", &settings_rels),
            ("word/recipientData.xml", RECIPIENTS),
        ],
    )
}

/// The relationships of a written settings part: (id, type's last segment, target, external).
fn settings_rels(files: &BTreeMap<String, Vec<u8>>) -> Vec<(String, String, String, bool)> {
    let xml = text(files, "word/_rels/settings.xml.rels");
    let attr = |tag: &str, name: &str| {
        let at = tag.find(&format!("{name}=\""))? + name.len() + 2;
        tag.get(at..).and_then(|t| t.split('"').next()).map(str::to_string)
    };
    xml.split("<Relationship ")
        .skip(1)
        .filter_map(|tag| {
            let kind = attr(tag, "Type")?;
            Some((
                attr(tag, "Id")?,
                kind.rsplit('/').next()?.to_string(),
                attr(tag, "Target")?,
                attr(tag, "TargetMode").as_deref() == Some("External"),
            ))
        })
        .collect()
}

/// Etch has no mail merge, but documents people receive may be mail merge
/// main documents. Their merge settings (data source, query, main document type, field mapping,
/// recipient data) survive an open, an edit and a save byte for byte, with the relationships they
/// name under the same ids and the recipient data part carried verbatim; saved again, the same.
#[test]
fn mail_merge_settings_and_their_relationships_survive_a_save() {
    let input = mail_merge_package();
    let mut doc = wordcraft_docx::read(&input).unwrap();
    assert_eq!(doc.settings.attached_template.as_deref(), Some("file:///C:/Templates/Letter.dotx"));
    // An edit: the merge settings don't depend on the body.
    doc.body.push(std::sync::Arc::new(wordcraft_doc::Block::Para(wordcraft_doc::Paragraph::with_text("P.S.", Default::default()))));
    let files = save(&doc);
    let s = text(&files, "word/settings.xml");
    assert_eq!(s.matches(MAIL_MERGE).count(), 1, "the merge settings, byte for byte: {s}");
    assert!(pos(&s, "attachedTemplate") < pos(&s, "<w:mailMerge>") && pos(&s, "</w:mailMerge>") < pos(&s, "defaultTabStop"), "{s}");
    let rels = settings_rels(&files);
    let rel = |id: &str| rels.iter().find(|r| r.0 == id).cloned();
    assert_eq!(rel("rId2"), Some(("rId2".into(), "mailMergeSource".into(), r"file:///C:\Lists\guests.xlsx".into(), true)), "{rels:?}");
    assert_eq!(rel("rId3"), Some(("rId3".into(), "mailMergeHeaderSource".into(), r"file:///C:\Lists\header.docx".into(), true)), "{rels:?}");
    assert_eq!(rel("rId4").map(|r| (r.1, r.3)), Some(("recipientData".into(), false)), "{rels:?}");
    let data = rel("rId4").map(|r| format!("word/{}", r.2)).unwrap();
    assert_eq!(files.get(&data).map(Vec::as_slice), Some(RECIPIENTS.as_bytes()), "the recipient data part, verbatim");
    let ct = text(&files, "[Content_Types].xml");
    assert!(
        ct.contains(&format!(
            r#"PartName="/{data}" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.mailMergeRecipientData+xml""#
        )),
        "{ct}"
    );
    // The attached template keeps its own relationship, under an id the kept markup doesn't use.
    let template: Vec<_> = rels.iter().filter(|r| r.1 == "attachedTemplate").collect();
    assert_eq!(template.len(), 1, "{rels:?}");
    assert!(!["rId2", "rId3", "rId4"].contains(&template[0].0.as_str()), "{rels:?}");
    assert!(s.contains(&format!(r#"<w:attachedTemplate r:id="{}"/>"#, template[0].0)), "{s}");
    let mut ids: Vec<&str> = rels.iter().map(|r| r.0.as_str()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), rels.len(), "relationship ids are unique: {rels:?}");
    // Saved again: the same settings, relationships and recipient data.
    let again = save(&wordcraft_docx::read(&wordcraft_docx::write(&doc).unwrap()).unwrap());
    assert_eq!(text(&again, "word/settings.xml"), s);
    assert_eq!(settings_rels(&again), rels);
    assert_eq!(again.get(&data), files.get(&data));
}

/// A merge setting naming a relationship the file doesn't have is dropped rather than left
/// dangling; the other merge settings and the rest of the part stay.
#[test]
fn a_mail_merge_source_without_its_relationship_is_dropped() {
    let settings = format!(
        r#"<w:settings xmlns:w="{W}" xmlns:r="{R_NS}"><w:mailMerge><w:mainDocumentType w:val="catalog"/><w:dataSource r:id="rId7"/></w:mailMerge><w:rsids><w:rsidRoot w:val="00112233"/></w:rsids></w:settings>"#
    );
    let files = save(&wordcraft_docx::read(&with_settings(&settings)).unwrap());
    let s = text(&files, "word/settings.xml");
    assert!(!s.contains("mailMerge") && !s.contains("rId7"), "{s}");
    assert!(s.contains(r#"<w:rsidRoot w:val="00112233"/>"#), "{s}");
}

/// Gap #5: `w:doNotTrackMoves` (ECMA-376 §17.15.1.29) turns Track moves off; it is read, written
/// back right after `w:trackRevisions` (in a merged part and a generated one), and left out
/// while moves are tracked.
#[test]
fn do_not_track_moves_round_trips() {
    let original = SETTINGS.replace("<w:trackRevisions/>", "<w:trackRevisions/><w:doNotTrackMoves/>");
    let mut doc = wordcraft_docx::read(&with_settings(&original)).unwrap();
    assert!(!doc.settings.track_moves);
    let s = text(&save(&doc), "word/settings.xml");
    assert_eq!(s.matches("<w:doNotTrackMoves/>").count(), 1, "{s}");
    assert!(pos(&s, "<w:trackRevisions/>") < pos(&s, "<w:doNotTrackMoves/>") && pos(&s, "<w:doNotTrackMoves/>") < pos(&s, "<w:defaultTabStop"));
    assert!(s.contains("ClauseSet"), "a merged part: {s}");
    doc.settings.track_moves = true;
    let s = text(&save(&doc), "word/settings.xml");
    assert!(!s.contains("doNotTrackMoves"), "{s}");

    // `w:val="false"` keeps moves tracked.
    let off = SETTINGS.replace("<w:trackRevisions/>", r#"<w:trackRevisions/><w:doNotTrackMoves w:val="false"/>"#);
    assert!(wordcraft_docx::read(&with_settings(&off)).unwrap().settings.track_moves);

    // A document of our own: absent by default, written when off, and read back.
    let mut doc = wordcraft_doc::Document::new();
    assert!(doc.settings.track_moves);
    assert!(!text(&save(&doc), "word/settings.xml").contains("doNotTrackMoves"));
    doc.settings.track_moves = false;
    let s = text(&save(&doc), "word/settings.xml");
    assert!(s.contains("<w:doNotTrackMoves/>"), "{s}");
    assert!(!wordcraft_docx::read(&wordcraft_docx::write(&doc).unwrap()).unwrap().settings.track_moves);
}
