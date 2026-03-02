#!/usr/bin/env python3
"""
Fine-tune a BERT-family model for PII NER (Named Entity Recognition).

Produces a token-classification model with BIO tags for:
  PER    — person names
  LOC    — locations / addresses
  ORG    — organizations
  HEALTH — medical conditions, medications, symptoms
  CHILD  — child-related references

Training data:
  - CoNLL-2003 (PER, LOC, ORG baseline via HuggingFace datasets)
  - OntoNotes 5.0 (18 entity types remapped to our 5-type schema)
  - WNUT-2017 (noisy text NER, 6 types remapped to our schema)
  - Custom augmented examples for HEALTH and CHILD (JSON-lines in data/)

Usage:
  python scripts/finetune.py --output_dir models/tinybert-ner --epochs 5
  python scripts/finetune.py --base_model distilbert-base-uncased --datasets conll,wnut --epochs 7
  python scripts/finetune.py --help
"""

import argparse
import json
import logging
import os
import sys
from pathlib import Path

import evaluate
import numpy as np
from datasets import ClassLabel, Dataset, DatasetDict, Features, Sequence, Value, load_dataset
from transformers import (
    AutoModelForTokenClassification,
    AutoTokenizer,
    DataCollatorForTokenClassification,
    EarlyStoppingCallback,
    Trainer,
    TrainingArguments,
)

logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")
logger = logging.getLogger(__name__)

# ─── Label schema ────────────────────────────────────────────────────────────

LABEL_LIST = [
    "O",        # Outside any entity
    "B-PER",    # Beginning of person name
    "I-PER",    # Inside person name
    "B-LOC",    # Beginning of location/address
    "I-LOC",    # Inside location/address
    "B-ORG",    # Beginning of organization
    "I-ORG",    # Inside organization
    "B-HEALTH", # Beginning of health/medical reference
    "I-HEALTH", # Inside health/medical reference
    "B-CHILD",  # Beginning of child-related reference
    "I-CHILD",  # Inside child-related reference
]

LABEL2ID = {label: i for i, label in enumerate(LABEL_LIST)}
ID2LABEL = {i: label for i, label in enumerate(LABEL_LIST)}

# CoNLL-2003 uses a different label set — map it to ours
CONLL_LABEL_MAP = {
    "O": "O",
    "B-PER": "B-PER",
    "I-PER": "I-PER",
    "B-LOC": "B-LOC",
    "I-LOC": "I-LOC",
    "B-ORG": "B-ORG",
    "I-ORG": "I-ORG",
    "B-MISC": "O",   # MISC → O (we don't use MISC)
    "I-MISC": "O",
}

# OntoNotes 5.0 uses 18 entity types — map to our 5-type schema
ONTONOTES_LABEL_MAP = {
    "PERSON": "PER",
    "GPE": "LOC",        # Geopolitical entities (countries, cities)
    "LOC": "LOC",        # Non-GPE locations (mountains, rivers)
    "FAC": "LOC",        # Facilities (airports, bridges)
    "ORG": "ORG",
    # Everything else → O
    "NORP": "O", "DATE": "O", "TIME": "O", "MONEY": "O",
    "CARDINAL": "O", "ORDINAL": "O", "PERCENT": "O",
    "QUANTITY": "O", "EVENT": "O", "WORK_OF_ART": "O",
    "LAW": "O", "LANGUAGE": "O", "PRODUCT": "O",
}

# WNUT-2017 uses 6 entity types — map to our schema
WNUT_LABEL_MAP = {
    "person": "PER",
    "location": "LOC",
    "corporation": "ORG",
    "group": "ORG",
    "creative-work": "O",
    "product": "O",
}

BASE_MODEL = "huawei-noah/TinyBERT_General_4L_312D"
MAX_LENGTH = 512


