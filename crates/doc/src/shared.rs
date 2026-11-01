//! [`Shared`]: copy-on-write sharing for the big, rarely edited parts of a document (styles,
//! numbering, comments, secondary stories, media, table rows), so a copy of a [`crate::Document`]
//! (an undo step, the snapshot a command can roll back to) shares them with the original
//! until one of the two changes them.
//!
//! It reads like the value it holds (`Deref`), and writing through it (`DerefMut`, `&mut`
//! method calls, assignments to fields) first copies the value if another document still shares
//! it ([`Arc::make_mut`]), so editing code doesn't change. Take `&mut` only to change something:
//! even an unused `&mut` access copies a shared value.
use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A value shared between copies until written (see the module docs). Serialized, compared,
/// hashed and printed as the value itself.
pub struct Shared<T>(Arc<T>);

impl<T> Shared<T> {
    pub fn new(v: T) -> Self {
        Shared(Arc::new(v))
    }
    /// Whether `a` and `b` are the same allocation (so certainly equal, unchanged since one was
    /// copied from the other). A fast path only: different allocations may still be equal.
    pub fn ptr_eq(a: &Self, b: &Self) -> bool {
        Arc::ptr_eq(&a.0, &b.0)
    }
}

impl<T: Clone> Shared<T> {
    /// The value to change, copied first if it is shared.
    pub fn make_mut(this: &mut Self) -> &mut T {
        Arc::make_mut(&mut this.0)
    }
    /// The value, copied if it is shared.
    pub fn into_inner(this: Self) -> T {
        Arc::unwrap_or_clone(this.0)
    }
}

impl<T> Clone for Shared<T> {
    fn clone(&self) -> Self {
        Shared(Arc::clone(&self.0))
    }
}

impl<T: Default> Default for Shared<T> {
    fn default() -> Self {
        Shared::new(T::default())
    }
}

impl<T> Deref for Shared<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: Clone> DerefMut for Shared<T> {
    fn deref_mut(&mut self) -> &mut T {
        Arc::make_mut(&mut self.0)
    }
}

impl<T> From<T> for Shared<T> {
    fn from(v: T) -> Self {
        Shared::new(v)
    }
}

impl<T, A> FromIterator<A> for Shared<T>
where
    T: FromIterator<A>,
{
    fn from_iter<I: IntoIterator<Item = A>>(iter: I) -> Self {
        Shared::new(iter.into_iter().collect())
    }
}

impl<'a, T> IntoIterator for &'a Shared<T>
where
    &'a T: IntoIterator,
{
    type Item = <&'a T as IntoIterator>::Item;
    type IntoIter = <&'a T as IntoIterator>::IntoIter;
    fn into_iter(self) -> Self::IntoIter {
        (&*self.0).into_iter()
    }
}

impl<'a, T: Clone> IntoIterator for &'a mut Shared<T>
where
    &'a mut T: IntoIterator,
{
    type Item = <&'a mut T as IntoIterator>::Item;
    type IntoIter = <&'a mut T as IntoIterator>::IntoIter;
    fn into_iter(self) -> Self::IntoIter {
        Arc::make_mut(&mut self.0).into_iter()
    }
}

impl<T: fmt::Debug> fmt::Debug for Shared<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Compares the values, as before they were shared (a value holding NaN isn't equal to
/// itself). Callers that only ask "unchanged?" can try [`Shared::ptr_eq`] first.
impl<T: PartialEq> PartialEq for Shared<T> {
    fn eq(&self, other: &Self) -> bool {
        *self.0 == *other.0
    }
}

impl<T: Eq> Eq for Shared<T> {}

impl<T: Hash> Hash for Shared<T> {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.0.hash(h)
    }
}

impl<T: Serialize> Serialize for Shared<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Shared<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        T::deserialize(d).map(Shared::new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_copy_only_a_shared_value() {
        let mut a = Shared::new(vec![1, 2, 3]);
        let b = a.clone();
        assert!(Shared::ptr_eq(&a, &b));
        a.push(4);
        assert!(!Shared::ptr_eq(&a, &b));
        assert_eq!(*b, vec![1, 2, 3], "the copy taken before the write is unchanged");
        assert_eq!(*a, vec![1, 2, 3, 4]);
        // Not shared any more: a write changes it in place.
        let p = Arc::as_ptr(&a.0);
        a.push(5);
        assert_eq!(Arc::as_ptr(&a.0), p);
    }

    #[test]
    fn equality_and_serde_are_the_values() {
        let a = Shared::new(vec![f32::NAN]);
        assert!(a != a.clone(), "NaN stays unequal, sharing or not");
        let b: Shared<Vec<u32>> = serde_json::from_str("[1,2]").unwrap_or_default();
        assert_eq!(serde_json::to_string(&b).unwrap_or_default(), "[1,2]");
        assert_eq!(format!("{b:?}"), "[1, 2]");
        let mut n = 0;
        for x in &b {
            n += x;
        }
        assert_eq!(n, 3);
    }
}
