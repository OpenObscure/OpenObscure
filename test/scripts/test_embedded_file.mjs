#!/usr/bin/env node
// test_embedded_file.mjs — Test a single file against the L1 Embedded plugin (redactPii).
//
// Produces two outputs:
//   1) JSON metadata  → <output_dir>/json/<filename>_embedded.json
//   2) Redacted file  → <output_dir>/redacted/<filename>  (original extension preserved)
//
// Usage:
//   node test/scripts/test_embedded_file.mjs <input_file> [output_dir]
//
// If output_dir is omitted, results are printed to stdout.
// If output_dir is provided, both json/ and redacted/ files are written.
//
// Environment:
//   PROXY_URL  — Proxy URL for NER bridge mode (default: http://127.0.0.1:18790)
//   USE_NER    — Set to "1" to use redactPiiWithNer instead of redactPii
//
// Note: If @openobscure/scanner-napi is installed, redactPii() automatically
// uses the native Rust HybridScanner (14 PII types) instead of JS regex (5 types).

import { readFileSync, writeFileSync, mkdirSync } from "fs";
import { basename, dirname, extname, join } from "path";

// Dynamic import of the plugin
const PLUGIN_PATH = new URL("../../openobscure-plugin/dist/core.js", import.meta.url).pathname;
let redactPii, redactPiiWithNer;

try {
  const mod = await import(PLUGIN_PATH);
  redactPii = mod.redactPii;
  redactPiiWithNer = mod.redactPiiWithNer;
} catch (err) {
  console.error(`Error: Cannot load plugin from ${PLUGIN_PATH}`);
  console.error("Build the plugin first: cd openobscure-plugin && npm run build");
  console.error(err.message);
  process.exit(1);
}

const args = process.argv.slice(2);
const inputFile = args[0];
const outputDir = args[1] || null;

if (!inputFile) {
  console.log("Usage: node test_embedded_file.mjs <input_file> [output_dir]");
  console.log("");
  console.log("Examples:");
  console.log("  node test/scripts/test_embedded_file.mjs test/data/input/PII_Detection/Credit_Card_Numbers.txt");
  console.log("  node test/scripts/test_embedded_file.mjs test/data/input/PII_Detection/Credit_Card_Numbers.txt test/data/output/PII_Detection");
  process.exit(1);
}

// Read input file
let text;
try {
  text = readFileSync(inputFile, "utf-8");
} catch (err) {
  console.error(`Error: Cannot read file: ${inputFile}`);
  process.exit(1);
}

// Run detection
const useNer = process.env.USE_NER === "1";
const proxyUrl = process.env.PROXY_URL || "http://127.0.0.1:18790";

let result;
let regexMs = 0;
let nerMs = 0;
const startMs = Date.now();

if (useNer && redactPiiWithNer) {
  // Run regex first for timing baseline
  const regexStart = Date.now();
  const regexResult = redactPii(text);
  regexMs = Date.now() - regexStart;

  // Then run NER-enhanced version
  try {
    const nerStart = Date.now();
    result = redactPiiWithNer(text, proxyUrl, process.env.AUTH_TOKEN || undefined);
    nerMs = Date.now() - nerStart;
  } catch (err) {
    console.error(`Warning: NER bridge failed (${err.message}), falling back to regex-only`);
    result = regexResult;
  }
} else {
  const regexStart = Date.now();
  result = redactPii(text);
  regexMs = Date.now() - regexStart;
}

const elapsedMs = Date.now() - startMs;

// Build JSON metadata envelope (aligned with gateway format)
const filename = basename(inputFile);
const envelope = {
  file: filename,
  path: inputFile,
  architecture: useNer ? "embedded+ner" : "embedded",
  redaction_mode: "label",
  timestamp: new Date().toISOString(),
  total_matches: result.count,
  type_summary: result.types,
  timing: {
    total_ms: elapsedMs,
    regex_ms: regexMs,
    ner_ms: nerMs,
  },
  matches: result.matches,
};

// Output
if (outputDir) {
  const jsonDir = join(outputDir, "json");
  const redactedDir = join(outputDir, "redacted");
  mkdirSync(jsonDir, { recursive: true });
  mkdirSync(redactedDir, { recursive: true });

  const nameNoExt = basename(inputFile, extname(inputFile));

  // Write JSON metadata
  const jsonFile = join(jsonDir, `${nameNoExt}_embedded.json`);
  writeFileSync(jsonFile, JSON.stringify(envelope, null, 2) + "\n");

  // Write redacted file (preserving original filename and extension)
  const redactedFile = join(redactedDir, filename);
  writeFileSync(redactedFile, result.text);

  console.log(`OK  ${filename} — ${result.count} matches → json/ + redacted/`);
} else {
  console.log("=== JSON Metadata ===");
  console.log(JSON.stringify(envelope, null, 2));
  console.log("");
  console.log("=== Redacted Preview (first 500 chars) ===");
  console.log(result.text.substring(0, 500));
}
