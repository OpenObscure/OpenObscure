#!/usr/bin/env python3
"""
Matryoshka Representation Learning (MRL) — ONNX Export & INT8 Quantization

Wraps a Hugging Face Matryoshka-trained model, bakes dimensionality reduction
(tensor slice) directly into the ONNX graph, exports FP32, then quantizes to
INT8 for edge deployment.

Source:  Architecture review (review-notes/arch_2026_02-22-16-18.md)
Status:  Reference only — not used in the current OpenObscure pipeline.
         Archived for Phase 4+ multimodal embedding evaluation.
Purpose: When evaluating multimodal embedding models (TinyCLIP, MobileCLIP,
         EmbeddingGemma, SigLIP), this script produces edge-ready ONNX models
         with reduced output dimensionality baked into the graph, so the Rust
         ort pipeline only receives the compact embedding vector.

Dependencies (not project dependencies):
    pip install torch transformers onnxruntime

Usage:
    python scripts/ml/matryoshka_export.py
    # Produces: openobscure_models/matryoshka_128d_int8.onnx
"""

import torch
import torch.nn.functional as F
from transformers import AutoModel, AutoTokenizer
from onnxruntime.quantization import quantize_dynamic, QuantType
import os


class MatryoshkaONNXWrapper(torch.nn.Module):
    def __init__(self, model_id: str, target_dim: int):
        super().__init__()
        # Load the base transformer model
        self.base_model = AutoModel.from_pretrained(model_id)
        self.target_dim = target_dim

    def forward(self, input_ids, attention_mask):
        # 1. Standard forward pass through the transformer
        outputs = self.base_model(input_ids=input_ids, attention_mask=attention_mask)

        # 2. Mean Pooling (standard for most dense embedding models)
        token_embeddings = outputs.last_hidden_state
        input_mask_expanded = attention_mask.unsqueeze(-1).expand(token_embeddings.size()).float()

        sum_embeddings = torch.sum(token_embeddings * input_mask_expanded, 1)
        sum_mask = torch.clamp(input_mask_expanded.sum(1), min=1e-9)
        embeddings = sum_embeddings / sum_mask

        # 3. L2 Normalization (crucial for cosine similarity later)
        embeddings = F.normalize(embeddings, p=2, dim=1)

        # 4. The Matryoshka Hack: Slice the tensor to the target dimension
        # This is baked into the ONNX graph, so the massive full-size tensor
        # is never passed back to your Rust environment.
        truncated_embeddings = embeddings[:, :self.target_dim]

        return truncated_embeddings


def build_openobscure_embedding_model(model_id: str, target_dim: int, output_dir: str):
    os.makedirs(output_dir, exist_ok=True)
    fp32_path = f"{output_dir}/matryoshka_{target_dim}d_fp32.onnx"
    int8_path = f"{output_dir}/matryoshka_{target_dim}d_int8.onnx"

    print(f"Loading {model_id} and wrapping for {target_dim}D Matryoshka slice...")
    tokenizer = AutoTokenizer.from_pretrained(model_id)
    model = MatryoshkaONNXWrapper(model_id, target_dim)
    model.eval()

    # Create dummy inputs for ONNX tracing
    dummy_text = ["Intercept and encrypt the user's PII before it reaches the LLM."]
    inputs = tokenizer(dummy_text, return_tensors="pt", padding=True, truncation=True, max_length=512)

    print(f"Exporting FP32 ONNX graph to {fp32_path}...")
    # Trace the PyTorch graph and export to ONNX
    torch.onnx.export(
        model,
        (inputs["input_ids"], inputs["attention_mask"]),
        fp32_path,
        export_params=True,
        opset_version=17,  # Using 17 for maximum compatibility with modern ORT execution providers
        do_constant_folding=True,
        input_names=["input_ids", "attention_mask"],
        output_names=["embeddings"],
        dynamic_axes={
            "input_ids": {0: "batch_size", 1: "sequence_length"},
            "attention_mask": {0: "batch_size", 1: "sequence_length"},
            "embeddings": {0: "batch_size"}
        }
    )

    print("Quantizing graph to INT8...")
    # Apply dynamic weight quantization to collapse FP32 weights to INT8
    quantize_dynamic(
        model_input=fp32_path,
        model_output=int8_path,
        weight_type=QuantType.QUInt8
    )

    print(f"Success! Your edge-ready model is at: {int8_path}")


if __name__ == "__main__":
    # Using a 300M parameter model as an example, truncating to 128 dimensions
    build_openobscure_embedding_model(
        model_id="tomaarsen/mpnet-base-nli-matryoshka",
        target_dim=128,
        output_dir="./openobscure_models"
    )
