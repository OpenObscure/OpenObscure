#!/usr/bin/env bash
# Build OpenObscure as a static library for iOS targets.
#
# Usage:
#   ./scripts/build_ios.sh [--release] [--xcframework]
#
# Prerequisites:
#   rustup target add aarch64-apple-ios aarch64-apple-ios-sim
#   Xcode with iOS SDK installed

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$SCRIPT_DIR/.."
PROXY_DIR="$PROJECT_DIR/openobscure-proxy"
PROFILE="debug"
BUILD_FLAG=""
CREATE_XCFRAMEWORK=false

for arg in "$@"; do
    case "$arg" in
        --release)
            PROFILE="release"
            BUILD_FLAG="--release"
            ;;
        --xcframework)
            CREATE_XCFRAMEWORK=true
            ;;
    esac
done

echo "=== OpenObscure iOS Build ==="
echo "Profile: $PROFILE"

# Verify targets are installed
for target in aarch64-apple-ios aarch64-apple-ios-sim; do
    if ! rustup target list --installed | grep -q "$target"; then
        echo "Installing target: $target"
        rustup target add "$target"
    fi
done

# Build for device (ARM64)
echo ""
echo "--- Building for iOS device (aarch64-apple-ios) ---"
cargo build --manifest-path "$PROXY_DIR/Cargo.toml" \
    --target aarch64-apple-ios $BUILD_FLAG --lib --features mobile-full

# Build for simulator (ARM64 — runs natively on Apple Silicon)
echo ""
echo "--- Building for iOS Simulator (aarch64-apple-ios-sim) ---"
cargo build --manifest-path "$PROXY_DIR/Cargo.toml" \
    --target aarch64-apple-ios-sim $BUILD_FLAG --lib --features mobile-full

DEVICE_LIB="$PROXY_DIR/target/aarch64-apple-ios/$PROFILE/libopenobscure_proxy.a"
SIM_LIB="$PROXY_DIR/target/aarch64-apple-ios-sim/$PROFILE/libopenobscure_proxy.a"

echo ""
echo "=== Build Complete ==="
echo "Device library:    $DEVICE_LIB"
echo "Simulator library: $SIM_LIB"

if [ -f "$DEVICE_LIB" ]; then
    echo "Device size: $(du -h "$DEVICE_LIB" | cut -f1)"
fi
if [ -f "$SIM_LIB" ]; then
    echo "Simulator size: $(du -h "$SIM_LIB" | cut -f1)"
fi

# Optionally create XCFramework
if [ "$CREATE_XCFRAMEWORK" = true ]; then
    echo ""
    echo "--- Creating XCFramework ---"
    XCFRAMEWORK_DIR="$PROXY_DIR/target/OpenObscure.xcframework"
    rm -rf "$XCFRAMEWORK_DIR"

    xcodebuild -create-xcframework \
        -library "$DEVICE_LIB" \
        -library "$SIM_LIB" \
        -output "$XCFRAMEWORK_DIR"

    echo "XCFramework: $XCFRAMEWORK_DIR"
fi
