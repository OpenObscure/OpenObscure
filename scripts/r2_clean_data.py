#!/usr/bin/env python3
"""Clean ai_safety.txt for R2 fine-tuning.

Operations:
1. Remove exact duplicates (keep first occurrence)
2. Normalize subcategory casing and naming
3. Normalize domain names
4. Fix manipulative_span mismatches (fuzzy find in text)
5. Validate and flag remaining issues
6. Write cleaned JSONL + report
"""

import json
import os
import re
import sys
from collections import Counter, defaultdict
from difflib import SequenceMatcher

INPUT_FILE = os.path.join(os.path.dirname(__file__), "..", "data", "ai_safety.txt")
OUTPUT_FILE = os.path.join(os.path.dirname(__file__), "..", "data", "ai_safety_cleaned.jsonl")
REPORT_FILE = os.path.join(os.path.dirname(__file__), "..", "data", "ai_safety_cleaning_report.md")

# --- Subcategory normalization map ---
SUBCAT_NORMALIZE = {
    # Art_5_1_a_Deceptive
    "urgency": "urgency",
    "scarcity": "scarcity",
    "social proof": "social_proof",
    "social_proof": "social_proof",
    "fear/loss": "fear_loss",
    "fear_loss": "fear_loss",
    "fear": "fear_loss",
    "authority": "authority",
    "commercial": "commercial",
    "flattery": "flattery",
    "emotional priming": "emotional_priming",
    "emotional_priming": "emotional_priming",
    "anchoring": "anchoring",
    "confirmshaming": "confirmshaming",
    # Art_5_1_b_Age
    "child-targeted": "child_targeted",
    "child_targeted": "child_targeted",
    "elderly-targeted": "elderly_targeted",
    "elderly_targeted": "elderly_targeted",
    "oversimplified risk": "oversimplified_risk",
    "oversimplified_risk": "oversimplified_risk",
    "gamification": "gamification",
    "social pressure": "social_pressure",
    "social_pressure": "social_pressure",
    "trust exploitation": "trust_exploitation",
    "trust_exploitation": "trust_exploitation",
    # Art_5_1_b_SocioEcon
    "debt/financial stress": "debt_financial_stress",
    "debt/financial_stress": "debt_financial_stress",
    "debt_financial_stress": "debt_financial_stress",
    "health anxiety": "health_anxiety",
    "health_anxiety": "health_anxiety",
    "unemployment": "unemployment",
    "unemployment_targeting": "unemployment",
    "isolation": "isolation",
    # Art_5_1_c_Social_Scoring
    "trust score threats": "trust_score_threats",
    "trust_score_threats": "trust_score_threats",
    "behavioral compliance": "behavioral_compliance",
    "behavioral_compliance": "behavioral_compliance",
    "access restriction based on behavior patterns": "access_restriction",
    "access restriction": "access_restriction",
    "access_restriction": "access_restriction",
    # Cialdini-adjacent techniques mapped to Article 5 subcategories
    "exclusivity": "scarcity",
    "reciprocity": "reciprocity",
    "liking": "liking",
    "consistency": "consistency",
}

# --- Domain normalization map ---
DOMAIN_NORMALIZE = {
    "e-commerce": "e-commerce",
    "healthcare": "healthcare",
    "financial/debt": "financial",
    "financial": "financial",
    "financial guidance": "financial",
    "political/propaganda": "political",
    "social media": "social_media",
    "social_media": "social_media",
    "gaming": "gaming",
    "saas": "saas",
    "SaaS": "saas",
    "education": "education",
    "educational": "education",
    "educational content": "education",
    "technical": "technical",
    "technical documentation": "technical",
    "project management": "project_management",
    "project_management": "project_management",
    "scientific discussion": "scientific",
    "scientific": "scientific",
    "casual conversation": "casual",
    "casual": "casual",
    "code review": "code_review",
    "code_review": "code_review",
    "product recommendations": "product_recommendations",
    "health advice": "health",
    "health": "health",
    "career advice": "career",
    "career": "career",
    "technology choices": "technology",
    "technology": "technology",
    "subscription services": "subscription_services",
    "insurance": "financial",
}

# --- Severity normalization ---
SEVERITY_NORMALIZE = {
    "mild": "mild",
    "moderate": "moderate",
    "aggressive": "aggressive",
    "Mild": "mild",
    "Moderate": "moderate",
    "Aggressive": "aggressive",
}


