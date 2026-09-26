use super::{seal, AD_KEYFILE};
use crate::error::{Error, Result};
use crate::object::{ChunkId, Hash256};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

/// Root secret. Everything derives from it; loss is unrecoverable by design.
pub struct MasterKey(pub Zeroizing<[u8; 32]>);

impl MasterKey {
    pub fn generate() -> Result<Self> {
        let mut k = Zeroizing::new([0u8; 32]);
        getrandom::getrandom(k.as_mut()).map_err(|_| Error::Crypto("rng failure"))?;
        Ok(MasterKey(k))
    }
    /// For tests and golden vectors only in spirit; harmless generally.
    pub fn from_bytes(b: [u8; 32]) -> Self {
        MasterKey(Zeroizing::new(b))
    }
}

/// Purpose-separated subkeys via BLAKE3 derive_key (KDF mode, domain contexts
/// in format-v0.md). Compromise of one purpose never crosses into another,
/// and separate identity keys mean chunk ids and tree ids are disjoint
/// keyspaces by construction.
pub struct Keys {
    pub id_key: Zeroizing<[u8; 32]>,
    pub tree_key: Zeroizing<[u8; 32]>,
    pub data_key: Zeroizing<[u8; 32]>,
    pub meta_key: Zeroizing<[u8; 32]>,
}

impl Keys {
    pub fn derive(mk: &MasterKey) -> Keys {
        Keys {
            id_key: Zeroizing::new(blake3::derive_key("warden/v0/id", &mk.0[..])),
            tree_key: Zeroizing::new(blake3::derive_key("warden/v0/treeid", &mk.0[..])),
            data_key: Zeroizing::new(blake3::derive_key("warden/v0/data", &mk.0[..])),
            meta_key: Zeroizing::new(blake3::derive_key("warden/v0/meta", &mk.0[..])),
        }
    }

    /// Chunk identity: keyed BLAKE3 of the plaintext.
    pub fn chunk_id(&self, plaintext: &[u8]) -> ChunkId {
        ChunkId(Hash256(
            *blake3::keyed_hash(&self.id_key, plaintext).as_bytes(),
        ))
    }

    /// Tree identity: keyed BLAKE3 of the canonical tree encoding, under its
    /// own subkey. Identical directory contents dedup like chunks do.
    pub fn tree_id(&self, tree_cbor: &[u8]) -> ChunkId {
        ChunkId(Hash256(
            *blake3::keyed_hash(&self.tree_key, tree_cbor).as_bytes(),
        ))
    }
}

/// Argon2id cost parameters. Production defaults are deliberately expensive;
/// tests use MIN_FOR_TESTS so suites stay fast without touching prod paths.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct KdfParams {
    pub m_cost: u32,
    pub t_cost: u32,
    pub p_cost: u32,
}

impl KdfParams {
    /// 64 MiB, 3 passes — interactive-unlock grade.
    pub const DEFAULT: KdfParams = KdfParams {
        m_cost: 65536,
        t_cost: 3,
        p_cost: 1,
    };
    pub const MIN_FOR_TESTS: KdfParams = KdfParams {
        m_cost: 8,
        t_cost: 1,
        p_cost: 1,
    };
}

/// On-disk keyfile: plaintext envelope holding only sealed key material.
#[derive(Serialize, Deserialize, Debug)]
pub struct Keyfile {
    pub salt: [u8; 16],
    pub kdf: KdfParams,
    /// nonce||ct of the master key, AD = AD_KEYFILE, under the Argon2id KEK.
    pub sealed_master: Vec<u8>,
}

fn kek(passphrase: &str, salt: &[u8; 16], p: KdfParams) -> Result<Zeroizing<[u8; 32]>> {
    let params = argon2::Params::new(p.m_cost, p.t_cost, p.p_cost, Some(32))
        .map_err(|_| Error::Crypto("bad kdf params"))?;
    let a = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut out = Zeroizing::new([0u8; 32]);
    a.hash_password_into(passphrase.as_bytes(), salt, &mut out[..])
        .map_err(|_| Error::Crypto("kdf failure"))?;
    Ok(out)
}

