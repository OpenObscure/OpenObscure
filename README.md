# OpenObscure

[![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE)
[![Security: Kerckhoffs](https://img.shields.io/badge/Security-Kerckhoffs-success)](SECURITY.md)

**The Endpoint Privacy Firewall for AI Agents.**

OpenObscure is an open-source security sidecar that intercepts, sanitizes, and encrypts PII (Personally Identifiable Information) *before* it leaves your device. It is designed to work with any AI agent. Includes first-class [OpenClaw](https://github.com/openclaw/openclaw) integration.

> **Verify, Don't Trust.** OpenObscure runs entirely on `localhost`. No remote servers, no telemetry, no cloud dependencies.

---

## Architecture

OpenObscure is not a monolithic app. It uses a **Sidecar + Plugin** hybrid architecture to provide Defense-in-Depth:

```mermaid
flowchart LR
    subgraph device ["🖥️ User's Device"]
        direction TB
        subgraph agent ["AI Agent (e.g. OpenClaw)"]
            tools["🔧 Agent Tools\nweb · file · API · bash"]
            L1["🛡️ L1 — Gateway Plugin\n(TypeScript)\nPII redact · file guard\nconsent · retention"]
            tools -- "tool results" --> L1
        end
        subgraph proxy ["L0 — PII Proxy (Rust)"]
            scanner["🔍 Hybrid Scanner\nregex · NER/CRF · keywords"]
            fpe["🔐 FF1 FPE Encrypt"]
            img["📷 Image Pipeline\nNSFW · face blur · OCR blur · EXIF strip"]
            scanner --> fpe
            scanner --> img
        end
        subgraph storage ["L2 — Crypto Store"]
            crypt[("🗄️ AES-256-GCM\nArgon2id KDF\nEncrypted Transcripts")]
        end
        agent -- "HTTP (localhost)" --> proxy
        L1 -. "redacted text" .-> crypt
    end
    proxy -- "sanitized request\n(PII encrypted)" --> llm["☁️ LLM Providers\nAnthropic · OpenAI\nOllama · etc."]
    llm -- "response\n(ciphertexts)" --> proxy

    style device fill:#1a1a2e,stroke:#16213e,color:#e0e0e0
    style agent fill:#0f3460,stroke:#533483,color:#e0e0e0
    style proxy fill:#533483,stroke:#e94560,color:#e0e0e0
    style storage fill:#16213e,stroke:#0f3460,color:#e0e0e0
    style llm fill:#e94560,stroke:#e94560,color:#fff
```

| Layer | Language | What it does |
|-------|----------|-------------|
| **L0** — PII Proxy | Rust | Intercepts HTTP traffic, scans JSON for PII, encrypts with FF1 FPE. Processes images (face blur, OCR text blur, EXIF strip). Includes compliance CLI (`openobscure compliance ...`). |
| **L1** — Gateway Plugin | TypeScript | Hooks tool results, redacts PII, blocks sensitive file reads, manages GDPR consent and memory governance. |
| **L2** — Encryption Layer | Rust | AES-256-GCM encryption for session transcripts at rest. |

For the full architecture, see [ARCHITECTURE.md](ARCHITECTURE.md).

---

## Quick Start

### Prerequisites

- **Rust** 1.75+ (for L0 proxy and L2 crypto)
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

For programmatic access to the L1 redaction and file-access guard from TypeScript/JavaScript, import directly from the plugin core:

```typescript
import { redactPii, checkFileAccess } from "openobscure-plugin/core";

// Scan text for PII
const result = redactPii(toolOutput);
if (result.count > 0) toolOutput = result.text;

// Check file safety before reading
const check = checkFileAccess("/path/to/file");
if (!check.allowed) throw new Error(check.reason);
```

This allows any agent — not just OpenClaw — to leverage OpenObscure's PII redaction and file access controls as a library.

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
# L0 Proxy (306 tests)
cd openobscure-proxy && cargo test

# L2 Crypto (16 tests)
cd openobscure-crypto && cargo test

# L1 Plugin (96 tests)
cd openobscure-plugin && npm test
```

**Total: 418 tests** across all components.

---

## How It Works

OpenObscure uses **Format-Preserving Encryption (FF1)** to replace PII with realistic-looking ciphertext. The LLM sees plausible data, preserving conversational context, while the real values never leave your device.

```mermaid
sequenceDiagram
    participant U as 👤 User
    participant A as 🤖 AI Agent
    participant P as 🔐 OpenObscure Proxy
    participant L as ☁️ LLM Provider

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
- **Regex** with post-validation (Luhn for credit cards, range checks for SSNs)
- **NER/CRF** (TinyBERT INT8) for semantic detection (names, addresses, orgs)
- **Keyword dictionary** (~700 terms) for health and child-related terms
- **Image pipeline** (BlazeFace + PaddleOCR ONNX) for visual PII in photos

---

## Visual PII Protection

OpenObscure doesn't just protect text — it also processes **images** for visual PII before they reach the LLM. The image pipeline runs entirely on-device using lightweight ONNX models.

### How Image Processing Works

```mermaid
sequenceDiagram
    participant U as 👤 User
    participant A as 🤖 AI Agent
    participant P as 🔐 OpenObscure Proxy
    participant L as ☁️ LLM Provider

    U->>A: Send photo via agent
    A->>P: Photo (base64 in JSON)
    Note over P: 1. Detect base64 image<br/>2. Decode + EXIF strip<br/>3. Resize (max 960px)
    Note over P: 4. NudeNet: NSFW check<br/>(if nudity → blur all, stop)
    Note over P: 5. BlazeFace: detect faces<br/>6. Gaussian blur face regions
    Note over P: 7. PaddleOCR: detect text<br/>8. Gaussian blur text regions
    P->>L: Sanitized image (faces + text blurred)
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
| **Face Detection + Blur** — BlazeFace detects faces and applies selective Gaussian blur to the face bounding box | ![Original face photo](docs/examples/images/face-original.jpg) | ![Face blurred](docs/examples/images/face-blurred.jpg) |
| **Child Face Privacy** — Automatically detects and blurs children's faces to protect minors' privacy | ![Original child photo](docs/examples/images/child-original.jpg) | ![Child face blurred](docs/examples/images/child-blurred.jpg) |
| **OCR Text Blur** — PaddleOCR detects text in documents/photos and blurs readable content | ![Original document](docs/examples/images/text-original.jpg) | ![Text blurred](docs/examples/images/text-blurred.jpg) |
| **Screenshot PII Blur** — Detects and blurs PII text in screenshots (names, SSNs, phone numbers, addresses) | ![Original screenshot](docs/examples/images/screenshot-original.png) | ![Screenshot blurred](docs/examples/images/screenshot-blurred.png) |

### Pipeline Details

The image pipeline processes images in three phases:

1. **NSFW detection** (Phase 0): NudeNet 320n ONNX (~12MB) checks for nudity. If detected, the entire image is blurred with heavy sigma=30 and subsequent phases are skipped.
2. **Face detection** (Phase 1): BlazeFace short-range ONNX (~408KB), 128x128 input, 896 anchors. Faces occupying >80% of the image trigger full-image blur; otherwise, selective Gaussian blur (sigma=25) is applied to the face bounding box with 15% padding.
3. **Text detection** (Phase 2): PaddleOCR v3 ONNX (~2.4MB), detects text regions, applies Gaussian blur (sigma=20) with vertical padding for complete coverage.

Additional features:
- **EXIF stripping**: Automatically removes GPS coordinates, camera model, timestamps from photos
- **Fail-open**: If a model fails to load, the pipeline skips that step and forwards the image as-is
- **Lazy loading**: Models are loaded on first use and evicted after idle timeout (default: 5 minutes)
- **Memory ceiling**: Models are loaded sequentially (never all in RAM) to stay within 275MB budget

---

## Security

OpenObscure follows **Kerckhoffs's principle** — security depends on the secrecy of keys, not code. All algorithms (FF1, AES-256-GCM, Argon2id) are public NIST/OWASP standards. Publishing source code does not weaken the system.

Key properties:
- **No telemetry** — zero outbound connections beyond forwarded LLM requests
- **Localhost-only** — proxy binds to `127.0.0.1`, not `0.0.0.0`
- **No default credentials** — FPE key must be explicitly generated
- **Memory-safe** — L0 and L2 are written in Rust

For the full security policy and vulnerability reporting instructions, see [SECURITY.md](SECURITY.md).

For the threat model, see [ARCHITECTURE.md — Threat Model](ARCHITECTURE.md#threat-model).

---

## Export Control

This software contains cryptographic functionality (AES-256-GCM, FF1, Argon2id, TLS) and may be subject to export restrictions. See [EXPORT_CONTROL_NOTICE.md](EXPORT_CONTROL_NOTICE.md) for details.

---

## Project Status

| Phase | Status | Coverage |
|-------|--------|----------|
| Phase 1 — Structured PII (regex + FPE) | Complete | 78% |
| Phase 2 — Semantic PII (NER + GDPR) | Complete | 91% |
| Phase 2.5 — Unified Logging | Complete | 91% |
| Phase 3 — Visual PII (faces + OCR) | Complete | 95% |
| Phase 4 — Compliance CLI + Hardening | Complete | 97% |
| Phase 5 — Key Rotation + Benchmarks | Complete | 97% |
| Phase 6 — Ensemble Recall + Cleanup | Complete | 97% |

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

See [EXPORT_CONTROL_NOTICE.md](EXPORT_CONTROL_NOTICE.md) for cryptographic export control information.
