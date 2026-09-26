# Overview

This repository contains eleven working command-line tools written in Rust,
built in two phases with two different depths — deliberately, and stated.

## Phase one: warden (backup), built to full depth

Warden is an encrypted, deduplicated snapshot backup tool whose defining
idea is that backups should PROVE they are restorable instead of being
assumed restorable. It was built across eight tagged milestones
(v0.1.0-m1 through v0.8.0-m8), each with its own tests, docs, and honest
benchmarks. What it does:

- Content-defined chunking, BLAKE3 identities, XChaCha20-Poly1305 sealing,
  Argon2 key derivation. Deduplication across snapshots.
- Pack files with per-blob compression framing and sniffing (incompressible
  data skips zstd; measured, see docs/bench-m8.md).
- Three verification tiers: keyless scrub (run it on the storage box, no
  passphrase), fast structural verify, deep verify that re-fetches and
  re-checks every reachable byte.
- A sealed, append-only health log: verify results, read-back-after-write
  results (every backup re-checks a sample of what it just wrote), and
  restore drills (randomly chosen files actually restored and compared).
  `warden health` turns the log into a cron-friendly exit code.
- Local filesystem and S3-compatible backends behind one conformance-tested
  trait; the SigV4 signer reproduces AWS's published example signature.
- Single-writer locking, crash-safe garbage collection (`forget` + `prune`)
  proven by fault injection at every mutation, and client-side rollback
  detection (the freshness pin).
- A frozen on-disk format (v1 candidate): future versions read today's
  repositories.

Its correctness story is unusual: seven numbered invariants stated before
code, golden format vectors machine-diffed at every regeneration, a
SIGKILL harness that kills real backups at measured fractions, and
fault-injection sweeps that fail every single mutation of the dangerous
operations in turn.

## Phase two: P2-P11, built fast as complete vertical slices

Ten more tools, each a single complete milestone: a working core, behavior
tests, a README that states scope and limits plainly. They share one cargo
workspace. What "vertical slice" means here: the tool does its central job
end to end and the tests prove it — but there was one milestone of depth,
not eight. Every README's IS/IS-NOT framing is the contract.

## The numbers

145 tests total (109 in warden, 36 in the portfolio), all passing;
`clippy -D warnings` clean and `cargo fmt --check` clean on both trees;
built on rustc 1.75, works on newer stable. Nothing in this repository ships
untested.
