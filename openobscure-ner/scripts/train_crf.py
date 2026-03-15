#!/usr/bin/env python3
"""
Train a linear-chain CRF for NER on OpenObscure's custom label schema.

Produces a `crf_model.json` file compatible with the Rust CRF scanner
(openobscure-core/src/crf_scanner.rs). The model uses hand-crafted features
(word shape, prefix/suffix, capitalization, gazetteers, context window)
and learns state feature weights + transition weights via L-BFGS.

Requires: sklearn-crfsuite (pip install sklearn-crfsuite)

Usage:
  python scripts/train_crf.py \
    --train_data data/custom_health_child.jsonl \
    --output_dir models/crf \
    [--max_iterations 100] \
    [--c1 0.1] [--c2 0.1]
"""

import argparse
import json
import logging
import os
import sys
from collections import defaultdict

logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")
logger = logging.getLogger(__name__)

# Same 11-label BIO schema as NER scanner and crf_scanner.rs
LABEL_LIST = [
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
]
NUM_LABELS = len(LABEL_LIST)
LABEL2ID = {label: i for i, label in enumerate(LABEL_LIST)}

# Gazetteer terms (subset — matches KeywordDict in Rust)
HEALTH_TERMS = {
    "diabetes",
    "asthma",
    "hypertension",
    "cancer",
    "depression",
    "anxiety",
    "adhd",
    "metformin",
    "lisinopril",
    "sertraline",
    "albuterol",
    "insulin",
    "chemotherapy",
    "mri",
    "x-ray",
    "prescription",
    "diagnosis",
    "symptoms",
    "fever",
    "autism",
    "herniated",
    "oncologist",
    "pediatrician",
    "dental",
    "appointment",
    "lab",
    "results",
    "hospital",
}

CHILD_TERMS = {
    "daughter",
    "son",
    "toddler",
    "baby",
    "child",
    "teenager",
    "grandson",
    "granddaughter",
    "newborn",
    "infant",
    "kindergarten",
    "daycare",
    "preschool",
    "school",
    "pediatrician",
    "breastfeeding",
    "8-year-old",
    "5-year-old",
    "psychologist",
}


def parse_args():
    parser = argparse.ArgumentParser(description="Train CRF NER model for OpenObscure")
    parser.add_argument(
        "--train_data",
        type=str,
        default="data/custom_health_child.jsonl",
        help="Path to training data (JSONL: {tokens, ner_tags})",
    )
    parser.add_argument(
        "--output_dir",
        type=str,
        default="models/crf",
        help="Output directory for crf_model.json",
    )
    parser.add_argument(
        "--max_iterations",
        type=int,
        default=100,
        help="Maximum L-BFGS iterations",
    )
    parser.add_argument("--c1", type=float, default=0.1, help="L1 regularization")
    parser.add_argument("--c2", type=float, default=0.1, help="L2 regularization")
    return parser.parse_args()


def load_data(path):
    """Load JSONL training data."""
    sentences = []
    with open(path, "r") as f:
        for line_num, line in enumerate(f, 1):
            line = line.strip()
            if not line:
                continue
            try:
                obj = json.loads(line)
                tokens = obj["tokens"]
                tags = obj["ner_tags"]
                if len(tokens) != len(tags):
                    logger.warning(
                        "Line %d: token/tag length mismatch, skipping", line_num
                    )
                    continue
                sentences.append((tokens, tags))
            except (json.JSONDecodeError, KeyError) as e:
                logger.warning("Line %d: parse error (%s), skipping", line_num, e)
    logger.info("Loaded %d sentences from %s", len(sentences), path)
    return sentences


def word_shape(word):
    """Generate collapsed word shape (mirrors Rust crf_scanner::word_shape)."""
    shape = []
    last_type = None
    for c in word:
        if c.isupper():
            t = "X"
        elif c.islower():
            t = "x"
        elif c.isdigit():
            t = "d"
        else:
            t = c
        if t != last_type:
            shape.append(t)
            last_type = t
    return "".join(shape)


def extract_features(tokens, idx):
    """
    Extract features for token at idx. Must match crf_scanner.rs exactly.

    Features:
    - word=<lower>, shape=<shape>
    - p1/p2/p3, s1/s2/s3 (prefix/suffix 1-3 chars)
    - isupper, istitle, isdigit, isalpha
    - len=<bucket>
    - gaz=health, gaz=child
    - BOS/EOS, -1:word, -1:shape, +1:word, +1:shape
    """
    word = tokens[idx]
    lower = word.lower()
    features = []

    # Current word
    features.append(f"word={lower}")
    features.append(f"shape={word_shape(word)}")

    # Prefix/suffix 1-3
    if len(lower) >= 1:
        features.append(f"p1={lower[:1]}")
        features.append(f"s1={lower[-1:]}")
    if len(lower) >= 2:
        features.append(f"p2={lower[:2]}")
        features.append(f"s2={lower[-2:]}")
    if len(lower) >= 3:
        features.append(f"p3={lower[:3]}")
        features.append(f"s3={lower[-3:]}")

    # Capitalization
    if word.isupper():
        features.append("isupper")
    if word[0].isupper() and word[1:].islower() and len(word) > 1:
        features.append("istitle")
    if word.isdigit():
        features.append("isdigit")
    if word.isalpha():
        features.append("isalpha")

    # Length bucket
    wlen = len(lower)
    if wlen <= 2:
        features.append("len=short")
    elif wlen <= 5:
        features.append("len=medium")
    elif wlen <= 10:
        features.append("len=long")
    else:
        features.append("len=vlong")

    # Gazetteer
    if lower in HEALTH_TERMS:
        features.append("gaz=health")
    if lower in CHILD_TERMS:
        features.append("gaz=child")

    # Context (±1)
    if idx == 0:
        features.append("BOS")
    else:
        prev = tokens[idx - 1]
        features.append(f"-1:word={prev.lower()}")
        features.append(f"-1:shape={word_shape(prev)}")

    if idx == len(tokens) - 1:
        features.append("EOS")
    else:
        nxt = tokens[idx + 1]
        features.append(f"+1:word={nxt.lower()}")
        features.append(f"+1:shape={word_shape(nxt)}")

    return features


