//! Saved-tab records: one append-only `.ctxlog` file per tab.
//!
//! Each record is a little-endian `u32` length followed by a `zstd`-compressed `postcard` body.
//! A crash mid-write leaves a truncated last record; readers stop at the last whole record and
//! [`TabLog::open`] truncates the file back to it.

use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

/// Prefix on an output line that is an event line drawn by ctx (never produced by a program:
/// terminal cells can't hold control characters).
pub const EVENT_MARK: char = '\u{1}';
/// Separates an event line's JSON form from its text: `EVENT_MARK json EVENT_SEP text`.
pub const EVENT_SEP: char = '\u{2}';

/// A stored event line: marker, JSON form, separator, text.
pub fn event_line(json: &str, text: &str) -> String {
    format!("{EVENT_MARK}{json}{EVENT_SEP}{text}")
}

/// (JSON form, text) of a stored event line; `None` for ordinary output.
pub fn split_event_line(l: &str) -> Option<(&str, &str)> {
    let rest = l.strip_prefix(EVENT_MARK)?;
    Some(match rest.split_once(EVENT_SEP) {
        Some((json, text)) => (json, text),
        None => ("", rest),
    })
}

/// Something that happened *to* a tab, shown as a dimmed `[…]` line.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Event {
    Opened { by: String },
    Reopened { reason: Option<String>, last_active_ms: i64 },
    Renamed { from: Option<String>, to: String },
    Moved { window: u64, window_name: Option<String> },
    SshStarted { target: String },
    SshEnded { target: String, how: String },
    ScreenCleared,
    Closed,
}

