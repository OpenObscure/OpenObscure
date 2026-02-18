import { describe, it, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import * as fs from "fs";
import * as path from "path";
import * as os from "os";
import { ConsentManager, aiDisclosureText } from "./consent-manager";
import { handlePrivacyCommand } from "./privacy-commands";

let manager: ConsentManager;
let tmpDir: string;
let dbPath: string;

function setup() {
  tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "openobscure-consent-"));
  dbPath = path.join(tmpDir, "consent.db");
  manager = new ConsentManager(dbPath);
}

function teardown() {
  manager.close();
  fs.rmSync(tmpDir, { recursive: true, force: true });
}

// ── ConsentManager Unit Tests ──

describe("ConsentManager", () => {
  beforeEach(() => setup());
  afterEach(() => teardown());

  it("grants consent and retrieves it", () => {
    const record = manager.grantConsent("user1", "processing", "PII scanning", "consent");
    assert.equal(record.user_id, "user1");
    assert.equal(record.consent_type, "processing");
    assert.equal(record.granted, true);
    assert.equal(record.version, 1);
    assert.ok(record.granted_at);

    const consents = manager.getConsents("user1");
    assert.equal(consents.length, 1);
    assert.equal(consents[0].consent_type, "processing");
  });

  it("bumps version on re-grant", () => {
    manager.grantConsent("user1", "processing");
    const second = manager.grantConsent("user1", "processing");
    assert.equal(second.version, 2);

    const consents = manager.getConsents("user1");
    assert.equal(consents.length, 1);
  });

  it("revokes consent", () => {
    manager.grantConsent("user1", "storage");
    const revoked = manager.revokeConsent("user1", "storage");
    assert.equal(revoked, true);

    assert.equal(manager.hasActiveConsent("user1", "storage"), false);

    const consents = manager.getConsents("user1");
    assert.equal(consents[0].granted, 0);
    assert.ok(consents[0].revoked_at);
  });

  it("returns false when revoking non-existent consent", () => {
    const revoked = manager.revokeConsent("user1", "transfer");
    assert.equal(revoked, false);
  });

  it("checks active consent", () => {
    assert.equal(manager.hasActiveConsent("user1", "processing"), false);
    manager.grantConsent("user1", "processing");
    assert.equal(manager.hasActiveConsent("user1", "processing"), true);
    manager.revokeConsent("user1", "processing");
    assert.equal(manager.hasActiveConsent("user1", "processing"), false);
  });

  it("tracks multiple consent types independently", () => {
    manager.grantConsent("user1", "processing");
    manager.grantConsent("user1", "storage");
    manager.grantConsent("user1", "ai_disclosure");

    assert.equal(manager.hasActiveConsent("user1", "processing"), true);
    assert.equal(manager.hasActiveConsent("user1", "storage"), true);
    assert.equal(manager.hasActiveConsent("user1", "transfer"), false);

    const consents = manager.getConsents("user1");
    assert.equal(consents.length, 3);
  });

  it("isolates users", () => {
    manager.grantConsent("user1", "processing");
    manager.grantConsent("user2", "storage");

    assert.equal(manager.hasActiveConsent("user1", "processing"), true);
    assert.equal(manager.hasActiveConsent("user1", "storage"), false);
    assert.equal(manager.hasActiveConsent("user2", "processing"), false);
    assert.equal(manager.hasActiveConsent("user2", "storage"), true);
  });
});

describe("Processing Log", () => {
  beforeEach(() => setup());
  afterEach(() => teardown());

  it("logs and retrieves processing entries", () => {
    manager.logProcessing("user1", "scan", ["email", "ssn"], "proxy");
    manager.logProcessing("user1", "encrypt", ["credit_card"], "proxy");

    const log = manager.getProcessingLog("user1");
    assert.equal(log.length, 2);
    assert.equal(log[0].action, "encrypt"); // DESC order
    assert.equal(log[0].pii_types, "credit_card");
    assert.equal(log[1].action, "scan");
    assert.equal(log[1].pii_types, "email,ssn");
  });

  it("respects limit parameter", () => {
    for (let i = 0; i < 5; i++) {
      manager.logProcessing("user1", "scan", ["email"], "proxy");
    }
    const log = manager.getProcessingLog("user1", 3);
    assert.equal(log.length, 3);
  });

  it("logs with optional details JSON", () => {
    manager.logProcessing("user1", "redact", ["person"], "plugin", {
      tool: "web_search",
      count: 2,
    });
    const log = manager.getProcessingLog("user1");
    assert.equal(log.length, 1);
    const details = JSON.parse(log[0].details!);
    assert.equal(details.tool, "web_search");
    assert.equal(details.count, 2);
  });
});