def parse_args():
    parser = argparse.ArgumentParser(description="Fine-tune a BERT model for PII NER")
    parser.add_argument(
        "--output_dir",
        type=str,
        default="models/tinybert-ner",
        help="Directory to save the fine-tuned model",
    )
    parser.add_argument("--epochs", type=int, default=5, help="Number of training epochs")
    parser.add_argument("--batch_size", type=int, default=16, help="Training batch size")
    parser.add_argument("--learning_rate", type=float, default=5e-5, help="Learning rate")
    parser.add_argument("--weight_decay", type=float, default=0.01, help="Weight decay")
    parser.add_argument("--warmup_ratio", type=float, default=0.1, help="Warmup ratio")
    parser.add_argument(
        "--custom_data",
        type=str,
        default=None,
        help="Path to custom JSONL data with HEALTH/CHILD annotations",
    )
    parser.add_argument(
        "--base_model",
        type=str,
        default=BASE_MODEL,
        help="Base model name or path",
    )
    parser.add_argument(
        "--datasets",
        type=str,
        default="conll",
        help="Comma-separated dataset list: conll,ontonotes,wnut (default: conll)",
    )
    parser.add_argument(
        "--ontonotes_weight",
        type=float,
        default=1.0,
        help="Sampling weight for OntoNotes (1.0 = use all, 0.5 = half)",
    )
    parser.add_argument("--seed", type=int, default=42, help="Random seed")
    parser.add_argument("--fp16", action="store_true", help="Use FP16 mixed precision")
    parser.add_argument(
        "--early_stopping_patience",
        type=int,
        default=3,
        help="Early stopping patience (0 to disable)",
    )
    return parser.parse_args()


# ─── Data loading ────────────────────────────────────────────────────────────


def load_conll2003():
    """Load CoNLL-2003 and remap labels to our schema."""
    logger.info("Loading CoNLL-2003 dataset...")
    raw = load_dataset("conll2003", trust_remote_code=True)

    # Get original label names
    original_labels = raw["train"].features["ner_tags"].feature.names

    def remap_labels(example):
        new_tags = []
        for tag_id in example["ner_tags"]:
            original_label = original_labels[tag_id]
            mapped_label = CONLL_LABEL_MAP.get(original_label, "O")
            new_tags.append(LABEL2ID[mapped_label])
        example["ner_tags"] = new_tags
        return example

    remapped = raw.map(remap_labels)
    logger.info(
        "CoNLL-2003 loaded: %d train, %d validation, %d test",
        len(remapped["train"]),
        len(remapped["validation"]),
        len(remapped["test"]),
    )
    return remapped


def load_ontonotes(weight: float = 1.0):
    """Load OntoNotes 5.0 and remap 18 entity types to our 5-type schema.

    The dataset is structured as documents → sentences. We flatten to individual
    sentences and remap NER tags using ONTONOTES_LABEL_MAP.

    Args:
        weight: Sampling fraction (1.0 = all data, 0.5 = random 50%).
    """
    logger.info("Loading OntoNotes 5.0 (english_v4)...")
    raw = load_dataset("conll2012_ontonotesv5", "english_v4", trust_remote_code=True)

    def flatten_and_remap(split_data):
        all_tokens = []
        all_tags = []
        for doc in split_data:
            for sentence in doc["sentences"]:
                tokens = sentence["words"]
                named_entities = sentence.get("named_entities", [])
                if not named_entities or len(named_entities) != len(tokens):
                    # Skip sentences without NER annotations or mismatched lengths
                    continue
                new_tags = []
                for ne_tag in named_entities:
                    # OntoNotes NER tags are in BIO format: "B-PERSON", "I-ORG", "O", etc.
                    if ne_tag == "O" or ne_tag == "*":
                        new_tags.append(LABEL2ID["O"])
                    elif ne_tag.startswith("B-") or ne_tag.startswith("I-"):
                        prefix = ne_tag[:2]  # "B-" or "I-"
                        entity_type = ne_tag[2:]
                        mapped = ONTONOTES_LABEL_MAP.get(entity_type, "O")
                        if mapped == "O":
                            new_tags.append(LABEL2ID["O"])
                        else:
                            new_tags.append(LABEL2ID[f"{prefix}{mapped}"])
                    else:
                        new_tags.append(LABEL2ID["O"])
                all_tokens.append(tokens)
                all_tags.append(new_tags)
        return all_tokens, all_tags

    result = {}
    for split_name in ["train", "validation", "test"]:
        if split_name not in raw:
            continue
        tokens, tags = flatten_and_remap(raw[split_name])
        ds = Dataset.from_dict({"tokens": tokens, "ner_tags": tags})
        if weight < 1.0 and len(ds) > 0:
            ds = ds.shuffle(seed=42).select(range(int(len(ds) * weight)))
        result[split_name] = ds
        logger.info("OntoNotes %s: %d sentences (weight=%.1f)", split_name, len(ds), weight)

    return DatasetDict(result)


