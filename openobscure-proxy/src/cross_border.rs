use crate::config::CrossBorderConfig;
use crate::pii_types::PiiType;
use crate::scanner::PiiMatch;

/// Jurisdiction classification for PII data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Jurisdiction {
    US,
    EU,
    UK,
    Other(String),
    Unknown,
}

impl std::fmt::Display for Jurisdiction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Jurisdiction::US => write!(f, "US"),
            Jurisdiction::EU => write!(f, "EU"),
            Jurisdiction::UK => write!(f, "UK"),
            Jurisdiction::Other(s) => write!(f, "{}", s),
            Jurisdiction::Unknown => write!(f, "UNKNOWN"),
        }
    }
}

/// A jurisdiction flag raised on a PII match.
#[derive(Debug, Clone)]
pub struct JurisdictionFlag {
    pub pii_type: PiiType,
    pub jurisdiction: Jurisdiction,
    /// Truncated value hint (e.g. "+44...") — never the full PII value.
    pub value_hint: String,
}

/// Policy action to take based on cross-border classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyAction {
    Allow,
    Warn,
    Block,
}

impl std::fmt::Display for PolicyAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PolicyAction::Allow => write!(f, "allow"),
            PolicyAction::Warn => write!(f, "warn"),
            PolicyAction::Block => write!(f, "block"),
        }
    }
}

/// Result of cross-border classification and policy evaluation.
#[derive(Debug)]
pub struct CrossBorderResult {
    pub flags: Vec<JurisdictionFlag>,
    pub action: PolicyAction,
}

/// Classify PII matches by jurisdiction and enforce cross-border policy.
pub fn classify_and_enforce(
    matches: &[PiiMatch],
    config: &CrossBorderConfig,
) -> CrossBorderResult {
    let mut flags = Vec::new();

    for m in matches {
        if let Some(jurisdiction) = classify_match(m) {
            let hint = truncate_value(&m.raw_value, 6);
            flags.push(JurisdictionFlag {
                pii_type: m.pii_type.clone(),
                jurisdiction,
                value_hint: hint,
            });
        }
    }

    let action = evaluate_policy(&flags, config);

    CrossBorderResult { flags, action }
}

/// Classify a single PII match to a jurisdiction based on its type and value.
fn classify_match(m: &PiiMatch) -> Option<Jurisdiction> {
    match m.pii_type {
        PiiType::Ssn => Some(Jurisdiction::US),
        PiiType::PhoneNumber => Some(phone_to_jurisdiction(&m.raw_value)),
        PiiType::Email => email_to_jurisdiction(&m.raw_value),
        PiiType::CreditCard => Some(credit_card_to_jurisdiction(&m.raw_value)),
        _ => None,
    }
}

/// Classify phone number by country code prefix.
fn phone_to_jurisdiction(phone: &str) -> Jurisdiction {
    let digits: String = phone.chars().filter(|c| c.is_ascii_digit() || *c == '+').collect();

    if digits.starts_with("+1") || digits.starts_with("1") && digits.len() >= 11 {
        return Jurisdiction::US;
    }
    if digits.starts_with("+44") {
        return Jurisdiction::UK;
    }

    // EU country codes
    let eu_prefixes = [
        "+33", "+49", "+34", "+39", "+31", "+32", "+43", "+48", "+46", "+45",
        "+351", "+353", "+358", "+30", "+36", "+40", "+420", "+421", "+386",
        "+372", "+371", "+370", "+356", "+357", "+352", "+385",
    ];
    for prefix in &eu_prefixes {
        if digits.starts_with(prefix) {
            return Jurisdiction::EU;
        }
    }

    // Other notable country codes
    if digits.starts_with("+86") {
        return Jurisdiction::Other("CN".to_string());
    }
    if digits.starts_with("+81") {
        return Jurisdiction::Other("JP".to_string());
    }
    if digits.starts_with("+91") {
        return Jurisdiction::Other("IN".to_string());
    }
    if digits.starts_with("+61") {
        return Jurisdiction::Other("AU".to_string());
    }
    if digits.starts_with("+7") {
        return Jurisdiction::Other("RU".to_string());
    }
    if digits.starts_with("+55") {
        return Jurisdiction::Other("BR".to_string());
    }
    if digits.starts_with("+82") {
        return Jurisdiction::Other("KR".to_string());
    }

    Jurisdiction::Unknown
}

