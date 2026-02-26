/**
 * PII Redactor — in-process PII detection and redaction for the L1 plugin.
 *
 * Automatically uses the best available engine:
 * - **Native addon** (`@openobscure/scanner-napi`): Rust HybridScanner with
 *   regex + keywords + NER TinyBERT — detects 14 PII types (auto-detected)
 * - **JS regex** fallback: 5 structured types (CC, SSN, phone, email, API key)
 *
 * `redactPiiWithNer()` provides an alternative path via the L0 proxy's /ner
 * HTTP endpoint (useful when proxy is running but addon is not installed).
 */

import { execFileSync } from "child_process";
import { existsSync } from "fs";
import { dirname, resolve } from "path";

// ── Native Scanner (optional dependency) ─────────────────────────────
// Auto-loaded at module init. If available, redactPii() uses the native
// Rust HybridScanner for 14-type detection. Otherwise, JS regex (5 types).

interface NativeScanMatch {
  start: number;
  end: number;
  piiType: string;
  confidence: number;
  rawValue: string;
}

interface NativeScanResult {
  matches: NativeScanMatch[];
  timingUs: number;
}

interface NativeScannerClass {
  new (nerModelDir?: string | null): {
    scanText(text: string): NativeScanResult;
    hasNer(): boolean;
  };
}

let NativeScanner: NativeScannerClass | null = null;
try {
  // eslint-disable-next-line @typescript-eslint/no-require-imports
  const mod = require("@openobscure/scanner-napi");
  NativeScanner = mod.OpenObscureScanner;
} catch {
  // Not installed — redactPii() falls back to JS regex
}

let _nativeInstance: InstanceType<NativeScannerClass> | null = null;

/**
 * Auto-detect NER model directory relative to the addon's location.
 * Looks for models at `../openobscure-proxy/models/ner` from the addon dir.
 * Returns the path if model files exist, undefined otherwise.
 */
function autoDetectNerModelDir(): string | undefined {
  try {
    const addonDir = dirname(
      require.resolve("@openobscure/scanner-napi/package.json"),
    );
    const candidate = resolve(addonDir, "..", "openobscure-proxy", "models", "ner");
    if (
      existsSync(resolve(candidate, "model_int8.onnx")) ||
      existsSync(resolve(candidate, "model.onnx"))
    ) {
      return candidate;
    }
  } catch {
    // addon not installed
  }
  return undefined;
}

/**
 * Run native scanner on text and return RedactionResult.
 * Caller must ensure NativeScanner is not null before calling.
 */
function redactViaNative(text: string): RedactionResult {
  if (!_nativeInstance) {
    const modelDir = autoDetectNerModelDir() ?? null;
    _nativeInstance = new NativeScanner!(modelDir);
  }

  const result = _nativeInstance.scanText(text);

  let redacted = text;
  const types: Record<string, number> = {};
  const matches: RedactionMatch[] = [];

  // Collect matches (ascending order for output)
  for (const m of result.matches) {
    if (m.start >= text.length || m.end > text.length) continue;
    matches.push({
      start: m.start,
      end: m.end,
      type: m.piiType,
      confidence: m.confidence,
    });
    types[m.piiType] = (types[m.piiType] || 0) + 1;
  }
  matches.sort((a, b) => a.start - b.start);

  // Apply redactions from end to start to preserve offsets
  const descending = [...matches].sort((a, b) => b.start - a.start);
  for (const m of descending) {
    const label = NER_TYPE_LABELS[m.type] ?? "[REDACTED]";
    redacted = redacted.slice(0, m.start) + label + redacted.slice(m.end);
  }

  return { text: redacted, count: matches.length, types, matches };
}

/** A single PII match with position in the original (pre-redaction) text. */
export interface RedactionMatch {
  start: number;
  end: number;
  type: string;
  confidence: number;
}

export interface RedactionResult {
  /** Redacted text. */
  text: string;
  /** Number of PII matches found and redacted. */
  count: number;
  /** Per-type counts. */
  types: Record<string, number>;
  /** Per-match details with positions in the original text. */
  matches: RedactionMatch[];
}

interface PiiPattern {
  type: string;
  regex: RegExp;
  replacement: string;
  validate?: (match: string) => boolean;
}

