# Quick Start

OpenObscure has two deployment models. Pick the one that fits your use case:

| Model | Use Case |
|-------|----------|
| [**Gateway**](#gateway-quick-start) | Any AI agent that makes HTTP requests to an LLM — just change the base URL |
| [**Embedded**](#embedded-quick-start) | Compile into your iOS/macOS/Android app as a native library — no proxy needed |

---

## Gateway Quick Start

OpenObscure works with **any AI agent** that makes HTTP requests to an LLM provider. No SDK required — just point your LLM base URL at the proxy.

### 1. Start the proxy

```bash
# First time only: generate an encryption key
cargo run --release -- --init-key

# Start the proxy
cargo run --release -- -c config/openobscure.toml
```

The proxy listens on `127.0.0.1:18790` by default.

### 2. Point your agent at the proxy

#### Python (OpenAI SDK)

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

#### Python (Anthropic SDK)

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

#### curl

```bash
curl http://127.0.0.1:18790/openai/v1/chat/completions \
  -H "Authorization: Bearer sk-..." \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-4o",
    "messages": [{"role": "user", "content": "My email is john@example.com"}]
  }'
```

#### LangChain

```python
from langchain_openai import ChatOpenAI

llm = ChatOpenAI(
    model="gpt-4o",
    base_url="http://127.0.0.1:18790/openai",
    api_key="sk-..."
)

response = llm.invoke("My card number is 4111-1111-1111-1111")
```

#### Node.js (OpenAI SDK)

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

#### Environment Variable (Works with most tools)

Many AI tools respect `OPENAI_BASE_URL`:

```bash
export OPENAI_BASE_URL=http://127.0.0.1:18790/openai
# Now run your tool normally — Cursor, Aider, Continue, etc.
```

### 3. Check what was protected

```bash
curl http://127.0.0.1:18790/_openobscure/health | python3 -m json.tool
```

Look for `pii_matches_total` — this shows how many PII items were encrypted before reaching the LLM.

### Supported Providers

| Route Prefix | Upstream Provider |
|-------------|-------------------|
| `/openai` | OpenAI (api.openai.com) |
| `/anthropic` | Anthropic (api.anthropic.com) |
| `/openrouter` | OpenRouter (openrouter.ai) |
| `/ollama` | Ollama (localhost:11434) |

Add custom providers in `config/openobscure.toml` under `[[providers]]`.

---

## Embedded Quick Start

Sanitize PII directly in your iOS, macOS, or Android app — no proxy, no HTTP server. Build the library, generate bindings, call three functions.

### 1. Build the library

```bash
cd ~/Desktop/OpenObscure

# macOS
cargo build --manifest-path openobscure-proxy/Cargo.toml \
  --lib --no-default-features --features mobile --release

# iOS (device + simulator)
./build/build_ios.sh --release

# Android (ARM64)
./build/build_android.sh --release
```

### 2. Generate bindings

```bash
# Swift (iOS/macOS)
./build/generate_bindings.sh --swift-only

# Kotlin (Android)
./build/generate_bindings.sh --kotlin-only
```

### 3. Sanitize text (Swift)

```swift
import openobscure_proxy

// Generate a 32-byte hex key (64 chars) and store in iOS Keychain
let fpeKey = "0123456789abcdef..."  // 64 hex characters

// Point to bundled models — auto-detects device tier and loads accordingly
let modelsDir = Bundle.main.resourcePath! + "/models"
let config = """
{"scanner_mode": "auto", "models_base_dir": "\(modelsDir)"}
"""

let handle = try createOpenobscure(configJson: config, fpeKeyHex: fpeKey)

// Sanitize before sending to LLM
let result = try sanitizeText(handle: handle, text: "My SSN is 123-45-6789")
print(result.sanitizedText)  // "My SSN is 847-29-3156" (encrypted, not real)

// Save result.mappingJson — you'll need it to restore

// After LLM responds, restore original PII
let restored = try restoreText(
    handle: handle,
    text: llmResponse,
    mappingJson: result.mappingJson
)
```

### 3. Sanitize text (Kotlin)

```kotlin
import uniffi.openobscure_proxy.*

val fpeKey = "0123456789abcdef..."  // 64 hex characters
val modelsDir = copyAssetsDir(context, "models")  // copy from assets to internal storage
val config = """{"scanner_mode": "auto", "models_base_dir": "$modelsDir"}"""

val handle = createOpenobscure(configJson = config, fpeKeyHex = fpeKey)

// Sanitize before sending to LLM
val result = sanitizeText(handle = handle, text = "My card is 4111-1111-1111-1111")
println(result.sanitizedText)  // card number is encrypted

// Restore after LLM responds
val restored = restoreText(
    handle = handle,
    text = llmResponse,
    mappingJson = result.mappingJson
)
```

### Full API

| Function | Purpose |
|----------|---------|
| `createOpenobscure(configJson, fpeKeyHex)` | Initialize with config and FPE key |
| `sanitizeText(handle, text)` | Scan + encrypt PII, return sanitized text + mapping |
| `restoreText(handle, text, mappingJson)` | Decrypt FPE values using saved mapping |
| `sanitizeImage(handle, imageBytes)` | EXIF strip (always) + face/OCR/NSFW redaction (model-dependent) |
| `sanitizeAudioTranscript(handle, transcript)` | Scan speech transcript for PII |
| `checkAudioPii(handle, transcript)` | Quick PII count (no encryption) |
| `rotateKey(handle, newKeyHex)` | Rotate FPE key with 30-second overlap |
| `scanResponse(handle, text)` | Scan LLM response for manipulation |

For Xcode/Gradle project setup and full integration examples, see the [Integration Guide](integration/INTEGRATION_GUIDE.md).

---

## What Happens Under the Hood

1. Your agent sends a request containing PII (e.g., `"My SSN is 123-45-6789"`)
2. OpenObscure detects PII using regex + CRF + NER (TinyBERT) ensemble
3. Each match is encrypted with **FF1 Format-Preserving Encryption** — ciphertext looks realistic (e.g., `847-29-3156`) so the LLM can still reason about the data structure
4. The sanitized request goes to the LLM provider (via proxy or directly from your app)
5. The response is decrypted before returning to the user

Your real PII never leaves your device.
