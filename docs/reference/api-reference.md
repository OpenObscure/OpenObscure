# Embedded API Reference

Complete type and function reference for the OpenObscure embedded (mobile) API. These types are exposed to Swift and Kotlin via [UniFFI](https://mozilla.github.io/uniffi-rs/) bindings.

Source: [uniffi_bindings.rs](../../openobscure-proxy/src/uniffi_bindings.rs), [lib_mobile.rs](../../openobscure-proxy/src/lib_mobile.rs)

---

## Functions

### create_openobscure

Create a new OpenObscure instance.

| Parameter | Type | Description |
|-----------|------|-------------|
| `config_json` | `String` | JSON string with [MobileConfig](#mobileconfig) fields. Pass `"{}"` for defaults. |
| `fpe_key_hex` | `String` | 64-character hex string encoding the 32-byte FPE master key. |

**Returns:** `OpenObscureHandle` (opaque) — or `MobileBindingError` on failure.

```swift
// Swift
let handle = try createOpenobscure(configJson: "{}", fpeKeyHex: "aa".repeated(32))
```

```kotlin
// Kotlin
val handle = createOpenobscure(configJson = "{}", fpeKeyHex = "aa".repeat(32))
```

---

### sanitize_text

Scan text for PII and encrypt matches with FF1 FPE.

| Parameter | Type | Description |
|-----------|------|-------------|
| `handle` | `OpenObscureHandle` | Instance handle from `create_openobscure`. |
| `text` | `String` | Text to scan for PII. |

**Returns:** [SanitizeResultFFI](#sanitizeresultffi) — or `MobileBindingError` on failure.

**Tweak derivation:** Each PII match is encrypted with a per-match FF1 tweak derived from `TweakGenerator::generate(&request_uuid, "m:{byte_offset}")`. The same plaintext appearing twice in the same request at different positions produces different ciphertext, preventing frequency analysis. External FPE verifiers must supply both the request UUID and the byte offset of each match to reproduce the tweak.

**FPE fallback:** When FF1 encryption fails for a match (e.g., value is shorter than the type's minimum domain length), the library silently substitutes a deterministic hash token (e.g., `EMAIL_a7f2`) instead of encrypting. `pii_count` still increments. The `mapping_json` does not distinguish FPE-encrypted entries from hash-token entries — hash tokens are not reversible via `restore_text`. Inspect the sanitized text for `TYPE_XXXX` patterns to identify hash-token replacements.

```swift
let result = try sanitizeText(handle: handle, text: "Call me at 555-123-4567")
print(result.sanitizedText)   // PII encrypted
print(result.piiCount)        // 1
```

---

### restore_text

Restore original PII values in response text using saved mappings.

| Parameter | Type | Description |
|-----------|------|-------------|
| `handle` | `OpenObscureHandle` | Instance handle. |
| `text` | `String` | Response text containing FPE-encrypted PII. |
| `mapping_json` | `String` | Opaque mapping from a prior `sanitize_text` call. |

**Returns:** `String` — text with original PII restored.

```swift
let restored = restoreText(handle: handle, text: response, mappingJson: result.mappingJson)
```

---

### sanitize_image

Process an image for visual PII (face redaction, OCR text redaction, EXIF strip).

| Parameter | Type | Description |
|-----------|------|-------------|
| `handle` | `OpenObscureHandle` | Instance handle. |
| `image_bytes` | `Vec<u8>` / `Data` | Raw image bytes (JPEG, PNG, etc.). |

**Returns:** `Vec<u8>` / `Data` — sanitized image bytes (JPEG format) — or `MobileBindingError` on failure.

```swift
let sanitized = try sanitizeImage(handle: handle, imageBytes: originalData)
```

---

### sanitize_audio_transcript

Scan a speech-to-text transcript for PII and encrypt matches.

| Parameter | Type | Description |
|-----------|------|-------------|
| `handle` | `OpenObscureHandle` | Instance handle. |
| `transcript` | `String` | Transcript from platform speech API (iOS `SFSpeechRecognizer`, Android `SpeechRecognizer`). |

**Returns:** [SanitizeResultFFI](#sanitizeresultffi) — or `MobileBindingError` on failure.

```swift
let result = try sanitizeAudioTranscript(handle: handle, transcript: speechResult.bestTranscription)
```

---

### check_audio_pii

Check if a transcript contains PII without encrypting.

| Parameter | Type | Description |
|-----------|------|-------------|
| `handle` | `OpenObscureHandle` | Instance handle. |
| `transcript` | `String` | Transcript text to check. |

**Returns:** `u32` — count of individual PII token spans detected. Overlapping spans are deduplicated via union-find before counting, but a single entity (e.g., a full name) may produce multiple non-overlapping spans. This is a span count, not a count of distinct entity classes — use `categories` from a full `sanitize_audio_transcript` call if you need entity-class information.

```kotlin
val count = checkAudioPii(handle = handle, transcript = transcript)
if (count > 0) { /* strip this audio block */ }
```

---

### scan_response

Scan an LLM response for persuasion/manipulation techniques (cognitive firewall).

| Parameter | Type | Description |
|-----------|------|-------------|
| `handle` | `OpenObscureHandle` | Instance handle. |
| `response_text` | `String` | LLM response text to scan. |

**Returns:** [RiReportFFI](#rireportffi) if manipulation detected, `nil`/`null` if clean or disabled.

```swift
if let report = scanResponse(handle: handle, responseText: llmResponse) {
    print("Severity: \(report.severity)")  // "Notice", "Warning", or "Caution"
}
```

---

### get_stats

Get current statistics for diagnostics.

| Parameter | Type | Description |
|-----------|------|-------------|
| `handle` | `OpenObscureHandle` | Instance handle. |

**Returns:** [MobileStatsFFI](#mobilestatsffi)

```kotlin
val stats = getStats(handle = handle)
Log.d("OO", "PII found: ${stats.totalPiiFound}, tier: ${stats.deviceTier}")
```

---

### get_debug_log

Get buffered debug log messages from the Rust layer. Returns all accumulated messages since the last call, then clears the buffer.

No parameters.

**Returns:** `String` — newline-separated log messages.

```swift
let log = getDebugLog()
if !log.isEmpty { print("OpenObscure debug:\n\(log)") }
```

> **Rust-internal methods not available to Swift/Kotlin:** `rotate_key()`, `release_models()`, and `ri_available()` have doc comments in `lib_mobile.rs` but are not decorated with `#[uniffi::export]` and are not reachable from Swift/Kotlin callers. To rotate the FPE key from embedded code, create a new handle via `create_openobscure` with the new key.

---

## Types

### OpenObscureHandle

Opaque handle to an OpenObscure instance. Created by [create_openobscure](#create_openobscure), passed to all other functions.

Not inspectable from Swift/Kotlin — treat as an opaque reference. Thread-safe (internally `Arc`-wrapped).

---

### MobileConfig

Configuration passed as JSON to [create_openobscure](#create_openobscure). All fields are optional — defaults apply when omitted.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `scanner_mode` | `string` | `"auto"` | Scanner backend: `"auto"`, `"ner"`, `"crf"`, `"regex"`. Unknown values fall through to `"regex"`. |
| `auto_detect` | `bool` | `true` | Enable hardware RAM detection at init. When `true`, selects a `CapabilityTier`: Full (≥8GB), Standard (4–8GB), Lite (<4GB). The tier gates which models are loaded: Full uses DistilBERT + SCRFD + NSFW; Standard uses DistilBERT or TinyBERT + SCRFD; Lite uses TinyBERT + Ultra-Light face only. Set `false` to skip detection and rely solely on `scanner_mode`. See [Deployment Tiers](../get-started/deployment-tiers.md) for the full feature matrix. |
| `keywords_enabled` | `bool` | `true` | Enable health/child keyword dictionary |
| `gazetteer_enabled` | `bool` | `true` | Enable name gazetteer for person detection |
| `ner_model_dir` | `string?` | `null` | Path to DistilBERT NER model directory (Full/Standard tier) |
| `ner_model_dir_lite` | `string?` | `null` | Path to TinyBERT NER model directory (Lite tier). Falls back to `ner_model_dir`. |
| `ner_pool_size` | `int` | `1` | Concurrent NER model instances |
| `crf_model_dir` | `string?` | `null` | Path to CRF model directory |
| `image_enabled` | `bool` | `true` | Enable image pipeline (device budget gates actual activation) |
| `face_model_dir` | `string?` | `null` | Path to BlazeFace model directory (Lite tier) |
| `scrfd_model_dir` | `string?` | `null` | Path to SCRFD model directory (Full/Standard tier) |
| `ocr_model_dir` | `string?` | `null` | Path to PaddleOCR model directory |
| `nsfw_model_dir` | `string?` | `null` | Path to ViT-base NSFW classifier model directory |
| `max_dimension` | `int` | `960` | Resize longest image edge before processing (pixels) |
| `ri_enabled` | `bool` | `true` | Enable cognitive firewall (device budget gates actual activation) |
| `ri_sensitivity` | `string` | `"medium"` | Sensitivity: `"off"`, `"low"`, `"medium"`, `"high"`. The field is parsed as an enum internally; unknown values silently default to `"medium"`. |
| `ri_model_dir` | `string?` | `null` | Path to R2 TinyBERT model directory (optional; R1 works without it) |
| `models_base_dir` | `string?` | `null` | Base directory for model subdirectories. Auto-resolves `ner/`, `ner_lite/`, `crf/`, `scrfd/`, `blazeface/`, `ocr/`, `nsfw/`, `ri/`. Individual `*_model_dir` fields override. |
| `ort_dylib_path` | `string?` | `null` | Path to `libonnxruntime.so` (Android only; ignored on iOS). If unset on Android, ORT models (NER, image pipeline, cognitive firewall) are unavailable and the library silently degrades to regex-only. No error is returned — check `get_debug_log()` for `"ort_dylib_path not set"` to detect this condition. |
| `enabled_languages` | `[string]` | `[]` | ISO 639-1 language codes for the multilingual scan pass (e.g., `["es", "fr", "de"]`). Empty list (default) activates all 8 supported non-English languages. |
| `nsfw_classifier_model_dir` | `string?` | `null` | **Legacy field — silently ignored.** Use `nsfw_model_dir` instead. No removal date is set; the field will remain accepted without error until a future major version removes it. |

```json
{
  "models_base_dir": "/app/models",
  "scanner_mode": "auto",
  "ri_sensitivity": "medium"
}
```

---

### SanitizeResultFFI

Returned by [sanitize_text](#sanitize_text) and [sanitize_audio_transcript](#sanitize_audio_transcript).

| Field | Type | Description |
|-------|------|-------------|
| `sanitized_text` | `String` | Text with PII replaced by FPE-encrypted or redacted values |
| `pii_count` | `u32` / `UInt32` | Count of individual detected token spans. Overlapping spans are deduplicated, but a single entity may produce multiple spans. Not a count of distinct entity classes. |
| `categories` | `[String]` | PII categories found (see [PII Category Strings](#pii-category-strings)) |
| `mapping_json` | `String` | Opaque mapping data — pass to [restore_text](#restore_text) to decrypt. Does not distinguish FPE-encrypted entries from hash-token fallback entries; hash tokens are not reversible. |

```swift
let result = try sanitizeText(handle: handle, text: "Email: alice@example.com")
// result.sanitizedText  → "Email: xyzqr@example.com"
// result.piiCount       → 1
// result.categories     → ["email"]
// result.mappingJson    → "{...}"  (opaque — do not parse)
```

---

### RiReportFFI

Returned by [scan_response](#scan_response) when manipulation is detected.

| Field | Type | Description |
|-------|------|-------------|
| `severity` | `String` | `"Notice"` (1 category matched), `"Warning"` (2–3 categories or ≥3 matched phrases), `"Caution"` (4+ categories). Underlying type is a `SeverityTier` enum serialized to string; no Swift/Kotlin enum typedef is generated — callers must use string comparison. |
| `categories` | `[String]` | R1 persuasion categories: `"Urgency"`, `"Scarcity"`, `"Social Proof"`, `"Fear"`, `"Authority"`, `"Commercial"`, `"Flattery"` |
| `flags` | `[String]` | Matched phrases from R1 dictionary scan |
| `r2_categories` | `[String]` | EU AI Act Article 5 categories from R2 classifier (empty if R2 model not loaded) |
| `scan_time_us` | `u64` / `UInt64` | Scan duration in **microseconds** |

```kotlin
val report = scanResponse(handle = handle, responseText = response)
if (report != null) {
    Log.w("RI", "${report.severity}: ${report.categories}")
    // "Warning: [Urgency, Fear]"
}
```

---

### MobileStatsFFI

Returned by [get_stats](#get_stats).

| Field | Type | Description |
|-------|------|-------------|
| `total_pii_found` | `u64` / `UInt64` | Total PII matches found across all calls |
| `total_images_processed` | `u64` / `UInt64` | Total images processed |
| `scanner_mode` | `String` | Active scanner mode: `"regex"`, `"crf"`, or `"ner"`. **Edge case:** when `auto_detect = false` and `scanner_mode = "auto"`, this field returns `"auto"` even though regex is in effect. Use an explicit `"regex"` in config to get an unambiguous value. |
| `image_pipeline_available` | `bool` / `Bool` | Whether image pipeline is loaded |
| `device_tier` | `String` | Detected tier: `"full"`, `"standard"`, or `"lite"`. |

```swift
let stats = getStats(handle: handle)
// stats.deviceTier             → "standard"
// stats.scannerMode            → "ner"
// stats.imagePipelineAvailable → true
```

---

### MobileBindingError

Error type thrown by all fallible functions.

| Variant | Description |
|---------|-------------|
| `Config(String)` | Invalid configuration JSON |
| `InvalidKey(String)` | FPE key is not valid hex or not 32 bytes |
| `Init(String)` | Initialization failed. Triggered by: ORT dynamic library not found (Android, when `ort_dylib_path` is unset or path is wrong); NER model directory missing or unreadable; image pipeline ONNX model corrupt or incompatible. **Note:** on Android, if `ort_dylib_path` is unset, the library does **not** return `Init` — it silently degrades to regex-only and continues. Check `get_debug_log()` for `"ort_dylib_path not set"`. |
| `Processing(String)` | Runtime processing error. May wrap an FPE error — see [FPE Error Conditions](#fpe-error-conditions). |

```swift
do {
    let handle = try createOpenobscure(configJson: "{}", fpeKeyHex: "bad")
} catch MobileBindingException.InvalidKey(let msg) {
    print("Key error: \(msg)")  // "Bad hex: ..." or "Expected 32 bytes, got ..."
}
```

```kotlin
try {
    val handle = createOpenobscure(configJson = "{}", fpeKeyHex = "bad")
} catch (e: MobileBindingException.InvalidKey) {
    Log.e("OO", "Key error: ${e.message}")
}
```

---

## PII Category Strings

String values that appear in `SanitizeResultFFI.categories`:

| String | Detection Source | Description |
|--------|-----------------|-------------|
| `"credit_card"` | Regex (Luhn) | Credit card number |
| `"ssn"` | Regex (range) | US Social Security Number |
| `"phone"` | Regex | Phone number |
| `"email"` | Regex | Email address |
| `"api_key"` | Regex (prefix) | API key (`sk-`, `AKIA`, `ghp_`, etc.) |
| `"ipv4_address"` | Regex | IPv4 address |
| `"ipv6_address"` | Regex | IPv6 address |
| `"gps_coordinate"` | Regex | GPS latitude/longitude |
| `"mac_address"` | Regex | MAC address |
| `"iban"` | Regex (country) | International Bank Account Number |
| `"health_keyword"` | Keywords | Health-related term (~350 terms, 9 languages) |
| `"child_keyword"` | Keywords | Child-related term (~350 terms, 9 languages) |
| `"person"` | NER / CRF | Person name |
| `"location"` | NER / CRF | Location name |
| `"organization"` | NER / CRF | Organization name |

---

## FPE Error Conditions

When `MobileBindingError::Processing` wraps an FPE failure (surfaced during `sanitize_text` or `sanitize_audio_transcript`), the underlying causes are:

| Condition | Cause | Behavior |
|-----------|-------|----------|
| `DomainTooSmall` | Value is shorter than the type's minimum length (`radix^len < 100`). Common for very short email local parts or 2-char IPv6 segments. | Silent fallback to hash token. `pii_count` still increments. No error returned. |
| `InvalidCharacter` | Input character is outside the type's alphabet (e.g., a non-hex digit in a MAC address field). | Fail-open: match skipped, original plaintext forwarded. |
| `InvalidNumeral` | Decrypted numeral maps to an invalid character during restoration. | Restoration silently returns unchanged ciphertext. |
| `NumeralString` | FF1 internal error during encryption or decryption. | Fail-open: original plaintext forwarded. |
| `InvalidRadix` | Radix is not supported by the FF1 implementation. | Match skipped. |
| `UnsupportedType` | PII type has no FPE alphabet mapping. | Match skipped. |

Hash-token substitutions (for `DomainTooSmall`) appear as `TYPE_XXXX` in sanitized text (e.g., `EMAIL_a7f2`, `PHONE_1b3c`). They are not reversible by `restore_text`.

---

## FF1 Radix and Domain Constraints

FF1 requires `radix^min_length ≥ 1,000,000` (NIST SP 800-38G §5.2). Values shorter than `min_length` for their type trigger a `DomainTooSmall` fallback (see [FPE Error Conditions](#fpe-error-conditions)).

| PII Type | Radix | Alphabet | Min length | Domain size |
|----------|-------|----------|------------|-------------|
| Credit card | 10 | `0-9` | 15 | 10¹⁵ |
| SSN | 10 | `0-9` | 9 | 10⁹ |
| Phone | 10 | `0-9` | 10 | 10¹⁰ |
| Email local part | 36 | `0-9a-z` | 4 | 36⁴ = 1.68M |
| API key | 62 | `0-9A-Za-z` | 6 | 62⁶ = 56.8B |
| IPv4 address | 10 | `0-9` | 4 | 10⁴ = 10,000 ¹ |
| IPv6 address | 16 | `0-9a-f` | 2 | 16² = 256 ¹ |
| GPS coordinate | 10 | `0-9` | 6 | 10⁶ = 1M |
| MAC address | 16 | `0-9a-f` | 6 | 16⁶ = 16.7M |
| IBAN (non-country) | 36 | `0-9a-z` | 6 | 36⁶ = 2.18B |

> ¹ IPv4 and IPv6 have segments below the 1,000,000 threshold. These types use a relaxed minimum (≥100) because the full address is encrypted per-segment with separators preserved, not as a single numeral string. Individual short segments (e.g., a 2-char IPv6 group) may still trigger `DomainTooSmall`.
