use crate::{Factory, Result};
use serde_json::Value;
use std::{collections::VecDeque, io::Write, sync::Arc};
use tokio::io::{AsyncRead, AsyncReadExt};

const MAX_LINE_BYTES: usize = 4 * 1024 * 1024;
const MAX_CAPTURE_BYTES: usize = 64 * 1024 * 1024;
const MAX_CAPTURE_LINES: usize = 8192;
const OVERSIZED_WARNING: &[u8] =
    b"[Factory warning] CLI emitted oversized line (>4 MiB); discarded through newline/EOF; continuing.\n";
const CAPTURE_WARNING: &[u8] = b"[Factory warning] CLI output capture limit (64 MiB/8192 lines) reached; oldest captured lines evicted; session log retained; continuing.\n";

enum Frame {
    Line(Vec<u8>),
    Oversized,
}

struct Lines {
    pending: Vec<u8>,
    discarding: bool,
    limit: usize,
}

impl Lines {
    fn new(limit: usize) -> Self {
        Self {
            pending: Vec::new(),
            discarding: false,
            limit,
        }
    }

    fn push(&mut self, bytes: &[u8]) -> Vec<Frame> {
        let mut frames = Vec::new();
        for part in bytes.split_inclusive(|byte| *byte == b'\n') {
            let newline = part.last() == Some(&b'\n');
            let payload = part.len() - usize::from(newline);
            if !self.discarding {
                if self.pending.len() + payload > self.limit {
                    self.pending.clear();
                    self.discarding = true;
                    frames.push(Frame::Oversized);
                } else {
                    self.pending.extend_from_slice(part);
                    if newline {
                        frames.push(Frame::Line(std::mem::take(&mut self.pending)));
                    }
                }
            }
            if newline {
                self.discarding = false;
            }
        }
        frames
    }

    fn finish(&mut self) -> Option<Vec<u8>> {
        (!self.discarding && !self.pending.is_empty()).then(|| std::mem::take(&mut self.pending))
    }
}

// Capture is only for final-result extraction; the redacted session log remains
// on disk. Evict whole old lines so a clipped JSON suffix cannot become a result.
struct Capture {
    lines: VecDeque<String>,
    bytes: usize,
    limit: usize,
}

impl Capture {
    fn new(limit: usize) -> Self {
        Self {
            lines: VecDeque::new(),
            bytes: 0,
            limit,
        }
    }

    fn push(&mut self, text: String) -> bool {
        let mut evicted = false;
        while self.bytes + text.len() > self.limit || self.lines.len() >= MAX_CAPTURE_LINES {
            let Some(oldest) = self.lines.pop_front() else {
                // A single redacted line can grow beyond the capture budget.
                return true;
            };
            self.bytes -= oldest.len();
            evicted = true;
        }
        self.bytes += text.len();
        self.lines.push_back(text);
        evicted
    }

    fn into_string(self) -> String {
        self.lines.into_iter().collect()
    }
}

pub(super) fn spawn_output<R: AsyncRead + Unpin + Send + 'static>(
    factory: &Arc<Factory>,
    run: &str,
    node: &str,
    mut reader: R,
    log: Arc<std::sync::Mutex<std::fs::File>>,
    capture: bool,
) -> tokio::task::JoinHandle<Result<String>> {
    let f = factory.clone();
    let run = run.to_string();
    let node = node.to_string();
    tokio::spawn(async move {
        let mut captured = Capture::new(MAX_CAPTURE_BYTES);
        // Trailing diagnostics must not evict the final response. Keep the last
        // candidate, including invalid JSON payloads, so validation rejects it without another agent invocation.
        let mut last_candidate = None;
        let mut warned_capture = false;
        let mut record = |line: &[u8]| -> Result<()> {
            let text = f.redact(&String::from_utf8_lossy(line));
            log.lock().unwrap().write_all(text.as_bytes())?;
            if capture {
                if let Ok(v) = serde_json::from_str::<Value>(&text) {
                    if result_text(&v, &text).is_some() {
                        last_candidate = Some(text.clone());
                    }
                    if let Some(session) =
                        v["thread_id"].as_str().or_else(|| v["sessionID"].as_str())
                    {
                        if let Ok(d) = f.store.detail(&run) {
                            if let Some(mut state) = d.nodes.into_iter().find(|n| n.id == node) {
                                state.session_id = Some(session.into());
                                let _ = f.store.save_node(&run, &state);
                            }
                        }
                    }
                }
                if captured.push(text) && !warned_capture {
                    log.lock().unwrap().write_all(CAPTURE_WARNING)?;
                    warned_capture = true;
                }
            }
            f.emit("log", Some(&run), Some(&node));
            Ok(())
        };
        let mut lines = Lines::new(MAX_LINE_BYTES);
        let mut buf = [0u8; 8192];
        loop {
            let n = reader.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            for frame in lines.push(&buf[..n]) {
                match frame {
                    Frame::Line(line) => record(&line)?,
                    Frame::Oversized => record(OVERSIZED_WARNING)?,
                }
            }
        }
        if let Some(line) = lines.finish() {
            record(&line)?;
        }
        Ok(last_candidate.unwrap_or_else(|| captured.into_string()))
    })
}

fn result_text<'a>(value: &'a Value, line: &'a str) -> Option<&'a str> {
    if value.get("status").is_some() || value.get("artifacts").is_some() {
        Some(line)
    } else {
        value["part"]["text"]
            .as_str()
            .or_else(|| value["item"]["text"].as_str())
    }
}

pub(super) fn extract_result(output: &str) -> Option<String> {
    output.lines().rev().find_map(|line| {
        let value: Value = serde_json::from_str(line).ok()?;
        result_text(&value, line).map(str::to_owned)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_lines_resynchronize_for_every_chunk_boundary() {
        let bytes = b"1234\n1234567\nabc\n8901234567";
        for chunk_size in 1..=bytes.len() {
            let mut lines = Lines::new(4);
            let mut kept = Vec::new();
            let mut warnings = 0;
            for chunk in bytes.chunks(chunk_size) {
                for frame in lines.push(chunk) {
                    match frame {
                        Frame::Line(line) => kept.push(line),
                        Frame::Oversized => warnings += 1,
                    }
                }
                assert!(lines.pending.len() <= 4);
            }
            assert!(lines.finish().is_none());
            assert_eq!(warnings, 2, "chunk size {chunk_size}");
            assert_eq!(kept, [b"1234\n".to_vec(), b"abc\n".to_vec()]);
        }
    }

    #[test]
    fn short_unterminated_utf8_line_survives_split_reads() {
        let mut lines = Lines::new(4);
        for byte in "éé".as_bytes() {
            assert!(lines.push(&[*byte]).is_empty());
        }
        assert_eq!(lines.finish().unwrap(), "éé".as_bytes());
    }

    #[test]
    fn capture_evicts_whole_lines_and_keeps_latest_final_response() {
        let mut capture = Capture::new(12);
        assert!(!capture.push("oldest\n".into()));
        assert!(!capture.push("old\n".into()));
        assert!(capture.push("final\n".into()));
        assert_eq!(capture.bytes, 10);
        assert_eq!(capture.into_string(), "old\nfinal\n");
    }

    #[test]
    fn capture_bounds_allocations_for_many_tiny_lines() {
        let mut capture = Capture::new(MAX_CAPTURE_BYTES);
        for _ in 0..10_000 {
            capture.push("\n".into());
        }
        assert!(capture.lines.len() <= 8192);
        assert!(capture.bytes <= 8192);
    }
}
