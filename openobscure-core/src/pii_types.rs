//! PII type taxonomy, FPE configuration, and format templates.
//!
//! Defines `PiiType` (15 types), `AlphabetMapper` (character set ↔ numeral
//! conversion for FF1), and `FormatTemplate` (separator-preserving encryption
//! that keeps dashes, spaces, and prefixes in place). Also provides
//! `FpeConfig` per type (radix, min length, alphabet) and the
//! `is_fpe_eligible()` predicate used by `body.rs` before calling the engine.

use std::collections::HashMap;
use std::fmt;

/// Each PII type the system recognizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PiiType {
    CreditCard,
    Ssn,
    PhoneNumber,
    Email,
    ApiKey,
    Ipv4Address,
    Ipv6Address,
    GpsCoordinate,
    MacAddress,
    HealthKeyword,
    ChildKeyword,
    Person,
    Location,
    Organization,
    Iban,
}

impl fmt::Display for PiiType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PiiType::CreditCard => write!(f, "credit_card"),
            PiiType::Ssn => write!(f, "ssn"),
            PiiType::PhoneNumber => write!(f, "phone"),
            PiiType::Email => write!(f, "email"),
            PiiType::ApiKey => write!(f, "api_key"),
            PiiType::Ipv4Address => write!(f, "ipv4_address"),
            PiiType::Ipv6Address => write!(f, "ipv6_address"),
            PiiType::GpsCoordinate => write!(f, "gps_coordinate"),
            PiiType::MacAddress => write!(f, "mac_address"),
            PiiType::HealthKeyword => write!(f, "health_keyword"),
            PiiType::ChildKeyword => write!(f, "child_keyword"),
            PiiType::Person => write!(f, "person"),
            PiiType::Location => write!(f, "location"),
            PiiType::Organization => write!(f, "organization"),
            PiiType::Iban => write!(f, "iban"),
        }
    }
}

impl PiiType {
    /// Returns FPE config for structured PII types.
    /// Keyword types (HealthKeyword, ChildKeyword) are redacted, not FPE-encrypted.
    pub fn config(&self) -> PiiTypeConfig {
        match self {
            PiiType::CreditCard => PiiTypeConfig {
                pii_type: *self,
                radix: 10,
                alphabet: "0123456789",
                min_length: 15, // 10^15 >> 1,000,000
            },
            PiiType::Ssn => PiiTypeConfig {
                pii_type: *self,
                radix: 10,
                alphabet: "0123456789",
                min_length: 9, // 10^9 >> 1,000,000
            },
            PiiType::PhoneNumber => PiiTypeConfig {
                pii_type: *self,
                radix: 10,
                alphabet: "0123456789",
                min_length: 10, // 10^10 >> 1,000,000
            },
            PiiType::Email => PiiTypeConfig {
                pii_type: *self,
                radix: 36,
                alphabet: "0123456789abcdefghijklmnopqrstuvwxyz",
                min_length: 4, // FF1 requires ≥4 numerals; 36^4 = 1.7M; catches admin, info, etc.
            },
            PiiType::ApiKey => PiiTypeConfig {
                pii_type: *self,
                radix: 62,
                alphabet: "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz",
                min_length: 6, // 62^6 >> 1,000,000
            },
            PiiType::Ipv4Address => PiiTypeConfig {
                pii_type: *self,
                radix: 10,
                alphabet: "0123456789",
                min_length: 4, // 10^4 = 10,000 ≥ FF1 min 100
            },
            PiiType::Ipv6Address => PiiTypeConfig {
                pii_type: *self,
                radix: 16,
                alphabet: "0123456789abcdef",
                min_length: 2, // 16^2 = 256 ≥ FF1 min 100
            },
            PiiType::GpsCoordinate => PiiTypeConfig {
                pii_type: *self,
                radix: 10,
                alphabet: "0123456789",
                min_length: 6, // 10^6 = 1,000,000
            },
            PiiType::MacAddress => PiiTypeConfig {
                pii_type: *self,
                radix: 16,
                alphabet: "0123456789abcdef",
                min_length: 6, // 16^6 >> 1,000,000
            },
            PiiType::Iban => PiiTypeConfig {
                pii_type: *self,
                radix: 36,
                alphabet: "0123456789abcdefghijklmnopqrstuvwxyz",
                min_length: 6, // 36^6 >> 1,000,000
            },
            // Semantic types (person, location, org, health/child keywords) are replaced
            // with hash-based tokens rather than FPE-encrypted, because their variable
            // length and character set make FF1 domain constraints impractical.
            // Callers must check `is_fpe_eligible()` before calling `config()`; this
            // dummy config is returned only as a fallback to avoid panicking.
            PiiType::HealthKeyword
            | PiiType::ChildKeyword
            | PiiType::Person
            | PiiType::Location
            | PiiType::Organization => PiiTypeConfig {
                pii_type: *self,
                radix: 0,
                alphabet: "",
                min_length: 0,
            },
        }
    }

