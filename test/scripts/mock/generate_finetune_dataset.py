#!/usr/bin/env python3
"""
Generate a fine-tuning dataset for TinyBERT PII NER.

Produces ~600 labeled samples in JSONL format with BIO tags for:
  PER    — person names
  LOC    — locations / addresses
  ORG    — organizations
  HEALTH — medical conditions, medications, symptoms, lab values
  CHILD  — child-related references (age descriptors, school, family)

Output: data/pii_finetune_dataset.jsonl

Each line is JSON:
  {"tokens": ["word1", "word2", ...], "ner_tags": ["O", "B-PER", ...]}
"""

import json
import random
import sys
from pathlib import Path

random.seed(42)

# ─── Name pools ──────────────────────────────────────────────────────────────

FIRST_NAMES = [
    "James", "Mary", "Robert", "Patricia", "John", "Jennifer", "Michael", "Linda",
    "David", "Elizabeth", "William", "Barbara", "Richard", "Susan", "Joseph", "Jessica",
    "Thomas", "Sarah", "Charles", "Karen", "Daniel", "Nancy", "Matthew", "Lisa",
    "Anthony", "Margaret", "Mark", "Betty", "Donald", "Sandra", "Steven", "Ashley",
    "Andrew", "Dorothy", "Paul", "Kimberly", "Joshua", "Emily", "Kenneth", "Donna",
    "Kevin", "Michelle", "Brian", "Carol", "George", "Amanda", "Timothy", "Melissa",
    "Ronald", "Deborah", "Jason", "Stephanie", "Ryan", "Rebecca", "Jacob", "Sharon",
    "Gary", "Laura", "Nicholas", "Cynthia", "Eric", "Kathleen", "Jonathan", "Amy",
    "Stephen", "Angela", "Larry", "Shirley", "Justin", "Brenda", "Scott", "Emma",
    "Brandon", "Anna", "Benjamin", "Pamela", "Samuel", "Nicole", "Frank", "Samantha",
    "Raymond", "Katherine", "Alexander", "Christine", "Patrick", "Debra", "Jack", "Rachel",
    "Dennis", "Carolyn", "Jerry", "Janet", "Tyler", "Catherine", "Aaron", "Maria",
    "Jose", "Heather", "Adam", "Diane", "Nathan", "Ruth", "Henry", "Julie",
    "Douglas", "Olivia", "Peter", "Joyce", "Zachary", "Virginia", "Kyle", "Victoria",
    "Wei", "Mei", "Raj", "Priya", "Amir", "Fatima", "Hiroshi", "Yuki",
    "Carlos", "Sofia", "Ahmed", "Aisha", "Ivan", "Natasha", "Kenji", "Hana",
    "Diego", "Valentina", "Chen", "Li", "Min-Jun", "Soo-Yeon", "Takeshi", "Sakura",
    "Omar", "Layla", "Pierre", "Isabelle", "Hans", "Greta", "Sven", "Ingrid",
]

LAST_NAMES = [
    "Smith", "Johnson", "Williams", "Brown", "Jones", "Garcia", "Miller", "Davis",
    "Rodriguez", "Martinez", "Hernandez", "Lopez", "Gonzalez", "Wilson", "Anderson",
    "Thomas", "Taylor", "Moore", "Jackson", "Martin", "Lee", "Perez", "Thompson",
    "White", "Harris", "Sanchez", "Clark", "Ramirez", "Lewis", "Robinson", "Walker",
    "Young", "Allen", "King", "Wright", "Scott", "Torres", "Nguyen", "Hill",
    "Flores", "Green", "Adams", "Nelson", "Baker", "Hall", "Rivera", "Campbell",
    "Mitchell", "Carter", "Roberts", "Gomez", "Phillips", "Evans", "Turner", "Diaz",
    "Parker", "Cruz", "Edwards", "Collins", "Reyes", "Stewart", "Morris", "Morales",
    "Murphy", "Cook", "Rogers", "Gutierrez", "Ortiz", "Morgan", "Cooper", "Peterson",
    "Bailey", "Reed", "Kelly", "Howard", "Ramos", "Kim", "Cox", "Ward",
    "Richardson", "Watson", "Brooks", "Chavez", "Wood", "James", "Bennett", "Gray",
    "Wang", "Li", "Zhang", "Chen", "Patel", "Singh", "Kumar", "Shah",
    "Tanaka", "Yamamoto", "Sato", "Watanabe", "Müller", "Schmidt", "Fischer", "Weber",
    "O'Brien", "McCarthy", "Sullivan", "Petrov", "Volkov", "Johansson", "Andersson",
]

CITIES = [
    "New York", "Los Angeles", "Chicago", "Houston", "Phoenix", "Philadelphia",
    "San Antonio", "San Diego", "Dallas", "San Jose", "Austin", "Jacksonville",
    "Fort Worth", "Columbus", "Charlotte", "Indianapolis", "San Francisco",
    "Seattle", "Denver", "Washington", "Nashville", "Oklahoma City", "El Paso",
    "Boston", "Portland", "Las Vegas", "Memphis", "Louisville", "Baltimore",
    "Milwaukee", "Albuquerque", "Tucson", "Fresno", "Sacramento", "Mesa",
    "Kansas City", "Atlanta", "Omaha", "Colorado Springs", "Raleigh",
    "Long Beach", "Virginia Beach", "Miami", "Oakland", "Minneapolis",
    "Tampa", "Tulsa", "Arlington", "New Orleans", "Cleveland",
]

