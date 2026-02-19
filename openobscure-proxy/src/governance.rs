//! L1 Governance Engine — Rust port of consent-manager.ts, file-guard.ts, memory-governance.ts.
//!
//! Provides on-device privacy governance for the Embedded (mobile) model:
//! - ConsentStore: GDPR consent records, processing logs, DSAR lifecycle (SQLite)
//! - FileGuard: deny-pattern matching for sensitive file paths (stateless)
//! - RetentionManager: tier-based data lifecycle (hot → warm → cold → expired)
//! - GovernanceEngine: composite of all three + privacy command router
//!
//! Gated behind the `governance` feature flag.

use std::sync::Arc;

use chrono::{Duration, Utc};
use regex::Regex;
use rusqlite::{params, Connection, OptionalExtension};

// ── Error Type ──

#[derive(Debug, thiserror::Error)]
pub enum GovernanceError {
    #[error("Database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("Invalid consent type: {0}")]
    InvalidConsentType(String),
    #[error("Invalid DSAR type: {0}")]
    InvalidDsarType(String),
    #[error("Invalid processing action: {0}")]
    InvalidAction(String),
    #[error("Invalid retention tier: {0}")]
    InvalidTier(String),
    #[error("Governance not enabled")]
    NotEnabled,
}

// ── Enums ──

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsentType {
    Processing,
    Storage,
    Transfer,
    AiDisclosure,
}

