#!/usr/bin/env bash
# download_kws_models.sh — Download KWS Zipformer models for audio PII detection.
#
# Downloads the sherpa-onnx-kws-zipformer-gigaspeech-3.3M model (INT8, ~5MB total)
# and generates the tokenized PII keywords file.
#
# Usage:
#   ./build/download_kws_models.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
KWS_DIR="$PROJECT_DIR/openobscure-core/models/kws"

MODEL_NAME="sherpa-onnx-kws-zipformer-gigaspeech-3.3M-2024-01-01"
MODEL_URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/kws-models/${MODEL_NAME}.tar.bz2"

mkdir -p "$KWS_DIR"

# Download and extract model if not already present
if [[ ! -f "$KWS_DIR/tokens.txt" ]]; then
  echo "Downloading KWS model: $MODEL_NAME..."
  TMP_DIR=$(mktemp -d)
  curl -sL "$MODEL_URL" -o "$TMP_DIR/model.tar.bz2"
  tar -xjf "$TMP_DIR/model.tar.bz2" -C "$TMP_DIR"

  # Copy INT8 ONNX files (smaller, faster)
  cp "$TMP_DIR/$MODEL_NAME/"*encoder*int8*.onnx "$KWS_DIR/" 2>/dev/null || \
    cp "$TMP_DIR/$MODEL_NAME/"*encoder*.onnx "$KWS_DIR/"
  cp "$TMP_DIR/$MODEL_NAME/"*decoder*int8*.onnx "$KWS_DIR/" 2>/dev/null || \
    cp "$TMP_DIR/$MODEL_NAME/"*decoder*.onnx "$KWS_DIR/"
  cp "$TMP_DIR/$MODEL_NAME/"*joiner*int8*.onnx "$KWS_DIR/" 2>/dev/null || \
    cp "$TMP_DIR/$MODEL_NAME/"*joiner*.onnx "$KWS_DIR/"

  # Copy tokens and BPE model
  cp "$TMP_DIR/$MODEL_NAME/tokens.txt" "$KWS_DIR/"
  cp "$TMP_DIR/$MODEL_NAME/bpe.model" "$KWS_DIR/" 2>/dev/null || true

  # Copy test keywords as reference
  cp "$TMP_DIR/$MODEL_NAME/test_wavs/test_keywords.txt" "$KWS_DIR/test_keywords_reference.txt" 2>/dev/null || true

  rm -rf "$TMP_DIR"
  echo "Model files extracted to $KWS_DIR/"
else
  echo "KWS model files already present in $KWS_DIR/"
fi

# Generate tokenized PII keywords
RAW_KEYWORDS="$KWS_DIR/pii_keywords_raw.txt"
TOKENIZED_KEYWORDS="$KWS_DIR/keywords.txt"

if [[ -f "$RAW_KEYWORDS" ]]; then
  if command -v sherpa-onnx-cli &>/dev/null && [[ -f "$KWS_DIR/bpe.model" ]]; then
    echo "Tokenizing PII keywords with sherpa-onnx-cli..."
    sherpa-onnx-cli text2token \
      --tokens "$KWS_DIR/tokens.txt" \
      --tokens-type bpe \
      --bpe-model "$KWS_DIR/bpe.model" \
      "$RAW_KEYWORDS" \
      "$TOKENIZED_KEYWORDS"
    echo "Tokenized keywords written to $TOKENIZED_KEYWORDS"
  else
    echo "sherpa-onnx-cli not found or bpe.model missing."
    echo "Using reference keywords file if available, or please tokenize manually."
    if [[ -f "$KWS_DIR/test_keywords_reference.txt" && ! -f "$TOKENIZED_KEYWORDS" ]]; then
      cp "$KWS_DIR/test_keywords_reference.txt" "$TOKENIZED_KEYWORDS"
      echo "Copied reference keywords to $TOKENIZED_KEYWORDS (update with PII keywords later)"
    fi
  fi
fi

echo ""
echo "=== KWS Model Summary ==="
echo "Directory: $KWS_DIR/"
ls -lh "$KWS_DIR/"*.onnx 2>/dev/null || echo "  (no ONNX files found)"
ls -lh "$KWS_DIR/tokens.txt" 2>/dev/null || echo "  (no tokens.txt found)"
ls -lh "$KWS_DIR/keywords.txt" 2>/dev/null || echo "  (no keywords.txt found)"
echo ""

TOTAL_SIZE=$(du -sh "$KWS_DIR" | awk '{print $1}')
echo "Total model size: $TOTAL_SIZE"
