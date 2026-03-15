#!/usr/bin/env bash
# test_embedded_l0_all.sh — Run the L0 Embedded test suite via cargo test.
#
# Exercises OpenObscureMobile (UniFFI-level API) directly — no HTTP server required.
# Writes output to:
#   test/data/output/<category>/json/<name>_l0_embedded.json
#   test/data/output/<category>/redacted/<name>_l0_embedded.<ext>
#   test/data/output/Visual_PII/json/<name>_l0_embedded.json   (model-gated)
#   test/data/output/Audio_PII/json/<name>_l0_embedded.json
#
# Usage:
#   ./test/scripts/test_embedded_l0_all.sh [--text-only] [--image-only] [--audio-only]
#
# Options:
#   --text-only    Run only text category tests
#   --image-only   Run only image pipeline tests (requires ONNX models)
#   --audio-only   Run only audio transcript tests
#   (no args)      Run all embedded tests
#
# Environment:
#   CARGO_FLAGS    Extra flags passed to cargo test (e.g. --release)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(dirname "$(dirname "$SCRIPT_DIR")")"
MANIFEST_PATH="$REPO_ROOT/openobscure-core/Cargo.toml"

TEXT_ONLY=false
IMAGE_ONLY=false
AUDIO_ONLY=false
CARGO_FLAGS="${CARGO_FLAGS:-}"

for arg in "$@"; do
  case "$arg" in
    --text-only)  TEXT_ONLY=true ;;
    --image-only) IMAGE_ONLY=true ;;
    --audio-only) AUDIO_ONLY=true ;;
    *) echo "Unknown option: $arg"; exit 1 ;;
  esac
done

echo "============================================"
echo "  L0 Embedded Test Suite"
echo "  $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo "============================================"
echo ""

# ─── Build filter ──────────────────────────────────────────────

if [[ "$TEXT_ONLY" == "true" ]]; then
  FILTER="test_text_all_categories"
  echo "  Mode: text only"
elif [[ "$IMAGE_ONLY" == "true" ]]; then
  FILTER="test_image_all_visual_pii_files"
  echo "  Mode: image only (requires ONNX models)"
elif [[ "$AUDIO_ONLY" == "true" ]]; then
  FILTER="test_audio_transcript_all"
  echo "  Mode: audio only"
else
  FILTER="embedded_suite"
  echo "  Mode: all (text + image + audio + RI + FPE roundtrip)"
fi

echo ""

# ─── Run cargo test ────────────────────────────────────────────

START_S=$(date +%s)

set +e
cargo test \
  --manifest-path "$MANIFEST_PATH" \
  --tests "$FILTER" \
  $CARGO_FLAGS \
  -- --nocapture 2>&1
EXIT_CODE=$?
set -e

END_S=$(date +%s)
ELAPSED=$(( END_S - START_S ))

echo ""
echo "============================================"
if [[ $EXIT_CODE -eq 0 ]]; then
  echo "  L0 Embedded: PASSED (${ELAPSED}s)"
else
  echo "  L0 Embedded: FAILED (${ELAPSED}s, exit=$EXIT_CODE)"
fi
echo "============================================"
echo ""

# ─── Count outputs ─────────────────────────────────────────────

OUTPUT_DIR="$REPO_ROOT/test/data/output"
L0_JSON_COUNT=0
while IFS= read -r -d '' _; do
  L0_JSON_COUNT=$(( L0_JSON_COUNT + 1 ))
done < <(find "$OUTPUT_DIR" -path "*/json/*_l0_embedded.json" -print0 2>/dev/null)

echo "  L0 Embedded JSON outputs: $L0_JSON_COUNT"
echo ""

exit $EXIT_CODE
