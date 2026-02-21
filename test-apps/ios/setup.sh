#!/usr/bin/env bash
# Set up the iOS test package by copying the Rust library and UniFFI bindings.
#
# Usage: ./test-apps/ios/setup.sh [--simulator|--device]
#
# This must be run after:
#   1. scripts/build_ios.sh (builds the static library)
#   2. scripts/generate_bindings.sh --swift-only (generates Swift bindings)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$SCRIPT_DIR/../.."
PROXY_DIR="$PROJECT_DIR/openobscure-proxy"
BINDINGS_DIR="$PROJECT_DIR/bindings/swift"

TARGET="simulator"
for arg in "$@"; do
    case "$arg" in
        --device) TARGET="device" ;;
        --simulator) TARGET="simulator" ;;
    esac
done

echo "=== Setting up iOS test package (target: $TARGET) ==="

# Determine which library to use
if [ "$TARGET" = "simulator" ]; then
    # For running tests on macOS host (swift test) or iOS Simulator
    # Use the macOS debug build (same architecture, runs natively)
    LIB_PATH="$PROXY_DIR/target/debug/libopenobscure_proxy.a"
    if [ ! -f "$LIB_PATH" ]; then
        # Try simulator target
        LIB_PATH="$PROXY_DIR/target/aarch64-apple-ios-sim/debug/libopenobscure_proxy.a"
    fi
else
    LIB_PATH="$PROXY_DIR/target/aarch64-apple-ios/release/libopenobscure_proxy.a"
fi

if [ ! -f "$LIB_PATH" ]; then
    echo "Error: Static library not found at $LIB_PATH"
    echo "Run: scripts/build_ios.sh first"
    exit 1
fi

# Check bindings exist
if [ ! -f "$BINDINGS_DIR/openobscure_proxy.swift" ]; then
    echo "Error: Swift bindings not found at $BINDINGS_DIR/"
    echo "Run: scripts/generate_bindings.sh --swift-only first"
    exit 1
fi

# Copy FFI header to COpenObscure
cp "$BINDINGS_DIR/openobscure_proxyFFI.h" "$SCRIPT_DIR/COpenObscure/"
echo "Copied FFI header"

# Copy Swift bindings to OpenObscure target
cp "$BINDINGS_DIR/openobscure_proxy.swift" "$SCRIPT_DIR/OpenObscure/"
echo "Copied Swift bindings"

# Copy static library to a known location for linking
mkdir -p "$SCRIPT_DIR/lib"
cp "$LIB_PATH" "$SCRIPT_DIR/lib/libopenobscure_proxy.a"
echo "Copied static library ($(du -h "$SCRIPT_DIR/lib/libopenobscure_proxy.a" | cut -f1))"

echo ""
echo "=== Setup complete ==="
echo "Run tests with: cd test-apps/ios && swift test"
