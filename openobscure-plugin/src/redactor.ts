/**
 * PII Redactor — regex-based PII detection and redaction, with optional
 * NER-enhanced scanning via L0 proxy endpoint.
 *
 * Complementary to L0 proxy FPE: catches PII in tool results that
 * bypass the proxy (local tools, file reads, etc.). Replaces with
 * [REDACTED-TYPE] placeholders since FPE is not available in-process.
 *
 * When L0 is healthy, `redactPiiWithNer()` calls the L0 /ner endpoint
 * to detect semantic PII (person names, locations, organizations) that
 * regex alone cannot catch.
 */

import { execFileSync } from "child_process";

export interface RedactionResult {
  /** Redacted text. */
  text: string;
  /** Number of PII matches found and redacted. */
  count: number;
  /** Per-type counts. */
  types: Record<string, number>;
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

/** Redact all detected PII in the given text. */
export function redactPii(text: string): RedactionResult {
  let result = text;
  let totalCount = 0;
  const types: Record<string, number> = {};

  for (const pattern of PII_PATTERNS) {
    // Reset regex lastIndex for global patterns
    pattern.regex.lastIndex = 0;

    result = result.replace(pattern.regex, (match) => {
      if (pattern.validate && !pattern.validate(match)) {
        return match; // Failed validation, don't redact
      }
      totalCount++;
      types[pattern.type] = (types[pattern.type] || 0) + 1;
      return pattern.replacement;
    });
  }

  return { text: result, count: totalCount, types };
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
 * Enhanced PII redaction: regex + NER semantic scanning via L0 endpoint.
 *
 * 1. Calls L0 /ner to get semantic matches on original text
 * 2. Applies NER redactions from end to start (preserves offsets)
 * 3. Runs regex redaction on the result (catches structured PII)
 * 4. Merges match counts
 */
export function redactPiiWithNer(
  text: string,
  proxyUrl: string,
  authToken?: string,
): RedactionResult {
  // Step 1: Get NER matches from L0 on original text
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

  // Step 3: Run regex redaction on (possibly NER-redacted) text
  const regexResult = redactPii(result);

  // Step 4: Merge counts
  const totalCount = nerCount + regexResult.count;
  const mergedTypes = { ...nerTypes };
  for (const [type, count] of Object.entries(regexResult.types)) {
    mergedTypes[type] = (mergedTypes[type] || 0) + count;
  }

  return { text: regexResult.text, count: totalCount, types: mergedTypes };
}
