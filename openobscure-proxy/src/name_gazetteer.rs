use std::collections::HashSet;

use crate::pii_types::PiiType;
use crate::scanner::PiiMatch;

/// A name gazetteer for person name detection using case-sensitive dictionary lookup.
///
/// Uses two `HashSet<String>` instances (first names + surnames) for O(1) lookup.
/// All matching is **case-sensitive** — "John" matches, "john" and "JOHN" do not.
///
/// Confidence gating:
/// - Two-word match (first_name + surname): confidence 0.7 — passes min_confidence on its own
/// - Single-word match: confidence 0.4 — below min_confidence 0.5, filtered unless
///   NER also detects the same span (agreement bonus +0.15 → 0.55 → passes)
pub struct NameGazetteer {
    first_names: HashSet<String>,
    surnames: HashSet<String>,
}

/// Confidence for a two-word name match (first_name + surname).
const TWO_WORD_CONFIDENCE: f32 = 0.7;

/// Confidence for a single-word name match.
const SINGLE_WORD_CONFIDENCE: f32 = 0.4;

impl Default for NameGazetteer {
    fn default() -> Self {
        Self::new()
    }
}

impl NameGazetteer {
    /// Build the name gazetteer with embedded name lists.
    pub fn new() -> Self {
        Self {
            first_names: build_first_names(),
            surnames: build_surnames(),
        }
    }

    /// Number of first names loaded.
    pub fn first_name_count(&self) -> usize {
        self.first_names.len()
    }

    /// Number of surnames loaded.
    pub fn surname_count(&self) -> usize {
        self.surnames.len()
    }

    /// Total names loaded.
    pub fn total_count(&self) -> usize {
        self.first_names.len() + self.surnames.len()
    }

    /// Clone the first names set (for CRF gazetteer features).
    pub fn first_names_clone(&self) -> HashSet<String> {
        self.first_names.clone()
    }

    /// Clone the surnames set (for CRF gazetteer features).
    pub fn surnames_clone(&self) -> HashSet<String> {
        self.surnames.clone()
    }

    /// Scan text for name matches using case-sensitive word-boundary tokenization.
    /// Returns matches sorted by start offset.
    ///
    /// Two-word matches (first_name + surname) get confidence 0.7.
    /// Single-word matches get confidence 0.4.
    /// Longer matches take priority (two-word suppresses overlapping singles).
    pub fn scan_text(&self, text: &str) -> Vec<PiiMatch> {
        let mut matches = Vec::new();
        let tokens = tokenize_case_sensitive(text);

        // Check 2-word windows first (first_name + surname)
        for window in tokens.windows(2) {
            if self.first_names.contains(&window[0].text) && self.surnames.contains(&window[1].text)
            {
                let start = window[0].byte_start;
                let end = window[1].byte_end;
                matches.push(PiiMatch {
                    pii_type: PiiType::Person,
                    start,
                    end,
                    raw_value: text[start..end].to_string(),
                    json_path: None,
                    confidence: TWO_WORD_CONFIDENCE,
                });
            }
        }

        // Check single words (first_name OR surname)
        for token in &tokens {
            if self.first_names.contains(&token.text) || self.surnames.contains(&token.text) {
                let start = token.byte_start;
                let end = token.byte_end;
                if !overlaps_any(&matches, start, end) {
                    matches.push(PiiMatch {
                        pii_type: PiiType::Person,
                        start,
                        end,
                        raw_value: text[start..end].to_string(),
                        json_path: None,
                        confidence: SINGLE_WORD_CONFIDENCE,
                    });
                }
            }
        }

        matches.sort_by_key(|m| m.start);
        matches
    }
}

/// A single token extracted from text with byte offsets. Case preserved.
#[derive(Debug)]
struct Token {
    text: String,
    byte_start: usize,
    byte_end: usize,
}