def sentences_to_features(sentences):
    """Convert sentences to feature sequences for sklearn-crfsuite."""
    X, y = [], []
    for tokens, tags in sentences:
        x_sent = []
        for i in range(len(tokens)):
            feats = extract_features(tokens, i)
            # sklearn-crfsuite expects dict features
            x_sent.append({f: 1.0 for f in feats})
        X.append(x_sent)
        y.append(tags)
    return X, y


def export_model(crf, output_dir):
    """
    Export trained CRF to crf_model.json format compatible with Rust scanner.

    Format:
    {
      "state_features": { "feature_name": [score_per_label...] },
      "transitions": [[from_i_to_j_score...]]
    }
    """
    os.makedirs(output_dir, exist_ok=True)

    # Extract state features
    state_features = defaultdict(lambda: [0.0] * NUM_LABELS)

    if hasattr(crf, "state_features_"):
        # sklearn-crfsuite stores state features as:
        # state_features_[attr_name] = {(label, attr_name): weight}
        # But the internal representation varies by version.
        # Use the transition_features_ and state_features_ attributes.
        for (label, feat_name), weight in crf.state_features_.items():
            idx = LABEL2ID.get(label, 0)
            state_features[feat_name][idx] = weight
    else:
        logger.warning("CRF model has no state_features_ — model may not be trained")

    # Extract transition weights
    transitions = [[0.0] * NUM_LABELS for _ in range(NUM_LABELS)]
    if hasattr(crf, "transition_features_"):
        for (from_label, to_label), weight in crf.transition_features_.items():
            from_idx = LABEL2ID.get(from_label, 0)
            to_idx = LABEL2ID.get(to_label, 0)
            transitions[from_idx][to_idx] = weight

    model_data = {
        "state_features": dict(state_features),
        "transitions": transitions,
    }

    output_path = os.path.join(output_dir, "crf_model.json")
    with open(output_path, "w") as f:
        json.dump(model_data, f, indent=2)

    # Stats
    num_features = len(state_features)
    non_zero_transitions = sum(
        1 for row in transitions for val in row if abs(val) > 1e-10
    )
    file_size = os.path.getsize(output_path) / 1024

    logger.info("Exported CRF model: %s", output_path)
    logger.info("  State features: %d", num_features)
    logger.info("  Non-zero transitions: %d / %d", non_zero_transitions, NUM_LABELS**2)
    logger.info("  File size: %.1f KB", file_size)

    return output_path


def evaluate(crf, X_test, y_test):
    """Evaluate CRF and print per-entity metrics."""
    from sklearn_crfsuite import metrics

    y_pred = crf.predict(X_test)

    # Entity-level metrics (exclude O)
    entity_labels = [l for l in LABEL_LIST if l != "O"]
    report = metrics.flat_classification_report(
        y_test, y_pred, labels=entity_labels, digits=3
    )
    logger.info("Classification report:\n%s", report)


def main():
    args = parse_args()

    # Check dependency
    try:
        import sklearn_crfsuite
    except ImportError:
        logger.error(
            "sklearn-crfsuite not installed. Run: pip install sklearn-crfsuite"
        )
        sys.exit(1)

    # Load training data
    sentences = load_data(args.train_data)
    if not sentences:
        logger.error("No training data loaded")
        sys.exit(1)

    # Convert to features
    X, y = sentences_to_features(sentences)
    logger.info("Feature sequences: %d sentences", len(X))

    # Train CRF
    logger.info(
        "Training CRF (L-BFGS, c1=%.3f, c2=%.3f, max_iter=%d)...",
        args.c1,
        args.c2,
        args.max_iterations,
    )
    crf = sklearn_crfsuite.CRF(
        algorithm="lbfgs",
        c1=args.c1,
        c2=args.c2,
        max_iterations=args.max_iterations,
        all_possible_transitions=True,
        all_possible_states=True,
    )
    crf.fit(X, y)
    logger.info("Training complete")

    # Evaluate on training data (small dataset — no separate test split)
    evaluate(crf, X, y)

    # Export to JSON format for Rust scanner
    export_model(crf, args.output_dir)

    logger.info("=== CRF model ready at %s ===", args.output_dir)
    logger.info(
        'Configure in openobscure.toml: scanner.crf_model_dir = "%s"', args.output_dir
    )


if __name__ == "__main__":
    main()