STATES = [
    "Alabama", "Alaska", "Arizona", "Arkansas", "California", "Colorado",
    "Connecticut", "Delaware", "Florida", "Georgia", "Hawaii", "Idaho",
    "Illinois", "Indiana", "Iowa", "Kansas", "Kentucky", "Louisiana",
    "Maine", "Maryland", "Massachusetts", "Michigan", "Minnesota", "Mississippi",
    "Missouri", "Montana", "Nebraska", "Nevada", "New Hampshire", "New Jersey",
    "New Mexico", "New York", "North Carolina", "North Dakota", "Ohio",
    "Oklahoma", "Oregon", "Pennsylvania", "Rhode Island", "South Carolina",
    "South Dakota", "Tennessee", "Texas", "Utah", "Vermont", "Virginia",
    "Washington", "West Virginia", "Wisconsin", "Wyoming",
]

STREET_NAMES = [
    "Main", "Oak", "Elm", "Maple", "Pine", "Cedar", "Birch", "Walnut",
    "Cherry", "Willow", "Spruce", "Ash", "Hickory", "Magnolia", "Poplar",
    "Park", "Lake", "River", "Highland", "Valley", "Mountain", "Forest",
    "Meadow", "Sunset", "Spring", "Garden", "Hill", "Ridge", "Creek",
    "Washington", "Lincoln", "Jefferson", "Madison", "Adams", "Franklin",
]

STREET_TYPES = ["Street", "Avenue", "Boulevard", "Drive", "Lane", "Road", "Way", "Court", "Place"]

ORGANIZATIONS = [
    "Microsoft", "Google", "Amazon", "Apple", "Meta", "Tesla",
    "Goldman Sachs", "JPMorgan Chase", "Bank of America", "Wells Fargo",
    "Mayo Clinic", "Cleveland Clinic", "Johns Hopkins Hospital", "Massachusetts General Hospital",
    "Harvard University", "Stanford University", "MIT", "Yale University",
    "Acme Corporation", "Globex Industries", "Initech", "Umbrella Corporation",
    "Red Cross", "World Health Organization", "United Nations", "Amnesty International",
    "SpaceX", "Boeing", "Lockheed Martin", "Northrop Grumman",
    "Pfizer", "Johnson & Johnson", "Merck", "AstraZeneca",
    "Walmart", "Target", "Costco", "Home Depot",
    "Netflix", "Disney", "Warner Bros", "Paramount",
]

# ─── Medical/Health pools ────────────────────────────────────────────────────

CONDITIONS = [
    "diabetes", "hypertension", "asthma", "COPD", "heart failure",
    "coronary artery disease", "atrial fibrillation", "pneumonia",
    "chronic kidney disease", "hepatitis C", "HIV", "tuberculosis",
    "breast cancer", "lung cancer", "prostate cancer", "colon cancer",
    "leukemia", "lymphoma", "melanoma", "pancreatic cancer",
    "depression", "anxiety", "bipolar disorder", "schizophrenia", "PTSD",
    "ADHD", "autism spectrum disorder", "epilepsy", "multiple sclerosis",
    "Parkinson's disease", "Alzheimer's disease", "rheumatoid arthritis",
    "lupus", "Crohn's disease", "ulcerative colitis", "celiac disease",
    "type 1 diabetes", "type 2 diabetes", "gestational diabetes",
    "hypothyroidism", "hyperthyroidism", "anemia", "sickle cell disease",
    "cystic fibrosis", "fibromyalgia", "migraine", "stroke",
    "deep vein thrombosis", "pulmonary embolism", "sepsis",
]

MEDICATIONS = [
    "metformin", "lisinopril", "atorvastatin", "amlodipine", "metoprolol",
    "omeprazole", "losartan", "albuterol", "gabapentin", "hydrochlorothiazide",
    "sertraline", "acetaminophen", "amoxicillin", "prednisone", "tramadol",
    "ibuprofen", "azithromycin", "furosemide", "pantoprazole", "doxycycline",
    "citalopram", "montelukast", "rosuvastatin", "fluoxetine", "trazodone",
    "meloxicam", "carvedilol", "tamsulosin", "duloxetine", "venlafaxine",
    "insulin glargine", "levothyroxine", "warfarin", "clopidogrel", "aspirin",
    "oxycodone", "morphine", "hydrocodone", "fentanyl", "buprenorphine",
    "clonazepam", "lorazepam", "diazepam", "alprazolam", "zolpidem",
    "aripiprazole", "quetiapine", "olanzapine", "lithium", "lamotrigine",
    "Humira", "Keytruda", "Opdivo", "Eliquis", "Xarelto",
]

PROCEDURES = [
    "MRI", "CT scan", "X-ray", "ultrasound", "echocardiogram",
    "colonoscopy", "endoscopy", "bronchoscopy", "biopsy", "mammogram",
    "EKG", "EEG", "PET scan", "bone scan", "stress test",
    "blood transfusion", "dialysis", "chemotherapy", "radiation therapy",
    "physical therapy", "occupational therapy", "speech therapy",
    "hip replacement", "knee replacement", "appendectomy", "cholecystectomy",
    "CABG", "angioplasty", "cardiac catheterization", "pacemaker implant",
]

