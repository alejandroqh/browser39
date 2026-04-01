#!/usr/bin/env bash
set -euo pipefail

# Static musl linking opens many file descriptors at once; raise the limit.
ulimit -n 4096 2>/dev/null || true

VERSION=$(cargo metadata --no-deps --format-version 1 | python3 -c "import sys,json; print(json.load(sys.stdin)['packages'][0]['version'])")
NAME="browser39"
OUT_DIR="dist"

mkdir -p "$OUT_DIR"

echo "=== Building $NAME v$VERSION ==="

# macOS ARM64 (native)
echo ""
echo "--- macOS arm64 (aarch64-apple-darwin) ---"
cargo build --release --target aarch64-apple-darwin
cp target/aarch64-apple-darwin/release/$NAME "$OUT_DIR/${NAME}-v${VERSION}-macos-arm64"
echo "  -> $OUT_DIR/${NAME}-v${VERSION}-macos-arm64"

# Linux ARM64 (cross-compile via zigbuild, static musl)
echo ""
echo "--- Linux arm64 (aarch64-unknown-linux-musl) ---"
cargo zigbuild --release --target aarch64-unknown-linux-musl
cp target/aarch64-unknown-linux-musl/release/$NAME "$OUT_DIR/${NAME}-v${VERSION}-linux-arm64"
echo "  -> $OUT_DIR/${NAME}-v${VERSION}-linux-arm64"

echo ""
echo "=== Done ==="
ls -lh "$OUT_DIR"/${NAME}-v${VERSION}-*
