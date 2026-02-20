#!/usr/bin/env bash
# Download ONNX models for OpenObscure image pipeline.
# Idempotent — skips files that already exist.
#
# Models:
#   BlazeFace short-range  — face detection, Lite tier (128x128 input)
#   SCRFD-2.5GF            — face detection, Full/Standard tier (640x640 input)
#   PaddleOCR v3 detector  — text region detection
#   PaddleOCR English rec  — text recognition (Tier 2 only)
#   NudeNet 320n           — NSFW/nudity detection (YOLOv8n)
#
# Sources:
#   BlazeFace: https://github.com/axinc-ai/ailia-models (GCS mirror)
#   SCRFD:     https://github.com/cysin/scrfd_onnx
#   PaddleOCR: https://huggingface.co/deepghs/paddleocr (PP-OCRv4 English rec)
#              https://huggingface.co/monkt/paddleocr-onnx (v3 detector)
#   NudeNet:   https://huggingface.co/vladmandic/nudenet

set -euo pipefail

PROXY_DIR="$(cd "$(dirname "$0")/../openobscure-proxy" && pwd)"
MODELS_DIR="$PROXY_DIR/models"

BLAZEFACE_DIR="$MODELS_DIR/blazeface"
SCRFD_DIR="$MODELS_DIR/scrfd"
PADDLEOCR_DIR="$MODELS_DIR/paddleocr"
NUDENET_DIR="$MODELS_DIR/nudenet"

# ── URLs ──
BLAZEFACE_URL="https://storage.googleapis.com/ailia-models/blazeface/blazeface.onnx"
SCRFD_URL="https://github.com/cysin/scrfd_onnx/raw/refs/heads/main/scrfd_2.5g_bnkps_shape640x640.onnx"
PADDLEOCR_DET_URL="https://huggingface.co/monkt/paddleocr-onnx/resolve/main/detection/v3/det.onnx"
PADDLEOCR_REC_URL="https://huggingface.co/deepghs/paddleocr/resolve/main/rec/en_PP-OCRv4_rec/model.onnx"
PADDLEOCR_DICT_URL="https://huggingface.co/deepghs/paddleocr/resolve/main/rec/en_PP-OCRv4_rec/dict.txt"
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

# ── SCRFD ──
echo ""
echo "SCRFD-2.5GF (face detection — Full/Standard tier):"
mkdir -p "$SCRFD_DIR"
download "$SCRFD_URL" "$SCRFD_DIR/scrfd_2.5g.onnx" "scrfd_2.5g.onnx (~3.1MB)"

# ── PaddleOCR ──
echo ""
echo "PaddleOCR (text detection + recognition):"
mkdir -p "$PADDLEOCR_DIR"
download "$PADDLEOCR_DET_URL" "$PADDLEOCR_DIR/det_model.onnx" "det_model.onnx (~2.4MB)"
download "$PADDLEOCR_REC_URL" "$PADDLEOCR_DIR/rec_model.onnx" "rec_model.onnx (~7.7MB, PP-OCRv4 English)"
download "$PADDLEOCR_DICT_URL" "$PADDLEOCR_DIR/ppocr_keys.txt" "ppocr_keys.txt (95-char English dictionary)"

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
echo "  $SCRFD_DIR/"
ls -lh "$SCRFD_DIR/" 2>/dev/null | grep -v total || true
echo "  $PADDLEOCR_DIR/"
ls -lh "$PADDLEOCR_DIR/" 2>/dev/null | grep -v total || true
echo "  $NUDENET_DIR/"
ls -lh "$NUDENET_DIR/" 2>/dev/null | grep -v total || true
echo ""
echo "Configure in openobscure.toml:"
echo '  [image]'
echo '  face_model = "scrfd"        # or "blazeface" for Lite tier'
echo '  face_model_dir = "models/blazeface"'
echo '  face_model_dir_scrfd = "models/scrfd"'
echo '  ocr_model_dir = "models/paddleocr"'
echo '  nsfw_model_dir = "models/nudenet"'
