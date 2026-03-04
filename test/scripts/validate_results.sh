#!/usr/bin/env bash
# validate_results.sh — Pass/fail validation of PII detection test results.
#
# Threshold mode (default): Reads expected_results.json manifest and checks:
#   1. Gateway JSON output exists for each file
#   2. Redacted output exists for each file
#   3. total_matches >= min_matches threshold
#   4. All expected PII types appear in the type_summary
#   5. must_detect PII strings are covered by scanner matches
#   6. Redacted file differs from original (FPE/labels applied)
#   7. FPE HTTP status was 200 (if field present)
#
# Strict mode (--strict): Reads snapshot.json and checks exact counts:
#   - Gateway: total_matches and per-type counts must match exactly
#   - Audio: pii_detected, keywords, action must match exactly
#   - Visual: faces_redacted, text_regions, nsfw_blocked, screenshot_detected must match exactly
#
# Also checks for input files NOT in the manifest (coverage gaps).
#
# Exit code: 0 = all pass, 1 = any fail
#
# Usage:
#   ./test/scripts/validate_results.sh                     # Threshold validation
#   ./test/scripts/validate_results.sh --strict             # Exact snapshot comparison
#   ./test/scripts/validate_results.sh --check-redacted     # Validate redacted file content
#   ./test/scripts/validate_results.sh --gateway-only       # Skip embedded checks
#   ./test/scripts/validate_results.sh --summary            # Summary only (no per-file output)
#   ./test/scripts/validate_results.sh --json               # JSON report to stdout
#   ./test/scripts/validate_results.sh --infrastructure     # Infrastructure test results
#   ./test/scripts/validate_results.sh --validate-only      # Schema + input validation only
#   ./test/scripts/validate_results.sh --run                 # Run all tests + validate results

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
TEST_DIR="$(dirname "$SCRIPT_DIR")"
MANIFEST="$TEST_DIR/expected_results.json"
SNAPSHOT="$TEST_DIR/snapshot.json"
INPUT_DIR="$TEST_DIR/data/input"
OUTPUT_DIR="$TEST_DIR/data/output"

# Parse flags
GATEWAY_ONLY=false
SUMMARY_ONLY=false
JSON_OUTPUT=false
STRICT=false
CHECK_REDACTED=false
INFRASTRUCTURE=false
VALIDATE_ONLY=false
RUN_TESTS=false
for arg in "$@"; do
  case "$arg" in
    --gateway-only) GATEWAY_ONLY=true ;;
    --summary) SUMMARY_ONLY=true ;;
    --json) JSON_OUTPUT=true ;;
    --strict) STRICT=true ;;
    --check-redacted) CHECK_REDACTED=true ;;
    --infrastructure) INFRASTRUCTURE=true ;;
    --validate-only) VALIDATE_ONLY=true ;;
    --run) RUN_TESTS=true ;;
  esac
done

# --run implies --infrastructure (validate after running tests)
if [[ "$RUN_TESTS" == "true" ]]; then
  INFRASTRUCTURE=true
fi

# Verify required files exist (skip for infrastructure-only or validate-only mode)
GW_JSON_COUNT=${GW_JSON_COUNT:-0}
EM_JSON_COUNT=${EM_JSON_COUNT:-0}
if [[ "$VALIDATE_ONLY" == "true" ]]; then
  : # Validate-only mode: no manifest/snapshot needed
elif [[ "$INFRASTRUCTURE" == "true" && $GW_JSON_COUNT -eq 0 && $EM_JSON_COUNT -eq 0 ]]; then
  : # Infrastructure-only mode: no manifest/snapshot needed
elif [[ "$STRICT" == "true" ]]; then
  if [[ ! -f "$SNAPSHOT" ]]; then
    echo "Error: Snapshot not found: $SNAPSHOT"
    echo "Generate it first: ./test/scripts/generate_snapshot.sh"
    exit 2
  fi
else
  if [[ ! -f "$MANIFEST" ]]; then
    echo "Error: Manifest not found: $MANIFEST"
    echo "Run from project root: ./test/scripts/validate_results.sh"
    exit 2
  fi
fi

# Counters
PASS=0
FAIL=0
WARN=0
SKIP=0
TOTAL=0

# Failure details for JSON output
FAILURES=()
WARNINGS=()

# Mode label
if [[ "$VALIDATE_ONLY" == "true" ]]; then
  MODE_LABEL="validate-only"
elif [[ "$RUN_TESTS" == "true" && "$STRICT" == "true" ]]; then
  MODE_LABEL="run+strict"
elif [[ "$RUN_TESTS" == "true" ]]; then
  MODE_LABEL="run+infrastructure"
elif [[ "$INFRASTRUCTURE" == "true" && "$STRICT" == "false" ]]; then
  MODE_LABEL="infrastructure"
elif [[ "$STRICT" == "true" && "$INFRASTRUCTURE" == "true" ]]; then
  MODE_LABEL="strict+infrastructure"
elif [[ "$STRICT" == "true" ]]; then
  MODE_LABEL="strict"
else
  MODE_LABEL="threshold"
fi

# ─── Helpers ──────────────────────────────────────────────────

pass() {
  PASS=$((PASS + 1))
  if [[ "$SUMMARY_ONLY" == "false" && "$JSON_OUTPUT" == "false" ]]; then
    printf "  \033[32mPASS\033[0m  %-50s  %s\n" "$1" "$2"
  fi
}

fail() {
  FAIL=$((FAIL + 1))
  FAILURES+=("$1: $2")
  if [[ "$SUMMARY_ONLY" == "false" && "$JSON_OUTPUT" == "false" ]]; then
    printf "  \033[31mFAIL\033[0m  %-50s  %s\n" "$1" "$2"
  fi
}

warn() {
  WARN=$((WARN + 1))
  WARNINGS+=("$1: $2")
  if [[ "$SUMMARY_ONLY" == "false" && "$JSON_OUTPUT" == "false" ]]; then
    printf "  \033[33mWARN\033[0m  %-50s  %s\n" "$1" "$2"
  fi
}

skip() {
  SKIP=$((SKIP + 1))
  if [[ "$SUMMARY_ONLY" == "false" && "$JSON_OUTPUT" == "false" ]]; then
    printf "  \033[90mSKIP\033[0m  %-50s  %s\n" "$1" "$2"
  fi
}

