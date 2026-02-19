use std::path::PathBuf;

use clap::Subcommand;

use crate::config::AppConfig;

/// Compliance CLI subcommands (GDPR Art. 30, 33, 35).
#[derive(Subcommand, Debug)]
pub enum ComplianceCommands {
    /// Print processing activity summary from audit log
    Summary {
        /// Show detailed per-type breakdown
        #[arg(long)]
        verbose: bool,
    },
    /// Generate GDPR Art. 30 Record of Processing Activities
    Ropa {
        /// Output format: "markdown" (default) or "json"
        #[arg(long, default_value = "markdown")]
        format: String,
        /// Write output to file instead of stdout
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Generate GDPR Art. 35 Data Protection Impact Assessment
    Dpia {
        /// Output format: "markdown" (default) or "json"
        #[arg(long, default_value = "markdown")]
        format: String,
        /// Write output to file instead of stdout
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Query and filter the GDPR audit log
    AuditLog {
        /// Show entries since this ISO 8601 date (e.g. 2026-02-17)
        #[arg(long)]
        since: Option<String>,
        /// Show entries until this ISO 8601 date
        #[arg(long)]
        until: Option<String>,
        /// Filter by operation type (e.g. "scan", "encrypt")
        #[arg(long)]
        operation: Option<String>,
        /// Maximum number of entries to show
        #[arg(long, default_value = "50")]
        limit: usize,
    },
    /// Check audit log for anomalies and generate breach assessment
    BreachCheck {
        /// Anomaly score threshold in standard deviations (default: 3.0)
        #[arg(long)]
        threshold: Option<f64>,
        /// Generate GDPR Art. 33 breach notification draft
        #[arg(long)]
        generate_notification: bool,
    },
    /// Export audit log in SIEM format (CEF/LEEF)
    Export {
        /// Export format: "cef" (default) or "leef"
        #[arg(long, default_value = "cef")]
        format: String,
        /// Write output to file instead of stdout
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Export entries since this ISO 8601 date
        #[arg(long)]
        since: Option<String>,
    },
}

/// Run a compliance CLI subcommand.
pub fn run(command: ComplianceCommands, config: &AppConfig) -> anyhow::Result<()> {
    let audit_path = config.logging.audit_log_path.as_deref();

    match command {
        ComplianceCommands::Summary { verbose } => cmd_summary(audit_path, config, verbose),
        ComplianceCommands::Ropa { format, output } => {
            cmd_ropa(audit_path, config, &format, output.as_deref())
        }
        ComplianceCommands::Dpia { format, output } => {
            cmd_dpia(audit_path, config, &format, output.as_deref())
        }
        ComplianceCommands::AuditLog {
            since,
            until,
            operation,
            limit,
        } => cmd_audit_log(
            audit_path,
            &AuditFilter {
                since,
                until,
                operation,
                limit,
            },
        ),
        ComplianceCommands::BreachCheck {
            threshold,
            generate_notification,
        } => cmd_breach_check(
            audit_path,
            config,
            threshold.unwrap_or(3.0),
            generate_notification,
        ),
        ComplianceCommands::Export {
            format,
            output,
            since,
        } => cmd_export(audit_path, &format, output.as_deref(), since.as_deref()),
    }
}

// ---------------------------------------------------------------------------
// Audit log parsing
// ---------------------------------------------------------------------------

/// A parsed audit log entry.
#[derive(Debug, Clone)]
pub struct AuditEntry {
    pub timestamp: String,
    pub module: String,
    pub operation: String,
    pub pii_total: Option<u64>,
    pub pii_breakdown: Option<String>,
    pub request_id: Option<String>,
}

/// Filter criteria for audit log queries.
pub struct AuditFilter {
    pub since: Option<String>,
    pub until: Option<String>,
    pub operation: Option<String>,
    pub limit: usize,
}

/// Parse audit log JSONL file, applying filter criteria.
pub fn parse_audit_log(
    path: Option<&str>,
    filter: &AuditFilter,
) -> anyhow::Result<Vec<AuditEntry>> {
    let path = path.ok_or_else(|| {
        anyhow::anyhow!(
            "No audit log path configured. Set logging.audit_log_path in openobscure.toml"
        )
    })?;

    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Vec::new());
        }
        Err(e) => return Err(e.into()),
    };

    let mut entries = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let obj: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue, // skip malformed lines
        };

        let timestamp = obj
            .get("timestamp")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();

        // Apply date filters
        if let Some(ref since) = filter.since {
            if timestamp < *since {
                continue;
            }
        }
        if let Some(ref until) = filter.until {
            if timestamp > *until {
                continue;
            }
        }

        let operation = obj
            .get("fields")
            .and_then(|f| f.get("operation"))
            .and_then(|v| v.as_str())
            .or_else(|| obj.get("operation").and_then(|v| v.as_str()))
            .unwrap_or_default()
            .to_string();

        if let Some(ref op_filter) = filter.operation {
            if operation != *op_filter {
                continue;
            }
        }

        let module = obj
            .get("fields")
            .and_then(|f| f.get("oo_module"))
            .and_then(|v| v.as_str())
            .or_else(|| obj.get("oo_module").and_then(|v| v.as_str()))
            .unwrap_or_default()
            .to_string();

        let pii_total = obj
            .get("fields")
            .and_then(|f| f.get("pii_total"))
            .and_then(|v| v.as_u64())
            .or_else(|| obj.get("pii_total").and_then(|v| v.as_u64()));

        let pii_breakdown = obj
            .get("fields")
            .and_then(|f| f.get("pii_breakdown"))
            .and_then(|v| v.as_str())
            .or_else(|| obj.get("pii_breakdown").and_then(|v| v.as_str()))
            .map(|s| s.to_string());

        let request_id = obj
            .get("fields")
            .and_then(|f| f.get("request_id"))
            .and_then(|v| v.as_str())
            .or_else(|| obj.get("request_id").and_then(|v| v.as_str()))
            .map(|s| s.to_string());

        entries.push(AuditEntry {
            timestamp,
            module,
            operation,
            pii_total,
            pii_breakdown,
            request_id,
        });

        if entries.len() >= filter.limit {
            break;
        }
    }

    Ok(entries)
}

