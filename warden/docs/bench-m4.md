# M4 pack-format benchmark (measured 2026-07-05)

## Method — and its limits, stated up front

Debug builds (this container cannot justify two release builds), single CPU,
tmpfs-backed /tmp, 3 runs per cell, medians. Binaries: `v0.3.0-m3` (loose
objects, no compression) built from a git worktree vs the M4 tree (packs +
zstd-3). The effect under test is ARCHITECTURAL — syscall + fsync count per
stored object — which debug builds measure faithfully; absolute MB/s numbers
here are NOT representative of release performance. Fixed per-command cost
(Argon2id unlock at production parameters, debug build) measured via `list`:
**~2.85s in both binaries** — subtract it mentally from every cell below.

restic/rustic comparison is not possible in this environment (binaries
unreachable through the network allowlist; no Go toolchain; rustic's MSRV
exceeds rustc 1.75). That comparison is documented as a user-side follow-up.

## Workloads

- `small_files`: 500 files x 4 KiB random bytes across 20 directories.
- `big_file`: one 24 MiB random-bytes file. Random bytes are deliberately
  zstd's WORST case (incompressible), so compression overhead is maximal.

## Medians (wall clock, includes ~2.85s unlock)

| workload    | op      | m3 loose | m4 packs | delta        |
|-------------|---------|----------|----------|--------------|
| small_files | backup  | 5.18 s   | 3.79 s   | -27% wall    |
| small_files | restore | 4.23 s   | 4.17 s   | ~parity      |
| big_file    | backup  | 8.56 s   | 8.95 s   | +4.6% wall   |
| big_file    | restore | 7.54 s   | 8.16 s   | +8.2% wall   |

Net of the ~2.85s unlock, small-file backup work drops **~2.33s -> ~0.94s
(~2.5x)**. The mechanism is visible in the store itself:

| store (small_files) | files in objects/ | bytes    |
|---------------------|-------------------|----------|
| m3 loose            | 521               | 2087 KiB |
| m4 packs            | **1**             | 2061 KiB |

521 fsync'd files (each with parent-dir fsync) collapse into one pack write.
This is the small-file economics the milestone existed to fix, and it holds
even in a debug build because the win is IO count, not CPU.

## Honest regressions

Large incompressible data pays for zstd it cannot use: +4.6% backup, and
restore pays +8.2% for per-blob ranged reads plus decompression of
incompressible frames. Real-world data compresses; random bytes do not.
Mitigation queued for M8 perf polish: compressibility sniffing (store-raw
when a sample does not compress, as restic does) and read coalescing for
runs of adjacent blobs in one pack. Neither is worth complexity before the
format freeze bakes.