# Print timing statistics (min/avg/p50/p95/max) for a list of values.
# Usage: print_timing_stats "metric_name" val1 val2 val3...
# Skips if no values provided.
print_timing_stats() {
  local metric="$1"
  shift
  local -a values=("$@")
  local count=${#values[@]}
  [[ $count -eq 0 ]] && return

  # Sort values numerically
  IFS=$'\n' read -r -d '' -a sorted < <(printf '%s\n' "${values[@]}" | sort -n && printf '\0') || true

  local min=${sorted[0]}
  local max=${sorted[$((count - 1))]}

  # Average
  local sum=0
  for v in "${sorted[@]}"; do sum=$((sum + v)); done
  local avg=$((sum / count))

  # p50 (median): index = ceil(count * 0.50) - 1
  local p50_idx=$(( (count * 50 + 49) / 100 - 1 ))
  [[ $p50_idx -lt 0 ]] && p50_idx=0
  local p50=${sorted[$p50_idx]}

  # p95: index = ceil(count * 0.95) - 1
  local p95_idx=$(( (count * 95 + 49) / 100 - 1 ))
  [[ $p95_idx -ge $count ]] && p95_idx=$((count - 1))
  local p95=${sorted[$p95_idx]}

  printf "  %-25s %5d  %8d  %8d  %8d  %8d  %8d\n" "$metric" "$count" "$min" "$avg" "$p50" "$p95" "$max"
}

# ─── --run helpers: execute test scripts ─────────────────────

RUN_TOTAL=0
RUN_PASS=0
RUN_FAIL=0

run_script() {
  local script="$1"
  local script_path="$SCRIPT_DIR/$script"
  local log_file
  log_file=$(mktemp /tmp/oo_run_XXXXXX.log)

  RUN_TOTAL=$((RUN_TOTAL + 1))

  if [[ "$JSON_OUTPUT" == "false" ]]; then
    printf "  %-45s " "$script"
  fi

  if "$script_path" > "$log_file" 2>&1; then
    RUN_PASS=$((RUN_PASS + 1))
    if [[ "$JSON_OUTPUT" == "false" ]]; then
      printf "\033[32mOK\033[0m\n"
    fi
  else
    local rc=$?
    RUN_FAIL=$((RUN_FAIL + 1))
    if [[ "$JSON_OUTPUT" == "false" ]]; then
      printf "\033[31mFAIL\033[0m (exit %d)\n" "$rc"
      tail -5 "$log_file" | sed 's/^/    /'
    fi
  fi
  rm -f "$log_file"
}

run_node_script() {
  local script="$1"
  local script_path="$SCRIPT_DIR/$script"
  local log_file
  log_file=$(mktemp /tmp/oo_run_XXXXXX.log)

  RUN_TOTAL=$((RUN_TOTAL + 1))

  if [[ "$JSON_OUTPUT" == "false" ]]; then
    printf "  %-45s " "$script"
  fi

  if node "$script_path" > "$log_file" 2>&1; then
    RUN_PASS=$((RUN_PASS + 1))
    if [[ "$JSON_OUTPUT" == "false" ]]; then
      printf "\033[32mOK\033[0m\n"
    fi
  else
    local rc=$?
    RUN_FAIL=$((RUN_FAIL + 1))
    if [[ "$JSON_OUTPUT" == "false" ]]; then
      printf "\033[31mFAIL\033[0m (exit %d)\n" "$rc"
      tail -5 "$log_file" | sed 's/^/    /'
    fi
  fi
  rm -f "$log_file"
}

# ─── Schema validation for *_validation.json files ───────────

# validate_schema FILE
# Validates that a *_validation.json file conforms to the expected schema.
# Returns 0 on pass, 1 on failure. Prints diagnostics.
validate_schema() {
  local vfile="$1"
  local fname
  fname=$(basename "$vfile")
  local errors=0

  # 1. Valid JSON
  if ! jq empty "$vfile" 2>/dev/null; then
    printf "  \033[31mSCHEMA FAIL\033[0m  %-40s  %s\n" "$fname" "invalid JSON"
    return 1
  fi

  # 2. Required top-level fields with types
  local suite ts total_v pass_v fail_v warn_v skip_v results_type

  suite=$(jq -r '.test_suite // empty' "$vfile")
  if [[ -z "$suite" ]]; then
    printf "  \033[31mSCHEMA FAIL\033[0m  %-40s  %s\n" "$fname" "missing or empty .test_suite"
    errors=$((errors + 1))
  fi

  ts=$(jq -r '.timestamp // empty' "$vfile")
  if [[ -z "$ts" ]]; then
    printf "  \033[31mSCHEMA FAIL\033[0m  %-40s  %s\n" "$fname" "missing .timestamp"
    errors=$((errors + 1))
  elif [[ ! "$ts" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}T ]]; then
    printf "  \033[31mSCHEMA FAIL\033[0m  %-40s  %s\n" "$fname" ".timestamp not ISO 8601: $ts"
    errors=$((errors + 1))
  fi

  for field in total pass fail warn skip; do
    local val
    val=$(jq ".$field // null" "$vfile")
    if [[ "$val" == "null" ]]; then
      printf "  \033[31mSCHEMA FAIL\033[0m  %-40s  %s\n" "$fname" "missing .$field"
      errors=$((errors + 1))
    elif ! [[ "$val" =~ ^[0-9]+$ ]]; then
      printf "  \033[31mSCHEMA FAIL\033[0m  %-40s  %s\n" "$fname" ".$field not a non-negative integer: $val"
      errors=$((errors + 1))
    fi
  done

  results_type=$(jq -r '.results | type' "$vfile" 2>/dev/null)
  if [[ "$results_type" != "array" ]]; then
    printf "  \033[31mSCHEMA FAIL\033[0m  %-40s  %s\n" "$fname" ".results is not an array (got $results_type)"
    errors=$((errors + 1))
  fi

  # Stop here if structural errors found
  [[ $errors -gt 0 ]] && return 1

  # 3. Counter consistency
  total_v=$(jq '.total' "$vfile")
  pass_v=$(jq '.pass' "$vfile")
  fail_v=$(jq '.fail' "$vfile")
  warn_v=$(jq '.warn' "$vfile")
  skip_v=$(jq '.skip' "$vfile")
  local counter_sum=$((pass_v + fail_v + warn_v + skip_v))
  if [[ "$counter_sum" -ne "$total_v" ]]; then
    printf "  \033[33mSCHEMA WARN\033[0m  %-40s  %s\n" "$fname" "counter sum ($counter_sum) != total ($total_v)"
  fi

  # 4. Array length consistency
  local arr_len
  arr_len=$(jq '.results | length' "$vfile")
  if [[ "$arr_len" -ne "$total_v" ]]; then
    printf "  \033[33mSCHEMA WARN\033[0m  %-40s  %s\n" "$fname" "results length ($arr_len) != total ($total_v)"
  fi

  # 5. Validate each result entry
  local entry_errors=0
  local entry_count
  entry_count=$(jq '.results | length' "$vfile")
  for ((i = 0; i < entry_count; i++)); do
    local ename estatus edetail
    ename=$(jq -r ".results[$i].name // empty" "$vfile")
    estatus=$(jq -r ".results[$i].status // empty" "$vfile")
    edetail=$(jq -r ".results[$i].detail // empty" "$vfile")

    if [[ -z "$ename" ]]; then
      printf "  \033[31mSCHEMA FAIL\033[0m  %-40s  %s\n" "$fname" "results[$i] missing .name"
      entry_errors=$((entry_errors + 1))
    fi
    if [[ -z "$estatus" ]]; then
      printf "  \033[31mSCHEMA FAIL\033[0m  %-40s  %s\n" "$fname" "results[$i] missing .status"
      entry_errors=$((entry_errors + 1))
    elif [[ "$estatus" != "pass" && "$estatus" != "fail" && "$estatus" != "warn" && "$estatus" != "skip" ]]; then
      printf "  \033[31mSCHEMA FAIL\033[0m  %-40s  %s\n" "$fname" "results[$i].status invalid: $estatus"
      entry_errors=$((entry_errors + 1))
    fi
    if [[ -z "$edetail" ]]; then
      printf "  \033[31mSCHEMA FAIL\033[0m  %-40s  %s\n" "$fname" "results[$i] missing .detail"
      entry_errors=$((entry_errors + 1))
    fi
  done

  [[ $entry_errors -gt 0 ]] && return 1
  return 0
}

# validate_test_scripts
# Checks that each expected infrastructure test script exists, is executable,
# has a proper shebang, creates its output directory, and writes validation JSON.
validate_test_scripts() {
  local scripts_dir="$SCRIPT_DIR"
  local expected_scripts=(
    test_health.sh
    test_auth.sh
    test_body_limits.sh
    test_device_tier.sh
    test_fail_mode.sh
    test_key_rotation.sh
    test_response_integrity.sh
    test_sse_streaming.sh
    test_gateway_category.sh
    test_agent_json.sh
    test_audio.sh
    test_visual.sh
  )
  local input_pass=0
  local input_fail=0

  for script_name in "${expected_scripts[@]}"; do
    local script_path="$scripts_dir/$script_name"

    if [[ ! -f "$script_path" ]]; then
      printf "  \033[31mFAIL\033[0m  %-40s  %s\n" "$script_name" "file not found"
      input_fail=$((input_fail + 1))
      continue
    fi

    if [[ ! -x "$script_path" ]]; then
      printf "  \033[31mFAIL\033[0m  %-40s  %s\n" "$script_name" "not executable"
      input_fail=$((input_fail + 1))
      continue
    fi

    local shebang
    shebang=$(head -1 "$script_path")
    if [[ "$shebang" != "#!/usr/bin/env bash" && "$shebang" != "#!/usr/bin/env node" && "$shebang" != "#!/bin/bash" ]]; then
      printf "  \033[31mFAIL\033[0m  %-40s  %s\n" "$script_name" "bad shebang: $shebang"
      input_fail=$((input_fail + 1))
      continue
    fi

    if ! grep -q 'mkdir -p' "$script_path"; then
      printf "  \033[33mWARN\033[0m  %-40s  %s\n" "$script_name" "no mkdir -p for output directory"
    fi

    if ! grep -q '_validation\.json' "$script_path"; then
      printf "  \033[31mFAIL\033[0m  %-40s  %s\n" "$script_name" "does not write _validation.json"
      input_fail=$((input_fail + 1))
      continue
    fi

    printf "  \033[32mPASS\033[0m  %-40s  %s\n" "$script_name" "exists, executable, valid structure"
    input_pass=$((input_pass + 1))
  done

  echo ""
  printf "  Input validation: %d passed, %d failed (%d scripts)\n" "$input_pass" "$input_fail" "${#expected_scripts[@]}"
  return $input_fail
}

# ─── --validate-only: schema + input checks, then exit ────────

if [[ "$VALIDATE_ONLY" == "true" ]]; then
  V_PASS=0
  V_FAIL=0
  V_TOTAL=0

  if [[ "$JSON_OUTPUT" == "false" ]]; then
    echo "============================================"
    echo "  OpenObscure Validation Schema Checker"
    echo "  $(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "============================================"
    echo ""
    echo "--- Input Validation (test scripts) ---"
  fi

  INPUT_FAIL=0
  if [[ "$JSON_OUTPUT" == "false" ]]; then
    validate_test_scripts || INPUT_FAIL=$?
  else
    validate_test_scripts > /dev/null 2>&1 || INPUT_FAIL=$?
  fi

  if [[ "$JSON_OUTPUT" == "false" ]]; then
    echo ""
    echo "--- Output Validation (*_validation.json) ---"
  fi

  SCHEMA_FOUND=0
  while IFS= read -r vfile; do
    [[ -f "$vfile" ]] || continue
    SCHEMA_FOUND=$((SCHEMA_FOUND + 1))
    V_TOTAL=$((V_TOTAL + 1))
    if validate_schema "$vfile"; then
      V_PASS=$((V_PASS + 1))
      if [[ "$JSON_OUTPUT" == "false" ]]; then
        printf "  \033[32mPASS\033[0m  %-40s  %s\n" "$(basename "$vfile")" "schema valid"
      fi
    else
      V_FAIL=$((V_FAIL + 1))
    fi
  done < <(find "$OUTPUT_DIR" -name "*_validation.json" -type f 2>/dev/null | sort)

  if [[ "$SCHEMA_FOUND" -eq 0 && "$JSON_OUTPUT" == "false" ]]; then
    echo "  No *_validation.json files found. Run infrastructure tests first."
  fi

  TOTAL_FAIL=$((V_FAIL + INPUT_FAIL))

  if [[ "$JSON_OUTPUT" == "true" ]]; then
    jq -n \
      --arg ts "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
      --arg mode "validate-only" \
      --argjson schema_pass "$V_PASS" \
      --argjson schema_fail "$V_FAIL" \
      --argjson schema_total "$V_TOTAL" \
      --argjson input_fail "$INPUT_FAIL" \
      --arg result "$(if [[ $TOTAL_FAIL -gt 0 ]]; then echo "FAIL"; else echo "PASS"; fi)" \
      '{
        timestamp: $ts,
        mode: $mode,
        result: $result,
        schema_pass: $schema_pass,
        schema_fail: $schema_fail,
        schema_total: $schema_total,
        input_failures: $input_fail
      }'
  else
    echo ""
    echo "============================================"
    if [[ $TOTAL_FAIL -eq 0 ]]; then
      printf "  Result: \033[32mALL PASS\033[0m (validate-only)\n"
    else
      printf "  Result: \033[31mFAIL\033[0m (validate-only)\n"
    fi
    printf "  Schema:  %d / %d passed\n" "$V_PASS" "$V_TOTAL"
    printf "  Input:   %d failures\n" "$INPUT_FAIL"
    echo "============================================"
  fi

  if [[ $TOTAL_FAIL -gt 0 ]]; then
    exit 1
  fi
  exit 0