// ---------------------------------------------------------------------------
// Subcommand handlers
// ---------------------------------------------------------------------------

fn cmd_summary(audit_path: Option<&str>, config: &AppConfig, verbose: bool) -> anyhow::Result<()> {
    let entries = parse_audit_log(
        audit_path,
        &AuditFilter {
            since: None,
            until: None,
            operation: None,
            limit: usize::MAX,
        },
    )?;

    let total_entries = entries.len();
    let total_pii: u64 = entries.iter().filter_map(|e| e.pii_total).sum();
    let scan_count = entries.iter().filter(|e| e.operation == "scan").count();
    let encrypt_count = entries.iter().filter(|e| e.operation == "encrypt").count();

    // Aggregate PII breakdown
    let mut pii_type_counts: std::collections::HashMap<String, u64> =
        std::collections::HashMap::new();
    for entry in &entries {
        if let Some(ref breakdown) = entry.pii_breakdown {
            for pair in breakdown.split(", ") {
                if let Some((pii_type, count_str)) = pair.split_once('=') {
                    if let Ok(count) = count_str.parse::<u64>() {
                        *pii_type_counts.entry(pii_type.to_string()).or_default() += count;
                    }
                }
            }
        }
    }

    println!("OpenObscure Compliance Summary");
    println!("============================");
    if let Some(ref org) = config.compliance.organization_name {
        println!("Organization: {}", org);
    }
    println!("Audit log entries: {}", total_entries);
    println!("PII detections:    {}", total_pii);
    println!("Scan operations:   {}", scan_count);
    println!("Encrypt operations: {}", encrypt_count);
    println!("Providers configured: {}", config.providers.len());
    for name in config.providers.keys() {
        println!("  - {}", name);
    }

    if verbose && !pii_type_counts.is_empty() {
        println!("\nPII Type Breakdown:");
        let mut sorted: Vec<_> = pii_type_counts.into_iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(&a.1));
        for (pii_type, count) in sorted {
            println!("  {}: {}", pii_type, count);
        }
    }

    if !verbose && total_pii > 0 {
        println!("\n(Use --verbose for per-type PII breakdown)");
    }

    Ok(())
}

fn cmd_ropa(
    audit_path: Option<&str>,
    config: &AppConfig,
    format: &str,
    output: Option<&std::path::Path>,
) -> anyhow::Result<()> {
    let entries = parse_audit_log(
        audit_path,
        &AuditFilter {
            since: None,
            until: None,
            operation: None,
            limit: usize::MAX,
        },
    )?;

    let content = match format {
        "json" => {
            let json = generate_ropa_json(&entries, config);
            serde_json::to_string_pretty(&json)?
        }
        _ => generate_ropa_markdown(&entries, config),
    };

    write_output(&content, output)
}

