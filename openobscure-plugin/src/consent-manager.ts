/**
 * GDPR Consent Manager — SQLite-backed consent tracking, processing logs, and DSAR support.
 *
 * Provides:
 * - Consent records (grant/revoke per type: processing, storage, transfer, ai_disclosure)
 * - Data processing logs (audit trail of PII operations — types only, never values)
 * - DSAR (Data Subject Access Request) lifecycle (access, rectification, erasure, portability)
 * - AI disclosure text per GDPR Art. 13/14
 */

import Database from "better-sqlite3";
import * as path from "path";
import * as fs from "fs";

// ── Types ──

export type ConsentType = "processing" | "storage" | "transfer" | "ai_disclosure";
export type LegalBasis = "consent" | "legitimate_interest" | "contract";
export type ProcessingAction = "scan" | "encrypt" | "redact" | "store" | "delete";
export type DsarType = "access" | "rectification" | "erasure" | "portability";
export type DsarStatus = "pending" | "in_progress" | "completed" | "denied";

export interface ConsentRecord {
  id: number;
  user_id: string;
  consent_type: ConsentType;
  granted: boolean;
  granted_at: string | null;
  revoked_at: string | null;
  purpose: string | null;
  legal_basis: LegalBasis | null;
  version: number;
}

export interface ProcessingLogEntry {
  id: number;
  user_id: string;
  timestamp: string;
  action: ProcessingAction;
  pii_types: string | null;
  source: string | null;
  details: string | null;
}

export interface DsarRequest {
  id: number;
  user_id: string;
  request_type: DsarType;
  requested_at: string;
  completed_at: string | null;
  status: DsarStatus;
  response_path: string | null;
}

export interface ConsentStatus {
  user_id: string;
  consents: ConsentRecord[];
  processing_log_count: number;
  pending_dsars: number;
}

export interface PrivacyExport {
  exported_at: string;
  user_id: string;
  consents: ConsentRecord[];
  processing_log: ProcessingLogEntry[];
  dsar_requests: DsarRequest[];
}

// ── AI Disclosure Template (GDPR Art. 13/14) ──

export function aiDisclosureText(modelName: string, provider: string): string {
  return (
    `Your data will be processed by ${modelName} via ${provider}. ` +
    `PII has been encrypted using format-preserving encryption before reaching the AI model. ` +
    `OpenObscure scans requests for personal data (names, addresses, health information, ` +
    `financial data) and applies encryption or redaction to protect your privacy. ` +
    `You can manage your privacy settings with /privacy status, /privacy consent, ` +
    `and /privacy export commands. For more information, see GDPR Articles 13 and 14.`
  );
}

// ── ConsentManager ──

export class ConsentManager {
  private db: Database.Database;

  constructor(dbPath: string) {
    const dir = path.dirname(dbPath);
    if (!fs.existsSync(dir)) {
      fs.mkdirSync(dir, { recursive: true });
    }

    this.db = new Database(dbPath);
    this.db.pragma("journal_mode = WAL");
    this.db.pragma("foreign_keys = ON");
    this.initSchema();
  }

  private initSchema(): void {
    this.db.exec(`
      CREATE TABLE IF NOT EXISTS consent_records (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id TEXT NOT NULL,
        consent_type TEXT NOT NULL,
        granted INTEGER NOT NULL DEFAULT 0,
        granted_at TEXT,
        revoked_at TEXT,
        purpose TEXT,
        legal_basis TEXT,
        version INTEGER DEFAULT 1
      );

      CREATE TABLE IF NOT EXISTS data_processing_log (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id TEXT NOT NULL,
        timestamp TEXT NOT NULL,
        action TEXT NOT NULL,
        pii_types TEXT,
        source TEXT,
        details TEXT
      );

      CREATE TABLE IF NOT EXISTS dsar_requests (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id TEXT NOT NULL,
        request_type TEXT NOT NULL,
        requested_at TEXT NOT NULL,
        completed_at TEXT,
        status TEXT NOT NULL DEFAULT 'pending',
        response_path TEXT
      );

      CREATE TABLE IF NOT EXISTS retention_entries (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id TEXT NOT NULL,
        created_at TEXT NOT NULL,
        tier TEXT NOT NULL DEFAULT 'hot',
        expires_at TEXT NOT NULL,
        source_table TEXT NOT NULL,
        source_id INTEGER NOT NULL
      );

      CREATE INDEX IF NOT EXISTS idx_consent_user ON consent_records(user_id);
      CREATE INDEX IF NOT EXISTS idx_processing_user ON data_processing_log(user_id);
      CREATE INDEX IF NOT EXISTS idx_dsar_user ON dsar_requests(user_id);
      CREATE INDEX IF NOT EXISTS idx_retention_tier ON retention_entries(tier);
      CREATE INDEX IF NOT EXISTS idx_retention_expires ON retention_entries(expires_at);
    `);
  }

  // ── Consent CRUD ──

