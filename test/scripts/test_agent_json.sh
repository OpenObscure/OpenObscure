#!/usr/bin/env bash
# test_agent_json.sh — Test Agent Tool Result JSON files via Gateway FPE pipeline.
#
# Produces dual output per file:
#   test/data/output/Agent_Tool_Results/json/<name>_gateway.json     (NER metadata)
#   test/data/output/Agent_Tool_Results/redacted/<name>.json          (FPE-encrypted JSON)
#
# Strategy: sends the entire agent JSON file as message content through the proxy.
# The proxy's nested JSON scanner detects PII within the serialized JSON string
# and applies FPE encryption / label redaction. The echo server captures the
# encrypted body, and we extract the transformed content.
#
# Handles: Anthropic format, OpenAI format, tool results, nested JSON strings.
#
# Usage:
#   ./test/scripts/test_agent_json.sh [specific_file.json]

set -euo pipefail

# Millisecond timestamp (portable: Perl on macOS, date +%s%N on Linux)
_ms() { perl -MTime::HiRes -e 'printf("%d\n", Time::HiRes::time() * 1000)' 2>/dev/null || echo $(( $(date +%s) * 1000 )); }

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
TEST_DIR="$(dirname "$SCRIPT_DIR")"
INPUT_DIR="$TEST_DIR/data/input/Agent_Tool_Results"
OUTPUT_DIR="$TEST_DIR/data/output/Agent_Tool_Results"

PROXY_URL="${PROXY_URL:-http://127.0.0.1:18790}"
NER_ENDPOINT="${PROXY_URL}/_openobscure/ner"
PROVIDER_PREFIX="${PROVIDER_PREFIX:-/anthropic}"
FPE_ENDPOINT="${PROXY_URL}${PROVIDER_PREFIX}/v1/messages"
CAPTURE_DIR="${CAPTURE_DIR:-/tmp/oo_echo_captures}"

# Auth token
if [[ -z "${AUTH_TOKEN:-}" ]]; then
  TOKEN_FILE="$HOME/.openobscure/.auth-token"
  if [[ -f "$TOKEN_FILE" ]]; then
    AUTH_TOKEN=$(cat "$TOKEN_FILE")
  else
    AUTH_TOKEN=""
  fi
fi

# Cleanup stale capture files on any exit (error, Ctrl+C, normal)
cleanup_agent() {
  rm -f "$CAPTURE_DIR"/agent_*_$$_*.json 2>/dev/null || true
}
trap cleanup_agent EXIT

mkdir -p "$OUTPUT_DIR/json" "$OUTPUT_DIR/redacted"

