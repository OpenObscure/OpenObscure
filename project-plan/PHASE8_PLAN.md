# Phase 8: Production Hardening — CI/CD, Mobile Integration, L1 Rust Port

**Status: PLANNED**

## Context

Phase 7 established two deployment models (Gateway + Embedded) and the mobile library API. Phase 8 takes this to production readiness: automated cross-platform CI/CD, mobile test apps, ONNX mobile execution providers, and porting L1 governance features from TypeScript to Rust for the Embedded Model.

### Current State (End of Phase 7)

- **431 tests** passing across all components (319 Rust proxy + 16 crypto + 96 TypeScript)
- **Gateway Model:** Full-featured on macOS, Linux x64. Build support for Windows, Linux ARM64.
- **Embedded Model:** `lib_mobile.rs` API + UniFFI bindings + build scripts. Not yet tested on physical devices or simulators.
- **L1 on mobile:** Not available — rely on Gateway-side L1 when data passes through.

### What Phase 8 Covers

1. **CI/CD cross-platform matrix** — automated build + test on all platforms
2. **Mobile integration testing** — iOS Simulator + Android Emulator test apps
3. **ONNX Runtime mobile** — hardware-accelerated inference on device NPUs
4. **L1 Rust port** — consent manager + file access guard in Rust for Embedded Model
5. **UniFFI binding automation** — Swift/Kotlin binding generation in CI
6. **Production packaging** — XCFramework, AAR, Homebrew formula, Cargo publish

---

## 8A: CI/CD Cross-Platform Build Matrix

**Goal:** Every push to `main` verifies builds across all target platforms.

### GitHub Actions Workflow

```yaml
# .github/workflows/ci.yml
name: CI
on: [push, pull_request]

jobs:
  test:
    strategy:
      matrix:
        include:
          # Gateway Model targets
          - os: macos-latest
            target: aarch64-apple-darwin
            test: true
          - os: ubuntu-latest
            target: x86_64-unknown-linux-gnu
            test: true
          - os: ubuntu-latest
            target: aarch64-unknown-linux-gnu
            cross: true
            test: true
          - os: windows-latest
            target: x86_64-pc-windows-msvc
            test: true

          # Embedded Model targets (build-only)
          - os: macos-latest
            target: aarch64-apple-ios
            test: false
          - os: macos-latest
            target: aarch64-apple-ios-sim
            test: false
          - os: ubuntu-latest
            target: aarch64-linux-android
            ndk: true
            test: false

    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}
      - name: Build
        run: cargo build --target ${{ matrix.target }} --release --lib
      - name: Test
        if: matrix.test
        run: cargo test --target ${{ matrix.target }}

  plugin:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with: { node-version: '22' }
      - run: cd openobscure-plugin && npm ci && npm test

  crypto:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cd openobscure-crypto && cargo test
```

### Cross-Compilation Tools

| Target | Tool | Notes |
|--------|------|-------|
| Linux ARM64 | `cross-rs` | Docker + QEMU user emulation |
| Windows | native runner | `windows-latest` GitHub runner |
| iOS | Xcode toolchain | `macos-latest` runner has Xcode pre-installed |
| Android | `cargo-ndk` | NDK installed via `setup-android` action |

### Files Changed

| File | Action |
|------|--------|
| `.github/workflows/ci.yml` | NEW — CI matrix |
| `.github/workflows/release.yml` | NEW — binary release builds |

---

## 8B: Mobile Integration Testing

**Goal:** Verify the Embedded Model works end-to-end on iOS Simulator and Android Emulator.

### 8B.1: iOS Test App

A minimal Swift app that links `OpenObscure.xcframework` and exercises the API:

```
test-apps/ios/
├── OpenObscureTest/
│   ├── OpenObscureTestApp.swift    # App entry
│   ├── ContentView.swift           # UI: text input → sanitize → show result
│   └── PrivacyManager.swift        # OpenObscure wrapper
├── OpenObscureTest.xcodeproj
└── OpenObscure.xcframework/        # Built by scripts/build_ios.sh --xcframework
```

