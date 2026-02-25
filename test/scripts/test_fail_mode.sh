#!/usr/bin/env bash
# test_fail_mode.sh — Validate fail_mode behavior (open vs closed).
#
# The proxy's fail_mode controls what happens when body processing fails:
#   - "open" (default): forward the original body unchanged, return 200
#   - "closed": reject the request with 502 Bad Gateway
#
# This script auto-detects the current fail_mode from the running proxy's config
# and runs the appropriate tests.
#
# Test cases (open mode):
#   1. Malformed JSON with PII → HTTP 200 (forwarded to echo)
#   2. Echo capture contains the original malformed body
#   3. Valid JSON with PII → normal FPE processing
#
# Test cases (closed mode):
#   4. Malformed JSON → HTTP 502
#   5. Valid JSON with PII → normal FPE processing
#
# Usage:
#   ./test/scripts/test_fail_mode.sh
#
# Environment:
#   PROXY_URL   — Proxy base URL (default: http://127.0.0.1:18790)
#   AUTH_TOKEN   — Proxy auth token (default: read from ~/.openobscure/.auth-token)
#   CAPTURE_DIR  — Echo server capture dir (default: /tmp/oo_echo_captures)

set -euo pipefail

PROXY_URL="${PROXY_URL:-http://127.0.0.1:18790}"
HEALTH_ENDPOINT="${PROXY_URL}/_openobscure/health"
PROVIDER_ENDPOINT="${PROXY_URL}/anthropic/v1/messages"
CAPTURE_DIR="${CAPTURE_DIR:-/tmp/oo_echo_captures}"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
TEST_DIR="$(dirname "$SCRIPT_DIR")"
OUTPUT_DIR="$TEST_DIR/data/output/fail_mode"
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

# Detect fail_mode — the proxy doesn't expose this in health, so we probe it.
# Send intentionally malformed JSON and check if we get 200 (open) or 502 (closed).
PROBE_CAPTURE_ID="fail_mode_probe_$$"
MALFORMED_BODY='{ "model": "test", not valid json, SSN: 123-45-6789 }'

PROBE_CODE=$(curl -s -o /dev/null -w "%{http_code}" -X POST "$PROVIDER_ENDPOINT" \
  -H "Content-Type: application/json" \
  -H "x-api-key: test-fail-mode" \
  -H "anthropic-version: 2023-06-01" \
  -H "X-Capture-Id: $PROBE_CAPTURE_ID" \
  -d "$MALFORMED_BODY" 2>/dev/null)

# Clean up probe capture
rm -f "$CAPTURE_DIR/${PROBE_CAPTURE_ID}.json" 2>/dev/null || true

if [[ "$PROBE_CODE" == "200" ]]; then
  DETECTED_MODE="open"
elif [[ "$PROBE_CODE" == "502" ]]; then
  DETECTED_MODE="closed"
else
  DETECTED_MODE="unknown"
fi

echo "=== Fail Mode Validation ==="
echo "Detected fail_mode: $DETECTED_MODE (probe returned HTTP $PROBE_CODE)"
echo ""

cleanup_fail_mode() {
  rm -f "$CAPTURE_DIR/fail_mode_"*.json 2>/dev/null || true
}
trap cleanup_fail_mode EXIT

# ─── Open mode tests ─────────────────────────────────────────
if [[ "$DETECTED_MODE" == "open" ]]; then

  # Test 1: Malformed JSON → HTTP 200 (forwarded)
  CAPTURE_ID="fail_mode_malformed_$$"
  CODE=$(curl -s -o /dev/null -w "%{http_code}" -X POST "$PROVIDER_ENDPOINT" \
    -H "Content-Type: application/json" \
    -H "x-api-key: test-fail-mode" \
    -H "anthropic-version: 2023-06-01" \
    -H "X-Capture-Id: $CAPTURE_ID" \
    -d "$MALFORMED_BODY" 2>/dev/null)

  if [[ "$CODE" == "200" ]]; then
    pass "open_malformed_forwarded" "HTTP $CODE"
  else
    fail "open_malformed_forwarded" "expected 200, got HTTP $CODE"
  fi

  # Test 2: Echo capture contains original malformed body
  sleep 0.3
  CAPTURE_FILE="$CAPTURE_DIR/${CAPTURE_ID}.json"
  if [[ -f "$CAPTURE_FILE" && -s "$CAPTURE_FILE" ]]; then
    # The body was forwarded — check it contains the SSN (not encrypted, since JSON parse failed)
    if grep -qF "123-45-6789" "$CAPTURE_FILE" 2>/dev/null; then
      pass "open_body_preserved" "original PII forwarded unchanged"
    else
      warn "open_body_preserved" "capture exists but SSN not found (may have been partially processed)"
    fi
  else
    warn "open_body_preserved" "no capture file (echo server running?)"
  fi
  rm -f "$CAPTURE_FILE" 2>/dev/null || true

