# everything

Eleven small, tested command-line tools written in Rust: one deep project
(**warden**, an encrypted backup tool that proves its snapshots restore) and
ten focused utilities for file transfer, sync, conversion, calendars, mail
archives, photos, OCR, chat, video, and spreadsheets.

Every claim in the table below has tests behind it. Test and benchmark runs
are recorded in [docs/RESULTS.md](docs/RESULTS.md).

## What is in here (and what is not)

| tool       | domain        | it IS                                        | it is NOT                        |
|------------|---------------|----------------------------------------------|----------------------------------|
| warden     | backup        | encrypted dedup snapshots that verify        | externally audited (yet)         |
|            |               | themselves; local + S3; GC; rollback alarm   |                                  |
| ferry      | file transfer | resumable TCP transfer, BLAKE3-gated         | a cloud service; encrypted       |
| mirrorer   | file sync     | one-way hash-verified mirror                 | bidirectional sync               |
| shapeshift | converter     | CSV/TSV <-> JSON, type inference             | Word/PDF conversion              |
| icalx      | calendar      | .ics parse + recurrence expansion + write    | a CalDAV server or GUI calendar  |
| mailsift   | email         | mbox archive stats/search/export             | an IMAP/SMTP mail client         |
| shutterbug | photos        | EXIF-dated organizing, dedup                 | a photo gallery app              |
| textract   | OCR           | batch OCR + searchable index (tesseract)     | its own OCR engine               |
| relay      | messaging     | TCP chat: rooms, nicks                       | accounts/encryption/persistence  |
| clipsmith  | video         | probe/trim/thumbnail via ffmpeg              | a video editor or call app       |
| gridcalc   | spreadsheet   | formula engine over CSV, cycle-safe          | a grid UI                        |

### Highlights

- **warden** — content-defined chunking, keyed BLAKE3 ids, XChaCha20-Poly1305
  sealing, Argon2 KDF; packs with zstd (skipped for incompressible data);
  keyless scrub, deep verify, restore drills, and a sealed health log that
  `warden health` turns into a cron-friendly exit code; hand-rolled SigV4 S3
  backend tested against a signature-verifying mock; crash-safe `forget` +
  `prune`; client-side rollback detection. On-disk format frozen (v1
  candidate). See [warden/README.md](warden/README.md).
- **ferry** — resume resends only the missing bytes; changed sources restart;
  whole-file BLAKE3 gate; path-traversal names refused.
- **mirrorer** — same-length-different-bytes is caught by hash; atomic copies;
  deletions are planned always, applied only with `--delete`.
- **shapeshift** — quoted commas/newlines survive round trips; ragged objects
  become deterministic union headers; nested values are refused, not flattened.
- **icalx** — unfolding/escaping, 75-octet folding, RRULE subset
  (DAILY/WEEKLY/MONTHLY/YEARLY with INTERVAL/COUNT/UNTIL/BYDAY/BYMONTHDAY).
- **mailsift** — `>From` quoting, folded headers, RFC 2822 dates with zone
  offsets, `=?B?`/`=?Q?` encoded subjects.
- **shutterbug** — pure-Rust EXIF `DateTimeOriginal` walker (both endians),
  `YYYY/MM` filing, BLAKE3 dedup, mtime fallback that says so.
- **textract** — engine trait; per-file failures recorded, never dropped.
- **relay** — nick/join/broadcast over real sockets; room isolation tested.
- **clipsmith** — pure argument builders + ffprobe JSON parsing; gated real
  ffmpeg smoke test.
- **gridcalc** — recursive-descent formulas, ranges, SUM/AVG/MIN/MAX/COUNT,
  memoized evaluation; cycles are named errors, not stack overflows.

## Quick start

Requirements: a Rust stable toolchain ([rustup.rs](https://rustup.rs)).
Optional: `tesseract` (textract) and `ffmpeg` (clipsmith); both tools explain
themselves if the binary is missing.

```sh
git clone https://github.com/ChPuru/everything.git
cd everything
./setup.sh           # release-builds all 11 tools into ./bin/
./run-all-tests.sh   # runs every test suite
export PATH="$PWD/bin:$PATH"
```

Or build one tree directly:

```sh
( cd warden    && cargo build --release -p warden-cli )
( cd portfolio && cargo build --release --workspace )
```

Platforms: Linux and macOS. On Windows, use WSL2: warden uses Unix file
metadata and does not compile natively (see [docs/RESULTS.md](docs/RESULTS.md)).

## Usage

```sh
# warden: back up, then prove it restores
export WARDEN_PASSPHRASE=demo
warden --repo /tmp/w-repo init
warden --repo /tmp/w-repo backup ./my-data      # prints snapshot id
warden --repo /tmp/w-repo verify --deep
warden --repo /tmp/w-repo drill
warden --repo /tmp/w-repo health

# shapeshift: CSV -> JSON with type inference
printf 'a,b\n1,true\n' | shapeshift csv2json --infer

# gridcalc: formulas over a CSV
printf '1,2,=A1+B1\n=SUM(A1:B1),,=C1*2\n' > sheet.csv
gridcalc recalc sheet.csv
```

More copy-paste demos for every tool: [TRY-IT.md](TRY-IT.md). A longer guided
tour: [docs/WALKTHROUGH.md](docs/WALKTHROUGH.md).

## Tests and benchmarks

Last full run (2026-09-26, rustc 1.98.0, Ubuntu 24.04 on WSL2):

| tree      | tests passed | failed | clippy `-D warnings` | `fmt --check` |
|-----------|--------------|--------|----------------------|---------------|
| warden    | 109          | 0      | clean                | clean         |
| portfolio | 36           | 0      | clean                | clean         |

Quick warden benchmark (release build, one 24 MiB incompressible file):
backup 0.21 s, unchanged re-backup 0.12 s (dedup), restore 0.15 s,
`verify --deep` 0.12 s. The restored file was byte-identical.

Full commands, environment, and output: [docs/RESULTS.md](docs/RESULTS.md).
Historical warden benchmarks: [warden/docs/bench-m4.md](warden/docs/bench-m4.md),
[warden/docs/bench-m8.md](warden/docs/bench-m8.md).

## Project layout

```
everything/
├── README.md, TRY-IT.md, CONTRIBUTING.md, LICENSE
├── setup.sh             # builds all 11 tools into ./bin/
├── run-all-tests.sh     # runs every test suite
├── docs/                # guides: install, testing, deployment, architecture, FAQ, results
├── warden/              # backup tool: Cargo workspace (warden-core lib + warden-cli)
│   └── docs/            # format spec, threat model, benchmarks
└── portfolio/           # the ten other tools: one Cargo workspace, one crate each
```

Documentation index: [docs/INDEX.md](docs/INDEX.md).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). In short: write a failing test first,
keep logic in `lib.rs`, and keep `cargo test`, `cargo clippy --all-targets --
-D warnings`, and `cargo fmt --check` green.

## License

MIT — see [LICENSE](LICENSE).
