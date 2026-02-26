# OpenObscure NAPI Scanner — Architecture

> Native Node.js addon wrapping the Rust HybridScanner via [napi-rs](https://napi.rs/).

---

## Role in OpenObscure

The NAPI addon is an **optional accelerator** for the L1 TypeScript plugin. When the `@openobscure/scanner-napi` package is installed, `redactPii()` in the L1 plugin automatically upgrades from JS regex to the Rust HybridScanner — the same engine that powers the L0 proxy.

```
L1 Plugin (redactPii)
    │
    ├── @openobscure/scanner-napi installed ──► Rust HybridScanner (14 PII types)
    │                                            ├── Regex + post-validation (CC Luhn, SSN ranges)
    │                                            ├── Keyword dictionary (~700 health/child terms)
    │                                            └── NER TinyBERT INT8 (if model dir provided)
    │
    └── Not installed ─────────────────────────► JS regex fallback (5 PII types)
                                                  └── CC, SSN, phone, email, API key
```

| Without NAPI (JS regex) | With NAPI (Rust HybridScanner) |
|--------------------------|-------------------------------|
| CC, SSN, phone, email, API key (5 types) | + IPv4/6, GPS, MAC, person, location, org, health, child (14 types) |
| ~0ms (pure regex) | <5ms (regex + keywords), <15ms (+ NER) |
| No model files needed | Optional NER model for semantic detection |

## Module Map

```
src/
└── lib.rs    OpenObscureScanner class, ScanMatch/ScanResult types (napi-rs exports)
```

## API Surface

| Class | Method | Returns | Description |
|-------|--------|---------|-------------|
| `OpenObscureScanner` | `constructor(nerModelDir?)` | — | Create scanner; optionally load NER model from directory |
| | `scanText(text)` | `ScanResult` | Scan text, return all PII matches + timing |
| | `hasNer()` | `boolean` | Check if NER model was loaded successfully |

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

NER model auto-discovery: the plugin looks for model files at `../openobscure-proxy/models/ner/` relative to the addon's install path.

## Technology Stack

| Component | Choice | Why |
|-----------|--------|-----|
| Bridge | napi-rs 2.x | Zero-copy Node.js ↔ Rust, auto-generates TS type definitions |
| Scanner | openobscure-proxy (path dependency) | Shares exact same HybridScanner as L0 proxy |
| NER | ort 2.0 (ONNX Runtime) via openobscure-proxy | TinyBERT INT8, loaded on demand |
| Release profile | `opt-level = "s"`, LTO, strip | Minimal binary size |

## Supported Platforms

| Platform | Triple | Status |
|----------|--------|--------|
| macOS ARM64 | `aarch64-apple-darwin` | Tested |
| macOS x64 | `x86_64-apple-darwin` | Configured |
| Linux x64 | `x86_64-unknown-linux-gnu` | CI |
| Linux ARM64 | `aarch64-unknown-linux-gnu` | CI |

## Resource Budget

| Metric | Value |
|--------|-------|
| Binary size (`scanner.node`) | ~17MB (includes ONNX Runtime) |
| RAM (regex + keywords only) | ~30MB |
| RAM (+ NER model loaded) | ~80MB |
| Scan latency (regex only) | <5ms |
| Scan latency (+ NER) | <15ms |
