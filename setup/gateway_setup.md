# Gateway Setup: OpenClaw + OpenObscure + Discord

A step-by-step guide for running OpenClaw (AI agent) with OpenObscure (privacy firewall) using Discord as the chat interface, on MacBook and iPhone.

**No programming experience required.** This guide uses terminal commands you can copy and paste.

> **Prerequisites:** Complete the [common setup](README.md) first (dev tools, Rust, clone, model download).

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

In addition to the [common prerequisites](README.md):

- [ ] **Node.js** (20+): `brew install node`
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

### Step 1 — Build the Privacy Proxy

This compiles OpenObscure from source. It will take a few minutes on the first build.

```bash
cd ~/Desktop/OpenObscure/openobscure-core
cargo build --release
```

When it finishes without errors, the proxy is ready. The compiled program is at `target/release/openobscure-core`.

### Step 2 — Generate Your Encryption Key

This creates a unique encryption key and stores it securely in your Mac's Keychain (the same place your saved passwords go):

```bash
cargo run --release -- --init-key
```

You should see a message confirming the key was generated. This only needs to be done once.

### Step 3 — Set Up a Local LLM (Optional — Skip If Using Cloud API)

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

OpenObscure's config (`config/openobscure.toml`) already includes provider routes for Anthropic, OpenAI, OpenRouter, and Ollama. When you start the proxy, requests to each route prefix are forwarded to the corresponding upstream API:

| Route Prefix | Upstream URL | Use Case |
|---|---|---|
| `/anthropic` | `https://api.anthropic.com` | Anthropic Claude (direct) |
| `/openai` | `https://api.openai.com` | OpenAI GPT (direct) |
| `/openrouter` | `https://openrouter.ai/api` | OpenRouter (multi-provider) |
| `/ollama` | `http://127.0.0.1:11434` | Ollama (local LLM) |

For local LLMs via Ollama, no API key is needed — Ollama runs locally with no authentication.

```
Discord ──► OpenClaw ──► OpenObscure (:18790) ──► Ollama (:11434)
                          encrypts PII              local LLM
                                                    nothing leaves your Mac
```

> **Configuration reference:** See [examples/openobscure.toml](examples/openobscure.toml) for a fully commented example configuration file with all available options.

---

### (Optional) Check Your Hardware Tier

Before starting the proxy, you can confirm which capability tier and features your machine will use:

```bash
cd ~/Desktop/OpenObscure/openobscure-core
cargo run --release -- check-tier
```

Example output on an Apple Silicon Mac:

```
Hardware:  16384 MB RAM  ·  12 CPU cores
Tier:      full (≥8 GB — full feature set)

Feature Budget:
  RAM cap:          275 MB
  NER scanner:      yes (model: distilbert, pool: 2)
  CRF scanner:      yes
  Ensemble voting:  yes
  Image pipeline:   yes (OCR: full_recognition)
  Face detection:   yes (model: scrfd)
  NSFW detection:   yes
  Screen guard:     yes
  Voice KWS:        yes
  Response integrity: yes
  Gazetteer:        yes
  Keyword dict:     yes
  Model idle TTL:   300s
```

| Tier | RAM | NER Model | Face Model | Image Pipeline |
|------|-----|-----------|------------|----------------|
| **Full** | ≥8 GB | DistilBERT (higher accuracy) | SCRFD-2.5GF | Full OCR |
| **Standard** | 4–8 GB | TinyBERT (fast) | SCRFD-2.5GF | Full OCR |
| **Lite** | <4 GB | TinyBERT (fast) | UltraLight | Detect-and-fill |

No server is started — the command exits immediately after printing.

---

### Step 4 — Start the Privacy Proxy

```bash
cd ~/Desktop/OpenObscure/openobscure-core
cargo run --release -- -c config/openobscure.toml
```

You should see output like:

```
INFO openobscure_core: Listening on 127.0.0.1:18790
INFO openobscure_core: Device tier: full
INFO openobscure_core: FPE engine ready
```

**Leave this Terminal window open.** The proxy needs to keep running.

### Step 5 — Verify the Proxy Is Running

Open a **new Terminal window** (Cmd+N) and run:

```bash
TOKEN=$(cat ~/.openobscure/.auth-token)
curl -s -H "X-OpenObscure-Token: $TOKEN" http://127.0.0.1:18790/_openobscure/health | python3 -m json.tool
```

