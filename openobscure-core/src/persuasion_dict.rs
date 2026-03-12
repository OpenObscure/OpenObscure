//! Persuasion technique phrase dictionary for response integrity scanning.
//!
//! Detects known persuasion/manipulation patterns in LLM responses across
//! 7 categories mapped to Cialdini's principles and EU AI Act Article 5.
//! Uses HashSet-based O(1) lookup with 3→2→1 word scanning (longest match first).

use std::collections::HashSet;

/// Category of persuasion technique detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PersuasionCategory {
    /// "Act now!", "limited time", "don't wait" — creates false time pressure
    Urgency,
    /// "Only X left", "exclusive offer", "one-time deal" — artificial scarcity
    Scarcity,
    /// "Everyone is using", "most popular", "trusted by millions" — bandwagon pressure
    SocialProof,
    /// "You could lose", "risk of missing out", "don't fall behind" — fear/loss framing
    Fear,
    /// "Experts agree", "studies show", "recommended by professionals" — unverified authority
    Authority,
    /// "Best deal", "save money", "free trial", "discount" — commercial pressure
    Commercial,
    /// "Smart choice", "you clearly understand", "as someone who values" — ego manipulation
    Flattery,
}

impl std::fmt::Display for PersuasionCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Urgency => write!(f, "Urgency"),
            Self::Scarcity => write!(f, "Scarcity"),
            Self::SocialProof => write!(f, "Social Proof"),
            Self::Fear => write!(f, "Fear"),
            Self::Authority => write!(f, "Authority"),
            Self::Commercial => write!(f, "Commercial"),
            Self::Flattery => write!(f, "Flattery"),
        }
    }
}

/// A single persuasion phrase match found in text.
#[derive(Debug, Clone)]
pub struct PersuasionMatch {
    pub category: PersuasionCategory,
    pub start: usize,
    pub end: usize,
    pub phrase: String,
}

/// Persuasion phrase dictionary scanner.
///
/// Holds 7 category HashSets for O(1) per-token lookup.
/// All phrases stored lowercase; input lowercased before matching.
pub struct PersuasionDict {
    urgency: HashSet<String>,
    scarcity: HashSet<String>,
    social_proof: HashSet<String>,
    fear: HashSet<String>,
    authority: HashSet<String>,
    commercial: HashSet<String>,
    flattery: HashSet<String>,
}

impl Default for PersuasionDict {
    fn default() -> Self {
        Self::new()
    }
}

impl PersuasionDict {
    /// Build the persuasion dictionary with all built-in phrases.
    pub fn new() -> Self {
        Self {
            urgency: build_urgency_terms(),
            scarcity: build_scarcity_terms(),
            social_proof: build_social_proof_terms(),
            fear: build_fear_terms(),
            authority: build_authority_terms(),
            commercial: build_commercial_terms(),
            flattery: build_flattery_terms(),
        }
    }

    /// Total phrases loaded across all categories.
    pub fn total_count(&self) -> usize {
        self.urgency.len()
            + self.scarcity.len()
            + self.social_proof.len()
            + self.fear.len()
            + self.authority.len()
            + self.commercial.len()
            + self.flattery.len()
    }

