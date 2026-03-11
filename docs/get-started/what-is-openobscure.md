# What Is OpenObscure?

## The Problem

Every message, tool result, and file a user shares with an AI agent gets sent to third-party LLM APIs in plaintext — credit cards, health discussions, API keys, children's information, photos. Once that data leaves the device, the user has no control over how it is stored, logged, or used for training. Meanwhile, the responses coming back can contain persuasion and manipulation techniques designed to influence user behavior.

## What It Intercepts

OpenObscure sits between the AI agent and the LLM provider, operating at two independent interception points. On the **request path**, it scans JSON payloads for PII (structured, semantic, visual, and audio), encrypts matches with Format-Preserving Encryption so the LLM sees plausible-looking data instead of real values, and processes images to redact faces, text, and NSFW content with irreversible solid fill. On the **response path**, it decrypts ciphertexts back to original values and scans for persuasion techniques (urgency, fear, false authority, commercial pressure) before the response reaches the user. A second layer runs in-process inside the host agent to catch PII in tool results — web scrapes, file reads, API outputs — that never pass through the HTTP proxy.

## What It Does NOT Do

OpenObscure does not protect against a compromised operating system or root-level access — if an attacker controls the OS, they can read memory and bypass any userspace protection. It does not perform file I/O; the agent's tools read files and produce text, and OpenObscure only sees the resulting text after extraction. It does not provision, store, or manage LLM API keys — it forwards the host agent's credentials unchanged. It does not phone home, send telemetry, or contact external servers. The only network traffic it produces is forwarding the host agent's existing LLM requests through a localhost proxy.

## Learn More

For the full system architecture — layers, data flow, deployment models, threat model, and design decisions — see [System Overview](../architecture/system-overview.md).