fn cmd_dpia(
    audit_path: Option<&str>,
    config: &AppConfig,
    format: &str,
    output: Option<&std::path::Path>,
) -> anyhow::Result<()> {
    let entries = parse_audit_log(
        audit_path,
        &AuditFilter {
            since: None,
            until: None,
            operation: None,
            limit: usize::MAX,
        },
    )?;

    let content = match format {
        "json" => {
            let json = generate_dpia_json(&entries, config);
            serde_json::to_string_pretty(&json)?
        }
        _ => generate_dpia_markdown(&entries, config),
    };

    write_output(&content, output)
}

fn cmd_audit_log(audit_path: Option<&str>, filter: &AuditFilter) -> anyhow::Result<()> {
    let entries = parse_audit_log(audit_path, filter)?;

    if entries.is_empty() {
        println!("No audit log entries found matching criteria.");
        return Ok(());
    }

    println!(
        "{:<24} {:<16} {:<12} {:<8} REQUEST_ID",
        "TIMESTAMP", "MODULE", "OPERATION", "PII"
    );
    println!("{}", "-".repeat(76));
    for entry in &entries {
        println!(
            "{:<24} {:<16} {:<12} {:<8} {}",
            entry.timestamp,
            entry.module,
            entry.operation,
            entry
                .pii_total
                .map(|n| n.to_string())
                .unwrap_or_else(|| "-".to_string()),
            entry.request_id.as_deref().unwrap_or("-"),
        );
    }
    println!("\n{} entries shown.", entries.len());

    Ok(())
}

fn cmd_breach_check(
    audit_path: Option<&str>,
    config: &AppConfig,
    threshold: f64,
    generate_notification: bool,
) -> anyhow::Result<()> {
    let entries = parse_audit_log(
        audit_path,
        &AuditFilter {
            since: None,
            until: None,
            operation: None,
            limit: usize::MAX,
        },
    )?;

    let assessment = crate::breach_detect::assess_breach(&entries, threshold);

    println!("Breach Assessment");
    println!("=================");
    println!("Risk level: {:?}", assessment.risk_level);
    println!("Anomalies detected: {}", assessment.anomalies.len());
    println!("Recommendation: {}", assessment.recommendation);

    if !assessment.anomalies.is_empty() {
        println!("\nAnomalous Periods:");
        println!(
            "{:<20} {:<10} {:<10} {:<10} TYPES",
            "HOUR", "PII_COUNT", "EXPECTED", "SIGMA"
        );
        for anomaly in &assessment.anomalies {
            println!(
                "{:<20} {:<10} {:<10.1} {:<10.1} {}",
                anomaly.hour,
                anomaly.pii_count,
                anomaly.expected_mean,
                anomaly.sigma_deviation,
                anomaly.pii_types.join(", "),
            );
        }
    }

    if generate_notification {
        let notification =
            crate::breach_detect::generate_art33_notification(&assessment, &config.compliance);
        println!("\n{}", notification);
    }

    Ok(())
}

fn cmd_export(
    audit_path: Option<&str>,
    format: &str,
    output: Option<&std::path::Path>,
    since: Option<&str>,
) -> anyhow::Result<()> {
    let entries = parse_audit_log(
        audit_path,
        &AuditFilter {
            since: since.map(|s| s.to_string()),
            until: None,
            operation: None,
            limit: usize::MAX,
        },
    )?;

    let lines: Vec<String> = entries
        .iter()
        .map(|e| match format {
            "leef" => format_leef_line(e),
            _ => format_cef_line(e),
        })
        .collect();

    let content = lines.join("\n");
    write_output(&content, output)
}

// ---------------------------------------------------------------------------
// ROPA generator (GDPR Art. 30)
// ---------------------------------------------------------------------------