def load_wnut2017():
    """Load WNUT-2017 noisy text NER and remap to our schema.

    WNUT-2017 has 6 entity types: person, location, corporation, group,
    creative-work, product. We remap to PER/LOC/ORG and drop the rest.
    """
    logger.info("Loading WNUT-2017 dataset...")
    raw = load_dataset("wnut_17", trust_remote_code=True)

    # Get original label names
    original_labels = raw["train"].features["ner_tags"].feature.names

    def remap_labels(example):
        new_tags = []
        for tag_id in example["ner_tags"]:
            original_label = original_labels[tag_id]
            if original_label == "O":
                new_tags.append(LABEL2ID["O"])
            elif original_label.startswith("B-") or original_label.startswith("I-"):
                prefix = original_label[:2]
                entity_type = original_label[2:]
                mapped = WNUT_LABEL_MAP.get(entity_type, "O")
                if mapped == "O":
                    new_tags.append(LABEL2ID["O"])
                else:
                    new_tags.append(LABEL2ID[f"{prefix}{mapped}"])
            else:
                new_tags.append(LABEL2ID["O"])
        example["ner_tags"] = new_tags
        return example

    remapped = raw.map(remap_labels)
    logger.info(
        "WNUT-2017 loaded: %d train, %d validation, %d test",
        len(remapped["train"]),
        len(remapped.get("validation", [])),
        len(remapped["test"]),
    )
    return remapped


def load_custom_data(path: str) -> Dataset:
    """
    Load custom JSONL data for HEALTH/CHILD entities.

    Expected format per line:
    {
      "tokens": ["My", "daughter", "has", "asthma"],
      "ner_tags": ["O", "B-CHILD", "O", "B-HEALTH"]
    }
    Tags can be string labels (mapped to IDs) or integer IDs.
    """
    logger.info("Loading custom data from %s", path)
    records = []
    with open(path) as f:
        for line_num, line in enumerate(f, 1):
            line = line.strip()
            if not line:
                continue
            record = json.loads(line)
            tokens = record["tokens"]
            tags = record["ner_tags"]
            # Convert string labels to IDs if needed
            if tags and isinstance(tags[0], str):
                tags = [LABEL2ID[t] for t in tags]
            assert len(tokens) == len(tags), f"Line {line_num}: token/tag length mismatch"
            records.append({"tokens": tokens, "ner_tags": tags})

    dataset = Dataset.from_dict(
        {
            "tokens": [r["tokens"] for r in records],
            "ner_tags": [r["ner_tags"] for r in records],
        }
    )
    logger.info("Custom data loaded: %d examples", len(dataset))
    return dataset


def _normalize_dataset(ds: DatasetDict) -> DatasetDict:
    """Strip extra columns and cast to uniform features for concatenation."""
    keep_cols = {"tokens", "ner_tags"}
    target_features = Features(
        {
            "tokens": Sequence(Value("string")),
            "ner_tags": Sequence(Value("int64")),
        }
    )
    result = {}
    for split, data in ds.items():
        cleaned = data.remove_columns([c for c in data.column_names if c not in keep_cols])
        result[split] = cleaned.cast(target_features)
    return DatasetDict(result)


def merge_datasets(
    datasets_list: list[DatasetDict],
    custom: Dataset | None,
    seed: int = 42,
) -> DatasetDict:
    """Merge multiple NER datasets + optional custom HEALTH/CHILD data."""
    from datasets import concatenate_datasets

    # Normalize all datasets to uniform schema
    normalized = [_normalize_dataset(ds) for ds in datasets_list]

    # Collect per-split
    train_parts = []
    val_parts = []
    test_parts = []

    for ds in normalized:
        if "train" in ds:
            train_parts.append(ds["train"])
        if "validation" in ds:
            val_parts.append(ds["validation"])
        if "test" in ds:
            test_parts.append(ds["test"])

    # Add custom data (80/20 train/val split)
    if custom is not None:
        target_features = Features(
            {
                "tokens": Sequence(Value("string")),
                "ner_tags": Sequence(Value("int64")),
            }
        )
        custom = custom.cast(target_features)
        custom_split = custom.train_test_split(test_size=0.2, seed=seed)
        train_parts.append(custom_split["train"])
        val_parts.append(custom_split["test"])

    merged_train = concatenate_datasets(train_parts) if train_parts else Dataset.from_dict({"tokens": [], "ner_tags": []})
    merged_val = concatenate_datasets(val_parts) if val_parts else Dataset.from_dict({"tokens": [], "ner_tags": []})
    merged_test = concatenate_datasets(test_parts) if test_parts else Dataset.from_dict({"tokens": [], "ner_tags": []})

    logger.info(
        "Merged dataset: %d train, %d validation, %d test (%d sources)",
        len(merged_train),
        len(merged_val),
        len(merged_test),
        len(datasets_list),
    )
    return DatasetDict(
        {"train": merged_train, "validation": merged_val, "test": merged_test}
    )


