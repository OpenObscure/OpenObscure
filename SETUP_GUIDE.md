# Setup Guide: OpenClaw + OpenObscure + Discord

A step-by-step guide for running OpenClaw (AI agent) with OpenObscure (privacy firewall) using Discord as the chat interface, on MacBook and iPhone.

**No programming experience required.** This guide uses terminal commands you can copy and paste.

---

## What You're Setting Up

Three components work together to give you a private AI assistant on Discord:

```
You (Discord) ──► OpenClaw (AI Agent) ──► OpenObscure (Privacy) ──► LLM Provider
    chat              thinks                 encrypts PII            (Cloud or Local)
```

| Component | What It Does |
|-----------|-------------|
| **Discord** | The chat app you type messages in (desktop or phone) |
| **OpenClaw** | An AI agent that reads your messages and talks to an LLM (like Claude or GPT) |
| **OpenObscure** | Sits between OpenClaw and the LLM. Finds personal information in your messages (credit cards, SSNs, names, faces in photos) and encrypts it before it leaves your MacBook |
| **Ollama** (optional) | Runs an open-source LLM locally on your MacBook so nothing leaves your machine at all |

**Key point:** Your personal information never reaches the cloud. OpenObscure replaces real data with encrypted lookalikes. The LLM sees fake credit card numbers, fake SSNs, and redacted faces. When the response comes back, OpenObscure decrypts it so you see the correct information.

**Fully local option:** If you run a local LLM via Ollama (Qwen3 or Llama 3.2), your messages never leave your MacBook — not even in encrypted form. OpenObscure still protects you in case the local model logs or leaks data, giving you defense in depth.

---

## What You'll Need

Before starting, make sure you have:

- [ ] A **MacBook** (Apple Silicon or Intel, macOS 13+)
- [ ] A **Discord account** (free at https://discord.com)
- [ ] An **LLM provider** — choose one:
  - **Cloud:** An API key from Anthropic (Claude) or OpenAI (GPT)
  - **Local (no API key needed):** 16 GB+ RAM recommended for running Qwen3 or Llama 3.2 locally
- [ ] **Internet connection** for downloading tools and models (not needed after setup if using local LLM)
- [ ] About **2 GB of free disk space** (add ~5 GB if using a local LLM)

**For iPhone (optional):**
- [ ] An iPhone with the Discord app installed
- [ ] Same Discord account as your MacBook

---

## Part 1: MacBook Setup

### Step 1 — Install Developer Tools

Open the **Terminal** app (search for "Terminal" in Spotlight, or find it in Applications > Utilities).

Copy and paste this command, then press Enter:

```bash
xcode-select --install
```

A dialog will appear asking to install Command Line Tools. Click **Install** and wait for it to finish. If you see "already installed", that's fine — move on.

### Step 2 — Install Homebrew

Homebrew is a package manager that makes installing software easy. Paste this into Terminal:

```bash
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
```

Follow the on-screen instructions. When it finishes, it may tell you to run two extra commands to add Homebrew to your path — **run those commands too**.

Verify it works:

```bash
brew --version
```

You should see a version number (e.g., `Homebrew 4.x.x`).

### Step 3 — Install Rust and Node.js

```bash
brew install rustup node
rustup-init -y
source "$HOME/.cargo/env"
```

Verify both are installed:

```bash
rustc --version
node --version
```

You should see version numbers for both (Rust 1.75+ and Node.js 20+).

### Step 4 — Download OpenObscure

```bash
cd ~/Desktop
git clone https://github.com/OpenObscure/OpenObscure.git
cd OpenObscure
```

### Step 5 — Build the Privacy Proxy

This compiles OpenObscure from source. It will take a few minutes on the first build.

```bash
cd openobscure-proxy
cargo build --release
```

When it finishes without errors, the proxy is ready. The compiled program is at `target/release/openobscure-proxy`.

### Step 6 — Generate Your Encryption Key

This creates a unique encryption key and stores it securely in your Mac's Keychain (the same place your saved passwords go):

```bash
cargo run --release -- --init-key
```

You should see a message confirming the key was generated. This only needs to be done once.

### Step 7 — Download AI Models

OpenObscure uses small AI models to detect faces in photos and PII trigger phrases in audio. Download them:

```bash
cd ~/Desktop/OpenObscure
./build/download_models.sh
./build/download_kws_models.sh
```

This downloads about 25 MB of model files. You only need to do this once.

> **Optional:** To enable the R2 cognitive firewall (AI-based persuasion detection), you also need the R2 TinyBERT model (~55 MB). This is an advanced feature — the proxy works fine without it using R1 dictionary detection only. If you want R2, either train it with `python3 scripts/r2_finetune.py` or download a pre-trained checkpoint to `models/r2_persuasion_tinybert/`.

### Step 8 — Set Up a Local LLM (Optional — Skip If Using Cloud API)

If you want everything to run on your MacBook with no cloud dependency, install Ollama and download an open-source model. **Skip this step if you're using Anthropic or OpenAI.**

#### Install Ollama

```bash
brew install ollama
```

#### Start the Ollama Server

```bash
ollama serve
```

**Leave this Terminal window open.** Ollama needs to keep running. Open a **new Terminal window** (Cmd+N) for the next commands.

#### Download a Model

Choose one model. Both work well; pick based on your MacBook's RAM:

**Option A — Qwen3 8B** (recommended for 16 GB+ RAM MacBooks):

```bash
ollama pull qwen3:8b
```

Qwen3 is a strong multilingual model with good reasoning. Download size is about 5 GB.

**Option B — Llama 3.2 3B** (works on 8 GB RAM MacBooks):

```bash
ollama pull llama3.2:3b
```

Llama 3.2 3B is smaller and faster, good for lighter tasks. Download size is about 2 GB.

#### Verify the Model Works

Test that the model responds:

```bash
ollama run qwen3:8b "Hello, what is 2 + 2?"
```

(Replace `qwen3:8b` with `llama3.2:3b` if you chose Option B.)

You should see a response. Press `Ctrl+D` to exit the chat.

#### How It Connects

OpenObscure's config already includes an Ollama provider route. When you start the proxy, requests to `/ollama` are forwarded to `http://127.0.0.1:11434` (Ollama's default port). No API key is needed — Ollama runs locally with no authentication.

