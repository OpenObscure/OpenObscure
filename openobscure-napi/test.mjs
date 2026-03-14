/**
 * NAPI addon integration tests — run with `node --test test.mjs`
 *
 * Requires a compiled scanner.node in the same directory.
 * Built by: `npm run build` or `./build/build_napi.sh`
 */

import { test } from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const __dirname = dirname(fileURLToPath(import.meta.url));
const require = createRequire(import.meta.url);

// Load the addon — prefer locally built scanner.node, fall back to index.js
// loader (which handles platform-specific package resolution).
let OpenObscureScanner, scanPersuasion;
try {
  const localNode = join(__dirname, "scanner.node");
  if (existsSync(localNode)) {
    ({ OpenObscureScanner, scanPersuasion } = require(localNode));
  } else {
    ({ OpenObscureScanner, scanPersuasion } = require("./index.js"));
  }
} catch (e) {
  console.error("Failed to load NAPI addon:", e.message);
  process.exit(1);
}

// ── OpenObscureScanner ──────────────────────────────────────────────────────

test("OpenObscureScanner: instantiates without model dir", () => {
  const scanner = new OpenObscureScanner();
  assert.ok(scanner, "scanner should be truthy");
  assert.equal(scanner.hasNer(), false, "hasNer should be false without model");
});

test("OpenObscureScanner: scanText returns ScanResult shape", () => {
  const scanner = new OpenObscureScanner();
  const result = scanner.scanText("hello world");
  assert.ok(typeof result === "object", "result should be object");
  assert.ok(Array.isArray(result.matches), "matches should be array");
  assert.ok(typeof result.timingUs === "number", "timingUs should be number");
});

test("OpenObscureScanner: detects email address", () => {
  const scanner = new OpenObscureScanner();
  const result = scanner.scanText("Contact us at alice@example.com for details.");
  const emails = result.matches.filter((m) => m.piiType === "email");
  assert.equal(emails.length, 1, "should detect exactly one email");
  assert.equal(emails[0].rawValue, "alice@example.com");
});

test("OpenObscureScanner: detects SSN", () => {
  const scanner = new OpenObscureScanner();
  const result = scanner.scanText("SSN: 123-45-6789");
  const ssns = result.matches.filter((m) => m.piiType === "ssn");
  assert.equal(ssns.length, 1, "should detect one SSN");
});

test("OpenObscureScanner: clean text produces no matches", () => {
  const scanner = new OpenObscureScanner();
  const result = scanner.scanText("The quick brown fox jumps over the lazy dog.");
  assert.equal(result.matches.length, 0, "should produce no matches for clean text");
});

test("OpenObscureScanner: match offsets are within string bounds", () => {
  const scanner = new OpenObscureScanner();
  const text = "Email: bob@test.org and phone +1-555-123-4567";
  const result = scanner.scanText(text);
  for (const m of result.matches) {
    assert.ok(m.start >= 0, "start >= 0");
    assert.ok(m.end <= text.length, `end (${m.end}) <= text.length (${text.length})`);
    assert.ok(m.start < m.end, "start < end");
    assert.equal(
      text.slice(m.start, m.end),
      m.rawValue,
      `rawValue should match text slice for ${m.piiType}`
    );
  }
});

test("OpenObscureScanner: confidence is in [0, 1]", () => {
  const scanner = new OpenObscureScanner();
  const result = scanner.scanText("My email is test@example.com");
  for (const m of result.matches) {
    assert.ok(m.confidence >= 0 && m.confidence <= 1, `confidence ${m.confidence} not in [0,1]`);
  }
});

// ── scanPersuasion ──────────────────────────────────────────────────────────

test("scanPersuasion: returns PersuasionScanResult shape", () => {
  const result = scanPersuasion("hello");
  assert.ok(typeof result === "object");
  assert.ok(Array.isArray(result.matches));
  assert.ok(typeof result.timingUs === "number");
});

test("scanPersuasion: detects urgency phrase", () => {
  const result = scanPersuasion("Act now before it's too late!");
  const urgency = result.matches.filter((m) => m.category === "Urgency");
  assert.ok(urgency.length > 0, "should detect urgency");
});

test("scanPersuasion: clean text has no matches", () => {
  const result = scanPersuasion("The function returns a sorted list of integers.");
  assert.equal(result.matches.length, 0, "clean text should produce no matches");
});
