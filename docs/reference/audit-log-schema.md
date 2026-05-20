# Audit Log Schema

OpenObscure audit logs are append-only JSONL files: each line is one complete JSON object. Configure the Rust core audit log with `logging.audit_log_path`; the TypeScript plugin also writes JSONL audit entries when its audit file is configured.

Audit records are intended for processing history, compliance review, and incident investigation. They must not contain plaintext PII. Store counts, labels, request identifiers, and redaction/encryption status instead of raw matched values.

## Record Shapes

### L0 Core Proxy

The Rust core writes audit records through `tracing_subscriber` JSON formatting. Core audit events are tagged with `oo_audit = true`; the event type is the `operation` field inside `fields`.

```json
{"timestamp":"2026-05-20T10:22:31.144Z","level":"INFO","fields":{"message":"audit","oo_module":"scanner","oo_audit":true,"operation":"scan","request_id":"req_01HX7TQ2R9J4","pii_types":["email","phone"],"match_count":2,"encrypted":true,"redacted":false}}
```

### L1 Plugin

The TypeScript plugin writes a flatter JSONL object. The event type is the top-level `operation` field.

```json
{"ts":"2026-05-20T10:23:04.012Z","module":"openobscure.redactor","operation":"redact","request_id":"req_01HX7TQ2R9J4","pii_types":["email"],"match_count":1,"redacted":true}
```

Consumers should support both shapes if they read logs from both layers.

## Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `timestamp` | string | L0 | RFC 3339 timestamp emitted by the Rust tracing JSON layer. |
| `ts` | string | L1 | ISO 8601 timestamp emitted by the TypeScript plugin audit writer. |
| `level` | string | L0 | Tracing level. Audit events are emitted at `INFO`. |
| `fields.message` | string | L0 | Tracing message. Core audit records use `"audit"`. |
| `fields.oo_audit` | boolean | L0 | Core audit marker. Should be `true` for records in the audit file. |
| `fields.oo_module` | string | L0 | Core module that emitted the audit record, such as `scanner`, `vault`, or `health`. |
| `module` | string | L1 | Plugin module name with `openobscure.` prefix, such as `openobscure.redactor`. |
| `operation` | string | Yes | Event type, such as `scan`, `encrypt`, `redact`, `key_rotation`, `grant`, or `revoke`. In L0 records this is under `fields.operation`; in L1 records it is top-level. |
| `request_id` | string | Optional | Stable request correlation ID when the event is tied to a proxied request. |
| `pii_types` | string[] | Optional | PII labels detected or processed, such as `email`, `phone`, or `api_key`. |
| `pii_count` | number | Optional | Count of PII items processed. Existing code may also use this name for match totals. |
| `match_count` | number | Optional | Count of matched PII spans for a scan/redaction event. |
| `encrypted` | boolean | Optional | Whether the event produced or used encrypted replacement values. |
| `redacted` | boolean | Optional | Whether the event redacted content instead of preserving encrypted structure. |
| `key_id` | string | Optional | Identifier or version label for key-management operations. Do not log key material. |

Additional event-specific fields are allowed, but they must follow the same no-plaintext-PII rule.

## Example Records

PII detection and encryption event from the L0 core:

```json
{"timestamp":"2026-05-20T10:22:31.144Z","level":"INFO","fields":{"message":"audit","oo_module":"scanner","oo_audit":true,"operation":"scan","request_id":"req_01HX7TQ2R9J4","pii_types":["email","phone"],"match_count":2,"encrypted":true,"redacted":false}}
```

Key rotation event from the L0 vault:

```json
{"timestamp":"2026-05-20T10:24:19.771Z","level":"INFO","fields":{"message":"audit","oo_module":"vault","oo_audit":true,"operation":"key_rotation","key_id":"fpe-master-key:v2","encrypted":true}}
```

Plugin redaction event:

```json
{"ts":"2026-05-20T10:25:10.338Z","module":"openobscure.redactor","operation":"redact","request_id":"req_01HX7TQ2R9J4","pii_types":["api_key"],"match_count":1,"redacted":true}
```

## PII Scrubbing

Audit logs are not a place to store raw secrets, prompts, completions, screenshots, transcripts, or matched PII text. Use labels such as `email`, counts such as `match_count`, and request IDs for correlation.

For key-management events, log key identifiers or versions only. Never log FPE master keys, wrapped keys, tokens, plaintext credentials, or before/after PII values.
