# Fail Behavior Reference

Every subsystem's behavior when it encounters an error, model absence, or resource constraint — including whether it fails open or closed, the exact log output, any HTTP status code returned to the caller, and any health counter incremented.

Source files: `body.rs`, `proxy.rs`, `fpe_engine.rs`, `vault.rs`, `hybrid_scanner.rs`, `ner_scanner.rs`, `ner_endpoint.rs`, `crf_scanner.rs`, `image_pipeline.rs`, `voice_pipeline.rs`, `kws_engine.rs`, `response_integrity.rs`, `response_format.rs`, `lang_detect.rs`, `multilingual/mod.rs`, `health.rs`, `main.rs`, `redactor.ts`, `index.ts`.

---

## Quick Reference Table

| Subsystem | Failure condition | Mode | HTTP status | Health stat incremented |
|-----------|-------------------|------|-------------|------------------------|
| **FPE vault / key init** | Keychain unavailable, bad hex key, or key not found | **Fatal** — process exits at startup | — | None |
| FPE encryption | Error, `fail_mode = "open"` | **Fail-open** — plaintext forwarded | None (response header set) | `fpe_unprotected_total` |
| FPE encryption | Error, `fail_mode = "closed"` | **Fail-closed** — `[REDACTED:{type}]` | None | `fpe_unprotected_total` |
| FPE encryption | `DomainTooSmall` (open mode) | Fail-open — hash-token fallback | None | *(none — not counted)* |
| NER model load (startup) | Model files missing or >70MB | Fail-open — falls back to CRF or regex | None | None |
| NER runtime — pool exhausted | All sessions held for >2s | Fail-open — regex-only for this request | None | None |
| NER runtime — inference error | ORT error or caught panic | Fail-open — empty semantic results | None | None |
| **NER HTTP endpoint** — bad request | Malformed JSON body | Hard fail | **400** Bad Request | None |
| **NER HTTP endpoint** — auth | Missing / wrong `x-openobscure-token` | Hard fail | **401** Unauthorized | None |
| **NER HTTP endpoint** — size | Input text >64KB | Hard fail | **413** Payload Too Large | None |
| CRF model load (startup) | `crf_model.json` missing/invalid | Fail-open — falls back to regex | None | None |
| CRF inference | N/A — infallible | N/A | N/A | N/A |
| Image — NSFW classifier load | Load error | Fail-open — NSFW phase skipped | None | None |
| Image — NSFW inference | ORT/inference error | Fail-open — NSFW phase skipped | None | None |
| Image — OCR detector load | Load error | Fail-open — OCR phase skipped | None | `onnx_panics_total` (on panic) |
| Image — OCR detection inference | ORT/inference error | Fail-open — OCR phase skipped | None | `onnx_panics_total` (on panic) |
| Image — OCR recognizer load | Load error | Fail-open — recognition phase skipped | None | `onnx_panics_total` (on panic) |
| Image — OCR recognition inference | ORT/inference error | Fail-open — recognition phase skipped | None | `onnx_panics_total` (on panic) |
| Image — SCRFD face detection | Inference error | Fail-open — face phase skipped | None | `onnx_panics_total` (on panic) |
| Image — Ultra-Light face detection | Inference error | Fail-open — face phase skipped | None | `onnx_panics_total` (on panic) |
| Image — BlazeFace load | Load error | Fail-open — face phase skipped | None | `onnx_panics_total` (on panic) |
| Image — BlazeFace inference | Inference error | Fail-open — face phase skipped | None | `onnx_panics_total` (on panic) |
| Image pipeline (whole) | Any unrecovered error | Fail-open — original image forwarded | None | `onnx_panics_total` (on panic) |
| Image pipeline — URL fetch | Network error or decode error | Fail-open — URL kept in body | None | None |
| Voice KWS (startup) | Model files missing | Fail-open — audio passes unscanned | None | None |
| Voice KWS (runtime) | Audio decode or inference error | Fail-open — audio block unchanged | None | None |
| R2 TinyBERT | Model dir missing | Fail-open — R1 dictionary scan only | None | None |
| R2 TinyBERT | Inference error | Fail-open — R1 dictionary scan only | None | None |
| **Response format** | `UnknownJson` or `Opaque` response | Fail-open — RI scan entirely skipped | None | None |
| `whatlang` detection | Text <20 bytes or confidence <0.15 | **Silent skip** — multilingual pass omitted | None | None |
| Request body (whole-body scan) | JSON/scan error, `fail_mode = "open"` | Fail-open — original body forwarded | None | `scan_latency` (still recorded) |
| Request body (whole-body scan) | JSON/scan error, `fail_mode = "closed"` | Fail-closed | **502** Bad Gateway | `scan_latency` (still recorded) |
| Body size limit | Body > tier limit | Hard fail | **413** Payload Too Large | None |
| L1 NAPI addon load | `require()` fails | Fail-open — JS regex (5 types) | N/A | N/A |
| L1 NER endpoint (`callNerEndpoint`) | curl timeout / connection refused | Fail-open — JS regex only | N/A | N/A |
| L1 `before_tool_call` registration | Hook not wired by framework | Fail-open — soft enforcement only | N/A | N/A |
| L1 `tool_result_persist` hook body | Unhandled exception | Propagates to plugin framework | Framework-dependent | N/A |

