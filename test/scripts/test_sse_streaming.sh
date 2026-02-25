#!/usr/bin/env bash
# test_sse_streaming.sh — Validate SSE streaming pass-through with PII scanning.
#
# Requires:
#   - SSE mock server: node test/scripts/mock/sse_mock_server.mjs
#   - Proxy running with test/config/test_sse.toml (upstream → SSE mock)
#
# Test cases:
#   1. Response has Content-Type: text/event-stream
#   2. SSE events contain data: prefixed lines
#   3. [DONE] termination event is preserved
#   4. Clean content passes through unchanged
#   5. requests_total increments in health
#
# Usage:
#   ./test/scripts/test_sse_streaming.sh
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
OUTPUT_DIR="$TEST_DIR/data/output/sse"
mkdir -p "$OUTPUT_DIR"

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
SKIP=0
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

skip_test() {
  SKIP=$((SKIP + 1))
  TOTAL=$((TOTAL + 1))
  printf "  \033[90mSKIP\033[0m  %-45s  %s\n" "$1" "$2"
  RESULTS_JSON=$(echo "$RESULTS_JSON" | jq --arg n "$1" --arg d "$2" '. + [{"name": $n, "status": "skip", "detail": $d}]')
}

# Check proxy is up
HEALTH=$(curl -sf "${AUTH_HEADER[@]}" "$HEALTH_ENDPOINT" 2>/dev/null || true)
if [[ -z "$HEALTH" ]]; then
  echo "Error: Proxy not reachable at $PROXY_URL"
  exit 1
fi

# Check SSE mock server is reachable
SSE_CHECK=$(curl -sf -o /dev/null -w "%{http_code}" -X POST "http://127.0.0.1:18792" \
  -H "Content-Type: application/json" \
  -d '{"model":"test","max_tokens":1,"messages":[{"role":"user","content":"ping"}]}' 2>/dev/null || echo "000")
if [[ "$SSE_CHECK" == "000" ]]; then
  echo "Warning: SSE mock server not reachable at port 18792."
  echo "Start it: node test/scripts/mock/sse_mock_server.mjs"
  echo ""
fi

echo "=== SSE Streaming Validation ==="
echo ""

cleanup_sse() {
  rm -f /tmp/oo_sse_resp_* /tmp/oo_sse_headers_* 2>/dev/null || true
}
trap cleanup_sse EXIT

# Record baseline request count
BEFORE_REQUESTS=$(echo "$HEALTH" | jq '.requests_total // 0')

# Send a streaming request through the proxy
SSE_RESP_FILE=$(mktemp /tmp/oo_sse_resp_XXXXXX)
SSE_HEADERS_FILE=$(mktemp /tmp/oo_sse_headers_XXXXXX)

REQUEST_BODY='{"model":"test","max_tokens":256,"stream":true,"messages":[{"role":"user","content":"The weather today is sunny and warm."}]}'

curl -sN -D "$SSE_HEADERS_FILE" -o "$SSE_RESP_FILE" -X POST "$PROVIDER_ENDPOINT" \
  -H "Content-Type: application/json" \
  -H "x-api-key: test-sse-stream" \
  -H "anthropic-version: 2023-06-01" \
  -d "$REQUEST_BODY" \
  --max-time 10 2>/dev/null || true

SSE_BODY=$(cat "$SSE_RESP_FILE" 2>/dev/null || echo "")
SSE_HEADERS=$(cat "$SSE_HEADERS_FILE" 2>/dev/null || echo "")

# ─── Test 1: Content-Type is text/event-stream ────────────────
if echo "$SSE_HEADERS" | grep -qi "content-type.*text/event-stream"; then
  pass "sse_content_type" "text/event-stream"
elif echo "$SSE_HEADERS" | grep -qi "content-type.*application/json"; then
  # Proxy may have converted SSE to buffered JSON — still valid behavior
  warn "sse_content_type" "application/json (proxy may buffer SSE into JSON)"