LAB_TESTS = [
    "hemoglobin", "hematocrit", "white blood cell count", "platelet count",
    "creatinine", "BUN", "GFR", "potassium", "sodium", "glucose",
    "HbA1c", "cholesterol", "LDL", "HDL", "triglycerides",
    "TSH", "T4", "PSA", "CEA", "AFP",
    "ALT", "AST", "bilirubin", "albumin", "INR",
    "troponin", "BNP", "D-dimer", "CRP", "ESR",
]

# ─── Child-related pools ─────────────────────────────────────────────────────

CHILD_TERMS = [
    "son", "daughter", "child", "baby", "toddler", "infant",
    "newborn", "teenager", "adolescent", "preschooler", "kindergartner",
    "grandson", "granddaughter", "nephew", "niece", "stepson", "stepdaughter",
    "foster child", "adopted son", "adopted daughter",
]

AGE_DESCRIPTORS = [
    "3-year-old", "4-year-old", "5-year-old", "6-year-old", "7-year-old",
    "8-year-old", "9-year-old", "10-year-old", "11-year-old", "12-year-old",
    "13-year-old", "14-year-old", "15-year-old", "16-year-old", "17-year-old",
    "2-month-old", "6-month-old", "9-month-old", "18-month-old",
]

SCHOOL_NAMES = [
    "Lincoln Elementary School", "Washington Middle School", "Jefferson High School",
    "Roosevelt Academy", "Kennedy Elementary", "Madison Preparatory School",
    "Sunset Elementary", "Lakewood Middle School", "Riverside High School",
    "Oakdale Elementary", "Pinecrest Academy", "Hillside Middle School",
    "Valley View Elementary", "Mountain Ridge High School", "Springdale Academy",
    "Cedar Park Elementary", "Maple Grove Middle School", "Northside High School",
]

CHILD_ACTIVITIES = [
    "daycare", "preschool", "kindergarten", "summer camp", "soccer practice",
    "swimming lessons", "piano lessons", "ballet class", "karate class",
    "tutoring", "speech therapy", "occupational therapy", "play therapy",
    "scouting", "little league", "gymnastics",
]

# ─── Helpers ─────────────────────────────────────────────────────────────────

def random_name():
    return random.choice(FIRST_NAMES), random.choice(LAST_NAMES)

def random_full_name():
    first, last = random_name()
    return f"{first} {last}"

def random_street():
    num = random.randint(100, 9999)
    name = random.choice(STREET_NAMES)
    stype = random.choice(STREET_TYPES)
    return str(num), name, stype

def tokenize_simple(text):
    """Split text into tokens, keeping punctuation separate."""
    tokens = []
    current = []
    for ch in text:
        if ch.isspace():
            if current:
                tokens.append("".join(current))
                current = []
        elif ch in ".,;:!?()[]{}\"'-/":
            if current:
                tokens.append("".join(current))
                current = []
            tokens.append(ch)
        else:
            current.append(ch)
    if current:
        tokens.append("".join(current))
    return tokens


def make_sample(tokens, ner_tags):
    """Create a sample dict, verifying lengths match."""
    assert len(tokens) == len(ner_tags), f"Length mismatch: {len(tokens)} tokens vs {len(ner_tags)} tags"
    return {"tokens": tokens, "ner_tags": ner_tags}


# ─── Sample generators ──────────────────────────────────────────────────────

def gen_health_condition_medication():
    """'Patient has [condition] and takes [medication].'"""
    samples = []
    for _ in range(60):
        first, last = random_name()
        cond = random.choice(CONDITIONS)
        med = random.choice(MEDICATIONS)
        cond_tokens = cond.split()
        med_tokens = med.split()

        tokens = [first, last, "was", "diagnosed", "with"]
        tags = ["B-PER", "I-PER", "O", "O", "O"]
        # condition
        tags.append("B-HEALTH")
        tokens.append(cond_tokens[0])
        for ct in cond_tokens[1:]:
            tokens.append(ct)
            tags.append("I-HEALTH")
        tokens.extend(["and", "takes"])
        tags.extend(["O", "O"])
        # medication
        tags.append("B-HEALTH")
        tokens.append(med_tokens[0])
        for mt in med_tokens[1:]:
            tokens.append(mt)
            tags.append("I-HEALTH")
        tokens.append(".")
        tags.append("O")
        samples.append(make_sample(tokens, tags))
    return samples


def gen_health_lab_results():
    """'Lab results: [test] is [value].'"""
    samples = []
    for _ in range(40):
        first, last = random_name()
        test = random.choice(LAB_TESTS)
        test_tokens = test.split()
        val = f"{random.uniform(0.1, 300.0):.1f}"

        tokens = [first, last, "'s"]
        tags = ["B-PER", "I-PER", "O"]
        # test
        tags.append("B-HEALTH")
        tokens.append(test_tokens[0])
        for tt in test_tokens[1:]:
            tokens.append(tt)
            tags.append("I-HEALTH")
        tokens.extend(["is", val, "."])
        tags.extend(["O", "O", "O"])
        samples.append(make_sample(tokens, tags))
    return samples


