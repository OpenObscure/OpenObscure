# OpenObscure Proxy — Architecture

> Layer 0 of the OpenObscure privacy firewall. See `../project-plan/MASTER_PLAN.md` for full system architecture.

---

## Role in OpenObscure

The Rust PII proxy is the **hard enforcement** layer. It sits between the AI agent and LLM providers as an HTTP reverse proxy. Every API request passes through it — there is no bypass path.

```
┌──────────────┐         ┌──────────────────┐         ┌──────────────┐
│   AI Agent   │  HTTP   │  OpenObscure Proxy  │  HTTPS  │ LLM Provider │
│  (Host)      │ ──────► │  (127.0.0.1:18790)│ ──────► │ (Anthropic,  │
│              │ ◄────── │                    │ ◄────── │  OpenAI...)  │
└──────────────┘         └──────────────────┘         └──────────────┘
                          │                  │
                          │  Request path:   │
                          │  Scan → FPE      │
                          │  encrypt PII     │
                          │                  │
                          │  Response path:  │
                          │  FPE decrypt     │
                          │  ciphertexts     │
                          └──────────────────┘
```

## Module Map

```
src/
├── main.rs              Entry point: CLI (clap subcommands), config, vault init, auth token, server startup, model eviction
├── config.rs            TOML config deserialization and validation (including ImageConfig)
├── server.rs            axum Router, middleware stack, graceful shutdown
├── proxy.rs             Reverse proxy handler (the core request/response loop)
│
│   ── Text PII ──
├── scanner.rs           PII regex scanner (RegexSet + individual Regex)
├── hybrid_scanner.rs    Hybrid scanner: regex → keywords → NER/CRF, dedup, nested JSON, code fences
├── keyword_dict.rs      Health/child keyword dictionary (~700 terms, HashSet O(1) lookup)
├── ner_scanner.rs       TinyBERT INT8 ONNX NER (BIO tags → PII spans)
├── crf_scanner.rs       CRF fallback classifier (<200MB RAM devices) + device profiler
├── wordpiece.rs         WordPiece tokenizer for NER input
├── fpe_engine.rs        FF1 FPE encrypt/decrypt engine
├── pii_types.rs         PII type definitions, alphabet mappers, format templates
├── mapping.rs           Per-request FPE mapping store for response decryption
├── body.rs              Two-pass body processing: images first, then text PII scanning
│
│   ── Visual PII (Phase 3) ──
├── image_detect.rs      Base64 image detection in JSON (Anthropic + OpenAI formats)
├── image_pipeline.rs    ImageModelManager orchestrator: decode → resize → face → OCR → encode
├── face_detector.rs     BlazeFace ONNX face detection (128x128, NMS, anchor decoding)
├── ocr_engine.rs        PaddleOCR det+rec ONNX (text region detection, CTC decode)
├── image_blur.rs        Gaussian blur for face and text regions (sub-image extract/paste)
├── screen_guard.rs      Screenshot heuristics (EXIF, resolution, status bar uniformity)
│
│   ── Compliance & Cross-Border (Phase 4) ──
├── compliance.rs        CLI dispatch: ROPA/DPIA/summary/audit-log/breach-check/export handlers
├── cross_border.rs      Jurisdiction classifier (phone codes, email TLDs, SSN) + policy engine
├── breach_detect.rs     Anomaly scoring (hourly buckets, mean+stddev) + Art. 33 notification draft
│
│   ── Infrastructure ──
├── vault.rs             OS keychain + env var bridge (FPE key + API keys)
├── health.rs            Health endpoint, HealthStats, crash marker, image + cross-border counters
├── cg_log.rs            Unified logging macros (cg_info!, cg_warn!, cg_audit!) + module constants
├── pii_scrub_layer.rs   PII scrub filter for log output (tracing MakeWriter wrapper)
├── crash_buffer.rs      mmap ring buffer for crash diagnostics (survives SIGKILL/OOM)
├── error.rs             Unified error types
└── integration_tests.rs E2E tests (wiremock + tower::oneshot)
```

