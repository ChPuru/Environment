# warden repository format v0 — FROZEN

As of v0.8.0-m8 this format is FROZEN as the v1 candidate: fields may be
ADDED to CBOR records (decoders tolerate unknown fields) and new Kinds
may appear, but existing bytes, layouts, identities, and ADs will not
change. Repositories written by this version will open in all future
versions.

Everything the storage backend holds is either sealed (encrypted+authenticated) or
content-free plumbing. The backend never sees file contents, file names, or tree
structure. All multi-byte integers inside CBOR follow CBOR rules; all hashes are
32 bytes, rendered lowercase hex.

## Layout (loose-object era; packs arrive in M4)

```
repo/
  config              plaintext CBOR: {version:0, repo_id:[u8;16], chunker:{min,avg,max}, compression, pack_target}
  keys/<hex16>        plaintext CBOR keyfile (see Keys) — contains only sealed key material
  objects/<aa>/<hex>  PACK files: raw concatenation of sealed blobs (chunks
                      and trees); filename = BLAKE3(whole pack bytes). No
                      in-pack TOC — the index is the locator. Packs flush at
                      pack_target and at end of backup, before the index.
  indexes/<hex>       sealed index records v1 (chunk id -> pack, offset,
                      sealed_len, plaintext len); filename = BLAKE3(sealed)
  snapshots/<hex>     sealed snapshots;     filename = BLAKE3(sealed bytes)
  health/<hex>        sealed health records (append-only receipts of every
                      verify / read-back / drill); filename = BLAKE3(sealed)
  tmp/                staging for atomic writes (tempfile + rename)
```

Naming stored files by the plain BLAKE3 of their *ciphertext* is what enables
keyless scrubbing: any party, holding zero keys, can detect bit rot by rehashing
files against their names (M3 exposes this as `verify`; M1's restore path already
performs it inline).

## Keys

- `master_key`: 32 random bytes.
- Keyfile: `{salt:[u8;16], m_cost, t_cost, p_cost, nonce:[u8;24], sealed_master:bytes}`.
  KEK = Argon2id(passphrase, salt, params) → 32 bytes.
  `sealed_master` = XChaCha20-Poly1305(KEK, nonce, AD = "warden/v0/keyfile", master_key).
- Subkeys via BLAKE3 `derive_key` (domain-separated KDF mode):
  - `id_key`   = derive_key("warden/v0/id",   master_key) — keyed chunk identity
  - `data_key` = derive_key("warden/v0/data", master_key) — chunk sealing
  - `meta_key` = derive_key("warden/v0/meta", master_key) — snapshot/index sealing

## Chunk identity and sealing

- Chunk boundaries: FastCDC (v2020), params from config. Defaults 512Ki/1Mi/8Mi.
- `chunk_id` = BLAKE3 keyed_hash(id_key, plaintext_chunk). Keyed, so possession of
  the repo does not let anyone confirm you hold a known file (confirmation-of-file
  attack); consequence: dedup is per-repository, by design.
