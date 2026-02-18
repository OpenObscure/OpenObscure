# Phase 7: Cross-Platform Support — Windows, Linux ARM, iOS, Android

**Status: PLANNED**

## Context

OpenClaw (OpenObscure's primary integration target) supports **Android and iOS** via native companion apps (iOS: Swift+SwiftUI, Android: Kotlin+Compose). OpenObscure's 275MB RAM / 70MB storage ceilings were designed with mobile-class devices in mind, but the codebase currently only builds and tests on macOS and Linux x86_64.

### How OpenClaw Mobile Architecture Works

OpenClaw uses a **hub-and-spoke** model:

```
                    +------------------+
                    |   LLM Providers  |
                    | (Claude, GPT...) |
                    +--------+---------+
                             |
                    +--------v---------+
   +-------+       |                   |       +---------+
   | macOS |<----->|     Gateway       |<----->| Android |
   |  App  | WS    | (Node.js 22+)    | WS    |  Node   |
   +-------+       | ws://127.0.0.1   |       +---------+
                    |    :18789        |
   +-------+       |                   |       +---------+
   |  iOS  |<----->| - Agent Runtime   |<----->| WebChat |
   |  Node | WS    | - Channel Adapters| HTTP  |   UI    |
   +-------+       | - Tool Registry   |       +---------+
                    | - Plugin Loader  |
                    +--------+---------+
                             |
              +--------------+--------------+
              |              |              |
         +----v----+   +----v----+   +-----v-----+
         | WhatsApp|   |Telegram |   | Discord   |
         | Slack   |   | Signal  |   | Teams ... |
         +---------+   +---------+   +-----------+
```

- **Gateway** (Node.js 22+) runs on macOS/Linux/Windows — this is the brain
- **Mobile apps** are **companion nodes** connecting to the Gateway via WebSocket
- Mobile apps discover Gateway via Bonjour/mDNS (`_openclaw-gw._tcp`, port 18790)
- Mobile apps expose device capabilities (camera, canvas, location, contacts) to the agent
- Mobile apps do NOT run the LLM pipeline — the Gateway does
- Session sharing: all clients see same conversation history
- Remote access possible via Tailscale or SSH tunnels

### Where OpenObscure Fits

```
Gateway-side (Phase 7A):
  Mobile App ──WS──▶ Gateway ──HTTP──▶ [OpenObscure Proxy] ──▶ LLM Provider
                                        ↑ runs on same host as Gateway

Mobile-embedded (Phase 7B):
  Mobile App ──▶ [OpenObscure lib] ──WS──▶ Gateway ──HTTP──▶ LLM Provider
                  ↑ runs on-device, sanitizes before reaching Gateway
```

**Phase 7A** ensures OpenObscure runs wherever the Gateway runs (Windows + Linux ARM in addition to existing macOS + Linux x86_64).

**Phase 7B** embeds OpenObscure as a library in mobile apps, sanitizing PII *before* it even reaches the Gateway over WebSocket — defense in depth.

---

## Current Platform-Specific Code (11 locations)

| File | What | Platforms |
|------|------|-----------|
| `Cargo.toml:40` | `keyring = { features = ["apple-native"] }` | macOS-only feature |
| `Cargo.toml:72-76` | `tracing-oslog` (macOS), `tracing-journald` (Linux) | macOS + Linux only |
| `src/main.rs:424-448` | `init_platform_log_layer()` with 3 `#[cfg]` branches | macOS + Linux + fallback |
| `src/main.rs:451-457` | `resolve_crash_buffer_path()` — HOME/USERPROFILE | macOS + Linux + Windows |
| `src/main.rs:585-617` | Auth token path + `#[cfg(unix)]` permissions | Unix-only perms |
| `src/crf_scanner.rs:474-511` | `available_ram_mb()` — vm_stat (macOS) / /proc (Linux) | macOS + Linux only |
| `src/crf_scanner.rs:490` | `libc::sysconf(libc::_SC_PAGESIZE)` | POSIX only |
| `src/vault.rs` | `keyring::Entry` (platform auto-selected) | macOS + Linux + Windows |
| `src/health.rs` | Crash marker file path | HOME/USERPROFILE |
| `openobscure-plugin/src/index.ts:74` | `process.env.HOME \|\| process.env.USERPROFILE` | All desktop |
| `openobscure-plugin/package.json` | `better-sqlite3` native addon | Desktop only (no mobile) |

---

## Phase 7A: Gateway-Side Cross-Platform (Windows + Linux ARM)

**Goal:** OpenObscure runs wherever the OpenClaw Gateway runs.
**Effort:** ~3-4 days code, ~1 day testing

### A1. Windows RAM Detection

**File:** `openobscure-proxy/src/crf_scanner.rs` (lines 474-511)

Add Windows implementation:
```rust
#[cfg(target_os = "windows")]
{
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut status = MEMORYSTATUSEX::default();
    status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    if unsafe { GlobalMemoryStatusEx(&mut status) }.is_ok() {
        return Some(status.ullAvailPhys / (1024 * 1024));
    }
    None
}
```

**Cargo.toml addition:**
```toml
[target.'cfg(target_os = "windows")'.dependencies]
windows = { version = "0.59", features = ["Win32_System_SystemInformation"] }
```

### A2. Windows Logging Backend

**File:** `openobscure-proxy/src/main.rs` (lines 424-448)

Add ETW (Event Tracing for Windows) logging:

```toml
[target.'cfg(target_os = "windows")'.dependencies]
tracing-etw = "0.2"
```

```rust
#[cfg(target_os = "windows")]
fn init_platform_log_layer() -> Option<tracing_etw::LayerBuilder> {
    match tracing_etw::LayerBuilder::new("OpenObscure").build() {
        Ok(layer) => {
            eprintln!("[OpenObscure] ETW tracing enabled");
            Some(layer)
        }
        Err(e) => {
            eprintln!("[OpenObscure] Failed to init ETW: {}", e);
            None
        }
    }
}
```

### A3. Keyring Feature Restructure

**File:** `openobscure-proxy/Cargo.toml` (line 40)

Current: `keyring = { version = "3", features = ["apple-native"] }` — forces macOS-specific feature flag. Won't compile on Windows/Linux without the feature being available.

Fix: Make `apple-native` conditional:

```toml
# Base dependency (works on all platforms)
keyring = "3"
```

```toml
# macOS-specific: use native Keychain API (faster than Security Framework)
[target.'cfg(target_os = "macos")'.dependencies]
keyring = { version = "3", features = ["apple-native"] }
```

**Note:** The `keyring` crate v3 auto-selects platform backends:
- macOS: Keychain (native with `apple-native` feature, Security Framework without)
- Linux: `keyutils`, `pass`, or `secret-service` (D-Bus)
- Windows: Credential Manager (via `winapi`)
- iOS/Android: NOT supported (env var fallback needed)

### A4. Linux ARM64 Verification

**No code changes.** The codebase already supports Linux via `/proc/meminfo` + `tracing-journald`. Just verify the build:

```bash
rustup target add aarch64-unknown-linux-gnu
# cargo-zigbuild for glibc version control:
cargo install cargo-zigbuild
pip3 install ziglang
cargo zigbuild --target aarch64-unknown-linux-gnu.2.17 --release
```

The `ort` crate with `download-binaries` includes ARM64 Linux ONNX Runtime prebuilts.

### A5. File Permissions

The existing code uses `#[cfg(unix)]` for auth token permission setting (mode 0o600). Non-Unix platforms skip this silently. **No change needed** — Windows default ACLs are acceptable for user-profile directories.

### A6. Path Resolution

Already handles `HOME` (Unix) and `USERPROFILE` (Windows). **No change needed.**

### A7. CI/CD — GitHub Actions Matrix

Add cross-platform build verification to CI:

```yaml
# .github/workflows/build.yml
strategy:
  matrix:
    include:
      - os: macos-latest
        target: aarch64-apple-darwin
      - os: ubuntu-latest
        target: x86_64-unknown-linux-gnu
      - os: ubuntu-latest
        target: aarch64-unknown-linux-gnu
        cross: true
      - os: windows-latest
        target: x86_64-pc-windows-msvc
```

### A8. Files Changed (Phase 7A)

| File | Action | Est. Lines |
|------|--------|-----------|
| `openobscure-proxy/Cargo.toml` | Add Windows deps, restructure keyring | ~10 |
| `openobscure-proxy/src/crf_scanner.rs` | Add `#[cfg(target_os = "windows")]` RAM detection | ~15 |
| `openobscure-proxy/src/main.rs` | Add `#[cfg(target_os = "windows")]` ETW logging layer | ~15 |
| `.github/workflows/build.yml` | NEW — cross-platform CI matrix | ~60 |

---

## Phase 7B: Mobile-Embedded Library (iOS + Android)

**Goal:** OpenObscure runs on-device inside the OpenClaw companion app, sanitizing PII before it reaches the Gateway.
**Effort:** ~10-14 days (iOS: 5-6 days, Android: 5-8 days)

### B1. Library Mode Architecture

The Rust proxy today is a **standalone binary** (axum HTTP server). For mobile, we need a **library** that the host app calls directly — no HTTP server, no socket, just function calls.

**New file:** `openobscure-proxy/src/lib_mobile.rs`

```rust
/// Mobile-facing API for OpenObscure.
/// Called via UniFFI-generated Swift/Kotlin bindings.
pub struct OpenObscureMobile {
    scanner: HybridScanner,
    fpe: FpeEngine,
    image_pipeline: Option<ImageModelManager>,
}

impl OpenObscureMobile {
    /// Initialize with config JSON and FPE key (passed from host app's secure storage)
    pub fn new(config_json: &str, fpe_key: &[u8]) -> Result<Self, MobileError> { ... }

    /// Scan text for PII and encrypt with FF1 FPE
    pub fn sanitize_text(&self, text: &str) -> SanitizeResult { ... }

    /// Process image for visual PII (face blur, OCR blur, EXIF strip)
    pub fn sanitize_image(&self, image_bytes: &[u8]) -> Result<Vec<u8>, MobileError> { ... }

    /// Decrypt FPE values in response text
    pub fn restore_text(&self, text: &str) -> String { ... }

    /// Get scanner stats (for diagnostics UI)
    pub fn stats(&self) -> MobileStats { ... }
}
```

**Cargo.toml changes:**
```toml
[lib]
name = "openobscure"
crate-type = ["lib", "staticlib", "cdylib"]
# lib = Rust tests, staticlib = iOS (.a), cdylib = Android (.so)

[[bin]]
name = "openobscure-proxy"
path = "src/main.rs"

[features]
default = ["server", "image-pipeline"]
server = ["axum", "hyper", "hyper-util", "http-body-util", "tower", "tower-http"]
image-pipeline = ["ort", "image", "ndarray", "kamadak-exif"]
mobile = ["uniffi"]
mobile-lite = ["mobile"]  # Text-only PII, no image pipeline
```

### B2. UniFFI Bindings

**FFI tool:** [UniFFI](https://github.com/mozilla/uniffi-rs) (Mozilla) — generates idiomatic Swift AND Kotlin bindings from a single Rust source. Used in production by Firefox mobile.

**New file:** `openobscure-proxy/src/uniffi_bindings.rs`

```rust
#[uniffi::export]
fn create_openobscure(config_json: String, fpe_key_hex: String) -> Result<Arc<OpenObscureMobile>, MobileError> { ... }

#[uniffi::export]
fn sanitize_text(handle: &Arc<OpenObscureMobile>, text: String) -> SanitizeResult { ... }

#[uniffi::export]
fn sanitize_image(handle: &Arc<OpenObscureMobile>, image_bytes: Vec<u8>) -> Result<Vec<u8>, MobileError> { ... }

#[uniffi::export]
fn restore_text(handle: &Arc<OpenObscureMobile>, text: String) -> String { ... }

#[derive(uniffi::Record)]
pub struct SanitizeResult {
    pub sanitized_text: String,
    pub pii_count: u32,
    pub categories: Vec<String>,
}
```

**Cargo.toml:**
```toml
[dependencies]
uniffi = { version = "0.29", features = ["cli"], optional = true }

[build-dependencies]
uniffi = { version = "0.29", features = ["build"], optional = true }
```

### B3. iOS Integration

**Build targets:**
```bash
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
# Device:
cargo build --target aarch64-apple-ios --release --lib --features mobile
# Simulator (Apple Silicon — runs natively, no emulation):
cargo build --target aarch64-apple-ios-sim --release --lib --features mobile
```

**XCFramework packaging (for distribution):**
```bash
# Create universal simulator lib (Intel + ARM)
lipo -create \
    target/aarch64-apple-ios-sim/release/libopenobscure.a \
    -output target/universal-sim/libopenobscure.a

# Package as XCFramework
xcodebuild -create-xcframework \
    -library target/aarch64-apple-ios/release/libopenobscure.a \
    -headers include/ \
    -library target/universal-sim/libopenobscure.a \
    -headers include/ \
    -output OpenObscure.xcframework
```

**Swift integration in OpenClaw iOS app:**
```swift
import OpenObscure

class PrivacyManager {
    private let openobscure: OpenObscureMobile

    init() throws {
        let key = try KeychainHelper.getFpeKey()  // iOS Keychain
        let config = MobileConfig.default().toJSON()
        self.openobscure = try createOpenobscure(configJson: config, fpeKeyHex: key)
    }

    func sanitize(_ text: String) -> SanitizeResult {
        return sanitizeText(handle: openobscure, text: text)
    }
}
```

**Platform adaptations for iOS:**
- **Keyring:** FPE key passed from Swift (iOS Keychain) → Rust via UniFFI parameter. No `keyring` crate needed on iOS.
- **Logging:** `tracing-oslog` already works on iOS (same Unified Logging as macOS). No change.
- **RAM detection:** Not needed. iOS manages memory pressure via `didReceiveMemoryWarning`. Default to CRF scanner on mobile (10MB vs 55MB NER).
- **Paths:** Passed from Swift via `FileManager.default.urls(for: .documentDirectory)` → Rust via UniFFI.
- **ONNX models:** Bundled in app's asset catalog (~30MB for face+OCR+NER). NSFW model optional on mobile.

**Testing on current Mac:**
- `aarch64-apple-ios-sim` target runs **natively** on Apple Silicon (no emulation penalty)
- Xcode iOS Simulator provides full debugging via LLDB
- No Apple Developer account needed for simulator testing
- No physical iPhone needed

### B4. Android Integration

**Build targets:**
```bash
cargo install cargo-ndk
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android

# ARM64 (modern phones, 95%+ market share):
cargo ndk --target aarch64-linux-android --platform 28 build --release --lib --features mobile
# Output: target/aarch64-linux-android/release/libopenobscure.so

# Copy to Android project:
cp target/aarch64-linux-android/release/libopenobscure.so \
   apps/android/app/src/main/jniLibs/arm64-v8a/
```

**Kotlin integration in OpenClaw Android app (UniFFI-generated):**
```kotlin
import ai.openobscure.OpenObscure

class PrivacyManager(context: Context) {
    private val openobscure: OpenObscureMobile

    init {
        val key = EncryptedSharedPreferences.get(context, "fpe_key")
        val config = MobileConfig.default().toJson()
        openobscure = OpenObscure.createOpenobscure(configJson = config, fpeKeyHex = key)
    }

    fun sanitize(text: String): SanitizeResult {
        return OpenObscure.sanitizeText(handle = openobscure, text = text)
    }
}
```

**Alternative: Gobley Gradle plugin** — automates UniFFI + Cargo build inside Gradle:
```kotlin
// build.gradle.kts
plugins {
    id("dev.gobley.cargo")
    id("dev.gobley.uniffi")
}
```

**Platform adaptations for Android:**
- **Keyring:** FPE key passed from Kotlin (`EncryptedSharedPreferences` / Android Keystore) → Rust via UniFFI. No `keyring` crate needed.
- **Logging:** `android_log` crate for logcat output:
  ```toml
  [target.'cfg(target_os = "android")'.dependencies]
  android_log = "0.1"
  ```
- **RAM detection:** `/proc/meminfo` already works (Android is Linux kernel). The existing Linux `#[cfg]` path handles this automatically.
- **Paths:** Passed from Kotlin via `context.filesDir.absolutePath` → Rust via UniFFI.
- **ONNX models:** Bundled in APK `assets/` (~30MB). Extracted to app cache on first use. NNAPI execution provider available for hardware-accelerated inference.

**Testing on current Mac:**
- Android Studio emulator runs **ARM64 natively** on Apple Silicon (Apple Hypervisor framework)
- Deploy `.apk` with embedded `.so` and test directly
- `adb logcat` for real-time log monitoring
- No physical Android device needed

### B5. ONNX Runtime Mobile Considerations

The `ort` crate (v2.0.0-rc.11) supports mobile platforms but requires setup:

**Pre-building ORT for mobile:**
```bash
# iOS
./build.sh --config Release --use_xcode \
    --ios --ios_sysroot iphoneos \
    --osx_arch arm64 --apple_deploy_target 15.0

# Android
./build.sh --config Release --build_shared_lib \
    --android --android_sdk_path $ANDROID_SDK \
    --android_ndk_path $ANDROID_NDK \
    --android_abi arm64-v8a --android_api 28
```

**ORT format models (smaller + faster on mobile):**
```bash
python -m onnxruntime.tools.convert_onnx_models_to_ort models/
# Converts .onnx → .ort (optimized for mobile, ~30% smaller)
```

**Hardware acceleration:**
- iOS: **CoreML** execution provider → Apple Neural Engine (~5x faster than CPU)
- Android: **NNAPI** execution provider → device NPU (~3-5x faster than CPU)

**Feature flag for image pipeline:**
```toml
[features]
default = ["server", "image-pipeline"]
image-pipeline = ["ort", "image", "ndarray", "kamadak-exif"]
mobile-lite = ["mobile"]  # Text-only PII, no image processing
```

Mobile apps can ship with `mobile-lite` (text PII only, ~5MB binary) or full `mobile` (text + image, ~25MB binary).

**Model size budget for mobile:**

| Model | Size | Purpose | Mobile? |
|-------|------|---------|---------|
| TinyBERT NER INT8 | ~15MB | Semantic PII detection | Yes |
| CRF fallback | ~5MB | Low-RAM semantic PII | Yes (default) |
| BlazeFace short | ~408KB | Face detection | Yes |
| PaddleOCR det+rec | ~5.6MB | Text detection | Optional |
| NudeNet 320n | ~12MB | NSFW detection | Optional |
| **Total (full)** | **~38MB** | All models | Fits 70MB budget |
| **Total (lite)** | **~5MB** | CRF only | Minimal footprint |

### B6. L1 Plugin on Mobile

The TypeScript plugin (L1) requires Node.js + `better-sqlite3` — **neither runs on mobile natively**.

**Options:**
1. **Skip L1 on mobile (MVP)** — rely on L0 library for PII protection. Accept reduced coverage (no consent manager, no file access guard, no memory governance). Since the Gateway still has full L1 running server-side, mobile gets L1 protection when data passes through the Gateway.
2. **Rewrite L1 in Rust (Phase 8)** — add consent manager and file access guard to the Rust library using `rusqlite`. Significant effort (~2 weeks). Provides true on-device privacy governance.
3. **Port to Kotlin/Swift (not recommended)** — doubles maintenance burden with two separate implementations.

**Recommendation:** Option 1 for Phase 7B. Option 2 deferred to Phase 8.

### B7. Files Changed (Phase 7B)

| File | Action | Est. Lines |
|------|--------|-----------|
| `openobscure-proxy/Cargo.toml` | Add crate-type, UniFFI dep, feature flags, mobile deps | ~30 |
| `openobscure-proxy/src/lib_mobile.rs` | NEW — mobile API surface | ~200 |
| `openobscure-proxy/src/uniffi_bindings.rs` | NEW — UniFFI interface definitions | ~80 |
| `openobscure-proxy/uniffi.toml` | NEW — UniFFI config | ~15 |
| `openobscure-proxy/build.rs` | NEW — UniFFI scaffolding generation | ~10 |
| `openobscure-proxy/src/lib.rs` | Add `lib_mobile` and `uniffi_bindings` module exports | ~5 |
| `openobscure-proxy/src/main.rs` | Add `#[cfg(target_os = "android")]` logging | ~8 |
| `scripts/build_ios.sh` | NEW — iOS build + XCFramework packaging | ~40 |
| `scripts/build_android.sh` | NEW — Android NDK build script | ~30 |

---

## Feature Parity Matrix

| Feature | macOS | Linux x64 | Linux ARM | Windows | iOS | Android |
|---------|-------|-----------|-----------|---------|-----|---------|
| **Text PII (regex+FPE)** | 7A | 7A | 7A | 7A | 7B | 7B |
| **NER semantic** | 7A | 7A | 7A | 7A | 7B | 7B |
| **CRF fallback** | 7A | 7A | 7A | 7A | 7B (default) | 7B (default) |
| **Keyword dictionary** | 7A | 7A | 7A | 7A | 7B | 7B |
| **Image pipeline** | 7A | 7A | 7A | 7A | 7B (opt) | 7B (opt) |
| **NSFW detection** | 7A | 7A | 7A | 7A | 7B (opt) | 7B (opt) |
| **EXIF stripping** | 7A | 7A | 7A | 7A | 7B | 7B |
| **OS keychain** | 7A | 7A | 7A | 7A | via Swift | via Kotlin |
| **Compliance CLI** | 7A | 7A | 7A | 7A | N/A | N/A |
| **SSE streaming** | 7A | 7A | 7A | 7A | N/A (lib) | N/A (lib) |
| **L1 consent manager** | 7A | 7A | 7A | 7A | Phase 8 | Phase 8 |
| **L1 file access guard** | 7A | 7A | 7A | 7A | Phase 8 | Phase 8 |
| **L1 memory governance** | 7A | 7A | 7A | 7A | Phase 8 | Phase 8 |

---

## Testing Strategy on Current Mac (No Physical Devices Needed)

| Platform | How to Test | Fidelity |
|----------|------------|----------|
| **macOS (current)** | `cargo test` | Full — native |
| **Linux x64** | `cross test --target x86_64-unknown-linux-gnu` (Docker) | Full |
| **Linux ARM64** | `cross test --target aarch64-unknown-linux-gnu` (Docker+QEMU) | Full |
| **Windows** | Build: `cargo xwin build --target x86_64-pc-windows-msvc` | Build only |
| **Windows** | Test: Wine 11 for CLI smoke, UTM VM for full | Smoke / Full |
| **iOS Simulator** | `cargo build --target aarch64-apple-ios-sim` + Xcode test app | Full — native ARM on Apple Silicon |
| **Android Emulator** | `cargo ndk` + Android Studio emulator (ARM64 via Hypervisor) | Full — native ARM on Apple Silicon |

**Required tools:**
- Xcode (for iOS Simulator) — already installed
- Android Studio + NDK (for Android Emulator) — one-time install
- Docker (for Linux cross-testing) — one-time install
- cargo-xwin, cargo-ndk, cargo-zigbuild, cross-rs — `cargo install`

---

## Execution Order

| Step | What | Duration | Dependencies |
|------|------|----------|-------------|
| **7A.1** | Windows: RAM detection + ETW logging + keyring restructure | 2 days | None |
| **7A.2** | Linux ARM64: build + test via Docker/cross-rs | 0.5 days | None |
| **7A.3** | CI/CD: GitHub Actions matrix (macOS/Linux/Windows) | 1 day | 7A.1 + 7A.2 |
| **7B.1** | Library mode: `lib_mobile.rs` + feature flags | 2 days | 7A.1 |
| **7B.2** | UniFFI bindings: Swift + Kotlin generation | 2 days | 7B.1 |
| **7B.3** | iOS: XCFramework + Simulator test app | 2 days | 7B.2 |
| **7B.4** | Android: NDK build + emulator test app | 2 days | 7B.2 |
| **7B.5** | ONNX mobile: ORT format + CoreML/NNAPI EP | 2 days | 7B.3 + 7B.4 |
| **7B.6** | Build scripts + documentation | 1 day | All above |

**Total: ~14 days** (7A: 3.5 days, 7B: 11 days, with overlap)

---

## Verification

```bash
# Phase 7A — Gateway-side (all platforms):
cargo test                                                    # macOS (current)
cross test --target x86_64-unknown-linux-gnu                 # Linux x64
cross test --target aarch64-unknown-linux-gnu                # Linux ARM64
cargo xwin build --target x86_64-pc-windows-msvc --release   # Windows build

# Phase 7B — Mobile library:
cargo build --target aarch64-apple-ios-sim --release --lib --features mobile    # iOS sim
cargo ndk -t aarch64-linux-android -p 28 build --release --lib --features mobile # Android

# Generate UniFFI bindings:
cargo run --features mobile --bin uniffi-bindgen generate \
    src/uniffi_bindings.rs --language swift --out-dir bindings/swift/
cargo run --features mobile --bin uniffi-bindgen generate \
    src/uniffi_bindings.rs --language kotlin --out-dir bindings/kotlin/

# Full suite (existing tests unbroken):
cd openobscure-proxy && cargo test
cd ../openobscure-crypto && cargo test
cd ../openobscure-plugin && npm test
```