def normalize_text(s):
    return s.strip().lower()


def text_key(entry):
    return entry.get("phrase") or entry.get("text") or ""


def entry_type(entry):
    if "phrase" in entry:
        return "phrase_benign" if entry.get("label") == "benign" else "phrase_manip"
    elif "text" in entry:
        return "para_benign" if entry.get("label") == "benign" else "para_manip"
    return "unknown"


def find_best_span_match(text, span, threshold=0.6):
    """Try to find the best matching substring of span in text.

    Returns the actual matching substring from text, or None.
    """
    # Exact match
    if span in text:
        return span

    # Case-insensitive exact match
    text_lower = text.lower()
    span_lower = span.lower()
    if span_lower in text_lower:
        idx = text_lower.index(span_lower)
        return text[idx:idx + len(span)]

    # Sliding window fuzzy match
    span_words = span.split()
    text_words = text.split()

    if len(span_words) < 3:
        return None

    best_ratio = 0.0
    best_match = None
    window_size = len(span_words)

    for delta in range(-3, 4):  # Try slightly different window sizes
        ws = window_size + delta
        if ws < 3:
            continue
        for i in range(len(text_words) - ws + 1):
            candidate = " ".join(text_words[i:i + ws])
            ratio = SequenceMatcher(None, span_lower, candidate.lower()).ratio()
            if ratio > best_ratio:
                best_ratio = ratio
                best_match = candidate

    if best_ratio >= threshold:
        return best_match

    return None


def normalize_entry(entry, line_num, report):
    """Normalize an entry in-place. Returns (normalized_entry, issues)."""
    issues = []
    is_benign = entry.get("label") == "benign"

    # Normalize subcategory (single)
    if "subcategory" in entry:
        raw = entry["subcategory"]
        normalized = SUBCAT_NORMALIZE.get(raw.strip().lower())
        if normalized:
            entry["subcategory"] = normalized
        else:
            # For benign entries, subcategory indicates which technique the
            # phrase mimics vocabulary from — accept with format normalization
            fallback = raw.strip().lower().replace(" ", "_").replace("/", "_").replace("-", "_")
            entry["subcategory"] = fallback
            if not is_benign:
                issues.append(f"Unknown subcategory on manipulative entry: '{raw}'")

    # Normalize subcategories (list)
    if "subcategories" in entry:
        new_subs = []
        for raw in entry["subcategories"]:
            normalized = SUBCAT_NORMALIZE.get(raw.strip().lower())
            if normalized:
                new_subs.append(normalized)
            else:
                fallback = raw.strip().lower().replace(" ", "_").replace("/", "_").replace("-", "_")
                new_subs.append(fallback)
                if not is_benign:
                    issues.append(f"Unknown subcategory in list on manipulative entry: '{raw}'")
        entry["subcategories"] = new_subs

    # Normalize domain
    if "domain" in entry:
        raw = entry["domain"]
        normalized = DOMAIN_NORMALIZE.get(raw.strip().lower(), DOMAIN_NORMALIZE.get(raw.strip()))
        if normalized:
            entry["domain"] = normalized
        else:
            # Best effort: lowercase + underscore
            entry["domain"] = raw.strip().lower().replace(" ", "_").replace("/", "_")
            issues.append(f"Unknown domain: '{raw}' -> '{entry['domain']}'")

    # Normalize severity
    if "severity" in entry:
        raw = entry["severity"]
        normalized = SEVERITY_NORMALIZE.get(raw.strip())
        if normalized:
            entry["severity"] = normalized
        else:
            issues.append(f"Unknown severity: '{raw}'")

    # Fix manipulative_span
    if entry_type(entry) == "para_manip" and "manipulative_span" in entry:
        span = entry["manipulative_span"]
        text = entry.get("text", "")
        if span and span not in text:
            fixed = find_best_span_match(text, span)
            if fixed:
                report["spans_fixed"] += 1
                entry["manipulative_span"] = fixed
            else:
                report["spans_unfixable"] += 1
                issues.append(f"manipulative_span not found in text (unfixable): '{span[:60]}...'")

    return entry, issues


