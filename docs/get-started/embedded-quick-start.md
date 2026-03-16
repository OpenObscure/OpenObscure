# Embedded Quick Start

Sanitize PII directly in your iOS, macOS, or Android app — no proxy, no HTTP server. Five steps.

---

## Prerequisites

| Tool | Minimum version | Required for |
|------|----------------|--------------|
| Rust | **1.75** | All targets |
| Xcode | 15+ | iOS / macOS builds |
| Android NDK | 27+ | Android builds |
| cargo-ndk | any | Android builds (`cargo install cargo-ndk`) |
| Git LFS | any | NER, NSFW, KWS, RI model files |

ONNX Runtime is auto-downloaded at build time — no manual installation required.

---

## 1. Build the library

```bash
# macOS
cargo build --manifest-path openobscure-core/Cargo.toml \
  --lib --no-default-features --features mobile --release

# iOS (produces XCFramework)
./build/build_ios.sh --release --xcframework

# Android
./build/build_android.sh --release
```

## 2. Generate UniFFI bindings

```bash
./build/generate_bindings.sh --swift-only    # iOS / macOS
./build/generate_bindings.sh --kotlin-only   # Android
```

## 3. Download models

```bash
./build/download_models.sh   # BlazeFace, SCRFD, PaddleOCR (~14 MB)
git lfs pull                 # NER, NSFW, RI, KWS (~175 MB)
```

## 4. Initialize and call

**Swift:**
```swift
let handle = try createOpenobscure(
    configJson: #"{"scanner_mode":"auto","models_base_dir":"\(Bundle.main.resourcePath!)/models"}"#,
    fpeKeyHex: KeychainHelper.load(key: "openobscure-fpe-key")!
)
let result = try sanitizeText(handle: handle, text: userInput)
// Send result.sanitizedText to LLM — real PII is FPE-encrypted
let restored = try restoreText(handle: handle, text: llmResponse, mappingJson: result.mappingJson)
```

**Kotlin:**
```kotlin
val handle = createOpenobscure(
    configJson = """{"scanner_mode":"auto","models_base_dir":"$modelsDir"}""",
    fpeKeyHex = keystoreHelper.load("openobscure-fpe-key")
)
val result = sanitizeText(handle = handle, text = userInput)
val restored = restoreText(handle = handle, text = llmResponse, mappingJson = result.mappingJson)
```

> **FPE key:** generate with `openssl rand -hex 32`. Store in iOS Keychain or Android Keystore — never hard-code in source.

## 5. Verify

```swift
let result = try sanitizeText(handle: handle, text: "My SSN is 123-45-6789")
print(result.sanitizedText)  // My SSN is 847-29-3156  (FF1-encrypted, differs per key)
```

---

## How It Works

1. `sanitizeText()` detects PII using regex + NER ensemble
2. Each match is encrypted with FF1 Format-Preserving Encryption — ciphertext looks realistic so the LLM can still reason about data structure
3. Your app sends the sanitized text to the LLM directly (no proxy)
4. `restoreText()` decrypts FPE values in the LLM response before showing it to the user
5. Real PII never leaves the device

---

## Next Steps

- [Embedded Setup Guide](../../setup/embedded_setup.md) — complete walkthrough with troubleshooting (platform prerequisites, build details, model loading, verification)
- [Integration Guide](../integrate/embedding/INTEGRATION_GUIDE.md) — Xcode SPM setup, Gradle + JNA + ProGuard, OkHttp interceptor, worked diffs from Enchanted and RikkaHub
- [API Reference](../reference/api-reference.md) — full function list, types, error conditions
