# Gateway Quick Start

OpenObscure works with **any AI agent** that makes HTTP requests to an LLM provider. No SDK required — just point your LLM base URL at the proxy.

---

## Prerequisites

| Tool | Minimum version | Required for |
|------|----------------|-------------|
| Rust | **1.75** | All builds. Install via [rustup.rs](https://rustup.rs). |
| Git LFS | any | Model files (NER, NSFW, KWS, RI). Skip for regex-only builds. |
| Node.js | **18** | L1 plugin only — not needed for the proxy itself. |

ONNX Runtime is auto-downloaded at build time — no manual installation required.

---

## 1. Build the proxy

```bash
cd openobscure-core
cargo build --release
```

## 2. Generate an FPE key (first time only)

```bash
# Store a 256-bit AES key in your OS keychain
cd openobscure-core
./target/release/openobscure --init-key
```

> **Note:** Always run the built binary directly (`./target/release/openobscure`), not via
> `cargo run` — the workspace has two binaries (`openobscure` and `uniffi-bindgen`) so
> `cargo run` requires `--bin openobscure` to disambiguate.

**Headless / Docker alternative** (no keychain required):

```bash
export OPENOBSCURE_MASTER_KEY=$(openssl rand -hex 32)
```

## 3. Download model files

```bash
# From the repo root — choose your deployment tier
./build/download_models.sh lite      # ~11MB  — face detection + OCR
./build/download_models.sh standard  # ~14MB  — adds SCRFD face detection
./build/download_models.sh full      # same as standard; NER/RI/KWS via Git LFS

# For NER, cognitive firewall (R2), and voice KWS models (stored in Git LFS):
git lfs pull
```

> The proxy starts without model files. Missing models disable those features silently — regex + keyword detection still runs for 15 structured PII types. See [Gateway Operations](gateway-operations.md#model-files) for the full model table and config paths.

## 4. Start the proxy

```bash
./target/release/openobscure serve
```

Listens on `127.0.0.1:18790` by default. Override config with `--config /path/to/file.toml`.

## 5. Point your agent at the proxy

Change one line — the base URL:

```python
import openai

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

For Node.js, curl, LangChain, Anthropic SDK, and environment variable approaches, see [Integration Reference](../integrate/integration-reference.md).

## 6. Verify

```bash
# Add -H "X-OpenObscure-Token: $OPENOBSCURE_AUTH_TOKEN" if auth token is configured
curl -s http://127.0.0.1:18790/_openobscure/health | python3 -m json.tool
```

Look for `pii_matches_total` to confirm PII is being encrypted before reaching the LLM.

---

## How It Works

1. Your agent sends a request containing PII (e.g., `"My SSN is 123-45-6789"`)
2. OpenObscure detects PII using regex + CRF + NER (TinyBERT) ensemble
3. Each match is encrypted with FF1 Format-Preserving Encryption — ciphertext looks realistic (e.g., `847-29-3156`) so the LLM can still reason about the data structure
4. The sanitized request goes to the LLM provider
5. The LLM response is decrypted before returning to your agent
6. The cognitive firewall scans the response for persuasion techniques

Your real PII never leaves your device. For the full architecture, see [System Overview](../architecture/system-overview.md).

---

## Next Steps

- [Gateway Operations](gateway-operations.md) — CLI subcommands, passthrough mode, L1 plugin setup, model config, cross-process auth
- [Configure](../configure/) — FPE settings, detection engine tuning, full config reference
- [Integration Reference](../integrate/integration-reference.md) — all provider examples, SSE streaming, custom providers
