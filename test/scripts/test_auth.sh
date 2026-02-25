#!/usr/bin/env bash
# test_auth.sh — Validate auth token enforcement on proxy endpoints.
#
# Test cases:
#   1-3. Health endpoint: valid token → 200, no token → 401, wrong token → 401
#   4-6. NER endpoint: valid token → 200, no token → 401, wrong token → 401
#   7.   Provider route: no auth gate → non-401
#
# Usage:
#   ./test/scripts/test_auth.sh
#
# Environment:
#   PROXY_URL   — Proxy base URL (default: http://127.0.0.1:18790)
#   AUTH_TOKEN   — Proxy auth token (default: read from ~/.openobscure/.auth-token)

set -euo pipefail

PROXY_URL="${PROXY_URL:-http://127.0.0.1:18790}"
HEALTH_ENDPOINT="${PROXY_URL}/_openobscure/health"
NER_ENDPOINT="${PROXY_URL}/_openobscure/ner"
PROVIDER_ENDPOINT="${PROXY_URL}/anthropic/v1/messages"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
TEST_DIR="$(dirname "$SCRIPT_DIR")"
OUTPUT_DIR="$TEST_DIR/data/output/auth"
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

# If no auth token is configured, we can't test auth enforcement
if [[ -z "$AUTH_TOKEN" ]]; then
  echo "SKIP: No auth token configured. Auth tests require a token in AUTH_TOKEN env or ~/.openobscure/.auth-token"
  exit 0
fi

WRONG_TOKEN="wrong-token-$(date +%s)"

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

skip_test() {
  SKIP=$((SKIP + 1))
  TOTAL=$((TOTAL + 1))
  printf "  \033[90mSKIP\033[0m  %-45s  %s\n" "$1" "$2"
  RESULTS_JSON=$(echo "$RESULTS_JSON" | jq --arg n "$1" --arg d "$2" '. + [{"name": $n, "status": "skip", "detail": $d}]')
}

# Check proxy is up first
UP_CHECK=$(curl -sf -o /dev/null -w "%{http_code}" -H "X-OpenObscure-Token: $AUTH_TOKEN" "$HEALTH_ENDPOINT" 2>/dev/null || echo "000")
if [[ "$UP_CHECK" == "000" ]]; then
  echo "Error: Proxy not reachable at $PROXY_URL"
  exit 1
fi

echo "=== Auth Token Enforcement Tests ==="
echo ""

# ─── Health endpoint ──────────────────────────────────────────
echo "--- Health Endpoint ---"

# 1. Valid token → 200
CODE=$(curl -s -o /dev/null -w "%{http_code}" -H "X-OpenObscure-Token: $AUTH_TOKEN" "$HEALTH_ENDPOINT" 2>/dev/null)
if [[ "$CODE" == "200" ]]; then
  pass "health_valid_token" "HTTP $CODE"
else
  fail "health_valid_token" "expected 200, got HTTP $CODE"
fi

# 2. No token → 401
CODE=$(curl -s -o /dev/null -w "%{http_code}" "$HEALTH_ENDPOINT" 2>/dev/null)
if [[ "$CODE" == "401" ]]; then
  pass "health_no_token" "HTTP $CODE"
else
  fail "health_no_token" "expected 401, got HTTP $CODE"
fi

# 3. Wrong token → 401
CODE=$(curl -s -o /dev/null -w "%{http_code}" -H "X-OpenObscure-Token: $WRONG_TOKEN" "$HEALTH_ENDPOINT" 2>/dev/null)
if [[ "$CODE" == "401" ]]; then
  pass "health_wrong_token" "HTTP $CODE"
else
  fail "health_wrong_token" "expected 401, got HTTP $CODE"
fi

# ─── NER endpoint ─────────────────────────────────────────────
echo ""
echo "--- NER Endpoint ---"

NER_BODY='{"text":"test auth check 123-45-6789"}'

# 4. Valid token → 200
CODE=$(curl -s -o /dev/null -w "%{http_code}" -X POST "$NER_ENDPOINT" \
  -H "Content-Type: application/json" \
  -H "X-OpenObscure-Token: $AUTH_TOKEN" \
  -d "$NER_BODY" 2>/dev/null)
if [[ "$CODE" == "200" ]]; then
  pass "ner_valid_token" "HTTP $CODE"
else
  fail "ner_valid_token" "expected 200, got HTTP $CODE"
fi

# 5. No token → 401
CODE=$(curl -s -o /dev/null -w "%{http_code}" -X POST "$NER_ENDPOINT" \
  -H "Content-Type: application/json" \
  -d "$NER_BODY" 2>/dev/null)
if [[ "$CODE" == "401" ]]; then
  pass "ner_no_token" "HTTP $CODE"
else
  fail "ner_no_token" "expected 401, got HTTP $CODE"
fi

# 6. Wrong token → 401
CODE=$(curl -s -o /dev/null -w "%{http_code}" -X POST "$NER_ENDPOINT" \
  -H "Content-Type: application/json" \
  -H "X-OpenObscure-Token: $WRONG_TOKEN" \
  -d "$NER_BODY" 2>/dev/null)
if [[ "$CODE" == "401" ]]; then
  pass "ner_wrong_token" "HTTP $CODE"
else
  fail "ner_wrong_token" "expected 401, got HTTP $CODE"
fi

# ─── Provider route (pass-through, no auth gate) ─────────────
echo ""
echo "--- Provider Route ---"

# 7. Provider route should not reject based on OpenObscure auth
# (it uses the upstream provider's own auth — x-api-key)
PROVIDER_BODY='{"model":"test","max_tokens":1,"messages":[{"role":"user","content":"hello"}]}'
CODE=$(curl -s -o /dev/null -w "%{http_code}" -X POST "$PROVIDER_ENDPOINT" \
  -H "Content-Type: application/json" \
  -H "x-api-key: test-auth-check" \
  -H "anthropic-version: 2023-06-01" \
  -d "$PROVIDER_BODY" 2>/dev/null)
if [[ "$CODE" != "401" ]]; then
  pass "provider_no_auth_gate" "HTTP $CODE (not 401)"
else
  fail "provider_no_auth_gate" "got 401 — proxy should not auth-gate provider routes"
fi

# ─── Write validation JSON ────────────────────────────────────
jq -n \
  --arg suite "auth" \
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
  }' > "$OUTPUT_DIR/auth_validation.json"

# ─── Summary ──────────────────────────────────────────────────
echo ""
if [[ $FAIL -eq 0 ]]; then
  printf "\033[32mPASS\033[0m  Auth validation: %d/%d passed\n" "$PASS" "$TOTAL"
else
  printf "\033[31mFAIL\033[0m  Auth validation: %d/%d passed, %d failed\n" "$PASS" "$TOTAL" "$FAIL"
fi

[[ $FAIL -gt 0 ]] && exit 1
exit 0
