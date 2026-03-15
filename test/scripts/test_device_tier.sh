#!/usr/bin/env bash
# test_device_tier.sh — Validate device tier auto-detection and feature budget consistency.
#
# Test cases:
#   1. device_tier is one of full/standard/lite
#   2. Feature budget fields match tier expectations
#   3. All budget types are correct (booleans, integers, strings)
#   4. Tier matches system RAM (sanity check)
#
# Usage:
#   ./test/scripts/test_device_tier.sh
#
# Environment:
#   PROXY_URL   — Proxy base URL (default: http://127.0.0.1:18790)
#   AUTH_TOKEN   — Proxy auth token (default: read from ~/.openobscure/.auth-token)

set -euo pipefail

PROXY_URL="${PROXY_URL:-http://127.0.0.1:18790}"
HEALTH_ENDPOINT="${PROXY_URL}/_openobscure/health"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
TEST_DIR="$(dirname "$SCRIPT_DIR")"
OUTPUT_DIR="$TEST_DIR/data/output/device_tier"
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

# Fetch health
HEALTH=$(curl -sf "${AUTH_HEADER[@]}" "$HEALTH_ENDPOINT" 2>/dev/null || true)
if [[ -z "$HEALTH" ]]; then
  echo "Error: Proxy not reachable at $PROXY_URL"
  exit 1
fi

echo "=== Device Tier & Feature Budget Validation ==="
echo ""

TIER=$(echo "$HEALTH" | jq -r '.device_tier // ""')
BUDGET=$(echo "$HEALTH" | jq '.feature_budget // {}'  )

# ─── Test 1: Tier is a valid enum ─────────────────────────────
case "$TIER" in
  full|standard|lite) pass "tier_valid_enum" "$TIER" ;;
  *) fail "tier_valid_enum" "got '$TIER' (expected full/standard/lite)" ;;
esac

# ─── Test 2: Feature budget consistency per tier ──────────────

NER=$(echo "$BUDGET" | jq '.ner_enabled')
CRF=$(echo "$BUDGET" | jq '.crf_enabled')
ENSEMBLE=$(echo "$BUDGET" | jq '.ensemble_enabled')
OCR_TIER=$(echo "$BUDGET" | jq -r '.ocr_tier')
NSFW=$(echo "$BUDGET" | jq '.nsfw_enabled')
SCREEN=$(echo "$BUDGET" | jq '.screen_guard_enabled')
FACE=$(echo "$BUDGET" | jq -r '.face_model')
RAM=$(echo "$BUDGET" | jq '.max_ram_mb // 0')

case "$TIER" in
  full)
    # Full: NER=true, ensemble=true, NSFW=true, screen_guard=true, face=scrfd
    if [[ "$NER" == "true" ]]; then
      pass "full_ner_enabled" "ner_enabled=true"
    else
      fail "full_ner_enabled" "expected true, got $NER"
    fi

    if [[ "$ENSEMBLE" == "true" ]]; then
      pass "full_ensemble_enabled" "ensemble_enabled=true"
    else
      fail "full_ensemble_enabled" "expected true, got $ENSEMBLE"
    fi

    if [[ "$NSFW" == "true" ]]; then
      pass "full_nsfw_enabled" "nsfw_enabled=true"
    else
      warn "full_nsfw_enabled" "expected true, got $NSFW (model may not be present)"
    fi

    if [[ "$SCREEN" == "true" ]]; then
      pass "full_screen_guard" "screen_guard_enabled=true"
    else
      fail "full_screen_guard" "expected true, got $SCREEN"
    fi

    if [[ "$FACE" == "scrfd" ]]; then
      pass "full_face_model" "face_model=scrfd"
    else
      warn "full_face_model" "expected scrfd, got $FACE (scrfd model may not be present)"
    fi

    if [[ "$RAM" -ge 200 ]]; then
      pass "full_max_ram" "max_ram_mb=$RAM"
    else
      fail "full_max_ram" "expected >= 200, got $RAM"
    fi
    ;;

  standard)
    # Standard: NER=true, ensemble=false, NSFW=true, face=scrfd
    if [[ "$NER" == "true" ]]; then
      pass "standard_ner_enabled" "ner_enabled=true"
    else
      fail "standard_ner_enabled" "expected true, got $NER"
    fi

    if [[ "$ENSEMBLE" == "false" ]]; then
      pass "standard_no_ensemble" "ensemble_enabled=false"
    else
      warn "standard_no_ensemble" "expected false, got $ENSEMBLE"
    fi

    if [[ "$RAM" -ge 100 && "$RAM" -le 275 ]]; then
      pass "standard_max_ram" "max_ram_mb=$RAM"
    else
      warn "standard_max_ram" "got $RAM (expected 100-275)"
    fi
    ;;

  lite)
    # Lite: NER=false, CRF=true, ensemble=false, NSFW=false, face=blazeface
    if [[ "$NER" == "false" ]]; then
      pass "lite_no_ner" "ner_enabled=false"
    else
      warn "lite_no_ner" "expected false, got $NER"
    fi

    if [[ "$CRF" == "true" ]]; then
      pass "lite_crf_enabled" "crf_enabled=true"
    else
      warn "lite_crf_enabled" "expected true, got $CRF"
    fi

    if [[ "$NSFW" == "false" ]]; then
      pass "lite_no_nsfw" "nsfw_enabled=false"
    else
      warn "lite_no_nsfw" "expected false, got $NSFW"
    fi

    if [[ "$FACE" == "blazeface" ]]; then
      pass "lite_face_model" "face_model=blazeface"
    else
      warn "lite_face_model" "expected blazeface, got $FACE"
    fi

    if [[ "$RAM" -le 100 ]]; then
      pass "lite_max_ram" "max_ram_mb=$RAM"
    else
      warn "lite_max_ram" "got $RAM (expected <= 100)"
    fi
    ;;
