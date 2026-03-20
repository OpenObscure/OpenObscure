# Embedded Integration Architecture

How OpenObscure integrates into third-party chat apps via the embedded model.
This document covers the data flow for both iOS (Enchanted) and Android (RikkaHub)
reference implementations.

## Core Principle

**The LLM never sees real PII.** All sensitive data is encrypted/tokenized before
leaving the device. The LLM works with opaque tokens (`PER_0dpx`, `HLT_k7qh`,
`487-14-6147`). Restore happens only at the UI rendering layer — never in shared
state or persistent storage.

## Data Flow: Text Sanitization

Both platforms follow the same logical flow, adapted to their frameworks.

### Outbound (User Message to LLM)

```mermaid
sequenceDiagram
    participant User
    participant App as App Layer<br>(ConversationStore / ChatService)
    participant OO as OpenObscure<br>(Rust Core)
    participant LLM

    User->>App: Send message with PII<br>"Angela Martinez, SSN 412-55-8823"
    App->>OO: sanitize_text(message)
    OO->>OO: Regex scan (SSN, phone, email)
    OO->>OO: NER scan (DistilBERT/TinyBERT)
    OO->>OO: FPE encrypt (FF1/AES-256)
    OO-->>App: "PER_0dpx, SSN 487-14-6147"<br>+ mapping_json
    App->>App: Cache sanitized content<br>(skip NER on next turn)
    App->>LLM: Send sanitized message
    Note over LLM: LLM sees only tokens<br>Never sees real PII
```

### Inbound (LLM Response to User)

```mermaid
sequenceDiagram
    participant LLM
    participant App as App Layer
    participant OO as OpenObscure<br>(Rust Core)
    participant DB as Database
    participant UI as UI Layer

    LLM-->>App: Stream response chunks<br>"The name is PER_0dpx"
    App->>DB: Save raw tokens to DB<br>(content = "PER_0dpx")
    App->>OO: scanResponse(fullText)
    OO->>OO: R1 dictionary scan
    OO->>OO: R2 TinyBERT (if R1 flags)
    OO-->>App: RiReport (severity, categories)<br>or null (clean)
    App->>App: Store RI flag if flagged
    UI->>OO: restore(rawTokenText)
    OO-->>UI: "The name is Angela Martinez"
    UI->>UI: Prepend RI warning if flagged
    Note over UI: User sees restored text<br>DB keeps tokens
```

## iOS Architecture (Enchanted)

### Key Files Modified

| File | Role |
|------|------|
| `Stores/ConversationStore.swift` | Orchestrates sanitize/restore/RI flow |
| `SwiftData/Models/MessageSD.swift` | SwiftData model with `@Transient displayContent` |
| `OpenObscureManager.swift` | Singleton wrapping Rust FFI calls |

### Sanitize Flow (sendPrompt)

```mermaid
sequenceDiagram
    participant CS as ConversationStore
    participant OOM as OpenObscureManager
    participant DB as SwiftData
    participant Ollama

    CS->>CS: Build rawMessages from DB<br>(user msgs = plaintext,<br>assistant msgs = tokens)
    CS->>CS: Check sanitizedContent cache
    alt Cache hit
        CS->>CS: Use cached sanitized text
    else Cache miss (new message)
        CS->>OOM: sanitize(newUserMsg)
        OOM->>OOM: Rust: scan + FPE encrypt
        OOM-->>CS: sanitizedText + mappings
        CS->>DB: Save sanitizedContent on MessageSD
    end
    CS->>CS: Build LLM request<br>(cached user + token assistant)
    CS->>Ollama: Send sanitized request
```

### Restore Flow (handleComplete)

```mermaid
sequenceDiagram
    participant CS as ConversationStore
    participant OOM as OpenObscureManager
    participant DB as SwiftData
    participant UI as SwiftUI View

    Note over CS: Stream completes
    CS->>CS: Flush streaming buffer
    CS->>DB: Save content (raw tokens)
    CS->>CS: Set .completed (sync on MainActor)
    CS->>OOM: restore(rawTokenText)
    OOM-->>CS: restoredText
    CS->>OOM: scanResponse(restoredText)
    OOM-->>CS: RiReport or null
    CS->>CS: Set displayContent =<br>warning + restoredText
    Note over DB: content = "PER_0dpx" (tokens)<br>displayContent = "Angela Martinez"<br>(@Transient, never persisted)
    UI->>UI: Read displayContent ?? content
```

