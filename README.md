# OpenObscure

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE)
[![Security: Kerckhoffs](https://img.shields.io/badge/Security-Kerckhoffs-success)](SECURITY.md)

**The Endpoint Privacy Firewall for AI Agents.**

OpenObscure is an open-source privacy firewall that intercepts, sanitizes, and encrypts PII (Personally Identifiable Information) *before* it leaves your device. It works with any AI agent, on any platform. Includes first-class [OpenClaw](https://github.com/openclaw/openclaw) integration.

> **Verify, Don't Trust.** OpenObscure runs entirely on your device. No remote servers, no telemetry, no cloud dependencies.

---

## Two Deployment Models

OpenObscure can protect AI agents in two ways, depending on where the agent runs:

### Gateway Model (Desktop / Server)

The proxy runs as a **sidecar process** on the same host as the AI agent. All LLM API traffic routes through it. This is the full-featured deployment with both layers active.

```mermaid
flowchart LR
    %% Direction and Grouping
    subgraph LocalEnv [" Local Machine / VPC "]
        direction LR
        agent["<b>AI Agent</b><br/>(Client Application)"]
        proxy["<b>OpenObscure Proxy</b><br/>(localhost:18790)"]
    end

    subgraph External [" Public Cloud "]
        llm["<b>LLM Provider</b><br/>(External API)"]
    end

    %% Solid lines for outward requests
    agent -- "1. HTTP Request" --> proxy
    proxy == "2. HTTPS (PII Encrypted)" ==> llm
    
    %% Dashed lines for returning responses
    llm -. "3. Response (Ciphertexts)" .-> proxy
    proxy -. "4. Response (PII Decrypted)" .-> agent

    %% --- AWS STYLE STYLING ---
    
    %% Local/Compute Styling (AWS Blue)
    style LocalEnv fill:#f2f5f7,stroke:#232F3E,stroke-width:2px,color:#232F3E
    style agent fill:#3b48cc,stroke:#232F3E,stroke-width:2px,color:#fff
    
    %% Proxy/Security Styling (AWS Slate/Grey)
    style proxy fill:#545b64,stroke:#232F3E,stroke-width:2px,color:#fff

    %% External Provider Styling (AWS Orange)
    style External fill:#fff7ed,stroke:#ff9900,stroke-width:2px,color:#232F3E
    style llm fill:#ff9900,stroke:#232F3E,stroke-width:2px,color:#fff

    %% Link Styling
    linkStyle 0,3 stroke:#545b64,stroke-width:2px
    linkStyle 1,2 stroke:#ff9900,stroke-width:3px
```

- **Platforms:** macOS, Linux (x64 + ARM64), Windows
- **Layers:** L0 (Rust proxy) + L1 (TypeScript plugin)
- **Features:** Full PII scanning (regex + NER/CRF + keywords + network/device identifiers), FPE encryption, image pipeline (face blur, OCR blur, EXIF strip), SSE streaming
- **Use case:** Desktop apps, servers, VPS, Raspberry Pi — anywhere the agent's Gateway runs

### Embedded Model (Mobile / Library)

OpenObscure is compiled as a **native library** and linked directly into the host application. Called via UniFFI-generated Swift/Kotlin bindings. No HTTP server, no sockets — just function calls.

```mermaid
flowchart TB
    %% Direction and Layout
    direction TB

    subgraph UserDevice [" User Device (Mobile) "]
        direction TB
        app["<b>Mobile App</b><br/>(Swift / Kotlin)"]
        lib["<b>OpenObscure Lib</b><br/>(In-Process)"]
        
        %% Internal logic
        app -- "1. sanitize_text()" --> lib
        lib -- "2. Result + Mapping" --> app
    end

    subgraph Cloud [" External Computer / Cloud "]
        gw["<b>Gateway</b><br/>(WebSocket Entry)"]
    end

    %% Network Connections
    app == "3. Secure WebSocket<br/>(PII Encrypted)" ==> gw
    gw -. "4. Response" .-> app
    
    %% Final local step
    app -. "5. restore_text()" .-> lib

    %% --- AWS STYLE STYLING ---
    
    %% Device Styling (AWS Blue/Grey tones)
    style UserDevice fill:#f2f5f7,stroke:#232F3E,stroke-width:2px,color:#232F3E
    style app fill:#3b48cc,stroke:#232F3E,stroke-width:2px,color:#fff
    style lib fill:#545b64,stroke:#232F3E,stroke-width:2px,color:#fff

    %% Cloud Styling (AWS Orange tones)
    style Cloud fill:#fff7ed,stroke:#ff9900,stroke-width:2px,color:#232F3E
    style gw fill:#ff9900,stroke:#232F3E,stroke-width:2px,color:#fff

    %% Link Styling
    linkStyle 0,1,4 stroke:#545b64,stroke-width:2px
    linkStyle 2,3 stroke:#ff9900,stroke-width:3px
```

- **Platforms:** iOS (aarch64), Android (arm64-v8a, armeabi-v7a, x86_64)
- **Layers:** L0 (PII scan + FPE)
- **Features:** Text PII scanning (regex + keywords + NER/CRF on capable devices), FPE encryption, image pipeline, restore/decrypt for responses
- **Use case:** Mobile companion apps that sanitize PII on-device *before* data reaches the Gateway over WebSocket — defense in depth

### When to Use Which

| Scenario | Model | Why |
|----------|-------|-----|
| Desktop AI agent (e.g. OpenClaw Gateway) | Gateway | Full feature set, both layers |
| Server / VPS deployment | Gateway | Same binary, headless key management |
| iOS / Android companion app | Embedded | On-device PII protection, native bindings |
| Custom Rust application | Embedded | Link as a library crate, call directly |
| Edge device (Raspberry Pi) | Gateway | Full features, runs on ARM Linux |

---

## Hardware Capability Detection

OpenObscure detects device hardware at startup and automatically selects features based on what the device can support. A phone with 12GB RAM gets the same PII detection efficacy as a desktop server.

### Capability Tiers

| Device RAM | Tier | Scanners | Image Pipeline | Model Idle Timeout |
|------------|------|----------|----------------|--------------------|
| 8GB+ | **Full** | NER + CRF + ensemble voting | Yes | 300s |
| 4–8GB | **Standard** | NER + CRF (no ensemble) | Yes | 120s |
| <4GB | **Lite** | CRF + regex only | Yes (shorter timeout) | 60s |

Tier classification uses **total physical RAM** — a stable device indicator that doesn't fluctuate with app usage.

### Gateway vs Embedded Budgets

- **Gateway** (desktop/server): Fixed RAM budget per tier (Full=275MB, Standard=200MB, Lite=80MB)
- **Embedded** (mobile): 20% of total device RAM, capped at 275MB. A 12GB phone gets a 275MB budget (Full tier). A 6GB phone gets 275MB capped from 1228MB (Standard). A 3GB phone gets ~614MB budget (Lite)

### Explicit Override

Hardware auto-detection is the default when `scanner_mode = "auto"`. You can bypass it with an explicit mode:

```toml
# config/openobscure.toml
[scanner]
scanner_mode = "ner"    # Force NER regardless of device tier
# scanner_mode = "crf"  # Force CRF
# scanner_mode = "regex" # Force regex-only
```

On mobile, set `auto_detect: false` in `MobileConfig` to disable hardware profiling.

### Health Endpoint

The health endpoint reports the detected tier and active feature budget:

```bash
curl -s http://127.0.0.1:18790/_openobscure/health | jq '.device_tier, .feature_budget'
```

```json
"full"
{
  "tier": "full",
  "max_ram_mb": 275,
  "ner_enabled": true,
  "crf_enabled": true,
  "ensemble_enabled": true,
  "image_pipeline_enabled": true
}
```

### Supported Platforms

Hardware detection is implemented for all supported platforms:

| Platform | Total RAM | Available RAM |
|----------|-----------|---------------|
| macOS / iOS | `sysctl hw.memsize` | `vm_stat` (macOS) |
| Linux / Android | `/proc/meminfo MemTotal` | `/proc/meminfo MemAvailable` |
| Windows | `GlobalMemoryStatusEx` | `GlobalMemoryStatusEx` |

---

## Architecture

OpenObscure uses a **Sidecar + Plugin** hybrid architecture (Gateway Model) to provide Defense-in-Depth:

```mermaid
flowchart LR
    %% Direction and Grouping
    subgraph Device [" User Device Boundary "]
        direction LR
        
        subgraph Agent [" AI Agent Environment "]
            direction TB
            tools["<b>Agent Tools</b>"]
            L1["<b>L1 Plugin</b><br/>(PII Redact)"]
            
            %% Spacing internal to Agent
            tools -- "tool results" --> L1
        end

        subgraph Proxy [" L0 Proxy Layer (Rust) "]
            direction TB
            scanner["<b>Hybrid Scanner</b>"]
            
            subgraph Processing [" Transformation "]
                direction LR
                fpe["<b>FPE Encrypt</b>"]
                img["<b>Image Pipeline</b>"]
            end
            
            scanner --> fpe
            scanner --> img
        end
        
        %% Connect Subgraphs
        Agent == "HTTP (Localhost)" ==> Proxy
    end

    subgraph External [" Public Cloud / SaaS "]
        llm["<b>LLM Providers</b><br/>(External API)"]
    end

    %% Network Connections
    Proxy == "HTTPS (PII Encrypted)" ==> llm
    llm -. "Response (Ciphertexts)" .-> Proxy

    %% --- AWS STYLE STYLING ---
    
    %% Main Boundary
    style Device fill:#f2f5f7,stroke:#232F3E,stroke-width:2px,color:#232F3E
    
    %% Internal Subnets
    style Agent fill:#e6f3f7,stroke:#3b48cc,stroke-dasharray: 5 5,color:#232F3E
    style Proxy fill:#e6f3f7,stroke:#545b64,stroke-dasharray: 5 5,color:#232F3E
    style Processing fill:#ffffff,stroke:#545b64,stroke-dasharray: 2 2,color:#232F3E

    %% Node Styling
    style tools fill:#3F4756,stroke:#545b64,color:#fff
    style L1 fill:#9D7BED,stroke:#232F3E,color:#fff
    style scanner fill:#545b64,stroke:#232F3E,color:#fff
    style fpe fill:#545b64,stroke:#232F3E,color:#fff
    style img fill:#545b64,stroke:#232F3E,color:#fff

    %% External Provider Styling
    style External fill:#fff7ed,stroke:#ff9900,stroke-width:2px,color:#232F3E
    style llm fill:#ff9900,stroke:#232F3E,stroke-width:2px,color:#fff

    %% Link Styling
    linkStyle 2 stroke:#3b48cc,stroke-width:3px
    linkStyle 3 stroke:#ff9900,stroke-width:3px
    linkStyle 4 stroke:#ff9900,stroke-width:3px
```

| Layer | Language | What it does |
|-------|----------|-------------|
| **L0** — PII Proxy | Rust | Intercepts HTTP traffic, scans JSON for PII (structured, network/device, semantic, keywords), encrypts with FF1 FPE or redacts. Processes images (face blur, OCR text blur, EXIF strip). |
| **L1** — Gateway Plugin | TypeScript | Hooks tool results, redacts PII. Heartbeat monitor for L0 health. |

For the full architecture, see [ARCHITECTURE.md](ARCHITECTURE.md).

---

## Quick Start

### Prerequisites

- **Rust** 1.75+ (for L0 proxy)
- **Node.js** 20+ (for L1 plugin)
- An AI agent that makes HTTP calls to an LLM provider (OpenClaw, custom agents, etc.)

### 1. Build the proxy

```bash
cd openobscure-proxy
cargo build --release
```

### 2. Generate an FPE key (first time only)

```bash
cargo run --release -- --init-key
```

This stores a 256-bit AES key in your OS keychain. For headless/Docker environments, set `OPENOBSCURE_MASTER_KEY` (64 hex chars) instead.

### 3. Start the proxy

```bash
cargo run --release -- -c config/openobscure.toml
```

The proxy listens on `127.0.0.1:18790` by default.

### 4. Verify

```bash
curl -H "X-OpenObscure-Token: $(cat ~/.openobscure/.auth-token)" \
     http://127.0.0.1:18790/_openobscure/health
```

You should see a JSON response with `"status": "ok"`.

### OpenClaw Integration

Point OpenClaw's LLM traffic through the proxy:

```
LLM_API_BASE=http://127.0.0.1:18790
```

Optionally, copy `openobscure-plugin/` into OpenClaw's `extensions/` directory and enable it in OpenClaw's plugin config for L1 in-process redaction.

### Generic Integration (Any AI Agent)

Any AI agent that sends HTTP requests to an LLM provider can route traffic through the L0 proxy. Set your agent's LLM base URL to the proxy address:

```
http://127.0.0.1:18790
```

The proxy transparently intercepts JSON payloads, scans for PII, applies FF1 Format-Preserving Encryption, and forwards the sanitized request to the upstream LLM provider. Responses are decrypted before being returned to your agent.

For programmatic access to the L1 redaction from TypeScript/JavaScript, import directly from the plugin core:

```typescript
import { redactPii } from "openobscure-plugin/core";

// Scan text for PII
const result = redactPii(toolOutput);
if (result.count > 0) toolOutput = result.text;
```

This allows any agent — not just OpenClaw — to leverage OpenObscure's PII redaction as a library.

---

## Configuration

OpenObscure is configured via `config/openobscure.toml`. Key sections:

```toml
[proxy]
listen_addr = "127.0.0.1:18790"
fail_mode = "open"          # "open" or "closed"

[scanner]
respect_code_fences = true  # Skip PII inside markdown code blocks
nested_json_depth = 2       # Scan PII inside escaped JSON strings

[image]
enabled = true
face_detection = true
ocr_enabled = true
ocr_tier = "detect_and_blur"  # "detect_and_blur" or "full_recognition"
max_dimension = 960

[logging]
json_output = false
pii_scrub = true
```

See `config/openobscure.toml` for all available options.

---

## Running Tests

```bash
# L0 Proxy
cd openobscure-proxy && cargo test

# L1 Plugin
cd openobscure-plugin && npm test
```

---

## How It Works

OpenObscure uses **Format-Preserving Encryption (FF1)** to replace PII with realistic-looking ciphertext. The LLM sees plausible data, preserving conversational context, while the real values never leave your device.

```mermaid
sequenceDiagram
    participant U as User
    participant A as AI Agent
    participant P as OpenObscure Proxy
    participant L as LLM Provider

    U->>A: "My card is 4111-1111-1111-1111"
    A->>P: Agent sends LLM request
    Note over P: FF1 encrypt → 8714-3927-6051-2483
    P->>L: "My card is 8714-3927-6051-2483"
    Note over L: LLM sees plausible data,<br/>never the real card number
    L->>P: "The card ending in 2483..."
    Note over P: FF1 decrypt → 1111
    P->>A: "The card ending in 1111..."
    A->>U: "The card ending in 1111..."
```

PII detection uses a hybrid approach:
- **Regex** with post-validation (Luhn for credit cards, range checks for SSNs, IPv4 validation)
- **Network/device identifiers** — IPv4, IPv6, GPS coordinates, MAC addresses (redacted to `[IPv4]`, `[IPv6]`, `[GPS]`, `[MAC]`)
- **NER/CRF** (TinyBERT INT8) for semantic detection (names, addresses, orgs)
- **Keyword dictionary** (~700 terms) for health and child-related terms
- **Image pipeline** (BlazeFace + PaddleOCR ONNX) for visual PII in photos

---

## Visual PII Protection

OpenObscure doesn't just protect text — it also processes **images** for visual PII before they reach the LLM. The image pipeline runs entirely on-device using lightweight ONNX models.

### How Image Processing Works

```mermaid
sequenceDiagram
    participant U as User
    participant A as AI Agent
    participant P as OpenObscure Proxy
    participant L as LLM Provider

    U->>A: Send photo via agent
    A->>P: Photo (base64 in JSON)
    Note over P: 1. Detect base64 image<br/>2. Decode + EXIF strip<br/>3. Resize (max 960px)
    Note over P: 4. NSFW check<br/>5. Face detect + blur<br/>6. OCR detect + blur
    P->>L: Sanitized image
    Note over L: LLM sees the image<br/>but PII is obscured
    L->>P: Response about image
    P->>A: Response forwarded
    A->>U: Agent presents response
```

### Before / After Examples

These examples were generated by running real images through the OpenObscure image pipeline using the `demo_image_pipeline` example binary:

```bash
# Download ONNX models (one-time)
./scripts/download_models.sh

# Process an image
cargo run --example demo_image_pipeline -- \
  --input photo.jpg --output photo-blurred.jpg
```

| Scenario | Before | After |
|----------|--------|-------|
| **Screenshot PII Blur** — Full patient record with names, SSNs, credit cards, phone numbers, emails, addresses, and medical history. PaddleOCR detects all text regions and blurs them. | ![Original screenshot](docs/examples/images/screenshot-original.png) | ![Screenshot blurred](docs/examples/images/screenshot-blurred.png) |
| **Face Detection + Blur** — BlazeFace detects faces and applies selective Gaussian blur to face bounding boxes | ![Original face photo](docs/examples/images/face-original.jpg) | ![Face blurred](docs/examples/images/face-blurred.jpg) |
| **Child Face Privacy** — Automatically detects and blurs children's faces to protect minors' privacy | ![Original child photo](docs/examples/images/child-original.jpg) | ![Child face blurred](docs/examples/images/child-blurred.jpg) |
| **OCR PII Blur** — PaddleOCR detects text regions, then PII scanner filters and blurs only sensitive values (names, SSNs, emails, phones, card numbers) while leaving non-PII text readable | ![Original document](docs/examples/images/text-original.jpg) | ![PII blurred](docs/examples/images/text-blurred.jpg) |

### Pipeline Details

The image pipeline processes images in three phases:

1. **NSFW detection** (Phase 0): NudeNet 320n ONNX (~12MB) checks for nudity. If detected, the entire image is blurred with heavy sigma=30 and subsequent phases are skipped.
2. **Face detection** (Phase 1): BlazeFace short-range ONNX (~408KB), 128x128 input, 896 anchors. Faces occupying >80% of the image trigger full-image blur; otherwise, selective Gaussian blur (sigma=25) is applied to the face bounding box with 15% padding.
3. **Text detection** (Phase 2): PaddleOCR v3 ONNX (~2.4MB), detects text regions, applies Gaussian blur (sigma=20) with vertical padding for complete coverage.

Additional features:
- **EXIF stripping**: Automatically removes GPS coordinates, camera model, timestamps from photos
- **Fail-open**: If a model fails to load, the pipeline skips that step and forwards the image as-is
- **Lazy loading**: Models are loaded on first use and evicted after idle timeout (default: 5 minutes)
- **Memory ceiling**: Models are loaded sequentially (never all in RAM) to stay within the device's feature budget (up to 275MB)

---

## Mobile Library (Embedded Model)

For iOS and Android apps, OpenObscure compiles as a native library with a simple API. Hardware capability detection runs at initialization — a phone with 8GB+ RAM automatically gets NER, CRF, ensemble voting, and full image pipeline, matching gateway-level efficacy.

```rust
// Initialize with FPE key from host app's secure storage
// auto_detect: true (default) — profiles device hardware and selects features
let mobile = OpenObscureMobile::new(MobileConfig::default(), fpe_key)?;

// Sanitize text before sending to Gateway
let result = mobile.sanitize_text("My card is 4111-1111-1111-1111")?;
// result.sanitized_text = "My card is 8714-3927-6051-2483"
// result.mapping_json = ... (save for response decryption)

// Restore original values in responses
let restored = mobile.restore_text(&response, &result.mapping_json);

// Check device tier and active features
let stats = mobile.stats();
println!("Device tier: {}", stats.device_tier); // "full", "standard", or "lite"
```

**Swift (iOS)** and **Kotlin (Android)** bindings are auto-generated by [UniFFI](https://github.com/mozilla/uniffi-rs). Build scripts provided:

```bash
# iOS (device + simulator, optional XCFramework)
./scripts/build_ios.sh --release --xcframework

# Android (ARM64, optional all ABIs)
./scripts/build_android.sh --release --all-abis
```

See [ARCHITECTURE.md — Embedded Model](ARCHITECTURE.md#embedded-model-mobile--library) for integration details.

---

## Supported Platforms

| Platform | Model | Binary | Status |
|----------|-------|--------|--------|
| macOS (Apple Silicon) | Gateway | `openobscure-proxy` | Full support |
| Linux x86_64 | Gateway | `openobscure-proxy` | Full support |
| Linux ARM64 | Gateway | `openobscure-proxy` | Build verified |
| Windows x86_64 | Gateway | `openobscure-proxy.exe` | Build support (RAM detection, keyring) |
| iOS (aarch64) | Embedded | `libopenobscure_proxy.a` | Library + UniFFI bindings |
| Android (arm64-v8a) | Embedded | `libopenobscure_proxy.so` | Library + UniFFI bindings |

---

## Security

OpenObscure follows **Kerckhoffs's principle** — security depends on the secrecy of keys, not code. All algorithms (FF1) are public NIST standards. Publishing source code does not weaken the system.

Key properties:
- **No telemetry** — zero outbound connections beyond forwarded LLM requests
- **Localhost-only** — proxy binds to `127.0.0.1`, not `0.0.0.0`
- **No default credentials** — FPE key must be explicitly generated
- **Memory-safe** — L0 is written in Rust

For the full security policy and vulnerability reporting instructions, see [SECURITY.md](SECURITY.md).

For the threat model, see [ARCHITECTURE.md — Threat Model](ARCHITECTURE.md#threat-model).

---

## Export Control

This software contains cryptographic functionality (FF1, TLS) and may be subject to export restrictions. See [EXPORT_CONTROL_NOTICE.md](EXPORT_CONTROL_NOTICE.md) for details.

---

## Contributing

Contributions are welcome. Please:

1. Read [ARCHITECTURE.md](ARCHITECTURE.md) to understand the system design
2. Run the full test suite before submitting PRs
3. Follow existing code conventions (Rust: `cargo clippy`, TypeScript: strict mode)
4. Report security vulnerabilities via [GitHub private reporting](SECURITY.md), not public issues

---

## License

OpenObscure is dual-licensed under **MIT** or **Apache-2.0**, at your option.

- [LICENSE](LICENSE) (MIT with Apache-2.0 option)
- Each component's dependency licenses are audited in their respective `LICENSE_AUDIT.md` files

See [EXPORT_CONTROL_NOTICE.md](EXPORT_CONTROL_NOTICE.md) for export control information.
