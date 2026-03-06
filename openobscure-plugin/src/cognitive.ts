/**
 * Cognitive Firewall — embedded persuasion/manipulation detection for L1 plugin.
 *
 * Mirrors the L0 Rust persuasion_dict.rs + response_integrity.rs logic:
 *   - 7 Cialdini categories (Urgency, Scarcity, SocialProof, Fear, Authority, Commercial, Flattery)
 *   - 3->2->1 word scanning (longest match first, overlap dedup)
 *   - Severity computation: Notice / Warning / Caution
 *   - Warning label formatting
 *
 * Tries NAPI native scanner first, falls back to pure JS dictionary.
 */

// ── Types ────────────────────────────────────────────────────────────

export type PersuasionCategory =
  | "Urgency"
  | "Scarcity"
  | "SocialProof"
  | "Fear"
  | "Authority"
  | "Commercial"
  | "Flattery";

export type SeverityTier = "Notice" | "Warning" | "Caution";

export interface PersuasionMatch {
  category: PersuasionCategory;
  start: number;
  end: number;
  phrase: string;
}

export interface PersuasionScanResult {
  matches: PersuasionMatch[];
  categories: PersuasionCategory[];
  severity: SeverityTier;
  scanTimeUs: number;
  source: "napi" | "js";
}

// ── Category Dictionaries ────────────────────────────────────────────

const URGENCY_TERMS: ReadonlySet<string> = new Set([
  "act now", "act fast", "act immediately", "act quickly",
  "don't wait", "don't delay", "don't hesitate", "don't miss out",
  "hurry", "hurry up", "right now", "right away", "immediately",
  "today only", "now or never", "time is running out",
  "running out of time", "before it's too late", "while you still can",
  "last chance", "final chance", "final opportunity",
  "limited time", "limited time offer", "limited time only",
  "time-sensitive", "urgent", "urgently", "asap",
  "expires soon", "expiring soon", "offer expires",
  "deadline approaching", "closing soon", "ending soon",
  "won't last", "won't last long", "this won't last",
]);

const SCARCITY_TERMS: ReadonlySet<string> = new Set([
  "only a few left", "limited supply", "limited availability",
  "limited quantities", "limited edition", "while supplies last",
  "selling fast", "selling out", "almost gone", "nearly sold out",
  "exclusive offer", "exclusive deal", "exclusive access",
  "exclusive opportunity", "one-time offer", "one-time deal",
  "one-time opportunity", "rare opportunity", "rare chance",
  "hard to find", "in high demand", "high demand", "going fast",
  "few remaining", "spots filling up", "limited spots",
  "limited seats", "first come first served",
]);

const SOCIAL_PROOF_TERMS: ReadonlySet<string> = new Set([
  "everyone is", "everyone loves", "everyone agrees", "everyone knows",
  "most people", "most users", "most customers",
  "millions of people", "millions of users", "thousands of people",
  "trusted by millions", "trusted by thousands",
  "join millions", "join thousands",
  "people are switching", "people are choosing",
  "the most popular", "best-selling", "top-rated", "highest-rated",
  "award-winning", "widely used", "widely adopted",
  "industry standard", "industry leading", "market leader",
  "don't be left behind", "you'll be left behind",
  "everyone else is", "your peers are", "your competitors are",
  "trending", "viral", "five-star", "five star",
]);

const FEAR_TERMS: ReadonlySet<string> = new Set([
  "you could lose", "you will lose", "you might lose",
  "risk of losing", "risk of missing", "fear of missing out", "fomo",
  "don't fall behind", "falling behind", "left behind",
  "miss out", "missing out",
  "you'll regret", "you will regret", "you might regret", "regret not",
  "can't afford to miss", "can't afford to wait",
  "at risk", "at serious risk", "dangerous to ignore",
  "catastrophic", "devastating consequences", "dire consequences",
  "irreversible damage", "irreversible consequences",
  "point of no return", "too late", "it may be too late",
  "before it's too late", "what if you don't", "imagine losing",
  "think about what happens", "worst case scenario", "you can't afford",
]);

const AUTHORITY_TERMS: ReadonlySet<string> = new Set([
  "experts agree", "experts recommend", "experts say", "experts suggest",
  "studies show", "studies prove", "studies confirm",
  "research shows", "research proves", "research confirms",
  "scientifically proven", "clinically proven", "clinically tested",
  "doctor recommended", "doctor approved",
  "recommended by professionals", "recommended by experts",
  "endorsed by", "backed by science", "backed by research",
  "according to experts", "leading authorities", "leading experts",
  "top experts", "world-renowned", "peer-reviewed",
  "harvard study", "stanford study", "published research",
  "as seen on", "featured in", "recognized by",
  "certified by", "approved by", "trusted by professionals",
]);

