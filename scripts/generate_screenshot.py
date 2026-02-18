#!/usr/bin/env python3
"""
Generate a synthetic screenshot containing PII text for OpenObscure demo.

Creates a realistic-looking "patient record" screenshot with names, SSNs,
phone numbers, and medical info — all fictitious. This lets the OCR pipeline
demonstrate text-region blurring on sensitive content.

Usage:
    python3 scripts/generate_screenshot.py [--output docs/examples/images/screenshot-original.png]
"""

import argparse
import os
import sys

try:
    from PIL import Image, ImageDraw, ImageFont
except ImportError:
    print("Pillow not installed. Run: pip3 install Pillow", file=sys.stderr)
    sys.exit(1)


def generate_screenshot(output_path: str) -> None:
    width, height = 640, 400
    bg_color = (245, 245, 245)
    header_color = (41, 98, 255)
    text_color = (33, 33, 33)
    label_color = (100, 100, 100)
    border_color = (200, 200, 200)

    img = Image.new("RGB", (width, height), bg_color)
    draw = ImageDraw.Draw(img)

    # Use default font (always available)
    try:
        font_large = ImageFont.truetype("/System/Library/Fonts/Helvetica.ttc", 18)
        font_medium = ImageFont.truetype("/System/Library/Fonts/Helvetica.ttc", 14)
        font_small = ImageFont.truetype("/System/Library/Fonts/Helvetica.ttc", 12)
    except (OSError, IOError):
        font_large = ImageFont.load_default()
        font_medium = font_large
        font_small = font_large

    # Header bar
    draw.rectangle([(0, 0), (width, 50)], fill=header_color)
    draw.text((20, 14), "Patient Record — HealthCorp EHR", fill="white", font=font_large)

    # Content area with border
    draw.rectangle([(20, 65), (620, 380)], outline=border_color, width=1)

    y = 80
    line_height = 28

    records = [
        ("Patient Name:", "Sarah M. Johnson"),
        ("Date of Birth:", "03/15/1988"),
        ("SSN:", "428-71-5523"),
        ("Phone:", "+1 (555) 847-2093"),
        ("Address:", "1423 Oak Street, Portland, OR 97205"),
        ("", ""),
        ("Diagnosis:", "Type 2 Diabetes Mellitus (E11.9)"),
        ("Medication:", "Metformin 500mg twice daily"),
        ("Provider:", "Dr. Robert Chen, MD"),
        ("Next Appt:", "02/28/2026 at 10:30 AM"),
    ]

    for label, value in records:
        if not label and not value:
            y += 10
            draw.line([(30, y), (610, y)], fill=border_color, width=1)
            y += 10
            continue
        draw.text((35, y), label, fill=label_color, font=font_small)
        draw.text((160, y), value, fill=text_color, font=font_medium)
        y += line_height

    os.makedirs(os.path.dirname(output_path), exist_ok=True)
    img.save(output_path, "PNG")
    size = os.path.getsize(output_path)
    print(f"Generated: {output_path} ({size} bytes)")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Generate synthetic PII screenshot")
    parser.add_argument(
        "--output",
        default="docs/examples/images/screenshot-original.png",
        help="Output PNG path",
    )
    args = parser.parse_args()
    generate_screenshot(args.output)