/// Classify email by TLD.
fn email_to_jurisdiction(email: &str) -> Option<Jurisdiction> {
    let domain = email.rsplit('@').next()?;
    let tld = domain.rsplit('.').next()?.to_lowercase();

    match tld.as_str() {
        "us" => Some(Jurisdiction::US),
        "uk" | "gb" => Some(Jurisdiction::UK),
        // EU country TLDs
        "de" | "fr" | "es" | "it" | "nl" | "be" | "at" | "pl" | "se" | "dk" | "pt" | "ie"
        | "fi" | "gr" | "hu" | "ro" | "cz" | "sk" | "si" | "ee" | "lv" | "lt" | "mt"
        | "cy" | "lu" | "hr" | "bg" | "eu" => Some(Jurisdiction::EU),
        "cn" => Some(Jurisdiction::Other("CN".to_string())),
        "jp" => Some(Jurisdiction::Other("JP".to_string())),
        "in" => Some(Jurisdiction::Other("IN".to_string())),
        "au" => Some(Jurisdiction::Other("AU".to_string())),
        "ru" => Some(Jurisdiction::Other("RU".to_string())),
        "br" => Some(Jurisdiction::Other("BR".to_string())),
        "kr" => Some(Jurisdiction::Other("KR".to_string())),
        // Generic TLDs (.com, .org, .net) — can't determine jurisdiction
        _ => None,
    }
}

/// Classify credit card by IIN (first 1-2 digits).
fn credit_card_to_jurisdiction(cc: &str) -> Jurisdiction {
    // Credit cards are issued globally — we can only approximate by IIN ranges.
    // For simplicity, treat all credit cards as Unknown jurisdiction.
    // In practice, BIN databases would be needed for accurate classification.
    let _ = cc;
    Jurisdiction::Unknown
}

/// Evaluate cross-border policy based on jurisdiction flags and config.
fn evaluate_policy(flags: &[JurisdictionFlag], config: &CrossBorderConfig) -> PolicyAction {
    if flags.is_empty() {
        return PolicyAction::Allow;
    }

    let has_blocked = flags.iter().any(|f| {
        let j = f.jurisdiction.to_string();
        config.blocked_jurisdictions.iter().any(|b| b.eq_ignore_ascii_case(&j))
    });

    if has_blocked && config.mode == "block" {
        return PolicyAction::Block;
    }

    let has_disallowed = if config.allowed_jurisdictions.is_empty() {
        false // empty allowed = all allowed
    } else {
        flags.iter().any(|f| {
            let j = f.jurisdiction.to_string();
            !config.allowed_jurisdictions.iter().any(|a| a.eq_ignore_ascii_case(&j))
                && f.jurisdiction != Jurisdiction::Unknown
        })
    };

    if has_disallowed && config.mode == "block" {
        return PolicyAction::Block;
    }

    if has_blocked || has_disallowed {
        return PolicyAction::Warn;
    }

    match config.mode.as_str() {
        "warn" => PolicyAction::Warn,
        _ => PolicyAction::Allow,
    }
}