fn generate_ropa_markdown(entries: &[AuditEntry], config: &AppConfig) -> String {
    let now = now_iso8601();
    let org = config
        .compliance
        .organization_name
        .as_deref()
        .unwrap_or("[Organization Name]");
    let dpo = config
        .compliance
        .dpo_email
        .as_deref()
        .unwrap_or("[Not configured]");
    let providers: Vec<&str> = config.providers.keys().map(|s| s.as_str()).collect();
    let total_pii: u64 = entries.iter().filter_map(|e| e.pii_total).sum();
    let pii_types = aggregate_pii_types(entries);

    let mut md = String::new();
    md.push_str("# Record of Processing Activities (GDPR Art. 30)\n\n");
    md.push_str(&format!("**Generated:** {}\n", now));
    md.push_str(&format!("**Organization:** {}\n", org));
    md.push_str(&format!("**Data Protection Officer:** {}\n\n", dpo));

    md.push_str("## 1. Controller Information\n\n");
    md.push_str("| Field | Value |\n|-------|-------|\n");
    md.push_str(&format!("| Organization | {} |\n", org));
    md.push_str(&format!("| DPO Contact | {} |\n", dpo));
    if let Some(ref dpa) = config.compliance.dpa_contact {
        md.push_str(&format!("| Supervisory Authority | {} |\n", dpa));
    }
    md.push_str(&format!(
        "| Retention Period | {} days |\n\n",
        config.compliance.retention_days
    ));

    md.push_str("## 2. Processing Activities\n\n");
    md.push_str(
        "| Activity | Legal Basis | Data Categories | Recipients | Retention |\n\
         |----------|-------------|-----------------|------------|-----------|\n\
         | LLM API relay | Legitimate interest | Chat messages, tool results | ",
    );
    md.push_str(&providers.join(", "));
    md.push_str(" | Session duration |\n");
    md.push_str(
        "| PII encryption (FPE) | Legal obligation (GDPR Art. 32) | SSN, CC, phone, email, names | Encrypted locally | Until session ends |\n",
    );
    if config.image.enabled {
        md.push_str(
            "| Image processing | Legitimate interest | Photos, screenshots | Processed locally | Not retained |\n",
        );
    }
    md.push('\n');

    md.push_str("## 3. PII Statistics (from audit log)\n\n");
    md.push_str(&format!("- Total audit log entries: {}\n", entries.len()));
    md.push_str(&format!("- Total PII detections: {}\n", total_pii));
    if !pii_types.is_empty() {
        md.push_str("- PII breakdown:\n");
        for (pii_type, count) in &pii_types {
            md.push_str(&format!("  - {}: {}\n", pii_type, count));
        }
    }
    md.push('\n');

    md.push_str("## 4. Technical Safeguards\n\n");
    md.push_str("- FF1 format-preserving encryption (AES-256) for structured PII\n");
    md.push_str(
        "- Per-record unique tweaks (UUID + JSON path hash) preventing frequency analysis\n",
    );
    md.push_str("- OS keychain key storage (with env var fallback for headless environments)\n");
    md.push_str("- Localhost-only binding (127.0.0.1) — not network-accessible\n");
    md.push_str("- Health endpoint authentication (X-OpenObscure-Token)\n");
    md.push_str("- PII scrubbing in log output (defense-in-depth)\n");
    md.push_str("- Audit log captures encrypted labels only, never plaintext PII\n");
    if config.image.enabled {
        md.push_str("- Face detection (BlazeFace) + Gaussian blur for biometric PII\n");
        md.push_str("- OCR text detection (PaddleOCR) for text in images\n");
        md.push_str("- EXIF metadata stripping from all processed images\n");
    }
    md.push('\n');

    md.push_str("## 5. Data Transfers\n\n");
    md.push_str(
        "PII is encrypted via FPE before transmission to LLM providers. \
         The following providers receive encrypted (not plaintext) data:\n\n",
    );
    for (name, provider) in &config.providers {
        md.push_str(&format!("- **{}**: `{}`\n", name, provider.upstream_url));
    }
    md.push_str("\n---\n*This document was auto-generated by OpenObscure Compliance CLI.*\n");

    md
}

fn generate_ropa_json(entries: &[AuditEntry], config: &AppConfig) -> serde_json::Value {
    let total_pii: u64 = entries.iter().filter_map(|e| e.pii_total).sum();
    let pii_types = aggregate_pii_types(entries);
    let providers: Vec<serde_json::Value> = config
        .providers
        .iter()
        .map(|(name, p)| {
            serde_json::json!({
                "name": name,
                "upstream_url": p.upstream_url,
            })
        })
        .collect();

    serde_json::json!({
        "title": "Record of Processing Activities (GDPR Art. 30)",
        "generated": now_iso8601(),
        "organization": config.compliance.organization_name,
        "dpo_email": config.compliance.dpo_email,
        "dpa_contact": config.compliance.dpa_contact,
        "retention_days": config.compliance.retention_days,
        "providers": providers,
        "statistics": {
            "audit_entries": entries.len(),
            "total_pii_detections": total_pii,
            "pii_breakdown": pii_types.into_iter().collect::<std::collections::HashMap<_,_>>(),
        },
        "safeguards": [
            "FF1 format-preserving encryption (AES-256)",
            "Per-record unique tweaks",
            "OS keychain key storage",
            "Localhost-only binding",
            "Health endpoint authentication",
            "PII scrubbing in logs",
        ],
    })
}

