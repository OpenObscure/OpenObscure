use std::collections::HashSet;

use crate::pii_types::PiiType;
use crate::scanner::PiiMatch;

/// A keyword dictionary scanner for health and child PII.
///
/// Uses two `HashSet<String>` instances (one per category) for O(1) lookup.
/// All terms are stored lowercased; input is lowercased before matching.
/// Scanning uses word-boundary tokenization (split on non-alphanumeric).
pub struct KeywordDict {
    health_terms: HashSet<String>,
    child_terms: HashSet<String>,
}

impl KeywordDict {
    /// Build the keyword dictionary with all built-in terms.
    pub fn new() -> Self {
        Self {
            health_terms: build_health_terms(),
            child_terms: build_child_terms(),
        }
    }

    /// Number of health terms loaded.
    pub fn health_count(&self) -> usize {
        self.health_terms.len()
    }

    /// Number of child terms loaded.
    pub fn child_count(&self) -> usize {
        self.child_terms.len()
    }

    /// Total terms loaded.
    pub fn total_count(&self) -> usize {
        self.health_terms.len() + self.child_terms.len()
    }

    /// Clone the health terms set (for CRF gazetteer features).
    pub fn health_terms_clone(&self) -> HashSet<String> {
        self.health_terms.clone()
    }

    /// Clone the child terms set (for CRF gazetteer features).
    pub fn child_terms_clone(&self) -> HashSet<String> {
        self.child_terms.clone()
    }

    /// Scan text for keyword matches using word-boundary tokenization.
    /// Returns matches sorted by start offset.
    pub fn scan_text(&self, text: &str) -> Vec<PiiMatch> {
        let mut matches = Vec::new();
        let lower = text.to_lowercase();

        // Scan for multi-word phrases first (2-word and 3-word), then single words.
        // This ensures longer matches take priority.
        let tokens = tokenize(&lower);

        // Check 3-word phrases
        for window in tokens.windows(3) {
            let phrase = format!("{} {} {}", window[0].text, window[1].text, window[2].text);
            let start = window[0].byte_start;
            let end = window[2].byte_end;
            if let Some(pii_type) = self.lookup_phrase(&phrase) {
                matches.push(PiiMatch {
                    pii_type,
                    start,
                    end,
                    raw_value: text[start..end].to_string(),
                    json_path: None,
                    confidence: 1.0,
                });
            }
        }

        // Check 2-word phrases
        for window in tokens.windows(2) {
            let phrase = format!("{} {}", window[0].text, window[1].text);
            let start = window[0].byte_start;
            let end = window[1].byte_end;
            if let Some(pii_type) = self.lookup_phrase(&phrase) {
                // Skip if overlapping with an existing (longer) match
                if !overlaps_any(&matches, start, end) {
                    matches.push(PiiMatch {
                        pii_type,
                        start,
                        end,
                        raw_value: text[start..end].to_string(),
                        json_path: None,
                        confidence: 1.0,
                    });
                }
            }
        }

        // Check single words
        for token in &tokens {
            if let Some(pii_type) = self.lookup_single(&token.text) {
                let start = token.byte_start;
                let end = token.byte_end;
                if !overlaps_any(&matches, start, end) {
                    matches.push(PiiMatch {
                        pii_type,
                        start,
                        end,
                        raw_value: text[start..end].to_string(),
                        json_path: None,
                        confidence: 1.0,
                    });
                }
            }
        }

        matches.sort_by_key(|m| m.start);
        matches
    }

    /// Look up a single word in both dictionaries. Health takes priority.
    fn lookup_single(&self, word: &str) -> Option<PiiType> {
        if self.health_terms.contains(word) {
            Some(PiiType::HealthKeyword)
        } else if self.child_terms.contains(word) {
            Some(PiiType::ChildKeyword)
        } else {
            None
        }
    }

    /// Look up a multi-word phrase in both dictionaries.
    fn lookup_phrase(&self, phrase: &str) -> Option<PiiType> {
        if self.health_terms.contains(phrase) {
            Some(PiiType::HealthKeyword)
        } else if self.child_terms.contains(phrase) {
            Some(PiiType::ChildKeyword)
        } else {
            None
        }
    }
}