/// Truncate a value for logging (never log full PII).
fn truncate_value(value: &str, max_len: usize) -> String {
    if value.len() <= max_len {
        value.to_string()
    } else {
        format!("{}...", &value[..max_len])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_match(pii_type: PiiType, value: &str) -> PiiMatch {
        PiiMatch {
            pii_type,
            raw_value: value.to_string(),
            start: 0,
            end: value.len(),
            json_path: None,
            confidence: 1.0,
        }
    }

    #[test]
    fn test_phone_us() {
        assert_eq!(phone_to_jurisdiction("+1-555-123-4567"), Jurisdiction::US);
    }

    #[test]
    fn test_phone_uk() {
        assert_eq!(phone_to_jurisdiction("+44-20-7946-0958"), Jurisdiction::UK);
    }

    #[test]
    fn test_phone_eu_france() {
        assert_eq!(phone_to_jurisdiction("+33-1-23-45-67-89"), Jurisdiction::EU);
    }

    #[test]
    fn test_phone_eu_germany() {
        assert_eq!(phone_to_jurisdiction("+49-30-12345678"), Jurisdiction::EU);
    }

    #[test]
    fn test_phone_china() {
        assert_eq!(
            phone_to_jurisdiction("+86-10-12345678"),
            Jurisdiction::Other("CN".to_string())
        );
    }

    #[test]
    fn test_phone_unknown() {
        assert_eq!(phone_to_jurisdiction("+999-12345"), Jurisdiction::Unknown);
    }

    #[test]
    fn test_email_eu_de() {
        assert_eq!(
            email_to_jurisdiction("user@example.de"),
            Some(Jurisdiction::EU)
        );
    }

    #[test]
    fn test_email_uk() {
        assert_eq!(
            email_to_jurisdiction("user@example.uk"),
            Some(Jurisdiction::UK)
        );
    }

    #[test]
    fn test_email_generic_com() {
        assert_eq!(email_to_jurisdiction("user@example.com"), None);
    }

    #[test]
    fn test_ssn_always_us() {
        let m = make_match(PiiType::Ssn, "123-45-6789");
        let jurisdiction = classify_match(&m);
        assert_eq!(jurisdiction, Some(Jurisdiction::US));
    }

    #[test]
    fn test_policy_allow_when_disabled() {
        let config = CrossBorderConfig {
            enabled: false,
            mode: "log".to_string(),
            allowed_jurisdictions: vec![],
            blocked_jurisdictions: vec![],
        };
        let flags = vec![JurisdictionFlag {
            pii_type: PiiType::PhoneNumber,
            jurisdiction: Jurisdiction::EU,
            value_hint: "+33...".to_string(),
        }];
        let action = evaluate_policy(&flags, &config);
        assert_eq!(action, PolicyAction::Allow);
    }

    #[test]
    fn test_policy_block_blocked_jurisdiction() {
        let config = CrossBorderConfig {
            enabled: true,
            mode: "block".to_string(),
            allowed_jurisdictions: vec![],
            blocked_jurisdictions: vec!["CN".to_string()],
        };
        let flags = vec![JurisdictionFlag {
            pii_type: PiiType::PhoneNumber,
            jurisdiction: Jurisdiction::Other("CN".to_string()),
            value_hint: "+86...".to_string(),
        }];
        let action = evaluate_policy(&flags, &config);
        assert_eq!(action, PolicyAction::Block);
    }

    #[test]
    fn test_policy_warn_blocked_jurisdiction_log_mode() {
        let config = CrossBorderConfig {
            enabled: true,
            mode: "log".to_string(),
            allowed_jurisdictions: vec![],
            blocked_jurisdictions: vec!["CN".to_string()],
        };
        let flags = vec![JurisdictionFlag {
            pii_type: PiiType::PhoneNumber,
            jurisdiction: Jurisdiction::Other("CN".to_string()),
            value_hint: "+86...".to_string(),
        }];
        let action = evaluate_policy(&flags, &config);
        assert_eq!(action, PolicyAction::Warn);
    }

    #[test]
    fn test_policy_block_not_in_allowed() {
        let config = CrossBorderConfig {
            enabled: true,
            mode: "block".to_string(),
            allowed_jurisdictions: vec!["US".to_string(), "EU".to_string()],
            blocked_jurisdictions: vec![],
        };
        let flags = vec![JurisdictionFlag {
            pii_type: PiiType::PhoneNumber,
            jurisdiction: Jurisdiction::UK,
            value_hint: "+44...".to_string(),
        }];
        let action = evaluate_policy(&flags, &config);
        assert_eq!(action, PolicyAction::Block);
    }

    #[test]
    fn test_empty_flags_always_allow() {
        let config = CrossBorderConfig {
            enabled: true,
            mode: "block".to_string(),
            allowed_jurisdictions: vec!["US".to_string()],
            blocked_jurisdictions: vec![],
        };
        let action = evaluate_policy(&[], &config);
        assert_eq!(action, PolicyAction::Allow);
    }

    #[test]
    fn test_classify_and_enforce_full() {
        let matches = vec![
            make_match(PiiType::PhoneNumber, "+44-20-7946-0958"),
            make_match(PiiType::Ssn, "123-45-6789"),
        ];
        let config = CrossBorderConfig {
            enabled: true,
            mode: "log".to_string(),
            allowed_jurisdictions: vec![],
            blocked_jurisdictions: vec![],
        };
        let result = classify_and_enforce(&matches, &config);
        assert_eq!(result.flags.len(), 2);
        assert_eq!(result.flags[0].jurisdiction, Jurisdiction::UK);
        assert_eq!(result.flags[1].jurisdiction, Jurisdiction::US);
        assert_eq!(result.action, PolicyAction::Allow);
    }

    #[test]
    fn test_truncate_value() {
        assert_eq!(truncate_value("+44-20-7946-0958", 6), "+44-20...");
        assert_eq!(truncate_value("short", 6), "short");
    }
}
