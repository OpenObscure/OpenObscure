#!/usr/bin/env python3
"""
Generate a mock ONNX NER model for development and testing.

Produces a small (~50KB) ONNX model with the same input/output signature
as the real TinyBERT NER model. This allows the Rust proxy's ONNX Runtime
integration to be developed and tested without waiting for actual training.

The mock model uses random weights — its predictions are meaningless,
but the tensor shapes and types match the real model exactly.

Usage:
  python test/scripts/mock/generate_mock_ner_model.py --output_dir models/mock
"""

import argparse
import json
import logging
import os

import numpy as np
import onnx

logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")
logger = logging.getLogger(__name__)

# Same label schema as finetune.py
LABEL_LIST = [
    "O",
    "B-PER", "I-PER",
    "B-LOC", "I-LOC",
    "B-ORG", "I-ORG",
    "B-HEALTH", "I-HEALTH",
    "B-CHILD", "I-CHILD",
]
NUM_LABELS = len(LABEL_LIST)
VOCAB_SIZE = 30522        # BERT WordPiece vocab size
HIDDEN_DIM = 312          # TinyBERT-4L hidden dimension
MAX_SEQ_LEN = 512


def parse_args():
    parser = argparse.ArgumentParser(description="Generate mock ONNX NER model")
    parser.add_argument(
        "--output_dir",
        type=str,
        default="models/mock",
        help="Directory for mock model output",
    )
    return parser.parse_args()


def build_mock_onnx():
    """Build a minimal ONNX graph: Gather embeddings → MatMul → output logits."""
    from onnx import TensorProto, helper, numpy_helper

    # Random embedding table: [vocab_size, hidden_dim]
    rng = np.random.default_rng(42)
    embedding_weights = rng.standard_normal((VOCAB_SIZE, HIDDEN_DIM)).astype(np.float32) * 0.02
    classifier_weights = rng.standard_normal((HIDDEN_DIM, NUM_LABELS)).astype(np.float32) * 0.02
    classifier_bias = np.zeros(NUM_LABELS, dtype=np.float32)

    # Initializers (constant tensors)
    embedding_init = numpy_helper.from_array(embedding_weights, name="embedding_table")
    classifier_w_init = numpy_helper.from_array(classifier_weights, name="classifier_weight")
    classifier_b_init = numpy_helper.from_array(classifier_bias, name="classifier_bias")

    # Inputs: same as real model
    input_ids = helper.make_tensor_value_info("input_ids", TensorProto.INT64, ["batch_size", "sequence_length"])
    attention_mask = helper.make_tensor_value_info("attention_mask", TensorProto.INT64, ["batch_size", "sequence_length"])
    token_type_ids = helper.make_tensor_value_info("token_type_ids", TensorProto.INT64, ["batch_size", "sequence_length"])

    # Output: logits [batch_size, sequence_length, num_labels]
    logits = helper.make_tensor_value_info("logits", TensorProto.FLOAT, ["batch_size", "sequence_length", NUM_LABELS])

    # Nodes: input_ids → Gather(embedding) → MatMul(classifier) → Add(bias) → logits
    gather_node = helper.make_node("Gather", inputs=["embedding_table", "input_ids"], outputs=["embedded"], axis=0)
    matmul_node = helper.make_node("MatMul", inputs=["embedded", "classifier_weight"], outputs=["pre_bias"])
    add_node = helper.make_node("Add", inputs=["pre_bias", "classifier_bias"], outputs=["logits"])

    # Build graph
    graph = helper.make_graph(
        [gather_node, matmul_node, add_node],
        "mock_ner_model",
        [input_ids, attention_mask, token_type_ids],
        [logits],
        initializer=[embedding_init, classifier_w_init, classifier_b_init],
    )

    # Build model
    model = helper.make_model(graph, opset_imports=[helper.make_opsetid("", 17)])
    model.ir_version = 9

    # Validate
    onnx.checker.check_model(model)

    return model


