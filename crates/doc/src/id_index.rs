//! A lookup by id beside a list that is searched far more often than it changes: a document's
//! styles (layout and the readers look a style up for every run) and lists. A search through
//! 10,000 styles for each of 750,000 runs took most of a minute.
//!
//! The index is checked on every use, so it never answers differently from a search through the
//! list from the start (unless the list was changed in place to hold the same id twice): it
//! knows which list it was built for (its buffer and length), takes in items appended since, is
//! rebuilt for any other list, and a hit is confirmed against the list itself; a miss on a list
//! changed in place is searched for. A copy of the list's owner starts without one.

use std::borrow::Borrow;
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::RwLock;

/// Lists shorter than this are searched: the index would cost more than it saves.
const INDEXED: usize = 64;

/// See the module's documentation.
pub(crate) struct IdIndex<K> {
    built: RwLock<Option<Built<K>>>,
}

struct Built<K> {
    /// The list's buffer and length when last looked at.
    ptr: usize,
    len: usize,
    /// Id → the position of the first item with it.
    at: HashMap<K, usize>,
}

impl<K> Default for IdIndex<K> {
    fn default() -> Self {
        IdIndex { built: RwLock::new(None) }
    }
}

impl<K> Clone for IdIndex<K> {
    /// A copy builds its own (the copy's list is another buffer).
    fn clone(&self) -> Self {
        IdIndex::default()
    }
}

impl<K> PartialEq for IdIndex<K> {
    /// Not part of what its owner is.
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl<K> std::fmt::Debug for IdIndex<K> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("IdIndex")
    }
}

impl<K: Hash + Eq> IdIndex<K> {
    /// The position of the first item of `items` whose id (`id_of`) is `id`, as a search from the
    /// start finds it.
    pub(crate) fn position<T, Q>(&self, items: &[T], id: &Q, id_of: impl Fn(&T) -> &Q, key: impl Fn(&T) -> K) -> Option<usize>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let search = || items.iter().position(|t| id_of(t) == id);
        if items.len() < INDEXED {
            return search();
        }
        let (ptr, len) = (items.as_ptr() as usize, items.len());
        if let Ok(b) = self.built.read()
            && let Some(b) = b.as_ref()
            && (b.ptr, b.len) == (ptr, len)
        {
            return match b.at.get(id) {
                Some(&i) if items.get(i).is_some_and(|t| id_of(t) == id) => Some(i),
                // Moved or renamed in place since: searched.
                _ => search(),
            };
        }
        let Ok(mut guard) = self.built.write() else { return search() };
        match guard.as_mut() {
            // Items appended since (a reader adding styles one by one): taken in.
            Some(b) if b.ptr == ptr && b.len < len => {
                for (i, t) in items.iter().enumerate().skip(b.len) {
                    b.at.entry(key(t)).or_insert(i);
                }
                b.len = len;
            }
            _ => {
                let mut at = HashMap::with_capacity(len);
                for (i, t) in items.iter().enumerate() {
                    at.entry(key(t)).or_insert(i);
                }
                *guard = Some(Built { ptr, len, at });
            }
        }
        match guard.as_ref().and_then(|b| b.at.get(id)) {
            Some(&i) if items.get(i).is_some_and(|t| id_of(t) == id) => Some(i),
            _ => search(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(n: usize) -> Vec<(String, usize)> {
        (0..n).map(|i| (format!("S{i}"), i)).collect()
    }

    fn find(ix: &IdIndex<String>, v: &[(String, usize)], id: &str) -> Option<usize> {
        ix.position(v, id, |t| t.0.as_str(), |t| t.0.clone())
    }

    /// Whatever is done to the list, the index answers as a search from the start does.
    #[test]
    fn the_index_answers_as_a_search_does() {
        let ix = IdIndex::default();
        let mut v = items(200);
        for id in ["S0", "S150", "S199", "S200", "nope"] {
            assert_eq!(find(&ix, &v, id), v.iter().position(|t| t.0 == id), "{id}");
        }
        // Appended (taken in), removed (rebuilt), renamed and swapped in place (confirmed or
        // searched), a duplicate appended (the first still wins).
        for step in 0..6 {
            match step {
                0 => v.extend((200..300).map(|i| (format!("S{i}"), i))),
                1 => {
                    v.remove(10);
                }
                2 => v[20].0 = "renamed".into(),
                3 => v.swap(30, 40),
                4 => v.push(("S50".into(), 0)),
                _ => v.truncate(70),
            }
            for id in ["S0", "S11", "S10", "renamed", "S21", "S30", "S40", "S50", "S250", "S299", "nope"] {
                assert_eq!(find(&ix, &v, id), v.iter().position(|t| t.0 == id), "step {step}: {id}");
            }
        }
        // Short lists are searched.
        let short = items(10);
        assert_eq!(find(&IdIndex::default(), &short, "S3"), Some(3));
    }
}
