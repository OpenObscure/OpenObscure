# OpenObscure — Semantic PII Detection Architecture

> NER/CRF subsystem within the L0 Rust proxy. See [ARCHITECTURE.md](../openobscure-proxy/ARCHITECTURE.md) for the full proxy architecture.

---

## Overview

OpenObscure uses a **multi-source ensemble** for PII detection. Regex handles structured patterns (credit cards, SSNs, phones), while the semantic subsystem catches **unstructured PII** — person names, addresses, organizations, health conditions, and child references that have no fixed format.

The semantic subsystem has two backends selected by device capability:

| Backend | Model | RAM | Latency | Accuracy | When Used |
|---------|-------|-----|---------|----------|-----------|
| **NER** (TinyBERT INT8) | ONNX, ~15MB on disk | ~55MB loaded | ~8–15ms/sentence | ~97% recall | Full + Standard tiers (4GB+ RAM) |
| **CRF** (hand-crafted features) | JSON, <5MB on disk | <10MB loaded | ~2ms/sentence | ~80–85% recall | Lite tier (<4GB RAM) |

Both backends produce the same output type (`Vec<PiiMatch>`) and use the same 11-label BIO schema. The `HybridScanner` orchestrates them alongside regex and keyword scanners, resolving overlaps via **ensemble confidence voting**.

---

## Architecture Diagram

```
Input text
    │
    ├──▶ WordPiece Tokenizer (wordpiece.rs)
    │    ├── Lowercase + pre-tokenize (whitespace/punctuation split)
    │    ├── Greedy longest-match subword segmentation
    │    ├── [CLS] tokens... [SEP] [PAD]... (max 512)
    │    └── word_ids: maps each token back to original word index
    │
    ├──▶ NER Scanner (ner_scanner.rs) — Full/Standard tiers
    │    ├── Build tensors: input_ids, attention_mask, token_type_ids
    │    ├── ONNX Runtime inference (TinyBERT INT8)
    │    ├── Softmax → per-token confidence scores
    │    ├── BIO tag decode (first sub-token per word)
    │    └── Entity spans with byte offsets + confidence
    │
    ├──▶ CRF Scanner (crf_scanner.rs) — Lite tier alternative
    │    ├── Simple word-boundary tokenization
    │    ├── 24-feature extraction per token (shape, prefix, suffix, gazetteer, context)
    │    ├── Viterbi decoding with transition matrix
    │    └── Entity spans with sigmoid-normalized confidence
    │
    ├──▶ Regex Scanner (scanner.rs) — all tiers
    │    └── RegexSet + individual patterns, confidence always 1.0
    │
    ├──▶ Keyword Dictionary (keyword_dict.rs) — all tiers
    │    └── ~700 health/child terms, HashSet O(1) lookup, confidence 1.0
    │
    ├──▶ Multilingual Scanner (multilingual/) — all tiers
    │    └── Language-specific national ID + phone patterns
    │
    └──▶ HybridScanner Ensemble (hybrid_scanner.rs)
         ├── Phase 1: Collect all matches, tag with source
         ├── Phase 2: Cluster overlapping spans (union-find)
         ├── Phase 3: Vote per cluster
         │   ├── Highest confidence wins
         │   ├── ≥2 sources agree → +0.15 agreement bonus
         │   └── Filter by min_confidence (0.5)
         └── Output: Vec<PiiMatch>
```

---

## BIO Label Schema

Both NER and CRF use the same 11-label BIO (Beginning/Inside/Outside) schema:

| Label ID | Tag | PII Type | Description |
|----------|-----|----------|-------------|
| 0 | O | — | Outside any entity |
| 1 | B-PER | Person | Beginning of person name |
| 2 | I-PER | Person | Inside person name |
| 3 | B-LOC | Location | Beginning of location/address |
| 4 | I-LOC | Location | Inside location |
| 5 | B-ORG | Organization | Beginning of org name |
| 6 | I-ORG | Organization | Inside org name |
| 7 | B-HEALTH | HealthKeyword | Beginning of health term |
| 8 | I-HEALTH | HealthKeyword | Inside health term |
| 9 | B-CHILD | ChildKeyword | Beginning of child reference |
| 10 | I-CHILD | ChildKeyword | Inside child reference |

**BIO decoding rules:**
- B-tag starts a new entity
- I-tag continues the current entity (must match type)
- I-tag with mismatched type treated as new B-tag
- O-tag or end-of-sequence flushes the current entity

