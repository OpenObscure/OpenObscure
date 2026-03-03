# Integration in 5 Minutes

OpenObscure works with **any AI agent** that makes HTTP requests to an LLM provider. No SDK required — just point your LLM base URL at the proxy.

---

## 1. Start the proxy

```bash
# First time only: generate an encryption key
cargo run --release -- --init-key

# Start the proxy
cargo run --release -- -c config/openobscure.toml
```

The proxy listens on `127.0.0.1:18790` by default.

---

## 2. Point your agent at the proxy

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

---

## 3. Check what was protected

```bash
curl http://127.0.0.1:18790/_openobscure/health | python3 -m json.tool
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

Add custom providers in `config/openobscure.toml` under `[[providers]]`.

---

## What Happens Under the Hood

1. Your agent sends a request containing PII (e.g., `"My SSN is 123-45-6789"`)
2. OpenObscure intercepts the request on localhost
3. PII is detected using regex + CRF + NER (TinyBERT) ensemble
4. Each PII match is encrypted with **FF1 Format-Preserving Encryption** — the ciphertext looks realistic (e.g., `847-29-3156`) so the LLM can still reason about the data structure
5. The sanitized request is forwarded to the real LLM provider
6. The LLM response is decrypted before returning to your agent

Your real PII never leaves your device.
