#!/bin/sh
# Runs every suite in both workspaces. Expect 145 passing tests (109 warden + 36 portfolio).
set -e
cd "$(dirname "$0")"
echo "==> warden test suites (P1)..."
( cd warden && cargo test -p warden-core && cargo test -p warden-cli )
echo "==> portfolio test suites (P2..P11)..."
( cd portfolio && cargo test --workspace )
echo
echo "==> ALL GREEN."