---

## NER Scanner (`ner_scanner.rs`)

### Model: TinyBERT-4L-312D INT8

| Property | Value |
|----------|-------|
| Architecture | 4-layer BERT with 312 hidden dims |
| Quantization | INT8 (mandatory — FP32 = ~200MB, INT8 = ~50MB) |
| Format | ONNX (cross-platform via `ort` crate) |
| Vocab | WordPiece, ~30K tokens |
| Max sequence length | 512 tokens |
| Inference | `ort::Session::run()` with `&mut self` |

### Inference Pipeline

```
Text: "Contact John Smith at john@example.com"
                    │
            ┌───────▼────────┐
            │  WordPiece      │
            │  Tokenizer      │
            └───────┬────────┘
                    │
    [CLS] contact john smith at john @ example . com [SEP] [PAD]...
    word_ids: [None, 0, 1, 2, 3, 4, 5, 6, 7, 8, None, None...]
                    │
            ┌───────▼────────┐
            │  ONNX Runtime   │
            │  TinyBERT INT8  │
            └───────┬────────┘
                    │
    Logits: [1, seq_len, 11] → Softmax → per-token confidence
                    │
            ┌───────▼────────┐
            │  BIO Decode     │
            │  (first sub-    │
            │   token/word)   │
            └───────┬────────┘
                    │
    Entities: [{type: Person, text: "John Smith", start: 8, end: 18, conf: 0.95}]
```

### Key Implementation Details

- **Sub-token handling:** Only the first sub-token of each word determines the BIO label. Continuation sub-tokens (prefixed `##`) inherit the word-level decision via `word_ids` mapping.
- **Softmax:** Computed per-token: `exp(logit_j) / sum(exp(logit_k))` for label j.
- **Confidence:** Average softmax score across all tokens in an entity span.
- **Model loading priority:** `model_int8.onnx` (preferred) → `model.onnx` (fallback).
- **Label map:** Read from `label_map.json` (defaults to 11 labels if missing).
- **Hardware acceleration:** Session built via `ort_ep::build_session()` — uses CoreML on Apple, NNAPI on Android, CPU elsewhere.

### Struct

```rust
pub struct NerScanner {
    session: Session,           // ONNX Runtime session (requires &mut for run)
    tokenizer: WordPieceTokenizer,
    confidence_threshold: f32,  // typically 0.5
    num_labels: usize,          // 11 (BIO schema)
}
```

---

## CRF Scanner (`crf_scanner.rs`)

### Design

The CRF is a lightweight alternative for devices where TinyBERT's ~55MB RAM footprint is too large. It uses **hand-crafted features** and a pre-trained transition matrix instead of learned embeddings.

| Property | Value |
|----------|-------|
| Model format | JSON (`crf_model.json`) |
| Features | 24 per token (shape, prefix, suffix, gazetteer, context) |
| Decoding | Viterbi (dynamic programming) |
| Labels | 11 (same BIO schema as NER) |
| RAM | <10MB |

### Feature Extraction (24 features per token)

| Category | Features | Example |
|----------|----------|---------|
| **Current word** | lowercase text, shape pattern | "John" → shape "Xx" |
| **Morphology** | prefix (1–3 chars), suffix (1–3 chars) | "Smith" → pre1="s", suf3="ith" |
| **Character class** | is_upper, is_title, is_digit, is_alpha | "John" → is_title=true |
| **Length** | bucket (short/medium/long) | "Dr" → short |
| **Gazetteer** | in_health_dict, in_child_dict | "diabetes" → health=true |
| **Context (±1)** | prev/next word shape, BOS/EOS markers | BOS + "John" → prev=BOS |

**Shape collapsing:** Consecutive same-type characters collapse: "John" → "Xx", "123-456" → "d-d", "ACME" → "X".

### Viterbi Decoding

```
Forward pass:
  viterbi[t][j] = max_i(viterbi[t-1][i] + transition[i][j]) + state_score[t][j]
  backptr[t][j] = argmax_i(...)

Backward pass:
  best_last = argmax_j(viterbi[T][j])
  path[T] = best_last
  path[t] = backptr[t+1][path[t+1]]
```

**Confidence:** Viterbi score normalized via sigmoid: `1.0 / (1.0 + exp(-score))`, averaged across entity tokens.

### Struct

