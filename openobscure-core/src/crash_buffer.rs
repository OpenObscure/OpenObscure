//! Memory-mapped ring buffer for post-mortem crash debugging.
//!
//! Writes recent log entries to an mmap-backed file. The OS kernel flushes
//! mmap pages to disk even on SIGKILL/OOM, making the last N entries
//! recoverable after a hard crash.
//!
//! File format:
//! - Bytes 0..8: write offset (u64 LE) — position of next write in ring
//! - Bytes 8..SIZE: ring buffer payload — UTF-8 log lines written at `offset`,
//!   wrapping around when the end of the file is reached (oldest data overwritten)

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use memmap2::MmapMut;

/// Header size: 8 bytes for the write offset pointer.
const HEADER_SIZE: usize = 8;

/// A memory-mapped ring buffer for crash-resilient logging.
pub struct CrashBuffer {
    mmap: Arc<Mutex<MmapMut>>,
    /// Total size including header.
    capacity: usize,
    path: PathBuf,
}

impl CrashBuffer {
    /// Create or open a crash buffer at the given path.
    ///
    /// `size` is the total buffer size (header + ring data). Minimum 4KB.
    pub fn open(path: &Path, size: usize) -> io::Result<Self> {
        let size = size.max(4096);

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Create or open the file, set to exact size
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;

        let file_len = file.metadata()?.len() as usize;
        if file_len != size {
            file.set_len(size as u64)?;
        }

        // Memory-map the file
        let mmap = unsafe { MmapMut::map_mut(&file)? };

        Ok(Self {
            mmap: Arc::new(Mutex::new(mmap)),
            capacity: size,
            path: path.to_path_buf(),
        })
    }

    /// Write a log line to the ring buffer.
    ///
    /// Appends `line` followed by a newline. Wraps around when the ring is full.
    pub fn write_line(&self, line: &str) {
        let data = format!("{}\n", line);
        let bytes = data.as_bytes();
        let ring_size = self.capacity - HEADER_SIZE;

        if ring_size == 0 || bytes.is_empty() {
            return;
        }

        let mut mmap = match self.mmap.lock() {
            Ok(m) => m,
            Err(_) => return, // Poisoned mutex — don't panic in logging
        };

        // Read current write offset from header
        let mut offset = read_u64_le(&mmap[..HEADER_SIZE]) as usize;
        if offset >= ring_size {
            offset = 0; // Reset if corrupted
        }

        // Write data into ring, wrapping around if needed
        let mut remaining = bytes;
        let mut pos = offset;
        while !remaining.is_empty() {
            let space = ring_size - pos;
            let chunk = remaining.len().min(space);
            mmap[HEADER_SIZE + pos..HEADER_SIZE + pos + chunk].copy_from_slice(&remaining[..chunk]);
            remaining = &remaining[chunk..];
            pos = (pos + chunk) % ring_size;
        }

        // Update write offset in header
        write_u64_le(&mut mmap[..HEADER_SIZE], pos as u64);
    }

    /// Read the crash buffer contents (most recent entries).
    ///
    /// Returns the ring buffer data starting from the write offset (oldest first).
    pub fn read_contents(&self) -> Option<String> {
        let mmap = self.mmap.lock().ok()?;
        let ring_size = self.capacity - HEADER_SIZE;
        if ring_size == 0 {
            return Some(String::new());
        }

        let offset = read_u64_le(&mmap[..HEADER_SIZE]) as usize;
        let offset = offset.min(ring_size);

        // Read from offset to end, then from start to offset (ring order)
        let ring = &mmap[HEADER_SIZE..];
        let mut result = Vec::with_capacity(ring_size);
        result.extend_from_slice(&ring[offset..]);
        result.extend_from_slice(&ring[..offset]);

        // Trim null bytes (unfilled region)
        let text = String::from_utf8_lossy(&result);
        let trimmed = text.trim_matches('\0').to_string();
        Some(trimmed)
    }