---

## Detailed Entries

### FPE Vault / Key Initialization (`vault.rs`, `main.rs`)

The FPE master key must be available before any request can be served. `Vault::init_fpe_key()` and `Vault::get_fpe_key()` are called during `run_serve()` startup. On failure, `main.rs` returns an `anyhow::Error` and the process exits.

**Error cases (all fatal):**

| `VaultError` variant | Cause | `anyhow` message |
|---------------------|-------|-----------------|
| `Keyring(keyring::Error)` | OS keychain unavailable or access denied | `"Failed to initialize FPE key: Keychain error: <msg>"` |
| `KeyNotFound(String)` | Key not yet provisioned in keychain and no `OO_FPE_KEY` env var | `"Failed to initialize FPE key: <msg>"` |
| `EnvVar(String)` | `OO_FPE_KEY` set but not valid hex | `"Failed to initialize FPE key: Environment variable error: <msg>"` |
| `InvalidKeyLength(usize)` | Key decoded to wrong byte length (must be 32) | `"Failed to initialize FPE key: FPE key has invalid length: expected 32 bytes, got N"` |

**Effect:** Process does not start; no HTTP server is bound. No health counter is relevant.

**Source:** `vault.rs:21–90`, `main.rs:211`

---

### FPE Encryption Per-Span (`body.rs`, `fpe_engine.rs`)

FF1 encryption is attempted for each detected PII span. Behavior depends on `fail_mode` in `[proxy]` config (default: `"open"`).

**`fail_mode = "open"` — error other than DomainTooSmall:**

```
WARN  body  FPE encryption failed, plaintext PII forwarded (fail-open)  type=<type>  error=<msg>
```

- Original text is forwarded unchanged (PII is not redacted).
- `fpe_unprotected_total` incremented by 1.
- Response carries `x-openobscure-pii-unprotected: N` header (N = total unprotected spans in the request).

**`fail_mode = "open"` — `DomainTooSmall` (value too short to encrypt with FF1):**

```
INFO  body  FPE domain too small, hash-token fallback  type=<type>
```

- A deterministic hash token (e.g., `PER_a3f2c1`) is substituted.
- Counter is incremented then immediately decremented — this case is **not** counted in `fpe_unprotected_total`.
- No `x-openobscure-pii-unprotected` header set for this case.

**`fail_mode = "closed"` — any error:**

```
WARN  body  FPE encryption failed, applying destructive redaction (fail-closed)  type=<type>  error=<msg>
```

- `[REDACTED:{type}]` is substituted (e.g., `[REDACTED:credit_card]`).
- `fpe_unprotected_total` incremented by 1. Response header still set if count > 0.

**Source:** `body.rs:188–232`

---

### NER Model Load Failure (startup, `ner_scanner.rs`, `main.rs`)

