# Architecture — the one decision that defines each tool

Every tool here is organized around a single load-bearing idea. If you
read nothing else about the design, read these.

## warden: identity is content, and proof is a first-class output
Everything stored is named by the BLAKE3 hash of its (sealed) bytes. That
one choice gives deduplication, keyless integrity scrubbing (you can
verify names against contents with no passphrase), and tamper-evidence for
free. The second idea — that a backup should EMIT PROOF — produces the
health log, read-back-after-write, and drills. Layers: warden-core
(library: chunker, crypto, objects, index, packs, backends, verify,
health, lock, repo) and warden-cli (the command surface). Backends
(local, S3) sit behind one trait with an executable conformance suite, so
"works on disk" and "works on S3" are the same guarantee.

## ferry: the receiver's byte offset is the protocol
Resume is not bolted on; it is the design. One JSON header line declares
(name, length, hash); the receiver replies with how many bytes it already
has for THIS (hash, length); the sender seeks and streams the rest.
Completion is a whole-file hash gate. A changed source can't resume onto
stale bytes because the partial is bound to its content hash.

## mirrorer: plan and apply are separate, and length is not identity
`plan()` is a pure function producing a list of copies and deletions;
`apply()` executes it. Change detection compares hashes when lengths
match, so a same-size edit is never missed. Deletions are always planned,
never applied without `--delete`. Copies are atomic (tmp + rename).

## shapeshift: refuse rather than lie
CSV<->JSON is easy until quoting and raggedness. The design commitment is
that ambiguity is surfaced, not papered over: nested JSON can't become a
CSV cell (error, not silent flattening), ragged objects get a
deterministic union-of-keys header, and the CSV round trip is a tested
fixed point.

## icalx: a documented subset beats a pretended full implementation
Calendaring is a swamp of edge cases. icalx implements a precise RRULE
subset and REFUSES everything outside it loudly, so you always know what
you got. All date math is Hinnant's civil-date algorithm (no chrono),
leap-year tested both directions.

## mailsift: parse the mail that archives actually contain
Not clean RFC mail — decades-old mbox reality: `>From ` body quoting,
folded headers, zone-offset RFC 2822 dates, MIME encoded-word subjects.
The design is a faithful reader (raw message preserved) with a small
composable search over parsed headers.

## shutterbug: read the bytes, don't trust a library
EXIF DateTimeOriginal is parsed by walking JPEG APP1 / TIFF IFDs directly
in both byte orders — no native dependency. Planning is pure (returns a
Move or a SkipIdentical decision); dedup is by BLAKE3; missing EXIF falls
back to mtime and SAYS so. The synthetic-JPEG generator is public so tests
carry no binary blobs.

## textract: own the pipeline, rent the pixels
OCR itself is hard and solved (tesseract). The design draws an honest line:
textract owns the batch pipeline, the searchable index, per-file error
capture, and search; the OCR engine is a trait. Tests prove the pipeline
with a fake engine and prove the seam with a gated real-tesseract smoke.

## relay: isolate rooms, never block on a slow reader
A shared hub holds clients; each client has its OWN writer thread fed by a
channel, so one stalled socket can't freeze a room. Broadcasts filter by
room, giving isolation. The whole protocol is three commands over lines.

## clipsmith: pure arg-building, tested seam, real gate
The risky-to-test part (spawning ffmpeg) is isolated. Argument construction
and ffprobe-JSON parsing are pure and unit-tested; the process seam is
proven end-to-end against SHIM executables that validate argv; a gated
smoke runs real ffmpeg when present. You get confidence without a
half-gigabyte test dependency.

## gridcalc: evaluate by need, and make cycles NAMED errors
A recursive-descent parser (expr/term/factor) gives correct precedence;
evaluation is memoized and tracks a visiting-set so a reference cycle
becomes an error that NAMES the cell instead of a stack overflow. Text is
0 for math; ranges and functions are explicit. Semantics live in the
tests.
