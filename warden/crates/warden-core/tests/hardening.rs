//! Parse hardening: the stable-toolchain substitute for cargo-fuzz (which
//! needs nightly; this environment is rustc 1.75 stable — stated in the
//! docs). Seeded and reproducible: every failure names its seed. The
//! promise under test is small and absolute: decoding attacker-controlled
//! bytes returns Err. It never panics, never hangs, never lies.

use std::panic::{catch_unwind, AssertUnwindSafe};
use warden_core::chunker::{chunk_reader, ChunkerParams};
use warden_core::object::{from_cbor, HealthRecord, IndexRecord, Snapshot, Tree};

const GOLDEN: &str = include_str!("golden/v0.json");

fn vector(name: &str) -> Vec<u8> {
    let key = format!("\"{name}\": \"");
    let start = GOLDEN
        .find(&key)
        .unwrap_or_else(|| panic!("{name} in golden"))
        + key.len();
    let hex = &GOLDEN[start..start + GOLDEN[start..].find('"').expect("close quote")];
    hex::decode(hex).expect("golden hex")
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() as usize) % n.max(1)
    }
}

/// Decode must be a total function into Result: no panic, ever.
fn assert_total<T: serde::de::DeserializeOwned>(bytes: &[u8], what: &str, seed: u64) {
    let r = catch_unwind(AssertUnwindSafe(|| {
        let _ = from_cbor::<T>(bytes);
    }));
    assert!(
        r.is_ok(),
        "decode of {what} PANICKED (seed {seed}, len {})",
        bytes.len()
    );
}

fn storm<T: serde::de::DeserializeOwned>(name: &str) {
    let valid = vector(name);
    // Truncation ladder: every prefix, including empty.
    for cut in 0..=valid.len() {
        assert_total::<T>(&valid[..cut], &format!("{name} truncated@{cut}"), 0);
    }
    // Seeded mutation storm: flips, splices, extensions.
    for seed in 1..=200u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15));
        let mut b = valid.clone();
        match rng.below(3) {
            0 => {
                // flip 1..4 bytes
                for _ in 0..=rng.below(3) {
                    let i = rng.below(b.len());
                    b[i] ^= (rng.next() as u8) | 1;
                }
            }
            1 => {
                // splice garbage into the middle
                let at = rng.below(b.len());
                let junk: Vec<u8> = (0..rng.below(32)).map(|_| rng.next() as u8).collect();
                b.splice(at..at, junk);
            }
            _ => {
                // truncate then extend with garbage
                b.truncate(rng.below(b.len()));
                for _ in 0..rng.below(64) {
                    b.push(rng.next() as u8);
                }
            }
        }
        assert_total::<T>(&b, name, seed);
    }
}

#[test]
fn every_record_type_survives_the_mutation_storm() {
    storm::<IndexRecord>("index_record_cbor");
    storm::<Tree>("tree_cbor");
    storm::<Snapshot>("snapshot_cbor");
    storm::<HealthRecord>("health_verify_cbor");
    storm::<HealthRecord>("health_drill_cbor");
    storm::<HealthRecord>("health_prune_cbor");
}

#[test]
fn random_byte_soup_never_panics_any_decoder() {
    for seed in 1..=200u64 {
        let mut rng = Rng(seed.wrapping_mul(0x243f_6a88_85a3_08d3));
        let len = rng.below(512);
        let soup: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
        assert_total::<IndexRecord>(&soup, "soup->IndexRecord", seed);
        assert_total::<Tree>(&soup, "soup->Tree", seed);
        assert_total::<Snapshot>(&soup, "soup->Snapshot", seed);
        assert_total::<HealthRecord>(&soup, "soup->HealthRecord", seed);
    }
}

// ---- chunker properties over random streams ----

fn chunks_of(data: &[u8], p: ChunkerParams) -> Vec<Vec<u8>> {
    chunk_reader(std::io::Cursor::new(data.to_vec()), p)
        .collect::<Result<Vec<_>, _>>()
        .expect("chunking in-memory data cannot fail")
}

#[test]
fn chunker_reassembles_bounds_and_repeats_itself() {
    let p = ChunkerParams::TINY_FOR_TESTS;
    for seed in 1..=60u64 {
        let mut rng = Rng(seed.wrapping_mul(0xdead_beef_cafe_f00d));
        let len = rng.below(64 * 1024);
        let data: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
        let chunks = chunks_of(&data, p);
        let glued: Vec<u8> = chunks.concat();
        assert_eq!(glued, data, "concat(chunks) != input (seed {seed})");
        for (i, c) in chunks.iter().enumerate() {
            assert!(c.len() <= p.max as usize, "chunk over max (seed {seed})");
            if i + 1 < chunks.len() {
                assert!(
                    c.len() >= p.min as usize,
                    "non-final chunk under min (seed {seed})"
                );
            }
        }
        assert_eq!(
            chunks,
            chunks_of(&data, p),
            "chunking must be deterministic"
        );
    }
}

/// The reason CDC exists: a one-byte insertion at the front must NOT move
/// every boundary. Soft property: at least half the boundaries survive
/// (shifted by one).
#[test]
fn content_defined_boundaries_survive_a_prefix_insertion() {
    let p = ChunkerParams::TINY_FOR_TESTS;
    let mut rng = Rng(0x5eed);
    let data: Vec<u8> = (0..256 * 1024).map(|_| rng.next() as u8).collect();
    let bounds = |chunks: &[Vec<u8>]| -> Vec<usize> {
        chunks
            .iter()
            .scan(0usize, |acc, c| {
                *acc += c.len();
                Some(*acc)
            })
            .collect()
    };
    let b1 = bounds(&chunks_of(&data, p));
    let mut shifted = vec![0x42u8];
    shifted.extend_from_slice(&data);
    let b2: std::collections::HashSet<usize> =
        bounds(&chunks_of(&shifted, p)).into_iter().collect();
    let survived = b1.iter().filter(|&&b| b2.contains(&(b + 1))).count();
    assert!(
        survived * 2 >= b1.len(),
        "CDC locality broken: {survived}/{} boundaries survived",
        b1.len()
    );
}
