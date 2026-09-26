//! Verification tiers. The product's spine: trust is demonstrated, not
//! assumed. Two tiers exist because they need different credentials:
//!
//! - **Scrub** (keyless): every stored file's name is the BLAKE3 of its
//!   bytes, so anyone — a cron job on the storage box, a third party — can
//!   detect bit rot by rehashing, holding zero secrets.
//! - **Verify** (keyed): walks snapshot → tree → chunk reachability and
//!   authenticates everything a restore would depend on. `deep` fetches and
//!   fully checks every reachable object; fast mode checks structure and
//!   existence only.
//!
//! Orphan objects (present but unreachable) are informational, not problems:
//! a crashed backup legitimately leaves them, and prune (M7) collects them.

use crate::backend::{Backend, Kind};
use crate::error::Result;
use crate::object::StoredName;
use std::fmt;

/// A concrete, attributable defect. Every variant names the object so a
/// human can go look at it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// Stored bytes no longer hash to the stored name: bit rot at rest.
    NameMismatch { kind: Kind, name: String },
    /// The backend could not produce the bytes at all.
    Unreadable { kind: Kind, name: String },
    /// AEAD authentication failed: tampering, wrong role, or corruption.
    AuthFailed { kind: Kind, name: String },
    /// Decrypted fine but the keyed identity does not match its reference.
    IdentityMismatch { name: String },
    /// A record decrypted but did not decode as its type.
    Undecodable { kind: Kind, name: String },
    /// A snapshot references a chunk the index/store cannot produce.
    MissingChunk { chunk: String, snapshot: String },
    /// A snapshot references a tree the index/store cannot produce.
    MissingTree { tree: String, snapshot: String },
    /// The snapshot record itself is unusable.
    SnapshotBad { id: String, reason: String },
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Problem::NameMismatch { kind, name } => {
                write!(f, "bit rot: {}/{name} does not match its name", kind.dir())
            }
            Problem::Unreadable { kind, name } => {
                write!(f, "unreadable: {}/{name}", kind.dir())
            }
            Problem::AuthFailed { kind, name } => {
                write!(f, "authentication failed: {}/{name}", kind.dir())
            }
            Problem::IdentityMismatch { name } => {
                write!(f, "identity mismatch: objects/{name}")
            }
            Problem::Undecodable { kind, name } => {
                write!(f, "undecodable record: {}/{name}", kind.dir())
            }
            Problem::MissingChunk { chunk, snapshot } => {
                write!(f, "missing chunk {chunk} referenced by snapshot {snapshot}")
            }
            Problem::MissingTree { tree, snapshot } => {
                write!(f, "missing tree {tree} referenced by snapshot {snapshot}")
            }
            Problem::SnapshotBad { id, reason } => write!(f, "snapshot {id} bad: {reason}"),
        }
    }
}

/// Keyless scrub result over every stored file.
#[derive(Debug, Default)]
pub struct ScrubReport {
    pub checked: u64,
    pub problems: Vec<Problem>,
}

/// Keyed verification result over everything snapshots depend on.
#[derive(Debug, Default)]
pub struct VerifyReport {
    pub snapshots_checked: u64,
    pub trees_checked: u64,
    pub chunks_checked: u64,
    /// Objects present in the store but reachable from no snapshot.
    /// Informational: crash leftovers live here until prune (M7).
    pub orphan_objects: u64,
    pub problems: Vec<Problem>,
}

/// Rehash every stored file against its content-derived name. Requires no
/// keys. Per-object failures are recorded, never aborted on: a scrub's job
/// is a complete damage report.
pub fn scrub<B: Backend>(backend: &B) -> Result<ScrubReport> {
    let mut rep = ScrubReport::default();
    for kind in [Kind::Object, Kind::Index, Kind::Snapshot, Kind::Health] {
        for name in backend.list(kind)? {
            rep.checked += 1;
            match backend.get(kind, &name) {
                Err(_) => {
                    rep.problems.push(Problem::Unreadable {
                        kind,
                        name: name.0.to_hex(),
                    });
                }
                Ok(bytes) => {
                    if !name_matches(&bytes, &name) {
                        rep.problems.push(Problem::NameMismatch {
                            kind,
                            name: name.0.to_hex(),
                        });
                    }
                }
            }
        }
    }
    Ok(rep)
}

pub(crate) fn name_matches(bytes: &[u8], name: &StoredName) -> bool {
    StoredName::of(bytes) == *name
}