**What it tests:**
- Library initialization with FPE key from iOS Keychain
- Text sanitization (credit card, SSN, email, phone)
- Restore/decrypt round-trip
- Image sanitization (camera photo → face blur → display)
- Memory footprint verification (Instruments)

**Testing on current Mac:**
- `aarch64-apple-ios-sim` runs natively on Apple Silicon (no emulation)
- Xcode Simulator provides full debugging via LLDB
- No Apple Developer account needed for simulator testing

### 8B.2: Android Test App

A minimal Kotlin app that loads `libopenobscure_proxy.so` via UniFFI:

```
test-apps/android/
├── app/
│   ├── src/main/
│   │   ├── java/ai/openobscure/test/
│   │   │   ├── MainActivity.kt        # UI: text input → sanitize → show result
│   │   │   └── PrivacyManager.kt       # OpenObscure wrapper
│   │   └── jniLibs/
│   │       └── arm64-v8a/
│   │           └── libopenobscure_proxy.so
│   └── build.gradle.kts
└── settings.gradle.kts
```

**What it tests:**
- Library loading via `System.loadLibrary`
- UniFFI-generated Kotlin bindings
- Text sanitization + restore round-trip
- FPE key from `EncryptedSharedPreferences`
- Memory footprint verification (Android Profiler)

**Testing on current Mac:**
- Android Studio emulator runs ARM64 natively on Apple Silicon (Hypervisor.framework)
- `adb logcat` for real-time log monitoring
- No physical device needed

### Files Changed

| File | Action |
|------|--------|
| `test-apps/ios/` | NEW — iOS test app |
| `test-apps/android/` | NEW — Android test app |
| `scripts/build_ios.sh` | Update to auto-copy XCFramework into test app |
| `scripts/build_android.sh` | Update to auto-copy .so into test app jniLibs |

---

## 8C: ONNX Runtime Mobile Execution Providers

**Goal:** Enable hardware-accelerated inference on mobile devices for face detection, OCR, and NER models.

### 8C.1: iOS — CoreML Execution Provider

Apple's Neural Engine runs ONNX models via CoreML EP, typically 3-5x faster than CPU:

```rust
use ort::{Session, ExecutionProviderDispatch};

let session = Session::builder()?
    .with_execution_providers([
        ExecutionProviderDispatch::CoreML(Default::default()),
        ExecutionProviderDispatch::CPU(Default::default()),  // fallback
    ])?
    .commit_from_file("model.onnx")?;
```

**Pre-built ORT for iOS:**
```bash
# Build ORT from source for iOS
./build.sh --config Release --use_xcode \
    --ios --ios_sysroot iphoneos \
    --osx_arch arm64 --apple_deploy_target 15.0 \
    --use_coreml
```

### 8C.2: Android — NNAPI Execution Provider

Android's Neural Networks API delegates to device NPU/DSP, typically 2-4x faster than CPU:

```rust
let session = Session::builder()?
    .with_execution_providers([
        ExecutionProviderDispatch::Nnapi(Default::default()),
        ExecutionProviderDispatch::CPU(Default::default()),  // fallback
    ])?
    .commit_from_file("model.onnx")?;
```

### 8C.3: ORT Format Models

Convert ONNX models to `.ort` format for mobile (smaller, faster loading):

```bash
python -m onnxruntime.tools.convert_onnx_models_to_ort models/ --optimization_style Fixed
# Output: .ort files (~30% smaller than .onnx)
```

### Model Size Budget (Mobile)

| Model | ONNX | ORT (est.) | Purpose | Priority |
|-------|------|-----------|---------|----------|
| CRF features | ~5MB | ~3.5MB | Semantic PII (default on mobile) | Required |
| BlazeFace short | ~408KB | ~300KB | Face detection | Optional |
| PaddleOCR det | ~2.4MB | ~1.7MB | Text region detection | Optional |
| PaddleOCR rec | ~7.8MB | ~5.5MB | Character recognition (Tier 2) | Optional |
| TinyBERT NER INT8 | ~15MB | ~10.5MB | Semantic PII (16GB+ devices) | Optional |
| NudeNet 320n | ~12MB | ~8.4MB | NSFW detection | Optional |
| **Total (full)** | **~43MB** | **~30MB** | All models | Fits 70MB budget |
| **Total (lite)** | **~5MB** | **~3.5MB** | CRF only | Minimal footprint |