```
Discord ──► OpenClaw ──► OpenObscure (:18790) ──► Ollama (:11434)
                          encrypts PII              local LLM
                                                    nothing leaves your Mac
```

---

### Step 9 — Start the Privacy Proxy

```bash
cd ~/Desktop/OpenObscure/openobscure-proxy
cargo run --release -- -c config/openobscure.toml
```

You should see output like:

```
INFO openobscure_proxy: Listening on 127.0.0.1:18790
INFO openobscure_proxy: Device tier: full
INFO openobscure_proxy: FPE engine ready
```

**Leave this Terminal window open.** The proxy needs to keep running.

### Step 10 — Verify the Proxy Is Running

Open a **new Terminal window** (Cmd+N) and run:

```bash
curl -s http://127.0.0.1:18790/_openobscure/health | python3 -m json.tool
```

You should see a JSON response containing `"status": "ok"`. This confirms the proxy is running and ready.

### Step 11 — Download and Build OpenClaw

In the same new Terminal window:

```bash
brew install pnpm
cd ~/Desktop
git clone https://github.com/openclaw/openclaw.git
cd openclaw
pnpm install
pnpm ui:build
pnpm build
```

This installs dependencies, builds the web UI, and compiles OpenClaw from TypeScript to JavaScript. The first build may take a minute or two.