def gen_health_procedure():
    """'Doctor ordered a [procedure] for the patient.'"""
    samples = []
    for _ in range(40):
        first, last = random_name()
        proc = random.choice(PROCEDURES)
        proc_tokens = proc.split()

        templates = [
            (["Dr.", first, last, "ordered", "a"], ["O", "B-PER", "I-PER", "O", "O"]),
            ([first, last, "is", "scheduled", "for", "a"], ["B-PER", "I-PER", "O", "O", "O", "O"]),
            (["The", "doctor", "recommended"], ["O", "O", "O"]),
        ]
        prefix_tokens, prefix_tags = random.choice(templates)
        tokens = list(prefix_tokens)
        tags = list(prefix_tags)

        tags.append("B-HEALTH")
        tokens.append(proc_tokens[0])
        for pt in proc_tokens[1:]:
            tokens.append(pt)
            tags.append("I-HEALTH")

        suffixes = [
            (["for", first, last, "."], ["O", "B-PER", "I-PER", "O"]),
            (["next", "week", "."], ["O", "O", "O"]),
            (["at", random.choice(ORGANIZATIONS), "."], ["O", "B-ORG", "O"]),
        ]
        suf_tokens, suf_tags = random.choice(suffixes)
        tokens.extend(suf_tokens)
        tags.extend(suf_tags)
        samples.append(make_sample(tokens, tags))
    return samples


def gen_health_with_org():
    """'Patient at [Hospital] was treated for [condition].'"""
    samples = []
    hospitals = [o for o in ORGANIZATIONS if any(w in o for w in ["Clinic", "Hospital", "General"])]
    if not hospitals:
        hospitals = ["General Hospital", "City Medical Center"]
    for _ in range(30):
        first, last = random_name()
        hosp = random.choice(hospitals)
        hosp_tokens = hosp.split()
        cond = random.choice(CONDITIONS)
        cond_tokens = cond.split()

        tokens = [first, last, "was", "treated", "at"]
        tags = ["B-PER", "I-PER", "O", "O", "O"]
        tags.append("B-ORG")
        tokens.append(hosp_tokens[0])
        for ht in hosp_tokens[1:]:
            tokens.append(ht)
            tags.append("I-ORG")
        tokens.append("for")
        tags.append("O")
        tags.append("B-HEALTH")
        tokens.append(cond_tokens[0])
        for ct in cond_tokens[1:]:
            tokens.append(ct)
            tags.append("I-HEALTH")
        tokens.append(".")
        tags.append("O")
        samples.append(make_sample(tokens, tags))
    return samples


def gen_child_school():
    """'My [child_term] attends [School Name].'"""
    samples = []
    for _ in range(40):
        child = random.choice(CHILD_TERMS)
        child_tokens = child.split()
        school = random.choice(SCHOOL_NAMES)
        school_tokens = school.split()
        first_name = random.choice(FIRST_NAMES)

        templates = [
            (
                ["My"] + child_tokens + [first_name, "attends"],
                ["O"] + ["B-CHILD"] + ["I-CHILD"] * (len(child_tokens) - 1) + ["B-PER", "O"],
            ),
            (
                ["Our"] + child_tokens + ["goes", "to"],
                ["O"] + ["B-CHILD"] + ["I-CHILD"] * (len(child_tokens) - 1) + ["O", "O"],
            ),
            (
                [first_name, ",", "my"] + child_tokens + [",", "is", "enrolled", "at"],
                ["B-PER", "O", "O"] + ["B-CHILD"] + ["I-CHILD"] * (len(child_tokens) - 1) + ["O", "O", "O", "O"],
            ),
        ]
        prefix_tokens, prefix_tags = random.choice(templates)
        tokens = list(prefix_tokens)
        tags = list(prefix_tags)

        tags.append("B-ORG")
        tokens.append(school_tokens[0])
        for st in school_tokens[1:]:
            tokens.append(st)
            tags.append("I-ORG")
        tokens.append(".")
        tags.append("O")
        samples.append(make_sample(tokens, tags))
    return samples


def gen_child_age():
    """'My [age]-year-old [child_term] ...'"""
    samples = []
    for _ in range(40):
        age_desc = random.choice(AGE_DESCRIPTORS)
        child = random.choice(["son", "daughter", "child"])
        first_name = random.choice(FIRST_NAMES)
        activity = random.choice(CHILD_ACTIVITIES)
        activity_tokens = activity.split()

        tokens = ["My", age_desc, child, first_name]
        tags = ["O", "B-CHILD", "I-CHILD", "B-PER"]

        verbs = [
            (["started"], ["O"]),
            (["loves"], ["O"]),
            (["goes", "to"], ["O", "O"]),
            (["is", "in"], ["O", "O"]),
        ]
        v_tokens, v_tags = random.choice(verbs)
        tokens.extend(v_tokens)
        tags.extend(v_tags)

        tags.append("B-CHILD")
        tokens.append(activity_tokens[0])
        for at in activity_tokens[1:]:
            tokens.append(at)
            tags.append("I-CHILD")

        tokens.extend(["this", "year", "."])
        tags.extend(["O", "O", "O"])
        samples.append(make_sample(tokens, tags))
    return samples


def gen_child_health():
    """'My [child] was diagnosed with [condition].'"""
    samples = []
    child_conditions = [
        "asthma", "ADHD", "autism spectrum disorder", "ear infection",
        "strep throat", "allergies", "eczema", "food allergy",
        "speech delay", "developmental delay", "anxiety", "depression",
        "type 1 diabetes", "epilepsy", "scoliosis", "celiac disease",
    ]
    for _ in range(40):
        child = random.choice(CHILD_TERMS)
        child_tokens = child.split()
        cond = random.choice(child_conditions)
        cond_tokens = cond.split()
        first_name = random.choice(FIRST_NAMES)

        tokens = ["My", child_tokens[0]]
        tags = ["O", "B-CHILD"]
        for ct in child_tokens[1:]:
            tokens.append(ct)
            tags.append("I-CHILD")

        tokens.append(first_name)
        tags.append("B-PER")
        tokens.extend(["was", "diagnosed", "with"])
        tags.extend(["O", "O", "O"])

        tags.append("B-HEALTH")
        tokens.append(cond_tokens[0])
        for ct in cond_tokens[1:]:
            tokens.append(ct)
            tags.append("I-HEALTH")
        tokens.append(".")
        tags.append("O")
        samples.append(make_sample(tokens, tags))
    return samples


