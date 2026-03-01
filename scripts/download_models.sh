#!/usr/bin/env bash
# download_models.sh — Download AGPL-licensed models not included in Git LFS.
#
# Models distributed via Git LFS (Apache 2.0 / MIT):
#   blazeface, scrfd, paddleocr, ner, kws
#
# Models requiring separate download (AGPL-3.0):
#   nudenet — NudeNet 320n NSFW classifier
#
# Usage:
#   ./scripts/download_models.sh          # Download all external models
#   ./scripts/download_models.sh nudenet   # Download only NudeNet

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"
MODELS_DIR="$ROOT_DIR/openobscure-proxy/models"

# ── NudeNet 320n (AGPL-3.0) ─────────────────────────────────────────────────
# Source: https://github.com/notAI-tech/NudeNet (v3.4-weights release)
# Mirror: https://huggingface.co/deepghs/nudenet_onnx (identical file)
# License: AGPL-3.0 — cannot be bundled in Apache/MIT repos
NUDENET_URL="https://huggingface.co/deepghs/nudenet_onnx/resolve/main/320n.onnx"
NUDENET_SHA256="c15d8273adad2d0a92f014cc69ab2d6c311a06777a55545f2c4eb46f51911f0f"
NUDENET_DIR="$MODELS_DIR/nudenet"

download_nudenet() {
    local dest="$NUDENET_DIR/320n.onnx"
    if [[ -f "$dest" ]]; then
        echo "NudeNet 320n already exists at $dest"
        # Verify checksum
        local actual
        actual=$(shasum -a 256 "$dest" | awk '{print $1}')
        if [[ "$actual" == "$NUDENET_SHA256" ]]; then
            echo "  Checksum OK"
            return 0
        else
            echo "  Checksum mismatch (expected $NUDENET_SHA256, got $actual)"
            echo "  Re-downloading..."
        fi
    fi

    echo "Downloading NudeNet 320n (12MB, AGPL-3.0)..."
    mkdir -p "$NUDENET_DIR"
    curl -fSL --progress-bar -o "$dest.tmp" "$NUDENET_URL"

    # Verify checksum
    local actual
    actual=$(shasum -a 256 "$dest.tmp" | awk '{print $1}')
    if [[ "$actual" != "$NUDENET_SHA256" ]]; then
        echo "ERROR: Checksum verification failed!"
        echo "  Expected: $NUDENET_SHA256"
        echo "  Got:      $actual"
        rm -f "$dest.tmp"
        exit 1
    fi

    mv "$dest.tmp" "$dest"
    echo "  Saved to $dest (checksum OK)"
}

# ── Main ─────────────────────────────────────────────────────────────────────

if [[ $# -eq 0 ]] || [[ "$1" == "all" ]]; then
    echo "=== Downloading external models ==="
    download_nudenet
    echo ""
    echo "Done. All models ready in $MODELS_DIR/"
elif [[ "$1" == "nudenet" ]]; then
    download_nudenet
else
    echo "Unknown model: $1"
    echo "Available: nudenet, all"
    exit 1
fi