const COMMERCIAL_TERMS: ReadonlySet<string> = new Set([
  "best deal", "best value", "best price",
  "great deal", "great value", "amazing deal", "incredible deal",
  "unbeatable price", "unbeatable deal", "lowest price",
  "save money", "save big", "save now", "huge savings", "massive savings",
  "discount", "special discount", "special offer", "special price",
  "special promotion", "free trial", "free shipping", "free bonus",
  "risk-free", "money back", "money-back guarantee",
  "satisfaction guaranteed", "no obligation", "no commitment",
  "cancel anytime", "buy now", "order now", "sign up now",
  "subscribe now", "get started now", "click here", "click now",
  "add to cart", "checkout now", "purchase now",
  "invest in", "invest now", "upgrade now",
  "premium", "pro version", "unlock", "unlock full",
]);

const FLATTERY_TERMS: ReadonlySet<string> = new Set([
  "smart choice", "smart decision", "wise choice", "wise decision",
  "great choice", "excellent choice", "perfect choice",
  "you clearly understand", "you obviously know", "you already know",
  "as someone who values", "as someone who cares",
  "as someone who understands", "you deserve", "you deserve the best",
  "you're worth it", "treat yourself", "reward yourself",
  "you've earned it", "you've earned this",
  "savvy", "discerning", "sophisticated",
  "someone like you", "people like you", "a person of your caliber",
  "forward-thinking", "ahead of the curve", "early adopter",
  "thought leader",
]);

const CATEGORY_SETS: ReadonlyArray<[PersuasionCategory, ReadonlySet<string>]> = [
  ["Urgency", URGENCY_TERMS],
  ["Scarcity", SCARCITY_TERMS],
  ["SocialProof", SOCIAL_PROOF_TERMS],
  ["Fear", FEAR_TERMS],
  ["Authority", AUTHORITY_TERMS],
  ["Commercial", COMMERCIAL_TERMS],
  ["Flattery", FLATTERY_TERMS],
];

// ── Tokenizer ────────────────────────────────────────────────────────

interface Token {
  text: string;
  byteStart: number;
  byteEnd: number;
}

/**
 * Tokenize text on word boundaries, preserving byte offsets.
 * Matches the Rust tokenizer: alphanumeric + '-' + '\'' are word chars,
 * leading/trailing hyphens and apostrophes are trimmed.
 */
export function tokenize(text: string): Token[] {
  const tokens: Token[] = [];
  let start: number | null = null;

  for (let i = 0; i <= text.length; i++) {
    const c = i < text.length ? text[i] : " "; // sentinel
    const isWordChar =
      (c >= "a" && c <= "z") ||
      (c >= "A" && c <= "Z") ||
      (c >= "0" && c <= "9") ||
      c === "-" ||
      c === "'";

    if (isWordChar) {
      if (start === null) start = i;
    } else if (start !== null) {
      const word = text.slice(start, i);
      const trimmed = word.replace(/^[-']+|[-']+$/g, "");
      if (trimmed.length > 0) {
        const trimOffset = word.indexOf(trimmed);
        tokens.push({
          text: trimmed,
          byteStart: start + trimOffset,
          byteEnd: start + trimOffset + trimmed.length,
        });
      }
      start = null;
    }
  }

  return tokens;
}

// ── Overlap check ────────────────────────────────────────────────────

function overlapsAny(matches: PersuasionMatch[], start: number, end: number): boolean {
  return matches.some((m) => start < m.end && end > m.start);
}

// ── Lookup ────────────────────────────────────────────────────────────

function lookupPhrase(phrase: string): PersuasionCategory | null {
  for (const [category, set] of CATEGORY_SETS) {
    if (set.has(phrase)) return category;
  }
  return null;
}

// ── Dictionary Scanner ───────────────────────────────────────────────

/**
 * Scan text for persuasion phrases using 3->2->1 word window scanning.
 * Longest match wins; overlapping shorter matches are suppressed.
 */