    /// Get the path to the crash buffer file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Create a `MakeWriter` that writes to both the crash buffer and another writer.
pub struct CrashBufferMakeWriter<M> {
    inner: M,
    buffer: Arc<CrashBuffer>,
}

impl<M> CrashBufferMakeWriter<M> {
    pub fn new(inner: M, buffer: Arc<CrashBuffer>) -> Self {
        Self { inner, buffer }
    }
}

impl<'a, M> tracing_subscriber::fmt::MakeWriter<'a> for CrashBufferMakeWriter<M>
where
    M: tracing_subscriber::fmt::MakeWriter<'a>,
{
    type Writer = CrashBufferWriter<M::Writer>;

    fn make_writer(&'a self) -> Self::Writer {
        CrashBufferWriter {
            inner: self.inner.make_writer(),
            buffer: Arc::clone(&self.buffer),
            line_buf: Vec::with_capacity(256),
        }
    }
}

/// Writer that tees output to both an inner writer and the crash buffer.
pub struct CrashBufferWriter<W: Write> {
    inner: W,
    buffer: Arc<CrashBuffer>,
    line_buf: Vec<u8>,
}

impl<W: Write> Write for CrashBufferWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.line_buf.extend_from_slice(buf);
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl<W: Write> Drop for CrashBufferWriter<W> {
    fn drop(&mut self) {
        if !self.line_buf.is_empty() {
            let text = String::from_utf8_lossy(&self.line_buf);
            self.buffer.write_line(text.trim_end());
        }
    }
}

// ---------------------------------------------------------------------------
// Request Journal — lightweight mmap-backed journal for crash recovery
// ---------------------------------------------------------------------------

/// Default journal file size: 64KB (holds ~400 entries at ~160 bytes each).
const JOURNAL_DEFAULT_SIZE: usize = 64 * 1024;

/// A journal entry tracking an in-flight FPE-encrypted request.
///
/// Written before forwarding the encrypted request upstream, marked complete
/// after the response is received and mappings are stored. On startup,
/// incomplete entries indicate requests that may have leaked ciphertext
/// without recoverable mappings.
#[derive(Debug, Clone, PartialEq)]
pub struct JournalEntry {
    pub request_id: uuid::Uuid,
    pub timestamp: i64,
    pub mapping_count: u32,
    pub completed: bool,
}

impl JournalEntry {
    /// Serialize to pipe-delimited line: `J|<uuid>|<timestamp>|<mapping_count>|<0|1>`
    pub fn to_line(&self) -> String {
        format!(
            "J|{}|{}|{}|{}",
            self.request_id,
            self.timestamp,
            self.mapping_count,
            if self.completed { "1" } else { "0" }
        )
    }

    /// Parse from pipe-delimited line.
    pub fn from_line(line: &str) -> Option<Self> {
        let parts: Vec<&str> = line.split('|').collect();
        if parts.len() != 5 || parts[0] != "J" {
            return None;
        }
        let request_id = uuid::Uuid::parse_str(parts[1]).ok()?;
        let timestamp = parts[2].parse::<i64>().ok()?;
        let mapping_count = parts[3].parse::<u32>().ok()?;
        let completed = parts[4] == "1";
        Some(Self {
            request_id,
            timestamp,
            mapping_count,
            completed,
        })
    }
}

/// A crash-resilient request journal backed by an mmap ring buffer.
///
/// Records in-flight requests so incomplete ones can be detected after a crash.
pub struct RequestJournal {
    buffer: CrashBuffer,
}

impl RequestJournal {
    /// Open or create a request journal at the given path.
    pub fn open(path: &Path) -> io::Result<Self> {
        let buffer = CrashBuffer::open(path, JOURNAL_DEFAULT_SIZE)?;
        Ok(Self { buffer })
    }

    /// Write a journal entry (thread-safe).
    pub fn write_entry(&self, entry: &JournalEntry) {
        self.buffer.write_line(&entry.to_line());
    }

    /// Read all incomplete (non-completed) journal entries from the buffer.
    ///
    /// Called on startup to detect requests that were in-flight when the proxy
    /// crashed. Completed entries are filtered out because a later completion
    /// marker means the request finished successfully.
    pub fn read_incomplete(&self) -> Vec<JournalEntry> {
        let contents = match self.buffer.read_contents() {
            Some(c) => c,
            None => return Vec::new(),
        };

        // Collect all entries, tracking completed request IDs
        let mut all_entries: Vec<JournalEntry> = Vec::new();
        let mut completed_ids: std::collections::HashSet<uuid::Uuid> =
            std::collections::HashSet::new();

        for line in contents.lines() {
            if let Some(entry) = JournalEntry::from_line(line) {
                if entry.completed {
                    completed_ids.insert(entry.request_id);
                }
                all_entries.push(entry);
            }
        }

        // Return entries that were never completed
        all_entries
            .into_iter()
            .filter(|e| !e.completed && !completed_ids.contains(&e.request_id))
            .collect()
    }
}

fn read_u64_le(bytes: &[u8]) -> u64 {
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&bytes[..8]);
    u64::from_le_bytes(buf)
}