    /// Scan text for persuasion phrase matches.
    /// Returns matches sorted by start offset.
    pub fn scan_text(&self, text: &str) -> Vec<PersuasionMatch> {
        let mut matches = Vec::new();
        let lower = text.to_lowercase();
        let tokens = tokenize(&lower);

        // Check 3-word phrases first (longest match priority)
        for window in tokens.windows(3) {
            let phrase = format!("{} {} {}", window[0].text, window[1].text, window[2].text);
            let start = window[0].byte_start;
            let end = window[2].byte_end;
            if let Some(category) = self.lookup(&phrase) {
                matches.push(PersuasionMatch {
                    category,
                    start,
                    end,
                    phrase: text[start..end].to_string(),
                });
            }
        }

        // Check 2-word phrases
        for window in tokens.windows(2) {
            let phrase = format!("{} {}", window[0].text, window[1].text);
            let start = window[0].byte_start;
            let end = window[1].byte_end;
            if let Some(category) = self.lookup(&phrase) {
                if !overlaps_any(&matches, start, end) {
                    matches.push(PersuasionMatch {
                        category,
                        start,
                        end,
                        phrase: text[start..end].to_string(),
                    });
                }
            }
        }

        // Check single words
        for token in &tokens {
            let start = token.byte_start;
            let end = token.byte_end;
            if let Some(category) = self.lookup(&token.text) {
                if !overlaps_any(&matches, start, end) {
                    matches.push(PersuasionMatch {
                        category,
                        start,
                        end,
                        phrase: text[start..end].to_string(),
                    });
                }
            }
        }

        matches.sort_by_key(|m| m.start);
        matches
    }

    /// Look up a phrase across all category sets. Returns first matching category.
    fn lookup(&self, phrase: &str) -> Option<PersuasionCategory> {
        if self.urgency.contains(phrase) {
            Some(PersuasionCategory::Urgency)
        } else if self.scarcity.contains(phrase) {
            Some(PersuasionCategory::Scarcity)
        } else if self.social_proof.contains(phrase) {
            Some(PersuasionCategory::SocialProof)
        } else if self.fear.contains(phrase) {
            Some(PersuasionCategory::Fear)
        } else if self.authority.contains(phrase) {
            Some(PersuasionCategory::Authority)
        } else if self.commercial.contains(phrase) {
            Some(PersuasionCategory::Commercial)
        } else if self.flattery.contains(phrase) {
            Some(PersuasionCategory::Flattery)
        } else {
            None
        }
    }
}

// ─── Tokenizer (same logic as keyword_dict.rs) ──────────────────────────────

#[derive(Debug)]
struct Token {
    text: String,
    byte_start: usize,
    byte_end: usize,
}

/// Tokenize text on word boundaries. Preserves byte offsets.
/// Input should already be lowercased.
fn tokenize(text: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut start = None;

    for (i, c) in text.char_indices() {
        if c.is_alphanumeric() || c == '-' || c == '\'' {
            if start.is_none() {
                start = Some(i);
            }
        } else if let Some(s) = start {
            let word = &text[s..i];
            let trimmed = word.trim_matches(|c: char| c == '-' || c == '\'');
            if !trimmed.is_empty() {
                let trim_offset = word.find(trimmed).unwrap_or(0);
                tokens.push(Token {
                    text: trimmed.to_string(),
                    byte_start: s + trim_offset,
                    byte_end: s + trim_offset + trimmed.len(),
                });
            }
            start = None;
        }
    }

    // Handle last token
    if let Some(s) = start {
        let word = &text[s..];
        let trimmed = word.trim_matches(|c: char| c == '-' || c == '\'');
        if !trimmed.is_empty() {
            let trim_offset = word.find(trimmed).unwrap_or(0);
            tokens.push(Token {
                text: trimmed.to_string(),
                byte_start: s + trim_offset,
                byte_end: s + trim_offset + trimmed.len(),
            });
        }
    }

    tokens
}

/// Check if a span [start, end) overlaps with any existing match.
fn overlaps_any(matches: &[PersuasionMatch], start: usize, end: usize) -> bool {
    matches.iter().any(|m| start < m.end && end > m.start)
}

// ─── Category dictionaries ──────────────────────────────────────────────────

