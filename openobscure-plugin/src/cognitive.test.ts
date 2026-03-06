import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  tokenize, scanDictionary, computeSeverity,
  formatWarningLabel, scanPersuasion, totalPhraseCount,
  getCategorySize,
} from "./cognitive";
import type { PersuasionMatch, PersuasionCategory } from "./cognitive";

// ── Dictionary size — pinned to Rust parity ─────────────────────────

describe("Persuasion Dictionary", () => {
  it("has exactly 248 total phrases (Rust parity)", () => {
    assert.equal(totalPhraseCount(), 248, `Expected 248 total phrases (Rust parity), got ${totalPhraseCount()}`);
  });
});

// ── Per-category phrase count parity (Tier 1a) ─────────────────────

describe("Per-Category Phrase Count Parity", () => {
  const expected: Record<string, number> = {
    URGENCY: 38,
    SCARCITY: 28,
    SOCIAL_PROOF: 35,
    FEAR: 35,
    AUTHORITY: 35,
    COMMERCIAL: 47,
    FLATTERY: 30,
  };

  it("Urgency: 38 phrases", () => {
    assert.equal(getCategorySize("Urgency"), expected.URGENCY);
  });

  it("Scarcity: 28 phrases", () => {
    assert.equal(getCategorySize("Scarcity"), expected.SCARCITY);
  });

  it("SocialProof: 35 phrases", () => {
    assert.equal(getCategorySize("SocialProof"), expected.SOCIAL_PROOF);
  });

  it("Fear: 35 phrases", () => {
    assert.equal(getCategorySize("Fear"), expected.FEAR);
  });

  it("Authority: 35 phrases", () => {
    assert.equal(getCategorySize("Authority"), expected.AUTHORITY);
  });

  it("Commercial: 47 phrases", () => {
    assert.equal(getCategorySize("Commercial"), expected.COMMERCIAL);
  });

  it("Flattery: 30 phrases", () => {
    assert.equal(getCategorySize("Flattery"), expected.FLATTERY);
  });

  it("sum of per-category counts equals total", () => {
    const sum = Object.values(expected).reduce((a, b) => a + b, 0);
    assert.equal(sum, 248);
    assert.equal(totalPhraseCount(), sum);
  });
});

// ── Tokenizer ────────────────────────────────────────────────────────

describe("Tokenizer", () => {
  it("splits on word boundaries", () => {
    const tokens = tokenize("hello world");
    assert.equal(tokens.length, 2);
    assert.equal(tokens[0].text, "hello");
    assert.equal(tokens[1].text, "world");
  });

  it("preserves byte offsets", () => {
    const text = "  hello  world  ";
    const tokens = tokenize(text);
    assert.equal(tokens.length, 2);
    assert.equal(text.slice(tokens[0].byteStart, tokens[0].byteEnd), "hello");
    assert.equal(text.slice(tokens[1].byteStart, tokens[1].byteEnd), "world");
  });

  it("treats hyphens and apostrophes as word chars", () => {
    const tokens = tokenize("don't time-sensitive");
    assert.equal(tokens.length, 2);
    assert.equal(tokens[0].text, "don't");
    assert.equal(tokens[1].text, "time-sensitive");
  });

  it("trims leading/trailing hyphens and apostrophes", () => {
    const tokens = tokenize("--hello-- 'world'");
    assert.equal(tokens.length, 2);
    assert.equal(tokens[0].text, "hello");
    assert.equal(tokens[1].text, "world");
  });

  it("handles empty string", () => {
    assert.equal(tokenize("").length, 0);
  });
});

// ── Category detection ───────────────────────────────────────────────

