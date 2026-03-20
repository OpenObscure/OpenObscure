# Integration Examples

Example diffs showing how OpenObscure was integrated into two third-party chat apps.
These diffs are provided as reference — adapt the patterns to your own app.

For detailed architecture and sequence diagrams, see [Embedded Architecture](../architecture.md).

## Enchanted (iOS/macOS — Ollama client)

- **Upstream**: [AugustDev/enchanted](https://github.com/AugustDev/enchanted)
- **Base commit**: `2f82ee2518c63fa7347c9e8e8e5a131ee0b75cbe` (feat: colour adjustments (#194))
- **Diff**: [enchanted-openobscure.diff](enchanted-openobscure.diff)

**What the diff covers:**
- `project.pbxproj` — wires `OpenObscureLib` (local SPM package), `OpenObscureManager.swift` (Sources), and `OpenObscureModels` (Resources) into the Xcode project
- `ConversationStore.swift` — batch `sanitizeMessages()`, streaming buffer flush, `@Transient displayContent` (DB stores FPE tokens, UI shows restored text), `restoreMessagesForDisplay()`, image sanitization via `sanitizeImage()`, cognitive firewall via `scanResponse()`
- `MessageSD.swift` — adds `@Transient displayContent` (display-only, never persisted) and `sanitizedContent` (cached sanitized text to skip NER on subsequent turns)
- `ConversationSD.swift` — adds `mappingJson` field for session mapping persistence across app restarts
- `MessageListVIew.swift` — direct `@Observable` tracking of `conversationState` (fixes SwiftUI observation miss on `.loading→.completed`)
- `ApplicationEntry.swift` — eager `OpenObscureManager.shared` init to avoid main-thread model loading stall
- `ChatView_iOS.swift` — speech transcript sanitization on iOS, `conversationState` parameter removal
- `ChatView_macOS.swift` — `conversationState` parameter removal
- `InputFields_macOS.swift` — speech transcript sanitization on macOS

> **Code signing note:** The diff includes `DEVELOPMENT_TEAM` and `PRODUCT_BUNDLE_IDENTIFIER`
> values specific to the author's environment. After applying the diff, you **must** change
> these to match your own Apple Developer team and bundle identifier. See
> [Embedded Setup — Step 4](../../../setup/embedded_setup.md#step-4--open-in-xcode-and-build)
> for instructions.

**Not included in the diff** (must be set up separately):
- `OpenObscureManager.swift` — see [templates/](../templates/OpenObscureManager.swift) (copied into `Enchanted/` by the setup guide)
- Local SPM package (`OpenObscureLib/`) wrapping UniFFI bindings + static library (created by the setup guide)
- `OpenObscureModels/` folder with ONNX models — use `build/bundle_models.sh` to prepare, see [Integration Guide Part 6a](../embedded_integration.md#part-6a-bundling-all-models-recommended)

## RikkaHub (Android — multi-provider LLM client)

- **Upstream**: [rikkahub/rikkahub](https://github.com/rikkahub/rikkahub)
- **Base commit**: `7e224767dcac8e76d21a57c74790089214e15d28` (fix(chat): keep assistant in sync when moving conversation)
- **Diff**: [rikkahub-openobscure.diff](rikkahub-openobscure.diff)

**What the diff covers:**
- `build.gradle.kts` — JNA dependency for UniFFI
- `proguard-rules.pro` — keep rules for UniFFI + JNA
- `RikkaHubApp.kt` — `OpenObscureManager.init(this)` in `Application.onCreate()`
- `DataSourceModule.kt` — wire `OpenObscureInterceptor` into OkHttp chain
- `settings.gradle.kts` — `mavenLocal()` before JitPack (build reliability)
- `OpenObscureManager.kt` — singleton wrapping Rust FFI (sanitize cache, isolated sanitization for auto-gen requests, RI warning storage, `sanitizeImage()`, `getMappingsJson()` / `loadMappings()`, and `getDebugLog()` diagnostics). Also available as [template](../templates/OpenObscureManager.kt).
- `OpenObscureInterceptor.kt` — OkHttp interceptor, outbound-only (multimodal image sanitization, auto-gen request detection with mapping isolation). Also available as [template](../templates/OpenObscureInterceptor.kt).
- `ChatService.kt` — restore + RI scan in `onSuccess`, mapping persistence
- `ChatMessage.kt` — `rememberRestoredText()` with RI warning label + `riVersion` recompose
- `ChatVM.kt` — `resetMappings()` + `loadMappings()` on conversation switch
- `ConversationEntity.kt` — `mappingJson` column for session mapping persistence
- `AppDatabase.kt` — Room migration 17 to 18

**Not included in the diff** (must be set up separately):
- UniFFI Kotlin bindings (`uniffi/openobscure_core/openobscure_core.kt`)
- Native `.so` libraries in `jniLibs/arm64-v8a/` (`libopenobscure_core.so` + `libonnxruntime.so`)
- Model files in `assets/models/` — use `build/bundle_models.sh` to prepare, see [Integration Guide Part 6a](../embedded_integration.md#part-6a-bundling-all-models-recommended)

## Verified Features (Both Platforms)

All features tested on iOS (iPhone 17 + Ollama llava:13b) and Android (Samsung + GPT-4o).
Full test results in [review-notes/embedded_integration_review_2026-03-19.md](../../../../review-notes/embedded_integration_review_2026-03-19.md).

| Feature | iOS | Android | Notes |
|---------|-----|---------|-------|
| PII text sanitization (11 types) | Pass | Pass | SSN, phone, email, name, GPS, health keywords |
| Stable FPE tokens across turns | Pass | Pass | Same token for same plaintext within session |
| PII leak prevention | Pass | Pass | LLM spells token chars, never real name |
| Sanitize cache (constant latency) | Pass | Pass | 0ms on turns 3+ (all cached) |
| NSFW detection + solid fill | Pass | Pass | 0.998 confidence, full-image redaction |
| Face redaction (SCRFD) | Pass | Pass | 1-3 faces, selective solid fill |
| OCR pre-filter (inverted band) | Pass | Pass | Skips photos (density > 0.12), scans documents |
| Cognitive firewall R1+R2 | Pass | Pass | Caution on marketing/persuasion content |
| RI warning label in UI | Pass | Pass | Prepended to flagged responses |
| Image sanitization (multimodal) | Pass | Pass | Base64 + URL fetch on Android |
| Session mapping persistence | Pass | Pass | mappingJson survives app restart |
| Conversation switch isolation | Pass | Pass | resetMappings on switch, no cross-leak |
| Auto-gen request isolation | N/A | Pass | Title/suggestion use disposable tokens |

## Latency Comparison

| Operation | iOS (iPhone 17) | Android (Samsung) |
|-----------|----------------|-------------------|
| NER DistilBERT (per new msg) | ~80ms | ~580ms |
| Cache hit (prior msg) | 0ms | 0ms |
| NSFW classifier | ~2200ms | ~1100ms |
| Face detection (SCRFD) | ~1950ms | ~1240ms |
| OCR pre-filter skip | ~339ms | ~312ms |
| R2 TinyBERT | ~170ms | ~515ms |
| Restore (per msg) | ~1-2ms | ~2-4ms |

## Known Limitations

1. **Android NER 7x slower than iOS** — CPU-only (NNAPI tested: slower + crashes SCRFD).
   See [review notes](../../../../review-notes/embedded_integration_review_2026-03-19.md#post-launch-android-acceleration-options) for post-launch acceleration options.

2. **RikkaHub image attachment for Ollama** — RikkaHub sends `[Image]` placeholder
   instead of base64 for Ollama provider. Works correctly with OpenAI-compatible APIs (GPT-4o).

3. **OCR 4.4s on document images** — PaddleOCR detector on CPU. Pre-filter skips photos
   but documents with text take full inference. Future: CoreML/GPU acceleration or lighter model.

4. **Title gen error on RikkaHub** — HTTP 403 `NOT_ENOUGH_BALANCE` from `api.rikka-ai.com`.
   RikkaHub billing issue, not OpenObscure.
