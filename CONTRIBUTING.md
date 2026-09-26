# Contributing / extending

These are standard cargo crates; the working style that produced them:

- **Test first.** Every behavior here has a test written before or with
  the code. Add a failing test, then make it pass.
- **Library, then CLI.** Put logic in `lib.rs` so it's testable; keep
  `main.rs` a thin argument-parsing shell.
- **Name tests as sentences** and put values (and random seeds) in
  assertion messages, so a failure is a bug report.
- **Keep it green and clean.** Before committing:
      cargo test --workspace
      cargo clippy --all-targets -- -D warnings
      cargo fmt
- **warden specifics.** It has seven numbered invariants (listed in warden/README.md)
  and golden format vectors. If you touch on-disk encoding, regenerate
  vectors deliberately and diff them:
      cd warden && cargo run -p warden-core --example gen_golden
  The format is FROZEN (v1 candidate): add fields, don't change existing
  bytes.

Toolchain: originally built on rustc 1.75 with pinned lockfiles; verified on current stable (see docs/RESULTS.md). For newer deps, use
current stable and `cargo update`.
