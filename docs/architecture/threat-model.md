# Threat Model

This document describes the threats OpenObscure is designed to mitigate, the trust
boundaries it enforces, and the rationale behind key design decisions. Security
researchers evaluating this project should start here.

## Trust Boundaries

```
+---------------------------+          +---------------------------+
|       User Device         |          |      LLM Provider         |
|                           |          |                           |
|  +---------------------+ |          |  Untrusted with PII       |
|  |   Host App          | |          |  - Sees only FPE tokens   |
|  |   (Enchanted,       | |  HTTPS   |  - Cannot reverse FF1     |
|  |    RikkaHub, etc.)  |----------->|    without the key        |
|  |                     | |          |  - Receives sanitized     |
|  |  +---------------+  | |          |    images (faces/text     |
|  |  | OpenObscure   |  | |          |    redacted)              |
|  |  | (Rust core)   |  | |          +---------------------------+
|  |  +---------------+  | |
|  |                     | |          +---------------------------+
|  |  +---------------+  | |          |  Auto-Gen Services        |
|  |  | Local DB      |  | |          |  (title gen, suggestions) |
|  |  | (SwiftData/   |  |----------->|  Untrusted — receives     |
|  |  |  Room)        |  | |          |  isolated disposable      |
|  |  +---------------+  | |          |  tokens, not conversation |
|  +---------------------+ |          |  pool tokens              |
|                           |          +---------------------------+
|  Trusted:                 |
|  - FPE key (Keychain/     |
|    Keystore)              |
|  - ONNX models (bundled)  |
|  - Mapping state          |
+---------------------------+
```

**Trusted:** The user's device, the host app process, the local database, the FPE key
material, and the bundled ONNX models.

**Untrusted:** Any network endpoint that receives data — LLM providers, auto-generated
request services, external image URLs. These never see real PII.

