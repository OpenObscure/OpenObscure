# OpenObscure Performance Observations

> Measured latencies from test output data (`test/data/output/*/json/`), collected on
> Apple Silicon MacBook. All numbers reflect real-world pipeline execution including
> ONNX model inference, not isolated micro-benchmarks.
>
> **Last updated:** 2026-02-25

---

## Table of Contents

1. [Test Environment](#test-environment)
2. [Measurement Methodology](#measurement-methodology)
3. [Gateway Pipeline Summary](#gateway-pipeline-summary)
4. [Image Pipeline Latency](#image-pipeline-latency)
5. [Text Scanning Latency](#text-scanning-latency)
6. [Voice Pipeline Latency](#voice-pipeline-latency)
7. [FPE Encryption Latency](#fpe-encryption-latency)
8. [Response Integrity Latency](#response-integrity-latency)
9. [Embedded Pipeline Latency](#embedded-pipeline-latency)
10. [Health Endpoint Histogram](#health-endpoint-histogram)
11. [Micro-Benchmarks vs Real World](#micro-benchmarks-vs-real-world)
12. [Outliers and Cold Start](#outliers-and-cold-start)
13. [UX Impact and Optimization Opportunities](#ux-impact-and-optimization-opportunities)
14. [Timing Headers Reference](#timing-headers-reference)

---

## Test Environment

| Property | Value |
|----------|-------|
| Platform | macOS (Apple Silicon) |
| Build | `cargo build --release --features voice` |
| Config | `test/config/test_fpe.toml` |
| Upstream | Echo server (`echo_server.mjs` on port 18791) |
| Proxy port | 18790 |
| Sample count | 105 files (47 visual + 13 audio + 45 text gateway) |
| NER model | TinyBERT 4L-312D INT8 (13.7 MB, 11 labels) |
| Collection date | 2026-02-25 |

---

## Measurement Methodology

The test output JSON files contain **two independent timing systems**. Understanding
which is which is critical for interpreting the numbers.

### Proxy-Internal Timing (accurate, no upstream)

Measured inside the Rust proxy via `std::time::Instant::now()`. These are **pure
proxy processing times** — they do NOT include upstream round-trip, HTTP overhead,
or echo server latency. Extracted from `X-OO-*` response headers.

| JSON Field | Source Header | What It Measures | Includes Upstream? |
|---|---|---|---|
| `scan_us` | `x-oo-scan-us` | Text PII scan (NER + regex + keywords) | **No** |
| `fpe_us` | `x-oo-fpe-us` | FPE encryption of matches | **No** |
| `image_us` | `x-oo-image-us` | Image pipeline total | **No** |
| `nsfw_ms` | `x-oo-nsfw-ms` | NSFW model inference | **No** |
| `face_ms` | `x-oo-face-ms` | Face detection model inference | **No** |
| `ocr_ms` | `x-oo-ocr-ms` | OCR model inference | **No** |
| `voice_ms` | `x-oo-voice-ms` | Voice pipeline (decode + KWS) | **No** |
| `kws_ms` | `x-oo-kws-ms` | KWS model inference only | **No** |
| `proxy_total_us` | `x-oo-total-us` | Full request-to-response | **Yes** |

**These are the numbers to use for production latency estimation.** For example,
`face_ms: 287` in the test output is the actual SCRFD-2.5GF inference time on
Apple Silicon — the same number you would see in a production deployment.

### Script-Measured Timing (includes HTTP overhead)

Measured by test scripts using shell `date +%s%3N` or Perl `Time::HiRes` around
HTTP curl calls. These include curl startup, TCP connection, HTTP framing, proxy
processing, upstream echo round-trip, and response delivery.

| JSON Field | What It Measures | Includes Upstream? |
|---|---|---|
| `ner_scan_ms` | curl POST to `/_openobscure/ner` endpoint | **Yes** (HTTP round-trip) |
| `fpe_pass_ms` | curl POST through proxy to echo server | **Yes** (proxy + echo + HTTP) |
| `total_ms` | `ner_scan_ms + fpe_pass_ms` | **Yes** |
| `pipeline_ms` | curl POST for visual/audio tests | **Yes** |

**Do not use these for production latency estimation.** They include test
infrastructure overhead (echo server, curl process spawn, TCP setup).

### How to get proxy-only latency in production

The `X-OO-*` response headers are emitted on every proxied response in all
deployments, not just test configurations. To measure actual production latency:

1. **Per-request**: Read `X-OO-*` headers from any proxied response
2. **Aggregated**: Query `GET /_openobscure/health` for p50/p95 latency histograms
3. **Proxy overhead only**: Subtract per-feature times from `x-oo-total-us` to
   isolate upstream LLM latency: `upstream_us = total_us - scan_us - fpe_us - ri_us`

---

## Gateway Pipeline Summary

### End-to-End (proxy_total_us, includes upstream echo)

| Content Type | Samples | Median | Average | Min | Max | Unit |
|---|---|---|---|---|---|---|
| **Image (Visual PII)** | 47 | 342 | 587 | 251 | 2,738 | ms |
| **Audio (Voice PII)** | 13 | 263 | 331 | 203 | 481 | ms |
| **Text (all categories)** | 45 | 164 | 225 | 82 | 1,097 | ms |

### Proxy-Only Processing (sum of per-feature headers, no upstream)

| Content Type | Proxy Overhead Median | Breakdown |
|---|---|---|
| **Image** | ~342 ms | image_us (~160) + scan_us (~164) — runs in parallel, max dominates |
| **Audio** | ~263 ms | voice_ms (~77) + scan_us (~164) |
| **Text** | ~164 ms | scan_us (~164) + fpe_us (~0.2) |

> For text requests, the echo server overhead is minimal (~5ms on localhost).
> For images, scan_us and image_us run on the same body processing call —
> `scan_us` covers the text portions while `image_us` covers image decode/inference.

---

## Image Pipeline Latency

### Per-Model Inference (47 samples)

| Model | Purpose | Median | Average | Min | Max | Unit |
|---|---|---|---|---|---|---|
| **NudeNet 320n** | NSFW detection | 4 | 18 | 3 | 680 | ms |
| **SCRFD-2.5GF** | Face detection | 9 | 15 | 7 | 322 | ms |
| **PaddleOCR v4** | Text detection + recognition | 106 | 338 | 50 | 1,668 | ms |
| **Image total** | All phases combined | 342 | 587 | 251 | 2,738 | ms |

> First-request cold start (model compilation) accounts for outlier max values.
> Warm-path median is the representative number.

### By Image Subcategory

| Subcategory | Samples | Pipeline Median | OCR Median | Notes |
|---|---|---|---|---|
| **Faces** | 13 | 302 ms | 76 ms | Low OCR (few text regions) |
| **Screenshots** | 7 | 321 ms | 67 ms | Screen guard detects 4/7 as screenshots |
| **EXIF test images** | 12 | 342 ms | 106 ms | 4032x3024 images resized to 960 max |
| **NSFW test images** | 7 | 376 ms | 167 ms | Smallest images |
| **Documents** | 8 | 1,131 ms | 932 ms | Heavy text content (9–31 regions) |

### OCR Scales with Text Density

| File | Text Regions | OCR (ms) | Pipeline (ms) |
|---|---|---|---|
| `face_single_frontal_04` | 0 | 57 | 276 |
| `nsfw_safe_landscape_01` | 1 | 84 | 283 |
| `doc_business_card_01` | 9 | 851 | 1,051 |
| `doc_w2_form_01` | 27 | 1,022 | 1,234 |
| `screenshot_ide_code_1920x1080` | 22 | 1,668 | 1,890 |
| `doc_medical_record_01` | 31 | 1,586 | 1,799 |

### Resolution Impact

| File | Resolution | File Size | Pipeline (ms) | Image (us) |
|---|---|---|---|---|
| `face_single_frontal_01` | 800x1200 | 99 KB | 280 | 101,992 |
| `screenshot_spreadsheet_2880x1800` | 2880x1800 | 46 KB | 321 | 141,595 |
| `nsfw_positive_placeholder_01` | 640x480 | 10 KB | 491 | 310,787 |
| `exif_camera_gps` | 4032x3024 | 1.3 MB | 498 | 312,475 |

> Resolution has minimal impact now — images are resized to 960px max dimension
> before inference. Pipeline time is dominated by OCR text density, not resolution.

---

## Text Scanning Latency

> **Model: TinyBERT 4L-312D INT8 (13.7 MB, 11 labels)** — deployed 2026-02-25,
> replacing BERT-base (103 MB). See [NER Model Optimization Results](#ner-model-optimization-results-2026-02-25).

### Proxy-Internal: scan_us (from `x-oo-scan-us` header)

This is the **proxy-only** text PII scanning time. It measures `scanner.scan_json()`
inside `body.rs` — NER TinyBERT 4L ONNX inference + regex + keyword dictionary +
multilingual patterns + ensemble voting. No upstream round-trip included.

| Category | Samples | Median scan_us | Average scan_us | Min | Max | Unit |
|---|---|---|---|---|---|---|
| **PII_Detection** | 15 | 191,919 | 232,842 | 163,541 | 381,835 | us |
| **Multilingual_PII** | 8 | 137,518 | 140,805 | 136,942 | 163,466 | us |
| **Code_Config_PII** | 8 | 109,072 | 232,541 | 81,974 | 1,096,802 | us |
| **Structured_Data_PII** | 5 | 89,840 | 91,952 | 83,021 | 110,475 | us |
| **Agent_Tool_Results** | 9 | 217,654 | 355,632 | 135,757 | 949,526 | us |

> **Overall (45 text files): median 163 ms, p95 757 ms, avg 225 ms.**
> Code_Config and Agent_Tool max values are driven by large JSON bodies
> (deeply nested structures that expand the regex + JSON traversal path).
> Median values are representative.

### Script-Measured: ner_scan_ms (includes HTTP round-trip)

These are **test-script wall-clock** times measured around `curl POST` to the
`/_openobscure/ner` endpoint. They include HTTP overhead but are useful for
comparing relative scan cost across categories.

| Category | Samples | NER Median | NER Min | NER Max | Matches Median | Unit |
|---|---|---|---|---|---|---|
| **PII_Detection** | 15 | 184 | 150 | 371 | 42 | ms |
| **Multilingual_PII** | 8 | 126 | 124 | 151 | 19 | ms |
| **Code_Config_PII** | 8 | 97 | 69 | 124 | 23 | ms |
| **Structured_Data_PII** | 5 | 75 | 70 | 98 | 50 | ms |
| **Agent_Tool_Results** | 9 | 42 | 41 | 70 | 10 | ms |

### Scan Time Scales with Input Length

| File | Category | Matches | NER Scan (ms) | Proxy scan_us |
|---|---|---|---|---|
| `agent_deeply_nested_json` | Agent_Tool_Results | 5 | 42 | 163,397 |
| `network_inventory` | Structured_Data_PII | 50 | 70 | 83,021 |
| `Phone_Numbers` | PII_Detection | 42 | 178 | 191,197 |
| `Health_Keywords` | PII_Detection | 178 | 289 | 304,593 |
| `Mixed_Structured_PII` | PII_Detection | 77 | 371 | 381,835 |

> NER inference itself is <5ms (see benchmark data below). The scan_us values above
> are dominated by regex + JSON traversal + keyword dictionary + multilingual patterns.
> NER is no longer the bottleneck — regex scanning on large text bodies is.

---

## Voice Pipeline Latency

### KWS Keyword Spotting (13 samples)

| Metric | Median | Average | Min | Max | Unit |
|---|---|---|---|---|---|
| **Voice total** (decode + KWS) | 80 | 149 | 21 | 325 | ms |
| **KWS inference** | 79 | 147 | 21 | 318 | ms |
| **Audio decode overhead** | ~1 | ~2 | ~0 | ~7 | ms |

### KWS Scales with Audio Duration

| File | Duration | KWS (ms) | Voice Total (ms) | Format |
|---|---|---|---|---|
| `audio_name_single.wav` | 2.3s | 21 | 21 | WAV |
| `audio_phone_us.wav` | 6.5s | 71 | 71 | WAV |
| `audio_address_single.wav` | 6.6s | 72 | 72 | WAV |
| `audio_ssn_single.wav` | 7.1s | 75 | 75 | WAV |
| `audio_job_screening.wav` | 20.9s | 232 | 233 | WAV |
| `audio_medical_intake.wav` | 21.2s | 231 | 232 | WAV |
| `audio_customer_service.wav` | 28.2s | 313 | 315 | WAV |

### Codec Decode Overhead (WAV vs MP3 vs OGG)

| File Pair | WAV Voice (ms) | Other Voice (ms) | Decode Delta | Format |
|---|---|---|---|---|
| `audio_ssn_single` | 75 | 80 | +5 ms | MP3 |
| `audio_phone_us` | 71 | 76 | +5 ms | OGG |
| `audio_medical_intake` | 232 | 242 | +10 ms | OGG |
| `audio_customer_service` | 315 | 325 | +10 ms | MP3 |

> OGG/MP3 decoding adds 5–10ms over WAV — minimal overhead.
> WAV is essentially zero-cost decode (raw PCM read).

---

## FPE Encryption Latency

### Measured from Proxy Headers (45 text samples)

| Metric | Median | Average | Min | Max | Unit |
|---|---|---|---|---|---|
| **FPE (fpe_us)** | 204 | 224 | 6 | 883 | us |

FPE operates in the low-microsecond range — negligible relative to scan and image pipeline.

### By Category

| Category | Median fpe_us | Max fpe_us |
|---|---|---|
| **PII_Detection** | 74 | 883 |
| **Multilingual_PII** | 255 | 349 |
| **Code_Config_PII** | 272 | 375 |
| **Structured_Data_PII** | 311 | 395 |
| **Agent_Tool_Results** | 145 | 223 |

> Higher match counts do not always produce higher FPE time — FPE latency depends
> on the number of FPE-eligible types (CC, SSN, phone, email, IP) vs label-only types
> (names, locations, organizations, health keywords).

---

## Response Integrity Latency

| Component | Documented Latency | Notes |
|---|---|---|
| **R1 Persuasion Dictionary** | <1ms | Pattern matching, runs on every response |
| **R2 TinyBERT Classifier** | ~30ms | FP32 ONNX, conditional cascade based on sensitivity |

> R2 activation: `high` = every response; `medium` = R1 flags + 10% sample;
> `low` = R1 flags only; `off` = skip all.

---

## Embedded Pipeline Latency

### L1 TypeScript Plugin — Regex Only (45 samples)

| Metric | Value | Unit |
|---|---|---|
| **Median elapsed_ms** | 0 | ms |
| **Average elapsed_ms** | 0.04 | ms |
| **Max elapsed_ms** | 1 | ms |
| **Sub-millisecond rate** | 43/45 (96%) | — |

All 45 embedded test files completed in under 1ms. The L1 regex scanner
is faster than the Gateway hybrid scanner because it skips NER model
inference and JSON traversal. With TinyBERT 4L, the gap is now ~2 orders
of magnitude (0ms vs ~164ms median) rather than the previous ~4 orders.

### L1 with NER Bridge (estimated)

When `redactPiiWithNer()` calls the L0 proxy's NER endpoint:

| Component | Estimated Latency | Notes |
|---|---|---|
| HTTP round-trip to L0 | 1–50ms | localhost, depends on connection reuse |
| L0 NER inference | 42–371ms | Same as Gateway text scan (TinyBERT 4L) |
| curl timeout | 2,000ms max | Hard limit in `--max-time 2` |
| execFileSync timeout | 3,000ms max | Process-level timeout |

### Mobile (lib_mobile.rs via UniFFI)

| Method | Expected Latency | Notes |
|---|---|---|
| `sanitize_text()` — regex only (Lite) | <1ms | Same regex engine as Gateway |
| `sanitize_text()` — with CRF (Standard) | 5–20ms | CRF feature extraction + Viterbi |
| `sanitize_text()` — with NER (Full) | 10–50ms | TinyBERT on mobile ONNX EP |
| `sanitize_image()` | 100–350ms | Same pipeline as Gateway, mobile EPs |
| `restore_text()` | <1ms | String replacement from mapping |

> Mobile timing is estimated from Gateway model inference. No instrumentation
> currently exists in `lib_mobile.rs` — `ImageStats` is collected but discarded.

### Planned: Embedded Voice (Phase 13)

| Engine | Target Latency | Model Size | Status |
|---|---|---|---|
| **openWakeWord WASM** | <1.5s detection | ~6MB | PENDING |
| **sherpa-onnx WASM** (fallback) | Similar to Gateway KWS | ~10–15MB | PENDING |
| **iOS SFSpeechRecognizer** | OS-dependent (~1–2x real-time) | 0MB (system) | PENDING |
| **Android SpeechRecognizer** | OS-dependent | 0MB (system) | PENDING |

---

## Health Endpoint Histogram

Percentiles from the `GET /_openobscure/health` latency histogram (16 geometric
buckets, coarse bin-center approximations):

| Metric | p50 | p95 |
|---|---|---|
| **Text scan** | 250 ms | 500 ms |
| **Face detection** | 10 ms | 10 ms |
| **OCR** | 250 ms | 2,500 ms |
| **NSFW** | 5 ms | 5 ms |
| **FPE** | 0.25 ms | 1 ms |
| **Image total** | 250 ms | 2,500 ms |
| **Request total** | 500 ms | 2,500 ms |
| **Voice** | 100 ms | 500 ms |

> Histogram uses geometric buckets — values are approximate bin-center estimates.
> Text scan dropped from 2,500ms to 250ms p50 after TinyBERT 4L deployment.

---

## Micro-Benchmarks vs Real World

Criterion micro-benchmarks (`cargo bench`) measure isolated operations on minimal
inputs. They test a different thing than per-feature proxy headers:

| What | Criterion Bench | Proxy Header (`x-oo-*`) |
|---|---|---|
| **Scope** | Single regex/FPE operation | Full NER + regex + JSON traversal |
| **Input size** | ~10 words | Full message body (100–5,000 words) |
| **Models** | None (regex only) | TinyBERT ONNX + CRF + keyword dict |
| **Includes upstream** | No | No |
| **Use case** | Regression detection | Production latency estimation |

### Comparison

| Operation | Criterion Bench | Proxy-Internal (x-oo-scan-us) | Factor |
|---|---|---|---|
| **Regex scan (single SSN)** | 0.23 us | — | — |
| **Regex scan (3 types mixed)** | 0.55 us | — | — |
| **Full text scan (NER + regex)** | — | 163,466 us median | — |
| **FPE encrypt (single CC)** | 13.4 us | — | — |
| **FPE (full request, x-oo-fpe-us)** | — | 204 us median | — |

The gap between criterion regex (0.55us) and proxy scan_us (163,466us) reflects
the difference between a single regex match vs full-body processing: NER TinyBERT 4L
inference (<5ms), regex across all patterns, JSON traversal, keyword dictionary,
multilingual patterns, ensemble voting. With BERT-base replaced by TinyBERT 4L,
NER is no longer the dominant factor — regex + JSON traversal on the full body is.

**Use criterion benchmarks for regression detection, proxy headers for latency estimation.**

---

## Outliers and Cold Start

### Cold Start Effect

The first request after proxy startup shows elevated inference times because ONNX
Runtime compiles CoreML models lazily:

| Model | Cold Start | Warm Steady-State | Delta |
|---|---|---|---|
| **NSFW (NudeNet)** | 680 ms | 3–5 ms | ~150x |
| **Face (SCRFD)** | 322 ms | 8–9 ms | ~35x |
| **OCR (PaddleOCR v4)** | 1,534 ms | 55–85 ms | ~20x |

The first image request triggers CoreML compilation for all three models.
Subsequent requests benefit from the compiled model cache.

### Document Image Outliers

Document images with dense text content dominate the pipeline due to OCR:

| File | Text Regions | OCR (ms) | Pipeline (ms) |
|---|---|---|---|
| `doc_medical_record_01` | 31 | 1,586 | 1,799 |
| `screenshot_ide_code_1920x1080` | 22 | 1,668 | 1,890 |
| `doc_w2_form_01` | 27 | 1,022 | 1,234 |

> Most non-document images process in 250–400ms. Dense documents with 20+ text
> regions can take 1–2s due to PaddleOCR recognition on each region.

### Text Gateway Outliers

The highest scan_us values are driven by large JSON body processing:

| File | scan_us | proxy_total_us | Script total_ms | Likely Cause |
|---|---|---|---|---|
| `sample_terraform.json` | 1,096,802 | 1,098,312 | 1,210 | Large nested JSON body |
| `agent_tool_result_network_scan` | 949,526 | 950,319 | 1,034 | Large tool result payload |
| `agent_tool_result_database_query` | 756,785 | 757,616 | 843 | Large query results |

> The small delta between `scan_us` and `proxy_total_us` (~1–2ms) confirms the
> echo server adds negligible overhead on localhost. These outliers are driven by
> regex scanning on large JSON structures, not NER inference.

### Stable scan_us Across Visual PII

The proxy-internal `scan_us` for visual PII tests is remarkably stable at ~164ms
(range 161,710–167,218 us) across all 47 samples. This represents the text scan
running on the JSON body wrapper. The variance in `proxy_total_us` comes from
`image_us` (which depends on resolution and text density), not `scan_us`.

### Isolating Upstream LLM Latency in Production

In production (real LLM upstream instead of echo server), calculate:

```
upstream_ms = (proxy_total_us - scan_us - fpe_us - ri_us) / 1000
```

The per-feature headers remain identical regardless of upstream — they measure
proxy processing before the upstream call (`scan_us`, `fpe_us`, `image_us`,
`voice_ms`) or after it returns (`ri_us`). Only `total_us` includes the upstream
round-trip.

---

## UX Impact and Optimization Opportunities

### Impact Assessment (Post TinyBERT 4L Optimization)

The proxy adds **~164ms median** to text requests (down from ~2.3s with the previous
BERT-base model). In the context of typical LLM API calls (3–15s for generation):

| LLM Latency | Proxy Overhead | Total | Overhead % | User Perception |
|---|---|---|---|---|
| 3s (fast model) | 0.16s | 3.16s | +5% | Imperceptible |
| 5s (typical) | 0.16s | 5.16s | +3% | Imperceptible |
| 10s (complex) | 0.16s | 10.16s | +2% | Imperceptible |
| 15s (long gen) | 0.16s | 15.16s | +1% | Imperceptible |

> Text scanning overhead is now **negligible** for all practical LLM workloads.
> The previous 2.3s bottleneck has been eliminated entirely.

### Where the Time Goes

With TinyBERT 4L-312D, NER inference is no longer the bottleneck. Regex + JSON
traversal on large payloads now dominates scan time.

| Component | Median Latency | % of scan_us | Acceptable? |
|---|---|---|---|
| **Regex + keywords + multilingual** | ~100–300 ms | ~85% | Yes |
| **NER TinyBERT 4L inference** | <5 ms | ~3% | Yes |
| **JSON traversal (deep/nested)** | 10–50 ms | ~10% | Yes |
| **CRF scanner** | ~5–20 ms | ~5% | Yes |
| **Ensemble voting** | <1 ms | <0.1% | Yes |
| **FPE encryption** | ~0.2 ms | — | Yes |
| **KWS voice** | ~77 ms | — | Yes |
| **Image pipeline** | ~300 ms | — | Separate path |

### What Works Well

- **All tiers**: text scan overhead is now under 400ms median for all tiers
- **Lite tier** (regex only): <1ms text scan — zero perceptible overhead
- **Image pipeline**: runs in parallel with text scan, does not add serially
- **Voice KWS**: 77ms median is well under human perception threshold
- **FPE**: 0.2ms is negligible at any scale
- **Streaming responses**: proxy overhead is request-side only; response streaming
  is unaffected since scan happens before upstream forwarding

### Remaining Optimization Opportunities

With NER eliminated as a bottleneck, the main targets are large payload handling:

#### 1. Conditional NER (Low Impact — already fast)

NER adds <5ms, so conditional skipping saves negligible time now. Still useful for
reducing ONNX session overhead on high-throughput servers.

#### 2. Async/Streaming Scan (Medium Impact for Large Payloads)

Scan request body while streaming to upstream, rather than buffering the full body
first. Proxy currently buffers → scans → forwards. For large JSON payloads (>64KB),
streaming would overlap scan time with upstream processing.

#### 3. Regex Optimization for Large Bodies (Medium Impact)

The largest scan times (1.1s for `sample_terraform.json`, 950ms for
`agent_tool_result_network_scan.json`) are driven by regex on large JSON structures.
Parallelizing regex across JSON value chunks could reduce these outliers.

### NER Model Optimization Results (2026-02-25)

The deployed model was identified as **BERT-base (~110M params, 103MB INT8, 1391 ONNX nodes)**,
not TinyBERT as documented. Two replacement models were fine-tuned on CoNLL-2003 + custom
PII data (740 training samples + 20 HEALTH/CHILD) and benchmarked:

| Model | Params | INT8 Size | p50 Latency | Test F1 | Entities/sample |
|---|---|---|---|---|---|
| **BERT-base (previous)** | ~110M | 103 MB | ~2,300 ms | ~92% | — |
| **TinyBERT 4L-312D (deployed)** | 14.5M | 13.7 MB | **0.8 ms** | 85.6% | 1.8 |
| **DistilBERT-base (comparison)** | 66M | 63.7 MB | 4.3 ms | 91.2% | 2.3 |

> Benchmark: 760 sentences from PII corpus, ONNX Runtime CPUExecutionProvider, 5 warmup iterations.
> Latency is model inference only (Python tokenizer + ONNX session.run), not proxy-internal timing.

**Decision**: TinyBERT 4L-312D deployed. The 6.5% F1 drop vs BERT-base is acceptable because:
1. NER only catches semantic entities (names, locations, orgs) — regex handles structured PII
2. Regex scanner achieves 99.7% recall independently
3. The ~2,875x speedup (2,300ms → 0.8ms) eliminates NER as a latency bottleneck
4. Model now supports HEALTH and CHILD entity types (11-label schema vs previous 9-label)

### Proxy-Measured Before/After Comparison

Real gateway test results with all 45 text files (proxy-internal `x-oo-scan-us`):

| Metric | BERT-base (before) | TinyBERT 4L (after) | Improvement |
|---|---|---|---|
| **Median scan_us** | 2,320,000 us (2.3s) | 163,466 us (163ms) | **14.2x faster** |
| **Average scan_us** | 3,214,000 us (3.2s) | 225,330 us (225ms) | **14.3x faster** |
| **p95 scan_us** | 5,486,000 us (5.5s) | 756,785 us (757ms) | **7.2x faster** |
| **Min scan_us** | 1,187,000 us (1.2s) | 81,974 us (82ms) | **14.5x faster** |
| **Max scan_us** | 14,952,000 us (15.0s) | 1,096,802 us (1.1s) | **13.6x faster** |
| **Model size** | 103 MB | 13.7 MB | **7.5x smaller** |
| **Total matches** | 753 | 1,454 | More (11-label schema) |

> The 14x improvement exceeds the pure NER inference speedup (2,875x) because
> scan_us includes regex + JSON traversal + ensemble. With NER removed as the
> dominant factor, the regex/traversal cost is now the baseline — and it was
> always fast.

### Tier Latency Summary (Updated)

| Tier | Text Scan | Image | Voice | Total Proxy Overhead |
|---|---|---|---|---|
| **Lite** (regex only) | <1 ms | N/A | N/A | <1 ms |
| **Standard** (regex + CRF) | 5–20 ms | 100–350 ms | 77 ms | <400 ms |
| **Full** (NER + regex + CRF) | **~164 ms** | ~300 ms | 77 ms | **<400 ms** |

> **Full tier text scan is now ~164ms median**, down from 2.3s. NER inference
> adds <5ms — comparable to CRF. The previous 2.3s NER bottleneck is eliminated.
>
> **Recommendation**: Full tier is now viable for all deployments. Text scanning
> overhead is imperceptible (<5% of typical LLM response time).

---

## Timing Headers Reference

All headers emitted by the Gateway proxy on every response. Only non-zero values
are included. These headers are available in **all deployments** (test and production).

### Request Processing Timeline

```
Request arrives (request_start = Instant::now())
    |
    |-- [1] Image pipeline ..................... x-oo-image-us (proxy-only)
    |       |-- NSFW detection ................ x-oo-nsfw-ms  (proxy-only)
    |       |-- Face detection + redaction .... x-oo-face-ms  (proxy-only)
    |       +-- OCR detection + redaction ..... x-oo-ocr-ms   (proxy-only)
    |
    |-- [2] Voice pipeline .................... x-oo-voice-ms (proxy-only)
    |       +-- KWS inference only ............ x-oo-kws-ms   (proxy-only)
    |
    |-- [3] Text PII scan (NER+regex+CRF) .... x-oo-scan-us  (proxy-only)
    |
    |-- [4] FPE encryption .................... x-oo-fpe-us   (proxy-only)
    |
    |-- [5] >>> Forward to upstream LLM >>>
    |-- [6] <<< Receive upstream response <<<     (not measured separately)
    |
    |-- [7] Response integrity scan ........... x-oo-ri-us    (proxy-only)
    |
    +-- Response sent ......................... x-oo-total-us (INCLUDES upstream)
```

**Proxy-only overhead** = `scan_us + fpe_us + image_us + voice_us + ri_us`

**Upstream LLM latency** = `total_us - (scan_us + fpe_us + image_us + voice_us + ri_us)`

### Header Reference Table

| Header | Unit | Proxy-Only? | What It Measures | Source |
|---|---|---|---|---|
| `x-oo-scan-us` | us | Yes | Text PII scanning (NER + regex + keywords + ensemble) | `body.rs:102–104` |
| `x-oo-fpe-us` | us | Yes | FPE encryption of PII matches | `body.rs:127–180` |
| `x-oo-image-us` | us | Yes | Image pipeline total (decode + NSFW + face + OCR + encode) | `body.rs:56–66` |
| `x-oo-nsfw-ms` | ms | Yes | NSFW/nudity detection per image | `image_pipeline.rs:271–329` |
| `x-oo-face-ms` | ms | Yes | Face detection + redaction per image | `image_pipeline.rs:333–381` |
| `x-oo-ocr-ms` | ms | Yes | OCR text detection + redaction per image | `image_pipeline.rs:384–499` |
| `x-oo-voice-ms` | ms | Yes | Voice pipeline total (audio decode + KWS) | `voice_pipeline.rs:57–118` |
| `x-oo-kws-ms` | ms | Yes | KWS keyword spotting inference only | `voice_pipeline.rs:57–118` |
| `x-oo-ri-us` | us | Yes | Response integrity scan (R1 dict + R2 TinyBERT) | `proxy.rs:392–398` |
| `x-oo-total-us` | us | **No** | Wall-clock request arrival to response sent | `proxy.rs:61→423` |

Header injection: `proxy.rs` lines 707–737 (`inject_timing_headers()`)

### Reading Headers in Production

```bash
# Single request — extract all timing headers
curl -s -D - https://your-proxy/anthropic/v1/messages \
  -H "Content-Type: application/json" \
  -d '{"model":"claude-sonnet-4-20250514","max_tokens":100,"messages":[{"role":"user","content":"test"}]}' \
  2>/dev/null | grep -i "x-oo-"

# Health endpoint — aggregated p50/p95 histograms
curl -s https://your-proxy/_openobscure/health | jq '{
  text_scan_p50: .text_scan_latency_p50_us,
  text_scan_p95: .text_scan_latency_p95_us,
  face_p50: .face_latency_p50_us,
  ocr_p50: .ocr_latency_p50_us,
  fpe_p50: .fpe_latency_p50_us,
  request_p50: .request_latency_p50_us,
  request_p95: .request_latency_p95_us
}'
```