### Files Changed

| File | Action |
|------|--------|
| `openobscure-proxy/Cargo.toml` | Add `ort` feature flags for CoreML/NNAPI EPs |
| `openobscure-proxy/src/lib_mobile.rs` | Add EP selection based on platform |
| `scripts/build_ort_mobile.sh` | NEW — build ORT from source for mobile |
| `scripts/convert_models_ort.sh` | NEW — convert .onnx → .ort format |

---

## 8D: L1 Rust Port (Governance on Mobile)

**Goal:** Port consent manager, file access guard, and memory governance from TypeScript to Rust so the Embedded Model gets full privacy governance on mobile.

### Why

The L1 Gateway Plugin (TypeScript) requires Node.js + `better-sqlite3` — neither runs on mobile natively. Without L1 on mobile, the Embedded Model has no:
- Consent management (GDPR Art. 13/14)
- File access guard (deny patterns for .env, SSH keys, etc.)
- Memory governance (retention tiers, auto-expiry)
- Privacy commands (/privacy status, /privacy consent, /privacy retention)

When both models run together (mobile + Gateway), the Gateway-side L1 provides these features for data that passes through it. But data that stays on-device (local conversations, cached tool results) has no governance.

### Architecture

New Rust module: `openobscure-proxy/src/governance.rs`

```rust
pub struct GovernanceEngine {
    consent_db: rusqlite::Connection,  // SQLite consent store
    file_guard: FileGuard,             // Deny pattern matcher
    retention: RetentionManager,       // Tier lifecycle (hot/warm/cold/expired)
}

impl GovernanceEngine {
    /// Check if processing is consented for this data category.
    pub fn check_consent(&self, category: &str) -> ConsentStatus { ... }

    /// Record user consent decision.
    pub fn set_consent(&self, category: &str, granted: bool) -> Result<()> { ... }

    /// Check if a file path is safe to read.
    pub fn check_file_access(&self, path: &str) -> FileAccessResult { ... }

    /// Enforce retention policy (expire old data).
    pub fn enforce_retention(&self) -> RetentionReport { ... }

    /// Get current retention tier for a data item.
    pub fn retention_tier(&self, item_id: &str) -> RetentionTier { ... }
}
```

### Mobile API Addition

```rust
impl OpenObscureMobile {
    /// Create with governance enabled (SQLite at provided path).
    pub fn new_with_governance(
        config: MobileConfig,
        fpe_key: [u8; 32],
        db_path: &str,
    ) -> Result<Self, MobileError> { ... }

    /// Check/set GDPR consent.
    pub fn check_consent(&self, category: &str) -> ConsentStatus { ... }
    pub fn set_consent(&self, category: &str, granted: bool) -> Result<(), MobileError> { ... }

    /// Check file access safety.
    pub fn check_file_access(&self, path: &str) -> FileAccessResult { ... }
}
```

### Dependencies

```toml
[dependencies]
rusqlite = { version = "0.32", features = ["bundled"], optional = true }

[features]
governance = ["rusqlite"]
mobile-full = ["mobile", "governance"]
```

### What Gets Ported

| L1 Feature | TypeScript Source | Rust Target | Effort |
|------------|-----------------|-------------|--------|
| PII Redactor | `redactor.ts` | Already in L0 (`scanner.rs`) | Done |
| File Access Guard | `file-guard.ts` | `governance.rs` → `FileGuard` | 1 day |
| Consent Manager | `consent-manager.ts` | `governance.rs` → `ConsentStore` | 2 days |
| Memory Governance | `memory-governance.ts` | `governance.rs` → `RetentionManager` | 2 days |
| Privacy Commands | `privacy-commands.ts` | CLI/API in `lib_mobile.rs` | 1 day |
| Heartbeat Monitor | `heartbeat.ts` | N/A (Embedded Model has no L0 to ping) | Skip |
| OO Log | `oo-log.ts` | Already in L0 (`oo_log.rs`) | Done |