    /// Whether this PII type uses FPE encryption (structured types) or redaction (keyword types).
    pub fn is_fpe_eligible(&self) -> bool {
        match self {
            PiiType::CreditCard
            | PiiType::Ssn
            | PiiType::PhoneNumber
            | PiiType::Email
            | PiiType::ApiKey
            | PiiType::Ipv4Address
            | PiiType::Ipv6Address
            | PiiType::GpsCoordinate
            | PiiType::MacAddress
            | PiiType::Iban => true,
            PiiType::HealthKeyword
            | PiiType::ChildKeyword
            | PiiType::Person
            | PiiType::Location
            | PiiType::Organization => false,
        }
    }

    /// The redaction placeholder for non-FPE types.
    pub fn redaction_label(&self) -> &'static str {
        match self {
            PiiType::Ipv4Address => "[IPv4]",
            PiiType::Ipv6Address => "[IPv6]",
            PiiType::GpsCoordinate => "[GPS]",
            PiiType::MacAddress => "[MAC]",
            PiiType::HealthKeyword => "[HEALTH]",
            PiiType::ChildKeyword => "[CHILD]",
            PiiType::Person => "[PERSON]",
            PiiType::Location => "[LOCATION]",
            PiiType::Organization => "[ORG]",
            PiiType::Iban => "[IBAN]",
            _ => "[REDACTED]",
        }
    }

    /// Short prefix for hash-based redaction tokens (3 chars).
    ///
    /// Used by `TokenGenerator` to produce tokens like `PER_a7f2`.
    pub fn hash_token_prefix(&self) -> &'static str {
        match self {
            PiiType::Ipv4Address => "IP4",
            PiiType::Ipv6Address => "IP6",
            PiiType::GpsCoordinate => "GPS",
            PiiType::MacAddress => "MAC",
            PiiType::HealthKeyword => "HLT",
            PiiType::ChildKeyword => "CHL",
            PiiType::Person => "PER",
            PiiType::Location => "LOC",
            PiiType::Organization => "ORG",
            PiiType::Iban => "IBN",
            // FPE types shouldn't call this, but provide a fallback
            _ => "RED",
        }
    }

    /// Config key name used for type_overrides in TOML.
    pub fn config_key(&self) -> &'static str {
        match self {
            PiiType::CreditCard => "credit_card",
            PiiType::Ssn => "ssn",
            PiiType::PhoneNumber => "phone",
            PiiType::Email => "email",
            PiiType::ApiKey => "api_key",
            PiiType::Ipv4Address => "ipv4_address",
            PiiType::Ipv6Address => "ipv6_address",
            PiiType::GpsCoordinate => "gps_coordinate",
            PiiType::MacAddress => "mac_address",
            PiiType::HealthKeyword => "health_keyword",
            PiiType::ChildKeyword => "child_keyword",
            PiiType::Person => "person",
            PiiType::Location => "location",
            PiiType::Organization => "organization",
            PiiType::Iban => "iban",
        }
    }
}

/// Configuration for how a PII type maps to FPE parameters.
#[derive(Debug, Clone)]
pub struct PiiTypeConfig {
    pub pii_type: PiiType,
    pub radix: u32,
    pub alphabet: &'static str,
    pub min_length: usize,
}

/// Maps characters to numeral values and back for a given alphabet.
#[derive(Debug, Clone)]
pub struct AlphabetMapper {
    pub radix: u32,
    char_to_num: HashMap<char, u16>,
    num_to_char: Vec<char>,
}

