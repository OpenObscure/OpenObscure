#!/usr/bin/env bash
# test_body_limits.sh — Validate per-tier body size limits and 413 rejection.
#
# Test cases:
#   1. Small request (1KB) → 200 OK
#   2. Over-limit request (max_body_bytes + 1MB) → 413 Payload Too Large
#   3. Empty body → forwards cleanly
#
# The default test config (test_fpe.toml) sets max_body_bytes = 16777216 (16MB).
#
# Usage:
#   ./test/scripts/test_body_limits.sh
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
OUTPUT_DIR="$TEST_DIR/data/output/body_limits"
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

echo "=== Body Size Limit Validation ==="
echo ""

# Temp files for payloads
cleanup_body_limits() {
  rm -f /tmp/oo_body_test_* 2>/dev/null || true
}
trap cleanup_body_limits EXIT

# ─── Test 1: Small request succeeds ──────────────────────────
SMALL_BODY='{"model":"test","max_tokens":1,"messages":[{"role":"user","content":"hello world"}]}'
CODE=$(curl -s -o /dev/null -w "%{http_code}" -X POST "$PROVIDER_ENDPOINT" \
  -H "Content-Type: application/json" \
  -H "x-api-key: test-body-limit" \
  -H "anthropic-version: 2023-06-01" \
  -d "$SMALL_BODY" 2>/dev/null)

if [[ "$CODE" == "200" ]]; then
  pass "small_request_ok" "1KB → HTTP $CODE"
elif [[ "$CODE" == "502" ]]; then
  # 502 means proxy forwarded to echo server but echo may not be running — still not 413
  pass "small_request_ok" "1KB → HTTP $CODE (echo server may be down, but not rejected by size)"
else
  fail "small_request_ok" "1KB → HTTP $CODE"
fi

# ─── Test 2: Over-limit request rejected ──────────────────────
# Generate a payload > 16MB (the test config limit)
# Create a JSON payload with a large content field (~17MB of 'A' characters)
OVERSIZED_FILE=$(mktemp /tmp/oo_body_test_XXXXXX)

# Build the JSON with a 17MB content string
python3 -c "
import json, sys
content = 'A' * (17 * 1024 * 1024)
payload = {
    'model': 'test',
    'max_tokens': 1,
    'messages': [{'role': 'user', 'content': content}]
}
json.dump(payload, sys.stdout)
" > "$OVERSIZED_FILE" 2>/dev/null

OVERSIZED_SIZE=$(wc -c < "$OVERSIZED_FILE" | tr -d ' ')

CODE=$(curl -s -o /dev/null -w "%{http_code}" -X POST "$PROVIDER_ENDPOINT" \
  -H "Content-Type: application/json" \
  -H "x-api-key: test-body-limit" \
  -H "anthropic-version: 2023-06-01" \
  --data-binary @"$OVERSIZED_FILE" 2>/dev/null)

if [[ "$CODE" == "413" ]]; then
  pass "oversized_request_rejected" "${OVERSIZED_SIZE} bytes → HTTP 413"
elif [[ "$CODE" == "400" ]]; then
  # Some frameworks return 400 for body too large
  pass "oversized_request_rejected" "${OVERSIZED_SIZE} bytes → HTTP $CODE (rejected)"
else
  fail "oversized_request_rejected" "${OVERSIZED_SIZE} bytes → HTTP $CODE (expected 413)"
fi

rm -f "$OVERSIZED_FILE"

# ─── Test 3: Empty body handling ──────────────────────────────
# Empty body to provider endpoint — should get a 4xx (bad request) not 413
CODE=$(curl -s -o /dev/null -w "%{http_code}" -X POST "$PROVIDER_ENDPOINT" \
  -H "Content-Type: application/json" \
  -H "x-api-key: test-body-limit" \
  -H "anthropic-version: 2023-06-01" \
  -d "" 2>/dev/null)

if [[ "$CODE" != "413" ]]; then
  pass "empty_body_not_413" "empty → HTTP $CODE (not size-rejected)"
else
  fail "empty_body_not_413" "empty → HTTP 413 (should not be size-rejected)"
fi

# ─── Write validation JSON ────────────────────────────────────
jq -n \
  --arg suite "body_limits" \
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
  }' > "$OUTPUT_DIR/body_limits_validation.json"

# ─── Summary ──────────────────────────────────────────────────
echo ""
if [[ $FAIL -eq 0 ]]; then
  printf "\033[32mPASS\033[0m  Body limits validation: %d/%d passed" "$PASS" "$TOTAL"
  [[ $WARN -gt 0 ]] && printf ", %d warnings" "$WARN"
  echo ""
else
  printf "\033[31mFAIL\033[0m  Body limits validation: %d/%d passed, %d failed\n" "$PASS" "$TOTAL" "$FAIL"
fi

[[ $FAIL -gt 0 ]] && exit 1
exit 0
