#!/usr/bin/env python3
"""Analyze ai_safety.txt for R2 fine-tuning data quality.

Checks: JSON validity, duplicates, taxonomy correctness, field validation,
content quality issues. Outputs a report and a cleaned JSONL file.
"""

import json
import sys
import os
from collections import Counter, defaultdict
from difflib import SequenceMatcher

INPUT_FILE = os.path.join(os.path.dirname(__file__), "..", "data", "ai_safety.txt")

# --- Valid taxonomy ---
VALID_CATEGORIES = {
    "Art_5_1_a_Deceptive",
    "Art_5_1_b_Age",
    "Art_5_1_b_SocioEcon",
    "Art_5_1_c_Social_Scoring",
}

VALID_SUBCATEGORIES_A = {
    "urgency", "scarcity", "social proof", "social_proof",
    "fear/loss", "fear", "fear_loss",
    "authority", "commercial", "flattery",
    "emotional priming", "emotional_priming",
    "anchoring", "confirmshaming",
}

VALID_SUBCATEGORIES_AGE = {
    "child-targeted", "child_targeted",
    "elderly-targeted", "elderly_targeted",
    "oversimplified risk", "oversimplified_risk",
    # paragraph-level subcategories (used in subcategories[] arrays)
    "gamification", "social pressure", "social_pressure",
    "urgency", "scarcity", "authority", "confirmshaming",
    "fear", "commercial",
}

VALID_SUBCATEGORIES_SOCIOECON = {
    "debt/financial stress", "debt_financial_stress", "debt/financial_stress",
    "health anxiety", "health_anxiety",
    "unemployment", "unemployment_targeting",
    "isolation",
    # paragraph-level
    "urgency", "scarcity", "authority", "commercial", "fear",
}

VALID_SUBCATEGORIES_SOCIAL = {
    "trust score threats", "trust_score_threats",
    "behavioral compliance", "behavioral_compliance",
    "access restriction based on behavior patterns",
    "access_restriction", "access restriction",
}

VALID_SEVERITIES = {"mild", "moderate", "aggressive"}
VALID_DOMAINS = {
    "e-commerce", "healthcare", "financial/debt", "political/propaganda",
    "social media", "gaming", "saas", "education",
    # benign domains
    "technical", "project management", "scientific discussion",
    "casual conversation", "code review",
    # paragraph domains
    "product recommendations", "health advice", "financial guidance",
    "career advice", "technology choices", "subscription services",
    "health", "technology", "financial", "career",
    "social media",
}


def normalize(s):
    """Lowercase and strip for comparison."""
    return s.strip().lower()


def text_key(entry):
    """Extract the text content for duplicate detection."""
    return entry.get("phrase") or entry.get("text") or ""


def entry_type(entry):
    """Classify entry as phrase_manip, phrase_benign, para_manip, or para_benign."""
    if "phrase" in entry:
        return "phrase_benign" if entry.get("label") == "benign" else "phrase_manip"
    elif "text" in entry:
        return "para_benign" if entry.get("label") == "benign" else "para_manip"
    return "unknown"


def validate_subcategory(entry, line_num):
    """Validate subcategory against category. Returns list of issues."""
    issues = []
    cat = entry.get("category", "")

    # Handle both "subcategory" (phrases) and "subcategories" (paragraphs)
    subcats = []
    if "subcategory" in entry:
        subcats = [entry["subcategory"]]
    elif "subcategories" in entry:
        subcats = entry["subcategories"]

    valid_set = set()
    if cat == "Art_5_1_a_Deceptive":
        valid_set = VALID_SUBCATEGORIES_A
    elif cat == "Art_5_1_b_Age":
        valid_set = VALID_SUBCATEGORIES_AGE
    elif cat == "Art_5_1_b_SocioEcon":
        valid_set = VALID_SUBCATEGORIES_SOCIOECON
    elif cat == "Art_5_1_c_Social_Scoring":
        valid_set = VALID_SUBCATEGORIES_SOCIAL

    for sc in subcats:
        if normalize(sc) not in valid_set:
            issues.append(f"  Line {line_num}: Invalid subcategory '{sc}' for {cat}")

    return issues


