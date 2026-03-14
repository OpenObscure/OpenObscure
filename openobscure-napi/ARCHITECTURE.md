# OpenObscure NAPI Scanner — Architecture

> Native Node.js addon wrapping the Rust HybridScanner via [napi-rs](https://napi.rs/). For system context, see [System Overview](../docs/architecture/system-overview.md).

---

## Role in OpenObscure

The NAPI addon is an **optional accelerator** for the L1 TypeScript plugin. When the `@openobscure/scanner-napi` package is installed, `redactPii()` in the L1 plugin automatically upgrades from JS regex to the Rust HybridScanner — the same engine that powers the L0 Core proxy.

```
L1 Plugin (redactPii)
    │
    ├── @openobscure/scanner-napi installed ──► Rust HybridScanner (15 PII types)
    │                                            ├── Regex + post-validation (CC Luhn, SSN ranges)
    │                                            ├── Keyword dictionary (~700 health/child terms)
    │                                            └── NER TinyBERT INT8 (if model dir provided)
    │
    └── Not installed ─────────────────────────► JS regex fallback (5 PII types)
                                                  └── CC, SSN, phone, email, API key
```

| Without NAPI (JS regex) | With NAPI (Rust HybridScanner) |
|--------------------------|-------------------------------|
| CC, SSN, phone, email, API key (5 types) | + IPv4/6, GPS, MAC, IBAN, person, location, org, health, child (15 types) |
| ~0ms (pure regex) | <5ms (regex + keywords), <15ms (+ NER) |
| No model files needed | Optional NER model for semantic detection |

## Module Map

```
src/
└── lib.rs    OpenObscureScanner class, scan_persuasion() function, types (napi-rs exports), 6 unit tests
```

## API Surface

| Class/Function | Method | Returns | Description |
|----------------|--------|---------|-------------|
| `OpenObscureScanner` | `constructor(nerModelDir?)` | — | Create scanner; optionally load NER model from directory |
| | `scanText(text)` | `ScanResult` | Scan text, return all PII matches + timing |
| | `hasNer()` | `boolean` | Check if NER model was loaded successfully |
| `scanPersuasion(text)` | — | `PersuasionScanResult` | Scan text for persuasion phrases (R1 dictionary, 7 categories) |

### Types

```typescript
interface ScanMatch {
  start: number;       // Byte offset start
  end: number;         // Byte offset end
  piiType: string;     // "ssn", "email", "person", "location", etc.
  confidence: number;  // 0.0–1.0 (regex/keyword = 1.0, NER = model score)
  rawValue: string;    // The matched text
}

interface ScanResult {
  matches: ScanMatch[];
  timingUs: number;    // Total scan time in microseconds
}

interface PersuasionMatch {
  category: string;    // "Urgency", "Fear", "Commercial", etc.
  start: number;       // Byte offset start
  end: number;         // Byte offset end
  phrase: string;      // Matched phrase text
}

interface PersuasionScanResult {
  matches: PersuasionMatch[];
  timingUs: number;    // Scan time in microseconds
}
```

## Build

```bash
./build/build_napi.sh          # release build
./build/build_napi.sh --debug  # debug build
```

Output: `openobscure-napi/scanner.node` (~17MB, includes ONNX Runtime + HybridScanner)

## How L1 Auto-Detection Works

At module load, `redactor.ts` attempts:

```typescript
const mod = require("@openobscure/scanner-napi");
NativeScanner = mod.OpenObscureScanner;
```

If the require succeeds, all `redactPii()` calls use the native scanner. If it fails (addon not installed), falls back silently to JS regex. No configuration needed.

**NER model auto-discovery:** The plugin searches two candidate paths in order:
1. `<addonDir>/models/ner/` — bundled model inside the platform npm package (installed from npm)
2. `<addonDir>/../openobscure-core/models/ner/` — dev monorepo layout (local build)

**Engine observability:** `activeEngine()` is exported from both `openobscure-plugin` and `openobscure-plugin/core`. It returns `"napi"` when the addon is loaded, `"js"` otherwise. The active engine is also logged at startup via `ooInfo`.

## Technology Stack

| Component | Choice | Why |
|-----------|--------|-----|
| Bridge | napi-rs 2.x | Zero-copy Node.js ↔ Rust, auto-generates TS type definitions |
| Scanner | openobscure-core (path dependency) | Shares exact same HybridScanner as L0 Core proxy |
| NER | ort 2.0 (ONNX Runtime) via openobscure-core | TinyBERT INT8, loaded on demand |
| Release profile | `opt-level = "s"`, LTO, strip | Minimal binary size |

## Distribution

Platform binaries are published as separate npm packages and installed as `optionalDependencies`. The umbrella package (`@openobscure/scanner-napi`) resolves the right one at install time.

| npm package | Platform | Build method | Status |
|-------------|----------|--------------|--------|
| `@openobscure/scanner-napi-darwin-arm64` | macOS Apple Silicon | macos-14 native | **Live on npm** |
| `@openobscure/scanner-napi-linux-x64-gnu` | Linux x64 glibc | ubuntu native | **Live on npm** |
| `@openobscure/scanner-napi-linux-arm64-gnu` | Linux ARM64 glibc | ubuntu + cargo-zigbuild | **Live on npm** |
| `@openobscure/scanner-napi-darwin-x64` | macOS Intel | macos-14 cross-compile | Deferred |
| `@openobscure/scanner-napi-linux-x64-musl` | Linux x64 Alpine | Alpine container | Deferred — ort no prebuilts |
| `@openobscure/scanner-napi-linux-arm64-musl` | Linux ARM64 Alpine | ubuntu + cargo-zigbuild | Deferred — ort no prebuilts |

Each live platform package includes the compiled `.node` binary **and** the bundled TinyBERT INT8 NER model (`models/ner/`), so NER works immediately after install without any extra model download step.

Publishing is automated via `.github/workflows/napi-publish.yml`, triggered by a `napi-v*` tag. The current workflow builds the 3 live platforms; musl and darwin-x64 are omitted until `ort` 2.0 provides prebuilt binaries for those targets.

> **Phase 4 (complete):** `@openobscure/scanner-napi` v0.1.1 is published on npm and wired as `optionalDependencies` in `openobscure-plugin/package.json`. On supported platforms the 15-type Rust scanner auto-loads; unsupported platforms fall back to JS regex until their packages are published.

## Supported Platforms

| Platform | Triple | Status |
|----------|--------|--------|
| macOS ARM64 | `aarch64-apple-darwin` | **Published (npm v0.1.1)** |
| Linux x64 glibc | `x86_64-unknown-linux-gnu` | **Published (npm v0.1.1)** |
| Linux ARM64 glibc | `aarch64-unknown-linux-gnu` | **Published (npm v0.1.1)** |
| macOS x64 | `x86_64-apple-darwin` | Deferred |
| Linux x64 musl (Alpine) | `x86_64-unknown-linux-musl` | Deferred — ort no prebuilts |
| Linux ARM64 musl (Alpine) | `aarch64-unknown-linux-musl` | Deferred — ort no prebuilts |

## Resource Budget

| Metric | Value |
|--------|-------|
| Binary size (`scanner.node`) | ~17MB (includes ONNX Runtime) |
| RAM (regex + keywords only) | ~30MB |
| RAM (+ NER model loaded) | ~80MB |
| Scan latency (regex only) | <5ms |
| Scan latency (+ NER) | <15ms |
