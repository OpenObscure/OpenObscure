#!/usr/bin/env bash
# test_gateway_file.sh — Test a single file against the L0 Gateway with FPE output.
#
# Produces two outputs:
#   1) JSON metadata  → <output_dir>/json/<name>_gateway.json   (NER spans + counts)
#   2) Redacted file  → <output_dir>/redacted/<filename>         (FPE-encrypted via proxy)
#
# The redacted file is produced by sending content through the proxy's full
# pass-through pipeline. The proxy applies:
#   - FF1 FPE encryption for CreditCard, SSN, Phone, Email, ApiKey
#     (format-preserving: "4111-1111-1111-1111" → "4732-8294-5617-3048")
#   - Label redaction for IPv4, IPv6, GPS, MAC, Health/Child keywords, NER entities
#
# Requires:
#   - Proxy running with upstream pointing to echo server (see test/config/test_fpe.toml)
#   - Echo server running: node test/scripts/echo_server.mjs
#
# Usage:
#   ./test/scripts/test_gateway_file.sh <input_file> [output_dir]
#
# Environment:
#   PROXY_URL       — Proxy base URL (default: http://127.0.0.1:18790)
#   PROVIDER_PREFIX — Route prefix for pass-through (default: /anthropic)
#   CAPTURE_DIR     — Echo server capture dir (default: /tmp/oo_echo_captures)
#   AUTH_TOKEN      — Proxy auth token (default: read from ~/.openobscure/.auth-token)

set -euo pipefail

# Millisecond timestamp (portable: Perl on macOS, date +%s%N on Linux)
_ms() { perl -MTime::HiRes -e 'printf("%d\n", Time::HiRes::time() * 1000)' 2>/dev/null || echo $(( $(date +%s) * 1000 )); }

PROXY_URL="${PROXY_URL:-http://127.0.0.1:18790}"
NER_ENDPOINT="${PROXY_URL}/_openobscure/ner"
PROVIDER_PREFIX="${PROVIDER_PREFIX:-/anthropic}"
FPE_ENDPOINT="${PROXY_URL}${PROVIDER_PREFIX}/v1/messages"
CAPTURE_DIR="${CAPTURE_DIR:-/tmp/oo_echo_captures}"

# Auth token: env var > file > empty
if [[ -z "${AUTH_TOKEN:-}" ]]; then
  TOKEN_FILE="$HOME/.openobscure/.auth-token"
  if [[ -f "$TOKEN_FILE" ]]; then
    AUTH_TOKEN=$(cat "$TOKEN_FILE")
  else
    AUTH_TOKEN=""
  fi
fi

INPUT_FILE="${1:-}"
OUTPUT_DIR="${2:-}"

if [[ -z "$INPUT_FILE" ]]; then
  echo "Usage: $0 <input_file> [output_dir]"
  echo ""
  echo "Examples:"
  echo "  $0 test/data/input/PII_Detection/Credit_Card_Numbers.txt"
  echo "  $0 test/data/input/PII_Detection/Credit_Card_Numbers.txt test/data/output/PII_Detection"
  exit 1
fi

if [[ ! -f "$INPUT_FILE" ]]; then
  echo "Error: File not found: $INPUT_FILE"
  exit 1
fi

# Check proxy health
if [[ -n "$AUTH_TOKEN" ]]; then
  HEALTH=$(curl -sf "${PROXY_URL}/_openobscure/health" -H "X-OpenObscure-Token: $AUTH_TOKEN" 2>/dev/null || true)
else
  HEALTH=$(curl -sf "${PROXY_URL}/_openobscure/health" 2>/dev/null || true)
fi
if [[ -z "$HEALTH" ]]; then
  echo "Error: Proxy not reachable at $PROXY_URL"
  echo "Start the proxy: OPENOBSCURE_MASTER_KEY=\$(openssl rand -hex 32) ./target/release/openobscure-proxy --config test/config/test_fpe.toml serve"
  exit 1
