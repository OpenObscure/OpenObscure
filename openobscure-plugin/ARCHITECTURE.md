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
│                                                              │
│   ┌─────────────┐    ┌──────────────────────────────┐       │
│   │  Agent Tool  │───►│  openobscure_file_check tool    │       │
│   │  (file_read) │    │  ┌─────────────────────────┐ │       │
│   │              │    │  │  File Access Guard       │ │       │
│   └─────────────┘    │  │  (deny patterns)         │ │       │
│                       │  └─────────────────────────┘ │       │
│                       └──────────────────────────────┘       │
└──────────────────────────────────────────────────────────────┘
```

## Module Map

```
src/
├── index.ts                Plugin entry point — register(), hook wiring, tool registration, auth token, retention timer
├── redactor.ts             PII Redactor — regex detection with Luhn/SSN validation
├── file-guard.ts           File Access Guard — sensitive file path blocking
├── consent-manager.ts      GDPR Consent Manager — SQLite storage, CRUD, DSAR, retention tables
├── privacy-commands.ts     /privacy slash command handler (status, consent, export, delete, disclosure, retention)
├── memory-governance.ts    Memory Governor — 4-tier retention lifecycle (hot→warm→cold→expired)
├── heartbeat.ts            L1 Heartbeat Monitor — pings L0 health endpoint with auth token
├── cg-log.ts               Unified logging API — cgInfo/cgWarn/cgError/cgDebug/cgAudit + PII scrub
├── types.ts                OpenClaw plugin API type definitions
├── redactor.test.ts        Redactor tests (9 cases)
├── file-guard.test.ts      File guard tests (11 cases)
├── consent-manager.test.ts Consent + DSAR + privacy command tests (28 cases)
├── memory-governance.test.ts Memory governance + retention command tests (17 cases)
├── heartbeat.test.ts       Heartbeat monitor + auth token tests (12 cases)
└── cg-log.test.ts          Logging API tests (17 cases)
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

### File Access Guard (file-guard.ts)

Blocks agent tools from reading sensitive files. Operates on file paths before any I/O occurs.

**Default deny patterns (15+):**

| Category | Patterns |
|----------|----------|
| Environment files | `.env`, `.env.*` |
| SSH keys | `.ssh/id_*`, `.ssh/authorized_keys`, `.ssh/known_hosts` |
| Cloud credentials | `.aws/credentials`, `.aws/config`, `.gcloud/*.json` |
| Generic secrets | `credentials.json`, `secrets.json`, `*.pem`, `*.key` |
| Databases | `*.sqlite`, `*.sqlite3`, `*.db` |
| OpenObscure files | `openobscure*.enc.json` |
| Shell history | `.*_history`, `.bash_history`, `.zsh_history` |
| Package tokens | `.npmrc`, `.pypirc` |

**Configurable:**
- `extraDenyPatterns`: Add custom deny patterns (regex)
- `allowPatterns`: Explicit overrides to permit specific files (e.g., `test.env`)

Allow patterns are checked **before** deny patterns — an explicit allow always wins.

```typescript
checkFileAccess("/home/user/.ssh/id_rsa")
// → { allowed: false, reason: "Matches sensitive file pattern: ..." }

checkFileAccess("/project/src/main.ts")
// → { allowed: true }
```

### GDPR Consent Manager (consent-manager.ts)

Tracks user consent for data processing per GDPR Articles 13/14. SQLite-backed (single file, embedded).

| Feature | Detail |
|---------|--------|
| **Consent types** | `processing`, `storage`, `transfer`, `ai_disclosure` |
| **Legal bases** | `consent`, `legitimate_interest`, `contract` |
| **DSAR support** | Access, rectification, erasure, portability requests |
| **Processing log** | Timestamped audit trail of all data operations (scan, encrypt, redact, store, delete) |

**Slash commands** (via `openobscure_privacy` tool):

| Command | Action |
|---------|--------|
| `/privacy status` | Show current consent state and data summary |
| `/privacy consent grant` | Grant consent for data processing |
| `/privacy consent revoke` | Revoke consent (stops non-essential processing) |
| `/privacy export` | Export all personal data (DSAR - access) |
| `/privacy delete` | Request data erasure (DSAR - erasure) |
| `/privacy disclosure` | Show AI model disclosure (Art. 13/14) |
| `/privacy retention status` | Show retention tier counts (hot/warm/cold/expired) |
| `/privacy retention enforce` | Run tier promotion + pruning now |
| `/privacy retention policy` | Show current retention policy |

### Heartbeat Monitor (heartbeat.ts)

Pings L0's `/_openobscure/health` endpoint to detect outages.

| State | User Impact |
|-------|-------------|
| **active** | Silent — no notification |
| **degraded** | Warning: "OpenObscure proxy is not responding — PII protection is disabled" |
| **recovering** | Log: "OpenObscure proxy recovered" |
| **disabled** | "OpenObscure is not enabled. PII will be sent in plaintext." |

