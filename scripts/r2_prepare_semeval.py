#!/usr/bin/env python3
"""12A.5: Download and preprocess SemEval-2023 Task 3 for R2 fine-tuning.

SemEval-2023 Task 3 — "Detecting the Category, the Framing, and the
Persuasion Techniques in Online News in a Multi-lingual Setup"

Dataset: https://propaganda.math.unipd.it/semeval2023task3/
Paper:   https://aclanthology.org/2023.semeval-1.317/

This script:
1. Downloads the SemEval-2023 Task 3 dataset (English subset, Subtask 3)
2. Maps 23 persuasion techniques to Article 5 categories
3. Outputs mapped JSONL for training augmentation

Usage:
    python3 scripts/r2_prepare_semeval.py --input data/semeval2023/ --output data/r2_training/semeval_mapped.jsonl

If --input is not provided, the script will attempt to download from the
official repository. You may need to request access from the task organisers.
"""

import argparse
import json
import os
import sys
from collections import Counter
from pathlib import Path

# ------------------------------------------------------------------
# SemEval-2023 Task 3 technique -> Article 5 category mapping
# ------------------------------------------------------------------
# Source: cognitive_firewall.md Section 14 + expanded mapping

TECHNIQUE_MAP = {
    # --- Art_5_1_a_Deceptive mappings ---
    # Fear subcategory
    "Appeal_to_Fear-Prejudice": ("Art_5_1_a_Deceptive", "fear_loss"),
    "Loaded_Language": ("Art_5_1_a_Deceptive", "emotional_priming"),

    # Social proof subcategory
    "Bandwagon": ("Art_5_1_a_Deceptive", "social_proof"),
    "Appeal_to_Popularity": ("Art_5_1_a_Deceptive", "social_proof"),

    # Authority subcategory
    "Appeal_to_Authority": ("Art_5_1_a_Deceptive", "authority"),

    # Deceptive / general manipulation
    "Causal_Oversimplification": ("Art_5_1_a_Deceptive", "anchoring"),
    "Exaggeration-Minimisation": ("Art_5_1_a_Deceptive", "emotional_priming"),
    "Repetition": ("Art_5_1_a_Deceptive", "emotional_priming"),
    "Obfuscation-Vagueness-Confusion": ("Art_5_1_a_Deceptive", "authority"),
    "Slogans": ("Art_5_1_a_Deceptive", "emotional_priming"),
    "Conversation_Killer": ("Art_5_1_a_Deceptive", "confirmshaming"),
    "Appeal_to_Values": ("Art_5_1_a_Deceptive", "emotional_priming"),
    "Consequential_Oversimplification": ("Art_5_1_a_Deceptive", "fear_loss"),
    "Appeal_to_Time": ("Art_5_1_a_Deceptive", "urgency"),
    "Guilt_by_Association": ("Art_5_1_a_Deceptive", "fear_loss"),
    "Appeal_to_Hypocrisy": ("Art_5_1_a_Deceptive", "confirmshaming"),

    # Emotional priming
    "Flag-Waving": ("Art_5_1_a_Deceptive", "emotional_priming"),

    # Anchoring / framing
    "Black-and-White_Fallacy": ("Art_5_1_a_Deceptive", "anchoring"),
    "False_Dilemma-No_Choice": ("Art_5_1_a_Deceptive", "anchoring"),

    # --- Out of scope (argumentative, not manipulative in Art. 5 sense) ---
    # These are kept for completeness but marked as excluded
    "Doubt": None,
    "Red_Herring": None,
    "Straw_Man": None,
    "Whataboutism": None,

    # Name calling is borderline — include as emotional priming
    "Name_Calling-Labeling": ("Art_5_1_a_Deceptive", "emotional_priming"),
}

# Reverse lookup: SemEval label variants (some datasets use different casing)
TECHNIQUE_ALIASES = {}
for key in TECHNIQUE_MAP:
    TECHNIQUE_ALIASES[key.lower()] = key
    TECHNIQUE_ALIASES[key.replace("_", " ").lower()] = key
    TECHNIQUE_ALIASES[key.replace("-", "_").lower()] = key


