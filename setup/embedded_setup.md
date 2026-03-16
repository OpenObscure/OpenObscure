# Embedded Setup: OpenObscure as a Native Library

Compile OpenObscure into your iOS, macOS, or Android app — no proxy, no HTTP server. PII is sanitized in-process before it ever leaves the device.

> **Prerequisites:** Complete the [common setup](README.md) first (dev tools, Rust toolchain, clone, `git lfs pull`).

---

## Part 1: Platform Prerequisites

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

## Part 2: Build the Library

> All build commands assume the working directory is the **repo root** (the directory containing `openobscure-core/` and `build/`).

### macOS

```bash
cargo build --manifest-path openobscure-core/Cargo.toml \
  --lib --no-default-features --features mobile --release
```

> `--features mobile` disables the HTTP proxy layer (axum/hyper) and enables UniFFI bindings. Use this flag for all embedded builds.

Expected output:
```
openobscure-core/target/release/libopenobscure_core.dylib   (~5–10 MB)
openobscure-core/target/release/libopenobscure_core.a       (~150 MB, includes ORT static lib)
```

### iOS (device + simulator)

```bash
./build/build_ios.sh --release

# Optional: create XCFramework for Xcode / SPM distribution
./build/build_ios.sh --release --xcframework
```

Expected output:
```
openobscure-core/target/aarch64-apple-ios/release/libopenobscure_core.a      (~150 MB)
openobscure-core/target/aarch64-apple-ios-sim/release/libopenobscure_core.a  (~150 MB)
```

With `--xcframework`:
```
openobscure-core/target/OpenObscure.xcframework/
```

> Each `.a` is large because ORT is linked statically on iOS — third-party dynamic libraries are not permitted.

### Android (ARM64)

```bash
# Requires cargo-ndk + Android NDK (see Part 1)
./build/build_android.sh --release

# Optional: all ABIs
./build/build_android.sh --release --all-abis
```

Expected output:
```
openobscure-core/target/aarch64-linux-android/release/libopenobscure_core.so  (~5–10 MB)
```

With `--all-abis`:
```
openobscure-core/target/aarch64-linux-android/release/libopenobscure_core.so    (arm64-v8a)
openobscure-core/target/armv7-linux-androideabi/release/libopenobscure_core.so  (armeabi-v7a)
openobscure-core/target/x86_64-linux-android/release/libopenobscure_core.so     (x86_64)
```

Copy each `.so` into your Android project:
```
app/src/main/jniLibs/arm64-v8a/libopenobscure_core.so
app/src/main/jniLibs/armeabi-v7a/libopenobscure_core.so
app/src/main/jniLibs/x86_64/libopenobscure_core.so
```

---

## Part 3: Generate UniFFI Bindings

```bash
# Swift (iOS / macOS)
./build/generate_bindings.sh --swift-only
```

Expected output:
```
bindings/swift/openobscure_core.swift
bindings/swift/openobscureProxy.modulemap
```

Drag both files into your Xcode project. The `.swift` file is the generated API surface; the `.modulemap` exposes the underlying C header to Swift.

```bash
# Kotlin (Android)
./build/generate_bindings.sh --kotlin-only
```

Expected output:
```
bindings/kotlin/uniffi/openobscure_core/openobscure_core.kt
```

Add the file to your Android source set (e.g. `app/src/main/java/` or a dedicated `uniffi/` directory). It must be compiled alongside `libopenobscure_core.so`.

---

## Part 4: Download Models

Models enable NER, image pipeline, voice KWS, and cognitive firewall. Without them, OpenObscure falls back to regex + keyword + gazetteer detection — 15 structured PII types still covered, but names, locations, and orgs are not detected.

```bash
# From repo root — downloads BlazeFace, SCRFD, PaddleOCR (~14 MB)
./build/download_models.sh

# NER, NSFW, response integrity, KWS models (~175 MB — stored in Git LFS)
git lfs pull
```

Expected model directories after both commands:
```
openobscure-core/models/
  blazeface/          — all tiers      (BlazeFace face detection, ~408 KB)
  paddleocr/          — all tiers      (PaddleOCR det + rec + dict, ~10 MB)
  scrfd/              — standard, full (SCRFD-2.5GF, ~3.1 MB)
  ner/                — full tier      (DistilBERT INT8, ~64 MB)
  ner-lite/           — lite, standard (TinyBERT INT8, ~14 MB)
  nsfw_classifier/    — all tiers      (ViT-base INT8, ~83 MB)
  ri/                 — full tier      (R2 response integrity, ~14 MB)
  kws/                — full + voice   (Zipformer KWS, ~5 MB)
```

Bundle the `models/` directory into your app's resources (iOS) or assets (Android). Set `models_base_dir` in your config JSON to point to the directory containing these subdirectories.

> **No models?** The library starts without error. Missing models disable only those features — regex detection still runs. Call `getDebugLog(handle)` after init to confirm which models loaded.

---

## Part 5: Integrate

### Step 1 — Generate an FPE key

```bash
openssl rand -hex 32
```

Store the result in iOS Keychain or Android Keystore. **Never hard-code it in source.** Pass it to `createOpenobscure()` at runtime.

### Step 2 — Initialize and sanitize (Swift)

