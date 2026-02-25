#!/usr/bin/env python3
"""12A.5: Download SemEval-2020 Task 11 PTC corpus and convert to Article 5 format.

Downloads the Propaganda Techniques Corpus (PTC) from Zenodo, converts
span-level annotations to paragraph-level multi-label format, and maps
14 propaganda techniques to EU AI Act Article 5 categories.

Source: https://zenodo.org/records/3952415
Paper:  https://arxiv.org/abs/2009.02696

Usage:
    python3 scripts/r2_download_semeval2020.py
    python3 scripts/r2_download_semeval2020.py --inspect   # schema only
    python3 scripts/r2_download_semeval2020.py --data-dir data/semeval2020/datasets  # skip download
"""

import argparse
import json
import os
import sys
import tarfile
import urllib.request
from collections import Counter
from pathlib import Path

ZENODO_URL = "https://zenodo.org/api/records/3952415/files/datasets-v2.tgz/content"

# SemEval-2020 Task 11 technique -> Article 5 category mapping
# Based on r2_prepare_semeval.py TECHNIQUE_MAP, adapted for 2020 naming
TECHNIQUE_MAP_2020 = {
    "Appeal_to_Authority": ("Art_5_1_a_Deceptive", "authority"),
    "Appeal_to_fear-prejudice": ("Art_5_1_a_Deceptive", "fear_loss"),
    "Bandwagon,Reductio_ad_hitlerum": ("Art_5_1_a_Deceptive", "social_proof"),
    "Black-and-White_Fallacy": ("Art_5_1_a_Deceptive", "anchoring"),
    "Causal_Oversimplification": ("Art_5_1_a_Deceptive", "anchoring"),
    "Exaggeration,Minimisation": ("Art_5_1_a_Deceptive", "emotional_priming"),
    "Flag-Waving": ("Art_5_1_a_Deceptive", "emotional_priming"),
    "Loaded_Language": ("Art_5_1_a_Deceptive", "emotional_priming"),
    "Name_Calling,Labeling": ("Art_5_1_a_Deceptive", "emotional_priming"),
    "Repetition": ("Art_5_1_a_Deceptive", "emotional_priming"),
    "Slogans": ("Art_5_1_a_Deceptive", "emotional_priming"),
    "Thought-terminating_Cliches": ("Art_5_1_a_Deceptive", "confirmshaming"),
    # Excluded: argumentative, not manipulative in Art. 5 sense
    "Doubt": None,
    "Whataboutism,Straw_Men,Red_Herring": None,
}

MIN_PARAGRAPH_LENGTH = 30
MAX_BENIGN_PARAGRAPHS = 500


def download_ptc(output_dir: Path) -> Path:
    """Download and extract PTC corpus from Zenodo."""
    tgz_path = output_dir / "datasets-v2.tgz"
    datasets_dir = output_dir / "datasets"

    if datasets_dir.exists():
        print(f"PTC corpus already extracted at {datasets_dir}")
        return datasets_dir

    if not tgz_path.exists():
        print(f"Downloading PTC corpus from Zenodo...")
        os.makedirs(output_dir, exist_ok=True)
        urllib.request.urlretrieve(ZENODO_URL, tgz_path)
        print(f"Downloaded: {tgz_path} ({tgz_path.stat().st_size:,} bytes)")

    print("Extracting...")
    with tarfile.open(tgz_path, "r:gz") as tar:
        tar.extractall(path=output_dir)

    if not datasets_dir.exists():
        print(f"ERROR: Expected {datasets_dir} after extraction")
        sys.exit(1)

    print(f"Extracted to {datasets_dir}")
    return datasets_dir


def load_articles(articles_dir: Path) -> dict[str, str]:
    """Load article texts keyed by article ID."""
    articles = {}
    for txt_file in sorted(articles_dir.glob("*.txt")):
        article_id = txt_file.stem.replace("article", "")
        with open(txt_file, encoding="utf-8") as f:
            articles[article_id] = f.read()
    return articles