def resolve_technique(raw_label: str) -> str | None:
    """Resolve a raw SemEval technique label to a canonical key."""
    clean = raw_label.strip()
    if clean in TECHNIQUE_MAP:
        return clean
    return TECHNIQUE_ALIASES.get(clean.lower())


def map_entry(text: str, techniques: list[str], source_file: str = "") -> list[dict]:
    """Map a SemEval entry to Article 5 format.

    Returns a list of mapped entries (one per unique Art 5 category found).
    """
    art5_cats = {}  # category -> set of subcategories

    for tech in techniques:
        canon = resolve_technique(tech)
        if canon is None:
            continue  # Unknown technique, skip
        mapping = TECHNIQUE_MAP.get(canon)
        if mapping is None:
            continue  # Out of scope technique

        category, subcategory = mapping
        if category not in art5_cats:
            art5_cats[category] = set()
        art5_cats[category].add(subcategory)

    results = []
    for category, subcats in art5_cats.items():
        results.append({
            "text": text,
            "category": category,
            "subcategories": sorted(subcats),
            "domain": "news_political",
            "source": "semeval2023_task3",
            "source_file": source_file,
            "original_techniques": techniques,
        })

    return results


def parse_semeval_folder(input_dir: str) -> list[dict]:
    """Parse SemEval-2023 Task 3 data from the official folder structure.

    Expected structure:
        input_dir/
            en/
                train-labels-subtask-3.txt   (or .template)
                dev-labels-subtask-3.txt
                train-articles/
                    article_XXXXX.txt
                dev-articles/
                    article_XXXXX.txt

    Label format (tab-separated):
        article_id  paragraph_id  technique1,technique2,...

    Articles are plain text, one paragraph per line.
    """
    input_path = Path(input_dir)
    entries = []

    # Try multiple possible directory layouts
    en_dir = input_path / "en"
    if not en_dir.exists():
        en_dir = input_path  # Flat structure

    # Find label files
    label_files = list(en_dir.glob("*labels*subtask*3*"))
    if not label_files:
        label_files = list(en_dir.glob("**/train-labels-subtask-3*"))
        label_files += list(en_dir.glob("**/dev-labels-subtask-3*"))

    if not label_files:
        print(f"No SemEval label files found in {input_dir}")
        print(f"Expected: *labels*subtask*3* in {en_dir}")
        print(f"Contents: {list(en_dir.iterdir())[:20]}")
        return []

    # Find article directories
    article_dirs = list(en_dir.glob("*articles*"))
    if not article_dirs:
        article_dirs = list(en_dir.glob("**/*articles*"))

    # Build article text lookup: article_id -> {para_id -> text}
    articles = {}
    for adir in article_dirs:
        if not adir.is_dir():
            continue
        for afile in adir.glob("article_*.txt"):
            article_id = afile.stem.replace("article_", "")
            with open(afile, "r", encoding="utf-8") as f:
                paragraphs = {}
                for i, line in enumerate(f, 1):
                    stripped = line.strip()
                    if stripped:
                        paragraphs[str(i)] = stripped
                articles[article_id] = paragraphs

    print(f"Loaded {len(articles)} articles from {len(article_dirs)} directories")

    # Parse label files
    for label_file in label_files:
        print(f"Processing {label_file.name}...")
        with open(label_file, "r", encoding="utf-8") as f:
            for line in f:
                line = line.strip()
                if not line or line.startswith("#"):
                    continue
                parts = line.split("\t")
                if len(parts) < 3:
                    continue

                article_id = parts[0].strip()
                para_id = parts[1].strip()
                techniques = [t.strip() for t in parts[2].split(",") if t.strip()]

                # Look up paragraph text
                text = ""
                if article_id in articles:
                    text = articles[article_id].get(para_id, "")

                if not text:
                    continue  # Skip entries without text

                mapped = map_entry(text, techniques, label_file.name)
                entries.extend(mapped)

    return entries


