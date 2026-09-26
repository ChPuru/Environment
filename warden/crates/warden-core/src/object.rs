use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};

/// A 32-byte hash value; rendered lowercase hex everywhere user-visible.
/// Serialized as a CBOR byte string (manual impl below): compact and
/// deterministic, instead of serde's default 32-element integer array.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Hash256(pub [u8; 32]);

impl Serialize for Hash256 {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_bytes(&self.0)
    }
}

impl<'de> Deserialize<'de> for Hash256 {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = Hash256;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("exactly 32 bytes")
            }
            fn visit_bytes<E: serde::de::Error>(self, v: &[u8]) -> std::result::Result<Hash256, E> {
                let a: [u8; 32] = v
                    .try_into()
                    .map_err(|_| E::invalid_length(v.len(), &self))?;
                Ok(Hash256(a))
            }
            fn visit_byte_buf<E: serde::de::Error>(
                self,
                v: Vec<u8>,
            ) -> std::result::Result<Hash256, E> {
                self.visit_bytes(&v)
            }
        }
        d.deserialize_bytes(V)
    }
}

impl Hash256 {
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }
    pub fn from_hex(s: &str) -> Result<Self> {
        let v = hex::decode(s).map_err(|_| Error::Format(format!("bad hash hex: {s}")))?;
        let a: [u8; 32] = v
            .try_into()
            .map_err(|_| Error::Format(format!("bad hash length: {s}")))?;
        Ok(Hash256(a))
    }
}

/// Dedup identity of a plaintext chunk: BLAKE3 keyed_hash(id_key, plaintext).
/// Keyed so a repo observer cannot confirm possession of known files.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Debug)]
pub struct ChunkId(pub Hash256);

/// Name of a sealed blob at rest: plain BLAKE3 of the ciphertext bytes.
/// This is what makes keyless scrubbing possible.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Debug)]
pub struct StoredName(pub Hash256);

impl StoredName {
    pub fn of(sealed: &[u8]) -> Self {
        StoredName(Hash256(*blake3::hash(sealed).as_bytes()))
    }
}

pub const SNAPSHOT_VERSION: u32 = 0;
pub const TREE_VERSION: u32 = 0;
pub const INDEX_VERSION: u32 = 1;

/// Per-repo payload compression, fixed at init. Applied to chunk and tree
/// plaintext BEFORE sealing; identities are over the UNCOMPRESSED bytes, so
/// the dedup universe is unaffected by this choice.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compression {
    None,
    Zstd,
}

/// Filename bytes. Unix filenames are arbitrary bytes, not UTF-8; encoding
/// them as CBOR bytes preserves every legal name exactly.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct NameBytes(pub Vec<u8>);

impl NameBytes {
    pub fn display_lossy(&self) -> String {
        String::from_utf8_lossy(&self.0).into_owned()
    }
}

impl Serialize for NameBytes {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_bytes(&self.0)
    }
}

impl<'de> Deserialize<'de> for NameBytes {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl<'de> serde::de::Visitor<'de> for V {
            type Value = NameBytes;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("byte string")
            }
            fn visit_bytes<E: serde::de::Error>(
                self,
                v: &[u8],
            ) -> std::result::Result<NameBytes, E> {
                Ok(NameBytes(v.to_vec()))
            }
            fn visit_byte_buf<E: serde::de::Error>(
                self,
                v: Vec<u8>,
            ) -> std::result::Result<NameBytes, E> {
                Ok(NameBytes(v))
            }
        }
        d.deserialize_bytes(V)
    }
}

/// One filesystem node. `mode` is the unix permission+type bits; `mtime` is
/// seconds since the epoch (may be negative). Symlink mtime is stored but not
/// restored (std has no lutimens); symlink modes are meaningless on Linux.
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub enum Node {
    File {
        mode: u32,
        mtime: i64,
        len: u64,
        b3: Hash256,
        chunks: Vec<ChunkId>,
    },
    Dir {
        mode: u32,
        mtime: i64,
        tree: ChunkId,
    },
    Symlink {
        mtime: i64,
        target: NameBytes,
    },
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct TreeEntry {
    pub name: NameBytes,
    pub node: Node,
}

/// Directory contents. Entries MUST be sorted by name bytes — `Tree::new`
/// enforces it — so identical directories encode to identical bytes and
/// dedup as tree objects (I4 extended to structure).
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct Tree {
    pub version: u32,
    pub entries: Vec<TreeEntry>,
}

impl Tree {
    pub fn new(mut entries: Vec<TreeEntry>) -> Tree {
        entries.sort_by(|a, b| a.name.0.cmp(&b.name.0));
        Tree {
            version: TREE_VERSION,
            entries,
        }
    }
}

/// A snapshot commits one root node — a directory tree or a single file.
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub version: u32,
    pub created_unix: u64,
    pub source_path: String,
    pub root: Node,
    pub total_len: u64,
    pub file_count: u64,
}

/// Locator for one sealed blob inside a pack file (index record v1).
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct IndexEntry {
    pub chunk: ChunkId,
    pub pack: StoredName,
    pub offset: u64,
    pub sealed_len: u64,
    /// Plaintext (uncompressed) length.
    pub len: u64,
}

/// Indexes are additive deltas; readers merge all of them.
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct IndexRecord {
    pub version: u32,
    pub entries: Vec<IndexEntry>,
}

/// CBOR with deterministic bytes: field order is fixed by struct definition
/// (no maps with runtime ordering anywhere in the format). The golden-vector
/// test locks the exact bytes so drift is a test failure, not a surprise.
pub fn to_cbor<T: Serialize>(v: &T) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    ciborium::into_writer(v, &mut out).map_err(|e| Error::Format(format!("cbor encode: {e}")))?;
    Ok(out)
}

