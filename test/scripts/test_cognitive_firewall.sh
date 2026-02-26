#!/usr/bin/env bash
# test_cognitive_firewall.sh — Generate Cognitive Firewall output with [OpenObscure] labels.
#
# Sends each input file through the proxy's Response Integrity (RI) pipeline
# via the RI mock server's "echo" mode.  The mock server returns the file's
# content as a mock LLM response, the proxy's RI scanner detects persuasion
# tactics and prepends [OpenObscure] warning labels, and this script captures
# the labeled output.
#
# Produces:
#   test/data/output/Cognitive_Firewall/gateway/<filename>     — labeled text
#   test/data/output/Cognitive_Firewall/json/<filename>_gateway.json — metadata
#
# Requires:
#   - RI mock server: node test/scripts/mock/ri_mock_server.mjs
#   - Proxy with test/config/test_ri.toml (response_integrity enabled, log_only=false)
#
# Usage:
#   ./test/scripts/test_cognitive_firewall.sh
#
# Environment:
#   PROXY_URL   — Proxy base URL (default: http://127.0.0.1:18790)
#   AUTH_TOKEN   — Proxy auth token (default: read from ~/.openobscure/.auth-token)

set -euo pipefail

PROXY_URL="${PROXY_URL:-http://127.0.0.1:18790}"
HEALTH_ENDPOINT="${PROXY_URL}/_openobscure/health"
PROVIDER_ENDPOINT="${PROXY_URL}/anthropic/v1/messages"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
TEST_DIR="$(dirname "$SCRIPT_DIR")"
INPUT_DIR="$TEST_DIR/data/input/Cognitive_Firewall"
OUTPUT_DIR="$TEST_DIR/data/output/Cognitive_Firewall"
GATEWAY_DIR="$OUTPUT_DIR/gateway"
JSON_DIR="$OUTPUT_DIR/json"

mkdir -p "$GATEWAY_DIR" "$JSON_DIR"

# Auth token: env var > file > empty
if [[ -z "${AUTH_TOKEN:-}" ]]; then
  TOKEN_FILE="$HOME/.openobscure/.auth-token"
  if [[ -f "$TOKEN_FILE" ]]; then
    AUTH_TOKEN=$(cat "$TOKEN_FILE")
  else
    AUTH_TOKEN=""
  fi
fi

AUTH_HEADER=()
if [[ -n "$AUTH_TOKEN" ]]; then
  AUTH_HEADER=(-H "X-OpenObscure-Token: $AUTH_TOKEN")
fi

# Counters
PASS=0
FAIL=0
WARN=0
TOTAL=0
RESULTS_JSON="[]"

pass() {
  PASS=$((PASS + 1))
  TOTAL=$((TOTAL + 1))
  printf "  \033[32mPASS\033[0m  %-45s  %s\n" "$1" "$2"
  RESULTS_JSON=$(echo "$RESULTS_JSON" | jq --arg n "$1" --arg d "$2" '. + [{"name": $n, "status": "pass", "detail": $d}]')
}

fail() {
  FAIL=$((FAIL + 1))
  TOTAL=$((TOTAL + 1))
  printf "  \033[31mFAIL\033[0m  %-45s  %s\n" "$1" "$2"
  RESULTS_JSON=$(echo "$RESULTS_JSON" | jq --arg n "$1" --arg d "$2" '. + [{"name": $n, "status": "fail", "detail": $d}]')
}

warn() {
  WARN=$((WARN + 1))
  TOTAL=$((TOTAL + 1))
  printf "  \033[33mWARN\033[0m  %-45s  %s\n" "$1" "$2"
  RESULTS_JSON=$(echo "$RESULTS_JSON" | jq --arg n "$1" --arg d "$2" '. + [{"name": $n, "status": "warn", "detail": $d}]')
}

# Check proxy is up
HEALTH=$(curl -sf "${AUTH_HEADER[@]}" "$HEALTH_ENDPOINT" 2>/dev/null || true)
if [[ -z "$HEALTH" ]]; then
  echo "Error: Proxy not reachable at $PROXY_URL"
  exit 1
fi

# Check RI is enabled
RI_SCANS=$(echo "$HEALTH" | jq '.ri_scans_total // -1')
if [[ "$RI_SCANS" == "-1" ]]; then
  echo "Error: Response integrity not enabled. Use test/config/test_ri.toml"
  exit 1
