#!/usr/bin/env bash
# Build OpenObscure as a shared library for Android targets.
#
# Usage:
#   ./scripts/build_android.sh [--release] [--all-abis]
#
# Prerequisites:
#   cargo install cargo-ndk
#   rustup target add aarch64-linux-android
#   Android NDK installed (via Android Studio or standalone)
#
# Environment variables:
#   ANDROID_NDK_HOME — path to Android NDK (auto-detected if installed via Android Studio)
#   ANDROID_API_LEVEL — minimum API level (default: 28, Android 9.0+)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$SCRIPT_DIR/.."
PROXY_DIR="$PROJECT_DIR/openobscure-proxy"
PROFILE="debug"
BUILD_FLAG=""
ALL_ABIS=false
API_LEVEL="${ANDROID_API_LEVEL:-28}"

for arg in "$@"; do
    case "$arg" in
        --release)
            PROFILE="release"
            BUILD_FLAG="--release"
            ;;
        --all-abis)
            ALL_ABIS=true
            ;;
    esac
done

echo "=== OpenObscure Android Build ==="
echo "Profile: $PROFILE"
echo "API Level: $API_LEVEL"

# Check cargo-ndk is installed
if ! command -v cargo-ndk &>/dev/null; then
    echo "Error: cargo-ndk not found. Install with: cargo install cargo-ndk"
    exit 1
fi

# Define targets
TARGETS=("aarch64-linux-android")
if [ "$ALL_ABIS" = true ]; then
    TARGETS+=("armv7-linux-androideabi" "x86_64-linux-android" "i686-linux-android")
fi

# Verify and install targets
for target in "${TARGETS[@]}"; do
    if ! rustup target list --installed | grep -q "$target"; then
        echo "Installing target: $target"
        rustup target add "$target"
    fi
done

# Build for each target
for target in "${TARGETS[@]}"; do
    echo ""
    echo "--- Building for $target ---"
    cargo ndk --manifest-path "$PROXY_DIR/Cargo.toml" \
        --target "$target" \
        --platform "$API_LEVEL" \
        build $BUILD_FLAG --lib
done

echo ""
echo "=== Build Complete ==="

# Map Rust targets to Android ABI names
declare -A ABI_MAP=(
    ["aarch64-linux-android"]="arm64-v8a"
    ["armv7-linux-androideabi"]="armeabi-v7a"
    ["x86_64-linux-android"]="x86_64"
    ["i686-linux-android"]="x86"
)

for target in "${TARGETS[@]}"; do
    abi="${ABI_MAP[$target]}"
    lib="$PROXY_DIR/target/$target/$PROFILE/libopenobscure_proxy.so"
    if [ -f "$lib" ]; then
        echo "$abi: $(du -h "$lib" | cut -f1)  ($lib)"
    else
        echo "$abi: NOT FOUND (expected at $lib)"
    fi
done

echo ""
echo "To use in an Android project, copy .so files to:"
echo "  app/src/main/jniLibs/<abi>/libopenobscure_proxy.so"
