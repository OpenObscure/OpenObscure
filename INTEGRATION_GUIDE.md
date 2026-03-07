# Embedding OpenObscure in Third-Party Apps

A step-by-step guide for integrating OpenObscure as a **native library** (embedded model) into iOS, Android, and macOS chat applications. This covers both first-party test apps and third-party apps like [Enchanted](https://github.com/AugustDev/enchanted) (iOS/macOS) and [RikkaHub](https://github.com/rikkahub/rikkahub) (Android).

> **Proxy vs Embedded:** The [SETUP_GUIDE.md](SETUP_GUIDE.md) covers the **gateway/proxy** model where OpenObscure runs as a sidecar HTTP proxy. This guide covers the **embedded** model where OpenObscure is compiled into your app as a native library.

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
| `createOpenobscure` | `(configJson: String, fpeKeyHex: String) -> Handle` | Initialize with config JSON and 32-byte FPE key (64 hex chars) |
| `sanitizeText` | `(handle, text) -> SanitizeResult` | Scan text for PII, return sanitized text + mapping JSON |
| `restoreText` | `(handle, text, mappingJson) -> String` | Decrypt FPE tokens in LLM response using saved mapping |
| `sanitizeImage` | `(handle, imageBytes) -> Data` | Face blur + OCR blur on image bytes (JPEG/PNG) |
| `sanitizeAudioTranscript` | `(handle, transcript) -> SanitizeResult` | Scan speech transcript for PII |
| `checkAudioPii` | `(handle, transcript) -> Int` | Quick PII count check (no encryption) |
| `getStats` | `(handle) -> MobileStats` | Device tier, total PII found, image count |

### SanitizeResult

```
sanitizedText: String    — Text with PII replaced by FPE ciphertexts
mappingJson: String      — JSON mapping for restore (save per-request)
piiCount: Int            — Number of PII items found
categories: [String]     — PII types found ("credit_card", "ssn", "email", etc.)
```

### MobileConfig (JSON)

```json
{
  "scanner_mode": "regex",
  "auto_detect": true,
  "keywords_enabled": true,
  "image_enabled": false
}
```

- `scanner_mode`: `"auto"` (default, uses device tier), `"regex"`, `"crf"`, `"ner"`
- `auto_detect`: `true` (default) — profiles device RAM for tier selection
- `keywords_enabled`: `true` (default) — health/child keyword dictionary
- `image_enabled`: `false` (default) — requires ONNX model files

### PII Types (Regex-Only Mode — No Models Required)

Credit card (Luhn), SSN (range-validated), phone, email, API key, IPv4, IPv6, GPS coordinates, MAC address, IBAN (22 countries with check-digit validation), health keywords (~350 terms), child-related keywords (~350 terms).

**12 of 15 types work without any model files.** Person names, locations, and organizations require the NER TinyBERT model (~14MB).

---

## Part 4: iOS/macOS Integration (Swift)

### 4a. Add to Xcode Project (SPM)

The simplest approach uses Swift Package Manager, matching our test app structure:

1. **Copy artifacts into your project:**

```
YourApp/
├── COpenObscure/
│   └── include/
│       ├── openobscure_proxyFFI.h       ← from bindings/swift/
│       └── module.modulemap              ← from bindings/swift/
├── OpenObscure/
│   └── openobscure_proxy.swift           ← from bindings/swift/
└── lib/
    └── libopenobscure_proxy.a            ← from build output
```

2. **Add to your Package.swift** (or Xcode target):

```swift
.target(
    name: "COpenObscure",
    path: "COpenObscure",
    linkerSettings: [
        .unsafeFlags(["-L", "lib"]),
        .linkedLibrary("openobscure_proxy"),
        .linkedLibrary("resolv"),
        .linkedFramework("Security"),
        .linkedFramework("SystemConfiguration"),
    ]
),
.target(
    name: "OpenObscure",
    dependencies: ["COpenObscure"],
    path: "OpenObscure"
),
```

3. **For Xcode projects without SPM**, add these Build Settings:
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
    let handle: UInt64

    private init() {
        // Generate or load a 32-byte key (64 hex chars)
        // In production: store in iOS Keychain
        let key = OpenObscureManager.getOrCreateKey()
        handle = try! createOpenobscure(
            configJson: #"{"scanner_mode": "regex"}"#,
            fpeKeyHex: key
        )
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
    if let jpegData = image.jpegData(compressionQuality: 1.0) {
        do {
            let sanitized = try sanitizeImage(
                handle: OpenObscureManager.shared.handle,
                imageBytes: jpegData
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

4. **Verify Gradle JNI config** — ensure `build.gradle.kts` has:

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
    val handle: Long by lazy {
        val key = getOrCreateKey()
        createOpenobscure(
            configJson = """{"scanner_mode": "regex"}""",
            fpeKeyHex = key
        )
    }

    private fun getOrCreateKey(): String {
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
    private val handle = OpenObscureManager.handle
    private var lastMappingJson: String = "{}"

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
        val root = Json.parseToJsonElement(json).jsonObject.toMutableMap()
        val messages = root["messages"]?.jsonArray ?: return json

        val sanitizedMessages = messages.map { msg ->
            val obj = msg.jsonObject
            val content = obj["content"]?.jsonPrimitive?.contentOrNull ?: return@map msg

            val result = sanitizeText(handle, content)
            if (result.piiCount > 0) {
                lastMappingJson = result.mappingJson
            }

            JsonObject(obj.toMutableMap().apply {
                put("content", JsonPrimitive(result.sanitizedText))
            })
        }

        root["messages"] = JsonArray(sanitizedMessages)
        return JsonObject(root).toString()
    }

    fun restoreResponse(text: String): String {
        return restoreText(handle, text, lastMappingJson)
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
| Image face blur | -- | -- | Yes |
| Image OCR blur | -- | -- | Yes |
| Audio transcript PII | Yes | Yes | Yes |

**Recommendation:** Start with `scanner_mode: "regex"` (zero model files, 12 of 15 PII types, 99.7% recall on structured PII). Add NER later if person/location detection is needed.

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

# Proxy unit tests (1,667 tests including mobile API)
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

---

## Reference: Tested Third-Party Apps

| App | Platform | Integration Approach | Key Files to Modify |
|-----|----------|---------------------|---------------------|
| **Enchanted** | iOS/macOS | SPM + direct API calls | `ConversationStore.swift` (send/receive), `InputFields_macOS.swift` (speech), `ChatView_iOS.swift` (speech) |
| **RikkaHub** | Android | OkHttp interceptor + JNI | `DataSourceModule.kt` (interceptor wire), `AIRequestInterceptor.kt` (replace with OpenObscure interceptor) |
| **OpenClaw** | Desktop | Gateway proxy (see SETUP_GUIDE.md) | Config only — point `LLM_API_BASE` at proxy |

---

## Reference: Build Scripts

| Script | What it builds | Output |
|--------|---------------|--------|
| `build/build_ios.sh` | iOS static libs + XCFramework | `.a` files + `.xcframework` |
| `build/build_android.sh` | Android shared libs | `.so` per ABI |
| `build/build_napi.sh` | Node.js native addon | `scanner.node` |
| `build/generate_bindings.sh` | UniFFI Swift + Kotlin bindings | `bindings/swift/`, `bindings/kotlin/` |
| `build/download_models.sh` | ONNX model files | `openobscure-proxy/models/` |