impl Event {
    /// The snake_case name used in JSON (`{"event": "reopened", …}`).
    pub fn word(&self) -> &'static str {
        match self {
            Event::Opened { .. } => "opened",
            Event::Reopened { .. } => "reopened",
            Event::Renamed { .. } => "renamed",
            Event::Moved { .. } => "moved",
            Event::SshStarted { .. } => "ssh_started",
            Event::SshEnded { .. } => "ssh_ended",
            Event::ScreenCleared => "screen_cleared",
            Event::Closed => "closed",
        }
    }

    /// The JSON form shown in `as json` output, e.g. `{"event":"reopened","at":"…","reason":"reboot"}`.
    pub fn json(&self, at_ms: i64) -> String {
        use ctx_protocol::time::iso_utc;
        let mut m = serde_json::Map::new();
        m.insert("event".into(), self.word().into());
        m.insert("at".into(), iso_utc(at_ms).into());
        match self {
            Event::Opened { by } => {
                m.insert("by".into(), by.clone().into());
            }
            Event::Reopened { reason, last_active_ms } => {
                m.insert(
                    "reason".into(),
                    reason.clone().map(serde_json::Value::from).unwrap_or(serde_json::Value::Null),
                );
                m.insert("last_active".into(), iso_utc(*last_active_ms).into());
            }
            Event::Renamed { from, to } => {
                m.insert("from".into(), from.clone().map(serde_json::Value::from).unwrap_or(serde_json::Value::Null));
                m.insert("to".into(), to.clone().into());
            }
            Event::Moved { window, window_name } => {
                m.insert("window".into(), (*window).into());
                m.insert(
                    "window_name".into(),
                    window_name.clone().map(serde_json::Value::from).unwrap_or(serde_json::Value::Null),
                );
            }
            Event::SshStarted { target } => {
                m.insert("target".into(), target.clone().into());
            }
            Event::SshEnded { target, how } => {
                m.insert("target".into(), target.clone().into());
                m.insert("how".into(), how.clone().into());
            }
            Event::ScreenCleared | Event::Closed => {}
        }
        serde_json::Value::Object(m).to_string()
    }

    /// The bracketed line, e.g. `[renamed "auth" → "Claude - auth refactor"]`.
    /// `tz` is the local offset east of UTC in seconds.
    pub fn line(&self, at_ms: i64, tz: i64) -> String {
        use ctx_protocol::time::{local_clock, local_minutes};
        let body = match self {
            Event::Opened { by } => format!("opened {} by {by}", local_minutes(at_ms, tz)),
            Event::Reopened { reason, last_active_ms } => {
                let mut s = format!("reopened {}", local_minutes(at_ms, tz));
                match reason.as_deref() {
                    Some("server restart") => s.push_str(" after server restart"),
                    Some(r) => {
                        s.push_str(&format!(" after {r} · last active {}", local_minutes(*last_active_ms, tz)));
                    }
                    None => s.push_str(&format!(" · last active {}", local_minutes(*last_active_ms, tz))),
                }
                s
            }
            Event::Renamed { from: Some(from), to } => format!("renamed \"{from}\" → \"{to}\""),
            Event::Renamed { from: None, to } => format!("named \"{to}\""),
            Event::Moved { window, window_name: Some(n) } => format!("moved to window {window} \"{n}\""),
            Event::Moved { window, window_name: None } => format!("moved to window {window}"),
            Event::SshStarted { target } => format!("ssh to {target} started {}", local_clock(at_ms, tz)),
            Event::SshEnded { target, how } => format!("ssh to {target} ended {} · {how}", local_clock(at_ms, tz)),
            Event::ScreenCleared => "program cleared the screen — earlier output is kept in the saved record".into(),
            Event::Closed => format!("closed {}", local_minutes(at_ms, tz)),
        };
        format!("[{body}]")
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Record {
    /// First record of every tab, and of every reopening.
    Opened {
        number: u64,
        at_ms: i64,
        window: u64,
        name: Option<String>,
        folder: String,
        opened_by: String,
        columns: u16,
        rows: u16,
    },
    Event {
        at_ms: i64,
        event: Event,
    },
    Name {
        at_ms: i64,
        name: Option<String>,
    },
    Window {
        at_ms: i64,
        window: u64,
    },
    Folder {
        at_ms: i64,
        folder: String,
    },
    /// Environment: the full set when the shell starts, then only changes.
    Env {
        at_ms: i64,
        full: bool,
        set: Vec<(String, String)>,
        unset: Vec<String>,
    },
    CommandStart {
        number: u64,
        at_ms: i64,
        text: String,
        folder: String,
        location: String,
        ran_by: String,
        approximate: bool,
    },
    /// Output lines (with SGR styles). `command: None` is output outside any marked command.
    Output {
        command: Option<u64>,
        at_ms: i64,
        lines: Vec<String>,
    },
    CommandEnd {
        number: u64,
        at_ms: i64,
        exit_status: Option<i32>,
        dropped_lines: u64,
    },
    /// The last screen of a full-screen program run by this command.
    FinalScreen {
        number: u64,
        lines: Vec<String>,
    },
    Resize {
        at_ms: i64,
        columns: u16,
        rows: u16,
    },
    /// `reason`: `closed` or `interrupted`.
    Closed {
        at_ms: i64,
        reason: String,
    },
    /// Names of exported variables kept out of the record by `dont_save_env` (never values).
    EnvHidden {
        at_ms: i64,
        names: Vec<String>,
    },
}

pub fn encode_record(r: &Record) -> io::Result<Vec<u8>> {
    let body = postcard::to_stdvec(r).map_err(|e| io::Error::other(e.to_string()))?;
    let compressed = zstd::bulk::compress(&body, 3)?;
    let mut out = Vec::with_capacity(compressed.len() + 4);
    out.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
    out.extend_from_slice(&compressed);
    Ok(out)
}

fn decode_record(compressed: &[u8]) -> io::Result<Record> {
    let body = zstd::stream::decode_all(compressed)?;
    postcard::from_bytes(&body).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))
}

/// Read every whole record from a file. Returns the records and the byte length of the whole
/// records (a truncated tail is ignored).
pub fn read_records(path: &Path) -> io::Result<(Vec<Record>, u64)> {
    let mut f = File::open(path)?;
    let mut data = Vec::new();
    f.read_to_end(&mut data)?;
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos + 4 <= data.len() {
        let len = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
        if pos + 4 + len > data.len() {
            break;
        }
        match decode_record(&data[pos + 4..pos + 4 + len]) {
            Ok(r) => out.push(r),
            Err(_) => break,
        }
        pos += 4 + len;
    }
    Ok((out, pos as u64))
}

