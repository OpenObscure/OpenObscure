# Performance Reference

> **All measurements in this document were taken with `--debug-logs` enabled (debug builds, unoptimized).** Release builds (`--release`) will be faster due to compiler optimizations, LTO, and the absence of debug instrumentation. Treat these numbers as upper bounds, not production benchmarks.

---

## Test Devices

| Device | OS | RAM | CPU | Tier | NER Model |
|--------|------|-----|-----|------|-----------|
| iPhone 17 | iOS 19 | 8 GB (reports 7671 MB) | Apple A19 (6 cores) | Full | DistilBERT |
| Samsung Galaxy S25 | Android 16 | 11 GB (reports 11114 MB) | Snapdragon 8 Elite (8 cores) | Full | DistilBERT |
| Android mid-range | Android 15 | 8 GB (reports 7461 MB) | Snapdragon 7 Gen 2 (8 cores) | Full | DistilBERT |

All measurements use Ollama (llava:13b) or GPT-4o as the LLM provider. OpenObscure latency is independent of the LLM — the numbers below measure only the on-device sanitization/restore pipeline.

---

## Text Sanitization

### Per-Message NER Latency

Single user message (366 chars, 11 PII matches — the medical record test case):

| Device | Regex (us) | Keywords (us) | NER/Semantic (us) | Total (ms) |
|--------|-----------|--------------|-------------------|------------|
| iPhone 17 | 4,025 | 343 | 88,482 | **97** |
| Samsung Galaxy | 6,069 | 133 | 141,126 | **149** |
| Android mid-range | 10,236 | 317 | 570,542 | **595** |

NER (DistilBERT INT8) dominates latency. Regex and keywords are negligible (<1ms).

### Cache Behavior (Multi-Turn)

With `sanitizedContent` caching (Enchanted) or in-memory hash cache (RikkaHub interceptor):

| Turn | Messages | Cached | Scanned | iPhone (ms) | Samsung (ms) | Mid-range (ms) |
|------|----------|--------|---------|-------------|-------------|----------------|
| 1 | 1 | 0 | 1 | 97 | 149 | 595 |
| 2 | 3 | 1 | 1 | 80 | 140 | 570 |
| 3 | 5 | 2 | 0 | **0** | **0** | **0** |
| 4 | 7 | 3 | 0 | **0** | **0** | **0** |
| 10 | 19 | 9 | 0 | **0** | **0** | **0** |

Sanitization latency is constant from turn 3 onward — all prior user messages are served from cache. Only new user messages incur NER cost.

### Stable Tokens

Same plaintext produces the same token across all turns within a conversation (via `existingMappingJson` seeding):

```
Turn 1: "Angela Martinez" → PER_ft25
Turn 2: "Angela Martinez" → PER_ft25  (stable)
Turn 5: "Angela Martinez" → PER_ft25  (stable)
```

---

## Image Pipeline

### Per-Phase Latency (720x960 landscape photo, no detections)

| Phase | iPhone 17 | Samsung Galaxy | Mid-range Android |
|-------|-----------|---------------|-------------------|
| NSFW classifier (ViT INT8) | 2,169 ms | 420 ms | 1,100 ms |
| Face detection (SCRFD) | 1,993 ms | 502 ms | 1,240 ms |
| OCR pre-filter (Sobel edge) | 339 ms | 148 ms | 312 ms |
| **Pipeline total** | **4,503 ms** | **1,072 ms** | **2,575 ms** |

### Cold vs Warm (Samsung Galaxy)

| Run | NSFW (ms) | Face (ms) | OCR pre-filter (ms) | Total (ms) |
|-----|-----------|-----------|---------------------|------------|
| Cold (first image) | 729 | 634 | 184 | 1,550 |
| Warm (second image) | 420 | 502 | 148 | 1,072 |
| Warm (third image) | 421 | 526 | 157 | 1,106 |

~30% speedup after first invocation due to ONNX session warm-up.

### NSFW Detection (Solid Fill)

When NSFW is detected (score > 0.5), the entire image is replaced with a solid fill. Face and OCR phases are skipped:

| Device | NSFW only (ms) | Output size |
|--------|---------------|-------------|
| iPhone 17 | 2,169 | 6,181 bytes (from 41KB) |
| Samsung Galaxy | 665 | 6,181 bytes |
| Mid-range Android | 1,104 | 6,181 bytes |

### OCR Pre-Filter Savings

The inverted band pre-filter (edge density 0.05-0.12) skips OCR on photos, saving ~5 seconds:

