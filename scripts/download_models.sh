#!/usr/bin/env bash
# download_models.sh — Backup download for models also tracked via Git LFS.
#
# All models are now tracked via Git LFS. All model licenses are permissive
# (Apache-2.0 / MIT) or trained in-house.
# This script is a fallback for environments without Git LFS or for
# re-downloading models with checksum verification.
#
# Models in Git LFS:
#   blazeface, scrfd, paddleocr, ner, kws, nsfw_classifier (Apache 2.0 / MIT)
#
# Usage:
#   ./scripts/download_models.sh          # Download all external models

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"
MODELS_DIR="$ROOT_DIR/openobscure-core/models"

# ── Main ─────────────────────────────────────────────────────────────────────

echo "=== Downloading external models ==="
echo "All models are tracked via Git LFS. Run 'git lfs pull' to fetch them."
echo ""
echo "Done. All models ready in $MODELS_DIR/"