  /** Grant consent for a specific type. Creates or updates the record. */
  grantConsent(
    userId: string,
    consentType: ConsentType,
    purpose?: string,
    legalBasis?: LegalBasis
  ): ConsentRecord {
    const now = new Date().toISOString();

    // Check for existing active consent of this type
    const existing = this.db
      .prepare(
        `SELECT * FROM consent_records
         WHERE user_id = ? AND consent_type = ? AND granted = 1 AND revoked_at IS NULL
         ORDER BY id DESC LIMIT 1`
      )
      .get(userId, consentType) as ConsentRecord | undefined;

    if (existing) {
      // Already granted — bump version
      const newVersion = existing.version + 1;
      this.db
        .prepare(
          `UPDATE consent_records SET version = ?, granted_at = ?, purpose = COALESCE(?, purpose)
           WHERE id = ?`
        )
        .run(newVersion, now, purpose ?? null, existing.id);
      return { ...existing, version: newVersion, granted_at: now };
    }

    // New consent record
    const result = this.db
      .prepare(
        `INSERT INTO consent_records (user_id, consent_type, granted, granted_at, purpose, legal_basis)
         VALUES (?, ?, 1, ?, ?, ?)`
      )
      .run(userId, consentType, now, purpose ?? null, legalBasis ?? null);

    return {
      id: Number(result.lastInsertRowid),
      user_id: userId,
      consent_type: consentType,
      granted: true,
      granted_at: now,
      revoked_at: null,
      purpose: purpose ?? null,
      legal_basis: legalBasis ?? null,
      version: 1,
    };
  }

  /** Revoke consent for a specific type. */
  revokeConsent(userId: string, consentType: ConsentType): boolean {
    const now = new Date().toISOString();
    const result = this.db
      .prepare(
        `UPDATE consent_records SET granted = 0, revoked_at = ?
         WHERE user_id = ? AND consent_type = ? AND granted = 1 AND revoked_at IS NULL`
      )
      .run(now, userId, consentType);
    return result.changes > 0;
  }

  /** Get all consent records for a user. */
  getConsents(userId: string): ConsentRecord[] {
    return this.db
      .prepare(
        `SELECT * FROM consent_records WHERE user_id = ? ORDER BY id DESC`
      )
      .all(userId) as ConsentRecord[];
  }

  /** Check if a specific consent type is currently active. */
  hasActiveConsent(userId: string, consentType: ConsentType): boolean {
    const row = this.db
      .prepare(
        `SELECT 1 FROM consent_records
         WHERE user_id = ? AND consent_type = ? AND granted = 1 AND revoked_at IS NULL
         LIMIT 1`
      )
      .get(userId, consentType);
    return row !== undefined;
  }

  // ── Processing Log ──

  /** Log a data processing action (PII types only, never actual values). */
  logProcessing(
    userId: string,
    action: ProcessingAction,
    piiTypes?: string[],
    source?: string,
    details?: Record<string, unknown>
  ): void {
    const now = new Date().toISOString();
    this.db
      .prepare(
        `INSERT INTO data_processing_log (user_id, timestamp, action, pii_types, source, details)
         VALUES (?, ?, ?, ?, ?, ?)`
      )
      .run(
        userId,
        now,
        action,
        piiTypes ? piiTypes.join(",") : null,
        source ?? null,
        details ? JSON.stringify(details) : null
      );
  }

  /** Get processing log entries for a user. */
  getProcessingLog(userId: string, limit?: number): ProcessingLogEntry[] {
    const sql = limit
      ? `SELECT * FROM data_processing_log WHERE user_id = ? ORDER BY id DESC LIMIT ?`
      : `SELECT * FROM data_processing_log WHERE user_id = ? ORDER BY id DESC`;
    const params = limit ? [userId, limit] : [userId];
    return this.db.prepare(sql).all(...params) as ProcessingLogEntry[];
  }

  // ── DSAR (Data Subject Access Requests) ──

  /** Create a new DSAR request. */
  createDsar(userId: string, requestType: DsarType): DsarRequest {
    const now = new Date().toISOString();
    const result = this.db
      .prepare(
        `INSERT INTO dsar_requests (user_id, request_type, requested_at, status)
         VALUES (?, ?, ?, 'pending')`
      )
      .run(userId, requestType, now);

    return {
      id: Number(result.lastInsertRowid),
      user_id: userId,
      request_type: requestType,
      requested_at: now,
      completed_at: null,
      status: "pending",
      response_path: null,
    };
  }

  /** Update DSAR status. */
  updateDsarStatus(
    dsarId: number,
    status: DsarStatus,
    responsePath?: string
  ): boolean {
    const completedAt =
      status === "completed" ? new Date().toISOString() : null;
    const result = this.db
      .prepare(
        `UPDATE dsar_requests SET status = ?, completed_at = COALESCE(?, completed_at),
         response_path = COALESCE(?, response_path)
         WHERE id = ?`
      )
      .run(status, completedAt, responsePath ?? null, dsarId);
    return result.changes > 0;
  }

  /** Get DSAR requests for a user. */
  getDsarRequests(userId: string): DsarRequest[] {
    return this.db
      .prepare(
        `SELECT * FROM dsar_requests WHERE user_id = ? ORDER BY id DESC`
      )
      .all(userId) as DsarRequest[];
  }

