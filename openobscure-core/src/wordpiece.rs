//! Minimal WordPiece tokenizer for BERT-style NER inference.
//!
//! Implements: optional lowercasing → whitespace/punctuation pre-tokenisation
//! → greedy longest-match subword segmentation. Produces token IDs, a
//! token-to-word-index alignment map, and a `[CLS]`/`[SEP]`-padded sequence
//! for direct use as ONNX `input_ids`. Vocabulary casing is auto-detected at
//! load time using `is_ascii_uppercase()`.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Minimal WordPiece tokenizer for BERT-style NER inference.
///
/// Implements: lowercase → pre-tokenize on whitespace/punctuation → greedy
/// longest-match WordPiece subword segmentation. Produces token IDs and a
/// word-to-token alignment map for BIO tag reassembly.
pub struct WordPieceTokenizer {
    vocab: HashMap<String, i64>,
    unk_id: i64,
    cls_id: i64,
    sep_id: i64,
    pad_id: i64,
    max_length: usize,
    do_lower_case: bool,
}

/// Result of tokenizing a single text.
#[derive(Debug)]
pub struct TokenizedInput {
    /// Token IDs for the model (includes [CLS], [SEP], [PAD]).
    pub input_ids: Vec<i64>,
    /// Attention mask (1 for real tokens, 0 for padding).
    pub attention_mask: Vec<i64>,
    /// Token type IDs (all zeros for single-sentence NER).
    pub token_type_ids: Vec<i64>,
    /// For each non-special token, the index of the original word it came from.
    /// Length equals the number of non-special, non-padding tokens.
    pub word_ids: Vec<Option<usize>>,
    /// The original pre-tokenized words with their byte offsets in the input text.
    pub words: Vec<WordSpan>,
}

/// A word from the original text with its byte offsets.
#[derive(Debug, Clone)]
pub struct WordSpan {
    pub text: String,
    pub byte_start: usize,
    pub byte_end: usize,
}

impl WordPieceTokenizer {
    /// Load a WordPiece vocabulary from a vocab.txt file (one token per line).
    pub fn from_file(path: &Path) -> Result<Self, WordPieceError> {
        let content = fs::read_to_string(path).map_err(|e| WordPieceError::Io(e.to_string()))?;
        let mut vocab = HashMap::new();
        for (i, line) in content.lines().enumerate() {
            vocab.insert(line.to_string(), i as i64);
        }
        Self::from_vocab(vocab)
    }

    /// Build from an in-memory vocabulary map.
    /// Automatically detects cased vs uncased vocab (cased vocabs contain uppercase
    /// word tokens like "The", "Robert", etc.).
    pub fn from_vocab(vocab: HashMap<String, i64>) -> Result<Self, WordPieceError> {
        let unk_id = *vocab
            .get("[UNK]")
            .ok_or(WordPieceError::MissingToken("[UNK]"))?;
        let cls_id = *vocab
            .get("[CLS]")
            .ok_or(WordPieceError::MissingToken("[CLS]"))?;
        let sep_id = *vocab
            .get("[SEP]")
            .ok_or(WordPieceError::MissingToken("[SEP]"))?;
        let pad_id = *vocab
            .get("[PAD]")
            .ok_or(WordPieceError::MissingToken("[PAD]"))?;

        // Cased vocab detection: look for multi-character tokens whose first byte is an
        // ASCII uppercase letter (e.g. "The", "Robert" in bert-base-cased).
        // We use `is_ascii_uppercase()` rather than `char::is_uppercase()` to avoid a
        // false positive from ℝ (U+211D, DOUBLE-STRUCK CAPITAL R), which appears in
        // bert-base-uncased vocabs and satisfies `is_uppercase()` but is not ASCII —
        // mistakenly treating uncased models as cased suppresses all lowercasing and
        // causes every token to be [UNK], collapsing NER recall to near zero.
        let is_cased = vocab.keys().any(|k| {
            k.len() > 1
                && !k.starts_with('[')
                && !k.starts_with('#')
                && k.as_bytes()
                    .first()
                    .is_some_and(|&b| b.is_ascii_uppercase())
        });

        Ok(Self {
            vocab,
            unk_id,
            cls_id,
            sep_id,
            pad_id,
            max_length: 512,
            do_lower_case: !is_cased,
        })
    }

