# Crash Recovery and Post-Mortem Diagnostics

OpenObscure writes three crash artifacts that survive SIGKILL, OOM kills, and Rust panics. This document describes each artifact's structure, how to read them manually, and a step-by-step workflow for diagnosing what happened after an unexpected proxy termination.

---

## Artifacts Overview

| Artifact | Path | Always present | Survives SIGKILL/OOM |
|----------|------|----------------|----------------------|
| Panic marker | `~/.openobscure/.crashed` | No — only on Rust panic | No — requires graceful OS flush |
| Request journal | `~/.openobscure/request_journal.buf` | Yes (opened on startup) | Yes — mmap-backed |
| Crash buffer | `~/.openobscure/crash.buf` | No — must be enabled | Yes — mmap-backed |

**Panic marker** is written by a Rust panic hook — it captures the panic message and timestamp. It is not written on SIGKILL, OOM kill, or hardware faults. It is deleted automatically the next time the proxy starts successfully.

**Request journal** is opened unconditionally on every startup. It records in-flight FPE requests so that incomplete ones can be detected after a crash. This is the primary tool for assessing data exposure.

**Crash buffer** is an optional ring buffer of all structured log lines. It requires explicit configuration (see [Enabling the crash buffer](#enabling-the-crash-buffer)). It provides the last N log lines leading up to the crash, even after SIGKILL.

---

## Enabling the Crash Buffer

The crash buffer is disabled by default. Enable it in `config/openobscure.toml`:

```toml
[logging]
crash_buffer = true
crash_buffer_size = 4194304  # 4MB — default is 2MB
```

When enabled, the proxy prints to stderr on startup:

```
[OpenObscure] Crash buffer enabled (4096KB at /home/user/.openobscure/crash.buf)
```

The buffer persists across restarts — it is opened with `truncate = false` and does not clear on startup. It accumulates entries until the ring wraps.

---

## Crash Buffer File Format

The crash buffer is a binary file with a fixed-size header followed by a ring of UTF-8 log lines.

```
Offset  Size   Field
──────  ────   ─────────────────────────────────────────────────────
0       8      Write offset: u64 little-endian. Position of the next
               write within the ring region. Wraps at ring_size.
8       N-8    Ring region: UTF-8 log lines separated by newlines.
               N = crash_buffer_size (default 2,097,152 bytes).
               The ring is zero-padded in unfilled regions.
```

**Ring read order.** The write offset points to the *oldest* data (the next write will overwrite it). To reconstruct chronological order:

1. Read the 8-byte offset as a u64 LE integer → call it `W`.
2. Read bytes `[8 + W .. end_of_file]` → tail segment (oldest entries).
3. Read bytes `[8 .. 8 + W]` → head segment (newest entries).
4. Concatenate: `tail + head`.
5. Trim leading null bytes (`\0`) from the tail — they are unfilled regions from a fresh or recently-reset buffer.

**What each line looks like.** The crash buffer uses the `tracing_subscriber::fmt` format with ANSI codes and target fields disabled:

```
2026-03-11T14:22:01.438Z  WARN proxy: FPE error on span  request_id=3f2a… type=SSN
2026-03-11T14:22:01.439Z  INFO proxy: Forwarding upstream request_id=3f2a… provider=openai
```

Lines are written at the `Drop` of each `tracing` event — there is no partial-line risk from mid-write crashes. A log line from the moment of SIGKILL may be absent if the mmap page had not been written yet, but all prior lines are present.

### Reading the crash buffer manually

**With Python (quickest):**

```python
import struct, sys

path = os.path.expanduser("~/.openobscure/crash.buf")
with open(path, "rb") as f:
    data = f.read()

offset = struct.unpack_from("<Q", data, 0)[0]
ring = data[8:]
ring_size = len(ring)
offset = min(offset, ring_size)  # bounds-check a corrupted header

# Reconstruct: tail first (oldest), then head (newest)
content = ring[offset:] + ring[:offset]
text = content.replace(b"\x00", b"").decode("utf-8", errors="replace")
print(text)
```

**With dd + hexdump (no Python):**

```bash
# Read the 8-byte offset
od -A none -t u8 -j 0 -N 8 ~/.openobscure/crash.buf

# Extract the full ring (skip 8-byte header)
dd if=~/.openobscure/crash.buf bs=1 skip=8 2>/dev/null | tr -d '\000' | less
```

> The `dd` approach does not reorder the ring around the write offset — you will see entries in write order, which may begin mid-line if the ring has wrapped. Use the Python approach for a clean chronological view.

---

## Request Journal File Format

The request journal is always enabled and uses a 64KB ring buffer that holds approximately 400 entries (at ~160 bytes each). Each entry is a pipe-delimited line:

```
J|<request_id>|<timestamp>|<mapping_count>|<completed>
```

| Field | Type | Description |
|-------|------|-------------|
| `J` | literal | Record type discriminator. Lines that do not start with `J\|` are skipped. |
| `request_id` | UUID v4 | Unique identifier for the request. Matches the `request_id` field in all crash buffer log lines for this request. |
| `timestamp` | i64 | Unix timestamp in seconds (UTC) when the entry was written. For start entries: time the request was forwarded upstream. For completion entries: time the response was fully processed. |
| `mapping_count` | u32 | Number of FPE-encrypted PII spans in the request body. Each span corresponds to one ciphertext token sent to the LLM. |
| `completed` | `0` or `1` | `0` = request was in-flight when written. `1` = response was fully processed and mappings were removed from the in-memory store. |

**Example lines:**

```
J|3f2a9c1e-47b8-4d2e-b901-a8f0c3e21047|1741701721|3|0
J|3f2a9c1e-47b8-4d2e-b901-a8f0c3e21047|1741701724|3|1
```

The first line is written before the upstream request is sent. The second line is written after the response is processed. A crash between these two lines leaves only the start entry — this is an **incomplete entry**.

### When completion entries are written

Completion entries (`completed=1`) are written at four points in `proxy.rs`:

| Response type | Completion point |
|---------------|-----------------|
| Non-streaming JSON response | After `process_response_body()` returns and mappings are removed from the store. |
| SSE stream: `[DONE]` frame | When the `data: [DONE]` line is received. |
| SSE stream: transport error | On any stream read error — treated as end-of-stream. |
| SSE stream: clean end (no `[DONE]`) | When the response body iterator returns `None`. |

**Journal entries are only written for requests that had PII.** If a request contained no FPE-encrypted spans (`mapping_count = 0`), no entry is written and no crash exposure exists for that request.

---

## Interpreting Incomplete Journal Entries

An incomplete entry means the proxy:

1. Detected PII in the request body.
2. Replaced PII spans with FPE ciphertext tokens.
3. Stored the ciphertext→plaintext mapping in memory.
4. Forwarded the encrypted request to the LLM upstream.
5. **Crashed before processing the LLM response** — the in-memory mappings were never used to decrypt the response.

### Consequence

The LLM received ciphertext tokens. Its response may reference those tokens back (for example, "I see you mentioned `_oo_abc123ef_`"). The proxy never decrypted the response, so the agent received — or never received — the raw response with opaque ciphertext in it.

### Reading `mapping_count`

`mapping_count` tells you how many PII spans were encrypted in that request. A count of 3 means the request body contained three distinct PII values (for example, one email, one phone number, one credit card). The types are not recorded in the journal; correlate with crash buffer log lines using `request_id`.

### Correlating with the crash buffer

Search the crash buffer for the `request_id` to find:

- The PII types that were detected (logged as match counts, never plaintext values).
- Whether the upstream request was sent successfully.
- Whether a response was received before the crash.

```bash
grep "3f2a9c1e-47b8-4d2e-b901-a8f0c3e21047" <(python3 read_crash_buf.py)
```

---

## Startup Warning Messages

On every startup, the proxy reads the journal and logs a warning for each incomplete entry. These appear before the first request is served:

```
WARN proxy: Incomplete journaled request from previous run (possible crash during FPE forward)
    request_id=3f2a9c1e-47b8-4d2e-b901-a8f0c3e21047
    timestamp=1741701721
    mapping_count=3

WARN proxy: Found incomplete journaled requests count=1
```

The proxy continues to start normally — incomplete entries are advisory. The journal is not cleared; new entries are appended to the same ring.

---

## False Positives: Ring Wrap

The journal is 64KB and holds ~400 entries. Under high request volume, the ring wraps and the oldest entries are overwritten. If a start entry (`completed=0`) survives into the new ring cycle but its matching completion entry (`completed=1`) was already overwritten, `read_incomplete()` will report it as incomplete even though it completed.

**How to identify ring-wrap false positives:**

- The `timestamp` on the entry is significantly older than the crash time.
- The gap between the entry's timestamp and current time exceeds the time it would take for 200+ requests to complete.
- The crash buffer shows no corresponding request activity near the timestamp.

If you see 10 or more incomplete entries spanning a wide timestamp range after a restart with no crash marker present, the most likely explanation is ring wrap, not a crash during those requests.

To reduce false positives under high load, increase the journal size by replacing the `JOURNAL_DEFAULT_SIZE` constant or using a larger `crash_buffer_size` for the crash buffer. Currently `request_journal.buf` size is not configurable via TOML — it is fixed at 64KB.

---

## Panic Marker

When the proxy crashes due to a Rust `panic!`, it writes `~/.openobscure/.crashed` before aborting. The file contains:

```
timestamp=2026-03-11T14:22:03.441Z
message=thread 'tokio-runtime-worker' panicked at 'index out of bounds: ...', src/body.rs:142:5
```

This file is **not written** for:
- SIGKILL (`kill -9`)
- OOM kill by the Linux kernel (`oom_killer`)
- Hardware faults
- `panic = "abort"` builds (the panic hook runs before abort, but only if there is time)

The marker is deleted automatically by `check_crash_marker()` on the next successful startup, after logging the recovery event. If the file persists across multiple restarts, the proxy is crashing before it reaches `check_crash_marker()` — check file permissions and disk space.

---

## Step-by-Step Diagnostic Workflow

### Step 1: Check for a panic marker

```bash
cat ~/.openobscure/.crashed
```

- **File exists:** The proxy panicked. The message contains the panic location and thread name. Proceed to step 4 for the crash buffer.
- **File absent:** The proxy was killed externally (SIGKILL/OOM) or the panic occurred before the hook ran. Proceed to step 2.

### Step 2: Check the system kill reason

```bash
# Linux — check dmesg for OOM kill
dmesg | grep -i "out of memory\|oom_kill\|openobscure"

# Linux — check systemd journal
journalctl -u openobscure --since "1 hour ago" | grep -i "kill\|oom\|signal"

# macOS — check unified log
log show --predicate 'process == "openobscure-proxy"' --last 1h | grep -i "kill\|crash"
```

An OOM kill shows as `Killed process <pid> (openobscure-proxy) total-vm:...` in dmesg.

### Step 3: Read the startup warning for incomplete journal entries

On the next startup, the proxy logs incomplete entries automatically. Check your log output or the log file:

```bash
grep "Incomplete journaled request\|Found incomplete" ~/.openobscure/openobscure.log
```

Or, to read the journal binary directly without restarting the proxy:

```python
import struct, uuid

path = os.path.expanduser("~/.openobscure/request_journal.buf")
with open(path, "rb") as f:
    data = f.read()

offset = struct.unpack_from("<Q", data, 0)[0]
ring = data[8:]
ring_size = len(ring)
offset = min(offset, ring_size)
content = (ring[offset:] + ring[:offset]).replace(b"\x00", b"").decode("utf-8", errors="replace")

# Parse all entries
completed_ids = set()
entries = []
for line in content.splitlines():
    parts = line.split("|")
    if len(parts) == 5 and parts[0] == "J":
        request_id, timestamp, mapping_count, done = parts[1], int(parts[2]), int(parts[3]), parts[4] == "1"
        if done:
            completed_ids.add(request_id)
        entries.append((request_id, timestamp, mapping_count, done))

incomplete = [(rid, ts, mc) for rid, ts, mc, done in entries if not done and rid not in completed_ids]
for rid, ts, mc in incomplete:
    print(f"INCOMPLETE  request_id={rid}  timestamp={ts}  mapping_count={mc}")
```

### Step 4: Read the crash buffer for context

```python
import struct, os

path = os.path.expanduser("~/.openobscure/crash.buf")
if not os.path.exists(path):
    print("Crash buffer not found — was crash_buffer = true set in [logging]?")
else:
    with open(path, "rb") as f:
        data = f.read()
    offset = struct.unpack_from("<Q", data, 0)[0]
    ring = data[8:]
    ring_size = len(ring)
    offset = min(offset, ring_size)
    content = (ring[offset:] + ring[:offset]).replace(b"\x00", b"").decode("utf-8", errors="replace")
    print(content)
```

Look for:
- The last few log lines before the crash (the buffer has chronological ordering after reconstruction).
- `WARN` or `ERROR` lines immediately before the gap.
- `FPE error`, `upstream error`, `OOM`, or `panic` messages.

### Step 5: Correlate incomplete requests

For each incomplete `request_id` found in step 3, search the crash buffer:

```bash
python3 read_crash_buf.py | grep "<request_id>"
```

This shows:
- What PII types were detected (counts logged as `pii_count=N type=...`, never plaintext).
- Whether the upstream send succeeded (`INFO proxy: Forwarding upstream`).
- Whether a response was received (`INFO proxy: Response received` or absence thereof).

### Step 6: Assess exposure

| Crash timing | Implication |
|-------------|-------------|
| Crash before upstream send | LLM never received the request. No ciphertext in LLM context. No exposure. |
| Crash after upstream send, before response | LLM received encrypted tokens. Its response — if any — was never seen by the proxy or agent. |
| Crash after response received, before decryption | LLM response with encrypted tokens was forwarded to the agent undecrypted. Agent saw ciphertext tokens. |
| Crash after SSE `[DONE]`, before journal write | Completion entry was not written but response was processed. This appears as incomplete but is a false positive — no exposure. |

Use the crash buffer log lines for the `request_id` to determine which stage was reached.

### Step 7: Clear artifacts and restart

After investigation:

```bash
# The panic marker is deleted automatically on startup.
# To manually clear it:
rm -f ~/.openobscure/.crashed

# The journal ring is never cleared — it accumulates across restarts.
# To reset it (discards all history):
rm -f ~/.openobscure/request_journal.buf
# The proxy recreates it on next startup.

# The crash buffer is never cleared on startup.
# To reset it:
rm -f ~/.openobscure/crash.buf
# The proxy recreates it on next startup (if crash_buffer = true).
```

Restart the proxy and confirm startup succeeds without incomplete-entry warnings:

```bash
openobscure-proxy serve 2>&1 | grep -i "incomplete\|crash\|recovered"
```

No output from this grep means the journal is clean.

---

## Summary of File Locations

| File | Default path |
|------|-------------|
| Panic marker | `~/.openobscure/.crashed` |
| Request journal | `~/.openobscure/request_journal.buf` |
| Crash buffer | `~/.openobscure/crash.buf` |
| Main log file | Set via `[logging].file_path` in `config/openobscure.toml` |

On Windows, `~` resolves from `%USERPROFILE%`. On Linux/macOS it resolves from `$HOME`.