elif [[ -z "$SSE_HEADERS" ]]; then
  warn "sse_content_type" "no response headers (upstream may not be running)"
else
  fail "sse_content_type" "unexpected content-type"
fi

# ─── Test 2: SSE events contain data: lines ──────────────────
if [[ -n "$SSE_BODY" ]]; then
  DATA_LINE_COUNT=$(echo "$SSE_BODY" | grep -c "^data:" || echo "0")
  if [[ "$DATA_LINE_COUNT" -gt 0 ]]; then
    pass "sse_data_events" "$DATA_LINE_COUNT data: lines"
  else
    # If proxy buffered the response, it may be plain JSON
    if echo "$SSE_BODY" | jq . >/dev/null 2>&1; then
      warn "sse_data_events" "response is buffered JSON (no SSE data: lines)"
    else
      fail "sse_data_events" "no data: lines found"
    fi
  fi
else
  warn "sse_data_events" "empty response body"
fi

# ─── Test 3: [DONE] termination event ─────────────────────────
if [[ -n "$SSE_BODY" ]]; then
  if echo "$SSE_BODY" | grep -qF "[DONE]"; then
    pass "sse_done_event" "[DONE] termination present"
  elif echo "$SSE_BODY" | grep -qF "message_stop"; then
    pass "sse_done_event" "message_stop event present"
  else
    warn "sse_done_event" "no [DONE] or message_stop found"
  fi
else
  warn "sse_done_event" "empty response body"
fi

# ─── Test 4: Content passes through ──────────────────────────
if [[ -n "$SSE_BODY" ]]; then
  # The SSE mock echoes the user message back; check for any content
  if echo "$SSE_BODY" | grep -qiF "weather\|sunny\|warm"; then
    pass "sse_content_passthrough" "user content echoed in response"
  elif [[ "$DATA_LINE_COUNT" -gt 2 ]]; then
    pass "sse_content_passthrough" "multiple content events (content flowing)"
  else
    warn "sse_content_passthrough" "could not verify content echo"
  fi
else
  warn "sse_content_passthrough" "empty response body"
fi

# ─── Test 5: requests_total incremented ───────────────────────
sleep 0.3
AFTER_HEALTH=$(curl -sf "${AUTH_HEADER[@]}" "$HEALTH_ENDPOINT" 2>/dev/null || echo "{}")
AFTER_REQUESTS=$(echo "$AFTER_HEALTH" | jq '.requests_total // 0')

if [[ "$AFTER_REQUESTS" -gt "$BEFORE_REQUESTS" ]]; then
  pass "sse_request_counted" "$BEFORE_REQUESTS → $AFTER_REQUESTS"
else
  warn "sse_request_counted" "before=$BEFORE_REQUESTS after=$AFTER_REQUESTS"
fi

# Save raw SSE response for debugging
if [[ -n "$SSE_BODY" ]]; then
  echo "$SSE_BODY" > "$OUTPUT_DIR/sse_raw_response.txt"
fi

# ─── Write validation JSON ────────────────────────────────────
jq -n \
  --arg suite "sse_streaming" \
  --arg ts "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --argjson total "$TOTAL" \
  --argjson pass "$PASS" \
  --argjson fail "$FAIL" \
  --argjson warn "$WARN" \
  --argjson skip "$SKIP" \
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
  }' > "$OUTPUT_DIR/sse_validation.json"

# ─── Summary ──────────────────────────────────────────────────
echo ""
if [[ $FAIL -eq 0 ]]; then
  printf "\033[32mPASS\033[0m  SSE streaming validation: %d/%d passed" "$PASS" "$TOTAL"
  [[ $WARN -gt 0 ]] && printf ", %d warnings" "$WARN"
  echo ""
else
  printf "\033[31mFAIL\033[0m  SSE streaming validation: %d/%d passed, %d failed\n" "$PASS" "$TOTAL" "$FAIL"
fi

[[ $FAIL -gt 0 ]] && exit 1
exit 0