pub fn from_cbor<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T> {
    ciborium::from_reader(bytes).map_err(|e| Error::Format(format!("cbor decode: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_snapshot() -> Snapshot {
        Snapshot {
            version: SNAPSHOT_VERSION,
            created_unix: 1_700_000_000,
            source_path: "/tmp/x".to_string(),
            root: Node::File {
                mode: 0o644,
                mtime: 1_600_000_000,
                len: 3,
                b3: Hash256([7u8; 32]),
                chunks: vec![ChunkId(Hash256([1u8; 32])), ChunkId(Hash256([2u8; 32]))],
            },
            total_len: 3,
            file_count: 1,
        }
    }

    #[test]
    fn snapshot_cbor_roundtrips() {
        let s = sample_snapshot();
        let bytes = to_cbor(&s).expect("encode");
        let back: Snapshot = from_cbor(&bytes).expect("decode");
        assert_eq!(back, s);
    }

    #[test]
    fn encoding_is_deterministic_across_constructions() {
        // Two independently built, logically equal values must byte-match:
        // content addressing of metadata depends on it (I4).
        let a = to_cbor(&sample_snapshot()).expect("a");
        let b = to_cbor(&sample_snapshot()).expect("b");
        assert_eq!(a, b);
    }

    #[test]
    fn hashes_encode_as_compact_byte_strings() {
        // CBOR bytes: 2-byte header + 32 bytes. The derived-array encoding
        // this replaced cost ~66 bytes; regression here doubles metadata.
        let bytes = to_cbor(&Hash256([0xffu8; 32])).expect("encode");
        assert_eq!(bytes.len(), 34, "Hash256 must serialize as a byte string");
        let back: Hash256 = from_cbor(&bytes).expect("decode");
        assert_eq!(back, Hash256([0xffu8; 32]));
    }

    #[test]
    fn wrong_length_hash_bytes_are_rejected() {
        let bytes = to_cbor(&NameBytes(vec![1u8; 31])).expect("encode 31 bytes");
        assert!(
            from_cbor::<Hash256>(&bytes).is_err(),
            "31 bytes must not decode"
        );
    }

    #[test]
    fn non_utf8_names_roundtrip_exactly() {
        let n = NameBytes(vec![0x63, 0x61, 0x66, 0xE9]); // "caf\xE9", invalid UTF-8
        let back: NameBytes = from_cbor(&to_cbor(&n).expect("enc")).expect("dec");
        assert_eq!(back, n);
    }

    #[test]
    fn tree_new_sorts_entries_by_name_bytes() {
        let e = |name: &[u8]| TreeEntry {
            name: NameBytes(name.to_vec()),
            node: Node::Symlink {
                mtime: 0,
                target: NameBytes(b"t".to_vec()),
            },
        };
        let t1 = Tree::new(vec![e(b"zz"), e(b"aa"), e(b"\xff"), e(b"mm")]);
        let t2 = Tree::new(vec![e(b"mm"), e(b"\xff"), e(b"aa"), e(b"zz")]);
        let names: Vec<&[u8]> = t1.entries.iter().map(|x| x.name.0.as_slice()).collect();
        assert_eq!(names, vec![&b"aa"[..], b"mm", b"zz", b"\xff"]);
        assert_eq!(
            to_cbor(&t1).expect("t1"),
            to_cbor(&t2).expect("t2"),
            "same set of entries must encode identically regardless of input order"
        );
    }

    #[test]
    fn index_record_cbor_roundtrips() {
        let r = IndexRecord {
            version: INDEX_VERSION,
            entries: vec![IndexEntry {
                chunk: ChunkId(Hash256([3u8; 32])),
                pack: StoredName(Hash256([4u8; 32])),
                offset: 17,
                sealed_len: 1234,
                len: 999,
            }],
        };
        let back: IndexRecord = from_cbor(&to_cbor(&r).expect("enc")).expect("dec");
        assert_eq!(back, r);
    }

    #[test]
    fn hex_roundtrips_and_rejects_garbage() {
        let h = Hash256([0xab; 32]);
        assert_eq!(Hash256::from_hex(&h.to_hex()).expect("hex"), h);
        assert!(Hash256::from_hex("zz").is_err());
        assert!(Hash256::from_hex("abcd").is_err()); // wrong length
    }

    #[test]
    fn truncated_cbor_is_rejected() {
        let bytes = to_cbor(&sample_snapshot()).expect("enc");
        let cut = &bytes[..bytes.len() / 2];
        assert!(from_cbor::<Snapshot>(cut).is_err());
    }
}

pub const HEALTH_VERSION: u32 = 0;

/// One entry in the append-only health log: what verification-flavored work
/// ran, when, and what it found. Sealed under the meta key; the log is the
/// repo's receipts for "this data has been PROVEN restorable, recently".
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub struct HealthRecord {
    pub version: u32,
    pub time_unix: u64,
    pub event: HealthEvent,
    pub problems_total: u64,
    /// Up to 8 rendered problem strings; the full set is re-derivable by
    /// re-running the operation.
    pub problems_sample: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
pub enum HealthEvent {
    Verify {
        deep: bool,
        snapshots: u64,
        trees: u64,
        chunks: u64,
        orphans: u64,
    },
    ReadBack {
        sampled_chunks: u64,
        sampled_trees: u64,
        of_chunks: u64,
        of_trees: u64,
    },
    Drill {
        snapshot: String,
        files: u64,
        bytes: u64,
    },
    Prune {
        snapshots_remaining: u64,
        packs_deleted: u64,
        bytes_reclaimed: u64,
    },
}