**Semi-trusted:** The host app developer. OpenObscure provides the tools and reference
implementations, but cannot prevent a malicious developer from intentionally bypassing
sanitization. The [What Your App Must Provide](../integrate/embedding/embedded_integration.md#what-your-app-must-provide)
checklist documents the integration points that must be implemented correctly.

## Threat Categories

### T1: PII Exfiltration via LLM Context

**Threat:** Sensitive data (names, SSNs, health records, GPS coordinates) sent to an
LLM provider in the conversation history. The provider stores, logs, or trains on this
data.

**Mitigation:** Format-Preserving Encryption (FF1/AES-256) encrypts structured PII
so the LLM sees format-valid but meaningless ciphertexts. Token-based replacement
handles semantic PII (names, health terms). The LLM operates on tokens without
access to the FPE key.

**Design decision:** FPE over redaction. `[REDACTED]` destroys LLM context and breaks
agentic workflows. FPE-encrypted values (`487-14-6147` for an SSN) maintain the
semantic structure the LLM needs to reason correctly.

### T2: Token Extraction via Adversarial Prompts

**Threat:** A user or injected prompt asks the LLM to "spell out PER_0dpx character
by character" or "encode PER_0dpx in Base64," attempting to extract the token string
in a form that bypasses restore.

**Mitigation:** FPE tokens are opaque — even if the LLM spells out the characters,
the result is `P, E, R, _, 0, d, p, x` which reveals nothing about the original
value without the FPE key. The token *is* the ciphertext. There is no plaintext
hidden inside it.

**Design decision:** We removed `containsAdversarialPrompt()` and
`detect_token_fragmentation()` after analysis showed they solved non-problems. If
FPE encrypt/decrypt works correctly, tokens are harmless regardless of how the LLM
manipulates them. Defense-in-depth at the prompt level added complexity without
security value.

### T3: Plaintext Persistence (The "Auto-Save Trap")

**Threat:** The host app restores FPE tokens to plaintext for UI display, then the
framework's auto-save mechanism (SwiftData `saveChanges()`, Room auto-flush)
persists the restored plaintext to the local database. On the next conversation
turn, the plaintext is read from the database and sent to the LLM.

**Mitigation:** Restore happens only at the UI rendering layer via ephemeral
properties:
- **iOS:** `@Transient displayContent` on SwiftData models — excluded from
  persistence entirely
- **Android:** Compose `rememberRestoredText()` — scoped to a single recomposition,
  never touches StateFlow or Room

The database always contains raw FPE tokens. See
[Architecture: Why @Transient](../integrate/embedding/architecture.md#why-transient-displaycontent-on-ios)
for the detailed explanation.

**Design decision:** This is the most critical integration point. Without it,
every reactive UI framework silently leaks PII. This threat was discovered
empirically during Enchanted integration testing and drove the entire embedded
architecture.

### T4: Cross-Conversation Token Leakage

**Threat:** FPE mappings from conversation A persist when the user switches to
conversation B. If conversation B happens to contain the same token (from a
different FPE session), `restore()` produces incorrect plaintext — or worse,
reveals PII from conversation A in conversation B's context.

**Mitigation:** `resetMappings()` is called on every conversation switch, clearing
the in-memory mapping pool, sanitize cache, and RI warning flags. Mappings are
stored per-conversation via `mappingJson` and loaded fresh on switch.

**Design decision:** This is a hard security boundary, not a convenience feature.
The library cannot enforce it — the host app must call `resetMappings()` at the
right lifecycle point.

### T5: Frequency Analysis of FPE Ciphertexts

**Threat:** An attacker observing multiple encrypted requests notices that the same
SSN always produces the same ciphertext, enabling frequency analysis to correlate
values across sessions.

**Mitigation:** Per-request tweaks generated from `uuid_bytes || sha256(json_path)`.
Each request produces different ciphertexts for the same plaintext. Within a single
multi-turn conversation, `encrypt_match_stable()` ensures the same plaintext maps
to the same token (for LLM coherence), but across sessions the tokens differ.

**Design decision:** The trade-off between intra-session token stability (LLM needs
consistent references) and inter-session uniqueness (frequency analysis resistance)
is managed by the stable mapping registry.

### T6: Image-Borne PII

**Threat:** Images sent to vision-capable LLMs contain faces, text with PII
(screenshots of medical records, IDs), NSFW content, or EXIF metadata with GPS
coordinates and device information.

**Mitigation:** Four-phase on-device pipeline:
1. **EXIF strip** — removed during image decode (implicit in `image` crate)
2. **NSFW classifier** — ViT-base INT8, threshold 0.50; triggers full-image
   solid fill, short-circuiting face/OCR phases
3. **Face detection** — SCRFD-2.5GF (Full/Standard) or Ultra-Light RFB-320
   (Lite); selective solid fill per face bounding box
4. **OCR text detection** — PaddleOCR PP-OCRv4 with inverted band pre-filter
   (density 0.05-0.12 = document text, >0.12 = photo, skip OCR)

**Design decision:** Fail-open on image pipeline errors — if sanitization throws,
the original image is forwarded. A blocked image prevents the user from completing
their task; a leaked face in one image is less harmful than a systematically
broken user experience that drives users to disable the tool entirely.

### T7: LLM Manipulation (Cognitive Firewall)

**Threat:** The LLM's response contains persuasion tactics, social engineering,
or manipulation patterns — urgency language, authority appeals, scarcity framing,
social proof fabrication — that influence the user's decision-making.

**Mitigation:** Two-stage cascade:
- **R1:** Dictionary of ~250 phrases across 7 Cialdini persuasion categories,
  sub-millisecond scan
- **R2:** TinyBERT classifier for 4 EU AI Act Article 5 categories (deceptive
  practices, age-based targeting, socioeconomic targeting, social scoring),
  invoked only when R1 flags content or by sampling

When triggered, a warning label is prepended to the displayed response. The
response is not blocked — the user sees both the warning and the content.

**Design decision:** Warn, don't block. Blocking LLM responses creates a
censorship tool. Warning preserves user agency while surfacing manipulation
patterns that are otherwise invisible.

### T8: Auto-Generated Request Token Pollution

**Threat:** Host apps generate background requests (title generation, suggestion
generation) that go through the same sanitization pipeline. These create fresh
FPE tokens that accumulate in the conversation's mapping pool, bloating memory
and potentially causing mapping collisions.

**Mitigation:** Auto-generated requests are detected (by model name, stream flag,
message count) and routed to `sanitizeMessagesIsolated()`. PII is still sanitized
(the LLM never sees real data), but the tokens are disposable — not merged into
the conversation's accumulated mappings.

**Design decision:** Every outbound request must be sanitized, regardless of
destination. Skipping sanitization for "internal" requests violates the core
principle. Isolated mappings maintain the security guarantee while preventing
pool pollution.

## Explicitly Out of Scope

These threats are **not** mitigated by OpenObscure:

| Threat | Why out of scope |
|--------|-----------------|
| **Compromised device OS** | If the OS is compromised, the attacker has access to the FPE key, the database, and memory. No application-layer defense is meaningful. |
| **Malicious host app developer** | OpenObscure provides tools; it cannot prevent a developer from intentionally exfiltrating PII. The reference implementations show the correct pattern. |
| **LLM model poisoning** | Attacks on the LLM's training data or weights are outside the network boundary. OpenObscure protects the data sent *to* the LLM, not the LLM itself. |
| **Side-channel attacks on FPE** | Timing attacks on the FF1 implementation are theoretical for on-device use. The `fpe` crate uses constant-time AES operations. |
| **PII in model weights** | If the LLM has memorized PII from training data, OpenObscure cannot detect or redact it in responses. The cognitive firewall detects manipulation patterns, not memorized data. |
| **Prompt injection via tool results** | L1 plugin scans tool results for PII, but adversarial tool outputs designed to manipulate the agent's behavior are an agent-framework concern, not a privacy-layer concern. |

## Fail-Open vs Fail-Closed Decisions

| Component | Behavior | Rationale |
|-----------|----------|-----------|
| FPE encryption error | **Fail-open** — skip the match, forward original text | A blocked message breaks the user's workflow. The PII was already in the user's input; failing to encrypt it doesn't create new exposure. |
| Image pipeline error | **Fail-open** — forward original image | Same rationale. The user chose to send the image. |
| NER model load failure | **Fail-open** — degrade to regex-only (10 types vs 15) | Regex scanner catches all structured PII. NER adds semantic types (names, orgs). Degraded coverage is better than no coverage. |
| NSFW detection | **Fail-closed** — solid fill entire image | NSFW content reaching a vision LLM is a higher-severity event than a blocked image. The user can re-send without the image. |
| Cognitive firewall | **Warn, don't block** — prepend warning label | Blocking responses creates censorship. Warning preserves user agency. |
| Key vault unavailable | **Fail-closed** — return HTTP 503 | Without the FPE key, no encryption is possible. Forwarding unencrypted traffic silently would violate the core guarantee. Gateway-only; embedded model stores key locally. |
