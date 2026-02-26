#!/usr/bin/env bash
# build_napi.sh — Build the OpenObscure NAPI native scanner addon.
#
# Usage:
#   ./build/build_napi.sh          # release build
#   ./build/build_napi.sh --debug  # debug build (faster, larger)
#
# Output: openobscure-napi/scanner.node

set -euo pipefail
cd "$(dirname "$0")/.."

NAPI_DIR="openobscure-napi"

if [ ! -d "$NAPI_DIR" ]; then
  echo "Error: $NAPI_DIR/ directory not found"
  exit 1
fi

cd "$NAPI_DIR"

# Install napi-rs CLI if needed
if [ ! -d node_modules ]; then
  echo "Installing dependencies..."
  npm install --ignore-scripts
fi

# Build
if [ "${1:-}" = "--debug" ]; then
  echo "Building NAPI addon (debug)..."
  npx napi build
else
  echo "Building NAPI addon (release)..."
  npx napi build --release
fi

# Verify output
NODE_FILE=$(ls -1 scanner*.node 2>/dev/null | head -1)
if [ -z "$NODE_FILE" ]; then
  echo "Error: No .node file produced"
  exit 1
fi

SIZE=$(du -h "$NODE_FILE" | cut -f1)
echo ""
echo "Build complete:"
echo "  File: $NAPI_DIR/$NODE_FILE"
echo "  Size: $SIZE"
echo "  Arch: $(file "$NODE_FILE" | grep -oE '(arm64|x86_64)')"