```swift
import openobscure_core

// Load key from Keychain — never hard-code in source
let fpeKey = KeychainHelper.load(key: "openobscure-fpe-key")!

let modelsDir = Bundle.main.resourcePath! + "/models"
let config = """
{"scanner_mode": "auto", "models_base_dir": "\(modelsDir)"}
"""

let handle = try createOpenobscure(configJson: config, fpeKeyHex: fpeKey)

// Sanitize user input before sending to LLM
let result = try sanitizeText(handle: handle, text: userMessage)
// Save result.mappingJson — needed to restore after LLM responds

// Restore original PII from LLM response
let restored = try restoreText(
    handle: handle,
    text: llmResponse,
    mappingJson: result.mappingJson
)
```

### Step 2 — Initialize and sanitize (Kotlin)

```kotlin
import uniffi.openobscure_core.*

// Load key from Android Keystore — never hard-code in source
val fpeKey = keystoreHelper.load("openobscure-fpe-key")

// createOpenobscure() requires a real filesystem path — APK asset URIs not accepted.
// Copy models/ to internal storage on first launch.
fun copyAssets(src: String, dest: File) {
    val items = context.assets.list(src) ?: return
    if (items.isEmpty()) { context.assets.open(src).use { it.copyTo(dest.outputStream()) }; return }
    dest.mkdirs()
    items.forEach { copyAssets("$src/$it", File(dest, it)) }
}
val modelsDir = context.filesDir.resolve("models")
    .also { if (!it.exists()) copyAssets("models", it) }.absolutePath

val config = """{"scanner_mode": "auto", "models_base_dir": "$modelsDir"}"""

val handle = createOpenobscure(configJson = config, fpeKeyHex = fpeKey)

// Sanitize user input before sending to LLM
val result = sanitizeText(handle = handle, text = userMessage)

// Restore after LLM responds
val restored = restoreText(
    handle = handle,
    text = llmResponse,
    mappingJson = result.mappingJson
)
```

---

## Part 6: Verify Detection

Call `sanitizeText()` with a known PII value before wiring the full LLM flow:

**Swift:**
```swift
let result = try sanitizeText(handle: handle, text: "My SSN is 123-45-6789")
print(result.sanitizedText)
// Expected: My SSN is 847-29-3156  (encrypted value differs per key)
```

**Kotlin:**
```kotlin
val result = sanitizeText(handle = handle, text = "My card is 4111-1111-1111-1111")
println(result.sanitizedText)
// Expected: My card is 7392-8841-5503-2947  (encrypted value differs per key)
```

Then check the active tier:

**Swift:**
```swift
let stats = getStats(handle: handle)
print(stats.deviceTier)  // "full", "standard", or "lite"
```

> **If `sanitizedText` equals your input unchanged** — regex detection failed. Check that `createOpenobscure()` received a valid 64-char hex key and a non-empty config.
>
> **If `deviceTier` is `"lite"` but you expected `"full"`** — verify `models_base_dir` is correct and that `ner/`, `nsfw_classifier/`, `ri/`, `kws/` subdirectories are present. Run `getDebugLog(handle)` to see which model paths were attempted.

---

## API Reference

| Function | Purpose |
|----------|---------|
| `createOpenobscure(configJson, fpeKeyHex)` | Initialize with config and FPE key |
| `sanitizeText(handle, text)` | Scan + encrypt PII, return sanitized text + mapping |
| `restoreText(handle, text, mappingJson)` | Decrypt FPE values using saved mapping |
| `sanitizeImage(handle, imageBytes)` | EXIF strip (always) + face/OCR/NSFW redaction (model-dependent) |
| `sanitizeAudioTranscript(handle, transcript)` | Scan speech transcript for PII |
| `checkAudioPii(handle, transcript)` | Quick PII count without encryption |
| `rotateKey(handle, newKeyHex)` | Rotate FPE key with 30-second overlap |
| `scanResponse(handle, text)` | Scan LLM response for manipulation techniques |
| `getStats(handle)` | PII counts, scanner mode, image pipeline status, device tier |
| `getDebugLog(handle)` | Retrieve buffered log entries for diagnostics |

---

## Part 7: Troubleshooting

### Build fails with "feature `mobile` not found"

Make sure you are building from the repo root and passing `--manifest-path openobscure-core/Cargo.toml`:

```bash
cargo build --manifest-path openobscure-core/Cargo.toml \
  --lib --no-default-features --features mobile --release
```

### "could not find `cargo-ndk`"

```bash
cargo install cargo-ndk
```

### Models not loading — tier shows "lite" unexpectedly

1. Confirm `git lfs pull` completed without errors — LFS pointer files (not real model data) will cause silent fallback to lite
2. Confirm `models_base_dir` in config JSON points to the directory containing `ner/`, `nsfw_classifier/`, etc. (not to a subdirectory)
3. Run `getDebugLog(handle)` immediately after `createOpenobscure()` to see which paths were tried

### `createOpenobscure()` throws on iOS

The most common causes:
- **Invalid key** — must be exactly 64 hexadecimal characters (`openssl rand -hex 32` produces the correct format)
- **Bad config JSON** — validate the JSON string before passing it
- **Models path wrong** — use `Bundle.main.resourcePath! + "/models"` and verify the models directory is included in the app target's Copy Bundle Resources build phase

---

## Next Steps

1. **[Integration Guide](../docs/integrate/embedding/INTEGRATION_GUIDE.md)** — Xcode SPM setup, Gradle + JNA + ProGuard, OkHttp interceptor, and worked diffs from tested apps (Enchanted, RikkaHub)
2. **[API Reference](../docs/reference/api-reference.md)** — full function signatures, type definitions, and error conditions
3. **[Deployment Tiers](../docs/get-started/deployment-tiers.md)** — what each tier enables and how to override