    /// Tokenize a text string for NER inference.
    pub fn tokenize(&self, text: &str) -> TokenizedInput {
        let words = if self.do_lower_case {
            let lower = text.to_lowercase();
            pre_tokenize(&lower, text)
        } else {
            pre_tokenize(text, text)
        };

        let mut input_ids = vec![self.cls_id];
        let mut word_ids: Vec<Option<usize>> = vec![None]; // [CLS]

        for (word_idx, word) in words.iter().enumerate() {
            let sub_tokens = self.wordpiece_segment(&word.text);
            for token_id in &sub_tokens {
                if input_ids.len() >= self.max_length - 1 {
                    break; // Reserve space for [SEP]
                }
                input_ids.push(*token_id);
                word_ids.push(Some(word_idx));
            }
            if input_ids.len() >= self.max_length - 1 {
                break;
            }
        }

        input_ids.push(self.sep_id);
        word_ids.push(None); // [SEP]

        let real_len = input_ids.len();

        // Pad to max_length
        while input_ids.len() < self.max_length {
            input_ids.push(self.pad_id);
            word_ids.push(None);
        }

        let attention_mask: Vec<i64> = (0..self.max_length)
            .map(|i| if i < real_len { 1 } else { 0 })
            .collect();
        let token_type_ids = vec![0i64; self.max_length];

        TokenizedInput {
            input_ids,
            attention_mask,
            token_type_ids,
            word_ids,
            words,
        }
    }

    /// Greedy longest-match WordPiece segmentation of a single word.
    fn wordpiece_segment(&self, word: &str) -> Vec<i64> {
        let mut tokens = Vec::new();
        let chars: Vec<char> = word.chars().collect();
        let mut start = 0;

        while start < chars.len() {
            let mut end = chars.len();
            let mut found = false;

            while start < end {
                let substr: String = chars[start..end].iter().collect();
                let lookup = if start == 0 {
                    substr.clone()
                } else {
                    format!("##{}", substr)
                };

                if let Some(&id) = self.vocab.get(&lookup) {
                    tokens.push(id);
                    start = end;
                    found = true;
                    break;
                }
                end -= 1;
            }

            if !found {
                tokens.push(self.unk_id);
                start += 1;
            }
        }

        tokens
    }

    pub fn vocab_size(&self) -> usize {
        self.vocab.len()
    }
}

/// Pre-tokenize text by splitting on whitespace and punctuation.
/// Returns words with byte offsets into the *original* (not lowered) text.
fn pre_tokenize(lowered: &str, original: &str) -> Vec<WordSpan> {
    let mut words = Vec::new();
    let mut start = None;

    for (i, c) in lowered.char_indices() {
        if c.is_alphanumeric() || c == '\'' {
            if start.is_none() {
                start = Some(i);
            }
        } else {
            // Emit accumulated word
            if let Some(s) = start {
                words.push(WordSpan {
                    text: lowered[s..i].to_string(),
                    byte_start: s,
                    byte_end: i,
                });
                start = None;
            }
            // Punctuation as its own token
            if !c.is_whitespace() {
                words.push(WordSpan {
                    text: c.to_string(),
                    byte_start: i,
                    byte_end: i + c.len_utf8(),
                });
            }
        }
    }
    // Last word
    if let Some(s) = start {
        words.push(WordSpan {
            text: lowered[s..].to_string(),
            byte_start: s,
            byte_end: original.len(),
        });
    }

    words
}

#[derive(Debug, thiserror::Error)]
pub enum WordPieceError {
    #[error("IO error: {0}")]
    Io(String),
    #[error("Missing required special token: {0}")]
    MissingToken(&'static str),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_vocab() -> HashMap<String, i64> {
        let mut v = HashMap::new();
        let tokens = [
            "[PAD]", "[UNK]", "[CLS]", "[SEP]", "[MASK]", "hello", "world", "john", "smith",
            "diabetes", "the", "is", "my", "has", "i", "a", "b", "c", "d", "e", "f", "g", "h", "j",
            "k", "l", "m", "n", "o", "p", "q", "r", "s", "t", "u", "v", "w", "x", "y", "z", "##s",
            "##ed", "##ing", ".", ",", "!", "?",
        ];
        for (i, t) in tokens.iter().enumerate() {
            v.insert(t.to_string(), i as i64);
        }
        v
    }