def write_mock_tokenizer_vocab(output_dir: str):
    """Write a minimal vocab.txt for WordPiece tokenization testing."""
    # Minimal vocab: just special tokens + basic ASCII chars + a few test words
    special_tokens = ["[PAD]", "[UNK]", "[CLS]", "[SEP]", "[MASK]"]
    # Basic ASCII characters
    ascii_chars = [chr(i) for i in range(ord('a'), ord('z') + 1)]
    ascii_upper = [chr(i) for i in range(ord('A'), ord('Z') + 1)]
    digits = [str(i) for i in range(10)]
    # Some common wordpiece tokens
    common = [
        "the", "is", "was", "has", "my", "and", "to", "of", "in", "at",
        "john", "smith", "diabetes", "metformin", "daughter", "school",
        "##s", "##ed", "##ing", "##er", "##ly",
    ]

    vocab = special_tokens + ascii_chars + ascii_upper + digits + common
    vocab_path = os.path.join(output_dir, "vocab.txt")
    with open(vocab_path, "w") as f:
        for token in vocab:
            f.write(token + "\n")
    logger.info("Mock vocab: %d tokens → %s", len(vocab), vocab_path)


def write_tokenizer_config(output_dir: str):
    """Write tokenizer config files matching BERT WordPiece format."""
    tokenizer_config = {
        "do_lower_case": True,
        "model_max_length": MAX_SEQ_LEN,
        "tokenizer_class": "BertTokenizer",
    }
    config_path = os.path.join(output_dir, "tokenizer_config.json")
    with open(config_path, "w") as f:
        json.dump(tokenizer_config, f, indent=2)

    special_tokens_map = {
        "unk_token": "[UNK]",
        "sep_token": "[SEP]",
        "pad_token": "[PAD]",
        "cls_token": "[CLS]",
        "mask_token": "[MASK]",
    }
    spm_path = os.path.join(output_dir, "special_tokens_map.json")
    with open(spm_path, "w") as f:
        json.dump(special_tokens_map, f, indent=2)


def main():
    args = parse_args()
    os.makedirs(args.output_dir, exist_ok=True)

    logger.info("=== Generating Mock ONNX NER Model ===")

    # Build and save ONNX model
    model = build_mock_onnx()
    model_path = os.path.join(args.output_dir, "model_int8.onnx")
    onnx.save(model, model_path)
    model_size = os.path.getsize(model_path) / (1024 * 1024)
    logger.info("Mock model: %s (%.2f MB)", model_path, model_size)

    # Save label map
    label_map = {
        "id2label": {str(i): label for i, label in enumerate(LABEL_LIST)},
        "label2id": {label: i for i, label in enumerate(LABEL_LIST)},
        "labels": LABEL_LIST,
    }
    label_map_path = os.path.join(args.output_dir, "label_map.json")
    with open(label_map_path, "w") as f:
        json.dump(label_map, f, indent=2)
    logger.info("Label map: %s", label_map_path)

    # Write mock tokenizer artifacts
    write_mock_tokenizer_vocab(args.output_dir)
    write_tokenizer_config(args.output_dir)

    # Quick smoke test
    logger.info("Running smoke test...")
    import onnxruntime as ort
    session = ort.InferenceSession(model_path, providers=["CPUExecutionProvider"])
    dummy_input = {
        "input_ids": np.array([[2, 10, 11, 12, 3] + [0] * 5], dtype=np.int64),
        "attention_mask": np.array([[1, 1, 1, 1, 1] + [0] * 5], dtype=np.int64),
        "token_type_ids": np.array([[0] * 10], dtype=np.int64),
    }
    outputs = session.run(None, dummy_input)
    logits = outputs[0]
    logger.info("  Output shape: %s (expected: [1, 10, %d])", logits.shape, NUM_LABELS)
    assert logits.shape == (1, 10, NUM_LABELS), f"Shape mismatch: {logits.shape}"
    logger.info("Smoke test passed!")

    logger.info("=== Mock model ready at %s ===", args.output_dir)
    logger.info("Use this for Rust proxy ONNX Runtime integration testing.")


if __name__ == "__main__":
    main()
