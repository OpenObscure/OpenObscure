# OpenObscure — System Architecture

> Privacy firewall for AI agents. Primary integration: [OpenClaw](https://github.com/openclaw/openclaw), the open-source AI assistant.

---

## What OpenObscure Does

Every message, tool result, and file a user shares with an AI agent gets sent to third-party LLM APIs in plaintext — credit cards, health discussions, API keys, children's information, photos. OpenObscure prevents this by intercepting data at multiple layers, encrypting or redacting PII before it leaves the device.

## Deployment Models

OpenObscure runs **entirely on the user's device** — no remote servers, no cloud components, no separate infrastructure. It supports two deployment models depending on where the AI agent runs.

### Gateway Model (Desktop / Server)

The full-featured deployment. OpenObscure runs as a **sidecar HTTP proxy** on the same host as the AI agent's Gateway. Both layers are active.

```mermaid
flowchart TB
    subgraph device ["User's Device"]
        subgraph gateway ["AI Agent Gateway (e.g. OpenClaw)"]
            L1["L1 Plugin<br>(in-process, no separate PID)"]
        end
        subgraph l0proc ["L0 Proxy (standalone Rust process)"]
        end
        gateway -- "HTTP (localhost)" --> l0proc
    end
    l0proc -- "HTTPS" --> llm(["LLM Providers (external)"])

    style device fill:#2f2f45,stroke:#4a4a6e,color:#d0d0e0
    style gateway fill:#4a7fb5,stroke:#6a9fd5,color:#f0f0f0
    style l0proc fill:#7c6cbf,stroke:#9c8cdf,color:#f0f0f0
    style llm fill:#d9556a,stroke:#e97585,color:#f0f0f0
```

| Component | Process | How it runs |
|-----------|---------|-------------|
| **L0** (Rust proxy) | Standalone binary | Separate process, started as sidecar alongside the host agent. |
| **L1** (TS plugin) | In-process | Loaded into the host agent's runtime (e.g., OpenClaw's Node.js via plugin SDK) or used as a library. |

**Supported platforms:** macOS (Apple Silicon), Linux (x64 + ARM64), Windows (x64).

**Activation:**
1. **At install time** — The host agent's bundler includes OpenObscure and activates it during setup (if user opts in). OpenClaw supports this via its plugin SDK.
2. **Post-install** — User enables OpenObscure by configuring the host agent to route API traffic through `127.0.0.1:18790` instead of directly to LLM providers, and installs the L1 plugin into the agent's extensions directory (e.g., OpenClaw's `extensions/`)

When disabled, the host agent operates normally with direct LLM connections — OpenObscure adds zero overhead when not active.

### Embedded Model (Mobile / Library)

For mobile apps and custom integrations, OpenObscure compiles as a **native library** (`.a` for iOS, `.so` for Android) linked directly into the host application. No HTTP server, no sockets — just function calls via UniFFI-generated Swift/Kotlin bindings.

```mermaid
flowchart TB
    subgraph mobile ["Mobile Device (iOS / Android)"]
        subgraph app ["Host App (e.g. OpenClaw iOS/Android)"]
            ui["UI Layer (Swift / Kotlin)"]
            lib["OpenObscure lib<br>(linked Rust library)"]
            ui -- "sanitize_text()" --> lib
            lib -- "SanitizeResult + mapping" --> ui
            ui -. "restore_text()" .-> lib
        end
    end

    app -- "WebSocket (PII encrypted)" --> gw
    gw -- "response" --> app

    subgraph desktop ["External Computer (macOS / Linux / Windows)"]
        gw["Gateway (Node.js 22+)"]
        gw -- "HTTP" --> llm(["LLM Provider"])
    end

    style mobile fill:#2f2f45,stroke:#4a4a6e,color:#d0d0e0
    style app fill:#4a7fb5,stroke:#6a9fd5,color:#f0f0f0
    style lib fill:#7c6cbf,stroke:#9c8cdf,color:#f0f0f0
    style desktop fill:#2f2f45,stroke:#4a4a6e,color:#d0d0e0
    style gw fill:#4a6a8a,stroke:#6a8aaa,color:#f0f0f0
    style llm fill:#d9556a,stroke:#e97585,color:#f0f0f0
```

| Component | What | How it runs |
|-----------|------|-------------|
| **L0** (Rust library) | `OpenObscureMobile` API | Linked into host app binary. PII scan + FPE encrypt/decrypt + image pipeline. FPE key provided by host app's native secure storage (iOS Keychain / Android Keystore). |

**Supported platforms:** iOS (aarch64 device + simulator), Android (arm64-v8a, armeabi-v7a, x86_64, x86).

**API surface:**

| Function | What it does |
|----------|-------------|
| `OpenObscureMobile::new(config, fpe_key)` | Initialize scanner + FPE engine with host-provided key |
| `sanitize_text(text)` | Scan for PII, encrypt with FPE, return sanitized text + mapping |
| `restore_text(text, mapping)` | Decrypt FPE values in response text using saved mapping |
| `sanitize_image(bytes)` | Face blur + OCR text blur + EXIF strip (optional, adds ~20MB) |
| `stats()` | PII counts, scanner mode, image pipeline status, device tier |

**Key differences from Gateway Model:**
- No HTTP server (axum/tokio not compiled in)
- FPE key passed from host app (no OS keychain access on mobile)
- Hardware auto-detection (`auto_detect: true` default) profiles device RAM and selects features automatically — phones with 8GB+ RAM get full NER + ensemble + image pipeline, matching gateway efficacy
- Image pipeline enabled automatically on capable devices (4GB+)

### Defense in Depth: Both Models Together

In the OpenClaw architecture, **both models can run simultaneously**. The mobile app sanitizes PII before it reaches the Gateway (Embedded Model), and the Gateway sanitizes again before forwarding to LLM providers (Gateway Model). Double protection for mobile-originated data:

