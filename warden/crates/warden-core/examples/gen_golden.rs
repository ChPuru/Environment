//! Generates the format-v0 golden vectors. Run once, commit the output as
//! `tests/golden/v0.json`; the `golden` test then locks the format: any
//! byte-level drift in key derivation, identities, or CBOR encoding becomes
//! a test failure instead of a silent compatibility break.

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

fn main() {
    let mk = MasterKey::from_bytes([7u8; 32]);
    let keys = Keys::derive(&mk);
    let chunk_id = keys.chunk_id(b"warden golden vector plaintext");

    let tree = Tree::new(vec![
        TreeEntry {
            name: NameBytes(b"caf\xE9.bin".to_vec()), // deliberately non-UTF-8
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
    let tree_cbor = to_cbor(&tree).expect("tree encode");
    let tree_id = keys.tree_id(&tree_cbor);

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
    let snapshot_cbor = to_cbor(&snap).expect("snapshot encode");

    println!("{{");
    println!("  \"master_key\": \"{}\",", hex::encode([7u8; 32]));
    println!("  \"id_key\": \"{}\",", hex::encode(&keys.id_key[..]));
    println!("  \"tree_key\": \"{}\",", hex::encode(&keys.tree_key[..]));
    println!("  \"data_key\": \"{}\",", hex::encode(&keys.data_key[..]));
    println!("  \"meta_key\": \"{}\",", hex::encode(&keys.meta_key[..]));
    println!(
        "  \"chunk_id_of_fixed_plaintext\": \"{}\",",
        chunk_id.0.to_hex()
    );
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
    println!(
        "  \"index_record_cbor\": \"{}\",",
        hex::encode(to_cbor(&index).expect("idx"))
    );
    let (hv, hd, hp) = golden_health_records();
    println!(
        "  \"health_verify_cbor\": \"{}\",",
        hex::encode(to_cbor(&hv).expect("hv"))
    );
    println!(
        "  \"health_drill_cbor\": \"{}\",",
        hex::encode(to_cbor(&hd).expect("hd"))
    );
    println!(
        "  \"health_prune_cbor\": \"{}\",",
        hex::encode(to_cbor(&hp).expect("hp"))
    );
    println!("  \"tree_cbor\": \"{}\",", hex::encode(&tree_cbor));
    println!("  \"tree_id\": \"{}\",", tree_id.0.to_hex());
    println!("  \"snapshot_cbor\": \"{}\"", hex::encode(snapshot_cbor));
    println!("}}");
}
