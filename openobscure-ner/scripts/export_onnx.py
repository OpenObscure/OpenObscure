#!/usr/bin/env python3
"""
Export fine-tuned TinyBERT NER model to ONNX + INT8 quantization.

Takes a trained PyTorch model directory and produces:
  1. model.onnx              — FP32 ONNX export
  2. model_optimized.onnx    — Graph-optimized (attention/LayerNorm/Gelu fusion)
  3. model_int8.onnx         — INT8 quantized (the artifact shipped with OpenObscure)
  4. label_map.json          — Label ID ↔ name mapping
  5. tokenizer files         — WordPiece vocab for inference

The INT8 model is ~15MB and runs in ~55MB RAM via ONNX Runtime.

Usage:
  python scripts/export_onnx.py --model_dir models/tinybert-ner --output_dir models/onnx
  python scripts/export_onnx.py --model_dir models/distilbert-ner --output_dir models/onnx --hidden_size 768
  python scripts/export_onnx.py --help
"""

import argparse
import json
import logging
import os
import shutil
from pathlib import Path

import numpy as np
import onnx
from onnxruntime.quantization import QuantType, quantize_dynamic
from optimum.onnxruntime import ORTModelForTokenClassification
from transformers import AutoTokenizer

logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")
logger = logging.getLogger(__name__)


def parse_args():
    parser = argparse.ArgumentParser(description="Export NER model to ONNX + INT8")
    parser.add_argument(
        "--model_dir",
        type=str,
        required=True,
        help="Path to fine-tuned PyTorch model directory",
    )
    parser.add_argument(
        "--output_dir",
        type=str,
        default="models/onnx",
        help="Directory for ONNX output artifacts",
    )
    parser.add_argument(
        "--max_length",
        type=int,
        default=512,
        help="Max sequence length for ONNX export",
    )
    parser.add_argument(
        "--validate",
        action="store_true",
        default=True,
        help="Run validation after export",
    )
    parser.add_argument(
        "--skip_fp32",
        action="store_true",
        help="Skip saving the FP32 model (only save INT8)",
    )
    parser.add_argument(
        "--num_heads",
        type=int,
        default=12,
        help="Number of attention heads (12 for BERT/DistilBERT/TinyBERT)",
    )
    parser.add_argument(
        "--hidden_size",
        type=int,
        default=312,
        help="Hidden size (312 for TinyBERT-4L, 768 for BERT-base/DistilBERT)",
    )
    parser.add_argument(
        "--skip_optimize",
        action="store_true",
        help="Skip ORT transformer graph optimization before quantization",
    )
    return parser.parse_args()


def export_to_onnx(model_dir: str, output_dir: str, max_length: int) -> str:
    """Export PyTorch model to FP32 ONNX using optimum."""
    logger.info("Exporting model to ONNX (FP32)...")

    # Use optimum for clean ONNX export with proper opset and dynamic axes
    ort_model = ORTModelForTokenClassification.from_pretrained(
        model_dir,
        export=True,
    )
    ort_model.save_pretrained(output_dir)

    onnx_path = os.path.join(output_dir, "model.onnx")
    if not os.path.exists(onnx_path):
        # optimum might save with a different name
        for f in Path(output_dir).glob("*.onnx"):
            if "quantized" not in f.name and "int8" not in f.name:
                onnx_path = str(f)
                break

    model_size = os.path.getsize(onnx_path) / (1024 * 1024)
    logger.info("FP32 ONNX model: %s (%.1f MB)", onnx_path, model_size)
    return onnx_path


def optimize_graph(fp32_path: str, output_dir: str, num_heads: int, hidden_size: int) -> str:
    """Apply ORT transformer-specific graph optimizations (attention/LayerNorm/Gelu fusion)."""
    from onnxruntime.transformers import optimizer as ort_optimizer

    logger.info(
        "Applying transformer graph optimizations (heads=%d, hidden=%d)...",
        num_heads,
        hidden_size,
    )

    opt_model = ort_optimizer.optimize_model(
        fp32_path,
        model_type="bert",
        num_heads=num_heads,
        hidden_size=hidden_size,
    )

    optimized_path = os.path.join(output_dir, "model_optimized.onnx")
    opt_model.save_model_to_file(optimized_path)

    original_size = os.path.getsize(fp32_path) / (1024 * 1024)
    optimized_size = os.path.getsize(optimized_path) / (1024 * 1024)
    logger.info(
        "Optimized ONNX model: %s (%.1f MB → %.1f MB)",
        optimized_path,
        original_size,
        optimized_size,
    )
    return optimized_path


def quantize_int8(fp32_path: str, output_dir: str) -> str:
    """Apply dynamic INT8 quantization to the ONNX model."""
    logger.info("Applying INT8 dynamic quantization...")

    int8_path = os.path.join(output_dir, "model_int8.onnx")
    quantize_dynamic(
        model_input=fp32_path,
        model_output=int8_path,
        weight_type=QuantType.QInt8,
        per_channel=True,
        reduce_range=False,
        extra_options={"DefaultTensorType": onnx.TensorProto.FLOAT},
    )

    model_size = os.path.getsize(int8_path) / (1024 * 1024)
    logger.info("INT8 ONNX model: %s (%.1f MB)", int8_path, model_size)

    if model_size > 20:
        logger.warning(
            "INT8 model is %.1f MB — exceeds 20MB target. "
            "Consider pruning or using a smaller base model.",
            model_size,
        )

    return int8_path


