# OpenObscure Embedded (Mobile) Test Guide

> Hands-on walkthrough of the Embedded/Mobile library.
> Demonstrates the direct Rust API and UniFFI bindings for Swift and Kotlin.

---

## Prerequisites

- Rust toolchain (1.75+)
- iOS: `rustup target add aarch64-apple-ios aarch64-apple-ios-sim`, Xcode 15+
- Android: `cargo install cargo-ndk`, `rustup target add aarch64-linux-android`, Android NDK

## Build

```bash
# iOS (static library + XCFramework)
./build/build_ios.sh --release --xcframework

# Android (shared library, all ABIs)
./build/build_android.sh --release --all-abis

# Generate UniFFI bindings (Swift + Kotlin)
./build/generate_bindings.sh
```

## Feature Flags

| Flag | What it enables |
|------|----------------|
| `mobile` | Core UniFFI exports (sanitize, restore, image, stats) |

```bash
# Build with mobile feature
cargo build --manifest-path openobscure-core/Cargo.toml \
  --lib --features mobile --release
```

---

## 1. Text PII Sanitization & Restore

```rust
use openobscure_core::lib_mobile::{MobileConfig, OpenObscureMobile};

// In production: load key from iOS Keychain / Android Keystore
let fpe_key = [0x42u8; 32];

// auto_detect: true (default) — detects device hardware and selects features
// A phone with 4GB+ RAM gets NER + ensemble + image pipeline (Full tier)
let mobile = OpenObscureMobile::new(MobileConfig::default(), fpe_key).unwrap();

// Check what tier the device was classified as
let stats = mobile.stats();
println!("Device tier: {}", stats.device_tier);
// Output: "full" (≥4GB RAM), "standard" (2-4GB), or "lite" (<2GB)

// Sanitize outbound text — scan for PII and encrypt matches
let result = mobile.sanitize_text(
    "My card is 4111-1111-1111-1111 and SSN 123-45-6789"
).unwrap();

println!("Sanitized: {}", result.sanitized_text);
// Output: "My card is 4732-8294-5617-3048 and SSN 123-45-6678"
// (FPE-encrypted — same format, different digits)

println!("PII count: {}", result.pii_count);
// Output: 2

println!("Categories: {:?}", result.categories);
// Output: ["credit_card", "ssn"]

// Save mapping_json alongside the outbound request
let mapping = result.mapping_json.clone();

// Later: restore original PII in the response
let response = "Your card ending in 3048 is valid.";
let restored = mobile.restore_text(response, &mapping);
println!("Restored: {}", restored);
// Any FPE-encrypted values in the response are replaced with originals
```

**SanitizeResult fields:**

| Field | Type | Description |
|-------|------|-------------|
| `sanitized_text` | `String` | Text with PII encrypted/redacted |
| `pii_count` | `u32` | Number of PII matches found |
| `categories` | `Vec<String>` | PII types detected (e.g. `"credit_card"`, `"ssn"`, `"email"`) |
| `mapping_json` | `String` | Opaque mapping data — pass to `restore_text()` |

---

## 2. Keyword Detection

Health and child-related keywords are detected alongside regex PII.

```rust
let result = mobile.sanitize_text(
    "The patient has diabetes and takes metformin. The child is 3 years old."
).unwrap();

println!("Categories: {:?}", result.categories);
// Includes "health_keyword" and/or "child_keyword"
// Keywords are label-redacted: [health_keyword], [child_keyword]
```

Enable/disable via config:

```rust
let config = MobileConfig {
    keywords_enabled: true,  // default: true
    ..MobileConfig::default()
};
```

---

## 3. Hardware Auto-Detection & NER on Mobile

By default, `auto_detect: true` detects device RAM and CPU cores at initialization. On a device with ≥4GB RAM, NER (DistilBERT INT8 on Full, TinyBERT INT8 on Standard) and ensemble voting are automatically enabled — matching gateway-level PII detection efficacy.

```rust
// Default: auto-detect hardware, select features by tier
let config = MobileConfig::default();
// auto_detect: true, scanner_mode: "auto"

// To use NER on mobile, provide the model directory path
let config = MobileConfig {
    ner_model_dir: Some("/path/to/ner_model".to_string()),
    ..MobileConfig::default()
};
let mobile = OpenObscureMobile::new(config, fpe_key).unwrap();

// On a 12GB phone: Full tier → NER + CRF + ensemble + image pipeline
// On a 6GB phone: Standard tier → NER + CRF + image pipeline
// On a 3GB phone: Lite tier → CRF + regex (no NER)
```

