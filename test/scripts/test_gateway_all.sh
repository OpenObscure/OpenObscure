#!/usr/bin/env bash
# test_gateway_all.sh — Test ALL text-based input categories via Gateway FPE pipeline.
#
# Produces dual output per file:
#   test/data/output/<category>/json/<filename>_gateway.json   (NER metadata)
#   test/data/output/<category>/redacted/<filename>             (FPE-encrypted)
#
# Automatically starts/stops the echo server if not already running.
#
# Usage:
#   ./test/scripts/test_gateway_all.sh
#
# Prerequisites:
#   - Proxy built and running with echo upstream:
#       OPENOBSCURE_MASTER_KEY=$(openssl rand -hex 32) \
#         ./target/release/openobscure-core --config test/config/test_fpe.toml serve
#
# Environment:
#   PROXY_URL       — Proxy base URL (default: http://127.0.0.1:18790)
#   ECHO_PORT       — Echo server port (default: 18791)
#   CAPTURE_DIR     — Echo capture dir (default: /tmp/oo_echo_captures)
#   NO_AUTO_ECHO    — Set to "1" to skip auto-start of echo server

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
TEST_DIR="$(dirname "$SCRIPT_DIR")"
INPUT_DIR="$TEST_DIR/data/input"

PROXY_URL="${PROXY_URL:-http://127.0.0.1:18790}"
ECHO_PORT="${ECHO_PORT:-18791}"
CAPTURE_DIR="${CAPTURE_DIR:-/tmp/oo_echo_captures}"
export CAPTURE_DIR  # pass to child scripts

ECHO_STARTED_BY_US=false

# ── Auto-start echo server if needed ──
start_echo() {
  if [[ "${NO_AUTO_ECHO:-}" == "1" ]]; then
    return
  fi

  # Check if echo server is already running
  if curl -sf "http://127.0.0.1:${ECHO_PORT}/" >/dev/null 2>&1; then
    echo "Echo server already running on port $ECHO_PORT"
    return
  fi

  echo "Starting echo server on port $ECHO_PORT..."
  ECHO_PORT="$ECHO_PORT" CAPTURE_DIR="$CAPTURE_DIR" \
    node "$SCRIPT_DIR/echo_server.mjs" &
  ECHO_PID=$!
  ECHO_STARTED_BY_US=true

  # Wait for it to be ready
  for i in {1..20}; do
    if curl -sf "http://127.0.0.1:${ECHO_PORT}/" >/dev/null 2>&1; then
      echo "Echo server ready (PID: $ECHO_PID)"
      break
    fi
    sleep 0.25
  done
}

cleanup() {
  # Stop echo server if we started it
  if [[ "$ECHO_STARTED_BY_US" == "true" && -n "${ECHO_PID:-}" ]]; then
    echo ""
    echo "Stopping echo server (PID: $ECHO_PID)..."
    kill "$ECHO_PID" 2>/dev/null || true
    wait "$ECHO_PID" 2>/dev/null || true
  fi
  # Remove any stale capture files left by interrupted tests
  if [[ -d "$CAPTURE_DIR" ]]; then
    rm -f "$CAPTURE_DIR"/*.json 2>/dev/null || true
  fi
}

# Cleanup on exit (normal, error, or Ctrl+C)
trap cleanup EXIT

# ── Auth token ──
if [[ -z "${AUTH_TOKEN:-}" ]]; then
  TOKEN_FILE="$HOME/.openobscure/.auth-token"
  if [[ -f "$TOKEN_FILE" ]]; then
    AUTH_TOKEN=$(cat "$TOKEN_FILE")
  else
    AUTH_TOKEN=""
  fi
fi
export AUTH_TOKEN  # pass to child scripts

# ── Verify proxy is running ──
if [[ -n "$AUTH_TOKEN" ]]; then
  HEALTH=$(curl -sf "${PROXY_URL}/_openobscure/health" -H "X-OpenObscure-Token: $AUTH_TOKEN" 2>/dev/null || true)
else
  HEALTH=$(curl -sf "${PROXY_URL}/_openobscure/health" 2>/dev/null || true)
fi
if [[ -z "$HEALTH" ]]; then
  echo "Error: Proxy not reachable at $PROXY_URL"
  echo ""
  echo "Start the proxy with echo upstream:"
  echo "  OPENOBSCURE_MASTER_KEY=\$(openssl rand -hex 32) \\"
  echo "    ./target/release/openobscure-core --config test/config/test_fpe.toml serve"
  exit 1
fi

VERSION=$(echo "$HEALTH" | jq -r '.version // "unknown"')
TIER=$(echo "$HEALTH" | jq -r '.device_tier // "unknown"')

start_echo

echo "============================================"
echo "  OpenObscure Gateway FPE Test Suite"
echo "  Proxy: $PROXY_URL (v$VERSION, tier: $TIER)"
echo "  Echo:  127.0.0.1:$ECHO_PORT"
echo "  Output: json/ (NER metadata) + redacted/ (FPE)"
echo "============================================"
echo ""

CATEGORIES=(
  "PII_Detection"
  "Multilingual_PII"
  "Code_Config_PII"
  "Structured_Data_PII"
  "Agent_Tool_Results"
)

# ── Purge previous gateway results ──
OUTPUT_DIR="$TEST_DIR/data/output"
echo "Purging previous gateway results..."
for cat in "${CATEGORIES[@]}"; do
  rm -f "$OUTPUT_DIR/$cat"/json/*_gateway.json 2>/dev/null || true
  rm -f "$OUTPUT_DIR/$cat"/redacted/* 2>/dev/null || true
done
# Visual and Audio purged by their own scripts
echo ""

START_TIME=$(date +%s)

for cat in "${CATEGORIES[@]}"; do
  cat_dir="$INPUT_DIR/$cat"
  if [[ ! -d "$cat_dir" ]]; then
    echo "SKIP $cat (directory not found)"
    continue
  fi

  echo "--- $cat ---"
  "$SCRIPT_DIR/test_gateway_category.sh" "$cat" 2>/dev/null | while IFS= read -r line; do
    echo "  $line"
  done
  echo ""
done

# ── Visual PII Tests ──
echo "--- Visual_PII ---"
if [[ -d "$INPUT_DIR/Visual_PII" ]]; then
  "$SCRIPT_DIR/test_visual.sh" 2>/dev/null | while IFS= read -r line; do
    echo "  $line"
  done
else
  echo "  SKIP Visual_PII (input directory not found)"
fi
echo ""

# ── Audio PII Tests ──
echo "--- Audio_PII ---"
if [[ -d "$INPUT_DIR/Audio_PII" ]]; then
  "$SCRIPT_DIR/test_audio.sh" 2>/dev/null | while IFS= read -r line; do
    echo "  $line"
  done
else
  echo "  SKIP Audio_PII (input directory not found)"
fi
echo ""

END_TIME=$(date +%s)
ELAPSED=$((END_TIME - START_TIME))

echo "============================================"
echo "  All categories tested in ${ELAPSED}s"
echo "  JSON metadata:  test/data/output/*/json/"
echo "  FPE redacted:   test/data/output/*/redacted/"
echo "============================================"