### Files Changed

| File | Action | Est. Lines |
|------|--------|-----------|
| `openobscure-proxy/Cargo.toml` | Add `rusqlite` optional dep, `governance` feature | ~5 |
| `openobscure-proxy/src/governance.rs` | NEW — consent + file guard + retention in Rust | ~500 |
| `openobscure-proxy/src/lib_mobile.rs` | Add governance methods | ~80 |
| `openobscure-proxy/src/uniffi_bindings.rs` | Add governance FFI exports | ~40 |
| `openobscure-proxy/src/lib.rs` | Wire `governance` module | ~2 |

---

## 8E: UniFFI Binding Automation

**Goal:** Automatically generate Swift and Kotlin bindings in CI and include them in releases.

### Binding Generation

```bash
# Generate Swift bindings
cargo run --features mobile --bin uniffi-bindgen generate \
    src/uniffi_bindings.rs --language swift --out-dir bindings/swift/

# Generate Kotlin bindings
cargo run --features mobile --bin uniffi-bindgen generate \
    src/uniffi_bindings.rs --language kotlin --out-dir bindings/kotlin/
```

### CI Integration

Add binding generation step to the release workflow:

```yaml
generate-bindings:
  runs-on: macos-latest
  steps:
    - uses: actions/checkout@v4
    - uses: dtolnay/rust-toolchain@stable
    - run: cargo build --features mobile --release
    - run: |
        cargo run --features mobile --bin uniffi-bindgen generate \
          src/uniffi_bindings.rs --language swift --out-dir bindings/swift/
        cargo run --features mobile --bin uniffi-bindgen generate \
          src/uniffi_bindings.rs --language kotlin --out-dir bindings/kotlin/
    - uses: actions/upload-artifact@v4
      with:
        name: uniffi-bindings
        path: bindings/
```

### Distribution Formats

| Platform | Package | Contents |
|----------|---------|----------|
| iOS | `OpenObscure.xcframework` | Universal static lib + Swift bindings + headers |
| Android | `openobscure-release.aar` | ARM64 + ARMv7 .so + Kotlin bindings |
| macOS/Linux | `openobscure-proxy` binary | Standalone binary (Homebrew, .deb, .rpm) |
| Rust | `openobscure-proxy` crate | `cargo install` or use as library dependency |

### Files Changed

| File | Action |
|------|--------|
| `.github/workflows/release.yml` | NEW — release builds + binding generation |
| `scripts/package_ios.sh` | NEW — XCFramework + Swift bindings packaging |
| `scripts/package_android.sh` | NEW — AAR packaging |
| `bindings/swift/` | Generated — Swift source files |
| `bindings/kotlin/` | Generated — Kotlin source files |

---

## 8F: Production Packaging

**Goal:** Distribute OpenObscure in standard package formats.

### Desktop

| Format | Target | How |
|--------|--------|-----|
| Homebrew | macOS/Linux | `brew install openobscure/tap/openobscure` |
| Cargo | All | `cargo install openobscure-proxy` |
| .deb | Debian/Ubuntu | `dpkg -i openobscure_0.1.0_arm64.deb` |
| .rpm | Fedora/RHEL | `rpm -i openobscure-0.1.0.aarch64.rpm` |
| .msi | Windows | Standard Windows installer |
| Docker | All | `ghcr.io/openobscure/openobscure-proxy:latest` |

### Mobile

| Format | Target | How |
|--------|--------|-----|
| XCFramework | iOS | `OpenObscure.xcframework` in Xcode project |
| Swift Package | iOS | `https://github.com/OpenObscure/openobscure-swift` SPM repo |
| AAR | Android | Maven Central: `ai.openobscure:openobscure:0.1.0` |
| Gradle plugin | Android | `id("dev.gobley.cargo")` + `id("dev.gobley.uniffi")` |

---

## Feature Parity Matrix (After Phase 8)