// ---------------------------------------------------------------------------
// DPIA generator (GDPR Art. 35)
// ---------------------------------------------------------------------------

fn generate_dpia_markdown(entries: &[AuditEntry], config: &AppConfig) -> String {
    let now = now_iso8601();
    let org = config
        .compliance
        .organization_name
        .as_deref()
        .unwrap_or("[Organization Name]");
    let total_pii: u64 = entries.iter().filter_map(|e| e.pii_total).sum();

    let mut md = String::new();
    md.push_str("# Data Protection Impact Assessment (GDPR Art. 35)\n\n");
    md.push_str(&format!("**Generated:** {}\n", now));
    md.push_str(&format!("**Organization:** {}\n\n", org));

    md.push_str("## 1. Description of Processing\n\n");
    md.push_str(
        "OpenObscure is a privacy firewall for AI agents. \
         It intercepts HTTP requests from AI agents to LLM providers (Anthropic, OpenAI, etc.) \
         and applies PII protection measures before data leaves the device.\n\n",
    );
    md.push_str("**Processing operations:**\n");
    md.push_str("- Scanning request/response JSON bodies for PII patterns\n");
    md.push_str("- Encrypting structured PII (SSN, CC, phone, email) with FF1 FPE\n");
    md.push_str("- Redacting semantic PII (names, health terms, child references)\n");
    if config.image.enabled {
        md.push_str("- Detecting and blurring faces in images\n");
        md.push_str("- Detecting and blurring text regions in images (OCR)\n");
        md.push_str("- Stripping EXIF metadata from images\n");
    }
    md.push('\n');

    md.push_str("## 2. Necessity and Proportionality\n\n");
    md.push_str(
        "**Purpose:** Prevent PII from being sent to third-party LLM providers in plaintext.\n\n\
         **Necessity:** LLM providers process user messages for inference. Without OpenObscure, \
         all PII in conversations is transmitted and potentially stored by providers.\n\n\
         **Proportionality:** OpenObscure uses format-preserving encryption (not redaction) where \
         possible, preserving conversational context for the LLM while protecting real values. \
         Processing is localhost-only with no server-side component.\n\n",
    );

    md.push_str("## 3. Risk Assessment\n\n");
    md.push_str(
        "| Risk | Likelihood (without/with CG) | Impact | Mitigation | Residual Risk |\n\
         |------|------------------------------|--------|------------|---------------|\n\
         | PII sent to LLM provider | High → Low | High | FPE encryption, regex + NER scanning | Low |\n\
         | PII in persisted transcripts | High → Low | High | L1 plugin redaction, L2 AES-256-GCM encryption | Low |\n",
    );
    if config.image.enabled {
        md.push_str(
            "| Face in image sent to LLM | Medium → Low | High | BlazeFace detection + Gaussian blur | Low |\n\
             | Text in screenshot sent to LLM | Medium → Low | Medium | PaddleOCR detection + blur | Low |\n",
        );
    }
    md.push_str(
        "| FPE key compromise | Low | Critical | OS keychain, env var fallback, key rotation (planned) | Low |\n\
         | Audit log PII leakage | N/A | Medium | Audit logs contain encrypted labels only, never plaintext | Very Low |\n\
         | Proxy bypass | Low | High | Proxy is the only path to LLM providers when configured | Low |\n\n",
    );

    md.push_str("## 4. Measures to Address Risks\n\n");
    md.push_str("- **Encryption:** FF1 AES-256 FPE for structured PII, AES-256-GCM for transcripts at rest\n");
    md.push_str(
        "- **Key management:** OS keychain with env var fallback for headless environments\n",
    );
    md.push_str("- **Access control:** Localhost binding, health endpoint authentication\n");
    md.push_str("- **Audit trail:** Append-only GDPR audit log with encrypted labels only\n");
    md.push_str(
        "- **Breach detection:** Anomaly scoring on audit log (Art. 33 notification support)\n",
    );
    md.push_str("- **Data minimization:** Process only request/response bodies, no persistent storage of PII\n");
    md.push_str("- **Right to erasure:** L1 plugin supports `/privacy delete` (GDPR Art. 17)\n\n");

    md.push_str("## 5. Processing Statistics\n\n");
    md.push_str(&format!(
        "- Audit log entries analyzed: {}\n",
        entries.len()
    ));
    md.push_str(&format!("- Total PII detections: {}\n", total_pii));
    md.push_str("\n---\n*This document was auto-generated by OpenObscure Compliance CLI.*\n");

    md
}