## Request Flow

```
1. Host agent sends request to proxy
        │
2. proxy_handler() receives request
        │
3. resolve_provider() — match path prefix to upstream URL
        │
4. Buffer request body (enforce size limit)
        │
5. Pass 1: Image processing (if image.enabled)
   │   a. Walk JSON tree for base64 image content blocks
   │      (Anthropic: type="image" + source.data, OpenAI: type="image_url" + data: URI)
   │   b. For each image: decode base64 → screen guard check → resize (960px max)
   │   c. Face detection: BlazeFace ONNX → NMS → Gaussian blur face regions
   │   d. OCR: PaddleOCR det → text regions → blur (Tier 1) or recognize+scan (Tier 2)
   │   e. Encode processed image → replace base64 in JSON
   │   f. Sequential model loading: face model dropped before OCR loaded
   │
6. Pass 2: Text PII scanning
   │   hybrid_scanner.scan_json() — multi-layer scan
   │   a. Regex scanner (CC, SSN, phone, email, API keys) + post-validation
   │   b. Keyword dictionary (health/child terms, ~700 entries)
   │   c. NER/CRF semantic scanner (names, addresses, orgs) if model loaded
   │   d. Deduplicate overlapping spans (regex wins on overlap)
   │   e. Nested JSON: parse serialized JSON strings, scan recursively (max depth 2)
   │   f. Code fences: mask content inside ``` and ` blocks before scanning
   │   - Skip configured fields (model, temperature, etc.)
   │   - Return Vec<PiiMatch> with byte offsets + JSON paths
   │
7. For each PiiMatch:
   │   a. extract_encryptable() — split prefix/domain from encryptable part
   │   b. FormatTemplate::from_raw() — strip separators (dashes, spaces)
   │   c. AlphabetMapper::string_to_numerals() — convert to Vec<u16>
   │   d. Validate domain size (radix^len ≥ 1,000,000)
   │   e. FF1<Aes256>::encrypt(tweak, numerals) — NIST SP 800-38G
   │   f. Reconstruct: numerals → string → re-insert separators → reattach context
   │   g. Store mapping: ciphertext → (plaintext, tweak, type)
   │
8. Apply replacements to JSON body (reverse offset order)
        │
8b. Cross-border jurisdiction classification (if enabled)
    │   Reconstruct PiiMatch from FpeMapping entries
    │   Classify jurisdiction: phone → country codes, email → TLDs, SSN → US
    │   Evaluate policy: allow / warn (log) / block (403 Forbidden)
    │   Record cross_border_flags_total in HealthStats
        │
9. Forward modified request to upstream LLM provider (HTTPS)
        │
10. Buffer upstream response
        │
11. For each stored mapping:
    │   - String-replace ciphertext with plaintext in response
    │   - Sort by ciphertext length desc (prevent partial matches)
    │
12. Return decrypted response to the host agent
        │
13. Clean up request mappings from store
```

## FPE (Format-Preserving Encryption)

**Algorithm:** FF1 per NIST SP 800-38G. FF3 is WITHDRAWN — never used.

**How it works:** Transforms plaintext to ciphertext of **identical format**. The LLM sees plausible-looking data instead of `[REDACTED]`, preserving conversational context.

| PII Type | Radix | What Gets Encrypted | What's Preserved | Example |
|----------|-------|---------------------|------------------|---------|
| Credit Card | 10 | 15-16 digits | Dash positions | `4111-1111-1111-1111` → `8714-3927-6051-2483` |
| SSN | 10 | 9 digits | Dash positions | `123-45-6789` → `847-29-3651` |
| Phone | 10 | 10+ digits | `+`, parens, spaces, dashes | `+1 (555) 123-4567` → `+1 (847) 293-6510` |
| Email | 36 | Local part (lowercase) | `@` + domain | `john.doe@gmail.com` → `q7k2m91@gmail.com` |
| API Key | 62 | Post-prefix body | Known prefix (`sk-`, `AKIA`...) | `sk-abc123def456` → `sk-x9q2w7m4k8p1` |

