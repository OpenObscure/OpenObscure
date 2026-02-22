#!/usr/bin/env bash
# test_audio.sh — Test KWS-based audio PII detection through the proxy.
#
# The proxy's voice pipeline detects audio blocks, decodes them to PCM,
# runs keyword spotting (sherpa-onnx KWS Zipformer) for PII trigger phrases,
# and strips only audio blocks where PII keywords are detected.
# Audio without PII keywords passes through unchanged.
#
# This script:
#   1. Sends each audio file as an Anthropic audio block through the proxy
#   2. Checks the echo-captured response to determine if audio was:
#      - STRIPPED: KWS detected PII keywords → audio replaced with notice
#      - CLEAN: KWS scanned, no PII keywords found → audio passed through
#      - PASS-THRU: No KWS engine loaded → audio passed through unscanned
#   3. Reports detection results and detected keywords
#
# Usage:
#   ./test/scripts/test_audio.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
TEST_DIR="$(dirname "$SCRIPT_DIR")"
INPUT_DIR="$TEST_DIR/data/input/Audio_PII"
OUTPUT_DIR="$TEST_DIR/data/output/Audio_PII"

PROXY_URL="${PROXY_URL:-http://127.0.0.1:18790}"
HEALTH_ENDPOINT="${PROXY_URL}/_openobscure/health"

AUDIO_TIMEOUT="${AUDIO_TIMEOUT:-60}"
CAPTURE_DIR="${CAPTURE_DIR:-/tmp/oo_echo_captures}"

# Cleanup temp files on any exit
cleanup_audio() {
  rm -f /tmp/oo_audio_*.json 2>/dev/null || true
  rm -f /tmp/oo_audio_payload_*.json 2>/dev/null || true
  rm -f /tmp/oo_audio_b64_*.txt 2>/dev/null || true
}
trap cleanup_audio EXIT

# Auth token
if [[ -z "${AUTH_TOKEN:-}" ]]; then
  TOKEN_FILE="$HOME/.openobscure/.auth-token"
  if [[ -f "$TOKEN_FILE" ]]; then
    AUTH_TOKEN=$(cat "$TOKEN_FILE")
  fi
fi

# Check proxy is running
if [[ -n "${AUTH_TOKEN:-}" ]]; then
  HEALTH=$(curl -sf "$HEALTH_ENDPOINT" -H "X-OpenObscure-Token: $AUTH_TOKEN" 2>/dev/null || true)
else
  HEALTH=$(curl -sf "$HEALTH_ENDPOINT" 2>/dev/null || true)
fi
if [[ -z "$HEALTH" ]]; then
  echo "Error: Proxy not reachable at $PROXY_URL"
  exit 1
fi

DEVICE_TIER=$(echo "$HEALTH" | jq -r '.device_tier // "unknown"')