const PII_PATTERNS: PiiPattern[] = [
  {
    type: "credit_card",
    regex:
      /\b(?:4[0-9]{3}|5[1-5][0-9]{2}|3[47][0-9]{2}|6(?:011|5[0-9]{2}))[- ]?[0-9]{4}[- ]?[0-9]{4}[- ]?[0-9]{1,7}\b/g,
    replacement: "[REDACTED-CC]",
    validate: luhnCheck,
  },
  {
    type: "ssn",
    regex: /\b[0-9]{3}[- ][0-9]{2}[- ][0-9]{4}\b/g,
    replacement: "[REDACTED-SSN]",
    validate: validateSsn,
  },
  {
    type: "phone",
    regex:
      /(?:\+[0-9]{1,3}[-.\s]?\(?[0-9]{3}\)?[-.\s]?[0-9]{3}[-.\s]?[0-9]{4}|\(?[0-9]{3}\)[-.\s]?[0-9]{3}[-.\s]?[0-9]{4}|\b[0-9]{3}[-.\s][0-9]{3}[-.\s]?[0-9]{4}\b)/g,
    replacement: "[REDACTED-PHONE]",
  },
  {
    type: "email",
    regex: /\b[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}\b/g,
    replacement: "[REDACTED-EMAIL]",
  },
  {
    type: "api_key",
    regex:
      /\b(?:sk-ant-[a-zA-Z0-9_-]{20,}|sk-[a-zA-Z0-9]{20,}|AKIA[0-9A-Z]{16}|ghp_[a-zA-Z0-9]{36,}|gho_[a-zA-Z0-9]{36,}|xoxb-[0-9]+-[a-zA-Z0-9]+|xoxp-[0-9]+-[a-zA-Z0-9]+)\b/g,
    replacement: "[REDACTED-KEY]",
  },
];

/**
 * Redact all detected PII in the given text.
 *
 * Automatically uses the best available engine:
 * - If `@openobscure/scanner-napi` is installed: Rust HybridScanner
 *   (regex + keywords + NER) detecting up to 14 PII types
 * - Otherwise: JS regex detecting 5 structured types
 */
export function redactPii(text: string): RedactionResult {
  // Auto-upgrade to native scanner when available
  if (NativeScanner) {
    return redactViaNative(text);
  }

  return redactPiiJs(text);
}

/** JS regex-only PII redaction (5 structured types). */
function redactPiiJs(text: string): RedactionResult {
  // Pass 1: Collect all valid matches with original-text positions
  const allMatches: RedactionMatch[] = [];

  for (const pattern of PII_PATTERNS) {
    pattern.regex.lastIndex = 0;
    let m;
    while ((m = pattern.regex.exec(text)) !== null) {
      if (pattern.validate && !pattern.validate(m[0])) continue;
      allMatches.push({
        start: m.index,
        end: m.index + m[0].length,
        type: pattern.type,
        confidence: 1.0,
      });
    }
  }

  // Sort ascending by start, then resolve overlaps (first match wins)
  allMatches.sort((a, b) => a.start - b.start);
  const matches: RedactionMatch[] = [];
  for (const m of allMatches) {
    if (matches.length > 0 && m.start < matches[matches.length - 1].end) {
      continue; // Skip overlapping match
    }
    matches.push(m);
  }

  // Pass 2: Apply replacements from end to start to preserve earlier offsets
  let result = text;
  const types: Record<string, number> = {};
  for (const m of [...matches].reverse()) {
    const pat = PII_PATTERNS.find((p) => p.type === m.type)!;
    result = result.slice(0, m.start) + pat.replacement + result.slice(m.end);
    types[m.type] = (types[m.type] || 0) + 1;
  }

  return { text: result, count: matches.length, types, matches };
}

/** Luhn algorithm check for credit card numbers. */
function luhnCheck(raw: string): boolean {
  const digits = raw.replace(/[^0-9]/g, "");
  if (digits.length < 13 || digits.length > 19) return false;

  let sum = 0;
  let double = false;
  for (let i = digits.length - 1; i >= 0; i--) {
    let val = parseInt(digits[i], 10);
    if (double) {
      val *= 2;
      if (val > 9) val -= 9;
    }
    sum += val;
    double = !double;
  }
  return sum % 10 === 0;
}

/** Validate SSN ranges (reject 000, 666, 900+ area numbers). */
function validateSsn(raw: string): boolean {
  const digits = raw.replace(/[^0-9]/g, "");
  if (digits.length !== 9) return false;
  const area = parseInt(digits.substring(0, 3), 10);
  const group = parseInt(digits.substring(3, 5), 10);
  const serial = parseInt(digits.substring(5, 9), 10);
  if (area === 0 || area === 666 || area >= 900) return false;
  if (group === 0 || serial === 0) return false;
  return true;
}

// ── NER-Enhanced Redaction ─────────────────────────────────────────────

/** A PII span returned by the L0 /ner endpoint. */
export interface NerMatch {
  start: number;
  end: number;
  type: string;
  confidence: number;
}

