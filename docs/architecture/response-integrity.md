# Response Integrity — Cognitive Firewall Architecture

> **Role in OpenObscure:** The cognitive firewall scans LLM **responses** for manipulation techniques before they reach users, providing client-side enforcement of EU AI Act Article 5 prohibitions on subliminal/manipulative techniques. For the full system context, see [System Overview](system-overview.md).
>
> **Implementation:** `openobscure-core/src/response_integrity.rs` and related modules. For configuration, see [Config Reference](../configure/config-reference.md#response_integrity).

---

## Module Map

| Module | Role |
|--------|------|
| `persuasion_dict.rs` | R1 dictionary (~250 phrases, 7 Cialdini categories, HashSet O(1) lookup) |
| `response_integrity.rs` | R1→R2 cascade: sensitivity tiers, R2Role dispatch, severity computation |
| `ri_model.rs` | R2 TinyBERT FP32 ONNX multi-label classifier (4 EU AI Act Article 5 categories) |
| `response_format.rs` | Multi-LLM response format detection (Anthropic/OpenAI/Gemini/Cohere/Ollama/plaintext) |

---

## Two-Tier Cascade

### R1 — Pattern-Based Dictionary

~250 phrases across 7 Cialdini categories: urgency, scarcity, social proof, fear, authority, commercial, flattery. Runs on every response, <1ms.

### R2 — TinyBERT ONNX Classifier

Multi-label classifier for 4 EU AI Act Article 5 categories. Runs conditionally based on sensitivity level and R1 results (~30ms when triggered).

**R2 roles:**
- **Confirm** — R2 agrees with R1's findings
- **Suppress** — R2 overrides R1 false positive (single-category R1 hits only)
- **Upgrade** — R2 adds categories R1 missed
- **Discover** — R2 catches paraphrased manipulation R1 missed

Multi-category R1 hits (2+ categories) are strong enough to stand on their own — R2 disagreement is treated as Confirm rather than Suppress.

---

## Sensitivity Levels

| Sensitivity | R1 | R2 | Behavior |
|-------------|----|----|----------|
| `off` | — | — | Disabled |
| `low` (default) | Every response | On R1 flags only | Minimal overhead (<1ms typical) |
| `medium` | Every response | On R1 flags + sample rate | `ri_sample_rate` fraction of unflagged responses |
| `high` | Every response | Every response | Full coverage (~30ms per response) |

---

## Severity Tiers

| Tier | Trigger | Action |
|------|---------|--------|
| Notice | 1 category flagged | Log only |
| Warning | 2–3 categories flagged | Prepend warning label (if `log_only = false`) |
| Caution | 4+ categories flagged | Prepend warning label (if `log_only = false`) |

---

## Request Flow Integration

Response integrity scanning occurs at step 12b of the proxy request flow:

1. Extract text from response JSON (auto-detects Anthropic/OpenAI/Gemini/Cohere/Ollama/plaintext format)
2. R1: Dictionary scan (~250 phrases, 7 categories)
3. R2: If triggered by sensitivity/R1 result, run TinyBERT classifier
4. Cascade: Confirm/Suppress/Upgrade/Discover
5. If flagged & `log_only = false`: prepend warning label

Fail-open on errors — response integrity never blocks delivery.