```rust
pub struct CrfScanner {
    model: CrfModel,
    gazetteer_health: HashSet<String>,
    gazetteer_child: HashSet<String>,
    confidence_threshold: f32,
}

pub struct CrfModel {
    state_features: HashMap<String, Vec<f64>>,  // feature → per-label scores
    transitions: Vec<Vec<f64>>,                  // [from][to] transition weights
    num_labels: usize,                           // 11
}
```

---

## WordPiece Tokenizer (`wordpiece.rs`)

Implements BERT-style tokenization with byte-offset tracking for span reconstruction.

### Pipeline

```
"John's email is john@example.com"
        │
  Pre-tokenize (whitespace + punctuation split)
        │
  ["John", "'s", "email", "is", "john", "@", "example", ".", "com"]
  byte_offsets: [(0,4), (4,6), (7,12), (13,15), (16,20), (20,21), (21,28), (28,29), (29,32)]
        │
  WordPiece segmentation (greedy longest-match)
        │
  ["john", "'", "s", "email", "is", "john", "@", "example", ".", "com"]
  word_ids: [0, 1, 1, 2, 3, 4, 5, 6, 7, 8]
        │
  Add special tokens + pad to 512
        │
  [CLS] john ' s email is john @ example . com [SEP] [PAD] ... [PAD]
```

### Byte-Offset Alignment

The tokenizer preserves `WordSpan { text, byte_start, byte_end }` for each pre-tokenized word. When NER decodes BIO tags at the word level, it uses these spans to produce byte-accurate `PiiMatch` offsets into the original text. This is critical for correct FPE replacement (replacements are applied by byte offset in reverse order).

---

## Ensemble Voting (`hybrid_scanner.rs`)

### Three-Phase Resolution

**Phase 1 — Collect:** Run all enabled scanners independently. Tag each match with its source:

| Source | Scanner | Confidence |
|--------|---------|------------|
| `Regex` | RegexSet + post-validation | Always 1.0 |
| `Keyword` | Health/child dictionary | Always 1.0 |
| `Semantic` | NER or CRF | Model output (0.0–1.0) |

**Phase 2 — Cluster:** Group overlapping spans using union-find with path compression. Two matches overlap if `start_a < end_b && start_b < end_a`.

**Phase 3 — Vote:** For each cluster:
1. Group matches by `PiiType`
2. For each type, find the match with highest raw confidence
3. Track which distinct sources detected this type
4. If **2+ distinct sources** agree on the same type: `adjusted_confidence = min(raw + 0.15, 1.0)`
5. Winner is the type with highest adjusted confidence
6. Filter: discard if adjusted confidence < `min_confidence` (default 0.5)

### Agreement Bonus Example

```
Cluster: byte range [8, 18]
  Regex:    Email match, confidence 1.0      ← source: Regex
  NER:      Person match, confidence 0.92    ← source: Semantic
  Keyword:  (no match)

Resolution:
  Email:  conf=1.0, sources={Regex}       → adjusted=1.0 (1 source, no bonus)
  Person: conf=0.92, sources={Semantic}   → adjusted=0.92 (1 source, no bonus)
  Winner: Email (1.0 > 0.92)
```

```
Cluster: byte range [24, 34]
  Regex:    Phone match, confidence 1.0      ← source: Regex
  NER:      Phone match, confidence 0.88     ← source: Semantic

Resolution:
  Phone: conf=1.0, sources={Regex, Semantic} → adjusted=min(1.0+0.15, 1.0)=1.0
  Winner: Phone (boosted, but already at max)
```

The agreement bonus matters most when individual sources have moderate confidence (0.5–0.85) — multi-source agreement can push them above the threshold.

### Code Fence Handling

