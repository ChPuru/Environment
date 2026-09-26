//! Pack accumulation. A pack is nothing but the concatenation of sealed
//! blobs; there is no in-pack table of contents because the index IS the
//! locator. The pack's stored name is the BLAKE3 of its full bytes, so the
//! keyless-scrub property survives packing untouched.
//!
//! One PackWriter lives per backup (inside its context); nothing spans
//! backups. Pure: the caller owns backend puts and index insertion.

use crate::object::ChunkId;
use std::collections::HashSet;

pub struct PendingBlob {
    pub id: ChunkId,
    pub offset: u64,
    pub sealed_len: u64,
    pub plain_len: u64,
}

pub struct PackWriter {
    target: usize,
    buf: Vec<u8>,
    pending: Vec<PendingBlob>,
    ids: HashSet<ChunkId>,
}

impl PackWriter {
    pub fn new(target: usize) -> PackWriter {
        PackWriter {
            target,
            buf: Vec::new(),
            pending: Vec::new(),
            ids: HashSet::new(),
        }
    }

    /// Is this identity already staged (sealed, waiting for flush)?
    /// Dedup within a backup = index hit OR this.
    pub fn contains(&self, id: &ChunkId) -> bool {
        self.ids.contains(id)
    }

    pub fn add(&mut self, id: ChunkId, sealed: &[u8], plain_len: u64) {
        let offset = self.buf.len() as u64;
        self.buf.extend_from_slice(sealed);
        self.pending.push(PendingBlob {
            id,
            offset,
            sealed_len: sealed.len() as u64,
            plain_len,
        });
        self.ids.insert(id);
    }

    pub fn should_flush(&self) -> bool {
        self.buf.len() >= self.target
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Drain the accumulated pack: (pack bytes, blob locators relative to
    /// those bytes). The caller hashes the bytes to learn the pack's name.
    pub fn take(&mut self) -> (Vec<u8>, Vec<PendingBlob>) {
        self.ids.clear();
        (
            std::mem::take(&mut self.buf),
            std::mem::take(&mut self.pending),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::Hash256;

    fn id(b: u8) -> ChunkId {
        ChunkId(Hash256([b; 32]))
    }

    #[test]
    fn offsets_are_contiguous_and_bytes_concatenate() {
        let mut pw = PackWriter::new(1024);
        pw.add(id(1), b"aaaa", 10);
        pw.add(id(2), b"bbbbbb", 20);
        pw.add(id(3), b"cc", 30);
        let (bytes, blobs) = pw.take();
        assert_eq!(bytes, b"aaaabbbbbbcc");
        let expect = [(0u64, 4u64, 10u64), (4, 6, 20), (10, 2, 30)];
        for (b, (off, slen, plen)) in blobs.iter().zip(expect) {
            assert_eq!((b.offset, b.sealed_len, b.plain_len), (off, slen, plen));
        }
        assert_eq!(blobs.len(), 3);
    }

    #[test]
    fn contains_tracks_staged_until_take() {
        let mut pw = PackWriter::new(1024);
        assert!(!pw.contains(&id(1)));
        pw.add(id(1), b"x", 1);
        assert!(pw.contains(&id(1)));
        let _ = pw.take();
        assert!(!pw.contains(&id(1)), "take must reset dedup state");
        assert!(pw.is_empty());
    }

    #[test]
    fn should_flush_trips_at_target() {
        let mut pw = PackWriter::new(8);
        pw.add(id(1), b"1234", 4);
        assert!(!pw.should_flush());
        pw.add(id(2), b"5678", 4);
        assert!(pw.should_flush());
    }
}
