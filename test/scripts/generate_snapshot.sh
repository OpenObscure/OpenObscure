#!/usr/bin/env bash
# generate_snapshot.sh — Generate snapshot.json from current test output for regression testing.
#
# Reads all gateway/audio/visual output JSONs and captures exact detection counts.
# Used by validate_results.sh --strict to catch any count changes.
#
# Usage:
#   ./test/scripts/generate_snapshot.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
TEST_DIR="$(dirname "$SCRIPT_DIR")"
OUTPUT_DIR="$TEST_DIR/data/output"
SNAPSHOT="$TEST_DIR/snapshot.json"

if [[ ! -d "$OUTPUT_DIR" ]]; then
  echo "Error: No output directory found: $OUTPUT_DIR"
  echo "Run the test suite first to generate outputs."
  exit 2
fi

TIMESTAMP=$(date -u +%Y-%m-%dT%H:%M:%SZ)

# ─── Gateway entries ──────────────────────────────────────────
gateway_json="{}"
while IFS= read -r -d '' gw_file; do
  category=$(echo "$gw_file" | sed "s|$OUTPUT_DIR/||" | cut -d'/' -f1)
  orig_file=$(jq -r '.file' "$gw_file")
  key="${category}/${orig_file}"
  total=$(jq '.total_matches // 0' "$gw_file")
  types=$(jq -c '.type_summary // {}' "$gw_file")
  gateway_json=$(echo "$gateway_json" | jq --arg k "$key" --argjson t "$total" --argjson ts "$types" \
    '. + {($k): {"total_matches": $t, "type_summary": $ts}}')
done < <(find "$OUTPUT_DIR" -path "*/json/*_gateway.json" -print0 2>/dev/null | sort -z)

# ─── Audio entries ────────────────────────────────────────────
audio_json="{}"
while IFS= read -r -d '' audio_file; do
  orig_file=$(jq -r '.file' "$audio_file")
  key="Audio_PII/${orig_file}"
  pii=$(jq '.kws_results.pii_detected // false' "$audio_file")
  kw=$(jq -r '.kws_results.keywords // ""' "$audio_file")
  action=$(jq -r '.kws_results.action // "UNKNOWN"' "$audio_file")
  audio_json=$(echo "$audio_json" | jq --arg k "$key" --argjson p "$pii" --arg kw "$kw" --arg a "$action" \
    '. + {($k): {"pii_detected": $p, "keywords": $kw, "action": $a}}')
done < <(find "$OUTPUT_DIR/Audio_PII" -path "*/json/*_audio.json" -print0 2>/dev/null | sort -z)

# ─── Visual entries ───────────────────────────────────────────
visual_json="{}"
while IFS= read -r -d '' visual_file; do
  orig_file=$(jq -r '.file' "$visual_file")
  subcategory=$(jq -r '.subcategory // "unknown"' "$visual_file")
  key="Visual_PII/${orig_file}"
  faces=$(jq '.pipeline_results.faces_blurred // 0' "$visual_file")
  text_regions=$(jq '.pipeline_results.text_regions_detected // 0' "$visual_file")
  nsfw_blocked=$(jq '.pipeline_results.nsfw_blocked // false' "$visual_file")
  screenshot_detected=$(jq '.pipeline_results.screenshot_detected // false' "$visual_file")
  visual_json=$(echo "$visual_json" | jq --arg k "$key" --arg sub "$subcategory" \
    --argjson f "$faces" --argjson tr "$text_regions" \
    --argjson nsfw "$nsfw_blocked" --argjson ss "$screenshot_detected" \
    '. + {($k): {"subcategory": $sub, "faces_blurred": $f, "text_regions_detected": $tr, "nsfw_blocked": $nsfw, "screenshot_detected": $ss}}')
done < <(find "$OUTPUT_DIR/Visual_PII" -path "*/json/*_visual.json" -print0 2>/dev/null | sort -z)

# ─── Assemble snapshot ───────────────────────────────────────
gw_count=$(echo "$gateway_json" | jq 'length')
audio_count=$(echo "$audio_json" | jq 'length')
visual_count=$(echo "$visual_json" | jq 'length')

jq -n \
  --arg ts "$TIMESTAMP" \
  --argjson gw_count "$gw_count" \
  --argjson audio_count "$audio_count" \
  --argjson visual_count "$visual_count" \
  --argjson gateway "$gateway_json" \
  --argjson audio "$audio_json" \
  --argjson visual "$visual_json" \
  '{
    _meta: {
      version: "1.0",
      generated: $ts,
      description: "Exact detection counts for regression testing. Regenerate with: ./test/scripts/generate_snapshot.sh",
      gateway_files: $gw_count,
      audio_files: $audio_count,
      visual_files: $visual_count
    },
    gateway: $gateway,
    audio: $audio,
    visual: $visual
  }' > "$SNAPSHOT"

echo "Snapshot generated: $SNAPSHOT"
echo "  Gateway: $gw_count files"
echo "  Audio:   $audio_count files"
echo "  Visual:  $visual_count files"
echo ""
echo "Remember to regenerate after scanner changes:"
echo "  ./test/scripts/generate_snapshot.sh"