    #[test]
    fn test_basic_tokenize() {
        let tok = WordPieceTokenizer::from_vocab(test_vocab()).unwrap();
        let result = tok.tokenize("Hello world");
        // [CLS] hello world [SEP] [PAD]...
        assert_eq!(result.input_ids[0], tok.cls_id);
        assert_eq!(result.input_ids[1], *tok.vocab.get("hello").unwrap());
        assert_eq!(result.input_ids[2], *tok.vocab.get("world").unwrap());
        assert_eq!(result.input_ids[3], tok.sep_id);
        assert_eq!(result.attention_mask[0], 1);
        assert_eq!(result.attention_mask[3], 1);
        assert_eq!(result.attention_mask[4], 0);
    }

    #[test]
    fn test_word_ids_alignment() {
        let tok = WordPieceTokenizer::from_vocab(test_vocab()).unwrap();
        let result = tok.tokenize("Hello world");
        assert_eq!(result.word_ids[0], None); // [CLS]
        assert_eq!(result.word_ids[1], Some(0)); // hello
        assert_eq!(result.word_ids[2], Some(1)); // world
        assert_eq!(result.word_ids[3], None); // [SEP]
    }

    #[test]
    fn test_unknown_words() {
        let tok = WordPieceTokenizer::from_vocab(test_vocab()).unwrap();
        let result = tok.tokenize("Hello xylophone");
        // "xylophone" is not in vocab → should produce [UNK] tokens
        assert!(result.input_ids[2..].contains(&tok.unk_id));
    }

    #[test]
    fn test_punctuation_split() {
        let tok = WordPieceTokenizer::from_vocab(test_vocab()).unwrap();
        let result = tok.tokenize("Hello, world!");
        // Should split: hello , world !
        assert_eq!(result.words.len(), 4);
        assert_eq!(result.words[0].text, "hello");
        assert_eq!(result.words[1].text, ",");
        assert_eq!(result.words[2].text, "world");
        assert_eq!(result.words[3].text, "!");
    }

    #[test]
    fn test_byte_offsets() {
        let tok = WordPieceTokenizer::from_vocab(test_vocab()).unwrap();
        let text = "John has diabetes";
        let result = tok.tokenize(text);
        assert_eq!(result.words[0].byte_start, 0);
        assert_eq!(result.words[0].byte_end, 4);
        assert_eq!(
            &text[result.words[2].byte_start..result.words[2].byte_end],
            "diabetes"
        );
    }

    #[test]
    fn test_pre_tokenize() {
        let text = "John Smith, MD.";
        let lower = text.to_lowercase();
        let words = pre_tokenize(&lower, text);
        let texts: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(texts, vec!["john", "smith", ",", "md", "."]);
    }

    #[test]
    fn test_subword_splitting() {
        let mut v = test_vocab();
        v.insert("work".to_string(), 50);
        v.insert("##ing".to_string(), 51);
        let tok = WordPieceTokenizer::from_vocab(v).unwrap();
        let result = tok.tokenize("working");
        // Should split as: work + ##ing
        assert_eq!(result.input_ids[1], 50); // work
        assert_eq!(result.input_ids[2], 51); // ##ing
                                             // Both sub-tokens map to word 0
        assert_eq!(result.word_ids[1], Some(0));
        assert_eq!(result.word_ids[2], Some(0));
    }

    /// Regression: Unicode math symbol ℝ (U+211D, DOUBLE-STRUCK CAPITAL R) is
    /// present in standard BERT uncased vocabs. Rust's char::is_uppercase()
    /// returns true for it (Unicode category Lu), which previously caused
    /// the auto-detect to misclassify the vocab as cased — disabling
    /// lowercasing and producing [UNK] for all capitalized words.
    #[test]
    fn test_unicode_math_symbol_does_not_trigger_cased_detection() {
        let mut v = test_vocab();
        // Add ℝ as a multi-char token (like real BERT uncased vocab)
        v.insert("\u{211D}".to_string(), 50); // single char ℝ, but len() > 1 in bytes
        let tok = WordPieceTokenizer::from_vocab(v).unwrap();
        // Should still lowercase — ℝ is a math symbol, not a cased-vocab indicator
        let result = tok.tokenize("Hello world");
        assert_eq!(result.words[0].text, "hello");
        assert_eq!(result.words[1].text, "world");
    }
}
