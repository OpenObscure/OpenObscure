#!/usr/bin/env node
// echo_server.mjs — Minimal HTTP server that captures FPE-encrypted request bodies
// from the OpenObscure proxy pass-through pipeline.
//
// The proxy FPE-encrypts PII in the outbound request, then forwards to this server.
// This server saves the encrypted request body so test scripts can extract it.
//
// Usage:
//   node test/scripts/echo_server.mjs
//
// Environment:
//   ECHO_PORT    — Listen port (default: 18791)
//   CAPTURE_DIR  — Directory for captured request bodies (default: /tmp/oo_echo_captures)
//
// Each request body is saved as <CAPTURE_DIR>/<X-Capture-Id>.json
// The X-Capture-Id header is set by the test scripts to avoid race conditions.

import { createServer } from "http";
import { writeFileSync, mkdirSync, unlinkSync } from "fs";
import { join } from "path";

const PORT = parseInt(process.env.ECHO_PORT || "18791");
const CAPTURE_DIR = process.env.CAPTURE_DIR || "/tmp/oo_echo_captures";
const PID_FILE = join(CAPTURE_DIR, "echo_server.pid");

mkdirSync(CAPTURE_DIR, { recursive: true });

let requestCount = 0;

const server = createServer((req, res) => {
  let body = "";
  req.on("data", (chunk) => (body += chunk));
  req.on("end", () => {
    requestCount++;

    // Use X-Capture-Id header if provided, otherwise auto-generate
    const captureId = req.headers["x-capture-id"] || `auto_${requestCount}`;
    const captureFile = join(CAPTURE_DIR, `${captureId}.json`);

    // Save the FPE-encrypted request body
    writeFileSync(captureFile, body);

    // Return a minimal valid Anthropic API response
    // The proxy will try to decrypt FPE values in the response, but since this
    // response contains no encrypted values, it passes through unchanged.
    const response = {
      id: `msg_echo_${requestCount}`,
      type: "message",
      role: "assistant",
      content: [{ type: "text", text: "Echo capture complete." }],
      model: "echo-server",
      stop_reason: "end_turn",
      usage: { input_tokens: 10, output_tokens: 5 },
    };

    res.writeHead(200, { "Content-Type": "application/json" });
    res.end(JSON.stringify(response));
  });
});

server.listen(PORT, "127.0.0.1", () => {
  // Write PID file for cleanup by test scripts
  writeFileSync(PID_FILE, String(process.pid));
  console.log(`Echo server listening on 127.0.0.1:${PORT}`);
  console.log(`Captures: ${CAPTURE_DIR}/`);
  console.log(`PID: ${process.pid}`);
});

// Graceful shutdown — remove PID file on any exit path
function removePidFile() {
  try { unlinkSync(PID_FILE); } catch { /* already removed */ }
}

process.on("SIGTERM", () => { removePidFile(); server.close(() => process.exit(0)); });
process.on("SIGINT",  () => { removePidFile(); server.close(() => process.exit(0)); });
process.on("exit",    () => { removePidFile(); });
