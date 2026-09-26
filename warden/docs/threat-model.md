# warden threat model (v0)

Adversary: the storage backend and anyone who obtains it — assumed curious AND
corrupting. Guarantees: (1) it never sees file contents, names, or structure;
(2) any modification/rot/truncation is detected — restore fails loudly, never
returns wrong bytes. Detected-not-prevented: deletion; rollback to an older
consistent state (freshness pin lands M8). Out of scope: a compromised client
(it holds plaintext), traffic analysis of sizes/counts/timing (honest leak list
in format-v0.md). Lost passphrase = unrecoverable data, by design. Crypto is
composed exclusively from audited primitives (XChaCha20-Poly1305, BLAKE3,
Argon2id); the composition itself requires external review before any 1.0.