**Auth token:** Reads `~/.openobscure/.auth-token` (written by L0 on startup) and sends it as `X-OpenObscure-Token` header with every health check. Without a valid token, L0 returns 401 and the monitor transitions to `degraded`.

### Unified Logging API (cg-log.ts)

All logging goes through a unified facade — no direct `console.*` calls outside this module. Every log line passes through PII scrubbing before output.

| Function | Level | Purpose |
|----------|-------|---------|
| `cgInfo(module, message, data?)` | INFO | General operational messages |
| `cgWarn(module, message, data?)` | WARN | Non-fatal issues (L0 unreachable, config fallback) |
| `cgError(module, message, data?)` | ERROR | Failures requiring attention |
| `cgDebug(module, message, data?)` | DEBUG | Detailed diagnostic output |
| `cgAudit(module, message, data?)` | AUDIT | GDPR audit trail (routed to separate JSONL file) |

**Module constants:** `REDACTOR`, `FILE_GUARD`, `CONSENT`, `PRIVACY`, `HEARTBEAT`, `PLUGIN` — prevent typos in log module tags.

**PII scrubbing:** All string fields run through `redactPii()` before output, ensuring no PII leaks through log messages even if developers forget to sanitize.

### Memory Governance (memory-governance.ts)

Manages data retention lifecycle to comply with GDPR Art. 5(1)(e) storage limitation.

| Tier | Retention | Description |
|------|-----------|-------------|
| hot | 7 days | Active conversation data |
| warm | 30 days | Recent but inactive |
| cold | 90 days | Archive before deletion |
| expired | 0 | Immediate deletion candidate |

The `MemoryGovernor` runs periodic enforcement (default: 1 hour interval) that promotes entries through tiers based on age and prunes expired entries. Retention policies are configurable via `RetentionPolicy` interface.

```typescript
const governor = new MemoryGovernor(consentManager, {
  hotDays: 7, warmDays: 30, coldDays: 90
});
const { promoted, pruned } = governor.enforce();
```

### Plugin Registration (index.ts)

```typescript
import { register } from "openobscure-plugin";

register(api, {
  redactToolResults: true,
  fileGuard: true,
  consentManager: true,
  heartbeat: true,
  memoryGovernance: true,
});
```

Registers five things with the host agent:

1. **`tool_result_persist` hook** — Called synchronously after every tool execution. Scans the result text with the PII Redactor and replaces matches before persistence.

2. **`openobscure_file_check` tool** — Registered via `registerTool`. Other tools can call it to pre-check file paths before reading. Returns `{ allowed, reason }`.

3. **`openobscure_privacy` tool** — GDPR consent manager slash commands (status, consent grant/revoke, export, delete, disclosure).

4. **Heartbeat monitor** — Background interval that pings L0 health with auth token, warns user on L0 failure, logs recovery.

5. **Retention enforcement timer** — Background interval (1 hour) that promotes retention tiers and prunes expired entries.

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
| `file-guard` | 11 | .env files, SSH keys, AWS creds, credentials.json, sqlite, OpenObscure enc files, regular files allowed, custom deny, allow overrides, Windows paths |
| `consent-manager` | 17 | Grant/revoke consent, version bumping, active checks, user isolation, processing log, DSAR requests, composite operations (status, export, delete) |
| `privacy-commands` | 10 | All /privacy subcommands, error handling, help text, granted consent display |
| `ai-disclosure` | 1 | Disclosure text generation with model/provider names |
| `memory-governance` | 17 | Tier promotions (hot→warm→cold→expired), pruning, retention summary, custom policy, privacy retention commands, idempotent enforce |
| `heartbeat` | 12 | Initial state, healthy check, degraded transition, consecutive failures, recovery flow, stop/disabled, non-200 status, auth token sent, missing auth → 401/degraded, lastHealth preservation |
| `state-messages` | 2 | Message content for each state, active silence |
| `cg-log` | 17 | Logging facade, PII scrubbing in logs, JSON/plain output, audit routing, module constants |
| **Total** | **96** | |

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
| SQLite | better-sqlite3 | Consent DB — synchronous, embedded, zero-config |
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
import { redactPii, checkFileAccess, ConsentManager } from "openobscure-plugin/core";
```

This exports all core logic (PII redaction, file access guard, consent management,
memory governance, health monitoring, logging) without any agent framework wiring.
The `register()` function and OpenClaw-specific tool definitions remain available
via the default entry point (`openobscure-plugin`).

## Future Work

- **Consent enforcement hooks:** Block tool execution if user hasn't consented to data processing
- **Vector embedding PII scan:** Scan vector embeddings for PII before storage
- **Streaming redaction:** Handle streamed tool results (e.g., large file reads) incrementally
- **NER in redactor:** Add TinyBERT semantic detection alongside regex in L1 redaction (currently regex-only; L0 has hybrid scanner)