> **Reinstalling or upgrading?** If you had a previous OpenClaw installation, purge stale config and state first — see [Purging a stale OpenClaw setup](#purging-a-stale-openclaw-setup) in Troubleshooting below. Then continue with Step 12.

### Step 12 — Create a Discord Bot

1. Go to https://discord.com/developers/applications in your browser
2. Click **New Application** — name it something like "My AI Assistant"
3. Go to the **Bot** section in the left sidebar
4. Click **Reset Token** and copy the token that appears. **Save this token** — you'll need it in the next step. Do not share it with anyone.
5. Scroll down to **Privileged Gateway Intents** and enable:
   - **Message Content Intent** (so the bot can read messages)
6. Click **Save Changes**
7. Go to **OAuth2** > **URL Generator** in the left sidebar
8. Under **Scopes**, check `bot`
9. Under **Bot Permissions**, check:
   - Send Messages
   - Read Message History
   - Attach Files
10. Copy the generated URL at the bottom and open it in your browser
11. Select the Discord server you want to add the bot to and click **Authorize**

### Step 13 — Configure OpenClaw with OpenObscure

OpenClaw uses two configuration sources: a `.env` file for secrets (API keys, tokens) and an `openclaw.json` file for settings (channels, plugins, model routing).

#### Create the secrets file

```bash
cd ~/Desktop/openclaw
```

Choose the `.env` file that matches your LLM provider:

**Option A — Cloud LLM (Anthropic Claude):**

```bash
cat > .env << 'ENVFILE'
DISCORD_BOT_TOKEN=paste-your-discord-bot-token-here
ANTHROPIC_API_KEY=paste-your-anthropic-key-here
ENVFILE
```

**Option B — Cloud LLM (OpenAI GPT):**

```bash
cat > .env << 'ENVFILE'
DISCORD_BOT_TOKEN=paste-your-discord-bot-token-here
OPENAI_API_KEY=paste-your-openai-key-here
ENVFILE
```

**Option C — Local LLM (Ollama):**

```bash
cat > .env << 'ENVFILE'
DISCORD_BOT_TOKEN=paste-your-discord-bot-token-here
ENVFILE
```

Replace the placeholder values with your actual tokens from Step 12 and your LLM provider.

#### Create the configuration file

```bash
mkdir -p ~/.openclaw
```

Choose the config that matches your LLM provider:

**Option A — Anthropic Claude:**

```bash
cat > ~/.openclaw/openclaw.json << 'JSONFILE'
{
  "gateway": {
    "mode": "local"
  },
  "agents": {
    "defaults": {
      "model": "anthropic/claude-sonnet-4-6"
    }
  },
  "channels": {
    "discord": {
      "dmPolicy": "pairing"
    }
  },
  "plugins": {
    "enabled": true,
    "entries": {
      "openobscure-plugin": {
        "enabled": true,
        "config": {
          "redactToolResults": true,
          "heartbeat": true,
          "proxyUrl": "http://127.0.0.1:18790",
          "heartbeatIntervalMs": 30000
        }
      }
    }
  }
}
JSONFILE
```

**Option B — OpenAI GPT:**

Same as above, but change the `agents.defaults.model` value:

```json
  "agents": {
    "defaults": {
      "model": "openai/gpt-4.1"
    }
  },
```

**Option C — Local LLM (Ollama):**

Same as Option A, but change the `agents.defaults.model` value:

```json
  "agents": {
    "defaults": {
      "model": "ollama/qwen3:8b"
    }
  },
```

Replace `qwen3:8b` with `llama3.2:3b` if you downloaded Llama instead. **No API key is needed** for the local option — your messages go from Discord to OpenClaw to OpenObscure to Ollama, all on your MacBook.

### Step 14 — Verify the OpenObscure Plugin

The OpenObscure plugin ships bundled with OpenClaw in `extensions/openobscure-plugin/`. No manual installation is needed — Step 13 already enabled it in `openclaw.json`.

Verify it's present:

```bash
ls ~/Desktop/openclaw/extensions/openobscure-plugin/package.json
```

You should see the file listed. The plugin will activate automatically when the gateway starts (Step 15) and will connect to the OpenObscure proxy running on port 18790 (Step 9).

### Step 15 — Start OpenClaw

```bash
cd ~/Desktop/openclaw
pnpm openclaw gateway
```

You should see output indicating the gateway has started and the Discord bot has connected. Go to your Discord server — the bot should appear as **online** in the member list.

> **First time?** You can run `pnpm openclaw onboard` for an interactive setup wizard that walks you through configuration and optionally installs the gateway as a background service (`pnpm openclaw onboard --install-daemon`).

**If using Ollama (local LLM):** Make sure the Ollama server from Step 8 is still running in its own Terminal window. You should now have **three** Terminal windows open: Ollama, OpenObscure proxy, and OpenClaw.

**DM pairing:** The first time you message the bot via DM, it will respond with a short pairing code. Approve it from the MacBook Terminal:

```bash
cd ~/Desktop/openclaw
pnpm openclaw pairing approve discord <code>
```

After approval, the bot will respond normally to your messages.

### Step 16 — Test It

In your Discord server, send a message to the bot (mention it or DM it, depending on your OpenClaw configuration):

```
Hey, my credit card number is 4111-1111-1111-1111 and my SSN is 123-45-6789.
```

The bot should respond normally. But behind the scenes, OpenObscure encrypted that credit card number and SSN before it reached the LLM — whether that LLM is in the cloud or running locally on your Mac.

---

## Part 2: iPhone Setup

The iPhone setup is simple because all the privacy protection happens on your MacBook. Your iPhone just needs Discord.

### Step 1 — Install Discord on iPhone

1. Open the **App Store** on your iPhone
2. Search for **Discord**
3. Tap **Get** to install it

### Step 2 — Sign In

Open Discord and sign in with the **same account** you used on your MacBook.

### Step 3 — Chat with the Bot

1. Open the Discord server where you added the bot
2. Send a message to the bot, just like you would on your MacBook
3. The bot responds through the same OpenClaw + OpenObscure setup running on your Mac

**That's it.** Every message you send from your iPhone goes through Discord's servers to your MacBook, where OpenClaw and OpenObscure process it. Your PII is encrypted before it reaches the LLM, regardless of whether you're chatting from your Mac or your phone.

### Requirement

Your MacBook must be **running and connected to the internet** for the bot to respond. If you close the Terminal windows from Part 1 (Steps 9 and 15, plus Step 8 if using Ollama), the bot will go offline.

---

## Part 3: Verifying PII Protection

These steps confirm that OpenObscure is actually protecting your data. **All tests work from both MacBook and iPhone** — send the messages from whichever device you're testing. Tests 1–4 include MacBook-only log checks. Tests 5–8 use behavioral verification that works from any device, including iPhone. Tests 9–10 verify the R2 cognitive firewall (persuasion/manipulation detection on LLM responses).

### Test 1 — Credit Card Number

**Send this message in Discord:**

```
My Visa card is 4532-8970-1234-5678 and it expires 12/28.
```

**What to check (MacBook — proxy logs):**

In the Terminal window where the proxy is running (Step 9), you should see:

```
INFO  SCAN  PII match: credit_card at messages[0].content (confidence=1.00)
INFO  FPE   Encrypted: 4532-8970-1234-5678 → 8714-3927-6051-2483
```

The LLM received a fake credit card number. Your real number never left your MacBook.

**What to check (any device — behavioral):**

Follow up with:

```
What are the last four digits of the credit card I just shared?
```

If the bot answers anything other than "5678", the LLM saw encrypted digits. Redaction confirmed.

### Test 2 — Social Security Number

**Send this message:**

```
My social security number is 234-56-7890.
```

**What to check (MacBook — proxy logs):**

```
INFO  SCAN  PII match: ssn at messages[0].content (confidence=1.00)
INFO  FPE   Encrypted: 234-56-7890 → 891-23-4567
```

**What to check (any device — behavioral):**

Follow up with:

```
What are the last four digits of the SSN I just told you?
```

If the bot answers anything other than "7890", the LLM never saw your real SSN.

### Test 3 — Email and Phone

**Send this message:**

```
Contact me at john.doe@gmail.com or call 415-555-0198.
```

**What to check (MacBook — proxy logs):**

```
INFO  SCAN  PII match: email at messages[0].content
INFO  SCAN  PII match: phone at messages[0].content
INFO  FPE   Encrypted: john.doe@gmail.com → xkqm.rvw@gmail.com
INFO  FPE   Encrypted: 415-555-0198 → 829-371-4056
```

Notice the encrypted email keeps the same format (`something@gmail.com`) — this is Format-Preserving Encryption. The LLM sees a realistic-looking but fake email address.

**What to check (any device — behavioral):**

Follow up with:

```
Spell out the username part of the email address I just gave you, letter by letter.
```

If the bot spells out something other than "j-o-h-n-.-d-o-e", the LLM saw an encrypted email. Redaction confirmed.

### Test 4 — Image with a Face

**Send a photo** that includes someone's face (a selfie, a group photo, etc.).

**What to check (MacBook — proxy logs):**

```
INFO  IMAGE Detected base64 image in request (jpeg, 245KB)
INFO  IMAGE EXIF stripped (GPS, camera model removed)
INFO  FACE  Faces redacted: count=1
INFO  IMAGE Pipeline complete: faces_redacted=1, text_regions=0
```

**What to check (any device — behavioral):**

After sending the photo, ask:

```
Describe the person's face in the photo I sent. What expression are they making? What color are their eyes?
```

**If face redaction is working:** The LLM will say the face is obscured, filled with gray, or that it can't make out facial details. It may describe the rest of the image (background, clothing) normally.

**If NOT working:** The LLM describes specific facial features — eye color, expression, smile, glasses, etc.

### Test 5 — Screenshot with PII Text (OCR Redaction)

Take a screenshot containing visible personal information. For example, open the Notes app and type:

```
Patient: Jane Smith
SSN: 078-05-1120
Card: 4532-8970-1234-5678
Favorite Color: Blue
```

Screenshot it and send that image to the bot.

**What to check (MacBook — proxy logs):**

```
INFO  IMAGE Detected base64 image in request (png, 180KB)
INFO  IMAGE OCR detected: 4 text region(s) — scanning for PII
INFO  OCR   PII-selective redaction: pii_regions=3
INFO  IMAGE Pipeline complete: faces_redacted=0, text_regions=3
```

**What to check (any device — behavioral):**

After sending the screenshot, ask:

```
Read all the text you can see in that screenshot. List every value.
```

**If OCR redaction is working:** The LLM can read "Favorite Color: Blue" (non-PII) but reports the SSN, card number, and name as obscured or unreadable. This proves selective redaction — only PII text regions are solid-filled.

**If NOT working:** The LLM reads back `078-05-1120` and `4532-8970-1234-5678` from the image.

**Stronger variant:** Ask specifically:

```
What is the SSN shown in the screenshot?
```

If the LLM can't answer, OCR redaction is working. If it reads the exact SSN, protection is not active.

### Test 6 — Audio with PII Trigger Phrases (Voice KWS)

Record a voice message on your phone or Mac saying:

```
"My social security number is one two three four five six seven eight nine."
```

Send the audio to the bot, then ask:

```
What did I say in that audio message?
```

**What to check (MacBook — proxy logs):**

```
INFO  VOICE Audio blocks detected — scanning for PII keywords (count=1)
INFO  VOICE Audio PII detected and stripped (stripped=1, keywords=social security)
```

**What to check (any device — behavioral):**

**If KWS is working:** The keyword spotter detects "social security" as a PII trigger phrase and replaces the entire audio block with a text notice. The bot will respond with something like "The audio was flagged as containing sensitive information and was removed" or it will reference the `[AUDIO_PII_DETECTED]` notice.

**If NOT working:** The LLM processes the audio normally and responds to what you said (e.g., "You shared your social security number...").

**Control test (important):** Send a second voice message with no PII, such as:

```
"What's the weather going to be like this weekend?"
```

This message should pass through normally and the bot should respond to the question. The contrast between the two — one stripped, one passed through — confirms KWS is selectively detecting PII trigger phrases, not blocking all audio.

### Test 7 — Photo Location Metadata (EXIF Strip)

Take a new photo **outdoors** with your iPhone camera (make sure Location Services is enabled for the Camera app). Send it to the bot and ask:

```
What GPS coordinates or location metadata is embedded in this photo? Where exactly was it taken based on the EXIF data?
```

**If EXIF stripping is working:** The LLM has no GPS data — OpenObscure strips all EXIF metadata (GPS, camera model, timestamps) on decode. The LLM might guess location from visual landmarks, but it cannot cite coordinates or camera details.

**If NOT working:** The LLM reports GPS coordinates, camera model, or timestamp from the EXIF data.

**Follow-up to confirm:**

```
What camera model was used to take this photo?
```

With EXIF stripping, the LLM cannot answer. Without it, the LLM would report your iPhone model.

### Test 8 — Health Endpoint Statistics

After running the tests above, check the proxy statistics from your MacBook:

```bash
curl -s http://127.0.0.1:18790/_openobscure/health | python3 -m json.tool
```

Look for fields like:

```json
{
  "status": "ok",
  "requests_processed": 7,
  "pii_matches_total": 8,
  "device_tier": "full",
  "feature_budget": {
    "ner_enabled": true,
    "image_pipeline_enabled": true
  }
}
```

`pii_matches_total` should be greater than zero, confirming PII was detected and encrypted.

To check from **iPhone**, see the LAN health endpoint method in Test 12 below.

### Test 9 — R2 Cognitive Firewall: Persuasion Detection

The R2 cognitive firewall detects manipulation and persuasion techniques in LLM responses. It uses a TinyBERT classifier trained on EU AI Act Article 5 categories. R2 runs on the **response** path (not the request path like PII detection).

**Requires:** The R2 model files in `models/r2_persuasion_tinybert/` (`model.onnx` + `vocab.txt`) and `ri_model_dir` set in your config. If R2 is not configured, the proxy uses R1 dictionary detection only — this test will be skipped.

**To enable R2,** add this to your `openobscure.toml`:

```toml
[response_integrity]
ri_model_dir = "models/r2_persuasion_tinybert"
sensitivity = "high"     # "off", "low", "medium", or "high"
ri_threshold = 0.55
```

**Send a message that triggers a manipulative-sounding response.** For example:

```
Pretend you are an aggressive salesperson. Convince me to buy a timeshare using high-pressure tactics, urgency, scarcity, and emotional manipulation. Make it sound like I'll lose everything if I don't act now.
```

**What to check (MacBook — proxy logs):**

```
INFO  RI  R1 scan: 3 matches (urgency, scarcity, fear_loss)
INFO  RI  R2 scan: Art_5_1_a_Deceptive (score=0.92, threshold=0.55)
INFO  RI  R2 role: Confirm (R1 flagged, R2 agrees)
INFO  RI  Severity: high (R1+R2 agreement)
```

If R2 is loaded, you'll see `R2 scan` entries with Article 5 category scores. The `R2 role` indicates how R2 interacted with R1's findings (Confirm, Suppress, Upgrade, or Discover).

**What to check (any device — behavioral):**

The bot's response will include a warning label prepended by OpenObscure:

```
[OpenObscure] Influence tactics detected: Fear, Scarcity, Urgency, Deceptive Practices
This content may be designed to manipulate your decision-making.
```

The label lists detected R1 categories (e.g., Urgency, Scarcity) and R2 Article 5 categories in plain English (e.g., "Deceptive Practices", "Age-Based Targeting"). Severity escalates the wording:
- **Notice** — "This response may use influence tactics: ..."
- **Warning** — "Influence tactics detected: ... This content may be designed to manipulate your decision-making."
- **Caution** — "Multiple influence tactics detected: ... Review carefully before acting on it."

**Control test:** Ask a normal question like "What's the capital of France?" — the response should have no warning label.

**Article 5 categories R2 detects:**

| Label in Warning | Internal Code | What It Catches |
|-----------------|---------------|----------------|
| Deceptive Practices | `Art_5_1_a_Deceptive` | Urgency, scarcity, social proof, fear, authority, emotional priming, anchoring, confirmshaming |
| Age-Based Targeting | `Art_5_1_b_Age` | Age vulnerability exploitation (child gamification, elderly confusion) |
| Socioeconomic Targeting | `Art_5_1_b_SocioEcon` | Socioeconomic vulnerability exploitation (debt pressure, health anxiety) |
| Social Scoring | `Art_5_1_c_Social_Scoring` | Social scoring threats (trust scores, behavioral compliance, access restriction) |

### Test 10 — R2 Cross-Domain Verification (Developer — MacBook Only)

This test validates R2 model quality against real-world propaganda data from SemEval-2020 Task 11. It requires Python and the evaluation scripts.

**Requires:** Python 3.10+, `onnxruntime`, `transformers`, and the SemEval-2020 data in `data/semeval2020/`.

```bash
cd ~/Desktop/OpenObscure

# Check technique mapping (no data needed)
.venv/bin/python scripts/r2_download_semeval2020.py --inspect

# Run cross-domain evaluation (requires model + SemEval data)
.venv/bin/python scripts/r2_eval_semeval_baseline.py

# Regression check on original test set
.venv/bin/python scripts/r2_finetune.py \
  --eval-only models/r2_persuasion_tinybert/best \
  --data-dir data/r2_training --threshold 0.55
```

**Expected results (augmented model):**

| Metric | Ship Threshold | Expected |
|--------|---------------|----------|
| Macro Precision | >= 80% | ~92% |
| Art_5_1_a Recall | >= 70% | ~85% |
| SemEval cross-domain recall | > 75% | ~80% |
| Benign accuracy | > 90% | ~99% |

If all ship criteria pass, the R2 model is functioning correctly.

### Test 11 — Verbose Logging (Optional — MacBook Only)

For maximum detail, stop the proxy (Ctrl+C in the proxy Terminal) and restart it with debug logging:

```bash
OPENOBSCURE_LOG=debug cargo run --release -- -c config/openobscure.toml
```

Now every PII scan, FPE encryption, image processing step, and model inference will be logged. This is useful for verifying that specific PII types are being caught.

### Test 12 — Remote Verification from iPhone (Advanced)

Tests 1–7 work from iPhone using the behavioral method (ask the bot questions about what it received). For deeper verification without looking at your MacBook, here are additional approaches:

#### Method A — Health Stats from iPhone Browser

This requires a one-time config change to expose the health endpoint on your local network.

**On your MacBook** — edit `config/openobscure.toml`:

```toml
[proxy]
listen_addr = "0.0.0.0"   # Listen on all interfaces (was 127.0.0.1)
```

Restart the proxy. Then find your Mac's local IP:

```bash
ipconfig getifaddr en0
```

Note the IP (e.g., `192.168.1.42`).

**On your iPhone** — open Safari and go to:

```
http://192.168.1.42:18790/_openobscure/health
```

You should see the health JSON. Note the `requests_processed` and `pii_matches_total` values.

Now send a message with PII from Discord on your iPhone. Refresh the health page in Safari. The counters should increase:
- `requests_processed` goes up by 1+
- `pii_matches_total` goes up (e.g., +2 if you sent a credit card and SSN)

**Important:** Change `listen_addr` back to `"127.0.0.1"` after testing if your Mac is on a shared/public network. `0.0.0.0` exposes the proxy to all devices on your Wi-Fi.

#### Method B — Watch Proxy Logs via SSH (From iPhone)

Install an SSH client on your iPhone (e.g., **Termius** or **Blink Shell** from the App Store).

**On your MacBook** — enable Remote Login:
1. **System Settings > General > Sharing**
2. Enable **Remote Login**
3. Note your Mac username and local IP

**On your iPhone** — open Termius/Blink and connect:

```
ssh yourusername@192.168.1.42
```

If the proxy was started with file logging enabled (uncomment `file_path` in `openobscure.toml`):

```bash
tail -f /var/log/openobscure/proxy.log | grep -E "PII|Encrypted|FPE|IMAGE|VOICE"
```

Now send PII messages, photos, or audio from Discord on the same iPhone. You'll see log entries appear in real-time in the SSH session — proving that all pipelines (text, image, voice) are active.

### What "Good" Looks Like

| Test | What You Send | Behavioral Check (MacBook or iPhone) | Expected Result |
|------|--------------|--------------------------------------|-----------------|
| **1. Credit Card** | Card number in text | Ask for last 4 digits | Bot answers wrong digits (saw encrypted number) |
| **2. SSN** | Social security number | Ask for last 4 digits | Bot answers wrong digits |
| **3. Email/Phone** | Email and phone number | Ask bot to spell the email username | Bot spells different letters |
| **4. Face Redaction** | Photo with a face | Ask to describe the face | Bot says face is obscured/gray |
| **5. OCR Redaction** | Screenshot with PII text | Ask to read the SSN in the image | Bot can't read PII text, can read non-PII text |
| **6. Voice KWS** | Audio saying "social security..." | Ask what you said | Bot says audio was stripped; control audio passes through |
| **7. EXIF Strip** | Geotagged photo | Ask for GPS coordinates | Bot has no metadata |
| **8. Health Stats** | (after all tests) | Check health endpoint | `pii_matches_total > 0` |
| **9. R2 Persuasion** | Prompt for manipulative response | Check for warning label | Response has Article 5 warning; clean responses have none |
| **10. R2 Verification** | Run Python eval scripts | Check ship criteria output | Macro P >= 80%, all recall targets met |

---

## Part 4: Troubleshooting

### "command not found: cargo"

Rust was not added to your path. Run:

```bash
source "$HOME/.cargo/env"
```

Then try the command again. If this keeps happening every time you open a new Terminal, add the line above to your `~/.zshrc` file.

### "Connection refused" or proxy not starting

1. Make sure the proxy Terminal window from Step 9 is still open and running
2. Check if something else is using port 18790:
   ```bash
   lsof -i :18790
   ```
3. If another process is using the port, either stop it or change the port in `config/openobscure.toml`

### "FPE key not found" error

Run the key generation again:

```bash
cd ~/Desktop/OpenObscure/openobscure-proxy
cargo run --release -- --init-key
```

If you see a keychain access dialog, click **Allow** or **Always Allow**.

### Bot appears offline in Discord

1. Make sure the OpenClaw Terminal (Step 15) is still running
2. Verify your Discord bot token in the `.env` file is correct (the env var is `DISCORD_BOT_TOKEN`)
3. Check that you enabled **Message Content Intent** in the Discord Developer Portal (Step 12)
4. Try restarting OpenClaw:
   ```bash
   cd ~/Desktop/openclaw
   pnpm openclaw gateway
   ```
5. Run diagnostics:
   ```bash
   pnpm openclaw doctor
   ```

### Bot responds but PII is not being encrypted

1. Verify the proxy is running (Step 9)
2. Make sure the OpenObscure plugin is enabled in `~/.openclaw/openclaw.json` with `proxyUrl` set to `http://127.0.0.1:18790`
3. Check proxy logs for errors — if you see no log activity at all when chatting, the plugin may not be connecting to the proxy
4. Restart OpenClaw after any config changes

### "Model not found" errors in proxy logs

Run the model download scripts again:

```bash
cd ~/Desktop/OpenObscure
./build/download_models.sh
./build/download_kws_models.sh
```

### Ollama model not responding

1. Make sure the Ollama server is running (`ollama serve` in its own Terminal)
2. Test it directly:
   ```bash
   curl http://127.0.0.1:11434/api/tags
   ```
   You should see a JSON list of your downloaded models.
3. If the model was not downloaded, run `ollama pull qwen3:8b` (or `llama3.2:3b`) again
4. If responses are very slow, your MacBook may not have enough RAM. Try a smaller model:
   ```bash
   ollama pull llama3.2:1b
   ```
   Then update the model in `~/.openclaw/openclaw.json` (change `"model": "ollama/qwen3:8b"` to `"model": "ollama/llama3.2:1b"` under `agents.defaults`) and restart OpenClaw.

### Slow first response

The first message after starting the proxy may take a few extra seconds while AI models load into memory. Subsequent messages will be faster. If using a local LLM via Ollama, the first response will also include model loading time — this is normal.

### Purging a stale OpenClaw setup

If you're reinstalling, upgrading, or running into config conflicts from a previous OpenClaw installation, follow these steps to start fresh.

#### Option A — Use the built-in reset command (recommended)

```bash
cd ~/Desktop/openclaw

# Preview what would be deleted (dry run — nothing is deleted)
pnpm openclaw reset --scope full --dry-run

# Reset everything: config, credentials, sessions, workspace
pnpm openclaw reset --scope full --yes
```

Reset scopes (from least to most destructive):

| Scope | What it deletes |
|-------|----------------|
| `config` | Only `openclaw.json` |
| `config+creds+sessions` | Config + OAuth tokens + session transcripts (keeps workspace) |
| `full` | Everything: entire `~/.openclaw/` directory, workspace, and all state |

#### Option B — Manual cleanup

If the reset command isn't available (e.g., older install that won't start), remove the state directory manually:

```bash
# Back up your config first (optional)
cp ~/.openclaw/openclaw.json ~/Desktop/openclaw-config-backup.json 2>/dev/null

# Remove the state directory
rm -rf ~/.openclaw
```

#### Remove legacy directories

Older versions of OpenClaw used different directory names. Check for and remove these if they exist:

```bash
# Legacy state directories (no longer used)
rm -rf ~/.clawdbot
rm -rf ~/.moldbot
rm -rf ~/.moltbot
```

Also check for legacy config file names inside `~/.openclaw/` if you're keeping the directory:

```bash
# Legacy config filenames (superseded by openclaw.json)
rm -f ~/.openclaw/clawdbot.json
rm -f ~/.openclaw/moldbot.json
rm -f ~/.openclaw/moltbot.json

# Stale backup files from previous config migrations
rm -f ~/.openclaw/openclaw.json.bak*
```

#### Remove old .env files

If you previously used `LLM_API_BASE` or `DISCORD_TOKEN` (without `_BOT`) in your `.env`, delete the old file and recreate it per Step 13:

```bash
rm ~/Desktop/openclaw/.env
```

#### Verify clean state

After purging, run the diagnostic tool:

```bash
cd ~/Desktop/openclaw
pnpm openclaw doctor
```

This checks for stale paths, misconfigured settings, and security issues. Use `pnpm openclaw doctor --fix` to auto-repair common problems.