- Sealed form of any blob: `nonce[24] || XChaCha20-Poly1305(key, nonce, AD, plaintext)`.
  Nonces are 24 random bytes per seal (XChaCha's nonce space makes this safe).
- Associated data binds ciphertexts to their role so they cannot be swapped:
  - chunks:    AD = "warden/v0/chunk" || chunk_id      (sealed with data_key)
  - snapshots: AD = "warden/v0/snapshot"               (sealed with meta_key)
  - indexes:   AD = "warden/v0/index"                  (sealed with meta_key)

## Records (CBOR structs, fixed field order = deterministic bytes; locked by golden test)

- Snapshot: `{version:0, created_unix:u64, source_path:str, file_len:u64,
              file_b3:[u8;32] (plain BLAKE3 of whole plaintext), chunks:[chunk_id...]}`
  M1 scope: exactly one file per snapshot. M2 replaces `source_path/file_*/chunks`
  with a root tree id.
- IndexRecord: `{version:0, entries:[{chunk:chunk_id, stored:stored_name, len:u64}...]}`
  Indexes are additive deltas; a reader merges all of them.

## Commit protocol

Order: chunk objects → index delta → snapshot (the atomic commit point). A crash
before the snapshot write orphans garbage but never damages committed snapshots.
Local backend writes are tempfile-in-repo + fsync + rename; content-addressed puts
are idempotent (name exists ⇒ Ok, never overwrite).

## Restore verification (all four, every restore)

1. plain BLAKE3(sealed bytes) == stored filename          (bit rot at rest)
2. AEAD tag verifies under the role AD                    (tamper / swap)
3. keyed_hash(id_key, plaintext) == chunk_id              (defense in depth)
4. whole-file BLAKE3 == snapshot.file_b3 after assembly   (assembly bugs)
Any failure aborts loudly; destination is written via temp + rename, so a failed
restore never leaves a partial or wrong file at the destination path.

## Compression

Chosen once at init (`compression: none | zstd`), applied to chunk and tree
plaintext BEFORE sealing. All identities (chunk id, tree id, whole-file
hash) are over the UNCOMPRESSED bytes, so the dedup universe and the golden
vectors are independent of the compression choice. Sealed payloads differ;
nothing else does. Plaintext length is carried in the index entry and
checked on every fetch.

Per-blob reads are ranged (offset + sealed_len within the pack). A short
range is a LOUD Corrupt error, never partial data. The per-blob stored-name
check of the loose-object era is superseded by AEAD (strictly stronger);
whole-PACK integrity remains covered keylessly by scrub, and deep verify
hashes every pack it touches once.

## Verification tiers

- **Scrub (keyless):** every stored file is named by the BLAKE3 of its bytes,
  so any party — including the storage host — can detect rot by rehashing,
  holding zero secrets. Exit contract: nonzero when problems exist.
- **Verify (keyed):** audits every snapshot and index record (name, AEAD,
  decode) and walks snapshot -> tree -> chunk reachability. Fast mode checks
  structure + existence; `--deep` re-fetches and re-checks every reachable
  object. Orphan objects (present, unreachable) are reported as a count, not
  problems: crashed backups legitimately leave them until prune (M7).
- Index deltas load *tolerantly* at open: a rotten delta is skipped (affected
  restores fail loudly via missing references) and named by `verify`. Opening
  must never be blocked by the damage it exists to report.
- Local writes fsync the file AND the parent directory after rename: the
  crash tests (deterministic every-prefix fault sweep + real SIGKILL harness)
  hold the commit protocol to "committed stays perfect; in-flight is absent
  or complete".

## Blob compression frame

Every pack-staged blob's plaintext (chunks and trees) is framed as
[tag u8][payload] BEFORE sealing: tag 0 = raw, tag 1 = zstd. The tag is
present in every repository regardless of the configured compression —
one decode path, no config-dependent parsing. Writers using Zstd sniff:
a 4 KiB sample that will not yield 2% is stored raw without compressing
the rest, and full compression that fails to win outright falls back to
raw (bench: docs/bench-m8.md). Metadata records (snapshots, index deltas,
health) are neither compressed nor framed. Unknown tags are a loud decode
error.

## Freshness pin (rollback detection)

A pin cannot live inside the repository — it would roll back with it. The
CLI keeps a per-repo-id file OUTSIDE the repo (XDG cache;
WARDEN_PIN_DIR overrides) recording the newest (created_unix, snapshot_id)
this client has seen. `verify` fails loudly when the repository's newest
snapshot is older than the pin ("possible ROLLBACK"), and a violated pin
never self-advances. Backups and `forget` on the same client re-anchor
it. Known limit, stated: a forget from ANOTHER client false-alarms until
this client backs up or removes the pin file; and a rolled-back repo plus
a wiped pin is undetectable — the pin is client-side trust-on-first-use,
not a consensus protocol.

## Space reclamation (forget + prune)

`forget` deletes snapshot objects (by id or keep-last policy) and nothing
else. `prune` is a PACK-GRANULARITY mark-and-sweep: walk the remaining
snapshots (full tree ladder), collect live index entries and live packs,
then — in this order — (1) write ONE compacted index delta of exactly the
live entries, (2) delete the old deltas, (3) delete dead packs, (4) record
a Prune health receipt. Every crash cut-point in that order leaves the
repo deep-verify green: duplicate live entries carry identical Locs (no
repacking in v0, so Locs never move), and dead packs are unreferenced by
the compacted index before they start disappearing. A fault-injection
sweep failing each mutation in turn proves it. Prune REFUSES to run when
reachable data is missing (from the index or from storage): pruning a
damaged repo would turn damage into loss. A missing DEAD pack does not
refuse — recovery-by-prune after a partial crash depends on that.
Limitation, stated: a pack with a single live chunk survives whole;
repacking is future work.

## S3 layout

An S3 repository uses the SAME key layout as a local one — `config`,
`keys/<id>`, `objects/aa/<hex>`, `indexes/<hex>`, `snapshots/<hex>`,
`health/<hex>`, `locks/writer` — under an optional key prefix, addressed
path-style. Every request is SigV4-signed (the signer reproduces AWS's
published example vector in its unit tests). Two S3 behaviors are defused
at the client: Range end-clamping (we demand EXACTLY the bytes asked for,
else loud Corrupt) and non-atomic overwrite (create-new writes send
If-None-Match:*, so `put_meta(overwrite=false)` — and therefore the writer
lock — requires an S3 implementation with conditional-write support; AWS
S3 and MinIO both qualify).

## Writer locking

`locks/writer` holds a CBOR {pid, token, created_unix, expires_unix}.
Claiming is a single atomic create-new (hard_link locally, If-None-Match:*
on S3). Release deletes the lockfile ONLY if it still carries the claim's
random token: a stale claimant can never evict its successor. A crashed
writer abandons its lock ON PURPOSE — recovery is expiry (expired locks
are stolen; garbage lockfiles count as expired) or the keyless
`warden unlock`. HONEST LIMIT: without compare-and-swap a pathological
resume of a long-sleeping stolen-from writer is not prevented; S3
conditional writes are the documented CAS upgrade path.

## Continuous verification (health log)

Every keyed proof-of-restorability appends a sealed HealthRecord
{version, time_unix, event, problems_total, problems_sample<=8} under
AD "warden/v0/health": Verify{deep, counts}, ReadBack{sampled/of counts},
Drill{snapshot, files, bytes}. Records are content-named like everything
else, so keyless scrub covers the receipts too, and keyed verify raw-audits
them. `warden health` computes status from the records and judges staleness
(verify_days / drill_days; 0 disables) — pessimistic tie-break when records
share an instant: the worst finding at the latest moment wins. Keyless
scrub deliberately writes NO record: recording requires keys; scrub is the
outside auditor whose report is its exit code.

## Known leaks (honest list)

Object counts, sealed sizes, and write timing → change-volume traffic analysis.
Snapshot count is visible as file count. Nothing else.