```mermaid
flowchart LR
    phone["Mobile App<br>+ OpenObscure lib<br>(FPE encrypt on-device)"] -- "WS (PII encrypted)" --> gw["Gateway + L1 Plugin<br>(redacts tool results)"]
    gw -- "HTTP" --> proxy["OpenObscure Proxy<br>(Gateway host)<br>(FPE encrypt again)"]
    proxy -- "HTTPS (encrypted twice)" --> llm["LLM Provider"]

    style phone fill:#4a7fb5,stroke:#6a9fd5,color:#f0f0f0
    style gw fill:#4a6a8a,stroke:#6a8aaa,color:#f0f0f0
    style proxy fill:#7c6cbf,stroke:#9c8cdf,color:#f0f0f0
    style llm fill:#d9556a,stroke:#e97585,color:#f0f0f0
```

### API Keys & External Connections

OpenObscure does **not** have its own LLM credentials and does **not** initiate its own API calls.

- **Gateway Model:** Passthrough-first — forwards the host agent's API keys unchanged.
- **Embedded Model:** No API calls at all — the library sanitizes text/images and returns results. The host app handles all networking.

The only network activity OpenObscure produces (Gateway Model only) is forwarding the host agent's existing LLM requests through the local proxy. No telemetry, no phone-home, no external dependencies at runtime.

## Two-Layer Defense-in-Depth

```mermaid
flowchart TB
    tools["Agent Tools<br>web · file · API · bash"]

    subgraph gateway ["AI Agent Gateway (e.g. OpenClaw)"]
        subgraph l1box ["L1 — Gateway Plugin (TypeScript)"]
            redact["PII Redactor<br>(regex → redaction)"]
            heartbeat["Heartbeat Monitor<br>(pings L0 every 30s)"]
        end
    end

    subgraph l0box ["L0 — Rust PII Proxy · 127.0.0.1:18790"]
        subgraph reqpath ["Request Path"]
            nested["Parse nested JSON<br>+ mask code fences"] --> hybrid["Hybrid Scanner<br>regex + NER/CRF + keywords"]
            hybrid --> imgpipe["Image Pipeline<br>NSFW · face · OCR · EXIF"]
            imgpipe --> ff1["FF1 FPE encrypt"]
        end
        subgraph respath ["Response Path"]
            decrypt["FPE decrypt<br>ciphertexts → plaintext"]
        end
    end

    llm(["LLM Providers<br>Anthropic · OpenAI · Ollama"])

    tools -- "tool results" --> gateway
    gateway -- "HTTP API calls" --> reqpath
    ff1 -- "sanitized HTTPS" --> llm
    llm -- "response" --> decrypt
    decrypt -- "decrypted response" --> gateway

    style gateway fill:#4a7fb5,stroke:#6a9fd5,color:#f0f0f0
    style l1box fill:#3a5580,stroke:#5a75a0,color:#f0f0f0
    style l0box fill:#7c6cbf,stroke:#9c8cdf,color:#f0f0f0
    style reqpath fill:#6a5aaa,stroke:#8a7aca,color:#f0f0f0
    style respath fill:#6a5aaa,stroke:#8a7aca,color:#f0f0f0
    style llm fill:#d9556a,stroke:#e97585,color:#f0f0f0
    style tools fill:#6a6a85,stroke:#8a8aa5,color:#f0f0f0
```

## Layer Details

### L0 — Rust PII Proxy (`openobscure-proxy/`)

The **hard enforcement** layer. Sits between the host agent and LLM providers as an HTTP reverse proxy. Every API request passes through it — there is no bypass path.

| Aspect | Detail |
|--------|--------|
| **What it does** | Scans JSON request bodies for PII via hybrid scanner (regex → keywords → NER/CRF) with ensemble confidence voting, encrypts matches with FF1 FPE, decrypts ciphertexts in responses (SSE streaming supported). Processes base64-encoded images (face blur, OCR text blur, EXIF strip). Handles nested/escaped JSON strings and respects markdown code fences. |
| **What it catches** | Structured: credit cards (Luhn), SSNs (range-validated), phones, emails, API keys. Network/device: IPv4 (rejects loopback/broadcast), IPv6 (full + compressed), GPS coordinates (4+ decimal precision), MAC addresses (colon/dash/dot). Semantic: person names, addresses, orgs (NER/CRF). Health/child keyword dictionary (~700 terms). Visual: nudity (NudeNet ONNX), faces in photos (BlazeFace ONNX), text in screenshots/images (PaddleOCR ONNX). |
| **Auth model** | Passthrough-first — forwards the host agent's API keys unchanged |
| **Key management** | FPE master key: `OPENOBSCURE_MASTER_KEY` env var (64 hex chars) or OS keychain via `keyring`. Env var takes priority (headless/Docker/CI). |
| **Content-Type** | Only scans JSON bodies. Binary, text, multipart pass through unchanged |
| **Fail mode** | Configurable fail-open (default) or fail-closed. Vault unavailable always blocks (503) |
| **Logging** | Unified `oo_*!()` macro API, PII scrub layer, mmap crash buffer, file rotation, platform logging (OSLog/journald) |
| **Stack** | Rust, axum 0.8, hyper 1, tokio, fpe 0.6 (FF1), ort (ONNX Runtime), image 0.25, keyring 3, clap 4 (CLI) |
| **Resource** | Tier-dependent: ~12MB (Lite/regex-only), ~67MB (Standard/NER), ~224MB peak (Full/image processing); 2.7MB binary |
| **Tests** | 650+ |
| **Deployment** | Gateway Model: standalone binary. Embedded Model: static/shared library with UniFFI bindings (Swift/Kotlin). |
| **Docs** | [openobscure-proxy/ARCHITECTURE.md](openobscure-proxy/ARCHITECTURE.md) |

### L1 — Gateway Plugin (`openobscure-plugin/`)

