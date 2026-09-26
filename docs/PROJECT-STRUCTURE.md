# Project structure — where everything lives

    everything/
    ├── README.md              # overview + the IS/IS-NOT table
    ├── setup.sh               # builds all 11 tools -> ./bin/
    ├── run-all-tests.sh       # runs every suite (145 tests)
    ├── TRY-IT.md              # copy-paste demos
    ├── docs/                  # THIS documentation set (see docs/INDEX.md, RESULTS.md)
    ├── bin/                   # created by setup.sh; the 11 executables
    │
    ├── warden/                # P1 — the full-depth project
    │   ├── README.md
    │   ├── CHANGELOG.md
    │   ├── Cargo.toml         # workspace
    │   ├── Cargo.lock         # pinned for rustc 1.75
    │   ├── docs/
    │   │   ├── format-v0.md   # the FROZEN on-disk format spec
    │   │   ├── bench-m4.md    # honest perf numbers (packs+zstd)
    │   │   ├── bench-m8.md    # honest perf numbers (sniffing)
    │   │   └── threat-model.md
    │   └── crates/
    │       ├── warden-core/   # the library: chunker, crypto, objects,
    │       │   ├── src/        #   index, packs, backend/{local,s3,...},
    │       │   ├── tests/      #   verify, health, lock, repo
    │       │   └── examples/gen_golden.rs   # regenerates golden vectors
    │       └── warden-cli/    # the `warden` binary + its e2e tests
    │
    └── portfolio/             # P2..P11 — one workspace, ten crates
        ├── Cargo.toml         # workspace + shared pinned deps
        ├── Cargo.lock
        ├── ferry/             # each crate: Cargo.toml, README.md,
        │   ├── src/lib.rs     #   src/lib.rs (logic + unit/integration
        │   ├── src/main.rs    #   tests) and src/main.rs (CLI)
        │   └── README.md
        ├── mirrorer/   ├── shapeshift/  ├── icalx/     ├── mailsift/
        ├── shutterbug/ ├── textract/    ├── relay/     ├── clipsmith/
        └── gridcalc/

## Conventions used everywhere

- Library in `lib.rs`, thin CLI in `main.rs`. Logic is testable without
  the binary.
- Tests live beside the code (`#[cfg(test)]`) or in `tests/` for
  integration. Test names are sentences; assertions carry values and seeds.
- warden pins its Cargo.lock for rustc 1.75; the portfolio shares one
  pinned lock. Delete them only if you intend to `cargo update`.
- No hidden global state; no network calls in tests except to localhost
  sockets the test itself binds.