fi

# ─── --run: execute all test scripts, then fall through to validation ──

if [[ "$RUN_TESTS" == "true" ]]; then

  PROXY_URL="${PROXY_URL:-http://127.0.0.1:18790}"

  # Auth token (same pattern used by test scripts)
  if [[ -z "${AUTH_TOKEN:-}" ]]; then
    TOKEN_FILE="$HOME/.openobscure/.auth-token"
    if [[ -f "$TOKEN_FILE" ]]; then
      AUTH_TOKEN=$(cat "$TOKEN_FILE")
    else
      AUTH_TOKEN=""
    fi
  fi
  export AUTH_TOKEN

  # Pre-flight: verify proxy is reachable
  if [[ -n "$AUTH_TOKEN" ]]; then
    RUN_HEALTH=$(curl -sf "${PROXY_URL}/_openobscure/health" -H "X-OpenObscure-Token: $AUTH_TOKEN" 2>/dev/null || true)
  else
    RUN_HEALTH=$(curl -sf "${PROXY_URL}/_openobscure/health" 2>/dev/null || true)
  fi
  if [[ -z "$RUN_HEALTH" ]]; then
    echo "Error: Proxy not reachable at $PROXY_URL"
    echo ""
    echo "Start the proxy first:"
    echo "  OPENOBSCURE_CONFIG=test/config/test_fpe.toml \\"
    echo "    ./openobscure-proxy/target/debug/openobscure-proxy serve"
    exit 1
  fi

  RUN_VERSION=$(echo "$RUN_HEALTH" | jq -r '.version // "unknown"')
  RUN_TIER=$(echo "$RUN_HEALTH" | jq -r '.device_tier // "unknown"')
  RUN_START=$(date +%s)

  if [[ "$JSON_OUTPUT" == "false" ]]; then
    echo "============================================"
    echo "  OpenObscure Full Test + Validation"
    echo "  Proxy: $PROXY_URL (v$RUN_VERSION, tier: $RUN_TIER)"
    echo "  $(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "============================================"
    echo ""
    echo "--- Phase 1: Gateway + Visual + Audio ---"
  fi

  run_script "test_gateway_all.sh"

  if [[ "$JSON_OUTPUT" == "false" ]]; then
    echo ""
    echo "--- Phase 2: Embedded ---"
  fi

  run_node_script "test_embedded_all.mjs"

  if [[ "$JSON_OUTPUT" == "false" ]]; then
    echo ""
    echo "--- Phase 3: Infrastructure ---"
  fi

  INFRA_RUN_SCRIPTS=(
    test_health.sh
    test_auth.sh
    test_body_limits.sh
    test_device_tier.sh
    test_fail_mode.sh
    test_key_rotation.sh
    test_response_integrity.sh
    test_sse_streaming.sh
  )

  for iscript in "${INFRA_RUN_SCRIPTS[@]}"; do
    if [[ -x "$SCRIPT_DIR/$iscript" ]]; then
      run_script "$iscript"
    else
      if [[ "$JSON_OUTPUT" == "false" ]]; then
        printf "  %-45s \033[90mSKIP\033[0m (not found)\n" "$iscript"
      fi
    fi
  done

  RUN_END=$(date +%s)
  RUN_ELAPSED=$((RUN_END - RUN_START))

  if [[ "$JSON_OUTPUT" == "false" ]]; then
    echo ""
    echo "--- Run Summary ---"
    printf "  Scripts run: %d  (passed: %d, failed: %d)\n" "$RUN_TOTAL" "$RUN_PASS" "$RUN_FAIL"
    echo "  Elapsed: ${RUN_ELAPSED}s"
    echo ""
    echo "--- Validating Results ---"
  fi

  # Fall through to --infrastructure validation below
fi

# ─── Header ───────────────────────────────────────────────────

if [[ "$JSON_OUTPUT" == "false" ]]; then
  echo "============================================"
  echo "  OpenObscure PII Detection Validator"
  echo "  Mode: $MODE_LABEL"
  echo "  $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "============================================"
  echo ""
fi

# ─── Check 0: Any results exist? ─────────────────────────────

GW_JSON_COUNT=0
REDACTED_COUNT=0
EM_JSON_COUNT=0

while IFS= read -r -d '' _; do
  GW_JSON_COUNT=$((GW_JSON_COUNT + 1))
done < <(find "$OUTPUT_DIR" -path "*/json/*_gateway.json" -print0 2>/dev/null)

while IFS= read -r -d '' _; do
  REDACTED_COUNT=$((REDACTED_COUNT + 1))
done < <(find "$OUTPUT_DIR" -path "*/redacted/*" -type f -print0 2>/dev/null)

while IFS= read -r -d '' _; do
  EM_JSON_COUNT=$((EM_JSON_COUNT + 1))
done < <(find "$OUTPUT_DIR" -path "*/json/*_embedded.json" -print0 2>/dev/null)

if [[ "$JSON_OUTPUT" == "false" ]]; then
  echo "Found: $GW_JSON_COUNT gateway JSON, $EM_JSON_COUNT embedded JSON, $REDACTED_COUNT redacted files"
  echo ""
fi

if [[ $GW_JSON_COUNT -eq 0 && $EM_JSON_COUNT -eq 0 && "$INFRASTRUCTURE" == "false" ]]; then
  if [[ "$JSON_OUTPUT" == "false" ]]; then
    echo "Error: No test results found in $OUTPUT_DIR"
    echo ""
    echo "Run the test suite first:"
    echo "  ./test/scripts/test_gateway_all.sh    # Gateway FPE tests"
    echo "  node test/scripts/test_embedded_all.mjs  # Embedded label tests"
  fi
  exit 2
fi

# ═══════════════════════════════════════════════════════════════
# STRICT MODE: Exact snapshot comparison
# ═══════════════════════════════════════════════════════════════

if [[ $GW_JSON_COUNT -eq 0 && $EM_JSON_COUNT -eq 0 ]]; then
  : # No PII results — skip strict/threshold validation (infrastructure-only mode)