### Why @Transient displayContent

```mermaid
sequenceDiagram
    participant CS as ConversationStore
    participant SD as SwiftData AutoSave
    participant DB as SQLite

    Note over CS: Without @Transient (OLD - BROKEN)
    CS->>CS: content = "Angela Martinez" (for display)
    CS->>CS: createMessage(nextMsg)
    SD->>SD: saveChanges() triggers
    SD->>DB: Persists ALL observed properties<br>including content = "Angela Martinez"
    Note over DB: DB now has plaintext!<br>Next turn sends plaintext to LLM

    Note over CS: With @Transient (NEW - FIXED)
    CS->>CS: content = "PER_0dpx" (tokens, saved)
    CS->>CS: displayContent = "Angela Martinez"
    CS->>CS: createMessage(nextMsg)
    SD->>SD: saveChanges() triggers
    SD->>DB: Skips @Transient properties<br>content = "PER_0dpx" preserved
    Note over DB: DB always has tokens
```

## Android Architecture (RikkaHub)

### Key Files Modified

| File | Role |
|------|------|
| `OpenObscureInterceptor.kt` | OkHttp interceptor, outbound-only |
| `OpenObscureManager.kt` | Singleton: sanitize, restore, RI, image, cache |
| `ChatService.kt` | Stream handling, RI scan in onSuccess |
| `ChatMessage.kt` | Compose `rememberRestoredText()` with RI warning |
| `ChatVM.kt` | Reset/load mappings on conversation switch |
| `ConversationEntity.kt` | Room entity with `mappingJson` column |

### Interceptor Flow (Outbound)

```mermaid
sequenceDiagram
    participant RH as RikkaHub App
    participant INT as OpenObscureInterceptor
    participant OOM as OpenObscureManager
    participant LLM as LLM Provider

    RH->>INT: HTTP POST /chat/completions<br>(JSON body with messages)
    INT->>INT: Parse JSON, detect request type
    alt Auto-generated (title/suggestion)
        INT->>INT: model="auto", stream=false
        INT->>OOM: sanitizeMessagesIsolated(msgs)
        Note over OOM: Disposable tokens<br>Not merged into conversation pool
    else Chat request
        INT->>INT: model="llava:13b", stream=true
        INT->>OOM: sanitizeMessages(msgs)<br>with cache + stable tokens
    end
    INT->>INT: Rebuild JSON with sanitized content<br>Keep multimodal (image) parts intact
    INT->>LLM: Forward sanitized request
    LLM-->>RH: Response passes through unmodified
```

### Restore Flow (Compose Layer)

```mermaid
sequenceDiagram
    participant CS as ChatService
    participant OOM as OpenObscureManager
    participant Compose as rememberRestoredText()
    participant UI as User Screen

    Note over CS: Stream completes (onSuccess)
    CS->>CS: Save conversation (raw tokens in StateFlow)
    CS->>OOM: restore(lastAssistantText)
    CS->>OOM: scanResponse(restoredText)
    alt RI flagged
        CS->>OOM: setRiWarning(text, severity)
        OOM->>OOM: riVersion++ (triggers recompose)
    end

    Note over Compose: On each render
    Compose->>Compose: remember(text, riVersion)
    Compose->>OOM: restore(rawTokenText)
    OOM-->>Compose: restoredText
    Compose->>OOM: getRiWarning(rawTokenText)
    alt Warning exists
        Compose->>UI: "--- WARNING ---" + restoredText
    else Clean
        Compose->>UI: restoredText
    end
    Note over UI: StateFlow unchanged<br>Always has raw tokens
```

### Mapping Isolation (Auto-Generated Requests)