describe("Category Detection", () => {
  it("detects urgency phrases", () => {
    const matches = scanDictionary("Act now before this limited time offer expires!");
    assert.ok(matches.some((m) => m.category === "Urgency"));
  });

  it("detects scarcity phrases", () => {
    const matches = scanDictionary("This is an exclusive offer with limited supply.");
    assert.ok(matches.some((m) => m.category === "Scarcity"));
  });

  it("detects social proof phrases", () => {
    const matches = scanDictionary("Most people choose this option. It's the most popular choice.");
    assert.ok(matches.some((m) => m.category === "SocialProof"));
  });

  it("detects fear phrases", () => {
    const matches = scanDictionary("You could lose everything if you don't act. Don't fall behind.");
    assert.ok(matches.some((m) => m.category === "Fear"));
  });

  it("detects authority phrases", () => {
    const matches = scanDictionary("Experts agree this is the best approach. Studies show results.");
    assert.ok(matches.some((m) => m.category === "Authority"));
  });

  it("detects commercial phrases", () => {
    const matches = scanDictionary("Get a free trial today with our special offer and save money.");
    assert.ok(matches.some((m) => m.category === "Commercial"));
  });

  it("detects flattery phrases", () => {
    const matches = scanDictionary("Smart choice! You deserve the best. People like you succeed.");
    assert.ok(matches.some((m) => m.category === "Flattery"));
  });

  it("is case insensitive", () => {
    const matches = scanDictionary("ACT NOW! EXPERTS AGREE this is EXCLUSIVE.");
    assert.ok(matches.length >= 2, `Expected >= 2 matches, got ${matches.length}`);
  });

  it("produces no false positives on clean text", () => {
    const matches = scanDictionary(
      "The function returns a list of integers sorted in ascending order. " +
      "Use the map method to transform each element.",
    );
    assert.equal(matches.length, 0, "Clean technical text should produce no matches");
  });
});

// ── Overlap dedup ────────────────────────────────────────────────────

describe("Overlap Dedup", () => {
  it("prefers 3-word phrase over 2-word overlap", () => {
    const matches = scanDictionary("This is a limited time offer.");
    const limited = matches.filter((m) =>
      m.phrase.toLowerCase().includes("limited"),
    );
    assert.equal(limited.length, 1, "Should have one match for 'limited time offer'");
    assert.ok(
      limited[0].phrase.toLowerCase().includes("limited time offer"),
      "Should prefer the longer 3-word phrase",
    );
  });
});

// ── Match offsets ────────────────────────────────────────────────────

describe("Match Offsets", () => {
  it("byte offsets point to matched text", () => {
    const text = "Please act now before it expires.";
    const matches = scanDictionary(text);
    for (const m of matches) {
      assert.equal(
        text.slice(m.start, m.end).toLowerCase(),
        m.phrase.toLowerCase(),
        `Offset [${m.start},${m.end}) should match phrase "${m.phrase}"`,
      );
    }
  });
});

// ── Multiple categories ──────────────────────────────────────────────

describe("Multiple Categories", () => {
  it("detects 3+ categories in mixed text", () => {
    const matches = scanDictionary(
      "Act now! Experts agree this exclusive offer is a smart choice. You could lose out.",
    );
    const categories = new Set(matches.map((m) => m.category));
    assert.ok(categories.size >= 3, `Expected >= 3 categories, got ${categories.size}`);
  });
});

// ── Severity computation ─────────────────────────────────────────────

describe("Severity Computation", () => {
  const mkMatch = (cat: PersuasionCategory): PersuasionMatch => ({
    category: cat, start: 0, end: 1, phrase: "x",
  });

  it("single category 1-2 matches -> Notice", () => {
    assert.equal(computeSeverity([mkMatch("Flattery")], ["Flattery"]), "Notice");
  });

  it("2 categories -> Warning", () => {
    const matches = [mkMatch("Urgency"), mkMatch("Flattery")];
    assert.equal(computeSeverity(matches, ["Urgency", "Flattery"]), "Warning");
  });

  it("3+ matches single category -> Warning", () => {
    const matches = [mkMatch("Urgency"), mkMatch("Urgency"), mkMatch("Urgency")];
    assert.equal(computeSeverity(matches, ["Urgency"]), "Warning");
  });

  it("4+ categories -> Caution", () => {
    const cats: PersuasionCategory[] = ["Urgency", "Fear", "Authority", "Commercial"];
    const matches = cats.map(mkMatch);
    assert.equal(computeSeverity(matches, cats), "Caution");
  });

  it("Commercial + Fear -> Caution", () => {
    const matches = [mkMatch("Commercial"), mkMatch("Fear")];
    assert.equal(computeSeverity(matches, ["Commercial", "Fear"]), "Caution");
  });

  it("Commercial + Urgency -> Caution", () => {
    const matches = [mkMatch("Commercial"), mkMatch("Urgency")];
    assert.equal(computeSeverity(matches, ["Commercial", "Urgency"]), "Caution");
  });
});

