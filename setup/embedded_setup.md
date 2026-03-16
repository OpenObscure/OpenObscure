# Embedded Setup: OpenObscure as a Native Library

Compile OpenObscure into your iOS, macOS, or Android app — no proxy, no HTTP server. PII is sanitized in-process before it ever leaves the device.

> **Prerequisites:** Complete the [common setup](README.md) first (dev tools, Rust toolchain, clone, `git lfs pull`).

---

## What You're Setting Up

OpenObscure compiles into your app as a **native library** — no sidecar process, no HTTP server. PII detection and encryption run in your app's own process, so sensitive data is sanitized before any network call is made.

```
Your app (Swift / Kotlin)
        │  user message
        ▼
OpenObscure library (in-process)
  ├─ Detects PII — regex + NER ensemble
  ├─ Encrypts with FF1 format-preserving encryption
  └─ Returns sanitized text + mapping
        │  sanitized request (no real PII)
        ▼
  LLM provider (cloud or on-device)
        │  LLM response (may contain FPE tokens)
        ▼
OpenObscure library
  └─ Restores original values using saved mapping
        │  restored response
        ▼
Your app (shows real values to user)
```

| What gets built | Platform | Size |
|-----------------|----------|------|
| `OpenObscure.xcframework` | iOS + simulator | ~300 MB (ORT linked statically) |
| `libopenobscure_core.so` | Android ARM64 | ~5–10 MB |
| UniFFI bindings | Swift / Kotlin | generated, ~50 KB |
| ONNX models | all | ~14 MB download + ~175 MB via Git LFS |

This guide covers two tested integration paths and a generic option:

- **Part 5A — Enchanted** (iOS/macOS Ollama client): apply a diff, copy artifacts, build in Xcode
- **Part 5B — RikkaHub** (Android multi-provider LLM client): apply a diff, copy artifacts, build in Android Studio
- **Part 5C — Generic app**: pointers to the [Integration Guide](../docs/integrate/embedding/embedded_integration.md) for your own app

---

## What You'll Need

In addition to the [common prerequisites](README.md):

**For iOS / macOS (Enchanted or your own app):**
- [ ] **Xcode 15+** with iOS SDK — `xcodebuild -version` to verify
- [ ] **iOS Rust targets:** `rustup target add aarch64-apple-ios aarch64-apple-ios-sim`

**For Android (RikkaHub or your own app):**
- [ ] **Android Studio** Hedgehog (2023.1.1) or later
- [ ] **Android NDK 27+** — installed via `sdkmanager` (see Part 1)
- [ ] **cargo-ndk** — `cargo install cargo-ndk`
- [ ] **Android Rust targets:** `rustup target add aarch64-linux-android x86_64-linux-android`

**All platforms:**
- [ ] **Git LFS** — model files (NER, NSFW, RI, KWS) are stored in LFS (~175 MB); `git lfs pull` in the repo root
- [ ] **~500 MB free disk space** — iOS static libs (~300 MB), Android `.so` (~10 MB), models (~190 MB)

### Set your fork paths

Set these once in your terminal session. All copy commands in Parts 2–5 use them:

```bash
# Path to this OpenObscure repo
export OO_REPO=~/Code/OpenObscure

# Path to your cloned Enchanted fork (iOS / macOS)
export FORK_SWIFT=~/Test/enchanted-openobscure

# Path to your cloned RikkaHub fork (Android)
export FORK_KOTLIN=~/Test/rikkahub-openobscure
```

> Adjust the paths to match where you cloned each repo.

---

## Part 1: Platform Prerequisites

### iOS / macOS

Run from any directory — `rustup target add` is a global Rust toolchain command:

```bash
# Install cross-compilation targets
rustup target add aarch64-apple-ios aarch64-apple-ios-sim

# Verify Xcode is installed (15+ with iOS SDK)
xcodebuild -version
```

### Android