The **second line of defense**. Runs in-process with the host agent. Catches PII that enters through tool results (web scraping, file reads, API responses) — data that never passes through the HTTP proxy.

| Aspect | Detail |
|--------|--------|
| **What it does** | Hooks the host agent's tool result persistence (e.g., OpenClaw's `tool_result_persist`) to scan and redact PII in tool outputs. Provides L0 heartbeat monitor with auth token validation and unified logging API (`ooInfo`/`ooWarn`/`ooAudit`). |
| **PII handling** | Redaction (`[REDACTED]`), not FPE — tool results are internal, don't need format preservation |
| **Heartbeat** | Pings L0 `/_openobscure/health` every 30s with `X-OpenObscure-Token` auth header. Warns user when L0 is down, logs recovery. |
| **Hook model** | Synchronous — must not return a Promise. OpenClaw-specific: OpenClaw silently skips async hooks. |
| **Logging** | Unified `ooInfo/ooWarn/ooError/ooDebug/ooAudit` API with PII scrubbing, JSON output |
| **Stack** | TypeScript 5.4, CommonJS |
| **Resource** | ~25MB RAM (within the host agent's process), ~3MB storage |
| **Tests** | 96 (9 redactor + 12 heartbeat + 2 state-messages + 17 oo-log) |
| **Docs** | [openobscure-plugin/ARCHITECTURE.md](openobscure-plugin/ARCHITECTURE.md) |

**Process watchdog** (install templates):
- macOS: launchd plist with `KeepAlive` + `ThrottleInterval`
- Linux: systemd unit with `Restart=on-failure` + `MemoryMax=275M`

## How FPE Works

Format-Preserving Encryption transforms plaintext into ciphertext of **identical format**. The LLM sees plausible-looking data instead of `[REDACTED]`, preserving conversational context.

```mermaid
sequenceDiagram
    participant U as User / Agent
    participant P as L0 Proxy (FF1)
    participant L as LLM Provider

    U->>P: "My card is 4111-1111-1111-1111<br/>and SSN 123-45-6789"

    Note over P: FF1 encrypt each match<br/>CC → 8714-3927-6051-2483<br/>SSN → 847-29-3651

    P->>L: "My card is 8714-3927-6051-2483<br/>and SSN 847-29-3651"

    Note over L: LLM processes plausible<br/>data — never sees real PII

    L->>P: "The card ending in 2483...<br/>SSN 847-29-3651 is..."

    Note over P: FF1 decrypt each match<br/>2483 → 1111<br/>847-29-3651 → 123-45-6789

    P->>U: "The card ending in 1111...<br/>SSN 123-45-6789 is..."
```

| PII Type | Radix | Encrypted Part | Preserved |
|----------|-------|----------------|-----------|
| Credit Card | 10 | 15-16 digits | Dash positions |
| SSN | 10 | 9 digits | Dash positions |
| Phone | 10 | 10+ digits | `+`, parens, spaces, dashes |
| Email | 36 | Local part | `@` + domain |
| API Key | 62 | Post-prefix body | Known prefix (`sk-`, `AKIA`...) |
| IPv4 Address | — | Redacted to `[IPv4]` | N/A (not FPE) |
| IPv6 Address | — | Redacted to `[IPv6]` | N/A (not FPE) |
| GPS Coordinate | — | Redacted to `[GPS]` | N/A (not FPE) |
| MAC Address | — | Redacted to `[MAC]` | N/A (not FPE) |

**Algorithm:** FF1 per NIST SP 800-38G. FF3 is **WITHDRAWN** (SP 800-38G Rev 2, Feb 2025) — never used.

**Tweak strategy:** Per-record `request_uuid (16B) || SHA-256(json_path)[0..16]` — same PII value in different requests produces different ciphertexts, preventing frequency analysis.

## L0 vs L1 — Why Both?

| | L0 (Proxy) | L1 (Plugin) |
|---|------------|-------------|
| **Intercept point** | HTTP requests/responses to LLMs | Tool results within the host agent |
| **PII handling** | FPE encryption (reversible) | Redaction (destructive) |
| **Catches** | All LLM API traffic | Web scrapes, file reads, API outputs |
| **Bypass possible?** | No — all traffic must route through proxy | Only if the host agent skips the hook |
| **Runs in** | Standalone Rust binary | Host agent process (e.g., OpenClaw Node.js) |

Neither layer alone is sufficient:
- L0 can't see tool results (they're generated inside the host agent, never pass through HTTP)
- L1 can't intercept before LLM sees data (in OpenClaw, only `tool_result_persist` is wired, not `before_tool_call`)

## Data Flow

### Outbound (user → LLM)

```mermaid
flowchart LR
    user["User<br>(WhatsApp, CLI, web)"] --> agent["Agent<br>receives message"]
    agent --> l0["L0 Proxy<br>scan JSON → FPE encrypt<br>(per-record tweaks)"]
    l0 --> llm["LLM Provider<br>(never sees real PII)"]

    style user fill:#4a6a8a,stroke:#6a8aaa,color:#f0f0f0
    style agent fill:#4a7fb5,stroke:#6a9fd5,color:#f0f0f0
    style l0 fill:#7c6cbf,stroke:#9c8cdf,color:#f0f0f0
    style llm fill:#d9556a,stroke:#e97585,color:#f0f0f0
```

### Inbound (LLM → user)

```mermaid
flowchart RL
    llm["LLM Provider"] --> proxy["L0 Proxy<br>FPE decrypt<br>ciphertexts → plaintext"]
    proxy --> agent["Host Agent"]
    agent --> user["User<br>(sees original values)"]

    style llm fill:#d9556a,stroke:#e97585,color:#f0f0f0
    style proxy fill:#7c6cbf,stroke:#9c8cdf,color:#f0f0f0
    style agent fill:#4a7fb5,stroke:#6a9fd5,color:#f0f0f0
    style user fill:#4a6a8a,stroke:#6a8aaa,color:#f0f0f0
```

