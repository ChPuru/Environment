//! Single-writer advisory lock, built on ONE backend primitive: atomic
//! `put_meta(overwrite=false)` — create_new-atomic on local disk
//! (hard_link), If-None-Match:* on S3. Same lock, both worlds.
//!
//! Design choices, stated:
//! - Every claim carries a random TOKEN; release deletes the lockfile only
//!   if it still holds OUR token. A stale claimant can never release its
//!   successor's lock.
//! - No RAII auto-release: a crashed process abandons its lock and EXPIRY
//!   is the recovery mechanism, by design. Steal happens only past expiry
//!   (garbage lockfiles count as expired).
//! - HONEST LIMIT: without compare-and-swap, two claimants racing an
//!   expired lock both delete-then-claim; exactly one claim wins (the
//!   atomic put decides), but a pathological resume of a long-sleeping
//!   original writer is not prevented. CAS via S3 conditional writes is
//!   the documented upgrade path. For the single-user CLI this protects
//!   prune (M7) from concurrent backups, which is its job.

use crate::backend::Backend;
use crate::error::{Error, Result};
use crate::object::{from_cbor, to_cbor};
use serde::{Deserialize, Serialize};

pub const LOCK_PATH: &str = "locks/writer";

#[derive(Serialize, Deserialize, Debug)]
pub struct LockRecord {
    pub pid: u32,
    pub token: u64,
    pub created_unix: u64,
    pub expires_unix: u64,
}

/// Proof of a held claim. Not Copy/Clone: releasing consumes it.
#[derive(Debug)]
pub struct LockClaim {
    token: u64,
}

fn read_holder<B: Backend>(b: &B) -> Result<Option<LockRecord>> {
    match b.get_meta(LOCK_PATH) {
        Ok(bytes) => Ok(Some(from_cbor(&bytes).unwrap_or(LockRecord {
            pid: 0,
            token: 0,
            created_unix: 0,
            expires_unix: 0, // garbage lockfile == expired
        }))),
        Err(Error::NotFound(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

fn try_claim<B: Backend>(b: &B, ttl_secs: u64, now_unix: u64) -> Result<Option<LockClaim>> {
    let mut tok = [0u8; 8];
    getrandom::getrandom(&mut tok).map_err(|_| Error::Crypto("rng failure"))?;
    let token = u64::from_le_bytes(tok);
    let rec = LockRecord {
        pid: std::process::id(),
        token,
        created_unix: now_unix,
        expires_unix: now_unix.saturating_add(ttl_secs),
    };
    match b.put_meta(LOCK_PATH, &to_cbor(&rec)?, false) {
        Ok(()) => Ok(Some(LockClaim { token })),
        Err(Error::AlreadyExists(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Peek at the current holder, if any: (pid, expires_unix). For operator
/// tooling; garbage lockfiles read as (0, 0).
pub fn read_holder_info<B: Backend>(b: &B) -> Result<Option<(u32, u64)>> {
    Ok(read_holder(b)?.map(|h| (h.pid, h.expires_unix)))
}

/// Claim the writer lock, stealing only an EXPIRED one.
pub fn acquire<B: Backend>(b: &B, ttl_secs: u64, now_unix: u64) -> Result<LockClaim> {
    if let Some(claim) = try_claim(b, ttl_secs, now_unix)? {
        return Ok(claim);
    }
    let holder = read_holder(b)?;
    if let Some(h) = &holder {
        if h.expires_unix > now_unix {
            return Err(Error::Locked {
                pid: h.pid,
                expires_unix: h.expires_unix,
            });
        }
    }
    // Expired (or vanished between attempts): one steal, then accept fate.
    b.delete_meta(LOCK_PATH)?;
    match try_claim(b, ttl_secs, now_unix)? {
        Some(claim) => Ok(claim),
        None => Err(Error::Locked {
            pid: 0,
            expires_unix: 0,
        }), // lost the race
    }
}

/// Token-checked release: deletes the lockfile only if it is still OURS.
/// Releasing after having been (legitimately) stolen is a silent no-op —
/// the successor's lock must survive us.
pub fn release<B: Backend>(b: &B, claim: LockClaim) -> Result<()> {
    match read_holder(b)? {
        Some(h) if h.token == claim.token => b.delete_meta(LOCK_PATH),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::LocalBackend;
    use tempfile::tempdir;

    #[test]
    fn second_claim_fails_while_held_then_succeeds_after_release() {
        let td = tempdir().unwrap();
        let be = LocalBackend::create(td.path()).unwrap();
        let c1 = acquire(&be, 3600, 1000).unwrap();
        match acquire(&be, 3600, 1001) {
            Err(Error::Locked { expires_unix, .. }) => assert_eq!(expires_unix, 4600),
            other => panic!("expected Locked, got {other:?}"),
        }
        release(&be, c1).unwrap();
        let c2 = acquire(&be, 3600, 1002).unwrap();
        release(&be, c2).unwrap();
    }

    #[test]
    fn expired_lock_is_stolen_and_stale_release_cannot_evict_successor() {
        let td = tempdir().unwrap();
        let be = LocalBackend::create(td.path()).unwrap();
        let stale = acquire(&be, 10, 1000).unwrap();
        assert!(matches!(acquire(&be, 10, 1005), Err(Error::Locked { .. })));
        let successor = acquire(&be, 3600, 1011).unwrap(); // expired -> stolen

        // The hazard this design exists to kill: the STALE claimant tries to
        // release AFTER being stolen from. Must be a no-op.
        release(&be, stale).unwrap();
        assert!(
            matches!(acquire(&be, 3600, 1012), Err(Error::Locked { .. })),
            "successor's lock must survive a stale release"
        );
        release(&be, successor).unwrap();
        let c = acquire(&be, 3600, 1013).unwrap();
        release(&be, c).unwrap();
    }

    #[test]
    fn garbage_lockfile_counts_as_expired() {
        let td = tempdir().unwrap();
        let be = LocalBackend::create(td.path()).unwrap();
        be.put_meta(LOCK_PATH, b"not cbor at all", false).unwrap();
        let c = acquire(&be, 3600, 1000).unwrap();
        release(&be, c).unwrap();
    }
}
