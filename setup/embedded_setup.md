# Embedded Setup: OpenObscure as a Native Library

This guide covers compiling OpenObscure into your iOS, macOS, or Android app as a static/dynamic library — no HTTP proxy needed. PII sanitization happens in-process via direct function calls.

> **Prerequisites:** Complete the [common setup](README.md) first (dev tools, Rust, clone).

---

## Platform-Specific Prerequisites

### iOS / macOS

```bash
# Install cross-compilation targets
rustup target add aarch64-apple-ios aarch64-apple-ios-sim

# Verify Xcode is installed (15+ with iOS SDK)
xcodebuild -version
```

### Android

```bash
# Install cross-compilation targets
rustup target add aarch64-linux-android x86_64-linux-android

# Install cargo-ndk (simplifies NDK builds)
cargo install cargo-ndk

# Install Android SDK + NDK (via Homebrew)
brew install --cask android-commandlinetools
sdkmanager "platforms;android-35" "build-tools;35.0.0" "ndk;27.2.12479018"
```

Set up environment variables (add to `~/.zshrc`):

```bash
export ANDROID_HOME="$HOME/Library/Android/sdk"
export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/27.2.12479018"
```

---

## Build the Native Library

### iOS (device + simulator)

```bash
cd ~/Desktop/OpenObscure

# Build static libraries
./build/build_ios.sh --release

# Create XCFramework (recommended for distribution)
./build/build_ios.sh --release --xcframework
```

**Output:**
- `openobscure-proxy/target/aarch64-apple-ios/release/libopenobscure_proxy.a` (device, ~160MB)
- `openobscure-proxy/target/aarch64-apple-ios-sim/release/libopenobscure_proxy.a` (simulator, ~160MB)
- `openobscure-proxy/target/OpenObscure.xcframework` (if `--xcframework`)

### macOS

```bash
cargo build --manifest-path openobscure-proxy/Cargo.toml \
  --lib --no-default-features --features mobile --release
```

**Output:**
- `openobscure-proxy/target/release/libopenobscure_proxy.a` (static, ~158MB)
- `openobscure-proxy/target/release/libopenobscure_proxy.dylib` (dynamic, ~19MB)

### Android (ARM64 + x86_64)

```bash
./build/build_android.sh --release --all-abis
```

**Output:**
- `openobscure-proxy/target/aarch64-linux-android/release/libopenobscure_proxy.so` (arm64-v8a, ~24MB)
- `openobscure-proxy/target/x86_64-linux-android/release/libopenobscure_proxy.so` (x86_64)

---

## Generate UniFFI Bindings

```bash
# Swift bindings (iOS/macOS)
./build/generate_bindings.sh --swift-only

# Kotlin bindings (Android)
./build/generate_bindings.sh --kotlin-only

# Both
./build/generate_bindings.sh
```

**Output files:**
- `bindings/swift/openobscure_proxy.swift` — UniFFI-generated Swift bridge
- `bindings/swift/openobscure_proxyFFI.h` — C FFI header
- `bindings/swift/openobscure_proxyFFI.modulemap` — Swift module map
- `bindings/kotlin/uniffi/openobscure_proxy/openobscure_proxy.kt` — UniFFI-generated Kotlin bridge

---

## API Overview

The embedded API is minimal — six functions cover all use cases:

| Function | Purpose |
|----------|---------|
| `createOpenobscure(configJson, fpeKeyHex)` | Initialize an `OpenObscureHandle` with config and FPE key |
| `sanitizeText(handle, text)` | Scan text for PII, encrypt matches, return sanitized text + mapping |
| `restoreText(handle, text, mappingJson)` | Decrypt FPE values in LLM response using saved mapping |
| `sanitizeImage(handle, imageBytes)` | EXIF strip (always) + face redact + OCR text redact + NSFW redact (model-dependent) |
| `sanitizeAudioTranscript(handle, transcript)` | Scan speech transcript for PII, return sanitized text + mapping |
| `checkAudioPii(handle, transcript)` | Quick PII count in audio transcript (no encryption) |
| `rotateKey(handle, newKeyHex)` | Rotate FPE key with 30-second overlap window |
| `scanResponse(handle, text)` | Scan LLM response for manipulation (cognitive firewall) |

---

## Next Steps: Integrate into Your App

For detailed integration instructions — Xcode project setup (SPM), Gradle configuration, OkHttp interceptor wiring, and working example diffs from tested third-party apps:

**[Integration Guide](../docs/integrate/embedding/INTEGRATION_GUIDE.md)** — covers:
- Bundling all models (~75 MB) with dynamic tier-based loading (Part 6a)
- Setting up a local SPM package for Xcode (iOS/macOS)
- Gradle + JNA + ProGuard configuration (Android)
- `OpenObscureManager` singleton pattern (key storage, sanitize/restore convenience methods)
- OkHttp interceptor for automatic request sanitization
- Tested integrations with [Enchanted](https://github.com/AugustDev/enchanted) and [RikkaHub](https://github.com/rikkahub/rikkahub)
- [Example diffs](../integration/examples/) and [reusable templates](../integration/templates/)