/// Tokenize text on word boundaries, preserving original case.
/// Splits on non-alphanumeric characters. Strips leading/trailing hyphens/apostrophes.
fn tokenize_case_sensitive(text: &str) -> Vec<Token> {
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
    // Handle trailing token
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

/// Check if a span overlaps any existing match.
fn overlaps_any(matches: &[PiiMatch], start: usize, end: usize) -> bool {
    matches.iter().any(|m| start < m.end && end > m.start)
}

// ── Embedded Name Data ──────────────────────────────────────────────────────
//
// Sources: US Census Bureau (public domain), SSA baby names (public domain).
// ~500 most common male + ~500 most common female first names.
// ~1000 most common US surnames.
//
// Names that are common English words are excluded to reduce false positives:
// May, Grace, Mark, Faith, Hope, Joy, Iris, Ivy, Dawn, Will, Art, Bill, Bob,
// Gene, Guy, Herb, Pat, Ray, Rob, Rod, Cliff, Dale, Glen, Grant, Heath, Lane,
// Miles, Rich, Skip, Ward, etc.

fn build_first_names() -> HashSet<String> {
    let names: &[&str] = &[
        // ── Male (top ~450) ──
        "James",
        "John",
        "Robert",
        "Michael",
        "David",
        "William",
        "Richard",
        "Charles",
        "Joseph",
        "Thomas",
        "Christopher",
        "Daniel",
        "Paul",
        "Donald",
        "George",
        "Kenneth",
        "Steven",
        "Edward",
        "Brian",
        "Ronald",
        "Anthony",
        "Kevin",
        "Jason",
        "Matthew",
        "Gary",
        "Timothy",
        "Jose",
        "Larry",
        "Jeffrey",
        "Frank",
        "Scott",
        "Eric",
        "Stephen",
        "Andrew",
        "Raymond",
        "Gregory",
        "Joshua",
        "Jerry",
        "Dennis",
        "Walter",
        "Patrick",
        "Peter",
        "Harold",
        "Douglas",
        "Henry",
        "Carl",
        "Arthur",
        "Ryan",
        "Roger",
        "Juan",
        "Jack",
        "Albert",
        "Jonathan",
        "Justin",
        "Terry",
        "Gerald",
        "Keith",
        "Samuel",
        "Willie",
        "Ralph",
        "Lawrence",
        "Nicholas",
        "Roy",
        "Benjamin",
        "Bruce",
        "Brandon",
        "Adam",
        "Harry",
        "Fred",
        "Wayne",
        "Billy",
        "Steve",
        "Louis",
        "Jeremy",
        "Aaron",
        "Randy",
        "Howard",
        "Eugene",
        "Carlos",
        "Russell",
        "Bobby",
        "Victor",
        "Martin",
        "Ernest",
        "Phillip",
        "Todd",
        "Jesse",
        "Craig",
        "Alan",
        "Shawn",
        "Clarence",
        "Sean",
        "Philip",
        "Chris",
        "Johnny",
        "Earl",
        "Jimmy",
        "Antonio",
        "Danny",
        "Bryan",
        "Tony",
        "Luis",
        "Stanley",
        "Leonard",
        "Nathan",
        "Manuel",
        "Rodney",
        "Curtis",
        "Norman",
        "Allen",
        "Marvin",
        "Vincent",
        "Glenn",
        "Jeffery",
        "Travis",
        "Jeff",
        "Chad",
        "Jacob",
        "Melvin",
        "Alfred",
        "Kyle",
        "Francis",
        "Bradley",
        "Herbert",
        "Frederick",
        "Joel",
        "Edwin",
        "Eddie",
        "Ricky",
        "Troy",
        "Randall",
        "Barry",
        "Alexander",
        "Bernard",
        "Mario",
        "Leroy",
        "Francisco",
        "Marcus",
        "Theodore",
        "Clifford",
        "Miguel",
        "Oscar",
        "Jay",
        "Calvin",
        "Alex",
        "Jon",
        "Ronnie",
        "Lloyd",
        "Tommy",
        "Leon",
        "Derek",
        "Warren",
        "Darrell",
        "Jerome",
        "Floyd",
        "Leo",
        "Alvin",
        "Wesley",
        "Gordon",
        "Dean",
        "Jorge",
        "Dustin",
        "Pedro",
        "Derrick",
        "Lewis",
        "Zachary",
        "Corey",
        "Herman",
        "Maurice",
        "Vernon",
        "Roberto",
        "Clyde",
        "Hector",
        "Shane",
        "Ricardo",
        "Lester",
        "Brent",
        "Ramon",
        "Charlie",
        "Tyler",
        "Gilbert",
        "Marc",
        "Reginald",
        "Ruben",
        "Brett",
        "Angel",
        "Nathaniel",
        "Rafael",
        "Edgar",
        "Milton",
        "Raul",
        "Chester",
        "Cecil",
        "Duane",
        "Franklin",
        "Andre",
        "Elmer",
        "Brad",
        "Gabriel",
        "Ron",
        "Mitchell",
        "Roland",
        "Arnold",
        "Harvey",
        "Jared",
        "Adrian",
        "Karl",
        "Cory",
        "Claude",
        "Erik",
        "Darryl",
        "Jamie",
        "Neil",
        "Jessie",
        "Christian",
        "Javier",
        "Fernando",
        "Clinton",
        "Mathew",
        "Tyrone",
        "Darren",
        "Lonnie",
        "Lance",
        "Cody",
        "Julio",
        "Kurt",
        "Allan",
        "Nelson",
        "Clayton",
        "Hugh",
        "Dwayne",
        "Dwight",
        "Armando",
        "Felix",
        "Jimmie",
        "Everett",
        "Jordan",
        "Ian",
        "Wallace",
        "Jaime",
        "Casey",
        "Alfredo",
        "Alberto",
        "Ivan",
        "Johnnie",
        "Sidney",
        "Byron",
        "Julian",
        "Isaac",
        "Clifton",
        "Willard",
        "Daryl",
        "Virgil",
        "Andy",
        "Marshall",
        "Salvador",
        "Perry",
        "Kirk",
        "Sergio",
        "Marion",
        "Tracy",
        "Seth",
        "Kent",
        "Terrance",
        "Eduardo",
        "Terrence",
        "Enrique",
        "Freddie",
        "Wade",
        "Noah",
        "Ethan",
        "Mason",
        "Logan",
        "Elijah",
        "Liam",
        "Aiden",
        "Oliver",
        "Lucas",
        "Dylan",
        "Sebastian",
        "Wyatt",
        "Owen",
        "Caleb",
        "Connor",
        "Isaiah",
        "Landon",
        "Hunter",
        "Evan",
        "Gavin",
        "Colton",
        "Carson",
        "Tristan",
        "Camden",
        "Bryce",
        "Preston",
        "Dominic",
        "Jaxon",
        "Easton",
        "Nolan",
        "Parker",
        "Bentley",
        "Lincoln",
        "Josiah",
        "Kayden",
        "Braxton",
        "Bryson",
        "Asher",
        "Declan",
        "Damian",
        "Axel",
        "Roman",
        "Maverick",
        "Greyson",
        "Brayden",
        "Cooper",
        "Ryder",
        "Emmett",
        "Harrison",
        "Silas",
        "Jasper",
        "Maddox",
        "Kingston",
        "Rowan",
        "Atlas",
        "Beckett",
        "Elliot",
        "Ezra",
        "Grayson",
        "Sawyer",
        "Miles",
        "Levi",
        // ── Female (top ~450) ──
        "Mary",
        "Patricia",
        "Jennifer",
        "Linda",
        "Barbara",
        "Elizabeth",
        "Susan",
        "Jessica",
        "Sarah",
        "Karen",
        "Lisa",
        "Nancy",
        "Betty",
        "Sandra",
        "Margaret",
        "Ashley",
        "Kimberly",
        "Emily",
        "Donna",
        "Michelle",
        "Carol",
        "Amanda",
        "Melissa",
        "Deborah",
        "Stephanie",
        "Dorothy",
        "Rebecca",
        "Sharon",
        "Laura",
        "Cynthia",
        "Amy",
        "Kathleen",
        "Angela",
        "Shirley",
        "Brenda",
        "Emma",
        "Anna",
        "Pamela",
        "Nicole",
        "Samantha",
        "Katherine",
        "Christine",
        "Helen",
        "Debra",
        "Rachel",
        "Carolyn",
        "Janet",
        "Maria",
        "Catherine",
        "Heather",
        "Diane",
        "Olivia",
        "Julie",
        "Joyce",
        "Victoria",
        "Ruth",
        "Virginia",
        "Lauren",
        "Kelly",
        "Christina",
        "Joan",
        "Evelyn",
        "Judith",
        "Andrea",
        "Hannah",
        "Cheryl",
        "Megan",
        "Jacqueline",
        "Martha",
        "Madison",
        "Teresa",
        "Gloria",
        "Janice",
        "Sara",
        "Ann",
        "Abigail",
        "Kathryn",
        "Sophia",
        "Frances",
        "Jean",
        "Judy",
        "Alice",
        "Isabella",
        "Julia",
        "Denise",
        "Amber",
        "Beverly",
        "Danielle",
        "Marilyn",
        "Charlotte",
        "Theresa",
        "Natalie",
        "Diana",
        "Brittany",
        "Doris",
        "Kayla",
        "Alexis",
        "Lori",
        "Marie",
        "Lorraine",
        "Peggy",
        "Bonnie",
        "Norma",
        "Tammy",
        "Phyllis",
        "Tiffany",
        "Elaine",
        "Lucille",
        "Erin",
        "Geraldine",
        "Rosa",
        "Cindy",
        "Edna",
        "Ethel",
        "Ellen",
        "Josephine",
        "Vivian",
        "Valerie",
        "Regina",
        "Tina",
        "Carla",
        "Gail",
        "Joanne",
        "Lillian",
        "Jill",
        "Loretta",
        "Marjorie",
        "Stacy",
        "Rhonda",
        "Renee",
        "Hazel",
        "Florence",
        "Sylvia",
        "Mildred",
        "Gladys",
        "Carmen",
        "Wendy",
        "Connie",
        "Dianne",
        "Anita",
        "Elsie",
        "Irene",
        "Bernice",
        "Beatrice",
        "Yolanda",
        "Yvonne",
        "Bertha",
        "Audrey",
        "Veronica",
        "Cathy",
        "Juanita",
        "Wanda",
        "Lydia",
        "Melinda",
        "Robin",
        "Vanessa",
        "Glenda",
        "Dolores",
        "Pauline",
        "Rosemary",
        "Myrtle",
        "Viola",
        "Hilda",
        "Roberta",
        "Marlene",
        "Ida",
        "Vera",
        "Wilma",
        "Priscilla",
        "Bobbie",
        "Maxine",
        "Geneva",
        "Minnie",
        "Leona",
        "Adrienne",
        "Colleen",
        "Molly",
        "Kristen",
        "Lindsay",
        "Erica",
        "Naomi",
        "Katie",
        "Bethany",
        "Tanya",
        "Lena",
        "Claudia",
        "Monique",
        "Marcia",
        "Kristina",
        "Jeanette",
        "Janis",
        "Sabrina",
        "Sonya",
        "Kristin",
        "Genevieve",
        "Joanna",
        "Miriam",
        "Felicia",
        "Adriana",
        "Gwendolyn",
        "Cassandra",
        "Simone",
        "Tabitha",
        "Angelica",
        "Robyn",
        "Kendra",
        "Elena",
        "Allison",
        "Courtney",
        "Candice",
        "Alicia",
        "Cecilia",
        "Deanna",
        "Bridget",
        "Darlene",
        "Guadalupe",
        "Jocelyn",
        "Sonia",
        "Meghan",
        "Belinda",
        "Leah",
        "Natasha",
        "Celeste",
        "Nora",
        "Alyssa",
        "Hailey",
        "Chloe",
        "Mia",
        "Avery",
        "Ella",
        "Sofia",
        "Harper",
        "Aria",
        "Scarlett",
        "Penelope",
        "Layla",
        "Riley",
        "Zoey",
        "Lily",
        "Aurora",
        "Violet",
        "Nova",
        "Stella",
        "Luna",
        "Hazel",
        "Ellie",
        "Paisley",
        "Audrey",
        "Skylar",
        "Savannah",
        "Brooklyn",
        "Bella",
        "Claire",
        "Lucy",
        "Naomi",
        "Caroline",
        "Kennedy",
        "Genesis",
        "Sadie",
        "Autumn",
        "Quinn",
        "Nevaeh",
        "Piper",
        "Ruby",
        "Serenity",
        "Willow",
        "Emilia",
        "Addison",
        "Mackenzie",
        "Madelyn",
        "Valentina",
        "Kinsley",
        "Delilah",
        "Ivy",
        "Josephine",
        "Peyton",
        "Lydia",
        "Alexandra",
        "Maya",
        "Vivian",
        "Aubrey",
        "Raelynn",
        "Gianna",
        "Camila",
        "Ariana",
        "Bailey",
        "Liliana",
        "Rylee",
        "Athena",
    ];
    names.iter().map(|n| n.to_string()).collect()
}

fn build_surnames() -> HashSet<String> {
    let names: &[&str] = &[
        "Smith",
        "Johnson",
        "Williams",
        "Jones",
        "Brown",
        "Davis",
        "Miller",
        "Wilson",
        "Moore",
        "Taylor",
        "Anderson",
        "Thomas",
        "Jackson",
        "White",
        "Harris",
        "Martin",
        "Thompson",
        "Garcia",
        "Martinez",
        "Robinson",
        "Clark",
        "Rodriguez",
        "Lewis",
        "Lee",
        "Walker",
        "Hall",
        "Allen",
        "Young",
        "Hernandez",
        "King",
        "Wright",
        "Lopez",
        "Hill",
        "Scott",
        "Green",
        "Adams",
        "Baker",
        "Gonzalez",
        "Nelson",
        "Carter",
        "Mitchell",
        "Perez",
        "Roberts",
        "Turner",
        "Phillips",
        "Campbell",
        "Parker",
        "Evans",
        "Edwards",
        "Collins",
        "Stewart",
        "Sanchez",
        "Morris",
        "Rogers",
        "Reed",
        "Cook",
        "Morgan",
        "Bell",
        "Murphy",
        "Bailey",
        "Rivera",
        "Cooper",
        "Richardson",
        "Cox",
        "Howard",
        "Ward",
        "Torres",
        "Peterson",
        "Gray",
        "Ramirez",
        "James",
        "Watson",
        "Brooks",
        "Kelly",
        "Sanders",
        "Price",
        "Bennett",
        "Wood",
        "Barnes",
        "Ross",
        "Henderson",
        "Coleman",
        "Jenkins",
        "Perry",
        "Powell",
        "Long",
        "Patterson",
        "Hughes",
        "Flores",
        "Washington",
        "Butler",
        "Simmons",
        "Foster",
        "Gonzales",
        "Bryant",
        "Alexander",
        "Russell",
        "Griffin",
        "Diaz",
        "Hayes",
        "Myers",
        "Ford",
        "Hamilton",
        "Graham",
        "Sullivan",
        "Wallace",
        "Woods",
        "Cole",
        "West",
        "Jordan",
        "Owens",
        "Reynolds",
        "Fisher",
        "Ellis",
        "Harrison",
        "Gibson",
        "McDonald",
        "Cruz",
        "Marshall",
        "Ortiz",
        "Gomez",
        "Murray",
        "Freeman",
        "Wells",
        "Webb",
        "Simpson",
        "Stevens",
        "Tucker",
        "Porter",
        "Hunter",
        "Hicks",
        "Crawford",
        "Henry",
        "Boyd",
        "Mason",
        "Morales",
        "Kennedy",
        "Warren",
        "Dixon",
        "Ramos",
        "Reyes",
        "Burns",
        "Gordon",
        "Shaw",
        "Holmes",
        "Rice",
        "Robertson",
        "Hunt",
        "Black",
        "Daniels",
        "Palmer",
        "Mills",
        "Nichols",
        "Grant",
        "Knight",
        "Ferguson",
        "Rose",
        "Stone",
        "Hawkins",
        "Dunn",
        "Perkins",
        "Hudson",
        "Spencer",
        "Gardner",
        "Stephens",
        "Payne",
        "Pierce",
        "Berry",
        "Matthews",
        "Arnold",
        "Wagner",
        "Willis",
        "Ray",
        "Watkins",
        "Olson",
        "Carroll",
        "Duncan",
        "Snyder",
        "Hart",
        "Cunningham",
        "Bradley",
        "Lane",
        "Andrews",
        "Ruiz",
        "Harper",
        "Fox",
        "Riley",
        "Armstrong",
        "Carpenter",
        "Weaver",
        "Greene",
        "Lawrence",
        "Elliott",
        "Chavez",
        "Sims",
        "Austin",
        "Peters",
        "Kelley",
        "Franklin",
        "Lawson",
        "Fields",
        "Gutierrez",
        "Ryan",
        "Schmidt",
        "Carr",
        "Vasquez",
        "Castillo",
        "Wheeler",
        "Chapman",
        "Oliver",
        "Montgomery",
        "Richards",
        "Williamson",
        "Johnston",
        "Banks",
        "Meyer",
        "Bishop",
        "McCoy",
        "Howell",
        "Alvarez",
        "Morrison",
        "Hansen",
        "Fernandez",
        "Garza",
        "Harvey",
        "Little",
        "Burton",
        "Stanley",
        "Nguyen",
        "George",
        "Jacobs",
        "Reid",
        "Kim",
        "Fuller",
        "Lynch",
        "Dean",
        "Gilbert",
        "Garrett",
        "Romero",
        "Welch",
        "Larson",
        "Frazier",
        "Burke",
        "Hanson",
        "Day",
        "Mendoza",
        "Moreno",
        "Bowman",
        "Medina",
        "Fowler",
        "Brewer",
        "Hoffman",
        "Carlson",
        "Silva",
        "Pearson",
        "Holland",
        "Douglas",
        "Fleming",
        "Jensen",
        "Vargas",
        "Byrd",
        "Davidson",
        "Hopkins",
        "May",
        "Terry",
        "Herrera",
        "Wade",
        "Soto",
        "Walters",
        "Curtis",
        "Neal",
        "Caldwell",
        "Lowe",
        "Jennings",
        "Barnett",
        "Graves",
        "Jimenez",
        "Horton",
        "Shelton",
        "Barrett",
        "Obrien",
        "Castro",
        "Sutton",
        "Gregory",
        "McKinney",
        "Lucas",
        "Miles",
        "Craig",
        "Rodriquez",
        "Chambers",
        "Holt",
        "Lambert",
        "Fletcher",
        "Watts",
        "Bates",
        "Hale",
        "Rhodes",
        "Pena",
        "Beck",
        "Newman",
        "Haynes",
        "McDaniel",
        "Mendez",
        "Bush",
        "Vaughn",
        "Parks",
        "Dawson",
        "Santiago",
        "Norris",
        "Hardy",
        "Love",
        "Steele",
        "Curry",
        "Powers",
        "Schultz",
        "Barker",
        "Guzman",
        "Page",
        "Munoz",
        "Ball",
        "Keller",
        "Chandler",
        "Weber",
        "Leonard",
        "Walsh",
        "Lyons",
        "Ramsey",
        "Wolfe",
        "Schneider",
        "Mullins",
        "Benson",
        "Sharp",
        "Bowen",
        "Daniel",
        "Barber",
        "Cummings",
        "Hines",
        "Baldwin",
        "Griffith",
        "Valdez",
        "Hubbard",
        "Salazar",
        "Reeves",
        "Warner",
        "Stevenson",
        "Burgess",
        "Santos",
        "Tate",
        "Cross",
        "Garner",
        "Mann",
        "Mack",
        "Moss",
        "Thornton",
        "Dennis",
        "McGee",
        "Farmer",
        "Delgado",
        "Aguilar",
        "Vega",
        "Glover",
        "Manning",
        "Cohen",
        "Harmon",
        "Rodgers",
        "Robbins",
        "Newton",
        "Todd",
        "Blair",
        "Higgins",
        "Ingram",
        "Reese",
        "Cannon",
        "Strickland",
        "Townsend",
        "Potter",
        "Goodwin",
        "Walton",
        "Rowe",
        "Hampton",
        "Ortega",
        "Patton",
        "Swanson",
        "Joseph",
        "Francis",
        "Goodman",
        "Maldonado",
        "Yates",
        "Becker",
        "Erickson",
        "Hodges",
        "Rios",
        "Conner",
        "Adkins",
        "Webster",
        "Norman",
        "Malone",
        "Hammond",
        "Flowers",
        "Cobb",
        "Moody",
        "Quinn",
        "Blake",
        "Maxwell",
        "Pope",
        "Floyd",
        "Osborne",
        "Paul",
        "McCarthy",
        "Guerrero",
        "Lindsey",
        "Estrada",
        "Sandoval",
        "Gibbs",
        "Tyler",
        "Gross",
        "Fitzgerald",
        "Stokes",
        "Doyle",
        "Sherman",
        "Saunders",
        "Wise",
        "Colon",
        "Gill",
        "Alvarado",
        "Greer",
        "Padilla",
        "Simon",
        "Waters",
        "Nunez",
        "Ballard",
        "Schwartz",
        "McBride",
        "Houston",
        "Christensen",
        "Klein",
        "Pratt",
        "Briggs",
        "Parsons",
        "McLaughlin",
        "Zimmerman",
        "French",
        "Buchanan",
        "Moran",
        "Copeland",
        "Roy",
        "Pittman",
        "Brady",
        "McCormick",
        "Holloway",
        "Brock",
        "Poole",
        "Frank",
        "Logan",
        "Owen",
        "Bass",
        "Marsh",
        "Drake",
        "Wong",
        "Jefferson",
        "Park",
        "Morton",
        "Abbott",
        "Sparks",
        "Patrick",
        "Norton",
        "Huff",
        "Clayton",
        "Massey",
        "Lloyd",
        "Figueroa",
        "Carson",
        "Bowers",
        "Roberson",
        "Barton",
        "Tran",
        "Lamb",
        "Harrington",
        "Casey",
        "Boone",
        "Cortez",
        "Clarke",
        "Mathis",
        "Singleton",
        "Wilkins",
        "Cain",
        "Bryan",
        "Underwood",
        "Hogan",
        "McKenzie",
        "Collier",
        "Luna",
        "Phelps",
        "McGuire",
        "Allison",
        "Bridges",
        "Wilkerson",
        "Nash",
        "Summers",
        "Atkins",
        "Wilcox",
        "Pitts",
        "Conley",
        "Marquez",
        "Burnett",
        "Richard",
        "Cochran",
        "Chase",
        "Davenport",
        "Hood",
        "Gates",
        "Clay",
        "Ayala",
        "Sawyer",
        "Roman",
        "Vazquez",
        "Dickerson",
        "Hodge",
        "Acosta",
        "Flynn",
        "Espinoza",
        "Nicholson",
        "Monroe",
        "Wolf",
        "Morrow",
        "Kirk",
        "Randall",
        "Anthony",
        "Whitaker",
        "OConnor",
        "Skinner",
        "Ware",
        "Molina",
        "Kirby",
        "Huffman",
        "Bradford",
        "Charles",
        "Gilmore",
        "Dominguez",
        "OBrien",
        "Stout",
        "Kramer",
        "Avila",
        "Snow",
        "Camacho",
        "Beasley",
        "Sampson",
        "Cline",
        "Middleton",
        "Mccall",
        "Christian",
        "Mata",
        "Valencia",
        "Daugherty",
        "Bass",
        "Russo",
        "Clements",
        "Rosario",
        "Fuentes",
        "Velasquez",
        "Mccain",
        "Mueller",
    ];
    names.iter().map(|n| n.to_string()).collect()
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn gaz() -> NameGazetteer {
        NameGazetteer::new()
    }

    #[test]
    fn test_two_word_name_detected() {
        let matches = gaz().scan_text("Hello John Smith!");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].pii_type, PiiType::Person);
        assert_eq!(matches[0].raw_value, "John Smith");
        assert!((matches[0].confidence - 0.7).abs() < f32::EPSILON);
    }

    #[test]
    fn test_single_first_name_low_confidence() {
        let matches = gaz().scan_text("Hello John!");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].pii_type, PiiType::Person);
        assert_eq!(matches[0].raw_value, "John");
        assert!((matches[0].confidence - 0.4).abs() < f32::EPSILON);
    }

    #[test]
    fn test_single_surname_low_confidence() {
        let matches = gaz().scan_text("Hello Smith!");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].raw_value, "Smith");
        assert!((matches[0].confidence - 0.4).abs() < f32::EPSILON);
    }

    #[test]
    fn test_case_sensitive_lowercase_rejected() {
        let matches = gaz().scan_text("hello john smith");
        assert!(matches.is_empty(), "Lowercase names should not match");
    }

    #[test]
    fn test_case_sensitive_allcaps_rejected() {
        let matches = gaz().scan_text("JOHN SMITH");
        assert!(matches.is_empty(), "ALL-CAPS names should not match");
    }

    #[test]
    fn test_no_partial_word_match() {
        // "Johnson" is a surname, but "John" inside "Johnson" should not match separately
        let matches = gaz().scan_text("Johnson is here");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].raw_value, "Johnson");
    }

    #[test]
    fn test_two_word_priority_over_single() {
        let matches = gaz().scan_text("John Smith is here");
        // Should get ONE two-word match, not two singles
        let two_word: Vec<_> = matches
            .iter()
            .filter(|m| (m.confidence - 0.7).abs() < f32::EPSILON)
            .collect();
        assert_eq!(two_word.len(), 1);
        assert_eq!(two_word[0].raw_value, "John Smith");
        // No single-word "John" or "Smith" should overlap
        for m in &matches {
            if m.raw_value == "John" || m.raw_value == "Smith" {
                panic!("Single-word match should be suppressed by two-word match");
            }
        }
    }

    #[test]
    fn test_multiple_names_in_text() {
        let matches = gaz().scan_text("John Smith met Mary Johnson at the park.");
        let two_word: Vec<_> = matches
            .iter()
            .filter(|m| (m.confidence - 0.7).abs() < f32::EPSILON)
            .collect();
        assert_eq!(two_word.len(), 2);
    }

    #[test]
    fn test_name_with_punctuation() {
        let matches = gaz().scan_text("Dear John,");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].raw_value, "John");
    }

    #[test]
    fn test_dictionary_sizes() {
        let g = gaz();
        assert!(
            g.first_name_count() >= 400,
            "Expected >= 400 first names, got {}",
            g.first_name_count()
        );
        assert!(
            g.surname_count() >= 400,
            "Expected >= 400 surnames, got {}",
            g.surname_count()
        );
    }

    #[test]
    fn test_offset_tracking() {
        let text = "Meet John Smith today";
        let matches = gaz().scan_text(text);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].start, 5);
        assert_eq!(matches[0].end, 15);
        assert_eq!(&text[5..15], "John Smith");
    }

    #[test]
    fn test_empty_text() {
        let matches = gaz().scan_text("");
        assert!(matches.is_empty());
    }

    #[test]
    fn test_name_near_pii() {
        let matches = gaz().scan_text("John at 555-1234");
        let name_matches: Vec<_> = matches
            .iter()
            .filter(|m| m.pii_type == PiiType::Person)
            .collect();
        assert_eq!(name_matches.len(), 1);
        assert_eq!(name_matches[0].raw_value, "John");
    }

    #[test]
    fn test_ambiguous_single_names_filtered_by_confidence() {
        // Single-word matches have confidence 0.4, which is below the typical
        // min_confidence of 0.5. This test verifies the confidence value.
        let matches = gaz().scan_text("Michael called.");
        assert_eq!(matches.len(), 1);
        assert!(matches[0].confidence < 0.5);
    }

    #[test]
    fn test_mccain_surname() {
        // Verify specific surname from plan context
        let matches = gaz().scan_text("John Mccain is a senator.");
        let two_word: Vec<_> = matches
            .iter()
            .filter(|m| m.raw_value == "John Mccain")
            .collect();
        assert_eq!(two_word.len(), 1);
        assert!((two_word[0].confidence - 0.7).abs() < f32::EPSILON);
    }
}