/// A single token extracted from text with its byte offsets.
#[derive(Debug)]
struct Token {
    text: String,
    byte_start: usize,
    byte_end: usize,
}

/// Tokenize text on word boundaries (split on non-alphanumeric characters).
/// Preserves byte offsets into the input string.
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
            // Strip leading/trailing hyphens/apostrophes
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
fn overlaps_any(matches: &[PiiMatch], start: usize, end: usize) -> bool {
    matches.iter().any(|m| start < m.end && end > m.start)
}

// ─── Health dictionary (~450 terms) ─────────────────────────────────────────

fn build_health_terms() -> HashSet<String> {
    let terms: &[&str] = &[
        // ── Medical conditions (~150) ──
        "diabetes",
        "hypertension",
        "depression",
        "asthma",
        "cancer",
        "hiv",
        "aids",
        "epilepsy",
        "arthritis",
        "alzheimer's",
        "parkinson's",
        "schizophrenia",
        "bipolar",
        "adhd",
        "autism",
        "dementia",
        "anemia",
        "pneumonia",
        "bronchitis",
        "tuberculosis",
        "hepatitis",
        "cirrhosis",
        "fibromyalgia",
        "lupus",
        "sclerosis",
        "leukemia",
        "lymphoma",
        "melanoma",
        "carcinoma",
        "sarcoma",
        "mesothelioma",
        "glioblastoma",
        "stroke",
        "aneurysm",
        "embolism",
        "thrombosis",
        "arrhythmia",
        "angina",
        "myocarditis",
        "endocarditis",
        "pericarditis",
        "atherosclerosis",
        "osteoporosis",
        "osteoarthritis",
        "scoliosis",
        "tendinitis",
        "bursitis",
        "gout",
        "psoriasis",
        "eczema",
        "dermatitis",
        "rosacea",
        "vitiligo",
        "alopecia",
        "celiac",
        "crohn's",
        "colitis",
        "diverticulitis",
        "gastritis",
        "pancreatitis",
        "cholecystitis",
        "appendicitis",
        "peritonitis",
        "meningitis",
        "encephalitis",
        "neuropathy",
        "myopathy",
        "dystrophy",
        "glaucoma",
        "cataracts",
        "macular degeneration",
        "retinopathy",
        "tinnitus",
        "vertigo",
        "sinusitis",
        "tonsillitis",
        "laryngitis",
        "pharyngitis",
        "emphysema",
        "fibrosis",
        "sarcoidosis",
        "copd",
        "sleep apnea",
        "narcolepsy",
        "insomnia",
        "restless leg",
        "hypothyroidism",
        "hyperthyroidism",
        "cushing's",
        "addison's",
        "pcos",
        "endometriosis",
        "infertility",
        "miscarriage",
        "preeclampsia",
        "gestational diabetes",
        "uti",
        "kidney stones",
        "kidney disease",
        "dialysis",
        "transplant",
        "sepsis",
        "mrsa",
        "c. diff",
        "influenza",
        "covid",
        "covid-19",
        "long covid",
        "malaria",
        "dengue",
        "zika",
        "ebola",
        "cholera",
        "typhoid",
        "rabies",
        "tetanus",
        "measles",
        "mumps",
        "rubella",
        "chickenpox",
        "shingles",
        "herpes",
        "chlamydia",
        "gonorrhea",
        "syphilis",
        "hpv",
        "eating disorder",
        "anorexia",
        "bulimia",
        "obesity",
        "ptsd",
        "ocd",
        "anxiety disorder",
        "panic disorder",
        "agoraphobia",
        "claustrophobia",
        "social anxiety",
        "borderline personality",
        "dissociative",
        "psychosis",
        "suicidal",
        "self-harm",
        "substance abuse",
        "alcoholism",
        "addiction",
        "overdose",
        "withdrawal",
        "detox",
        "rehabilitation",
        "remission",
        "relapse",
        "terminal",
        "palliative",
        "hospice",
        "prognosis",
        "benign",
        "malignant",
        "metastatic",
        "chronic",
        "acute",
        "congenital",
        "hereditary",
        "autoimmune",
        "immunodeficiency",
        // ── Medications (~150) ──
        "metformin",
        "insulin",
        "lisinopril",
        "amlodipine",
        "metoprolol",
        "atorvastatin",
        "simvastatin",
        "rosuvastatin",
        "omeprazole",
        "pantoprazole",
        "esomeprazole",
        "levothyroxine",
        "losartan",
        "valsartan",
        "hydrochlorothiazide",
        "furosemide",
        "spironolactone",
        "warfarin",
        "heparin",
        "aspirin",
        "ibuprofen",
        "naproxen",
        "acetaminophen",
        "gabapentin",
        "pregabalin",
        "amitriptyline",
        "duloxetine",
        "sertraline",
        "fluoxetine",
        "paroxetine",
        "citalopram",
        "escitalopram",
        "venlafaxine",
        "bupropion",
        "mirtazapine",
        "trazodone",
        "clonazepam",
        "lorazepam",
        "diazepam",
        "alprazolam",
        "zolpidem",
        "aripiprazole",
        "quetiapine",
        "olanzapine",
        "risperidone",
        "lithium",
        "lamotrigine",
        "valproate",
        "carbamazepine",
        "topiramate",
        "levetiracetam",
        "phenytoin",
        "methylphenidate",
        "amphetamine",
        "atomoxetine",
        "montelukast",
        "fluticasone",
        "budesonide",
        "albuterol",
        "salbutamol",
        "ipratropium",
        "tiotropium",
        "prednisone",
        "prednisolone",
        "dexamethasone",
        "hydrocortisone",
        "methylprednisolone",
        "azithromycin",
        "amoxicillin",
        "ciprofloxacin",
        "levofloxacin",
        "doxycycline",
        "metronidazole",
        "clindamycin",
        "trimethoprim",
        "nitrofurantoin",
        "fluconazole",
        "acyclovir",
        "valacyclovir",
        "oseltamivir",
        "tamiflu",
        "remdesivir",
        "paxlovid",
        "tamoxifen",
        "letrozole",
        "anastrozole",
        "rituximab",
        "trastuzumab",
        "pembrolizumab",
        "nivolumab",
        "imatinib",
        "erlotinib",
        "sorafenib",
        "sunitinib",
        "methotrexate",
        "cyclophosphamide",
        "cisplatin",
        "carboplatin",
        "doxorubicin",
        "paclitaxel",
        "docetaxel",
        "vincristine",
        "etoposide",
        "oxycodone",
        "hydrocodone",
        "morphine",
        "fentanyl",
        "codeine",
        "tramadol",
        "buprenorphine",
        "naloxone",
        "naltrexone",
        "methadone",
        "suboxone",
        "narcan",
        "sildenafil",
        "tadalafil",
        "finasteride",
        "dutasteride",
        "tamsulosin",
        "donepezil",
        "memantine",
        "levodopa",
        "carbidopa",
        "ropinirole",
        "pramipexole",
        "adalimumab",
        "humira",
        "etanercept",
        "infliximab",
        "hydroxychloroquine",
        "sulfasalazine",
        "tacrolimus",
        "cyclosporine",
        "mycophenolate",
        "azathioprine",
        "epinephrine",
        "epipen",
        "nitroglycerin",
        "digoxin",
        "amiodarone",
        "diltiazem",
        "clopidogrel",
        "ticagrelor",
        "rivaroxaban",
        "apixaban",
        "dabigatran",
        "enoxaparin",
        // ── Medical terms/procedures (~100) ──
        "diagnosis",
        "prescription",
        "lab results",
        "blood pressure",
        "blood test",
        "blood work",
        "mri",
        "ct scan",
        "x-ray",
        "ultrasound",
        "mammogram",
        "colonoscopy",
        "endoscopy",
        "biopsy",
        "pathology",
        "radiology",
        "oncology",
        "cardiology",
        "neurology",
        "dermatology",
        "psychiatry",
        "ophthalmology",
        "orthopedics",
        "gynecology",
        "urology",
        "gastroenterology",
        "pulmonology",
        "endocrinology",
        "rheumatology",
        "hematology",
        "nephrology",
        "immunology",
        "anesthesia",
        "surgery",
        "chemotherapy",
        "radiation therapy",
        "immunotherapy",
        "physical therapy",
        "occupational therapy",
        "psychotherapy",
        "cbt",
        "counseling",
        "inpatient",
        "outpatient",
        "emergency room",
        "icu",
        "nicu",
        "ventilator",
        "intubation",
        "defibrillator",
        "pacemaker",
        "stent",
        "catheter",
        "iv drip",
        "blood transfusion",
        "bone marrow",
        "organ donor",
        "medical records",
        "health insurance",
        "hipaa",
        "patient",
        "clinical trial",
        "informed consent",
        "side effects",
        "adverse reaction",
        "contraindication",
        "dosage",
        "refill",
        "pharmacy",
        "pharmacist",
        "physician",
        "surgeon",
        "oncologist",
        "cardiologist",
        "neurologist",
        "psychiatrist",
        "therapist",
        "vaccination",
        "immunization",
        "booster",
        "antibody",
        "antigen",
        "viral load",
        "white blood cell",
        "red blood cell",
        "platelet",
        "hemoglobin",
        "cholesterol",
        "triglycerides",
        "glucose",
        "a1c",
        "creatinine",
        "bilirubin",
        "urinalysis",
        "ekg",
        "echocardiogram",
        "stress test",
        "pulmonary function",
        "spirometry",
        // ── Symptoms (~50) ──
        "chest pain",
        "shortness of breath",
        "palpitations",
        "dizziness",
        "fainting",
        "seizure",
        "convulsion",
        "paralysis",
        "numbness",
        "tingling",
        "tremor",
        "chronic pain",
        "migraine",
        "headache",
        "nausea",
        "vomiting",
        "diarrhea",
        "constipation",
        "bloating",
        "abdominal pain",
        "blood in stool",
        "blood in urine",
        "jaundice",
        "swelling",
        "edema",
        "rash",
        "itching",
        "hives",
        "bruising",
        "bleeding",
        "coughing blood",
        "wheezing",
        "chronic cough",
        "fever",
        "chills",
        "night sweats",
        "fatigue",
        "malaise",
        "weight loss",
        "weight gain",
        "loss of appetite",
        "difficulty swallowing",
        "difficulty breathing",
        "blurred vision",
        "hearing loss",
        "memory loss",
        "confusion",
        "hallucinations",
        "delusions",
        "insomnia",
    ];

    terms.iter().map(|t| t.to_lowercase()).collect()
}