**Tweak strategy:** Per-record tweak = `request_uuid (16B) || SHA-256(json_path)[0..16]`. Same PII value in different requests produces different ciphertexts (prevents frequency analysis).

**Domain size safety:** FF1 requires radix^len ≥ 1,000,000. Values shorter than the minimum length for their radix are rejected (logged, forwarded unencrypted).

## Authentication & Key Management

### Passthrough-First Design

OpenObscure reuses the host agent's API keys by default — **no duplicate key management**. The proxy forwards all auth headers from the host agent to upstream providers untouched:

```
Host Agent (has API keys)           OpenObscure Proxy              LLM Provider
       │                                │                            │
       │  Authorization: Bearer sk-...  │  Authorization: Bearer sk-...  │
       │ ──────────────────────────────►│ ──────────────────────────────►│
       │  x-api-key: sk-ant-...        │  x-api-key: sk-ant-...        │
       │ ──────────────────────────────►│ ──────────────────────────────►│
```

All original request headers are forwarded except:
- **Hop-by-hop headers** (RFC 7230): `Connection`, `Transfer-Encoding`, `Host`, etc.
- **Provider-specific strip_headers**: configured per provider in TOML (e.g., `x-openobscure-internal`)

### Optional Auth Override

For advanced setups where OpenObscure needs **different** API keys than the host agent (e.g., separate billing account for privacy-proxied traffic), set `override_auth = true` per provider:

```toml
[providers.anthropic]
upstream_url = "https://api.anthropic.com"
route_prefix = "/anthropic"
override_auth = true              # Inject key from OpenObscure vault
vault_key_name = "anthropic"      # Vault entry name (defaults to provider name)
auth_header_name = "x-api-key"    # Anthropic uses x-api-key, not Authorization
```

When `override_auth = true`:
1. Vault key is looked up by `vault_key_name` (or provider name)
2. The `auth_header_name` header is injected/replaced in the upstream request
3. For `authorization` headers, the value is automatically prefixed with `Bearer `
4. If the vault key is missing, falls back to passthrough with a warning

### FPE Key Management

FPE master key resolution (priority order):

1. **`OPENOBSCURE_MASTER_KEY` env var** (64 hex chars → 32 bytes) — for Docker, VPS, CI, headless environments
2. **OS keychain** via `keyring` crate — for desktop environments (macOS Keychain, Linux keyutils, Windows Credential Store)
3. **Fail with error** listing both options

- Generated once with `--init-key` using `OsRng` (cryptographically secure)
- When `OPENOBSCURE_HEADLESS=1` is set, `--init-key` also prints the key as hex to stdout for capture
- Override API keys (optional): stored in keychain under `api-key-{provider}`

### Health Endpoint Auth Token

L0 generates a shared auth token for the health endpoint:

1. **`OPENOBSCURE_AUTH_TOKEN` env var** — explicit token for Docker/CI
2. **`~/.openobscure/.auth-token` file** — auto-generated on first run (0600 perms on Unix)
3. **Auto-generate** — random 32-byte hex written to file

L1 reads the token from `~/.openobscure/.auth-token` and sends it as `X-OpenObscure-Token` header. Health endpoint returns 401 without a valid token. Proxy routes are NOT auth-gated — only the health endpoint.

## Content-Type Handling

The proxy only processes **JSON** request bodies:

| Content-Type | Action |
|-------------|--------|
| `application/json` | Process images (Pass 1) + scan text for PII (Pass 2) |
| `*/*+json` (e.g., `application/vnd.api+json`) | Process images + scan text for PII |
| Missing (no Content-Type header) | Process optimistically (common in API calls) |
| `text/plain`, `multipart/*`, binary, etc. | Pass through without scanning |