# ─── Tokenization ───────────────────────────────────────────────────────────


def tokenize_and_align_labels(examples, tokenizer):
    """
    Tokenize with WordPiece and align BIO labels to sub-tokens.

    Rules:
    - First sub-token of a word keeps the original label
    - Subsequent sub-tokens of a B-X word get I-X
    - Subsequent sub-tokens of an I-X word keep I-X
    - Special tokens ([CLS], [SEP], [PAD]) get label -100 (ignored in loss)
    """
    tokenized = tokenizer(
        examples["tokens"],
        truncation=True,
        max_length=MAX_LENGTH,
        is_split_into_words=True,
        padding=False,
    )

    all_labels = []
    for i, labels in enumerate(examples["ner_tags"]):
        word_ids = tokenized.word_ids(batch_index=i)
        previous_word_id = None
        label_ids = []
        for word_id in word_ids:
            if word_id is None:
                # Special token
                label_ids.append(-100)
            elif word_id != previous_word_id:
                # First sub-token of a new word
                label_ids.append(labels[word_id])
            else:
                # Subsequent sub-token of the same word
                label = labels[word_id]
                label_name = ID2LABEL.get(label, "O")
                if label_name.startswith("B-"):
                    # Convert B-X → I-X for continuation sub-tokens
                    continuation = LABEL2ID.get("I-" + label_name[2:], label)
                    label_ids.append(continuation)
                else:
                    label_ids.append(label)
            previous_word_id = word_id
        all_labels.append(label_ids)

    tokenized["labels"] = all_labels
    return tokenized


# ─── Metrics ─────────────────────────────────────────────────────────────────


def build_compute_metrics(seqeval_metric):
    """Build a compute_metrics function for the Trainer."""

    def compute_metrics(eval_pred):
        predictions, labels = eval_pred
        predictions = np.argmax(predictions, axis=2)

        # Remove special token labels (-100) and convert to string labels
        true_labels = []
        true_predictions = []
        for pred_seq, label_seq in zip(predictions, labels):
            pred_tags = []
            true_tags = []
            for p, l in zip(pred_seq, label_seq):
                if l == -100:
                    continue
                pred_tags.append(ID2LABEL[p])
                true_tags.append(ID2LABEL[l])
            true_predictions.append(pred_tags)
            true_labels.append(true_tags)

        results = seqeval_metric.compute(
            predictions=true_predictions, references=true_labels
        )
        return {
            "precision": results["overall_precision"],
            "recall": results["overall_recall"],
            "f1": results["overall_f1"],
            "accuracy": results["overall_accuracy"],
        }

    return compute_metrics


# ─── Main ────────────────────────────────────────────────────────────────────


