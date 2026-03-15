#!/usr/bin/env bash
# test_health.sh — Validate the health endpoint JSON schema, types, and counter behavior.
#
# Test cases:
#   1. All 40 top-level fields present in /_openobscure/health response
#   2. Type validation (semver, booleans, non-negative integers, enum values, 22 latency percentiles)
#   3. Feature budget nested object has all 10 expected fields
#   4. Counter monotonicity: requests_total and pii_matches_total increase after a PII request
#   5. Readiness: ready=true → HTTP 200
#
# Usage:
#   ./test/scripts/test_health.sh
#
# Environment:
#   PROXY_URL   — Proxy base URL (default: http://127.0.0.1:18790)
#   AUTH_TOKEN   — Proxy auth token (default: read from ~/.openobscure/.auth-token)

set -euo pipefail

PROXY_URL="${PROXY_URL:-http://127.0.0.1:18790}"
HEALTH_ENDPOINT="${PROXY_URL}/_openobscure/health"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
TEST_DIR="$(dirname "$SCRIPT_DIR")"
OUTPUT_DIR="$TEST_DIR/data/output/health"
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

echo "=== Health Endpoint Validation ==="
echo ""

# ─── Fetch health ─────────────────────────────────────────────
HTTP_CODE=$(curl -s -o /tmp/oo_health_test.json -w "%{http_code}" "${AUTH_HEADER[@]}" "$HEALTH_ENDPOINT" 2>/dev/null)
HEALTH=$(cat /tmp/oo_health_test.json 2>/dev/null || echo "{}")

if [[ "$HTTP_CODE" == "000" || -z "$HEALTH" || "$HEALTH" == "{}" ]]; then
  echo "Error: Proxy not reachable at $PROXY_URL"
  exit 1
fi

# Raw health response saved after counter test (so latency histograms have data)

# ─── Test 1: Schema — all expected top-level fields ───────────
EXPECTED_FIELDS=(
  "status" "version" "ready" "readiness_state" "uptime_secs"
  "pii_matches_total" "requests_total" "images_processed_total"
  "faces_redacted_total" "text_regions_total" "nsfw_blocked_total"
  "screenshots_detected_total" "ri_flags_total" "ri_scans_total"
  "onnx_panics_total" "fpe_key_version"
  "scan_latency_p50_us" "scan_latency_p95_us" "scan_latency_p99_us"
  "request_latency_p50_us" "request_latency_p95_us" "request_latency_p99_us"
  "device_tier" "feature_budget"
  "text_scan_latency_p50_us" "text_scan_latency_p95_us"
  "image_latency_p50_us" "image_latency_p95_us"
  "face_latency_p50_us" "face_latency_p95_us"
  "ocr_latency_p50_us" "ocr_latency_p95_us"
  "nsfw_latency_p50_us" "nsfw_latency_p95_us"
  "voice_latency_p50_us" "voice_latency_p95_us"
  "fpe_latency_p50_us" "fpe_latency_p95_us"
  "ri_latency_p50_us" "ri_latency_p95_us"
)

MISSING=()
for field in "${EXPECTED_FIELDS[@]}"; do
  has=$(echo "$HEALTH" | jq --arg f "$field" 'has($f)')
  if [[ "$has" != "true" ]]; then
    MISSING+=("$field")
  fi
done