elif [[ "$STRICT" == "true" ]]; then

  # ─── Strict: Gateway validation ──────────────────────────────

  STRICT_GW_KEYS=$(jq -r '.gateway | keys[]' "$SNAPSHOT" 2>/dev/null)

  if [[ -n "$STRICT_GW_KEYS" ]]; then
    CURRENT_CAT=""
    for key in $STRICT_GW_KEYS; do
      TOTAL=$((TOTAL + 1))
      category="${key%%/*}"
      filename="${key#*/}"
      name_no_ext="${filename%.*}"

      if [[ "$category" != "$CURRENT_CAT" ]]; then
        CURRENT_CAT="$category"
        if [[ "$SUMMARY_ONLY" == "false" && "$JSON_OUTPUT" == "false" ]]; then
          echo "--- $category ---"
        fi
      fi

      gw_json="$OUTPUT_DIR/$category/json/${name_no_ext}_gateway.json"

      if [[ ! -f "$gw_json" ]]; then
        skip "$key" "no gateway JSON"
        continue
      fi

      # Compare total_matches exactly
      expected_total=$(jq -r ".gateway[\"$key\"].total_matches" "$SNAPSHOT")
      actual_total=$(jq '.total_matches // 0' "$gw_json")

      if [[ "$actual_total" -ne "$expected_total" ]]; then
        fail "$key" "total_matches: got $actual_total, expected $expected_total"
        continue
      fi

      # Compare per-type counts exactly
      type_mismatch=false
      type_msg=""
      expected_types_keys=$(jq -r ".gateway[\"$key\"].type_summary | keys[]" "$SNAPSHOT" 2>/dev/null)
      for etype in $expected_types_keys; do
        exp_count=$(jq -r ".gateway[\"$key\"].type_summary[\"$etype\"]" "$SNAPSHOT")
        act_count=$(jq -r ".type_summary[\"$etype\"] // 0" "$gw_json")
        if [[ "$act_count" -ne "$exp_count" ]]; then
          type_mismatch=true
          type_msg="$etype: got $act_count, expected $exp_count"
          break
        fi
      done

      # Check for unexpected new types
      if [[ "$type_mismatch" == "false" ]]; then
        actual_types_keys=$(jq -r '.type_summary | keys[]' "$gw_json" 2>/dev/null)
        for atype in $actual_types_keys; do
          has_expected=$(jq -r ".gateway[\"$key\"].type_summary[\"$atype\"] // \"missing\"" "$SNAPSHOT")
          if [[ "$has_expected" == "missing" ]]; then
            warn "$key" "unexpected new type '$atype' not in snapshot"
          fi
        done
      fi

      if [[ "$type_mismatch" == "true" ]]; then
        fail "$key" "type mismatch: $type_msg"
        continue
      fi

      pass "$key" "$actual_total matches (exact)"
    done
  fi

  # ─── Strict: Embedded validation ────────────────────────────────

  if [[ "$GATEWAY_ONLY" == "false" ]]; then
    STRICT_EM_KEYS=$(jq -r '.embedded // {} | keys[]' "$SNAPSHOT" 2>/dev/null)

    if [[ -n "$STRICT_EM_KEYS" ]]; then
      CURRENT_CAT=""
      for key in $STRICT_EM_KEYS; do
        TOTAL=$((TOTAL + 1))
        category="${key%%/*}"
        filename="${key#*/}"
        name_no_ext="${filename%.*}"

        if [[ "$category" != "$CURRENT_CAT" ]]; then
          CURRENT_CAT="$category"
          if [[ "$SUMMARY_ONLY" == "false" && "$JSON_OUTPUT" == "false" ]]; then
            echo "--- $category (embedded) ---"
          fi
        fi

        em_json="$OUTPUT_DIR/$category/json/${name_no_ext}_embedded.json"

        if [[ ! -f "$em_json" ]]; then
          skip "$key (embedded)" "no embedded JSON"
          continue
        fi

        expected_total=$(jq -r ".embedded[\"$key\"].total_matches" "$SNAPSHOT")
        actual_total=$(jq '.total_matches // 0' "$em_json")

        if [[ "$actual_total" -ne "$expected_total" ]]; then
          fail "$key (embedded)" "total_matches: got $actual_total, expected $expected_total"
          continue
        fi

        # Compare per-type counts exactly
        type_mismatch=false
        type_msg=""
        expected_types_keys=$(jq -r ".embedded[\"$key\"].type_summary | keys[]" "$SNAPSHOT" 2>/dev/null)
        for etype in $expected_types_keys; do
          exp_count=$(jq -r ".embedded[\"$key\"].type_summary[\"$etype\"]" "$SNAPSHOT")
          act_count=$(jq -r ".type_summary[\"$etype\"] // 0" "$em_json")
          if [[ "$act_count" -ne "$exp_count" ]]; then
            type_mismatch=true
            type_msg="$etype: got $act_count, expected $exp_count"
            break
          fi
        done

        # Check for unexpected new types
        if [[ "$type_mismatch" == "false" ]]; then
          actual_types_keys=$(jq -r '.type_summary | keys[]' "$em_json" 2>/dev/null)
          for atype in $actual_types_keys; do
            has_expected=$(jq -r ".embedded[\"$key\"].type_summary[\"$atype\"] // \"missing\"" "$SNAPSHOT")
            if [[ "$has_expected" == "missing" ]]; then
              warn "$key (embedded)" "unexpected new type '$atype' not in snapshot"
            fi
          done
        fi

        if [[ "$type_mismatch" == "true" ]]; then
          fail "$key (embedded)" "type mismatch: $type_msg"
          continue
        fi

        pass "$key (embedded)" "$actual_total matches (exact)"
      done
    fi
  fi

  # ─── Strict: Audio validation ──────────────────────────────────

  if [[ "$GATEWAY_ONLY" == "false" ]]; then
    STRICT_AUDIO_KEYS=$(jq -r '.audio | keys[]' "$SNAPSHOT" 2>/dev/null)

    if [[ -n "$STRICT_AUDIO_KEYS" ]]; then
      if [[ "$SUMMARY_ONLY" == "false" && "$JSON_OUTPUT" == "false" ]]; then
        echo ""
        echo "--- Audio_PII ---"
      fi

      for key in $STRICT_AUDIO_KEYS; do
        TOTAL=$((TOTAL + 1))
        filename="${key#Audio_PII/}"
        ext="${filename##*.}"
        name_no_ext="${filename%.*}"
        # Audio output files use underscored extension in filename
        audio_json="$OUTPUT_DIR/Audio_PII/json/${name_no_ext}_${ext}_audio.json"

        if [[ ! -f "$audio_json" ]]; then
          skip "$key" "no audio JSON"
          continue
        fi

        exp_pii=$(jq -r ".audio[\"$key\"].pii_detected" "$SNAPSHOT")
        exp_kw=$(jq -r ".audio[\"$key\"].keywords" "$SNAPSHOT")
        exp_action=$(jq -r ".audio[\"$key\"].action" "$SNAPSHOT")

        act_pii=$(jq '.kws_results.pii_detected // false' "$audio_json")
        act_kw=$(jq -r '.kws_results.keywords // ""' "$audio_json")
        act_action=$(jq -r '.kws_results.action // "UNKNOWN"' "$audio_json")

        if [[ "$act_pii" != "$exp_pii" ]]; then
          fail "$key" "pii_detected: got $act_pii, expected $exp_pii"
          continue
        fi

        if [[ "$act_action" != "$exp_action" ]]; then
          fail "$key" "action: got $act_action, expected $exp_action"
          continue
        fi

        if [[ "$act_kw" != "$exp_kw" ]]; then
          warn "$key" "keywords differ: got '$act_kw', expected '$exp_kw'"
        fi

        pass "$key" "$act_action ($act_kw)"
      done
    fi

    # ─── Strict: Visual validation ────────────────────────────────

    STRICT_VISUAL_KEYS=$(jq -r '.visual | keys[]' "$SNAPSHOT" 2>/dev/null)

    if [[ -n "$STRICT_VISUAL_KEYS" ]]; then
      if [[ "$SUMMARY_ONLY" == "false" && "$JSON_OUTPUT" == "false" ]]; then
        echo ""
        echo "--- Visual_PII ---"
      fi

      for key in $STRICT_VISUAL_KEYS; do
        TOTAL=$((TOTAL + 1))
        filename="${key#Visual_PII/}"
        name_no_ext="${filename%.*}"
        visual_json="$OUTPUT_DIR/Visual_PII/json/${name_no_ext}_visual.json"

        if [[ ! -f "$visual_json" ]]; then
          skip "$key" "no visual JSON"
          continue
        fi

        exp_faces=$(jq -r ".visual[\"$key\"].faces_redacted" "$SNAPSHOT")
        exp_text=$(jq -r ".visual[\"$key\"].text_regions_detected" "$SNAPSHOT")
        exp_nsfw=$(jq -r ".visual[\"$key\"].nsfw_blocked // false" "$SNAPSHOT")
        exp_screenshot=$(jq -r ".visual[\"$key\"].screenshot_detected // false" "$SNAPSHOT")

        act_faces=$(jq '.pipeline_results.faces_redacted // 0' "$visual_json")
        act_text=$(jq '.pipeline_results.text_regions_detected // 0' "$visual_json")
        act_nsfw=$(jq '.pipeline_results.nsfw_blocked // false' "$visual_json")
        act_screenshot=$(jq '.pipeline_results.screenshot_detected // false' "$visual_json")

        if [[ "$act_faces" -ne "$exp_faces" ]]; then
          fail "$key" "faces_redacted: got $act_faces, expected $exp_faces"
          continue
        fi

        if [[ "$act_text" -ne "$exp_text" ]]; then
          fail "$key" "text_regions: got $act_text, expected $exp_text"
          continue
        fi

        if [[ "$act_nsfw" != "$exp_nsfw" ]]; then
          fail "$key" "nsfw_blocked: got $act_nsfw, expected $exp_nsfw"
          continue
        fi

        if [[ "$act_screenshot" != "$exp_screenshot" ]]; then
          fail "$key" "screenshot_detected: got $act_screenshot, expected $exp_screenshot"
          continue
        fi

        detail="faces:$act_faces text:$act_text"
        [[ "$act_nsfw" == "true" ]] && detail="${detail} nsfw:YES"
        [[ "$act_screenshot" == "true" ]] && detail="${detail} screenshot:YES"
        pass "$key" "${detail} (exact)"
      done
    fi
  fi

# ═══════════════════════════════════════════════════════════════
# THRESHOLD MODE: min_matches + expected_types + must_detect
# ═══════════════════════════════════════════════════════════════

