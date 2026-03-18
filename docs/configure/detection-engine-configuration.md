# Detection Engine Configuration

OpenObscure detects PII using a layered stack of engines. On every request, the **HybridScanner** runs all enabled engines in parallel, merges their results with confidence voting, and feeds the final match list to the FPE encryption stage. Which engines are enabled depends on your hardware tier and configuration.

This page answers: **which engine runs when, and how do I change it?**

---

**Contents**

- [Detection Stack](#detection-stack)
- [Text Detection Engines](#text-detection-engines)
- [Multilingual Scanner](#multilingual-scanner)
- [Image Detection Engines](#image-detection-engines)
- [Runtime Fallback Conditions](#runtime-fallback-conditions)
- [Forcing a Specific Engine](#forcing-a-specific-engine)
- [Ensemble Confidence Voting](#ensemble-confidence-voting)
- [Tier-to-Engine Mapping](#tier-to-engine-mapping)
- [L1 Plugin Scanner (NAPI)](#l1-plugin-scanner-napi)
- [Verifying Active Engines](#verifying-active-engines)

## Detection Stack

```
Request text
    │
    ▼
┌──────────────────────────────────────────────────────────┐
│                     HybridScanner                        │
│                                                          │
│  ┌─────────────┐  ┌──────────────┐  ┌────────────────┐  │
│  │ Regex        │  │ Keywords     │  │ Gazetteer      │  │
│  │ (always on)  │  │ (~700 terms) │  │ (name lists)   │  │
│  │ 10 PII types │  │ health/child │  │ first/last     │  │
│  │ conf = 1.0   │  │ 9 languages  │  │ optional       │  │
│  └──────┬───────┘  └──────┬───────┘  └──────┬─────────┘  │
│         │                 │                 │            │
│         └────────┬────────┴────────┬────────┘            │
│                  ▼                 ▼                      │
│         ┌──────────────────────────────┐                 │
│         │    Semantic Backend           │                 │
│         │    (one of, tier-dependent):  │                 │
│         │                              │                 │
│         │  ┌─────────┐  ┌───────────┐  │                 │
│         │  │ NER      │  │ CRF       │  │                 │
│         │  │ TinyBERT │  │ Viterbi   │  │                 │
│         │  │ or       │  │ ~2ms      │  │                 │
│         │  │ DistilBE │  │ <10MB RAM │  │                 │
│         │  └─────────┘  └───────────┘  │                 │
│         └──────────────────────────────┘                 │
│                        │                                 │
│                        ▼                                 │
│         ┌──────────────────────────────┐                 │
│         │  Overlap Resolution          │                 │
│         │  • Union-find clustering     │                 │
│         │  • Confidence voting         │                 │
│         │  • Agreement bonus (+0.15)   │                 │
│         │  • min_confidence filter     │                 │
│         └──────────────────────────────┘                 │
└──────────────────────────────────────────────────────────┘
    │
    ▼
FPE Encryption (see FPE Configuration)
```

**Always-on layers:** Regex scanner (10 structured PII types, confidence 1.0) and keyword dictionary (health/child terms) run on every tier. They require no model files and no significant RAM.

**Semantic backend:** One of NER or CRF runs depending on tier and config. If neither can load (missing model files), the scanner degrades gracefully to regex + keywords only. Startup is never blocked by a model failure.

---

## Text Detection Engines

### NER (TinyBERT / DistilBERT)

Neural Named Entity Recognition via ONNX Runtime. Detects Person, Location, Organization, Health, and Child entities using a fine-tuned transformer with an 11-label BIO schema.

| Variant | Params | Size (INT8) | Latency (p50) | F1 | RAM per session |
|---------|--------|-------------|---------------|-----|----------------|
| TinyBERT (4L-312D) | 14.5M | 13.7MB | ~0.8ms | 85.6% | ~14MB |
| DistilBERT (6L) | 66M | 63.7MB | ~4.3ms | 91.2% | ~64MB |

**When it activates:**
- `scanner_mode = "auto"` (default) + `budget.ner_enabled = true` + model files present
- `scanner_mode = "ner"` (forced) — falls back to regex if model unavailable

**Which variant:**
- Full tier (gateway): DistilBERT (higher accuracy)
- Standard/Lite tier (gateway): TinyBERT (lower RAM)
- Embedded: DistilBERT if budget >= 120MB, else TinyBERT
- Override with `scanner.ner_model = "tinybert"` or `"distilbert"` in TOML

**Model files required:** `model_int8.onnx`, `vocab.txt`, `label_map.json` in the configured model directory.

**Pool size:** Multiple concurrent NER sessions can run in parallel. Full tier defaults to 2 sessions; Standard/Lite to 1. Override with `scanner.ner_pool_size`.

**TOML options:**

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `scanner.ner_enabled` | bool | `true` | Master switch (device budget may still disable) |
| `scanner.ner_model` | string | (auto) | Force `"tinybert"` or `"distilbert"`, overriding tier |
| `scanner.ner_model_dir` | string | `"models/ner"` | Path to DistilBERT / default NER model |
| `scanner.ner_model_dir_lite` | string | (none) | Path to TinyBERT model; falls back to `ner_model_dir` |
| `scanner.ner_pool_size` | int | `2` (Full) / `1` (Standard, Lite) | Concurrent model sessions. Each additional session adds ~14MB (TinyBERT) or ~64MB (DistilBERT) — verify total stays within tier RAM budget before increasing. |
| `scanner.ner_confidence_threshold` | float | `0.5` | Per-token confidence cutoff |

### CRF (Conditional Random Field)

Lightweight sequence labeler using hand-crafted features (word shape, prefix/suffix, capitalization, gazetteer membership, context window) and Viterbi decoding. Same 11-label BIO schema as NER.

| Metric | Value |
|--------|-------|
| Inference | ~2ms |
| RAM | <10MB |
| Model file | `crf_model.json` |

**When it activates:**
- `scanner_mode = "auto"` + NER unavailable + `budget.crf_enabled = true`
- `scanner_mode = "crf"` (forced) — falls back to regex if model unavailable
- All gateway tiers have CRF enabled as a fallback

**Tradeoffs vs NER:**
- Much lower RAM and no ONNX Runtime dependency
- Deterministic (no neural network variance)
- Lower recall than NER — relies on hand-crafted features rather than learned representations
- Best as a fallback safety net, not a primary engine

**TOML options:**

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `scanner.crf_model_dir` | string | (none) | Path to directory containing `crf_model.json` |

### Regex Scanner

Pattern-based detection for 10 structured PII types. Always enabled, cannot be disabled. Produces confidence 1.0 matches with post-validation:

- **Credit card:** Luhn checksum validation
- **SSN:** Range validation (rejects 000, 666, 900+ area numbers)
- **Phone:** Requires separator or `+` prefix (avoids false positives on bare digit runs)
- **Email:** Standard RFC-style pattern
- **API key:** Known prefix patterns (`sk-`, `AKIA`, `ghp_`, etc.)
- **IPv4/IPv6, GPS, MAC, IBAN:** Format-specific patterns

### Keyword Dictionary

Hash-based O(1) lookup of ~700 health and child-related terms across 9 languages (English, Spanish, French, German, Portuguese, Japanese, Chinese, Korean, Arabic). Always enabled by default.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `scanner.keywords_enabled` | bool | `true` | Enable/disable keyword scanning |

### Name Gazetteer

Embedded first-name and last-name lists. Provides supporting evidence for NER person detections. No model files required.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `scanner.gazetteer_enabled` | bool | `true` | Enable/disable gazetteer |

### Multilingual Scanner

Language-specific PII pattern sets for 8 non-English languages. Runs after the base regex pass on every text body — no model files required.

#### How it works

1. `whatlang` trigram detection identifies the dominant language of the text with a confidence score.
2. If the detected language is not English and confidence ≥ 15%, the matching language module's patterns are applied.
3. Because `whatlang` can confuse closely related languages on short texts, a small set of confusable pairs are also scanned as companions:

   | Detected | Companion scans |
   |----------|----------------|
   | Spanish | Portuguese |
   | Portuguese | Spanish |
   | French | Spanish, Portuguese |
   | German, Japanese, Chinese, Korean, Arabic | (none) |

4. Validation functions (check digits, IBAN mod-97, Luhn) run per match to prevent false positives caused by companion scanning.
5. English text never triggers the multilingual pass — English patterns are handled entirely by the base regex scanner.

#### Supported languages and PII types

| Language | Code | PII types covered |
|----------|------|-------------------|
| Spanish | `es` | DNI, NIE, phone (+34), IBAN (ES) |
| French | `fr` | Numéro INSEE (NIR), phone (+33), IBAN (FR) |
| German | `de` | Personalausweis (IDNR), phone (+49), IBAN (DE) |
| Portuguese | `pt` | CPF, NIF, phone (+55/+351), IBAN (PT) |
| Japanese | `ja` | My Number (マイナンバー), phone (+81) |
| Chinese | `zh` | Resident ID (RIC), phone (+86) |
| Korean | `ko` | RRN (주민등록번호), phone (+82) |
| Arabic | `ar` | National ID, phone (+966/+971/+20) |

All detected spans are merged into the base scanner's output and deduplicated using the same overlap resolution as the main ensemble. Multilingual matches carry confidence `1.0` (post-validation).

#### Multilingual Scanner Configuration

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `scanner.enabled_languages` | string[] | `[]` (all) | Restrict the multilingual pass to the listed ISO 639-1 codes. Empty list activates all 8 languages. |

**Default (all languages active):**

```toml
# Not set — all 8 non-English languages are eligible
```

**Restrict to Spanish and French only:**

```toml
[scanner]
enabled_languages = ["es", "fr"]
```

When a code is listed, only that language's patterns run — its confusable companions are also suppressed unless they are explicitly listed. For example, `["es"]` scans Spanish patterns but not Portuguese even though they would normally companion-scan together.

**Disable multilingual scanning entirely:**

There is no `multilingual_enabled = false` flag. To disable the multilingual pass completely, list a single code that will never be detected in your agent's traffic. Alternatively, set `scanner_mode = "regex"` — this limits the scanner to the base regex engine and the keyword dictionary with no language detection overhead.

**Mobile (embedded model):**

The same `enabled_languages` field is available in `MobileConfig`:

```swift
var config = MobileConfig()
config.enabled_languages = ["ja", "zh"]   // Only Japanese and Chinese
```

```kotlin
val config = MobileConfig(enabledLanguages = listOf("ja", "zh"))
```

---

## Image Detection Engines

The image pipeline runs on base64-encoded images found in JSON request bodies. Phases execute sequentially — each model is loaded, used, and evicted before the next loads (never two models in RAM simultaneously).

### Phase 0: NSFW Classifier

ViT-base 5-class image classifier (drawings, hentai, neutral, porn, sexy). If nudity is detected, the entire image is solid-filled and Phases 1–2 are skipped.

| Metric | Value |
|--------|-------|
| Model | ViT-base (LukeJacob2023/nsfw-image-detector) |
| Input | Full image (224x224 normalized) |
| Threshold | P(hentai) + P(porn) + P(sexy) >= 0.50 |
| Action | Full-image solid fill |

**When it activates:** `image.enabled = true` AND `budget.nsfw_enabled = true` (Full and Standard tiers).

**TOML options:**

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `image.enabled` | bool | `true` | Master switch for entire image pipeline |
| `image.nsfw_model_dir` | string | `"models/nsfw_classifier"` | Path to NSFW model |
| `image.nsfw_threshold` | float | `0.50` | Sum-of-classes confidence threshold |

### Phase 1: Face Detection

Detects faces and applies solid-fill redaction with 15% bounding-box expansion. If face area exceeds 80% of the image, full-image solid fill is applied instead of selective redaction.

| Model | Size | Input | Tiers |
|-------|------|-------|-------|
| SCRFD-2.5GF | ~3MB | 640x640 | Full, Standard |
| Ultra-Light RFB-320 | ~1.2MB | 320x240 | Lite |
| BlazeFace (fallback) | 230KB INT8 | 128x128 | Auto-fallback on error |

**When it activates:** `image.enabled = true` AND `budget.image_pipeline_enabled = true`.

**Fallback chain:** SCRFD → Ultra-Light → BlazeFace. If the tier-selected model fails to load, the next model in the chain is tried automatically.

**TOML options:**

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `image.face_model_dir` | string | `"models/blazeface"` | BlazeFace model path |
| `image.face_model_dir_scrfd` | string | `"models/scrfd"` | SCRFD model path |
| `image.face_model_dir_ultralight` | string | `"models/ultralight"` | Ultra-Light model path |

### Phase 2: OCR Text Detection

PaddleOCR v4 detects text regions in images. Two operating modes depending on tier:

| Mode | What it does | Tiers |
|------|-------------|-------|
| `full_recognition` | Detect text regions → recognize characters → scan for PII → selectively redact PII regions only | Full, Standard |
| `detect_and_fill` | Detect text regions → solid-fill all text (no recognition) | Lite |

Text regions get 50% vertical padding before redaction to ensure full coverage.

**When it activates:** `image.enabled = true` AND `budget.image_pipeline_enabled = true`.

**TOML options:**

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `image.ocr_model_dir` | string | `"models/paddleocr"` | PaddleOCR model directory |

### EXIF Stripping

Automatic — the `image` crate strips EXIF metadata during decode. No configuration needed. Runs on all tiers regardless of whether the rest of the image pipeline is enabled.

### Model Idle Eviction

Models are evicted from RAM after an idle timeout to free memory between requests.

| Tier | Timeout |
|------|---------|
| Full | 300s (5 min) |
| Standard | 120s (2 min) |
| Lite | 60s (1 min) |

Override with `image.model_idle_timeout_secs` in TOML.

---

## Runtime Fallback Conditions

This section documents every code path where the semantic backend selection or CRF invocation changes behavior at request time — after startup is complete. Understanding these conditions matters for tuning pool sizes, diagnosing WARN log entries, and reasoning about detection coverage under load.

### Architecture note: backend selection is fixed at startup

The `SemanticBackend` inside `HybridScanner` is an enum — either `Ner(NerPool)` or `Crf(CrfScanner)` — set once during `build_scanner()` and not changed for the lifetime of the process. There is **no per-request dynamic switch from NER to CRF**. If NER is the configured backend, a failed NER inference drops the semantic results for that request; it does not fall through to CRF. CRF is only active when it was selected at startup.

### Condition 1: NER pool exhausted (runtime)

**Source:** `ner_scanner.rs` — `NerPool::acquire()`, line ~731; `hybrid_scanner.rs` — semantic backend dispatch, line ~198.

`NerPool::acquire()` tries to check out a session from the pool. If all sessions are busy, it blocks on a `Condvar` with a **2-second timeout**. When the timeout expires:

- `acquire()` returns `None`.
- A WARN is emitted: `"NER pool exhausted after 2s, falling back to regex-only"` with `pool_size=N`.
- The semantic match list for this request is silently set to empty.
- Regex scanner, keyword dictionary, and gazetteer results are still returned normally.
- **CRF is not invoked.** The backend is NER; there is no per-request switch.

A contention warning (not a fallback) is also logged if a session is acquired but waited more than 100ms: `"NER pool contention"`.

**Mitigation:** Increase `scanner.ner_pool_size` to reduce contention. Full-tier defaults to 2 sessions; Standard/Lite to 1. Each additional session adds ~14MB (TinyBERT) or ~64MB (DistilBERT) to peak RAM. Note: pool exhaustion silently reduces detection to regex + keywords + gazetteer for that request — CRF is not invoked as a fallback. See [Fail Behavior Reference](../reference/fail-behavior-reference.md) for the full per-subsystem behavior table.

```toml
[scanner]
ner_pool_size = 4   # Add sessions to reduce pool exhaustion under load
```

### Condition 2: NER inference error (runtime)

**Source:** `ner_scanner.rs` — `scan_text_single_pass()`, line ~245; `hybrid_scanner.rs` — NER error handling, lines ~201–204.

When a pooled NER session runs inference and the ONNX Runtime returns an error:

- `NerScanner::scan_text()` returns `Err(NerError::OnnxRuntime(...))`.
- `NerPoolGuard::scan_text()` propagates the error to `HybridScanner::scan_text_with_timing()`.
- A WARN is emitted: `"NER inference failed, skipping"` with the error message.
- The semantic match list for this request is set to empty.
- Regex, keywords, and gazetteer results are still returned.
- **CRF is not invoked.**
- The pooled session is returned to the pool normally via its RAII `Drop` impl.

### Condition 3: ONNX Runtime panic during NER inference (runtime)

**Source:** `ner_scanner.rs` — `scan_text_single_pass()`, lines ~245–265.

ONNX Runtime may panic (rather than return an error) under certain error conditions. `NerScanner::scan_text_single_pass()` wraps the `session.run()` call in `std::panic::catch_unwind`:

- A panic is caught and converted to `Err(NerError::OnnxRuntime("ONNX Runtime panicked during NER inference"))`.
- This error follows the same path as Condition 2: WARN logged, semantic matches empty, request continues.
- **CRF is not invoked.**

This path is tested by `test_catch_unwind_ner_panic_to_error` in `ner_scanner.rs`.

### Condition 4: NER model absent or fails to load at startup → CRF selected (startup fallback)

**Source:** `main.rs` — `build_scanner()`, lines ~1281–1313.

This is a **startup-time** event, not a per-request one, but it determines whether CRF runs on all subsequent requests. In `scanner_mode = "auto"`:

1. If `budget.ner_enabled` is true, `try_load_ner_pool()` attempts to load the model. If the model directory is missing or the model file fails to load (parse error, size guard, I/O error), `try_load_ner_pool()` returns `None`.
2. If NER fails to load and `budget.crf_enabled` is true, `try_load_crf()` is attempted.
   - If it succeeds → CRF becomes the semantic backend for all requests. Log: `"NER model unavailable, falling back to CRF"`.
   - If CRF also fails → regex + keywords only. Log: `"No semantic model available, using regex+keywords only"`.
3. If `budget.ner_enabled` is false but `budget.crf_enabled` is true (e.g., embedded Lite with 25–80MB budget) → CRF is selected directly without trying NER.

**Conditions that trigger NER load failure:**
- Model directory does not exist (e.g., `models/ner/model_int8.onnx` missing).
- Model file exceeds the 70MB size guard (signals wrong model placed there — e.g., BERT-base instead of TinyBERT/DistilBERT).
- `vocab.txt` missing or unparseable.
- ONNX Runtime fails to initialize the session (corrupt file, incompatible ORT version).
- `scanner_mode = "crf"` forced explicitly.

**Effect:** Once CRF is selected at startup, it runs on every request as the semantic engine. `CrfScanner::scan_text()` is infallible (returns `Vec<PiiMatch>` directly, no `Result`) — there is no runtime error path for CRF inference itself.

### Condition 5: CRF confidence threshold filtering (runtime)

**Source:** `crf_scanner.rs` — `push_entity()`, lines ~369–372.

During Viterbi decoding, each decoded entity's average Viterbi score is converted to a confidence via sigmoid: `1 / (1 + exp(-score))`. If the result is below `confidence_threshold` (set from `scanner.ner_confidence_threshold`, default `0.5`), the entity is silently dropped.

This is not a fallback — CRF still ran — but it is a runtime condition that affects how many CRF entities reach the voting stage. Lower values (e.g., `0.3`) recover more borderline entities at the cost of more false positives; higher values (e.g., `0.7`) are stricter.

```toml
[scanner]
ner_confidence_threshold = 0.5  # Applies to both NER softmax confidence and CRF sigmoid score
```

### Condition 6: Text chunking in NER (runtime, per-chunk)

**Source:** `ner_scanner.rs` — `scan_text()`, lines ~170–203.

NER has a 512-token context window. Text longer than `MAX_CHUNK_BYTES` (800 bytes) is split into overlapping chunks (`CHUNK_OVERLAP_BYTES` = 150 bytes) and each chunk is inferred independently. If any single chunk's inference fails (Condition 2 or 3 above), only that chunk's semantic matches are lost — other chunks continue normally.

CRF has no chunking limit. Viterbi runs on the full token sequence regardless of length, making CRF more consistent for very long text fields but without neural context.

### Summary: what remains active when NER fails at runtime

| Condition | Regex | Keywords | Gazetteer | NER | CRF |
|-----------|-------|----------|-----------|-----|-----|
| NER pool exhausted (2s timeout) | ✓ | ✓ | ✓ | ✗ this request | ✗ |
| NER inference error (ORT error) | ✓ | ✓ | ✓ | ✗ this request | ✗ |
| ONNX Runtime panic (caught) | ✓ | ✓ | ✓ | ✗ this request | ✗ |
| NER model missing at startup → CRF fallback | ✓ | ✓ | ✓ | ✗ permanently | ✓ all requests |
| CRF model also missing at startup | ✓ | ✓ | ✓ | ✗ | ✗ |
| `scanner_mode = "regex"` | ✓ | ✓ | ✓ | ✗ | ✗ |

To verify which backend is active after startup:

```bash
# Add -H "X-OpenObscure-Token: $OPENOBSCURE_AUTH_TOKEN" if auth token is configured
curl -s http://127.0.0.1:18790/_openobscure/health | python3 -m json.tool | grep scanner_mode
```

---

## Forcing a Specific Engine

The `scanner_mode` config key controls which semantic backend runs. Set it in `config/openobscure.toml` under `[scanner]`, or pass `scanner_mode` in the embedded JSON config.

| Value | Text Backend | Behavior |
|-------|-------------|----------|
| `"auto"` (default) | Tier-selected | NER if budget allows → CRF fallback → regex-only |
| `"ner"` | Force NER | Warn + fall back to regex if model unavailable |
| `"crf"` | Force CRF | Warn + fall back to regex if model unavailable |
| `"regex"` | Regex only | No semantic scanning; keywords and gazetteer still run |

```toml
[scanner]
scanner_mode = "crf"   # Force CRF backend regardless of tier
```

For embedded (mobile), pass in the config JSON:

```swift
let config = """
{"scanner_mode": "ner", "models_base_dir": "\(modelsDir)"}
"""
```

**Note:** `scanner_mode` only affects the semantic text backend (NER/CRF). It does not affect the regex scanner, keyword dictionary, gazetteer, or image pipeline — those are controlled by their own config keys and the device budget.

---

## Ensemble Confidence Voting

When multiple engines detect overlapping PII spans, the HybridScanner resolves conflicts:

1. **Cluster:** Overlapping spans are grouped via union-find
2. **Vote:** Within each cluster, the highest-confidence match per PII type wins
3. **Agreement bonus:** When 2+ engines agree on the same type at the same span, confidence gets +0.15 (capped at 1.0)
4. **Filter:** Matches below `min_confidence` are discarded

Ensemble voting (the agreement bonus) is only enabled on the Full tier. On Standard and Lite, overlaps are still resolved but without the bonus.

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `scanner.min_confidence` | float | `0.5` | Discard matches below this threshold |
| `scanner.agreement_bonus` | float | `0.15` | Bonus when 2+ engines agree |
| `scanner.respect_code_fences` | bool | `true` | Skip scanning inside markdown code blocks |

---

## Tier-to-Engine Mapping

### Gateway

| Feature | Full (≥4GB) | Standard (2–4GB) | Lite (<2GB) |
|---------|-------------|-------------------|-------------|
| **Regex** | Yes | Yes | Yes |
| **Keywords** | Yes | Yes | Yes |
| **Gazetteer** | Yes | Yes | Yes |
| **NER model** | DistilBERT | TinyBERT | TinyBERT |
| **NER pool** | 2 sessions | 1 session | 1 session |
| **CRF** | Yes (fallback) | Yes (fallback) | Yes (fallback) |
| **Ensemble voting** | Yes | No | No |
| **Face model** | SCRFD | SCRFD | Ultra-Light |
| **OCR mode** | full_recognition | full_recognition | detect_and_fill |
| **NSFW classifier** | Yes | Yes | No |
| **Screen guard** | Yes | Yes | No |
| **Voice KWS** | Yes | Yes | No |
| **Cognitive firewall** | Yes | Yes | No |
| **RAM budget** | 275MB | 200MB | 80MB |
| **Model eviction** | 300s | 120s | 60s |

### Embedded (Mobile)

Embedded budgets use 20% of device RAM, clamped to [12MB, 275MB]. Features activate conditionally based on the available budget:

| Feature | Full | Standard | Lite |
|---------|------|----------|------|
| **Regex + Keywords** | Always | Always | Always |
| **NER** | DistilBERT | Yes if budget >= 80MB | Yes if budget >= 25MB |
| **NER model** | DistilBERT | DistilBERT if >= 120MB, else TinyBERT | TinyBERT |
| **CRF** | Yes | Yes | Yes if budget >= 25MB |
| **Image pipeline** | Yes | Yes if budget >= 100MB | Yes if budget >= 40MB |
| **NSFW** | Yes | Yes if budget >= 150MB | No |
| **Voice** | Yes | Yes if budget >= 50MB | No |
| **Cognitive firewall** | Yes | Yes if budget >= 80MB | No |

For full tier definitions and how to override the auto-detected tier, see [Deployment Tiers](../get-started/deployment-tiers.md).

---

## L1 Plugin Scanner (NAPI)

The L1 TypeScript plugin has its own scanner that runs in-process within the host agent (e.g., OpenClaw). It uses **redaction** (not FPE) since tool results are internal to the agent.

| Mode | PII Types | RAM | How to enable |
|------|-----------|-----|---------------|
| JS regex (default) | 5 types (CC, SSN, phone, email, API key) | ~5MB | Built-in, always available |
| NAPI native addon | 15 types (same as L0) + NER | ~30–80MB | Install `@openobscure/scanner-napi` |

The NAPI addon is auto-detected at startup — no configuration needed. If present, it replaces the JS regex scanner transparently. It can also bridge to L0's NER endpoint for enhanced detection via `POST /_openobscure/ner`.

---

## Verifying Active Engines

### Gateway

```bash
# Add -H "X-OpenObscure-Token: $OPENOBSCURE_AUTH_TOKEN" if auth token is configured
curl -s http://127.0.0.1:18790/_openobscure/health | python3 -m json.tool
```

Look for:
- `scanner_mode` — active backend (`"ner"`, `"crf"`, or `"regex"`)
- `ner_model` — loaded model variant (`"tinybert"`, `"distilbert"`, or `null`)
- `device_tier` — detected tier (`"full"`, `"standard"`, `"lite"`)
- `image_pipeline_enabled` — whether image engines are active
- `nsfw_enabled`, `voice_enabled`, `ri_enabled` — per-feature status

### Embedded

```swift
let stats = getStats(handle: handle)
print(stats.deviceTier)        // "full", "standard", or "lite"
print(stats.piiMatchesTotal)   // total PII detections
```