/** Map L0 PII type names to redaction labels. */
const NER_TYPE_LABELS: Record<string, string> = {
  person: "[REDACTED-PERSON]",
  location: "[REDACTED-LOCATION]",
  organization: "[REDACTED-ORG]",
  health_keyword: "[REDACTED-HEALTH]",
  child_keyword: "[REDACTED-CHILD]",
  ipv4_address: "[REDACTED-IP]",
  ipv6_address: "[REDACTED-IP]",
  gps_coordinate: "[REDACTED-GPS]",
  mac_address: "[REDACTED-MAC]",
  // Structured types (also handled by regex, but include for completeness)
  credit_card: "[REDACTED-CC]",
  ssn: "[REDACTED-SSN]",
  phone: "[REDACTED-PHONE]",
  email: "[REDACTED-EMAIL]",
  api_key: "[REDACTED-KEY]",
};

/**
 * Call L0 proxy /ner endpoint synchronously.
 *
 * Uses `curl` via `execFileSync` (no shell, safe from injection).
 * Returns null on any error (timeout, connection refused, etc.).
 */
export function callNerEndpoint(
  text: string,
  proxyUrl: string,
  authToken?: string,
): NerMatch[] | null {
  try {
    const args = [
      "--fail",
      "--silent",
      "--max-time",
      "2",
      "-X",
      "POST",
      "-H",
      "Content-Type: application/json",
    ];
    if (authToken) {
      args.push("-H", `X-OpenObscure-Token: ${authToken}`);
    }
    args.push("-d", JSON.stringify({ text }));
    args.push(`${proxyUrl}/_openobscure/ner`);

    const result = execFileSync("curl", args, {
      timeout: 3000,
      encoding: "utf-8",
      maxBuffer: 1024 * 1024, // 1MB
    });
    return JSON.parse(result) as NerMatch[];
  } catch {
    return null;
  }
}

/**
 * Enhanced PII redaction: regex + NER semantic scanning.
 *
 * Uses the best available engine:
 * - If native addon is installed: uses it directly (same engine, no HTTP)
 * - Otherwise: calls L0 /ner endpoint via curl, then merges with JS regex
 */
export function redactPiiWithNer(
  text: string,
  proxyUrl: string,
  authToken?: string,
): RedactionResult {
  // Native addon has the same HybridScanner — use it directly, skip HTTP
  if (NativeScanner) {
    return redactViaNative(text);
  }

  // Fallback: call L0 proxy /ner endpoint via curl
  const nerMatches = callNerEndpoint(text, proxyUrl, authToken);

  // Step 2: Apply NER redactions from end to start to preserve offsets
  let result = text;
  let nerCount = 0;
  const nerTypes: Record<string, number> = {};

  if (nerMatches && nerMatches.length > 0) {
    // Filter to only semantic types that regex doesn't already catch well
    const semanticTypes = new Set([
      "person",
      "location",
      "organization",
      "health_keyword",
      "child_keyword",
      "ipv4_address",
      "ipv6_address",
      "gps_coordinate",
      "mac_address",
    ]);

    const semanticMatches = nerMatches
      .filter((m) => semanticTypes.has(m.type))
      .sort((a, b) => b.start - a.start); // Descending start for safe replacement

    for (const match of semanticMatches) {
      if (match.start >= result.length || match.end > result.length) continue;
      const label = NER_TYPE_LABELS[match.type] ?? "[REDACTED]";
      result =
        result.slice(0, match.start) + label + result.slice(match.end);
      nerCount++;
      nerTypes[match.type] = (nerTypes[match.type] || 0) + 1;
    }
  }

  // Collect NER matches for the output
  const collectedNerMatches: RedactionMatch[] = [];
  if (nerMatches && nerMatches.length > 0) {
    const semanticTypes = new Set([
      "person", "location", "organization", "health_keyword",
      "child_keyword", "ipv4_address", "ipv6_address",
      "gps_coordinate", "mac_address",
    ]);
    for (const m of nerMatches.filter((m) => semanticTypes.has(m.type))) {
      if (m.start < text.length && m.end <= text.length) {
        collectedNerMatches.push({
          start: m.start,
          end: m.end,
          type: m.type,
          confidence: m.confidence,
        });
      }
    }
  }

  // Step 3: Run regex redaction on (possibly NER-redacted) text
  const regexResult = redactPii(result);

  // Step 4: Merge counts and matches
  const totalCount = nerCount + regexResult.count;
  const mergedTypes = { ...nerTypes };
  for (const [type, count] of Object.entries(regexResult.types)) {
    mergedTypes[type] = (mergedTypes[type] || 0) + count;
  }

  // Note: regex match offsets are in the NER-redacted text, not the original.
  // NER matches are in the original. We return both sorted by start position.
  const mergedMatches = [...collectedNerMatches, ...regexResult.matches]
    .sort((a, b) => a.start - b.start);

  return { text: regexResult.text, count: totalCount, types: mergedTypes, matches: mergedMatches };
}

