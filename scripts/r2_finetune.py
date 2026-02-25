#!/usr/bin/env python3
"""12C.1: Fine-tune TinyBERT for multi-label Article 5 persuasion detection.

Fine-tunes huawei-noah/TinyBERT_General_4L_312D on the R2 training dataset
for multi-label binary classification (4 Article 5 categories).

Usage:
    # Install dependencies
    pip install torch transformers datasets scikit-learn onnx onnxruntime

    # Train
    python3 scripts/r2_finetune.py

    # Train with custom params
    python3 scripts/r2_finetune.py --lr 3e-5 --epochs 5 --batch-size 32

    # Export best checkpoint to ONNX
    python3 scripts/r2_finetune.py --export-onnx models/r2_persuasion_tinybert_int8.onnx
"""

import argparse
import json
import os
import sys
from pathlib import Path

import numpy as np

try:
    import torch
    import torch.nn as nn
    from torch.utils.data import DataLoader, Dataset
    from transformers import AutoTokenizer, AutoModelForSequenceClassification
    from sklearn.metrics import precision_score, recall_score, f1_score, classification_report
except ImportError as e:
    print(f"Missing dependency: {e}")
    print("Install with: pip install torch transformers scikit-learn")
    sys.exit(1)

# --- Constants ---
MODEL_NAME = "huawei-noah/TinyBERT_General_4L_312D"
NUM_LABELS = 4
MAX_LENGTH = 512
CATEGORIES = [
    "Art_5_1_a_Deceptive",
    "Art_5_1_b_Age",
    "Art_5_1_b_SocioEcon",
    "Art_5_1_c_Social_Scoring",
]

SCRIPT_DIR = Path(__file__).resolve().parent.parent
DATA_DIR = str(SCRIPT_DIR / "data" / "r2_training")
OUTPUT_DIR = str(SCRIPT_DIR / "models" / "r2_persuasion_tinybert")


class R2Dataset(Dataset):
    """Multi-label classification dataset for R2 training."""

    def __init__(self, path: str, tokenizer, max_length: int = MAX_LENGTH):
        self.entries = []
        with open(path, "r", encoding="utf-8") as f:
            for line in f:
                line = line.strip()
                if line:
                    self.entries.append(json.loads(line))

        self.tokenizer = tokenizer
        self.max_length = max_length

    def __len__(self):
        return len(self.entries)

    def __getitem__(self, idx):
        entry = self.entries[idx]
        text = entry["text"]
        labels = entry["labels"]

        encoding = self.tokenizer(
            text,
            max_length=self.max_length,
            padding="max_length",
            truncation=True,
            return_tensors="pt",
        )

        return {
            "input_ids": encoding["input_ids"].squeeze(0),
            "attention_mask": encoding["attention_mask"].squeeze(0),
            "labels": torch.tensor(labels, dtype=torch.float32),
        }


def evaluate(model, dataloader, device, threshold=0.5):
    """Evaluate model on a dataloader. Returns metrics dict."""
    model.eval()
    all_preds = []
    all_labels = []

    with torch.no_grad():
        for batch in dataloader:
            input_ids = batch["input_ids"].to(device)
            attention_mask = batch["attention_mask"].to(device)
            labels = batch["labels"]

            outputs = model(input_ids=input_ids, attention_mask=attention_mask)
            logits = outputs.logits
            probs = torch.sigmoid(logits).cpu().numpy()
            preds = (probs >= threshold).astype(int)

            all_preds.append(preds)
            all_labels.append(labels.numpy())

    all_preds = np.vstack(all_preds)
    all_labels = np.vstack(all_labels)

    # Per-category metrics
    results = {}
    for i, cat in enumerate(CATEGORIES):
        short = cat.replace("Art_5_1_", "")
        p = precision_score(all_labels[:, i], all_preds[:, i], zero_division=0)
        r = recall_score(all_labels[:, i], all_preds[:, i], zero_division=0)
        f1 = f1_score(all_labels[:, i], all_preds[:, i], zero_division=0)
        results[short] = {"precision": p, "recall": r, "f1": f1}

    # Macro averages
    results["macro_precision"] = precision_score(all_labels, all_preds, average="macro", zero_division=0)
    results["macro_recall"] = recall_score(all_labels, all_preds, average="macro", zero_division=0)
    results["macro_f1"] = f1_score(all_labels, all_preds, average="macro", zero_division=0)

    # Benign accuracy (all-zeros prediction for all-zeros label)
    benign_mask = all_labels.sum(axis=1) == 0
    if benign_mask.sum() > 0:
        benign_preds = all_preds[benign_mask].sum(axis=1) == 0
        results["benign_accuracy"] = benign_preds.mean()
    else:
        results["benign_accuracy"] = 0.0

    return results, all_preds, all_labels


