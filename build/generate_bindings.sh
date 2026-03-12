#!/usr/bin/env bash
# Generate UniFFI Swift and Kotlin bindings for OpenObscure mobile library.
#
# Usage:
#   ./build/generate_bindings.sh [--swift-only] [--kotlin-only]
#
# Output:
#   bindings/swift/   — Swift source files
#   bindings/kotlin/  — Kotlin source files

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$SCRIPT_DIR/.."
PROXY_DIR="$PROJECT_DIR/openobscure-core"
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

# Build the library with bindgen features (includes mobile + cli).
# --no-default-features excludes voice (sherpa-rs) which requires native libs
# that are unavailable during cross-compilation and CI.
echo ""
echo "--- Building library with bindgen features ---"
cargo build --manifest-path "$PROXY_DIR/Cargo.toml" --no-default-features --features bindgen --lib

# Find the built library
LIB_PATH="$PROXY_DIR/target/debug/libopenobscure_core.dylib"
if [ ! -f "$LIB_PATH" ]; then
    LIB_PATH="$PROXY_DIR/target/debug/libopenobscure_core.so"
fi
if [ ! -f "$LIB_PATH" ]; then
    echo "Error: Could not find built library. Expected at:"
    echo "  $PROXY_DIR/target/debug/libopenobscure_core.{dylib,so}"
    exit 1
fi

echo "Using library: $LIB_PATH"

# uniffi-bindgen needs cargo metadata, which requires running from a directory
# with a Cargo.toml. Run from within the core directory.
BINDGEN_BIN="$PROXY_DIR/target/debug/uniffi-bindgen"

# Build the uniffi-bindgen binary
cargo build --manifest-path "$PROXY_DIR/Cargo.toml" --no-default-features --features bindgen --bin uniffi-bindgen

# Generate Swift bindings
if [ "$GENERATE_SWIFT" = true ]; then
    echo ""
    echo "--- Generating Swift bindings ---"
    mkdir -p "$BINDINGS_DIR/swift"
    (cd "$PROXY_DIR" && "$BINDGEN_BIN" generate \
        --library "$LIB_PATH" \
        --language swift \
        --out-dir "$BINDINGS_DIR/swift")
    echo "Swift bindings: $BINDINGS_DIR/swift/"
    ls -la "$BINDINGS_DIR/swift/" 2>/dev/null || true
fi

# Generate Kotlin bindings
if [ "$GENERATE_KOTLIN" = true ]; then
    echo ""
    echo "--- Generating Kotlin bindings ---"
    mkdir -p "$BINDINGS_DIR/kotlin"
    (cd "$PROXY_DIR" && "$BINDGEN_BIN" generate \
        --library "$LIB_PATH" \
        --language kotlin \
        --out-dir "$BINDINGS_DIR/kotlin")
    echo "Kotlin bindings: $BINDINGS_DIR/kotlin/"
    ls -la "$BINDINGS_DIR/kotlin/" 2>/dev/null || true
fi

echo ""
echo "=== Binding Generation Complete ==="