export function scanDictionary(text: string): PersuasionMatch[] {
  const matches: PersuasionMatch[] = [];
  const lower = text.toLowerCase();
  const tokens = tokenize(lower);

  // 3-word phrases (longest match priority)
  for (let i = 0; i + 2 < tokens.length; i++) {
    const phrase = `${tokens[i].text} ${tokens[i + 1].text} ${tokens[i + 2].text}`;
    const start = tokens[i].byteStart;
    const end = tokens[i + 2].byteEnd;
    const category = lookupPhrase(phrase);
    if (category) {
      matches.push({ category, start, end, phrase: text.slice(start, end) });
    }
  }

  // 2-word phrases
  for (let i = 0; i + 1 < tokens.length; i++) {
    const phrase = `${tokens[i].text} ${tokens[i + 1].text}`;
    const start = tokens[i].byteStart;
    const end = tokens[i + 1].byteEnd;
    const category = lookupPhrase(phrase);
    if (category && !overlapsAny(matches, start, end)) {
      matches.push({ category, start, end, phrase: text.slice(start, end) });
    }
  }

  // 1-word phrases
  for (const token of tokens) {
    const category = lookupPhrase(token.text);
    if (category && !overlapsAny(matches, token.byteStart, token.byteEnd)) {
      matches.push({
        category,
        start: token.byteStart,
        end: token.byteEnd,
        phrase: text.slice(token.byteStart, token.byteEnd),
      });
    }
  }

  matches.sort((a, b) => a.start - b.start);
  return matches;
}

// ── Severity Computation ─────────────────────────────────────────────

/**
 * Compute severity tier from R1 dictionary results.
 * Mirrors compute_severity() in response_integrity.rs (R1-only path, no R2).
 */
export function computeSeverity(
  matches: PersuasionMatch[],
  categories: PersuasionCategory[],
): SeverityTier {
  const numCategories = categories.length;
  const numFlags = matches.length;

  // 4+ categories -> Caution
  if (numCategories >= 4) return "Caution";

  // Commercial + (Fear or Urgency) -> Caution
  const hasCom = categories.includes("Commercial");
  const hasFear = categories.includes("Fear");
  const hasUrg = categories.includes("Urgency");
  if (hasCom && (hasFear || hasUrg)) return "Caution";

  // 2+ categories or 3+ flags -> Warning
  if (numCategories >= 2 || numFlags >= 3) return "Warning";

  // Default: Notice
  return "Notice";
}

// ── Warning Label Formatting ─────────────────────────────────────────

const CATEGORY_DISPLAY: Record<PersuasionCategory, string> = {
  Urgency: "Urgency",
  Scarcity: "Scarcity",
  SocialProof: "Social Proof",
  Fear: "Fear",
  Authority: "Authority",
  Commercial: "Commercial",
  Flattery: "Flattery",
};

/**
 * Format a user-facing warning label matching L0 format_warning_label().
 */
export function formatWarningLabel(severity: SeverityTier, categories: PersuasionCategory[]): string {
  const tactics = [...new Set(categories.map((c) => CATEGORY_DISPLAY[c]))]
    .sort()
    .join(" \u2022 ");

  switch (severity) {
    case "Notice":
      return (
        `--- OpenObscure WARNING ---\n` +
        `Detected: ${tactics}\n` +
        `---\n\n`
      );
    case "Warning":
      return (
        `--- OpenObscure WARNING ---\n` +
        `Detected: ${tactics}\n` +
        `This response contains language patterns associated with influence tactics.\n` +
        `---\n\n`
      );
    case "Caution":
      return (
        `--- OpenObscure WARNING ---\n` +
        `Detected: ${tactics}\n` +
        `Recommendation: Pause and verify with objective evidence before acting.\n` +
        `---\n\n`
      );
  }
}

// ── Public API ────────────────────────────────────────────────────────

/**
 * Scan text for persuasion/manipulation patterns.
 *
 * Uses pure JS dictionary scanner (future: NAPI bridge to Rust R1+R2 cascade).
 * Returns null if no persuasion detected.
 */
export function scanPersuasion(text: string): PersuasionScanResult | null {
  const start = performance.now();
  const matches = scanDictionary(text);
  const scanTimeUs = Math.round((performance.now() - start) * 1000);

  if (matches.length === 0) return null;

  const categorySet = new Set(matches.map((m) => m.category));
  const categories = [...categorySet] as PersuasionCategory[];
  const severity = computeSeverity(matches, categories);

  return {
    matches,
    categories,
    severity,
    scanTimeUs,
    source: "js",
  };
}

/**
 * Total number of phrases across all 7 categories.
 */
export function totalPhraseCount(): number {
  return CATEGORY_SETS.reduce((sum, [, set]) => sum + set.size, 0);
}

/**
 * Get the number of phrases in a specific category.
 */
export function getCategorySize(category: PersuasionCategory): number {
  const entry = CATEGORY_SETS.find(([cat]) => cat === category);
  return entry ? entry[1].size : 0;
}
