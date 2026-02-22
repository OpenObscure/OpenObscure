# PII Test Data Collection Prompts

Prompts for generating or downloading test data for all OpenObscure detection capabilities beyond text-based structured PII. Each section specifies what to collect, where to source it, folder structure, and validation criteria.

---

## 1. Visual PII — Images with Faces

**Folder**: `test/data/input/Visual_PII/Faces/`

**What to collect** (20-30 images):
- Single-face portrait photos (frontal, varied lighting)
- Multi-face group photos (2-5 people)
- Side-profile and partially occluded faces
- Small faces in wide-angle/landscape photos (face < 20% of image area)
- Large face selfies (face > 80% of image area — triggers full-image blur)
- Faces at various distances (close-up, medium, far)

**Formats**: PNG, JPEG, WebP (at least 5 of each format)

**Sources (real, open-license)**:
- [Unsplash](https://unsplash.com) — search "portrait", "group photo", "crowd" (Unsplash License, free commercial use)
- [Pexels](https://pexels.com) — search "headshot", "family photo", "office meeting" (Pexels License, free)
- [WIDER FACE dataset](http://shuoyang1213.me/WIDERFACE/) — academic face detection benchmark (non-commercial)
- [LFW (Labeled Faces in the Wild)](http://vis-www.cs.umass.edu/lfw/) — 13,000+ labeled face images (research use)
- [UTKFace](https://susanqq.github.io/UTKFace/) — 20,000+ face images with age/gender/ethnicity labels

**File naming convention**:
```
face_single_frontal_01.jpg
face_single_profile_01.png
face_group_3people_01.jpg
face_small_landscape_01.webp
face_large_selfie_01.jpg     (face > 80% area — expect full-image blur)
face_occluded_hat_01.png
```

**Validation**: Each image should trigger BlazeFace (128x128 input) or SCRFD (640x640 input). Log expected face count per image for ground truth.

---

## 2. Visual PII — Screenshots with PII Text

**Folder**: `test/data/input/Visual_PII/Screenshots/`

**What to collect** (15-20 images):
- Desktop screenshots showing email inboxes with visible email addresses
- Browser screenshots showing web forms with PII fields filled in (name, SSN, phone)
- Terminal/CLI screenshots showing log output with IP addresses, API keys
- Mobile screenshots of messaging apps with phone numbers or names visible
- Screenshots of spreadsheets/tables containing PII columns (SSN, CC, email)
- IDE/code editor screenshots showing hardcoded API keys or credentials

**Formats**: PNG (primary, most screenshots are PNG), JPEG

**How to generate**:
1. Open a text editor or browser, type synthetic PII data (from our existing `.txt` files)
2. Take a screenshot (Cmd+Shift+4 on macOS, Snipping Tool on Windows)
3. Key: screenshot must match common screen resolutions for screen_guard detection:
   - Desktop: 1920x1080, 2560x1440, 1366x768, 1440x900, 2880x1800 (Retina)
   - Mobile: 1170x2532 (iPhone 14), 1284x2778 (iPhone Pro Max), 1080x2340 (Android)
4. Leave macOS/Windows status bar visible (triggers status-bar uniformity heuristic, variance < 50)

**File naming convention**:
```
screenshot_email_inbox_1920x1080.png
screenshot_web_form_ssn_2560x1440.png
screenshot_terminal_ipv4_apikey_1440x900.png
screenshot_mobile_chat_phone_1170x2532.png
screenshot_spreadsheet_mixed_pii_2880x1800.png
screenshot_ide_hardcoded_keys_1920x1080.png
```

**Validation**: Screen guard should flag these via resolution match + status bar heuristic. OCR pipeline should extract PII text and detect it.

---

## 3. Visual PII — ID Documents and Cards

**Folder**: `test/data/input/Visual_PII/Documents/`

**What to collect** (10-15 images):
- Synthetic/sample driver's license images
- Sample passport photo pages (fictional data)
- Credit card photos (use obviously fake numbers like 4111-1111-1111-1111)
- Social Security card mockups (fictional SSNs from our test data)
- Insurance card samples
- Business cards with name, email, phone

**Sources (synthetic only — never use real documents)**:
- Generate with tools like [FakeID](https://github.com/topics/fake-id-generator) generators
- Use HTML/CSS templates to create realistic-looking but clearly fake documents
- Download sample/template documents from government websites (IRS example forms)
- [Nightfall sample images](https://playground.nightfall.ai) — DLP test images of IDs

**File naming convention**:
```
doc_drivers_license_sample_01.jpg
doc_passport_sample_01.png
doc_credit_card_mock_01.jpg
doc_ssn_card_mock_01.png
doc_insurance_card_01.jpg
doc_business_card_01.png
```

**Validation**: OCR should extract text → PII scanner should detect CC numbers, SSNs, names, etc. Face detector should find the portrait photo on license/passport.

---

## 4. Visual PII — EXIF Metadata

**Folder**: `test/data/input/Visual_PII/EXIF/`

**What to collect** (10 images):
- Photos taken with smartphones (contain GPS coordinates in EXIF)
- Photos with camera make/model, lens info, timestamps
- Photos with embedded GPS lat/long (publicly available geotagged images)
- Screenshots with EXIF `Software` field set to known screenshot tools:
  `Snipping Tool`, `gnome-screenshot`, `Spectacle`, `Flameshot`, `ShareX`, `CleanShot`, `screencapture`

**Sources**:
- Take photos with a smartphone (GPS enabled) of public places
- [Flickr Creative Commons](https://www.flickr.com/creativecommons/) — many photos retain EXIF GPS data
- [ExifTool sample images](https://exiftool.org/sample_images.html) — test images with rich EXIF
- Generate: Use `exiftool` CLI to inject EXIF GPS into existing images:
  ```bash
  exiftool -GPSLatitude=40.6892 -GPSLongitude=-74.0445 \
           -GPSLatitudeRef=N -GPSLongitudeRef=W \
           -Software="Snipping Tool" photo.jpg
  ```

**File naming convention**:
```
exif_gps_smartphone_01.jpg        (has GPS lat/long)
exif_gps_camera_dslr_01.jpg       (has GPS + camera model)
exif_screenshot_snipping_01.png   (Software: "Snipping Tool")
exif_screenshot_sharex_01.png     (Software: "ShareX")
exif_no_gps_stripped_01.jpg       (control: no EXIF, expect pass-through)
```

**Validation**: Pipeline should strip ALL EXIF before forwarding. GPS coordinates in EXIF are PII.

---

## 5. Visual PII — NSFW Content

**Folder**: `test/data/input/Visual_PII/NSFW/`

**What to collect** (5-10 images):
- **Safe images** (control): fully clothed people, landscapes, objects (expect no NSFW trigger)
- **Borderline images**: swimwear, athletic wear (test threshold sensitivity)
- **NSFW-positive images**: content that NudeNet 320n classifies as exposed (for testing blur)
  - Categories detected: BUTTOCKS_EXPOSED, FEMALE_BREAST_EXPOSED, FEMALE_GENITALIA_EXPOSED, ANUS_EXPOSED, MALE_GENITALIA_EXPOSED

**Sources**:
- Safe controls: Unsplash, Pexels (beach, sports, fashion photography)
- NSFW test data: Use [NudeNet's own test dataset](https://github.com/notAI-tech/NudeNet) for evaluation
- Academic datasets: [NSFW Detection datasets on HuggingFace](https://huggingface.co/datasets?search=nsfw)
  - Note: Handle with care, store in restricted access folder

**File naming convention**:
```
nsfw_safe_portrait_01.jpg         (control — expect NO blur)
nsfw_safe_landscape_01.jpg        (control — expect NO blur)
nsfw_borderline_swimwear_01.jpg   (borderline — test threshold)
nsfw_positive_01.jpg              (expect full-image blur, sigma=30)
```

**Validation**: NSFW-positive images should trigger Phase 0 full-image blur (sigma=30). Pipeline should skip face/OCR phases when NSFW is detected. Safe controls should pass through to face/OCR phases.

---

## 6. Audio PII — Spoken PII in Voice Recordings

**Folder**: `test/data/input/Audio_PII/`

**What to collect** (15-20 audio files):

### Category A: Single PII type per clip
- Someone reading a credit card number aloud
- Someone stating their SSN ("My social security number is...")
- Someone dictating a phone number
- Someone spelling out an email address
- Someone reading an address aloud

### Category B: Multi-PII conversational clips
- Simulated customer service call (name, SSN, card number, phone, address)
- Doctor-patient intake conversation (name, DOB, SSN, medications, conditions)
- Job application phone screening (name, SSN, address, employment history)

### Category C: Edge cases
- Background noise with spoken PII (test Whisper robustness)
- Accented English with PII
- Fast speech with PII
- Whispered PII (low volume)

**Formats**: WAV (primary), MP3, OGG Vorbis, WebM/Opus (at least 2 files per format)

**How to generate**:
1. **Text-to-Speech** (fastest):
   ```bash
   # macOS
   say -o spoken_ssn.wav "My social security number is 287 65 4321"
   say -o spoken_cc.wav "Please charge my Visa card 4532 0151 1283 0366"
   say -o spoken_phone.wav "Call me back at 415 555 0132"
   say -o spoken_email.wav "Send it to john dot doe at gmail dot com"

   # Convert to other formats with ffmpeg
   ffmpeg -i spoken_ssn.wav spoken_ssn.mp3
   ffmpeg -i spoken_ssn.wav -c:a libvorbis spoken_ssn.ogg
   ffmpeg -i spoken_ssn.wav -c:a libopus spoken_ssn.webm
   ```
2. **Self-recorded** (more realistic): Record yourself reading PII from the test `.txt` files
3. **AI TTS services**: Use ElevenLabs, Google Cloud TTS, or Amazon Polly for varied voices

**Sources (open datasets with spoken PII)**:
- [LibriSpeech](https://www.openslr.org/12/) — clean audiobook speech (inject PII into transcripts for ground truth)
- [Mozilla Common Voice](https://commonvoice.mozilla.org/) — crowdsourced voice clips (no PII, but useful for noise/accent controls)
- Generate custom clips using the macOS `say` command or `pyttsx3` Python library

**File naming convention**:
```
audio_ssn_single_wav.wav
audio_cc_visa_single_mp3.mp3
audio_phone_us_single_ogg.ogg
audio_email_single_webm.webm
audio_customer_service_call_wav.wav    (multi-PII)
audio_medical_intake_wav.wav           (multi-PII + health terms)
audio_noisy_background_ssn_wav.wav     (edge case)
audio_accented_phone_mp3.mp3           (edge case)
```

**Validation**: Whisper should transcribe → PII scanner should detect entities in transcript. Affected audio segments should be silenced (default) or beeped. Output duration should match input duration.

---

## 7. Agent Tool Results — JSON Payloads with Embedded PII

**Folder**: `test/data/input/Agent_Tool_Results/`

**What to collect** (10-15 JSON files):

### Category A: Anthropic message format
```json
{
  "role": "assistant",
  "content": [
    {"type": "text", "text": "The customer's SSN is 287-65-4321 and their email is john@example.com."},
    {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "<base64_of_face_image>"}},
    {"type": "audio", "source": {"type": "base64", "media_type": "audio/wav", "data": "<base64_of_spoken_pii>"}}
  ]
}
```

### Category B: OpenAI message format
```json
{
  "role": "assistant",
  "content": [
    {"type": "text", "text": "Found API key sk-ant-api03-abc123def456 in the config file."},
    {"type": "image_url", "image_url": {"url": "data:image/jpeg;base64,<base64_data>"}},
    {"type": "input_audio", "input_audio": {"data": "<base64_data>", "format": "wav"}}
  ]
}
```

### Category C: Tool use results with nested PII
```json
{
  "role": "tool",
  "tool_use_id": "toolu_01ABC",
  "content": "{\"user\": {\"name\": \"James Henderson\", \"ssn\": \"287-65-4321\", \"email\": \"j.henderson@company.com\", \"ip\": \"203.0.113.42\"}}"
}
```

### Category D: Deeply nested JSON (test 2-level depth limit)
```json
{
  "result": "{\"data\": \"{\\\"inner_ssn\\\": \\\"154-32-8877\\\", \\\"inner_email\\\": \\\"nested@test.com\\\"}\"}"
}
```

### Category E: Code fences in assistant messages
````json
{
  "role": "assistant",
  "content": "Here's the config:\n```yaml\napi_key: sk-ant-api03-TestKey123456789012345678901234567890abc\ndb_host: 10.0.0.55\n```\nDon't commit this."
}
````

### Category F: Mixed multimodal (text + image + audio in single message)

**How to generate**:
1. Take images from `Visual_PII/` folder, base64-encode them:
   ```bash
   base64 -i face_single_frontal_01.jpg | tr -d '\n' > face_b64.txt
   ```
2. Take audio from `Audio_PII/` folder, base64-encode:
   ```bash
   base64 -i audio_ssn_single_wav.wav | tr -d '\n' > audio_b64.txt
   ```
3. Compose JSON files using the templates above, inserting real base64 data

**File naming convention**:
```
agent_anthropic_text_pii.json
agent_anthropic_image_face.json
agent_anthropic_audio_spoken_ssn.json
agent_openai_text_apikey.json
agent_openai_image_screenshot.json
agent_tool_result_nested_pii.json
agent_deeply_nested_json.json
agent_code_fence_credentials.json
agent_multimodal_mixed.json
```

**Validation**: JSON scanner should traverse all string values, detect PII at correct json_path (e.g., `content[0].text`). Nested JSON should be parsed up to depth 2. Base64 images/audio should be decoded and processed through their respective pipelines.

---

## 8. Multilingual PII — Country-Specific Identifiers

**Folder**: `test/data/input/Multilingual_PII/`

**What to collect** (1 file per language, 8 files total):

### 8a. Spanish (`es_Spanish_PII.txt`)
```
Mi DNI es 12345678Z.                             (8 digits + check letter, mod-23)
Mi NIE es X1234567L.                              (X/Y/Z + 7 digits + check letter)
Llámame al +34 612 345 678.                       (Spanish mobile)
Mi IBAN es ES91 2100 0418 4502 0005 1332.         (ES + mod-97 validation)
Mi número de seguridad social es 281234567890.    (12 digits)
```

### 8b. German (`de_German_PII.txt`)
```
Meine Steuer-ID ist 12345678911.                  (11-digit tax ID, check digit)
Rufen Sie mich an: +49 30 1234 5678.              (German landline)
Meine IBAN lautet DE89 3704 0044 0532 0130 00.    (DE + mod-97)
Mobilnummer: +49 170 1234567.                     (German mobile)
```

### 8c. French (`fr_French_PII.txt`)
```
Mon numéro NIR est 1 85 01 75 115 005 42.         (13-digit national registration)
Appelez-moi au +33 1 23 45 67 89.                 (French landline)
Mon IBAN est FR76 3000 6000 0112 3456 7890 189.    (FR + mod-97)
Numéro de mobile: 06 12 34 56 78.                  (French mobile, 0X format)
```

### 8d. Portuguese (`pt_Portuguese_PII.txt`)
```
Meu CPF é 123.456.789-09.                          (Brazilian, 11 digits, Luhn variant)
O CNPJ da empresa é 12.345.678/0001-95.            (Brazilian business, 14 digits)
Ligue para +55 11 91234-5678.                      (Brazilian mobile)
Telefone Portugal: +351 912 345 678.                (Portuguese mobile)
```

### 8e. Japanese (`ja_Japanese_PII.txt`)
```
マイナンバーは 123456789012 です。                    (My Number, 12 digits + check)
電話番号は +81 3-1234-5678 です。                    (Japanese landline)
携帯: 090-1234-5678                                 (Japanese mobile)
パスポート番号: AB1234567                             (2 letters + 7 digits)
```

### 8f. Korean (`ko_Korean_PII.txt`)
```
주민등록번호는 850101-1234567 입니다.                 (RRN, 13 digits + check digit)
전화번호: +82 2-1234-5678                            (Korean landline)
휴대폰: 010-1234-5678                               (Korean mobile)
```

### 8g. Chinese (`zh_Chinese_PII.txt`)
```
身份证号码：110101199001011234                        (18-digit citizen ID + check)
手机号码：+86 138 1234 5678                          (Chinese mobile)
固定电话：+86 10 1234 5678                           (Chinese landline)
```

### 8h. Arabic (`ar_Arabic_PII.txt`)
```
رقم الهوية السعودية: 1234567890                      (Saudi National ID, 10 digits)
رقم هوية الإمارات: 784-1990-1234567-8                (UAE Emirates ID, 15 digits)
هاتف: +966 50 123 4567                              (Saudi mobile)
هاتف الإمارات: +971 50 123 4567                      (UAE mobile)
```

**Sources**:
- [AI4Privacy pii-masking-200k](https://huggingface.co/datasets/ai4privacy/pii-masking-200k) — multilingual (EN, FR, DE, IT, NL, ES)
- [Gretel synthetic_pii_finance_multilingual](https://huggingface.co/datasets/gretelai/synthetic_pii_finance_multilingual) — EN, ES, SV, DE, IT, NL, FR
- Python `Faker` with locale providers: `Faker('es_ES')`, `Faker('de_DE')`, `Faker('fr_FR')`, `Faker('pt_BR')`, `Faker('ja_JP')`, `Faker('ko_KR')`, `Faker('zh_CN')`, `Faker('ar_SA')`
- Generate with check-digit validators to ensure numbers pass OpenObscure's validation

**Validation**: Each national ID must pass the scanner's check-digit validation (mod-23 for Spanish DNI, mod-97 for IBAN, Luhn variant for Brazilian CPF, etc.). Include both valid and intentionally invalid samples to test rejection.

---

## 9. Code and Config PII — PII in Source Code and Configuration

**Folder**: `test/data/input/Code_Config_PII/`

**What to collect** (8-10 files):

### 9a. Python source with hardcoded credentials (`sample_python.py`)
```python
import openai
client = openai.Client(api_key="sk-proj-abc123def456ghi789jklmnopqrstuvwxyz")
ANTHROPIC_KEY = "sk-ant-api03-HardcodedKey123456789012345678901234567890abc"
DATABASE_URL = "postgresql://admin:P@ssw0rd@10.0.0.55:5432/production"
```

### 9b. YAML/TOML config (`sample_config.yaml`)
```yaml
api_keys:
  anthropic: sk-ant-api03-ConfigKey123456789012345678901234567890abc
  aws_access_key: AKIAIOSFODNN7EXAMPLE
database:
  host: 10.0.0.55
  password: SuperSecret123!
slack:
  bot_token: xoxb-123456789012-1234567890123-AbCdEfGhIjKlMnOp
```

### 9c. `.env` file (`sample.env`)
```
ANTHROPIC_API_KEY=sk-ant-api03-EnvFileKey123456789012345678901234567890abc
AWS_ACCESS_KEY_ID=AKIA2OGYBAH6CEXAMPLE
GITHUB_TOKEN=ghp_EnvFileTokenThatShouldNeverBeCommitted12
DB_HOST=10.0.0.55
ADMIN_EMAIL=admin@company.com
ADMIN_PHONE=415-555-0132
```

### 9d. Server log file (`sample_access.log`)
```
[2026-02-18 14:32:01] 203.0.113.42 - GET /api/users/287-65-4321 - 200
[2026-02-18 14:32:02] 198.51.100.17 - POST /api/payment - card=4532015112830366 - 200
[2026-02-18 14:32:03] 2001:db8:85a3::8a2e:370:7334 - GET /api/profile?email=user@test.com - 200
```

### 9e. Markdown with code fences (`sample_docs.md`)
````markdown
# API Setup Guide
Configure your client:
```python
client = anthropic.Client(api_key="sk-ant-api03-DocExample123456789012345678901234567890abc")
```
Server runs at `10.0.0.55:8080`. Contact admin@company.com for access.
````

### 9f. JSON config (`sample_terraform.json`)
```json
{
  "provider": {"aws": {"access_key": "AKIAIOSFODNN7EXAMPLE", "region": "us-east-1"}},
  "resource": {"instance": {"private_ip": "10.0.0.55", "mac": "00:1A:2B:3C:4D:5E"}}
}
```

### 9g. Shell script (`sample_deploy.sh`)
```bash
#!/bin/bash
export API_KEY="sk-ant-api03-DeployKey123456789012345678901234567890abc"
curl -H "Authorization: Bearer $API_KEY" https://api.anthropic.com/v1/messages
ssh admin@10.0.0.55 "systemctl restart app"
echo "Deploying for user john.doe@company.com (SSN: 287-65-4321)"
```

### 9h. Git diff output (`sample_git_diff.txt`)
```diff
- ANTHROPIC_API_KEY=sk-ant-api03-OldKey123456789012345678901234567890abc
+ ANTHROPIC_API_KEY=sk-ant-api03-NewKey987654321098765432109876543210xyz
  DB_HOST=10.0.0.55
```

**Sources**: Generate manually using patterns from existing test data. Model after real-world config file structures.

**Validation**: Code fence detection should identify fenced regions. PII scanner should detect API keys, IPs, emails, SSNs, etc. within both fenced and unfenced code/config content.

---

## 10. Structured Data PII — CSV, TSV, and Tabular Formats

**Folder**: `test/data/input/Structured_Data_PII/`

**What to collect** (5-8 files):

### 10a. Employee roster CSV (`employee_roster.csv`)
```csv
Name,SSN,Email,Phone,Department,Office_IP,Badge_MAC
James Henderson,287-65-4321,j.henderson@company.com,(206) 555-0312,Engineering,10.0.1.50,3C:22:FB:7A:B1:90
Sarah Mitchell,378-22-9104,s.mitchell@company.com,(503) 555-0147,Engineering,10.0.1.51,48:D7:05:F3:A2:81
```

### 10b. Customer database CSV (`customer_database.csv`)
```csv
CustomerID,Name,Email,Phone,CreditCard,Address,GPS
CUST-001,Michael Thompson,m.thompson@outlook.com,+1-303-555-0188,5425233430109903,"1234 Birch Lane, Denver, CO","39.7392,-104.9903"
```

### 10c. Network inventory TSV (`network_inventory.tsv`)
```tsv
Hostname	IPv4	IPv6	MAC	Location	Admin_Email
web-01	203.0.113.50	2001:db8:85a3::8a2e:370:7334	00:1A:2B:3C:4D:5E	Austin DC	admin@company.com
```

### 10d. Medical records CSV (`patient_records.csv`)
```csv
PatientID,Name,SSN,DOB,Diagnosis,Medication,Phone,Email,Provider
P-001,Angela Martinez,412-55-8823,06/14/1982,hypertension;diabetes,metformin;lisinopril,(305) 555-0188,a.martinez@email.com,Dr. Rebecca Chen
```

### 10e. Financial transaction log (`transactions.csv`)
```csv
TxnID,Timestamp,CardNumber,Amount,MerchantIP,CustomerEmail,CustomerPhone
TXN-001,2026-02-18T14:32:01Z,4532015112830366,$149.99,203.0.113.42,customer@example.com,(415) 555-0132
```

**Sources**:
- [Mendeley Synthetic PII Financial Documents](https://data.mendeley.com/datasets/tzrjx692jy/1) — CC BY 4.0
- [DLP Test sample data](https://dlptest.com/sample-data/) — downloadable CSV/PDF/XLSX
- [ptrav/dlptest GitHub](https://github.com/ptrav/dlptest) — pre-seeded JSON with PII combinations
- Generate with Python `Faker` + `csv` module using values from our existing test data
- Generate with this Faker script:
  ```python
  from faker import Faker
  import csv
  fake = Faker('en_US')
  with open('employee_roster.csv', 'w', newline='') as f:
      writer = csv.writer(f)
      writer.writerow(['Name','SSN','Email','Phone','CreditCard','IPv4'])
      for _ in range(50):
          writer.writerow([fake.name(), fake.ssn(), fake.email(),
                          fake.phone_number(), fake.credit_card_number(),
                          fake.ipv4_public()])
  ```

**Validation**: JSON scanner handles CSV/TSV when embedded as string values in agent tool results. Standalone CSV tests validate that line-by-line text scanning catches all PII types across columns.

---

## Summary: Complete Folder Structure

```
test/
├── data/
│   ├── input/
│   ├── PII_Detection/              (15 files — DONE)
│   │   ├── Credit_Card_Numbers.txt
│   │   ├── Social_Security_Numbers.txt
│   │   ├── Phone_Numbers.txt
│   │   ├── Email_Addresses.txt
│   │   ├── API_Keys_Tokens.txt
│   │   ├── IPv4_Addresses.txt
│   │   ├── IPv6_Addresses.txt
│   │   ├── GPS_Coordinates.txt
│   │   ├── MAC_Addresses.txt
│   │   ├── Health_Keywords.txt
│   │   ├── Child_Keywords.txt
│   │   ├── Person_Names.txt
│   │   ├── Locations.txt
│   │   ├── Organizations.txt
│   │   └── Mixed_Structured_PII.txt
│   ├── Visual_PII/                 (TODO)
│   │   ├── Faces/                  (20-30 images)
│   │   ├── Screenshots/            (15-20 images)
│   │   ├── Documents/              (10-15 images)
│   │   ├── EXIF/                   (10 images)
│   │   └── NSFW/                   (5-10 images)
│   ├── Audio_PII/                  (TODO: 15-20 audio files)
│   ├── Agent_Tool_Results/         (TODO: 10-15 JSON files)
│   ├── Multilingual_PII/           (TODO: 8 language files)
│   ├── Code_Config_PII/            (TODO: 8-10 code/config files)
│   └── Structured_Data_PII/        (TODO: 5-8 CSV/TSV files)
│   └── output/
    ├── PII_Detection/              (empty — for test results)
    ├── Visual_PII/                 (empty — for test results)
    ├── Audio_PII/                  (empty — for test results)
    ├── Agent_Tool_Results/         (empty — for test results)
    ├── Multilingual_PII/           (empty — for test results)
    ├── Code_Config_PII/            (empty — for test results)
    └── Structured_Data_PII/        (empty — for test results)
```

## Estimated Total Test Data

| Category             | Files  | Est. Size   | Priority |
|----------------------|--------|-------------|----------|
| PII_Detection (text) | 15     | ~100 KB     | DONE     |
| Visual_PII           | 60-75  | ~50-200 MB  | HIGH     |
| Audio_PII            | 15-20  | ~20-50 MB   | HIGH     |
| Agent_Tool_Results   | 10-15  | ~5-50 MB    | HIGH     |
| Multilingual_PII     | 8      | ~40 KB      | MEDIUM   |
| Code_Config_PII      | 8-10   | ~30 KB      | MEDIUM   |
| Structured_Data_PII  | 5-8    | ~50 KB      | MEDIUM   |