fi

FILENAME=$(basename "$INPUT_FILE")

# Read file content (truncate to 64KB for NER endpoint limit)
TEXT=$(cat "$INPUT_FILE")
BYTE_COUNT=$(echo -n "$TEXT" | wc -c | tr -d ' ')
if (( BYTE_COUNT > 65536 )); then
  echo "Warning: File is ${BYTE_COUNT} bytes (max 65536). Truncating to 64KB."
  TEXT=$(echo -n "$TEXT" | head -c 65536)
fi

# ─────────────────────────────────────────────
# Step 1: NER endpoint → JSON metadata
# ─────────────────────────────────────────────
NER_PAYLOAD=$(jq -n --arg text "$TEXT" '{text: $text}')

NER_START=$(_ms)
if [[ -n "$AUTH_TOKEN" ]]; then
  NER_RESPONSE=$(curl -sf -X POST "$NER_ENDPOINT" \
    -H "Content-Type: application/json" \
    -H "X-OpenObscure-Token: $AUTH_TOKEN" \
    -d "$NER_PAYLOAD" 2>/dev/null || echo "[]")
else
  NER_RESPONSE=$(curl -sf -X POST "$NER_ENDPOINT" \
    -H "Content-Type: application/json" \
    -d "$NER_PAYLOAD" 2>/dev/null || echo "[]")
fi
NER_ELAPSED_MS=$(( $(_ms) - NER_START ))

MATCH_COUNT=$(echo "$NER_RESPONSE" | jq 'length')
TYPE_SUMMARY=$(echo "$NER_RESPONSE" | jq '[.[].type] | group_by(.) | map({(.[0]): length}) | add // {}')

# RESULT constructed after Step 2 (needs timing data)

# ─────────────────────────────────────────────
# Step 2: Proxy pass-through → FPE-encrypted file
# ─────────────────────────────────────────────
# Wrap file content in an Anthropic message and send through the proxy.
# The proxy FPE-encrypts PII and forwards to the echo server.
# The echo server saves the encrypted body for us to read.

CAPTURE_ID="${FILENAME}_$$_$(date +%s)"

# Ensure capture file is cleaned up on any exit (error, Ctrl+C, etc.)
cleanup_capture() {
  rm -f "$CAPTURE_DIR/${CAPTURE_ID}.json" 2>/dev/null || true
  rm -f /tmp/oo_gw_hdr_* 2>/dev/null || true
}
trap cleanup_capture EXIT
# Extract an X-OO-* header value from a curl header dump, defaulting to 0.
_oo_hdr() { local v; v=$({ grep -i "^${1}:" "$2" 2>/dev/null || true; } | head -1 | awk '{print $2}' | tr -d '\r\n '); echo "${v:-0}"; }

FPE_PAYLOAD=$(jq -n --arg text "$TEXT" '{
  model: "test-fpe-capture",
  max_tokens: 1,
  messages: [{role: "user", content: $text}]
}')

TMP_FPE_HEADERS=$(mktemp /tmp/oo_gw_hdr_XXXXXX)

FPE_START=$(_ms)
FPE_HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" -D "$TMP_FPE_HEADERS" -X POST "$FPE_ENDPOINT" \
  -H "Content-Type: application/json" \
  -H "x-api-key: test-fpe-scan" \
  -H "anthropic-version: 2023-06-01" \
  -H "X-Capture-Id: $CAPTURE_ID" \
  ${AUTH_TOKEN:+-H "X-OpenObscure-Token: $AUTH_TOKEN"} \
  -d "$FPE_PAYLOAD" 2>/dev/null)
FPE_ELAPSED_MS=$(( $(_ms) - FPE_START ))
TOTAL_ELAPSED_MS=$(( NER_ELAPSED_MS + FPE_ELAPSED_MS ))

