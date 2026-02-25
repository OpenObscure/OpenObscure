# OpenObscure Gateway Test Guide

> Hands-on walkthrough of every Gateway (L0 proxy) feature.
> Each section includes runnable commands you can copy-paste.

---

## Prerequisites

- Rust toolchain (1.75+)
- Built proxy binary (`cargo build --release`)
- OS keychain access (macOS Keychain, Linux secret-service) — or use `OPENOBSCURE_MASTER_KEY` env var

## Quick Start

```bash
# Build the proxy
cargo build --release --manifest-path openobscure-proxy/Cargo.toml

# First run: generate FPE key in OS keychain
./target/release/openobscure-proxy --init-key

# Start the proxy (default: 127.0.0.1:18790)
./target/release/openobscure-proxy serve
```

**Headless / Docker alternative** (no keychain required):

```bash
export OPENOBSCURE_MASTER_KEY=$(openssl rand -hex 32)
./target/release/openobscure-proxy serve
```

The proxy reads `config/openobscure.toml` by default. Override with `--config /path/to/file.toml` or `OPENOBSCURE_CONFIG`.

---

## 1. Health Check

```bash
curl -s http://127.0.0.1:18790/_openobscure/health | jq .
```

**Expected response:**

```json
{
  "status": "ok",
  "version": "0.6.0",
  "uptime_secs": 42,
  "pii_matches_total": 0,
  "requests_total": 0,
  "images_processed_total": 0,
  "faces_redacted_total": 0,
  "text_regions_total": 0,
  "scan_latency_p50_us": 0,
  "scan_latency_p95_us": 0,
  "scan_latency_p99_us": 0,
  "request_latency_p50_us": 0,
  "request_latency_p95_us": 0,
  "request_latency_p99_us": 0,
  "device_tier": "full",
  "feature_budget": {
    "tier": "full",
    "max_ram_mb": 275,
    "ner_enabled": true,
    "crf_enabled": true,
    "ensemble_enabled": true,
    "image_pipeline_enabled": true
  }
}
```

**With auth token** (auto-generated on first run at `~/.openobscure/.auth-token`):

```bash
TOKEN=$(cat ~/.openobscure/.auth-token)
curl -s -H "X-OpenObscure-Token: $TOKEN" \
  http://127.0.0.1:18790/_openobscure/health | jq .
```

> **Note:** If no auth token is configured, the health endpoint is open. When a token exists, requests without a valid `X-OpenObscure-Token` header receive `401 Unauthorized`.

---

## 2. Text PII Redaction

All requests are sent through the proxy to a configured provider. The proxy scans JSON message content, encrypts PII with FF1 Format-Preserving Encryption, and forwards the sanitized request upstream.

### Credit Card (Luhn-validated)

```bash
curl -s -X POST http://127.0.0.1:18790/anthropic/v1/messages \
  -H "Content-Type: application/json" \
  -H "x-api-key: YOUR_KEY" \
  -H "anthropic-version: 2023-06-01" \
  -d '{
    "model": "claude-sonnet-4-20250514",
    "max_tokens": 256,
    "messages": [{
      "role": "user",
      "content": "My card is 4111-1111-1111-1111"
    }]
  }'
```

**What upstream sees:** `4111-1111-1111-1111` replaced with an FPE-encrypted value like `4732-8294-5617-3048` (same format, different digits).

**What you receive back:** The original `4111-1111-1111-1111` restored in the response via cached mapping.

### SSN (range-validated, rejects 000/666/900+)

```bash
curl -s -X POST http://127.0.0.1:18790/anthropic/v1/messages \
  -H "Content-Type: application/json" \
  -H "x-api-key: YOUR_KEY" \
  -H "anthropic-version: 2023-06-01" \
  -d '{
    "model": "claude-sonnet-4-20250514",
    "max_tokens": 256,
    "messages": [{
      "role": "user",
      "content": "SSN: 123-45-6789"
    }]
  }'
```