test_audio() {
  local file="$1"
  local filename
  filename=$(basename "$file")
  local ext="${filename##*.}"
  local name_no_ext="${filename%.*}"
  local json_dir="$OUTPUT_DIR/json"

  mkdir -p "$json_dir"

  # Determine media type from extension
  local media_type="audio/wav"
  case "$ext" in
    mp3) media_type="audio/mp3" ;;
    ogg) media_type="audio/ogg" ;;
    webm) media_type="audio/webm" ;;
    flac) media_type="audio/flac" ;;
  esac

  # Encode to base64
  local audio_b64
  audio_b64=$(base64 -i "$file" | tr -d '\n')

  # Get file size
  local file_size
  file_size=$(wc -c < "$file" | tr -d ' ')

  # Get audio duration
  local duration="unknown"
  if command -v afinfo &>/dev/null; then
    duration=$(afinfo "$file" 2>/dev/null | grep "estimated duration" | awk '{print $3}' || echo "unknown")
  elif command -v ffprobe &>/dev/null; then
    duration=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$file" 2>/dev/null || echo "unknown")
  fi

  local tmp_response
  tmp_response=$(mktemp /tmp/oo_audio_XXXXXX.json)

  # Build JSON payload
  local tmp_payload
  tmp_payload=$(mktemp /tmp/oo_audio_payload_XXXXXX.json)
  local tmp_b64_file
  tmp_b64_file=$(mktemp /tmp/oo_audio_b64_XXXXXX.txt)
  echo -n "$audio_b64" > "$tmp_b64_file"

  jq -n \
    --arg media_type "$media_type" \
    --rawfile audio_b64 "$tmp_b64_file" \
    '{
      model: "claude-sonnet-4-20250514",
      max_tokens: 64,
      messages: [{
        role: "user",
        content: [
          {
            type: "audio",
            source: {
              type: "base64",
              media_type: $media_type,
              data: ($audio_b64 | rtrimstr("\n"))
            }
          },
          {type: "text", text: "Transcribe this audio."}
        ]
      }]
    }' > "$tmp_payload"
  rm -f "$tmp_b64_file"

  # Unique capture ID for this request
  local capture_id="audio_${name_no_ext}_$$"

  # Build curl headers
  local -a curl_headers=(
    -H "Content-Type: application/json"
    -H "x-api-key: test-audio-scan"
    -H "anthropic-version: 2023-06-01"
    -H "X-Capture-Id: $capture_id"
  )
  if [[ -n "${AUTH_TOKEN:-}" ]]; then
    curl_headers+=(-H "X-OpenObscure-Token: $AUTH_TOKEN")
  fi

  # Send audio through proxy — echo server captures the processed request body
  local response_code
  response_code=$(curl -s -o "$tmp_response" -w "%{http_code}" \
    --max-time "$AUDIO_TIMEOUT" \
    -X POST "${PROXY_URL}/anthropic/v1/messages" \
    "${curl_headers[@]}" \
    -d @"$tmp_payload" 2>/dev/null || echo "000")
  rm -f "$tmp_payload"

  # Parse the CAPTURED request body (what the proxy sent upstream)
  # The echo server saves it to $CAPTURE_DIR/<capture_id>.json
  local pii_detected=false
  local keywords=""
  local action="UNKNOWN"

  local captured_body="$CAPTURE_DIR/${capture_id}.json"

  if [[ "$response_code" == "000" ]]; then
    action="TIMEOUT"
  elif [[ "$response_code" == "502" ]]; then
    action="ERROR"
  elif [[ -f "$captured_body" ]]; then
    # Check the first content block in the captured request body
    local first_block_type
    first_block_type=$(jq -r '.messages[0].content[0].type // empty' "$captured_body" 2>/dev/null || true)
    local first_block_text
    first_block_text=$(jq -r '.messages[0].content[0].text // empty' "$captured_body" 2>/dev/null || true)

    if [[ "$first_block_type" == "text" && "$first_block_text" == *"AUDIO_PII_DETECTED"* ]]; then
      pii_detected=true
      # Extract keywords from notice: [AUDIO_PII_DETECTED: keywords={...} — audio stripped]
      keywords=$(echo "$first_block_text" | sed -n 's/.*keywords={\(.*\)}.*/\1/p')
      action="PII_DETECTED"
    elif [[ "$first_block_type" == "audio" ]]; then
      # Audio block passed through unchanged — KWS scanned but found no PII
      action="CLEAN"
    else
      action="PASS-THRU"
    fi
    rm -f "$captured_body"
  fi

  # Save JSON metadata
  local result
  result=$(jq -n \
    --arg file "$filename" \
    --arg ext "$ext" \
    --arg media_type "$media_type" \
    --argjson file_size "$file_size" \
    --arg duration "$duration" \
    --argjson http_status "$response_code" \
    --argjson pii_detected "$pii_detected" \
    --arg keywords "$keywords" \
    --arg action "$action" \
    --arg device_tier "$DEVICE_TIER" \
    --arg ts "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    '{
      file: $file,
      extension: $ext,
      media_type: $media_type,
      file_size_bytes: $file_size,
      duration_secs: $duration,
      http_status: $http_status,
      kws_results: {
        pii_detected: $pii_detected,
        keywords: $keywords,
        action: $action
      },
      device_tier: $device_tier,
      timestamp: $ts
    }')

  local json_name="${name_no_ext}_${ext}_audio.json"
  echo "$result" | jq . > "$json_dir/$json_name"

  rm -f "$tmp_response"

  # Format output
  local status_icon="OK"
  local keywords_display=""
  if [[ "$action" == "PII_DETECTED" ]]; then
    status_icon="PII"
    keywords_display="  → $keywords"
  elif [[ "$action" == "CLEAN" ]]; then
    status_icon="OK"
    keywords_display="  (no PII)"
  elif [[ "$action" == "PASS-THRU" ]]; then
    status_icon="---"
    keywords_display="  (no KWS)"
  elif [[ "$action" == "TIMEOUT" ]]; then
    status_icon="ERR"
  fi

  printf "  %-4s %-12s %-35s %6sB  HTTP %s%s\n" \
    "$status_icon" "$action" "$filename" "$file_size" "$response_code" "$keywords_display"
}

# ── Main ──
echo "=== Audio PII Detection Tests (KWS) ==="
echo "Device tier: $DEVICE_TIER"
echo "Output: test/data/output/Audio_PII/json/"
echo ""

if [[ ! -d "$INPUT_DIR" ]]; then
  echo "Error: Audio input directory not found: $INPUT_DIR"
  exit 1
fi

# Purge previous audio results
echo "Purging previous audio results..."
rm -f "$OUTPUT_DIR"/json/*_audio.json 2>/dev/null || true
echo ""

TOTAL=0
PII_FOUND=0
CLEAN=0
PASSTHRU=0

for file in "$INPUT_DIR"/*; do
  [[ -f "$file" ]] || continue
  ext="${file##*.}"
  case "$ext" in
    wav|mp3|ogg|webm|flac)
      test_audio "$file"
      TOTAL=$((TOTAL + 1))
      ;;
    *)
      echo "  SKIP $(basename "$file") (unsupported: .$ext)"
      ;;
  esac
done

# Count results
for jf in "$OUTPUT_DIR"/json/*_audio.json; do
  [[ -f "$jf" ]] || continue
  action=$(jq -r '.kws_results.action' "$jf" 2>/dev/null || echo "UNKNOWN")
  case "$action" in
    PII_DETECTED) PII_FOUND=$((PII_FOUND + 1)) ;;
    CLEAN) CLEAN=$((CLEAN + 1)) ;;
    PASS-THRU) PASSTHRU=$((PASSTHRU + 1)) ;;
  esac
done

echo ""
echo "=== Summary ==="
echo "Audio files tested:  $TOTAL"
echo "PII detected+stripped: $PII_FOUND"
echo "Clean (no PII):      $CLEAN"
echo "Pass-through (no KWS): $PASSTHRU"
echo "JSON metadata:       $OUTPUT_DIR/json/"
echo ""
if [[ "$PASSTHRU" -eq "$TOTAL" && "$TOTAL" -gt 0 ]]; then
  echo "WARNING: No KWS engine loaded — all audio passed through unscanned."
  echo "         Ensure voice.enabled=true and KWS models are downloaded."
elif [[ "$PII_FOUND" -gt 0 ]]; then
  echo "KWS keyword spotting active: $PII_FOUND/$TOTAL audio files had PII detected."
else
  echo "KWS active but no PII keywords detected in any audio."
fi
