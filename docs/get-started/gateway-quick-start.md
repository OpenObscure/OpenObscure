# Gateway Quick Start

OpenObscure works with **any AI agent** that makes HTTP requests to an LLM provider. No SDK required — just point your LLM base URL at the proxy.

---

## Prerequisites

| Tool | Minimum version | Notes |
|------|----------------|-------|
| Rust | **1.75** | MSRV set in `openobscure-proxy/Cargo.toml`. Install via [rustup.rs](https://rustup.rs). |
| Cargo | ships with Rust | Bundled with the Rust toolchain. No separate minimum. |
| Node.js | **18** | Required for the L1 plugin (`openobscure-plugin/`) only. The proxy itself has no Node.js dependency. CI tests on Node 22. |
| npm | ships with Node.js | Required for `npm ci` and `npm run build` in `openobscure-plugin/`. Bundled with Node.js. |
| Git LFS | any | Required to pull NER, NSFW, KWS, and RI model files via `git lfs pull`. Not needed if skipping those model tiers. |
| ONNX Runtime | auto-downloaded | The `ort` crate (`=2.0.0-rc.11`) downloads the native library at build time via the `download-binaries` feature. No manual installation required. |

---

> **Several protection layers are inactive without model files.**
>
> The proxy starts and accepts traffic without any ONNX models, but the following features are silently disabled until the corresponding model files are present and configured:
>
> | Feature | Inactive when... | What still works |
> |---------|-----------------|-----------------|
> | NER-based PII detection (names, locations, orgs) | `ner_model_dir` not set | Regex + keyword + gazetteer — 15 structured PII types |
> | Face redaction | `face_model_dir` not set | Images forwarded without face blurring |
> | OCR text redaction | `ocr_model_dir` not set | Images forwarded with text visible |
> | NSFW detection | `nsfw_model_dir` not set | Images forwarded without nudity check |
> | R2 cognitive firewall | `ri_model_dir` not set | R1 dictionary-based persuasion detection still runs |
> | Voice keyword spotting | KWS models absent | Audio pass-through without PII detection |
>
> No error is raised and no warning is logged in the response path when a model is missing — requests complete normally with the affected layer skipped.
>
> **To enable all features:** see [Step 3 — Download model files](#3-download-model-files) before starting the proxy.

---

## 1. Build the proxy

```bash
cd openobscure-proxy
cargo build --release
```

## 2. Generate an FPE key (first time only)

```bash
# Store a 256-bit AES key in your OS keychain
./target/release/openobscure-proxy --init-key
```

**Headless / Docker alternative** (no keychain required):

```bash
export OPENOBSCURE_MASTER_KEY=$(openssl rand -hex 32)
```

## 3. Download model files

OpenObscure uses ONNX models for face detection, OCR, NSFW classification, NER-based PII scanning, and voice keyword spotting. Run the download script for your deployment tier:

```bash
# From the repo root
./build/download_models.sh lite      # ~11MB  — face detection + OCR
./build/download_models.sh standard  # ~14MB  — adds SCRFD face detection
./build/download_models.sh full      # same as standard; NER/RI/KWS via Git LFS
```

For NER, response integrity (R2 cognitive firewall), and voice KWS models, which are stored in Git LFS:

```bash
git lfs pull                           # fetch all LFS-tracked model files
# or download KWS models separately:
./build/download_kws_models.sh         # sherpa-onnx Zipformer KWS (~5MB)
```

### Model files by tier

| Model file | Size | Feature | Tier | Source |
|-----------|------|---------|------|--------|
| `models/blazeface/blazeface.onnx` | 408 KB | Face detection (128×128 input) | All tiers | [ailia-models GCS](https://storage.googleapis.com/ailia-models/blazeface/blazeface.onnx) |
| `models/paddleocr/det_model.onnx` | 2.3 MB | OCR text region detection | All tiers | [HuggingFace monkt/paddleocr-onnx](https://huggingface.co/monkt/paddleocr-onnx/resolve/main/detection/v3/det.onnx) |
| `models/paddleocr/rec_model.onnx` | 7.3 MB | OCR text recognition (PP-OCRv4) | All tiers | [HuggingFace deepghs/paddleocr](https://huggingface.co/deepghs/paddleocr/resolve/main/rec/en_PP-OCRv4_rec/model.onnx) |
| `models/paddleocr/ppocr_keys.txt` | ~2 KB | OCR 95-char English dictionary | All tiers | [HuggingFace deepghs/paddleocr](https://huggingface.co/deepghs/paddleocr/resolve/main/rec/en_PP-OCRv4_rec/dict.txt) |
| `models/ner-lite/model_int8.onnx` | 14 MB | NER PII detection (TinyBERT 4L, INT8) | Lite + Standard | Git LFS |
| `models/nsfw_classifier/nsfw_5class_int8.onnx` | 83 MB | NSFW classification (ViT-base-patch16-224, INT8, 5 classes) | All tiers | Git LFS |
| `models/scrfd/scrfd_2.5g.onnx` | 3.1 MB | Face detection (SCRFD-2.5GF, 640×640) | Standard + Full | [GitHub cysin/scrfd_onnx](https://github.com/cysin/scrfd_onnx/raw/refs/heads/main/scrfd_2.5g_bnkps_shape640x640.onnx) |
| `models/ner/model_int8.onnx` | 64 MB | NER PII detection (DistilBERT 6L, INT8) | Full | Git LFS |
| `models/ri/model_int8.onnx` | 14 MB | Response integrity R2 classifier (TinyBERT, INT8) | Full | Git LFS |
| `models/ri/vocab.txt` | ~230 KB | R2 model vocabulary | Full | Git LFS |
| `models/kws/encoder-*.int8.onnx` | 4.6 MB | Voice KWS encoder (Zipformer, INT8) | Full + voice feature | [GitHub k2-fsa/sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx/releases/tag/kws-models) |
| `models/kws/decoder-*.int8.onnx` | 271 KB | Voice KWS decoder (Zipformer, INT8) | Full + voice feature | [GitHub k2-fsa/sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx/releases/tag/kws-models) |
| `models/kws/joiner-*.int8.onnx` | 160 KB | Voice KWS joiner (Zipformer, INT8) | Full + voice feature | [GitHub k2-fsa/sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx/releases/tag/kws-models) |

**NER model selection by tier:** Lite and Standard tiers load `models/ner-lite/` (TinyBERT, 14 MB, 0.8 ms p50). Full tier loads `models/ner/` (DistilBERT, 64 MB, 4.3 ms p50, higher F1). Both directories include a `model_int8.onnx`, `vocab.txt`, `tokenizer.json`, and supporting files.

**NSFW model note:** The current model (`nsfw_5class_int8.onnx`) is a ViT-base-patch16-224 (INT8) that replaces the previous NudeNet + ViT-tiny cascade as of commit `96b35d8`. The legacy `models/nudenet/320n.onnx` and `models/nsfw_classifier/nsfw_classifier.onnx` (FP32) files are no longer used by the image pipeline.

### All models are optional

The proxy starts and handles requests without any model files present. Models are lazy-loaded on first use:

- Missing `nsfw_model_dir` → NSFW detection disabled for this session; images pass through unscanned for nudity.
- Missing `face_model_dir` → face detection and face redaction disabled.
- Missing `ocr_model_dir` → OCR text redaction disabled.
- Missing `ri_model_dir` → R2 cognitive firewall disabled; R1 dictionary-only response integrity still runs.
- Missing NER model (`ner_model_dir`) → hybrid scanner falls back to regex + keyword + gazetteer.

### Checksum verification

Models fetched by `download_models.sh` and `download_kws_models.sh` are downloaded directly from their upstream sources without automated checksum validation in the script. For production deployments, verify downloaded files against known-good hashes before use.

Models committed to the repository via Git LFS are content-addressed by Git LFS's SHA256 object store. Running `git lfs pull` fetches the exact committed versions — substitution requires tampering with the repository itself. See [SECURITY.md](../../SECURITY.md#image-processing-attack-surface-phase-3) for the ONNX model substitution threat model.

### Configure model paths

After downloading, tell the proxy where the models live in `config/openobscure.toml`:

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

## 4. Start the proxy

```bash
./target/release/openobscure-proxy serve
```

The proxy reads `config/openobscure.toml` by default. Override with `--config /path/to/file.toml` or the `OPENOBSCURE_CONFIG` env var. It listens on `127.0.0.1:18790` by default.

## 5. Point your agent at the proxy

Pick your language/tool — the only change is the base URL:

### Python (OpenAI SDK)

```python
import openai

# Just change the base URL — everything else stays the same
client = openai.OpenAI(
    base_url="http://127.0.0.1:18790/openai",
    api_key="sk-..."  # your real API key
)

response = client.chat.completions.create(
    model="gpt-4o",
    messages=[{"role": "user", "content": "My SSN is 123-45-6789 and I need tax help"}]
)
# The LLM never sees your real SSN — OpenObscure encrypts it with FF1
print(response.choices[0].message.content)
```

### Python (Anthropic SDK)

```python
import anthropic

client = anthropic.Anthropic(
    base_url="http://127.0.0.1:18790/anthropic",
    api_key="sk-ant-..."
)

message = client.messages.create(
    model="claude-sonnet-4-20250514",
    max_tokens=1024,
    messages=[{"role": "user", "content": "Call me at 555-123-4567"}]
)
```

### curl

```bash
curl http://127.0.0.1:18790/openai/v1/chat/completions \
  -H "Authorization: Bearer sk-..." \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-4o",
    "messages": [{"role": "user", "content": "My email is john@example.com"}]
  }'
```

### LangChain

```python
from langchain_openai import ChatOpenAI

llm = ChatOpenAI(
    model="gpt-4o",
    base_url="http://127.0.0.1:18790/openai",
    api_key="sk-..."
)

response = llm.invoke("My card number is 4111-1111-1111-1111")
```

### Node.js (OpenAI SDK)

```typescript
import OpenAI from "openai";

const client = new OpenAI({
  baseURL: "http://127.0.0.1:18790/openai",
  apiKey: "sk-..."
});

const response = await client.chat.completions.create({
  model: "gpt-4o",
  messages: [{ role: "user", content: "My phone is 555-867-5309" }]
});
```

### Environment Variable (Works with most tools)

Many AI tools respect `OPENAI_BASE_URL`:

```bash
export OPENAI_BASE_URL=http://127.0.0.1:18790/openai
# Now run your tool normally — Cursor, Aider, Continue, etc.
```

## 6. Check what was protected

```bash
curl -s http://127.0.0.1:18790/_openobscure/health | python3 -m json.tool
```

Look for `pii_matches_total` — this shows how many PII items were encrypted before reaching the LLM.

---

## Supported Providers

| Route Prefix | Upstream Provider |
|-------------|-------------------|
| `/openai` | OpenAI (api.openai.com) |
| `/anthropic` | Anthropic (api.anthropic.com) |
| `/openrouter` | OpenRouter (openrouter.ai) |
| `/ollama` | Ollama (localhost:11434) |

Add custom providers in `config/openobscure.toml` under `[providers.<name>]`. For the full provider reference — Google Gemini, Ollama, custom providers, per-provider interception details, and SSE streaming — see [Integration Reference](../integrate/integration-reference.md).

---

## CLI Subcommands

| Command | Description |
|---------|-------------|
| `openobscure-proxy serve` | Start the proxy (default if no subcommand given) |
| `openobscure-proxy key-rotate` | Rotate the FPE encryption key (zero-downtime, 30s overlap window) |
| `openobscure-proxy passthrough` | Run in passthrough mode (no PII scanning or encryption) |
| `openobscure-proxy service install` | Install as a background service (launchd on macOS, systemd on Linux) |
| `openobscure-proxy service start` | Start the installed background service |
| `openobscure-proxy service stop` | Stop the background service |
| `openobscure-proxy service stop --passthrough` | Stop the managed service and immediately start passthrough |
| `openobscure-proxy service status` | Check background service status |
| `openobscure-proxy service uninstall` | Remove the background service |

### Passthrough mode

```bash
openobscure-proxy passthrough
```

Passthrough mode starts a lightweight HTTP relay on the same port (`127.0.0.1:18790` by default) that forwards requests directly to upstream providers. **Every PII protection feature is disabled:**

- No PII scanning (no regex, no keyword matching, no NER/CRF)
- No FPE encryption or decryption — plaintext is forwarded and returned as-is
- No image pipeline (no NSFW detection, no face redaction, no OCR redaction, no EXIF strip)
- No response integrity scanning (no R1 dictionary, no R2 cognitive firewall)
- No SSE accumulation — response body is streamed byte-for-byte without buffering
- No request journal — crash recovery is disabled in this mode

The health endpoint returns `"status": "passthrough"` and `"pii_protection": false`. The L1 plugin heartbeat detects this response and automatically drops from NER-assisted redaction to regex-only redaction for in-process tool results. L0-level PII protection is not restored until the proxy is restarted in `serve` mode.

> **Security warning:** All PII sent through the proxy in passthrough mode reaches the LLM provider in plaintext. Use passthrough only when privacy protection is intentionally not required — for example, during initial integration testing before key provisioning, or as a short-term fallback while the full proxy is being upgraded. Never use passthrough mode in production environments handling real user data.

**When to use passthrough:**

- **Integration testing** — verify your agent's base URL and provider routing before enabling PII protection, without needing an FPE key provisioned.
- **Temporary fallback after `service stop`** — keeps the agent working while the managed service is reloading or being upgraded. Use `service stop --passthrough` to do this atomically.
- **Debugging upstream connectivity** — isolate whether a problem is in OpenObscure's processing or in the upstream provider by temporarily bypassing all interception.

#### Passthrough vs. `scanner_mode = "regex"`

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

`scanner_mode = "regex"` is a **full privacy pipeline** that skips ML model loading (NER/CRF) in favour of pattern matching. It detects and encrypts PII using regex, keyword dictionary, and gazetteer — all 15 PII types at full throughput, with no ONNX model dependency. Configure it in `config/openobscure.toml`:

```toml
[scanner]
scanner_mode = "regex"   # Options: "auto" (default), "regex", "ner", "crf"
```

`passthrough` is not a scanner mode — it is a separate binary entry point that bypasses the entire `proxy.rs` request handler.

---

## Optional: L1 Plugin (In-Process Redaction)

The L1 TypeScript plugin adds a second layer of defense inside the host agent — it catches PII in tool results (web scrapes, file reads) that never pass through the HTTP proxy.

```bash
# Build the plugin
cd openobscure-plugin && npm run build

# Optional: build the NAPI native addon (upgrades 5 regex types → 15 types)
./build/build_napi.sh
```

Copy the plugin into your agent's extensions directory. It auto-detects the NAPI addon at startup.

### Cross-Process Auth Token Requirements

L0 protects its health endpoint with a shared secret. L1 uses this secret for heartbeat checks and NER endpoint calls. For the token exchange to work, both processes must run as the **same OS user**.

**How the token is established:**

1. On first startup, L0 generates a random 32-byte hex token and writes it to `~/.openobscure/.auth-token` with permissions `0600` (owner read/write only on Unix). The file path is resolved from `$HOME` (or `%USERPROFILE%` on Windows).
2. L1 reads `~/.openobscure/.auth-token` at plugin registration time using the same `$HOME` / `$USERPROFILE` resolution. It sends the token as `X-OpenObscure-Token` on every health check.
3. L0 verifies the header on every request to `/_openobscure/health`. A missing or wrong token returns `401 Unauthorized`.

**What happens when L1 runs as a different OS user:**

- L1's `$HOME` resolves to a different directory — it looks for `~<other-user>/.openobscure/.auth-token`, which either does not exist (ENOENT) or is owned by the L0 user and unreadable (EACCES, because `0600` denies non-owners).
- Both cases are caught silently in `readAuthToken()`. L1 proceeds with `authToken = undefined`.
- All health checks reach L0 without the `X-OpenObscure-Token` header. L0 rejects them with `401`.
- The heartbeat monitor transitions to `degraded` state and logs an error, even though L0 is running correctly.
- NER endpoint calls (used for enhanced entity detection in tool results) also fail with `401`.

**Workarounds:**

| Deployment | Solution |
|-----------|----------|
| Both processes same user (default desktop setup) | No action required — file exchange works automatically |
| Different users (systemd service, Docker with separate UID) | Set `OPENOBSCURE_AUTH_TOKEN=<value>` in L0's environment **and** write the same value to `~<L1-user>/.openobscure/.auth-token` with mode `0600` |
| Container with shared `$HOME` mounted | Mount or symlink `~/.openobscure/` so both processes resolve to the same path |

**`OPENOBSCURE_AUTH_TOKEN` env var behavior:**

When `OPENOBSCURE_AUTH_TOKEN` is set, L0 uses it directly and **does not write** `~/.openobscure/.auth-token`. L1's `readAuthToken()` reads only from the file — it does not check any environment variable. If L0 is configured via env var and the file is absent, L1 will not find the token and health checks will return `401`.

To use the env var approach with L1:

```bash
# Generate a token
TOKEN=$(openssl rand -hex 32)

# Pass to L0 via env var (file is not written)
OPENOBSCURE_AUTH_TOKEN=$TOKEN ./openobscure-proxy serve

# Write the same token to the file path L1 reads, with correct permissions
mkdir -p ~/.openobscure
echo -n "$TOKEN" > ~/.openobscure/.auth-token
chmod 600 ~/.openobscure/.auth-token
```

For programmatic access from TypeScript/JavaScript:

```typescript
import { redactPii } from "openobscure-plugin/core";

// Auto-uses native scanner (15 types) if @openobscure/scanner-napi installed,
// otherwise falls back to JS regex (5 types)
const result = redactPii(toolOutput);
if (result.count > 0) toolOutput = result.text;
```

---

## What Happens Under the Hood

1. Your agent sends a request containing PII (e.g., `"My SSN is 123-45-6789"`)
2. OpenObscure detects PII using regex + CRF + NER (TinyBERT) ensemble
3. Each match is encrypted with **FF1 Format-Preserving Encryption** — ciphertext looks realistic (e.g., `847-29-3156`) so the LLM can still reason about the data structure
4. The sanitized request goes to the LLM provider via the proxy
5. The LLM response is decrypted before returning to the user
6. The cognitive firewall scans the response for persuasion techniques

Your real PII never leaves your device. For the full architecture, see [System Overview](../architecture/system-overview.md).