### Tool Results (agent tools → persistence)

```mermaid
flowchart LR
    tool["Agent Tool<br>file_read · web_fetch<br>bash · API"]
    tool --> result["Tool result text"]
    result --> hook["L1 hook<br>tool_result_persist<br>(synchronous)"]
    hook --> redact["PII Redactor<br>matches → REDACTED"]
    redact --> persist[("Transcript<br>(redacted)")]

    style tool fill:#4a6a8a,stroke:#6a8aaa,color:#f0f0f0
    style result fill:#4a7fb5,stroke:#6a9fd5,color:#f0f0f0
    style hook fill:#4a7fb5,stroke:#6a9fd5,color:#f0f0f0
    style redact fill:#7c6cbf,stroke:#9c8cdf,color:#f0f0f0
    style persist fill:#4a6a8a,stroke:#6a8aaa,color:#f0f0f0
```

**Important:** OpenObscure never reads local files itself. The agent's tools perform all file I/O and produce text results. OpenObscure only sees the resulting text *after* the agent has already read and extracted it. L1 operates on text strings from tool outputs, not on files directly.

## Authentication Model

**Passthrough-first** — OpenObscure is transparent to API authentication:

```mermaid
sequenceDiagram
    participant A as Host Agent (has API keys)
    participant P as OpenObscure Proxy
    participant L as LLM Provider

    A->>P: Authorization: sk-... (+ all headers)
    Note over P: Headers pass through unmodified
    P->>L: Authorization: sk-... (identical)
    Note over L: Transparent proxy —<br/>provider sees original keys
```

- All original request headers forwarded (except hop-by-hop per RFC 7230)
- FPE master key is separate — 32-byte AES-256 via `OPENOBSCURE_MASTER_KEY` env var (headless) or OS keychain (desktop), generated with `--init-key`

## Resource Budget

OpenObscure uses a **hardware capability detection system** to select features at startup. The `device_profile` module detects total RAM, available RAM, and CPU cores, classifies the device into a capability tier, and derives a feature budget.

### Capability Tiers

| Device RAM | Tier | Scanners | Image Pipeline | Model Idle Timeout |
|------------|------|----------|----------------|--------------------|
| 8GB+ | **Full** | NER + CRF + ensemble voting | Yes | 300s |
| 4–8GB | **Standard** | NER + CRF (no ensemble) | Yes | 120s |
| <4GB | **Lite** | CRF + regex only | Yes (shorter timeout) | 60s |

### Gateway Budgets (fixed per tier)

| Tier | Max RAM | NER | CRF | Ensemble | Image |
|------|---------|-----|-----|----------|-------|
| Full | 275MB | Yes | Yes | Yes | Yes |
| Standard | 200MB | Yes | Yes | No | Yes |
| Lite | 80MB | No | Yes | No | Yes |

### Embedded Budgets (proportional to device RAM)

Budget = 20% of total RAM, clamped to [12MB, 275MB]. Features enabled based on available budget within the tier:

| Device | Total RAM | Budget | Tier | NER | CRF | Ensemble | Image |
|--------|-----------|--------|------|-----|-----|----------|-------|
| iPhone 16 Pro | 12GB | 275MB (capped) | Full | Yes | Yes | Yes | Yes |
| iPhone 15 | 6GB | 275MB (capped) | Standard | Yes | Yes | No | Yes |
| Budget Android | 3GB | 614MB | Lite | No | Yes | No | Yes |
| Embedded IoT | 512MB | 102MB | Lite | No | Yes | No | Yes |

### Full Stack Component Breakdown

| Component | RAM | Resident? |
|-----------|-----|-----------|
| L0 + L1 + runtime | 115MB | Always |
| TinyBERT INT8 NER | 55MB | Always (when tier enables NER) |
| Health/child keyword dict | 2MB | Always |
| BlazeFace (face detection) | 8MB | On-demand |
| PaddleOCR-Lite (OCR) | 35MB | On-demand |
| Image buffer | 48MB | On-demand |
| **Peak (Full tier)** | **224MB** | — |
| **Hard ceiling** | **275MB** | — |

Storage ceiling: **62MB** (including all models, ONNX Runtime, config).

Explicit `scanner_mode` config ("ner", "crf", "regex") overrides auto-detection.

## PII Coverage Roadmap

| Phase | Coverage | What's Added |
|-------|----------|--------------|
| **Phase 1** (complete) | **78%** | Regex + FPE for structured PII (CC, SSN, phone, email, API keys) |
| **Phase 2** (complete) | **91%** | Hybrid scanner (NER/CRF + keywords), health monitoring, nested JSON, code fences |
| **Phase 2.5** (complete) | **91%** | Unified logging, PII scrub layer, mmap crash buffer, file rotation |
| **Phase 3** (complete) | **95%** | Visual PII (face blur, OCR text extraction, EXIF strip, screenshot detection, platform logging) |
| **Phase 5** (complete) | **97%** | SSE streaming, PII benchmark corpus (~400 samples, 100% recall), production benchmarks (criterion) |
| **Phase 6** (complete) | **97%** | Ensemble confidence voting (cluster-based overlap resolution + agreement bonus) |
| **Phase 7** (complete) | **97%** | Cross-platform support (Windows, Linux ARM64), mobile library API (iOS + Android via UniFFI), Embedded deployment model |
| **Post-Phase 7** (complete) | **98%** | Network/device identifier detection (IPv4, IPv6, GPS coordinates, MAC addresses) — closes PII-06 + PII-12 |
| **Phase 9** (complete) | **98%** | Runtime hardware capability detection — device profiler auto-selects features based on RAM; mobile devices with 8GB+ get full NER + ensemble parity with gateway |

## Project Layout

