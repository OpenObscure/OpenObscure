# OpenObscure: Privacy Firewall for AI Agents — Complete Design Reference

> **Purpose of this document:** This is the single source of truth for the OpenObscure project. Attach this file to any new Claude conversation to resume work without re-deriving decisions. Every section below represents a FINAL DECISION unless marked [OPEN]. Do not re-analyze or re-debate settled decisions — build from them.

> **Last updated:** 2026-02-18
> **Conversation count:** 5 sessions (security analysis → architecture → V2 planning/costs → FPE/face/OCR/NER/final build plan → cross-platform/mobile)

---

## 1. WHAT IS OPENOBSCURE

OpenObscure is a **privacy firewall** for AI agents. Primary integration: [OpenClaw](https://github.com/openclaw/openclaw), the open-source personal AI assistant platform. It prevents personally identifiable information (PII) from leaking to LLM providers, encrypts data at rest, manages GDPR consent, and provides CLI-based compliance tooling.

OpenClaw is a hub-and-spoke architecture with a Gateway Server as the central control plane. It routes messages from channels (WhatsApp, Telegram, Discord, Slack, iMessage, web UI, CLI) through an Agent Runtime that calls LLM providers (Anthropic, OpenAI, etc.) and executes tools (bash, file ops, browser).

**The core problem:** Every message, tool result, and file the user shares with OpenClaw gets sent to third-party LLM APIs in plaintext. This includes credit card numbers, health discussions, API keys, children's information, and biometric data (photos). There is no built-in PII filtering, consent management, or data residency enforcement.

---

## 2. OPENCLAW ARCHITECTURE CONSTRAINTS

These constraints are FIXED and drive all OpenObscure design decisions:

### 2.1 Hook System Limitation (CRITICAL)
OpenClaw defines **14 lifecycle hooks** but only **`tool_result_persist`** has actual invocation sites in the published codebase (confirmed via GitHub Issue #6535). The hooks `before_tool_call`, `message_sending`, `session_start`, etc. are defined in TypeScript types but NEVER called.

**Implication:** An in-process Gateway plugin CANNOT intercept data before it reaches the LLM. It can only modify data before transcript persistence. This is why OpenObscure requires a **sidecar HTTP proxy** — it's the only component that physically controls the connection to LLM providers.

### 2.2 Plugin Architecture
OpenClaw plugins extend the Gateway via 4 slot types: channels, tools, providers, memory. Plugins are TypeScript packages in `extensions/` that export a `register(api)` function. The `registerTool` API is the most powerful surface — it injects logic into the agent's decision flow.

### 2.3 Extension Points Available to OpenObscure
- **`tool_result_persist` hook** — synchronous, modifies tool results before transcript persistence. Must return synchronously (Promise causes silent skip).
- **`registerTool`** — registers custom tools the agent can call (e.g., a security gate tool).
- **Provider plugin** — can intercept/modify LLM API calls IF registered as a custom provider.
- **HTTP proxy** — external process that OpenClaw routes API calls through (ClawShell pattern).

### 2.4 Reference Implementations
- **ClawShell** — Existing Rust proxy that isolates API keys. Proves the sidecar proxy pattern works. ~2,000 lines of Rust.
- **openclaw-shield** (Knostic) — 5-layer security plugin. Demonstrates `tool_result_persist` for PII redaction and `registerTool` for a security gate. Key finding: L2 redacts after LLM sees data (not before), and only 3 of 14 hooks are actually wired.

---

## 3. THREE-LAYER ARCHITECTURE (FINAL)

### Layer 0: Rust PII Proxy (L0) — Hard enforcement
- **Function:** HTTP reverse proxy between OpenClaw Gateway and LLM providers. Intercepts ALL API requests. Strips/encrypts PII BEFORE it reaches the LLM. Decrypts FPE values in responses.
- **Runtime:** Standalone Rust binary. ~12MB RAM, ~8MB storage.
- **Contains:** Regex PII scanner, FF1 FPE engine, credential vault (OS keychain bridge), cross-border routing policy engine, breach monitoring log watcher.
- **Auth:** Passthrough-first — forwards OpenClaw's API keys/auth headers by default. No duplicate key management. Optional `override_auth` per provider for secondary vault keys.
- **Port:** Configurable, default suggested in OpenClaw config via `openclaw config set model`.

### Layer 1: Gateway Plugin (L1) — Classification + policy
- **Function:** TypeScript plugin running in-process. Uses `tool_result_persist` hook + `registerTool`. Classifies data by privacy category, enforces redaction/blocking policies, manages consent, gates file access.
- **Runtime:** Node.js (in-process with Gateway). ~25MB RAM, ~3MB storage.
- **Contains:** PII Redactor (regex), File Access Guard, Consent Manager (SQLite), Memory Governance (tier lifecycle), DSAR/ROPA/DPIA generators (CLI).
- **Upgrade path:** When OpenClaw wires `before_tool_call`, L1 becomes hard enforcement.

### Layer 2: Storage Engine (L2) — Encryption at rest
- **Function:** Encrypts session transcripts, credentials, and memory stores. Implements memory retention tiers and right-to-erasure.
- **Tech:** AES-256-GCM for transcripts, Argon2id for KDF. ~6MB RAM.
- **Contains:** Transcript encryption, credential file encryption, FPE key storage.

*Note: Compliance CLI features (ROPA, DPIA, breach notification, privacy summary) are built into L0 (`openobscure compliance <subcommand>`) and L1 (`/privacy` commands). No separate layer.*

---

## 4. PII RISK REGISTER (15 RISKS, PRIORITIZED)

| ID | Category | Severity | FPE Viable | NER Needed | Final Mitigation |
|----|----------|----------|-----------|-----------|-----------------|
| PII-01 | Chat message content | Critical | Partial (structured) | Yes (semantic) | Regex + FPE (structured) + TinyBERT NER (semantic) |
| PII-02 | Contact information | High | 80% (phone/email) | Yes (names/addresses) | FPE for phone/email + NER for names |
| PII-03 | API keys/tokens | Critical | 85% (prefix-preserving) | No | Credential Vault (primary) + FPE (backup) |
| PII-04 | Financial data | Critical | 95% (CC, SSN, salary) | Partial (semantic refs) | FPE (structured) + health keyword dict |
| PII-05 | Biometric & photos | High | 0% | N/A (visual pipeline) | BlazeFace blur + EXIF strip + OCR+FPE |
| PII-06 | Location data | Medium | Partial (coordinates) | Yes (place names) | FPE (coords) + NER (place names) |
| PII-07 | Session transcripts | High | N/A | N/A | AES-256-GCM encryption at rest |
| PII-08 | Tool results | High | Partial | Yes | L1 `tool_result_persist` hook + regex + NER |
| PII-09 | Provider API metadata | Critical | N/A | N/A | Proxy forwards OpenClaw auth headers (passthrough-first), optional vault override, strips internal headers |
| PII-10 | Children's data | High | 10% (student IDs only) | Yes (detection is the hard part) | NER + child keyword dict (~200 terms) |
| PII-11 | Health data | High | 30% (lab values, MRNs) | Yes (conditions, symptoms) | NER + health keyword dict (~500 terms) + FPE (numeric) |
| PII-12 | Cross-border transfers | High | N/A | N/A | Proxy routing policy (phone country code, email domain, language detection) |
| PII-13 | Memory/RAG content | Medium | Partial | Yes | Memory governance + PII scan on write + vector embedding erasure |
| PII-14 | Credential exposure | Critical | N/A | N/A | Vault + virtual key substitution + `tool_result_persist` redaction |
| PII-15 | Screen captures | High | 25% (OCR text only) | Partial | EXIF strip + face blur + OCR + FPE + sensitive window detection |

---

## 5. FORMAT-PRESERVING ENCRYPTION (FPE) — DECISIONS

### 5.1 Algorithm
- **USE:** FF1 (NIST SP 800-38G, approved)
- **DO NOT USE:** FF3 — **WITHDRAWN** by NIST in SP 800-38G Rev 2 (February 3, 2025) due to Beyne 2021 attack. The `fpe-arena` GitHub repo uses FF3 and is labeled "educational use only."
- **Production libraries:** Capital One Go implementation, python-fpe, Mysto Java/Python, or implement in Rust using AES primitives.

### 5.2 How FPE Works in OpenObscure
FPE transforms plaintext into ciphertext of **identical format**. Credit card 4532-1234-5678-9012 becomes 8714-3927-6051-2483. The LLM sees plausible-looking data instead of `[REDACTED]` placeholders — preserving context without leaking real PII. The proxy decrypts on the response path.

### 5.3 FPE Constraints
- **Minimum domain size:** radix^minLen ≥ 1,000,000. Values <6 digits are unsafe.
- **Deterministic:** Same plaintext + key + tweak = same ciphertext. Use per-record tweaks to prevent frequency analysis.
- **Structured data only:** Operates on finite character sets. Cannot encrypt free-form prose.
- **Key management:** FPE key stored in OS keychain. Key compromise = all FPE data compromised.

### 5.4 Authentication & Key Management (FINAL)
- **Passthrough-first:** OpenObscure forwards all auth headers from OpenClaw to upstream providers unchanged. Users configure API keys once in OpenClaw — no duplicate key management.
- **Optional override:** Per-provider `override_auth = true` injects/replaces the auth header from OpenObscure's OS keychain vault. Use for separate billing accounts or key rotation independent of OpenClaw.
- **Header forwarding:** All original request headers forwarded except hop-by-hop (RFC 7230: Connection, Transfer-Encoding, Host, etc.) and provider-specific `strip_headers`.
- **FPE key:** Separate from API keys. 32-byte AES-256 key in OS keychain. Generated once with `--init-key`. Vault unavailable → 503 (blocking — no privacy without FPE key).

### 5.5 FPE Coverage by PII Category
- Financial (PII-04): **95%** — CC#, SSN, bank accounts, salary figures all FPE-friendly.
- API keys (PII-03): **85%** — Prefix-preserving FPE (encrypt after prefix). Vault is primary defense.
- Contacts (PII-02): **80%** — Phone numbers and emails. Names need NER.
- Health (PII-11): **30%** — Lab values, MRNs. Conditions/symptoms need NER.
- Screen captures (PII-15): **25%** — FPE on text extracted by OCR.
- Children (PII-10): **10%** — Student IDs only. Detection requires NER.
- Biometric (PII-05): **0%** — Binary data, outside FPE domain.

### 5.6 FPE Integration Pattern
- **Pattern A (PRIMARY):** Proxy-Embedded. FPE implemented directly in Rust proxy. Zero network overhead. Transparent to agent. Agent never knows FPE is happening.
- **Pattern B (COMPLEMENTARY):** MCP Tool. Expose `fpe_encrypt`/`fpe_decrypt` as MCP tools. Agent can explicitly encrypt sensitive data before storing. Complementary, not replacement.

---

## 6. NER CLASSIFIER — DECISIONS

### 6.1 Primary: TinyBERT-4L-312D (INT8 Quantized)
- **Storage:** ~15MB (ONNX, INT8)
- **RAM:** ~50-55MB loaded
- **Accuracy:** ~96% of bert-base performance
- **Latency:** ~8-15ms/sentence (CPU), ~2-4ms (GPU/NPU)
- **Integration:** ONNX export via HuggingFace Optimum → shared between Rust proxy (`ort` crate) and Node.js plugin (`onnxruntime-node`)
- **Preparation:** Fine-tune on PII-specific corpus (500-1000 labeled examples) BEFORE quantization. Pre-trained TinyBERT is general NER — needs PII-specific training data.

### 6.2 Fallback: CRF (Conditional Random Field)
- **When:** Device RAM < 200MB free. Auto-detected by installer.
- **Storage:** <5MB
- **RAM:** <50MB
- **Accuracy:** ~80-85% (structured PII), ~60% (contextual)
- **Latency:** Microseconds per token
- **Limitation:** Cannot do contextual disambiguation. "Amazon" always tagged same regardless of context.

### 6.3 Deferred: GLiNER (ONNX Quantized)
- **Status:** Shelved for V2. 200-250MB RAM consumes almost entire budget.
- **When to reconsider:** If TinyBERT accuracy proves insufficient on 16GB+ devices (Tier 3 upgrade).

### 6.4 Hybrid NER Strategy
1. **Regex runs first** (fast, <1ms) — catches structured PII (CC#, SSN, phone, email, API keys).
2. **FPE encrypts** regex-detected fields in-place.
3. **TinyBERT NER runs second** (~10ms) — catches semantic PII (health discussions, contextual names, child references).
4. **Health/child keyword dict** (~500 medical + ~200 child terms) supplements NER for domain-specific detection.

---

## 7. FACE DETECTION — DECISIONS

### 7.1 Tier 1-2: MediaPipe BlazeFace
- **Model:** `blaze_face_short_range.tflite` (~230KB) for selfies/video calls. Full-range (~2.3MB) for general images.
- **RAM:** ~8-15MB when loaded. **Load-on-demand, evict after processing.**
- **Latency:** <5ms on modern phones (GPU), ~10-20ms CPU (x86).
- **Accuracy:** AP ~98.6%. Near-zero false positives.
- **Strengths:** Smallest viable face detector. Native SDKs for Python, iOS, Android, Web, C++. Temporal stability (weighted blending reduces jitter 30-40%).
- **Weakness:** Fixed input sizes (128×128 or 256×256). Struggles with small/distant faces.
- **Integration:** ONNX export → `ort` crate in Rust proxy.

### 7.2 Tier 2.5+: SCRFD (via ONNX)
- **When:** Screen captures with mixed-size faces (video call thumbnails, profile pictures, ID photos).
- **Model:** SCRFD-2.5GF (~6MB). Sweet spot for accuracy vs size.
- **RAM:** ~25-40MB loaded.
- **Accuracy:** WIDERFace Hard AP 90.57% — significantly outperforms BlazeFace on tiny/occluded/rotated faces.
- **Strengths:** Multi-scale feature pyramid (10px to 1000px+ faces in same image). ICLR 2022.
- **Integration:** ONNX export → same `ort` crate path as BlazeFace.

---

## 8. OCR ENGINE — DECISIONS

### 8.1 Primary: PaddleOCR-Lite (ONNX Runtime)
- **Models:**
  - Detection: `ch_PP-OCRv3_det_slim_quant.onnx` (~1.1MB)
  - Recognition: `ch_PP-OCRv3_rec_slim_quant.onnx` (~4.5MB)
  - Total: <10MB on disk
- **RAM:** ~35-40MB loaded. **Load-on-demand, evict after processing.**
- **Latency:** ~50-150ms/image (CPU, few text lines). ~200-500ms (dense documents).
- **Accuracy:** 98-99% page-level on printed text. PP-OCRv5 adds 13% accuracy over v4.
- **Integration:** `ort` in Rust, `onnxruntime-node` in Node.js. Pre-converted ONNX models on HuggingFace (`monkt/paddleocr-onnx`).
- **Tier-appropriate deployment:**
  - **Tier 1:** Detection-only (~1.1MB, ~30ms) — identify text regions for blur, skip recognition.
  - **Tier 2+:** Full det+rec pipeline → extract text → regex/FPE on extracted PII.

### 8.2 REJECTED: PaddleOCR Native (PaddlePaddle)
- **Reason:** ~800MB-1.5GB PaddlePaddle framework dependency. 200-400MB RAM. Python-only. Non-starter for endpoint devices.

### 8.3 REJECTED: Tesseract (for primary use)
- **Reason:** Larger than PaddleOCR-Lite (~30MB+ with language data), lower accuracy on non-document images.
- **Acceptable as:** Fallback for scanned documents (black text, white paper). Use `tessdata_fast` eng.traineddata (~4MB). Binarize image first.

---

## 9. RAM & STORAGE BUDGET (275MB / 70MB)

### 9.1 RAM Budget — 275MB Ceiling

| Component | MB | Resident? | Notes |
|-----------|-----|-----------|-------|
| Rust PII Proxy | 12 | Yes | HTTP proxy + regex + FPE + vault + routing |
| Gateway Plugin (Node.js) | 25 | Yes | Redactor + file guard + consent + memory governance |
| TinyBERT INT8 NER | 55 | Yes | Largest permanent resident. INT8 mandatory (FP32 = ~200MB) |
| Health/Child Keyword Dict | 2 | Yes | Hash set, ~700 terms |
| Encryption Layer | 6 | Yes | AES-256-GCM + Argon2id |
| Runtime Overhead | 15 | Yes | V8 baseline + tokio + ONNX Runtime shared lib + IPC |
| **Subtotal (always resident)** | **115** | | **Baseline when idle (text-only chat)** |
| Face Detection (BlazeFace) | 8 | On-demand | Load when image arrives, evict after |
| OCR (PaddleOCR-Lite) | 35 | On-demand | Load when image arrives, evict after |
| Image Buffer (shared) | 48 | On-demand | 960px max. THE RAM KILLER — see memory rules |
| **Peak (during image processing)** | **224** | | **51MB headroom to 275MB ceiling** |

### 9.2 Storage Budget — 70MB Ceiling

| Component | MB |
|-----------|-----|
| Rust proxy binary | 8 |
| Node.js plugin code | 3 |
| TinyBERT INT8 ONNX model | 15 |
| BlazeFace models (short+full) | 3 |
| PaddleOCR-Lite ONNX models | 10 |
| Health/child keyword dict | 1 |
| ONNX Runtime shared lib | 12 |
| Consent SQLite DB (empty) | 1 |
| Config/templates/schemas | 2 |
| Headroom (logs, cache) | 15 |
| **Total** | **62** |

---

## 10. EIGHT CRITICAL MEMORY RULES

These rules are the difference between fitting in 275MB and OOM. Every developer must follow them.

1. **Image Buffer is the RAM Killer.** A 12MP photo (4032×3024) in ARGB_8888 = 48MB. ALWAYS resize to 960px long-side BEFORE loading. 960×720 ARGB = ~2.6MB actual.
2. **Sequential, Never Parallel.** Face detection and OCR share the same image buffer. Process sequentially: face detect → blur → release face model → load OCR → extract text → release OCR. Never hold both models simultaneously.
3. **On-Demand Model Loading.** Only NER + proxy + plugin are permanently resident. Face and OCR models load on-demand and evict after processing. Saves ~43MB when no images are being processed.
4. **Single ONNX Runtime Instance.** All models (NER, Face, OCR) share one ONNX Runtime library (~12MB). Don't create separate runtime instances.
5. **Bitmap Pooling.** Android: `inBitmap` reuse. iOS: pre-allocated `CGContext` backing store. Never allocate new bitmap per image.
6. **INT8 Everything.** FP32 → INT8 = 4× RAM reduction. TinyBERT FP32 ~200MB → INT8 ~50MB. PaddleOCR FP32 ~40MB → slim_quant ~10MB. Quantization is mandatory.
7. **Image Tiling for Large Documents.** Images >960px: crop into overlapping tiles (top/bottom half, 10% overlap). Process each sequentially. Merge results. Prevents OOM from intermediate tensors.
8. **Compliance CLI is Lightweight.** ROPA/DPIA generators run on-demand as CLI subcommands. No web server, no React app, no persistent RAM cost. Compliance output is Markdown/JSON/PDF files written to disk.

---

## 11. IMAGE/SCREENSHOT SANITIZATION PIPELINE

Full pipeline for visual PII (Priority 4: Biometric, Priority 7: Screen Captures):

```
1. Image/Screenshot arrives at proxy
2. EXIF metadata strip (always, zero cost — remove GPS, device, timestamps)
3. Resize to 960px max long-side (mandatory before any processing)
4. Face Detection (BlazeFace T1-T2 / SCRFD T2.5+) → Gaussian blur face regions
5. Sensitive Window Check (URL bar / window title matching: banking, health, password manager sites)
6. OCR Text Detection (PaddleOCR det model, ~30ms)
   → Tier 1: Blur all detected text regions (coarse but fast, no recognition)
   → Tier 2+: Full OCR recognition → regex identifies PII → FPE encrypts in-place
   → Tier 2.5+: Add health/child keyword scan on OCR text → flag + FPE adjacent values
7. Sanitized image sent to LLM (faces blurred, text PII encrypted or blurred, EXIF stripped)
```

**Latency by tier:** Tier 1: ~40-80ms. Tier 2: ~100-250ms. Tier 2.5+: ~120-300ms.

---

## 12. BUILD PLAN — 5 PHASES, 20-25 DAYS

### Phase 1: Core + FPE (5-7 days) — 78% PII coverage
- **RAM:** ~60MB. **Storage:** ~14MB.
- Rust PII Proxy (2 days): HTTP reverse proxy, regex scanner, FF1 FPE, credential vault, TOML config.
- Auth Design: Passthrough-first — reuse OpenClaw's API keys by default, optional vault override per provider.
- Gateway Plugin (1.5 days): PII Redactor + File Access Guard via OpenClaw plugin SDK.
- Encryption Layer (1 day): AES-256-GCM transcript encryption, Argon2id KDF.
- FPE Integration (0.5 days): FF1 in Rust proxy. Per-record tweaks. CC#, SSN, phone, email, API keys.
- **Milestone:** Functional privacy proxy. All structured PII FPE-encrypted. Deployable on ANY device with 2GB+ free RAM.
- **UX review:** Verify proxy startup/shutdown messages are clear. Confirm fail-open/closed behavior produces understandable errors (not raw connection refused).

### Phase 2: TinyBERT NER + Consent (5-6 days) — 91% PII coverage
- **RAM:** ~140MB. **Storage:** ~35MB.
- TinyBERT INT8 ONNX (2.5 days): Fine-tune on PII corpus, INT8 quantize, ONNX export. Hybrid mode: regex first, NER second.
- CRF Fallback (1 day): Feature-engineered CRF for <200MB devices. Auto-activated by device profile.
- Consent Manager (1.5 days): GDPR Art. 13/14 system. SQLite, slash commands, AI disclosure, DSAR.
- Health/Child Keywords (0.5 days): ~500 medical + ~200 child terms. Hash set alongside NER.
- Health Monitoring (0.5 days): L0 health endpoint, L1 heartbeat monitor, panic hook + crash marker.
- **Milestone:** Semantic PII detection live. Health, names, child references caught. All 15 PII risks addressed.
- **UX review:** Verify all OpenObscure states (active, degraded, disabled, crashed, OOM, recovering) produce clear, intuitive user messages. Consent slash commands are discoverable. NER confidence warnings are actionable (not cryptic).

### Phase 2.5: Logging Foundation (3 days) — Unified CG_LOG API + Cross-Platform Logging
- **RAM:** ~3.5MB additional (within 15MB "Runtime Overhead" budget).
- **Storage:** ≤30MB log files (within 70MB ceiling).
- **Full design:** See `project-plan/LOGGING_STRATEGY.md`.
- CG_LOG Unified API (0.5 day): `cg_log.rs` Rust macros (`cg_info!`, `cg_warn!`, `cg_audit!`) + `cg-log.ts` TypeScript functions (`cgInfo`, `cgWarn`, `cgAudit`). Single call-site API — modules never import platform loggers directly. Module name constants prevent typos.
- CG_LOG Migration (0.5 day): Replace all ~25 Rust `tracing::*!()` and ~10 TypeScript `console.*` calls. Mechanical find-and-replace.
- PII Scrub Filter (0.5 day): Rust tracing Layer scrubs all string fields. TypeScript `cgLog()` scrubs via existing `redactPii()`. Defense-in-depth for logs.
- File Rotation (0.25 day): `tracing-appender` RollingFileAppender. 5MB roll size, 4 max files.
- GDPR Audit Log (0.25 day): `cg_audit!`/`cgAudit()` tagged events routed to separate durable file. Processing operations only.
- Crash Buffer (0.5 day): mmap ring buffer via `memmap2`. 2MB circular buffer survives SIGKILL/OOM. Read on next startup for post-mortem.
- Config + JSON Mode (0.5 day): `[logging]` TOML section. JSON structured output for Docker/SIEM.
- **Milestone:** Unified logging API across L0+L1. No module calls platform loggers directly. Privacy-safe PII scrubbing on every log event. GDPR audit trail. Crash diagnostics without runtime overhead.

### Phase 3: Visual PII — Face + OCR (4-5 days) — 95% PII coverage
- **RAM:** ~224MB peak (during image processing). **Storage:** ~58MB.
- BlazeFace Integration (1 day): ONNX export, load-on-demand, Gaussian blur.
- PaddleOCR-Lite ONNX (1.5 days): Slim quantized det+rec, 960px input cap, single-threaded.
- Image Pipeline (1.5 days): Full EXIF → resize → face → blur → OCR → regex/NER → FPE → sanitize pipeline.
- Screen Capture Guard (0.5 days): Window title / URL heuristic matching.
- Platform Logging Backends (1.25 days): OSLog (macOS), journald (Linux), Windows Event Log. Conditional compilation per target.
- **Milestone:** Full visual PII protection. Faces blurred, text in images FPE-encrypted, screenshots sanitized. Platform-native logging active.
- **Memory note:** 224MB peak ONLY during image processing. Returns to ~140MB baseline between images.
- **UX review:** Verify image processing latency is communicated (progress indicator or "processing image..." message). OOM during image processing produces a clear recovery message, not a silent crash.

### Phase 4: Compliance + Hardening (2-3 days) — 97% PII coverage
- **RAM:** ~224MB peak (unchanged). **Storage:** ~62MB.
- Cross-Border Router (0.5 days): Policy engine. Residency classification.
- Memory Governance (1 day): Tiered lifecycle, PII scan on write, vector embedding erasure, FPE key rotation.
- ROPA/DPIA Generators (0.5 days): CLI tools (`openobscure compliance ropa`, `openobscure compliance dpia`), Markdown/JSON/PDF output.
- Breach Detection (0.5 days): Lightweight log watcher, anomaly scoring, Art. 33 notification drafts. Feeds from GDPR audit log.
- Process Watchdog: launchd/systemd integration for auto-restart of L0 on crash.
- Advanced Logging (1.5 days): ETW for Windows IoT, Android Logcat bridge (NDK), SIEM export (CEF/LEEF), log-based breach detection feed.
- **Milestone:** Complete OpenObscure product. Full compliance automation. Production-ready.
- **UX review:** CLI compliance commands produce clear, well-formatted output. DSAR export format is understandable. Breach alerts are actionable. Watchdog auto-restart is invisible to user (recovery message only).

### Phase 5: Final Hardening — Key Rotation, SSE, Benchmarks (COMPLETE)
- FPE key rotation with versioned vault keys + 30s overlap window
- SSE streaming proxy for `text/event-stream` responses
- PII benchmark corpus (~400 samples, 100% recall/precision)
- Production benchmarks via criterion

### Phase 6: Ensemble Recall + Cleanup (COMPLETE)
- Ensemble confidence voting in HybridScanner (cluster-based overlap resolution + agreement bonus)
- Detection verification framework (BboxMeta, NsfwMeta, ScreenshotMeta validators)
- Image pipeline accuracy improvements

### Phase 7: Cross-Platform + Mobile Library (COMPLETE)
- **Two deployment models:** Gateway Model (sidecar proxy, all platforms) and Embedded Model (native library, mobile)
- **Gateway-side (7A):** Windows RAM detection (`GlobalMemoryStatusEx`), keyring platform compatibility, Linux ARM64 build verification
- **Mobile-embedded (7B):** `lib_mobile.rs` — `OpenObscureMobile` API (`sanitize_text`, `restore_text`, `sanitize_image`, `stats`). UniFFI bindings for Swift/Kotlin auto-generation. Cargo `staticlib`/`cdylib` output. Build scripts for iOS (XCFramework) and Android (cargo-ndk).
- **Milestone:** OpenObscure runs on all platforms where AI agents run — desktop, server, phone, tablet.

---

## 13. DEVICE COMPATIBILITY

### Adaptive Deployment Strategy
The installer auto-detects available RAM and selects configuration:
- **≥2.5GB free RAM:** Full stack with TinyBERT NER (224MB peak). All 4 phases.
- **1.5-2.5GB free:** CRF fallback, Phase 1-2 only (~90MB peak). 85% PII coverage.
- **<1.5GB free:** Companion mode, relaying through desktop OpenObscure.

### Device Profiles

| Device | Free RAM | Fit | Notes |
|--------|----------|-----|-------|
| MacBook Air M1 (8GB) | ~2.8GB | All 4 phases | Sweet spot device. Neural Engine accelerates inference. |
| MacBook Air M2/M3 (16GB) | ~5.5GB | Full + headroom | Can run SCRFD + full PaddleOCR + potentially local Ollama. |
| Windows Laptop (16GB) | ~5GB | All 4 phases | NVIDIA GPU or Copilot+ NPU accelerates inference. |
| Windows Ultrabook (8GB) | ~2.2GB | Phase 1-2 | CRF fallback recommended. Phase 3 possible with aggressive tiling. |
| Linux Dev (16GB) | ~8GB | Full + headroom | Lowest OS overhead. Best ONNX Runtime CPU performance. |
| Raspberry Pi 5 (8GB) | ~5.5GB | All 4 phases | ARM Cortex-A76 runs TinyBERT INT8 ~20ms. OCR ~200ms. |
| Android Flagship (12GB) | ~4GB | All 4 phases | Via Termux. Hexagon NPU accelerates NER. |
| Android Mid-Range (6GB) | ~1.5GB | Phase 1-2 | CRF fallback. Phase 3 tight. |
| iPhone 15/16 (6GB) | ~1.8GB | Companion | iOS sandbox prevents background daemons. Neural Engine excellent but inaccessible for background use. |
| Linux VPS (4GB) | ~2.5GB | Phase 1-2 | Skip Phase 3 if mostly text (no images). |

---

## 14. COST ESTIMATES

### API Costs (Claude Code)
- **Phase 1 (V1 MVP):** $175-370 (30-50 sessions × $6-12 per session)
- **Phase 2-4 (V2):** $186-286 additional
- **Phase 2.5 (Logging + CG_LOG):** $36-72 additional
- **Total:** ~$397-728

### Developer Time
- **Phase 1:** 5-7 working days, 40-55 hours
- **Phase 2-4:** 12-15 working days, 85-135 hours
- **Phase 2.5:** 3 working days, ~24 hours
- **Total:** 20-25 working days, ~149-214 hours

### Deployment Paths
- **Path A:** V1 only. $175-370, 1 week. 78% PII coverage. Solo developer without EU contacts.
- **Path B:** V1 + NER + Consent + Logging. $270-521, 2-3 weeks. 91% coverage. Legal defensibility. Privacy-safe logs.
- **Path C:** Full build (all 5 phases). $391-706, 4-6 weeks. 97% coverage. Regulated industries.
- **Path D:** Full build + contractor. $4,891-8,706, 3-4 weeks. Parallelized timeline.

---

## 15. TECHNOLOGY STACK SUMMARY

| Component | Technology | Size | Why This Choice |
|-----------|-----------|------|----------------|
| PII Proxy | Rust (tokio + hyper) | 8MB binary | Performance, safety, AES-NI access |
| FPE | FF1 via AES primitives in Rust | 0MB additional | NIST-approved. FF3 is WITHDRAWN. |
| Gateway Plugin | TypeScript (OpenClaw SDK) | 3MB | Native OpenClaw integration |
| NER (primary) | TinyBERT-4L-312D INT8 ONNX | 15MB model | 96% BERT accuracy at 1/25th size |
| NER (fallback) | CRF | <5MB | For <200MB devices |
| Face Detection (T1-T2) | MediaPipe BlazeFace ONNX | 0.23-2.3MB | Smallest viable detector |
| Face Detection (T2.5+) | SCRFD-2.5GF ONNX | 6MB | Multi-scale, hard-case accuracy |
| OCR | PaddleOCR-Lite v3 slim quant ONNX | <10MB | Best accuracy-per-MB ratio |
| Inference Runtime | ONNX Runtime | 12MB shared | Single runtime for all models |
| Encryption | AES-256-GCM + Argon2id | 0MB (stdlib) | Industry standard |
| Consent DB | SQLite | <1MB | Embedded, zero-config |
| Compliance CLI | Part of L0 binary | 0MB additional | CLI subcommands for ROPA/DPIA/breach |
| Config | TOML | <1MB | Human-readable |
| Logging (core) | tracing + tracing-subscriber | 0MB (existing) | Structured, async-aware, layered |
| Logging (crash) | memmap2 (mmap ring buffer) | <0.1MB | Survives SIGKILL/OOM kill |
| Logging (macOS) | tracing-oslog | <0.1MB | Console.app + Unified Logging |
| Logging (Linux) | tracing-journald | <0.1MB | systemd integration |
| Logging (Windows) | tracing-eventlog / tracing-etw | <0.1MB | Event Viewer + IoT tokenized |

---

## 16. OPEN ITEMS & FUTURE DECISIONS

- [OPEN] **TinyBERT fine-tuning dataset:** Need 500-1000 labeled PII examples covering health, child, financial, and contact categories. Source options: synthetic generation, Presidio benchmark data, or manual labeling from OpenClaw usage logs.
- [OPEN] **OpenClaw hook improvements:** Monitor for `before_tool_call` wiring in future releases. When it ships, L1 plugin upgrades from soft to hard enforcement (eliminating L0 proxy for some use cases).
- [DROPPED] **MCP integration:** Not needed — L0 proxy handles FPE transparently on the network path, L1 plugin handles redaction in-process. MCP would be redundant indirection.
- [OPEN] **Voice anonymization:** Speaker voice anonymization (~200MB, Tier 4 only) deferred. Evaluate when high-resource device adoption increases.
- [OPEN] **GLiNER evaluation:** If TinyBERT accuracy proves insufficient on contextual PII, evaluate GLiNER on 16GB+ devices as Tier 3 NER upgrade.
- [DECIDED] **Logging strategy:** Platform-specific logging documented in `project-plan/LOGGING_STRATEGY.md`. Android: mmap ring buffer. iOS/macOS: OSLog. Windows: async file rotation + ETW (IoT). Linux: file rotation + journald. PII scrub filter on all platforms. Phase 2.5 (foundation) → Phase 3 (platform backends) → Phase 4 (advanced/SIEM).
- [DECIDED] **Document classifier:** Collection-based dictionary matching + confidentiality markers. Documented in `project-plan/DOCUMENT_CLASSIFIER_REQUIREMENTS.md`. Not yet scheduled.
- [DECIDED] **Two deployment models (Phase 7):** Gateway Model (sidecar proxy, L0+L1+L2, desktop/server) and Embedded Model (native library, L0 only, mobile). Both can run simultaneously for defense in depth. See `ARCHITECTURE.md`.
- [OPEN] **L1 Rust rewrite (Phase 8):** Port consent manager, file access guard, and memory governance from TypeScript to Rust for Embedded Model. Enables full governance on mobile. ~2 weeks effort.
- [OPEN] **ONNX mobile execution providers:** CoreML (iOS Neural Engine) and NNAPI (Android NPU) for hardware-accelerated inference on mobile. Requires pre-built ORT binaries.
- [OPEN] **CI/CD cross-platform matrix:** GitHub Actions build verification for macOS, Linux x64, Linux ARM64, Windows, iOS simulator, Android NDK.

---

## 17. KEY WARNINGS

1. **FF3 is WITHDRAWN.** Never use FF3 for FPE. `fpe-arena` is educational only. Use FF1 exclusively.
2. **`tool_result_persist` is synchronous.** Returning a Promise causes OpenClaw to silently skip the hook. All L1 processing must be sync.
3. **Only 3 of 14 OpenClaw hooks are wired.** Do not depend on `before_tool_call`, `message_sending`, etc. They are defined but never invoked.
4. **OpenClaw updates constantly.** 40+ security patches per release. OpenObscure modules touching internal APIs may break. Pin to known-good OpenClaw versions.
5. **Image buffers, not models, cause OOM.** A single 12MP ARGB bitmap = 48MB. Always resize before loading.
6. **INT8 quantization is not optional.** FP32 TinyBERT = ~200MB. INT8 = ~50MB. The difference between fitting and crashing.

---

## 18. ARTIFACTS PRODUCED

All interactive analysis artifacts (React .jsx files) are available:
- `OpenObscure_Architecture.jsx` — 3-layer architecture with module details
- `OpenObscure_V2_Plan.jsx` / `OpenObscure_V2_Full_Plan.jsx` — V2 module breakdown, costs, timeline
- `OpenObscure_Hardware_Requirements.jsx` — Device profiles, tier mapping (T0-T4)
- `OpenObscure_Practical_Plan.jsx` — Practical build plan
- `OpenObscure_FPE_Analysis.jsx` — FPE viability across 7 PII categories, multi-modal strategies
- `OpenObscure_FaceOCR_Analysis.jsx` — MediaPipe vs SCRFD, PaddleOCR ONNX vs Native
- `OpenObscure_Build_Plan_Final.jsx` — Final RAM/storage budget, NER selection, 4-phase build, memory rules, device compatibility

**To run locally:** `npm create vite@latest openobscure-docs -- --template react` → copy .jsx files to `src/` → import in `App.jsx` → `npm run dev`.

---

## 19. PROJECT CONVENTIONS

- **Session notes**: Create a session notes file in `session-notes/` at every `/compact` point and at the end of each session, always using `ses_YY-MM-DD-HH-MM.md` format (e.g., `session-notes/ses_26-02-16-20-04.md`)
- **Phase documentation**: When completing a phase, create `ARCHITECTURE.md` and `LICENSE_AUDIT.md` in each component folder, update `project-plan/PHASE<N>_PLAN.md`, and create next phase plan if applicable
- **Architecture updates**: Keep root `ARCHITECTURE.md` up to date as design evolves during implementation

---

## 20. INSTRUCTION FOR CLAUDE

When this document is attached to a new conversation:
1. **Do not re-derive or re-debate any decision marked as FINAL.** Build from them.
2. **Check Section 16 (Open Items)** for unresolved decisions before starting work.
3. **Check Section 17 (Warnings)** before writing any code.
4. **Reference Section 9 (RAM/Storage Budget)** before adding any new component — every megabyte is accounted for.
5. **The build plan in Section 12 is the canonical work sequence.** Ask which phase the user is working on and proceed from there.
6. If the user asks to modify architecture, check impact against the RAM budget first. The 275MB ceiling is a hard constraint.
