#!/usr/bin/env node
// test_l1_plugin_all.mjs — Test ALL text-based input categories via L1 TypeScript Plugin.
//
// Produces dual output per file:
//   test/data/output/<category>/json/<filename>_l1_plugin.json
//   test/data/output/<category>/redacted/<filename>_l1_plugin.<ext>
//
// Usage:
//   node test/scripts/test_l1_plugin_all.mjs
//
// Environment:
//   USE_NER=1   — Enable NER bridge mode (requires proxy running)
//   PROXY_URL   — Proxy URL for NER bridge (default: http://127.0.0.1:18790)
//   AUTH_TOKEN   — Proxy auth token (default: read from ~/.openobscure/.auth-token)
//
// Note: If @openobscure/scanner-napi is installed, redactPii() automatically
// uses the native Rust HybridScanner (14 PII types) instead of JS regex (5 types).

import { readdirSync, statSync, readFileSync, writeFileSync, mkdirSync, rmSync, existsSync } from "fs";
import { join, dirname, basename, extname } from "path";
import { homedir } from "os";
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

// Read auth token (required for NER endpoint)
let authToken = process.env.AUTH_TOKEN || "";
if (!authToken) {
  const tokenFile = join(homedir(), ".openobscure", ".auth-token");
  try {
    authToken = readFileSync(tokenFile, "utf-8").trim();
  } catch {
    // No token file — NER calls will fail with 401 if proxy requires auth
  }
}

const mode = useNer ? "l1_plugin+ner" : "l1_plugin";

console.log("============================================");
console.log("  OpenObscure L1 Plugin Test Suite");
console.log(`  Mode: ${mode}`);
if (useNer) {
  console.log(`  Proxy: ${proxyUrl}`);
  console.log(`  Auth:  ${authToken ? "token loaded" : "NO TOKEN (NER will fail with 401)"}`);
}
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
  // Cognitive_Firewall is tested via test_cognitive_firewall.sh (proxy RI pipeline, not PII scanning)
];

let grandTotalFiles = 0;
let grandTotalMatches = 0;
let grandPass = 0;
let grandFail = 0;
const grandResults = [];

// Purge previous l1_plugin results
console.log("Purging previous l1_plugin results...");
for (const cat of CATEGORIES) {
  const jsonDir = join(OUTPUT_DIR, cat, "json");
  const redactedDir = join(OUTPUT_DIR, cat, "redacted");
  if (existsSync(jsonDir)) {
    for (const f of readdirSync(jsonDir).filter((f) => f.endsWith("_l1_plugin.json"))) {
      rmSync(join(jsonDir, f), { force: true });
    }
  }
  if (existsSync(redactedDir)) {
    for (const f of readdirSync(redactedDir).filter((f) => f.includes("_l1_plugin."))) {
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
    const ext = extname(file);
    const nameNoExt = basename(file, ext);

    try {
      const text = readFileSync(inputPath, "utf-8");
      const t0 = Date.now();

      let result;
      if (useNer && redactPiiWithNer) {
        try {
          result = redactPiiWithNer(text, proxyUrl, authToken);
        } catch {
          result = redactPii(text);
        }
      } else {
        result = redactPii(text);
      }

      const elapsedMs = Date.now() - t0;

      // Write JSON metadata (aligned with gateway format)
      const envelope = {
        file,
        path: inputPath,
        architecture: mode,
        redaction_mode: "label",
        timestamp: new Date().toISOString(),
        total_matches: result.count,
        type_summary: result.types,
        timing: {
          total_ms: elapsedMs,
        },
        matches: result.matches,
      };
      writeFileSync(join(jsonDir, `${nameNoExt}_l1_plugin.json`), JSON.stringify(envelope, null, 2) + "\n");

      // Write redacted file with _l1_plugin suffix
      writeFileSync(join(redactedDir, `${nameNoExt}_l1_plugin${ext}`), result.text);

      console.log(`  OK  ${file} — ${result.count} matches (${elapsedMs}ms)`);

      catMatches += result.count;
      catPass++;
      grandResults.push({ name: file, status: "pass", detail: `${result.count} matches` });
    } catch (err) {
      console.log(`  FAIL ${file} — ${err.message}`);
      catFail++;
      grandResults.push({ name: file, status: "fail", detail: err.message });
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

// Write validation JSON
const validationJson = {
  test_suite: "l1_plugin_all",
  timestamp: new Date().toISOString().replace(/\.\d{3}Z$/, "Z"),
  total: grandTotalFiles,
  pass: grandPass,
  fail: grandFail,
  warn: 0,
  skip: 0,
  results: grandResults,
};
writeFileSync(join(OUTPUT_DIR, "l1_plugin_all_validation.json"), JSON.stringify(validationJson, null, 2) + "\n");

process.exit(grandFail > 0 ? 1 : 0);
