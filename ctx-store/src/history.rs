//! A tab rebuilt from its records: its commands with their outputs, its environment and a
//! transcript of everything it showed. Used for closed tabs (`read`, `find`, `reopen`) and for
//! the parts of open tabs that are already on disk.

use crate::record::{EVENT_MARK, Event, Record, event_line, for_each_record, read_records};
use std::collections::BTreeMap;
use std::io;
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SavedCommand {
    pub number: u64,
    pub text: String,
    pub folder: String,
    pub location: String,
    pub ran_by: String,
    pub started_ms: i64,
    pub finished_ms: Option<i64>,
    pub exit_status: Option<i32>,
    pub approximate: bool,
    pub full_screen: bool,
    /// Output lines with SGR styles; event lines start with [`EVENT_MARK`].
    pub output: Vec<String>,
    pub dropped_lines: u64,
}

#[derive(Debug, Clone, Default)]
pub struct History {
    pub number: u64,
    pub name: Option<String>,
    pub folder: String,
    pub window: u64,
    pub opened_by: String,
    pub opened_ms: i64,
    pub closed_ms: Option<i64>,
    /// `closed` or `interrupted`, when it ended.
    pub ended: Option<String>,
    pub last_active_ms: i64,
    pub columns: u16,
    pub rows: u16,
    pub env: BTreeMap<String, String>,
    /// Variables kept out of the record by `dont_save_env`: not restored on reopen.
    pub hidden_env: Vec<String>,
    pub commands: Vec<SavedCommand>,
    /// Every line in order: prompts with commands, outputs, unmarked output and event lines.
    pub transcript: Vec<String>,
    /// The command that was running when the tab ended, if any.
    pub unfinished: Option<u64>,
    /// Remote target if the tab ended inside ssh.
    pub ssh_target: Option<String>,
    pub tz: i64,
}

impl History {
    pub fn load(path: &Path, tz: i64) -> io::Result<History> {
        let (records, _) = read_records(path)?;
        let mut h = History { tz, ..Default::default() };
        for r in records {
            h.apply(r);
        }
        Ok(h)
    }

    /// Apply one record.
    pub fn apply(&mut self, r: Record) {
        match r {
            Record::Opened { number, at_ms, window, name, folder, opened_by, columns, rows } => {
                if self.number == 0 {
                    self.opened_ms = at_ms;
                    self.opened_by = opened_by;
                }
                self.number = number;
                self.window = window;
                self.name = name;
                self.folder = folder;
                self.columns = columns;
                self.rows = rows;
                self.closed_ms = None;
                self.ended = None;
                self.last_active_ms = self.last_active_ms.max(at_ms);
            }
            Record::Event { at_ms, event } => {
                match &event {
                    Event::SshStarted { target } => self.ssh_target = Some(target.clone()),
                    Event::SshEnded { .. } => self.ssh_target = None,
                    _ => {}
                }
                let line = event_line(&event.json(at_ms), &event.line(at_ms, self.tz));
                if let Some(c) = self.unfinished.and_then(|n| self.commands.iter_mut().rev().find(|c| c.number == n)) {
                    c.output.push(line.clone());
                }
                self.transcript.push(line);
            }
            Record::Name { name, .. } => self.name = name,
            Record::Window { window, .. } => self.window = window,
            Record::Folder { folder, .. } => self.folder = folder,
            Record::Env { full, set, unset, .. } => {
                if full {
                    self.env.clear();
                }
                for (k, v) in set {
                    self.env.insert(k, v);
                }
                for k in unset {
                    self.env.remove(&k);
                }
            }
            Record::CommandStart { number, at_ms, text, folder, location, ran_by, approximate } => {
                self.transcript.push(format!("{location}$ {text}"));
                self.commands.push(SavedCommand {
                    number,
                    text,
                    folder,
                    location,
                    ran_by,
                    started_ms: at_ms,
                    approximate,
                    ..Default::default()
                });
                self.unfinished = Some(number);
                self.last_active_ms = self.last_active_ms.max(at_ms);
            }
            Record::Output { command, at_ms, lines } => {
                if let Some(n) = command
                    && let Some(c) = self.commands.iter_mut().rev().find(|c| c.number == n)
                {
                    c.output.extend(lines.iter().cloned());
                }
                self.transcript.extend(lines);
                self.last_active_ms = self.last_active_ms.max(at_ms);
            }
            Record::CommandEnd { number, at_ms, exit_status, dropped_lines } => {
                if let Some(c) = self.commands.iter_mut().rev().find(|c| c.number == number) {
                    c.finished_ms = Some(at_ms);
                    c.exit_status = exit_status;
                    c.dropped_lines = dropped_lines;
                }
                if self.unfinished == Some(number) {
                    self.unfinished = self.commands.iter().rev().find(|c| c.finished_ms.is_none()).map(|c| c.number);
                }
                self.last_active_ms = self.last_active_ms.max(at_ms);
            }
            Record::FinalScreen { number, lines } => {
                if let Some(c) = self.commands.iter_mut().rev().find(|c| c.number == number) {
                    c.full_screen = true;
                    c.output.extend(lines.iter().cloned());
                }
                self.transcript.extend(lines);
            }
            Record::Resize { columns, rows, .. } => {
                self.columns = columns;
                self.rows = rows;
            }
            Record::EnvHidden { names, .. } => {
                for n in names {
                    if !self.hidden_env.contains(&n) {
                        self.hidden_env.push(n);
                    }
                }
            }
            Record::Closed { at_ms, reason } => {
                self.closed_ms = Some(at_ms);
                self.ended = Some(reason);
                self.last_active_ms = self.last_active_ms.max(at_ms);
            }
        }
    }

