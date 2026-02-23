# Face Detector Benchmark Results

Comparison of face detection models for OpenObscure Lite tier (80MB RAM budget).

## How to Run

```bash
# Install dependencies (not project deps — benchmark tooling only)
pip install onnxruntime numpy Pillow

# Run BlazeFace only (current Lite tier model)
python scripts/benchmark_face_detectors.py

# Run both models
python scripts/benchmark_face_detectors.py \
    --ultralight-model path/to/ultra_light_320.onnx \
    --output scripts/benchmark_results/latest.md
```

### Obtaining the Ultra-Light Model

Download from [Ultra-Light-Fast-Generic-Face-Detector-1MB](https://github.com/Linzaer/Ultra-Light-Fast-Generic-Face-Detector-1MB):

- `version-RFB/RFB-320.onnx` (320x240 input, recommended for Lite tier comparison)
- `version-slim/slim-320.onnx` (320x240 input, even smaller variant)

## Results Template

| Model | Input Size | Disk | RAM (est) | Avg Latency (ms) | Faces Found | Notes |
|-------|-----------|------|-----------|-------------------|-------------|-------|
| BlazeFace Short | 128x128 | 400KB | ~8MB | TBD | TBD | Current Lite tier |
| Ultra-Light RFB-320 | 320x240 | ~1MB | ~2MB (claimed) | TBD | TBD | Candidate |
| Ultra-Light Slim-320 | 320x240 | ~1MB | ~2MB (claimed) | TBD | TBD | Smaller variant |

## Decision Criteria

Replace BlazeFace with Ultra-Light on Lite tier if ALL conditions are met:

1. **Recall**: Detects >= the same number of faces as BlazeFace on the test image set (no regression)
2. **RAM**: Estimated RAM <= 4MB (saving at least 4MB vs BlazeFace's ~8MB)
3. **Latency**: Average inference <= 2x BlazeFace latency (acceptable tradeoff for RAM savings)

If Ultra-Light wins on these criteria, a follow-up task adds `UltraLightDetector` to `face_detector.rs` as an additional option alongside BlazeFace.
