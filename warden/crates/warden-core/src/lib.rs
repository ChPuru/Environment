//! warden-core: encrypted, deduplicated, verifiable snapshot storage.
//!
//! Format and threat model: `docs/format-v0.md`, `docs/threat-model.md`.
//! Everything the storage backend holds is sealed client-side; every restore
//! self-verifies four ways (stored-name hash, AEAD tag, keyed chunk identity,
//! whole-file hash). Silent corruption is the failure mode this crate exists
//! to make impossible.

pub mod backend;
pub mod chunker;
pub mod crypto;
pub mod error;
pub mod health;
pub mod index;
pub mod lock;
pub mod object;
pub mod pack;
pub mod repo;
pub mod verify;

pub use error::{Error, Result};
