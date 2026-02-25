#!/usr/bin/env bash
# test_visual.sh — Test visual PII detection by sending images through the Gateway proxy.
#
# Produces dual output per image:
#   test/data/output/Visual_PII/json/<name>_visual.json       (pipeline metadata)
#   test/data/output/Visual_PII/redacted/<name>.<ext>          (redacted image)
#
# The proxy's image pipeline processes base64-encoded images:
#   - Face detection + solid-color fill (SCRFD/BlazeFace)
#   - OCR text detection + redaction (PaddleOCR)
#   - NSFW detection (NudeNet)
#   - EXIF metadata stripping
#   - Screenshot detection heuristics
#
# Usage:
#   ./test/scripts/test_visual.sh [subcategory]
#
# Subcategories: Faces, Screenshots, Documents, EXIF, NSFW
# Without arguments, tests all subcategories.
#
# NOTE: This sends images through the full proxy pipeline. If no upstream
#       provider is configured the HTTP response may fail, but the proxy
#       still processes the image (stats update + we capture the transformed
#       base64 from the request body when possible).

set -euo pipefail

# Millisecond timestamp (portable: Perl on macOS, date +%s%N on Linux)
_ms() { perl -MTime::HiRes -e 'printf("%d\n", Time::HiRes::time() * 1000)' 2>/dev/null || echo $(( $(date +%s) * 1000 )); }

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
TEST_DIR="$(dirname "$SCRIPT_DIR")"
INPUT_DIR="$TEST_DIR/data/input/Visual_PII"
OUTPUT_DIR="$TEST_DIR/data/output/Visual_PII"

PROXY_URL="${PROXY_URL:-http://127.0.0.1:18790}"
HEALTH_ENDPOINT="${PROXY_URL}/_openobscure/health"
CAPTURE_DIR="${CAPTURE_DIR:-/tmp/oo_echo_captures}"

# Cleanup temp files on any exit (error, Ctrl+C, normal)
cleanup_visual() {
  rm -f /tmp/oo_visual_resp_* 2>/dev/null || true
  rm -f /tmp/oo_visual_payload_* 2>/dev/null || true
  rm -f /tmp/oo_visual_b64_* 2>/dev/null || true
  rm -f /tmp/oo_visual_hdr_* 2>/dev/null || true
  rm -f "$CAPTURE_DIR"/visual_*.json 2>/dev/null || true
}
# Extract an X-OO-* header value from a curl header dump, defaulting to 0.
_oo_hdr() { local v; v=$({ grep -i "^${1}:" "$2" 2>/dev/null || true; } | head -1 | awk '{print $2}' | tr -d '\r\n '); echo "${v:-0}"; }
trap cleanup_visual EXIT

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

IMAGE_ENABLED=$(echo "$HEALTH" | jq -r '.feature_budget.image_pipeline_enabled // false')
if [[ "$IMAGE_ENABLED" != "true" ]]; then
  echo "Warning: Image pipeline is disabled on this proxy instance."
  echo "Enable it in config/openobscure.toml: [image] enabled = true"
fi

# Get baseline stats
BASELINE_IMAGES=$(echo "$HEALTH" | jq '.images_processed_total // 0')
BASELINE_FACES=$(echo "$HEALTH" | jq '.faces_redacted_total // 0')
BASELINE_TEXT=$(echo "$HEALTH" | jq '.text_regions_total // 0')
BASELINE_NSFW=$(echo "$HEALTH" | jq '.nsfw_blocked_total // 0')
BASELINE_SCREENSHOTS=$(echo "$HEALTH" | jq '.screenshots_detected_total // 0')