    /// Commands, newest last, by number.
    pub fn command(&self, number: u64) -> Option<&SavedCommand> {
        self.commands.iter().rev().find(|c| c.number == number)
    }

    pub fn next_command_number(&self) -> u64 {
        self.commands.iter().map(|c| c.number).max().unwrap_or(0) + 1
    }
}

/// A line of a tab's record that matched a search, with two lines of context either side.
#[derive(Debug, Clone, PartialEq)]
pub struct LineHit {
    pub command: Option<u64>,
    /// Line number within the command's output (or within the unmarked output), from 1.
    pub line: u64,
    pub text: String,
    pub before: Vec<String>,
    pub after: Vec<String>,
}

/// Search a tab's record for output lines containing `needle_lower` (case-insensitive). `hit`
/// returns false to stop early.
pub fn search_lines(path: &Path, needle_lower: &str, mut hit: impl FnMut(LineHit) -> bool) -> io::Result<()> {
    struct State<'a, F: FnMut(LineHit) -> bool> {
        needle: &'a str,
        counters: BTreeMap<Option<u64>, u64>,
        recent: Vec<String>,
        pending: Vec<LineHit>,
        hit: F,
        stopped: bool,
    }
    impl<F: FnMut(LineHit) -> bool> State<'_, F> {
        fn line(&mut self, command: Option<u64>, plain: String) {
            let n = self.counters.entry(command).or_insert(0);
            *n += 1;
            let n = *n;
            let mut i = 0;
            while i < self.pending.len() {
                self.pending[i].after.push(plain.clone());
                if self.pending[i].after.len() >= 2 {
                    let done = self.pending.remove(i);
                    if !(self.hit)(done) {
                        self.stopped = true;
                    }
                } else {
                    i += 1;
                }
            }
            if plain.to_lowercase().contains(self.needle) {
                self.pending.push(LineHit {
                    command,
                    line: n,
                    text: plain.clone(),
                    before: self.recent.clone(),
                    after: Vec::new(),
                });
            }
            self.recent.push(plain);
            if self.recent.len() > 2 {
                self.recent.remove(0);
            }
        }
        fn new_group(&mut self) {
            self.recent.clear();
        }
    }
    let mut st = State {
        needle: needle_lower,
        counters: BTreeMap::new(),
        recent: Vec::new(),
        pending: Vec::new(),
        hit: &mut hit,
        stopped: false,
    };
    for_each_record(path, |r| {
        match r {
            Record::CommandStart { .. } => st.new_group(),
            Record::Output { command, lines, .. } => {
                for l in lines {
                    if !l.starts_with(EVENT_MARK) {
                        st.line(command, crate::ansi::strip(&l));
                    }
                }
            }
            Record::FinalScreen { number, lines } => {
                for l in lines {
                    st.line(Some(number), crate::ansi::strip(&l));
                }
            }
            _ => {}
        }
        !st.stopped
    })?;
    if !st.stopped {
        for p in std::mem::take(&mut st.pending) {
            if !(st.hit)(p) {
                break;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::TabLog;

    #[test]
    fn rebuilds_commands_and_transcript() {
        let dir = std::env::temp_dir().join(format!("ctx-history-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("3.ctxlog");
        let mut log = TabLog::open(&path).unwrap();
        log.push(&Record::Opened {
            number: 3,
            at_ms: 1,
            window: 1,
            name: None,
            folder: "/a".into(),
            opened_by: "you".into(),
            columns: 80,
            rows: 24,
        });
        log.push(&Record::Env {
            at_ms: 1,
            full: true,
            set: vec![("A".into(), "1".into()), ("B".into(), "2".into())],
            unset: vec![],
        });
        log.push(&Record::CommandStart {
            number: 1,
            at_ms: 2,
            text: "echo hi".into(),
            folder: "/a".into(),
            location: "c@m:/a".into(),
            ran_by: "you".into(),
            approximate: false,
        });
        log.push(&Record::Output { command: Some(1), at_ms: 3, lines: vec!["hi".into()] });
        log.push(&Record::CommandEnd { number: 1, at_ms: 4, exit_status: Some(0), dropped_lines: 0 });
        log.push(&Record::Env { at_ms: 5, full: false, set: vec![("A".into(), "9".into())], unset: vec!["B".into()] });
        log.push(&Record::CommandStart {
            number: 2,
            at_ms: 6,
            text: "sleep 100".into(),
            folder: "/a".into(),
            location: "c@m:/a".into(),
            ran_by: "claude (pid 1)".into(),
            approximate: false,
        });
        log.push(&Record::Closed { at_ms: 7, reason: "interrupted".into() });
        log.flush().unwrap();
        let h = History::load(&path, 0).unwrap();
        assert_eq!(h.commands.len(), 2);
        assert_eq!(h.command(1).unwrap().output, vec!["hi".to_string()]);
        assert_eq!(h.unfinished, Some(2));
        assert_eq!(h.env.get("A").map(String::as_str), Some("9"));
        assert!(!h.env.contains_key("B"));
        assert_eq!(h.transcript[0], "c@m:/a$ echo hi");
        assert_eq!(h.ended.as_deref(), Some("interrupted"));
        assert_eq!(h.next_command_number(), 3);

        let mut hits = Vec::new();
        search_lines(&path, "hi", |h| {
            hits.push((h.command, h.line, h.text));
            true
        })
        .unwrap();
        assert_eq!(hits, vec![(Some(1), 1, "hi".to_string())]);
    }
}