// ── Warning label formatting ─────────────────────────────────────────

describe("Warning Label", () => {
  it("Notice format", () => {
    const label = formatWarningLabel("Notice", ["Flattery"]);
    assert.ok(label.includes("--- OpenObscure WARNING ---"));
    assert.ok(label.includes("Detected: Flattery"));
    assert.ok(label.endsWith("---\n\n"));
    assert.ok(!label.includes("language patterns"));
  });

  it("Warning format includes influence message", () => {
    const label = formatWarningLabel("Warning", ["Urgency", "Commercial"]);
    assert.ok(label.includes("--- OpenObscure WARNING ---"));
    assert.ok(label.includes("language patterns associated with influence tactics"));
    assert.ok(label.includes("\u2022"), "Should use bullet separator");
  });

  it("Caution format includes verification advice", () => {
    const label = formatWarningLabel("Caution", ["Fear", "Commercial", "Urgency", "Authority"]);
    assert.ok(label.includes("Pause and verify with objective evidence"));
    assert.ok(label.endsWith("---\n\n"));
  });

  it("deduplicates categories", () => {
    const label = formatWarningLabel("Notice", ["Urgency", "Urgency"]);
    const count = (label.match(/Urgency/g) || []).length;
    assert.equal(count, 1, "Should deduplicate repeated categories");
  });

  it("sorts category names alphabetically", () => {
    const label = formatWarningLabel("Warning", ["Urgency", "Authority"]);
    const authIdx = label.indexOf("Authority");
    const urgIdx = label.indexOf("Urgency");
    assert.ok(authIdx < urgIdx, "Categories should be sorted alphabetically");
  });
});

// ── Cognitive edge cases (Tier 3b) ──────────────────────────────────

describe("Cognitive Edge Cases", () => {
  it("handles unicode text without crash", () => {
    const matches = scanDictionary("Achetez maintenant! 🚀 立即行动 experts agree");
    assert.ok(matches.some((m) => m.category === "Authority"));
  });

  it("handles very long text", () => {
    const long = "This is fine. ".repeat(5000) + "Act now!";
    const matches = scanDictionary(long);
    assert.ok(matches.some((m) => m.category === "Urgency"));
  });

  it("handles HTML tags in text", () => {
    const matches = scanDictionary("<b>Act now</b> before <em>time is running out</em>!");
    assert.ok(matches.some((m) => m.category === "Urgency"));
  });

  it("handles newlines and tabs", () => {
    const matches = scanDictionary("Please\n\tact now\n\tbefore it expires");
    assert.ok(matches.some((m) => m.category === "Urgency"));
  });

  it("handles smart quotes / curly apostrophes as non-word chars", () => {
    // Smart quotes (U+2018/U+2019) are NOT word chars — phrases with ' won't match via smart quotes
    const matches = scanDictionary("don\u2019t wait");
    // This should NOT match because the tokenizer treats \u2019 as a word boundary
    // The phrase "don't wait" uses ASCII apostrophe
    assert.equal(matches.length, 0);
  });

  it("handles repeated whitespace", () => {
    const matches = scanDictionary("act   now   please");
    // Multiple spaces — tokenizer still matches; phrase includes original spacing
    assert.ok(matches.some((m) => m.category === "Urgency"), "Should detect urgency in spaced text");
  });
});

// ── Severity boundary tests (Tier 3c) ──────────────────────────────