fi

echo "=== Cognitive Firewall Gateway Tests ==="
echo ""
echo "Sending input files through proxy RI pipeline (echo mode)..."
echo "Output: test/data/output/Cognitive_Firewall/gateway/"
echo ""

# Process each input file
for INPUT_FILE in "$INPUT_DIR"/*.txt; do
  FILENAME=$(basename "$INPUT_FILE")
  NAME_NO_EXT="${FILENAME%.txt}"
  FILE_CONTENT=$(cat "$INPUT_FILE")

  # Build Anthropic API request with file content as user message
  # The mock server in "echo" mode returns user content as the assistant response
  REQUEST_JSON=$(jq -n \
    --arg content "$FILE_CONTENT" \
    '{
      model: "test",
      max_tokens: 4096,
      messages: [{role: "user", content: $content}]
    }')

  # Send through proxy → mock server (echo) → RI pipeline → labeled output
  RESP=$(curl -sf -X POST "$PROVIDER_ENDPOINT" \
    "${AUTH_HEADER[@]}" \
    -H "Content-Type: application/json" \
    -H "x-api-key: test-ri-scan" \
    -H "anthropic-version: 2023-06-01" \
    -H "X-Mock-Response: echo" \
    -d "$REQUEST_JSON" 2>/dev/null || echo "")

  if [[ -z "$RESP" ]]; then
    fail "$FILENAME" "empty response (proxy or mock server down?)"
    continue
  fi

  # Extract the text content from the Anthropic response
  RESP_TEXT=$(echo "$RESP" | jq -r '.content[0].text // ""' 2>/dev/null || echo "")

  if [[ -z "$RESP_TEXT" ]]; then
    fail "$FILENAME" "no text in response"
    continue
  fi

  # Save gateway output (the labeled text)
  echo "$RESP_TEXT" > "$GATEWAY_DIR/$FILENAME"

  # Determine if RI label was prepended (always at the very start of the text)
  HAS_LABEL=false
  LABEL_LINE=""
  FIRST_LINE=$(echo "$RESP_TEXT" | head -1)
  if [[ "$FIRST_LINE" == "[OpenObscure]"* ]]; then
    HAS_LABEL=true
    LABEL_LINE="$FIRST_LINE"
  fi

  # Save JSON metadata
  jq -n \
    --arg file "$FILENAME" \
    --arg path "$INPUT_FILE" \
    --arg architecture "gateway" \
    --arg timestamp "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    --argjson has_label "$HAS_LABEL" \
    --arg label "$LABEL_LINE" \
    '{
      file: $file,
      path: $path,
      architecture: $architecture,
      redaction_mode: "cognitive_firewall",
      timestamp: $timestamp,
      has_warning_label: $has_label,
      label_text: $label
    }' > "$JSON_DIR/${NAME_NO_EXT}_gateway.json"

  # Validation: persuasion files should have labels, clean should not
  case "$FILENAME" in
    clean_*)
      if $HAS_LABEL; then
        warn "$FILENAME" "unexpected [OpenObscure] label on clean content"
      else
        pass "$FILENAME" "no label (clean content)"
      fi
      ;;
    persuasion_*|mixed_*)
      if $HAS_LABEL; then
        pass "$FILENAME" "label: ${LABEL_LINE:0:70}..."
      else
        fail "$FILENAME" "missing [OpenObscure] label on persuasive content"
      fi
      ;;
    *)
      if $HAS_LABEL; then
        pass "$FILENAME" "label present"
      else
        warn "$FILENAME" "no label (unknown category)"
      fi
      ;;
  esac
done

# Write validation JSON
jq -n \
  --arg suite "cognitive_firewall" \
  --arg ts "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --argjson total "$TOTAL" \
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
  }' > "$OUTPUT_DIR/cognitive_firewall_validation.json"

echo ""
if [[ $FAIL -eq 0 ]]; then
  printf "\033[32mPASS\033[0m  Cognitive Firewall: %d/%d passed" "$PASS" "$TOTAL"
  [[ $WARN -gt 0 ]] && printf ", %d warnings" "$WARN"
  echo ""
else
  printf "\033[31mFAIL\033[0m  Cognitive Firewall: %d/%d passed, %d failed\n" "$PASS" "$TOTAL" "$FAIL"
fi

[[ $FAIL -gt 0 ]] && exit 1
exit 0
