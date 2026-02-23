#!/usr/bin/env bash
# test_gateway_category.sh — Test all files in a specific input category via Gateway FPE.
#
# Produces dual output per file:
#   <output_dir>/<category>/json/<filename>_gateway.json     (NER metadata)
#   <output_dir>/<category>/redacted/<filename>               (FPE-encrypted)
#
# Usage:
#   ./test/scripts/test_gateway_category.sh <category>
#
# Categories: PII_Detection, Multilingual_PII, Code_Config_PII,
#             Structured_Data_PII, Agent_Tool_Results

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
TEST_DIR="$(dirname "$SCRIPT_DIR")"
INPUT_DIR="$TEST_DIR/data/input"
OUTPUT_DIR="$TEST_DIR/data/output"

CATEGORY="${1:-}"

if [[ -z "$CATEGORY" ]]; then
  echo "Usage: $0 <category>"
  echo ""
  echo "Available categories:"
  for dir in "$INPUT_DIR"/*/; do
    name=$(basename "$dir")
    if [[ "$name" != "Visual_PII" && "$name" != "Audio_PII" ]]; then
      count=$(find "$dir" -maxdepth 1 -type f | wc -l | tr -d ' ')
      echo "  $name ($count files)"
    fi
  done
  exit 1
fi

CAT_INPUT="$INPUT_DIR/$CATEGORY"
CAT_OUTPUT="$OUTPUT_DIR/$CATEGORY"

if [[ ! -d "$CAT_INPUT" ]]; then
  echo "Error: Category not found: $CAT_INPUT"
  exit 1
fi

mkdir -p "$CAT_OUTPUT/json" "$CAT_OUTPUT/redacted"

# Purge previous gateway results for this category
rm -f "$CAT_OUTPUT"/json/*_gateway.json 2>/dev/null || true
rm -f "$CAT_OUTPUT"/redacted/* 2>/dev/null || true

echo "=== Gateway FPE Test: $CATEGORY ==="
echo ""

TOTAL_FILES=0
TOTAL_MATCHES=0
TOTAL_TIME_MS=0
MAX_TIME_MS=0
MAX_TIME_FILE=""
PASS=0
FAIL=0

for file in "$CAT_INPUT"/*; do
  [[ -f "$file" ]] || continue

  filename=$(basename "$file")
  ext="${filename##*.}"

  # Skip non-text files
  case "$ext" in
    txt|csv|tsv|env|py|yaml|yml|json|sh|md|log) ;;
    *) echo "SKIP $filename (non-text: .$ext)"; continue ;;
  esac

  if "$SCRIPT_DIR/test_gateway_file.sh" "$file" "$CAT_OUTPUT" 2>/dev/null; then
    name_no_ext="${filename%.*}"
    json_file="$CAT_OUTPUT/json/${name_no_ext}_gateway.json"
    if [[ -f "$json_file" ]]; then
      matches=$(jq '.total_matches' "$json_file" 2>/dev/null || echo 0)
      TOTAL_MATCHES=$((TOTAL_MATCHES + matches))
      file_time_ms=$(jq '.timing.total_ms // 0' "$json_file" 2>/dev/null || echo 0)
      TOTAL_TIME_MS=$((TOTAL_TIME_MS + file_time_ms))
      if [[ "$file_time_ms" -gt "$MAX_TIME_MS" ]]; then
        MAX_TIME_MS=$file_time_ms
        MAX_TIME_FILE=$filename
      fi
    fi
    PASS=$((PASS + 1))
  else
    echo "FAIL $filename"
    FAIL=$((FAIL + 1))
  fi

  TOTAL_FILES=$((TOTAL_FILES + 1))
done

AVG_TIME_MS=0
if [[ "$TOTAL_FILES" -gt 0 ]]; then
  AVG_TIME_MS=$((TOTAL_TIME_MS / TOTAL_FILES))
fi

echo ""
echo "=== Summary ==="
echo "Category:       $CATEGORY"
echo "Files tested:   $TOTAL_FILES"
echo "Passed:         $PASS"
echo "Failed:         $FAIL"
echo "Total matches:  $TOTAL_MATCHES"
echo "Timing:         ${TOTAL_TIME_MS}ms total (avg: ${AVG_TIME_MS}ms/file, max: ${MAX_TIME_MS}ms — ${MAX_TIME_FILE:-n/a})"
echo "JSON results:   $CAT_OUTPUT/json/"
echo "FPE redacted:   $CAT_OUTPUT/redacted/"
