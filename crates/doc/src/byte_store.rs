//! How [`crate::Document::media`] and [`crate::Document::passthrough`] are written in the native
//! JSON form (`.json` files, crash-recovery copies): each part's bytes as one base64 string.
//! An array of numbers, serde's default for bytes, took 3.6 times the bytes compact and over
//! six times pretty-printed (a 27 MB picture made a 99 MB recovery copy).
use std::collections::BTreeMap;
use std::sync::Arc;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serializer};

type Store = crate::Shared<BTreeMap<String, Arc<Vec<u8>>>>;

pub(crate) fn is_empty(store: &Store) -> bool {
    store.is_empty()
}

pub(crate) fn serialize<S: Serializer>(store: &Store, s: S) -> Result<S::Ok, S::Error> {
    // One part encoded at a time, so only one extra copy is held.
    s.collect_map(store.iter().map(|(name, bytes)| (name, encode(bytes))))
}

pub(crate) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Store, D::Error> {
    let raw = BTreeMap::<String, Part>::deserialize(d)?;
    raw.into_iter()
        .map(|(name, part)| match part {
            Part::Bytes(bytes) => Ok((name, Arc::new(bytes))),
            Part::Text(text) => match decode(&text) {
                Some(bytes) => Ok((name, Arc::new(bytes))),
                None => Err(D::Error::custom(format!("{name}: not base64"))),
            },
        })
        .collect::<Result<BTreeMap<_, _>, _>>()
        .map(crate::Shared::new)
}

/// One part as written: base64 text, or (files and crash-recovery copies written before base64)
/// an array of numbers, read straight into bytes.
enum Part {
    Text(String),
    Bytes(Vec<u8>),
}

impl<'de> Deserialize<'de> for Part {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = Part;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("base64 text or an array of bytes")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Part, E> {
                Ok(Part::Text(v.to_string()))
            }
            fn visit_string<E: serde::de::Error>(self, v: String) -> Result<Part, E> {
                Ok(Part::Text(v))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Part, A::Error> {
                // The hint is the input's claim: capped, never trusted for the allocation.
                let mut out = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(1 << 20));
                while let Some(b) = seq.next_element::<u8>()? {
                    out.push(b);
                }
                Ok(Part::Bytes(out))
            }
        }
        d.deserialize_any(Visitor)
    }
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding.
pub fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3).saturating_mul(4));
    for chunk in bytes.chunks(3) {
        let b = [chunk.first().copied().unwrap_or(0), chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for (i, shift) in [18u32, 12, 6, 0].into_iter().enumerate() {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET.get(((n >> shift) & 63) as usize).copied().unwrap_or(b'A')));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// [`encode`]'s inverse; `None` for anything that isn't base64 (padding is optional).
pub fn decode(text: &str) -> Option<Vec<u8>> {
    let text = text.trim_end_matches('=');
    let mut out = Vec::with_capacity(text.len() / 4 * 3 + 2);
    let (mut buf, mut bits) = (0u32, 0u32);
    for c in text.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        buf = (buf << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    // A lone sixth bit group can't end a valid encoding.
    (bits < 6).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_length_and_byte() {
        let all: Vec<u8> = (0..=255).collect();
        for n in 0..all.len() {
            let bytes = &all[..n];
            assert_eq!(decode(&encode(bytes)).as_deref(), Some(bytes), "{n}");
        }
        assert_eq!(encode(b"Man"), "TWFu");
        assert_eq!(encode(b"Ma"), "TWE=");
        assert_eq!(encode(b"M"), "TQ==");
    }

    #[test]
    fn hostile_text_is_refused_not_a_panic() {
        for bad in ["T", "TWFu!", "\u{e9}", "TQ=x", "-_"] {
            assert_eq!(decode(bad), None, "{bad}");
        }
        assert_eq!(decode(""), Some(Vec::new()));
    }

    /// Native files and recovery copies written before base64 hold arrays of numbers: they
    /// still open (they failed with "invalid type: sequence, expected a string").
    #[test]
    fn arrays_of_numbers_still_read() {
        let mut doc = crate::Document::from_text("Old synthetic file");
        let mut value = serde_json::to_value(&doc).unwrap();
        value["media"] = serde_json::json!({"a.png": [0, 1, 255], "b.png": "AAH/"});
        let read: crate::Document = serde_json::from_value(value.clone()).unwrap();
        doc.media.insert("a.png".into(), Arc::new(vec![0, 1, 255]));
        doc.media.insert("b.png".into(), Arc::new(vec![0, 1, 255]));
        assert_eq!(read.media, doc.media);
        // A number that isn't a byte, or another type: an error, not a panic.
        for bad in [serde_json::json!([256]), serde_json::json!([-1]), serde_json::json!({}), serde_json::json!(true)] {
            value["media"] = serde_json::json!({ "a.png": bad });
            assert!(serde_json::from_value::<crate::Document>(value.clone()).is_err());
        }
    }
}