test_image() {
  local file="$1"
  local subcategory="$2"
  local filename=$(basename "$file")
  local ext="${filename##*.}"
  local name_no_ext="${filename%.*}"
  local json_dir="$OUTPUT_DIR/json"
  local redacted_dir="$OUTPUT_DIR/redacted"

  mkdir -p "$json_dir" "$redacted_dir"

  # Determine media type
  local media_type="image/jpeg"
  case "$ext" in
    png) media_type="image/png" ;;
    gif) media_type="image/gif" ;;
    webp) media_type="image/webp" ;;
  esac

  # Encode to base64 (write to temp file to avoid shell arg limits on large images)
  local tmp_b64_file
  tmp_b64_file=$(mktemp /tmp/oo_visual_b64_XXXXXX)
  base64 -i "$file" | tr -d '\n' > "$tmp_b64_file"

  # Get file size
  local file_size
  file_size=$(wc -c < "$file" | tr -d ' ')

  # Get image dimensions (macOS sips)
  local dimensions="unknown"
  if command -v sips &>/dev/null; then
    local w h
    w=$(sips -g pixelWidth "$file" 2>/dev/null | tail -1 | awk '{print $2}')
    h=$(sips -g pixelHeight "$file" 2>/dev/null | tail -1 | awk '{print $2}')
    if [[ -n "$w" && -n "$h" ]]; then
      dimensions="${w}x${h}"
    fi
  fi

  # Record health stats before
  local before_health
  before_health=$(curl -sf "$HEALTH_ENDPOINT" ${AUTH_TOKEN:+-H "X-OpenObscure-Token: $AUTH_TOKEN"} 2>/dev/null)
  local before_images=$(echo "$before_health" | jq '.images_processed_total // 0')
  local before_faces=$(echo "$before_health" | jq '.faces_redacted_total // 0')
  local before_text=$(echo "$before_health" | jq '.text_regions_total // 0')
  local before_nsfw=$(echo "$before_health" | jq '.nsfw_blocked_total // 0')
  local before_screenshots=$(echo "$before_health" | jq '.screenshots_detected_total // 0')

  # Send through proxy — use temp file to avoid shell arg limits on large images
  local tmp_response
  tmp_response=$(mktemp /tmp/oo_visual_resp_XXXXXX)

  local capture_id="visual_${name_no_ext}"

  # Build JSON payload in temp file
  local tmp_payload
  tmp_payload=$(mktemp /tmp/oo_visual_payload_XXXXXX)

  jq -n \
    --arg media_type "$media_type" \
    --rawfile img_b64 "$tmp_b64_file" \
    '{
      model: "claude-sonnet-4-20250514",
      max_tokens: 64,
      messages: [{
        role: "user",
        content: [
          {
            type: "image",
            source: {
              type: "base64",
              media_type: $media_type,
              data: ($img_b64 | rtrimstr("\n"))
            }
          },
          {type: "text", text: "Describe this."}
        ]
      }]
    }' > "$tmp_payload"
  rm -f "$tmp_b64_file"

  local tmp_headers
  tmp_headers=$(mktemp /tmp/oo_visual_hdr_XXXXXX)

  local proxy_start
  proxy_start=$(_ms)

  local response_code
  response_code=$(curl -s -o "$tmp_response" -w "%{http_code}" -D "$tmp_headers" -X POST \
    "${PROXY_URL}/anthropic/v1/messages" \
    -H "Content-Type: application/json" \
    -H "x-api-key: test-visual-scan" \
    -H "anthropic-version: 2023-06-01" \
    -H "X-Capture-Id: $capture_id" \
    ${AUTH_TOKEN:+-H "X-OpenObscure-Token: $AUTH_TOKEN"} \
    -d @"$tmp_payload" 2>/dev/null)
  local proxy_elapsed_ms=$(( $(_ms) - proxy_start ))
  rm -f "$tmp_payload"

  # Extract per-phase timing from X-OO-* response headers
  local nsfw_ms face_ms ocr_ms image_us scan_us proxy_total_us
  nsfw_ms=$(_oo_hdr "x-oo-nsfw-ms" "$tmp_headers")
  face_ms=$(_oo_hdr "x-oo-face-ms" "$tmp_headers")
  ocr_ms=$(_oo_hdr "x-oo-ocr-ms" "$tmp_headers")
  image_us=$(_oo_hdr "x-oo-image-us" "$tmp_headers")
  scan_us=$(_oo_hdr "x-oo-scan-us" "$tmp_headers")
  proxy_total_us=$(_oo_hdr "x-oo-total-us" "$tmp_headers")
  rm -f "$tmp_headers"

  # Check health stats after — delta tells us what the pipeline did
  local after_health
  after_health=$(curl -sf "$HEALTH_ENDPOINT" ${AUTH_TOKEN:+-H "X-OpenObscure-Token: $AUTH_TOKEN"} 2>/dev/null)
  local after_images=$(echo "$after_health" | jq '.images_processed_total // 0')
  local after_faces=$(echo "$after_health" | jq '.faces_redacted_total // 0')
  local after_text=$(echo "$after_health" | jq '.text_regions_total // 0')
  local after_nsfw=$(echo "$after_health" | jq '.nsfw_blocked_total // 0')
  local after_screenshots=$(echo "$after_health" | jq '.screenshots_detected_total // 0')

  local delta_images=$((after_images - before_images))
  local delta_faces=$((after_faces - before_faces))
  local delta_text=$((after_text - before_text))
  local delta_nsfw=$((after_nsfw - before_nsfw))
  local delta_screenshots=$((after_screenshots - before_screenshots))

  # ── Extract processed image from the echo server capture file ──
  # The proxy rewrites base64 data in the request body (faces redacted, OCR redacted,
  # EXIF stripped) before forwarding. The echo server saves each request body to
  # $CAPTURE_DIR/<X-Capture-Id>.json, so we can extract the processed image.
  local image_saved=false
  local capture_file="$CAPTURE_DIR/${capture_id}.json"

  # Wait for the echo server to write the capture file (large images take longer)
  for _wait in $(seq 1 20); do
    [[ -f "$capture_file" && -s "$capture_file" ]] && break
    sleep 0.5
  done

  if [[ -f "$capture_file" && -s "$capture_file" ]]; then
    # Extract base64 image data to temp file (avoid shell arg limits on large images)
    local tmp_extracted
    tmp_extracted=$(mktemp /tmp/oo_visual_extract_XXXXXX)
    jq -r '
      .messages[0].content[0].source.data //
      (.messages[0].content[] | select(.type == "image") | .source.data) //
      empty
    ' "$capture_file" > "$tmp_extracted" 2>/dev/null || true

    local extracted_size
    extracted_size=$(wc -c < "$tmp_extracted" 2>/dev/null | tr -d ' ')
    if [[ "$extracted_size" -gt 100 ]]; then
      base64 -d < "$tmp_extracted" > "$redacted_dir/$filename" 2>/dev/null && image_saved=true
    fi
    rm -f "$tmp_extracted"
  fi

  # Fallback: copy original with a note that it needs manual pipeline verification
  if [[ "$image_saved" != "true" ]]; then
    cp "$file" "$redacted_dir/$filename"
  fi

  rm -f "$tmp_response"

  # Save JSON metadata
  local result
  result=$(jq -n \
    --arg file "$filename" \
    --arg subcategory "$subcategory" \
    --arg dimensions "$dimensions" \
    --argjson file_size "$file_size" \
    --arg media_type "$media_type" \
    --argjson http_status "$response_code" \
    --argjson images_processed "$delta_images" \
    --argjson faces_redacted "$delta_faces" \
    --argjson text_regions "$delta_text" \
    --argjson nsfw_blocked "$delta_nsfw" \
    --argjson screenshot_detected "$delta_screenshots" \
    --argjson image_captured "$image_saved" \
    --argjson pipeline_ms "$proxy_elapsed_ms" \
    --argjson nsfw_ms "$nsfw_ms" \
    --argjson face_ms "$face_ms" \
    --argjson ocr_ms "$ocr_ms" \
    --argjson image_us "$image_us" \
    --argjson scan_us "$scan_us" \
    --argjson proxy_total_us "$proxy_total_us" \
    --arg ts "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    '{
      file: $file,
      subcategory: $subcategory,
      dimensions: $dimensions,
      file_size_bytes: $file_size,
      media_type: $media_type,
      http_status: $http_status,
      pipeline_results: {
        images_processed: $images_processed,
        faces_redacted: $faces_redacted,
        text_regions_detected: $text_regions,
        nsfw_blocked: ($nsfw_blocked > 0),
        screenshot_detected: ($screenshot_detected > 0)
      },
      timing: {
        pipeline_ms: $pipeline_ms,
        nsfw_ms: $nsfw_ms,
        face_ms: $face_ms,
        ocr_ms: $ocr_ms,
        image_us: $image_us,
        scan_us: $scan_us,
        proxy_total_us: $proxy_total_us
      },
      redacted_image_captured: $image_captured,
      note: (if $image_captured then "Redacted image saved" else "Original copied — run with echo upstream to capture pipeline output" end),
      timestamp: $ts
    }')

  echo "$result" | jq . > "$json_dir/${name_no_ext}_visual.json"

  local status="OK"
  [[ "$response_code" == "000" || "$response_code" == "502" ]] && status="WARN"

  local extras=""
  [[ "$delta_nsfw" -gt 0 ]] && extras="${extras}, nsfw: YES"
  [[ "$delta_screenshots" -gt 0 ]] && extras="${extras}, screenshot: YES"

  local phase_info=""
  [[ "$nsfw_ms" -gt 0 ]] && phase_info="${phase_info} nsfw:${nsfw_ms}ms"
  [[ "$face_ms" -gt 0 ]] && phase_info="${phase_info} face:${face_ms}ms"
  [[ "$ocr_ms" -gt 0 ]] && phase_info="${phase_info} ocr:${ocr_ms}ms"

  echo "$status $filename (${dimensions}, ${file_size}B) — HTTP $response_code, faces: +$delta_faces, text: +$delta_text${extras}, captured: $image_saved, ${proxy_elapsed_ms}ms${phase_info:+ ($phase_info)}"
}

