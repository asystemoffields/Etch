//! Structural hashing of anything `Serialize`: a serde serializer that feeds every field
//! straight into a [`Hasher`] instead of building text. Cache keys use it for document
//! formatting (styles, numbering, table-style cell formatting) that has no `Hash` of its own
//! (it holds `f32`s): no allocation, and any change to any field changes the hash.

use serde::ser::{self, Serialize};
use std::fmt;
use std::hash::Hasher;

/// The structural hash of `v` (stable within a process run, like `DefaultHasher`).
pub(crate) fn hash_ser<T: Serialize + ?Sized>(v: &T) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    feed(&mut h, v);
    h.finish()
}

/// Feed `v`'s structure into `h`.
pub(crate) fn feed<H: Hasher, T: Serialize + ?Sized>(h: &mut H, v: &T) {
    // Serializing into a hasher can't fail; a type whose `Serialize` reports an error still
    // hashes what it fed before it (and the error's tag), which only costs cache hits.
    if v.serialize(HashSer(h)).is_err() {
        h.write_u8(0xEE);
    }
}

struct HashSer<'a, H: Hasher>(&'a mut H);

/// Never produced by [`HashSer`] itself; serde needs an error type.
#[derive(Debug)]
pub(crate) struct HashErr;

impl fmt::Display for HashErr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("hash serializer error")
    }
}

impl std::error::Error for HashErr {}

impl ser::Error for HashErr {
    fn custom<T: fmt::Display>(_: T) -> Self {
        HashErr
    }
}

// Tags keep differently shaped values apart (`None` vs `Some(())`, a unit vs an empty string…).
const T_NONE: u8 = 1;
const T_SOME: u8 = 2;
const T_UNIT: u8 = 3;
const T_SEQ: u8 = 4;
const T_MAP: u8 = 5;
const T_STRUCT: u8 = 6;
const T_VARIANT: u8 = 7;
const T_END: u8 = 8;

impl<'a, H: Hasher> ser::Serializer for HashSer<'a, H> {
    type Ok = ();
    type Error = HashErr;
    type SerializeSeq = Self;
    type SerializeTuple = Self;
    type SerializeTupleStruct = Self;
    type SerializeTupleVariant = Self;
    type SerializeMap = Self;
    type SerializeStruct = Self;
    type SerializeStructVariant = Self;

    fn serialize_bool(self, v: bool) -> Result<(), HashErr> {
        self.0.write_u8(u8::from(v));
        Ok(())
    }
    fn serialize_i8(self, v: i8) -> Result<(), HashErr> {
        self.0.write_i8(v);
        Ok(())
    }
    fn serialize_i16(self, v: i16) -> Result<(), HashErr> {
        self.0.write_i16(v);
        Ok(())
    }
    fn serialize_i32(self, v: i32) -> Result<(), HashErr> {
        self.0.write_i32(v);
        Ok(())
    }
    fn serialize_i64(self, v: i64) -> Result<(), HashErr> {
        self.0.write_i64(v);
        Ok(())
    }
    fn serialize_i128(self, v: i128) -> Result<(), HashErr> {
        self.0.write_i128(v);
        Ok(())
    }
    fn serialize_u8(self, v: u8) -> Result<(), HashErr> {
        self.0.write_u8(v);
        Ok(())
    }
    fn serialize_u16(self, v: u16) -> Result<(), HashErr> {
        self.0.write_u16(v);
        Ok(())
    }
    fn serialize_u32(self, v: u32) -> Result<(), HashErr> {
        self.0.write_u32(v);
        Ok(())
    }
    fn serialize_u64(self, v: u64) -> Result<(), HashErr> {
        self.0.write_u64(v);
        Ok(())
    }
    fn serialize_u128(self, v: u128) -> Result<(), HashErr> {
        self.0.write_u128(v);
        Ok(())
    }
    fn serialize_f32(self, v: f32) -> Result<(), HashErr> {
        self.0.write_u32(v.to_bits());
        Ok(())
    }
    fn serialize_f64(self, v: f64) -> Result<(), HashErr> {
        self.0.write_u64(v.to_bits());
        Ok(())
    }
    fn serialize_char(self, v: char) -> Result<(), HashErr> {
        self.0.write_u32(u32::from(v));
        Ok(())
    }
    fn serialize_str(self, v: &str) -> Result<(), HashErr> {
        self.0.write_usize(v.len());
        self.0.write(v.as_bytes());
        Ok(())
    }
    fn serialize_bytes(self, v: &[u8]) -> Result<(), HashErr> {
        self.0.write_usize(v.len());
        self.0.write(v);
        Ok(())
    }
    fn serialize_none(self) -> Result<(), HashErr> {
        self.0.write_u8(T_NONE);
        Ok(())
    }
    fn serialize_some<T: Serialize + ?Sized>(self, v: &T) -> Result<(), HashErr> {
        self.0.write_u8(T_SOME);
        v.serialize(self)
    }
    fn serialize_unit(self) -> Result<(), HashErr> {
        self.0.write_u8(T_UNIT);
        Ok(())
    }
    fn serialize_unit_struct(self, _: &'static str) -> Result<(), HashErr> {
        self.0.write_u8(T_UNIT);
        Ok(())
    }
    fn serialize_unit_variant(self, _: &'static str, i: u32, _: &'static str) -> Result<(), HashErr> {
        self.0.write_u8(T_VARIANT);
        self.0.write_u32(i);
        Ok(())
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(self, _: &'static str, v: &T) -> Result<(), HashErr> {
        v.serialize(self)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(self, _: &'static str, i: u32, _: &'static str, v: &T) -> Result<(), HashErr> {
        self.0.write_u8(T_VARIANT);
        self.0.write_u32(i);
        v.serialize(self)
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<Self, HashErr> {
        self.0.write_u8(T_SEQ);
        if let Some(n) = len {
            self.0.write_usize(n);
        }
        Ok(self)
    }
    fn serialize_tuple(self, _: usize) -> Result<Self, HashErr> {
        self.0.write_u8(T_SEQ);
        Ok(self)
    }
    fn serialize_tuple_struct(self, _: &'static str, _: usize) -> Result<Self, HashErr> {
        self.0.write_u8(T_STRUCT);
        Ok(self)
    }
    fn serialize_tuple_variant(self, _: &'static str, i: u32, _: &'static str, _: usize) -> Result<Self, HashErr> {
        self.0.write_u8(T_VARIANT);
        self.0.write_u32(i);
        Ok(self)
    }
    fn serialize_map(self, len: Option<usize>) -> Result<Self, HashErr> {
        self.0.write_u8(T_MAP);
        if let Some(n) = len {
            self.0.write_usize(n);
        }
        Ok(self)
    }
    fn serialize_struct(self, _: &'static str, _: usize) -> Result<Self, HashErr> {
        self.0.write_u8(T_STRUCT);
        Ok(self)
    }
    fn serialize_struct_variant(self, _: &'static str, i: u32, _: &'static str, _: usize) -> Result<Self, HashErr> {
        self.0.write_u8(T_VARIANT);
        self.0.write_u32(i);
        Ok(self)
    }
}

impl<'a, H: Hasher> HashSer<'a, H> {
    fn elem<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), HashErr> {
        v.serialize(HashSer(&mut *self.0))
    }
    fn end(self) -> Result<(), HashErr> {
        self.0.write_u8(T_END);
        Ok(())
    }
}

impl<'a, H: Hasher> ser::SerializeSeq for HashSer<'a, H> {
    type Ok = ();
    type Error = HashErr;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), HashErr> {
        self.elem(v)
    }
    fn end(self) -> Result<(), HashErr> {
        HashSer::end(self)
    }
}

impl<'a, H: Hasher> ser::SerializeTuple for HashSer<'a, H> {
    type Ok = ();
    type Error = HashErr;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), HashErr> {
        self.elem(v)
    }
    fn end(self) -> Result<(), HashErr> {
        HashSer::end(self)
    }
}

