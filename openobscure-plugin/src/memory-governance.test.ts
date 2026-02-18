import { describe, it, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import * as fs from "fs";
import * as path from "path";
import * as os from "os";
import { ConsentManager } from "./consent-manager";
import { MemoryGovernor, DEFAULT_RETENTION_POLICY } from "./memory-governance";
import { handlePrivacyCommand } from "./privacy-commands";

let manager: ConsentManager;
let tmpDir: string;
let dbPath: string;

function setup() {
  tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "openobscure-retention-"));
  dbPath = path.join(tmpDir, "retention.db");
  manager = new ConsentManager(dbPath);
}

function teardown() {
  manager.close();
  fs.rmSync(tmpDir, { recursive: true, force: true });
}

/** Create a date N days in the past from a reference date. */
function daysAgo(days: number, from?: Date): Date {
  const d = new Date(from ?? Date.now());
  d.setDate(d.getDate() - days);
  return d;
}

// ── MemoryGovernor Unit Tests ──

describe("MemoryGovernor", () => {
  beforeEach(() => setup());
  afterEach(() => teardown());

  it("tracks entries in hot tier", () => {
    const governor = new MemoryGovernor(manager);
    governor.trackEntry("user1", 1);
    governor.trackEntry("user1", 2);

    const summary = governor.getSummary();
    assert.equal(summary.hot, 2);
    assert.equal(summary.total, 2);
  });

  it("returns default policy", () => {
    const governor = new MemoryGovernor(manager);
    const policy = governor.getPolicy();
    assert.equal(policy.hotDays, 7);
    assert.equal(policy.warmDays, 30);
    assert.equal(policy.coldDays, 90);
  });

  it("accepts custom policy", () => {
    const governor = new MemoryGovernor(manager, { hotDays: 3, warmDays: 14 });
    const policy = governor.getPolicy();
    assert.equal(policy.hotDays, 3);
    assert.equal(policy.warmDays, 14);
    assert.equal(policy.coldDays, 90); // default
  });

  it("promotes hot → warm when expired", () => {
    const governor = new MemoryGovernor(manager, { hotDays: 7 });
    const now = new Date("2026-02-17T12:00:00Z");

    // Track entry that was created 8 days ago with hot expiry 1 day ago
    const pastExpiry = daysAgo(1, now);
    manager.trackRetention("user1", "data_processing_log", 100, "hot", pastExpiry.toISOString());

    const result = governor.enforce(now);
    assert.equal(result.promoted, 1);

    const summary = governor.getSummary();
    assert.equal(summary.hot, 0);
    assert.equal(summary.warm, 1);
  });

  it("promotes warm → cold when expired", () => {
    const governor = new MemoryGovernor(manager, { warmDays: 30, coldDays: 90 });
    const now = new Date("2026-02-17T12:00:00Z");

    const pastExpiry = daysAgo(1, now);
    manager.trackRetention("user1", "data_processing_log", 200, "warm", pastExpiry.toISOString());

    const result = governor.enforce(now);
    assert.equal(result.promoted, 1);

    const summary = governor.getSummary();
    assert.equal(summary.warm, 0);
    assert.equal(summary.cold, 1);
  });

  it("promotes cold → expired when expired", () => {
    const governor = new MemoryGovernor(manager, { coldDays: 90 });
    const now = new Date("2026-02-17T12:00:00Z");

    const pastExpiry = daysAgo(1, now);
    manager.trackRetention("user1", "data_processing_log", 300, "cold", pastExpiry.toISOString());

    const result = governor.enforce(now);
    assert.ok(result.promoted >= 1);

    const summary = governor.getSummary();
    assert.equal(summary.cold, 0);
    assert.equal(summary.expired, 1);
  });

  it("prunes expired entries and source records", () => {
    const governor = new MemoryGovernor(manager);
    const now = new Date("2026-02-17T12:00:00Z");

    // Create a processing log entry
    manager.logProcessing("user1", "scan", ["email"], "proxy");
    const log = manager.getProcessingLog("user1");
    assert.equal(log.length, 1);
    const logId = log[0].id;

    // Track it as expired
    const pastExpiry = daysAgo(1, now);
    manager.trackRetention("user1", "data_processing_log", logId, "expired", pastExpiry.toISOString());

    const result = governor.enforce(now);
    assert.equal(result.pruned, 1);

    // Source record should be deleted
    const logAfter = manager.getProcessingLog("user1");
    assert.equal(logAfter.length, 0);
  });

  it("does not promote entries that have not expired", () => {
    const governor = new MemoryGovernor(manager, { hotDays: 7 });
    const now = new Date("2026-02-17T12:00:00Z");

    // Entry expires in the future
    const futureExpiry = new Date(now.getTime() + 86400000); // +1 day
    manager.trackRetention("user1", "data_processing_log", 400, "hot", futureExpiry.toISOString());

    const result = governor.enforce(now);
    assert.equal(result.promoted, 0);
    assert.equal(result.pruned, 0);

    const summary = governor.getSummary();
    assert.equal(summary.hot, 1);
  });

  it("handles empty database gracefully", () => {
    const governor = new MemoryGovernor(manager);
    const result = governor.enforce();
    assert.equal(result.promoted, 0);
    assert.equal(result.pruned, 0);

    const summary = governor.getSummary();
    assert.equal(summary.total, 0);
  });

  it("cascades through multiple tiers in single enforce", () => {
    const governor = new MemoryGovernor(manager, { hotDays: 7, warmDays: 30, coldDays: 90 });
    const now = new Date("2026-02-17T12:00:00Z");

    // hot entry that expired
    const pastExpiry1 = daysAgo(1, now);
    manager.trackRetention("user1", "data_processing_log", 500, "hot", pastExpiry1.toISOString());

    // warm entry that expired
    const pastExpiry2 = daysAgo(2, now);
    manager.trackRetention("user1", "data_processing_log", 501, "warm", pastExpiry2.toISOString());

    // cold entry that expired (will become expired)
    const pastExpiry3 = daysAgo(3, now);
    manager.trackRetention("user1", "data_processing_log", 502, "cold", pastExpiry3.toISOString());

    const result = governor.enforce(now);
    assert.equal(result.promoted, 3); // hot→warm, warm→cold, cold→expired

    const summary = governor.getSummary();
    assert.equal(summary.hot, 0);
    assert.equal(summary.warm, 1);
    assert.equal(summary.cold, 1);
    assert.equal(summary.expired, 1);
  });

  it("multiple enforce calls are idempotent for non-expired", () => {
    const governor = new MemoryGovernor(manager, { hotDays: 7 });
    const now = new Date("2026-02-17T12:00:00Z");

    const pastExpiry = daysAgo(1, now);
    manager.trackRetention("user1", "data_processing_log", 600, "hot", pastExpiry.toISOString());

    const result1 = governor.enforce(now);
    assert.equal(result1.promoted, 1);

    // Second enforce — entry is now warm with future expiry, should not promote
    const result2 = governor.enforce(now);
    assert.equal(result2.promoted, 0);

    const summary = governor.getSummary();
    assert.equal(summary.warm, 1);
  });
});

