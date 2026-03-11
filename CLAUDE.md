<!-- INTERNAL: AI coding assistant instructions. Not user docs. -->

# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Test Commands

```bash
# L0 Proxy (Rust) — the core component
cd openobscure-proxy
cargo build --release                        # Build release binary
cargo test --lib --all-features              # Unit + integration tests (lib target)
cargo test --bin openobscure-proxy           # Binary tests (server-dependent)
cargo test --lib --all-features -- test_name # Run a single test
cargo fmt --check                            # Format check
cargo clippy --all-features -- -D warnings   # Lint

# L1 Plugin (TypeScript)
cd openobscure-plugin
npm run build                                # Compile TS → dist/
npm test                                     # Run all tests
node --import tsx --test src/foo.test.ts     # Run a single test file

# L2 Crypto (Rust, enterprise)
cd openobscure-crypto && cargo test

# NAPI addon (native bridge for L1)
cd openobscure-napi && npm run build

# Run the proxy
./openobscure-proxy/target/release/openobscure-proxy serve
```

## Architecture

On-device privacy firewall for AI agents. Three layers, two deployment models:

- **L0 — `openobscure-proxy/`** (Rust): HTTP reverse proxy that intercepts LLM traffic. Detects PII with 4-engine ensemble (regex → keywords → NER/CRF → gazetteer), encrypts with FF1 FPE, processes images (NSFW/face/OCR), scans responses for manipulation. This is the bulk of the codebase.
- **L1 — `openobscure-plugin/`** (TypeScript): In-process plugin for the host agent. Catches PII in tool results (web scrapes, file reads) that bypass the proxy. Optional NAPI addon (`openobscure-napi/`) upgrades JS regex (5 types) to Rust HybridScanner (15 types).
- **L2 — `enterprise/openobscure-crypto/`** (Rust): AES-256-GCM encrypted storage with Argon2id KDF. Enterprise-only.

**Gateway model:** L0 runs as a sidecar HTTP proxy. Agent points `base_url` at `127.0.0.1:18790`.
**Embedded model:** L0 compiles as a static/shared library. Swift/Kotlin call it via UniFFI bindings. No HTTP server.

### Key L0 Modules

The proxy has dual entry points — `main.rs` (binary, owns all modules) and `lib.rs` (library, exports public modules for tests/benchmarks/mobile). They share the same source files.

- `hybrid_scanner.rs` — orchestrates all detection engines, merges results with confidence voting
- `scanner.rs` — regex patterns for 10 structured PII types with post-validation (Luhn, SSN ranges, etc.)
- `fpe_engine.rs` / `pii_types.rs` — FF1 encryption with per-type radix/alphabet; `PiiType` enum defines all 15 types
- `body.rs` — request/response body processing: image-first pass, then text PII pass
- `device_profile.rs` — hardware detection, tier classification, `FeatureBudget` struct
- `config.rs` — all TOML config structs with defaults; mirrors `config/openobscure.toml`
- `image_pipeline.rs` — lazy-load model manager (NSFW → face → OCR → EXIF strip)
- `response_integrity.rs` — R1 dictionary + R2 TinyBERT cognitive firewall cascade
- `lib_mobile.rs` / `uniffi_bindings.rs` — mobile API and FFI type wrappers

### Feature Flags (Cargo)

- `server` (default) — axum/hyper HTTP stack. Disabled for mobile builds.
- `voice` (default) — sherpa-onnx KWS keyword spotting + symphonia audio decoding.
- `mobile` — UniFFI bindings for Swift/Kotlin. Enables `uniffi_bindings.rs`.
- `bindgen` — UniFFI CLI tool for generating bindings.

## Guardrails

- **Feature gating is mandatory** — every feature must be tier-gated via `FeatureBudget`. Follow the 6-step checklist in `docs/contribute/feature-gating-protocol.md`. `FeatureBudget` has no `Default` impl — missing fields cause compile errors.
- **Never modify code in the test repo** (`/Users/admin/Test/OpenObscure`) — commit, push, pull there
- **Never commit `project-plan/`** to git — it's in `.gitignore`
- **Never copy code or binaries from dev to test env** — commit → push, pull in test env, build there
- **Enterprise-only features** (compliance CLI, breach detection, encrypted storage) must NOT appear in public-facing docs (README, docs/, setup/)
- **FF1 only** — FF3 is NIST-withdrawn. The `fpe` crate 0.6 uses `FF1::<Aes256>::new(key, radix)`.
- **Fail-open default** — FPE errors skip the match and forward original text, never block the AI agent.

## Documentation Structure

```
docs/
  get-started/     Quick starts, deployment models, tiers
  configure/       FPE, detection engines, config reference (every TOML key)
  integrate/       LLM providers, third-party app embedding
  architecture/    System overview + 6 component deep-dives
  reference/       API types (FFI), PII coverage, config keys
  contribute/      Dev workflow, feature gating protocol
```

Architecture entry point: `docs/architecture/system-overview.md`. Config reference: `docs/configure/config-reference.md`.

## Session Notes

- Create at every `/compact` point and end of session
- Format: `session-notes/ses_YY-MM-DD-HH-MM.md`
- Phase plans: `project-plan/PHASE<N>_PLAN.md` (gitignored)
