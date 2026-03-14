# Configuration Reference

Complete reference for every key in `config/openobscure.toml`. All keys are optional — defaults are applied when omitted.

> **For contextual guidance, see:**
> - [FPE Configuration](fpe-configuration.md) — encryption behavior, key management, fail modes
> - [Detection Engine Configuration](detection-engine-configuration.md) — scanner engines, tier mapping, ensemble voting
> - [Deployment Tiers](../get-started/deployment-tiers.md) — hardware auto-detection and tier overrides

---

**Contents**

- [`[proxy]`](#proxy)
- [`[fpe]`](#fpe)
- [`[scanner]`](#scanner)
- [`[image]`](#image)
- [`[response_integrity]`](#response_integrity)
- [`[voice]`](#voice)
- [`[logging]`](#logging)
- [`[providers.<name>]`](#providersname)
- [Config Validation Failures](#config-validation-failures)
- [Environment Variables](#environment-variables)

## `[proxy]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `listen_addr` | string | `"127.0.0.1"` | Bind address |
| `port` | int | `18790` | Listen port |
| `request_timeout_secs` | int | `120` | Upstream request timeout (seconds) |
| `max_body_bytes` | int | `16777216` (16MB) | Maximum request body size |
| `fail_mode` | `"open"` \| `"closed"` | `"open"` | Behavior on processing errors. `open`: forward original body. `closed`: reject with 502. See [FPE Configuration](fpe-configuration.md#fail-open-vs-fail-closed). |
| `body_limit_lite` | int | `10485760` (10MB) | Body size limit for Lite tier. Requests exceeding this limit are rejected with **413 Payload Too Large** before any processing occurs. |
| `body_limit_standard` | int | `52428800` (50MB) | Body size limit for Standard tier. Requests exceeding this limit are rejected with **413 Payload Too Large**. |
| `body_limit_full` | int | `104857600` (100MB) | Body size limit for Full tier. Requests exceeding this limit are rejected with **413 Payload Too Large**. |
| `image_budget_fraction` | float | `0.5` | Fraction of tier body limit reserved for base64 image content |
| `enable_prewarm` | bool | `true` | Pre-warm NER model on startup |
| `sse_buffer_size` | int | `512` | SSE frame accumulation buffer (bytes) |
| `sse_flush_timeout_ms` | int | `200` | How long to wait for the next SSE frame before emitting an empty keepalive chunk (milliseconds). **Does not flush the content buffer.** When this timeout fires between frames, the proxy yields an empty HTTP chunk to keep the connection alive and resumes waiting — any partial PII span or FPE ciphertext held in the accumulator buffer is **retained, not forwarded and not dropped** (`proxy.rs:567–575`). The buffer is flushed (with full FPE decryption applied) only at stream termination: `[DONE]` event, clean stream end, or stream error. Increase if the LLM generates long pauses between tokens and clients are dropping connections; decrease for more frequent keepalives. |

---

## `[fpe]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `enabled` | bool | `true` | Master switch for FPE encryption |
| `keychain_service` | string | `"openobscure"` | OS keychain service name |
| `keychain_user` | string | `"fpe-master-key"` | OS keychain account name |

### `[fpe.type_overrides]`

Per-PII-type enable/disable. All default to `true` when `fpe.enabled = true`.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `credit_card` | bool | `true` | FPE for credit card numbers |
| `ssn` | bool | `true` | FPE for Social Security Numbers |
| `phone` | bool | `true` | FPE for phone numbers |
| `email` | bool | `true` | FPE for email addresses (local part) |
| `api_key` | bool | `true` | FPE for API keys |
| `ipv4_address` | bool | `true` | FPE for IPv4 addresses |
| `ipv6_address` | bool | `true` | FPE for IPv6 addresses |
| `gps_coordinate` | bool | `true` | FPE for GPS coordinates |
| `mac_address` | bool | `true` | FPE for MAC addresses |
| `iban` | bool | `true` | FPE for IBANs |

See [FPE Configuration](fpe-configuration.md) for per-type radix, alphabet, and encryption behavior.

---

## `[scanner]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `enabled` | bool | `true` | Master switch for all PII scanning |
| `scanner_mode` | `"auto"` \| `"ner"` \| `"crf"` \| `"regex"` | `"auto"` | Semantic backend selection. See [Detection Engine Configuration](detection-engine-configuration.md#forcing-a-specific-engine). |
| `skip_fields` | string[] | `["model", "stream", "temperature", "max_tokens", "top_p", "top_k"]` | JSON fields to skip during scanning |
| `keywords_enabled` | bool | `true` | Enable health/child keyword dictionary |
| `gazetteer_enabled` | bool | `true` | Enable name gazetteer for person detection |
| `ner_enabled` | bool | `true` | Enable NER scanner (device budget may override) |
| `ner_model` | string? | (auto) | Force NER variant: `"tinybert"` or `"distilbert"` |
| `ner_model_dir` | string? | `"models/ner"` | Path to DistilBERT / default NER model directory |
| `ner_model_dir_lite` | string? | `"models/ner-lite"` | Path to TinyBERT model directory; falls back to `ner_model_dir` |
| `ner_pool_size` | int | `4` | Concurrent NER sessions (~14MB each for TinyBERT, ~64MB for DistilBERT) |
| `ner_confidence_threshold` | float | `0.60` | Per-token NER confidence cutoff |
| `crf_model_dir` | string? | (none) | Path to CRF model directory (containing `crf_model.json`) |
| `ram_threshold_mb` | int | `200` | **Deprecated — has no runtime effect.** Accepted and deserialized for backward compatibility with existing config files but is never read by any dispatch or fallback logic. The NER→CRF selection it originally controlled was replaced by the `CapabilityTier` / `FeatureBudget` system (see [`device_profile.rs`](../../openobscure-core/src/device_profile.rs)). Setting this to any value has no observable effect. Safe to remove from new configs. |
| `respect_code_fences` | bool | `true` | Skip scanning inside markdown code fences and inline code |
| `min_confidence` | float | `0.5` | Ensemble voting minimum confidence threshold |
| `agreement_bonus` | float | `0.15` | Confidence bonus when 2+ scanners agree on overlapping span |
| `enabled_languages` | string[] | `[]` | ISO 639-1 language codes for the multilingual scan pass. Empty list (default) activates all 8 supported non-English languages (`es`, `fr`, `de`, `pt`, `ja`, `zh`, `ko`, `ar`). See [Multilingual Scanner Configuration](detection-engine-configuration.md#multilingual-scanner-configuration). |

### `[scanner.custom_patterns.<name>]`

Add custom regex patterns for application-specific PII types.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `regex` | string | (required) | Regular expression pattern |
| `radix` | int | (required) | FPE radix (10, 16, 36, or 62) |
| `alphabet` | string? | (auto from radix) | Character alphabet for FPE encryption |

Example:

```toml
[scanner.custom_patterns.employee_id]
regex = "EMP-[A-Z]{4}-[0-9]{8}"
radix = 36
alphabet = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ"
```

See [Detection Engine Configuration](detection-engine-configuration.md) for engine details and tier mapping.

---

## `[image]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `enabled` | bool | `true` | Master switch for image pipeline |
| `face_detection` | bool | `true` | Enable face detection and redaction |
| `face_model` | `"scrfd"` \| `"blazeface"` \| `"ultralight"` | `"blazeface"` | Face detection model (tier auto-selects; this is the config default, overridden by device budget) |
| `face_model_dir` | string? | `"models/blazeface"` | BlazeFace model directory |
| `face_model_dir_scrfd` | string? | `"models/scrfd"` | SCRFD model directory |
| `face_model_dir_ultralight` | string? | `"models/ultralight"` | Ultra-Light face model directory |
| `ocr_enabled` | bool | `true` | Enable OCR text detection |
| `ocr_tier` | `"detect_and_fill"` \| `"full_recognition"` | `"detect_and_fill"` | OCR mode (tier auto-selects; config default overridden by device budget) |
| `ocr_model_dir` | string? | `"models/paddleocr"` | PaddleOCR model directory |
| `nsfw_detection` | bool | `true` | Enable NSFW classifier (Phase 0 of image pipeline) |
| `nsfw_model_dir` | string? | `"models/nsfw_classifier"` | NSFW ViT-base classifier model directory |
| `nsfw_threshold` | float | `0.50` | NSFW confidence threshold on P(hentai) + P(porn) + P(sexy) |
| `screen_guard` | bool | `true` | Enable screenshot detection heuristics |
| `exif_strip` | bool | `true` | Strip EXIF metadata from images |
| `max_dimension` | int | `960` | Resize longest edge before processing (pixels) |
| `model_idle_timeout_secs` | int | `300` | Evict idle face/OCR models after N seconds |
| `url_fetch_enabled` | bool | `true` | Fetch and process URL-referenced images |
| `url_max_bytes` | int | `10485760` (10MB) | Maximum download size for URL images |
| `url_timeout_secs` | int | `10` | URL image fetch timeout (seconds) |
| `url_allow_localhost_http` | bool | `true` | Allow HTTP (non-TLS) for localhost image URLs. **Set to `false` in production** — see security note below. |

### Security Note: `url_allow_localhost_http` and SSRF

**Default is `true` for local testing. Production deployments must set this to `false`.**

When `url_allow_localhost_http = true`, the proxy will fetch images from `http://127.0.0.1:*`, `http://localhost:*`, and `http://[::1]:*`. This creates a **confused-deputy SSRF path**: an attacker who can inject an `image_url` into a request the agent sends through the proxy (for example, via a malicious web-scraping tool result, a manipulated LLM response in an agentic loop, or a prompt injection) can cause the proxy to make HTTP GET requests to any service bound to the local loopback interface.

**What an attacker can achieve:**

| Goal | Mechanism |
|------|-----------|
| Probe running services | Send `http://127.0.0.1:PORT/path` — request succeeds or fails, timing reveals whether a service is listening |
| Trigger side effects | GET requests to services that change state on GET (REST APIs, admin panels, Ollama `api/tags`, etc.) |
| OCR exfiltration | Control the HTTP server; serve a PNG with image magic bytes and embedded text → proxy OCR extracts the text into LLM context |
| Bypass local-trust assumptions | Services that trust loopback (Redis, Elasticsearch, Ollama, dev servers) receive unauthenticated requests from the proxy |

**What mitigates the risk:**

- Non-localhost `http://` URLs are always rejected regardless of this setting
- Private IPv4/IPv6 ranges (10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16, link-local, CGNAT) are rejected even over HTTPS
- The response must pass magic-byte image validation before entering the image pipeline — raw JSON/text responses from internal APIs are rejected with `NotImage`

**Residual risk with `url_allow_localhost_http = false`:** The private-IP check in `image_fetch.rs` only applies to numeric IP literals. Hostnames like `https://internal.corp.local/image.png` skip the private-IP check and are resolved at connection time. In environments with internal DNS, set `url_fetch_enabled = false` entirely, or use a network-level egress control to block the proxy from reaching internal subnets.

**Recommended production config:**

```toml
[image]
url_allow_localhost_http = false   # Never allow HTTP to loopback (default true is for testing only)
url_fetch_enabled = true           # Keep enabled only if your agent legitimately sends URL image references
```

See [Detection Engine Configuration](detection-engine-configuration.md#image-detection-engines) for pipeline phases and tier-gated activation.

---

## `[response_integrity]`

Cognitive firewall — scans LLM responses for persuasion/manipulation techniques.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `enabled` | bool | `true` | Master switch for response integrity scanning |
| `sensitivity` | `"off"` \| `"low"` \| `"medium"` \| `"high"` | `"low"` | Scan intensity. `off`: disabled. `low`: R1 dictionary only, R2 on R1 flags. `medium`: R1 always, R2 on sample + flags. `high`: R1 + R2 on every response. |
| `ri_model_dir` | string? | `"models/ri"` | R2 TinyBERT ONNX model directory |
| `ri_threshold` | float | `0.55` | R2 classification threshold (sigmoid output) |
| `ri_early_exit_threshold` | float | `0.30` | R2 early-exit threshold — skip full inference if first-window max score is below this |
| `ri_idle_evict_secs` | int | `300` | Evict idle R2 model after N seconds |
| `ri_sample_rate` | float | `0.10` | Fraction of responses scanned by R2 when R1 did not flag (medium sensitivity only) |
| `ri_min_flags` | int | `3` | Minimum R1 phrase matches before injecting a warning label |

---

## `[voice]`

Voice PII detection via keyword spotting. Requires the `voice` Cargo feature.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `enabled` | bool | `true` | Enable voice PII detection pipeline |
| `kws_model_dir` | string | `"models/kws"` | KWS Zipformer ONNX model directory |
| `kws_keywords_file` | string | `"models/kws/keywords.txt"` | Tokenized PII keywords file |
| `kws_threshold` | float | `0.1` | Detection threshold (0–1). Lower = more sensitive. |
| `kws_score` | float | `3.0` | Keyword boosting score. Higher = easier to trigger. |

---

## `[logging]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `level` | string | `"info"` | Log level: `trace`, `debug`, `info`, `warn`, `error` |
| `json_output` | bool | `false` | Emit structured JSON logs (vs human-readable) |
| `file_path` | string? | (none) | Log file path. If unset, logs to stderr only. |
| `max_file_size` | int | `10485760` (10MB) | Max log file size before rotation (bytes) |
| `max_files` | int | `3` | Number of rotated log files to keep |
| `audit_log_path` | string? | (none) | GDPR audit log path (append-only JSONL) |
| `pii_scrub` | bool | `true` | Scrub PII from log output (defense-in-depth) |
| `crash_buffer` | bool | `false` | Enable mmap crash buffer for post-mortem debugging |
| `crash_buffer_size` | int | `2097152` (2MB) | Crash buffer size (bytes) |

---

## `[providers.<name>]`

Define LLM provider routing. Each provider maps a local path prefix to an upstream URL.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `upstream_url` | string | (required) | Upstream provider base URL |
| `route_prefix` | string | (required) | Local path prefix (e.g., `"/openai"`) |
| `strip_headers` | string[] | `[]` | Headers to remove before forwarding to upstream |

Example:

```toml
[providers.openai]
upstream_url = "https://api.openai.com"
route_prefix = "/openai"
strip_headers = ["x-openobscure-internal"]
```

See [Integration Reference](../integrate/integration-reference.md) for all built-in providers and custom provider setup.

---

## Config Validation Failures

`AppConfig::validate()` runs after loading the TOML file and returns an error for the following conditions. The proxy will not start if validation fails.

| Condition | Error message |
|-----------|--------------|
| `proxy.port = 0` | `"Proxy port must be non-zero"` |
| `proxy.max_body_bytes = 0` | `"Max body bytes must be non-zero"` |
| `scanner.ner_pool_size < 1` (when NER enabled) | `"scanner.ner_pool_size must be >= 1 when NER is enabled"` |
| `scanner.ner_pool_size > 32` | `"scanner.ner_pool_size must be <= 32 (each session uses ~14–64MB RAM)"` |
| Provider `upstream_url` is empty | `"Provider '<name>' has empty upstream_url"` |
| Provider `route_prefix` is empty | `"Provider '<name>' has empty route_prefix"` |
| Provider `route_prefix` does not start with `/` | `"Provider '<name>' route_prefix must start with '/'"` |

---

## Environment Variables

These environment variables override or supplement TOML configuration.

| Variable | Description |
|----------|-------------|
| `OPENOBSCURE_MASTER_KEY` | FPE master key (64 hex chars). Step 1 of key resolution — takes priority over all file and keychain sources. |
| `OPENOBSCURE_KEY_FILE` | Path to a file containing the 64-hex-char FPE key. Step 3 of key resolution — used for non-standard mount points. |
| `OPENOBSCURE_AUTH_TOKEN` | Health endpoint auth token (hex). Overrides `~/.openobscure/.auth-token`. |
| `OPENOBSCURE_LISTEN_ADDR` | Override `proxy.listen_addr`. Set to `0.0.0.0` in containers (default: `127.0.0.1`). |
| `OPENOBSCURE_PORT` | Override `proxy.port` (default: `18790`). Equivalent to `--port`. |
| `OPENOBSCURE_CONFIG` | Path to TOML config file. Overrides default `config/openobscure.toml`. |
| `OPENOBSCURE_LOG` | Log level: `trace`, `debug`, `info`, `warn`, `error` (default: `info`). |
| `OPENOBSCURE_HEADLESS` | Set to `1` to print FPE key to stdout during `--init-key`. |
| `OPENAI_BASE_URL` | Used by many AI tools (Cursor, Aider, etc.) to route through the proxy. |
| `ANTHROPIC_BASE_URL` | Used by Anthropic-compatible tools to route through the proxy. |

**FPE key resolution order** (first found wins):
1. `OPENOBSCURE_MASTER_KEY` env var
2. `/run/secrets/openobscure-master-key` file (Kubernetes Secrets / Docker Secrets standard path)
3. `OPENOBSCURE_KEY_FILE` env var (custom file path)
4. `~/.openobscure/master-key` file (volume-mounted home directory)
5. OS keychain (native desktop/laptop install)
