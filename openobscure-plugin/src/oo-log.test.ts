import { describe, it, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import * as fs from "fs";
import * as path from "path";
import * as os from "os";

import {
  ooLog,
  ooInfo,
  ooWarn,
  ooError,
  ooDebug,
  ooAudit,
  ooLogInit,
  ooLogShutdown,
  OO_MODULES,
  type OoLogLevel,
} from "./oo-log";

// ── Test helpers ──

/** Capture console output during a callback. */
function captureConsole(
  level: "log" | "warn" | "error",
  fn: () => void
): string[] {
  const captured: string[] = [];
  const original = console[level];
  console[level] = (...args: unknown[]) => {
    captured.push(args.map(String).join(" "));
  };
  try {
    fn();
  } finally {
    console[level] = original;
  }
  return captured;
}

// ── Tests ──

describe("ooLog", () => {
  beforeEach(() => {
    // Reset to defaults
    ooLogInit({ level: "info", jsonOutput: false });
  });

  afterEach(() => {
    ooLogShutdown();
  });

  it("logs info messages with module prefix", () => {
    const output = captureConsole("log", () => {
      ooInfo(OO_MODULES.REDACTOR, "Redacted PII", { count: 3 });
    });
    assert.equal(output.length, 1);
    assert.ok(output[0].includes("[OpenObscure L1] [redactor]"));
    assert.ok(output[0].includes("Redacted PII"));
    assert.ok(output[0].includes("count=3"));
  });

  it("logs warn messages via console.warn", () => {
    const output = captureConsole("warn", () => {
      ooWarn(OO_MODULES.HEARTBEAT, "Proxy not responding", { failures: 2 });
    });
    assert.equal(output.length, 1);
    assert.ok(output[0].includes("[heartbeat]"));
    assert.ok(output[0].includes("Proxy not responding"));
  });

  it("logs error messages via console.error", () => {
    const output = captureConsole("error", () => {
      ooError(OO_MODULES.PLUGIN, "Registration failed");
    });
    assert.equal(output.length, 1);
    assert.ok(output[0].includes("[plugin]"));
    assert.ok(output[0].includes("Registration failed"));
  });

  it("respects log level filtering", () => {
    ooLogInit({ level: "warn" });
    const infoOutput = captureConsole("log", () => {
      ooInfo(OO_MODULES.REDACTOR, "This should be filtered");
    });
    const warnOutput = captureConsole("warn", () => {
      ooWarn(OO_MODULES.REDACTOR, "This should appear");
    });
    assert.equal(infoOutput.length, 0);
    assert.equal(warnOutput.length, 1);
  });

  it("debug level filtered at info default", () => {
    const output = captureConsole("log", () => {
      ooDebug(OO_MODULES.CONSENT, "Debug detail");
    });
    assert.equal(output.length, 0);
  });

  it("debug level passes when level set to debug", () => {
    ooLogInit({ level: "debug" });
    const output = captureConsole("log", () => {
      ooDebug(OO_MODULES.CONSENT, "Debug detail");
    });
    assert.equal(output.length, 1);
    assert.ok(output[0].includes("Debug detail"));
  });

  it("produces JSON output when configured", () => {
    ooLogInit({ jsonOutput: true });
    const output = captureConsole("log", () => {
      ooInfo(OO_MODULES.REDACTOR, "PII found", { tool: "web_search", count: 2 });
    });
    assert.equal(output.length, 1);
    const parsed = JSON.parse(output[0]);
    assert.equal(parsed.level, "info");
    assert.equal(parsed.module, "openobscure.redactor");
    assert.equal(parsed.msg, "PII found");
    assert.equal(parsed.tool, "web_search");
    assert.equal(parsed.count, 2);
    assert.ok(parsed.ts); // ISO timestamp present
  });

  it("handles missing fields gracefully", () => {
    const output = captureConsole("log", () => {
      ooInfo(OO_MODULES.PLUGIN, "Simple message");
    });
    assert.equal(output.length, 1);
    assert.ok(output[0].includes("Simple message"));
    // No trailing space or "undefined"
    assert.ok(!output[0].includes("undefined"));
  });
});

describe("PII scrubbing", () => {
  beforeEach(() => {
    ooLogInit({ level: "info", jsonOutput: false });
  });

  it("scrubs SSN from message", () => {
    const output = captureConsole("log", () => {
      ooInfo(OO_MODULES.REDACTOR, "Found SSN: 123-45-6789 in text");
    });
    assert.equal(output.length, 1);
    assert.ok(!output[0].includes("123-45-6789"));
    assert.ok(output[0].includes("[REDACTED-SSN]"));
  });

  it("scrubs email from fields", () => {
    const output = captureConsole("log", () => {
      ooInfo(OO_MODULES.REDACTOR, "User data", { email: "user@example.com" });
    });
    assert.equal(output.length, 1);
    assert.ok(!output[0].includes("user@example.com"));
    assert.ok(output[0].includes("[REDACTED-EMAIL]"));
  });

  it("scrubs credit card from message", () => {
    const output = captureConsole("log", () => {
      ooInfo(OO_MODULES.REDACTOR, "Card: 4111-1111-1111-1111");
    });
    assert.equal(output.length, 1);
    assert.ok(!output[0].includes("4111-1111-1111-1111"));
    assert.ok(output[0].includes("[REDACTED-CC]"));
  });

  it("scrubs PII in JSON output mode too", () => {
    ooLogInit({ jsonOutput: true });
    const output = captureConsole("log", () => {
      ooInfo(OO_MODULES.REDACTOR, "SSN: 123-45-6789", { phone: "+1-555-123-4567" });
    });
    const parsed = JSON.parse(output[0]);
    assert.ok(!parsed.msg.includes("123-45-6789"));
    assert.ok(parsed.msg.includes("[REDACTED-SSN]"));
    assert.ok(
      typeof parsed.phone === "string" && !parsed.phone.includes("555-123-4567")
    );
  });

  it("leaves non-PII fields unchanged", () => {
    const output = captureConsole("log", () => {
      ooInfo(OO_MODULES.REDACTOR, "Stats", { pii_total: 3, tool: "web_search" });
    });
    assert.ok(output[0].includes("pii_total=3"));
    assert.ok(output[0].includes("tool=web_search"));
  });
});

describe("GDPR audit log", () => {
  let tmpDir: string;
  let auditPath: string;

  beforeEach(() => {
    tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "oo-log-test-"));
    auditPath = path.join(tmpDir, "audit.jsonl");
    ooLogInit({ level: "info", auditLogPath: auditPath });
  });

  afterEach(() => {
    ooLogShutdown();
    try {
      fs.rmSync(tmpDir, { recursive: true });
    } catch {
      // Cleanup best-effort
    }
  });

  it("writes audit entries to file", () => {
    // Capture console to avoid test noise
    captureConsole("log", () => {
      ooAudit(OO_MODULES.REDACTOR, "scrub", {
        user_id: "u123",
        pii_type: "email",
      });
    });

    const content = fs.readFileSync(auditPath, "utf-8").trim();
    const entry = JSON.parse(content);
    assert.equal(entry.module, "openobscure.redactor");
    assert.equal(entry.operation, "scrub");
    assert.equal(entry.user_id, "u123");
    assert.equal(entry.pii_type, "email");
    assert.ok(entry.ts);
  });

  it("appends multiple audit entries", () => {
    captureConsole("log", () => {
      ooAudit(OO_MODULES.REDACTOR, "redact", { pii_count: 2 });
      ooAudit(OO_MODULES.PLUGIN, "revoke", { user_id: "u456" });
    });

    const lines = fs.readFileSync(auditPath, "utf-8").trim().split("\n");
    assert.equal(lines.length, 2);
    assert.equal(JSON.parse(lines[0]).operation, "redact");
    assert.equal(JSON.parse(lines[1]).operation, "revoke");
  });

  it("also logs audit events to console at info level", () => {
    const output = captureConsole("log", () => {
      ooAudit(OO_MODULES.CONSENT, "export", { format: "json" });
    });
    assert.equal(output.length, 1);
    assert.ok(output[0].includes("audit: export"));
  });
});

describe("OO_MODULES constants", () => {
  it("all module constants are defined", () => {
    assert.equal(OO_MODULES.REDACTOR, "redactor");
    assert.equal(OO_MODULES.HEARTBEAT, "heartbeat");
    assert.equal(OO_MODULES.PLUGIN, "plugin");
  });
});
