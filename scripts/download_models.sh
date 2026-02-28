#!/usr/bin/env bash
# download_models.sh — Download GPL-licensed models not included in Git LFS.
#
# Models distributed via Git LFS (Apache 2.0 / MIT):
#   blazeface, scrfd, paddleocr, ner, kws
#
# Models requiring separate download (GPL-3.0):
#   nudenet — NudeNet 320n NSFW classifier
#
# Usage:
#   ./scripts/download_models.sh          # Download all external models
#   ./scripts/download_models.sh nudenet   # Download only NudeNet

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(dirname "$SCRIPT_DIR")"
MODELS_DIR="$ROOT_DIR/openobscure-proxy/models"

# ── NudeNet 320n (GPL-3.0) ──────────────────────────────────────────────────
# Source: https://github.com/notAI-tech/NudeNet
# License: GPL-3.0 — cannot be bundled in Apache/MIT repos
NUDENET_URL="https://huggingface.co/deepghs/imgutils-models/resolve/main/nudenet/320n.onnx"
NUDENET_SHA256="9832f15515bdb06bcb5a77beb60bc8ea54439bd7ecbaac46dac3b760b3dd13cc"
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

    echo "Downloading NudeNet 320n (12MB, GPL-3.0)..."
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