// ─── Child dictionary (~200 terms) ──────────────────────────────────────────

fn build_child_terms() -> HashSet<String> {
    let terms: &[&str] = &[
        // ── Child references (~100) ──
        "my son",
        "my daughter",
        "my child",
        "my children",
        "my kid",
        "my kids",
        "my baby",
        "my toddler",
        "my infant",
        "my newborn",
        "my teenager",
        "my teen",
        "our son",
        "our daughter",
        "our child",
        "our children",
        "our baby",
        "our toddler",
        "her son",
        "her daughter",
        "his son",
        "his daughter",
        "stepson",
        "stepdaughter",
        "stepchild",
        "foster child",
        "adopted child",
        "grandchild",
        "grandson",
        "granddaughter",
        "niece",
        "nephew",
        "godchild",
        "godson",
        "goddaughter",
        "toddler",
        "infant",
        "newborn",
        "preschooler",
        "kindergartner",
        "first-grader",
        "second-grader",
        "third-grader",
        "fourth-grader",
        "fifth-grader",
        "sixth-grader",
        "seventh-grader",
        "eighth-grader",
        "tween",
        "preteen",
        "adolescent",
        "minor",
        "juvenile",
        "underage",
        "pediatric",
        "neonatal",
        "year-old",
        "years-old",
        "month-old",
        "months-old",
        "week-old",
        "weeks-old",
        "day-old",
        "days-old",
        "baby boy",
        "baby girl",
        "little boy",
        "little girl",
        "young boy",
        "young girl",
        "school-age",
        "child's",
        "children's",
        "kid's",
        "baby's",
        "boyhood",
        "girlhood",
        "childhood",
        "puberty",
        "teething",
        "crawling",
        "potty training",
        "diaper",
        "diapers",
        "pacifier",
        "stroller",
        "car seat",
        "crib",
        "bassinet",
        "playpen",
        "highchair",
        "baby monitor",
        "baby formula",
        "breastfeeding",
        "breastfed",
        "bottle-fed",
        "nursing",
        "weaning",
        "babysitter",
        "babysitting",
        "nanny",
        "au pair",
        "daycare",
        "childcare",
        // ── School/education (~50) ──
        "school",
        "preschool",
        "kindergarten",
        "elementary school",
        "middle school",
        "high school",
        "grade school",
        "teacher",
        "principal",
        "school counselor",
        "school nurse",
        "homework",
        "recess",
        "field trip",
        "report card",
        "parent-teacher",
        "pta",
        "iep",
        "special education",
        "gifted program",
        "school bus",
        "school lunch",
        "back to school",
        "class",
        "classroom",
        "tutor",
        "tutoring",
        "afterschool",
        "after-school",
        "extracurricular",
        "little league",
        "scout",
        "scouts",
        "cub scouts",
        "girl scouts",
        "boy scouts",
        "youth group",
        "youth sports",
        "youth program",
        "camp",
        "summer camp",
        "playdate",
        "play date",
        "sleepover",
        "birthday party",
        "trick or treat",
        "tooth fairy",
        "santa claus",
        "easter bunny",
        // ── Age indicators (~50) ──
        "1-year-old",
        "2-year-old",
        "3-year-old",
        "4-year-old",
        "5-year-old",
        "6-year-old",
        "7-year-old",
        "8-year-old",
        "9-year-old",
        "10-year-old",
        "11-year-old",
        "12-year-old",
        "13-year-old",
        "14-year-old",
        "15-year-old",
        "16-year-old",
        "17-year-old",
        "one year old",
        "two year old",
        "three year old",
        "four year old",
        "five year old",
        "six year old",
        "seven year old",
        "eight year old",
        "nine year old",
        "ten year old",
        "eleven year old",
        "twelve year old",
        "thirteen year old",
        "fourteen year old",
        "fifteen year old",
        "sixteen year old",
        "seventeen year old",
        "pediatrician",
        "child psychologist",
        "child psychiatrist",
        "child therapist",
        "child welfare",
        "child custody",
        "child support",
        "child abuse",
        "child neglect",
        "child protective",
        "guardianship",
        "legal guardian",
        "parental consent",
        "parental rights",
        "custody agreement",
        "visitation rights",
        "family court",
    ];

    terms.iter().map(|t| t.to_lowercase()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dictionary_sizes() {
        let dict = KeywordDict::new();
        // Plan calls for ~450 health + ~200 child = ~650-700 total
        assert!(
            dict.health_count() >= 400,
            "Expected >=400 health terms, got {}",
            dict.health_count()
        );
        assert!(
            dict.child_count() >= 150,
            "Expected >=150 child terms, got {}",
            dict.child_count()
        );
        assert!(
            dict.total_count() >= 600,
            "Expected >=600 total terms, got {}",
            dict.total_count()
        );
    }

    #[test]
    fn test_health_single_word() {
        let dict = KeywordDict::new();
        let matches = dict.scan_text("I was diagnosed with diabetes last year");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].pii_type, PiiType::HealthKeyword);
        assert_eq!(matches[0].raw_value.to_lowercase(), "diabetes");
    }

    #[test]
    fn test_health_medication() {
        let dict = KeywordDict::new();
        let matches = dict.scan_text("I take metformin and lisinopril daily");
        assert_eq!(matches.len(), 2);
        assert!(matches.iter().all(|m| m.pii_type == PiiType::HealthKeyword));
    }

    #[test]
    fn test_health_multi_word_phrase() {
        let dict = KeywordDict::new();
        let matches = dict.scan_text("My blood pressure has been high");
        // "blood pressure" should be a single match
        let bp_match = matches
            .iter()
            .find(|m| m.raw_value.to_lowercase().contains("blood pressure"));
        assert!(
            bp_match.is_some(),
            "Should detect 'blood pressure' as a phrase"
        );
    }

    #[test]
    fn test_child_possessive_phrases() {
        let dict = KeywordDict::new();
        let matches = dict.scan_text("my daughter started kindergarten this fall");
        assert!(matches
            .iter()
            .any(|m| m.pii_type == PiiType::ChildKeyword
                && m.raw_value.to_lowercase() == "my daughter"));
        assert!(matches
            .iter()
            .any(|m| m.pii_type == PiiType::ChildKeyword
                && m.raw_value.to_lowercase() == "kindergarten"));
    }

    #[test]
    fn test_child_age_indicators() {
        let dict = KeywordDict::new();
        let matches = dict.scan_text("My 8-year-old loves reading");
        assert!(matches
            .iter()
            .any(|m| m.pii_type == PiiType::ChildKeyword
                && m.raw_value.to_lowercase() == "8-year-old"));
    }

    #[test]
    fn test_case_insensitive() {
        let dict = KeywordDict::new();
        let matches = dict.scan_text("DIABETES is manageable with METFORMIN");
        assert_eq!(matches.len(), 2);
    }

    #[test]
    fn test_no_false_positives_on_clean_text() {
        let dict = KeywordDict::new();
        let matches = dict.scan_text("The weather is nice today. Let's go for a walk.");
        assert_eq!(matches.len(), 0);
    }

    #[test]
    fn test_no_false_positives_on_common_words() {
        let dict = KeywordDict::new();
        // These common words should NOT trigger matches
        let matches = dict.scan_text("I went to the store and bought some food for dinner");
        assert_eq!(matches.len(), 0);
    }

    #[test]
    fn test_mixed_health_and_child() {
        let dict = KeywordDict::new();
        let matches = dict.scan_text("My daughter has asthma and uses an inhaler");
        let child_matches: Vec<_> = matches
            .iter()
            .filter(|m| m.pii_type == PiiType::ChildKeyword)
            .collect();
        let health_matches: Vec<_> = matches
            .iter()
            .filter(|m| m.pii_type == PiiType::HealthKeyword)
            .collect();
        assert!(!child_matches.is_empty(), "Should find child keyword");
        assert!(!health_matches.is_empty(), "Should find health keyword");
    }

    #[test]
    fn test_match_offsets_correct() {
        let dict = KeywordDict::new();
        let text = "She takes metformin daily";
        let matches = dict.scan_text(text);
        assert_eq!(matches.len(), 1);
        let m = &matches[0];
        assert_eq!(&text[m.start..m.end].to_lowercase(), "metformin");
    }

    #[test]
    fn test_symptoms_detected() {
        let dict = KeywordDict::new();
        let matches = dict.scan_text("I've been having chest pain and shortness of breath");
        assert!(matches
            .iter()
            .any(|m| m.raw_value.to_lowercase() == "chest pain"));
    }

    #[test]
    fn test_school_terms() {
        let dict = KeywordDict::new();
        let matches = dict.scan_text("She goes to elementary school and loves recess");
        assert!(matches.iter().any(|m| m.pii_type == PiiType::ChildKeyword));
    }

    #[test]
    fn test_word_boundary_no_partial_match() {
        let dict = KeywordDict::new();
        // "camp" is a child term, but "campaign" should not match
        let matches = dict.scan_text("The marketing campaign was successful");
        assert!(
            matches.is_empty(),
            "Should not match 'camp' inside 'campaign'"
        );
    }

    #[test]
    fn test_deduplication_no_overlaps() {
        let dict = KeywordDict::new();
        let matches = dict.scan_text("blood pressure reading was normal");
        // "blood pressure" is a phrase, "blood" is not a standalone health term,
        // so we should get exactly one match for the phrase
        let blood_matches: Vec<_> = matches
            .iter()
            .filter(|m| m.raw_value.to_lowercase().contains("blood"))
            .collect();
        assert_eq!(
            blood_matches.len(),
            1,
            "Should have one match for 'blood pressure', not separate matches"
        );
    }

    #[test]
    fn test_tokenize_basic() {
        let tokens = tokenize("hello world foo");
        assert_eq!(tokens.len(), 3);
        assert_eq!(tokens[0].text, "hello");
        assert_eq!(tokens[1].text, "world");
        assert_eq!(tokens[2].text, "foo");
    }

    #[test]
    fn test_tokenize_with_hyphens() {
        let tokens = tokenize("8-year-old child");
        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[0].text, "8-year-old");
        assert_eq!(tokens[1].text, "child");
    }

    #[test]
    fn test_tokenize_preserves_offsets() {
        let text = "hello world";
        let tokens = tokenize(text);
        assert_eq!(tokens[0].byte_start, 0);
        assert_eq!(tokens[0].byte_end, 5);
        assert_eq!(tokens[1].byte_start, 6);
        assert_eq!(tokens[1].byte_end, 11);
    }
}
