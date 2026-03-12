# Embedded Quick Start

Sanitize PII directly in your iOS, macOS, or Android app — no proxy, no HTTP server. Build the library, generate bindings, call three functions.

---

**Contents**

- [Prerequisites](#prerequisites)
- [1. Build the library](#1-build-the-library)
- [2. Generate bindings](#2-generate-bindings)
- [3. Download models](#3-download-models)
- [4. Integrate](#4-integrate)
- [4b. Verify your first detection](#4b-verify-your-first-detection)
- [5. Verify](#5-verify)
- [Full API](#full-api)
- [What Happens Under the Hood](#what-happens-under-the-hood)
- [Next Steps](#next-steps)

## Prerequisites

| Tool | Minimum version | Required for |
|------|----------------|--------------|
| Rust | **1.75** | All targets. MSRV set in `openobscure-proxy/Cargo.toml`. Install via [rustup.rs](https://rustup.rs). |
| Cargo | ships with Rust | All targets. Bundled with the Rust toolchain. No separate minimum. |
| Xcode | — | iOS and macOS builds. No minimum version pinned; CI runs on macOS 14. Must include command-line tools (`xcode-select --install`). |
| Android NDK | — | Android builds only. No minimum version pinned in `build_android.sh`. Install via Android Studio SDK Manager or `sdkmanager`. |
| cargo-ndk | — | Android builds only. Required by `./build/build_android.sh`. Install: `cargo install cargo-ndk`. |
| Git LFS | any | Pulling NER, KWS, and RI model files (`git lfs pull`). Not needed for regex-only builds or if using `download_models.sh` exclusively. |
| ONNX Runtime | auto-downloaded | All targets. The `ort` crate (`=2.0.0-rc.11`) downloads the native library at build time. No manual installation required. |
| FPE key | 64-char hex string (32 bytes) | All targets — passed to `createOpenobscure()`. Generate: `openssl rand -hex 32`. Store in iOS Keychain or Android Keystore; never hard-code in source. |

Node.js and npm are **not** required for the embedded library. They are only needed if you are also building the gateway L1 plugin.

---

## 1. Build the library

> All commands in this guide assume the working directory is the repository root (the directory containing `openobscure-proxy/` and `build/`).

### macOS

```bash
cargo build --manifest-path openobscure-proxy/Cargo.toml \
  --lib --no-default-features --features mobile --release
```

> `--features mobile` disables the HTTP proxy layer (axum/hyper) and enables UniFFI bindings — use this flag for all embedded builds. Building the gateway instead? See [Gateway Quick Start](gateway-quick-start.md).

> **You should see:**
> ```
> openobscure-proxy/target/release/libopenobscure_proxy.dylib   (~5–10 MB)
> openobscure-proxy/target/release/libopenobscure_proxy.a       (~150 MB, includes ORT static lib)
> ```
> The `.dylib` (cdylib) is used for macOS app integration. The `.a` (staticlib) is available for static linking. ORT is loaded dynamically on macOS, so the dylib contains only the Rust code.

### iOS (device + simulator)

```bash
./build/build_ios.sh --release

# Optional: create XCFramework for distribution
./build/build_ios.sh --release --xcframework
```

> **You should see:**
> ```
> openobscure-proxy/target/aarch64-apple-ios/release/libopenobscure_proxy.a      (~150 MB)
> openobscure-proxy/target/aarch64-apple-ios-sim/release/libopenobscure_proxy.a  (~150 MB)
> ```
> Each `.a` is large because it includes the ORT static library — iOS does not permit third-party dynamic libraries, so ORT is linked in at build time. The build script prints the exact sizes at completion.
>
> With `--xcframework`, the two slices are combined into:
> ```
> openobscure-proxy/target/OpenObscure.xcframework/
> ```
> Use the XCFramework when distributing via Swift Package Manager or adding to an Xcode project directly.

### Android (ARM64)

```bash
# Requires cargo-ndk + Android NDK
./build/build_android.sh --release

# Optional: all ABIs (arm64-v8a, armeabi-v7a, x86_64)
./build/build_android.sh --release --all-abis
```

> **You should see:**
> ```
> openobscure-proxy/target/aarch64-linux-android/release/libopenobscure_proxy.so  (~5–10 MB)
> ```
> With `--all-abis`, one `.so` is produced per ABI:
> ```
> openobscure-proxy/target/aarch64-linux-android/release/libopenobscure_proxy.so    (arm64-v8a)
> openobscure-proxy/target/armv7-linux-androideabi/release/libopenobscure_proxy.so  (armeabi-v7a)
> openobscure-proxy/target/x86_64-linux-android/release/libopenobscure_proxy.so     (x86_64)
> ```
> The `.so` files are small because ORT is loaded dynamically at runtime on Android (`load-dynamic` feature). Copy each into your Android project under the matching ABI directory:
> ```
> app/src/main/jniLibs/arm64-v8a/libopenobscure_proxy.so
> app/src/main/jniLibs/armeabi-v7a/libopenobscure_proxy.so
> app/src/main/jniLibs/x86_64/libopenobscure_proxy.so
> ```

## 2. Generate bindings

```bash
# Swift (iOS/macOS)
./build/generate_bindings.sh --swift-only
```

> **You should see:**
> ```
> bindings/swift/openobscure_proxy.swift
> bindings/swift/openobscureProxy.modulemap
> ```
> Drag both files into your Xcode project. The `.swift` file contains the generated API surface; the `.modulemap` exposes the underlying C header to Swift.

```bash
# Kotlin (Android)
./build/generate_bindings.sh --kotlin-only
```

> **You should see:**
> ```
> bindings/kotlin/uniffi/openobscure_proxy/openobscure_proxy.kt
> ```
> Add the file to your Android source set (e.g. `app/src/main/java/` or a dedicated `uniffi/` source directory). It must be compiled alongside `libopenobscure_proxy.so`.

## 3. Download models

Models enable NER, image pipeline, and cognitive firewall. Without them, OpenObscure falls back to regex + keyword + gazetteer detection only. NER, face redaction, OCR, NSFW detection, the cognitive firewall, and voice keyword spotting are all silently inactive until model files are present — no exception is thrown. Set `models_base_dir` in your config JSON to activate them.

<details>
<summary>Full list of features inactive without model files</summary>

| Feature | Inactive when... | What still works |
|---------|-----------------|-----------------|
| NER-based PII detection (names, locations, orgs) | NER model absent from `models_base_dir/ner/` or `ner_lite/` | Regex + keyword + gazetteer — 15 structured PII types |
| Face redaction in `sanitizeImage()` | No face model in `models_base_dir/blazeface/` or `scrfd/` | EXIF stripping always runs; pixels forwarded without face blurring |
| OCR text redaction in `sanitizeImage()` | No OCR model in `models_base_dir/ocr/` | EXIF stripping always runs; text in images not redacted |
| NSFW detection in `sanitizeImage()` | No NSFW model in `models_base_dir/nsfw_classifier/` | Images processed but nudity not detected or redacted |
| R2 cognitive firewall in `scanResponse()` | No RI model in `models_base_dir/ri/` | R1 dictionary-based persuasion detection still runs |
| Voice keyword spotting | KWS models absent from `models_base_dir/kws/` | Audio transcripts processed by text scanner only |

</details>

> **Download size:** ~11 MB for `lite` (BlazeFace + PaddleOCR), ~14 MB for `standard`/`full` (adds SCRFD). NER, NSFW, RI, and KWS models are an additional ~175 MB fetched separately via `git lfs pull` — they are not downloaded by this script.
>
> The script is re-runnable: files that already exist are skipped, so it is safe to interrupt and re-run. A file that was only partially downloaded before interruption will not be automatically re-fetched; delete it manually before re-running if a previous run was cut short.

```bash
./build/download_models.sh
```

> **You should see** the following directories created under `openobscure-proxy/models/`:
> ```
> models/blazeface/          — all tiers      (BlazeFace face detection, ~408 KB)
> models/paddleocr/          — all tiers      (PaddleOCR det + rec + dict, ~10 MB)
> models/scrfd/              — standard, full (SCRFD-2.5GF face detection, ~3.1 MB)
> ```
> The remaining subdirectories are populated by `git lfs pull`, not this script:
> ```
> models/ner/                — full tier      (DistilBERT INT8, ~64 MB)
> models/ner-lite/           — lite, standard (TinyBERT INT8, ~14 MB)
> models/nsfw_classifier/    — all tiers      (ViT-base 5-class INT8, ~83 MB)
> models/ri/                 — full tier      (R2 response integrity, ~14 MB)
> models/kws/                — full + voice   (Zipformer KWS, ~5 MB)
> ```
> When bundling for mobile (`./build/bundle_models.sh`), `paddleocr/` is renamed to `ocr/` in the app bundle. Set `models_base_dir` in your config JSON to the directory containing these subdirectories.

**To also fetch NER, NSFW, RI, and KWS models (~175 MB total):**

```bash
git lfs pull
```

> **You should see** progress for each tracked file:
> ```
> Downloading LFS objects: 100% (5/5), 175 MB | 8 MB/s
> ```
> If Git LFS is not installed, run `git lfs install` first then re-run `git lfs pull`. If the remote does not serve LFS objects, the pointer files will remain on disk and model loading will fall back silently to the lite tier — check `getDebugLog()` after init to confirm which models loaded.

Bundle the `models/` directory into your app's resources (iOS) or assets (Android). Set `models_base_dir` in config to point to the directory — the library auto-resolves model paths from standard subdirectories (`ner/` or `ner-lite/`, `blazeface/` or `scrfd/`, `ocr/`, `nsfw_classifier/`, `ri/`, `kws/`).

## 4. Integrate

### Swift (iOS / macOS)

```swift
import openobscure_proxy

// Replace with output of: openssl rand -hex 32
// Store in iOS Keychain — never hard-code in source.
// createOpenobscure() throws if the key is not exactly 64 hexadecimal characters.
let fpeKey = KeychainHelper.load(key: "openobscure-fpe-key")!

// Point to bundled models — auto-detects device tier and loads accordingly
let modelsDir = Bundle.main.resourcePath! + "/models"
let config = """
{"scanner_mode": "auto", "models_base_dir": "\(modelsDir)"}
"""

let handle = try createOpenobscure(configJson: config, fpeKeyHex: fpeKey)

// Sanitize user input before sending to LLM — real PII is encrypted in-place.
// See Step 4b for a test call with a known value to verify detection is active.
let userMessage = "..." // your actual user input
let result = try sanitizeText(handle: handle, text: userMessage)

// Save result.mappingJson — you'll need it to restore

// After LLM responds, restore original PII
let restored = try restoreText(
    handle: handle,
    text: llmResponse,
    mappingJson: result.mappingJson
)
```

### Kotlin (Android)

```kotlin
import uniffi.openobscure_proxy.*

// Replace with output of: openssl rand -hex 32
// Store in Android Keystore — never hard-code in source.
// createOpenobscure() throws if the key is not exactly 64 hexadecimal characters.
val fpeKey = keystoreHelper.load("openobscure-fpe-key")

// createOpenobscure() requires a real filesystem path — APK asset URIs are not accepted.
// Copy the bundled models/ tree to internal storage on first launch; skip if already present.
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

// Sanitize user input before sending to LLM — real PII is encrypted in-place.
// See Step 4b for a test call with a known value to verify detection is active.
val userMessage = "..." // your actual user input
val result = sanitizeText(handle = handle, text = userMessage)

// Restore after LLM responds
val restored = restoreText(
    handle = handle,
    text = llmResponse,
    mappingJson = result.mappingJson
)
```

## 4b. Verify your first detection

Call `sanitizeText()` with a known PII value and inspect the result before wiring the full LLM flow.

### Swift

```swift
let result = try sanitizeText(handle: handle, text: "My SSN is 123-45-6789")
print(result.sanitizedText)
```

Expected output (the encrypted value will differ — FF1 output depends on your key):

```
My SSN is 847-29-3156
```

### Kotlin

```kotlin
val result = sanitizeText(handle = handle, text = "My card is 4111-1111-1111-1111")
println(result.sanitizedText)
```

Expected output (the encrypted value will differ — FF1 output depends on your key):

```
My card is 7392-8841-5503-2947
```

> **If `sanitizedText` equals your input unchanged**, PII detection did not fire. Confirm that Step 3 completed successfully and that `models_base_dir` in your config JSON points to the directory containing the `ner/`, `nsfw_classifier/`, `ri/`, and `kws/` subdirectories. Regex-based types (SSN, credit card) do not require models — if even those are not redacted, the handle was likely initialized with an empty or invalid config.

## 5. Verify

```swift
// Check device tier and active features
let stats = getStats(handle: handle)
print(stats.deviceTier)  // "full", "standard", or "lite"
print(stats.piiMatchesTotal)
```

Expected output on a modern device with all models loaded:

```
full
0
```

On a device where models are missing or `models_base_dir` is not set, the tier will be lower:

```
lite     // or "standard" — NER, NSFW, RI, and/or KWS layers inactive
0
```

> **If `deviceTier` is `"lite"` and you expected `"full"`:** verify that `models_base_dir` in your config JSON points to the correct directory and that all required subdirectories (`ner/`, `nsfw_classifier/`, `ri/`, `kws/`) are populated. Run `getDebugLog(handle: handle)` to see which model paths were attempted and which failed to load.

---

## Full API

| Function | Purpose |
|----------|---------|
| `createOpenobscure(configJson, fpeKeyHex)` | Initialize with config and FPE key |
| `sanitizeText(handle, text)` | Scan + encrypt PII, return sanitized text + mapping |
| `restoreText(handle, text, mappingJson)` | Decrypt FPE values using saved mapping |
| `sanitizeImage(handle, imageBytes)` | EXIF strip (always) + face/OCR/NSFW redaction (model-dependent) |
| `sanitizeAudioTranscript(handle, transcript)` | Scan speech transcript for PII |
| `checkAudioPii(handle, transcript)` | Quick PII count (no encryption) |
| `rotateKey(handle, newKeyHex)` | Rotate FPE key with 30-second overlap |
| `scanResponse(handle, text)` | Scan LLM response for manipulation |
| `getStats(handle)` | PII counts, scanner mode, image pipeline status, device tier |
| `getDebugLog(handle)` | Retrieve buffered log entries for diagnostics |

---

## What Happens Under the Hood

1. Your app calls `sanitizeText()` with user input containing PII
2. OpenObscure detects PII using regex + keywords + NER (tier-dependent)
3. Each match is encrypted with **FF1 Format-Preserving Encryption** — ciphertext looks realistic so the LLM can still reason about data structure
4. Your app sends the sanitized text to the LLM directly (no proxy involved)
5. When the response arrives, `restoreText()` decrypts FPE values back to originals
6. Optionally, `scanResponse()` checks the response for persuasion techniques

Your real PII never leaves your device. Hardware auto-detection profiles the device at init and selects features accordingly — a phone with 8GB+ RAM gets the same detection as a desktop. See [Deployment Tiers](deployment-tiers.md) for details.

---

## Next Steps

- [Integration Guide](../integrate/embedding/INTEGRATION_GUIDE.md) — Xcode/Gradle project setup, tested integrations with Enchanted and RikkaHub
- [Deployment Tiers](deployment-tiers.md) — what each tier enables and how to override
- [System Overview](../architecture/system-overview.md) — full architecture
