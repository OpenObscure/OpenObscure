#!/usr/bin/env python3
"""quantize_image_models.py — Quantize FP32 image pipeline ONNX models to INT8.

Applies ONNX Runtime dynamic INT8 quantization to reduce model size and
potentially improve inference speed on CPU. CoreML EP on Apple Silicon does
NOT support quantized operators (QLinearConv, MatMulInteger), so those ops
will fall back to CPU — benchmark before deploying.

Models quantized:
  - PaddleOCR rec v4   (7.3 MB  → ~2-3 MB)
  - SCRFD-2.5GF        (3.1 MB  → ~1-2 MB)
  - PaddleOCR det v4   (2.3 MB  → ~1 MB)

Skipped:
  - BlazeFace (0.4 MB — too small to benefit)

Usage:
  python build/quantize_image_models.py [--dry-run]

The script backs up originals to <name>_fp32.onnx before overwriting.
"""

import argparse
import os
import shutil
import sys

import onnx
from onnxruntime.quantization import QuantType, quantize_dynamic


MODELS_DIR = os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
    "openobscure-core", "models",
)

TARGETS = [
    {
        "name": "PaddleOCR rec v4",
        "path": os.path.join(MODELS_DIR, "paddleocr", "rec_model.onnx"),
        "preprocess": True,  # Uses Constant ops instead of initializers
    },
    {
        "name": "SCRFD-2.5GF",
        "path": os.path.join(MODELS_DIR, "scrfd", "scrfd_2.5g.onnx"),
        "preprocess": False,  # Has proper initializers
    },
    {
        "name": "PaddleOCR det v4",
        "path": os.path.join(MODELS_DIR, "paddleocr", "det_model.onnx"),
        "preprocess": True,  # Uses Constant ops instead of initializers
    },
]


def size_mb(path: str) -> float:
    return os.path.getsize(path) / (1024 * 1024)


def convert_constants_to_initializers(model_path: str) -> str:
    """PaddleOCR models embed weights as Constant ops instead of initializers.
    Dynamic quantization only touches initializers, so we need to convert first.
    Returns path to preprocessed model (temp file)."""
    model = onnx.load(model_path)
    graph = model.graph

    constant_nodes_to_remove = []
    for node in graph.node:
        if node.op_type == "Constant":
            # Extract the tensor value from the Constant node
            for attr in node.attribute:
                if attr.name == "value" and attr.t is not None:
                    tensor = attr.t
                    tensor.name = node.output[0]
                    graph.initializer.append(tensor)
                    constant_nodes_to_remove.append(node)
                    break

    for node in constant_nodes_to_remove:
        graph.node.remove(node)

    preprocessed_path = model_path.replace(".onnx", "_preprocessed.onnx")
    onnx.save(model, preprocessed_path)
    return preprocessed_path


def quantize_model(target: dict, dry_run: bool = False) -> dict:
    """Quantize a single model. Returns stats dict."""
    name = target["name"]
    path = target["path"]

    if not os.path.exists(path):
        return {"name": name, "status": "SKIP", "reason": "file not found"}

    fp32_size = size_mb(path)
    backup_path = path.replace(".onnx", "_fp32.onnx")

    if dry_run:
        return {
            "name": name,
            "status": "DRY-RUN",
            "fp32_mb": round(fp32_size, 1),
        }

    # Back up the original FP32 model
    if not os.path.exists(backup_path):
        shutil.copy2(path, backup_path)
        print(f"  Backed up FP32 → {os.path.basename(backup_path)}")

    # Preprocess PaddleOCR models (Constant → initializer conversion)
    input_path = path
    preprocessed_path = None
    if target["preprocess"]:
        print(f"  Preprocessing: converting Constant ops to initializers...")
        preprocessed_path = convert_constants_to_initializers(path)
        input_path = preprocessed_path

    # Output to a temp file, then replace original
    output_path = path.replace(".onnx", "_int8_tmp.onnx")

    try:
        quantize_dynamic(
            model_input=input_path,
            model_output=output_path,
            weight_type=QuantType.QUInt8,
            extra_options={"DefaultTensorType": 1},  # onnx.TensorProto.FLOAT
        )
    finally:
        # Clean up preprocessed temp file
        if preprocessed_path and os.path.exists(preprocessed_path):
            os.remove(preprocessed_path)

    int8_size = size_mb(output_path)
    reduction = (1 - int8_size / fp32_size) * 100

    # Replace the original with the quantized version
    os.replace(output_path, path)

    return {
        "name": name,
        "status": "OK",
        "fp32_mb": round(fp32_size, 1),
        "int8_mb": round(int8_size, 1),
        "reduction": round(reduction, 1),
    }


def main():
    parser = argparse.ArgumentParser(description="Quantize image pipeline models to INT8")
    parser.add_argument("--dry-run", action="store_true", help="Show what would be done")
    args = parser.parse_args()

    print("=" * 60)
    print("  OpenObscure — Image Model INT8 Quantization")
    print("=" * 60)
    print()

    results = []
    for target in TARGETS:
        print(f"[{target['name']}] {os.path.basename(target['path'])}")
        result = quantize_model(target, dry_run=args.dry_run)
        results.append(result)

        if result["status"] == "OK":
            print(f"  {result['fp32_mb']} MB → {result['int8_mb']} MB ({result['reduction']}% reduction)")
        elif result["status"] == "DRY-RUN":
            print(f"  FP32: {result['fp32_mb']} MB (dry run — no changes)")
        else:
            print(f"  {result['status']}: {result.get('reason', 'unknown')}")
        print()

    # Summary
    print("=" * 60)
    print("  Summary")
    print("=" * 60)
    ok_results = [r for r in results if r["status"] == "OK"]
    if ok_results:
        total_fp32 = sum(r["fp32_mb"] for r in ok_results)
        total_int8 = sum(r["int8_mb"] for r in ok_results)
        print(f"  Models quantized: {len(ok_results)}/{len(TARGETS)}")
        print(f"  Total FP32: {total_fp32:.1f} MB → INT8: {total_int8:.1f} MB")
        print(f"  Total saved: {total_fp32 - total_int8:.1f} MB ({(1 - total_int8/total_fp32)*100:.1f}%)")
    else:
        for r in results:
            print(f"  {r['name']}: {r['status']}")
    print()


if __name__ == "__main__":
    main()
