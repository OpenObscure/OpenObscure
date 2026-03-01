#!/usr/bin/env python3
"""Export Marqo/nsfw-image-detection-384 to ONNX and optionally INT8-quantize.

Usage:
    pip install optimum[onnxruntime] onnxruntime Pillow
    python scripts/export_nsfw_classifier.py [--quantize]

Output:
    models/nsfw_classifier/nsfw_classifier.onnx (FP32 or INT8)

The model is a ViT-tiny (5.7M params) fine-tuned for binary SFW/NSFW classification.
Input:  [1, 3, 384, 384] NCHW float32, normalized with mean=0.5, std=0.5
Output: [1, 2] logits → softmax → [P(NSFW), P(SFW)]

Label mapping: id2label = {0: "NSFW", 1: "SFW"}
"""

import argparse
import os
import sys
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description="Export NSFW classifier to ONNX")
    parser.add_argument(
        "--quantize",
        action="store_true",
        help="Apply INT8 dynamic quantization after export",
    )
    parser.add_argument(
        "--output-dir",
        type=str,
        default="openobscure-proxy/models/nsfw_classifier",
        help="Output directory for the ONNX model",
    )
    parser.add_argument(
        "--model-name",
        type=str,
        default="Marqo/nsfw-image-detection-384",
        help="HuggingFace model ID",
    )
    args = parser.parse_args()

    output_dir = Path(args.output_dir)
    output_dir.mkdir(parents=True, exist_ok=True)

    # Step 1: Export to ONNX via optimum
    print(f"Exporting {args.model_name} to ONNX...")
    try:
        from optimum.onnxruntime import ORTModelForImageClassification
    except ImportError:
        print("Error: optimum[onnxruntime] not installed.")
        print("Run: pip install optimum[onnxruntime]")
        sys.exit(1)

    tmp_dir = output_dir / "_tmp_export"
    model = ORTModelForImageClassification.from_pretrained(
        args.model_name, export=True
    )
    model.save_pretrained(tmp_dir)

    # The exported model is in tmp_dir/model.onnx
    src_model = tmp_dir / "model.onnx"
    if not src_model.exists():
        print(f"Error: Expected {src_model} after export, not found.")
        sys.exit(1)

    # Step 2: Optional INT8 quantization
    final_model = output_dir / "nsfw_classifier.onnx"

    if args.quantize:
        print("Applying INT8 dynamic quantization...")
        try:
            from onnxruntime.quantization import quantize_dynamic, QuantType
        except ImportError:
            print("Error: onnxruntime not installed for quantization.")
            print("Run: pip install onnxruntime")
            sys.exit(1)

        quantize_dynamic(
            str(src_model),
            str(final_model),
            weight_type=QuantType.QInt8,
        )
        print(f"Quantized model saved to {final_model}")
    else:
        import shutil
        shutil.copy2(src_model, final_model)
        print(f"FP32 model saved to {final_model}")

    # Step 3: Validate output shape
    print("Validating model output shape...")
    try:
        import onnxruntime as ort
        import numpy as np

        sess = ort.InferenceSession(str(final_model))
        input_name = sess.get_inputs()[0].name
        input_shape = sess.get_inputs()[0].shape
        print(f"  Input: {input_name} {input_shape}")

        # Create dummy input
        dummy = np.random.randn(1, 3, 384, 384).astype(np.float32)
        outputs = sess.run(None, {input_name: dummy})
        output_shape = outputs[0].shape
        print(f"  Output shape: {output_shape}")

        assert output_shape == (1, 2), f"Expected (1, 2), got {output_shape}"
        print("  Validation passed!")

    except ImportError:
        print("  Skipping validation (onnxruntime not available)")

    # Cleanup tmp dir
    import shutil
    shutil.rmtree(tmp_dir, ignore_errors=True)

    # Report size
    size_mb = final_model.stat().st_size / (1024 * 1024)
    print(f"\nModel size: {size_mb:.1f} MB")
    print(f"Output: {final_model}")
    print("\nDone! Add to Git LFS with:")
    print(f"  git lfs track '{output_dir}/*.onnx'")
    print(f"  git add {final_model}")


if __name__ == "__main__":
    main()