def parse_huggingface_format(input_dir: str) -> list[dict]:
    """Parse from HuggingFace datasets format (JSONL or CSV).

    Some mirrors of SemEval data use a flat JSONL format:
        {"text": "...", "labels": ["technique1", "technique2"]}
    """
    entries = []
    input_path = Path(input_dir)

    for jsonl_file in input_path.glob("*.jsonl"):
        print(f"Processing {jsonl_file.name}...")
        with open(jsonl_file, "r", encoding="utf-8") as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                try:
                    obj = json.loads(line)
                except json.JSONDecodeError:
                    continue

                text = obj.get("text", obj.get("sentence", ""))
                techniques = obj.get("labels", obj.get("techniques", []))
                if isinstance(techniques, str):
                    techniques = [t.strip() for t in techniques.split(",")]

                if text and techniques:
                    mapped = map_entry(text, techniques, jsonl_file.name)
                    entries.extend(mapped)

    return entries


def main():
    parser = argparse.ArgumentParser(description="Prepare SemEval-2023 Task 3 data for R2")
    parser.add_argument(
        "--input", "-i",
        default="data/semeval2023/",
        help="Path to SemEval-2023 Task 3 data directory",
    )
    parser.add_argument(
        "--output", "-o",
        default="data/r2_training/semeval_mapped.jsonl",
        help="Output JSONL path",
    )
    parser.add_argument(
        "--stats-only", action="store_true",
        help="Print mapping statistics without processing data",
    )
    args = parser.parse_args()

    if args.stats_only:
        print("=== SemEval -> Article 5 Technique Mapping ===\n")
        mapped = 0
        excluded = 0
        for tech, mapping in sorted(TECHNIQUE_MAP.items()):
            if mapping is None:
                print(f"  {tech:<45} -> EXCLUDED (argumentative)")
                excluded += 1
            else:
                cat, subcat = mapping
                print(f"  {tech:<45} -> {cat} / {subcat}")
                mapped += 1
        print(f"\n  Mapped: {mapped}, Excluded: {excluded}, Total: {mapped + excluded}")
        return

    input_path = Path(args.input)
    if not input_path.exists():
        print(f"Input directory not found: {args.input}")
        print()
        print("To use this script:")
        print("  1. Download SemEval-2023 Task 3 data from:")
        print("     https://propaganda.math.unipd.it/semeval2023task3/")
        print("  2. Extract to data/semeval2023/")
        print("  3. Re-run: python3 scripts/r2_prepare_semeval.py")
        print()
        print("Alternatively, show the technique mapping with: --stats-only")
        print()
        print("The R2 training pipeline can proceed with synthetic data alone")
        print(f"(2,750 entries in data/ai_safety_cleaned.jsonl).")
        print("SemEval augmentation improves robustness but is not blocking.")
        sys.exit(0)

    # Try official format first, then HuggingFace format
    entries = parse_semeval_folder(args.input)
    if not entries:
        entries = parse_huggingface_format(args.input)

    if not entries:
        print("No entries could be parsed from the input directory.")
        print("Check the directory structure and file formats.")
        sys.exit(1)

    # Write output
    os.makedirs(os.path.dirname(args.output), exist_ok=True)
    with open(args.output, "w", encoding="utf-8") as f:
        for entry in entries:
            f.write(json.dumps(entry, ensure_ascii=False) + "\n")

    # Statistics
    cat_counts = Counter(e["category"] for e in entries)
    subcat_counts = Counter()
    for e in entries:
        for sc in e.get("subcategories", []):
            subcat_counts[sc] += 1

    print(f"\n=== SemEval Mapping Results ===")
    print(f"Total mapped entries: {len(entries)}")
    print(f"\nBy Article 5 category:")
    for cat, count in sorted(cat_counts.items()):
        print(f"  {cat}: {count}")
    print(f"\nBy subcategory:")
    for sc, count in sorted(subcat_counts.items(), key=lambda x: -x[1]):
        print(f"  {sc}: {count}")
    print(f"\nOutput: {args.output}")


if __name__ == "__main__":
    main()