fn generate_dpia_json(entries: &[AuditEntry], config: &AppConfig) -> serde_json::Value {
    let total_pii: u64 = entries.iter().filter_map(|e| e.pii_total).sum();

    serde_json::json!({
        "title": "Data Protection Impact Assessment (GDPR Art. 35)",
        "generated": now_iso8601(),
        "organization": config.compliance.organization_name,
        "description": "OpenObscure privacy firewall for AI agents",
        "statistics": {
            "audit_entries": entries.len(),
            "total_pii_detections": total_pii,
        },
        "risks": [
            {
                "risk": "PII sent to LLM provider",
                "likelihood_without": "High",
                "likelihood_with": "Low",
                "impact": "High",
                "mitigation": "FPE encryption, regex + NER scanning",
                "residual": "Low",
            },
            {
                "risk": "FPE key compromise",
                "likelihood_without": "N/A",
                "likelihood_with": "Low",
                "impact": "Critical",
                "mitigation": "OS keychain, env var fallback",
                "residual": "Low",
            },
        ],
        "safeguards": [
            "FF1 AES-256 FPE encryption",
            "AES-256-GCM transcript encryption",
            "OS keychain key management",
            "Localhost-only binding",
            "Append-only audit log",
            "Anomaly-based breach detection",
        ],
    })
}

// ---------------------------------------------------------------------------
// SIEM export (CEF / LEEF)
// ---------------------------------------------------------------------------

/// Format a single audit entry as a CEF (Common Event Format) line.
pub fn format_cef_line(entry: &AuditEntry) -> String {
    let severity = entry
        .pii_total
        .map(|n| {
            if n > 10 {
                7
            } else if n > 0 {
                5
            } else {
                3
            }
        })
        .unwrap_or(1);
    format!(
        "CEF:0|OpenObscure|PII-Proxy|{}|{}|{}|{}|src=127.0.0.1 request={} pii_total={} module={}",
        env!("CARGO_PKG_VERSION"),
        entry.operation,
        entry.operation,
        severity,
        entry.request_id.as_deref().unwrap_or("-"),
        entry.pii_total.unwrap_or(0),
        entry.module,
    )
}

/// Format a single audit entry as a LEEF (Log Event Extended Format) line.
pub fn format_leef_line(entry: &AuditEntry) -> String {
    format!(
        "LEEF:2.0|OpenObscure|PII-Proxy|{}|{}|src=127.0.0.1\trequest={}\tpii_total={}\tmodule={}",
        env!("CARGO_PKG_VERSION"),
        entry.operation,
        entry.request_id.as_deref().unwrap_or("-"),
        entry.pii_total.unwrap_or(0),
        entry.module,
    )
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn aggregate_pii_types(entries: &[AuditEntry]) -> Vec<(String, u64)> {
    let mut counts: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
    for entry in entries {
        if let Some(ref breakdown) = entry.pii_breakdown {
            for pair in breakdown.split(", ") {
                if let Some((pii_type, count_str)) = pair.split_once('=') {
                    if let Ok(count) = count_str.parse::<u64>() {
                        *counts.entry(pii_type.to_string()).or_default() += count;
                    }
                }
            }
        }
    }
    let mut sorted: Vec<_> = counts.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));
    sorted
}

fn write_output(content: &str, path: Option<&std::path::Path>) -> anyhow::Result<()> {
    match path {
        Some(p) => {
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(p, content)?;
            println!("Written to {}", p.display());
            Ok(())
        }
        None => {
            println!("{}", content);
            Ok(())
        }
    }
}

pub(crate) fn now_iso8601() -> String {
    // Use std::time for basic ISO 8601 timestamp (no chrono dependency)
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();
    // Simple UTC timestamp: YYYY-MM-DDTHH:MM:SSZ
    // Calculate from epoch seconds
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    // Approximate date calculation (good enough for report generation)
    let (year, month, day) = epoch_days_to_date(days);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hours, minutes, seconds
    )
}

