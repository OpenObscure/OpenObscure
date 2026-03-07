# Embedding OpenObscure in Third-Party Apps

A step-by-step guide for integrating OpenObscure as a **native library** (embedded model) into iOS, Android, and macOS chat applications. This covers both first-party test apps and third-party apps like [Enchanted](https://github.com/AugustDev/enchanted) (iOS/macOS) and [RikkaHub](https://github.com/rikkahub/rikkahub) (Android).

> **Proxy vs Embedded:** The [gateway setup guide](../setup/gateway_setup.md) covers the **gateway/proxy** model where OpenObscure runs as a sidecar HTTP proxy. This guide covers the **embedded** model where OpenObscure is compiled into your app as a native library. See [embedded setup](../setup/embedded_setup.md) for build prerequisites.

---

## Prerequisites

| Tool | iOS/macOS | Android |
|------|-----------|---------|
| Rust toolchain | `rustup` (stable) | `rustup` (stable) |
| Platform targets | `rustup target add aarch64-apple-ios aarch64-apple-ios-sim` | `rustup target add aarch64-linux-android x86_64-linux-android` |
| Build tool | Xcode 15+ with iOS SDK | `cargo install cargo-ndk` + Android NDK |
| Bindings | `./build/generate_bindings.sh --swift-only` | `./build/generate_bindings.sh --kotlin-only` |

---

## Part 1: Build the Native Library

### iOS (device + simulator)

```bash
cd /path/to/OpenObscure

# Build static libraries for iOS device and simulator
./build/build_ios.sh --release

# Optionally create XCFramework (recommended for distribution)
./build/build_ios.sh --release --xcframework
```

**Output:**
- `openobscure-proxy/target/aarch64-apple-ios/release/libopenobscure_proxy.a` (device)
- `openobscure-proxy/target/aarch64-apple-ios-sim/release/libopenobscure_proxy.a` (simulator)
- `openobscure-proxy/target/OpenObscure.xcframework` (if `--xcframework`)

### macOS (for macOS-native apps like Enchanted)

```bash
# Build for macOS (Apple Silicon)
cargo build --manifest-path openobscure-proxy/Cargo.toml \
  --lib --no-default-features --features mobile --release
```

**Output:**
- `openobscure-proxy/target/release/libopenobscure_proxy.a` (static, ~158MB)
- `openobscure-proxy/target/release/libopenobscure_proxy.dylib` (dynamic, ~19MB)

### Android (ARM64 + x86_64)

```bash
# Build for Android (requires cargo-ndk + NDK)
./build/build_android.sh --release --all-abis
```

**Output:**
- `openobscure-proxy/target/aarch64-linux-android/release/libopenobscure_proxy.so` (arm64-v8a)
- `openobscure-proxy/target/x86_64-linux-android/release/libopenobscure_proxy.so` (x86_64)

---

## Part 2: Generate Bindings

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

## Part 3: API Reference (All Platforms)

The embedded API is identical across Swift, Kotlin, and Rust:

| Function | Signature | Description |
|----------|-----------|-------------|
| `createOpenobscure` | `(configJson: String, fpeKeyHex: String) -> OpenObscureHandle` | Initialize with config JSON and 32-byte FPE key (64 hex chars) |
| `sanitizeText` | `(handle, text) -> SanitizeResult` | Scan text for PII, return sanitized text + mapping JSON |
| `restoreText` | `(handle, text, mappingJson) -> String` | Decrypt FPE tokens in LLM response using saved mapping |
| `sanitizeImage` | `(handle, imageBytes) -> Data` | Face redaction + OCR redaction on image bytes (JPEG/PNG) |
| `sanitizeAudioTranscript` | `(handle, transcript) -> SanitizeResult` | Scan speech transcript for PII |
| `checkAudioPii` | `(handle, transcript) -> Int` | Quick PII count check (no encryption) |
| `scanResponse` | `(handle, responseText) -> RiReportFFI?` | Scan LLM response for persuasion/manipulation (cognitive firewall) |
| `getStats` | `(handle) -> MobileStats` | Device tier, total PII found, image count |

### SanitizeResult

```
sanitizedText: String    — Text with PII replaced by FPE ciphertexts
mappingJson: String      — JSON mapping for restore (save per-request)
piiCount: UInt32         — Number of PII items found
categories: [String]     — PII types found ("credit_card", "ssn", "email", etc.)
```

### RiReportFFI (Response Integrity)

```
severity: String          — "Notice", "Warning", or "Caution"
categories: [String]      — Persuasion categories detected (Urgency, Authority, Scarcity, etc.)
flags: [String]           — Matched phrases from R1 dictionary scan
r2Categories: [String]    — EU AI Act Article 5 categories from R2 classifier (if model loaded)
scanTimeUs: UInt64        — Scan duration in microseconds
```

Returns `nil`/`null` when no manipulation is detected, RI is disabled, or device is Lite tier.

### MobileConfig (JSON)

```json
{
  "scanner_mode": "regex",
  "auto_detect": true,
  "keywords_enabled": true,
  "image_enabled": false,
  "ri_enabled": false,
  "ri_sensitivity": "medium",
  "ri_model_dir": null
}
```

- `scanner_mode`: `"auto"` (default, uses device tier), `"regex"`, `"crf"`, `"ner"`
- `auto_detect`: `true` (default) — profiles device RAM for tier selection
- `keywords_enabled`: `true` (default) — health/child keyword dictionary
- `image_enabled`: `false` (default) — requires ONNX model files
- `ri_enabled`: `false` (default) — enable cognitive firewall (response integrity scanning)
- `ri_sensitivity`: `"medium"` (default) — `"off"`, `"low"`, `"medium"`, `"high"` — controls R2 classifier invocation threshold
- `ri_model_dir`: `null` (default) — path to R2 model directory; R1 dictionary works without it

### PII Types (Regex-Only Mode — No Models Required)

Credit card (Luhn), SSN (range-validated), phone, email, API key, IPv4, IPv6, GPS coordinates, MAC address, IBAN (22 countries with check-digit validation), health keywords (~350 terms), child-related keywords (~350 terms).

**12 of 15 types work without any model files.** Person names, locations, and organizations require the NER TinyBERT model (~14MB).

---

## Part 4: iOS/macOS Integration (Swift)

### 4a. Add to Xcode Project (Local SPM Package)

Create a local Swift package alongside the app project. This approach was verified with Enchanted (macOS BUILD SUCCEEDED).

> **Important:** The C target name **must** match the `module` name in the `.modulemap` file. UniFFI generates `module openobscure_proxyFFI`, so the target must be named `openobscure_proxyFFI`.

1. **Create the local package directory:**

```
OpenObscureLib/
├── Package.swift
├── Sources/
│   ├── COpenObscure/
│   │   └── include/
│   │       ├── openobscure_proxyFFI.h         ← from bindings/swift/
│   │       └── openobscure_proxyFFI.modulemap ← from bindings/swift/
│   └── OpenObscureLib/
│       └── openobscure_proxy.swift            ← from bindings/swift/
└── lib/
    └── libopenobscure_proxy.a                 ← from build output
```

2. **Package.swift:**

```swift
// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "OpenObscureLib",
    platforms: [.iOS(.v17), .macOS(.v14)],
    products: [
        .library(name: "OpenObscureLib", targets: ["OpenObscureLib"]),
    ],
    targets: [
        .target(
            name: "openobscure_proxyFFI",  // MUST match modulemap module name
            path: "Sources/COpenObscure",
            publicHeadersPath: "include"
        ),
        .target(
            name: "OpenObscureLib",
            dependencies: ["openobscure_proxyFFI"],
            path: "Sources/OpenObscureLib",
            linkerSettings: [
                .unsafeFlags(["-L\(Context.packageDirectory)/../lib"]),
                .linkedLibrary("openobscure_proxy"),
                .linkedLibrary("resolv"),
                .linkedFramework("Security"),
                .linkedFramework("SystemConfiguration"),
            ]
        ),
    ]
)
```

3. **Add to Xcode project:**
   - File > Add Package Dependencies > Add Local > select `OpenObscureLib/`
   - Add `OpenObscureLib` framework to your target's "Frameworks, Libraries, and Embedded Content"
   - `import OpenObscureLib` in Swift files that use OpenObscure

4. **For Xcode projects without SPM**, add these Build Settings:
   - Other Linker Flags: `-lopenobscure_proxy -lresolv`
   - Library Search Paths: `$(PROJECT_DIR)/lib`
   - Header Search Paths: `$(PROJECT_DIR)/COpenObscure/include`
   - Linked Frameworks: Security, SystemConfiguration

### 4b. Initialize OpenObscure

Create a singleton manager:

```swift
import Foundation

final class OpenObscureManager {
    static let shared = OpenObscureManager()
    let handle: OpenObscureHandle
    var lastMappingJson: String = "{}"

    private init() {
        // Generate or load a 32-byte key (64 hex chars)
        // In production: store in iOS Keychain
        let key = OpenObscureManager.getOrCreateKey()
        handle = try! createOpenobscure(
            configJson: #"{"scanner_mode": "regex"}"#,
            fpeKeyHex: key
        )
    }

    func sanitize(_ text: String) -> (sanitizedText: String, piiCount: UInt32) {
        let result = try! sanitizeText(handle: handle, text: text)
        if result.piiCount > 0 {
            lastMappingJson = result.mappingJson
        }
        return (result.sanitizedText, result.piiCount)
    }

    func restore(_ text: String) -> String {
        return restoreText(handle: handle, text: text, mappingJson: lastMappingJson)
    }

    func sanitizeTranscript(_ transcript: String) -> String {
        let result = try! sanitizeAudioTranscript(handle: handle, transcript: transcript)
        if result.piiCount > 0 {
            lastMappingJson = result.mappingJson
            return result.sanitizedText
        }
        return transcript
    }

    private static func getOrCreateKey() -> String {
        let service = "com.yourapp.openobscure"
        let account = "fpe-key"

        // Try to load existing key from Keychain
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecReturnData as String: true
        ]
        var result: AnyObject?
        if SecItemCopyMatching(query as CFDictionary, &result) == errSecSuccess,
           let data = result as? Data {
            return String(data: data, encoding: .utf8)!
        }

        // Generate new 32-byte random key
        var bytes = [UInt8](repeating: 0, count: 32)
        _ = SecRandomCopyBytes(kSecRandomDefault, 32, &bytes)
        let hex = bytes.map { String(format: "%02x", $0) }.joined()

        // Store in Keychain
        let addQuery: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecValueData as String: hex.data(using: .utf8)!
        ]
        SecItemAdd(addQuery as CFDictionary, nil)
        return hex
    }
}
```

### 4c. Integration Points for Enchanted

Enchanted (Ollama-compatible iOS/macOS chat app) has four integration points:

**1. Outbound messages** — `ConversationStore.swift`, `sendPrompt()` method:

```swift
// After building messageHistory (line ~140), before creating OKChatRequestData:
var lastMappingJson: String = "{}"

messageHistory = messageHistory.map { msg in
    if msg.role == .user {
        let result = try! sanitizeText(
            handle: OpenObscureManager.shared.handle,
            text: msg.content
        )
        if result.piiCount > 0 {
            lastMappingJson = result.mappingJson
        }
        return OKChatRequestData.Message(
            role: msg.role,
            content: result.sanitizedText,
            images: msg.images
        )
    }
    return msg
}
```

**2. Inbound responses** — `ConversationStore.swift`, `handleReceive()` method:

```swift
// In handleReceive(), after extracting responseContent (line ~194):
if let responseContent = response.message?.content {
    let restored = restoreText(
        handle: OpenObscureManager.shared.handle,
        text: responseContent,
        mappingJson: lastMappingJson
    )
    currentMessageBuffer = currentMessageBuffer + restored
}
```

**3. Speech transcripts** — `InputFields_macOS.swift` (line ~97) or `ChatView_iOS.swift` (line ~150):

```swift
RecordingView(isRecording: $isRecording.animation()) { transcription in
    let piiCount = checkAudioPii(
        handle: OpenObscureManager.shared.handle,
        transcript: transcription
    )
    if piiCount > 0 {
        let result = try! sanitizeAudioTranscript(
            handle: OpenObscureManager.shared.handle,
            transcript: transcription
        )
        self.message = result.sanitizedText
    } else {
        self.message = transcription
    }
}
```

**4. Images** — `ConversationStore.swift`, before base64 encoding (line ~148):

```swift
if let image = image?.render() {
    // Use compressImageData() — works on both iOS (UIImage) and macOS (NSImage)
    // Note: NSImage does NOT have jpegData() — always use compressImageData()
    if let imageData = image.compressImageData() {
        do {
            let sanitized = try sanitizeImage(
                handle: OpenObscureManager.shared.handle,
                imageBytes: imageData
            )
            let base64 = sanitized.base64EncodedString()
            // Use sanitized base64 instead of original
        } catch {
            // Fail-open: use original image if sanitization fails
            let base64 = image.convertImageToBase64String()
        }
    }
}
```

---

## Part 5: Android Integration (Kotlin)

### 5a. Add to Android Project

1. **Copy native libraries:**

```
app/src/main/jniLibs/
├── arm64-v8a/
│   └── libopenobscure_proxy.so
└── x86_64/
    └── libopenobscure_proxy.so
```

2. **Copy Kotlin bindings:**

```
app/src/main/java/uniffi/openobscure_proxy/
└── openobscure_proxy.kt
```

3. **Add JNA dependency** in `app/build.gradle.kts`:

```kotlin
dependencies {
    implementation("net.java.dev.jna:jna:5.15.0@aar")
}
```

4. **Add ProGuard keep rules** in `proguard-rules.pro`:

```
-keep class uniffi.openobscure_proxy.** { *; }
-keep class com.sun.jna.** { *; }
-dontwarn com.sun.jna.**
```

5. **Verify Gradle JNI config** — ensure `build.gradle.kts` has:

```kotlin
android {
    defaultConfig {
        ndk { abiFilters += listOf("arm64-v8a", "x86_64") }
    }
    packaging {
        jniLibs { useLegacyPackaging = true }
    }
}
```

### 5b. Initialize OpenObscure

```kotlin
import uniffi.openobscure_proxy.*

object OpenObscureManager {
    private var _handle: OpenObscureHandle? = null
    var lastMappingJson: String = "{}"

    fun init(context: Context) {
        if (_handle != null) return
        val key = getOrCreateKey(context)
        _handle = createOpenobscure(
            configJson = """{"scanner_mode": "regex"}""",
            fpeKeyHex = key
        )
    }

    private val handle: OpenObscureHandle
        get() = _handle ?: error("OpenObscureManager.init() not called")

    fun sanitize(text: String): SanitizeResultFfi {
        val result = sanitizeText(handle = handle, text = text)
        if (result.piiCount > 0u) {
            lastMappingJson = result.mappingJson
        }
        return result
    }

    fun restore(text: String): String {
        return restoreText(handle = handle, text = text, mappingJson = lastMappingJson)
    }

    private fun getOrCreateKey(context: Context): String {
        val prefs = context.getSharedPreferences("openobscure", Context.MODE_PRIVATE)
        prefs.getString("fpe_key", null)?.let { return it }

        // Generate 32 random bytes → 64 hex chars
        val bytes = ByteArray(32)
        java.security.SecureRandom().nextBytes(bytes)
        val hex = bytes.joinToString("") { "%02x".format(it) }

        // In production: use Android Keystore instead of SharedPreferences
        prefs.edit().putString("fpe_key", hex).apply()
        return hex
    }
}
```

### 5c. Integration Points for RikkaHub

RikkaHub (Android LLM chat client) supports OpenAI/Claude/Google providers with configurable base URLs and an OkHttp interceptor stack.

**Option A: OkHttp Interceptor (Recommended — least invasive)**

Replace/extend `AIRequestInterceptor.kt` in `app/src/main/java/me/rerere/rikkahub/data/ai/`:

```kotlin
import okhttp3.Interceptor
import okhttp3.Response
import okhttp3.RequestBody.Companion.toRequestBody
import okio.Buffer
import uniffi.openobscure_proxy.*
import kotlinx.serialization.json.*

class OpenObscureInterceptor : Interceptor {

    override fun intercept(chain: Interceptor.Chain): Response {
        var request = chain.request()

        // Only process JSON chat requests
        val body = request.body ?: return chain.proceed(request)
        val contentType = body.contentType()
        if (contentType?.subtype != "json") return chain.proceed(request)

        // Read and sanitize request body
        val buffer = Buffer()
        body.writeTo(buffer)
        val bodyStr = buffer.readUtf8()

        val sanitizedBody = sanitizeRequestJson(bodyStr)
        val newRequest = request.newBuilder()
            .method(request.method, sanitizedBody.toRequestBody(contentType))
            .build()

        return chain.proceed(newRequest)
    }

    private fun sanitizeRequestJson(json: String): String {
        val root = try {
            Json.parseToJsonElement(json).jsonObject.toMutableMap()
        } catch (_: Exception) {
            return json
        }

        val messages = root["messages"]?.jsonArray ?: return json
        val mgr = OpenObscureManager

        val sanitizedMessages = messages.map { msg ->
            val obj = msg.jsonObject
            val content = obj["content"]?.jsonPrimitive?.contentOrNull ?: return@map msg

            val result = mgr.sanitize(content)
            if (result.piiCount > 0u) {
                JsonObject(obj.toMutableMap().apply {
                    put("content", JsonPrimitive(result.sanitizedText))
                })
            } else {
                msg
            }
        }

        root["messages"] = JsonArray(sanitizedMessages)
        return JsonObject(root).toString()
    }
}
```

Wire into `DataSourceModule.kt` (line ~192):

```kotlin
.addInterceptor(OpenObscureInterceptor())  // replaces AIRequestInterceptor
```

**Option B: Message-Level Integration**

For finer control, intercept at the message building level in `ChatCompletionsAPI.kt`:

```kotlin
// In buildMessages() (line ~377), wrap each user message:
val result = sanitizeText(OpenObscureManager.handle, userText)
// Use result.sanitizedText instead of userText in the JSON builder

// In parseMessage() (line ~568), restore response text:
val restored = restoreText(OpenObscureManager.handle, assistantText, lastMappingJson)
```

---

## Part 6: Feature Coverage

| Feature | Regex-only (no models) | With NER model (+14MB) | With all models (+~80MB) |
|---------|----------------------|----------------------|------------------------|
| Credit card (Luhn validated) | Yes | Yes | Yes |
| SSN (range validated) | Yes | Yes | Yes |
| Phone, Email, API Key | Yes | Yes | Yes |
| IPv4, IPv6, GPS, MAC, IBAN | Yes | Yes | Yes |
| Health/Child keywords | Yes | Yes | Yes |
| Person names | -- | Yes | Yes |
| Locations, Organizations | -- | Yes | Yes |
| Image face solid fill | -- | -- | Yes |
| Image OCR solid fill | -- | -- | Yes |
| Audio transcript PII | Yes | Yes | Yes |

**Recommendation:** Start with `scanner_mode: "regex"` (zero model files, 12 of 15 PII types, 99.7% recall on structured PII). Add NER later if person/location detection is needed.

---

## Part 6b: Adding NER (Named Entity Recognition)

NER adds detection of **person names**, **locations**, and **organizations** — the 3 PII types that regex alone cannot catch. Two model variants are available:

| Model | File | Size | Latency (p50) | F1 Score | Device Tier |
|-------|------|------|---------------|----------|-------------|
| TinyBERT 4L INT8 | `ner-lite/model_int8.onnx` | ~14 MB | 0.8 ms | 85.6% | Standard, Lite (4GB+) |
| DistilBERT 6L INT8 | `ner/model_int8.onnx` | ~64 MB | 4.3 ms | ≥93% | Full (8GB+) |

### Step 1: Copy Model Files

Each NER model directory contains:

```
model_int8.onnx      — ONNX INT8 quantized model
vocab.txt            — WordPiece vocabulary (30,522 tokens)
label_map.json       — 11-label NER schema (PER/LOC/ORG/HEALTH/CHILD)
config.json          — Model architecture config
tokenizer.json       — HuggingFace tokenizer config
```

**iOS/macOS:** Add the model directory to your app bundle:

```
YourApp/
├── Resources/
│   └── ner-lite/            ← or ner/ for DistilBERT
│       ├── model_int8.onnx
│       ├── vocab.txt
│       └── label_map.json   (+ config.json, tokenizer.json)
```

**Android:** Add to `assets/`:

```
app/src/main/assets/
└── ner-lite/                ← or ner/ for DistilBERT
    ├── model_int8.onnx
    ├── vocab.txt
    └── label_map.json       (+ config.json, tokenizer.json)
```

The model files ship with OpenObscure under `openobscure-proxy/models/ner/` (DistilBERT) and `openobscure-proxy/models/ner-lite/` (TinyBERT). Copy the appropriate directory into your app.

### Step 2: Update Config JSON

Change `scanner_mode` and provide the model path:

**Option A: Explicit NER mode** — always use NER regardless of device tier:

```json
{
  "scanner_mode": "ner",
  "ner_model_dir": "/path/to/ner",
  "ner_model_dir_lite": "/path/to/ner-lite"
}
```

**Option B: Auto mode (recommended)** — let device profiling choose the best scanner:

```json
{
  "scanner_mode": "auto",
  "auto_detect": true,
  "ner_model_dir": "/path/to/ner",
  "ner_model_dir_lite": "/path/to/ner-lite"
}
```

In auto mode:
- **Full tier (≥8 GB RAM):** Uses DistilBERT from `ner_model_dir`
- **Standard tier (4–8 GB):** Uses TinyBERT from `ner_model_dir_lite`
- **Lite tier (<4 GB):** Falls back to regex-only (no NER model loaded)

If only `ner_model_dir` is set (no `_lite` variant), all tiers that enable NER use that single model. If only `ner_model_dir_lite` is set, the TinyBERT path is used as fallback for all tiers.

### Step 3: Platform-Specific Model Path Resolution

**Swift (iOS/macOS):**

```swift
// Resolve model path from app bundle
let nerLitePath = Bundle.main.path(forResource: "ner-lite", ofType: nil)!
let nerPath = Bundle.main.path(forResource: "ner", ofType: nil)  // optional

let config = """
{
  "scanner_mode": "auto",
  "auto_detect": true,
  "ner_model_dir_lite": "\(nerLitePath)",
  "ner_model_dir": "\(nerPath ?? nerLitePath)"
}
"""

let handle = try createOpenobscure(configJson: config, fpeKeyHex: key)
```

**Kotlin (Android):**

```kotlin
// Copy model from assets to internal storage (required — ONNX Runtime needs file paths)
fun copyAssetsDir(context: Context, assetDir: String): String {
    val outDir = File(context.filesDir, assetDir)
    if (!outDir.exists()) {
        outDir.mkdirs()
        context.assets.list(assetDir)?.forEach { file ->
            context.assets.open("$assetDir/$file").use { input ->
                File(outDir, file).outputStream().use { output ->
                    input.copyTo(output)
                }
            }
        }
    }
    return outDir.absolutePath
}

val nerLitePath = copyAssetsDir(context, "ner-lite")
val nerPath = copyAssetsDir(context, "ner")  // optional

val config = """
{
  "scanner_mode": "auto",
  "auto_detect": true,
  "ner_model_dir_lite": "$nerLitePath",
  "ner_model_dir": "$nerPath"
}
""".trimIndent()

val handle = createOpenobscure(configJson = config, fpeKeyHex = key)
```

### Step 4: Verify NER Detection

Test with names and locations that regex cannot detect:

```
"Meeting with John Smith tomorrow"           → PER detected
"Our office is in San Francisco"             → LOC detected
"Contract with Acme Corporation"             → ORG detected
"Dr. Emily Chen diagnosed hypertension"     → PER + HEALTH detected
```

Check the scanner mode in stats:

```swift
let stats = getStats(handle: handle)
print(stats.scannerMode)  // "ner" if model loaded, "regex" if fallback
```

```kotlin
val stats = getStats(handle)
println(stats.scannerMode)  // "ner" if model loaded, "regex" if fallback
```

If `scannerMode` reports `"regex"` when you expected `"ner"`, check:
1. Model path is correct and accessible at runtime
2. `model_int8.onnx` and `vocab.txt` exist in the model directory
3. Device has enough RAM for the selected model tier
4. Model file isn't a Git LFS pointer (should be >1 MB, not 130 bytes)

### NER Label Schema

The 11-label BIO schema detects 5 entity types:

| Label | Entity Type | Example |
|-------|-------------|---------|
| PER | Person name | "John Smith", "Dr. Chen" |
| LOC | Location | "San Francisco", "123 Main St" |
| ORG | Organization | "Acme Corp", "WHO" |
| HEALTH | Health term | "diabetes", "MRI scan" |
| CHILD | Child-related | "pediatric", "minor" |

NER results are merged with regex matches by the HybridScanner — overlapping detections are deduplicated automatically with confidence-based resolution.

---

## Part 6c: Cognitive Firewall (Response Integrity)

The cognitive firewall scans LLM responses for persuasion, manipulation, and social engineering techniques *before* they reach the user. It uses a two-tier cascade:

- **R1 — Dictionary scan** (<1ms): ~250 phrases across 7 persuasion categories (urgency, authority, scarcity, social proof, fear, reciprocity, commitment)
- **R2 — TinyBERT classifier** (~30ms): 4 EU AI Act Article 5 categories (subliminal manipulation, exploitation of vulnerabilities, social scoring, real-time biometric)

### Current Status: Available in Both Modes

The cognitive firewall is available in **both** the proxy/gateway path and the mobile embedded API.

| Mode | Cognitive Firewall | How |
|------|-------------------|-----|
| **Proxy** (gateway) | Available | Scans responses automatically after FPE decryption |
| **Embedded** (mobile) | Available | Call `scanResponse(handle, responseText)` after `restoreText()` |

**Tier gating:**
- **Full/Standard tier:** R1 dictionary + R2 TinyBERT classifier (if model loaded)
- **Lite tier:** Disabled by `FeatureBudget` (`ri_enabled: false`)

### Embedded API Reference

**UniFFI function:**
```
scanResponse(handle, responseText) -> RiReportFFI?
```

Returns `nil`/`null` if no manipulation detected or if RI is disabled.

**`RiReportFFI`:**
```
severity: String          — "Notice", "Warning", or "Caution"
categories: [String]      — Persuasion categories detected (Urgency, Authority, etc.)
flags: [String]           — Matched phrases from R1 dictionary
r2Categories: [String]    — Article 5 categories from R2 classifier (if model loaded)
scanTimeUs: UInt64        — Scan duration in microseconds
```

**Config fields** (in `MobileConfig` JSON):
```json
{
  "ri_enabled": true,
  "ri_sensitivity": "medium",
  "ri_model_dir": "/path/to/ri"
}
```

- `ri_enabled` — Enable/disable the cognitive firewall (default: `false`)
- `ri_sensitivity` — `"off"`, `"low"`, `"medium"` (default), `"high"` — controls when R2 is invoked
- `ri_model_dir` — Path to R2 model directory (optional — R1 works without it)

**Model files** (optional — R1 dictionary works without models):
```
models/ri/
├── model_int8.onnx      — TinyBERT R2 classifier
└── vocab.txt            — WordPiece vocabulary
```

R1 dictionary scanning requires no model files and runs in <1ms. R2 model adds deeper classification but is optional. If `ri_model_dir` points to a missing directory, the scanner gracefully falls back to R1-only mode.

### Alternative: Proxy Mode

For apps using the proxy/gateway model, the cognitive firewall runs automatically — no code changes needed. Point the app's LLM API base URL at the proxy:

**Enchanted (macOS):** Settings > Server Address > `http://127.0.0.1:18790/ollama`

**RikkaHub (Android):** Provider Settings > Base URL > `http://10.0.2.2:18790/openai/v1` (emulator) or `http://<host-ip>:18790/openai/v1` (device)

### Integration Pattern

**Swift:**
```swift
// After restoring response text
let restored = restoreText(handle: h, text: responseText, mappingJson: mapping)

// Scan for manipulation
let report = scanResponse(handle: h, responseText: restored)
if report.severity == "WARNING" {
    // Show warning banner to user before displaying response
    showManipulationWarning(report.categories)
}
```

**Kotlin:**
```kotlin
// After restoring response text
val restored = restoreText(handle, responseText, mappingJson)

// Scan for manipulation
val report = scanResponse(handle, restored)
if (report.severity == "WARNING") {
    showManipulationWarning(report.categories)
}
```

The tier gating follows the existing pattern:
- **Full/Standard tier:** R1 dictionary + R2 TinyBERT classifier
- **Lite tier:** R1 dictionary only (no R2 model)

---

## Part 7: Testing Your Integration

### Verify PII Detection

Send these test strings and confirm they are sanitized:

```
"My card is 4111-1111-1111-1111"          → CC encrypted (Luhn-valid replacement)
"SSN: 123-45-6789"                         → SSN encrypted (format preserved)
"Email: john.doe@example.com"              → Email encrypted
"Call 555-867-5309"                         → Phone encrypted
"Server at 192.168.1.100"                  → IPv4 encrypted
"IBAN: DE89370400440532013000"             → IBAN encrypted
"Patient diagnosed with diabetes"          → Health keyword detected
```

### Verify Round-Trip Restore

```swift
let result = try sanitizeText(handle: h, text: "Card: 4111-1111-1111-1111")
assert(result.piiCount >= 1)
assert(!result.sanitizedText.contains("4111"))

let restored = restoreText(handle: h, text: result.sanitizedText, mappingJson: result.mappingJson)
assert(restored.contains("4111-1111-1111-1111"))
```

### Verify Speech Transcript

```swift
let result = try sanitizeAudioTranscript(handle: h, transcript: "my ssn is 123-45-6789")
assert(result.piiCount >= 1)
assert(!result.sanitizedText.contains("123-45-6789"))
```

### Run Existing Test Suites

```bash
# iOS test app (30 XCTests)
cd test/apps/ios && swift test

# Android test app (36 instrumented tests)
cd test/apps/android && ./gradlew connectedAndroidTest

# Proxy unit tests (1,683 tests including mobile API)
cargo test --manifest-path openobscure-proxy/Cargo.toml --lib --all-features
```

---

## Part 8: Troubleshooting

| Issue | Cause | Fix |
|-------|-------|-----|
| `MobileBindingError` on init | Invalid FPE key (must be exactly 64 hex chars = 32 bytes) | Check key length and hex format |
| Linker error: `_openobscure_proxy_*` | Library not linked | Add `-lopenobscure_proxy` to Other Linker Flags |
| `UnsatisfiedLinkError` on Android | `.so` not in correct ABI folder | Verify `jniLibs/<abi>/libopenobscure_proxy.so` path |
| JNA not found on Android | Missing dependency | Add `implementation("net.java.dev.jna:jna:5.15.0@aar")` |
| Image sanitization fails | `image_enabled: false` or no model files | Set `image_enabled: true` and provide model paths in config |
| 0 PII detected for names | Regex mode can't detect names | Switch to `scanner_mode: "ner"` and provide NER model |
| `Cannot find 'createOpenobscure' in scope` (Swift) | Missing import | Add `import OpenObscureLib` to the file |
| Type mismatch `UInt64` vs `OpenObscureHandle` | UniFFI generates an opaque class | Use `OpenObscureHandle` type, not `UInt64` |
| Type mismatch `Int32` vs `UInt32` for `piiCount` | UniFFI uses unsigned types | `piiCount` is `UInt32` in Swift, compare with `> 0u` in Kotlin |
| macOS `NSImage` has no `jpegData()` | iOS-only API | Use `compressImageData()` which works on both iOS and macOS |
| `sherpa-rs-sys` linker failure on iOS/Android | Voice feature pulls in native deps | Build with `--no-default-features --features mobile` |
| `cargo-ndk --manifest-path` fails | `cargo-ndk` doesn't support this flag | Use `(cd proxy-dir && cargo ndk ...)` subshell pattern |
| JitPack dependency timeout | JitPack.io outage | Clone repos locally, `publishToMavenLocal`, add `mavenLocal()` before JitPack in `settings.gradle.kts` |
| `ort-sys` no prebuilt for `x86_64-linux-android` | Expected limitation | Only build `arm64-v8a` for real devices; x86_64 is emulator-only |

---

## Reference: Tested Third-Party Apps

| App | Platform | Integration Approach | Build Status | Key Files Modified |
|-----|----------|---------------------|-------------|---------------------|
| **Enchanted** | iOS/macOS | Local SPM package + direct API calls | **BUILD SUCCEEDED** (macOS ad-hoc) | `OpenObscureManager.swift` (new), `ConversationStore.swift` (send/receive/image), `InputFields_macOS.swift` (speech), `ChatView_iOS.swift` (speech), `project.pbxproj` |
| **RikkaHub** | Android | OkHttp interceptor + JNI/JNA | **BUILD SUCCEEDED** (debug APK 76MB arm64) | `OpenObscureManager.kt` (new), `OpenObscureInterceptor.kt` (new), `DataSourceModule.kt` (interceptor wire), `RikkaHubApp.kt` (init), `build.gradle.kts` (JNA dep), `proguard-rules.pro` |
| **OpenClaw** | Desktop | Gateway proxy (see [gateway setup](../setup/gateway_setup.md)) | Verified | Config only — point `LLM_API_BASE` at proxy |

### Integration Artifacts

| Artifact | Enchanted (macOS) | RikkaHub (Android) |
|----------|-------------------|-------------------|
| Fork location | `/Users/admin/Test/enchanted-openobscure/` | `/Users/admin/Test/rikkahub-openobscure/` |
| Native library | 158MB static `.a` (macOS) / 160MB (iOS) | 24MB `.so` (arm64-v8a) |
| Bindings | `openobscure_proxy.swift` + FFI header + modulemap | `openobscure_proxy.kt` (UniFFI) |
| Key storage | iOS Keychain | Android SharedPreferences |
| Intercept pattern | Direct API calls in ConversationStore | OkHttp Interceptor on request JSON |
| Build output | Xcode build (CODE_SIGNING_ALLOWED=NO) | `app-arm64-v8a-debug.apk` (76MB) |

---

## Reference: Build Scripts

| Script | What it builds | Output |
|--------|---------------|--------|
| `build/build_ios.sh` | iOS static libs + XCFramework | `.a` files + `.xcframework` |
| `build/build_android.sh` | Android shared libs | `.so` per ABI |
| `build/build_napi.sh` | Node.js native addon | `scanner.node` |
| `build/generate_bindings.sh` | UniFFI Swift + Kotlin bindings | `bindings/swift/`, `bindings/kotlin/` |
| `build/download_models.sh` | ONNX model files | `openobscure-proxy/models/` |
