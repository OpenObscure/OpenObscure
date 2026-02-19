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
                min_length: 6, // 36^6 >> 1,000,000
            },
            PiiType::ApiKey => PiiTypeConfig {
                pii_type: *self,
                radix: 62,
                alphabet: "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz",
                min_length: 6, // 62^6 >> 1,000,000
            },
            // Network/device identifiers and keyword types are redacted, not FPE-encrypted.
            // Returning a dummy config — callers should check is_fpe_eligible() first.
            PiiType::Ipv4Address
            | PiiType::Ipv6Address
            | PiiType::GpsCoordinate
            | PiiType::MacAddress
            | PiiType::HealthKeyword
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
            | PiiType::ApiKey => true,
            PiiType::Ipv4Address
            | PiiType::Ipv6Address
            | PiiType::GpsCoordinate
            | PiiType::MacAddress
            | PiiType::HealthKeyword
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
            _ => "[REDACTED]",
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
    for prefix in API_KEY_PREFIXES {
        if raw.starts_with(prefix) {
            return Some(prefix);
        }
    }
    None
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
}