else

  MANIFEST_KEYS=$(jq -r '.files | keys[]' "$MANIFEST")

  # Group by category for display
  CURRENT_CAT=""

  for key in $MANIFEST_KEYS; do
    TOTAL=$((TOTAL + 1))

    category="${key%%/*}"
    filename="${key#*/}"
    name_no_ext="${filename%.*}"

    # Print category header
    if [[ "$category" != "$CURRENT_CAT" ]]; then
      CURRENT_CAT="$category"
      if [[ "$SUMMARY_ONLY" == "false" && "$JSON_OUTPUT" == "false" ]]; then
        echo "--- $category ---"
      fi
    fi

    # Read expected values from manifest
    min_matches=$(jq -r ".files[\"$key\"].min_matches" "$MANIFEST")
    expected_types_json=$(jq -c ".files[\"$key\"].expected_types // []" "$MANIFEST")

    # ── Gateway JSON check ──

    gw_json="$OUTPUT_DIR/$category/json/${name_no_ext}_gateway.json"

    if [[ ! -f "$gw_json" ]]; then
      # Maybe the test wasn't run for this category
      skip "$key" "no gateway JSON (tests not run?)"
      continue
    fi

    # ── Check: total_matches >= min_matches ──

    actual_matches=$(jq '.total_matches // 0' "$gw_json")
    if [[ "$actual_matches" -lt "$min_matches" ]]; then
      fail "$key" "matches: $actual_matches < min $min_matches"
      continue
    fi

    # ── Check: expected types present ──

    types_ok=true
    missing_type=""
    for expected_type in $(echo "$expected_types_json" | jq -r '.[]'); do
      has_type=$(jq -r ".type_summary[\"$expected_type\"] // 0" "$gw_json")
      if [[ "$has_type" -eq 0 ]]; then
        # Type not found — check case-insensitive variants
        # The proxy may use CamelCase or snake_case
        alt_found=false
        for alt in $(jq -r '.type_summary | keys[]' "$gw_json" 2>/dev/null); do
          alt_lower=$(echo "$alt" | tr '[:upper:]' '[:lower:]' | tr '-' '_')
          expected_lower=$(echo "$expected_type" | tr '[:upper:]' '[:lower:]' | tr '-' '_')
          if [[ "$alt_lower" == "$expected_lower" ]]; then
            alt_found=true
            break
          fi
        done
        if [[ "$alt_found" == "false" ]]; then
          types_ok=false
          missing_type="$expected_type"
          break
        fi
      fi
    done

    if [[ "$types_ok" == "false" ]]; then
      actual_types=$(jq -r '.type_summary | keys | join(", ")' "$gw_json" 2>/dev/null || echo "none")
      fail "$key" "missing type '$missing_type' (found: $actual_types)"
      continue
    fi

    # ── Check: must_detect PII strings ──

    must_detect_count=$(jq -r ".files[\"$key\"].must_detect // [] | length" "$MANIFEST")
    must_detect_ok=true
    must_detect_msg=""

    if [[ "$must_detect_count" -gt 0 ]]; then
      original_file="$INPUT_DIR/$key"

      if [[ -f "$original_file" ]]; then
        for idx in $(seq 0 $((must_detect_count - 1))); do
          md_text=$(jq -r ".files[\"$key\"].must_detect[$idx].text" "$MANIFEST")
          md_type=$(jq -r ".files[\"$key\"].must_detect[$idx].type" "$MANIFEST")

          # Find byte offset of text in original file using python3
          offset=$(python3 -c "
import sys
with open(sys.argv[1], 'r', errors='replace') as f:
    content = f.read()
idx = content.find(sys.argv[2])
print(idx)
" "$original_file" "$md_text" 2>/dev/null || echo "-1")

          if [[ "$offset" == "-1" ]]; then
            warn "$key" "must_detect text not found in input: '${md_text:0:30}...'"
            continue
          fi

          text_len=${#md_text}

          # Check if any match of the right type covers this text.
          # NER endpoint offsets may differ slightly from file offsets (JSON wrapping),
          # so use a tolerance window: match.start within ±10 of expected offset AND
          # match span length within ±4 of expected text length.
          covered=$(jq --argjson start "$offset" --argjson tlen "$text_len" --arg mtype "$md_type" \
            '[.matches[] | select(
              .type == $mtype and
              .start >= ($start - 10) and .start <= ($start + 10) and
              ((.end - .start) >= ($tlen - 4)) and ((.end - .start) <= ($tlen + 4))
            )] | length' \
            "$gw_json" 2>/dev/null || echo "0")

          if [[ "$covered" -eq 0 ]]; then
            must_detect_ok=false
            must_detect_msg="must_detect not covered: '${md_text:0:40}' ($md_type)"
            break
          fi
        done
      fi
    fi

    if [[ "$must_detect_ok" == "false" ]]; then
      fail "$key" "$must_detect_msg"
      continue
    fi

    # ── Check: FPE HTTP status (if present in JSON) ──

    fpe_http=$(jq -r '.fpe_http_status // "none"' "$gw_json")
    if [[ "$fpe_http" != "none" && "$fpe_http" != "200" ]]; then
      warn "$key" "FPE HTTP $fpe_http (expected 200)"
    fi

    # ── Check: Redacted file exists ──

    redacted_file="$OUTPUT_DIR/$category/redacted/$filename"
    if [[ ! -f "$redacted_file" ]]; then
      fail "$key" "no redacted file (FPE capture missing?)"
      continue
    fi

    # ── Check: Redacted differs from original ──

    original_file="$INPUT_DIR/$key"
    if [[ -f "$original_file" ]]; then
      if diff -q "$original_file" "$redacted_file" >/dev/null 2>&1; then
        fail "$key" "redacted IDENTICAL to original (echo server down? proxy not FPE-encrypting?)"
        continue
      fi
    fi

    # ── All checks passed ──

    type_list=$(jq -r '.type_summary | to_entries | map("\(.key):\(.value)") | join(", ")' "$gw_json" 2>/dev/null || echo "?")
    pass "$key" "$actual_matches matches ($type_list)"
  done

  # ─── Embedded validation (optional) ──────────────────────────

  if [[ "$GATEWAY_ONLY" == "false" && $EM_JSON_COUNT -gt 0 ]]; then
    if [[ "$JSON_OUTPUT" == "false" ]]; then
      echo ""
      echo "--- Embedded Results ---"
    fi

    EM_PASS=0
    EM_FAIL=0

    for key in $MANIFEST_KEYS; do
      category="${key%%/*}"
      filename="${key#*/}"
      name_no_ext="${filename%.*}"

      # Skip categories that embedded doesn't handle
      case "$category" in
        Visual_PII|Audio_PII) continue ;;
      esac

      em_json="$OUTPUT_DIR/$category/json/${name_no_ext}_embedded.json"
      [[ -f "$em_json" ]] || continue

      em_matches=$(jq '.total_matches // 0' "$em_json")
      min_matches=$(jq -r ".files[\"$key\"].min_matches" "$MANIFEST")

      # Use explicit embedded_min_matches if present, otherwise derive from gateway
      em_min_explicit=$(jq -r ".files[\"$key\"].embedded_min_matches // \"null\"" "$MANIFEST")
      if [[ "$em_min_explicit" != "null" ]]; then
        em_min=$em_min_explicit
      else
        # Check if embedded results include NER types (person/location/organization)
        has_ner=$(jq '[.type_summary.person // 0, .type_summary.location // 0, .type_summary.organization // 0] | add' "$em_json")
        if [[ "$has_ner" -gt 0 ]]; then
          # NER-capable embedded: use same thresholds as gateway
          em_min=$min_matches
        else
          # Regex-only embedded: use 30% of gateway threshold
          em_min=$(( min_matches * 3 / 10 ))
        fi
        [[ $em_min -lt 1 ]] && em_min=1
      fi

      if [[ "$em_matches" -ge "$em_min" ]]; then
        EM_PASS=$((EM_PASS + 1))
      else
        EM_FAIL=$((EM_FAIL + 1))
        if [[ "$SUMMARY_ONLY" == "false" && "$JSON_OUTPUT" == "false" ]]; then
          printf "  \033[31mFAIL\033[0m  %-50s  embedded: %d < min %d\n" "$key" "$em_matches" "$em_min"
        fi
      fi
    done

    if [[ "$JSON_OUTPUT" == "false" && "$SUMMARY_ONLY" == "false" ]]; then
      echo "  Embedded: $EM_PASS passed, $EM_FAIL failed"
    fi
  fi

  # ─── Threshold: Visual validation ──────────────────────────────

  VISUAL_KEYS=$(jq -r '.visual_files // {} | keys[]' "$MANIFEST" 2>/dev/null)

  if [[ -n "$VISUAL_KEYS" && "$GATEWAY_ONLY" == "false" ]]; then
    if [[ "$SUMMARY_ONLY" == "false" && "$JSON_OUTPUT" == "false" ]]; then
      echo ""
      echo "--- Visual_PII ---"
    fi

    CURRENT_VISUAL_CAT=""

    for vkey in $VISUAL_KEYS; do
      TOTAL=$((TOTAL + 1))

      filename="${vkey#Visual_PII/}"
      name_no_ext="${filename%.*}"
      visual_json="$OUTPUT_DIR/Visual_PII/json/${name_no_ext}_visual.json"

      subcategory=$(jq -r ".visual_files[\"$vkey\"].subcategory // \"unknown\"" "$MANIFEST")

      # Print subcategory header
      if [[ "$subcategory" != "$CURRENT_VISUAL_CAT" ]]; then
        CURRENT_VISUAL_CAT="$subcategory"
        if [[ "$SUMMARY_ONLY" == "false" && "$JSON_OUTPUT" == "false" ]]; then
          echo "  [$subcategory]"
        fi
      fi

      if [[ ! -f "$visual_json" ]]; then
        skip "$vkey" "no visual JSON (test not run?)"
        continue
      fi

      # Read expected thresholds
      min_faces=$(jq -r ".visual_files[\"$vkey\"].min_faces // 0" "$MANIFEST")
      min_text=$(jq -r ".visual_files[\"$vkey\"].min_text_regions // 0" "$MANIFEST")
      nsfw_exp=$(jq -r ".visual_files[\"$vkey\"].nsfw_expected // false" "$MANIFEST")
      screenshot_exp=$(jq -r ".visual_files[\"$vkey\"].screenshot_expected // false" "$MANIFEST")

      # Read actual results
      act_faces=$(jq '.pipeline_results.faces_redacted // 0' "$visual_json")
      act_text=$(jq '.pipeline_results.text_regions_detected // 0' "$visual_json")
      act_nsfw=$(jq '.pipeline_results.nsfw_blocked // false' "$visual_json")
      act_screenshot=$(jq '.pipeline_results.screenshot_detected // false' "$visual_json")

      # Check min_faces
      if [[ "$act_faces" -lt "$min_faces" ]]; then
        fail "$vkey" "faces: $act_faces < min $min_faces"
        continue
      fi

      # Check min_text_regions
      if [[ "$act_text" -lt "$min_text" ]]; then
        fail "$vkey" "text_regions: $act_text < min $min_text"
        continue
      fi

      # Check nsfw_expected
      if [[ "$nsfw_exp" == "true" && "$act_nsfw" != "true" ]]; then
        fail "$vkey" "nsfw: expected true, got $act_nsfw"
        continue
      fi
      if [[ "$nsfw_exp" == "false" && "$act_nsfw" == "true" ]]; then
        fail "$vkey" "nsfw: expected false, got true (false positive)"
        continue
      fi

      # Check screenshot_expected
      if [[ "$screenshot_exp" == "true" && "$act_screenshot" != "true" ]]; then
        fail "$vkey" "screenshot: expected true, got $act_screenshot"
        continue
      fi
      if [[ "$screenshot_exp" == "false" && "$act_screenshot" == "true" ]]; then
        fail "$vkey" "screenshot: expected false, got true (false positive)"
        continue
      fi

      detail="faces:$act_faces text:$act_text"
      [[ "$act_nsfw" == "true" ]] && detail="${detail} nsfw:YES"
      [[ "$act_screenshot" == "true" ]] && detail="${detail} screenshot:YES"
      pass "$vkey" "$detail"
    done
  fi

  # ─── Threshold: Audio validation ──────────────────────────────

  AUDIO_KEYS=$(jq -r '.audio_files // {} | keys[]' "$MANIFEST" 2>/dev/null)

  if [[ -n "$AUDIO_KEYS" && "$GATEWAY_ONLY" == "false" ]]; then
    if [[ "$SUMMARY_ONLY" == "false" && "$JSON_OUTPUT" == "false" ]]; then
      echo ""
      echo "--- Audio_PII ---"
    fi

    for akey in $AUDIO_KEYS; do
      TOTAL=$((TOTAL + 1))

      filename="${akey#Audio_PII/}"
      ext="${filename##*.}"
      name_no_ext="${filename%.*}"
      audio_json="$OUTPUT_DIR/Audio_PII/json/${name_no_ext}_${ext}_audio.json"

      if [[ ! -f "$audio_json" ]]; then
        skip "$akey" "no audio JSON (test not run or voice feature disabled?)"
        continue
      fi

      # Read expected values
      exp_action=$(jq -r ".audio_files[\"$akey\"].expected_action // \"UNKNOWN\"" "$MANIFEST")
      exp_keywords_json=$(jq -c ".audio_files[\"$akey\"].expected_keywords // []" "$MANIFEST")

      # Read actual values
      act_action=$(jq -r '.kws_results.action // "UNKNOWN"' "$audio_json")
      act_keywords=$(jq -r '.kws_results.keywords // ""' "$audio_json")
      act_voice_ms=$(jq '.timing.voice_ms // 0' "$audio_json")

      # Check action matches
      if [[ "$act_action" == "PASS-THRU" ]]; then
        warn "$akey" "pass-through (voice feature not enabled or KWS not loaded)"
        continue
      fi

      if [[ "$act_action" != "$exp_action" ]]; then
        fail "$akey" "action: got $act_action, expected $exp_action"
        continue
      fi

      # For PII_DETECTED: verify each expected keyword appears in actual keywords
      if [[ "$exp_action" == "PII_DETECTED" ]]; then
        keyword_miss=false
        while IFS= read -r kw; do
          if [[ "$act_keywords" != *"$kw"* ]]; then
            fail "$akey" "missing keyword: $kw (got: $act_keywords)"
            keyword_miss=true
            break
          fi
        done < <(echo "$exp_keywords_json" | jq -r '.[]')

        if [[ "$keyword_miss" == "true" ]]; then
          continue
        fi
      fi

      detail="action:$act_action voice:${act_voice_ms}ms"
      [[ -n "$act_keywords" ]] && detail="${detail} kw:$act_keywords"
      pass "$akey" "$detail"
    done
  fi

