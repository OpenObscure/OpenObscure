#!/usr/bin/env python3
"""
Generate a mock CRF model JSON for development and testing.

Produces a `crf_model.json` with hand-tuned weights that demonstrate
the CRF scanner's capabilities without requiring actual training.
The mock model assigns reasonable weights to common features so it
produces plausible (but not production-quality) entity predictions.

No dependencies beyond Python stdlib + json.

Usage:
  python test/scripts/synthetic/generate_mock_crf_model.py --output_dir models/crf_mock
"""

import argparse
import json
import logging
import os

logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")
logger = logging.getLogger(__name__)

# Same 11-label BIO schema
LABELS = [
    "O",           # 0
    "B-PER",       # 1
    "I-PER",       # 2
    "B-LOC",       # 3
    "I-LOC",       # 4
    "B-ORG",       # 5
    "I-ORG",       # 6
    "B-HEALTH",    # 7
    "I-HEALTH",    # 8
    "B-CHILD",     # 9
    "I-CHILD",     # 10
]
NUM_LABELS = len(LABELS)
L = {name: i for i, name in enumerate(LABELS)}


def parse_args():
    parser = argparse.ArgumentParser(description="Generate mock CRF model")
    parser.add_argument(
        "--output_dir",
        type=str,
        default="models/crf_mock",
        help="Output directory for crf_model.json",
    )
    return parser.parse_args()


def zeros():
    return [0.0] * NUM_LABELS


def build_state_features():
    """Build state feature weights that produce plausible NER predictions."""
    features = {}

    # --- PERSON features ---
    person_names = [
        "john", "smith", "johnson", "sarah", "michael", "emily", "david",
        "james", "robert", "mary", "william", "jones", "brown", "davis",
    ]
    for name in person_names:
        w = zeros()
        w[L["B-PER"]] = 2.5
        features[f"word={name}"] = w

    # Title case → likely person or location
    w = zeros()
    w[L["B-PER"]] = 1.2
    w[L["B-LOC"]] = 0.5
    w[L["B-ORG"]] = 0.5
    features["istitle"] = w

    # All-caps → org or acronym
    w = zeros()
    w[L["B-ORG"]] = 0.8
    w[L["B-HEALTH"]] = 0.3  # medical acronyms like ADHD, MRI
    features["isupper"] = w

    # --- LOCATION features ---
    location_words = [
        "street", "avenue", "road", "drive", "boulevard", "lane", "place",
        "city", "state", "country", "town", "village",
    ]
    for word in location_words:
        w = zeros()
        w[L["I-LOC"]] = 2.0
        w[L["B-LOC"]] = 0.5
        features[f"word={word}"] = w

    location_names = [
        "springfield", "portland", "oregon", "california", "new", "york",
    ]
    for name in location_names:
        w = zeros()
        w[L["B-LOC"]] = 2.5
        features[f"word={name}"] = w

    # Numbers preceding location words (addresses)
    w = zeros()
    w[L["B-LOC"]] = 0.8
    features["isdigit"] = w

    # --- ORGANIZATION features ---
    org_words = [
        "corporation", "company", "inc", "ltd", "llc", "university",
        "hospital", "clinic", "academy", "institute", "foundation",
    ]
    for word in org_words:
        w = zeros()
        w[L["I-ORG"]] = 2.0
        w[L["B-ORG"]] = 0.8
        features[f"word={word}"] = w

    # --- HEALTH features ---
    # Gazetteer feature is the strongest signal
    w = zeros()
    w[L["B-HEALTH"]] = 3.0
    features["gaz=health"] = w

    health_words = [
        "diabetes", "hypertension", "cancer", "depression", "anxiety",
        "asthma", "adhd", "autism", "metformin", "lisinopril", "sertraline",
        "albuterol", "insulin", "chemotherapy", "herniated", "fever",
        "prescription", "diagnosis", "symptoms", "mri", "x-ray",
    ]
    for word in health_words:
        w = zeros()
        w[L["B-HEALTH"]] = 2.5
        features[f"word={word}"] = w

    # Multi-word health phrases (I-HEALTH triggers)
    health_continuations = [
        "pressure", "disorder", "disease", "syndrome", "failure",
        "attack", "infection", "therapy", "treatment", "surgery",
        "disc", "results", "appointment",
    ]
    for word in health_continuations:
        w = zeros()
        w[L["I-HEALTH"]] = 1.5
        w[L["B-HEALTH"]] = 0.5
        features[f"word={word}"] = w

    # --- CHILD features ---
    w = zeros()
    w[L["B-CHILD"]] = 3.0
    features["gaz=child"] = w

    child_words = [
        "daughter", "son", "toddler", "baby", "child", "teenager",
        "grandson", "granddaughter", "newborn", "infant",
    ]
    for word in child_words:
        w = zeros()
        w[L["B-CHILD"]] = 2.5
        features[f"word={word}"] = w

    child_context_words = [
        "kindergarten", "daycare", "preschool", "school", "breastfeeding",
    ]
    for word in child_context_words:
        w = zeros()
        w[L["B-CHILD"]] = 1.5
        features[f"word={word}"] = w

    # Age-pattern words
    age_patterns = ["8-year-old", "5-year-old", "3-year-old", "10-year-old"]
    for word in age_patterns:
        w = zeros()
        w[L["B-CHILD"]] = 2.5
        features[f"word={word}"] = w

    # --- Shape features ---
    # Xx shape (title case) — person/location
    w = zeros()
    w[L["B-PER"]] = 0.5
    w[L["B-LOC"]] = 0.3
    features["shape=Xx"] = w

    # X shape (all caps)
    w = zeros()
    w[L["B-ORG"]] = 0.4
    features["shape=X"] = w

    # d-x-x shape (age patterns like 8-year-old)
    w = zeros()
    w[L["B-CHILD"]] = 1.0
    features["shape=d-x-x"] = w

    # --- Context features ---
    # "My" preceding → child/health context
    w = zeros()
    w[L["B-CHILD"]] = 0.8
    w[L["B-PER"]] = 0.3
    features["-1:word=my"] = w

    # "Dr." preceding → person
    w = zeros()
    w[L["B-PER"]] = 1.5
    features["-1:word=dr."] = w

    # "at" preceding → location or organization
    w = zeros()
    w[L["B-LOC"]] = 0.5
    w[L["B-ORG"]] = 0.5
    features["-1:word=at"] = w

    # "has" / "have" preceding → health
    for word in ["has", "have", "with"]:
        w = zeros()
        w[L["B-HEALTH"]] = 0.5
        features[f"-1:word={word}"] = w

    # --- O (negative) features ---
    # Common non-entity words get negative entity weights
    non_entity_words = [
        "the", "a", "an", "is", "was", "has", "have", "had", "and", "or",
        "but", "in", "at", "to", "for", "of", "on", "with", "by", "from",
        "i", "me", "my", "we", "our", "this", "that", "these", "those",
    ]
    for word in non_entity_words:
        if f"word={word}" not in features:
            w = zeros()
            w[L["O"]] = 1.5
            features[f"word={word}"] = w

    # Short words are less likely entities
    w = zeros()
    w[L["O"]] = 0.5
    features["len=short"] = w

    # BOS/EOS bias toward O
    w = zeros()
    w[L["O"]] = 0.3
    features["BOS"] = w
    features["EOS"] = w.copy()

    return features


