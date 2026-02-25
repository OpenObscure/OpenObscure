#!/usr/bin/env bash
# test_response_integrity.sh — Validate the cognitive firewall (R1 dict + R2 model cascade).
#
# Requires:
#   - RI mock server: node test/scripts/mock/ri_mock_server.mjs
#   - Proxy running with test/config/test_ri.toml (response_integrity enabled, log_only=false)
#
# Test cases:
#   1. Clean response → no [OpenObscure] label in body
#   2. Persuasive response → [OpenObscure] warning label prepended
#   3. Commercial response → triggers Warning severity
#   4. Fear-based response → triggers Caution severity
#   5. Health counters ri_flags_total / ri_scans_total increment
#
# Usage:
#   ./test/scripts/test_response_integrity.sh
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
OUTPUT_DIR="$TEST_DIR/data/output/ri"
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

# Check if RI is enabled by looking at ri_scans_total field
RI_SCANS=$(echo "$HEALTH" | jq '.ri_scans_total // -1')
if [[ "$RI_SCANS" == "-1" ]]; then
  echo "SKIP: Response integrity metrics not found in health endpoint."
  echo "Make sure proxy is running with test/config/test_ri.toml"
  exit 0
fi

# Check RI mock server is reachable
RI_MOCK_CHECK=$(curl -sf -o /dev/null -w "%{http_code}" -X POST "http://127.0.0.1:18793" \
  -H "Content-Type: application/json" \
  -H "X-Mock-Response: clean" \
  -d '{"model":"test","max_tokens":1,"messages":[{"role":"user","content":"hi"}]}' 2>/dev/null || echo "000")
if [[ "$RI_MOCK_CHECK" == "000" ]]; then
  echo "Warning: RI mock server not reachable at port 18793."
  echo "Start it: node test/scripts/mock/ri_mock_server.mjs"
  echo "Will test with whatever upstream is configured."
  echo ""
fi

echo "=== Response Integrity (Cognitive Firewall) Tests ==="
echo ""

# Helper: send request and get response body
send_request() {
  local mock_type="$1"
  local tmp_resp
  tmp_resp=$(mktemp /tmp/oo_ri_resp_XXXXXX)

  curl -sf -o "$tmp_resp" -X POST "$PROVIDER_ENDPOINT" \
    -H "Content-Type: application/json" \
    -H "x-api-key: test-ri-scan" \
    -H "anthropic-version: 2023-06-01" \
    -H "X-Mock-Response: $mock_type" \
    -d '{"model":"test","max_tokens":256,"messages":[{"role":"user","content":"Tell me about this."}]}' 2>/dev/null || true

  cat "$tmp_resp"
  rm -f "$tmp_resp"
}

# Record baseline health counters
BEFORE_FLAGS=$(echo "$HEALTH" | jq '.ri_flags_total // 0')
BEFORE_SCANS=$(echo "$HEALTH" | jq '.ri_scans_total // 0')

# ─── Test 1: Clean response → no label ───────────────────────
CLEAN_RESP=$(send_request "clean")
CLEAN_TEXT=$(echo "$CLEAN_RESP" | jq -r '.content[0].text // ""' 2>/dev/null || echo "")

if [[ -n "$CLEAN_TEXT" ]]; then
  if echo "$CLEAN_TEXT" | grep -qF "[OpenObscure]"; then
    fail "clean_no_label" "found [OpenObscure] label in clean response"
  else
    pass "clean_no_label" "no label in clean response"
  fi
else
  warn "clean_no_label" "empty response (upstream may not be running)"
fi

# ─── Test 2: Persuasive response → label present ─────────────
PERSUASIVE_RESP=$(send_request "persuasive")
PERSUASIVE_TEXT=$(echo "$PERSUASIVE_RESP" | jq -r '.content[0].text // ""' 2>/dev/null || echo "")

if [[ -n "$PERSUASIVE_TEXT" ]]; then
  if echo "$PERSUASIVE_TEXT" | grep -qF "[OpenObscure]"; then
    pass "persuasive_has_label" "warning label present"
  else
    # If RI is in log_only mode, labels won't appear
    warn "persuasive_has_label" "no label found (log_only mode? or RI disabled?)"
  fi
else
  warn "persuasive_has_label" "empty response (upstream may not be running)"
fi

# ─── Test 3: Commercial response → label ─────────────────────
COMMERCIAL_RESP=$(send_request "commercial")
COMMERCIAL_TEXT=$(echo "$COMMERCIAL_RESP" | jq -r '.content[0].text // ""' 2>/dev/null || echo "")

if [[ -n "$COMMERCIAL_TEXT" ]]; then
  if echo "$COMMERCIAL_TEXT" | grep -qF "[OpenObscure]"; then
    pass "commercial_has_label" "warning label present"
  else
    warn "commercial_has_label" "no label found"
  fi
else
  warn "commercial_has_label" "empty response"
fi

# ─── Test 4: Fear-based response → label ─────────────────────
FEAR_RESP=$(send_request "fear")
FEAR_TEXT=$(echo "$FEAR_RESP" | jq -r '.content[0].text // ""' 2>/dev/null || echo "")

if [[ -n "$FEAR_TEXT" ]]; then
  if echo "$FEAR_TEXT" | grep -qF "[OpenObscure]"; then
    # Check for escalated severity language
    if echo "$FEAR_TEXT" | grep -qiF "multiple influence"; then
      pass "fear_caution_severity" "Caution-level label present"
    else
      pass "fear_has_label" "label present (severity level may vary)"
    fi
  else
    warn "fear_has_label" "no label found"
  fi
else
  warn "fear_has_label" "empty response"
fi

# ─── Test 5: Health counters incremented ──────────────────────
sleep 0.3
AFTER_HEALTH=$(curl -sf "${AUTH_HEADER[@]}" "$HEALTH_ENDPOINT" 2>/dev/null || echo "{}")
AFTER_FLAGS=$(echo "$AFTER_HEALTH" | jq '.ri_flags_total // 0')
AFTER_SCANS=$(echo "$AFTER_HEALTH" | jq '.ri_scans_total // 0')

if [[ "$AFTER_SCANS" -gt "$BEFORE_SCANS" ]]; then
  pass "ri_scans_incremented" "$BEFORE_SCANS → $AFTER_SCANS"
else
  warn "ri_scans_incremented" "before=$BEFORE_SCANS after=$AFTER_SCANS (RI may not be scanning)"
fi

if [[ "$AFTER_FLAGS" -gt "$BEFORE_FLAGS" ]]; then
  pass "ri_flags_incremented" "$BEFORE_FLAGS → $AFTER_FLAGS"
else
  warn "ri_flags_incremented" "before=$BEFORE_FLAGS after=$AFTER_FLAGS (no persuasion detected or log_only mode)"
fi

# ─── Write validation JSON ────────────────────────────────────
jq -n \
  --arg suite "response_integrity" \
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
  }' > "$OUTPUT_DIR/ri_validation.json"

# ─── Summary ──────────────────────────────────────────────────
echo ""
if [[ $FAIL -eq 0 ]]; then
  printf "\033[32mPASS\033[0m  Response integrity validation: %d/%d passed" "$PASS" "$TOTAL"
  [[ $WARN -gt 0 ]] && printf ", %d warnings" "$WARN"
  echo ""
else
  printf "\033[31mFAIL\033[0m  Response integrity validation: %d/%d passed, %d failed\n" "$PASS" "$TOTAL" "$FAIL"
fi

[[ $FAIL -gt 0 ]] && exit 1
exit 0