# Extract per-feature timing from X-OO-* response headers
PROXY_SCAN_US=$(_oo_hdr "x-oo-scan-us" "$TMP_FPE_HEADERS")
PROXY_FPE_US=$(_oo_hdr "x-oo-fpe-us" "$TMP_FPE_HEADERS")
PROXY_TOTAL_US=$(_oo_hdr "x-oo-total-us" "$TMP_FPE_HEADERS")
rm -f "$TMP_FPE_HEADERS"

# Read the captured FPE body from echo server
CAPTURE_FILE="$CAPTURE_DIR/${CAPTURE_ID}.json"
FPE_TEXT=""

if [[ -f "$CAPTURE_FILE" ]]; then
  # Extract the message content — this is the FPE-encrypted version of our file
  FPE_TEXT=$(jq -r '.messages[0].content // empty' "$CAPTURE_FILE" 2>/dev/null || true)
  # Clean up capture file
  rm -f "$CAPTURE_FILE"
fi

# Fallback: if FPE capture failed, use original text with a warning
if [[ -z "$FPE_TEXT" ]]; then
  FPE_TEXT="$TEXT"
  if [[ "$FPE_HTTP_CODE" != "200" ]]; then
    echo "WARN $FILENAME — FPE pass-through returned HTTP $FPE_HTTP_CODE (echo server running?)"
  fi
fi

# ─────────────────────────────────────────────
# Step 3: Build result JSON and write outputs
# ─────────────────────────────────────────────
RESULT=$(jq -n \
  --arg file "$FILENAME" \
  --arg path "$INPUT_FILE" \
  --arg arch "gateway" \
  --arg endpoint "$NER_ENDPOINT" \
  --arg ts "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --argjson count "$MATCH_COUNT" \
  --argjson types "$TYPE_SUMMARY" \
  --argjson matches "$NER_RESPONSE" \
  --argjson ner_ms "$NER_ELAPSED_MS" \
  --argjson fpe_ms "$FPE_ELAPSED_MS" \
  --argjson total_ms "$TOTAL_ELAPSED_MS" \
  --argjson proxy_scan_us "$PROXY_SCAN_US" \
  --argjson proxy_fpe_us "$PROXY_FPE_US" \
  --argjson proxy_total_us "$PROXY_TOTAL_US" \
  '{
    file: $file,
    path: $path,
    architecture: $arch,
    redaction_mode: "fpe",
    endpoint: $endpoint,
    timestamp: $ts,
    total_matches: $count,
    type_summary: $types,
    timing: {
      ner_scan_ms: $ner_ms,
      fpe_pass_ms: $fpe_ms,
      total_ms: $total_ms,
      proxy_scan_us: $proxy_scan_us,
      proxy_fpe_us: $proxy_fpe_us,
      proxy_total_us: $proxy_total_us
    },
    matches: $matches
  }')

if [[ -n "$OUTPUT_DIR" ]]; then
  JSON_DIR="$OUTPUT_DIR/json"
  REDACTED_DIR="$OUTPUT_DIR/redacted"
  mkdir -p "$JSON_DIR" "$REDACTED_DIR"

  NAME_NO_EXT="${FILENAME%.*}"

  # Write JSON metadata
  echo "$RESULT" | jq . > "$JSON_DIR/${NAME_NO_EXT}_gateway.json"

  # Write FPE-encrypted file (preserving original filename)
  printf '%s' "$FPE_TEXT" > "$REDACTED_DIR/$FILENAME"

  echo "OK  $FILENAME — $MATCH_COUNT matches, FPE HTTP $FPE_HTTP_CODE, ${TOTAL_ELAPSED_MS}ms (ner:${NER_ELAPSED_MS}ms fpe:${FPE_ELAPSED_MS}ms, proxy scan:${PROXY_SCAN_US}us fpe:${PROXY_FPE_US}us)"
else
  echo "=== JSON Metadata ==="
  echo "$RESULT" | jq .
  echo ""
  echo "=== FPE-Encrypted Preview (first 500 chars) ==="
  echo "${FPE_TEXT:0:500}"
fi