```
OpenObscure/
├── ARCHITECTURE.md              ← this file (system-level architecture)
├── session-notes/               Per-session implementation logs
├── .github/workflows/
│   ├── ci.yml                   CI: proxy-test matrix, cross-arm64, mobile-build, plugin, lint
│   └── release.yml              Release: binary matrix + iOS XCFramework + UniFFI bindings
├── scripts/
│   ├── download_models.sh       Download ONNX models for image pipeline
│   ├── generate_screenshot.py   Generate synthetic PII screenshot for demos
│   ├── build_ios.sh             Build iOS static library + XCFramework
│   ├── build_android.sh         Build Android shared library via cargo-ndk
│   └── generate_bindings.sh     Generate UniFFI Swift/Kotlin bindings
├── docs/examples/images/        Before/after visual PII examples
├── openobscure-proxy/             L0: Rust PII proxy (+ embedded mobile library)
│   ├── ARCHITECTURE.md          L0 architecture details
│   ├── LICENSE_AUDIT.md         Dependency license audit
│   ├── src/                     Rust source
│   ├── examples/                Demo binaries (demo_image_pipeline)
│   ├── models/                  ONNX models (git-ignored, download via script)
│   ├── config/openobscure.toml    Default configuration
│   └── install/                 Process watchdog templates (launchd, systemd)
├── review-notes/                Architecture review analysis & responses
├── openobscure-plugin/            L1: Gateway plugin
│   ├── ARCHITECTURE.md          L1 architecture details
│   ├── LICENSE_AUDIT.md         Dependency license audit
│   └── src/                     TypeScript source (redactor, heartbeat, oo-log)
└── project-plan/
    ├── MASTER_PLAN.md           Full design reference (single source of truth)
    ├── PHASE1_PLAN.md           Phase 1 plan (COMPLETE — 75 tests)
    ├── PHASE2_PLAN.md           Phase 2 plan (COMPLETE — 193 tests)
    ├── PHASE3_PLAN.md           Phase 3 plan (COMPLETE — 319 tests)
    ├── PHASE4_PLAN.md           Phase 4 plan (COMPLETE — 376 tests)
    ├── PHASE5_PLAN.md           Phase 5 plan (COMPLETE — 399 tests)
    ├── PHASE6_PLAN.md           Phase 6 plan (COMPLETE — 418 tests)
    ├── PHASE7_PLAN.md           Phase 7 plan (COMPLETE — 431 tests)
    ├── PHASE8_PLAN.md           Phase 8 plan (future work)
    └── LOGGING_STRATEGY.md      Platform-specific logging strategy
```

## Key Design Decisions

| Decision | Rationale |
|----------|-----------|
| FF1 only, never FF3 | FF3 withdrawn by NIST (SP 800-38G Rev 2, Feb 2025) |
| Fail-open default | Proxy must never block AI functionality due to FPE edge cases |
| Vault unavailable → 503 | No privacy guarantees without FPE key — blocking is correct |
| Passthrough-first auth | No duplicate key management; OpenObscure is transparent to the host agent |
| Per-record FPE tweaks | Prevents frequency analysis across requests |
| L1 redacts, not encrypts | Tool results are internal — redaction is simpler and guarantees removal |
| Synchronous hooks only | OpenClaw-specific: OpenClaw silently skips async hook returns |
| INT8 quantization mandatory | FP32 TinyBERT = ~200MB; INT8 = ~50MB — difference between fitting and OOM |
| On-demand model loading | Face + OCR models load/evict per image, saving ~43MB between images |
| Sequential model loading | Face model loaded/used/dropped before OCR model loaded — never both in RAM |
| Two-pass body processing | Images processed first (replaces base64 strings), then text PII (replaces substrings by offset) |
| EXIF strip via decode/encode | `image` crate loads pixels only, discarding all EXIF metadata — no explicit strip step |
| 960px image cap | A 12MP ARGB bitmap = 48MB; resizing before load prevents OOM |

## Host Agent Constraints (OpenClaw Reference)

Three critical OpenClaw-specific constraints that shaped OpenObscure's architecture. Other host agents may have different constraints:

1. **Only `tool_result_persist` is wired** — of OpenClaw's 14 defined hooks, only 3 have invocation sites. `before_tool_call`, `message_sending`, etc. are defined in TypeScript types but never called. This is why L0 (HTTP proxy) exists — it's the only way to intercept data *before* the LLM sees it.

2. **`tool_result_persist` is synchronous** — returning a Promise causes OpenClaw to silently skip the hook. All L1 processing must be synchronous.

3. **OpenClaw updates constantly** — 40+ security patches per release. OpenObscure modules touching internal APIs may break. Pin to known-good OpenClaw versions.

## Running

```bash
# Generate FPE key (first time only)
cd openobscure-proxy && cargo run -- --init-key

# Start proxy
cargo run -- -c config/openobscure.toml

# Run all tests
cd openobscure-proxy && cargo test
cd openobscure-plugin && npm test
```

## Health Monitoring & User Experience

OpenObscure must be **invisible when working, clear when not**. Users should never wonder whether their PII is protected.

### OpenObscure States (from the user's perspective)

| State | What the user sees | What happens |
|-------|-------------------|--------------|
| **Active** | Nothing — AI works normally | L0 encrypts PII, L1 redacts tool results. Silent protection. |
| **Degraded** | Warning: "OpenObscure proxy is not responding — PII protection is disabled" | L1 detects L0 is down. Agent requests fail (no bypass). User is informed. |
| **Disabled** | Startup message: "OpenObscure is not enabled. PII will be sent in plaintext." | Host agent configured for direct LLM connections. No protection. |
| **Crashed** | Same as Degraded — L1 warns, requests fail | L0 process died. Crash marker written for diagnostics. |
| **OOM** | Warning: "OpenObscure ran out of memory and stopped" + crash marker | L0 killed by OS. L1 detects, warns. Crash marker includes memory stats. |
| **Recovering** | "OpenObscure proxy recovered from a previous crash" | L0 restarts, finds crash marker, logs recovery, resumes. |

### Design Principle

