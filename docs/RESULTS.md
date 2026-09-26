# Test and benchmark results

Recorded 2026-09-26. Everything below was run on this date; numbers are
copied from real output, trimmed for length.

## Environment

| item      | value                                                          |
|-----------|----------------------------------------------------------------|
| machine   | Intel Core i7-13650HX (20 threads), laptop                     |
| host OS   | Windows 11 (10.0.26200)                                        |
| test OS   | Ubuntu 24.04.3 LTS under WSL2 (kernel 5.15.167.4), 5 GiB RAM   |
| toolchain | rustc 1.98.0 / cargo 1.98.0 (stable)                           |
| lockfiles | the committed `Cargo.lock` files, unchanged                    |

Note: the machine was running other heavy builds at the same time, so the
timings below are conservative.

## Tests

Commands (from the repo root; equivalent to `./run-all-tests.sh`):

```sh
( cd warden    && cargo test --workspace --no-fail-fast )
( cd portfolio && cargo test --workspace --no-fail-fast )
```

| tree      | test targets  | tests passed | failed |
|-----------|---------------|--------------|--------|
| warden    | 29            | 109          | 0      |
| portfolio | 30            | 36           | 0      |
| **total** | **59**        | **145**      | **0**  |

### warden (slowest suites do real work on purpose)

| suite                        | tests | time   |
|------------------------------|-------|--------|
| warden-core unit tests       | 48    | 5.5 s  |
| tests/crash.rs (SIGKILL harness, real binary) | 1 | 52.1 s |
| tests/health_cli.rs          | 1     | 34.5 s |
| tests/prune_cli.rs           | 1     | 21.4 s |
| tests/pin_cli.rs             | 1     | 19.0 s |
| tests/cli.rs                 | 3     | 15.3 s |
| tests/prune.rs (fault injection at every GC mutation) | 4 | 15.3 s |
| tests/roundtrip.rs           | 6     | 8.4 s  |
| tests/verify.rs              | 6     | 0.7 s  |
| tests/health.rs              | 5     | 1.0 s  |
| tests/hardening.rs (seeded mutation storms) | 4 | 0.1 s |
| tests/s3.rs (signature-verifying mock S3) | 3 | 0.4 s |
| 15 other suites              | 26    | < 11 s each |

### portfolio

| crate      | tests | | crate      | tests |
|------------|-------|-|------------|-------|
| clipsmith  | 4     | | mirrorer   | 2     |
| ferry      | 4     | | relay      | 2     |
| gridcalc   | 6     | | shapeshift | 4     |
| icalx      | 6     | | shutterbug | 3     |
| mailsift   | 2     | | textract   | 3     |

The real-binary smoke tests (`real_tesseract_smoke_when_installed`,
`real_ffmpeg_smoke_when_installed`) skip themselves when `tesseract` or
`ffmpeg` is not on `PATH`. Neither was installed in the WSL environment, so
those two passed by skipping.

### Lint gates

```sh
cargo fmt --all --check                                  # exit 0, both trees
cargo clippy --workspace --all-targets -- -D warnings    # exit 0, both trees
```

`clippy.toml` in each workspace pins `msrv = "1.75.0"` so clippy suggestions
stay compatible with the original toolchain.

### Platform notes

- **warden is Unix-only.** `warden-core` uses `std::os::unix` for file modes
  and symlinks, so it does not compile on native Windows. Use Linux, macOS,
  or WSL2.
- **Native Windows was not verified in this run.** A native Windows
  `cargo test` of the portfolio got as far as compiling, where it failed on
  one clipsmith test helper that uses Unix file permissions. That test (it
  drives `#!/bin/sh` shim executables) is now marked `#[cfg(unix)]`. The
  rebuild after that fix did not finish on the loaded host, so Linux/WSL2
  is the supported test platform.

## Demo output (release build, WSL2)

```text
$ warden --repo repo init
initialized repository at repo
$ warden --repo repo backup data
22d71b8279297cdbddfc63b35a5ed0297758a416875575f72c73823906a56b55
$ warden --repo repo verify --deep
verified 1 snapshots, 2 trees, 2 chunks; 0 orphan objects; 0 problems
$ warden --repo repo drill
drilled snapshot 22d71b82…6b55: 2 files, 200009 bytes restored and verified
$ warden --repo repo health
health records: 3
last deep verify: 0 days ago
last fast verify: never
last read-back:   0 days ago
last drill:       0 days ago
last prune:       never
latest problems:  0
$ warden --repo repo scrub            # no passphrase set
scrubbed 6 stored files, 0 problems
$ warden --repo repo restore <id> out && diff -r data out && echo identical
restored to out
identical

$ printf 'a,b\n1,true\n' | shapeshift csv2json --infer
[
  {
    "a": 1,
    "b": true
  }
]
$ echo '[{"x":1},{"y":2}]' | shapeshift json2csv
x,y
1,
,2
$ printf '1,2,=A1+B1\n=SUM(A1:B1),,=C1*2\n' > sheet.csv
$ gridcalc recalc sheet.csv
1,2,3
3,,6
$ icalx cal.ics --from 20260101 --to 20261231   # WEEKLY;BYDAY=MO,WE;COUNT=4
2026-01-05 09:00  Standup
2026-01-07 09:00  Standup
2026-01-12 09:00  Standup
2026-01-14 09:00  Standup
```

## warden quick benchmark

One 24 MiB file of random (incompressible) bytes, local repository, release
build, default settings, one run each. Wall time includes the Argon2 key
derivation on every invocation.

| operation                          | wall time |
|------------------------------------|-----------|
| `backup` (first snapshot)          | 0.21 s    |
| `backup` again, unchanged (dedup)  | 0.12 s    |
| `restore`                          | 0.15 s    |
| `verify --deep`                    | 0.12 s    |

Repository size after both snapshots: 25,171,477 bytes for a
25,165,824-byte file (the second snapshot adds almost nothing). The restored
file was byte-identical (`cmp`).

These are single runs, not a statistical benchmark. The milestone benchmark
write-ups, measured with debug builds on a 1-vCPU container, are in
[warden/docs/bench-m4.md](../warden/docs/bench-m4.md) and
[warden/docs/bench-m8.md](../warden/docs/bench-m8.md).

## Reproduce

```sh
./run-all-tests.sh
cargo build --release -p warden-cli        # in warden/
head -c 25165824 /dev/urandom > junk.bin
export WARDEN_PASSPHRASE=bench
warden --repo brepo init
time warden --repo brepo backup junk.bin
time warden --repo brepo restore <snapshot-id> out.bin
cmp junk.bin out.bin
```
