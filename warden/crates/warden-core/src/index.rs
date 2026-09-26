use crate::object::{ChunkId, IndexEntry, IndexRecord, StoredName};
use std::collections::HashMap;

/// Where a blob lives: which pack, at what offset, how long sealed, and how
/// long its uncompressed plaintext is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Loc {
    pub pack: StoredName,
    pub offset: u64,
    pub sealed_len: u64,
    pub len: u64,
}

impl Loc {
    fn of(e: &IndexEntry) -> Loc {
        Loc {
            pack: e.pack,
            offset: e.offset,
            sealed_len: e.sealed_len,
            len: e.len,
        }
    }
}

/// In-memory merge of all index deltas: content identity -> locator. First
/// writer wins on duplicates — duplicate ids refer to identical content by
/// construction, so this is a tie-break, not a choice.
#[derive(Default)]
pub struct MemIndex {
    map: HashMap<ChunkId, Loc>,
}

impl MemIndex {
    pub fn merge_record(&mut self, rec: IndexRecord) {
        for e in rec.entries {
            self.map.entry(e.chunk).or_insert(Loc::of(&e));
        }
    }
    pub fn insert(&mut self, e: IndexEntry) {
        self.map.entry(e.chunk).or_insert(Loc::of(&e));
    }
    pub fn get(&self, id: &ChunkId) -> Option<&Loc> {
        self.map.get(id)
    }
    pub fn contains(&self, id: &ChunkId) -> bool {
        self.map.contains_key(id)
    }
    pub fn len(&self) -> usize {
        self.map.len()
    }
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::Hash256;

    fn id(b: u8) -> ChunkId {
        ChunkId(Hash256([b; 32]))
    }
    fn name(b: u8) -> StoredName {
        StoredName(Hash256([b; 32]))
    }
    fn entry(b: u8, p: u8, offset: u64, sealed_len: u64, len: u64) -> IndexEntry {
        IndexEntry {
            chunk: id(b),
            pack: name(p),
            offset,
            sealed_len,
            len,
        }
    }
    fn loc(p: u8, offset: u64, sealed_len: u64, len: u64) -> Loc {
        Loc {
            pack: name(p),
            offset,
            sealed_len,
            len,
        }
    }

    #[test]
    fn merging_two_deltas_unions_entries() {
        let mut ix = MemIndex::default();
        ix.merge_record(IndexRecord {
            version: 1,
            entries: vec![entry(1, 10, 0, 50, 100)],
        });
        ix.merge_record(IndexRecord {
            version: 1,
            entries: vec![entry(2, 20, 5, 60, 200)],
        });
        assert_eq!(ix.len(), 2);
        assert_eq!(ix.get(&id(1)), Some(&loc(10, 0, 50, 100)));
        assert_eq!(ix.get(&id(2)), Some(&loc(20, 5, 60, 200)));
    }

    #[test]
    fn duplicate_chunk_id_first_writer_wins() {
        let mut ix = MemIndex::default();
        ix.insert(entry(1, 10, 0, 50, 100));
        ix.insert(entry(1, 99, 9, 99, 999));
        assert_eq!(ix.len(), 1);
        assert_eq!(ix.get(&id(1)), Some(&loc(10, 0, 50, 100)));
    }

    #[test]
    fn contains_and_empty_behave() {
        let mut ix = MemIndex::default();
        assert!(ix.is_empty());
        assert!(!ix.contains(&id(1)));
        ix.insert(entry(1, 10, 0, 5, 1));
        assert!(ix.contains(&id(1)));
        assert!(!ix.is_empty());
    }
}
