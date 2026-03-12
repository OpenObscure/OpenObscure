#!/usr/bin/env python3
"""
Face Detector Benchmark: BlazeFace vs Ultra-Light-Fast-Generic-Face-Detector

Compares inference latency, detection count, and resource usage between
BlazeFace (current Lite tier default) and Ultra-Light face detector
(candidate for Lite tier evaluation).

Neither model is modified. This script is evaluation-only.

Usage:
    python scripts/benchmark_face_detectors.py \
        --blazeface-model openobscure-core/models/blazeface/blazeface.onnx \
        --ultralight-model path/to/ultra_light_320.onnx \
        --images-dir docs/examples/images/

Dependencies (not project dependencies):
    pip install onnxruntime numpy Pillow
"""

import argparse
import os
import sys
import time
from pathlib import Path

import numpy as np
from PIL import Image

try:
    import onnxruntime as ort
except ImportError:
    print("Error: onnxruntime not installed. Run: pip install onnxruntime")
    sys.exit(1)


# ---------------------------------------------------------------------------
# BlazeFace pre/post processing (mirrors face_detector.rs)
# ---------------------------------------------------------------------------

BLAZEFACE_INPUT_SIZE = 128
BLAZEFACE_CONF_THRESHOLD = 0.75
BLAZEFACE_NMS_THRESHOLD = 0.3


def preprocess_blazeface(img: Image.Image) -> np.ndarray:
    """Resize to 128x128 and normalize to [-1, 1]."""
    resized = img.resize((BLAZEFACE_INPUT_SIZE, BLAZEFACE_INPUT_SIZE), Image.BILINEAR)
    arr = np.array(resized, dtype=np.float32)
    if arr.ndim == 2:
        arr = np.stack([arr, arr, arr], axis=-1)
    elif arr.shape[2] == 4:
        arr = arr[:, :, :3]
    arr = (arr - 127.5) / 127.5  # normalize to [-1, 1]
    arr = np.transpose(arr, (2, 0, 1))  # HWC -> CHW
    return np.expand_dims(arr, axis=0)  # add batch dim


def postprocess_blazeface(outputs: list, img_w: int, img_h: int) -> list:
    """Extract face bounding boxes from BlazeFace output tensors."""
    # BlazeFace outputs: [regressors (1,896,16), classificators (1,896,1)]
    if len(outputs) < 2:
        return []

    scores = outputs[1].flatten()
    # Apply sigmoid to raw scores
    scores = 1.0 / (1.0 + np.exp(-scores))

    boxes = outputs[0][0]  # (896, 16)
    detections = []

    for i, score in enumerate(scores):
        if score < BLAZEFACE_CONF_THRESHOLD:
            continue
        # BlazeFace regression values are relative to 128x128 input
        cx = boxes[i, 0] / BLAZEFACE_INPUT_SIZE
        cy = boxes[i, 1] / BLAZEFACE_INPUT_SIZE
        w = boxes[i, 2] / BLAZEFACE_INPUT_SIZE
        h = boxes[i, 3] / BLAZEFACE_INPUT_SIZE

        x1 = max(0.0, (cx - w / 2)) * img_w
        y1 = max(0.0, (cy - h / 2)) * img_h
        x2 = min(1.0, (cx + w / 2)) * img_w
        y2 = min(1.0, (cy + h / 2)) * img_h

        detections.append({
            "bbox": [x1, y1, x2, y2],
            "confidence": float(score),
        })

    return nms(detections, BLAZEFACE_NMS_THRESHOLD)


# ---------------------------------------------------------------------------
# Ultra-Light pre/post processing
# ---------------------------------------------------------------------------

ULTRALIGHT_CONF_THRESHOLD = 0.7
ULTRALIGHT_NMS_THRESHOLD = 0.3