describe("Severity Boundaries", () => {
  const mkMatch = (cat: PersuasionCategory): PersuasionMatch => ({
    category: cat, start: 0, end: 1, phrase: "x",
  });

  it("0 matches -> no severity (handled by caller)", () => {
    // computeSeverity with empty input returns Notice (lowest)
    assert.equal(computeSeverity([], []), "Notice");
  });

  it("1 match 1 category -> Notice", () => {
    assert.equal(computeSeverity([mkMatch("Authority")], ["Authority"]), "Notice");
  });

  it("2 matches 1 category -> Notice", () => {
    const matches = [mkMatch("Scarcity"), mkMatch("Scarcity")];
    assert.equal(computeSeverity(matches, ["Scarcity"]), "Notice");
  });

  it("3 matches 1 category -> Warning (boundary)", () => {
    const matches = [mkMatch("Fear"), mkMatch("Fear"), mkMatch("Fear")];
    assert.equal(computeSeverity(matches, ["Fear"]), "Warning");
  });

  it("exactly 2 categories -> Warning", () => {
    const matches = [mkMatch("Urgency"), mkMatch("Scarcity")];
    assert.equal(computeSeverity(matches, ["Urgency", "Scarcity"]), "Warning");
  });

  it("exactly 3 categories -> Warning (not yet Caution)", () => {
    const cats: PersuasionCategory[] = ["Urgency", "Fear", "Authority"];
    assert.equal(computeSeverity(cats.map(mkMatch), cats), "Warning");
  });

  it("exactly 4 categories -> Caution (boundary)", () => {
    const cats: PersuasionCategory[] = ["Urgency", "Fear", "Authority", "Scarcity"];
    assert.equal(computeSeverity(cats.map(mkMatch), cats), "Caution");
  });

  it("Commercial + Fear -> Caution (combo override)", () => {
    assert.equal(computeSeverity([mkMatch("Commercial"), mkMatch("Fear")], ["Commercial", "Fear"]), "Caution");
  });

  it("Commercial + Urgency -> Caution (combo override)", () => {
    assert.equal(computeSeverity([mkMatch("Commercial"), mkMatch("Urgency")], ["Commercial", "Urgency"]), "Caution");
  });

  it("Commercial + Scarcity -> Warning (no combo override)", () => {
    assert.equal(computeSeverity([mkMatch("Commercial"), mkMatch("Scarcity")], ["Commercial", "Scarcity"]), "Warning");
  });

  it("Commercial + Flattery -> Warning (no combo override)", () => {
    assert.equal(computeSeverity([mkMatch("Commercial"), mkMatch("Flattery")], ["Commercial", "Flattery"]), "Warning");
  });
});

// ── Warning label exact format (Tier 3d) ────────────────────────────

describe("Warning Label Exact Format", () => {
  it("Notice matches Rust format exactly", () => {
    const label = formatWarningLabel("Notice", ["Authority"]);
    assert.equal(label,
      "--- OpenObscure WARNING ---\n" +
      "Detected: Authority\n" +
      "---\n\n"
    );
  });

  it("Warning matches Rust format exactly", () => {
    const label = formatWarningLabel("Warning", ["Fear", "Urgency"]);
    assert.equal(label,
      "--- OpenObscure WARNING ---\n" +
      "Detected: Fear \u2022 Urgency\n" +
      "This response contains language patterns associated with influence tactics.\n" +
      "---\n\n"
    );
  });

  it("Caution matches Rust format exactly", () => {
    const label = formatWarningLabel("Caution", ["Commercial", "Fear"]);
    assert.equal(label,
      "--- OpenObscure WARNING ---\n" +
      "Detected: Commercial \u2022 Fear\n" +
      "Recommendation: Pause and verify with objective evidence before acting.\n" +
      "---\n\n"
    );
  });

  it("SocialProof displays as 'Social Proof'", () => {
    const label = formatWarningLabel("Notice", ["SocialProof"]);
    assert.ok(label.includes("Social Proof"), "SocialProof should display as 'Social Proof'");
  });
});

// ── Public API: scanPersuasion ───────────────────────────────────────

describe("scanPersuasion", () => {
  it("returns null for clean text", () => {
    const result = scanPersuasion("Here is a Python function that sorts a list.");
    assert.equal(result, null);
  });

  it("returns result with severity for persuasive text", () => {
    const result = scanPersuasion("Act now! This is a smart choice.");
    assert.ok(result !== null);
    assert.ok(result!.matches.length > 0);
    assert.ok(result!.categories.length > 0);
    assert.ok(["Notice", "Warning", "Caution"].includes(result!.severity));
    assert.equal(result!.source, "js");
    assert.ok(result!.scanTimeUs >= 0);
  });

  it("detects Caution-level manipulation", () => {
    const result = scanPersuasion(
      "Buy now or you could lose this amazing deal forever! Experts agree.",
    );
    assert.ok(result !== null);
    // Should detect Commercial + Fear/Urgency + Authority
    assert.ok(result!.categories.length >= 2);
  });

  it("empty string returns null", () => {
    assert.equal(scanPersuasion(""), null);
  });
});