fn epoch_days_to_date(days: u64) -> (u64, u64, u64) {
    // Algorithm from http://howardhinnant.github.io/date_algorithms.html
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_audit_jsonl() -> String {
        r#"{"timestamp":"2026-02-17T10:00:00Z","fields":{"oo_module":"scanner","operation":"scan","pii_total":3,"pii_breakdown":"ssn=1, email=2","request_id":"abc-123"}}
{"timestamp":"2026-02-17T10:05:00Z","fields":{"oo_module":"fpe","operation":"encrypt","pii_total":2,"pii_breakdown":"ssn=1, cc=1","request_id":"def-456"}}
{"timestamp":"2026-02-17T11:00:00Z","fields":{"oo_module":"scanner","operation":"scan","pii_total":1,"pii_breakdown":"phone=1","request_id":"ghi-789"}}
"#
        .to_string()
    }

    #[test]
    fn test_parse_audit_log_valid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        std::fs::write(&path, sample_audit_jsonl()).unwrap();

        let entries = parse_audit_log(
            Some(path.to_str().unwrap()),
            &AuditFilter {
                since: None,
                until: None,
                operation: None,
                limit: usize::MAX,
            },
        )
        .unwrap();

        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].operation, "scan");
        assert_eq!(entries[0].pii_total, Some(3));
        assert_eq!(entries[1].operation, "encrypt");
        assert_eq!(entries[2].module, "scanner");
    }

    #[test]
    fn test_parse_audit_log_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        std::fs::write(&path, "").unwrap();

        let entries = parse_audit_log(
            Some(path.to_str().unwrap()),
            &AuditFilter {
                since: None,
                until: None,
                operation: None,
                limit: usize::MAX,
            },
        )
        .unwrap();

        assert!(entries.is_empty());
    }

    #[test]
    fn test_parse_audit_log_missing_file() {
        let entries = parse_audit_log(
            Some("/tmp/nonexistent-openobscure-test.jsonl"),
            &AuditFilter {
                since: None,
                until: None,
                operation: None,
                limit: usize::MAX,
            },
        )
        .unwrap();

        assert!(entries.is_empty());
    }

    #[test]
    fn test_parse_audit_log_malformed_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        std::fs::write(
            &path,
            "not json\n{\"timestamp\":\"2026-01-01T00:00:00Z\",\"fields\":{\"oo_module\":\"test\",\"operation\":\"scan\"}}\nbroken{",
        )
        .unwrap();

        let entries = parse_audit_log(
            Some(path.to_str().unwrap()),
            &AuditFilter {
                since: None,
                until: None,
                operation: None,
                limit: usize::MAX,
            },
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].operation, "scan");
    }

    #[test]
    fn test_parse_audit_log_no_path_configured() {
        let result = parse_audit_log(
            None,
            &AuditFilter {
                since: None,
                until: None,
                operation: None,
                limit: usize::MAX,
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_filter_by_operation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        std::fs::write(&path, sample_audit_jsonl()).unwrap();

        let entries = parse_audit_log(
            Some(path.to_str().unwrap()),
            &AuditFilter {
                since: None,
                until: None,
                operation: Some("scan".to_string()),
                limit: usize::MAX,
            },
        )
        .unwrap();

        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|e| e.operation == "scan"));
    }

    #[test]
    fn test_filter_by_date() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        std::fs::write(&path, sample_audit_jsonl()).unwrap();

        let entries = parse_audit_log(
            Some(path.to_str().unwrap()),
            &AuditFilter {
                since: Some("2026-02-17T10:03:00Z".to_string()),
                until: None,
                operation: None,
                limit: usize::MAX,
            },
        )
        .unwrap();

        assert_eq!(entries.len(), 2); // 10:05 and 11:00
    }

    #[test]
    fn test_filter_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        std::fs::write(&path, sample_audit_jsonl()).unwrap();

        let entries = parse_audit_log(
            Some(path.to_str().unwrap()),
            &AuditFilter {
                since: None,
                until: None,
                operation: None,
                limit: 1,
            },
        )
        .unwrap();

        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn test_aggregate_pii_types() {
        let entries = vec![
            AuditEntry {
                timestamp: String::new(),
                module: String::new(),
                operation: String::new(),
                pii_total: Some(3),
                pii_breakdown: Some("ssn=1, email=2".to_string()),
                request_id: None,
            },
            AuditEntry {
                timestamp: String::new(),
                module: String::new(),
                operation: String::new(),
                pii_total: Some(2),
                pii_breakdown: Some("ssn=1, cc=1".to_string()),
                request_id: None,
            },
        ];

        let types = aggregate_pii_types(&entries);
        assert_eq!(types.len(), 3);
        // ssn and email both have count 2 — order between them is non-deterministic
        let top2: std::collections::HashSet<_> =
            types[..2].iter().map(|(k, _)| k.as_str()).collect();
        assert!(top2.contains("ssn"));
        assert!(top2.contains("email"));
        assert_eq!(types[0].1, 2);
        assert_eq!(types[1].1, 2);
        assert_eq!(types[2], ("cc".to_string(), 1));
    }

    #[test]
    fn test_ropa_markdown_generation() {
        let config = AppConfig {
            proxy: Default::default(),
            providers: {
                let mut m = std::collections::HashMap::new();
                m.insert(
                    "anthropic".to_string(),
                    crate::config::ProviderConfig {
                        upstream_url: "https://api.anthropic.com".to_string(),
                        route_prefix: "/anthropic".to_string(),
                        override_auth: false,
                        vault_key_name: None,
                        auth_header_name: None,
                        strip_headers: vec![],
                    },
                );
                m
            },
            fpe: Default::default(),
            scanner: Default::default(),
            logging: Default::default(),
            image: Default::default(),
            compliance: crate::config::ComplianceConfig {
                organization_name: Some("TestOrg".to_string()),
                dpo_email: Some("dpo@test.org".to_string()),
                ..Default::default()
            },
            cross_border: Default::default(),
        };

        let entries = vec![AuditEntry {
            timestamp: "2026-02-17T10:00:00Z".to_string(),
            module: "scanner".to_string(),
            operation: "scan".to_string(),
            pii_total: Some(3),
            pii_breakdown: Some("ssn=1, email=2".to_string()),
            request_id: Some("abc-123".to_string()),
        }];

        let md = generate_ropa_markdown(&entries, &config);
        assert!(md.contains("Record of Processing Activities"));
        assert!(md.contains("TestOrg"));
        assert!(md.contains("dpo@test.org"));
        assert!(md.contains("anthropic"));
        assert!(md.contains("Total PII detections: 3"));
    }

    #[test]
    fn test_ropa_json_generation() {
        let config = AppConfig {
            proxy: Default::default(),
            providers: std::collections::HashMap::new(),
            fpe: Default::default(),
            scanner: Default::default(),
            logging: Default::default(),
            image: Default::default(),
            compliance: Default::default(),
            cross_border: Default::default(),
        };

        let json = generate_ropa_json(&[], &config);
        assert_eq!(
            json["title"],
            "Record of Processing Activities (GDPR Art. 30)"
        );
        assert_eq!(json["statistics"]["audit_entries"], 0);
    }

    #[test]
    fn test_dpia_markdown_generation() {
        let config = AppConfig {
            proxy: Default::default(),
            providers: std::collections::HashMap::new(),
            fpe: Default::default(),
            scanner: Default::default(),
            logging: Default::default(),
            image: Default::default(),
            compliance: Default::default(),
            cross_border: Default::default(),
        };

        let md = generate_dpia_markdown(&[], &config);
        assert!(md.contains("Data Protection Impact Assessment"));
        assert!(md.contains("Risk Assessment"));
        assert!(md.contains("Measures to Address Risks"));
    }

    #[test]
    fn test_cef_format() {
        let entry = AuditEntry {
            timestamp: "2026-02-17T10:00:00Z".to_string(),
            module: "scanner".to_string(),
            operation: "scan".to_string(),
            pii_total: Some(5),
            pii_breakdown: None,
            request_id: Some("req-001".to_string()),
        };

        let line = format_cef_line(&entry);
        assert!(line.starts_with("CEF:0|OpenObscure|PII-Proxy|"));
        assert!(line.contains("scan"));
        assert!(line.contains("pii_total=5"));
        assert!(line.contains("request=req-001"));
    }

    #[test]
    fn test_leef_format() {
        let entry = AuditEntry {
            timestamp: "2026-02-17T10:00:00Z".to_string(),
            module: "fpe".to_string(),
            operation: "encrypt".to_string(),
            pii_total: Some(2),
            pii_breakdown: None,
            request_id: None,
        };

        let line = format_leef_line(&entry);
        assert!(line.starts_with("LEEF:2.0|OpenObscure|PII-Proxy|"));
        assert!(line.contains("encrypt"));
        assert!(line.contains("pii_total=2"));
    }

    #[test]
    fn test_epoch_days_to_date() {
        // 2026-02-17 is day 20501 since Unix epoch
        let (y, m, d) = epoch_days_to_date(20501);
        assert_eq!(y, 2026);
        assert_eq!(m, 2);
        assert_eq!(d, 17);
    }
}