impl AlphabetMapper {
    pub fn new(alphabet: &str) -> Self {
        let mut char_to_num = HashMap::new();
        let mut num_to_char = Vec::new();
        for (i, c) in alphabet.chars().enumerate() {
            char_to_num.insert(c, i as u16);
            num_to_char.push(c);
        }
        Self {
            radix: alphabet.len() as u32,
            char_to_num,
            num_to_char,
        }
    }

    pub fn char_to_numeral(&self, c: char) -> Option<u16> {
        self.char_to_num.get(&c).copied()
    }

    pub fn numeral_to_char(&self, n: u16) -> Option<char> {
        self.num_to_char.get(n as usize).copied()
    }

    pub fn string_to_numerals(&self, s: &str) -> Option<Vec<u16>> {
        s.chars().map(|c| self.char_to_numeral(c)).collect()
    }

    pub fn numerals_to_string(&self, nums: &[u16]) -> Option<String> {
        nums.iter().map(|&n| self.numeral_to_char(n)).collect()
    }

    /// Check if a character is in this alphabet.
    pub fn contains(&self, c: char) -> bool {
        self.char_to_num.contains_key(&c)
    }
}

/// Tracks where separators live in a formatted PII value.
/// Used to strip them before FPE and re-insert after.
#[derive(Debug, Clone)]
pub struct FormatTemplate {
    /// Positions and characters of non-alphabet separators in the original string.
    pub separators: Vec<(usize, char)>,
    /// The "naked" string with separators removed — this is what gets encrypted.
    pub naked: String,
}

impl FormatTemplate {
    /// Strip non-alphabet characters from a raw PII match.
    pub fn from_raw(raw: &str, mapper: &AlphabetMapper) -> Self {
        let mut separators = Vec::new();
        let mut naked = String::new();
        for (i, c) in raw.chars().enumerate() {
            if mapper.contains(c) {
                naked.push(c);
            } else {
                separators.push((i, c));
            }
        }
        Self { separators, naked }
    }

    /// Re-insert separators into an encrypted naked string.
    pub fn apply(&self, encrypted_naked: &str) -> String {
        let mut result: Vec<char> = encrypted_naked.chars().collect();
        for &(pos, sep) in &self.separators {
            if pos <= result.len() {
                result.insert(pos, sep);
            }
        }
        result.into_iter().collect()
    }
}

/// Build AlphabetMappers for all PII types.
pub fn build_mappers() -> HashMap<PiiType, AlphabetMapper> {
    let mut mappers = HashMap::new();
    let types = [
        PiiType::CreditCard,
        PiiType::Ssn,
        PiiType::PhoneNumber,
        PiiType::Email,
        PiiType::ApiKey,
        PiiType::Ipv4Address,
        PiiType::Ipv6Address,
        PiiType::GpsCoordinate,
        PiiType::MacAddress,
        PiiType::Iban,
    ];
    for pii_type in types {
        let config = pii_type.config();
        mappers.insert(pii_type, AlphabetMapper::new(config.alphabet));
    }
    mappers
}

/// Known API key prefixes that should be preserved during FPE.
pub const API_KEY_PREFIXES: &[&str] = &[
    "sk-ant-", // Anthropic
    "sk-",     // OpenAI
    "AKIA",    // AWS
    "ghp_",    // GitHub
    "gho_",    // GitHub OAuth
    "xoxb-",   // Slack bot
    "xoxp-",   // Slack user
];

