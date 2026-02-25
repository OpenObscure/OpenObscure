#!/usr/bin/env python3
"""12B: Baseline evaluation and gap analysis for R2 cognitive firewall.

Two-directional evaluation:
  Part A: Our R2 TinyBERT (ONNX) on SemEval-2020 mapped paragraphs
  Part B: Auto-generate gap analysis report

Usage:
    python3 scripts/r2_eval_semeval_baseline.py
    python3 scripts/r2_eval_semeval_baseline.py --threshold 0.55
"""

import argparse
import json
import os
import sys
from collections import Counter
from pathlib import Path

import numpy as np

try:
    import onnxruntime as ort
    from transformers import AutoTokenizer
except ImportError as e:
    print(f"Missing dependency: {e}")
    print("Install with: pip install onnxruntime transformers")
    sys.exit(1)

SCRIPT_DIR = Path(__file__).resolve().parent.parent

CATEGORIES = [
    "Art_5_1_a_Deceptive",
    "Art_5_1_b_Age",
    "Art_5_1_b_SocioEcon",
    "Art_5_1_c_Social_Scoring",
]
CAT_SHORT = ["a_Deceptive", "b_Age", "b_SocioEcon", "c_Social_Scoring"]

MODEL_NAME = "huawei-noah/TinyBERT_General_4L_312D"
MAX_LENGTH = 512


def sigmoid(x):
    return 1.0 / (1.0 + np.exp(-x))