```mermaid
sequenceDiagram
    participant INT as Interceptor
    participant OOM as OpenObscureManager

    Note over INT: Turn 1: Chat request
    INT->>OOM: sanitizeMessages([userMsg])
    OOM->>OOM: NER scan, FPE encrypt
    OOM->>OOM: mappings = 11 entries
    OOM-->>INT: sanitized + mappings

    Note over INT: Title generation (auto)
    INT->>OOM: sanitizeMessagesIsolated([titlePrompt])
    OOM->>OOM: NER scan, FPE encrypt<br>(different tokens)
    Note over OOM: NOT merged into pool<br>mappings still = 11

    Note over INT: Suggestion generation (auto)
    INT->>OOM: sanitizeMessagesIsolated([suggestPrompt])
    Note over OOM: NOT merged into pool<br>mappings still = 11

    Note over INT: Turn 2: Chat request
    INT->>OOM: sanitizeMessages([...history])
    Note over OOM: mappings still = 11<br>(not 35+ from auto requests)
```

## Image Sanitization Pipeline

Same pipeline on both platforms. The integration point differs:
- **iOS**: Called directly in `ConversationStore.sendPrompt()` before building the LLM request
- **Android**: Called in `OpenObscureInterceptor.sanitizeMultimodalMessage()` when processing `image_url` parts

```mermaid
sequenceDiagram
    participant App
    participant OO as OpenObscure Rust Core
    participant NSFW as NSFW Classifier<br>(ViT INT8)
    participant Face as Face Detector<br>(SCRFD)
    participant OCR as OCR Engine<br>(PaddleOCR)

    App->>OO: sanitizeImage(imageBytes)
    OO->>OO: Decode image (strips EXIF)
    OO->>OO: Screen guard (screenshot detection)
    OO->>NSFW: Classify (5 classes)
    alt NSFW detected (score > 0.5)
        NSFW-->>OO: nsfw=true
        OO->>OO: Solid fill entire image
        Note over OO: Skip face + OCR
    else Safe
        NSFW-->>OO: nsfw=false
        OO->>Face: Detect faces
        Face-->>OO: bounding boxes + confidence
        OO->>OO: Solid fill each face bbox
        OO->>OO: OCR pre-filter (edge density)
        alt Density in [0.05, 0.12] or screenshot
            OO->>OCR: Detect text regions
            OCR-->>OO: text bounding boxes
            OO->>OO: Solid fill text regions
        else Density > 0.12 (photo) or < 0.05 (blank)
            Note over OO: Skip OCR (~5s saved)
        end
    end
    OO->>OO: Re-encode as JPEG
    OO-->>App: sanitizedImageBytes
```

## Session Mapping Lifecycle

```mermaid
sequenceDiagram
    participant User
    participant App
    participant OOM as OpenObscureManager
    participant DB as Database

    Note over User: Start conversation A
    App->>OOM: resetMappings()
    App->>DB: Load mappingJson for conv A
    App->>OOM: loadMappings(json)

    loop Each turn
        App->>OOM: sanitize(newMsg)
        OOM->>OOM: Accumulate mappings
        App->>DB: Save mappingJson
    end

    Note over User: Switch to conversation B
    App->>OOM: resetMappings()
    App->>DB: Load mappingJson for conv B
    App->>OOM: loadMappings(json)
    Note over OOM: Clean slate<br>No cross-conversation leaks
```

## Platform Comparison

| Aspect | iOS (Enchanted) | Android (RikkaHub) |
|--------|----------------|-------------------|
| **Sanitize entry** | `ConversationStore.sendPrompt()` | `OpenObscureInterceptor.intercept()` |
| **DB token storage** | `@Transient displayContent` on MessageSD | StateFlow unchanged, Compose restores |
| **Restore entry** | `handleComplete()` + `restoreMessagesForDisplay()` | `rememberRestoredText()` (Compose) |
| **RI warning** | `riWarningLabel()` in handleComplete | `setRiWarning()` + `riVersion` recompose |
| **Cache mechanism** | `sanitizedContent` on MessageSD (persisted) | `sanitizeCache` HashMap (in-memory) |
| **Mapping persistence** | `mappingJson` on ConversationSD | `mappingJson` on ConversationEntity (Room) |
| **Image sanitize** | Direct call in ConversationStore | Interceptor multimodal part processing |
| **Auto-gen isolation** | N/A (Enchanted has no auto-gen) | `sanitizeMessagesIsolated()` |
| **Conversation switch** | `selectConversation()` | `ChatVM.init` observer |
