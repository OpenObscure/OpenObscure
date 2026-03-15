#!/usr/bin/env node
// test_l1_plugin_category.mjs — Test all files in a specific input category via L1 TypeScript Plugin.
//
// Produces dual output per file:
//   <output_dir>/<category>/json/<filename>_l1_plugin.json
//   <output_dir>/<category>/redacted/<filename>_l1_plugin.<ext>
//
// Usage:
//   node test/scripts/test_l1_plugin_category.mjs <category>
//
// Categories: PII_Detection, Multilingual_PII, Code_Config_PII,
//             Structured_Data_PII, Agent_Tool_Results

import { readdirSync, statSync, readFileSync, writeFileSync, mkdirSync, rmSync, existsSync } from "fs";
import { join, dirname, basename, extname } from "path";
import { fileURLToPath } from "url";
import { execFileSync } from "child_process";

const __dirname = dirname(fileURLToPath(import.meta.url));
const TEST_DIR = dirname(__dirname);
const INPUT_DIR = join(TEST_DIR, "data", "input");
const OUTPUT_DIR = join(TEST_DIR, "data", "output");

const category = process.argv[2];

if (!category) {
  console.log("Usage: node test_l1_plugin_category.mjs <category>");
  console.log("");
  console.log("Available categories:");
  for (const dir of readdirSync(INPUT_DIR)) {
    const fullPath = join(INPUT_DIR, dir);
    if (!statSync(fullPath).isDirectory()) continue;
    if (dir === "Visual_PII" || dir === "Audio_PII") continue;
    const count = readdirSync(fullPath).filter((f) => statSync(join(fullPath, f)).isFile()).length;
    console.log(`  ${dir} (${count} files)`);
  }
  process.exit(1);
}

const catInput = join(INPUT_DIR, category);
const catOutput = join(OUTPUT_DIR, category);

try {
  statSync(catInput);
} catch {
  console.error(`Error: Category not found: ${catInput}`);
  process.exit(1);
}

const TEXT_EXTENSIONS = new Set([".txt", ".csv", ".tsv", ".env", ".py", ".yaml", ".yml", ".json", ".sh", ".md", ".log"]);

// Purge previous l1_plugin results for this category
const jsonDir = join(catOutput, "json");
const redactedDir = join(catOutput, "redacted");
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

console.log(`=== L1 Plugin Test: ${category} ===`);
console.log("");

let totalFiles = 0;
let totalMatches = 0;
let pass = 0;
let fail = 0;
const results = [];

const files = readdirSync(catInput).filter((f) => {
  const ext = extname(f).toLowerCase();
  return statSync(join(catInput, f)).isFile() && TEXT_EXTENSIONS.has(ext);
});

for (const file of files) {
  const inputPath = join(catInput, file);
  const nameNoExt = basename(file, extname(file));

  try {
    const scriptPath = join(__dirname, "test_l1_plugin_file.mjs");
    execFileSync("node", [scriptPath, inputPath, catOutput], {
      stdio: ["pipe", "pipe", "pipe"],
      timeout: 30000,
    });

    // Read JSON result to get match count
    const jsonPath = join(catOutput, "json", `${nameNoExt}_l1_plugin.json`);
    const result = JSON.parse(readFileSync(jsonPath, "utf-8"));
    totalMatches += result.total_matches;
    console.log(`OK  ${file} — ${result.total_matches} matches`);
    pass++;
    results.push({ name: file, status: "pass", detail: `${result.total_matches} matches` });
  } catch (err) {
    console.log(`FAIL ${file} — ${err.message}`);
    fail++;
    results.push({ name: file, status: "fail", detail: err.message });
  }

  totalFiles++;
}

console.log("");
console.log("=== Summary ===");
console.log(`Category:       ${category}`);
console.log(`Files tested:   ${totalFiles}`);
console.log(`Passed:         ${pass}`);
console.log(`Failed:         ${fail}`);
console.log(`Total matches:  ${totalMatches}`);
console.log(`JSON results:   ${catOutput}/json/`);
console.log(`Redacted files: ${catOutput}/redacted/`);

// Write validation JSON
mkdirSync(catOutput, { recursive: true });
const validationJson = {
  test_suite: `l1_plugin_${category}`,
  timestamp: new Date().toISOString().replace(/\.\d{3}Z$/, "Z"),
  total: totalFiles,
  pass,
  fail,
  warn: 0,
  skip: 0,
  results,
};
writeFileSync(join(catOutput, "l1_plugin_category_validation.json"), JSON.stringify(validationJson, null, 2) + "\n");
