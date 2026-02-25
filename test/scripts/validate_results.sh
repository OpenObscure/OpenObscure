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
for arg in "$@"; do
  case "$arg" in
    --gateway-only) GATEWAY_ONLY=true ;;
    --summary) SUMMARY_ONLY=true ;;
    --json) JSON_OUTPUT=true ;;
    --strict) STRICT=true ;;
    --check-redacted) CHECK_REDACTED=true ;;
  esac
done

# Verify required files exist
if [[ "$STRICT" == "true" ]]; then
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
if [[ "$STRICT" == "true" ]]; then
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

if [[ $GW_JSON_COUNT -eq 0 && $EM_JSON_COUNT -eq 0 ]]; then
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

if [[ "$STRICT" == "true" ]]; then

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
      # Embedded regex-only: expect at least 1 match for files with regex-detectable types
      # (NER types like person/location/org won't be found without USE_NER=1)
      min_matches=$(jq -r ".files[\"$key\"].min_matches" "$MANIFEST")

      # Embedded detects fewer types, so use 30% of gateway threshold
      em_min=$(( min_matches * 3 / 10 ))
      [[ $em_min -lt 1 ]] && em_min=1

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

fi  # end threshold/strict branching

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

# ─── Coverage check: input files not in manifest ─────────────

if [[ "$JSON_OUTPUT" == "false" ]]; then
  echo ""
  echo "--- Coverage Check ---"
fi

UNCOVERED=0
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

# ─── Type breakdown ──────────────────────────────────────────

if [[ "$JSON_OUTPUT" == "false" && "$SUMMARY_ONLY" == "false" ]]; then
  echo ""
  echo "--- Gateway Type Totals ---"
  for gw_file in $(find "$OUTPUT_DIR" -path "*/json/*_gateway.json" 2>/dev/null | sort); do
    jq -r '.type_summary // {} | to_entries[] | "\(.key)\t\(.value)"' "$gw_file" 2>/dev/null
  done | sort | awk -F'\t' '{counts[$1]+=$2} END {for(t in counts) printf "  %-20s %d\n", t, counts[t]}' | sort -t' ' -k2 -nr
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
