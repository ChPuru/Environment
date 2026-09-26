use crate::error::{Error, Result};

pub const NONCE_LEN: usize = 24;
const TAG_LEN: usize = 16;

/// Sealed form: `nonce[24] || XChaCha20-Poly1305 ciphertext(+tag)`.
/// A fresh random 24-byte nonce per seal is safe in XChaCha's nonce space.
/// `ad` binds the blob's role/identity; opening under a different `ad` fails.
pub fn seal(key: &[u8; 32], ad: &[u8], plaintext: &[u8]) -> Result<Vec<u8>> {
    use chacha20poly1305::aead::{Aead, KeyInit, Payload};
    use chacha20poly1305::{XChaCha20Poly1305, XNonce};

    let mut nonce = [0u8; NONCE_LEN];
    getrandom::getrandom(&mut nonce).map_err(|_| Error::Crypto("rng failure"))?;
    let cipher = XChaCha20Poly1305::new(key.into());
    let ct = cipher
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad: ad,
            },
        )
        .map_err(|_| Error::Crypto("seal failure"))?;
    let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Inverse of `seal`. Fails on any tamper, truncation, wrong key, or wrong AD.
pub fn open(key: &[u8; 32], ad: &[u8], sealed: &[u8]) -> Result<Vec<u8>> {
    use chacha20poly1305::aead::{Aead, KeyInit, Payload};
    use chacha20poly1305::{XChaCha20Poly1305, XNonce};

    if sealed.len() < NONCE_LEN + TAG_LEN {
        return Err(Error::Crypto("sealed blob too short"));
    }
    let (nonce, ct) = sealed.split_at(NONCE_LEN);
    let cipher = XChaCha20Poly1305::new(key.into());
    cipher
        .decrypt(XNonce::from_slice(nonce), Payload { msg: ct, aad: ad })
        .map_err(|_| Error::Crypto("authentication failed"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: [u8; 32] = [9u8; 32];
    const AD: &[u8] = b"test/ad";

    #[test]
    fn seal_open_roundtrips_including_empty() {
        for msg in [&b""[..], b"x", b"hello warden, this is a longer message"] {
            let sealed = seal(&KEY, AD, msg).expect("seal");
            assert!(sealed.len() >= NONCE_LEN + 16);
            assert_ne!(
                &sealed[NONCE_LEN..],
                msg,
                "ciphertext must not equal plaintext"
            );
            let back = open(&KEY, AD, &sealed).expect("open");
            assert_eq!(back, msg);
        }
    }

    #[test]
    fn every_probed_bitflip_is_rejected() {
        let sealed = seal(&KEY, AD, b"attack at dawn").expect("seal");
        // nonce region, first ct byte, last (tag) byte
        for pos in [0usize, NONCE_LEN, sealed.len() - 1] {
            let mut bad = sealed.clone();
            bad[pos] ^= 0x01;
            assert!(open(&KEY, AD, &bad).is_err(), "flip at {pos} accepted");
        }
    }

    #[test]
    fn wrong_ad_is_rejected() {
        let sealed = seal(&KEY, AD, b"payload").expect("seal");
        assert!(open(&KEY, b"other/ad", &sealed).is_err());
    }

    #[test]
    fn wrong_key_is_rejected() {
        let sealed = seal(&KEY, AD, b"payload").expect("seal");
        let other = [8u8; 32];
        assert!(open(&other, AD, &sealed).is_err());
    }

    #[test]
    fn truncation_is_rejected() {
        let sealed = seal(&KEY, AD, b"payload").expect("seal");
        for cut in [0usize, NONCE_LEN - 1, NONCE_LEN + 3, sealed.len() - 1] {
            assert!(
                open(&KEY, AD, &sealed[..cut]).is_err(),
                "cut at {cut} accepted"
            );
        }
    }

    #[test]
    fn nonces_are_fresh_per_seal() {
        let a = seal(&KEY, AD, b"same").expect("a");
        let b = seal(&KEY, AD, b"same").expect("b");
        assert_ne!(a[..NONCE_LEN], b[..NONCE_LEN], "nonce reuse");
        assert_ne!(a, b);
    }
}