### Phone Number

```bash
curl -s -X POST http://127.0.0.1:18790/anthropic/v1/messages \
  -H "Content-Type: application/json" \
  -H "x-api-key: YOUR_KEY" \
  -H "anthropic-version: 2023-06-01" \
  -d '{
    "model": "claude-sonnet-4-20250514",
    "max_tokens": 256,
    "messages": [{
      "role": "user",
      "content": "Call me at +1-555-867-5309"
    }]
  }'
```

### Email (FPE on local part, domain preserved)

```bash
curl -s -X POST http://127.0.0.1:18790/anthropic/v1/messages \
  -H "Content-Type: application/json" \
  -H "x-api-key: YOUR_KEY" \
  -H "anthropic-version: 2023-06-01" \
  -d '{
    "model": "claude-sonnet-4-20250514",
    "max_tokens": 256,
    "messages": [{
      "role": "user",
      "content": "Email: jane.doe@example.com"
    }]
  }'
```

Upstream sees something like `xkrp.bwq@example.com` — the domain stays intact.

### API Key

```bash
curl -s -X POST http://127.0.0.1:18790/anthropic/v1/messages \
  -H "Content-Type: application/json" \
  -H "x-api-key: YOUR_KEY" \
  -H "anthropic-version: 2023-06-01" \
  -d '{
    "model": "claude-sonnet-4-20250514",
    "max_tokens": 256,
    "messages": [{
      "role": "user",
      "content": "My key is sk-ant-api03-abcdefghij1234567890"
    }]
  }'
```

### Other PII Types

```bash
# IPv4 Address
"content": "Server at 192.168.1.42"
# → redacted to [IPv4]

# IPv6 Address
"content": "Address: 2001:db8::1"
# → redacted to [IPv6]

# GPS Coordinates
"content": "Location: 45.5231, -122.6765"
# → redacted to [GPS]

# MAC Address
"content": "Device MAC: 00:1A:2B:3C:4D:5E"
# → redacted to [MAC]
```

### Multiple PII in One Message

```bash
curl -s -X POST http://127.0.0.1:18790/anthropic/v1/messages \
  -H "Content-Type: application/json" \
  -H "x-api-key: YOUR_KEY" \
  -H "anthropic-version: 2023-06-01" \
  -d '{
    "model": "claude-sonnet-4-20250514",
    "max_tokens": 256,
    "messages": [{
      "role": "user",
      "content": "Card 4111-1111-1111-1111, SSN 123-45-6789, email jane@example.com"
    }]
  }'
```

All three PII values are independently encrypted/redacted. The `skip_fields` config (`model`, `stream`, `temperature`, `max_tokens`, `top_p`, `top_k`) are never scanned.

---

## 3. Keyword Detection (Health / Child)

Health and child-related keywords are detected and label-redacted (not FPE-encrypted).

```bash
curl -s -X POST http://127.0.0.1:18790/anthropic/v1/messages \
  -H "Content-Type: application/json" \
  -H "x-api-key: YOUR_KEY" \
  -H "anthropic-version: 2023-06-01" \
  -d '{
    "model": "claude-sonnet-4-20250514",
    "max_tokens": 256,
    "messages": [{
      "role": "user",
      "content": "The patient has diabetes and takes metformin. The child is 3 years old."
    }]
  }'
```

Upstream sees keywords replaced with `[health_keyword]` and `[child_keyword]` labels.

---

## 4. NER Detection (Person, Location, Organization)

Named Entity Recognition identifies semantic PII that regex cannot catch. The proxy supports three scanner modes:

```toml
# config/openobscure.toml
[scanner]
scanner_mode = "auto"   # "auto" | "ner" | "crf" | "regex"
```

- **auto** (default): hardware profiler detects device tier — Full (8GB+) enables NER + ensemble, Standard (4–8GB) enables NER, Lite (<4GB) uses CRF/regex
- **ner**: Force TinyBERT INT8 NER model
- **crf**: Force CRF (lighter-weight)
- **regex**: Regex + keywords only

