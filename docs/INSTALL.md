# Installation, in detail

## Requirements

- Rust stable toolchain (rustup). Built on 1.75; newer stable is fine.
- Linux or macOS. Windows: use WSL2 (the tools use Unix process/file
  semantics; warden's crash tests and ferry's tests assume Unix).
- ~2 GB free disk for the release builds; a few minutes of CPU.
- Optional: `tesseract-ocr` (textract), `ffmpeg` (clipsmith).

## The one-command path

    ./setup.sh

builds `warden` (from warden/) and the ten portfolio tools (from
portfolio/) in release mode and copies all eleven executables into
`./bin/`. Add to PATH:

    export PATH="$PWD/bin:$PATH"        # session
    # or copy them somewhere permanent:
    sudo install -m755 bin/* /usr/local/bin/

## Building selectively

    ( cd warden    && cargo build --release -p warden-cli )   # just warden
    ( cd portfolio && cargo build --release -p ferry )        # just one tool
    ( cd portfolio && cargo install --path ferry )            # into ~/.cargo/bin

## Optional dependencies

Debian/Ubuntu:   sudo apt-get install tesseract-ocr ffmpeg
macOS (brew):    brew install tesseract ffmpeg

Overrides if the binaries live somewhere unusual:
    TEXTRACT_TESSERACT=/path/to/tesseract
    CLIPSMITH_FFMPEG=/path/to/ffmpeg  CLIPSMITH_FFPROBE=/path/to/ffprobe

## Troubleshooting

**"feature `edition2024` is required" or similar during build.**
You should not see this: both trees ship Cargo.lock files with every
dependency pinned to versions that build on 1.75+. If you deleted the
lock files, restore them with `git checkout`. If you WANT newer deps, use a
current toolchain and `cargo update` freely.

**Old toolchain (< 1.75).** `rustup update stable`.

**Linker/cc errors on a minimal Linux.** Install build essentials:
`sudo apt-get install build-essential` (a couple of C dependencies —
ring, zstd — compile during the warden build).

**textract: "tesseract not found".** Install the package or set
TEXTRACT_TESSERACT. The tool's test suite passes either way (the OCR
engine is a pluggable seam; the real-binary smoke test self-skips).

**warden: "set WARDEN_PASSPHRASE".** Every keyed command reads the
passphrase from that environment variable in this version.