def detect_ultralight_input_size(session: ort.InferenceSession) -> tuple:
    """Detect expected input dimensions from model metadata."""
    input_shape = session.get_inputs()[0].shape
    # Typically [1, 3, H, W] for Ultra-Light models
    if len(input_shape) == 4:
        return int(input_shape[3]), int(input_shape[2])  # (width, height)
    return 320, 240  # default for version-320


def preprocess_ultralight(img: Image.Image, target_w: int, target_h: int) -> np.ndarray:
    """Resize and normalize for Ultra-Light model (mean/std normalization)."""
    resized = img.resize((target_w, target_h), Image.BILINEAR)
    arr = np.array(resized, dtype=np.float32)
    if arr.ndim == 2:
        arr = np.stack([arr, arr, arr], axis=-1)
    elif arr.shape[2] == 4:
        arr = arr[:, :, :3]
    # Ultra-Light uses ImageNet-style normalization
    mean = np.array([127.0, 127.0, 127.0], dtype=np.float32)
    std = np.array([128.0, 128.0, 128.0], dtype=np.float32)
    arr = (arr - mean) / std
    arr = np.transpose(arr, (2, 0, 1))  # HWC -> CHW
    return np.expand_dims(arr, axis=0)  # add batch dim


def postprocess_ultralight(outputs: list, img_w: int, img_h: int) -> list:
    """Extract face bounding boxes from Ultra-Light output tensors."""
    # Ultra-Light outputs: [confidences (1,N,2), boxes (1,N,4)]
    if len(outputs) < 2:
        return []

    confidences = outputs[0][0]  # (N, 2) — [bg_score, face_score]
    boxes = outputs[1][0]  # (N, 4) — [x1, y1, x2, y2] normalized

    detections = []
    for i in range(confidences.shape[0]):
        face_score = confidences[i, 1]
        if face_score < ULTRALIGHT_CONF_THRESHOLD:
            continue

        x1 = max(0.0, boxes[i, 0]) * img_w
        y1 = max(0.0, boxes[i, 1]) * img_h
        x2 = min(1.0, boxes[i, 2]) * img_w
        y2 = min(1.0, boxes[i, 3]) * img_h

        detections.append({
            "bbox": [x1, y1, x2, y2],
            "confidence": float(face_score),
        })

    return nms(detections, ULTRALIGHT_NMS_THRESHOLD)


# ---------------------------------------------------------------------------
# Shared utilities
# ---------------------------------------------------------------------------

def nms(detections: list, threshold: float) -> list:
    """Non-maximum suppression."""
    if not detections:
        return []

    detections.sort(key=lambda d: d["confidence"], reverse=True)
    keep = []

    for det in detections:
        suppress = False
        for kept in keep:
            if iou(det["bbox"], kept["bbox"]) > threshold:
                suppress = True
                break
        if not suppress:
            keep.append(det)

    return keep


def iou(box_a: list, box_b: list) -> float:
    """Intersection over Union for two [x1, y1, x2, y2] boxes."""
    x1 = max(box_a[0], box_b[0])
    y1 = max(box_a[1], box_b[1])
    x2 = min(box_a[2], box_b[2])
    y2 = min(box_a[3], box_b[3])

    inter = max(0, x2 - x1) * max(0, y2 - y1)
    area_a = (box_a[2] - box_a[0]) * (box_a[3] - box_a[1])
    area_b = (box_b[2] - box_b[0]) * (box_b[3] - box_b[1])
    union = area_a + area_b - inter

    return inter / union if union > 0 else 0.0


def load_images(images_dir: str) -> list:
    """Load all images from directory."""
    exts = {".jpg", ".jpeg", ".png", ".bmp", ".webp"}
    images = []
    for f in sorted(Path(images_dir).rglob("*")):
        if f.suffix.lower() in exts and f.is_file():
            try:
                img = Image.open(f).convert("RGB")
                images.append((f.name, img))
            except Exception as e:
                print(f"  Warning: could not load {f}: {e}")
    return images