The first line reads the auth token that was auto-generated when the proxy first started. It is saved to `~/.openobscure/.auth-token` (mode 0600). Alternatively, if you set `OPENOBSCURE_AUTH_TOKEN` in your environment before starting the proxy, use that value instead:

```bash
curl -s -H "X-OpenObscure-Token: $OPENOBSCURE_AUTH_TOKEN" http://127.0.0.1:18790/_openobscure/health | python3 -m json.tool
```

You should see a JSON response containing `"status": "ok"`. This confirms the proxy is running and ready.

### Step 6 — Download and Build OpenClaw

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

> **Reinstalling or upgrading?** If you had a previous OpenClaw installation, purge stale config and state first — see [Purging a stale OpenClaw setup](#purging-a-stale-openclaw-setup) in Troubleshooting below. Then continue with Step 7.

### Step 7 — Create a Discord Bot

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

### Step 8 — Configure OpenClaw with OpenObscure

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

**Option C — Cloud LLM via OpenRouter (multi-provider access):**

```bash
cat > .env << 'ENVFILE'
DISCORD_BOT_TOKEN=paste-your-discord-bot-token-here
OPENROUTER_API_KEY=paste-your-openrouter-key-here
ENVFILE
```

OpenRouter gives you access to many LLM providers (Claude, GPT, Gemini, open-source models) through a single API key. Get one at https://openrouter.ai/keys.

**Option D — Local LLM (Ollama):**

```bash
cat > .env << 'ENVFILE'
DISCORD_BOT_TOKEN=paste-your-discord-bot-token-here
OLLAMA_API_KEY=ollama-local
ENVFILE
```

> `OLLAMA_API_KEY` can be any non-empty value — Ollama itself ignores it, but OpenClaw uses it to register Ollama as a provider.

Replace the placeholder values with your actual tokens from Step 7 and your LLM provider.

#### Create the configuration file

```bash
mkdir -p ~/.openclaw
```

Choose the config that matches your LLM provider. The `models.providers` section is the key integration point — it tells OpenClaw to route all LLM API calls through the OpenObscure proxy (`localhost:18790`) instead of directly to the provider. This is how PII gets intercepted and redacted before reaching the LLM.

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
  "models": {
    "providers": {
      "anthropic": {
        "baseUrl": "http://127.0.0.1:18790/anthropic",
        "models": [
          { "id": "claude-sonnet-4-6", "name": "Claude Sonnet 4.6" }
        ]
      }
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

Same structure as above, but change the `agents.defaults.model` and `models.providers` sections:

```json
  "agents": {
    "defaults": {
      "model": "openai/gpt-4.1"
    }
  },
  "models": {
    "providers": {
      "openai": {
        "baseUrl": "http://127.0.0.1:18790/openai/v1",
        "models": [
          { "id": "gpt-4.1", "name": "GPT-4.1" }
        ]
      }
    }
  },
```

> **Important:** The `baseUrl` must include `/v1` after the route prefix. OpenClaw appends paths like `/chat/completions` to this URL. Without `/v1`, the upstream URL becomes `/api/chat/completions` instead of `/api/v1/chat/completions`, which causes connection errors.

**Option C — OpenRouter (multi-provider):**

Same structure as Option A, but change the `agents.defaults.model` and `models.providers` sections:

```json
  "agents": {
    "defaults": {
      "model": "openrouter/anthropic/claude-sonnet-4-6"
    }
  },
  "models": {
    "providers": {
      "openrouter": {
        "baseUrl": "http://127.0.0.1:18790/openrouter/v1",
        "api": "openai-completions",
        "models": [
          {
            "id": "anthropic/claude-sonnet-4-6",
            "name": "Claude Sonnet 4.6 (via OpenRouter)",
            "input": ["text", "image"],
            "contextWindow": 200000,
            "maxTokens": 8192
          }
        ]
      }
    }
  },
```

Key details for OpenRouter:
- **Model prefix:** Use `openrouter/` followed by the OpenRouter model ID (e.g., `openrouter/anthropic/claude-sonnet-4-6`). OpenClaw splits on the first `/` to determine the provider.
- **`api` field:** Must be `"openai-completions"` — OpenRouter uses the OpenAI-compatible API format.
- **`baseUrl` includes `/v1`:** The proxy route is `/openrouter`, and OpenRouter's API path requires `/v1`. Without it, API calls fail with connection errors.
- **`input` field:** Include `["text", "image"]` if the model supports vision. This tells OpenClaw to send images directly in chat requests (enabling OpenObscure's image pipeline to scan them for PII).
- You can add multiple models to the `models` array (e.g., `auto` for OpenRouter's auto-routing).

**Option D — Local LLM (Ollama):**

Same structure as Option A, but change the `agents.defaults.model` and `models.providers` sections:

```json
  "agents": {
    "defaults": {
      "model": "ollama/qwen3:8b"
    }
  },
  "models": {
    "providers": {
      "ollama": {
        "baseUrl": "http://127.0.0.1:18790/ollama",
        "api": "ollama",
        "models": [
          { "id": "qwen3:8b", "name": "Qwen3 8B" }
        ]
      }
    }
  },
```

Replace `qwen3:8b` with `llama3.2:3b` (and update the `models` array to match) if you downloaded Llama instead. **No API key is needed** for the local option — your messages go from Discord to OpenClaw to OpenObscure to Ollama, all on your MacBook.

> **Note:** Ollama uses its own API format (`"api": "ollama"`), not the OpenAI format. The `baseUrl` does **not** need `/v1` — just the route prefix (`/ollama`).

> **How it works:** The `baseUrl` in each provider points to the OpenObscure proxy's route prefix (e.g. `/ollama`). The proxy strips the route prefix from the URL path and forwards the remainder to the real provider's upstream URL. For example, a request to `http://127.0.0.1:18790/openrouter/v1/chat/completions` gets the `/openrouter` prefix stripped, then forwards `/v1/chat/completions` to `https://openrouter.ai/api`. The proxy scans request bodies for PII, redacts or encrypts matches, and restores PII in the response before sending it back to OpenClaw.
>
> **`baseUrl` and API versioning:** For providers that use the OpenAI-compatible API format (`"api": "openai-completions"`), include `/v1` after the route prefix in `baseUrl` (e.g. `http://127.0.0.1:18790/openai/v1`). OpenClaw appends `/chat/completions` to this URL, so without `/v1` the upstream path would be wrong. Anthropic and Ollama use their own API formats and handle versioning internally — their `baseUrl` does not need `/v1`.

### Step 9 — Build and Verify the OpenObscure Plugin

The OpenObscure plugin ships bundled with OpenClaw in `extensions/openobscure-plugin/`. Step 8 already enabled it in `openclaw.json`, but you need to compile it first.

```bash
cd ~/Desktop/openclaw/extensions/openobscure-plugin
pnpm install
pnpm run build
```

Verify the build succeeded:

```bash
ls ~/Desktop/openclaw/extensions/openobscure-plugin/dist/index.js
```

You should see the file listed. The plugin will activate automatically when the gateway starts (Step 10) and will connect to the OpenObscure proxy running on port 18790 (Step 4).

When the gateway starts successfully with the plugin, you should see these log lines:

```
[OpenObscure L1] [plugin] Auth token loaded path=/Users/<you>/.openobscure/.auth-token
[gateway] Heartbeat monitor started (proxy: http://127.0.0.1:18790)
[gateway] OpenObscure plugin registered (redactor=true, heartbeat=true)
```

> **If the plugin doesn't load:** Check that `dist/` exists (it's created by `pnpm run build`). If the gateway logs don't show the lines above at startup, the plugin wasn't compiled or the `plugins` section in `openclaw.json` is missing.

### Step 10 — Start OpenClaw

```bash
cd ~/Desktop/openclaw
pnpm openclaw gateway
```

You should see output indicating the gateway has started and the Discord bot has connected. Go to your Discord server — the bot should appear as **online** in the member list.

> **First time?** You can run `pnpm openclaw onboard` for an interactive setup wizard that walks you through configuration and optionally installs the gateway as a background service (`pnpm openclaw onboard --install-daemon`).

**If using Ollama (local LLM):** Make sure the Ollama server from Step 3 is still running in its own Terminal window. You should now have **three** Terminal windows open: Ollama, OpenObscure proxy, and OpenClaw.

**DM pairing:** The first time you message the bot via DM, it will respond with a short pairing code. Approve it from the MacBook Terminal:

```bash
cd ~/Desktop/openclaw
pnpm openclaw pairing approve discord <code>
```

After approval, the bot will respond normally to your messages.

### Step 11 — Test It

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

Your MacBook must be **running and connected to the internet** for the bot to respond. If you close the Terminal windows from Part 1 (Steps 4 and 10, plus Step 3 if using Ollama), the bot will go offline.

---

## Part 3: Verifying PII Protection

These steps confirm that OpenObscure is actually protecting your data. **All tests work from both MacBook and iPhone** — send the messages from whichever device you're testing. Tests 1–4 include MacBook-only log checks. Tests 5–8 use behavioral verification that works from any device, including iPhone. Tests 9–10 verify the R2 cognitive firewall (persuasion/manipulation detection on LLM responses).

### Test 1 — Credit Card Number

**Send this message in Discord:**

```
My Visa card is 4532-8970-1234-5678 and it expires 12/28.
```

**What to check (MacBook — proxy logs):**

In the Terminal window where the proxy is running (Step 4), you should see:

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
TOKEN=$(cat ~/.openobscure/.auth-token)
curl -s -H "X-OpenObscure-Token: $TOKEN" http://127.0.0.1:18790/_openobscure/health | python3 -m json.tool
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

1. Make sure the proxy Terminal window from Step 4 is still open and running
2. Check if something else is using port 18790:
   ```bash
   lsof -i :18790
   ```
3. If another process is using the port, either stop it or change the port in `config/openobscure.toml`

### "FPE key not found" error

See [FPE Configuration](../docs/configure/fpe-configuration.md) for key generation, storage, and rotation details. Quick fix — run the key generation again:

```bash
cd ~/Desktop/OpenObscure/openobscure-core
cargo run --release -- --init-key
```

If you see a keychain access dialog, click **Allow** or **Always Allow**.

### Bot appears offline in Discord

1. Make sure the OpenClaw Terminal (Step 10) is still running
2. Verify your Discord bot token in the `.env` file is correct (the env var is `DISCORD_BOT_TOKEN`)
3. Check that you enabled **Message Content Intent** in the Discord Developer Portal (Step 7)
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

1. Verify the proxy is running (Step 4)
2. Make sure the OpenObscure plugin is enabled in `~/.openclaw/openclaw.json` with `proxyUrl` set to `http://127.0.0.1:18790`
3. Check proxy logs for errors — if you see no log activity at all when chatting, the plugin may not be connecting to the proxy
4. Restart OpenClaw after any config changes

### "Model not found" errors in proxy logs

Models are stored in Git LFS. Pull them:

```bash
cd ~/Desktop/OpenObscure
git lfs install   # one-time setup
git lfs pull      # re-download all models
```

If you don't have Git LFS, use the fallback scripts instead:

```bash
./build/download_models.sh full
./build/download_kws_models.sh
```

### "Unknown model: ollama/..." error

OpenClaw requires `OLLAMA_API_KEY` in your `.env` file to register Ollama as a provider (any non-empty value works). Add this line to `~/Desktop/openclaw/.env`:

```
OLLAMA_API_KEY=ollama-local
```

Then restart the gateway.

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

### PII not being redacted (proxy bypassed)

If the AI responds to PII-containing messages without any sign of redaction, OpenClaw may be talking directly to the LLM instead of going through the proxy. This happens when a stale `models.json` cache preserves the old provider URL.

Fix: delete the cached model config and restart the gateway:

```bash
rm ~/.openclaw/agents/main/agent/models.json
# Then restart the gateway
cd ~/Desktop/openclaw && pnpm openclaw gateway
```

Verify the fix:

```bash
cat ~/.openclaw/agents/main/agent/models.json | python3 -c "import json,sys; d=json.load(sys.stdin); print(d['providers']['ollama']['baseUrl'])"
# Should print: http://127.0.0.1:18790/ollama (not http://127.0.0.1:11434)
```

### Slow first response

The first message after starting the proxy may take a few extra seconds while AI models load into memory. Subsequent messages will be faster. If using a local LLM via Ollama, the first response will also include model loading time — this is normal.

### Connection errors or "terminated connection" with OpenRouter/OpenAI

If you see connection termination errors, retries, or non-streaming responses when using OpenRouter or OpenAI, the most likely cause is a missing `/v1` in the `baseUrl`.

**Wrong:**
```json
"baseUrl": "http://127.0.0.1:18790/openrouter"
```

**Correct:**
```json
"baseUrl": "http://127.0.0.1:18790/openrouter/v1"
```

Without `/v1`, the API path becomes `/api/chat/completions` instead of `/api/v1/chat/completions`. The upstream API returns an error or a non-streaming response, and OpenClaw retries repeatedly.

Check both `~/.openclaw/openclaw.json` and `~/.openclaw/agents/main/agent/models.json` — the `baseUrl` must include `/v1` in both files.

### Image PII redaction not working (images bypass proxy)

If images in Discord messages are not being scanned for faces or OCR text, check these in order:

1. **Model in `models.json` must include `"input": ["text", "image"]`** — this tells OpenClaw that the model supports vision natively, so images are injected directly into chat requests (where the proxy can scan them). Without this, OpenClaw may use a separate vision model or skip image injection entirely.

2. **Auth profiles can cause direct API calls** — if `~/.openclaw/agents/main/agent/auth-profiles.json` contains API keys for providers like `anthropic`, OpenClaw's auto-discovery (`AUTO_IMAGE_KEY_PROVIDERS`) will send vision model requests directly to those providers, bypassing the proxy. Clear the profiles file:
   ```bash
   echo '{"version": 1, "profiles": {}}' > ~/.openclaw/agents/main/agent/auth-profiles.json
   ```

3. **Verify images appear in proxy logs** — if the proxy shows `base64_processed=0` and `url_refs_found=0`, the images are not reaching the proxy at all. Check that `baseUrl` points to the proxy (not directly to the provider).

### OpenClaw daemon keeps restarting

If OpenClaw was installed as a background service via `pnpm openclaw onboard --install-daemon`, it runs under launchd and automatically restarts. To stop it permanently:

```bash
# Find and unload the launchd service
launchctl list | grep openclaw
launchctl unload ~/Library/LaunchAgents/com.openclaw.gateway.plist
```

To restart manually after stopping:

```bash
cd ~/Desktop/openclaw
pnpm openclaw gateway
```

### OpenObscure plugin not loading

If gateway startup logs don't show the OpenObscure plugin loading:

1. **Check that `dist/` exists** — the plugin must be compiled first:
   ```bash
   cd ~/Desktop/openclaw/extensions/openobscure-plugin
   pnpm run build
   ls dist/index.js
   ```

2. **Check `openclaw.json`** — the `plugins` section must be present with `openobscure-plugin` enabled:
   ```json
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
   ```

3. **Restart the gateway** after any plugin changes — plugins are loaded at startup.

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

If you previously used `LLM_API_BASE` or `DISCORD_TOKEN` (without `_BOT`) in your `.env`, delete the old file and recreate it per Step 8:

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

Then proceed with Step 8 to reconfigure from scratch.

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
cd ~/Desktop/OpenObscure/openobscure-core
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
cd ~/Desktop/OpenObscure/openobscure-core
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

> **Fail mode:** The proxy defaults to `fail_mode = "open"` — if PII scanning errors occur, the original request is forwarded unchanged (AI functionality is never blocked). Set `fail_mode = "closed"` in `openobscure.toml` for strict privacy mode where any processing failure returns a 502 error. Vault unavailable always blocks regardless of this setting.

### Useful Commands

| Command | What It Does |
|---------|-------------|
| `TOKEN=$(cat ~/.openobscure/.auth-token) && curl -s -H "X-OpenObscure-Token: $TOKEN" http://127.0.0.1:18790/_openobscure/health \| python3 -m json.tool` | Check proxy status |
| `OPENOBSCURE_LOG=debug cargo run --release -- -c config/openobscure.toml` | Start proxy with verbose logs |
| `lsof -i :18790` | Check if proxy port is in use |
| `cargo run --release -- --init-key` | Regenerate encryption key |
| `cargo run --release -- key-rotate` | Rotate FPE key (zero-downtime, 30s overlap window) |
| `cargo run --release -- passthrough` | Run in passthrough mode (no scanning/encryption) |
| `cargo run --release -- service install` | Install as background service (launchd/systemd) |
| `cargo run --release -- service start` | Start the installed background service |
| `cargo run --release -- service stop` | Stop the background service |
| `cargo run --release -- service status` | Check background service status |
| `cargo run --release -- service uninstall` | Remove the background service |
| `ollama list` | Show downloaded local LLM models |
| `ollama pull qwen3:8b` | Download Qwen3 8B model |
| `ollama pull llama3.2:3b` | Download Llama 3.2 3B model |
| `curl http://127.0.0.1:11434/api/tags` | Check if Ollama is running |