def gen_person_at_org():
    """'[Name] works at [Organization].'"""
    samples = []
    roles = [
        "works", "is employed", "has a position", "serves", "volunteers",
    ]
    for _ in range(40):
        first, last = random_name()
        org = random.choice(ORGANIZATIONS)
        org_tokens = org.split()
        role = random.choice(roles)
        role_tokens = role.split()

        tokens = [first, last]
        tags = ["B-PER", "I-PER"]
        for rt in role_tokens:
            tokens.append(rt)
            tags.append("O")
        tokens.append("at")
        tags.append("O")
        tags.append("B-ORG")
        tokens.append(org_tokens[0])
        for ot in org_tokens[1:]:
            tokens.append(ot)
            tags.append("I-ORG")
        tokens.append(".")
        tags.append("O")
        samples.append(make_sample(tokens, tags))
    return samples


def gen_person_at_location():
    """'[Name] lives at [Address] in [City], [State].'"""
    samples = []
    for _ in range(40):
        first, last = random_name()
        num, street, stype = random_street()
        city = random.choice(CITIES)
        city_tokens = city.split()
        state = random.choice(STATES)
        state_tokens = state.split()

        tokens = [first, last, "lives", "at", num, street, stype, "in"]
        tags = ["B-PER", "I-PER", "O", "O", "B-LOC", "I-LOC", "I-LOC", "O"]

        tags.append("B-LOC")
        tokens.append(city_tokens[0])
        for ct in city_tokens[1:]:
            tokens.append(ct)
            tags.append("I-LOC")
        tokens.append(",")
        tags.append("O")

        tags.append("B-LOC")
        tokens.append(state_tokens[0])
        for st in state_tokens[1:]:
            tokens.append(st)
            tags.append("I-LOC")
        tokens.append(".")
        tags.append("O")
        samples.append(make_sample(tokens, tags))
    return samples


def gen_location_only():
    """Sentences with just location mentions."""
    samples = []
    for _ in range(30):
        city = random.choice(CITIES)
        city_tokens = city.split()
        state = random.choice(STATES)
        state_tokens = state.split()

        templates = [
            (["The", "office", "is", "located", "in"], ["O", "O", "O", "O", "O"]),
            (["We", "are", "moving", "to"], ["O", "O", "O", "O"]),
            (["The", "conference", "will", "be", "held", "in"], ["O", "O", "O", "O", "O", "O"]),
            (["I", "recently", "visited"], ["O", "O", "O"]),
        ]
        prefix_tokens, prefix_tags = random.choice(templates)
        tokens = list(prefix_tokens)
        tags = list(prefix_tags)

        tags.append("B-LOC")
        tokens.append(city_tokens[0])
        for ct in city_tokens[1:]:
            tokens.append(ct)
            tags.append("I-LOC")
        tokens.append(",")
        tags.append("O")
        tags.append("B-LOC")
        tokens.append(state_tokens[0])
        for st in state_tokens[1:]:
            tokens.append(st)
            tags.append("I-LOC")
        tokens.append(".")
        tags.append("O")
        samples.append(make_sample(tokens, tags))
    return samples


def gen_multi_entity():
    """Sentences with multiple entity types."""
    samples = []
    for _ in range(50):
        first, last = random_name()
        org = random.choice(ORGANIZATIONS)
        org_tokens = org.split()
        city = random.choice(CITIES)
        city_tokens = city.split()
        cond = random.choice(CONDITIONS)
        cond_tokens = cond.split()

        # "[Name] at [Org] in [City] was treated for [condition]."
        tokens = [first, last, "at"]
        tags = ["B-PER", "I-PER", "O"]
        tags.append("B-ORG")
        tokens.append(org_tokens[0])
        for ot in org_tokens[1:]:
            tokens.append(ot)
            tags.append("I-ORG")
        tokens.append("in")
        tags.append("O")
        tags.append("B-LOC")
        tokens.append(city_tokens[0])
        for ct in city_tokens[1:]:
            tokens.append(ct)
            tags.append("I-LOC")
        tokens.extend(["was", "treated", "for"])
        tags.extend(["O", "O", "O"])
        tags.append("B-HEALTH")
        tokens.append(cond_tokens[0])
        for ct in cond_tokens[1:]:
            tokens.append(ct)
            tags.append("I-HEALTH")
        tokens.append(".")
        tags.append("O")
        samples.append(make_sample(tokens, tags))
    return samples