`NerScanner::load()` is called during `build_scanner()` at startup. Errors are caught and the scanner falls back gracefully — no HTTP error is ever returned due to a load-time NER failure.

**Model files missing:**

```
WARN  scanner  Scanner mode 'ner' requested but model unavailable, using regex+keywords
```

(Only in forced `scanner_mode = "ner"`. In `auto` mode the fallback is silent at INFO level.)

**Model too large (>70MB):**

The load returns `NerError::OnnxRuntime("NER model is X.X MB — exceeds Y MB limit. Expected TinyBERT 4L INT8 (~14 MB) or DistilBERT 6L INT8 (~64 MB)...")` and the startup fallback chain applies.

**Degraded state:** `SemanticBackend` is set to `Crf` (if CRF loads) or omitted (regex+keywords only) for the lifetime of the process.

**Source:** `ner_scanner.rs:73–90`, `main.rs:1251–1257`

---

### NER Runtime Inference Failure (per-request, `ner_scanner.rs`, `hybrid_scanner.rs`)

Each request acquires a session from `NerPool`. Three distinct error paths exist.

**Pool exhaustion (all sessions held for >2 seconds):**

```
WARN  ner  NER pool exhausted after 2s, falling back to regex-only
```

`hybrid_scanner.rs` receives `None` from `pool.acquire()` and returns `Vec::new()` for the semantic step. Regex, keyword, gazetteer, and multilingual passes all complete normally.

**NER inference error (ORT returns Err):**

```
WARN  hybrid  NER inference failed, skipping  error=<msg>
```

Returns `Vec::new()` for the semantic step.

**NER ONNX panic (caught by `catch_unwind`):**

`scan_text_single_pass()` wraps `session.run()` in `catch_unwind`. Panics are converted to `Err(NerError::OnnxRuntime("ONNX Runtime panicked during NER inference"))` and surface as the inference-error path above. `onnx_panics_total` is **not** incremented here — that counter is only for image pipeline panics.

**Source:** `ner_scanner.rs:245–265`, `ner_scanner.rs:712–743`, `hybrid_scanner.rs:196–208`

---

### NER HTTP Endpoint Errors (`ner_endpoint.rs`)

The `POST /_openobscure/ner` endpoint has three hard-fail cases that return HTTP errors without scanning. These are synchronous rejections, not fail-open.

| Condition | HTTP status | Source |
|-----------|-------------|--------|
| Missing or wrong `x-openobscure-token` header (when auth is configured) | **401** Unauthorized | `ner_endpoint.rs:54` |
| Request body is not valid JSON | **400** Bad Request | `ner_endpoint.rs:59` |
| `text` field length > 65,536 bytes | **413** Payload Too Large | `ner_endpoint.rs:63` |

No log is emitted for 400/401/413 rejections. The L1 plugin's `callNerEndpoint()` treats all non-200 responses as errors and returns `null` (silent fail-open on the L1 side).

**Source:** `ner_endpoint.rs:45–64`

---

### CRF Model Load Failure (startup, `crf_scanner.rs`, `main.rs`)

`CrfScanner::load()` returns `Err(CrfError::ModelNotFound | ModelParse | Io)` when the model directory or `crf_model.json` is absent or corrupt.

```
WARN  scanner  Scanner mode 'crf' requested but model unavailable, using regex+keywords
```

(Only in forced `scanner_mode = "crf"`. In `auto` mode after NER miss, fallback to regex is logged at INFO.)

**Source:** `crf_scanner.rs:58–92`, `main.rs:1260–1268`

---

### CRF Inference (`crf_scanner.rs`)

`CrfScanner::scan_text(&self, text: &str) -> Vec<PiiMatch>` is **infallible**. Viterbi decoding and feature extraction have no error return paths. Entities below the confidence threshold are silently dropped in `push_entity()`. Empty text returns an empty vec immediately.

**Source:** `crf_scanner.rs:95–115`

---

### Image Pipeline — NSFW Phase (`image_pipeline.rs`)

Two distinct fail-open events — load and inference are logged separately.

**NSFW classifier load failure** (`image_pipeline.rs:375`):