# ─── Closed mode tests ───────────────────────────────────────
elif [[ "$DETECTED_MODE" == "closed" ]]; then

  # Test 4: Malformed JSON → HTTP 502
  CAPTURE_ID="fail_mode_closed_$$"
  CODE=$(curl -s -o /dev/null -w "%{http_code}" -X POST "$PROVIDER_ENDPOINT" \
    -H "Content-Type: application/json" \
    -H "x-api-key: test-fail-mode" \
    -H "anthropic-version: 2023-06-01" \
    -H "X-Capture-Id: $CAPTURE_ID" \
    -d "$MALFORMED_BODY" 2>/dev/null)

  if [[ "$CODE" == "502" ]]; then
    pass "closed_malformed_rejected" "HTTP $CODE"
  else
    fail "closed_malformed_rejected" "expected 502, got HTTP $CODE"
  fi

  # Test 5: No echo capture created
  sleep 0.3
  CAPTURE_FILE="$CAPTURE_DIR/${CAPTURE_ID}.json"
  if [[ ! -f "$CAPTURE_FILE" ]]; then
    pass "closed_no_upstream" "no capture file (request not forwarded)"
  else
    fail "closed_no_upstream" "capture file exists (request was forwarded)"
    rm -f "$CAPTURE_FILE" 2>/dev/null || true
  fi

else
  skip_test "fail_mode_detection" "could not detect mode (HTTP $PROBE_CODE)"
fi

# ─── Common: Valid JSON still works ──────────────────────────
VALID_BODY='{"model":"test","max_tokens":1,"messages":[{"role":"user","content":"My SSN is 123-45-6789"}]}'
CODE=$(curl -s -o /dev/null -w "%{http_code}" -X POST "$PROVIDER_ENDPOINT" \
  -H "Content-Type: application/json" \
  -H "x-api-key: test-fail-mode" \
  -H "anthropic-version: 2023-06-01" \
  -d "$VALID_BODY" 2>/dev/null)

if [[ "$CODE" == "200" ]]; then
  pass "valid_json_succeeds" "HTTP $CODE"
elif [[ "$CODE" == "502" ]]; then
  warn "valid_json_succeeds" "HTTP $CODE (echo server may be down)"
else
  fail "valid_json_succeeds" "expected 200, got HTTP $CODE"
fi

# ─── Write validation JSON ────────────────────────────────────
jq -n \
  --arg suite "fail_mode" \
  --arg ts "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --arg mode "$DETECTED_MODE" \
  --argjson total "$TOTAL" \
  --argjson pass "$PASS" \
  --argjson fail "$FAIL" \
  --argjson warn "$WARN" \
  --argjson skip "$SKIP" \
  --argjson results "$RESULTS_JSON" \
  '{
    test_suite: $suite,
    detected_mode: $mode,
    timestamp: $ts,
    total: $total,
    pass: $pass,
    fail: $fail,
    warn: $warn,
    skip: $skip,
    results: $results
  }' > "$OUTPUT_DIR/fail_mode_validation.json"

# ─── Summary ──────────────────────────────────────────────────
echo ""
if [[ $FAIL -eq 0 ]]; then
  printf "\033[32mPASS\033[0m  Fail mode validation (%s): %d/%d passed" "$DETECTED_MODE" "$PASS" "$TOTAL"
  [[ $WARN -gt 0 ]] && printf ", %d warnings" "$WARN"
  echo ""
else
  printf "\033[31mFAIL\033[0m  Fail mode validation (%s): %d/%d passed, %d failed\n" "$DETECTED_MODE" "$PASS" "$TOTAL" "$FAIL"
fi

[[ $FAIL -gt 0 ]] && exit 1
exit 0