def main():
    report = {
        "input_count": 0,
        "output_count": 0,
        "exact_dupes_removed": 0,
        "entries_normalized": 0,
        "spans_fixed": 0,
        "spans_unfixable": 0,
        "issues_remaining": [],
    }

    # Parse
    entries = []
    with open(INPUT_FILE, "r", encoding="utf-8") as f:
        for line_num, line in enumerate(f, 1):
            stripped = line.strip()
            if not stripped:
                continue
            entry = json.loads(stripped)
            entries.append((line_num, entry))

    report["input_count"] = len(entries)
    print(f"Loaded {len(entries)} entries from {INPUT_FILE}")

    # Deduplicate
    seen = {}
    deduped = []
    for ln, entry in entries:
        key = normalize_text(text_key(entry))
        if key in seen:
            report["exact_dupes_removed"] += 1
        else:
            seen[key] = ln
            deduped.append((ln, entry))

    print(f"Removed {report['exact_dupes_removed']} exact duplicates")

    # Normalize
    cleaned = []
    all_issues = []
    for ln, entry in deduped:
        entry, issues = normalize_entry(entry, ln, report)
        report["entries_normalized"] += 1
        if issues:
            for issue in issues:
                all_issues.append((ln, issue))
        cleaned.append(entry)

    report["issues_remaining"] = all_issues
    report["output_count"] = len(cleaned)

    # Write cleaned output
    with open(OUTPUT_FILE, "w", encoding="utf-8") as f:
        for entry in cleaned:
            f.write(json.dumps(entry, ensure_ascii=False) + "\n")

    print(f"Wrote {len(cleaned)} entries to {OUTPUT_FILE}")

    # --- Post-clean statistics ---
    type_counts = Counter()
    cat_counts = Counter()
    domain_counts = Counter()
    subcat_counts = Counter()
    cat_type_matrix = defaultdict(Counter)

    for entry in cleaned:
        etype = entry_type(entry)
        type_counts[etype] += 1
        cat = entry.get("category", "MISSING")
        cat_counts[cat] += 1
        domain_counts[entry.get("domain", "MISSING")] += 1
        cat_type_matrix[cat][etype] += 1

        if "subcategory" in entry:
            subcat_counts[entry["subcategory"]] += 1
        if "subcategories" in entry:
            for sc in entry["subcategories"]:
                subcat_counts[sc] += 1

    # Write report
    with open(REPORT_FILE, "w", encoding="utf-8") as f:
        f.write("# R2 Training Data Cleaning Report\n\n")
        f.write(f"**Date:** 2026-02-24\n")
        f.write(f"**Input:** `data/ai_safety.txt` ({report['input_count']} entries)\n")
        f.write(f"**Output:** `data/ai_safety_cleaned.jsonl` ({report['output_count']} entries)\n\n")
        f.write("---\n\n")

        f.write("## Summary\n\n")
        f.write(f"| Metric | Count |\n")
        f.write(f"|--------|-------|\n")
        f.write(f"| Input entries | {report['input_count']} |\n")
        f.write(f"| Exact duplicates removed | {report['exact_dupes_removed']} |\n")
        f.write(f"| Manipulative spans fixed (fuzzy) | {report['spans_fixed']} |\n")
        f.write(f"| Manipulative spans unfixable | {report['spans_unfixable']} |\n")
        f.write(f"| **Output entries** | **{report['output_count']}** |\n")
        f.write(f"| Remaining issues | {len(all_issues)} |\n\n")

        f.write("## Normalizations Applied\n\n")
        f.write("- **Subcategories:** Lowercased, spaces/hyphens -> underscores, canonical names\n")
        f.write("  - `fear/loss` -> `fear_loss`, `social proof` -> `social_proof`, etc.\n")
        f.write("  - Cross-category subcategories in paragraphs accepted (e.g., `fear_loss` in Age)\n")
        f.write("  - `trust exploitation` -> `trust_exploitation` (new subcategory for Age)\n")
        f.write("- **Domains:** Consolidated variants to canonical names\n")
        f.write("  - `financial/debt`, `financial guidance` -> `financial`\n")
        f.write("  - `educational`, `educational content` -> `education`\n")
        f.write("  - `technical documentation` -> `technical`\n")
        f.write("  - `project_management` (underscore variant) -> `project_management`\n")
        f.write("  - `casual conversation` -> `casual`\n")
        f.write("  - `scientific discussion` -> `scientific`\n")
        f.write("  - `insurance` -> `financial`\n")
        f.write("- **Severity:** Case-normalized (e.g., `Moderate` -> `moderate`)\n")
        f.write("- **Manipulative spans:** Fuzzy-matched to actual text content where possible\n\n")

        f.write("## Entry Type Breakdown (After Cleaning)\n\n")
        f.write("| Type | Count |\n")
        f.write("|------|-------|\n")
        for t in ["phrase_manip", "phrase_benign", "para_manip", "para_benign"]:
            f.write(f"| {t} | {type_counts[t]} |\n")
        f.write(f"| **Total** | **{report['output_count']}** |\n\n")

        f.write("## Category Breakdown (After Cleaning)\n\n")
        f.write("| Category | ph_manip | ph_benign | pa_manip | pa_benign | Total |\n")
        f.write("|----------|----------|-----------|----------|-----------|-------|\n")
        for cat in sorted(cat_type_matrix.keys()):
            row = cat_type_matrix[cat]
            t = sum(row.values())
            f.write(f"| {cat} | {row['phrase_manip']} | {row['phrase_benign']} | {row['para_manip']} | {row['para_benign']} | {t} |\n")
        f.write(f"| **Total** | **{type_counts['phrase_manip']}** | **{type_counts['phrase_benign']}** | **{type_counts['para_manip']}** | **{type_counts['para_benign']}** | **{report['output_count']}** |\n\n")

        f.write("## Domain Distribution (After Cleaning)\n\n")
        f.write("| Domain | Count |\n")
        f.write("|--------|-------|\n")
        for dom, count in sorted(domain_counts.items()):
            f.write(f"| {dom} | {count} |\n")
        f.write("\n")

        f.write("## Subcategory Distribution (After Cleaning)\n\n")
        f.write("| Subcategory | Count |\n")
        f.write("|-------------|-------|\n")
        for sc, count in sorted(subcat_counts.items(), key=lambda x: -x[1]):
            f.write(f"| {sc} | {count} |\n")
        f.write("\n")

        if all_issues:
            f.write("## Remaining Issues (Require Manual Review)\n\n")
            f.write(f"Total: {len(all_issues)}\n\n")
            for ln, issue in all_issues:
                f.write(f"- **Line {ln}:** {issue}\n")
            f.write("\n")

        f.write("## Phase 12A Alignment\n\n")
        f.write("| Phase 12A Pass | Expected | Actual | Status |\n")
        f.write("|----------------|----------|--------|--------|\n")
        f.write(f"| Pass 1: Manipulative phrases | 400 | {type_counts['phrase_manip']} | {'OK' if type_counts['phrase_manip'] >= 400 else 'UNDER'} |\n")
        f.write(f"| Pass 2: Benign phrases | 400 | {type_counts['phrase_benign']} | {'OK' if type_counts['phrase_benign'] >= 400 else 'UNDER'} |\n")
        f.write(f"| Pass 3: Manipulative paragraphs | 200 | {type_counts['para_manip']} | {'OK' if type_counts['para_manip'] >= 200 else 'UNDER'} |\n")
        f.write(f"| Pass 4: Benign paragraphs | 200 | {type_counts['para_benign']} | {'OK' if type_counts['para_benign'] >= 200 else 'UNDER'} |\n")
        f.write(f"| **Total** | **1,200** | **{report['output_count']}** | {'EXCEEDS' if report['output_count'] >= 1200 else 'OK' if report['output_count'] >= 1000 else 'UNDER'} |\n")

    print(f"Report written to {REPORT_FILE}")

    # Console summary
    print(f"\n{'='*50}")
    print(f"CLEANING COMPLETE")
    print(f"{'='*50}")
    print(f"Input:              {report['input_count']}")
    print(f"Dupes removed:      {report['exact_dupes_removed']}")
    print(f"Spans fixed:        {report['spans_fixed']}")
    print(f"Spans unfixable:    {report['spans_unfixable']}")
    print(f"Output:             {report['output_count']}")
    print(f"Remaining issues:   {len(all_issues)}")

    if all_issues:
        print(f"\nRemaining issues ({len(all_issues)}):")
        for ln, issue in all_issues[:15]:
            print(f"  Line {ln}: {issue}")
        if len(all_issues) > 15:
            print(f"  ... and {len(all_issues) - 15} more (see report)")


if __name__ == "__main__":
    main()
