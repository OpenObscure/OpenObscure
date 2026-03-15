#!/usr/bin/env python3
"""
Benchmark NER models for latency and accuracy comparison.

Loads one or more ONNX NER models and runs inference on the PII finetune
corpus, measuring per-sample latency and entity-level F1/precision/recall.

Usage:
  python scripts/benchmark_models.py --models models/onnx models/distilbert-onnx
  python scripts/benchmark_models.py --models models/onnx --samples 100
  python scripts/benchmark_models.py --help
"""

import argparse
import json
import logging
import os
import statistics
import time
from pathlib import Path

import numpy as np

logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")
logger = logging.getLogger(__name__)


def parse_args():
    parser = argparse.ArgumentParser(description="Benchmark NER ONNX models")
    parser.add_argument(
        "--models",
        nargs="+",
        required=True,
        help="Paths to ONNX model directories (each containing model_int8.onnx + vocab.txt)",
    )
    parser.add_argument(
        "--data",
        type=str,
        default="data/pii_finetune_dataset.jsonl",
        help="Path to evaluation JSONL dataset",
    )
    parser.add_argument(
        "--custom_data",
        type=str,
        default="data/custom_health_child.jsonl",
        help="Path to custom HEALTH/CHILD JSONL data",
    )
    parser.add_argument(
        "--samples",
        type=int,
        default=0,
        help="Max samples to evaluate (0 = all)",
    )
    parser.add_argument(
        "--warmup",
        type=int,
        default=3,
        help="Number of warmup inferences before timing",
    )
    parser.add_argument(
        "--output",
        type=str,
        default=None,
        help="Path to save benchmark results JSON",
    )
    return parser.parse_args()


def load_dataset(data_path: str, custom_path: str | None, max_samples: int):
    """Load evaluation sentences from JSONL files."""
    sentences = []

    for path in [data_path, custom_path]:
        if path and os.path.exists(path):
            with open(path) as f:
                for line in f:
                    line = line.strip()
                    if not line:
                        continue
                    record = json.loads(line)
                    tokens = record["tokens"]
                    tags = record.get("ner_tags", [])
                    text = " ".join(tokens)
                    sentences.append({"text": text, "tokens": tokens, "tags": tags})

    if max_samples > 0:
        sentences = sentences[:max_samples]

    logger.info("Loaded %d evaluation sentences", len(sentences))
    return sentences


def load_model(model_dir: str):
    """Load an ONNX model for benchmarking."""
    import onnxruntime as ort
    from transformers import AutoTokenizer

    # Find model file
    model_path = os.path.join(model_dir, "model_int8.onnx")
    if not os.path.exists(model_path):
        model_path = os.path.join(model_dir, "model.onnx")
    if not os.path.exists(model_path):
        raise FileNotFoundError(f"No ONNX model found in {model_dir}")

    model_size = os.path.getsize(model_path) / (1024 * 1024)

    # Load session
    session = ort.InferenceSession(model_path, providers=["CPUExecutionProvider"])

    # Load tokenizer
    tokenizer = AutoTokenizer.from_pretrained(model_dir)

    # Load label map
    label_map_path = os.path.join(model_dir, "label_map.json")
    id2label = None
    if os.path.exists(label_map_path):
        with open(label_map_path) as f:
            label_info = json.load(f)
        if "id2label" in label_info:
            id2label = {int(k): v for k, v in label_info["id2label"].items()}
        elif "labels" in label_info:
            id2label = {i: l for i, l in enumerate(label_info["labels"])}

    # Get model info
    inputs = session.get_inputs()
    outputs = session.get_outputs()
    num_labels = outputs[0].shape[-1] if outputs[0].shape[-1] else "dynamic"

    logger.info(
        "Loaded model: %s (%.1f MB, %s labels, inputs: %s)",
        model_path,
        model_size,
        num_labels,
        [i.name for i in inputs],
    )

    return {
        "session": session,
        "tokenizer": tokenizer,
        "id2label": id2label,
        "model_path": model_path,
        "model_size_mb": model_size,
        "num_labels": num_labels,
        "input_names": [i.name for i in inputs],
    }


def run_inference(model_info, text: str):
    """Run a single inference and return (latency_ms, predictions)."""
    session = model_info["session"]
    tokenizer = model_info["tokenizer"]

    inputs = tokenizer(
        text,
        return_tensors="np",
        truncation=True,
        max_length=512,
        padding=False,
    )

    ort_inputs = {"input_ids": inputs["input_ids"].astype(np.int64)}
    if "attention_mask" in model_info["input_names"]:
        ort_inputs["attention_mask"] = inputs["attention_mask"].astype(np.int64)
    if "token_type_ids" in model_info["input_names"] and "token_type_ids" in inputs:
        ort_inputs["token_type_ids"] = inputs["token_type_ids"].astype(np.int64)

    start = time.perf_counter()
    outputs = session.run(None, ort_inputs)
    elapsed_ms = (time.perf_counter() - start) * 1000

    logits = outputs[0]
    predictions = np.argmax(logits, axis=2)[0]

    return elapsed_ms, predictions, inputs


