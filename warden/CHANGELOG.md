# warden changelog

## v0.8.0-m8 — hardening + freeze (P1 complete)
- Per-blob compression frame (tag 0 raw / 1 zstd) + sample-first sniffing:
  incompressible data skips zstd (7.7%/7.9% faster backup/restore on the
  24 MiB junk bench, storage unchanged — docs/bench-m8.md).
- Parse hardening: seeded mutation storms, truncation ladders, byte soup —
  every decoder proven total; chunker CDC locality property.
- Client-side freshness pin: `verify` detects whole-repo rollback; the
  alarm never self-clears; remediation documented.
- Restore read coalescing: adjacent chunks fetched as single ranged runs.
- Format FROZEN as v1 candidate. MIT license. Final review pass.

## v0.7.0-m7 — forget + prune (I6 retired)
Pack-granularity mark-and-sweep with green-at-every-cut deletion order,
proven by fault injection at every mutation; double refusal on damaged
repos; exact-accounting receipts; keep-last policies.

## v0.6.0-m6 — S3 + conformance + locking
Hand-rolled SigV4 (reproduces AWS's published vector), signature-verifying
mock, executable backend conformance contract, token-guarded writer lock,
s3:// CLI dispatch, keyless `unlock`.

## v0.5.0-m5 — continuous verification
Sealed health log; read-back-after-write; restore drills; `health` with
staleness clocks; pessimistic same-instant tie-break.

## v0.4.0-m4 — packs + zstd
Pack files, index v1 with ranged reads, compression, honest benches.

## v0.3.0-m3 — verify tiers + crash safety
Keyless scrub; keyed fast/deep verify; SIGKILL harness; fault injection.

## v0.2.0-m2 — trees
Directories, symlinks, hardlink dedup, deterministic tree encoding.

## v0.1.0-m1 — vertical slice
Encrypted deduplicated single-file backup/restore; golden vectors; the
seven invariants stated.