def find_near_duplicates(entries, threshold=0.85):
    """Find near-duplicate pairs using SequenceMatcher. Returns list of (i, j, ratio)."""
    pairs = []
    texts = [(i, normalize(text_key(e))) for i, e in entries]

    # Group by type to only compare within same type
    by_type = defaultdict(list)
    for i, e in entries:
        by_type[entry_type(e)].append((i, normalize(text_key(e))))

    for etype, group in by_type.items():
        n = len(group)
        for a in range(n):
            for b in range(a + 1, n):
                idx_a, text_a = group[a]
                idx_b, text_b = group[b]
                # Quick length filter
                if abs(len(text_a) - len(text_b)) > max(len(text_a), len(text_b)) * 0.3:
                    continue
                ratio = SequenceMatcher(None, text_a, text_b).ratio()
                if ratio >= threshold:
                    pairs.append((idx_a, idx_b, ratio))
    return pairs


def main():
    print(f"=== R2 Training Data Analysis ===\n")
    print(f"Input: {INPUT_FILE}\n")

    # --- Parse all lines ---
    entries = []  # (line_num, entry_dict)
    parse_errors = []
    blank_lines = 0

    with open(INPUT_FILE, "r", encoding="utf-8") as f:
        for line_num, line in enumerate(f, 1):
            stripped = line.strip()
            if not stripped:
                blank_lines += 1
                continue
            try:
                entry = json.loads(stripped)
                entries.append((line_num, entry))
            except json.JSONDecodeError as e:
                parse_errors.append((line_num, str(e), stripped[:80]))

    total = len(entries)
    print(f"Total lines: {total + blank_lines + len(parse_errors)}")
    print(f"Valid JSON entries: {total}")
    print(f"Blank lines: {blank_lines}")
    print(f"JSON parse errors: {len(parse_errors)}")

    if parse_errors:
        print(f"\n--- JSON Parse Errors ---")
        for ln, err, preview in parse_errors[:20]:
            print(f"  Line {ln}: {err}")
            print(f"    Preview: {preview}...")

    # --- Entry type breakdown ---
    type_counts = Counter()
    for _, e in entries:
        type_counts[entry_type(e)] += 1

    print(f"\n--- Entry Type Breakdown ---")
    for t in ["phrase_manip", "phrase_benign", "para_manip", "para_benign", "unknown"]:
        if type_counts[t] > 0:
            print(f"  {t}: {type_counts[t]}")
    print(f"  Total: {total}")

    # --- Category breakdown ---
    cat_counts = Counter()
    for _, e in entries:
        cat_counts[e.get("category", "MISSING")] += 1

    print(f"\n--- Category Breakdown ---")
    for cat, count in sorted(cat_counts.items()):
        valid = "OK" if cat in VALID_CATEGORIES else "INVALID"
        print(f"  {cat}: {count} [{valid}]")

    invalid_cats = [(ln, e.get("category")) for ln, e in entries if e.get("category") not in VALID_CATEGORIES]
    if invalid_cats:
        print(f"\n  Invalid category entries ({len(invalid_cats)}):")
        for ln, cat in invalid_cats[:10]:
            print(f"    Line {ln}: '{cat}'")

    # --- Subcategory validation ---
    subcat_issues = []
    for ln, e in entries:
        subcat_issues.extend(validate_subcategory(e, ln))

    print(f"\n--- Subcategory Validation ---")
    print(f"  Issues found: {len(subcat_issues)}")
    if subcat_issues:
        for issue in subcat_issues[:20]:
            print(issue)
        if len(subcat_issues) > 20:
            print(f"  ... and {len(subcat_issues) - 20} more")

    # --- Severity validation (phrases only) ---
    severity_issues = []
    severity_counts = Counter()
    for ln, e in entries:
        sev = e.get("severity")
        if sev is not None:
            severity_counts[normalize(sev)] += 1
            if normalize(sev) not in VALID_SEVERITIES:
                severity_issues.append((ln, sev))

    print(f"\n--- Severity Distribution (phrases only) ---")
    for sev, count in sorted(severity_counts.items()):
        valid = "OK" if sev in VALID_SEVERITIES else "INVALID"
        print(f"  {sev}: {count} [{valid}]")

    if severity_issues:
        print(f"  Invalid severity values ({len(severity_issues)}):")
        for ln, sev in severity_issues[:10]:
            print(f"    Line {ln}: '{sev}'")

    # --- Domain validation ---
    domain_counts = Counter()
    for _, e in entries:
        d = e.get("domain", "MISSING")
        domain_counts[normalize(d)] += 1

    print(f"\n--- Domain Distribution ---")
    for dom, count in sorted(domain_counts.items()):
        valid = "OK" if dom in VALID_DOMAINS else "CHECK"
        print(f"  {dom}: {count} [{valid}]")

    # --- Exact duplicates ---
    seen_texts = {}  # normalized text -> first line
    exact_dupes = []
    for ln, e in entries:
        key = normalize(text_key(e))
        if key in seen_texts:
            exact_dupes.append((ln, seen_texts[key], key[:60]))
        else:
            seen_texts[key] = ln

    print(f"\n--- Exact Duplicates ---")
    print(f"  Found: {len(exact_dupes)}")
    if exact_dupes:
        for ln, first_ln, preview in exact_dupes[:20]:
            print(f"  Line {ln} duplicates line {first_ln}: \"{preview}...\"")
        if len(exact_dupes) > 20:
            print(f"  ... and {len(exact_dupes) - 20} more")

    # --- Near duplicates (fuzzy) ---
    print(f"\n--- Near Duplicates (>= 85% similarity) ---")
    print(f"  Scanning... (this may be slow for {total} entries)")

    # Only scan phrases (much faster, paragraphs less likely to be near-dupes)
    phrase_entries = [(ln, e) for ln, e in entries if "phrase" in e]
    near_dupes = find_near_duplicates(phrase_entries, threshold=0.85)

    print(f"  Found: {len(near_dupes)}")
    if near_dupes:
        for idx_a, idx_b, ratio in sorted(near_dupes, key=lambda x: -x[2])[:30]:
            entry_a = next(e for ln, e in entries if ln == idx_a)
            entry_b = next(e for ln, e in entries if ln == idx_b)
            print(f"  Lines {idx_a} <-> {idx_b} ({ratio:.1%}):")
            print(f"    A: \"{text_key(entry_a)[:70]}\"")
            print(f"    B: \"{text_key(entry_b)[:70]}\"")
        if len(near_dupes) > 30:
            print(f"  ... and {len(near_dupes) - 30} more")

    # --- Content quality checks ---
    print(f"\n--- Content Quality Issues ---")
    quality_issues = []

    for ln, e in entries:
        text = text_key(e)
        etype = entry_type(e)

        # Too short
        if etype.startswith("phrase") and len(text) < 5:
            quality_issues.append((ln, "TOO_SHORT", f"Phrase only {len(text)} chars: \"{text}\""))
        if etype.startswith("para") and len(text) < 50:
            quality_issues.append((ln, "TOO_SHORT", f"Paragraph only {len(text)} chars: \"{text[:50]}\""))

        # Too long for a phrase
        if etype.startswith("phrase") and len(text.split()) > 20:
            quality_issues.append((ln, "LONG_PHRASE", f"Phrase has {len(text.split())} words: \"{text[:70]}...\""))

        # Too short for a paragraph
        if etype.startswith("para") and len(text.split()) < 15:
            quality_issues.append((ln, "SHORT_PARA", f"Paragraph only {len(text.split())} words"))

        # Missing required fields
        if "category" not in e:
            quality_issues.append((ln, "MISSING_FIELD", "No 'category' field"))
        if etype.startswith("phrase") and "subcategory" not in e:
            quality_issues.append((ln, "MISSING_FIELD", "Phrase missing 'subcategory'"))
        if etype == "phrase_manip" and "severity" not in e:
            quality_issues.append((ln, "MISSING_FIELD", "Manipulative phrase missing 'severity'"))
        if etype == "para_manip" and "manipulative_span" not in e:
            quality_issues.append((ln, "MISSING_FIELD", "Manipulative paragraph missing 'manipulative_span'"))
        if etype == "para_manip" and "subcategories" not in e:
            quality_issues.append((ln, "MISSING_FIELD", "Manipulative paragraph missing 'subcategories'"))

        # Manipulative span not found in text
        if etype == "para_manip" and "manipulative_span" in e:
            span = e["manipulative_span"]
            if span and span not in text:
                # Try case-insensitive
                if span.lower() not in text.lower():
                    quality_issues.append((ln, "SPAN_MISMATCH", f"manipulative_span not found in text: \"{span[:50]}\""))

        # Benign entry with severity (should not have)
        if etype.endswith("benign") and "severity" in e:
            quality_issues.append((ln, "EXTRA_FIELD", "Benign entry has 'severity' field"))

        # Manipulative entry with label=benign mismatch
        if etype == "phrase_manip" and e.get("label") == "benign":
            quality_issues.append((ln, "LABEL_CONFLICT", "Has both manipulative markers and label=benign"))

    issue_counts = Counter(issue_type for _, issue_type, _ in quality_issues)
    print(f"  Total issues: {len(quality_issues)}")
    for issue_type, count in sorted(issue_counts.items()):
        print(f"    {issue_type}: {count}")

    if quality_issues:
        print(f"\n  Details (first 30):")
        for ln, issue_type, desc in quality_issues[:30]:
            print(f"    Line {ln} [{issue_type}]: {desc}")
        if len(quality_issues) > 30:
            print(f"    ... and {len(quality_issues) - 30} more")

    # --- Category x Type matrix ---
    print(f"\n--- Category x Type Matrix ---")
    matrix = defaultdict(Counter)
    for _, e in entries:
        matrix[e.get("category", "MISSING")][entry_type(e)] += 1

    header = f"  {'Category':<30} {'ph_man':>7} {'ph_ben':>7} {'pa_man':>7} {'pa_ben':>7} {'total':>7}"
    print(header)
    print(f"  {'-'*68}")
    for cat in sorted(matrix.keys()):
        row = matrix[cat]
        t = sum(row.values())
        print(f"  {cat:<30} {row['phrase_manip']:>7} {row['phrase_benign']:>7} {row['para_manip']:>7} {row['para_benign']:>7} {t:>7}")

    # --- Severity x Category (manipulative phrases only) ---
    print(f"\n--- Severity x Category (manipulative phrases) ---")
    sev_matrix = defaultdict(Counter)
    for _, e in entries:
        if entry_type(e) == "phrase_manip" and "severity" in e:
            sev_matrix[e["category"]][normalize(e["severity"])] += 1

    header = f"  {'Category':<30} {'mild':>7} {'moderate':>7} {'aggressive':>7} {'total':>7}"
    print(header)
    print(f"  {'-'*60}")
    for cat in sorted(sev_matrix.keys()):
        row = sev_matrix[cat]
        t = sum(row.values())
        pcts = {s: f"{row[s]/t*100:.0f}%" if t > 0 else "0%" for s in ["mild", "moderate", "aggressive"]}
        print(f"  {cat:<30} {row['mild']:>4} ({pcts['mild']:>3}) {row['moderate']:>4} ({pcts['moderate']:>3}) {row['aggressive']:>4} ({pcts['aggressive']:>3}) {t:>7}")

    # --- Summary ---
    print(f"\n{'='*60}")
    print(f"SUMMARY")
    print(f"{'='*60}")
    print(f"Total valid entries:       {total}")
    print(f"JSON parse errors:         {len(parse_errors)}")
    print(f"Exact duplicates:          {len(exact_dupes)}")
    print(f"Near duplicates (>=85%):   {len(near_dupes)}")
    print(f"Subcategory issues:        {len(subcat_issues)}")
    print(f"Severity issues:           {len(severity_issues)}")
    print(f"Content quality issues:    {len(quality_issues)}")
    print(f"Entries after dedup:       {total - len(exact_dupes)}")

    all_issues = len(parse_errors) + len(exact_dupes) + len(subcat_issues) + len(severity_issues) + len(quality_issues)
    if all_issues == 0:
        print(f"\nDATA IS CLEAN - no issues found.")
    else:
        print(f"\nTotal issues to address:   {all_issues}")


if __name__ == "__main__":
    main()
