# Image Pipeline Architecture

> **Role in OpenObscure:** The image pipeline is the **visual PII detection core** of L0. It processes base64-encoded images through a phased pipeline — NSFW classification, face detection, OCR text detection, and EXIF stripping — solid-filling sensitive regions before the image leaves the device. For the full system context, see [System Overview](system-overview.md).
>
> **Implementation:** `openobscure-core/src/image_pipeline.rs` and related modules. For module-level details, see [L0 Core Architecture](l0-core.md).

---

## Pipeline Phases

Two-pass processing in `body.rs`: images first (entire base64 string replacement), then text PII (substring FPE).

**Pipeline phases:** NSFW detection (ViT-base 5-class classifier) → face solid-fill (SCRFD or BlazeFace) → OCR text solid-fill (PaddleOCR) → EXIF strip → re-encode. If NSFW detected (P(hentai) + P(porn) + P(sexy) ≥ 0.50), entire image is solid-filled and face/OCR phases are skipped. Models load on-demand and evict after 300s idle.

---

## ImageModelManager (`image_pipeline.rs`)

```rust
pub struct ImageModelManager {
    nsfw_classifier: Mutex<Option<Arc<Mutex<NsfwClassifier>>>>,
    face_detector: Mutex<Option<Arc<Mutex<FaceDetector>>>>,
    scrfd_detector: Mutex<Option<Arc<Mutex<ScrfdDetector>>>>,
    ultralight_detector: Mutex<Option<Arc<Mutex<UltraLightDetector>>>>,
    ocr_detector: Mutex<Option<Arc<Mutex<OcrDetector>>>>,
    ocr_recognizer: Mutex<Option<Arc<Mutex<OcrRecognizer>>>>,
    last_use: Mutex<Instant>,
    config: ImageConfig,
}
```

**Request-scoped model guards:** Models use a double-`Mutex<Option<Arc<Mutex<T>>>>` pattern. The outer Mutex protects load/evict, the inner Mutex gives `&mut` access for ONNX inference (`Session::run` requires `&mut self`). Requests clone the inner `Arc` before releasing the outer lock. Eviction sets the slot to `None` without invalidating in-flight references — existing `Arc` clones keep models alive until the request completes.

**Memory rule:** Models loaded sequentially. Face model loaded/used/dropped before OCR model loaded. Never both in RAM simultaneously. The NSFW classifier follows the same lazy-load/evict pattern. Background eviction task (60s interval) evicts models idle beyond `model_idle_timeout_secs` (default 300).

---

## Face Detection (`face_detector.rs`)

Tier-gated face detection: SCRFD-2.5GF for Full/Standard tiers (multi-scale), Ultra-Light RFB-320 for Lite tier (better resolution than BlazeFace), BlazeFace as fallback. Auto-fallback chain: SCRFD → Ultra-Light → BlazeFace on error.

### SCRFD-2.5GF (Full/Standard tier)

| Property | Value |
|----------|-------|
| Model | SCRFD-2.5GF (~3MB) |
| Input | `[1, 3, 640, 640]` float32, RGB |
| Output | 9 tensors: score/bbox/kps at strides 8/16/32, 2 anchors per cell |
| Post-processing | Score threshold + NMS (IoU 0.4) |
| Strengths | Multi-scale FPN: detects 20px–400px faces in same image |

### Ultra-Light RFB-320 (Lite tier)

| Property | Value |
|----------|-------|
| Model | Ultra-Light-Fast-Generic-Face-Detector-1MB (~1.2MB) |
| Input | `[1, 3, 240, 320]` float32, RGB normalized (pixel-127)/128 |
| Output | Scores `[1, 4420, 2]` (background/face) + boxes `[1, 4420, 4]` (normalized coords) |
| Post-processing | Confidence threshold + NMS (IoU 0.3) |
| Strengths | 2.5x resolution vs BlazeFace (320x240 vs 128x128), ~1.2MB model size |

### BlazeFace (fallback)

| Property | Value |
|----------|-------|
| Model | BlazeFace short-range (~230KB INT8) |
| Input | `[1, 3, 128, 128]` float32, RGB normalized to [-1, 1] |
| Output | Bounding boxes + confidence scores |
| Post-processing | Sigmoid activation, anchor-relative decoding, NMS (IoU 0.3) |
| Anchors | 1664 generated (strides 8/16/16/16, 2/6/6/6 per stride) |

**BlazeFace Tiling Heuristic:** When BlazeFace is used (fallback path) on images with longest side > 512px and the first pass finds 0 faces, an automatic tiled second pass runs: 4 overlapping quadrants (62.5% of each dimension, ~25% overlap), BlazeFace on each tile, coordinate remapping, and NMS merge (IoU 0.3). Cost: ~4ms extra, only when needed.

---

## OCR Engine (`ocr_engine.rs`)

PaddleOCR PP-OCRv4 text detection and recognition via ONNX Runtime.

| Component | Model | Input | Output |
|-----------|-------|-------|--------|
| Detector | det_model.onnx (~2.4MB) | `[1, 3, H, W]` BGR, ImageNet norm | Probability map → binary mask → connected components → text regions |
| Recognizer | rec_model.onnx (PP-OCRv4, ~10MB) | `[B, 3, 48, W]` cropped regions | Logits → CTC greedy decode with dictionary |

**Two tiers:**
- **DetectAndFill (Tier 1, default):** Detect text regions → solid-fill all. No recognition model needed.
- **FullRecognition (Tier 2+):** Detect → recognize → scan text for PII → selectively solid-fill PII regions.

---

## Screenshot Detection (`screen_guard.rs`)

| Heuristic | Method | Weight |
|-----------|--------|--------|
| EXIF software | Check Software/UserComment tags for 18 screenshot tool names | Definitive (single match = screenshot) |
| No camera hardware | EXIF has Software but no Make/Model | Supporting |
| Screen resolution | Match against 21 common resolutions (desktop + mobile, 1x + 2x) | Supporting |
| Status bar | Color variance in top 5% strip < 50 (uniform = status bar) | Supporting |

Explicit EXIF software match → screenshot. Otherwise need >= 2 supporting heuristics.
