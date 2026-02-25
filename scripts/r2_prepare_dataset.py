#!/usr/bin/env python3
"""12A.6: Prepare final R2 training dataset.

Combines synthetic data (ai_safety_cleaned.jsonl) with optional SemEval
augmentation, converts to multi-label classification format, and creates
train/val/test splits.

Output format (JSONL):
    {"text": "...", "labels": [0, 1, 0, 0], "source": "synthetic"}

Labels are a 4-element binary vector:
    [Art_5_1_a_Deceptive, Art_5_1_b_Age, Art_5_1_b_SocioEcon, Art_5_1_c_Social_Scoring]

Usage:
    python3 scripts/r2_prepare_dataset.py
    python3 scripts/r2_prepare_dataset.py --semeval data/r2_training/semeval_mapped.jsonl
"""

import argparse
import json
import os
import random
import sys
from collections import Counter
from pathlib import Path

CATEGORIES = [
    "Art_5_1_a_Deceptive",
    "Art_5_1_b_Age",
    "Art_5_1_b_SocioEcon",
    "Art_5_1_c_Social_Scoring",
]

CAT_TO_IDX = {cat: i for i, cat in enumerate(CATEGORIES)}

SYNTHETIC_FILE = "data/ai_safety_cleaned.jsonl"
OUTPUT_DIR = "data/r2_training"

SEED = 42
TRAIN_RATIO = 0.70
VAL_RATIO = 0.15
TEST_RATIO = 0.15


def text_from_entry(entry: dict) -> str:
    """Extract text content from an entry."""
    return entry.get("text") or entry.get("phrase") or ""


def is_manipulative(entry: dict) -> bool:
    """Check if an entry is manipulative (positive label)."""
    return entry.get("label") != "benign"


def entry_to_training(entry: dict, source: str = "synthetic") -> dict | None:
    """Convert a raw JSONL entry to training format.

    Returns:
        {"text": str, "labels": [int, int, int, int], "source": str}
        or None if the entry can't be converted.
    """
    text = text_from_entry(entry)
    if not text or len(text.strip()) < 5:
        return None

    labels = [0, 0, 0, 0]

    if is_manipulative(entry):
        cat = entry.get("category", "")
        if cat in CAT_TO_IDX:
            labels[CAT_TO_IDX[cat]] = 1
        else:
            return None  # Unknown category
    # Benign entries get all-zeros label vector

    return {
        "text": text.strip(),
        "labels": labels,
        "source": source,
    }


