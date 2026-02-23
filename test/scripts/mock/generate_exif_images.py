#!/usr/bin/env python3
"""
Generate EXIF test images for OpenObscure visual pipeline testing.

Creates 12 JPEG images with controlled EXIF metadata to validate:
- GPS coordinate stripping
- Camera/smartphone metadata stripping
- Screenshot software detection via EXIF
- Clean pass-through of already-stripped images

Base images: downloads CC0 landscape photos from Unsplash; falls back to
Pillow-generated gradient images if download fails.

Usage:
    python3 test/scripts/mock/generate_exif_images.py [--output-dir test/data/input/Visual_PII/EXIF]
"""

import argparse
import io
import os
import struct
import sys

try:
    from PIL import Image
except ImportError:
    print("Pillow not installed. Run: pip3 install Pillow", file=sys.stderr)
    sys.exit(1)

try:
    import piexif
except ImportError:
    print("piexif not installed. Run: pip3 install piexif", file=sys.stderr)
    sys.exit(1)


# ── Unsplash CC0 photo IDs for base images ─────────────────────
# These are CC0 landscape/nature photos. IDs are stable.
UNSPLASH_IDS = [
    "1470071459604-3b5ec3a7fe05",  # landscape mountains
    "1506744038136-46273834b3fb",  # mountain lake
    "1441974231531-c6227db76b6e",  # forest path
]


def download_unsplash(photo_id: str, size: int = 1200) -> Image.Image | None:
    """Download a photo from Unsplash. Returns PIL Image or None."""
    try:
        import urllib.request
        # Use the Unsplash source URL (no API key needed, CC0 license)
        url = f"https://images.unsplash.com/photo-{photo_id}?w={size}&q=80&auto=format"
        req = urllib.request.Request(url, headers={"User-Agent": "OpenObscure-TestGen/1.0"})
        with urllib.request.urlopen(req, timeout=15) as resp:
            data = resp.read()
        return Image.open(io.BytesIO(data)).convert("RGB")
    except Exception as e:
        print(f"  Unsplash download failed ({photo_id}): {e}", file=sys.stderr)
        return None


def generate_gradient(width: int, height: int, seed: int) -> Image.Image:
    """Generate a gradient image as fallback (no network needed)."""
    img = Image.new("RGB", (width, height))
    pixels = img.load()
    for y in range(height):
        for x in range(width):
            r = int(((x + seed * 37) % width) / width * 200 + 30)
            g = int(((y + seed * 53) % height) / height * 180 + 40)
            b = int(((x + y + seed * 17) % (width + height)) / (width + height) * 160 + 60)
            pixels[x, y] = (r, g, b)
    return img


def get_base_images(count: int = 3) -> list[Image.Image]:
    """Get base images: try Unsplash first, fall back to gradients."""
    images = []
    for i, photo_id in enumerate(UNSPLASH_IDS[:count]):
        img = download_unsplash(photo_id)
        if img is None:
            print(f"  Using gradient fallback for image {i + 1}", file=sys.stderr)
            img = generate_gradient(4032, 3024, seed=i + 1)
        else:
            # Resize to camera-like resolution
            img = img.resize((4032, 3024), Image.LANCZOS)
        images.append(img)
    return images


def make_thumbnail(img: Image.Image, size: int = 160) -> bytes:
    """Create a JPEG thumbnail for EXIF embedding."""
    thumb = img.copy()
    thumb.thumbnail((size, size))
    buf = io.BytesIO()
    thumb.save(buf, "JPEG", quality=70)
    return buf.getvalue()


def gps_to_rational(degrees: float) -> tuple:
    """Convert decimal degrees to EXIF GPS rational format ((deg, 1), (min, 1), (sec*100, 100))."""
    d = int(abs(degrees))
    m = int((abs(degrees) - d) * 60)
    s = int(((abs(degrees) - d) * 60 - m) * 60 * 100)
    return ((d, 1), (m, 1), (s, 100))


def save_with_exif(img: Image.Image, exif_dict: dict, output_path: str) -> None:
    """Save image as JPEG with injected EXIF data."""
    exif_bytes = piexif.dump(exif_dict)
    buf = io.BytesIO()
    img.save(buf, "JPEG", quality=92)
    buf.seek(0)
    piexif.insert(exif_bytes, buf.getvalue(), output_path)


def save_no_exif(img: Image.Image, output_path: str) -> None:
    """Save image as JPEG with zero EXIF."""
    img.save(output_path, "JPEG", quality=92)