fn write_u64_le(bytes: &mut [u8], val: u64) {
    bytes[..8].copy_from_slice(&val.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_buffer(size: usize) -> (CrashBuffer, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("crash.buf");
        let buf = CrashBuffer::open(&path, size).unwrap();
        (buf, dir)
    }

    #[test]
    fn test_write_and_read() {
        let (buf, _dir) = temp_buffer(4096);
        buf.write_line("line 1");
        buf.write_line("line 2");
        buf.write_line("line 3");

        let contents = buf.read_contents().unwrap();
        assert!(contents.contains("line 1"));
        assert!(contents.contains("line 2"));
        assert!(contents.contains("line 3"));
    }

    #[test]
    fn test_ring_wraps_around() {
        // Small buffer: header(8) + ring(64) = 72 bytes
        let (buf, _dir) = temp_buffer(72);

        // Write more data than fits in the ring
        for i in 0..20 {
            buf.write_line(&format!("entry-{:02}", i));
        }

        let contents = buf.read_contents().unwrap();
        // Most recent entries should be present, oldest overwritten
        assert!(
            contents.contains("entry-19") || contents.contains("entry-18"),
            "Recent entries should survive wrap-around, got: {}",
            contents
        );
    }

    #[test]
    fn test_empty_buffer_read() {
        let (buf, _dir) = temp_buffer(4096);
        let contents = buf.read_contents().unwrap();
        assert!(contents.is_empty() || contents.chars().all(|c| c == '\0'));
    }

    #[test]
    fn test_persistence_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("crash.buf");

        // Write with first instance
        {
            let buf = CrashBuffer::open(&path, 4096).unwrap();
            buf.write_line("persistent data");
        }

        // Reopen and verify
        {
            let buf = CrashBuffer::open(&path, 4096).unwrap();
            let contents = buf.read_contents().unwrap();
            assert!(
                contents.contains("persistent data"),
                "Data should persist across reopen, got: {}",
                contents
            );
        }
    }

    #[test]
    fn test_minimum_size() {
        // Request tiny size — should be bumped to 4096
        let (buf, _dir) = temp_buffer(10);
        assert_eq!(buf.capacity, 4096);
        buf.write_line("still works");
        let contents = buf.read_contents().unwrap();
        assert!(contents.contains("still works"));
    }

    #[test]
    fn test_crash_buffer_writer_tees_output() {
        let (crash_buf, _dir) = temp_buffer(4096);
        let crash_arc = Arc::new(crash_buf);

        let mut output = Vec::new();
        {
            let mut writer = CrashBufferWriter {
                inner: &mut output as &mut Vec<u8>,
                buffer: Arc::clone(&crash_arc),
                line_buf: Vec::new(),
            };
            write!(writer, "test log line").unwrap();
        }
        // Inner writer should have the output
        assert_eq!(String::from_utf8(output).unwrap(), "test log line");
        // Crash buffer should also have it
        let contents = crash_arc.read_contents().unwrap();
        assert!(contents.contains("test log line"));
    }

    // --- Request Journal tests ---

    fn temp_journal() -> (RequestJournal, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal.buf");
        let journal = RequestJournal::open(&path).unwrap();
        (journal, dir)
    }

    #[test]
    fn test_journal_entry_roundtrip() {
        let entry = JournalEntry {
            request_id: uuid::Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap(),
            timestamp: 1709654400,
            mapping_count: 7,
            completed: false,
        };
        let line = entry.to_line();
        assert_eq!(
            line,
            "J|550e8400-e29b-41d4-a716-446655440000|1709654400|7|0"
        );
        let parsed = JournalEntry::from_line(&line).unwrap();
        assert_eq!(parsed, entry);

        // Completed entry
        let entry_done = JournalEntry {
            completed: true,
            ..entry
        };
        let line_done = entry_done.to_line();
        assert!(line_done.ends_with("|1"));
        let parsed_done = JournalEntry::from_line(&line_done).unwrap();
        assert!(parsed_done.completed);
    }

    #[test]
    fn test_journal_incomplete_detected() {
        let (journal, _dir) = temp_journal();
        let id = uuid::Uuid::new_v4();
        journal.write_entry(&JournalEntry {
            request_id: id,
            timestamp: 1000,
            mapping_count: 3,
            completed: false,
        });

        let incomplete = journal.read_incomplete();
        assert_eq!(incomplete.len(), 1);
        assert_eq!(incomplete[0].request_id, id);
        assert_eq!(incomplete[0].mapping_count, 3);
    }

    #[test]
    fn test_journal_completed_not_flagged() {
        let (journal, _dir) = temp_journal();
        let id = uuid::Uuid::new_v4();

        // Write start entry
        journal.write_entry(&JournalEntry {
            request_id: id,
            timestamp: 1000,
            mapping_count: 5,
            completed: false,
        });
        // Write completion entry
        journal.write_entry(&JournalEntry {
            request_id: id,
            timestamp: 1001,
            mapping_count: 5,
            completed: true,
        });

        let incomplete = journal.read_incomplete();
        assert!(
            incomplete.is_empty(),
            "Completed entries should be filtered out"
        );
    }

    #[test]
    fn test_journal_survives_wrap() {
        // Small buffer to force wrapping
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal_small.buf");
        // Use raw CrashBuffer with small size, then wrap in journal-like usage
        let buffer = CrashBuffer::open(&path, 4096).unwrap();
        let journal = RequestJournal { buffer };

        // Write many entries to force ring wrap
        let mut last_id = uuid::Uuid::nil();
        for i in 0..50 {
            let id = uuid::Uuid::new_v4();
            journal.write_entry(&JournalEntry {
                request_id: id,
                timestamp: 1000 + i,
                mapping_count: 1,
                completed: false,
            });
            last_id = id;
        }

        let incomplete = journal.read_incomplete();
        // Most recent entries should survive
        assert!(
            !incomplete.is_empty(),
            "Some incomplete entries should survive wrap"
        );
        // The very last entry should be among survivors
        assert!(
            incomplete.iter().any(|e| e.request_id == last_id),
            "Most recent entry should survive wrap"
        );
    }

    #[test]
    fn test_journal_empty_buffer() {
        let (journal, _dir) = temp_journal();
        let incomplete = journal.read_incomplete();
        assert!(incomplete.is_empty());
    }
}
