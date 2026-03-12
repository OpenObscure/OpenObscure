# Gateway Operations

Operational reference for the OpenObscure Gateway proxy. This page assumes you have already completed the [Gateway Quick Start](gateway-quick-start.md).

---

## Supported Providers

| Route prefix | Provider | Upstream |
|-------------|----------|---------|
| `/openai` | OpenAI | api.openai.com |
| `/anthropic` | Anthropic | api.anthropic.com |
| `/openrouter` | OpenRouter | openrouter.ai |
| `/ollama` | Ollama | localhost:11434 |

Add custom providers in `config/openobscure.toml` under `[providers.<name>]`. For per-provider details, Google Gemini setup, and SSE streaming, see [Integration Reference](../integrate/integration-reference.md).

---

## CLI Subcommands

| Command | Description |
|---------|-------------|
| `openobscure serve` | Start the proxy (default if no subcommand given) |
| `openobscure key-rotate` | Rotate the FPE encryption key (zero-downtime, 30s overlap window) |
| `openobscure passthrough` | Run in passthrough mode (no PII scanning or encryption) |
| `openobscure service install` | Install as a background service (launchd on macOS, systemd on Linux) |
| `openobscure service start` | Start the installed background service |
| `openobscure service stop` | Stop the background service |
| `openobscure service stop --passthrough` | Stop the managed service and immediately start passthrough |
| `openobscure service status` | Check background service status |
| `openobscure service uninstall` | Remove the background service |

---

## Passthrough Mode

```bash
openobscure passthrough
```

Starts a lightweight HTTP relay on the same port that forwards requests directly to upstream providers — **every PII protection feature is disabled:**

- No PII scanning (no regex, no keyword matching, no NER/CRF)
- No FPE encryption or decryption — plaintext forwarded as-is
- No image pipeline (NSFW, face, OCR, EXIF all disabled)
- No response integrity scanning (R1 dictionary, R2 cognitive firewall both disabled)
- No SSE accumulation — raw byte-for-byte stream
- No request journal — crash recovery disabled

The health endpoint returns `"status": "passthrough"` and `"pii_protection": false`. The L1 plugin heartbeat detects this and drops to regex-only redaction automatically.

> **Security warning:** All PII sent through the proxy in passthrough mode reaches the LLM provider in plaintext. Use passthrough only when privacy protection is intentionally not required — initial integration testing before key provisioning, or as a short-term fallback during upgrades. Never use in production environments handling real user data.

**When to use passthrough:**

- **Integration testing** — verify routing before enabling PII protection, without needing an FPE key provisioned
- **Temporary fallback after `service stop`** — keeps the agent working while the service is reloading (`service stop --passthrough` does this atomically)
- **Debugging upstream connectivity** — isolate whether a problem is in OpenObscure or the upstream provider

### Passthrough vs. `scanner_mode = "regex"`

These are often confused but are fundamentally different:

| | `passthrough` subcommand | `scanner_mode = "regex"` |
|---|---|---|
| **PII scanning** | None | Regex + keywords + gazetteer |
| **FPE encryption** | None — plaintext forwarded | Yes — all matched PII is encrypted |
| **FPE decryption** | None — response forwarded as-is | Yes — ciphertext restored in response |
| **Image pipeline** | Disabled | Active (NSFW, face, OCR, EXIF) |
| **Cognitive firewall** | Disabled | Active (R1 + R2) |
| **SSE accumulation** | Disabled — raw stream | Active — frame buffering for span detection |
| **FPE key required** | No | Yes |
| **Use case** | Testing / emergency fallback | Production with fast, no-ML scanning |

`scanner_mode = "regex"` is a full privacy pipeline that skips ML model loading in favour of pattern matching — 15 PII types, full throughput, no ONNX dependency:

```toml
[scanner]
scanner_mode = "regex"   # Options: "auto" (default), "regex", "ner", "crf"
```

`passthrough` is not a scanner mode — it is a separate binary entry point that bypasses the entire request handler.

---

## Model Files

### Download

```bash
./build/download_models.sh lite      # ~11MB  — face detection + OCR
./build/download_models.sh standard  # ~14MB  — adds SCRFD face detection
./build/download_models.sh full      # same as standard; NER/RI/KWS via Git LFS

git lfs pull                         # NER, cognitive firewall (R2), voice KWS
./build/download_kws_models.sh       # download KWS models separately (~5MB)
```

### Model Table by Tier