/// Stream records one at a time (for `find` over big files), stopping at a truncated tail.
pub fn for_each_record(path: &Path, mut f: impl FnMut(Record) -> bool) -> io::Result<()> {
    let mut file = io::BufReader::new(File::open(path)?);
    loop {
        let mut len = [0u8; 4];
        if file.read_exact(&mut len).is_err() {
            return Ok(());
        }
        let len = u32::from_le_bytes(len) as usize;
        let mut buf = vec![0u8; len];
        if file.read_exact(&mut buf).is_err() {
            return Ok(());
        }
        let Ok(r) = decode_record(&buf) else { return Ok(()) };
        if !f(r) {
            return Ok(());
        }
    }
}

/// An open tab's record file, appended to in batches.
#[derive(Debug)]
pub struct TabLog {
    pub path: PathBuf,
    file: File,
    pending: Vec<u8>,
    written: u64,
}

impl TabLog {
    /// Open (or create) a tab's file for appending, dropping any truncated tail.
    pub fn open(path: &Path) -> io::Result<TabLog> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let valid = if path.exists() { read_records(path)?.1 } else { 0 };
        let file = OpenOptions::new().create(true).truncate(false).read(true).write(true).mode(0o600).open(path)?;
        file.set_len(valid)?;
        let mut file = file;
        file.seek(SeekFrom::End(0))?;
        Ok(TabLog { path: path.to_path_buf(), file, pending: Vec::new(), written: valid })
    }

    /// Queue a record; it reaches the disk on the next [`flush`](Self::flush).
    pub fn push(&mut self, r: &Record) {
        if let Ok(bytes) = encode_record(r) {
            self.pending.extend_from_slice(&bytes);
        }
    }

    /// Write queued records. No fsync: a crash loses at most what was queued.
    pub fn flush(&mut self) -> io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        self.file.write_all(&self.pending)?;
        self.written += self.pending.len() as u64;
        self.pending.clear();
        Ok(())
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Bytes on disk.
    pub fn size(&self) -> u64 {
        self.written
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ctx-store-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("1.ctxlog")
    }

    fn sample() -> Vec<Record> {
        vec![
            Record::Opened {
                number: 1,
                at_ms: 5,
                window: 1,
                name: Some("Dev".into()),
                folder: "/x".into(),
                opened_by: "you".into(),
                columns: 80,
                rows: 24,
            },
            Record::CommandStart {
                number: 1,
                at_ms: 6,
                text: "ls".into(),
                folder: "/x".into(),
                location: "c@m:/x".into(),
                ran_by: "you".into(),
                approximate: false,
            },
            Record::Output { command: Some(1), at_ms: 7, lines: vec!["a".into(), "\u{1b}[31mb\u{1b}[0m".into()] },
            Record::CommandEnd { number: 1, at_ms: 8, exit_status: Some(0), dropped_lines: 0 },
            Record::Event { at_ms: 9, event: Event::Renamed { from: Some("Dev".into()), to: "Dev server".into() } },
        ]
    }

    #[test]
    fn write_read_round_trip() {
        let path = tmp("rt");
        let mut log = TabLog::open(&path).unwrap();
        for r in sample() {
            log.push(&r);
        }
        log.flush().unwrap();
        let (back, _) = read_records(&path).unwrap();
        assert_eq!(back, sample());
        let mut n = 0;
        for_each_record(&path, |_| {
            n += 1;
            true
        })
        .unwrap();
        assert_eq!(n, 5);
    }

    #[test]
    fn truncated_tail_recovers_to_last_whole_record() {
        let path = tmp("trunc");
        let mut log = TabLog::open(&path).unwrap();
        for r in sample() {
            log.push(&r);
        }
        log.flush().unwrap();
        let full = std::fs::metadata(&path).unwrap().len();
        // Simulate a crash mid-write: cut the last record short.
        let f = OpenOptions::new().write(true).open(&path).unwrap();
        f.set_len(full - 3).unwrap();
        let (back, valid) = read_records(&path).unwrap();
        assert_eq!(back.len(), 4);
        // Reopening drops the partial record and appends cleanly after it.
        let mut log = TabLog::open(&path).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().len(), valid);
        log.push(&Record::Closed { at_ms: 10, reason: "closed".into() });
        log.flush().unwrap();
        let (back, _) = read_records(&path).unwrap();
        assert_eq!(back.len(), 5);
        assert!(matches!(back[4], Record::Closed { .. }));
    }

    #[test]
    fn event_lines() {
        let at = 1_791_654_721_000; // 2026-10-10T17:52:01Z
        assert_eq!(
            Event::Renamed { from: Some("auth".into()), to: "Claude - auth refactor".into() }.line(at, 0),
            "[renamed \"auth\" → \"Claude - auth refactor\"]"
        );
        assert_eq!(
            Event::Reopened { reason: Some("reboot".into()), last_active_ms: at - 3_600_000 }.line(at, 0),
            "[reopened 2026-10-10 17:52 after reboot · last active 2026-10-10 16:52]"
        );
        assert_eq!(
            Event::Reopened { reason: Some("server restart".into()), last_active_ms: 0 }.line(at, 0),
            "[reopened 2026-10-10 17:52 after server restart]"
        );
        assert_eq!(
            Event::Moved { window: 2, window_name: Some("Work".into()) }.line(at, 0),
            "[moved to window 2 \"Work\"]"
        );
    }
}

