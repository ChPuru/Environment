#!/bin/sh
# Builds all 11 tools in release mode and collects binaries into ./bin/
set -e
cd "$(dirname "$0")"
command -v cargo >/dev/null 2>&1 || { echo "Rust not found — install from https://rustup.rs and re-run."; exit 1; }
echo "==> building warden (P1)..."
( cd warden && cargo build --release -p warden-cli )
echo "==> building portfolio (P2..P11)..."
( cd portfolio && cargo build --release --workspace )
mkdir -p bin
cp warden/target/release/warden bin/
for b in ferry mirrorer shapeshift icalx mailsift shutterbug textract relay clipsmith gridcalc; do
  cp "portfolio/target/release/$b" bin/
done
echo
echo "==> done. 11 executables in ./bin — add it to PATH or run ./bin/<tool>"
ls -1 bin