| Model file | Size | Feature | Tier | Source |
|-----------|------|---------|------|--------|
| `models/blazeface/blazeface.onnx` | 408 KB | Face detection (128×128) | All tiers | ailia-models GCS |
| `models/paddleocr/det_model.onnx` | 2.3 MB | OCR text region detection | All tiers | HuggingFace monkt/paddleocr-onnx |
| `models/paddleocr/rec_model.onnx` | 7.3 MB | OCR text recognition (PP-OCRv4) | All tiers | HuggingFace deepghs/paddleocr |
| `models/paddleocr/ppocr_keys.txt` | ~2 KB | OCR 95-char English dictionary | All tiers | HuggingFace deepghs/paddleocr |
| `models/ner-lite/model_int8.onnx` | 14 MB | NER PII detection (TinyBERT 4L, INT8) | Lite + Standard | Git LFS |
| `models/nsfw_classifier/nsfw_5class_int8.onnx` | 83 MB | NSFW classification (ViT-base, INT8, 5 classes) | All tiers | Git LFS |
| `models/scrfd/scrfd_2.5g.onnx` | 3.1 MB | Face detection (SCRFD-2.5GF, 640×640) | Standard + Full | GitHub cysin/scrfd_onnx |
| `models/ner/model_int8.onnx` | 64 MB | NER PII detection (DistilBERT 6L, INT8) | Full | Git LFS |
| `models/ri/model_int8.onnx` | 14 MB | Response integrity R2 classifier (TinyBERT, INT8) | Full | Git LFS |
| `models/ri/vocab.txt` | ~230 KB | R2 model vocabulary | Full | Git LFS |
| `models/kws/encoder-*.int8.onnx` | 4.6 MB | Voice KWS encoder (Zipformer, INT8) | Full + `voice` feature | GitHub k2-fsa/sherpa-onnx |
| `models/kws/decoder-*.int8.onnx` | 271 KB | Voice KWS decoder | Full + `voice` feature | GitHub k2-fsa/sherpa-onnx |
| `models/kws/joiner-*.int8.onnx` | 160 KB | Voice KWS joiner | Full + `voice` feature | GitHub k2-fsa/sherpa-onnx |

**NER model selection:** Lite/Standard load `models/ner-lite/` (TinyBERT, 14 MB, 0.8ms p50). Full loads `models/ner/` (DistilBERT, 64 MB, 4.3ms p50, higher F1).

**Checksum note:** Models from `download_models.sh` are downloaded without automated checksum validation. For production deployments, verify downloaded files against known-good hashes. Git LFS models are content-addressed by SHA256.

### Configure Model Paths

After downloading, set paths in `config/openobscure.toml`:

```toml
[image]
face_model = "scrfd"                         # "scrfd" (Standard/Full) or "blazeface" (Lite)
face_model_dir = "models/blazeface"
face_model_dir_scrfd = "models/scrfd"
ocr_model_dir = "models/paddleocr"
nsfw_model_dir = "models/nsfw_classifier"

[scanner]
ner_model_dir = "models/ner-lite"            # or "models/ner" for Full tier

[response_integrity]
ri_model_dir = "models/ri"
```

Paths are resolved relative to the config file location unless absolute.

---

## L1 Plugin (In-Process Redaction)

The L1 TypeScript plugin adds a second layer of defense inside the host agent — it catches PII in tool results (web scrapes, file reads) that never pass through the HTTP proxy.

```bash
# Build the plugin
cd openobscure-plugin && npm run build

# Optional: build the NAPI native addon (upgrades 5 regex types → 15 types)
./build/build_napi.sh
```

Copy the plugin into your agent's extensions directory. It auto-detects the NAPI addon at startup.

For programmatic access from TypeScript:

```typescript
import { redactPii } from "openobscure-plugin/core";

const result = redactPii(toolOutput);
if (result.count > 0) toolOutput = result.text;
```

### Cross-Process Auth Token

L0 protects its health endpoint with a shared secret. L1 uses this secret for heartbeat checks and NER endpoint calls. Both processes must run as the **same OS user** for the default file-based exchange to work.

**How it works:**

1. On first startup, L0 generates a random 32-byte hex token and writes it to `~/.openobscure/.auth-token` (mode `0600`).
2. L1 reads `~/.openobscure/.auth-token` at plugin registration time and sends it as `X-OpenObscure-Token` on every health check.
3. L0 verifies the header on every `/_openobscure/health` request. A missing or wrong token returns `401`.

**If L1 runs as a different OS user** (systemd service, Docker with separate UID): L1 cannot read the token file, health checks return `401`, and the heartbeat degrades — even though L0 is running correctly.

**Workarounds:**

| Deployment | Solution |
|-----------|----------|
| Both processes same user (default desktop) | No action required |
| Different users (systemd, Docker separate UID) | Set `OPENOBSCURE_AUTH_TOKEN=<value>` in L0's environment and write the same value to `~<L1-user>/.openobscure/.auth-token` with mode `0600` |
| Container with shared `$HOME` mounted | Mount or symlink `~/.openobscure/` so both processes resolve to the same path |

**Using env var with L1:**

```bash
TOKEN=$(openssl rand -hex 32)
OPENOBSCURE_AUTH_TOKEN=$TOKEN ./openobscure serve

mkdir -p ~/.openobscure
echo -n "$TOKEN" > ~/.openobscure/.auth-token
chmod 600 ~/.openobscure/.auth-token
```

> When `OPENOBSCURE_AUTH_TOKEN` is set, L0 uses it directly and does **not** write `~/.openobscure/.auth-token`. L1 reads only from the file — if the file is absent, health checks will return `401`.
