# Getting started (zero to running, ~10 minutes)

## 1. Install Rust (once)

    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
    . "$HOME/.cargo/env"

Any recent stable toolchain works; the code was built on 1.75.

## 2. Clone and build

    git clone https://github.com/ChPuru/everything.git
    cd everything
    ./setup.sh

This builds all eleven tools in release mode and collects the binaries in
`./bin/`. First build takes a few minutes (dependencies compile once).

## 3. Prove it works

    ./run-all-tests.sh

Expected: every suite green — 109 warden tests, then 36 portfolio tests.
This is the same evidence the tools were shipped with.

## 4. First real thing: back something up and PROVE it restores

    export PATH="$PWD/bin:$PATH"
    export WARDEN_PASSPHRASE=pick-a-real-one

    warden --repo /tmp/demo-repo init
    warden --repo /tmp/demo-repo backup ~/Documents
    # prints a 64-char snapshot id; a sample of what was written was
    # already read back through full verification before it returned

    warden --repo /tmp/demo-repo verify --deep     # every byte re-checked
    warden --repo /tmp/demo-repo drill             # random files actually restored
    warden --repo /tmp/demo-repo health            # the receipts, as an exit code

    warden --repo /tmp/demo-repo restore <SNAPSHOT-ID> /tmp/demo-out
    diff -r ~/Documents /tmp/demo-out              # byte-identical

## 5. Tour the rest

Open `TRY-IT.md` at the repository root — copy-paste demos for all ten other
tools — or take the longer guided tour in `docs/WALKTHROUGH.md`.

## Optional system packages

- textract needs `tesseract-ocr` (apt/brew) for real OCR.
- clipsmith needs `ffmpeg`.
Both fail with a clear message rather than a mystery if absent.