pub fn wrap_master_key(mk: &MasterKey, passphrase: &str, kdf: KdfParams) -> Result<Keyfile> {
    let mut salt = [0u8; 16];
    getrandom::getrandom(&mut salt).map_err(|_| Error::Crypto("rng failure"))?;
    let kek = kek(passphrase, &salt, kdf)?;
    let sealed_master = seal::seal(&kek, AD_KEYFILE, &mk.0[..])?;
    Ok(Keyfile {
        salt,
        kdf,
        sealed_master,
    })
}

pub fn unwrap_master_key(kf: &Keyfile, passphrase: &str) -> Result<MasterKey> {
    let kek = kek(passphrase, &kf.salt, kf.kdf)?;
    // Any failure here is indistinguishable between wrong passphrase and a
    // damaged keyfile — the error says both, and the caller cannot tell more.
    let plain = Zeroizing::new(
        seal::open(&kek, AD_KEYFILE, &kf.sealed_master).map_err(|_| Error::BadPassphrase)?,
    );
    let arr: [u8; 32] = plain
        .as_slice()
        .try_into()
        .map_err(|_| Error::BadPassphrase)?;
    Ok(MasterKey(Zeroizing::new(arr)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyfile_wrap_unwrap_roundtrips() {
        let mk = MasterKey::from_bytes([3u8; 32]);
        let kf = wrap_master_key(&mk, "hunter2", KdfParams::MIN_FOR_TESTS).expect("wrap");
        let mk2 = unwrap_master_key(&kf, "hunter2").expect("unwrap");
        assert_eq!(*mk2.0, *mk.0);
    }

    #[test]
    fn wrong_passphrase_is_bad_passphrase_error() {
        let mk = MasterKey::from_bytes([3u8; 32]);
        let kf = wrap_master_key(&mk, "hunter2", KdfParams::MIN_FOR_TESTS).expect("wrap");
        match unwrap_master_key(&kf, "hunter3") {
            Err(Error::BadPassphrase) => {}
            Err(e) => panic!("expected BadPassphrase, got {e:?}"),
            Ok(_) => panic!("expected BadPassphrase, got Ok"),
        }
    }

    #[test]
    fn tampered_keyfile_is_rejected() {
        let mk = MasterKey::from_bytes([3u8; 32]);
        let mut kf = wrap_master_key(&mk, "hunter2", KdfParams::MIN_FOR_TESTS).expect("wrap");
        let last = kf.sealed_master.len() - 1;
        kf.sealed_master[last] ^= 0xff;
        assert!(unwrap_master_key(&kf, "hunter2").is_err());
    }

    #[test]
    fn subkeys_are_pairwise_distinct_and_deterministic() {
        let mk = MasterKey::from_bytes([5u8; 32]);
        let a = Keys::derive(&mk);
        let b = Keys::derive(&mk);
        assert_eq!(*a.id_key, *b.id_key);
        assert_eq!(*a.tree_key, *b.tree_key);
        let ks = [&a.id_key, &a.tree_key, &a.data_key, &a.meta_key];
        for i in 0..ks.len() {
            for j in (i + 1)..ks.len() {
                assert_ne!(**ks[i], **ks[j], "subkeys {i} and {j} collide");
            }
        }
    }

    #[test]
    fn chunk_id_is_keyed() {
        let a = Keys::derive(&MasterKey::from_bytes([5u8; 32]));
        let b = Keys::derive(&MasterKey::from_bytes([6u8; 32]));
        let p = b"same plaintext";
        assert_eq!(a.chunk_id(p), a.chunk_id(p), "must be deterministic");
        assert_ne!(a.chunk_id(p), b.chunk_id(p), "must depend on the key");
    }
}
