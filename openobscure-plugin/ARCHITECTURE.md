# OpenObscure Plugin — Architecture

> Layer 1 of the OpenObscure privacy firewall. See `../project-plan/MASTER_PLAN.md` for full system architecture.

---

## Role in OpenObscure

The gateway plugin is the **second line of defense**. While L0 (Rust proxy) handles outbound PII encryption, L1 catches PII that enters through **tool results** — web scraping, file reads, API responses, and other agent tool outputs that bypass the proxy entirely.

```
┌──────────────────────────────────────────────────────────────┐
│  AI Agent Gateway (e.g., OpenClaw) (Node.js)                  │
│                                                              │
│   ┌─────────────┐    ┌──────────────────────────────┐       │
│   │  Agent Tool  │───►│  tool_result_persist hook     │       │
│   │  (web, file, │    │  ┌─────────────────────────┐ │       │
│   │   API, etc.) │    │  │  OpenObscure PII Redactor  │ │       │
│   └─────────────┘    │  │  (regex + Luhn + SSN)    │ │       │
│                       │  └─────────────────────────┘ │       │
│                       └──────────────────────────────┘       │
└──────────────────────────────────────────────────────────────┘
```

## Module Map

```
src/
├── index.ts                Plugin entry point — register(), hook wiring, tool registration, auth token
├── redactor.ts             PII Redactor — regex detection with Luhn/SSN validation
├── heartbeat.ts            L1 Heartbeat Monitor — pings L0 health endpoint with auth token
├── oo-log.ts               Unified logging API — ooInfo/ooWarn/ooError/ooDebug/ooAudit + PII scrub
├── types.ts                OpenClaw plugin API type definitions
├── redactor.test.ts        Redactor tests (9 cases)
├── heartbeat.test.ts       Heartbeat monitor + auth token tests (12 cases)
└── oo-log.test.ts          Logging API tests (17 cases)
```

## Components

### PII Redactor (redactor.ts)

Scans tool result text for PII and replaces matches with `[REDACTED]` placeholders. Uses the same patterns as L0 proxy for consistency:

| PII Type | Pattern | Post-Validation |
|----------|---------|-----------------|
| Credit Card | 13-19 digits with optional separators | Luhn check (rejects invalid) |
| SSN | `NNN-NN-NNNN` | Area validation (no 000/666/900+) |
| Phone | 10+ digits with separators or `+` prefix | Separator/`+` required |
| Email | RFC-like `local@domain.tld` | None |
| API Key | Known prefixes: `sk-`, `AKIA`, `ghp_`, etc. | Prefix match |

**Key difference from L0:** L1 uses **redaction** (`[REDACTED]`), not FPE encryption. Tool results are internal to OpenClaw and don't need format-preserving properties. Redaction is simpler and guarantees PII removal.

```typescript
const result = redactPii("My SSN is 123-45-6789");
// result.text → "My SSN is [REDACTED]"
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

## Test Coverage

| Module | Tests | What's Covered |
|--------|-------|----------------|
| `redactor` | 9 | SSN, CC (Luhn valid/invalid), email, phone, API key, multiple PII, invalid SSN areas, clean text |
| `heartbeat` | 12 | Initial state, healthy check, degraded transition, consecutive failures, recovery flow, stop/disabled, non-200 status, auth token sent, missing auth -> 401/degraded, lastHealth preservation |
| `state-messages` | 2 | Message content for each state, active silence |
| `oo-log` | 17 | Logging facade, PII scrubbing in logs, JSON/plain output, audit routing, module constants |
| **Total** | **40** | |

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
| Testing | node:test + node:assert | Zero-dependency, built into Node.js |
| Test runner | tsx | TypeScript execution without pre-compilation |

## Relationship to L0 (Rust Proxy)

| Aspect | L0 (Proxy) | L1 (Plugin) |
|--------|------------|-------------|
| **Intercept point** | HTTP requests/responses | Tool results |
| **PII handling** | FPE encryption (format-preserving) | Redaction (`[REDACTED]`) |
| **Reversible?** | Yes (decrypt on response) | No (destructive redaction) |
| **Runs in** | Standalone Rust binary | OpenClaw Node.js process |
| **Catches** | All LLM API traffic | Tool outputs (web, file, API) |

Together, L0 and L1 form a **defense-in-depth** strategy: L0 encrypts PII in transit to LLMs, L1 redacts PII from local tool operations.

## Generic Usage (without OpenClaw)

For agent-agnostic access to OpenObscure's privacy functions, use the core entry point:

```typescript
import { redactPii } from "openobscure-plugin/core";
```

This exports core logic (PII redaction, health monitoring, logging) without any agent framework wiring.
The `register()` function and OpenClaw-specific tool definitions remain available
via the default entry point (`openobscure-plugin`).

## Future Work

- **Streaming redaction:** Handle streamed tool results (e.g., large file reads) incrementally
- **NER in redactor:** Add TinyBERT semantic detection alongside regex in L1 redaction (currently regex-only; L0 has hybrid scanner)