def format_bbox(bbox: list) -> str:
    """Format bbox as compact string."""
    return f"[{bbox[0]:.0f},{bbox[1]:.0f},{bbox[2]:.0f},{bbox[3]:.0f}]"


# ---------------------------------------------------------------------------
# Benchmark runner
# ---------------------------------------------------------------------------

def benchmark_model(
    session: ort.InferenceSession,
    preprocess_fn,
    postprocess_fn,
    images: list,
    runs: int,
    model_name: str,
) -> list:
    """Run benchmark for a single model across all images."""
    input_name = session.get_inputs()[0].name
    results = []

    for img_name, img in images:
        input_tensor = preprocess_fn(img)

        # Warm-up run
        session.run(None, {input_name: input_tensor})

        # Timed runs
        latencies = []
        for _ in range(runs):
            t0 = time.perf_counter()
            outputs = session.run(None, {input_name: input_tensor})
            t1 = time.perf_counter()
            latencies.append((t1 - t0) * 1000)  # ms

        detections = postprocess_fn(outputs, img.width, img.height)
        avg_ms = sum(latencies) / len(latencies)
        min_ms = min(latencies)
        max_ms = max(latencies)

        results.append({
            "image": img_name,
            "faces": len(detections),
            "detections": detections,
            "avg_ms": avg_ms,
            "min_ms": min_ms,
            "max_ms": max_ms,
        })

        bbox_str = ", ".join(
            f"{format_bbox(d['bbox'])} ({d['confidence']:.2f})"
            for d in detections
        )
        print(
            f"  {img_name:<40} faces={len(detections):>2}  "
            f"avg={avg_ms:>6.1f}ms  min={min_ms:>6.1f}ms  max={max_ms:>6.1f}ms"
            f"  {bbox_str}"
        )

    return results


def print_summary(
    blazeface_results: list | None,
    ultralight_results: list | None,
    blazeface_size: int | None,
    ultralight_size: int | None,
):
    """Print markdown comparison table."""
    print("\n" + "=" * 80)
    print("COMPARISON SUMMARY")
    print("=" * 80)

    rows = []

    if blazeface_results:
        total_faces = sum(r["faces"] for r in blazeface_results)
        avg_latency = sum(r["avg_ms"] for r in blazeface_results) / len(blazeface_results)
        size_str = f"{blazeface_size / 1024:.0f}KB" if blazeface_size else "N/A"
        rows.append(("BlazeFace Short", "128x128", size_str, "~8MB", f"{avg_latency:.1f}", str(total_faces), "Current Lite tier"))

    if ultralight_results:
        total_faces = sum(r["faces"] for r in ultralight_results)
        avg_latency = sum(r["avg_ms"] for r in ultralight_results) / len(ultralight_results)
        size_str = f"{ultralight_size / 1024:.0f}KB" if ultralight_size else "N/A"
        rows.append(("Ultra-Light", "320x240*", size_str, "TBD", f"{avg_latency:.1f}", str(total_faces), "Candidate"))

    print()
    print("| Model | Input Size | Disk | RAM (est) | Avg Latency (ms) | Total Faces | Notes |")
    print("|-------|-----------|------|-----------|-------------------|-------------|-------|")
    for r in rows:
        print(f"| {r[0]} | {r[1]} | {r[2]} | {r[3]} | {r[4]} | {r[5]} | {r[6]} |")

    print()
    print("* Ultra-Light input size auto-detected from ONNX model metadata")