fi  # end threshold/strict/infrastructure-only branching

# ═══════════════════════════════════════════════════════════════
# REDACTED CONTENT VALIDATION (--check-redacted)
# ═══════════════════════════════════════════════════════════════

if [[ "$CHECK_REDACTED" == "true" && -f "$MANIFEST" ]]; then

  if [[ "$JSON_OUTPUT" == "false" ]]; then
    echo ""
    echo "--- Redacted Content Validation ---"
  fi

  REDACT_PASS=0
  REDACT_FAIL=0

  # Check must_not_contain: specific PII strings that must NOT appear in redacted output
  for key in $(jq -r '.files | to_entries[] | select(.value.must_not_contain != null) | .key' "$MANIFEST"); do
    category="${key%%/*}"
    filename="${key#*/}"
    redacted_file="$OUTPUT_DIR/$category/redacted/$filename"

    if [[ ! -f "$redacted_file" ]]; then
      continue
    fi

    mnc_count=$(jq -r ".files[\"$key\"].must_not_contain | length" "$MANIFEST")
    file_ok=true

    for idx in $(seq 0 $((mnc_count - 1))); do
      pii_string=$(jq -r ".files[\"$key\"].must_not_contain[$idx]" "$MANIFEST")
      if grep -cF "$pii_string" "$redacted_file" >/dev/null 2>&1; then
        hit_count=$(grep -cF "$pii_string" "$redacted_file")
        fail "$key" "must_not_contain LEAKED: \"$pii_string\" found $hit_count time(s) in redacted"
        REDACT_FAIL=$((REDACT_FAIL + 1))
        file_ok=false
        break
      fi
    done

    if [[ "$file_ok" == "true" ]]; then
      REDACT_PASS=$((REDACT_PASS + 1))
      pass "$key" "must_not_contain: all $mnc_count strings redacted"
    fi
  done

  # Check placeholder presence: if gateway JSON has NER types, redacted file should have placeholders
  for key in $(jq -r '.files | keys[]' "$MANIFEST"); do
    category="${key%%/*}"
    filename="${key#*/}"
    name_no_ext="${filename%.*}"

    gw_json="$OUTPUT_DIR/$category/json/${name_no_ext}_gateway.json"
    redacted_file="$OUTPUT_DIR/$category/redacted/$filename"

    [[ -f "$gw_json" && -f "$redacted_file" ]] || continue

    # Check person → [PERSON_
    has_person=$(jq -r '.type_summary.person // 0' "$gw_json")
    if [[ "$has_person" -gt 0 ]]; then
      if ! grep -qF "[PERSON_" "$redacted_file" 2>/dev/null; then
        warn "$key" "gateway has $has_person person matches but redacted has no [PERSON_ placeholders"
      fi
    fi

    # Check organization → [ORG_
    has_org=$(jq -r '.type_summary.organization // 0' "$gw_json")
    if [[ "$has_org" -gt 0 ]]; then
      if ! grep -qF "[ORG_" "$redacted_file" 2>/dev/null; then
        warn "$key" "gateway has $has_org organization matches but redacted has no [ORG_ placeholders"
      fi
    fi

    # Check location → [LOCATION_
    has_loc=$(jq -r '.type_summary.location // 0' "$gw_json")
    if [[ "$has_loc" -gt 0 ]]; then
      if ! grep -qF "[LOCATION_" "$redacted_file" 2>/dev/null; then
        warn "$key" "gateway has $has_loc location matches but redacted has no [LOCATION_ placeholders"
      fi
    fi
  done

  if [[ "$JSON_OUTPUT" == "false" ]]; then
    echo "  Redacted check: $REDACT_PASS passed, $REDACT_FAIL failed"
  fi

  # ─── EXIF verification ──────────────────────────────────────

  EXIF_PASS=0
  EXIF_FAIL=0

  if [[ "$JSON_OUTPUT" == "false" && "$SUMMARY_ONLY" == "false" ]]; then
    echo ""
    echo "  [EXIF Stripping]"
  fi

  for vkey in $(jq -r '.visual_files | to_entries[] | select(.value.input_exif_tags_min != null) | .key' "$MANIFEST" 2>/dev/null); do
    TOTAL=$((TOTAL + 1))
    filename="${vkey#Visual_PII/}"
    input_file="$INPUT_DIR/Visual_PII/EXIF/$filename"
    name_no_ext="${filename%.*}"
    output_file="$OUTPUT_DIR/Visual_PII/redacted/$filename"

    min_input_tags=$(jq -r ".visual_files[\"$vkey\"].input_exif_tags_min" "$MANIFEST")
    max_output_tags=$(jq -r ".visual_files[\"$vkey\"].output_exif_tags_max // 0" "$MANIFEST")
    must_strip_gps=$(jq -r ".visual_files[\"$vkey\"].must_strip_gps // false" "$MANIFEST")

    if [[ ! -f "$input_file" ]]; then
      skip "$vkey" "no input file"
      continue
    fi

    # Count EXIF tags in input
    input_tags=$(python3 -c "
import sys
try:
    import piexif
    d = piexif.load(sys.argv[1])
    count = sum(len(ifd) for ifd in [d.get('0th',{}), d.get('Exif',{}), d.get('GPS',{})])
    print(count)
except Exception:
    print(0)
" "$input_file" 2>/dev/null)

    if [[ "$input_tags" -lt "$min_input_tags" ]]; then
      fail "$vkey" "input has $input_tags EXIF tags < min $min_input_tags (regenerate test images?)"
      EXIF_FAIL=$((EXIF_FAIL + 1))
      continue
    fi

    if [[ ! -f "$output_file" ]]; then
      skip "$vkey" "no output file (test not run?)"
      continue
    fi

    # Count EXIF tags in output
    output_tags=$(python3 -c "
import sys
try:
    import piexif
    d = piexif.load(sys.argv[1])
    count = sum(len(ifd) for ifd in [d.get('0th',{}), d.get('Exif',{}), d.get('GPS',{})])
    print(count)
except Exception:
    print(0)
" "$output_file" 2>/dev/null)

    if [[ "$output_tags" -gt "$max_output_tags" ]]; then
      fail "$vkey" "output has $output_tags EXIF tags > max $max_output_tags"
      EXIF_FAIL=$((EXIF_FAIL + 1))
      continue
    fi

    # GPS check
    if [[ "$must_strip_gps" == "true" ]]; then
      output_gps=$(python3 -c "
import sys
try:
    import piexif
    d = piexif.load(sys.argv[1])
    print(len(d.get('GPS',{})))
except Exception:
    print(0)
" "$output_file" 2>/dev/null)

      if [[ "$output_gps" -gt 0 ]]; then
        fail "$vkey" "output still has $output_gps GPS tags (not stripped!)"
        EXIF_FAIL=$((EXIF_FAIL + 1))
        continue
      fi
    fi

    EXIF_PASS=$((EXIF_PASS + 1))
    pass "$vkey" "input:${input_tags}tags output:${output_tags}tags"
  done

  if [[ "$JSON_OUTPUT" == "false" ]]; then
    echo "  EXIF stripping: $EXIF_PASS passed, $EXIF_FAIL failed"
  fi
fi

# ═══════════════════════════════════════════════════════════════
# INFRASTRUCTURE TEST RESULTS (--infrastructure)
# ═══════════════════════════════════════════════════════════════

if [[ "$INFRASTRUCTURE" == "true" ]]; then

  INFRA_TOTAL=0
  INFRA_PASS=0
  INFRA_FAIL=0
  INFRA_WARN=0
  INFRA_SKIP=0
  INFRA_FOUND=0

  if [[ "$JSON_OUTPUT" == "false" ]]; then
    echo ""
    echo "--- Infrastructure Test Results ---"
  fi

  while IFS= read -r vfile; do
    [[ -f "$vfile" ]] || continue
    INFRA_FOUND=$((INFRA_FOUND + 1))

    dir_name=$(basename "$(dirname "$vfile")")

    # Schema validation: skip file if malformed
    if ! validate_schema "$vfile" > /dev/null 2>&1; then
      INFRA_FAIL=$((INFRA_FAIL + 1))
      FAIL=$((FAIL + 1))
      TOTAL=$((TOTAL + 1))
      INFRA_TOTAL=$((INFRA_TOTAL + 1))
      FAILURES+=("$dir_name: schema validation failed for $(basename "$vfile")")
      if [[ "$SUMMARY_ONLY" == "false" && "$JSON_OUTPUT" == "false" ]]; then
        printf "  \033[31mFAIL\033[0m  %-45s  %s\n" "$dir_name" "schema validation failed: $(basename "$vfile")"
      fi
      continue
    fi

    suite_name=$(jq -r '.test_suite // "unknown"' "$vfile")
    suite_pass=$(jq '.pass // 0' "$vfile")
    suite_fail=$(jq '.fail // 0' "$vfile")
    suite_warn=$(jq '.warn // 0' "$vfile")
    suite_skip=$(jq '.skip // 0' "$vfile")
    suite_total=$(jq '.total // 0' "$vfile")

    INFRA_PASS=$((INFRA_PASS + suite_pass))
    INFRA_FAIL=$((INFRA_FAIL + suite_fail))
    INFRA_WARN=$((INFRA_WARN + suite_warn))
    INFRA_SKIP=$((INFRA_SKIP + suite_skip))
    INFRA_TOTAL=$((INFRA_TOTAL + suite_total))

    # Add to global counters
    PASS=$((PASS + suite_pass))
    FAIL=$((FAIL + suite_fail))
    WARN=$((WARN + suite_warn))
    SKIP=$((SKIP + suite_skip))
    TOTAL=$((TOTAL + suite_total))

    if [[ "$SUMMARY_ONLY" == "false" && "$JSON_OUTPUT" == "false" ]]; then
      if [[ "$suite_fail" -eq 0 ]]; then
        printf "  \033[32mPASS\033[0m  %-45s  %d/%d passed" "$suite_name" "$suite_pass" "$suite_total"
      else
        printf "  \033[31mFAIL\033[0m  %-45s  %d/%d passed, %d failed" "$suite_name" "$suite_pass" "$suite_total" "$suite_fail"
      fi
      [[ "$suite_warn" -gt 0 ]] && printf ", %d warnings" "$suite_warn"
      echo ""
    fi

    # Add individual failures to the FAILURES array
    if [[ "$suite_fail" -gt 0 ]]; then
      while IFS= read -r result_line; do
        result_name=$(echo "$result_line" | jq -r '.name')
        result_detail=$(echo "$result_line" | jq -r '.detail')
        FAILURES+=("$suite_name/$result_name: $result_detail")
      done < <(jq -c '.results[] | select(.status == "fail")' "$vfile" 2>/dev/null)
    fi
  done < <(find "$OUTPUT_DIR" -name "*_validation.json" -type f 2>/dev/null | sort)

  if [[ "$JSON_OUTPUT" == "false" ]]; then
    if [[ $INFRA_FOUND -eq 0 ]]; then
      echo "  No infrastructure validation files found."
      echo "  Run infrastructure test scripts first (test_health.sh, test_auth.sh, etc.)"
    else
      echo ""
      printf "  Infrastructure totals: %d passed, %d failed" "$INFRA_PASS" "$INFRA_FAIL"
      [[ $INFRA_WARN -gt 0 ]] && printf ", %d warnings" "$INFRA_WARN"
      [[ $INFRA_SKIP -gt 0 ]] && printf ", %d skipped" "$INFRA_SKIP"
      printf " (%d suites)\n" "$INFRA_FOUND"
    fi
  fi
fi

# ─── Coverage check: input files not in manifest ─────────────

UNCOVERED=0

if [[ $GW_JSON_COUNT -gt 0 || $EM_JSON_COUNT -gt 0 ]]; then

if [[ "$JSON_OUTPUT" == "false" ]]; then
  echo ""
  echo "--- Coverage Check ---"
fi

COVERED_CATEGORIES=(
  "PII_Detection"
  "Multilingual_PII"
  "Code_Config_PII"
  "Structured_Data_PII"
  "Agent_Tool_Results"
)

for cat_name in "${COVERED_CATEGORIES[@]}"; do
  cat_dir="$INPUT_DIR/$cat_name"
  [[ -d "$cat_dir" ]] || continue

  for input_file in "$cat_dir"/*; do
    [[ -f "$input_file" ]] || continue
    fname=$(basename "$input_file")

    # Skip non-testable files
    ext="${fname##*.}"
    case "$ext" in
      txt|csv|tsv|env|py|yaml|yml|json|sh|md|log) ;;
      *) continue ;;
    esac

    manifest_key="${cat_name}/${fname}"
    has_entry=$(jq -r ".files[\"$manifest_key\"] // \"missing\"" "$MANIFEST")
    if [[ "$has_entry" == "missing" ]]; then
      UNCOVERED=$((UNCOVERED + 1))
      if [[ "$SUMMARY_ONLY" == "false" && "$JSON_OUTPUT" == "false" ]]; then
        printf "  \033[33mNO MANIFEST\033[0m  %s\n" "$manifest_key"
      fi
    fi
  done
done

if [[ "$JSON_OUTPUT" == "false" ]]; then
  if [[ $UNCOVERED -eq 0 ]]; then
    echo "  All text input files have manifest entries."
  else
    echo "  $UNCOVERED input file(s) missing from expected_results.json"
  fi
fi

# Visual coverage check
VISUAL_UNCOVERED=0
VISUAL_SUBCATS=("Faces" "Screenshots" "Documents" "EXIF" "NSFW")
for subcat in "${VISUAL_SUBCATS[@]}"; do
  subcat_dir="$INPUT_DIR/Visual_PII/$subcat"
  [[ -d "$subcat_dir" ]] || continue
  for input_file in "$subcat_dir"/*; do
    [[ -f "$input_file" ]] || continue
    fname=$(basename "$input_file")
    ext="${fname##*.}"
    case "$ext" in
      jpg|jpeg|png|gif|webp) ;;
      *) continue ;;
    esac
    manifest_key="Visual_PII/${fname}"
    has_entry=$(jq -r ".visual_files[\"$manifest_key\"] // \"missing\"" "$MANIFEST" 2>/dev/null)
    if [[ "$has_entry" == "missing" ]]; then
      VISUAL_UNCOVERED=$((VISUAL_UNCOVERED + 1))
      if [[ "$SUMMARY_ONLY" == "false" && "$JSON_OUTPUT" == "false" ]]; then
        printf "  \033[33mNO MANIFEST\033[0m  %s\n" "$manifest_key"
      fi
    fi
  done
done

if [[ "$JSON_OUTPUT" == "false" ]]; then
  if [[ $VISUAL_UNCOVERED -eq 0 ]]; then
    echo "  All visual input files have manifest entries."
  else
    echo "  $VISUAL_UNCOVERED visual file(s) missing from expected_results.json"
  fi
fi

fi  # end coverage check guard (PII results exist)

# ─── Type breakdown ──────────────────────────────────────────

if [[ "$JSON_OUTPUT" == "false" && "$SUMMARY_ONLY" == "false" ]]; then
  echo ""
  echo "--- Gateway Type Totals ---"
  for gw_file in $(find "$OUTPUT_DIR" -path "*/json/*_gateway.json" 2>/dev/null | sort); do
    jq -r '.type_summary // {} | to_entries[] | "\(.key)\t\(.value)"' "$gw_file" 2>/dev/null
  done | sort | awk -F'\t' '{counts[$1]+=$2} END {for(t in counts) printf "  %-20s %d\n", t, counts[t]}' | sort -t' ' -k2 -nr
fi

# ─── Timing Statistics ────────────────────────────────────────

if [[ "$JSON_OUTPUT" == "false" && "$SUMMARY_ONLY" == "false" ]]; then
  _has_timing=false

  # Collect gateway timing
  gw_total_ms=(); gw_scan_us=(); gw_fpe_us=()
  for gw_file in $(find "$OUTPUT_DIR" -path "*/json/*_gateway.json" 2>/dev/null | sort); do
    v=$(jq '.timing.total_ms // empty' "$gw_file" 2>/dev/null)
    [[ -n "$v" && "$v" != "0" && "$v" != "null" ]] && gw_total_ms+=("$v")
    v=$(jq '.timing.proxy_scan_us // empty' "$gw_file" 2>/dev/null)
    [[ -n "$v" && "$v" != "0" && "$v" != "null" ]] && gw_scan_us+=("$v")
    v=$(jq '.timing.proxy_fpe_us // empty' "$gw_file" 2>/dev/null)
    [[ -n "$v" && "$v" != "0" && "$v" != "null" ]] && gw_fpe_us+=("$v")
  done

  # Collect embedded timing
  em_total_ms=(); em_regex_ms=(); em_ner_ms=()
  for em_file in $(find "$OUTPUT_DIR" -path "*/json/*_embedded.json" 2>/dev/null | sort); do
    v=$(jq '.timing.total_ms // .elapsed_ms // empty' "$em_file" 2>/dev/null)
    [[ -n "$v" && "$v" != "0" && "$v" != "null" ]] && em_total_ms+=("$v")
    v=$(jq '.timing.regex_ms // empty' "$em_file" 2>/dev/null)
    [[ -n "$v" && "$v" != "0" && "$v" != "null" ]] && em_regex_ms+=("$v")
    v=$(jq '.timing.ner_ms // empty' "$em_file" 2>/dev/null)
    [[ -n "$v" && "$v" != "0" && "$v" != "null" ]] && em_ner_ms+=("$v")
  done

  # Collect visual timing
  vis_pipeline_ms=(); vis_nsfw_ms=(); vis_face_ms=(); vis_ocr_ms=()
  for vis_file in $(find "$OUTPUT_DIR" -path "*/json/*_visual.json" 2>/dev/null | sort); do
    v=$(jq '.timing.pipeline_ms // empty' "$vis_file" 2>/dev/null)
    [[ -n "$v" && "$v" != "0" && "$v" != "null" ]] && vis_pipeline_ms+=("$v")
    v=$(jq '.timing.nsfw_ms // empty' "$vis_file" 2>/dev/null)
    [[ -n "$v" && "$v" != "0" && "$v" != "null" ]] && vis_nsfw_ms+=("$v")
    v=$(jq '.timing.face_ms // empty' "$vis_file" 2>/dev/null)
    [[ -n "$v" && "$v" != "0" && "$v" != "null" ]] && vis_face_ms+=("$v")
    v=$(jq '.timing.ocr_ms // empty' "$vis_file" 2>/dev/null)
    [[ -n "$v" && "$v" != "0" && "$v" != "null" ]] && vis_ocr_ms+=("$v")
  done

  # Collect audio timing
  aud_pipeline_ms=(); aud_voice_ms=(); aud_kws_ms=()
  for aud_file in $(find "$OUTPUT_DIR" -path "*/json/*_audio.json" 2>/dev/null | sort); do
    v=$(jq '.timing.pipeline_ms // empty' "$aud_file" 2>/dev/null)
    [[ -n "$v" && "$v" != "0" && "$v" != "null" ]] && aud_pipeline_ms+=("$v")
    v=$(jq '.timing.voice_ms // empty' "$aud_file" 2>/dev/null)
    [[ -n "$v" && "$v" != "0" && "$v" != "null" ]] && aud_voice_ms+=("$v")
    v=$(jq '.timing.kws_ms // empty' "$aud_file" 2>/dev/null)
    [[ -n "$v" && "$v" != "0" && "$v" != "null" ]] && aud_kws_ms+=("$v")
  done

  # Print if any timing data exists
  total_timing_count=$(( ${#gw_total_ms[@]} + ${#em_total_ms[@]} + ${#vis_pipeline_ms[@]} + ${#aud_pipeline_ms[@]} ))
  if [[ $total_timing_count -gt 0 ]]; then
    echo ""
    echo "--- Timing Statistics ---"
    printf "  %-25s %5s  %8s  %8s  %8s  %8s  %8s\n" "Metric" "Count" "Min" "Avg" "P50" "P95" "Max"
    printf "  %-25s %5s  %8s  %8s  %8s  %8s  %8s\n" "─────────────────────────" "─────" "────────" "────────" "────────" "────────" "────────"

    print_timing_stats "Gateway total (ms)"     "${gw_total_ms[@]+"${gw_total_ms[@]}"}"
    print_timing_stats "Gateway proxy scan (us)" "${gw_scan_us[@]+"${gw_scan_us[@]}"}"
    print_timing_stats "Gateway proxy FPE (us)"  "${gw_fpe_us[@]+"${gw_fpe_us[@]}"}"
    print_timing_stats "Embedded total (ms)"     "${em_total_ms[@]+"${em_total_ms[@]}"}"
    print_timing_stats "Embedded regex (ms)"     "${em_regex_ms[@]+"${em_regex_ms[@]}"}"
    print_timing_stats "Embedded NER (ms)"       "${em_ner_ms[@]+"${em_ner_ms[@]}"}"
    print_timing_stats "Visual pipeline (ms)"    "${vis_pipeline_ms[@]+"${vis_pipeline_ms[@]}"}"
    print_timing_stats "Visual NSFW (ms)"        "${vis_nsfw_ms[@]+"${vis_nsfw_ms[@]}"}"
    print_timing_stats "Visual face (ms)"        "${vis_face_ms[@]+"${vis_face_ms[@]}"}"
    print_timing_stats "Visual OCR (ms)"         "${vis_ocr_ms[@]+"${vis_ocr_ms[@]}"}"
    print_timing_stats "Audio pipeline (ms)"     "${aud_pipeline_ms[@]+"${aud_pipeline_ms[@]}"}"
    print_timing_stats "Audio voice (ms)"        "${aud_voice_ms[@]+"${aud_voice_ms[@]}"}"
    print_timing_stats "Audio KWS (ms)"          "${aud_kws_ms[@]+"${aud_kws_ms[@]}"}"
  fi
fi

# ─── Summary ─────────────────────────────────────────────────

if [[ "$JSON_OUTPUT" == "true" ]]; then
  # JSON report
  failures_json="[]"
  if [[ ${#FAILURES[@]} -gt 0 ]]; then
    failures_json=$(printf '%s\n' "${FAILURES[@]}" | jq -R -s 'split("\n") | map(select(. != ""))')
  fi
  warnings_json="[]"
  if [[ ${#WARNINGS[@]} -gt 0 ]]; then
    warnings_json=$(printf '%s\n' "${WARNINGS[@]}" | jq -R -s 'split("\n") | map(select(. != ""))')
  fi

  jq -n \
    --argjson pass "$PASS" \
    --argjson fail "$FAIL" \
    --argjson warn "$WARN" \
    --argjson skip "$SKIP" \
    --argjson total "$TOTAL" \
    --argjson uncovered "$UNCOVERED" \
    --argjson failures "$failures_json" \
    --argjson warnings "$warnings_json" \
    --arg ts "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    --arg mode "$MODE_LABEL" \
    '{
      timestamp: $ts,
      mode: $mode,
      result: (if $fail > 0 then "FAIL" else "PASS" end),
      pass: $pass,
      fail: $fail,
      warn: $warn,
      skip: $skip,
      total: $total,
      uncovered_files: $uncovered,
      failures: $failures,
      warnings: $warnings
    }'
else
  echo ""
  echo "============================================"
  if [[ $FAIL -eq 0 && $SKIP -eq 0 ]]; then
    printf "  Result: \033[32mALL PASS\033[0m (%s)\n" "$MODE_LABEL"
  elif [[ $FAIL -eq 0 ]]; then
    printf "  Result: \033[32mPASS\033[0m (%s, %d skipped)\n" "$MODE_LABEL" "$SKIP"
  else
    printf "  Result: \033[31mFAIL\033[0m (%s)\n" "$MODE_LABEL"
  fi
  echo ""
  printf "  Passed:    %d / %d\n" "$PASS" "$TOTAL"
  printf "  Failed:    %d / %d\n" "$FAIL" "$TOTAL"
  [[ $WARN -gt 0 ]] && printf "  Warnings:  %d\n" "$WARN"
  [[ $SKIP -gt 0 ]] && printf "  Skipped:   %d (no output files)\n" "$SKIP"
  [[ $UNCOVERED -gt 0 ]] && printf "  Uncovered: %d input files not in manifest\n" "$UNCOVERED"
  echo ""

  if [[ $FAIL -gt 0 ]]; then
    echo "  Failed files:"
    for f in "${FAILURES[@]}"; do
      echo "    - $f"
    done
    echo ""
  fi

  echo "============================================"
fi

# Exit code: 0 = pass, 1 = fail, 2 = no results
if [[ $FAIL -gt 0 ]]; then
  exit 1
fi
exit 0
