#!/usr/bin/env python3
"""
Generate a synthetic screenshot containing PII text for OpenObscure testing.

Creates a realistic-looking multi-section document with names, SSNs, credit cards,
phone numbers, emails, addresses, and medical info — all fictitious. This lets
the OCR pipeline demonstrate text-region blurring on sensitive content.

Usage:
    python3 test/scripts/mock/generate_screenshot.py [--output docs/examples/images/screenshot-original.png]
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
    width, height = 800, 620
    bg_color = (248, 248, 252)
    header_color = (41, 65, 122)
    section_color = (55, 90, 160)
    text_color = (33, 33, 33)
    label_color = (90, 90, 100)
    border_color = (200, 205, 215)
    stripe_color = (240, 242, 248)

    img = Image.new("RGB", (width, height), bg_color)
    draw = ImageDraw.Draw(img)

    # Use system fonts (macOS), fallback to default
    try:
        font_title = ImageFont.truetype("/System/Library/Fonts/Helvetica.ttc", 20)
        font_section = ImageFont.truetype("/System/Library/Fonts/Helvetica.ttc", 15)
        font_medium = ImageFont.truetype("/System/Library/Fonts/Helvetica.ttc", 13)
        font_small = ImageFont.truetype("/System/Library/Fonts/Helvetica.ttc", 11)
    except (OSError, IOError):
        font_title = ImageFont.load_default()
        font_section = font_title
        font_medium = font_title
        font_small = font_title

    # Header bar
    draw.rectangle([(0, 0), (width, 52)], fill=header_color)
    draw.text((24, 14), "HealthCorp EHR — Patient Record #2847-A", fill="white", font=font_title)

    y = 62

    def draw_section(title: str, records: list[tuple[str, str]]) -> int:
        nonlocal y
        # Section header
        draw.rectangle([(16, y), (width - 16, y + 26)], fill=section_color)
        draw.text((24, y + 5), title, fill="white", font=font_section)
        y += 32

        # Content rows with alternating stripes
        for i, (label, value) in enumerate(records):
            row_y = y
            if i % 2 == 0:
                draw.rectangle([(16, row_y - 2), (width - 16, row_y + 20)], fill=stripe_color)
            draw.text((28, row_y), label, fill=label_color, font=font_small)
            draw.text((180, row_y), value, fill=text_color, font=font_medium)
            y += 22
        y += 8
        return y

    draw_section("Patient Information", [
        ("Full Name:", "Sarah Michelle Johnson"),
        ("Date of Birth:", "March 15, 1988"),
        ("SSN:", "428-71-5523"),
        ("Phone:", "+1 (555) 847-2093"),
        ("Email:", "sarah.johnson@protonmail.com"),
        ("Address:", "1423 Oak Street, Apt 7B, Portland, OR 97205"),
    ])

    draw_section("Insurance & Billing", [
        ("Provider:", "BlueCross BlueShield — Gold Plan"),
        ("Member ID:", "BCB-882847-AX"),
        ("Credit Card:", "4532-8891-0047-6623"),
        ("Billing Phone:", "+1 (800) 555-0172"),
        ("Employer:", "Acme Technologies, Inc."),
    ])

    draw_section("Medical History", [
        ("Diagnosis:", "Type 2 Diabetes Mellitus (E11.9)"),
        ("Medication:", "Metformin 500mg twice daily, Lisinopril 10mg"),
        ("Allergies:", "Penicillin (anaphylaxis), Sulfa drugs"),
        ("Provider:", "Dr. Robert Chen, MD — Internal Medicine"),
        ("Last Visit:", "January 12, 2026"),
    ])

    draw_section("Emergency Contact", [
        ("Name:", "Michael David Johnson"),
        ("Relationship:", "Spouse"),
        ("Phone:", "+1 (555) 293-8471"),
        ("Email:", "m.johnson88@gmail.com"),
        ("Address:", "1423 Oak Street, Apt 7B, Portland, OR 97205"),
    ])

    # Footer
    draw.line([(16, y + 2), (width - 16, y + 2)], fill=border_color, width=1)
    draw.text(
        (24, y + 8),
        "CONFIDENTIAL — HIPAA Protected Health Information — Do Not Distribute",
        fill=(180, 60, 60),
        font=font_small,
    )
    draw.text(
        (24, y + 24),
        "Generated: 2026-02-18 10:30:15 UTC | Record ID: EHR-2847-A | Facility: Portland General",
        fill=label_color,
        font=font_small,
    )

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
