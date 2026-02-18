# Phase 6: Ensemble Recall + Cleanup

**Status: COMPLETE** (2026-02-18)

## Context

Phase 5 delivered key rotation, SSE streaming, and benchmarks (399 tests). Phase 6 was the final pre-release cleanup: ensemble confidence voting for the hybrid scanner, detection verification framework, and accuracy hardening.

---

## Implementation Summary

### Item 1: Ensemble Confidence Voting — COMPLETE

HybridScanner cluster-based overlap resolution with agreement bonus. When regex and NER/CRF agree on a PII match, confidence is boosted. Overlapping detections are merged into clusters with the highest-confidence match winning.

**Files:** `hybrid_scanner.rs` (cluster merge + agreement bonus logic)

### Item 2: Detection Verification Framework — COMPLETE (46 tests)

Two-layer architecture for blazing-fast detection verification:
- **Layer 1 — Detection Metadata:** `BboxMeta`, `NsfwMeta`, `ScreenshotMeta`, `PipelineMeta` structs collected during inference (+10-20ms overhead)
- **Layer 2 — Pure-Logic Validators:** `validate_bbox()`, `validate_face_detections()`, `validate_text_regions()`, `validate_nsfw()`, `validate_screenshot()`, `bbox_iou()`, `precision_recall()` — microsecond execution, no models needed

**New files:**
- `src/detection_meta.rs` — metadata structs (~112 lines)
- `src/detection_validators.rs` — validators + 40 inline tests (~1025 lines)
- `tests/pipeline_validation_test.rs` — 6 model-gated integration tests

**Modified files:**
- `src/image_pipeline.rs` — changed `process_image()` return type to include `PipelineMeta`
- `src/face_detector.rs` — added `FaceDetection::to_bbox_meta()`
- `src/ocr_engine.rs` — added `TextRegion::to_bbox_meta()`
- `src/lib.rs`, `src/main.rs` — module wiring
- `src/body.rs`, `examples/demo_image_pipeline.rs` — updated callers
- `tests/accuracy_test.rs` — 4 PII structural sanity tests

### Item 3: Image Pipeline Fixes — COMPLETE

- BlazeFace /128 normalization regression fix
- NSFW detection integration (NudeNet 320n ONNX)
- OCR blur thickening for better coverage
- README updates with before/after image examples

---

## Test Counts

- **Proxy:** 306 tests (297 bin + 9 accuracy) → with verification: 326 bin + 216 lib + 13 accuracy + 6 pipeline = 561 test runs
- **Crypto:** 16 tests
- **Plugin:** 96 tests
- **Total:** 418 unique tests (some run in both bin and lib targets)