/// Find the known prefix of an API key, if any.
pub fn find_api_key_prefix(raw: &str) -> Option<&str> {
    API_KEY_PREFIXES
        .iter()
        .find(|&prefix| raw.starts_with(prefix))
        .map(|v| v as _)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_alphabet_mapper_digits() {
        let mapper = AlphabetMapper::new("0123456789");
        assert_eq!(mapper.char_to_numeral('0'), Some(0));
        assert_eq!(mapper.char_to_numeral('9'), Some(9));
        assert_eq!(mapper.char_to_numeral('a'), None);
        assert_eq!(mapper.numeral_to_char(0), Some('0'));
        assert_eq!(mapper.numeral_to_char(9), Some('9'));
        assert_eq!(mapper.numeral_to_char(10), None);
    }

    #[test]
    fn test_format_template_credit_card() {
        let mapper = AlphabetMapper::new("0123456789");
        let tmpl = FormatTemplate::from_raw("4532-1234-5678-9012", &mapper);
        assert_eq!(tmpl.naked, "4532123456789012");
        assert_eq!(tmpl.separators, vec![(4, '-'), (9, '-'), (14, '-')]);
        let restored = tmpl.apply("8714392760512483");
        assert_eq!(restored, "8714-3927-6051-2483");
    }

    #[test]
    fn test_format_template_ssn() {
        let mapper = AlphabetMapper::new("0123456789");
        let tmpl = FormatTemplate::from_raw("123-45-6789", &mapper);
        assert_eq!(tmpl.naked, "123456789");
        let restored = tmpl.apply("847293651");
        assert_eq!(restored, "847-29-3651");
    }

    #[test]
    fn test_find_api_key_prefix() {
        assert_eq!(find_api_key_prefix("sk-ant-abc123"), Some("sk-ant-"));
        assert_eq!(find_api_key_prefix("sk-abc123"), Some("sk-"));
        assert_eq!(find_api_key_prefix("AKIAIOSFODNN7EXAMPLE"), Some("AKIA"));
        assert_eq!(
            find_api_key_prefix("ghp_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"),
            Some("ghp_")
        );
        assert_eq!(find_api_key_prefix("unknown-key"), None);
    }

    #[test]
    fn test_iban_config() {
        let config = PiiType::Iban.config();
        assert_eq!(config.radix, 36);
        assert_eq!(config.min_length, 6);
        assert!(PiiType::Iban.is_fpe_eligible());
        assert_eq!(PiiType::Iban.config_key(), "iban");
        assert_eq!(PiiType::Iban.hash_token_prefix(), "IBN");
        assert_eq!(PiiType::Iban.redaction_label(), "[IBAN]");
        assert_eq!(format!("{}", PiiType::Iban), "iban");
    }

    #[test]
    fn test_new_fpe_types_eligible() {
        assert!(PiiType::Ipv4Address.is_fpe_eligible());
        assert!(PiiType::Ipv6Address.is_fpe_eligible());
        assert!(PiiType::GpsCoordinate.is_fpe_eligible());
        assert!(PiiType::MacAddress.is_fpe_eligible());
        assert!(PiiType::Iban.is_fpe_eligible());
        // These should remain non-FPE
        assert!(!PiiType::HealthKeyword.is_fpe_eligible());
        assert!(!PiiType::ChildKeyword.is_fpe_eligible());
        assert!(!PiiType::Person.is_fpe_eligible());
        assert!(!PiiType::Location.is_fpe_eligible());
        assert!(!PiiType::Organization.is_fpe_eligible());
    }

    #[test]
    fn test_format_template_ipv4() {
        let mapper = AlphabetMapper::new("0123456789");
        let tmpl = FormatTemplate::from_raw("192.168.1.42", &mapper);
        assert_eq!(tmpl.naked, "192168142");
        assert_eq!(tmpl.separators, vec![(3, '.'), (7, '.'), (9, '.')]);
    }

    #[test]
    fn test_format_template_gps() {
        let mapper = AlphabetMapper::new("0123456789");
        let tmpl = FormatTemplate::from_raw("45.5231, -122.6765", &mapper);
        assert_eq!(tmpl.naked, "4552311226765");
        // Separators: '.', ',', ' ', '-', '.'
        assert!(tmpl.separators.contains(&(2, '.')));
        assert!(tmpl.separators.contains(&(9, '-')));
    }

    #[test]
    fn test_format_template_mac_lowercase() {
        let mapper = AlphabetMapper::new("0123456789abcdef");
        let tmpl = FormatTemplate::from_raw("00:1a:2b:3c:4d:5e", &mapper);
        assert_eq!(tmpl.naked, "001a2b3c4d5e");
        assert_eq!(tmpl.separators.len(), 5); // 5 colons
    }

    #[test]
    fn test_build_mappers_includes_new_types() {
        let mappers = build_mappers();
        assert!(mappers.contains_key(&PiiType::Ipv4Address));
        assert!(mappers.contains_key(&PiiType::Ipv6Address));
        assert!(mappers.contains_key(&PiiType::GpsCoordinate));
        assert!(mappers.contains_key(&PiiType::MacAddress));
        assert!(mappers.contains_key(&PiiType::Iban));
    }
}
