#!/usr/bin/env node
// test_napi_smoke.mjs — Smoke test for the OpenObscure NAPI native scanner.
//
// Verifies:
//   1) The native addon loads and scans correctly (direct API)
//   2) redactPii() from the plugin auto-upgrades to use the native scanner
//   3) Performance benchmarks
//
// Usage:
//   node test/scripts/test_napi_smoke.mjs

import { createRequire } from "module";
import { existsSync } from "fs";
import { resolve, dirname, join } from "path";
import { fileURLToPath } from "url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(__dirname, "../..");
const require = createRequire(import.meta.url);

let passed = 0;
let failed = 0;

function assert(condition, message) {
  if (condition) {
    console.log(`  ✓ ${message}`);
    passed++;
  } else {
    console.log(`  ✗ ${message}`);
    failed++;
  }
}

// ── Test 1: Direct addon API ─────────────────────────────────────────
console.log("▶ Direct addon API");

let OpenObscureScanner;
try {
  const mod = require(resolve(ROOT, "openobscure-napi/index.js"));
  OpenObscureScanner = mod.OpenObscureScanner;
} catch (err) {
  console.error("Error: Cannot load native addon.");
  console.error("Build first: ./build/build_napi.sh");
  console.error(err.message);
  process.exit(1);
}

const scanner = new OpenObscureScanner();

// SSN
const r1 = scanner.scanText("My SSN is 123-45-6789");
assert(r1.matches.length === 1, `SSN detected (${r1.matches.length} match)`);
assert(r1.matches[0]?.piiType === "ssn", `Type is "ssn"`);
assert(r1.matches[0]?.rawValue === "123-45-6789", "Raw value correct");

// Email + Phone
const r2 = scanner.scanText("Email: john@example.com, Phone: 555-867-5309");
assert(r2.matches.length === 2, `Email+Phone detected (${r2.matches.length} matches)`);

// Credit card (Luhn-valid)
const r3 = scanner.scanText("Card: 4532015112830366");
assert(r3.matches.length === 1, `Credit card detected (${r3.matches.length} match)`);

// IPv4 + MAC (types JS regex doesn't catch)
const r4 = scanner.scanText("Server 192.168.1.100, MAC AA:BB:CC:DD:EE:FF");
assert(r4.matches.length === 2, `IPv4+MAC detected (${r4.matches.length} matches)`);

// Clean text
const r5 = scanner.scanText("The quick brown fox jumps over the lazy dog.");
assert(r5.matches.length === 0, `No false positives on clean text`);

// ── Test 2: Plugin auto-upgrade ──────────────────────────────────────
console.log("\n▶ Plugin redactPii() auto-upgrade");

const PLUGIN_PATH = join(ROOT, "openobscure-plugin", "dist", "core.js");
let redactPii;
try {
  const mod = await import(PLUGIN_PATH);
  redactPii = mod.redactPii;
} catch (err) {
  console.error("Error: Cannot load plugin. Build first: cd openobscure-plugin && npm run build");
  console.error(err.message);
  process.exit(1);
}

// redactPii() should detect types that JS regex alone cannot (IPv4, MAC, GPS)
const p1 = redactPii("Server 192.168.1.100, MAC AA:BB:CC:DD:EE:FF");
assert(p1.count === 2, `redactPii() detects IPv4+MAC (${p1.count} matches — native upgrade active)`);
assert(p1.types["ipv4_address"] === 1, "IPv4 type in redactPii() result");
assert(p1.types["mac_address"] === 1, "MAC type in redactPii() result");

// Structured types still work
const p2 = redactPii("SSN 123-45-6789, email john@example.com, card 4532015112830366");
assert(p2.count >= 3, `Structured types detected (${p2.count} matches)`);
assert(p2.text.includes("[REDACTED-SSN]"), "SSN redacted");
assert(p2.text.includes("[REDACTED-EMAIL]"), "Email redacted");
assert(p2.text.includes("[REDACTED-CC]"), "CC redacted");

// ── Test 3: NER auto-detection ───────────────────────────────────────
const nerDir = resolve(ROOT, "openobscure-core/models/ner");
if (existsSync(resolve(nerDir, "model_int8.onnx")) || existsSync(resolve(nerDir, "model.onnx"))) {
  console.log("\n▶ NER auto-detection");

  // Health keywords should be detected via native scanner with auto-detected NER
  const p3 = redactPii("Patient diagnosed with diabetes");
  const hasHealth = p3.types["health_keyword"] >= 1;
  assert(hasHealth, `Health keywords detected via redactPii() (${p3.count} matches)`);
} else {
  console.log("\n▶ NER auto-detection (SKIPPED — no models)");
}

// ── Test 4: Performance ──────────────────────────────────────────────
console.log("\n▶ Performance");

const perfText =
  "John Smith (SSN 123-45-6789) email john@example.com, card 4532015112830366, phone 555-867-5309";

// Warmup
for (let i = 0; i < 100; i++) redactPii(perfText);

const iterations = 1000;
const start = process.hrtime.bigint();
for (let i = 0; i < iterations; i++) {
  redactPii(perfText);
}
const elapsed = Number(process.hrtime.bigint() - start) / 1e6;
const avgMs = elapsed / iterations;

console.log(`  ${iterations} scans in ${elapsed.toFixed(1)}ms (avg ${avgMs.toFixed(3)}ms/scan)`);
// With NER model loaded, ~28ms per scan is expected (vs 76ms curl bridge = 2.7x faster)
// Without NER (regex-only), <1ms per scan
assert(avgMs < 50, `Average latency < 50ms (${avgMs.toFixed(3)}ms)`);

// ── Summary ──────────────────────────────────────────────────────────
console.log(`\n${"═".repeat(50)}`);
console.log(`Results: ${passed} passed, ${failed} failed`);
process.exit(failed > 0 ? 1 : 0);