esac

# ─── Test 3: Budget type validation ──────────────────────────
# OCR tier is valid
case "$OCR_TIER" in
  detect_and_fill|full_recognition) pass "ocr_tier_valid" "$OCR_TIER" ;;
  *) fail "ocr_tier_valid" "got '$OCR_TIER'" ;;
esac

# Face model is valid
case "$FACE" in
  scrfd|blazeface) pass "face_model_valid" "$FACE" ;;
  *) fail "face_model_valid" "got '$FACE'" ;;
esac

# Budget tier matches top-level tier
BUDGET_TIER=$(echo "$BUDGET" | jq -r '.tier // ""')
if [[ "$BUDGET_TIER" == "$TIER" ]]; then
  pass "budget_tier_matches" "both report '$TIER'"
else
  fail "budget_tier_matches" "device_tier=$TIER but budget.tier=$BUDGET_TIER"
fi

# ─── Test 4: System RAM sanity check ─────────────────────────
SYSTEM_RAM_MB=0
if [[ "$(uname)" == "Darwin" ]]; then
  SYSTEM_RAM_BYTES=$(sysctl -n hw.memsize 2>/dev/null || echo 0)
  SYSTEM_RAM_MB=$((SYSTEM_RAM_BYTES / 1048576))
elif [[ -f /proc/meminfo ]]; then
  SYSTEM_RAM_KB=$(awk '/MemTotal/ {print $2}' /proc/meminfo 2>/dev/null || echo 0)
  SYSTEM_RAM_MB=$((SYSTEM_RAM_KB / 1024))
fi

if [[ "$SYSTEM_RAM_MB" -gt 0 ]]; then
  if [[ "$SYSTEM_RAM_MB" -ge 8192 && "$TIER" == "full" ]]; then
    pass "tier_matches_system_ram" "system=${SYSTEM_RAM_MB}MB → full"
  elif [[ "$SYSTEM_RAM_MB" -ge 4096 && "$SYSTEM_RAM_MB" -lt 8192 && "$TIER" == "standard" ]]; then
    pass "tier_matches_system_ram" "system=${SYSTEM_RAM_MB}MB → standard"
  elif [[ "$SYSTEM_RAM_MB" -lt 4096 && "$TIER" == "lite" ]]; then
    pass "tier_matches_system_ram" "system=${SYSTEM_RAM_MB}MB → lite"
  else
    warn "tier_matches_system_ram" "system=${SYSTEM_RAM_MB}MB detected tier=$TIER (may be config-overridden)"
  fi
else
  warn "tier_matches_system_ram" "could not detect system RAM"
fi

# ─── Write validation JSON ────────────────────────────────────
jq -n \
  --arg suite "device_tier" \
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
  }' > "$OUTPUT_DIR/tier_validation.json"

# ─── Summary ──────────────────────────────────────────────────
echo ""
if [[ $FAIL -eq 0 ]]; then
  printf "\033[32mPASS\033[0m  Device tier validation (%s): %d/%d passed" "$TIER" "$PASS" "$TOTAL"
  [[ $WARN -gt 0 ]] && printf ", %d warnings" "$WARN"
  echo ""
else
  printf "\033[31mFAIL\033[0m  Device tier validation (%s): %d/%d passed, %d failed\n" "$TIER" "$PASS" "$TOTAL" "$FAIL"
fi

[[ $FAIL -gt 0 ]] && exit 1
exit 0