```
WARN  image  NSFW classifier load failed (fail-open)  error=<msg>
```

The classifier slot remains `None`. NSFW phase is skipped for all subsequent requests until restart; downstream face and OCR phases proceed.

**NSFW inference failure** (`image_pipeline.rs:422`):

```
WARN  image  NSFW classification failed (fail-open)  error=<msg>
```

The NSFW result is treated as "no detection". Downstream phases proceed.

**Success path:** `nsfw_blocked_total` is incremented via `record_nsfw_blocked(1)` only on successful detection.

---

### Image Pipeline — Face Detection Phase (`image_pipeline.rs`)

Three face detector variants are tried in tier order (SCRFD → Ultra-Light → BlazeFace). Each has distinct fail-open log messages.

**SCRFD detection inference failure** (`image_pipeline.rs:704`):

```
WARN  face  SCRFD detection failed (fail-open)  error=<msg>
```

**Ultra-Light detection inference failure** (`image_pipeline.rs:751`):

```
WARN  face  Ultra-Light detection failed (fail-open)  error=<msg>
```

**BlazeFace model load failure** (`image_pipeline.rs:775`):

```
WARN  face  BlazeFace load failed (fail-open)  error=<msg>
```

**BlazeFace inference failure** (`image_pipeline.rs:790`):

```
WARN  face  BlazeFace detection failed (fail-open)  error=<msg>
```

On any face-phase failure, face detection results are empty (`faces = []`) and the OCR phase proceeds. `faces_redacted_total` is only incremented on successful detection and redaction.

---

### Image Pipeline — OCR Phase (`image_pipeline.rs`)

The OCR pipeline has a two-stage architecture (detector then recognizer), each with separate load and inference fail-open paths.

**OCR detector load failure** (`image_pipeline.rs:493`):

```
WARN  ocr  OCR detector load failed (fail-open)  error=<msg>
```

**OCR detection inference failure** (`image_pipeline.rs:653`):

```
WARN  ocr  OCR detection failed (fail-open)  error=<msg>
```

**OCR recognizer load failure** (`image_pipeline.rs:541`):

```
WARN  ocr  OCR recognizer load failed (fail-open)  error=<msg>
```

**OCR recognition inference failure** (`image_pipeline.rs:641`):

```
WARN  ocr  OCR recognition failed (fail-open)  error=<msg>
```

On any OCR-phase failure, text regions are not redacted. `text_regions_total` is only incremented on successful detection. `onnx_panics_total` is incremented if any of these failures is an ONNX panic (tracked via `ImageStats.onnx_panics`).

---

### Image Pipeline — Whole-Pipeline Failure (`body.rs`)

If an error propagates out of `process_single_image()` to the `body.rs` call site (i.e., an error not caught by any of the per-phase handlers above):

```
WARN  body  Image processing failed (fail-open)  error=<msg>
```

The original base64-encoded image is left unmodified in the request body and forwarded to the upstream LLM. `images_processed_total` is **not** incremented for failed images.

**Source:** `body.rs:421`

---

### Image Pipeline — URL Image Fetch (`body.rs`)

When an image URL is found in a JSON body, the proxy fetches it via `image_fetch.rs`. Two distinct error paths:

**Network error, timeout, or non-2xx response:**

```
WARN  body  URL image fetch failed (fail-open)  url=<url>  error=<msg>
```

**Decode or pipeline error after successful fetch:**

```
WARN  body  URL image processing failed (fail-open)  error=<msg>
```

In both cases the original URL string is left in the request body unchanged.

**Source:** `body.rs:548`, `body.rs:557`

---

### Voice KWS — Startup Load Failure (`main.rs`, `kws_engine.rs`)

`KwsEngine::new()` is called during `run_serve()`. On any `KwsError` (model files missing, null handle from C API, or C string conversion error):

```
WARN  voice  KWS models not found, audio will pass through unscanned  error=<msg>
```

The voice engine is set to `None`. Audio blocks in subsequent requests are left unmodified with no per-request log.

**Budget-disabled path** (device tier too low):