```bash
curl -s -X POST http://127.0.0.1:18790/anthropic/v1/messages \
  -H "Content-Type: application/json" \
  -H "x-api-key: YOUR_KEY" \
  -H "anthropic-version: 2023-06-01" \
  -d '{
    "model": "claude-sonnet-4-20250514",
    "max_tokens": 256,
    "messages": [{
      "role": "user",
      "content": "John Smith from Google visited Berlin last week."
    }]
  }'
```

Upstream sees: `[PERSON_0] from [ORG_0] visited [LOCATION_0] last week.`

When multiple scanners agree on a match, the ensemble confidence voting system applies an agreement bonus (default +0.15), increasing detection reliability.

---

## 5. Image Pipeline

The proxy detects base64-encoded images in JSON payloads and processes them through a multi-stage pipeline.

```bash
# Encode a test photo
IMG_B64=$(base64 -i test_photo.jpg)

curl -s -X POST http://127.0.0.1:18790/anthropic/v1/messages \
  -H "Content-Type: application/json" \
  -H "x-api-key: YOUR_KEY" \
  -H "anthropic-version: 2023-06-01" \
  -d "{
    \"model\": \"claude-sonnet-4-20250514\",
    \"max_tokens\": 256,
    \"messages\": [{
      \"role\": \"user\",
      \"content\": [
        {
          \"type\": \"image\",
          \"source\": {
            \"type\": \"base64\",
            \"media_type\": \"image/jpeg\",
            \"data\": \"$IMG_B64\"
          }
        },
        {\"type\": \"text\", \"text\": \"Describe this photo\"}
      ]
    }]
  }"
```

**Pipeline stages:**

1. Detect base64 image in JSON
2. Decode to DynamicImage (EXIF metadata auto-stripped)
3. Resize if exceeds `max_dimension` (default: 960px)
4. Face redaction — SCRFD-2.5GF (Full/Standard) or BlazeFace (Lite) detects faces, applies solid-color fill
5. OCR redaction — PaddleOCR ONNX detects text regions, applies solid-color fill
6. NSFW check — NudeNet ONNX confidence score
7. Screenshot detection — heuristics (solid color bars, pixel patterns)
8. Re-encode to base64, substitute back into JSON

**Image config** (`config/openobscure.toml`):

```toml
[image]
enabled = true
face_detection = true
ocr_enabled = true
ocr_tier = "detect_and_fill"
max_dimension = 960
model_idle_timeout_secs = 300
screen_guard = true
exif_strip = true
nsfw_detection = true
nsfw_threshold = 0.45
```

Health endpoint tracks: `images_processed_total`, `faces_redacted_total`, `text_regions_total`.

---

## 6. SSE Streaming

When `stream: true` is set, the proxy detects `Content-Type: text/event-stream` in the upstream response and switches to per-chunk decryption.

```bash
curl -s -N -X POST http://127.0.0.1:18790/anthropic/v1/messages \
  -H "Content-Type: application/json" \
  -H "x-api-key: YOUR_KEY" \
  -H "anthropic-version: 2023-06-01" \
  -d '{
    "model": "claude-sonnet-4-20250514",
    "max_tokens": 256,
    "stream": true,
    "messages": [{
      "role": "user",
      "content": "My SSN is 123-45-6789. Tell me a joke."
    }]
  }'
```

Each `data:` SSE chunk is decrypted independently using the mapping from the request scan. The `-N` flag disables curl buffering so you see events in real time.

---

## 7. Auth Token Passthrough

The proxy uses a **passthrough-first** auth model by default — the host agent's auth headers pass through unchanged.

```toml
# config/openobscure.toml
[providers.anthropic]
upstream_url = "https://api.anthropic.com"
route_prefix = "/anthropic"
override_auth = false   # Host agent's x-api-key passes through unchanged
```

