# Semantic PII Detection Architecture

> **Role in OpenObscure:** The HybridScanner is the **text PII detection core** of L0. It orchestrates four detection engines — regex, keywords, NER, and CRF — in sequence, merges their results with confidence voting, and produces a unified match list for FPE encryption. For the full system context, see [System Overview](system-overview.md).
>
> **Implementation:** `openobscure-core/src/hybrid_scanner.rs` and related modules. For module-level details, see [L0 Core Architecture](l0-core.md).

---

## Pre-processing

Before any engine runs, two transformations are applied to the raw input text:

- **Code fence masking:** Content inside `` ` `` (inline) and ```` ``` ```` (fenced) blocks is replaced with spaces, preserving byte offsets. This prevents false positives on code snippets. After voting, match values are restored from the original unmasked text using the preserved offsets. Masked content is never scanned.
- **Skip fields:** Configured JSON fields (`model`, `temperature`, `stream`, etc.) are excluded before text is passed to engines. See [Detection Engine Configuration](../configure/detection-engine-configuration.md) for the full skip-field list.

---

## Detection Engines

The HybridScanner runs enabled engines in sequence on each text input. JSON field traversal is parallelized via rayon when multiple fields are present; per-text scanning within each field is always sequential.

| Engine | Module | What it detects | Confidence | Activation |
|--------|--------|----------------|------------|------------|
| **Regex** | `scanner.rs` | 10 structured types (CC, SSN, phone, email, API key, IPv4, IPv6, GPS, MAC, IBAN) | 1.0 (deterministic) | Always on |
| **Keywords** | `keyword_dict.rs` | ~700 health/child terms, 9 languages | 1.0 | Always on (configurable) |
| **NER** | `ner_scanner.rs` | Person, Location, Organization, Health, Child entities | Model score (0.0–1.0) | Tier-dependent |
| **CRF** | `crf_scanner.rs` | Same entity types as NER | Model score | Fallback when NER unavailable |

### Regex Scanner (`scanner.rs`)

Pattern-based detection with post-validation:

| PII Type | Validation | False Positive Prevention |
|----------|-----------|--------------------------|
| Credit Card | Luhn checksum | Rejects invalid card numbers |
| SSN | Range validation | Rejects 000, 666, 900+ area numbers |
| Phone | Separator required | Requires `-`, `(`, `)`, space, or `+` prefix — avoids bare digit runs |
| Email | RFC-like pattern | Standard `local@domain.tld` |
| API Key | Prefix match | Known prefixes: `sk-`, `AKIA`, `ghp_`, `xoxb-`, etc. |
| IPv4 | Structural | Rejects loopback/broadcast addresses |
| IPv6 | Full + compressed | Handles `::` shorthand |
| GPS | Precision check | 4+ decimal digits |
| MAC | Three formats | Colon, dash, dot separators |
| IBAN | Country code | 2-letter prefix preserved |

Uses `RegexSet` for multi-pattern matching in a single pass (linear time).

Validation failures (failed Luhn check, invalid SSN range, missing phone separator) are silent match discards within the regex engine. There is no separate post-match filter stage — exclusion is embedded in the match conditions.

### Keyword Dictionary (`keyword_dict.rs`)

HashSet-based O(1) lookup of health and child-related terms. Supports 9 languages: English, Spanish, French, German, Portuguese, Japanese, Chinese, Korean, Arabic.

Keyword matching requires no anchoring or validation — terms are matched exactly. The keyword engine does not interact with the checksum validation in the regex engine. Keyword hits always produce confidence 1.0.

### NER Scanner (`ner_scanner.rs`)

Neural Named Entity Recognition via ONNX Runtime (`ort 2.0`):

| Variant | Parameters | Size (INT8) | Latency (p50) | F1 | Tier |
|---------|-----------|-------------|---------------|-----|------|
| TinyBERT (4L-312D) | 14.5M | 13.7MB | ~0.8ms | 85.6% | Standard, Lite |
| DistilBERT (6L) | 66M | 63.7MB | ~4.3ms | 91.2% | Full |

- **Label schema:** 11-label BIO (B-PER, I-PER, B-LOC, I-LOC, B-ORG, I-ORG, B-HEALTH, I-HEALTH, B-CHILD, I-CHILD, O)
- **Tokenizer:** WordPiece (`wordpiece.rs`) with 512-token context window
- **Pool size:** Configurable concurrent sessions (Full: 2, Standard/Lite: 1)
- **Model guard:** Rejects models >70MB to prevent accidental BERT-base loading

### CRF Scanner (`crf_scanner.rs`)

Lightweight fallback using hand-crafted features and Viterbi decoding:

- Same 11-label BIO schema as NER
- Feature extraction: word shape, prefix/suffix (2-4 chars), capitalization, gazetteer membership, 2-word context window
- ~2ms inference, <10MB RAM, no ONNX dependency
- Lower recall than NER — best as a safety net, not a primary engine

### Name Gazetteer

Embedded first-name and last-name lists providing supporting evidence for NER person detections. No model files required. Enabled by default (`gazetteer_enabled = true`).

## Multilingual Detection (`multilingual/`)

Language detection via `whatlang` triggers language-specific scanners:

| Language | Module | National IDs | Validation |
|----------|--------|-------------|------------|
| Spanish | `es.rs` | DNI, NIE | Check-digit |
| French | `fr.rs` | NIR | Modulus 97 |
| German | `de.rs` | Personalausweis | Format |
| Portuguese | `pt.rs` | CPF, CNPJ | Mod 11 check digits |
| Japanese | `ja.rs` | My Number | Weighted mod 11 |
| Chinese | `zh.rs` | Citizen ID (18-digit) | Weighted mod 11 + X |
| Korean | `ko.rs` | RRN | Weighted mod 11 |
| Arabic | `ar.rs` | National ID, Gulf/Egypt phone | Format |

Each module also includes country-specific phone and IBAN patterns.

## Overlap Resolution & Ensemble Voting

When multiple engines detect overlapping spans for the same text region:

1. **Cluster:** Overlapping spans are grouped via union-find
2. **Vote:** Within each cluster, the highest-confidence match per PII type wins
3. **Agreement bonus:** When 2+ engines agree on the same type at the same span, confidence gets +0.15 (capped at 1.0). This bonus is engine-agnostic — it applies whenever any two engines (regex + NER, CRF + keywords, etc.) agree, not specifically to CRF. **Full tier only** — on Standard and Lite tiers, `ensemble_enabled = false` and the bonus is always 0.0, regardless of engine agreement.
4. **Filter:** Matches below `min_confidence` (default 0.5) are discarded
5. **Regex priority:** On direct type conflict, regex wins (confidence 1.0)

## Nested JSON

Serialized JSON strings within JSON fields are parsed and scanned recursively (max depth 2).

## Scanner Mode Selection

The `scanner_mode` config key controls which semantic backend runs:

| Mode | Backend | Behavior |
|------|---------|----------|
| `"auto"` (default) | Tier-selected | NER if budget allows → CRF fallback → regex-only |
| `"ner"` | Force NER | Warn + fall back to regex if model unavailable |
| `"crf"` | Force CRF | Warn + fall back to regex if model unavailable |
| `"regex"` | Regex only | No semantic scanning; keywords and gazetteer still run |

For all TOML configuration options, see [Detection Engine Configuration](../configure/detection-engine-configuration.md).

## Benchmark Results

**Overall:** 99.7% recall, 100% precision, F1=0.998 across ~400-sample benchmark corpus (regex scanner). HybridScanner 99.7% overall.
