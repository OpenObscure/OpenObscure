# L1 Plugin Architecture

> **Role in OpenObscure:** The gateway plugin is the **second line of defense**. While L0 (Rust proxy) handles outbound PII encryption, L1 catches PII that enters through **tool results** — web scraping, file reads, API responses, and other agent tool outputs that bypass the proxy entirely. For the full system context, see [System Overview](system-overview.md).

---

```mermaid
graph TD
    subgraph Gateway ["AI Agent Gateway (Node.js)"]
        direction TB
        AgentTool["Agent Tool (Web, File, API)"]
        subgraph HookContainer ["tool_result_persist hook"]
            direction TB
            Redactor["OpenObscure PII Redactor (Native / Regex + NER via L0)"]
        end
    end

    AgentTool ==> HookContainer

    style Gateway fill:#f2f5f7,stroke:#232F3E,stroke-width:2px,color:#232F3E
    style HookContainer fill:#e6f3f7,stroke:#9D7BED,stroke-dasharray: 5 5,color:#232F3E
    style AgentTool fill:#3F4756,stroke:#545b64,color:#fff
    style Redactor fill:#9D7BED,stroke:#232F3E,color:#fff
```

**Contents**

- [Module Map](#module-map)
- [Components](#components)
- [Hook Design: tool_result_persist](#hook-design-tool_result_persist)
- [Error Handling](#error-handling)
- [Resource Budget](#resource-budget)
- [Technology Stack](#technology-stack)
- [Relationship to L0 (Rust Proxy)](#relationship-to-l0-rust-proxy)
- [Generic Usage (without OpenClaw)](#generic-usage-without-openclaw)

## Module Map

```
src/
├── index.ts                Plugin entry point — register(), hook wiring, tool registration, auth token, before_tool_call
├── core.ts                 Agent-agnostic API — exports redactPii, redactPiiWithNer, cognitive firewall, logging
├── redactor.ts             PII Redactor — regex detection + NER-enhanced redaction via L0 endpoint
├── cognitive.ts            Cognitive Firewall — JS persuasion dictionary (7 Cialdini categories, 248 phrases, 3→2→1 scanning, severity tiers, warning labels)
├── heartbeat.ts            L1 Heartbeat Monitor — pings L0 health endpoint with auth token
├── oo-log.ts               Unified logging API — ooInfo/ooWarn/ooError/ooDebug/ooAudit + PII scrub
├── types.ts                OpenClaw plugin API type definitions
├── redactor.test.ts        Redactor tests (regex + NER-enhanced)
├── cognitive.test.ts       Cognitive firewall tests (parity, tokenizer, categories, severity, labels, edge cases — 59 tests)
├── heartbeat.test.ts       Heartbeat monitor + auth token tests
├── oo-log.test.ts          Logging API + PII scrubbing + audit log + module constants tests
└── before-tool-call.test.ts  Prepared before_tool_call handler tests
```

## Components

### PII Redactor (redactor.ts)

Scans tool result text for PII and replaces matches with `[REDACTED-*]` labels. Automatically selects the best available detection engine at module load:

**Detection Engines (auto-detected, highest priority first):**

| Engine | When Used | PII Types | Latency |
|--------|-----------|-----------|---------|
| **Native addon** (`@openobscure/scanner-napi`) | Addon installed | 15 (regex + keywords + NER) | <5ms |
| **L0 NER endpoint** (`POST /_openobscure/ner`) | L0 proxy running, no addon | 15 (semantic NER + regex merge) | ~15ms (HTTP) |
| **JS regex** fallback | Neither available | 5 (CC, SSN, phone, email, API key) | ~0ms |

The native addon wraps the same Rust HybridScanner that powers L0. Auto-detection happens once at module load via `require("@openobscure/scanner-napi")`. If the require fails, falls back silently.

**NER model auto-discovery:** When the native addon is loaded, the redactor looks for NER model files at `../openobscure-proxy/models/ner/` relative to the addon's install location. If found, enables NER (person, location, org detection) for 15-type coverage.

**JS Regex Fallback (5 types):**

| PII Type | Pattern | Post-Validation |
|----------|---------|-----------------|
| Credit Card | 13-19 digits with optional separators | Luhn check (rejects invalid) |
| SSN | `NNN-NN-NNNN` | Area validation (no 000/666/900+) |
| Phone | 10+ digits with separators or `+` prefix | Separator/`+` required |
| Email | RFC-like `local@domain.tld` | None |
| API Key | Known prefixes: `sk-`, `AKIA`, `ghp_`, etc. | Prefix match |

**Key difference from L0:** L1 uses **redaction** (`[REDACTED-*]`), not FPE encryption. Tool results are internal and don't need format-preserving properties.

```typescript
const result = redactPii("My SSN is 123-45-6789");
// result.text → "My SSN is [REDACTED-SSN]"
// result.count → 1
// result.types → { ssn: 1 }
```

### Heartbeat Monitor (heartbeat.ts)

Pings L0's `/_openobscure/health` endpoint to detect outages.

| State | User Impact |
|-------|-------------|
| **active** | Silent — no notification |
| **degraded** | Warning: "OpenObscure proxy is not responding — PII protection is disabled" |
| **recovering** | Log: "OpenObscure proxy recovered" |
| **disabled** | "OpenObscure is not enabled. PII will be sent in plaintext." |

**Auth token:** Reads `~/.openobscure/.auth-token` (written by L0 on startup) and sends it as `X-OpenObscure-Token` header with every health check. Without a valid token, L0 returns 401 and the monitor transitions to `degraded`.

### Unified Logging API (oo-log.ts)

All logging goes through a unified facade — no direct `console.*` calls outside this module. Every log line passes through PII scrubbing before output.

| Function | Level | Purpose |
|----------|-------|---------|
| `ooInfo(module, message, data?)` | INFO | General operational messages |
| `ooWarn(module, message, data?)` | WARN | Non-fatal issues (L0 unreachable, config fallback) |
| `ooError(module, message, data?)` | ERROR | Failures requiring attention |
| `ooDebug(module, message, data?)` | DEBUG | Detailed diagnostic output |
| `ooAudit(module, message, data?)` | AUDIT | GDPR audit trail (routed to separate JSONL file) |

**Module constants:** `REDACTOR`, `HEARTBEAT`, `PLUGIN` — prevent typos in log module tags.

**PII scrubbing:** All string fields run through `redactPii()` before output, ensuring no PII leaks through log messages even if developers forget to sanitize.

### Cognitive Firewall (cognitive.ts)

Embedded JS persuasion/manipulation scanner mirroring the L0 Rust `persuasion_dict.rs` + `response_integrity.rs` logic. Provides L1-level response integrity scanning without requiring the L0 proxy.

| Aspect | Detail |
|--------|--------|
| **Categories** | 7 Cialdini categories: Urgency, Scarcity, SocialProof, Fear, Authority, Commercial, Flattery |
| **Phrases** | 248 total (pinned to exact Rust parity per category) |
| **Scanning** | 3→2→1 word window (longest match first, overlap dedup via byte offsets) |
| **Severity** | Notice (1 cat, ≤2 matches), Warning (2+ cats or 3+ matches), Caution (4+ cats or Commercial+Fear/Urgency) |
| **Warning labels** | `--- OpenObscure WARNING ---` format matching L0 output exactly |
| **NAPI bridge** | `scan_persuasion()` free function wraps Rust `PersuasionDict` for native speed when NAPI addon installed |

```typescript
import { scanPersuasion } from "openobscure-plugin/core";
const result = scanPersuasion("Act now! This exclusive offer expires soon.");
// result.severity → "Caution", result.categories → ["Urgency", "Scarcity", "Commercial"]
```

### Plugin Registration (index.ts)

```typescript
import { register } from "openobscure-plugin";

register(api, {
  redactToolResults: true,
  heartbeat: true,
});
```

Registers two things with the host agent:

1. **`tool_result_persist` hook** — Called synchronously after every tool execution. Scans the result text with the PII Redactor and replaces matches before persistence.

2. **Heartbeat monitor** — Background interval that pings L0 health with auth token, warns user on L0 failure, logs recovery.

## Hook Design: tool_result_persist

The `tool_result_persist` hook is **synchronous** (no async/Promise). This is critical because:
- OpenClaw persists tool results immediately after hook return
- An async hook would allow PII to be persisted before redaction completes
- The regex-based redactor is fast enough for synchronous execution (~1ms for typical tool results)

```
Tool executes → tool_result_persist fires → PII Redactor scans → redacted result persisted
                                                 │
                                                 └─ Synchronous, blocking
```

## Error Handling

### `tool_result_persist` hook throws an exception

The hook body (`index.ts:94–119`) has **no try/catch**. If any synchronous call inside it raises an uncaught exception — for example, a crash in `redactPii()` or an unexpected `JSON.parse` failure — the exception propagates out of the hook function into OpenClaw's hook dispatcher. Whether the request proceeds, is dropped, or the process crashes depends entirely on how the host agent handles hook exceptions. L1 provides no fallback of its own.

The most likely exception source is an NAPI addon crash (see below). Pure-JS paths (`redactPiiJs`, `callNerEndpoint`) do not throw outside of try/catch.

### NER endpoint returns a non-200 response

`callNerEndpoint()` (`redactor.ts:297–328`) calls curl via `execFileSync` with `--fail`. The `--fail` flag causes curl to exit with a non-zero code on any non-2xx response, which causes `execFileSync` to throw. The outer `try/catch` catches all throws and returns `null`.

Back in `redactPiiWithNer()`, a `null` NER result means the semantic pass (person, location, org, etc.) is silently skipped. JS regex redaction (`redactPii()`) runs on the full text instead. **No log is emitted.** Coverage falls from 15 types to 5 structured types (or 15 if the native addon is installed, since `redactPiiWithNer` short-circuits to `redactViaNative` when the addon is present).

This same path applies to: connection refused, timeout, HTTP 401/400/413, and malformed JSON in the response body.

### NAPI addon crashes at runtime

The initial `require("@openobscure/scanner-napi")` at module load (`redactor.ts:44–48`) is wrapped in a try/catch — a failed load falls back to JS regex with no error. However, a runtime crash inside `NativeScanner.scanText()` is **not caught**. The exception propagates through `redactViaNative()` → `redactPii()` → the hook handler, which also has no try/catch (see above).

After a runtime crash the `_nativeInstance` singleton remains set. Every subsequent call to `redactPii()` will attempt `redactViaNative()` again and throw again. There is no automatic fallback to `redactPiiJs()` after a runtime native failure.

### Heartbeat auth token is invalid

`fetchHealth()` (`heartbeat.ts:166–206`) treats any non-200 status code — including L0's HTTP 401 for an invalid or missing token — as a rejection: `reject(new Error("Health check returned 401"))`. The `check()` method catches this, increments `_consecutiveFailures`, and transitions state to `degraded` (if currently `active`, `recovering`, or `passthrough`). The default state-change handler logs:

```
ERROR  heartbeat  OpenObscure proxy is not responding — LLM requests will fail.
```

There is **no distinction** between "L0 is unreachable" and "auth token is wrong" — both produce non-200 responses and both result in `degraded` state.

**Effect on redaction:** The `tool_result_persist` hook gates NER usage on `monitor?.state === "active"` (`index.ts:97`). With the monitor in `degraded`, `useNer` is `false` and `redactPii()` runs instead of `redactPiiWithNer()`. PII is still redacted — using the native addon if installed, or JS regex (5 types) otherwise — just without the L0 NER semantic pass.

Recovery: once the token is corrected and L0 starts returning 200, the next heartbeat tick transitions through `recovering` → `active` and NER-enhanced redaction resumes automatically.

---

| Suite | What's Covered |
|-------|----------------|
| `PII Redactor` | SSN, CC (Luhn valid/invalid), email, phone, API key, multiple PII, invalid SSN areas, clean text |
| `NER-Enhanced Redaction` | NER type labels cover all L0 PII types, NER-enhanced redaction merge |
| `HeartbeatMonitor` | Initial state, healthy check, degraded transition, consecutive failures, recovery, auth token |
| `STATE_MESSAGES` | Message content for each state, active silence |
| `ooLog` | Logging facade, PII scrubbing, JSON/plain output |
| `PII scrubbing` | Defense-in-depth PII scrub in all log string fields |
| `GDPR audit log` | Audit routing to separate JSONL file |
| `OO_MODULES constants` | Module constant values and coverage |
| `before_tool_call handler` | Prepared handler registration, feature check, fallback behavior |
| `Persuasion Dictionary` | Total phrase count pinned to 248 (Rust parity) |
| `Per-Category Phrase Count Parity` | 7 per-category count assertions + sum check (Tier 1a) |
| `Cognitive Edge Cases` | Unicode, long text, HTML tags, newlines, smart quotes, repeated whitespace |
| `Severity Boundaries` | All boundary conditions (Notice/Warning/Caution thresholds, combo overrides) |
| `Warning Label Exact Format` | Notice/Warning/Caution string match, SocialProof display name |
| `scanPersuasion` | Clean text, persuasive text, Caution-level, empty string |
| **Total** | **112 tests across 22 suites** |

## Resource Budget

| Metric | Target | Actual |
|--------|--------|--------|
| RAM (resident) | ~25MB | Part of OpenClaw Node.js process |
| Storage | ~3MB | Source + compiled JS |
| Latency (redaction) | <5ms | ~1ms for typical tool results |

## Technology Stack

| Component | Choice | Why |
|-----------|--------|-----|
| Language | TypeScript 5.4 | Compatible with host agent runtime |
| Module system | CommonJS | Compatible with OpenClaw plugin loader |
| Native scanner | `@openobscure/scanner-napi` (optional) | 15-type Rust HybridScanner via napi-rs |
| Testing | node:test + node:assert | Zero-dependency, built into Node.js |
| Test runner | tsx | TypeScript execution without pre-compilation |

## Relationship to L0 (Rust Proxy)

> **Comparison table:** See [L0 vs L1 — When to Use Which](system-overview.md#two-layer-defense-in-depth) in the
> System Overview.

Together, L0 and L1 form a **defense-in-depth** strategy: L0 encrypts PII in transit to LLMs, L1 redacts PII from local tool operations.

## Generic Usage (without OpenClaw)

For agent-agnostic access to OpenObscure's privacy functions, use the core entry point:

```typescript
import { redactPii } from "openobscure-plugin/core";
```

This exports core logic (PII redaction, health monitoring, logging) without any agent framework wiring.
The `register()` function and OpenClaw-specific tool definitions remain available
via the default entry point (`openobscure-plugin`).