def load_synthetic(path: str) -> list[dict]:
    """Load and convert synthetic data."""
    entries = []
    with open(path, "r", encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            raw = json.loads(line)
            converted = entry_to_training(raw, "synthetic")
            if converted:
                entries.append(converted)
    return entries


def load_semeval(path: str) -> list[dict]:
    """Load and convert SemEval-mapped data."""
    entries = []
    with open(path, "r", encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            raw = json.loads(line)
            converted = entry_to_training(raw, "semeval")
            if converted:
                entries.append(converted)
    return entries


def stratified_split(
    entries: list[dict],
    train_ratio: float,
    val_ratio: float,
    seed: int,
) -> tuple[list[dict], list[dict], list[dict]]:
    """Split entries into train/val/test with stratification by label pattern.

    Stratifies on the label vector to ensure each split has a proportional
    representation of each category and benign examples.
    """
    rng = random.Random(seed)

    # Group by label pattern
    groups: dict[str, list[dict]] = {}
    for entry in entries:
        key = str(entry["labels"])
        if key not in groups:
            groups[key] = []
        groups[key].append(entry)

    train, val, test = [], [], []

    for key, group in groups.items():
        rng.shuffle(group)
        n = len(group)
        n_train = max(1, int(n * train_ratio))
        n_val = max(1, int(n * val_ratio)) if n > 2 else 0
        # Ensure at least 1 in train
        if n_train + n_val >= n:
            n_val = max(0, n - n_train - 1)

        train.extend(group[:n_train])
        val.extend(group[n_train:n_train + n_val])
        test.extend(group[n_train + n_val:])

    # Final shuffle within each split
    rng.shuffle(train)
    rng.shuffle(val)
    rng.shuffle(test)

    return train, val, test


def write_split(entries: list[dict], path: str):
    """Write entries to a JSONL file."""
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as f:
        for entry in entries:
            f.write(json.dumps(entry, ensure_ascii=False) + "\n")


def print_stats(name: str, entries: list[dict]):
    """Print statistics for a split."""
    n = len(entries)
    if n == 0:
        print(f"  {name}: 0 entries")
        return

    # Count by label pattern
    benign = sum(1 for e in entries if e["labels"] == [0, 0, 0, 0])
    manip = n - benign

    # Count per category (can overlap for multi-label)
    per_cat = [0, 0, 0, 0]
    for e in entries:
        for i in range(4):
            per_cat[i] += e["labels"][i]

    # Source distribution
    sources = Counter(e["source"] for e in entries)

    print(f"  {name}: {n} entries ({manip} manipulative, {benign} benign)")
    for i, cat in enumerate(CATEGORIES):
        short = cat.replace("Art_5_1_", "").replace("_Deceptive", "(a)").replace("_Age", "(b_Age)").replace("_SocioEcon", "(b_SE)").replace("_Social_Scoring", "(c)")
        print(f"    {short}: {per_cat[i]}")
    for src, count in sorted(sources.items()):
        print(f"    source={src}: {count}")


def main():
    parser = argparse.ArgumentParser(description="Prepare R2 training dataset")
    parser.add_argument(
        "--synthetic", default=SYNTHETIC_FILE,
        help="Path to cleaned synthetic JSONL",
    )
    parser.add_argument(
        "--semeval", default=None,
        help="Optional path to SemEval-mapped JSONL",
    )
    parser.add_argument(
        "--output-dir", default=OUTPUT_DIR,
        help="Output directory for split files",
    )
    parser.add_argument(
        "--seed", type=int, default=SEED,
        help="Random seed for reproducible splits",
    )
    parser.add_argument(
        "--semeval-weight", type=float, default=0.5,
        help="Downsample SemEval to this fraction (domain adaptation)",
    )
    args = parser.parse_args()

    print("=== R2 Training Dataset Preparation ===\n")

    # Load synthetic data
    print(f"Loading synthetic data from {args.synthetic}...")
    synthetic = load_synthetic(args.synthetic)
    print(f"  Loaded {len(synthetic)} entries")

    # Load optional SemEval data
    semeval = []
    if args.semeval and os.path.exists(args.semeval):
        print(f"Loading SemEval data from {args.semeval}...")
        semeval_full = load_semeval(args.semeval)
        print(f"  Loaded {len(semeval_full)} entries")

        # Downsample for domain adaptation
        rng = random.Random(args.seed)
        n_keep = int(len(semeval_full) * args.semeval_weight)
        rng.shuffle(semeval_full)
        semeval = semeval_full[:n_keep]
        print(f"  Downsampled to {len(semeval)} ({args.semeval_weight:.0%})")
    elif args.semeval:
        print(f"SemEval file not found: {args.semeval}")
        print("  Proceeding with synthetic data only.")

    # Combine
    combined = synthetic + semeval
    print(f"\nCombined: {len(combined)} entries")

    # Deduplicate by text
    seen = set()
    deduped = []
    for entry in combined:
        key = entry["text"].strip().lower()
        if key not in seen:
            seen.add(key)
            deduped.append(entry)
    n_dupes = len(combined) - len(deduped)
    if n_dupes > 0:
        print(f"  Removed {n_dupes} cross-source duplicates")
    combined = deduped
    print(f"  Final: {len(combined)} entries")

    # Split
    print(f"\nSplitting ({TRAIN_RATIO:.0%}/{VAL_RATIO:.0%}/{TEST_RATIO:.0%})...")
    train, val, test = stratified_split(combined, TRAIN_RATIO, VAL_RATIO, args.seed)

    # Write
    train_path = os.path.join(args.output_dir, "train.jsonl")
    val_path = os.path.join(args.output_dir, "val.jsonl")
    test_path = os.path.join(args.output_dir, "test.jsonl")

    write_split(train, train_path)
    write_split(val, val_path)
    write_split(test, test_path)

    # Stats
    print(f"\n=== Split Statistics ===")
    print_stats("Train", train)
    print_stats("Val", val)
    print_stats("Test", test)

    print(f"\n=== Output Files ===")
    print(f"  {train_path} ({len(train)} entries)")
    print(f"  {val_path} ({len(val)} entries)")
    print(f"  {test_path} ({len(test)} entries)")

    # Write metadata
    meta = {
        "seed": args.seed,
        "synthetic_file": args.synthetic,
        "semeval_file": args.semeval,
        "semeval_weight": args.semeval_weight,
        "split_ratios": {"train": TRAIN_RATIO, "val": VAL_RATIO, "test": TEST_RATIO},
        "counts": {"train": len(train), "val": len(val), "test": len(test), "total": len(combined)},
        "categories": CATEGORIES,
        "label_format": "multi-label binary vector [a_Deceptive, b_Age, b_SocioEcon, c_Social_Scoring]",
    }
    meta_path = os.path.join(args.output_dir, "dataset_meta.json")
    with open(meta_path, "w") as f:
        json.dump(meta, f, indent=2)
    print(f"  {meta_path}")


if __name__ == "__main__":
    main()