fn build_urgency_terms() -> HashSet<String> {
    let terms: &[&str] = &[
        "act now",
        "act fast",
        "act immediately",
        "act quickly",
        "don't wait",
        "don't delay",
        "don't hesitate",
        "don't miss out",
        "hurry",
        "hurry up",
        "right now",
        "right away",
        "immediately",
        "today only",
        "now or never",
        "time is running out",
        "running out of time",
        "before it's too late",
        "while you still can",
        "last chance",
        "final chance",
        "final opportunity",
        "limited time",
        "limited time offer",
        "limited time only",
        "time-sensitive",
        "urgent",
        "urgently",
        "asap",
        "expires soon",
        "expiring soon",
        "offer expires",
        "deadline approaching",
        "closing soon",
        "ending soon",
        "won't last",
        "won't last long",
        "this won't last",
    ];
    terms.iter().map(|t| t.to_lowercase()).collect()
}

fn build_scarcity_terms() -> HashSet<String> {
    let terms: &[&str] = &[
        "only a few left",
        "limited supply",
        "limited availability",
        "limited quantities",
        "limited edition",
        "while supplies last",
        "selling fast",
        "selling out",
        "almost gone",
        "nearly sold out",
        "exclusive offer",
        "exclusive deal",
        "exclusive access",
        "exclusive opportunity",
        "one-time offer",
        "one-time deal",
        "one-time opportunity",
        "rare opportunity",
        "rare chance",
        "hard to find",
        "in high demand",
        "high demand",
        "going fast",
        "few remaining",
        "spots filling up",
        "limited spots",
        "limited seats",
        "first come first served",
    ];
    terms.iter().map(|t| t.to_lowercase()).collect()
}

fn build_social_proof_terms() -> HashSet<String> {
    let terms: &[&str] = &[
        "everyone is",
        "everyone loves",
        "everyone agrees",
        "everyone knows",
        "most people",
        "most users",
        "most customers",
        "millions of people",
        "millions of users",
        "thousands of people",
        "trusted by millions",
        "trusted by thousands",
        "join millions",
        "join thousands",
        "people are switching",
        "people are choosing",
        "the most popular",
        "best-selling",
        "top-rated",
        "highest-rated",
        "award-winning",
        "widely used",
        "widely adopted",
        "industry standard",
        "industry leading",
        "market leader",
        "don't be left behind",
        "you'll be left behind",
        "everyone else is",
        "your peers are",
        "your competitors are",
        "trending",
        "viral",
        "five-star",
        "five star",
    ];
    terms.iter().map(|t| t.to_lowercase()).collect()
}

fn build_fear_terms() -> HashSet<String> {
    let terms: &[&str] = &[
        "you could lose",
        "you will lose",
        "you might lose",
        "risk of losing",
        "risk of missing",
        "fear of missing out",
        "fomo",
        "don't fall behind",
        "falling behind",
        "left behind",
        "miss out",
        "missing out",
        "you'll regret",
        "you will regret",
        "you might regret",
        "regret not",
        "can't afford to miss",
        "can't afford to wait",
        "at risk",
        "at serious risk",
        "dangerous to ignore",
        "catastrophic",
        "devastating consequences",
        "dire consequences",
        "irreversible damage",
        "irreversible consequences",
        "point of no return",
        "too late",
        "it may be too late",
        "before it's too late",
        "what if you don't",
        "imagine losing",
        "think about what happens",
        "worst case scenario",
        "you can't afford",
    ];
    terms.iter().map(|t| t.to_lowercase()).collect()
}

fn build_authority_terms() -> HashSet<String> {
    let terms: &[&str] = &[
        "experts agree",
        "experts recommend",
        "experts say",
        "experts suggest",
        "studies show",
        "studies prove",
        "studies confirm",
        "research shows",
        "research proves",
        "research confirms",
        "scientifically proven",
        "clinically proven",
        "clinically tested",
        "doctor recommended",
        "doctor approved",
        "recommended by professionals",
        "recommended by experts",
        "endorsed by",
        "backed by science",
        "backed by research",
        "according to experts",
        "leading authorities",
        "leading experts",
        "top experts",
        "world-renowned",
        "peer-reviewed",
        "harvard study",
        "stanford study",
        "published research",
        "as seen on",
        "featured in",
        "recognized by",
        "certified by",
        "approved by",
        "trusted by professionals",
    ];
    terms.iter().map(|t| t.to_lowercase()).collect()
}