To disable auto-detection and force a specific scanner:

```rust
let config = MobileConfig {
    auto_detect: false,
    scanner_mode: "crf".to_string(),  // or "ner", "regex"
    ..MobileConfig::default()
};
```

### Capability Tiers

| Device RAM | Tier | Scanners | Image Pipeline |
|------------|------|----------|----------------|
| ≥4GB | **Full** | NER + CRF + ensemble | Yes |
| 2–4GB | **Standard** | NER + CRF | Yes |
| <2GB | **Lite** | CRF + regex | Yes (if budget allows) |

---

## 4. Image Sanitization

Process images for visual PII: face redaction, OCR text redaction, EXIF metadata strip.

```rust
let config = MobileConfig {
    image_enabled: true,
    face_model_dir: Some("/path/to/scrfd".to_string()),
    ocr_model_dir: Some("/path/to/paddleocr".to_string()),
    max_dimension: 960,  // resize if larger
    ..MobileConfig::default()
};
let mobile = OpenObscureMobile::new(config, fpe_key).unwrap();

let photo_bytes = std::fs::read("test_photo.jpg").unwrap();
let sanitized = mobile.sanitize_image(&photo_bytes).unwrap();
std::fs::write("sanitized_photo.jpg", &sanitized).unwrap();
// Faces solid-filled, text regions solid-filled, EXIF stripped, output as JPEG
```

> **Note:** Image pipeline is optional on mobile. Set `image_enabled: true` and provide model directory paths. Without models, `sanitize_image()` returns `MobileError::ImageError`.

---

## 5. UniFFI Bindings (Swift & Kotlin)

UniFFI auto-generates idiomatic wrappers from the Rust API. All functions below are available after running `build/generate_bindings.sh`.

### Swift (iOS)

```swift
import OpenObscure

let handle = try createOpenobscure(
    configJson: #"{"keywords_enabled": true, "scanner_mode": "auto"}"#,
    fpeKeyHex: String(repeating: "42", count: 32)  // 64 hex chars = 32 bytes
)

let result = try sanitizeText(handle: handle, text: "Card 4111-1111-1111-1111")
print("Sanitized: \(result.sanitizedText)")
print("PII count: \(result.piiCount)")
print("Categories: \(result.categories)")

let restored = restoreText(
    handle: handle,
    text: result.sanitizedText,
    mappingJson: result.mappingJson
)

let stats = getStats(handle: handle)
print("Total PII found: \(stats.totalPiiFound)")
```

### Kotlin (Android)

```kotlin
import com.openobscure.proxy.*

val handle = createOpenobscure(
    configJson = """{"keywords_enabled": true, "scanner_mode": "auto"}""",
    fpeKeyHex = "42".repeat(32)
)

val result = sanitizeText(handle, "Card 4111-1111-1111-1111")
println("Sanitized: ${result.sanitizedText}")
println("PII count: ${result.piiCount}")
println("Categories: ${result.categories}")

val restored = restoreText(handle, result.sanitizedText, result.mappingJson)

val stats = getStats(handle)
println("Total PII found: ${stats.totalPiiFound}")
```

### UniFFI Record Types Reference

| Record | Fields |
|--------|--------|
| `SanitizeResultFFI` | `sanitized_text`, `pii_count`, `categories`, `mapping_json` |
| `MobileStatsFFI` | `total_pii_found`, `total_images_processed`, `scanner_mode`, `image_pipeline_available`, `device_tier` |

### Error Type

```
MobileBindingError
  ├── Config(String)       — Invalid JSON config
  ├── InvalidKey(String)   — Bad hex key or wrong length
  ├── Init(String)         — Initialization failed
  └── Processing(String)   — Runtime error
```

---

## Feature Parity

> For the full feature comparison across Gateway, L0 Embedded, and L1 Plugin, see [GATEWAY_TEST.md — Feature Parity](GATEWAY_TEST.md#feature-parity).

The following feature is **L0 Embedded-only**:

| Feature | Notes |
|---------|-------|
| UniFFI bindings (Swift/Kotlin) | Mobile-only FFI layer; not available in Gateway or L1 Plugin |

For NAPI addon (L1 Plugin upgrade for Node.js agents), see [TESTING_GUIDE.md — NAPI](TESTING_GUIDE.md).