def gen_multi_person():
    """Sentences with multiple people."""
    samples = []
    for _ in range(30):
        f1, l1 = random_name()
        f2, l2 = random_name()
        while f2 == f1 and l2 == l1:
            f2, l2 = random_name()
        org = random.choice(ORGANIZATIONS)
        org_tokens = org.split()

        templates = [
            (
                [f1, l1, "and", f2, l2, "work", "at"],
                ["B-PER", "I-PER", "O", "B-PER", "I-PER", "O", "O"],
            ),
            (
                [f1, l1, "met", f2, l2, "at"],
                ["B-PER", "I-PER", "O", "B-PER", "I-PER", "O"],
            ),
            (
                [f1, l1, "introduced", f2, l2, "to", "the", "team", "at"],
                ["B-PER", "I-PER", "O", "B-PER", "I-PER", "O", "O", "O", "O"],
            ),
        ]
        prefix_tokens, prefix_tags = random.choice(templates)
        tokens = list(prefix_tokens)
        tags = list(prefix_tags)
        tags.append("B-ORG")
        tokens.append(org_tokens[0])
        for ot in org_tokens[1:]:
            tokens.append(ot)
            tags.append("I-ORG")
        tokens.append(".")
        tags.append("O")
        samples.append(make_sample(tokens, tags))
    return samples


def gen_negative_samples():
    """Sentences with NO PII — all O tags."""
    sentences = [
        "The weather forecast predicts rain for tomorrow .",
        "Please submit the report by end of day Friday .",
        "The quarterly earnings exceeded analyst expectations .",
        "Our team completed the sprint ahead of schedule .",
        "The software update includes performance improvements and bug fixes .",
        "The meeting has been rescheduled to next Tuesday at 3 PM .",
        "Version 2.0 of the application was released yesterday .",
        "The database migration completed successfully with zero downtime .",
        "Pull request 1234 has been approved and merged .",
        "The deployment pipeline runs automated tests before each release .",
        "Memory usage peaked at 256 megabytes during load testing .",
        "The API response time averaged 45 milliseconds .",
        "We need to refactor the authentication module .",
        "The code review identified three potential issues .",
        "Unit test coverage increased from 78 percent to 92 percent .",
        "The CI pipeline takes approximately 12 minutes to complete .",
        "Docker containers are configured with resource limits .",
        "The logging framework captures both errors and warnings .",
        "Network latency between data centers is under 10 milliseconds .",
        "The backup process runs every 6 hours automatically .",
        "Cache invalidation is handled by the pub/sub system .",
        "The load balancer distributes traffic across 4 instances .",
        "Rate limiting is set to 100 requests per minute .",
        "The encryption algorithm uses AES-256 in GCM mode .",
        "Feature flags allow gradual rollout to users .",
        "The microservice architecture enables independent deployments .",
        "GraphQL queries are validated against the schema .",
        "The monitoring dashboard shows real-time metrics .",
        "Automated alerts trigger when error rates exceed 1 percent .",
        "The search index is rebuilt nightly for optimal performance .",
        "Configuration values are stored in environment variables .",
        "The webhook endpoint accepts POST requests only .",
        "SSL certificates are renewed automatically 30 days before expiry .",
        "The retry policy uses exponential backoff with jitter .",
        "Connection pooling reduces database overhead significantly .",
        "The build system supports incremental compilation .",
        "Static analysis tools catch common programming errors .",
        "The application supports dark mode and light mode themes .",
        "Pagination is implemented using cursor-based navigation .",
        "The workflow engine processes up to 10000 events per second .",
    ]
    samples = []
    for sent in sentences:
        tokens = sent.split()
        tags = ["O"] * len(tokens)
        samples.append(make_sample(tokens, tags))
    return samples


def gen_financial_context():
    """Financial contexts with person and org entities."""
    samples = []
    for _ in range(30):
        first, last = random_name()
        bank = random.choice(["Goldman Sachs", "JPMorgan Chase", "Bank of America", "Wells Fargo",
                              "Citibank", "Morgan Stanley", "Charles Schwab", "Fidelity Investments"])
        bank_tokens = bank.split()

        templates = [
            (
                [first, last, "opened", "an", "account", "at"],
                ["B-PER", "I-PER", "O", "O", "O", "O"],
            ),
            (
                [first, last, "transferred", "funds", "from"],
                ["B-PER", "I-PER", "O", "O", "O"],
            ),
            (
                [first, last, "'s", "portfolio", "at"],
                ["B-PER", "I-PER", "O", "O", "O"],
            ),
        ]
        prefix_tokens, prefix_tags = random.choice(templates)
        tokens = list(prefix_tokens)
        tags = list(prefix_tags)
        tags.append("B-ORG")
        tokens.append(bank_tokens[0])
        for bt in bank_tokens[1:]:
            tokens.append(bt)
            tags.append("I-ORG")
        tokens.append(".")
        tags.append("O")
        samples.append(make_sample(tokens, tags))
    return samples


def gen_email_context():
    """Sentences mentioning email communication with names."""
    samples = []
    for _ in range(20):
        first, last = random_name()
        f2, l2 = random_name()

        templates = [
            (
                [first, last, "sent", "an", "email", "to", f2, l2, "regarding", "the", "project", "."],
                ["B-PER", "I-PER", "O", "O", "O", "O", "B-PER", "I-PER", "O", "O", "O", "O"],
            ),
            (
                ["Please", "contact", first, last, "for", "more", "information", "."],
                ["O", "O", "B-PER", "I-PER", "O", "O", "O", "O"],
            ),
            (
                [first, last, "will", "follow", "up", "with", f2, l2, "tomorrow", "."],
                ["B-PER", "I-PER", "O", "O", "O", "O", "B-PER", "I-PER", "O", "O"],
            ),
        ]
        tokens, tags = random.choice(templates)
        samples.append(make_sample(list(tokens), list(tags)))
    return samples