fn build_commercial_terms() -> HashSet<String> {
    let terms: &[&str] = &[
        "best deal",
        "best value",
        "best price",
        "great deal",
        "great value",
        "amazing deal",
        "incredible deal",
        "unbeatable price",
        "unbeatable deal",
        "lowest price",
        "save money",
        "save big",
        "save now",
        "huge savings",
        "massive savings",
        "discount",
        "special discount",
        "special offer",
        "special price",
        "special promotion",
        "free trial",
        "free shipping",
        "free bonus",
        "risk-free",
        "money back",
        "money-back guarantee",
        "satisfaction guaranteed",
        "no obligation",
        "no commitment",
        "cancel anytime",
        "buy now",
        "order now",
        "sign up now",
        "subscribe now",
        "get started now",
        "click here",
        "click now",
        "add to cart",
        "checkout now",
        "purchase now",
        "invest in",
        "invest now",
        "upgrade now",
        "premium",
        "pro version",
        "unlock",
        "unlock full",
    ];
    terms.iter().map(|t| t.to_lowercase()).collect()
}

fn build_flattery_terms() -> HashSet<String> {
    let terms: &[&str] = &[
        "smart choice",
        "smart decision",
        "wise choice",
        "wise decision",
        "great choice",
        "excellent choice",
        "perfect choice",
        "you clearly understand",
        "you obviously know",
        "you already know",
        "as someone who values",
        "as someone who cares",
        "as someone who understands",
        "you deserve",
        "you deserve the best",
        "you're worth it",
        "treat yourself",
        "reward yourself",
        "you've earned it",
        "you've earned this",
        "savvy",
        "discerning",
        "sophisticated",
        "someone like you",
        "people like you",
        "a person of your caliber",
        "forward-thinking",
        "ahead of the curve",
        "early adopter",
        "thought leader",
    ];
    terms.iter().map(|t| t.to_lowercase()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dictionary_sizes() {
        let dict = PersuasionDict::new();
        // Should have ~250 total phrases across 7 categories
        assert!(
            dict.total_count() >= 200,
            "Expected >=200 total phrases, got {}",
            dict.total_count()
        );
        assert!(!dict.urgency.is_empty());
        assert!(!dict.scarcity.is_empty());
        assert!(!dict.social_proof.is_empty());
        assert!(!dict.fear.is_empty());
        assert!(!dict.authority.is_empty());
        assert!(!dict.commercial.is_empty());
        assert!(!dict.flattery.is_empty());
    }

    #[test]
    fn test_urgency_detection() {
        let dict = PersuasionDict::new();
        let matches = dict.scan_text("Act now before this limited time offer expires!");
        assert!(
            matches
                .iter()
                .any(|m| m.category == PersuasionCategory::Urgency),
            "Should detect urgency phrases"
        );
    }

    #[test]
    fn test_scarcity_detection() {
        let dict = PersuasionDict::new();
        let matches = dict.scan_text("This is an exclusive offer with limited supply.");
        assert!(
            matches
                .iter()
                .any(|m| m.category == PersuasionCategory::Scarcity),
            "Should detect scarcity phrases"
        );
    }

    #[test]
    fn test_social_proof_detection() {
        let dict = PersuasionDict::new();
        let matches =
            dict.scan_text("Most people choose this option. It's the most popular choice.");
        assert!(
            matches
                .iter()
                .any(|m| m.category == PersuasionCategory::SocialProof),
            "Should detect social proof phrases"
        );
    }

    #[test]
    fn test_fear_detection() {
        let dict = PersuasionDict::new();
        let matches =
            dict.scan_text("You could lose everything if you don't act. Don't fall behind.");
        assert!(
            matches
                .iter()
                .any(|m| m.category == PersuasionCategory::Fear),
            "Should detect fear phrases"
        );
    }

    #[test]
    fn test_authority_detection() {
        let dict = PersuasionDict::new();
        let matches = dict.scan_text(
            "Experts agree this is the best approach. Studies show significant results.",
        );
        assert!(
            matches
                .iter()
                .any(|m| m.category == PersuasionCategory::Authority),
            "Should detect authority phrases"
        );
    }

    #[test]
    fn test_commercial_detection() {
        let dict = PersuasionDict::new();
        let matches =
            dict.scan_text("Get a free trial today with our special offer and save money.");
        assert!(
            matches
                .iter()
                .any(|m| m.category == PersuasionCategory::Commercial),
            "Should detect commercial phrases"
        );
    }

    #[test]
    fn test_flattery_detection() {
        let dict = PersuasionDict::new();
        let matches =
            dict.scan_text("Smart choice! You deserve the best. People like you always succeed.");
        assert!(
            matches
                .iter()
                .any(|m| m.category == PersuasionCategory::Flattery),
            "Should detect flattery phrases"
        );
    }

    #[test]
    fn test_case_insensitive() {
        let dict = PersuasionDict::new();
        let matches = dict.scan_text("ACT NOW! EXPERTS AGREE this is EXCLUSIVE.");
        assert!(
            matches.len() >= 2,
            "Should detect phrases regardless of case"
        );
    }

    #[test]
    fn test_no_false_positives_on_clean_text() {
        let dict = PersuasionDict::new();
        let matches = dict.scan_text(
            "The function returns a list of integers sorted in ascending order. \
             Use the map method to transform each element.",
        );
        assert_eq!(
            matches.len(),
            0,
            "Clean technical text should produce no matches"
        );
    }

    #[test]
    fn test_overlap_dedup() {
        let dict = PersuasionDict::new();
        // "limited time offer" is a 3-word phrase; "limited time" is a 2-word phrase
        // Only the longer match should appear
        let matches = dict.scan_text("This is a limited time offer.");
        let limited_matches: Vec<_> = matches
            .iter()
            .filter(|m| m.phrase.to_lowercase().contains("limited"))
            .collect();
        assert_eq!(
            limited_matches.len(),
            1,
            "Should have one match for 'limited time offer', not separate overlapping matches"
        );
        assert!(
            limited_matches[0]
                .phrase
                .to_lowercase()
                .contains("limited time offer"),
            "Should prefer the longer 3-word phrase"
        );
    }

    #[test]
    fn test_multiple_categories() {
        let dict = PersuasionDict::new();
        let matches = dict.scan_text(
            "Act now! Experts agree this exclusive offer is a smart choice. You could lose out.",
        );
        let categories: HashSet<_> = matches.iter().map(|m| m.category).collect();
        assert!(
            categories.len() >= 3,
            "Should detect multiple categories, got: {:?}",
            categories
        );
    }

    #[test]
    fn test_match_offsets_correct() {
        let dict = PersuasionDict::new();
        let text = "Please act now before it expires.";
        let matches = dict.scan_text(text);
        for m in &matches {
            assert_eq!(
                text[m.start..m.end].to_lowercase(),
                m.phrase.to_lowercase(),
                "Offset should point to the matched phrase"
            );
        }
    }

    #[test]
    fn test_category_display() {
        assert_eq!(PersuasionCategory::Urgency.to_string(), "Urgency");
        assert_eq!(PersuasionCategory::SocialProof.to_string(), "Social Proof");
        assert_eq!(PersuasionCategory::Fear.to_string(), "Fear");
        assert_eq!(PersuasionCategory::Commercial.to_string(), "Commercial");
        assert_eq!(PersuasionCategory::Flattery.to_string(), "Flattery");
    }

    #[test]
    fn test_empty_text() {
        let dict = PersuasionDict::new();
        let matches = dict.scan_text("");
        assert_eq!(matches.len(), 0);
    }
}