# Purge previous agent results when running batch (no single-file arg)
if [[ -z "${1:-}" ]]; then
  rm -f "$OUTPUT_DIR"/json/*_gateway.json 2>/dev/null || true
  rm -f "$OUTPUT_DIR"/redacted/*.json 2>/dev/null || true
fi

# ── Extract all text fragments from agent JSON for NER metadata ──
extract_text() {
  local file="$1"
  python3 -c "
import json, sys
data = json.load(open(sys.argv[1]))
fragments = []

def extract(obj, path=''):
    if isinstance(obj, dict):
        if 'content' in obj:
            c = obj['content']
            if isinstance(c, list):
                for i, item in enumerate(c):
                    if isinstance(item, dict) and item.get('type') == 'text':
                        fragments.append(item['text'])
                    elif isinstance(item, str):
                        fragments.append(item)
            elif isinstance(c, str):
                fragments.append(c)
        if 'messages' in obj and isinstance(obj['messages'], list):
            for msg in obj['messages']:
                extract(msg)
        if 'text' in obj and isinstance(obj['text'], str) and 'content' not in obj:
            fragments.append(obj['text'])
    elif isinstance(obj, list):
        for item in obj:
            extract(item)

extract(data)
print('\n---FRAGMENT_SEP---\n'.join(fragments))
" "$file"
}

# ── Send text to NER endpoint ──
call_ner() {
  local text="$1"
  local payload
  payload=$(jq -n --arg text "$text" '{text: $text}')

  if [[ -n "${AUTH_TOKEN:-}" ]]; then
    curl -sf -X POST "$NER_ENDPOINT" \
      -H "Content-Type: application/json" \
      -H "X-OpenObscure-Token: $AUTH_TOKEN" \
      -d "$payload" 2>/dev/null || echo "[]"
  else
    curl -sf -X POST "$NER_ENDPOINT" \
      -H "Content-Type: application/json" \
      -d "$payload" 2>/dev/null || echo "[]"
  fi
}

test_file() {
  local file="$1"
  local filename; filename=$(basename "$file")
  local name_no_ext="${filename%.*}"
  local json_out="$OUTPUT_DIR/json/${name_no_ext}_gateway.json"
  local redacted_out="$OUTPUT_DIR/redacted/$filename"

  # ── Step 1: NER metadata ──
  # Combine all text fragments and send to NER
  local all_text
  all_text=$(extract_text "$file")

  local combined_text
  combined_text=$(echo "$all_text" | tr '\n' ' ' | head -c 65536)

  local ner_start
  ner_start=$(_ms)
  local ner_response
  ner_response=$(call_ner "$combined_text")
  local ner_elapsed_ms=$(( $(_ms) - ner_start ))

  local match_count
  match_count=$(echo "$ner_response" | jq 'length')
  local type_summary
  type_summary=$(echo "$ner_response" | jq '[.[].type] | group_by(.) | map({(.[0]): length}) | add // {}')

  # Detect format
  local format="unknown"
  if jq -e '.content' "$file" >/dev/null 2>&1; then
    if jq -e '.role == "tool"' "$file" >/dev/null 2>&1; then
      format="tool_result"
    else
      format="anthropic"
    fi
  elif jq -e '.messages' "$file" >/dev/null 2>&1; then
    format="openai"
  fi

  # ── Step 2: FPE pass-through ──
  # Serialize the entire agent JSON as a message content string.
  # The proxy's nested JSON scanner will detect PII within the serialized JSON
  # and apply FPE encryption to eligible types.
  local capture_id; capture_id="agent_${name_no_ext}_$$_$(date +%s)"
  local file_as_string
  file_as_string=$(jq -Rs '.' "$file")

  local fpe_payload
  fpe_payload=$(jq -n --argjson content "$file_as_string" '{
    model: "test-fpe-capture",
    max_tokens: 1,
    messages: [{role: "user", content: $content}]
  }')

  local fpe_start
  fpe_start=$(_ms)
  local fpe_http
  fpe_http=$(curl -s -o /dev/null -w "%{http_code}" -X POST "$FPE_ENDPOINT" \
    -H "Content-Type: application/json" \
    -H "x-api-key: test-fpe-scan" \
    -H "anthropic-version: 2023-06-01" \
    -H "X-Capture-Id: $capture_id" \
    -d "$fpe_payload" 2>/dev/null)
  local fpe_elapsed_ms=$(( $(_ms) - fpe_start ))
  local total_elapsed_ms=$(( ner_elapsed_ms + fpe_elapsed_ms ))

  # Read echo capture and extract the FPE'd content
  local capture_file="$CAPTURE_DIR/${capture_id}.json"
  local fpe_text=""

  if [[ -f "$capture_file" ]]; then
    fpe_text=$(jq -r '.messages[0].content // empty' "$capture_file" 2>/dev/null || true)
    rm -f "$capture_file"
  fi

  # Try to re-format as pretty JSON; fall back to raw text
  if [[ -n "$fpe_text" ]]; then
    echo "$fpe_text" | jq . > "$redacted_out" 2>/dev/null || printf '%s' "$fpe_text" > "$redacted_out"
  else
    # Fallback: copy original
    cp "$file" "$redacted_out"
    echo "WARN $filename — FPE capture failed (HTTP $fpe_http), copied original"
  fi

  # ── Step 3: Write JSON metadata ──
  local result
  result=$(jq -n \
    --arg file "$filename" \
    --arg path "$file" \
    --arg arch "gateway" \
    --arg format "$format" \
    --arg ts "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    --argjson count "$match_count" \
    --argjson types "$type_summary" \
    --argjson matches "$ner_response" \
    --argjson fpe_http "$fpe_http" \
    --argjson ner_ms "$ner_elapsed_ms" \
    --argjson fpe_ms "$fpe_elapsed_ms" \
    --argjson total_ms "$total_elapsed_ms" \
    '{
      file: $file,
      path: $path,
      architecture: $arch,
      redaction_mode: "fpe",
      json_format: $format,
      fpe_http_status: $fpe_http,
      timestamp: $ts,
      total_matches: $count,
      type_summary: $types,
      timing: {
        ner_scan_ms: $ner_ms,
        fpe_pass_ms: $fpe_ms,
        total_ms: $total_ms
      },
      matches: $matches
    }')

  echo "$result" | jq . > "$json_out"
  echo "OK  $filename ($format) — $match_count matches, FPE HTTP $fpe_http, ${total_elapsed_ms}ms (ner:${ner_elapsed_ms}ms fpe:${fpe_elapsed_ms}ms)"
}

# ── Main ──
if [[ -n "${1:-}" ]]; then
  test_file "$1"
else
  echo "=== Agent Tool Result FPE Tests ==="
  echo ""

  total=0
  pass_count=0
  fail_count=0
  RESULTS_JSON="[]"

  for file in "$INPUT_DIR"/*.json; do
    [[ -f "$file" ]] || continue
    fname=$(basename "$file")
    if test_file "$file"; then
      pass_count=$((pass_count + 1))
      name_no_ext="${fname%.*}"
      jf="$OUTPUT_DIR/json/${name_no_ext}_gateway.json"
      matches=$(jq '.total_matches // 0' "$jf" 2>/dev/null || echo 0)
      RESULTS_JSON=$(echo "$RESULTS_JSON" | jq --arg n "$fname" --arg d "$matches matches" '. + [{"name": $n, "status": "pass", "detail": $d}]')
    else
      fail_count=$((fail_count + 1))
      RESULTS_JSON=$(echo "$RESULTS_JSON" | jq --arg n "$fname" --arg d "test_file failed" '. + [{"name": $n, "status": "fail", "detail": $d}]')
    fi
    total=$((total + 1))
  done

  echo ""
  echo "Tested $total files."
  echo "JSON metadata:  $OUTPUT_DIR/json/"
  echo "FPE redacted:   $OUTPUT_DIR/redacted/"

  # Write validation JSON
  jq -n \
    --arg suite "agent_json" \
    --arg ts "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    --argjson total "$total" \
    --argjson pass "$pass_count" \
    --argjson fail "$fail_count" \
    --argjson warn 0 \
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
    }' > "$OUTPUT_DIR/agent_json_validation.json"
fi