def gen_title_prefix():
    """Names with titles: Dr., Mr., Mrs., Prof., etc."""
    samples = []
    titles = ["Dr.", "Mr.", "Mrs.", "Ms.", "Prof.", "Rev.", "Judge"]
    for _ in range(30):
        title = random.choice(titles)
        first, last = random_name()
        org = random.choice(ORGANIZATIONS)
        org_tokens = org.split()

        tokens = [title, first, last, "from"]
        tags = ["O", "B-PER", "I-PER", "O"]
        tags.append("B-ORG")
        tokens.append(org_tokens[0])
        for ot in org_tokens[1:]:
            tokens.append(ot)
            tags.append("I-ORG")

        endings = [
            (["presented", "the", "findings", "."], ["O", "O", "O", "O"]),
            (["reviewed", "the", "case", "."], ["O", "O", "O", "O"]),
            (["published", "a", "paper", "."], ["O", "O", "O", "O"]),
        ]
        end_tokens, end_tags = random.choice(endings)
        tokens.extend(end_tokens)
        tags.extend(end_tags)
        samples.append(make_sample(tokens, tags))
    return samples


def gen_address_detailed():
    """Detailed address formats."""
    samples = []
    for _ in range(30):
        num, street, stype = random_street()
        city = random.choice(CITIES)
        city_tokens = city.split()
        state = random.choice(STATES)
        state_tokens = state.split()
        zipcode = str(random.randint(10000, 99999))

        templates = [
            (["The", "office", "is", "at"], ["O", "O", "O", "O"]),
            (["Send", "mail", "to"], ["O", "O", "O"]),
            (["Located", "at"], ["O", "O"]),
            (["Address", ":"], ["O", "O"]),
        ]
        prefix_tokens, prefix_tags = random.choice(templates)
        tokens = list(prefix_tokens)
        tags = list(prefix_tags)

        # Street address
        tokens.extend([num, street, stype])
        tags.extend(["B-LOC", "I-LOC", "I-LOC"])
        tokens.append(",")
        tags.append("O")

        # City
        tags.append("B-LOC")
        tokens.append(city_tokens[0])
        for ct in city_tokens[1:]:
            tokens.append(ct)
            tags.append("I-LOC")
        tokens.append(",")
        tags.append("O")

        # State
        tags.append("B-LOC")
        tokens.append(state_tokens[0])
        for st in state_tokens[1:]:
            tokens.append(st)
            tags.append("I-LOC")

        tokens.extend([zipcode, "."])
        tags.extend(["B-LOC", "O"])
        samples.append(make_sample(tokens, tags))
    return samples


def gen_child_medical():
    """Child + health combined scenarios (pediatric)."""
    samples = []
    for _ in range(30):
        child = random.choice(["son", "daughter", "child", "baby", "toddler"])
        first_name = random.choice(FIRST_NAMES)
        cond = random.choice(["ear infection", "fever", "cough", "allergies", "eczema",
                              "asthma", "ADHD", "strep throat", "RSV", "croup"])
        cond_tokens = cond.split()
        med = random.choice(["amoxicillin", "ibuprofen", "acetaminophen", "albuterol",
                             "prednisolone", "cetirizine", "montelukast"])
        med_tokens = med.split()

        tokens = ["My", child, first_name, "has"]
        tags = ["O", "B-CHILD", "B-PER", "O"]

        tags.append("B-HEALTH")
        tokens.append(cond_tokens[0])
        for ct in cond_tokens[1:]:
            tokens.append(ct)
            tags.append("I-HEALTH")

        tokens.extend(["and", "the", "pediatrician", "prescribed"])
        tags.extend(["O", "O", "B-CHILD", "O"])

        tags.append("B-HEALTH")
        tokens.append(med_tokens[0])
        for mt in med_tokens[1:]:
            tokens.append(mt)
            tags.append("I-HEALTH")

        tokens.append(".")
        tags.append("O")
        samples.append(make_sample(tokens, tags))
    return samples


def gen_referral():
    """'[Doctor] referred [Patient] to [Specialist] at [Hospital].'"""
    samples = []
    for _ in range(20):
        d_first, d_last = random_name()
        p_first, p_last = random_name()
        hosp = random.choice(["Mayo Clinic", "Cleveland Clinic", "Johns Hopkins Hospital",
                               "Massachusetts General Hospital", "Mount Sinai Hospital",
                               "Stanford Medical Center", "UCSF Medical Center"])
        hosp_tokens = hosp.split()

        tokens = ["Dr.", d_first, d_last, "referred", p_first, p_last, "to"]
        tags = ["O", "B-PER", "I-PER", "O", "B-PER", "I-PER", "O"]

        tags.append("B-ORG")
        tokens.append(hosp_tokens[0])
        for ht in hosp_tokens[1:]:
            tokens.append(ht)
            tags.append("I-ORG")

        endings = [
            (["for", "evaluation", "."], ["O", "O", "O"]),
            (["for", "further", "testing", "."], ["O", "O", "O", "O"]),
            (["for", "a", "second", "opinion", "."], ["O", "O", "O", "O", "O"]),
        ]
        end_tokens, end_tags = random.choice(endings)
        tokens.extend(end_tokens)
        tags.extend(end_tags)
        samples.append(make_sample(tokens, tags))
    return samples