impl<'a, H: Hasher> ser::SerializeTupleStruct for HashSer<'a, H> {
    type Ok = ();
    type Error = HashErr;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), HashErr> {
        self.elem(v)
    }
    fn end(self) -> Result<(), HashErr> {
        HashSer::end(self)
    }
}

impl<'a, H: Hasher> ser::SerializeTupleVariant for HashSer<'a, H> {
    type Ok = ();
    type Error = HashErr;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), HashErr> {
        self.elem(v)
    }
    fn end(self) -> Result<(), HashErr> {
        HashSer::end(self)
    }
}

impl<'a, H: Hasher> ser::SerializeMap for HashSer<'a, H> {
    type Ok = ();
    type Error = HashErr;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, k: &T) -> Result<(), HashErr> {
        self.elem(k)
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), HashErr> {
        self.elem(v)
    }
    fn end(self) -> Result<(), HashErr> {
        HashSer::end(self)
    }
}

impl<'a, H: Hasher> ser::SerializeStruct for HashSer<'a, H> {
    type Ok = ();
    type Error = HashErr;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, key: &'static str, v: &T) -> Result<(), HashErr> {
        // The field name, so a field that is skipped when empty can't alias its neighbour.
        self.0.write(key.as_bytes());
        self.elem(v)
    }
    fn skip_field(&mut self, key: &'static str) -> Result<(), HashErr> {
        self.0.write(key.as_bytes());
        self.0.write_u8(T_NONE);
        Ok(())
    }
    fn end(self) -> Result<(), HashErr> {
        HashSer::end(self)
    }
}

impl<'a, H: Hasher> ser::SerializeStructVariant for HashSer<'a, H> {
    type Ok = ();
    type Error = HashErr;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, key: &'static str, v: &T) -> Result<(), HashErr> {
        self.0.write(key.as_bytes());
        self.elem(v)
    }
    fn end(self) -> Result<(), HashErr> {
        HashSer::end(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_follows_every_field() {
        use wordcraft_doc::props::{CharProps, ParaProps};
        let a = CharProps::default();
        let mut b = a.clone();
        assert_eq!(hash_ser(&a), hash_ser(&b));
        b.size = Some(11.0);
        assert_ne!(hash_ser(&a), hash_ser(&b));
        let mut c = b.clone();
        c.size = Some(11.5);
        assert_ne!(hash_ser(&b), hash_ser(&c));
        let p = ParaProps::default();
        let mut q = p.clone();
        q.space_after = Some(0.0);
        assert_ne!(hash_ser(&p), hash_ser(&q));
        // Shapes are told apart.
        assert_ne!(hash_ser(&None::<u8>), hash_ser(&Some(())));
        assert_ne!(hash_ser(&("ab", "c")), hash_ser(&("a", "bc")));
        assert_ne!(hash_ser(&(vec![1u8], Vec::<u8>::new())), hash_ser(&(Vec::<u8>::new(), vec![1u8])));
    }
}
