#!/usr/bin/env python3
"""Validate NudeNet 320n against the official Python implementation.

Usage:
    python3 scripts/validate_nudenet.py <image_path>

Requires: pip install nudenet opencv-python-headless
"""
import sys
import os

def run_official(image_path: str):
    """Run official NudeNet Python detector."""
    try:
        from nudenet import NudeDetector
    except ImportError:
        print("ERROR: nudenet not installed. Run: pip install nudenet")
        sys.exit(1)

    detector = NudeDetector()
    results = detector.detect(image_path)

    print(f"\n=== Official NudeNet Results for: {os.path.basename(image_path)} ===")
    if not results:
        print("  No detections.")
        return

    for det in sorted(results, key=lambda d: d["score"], reverse=True):
        label = det["class"]
        score = det["score"]
        box = det["box"]
        exposed = "_EXPOSED" in label and label not in (
            "ARMPITS_EXPOSED", "FEET_EXPOSED", "BELLY_EXPOSED",
            "MALE_BREAST_EXPOSED"
        )
        marker = " *** NSFW ***" if exposed else ""
        print(f"  {label}: {score:.1%}  box={box}{marker}")

    nsfw_classes = [
        "BUTTOCKS_EXPOSED", "FEMALE_BREAST_EXPOSED",
        "FEMALE_GENITALIA_EXPOSED", "ANUS_EXPOSED",
        "MALE_GENITALIA_EXPOSED"
    ]
    nsfw_dets = [d for d in results if d["class"] in nsfw_classes]
    if nsfw_dets:
        best = max(nsfw_dets, key=lambda d: d["score"])
        print(f"\n  NSFW DETECTED: {best['class']} at {best['score']:.1%}")
    else:
        print(f"\n  No NSFW detected (no exposed classes found)")

if __name__ == "__main__":
    if len(sys.argv) != 2:
        print(f"Usage: {sys.argv[0]} <image_path>")
        sys.exit(1)
    run_official(sys.argv[1])