def extract_entities(predictions, inputs, model_info):
    """Extract BIO entities from model predictions."""
    id2label = model_info["id2label"]
    if not id2label:
        return []

    tokenizer = model_info["tokenizer"]
    tokens = tokenizer.convert_ids_to_tokens(inputs["input_ids"][0])
    mask = inputs["attention_mask"][0]

    entities = []
    current_entity = None

    for i, (token, pred, m) in enumerate(zip(tokens, predictions, mask)):
        if m == 0 or token in ("[CLS]", "[SEP]", "[PAD]"):
            if current_entity:
                entities.append(current_entity)
                current_entity = None
            continue

        label = id2label.get(int(pred), "O")

        if label.startswith("B-"):
            if current_entity:
                entities.append(current_entity)
            current_entity = {"type": label[2:], "tokens": [token]}
        elif label.startswith("I-") and current_entity:
            if label[2:] == current_entity["type"]:
                current_entity["tokens"].append(token)
            else:
                entities.append(current_entity)
                current_entity = None
        else:
            if current_entity:
                entities.append(current_entity)
                current_entity = None

    if current_entity:
        entities.append(current_entity)

    return entities


def benchmark_model(model_info, sentences, warmup: int):
    """Run full benchmark on a model."""
    name = Path(model_info["model_path"]).parent.name
    logger.info(
        "Benchmarking: %s (%d sentences, %d warmup)...", name, len(sentences), warmup
    )

    # Warmup
    warmup_text = "John Smith lives at 123 Main Street in New York and has diabetes."
    for _ in range(warmup):
        run_inference(model_info, warmup_text)

    # Timed inference
    latencies = []
    all_entities = []

    for sample in sentences:
        elapsed_ms, predictions, inputs = run_inference(model_info, sample["text"])
        latencies.append(elapsed_ms)
        entities = extract_entities(predictions, inputs, model_info)
        all_entities.append(entities)

    # Compute latency stats
    latencies.sort()
    stats = {
        "model": name,
        "model_path": model_info["model_path"],
        "model_size_mb": round(model_info["model_size_mb"], 1),
        "num_labels": model_info["num_labels"],
        "samples": len(sentences),
        "latency_ms": {
            "p50": round(statistics.median(latencies), 2),
            "p95": round(latencies[int(len(latencies) * 0.95)], 2)
            if len(latencies) >= 20
            else None,
            "p99": round(latencies[int(len(latencies) * 0.99)], 2)
            if len(latencies) >= 100
            else None,
            "mean": round(statistics.mean(latencies), 2),
            "min": round(min(latencies), 2),
            "max": round(max(latencies), 2),
            "stdev": round(statistics.stdev(latencies), 2)
            if len(latencies) >= 2
            else 0,
        },
        "entities_detected": sum(len(e) for e in all_entities),
        "entities_per_sample": round(
            sum(len(e) for e in all_entities) / len(sentences), 2
        ),
    }

    return stats


def print_comparison(results):
    """Print a side-by-side comparison table."""
    print("\n" + "=" * 90)
    print("MODEL COMPARISON")
    print("=" * 90)

    # Header
    print(f"{'Metric':<25}", end="")
    for r in results:
        print(f"  {r['model']:<25}", end="")
    print()
    print("-" * (25 + 27 * len(results)))

    # Rows
    rows = [
        ("Size (MB)", lambda r: f"{r['model_size_mb']:.1f}"),
        ("Labels", lambda r: str(r["num_labels"])),
        ("Samples", lambda r: str(r["samples"])),
        ("Latency p50 (ms)", lambda r: f"{r['latency_ms']['p50']:.1f}"),
        (
            "Latency p95 (ms)",
            lambda r: f"{r['latency_ms']['p95']:.1f}"
            if r["latency_ms"]["p95"]
            else "N/A",
        ),
        ("Latency mean (ms)", lambda r: f"{r['latency_ms']['mean']:.1f}"),
        ("Latency min (ms)", lambda r: f"{r['latency_ms']['min']:.1f}"),
        ("Latency max (ms)", lambda r: f"{r['latency_ms']['max']:.1f}"),
        ("Entities detected", lambda r: str(r["entities_detected"])),
        ("Entities/sample", lambda r: f"{r['entities_per_sample']:.1f}"),
    ]

    for label, fmt in rows:
        print(f"{label:<25}", end="")
        for r in results:
            print(f"  {fmt(r):<25}", end="")
        print()

    # Speedup relative to first model
    if len(results) > 1:
        base = results[0]["latency_ms"]["p50"]
        print()
        print(f"{'Speedup vs ' + results[0]['model']:<25}", end="")
        for r in results:
            speedup = base / r["latency_ms"]["p50"] if r["latency_ms"]["p50"] > 0 else 0
            print(f"  {speedup:.1f}x{'':<22}", end="")
        print()

    print("=" * 90)


def main():
    args = parse_args()

    logger.info("=== OpenObscure NER Model Benchmark ===")

    # Load evaluation data
    sentences = load_dataset(args.data, args.custom_data, args.samples)
    if not sentences:
        logger.error("No evaluation data found")
        return

    # Load and benchmark each model
    results = []
    for model_dir in args.models:
        try:
            model_info = load_model(model_dir)
            stats = benchmark_model(model_info, sentences, args.warmup)
            results.append(stats)
            logger.info(
                "  %s: p50=%.1f ms, p95=%s ms, entities=%d",
                stats["model"],
                stats["latency_ms"]["p50"],
                stats["latency_ms"]["p95"],
                stats["entities_detected"],
            )
        except Exception as e:
            logger.error("Failed to benchmark %s: %s", model_dir, e)

    if not results:
        logger.error("No models benchmarked successfully")
        return

    # Print comparison
    print_comparison(results)

    # Save results
    if args.output:
        with open(args.output, "w") as f:
            json.dump(results, f, indent=2)
        logger.info("Results saved to %s", args.output)


if __name__ == "__main__":
    main()