def main():
    args = parse_args()

    logger.info("=== OpenObscure NER Fine-Tuning ===")
    logger.info("Base model: %s", args.base_model)
    logger.info("Output: %s", args.output_dir)
    logger.info("Labels: %s", LABEL_LIST)

    # DistilBERT-specific hyperparameter suggestions
    if "distilbert" in args.base_model.lower():
        if args.learning_rate == 5e-5:  # only override if user didn't explicitly set
            args.learning_rate = 3e-5
            logger.info("DistilBERT detected: adjusted learning_rate to %.0e", args.learning_rate)
        if args.epochs == 5:  # only override if user didn't explicitly set
            args.epochs = 7
            logger.info("DistilBERT detected: adjusted epochs to %d", args.epochs)

    # Load tokenizer
    tokenizer = AutoTokenizer.from_pretrained(args.base_model)

    # Load datasets based on --datasets flag
    requested = [d.strip().lower() for d in args.datasets.split(",")]
    datasets_list = []
    for ds_name in requested:
        if ds_name == "conll":
            datasets_list.append(load_conll2003())
        elif ds_name == "ontonotes":
            datasets_list.append(load_ontonotes(weight=args.ontonotes_weight))
        elif ds_name == "wnut":
            datasets_list.append(load_wnut2017())
        else:
            logger.warning("Unknown dataset '%s', skipping", ds_name)
    if not datasets_list:
        logger.error("No datasets loaded! Check --datasets flag.")
        sys.exit(1)

    custom = load_custom_data(args.custom_data) if args.custom_data else None
    dataset = merge_datasets(datasets_list, custom, seed=args.seed)

    # Tokenize
    logger.info("Tokenizing dataset...")
    tokenized = dataset.map(
        lambda examples: tokenize_and_align_labels(examples, tokenizer),
        batched=True,
        remove_columns=dataset["train"].column_names,
    )

    # Load model
    logger.info("Loading model: %s", args.base_model)
    model = AutoModelForTokenClassification.from_pretrained(
        args.base_model,
        num_labels=len(LABEL_LIST),
        id2label=ID2LABEL,
        label2id=LABEL2ID,
    )

    # Data collator
    data_collator = DataCollatorForTokenClassification(tokenizer=tokenizer)

    # Metrics
    seqeval_metric = evaluate.load("seqeval")
    compute_metrics = build_compute_metrics(seqeval_metric)

    # Training arguments
    training_args = TrainingArguments(
        output_dir=args.output_dir,
        num_train_epochs=args.epochs,
        per_device_train_batch_size=args.batch_size,
        per_device_eval_batch_size=args.batch_size * 2,
        learning_rate=args.learning_rate,
        weight_decay=args.weight_decay,
        warmup_ratio=args.warmup_ratio,
        eval_strategy="epoch",
        save_strategy="epoch",
        logging_strategy="steps",
        logging_steps=100,
        load_best_model_at_end=True,
        metric_for_best_model="f1",
        greater_is_better=True,
        save_total_limit=2,
        seed=args.seed,
        fp16=args.fp16,
        report_to="none",
        dataloader_num_workers=0,
    )

    callbacks = []
    if args.early_stopping_patience > 0:
        callbacks.append(
            EarlyStoppingCallback(early_stopping_patience=args.early_stopping_patience)
        )

    # Trainer
    trainer = Trainer(
        model=model,
        args=training_args,
        train_dataset=tokenized["train"],
        eval_dataset=tokenized["validation"],
        tokenizer=tokenizer,
        data_collator=data_collator,
        compute_metrics=compute_metrics,
        callbacks=callbacks,
    )

    # Train
    logger.info("Starting training...")
    train_result = trainer.train()
    logger.info(
        "Training complete: loss=%.4f, epochs=%d",
        train_result.training_loss,
        int(train_result.metrics.get("epoch", args.epochs)),
    )

    # Evaluate on test set
    logger.info("Evaluating on test set...")
    test_results = trainer.evaluate(tokenized["test"])
    logger.info("Test results: %s", test_results)

    # Per-entity type breakdown
    logger.info("=== Per-Entity Type Breakdown ===")
    test_predictions = trainer.predict(tokenized["test"])
    predictions = np.argmax(test_predictions.predictions, axis=2)
    labels = test_predictions.label_ids

    # Collect per-type predictions and references
    true_labels_all = []
    true_preds_all = []
    for pred_seq, label_seq in zip(predictions, labels):
        pred_tags = []
        true_tags = []
        for p, l in zip(pred_seq, label_seq):
            if l == -100:
                continue
            pred_tags.append(ID2LABEL[p])
            true_tags.append(ID2LABEL[l])
        true_preds_all.append(pred_tags)
        true_labels_all.append(true_tags)

    # Use seqeval for per-type metrics
    per_type_results = seqeval_metric.compute(
        predictions=true_preds_all,
        references=true_labels_all,
        mode="strict",
        scheme="IOB2",
    )
    for entity_type in ["PER", "LOC", "ORG", "HEALTH", "CHILD"]:
        if entity_type in per_type_results:
            r = per_type_results[entity_type]
            logger.info(
                "  %s: P=%.3f R=%.3f F1=%.3f (support=%d)",
                entity_type,
                r["precision"],
                r["recall"],
                r["f1"],
                r["number"],
            )
        else:
            logger.info("  %s: no examples in test set", entity_type)

    # Save
    trainer.save_model(args.output_dir)
    tokenizer.save_pretrained(args.output_dir)

    # Save label map for inference
    label_map_path = os.path.join(args.output_dir, "label_map.json")
    with open(label_map_path, "w") as f:
        json.dump({"id2label": ID2LABEL, "label2id": LABEL2ID, "labels": LABEL_LIST}, f, indent=2)
    logger.info("Label map saved to %s", label_map_path)

    logger.info("=== Done! Model saved to %s ===", args.output_dir)


if __name__ == "__main__":
    main()
