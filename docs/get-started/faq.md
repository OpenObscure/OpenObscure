# FAQ

**Does OpenObscure read local files to scan for PII?**
No. OpenObscure never performs file I/O. The agent's tools (`file_read`, `web_fetch`, etc.) read files and produce text results. OpenObscure's L1 plugin only sees the resulting text after the agent has extracted it, via the tool result persistence hook.

**Does OpenObscure need its own API keys?**
No. By default, OpenObscure forwards the host agent's existing API keys unchanged (passthrough-first). It never provisions, generates, or requires separate LLM credentials.

**Does OpenObscure phone home or contact external servers?**
No. The only network traffic OpenObscure produces is forwarding the host agent's existing LLM API requests through the local proxy. No telemetry, no update checks, no external dependencies at runtime. Everything runs locally on the user's device.

**Is L0 (the proxy) a separate server I need to host?**
No. L0 runs as a lightweight sidecar process on the same device as the host agent, listening on `127.0.0.1:18790` (localhost only). It is started alongside the agent and is not exposed to the network.

**Does OpenObscure intercept data before the LLM sees it?**
L0 (proxy) does — it sits in the HTTP path and encrypts PII before the request reaches the LLM provider. L1 (plugin) hooks the agent's tool result persistence, which fires after tool execution. L1 prevents PII from being persisted to transcripts, but cannot prevent it from being sent to the LLM in tool results that route directly through the proxy.

**How much RAM does OpenObscure actually use?**
It depends on the hardware tier. OpenObscure detects RAM at startup and selects features automatically:
- Lite (<2GB RAM): ~12–80MB
- Standard (2–4GB): ~67–200MB
- Full (≥4GB): up to 224MB peak, 275MB hard ceiling

On mobile, the budget is 20% of device RAM (capped at 275MB). See [Deployment Tiers](deployment-tiers.md) for the full breakdown.

**What happens if OpenObscure is disabled or crashes?**
If L0 is not running, the host agent cannot reach LLM providers — traffic is routed through the proxy, so a missing proxy breaks connectivity. If L1 crashes, the agent continues normally but tool results are not redacted. If OpenObscure is fully disabled via configuration, the agent operates with direct LLM connections and zero overhead.

**Can I run just L0 without L1?**
Yes. L0 alone provides the core privacy protection — all PII encryption, image pipeline, voice pipeline, and cognitive firewall. L1 adds a second layer only for tool results (web scrapes, file reads) that never pass through HTTP. For the Embedded model (mobile), L1 is not used at all.

**What is FF1 FPE and why not just redact?**
FF1 (NIST SP 800-38G) is Format-Preserving Encryption: a credit card number encrypts to another valid-looking credit card number, an SSN to another SSN-format value. The LLM sees plausible data rather than `[REDACTED]` tokens, preserving its ability to reason about structure and context. See [FPE Configuration](../configure/fpe-configuration.md) for full details.
