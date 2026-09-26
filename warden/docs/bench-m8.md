# bench-m8: compression sniffing (debug builds, 1 vCPU container)

Method identical to bench-m4: 24 MiB of xorshift junk (incompressible),
one file, production chunker, Zstd repo, `--readback none`. Wall times
include the ~2.85 s KDF constant per invocation (measured in bench-m4);
the DELTA between rows is the zstd toll removed by sniffing.

| build                    | backup  | restore | stored bytes |
|--------------------------|---------|---------|--------------|
| v0.7.0-m7 always-compress| 10.39 s | 9.33 s  | 25 168 203   |
| M8 sniffing (this)       |  9.60 s | 8.59 s  | 25 167 483   |

Backup 7.7% faster, restore 7.9% faster, storage unchanged (the raw path
even saves zstd's per-frame bytes). This recovers the incompressible-data
regression bench-m4 recorded when packs+zstd landed (+4.6% backup /
+8.2% restore); compressible workloads keep their wins (see the
compression test suite's size assertions). Parallel-pipeline perf work
remains descoped: this container has one CPU and cannot verify it.
