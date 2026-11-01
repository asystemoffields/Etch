use std::sync::Arc;

use wordcraft_doc::graphic::{Embedded, EmbeddedRef, Graphic};
use wordcraft_doc::props::CharProps;
use wordcraft_doc::{Block, Document, InlineObject};

/// Original synthetic model data: sample formatting/tables/footer plus picture and source parts.
fn complete_document() -> Document {
    let mut doc = wordcraft_engine::sample::sample_document();
    let picture = Arc::new(vec![0, 1, 127, 128, 254, 255]);
    doc.media.insert("synthetic.png".into(), picture);
    doc.passthrough.insert("word/embeddings/synthetic.bin".into(), Arc::new(vec![9, 0, 255, 7]));
    let source = Arc::new(Embedded {
        xml: "<synthetic r:id=\"r1\"/>".into(),
        refs: vec![EmbeddedRef {
            id: "r1".into(),
            kind: "urn:synthetic:embedding".into(),
            target: "word/embeddings/synthetic.bin".into(),
            external: false,
        }],
    });
    let Block::Para(p) = Arc::make_mut(doc.body.first_mut().unwrap()) else { panic!("sample starts with a paragraph") };
    p.insert_object(
        0,
        InlineObject::Image {
            media: "synthetic.png".into(),
            w: 12.0,
            h: 12.0,
            alt: "Original synthetic picture".into(),
            float: Default::default(),
            crop: Default::default(),
            ole: Some(source.clone()),
        },
        &CharProps::default(),
    )
    .unwrap();
    p.insert_object(
        0,
        InlineObject::Graphic {
            w: 24.0,
            h: 18.0,
            alt: "Original synthetic chart".into(),
            float: Default::default(),
            graphic: Arc::new(Graphic { source: Some(source), ..Default::default() }),
        },
        &CharProps::default(),
    )
    .unwrap();
    doc
}

#[test]
fn native_json_preserves_the_entire_document() {
    let doc = complete_document();
    let bytes = wordcraft_engine::io::save_bytes("recovery.json", &doc).unwrap();
    let restored = wordcraft_engine::io::open_bytes("recovery.json", &bytes).unwrap();
    assert_eq!(restored.media, doc.media, "picture bytes must survive");
    assert_eq!(restored.passthrough, doc.passthrough, "opaque parts must survive");
    assert_eq!(restored, doc, "object markup and relationships, stories and formatting must survive");
}

#[test]
fn existing_native_json_without_byte_stores_still_opens() {
    let mut value = serde_json::to_value(Document::from_text("Old native document")).unwrap();
    value.as_object_mut().unwrap().remove("media");
    value.as_object_mut().unwrap().remove("passthrough");
    let doc = wordcraft_engine::io::open_bytes("old.json", &serde_json::to_vec(&value).unwrap()).unwrap();
    assert_eq!(doc.plain_text(wordcraft_doc::StoryRef::Body), "Old native document");
}

/// Picture and part bytes are base64 in native JSON: an array of numbers took 3.6 times their
/// size (over six times pretty-printed, as `.json` files are).
#[test]
fn native_json_stores_bytes_compactly() {
    let mut doc = Document::from_text("Synthetic");
    let without = wordcraft_engine::io::save_bytes("doc.json", &doc).unwrap().len();
    let bytes: Vec<u8> = (0..300_000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
    doc.media.insert("big.png".into(), Arc::new(bytes.clone()));
    let saved = wordcraft_engine::io::save_bytes("doc.json", &doc).unwrap();
    assert!(saved.len() - without < bytes.len() * 14 / 10, "{} bytes for {}", saved.len() - without, bytes.len());
    assert_eq!(wordcraft_engine::io::open_bytes("doc.json", &saved).unwrap().media, doc.media);
    // Bytes that aren't base64 are an error, never a panic.
    let mut value: serde_json::Value = serde_json::from_slice(&saved).unwrap();
    value["media"]["big.png"] = serde_json::json!("not base64!");
    assert!(wordcraft_engine::io::open_bytes("doc.json", &serde_json::to_vec(&value).unwrap()).is_err());
}
