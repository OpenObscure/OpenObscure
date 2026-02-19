#!/usr/bin/env bash
# Generate UniFFI Swift and Kotlin bindings for OpenObscure mobile library.
#
# Usage:
#   ./scripts/generate_bindings.sh [--swift-only] [--kotlin-only]
#
# Output:
#   bindings/swift/   — Swift source files
#   bindings/kotlin/  — Kotlin source files

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$SCRIPT_DIR/.."
PROXY_DIR="$PROJECT_DIR/openobscure-proxy"
BINDINGS_DIR="$PROJECT_DIR/bindings"

GENERATE_SWIFT=true
GENERATE_KOTLIN=true

for arg in "$@"; do
    case "$arg" in
        --swift-only)
            GENERATE_KOTLIN=false
            ;;
        --kotlin-only)
            GENERATE_SWIFT=false
            ;;
    esac
done

echo "=== OpenObscure UniFFI Binding Generation ==="

# Build the library with mobile-full features (needed for uniffi-bindgen)
echo ""
echo "--- Building library with mobile-full features ---"
cargo build --manifest-path "$PROXY_DIR/Cargo.toml" --features mobile-full --lib

# Find the built library
LIB_PATH="$PROXY_DIR/target/debug/libopenobscure_proxy.dylib"
if [ ! -f "$LIB_PATH" ]; then
    LIB_PATH="$PROXY_DIR/target/debug/libopenobscure_proxy.so"
fi
if [ ! -f "$LIB_PATH" ]; then
    echo "Error: Could not find built library. Expected at:"
    echo "  $PROXY_DIR/target/debug/libopenobscure_proxy.{dylib,so}"
    exit 1
fi

echo "Using library: $LIB_PATH"

# Generate Swift bindings
if [ "$GENERATE_SWIFT" = true ]; then
    echo ""
    echo "--- Generating Swift bindings ---"
    mkdir -p "$BINDINGS_DIR/swift"
    cargo run --manifest-path "$PROXY_DIR/Cargo.toml" \
        --features mobile-full \
        --bin uniffi-bindgen -- \
        generate --library "$LIB_PATH" \
        --language swift \
        --out-dir "$BINDINGS_DIR/swift"
    echo "Swift bindings: $BINDINGS_DIR/swift/"
    ls -la "$BINDINGS_DIR/swift/" 2>/dev/null || true
fi

# Generate Kotlin bindings
if [ "$GENERATE_KOTLIN" = true ]; then
    echo ""
    echo "--- Generating Kotlin bindings ---"
    mkdir -p "$BINDINGS_DIR/kotlin"
    cargo run --manifest-path "$PROXY_DIR/Cargo.toml" \
        --features mobile-full \
        --bin uniffi-bindgen -- \
        generate --library "$LIB_PATH" \
        --language kotlin \
        --out-dir "$BINDINGS_DIR/kotlin"
    echo "Kotlin bindings: $BINDINGS_DIR/kotlin/"
    ls -la "$BINDINGS_DIR/kotlin/" 2>/dev/null || true
fi

echo ""
echo "=== Binding Generation Complete ==="
