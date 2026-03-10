#!/bin/bash
# Bundle all ONNX models for embedded (iOS/Android) distribution.
#
# Maps dev repo model directory names to the standard names expected by
# resolve_model_dirs() in lib_mobile.rs.
#
# Usage:
#   ./build/bundle_models.sh <output_dir>
#   ./build/bundle_models.sh /path/to/app/models

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
MODELS_SRC="$SCRIPT_DIR/../openobscure-proxy/models"

if [ $# -lt 1 ]; then
    echo "Usage: $0 <output_dir>"
    echo "Example: $0 ~/Test/enchanted-openobscure/models"
    exit 1
fi

OUTPUT_DIR="$1"
mkdir -p "$OUTPUT_DIR"

# source_name:dest_name pairs
MAPPINGS="ner:ner ner-lite:ner_lite scrfd:scrfd blazeface:blazeface paddleocr:ocr nsfw_classifier:nsfw_classifier ri:ri"

COPIED=0
SKIPPED=0

for mapping in $MAPPINGS; do
    src_name="${mapping%%:*}"
    dest_name="${mapping##*:}"
    src_path="$MODELS_SRC/$src_name"
    dest_path="$OUTPUT_DIR/$dest_name"

    if [ ! -d "$src_path" ]; then
        echo "  SKIP  $src_name -> $dest_name (source not found at $src_path)"
        SKIPPED=$((SKIPPED + 1))
        continue
    fi

    if [ -d "$dest_path" ]; then
        echo "  UPDATE $dest_name (replacing existing)"
        rm -rf "$dest_path"
    fi

    cp -r "$src_path" "$dest_path"
    size=$(du -sh "$dest_path" | cut -f1)
    echo "  COPY  $src_name -> $dest_name ($size)"
    COPIED=$((COPIED + 1))
done

echo ""
echo "=== Bundle Complete ==="
echo "Output: $OUTPUT_DIR"
echo "Copied: $COPIED models, Skipped: $SKIPPED"
total_size=$(du -sh "$OUTPUT_DIR" | cut -f1)
echo "Total size: $total_size"

# Verify all expected subdirectories exist
echo ""
echo "=== Verification ==="
EXPECTED="ner ner_lite scrfd blazeface ocr nsfw_classifier ri"
ALL_OK=true
for name in $EXPECTED; do
    if [ -d "$OUTPUT_DIR/$name" ]; then
        echo "  OK  $name"
    else
        echo "  MISSING  $name"
        ALL_OK=false
    fi
done

if $ALL_OK; then
    echo ""
    echo "All models bundled successfully."
else
    echo ""
    echo "WARNING: Some models are missing. Check source directory."
    exit 1
fi