| Image type | Edge density | OCR runs? | OCR time saved |
|------------|-------------|-----------|----------------|
| Photo (landscape) | 0.46 | No (above band) | ~4,500 ms |
| Photo (faces) | 0.43 | No (above band) | ~4,500 ms |
| Document (text) | 0.085 | Yes (in band) | 0 |
| Solid color | 0.00 | No (below band) | ~4,500 ms |

### EXIF Stripping

EXIF metadata (GPS, device info, timestamps) is stripped during the decode-reencode cycle:

```
exif_strip: input_had_exif=false, format=jpeg    # app already stripped
exif_strip: output_has_exif=false                 # confirmed clean
```

LLM confirms: "I can't determine the GPS coordinates from the image alone."

---

## Cognitive Firewall (Response Integrity)

### R1 Dictionary + R2 TinyBERT

| Device | R1 scan (us) | R2 invoked? | R2 inference (ms) | Total (ms) |
|--------|-------------|-------------|-------------------|------------|
| iPhone 17 | 109 | Yes (first call) | 50.0 | 50 |
| Samsung Galaxy | 75 | Yes | 13.3 | 13 |
| Mid-range Android | 120 | Yes | 250.0 | 250 |

R1 (dictionary scan) is sub-millisecond on all devices. R2 (TinyBERT inference) is invoked when R1 flags content or on sampling. The Samsung Galaxy achieves 13ms R2 inference — effectively instant.

### Flagging Example

Black Friday marketing email (634-1633 chars):

```
ri_scan: r1_flagged=true, r1_categories=3
ri_scan: r2_invoked=true, r2_role=Upgrade, r2_ms=170.3  (iPhone)
ri_scan: r2_invoked=true, r2_role=Upgrade, r2_ms=515.0  (mid-range Android)
[RI] FLAGGED: severity=Caution
```

---

## Restore

| Device | Per-call (ms) | Notes |
|--------|-------------|-------|
| iPhone 17 | 0.5-2.0 | String replacement, no model inference |
| Samsung Galaxy | 0.1-0.2 | |
| Mid-range Android | 0.1-0.3 | |

Restore is purely string replacement — no ONNX models involved. Sub-millisecond on all devices.

---

## Cross-Platform Summary

| Operation | iPhone 17 | Samsung Galaxy | Mid-range Android | Winner |
|-----------|-----------|---------------|-------------------|--------|
| NER (per new msg) | **80 ms** | 140 ms | 560 ms | iPhone |
| Image pipeline | 4,503 ms | **1,072 ms** | 2,575 ms | Samsung |
| NSFW (solid fill) | 2,169 ms | **665 ms** | 1,104 ms | Samsung |
| R2 inference | 50 ms | **13 ms** | 250 ms | Samsung |
| Cache (turns 3+) | **0 ms** | **0 ms** | **0 ms** | Tie |
| Restore | 0.5 ms | **0.1 ms** | 0.1 ms | Tie |

**Key takeaway:** Modern flagship Android devices (Samsung Galaxy with Snapdragon 8 Elite) outperform iPhone on image processing and RI inference. iPhone leads on NER due to optimized MLAS kernels. The sanitize cache eliminates NER cost on all platforms after turn 2, making the difference negligible in practice.

---

## How to Reproduce

Build with `--debug-logs` to enable `oo_dbg!` instrumentation:

```bash
# iOS
./build/build_ios.sh --debug-logs

# Android
./build/build_android.sh --debug-logs

# Gateway (desktop)
cd openobscure-core
cargo build --features debug-logs
```

Filter logs:

```bash
# iOS (Xcode console)
# Filter by: [OO]

# Android (Logcat)
adb logcat -s OpenObscure OO-Interceptor OO-Chat

# Gateway
# Logs go to stderr
```

Key log patterns to look for:

| Log prefix | What it measures |
|------------|-----------------|
| `hybrid_scan: ... total_us=` | Total text scan time (regex + keywords + NER) |
| `sanitize_text: ... scan_ms=` | Per-message sanitize time |
| `CACHE msg[N] hit` | Cache serving a prior message (0ms) |
| `image_pipeline: total_ms=` | Full image pipeline time |
| `nsfw_classify: scores=` | NSFW 5-class scores and decision |
| `face[N]: bbox=... conf=` | Per-face detection detail |
| `ocr_prefilter: density=` | Edge density and skip/run decision |
| `exif_strip: input_had_exif=` | EXIF presence in input |
| `exif_strip: output_has_exif=` | EXIF confirmed stripped in output |
| `ri_scan: ... r2_ms=` | Cognitive firewall timing |
| `restore_text: ... ms=` | Token restore timing |
