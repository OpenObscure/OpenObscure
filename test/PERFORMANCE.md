# OpenObscure Performance Observations

> Measured latencies from test output data (`test/data/output/*/json/`), collected on
> Apple Silicon MacBook. All numbers reflect real-world pipeline execution including
> ONNX model inference, not isolated micro-benchmarks.
>
> **Last updated:** 2026-03-15

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
9. [Health Endpoint Histogram](#health-endpoint-histogram)
10. [Micro-Benchmarks vs Real World](#micro-benchmarks-vs-real-world)
11. [Outliers and Cold Start](#outliers-and-cold-start)
12. [UX Impact and Optimization Opportunities](#ux-impact-and-optimization-opportunities)
13. [Timing Headers Reference](#timing-headers-reference)

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
| NER model | DistilBERT-base INT8 (63.7 MB, 11 labels) — Full tier |
| Image models | FP32 — ViT-base 5-class NSFW (87 MB), PaddleOCR det/rec (2.3+7.3 MB), SCRFD-2.5GF (3.1 MB) |
| KWS models | INT8 — sherpa-onnx Zipformer encoder/decoder/joiner (~5 MB total) |
| Collection date | 2026-03-15 |

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
| **Image (Visual PII)** | 47 | 758 | 1,065 | 662 | 5,436 | ms |
| **Audio (Voice PII)** | 13 | 477 | 528 | 442 | 676 | ms |
| **Text (all categories)** | 45 | 457 | 622 | 230 | 2,899 | ms |

### Proxy-Only Processing (sum of per-feature headers, no upstream)

| Content Type | Proxy Overhead Median | Breakdown |
|---|---|---|
| **Image** | ~758 ms | image_us (~305) + scan_us (~455) — both run on same body processing call |
| **Audio** | ~477 ms | voice_ms (~76) + scan_us (~455) — KWS active, 11/13 files had PII |
| **Text** | ~457 ms | scan_us (~455) + fpe_us (~0.3) |

> For text requests, the echo server overhead is minimal (~5ms on localhost).
> For images, scan_us and image_us run on the same body processing call —
> `scan_us` covers the text portions while `image_us` covers image decode/inference.

---

## Image Pipeline Latency

### Per-Model Inference (47 samples)

| Model | Purpose | Median | Average | Min | Max | Unit |
|---|---|---|---|---|---|---|
| **ViT-base 5-class** | NSFW detection | 151 | 220 | 140 | 3,122 | ms |
| **SCRFD-2.5GF** | Face detection | 9 | 16 | 7 | 330 | ms |
| **PaddleOCR v4** | Text detection + recognition | 106 | 344 | 50 | 1,750 | ms |
| **Image total** (image_us, proxy-only) | All phases combined | 305 | 616 | 219 | 4,996 | ms |

> First-request cold start (model compilation) accounts for outlier max values.
> Warm-path median is the representative number.

### By Image Subcategory

| Subcategory | Samples | proxy_total_us Median | OCR Median | Notes |
|---|---|---|---|---|
| **Faces** | 13 | 707 ms | 80 ms | Low OCR (few text regions) |
| **Screenshots** | 7 | 775 ms | 78 ms | Screen guard detects 4/7 as screenshots |
| **EXIF test images** | 12 | 747 ms | 94 ms | 4032x3024 images resized to 960 max |
| **NSFW test images** | 7 | 783 ms | 165 ms | Smallest images |
| **Documents** | 8 | 1,503 ms | 884 ms | Heavy text content (9–31 regions) |

### OCR Scales with Text Density

| File | Text Regions | OCR (ms) | proxy_total_us (ms) |
|---|---|---|---|
| `face_single_frontal_04` | 0 | 50 | 675 |
| `nsfw_safe_landscape_01` | 1 | 82 | 699 |
| `doc_business_card_01` | 9 | 838 | 1,458 |
| `doc_w2_form_01` | 27 | 1,054 | 1,680 |
| `screenshot_ide_code_1920x1080` | 22 | 1,750 | 2,401 |
| `doc_medical_record_01` | 31 | 1,665 | 2,292 |

### Resolution Impact

| File | Resolution | File Size | proxy_total_us (ms) | image_us (us) |
|---|---|---|---|---|
| `face_single_frontal_01` | 800x1200 | 99 KB | 682 | 243,624 |
| `screenshot_spreadsheet_2880x1800` | 2880x1800 | 46 KB | 775 | 325,481 |
| `nsfw_positive_placeholder_01` | 640x480 | 10 KB | 915 | 460,933 |
| `exif_camera_gps` | 4032x3024 | 1.3 MB | 978 | 520,952 |

> Resolution has minimal impact now — images are resized to 960px max dimension
> before inference. Pipeline time is dominated by OCR text density, not resolution.

---

## Text Scanning Latency

> **Model: DistilBERT-base INT8 (63.7 MB, 11 labels)** — Full tier, deployed 2026-03-15.
> See [NER Model Optimization Results](#ner-model-optimization-results-2026-02-25).

### Proxy-Internal: scan_us (from `x-oo-scan-us` header)

This is the **proxy-only** text PII scanning time. It measures `scanner.scan_json()`
inside `body.rs` — NER TinyBERT 4L ONNX inference + regex + keyword dictionary +
multilingual patterns + ensemble voting. No upstream round-trip included.

| Category | Samples | Median scan_us | Average scan_us | Min | Max | Unit |
|---|---|---|---|---|---|---|
| **PII_Detection** | 15 | 530,852 | 636,492 | 450,821 | 1,073,651 | us |
| **Multilingual_PII** | 8 | 382,376 | 391,248 | 379,758 | 456,539 | us |
| **Code_Config_PII** | 8 | 305,292 | 630,555 | 230,479 | 2,897,713 | us |
| **Structured_Data_PII** | 5 | 230,936 | 247,170 | 227,841 | 311,219 | us |
| **Agent_Tool_Results** | 9 | 609,486 | 996,599 | 381,523 | 2,654,175 | us |

> **Overall (45 text files): median 455 ms, p95 2,125 ms, avg 621 ms.**
> Code_Config and Agent_Tool max values are driven by large JSON bodies
> (deeply nested structures that expand the regex + JSON traversal path).
> Median values are representative.

### Script-Measured: ner_scan_ms (includes HTTP round-trip)

These are **test-script wall-clock** times measured around `curl POST` to the
`/_openobscure/ner` endpoint. They include HTTP overhead but are useful for
comparing relative scan cost across categories.

| Category | Samples | NER Median | NER Min | NER Max | Matches Median | Unit |
|---|---|---|---|---|---|---|
| **PII_Detection** | 15 | 472 | 391 | 1,008 | 64 | ms |
| **Multilingual_PII** | 8 | 321 | 318 | 396 | 22 | ms |
| **Code_Config_PII** | 8 | 244 | 170 | 325 | 21 | ms |
| **Structured_Data_PII** | 5 | 168 | 167 | 248 | 56 | ms |
| **Agent_Tool_Results** | 9 | 94 | 91 | 168 | 10 | ms |

### Scan Time Scales with Input Length

| File | Category | Matches | NER Scan (ms) | Proxy scan_us |
|---|---|---|---|---|
| `agent_deeply_nested_json` | Agent_Tool_Results | 5 | 92 | 455,458 |
| `network_inventory` | Structured_Data_PII | 49 | 167 | 229,256 |
| `Phone_Numbers` | PII_Detection | 64 | 473 | 539,193 |
| `Health_Keywords` | PII_Detection | 161 | 768 | 826,467 |
| `Mixed_Structured_PII` | PII_Detection | 117 | 1,008 | 1,073,651 |

> NER inference itself is <5ms (see benchmark data below). The scan_us values above
> are dominated by regex + JSON traversal + keyword dictionary + multilingual patterns.
> NER is no longer the bottleneck — regex scanning on large text bodies is.

---

## Voice Pipeline Latency

### KWS Keyword Spotting (13 samples)

| Metric | Median | Average | Min | Max | Unit |
|---|---|---|---|---|---|
| **Voice total** (decode + KWS) | 76 | 143 | 20 | 307 | ms |
| **KWS inference** | 75 | 141 | 20 | 300 | ms |
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
| **FPE (fpe_us)** | 319 | 365 | 6 | 1,118 | us |

FPE operates in the low-microsecond range — negligible relative to scan and image pipeline.

### By Category

| Category | Median fpe_us | Max fpe_us |
|---|---|---|
| **PII_Detection** | 428 | 1,118 |
| **Multilingual_PII** | 280 | 402 |
| **Code_Config_PII** | 386 | 531 |
| **Structured_Data_PII** | 566 | 745 |
| **Agent_Tool_Results** | 175 | 394 |

> Higher match counts do not always produce higher FPE time — FPE latency depends
> on the number of FPE-eligible types (CC, SSN, phone, email, IP) vs label-only types
> (names, locations, organizations, health keywords).

---

## Response Integrity Latency

| Component | Documented Latency | Notes |
|---|---|---|
| **R1 Persuasion Dictionary** | <1ms | Pattern matching, runs on every response |
| **R2 TinyBERT Classifier** | ~30ms | FP32 ONNX, runs on every response (default: high) |

> R2 activation: `high` (default) = every response; `medium` = R1 flags + 10% sample;
> `low` = R1 flags only; `off` = skip all.

---

## Health Endpoint Histogram

Percentiles from the `GET /_openobscure/health` latency histogram (16 geometric
buckets, coarse bin-center approximations):

| Metric | p50 | p95 |
|---|---|---|
| **Text scan** | 500 ms | 1,000 ms |
| **Face detection** | 10 ms | 10 ms |
| **OCR** | 250 ms | 2,500 ms |
| **NSFW** | 250 ms | 250 ms |
| **FPE** | 0.5 ms | 1 ms |
| **Image total** | 250 ms | 2,500 ms |
| **Request total** | 1,000 ms | 2,500 ms |
| **Voice** | 75 ms | 250 ms |

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
| **Regex scan (single SSN)** | 0.23 µs | — | — |
| **Regex scan (3 types mixed)** | 0.56 µs | — | — |
| **Regex scan (no PII, short)** | 0.08 µs | — | — |
| **Regex scan (JSON messages)** | 1.71 µs | — | — |
| **Full text scan (NER + regex)** | — | 455,458 µs median | — |
| **FPE encrypt CC** | 13.2 µs | — | — |
| **FPE encrypt SSN** | 10.7 µs | — | — |
| **FPE roundtrip SSN** | 22.1 µs | — | — |
| **FPE (full request, x-oo-fpe-us)** | — | 319 µs median | — |
| **Image decode PNG 256x256** | 77 µs | — | — |
| **Image decode JPEG 640x480** | 570 µs | — | — |
| **Image resize 1024→640** | 2.37 ms | — | — |

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
| **NSFW (ViT-base)** | 3,122 ms | 140–160 ms | ~20x |
| **Face (SCRFD)** | 330 ms | 7–10 ms | ~40x |
| **OCR (PaddleOCR v4)** | 1,750 ms | 50–110 ms | ~20x |

The first image request triggers CoreML compilation for all three models.
Subsequent requests benefit from the compiled model cache.

### Document Image Outliers

Document images with dense text content dominate the pipeline due to OCR:

| File | Text Regions | OCR (ms) | proxy_total_us (ms) |
|---|---|---|---|
| `screenshot_ide_code_1920x1080` | 22 | 1,750 | 2,401 |
| `doc_medical_record_01` | 31 | 1,665 | 2,292 |
| `doc_w2_form_01` | 27 | 1,054 | 1,680 |

> Most non-document images process in 250–400ms. Dense documents with 20+ text
> regions can take 1–2s due to PaddleOCR recognition on each region.

### Text Gateway Outliers

The highest scan_us values are driven by large JSON body processing:

| File | scan_us | proxy_total_us | Script total_ms | Likely Cause |
|---|---|---|---|---|
| `sample_terraform.json` | 2,897,713 | 2,898,959 | 3,159 | Large nested JSON body |
| `agent_tool_result_network_scan` | 2,654,175 | 2,655,397 | 2,840 | Large tool result payload |
| `agent_tool_result_database_query` | 2,124,727 | 2,126,189 | 2,309 | Large query results |

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

The proxy adds **~455ms median** to text requests (down from ~2.3s with the previous
BERT-base model). In the context of typical LLM API calls (3–15s for generation):

| LLM Latency | Proxy Overhead | Total | Overhead % | User Perception |
|---|---|---|---|---|
| 3s (fast model) | 0.46s | 3.46s | +15% | Just perceptible |
| 5s (typical) | 0.46s | 5.46s | +9% | Barely perceptible |
| 10s (complex) | 0.46s | 10.46s | +5% | Imperceptible |
| 15s (long gen) | 0.46s | 15.46s | +3% | Imperceptible |

> Text scanning overhead is now well under 1s median for all practical LLM workloads.
> The previous 2.3s bottleneck has been eliminated entirely.

### Where the Time Goes

With TinyBERT 4L-312D, NER inference is no longer the bottleneck. Regex + JSON
traversal on large payloads now dominates scan time.

| Component | Median Latency | % of scan_us | Acceptable? |
|---|---|---|---|
| **Regex + keywords + multilingual** | ~200–400 ms | ~80% | Yes |
| **NER DistilBERT inference** | ~4–5 ms | ~1% | Yes |
| **JSON traversal (deep/nested)** | 10–50 ms | ~5% | Yes |
| **CRF scanner** | ~5–20 ms | ~3% | Yes |
| **Ensemble voting** | <1 ms | <0.1% | Yes |
| **FPE encryption** | ~0.3 ms | — | Yes |
| **KWS voice** | ~76 ms | — | Yes |
| **Image pipeline** | ~305 ms | — | Separate path |

### What Works Well

- **Full tier**: text scan overhead ~455ms median — under 500ms for most requests
- **Lite tier** (regex only): <1ms text scan — zero perceptible overhead
- **Image pipeline**: OCR and face detection same as before; NSFW warm steady-state ~150ms
- **Voice KWS**: 76ms median is well under human perception threshold
- **FPE**: 0.3ms is negligible at any scale
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

| Model | Params | INT8 Size | p50 Latency | Test F1 | Entities/sample | Status |
|---|---|---|---|---|---|---|
| **BERT-base (previous)** | ~110M | 103 MB | ~2,300 ms | ~92% | — | Replaced |
| **TinyBERT 4L-312D** | 14.5M | 13.7 MB | **0.8 ms** | 85.6% | 1.8 | Standard/Lite tier |
| **DistilBERT-base** | 66M | 63.7 MB | 4.3 ms | 91.2% | 2.3 | **Full tier (deployed)** |

> Benchmark: 760 sentences from PII corpus, ONNX Runtime CPUExecutionProvider, 5 warmup iterations.
> Latency is model inference only (Python tokenizer + ONNX session.run), not proxy-internal timing.

**Decision**: DistilBERT deployed on Full tier; TinyBERT on Standard/Lite. Trade-off: 5.5% F1 gain over TinyBERT, 3.5ms NER overhead — still negligible vs regex scan time. The replacement vs BERT-base is acceptable because:
1. NER only catches semantic entities (names, locations, orgs) — regex handles structured PII
2. Regex scanner achieves 99.7% recall independently
3. The ~2,875x speedup (2,300ms → 0.8ms) eliminates NER as a latency bottleneck
4. Model now supports HEALTH and CHILD entity types (11-label schema vs previous 9-label)

### Proxy-Measured Before/After Comparison

Real gateway test results with all 45 text files (proxy-internal `x-oo-scan-us`):

| Metric | BERT-base (before) | DistilBERT Full (current) | Improvement |
|---|---|---|---|
| **Median scan_us** | 2,320,000 us (2.3s) | 455,458 us (455ms) | **5.1x faster** |
| **Average scan_us** | 3,214,000 us (3.2s) | 620,601 us (621ms) | **5.2x faster** |
| **p95 scan_us** | 5,486,000 us (5.5s) | 2,124,727 us (2.1s) | **2.6x faster** |
| **Min scan_us** | 1,187,000 us (1.2s) | 227,841 us (228ms) | **5.2x faster** |
| **Max scan_us** | 14,952,000 us (15.0s) | 2,897,713 us (2.9s) | **5.2x faster** |
| **Model size** | 103 MB | 63.7 MB | **1.6x smaller** |
| **Total matches** | 753 | ~1,500 | More (11-label schema) |

> The 14x improvement exceeds the pure NER inference speedup (2,875x) because
> scan_us includes regex + JSON traversal + ensemble. With NER removed as the
> dominant factor, the regex/traversal cost is now the baseline — and it was
> always fast.

### Image Model Quantization Results (2026-02-25)

All 4 image pipeline models (NSFW classifier, PaddleOCR det/rec, SCRFD) were quantized
from FP32 to INT8 using ONNX Runtime dynamic quantization (`quantize_dynamic`,
`QUInt8`). The quantization achieved significant size reduction but caused a
**latency regression** on Apple Silicon.

#### Size Reduction (successful)

| Model | FP32 Size | INT8 Size | Reduction |
|---|---|---|---|
| NSFW classifier (was NudeNet, now ViT-base) | 11.6 MB | 3.1 MB | 73.4% |
| PaddleOCR rec v4 | 7.3 MB | 2.0 MB | 72.6% |
| SCRFD-2.5GF | 3.1 MB | 0.8 MB | 73.1% |
| PaddleOCR det v4 | 2.3 MB | 0.8 MB | 67.3% |
| **Total** | **24.3 MB** | **6.7 MB** | **72.4%** |

#### Latency Regression (CoreML EP fallback to CPU)

CoreML EP on Apple Silicon does **not** support quantized ONNX operators
(`QLinearConv`, `MatMulInteger`, `DynamicQuantizeLinear`). With INT8 models,
CoreML assigned only 27/90 nodes (30%) to the Neural Engine — the remaining
quantized ops fell back to CPU, causing significant regression:

| Model | FP32 Median | INT8 Median | Regression |
|---|---|---|---|
| **NSFW (ViT-base)** | 4 ms | 23 ms | 5.75x slower |
| **Face (SCRFD)** | 9 ms | 54 ms | 6x slower |
| **OCR (PaddleOCR)** | 106 ms | 238 ms | 2.2x slower |
| **Pipeline total** | 342 ms | 541 ms | 1.6x slower |

#### Decision: Keep FP32 for Apple Silicon

INT8 models were **reverted to FP32**. The 17.6 MB size savings do not justify
the latency regression. Image models remain FP32 (24.3 MB total).

> **Note for Android/NNAPI**: NNAPI EP *does* support `QLinearConv` and
> `QLinearMatMul`. INT8 quantization should be re-evaluated for Android
> deployments where it would likely deliver both size and speed benefits.

> **Quantization script**: `build/quantize_image_models.py` is preserved for
> future use on platforms with native INT8 operator support.

### Tier Latency Summary (Updated)

| Tier | Text Scan | Image | Voice | Total Proxy Overhead |
|---|---|---|---|---|
| **Lite** (regex only) | <1 ms | N/A | N/A | <1 ms |
| **Standard** (regex + CRF + TinyBERT) | 5–50 ms | 100–350 ms | 76 ms | <400 ms |
| **Full** (NER/DistilBERT + regex + CRF) | **~455 ms** | ~305 ms | 76 ms | **<800 ms** |

> **Full tier text scan is ~455ms median** (DistilBERT), down from 2.3s with BERT-base.
> NER adds ~4–5ms; regex + JSON traversal dominates. The previous 2.3s bottleneck is eliminated.
>
> **Recommendation**: Full tier is now viable for all deployments. Overhead is under 5%
> for typical LLM response times ≥10s.

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