describe("DSAR Requests", () => {
  beforeEach(() => setup());
  afterEach(() => teardown());

  it("creates and retrieves DSAR requests", () => {
    const dsar = manager.createDsar("user1", "access");
    assert.equal(dsar.user_id, "user1");
    assert.equal(dsar.request_type, "access");
    assert.equal(dsar.status, "pending");
    assert.ok(dsar.requested_at);

    const dsars = manager.getDsarRequests("user1");
    assert.equal(dsars.length, 1);
  });

  it("updates DSAR status to completed", () => {
    const dsar = manager.createDsar("user1", "access");
    const updated = manager.updateDsarStatus(dsar.id, "completed", "/tmp/export.json");
    assert.equal(updated, true);

    const dsars = manager.getDsarRequests("user1");
    assert.equal(dsars[0].status, "completed");
    assert.ok(dsars[0].completed_at);
    assert.equal(dsars[0].response_path, "/tmp/export.json");
  });

  it("tracks multiple DSAR types", () => {
    manager.createDsar("user1", "access");
    manager.createDsar("user1", "erasure");
    manager.createDsar("user1", "portability");

    const dsars = manager.getDsarRequests("user1");
    assert.equal(dsars.length, 3);
  });
});

describe("Composite Operations", () => {
  beforeEach(() => setup());
  afterEach(() => teardown());

  it("getStatus returns full summary", () => {
    manager.grantConsent("user1", "processing");
    manager.grantConsent("user1", "storage");
    manager.logProcessing("user1", "scan", ["email"], "proxy");
    manager.logProcessing("user1", "encrypt", ["ssn"], "proxy");
    manager.createDsar("user1", "access");

    const status = manager.getStatus("user1");
    assert.equal(status.user_id, "user1");
    assert.equal(status.consents.length, 2);
    assert.equal(status.processing_log_count, 2);
    assert.equal(status.pending_dsars, 1);
  });

  it("exportUserData includes all tables", () => {
    manager.grantConsent("user1", "processing");
    manager.logProcessing("user1", "scan", ["email"], "proxy");
    manager.createDsar("user1", "access");

    const data = manager.exportUserData("user1");
    assert.equal(data.user_id, "user1");
    assert.equal(data.consents.length, 1);
    assert.equal(data.processing_log.length, 1);
    assert.equal(data.dsar_requests.length, 1);
    assert.ok(data.exported_at);
  });

  it("deleteUserData removes all records", () => {
    manager.grantConsent("user1", "processing");
    manager.grantConsent("user1", "storage");
    manager.logProcessing("user1", "scan", ["email"], "proxy");
    manager.logProcessing("user1", "encrypt", ["ssn"], "proxy");
    manager.createDsar("user1", "access");

    const deleted = manager.deleteUserData("user1");
    assert.equal(deleted, 5); // 2 consents + 2 logs + 1 dsar

    assert.equal(manager.getConsents("user1").length, 0);
    assert.equal(manager.getProcessingLog("user1").length, 0);
    assert.equal(manager.getDsarRequests("user1").length, 0);
  });

  it("deleteUserData does not affect other users", () => {
    manager.grantConsent("user1", "processing");
    manager.grantConsent("user2", "storage");

    manager.deleteUserData("user1");

    assert.equal(manager.getConsents("user1").length, 0);
    assert.equal(manager.getConsents("user2").length, 1);
  });
});

// ── Privacy Command Tests ──

