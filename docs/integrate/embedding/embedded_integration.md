# Embedding OpenObscure in Third-Party Apps

A step-by-step guide for integrating OpenObscure as a **native library** (embedded model) into iOS, Android, and macOS chat applications. This covers both first-party test apps and third-party apps like [Enchanted](https://github.com/AugustDev/enchanted) (iOS/macOS) and [RikkaHub](https://github.com/rikkahub/rikkahub) (Android).

> **Proxy vs Embedded:** The [gateway quick start](../../get-started/gateway-quick-start.md) covers the **gateway/proxy** model where OpenObscure runs as a sidecar HTTP proxy. This guide covers the **embedded** model where OpenObscure is compiled into your app as a native library. See [Embedded Setup](../../../setup/embedded_setup.md) for the complete build walkthrough.

---

**Contents**

- [Prerequisites](#prerequisites)
- [Part 3: API Reference (All Platforms)](#part-3-api-reference-all-platforms)
- [Part 3a: Reference API Usage](#part-3a-reference-api-usage)
- [Part 4: iOS/macOS Integration (Swift)](#part-4-iosmacos-integration-swift)
- [Part 5: Android Integration (Kotlin)](#part-5-android-integration-kotlin)
- [Part 6: Feature Coverage](#part-6-feature-coverage)
- [Part 6a: Bundling All Models (Recommended)](#part-6a-bundling-all-models-recommended)
- [Part 6b: Adding NER Only (Minimal)](#part-6b-adding-ner-only-minimal)
- [Part 6c: Cognitive Firewall (Response Integrity)](#part-6c-cognitive-firewall-response-integrity)
- [Part 7: Testing Your Integration](#part-7-testing-your-integration)
- [Part 8: Troubleshooting](#part-8-troubleshooting)
- [Reference: Tested Third-Party Apps](#reference-tested-third-party-apps)
- [Reference: Build Scripts](#reference-build-scripts)

## Prerequisites

| Tool | iOS/macOS | Android |
|------|-----------|---------|
| Rust toolchain | `rustup` (stable) | `rustup` (stable) |
| Platform targets | `rustup target add aarch64-apple-ios aarch64-apple-ios-sim` | `rustup target add aarch64-linux-android x86_64-linux-android` |
| Build tool | Xcode 15+ with iOS SDK | `cargo install cargo-ndk` + Android NDK |
| Bindings | `./build/generate_bindings.sh --swift-only` | `./build/generate_bindings.sh --kotlin-only` |

---

> **Build steps live in one place.**
> See [Embedded Setup — Part 2: Build the library](../../../setup/embedded_setup.md#part-2-build-the-library)
> and [Part 3: Generate bindings](../../../setup/embedded_setup.md#part-3-generate-uniffi-bindings)
> for the canonical build commands. The steps below assume you
> have completed that setup.

---

## Part 3: API Reference (All Platforms)

The embedded API is identical across Swift, Kotlin, and Rust:

| Function | Signature | Description |
|----------|-----------|-------------|
| `createOpenobscure` | `(configJson: String, fpeKeyHex: String) -> OpenObscureHandle` | Initialize with config JSON and 32-byte FPE key (64 hex chars) |
| `sanitizeText` | `(handle, text) -> SanitizeResult` | Scan text for PII, return sanitized text + mapping JSON |
| `sanitizeMessages` | `(handle, messages: [ChatMessageFfi]) -> SanitizeMessagesResult` | Sanitize all roles (user + assistant + system) in one pass with shared token registry — preferred for multi-turn conversations |
| `restoreText` | `(handle, text, mappingJson) -> String` | Decrypt FPE tokens in LLM response using saved mapping |
| `sanitizeImage` | `(handle, imageBytes) -> Data` | EXIF strip (always) + face/OCR/NSFW redaction (model-dependent) on image bytes (JPEG/PNG) |
| `sanitizeAudioTranscript` | `(handle, transcript) -> SanitizeResult` | Scan speech transcript for PII |
| `checkAudioPii` | `(handle, transcript) -> Int` | Quick PII count check (no encryption) |
| `scanResponse` | `(handle, responseText) -> RiReportFFI?` | Scan LLM response for persuasion/manipulation (cognitive firewall) |
| `rotateKey` | `(handle, newKeyHex: String)` | Rotate FPE key with 30s overlap window for in-flight mappings |
| `getStats` | `(handle) -> MobileStats` | Device tier, total PII found, image count |
| `getDebugLog` | `() -> String` | Drain accumulated Rust-side debug log (model loading, errors, verification). Call after restore to surface token match/miss diagnostics. |

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

### Debug Log

`getDebugLog()` is a standalone function (no handle required) that drains the Rust-side debug buffer. It returns all accumulated log messages since the last call, then clears the buffer. Useful for diagnosing model loading issues on iOS where `stderr` goes to `/dev/null`.

**What it captures:**
- Model directory verification results (present/missing subdirectories)
- NER model selection and loading (budget tier, model variant, fallback)
- NER/image pipeline load failures with error details
- Device tier and scanner mode selection

**When to call it:**
- After `createOpenobscure()` — to check model loading diagnostics
- After `sanitizeImage()` failures — to see image pipeline errors
- On debug/beta builds — write to app log file for support

### MobileConfig (JSON)

```json
{
  "scanner_mode": "regex",
  "auto_detect": true,
  "keywords_enabled": true,
  "gazetteer_enabled": true,
  "image_enabled": true,
  "ri_enabled": true,
  "ri_sensitivity": "medium",
  "ri_model_dir": null,
  "nsfw_classifier_model_dir": null,
  "models_base_dir": null,
  "ner_pool_size": 1
}
```

- `scanner_mode`: `"auto"` (default, uses device tier), `"regex"`, `"crf"`, `"ner"`
- `auto_detect`: `true` (default) — profiles device RAM for tier selection
- `keywords_enabled`: `true` (default) — health/child keyword dictionary; budget-gated
- `gazetteer_enabled`: `true` (default) — name gazetteer for person name detection (embedded name lists, no model files); budget-gated
- `image_enabled`: `true` (default) — device budget gates actual activation; requires ONNX model files for face/OCR/NSFW redaction. Set `false` to disable explicitly.
- `ri_enabled`: `true` (default) — device budget gates actual activation. Set `false` to disable explicitly.
- `ri_sensitivity`: `"medium"` (default) — `"off"`, `"low"`, `"medium"`, `"high"` — controls R2 classifier invocation threshold
- `ri_model_dir`: `null` (default) — path to R2 model directory; R1 dictionary works without it
- `nsfw_classifier_model_dir`: `null` (default) — path to ViT-base NSFW classifier (~83 MB INT8). This is the sole NSFW detection model (NudeNet has been removed)
- `models_base_dir`: `null` (default) — base directory containing model subdirectories. When set, individual `*_model_dir` fields are auto-resolved from standard subdirectory names. Explicit per-model paths always take priority. Standard subdirectories: `ner/`, `ner_lite/`, `crf/`, `scrfd/`, `blazeface/`, `ocr/`, `nsfw_classifier/`, `ri/`.
- `ner_pool_size`: `1` (default) — number of NER model instances; budget caps the maximum (Full gateway: 2, all embedded: 1)

> **Migration note (v0.18+):** `image_enabled` and `ri_enabled` now default to `true`. Without model files on disk these features are effectively no-ops, but if you previously relied on the `false` default, set them to `false` explicitly in your config JSON.

### PII Types (Regex-Only Mode — No Models Required)

Credit card (Luhn), SSN (range-validated), phone, email, API key, IPv4, IPv6, GPS coordinates, MAC address, IBAN (22 countries with check-digit validation), health keywords (~350 terms), child-related keywords (~350 terms).

**12 of 15 types work without any model files.** Person names, locations, and organizations require the NER TinyBERT model (~14MB).

---

## Part 3a: Reference API Usage

Call `sanitizeText()` with a known PII value to confirm the library is wired correctly before integrating the full LLM flow:

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

## Part 4: iOS/macOS Integration (Swift)

### 4a. Add to Xcode Project (Local SPM Package)

Create a local Swift package alongside the app project. This approach was verified with Enchanted (macOS BUILD SUCCEEDED).

> **Important:** The C target name **must** match the `module` name in the `.modulemap` file. UniFFI generates `module openobscure_coreFFI`, so the target must be named `openobscure_coreFFI`.

1. **Create the local package directory:**

```
OpenObscureLib/
├── Package.swift
├── Sources/
│   ├── COpenObscure/
│   │   └── include/
│   │       ├── openobscure_coreFFI.h         ← from bindings/swift/
│   │       └── openobscure_coreFFI.modulemap ← from bindings/swift/
│   └── OpenObscureLib/
│       └── openobscure_core.swift            ← from bindings/swift/
└── lib/
    └── libopenobscure_core.a                 ← from build output
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
            name: "openobscure_coreFFI",  // MUST match modulemap module name
            path: "Sources/COpenObscure",
            publicHeadersPath: "include"
        ),
        .target(
            name: "OpenObscureLib",
            dependencies: ["openobscure_coreFFI"],
            path: "Sources/OpenObscureLib",
            linkerSettings: [
                .unsafeFlags(["-L\(Context.packageDirectory)/../lib"]),
                .linkedLibrary("openobscure_core"),
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
   - Other Linker Flags: `-lopenobscure_core -lresolv`
   - Library Search Paths: `$(PROJECT_DIR)/lib`
   - Header Search Paths: `$(PROJECT_DIR)/COpenObscure/include`
   - Linked Frameworks: Security, SystemConfiguration

### 4b. Initialize OpenObscure

Create a singleton manager (see full template at [templates/OpenObscureManager.swift](templates/OpenObscureManager.swift)):

```swift
import Foundation
import Security
import OpenObscureLib

final class OpenObscureManager {
    static let shared = OpenObscureManager()
    let handle: OpenObscureHandle
    /// Accumulated token→plaintext mappings across all sanitize calls in a conversation.
    private var accumulatedMappings: [[String]] = []

    private init() {
        let key = OpenObscureManager.getOrCreateKey()
        let modelsDir = Bundle.main.resourcePath.map { $0 + "/models" }
            ?? Bundle.main.bundlePath + "/Contents/Resources/models"
        handle = try! createOpenobscure(
            configJson: """
            {"scanner_mode": "auto", "models_base_dir": "\(modelsDir)"}
            """,
            fpeKeyHex: key
        )
        // Drain Rust-side debug log — model loading diagnostics.
        // On iOS, stderr goes to /dev/null so this is the only way to see Rust logs.
        let debugLog = getDebugLog()
        if !debugLog.isEmpty { print("[OpenObscure] \(debugLog)") }
    }

    func sanitize(_ text: String) -> (sanitizedText: String, piiCount: UInt32) {
        let result = try! sanitizeText(handle: handle, text: text)
        if result.piiCount > 0 { mergeMappings(result.mappingJson) }
        return (result.sanitizedText, result.piiCount)
    }

    func restore(_ text: String) -> String {
        let json = (try? JSONSerialization.data(withJSONObject: accumulatedMappings)) ?? Data("{}".utf8)
        return restoreText(handle: handle, text: text, mappingJson: String(data: json, encoding: .utf8) ?? "{}")
    }

    func scanResponse(_ text: String) -> RiReportFfi? {
        return OpenObscureLib.scanResponse(handle: handle, responseText: text)
    }

    func sanitizeTranscript(_ transcript: String) -> String {
        let result = try! sanitizeAudioTranscript(handle: handle, transcript: transcript)
        if result.piiCount > 0 {
            mergeMappings(result.mappingJson)
            return result.sanitizedText
        }
        return transcript
    }

    /// Reset mappings when starting a new conversation.
    func resetMappings() { accumulatedMappings = [] }

    // See template for full mergeMappings() and getOrCreateKey() implementations
    private func mergeMappings(_ json: String) { /* ... */ }
    private static func getOrCreateKey() -> String { /* Keychain storage ... */ }
}
```

### 4c. Integration Points for Enchanted

Enchanted (Ollama-compatible iOS/macOS chat app) has four integration points:

**1. Outbound messages (all roles)** — `ConversationStore.swift`, `sendPrompt()` method:

Use `sanitizeMessages` (not `sanitizeText`) so the full conversation history — including prior assistant messages — is sanitized in a single pass with consistent tokens across all roles. Sanitizing only user messages leaves restored plaintext PII in assistant history, which the LLM would see on the next turn.

```swift
// Build raw message history from SwiftData (all roles: user, assistant, system)
let oo = OpenObscureManager.shared
let rawMessages = conversation.messages
    .sorted { $0.createdAt < $1.createdAt }
    .map { (role: $0.role, content: $0.content) }

// sanitizeMessages pre-restores non-user messages to resolve stale tokens from
// prior turns before re-tokenizing, ensuring the LLM sees consistent tokens.
let sanitizedPairs = oo.sanitizeMessages(rawMessages)
let messageHistory = sanitizedPairs.map { msg -> OKChatRequestData.Message in
    let role = OKChatRequestData.Message.Role(rawValue: msg.role) ?? .assistant
    return OKChatRequestData.Message(role: role, content: msg.content)
}
```

**2. Inbound responses** — `ConversationStore.swift`, `handleComplete()` method:

Restore is done once when the stream ends, not per-chunk. **Critical:** flush the streaming throttler buffer before calling `restore` — the final batch of tokens may still be in `currentMessageBuffer` when the stream completion fires. Without the flush, `restore` receives a truncated response and the FPE token is never seen, so the original PII value is never restored in the UI.

```swift
@MainActor
private func handleComplete() {
    guard let lastMesasge = messages.last else { return }

    // Flush any tokens still buffered by the throttler before restore.
    // Without this, restore() sees a truncated response (missing final chunk).
    if !currentMessageBuffer.isEmpty {
        let lastIndex = messages.count - 1
        messages[lastIndex].content.append(currentMessageBuffer)
        currentMessageBuffer = ""
    }

    lastMesasge.done = true
    // ...
    Task(priority: .background) {
        let restored = OpenObscureManager.shared.restore(lastMesasge.content)
        OpenObscureManager.shared.scanResponse(restored)
        await MainActor.run { lastMesasge.content = restored }
        try? await swiftDataService.updateMessage(lastMesasge)
    }
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

### LLM Response Handling — Key Implementation Notes

Six issues were identified during the Enchanted integration. Any streaming chat app embedding OpenObscure should account for all six.

**1. Use `sanitizeMessages` for multi-turn conversations**

Each call to `sanitize_messages` generates fresh random FPE tokens. Calling `sanitizeText` per user message means turn 1 assigns `Angela Martinez → PER_7neo` and turn 2 assigns `Angela Martinez → PER_u426` — two different tokens for the same entity. The LLM then sees inconsistent context across turns.

Use `sanitizeMessages` instead, passing the full conversation history (all roles). It sanitizes user and system messages in one pass so every mention of the same entity gets the same token. Assistant messages pass through unchanged — they were already sanitized on their original turn.

**2. Pre-restore non-user messages before re-sanitizing**

`sanitizeMessages` in `OpenObscureManager.swift` pre-restores prior assistant and system messages before calling the FFI. This is necessary because the SwiftData store holds the *restored* plaintext (e.g. `"Angela Martinez"`) after `handleComplete` restores the previous response. If those messages were sent through `sanitize_messages` as-is, the plaintext would get a fresh token (the same token inconsistency problem but in the other direction — PII present in context). Pre-restoring is a no-op on messages that are already plaintext; it only matters when a message contains stale tokens from a prior turn.

**3. Flush the streaming throttler buffer before calling `restore`**

Enchanted batches streaming tokens through a `Throttler` before updating the SwiftUI view. The throttler fires on a timer, so the final batch can still be sitting in `currentMessageBuffer` when the stream `.finished` completion fires. If `handleComplete` calls `restore(lastMesasge.content)` before flushing that buffer, `restore` receives a truncated response — e.g. `"The name in the record"` instead of `"The name in the record is \"PER_ud6c\"."` — and the FPE token is never seen, so the original PII value is never restored.

Fix: check and flush `currentMessageBuffer` at the top of `handleComplete`, before any restore call.

**4. Run leak scan before restore, not after**

`leakedTokenCount` must be called on the raw LLM response **before** `restore()`. If called after, known tokens have already been replaced with plaintext and won't be detected. The function also includes a regex fallback (`[A-Z]{2,4}_[a-z0-9]{4}`) that catches token-shaped strings the LLM may have hallucinated — these are never in the mapping and would be invisible to a map-only check.

```swift
func handleComplete(newResponse: String) {
    // 1. Leak scan FIRST — on raw LLM text, before any restoration
    let leaks = OpenObscureManager.shared.leakedTokenCount(in: newResponse)
    if leaks > 0 {
        print("[OO] WARNING: \(leaks) leaked token(s) in response")
    }

    // 2. Restore PII tokens → plaintext
    let restored = OpenObscureManager.shared.restore(newResponse)

    // 3. Append only the restored response to display history
    conversationHistory.append(Message(role: .assistant, content: restored))
}
```

**5. Isolate display history from request context**

The conversation history backing the UI must only receive the singular new assistant response after `restore()`. Never read from the `requestMessages` array (the sanitized context sent to the LLM) for display. Rebuild `requestMessages` fresh from `conversationHistory` at each `sendPrompt` call — it is ephemeral, not persisted.

**6. Move `sanitizeMessages` off the main thread**

`sanitizeMessages` cost grows linearly with conversation length (~80-90ms per message). By turn 6-8 this exceeds 500ms on the main thread, causing visible UI jank. Wrap the call in `Task {}` or `DispatchQueue.global().async`:

```swift
Task {
    let sanitized = OpenObscureManager.shared.sanitizeMessages(messages)
    await MainActor.run {
        // Send sanitized messages to Ollama
    }
}
```

**Diagnostic tip — draining the Rust debug log after `restore`**

`restore_text` in Rust logs match/unmatch counts via `oo_warn!` into an in-process ring buffer. On iOS, stderr goes to `/dev/null`, so these logs are invisible unless explicitly drained. Call `getDebugLog()` immediately after `restoreText(...)` and print the result to surface token match diagnostics during development.

---

## Part 5: Android Integration (Kotlin)

### 5a. Add to Android Project

1. **Copy native libraries:**

```
app/src/main/jniLibs/
├── arm64-v8a/
│   └── libopenobscure_core.so
└── x86_64/
    └── libopenobscure_core.so
```

2. **Copy Kotlin bindings:**

```
app/src/main/java/uniffi/openobscure_core/
└── openobscure_core.kt
```

3. **Add JNA dependency** in `app/build.gradle.kts`:

```kotlin
dependencies {
    implementation("net.java.dev.jna:jna:5.15.0@aar")
}
```

4. **Add ProGuard keep rules** in `proguard-rules.pro`:

```
-keep class uniffi.openobscure_core.** { *; }
-keep class com.sun.jna.** { *; }
-dontwarn com.sun.jna.**
```

5. **Verify Gradle JNI config** — RikkaHub already has both of these; confirm they are present in `app/build.gradle.kts`:

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

See full template at [templates/OpenObscureManager.kt](templates/OpenObscureManager.kt) (includes accumulated mappings, `scanResponse()`, `resetMappings()`, recursive `copyAssetsDir()`, and `getDebugLog()` diagnostics).

```kotlin
import android.util.Log
import uniffi.openobscure_core.*

object OpenObscureManager {
    private var _handle: OpenObscureHandle? = null
    private val accumulatedMappings = mutableListOf<List<String>>()

    fun init(context: Context) {
        if (_handle != null) return
        val key = getOrCreateKey(context)
        val modelsDir = copyAssetsDir(context, "models")
        _handle = createOpenobscure(
            configJson = """{"scanner_mode": "auto", "models_base_dir": "$modelsDir"}""",
            fpeKeyHex = key
        )
        // Drain Rust-side debug log — model loading diagnostics
        val debugLog = getDebugLog()
        if (debugLog.isNotEmpty()) Log.d("OpenObscure", debugLog)
    }

    val handle: OpenObscureHandle
        get() = _handle ?: error("OpenObscureManager.init() not called")

    fun sanitize(text: String): SanitizeResultFfi {
        val result = sanitizeText(handle, text)
        if (result.piiCount > 0u) mergeMappings(result.mappingJson)
        return result
    }

    fun restore(text: String): String {
        val json = /* serialize accumulatedMappings */ "{}"
        return restoreText(handle, text, json)
    }

    fun scanResponse(text: String): RiReportFfi? {
        return uniffi.openobscure_core.scanResponse(handle, text)
    }

    fun resetMappings() { accumulatedMappings.clear() }

    // See template for full mergeMappings(), copyAssetsDir(), getOrCreateKey()
    private fun mergeMappings(json: String) { /* ... */ }
    private fun copyAssetsDir(context: Context, dir: String): String { /* ... */ }
    private fun getOrCreateKey(context: Context): String { /* ... */ }
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
import uniffi.openobscure_core.*
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
// In buildMessages() (line ~399), wrap each user message:
val result = sanitizeText(OpenObscureManager.handle, userText)
// Use result.sanitizedText instead of userText in the JSON builder

// In parseMessage() (line ~590), restore response text:
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
| Multilingual national IDs (9 langs) | Yes | Yes | Yes |
| Name gazetteer (common names) | Yes | Yes | Yes |
| Person names (semantic) | -- | Yes | Yes |
| Locations, Organizations | -- | Yes | Yes |
| Image face solid fill | -- | -- | Yes |
| Image OCR solid fill (full scanner) | -- | -- | Yes |
| Screenshot detection | -- | -- | Yes |
| Cognitive firewall (R1+R2) | Yes (R1) | Yes (R1) | Yes (R1+R2) |
| Audio transcript PII | Yes | Yes | Yes |
| FPE key rotation (30s overlap) | Yes | Yes | Yes |

**Recommendation:** Bundle all models with `scanner_mode: "auto"` and `models_base_dir`. The tier system dynamically loads only what the device can handle — unused models stay on disk, not in RAM.

---

## Part 6a: Bundling All Models (Recommended)

The simplest approach: bundle all model files and let OpenObscure's tier system decide what to load based on device RAM. Models that aren't activated by the device budget are never loaded into memory.

### Model Directory Layout

Use the bundling script to copy models with correct directory naming:

```bash
# Bundle all models from dev repo to app resources
./build/bundle_models.sh /path/to/your/app/models

# Example for Enchanted (cloned as ~/Test/enchanted)
./build/bundle_models.sh ~/Test/enchanted/models
```

The script maps dev repo directory names to the standard names expected by `models_base_dir` auto-resolution (e.g., `paddleocr` → `ocr`, `ner-lite` → `ner_lite`). It verifies all expected subdirectories exist after copying.

Alternatively, copy the `models/` directory manually from `openobscure-core/models/` into your app resources:

```
models/
├── ner/               — DistilBERT NER (~64 MB, Full tier — optional)
│   ├── model_int8.onnx
│   ├── vocab.txt
│   ├── label_map.json
│   ├── config.json
│   ├── tokenizer.json
│   ├── tokenizer_config.json
│   └── special_tokens_map.json
├── ner_lite/          — TinyBERT NER (~14 MB, Standard/Lite tier)
│   ├── model_int8.onnx
│   ├── vocab.txt
│   ├── label_map.json
│   ├── config.json
│   ├── tokenizer.json
│   ├── tokenizer_config.json
│   └── special_tokens_map.json
├── scrfd/             — SCRFD face detection (~3.1 MB, Standard/Full tier)
│   └── scrfd_2.5g.onnx
├── blazeface/         — BlazeFace face detection (~408 KB, Lite tier fallback)
│   └── blazeface.onnx
├── ocr/               — PaddleOCR v4 text detection + recognition (~9.7 MB)
│   ├── det_model.onnx
│   ├── rec_model.onnx
│   └── ppocr_keys.txt
├── nsfw_classifier/   — ViT-base NSFW classifier (~83 MB INT8, Apache-2.0)
│   └── nsfw_classifier.onnx
└── ri/                — R2 response integrity classifier (~14 MB, optional)
    ├── model_int8.onnx
    └── vocab.txt
```

**Total size on disk: ~125 MB** (without DistilBERT NER). With `ner/` (~64 MB) for Full-tier DistilBERT NER: **~190 MB**.

> **Recommended:** Bundle both `ner/` and `ner_lite/`. The tier system loads only one based on device RAM. If only one is bundled, the NER loader automatically falls back to whichever is available.

### Config

```json
{"scanner_mode": "auto", "models_base_dir": "<path_to_models>"}
```

### iOS/macOS Setup

1. Copy the `models/` folder into your Xcode project root
2. In Xcode: right-click the project navigator → **Add Files** → select `models/` → check **Create folder references** (blue folder icon, not yellow group)
3. Verify the `models` folder appears in **Build Phases → Copy Bundle Resources**

```swift
let modelsDir = Bundle.main.resourcePath! + "/models"
let config = """
{"scanner_mode": "auto", "models_base_dir": "\(modelsDir)"}
"""
let handle = try createOpenobscure(configJson: config, fpeKeyHex: key)

// Check model loading diagnostics (especially useful on iOS where stderr is silent)
let debugLog = getDebugLog()
print("[OpenObscure] \(debugLog)")
```

### Android Setup

1. Copy model subdirectories to `app/src/main/assets/models/`
2. At runtime, copy from assets to internal storage (ONNX Runtime needs file paths):

```kotlin
val modelsDir = copyAssetsDir(context, "models")
val config = """{"scanner_mode": "auto", "models_base_dir": "$modelsDir"}"""
val handle = createOpenobscure(configJson = config, fpeKeyHex = key)
```

### What Gets Loaded by Tier

| Device RAM | Tier | NER | Face | OCR | NSFW | R2 |
|-----------|------|-----|------|-----|------|----|
| ≥8 GB | Full | DistilBERT (or TinyBERT) | SCRFD | Full recognition | Yes | Yes |
| 4–8 GB | Standard | TinyBERT | SCRFD | Detect + fill | Yes (if budget ≥150 MB) | Yes |
| <4 GB | Lite | TinyBERT | BlazeFace | Detect + fill | No | No |

Models are loaded **on-demand** when first needed and **evicted after idle timeout** (60–300s depending on tier) to free RAM. EXIF metadata is always stripped from images regardless of which models are loaded.

### Platform-Specific Execution Providers

On mobile, OpenObscure uses hardware-accelerated ONNX Runtime execution providers where available:

| Platform | EP | Details |
|----------|-----|---------|
| **iOS** | CoreML (NeuralNetwork + CPUAndGPU) | CNN models (SCRFD, OCR, NSFW) use CoreML for GPU acceleration. ANE is skipped (some devices report unknown subtype). |
| **macOS** | CoreML (default) | MLProgram format + all compute units (ANE/GPU/CPU) |
| **Android** | NNAPI | Qualcomm Hexagon / Mali GPU acceleration |
| **Other** | CPU | Default fallback |

**NER models always use CPU-only inference** — CoreML (even NeuralNetwork format) fails to load TinyBERT/DistilBERT transformer architectures on iOS. At 0.8ms p50 on CPU, there is no meaningful latency difference.

### NER Model Fallback

If the budget-selected NER model directory is missing, the loader automatically falls back:
- **DistilBERT selected** (Full tier) but `ner/` missing → falls back to `ner_lite/` (TinyBERT)
- **TinyBERT selected** (Standard/Lite) but `ner_lite/` missing → falls back to `ner/` (DistilBERT)

This means bundling only one NER variant still works — the tier system adapts. Check `getDebugLog()` output to see which model was actually loaded.

### NSFW Model License

The NSFW classifier (`nsfw_classifier/nsfw_classifier.onnx`) is Apache-2.0 licensed — no copyleft obligations.

---

## Part 6b: Adding NER Only (Minimal)

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

The model files ship with OpenObscure under `openobscure-core/models/ner/` (DistilBERT) and `openobscure-core/models/ner-lite/` (TinyBERT). Copy the appropriate directory into your app.

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

**Option C: `models_base_dir` (simplest)** — point to a single directory containing all model subdirectories:

```json
{
  "scanner_mode": "auto",
  "auto_detect": true,
  "models_base_dir": "/path/to/models"
}
```

When `models_base_dir` is set, OpenObscure auto-resolves `ner_model_dir` from `<base>/ner/`, `ner_model_dir_lite` from `<base>/ner_lite/`, and so on for all model directories. Only subdirectories that exist on disk are used. Explicit per-model paths (e.g., `"ner_model_dir": "/custom/path"`) always override auto-resolved paths.

In auto mode:
- **Full tier (≥8 GB RAM):** Uses DistilBERT from `ner_model_dir`
- **Standard tier (4–8 GB):** Uses TinyBERT from `ner_model_dir_lite`
- **Lite tier (<4 GB):** Falls back to regex-only (no NER model loaded)

If only `ner_model_dir` is set (no `_lite` variant), all tiers that enable NER use that single model. If only `ner_model_dir_lite` is set, the TinyBERT path is used as fallback for all tiers. The NER loader has automatic fallback: if the budget-selected model directory is missing, it tries the other variant before giving up.

> **Note:** NER models always run on CPU, even on iOS/Android where CoreML/NNAPI is available. CoreML cannot load transformer architectures (TinyBERT/DistilBERT). At 0.8ms p50, CPU inference is fast enough.

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

**Swift alternative — `models_base_dir`:** If all model directories are bundled under a single `Models/` folder:

```swift
let modelsBase = Bundle.main.path(forResource: "Models", ofType: nil)!
let config = """
{"scanner_mode": "auto", "auto_detect": true, "models_base_dir": "\(modelsBase)"}
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

**Kotlin alternative — `models_base_dir`:** If all model directories are copied under a single `models/` folder:

```kotlin
val modelsBase = copyAssetsDir(context, "models")
val config = """{"scanner_mode": "auto", "auto_detect": true, "models_base_dir": "$modelsBase"}"""
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
5. Call `getDebugLog()` after init — look for `"NER budget: ... dir=None"` or `"NER model load FAILED"` messages
6. If using `models_base_dir`, verify subdirectory names match: `ner/` (not `ner-distilbert/`) and `ner_lite/` (not `ner-lite/`). Use `bundle_models.sh` to ensure correct naming.

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

- `ri_enabled` — Enable/disable the cognitive firewall (default: `true`; device budget gates actual activation)
- `ri_sensitivity` — `"off"`, `"low"`, `"medium"` (default), `"high"` — controls when R2 is invoked
- `ri_model_dir` — Path to R2 model directory (optional — R1 works without it)

**R2 cascade behavior:** When R2 disagrees with R1, the cascade role depends on the strength of R1 evidence. If R1 flagged matches across **2 or more** persuasion categories, R2 disagreement is treated as Confirm (strong R1 evidence stands). Single-category R1 hits may be suppressed by R2 (Suppress role). This prevents false-negative suppression of genuine multi-vector persuasion attempts.

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

# Proxy unit tests (1,677 tests including mobile API)
cargo test --manifest-path openobscure-core/Cargo.toml --lib --all-features
```

---

## Part 8: Troubleshooting

| Issue | Cause | Fix |
|-------|-------|-----|
| `MobileBindingError` on init | Invalid FPE key (must be exactly 64 hex chars = 32 bytes) | Check key length and hex format |
| Linker error: `_openobscure_core_*` | Library not linked | Add `-lopenobscure_core` to Other Linker Flags |
| `UnsatisfiedLinkError` on Android | `.so` not in correct ABI folder | Verify `jniLibs/<abi>/libopenobscure_core.so` path |
| JNA not found on Android | Missing dependency | Add `implementation("net.java.dev.jna:jna:5.15.0@aar")` |
| Image sanitization fails | No model files on disk | Provide face/OCR model paths in config (or use `models_base_dir`). EXIF is still stripped even without models. |
| GPS/EXIF leaks in LLM response | Models dir missing or not in app bundle | Verify `models/` is added as a **folder reference** (blue icon) in Xcode, not a group (yellow icon). Check debug log for `models_dir:` path. EXIF stripping is always active — if GPS leaks, `sanitizeImage()` may be failing silently (check for catch blocks using original image). |
| Image/RI features active unexpectedly | `image_enabled` and `ri_enabled` now default to `true` | Set `"image_enabled": false` or `"ri_enabled": false` explicitly in config JSON to disable |
| 0 PII detected for names | Regex mode can't detect names | Switch to `scanner_mode: "auto"` with `models_base_dir` pointing to bundled NER model |
| `Cannot find 'createOpenobscure' in scope` (Swift) | Missing import | Add `import OpenObscureLib` to the file |
| Type mismatch `UInt64` vs `OpenObscureHandle` | UniFFI generates an opaque class | Use `OpenObscureHandle` type, not `UInt64` |
| Type mismatch `Int32` vs `UInt32` for `piiCount` | UniFFI uses unsigned types | `piiCount` is `UInt32` in Swift, compare with `> 0u` in Kotlin |
| macOS `NSImage` has no `jpegData()` | iOS-only API | Use `compressImageData()` which works on both iOS and macOS |
| `sherpa-rs-sys` linker failure on iOS/Android | Voice feature pulls in native deps | Build with `--no-default-features --features mobile` |
| `cargo-ndk --manifest-path` fails | `cargo-ndk` doesn't support this flag | Use `(cd proxy-dir && cargo ndk ...)` subshell pattern |
| JitPack dependency timeout | JitPack.io outage | Clone repos locally, `publishToMavenLocal`, add `mavenLocal()` before JitPack in `settings.gradle.kts` |
| `ort-sys` no prebuilt for `x86_64-linux-android` | Expected limitation | Only build `arm64-v8a` for real devices; x86_64 is emulator-only |
| `scannerMode` is `"regex"` but expected `"ner"` | NER model dir missing or not resolved | Check `getDebugLog()` for `"NER budget: ... dir=None"`. Ensure both `ner/` and `ner_lite/` are bundled, or use `models_base_dir` for auto-resolution. Run `bundle_models.sh` to ensure correct directory naming. |
| CoreML Conv padding warnings on iOS | MLProgram format incompatible with CNN models | Already fixed in OpenObscure — iOS uses NeuralNetwork format + CPUAndGPU. If you see these warnings, rebuild with the latest `.a` file and clear Xcode DerivedData: `rm -rf ~/Library/Developer/Xcode/DerivedData/YourApp-*` |
| NER model fails to load on iOS | CoreML can't handle transformer architectures | Already fixed — NER uses CPU-only inference on all platforms. Check `getDebugLog()` for `"NER model load FAILED"` messages. |
| Xcode links stale binary after rebuild | DerivedData caching | Delete DerivedData: `rm -rf ~/Library/Developer/Xcode/DerivedData/YourApp-*`, then rebuild. Also re-resolve packages: `xcodebuild -resolvePackageDependencies` |
| No Rust debug output on iOS device | `stderr` goes to `/dev/null` on iOS | Use `getDebugLog()` to retrieve Rust-side diagnostics. Call after `createOpenobscure()` or after operations that may fail. |

---

## Reference: Tested Third-Party Apps

| App | Platform | Integration Approach | Build Status | Key Files Modified |
|-----|----------|---------------------|-------------|---------------------|
| **Enchanted** | iOS/macOS | Local SPM package + direct API calls | **BUILD SUCCEEDED** (macOS ad-hoc) | `OpenObscureManager.swift` (new), `ConversationStore.swift` (send/receive/image), `InputFields_macOS.swift` (speech), `ChatView_iOS.swift` (speech), `project.pbxproj` |
| **RikkaHub** | Android | OkHttp interceptor + JNI/JNA | **BUILD SUCCEEDED** (debug APK 76MB arm64) | `OpenObscureManager.kt` (new), `OpenObscureInterceptor.kt` (new), `DataSourceModule.kt` (interceptor wire), `RikkaHubApp.kt` (init), `build.gradle.kts` (JNA dep), `proguard-rules.pro` |
| **OpenClaw** | Desktop | Gateway proxy (see [gateway setup](../../get-started/gateway-quick-start.md)) | Verified | Config only — point `LLM_API_BASE` at proxy |

### Integration Artifacts

| Artifact | Enchanted (macOS) | RikkaHub (Android) |
|----------|-------------------|-------------------|
| Fork location | `/Users/admin/Test/enchanted-openobscure/` | `/Users/admin/Test/rikkahub-openobscure/` |
| Native library | 158MB static `.a` (macOS) / 160MB (iOS) | 24MB `.so` (arm64-v8a) |
| Bindings | `openobscure_core.swift` + FFI header + modulemap | `openobscure_core.kt` (UniFFI) |
| Key storage | iOS Keychain | Android SharedPreferences |
| Intercept pattern | Direct API calls in ConversationStore | OkHttp Interceptor on request JSON |
| Build output | Xcode build (CODE_SIGNING_ALLOWED=NO) | `app-arm64-v8a-debug.apk` (76MB) |

> **⚠ Android key storage:** The RikkaHub reference integration uses `SharedPreferences`, which stores the FPE key in plaintext on the device filesystem. For production use, replace with `EncryptedSharedPreferences` backed by the Android Keystore. Plain `SharedPreferences` is acceptable for development and testing only.

---

## Reference: Build Scripts

| Script | What it builds | Output |
|--------|---------------|--------|
| `build/build_ios.sh` | iOS static libs + XCFramework | `.a` files + `.xcframework` |
| `build/build_android.sh` | Android shared libs | `.so` per ABI |
| `build/build_napi.sh` | Node.js native addon | `scanner.node` |
| `build/generate_bindings.sh` | UniFFI Swift + Kotlin bindings | `bindings/swift/`, `bindings/kotlin/` |
| `build/bundle_models.sh` | Copy & rename models for embedded apps | `<output_dir>/{ner,ner_lite,scrfd,blazeface,ocr,nsfw,nsfw_classifier,ri}/` |
| `build/download_models.sh` | ONNX model files | `openobscure-core/models/` |