/// `docs/events.md`: every event, its line and its JSON form.
pub fn events_markdown() -> String {
    let at = 1_791_654_721_000; // 2026-10-10T17:52:01Z
    let events = [
        (Event::Opened { by: "claude (pid 5512)".into() }, "A tab was opened (by you or an agent)."),
        (
            Event::Reopened { reason: Some("reboot".into()), last_active_ms: at - 3_600_000 },
            "A closed tab came back after a reboot.",
        ),
        (Event::Reopened { reason: None, last_active_ms: at - 3_600_000 }, "A closed tab was reopened."),
        (
            Event::Reopened { reason: Some("server restart".into()), last_active_ms: at },
            "Open tabs came back after the server restarted.",
        ),
        (Event::Renamed { from: Some("auth".into()), to: "Claude - auth refactor".into() }, "The tab was renamed."),
        (Event::Renamed { from: None, to: "Dev server".into() }, "The tab got its first name."),
        (Event::Moved { window: 2, window_name: Some("Work".into()) }, "The tab moved to another window."),
        (Event::SshStarted { target: "deploy@web-1".into() }, "An interactive ssh session started."),
        (
            Event::SshEnded { target: "deploy@web-1".into(), how: "connection lost".into() },
            "It ended (`closed` or `connection lost`).",
        ),
        (Event::ScreenCleared, "A program tried to erase the scrollback; ctx kept it."),
        (Event::Closed, "The tab was closed."),
    ];
    let mut out = String::from(
        "# ctx events\n\n_Generated from `ctx-store/src/record.rs`. Don't edit by hand._\n\n\
Things that happen *to* a tab, as opposed to output from a program, are drawn as one dimmed line in\n\
square brackets between output lines, included in `ctx read` and `ctx run` output, and kept in the saved\n\
record as events (never as text, so a program can't fake one). The set is fixed:\n\n\
| Event | Line | JSON |\n|---|---|---|\n",
    );
    for (e, what) in events {
        let json = e.json(at);
        out.push_str(&format!("| {what} | `{}` | `{json}` |\n", e.line(at, 0)));
    }
    out.push_str("\nTimes in lines are local; the examples use UTC.\n");
    out
}

#[cfg(test)]
mod doc_tests {
    #[test]
    fn events_doc_is_current() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../docs/events.md");
        let want = super::events_markdown();
        if std::fs::read_to_string(path).unwrap_or_default() != want {
            std::fs::write(path, want).unwrap();
            panic!("docs/events.md was out of date and has been regenerated; commit it");
        }
    }
}