def export_onnx(model, tokenizer, output_path: str, max_length: int = MAX_LENGTH):
    """Export model to ONNX format."""
    try:
        import onnx
        from onnxruntime.quantization import quantize_dynamic, QuantType
    except ImportError:
        print("Install onnx and onnxruntime for export: pip install onnx onnxruntime")
        return

    model.eval()
    model.cpu()

    # Create dummy input (including token_type_ids for Rust compatibility)
    dummy = tokenizer(
        "This is a test sentence for ONNX export.",
        max_length=max_length,
        padding="max_length",
        truncation=True,
        return_tensors="pt",
    )
    # Ensure token_type_ids is present
    if "token_type_ids" not in dummy:
        dummy["token_type_ids"] = torch.zeros_like(dummy["input_ids"])

    # Export float32
    float_path = output_path
    os.makedirs(os.path.dirname(output_path), exist_ok=True)

    torch.onnx.export(
        model,
        (dummy["input_ids"], dummy["attention_mask"], dummy["token_type_ids"]),
        float_path,
        input_names=["input_ids", "attention_mask", "token_type_ids"],
        output_names=["logits"],
        dynamic_axes={
            "input_ids": {0: "batch_size", 1: "sequence_length"},
            "attention_mask": {0: "batch_size", 1: "sequence_length"},
            "token_type_ids": {0: "batch_size", 1: "sequence_length"},
            "logits": {0: "batch_size"},
        },
        opset_version=14,
        do_constant_folding=True,
    )

    fp32_size = os.path.getsize(float_path) / (1024 * 1024)
    print(f"  FP32 ONNX exported: {float_path} ({fp32_size:.1f} MB)")

    # Optionally quantize to INT8 (kept as separate file)
    int8_path = output_path.replace(".onnx", "_int8.onnx")
    try:
        quantize_dynamic(
            float_path,
            int8_path,
            weight_type=QuantType.QInt8,
        )
        int8_size = os.path.getsize(int8_path) / (1024 * 1024)
        print(f"  INT8 ONNX exported: {int8_path} ({int8_size:.1f} MB)")
    except Exception as e:
        print(f"  INT8 quantization failed: {e}")

    # Verify FP32 model
    import onnxruntime as ort

    session = ort.InferenceSession(float_path)
    ort_inputs = {
        "input_ids": dummy["input_ids"].numpy(),
        "attention_mask": dummy["attention_mask"].numpy(),
        "token_type_ids": dummy["token_type_ids"].numpy(),
    }
    ort_outputs = session.run(None, ort_inputs)
    print(f"  Verification: ONNX output shape {ort_outputs[0].shape}")

    # Compare with PyTorch
    with torch.no_grad():
        pt_out = model(
            dummy["input_ids"],
            attention_mask=dummy["attention_mask"],
            token_type_ids=dummy["token_type_ids"],
        )
        pt_logits = pt_out.logits.numpy()

    max_diff = np.abs(pt_logits - ort_outputs[0]).max()
    print(f"  Max PyTorch vs ONNX FP32 difference: {max_diff:.6f}")
    if max_diff < 0.01:
        print("  FP32 accuracy: OK")
    else:
        print(f"  WARNING: FP32 export error ({max_diff:.6f})")


