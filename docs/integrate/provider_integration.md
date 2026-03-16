# LLM Provider Integration Reference

OpenObscure works with any AI agent or tool that makes HTTP requests to an LLM provider. The only change required is pointing your base URL at the proxy instead of directly at the provider.

**How it works:** The proxy maps local route prefixes to upstream provider URLs. Your API keys pass through unchanged — OpenObscure never stores or manages LLM credentials.

---

**Contents**

- [Supported Providers](#supported-providers)
- [OpenAI](#openai)
- [Anthropic](#anthropic)
- [Google Gemini (via OpenRouter)](#google-gemini-via-openrouter)
- [LangChain](#langchain)
- [Environment Variable (Cursor, Aider, Continue, etc.)](#environment-variable-cursor-aider-continue-etc)
- [curl](#curl)
- [Ollama (Local LLMs)](#ollama-local-llms)
- [Adding a Custom Provider](#adding-a-custom-provider)
- [Common Patterns](#common-patterns)
- [Wiring before_tool_call in Custom Agents](#wiring-before_tool_call-in-custom-agents)
- [Next Steps](#next-steps)

## Supported Providers

| Route Prefix | Upstream | Request Format | Response Format |
|-------------|----------|----------------|-----------------|
| `/openai` | `api.openai.com` | OpenAI Chat Completions | OpenAI (JSON + SSE delta) |
| `/anthropic` | `api.anthropic.com` | Anthropic Messages | Anthropic (JSON + SSE events) |
| `/openrouter` | `openrouter.ai/api` | OpenAI-compatible | OpenAI-compatible |
| `/ollama` | `localhost:11434` | OpenAI-compatible | OpenAI-compatible |

Google Gemini, Cohere, Mistral, and other providers are accessible via OpenRouter — use the `/openrouter` prefix with the appropriate model name.

Custom providers can be added in `config/openobscure.toml` under `[providers.<name>]`. See [Adding a Custom Provider](#adding-a-custom-provider) below.

---

## OpenAI

### Python SDK

```python
import openai, os

client = openai.OpenAI(
    base_url="http://127.0.0.1:18790/openai",  # only change
    api_key=os.environ["OPENAI_API_KEY"]        # never hard-code keys
)

response = client.chat.completions.create(
    model="gpt-4o",
    # OpenObscure encrypts any real SSN before it leaves your device.
    # This example uses a fictional format; real user input passes through unchanged.
    messages=[{"role": "user", "content": "I need help with my tax filing"}]
)
print(response.choices[0].message.content)
```

### Node.js SDK

```typescript
import OpenAI from "openai";

const client = new OpenAI({
  baseURL: "http://127.0.0.1:18790/openai",  // only change
  apiKey: process.env.OPENAI_API_KEY          // never hard-code keys
});

const response = await client.chat.completions.create({
  model: "gpt-4o",
  messages: [{ role: "user", content: "My phone is 555-555-0100" }]
  // ^ 555-555-0100 is an NANP fictional number; real numbers are encrypted by the proxy
});
```

**What the proxy intercepts:** JSON request body fields (all `messages[].content` strings, nested JSON, base64-encoded images in `messages[].content[].image_url`). Skips `model`, `stream`, `temperature`, `max_tokens`, `top_p`, `top_k`, `tools`, `functions`, `tool_choice`. Response path: FPE decryption + cognitive firewall scan. SSE streaming supported (OpenAI delta format).

---

## Anthropic

```python
import anthropic

client = anthropic.Anthropic(
    base_url="http://127.0.0.1:18790/anthropic",  # only change
    api_key=os.environ["ANTHROPIC_API_KEY"]        # never hard-code keys
)

message = client.messages.create(
    model="claude-sonnet-4-20250514",
    max_tokens=1024,
    messages=[{"role": "user", "content": "Call me at 555-555-0100"}]
    # ^ fictional NANP number; real numbers are encrypted by the proxy
)
```

**What the proxy intercepts:** JSON request body fields (all `messages[].content` strings and `messages[].content[].text` blocks, base64 images in `messages[].content[].source`). Response path: FPE decryption + cognitive firewall scan. SSE streaming supported (Anthropic event format with `content_block_delta`).

---

## Google Gemini (via OpenRouter)

```python
import openai

client = openai.OpenAI(
    base_url="http://127.0.0.1:18790/openrouter",   # route through OpenRouter
    api_key=os.environ["OPENROUTER_API_KEY"]         # never hard-code keys
)

response = client.chat.completions.create(
    model="google/gemini-2.5-pro",
    messages=[{"role": "user", "content": "What's the best approach for my project?"}]
)
```

**What the proxy intercepts:** Same as OpenAI — OpenRouter uses OpenAI-compatible request/response formats. SSE streaming supported.

> **Note:** OpenRouter may route requests to third-party model providers beyond the one you specify. Hash-token redacted values (person names, locations, organizations) are not format-preserving encrypted — they become stable tokens. If you need to limit which provider sees your data, use the provider-specific route prefix (`/openai`, `/anthropic`) instead.

---

## LangChain

```python
from langchain_openai import ChatOpenAI

llm = ChatOpenAI(
    model="gpt-4o",
    base_url="http://127.0.0.1:18790/openai",  # only change
    api_key=os.environ["OPENAI_API_KEY"]        # never hard-code keys
)

response = llm.invoke("Help me summarize my financial documents")
```

For Anthropic models via LangChain:

```python
from langchain_anthropic import ChatAnthropic

llm = ChatAnthropic(
    model="claude-sonnet-4-20250514",
    base_url="http://127.0.0.1:18790/anthropic",
    api_key=os.environ["ANTHROPIC_API_KEY"]     # never hard-code keys
)
```

**What the proxy intercepts:** Same as the underlying provider (OpenAI or Anthropic format). LangChain uses the standard SDK under the hood.

---

## Environment Variable (Cursor, Aider, Continue, etc.)

Many AI tools respect `OPENAI_BASE_URL` or provider-specific base URL environment variables:

```bash
# OpenAI-compatible tools
export OPENAI_BASE_URL=http://127.0.0.1:18790/openai

# Anthropic-compatible tools
export ANTHROPIC_BASE_URL=http://127.0.0.1:18790/anthropic

# Now run your tool normally — no code changes needed
```

This works with Cursor, Aider, Continue, Open Interpreter, and any tool that reads these environment variables.

**What the proxy intercepts:** Depends on the provider format the tool uses. The proxy auto-detects request format from the route prefix.

---

## curl

### OpenAI format

```bash
# X-OpenObscure-Token is required on all routes when OPENOBSCURE_AUTH_TOKEN is set
curl http://127.0.0.1:18790/openai/v1/chat/completions \
  -H "Authorization: Bearer $OPENAI_API_KEY" \
  -H "X-OpenObscure-Token: $OPENOBSCURE_AUTH_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-4o",
    "messages": [{"role": "user", "content": "My email is user@example.com"}]
  }'
```

### Anthropic format

```bash
curl http://127.0.0.1:18790/anthropic/v1/messages \
  -H "x-api-key: $ANTHROPIC_API_KEY" \
  -H "X-OpenObscure-Token: $OPENOBSCURE_AUTH_TOKEN" \
  -H "anthropic-version: 2023-06-01" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "claude-sonnet-4-20250514",
    "max_tokens": 1024,
    "messages": [{"role": "user", "content": "I need help with my tax documents"}]
  }'
```

**What the proxy intercepts:** The JSON body. Non-JSON content types (binary, multipart, text/plain) pass through unchanged.

---

## Ollama (Local LLMs)

```python
import openai

client = openai.OpenAI(
    base_url="http://127.0.0.1:18790/ollama/v1",
    api_key="ollama"  # Ollama ignores this field; any non-empty string works
)

response = client.chat.completions.create(
    model="llama3.2",
    messages=[{"role": "user", "content": "Help me draft a cover letter"}]
)
```

Or via environment variable:

```bash
export OPENAI_BASE_URL=http://127.0.0.1:18790/ollama/v1
```

**Why use OpenObscure with a local LLM?** Defense in depth. Local models can still log inputs, leak data through plugins, or be replaced with a compromised model. OpenObscure encrypts PII regardless of where the LLM runs.

---

## Adding a Custom Provider

Add a new `[providers.<name>]` section to `config/openobscure.toml`:

```toml
[providers.my_provider]
upstream_url = "https://api.my-provider.com"
route_prefix = "/my-provider"
strip_headers = ["x-openobscure-internal"]
```

Then use `http://127.0.0.1:18790/my-provider` as your base URL. The proxy uses longest-prefix matching when multiple providers overlap.

---

## Common Patterns

**Image PII protection:** When sending images via the OpenAI or Anthropic vision APIs, the proxy detects base64-encoded images in the request body and processes them through the image pipeline (NSFW detection, face redaction, OCR text redaction, EXIF strip) before forwarding.

**SSE streaming:** The proxy accumulates SSE frames to detect PII that spans frame boundaries, then forwards frames with FPE-encrypted values. Supported for OpenAI, Anthropic, and Gemini delta formats.

**Auth passthrough:** All original request headers (including `Authorization`, `x-api-key`, `anthropic-version`) are forwarded unchanged. The proxy adds only `x-openobscure-internal` headers for its own use, which are stripped before forwarding via the `strip_headers` config.

**Auth token on proxy routes:** When `OPENOBSCURE_AUTH_TOKEN` is set (or auto-generated on first startup), the proxy enforces it on **all routes** — including LLM proxy routes like `/openai/v1/chat/completions`. Requests missing the `X-OpenObscure-Token` header return HTTP 401. The L1 plugin handles this automatically via the shared token file (`~/.openobscure/.auth-token`); curl examples above must include the header explicitly.

**OpenClaw `baseUrl` must include `/v1`:** When configuring OpenClaw to route through the proxy, set `baseUrl` to `http://127.0.0.1:18790/openai` (not `/openai/v1`). The OpenAI-compatible SDK appends `/v1/chat/completions` automatically — adding `/v1` to the base URL results in `/v1/v1/chat/completions` and a connection error. If you see stale URLs after a config change, delete `~/.openclaw/agents/main/agent/models.json` and restart the gateway to force regeneration.

---

## Wiring before_tool_call in Custom Agents

The L1 plugin exposes a `before_tool_call` hook that intercepts tool calls **before execution**, allowing PII to be redacted from tool arguments before the tool ever runs. This complements the `tool_result_persist` hook (which fires after the tool runs, on the result).

**Current status:** The hook is defined and fully implemented in the plugin, but is not yet invoked by OpenClaw. Custom agents that implement the `PluginAPI` interface can enable it today.

### Why it matters

`tool_result_persist` catches PII in tool *outputs* (web scrapes, file reads). `before_tool_call` catches PII in tool *inputs* — for example, an LLM response that includes a user's SSN as an argument to a `send_email` or `write_file` call. Without this hook, the data reaches the tool before the plugin has a chance to redact it.

### Interface definition

```typescript
import type { PluginAPI, ToolCall, ToolResult } from "@openobscure/plugin";

// ToolCall shape
interface ToolCall {
  tool_name: string;                   // Name of the tool being called
  arguments: Record<string, unknown>;  // Tool arguments (may contain PII)
  metadata?: Record<string, unknown>;  // Optional pass-through metadata
}

// PluginAPI hooks shape (relevant fields)
interface PluginAPI {
  hooks: {
    tool_result_persist: (handler: (result: ToolResult) => ToolResult) => void;
    before_tool_call?: (handler: (call: ToolCall) => ToolCall | null) => void; // optional
  };
  registerTool: (tool: ToolDefinition) => void;
}
```

### Feature detection

The plugin checks for `api.hooks.before_tool_call` existence before registering. Your agent only needs to define the field on the `hooks` object:

```typescript
const api: PluginAPI = {
  hooks: {
    // Required — always present
    tool_result_persist: (handler) => {
      myAgent.onToolResult((result) => handler(result));
    },

    // Optional — define this to enable before_tool_call interception
    before_tool_call: (handler) => {
      myAgent.onBeforeToolCall((call) => handler(call));
    },
  },
  registerTool: (tool) => { /* ... */ },
};

register(api, { heartbeat: true });
```

If `before_tool_call` is absent or throws during registration, the plugin falls back silently to `tool_result_persist`-only mode — no exception propagates. The plugin logs `"before_tool_call registration failed, falling back to tool_result_persist-only"` at INFO level. Check your host agent's console output or the OpenObscure audit log to confirm which mode is active after startup.

### Handler contract

| Property | Requirement |
|----------|-------------|
| **Synchronous** | Yes. The handler must return `ToolCall \| null` synchronously. A returned Promise is treated as a non-null object and will not be awaited. |
| **Return modified call** | Return a new `ToolCall` object with redacted `arguments`. The plugin does this via `JSON.parse(redacted.text)`. |
| **Return original call** | Return `call` unchanged if no PII was detected (zero-cost passthrough). |
| **Return null** | Signals cancellation — the tool call should not execute. The plugin currently never returns null, but agents must handle it. Example: `const result = handler(call); if (result === null) return; /* abort tool */ executeToolWith(result);` |

> **Note on LLM-generated PII in tool arguments:** If the LLM itself generates a tool call containing user PII (for example, `write_file(content="SSN: 123-45-6789")`), that content appears in the LLM's *response* — which L0 decrypts and the cognitive firewall scans on the inbound path. The original PII in the outbound request was already encrypted before the LLM generated the response. The `before_tool_call` hook catches LLM-generated PII in tool arguments before the tool executes, but it is not yet invoked by OpenClaw.

### What the plugin redacts

The handler serializes `call.arguments` to a JSON string, scans it with the full PII engine (regex + NER if heartbeat is active and proxy is reachable), then parses the redacted JSON back to an object. All structured PII types are caught — SSN, credit card, email, phone, etc. Redacted values are replaced with `[REDACTED-<TYPE>]` tokens inline.

Example: an SSN in any string-valued argument field is replaced before the tool executes:

```typescript
// Input
{ tool_name: "send_email", arguments: { body: "My SSN is 123-45-6789" } }

// Output from handler
{ tool_name: "send_email", arguments: { body: "My SSN is [REDACTED-SSN]" } }
```

### Activation condition

`before_tool_call` is only registered when `redactToolResults` is `true` (the default). Passing `redactToolResults: false` to `register()` disables both hooks.

```typescript
register(api, { redactToolResults: false }); // neither hook registers
```

### Minimal wiring example

```typescript
import { register } from "@openobscure/plugin";
import type { PluginAPI } from "@openobscure/plugin";

function wirePlugin(agent: MyAgent): void {
  const api: PluginAPI = {
    hooks: {
      tool_result_persist: (handler) => {
        // Must be synchronous — do not async-wrap
        agent.beforePersist((result) => handler(result));
      },
      before_tool_call: (handler) => {
        agent.beforeToolExecution((call) => {
          const sanitized = handler(call);
          // null means cancel — decide how your agent handles cancellation
          return sanitized ?? null;
        });
      },
    },
    registerTool: (tool) => agent.addTool(tool),
  };

  register(api, { heartbeat: true, proxyUrl: "http://127.0.0.1:18790" });
}
```

---

## Next Steps

- [FPE Configuration](../configure/fpe-configuration.md) — key generation, rotation, per-type encryption behavior, fail modes
- [Detection Engine Configuration](../configure/detection-engine-configuration.md) — which scanners run, how to force a specific engine
- [Config Reference](../configure/config-reference.md) — every TOML key with type, default, and description
