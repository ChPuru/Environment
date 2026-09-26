# portfolio — projects P2 through P11

Ten standalone Rust tools, one workspace, every crate green: 36 tests
across 30 suites, `clippy -D warnings` clean, `cargo fmt --check` clean,
rustc 1.75. Built after P1 (warden, a backup tool with 8 milestones and
its own repository) established the method; these ship as complete
vertical slices — working core, behavior tests, honest READMEs about
scope and limits — rather than multi-milestone builds. Each crate's
README states exactly what it proves and how.

| crate      | what it is                                                    |
|------------|---------------------------------------------------------------|
| ferry      | resumable TCP file transfer, whole-file BLAKE3 gate            |
| mirrorer   | one-way directory mirror, hash-honest change detection         |
| shapeshift | CSV/TSV <-> JSON with inference; refuses to flatten lies       |
| icalx      | iCalendar parse/expand/write; documented RRULE subset          |
| mailsift   | mbox stats/search/export; RFC2822 dates, encoded-word subjects |
| shutterbug | photo organizer; hand-rolled EXIF walker, both endians         |
| textract   | batch OCR pipeline; engine seam proven vs real tesseract       |
| relay      | TCP chat: rooms, nicks, isolation over real sockets            |
| clipsmith  | ffmpeg probe/trim/thumbnail; shim-validated seam + real gate   |
| gridcalc   | spreadsheet engine over CSV; formulas, ranges, named cycles    |

Run everything: `cargo test --workspace`.