Non-JSON bodies are forwarded to upstream unchanged. Base64-encoded images within JSON bodies are detected and processed (face blur, OCR text blur, EXIF strip) before text PII scanning.

## PII Statistics Logging

Each request logs per-type PII match counts **without logging PII values**:

```
INFO request_id=550e8400-... pii_total=3 pii_breakdown="ssn=1, email=1, phone=1" "PII encrypted in request"
```

This enables monitoring PII volume without creating a new privacy risk in logs.

## Error Handling & Fail Mode

Configurable via `fail_mode` in `openobscure.toml`:

### Fail-Open (default)
- FPE encryption error on a single match → log, skip that match, forward original value
- Entire body processing error → log, forward original body unmodified
- The proxy must never block AI functionality due to FPE bugs or edge cases

### Fail-Closed
- Body processing error → reject with **502 Bad Gateway**, do not forward to upstream
- Use when privacy guarantees are more important than availability

### Always blocking (regardless of fail mode)
- Vault unavailable (keychain locked) → **503 Service Unavailable** (no privacy guarantees without the FPE key)
- Upstream unreachable → 502 Bad Gateway
- Body exceeds `max_body_bytes` → **413 Payload Too Large**

L1 (Gateway Plugin) provides a second line of defense for tool results.

## Provider Routing

Configured via TOML. Each provider maps a route prefix to an upstream URL:

```
Request:  POST http://127.0.0.1:18790/anthropic/v1/messages
          ├── Match: /anthropic → providers.anthropic
          ├── Strip prefix: /v1/messages
          └── Forward: POST https://api.anthropic.com/v1/messages
```

Longest prefix match wins when multiple providers overlap.

## Resource Budget

| Metric | Target | Actual |
|--------|--------|--------|
| RAM (regex-only) | ~12MB | TBD (runtime profiling) |
| RAM (with NER) | ~67MB | TBD (runtime profiling) |
| RAM (peak, image processing) | ~224MB | Sequential model loading (face → OCR) |
| Binary size | <8MB | **2.7MB** (release, stripped, LTO) |
| Dependencies | Minimal | ~35 direct + 1 dev (wiremock) |
| Latency overhead | <5ms (regex), <15ms (NER), <80ms (image) | TBD |
| Test count | — | **264** (252 unit + 12 integration) |

## Technology Stack

| Component | Choice | Why |
|-----------|--------|-----|
| HTTP framework | axum 0.8 | Ergonomic, tower middleware, low overhead |
| Async runtime | tokio | Industry standard for async Rust |
| HTTP client | hyper 1 + hyper-util | Direct control over body transformation |
| TLS | rustls + hyper-rustls | Pure Rust, no OpenSSL dependency at link time |
| FPE | fpe 0.6 (FF1) | NIST-approved, pure Rust, RustCrypto AES |
| NER inference | ort 2.0 (ONNX Runtime) | TinyBERT INT8 + BlazeFace + PaddleOCR, cross-platform |
| Image processing | image 0.25 | Decode/encode/resize/blur, pure Rust, strips EXIF |
| Base64 | base64 0.22 | Image content decode/encode |
| EXIF reading | kamadak-exif 0.5 | Screenshot detection (pre-strip analysis) |
| Regex | regex (RegexSet) | Linear time, multi-pattern in one pass |
| Config | serde + toml | Human-readable, Rust ecosystem standard |
| Keychain | keyring 3 | Cross-platform OS credential storage |
| Hex encoding | hex 0.4 | Env var key decoding (headless deployments) |
| Logging | tracing + tracing-oslog/journald | Structured, async-aware, platform-native |
| Crash buffer | memmap2 0.9 | mmap ring buffer survives SIGKILL/OOM |

## Image Pipeline (Phase 3)