```
INFO  voice  Voice pipeline disabled by device budget  tier=<tier>  max_ram_mb=<N>
```

**Feature flag disabled** (`voice.enabled = false`):

```
INFO  voice  Voice pipeline disabled
```

**Source:** `main.rs:335–357`, `kws_engine.rs:18–36`

---

### Voice KWS — Runtime Inference Failure (`voice_pipeline.rs`)

For each audio block, `scan_single_block()` calls `audio_decode::decode_audio_to_pcm()` then `kws_engine.detect_pii()`. Both propagate as a `String` error. On any error:

```
WARN  voice  Audio PII scan failed, passing through  error=<msg>  path=<json.path>
```

The audio block is left unmodified. The block counts in `blocks_scanned` but not `blocks_stripped`.

**Source:** `voice_pipeline.rs:115–123`

---

### R2 TinyBERT — Response Integrity (`response_integrity.rs`)

The response integrity scanner has two cascades: R1 (dictionary, always active) and R2 (TinyBERT ONNX, optional).

**R2 model directory missing:**

```
INFO  ri  R2 model directory not found, running R1-only mode
```

R2 is not loaded. All subsequent responses use R1 only.

**R2 inference error (per-response):**

```
WARN  ri  R2 inference failed, falling back to R1-only
```

Returns `(None, R2Role::NotUsed, Vec::new())` for the R2 cascade. R1 flags are still surfaced normally. Responses always pass through regardless of RI findings — RI is advisory, not blocking.

**Health stats on success:** `ri_scans_total` and `ri_flags_total` incremented via `record_ri_scan()` / `record_ri_flags(count)`.

**Source:** `response_integrity.rs:160–176`, `response_integrity.rs:296–303`

---

### Response Format — Unknown JSON or Opaque (`response_format.rs`)

Before the RI scanner runs, `response_format::detect()` classifies the upstream response body. When the format is `UnknownJson` (valid JSON but no recognized provider structure) or `Opaque` (non-JSON content-type, or body fails JSON parse), `ResponseFormat::supports_ri()` returns `false`:

```
// response_format.rs:40–42
pub fn supports_ri(&self) -> bool {
    !matches!(self, Self::UnknownJson | Self::Opaque)
}
```

**No log message is emitted.** The RI scan is skipped entirely for that response — `extract_text()`, `inject_warning()`, and R2 inference are all bypassed. The response is forwarded unmodified. `ri_scans_total` is **not** incremented when RI is skipped due to format.

Affected cases: image/video/binary responses, streaming responses with unrecognized structure, responses from custom/private LLM providers that don't match any known schema.

**Source:** `response_format.rs:38–43`, `response_format.rs:156–160`, `response_format.rs:261–268`

---

### `whatlang` Language Detection — `None` return (`lang_detect.rs`, `hybrid_scanner.rs`)

`detect_language()` returns `None` when:
- `text.len() < 20` (`lang_detect.rs:76`)
- `whatlang::detect()` returns `None` (text too ambiguous for trigram analysis)
- Confidence < 0.15 (`lang_detect.rs:85`)
- Detected language not in the supported set (`lang_detect.rs:99`)

**No log message is emitted.** `languages_to_scan(None)` returns `vec![]` (`multilingual/mod.rs:42`) and the multilingual loop body never executes. All other engines (regex, keywords, gazetteer, NER/CRF) run unconditionally before this step and are unaffected.

**Source:** `lang_detect.rs:74–106`, `multilingual/mod.rs:39–59`, `hybrid_scanner.rs:219–236`

---

### Whole-Body Scan Errors (`proxy.rs`, `body.rs`)

`process_request_body()` returns `Err(BodyError)` on JSON parse failure or internal serialization error. The `proxy_handler()` caller applies `fail_mode`.

**`fail_mode = "open"` (default):**

```
WARN  proxy  Body processing failed (fail-open), forwarding original  error=<msg>
```

The original, unmodified request body is forwarded. `scan_latency` histogram is still recorded.

**`fail_mode = "closed"`:**

```
ERROR  proxy  Body processing failed (fail-closed), rejecting request  error=<msg>
```