```bash
# Install cross-compilation targets
rustup target add aarch64-linux-android x86_64-linux-android

# Install cargo-ndk (simplifies NDK builds)
cargo install cargo-ndk

# Install Android SDK + NDK (via Homebrew)
brew install --cask android-commandlinetools
sdkmanager "platforms;android-35" "build-tools;35.0.0" "ndk;27.2.12479018"
```

Set up environment variables (add to `~/.zshrc`):

```bash
export ANDROID_HOME="$HOME/Library/Android/sdk"
export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/27.2.12479018"
```

---

## Part 2: Build the Library

> All build commands assume the working directory is the **repo root** (the directory containing `openobscure-core/` and `build/`).

### macOS

```bash
cargo build --manifest-path openobscure-core/Cargo.toml \
  --lib --no-default-features --features mobile --release
```

> `--features mobile` disables the HTTP proxy layer (axum/hyper) and enables UniFFI bindings. Use this flag for all embedded builds.

Expected output:
```
openobscure-core/target/release/libopenobscure_core.dylib   (~5–10 MB)
openobscure-core/target/release/libopenobscure_core.a       (~150 MB, includes ORT static lib)
```

### iOS (device + simulator)

```bash
./build/build_ios.sh --release

# Optional: create XCFramework for Xcode / SPM distribution
./build/build_ios.sh --release --xcframework
```

Expected output:
```
openobscure-core/target/aarch64-apple-ios/release/libopenobscure_core.a      (~150 MB)
openobscure-core/target/aarch64-apple-ios-sim/release/libopenobscure_core.a  (~150 MB)
```

With `--xcframework`:
```
openobscure-core/target/OpenObscure.xcframework/
```

> Each `.a` is large because ORT is linked statically on iOS — third-party dynamic libraries are not permitted.

### Android (ARM64)

```bash
# Requires cargo-ndk + Android NDK (see Part 1)
./build/build_android.sh --release

# Optional: all ABIs
./build/build_android.sh --release --all-abis
```

Expected output:
```
openobscure-core/target/aarch64-linux-android/release/libopenobscure_core.so  (~5–10 MB)
```

With `--all-abis`:
```
openobscure-core/target/aarch64-linux-android/release/libopenobscure_core.so    (arm64-v8a)
openobscure-core/target/armv7-linux-androideabi/release/libopenobscure_core.so  (armeabi-v7a)
openobscure-core/target/x86_64-linux-android/release/libopenobscure_core.so     (x86_64)
```

> **Note:** If you are integrating into RikkaHub or another existing app, you won't have a target project directory yet. Skip this copy step for now and complete it after **Part 5B Step 1** (clone the fork) when the destination exists.

Copy each `.so` from the build output into your Android project (run from the repo root):

```bash
mkdir -p $FORK_KOTLIN/app/src/main/jniLibs/arm64-v8a
mkdir -p $FORK_KOTLIN/app/src/main/jniLibs/armeabi-v7a
mkdir -p $FORK_KOTLIN/app/src/main/jniLibs/x86_64

cp openobscure-core/target/aarch64-linux-android/release/libopenobscure_core.so \
   $FORK_KOTLIN/app/src/main/jniLibs/arm64-v8a/libopenobscure_core.so
cp openobscure-core/target/armv7-linux-androideabi/release/libopenobscure_core.so \
   $FORK_KOTLIN/app/src/main/jniLibs/armeabi-v7a/libopenobscure_core.so
cp openobscure-core/target/x86_64-linux-android/release/libopenobscure_core.so \
   $FORK_KOTLIN/app/src/main/jniLibs/x86_64/libopenobscure_core.so
```

---

## Part 3: Generate UniFFI Bindings

```bash
# Swift (iOS / macOS)
./build/generate_bindings.sh --swift-only
```

Expected output:
```
bindings/swift/openobscure_core.swift
bindings/swift/openobscure_coreFFI.modulemap
```

Copy both files from the OpenObscure repo root into your iOS/macOS app (run from the repo root):

```bash
cp bindings/swift/openobscure_core.swift $FORK_SWIFT/Enchanted/openobscure_core.swift
cp bindings/swift/openobscure_coreFFI.modulemap $FORK_SWIFT/Enchanted/openobscure_coreFFI.modulemap
```