def load_tc_labels(labels_path: Path) -> dict[str, list[dict]]:
    """Load technique classification labels grouped by article ID."""
    labels = {}
    with open(labels_path, encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            parts = line.split("\t")
            if len(parts) < 4:
                continue
            article_id = parts[0]
            technique = parts[1]
            start = int(parts[2])
            end = int(parts[3])
            if article_id not in labels:
                labels[article_id] = []
            labels[article_id].append({
                "technique": technique,
                "start": start,
                "end": end,
            })
    return labels


def split_paragraphs(text: str) -> list[tuple[int, int, str]]:
    """Split text into paragraphs with character offsets.

    Returns list of (start_offset, end_offset, paragraph_text).
    """
    paragraphs = []
    # Split on double newline first
    parts = text.split("\n\n")

    offset = 0
    for part in parts:
        # Skip empty parts
        stripped = part.strip()
        if len(stripped) < MIN_PARAGRAPH_LENGTH:
            offset += len(part) + 2  # +2 for the \n\n
            continue

        # Find actual start/end in original text
        start = text.find(part, offset)
        if start == -1:
            start = offset
        end = start + len(part)
        paragraphs.append((start, end, stripped))
        offset = end + 2  # +2 for the \n\n

    # Fallback: if no double newlines, split by single newlines
    if not paragraphs:
        offset = 0
        for line in text.split("\n"):
            stripped = line.strip()
            if len(stripped) >= MIN_PARAGRAPH_LENGTH:
                start = text.find(line, offset)
                if start == -1:
                    start = offset
                end = start + len(line)
                paragraphs.append((start, end, stripped))
            offset += len(line) + 1

    return paragraphs


def compute_overlap(span_start: int, span_end: int, para_start: int, para_end: int) -> float:
    """Compute fraction of span that overlaps with paragraph."""
    span_len = span_end - span_start
    if span_len <= 0:
        return 0.0
    overlap_start = max(span_start, para_start)
    overlap_end = min(span_end, para_end)
    overlap_len = max(0, overlap_end - overlap_start)
    return overlap_len / span_len


def convert_articles(articles: dict[str, str], labels: dict[str, list[dict]],
                     overlap_threshold: float = 0.5) -> tuple[list[dict], list[dict]]:
    """Convert span-level annotations to paragraph-level entries.

    Returns (manipulative_entries, benign_entries).
    """
    manipulative = []
    benign = []
    stats = Counter()

    for article_id, text in articles.items():
        article_labels = labels.get(article_id, [])
        paragraphs = split_paragraphs(text)

        for para_start, para_end, para_text in paragraphs:
            # Find all techniques that overlap with this paragraph
            techniques = set()
            for label in article_labels:
                overlap = compute_overlap(label["start"], label["end"], para_start, para_end)
                if overlap >= overlap_threshold:
                    tech = label["technique"]
                    mapping = TECHNIQUE_MAP_2020.get(tech)
                    if mapping is not None:
                        techniques.add((tech, mapping[0], mapping[1]))
                    elif tech in TECHNIQUE_MAP_2020:
                        stats["excluded_technique"] += 1

            if techniques:
                # Aggregate subcategories
                subcategories = sorted(set(m[2] for m in techniques))
                original_techniques = sorted(set(m[0] for m in techniques))
                entry = {
                    "text": para_text,
                    "category": "Art_5_1_a_Deceptive",
                    "subcategories": subcategories,
                    "domain": "news_political",
                    "source": "semeval2020_task11",
                    "source_file": f"article{article_id}",
                    "original_techniques": original_techniques,
                }
                manipulative.append(entry)
                stats["manipulative"] += 1
            else:
                # Paragraph with no matching techniques -> benign candidate
                if len(benign) < MAX_BENIGN_PARAGRAPHS:
                    entry = {
                        "text": para_text,
                        "category": "Art_5_1_a_Deceptive",
                        "label": "benign",
                        "domain": "news_political",
                        "source": "semeval2020_task11",
                        "source_file": f"article{article_id}",
                    }
                    benign.append(entry)
                stats["benign"] += 1

    stats["total_articles"] = len(articles)
    stats["total_paragraphs"] = stats["manipulative"] + stats["benign"]
    return manipulative, benign, stats


def main():
    parser = argparse.ArgumentParser(
        description="Download SemEval-2020 Task 11 PTC and convert to Article 5 format"
    )
    parser.add_argument(
        "--output-dir", "-o",
        default="data/semeval2020",
        help="Output directory for mapped JSONL files",
    )
    parser.add_argument(
        "--data-dir",
        default=None,
        help="Path to already-extracted PTC datasets directory (skip download)",
    )
    parser.add_argument(
        "--inspect", action="store_true",
        help="Print technique mapping and exit",
    )
    parser.add_argument(
        "--overlap-threshold",
        type=float, default=0.5,
        help="Minimum span overlap fraction to assign technique to paragraph (default: 0.5)",
    )
    args = parser.parse_args()

    if args.inspect:
        print("=== SemEval-2020 Task 11 -> Article 5 Technique Mapping ===\n")
        mapped = excluded = 0
        for tech, mapping in sorted(TECHNIQUE_MAP_2020.items()):
            if mapping is None:
                print(f"  {tech:<45} -> EXCLUDED (argumentative)")
                excluded += 1
            else:
                cat, subcat = mapping
                print(f"  {tech:<45} -> {cat} / {subcat}")
                mapped += 1
        print(f"\n  Mapped: {mapped}, Excluded: {excluded}, Total: {mapped + excluded}")
        return

    output_dir = Path(args.output_dir)

    # Download or locate PTC data
    if args.data_dir:
        datasets_dir = Path(args.data_dir)
        if not datasets_dir.exists():
            print(f"ERROR: Data directory not found: {datasets_dir}")
            sys.exit(1)
    else:
        datasets_dir = download_ptc(output_dir)

    # Load articles and labels
    print("\nLoading articles...")
    train_articles = load_articles(datasets_dir / "train-articles")
    dev_articles = load_articles(datasets_dir / "dev-articles")
    all_articles = {**train_articles, **dev_articles}
    print(f"  Train articles: {len(train_articles)}")
    print(f"  Dev articles:   {len(dev_articles)}")
    print(f"  Total:          {len(all_articles)}")

    print("\nLoading technique labels...")
    tc_labels = load_tc_labels(datasets_dir / "train-task2-TC.labels")
    print(f"  Articles with labels: {len(tc_labels)}")
    total_annotations = sum(len(v) for v in tc_labels.values())
    print(f"  Total annotations:    {total_annotations}")

    # Convert
    print(f"\nConverting (overlap threshold: {args.overlap_threshold})...")
    manipulative, benign, stats = convert_articles(
        all_articles, tc_labels, args.overlap_threshold
    )

    # Write output
    os.makedirs(output_dir, exist_ok=True)

    mapped_path = output_dir / "semeval2020_mapped.jsonl"
    with open(mapped_path, "w", encoding="utf-8") as f:
        for entry in manipulative:
            f.write(json.dumps(entry, ensure_ascii=False) + "\n")
    print(f"\nManipulative entries: {len(manipulative)} -> {mapped_path}")

    benign_path = output_dir / "semeval2020_benign.jsonl"
    with open(benign_path, "w", encoding="utf-8") as f:
        for entry in benign:
            f.write(json.dumps(entry, ensure_ascii=False) + "\n")
    print(f"Benign entries:      {len(benign)} -> {benign_path}")

    # Statistics
    print(f"\n=== Conversion Statistics ===")
    print(f"  Total articles:        {stats['total_articles']}")
    print(f"  Total paragraphs:      {stats['total_paragraphs']}")
    print(f"  Manipulative:          {stats['manipulative']}")
    print(f"  Benign (capped at {MAX_BENIGN_PARAGRAPHS}): {len(benign)}")
    print(f"  Excluded (argumentative): {stats.get('excluded_technique', 0)}")

    # Subcategory distribution
    subcat_counts = Counter()
    tech_counts = Counter()
    for entry in manipulative:
        for sc in entry["subcategories"]:
            subcat_counts[sc] += 1
        for tech in entry["original_techniques"]:
            tech_counts[tech] += 1

    print(f"\n  By original technique:")
    for tech, count in sorted(tech_counts.items(), key=lambda x: -x[1]):
        print(f"    {tech:<45} {count}")

    print(f"\n  By Article 5 subcategory:")
    for sc, count in sorted(subcat_counts.items(), key=lambda x: -x[1]):
        print(f"    {sc:<30} {count}")


if __name__ == "__main__":
    main()