Two-pass processing in `body.rs`: images first (entire base64 string replacement), then text PII (substring FPE).

### ImageModelManager (`image_pipeline.rs`)

```rust
pub struct ImageModelManager {
    face_detector: Mutex<Option<FaceDetector>>,
    ocr_detector: Mutex<Option<OcrDetector>>,
    ocr_recognizer: Mutex<Option<OcrRecognizer>>,
    last_use: Mutex<Instant>,
    config: ImageConfig,
}
```

**Memory rule:** Models loaded sequentially via `Mutex<Option<_>>`. Face model loaded/used/dropped before OCR model loaded. Never both in RAM simultaneously. Background eviction task (60s interval) evicts models idle beyond `model_idle_timeout_secs` (default 300).

### Face Detection (`face_detector.rs`)

BlazeFace short-range ONNX model for selfie-distance face detection.

| Property | Value |
|----------|-------|
| Model | BlazeFace short-range (~230KB INT8) |
| Input | `[1, 3, 128, 128]` float32, RGB normalized to [-1, 1] |
| Output | Bounding boxes + confidence scores |
| Post-processing | Sigmoid activation, anchor-relative decoding, NMS (IoU 0.3) |
| Anchors | 1664 generated (strides 8/16/16/16, 2/6/6/6 per stride) |

### OCR Engine (`ocr_engine.rs`)

PaddleOCR-Lite text detection and recognition via ONNX Runtime.

| Component | Model | Input | Output |
|-----------|-------|-------|--------|
| Detector | det_model.onnx (~1.1MB) | `[1, 3, H, W]` BGR, ImageNet norm | Probability map → binary mask → connected components → text regions |
| Recognizer | rec_model.onnx (~4.5MB) | `[B, 3, 48, W]` cropped regions | Logits → CTC greedy decode with dictionary |

**Two tiers:**
- **DetectAndBlur (Tier 1, default):** Detect text regions → blur all. No recognition model needed.
- **FullRecognition (Tier 2+):** Detect → recognize → scan text for PII → selectively blur PII regions.

### Screenshot Detection (`screen_guard.rs`)

| Heuristic | Method | Weight |
|-----------|--------|--------|
| EXIF software | Check Software/UserComment tags for 18 screenshot tool names | Definitive (single match = screenshot) |
| No camera hardware | EXIF has Software but no Make/Model | Supporting |
| Screen resolution | Match against 21 common resolutions (desktop + mobile, 1x + 2x) | Supporting |
| Status bar | Color variance in top 5% strip < 50 (uniform = status bar) | Supporting |

Explicit EXIF software match → screenshot. Otherwise need >= 2 supporting heuristics.

## CLI Subcommands (Phase 4)

The proxy binary supports both server mode and compliance CLI:

```
openobscure                          # Default: starts proxy (backward compatible)
openobscure serve                    # Explicit: starts proxy
openobscure compliance summary       # Aggregate stats from audit log
openobscure compliance ropa          # GDPR Art. 30 ROPA (--format markdown|json)
openobscure compliance dpia          # GDPR Art. 35 DPIA (--format markdown|json)
openobscure compliance audit-log     # Query/filter audit log (--since, --until, --limit)
openobscure compliance breach-check  # Anomaly detection + Art. 33 draft
openobscure compliance export        # SIEM export (--format cef|leef)
```

Backward compatibility: `openobscure` with no subcommand still starts the proxy server via clap's `Option<Commands>` pattern.

## Future Work

- **FPE key rotation:** Vault key versioning, re-encryption of active mappings
- **SSE streaming:** Process `text/event-stream` responses event-by-event instead of buffering
- **SCRFD upgrade:** SCRFD-2.5GF for multi-scale face detection on screenshots with mixed-size faces
- **Production benchmarks:** p50/p95/p99 latency profiling under realistic load
- **Real-time breach monitoring:** Rolling window anomaly detection in proxy (batch CLI sufficient for v1)
