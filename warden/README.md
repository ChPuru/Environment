# warden

Encrypted, deduplicated, **provably restorable** snapshots. The differentiator
is trust: content-defined dedup, client-side sealing (storage never sees
plaintext), and verification as the product's spine — restore self-checks four
ways today; continuous scrubbing, read-back-after-write, and restore drills
land in M5.

**Status:** feature complete through milestone M8; on-disk format frozen as a
v1 candidate (see `docs/format-v0.md`). All seven design invariants are under
test. It has not had an external security review — keep independent backups
of anything that matters.

```
export WARDEN_PASSPHRASE='...'
warden --repo /path/repo init
warden --repo /path/repo backup ./file.bin     # prints snapshot id
warden --repo /path/repo list
warden --repo /path/repo restore <id> ./restored.bin
warden --repo /path/repo scrub            # keyless: no passphrase needed
warden --repo /path/repo verify --deep    # keyed, fetches everything, records
warden --repo /path/repo drill            # restore-drill random files, record
warden --repo /path/repo health           # judge freshness; exit 1 if stale
```

### Stability

The repository format is frozen (see docs/format-v0.md): future versions
will read today's repositories. Parse hardening runs as seeded property
tests on stable Rust — cargo-fuzz needs nightly, which the development
environment does not have; the substitution and its seeds are in
`tests/hardening.rs`. Rollback detection is client-side by necessity and
its limits are documented, not hidden. License: MIT.

### Reclaiming space

```
warden --repo /path/repo forget --keep-last 30   # or: forget <snapshot-id>
warden --repo /path/repo prune --dry-run         # the plan, no deletions
warden --repo /path/repo prune                   # reclaim; receipt recorded
```

Prune refuses to run on a repo that is missing live data, holds the writer
lock throughout, and records what it reclaimed in the health log. It works
at pack granularity in v0 (a pack with one live chunk survives whole).

### S3 repositories

```
export WARDEN_S3_ACCESS_KEY=... WARDEN_S3_SECRET_KEY=...
export WARDEN_S3_ENDPOINT=http://minio.local:9000   # omit for AWS
warden --repo s3://bucket/some/prefix init          # bucket must exist
warden --repo s3://bucket/some/prefix backup ~/data
```

Same commands, same format, same proofs. The store must support S3
conditional writes (If-None-Match) — AWS S3 and MinIO both do; it is what
makes init and the writer lock atomic. `warden unlock` (keyless) clears a
crashed writer's lock; expired locks are stolen automatically. Development
is tested against a signature-verifying in-process mock plus a
backend conformance suite; integration against real S3/MinIO is a
user-side step by container necessity, and bug reports from it are gold.

Put `verify --deep` and `drill` in cron, and `health` in monitoring: the
repo then carries sealed receipts that its data has been PROVEN restorable,
recently — not assumed restorable, proven.

### Design invariants (all under test)

- **I1** `restore(backup(x)) == x`, byte for byte.
- **I2** chunk concatenation identity, size bounds, and edit locality.
- **I3** no silent corruption, ever.
- **I4** deterministic encoding (golden vectors in `crates/warden-core/tests/golden/`).
- **I5** crash safety: the snapshot write is the commit point (SIGKILL harness).
- **I6** prune preserves restores (fault injection at every mutation).
- **I7** the backend never receives plaintext content or names (canary-tested).

Key decisions: FastCDC chunking; keyed-BLAKE3 chunk ids; XChaCha20-Poly1305
with role/identity associated data; ciphertext-hash object names (keyless
scrub); fixed-order CBOR structs; sync core, no async runtime.

### Build and test

```
cargo build --release -p warden-cli     # binary: target/release/warden
cargo test --workspace
cargo clippy --all-targets -- -D warnings
```

Regenerate golden vectors only deliberately, then diff them:
`cargo run -p warden-core --example gen_golden`.

### Docs

- `docs/format-v0.md` — frozen on-disk format and crypto
- `docs/threat-model.md` — what warden defends against, and what it does not
- `docs/bench-m4.md`, `docs/bench-m8.md` — measured performance
- `CHANGELOG.md` — milestone history (M1 slice → M8 hardening/freeze)

License: MIT (see `LICENSE`).