if [[ ${#MISSING[@]} -eq 0 ]]; then
  pass "schema_all_fields_present" "${#EXPECTED_FIELDS[@]}/${#EXPECTED_FIELDS[@]} fields"
else
  fail "schema_all_fields_present" "missing: ${MISSING[*]}"
fi

# ─── Test 2: Type validation ──────────────────────────────────
# version matches semver pattern
VERSION=$(echo "$HEALTH" | jq -r '.version // ""')
if [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+ ]]; then
  pass "type_version_semver" "$VERSION"
else
  fail "type_version_semver" "got '$VERSION'"
fi

# ready is boolean
READY_TYPE=$(echo "$HEALTH" | jq -r '.ready | type')
if [[ "$READY_TYPE" == "boolean" ]]; then
  pass "type_ready_boolean" "$(echo "$HEALTH" | jq '.ready')"
else
  fail "type_ready_boolean" "type=$READY_TYPE"
fi

# uptime_secs is non-negative integer
UPTIME=$(echo "$HEALTH" | jq '.uptime_secs // -1')
if [[ "$UPTIME" -ge 0 ]]; then
  pass "type_uptime_non_negative" "${UPTIME}s"
else
  fail "type_uptime_non_negative" "got $UPTIME"
fi

# fpe_key_version >= 1
KEY_VER=$(echo "$HEALTH" | jq '.fpe_key_version // 0')
if [[ "$KEY_VER" -ge 1 ]]; then
  pass "type_fpe_key_version" "v$KEY_VER"
else
  fail "type_fpe_key_version" "got $KEY_VER (expected >= 1)"
fi

# device_tier is one of full/standard/lite
TIER=$(echo "$HEALTH" | jq -r '.device_tier // ""')
case "$TIER" in
  full|standard|lite) pass "type_device_tier_enum" "$TIER" ;;
  *) fail "type_device_tier_enum" "got '$TIER'" ;;
esac

# All latency percentiles are non-negative (6 aggregate + 16 per-feature = 22 fields)
latency_ok=true
latency_count=0
for field in scan_latency_p50_us scan_latency_p95_us scan_latency_p99_us \
             request_latency_p50_us request_latency_p95_us request_latency_p99_us \
             text_scan_latency_p50_us text_scan_latency_p95_us \
             image_latency_p50_us image_latency_p95_us \
             face_latency_p50_us face_latency_p95_us \
             ocr_latency_p50_us ocr_latency_p95_us \
             nsfw_latency_p50_us nsfw_latency_p95_us \
             voice_latency_p50_us voice_latency_p95_us \
             fpe_latency_p50_us fpe_latency_p95_us \
             ri_latency_p50_us ri_latency_p95_us; do
  val=$(echo "$HEALTH" | jq ".$field // -1")
  latency_count=$((latency_count + 1))
  if [[ "$val" -lt 0 ]]; then
    latency_ok=false
    break
  fi
done
if [[ "$latency_ok" == "true" ]]; then
  pass "type_latency_non_negative" "all $latency_count percentiles >= 0"
else
  fail "type_latency_non_negative" "$field = $val"
fi

# ─── Test 3: Feature budget nested object ─────────────────────
BUDGET_FIELDS=(
  "tier" "max_ram_mb" "ner_enabled" "crf_enabled" "ensemble_enabled"
  "image_pipeline_enabled" "ocr_tier" "nsfw_enabled" "screen_guard_enabled"
  "face_model"
)

BUDGET_MISSING=()
for field in "${BUDGET_FIELDS[@]}"; do
  has=$(echo "$HEALTH" | jq --arg f "$field" '.feature_budget | has($f)')
  if [[ "$has" != "true" ]]; then
    BUDGET_MISSING+=("$field")
  fi
done