| Feature | Gateway | Embedded | Notes |
|---------|---------|----------|-------|
| Text PII (regex + FPE) | Yes | Yes | Full parity |
| NER semantic | Yes | Optional | High RAM, use CRF on mobile |
| CRF fallback | Yes | Yes (default) | Low RAM, good accuracy |
| Keyword dictionary | Yes | Yes | Full parity |
| Image pipeline | Yes | Optional | +20MB binary, feature-gated |
| NSFW detection | Yes | Optional | +12MB model |
| EXIF stripping | Yes | Yes | Full parity |
| OS keychain | Yes | Via host app | iOS Keychain / Android Keystore |
| Compliance CLI | Yes | N/A | Desktop/server only |
| SSE streaming | Yes | N/A | HTTP proxy only |
| Consent manager | Yes (L1) | **Yes (Rust)** | Phase 8D |
| File access guard | Yes (L1) | **Yes (Rust)** | Phase 8D |
| Memory governance | Yes (L1) | **Yes (Rust)** | Phase 8D |
| Hardware accel | CPU only | CoreML/NNAPI | Phase 8C |
| CI/CD | Manual | **Automated** | Phase 8A |

---

## Execution Order

| Step | What | Est. Duration | Dependencies |
|------|------|--------------|-------------|
| **8A** | CI/CD cross-platform matrix | 2 days | None |
| **8B.1** | iOS test app + Simulator testing | 2 days | 8A |
| **8B.2** | Android test app + Emulator testing | 2 days | 8A |
| **8C** | ONNX mobile EPs + ORT format models | 3 days | 8B |
| **8D** | L1 Rust port (governance) | 6 days | 8B |
| **8E** | UniFFI binding automation in CI | 1 day | 8A + 8B |
| **8F** | Production packaging (Homebrew, AAR, SPM) | 3 days | All above |

**Total: ~19 days** (can overlap: 8A→8B parallel with 8D research)

---

## Verification

```bash
# Phase 8A — CI/CD:
# Verify GitHub Actions passes on all matrix targets

# Phase 8B — Mobile test apps:
xcodebuild test -project test-apps/ios/OpenObscureTest.xcodeproj \
    -scheme OpenObscureTest -destination 'platform=iOS Simulator,name=iPhone 16'
cd test-apps/android && ./gradlew connectedAndroidTest

# Phase 8C — ONNX mobile:
cargo build --target aarch64-apple-ios-sim --features "mobile,image-pipeline" --release
cargo ndk -t aarch64-linux-android -p 28 build --features "mobile,image-pipeline" --release

# Phase 8D — L1 Rust port:
cargo test governance
cargo test lib_mobile::tests::test_mobile_consent
cargo test lib_mobile::tests::test_mobile_file_guard
cargo test lib_mobile::tests::test_mobile_retention

# Phase 8E — UniFFI bindings:
cargo run --features mobile --bin uniffi-bindgen generate \
    src/uniffi_bindings.rs --language swift --out-dir bindings/swift/
cargo run --features mobile --bin uniffi-bindgen generate \
    src/uniffi_bindings.rs --language kotlin --out-dir bindings/kotlin/

# Full suite (existing tests unbroken):
cd openobscure-proxy && cargo test
cd ../openobscure-crypto && cargo test
cd ../openobscure-plugin && npm test
```

---

## Deferred Beyond Phase 8

These items are not scheduled and may never be implemented:

| Item | Reason for Deferral |
|------|-------------------|
| **GLiNER NER** | Requires 200-250MB RAM. Only viable on 16GB+ devices. Revisit if TinyBERT accuracy proves insufficient. |
| **FastText document classifier** | +15MB RAM. Useful for collection-based topic classification but not critical. |
| **SCRFD face detection** | Multi-scale detector for mixed-size faces in screenshots. BlazeFace sufficient for primary use case. |
| **Voice anonymization** | ~200MB model. Only viable on high-resource Tier 4 devices. |
| **Real-time breach monitoring** | Rolling window anomaly detection in proxy. Batch CLI (`breach-check`) sufficient for v1. |
| **Multilingual PII** | FastText language detection + multilingual regex patterns. Current focus is English-language PII. |
| **tracing-etw (Windows)** | ETW logging backend. Generic return type incompatible with current pattern. File+stderr sufficient. |
