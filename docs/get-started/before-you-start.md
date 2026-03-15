# Before You Start

This document covers decisions and constraints that affect security posture and regulatory exposure from the first request forward. These are easier to get right before deployment than to correct retroactively.

---

**Contents**

- [Compliance Considerations Before First Deployment](#compliance-considerations-before-first-deployment)
- [Host Agent Compatibility Requirements](#host-agent-compatibility-requirements)
- [What OpenObscure Does Not Guarantee](#what-openobscure-does-not-guarantee)
- [Pre-Deployment Checklist](#pre-deployment-checklist)
- [Next Steps](#next-steps)

## Compliance Considerations Before First Deployment

### What OpenObscure is and is not

OpenObscure is a technical control that reduces the amount of PII reaching LLM providers. It is not a compliance solution, a legal advisor, or a guarantee of regulatory conformance. Attestation, Business Associate Agreements, Data Processing Agreements, audit trails, staff training, and other administrative controls required by HIPAA, GDPR, and PCI-DSS remain the operator's responsibility.

What OpenObscure provides: a cryptographic transformation (FF1 FPE) and a set of detection engines that, when operating correctly, prevent structured PII from reaching LLM providers in plaintext. What happens when those engines fail is controlled by a single configuration decision.

---

### `fail_mode`: the central compliance decision

The `[proxy]` section of `openobscure.toml` has one setting that governs the entire system's behavior under failure conditions:

```toml
[proxy]
fail_mode = "open"   # default — fail open, forward original on errors
# fail_mode = "closed"  # fail closed — block or redact on errors
```

This setting controls two distinct code paths, each with different compliance implications.

#### Code path 1: per-span FPE encryption errors

For each PII match in a request, the proxy attempts FF1 encryption. If that encryption fails (key temporarily unavailable, internal error):

| Mode | What happens | What the LLM receives |
|------|-------------|----------------------|
| `fail_mode = "open"` | Original text forwarded unchanged | Plaintext PII in the message |
| `fail_mode = "closed"` | `[REDACTED:{type}]` substituted | Opaque label — PII not disclosed |

When fail-open triggers, `fpe_unprotected_total` is incremented and the response carries `x-openobscure-pii-unprotected: N`. A WARN-level log line is written:

```
WARN  body  FPE encryption failed, plaintext PII forwarded (fail-open)  type=ssn  error=<msg>
```

When fail-closed triggers, the same counter is incremented and the substitution `[REDACTED:ssn]` is irreversible — the mapping is not stored, and the original value cannot be recovered from the LLM response.

**Exception — `DomainTooSmall`:** When a detected value is too short for FF1 to encrypt (domain smaller than the minimum FF1 radix requirement), a deterministic hash token is substituted in both modes. This case is not counted in `fpe_unprotected_total` and does not affect the unprotected count.

#### Code path 2: whole-body scan errors

If the entire request body scan fails (JSON parse error, serialization failure):

| Mode | What happens | HTTP status to agent |
|------|-------------|---------------------|
| `fail_mode = "open"` | Entire original request body forwarded unchanged | None — request proceeds |
| `fail_mode = "closed"` | Request blocked entirely | **502 Bad Gateway** |

Under fail-open whole-body errors, every PII value in the request reaches the LLM, regardless of what was detected before the error.

#### What `fail_mode` does NOT cover

`fail_mode` applies only to the text PII scanning and FPE pipeline. The following subsystems are **always fail-open regardless of `fail_mode`**:

- **Image pipeline** — NSFW, face, and OCR inference failures always pass the original image through. There is no `fail_mode = "closed"` equivalent for image content.
- **NER model load/inference** — falls back to regex-only; plaintext entity names (person, location, organization) may reach the LLM.
- **Voice KWS** — audio passes through unscanned if models are absent or fail.
- **R2 cognitive firewall** — response integrity is always advisory; manipulation warnings are injected into the response but never block it.
- **L1 plugin** — all L1 redaction is fail-open; no `fail_mode` concept exists in the TypeScript layer.

Operators relying on `fail_mode = "closed"` for text PII should also consider whether image-embedded PII (via OCR) or entity names (via NER) are in scope for their threat model.

---

### HIPAA considerations

HIPAA's Privacy Rule (45 CFR Part 164) requires covered entities and business associates to protect Protected Health Information (PHI). When AI agents process records that contain PHI:

**Under `fail_mode = "open"` (default):**

FPE engine failures allow PHI to reach the LLM provider in plaintext. If the LLM provider is not a HIPAA Business Associate with a signed BAA, this constitutes unauthorized disclosure of PHI under the Privacy Rule. The disclosure is detectable via `fpe_unprotected_total` and the WARN log, but the data has already been transmitted.

OpenObscure's health keyword detection (~350 terms across 9 languages) and NER-based entity detection reduce the likelihood that health-related PII passes undetected. But detection coverage is not complete, and fail-open means detection success alone is insufficient — the encryption step can also fail.

**Under `fail_mode = "closed"`:**

PHI that cannot be encrypted is replaced with `[REDACTED:{type}]` or the request is blocked (502). The LLM provider never receives plaintext PHI from a failed span, but the agent workflow is interrupted. This is the appropriate mode for non-critical workflows where data integrity matters more than availability.

**HIPAA Safe Harbor de-identification (45 CFR §164.514(b)):**

FF1 FPE is pseudonymization, not de-identification. Pseudonymized data remains PHI under HIPAA because it can be re-identified with the FPE key. Safe Harbor de-identification requires removal or transformation of 18 specific identifiers. OpenObscure covers many of these (SSN, phone, email, account numbers, geographic coordinates, IP addresses) but does not cover all 18 — dates other than year, geographic subdivisions smaller than state not expressed as coordinates, and biometric data in audio/video are not fully addressed. Do not rely on OpenObscure alone to meet Safe Harbor requirements.

**Recommended posture for HIPAA-regulated workloads:**

Use `fail_mode = "closed"` for any workflow where the AI agent may encounter PHI. Ensure the LLM provider has a signed BAA regardless — FPE reduces what PHI the provider sees, but OpenObscure's fail-open subsystems (image pipeline, NER, voice) mean PHI may still reach the provider in some cases.

---

### GDPR considerations

GDPR (EU 2016/679) applies to processing of personal data about EU data subjects. Article 32 requires "appropriate technical and organisational measures" to ensure a level of security appropriate to the risk, including pseudonymization and encryption. Article 25 requires data protection by design and by default.

**Under `fail_mode = "open"` (default):**

FPE failures allow personal data to reach the LLM provider without transformation. If the LLM provider is not covered by an appropriate Data Processing Agreement (DPA), or if the provider is outside the EU/EEA without an adequacy decision or Standard Contractual Clauses, this may constitute unauthorized transfer or processing of personal data.

GDPR Article 33/34 breach notification obligations may be triggered if fail-open events result in personal data reaching parties not covered by the organization's legal basis for processing. The `fpe_unprotected_total` counter and `x-openobscure-pii-unprotected` header provide detection, but notification obligations apply to the event itself, not just to detected events.

**Under `fail_mode = "closed"`:**

Processing is interrupted when data cannot be safely transformed. This is more aligned with Article 25(2) ("data protection by default" — process no more data than necessary), but creates availability impact. The 502 response allows the agent to handle the error at the application layer.

**Pseudonymization under GDPR:**

FF1 FPE constitutes pseudonymization within the meaning of GDPR Article 4(5) and Recital 26. Pseudonymized data remains personal data under GDPR as long as the key is held by the same controller. OpenObscure stores the FPE key on the operator's device (OS keychain or environment variable). The LLM provider receives pseudonymized data (ciphertext) and cannot re-identify it without the key. This supports a legitimate interest or consent basis for processing but does not eliminate GDPR obligations.

**Audit log:**

GDPR Article 30 (Records of Processing Activities) benefits from a durable audit trail. Enable the OpenObscure audit log:

```toml
[logging]
audit_log_path = "/var/log/openobscure/audit.jsonl"
```

This produces an append-only JSONL log of PII detection events (type and count, never plaintext values) that can feed a ROPA or support data subject access requests.

**Recommended posture for GDPR-regulated workloads:**

Use `fail_mode = "closed"` to implement privacy by default. Enable the audit log. Ensure DPAs are in place with the LLM provider. Note that NER-based entity detection (names, locations, organizations) may fail silently if the NER model is unavailable — consider `fail_mode = "closed"` an incomplete control for unstructured personal data.

---

### PCI-DSS considerations

PCI-DSS v4.0 Requirement 3 (Protect Stored Account Data) and Requirement 4 (Protect Cardholder Data with Strong Cryptography During Transmission) are most relevant when AI agents process Primary Account Numbers (PANs).

OpenObscure applies Luhn checksum validation before encrypting credit card matches, which means syntactically valid card numbers are reliably detected before reaching the LLM. Truncated or masked numbers (e.g., `4111 **** **** 1111`) may not match the regex pattern and would pass through unmodified.

**Under `fail_mode = "open"` (default):**

If FPE fails for a card number span, the full PAN is forwarded to the LLM provider. Transmitting a PAN outside the Cardholder Data Environment (CDE) to an untrusted third party (the LLM provider) constitutes a PCI-DSS violation under Requirement 4. The LLM provider is almost certainly not a PCI-compliant payment processor.

**Under `fail_mode = "closed"`:**

The PAN span is replaced with `[REDACTED:credit_card]` on encryption failure. The LLM provider never receives a PAN from a failed span. The request to the agent fails (502), which may interrupt a payment workflow.

PCI-DSS Requirement 10 (Log and Monitor All Access) is supported by `fpe_unprotected_total` and the WARN logs. Set up alerting on `fpe_unprotected_total > 0` at the health endpoint:

```bash
# Health endpoint includes fpe_unprotected_total
# Add -H "X-OpenObscure-Token: $OPENOBSCURE_AUTH_TOKEN" if auth token is configured
curl -s http://127.0.0.1:18790/_openobscure/health | jq '.fpe_unprotected_total'
```

**Recommended posture for PCI-DSS in-scope workloads:**

Use `fail_mode = "closed"` for any agent that may encounter PANs. Treat the LLM provider as out-of-scope for the CDE regardless — OpenObscure reduces PAN exposure but does not make a third-party LLM provider PCI-compliant. Consider whether the AI agent workflow itself needs to be scoped into the CDE assessment.

---

### Decision matrix

| Deployment context | Recommended `fail_mode` | Key rationale |
|-------------------|------------------------|---------------|
| HIPAA — non-critical workflow (scheduling, documentation) | `closed` | PHI must not reach LLM on errors; workflow interruption acceptable |
| HIPAA — critical care path (real-time clinical decision support) | `open` + alerting on `fpe_unprotected_total` | Availability constraint; accept monitored risk; ensure BAA covers provider |
| GDPR — personal data processing (EU data subjects) | `closed` | Privacy by default (Art. 25); interruption preferable to unauthorized transfer |
| PCI-DSS — PAN in scope | `closed` | PAN must never leave CDE to untrusted party; 502 acceptable for failed transactions |
| Internal tools, non-regulated data | `open` (default) | Availability priority; transient errors don't block workflow |
| Development and testing | `open` (default) | Avoid interruptions from missing keys or infrastructure issues |
| High-availability regulated service | `open` + `fail_mode = "closed"` for specific PII types via `[fpe.type_overrides]` | Selective hardening — close only the types in scope; see [FPE Configuration](../configure/fpe-configuration.md) |

---

### Monitoring fail-open events

Regardless of which mode you choose, instrument `fpe_unprotected_total` before going to production:

```bash
# Check current unprotected count
# Add -H "X-OpenObscure-Token: $OPENOBSCURE_AUTH_TOKEN" if auth token is configured
curl -s http://127.0.0.1:18790/_openobscure/health | jq '.fpe_unprotected_total'
```

Any non-zero value in `fail_mode = "open"` means plaintext PII reached the LLM in at least one request since last restart. The WARN log identifies the PII type and error:

```
WARN  body  FPE encryption failed, plaintext PII forwarded (fail-open)  type=credit_card  error=vault unavailable
```

For `fail_mode = "closed"`, the same counter tracks destructive redactions (`[REDACTED:{type}]`) — these are not data disclosures, but they indicate FPE reliability issues that deserve investigation.

---

## Host Agent Compatibility Requirements

The L1 TypeScript plugin integrates with the host agent through a hook-based API. Whether the plugin works at all, and how much protection it provides, depends entirely on what that API exposes. The reference integration is **OpenClaw** — a separate desktop AI agent project; it is the only framework L1 has been tested against. Before deploying L1 with any other framework, verify the following.

---

### What L1 requires from the host agent

The plugin's `register(api)` function registers two hooks:

| Hook | Signature | Classification | Status in OpenClaw |
|------|-----------|----------------|--------------------|
| `tool_result_persist` | `(result: ToolResult) => ToolResult` | **Mandatory** | Wired and tested |
| `before_tool_call` | `(call: ToolCall) => ToolCall \| null` | Optional (upgrades to hard enforcement) | Defined in types; not yet invoked |

Both hooks **must be invoked synchronously** by the host agent. The handler receives a value, modifies it, and returns the modified value. The framework is expected to use the returned value — not the original — for the next step in its pipeline.

This synchronous contract is not incidental. `redactPiiWithNer()`, the NER-enhanced redaction path, calls the L0 Core proxy's `/ner` endpoint via `execFileSync("curl", ...)`. A synchronous I/O call inside an async handler would return a `Promise` instead of a `ToolResult`, and any framework that does not explicitly `await` that return value would silently discard the redacted result.

### `tool_result_persist`: what breaks if the hook is async-only

`tool_result_persist` fires after a tool executes and before its output is written to the agent's conversation transcript. If a framework invokes the handler but ignores non-`Promise` return values (i.e., requires an async handler), the registration call succeeds silently, the handler is called, but the framework uses the original, unredacted `ToolResult` rather than the returned redacted copy.

There is no error, no warning, and no indication that redaction was skipped. From L1's perspective, the hook appears to work — log lines show `"Plugin registered"` — but PII in tool outputs reaches the transcript and is sent to the LLM unchanged.

**The only L1 protection that still operates in this case** is the heartbeat monitor: it runs on its own async interval loop and does not depend on the synchronous hook contract. The heartbeat continues to report proxy health state, but no redaction is performed.

### `before_tool_call`: optional hard enforcement

`before_tool_call` runs before tool arguments are passed to the tool executor. When wired, it allows L1 to sanitize PII out of tool call arguments before they are sent to external services (web search queries, file path arguments, API parameters). The handler returns the modified `ToolCall`, or `null` to block the call entirely.

This hook is **not yet invoked by OpenClaw** — it is defined in the `PluginAPI` type and L1 registers a handler when the field is present, but OpenClaw does not call it. Agents that expose this hook gain pre-execution enforcement. Agents that do not fall back to `tool_result_persist` only (post-execution, after the tool has already run with unredacted arguments).

The protection difference:

| What `before_tool_call` prevents | What `tool_result_persist` alone does not prevent |
|----------------------------------|---------------------------------------------------|
| PII sent in tool arguments to external services (search queries, API calls) | Tool argument PII reaching external services during execution |
| PII appearing in tool call logs / audit trails | — |
| Downstream tool calls that chain PII from arguments into results | — |

### What to verify before integrating with a new agent framework

1. **Does `tool_result_persist` block synchronously?** — Invoke `register()`, then trigger a tool call. Confirm the handler's return value (not the original input) is what the framework writes to the transcript.

2. **Does returning a plain object (not a `Promise`) work?** — Some frameworks accept either. If the framework automatically wraps the return in `Promise.resolve()` and awaits it, a plain object works fine. If the framework expects and only processes a `Promise`, it will schedule the resolution as a microtask but use the original value for the transcript write that happens synchronously.

3. **Is `before_tool_call` (or an equivalent pre-execution hook) available?** — Check whether the framework exposes a hook on the `api.hooks` object that fires before tool arguments reach the executor. If so, implement it to match the `(call: ToolCall) => ToolCall | null` signature.

4. **Are hooks called once per tool invocation?** — Some frameworks batch tool calls. If the hook is called once for a batch, the `ToolResult.content` string may contain the outputs of multiple tools concatenated. L1's scanner handles this correctly (it scans the full string), but verify that the framework uses the entire returned `content` field, not a slice of it.

5. **Does the framework's plugin loader handle `register()` side effects?** — `register()` starts a `setInterval`-based heartbeat monitor, reads a file from disk, and may call `execFileSync`. Frameworks that sandbox plugin execution (restrict `child_process`, filesystem access, or timers) will silently break one or more of these.

### Summary

| Capability | Requires |
|-----------|----------|
| Tool result PII redaction (L1 core) | `tool_result_persist` called synchronously, return value used |
| NER-enhanced redaction (14 types vs. 5) | Same as above; L0 running; `curl` executable available |
| Pre-execution argument sanitization | `before_tool_call` hook available and called synchronously |
| Proxy health monitoring | `setInterval` available; no hook requirement |
| Full L1 protection | All of the above |

---

## What OpenObscure Does Not Guarantee

Even with `fail_mode = "closed"` and all models loaded, the following are out of scope or known limitations:

- **Complete PHI/PII coverage** — detection engines cover structured types (SSN, credit card, email, phone, national IDs) and some unstructured types (names, locations via NER). Free-form descriptions of sensitive data, unusual PII formats, and PII types not in the current detection model are not guaranteed to be caught.
- **Image-embedded PII** — image pipeline is always fail-open. If OCR or face detection fails, image content passes through unmodified regardless of `fail_mode`.
- **LLM provider compliance** — OpenObscure does not make LLM providers HIPAA-compliant, GDPR-compliant, or PCI-DSS-certified. Appropriate contracts (BAA, DPA, SCCs) with the provider are required independently.
- **Decryption key security** — if the FPE master key is compromised, all pseudonymized data can be re-identified. Key storage security (OS keychain vs. environment variable) is an operator responsibility. See [Secrets Management in SECURITY.md](../../SECURITY.md#secrets-management).
- **Retroactive protection** — LLM conversation history, provider logs, and model training data that pre-dates OpenObscure deployment are not affected.
- **Response content** — OpenObscure decrypts FPE tokens in LLM responses but does not prevent the LLM from generating new PII or disclosing PII that it inferred from context.

---

## Pre-Deployment Checklist

Before serving production traffic:

- [ ] Set `fail_mode` explicitly — do not rely on the default `"open"` for regulated workloads
- [ ] Provision the FPE key in the OS keychain (`--init-key`) or set `OPENOBSCURE_MASTER_KEY` — the proxy refuses to start without a key
- [ ] Download model files for your tier (`./build/download_models.sh <tier>` + `git lfs pull`) — features are silently disabled without them
- [ ] Set model paths in `openobscure.toml` (`nsfw_model_dir`, `face_model_dir`, `ocr_model_dir`, `ner_model_dir`, `ri_model_dir`)
- [ ] Enable the audit log (`logging.audit_log_path`) for any GDPR or HIPAA workload
- [ ] Configure alerting on `fpe_unprotected_total` at the health endpoint
- [ ] Verify the LLM provider relationship (BAA for HIPAA, DPA for GDPR, contractual scope for PCI-DSS)
- [ ] Set `url_allow_localhost_http = false` in `[image]` for production — the default `true` is for testing only
- [ ] Test the fail behavior explicitly — send a request with the FPE key temporarily unavailable and verify the expected mode activates

---

## Next Steps

- [Gateway Quick Start](gateway-quick-start.md) — proxy setup step by step
- [Embedded Quick Start](embedded-quick-start.md) — iOS/Android library setup
- [FPE Configuration](../configure/fpe-configuration.md) — key generation, fail mode, per-type overrides
- [Fail Behavior Reference](../reference/fail-behavior-reference.md) — every subsystem's exact behavior on error
- [SECURITY.md](../../SECURITY.md) — threat model, secrets management, attack surface
