#!/usr/bin/env bash
# Download ONNX models for OpenObscure image pipeline.
# Idempotent — skips files that already exist.
#
# Models:
#   BlazeFace short-range  — face detection (128x128 input, 896 anchors)
#   PaddleOCR v3 detector  — text region detection
#   PaddleOCR English rec  — text recognition (Tier 2 only)
#   NudeNet 320n           — NSFW/nudity detection (YOLOv8n)
#
# Sources:
#   BlazeFace: https://github.com/axinc-ai/ailia-models (GCS mirror)
#   PaddleOCR: https://huggingface.co/monkt/paddleocr-onnx
#   NudeNet:   https://huggingface.co/vladmandic/nudenet

set -euo pipefail

PROXY_DIR="$(cd "$(dirname "$0")/../openobscure-proxy" && pwd)"
MODELS_DIR="$PROXY_DIR/models"

BLAZEFACE_DIR="$MODELS_DIR/blazeface"
PADDLEOCR_DIR="$MODELS_DIR/paddleocr"
NUDENET_DIR="$MODELS_DIR/nudenet"

# ── URLs ──
BLAZEFACE_URL="https://storage.googleapis.com/ailia-models/blazeface/blazeface.onnx"
PADDLEOCR_DET_URL="https://huggingface.co/monkt/paddleocr-onnx/resolve/main/detection/v3/det.onnx"
PADDLEOCR_REC_URL="https://huggingface.co/monkt/paddleocr-onnx/resolve/main/languages/english/rec.onnx"
PADDLEOCR_DICT_URL="https://huggingface.co/monkt/paddleocr-onnx/resolve/main/languages/english/dict.txt"
NUDENET_URL="https://huggingface.co/vladmandic/nudenet/resolve/main/nudenet.onnx"

download() {
    local url="$1" dest="$2" label="$3"
    if [ -f "$dest" ]; then
        echo "  [skip] $label (already exists)"
        return
    fi
    echo "  [download] $label"
    curl --fail --location --silent --show-error -o "$dest" "$url"
    local size
    size=$(wc -c < "$dest" | tr -d ' ')
    echo "           → $size bytes"
}

echo "=== OpenObscure ONNX Model Download ==="
echo ""

# ── BlazeFace ──
echo "BlazeFace (face detection):"
mkdir -p "$BLAZEFACE_DIR"
# The code looks for blazeface_short.onnx, blazeface.onnx, or model.onnx
download "$BLAZEFACE_URL" "$BLAZEFACE_DIR/blazeface.onnx" "blazeface.onnx (~408KB)"

# ── PaddleOCR ──
echo ""
echo "PaddleOCR (text detection + recognition):"
mkdir -p "$PADDLEOCR_DIR"
download "$PADDLEOCR_DET_URL" "$PADDLEOCR_DIR/det_model.onnx" "det_model.onnx (~2.4MB)"
download "$PADDLEOCR_REC_URL" "$PADDLEOCR_DIR/rec_model.onnx" "rec_model.onnx (~7.8MB)"
download "$PADDLEOCR_DICT_URL" "$PADDLEOCR_DIR/ppocr_keys.txt" "ppocr_keys.txt (dictionary)"

# ── NudeNet ──
echo ""
echo "NudeNet (NSFW/nudity detection):"
mkdir -p "$NUDENET_DIR"
download "$NUDENET_URL" "$NUDENET_DIR/nudenet.onnx" "nudenet.onnx (~12MB)"

echo ""
echo "=== Done ==="
echo ""
echo "Model layout:"
echo "  $BLAZEFACE_DIR/"
ls -lh "$BLAZEFACE_DIR/" 2>/dev/null | grep -v total || true
echo "  $PADDLEOCR_DIR/"
ls -lh "$PADDLEOCR_DIR/" 2>/dev/null | grep -v total || true
echo "  $NUDENET_DIR/"
ls -lh "$NUDENET_DIR/" 2>/dev/null | grep -v total || true
echo ""
echo "Configure in openobscure.toml:"
echo '  [image]'
echo '  face_model_dir = "models/blazeface"'
echo '  ocr_model_dir = "models/paddleocr"'
echo '  nsfw_model_dir = "models/nudenet"'
