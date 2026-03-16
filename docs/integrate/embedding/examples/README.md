# Integration Examples

Example diffs showing how OpenObscure was integrated into two third-party chat apps.
These diffs are provided as reference — adapt the patterns to your own app.

## Enchanted (iOS/macOS — Ollama client)

- **Upstream**: [AugustDev/enchanted](https://github.com/AugustDev/enchanted)
- **Base commit**: `2f82ee2518c63fa7347c9e8e8e5a131ee0b75cbe` (feat: colour adjustments (#194))
- **Diff**: [enchanted-openobscure.diff](enchanted-openobscure.diff)

**What the diff covers:**
- `ConversationStore.swift` — outbound message sanitization, inbound response restoration, image sanitization via `sanitizeImage()`, cognitive firewall via `scanResponse()`
- `ChatView_iOS.swift` — speech transcript sanitization on iOS
- `InputFields_macOS.swift` — speech transcript sanitization on macOS

**Not included in the diff** (must be set up separately):
- `OpenObscureManager.swift` — see [templates/](../templates/OpenObscureManager.swift) (includes accumulated mappings, `scanResponse()`, `resetMappings()`, and `getDebugLog()` diagnostics)
- Local SPM package (`OpenObscureLib`) wrapping UniFFI bindings + static library
- Xcode project changes (`.pbxproj`) — add the local package dependency manually
- `models/` folder reference in Xcode (add as folder reference → Copy Bundle Resources) — use `build/bundle_models.sh` to prepare, see [Integration Guide Part 6a](../embedded_integration.md#part-6a-bundling-all-models-recommended)

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

**Not included in the diff** (must be set up separately):
- `OpenObscureManager.kt` — see [templates/](../templates/OpenObscureManager.kt) (includes accumulated mappings, `scanResponse()`, `resetMappings()`, recursive `copyAssetsDir()`, and `getDebugLog()` diagnostics)
- `OpenObscureInterceptor.kt` — see [templates/](../templates/OpenObscureInterceptor.kt)
- UniFFI Kotlin bindings (`uniffi/openobscure_core/openobscure_core.kt`)
- Native `.so` library in `jniLibs/arm64-v8a/`
- Model files in `assets/models/` — use `build/bundle_models.sh` to prepare, see [Integration Guide Part 6a](../embedded_integration.md#part-6a-bundling-all-models-recommended)