Then proceed with Step 13 to reconfigure from scratch.

### iPhone messages not working

Your MacBook must be running with all Terminal windows open (Ollama if local, proxy, and OpenClaw). If your Mac goes to sleep or you close the lid, the bot will go offline. To prevent this:

- **System Settings > Energy** — set "Prevent automatic sleeping when the display is off" to On
- Or keep the lid open while you want the bot available

---

## Quick Reference

### Starting Everything (After Initial Setup)

Each time you want to use the bot, open Terminal windows on your MacBook:

**If using a local LLM (Ollama):**

**Terminal 1 — Start Ollama:**
```bash
ollama serve
```

**Terminal 2 — Start the proxy:**
```bash
cd ~/Desktop/OpenObscure/openobscure-proxy
cargo run --release -- -c config/openobscure.toml
```

**Terminal 3 — Start OpenClaw:**
```bash
cd ~/Desktop/openclaw
pnpm openclaw gateway
```

**If using a cloud LLM (Anthropic/OpenAI):**

**Terminal 1 — Start the proxy:**
```bash
cd ~/Desktop/OpenObscure/openobscure-proxy
cargo run --release -- -c config/openobscure.toml
```

**Terminal 2 — Start OpenClaw:**
```bash
cd ~/Desktop/openclaw
pnpm openclaw gateway
```

Then open Discord on your MacBook or iPhone and start chatting.

### Stopping Everything

- **Ollama (if running):** Press `Ctrl+C` in its Terminal
- **Proxy:** Press `Ctrl+C` in its Terminal
- **OpenClaw:** Press `Ctrl+C` in its Terminal

### Useful Commands

| Command | What It Does |
|---------|-------------|
| `curl -s http://127.0.0.1:18790/_openobscure/health \| python3 -m json.tool` | Check proxy status |
| `OPENOBSCURE_LOG=debug cargo run --release -- -c config/openobscure.toml` | Start proxy with verbose logs |
| `lsof -i :18790` | Check if proxy port is in use |
| `cargo run --release -- --init-key` | Regenerate encryption key |
| `ollama list` | Show downloaded local LLM models |
| `ollama pull qwen3:8b` | Download Qwen3 8B model |
| `ollama pull llama3.2:3b` | Download Llama 3.2 3B model |
| `curl http://127.0.0.1:11434/api/tags` | Check if Ollama is running |
