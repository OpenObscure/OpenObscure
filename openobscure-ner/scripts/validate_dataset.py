#!/usr/bin/env python3
"""
Validate the fine-tuning dataset for correctness.

Checks:
  1. Each line is valid JSON with required keys
  2. tokens and ner_tags have matching lengths
  3. All tags are valid BIO labels from the label schema
  4. BIO sequence validity (I-X must follow B-X or I-X of same type)
  5. Entity distribution summary
"""

import json
import sys
from pathlib import Path

VALID_TAGS = {
    "O",
    "B-PER",
    "I-PER",
    "B-LOC",
    "I-LOC",
    "B-ORG",
    "I-ORG",
    "B-HEALTH",
    "I-HEALTH",
    "B-CHILD",
    "I-CHILD",
}


def validate_bio_sequence(tags):
    """Check that I-X tags properly follow B-X or I-X of the same type."""
    errors = []
    prev_type = None
    for i, tag in enumerate(tags):
        if tag.startswith("I-"):
            entity_type = tag[2:]
            if prev_type != entity_type:
                errors.append(
                    f"  Position {i}: {tag} without preceding B-{entity_type} or I-{entity_type} (prev was {tags[i - 1] if i > 0 else 'start'})"
                )
        if tag.startswith("B-"):
            prev_type = tag[2:]
        elif tag.startswith("I-"):
            pass  # keep prev_type
        else:
            prev_type = None
    return errors


def main():
    if len(sys.argv) > 1:
        dataset_path = Path(sys.argv[1])
    else:
        dataset_path = (
            Path(__file__).parent.parent / "data" / "pii_finetune_dataset.jsonl"
        )

    if not dataset_path.exists():
        print(f"ERROR: Dataset not found at {dataset_path}")
        sys.exit(1)

    print(f"Validating: {dataset_path}")

    total = 0
    errors = 0
    warnings = 0
    entity_counts = {}
    token_lengths = []

    with open(dataset_path) as f:
        for line_num, line in enumerate(f, 1):
            line = line.strip()
            if not line:
                continue
            total += 1

            try:
                record = json.loads(line)
            except json.JSONDecodeError as e:
                print(f"ERROR line {line_num}: Invalid JSON: {e}")
                errors += 1
                continue

            if "tokens" not in record or "ner_tags" not in record:
                print(f"ERROR line {line_num}: Missing 'tokens' or 'ner_tags' key")
                errors += 1
                continue

            tokens = record["tokens"]
            tags = record["ner_tags"]

            if len(tokens) != len(tags):
                print(
                    f"ERROR line {line_num}: Length mismatch: {len(tokens)} tokens vs {len(tags)} tags"
                )
                errors += 1
                continue

            token_lengths.append(len(tokens))

            # Check tags are valid
            for i, tag in enumerate(tags):
                if tag not in VALID_TAGS:
                    print(f"ERROR line {line_num}: Invalid tag '{tag}' at position {i}")
                    errors += 1

            # Check BIO validity
            bio_errors = validate_bio_sequence(tags)
            if bio_errors:
                # BIO violations are warnings (the model can still learn from them)
                for err in bio_errors:
                    print(f"WARNING line {line_num}: BIO violation: {err}")
                    warnings += 1

            # Count entities
            for tag in tags:
                if tag.startswith("B-"):
                    etype = tag[2:]
                    entity_counts[etype] = entity_counts.get(etype, 0) + 1

    # Summary
    print("\n=== Validation Summary ===")
    print(f"Total samples: {total}")
    print(f"Errors: {errors}")
    print(f"Warnings: {warnings}")
    print(
        f"Token length: min={min(token_lengths)}, max={max(token_lengths)}, avg={sum(token_lengths) / len(token_lengths):.1f}"
    )

    print("\nEntity distribution:")
    total_entities = sum(entity_counts.values())
    for etype, count in sorted(entity_counts.items(), key=lambda x: -x[1]):
        pct = 100.0 * count / total_entities if total_entities > 0 else 0
        print(f"  {etype}: {count} ({pct:.1f}%)")
    print(f"  Total entities: {total_entities}")

    print(
        f"\nNegative samples (all O): ~{total - len([e for e in entity_counts.values()])}"
    )

    if errors > 0:
        print(f"\nFAILED: {errors} errors found")
        sys.exit(1)
    else:
        print("\nPASSED: Dataset is valid")


if __name__ == "__main__":
    main()
