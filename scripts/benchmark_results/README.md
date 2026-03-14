# Face Detector Benchmark Results

Comparison of face detection models for OpenObscure Lite tier (80MB RAM budget).

## Decision (Completed)

**Ultra-Light RFB-320 is deployed as the Lite tier face detector** (`face_model: "ultralight"` in `device_profile.rs`).

All three decision criteria were met:

| Criterion | Threshold | Ultra-Light Result | Met? |
|-----------|-----------|-------------------|------|
| Recall | ≥ same faces as BlazeFace | Equal or better on test set | Yes |
| RAM | ≤ 4 MB | ~2 MB (claimed, consistent with 1MB model disk size) | Yes |
| Latency | ≤ 2x BlazeFace | Comparable; smaller input (320x240) offsets anchor count | Yes |

BlazeFace (~8MB RAM, 128x128 input) failed the RAM criterion. Ultra-Light RFB-320 (~2MB RAM,
320x240 input) meets all three. `UltraLightDetector` was added to `face_detector.rs` and wired
into the Lite tier budget.

Full/Standard tiers continue to use SCRFD-2.5GF (640x640, ~3.1MB disk, higher multi-scale recall).

## Benchmark Results

| Model | Input Size | Disk | RAM (est) | Tier | Status |
|-------|-----------|------|-----------|------|--------|
| BlazeFace Short | 128x128 | ~400 KB | ~8 MB | — | Not deployed (RAM too high for Lite budget) |
| **Ultra-Light RFB-320** | 320x240 | ~1 MB | ~2 MB | **Lite** | **Deployed** |
| SCRFD-2.5GF | 640x640 | ~3.1 MB | ~6 MB | Full/Standard | Deployed |

> Exact latency numbers from the benchmark run were not saved. Run `benchmark_face_detectors.py`
> to regenerate per-platform measurements.

## How to Re-Run

```bash
# Install dependencies (not project deps — benchmark tooling only)
pip install onnxruntime numpy Pillow

# Run Ultra-Light only (current Lite tier model)
python scripts/benchmark_face_detectors.py \
    --ultralight-model openobscure-core/models/ultralight/version-RFB-320.onnx

# Run all three models
python scripts/benchmark_face_detectors.py \
    --blazeface-model openobscure-core/models/blazeface/blazeface.onnx \
    --ultralight-model openobscure-core/models/ultralight/version-RFB-320.onnx \
    --scrfd-model openobscure-core/models/scrfd/scrfd_2.5g.onnx \
    --output scripts/benchmark_results/latest.md
```

### Obtaining the Models

- **Ultra-Light RFB-320**: [Ultra-Light-Fast-Generic-Face-Detector-1MB](https://github.com/Linzaer/Ultra-Light-Fast-Generic-Face-Detector-1MB) — `version-RFB/RFB-320.onnx`
- **BlazeFace**: [MediaPipe BlazeFace](https://github.com/google/mediapipe) — `blazeface.onnx`
- **SCRFD-2.5GF**: [insightface model zoo](https://github.com/deepinsight/insightface) — `scrfd_2.5g.onnx`
