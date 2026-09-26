//! I4: format v0 golden vectors. If this fails, the on-disk format drifted —
//! that is a compatibility break, not a refactor detail. Regenerate only with
//! a deliberate, documented format change (`cargo run --example gen_golden`).

use warden_core::crypto::{Keys, MasterKey};
use warden_core::object::{
    to_cbor, Hash256, HealthEvent, HealthRecord, IndexEntry, IndexRecord, NameBytes, Node,
    Snapshot, StoredName, Tree, TreeEntry, HEALTH_VERSION, INDEX_VERSION,
};

fn golden_health_records() -> (HealthRecord, HealthRecord, HealthRecord) {
    (
        HealthRecord {
            version: HEALTH_VERSION,
            time_unix: 1_700_000_000,
            event: HealthEvent::Verify {
                deep: true,
                snapshots: 2,
                trees: 5,
                chunks: 9,
                orphans: 1,
            },
            problems_total: 1,
            problems_sample: vec!["bit rot: objects/aa".into()],
        },
        HealthRecord {
            version: HEALTH_VERSION,
            time_unix: 1_700_000_001,
            event: HealthEvent::Drill {
                snapshot: "deadbeef".into(),
                files: 3,
                bytes: 4096,
            },
            problems_total: 0,
            problems_sample: vec![],
        },
        HealthRecord {
            version: HEALTH_VERSION,
            time_unix: 1_700_000_002,
            event: HealthEvent::Prune {
                snapshots_remaining: 2,
                packs_deleted: 7,
                bytes_reclaimed: 65_536,
            },
            problems_total: 0,
            problems_sample: vec![],
        },
    )
}

const GOLDEN: &str = include_str!("golden/v0.json");

/// Naive `"key": "value"` extractor — the file is ours and machine-written.
fn field(key: &str) -> String {
    let pat = format!("\"{key}\": \"");
    let start = GOLDEN
        .find(&pat)
        .unwrap_or_else(|| panic!("golden missing {key}"))
        + pat.len();
    let rest = &GOLDEN[start..];
    let end = rest.find('"').expect("unterminated golden value");
    rest[..end].to_string()
}

#[test]
fn format_v0_vectors_have_not_drifted() {
    let mk = MasterKey::from_bytes([7u8; 32]);
    let keys = Keys::derive(&mk);
    assert_eq!(
        hex::encode(&keys.id_key[..]),
        field("id_key"),
        "id_key derivation drifted"
    );
    assert_eq!(
        hex::encode(&keys.tree_key[..]),
        field("tree_key"),
        "tree_key derivation drifted"
    );
    assert_eq!(
        hex::encode(&keys.data_key[..]),
        field("data_key"),
        "data_key derivation drifted"
    );
    assert_eq!(
        hex::encode(&keys.meta_key[..]),
        field("meta_key"),
        "meta_key derivation drifted"
    );

    let chunk_id = keys.chunk_id(b"warden golden vector plaintext");
    assert_eq!(
        chunk_id.0.to_hex(),
        field("chunk_id_of_fixed_plaintext"),
        "chunk identity drifted"
    );

    let tree = Tree::new(vec![
        TreeEntry {
            name: NameBytes(b"caf\xE9.bin".to_vec()),
            node: Node::File {
                mode: 0o100644,
                mtime: 1_600_000_000,
                len: 30,
                b3: Hash256([9u8; 32]),
                chunks: vec![chunk_id],
            },
        },
        TreeEntry {
            name: NameBytes(b"link".to_vec()),
            node: Node::Symlink {
                mtime: -5,
                target: NameBytes(b"caf\xE9.bin".to_vec()),
            },
        },
    ]);
    let index = IndexRecord {
        version: INDEX_VERSION,
        entries: vec![IndexEntry {
            chunk: chunk_id,
            pack: StoredName(Hash256([2u8; 32])),
            offset: 7,
            sealed_len: 99,
            len: 30,
        }],
    };
    assert_eq!(
        hex::encode(to_cbor(&index).expect("idx")),
        field("index_record_cbor"),
        "index record encoding drifted (I4)"
    );

    let (hv, hd, hp) = golden_health_records();
    assert_eq!(
        hex::encode(to_cbor(&hv).expect("hv")),
        field("health_verify_cbor"),
        "health record (Verify) encoding drifted (I4)"
    );
    assert_eq!(
        hex::encode(to_cbor(&hd).expect("hd")),
        field("health_drill_cbor"),
        "health record (Drill) encoding drifted (I4)"
    );
    assert_eq!(
        hex::encode(to_cbor(&hp).expect("hp")),
        field("health_prune_cbor"),
        "health record (Prune) encoding drifted (I4)"
    );

    let tree_cbor = to_cbor(&tree).expect("tree encode");
    assert_eq!(
        hex::encode(&tree_cbor),
        field("tree_cbor"),
        "tree encoding drifted (I4)"
    );
    let tree_id = keys.tree_id(&tree_cbor);
    assert_eq!(
        tree_id.0.to_hex(),
        field("tree_id"),
        "tree identity drifted"
    );

    let snap = Snapshot {
        version: 0,
        created_unix: 1_700_000_000,
        source_path: "/tmp/example".to_string(),
        root: Node::Dir {
            mode: 0o40755,
            mtime: 1_600_000_001,
            tree: tree_id,
        },
        total_len: 30,
        file_count: 1,
    };
    assert_eq!(
        hex::encode(to_cbor(&snap).expect("encode")),
        field("snapshot_cbor"),
        "canonical snapshot encoding drifted (I4)"
    );
}