def generate_all(output_dir: str) -> None:
    os.makedirs(output_dir, exist_ok=True)

    print("Fetching base images...")
    bases = get_base_images(3)
    base_cam = bases[0]  # Camera-style
    base_phone = bases[1]  # Smartphone-style
    base_screen = bases[2] if len(bases) > 2 else bases[0]

    # ── 1. exif_camera_gps.jpg — Canon DSLR with GPS (Eiffel Tower) ──
    thumb_bytes = make_thumbnail(base_cam)
    exif = {
        "0th": {
            piexif.ImageIFD.Make: b"Canon",
            piexif.ImageIFD.Model: b"Canon EOS R5",
            piexif.ImageIFD.Software: b"Adobe Lightroom 14.2",
            piexif.ImageIFD.DateTime: b"2025:09:14 14:32:08",
            piexif.ImageIFD.ImageWidth: 4032,
            piexif.ImageIFD.ImageLength: 3024,
            piexif.ImageIFD.Orientation: 1,
        },
        "Exif": {
            piexif.ExifIFD.ExposureTime: (1, 250),
            piexif.ExifIFD.FNumber: (28, 10),
            piexif.ExifIFD.ISOSpeedRatings: 400,
            piexif.ExifIFD.FocalLength: (24, 1),
            piexif.ExifIFD.LensModel: b"RF24-105mm F4 L IS USM",
            piexif.ExifIFD.DateTimeOriginal: b"2025:09:14 14:32:08",
            piexif.ExifIFD.ColorSpace: 1,
        },
        "GPS": {
            piexif.GPSIFD.GPSLatitudeRef: b"N",
            piexif.GPSIFD.GPSLatitude: gps_to_rational(48.8566),
            piexif.GPSIFD.GPSLongitudeRef: b"E",
            piexif.GPSIFD.GPSLongitude: gps_to_rational(2.3522),
            piexif.GPSIFD.GPSAltitude: (35, 1),
            piexif.GPSIFD.GPSAltitudeRef: 0,
        },
        "1st": {
            piexif.ImageIFD.Compression: 6,
        },
        "thumbnail": thumb_bytes,
    }
    save_with_exif(base_cam, exif, os.path.join(output_dir, "exif_camera_gps.jpg"))
    print("  Created exif_camera_gps.jpg (Canon EOS R5 + GPS Eiffel Tower)")

    # ── 2. exif_camera_no_gps.jpg — Canon DSLR without GPS ──
    exif_no_gps = {
        "0th": {
            piexif.ImageIFD.Make: b"Canon",
            piexif.ImageIFD.Model: b"Canon EOS R5",
            piexif.ImageIFD.Software: b"Adobe Lightroom 14.2",
            piexif.ImageIFD.DateTime: b"2025:09:14 16:45:22",
            piexif.ImageIFD.ImageWidth: 4032,
            piexif.ImageIFD.ImageLength: 3024,
            piexif.ImageIFD.Orientation: 1,
        },
        "Exif": {
            piexif.ExifIFD.ExposureTime: (1, 125),
            piexif.ExifIFD.FNumber: (56, 10),
            piexif.ExifIFD.ISOSpeedRatings: 200,
            piexif.ExifIFD.FocalLength: (50, 1),
            piexif.ExifIFD.LensModel: b"RF24-105mm F4 L IS USM",
            piexif.ExifIFD.DateTimeOriginal: b"2025:09:14 16:45:22",
            piexif.ExifIFD.ColorSpace: 1,
        },
        "GPS": {},
        "1st": {},
    }
    save_with_exif(base_cam, exif_no_gps, os.path.join(output_dir, "exif_camera_no_gps.jpg"))
    print("  Created exif_camera_no_gps.jpg (Canon EOS R5, no GPS)")

    # ── 3. exif_smartphone_gps.jpg — iPhone with GPS (Statue of Liberty) ──
    thumb_bytes2 = make_thumbnail(base_phone)
    exif_iphone_gps = {
        "0th": {
            piexif.ImageIFD.Make: b"Apple",
            piexif.ImageIFD.Model: b"iPhone 15 Pro",
            piexif.ImageIFD.Software: b"17.3",
            piexif.ImageIFD.DateTime: b"2025:11:22 09:15:44",
            piexif.ImageIFD.ImageWidth: 4032,
            piexif.ImageIFD.ImageLength: 3024,
            piexif.ImageIFD.Orientation: 1,
        },
        "Exif": {
            piexif.ExifIFD.ExposureTime: (1, 1000),
            piexif.ExifIFD.FNumber: (16, 10),
            piexif.ExifIFD.ISOSpeedRatings: 50,
            piexif.ExifIFD.FocalLength: (69, 10),
            piexif.ExifIFD.LensModel: b"iPhone 15 Pro back triple camera 6.765mm f/1.78",
            piexif.ExifIFD.DateTimeOriginal: b"2025:11:22 09:15:44",
            piexif.ExifIFD.ColorSpace: 65535,
        },
        "GPS": {
            piexif.GPSIFD.GPSLatitudeRef: b"N",
            piexif.GPSIFD.GPSLatitude: gps_to_rational(40.6892),
            piexif.GPSIFD.GPSLongitudeRef: b"W",
            piexif.GPSIFD.GPSLongitude: gps_to_rational(74.0445),
            piexif.GPSIFD.GPSAltitude: (10, 1),
            piexif.GPSIFD.GPSAltitudeRef: 0,
        },
        "1st": {
            piexif.ImageIFD.Compression: 6,
        },
        "thumbnail": thumb_bytes2,
    }
    save_with_exif(base_phone, exif_iphone_gps, os.path.join(output_dir, "exif_smartphone_gps.jpg"))
    print("  Created exif_smartphone_gps.jpg (iPhone 15 Pro + GPS Statue of Liberty)")

    # ── 4. exif_smartphone_no_gps.jpg — iPhone without GPS ──
    exif_iphone_no_gps = {
        "0th": {
            piexif.ImageIFD.Make: b"Apple",
            piexif.ImageIFD.Model: b"iPhone 15 Pro",
            piexif.ImageIFD.Software: b"17.3",
            piexif.ImageIFD.DateTime: b"2025:11:22 11:30:02",
            piexif.ImageIFD.ImageWidth: 4032,
            piexif.ImageIFD.ImageLength: 3024,
            piexif.ImageIFD.Orientation: 1,
        },
        "Exif": {
            piexif.ExifIFD.ExposureTime: (1, 500),
            piexif.ExifIFD.FNumber: (16, 10),
            piexif.ExifIFD.ISOSpeedRatings: 100,
            piexif.ExifIFD.FocalLength: (69, 10),
            piexif.ExifIFD.LensModel: b"iPhone 15 Pro back triple camera 6.765mm f/1.78",
            piexif.ExifIFD.DateTimeOriginal: b"2025:11:22 11:30:02",
        },
        "GPS": {},
        "1st": {},
    }
    save_with_exif(base_phone, exif_iphone_no_gps, os.path.join(output_dir, "exif_smartphone_no_gps.jpg"))
    print("  Created exif_smartphone_no_gps.jpg (iPhone 15 Pro, no GPS)")

    # ── 5-10. Screenshot tool images ──
    screenshot_tools = [
        ("exif_screenshot_cleanshot.jpg", b"CleanShot X"),
        ("exif_screenshot_flameshot.jpg", b"Flameshot 12.1.0"),
        ("exif_screenshot_gnome.jpg", b"gnome-screenshot"),
        ("exif_screenshot_screencapture.jpg", b"screencapture"),
        ("exif_screenshot_sharex.jpg", b"ShareX"),
        ("exif_screenshot_snipping_tool.jpg", b"Snipping Tool"),
    ]

    for filename, software in screenshot_tools:
        # Screenshots are typically smaller resolution
        screen_img = base_screen.resize((1920, 1080), Image.LANCZOS)
        exif_ss = {
            "0th": {
                piexif.ImageIFD.Software: software,
                piexif.ImageIFD.DateTime: b"2025:12:01 10:00:00",
            },
            "Exif": {},
            "GPS": {},
            "1st": {},
        }
        save_with_exif(screen_img, exif_ss, os.path.join(output_dir, filename))
        print(f"  Created {filename} (Software={software.decode()})")

    # ── 11. exif_no_metadata_control.jpg — Zero EXIF ──
    # Use non-screenshot resolution (1600x1200) to avoid false positive
    save_no_exif(base_cam.resize((1600, 1200), Image.LANCZOS),
                 os.path.join(output_dir, "exif_no_metadata_control.jpg"))
    print("  Created exif_no_metadata_control.jpg (zero EXIF, 1600x1200)")

    # ── 12. exif_stripped_control.jpg — Zero EXIF ──
    save_no_exif(base_phone.resize((1600, 1200), Image.LANCZOS),
                 os.path.join(output_dir, "exif_stripped_control.jpg"))
    print("  Created exif_stripped_control.jpg (zero EXIF, 1600x1200)")

    # ── Verification report ──
    print("\nVerification:")
    for fname in sorted(os.listdir(output_dir)):
        if not fname.endswith(".jpg"):
            continue
        fpath = os.path.join(output_dir, fname)
        size = os.path.getsize(fpath)
        try:
            exif_data = piexif.load(fpath)
            tag_count = sum(len(ifd) for ifd in [exif_data.get("0th", {}),
                                                   exif_data.get("Exif", {}),
                                                   exif_data.get("GPS", {})])
            has_gps = len(exif_data.get("GPS", {})) > 0
        except Exception:
            tag_count = 0
            has_gps = False
        gps_str = " GPS" if has_gps else ""
        print(f"  {fname:45s} {size:>8d} bytes  {tag_count:>2d} EXIF tags{gps_str}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Generate EXIF test images")
    parser.add_argument(
        "--output-dir",
        default="test/data/input/Visual_PII/EXIF",
        help="Output directory for EXIF test images",
    )
    args = parser.parse_args()
    generate_all(args.output_dir)