**HTTP 502 Bad Gateway** returned to the host agent. No request reaches the upstream LLM.

**Source:** `proxy.rs:240–261`

---

### Request Body Size Limit (`proxy.rs`)

`buffer_body()` accumulates streaming chunks. When the running total exceeds the tier-configured limit:

```
WARN  proxy  Request body exceeds size limit  accumulated=<N>  limit=<M>
```

**HTTP 413 Payload Too Large** is returned immediately. `fail_mode` does not apply — 413 is always returned regardless of mode.

Default limits: Lite 10MB, Standard 50MB, Full 100MB. Override via `proxy.body_limit_lite/standard/full`.

**Source:** `proxy.rs:1072–1095`

---

## L1 Plugin (TypeScript)

### NAPI Addon Load Failure (`redactor.ts:42–48`)

```typescript
try {
  const mod = require("@openobscure/scanner-napi");
  NativeScanner = mod.OpenObscureScanner;
} catch {
  // Not installed — redactPii() falls back to JS regex
}
```

**Silent fail-open.** No log. `redactPii()` uses the JS regex path (5 structured types). No error thrown to the caller.

---

### NER Endpoint Call Failure (`redactor.ts:297–328`)

`callNerEndpoint()` uses `execFileSync("curl", ...)` with `--max-time 2` (curl timeout) and a 3-second Node.js `timeout`. On any error — connection refused, OS timeout, non-200 status, JSON parse failure:

```typescript
} catch {
  return null;
}
```

**Silent fail-open.** Returns `null`. `redactPiiWithNer()` skips the NER merge and proceeds with regex-only redaction. No log message.

---

### `before_tool_call` Hook Registration Failure (`index.ts:126–153`)

**If `api.hooks.before_tool_call` throws during registration:**

```
INFO  plugin  before_tool_call hook not available — soft enforcement only
```

Enforcement falls back to `tool_result_persist` only (post-execution redaction). No error thrown to the plugin framework.

---

### `tool_result_persist` Hook — Unhandled Exception (`index.ts:94–119`)

The hook callback has **no internal `try/catch`**. If `redactPii()` or `redactPiiWithNer()` throws, the exception propagates to the plugin framework. OpenClaw's behavior on hook-callback exception is framework-defined.

---

## Health Endpoint Observable Counters

All counters are exposed at `GET /_openobscure/health` and persist to disk between restarts (`health.rs:228–330`).

| Counter | Incremented by | Incremented when |
|---------|---------------|-----------------|
| `fpe_unprotected_total` | `record_fpe_unprotected(N)` | FPE fail-open (plaintext forwarded) **or** fail-closed (destructive redaction). **Not** for DomainTooSmall hash-token fallback. |
| `onnx_panics_total` | `record_onnx_panic()` | ONNX panic during **image pipeline** inference only. NER panics are caught by `catch_unwind` and do not increment this counter. |
| `nsfw_blocked_total` | `record_nsfw_blocked(1)` | NSFW classifier **detected** explicit content (success path only — failures do not increment). |
| `faces_redacted_total` | `record_faces_redacted(N)` | Face detection and redaction **succeeded** (not on errors). |
| `text_regions_total` | `record_text_regions(N)` | OCR detected text regions **successfully** (not on errors). |
| `ri_scans_total` | `record_ri_scan()` | RI scan attempted. **Not** incremented when RI is skipped due to `UnknownJson`/`Opaque` format. |
| `ri_flags_total` | `record_ri_flags(N)` | RI flags raised in an R1 or R2 scan. |
| `requests_total` | `record_request()` | Every proxied request, including errored and rejected ones. |
| `images_processed_total` | `record_images_processed(1)` | Image pipeline ran and completed successfully. **Not** incremented for failed images. |
| `screenshots_detected_total` | `record_screenshots_detected(1)` | EXIF/heuristic screenshot detection (success path). |

**Counters with no failure-mode increment:** NER pool exhaustion, NER inference errors, CRF load failures, voice inference failures, `whatlang` detection misses, response format skips, R2 inference failures. All of these are observable only in log output (WARN or INFO level).
