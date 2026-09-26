# Testing guide — running the proofs

## Everything at once

    ./run-all-tests.sh

Runs warden's suites then the portfolio's (Linux, macOS, or WSL2). Expected totals: 145 tests
(109 + 36), zero failures. Warden's suites take a couple of minutes —
several of them do real work on purpose (see below).

## Per tree

    ( cd warden    && cargo test -p warden-core )   # 20 suites
    ( cd warden    && cargo test -p warden-cli )    # 9 suites (slower: real KDF)
    ( cd portfolio && cargo test --workspace )      # 30 suites / 36 tests

## Per tool

    ( cd portfolio && cargo test -p ferry )         # any of the ten names

## What the interesting suites actually do

- warden `crash.rs`: spawns the real binary, SIGKILLs it mid-backup at
  measured fractions, then proves committed history is untouched and the
  repo scrubs clean. Takes ~40s. That is the point.
- warden `prune.rs`: injects a failure at EVERY mutation of the garbage
  collector in turn; after each cut the repo must deep-verify green and
  restore byte-perfect.
- warden `s3.rs`: runs the whole product against an in-process mock S3
  that VERIFIES every request's SigV4 signature (a wrong secret is a 403).
- warden `hardening.rs`: seeded mutation storms and truncation ladders
  prove every decoder returns errors instead of panicking.
- ferry: kills a transfer at 100,000 of 300,000 bytes, then asserts the
  resume sends ONLY the remaining 200,000.
- relay: three real TCP clients prove room isolation over sockets.
- textract / clipsmith: engine seams tested against fakes/shims always,
  plus gated smoke tests against real tesseract/ffmpeg that SELF-SKIP
  (with a stderr note) when the binary is absent — absence never fails
  the suite.

## Reading a failure

Test names are sentences ("interrupted_transfer_resumes_from_offset_and_
verifies"); assertion messages carry the values and, in randomized tests,
the SEED to reproduce. If something fails on your machine, that name +
message + seed is a complete bug report.

## Lint gates (what "clean" meant at ship time)

    cargo clippy --all-targets -- -D warnings   # in either tree
    cargo fmt --check

## Latest recorded run

See RESULTS.md for the most recent full run: environment, per-suite counts
and timings, lint gates, demo output, and a quick warden benchmark.
