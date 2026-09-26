# FAQ

**Is everything really written and runnable?**
Yes, at the scope each README states. 145 tests pass; `./run-all-tests.sh`
is the proof. Read the IS/IS-NOT table in the top-level README for scope.

**Did you build videocalls / a contacts server?**
No. Neither was built. clipsmith does ffmpeg probe/trim/thumbnail (not
calls). There is no contacts/CardDAV tool. This is stated wherever the
list appears rather than implied.

**Which of these are "real" vs. demos?**
All are real and tested. Depth differs: warden is a full eight-milestone
tool; P2-P11 are complete single-milestone vertical slices. "Vertical
slice" = the core job works end to end with tests, but one milestone of
depth, not eight.

**Can I use these in production?**
Cautiously and for what they are. warden is the most complete and has a
real backup/verify/prune story and a frozen format. ferry and relay are
UNENCRYPTED transports — tunnel them. Nothing here was hardened against
hostile input or exposed networks. Keep real backups of anything that
matters and read docs/DEPLOYMENT.md's security section.

**Why Rust 1.75 and pinned dependencies?**
That was the build/test toolchain; the locks guarantee a clean build
today. Want newer crates? Use current stable and `cargo update`.

**A build fails with an "edition2024" error.**
You removed a Cargo.lock. Restore it with `git checkout -- Cargo.lock`, or move to a current
toolchain and update. Details in docs/INSTALL.md.

**textract or clipsmith "not found" errors.**
Install `tesseract-ocr` / `ffmpeg`, or set TEXTRACT_TESSERACT /
CLIPSMITH_FFMPEG. Their test suites pass without these installed (the
real-binary tests self-skip).

**Where's the deep detail on warden?**
warden/README.md (invariants, usage), warden/docs/format-v0.md (frozen
format), warden/docs/bench-*.md (numbers), warden/CHANGELOG.md.

**How do I extend one of these?**
Each is a normal cargo crate. Logic is in lib.rs with tests; add a test
first (the codebase's habit), then the code. warden's invariants and
golden-vector regeneration (`cargo run -p warden-core --example
gen_golden`) are documented in warden/README.md and CONTRIBUTING.md.

**Windows?**
Use WSL2. The tools rely on Unix file/process semantics and some tests
assume it.
