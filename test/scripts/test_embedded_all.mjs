#!/usr/bin/env node
// test_embedded_all.mjs — Test ALL text-based input categories via Embedded plugin.
//
// Produces dual output per file:
//   test/data/output/<category>/json/<filename>_embedded.json
//   test/data/output/<category>/redacted/<filename>
//
// Usage:
//   node test/scripts/test_embedded_all.mjs
//
// Environment:
//   USE_NER=1   — Enable NER bridge mode (requires proxy running)
//   PROXY_URL   — Proxy URL for NER bridge (default: http://127.0.0.1:18790)

import { readdirSync, statSync, readFileSync, writeFileSync, mkdirSync, rmSync, existsSync } from "fs";
import { join, dirname, basename, extname } from "path";
import { fileURLToPath } from "url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const TEST_DIR = dirname(__dirname);
const INPUT_DIR = join(TEST_DIR, "data", "input");
const OUTPUT_DIR = join(TEST_DIR, "data", "output");

// Load plugin
const PLUGIN_PATH = join(TEST_DIR, "..", "openobscure-plugin", "dist", "core.js");
let redactPii, redactPiiWithNer;

try {
  const mod = await import(PLUGIN_PATH);
  redactPii = mod.redactPii;
  redactPiiWithNer = mod.redactPiiWithNer;
} catch (err) {
  console.error(`Error: Cannot load plugin from ${PLUGIN_PATH}`);
  console.error("Build the plugin first: cd openobscure-plugin && npm run build");
  process.exit(1);
}

const useNer = process.env.USE_NER === "1";
const proxyUrl = process.env.PROXY_URL || "http://127.0.0.1:18790";
const mode = useNer ? "embedded+ner" : "embedded";

console.log("============================================");
console.log("  OpenObscure Embedded Test Suite");
console.log(`  Mode: ${mode}`);
if (useNer) console.log(`  Proxy: ${proxyUrl}`);
console.log("  Output: test/data/output/<cat>/json/ + redacted/");
console.log("============================================");
console.log("");

const TEXT_EXTENSIONS = new Set([".txt", ".csv", ".tsv", ".env", ".py", ".yaml", ".yml", ".json", ".sh", ".md", ".log"]);

const CATEGORIES = [
  "PII_Detection",
  "Multilingual_PII",
  "Code_Config_PII",
  "Structured_Data_PII",
  "Agent_Tool_Results",
];

let grandTotalFiles = 0;
let grandTotalMatches = 0;
let grandPass = 0;
let grandFail = 0;

// Purge previous embedded results
console.log("Purging previous embedded results...");
for (const cat of CATEGORIES) {
  const jsonDir = join(OUTPUT_DIR, cat, "json");
  const redactedDir = join(OUTPUT_DIR, cat, "redacted");
  if (existsSync(jsonDir)) {
    for (const f of readdirSync(jsonDir).filter((f) => f.endsWith("_embedded.json"))) {
      rmSync(join(jsonDir, f), { force: true });
    }
  }
  if (existsSync(redactedDir)) {
    for (const f of readdirSync(redactedDir)) {
      rmSync(join(redactedDir, f), { force: true });
    }
  }
}
console.log("");

const startTime = Date.now();

for (const category of CATEGORIES) {
  const catInput = join(INPUT_DIR, category);
  const catOutput = join(OUTPUT_DIR, category);

  try {
    statSync(catInput);
  } catch {
    console.log(`SKIP ${category} (directory not found)`);
    continue;
  }

  const jsonDir = join(catOutput, "json");
  const redactedDir = join(catOutput, "redacted");
  mkdirSync(jsonDir, { recursive: true });
  mkdirSync(redactedDir, { recursive: true });

  console.log(`--- ${category} ---`);

  const files = readdirSync(catInput).filter((f) => {
    const ext = extname(f).toLowerCase();
    return statSync(join(catInput, f)).isFile() && TEXT_EXTENSIONS.has(ext);
  });

  let catMatches = 0;
  let catPass = 0;
  let catFail = 0;

  for (const file of files) {
    const inputPath = join(catInput, file);
    const nameNoExt = basename(file, extname(file));

    try {
      const text = readFileSync(inputPath, "utf-8");
      const t0 = Date.now();

      let result;
      if (useNer && redactPiiWithNer) {
        try {
          result = redactPiiWithNer(text, proxyUrl);
        } catch {
          result = redactPii(text);
        }
      } else {
        result = redactPii(text);
      }

      const elapsedMs = Date.now() - t0;

      // Write JSON metadata
      const envelope = {
        file,
        path: inputPath,
        architecture: mode,
        timestamp: new Date().toISOString(),
        elapsed_ms: elapsedMs,
        total_matches: result.count,
        type_summary: result.types,
      };
      writeFileSync(join(jsonDir, `${nameNoExt}_embedded.json`), JSON.stringify(envelope, null, 2) + "\n");

      // Write redacted file (preserving original filename)
      writeFileSync(join(redactedDir, file), result.text);

      console.log(`  OK  ${file} — ${result.count} matches (${elapsedMs}ms)`);

      catMatches += result.count;
      catPass++;
    } catch (err) {
      console.log(`  FAIL ${file} — ${err.message}`);
      catFail++;
    }

    grandTotalFiles++;
  }

  console.log(`  Summary: ${catPass} passed, ${catFail} failed, ${catMatches} matches`);
  console.log("");

  grandTotalMatches += catMatches;
  grandPass += catPass;
  grandFail += catFail;
}

const elapsedSec = ((Date.now() - startTime) / 1000).toFixed(1);

console.log("============================================");
console.log(`  Total files:   ${grandTotalFiles}`);
console.log(`  Passed:        ${grandPass}`);
console.log(`  Failed:        ${grandFail}`);
console.log(`  Total matches: ${grandTotalMatches}`);
console.log(`  Elapsed:       ${elapsedSec}s`);
console.log(`  JSON metadata:  test/data/output/*/json/`);
console.log(`  Redacted files: test/data/output/*/redacted/`);
console.log("============================================");

process.exit(grandFail > 0 ? 1 : 0);