impl ConsentType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Processing => "processing",
            Self::Storage => "storage",
            Self::Transfer => "transfer",
            Self::AiDisclosure => "ai_disclosure",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Result<Self, GovernanceError> {
        match s {
            "processing" => Ok(Self::Processing),
            "storage" => Ok(Self::Storage),
            "transfer" => Ok(Self::Transfer),
            "ai_disclosure" => Ok(Self::AiDisclosure),
            _ => Err(GovernanceError::InvalidConsentType(s.to_string())),
        }
    }

    pub fn all() -> &'static [ConsentType] {
        &[
            Self::Processing,
            Self::Storage,
            Self::Transfer,
            Self::AiDisclosure,
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegalBasis {
    Consent,
    LegitimateInterest,
    Contract,
}

impl LegalBasis {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Consent => "consent",
            Self::LegitimateInterest => "legitimate_interest",
            Self::Contract => "contract",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessingAction {
    Scan,
    Encrypt,
    Redact,
    Store,
    Delete,
}

impl ProcessingAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Scan => "scan",
            Self::Encrypt => "encrypt",
            Self::Redact => "redact",
            Self::Store => "store",
            Self::Delete => "delete",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Result<Self, GovernanceError> {
        match s {
            "scan" => Ok(Self::Scan),
            "encrypt" => Ok(Self::Encrypt),
            "redact" => Ok(Self::Redact),
            "store" => Ok(Self::Store),
            "delete" => Ok(Self::Delete),
            _ => Err(GovernanceError::InvalidAction(s.to_string())),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DsarType {
    Access,
    Rectification,
    Erasure,
    Portability,
}

impl DsarType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Access => "access",
            Self::Rectification => "rectification",
            Self::Erasure => "erasure",
            Self::Portability => "portability",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Result<Self, GovernanceError> {
        match s {
            "access" => Ok(Self::Access),
            "rectification" => Ok(Self::Rectification),
            "erasure" => Ok(Self::Erasure),
            "portability" => Ok(Self::Portability),
            _ => Err(GovernanceError::InvalidDsarType(s.to_string())),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DsarStatus {
    Pending,
    InProgress,
    Completed,
    Denied,
}

impl DsarStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
            Self::Denied => "denied",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetentionTier {
    Hot,
    Warm,
    Cold,
    Expired,
}

impl RetentionTier {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Hot => "hot",
            Self::Warm => "warm",
            Self::Cold => "cold",
            Self::Expired => "expired",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Result<Self, GovernanceError> {
        match s {
            "hot" => Ok(Self::Hot),
            "warm" => Ok(Self::Warm),
            "cold" => Ok(Self::Cold),
            "expired" => Ok(Self::Expired),
            _ => Err(GovernanceError::InvalidTier(s.to_string())),
        }
    }
}

// ── Record Structs ──

#[derive(Debug, Clone, serde::Serialize)]
pub struct ConsentRecord {
    pub id: i64,
    pub user_id: String,
    pub consent_type: String,
    pub granted: bool,
    pub granted_at: Option<String>,
    pub revoked_at: Option<String>,
    pub purpose: Option<String>,
    pub legal_basis: Option<String>,
    pub version: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ProcessingLogEntry {
    pub id: i64,
    pub user_id: String,
    pub timestamp: String,
    pub action: String,
    pub pii_types: Option<String>,
    pub source: Option<String>,
    pub details: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DsarRequest {
    pub id: i64,
    pub user_id: String,
    pub request_type: String,
    pub requested_at: String,
    pub completed_at: Option<String>,
    pub status: String,
    pub response_path: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RetentionEntry {
    pub id: i64,
    pub user_id: String,
    pub created_at: String,
    pub tier: String,
    pub expires_at: String,
    pub source_table: String,
    pub source_id: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ConsentStatus {
    pub user_id: String,
    pub consents: Vec<ConsentRecord>,
    pub processing_log_count: i64,
    pub pending_dsars: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PrivacyExport {
    pub exported_at: String,
    pub user_id: String,
    pub consents: Vec<ConsentRecord>,
    pub processing_log: Vec<ProcessingLogEntry>,
    pub dsar_requests: Vec<DsarRequest>,
}

#[derive(Debug, Clone, Default)]
pub struct RetentionSummary {
    pub hot: u32,
    pub warm: u32,
    pub cold: u32,
    pub expired: u32,
    pub total: u32,
}

// ── ConsentStore ──

pub struct ConsentStore {
    conn: std::sync::Mutex<Connection>,
}

impl ConsentStore {
    /// Open (or create) a governance SQLite database.
    pub fn open(db_path: &str) -> Result<Self, GovernanceError> {
        let conn = if db_path == ":memory:" {
            Connection::open_in_memory()?
        } else {
            if let Some(parent) = std::path::Path::new(db_path).parent() {
                std::fs::create_dir_all(parent).ok();
            }
            Connection::open(db_path)?
        };
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
        let store = Self {
            conn: std::sync::Mutex::new(conn),
        };
        store.init_schema()?;
        Ok(store)
    }

    fn init_schema(&self) -> Result<(), GovernanceError> {
        self.conn.lock().unwrap().execute_batch(
            "CREATE TABLE IF NOT EXISTS consent_records (
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
            CREATE INDEX IF NOT EXISTS idx_retention_expires ON retention_entries(expires_at);",
        )?;
        Ok(())
    }

    // ── Consent CRUD ──

    /// Grant consent for a specific type. Creates or updates the record.
    pub fn grant_consent(
        &self,
        user_id: &str,
        consent_type: ConsentType,
        purpose: Option<&str>,
        legal_basis: Option<LegalBasis>,
    ) -> Result<ConsentRecord, GovernanceError> {
        let now = Utc::now().to_rfc3339();
        let ct = consent_type.as_str();
        let conn = self.conn.lock().unwrap();

        // Check for existing active consent
        let existing: Option<(i64, i64)> = conn
            .query_row(
                "SELECT id, version FROM consent_records
             WHERE user_id = ?1 AND consent_type = ?2 AND granted = 1 AND revoked_at IS NULL
             ORDER BY id DESC LIMIT 1",
                params![user_id, ct],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;

        if let Some((id, version)) = existing {
            let new_version = version + 1;
            conn.execute(
                "UPDATE consent_records SET version = ?1, granted_at = ?2, purpose = COALESCE(?3, purpose)
                 WHERE id = ?4",
                params![new_version, now, purpose, id],
            )?;
            return Ok(ConsentRecord {
                id,
                user_id: user_id.to_string(),
                consent_type: ct.to_string(),
                granted: true,
                granted_at: Some(now),
                revoked_at: None,
                purpose: purpose.map(|s| s.to_string()),
                legal_basis: legal_basis.map(|b| b.as_str().to_string()),
                version: new_version,
            });
        }

        let lb = legal_basis.map(|b| b.as_str().to_string());
        conn.execute(
            "INSERT INTO consent_records (user_id, consent_type, granted, granted_at, purpose, legal_basis)
             VALUES (?1, ?2, 1, ?3, ?4, ?5)",
            params![user_id, ct, now, purpose, lb],
        )?;
        let id = conn.last_insert_rowid();

        Ok(ConsentRecord {
            id,
            user_id: user_id.to_string(),
            consent_type: ct.to_string(),
            granted: true,
            granted_at: Some(now),
            revoked_at: None,
            purpose: purpose.map(|s| s.to_string()),
            legal_basis: lb,
            version: 1,
        })
    }

    /// Revoke consent for a specific type.
    pub fn revoke_consent(
        &self,
        user_id: &str,
        consent_type: ConsentType,
    ) -> Result<bool, GovernanceError> {
        let now = Utc::now().to_rfc3339();
        let changes = self.conn.lock().unwrap().execute(
            "UPDATE consent_records SET granted = 0, revoked_at = ?1
             WHERE user_id = ?2 AND consent_type = ?3 AND granted = 1 AND revoked_at IS NULL",
            params![now, user_id, consent_type.as_str()],
        )?;
        Ok(changes > 0)
    }

    /// Get all consent records for a user.
    pub fn get_consents(&self, user_id: &str) -> Result<Vec<ConsentRecord>, GovernanceError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, user_id, consent_type, granted, granted_at, revoked_at, purpose, legal_basis, version
             FROM consent_records WHERE user_id = ?1 ORDER BY id DESC"
        )?;
        let rows = stmt.query_map(params![user_id], |row| {
            Ok(ConsentRecord {
                id: row.get(0)?,
                user_id: row.get(1)?,
                consent_type: row.get(2)?,
                granted: row.get::<_, i32>(3)? != 0,
                granted_at: row.get(4)?,
                revoked_at: row.get(5)?,
                purpose: row.get(6)?,
                legal_basis: row.get(7)?,
                version: row.get(8)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Check if a specific consent type is currently active.
    pub fn has_active_consent(
        &self,
        user_id: &str,
        consent_type: ConsentType,
    ) -> Result<bool, GovernanceError> {
        let exists: Option<i32> = self
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT 1 FROM consent_records
             WHERE user_id = ?1 AND consent_type = ?2 AND granted = 1 AND revoked_at IS NULL
             LIMIT 1",
                params![user_id, consent_type.as_str()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(exists.is_some())
    }

    // ── Processing Log ──

    /// Log a data processing action (PII types only, never actual values).
    pub fn log_processing(
        &self,
        user_id: &str,
        action: ProcessingAction,
        pii_types: Option<&[&str]>,
        source: Option<&str>,
        details: Option<&str>,
    ) -> Result<i64, GovernanceError> {
        let now = Utc::now().to_rfc3339();
        let pii = pii_types.map(|t| t.join(","));
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO data_processing_log (user_id, timestamp, action, pii_types, source, details)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![user_id, now, action.as_str(), pii, source, details],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Get processing log entries for a user.
    pub fn get_processing_log(
        &self,
        user_id: &str,
        limit: Option<usize>,
    ) -> Result<Vec<ProcessingLogEntry>, GovernanceError> {
        let sql = match limit {
            Some(_) => {
                "SELECT id, user_id, timestamp, action, pii_types, source, details
                        FROM data_processing_log WHERE user_id = ?1 ORDER BY id DESC LIMIT ?2"
            }
            None => {
                "SELECT id, user_id, timestamp, action, pii_types, source, details
                     FROM data_processing_log WHERE user_id = ?1 ORDER BY id DESC"
            }
        };
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(sql)?;
        let rows = match limit {
            Some(lim) => stmt.query_map(params![user_id, lim as i64], map_processing_log)?,
            None => stmt.query_map(params![user_id], map_processing_log)?,
        };
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Get all processing log entries across all users (for breach assessment).
    pub fn get_all_processing_log(
        &self,
        limit: Option<usize>,
    ) -> Result<Vec<ProcessingLogEntry>, GovernanceError> {
        let sql = match limit {
            Some(_) => {
                "SELECT id, user_id, timestamp, action, pii_types, source, details
                        FROM data_processing_log ORDER BY id DESC LIMIT ?1"
            }
            None => {
                "SELECT id, user_id, timestamp, action, pii_types, source, details
                     FROM data_processing_log ORDER BY id DESC"
            }
        };
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(sql)?;
        let rows = match limit {
            Some(lim) => stmt.query_map(params![lim as i64], map_processing_log)?,
            None => stmt.query_map([], map_processing_log)?,
        };
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    // ── DSAR ──

    /// Create a new DSAR request.
    pub fn create_dsar(
        &self,
        user_id: &str,
        request_type: DsarType,
    ) -> Result<DsarRequest, GovernanceError> {
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO dsar_requests (user_id, request_type, requested_at, status)
             VALUES (?1, ?2, ?3, 'pending')",
            params![user_id, request_type.as_str(), now],
        )?;
        let id = conn.last_insert_rowid();
        Ok(DsarRequest {
            id,
            user_id: user_id.to_string(),
            request_type: request_type.as_str().to_string(),
            requested_at: now,
            completed_at: None,
            status: "pending".to_string(),
            response_path: None,
        })
    }

    /// Update DSAR status.
    pub fn update_dsar_status(
        &self,
        dsar_id: i64,
        status: DsarStatus,
        response_path: Option<&str>,
    ) -> Result<bool, GovernanceError> {
        let completed_at = if status == DsarStatus::Completed {
            Some(Utc::now().to_rfc3339())
        } else {
            None
        };
        let changes = self.conn.lock().unwrap().execute(
            "UPDATE dsar_requests SET status = ?1, completed_at = COALESCE(?2, completed_at),
             response_path = COALESCE(?3, response_path)
             WHERE id = ?4",
            params![status.as_str(), completed_at, response_path, dsar_id],
        )?;
        Ok(changes > 0)
    }

    /// Get DSAR requests for a user.
    pub fn get_dsar_requests(&self, user_id: &str) -> Result<Vec<DsarRequest>, GovernanceError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, user_id, request_type, requested_at, completed_at, status, response_path
             FROM dsar_requests WHERE user_id = ?1 ORDER BY id DESC",
        )?;
        let rows = stmt.query_map(params![user_id], |row| {
            Ok(DsarRequest {
                id: row.get(0)?,
                user_id: row.get(1)?,
                request_type: row.get(2)?,
                requested_at: row.get(3)?,
                completed_at: row.get(4)?,
                status: row.get(5)?,
                response_path: row.get(6)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    // ── Composite ──

    /// Get full consent status for a user.
    pub fn get_status(&self, user_id: &str) -> Result<ConsentStatus, GovernanceError> {
        let consents = self.get_consents(user_id)?;
        let conn = self.conn.lock().unwrap();
        let log_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM data_processing_log WHERE user_id = ?1",
            params![user_id],
            |row| row.get(0),
        )?;
        let pending_dsars: i64 = conn.query_row(
            "SELECT COUNT(*) FROM dsar_requests WHERE user_id = ?1 AND status IN ('pending', 'in_progress')",
            params![user_id],
            |row| row.get(0),
        )?;
        Ok(ConsentStatus {
            user_id: user_id.to_string(),
            consents,
            processing_log_count: log_count,
            pending_dsars,
        })
    }

    /// Export all user data (for DSAR access request).
    pub fn export_user_data(&self, user_id: &str) -> Result<PrivacyExport, GovernanceError> {
        Ok(PrivacyExport {
            exported_at: Utc::now().to_rfc3339(),
            user_id: user_id.to_string(),
            consents: self.get_consents(user_id)?,
            processing_log: self.get_processing_log(user_id, None)?,
            dsar_requests: self.get_dsar_requests(user_id)?,
        })
    }

    /// Delete all user data (for DSAR erasure request). Returns total records deleted.
    pub fn delete_user_data(&self, user_id: &str) -> Result<usize, GovernanceError> {
        let conn = self.conn.lock().unwrap();
        let c1 = conn.execute(
            "DELETE FROM consent_records WHERE user_id = ?1",
            params![user_id],
        )?;
        let c2 = conn.execute(
            "DELETE FROM data_processing_log WHERE user_id = ?1",
            params![user_id],
        )?;
        let c3 = conn.execute(
            "DELETE FROM dsar_requests WHERE user_id = ?1",
            params![user_id],
        )?;
        let c4 = conn.execute(
            "DELETE FROM retention_entries WHERE user_id = ?1",
            params![user_id],
        )?;
        Ok(c1 + c2 + c3 + c4)
    }

    // ── Retention ──

    /// Track a processing log entry for retention governance.
    pub fn track_retention(
        &self,
        user_id: &str,
        source_table: &str,
        source_id: i64,
        tier: RetentionTier,
        expires_at: &str,
    ) -> Result<(), GovernanceError> {
        let now = Utc::now().to_rfc3339();
        self.conn.lock().unwrap().execute(
            "INSERT INTO retention_entries (user_id, created_at, tier, expires_at, source_table, source_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![user_id, now, tier.as_str(), expires_at, source_table, source_id],
        )?;
        Ok(())
    }

    /// Get count of retention entries per tier.
    pub fn get_retention_summary(&self) -> Result<RetentionSummary, GovernanceError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT tier, COUNT(*) FROM retention_entries GROUP BY tier")?;
        let mut summary = RetentionSummary::default();
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
        })?;
        for (tier, count) in rows.flatten() {
            match tier.as_str() {
                "hot" => summary.hot = count,
                "warm" => summary.warm = count,
                "cold" => summary.cold = count,
                "expired" => summary.expired = count,
                _ => {}
            }
            summary.total += count;
        }
        Ok(summary)
    }

    /// Get retention entries that should be promoted to the next tier.
    pub fn get_retention_candidates(
        &self,
        tier: RetentionTier,
        now: &str,
    ) -> Result<Vec<RetentionEntry>, GovernanceError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, user_id, created_at, tier, expires_at, source_table, source_id
             FROM retention_entries WHERE tier = ?1 AND expires_at <= ?2 ORDER BY expires_at ASC",
        )?;
        let rows = stmt.query_map(params![tier.as_str(), now], |row| {
            Ok(RetentionEntry {
                id: row.get(0)?,
                user_id: row.get(1)?,
                created_at: row.get(2)?,
                tier: row.get(3)?,
                expires_at: row.get(4)?,
                source_table: row.get(5)?,
                source_id: row.get(6)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Update the tier of a retention entry.
    pub fn update_retention_tier(
        &self,
        id: i64,
        new_tier: RetentionTier,
        new_expires_at: &str,
    ) -> Result<(), GovernanceError> {
        self.conn.lock().unwrap().execute(
            "UPDATE retention_entries SET tier = ?1, expires_at = ?2 WHERE id = ?3",
            params![new_tier.as_str(), new_expires_at, id],
        )?;
        Ok(())
    }

    /// Delete expired retention entries and their source records. Returns count deleted.
    pub fn prune_expired(&self, now: &str) -> Result<usize, GovernanceError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, source_table, source_id FROM retention_entries WHERE tier = 'expired' AND expires_at <= ?1"
        )?;
        let expired: Vec<(i64, String, i64)> = stmt
            .query_map(params![now], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .filter_map(|r| r.ok())
            .collect();

        if expired.is_empty() {
            return Ok(0);
        }

        let mut count = 0;
        for (id, source_table, source_id) in &expired {
            if source_table == "data_processing_log" {
                conn.execute(
                    "DELETE FROM data_processing_log WHERE id = ?1",
                    params![source_id],
                )?;
            }
            conn.execute("DELETE FROM retention_entries WHERE id = ?1", params![id])?;
            count += 1;
        }
        Ok(count)
    }
}

fn map_processing_log(row: &rusqlite::Row) -> rusqlite::Result<ProcessingLogEntry> {
    Ok(ProcessingLogEntry {
        id: row.get(0)?,
        user_id: row.get(1)?,
        timestamp: row.get(2)?,
        action: row.get(3)?,
        pii_types: row.get(4)?,
        source: row.get(5)?,
        details: row.get(6)?,
    })
}

/// Convert processing log entries to audit entries for breach assessment.
pub fn processing_log_to_audit_entries(
    entries: &[ProcessingLogEntry],
) -> Vec<crate::compliance::AuditEntry> {
    entries
        .iter()
        .map(|e| {
            let pii_types_str = e.pii_types.as_deref().unwrap_or("");
            let pii_count = if pii_types_str.is_empty() {
                0u64
            } else {
                pii_types_str.split(',').count() as u64
            };
            let pii_breakdown = if pii_types_str.is_empty() {
                None
            } else {
                // Convert "email,ssn,cc" → "email=1, ssn=1, cc=1"
                let breakdown = pii_types_str
                    .split(',')
                    .map(|t| format!("{}=1", t.trim()))
                    .collect::<Vec<_>>()
                    .join(", ");
                Some(breakdown)
            };
            crate::compliance::AuditEntry {
                timestamp: e.timestamp.clone(),
                module: e.source.clone().unwrap_or_else(|| "governance".to_string()),
                operation: e.action.clone(),
                pii_total: Some(pii_count),
                pii_breakdown,
                request_id: None,
            }
        })
        .collect()
}

// ── FileGuard ──

/// Default deny patterns for sensitive file paths (matches TypeScript file-guard.ts).
const DEFAULT_DENY_PATTERNS: &[&str] = &[
    r"\.env$",
    r"\.env\.\w+$",
    r"credentials\.json$",
    r"\.credentials$",
    r"secret[s]?\.ya?ml$",
    r"secret[s]?\.json$",
    r"\.ssh/(?:id_|known_hosts|authorized_keys)",
    r"\.gnupg/",
    r"\.aws/credentials",
    r"\.aws/config",
    r"\.azure/accessTokens",
    r"\.config/gcloud",
    r"\.npmrc$",
    r"\.pypirc$",
    r"\.sqlite3?$",
    r"\.db$",
    r"(?i)keychain",
    r"credential\.store",
    r"openobscure.*\.enc\.json$",
];

#[derive(Default)]
pub struct FileGuardConfig {
    pub extra_deny: Vec<String>,
    pub allow: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct FileCheckResult {
    pub allowed: bool,
    pub reason: Option<String>,
}

pub struct FileGuard {
    deny_patterns: Vec<Regex>,
    allow_patterns: Vec<Regex>,
}

impl FileGuard {
    pub fn new(config: Option<FileGuardConfig>) -> Self {
        let config = config.unwrap_or_default();

        let mut deny_patterns: Vec<Regex> = DEFAULT_DENY_PATTERNS
            .iter()
            .filter_map(|p| Regex::new(p).ok())
            .collect();

        for pattern in &config.extra_deny {
            if let Ok(re) = Regex::new(pattern) {
                deny_patterns.push(re);
            }
        }

        let allow_patterns: Vec<Regex> = config
            .allow
            .iter()
            .filter_map(|p| Regex::new(p).ok())
            .collect();

        Self {
            deny_patterns,
            allow_patterns,
        }
    }

    /// Check if a file path is allowed for agent access.
    pub fn check_access(&self, path: &str) -> FileCheckResult {
        let normalized = path.replace('\\', "/");

        // Allow list overrides deny
        for pattern in &self.allow_patterns {
            if pattern.is_match(&normalized) {
                return FileCheckResult {
                    allowed: true,
                    reason: None,
                };
            }
        }

        // Check deny patterns
        for pattern in &self.deny_patterns {
            if pattern.is_match(&normalized) {
                return FileCheckResult {
                    allowed: false,
                    reason: Some(format!(
                        "Path matches sensitive pattern: {}",
                        pattern.as_str()
                    )),
                };
            }
        }

        FileCheckResult {
            allowed: true,
            reason: None,
        }
    }
}

// ── RetentionManager ──

#[derive(Debug, Clone)]
pub struct RetentionPolicy {
    pub hot_days: i64,
    pub warm_days: i64,
    pub cold_days: i64,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            hot_days: 7,
            warm_days: 30,
            cold_days: 90,
        }
    }
}

#[derive(Debug, Clone)]
pub struct EnforceResult {
    pub promoted: u32,
    pub pruned: u32,
}

pub struct RetentionManager {
    store: Arc<ConsentStore>,
    policy: RetentionPolicy,
}

impl RetentionManager {
    pub fn new(store: Arc<ConsentStore>, policy: Option<RetentionPolicy>) -> Self {
        Self {
            store,
            policy: policy.unwrap_or_default(),
        }
    }

    /// Run tier promotion + pruning. Returns counts of promoted and pruned entries.
    pub fn enforce(&self, now: Option<&str>) -> Result<EnforceResult, GovernanceError> {
        let current_time = now
            .map(|s| s.to_string())
            .unwrap_or_else(|| Utc::now().to_rfc3339());
        let mut promoted = 0u32;

        // Tier transitions: hot→warm, warm→cold, cold→expired
        let transitions = [
            (
                RetentionTier::Hot,
                RetentionTier::Warm,
                self.policy.warm_days,
            ),
            (
                RetentionTier::Warm,
                RetentionTier::Cold,
                self.policy.cold_days,
            ),
            (
                RetentionTier::Cold,
                RetentionTier::Expired,
                self.policy.cold_days,
            ),
        ];

        for (from, to, days) in &transitions {
            let candidates = self.store.get_retention_candidates(*from, &current_time)?;
            for entry in &candidates {
                let new_expires = (Utc::now() + Duration::days(*days)).to_rfc3339();
                self.store
                    .update_retention_tier(entry.id, *to, &new_expires)?;
                promoted += 1;
            }
        }

        let pruned = self.store.prune_expired(&current_time)? as u32;

        Ok(EnforceResult { promoted, pruned })
    }

    /// Get retention summary.
    pub fn get_summary(&self) -> Result<RetentionSummary, GovernanceError> {
        self.store.get_retention_summary()
    }

    /// Get the current retention policy.
    pub fn get_policy(&self) -> RetentionPolicy {
        self.policy.clone()
    }

    /// Track a new processing log entry in the retention system.
    pub fn track_entry(
        &self,
        user_id: &str,
        source_id: i64,
        now: Option<&str>,
    ) -> Result<(), GovernanceError> {
        let current_time = now
            .map(|s| s.to_string())
            .unwrap_or_else(|| Utc::now().to_rfc3339());
        let expires_at = if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(&current_time) {
            (parsed + Duration::days(self.policy.hot_days)).to_rfc3339()
        } else {
            (Utc::now() + Duration::days(self.policy.hot_days)).to_rfc3339()
        };
        self.store.track_retention(
            user_id,
            "data_processing_log",
            source_id,
            RetentionTier::Hot,
            &expires_at,
        )
    }
}

// ── GovernanceEngine (composite) ──

pub struct GovernanceEngine {
    consent_store: Arc<ConsentStore>,
    file_guard: FileGuard,
    retention: RetentionManager,
}

impl GovernanceEngine {
    #[allow(clippy::arc_with_non_send_sync)]
    pub fn new(
        db_path: &str,
        file_guard_config: Option<FileGuardConfig>,
    ) -> Result<Self, GovernanceError> {
        let consent_store = Arc::new(ConsentStore::open(db_path)?);
        let file_guard = FileGuard::new(file_guard_config);
        let retention = RetentionManager::new(Arc::clone(&consent_store), None);
        Ok(Self {
            consent_store,
            file_guard,
            retention,
        })
    }

    pub fn consent_store(&self) -> &ConsentStore {
        &self.consent_store
    }

    pub fn file_guard(&self) -> &FileGuard {
        &self.file_guard
    }

    pub fn retention(&self) -> &RetentionManager {
        &self.retention
    }
}

// ── Privacy Command Router ──

#[derive(Debug, Clone)]
pub struct PrivacyCommandResult {
    pub text: String,
    pub success: bool,
}

/// AI disclosure text (GDPR Art. 13/14).
pub fn ai_disclosure_text(model_name: &str, provider: &str) -> String {
    format!(
        "Your data will be processed by {} via {}. \
         PII has been encrypted using format-preserving encryption before reaching the AI model. \
         OpenObscure scans requests for personal data (names, addresses, health information, \
         financial data) and applies encryption or redaction to protect your privacy. \
         You can manage your privacy settings with /privacy status, /privacy consent, \
         and /privacy export commands. For more information, see GDPR Articles 13 and 14.",
        model_name, provider
    )
}

/// Route a /privacy command to the appropriate handler.
pub fn handle_privacy_command(
    engine: &GovernanceEngine,
    user_id: &str,
    args: &[&str],
) -> PrivacyCommandResult {
    let subcommand = args.first().map(|s| s.to_lowercase());

    match subcommand.as_deref() {
        Some("status") => cmd_status(engine, user_id),
        Some("consent") => {
            let action = args.get(1).map(|s| s.to_lowercase());
            let consent_type = args.get(2).map(|s| s.to_lowercase());
            match action.as_deref() {
                Some("grant") => cmd_consent_grant(engine, user_id, consent_type.as_deref()),
                Some("revoke") => cmd_consent_revoke(engine, user_id, consent_type.as_deref()),
                _ => PrivacyCommandResult {
                    text: "Usage: /privacy consent <grant|revoke> [type]\nTypes: processing, storage, transfer, ai_disclosure".to_string(),
                    success: false,
                },
            }
        }
        Some("export") => cmd_export(engine, user_id),
        Some("delete") => cmd_delete(engine, user_id),
        Some("disclosure") => {
            let model = args.get(1).copied().unwrap_or("the AI model");
            let provider = args.get(2).copied().unwrap_or("the configured provider");
            PrivacyCommandResult {
                text: ai_disclosure_text(model, provider),
                success: true,
            }
        }
        Some("retention") => {
            let action = args.get(1).map(|s| s.to_lowercase());
            cmd_retention(engine, action.as_deref())
        }
        _ => PrivacyCommandResult {
            text: [
                "OpenObscure Privacy Commands:",
                "  /privacy status             \u{2014} Show consent state and data summary",
                "  /privacy consent grant      \u{2014} Grant consent for data processing",
                "  /privacy consent revoke     \u{2014} Revoke consent",
                "  /privacy export             \u{2014} Export all your personal data",
                "  /privacy delete             \u{2014} Request erasure of all your data",
                "  /privacy disclosure         \u{2014} Show AI model privacy disclosure",
                "  /privacy retention status   \u{2014} Show retention tier counts",
                "  /privacy retention enforce  \u{2014} Run tier promotion + pruning now",
                "  /privacy retention policy   \u{2014} Show current retention policy",
            ]
            .join("\n"),
            success: true,
        },
    }
}

fn cmd_status(engine: &GovernanceEngine, user_id: &str) -> PrivacyCommandResult {
    match engine.consent_store().get_status(user_id) {
        Ok(status) => {
            let mut lines = vec!["OpenObscure Privacy Status".to_string(), String::new()];

            let active: Vec<_> = status
                .consents
                .iter()
                .filter(|c| c.granted && c.revoked_at.is_none())
                .collect();
            let revoked: Vec<_> = status
                .consents
                .iter()
                .filter(|c| !c.granted || c.revoked_at.is_some())
                .collect();

            if !active.is_empty() {
                lines.push("Active Consents:".to_string());
                for c in &active {
                    let basis = c
                        .legal_basis
                        .as_deref()
                        .map(|b| format!(" (basis: {})", b))
                        .unwrap_or_default();
                    let purpose = c
                        .purpose
                        .as_deref()
                        .map(|p| format!(" \u{2014} {}", p))
                        .unwrap_or_default();
                    lines.push(format!(
                        "  [granted] {}{}{} (v{})",
                        c.consent_type, basis, purpose, c.version
                    ));
                }
            } else {
                lines.push("Active Consents: none".to_string());
            }

            if !revoked.is_empty() {
                lines.push("Revoked Consents:".to_string());
                for c in &revoked {
                    let revoked_at = c.revoked_at.as_deref().unwrap_or("unknown");
                    lines.push(format!(
                        "  [revoked] {} (revoked: {})",
                        c.consent_type, revoked_at
                    ));
                }
            }

            lines.push(String::new());
            lines.push(format!(
                "Data Processing Log: {} entries",
                status.processing_log_count
            ));
            lines.push(format!("Pending DSARs: {}", status.pending_dsars));

            PrivacyCommandResult {
                text: lines.join("\n"),
                success: true,
            }
        }
        Err(e) => PrivacyCommandResult {
            text: format!("Error: {}", e),
            success: false,
        },
    }
}

fn cmd_consent_grant(
    engine: &GovernanceEngine,
    user_id: &str,
    consent_type: Option<&str>,
) -> PrivacyCommandResult {
    let ct_str = consent_type.unwrap_or("processing");
    let ct = match ConsentType::from_str(ct_str) {
        Ok(ct) => ct,
        Err(_) => {
            let valid: Vec<_> = ConsentType::all().iter().map(|c| c.as_str()).collect();
            return PrivacyCommandResult {
                text: format!(
                    "Invalid consent type: \"{}\". Valid types: {}",
                    ct_str,
                    valid.join(", ")
                ),
                success: false,
            };
        }
    };

    match engine
        .consent_store()
        .grant_consent(user_id, ct, Some("User-initiated consent"), None)
    {
        Ok(record) => {
            let _ = engine.consent_store().log_processing(
                user_id,
                ProcessingAction::Store,
                Some(&[]),
                Some("consent_manager"),
                Some(&format!(
                    "{{\"action\":\"consent_grant\",\"consent_type\":\"{}\"}}",
                    ct_str
                )),
            );
            PrivacyCommandResult {
                text: format!(
                    "Consent granted for \"{}\" (version {}). You can revoke at any time with /privacy consent revoke {}.",
                    ct_str, record.version, ct_str
                ),
                success: true,
            }
        }
        Err(e) => PrivacyCommandResult {
            text: format!("Error: {}", e),
            success: false,
        },
    }
}

fn cmd_consent_revoke(
    engine: &GovernanceEngine,
    user_id: &str,
    consent_type: Option<&str>,
) -> PrivacyCommandResult {
    let ct_str = consent_type.unwrap_or("processing");
    let ct = match ConsentType::from_str(ct_str) {
        Ok(ct) => ct,
        Err(_) => {
            let valid: Vec<_> = ConsentType::all().iter().map(|c| c.as_str()).collect();
            return PrivacyCommandResult {
                text: format!(
                    "Invalid consent type: \"{}\". Valid types: {}",
                    ct_str,
                    valid.join(", ")
                ),
                success: false,
            };
        }
    };

    match engine.consent_store().revoke_consent(user_id, ct) {
        Ok(true) => {
            let _ = engine.consent_store().log_processing(
                user_id,
                ProcessingAction::Store,
                Some(&[]),
                Some("consent_manager"),
                Some(&format!(
                    "{{\"action\":\"consent_revoke\",\"consent_type\":\"{}\"}}",
                    ct_str
                )),
            );
            PrivacyCommandResult {
                text: format!(
                    "Consent for \"{}\" has been revoked. Non-essential data processing for this category will stop.",
                    ct_str
                ),
                success: true,
            }
        }
        Ok(false) => PrivacyCommandResult {
            text: format!("No active consent for \"{}\" to revoke.", ct_str),
            success: false,
        },
        Err(e) => PrivacyCommandResult {
            text: format!("Error: {}", e),
            success: false,
        },
    }
}

fn cmd_export(engine: &GovernanceEngine, user_id: &str) -> PrivacyCommandResult {
    match engine.consent_store().export_user_data(user_id) {
        Ok(data) => {
            // Create DSAR access request
            let dsar = engine
                .consent_store()
                .create_dsar(user_id, DsarType::Access)
                .ok();
            if let Some(ref d) = dsar {
                let _ =
                    engine
                        .consent_store()
                        .update_dsar_status(d.id, DsarStatus::Completed, None);
            }

            match serde_json::to_string_pretty(&data) {
                Ok(json) => {
                    let dsar_note = dsar
                        .map(|d| format!("\nDSAR request #{} fulfilled.", d.id))
                        .unwrap_or_default();
                    PrivacyCommandResult {
                        text: format!("{}{}", json, dsar_note),
                        success: true,
                    }
                }
                Err(e) => PrivacyCommandResult {
                    text: format!("Serialization error: {}", e),
                    success: false,
                },
            }
        }
        Err(e) => PrivacyCommandResult {
            text: format!("Error: {}", e),
            success: false,
        },
    }
}

fn cmd_delete(engine: &GovernanceEngine, user_id: &str) -> PrivacyCommandResult {
    // Create DSAR erasure request first
    let _ = engine
        .consent_store()
        .create_dsar(user_id, DsarType::Erasure);

    match engine.consent_store().delete_user_data(user_id) {
        Ok(deleted_count) => {
            // Re-create a completion record after deletion
            if let Ok(d) = engine
                .consent_store()
                .create_dsar(user_id, DsarType::Erasure)
            {
                let _ =
                    engine
                        .consent_store()
                        .update_dsar_status(d.id, DsarStatus::Completed, None);
            }
            PrivacyCommandResult {
                text: format!("Data erasure complete. {} records deleted across all tables.\nDSAR erasure request fulfilled.", deleted_count),
                success: true,
            }
        }
        Err(e) => PrivacyCommandResult {
            text: format!("Error: {}", e),
            success: false,
        },
    }
}

fn cmd_retention(engine: &GovernanceEngine, action: Option<&str>) -> PrivacyCommandResult {
    match action {
        Some("status") => match engine.retention().get_summary() {
            Ok(summary) => {
                let lines = [
                    "Retention Tier Summary:".to_string(),
                    format!("  hot:     {} entries", summary.hot),
                    format!("  warm:    {} entries", summary.warm),
                    format!("  cold:    {} entries", summary.cold),
                    format!("  expired: {} entries", summary.expired),
                    format!("  total:   {} entries", summary.total),
                ];
                PrivacyCommandResult {
                    text: lines.join("\n"),
                    success: true,
                }
            }
            Err(e) => PrivacyCommandResult {
                text: format!("Error: {}", e),
                success: false,
            },
        },
        Some("enforce") => match engine.retention().enforce(None) {
            Ok(result) => PrivacyCommandResult {
                text: format!(
                    "Retention enforcement complete. Promoted: {}, Pruned: {}.",
                    result.promoted, result.pruned
                ),
                success: true,
            },
            Err(e) => PrivacyCommandResult {
                text: format!("Error: {}", e),
                success: false,
            },
        },
        Some("policy") => {
            let policy = engine.retention().get_policy();
            let lines = [
                "Retention Policy:".to_string(),
                format!("  hot  \u{2192} warm:    after {} days", policy.hot_days),
                format!("  warm \u{2192} cold:    after {} days", policy.warm_days),
                format!("  cold \u{2192} expired: after {} days", policy.cold_days),
                "  expired: deleted on next enforcement run".to_string(),
            ];
            PrivacyCommandResult {
                text: lines.join("\n"),
                success: true,
            }
        }
        _ => PrivacyCommandResult {
            text: "Usage: /privacy retention <status|enforce|policy>".to_string(),
            success: false,
        },
    }
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store() -> ConsentStore {
        ConsentStore::open(":memory:").unwrap()
    }

    fn test_engine() -> GovernanceEngine {
        GovernanceEngine::new(":memory:", None).unwrap()
    }

    // ── ConsentStore Tests ──

    #[test]
    fn test_grant_consent() {
        let store = test_store();
        let record = store
            .grant_consent("user1", ConsentType::Processing, Some("Test"), None)
            .unwrap();
        assert_eq!(record.consent_type, "processing");
        assert!(record.granted);
        assert_eq!(record.version, 1);
        assert_eq!(record.purpose.as_deref(), Some("Test"));
    }

    #[test]
    fn test_grant_consent_bumps_version() {
        let store = test_store();
        let r1 = store
            .grant_consent("user1", ConsentType::Processing, None, None)
            .unwrap();
        assert_eq!(r1.version, 1);
        let r2 = store
            .grant_consent("user1", ConsentType::Processing, None, None)
            .unwrap();
        assert_eq!(r2.version, 2);
    }

    #[test]
    fn test_revoke_consent() {
        let store = test_store();
        store
            .grant_consent("user1", ConsentType::Storage, None, None)
            .unwrap();
        assert!(store
            .has_active_consent("user1", ConsentType::Storage)
            .unwrap());

        let revoked = store.revoke_consent("user1", ConsentType::Storage).unwrap();
        assert!(revoked);
        assert!(!store
            .has_active_consent("user1", ConsentType::Storage)
            .unwrap());
    }

    #[test]
    fn test_revoke_nonexistent() {
        let store = test_store();
        let revoked = store
            .revoke_consent("user1", ConsentType::Transfer)
            .unwrap();
        assert!(!revoked);
    }

    #[test]
    fn test_get_consents() {
        let store = test_store();
        store
            .grant_consent("user1", ConsentType::Processing, None, None)
            .unwrap();
        store
            .grant_consent("user1", ConsentType::Storage, None, None)
            .unwrap();
        let consents = store.get_consents("user1").unwrap();
        assert_eq!(consents.len(), 2);
    }

    #[test]
    fn test_has_active_consent() {
        let store = test_store();
        assert!(!store
            .has_active_consent("user1", ConsentType::Processing)
            .unwrap());
        store
            .grant_consent("user1", ConsentType::Processing, None, None)
            .unwrap();
        assert!(store
            .has_active_consent("user1", ConsentType::Processing)
            .unwrap());
    }

    #[test]
    fn test_log_processing() {
        let store = test_store();
        let id = store
            .log_processing(
                "user1",
                ProcessingAction::Scan,
                Some(&["email", "phone"]),
                Some("proxy"),
                None,
            )
            .unwrap();
        assert!(id > 0);

        let log = store.get_processing_log("user1", None).unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].action, "scan");
        assert_eq!(log[0].pii_types.as_deref(), Some("email,phone"));
    }

    #[test]
    fn test_processing_log_limit() {
        let store = test_store();
        for _ in 0..5 {
            store
                .log_processing("user1", ProcessingAction::Scan, None, None, None)
                .unwrap();
        }
        let log = store.get_processing_log("user1", Some(3)).unwrap();
        assert_eq!(log.len(), 3);
    }

    #[test]
    fn test_dsar_lifecycle() {
        let store = test_store();
        let dsar = store.create_dsar("user1", DsarType::Access).unwrap();
        assert_eq!(dsar.status, "pending");

        store
            .update_dsar_status(dsar.id, DsarStatus::Completed, None)
            .unwrap();
        let requests = store.get_dsar_requests("user1").unwrap();
        assert_eq!(requests[0].status, "completed");
        assert!(requests[0].completed_at.is_some());
    }

    #[test]
    fn test_get_status() {
        let store = test_store();
        store
            .grant_consent("user1", ConsentType::Processing, None, None)
            .unwrap();
        store
            .log_processing("user1", ProcessingAction::Scan, None, None, None)
            .unwrap();
        store.create_dsar("user1", DsarType::Access).unwrap();

        let status = store.get_status("user1").unwrap();
        assert_eq!(status.consents.len(), 1);
        assert_eq!(status.processing_log_count, 1);
        assert_eq!(status.pending_dsars, 1);
    }

    #[test]
    fn test_export_user_data() {
        let store = test_store();
        store
            .grant_consent("user1", ConsentType::Processing, None, None)
            .unwrap();
        store
            .log_processing("user1", ProcessingAction::Encrypt, None, None, None)
            .unwrap();

        let export = store.export_user_data("user1").unwrap();
        assert_eq!(export.user_id, "user1");
        assert_eq!(export.consents.len(), 1);
        assert_eq!(export.processing_log.len(), 1);
    }

    #[test]
    fn test_delete_user_data() {
        let store = test_store();
        store
            .grant_consent("user1", ConsentType::Processing, None, None)
            .unwrap();
        store
            .log_processing("user1", ProcessingAction::Scan, None, None, None)
            .unwrap();
        store.create_dsar("user1", DsarType::Access).unwrap();

        let deleted = store.delete_user_data("user1").unwrap();
        assert!(deleted >= 3);

        let status = store.get_status("user1").unwrap();
        assert_eq!(status.consents.len(), 0);
        assert_eq!(status.processing_log_count, 0);
    }

    // ── FileGuard Tests ──

    #[test]
    fn test_file_guard_deny_env() {
        let guard = FileGuard::new(None);
        let result = guard.check_access("/home/user/project/.env");
        assert!(!result.allowed);
        assert!(result.reason.unwrap().contains("sensitive pattern"));
    }

    #[test]
    fn test_file_guard_deny_env_variants() {
        let guard = FileGuard::new(None);
        assert!(!guard.check_access("/project/.env.production").allowed);
        assert!(!guard.check_access("/project/.env.local").allowed);
    }

    #[test]
    fn test_file_guard_deny_ssh_key() {
        let guard = FileGuard::new(None);
        assert!(!guard.check_access("/home/user/.ssh/id_rsa").allowed);
        assert!(!guard.check_access("/home/user/.ssh/id_ed25519").allowed);
        assert!(
            !guard
                .check_access("/home/user/.ssh/authorized_keys")
                .allowed
        );
    }

    #[test]
    fn test_file_guard_deny_aws() {
        let guard = FileGuard::new(None);
        assert!(!guard.check_access("/home/user/.aws/credentials").allowed);
        assert!(!guard.check_access("/home/user/.aws/config").allowed);
    }

    #[test]
    fn test_file_guard_deny_credentials_json() {
        let guard = FileGuard::new(None);
        assert!(!guard.check_access("/project/credentials.json").allowed);
        assert!(!guard.check_access("/project/secrets.yml").allowed);
        assert!(!guard.check_access("/project/secrets.json").allowed);
    }

    #[test]
    fn test_file_guard_allow_normal_files() {
        let guard = FileGuard::new(None);
        assert!(guard.check_access("/home/user/project/src/main.rs").allowed);
        assert!(guard.check_access("/home/user/project/README.md").allowed);
        assert!(guard.check_access("/home/user/project/Cargo.toml").allowed);
    }

    #[test]
    fn test_file_guard_extra_deny() {
        let config = FileGuardConfig {
            extra_deny: vec![r"\.secret_stuff$".to_string()],
            allow: vec![],
        };
        let guard = FileGuard::new(Some(config));
        assert!(!guard.check_access("/project/data.secret_stuff").allowed);
        assert!(guard.check_access("/project/data.txt").allowed);
    }

    #[test]
    fn test_file_guard_allow_override() {
        let config = FileGuardConfig {
            extra_deny: vec![],
            allow: vec![r"allowed\.env$".to_string()],
        };
        let guard = FileGuard::new(Some(config));
        // The allow pattern matches first
        assert!(guard.check_access("/project/allowed.env").allowed);
        // Regular .env still denied
        assert!(!guard.check_access("/project/.env").allowed);
    }

    #[test]
    fn test_file_guard_windows_backslashes() {
        let guard = FileGuard::new(None);
        assert!(!guard.check_access("C:\\Users\\user\\.ssh\\id_rsa").allowed);
        assert!(!guard.check_access("C:\\project\\.env").allowed);
    }

    // ── RetentionManager Tests ──

    #[test]
    fn test_retention_track_and_summary() {
        let store = Arc::new(test_store());
        let rm = RetentionManager::new(Arc::clone(&store), None);

        let log_id = store
            .log_processing("user1", ProcessingAction::Scan, None, None, None)
            .unwrap();
        rm.track_entry("user1", log_id, None).unwrap();

        let summary = rm.get_summary().unwrap();
        assert_eq!(summary.hot, 1);
        assert_eq!(summary.total, 1);
    }

    #[test]
    fn test_retention_enforce_promotion() {
        let store = Arc::new(test_store());
        let rm = RetentionManager::new(Arc::clone(&store), None);

        // Create an entry with an already-expired hot tier
        let past = "2020-01-01T00:00:00+00:00";
        store
            .track_retention("user1", "data_processing_log", 1, RetentionTier::Hot, past)
            .unwrap();

        let result = rm.enforce(None).unwrap();
        assert!(result.promoted >= 1);

        let summary = rm.get_summary().unwrap();
        // Should have been promoted from hot to warm
        assert_eq!(summary.hot, 0);
        assert_eq!(summary.warm, 1);
    }

    #[test]
    fn test_retention_prune_expired() {
        let store = Arc::new(test_store());
        let rm = RetentionManager::new(Arc::clone(&store), None);

        // Insert a processing log entry
        let log_id = store
            .log_processing("user1", ProcessingAction::Scan, None, None, None)
            .unwrap();
        // Track it as already expired
        let past = "2020-01-01T00:00:00+00:00";
        store
            .track_retention(
                "user1",
                "data_processing_log",
                log_id,
                RetentionTier::Expired,
                past,
            )
            .unwrap();

        let result = rm.enforce(None).unwrap();
        assert_eq!(result.pruned, 1);

        // Processing log entry should be deleted
        let log = store.get_processing_log("user1", None).unwrap();
        assert_eq!(log.len(), 0);
    }

    #[test]
    fn test_retention_default_policy() {
        let store = Arc::new(test_store());
        let rm = RetentionManager::new(Arc::clone(&store), None);
        let policy = rm.get_policy();
        assert_eq!(policy.hot_days, 7);
        assert_eq!(policy.warm_days, 30);
        assert_eq!(policy.cold_days, 90);
    }

    #[test]
    fn test_retention_custom_policy() {
        let store = Arc::new(test_store());
        let policy = RetentionPolicy {
            hot_days: 1,
            warm_days: 7,
            cold_days: 30,
        };
        let rm = RetentionManager::new(Arc::clone(&store), Some(policy));
        let p = rm.get_policy();
        assert_eq!(p.hot_days, 1);
        assert_eq!(p.warm_days, 7);
        assert_eq!(p.cold_days, 30);
    }

    // ── GovernanceEngine Tests ──

    #[test]
    fn test_engine_creation() {
        let engine = test_engine();
        assert!(engine
            .consent_store()
            .get_consents("nobody")
            .unwrap()
            .is_empty());
        assert!(engine.file_guard().check_access("/safe/file.rs").allowed);
    }

    // ── Privacy Command Tests ──

    #[test]
    fn test_cmd_help() {
        let engine = test_engine();
        let result = handle_privacy_command(&engine, "user1", &[]);
        assert!(result.success);
        assert!(result.text.contains("Privacy Commands"));
    }

    #[test]
    fn test_cmd_status_empty() {
        let engine = test_engine();
        let result = handle_privacy_command(&engine, "user1", &["status"]);
        assert!(result.success);
        assert!(result.text.contains("Active Consents: none"));
    }

    #[test]
    fn test_cmd_consent_grant_and_status() {
        let engine = test_engine();
        let grant = handle_privacy_command(&engine, "user1", &["consent", "grant", "processing"]);
        assert!(grant.success);
        assert!(grant.text.contains("Consent granted"));

        let status = handle_privacy_command(&engine, "user1", &["status"]);
        assert!(status.text.contains("[granted] processing"));
    }

    #[test]
    fn test_cmd_consent_revoke() {
        let engine = test_engine();
        handle_privacy_command(&engine, "user1", &["consent", "grant", "storage"]);
        let revoke = handle_privacy_command(&engine, "user1", &["consent", "revoke", "storage"]);
        assert!(revoke.success);
        assert!(revoke.text.contains("revoked"));
    }

    #[test]
    fn test_cmd_consent_invalid_type() {
        let engine = test_engine();
        let result = handle_privacy_command(&engine, "user1", &["consent", "grant", "invalid"]);
        assert!(!result.success);
        assert!(result.text.contains("Invalid consent type"));
    }

    #[test]
    fn test_cmd_export() {
        let engine = test_engine();
        engine
            .consent_store()
            .grant_consent("user1", ConsentType::Processing, None, None)
            .unwrap();
        let result = handle_privacy_command(&engine, "user1", &["export"]);
        assert!(result.success);
        assert!(result.text.contains("user1"));
        assert!(result.text.contains("processing"));
    }

    #[test]
    fn test_cmd_delete() {
        let engine = test_engine();
        engine
            .consent_store()
            .grant_consent("user1", ConsentType::Processing, None, None)
            .unwrap();
        let result = handle_privacy_command(&engine, "user1", &["delete"]);
        assert!(result.success);
        assert!(result.text.contains("erasure complete"));
    }

    #[test]
    fn test_cmd_disclosure() {
        let engine = test_engine();
        let result =
            handle_privacy_command(&engine, "user1", &["disclosure", "Claude", "Anthropic"]);
        assert!(result.success);
        assert!(result.text.contains("Claude"));
        assert!(result.text.contains("Anthropic"));
    }

    #[test]
    fn test_cmd_retention_status() {
        let engine = test_engine();
        let result = handle_privacy_command(&engine, "user1", &["retention", "status"]);
        assert!(result.success);
        assert!(result.text.contains("total:"));
    }

    #[test]
    fn test_cmd_retention_policy() {
        let engine = test_engine();
        let result = handle_privacy_command(&engine, "user1", &["retention", "policy"]);
        assert!(result.success);
        assert!(result.text.contains("7 days"));
        assert!(result.text.contains("30 days"));
        assert!(result.text.contains("90 days"));
    }

    #[test]
    fn test_cmd_retention_enforce() {
        let engine = test_engine();
        let result = handle_privacy_command(&engine, "user1", &["retention", "enforce"]);
        assert!(result.success);
        assert!(result.text.contains("Promoted:"));
    }

    // ── Processing Log + Audit Entry Converter Tests ──

    #[test]
    fn test_get_all_processing_log_empty() {
        let store = test_store();
        let entries = store.get_all_processing_log(None).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn test_get_all_processing_log_multiple_users() {
        let store = test_store();
        store
            .log_processing(
                "alice",
                ProcessingAction::Scan,
                Some(&["email"]),
                Some("proxy"),
                None,
            )
            .unwrap();
        store
            .log_processing(
                "bob",
                ProcessingAction::Encrypt,
                Some(&["ssn", "cc"]),
                Some("proxy"),
                None,
            )
            .unwrap();
        store
            .log_processing(
                "alice",
                ProcessingAction::Redact,
                Some(&["phone"]),
                Some("plugin"),
                None,
            )
            .unwrap();

        let all = store.get_all_processing_log(None).unwrap();
        assert_eq!(all.len(), 3);

        let limited = store.get_all_processing_log(Some(2)).unwrap();
        assert_eq!(limited.len(), 2);
    }

    #[test]
    fn test_processing_log_to_audit_entries_conversion() {
        let store = test_store();
        store
            .log_processing(
                "user1",
                ProcessingAction::Scan,
                Some(&["email", "ssn"]),
                Some("proxy"),
                None,
            )
            .unwrap();
        store
            .log_processing("user1", ProcessingAction::Encrypt, None, None, None)
            .unwrap();

        let log = store.get_all_processing_log(None).unwrap();
        let audit = processing_log_to_audit_entries(&log);
        assert_eq!(audit.len(), 2);

        // Entry with PII types
        let with_pii = audit.iter().find(|e| e.operation == "scan").unwrap();
        assert_eq!(with_pii.module, "proxy");
        assert_eq!(with_pii.pii_total, Some(2));
        assert!(with_pii.pii_breakdown.as_ref().unwrap().contains("email=1"));
        assert!(with_pii.pii_breakdown.as_ref().unwrap().contains("ssn=1"));

        // Entry without PII types
        let without = audit.iter().find(|e| e.operation == "encrypt").unwrap();
        assert_eq!(without.module, "governance"); // fallback when source is None
        assert_eq!(without.pii_total, Some(0));
        assert!(without.pii_breakdown.is_none());
    }
}