def build_transitions():
    """Build transition weight matrix with BIO constraints."""
    trans = [[0.0] * NUM_LABELS for _ in range(NUM_LABELS)]

    # O → O is common
    trans[L["O"]][L["O"]] = 1.0

    # O → B-* is neutral/slightly positive
    for label in LABELS:
        if label.startswith("B-"):
            trans[L["O"]][L[label]] = 0.0

    # B-* → I-* of same type is strongly positive
    type_pairs = [
        ("B-PER", "I-PER"), ("B-LOC", "I-LOC"), ("B-ORG", "I-ORG"),
        ("B-HEALTH", "I-HEALTH"), ("B-CHILD", "I-CHILD"),
    ]
    for b, i in type_pairs:
        trans[L[b]][L[i]] = 2.0

    # I-* → I-* of same type is positive (continuation)
    i_pairs = [
        ("I-PER", "I-PER"), ("I-LOC", "I-LOC"), ("I-ORG", "I-ORG"),
        ("I-HEALTH", "I-HEALTH"), ("I-CHILD", "I-CHILD"),
    ]
    for i1, i2 in i_pairs:
        trans[L[i1]][L[i2]] = 1.5

    # B/I → O is slightly positive (entities end)
    for label in LABELS:
        if label != "O":
            trans[L[label]][L["O"]] = 0.5

    # Cross-type B→I and I→I transitions are negative (invalid BIO)
    for i in range(NUM_LABELS):
        for j in range(NUM_LABELS):
            if trans[i][j] == 0.0 and i != j:
                # Penalize: O→I-* (invalid), cross-type I, etc.
                if LABELS[j].startswith("I-"):
                    # O→I is invalid
                    if LABELS[i] == "O":
                        trans[i][j] = -2.0
                    else:
                        # Cross-type continuation
                        trans[i][j] = -1.0
                else:
                    trans[i][j] = -0.3

    return trans


def main():
    args = parse_args()
    os.makedirs(args.output_dir, exist_ok=True)

    logger.info("=== Generating Mock CRF Model ===")

    state_features = build_state_features()
    transitions = build_transitions()

    model_data = {
        "state_features": state_features,
        "transitions": transitions,
    }

    output_path = os.path.join(args.output_dir, "crf_model.json")
    with open(output_path, "w") as f:
        json.dump(model_data, f, indent=2)

    file_size = os.path.getsize(output_path) / 1024
    logger.info("Mock CRF model: %s (%.1f KB)", output_path, file_size)
    logger.info("  State features: %d", len(state_features))
    logger.info("  Transition matrix: %dx%d", NUM_LABELS, NUM_LABELS)

    # Quick validation
    for feat_name, weights in state_features.items():
        assert len(weights) == NUM_LABELS, f"Feature {feat_name} has wrong length"
    assert len(transitions) == NUM_LABELS
    for row in transitions:
        assert len(row) == NUM_LABELS

    logger.info("Validation passed")
    logger.info("=== Mock CRF model ready at %s ===", args.output_dir)
    logger.info("Configure: scanner.crf_model_dir = \"%s\"", args.output_dir)


if __name__ == "__main__":
    main()
