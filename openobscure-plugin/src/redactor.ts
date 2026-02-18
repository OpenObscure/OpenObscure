/**
 * PII Redactor — regex-based PII detection and redaction.
 *
 * Complementary to L0 proxy FPE: catches PII in tool results that
 * bypass the proxy (local tools, file reads, etc.). Replaces with
 * [REDACTED-TYPE] placeholders since FPE is not available in-process.
 */

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