def validate_model(int8_path: str, model_dir: str):
    """Run basic validation: load the INT8 model and do a test inference."""
    import onnxruntime as ort

    logger.info("Validating INT8 model...")

    tokenizer = AutoTokenizer.from_pretrained(model_dir)

    # Load label map
    label_map_path = os.path.join(model_dir, "label_map.json")
    if os.path.exists(label_map_path):
        with open(label_map_path) as f:
            label_info = json.load(f)
        id2label = {int(k): v for k, v in label_info["id2label"].items()}
    else:
        logger.warning("No label_map.json found, using generic labels")
        id2label = None

    # Create ONNX Runtime session
    session = ort.InferenceSession(int8_path, providers=["CPUExecutionProvider"])

    # Test sentences
    test_sentences = [
        "John Smith lives at 123 Oak Street in Springfield",
        "I have diabetes and take metformin daily",
        "My daughter Sarah goes to Lincoln Elementary School",
        "Please email john.doe@example.com about the meeting",
    ]

    for sentence in test_sentences:
        inputs = tokenizer(
            sentence,
            return_tensors="np",
            truncation=True,
            max_length=512,
            padding="max_length",
        )

        ort_inputs = {
            "input_ids": inputs["input_ids"].astype(np.int64),
            "attention_mask": inputs["attention_mask"].astype(np.int64),
        }
        if "token_type_ids" in inputs:
            ort_inputs["token_type_ids"] = inputs["token_type_ids"].astype(np.int64)

        outputs = session.run(None, ort_inputs)
        logits = outputs[0]
        predictions = np.argmax(logits, axis=2)[0]

        # Decode predictions for non-padding tokens
        tokens = tokenizer.convert_ids_to_tokens(inputs["input_ids"][0])
        mask = inputs["attention_mask"][0]

        entities = []
        for token, pred, m in zip(tokens, predictions, mask):
            if m == 0 or token in ("[CLS]", "[SEP]", "[PAD]"):
                continue
            label = id2label[pred] if id2label else f"LABEL_{pred}"
            if label != "O":
                entities.append(f"{token}/{label}")

        logger.info("  Input: %s", sentence)
        logger.info("  Entities: %s", entities if entities else "(none)")

    logger.info("Validation complete — model loads and produces predictions")


def copy_inference_artifacts(model_dir: str, output_dir: str):
    """Copy tokenizer and label map to output for self-contained inference."""
    # Copy tokenizer files
    tokenizer = AutoTokenizer.from_pretrained(model_dir)
    tokenizer.save_pretrained(output_dir)

    # Copy label map
    label_map_src = os.path.join(model_dir, "label_map.json")
    label_map_dst = os.path.join(output_dir, "label_map.json")
    if os.path.exists(label_map_src):
        shutil.copy2(label_map_src, label_map_dst)
        logger.info("Copied label_map.json to output")


def main():
    args = parse_args()

    logger.info("=== OpenObscure NER ONNX Export ===")
    logger.info("Model: %s", args.model_dir)
    logger.info("Output: %s", args.output_dir)

    os.makedirs(args.output_dir, exist_ok=True)

    # 1. Export to FP32 ONNX
    fp32_path = export_to_onnx(args.model_dir, args.output_dir, args.max_length)

    # 2. Transformer graph optimization (attention/LayerNorm/Gelu fusion)
    quant_input = fp32_path
    if not args.skip_optimize:
        optimized_path = optimize_graph(
            fp32_path, args.output_dir, args.num_heads, args.hidden_size
        )
        quant_input = optimized_path

    # 3. INT8 quantization (on optimized graph if available)
    int8_path = quantize_int8(quant_input, args.output_dir)

    # 4. Clean up intermediate files
    if not args.skip_optimize and os.path.exists(quant_input) and quant_input != int8_path:
        os.remove(quant_input)
        logger.info("Removed intermediate optimized model")
    if args.skip_fp32 and os.path.exists(fp32_path) and fp32_path != int8_path:
        os.remove(fp32_path)
        logger.info("Removed FP32 model (--skip_fp32)")

    # 5. Copy inference artifacts (tokenizer, label map)
    copy_inference_artifacts(args.model_dir, args.output_dir)

    # 6. Validate
    if args.validate:
        validate_model(int8_path, args.output_dir)

    # Summary
    final_size = os.path.getsize(int8_path) / (1024 * 1024)
    logger.info("=== Export complete ===")
    logger.info("  INT8 model: %s (%.1f MB)", int8_path, final_size)
    logger.info("  Target: <15 MB, <55 MB RAM at inference")


if __name__ == "__main__":
    main()
