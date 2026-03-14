/**
 * postinstall.js — compile from source when no prebuilt binary is available.
 *
 * Runs automatically after `npm install`. Checks whether a prebuilt platform
 * binary already exists (installed from the platform-specific npm package).
 * If one does, exits immediately. If not, attempts to compile from source
 * using the local Rust toolchain. Silently skips if Rust is not installed.
 */

/* eslint-disable @typescript-eslint/no-require-imports */
const { execSync } = require("child_process");
const { existsSync } = require("fs");
const { join } = require("path");

// Platform binary names that napi-rs generates
const platformBinaries = [
  `scanner.${process.platform}-${process.arch}.node`,
  `scanner.${process.platform}-${process.arch}-gnu.node`,
  `scanner.${process.platform}-${process.arch}-musl.node`,
  "scanner.node",
];

const prebuiltExists = platformBinaries.some((name) =>
  existsSync(join(__dirname, name))
);

if (prebuiltExists) {
  // Prebuilt binary present — nothing to do
  process.exit(0);
}

// No prebuilt binary. Try compiling from source if Rust is available.
try {
  execSync("cargo --version", { stdio: "ignore" });
} catch {
  // Rust not installed — cannot compile. Binary must be installed via
  // the platform-specific npm package (@openobscure/scanner-napi-<platform>).
  process.exit(0);
}

try {
  process.stdout.write(
    "[OpenObscure] No prebuilt binary found — compiling scanner-napi from source...\n"
  );
  execSync("npm run build", { stdio: "inherit", cwd: __dirname });
  process.stdout.write("[OpenObscure] scanner-napi compiled successfully.\n");
} catch (e) {
  // Build failed — log but don't block install. The plugin will fall back
  // to JS regex until a working binary is available.
  process.stderr.write(
    "[OpenObscure] scanner-napi source compile failed: " + String(e) + "\n" +
    "The plugin will use JS regex fallback (5 types) until a binary is available.\n"
  );
}
