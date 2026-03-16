# Embedded Setup: OpenObscure as a Native Library

This guide covers the **platform-specific prerequisites** for compiling OpenObscure into your iOS, macOS, or Android app — cross-compilation targets, Android NDK, and environment variables.

> **Prerequisites:** Complete the [common setup](README.md) first (dev tools, Rust, clone).

---

## Platform-Specific Prerequisites

### iOS / macOS

Run from any directory — `rustup target add` is a global Rust toolchain command:

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

## Next Steps

Once prerequisites are installed:

1. **[Embedded Quick Start](../docs/get-started/embedded-quick-start.md)** — build the library, generate UniFFI bindings, download models, integrate into Swift or Kotlin, and verify detection
2. **[Integration Guide](../docs/integrate/embedding/INTEGRATION_GUIDE.md)** — Xcode SPM setup, Gradle + JNA + ProGuard configuration, OkHttp interceptor, and worked example diffs from tested apps (Enchanted, RikkaHub)
3. **[API Reference](../docs/reference/api-reference.md)** — full function list, type definitions, and error conditions