// ── Privacy Retention Commands ──

describe("Privacy Retention Commands", () => {
  beforeEach(() => setup());
  afterEach(() => teardown());

  it("/privacy retention status shows tier counts", () => {
    const governor = new MemoryGovernor(manager);
    governor.trackEntry("user1", 1);
    governor.trackEntry("user1", 2);

    const result = handlePrivacyCommand(manager, "user1", ["retention", "status"], {
      governor,
    });
    assert.equal(result.success, true);
    assert.ok(result.text.includes("hot:     2"));
    assert.ok(result.text.includes("total:   2"));
  });

  it("/privacy retention enforce runs promotion", () => {
    const governor = new MemoryGovernor(manager);

    const result = handlePrivacyCommand(manager, "user1", ["retention", "enforce"], {
      governor,
    });
    assert.equal(result.success, true);
    assert.ok(result.text.includes("Promoted:"));
    assert.ok(result.text.includes("Pruned:"));
  });

  it("/privacy retention policy shows policy", () => {
    const governor = new MemoryGovernor(manager);

    const result = handlePrivacyCommand(manager, "user1", ["retention", "policy"], {
      governor,
    });
    assert.equal(result.success, true);
    assert.ok(result.text.includes("7 days"));
    assert.ok(result.text.includes("30 days"));
    assert.ok(result.text.includes("90 days"));
  });

  it("/privacy retention without governor shows error", () => {
    const result = handlePrivacyCommand(manager, "user1", ["retention", "status"]);
    assert.equal(result.success, false);
    assert.ok(result.text.includes("not enabled"));
  });

  it("/privacy retention with no subcommand shows usage", () => {
    const governor = new MemoryGovernor(manager);
    const result = handlePrivacyCommand(manager, "user1", ["retention"], {
      governor,
    });
    assert.equal(result.success, false);
    assert.ok(result.text.includes("Usage:"));
  });

  it("/privacy help includes retention commands", () => {
    const result = handlePrivacyCommand(manager, "user1", []);
    assert.ok(result.text.includes("retention status"));
    assert.ok(result.text.includes("retention enforce"));
    assert.ok(result.text.includes("retention policy"));
  });
});
