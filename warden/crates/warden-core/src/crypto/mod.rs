//! Sealing and key hierarchy. Standard constructions only — see
//! `docs/format-v0.md`; the composition gets external review before any 1.0.

pub mod keys;
pub mod seal;

pub use keys::{unwrap_master_key, wrap_master_key, KdfParams, Keyfile, Keys, MasterKey};
pub use seal::{open, seal, NONCE_LEN};

use crate::object::ChunkId;

/// Associated data binds every ciphertext to its role (and, for chunks, its
/// identity) so sealed blobs can never be swapped between roles undetected.
pub const AD_KEYFILE: &[u8] = b"warden/v0/keyfile";
pub const AD_SNAPSHOT: &[u8] = b"warden/v0/snapshot";
pub const AD_HEALTH: &[u8] = b"warden/v0/health";
pub const AD_INDEX: &[u8] = b"warden/v0/index";

pub fn chunk_ad(id: &ChunkId) -> Vec<u8> {
    let mut ad = Vec::with_capacity(15 + 32);
    ad.extend_from_slice(b"warden/v0/chunk");
    ad.extend_from_slice(&id.0 .0);
    ad
}

pub fn tree_ad(id: &ChunkId) -> Vec<u8> {
    let mut ad = Vec::with_capacity(14 + 32);
    ad.extend_from_slice(b"warden/v0/tree");
    ad.extend_from_slice(&id.0 .0);
    ad
}