  // ── Composite Operations ──

  /** Get full consent status for a user (for /privacy status). */
  getStatus(userId: string): ConsentStatus {
    const consents = this.getConsents(userId);

    const logCount = this.db
      .prepare(
        `SELECT COUNT(*) as count FROM data_processing_log WHERE user_id = ?`
      )
      .get(userId) as { count: number };

    const pendingDsars = this.db
      .prepare(
        `SELECT COUNT(*) as count FROM dsar_requests
         WHERE user_id = ? AND status IN ('pending', 'in_progress')`
      )
      .get(userId) as { count: number };

    return {
      user_id: userId,
      consents,
      processing_log_count: logCount.count,
      pending_dsars: pendingDsars.count,
    };
  }

  /** Export all user data (for DSAR access request). */
  exportUserData(userId: string): PrivacyExport {
    return {
      exported_at: new Date().toISOString(),
      user_id: userId,
      consents: this.getConsents(userId),
      processing_log: this.getProcessingLog(userId),
      dsar_requests: this.getDsarRequests(userId),
    };
  }

  /**
   * Delete all user data (for DSAR erasure request).
   * Returns count of deleted records across all tables.
   */
  deleteUserData(userId: string): number {
    const deleteAll = this.db.transaction(() => {
      const c1 = this.db
        .prepare(`DELETE FROM consent_records WHERE user_id = ?`)
        .run(userId);
      const c2 = this.db
        .prepare(`DELETE FROM data_processing_log WHERE user_id = ?`)
        .run(userId);
      const c3 = this.db
        .prepare(`DELETE FROM dsar_requests WHERE user_id = ?`)
        .run(userId);
      return c1.changes + c2.changes + c3.changes;
    });
    return deleteAll();
  }

  // ── Retention Tier Management ──

  /** Track a processing log entry for retention governance. */
  trackRetention(
    userId: string,
    sourceTable: string,
    sourceId: number,
    tier: RetentionTier,
    expiresAt: string
  ): void {
    const now = new Date().toISOString();
    this.db
      .prepare(
        `INSERT INTO retention_entries (user_id, created_at, tier, expires_at, source_table, source_id)
         VALUES (?, ?, ?, ?, ?, ?)`
      )
      .run(userId, now, tier, expiresAt, sourceTable, sourceId);
  }

  /** Get count of retention entries per tier. */
  getRetentionSummary(): RetentionSummary {
    const rows = this.db
      .prepare(
        `SELECT tier, COUNT(*) as count FROM retention_entries GROUP BY tier`
      )
      .all() as Array<{ tier: string; count: number }>;

    const summary: RetentionSummary = { hot: 0, warm: 0, cold: 0, expired: 0, total: 0 };
    for (const row of rows) {
      if (row.tier in summary) {
        (summary as unknown as Record<string, number>)[row.tier] = row.count;
      }
      summary.total += row.count;
    }
    return summary;
  }

  /** Get retention entries that should be promoted to the next tier. */
  getRetentionCandidates(tier: RetentionTier, now?: string): RetentionEntry[] {
    const currentTime = now ?? new Date().toISOString();
    return this.db
      .prepare(
        `SELECT * FROM retention_entries WHERE tier = ? AND expires_at <= ? ORDER BY expires_at ASC`
      )
      .all(tier, currentTime) as RetentionEntry[];
  }

  /** Update the tier of a retention entry. */
  updateRetentionTier(id: number, newTier: RetentionTier, newExpiresAt: string): void {
    this.db
      .prepare(
        `UPDATE retention_entries SET tier = ?, expires_at = ? WHERE id = ?`
      )
      .run(newTier, newExpiresAt, id);
  }

  /** Delete expired retention entries and their source records. Returns count deleted. */
  pruneExpired(now?: string): number {
    const currentTime = now ?? new Date().toISOString();
    const expired = this.db
      .prepare(
        `SELECT * FROM retention_entries WHERE tier = 'expired' AND expires_at <= ?`
      )
      .all(currentTime) as RetentionEntry[];

    if (expired.length === 0) return 0;

    const deleteRetention = this.db.prepare(
      `DELETE FROM retention_entries WHERE id = ?`
    );
    const deleteProcessingLog = this.db.prepare(
      `DELETE FROM data_processing_log WHERE id = ?`
    );

    const prune = this.db.transaction(() => {
      let count = 0;
      for (const entry of expired) {
        if (entry.source_table === "data_processing_log") {
          deleteProcessingLog.run(entry.source_id);
        }
        deleteRetention.run(entry.id);
        count++;
      }
      return count;
    });

    return prune();
  }

  /** Close the database connection. */
  close(): void {
    this.db.close();
  }
}

// ── Retention Types ──

export type RetentionTier = "hot" | "warm" | "cold" | "expired";

export interface RetentionEntry {
  id: number;
  user_id: string;
  created_at: string;
  tier: RetentionTier;
  expires_at: string;
  source_table: string;
  source_id: number;
}

export interface RetentionSummary {
  hot: number;
  warm: number;
  cold: number;
  expired: number;
  total: number;
}