**Warn, don't block.** When L0 is down, L1 should warn the user clearly — but not prevent the host agent from functioning. The user decides whether to continue without protection. L0 being down already blocks LLM requests (traffic is routed through the proxy), so L1's role is **explanation**, not enforcement.

### Health Monitoring Architecture

```mermaid
flowchart LR
    subgraph l1side ["L1 Plugin"]
        hb["Heartbeat Monitor<br>(every 30s)"]
        hb --> down["Down? → warn user"]
        hb --> back["Recovered? → log"]
        hb --> auth_fail["401? → degraded"]
    end

    subgraph l0side ["L0 Proxy"]
        endpoint["GET /_openobscure/health"]
        auth_check["Validate<br>X-OpenObscure-Token"]
        response["Status JSON<br>(stats + version + uptime)"]
        endpoint --> auth_check --> response
    end

    token[("~/.openobscure/.auth-token<br>(0600)")]

    hb -- "HTTP + auth token" --> endpoint
    token -. "written by L0 at startup" .-> l0side
    token -. "read by L1 for auth" .-> l1side

    style l0side fill:#7c6cbf,stroke:#9c8cdf,color:#f0f0f0
    style l1side fill:#4a7fb5,stroke:#6a9fd5,color:#f0f0f0
    style token fill:#4a6a8a,stroke:#6a8aaa,color:#f0f0f0
```

**Crash path:**

```mermaid
flowchart LR
    subgraph crash ["Crash (immediate)"]
        panic["panic hook"] --> write["Write<br>~/.openobscure/.crashed"] --> abort["abort"]
    end
    subgraph recovery ["Recovery (next startup)"]
        restart["Startup"] --> detect["Detect .crashed"] --> log["Log: recovered<br>from crash"] --> delete["Delete marker"]
    end

    style crash fill:#3a2a2a,stroke:#d9556a,color:#f0f0f0
    style recovery fill:#2a3a2a,stroke:#3d8a55,color:#f0f0f0
    style panic fill:#d9556a,stroke:#e97585,color:#f0f0f0
    style abort fill:#d9556a,stroke:#e97585,color:#f0f0f0
    style restart fill:#4a7fb5,stroke:#6a9fd5,color:#f0f0f0
    style log fill:#7c6cbf,stroke:#9c8cdf,color:#f0f0f0
```

**Auth token handshake:** L0 generates a random 32-byte hex token on first startup, writes to `~/.openobscure/.auth-token` (file permissions 0600 on Unix). L1 reads this file and sends it as the `X-OpenObscure-Token` header with every health check. If the token is missing or wrong, L0 returns 401 Unauthorized. This prevents other localhost processes from querying or impersonating the health endpoint.

Token resolution (L0 startup): `OPENOBSCURE_AUTH_TOKEN` env var → `~/.openobscure/.auth-token` file → auto-generate and write.

| Component | What | Status |
|-----------|------|--------|
| `GET /_openobscure/health` endpoint | Returns status, version, uptime, PII stats, device tier, feature budget. Auth-gated via `X-OpenObscure-Token`. | Complete |
| L1 heartbeat monitor | Pings health endpoint every 30s with auth token, warns user on failure | Complete |
| L0/L1 auth token | Shared via file (`~/.openobscure/.auth-token`) or env var. Auto-generated on first run. | Complete |
| Panic hook + crash marker | Writes `~/.openobscure/.crashed` before abort | Complete |
| Graceful shutdown logging | "OpenObscure proxy shutting down" on SIGTERM/SIGINT | Complete |
| Process watchdog (launchd/systemd) | Auto-restart L0 on crash via `install/launchd/` and `install/systemd/` templates | Complete |

## Logging Architecture (Phase 2.5)

All logging across both L0 (Rust) and L1 (TypeScript) uses a **unified facade API** — no direct `tracing::*!()` or `console.*` calls outside the logging module. This guarantees every log line passes through PII scrubbing and audit routing.

### L0 Logging Stack

```mermaid
flowchart TB
    macros["oo_info! · oo_warn! · oo_error! · oo_debug! · oo_audit!"]
    subscriber["tracing subscriber (layered)"]
    macros --> subscriber

    stderr["Stderr<br>JSON or plain"]
    filelog["File<br>daily rotation, PII scrub"]
    audit["Audit Log<br>oo_audit events → JSONL"]
    crash["Crash Buffer<br>mmap ring<br>survives SIGKILL/OOM"]

    subscriber --> stderr
    subscriber --> filelog
    subscriber --> audit
    subscriber --> crash

    style macros fill:#7c6cbf,stroke:#9c8cdf,color:#f0f0f0
    style subscriber fill:#4a7fb5,stroke:#6a9fd5,color:#f0f0f0
    style stderr fill:#4a6a8a,stroke:#6a8aaa,color:#f0f0f0
    style filelog fill:#4a6a8a,stroke:#6a8aaa,color:#f0f0f0
    style audit fill:#4a6a8a,stroke:#6a8aaa,color:#f0f0f0
    style crash fill:#4a6a8a,stroke:#d9556a,color:#f0f0f0
```

| Layer | Purpose | Config |
|-------|---------|--------|
| **Stderr** | Primary output, JSON or human-readable | `logging.json_output` |
| **PII scrub** | Regex-based scrub of SSN, CC, email, phone, API keys in log text | `logging.pii_scrub` (default: true) |
| **File rotation** | Daily rolling log files | `logging.file_path`, `max_file_size`, `max_files` |
| **Audit log** | Audit trail — only `oo_audit!` events routed to separate JSONL | `logging.audit_log_path` |
| **Crash buffer** | mmap ring buffer (default 2MB) — kernel flushes pages even on hard crash | `logging.crash_buffer`, `crash_buffer_size` |

**Module tagging:** Every log line includes a `module` field (PROXY, SCANNER, HYBRID, FPE, VAULT, HEALTH, CONFIG, NER, CRF, BODY, SERVER, MAPPING, DEVICE) for structured filtering.