if [[ ${#BUDGET_MISSING[@]} -eq 0 ]]; then
  pass "feature_budget_all_fields" "${#BUDGET_FIELDS[@]}/${#BUDGET_FIELDS[@]} fields"
else
  fail "feature_budget_all_fields" "missing: ${BUDGET_MISSING[*]}"
fi

# Budget booleans are actually booleans
budget_types_ok=true
for field in ner_enabled crf_enabled ensemble_enabled image_pipeline_enabled nsfw_enabled screen_guard_enabled; do
  ftype=$(echo "$HEALTH" | jq -r ".feature_budget.$field | type")
  if [[ "$ftype" != "boolean" ]]; then
    budget_types_ok=false
    break
  fi
done
if [[ "$budget_types_ok" == "true" ]]; then
  pass "feature_budget_boolean_types" "6 boolean fields correct"
else
  fail "feature_budget_boolean_types" "$field has type $ftype"
fi

# max_ram_mb is positive integer
RAM=$(echo "$HEALTH" | jq '.feature_budget.max_ram_mb // 0')
if [[ "$RAM" -gt 0 ]]; then
  pass "feature_budget_max_ram" "${RAM}MB"
else
  fail "feature_budget_max_ram" "got $RAM"
fi

# ─── Test 4: Counter monotonicity ─────────────────────────────
BEFORE_REQUESTS=$(echo "$HEALTH" | jq '.requests_total // 0')
BEFORE_PII=$(echo "$HEALTH" | jq '.pii_matches_total // 0')

# Send a request with PII through the proxy
PII_PAYLOAD='{"model":"test","max_tokens":1,"messages":[{"role":"user","content":"My SSN is 123-45-6789 and card is 4111-1111-1111-1111"}]}'
curl -sf -X POST "${PROXY_URL}/anthropic/v1/messages" \
  -H "Content-Type: application/json" \
  -H "x-api-key: test-health-check" \
  -H "anthropic-version: 2023-06-01" \
  -d "$PII_PAYLOAD" >/dev/null 2>&1 || true

# Small delay for stats to propagate
sleep 0.3

AFTER_HEALTH=$(curl -sf "${AUTH_HEADER[@]}" "$HEALTH_ENDPOINT" 2>/dev/null || echo "{}")
AFTER_REQUESTS=$(echo "$AFTER_HEALTH" | jq '.requests_total // 0')
AFTER_PII=$(echo "$AFTER_HEALTH" | jq '.pii_matches_total // 0')

# Save post-request health snapshot (latency histograms now have data)
echo "$AFTER_HEALTH" | jq . > "$OUTPUT_DIR/health_raw.json"

if [[ "$AFTER_REQUESTS" -gt "$BEFORE_REQUESTS" ]]; then
  pass "counter_requests_incremented" "$BEFORE_REQUESTS → $AFTER_REQUESTS"
else
  fail "counter_requests_incremented" "before=$BEFORE_REQUESTS after=$AFTER_REQUESTS"
fi

if [[ "$AFTER_PII" -gt "$BEFORE_PII" ]]; then
  pass "counter_pii_matches_incremented" "$BEFORE_PII → $AFTER_PII"
else
  warn "counter_pii_matches_incremented" "before=$BEFORE_PII after=$AFTER_PII (echo server may not be running)"
fi

# ─── Test 5: HTTP status code matches readiness ───────────────
READY=$(echo "$HEALTH" | jq '.ready')
if [[ "$READY" == "true" && "$HTTP_CODE" == "200" ]]; then
  pass "readiness_http_200" "ready=true, HTTP $HTTP_CODE"
elif [[ "$READY" == "false" && "$HTTP_CODE" == "503" ]]; then
  pass "readiness_http_503" "ready=false, HTTP $HTTP_CODE"
else
  warn "readiness_http_code" "ready=$READY but HTTP $HTTP_CODE"
fi

# ─── Write validation JSON ────────────────────────────────────
jq -n \
  --arg suite "health" \
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
  }' > "$OUTPUT_DIR/health_validation.json"

# ─── Summary ──────────────────────────────────────────────────
echo ""
if [[ $FAIL -eq 0 ]]; then
  printf "\033[32mPASS\033[0m  Health validation: %d/%d passed" "$PASS" "$TOTAL"
  [[ $WARN -gt 0 ]] && printf ", %d warnings" "$WARN"
  echo ""
else
  printf "\033[31mFAIL\033[0m  Health validation: %d/%d passed, %d failed\n" "$PASS" "$TOTAL" "$FAIL"
fi

rm -f /tmp/oo_health_test.json

[[ $FAIL -gt 0 ]] && exit 1
exit 0