```bash
# Your API key goes directly to Anthropic
curl -X POST http://127.0.0.1:18790/anthropic/v1/messages \
  -H "x-api-key: sk-ant-your-actual-key" \
  -H "anthropic-version: 2023-06-01" \
  -H "Content-Type: application/json" \
  -d '{"model": "claude-sonnet-4-20250514", "max_tokens": 256, "messages": [{"role": "user", "content": "Hello"}]}'
```

---

## 8. Environment Variables & Configuration

### Environment Variables

| Variable | Purpose |
|----------|---------|
| `OPENOBSCURE_CONFIG` | Path to TOML config file (default: `config/openobscure.toml`) |
| `OPENOBSCURE_PORT` | Override listen port (default: 18790) |
| `OPENOBSCURE_LOG` | Override log level (`trace`, `debug`, `info`, `warn`, `error`) |
| `OPENOBSCURE_AUTH_TOKEN` | Health endpoint auth token |
| `OPENOBSCURE_MASTER_KEY` | FPE key as 64 hex chars (headless/Docker environments) |
| `OPENOBSCURE_HEADLESS` | Set to `1` to print generated key to stdout on `--init-key` |

### Minimal Config

```toml
[proxy]
port = 18790

[providers.anthropic]
upstream_url = "https://api.anthropic.com"
route_prefix = "/anthropic"
```

### Full Config Reference

See [config/openobscure.toml](../openobscure-proxy/config/openobscure.toml) for all options with comments.

---

## 9. Response Integrity (Cognitive Firewall)

Response integrity scans LLM responses for persuasion and manipulation techniques. R1 uses dictionary matching; R2 uses a TinyBERT ONNX classifier (optional).

### R1 Dictionary Detection

R1 runs on every response when `[response_integrity]` is enabled. Configure the echo server to return manipulative text and verify detection:

```bash
# Echo server returns persuasive text, proxy detects it
curl -s http://localhost:18790/anthropic/v1/messages \
  -H "Content-Type: application/json" \
  -H "X-Api-Key: test" \
  -d '{"model":"test","messages":[{"role":"user","content":"test"}]}'
```

Check proxy logs for detection output: category names, match count, severity tier (Notice/Warning/Caution).

### R2 Model Detection (Optional)

R2 requires the ONNX model files. Configure:

```toml
[response_integrity]
enabled = true
sensitivity = "high"        # Force R2 on every response
log_only = true
ri_model_dir = "models/r2_persuasion_tinybert"
ri_threshold = 0.55
ri_sample_rate = 0.10
```

With `sensitivity = "high"`, R2 runs on every response. Check logs for `r2_role` (Confirm/Suppress/Upgrade/Discover) and `r2_categories` fields.

### R2 Graceful Degradation

When `ri_model_dir` is not set or the model directory doesn't exist, the proxy falls back to R1-only. No error — this is the expected default behavior.

---

## Feature Parity

With hardware capability detection (Phase 9), NER and ensemble voting are now available on capable mobile devices (8GB+ RAM). The following features remain **Gateway-only**:

| Feature | Why Gateway-only |
|---------|-----------------|
| SSE streaming | HTTP proxy feature |
| Auth token passthrough | HTTP proxy feature |
| Response integrity (R1+R2 cognitive firewall) | Response-path scanning is a proxy feature |

The following features are **tier-dependent** (available on both Gateway and Embedded if the device has sufficient RAM):

| Feature | Required Tier | RAM Needed |
|---------|--------------|------------|
| NER — TinyBERT INT8 | Standard+ (4GB+) | ~55MB |
| Ensemble confidence voting | Full (8GB+) | NER + CRF + agreement bonus |
| Image pipeline | All tiers | ~8–35MB on-demand |

See [EMBEDDED_TEST.md](EMBEDDED_TEST.md) for mobile-specific features (UniFFI bindings, image sanitization, auto-detection).