Then add both files to the app target in Xcode (drag from the Finder into the `Enchanted/` group, or use **File → Add Files**). The `.swift` file is the generated API surface; the `.modulemap` exposes the underlying C header to Swift.

> **Note:** If you are integrating into Enchanted, do this after **Part 5A Step 1** (clone the fork) when the destination directory exists.

```bash
# Kotlin (Android)
./build/generate_bindings.sh --kotlin-only
```

Expected output:
```
bindings/kotlin/uniffi/openobscure_core/openobscure_core.kt
```

Copy the file into your Android project's source set (run from the repo root):

```bash
mkdir -p $FORK_KOTLIN/app/src/main/java/uniffi/openobscure_core
cp bindings/kotlin/uniffi/openobscure_core/openobscure_core.kt \
   $FORK_KOTLIN/app/src/main/java/uniffi/openobscure_core/openobscure_core.kt
```

The file must be compiled alongside `libopenobscure_core.so`.

> **Note:** If you are integrating into RikkaHub, do this after **Part 5B Step 1** (clone the fork).

---

## Part 4: Download Models

Models enable NER, image pipeline, voice KWS, and cognitive firewall. Without them, OpenObscure falls back to regex + keyword + gazetteer detection — 15 structured PII types still covered, but names, locations, and orgs are not detected.

Both commands are required — they fetch different model sets:

```bash
# NER, NSFW, response integrity, KWS models (~175 MB)
# Stored in Git LFS — pulled from this repo
git lfs pull

# BlazeFace, SCRFD, PaddleOCR (~14 MB)
# Downloaded directly from upstream sources (license restricts repo distribution)
./build/download_models.sh
```

Expected model directories after both commands:
```
openobscure-core/models/
  blazeface/          — all tiers      (BlazeFace face detection, ~408 KB)
  paddleocr/          — all tiers      (PaddleOCR det + rec + dict, ~10 MB)
  scrfd/              — standard, full (SCRFD-2.5GF, ~3.1 MB)
  ner/                — full tier      (DistilBERT INT8, ~64 MB)
  ner-lite/           — lite, standard (TinyBERT INT8, ~14 MB)
  nsfw_classifier/    — all tiers      (ViT-base INT8, ~83 MB)
  ri/                 — full tier      (R2 response integrity, ~14 MB)
  kws/                — full + voice   (Zipformer KWS, ~5 MB)
```

Bundle the `models/` directory into your app's resources (iOS) or assets (Android). Set `models_base_dir` in your config JSON to point to the directory containing these subdirectories.

> **No models?** The library starts without error. Missing models disable only those features — regex detection still runs. Call `getDebugLog(handle)` after init to confirm which models loaded.

---

## Part 5A: Integrate — Enchanted (iOS/macOS)

Enchanted is an Ollama client for iOS and macOS. The integration adds outbound PII sanitization, inbound response restoration, image sanitization, and voice transcript sanitization.