def main():
    parser = argparse.ArgumentParser(
        description="Benchmark BlazeFace vs Ultra-Light face detectors for OpenObscure Lite tier evaluation"
    )
    parser.add_argument(
        "--blazeface-model",
        default="openobscure-core/models/blazeface/blazeface.onnx",
        help="Path to BlazeFace ONNX model (default: openobscure-core/models/blazeface/blazeface.onnx)",
    )
    parser.add_argument(
        "--ultralight-model",
        default=None,
        help="Path to Ultra-Light ONNX model (optional — download from github.com/Linzaer/Ultra-Light-Fast-Generic-Face-Detector-1MB)",
    )
    parser.add_argument(
        "--images-dir",
        default="docs/examples/images/",
        help="Directory containing test images (default: docs/examples/images/)",
    )
    parser.add_argument(
        "--output",
        default=None,
        help="Optional path to write results markdown file",
    )
    parser.add_argument(
        "--runs",
        type=int,
        default=10,
        help="Number of inference runs per image for latency averaging (default: 10)",
    )
    args = parser.parse_args()

    # Load images
    print(f"Loading images from {args.images_dir}...")
    images = load_images(args.images_dir)
    if not images:
        print(f"Error: no images found in {args.images_dir}")
        sys.exit(1)
    print(f"Found {len(images)} images\n")

    blazeface_results = None
    blazeface_size = None
    ultralight_results = None
    ultralight_size = None

    # Benchmark BlazeFace
    if os.path.exists(args.blazeface_model):
        print(f"--- BlazeFace ({args.blazeface_model}) ---")
        blazeface_size = os.path.getsize(args.blazeface_model)
        print(f"Model size: {blazeface_size / 1024:.0f} KB")

        sess_opts = ort.SessionOptions()
        sess_opts.intra_op_num_threads = 1
        session = ort.InferenceSession(args.blazeface_model, sess_opts)

        blazeface_results = benchmark_model(
            session=session,
            preprocess_fn=preprocess_blazeface,
            postprocess_fn=postprocess_blazeface,
            images=images,
            runs=args.runs,
            model_name="BlazeFace",
        )
        print()
    else:
        print(f"Warning: BlazeFace model not found at {args.blazeface_model}, skipping\n")

    # Benchmark Ultra-Light
    if args.ultralight_model and os.path.exists(args.ultralight_model):
        print(f"--- Ultra-Light ({args.ultralight_model}) ---")
        ultralight_size = os.path.getsize(args.ultralight_model)
        print(f"Model size: {ultralight_size / 1024:.0f} KB")

        sess_opts = ort.SessionOptions()
        sess_opts.intra_op_num_threads = 1
        session = ort.InferenceSession(args.ultralight_model, sess_opts)

        ul_w, ul_h = detect_ultralight_input_size(session)
        print(f"Detected input size: {ul_w}x{ul_h}")

        def ul_preprocess(img):
            return preprocess_ultralight(img, ul_w, ul_h)

        ultralight_results = benchmark_model(
            session=session,
            preprocess_fn=ul_preprocess,
            postprocess_fn=postprocess_ultralight,
            images=images,
            runs=args.runs,
            model_name="Ultra-Light",
        )
        print()
    elif args.ultralight_model:
        print(f"Warning: Ultra-Light model not found at {args.ultralight_model}, skipping\n")
    else:
        print("Note: --ultralight-model not provided, running BlazeFace only\n")

    # Print summary
    print_summary(blazeface_results, ultralight_results, blazeface_size, ultralight_size)

    # Write output file
    if args.output:
        with open(args.output, "w") as f:
            f.write("# Face Detector Benchmark Results\n\n")
            f.write(f"Images: {len(images)} from `{args.images_dir}`\n")
            f.write(f"Runs per image: {args.runs}\n\n")

            if blazeface_results:
                f.write("## BlazeFace\n\n")
                for r in blazeface_results:
                    f.write(f"- {r['image']}: {r['faces']} faces, {r['avg_ms']:.1f}ms avg\n")
                f.write("\n")

            if ultralight_results:
                f.write("## Ultra-Light\n\n")
                for r in ultralight_results:
                    f.write(f"- {r['image']}: {r['faces']} faces, {r['avg_ms']:.1f}ms avg\n")
                f.write("\n")

        print(f"\nResults written to {args.output}")


if __name__ == "__main__":
    main()