def main():
    parser = argparse.ArgumentParser(description="Fine-tune TinyBERT for R2")
    parser.add_argument("--lr", type=float, default=3e-5, help="Learning rate")
    parser.add_argument("--epochs", type=int, default=5, help="Training epochs")
    parser.add_argument("--batch-size", type=int, default=16, help="Batch size")
    parser.add_argument("--patience", type=int, default=2, help="Early stopping patience")
    parser.add_argument("--threshold", type=float, default=0.5, help="Classification threshold")
    parser.add_argument("--pos-weight", type=float, default=1.0, help="Positive class weight (<1 = precision-biased)")
    parser.add_argument("--warmup-ratio", type=float, default=0.1, help="Warmup ratio for linear schedule")
    parser.add_argument("--data-dir", default=DATA_DIR, help="Training data directory")
    parser.add_argument("--output-dir", default=OUTPUT_DIR, help="Model output directory")
    parser.add_argument("--export-onnx", default=None, help="Export to ONNX INT8 at this path")
    parser.add_argument("--eval-only", default=None, help="Evaluate a saved checkpoint")
    args = parser.parse_args()

    if torch.cuda.is_available():
        device = torch.device("cuda")
    elif hasattr(torch.backends, "mps") and torch.backends.mps.is_available():
        device = torch.device("mps")
    else:
        device = torch.device("cpu")
    print(f"Device: {device}")

    # Load tokenizer
    print(f"Loading tokenizer: {MODEL_NAME}")
    tokenizer = AutoTokenizer.from_pretrained(MODEL_NAME)

    # ONNX export mode
    if args.export_onnx:
        checkpoint = args.eval_only or os.path.join(args.output_dir, "best")
        print(f"Loading checkpoint: {checkpoint}")
        model = AutoModelForSequenceClassification.from_pretrained(
            checkpoint, num_labels=NUM_LABELS, problem_type="multi_label_classification"
        )
        print(f"Exporting to ONNX INT8: {args.export_onnx}")
        export_onnx(model, tokenizer, args.export_onnx)
        return

    # Load datasets
    train_path = os.path.join(args.data_dir, "train.jsonl")
    val_path = os.path.join(args.data_dir, "val.jsonl")
    test_path = os.path.join(args.data_dir, "test.jsonl")

    for p in [train_path, val_path, test_path]:
        if not os.path.exists(p):
            print(f"Missing data file: {p}")
            print("Run: python3 scripts/r2_prepare_dataset.py")
            sys.exit(1)

    train_dataset = R2Dataset(train_path, tokenizer, MAX_LENGTH)
    val_dataset = R2Dataset(val_path, tokenizer, MAX_LENGTH)
    test_dataset = R2Dataset(test_path, tokenizer, MAX_LENGTH)

    print(f"Train: {len(train_dataset)}, Val: {len(val_dataset)}, Test: {len(test_dataset)}")

    train_loader = DataLoader(train_dataset, batch_size=args.batch_size, shuffle=True)
    val_loader = DataLoader(val_dataset, batch_size=args.batch_size)
    test_loader = DataLoader(test_dataset, batch_size=args.batch_size)

    # Eval-only mode
    if args.eval_only:
        print(f"Loading checkpoint: {args.eval_only}")
        model = AutoModelForSequenceClassification.from_pretrained(
            args.eval_only, num_labels=NUM_LABELS, problem_type="multi_label_classification"
        )
        model.to(device)
        results, _, _ = evaluate(model, test_loader, device, args.threshold)
        print("\n=== Test Results ===")
        for key, val in results.items():
            if isinstance(val, dict):
                print(f"  {key}: P={val['precision']:.3f} R={val['recall']:.3f} F1={val['f1']:.3f}")
            else:
                print(f"  {key}: {val:.3f}")
        return

    # Load model
    print(f"Loading model: {MODEL_NAME}")
    model = AutoModelForSequenceClassification.from_pretrained(
        MODEL_NAME, num_labels=NUM_LABELS, problem_type="multi_label_classification"
    )
    model.to(device)

    # Optimizer
    optimizer = torch.optim.AdamW(model.parameters(), lr=args.lr, weight_decay=0.01)

    # Loss with optional pos_weight to bias toward precision
    if args.pos_weight != 1.0:
        pw = torch.tensor([args.pos_weight] * NUM_LABELS, device=device)
        loss_fn = nn.BCEWithLogitsLoss(pos_weight=pw)
        print(f"  pos_weight={args.pos_weight} (precision-biased)")
    else:
        loss_fn = nn.BCEWithLogitsLoss()

    # Linear warmup + decay scheduler
    total_steps = len(train_loader) * args.epochs
    warmup_steps = int(total_steps * args.warmup_ratio)

    def lr_lambda(step):
        if step < warmup_steps:
            return float(step) / max(1, warmup_steps)
        return max(0.0, float(total_steps - step) / max(1, total_steps - warmup_steps))

    scheduler = torch.optim.lr_scheduler.LambdaLR(optimizer, lr_lambda)

    # Training loop
    best_val_f1 = 0.0
    patience_counter = 0
    os.makedirs(args.output_dir, exist_ok=True)

    print(f"\n=== Training ===")
    print(f"LR: {args.lr}, Epochs: {args.epochs}, Batch: {args.batch_size}, Warmup: {warmup_steps}/{total_steps} steps")

    for epoch in range(args.epochs):
        model.train()
        total_loss = 0.0
        n_batches = 0

        for batch in train_loader:
            input_ids = batch["input_ids"].to(device)
            attention_mask = batch["attention_mask"].to(device)
            labels = batch["labels"].to(device)

            outputs = model(input_ids=input_ids, attention_mask=attention_mask)
            loss = loss_fn(outputs.logits, labels)

            optimizer.zero_grad()
            loss.backward()
            torch.nn.utils.clip_grad_norm_(model.parameters(), 1.0)
            optimizer.step()
            scheduler.step()

            total_loss += loss.item()
            n_batches += 1

        avg_loss = total_loss / n_batches

        # Validate
        val_results, _, _ = evaluate(model, val_loader, device, args.threshold)
        val_f1 = val_results["macro_f1"]

        print(
            f"  Epoch {epoch + 1}/{args.epochs}: "
            f"loss={avg_loss:.4f} "
            f"val_F1={val_f1:.3f} "
            f"val_P={val_results['macro_precision']:.3f} "
            f"val_R={val_results['macro_recall']:.3f} "
            f"benign_acc={val_results['benign_accuracy']:.3f}"
        )

        # Early stopping
        if val_f1 > best_val_f1:
            best_val_f1 = val_f1
            patience_counter = 0
            # Save best checkpoint
            best_path = os.path.join(args.output_dir, "best")
            model.save_pretrained(best_path)
            tokenizer.save_pretrained(best_path)
            print(f"    Saved best checkpoint (F1={val_f1:.3f})")
        else:
            patience_counter += 1
            if patience_counter >= args.patience:
                print(f"    Early stopping at epoch {epoch + 1} (patience={args.patience})")
                break

    # Evaluate on test set with best checkpoint
    print(f"\n=== Test Evaluation (best checkpoint) ===")
    best_path = os.path.join(args.output_dir, "best")
    model = AutoModelForSequenceClassification.from_pretrained(
        best_path, num_labels=NUM_LABELS, problem_type="multi_label_classification"
    )
    model.to(device)

    test_results, test_preds, test_labels = evaluate(model, test_loader, device, args.threshold)

    for key, val in test_results.items():
        if isinstance(val, dict):
            print(f"  {key}: P={val['precision']:.3f} R={val['recall']:.3f} F1={val['f1']:.3f}")
        else:
            print(f"  {key}: {val:.3f}")

    # Ship criteria check
    print(f"\n=== Ship Criteria ===")
    checks = []

    mp = test_results["macro_precision"]
    checks.append(("Precision >= 80%", mp >= 0.80, f"{mp:.1%}"))

    a_recall = test_results.get("a_Deceptive", {}).get("recall", 0)
    checks.append(("Recall(Art_5_1_a) >= 70%", a_recall >= 0.70, f"{a_recall:.1%}"))

    b_age_recall = test_results.get("b_Age", {}).get("recall", 0)
    checks.append(("Recall(Art_5_1_b_Age) >= 60%", b_age_recall >= 0.60, f"{b_age_recall:.1%}"))

    b_se_recall = test_results.get("b_SocioEcon", {}).get("recall", 0)
    checks.append(("Recall(Art_5_1_b_SE) >= 60%", b_se_recall >= 0.60, f"{b_se_recall:.1%}"))

    c_recall = test_results.get("c_Social_Scoring", {}).get("recall", 0)
    checks.append(("Recall(Art_5_1_c) >= 60%", c_recall >= 0.60, f"{c_recall:.1%}"))

    all_pass = True
    for name, passed, value in checks:
        status = "PASS" if passed else "FAIL"
        print(f"  [{status}] {name}: {value}")
        if not passed:
            all_pass = False

    if all_pass:
        print(f"\n  All ship criteria PASSED. Ready for ONNX export.")
        print(f"  Export with: python3 scripts/r2_finetune.py --export-onnx models/r2_persuasion_tinybert_int8.onnx")
    else:
        print(f"\n  Some criteria FAILED. Consider tuning hyperparameters or adding data.")

    # Save test results
    results_path = os.path.join(str(SCRIPT_DIR / "data" / "r2_eval"), "test_results.json")
    os.makedirs(os.path.dirname(results_path), exist_ok=True)
    with open(results_path, "w") as f:
        # Convert numpy types for JSON serialization
        serializable = {}
        for k, v in test_results.items():
            if isinstance(v, dict):
                serializable[k] = {kk: float(vv) for kk, vv in v.items()}
            else:
                serializable[k] = float(v)
        json.dump(serializable, f, indent=2)
    print(f"\n  Results saved: {results_path}")


if __name__ == "__main__":
    main()