def gen_education():
    """'[Name] graduated from [University] in [City].'"""
    samples = []
    universities = [
        "Harvard University", "Stanford University", "MIT",
        "Yale University", "Princeton University", "Columbia University",
        "University of Chicago", "Duke University", "Northwestern University",
        "University of Michigan", "UC Berkeley", "UCLA",
        "Cornell University", "Brown University", "University of Pennsylvania",
    ]
    for _ in range(20):
        first, last = random_name()
        uni = random.choice(universities)
        uni_tokens = uni.split()
        city = random.choice(CITIES)
        city_tokens = city.split()

        tokens = [first, last, "graduated", "from"]
        tags = ["B-PER", "I-PER", "O", "O"]

        tags.append("B-ORG")
        tokens.append(uni_tokens[0])
        for ut in uni_tokens[1:]:
            tokens.append(ut)
            tags.append("I-ORG")

        tokens.append("in")
        tags.append("O")

        tags.append("B-LOC")
        tokens.append(city_tokens[0])
        for ct in city_tokens[1:]:
            tokens.append(ct)
            tags.append("I-LOC")

        tokens.append(".")
        tags.append("O")
        samples.append(make_sample(tokens, tags))
    return samples


def gen_conversation():
    """Conversational/informal contexts."""
    samples = []
    for _ in range(20):
        first, last = random_name()
        f2 = random.choice(FIRST_NAMES)
        city = random.choice(CITIES)
        city_tokens = city.split()

        templates = [
            (
                ["Hey", ",", "have", "you", "talked", "to", first, last, "lately", "?"],
                ["O", "O", "O", "O", "O", "O", "B-PER", "I-PER", "O", "O"],
            ),
            (
                ["I", "ran", "into", first, last, "at", "the", "store", "in"],
                ["O", "O", "O", "B-PER", "I-PER", "O", "O", "O", "O"],
            ),
            (
                [f2, "told", "me", "that", first, last, "is", "moving", "to"],
                ["B-PER", "O", "O", "O", "B-PER", "I-PER", "O", "O", "O"],
            ),
        ]
        prefix_tokens, prefix_tags = random.choice(templates)
        tokens = list(prefix_tokens)
        tags = list(prefix_tags)

        if tokens[-1] == "in" or tokens[-1] == "to":
            tags.append("B-LOC")
            tokens.append(city_tokens[0])
            for ct in city_tokens[1:]:
                tokens.append(ct)
                tags.append("I-LOC")
            tokens.append(".")
            tags.append("O")

        samples.append(make_sample(tokens, tags))
    return samples


# ─── Main ────────────────────────────────────────────────────────────────────

def main():
    output_dir = Path(__file__).parent.parent / "data"
    output_dir.mkdir(exist_ok=True)
    output_path = output_dir / "pii_finetune_dataset.jsonl"

    # Include existing custom data
    existing_path = output_dir / "custom_health_child.jsonl"
    existing_samples = []
    if existing_path.exists():
        with open(existing_path) as f:
            for line in f:
                line = line.strip()
                if line:
                    existing_samples.append(json.loads(line))

    # Generate all categories
    generators = [
        ("health_condition_medication", gen_health_condition_medication),
        ("health_lab_results", gen_health_lab_results),
        ("health_procedure", gen_health_procedure),
        ("health_with_org", gen_health_with_org),
        ("child_school", gen_child_school),
        ("child_age", gen_child_age),
        ("child_health", gen_child_health),
        ("child_medical", gen_child_medical),
        ("person_at_org", gen_person_at_org),
        ("person_at_location", gen_person_at_location),
        ("location_only", gen_location_only),
        ("multi_entity", gen_multi_entity),
        ("multi_person", gen_multi_person),
        ("financial_context", gen_financial_context),
        ("email_context", gen_email_context),
        ("title_prefix", gen_title_prefix),
        ("address_detailed", gen_address_detailed),
        ("referral", gen_referral),
        ("education", gen_education),
        ("conversation", gen_conversation),
        ("negative_samples", gen_negative_samples),
    ]

    all_samples = list(existing_samples)
    category_counts = {}

    for name, gen_fn in generators:
        samples = gen_fn()
        category_counts[name] = len(samples)
        all_samples.extend(samples)

    # Shuffle
    random.shuffle(all_samples)

    # Write output
    with open(output_path, "w") as f:
        for sample in all_samples:
            f.write(json.dumps(sample) + "\n")

    # Summary
    print(f"=== Fine-tuning dataset generated ===")
    print(f"Output: {output_path}")
    print(f"Total samples: {len(all_samples)}")
    print(f"  Existing (custom_health_child): {len(existing_samples)}")
    print(f"  Generated:")
    for name, count in sorted(category_counts.items()):
        print(f"    {name}: {count}")

    # Validate: count entity distribution
    entity_counts = {}
    for sample in all_samples:
        for tag in sample["ner_tags"]:
            if tag.startswith("B-"):
                etype = tag[2:]
                entity_counts[etype] = entity_counts.get(etype, 0) + 1

    print(f"\nEntity distribution:")
    for etype, count in sorted(entity_counts.items(), key=lambda x: -x[1]):
        print(f"  {etype}: {count}")

    negative_count = sum(1 for s in all_samples if all(t == "O" for t in s["ner_tags"]))
    print(f"\nNegative samples (all O): {negative_count}")
    print(f"Positive samples: {len(all_samples) - negative_count}")


if __name__ == "__main__":
    main()