### L1 Logging Stack

```mermaid
flowchart TB
    funcs["ooInfo · ooWarn · ooError · ooDebug · ooAudit"]
    facade["ooLog() facade"]
    funcs --> facade

    console["console.*<br>JSON or plain, PII-scrubbed"]
    auditlog["Audit Log<br>append-only JSONL file"]

    facade --> console
    facade --> auditlog

    style funcs fill:#7c6cbf,stroke:#9c8cdf,color:#f0f0f0
    style facade fill:#4a7fb5,stroke:#6a9fd5,color:#f0f0f0
    style console fill:#4a6a8a,stroke:#6a8aaa,color:#f0f0f0
    style auditlog fill:#4a6a8a,stroke:#6a8aaa,color:#f0f0f0
```

Module constants: REDACTOR, HEARTBEAT, PLUGIN.

All string fields are run through `redactPii()` before output — defense-in-depth ensures no PII leaks through log messages even if developers forget to sanitize.

---

## Image Pipeline (Phase 3)

L0 detects base64-encoded images in JSON request bodies (both Anthropic and OpenAI formats) and processes them before text PII scanning. For before/after visual examples of the pipeline in action, see [README.md — Visual PII Protection](README.md#visual-pii-protection).

```mermaid
flowchart TB
    entry["process_request_body (body.rs)"]

    subgraph pass1 ["Pass 1 — Image Processing"]
        direction LR
        walk["Walk JSON tree"] --> detect["Detect image<br>content blocks"]
        detect --> decode["Decode base64"]
        decode --> exif["EXIF strip"]
        exif --> resize["Resize (960px)"]
        resize --> nsfw["NSFW check<br>(NudeNet)"]
        nsfw -->|"safe"| face["Face blur<br>(BlazeFace)"]
        nsfw -->|"nudity"| fullblur["Full-image blur"]
        face --> ocr["OCR text blur<br>(PaddleOCR)"]
        ocr --> encode["Re-encode<br>→ replace in JSON"]
        fullblur --> encode
    end

    subgraph pass2 ["Pass 2 — Text PII Scanning"]
        direction LR
        scan["scan_json"] --> match["Regex + Keywords<br>+ NER/CRF"]
        match --> encrypt["FF1 FPE encrypt<br>→ replace in JSON"]
    end

    entry --> pass1
    pass1 --> pass2

    style entry fill:#7c6cbf,stroke:#9c8cdf,color:#f0f0f0
    style pass1 fill:#4a7fb5,stroke:#6a9fd5,color:#f0f0f0
    style pass2 fill:#4a6a8a,stroke:#6a8aaa,color:#f0f0f0
```

**Provider formats:**
- **Anthropic:** `{"type":"image","source":{"type":"base64","media_type":"image/png","data":"iVBOR..."}}`
- **OpenAI:** `{"type":"image_url","image_url":{"url":"data:image/png;base64,iVBOR..."}}`

**Key properties:**
- Images processed BEFORE text so byte offsets remain correct
- **Three-phase pipeline:** Phase 0 (NSFW check) → Phase 1 (face detection + blur) → Phase 2 (OCR text detection + blur)
- NSFW detection: if nudity found, blur entire image with heavy sigma=30 and skip face/OCR phases
- Face blur: if face occupies >80% of image area, blur entire image; otherwise selective blur with 15% padding
- Sequential model loading: models loaded/used/dropped one at a time (never multiple in RAM)
- EXIF metadata stripped implicitly — `image` crate loads pixels only, discarding all metadata
- Fail-open: corrupt base64, unsupported format, or model failure → forward original image unchanged
- Screenshot detection (EXIF software tags, screen resolution, status bar uniformity) flags images for aggressive text blur

**Models (on-demand, evicted after 300s idle):**

| Model | Size | RAM | Purpose |
|-------|------|-----|---------|
| NudeNet 320n | ~12MB | ~20MB | NSFW/nudity detection (YOLOv8n, 320x320 input) |
| BlazeFace short-range | ~408KB | ~8MB | Face detection (128x128 input, NMS) |
| PaddleOCR det | ~2.4MB | ~15MB | Text region detection |
| PaddleOCR rec | ~7.8MB | ~20MB | Character recognition (Tier 2 only) |

**Two OCR tiers:**
- **Tier 1 (default):** Detect text regions → blur all. No recognition model needed.
- **Tier 2:** Detect → recognize → scan text for PII → selectively blur PII regions only.

---

## Threat Model

OpenObscure is designed for open-source distribution. Security follows **Kerckhoffs's principle** — the system is secure even when all source code, documentation, and algorithms are public. Security depends entirely on the secrecy of keys, never on code obscurity.

### What OpenObscure Protects Against

| Threat | Protection | Layer |
|--------|-----------|-------|
| PII leaking to LLM providers in API requests | FF1 FPE encryption of structured PII before request leaves device | L0 |
| Visual PII in images (faces, text, EXIF) | NSFW full-image blur, face blur, OCR text blur, EXIF metadata stripping on base64 images | L0 |
| PII persisted in tool result transcripts | Regex redaction of PII in tool outputs before persistence | L1 |
| Frequency analysis of FPE ciphertexts | Per-record tweaks (UUID + JSON path hash) produce unique ciphertexts for identical inputs | L0 |
| API key exposure via proxy | Passthrough-first — keys are never stored or logged by OpenObscure | L0 |

### What OpenObscure Does NOT Protect Against

| Threat | Why | Mitigation |
|--------|-----|------------|
| **Compromised OS / root access** | Attacker with root can read process memory, dump OS keychain, intercept localhost traffic. No userspace software can defend against this. | OS-level security (disk encryption, patching, access controls) |
| **Semantic PII not covered by regex** (Phase 1) | Names, addresses, health conditions bypass regex. "Tell John about my diabetes" passes through unencrypted. | Phase 2 TinyBERT NER closes this gap (~91% coverage) |
| **PII in tool results sent to LLM** | L1 hooks `tool_result_persist` (after LLM sees data), not `before_tool_call`. Tool result PII reaches the LLM before L1 can redact it. | OpenClaw limitation — when `before_tool_call` is wired, L1 upgrades to pre-LLM enforcement |
| **Side-channel attacks on FPE** | Timing analysis of FF1 encrypt/decrypt could theoretically leak information. | AES-NI hardware acceleration provides constant-time operations on supported CPUs |
| **Model extraction from ONNX** (Phase 2+) | NER model weights are readable from the ONNX file. | Not a concern — the model detects PII patterns, it doesn't contain user data. Knowing the model helps craft evasion, but NER is supplementary to regex, not a sole defense |

### Secrets Inventory

All runtime secrets live in the **OS keychain** or (for headless environments) environment variables. Never in source code or config files:

| Secret | Format | Where | Generated |
|--------|--------|-------|-----------|
| FPE master key | 32 bytes (AES-256) | `OPENOBSCURE_MASTER_KEY` env var (64 hex chars) **or** OS keychain (`openobscure/fpe-key`). Env var takes priority. | `--init-key` with `OsRng` |
| L0/L1 auth token | 32 bytes (hex string) | `OPENOBSCURE_AUTH_TOKEN` env var **or** `~/.openobscure/.auth-token` file (0600). Auto-generated on first run. | `OsRng` at startup |

**Key compromise impact:**
- FPE key compromised → all FPE ciphertexts are decryptable (but attacker needs both the key AND the ciphertexts, which exist only in LLM provider logs)

### Open-Source Security Considerations

Publishing source code does **not** weaken OpenObscure's security posture:

1. **Algorithms are public standards** — FF1 (NIST SP 800-38G) is a published, peer-reviewed algorithm. Security never depended on algorithm secrecy.

2. **Regex patterns are standard** — Credit card (Luhn), SSN (range validation), phone, email, and API key patterns are well-known. An attacker doesn't need source code to guess them.

3. **NER models are not secrets** — The TinyBERT model detects PII patterns; it doesn't contain user data. An attacker could study the model to craft evasion inputs, but NER is layered on top of regex, not a sole defense.

4. **Community audit is a net positive** — Cryptographic implementations benefit from public scrutiny. Bugs found by the community are bugs that don't become exploits.

### Attack Surface Reduction

- **Localhost-only binding** — L0 proxy listens on `127.0.0.1:18790`, not `0.0.0.0`. Not network-accessible.
- **Health endpoint auth** — `/_openobscure/health` requires `X-OpenObscure-Token` header. Prevents other localhost processes from querying or impersonating L0.
- **No telemetry** — Zero outbound connections beyond forwarded LLM requests.
- **No default credentials** — FPE key must be explicitly generated. No fallback "demo mode" keys. Auth token auto-generated with secure random on first run.
- **Minimal dependencies** — Rust binary has no runtime dependency beyond libc.
- **Memory-safe language** — L0 is Rust (no buffer overflows, use-after-free, or memory corruption).

---

## FAQ

**Does OpenObscure read local files to scan for PII?**
No. OpenObscure never performs file I/O. The agent's tools (file_read, web_fetch, etc.) read files and produce text results. OpenObscure's L1 plugin only sees the resulting text after the agent has extracted it, via the tool result persistence hook.

**Does OpenObscure need its own API keys?**
No. By default, OpenObscure forwards the host agent's existing API keys unchanged (passthrough-first). It never provisions, generates, or requires separate LLM credentials.

**Does OpenObscure phone home or contact external servers?**
No. The only network traffic OpenObscure produces is forwarding the host agent's existing LLM API requests through the local proxy. No telemetry, no update checks, no external dependencies at runtime. Everything runs locally on the user's device.

**Is L0 (proxy) a separate server I need to host?**
No. L0 runs as a lightweight sidecar process on the same device as the host agent, listening on `127.0.0.1:18790` (localhost only). It's started alongside the agent — either automatically during installation or manually when the user enables OpenObscure. It's not exposed to the network.

**Does OpenObscure intercept data *before* the LLM sees it?**
L0 (proxy) does — it sits in the HTTP path and encrypts PII before the request reaches the LLM provider. L1 (plugin) hooks the agent's tool result persistence (e.g., OpenClaw's `tool_result_persist`), which fires *after* tool execution. L1 prevents PII from being persisted to transcripts, but cannot prevent it from being sent to the LLM via tool results.

