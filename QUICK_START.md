# Quick Start

Get the proxy running in under 2 minutes.

```bash
# 1. Build
cd openobscure-core && cargo build --release

# 2. Generate an FPE key — stored in your OS keychain (first time only)
./target/release/openobscure-core --init-key

# 3. Start
./target/release/openobscure-core serve

# 4. Point your agent at the proxy — change one line:
#    base_url = "http://127.0.0.1:18790/openai"   # was: https://api.openai.com
#    base_url = "http://127.0.0.1:18790/anthropic" # was: https://api.anthropic.com

# 5. Verify
curl http://127.0.0.1:18790/health
```

This starts the proxy in regex-only mode — no model downloads required. PII in requests is detected (15 structured types) and encrypted with FF1 FPE before reaching the LLM.

## Go Deeper

- [Gateway Quick Start](docs/get-started/gateway-quick-start.md) — model downloads, full provider setup, L1 plugin, passthrough mode
- [Embedded Quick Start](docs/get-started/embedded-quick-start.md) — compile into your iOS/macOS/Android app as a native library