describe("Privacy Commands", () => {
  beforeEach(() => setup());
  afterEach(() => teardown());

  it("/privacy status shows empty state", () => {
    const result = handlePrivacyCommand(manager, "user1", ["status"]);
    assert.equal(result.success, true);
    assert.ok(result.text.includes("Active Consents: none"));
    assert.ok(result.text.includes("Processing Log: 0"));
  });

  it("/privacy consent grant creates consent", () => {
    const result = handlePrivacyCommand(manager, "user1", [
      "consent", "grant", "processing",
    ]);
    assert.equal(result.success, true);
    assert.ok(result.text.includes("Consent granted"));
    assert.equal(manager.hasActiveConsent("user1", "processing"), true);
  });

  it("/privacy consent grant defaults to processing", () => {
    const result = handlePrivacyCommand(manager, "user1", [
      "consent", "grant",
    ]);
    assert.equal(result.success, true);
    assert.equal(manager.hasActiveConsent("user1", "processing"), true);
  });

  it("/privacy consent revoke removes consent", () => {
    manager.grantConsent("user1", "storage");
    const result = handlePrivacyCommand(manager, "user1", [
      "consent", "revoke", "storage",
    ]);
    assert.equal(result.success, true);
    assert.ok(result.text.includes("revoked"));
    assert.equal(manager.hasActiveConsent("user1", "storage"), false);
  });

  it("/privacy consent revoke fails gracefully for non-existent", () => {
    const result = handlePrivacyCommand(manager, "user1", [
      "consent", "revoke", "transfer",
    ]);
    assert.equal(result.success, false);
    assert.ok(result.text.includes("No active consent"));
  });

  it("/privacy consent with invalid type returns error", () => {
    const result = handlePrivacyCommand(manager, "user1", [
      "consent", "grant", "invalid_type" as any,
    ]);
    assert.equal(result.success, false);
    assert.ok(result.text.includes("Invalid consent type"));
  });

  it("/privacy export creates DSAR and exports data", () => {
    manager.grantConsent("user1", "processing");
    manager.logProcessing("user1", "scan", ["email"], "proxy");

    const exportDir = path.join(tmpDir, "exports");
    const result = handlePrivacyCommand(
      manager, "user1", ["export"], exportDir
    );
    assert.equal(result.success, true);
    assert.ok(result.text.includes("Data export complete"));
    assert.ok(result.text.includes("DSAR request"));

    // Verify file was written
    const files = fs.readdirSync(exportDir);
    assert.equal(files.length, 1);
    assert.ok(files[0].startsWith("privacy-export-user1-"));

    const exported = JSON.parse(
      fs.readFileSync(path.join(exportDir, files[0]), "utf-8")
    );
    assert.equal(exported.user_id, "user1");
    assert.equal(exported.consents.length, 1);
  });

  it("/privacy export without dir returns inline summary", () => {
    manager.grantConsent("user1", "processing");
    const result = handlePrivacyCommand(manager, "user1", ["export"]);
    assert.equal(result.success, true);
    assert.ok(result.text.includes("Consent records: 1"));
  });

  it("/privacy delete erases all data", () => {
    manager.grantConsent("user1", "processing");
    manager.logProcessing("user1", "scan", ["email"], "proxy");
    manager.createDsar("user1", "access");

    const result = handlePrivacyCommand(manager, "user1", ["delete"]);
    assert.equal(result.success, true);
    assert.ok(result.text.includes("erasure complete"));

    // Verify data is gone (except the completion DSAR)
    assert.equal(manager.getConsents("user1").length, 0);
    assert.equal(manager.getProcessingLog("user1").length, 0);
  });

  it("/privacy disclosure shows Art. 13/14 text", () => {
    const result = handlePrivacyCommand(manager, "user1", [
      "disclosure", "Claude", "Anthropic",
    ]);
    assert.equal(result.success, true);
    assert.ok(result.text.includes("Claude"));
    assert.ok(result.text.includes("Anthropic"));
    assert.ok(result.text.includes("format-preserving encryption"));
  });

  it("/privacy with no args shows help", () => {
    const result = handlePrivacyCommand(manager, "user1", []);
    assert.equal(result.success, true);
    assert.ok(result.text.includes("/privacy status"));
    assert.ok(result.text.includes("/privacy export"));
  });

  it("/privacy status shows granted consents", () => {
    manager.grantConsent("user1", "processing", "PII scanning", "consent");
    manager.grantConsent("user1", "ai_disclosure");
    manager.logProcessing("user1", "scan", ["email"], "proxy");

    const result = handlePrivacyCommand(manager, "user1", ["status"]);
    assert.equal(result.success, true);
    assert.ok(result.text.includes("[granted] processing"));
    assert.ok(result.text.includes("[granted] ai_disclosure"));
    assert.ok(result.text.includes("Processing Log: 1"));
  });
});

describe("AI Disclosure", () => {
  it("generates disclosure text with model and provider", () => {
    const text = aiDisclosureText("GPT-4", "OpenAI");
    assert.ok(text.includes("GPT-4"));
    assert.ok(text.includes("OpenAI"));
    assert.ok(text.includes("format-preserving encryption"));
    assert.ok(text.includes("GDPR Articles 13 and 14"));
  });
});