Before scanning, content inside markdown code fences (`` ``` `` and single backticks) is replaced with spaces. This prevents false positives on code snippets that contain patterns resembling PII (e.g., example API keys in documentation). Byte offsets are preserved so matches can be correctly mapped back.

---

## Device Tier Gating (`device_profile.rs`)

The `FeatureBudget` determines which semantic backend loads at startup:

### Gateway Mode (fixed budgets)

| Tier | Device RAM | NER | CRF | Ensemble Voting | Max RAM |
|------|------------|-----|-----|-----------------|---------|
| **Full** | 8GB+ | Yes | Yes | Yes (+0.15 bonus) | 275MB |
| **Standard** | 4–8GB | Yes | Yes | No | 200MB |
| **Lite** | <4GB | No | Yes | No | 80MB |

### Embedded Mode (mobile, proportional to device RAM)

Budget = 20% of total RAM, clamped to [12MB, 275MB].

| Tier | NER | CRF | Condition |
|------|-----|-----|-----------|
| Full | Yes | Yes | budget ≥ 275MB (capped) |
| Standard | Yes (if budget ≥ 80MB) | Yes | 4–8GB device |
| Lite | No | Yes (if budget ≥ 25MB) | <4GB device |

### Scanner Selection Flow

```
Startup
  │
  ├── DeviceProfile::detect() → total_ram, cpu_cores
  │
  ├── tier_for_profile() → Full / Standard / Lite
  │
  ├── budget_for_gateway() or budget_for_embedded()
  │   → FeatureBudget { ner_enabled, crf_enabled, ensemble_enabled, ... }
  │
  └── Initialize HybridScanner:
      ├── budget.ner_enabled → load NerScanner (TinyBERT INT8 ONNX)
      │   └── HybridScanner::new(keywords, Some(ner_scanner))
      ├── !ner_enabled && budget.crf_enabled → load CrfScanner (JSON model)
      │   └── HybridScanner::with_crf(keywords, Some(crf_scanner))
      └── neither → regex-only
          └── HybridScanner::regex_only()
```

---

## NER Endpoint (`ner_endpoint.rs`)

Exposes the hybrid scanner via HTTP for the L1 plugin's NER-enhanced redaction.

| Property | Value |
|----------|-------|
| Path | `POST /_openobscure/ner` |
| Auth | `X-OpenObscure-Token` header (same as health endpoint) |
| Input | `{"text": "..."}` (max 65KB) |
| Output | `[{"start": N, "end": N, "type": "person", "confidence": 0.95}]` |
| Scanner | Uses `HybridScanner` (not `NerScanner` directly) — includes ensemble voting |

The L1 plugin calls this endpoint synchronously (via `execFileSync(curl)`) when the heartbeat shows L0 is healthy. This gives tool result redaction access to semantic NER without loading ONNX models in the Node.js process.

---

## Model Files

All models are stored in `openobscure-proxy/models/` (git-ignored, downloaded via `build/download_models.sh`):

| File | Size | Purpose |
|------|------|---------|
| `model_int8.onnx` | ~15MB | TinyBERT INT8 quantized NER model |
| `model.onnx` | ~50MB | TinyBERT FP32 fallback (not used in production) |
| `vocab.txt` | ~230KB | WordPiece vocabulary (~30K tokens) |
| `label_map.json` | <1KB | BIO label ID → label name mapping |
| `crf_model.json` | <5MB | CRF weights (state features + transitions) |

---

## Performance Characteristics

| Metric | NER (TinyBERT INT8) | CRF |
|--------|---------------------|-----|
| Load time | ~500ms (first inference slower) | ~50ms |
| Inference | ~8–15ms/sentence (CPU) | ~2ms/sentence |
| RAM (loaded) | ~55MB | <10MB |
| Recall | ~97% | ~80–85% |
| Precision | ~95% | ~90% |
| Ensemble recall | 99.7% (with regex + keywords) | ~95% (with regex + keywords) |

The ensemble achieves 99.7% recall because regex handles all structured PII at 100% recall, and NER catches the semantic PII that regex misses. CRF's lower recall is acceptable on Lite tier devices where the RAM constraint is binding.

---

## Key Design Decisions

| Decision | Rationale |
|----------|-----------|
| INT8 quantization mandatory | FP32 TinyBERT = ~200MB; INT8 = ~50MB. Difference between fitting and OOM. |
| CRF as fallback, not replacement | CRF is ~15% lower recall but 1/5 the RAM. Acceptable trade-off for Lite devices. |
| Same BIO schema for both | Ensures HybridScanner can treat NER and CRF interchangeably. |
| Agreement bonus (0.15) | Multi-source agreement is a strong signal. Boosts moderate-confidence NER matches above threshold. |
| NER behind Mutex | `ort::Session::run()` requires `&mut self`. Mutex allows shared access from async handlers. |
| First sub-token for BIO | WordPiece splits words into sub-tokens. Using the first sub-token's label avoids inconsistent BIO assignments across sub-tokens. |
| 512 max tokens | BERT architectural limit. Longer texts should be chunked (not yet implemented — current proxy messages are typically <512 tokens). |
| GLiNER DROPPED | Evaluated 2026-02-19: best recall 82.78% vs TinyBERT 97%. Zero-shot flexibility doesn't compensate for lower per-type accuracy. |
