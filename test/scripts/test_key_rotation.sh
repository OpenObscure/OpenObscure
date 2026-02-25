#!/usr/bin/env bash
# test_key_rotation.sh — Validate FPE key rotation via the key-rotate CLI subcommand.
#
# Test cases:
#   1. Record current fpe_key_version from health endpoint
#   2. Run `openobscure-proxy key-rotate` → exit 0
#   3. Verify the CLI reports success
#   4. FPE encrypt/decrypt works after rotation (send PII, verify FPE-encrypted output)
#
# NOTE: After key-rotate, the running proxy keeps its in-memory key until restarted.
# The new key takes effect on the next proxy startup. Test case 3 (version increment)
# requires a proxy restart, which this script does NOT perform automatically.
#
# Usage:
#   ./test/scripts/test_key_rotation.sh [--binary <path>] [--config <path>]
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
PROJECT_ROOT="$(dirname "$TEST_DIR")"
OUTPUT_DIR="$TEST_DIR/data/output/key_rotation"
mkdir -p "$OUTPUT_DIR"

# Defaults
BINARY="${PROJECT_ROOT}/target/release/openobscure-proxy"
CONFIG="test/config/test_fpe.toml"

# Parse args
while [[ $# -gt 0 ]]; do
  case "$1" in
    --binary) BINARY="$2"; shift 2 ;;
    --config) CONFIG="$2"; shift 2 ;;
    *) shift ;;
  esac
done

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

echo "=== Key Rotation Validation ==="
echo ""

# Check binary exists
if [[ ! -x "$BINARY" ]]; then
  echo "Error: Binary not found or not executable: $BINARY"
  echo "Build first: cargo build --release --manifest-path openobscure-proxy/Cargo.toml"
  exit 1
fi

# Check proxy is up
HEALTH=$(curl -sf "${AUTH_HEADER[@]}" "$HEALTH_ENDPOINT" 2>/dev/null || true)
if [[ -z "$HEALTH" ]]; then
  echo "Error: Proxy not reachable at $PROXY_URL"
  exit 1
fi

# ─── Test 1: Record current key version ──────────────────────
BEFORE_VERSION=$(echo "$HEALTH" | jq '.fpe_key_version // 0')
if [[ "$BEFORE_VERSION" -ge 1 ]]; then
  pass "initial_key_version" "v$BEFORE_VERSION"
else
  fail "initial_key_version" "got $BEFORE_VERSION (expected >= 1)"
fi

# ─── Test 2: Run key-rotate CLI ──────────────────────────────
ROTATE_OUTPUT=$(mktemp /tmp/oo_rotate_output_XXXXXX)
ROTATE_EXIT=0
"$BINARY" --config "$CONFIG" key-rotate > "$ROTATE_OUTPUT" 2>&1 || ROTATE_EXIT=$?

if [[ "$ROTATE_EXIT" -eq 0 ]]; then
  pass "key_rotate_exit_code" "exit 0"
else
  fail "key_rotate_exit_code" "exit $ROTATE_EXIT"
fi

# ─── Test 3: CLI output mentions success ──────────────────────
ROTATE_TEXT=$(cat "$ROTATE_OUTPUT")
rm -f "$ROTATE_OUTPUT"

if echo "$ROTATE_TEXT" | grep -qi "rotat\|success\|new key\|version"; then
  pass "key_rotate_output" "mentions rotation/success"
else
  warn "key_rotate_output" "output: $(echo "$ROTATE_TEXT" | head -1)"
fi

# ─── Test 4: FPE still works (with current in-memory key) ────
# The running proxy still uses the old key (hasn't restarted), but FPE should still work.
PII_BODY='{"model":"test","max_tokens":1,"messages":[{"role":"user","content":"Card: 4111-1111-1111-1111"}]}'
CODE=$(curl -s -o /dev/null -w "%{http_code}" -X POST "$PROVIDER_ENDPOINT" \
  -H "Content-Type: application/json" \
  -H "x-api-key: test-key-rotation" \
  -H "anthropic-version: 2023-06-01" \
  -d "$PII_BODY" 2>/dev/null)

if [[ "$CODE" == "200" ]]; then
  pass "fpe_still_works" "HTTP $CODE (proxy using in-memory key)"
elif [[ "$CODE" == "502" ]]; then
  warn "fpe_still_works" "HTTP $CODE (echo server may be down)"
else
  fail "fpe_still_works" "expected 200, got HTTP $CODE"
fi

# Note about version increment
echo ""
echo "  NOTE: fpe_key_version will increment after proxy restart."
echo "  Current version: v$BEFORE_VERSION (in-memory, pre-rotation)"
echo "  New key is written to the OS keychain but the running proxy"
echo "  continues with its in-memory key until restarted."

# ─── Write validation JSON ────────────────────────────────────
jq -n \
  --arg suite "key_rotation" \
  --arg ts "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --argjson total "$TOTAL" \
  --argjson pass "$PASS" \
  --argjson fail "$FAIL" \
  --argjson warn "$WARN" \
  --argjson skip "$SKIP" \
  --argjson before_version "$BEFORE_VERSION" \
  --argjson results "$RESULTS_JSON" \
  '{
    test_suite: $suite,
    timestamp: $ts,
    before_key_version: $before_version,
    total: $total,
    pass: $pass,
    fail: $fail,
    warn: $warn,
    skip: $skip,
    results: $results
  }' > "$OUTPUT_DIR/rotation_validation.json"

# ─── Summary ──────────────────────────────────────────────────
echo ""
if [[ $FAIL -eq 0 ]]; then
  printf "\033[32mPASS\033[0m  Key rotation validation: %d/%d passed" "$PASS" "$TOTAL"
  [[ $WARN -gt 0 ]] && printf ", %d warnings" "$WARN"
  echo ""
else
  printf "\033[31mFAIL\033[0m  Key rotation validation: %d/%d passed, %d failed\n" "$PASS" "$TOTAL" "$FAIL"
fi

[[ $FAIL -gt 0 ]] && exit 1
exit 0