def load_jsonl(path: str) -> list[dict]:
    entries = []
    with open(path, "r", encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if line:
                entries.append(json.loads(line))
    return entries


def run_onnx_inference(session, tokenizer, texts: list[str],
                       threshold: float, batch_size: int = 32) -> np.ndarray:
    """Run ONNX inference on a list of texts. Returns (N, 4) binary predictions."""
    all_preds = []

    for i in range(0, len(texts), batch_size):
        batch_texts = texts[i:i + batch_size]
        encoding = tokenizer(
            batch_texts,
            max_length=MAX_LENGTH,
            padding="max_length",
            truncation=True,
            return_tensors="np",
        )

        feed = {
            "input_ids": encoding["input_ids"].astype(np.int64),
            "attention_mask": encoding["attention_mask"].astype(np.int64),
        }
        # Add token_type_ids if the model expects it
        input_names = [inp.name for inp in session.get_inputs()]
        if "token_type_ids" in input_names:
            if "token_type_ids" in encoding:
                feed["token_type_ids"] = encoding["token_type_ids"].astype(np.int64)
            else:
                feed["token_type_ids"] = np.zeros_like(feed["input_ids"])

        logits = session.run(None, feed)[0]
        probs = sigmoid(logits)
        preds = (probs >= threshold).astype(int)
        all_preds.append(preds)

    return np.vstack(all_preds)


def compute_metrics(y_true: np.ndarray, y_pred: np.ndarray) -> dict:
    """Compute per-category and macro P/R/F1."""
    results = {}
    for i, cat in enumerate(CAT_SHORT):
        tp = ((y_true[:, i] == 1) & (y_pred[:, i] == 1)).sum()
        fp = ((y_true[:, i] == 0) & (y_pred[:, i] == 1)).sum()
        fn = ((y_true[:, i] == 1) & (y_pred[:, i] == 0)).sum()

        p = tp / (tp + fp) if (tp + fp) > 0 else 0.0
        r = tp / (tp + fn) if (tp + fn) > 0 else 0.0
        f1 = 2 * p * r / (p + r) if (p + r) > 0 else 0.0

        results[cat] = {
            "precision": float(p),
            "recall": float(r),
            "f1": float(f1),
            "tp": int(tp), "fp": int(fp), "fn": int(fn),
            "support": int(y_true[:, i].sum()),
        }

    # Macro averages (only over categories with support > 0)
    active = [cat for cat in CAT_SHORT if results[cat]["support"] > 0]
    if active:
        results["macro_precision"] = float(np.mean([results[c]["precision"] for c in active]))
        results["macro_recall"] = float(np.mean([results[c]["recall"] for c in active]))
        results["macro_f1"] = float(np.mean([results[c]["f1"] for c in active]))
    else:
        results["macro_precision"] = 0.0
        results["macro_recall"] = 0.0
        results["macro_f1"] = 0.0

    # Benign accuracy
    benign_mask = y_true.sum(axis=1) == 0
    if benign_mask.sum() > 0:
        benign_correct = (y_pred[benign_mask].sum(axis=1) == 0)
        results["benign_accuracy"] = float(benign_correct.mean())
        results["benign_count"] = int(benign_mask.sum())
    else:
        results["benign_accuracy"] = None
        results["benign_count"] = 0

    return results


def eval_r2_on_semeval(model_path: str, tokenizer, semeval_mapped: list[dict],
                       semeval_benign: list[dict], threshold: float) -> dict:
    """Part A: Our R2 model on SemEval-2020 data."""
    print("\n=== Part A: R2 TinyBERT on SemEval-2020 Data ===")

    session = ort.InferenceSession(model_path)
    print(f"  Model: {model_path}")
    print(f"  Threshold: {threshold}")

    # Build texts and ground-truth labels
    texts = []
    labels = []

    for entry in semeval_mapped:
        texts.append(entry["text"])
        # SemEval-2020 maps only to Art_5_1_a_Deceptive
        label_vec = [1, 0, 0, 0]
        labels.append(label_vec)

    for entry in semeval_benign:
        texts.append(entry["text"])
        labels.append([0, 0, 0, 0])

    y_true = np.array(labels)
    print(f"  Samples: {len(texts)} ({len(semeval_mapped)} manipulative, {len(semeval_benign)} benign)")

    # Run inference
    y_pred = run_onnx_inference(session, tokenizer, texts, threshold)

    # Compute metrics
    metrics = compute_metrics(y_true, y_pred)

    # Print results
    print(f"\n  Per-category results:")
    for cat in CAT_SHORT:
        m = metrics[cat]
        if m["support"] > 0:
            print(f"    {cat:<20} P={m['precision']:.3f}  R={m['recall']:.3f}  F1={m['f1']:.3f}  (support={m['support']})")
        else:
            print(f"    {cat:<20} (no support in SemEval-2020)")

    print(f"\n  Macro (active categories): P={metrics['macro_precision']:.3f}  R={metrics['macro_recall']:.3f}  F1={metrics['macro_f1']:.3f}")
    if metrics["benign_accuracy"] is not None:
        print(f"  Benign accuracy: {metrics['benign_accuracy']:.3f} ({metrics['benign_count']} samples)")

    # Subcategory analysis for mapped entries
    subcat_stats = Counter()
    for i, entry in enumerate(semeval_mapped):
        pred = y_pred[i]
        if pred[0] == 1:
            for sc in entry.get("subcategories", []):
                subcat_stats[f"{sc}_tp"] += 1
        else:
            for sc in entry.get("subcategories", []):
                subcat_stats[f"{sc}_fn"] += 1

    print(f"\n  By original subcategory (recall):")
    subcats = set()
    for k in subcat_stats:
        subcats.add(k.rsplit("_", 1)[0])
    for sc in sorted(subcats):
        tp = subcat_stats.get(f"{sc}_tp", 0)
        fn = subcat_stats.get(f"{sc}_fn", 0)
        total = tp + fn
        recall = tp / total if total > 0 else 0
        print(f"    {sc:<25} {recall:.3f} ({tp}/{total})")

    metrics["subcat_recall"] = {}
    for sc in sorted(subcats):
        tp = subcat_stats.get(f"{sc}_tp", 0)
        fn = subcat_stats.get(f"{sc}_fn", 0)
        total = tp + fn
        metrics["subcat_recall"][sc] = {
            "recall": float(tp / total) if total > 0 else 0.0,
            "tp": tp, "total": total,
        }

    return metrics


def eval_r2_on_original(model_path: str, tokenizer, test_path: str,
                        threshold: float) -> dict:
    """Part B: Our R2 model on original synthetic test data (reference)."""
    print("\n=== Part B: R2 TinyBERT on Original Test Data (reference) ===")

    session = ort.InferenceSession(model_path)

    entries = load_jsonl(test_path)
    texts = [e["text"] for e in entries]
    y_true = np.array([e["labels"] for e in entries])

    print(f"  Test samples: {len(entries)}")

    y_pred = run_onnx_inference(session, tokenizer, texts, threshold)
    metrics = compute_metrics(y_true, y_pred)

    print(f"\n  Per-category results:")
    for cat in CAT_SHORT:
        m = metrics[cat]
        if m["support"] > 0:
            print(f"    {cat:<20} P={m['precision']:.3f}  R={m['recall']:.3f}  F1={m['f1']:.3f}  (support={m['support']})")

    print(f"\n  Macro: P={metrics['macro_precision']:.3f}  R={metrics['macro_recall']:.3f}  F1={metrics['macro_f1']:.3f}")
    if metrics["benign_accuracy"] is not None:
        print(f"  Benign accuracy: {metrics['benign_accuracy']:.3f}")

    return metrics


def generate_gap_analysis(r2_semeval: dict, r2_original: dict,
                          semeval_mapped: list[dict], output_dir: str):
    """Part C: Generate gap analysis report."""
    print("\n=== Part C: Gap Analysis ===")

    # Compute domain gap deltas
    a_recall_original = r2_original.get("a_Deceptive", {}).get("recall", 0)
    a_recall_semeval = r2_semeval.get("a_Deceptive", {}).get("recall", 0)
    recall_gap = a_recall_original - a_recall_semeval

    a_f1_original = r2_original.get("a_Deceptive", {}).get("f1", 0)
    a_f1_semeval = r2_semeval.get("a_Deceptive", {}).get("f1", 0)
    f1_gap = a_f1_original - a_f1_semeval

    benign_acc_original = r2_original.get("benign_accuracy", 0) or 0
    benign_acc_semeval = r2_semeval.get("benign_accuracy", 0) or 0

    # Decision logic
    n_mapped = sum(1 for e in semeval_mapped if e.get("category") == "Art_5_1_a_Deceptive")
    augment_recommended = n_mapped >= 1000 and recall_gap >= 0.10
    sufficient_data = n_mapped >= 500

    print(f"  Art_5_1_a recall — original: {a_recall_original:.3f}, SemEval: {a_recall_semeval:.3f}, gap: {recall_gap:+.3f}")
    print(f"  Art_5_1_a F1     — original: {a_f1_original:.3f}, SemEval: {a_f1_semeval:.3f}, gap: {f1_gap:+.3f}")
    print(f"  Benign accuracy  — original: {benign_acc_original:.3f}, SemEval: {benign_acc_semeval:.3f}")
    print(f"  Mapped paragraphs: {n_mapped}")
    print(f"  Augmentation recommended: {augment_recommended}")

    # Write gap analysis markdown report
    report_path = os.path.join(output_dir, "gap_analysis.md")

    lines = [
        "# R2 Cognitive Firewall — Gap Analysis Report",
        "",
        f"Generated by `r2_eval_semeval_baseline.py`",
        "",
        "## Summary",
        "",
        f"- **SemEval-2020 Task 11 mapped paragraphs**: {n_mapped} manipulative + {r2_semeval.get('benign_count', 0)} benign",
        f"- **Original test set**: {sum(r2_original[c]['support'] for c in CAT_SHORT)} samples",
        f"- **Model**: TinyBERT FP32 ONNX (threshold={r2_semeval.get('threshold', 0.55)})",
        "",
        "## Side-by-Side Comparison",
        "",
        "### Art_5_1_a_Deceptive (primary category in SemEval-2020)",
        "",
        "| Metric | Original Test | SemEval-2020 | Delta |",
        "|--------|--------------|-------------|-------|",
        f"| Precision | {r2_original.get('a_Deceptive', {}).get('precision', 0):.3f} | {r2_semeval.get('a_Deceptive', {}).get('precision', 0):.3f} | {r2_semeval.get('a_Deceptive', {}).get('precision', 0) - r2_original.get('a_Deceptive', {}).get('precision', 0):+.3f} |",
        f"| Recall | {a_recall_original:.3f} | {a_recall_semeval:.3f} | {-recall_gap:+.3f} |",
        f"| F1 | {a_f1_original:.3f} | {a_f1_semeval:.3f} | {-f1_gap:+.3f} |",
        "",
        "### Macro Metrics",
        "",
        "| Metric | Original Test | SemEval-2020 | Delta |",
        "|--------|--------------|-------------|-------|",
        f"| Macro Precision | {r2_original['macro_precision']:.3f} | {r2_semeval['macro_precision']:.3f} | {r2_semeval['macro_precision'] - r2_original['macro_precision']:+.3f} |",
        f"| Macro Recall | {r2_original['macro_recall']:.3f} | {r2_semeval['macro_recall']:.3f} | {r2_semeval['macro_recall'] - r2_original['macro_recall']:+.3f} |",
        f"| Macro F1 | {r2_original['macro_f1']:.3f} | {r2_semeval['macro_f1']:.3f} | {r2_semeval['macro_f1'] - r2_original['macro_f1']:+.3f} |",
        f"| Benign Accuracy | {benign_acc_original:.3f} | {benign_acc_semeval:.3f} | {benign_acc_semeval - benign_acc_original:+.3f} |",
        "",
    ]

    # Subcategory breakdown
    subcat_recall = r2_semeval.get("subcat_recall", {})
    if subcat_recall:
        lines.extend([
            "### SemEval-2020 Subcategory Recall",
            "",
            "| Subcategory | Recall | TP/Total |",
            "|-------------|--------|----------|",
        ])
        for sc, data in sorted(subcat_recall.items(), key=lambda x: -x[1]["recall"]):
            lines.append(f"| {sc} | {data['recall']:.3f} | {data['tp']}/{data['total']} |")
        lines.append("")

    # Domain gap analysis
    lines.extend([
        "## Domain Gap Analysis",
        "",
        f"The Art_5_1_a recall gap between original test data and SemEval-2020 is **{recall_gap:+.1%}**.",
        "",
    ])

    if recall_gap > 0.10:
        lines.append(f"The gap exceeds 10%, indicating the model underperforms on real-world propaganda text "
                      f"compared to synthetic test data. Data augmentation with SemEval-2020 is recommended.")
    elif recall_gap > 0:
        lines.append(f"The gap is modest ({recall_gap:.1%}). The model generalizes reasonably well to "
                      f"real-world propaganda. Augmentation is optional but may help.")
    else:
        lines.append(f"The model performs equally well or better on SemEval-2020 than on synthetic data. "
                      f"No domain gap detected.")

    lines.extend([
        "",
        "## Augmentation Decision",
        "",
        "| Criterion | Value | Threshold | Met? |",
        "|-----------|-------|-----------|------|",
        f"| Mapped paragraphs | {n_mapped} | >= 500 | {'Yes' if sufficient_data else 'No'} |",
        f"| Art_5_1_a recall gap | {recall_gap:+.1%} | >= 10% | {'Yes' if recall_gap >= 0.10 else 'No'} |",
        "",
    ])

    if augment_recommended:
        lines.extend([
            "**Recommendation: AUGMENT**",
            "",
            f"Both criteria met. Run data augmentation with SemEval-2020 mapped data:",
            "```bash",
            ".venv/bin/python scripts/r2_prepare_dataset.py \\",
            "  --semeval data/semeval2020/semeval2020_mapped.jsonl \\",
            "  --semeval-weight 0.5 \\",
            "  --output-dir data/r2_training_augmented",
            "```",
        ])
    elif not sufficient_data:
        lines.extend([
            "**Recommendation: SKIP (insufficient data)**",
            "",
            f"Only {n_mapped} mapped paragraphs available (need >= 500). "
            f"Mark 12A.5 + 12B as COMPLETE with 'insufficient data' note.",
        ])
    else:
        lines.extend([
            "**Recommendation: SKIP (small gap)**",
            "",
            f"The recall gap ({recall_gap:+.1%}) is below the 10% threshold. "
            f"The model generalizes adequately to real propaganda text. "
            f"Mark 12B as COMPLETE without retraining.",
        ])

    lines.extend([
        "",
        "## Coverage Limitations",
        "",
        "SemEval-2020 Task 11 only maps to `Art_5_1_a_Deceptive`. No evaluation data exists for:",
        "- `Art_5_1_b_Age` (vulnerability exploitation by age)",
        "- `Art_5_1_b_SocioEcon` (vulnerability exploitation by socioeconomic status)",
        "- `Art_5_1_c_Social_Scoring` (social scoring systems)",
        "",
        "These categories rely entirely on synthetic training data.",
    ])

    os.makedirs(output_dir, exist_ok=True)
    with open(report_path, "w") as f:
        f.write("\n".join(lines) + "\n")
    print(f"  Report written: {report_path}")

    return {
        "n_mapped": n_mapped,
        "recall_gap": float(recall_gap),
        "f1_gap": float(f1_gap),
        "augment_recommended": augment_recommended,
        "sufficient_data": sufficient_data,
    }


def main():
    parser = argparse.ArgumentParser(
        description="R2 baseline evaluation and gap analysis"
    )
    parser.add_argument(
        "--model", default=str(SCRIPT_DIR / "models" / "r2_persuasion_tinybert" / "model.onnx"),
        help="Path to R2 ONNX model",
    )
    parser.add_argument(
        "--semeval-mapped", default=str(SCRIPT_DIR / "data" / "semeval2020" / "semeval2020_mapped.jsonl"),
        help="Path to SemEval-2020 mapped JSONL",
    )
    parser.add_argument(
        "--semeval-benign", default=str(SCRIPT_DIR / "data" / "semeval2020" / "semeval2020_benign.jsonl"),
        help="Path to SemEval-2020 benign JSONL",
    )
    parser.add_argument(
        "--test-data", default=str(SCRIPT_DIR / "data" / "r2_training" / "test.jsonl"),
        help="Path to original synthetic test data",
    )
    parser.add_argument(
        "--threshold", type=float, default=0.55,
        help="Classification threshold (default: 0.55, matching R2 production)",
    )
    parser.add_argument(
        "--output-dir", default=str(SCRIPT_DIR / "data" / "r2_eval"),
        help="Output directory for evaluation results",
    )
    args = parser.parse_args()

    # Validate inputs
    for path, name in [(args.model, "R2 ONNX model"),
                       (args.semeval_mapped, "SemEval mapped"),
                       (args.semeval_benign, "SemEval benign"),
                       (args.test_data, "Original test data")]:
        if not os.path.exists(path):
            print(f"ERROR: {name} not found: {path}")
            sys.exit(1)

    # Load tokenizer
    print(f"Loading tokenizer: {MODEL_NAME}")
    tokenizer = AutoTokenizer.from_pretrained(MODEL_NAME)

    # Load SemEval data
    semeval_mapped = load_jsonl(args.semeval_mapped)
    semeval_benign = load_jsonl(args.semeval_benign)
    print(f"SemEval-2020: {len(semeval_mapped)} mapped + {len(semeval_benign)} benign")

    # Part A: R2 on SemEval-2020
    r2_semeval = eval_r2_on_semeval(
        args.model, tokenizer, semeval_mapped, semeval_benign, args.threshold
    )
    r2_semeval["threshold"] = args.threshold

    # Part B: R2 on original test data
    r2_original = eval_r2_on_original(
        args.model, tokenizer, args.test_data, args.threshold
    )

    # Part C: Gap analysis
    gap = generate_gap_analysis(r2_semeval, r2_original, semeval_mapped, args.output_dir)

    # Save results
    os.makedirs(args.output_dir, exist_ok=True)

    semeval_results_path = os.path.join(args.output_dir, "r2_on_semeval_results.json")
    with open(semeval_results_path, "w") as f:
        json.dump(r2_semeval, f, indent=2)
    print(f"\nSemEval results saved: {semeval_results_path}")

    original_results_path = os.path.join(args.output_dir, "r2_on_original_results.json")
    with open(original_results_path, "w") as f:
        json.dump(r2_original, f, indent=2)
    print(f"Original results saved: {original_results_path}")

    gap_results_path = os.path.join(args.output_dir, "gap_decision.json")
    with open(gap_results_path, "w") as f:
        json.dump(gap, f, indent=2)
    print(f"Gap decision saved: {gap_results_path}")

    # Final summary
    print(f"\n{'=' * 60}")
    print(f"  DECISION: {'AUGMENT' if gap['augment_recommended'] else 'SKIP augmentation'}")
    print(f"  Art_5_1_a recall gap: {gap['recall_gap']:+.1%}")
    print(f"  Mapped paragraphs: {gap['n_mapped']}")
    print(f"{'=' * 60}")


if __name__ == "__main__":
    main()