**Upstream:** [AugustDev/enchanted](https://github.com/AugustDev/enchanted) — base commit `2f82ee2`

### Step 1 — Clone the Enchanted fork

Run from **outside** the OpenObscure repo (e.g. `~/Test/`):

```bash
git clone https://github.com/AugustDev/enchanted.git enchanted-openobscure
cd enchanted-openobscure
git checkout 2f82ee2518c63fa7347c9e8e8e5a131ee0b75cbe
```

> **Reminder:** If you skipped the Swift bindings copy in Part 3, do it now — run the copy commands from the OpenObscure repo root before continuing.

### Step 2 — Copy the OpenObscure artifacts

> **Prerequisite:** Complete the **iOS** build in Part 2 (`./build/build_ios.sh --release`) before running this step. The commands below copy the iOS device library. If you only ran the macOS build, see the commented-out alternative in step 3 below.

Run these commands from the OpenObscure **repo root** (the directory containing `openobscure-core/` and `build/`):

```bash
cd $OO_REPO

# 1. Create the local SPM package that Xcode will import
mkdir -p $FORK_SWIFT/OpenObscureLib/Sources/COpenObscure/include
mkdir -p $FORK_SWIFT/OpenObscureLib/Sources/OpenObscureLib
mkdir -p $FORK_SWIFT/OpenObscureLib/lib

# 2. UniFFI bindings — C header + modulemap go into COpenObscure, Swift file into OpenObscureLib
cp bindings/swift/openobscure_coreFFI.h \
   $FORK_SWIFT/OpenObscureLib/Sources/COpenObscure/include/
# SPM requires the modulemap to be named module.modulemap in publicHeadersPath
cp bindings/swift/openobscure_coreFFI.modulemap \
   $FORK_SWIFT/OpenObscureLib/Sources/COpenObscure/include/module.modulemap
cp bindings/swift/openobscure_core.swift \
   $FORK_SWIFT/OpenObscureLib/Sources/OpenObscureLib/

# 2b. Dummy C file — SPM C target must have at least one source file to produce an .o;
#     without it Xcode fails with "Build input file cannot be found: openobscure_coreFFI.o"
echo "// required: forces SPM to produce an object file for this C module target" \
  > $FORK_SWIFT/OpenObscureLib/Sources/COpenObscure/openobscure_coreFFI.c

# 3. Static library — use the iOS device build (from Part 2 "iOS" section)
cp openobscure-core/target/aarch64-apple-ios/release/libopenobscure_core.a \
   $FORK_SWIFT/OpenObscureLib/lib/

# For macOS testing only (runs on My Mac destination, not on iPhone):
# cp openobscure-core/target/release/libopenobscure_core.a \
#    $FORK_SWIFT/OpenObscureLib/lib/

# 4. Package.swift
cat > $FORK_SWIFT/OpenObscureLib/Package.swift << 'EOF'
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
            name: "openobscure_coreFFI",
            path: "Sources/COpenObscure",
            sources: ["openobscure_coreFFI.c"],
            publicHeadersPath: "include"
        ),
        .target(
            name: "OpenObscureLib",
            dependencies: ["openobscure_coreFFI"],
            path: "Sources/OpenObscureLib",
            linkerSettings: [
                .unsafeFlags(["-L\(Context.packageDirectory)/lib"]),
                .linkedLibrary("openobscure_core"),
                .linkedLibrary("resolv"),
                .linkedFramework("Security"),
                .linkedFramework("SystemConfiguration"),
            ]
        ),
    ]
)
EOF

# 5. OpenObscureManager singleton (handles key storage, mapping accumulation, RI scan)
cp docs/integrate/embedding/templates/OpenObscureManager.swift \
   $FORK_SWIFT/Enchanted/

# 6. Model files — use a distinct folder name to avoid colliding with Enchanted/Models/ (Swift files)
./build/bundle_models.sh $FORK_SWIFT/Enchanted/OpenObscureModels
```

> `bundle_models.sh` selects the right models for the host machine's tier. For a full-tier bundle (all models), run `./build/bundle_models.sh --full /path/to/...`.

### Step 3 — Apply the diff

```bash
cd $FORK_SWIFT
git apply $OO_REPO/docs/integrate/embedding/examples/enchanted-openobscure.diff
```

### Step 4 — Open in Xcode and build

The diff already wires everything into `project.pbxproj`: the `OpenObscureLib` local SPM package reference, `OpenObscureManager.swift` in the Sources build phase, and `OpenObscureModels` as a folder reference in the Resources build phase. No manual Xcode project editing required.

1. Open `Enchanted.xcodeproj` in Xcode 15+.
2. Set the run destination to your connected iPhone (top of the Xcode window, next to the scheme selector). Enchanted is an iOS-first app and the library copied in Step 2 is an iOS device binary.

   > **macOS instead:** If you don't have a device available, re-run Step 2's library copy with the commented-out macOS line (`target/release/libopenobscure_core.a`), then select **My Mac** as the destination.

3. Fix code signing — the original bundle ID `subj.Enchanted` is registered to the author's Apple account and will fail on your machine:
   - Select the **Enchanted** project (blue icon) in the navigator → **Enchanted** target → **Signing & Capabilities** tab
   - Check **Automatically manage signing**
   - Set **Team** to your Apple ID (add via Xcode → Settings → Accounts if needed)
   - Change **Bundle Identifier** to something unique, e.g. `com.local.enchanted-test`
4. **Product → Build** (⌘B).

### Step 5 — Start Ollama and find its address

Enchanted connects to an Ollama server. The iPhone and your Mac must be on the same Wi-Fi network.

**1. Start Ollama bound to all interfaces** (required — the default `127.0.0.1` is not reachable from an iPhone):

```bash
OLLAMA_HOST=0.0.0.0 ollama serve
```

**2. Find your Mac's LAN IP:**

```bash
ipconfig getifaddr en0    # Wi-Fi on most Macs
# or
ipconfig getifaddr en1    # try this if en0 returns nothing
```

The output will be an address like `10.0.0.216` or `192.168.1.x`. That is your Ollama server URL base.

**3. Verify Ollama is reachable** from your Mac before switching to the iPhone:

```bash
curl http://$(ipconfig getifaddr en0):11434
# expected: "Ollama is running"
```

**4. Set the server URL in Enchanted on your iPhone:**

- Open Enchanted → tap the gear icon → **Server URL**
- Enter `http://<your-mac-ip>:11434` (e.g. `http://10.0.0.216:11434`)
- Tap **Save** — Enchanted will test the connection

### Step 6 — Verify PII sanitization

Send a message containing a known PII value (e.g. `My SSN is 123-45-6789`). Check the Xcode console for:

```
[OpenObscure] sanitized 1 PII item(s)
[OpenObscure] RI: severity=Notice cats=[]
```

The LLM response in the chat UI should show the restored original value, not the FPE ciphertext.

---

## Part 5B: Integrate — RikkaHub (Android)

RikkaHub is a multi-provider LLM client for Android. The integration wires an OkHttp interceptor that sanitizes every outbound request body and restores every response.

**Upstream:** [rikkahub/rikkahub](https://github.com/rikkahub/rikkahub) — base commit `7e22476`

### Step 1 — Clone the RikkaHub fork

Run from **outside** the OpenObscure repo (e.g. `~/Test/`):

```bash
git clone https://github.com/rikkahub/rikkahub.git rikkahub-openobscure
cd rikkahub-openobscure
git checkout 7e224767dcac8e76d21a57c74790089214e15d28
```

> **Reminder:** If you skipped the `.so` copy in Part 2, do it now — run the copy commands from the OpenObscure repo root before continuing.

### Step 2 — Copy the OpenObscure artifacts

Run these from the OpenObscure **repo root**:

```bash
# 1. Native library (built in Part 2 for Android)
mkdir -p $FORK_KOTLIN/app/src/main/jniLibs/arm64-v8a
cp openobscure-core/target/aarch64-linux-android/release/libopenobscure_core.so \
   $FORK_KOTLIN/app/src/main/jniLibs/arm64-v8a/

# 2. UniFFI Kotlin bindings (generated in Part 3)
mkdir -p $FORK_KOTLIN/app/src/main/java/uniffi/openobscure_core
cp bindings/kotlin/uniffi/openobscure_core/openobscure_core.kt \
   $FORK_KOTLIN/app/src/main/java/uniffi/openobscure_core/

# 3. Manager + interceptor (handles init, key storage, OkHttp wiring)
mkdir -p $FORK_KOTLIN/app/src/main/java/me/rerere/rikkahub/data/ai
cp docs/integrate/embedding/templates/OpenObscureManager.kt \
   docs/integrate/embedding/templates/OpenObscureInterceptor.kt \
   $FORK_KOTLIN/app/src/main/java/me/rerere/rikkahub/data/ai/

# 4. Model files
mkdir -p $FORK_KOTLIN/app/src/main/assets
./build/bundle_models.sh --android $FORK_KOTLIN/app/src/main/assets/models
```

> For x86_64 (emulator) support also copy: `openobscure-core/target/x86_64-linux-android/release/libopenobscure_core.so` → `jniLibs/x86_64/`.

### Step 3 — Apply the diff

```bash
cd $FORK_KOTLIN
git apply /path/to/openobscure-repo/docs/integrate/embedding/examples/rikkahub-openobscure.diff
```

The diff adds:
- `jna:5.15.0@aar` dependency to `build.gradle.kts`
- ProGuard keep rules for UniFFI + JNA in `proguard-rules.pro`
- `OpenObscureManager.init(this)` call in `RikkaHubApp.onCreate()`
- `OpenObscureInterceptor` wired into the OkHttp client in `DataSourceModule.kt`
- `mavenLocal()` before JitPack in `settings.gradle.kts`

### Step 4 — Open in Android Studio and build

1. Open the `rikkahub-openobscure/` directory in Android Studio Hedgehog (2023.1.1) or later.
2. **File → Sync Project with Gradle Files**. The JNA dependency downloads automatically.
3. Confirm `libopenobscure_core.so` is visible under `app/src/main/jniLibs/arm64-v8a/` in the Project view.
4. **Build → Make Project** (⌃F9). Fix any unresolved reference errors — ensure `uniffi.openobscure_core` package is on the source path.
5. **Run → Run 'app'** on a connected device or an ARM64 emulator (API 27+).

> **x86_64 emulators:** Standard AVD images are x86_64. Either use an ARM64 image (slower) or add the x86_64 `.so` as described in Step 2.

### Step 5 — Verify

Send a chat message containing a test PII value. Open Logcat and filter by tag `OpenObscure`:

```
D OpenObscure: sanitized 1 PII item(s) in 3ms
D OpenObscure: response restored, 1 token(s) decrypted
```

The response text shown in the RikkaHub UI should display the original value, not the FPE ciphertext.

---

## Part 5C: Integrate — Generic iOS/Android app

For any iOS/macOS or Android app not covered above, see the [Integration Guide](../docs/integrate/embedding/embedded_integration.md). It covers:

- Xcode SPM local package setup
- Gradle + JNA + ProGuard configuration
- Swift `OpenObscureManager` and Kotlin `OpenObscureManager` / `OpenObscureInterceptor` templates
- Model bundling options (full, NER-only, cognitive firewall)

---

## Part 6: Troubleshooting

### Build fails with "feature `mobile` not found"

Make sure you are building from the repo root and passing `--manifest-path openobscure-core/Cargo.toml`:

```bash
cargo build --manifest-path openobscure-core/Cargo.toml \
  --lib --no-default-features --features mobile --release
```

### "could not find `cargo-ndk`"

```bash
cargo install cargo-ndk
```

### Models not loading — tier shows "lite" unexpectedly

1. Confirm `git lfs pull` completed without errors — LFS pointer files (not real model data) will cause silent fallback to lite
2. Confirm `models_base_dir` in config JSON points to the directory containing `ner/`, `nsfw_classifier/`, etc. (not to a subdirectory)
3. Run `getDebugLog(handle)` immediately after `createOpenobscure()` to see which paths were tried

### `createOpenobscure()` throws on iOS

The most common causes:
- **Invalid key** — must be exactly 64 hexadecimal characters (`openssl rand -hex 32` produces the correct format)
- **Bad config JSON** — validate the JSON string before passing it
- **Models path wrong** — use `Bundle.main.resourcePath! + "/models"` and verify the models directory is included in the app target's Copy Bundle Resources build phase

---

## Next Steps

1. **[Integration Guide](../docs/integrate/embedding/embedded_integration.md)** — Xcode SPM setup, Gradle + JNA + ProGuard, OkHttp interceptor, and worked diffs from tested apps (Enchanted, RikkaHub)
2. **[API Reference](../docs/reference/api-reference.md)** — full function signatures, type definitions, and error conditions
3. **[Deployment Tiers](../docs/get-started/deployment-tiers.md)** — what each tier enables and how to override