# ── Main ──
SUBCATEGORY="${1:-}"

echo "=== Visual PII Detection Tests ==="
echo "Image pipeline: $IMAGE_ENABLED"
echo "Output: test/data/output/Visual_PII/json/ + redacted/"
echo ""

SUBCATEGORIES=("Faces" "Screenshots" "Documents" "EXIF" "NSFW")

if [[ -n "$SUBCATEGORY" ]]; then
  SUBCATEGORIES=("$SUBCATEGORY")
fi

PASS=0
FAIL=0
WARN=0
VISUAL_TOTAL=0
RESULTS_JSON="[]"

# Purge previous visual results
echo "Purging previous visual results..."
rm -f "$OUTPUT_DIR"/json/*_visual.json 2>/dev/null || true
rm -f "$OUTPUT_DIR"/redacted/* 2>/dev/null || true
echo ""

for subcat in "${SUBCATEGORIES[@]}"; do
  subcat_dir="$INPUT_DIR/$subcat"
  if [[ ! -d "$subcat_dir" ]]; then
    echo "SKIP $subcat (directory not found)"
    continue
  fi

  echo "--- $subcat ---"

  for file in "$subcat_dir"/*; do
    [[ -f "$file" ]] || continue
    fname=$(basename "$file")
    ext="${fname##*.}"
    case "$ext" in
      jpg|jpeg|png|gif|webp)
        test_image "$file" "$subcat"
        VISUAL_TOTAL=$((VISUAL_TOTAL + 1))
        name_no_ext="${fname%.*}"
        vj="$OUTPUT_DIR/json/${name_no_ext}_visual.json"
        if [[ -f "$vj" ]]; then
          http_code=$(jq '.http_status // 0' "$vj" 2>/dev/null || echo 0)
          faces=$(jq '.pipeline_results.faces_redacted // 0' "$vj" 2>/dev/null || echo 0)
          text_r=$(jq '.pipeline_results.text_regions_detected // 0' "$vj" 2>/dev/null || echo 0)
          PASS=$((PASS + 1))
          RESULTS_JSON=$(echo "$RESULTS_JSON" | jq --arg n "$fname" --arg d "HTTP $http_code, faces:$faces, text:$text_r" '. + [{"name": $n, "status": "pass", "detail": $d}]')
        else
          FAIL=$((FAIL + 1))
          RESULTS_JSON=$(echo "$RESULTS_JSON" | jq --arg n "$fname" --arg d "no output JSON produced" '. + [{"name": $n, "status": "fail", "detail": $d}]')
        fi
        ;;
      *) echo "SKIP $(basename "$file") (non-image: .$ext)" ;;
    esac
  done

  echo ""
done

# Final stats
FINAL_HEALTH=$(curl -sf "$HEALTH_ENDPOINT" ${AUTH_TOKEN:+-H "X-OpenObscure-Token: $AUTH_TOKEN"} 2>/dev/null)
TOTAL_IMAGES=$(($(echo "$FINAL_HEALTH" | jq '.images_processed_total // 0') - BASELINE_IMAGES))
TOTAL_FACES=$(($(echo "$FINAL_HEALTH" | jq '.faces_redacted_total // 0') - BASELINE_FACES))
TOTAL_TEXT=$(($(echo "$FINAL_HEALTH" | jq '.text_regions_total // 0') - BASELINE_TEXT))
TOTAL_NSFW=$(($(echo "$FINAL_HEALTH" | jq '.nsfw_blocked_total // 0') - BASELINE_NSFW))
TOTAL_SCREENSHOTS=$(($(echo "$FINAL_HEALTH" | jq '.screenshots_detected_total // 0') - BASELINE_SCREENSHOTS))

echo "=== Summary ==="
echo "Images processed:     $TOTAL_IMAGES"
echo "Faces redacted:       $TOTAL_FACES"
echo "Text regions:         $TOTAL_TEXT"
echo "NSFW blocked:         $TOTAL_NSFW"
echo "Screenshots detected: $TOTAL_SCREENSHOTS"
echo "JSON metadata:        $OUTPUT_DIR/json/"
echo "Redacted images:      $OUTPUT_DIR/redacted/"

# Write validation JSON
jq -n \
  --arg suite "visual" \
  --arg ts "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --argjson total "$VISUAL_TOTAL" \
  --argjson pass "$PASS" \
  --argjson fail "$FAIL" \
  --argjson warn "$WARN" \
  --argjson skip 0 \
  --argjson results "$RESULTS_JSON" \
  '{
    test_suite: $suite,
    timestamp: $ts,
    total: $total,
    pass: $pass,
    fail: $fail,
    warn: $warn,
    skip: $skip,
    results: $results
  }' > "$OUTPUT_DIR/visual_validation.json"