**How much RAM does OpenObscure actually use?**
It depends on the device's capability tier. OpenObscure detects hardware at startup and selects features automatically. Lite tier (regex/CRF only): ~12–80MB. Standard tier (NER + images): ~67–200MB. Full tier (NER + ensemble + images): up to 224MB peak. The 275MB ceiling is the hard limit. On mobile, the budget is 20% of device RAM (capped at 275MB), so a 12GB phone gets the same features as a desktop server.

**What happens if OpenObscure is disabled or crashes?**
If L0 is not running, the host agent can't reach LLM providers (traffic is configured to route through the proxy). If L1 crashes, the agent continues normally but tool results won't be redacted. If OpenObscure is fully disabled via configuration, the agent operates with direct LLM connections — zero overhead.

## Future Architecture Changes

Planned (future):
- **ONNX Runtime mobile** — pre-built ORT for iOS (CoreML EP) and Android (NNAPI EP), `.ort` format models
- **SCRFD multi-scale face detection** — SCRFD-2.5GF for mixed-size faces in screenshots (replaces BlazeFace on desktop)
- **Multilingual PII detection** — FastText language identification + per-language regex/keywords for 9 languages
- **GLiNER NER upgrade** — 99.5%+ recall on semantic PII for 16GB+ devices
- **Voice anonymization** — Whisper-base speech-to-text + PII masking in audio (experimental, 16GB+)
