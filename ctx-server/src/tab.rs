//! One tab: its pseudo-terminal, emulator, scrollback, shell-integration state, the commands
//! it ran and its saved record.

use crate::cold::ColdStore;
use crate::scan::{Mark, Piece, Scanner};
use crate::terminal::{HarvestedRow, Pin, TermEvent, Terminal};
use ctx_protocol::config::Config;
use ctx_store::index::{CommandRow, Index};
use ctx_store::record::{EVENT_MARK, Event, Record, TabLog};
use std::collections::{BTreeMap, VecDeque};
use std::os::fd::{AsRawFd, OwnedFd};

/// Lines of a running command kept in memory for `run`'s answer (head and tail each).
pub const KEEP_LINES: usize = 5000;

/// A command between its start (OSC 133;C) and end (OSC 133;D) marks.
#[derive(Debug)]
pub struct Running {
    pub number: u64,
    pub text: String,
    pub started_ms: i64,
    pub ran_by: String,
    pub location: String,
    pub folder: String,
    pub start_pin: Option<Pin>,
    /// First lines of output (up to KEEP_LINES).
    pub head: Vec<String>,
    /// Last lines of output (up to KEEP_LINES), when there are more than fit in `head`.
    pub tail: VecDeque<String>,
    pub total_lines: u64,
    /// Lines waiting to be written to the record.
    pub unflushed: Vec<String>,
    /// Bytes of output saved so far (the head part of the per-command cap).
    pub saved_bytes: u64,
    /// Past the head cap: the newest output, kept within the tail half of the cap.
    pub saved_tail: VecDeque<String>,
    pub saved_tail_bytes: u64,
    pub dropped_lines: u64,
    pub full_screen: bool,
    pub final_screen: Option<Vec<String>>,
    /// A soft-wrapped line still being joined.
    pub partial: Option<String>,
    pub approximate: bool,
    pub finished_ms: Option<i64>,
    pub exit_status: Option<i32>,
}

impl Running {
    pub fn total_time(&self) -> f64 {
        (self.finished_ms.unwrap_or_else(ctx_protocol::time::now_ms) - self.started_ms) as f64 / 1000.0
    }

    fn push_line(&mut self, line: String, cap_bytes: u64) {
        self.total_lines += 1;
        if self.head.len() < KEEP_LINES {
            self.head.push(line.clone());
        } else {
            self.tail.push_back(line.clone());
            if self.tail.len() > KEEP_LINES {
                self.tail.pop_front();
            }
        }
        if line.starts_with(EVENT_MARK) {
            return;
        }
        let half = cap_bytes / 2;
        if self.saved_bytes + line.len() as u64 <= half && self.saved_tail.is_empty() {
            self.saved_bytes += line.len() as u64 + 1;
            self.unflushed.push(line);
        } else {
            self.saved_tail_bytes += line.len() as u64 + 1;
            self.saved_tail.push_back(line);
            while self.saved_tail_bytes > half
                && let Some(old) = self.saved_tail.pop_front()
            {
                self.saved_tail_bytes -= old.len() as u64 + 1;
                self.dropped_lines += 1;
            }
        }
    }

    /// Output kept in memory, in order (head, then tail), and whether lines in between were
    /// not kept.
    pub fn kept_output(&self) -> (Vec<String>, bool) {
        let mut out = self.head.clone();
        let gap = self.total_lines > (self.head.len() + self.tail.len()) as u64;
        out.extend(self.tail.iter().cloned());
        (out, gap)
    }
}

/// What processing output changed, for the server to act on.
#[derive(Debug, Default)]
pub struct Effects {
    pub replies: Vec<u8>,
    pub started: Vec<u64>,
    /// (command number, exit status)
    pub finished: Vec<(u64, Option<i32>)>,
    /// The prompt came back without a command having started (a syntax error, an empty line).
    pub prompt_without_command: Option<Option<i32>>,
    pub prompt: bool,
    pub title_changed: bool,
    pub bell: bool,
    pub clipboard: Option<String>,
    pub new_lines: u64,
    pub folder_changed: bool,
    pub typing_changed: bool,
    pub hello: bool,
    pub output: bool,
}

#[derive(Debug)]
pub struct Tab {
    pub number: u64,
    pub name: Option<String>,
    pub window: u64,
    pub folder: String,
    pub host: String,
    /// `user@host`
    pub location: String,
    pub opened_by: String,
    pub opened_ms: i64,
    pub master: OwnedFd,
    pub launch_pid: i32,
    pub term: Box<dyn Terminal>,
    pub scanner: Scanner,
    pub cold: ColdStore,
    pub log: TabLog,
    /// Bytes waiting to be written to the pseudo-terminal.
    pub to_pty: Vec<u8>,
    /// Whether ctx's shell integration is loaded in this shell.
    pub integration_expected: bool,
    pub integration: bool,
    /// A shell prompt is in front, waiting for a command.
    pub at_prompt: bool,
    pub prompt_pin: Option<Pin>,
    /// Someone has typed at the prompt without pressing Enter.
    pub typing: bool,
    /// Command text reported just before its start mark.
    pub pending_text: Option<String>,
    pub running: Vec<Running>,
    /// The last few finished commands, kept for `run`'s answer.
    pub finished: VecDeque<Running>,
    pub next_command: u64,
    pub last_exit: Option<i32>,
    pub last_finished: Option<u64>,
    /// Who typed into the current prompt line (`you`, `claude (pid 5512)`).
    pub contributors: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub env_reported: bool,
    pub title: Option<String>,
    pub last_output_ms: i64,
    pub unread_lines: u64,
    /// Event lines waiting for a place between output lines.
    pub event_queue: Vec<String>,
    /// Viewers (CTX.app subscriptions and attached terminals).
    pub viewers: usize,
    /// Output lines outside any command, waiting for the record (only without integration).
    pub unmarked: Vec<String>,
    pub unmarked_partial: Option<String>,
    pub last_flush_ms: i64,
    pub columns: u16,
    pub rows: u16,
    /// ssh target while an ssh command is running in front.
    pub ssh_target: Option<String>,
    /// Bumped on every chunk of output.
    pub output_seq: u64,
    pub ended: bool,
    pub shell_name: String,
    pub created_ms: i64,
    pub render: Option<crate::screen::Renderer>,
}

pub struct Ctx<'a> {
    pub index: &'a Index,
    pub config: &'a Config,
    pub tz: i64,
    pub now_ms: i64,
}

impl Tab {
    pub fn fd(&self) -> i32 {
        self.master.as_raw_fd()
    }

    pub fn busy(&self) -> bool {
        !self.running.is_empty() || (!self.at_prompt && self.integration)
    }

    /// The command in front, if any (innermost).
    pub fn current(&self) -> Option<&Running> {
        self.running.last()
    }

    /// Process output from the program.
    pub fn process_output(&mut self, data: &[u8], cx: &Ctx) -> Effects {
        let mut fx = Effects { output: true, ..Default::default() };
        self.last_output_ms = cx.now_ms;
        self.output_seq += 1;
        self.scanner.allow_clear_scrollback = cx.config.allow_programs_to_clear_scrollback;
        let newlines = data.iter().filter(|b| **b == b'\n').count() as u64;
        if self.viewers == 0 {
            self.unread_lines += newlines;
        }
        fx.new_lines = newlines;
        // Feed in bounded chunks so harvesting keeps up with pruning.
        for piece in self.scanner.feed(data) {
            match piece {
                Piece::Data(d) => {
                    for chunk in d.chunks(4096) {
                        self.term.write(chunk);
                        self.after_write(cx, &mut fx);
                    }
                }
                Piece::Mark(m) => self.handle_mark(m, cx, &mut fx),
            }
        }
        self.place_queued_events(cx);
        fx
    }

    fn after_write(&mut self, cx: &Ctx, fx: &mut Effects) {
        for e in self.term.take_events() {
            match e {
                TermEvent::Reply(b) => fx.replies.extend(b),
                TermEvent::Title(t) => {
                    // Programs can't make a tab look like another tab by its number.
                    let t = t.trim().to_string();
                    let looks_like_tab =
                        t.to_lowercase().starts_with("tab ") && t[4..].trim().chars().all(|c| c.is_ascii_digit());
                    self.title = if t.is_empty() || looks_like_tab { None } else { Some(t) };
                    fx.title_changed = true;
                }
                TermEvent::Bell => fx.bell = true,
                TermEvent::Clipboard(c) => fx.clipboard = Some(c),
            }
        }
        let rows = self.term.harvest();
        for r in rows {
            self.route_row(r, cx);
        }
    }

    /// A row left the hot region: into the cold store, and into the current command's output
    /// (or the unmarked output) for the record.
    fn route_row(&mut self, row: HarvestedRow, cx: &Ctx) {
        self.cold.push_row(&row);
        let cap = (cx.config.saved_output_mb_per_command * 1024.0 * 1024.0) as u64;
        let term = &self.term;
        if let Some(cmd) = self.running.last_mut() {
            let start = cmd.start_pin.as_ref().and_then(|p| term.pin_row(p));
            let inside = match start {
                Some(s) => row.y >= s,
                None => true,
            };
            if inside {
                let mut text = cmd.partial.take().unwrap_or_default();
                text.push_str(&row.text);
                if row.wraps {
                    cmd.partial = Some(text);
                } else {
                    let line = if row.event { format!("{EVENT_MARK}{text}") } else { text };
                    cmd.push_line(line, cap);
                }
                return;
            }
        }
        if !self.integration && !row.event {
            let mut text = self.unmarked_partial.take().unwrap_or_default();
            text.push_str(&row.text);
            if row.wraps {
                self.unmarked_partial = Some(text);
            } else {
                self.unmarked.push(text);
            }
        }
    }

    fn handle_mark(&mut self, m: Mark, cx: &Ctx, fx: &mut Effects) {
        match m {
            Mark::PromptStart => {
                self.integration = true;
                // The shell is back: any mode a program left on is put back.
                if self.running.iter().all(|r| r.text.is_empty()) || self.running.is_empty() {
                    self.term.reset_program_modes();
                }
                self.at_prompt = true;
                self.prompt_pin = self.term.pin_cursor_row();
                self.contributors.clear();
                self.typing = false;
                fx.prompt = true;
            }
            Mark::InputStart => {}
            Mark::OutputStart => {
                self.integration = true;
                self.start_command(cx, fx);
            }
            Mark::CommandEnd(status) => {
                if self.running.is_empty() {
                    if self.at_prompt || self.integration {
                        fx.prompt_without_command = Some(status);
                    }
                    self.last_exit = status.or(self.last_exit);
                } else {
                    self.finish_command(status, cx, fx);
                }
            }
            Mark::Folder { host, path } => {
                if path != self.folder {
                    self.folder = path.clone();
                    self.log.push(&Record::Folder { at_ms: cx.now_ms, folder: path.clone() });
                    let _ = cx.index.set_folder(self.number, &path, cx.now_ms);
                    fx.folder_changed = true;
                }
                self.host = host;
            }
            Mark::Ctx(payload) => self.handle_private(&payload, cx, fx),
            Mark::ClearScrollback => {
                if !cx.config.allow_programs_to_clear_scrollback {
                    self.add_event(Event::ScreenCleared, cx);
                }
            }
            Mark::EnteredFullScreen => {
                if let Some(r) = self.running.last_mut() {
                    r.full_screen = true;
                }
            }
            Mark::LeavingFullScreen => {
                let screen = self.term.screen(true);
                let mut lines: Vec<String> = screen;
                while lines.last().is_some_and(|l| ctx_store::ansi::strip(l).trim().is_empty()) {
                    lines.pop();
                }
                if let Some(r) = self.running.last_mut() {
                    r.final_screen = Some(lines);
                }
            }
        }
    }

    fn handle_private(&mut self, payload: &str, cx: &Ctx, fx: &mut Effects) {
        let (kind, rest) = payload.split_once(';').unwrap_or((payload, ""));
        match kind {
            "cmd" => self.pending_text = Some(rest.to_string()),
            "where" => self.location = rest.to_string(),
            "typing" => {
                self.typing = rest == "1";
                fx.typing_changed = true;
            }
            "hello" => {
                self.integration = true;
                fx.hello = true;
            }
            "env" => {
                let (mode, body) = rest.split_once(';').unwrap_or((rest, ""));
                let full = mode == "full";
                let mut set = Vec::new();
                let mut unset = Vec::new();
                for entry in body.split('\x1e').filter(|e| !e.is_empty()) {
                    let mut parts = entry.splitn(3, '\x1f');
                    match (parts.next(), parts.next(), parts.next()) {
                        (Some("S"), Some(k), Some(v)) => {
                            if !cx.config.env_excluded(k) {
                                set.push((k.to_string(), v.to_string()));
                            }
                        }
                        (Some("U"), Some(k), _) => unset.push(k.to_string()),
                        _ => {}
                    }
                }
                if full {
                    self.env.clear();
                }
                for (k, v) in &set {
                    self.env.insert(k.clone(), v.clone());
                }
                for k in &unset {
                    self.env.remove(k);
                }
                if full || !set.is_empty() || !unset.is_empty() {
                    self.log.push(&Record::Env { at_ms: cx.now_ms, full, set, unset });
                }
                self.env_reported = true;
            }
            "ssh" => {
                // ssh;start;target or ssh;end;target;how
                let mut parts = rest.splitn(3, ';');
                match (parts.next(), parts.next()) {
                    (Some("start"), Some(target)) => {
                        self.ssh_target = Some(target.to_string());
                        self.add_event(Event::SshStarted { target: target.to_string() }, cx);
                    }
                    (Some("end"), Some(target)) => {
                        let how = parts.next().unwrap_or("closed").to_string();
                        self.ssh_target = None;
                        self.add_event(Event::SshEnded { target: target.to_string(), how }, cx);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn start_command(&mut self, cx: &Ctx, fx: &mut Effects) {
        let number = self.next_command;
        self.next_command += 1;
        let text = self.pending_text.take().unwrap_or_default().trim_end_matches('\n').to_string();
        let mut by: Vec<String> = Vec::new();
        for c in &self.contributors {
            if !by.contains(c) {
                by.push(c.clone());
            }
        }
        if by.is_empty() {
            by.push("you".into());
        }
        let ran_by = by.join(" and ");
        let location = format!(
            "{}:{}",
            if self.location.is_empty() { "?".to_string() } else { self.location.clone() },
            tilde(&self.folder, self.env.get("HOME").map(String::as_str))
        );
        let pin = self.term.pin_cursor_row();
        self.log.push(&Record::CommandStart {
            number,
            at_ms: cx.now_ms,
            text: text.clone(),
            folder: self.folder.clone(),
            location: location.clone(),
            ran_by: ran_by.clone(),
            approximate: false,
        });
        let _ = cx.index.add_command(&CommandRow {
            tab: self.number,
            number,
            text: text.clone(),
            folder: self.folder.clone(),
            location: location.clone(),
            ran_by: ran_by.clone(),
            exit_status: None,
            started_ms: cx.now_ms,
            finished_ms: None,
            approximate: false,
        });
        self.running.push(Running {
            number,
            text,
            started_ms: cx.now_ms,
            ran_by,
            location,
            folder: self.folder.clone(),
            start_pin: pin,
            head: Vec::new(),
            tail: VecDeque::new(),
            total_lines: 0,
            unflushed: Vec::new(),
            saved_bytes: 0,
            saved_tail: VecDeque::new(),
            saved_tail_bytes: 0,
            dropped_lines: 0,
            full_screen: false,
            final_screen: None,
            partial: None,
            approximate: false,
            finished_ms: None,
            exit_status: None,
        });
        // On disk at once: if the server dies, a reopened tab can still offer this command.
        let _ = self.log.flush();
        self.at_prompt = false;
        self.typing = false;
        fx.started.push(number);
    }

    /// Rows of the current command still in the emulator (not yet harvested), up to the cursor.
    pub fn live_rows(&self, styled: bool) -> Vec<String> {
        let Some(cmd) = self.running.last() else { return Vec::new() };
        let start = cmd.start_pin.as_ref().and_then(|p| self.term.pin_row(p)).unwrap_or(0);
        let from = start.max(self.term.harvest_position());
        let (cx, cy) = self.term.cursor();
        let cursor_row = self.term.active_top() + cy as u64;
        let to = if cx > 0 { cursor_row + 1 } else { cursor_row };
        let mut out = Vec::new();
        let mut partial: Option<String> = if from == self.term.harvest_position() { cmd.partial.clone() } else { None };
        for y in from..to {
            let mut text = partial.take().unwrap_or_default();
            text.push_str(&self.term.row(y, styled));
            if self.term.row_wraps(y) && y + 1 < to {
                partial = Some(text);
            } else {
                out.push(text);
            }
        }
        if let Some(p) = partial {
            out.push(p);
        }
        out
    }

    fn finish_command(&mut self, status: Option<i32>, cx: &Ctx, fx: &mut Effects) {
        let rest = self.live_rows(true);
        let cap = (cx.config.saved_output_mb_per_command * 1024.0 * 1024.0) as u64;
        let Some(mut cmd) = self.running.pop() else { return };
        if let Some(p) = cmd.partial.take() {
            cmd.push_line(p, cap);
        }
        for l in rest {
            cmd.push_line(l, cap);
        }
        // Trailing blank lines (the prompt's own spacing) aren't output.
        while cmd.head.last().is_some_and(|l| ctx_store::ansi::strip(l).trim().is_empty()) && cmd.tail.is_empty() {
            cmd.head.pop();
            cmd.total_lines -= 1;
            if cmd.saved_tail.is_empty() {
                cmd.unflushed.pop();
            }
        }
        self.flush_command_output(&mut cmd, cx.now_ms);
        if !cmd.saved_tail.is_empty() {
            let lines: Vec<String> = cmd.saved_tail.drain(..).collect();
            self.log.push(&Record::Output { command: Some(cmd.number), at_ms: cx.now_ms, lines });
        }
        if let Some(screen) = &cmd.final_screen {
            self.log.push(&Record::FinalScreen { number: cmd.number, lines: screen.clone() });
        }
        self.log.push(&Record::CommandEnd {
            number: cmd.number,
            at_ms: cx.now_ms,
            exit_status: status,
            dropped_lines: cmd.dropped_lines,
        });
        let _ = cx.index.finish_command(self.number, cmd.number, status, cx.now_ms);
        let _ = self.log.flush();
        self.last_flush_ms = cx.now_ms;
        self.last_exit = status;
        self.last_finished = Some(cmd.number);
        cmd.finished_ms = Some(cx.now_ms);
        cmd.exit_status = status;
        fx.finished.push((cmd.number, status));
        self.finished.push_back(cmd);
        while self.finished.len() > 4 {
            self.finished.pop_front();
        }
    }

    fn flush_command_output(&mut self, cmd: &mut Running, now_ms: i64) {
        if !cmd.unflushed.is_empty() {
            let lines = std::mem::take(&mut cmd.unflushed);
            self.log.push(&Record::Output { command: Some(cmd.number), at_ms: now_ms, lines });
        }
    }

    /// Write what's waiting to the record (every 30 seconds while output keeps coming).
    pub fn flush(&mut self, now_ms: i64) {
        let mut running = std::mem::take(&mut self.running);
        for cmd in running.iter_mut() {
            self.flush_command_output(cmd, now_ms);
        }
        self.running = running;
        if !self.unmarked.is_empty() {
            let lines = std::mem::take(&mut self.unmarked);
            self.log.push(&Record::Output { command: None, at_ms: now_ms, lines });
        }
        let _ = self.log.flush();
        self.last_flush_ms = now_ms;
    }

    /// Record an event and show it as a dimmed line between output lines.
    pub fn add_event(&mut self, event: Event, cx: &Ctx) {
        self.log.push(&Record::Event { at_ms: cx.now_ms, event: event.clone() });
        self.event_queue.push(event.line(cx.now_ms, cx.tz));
        self.place_queued_events(cx);
    }

    /// Draw queued event lines if there's a place for them now: above the prompt, or at the
    /// start of an empty line while a (non-full-screen) command runs.
    pub fn place_queued_events(&mut self, _cx: &Ctx) {
        if self.event_queue.is_empty() || self.term.full_screen() {
            return;
        }
        let prompt_top =
            if self.at_prompt { self.prompt_pin.as_ref().and_then(|p| self.term.pin_row(p)) } else { None };
        let (cx, _) = self.term.cursor();
        if prompt_top.is_none() && cx != 0 && self.integration {
            return;
        }
        let mut prompt_top = prompt_top;
        for line in std::mem::take(&mut self.event_queue) {
            prompt_top = self.term.insert_event_line(&line, prompt_top);
        }
        // Row anchors don't follow content moved inside a scroll region: pin the prompt again.
        if let Some(y) = prompt_top {
            self.prompt_pin = self.term.pin_screen_row(y);
        }
    }

    /// The plain or styled text of the last `n` lines (cold store + emulator), newest last,
    /// with trailing blank rows below the cursor removed.
    pub fn last_lines(&self, n: usize, styled: bool) -> (Vec<String>, u64) {
        let screen_rows = self.term.history_rows() + self.term.size().1 as u64;
        let (_, cy) = self.term.cursor();
        let last_used = self.term.active_top() + cy as u64;
        let mut emu: Vec<String> = Vec::new();
        let start = self.term.harvest_position();
        let mut partial: Option<String> = None;
        for y in start..screen_rows.min(last_used + 1) {
            let mut text = partial.take().unwrap_or_default();
            text.push_str(&self.term.row(y, true));
            if self.term.row_wraps(y) {
                partial = Some(text);
            } else {
                emu.push(text);
            }
        }
        if let Some(p) = partial {
            emu.push(p);
        }
        while emu.last().is_some_and(|l| ctx_store::ansi::strip(l).trim().is_empty()) {
            emu.pop();
        }
        let total = (self.cold.len() + emu.len()) as u64 + self.cold.dropped_lines;
        let need_cold = n.saturating_sub(emu.len());
        let mut out = if need_cold > 0 { self.cold.last(need_cold) } else { Vec::new() };
        let skip = emu.len().saturating_sub(n);
        out.extend(emu.into_iter().skip(skip));
        let out = out.into_iter().map(|l| if styled { l } else { plain_line(&l) }).collect();
        (out, total)
    }

    /// Recent output as plain text: the running command's latest output lines, or (with no
    /// command running) the last lines of the tab. Never the command line someone typed.
    pub fn recent_output(&self, n: usize) -> Vec<String> {
        match self.running.last() {
            Some(cmd) => {
                let mut lines: Vec<String> = Vec::new();
                let kept: Vec<&String> =
                    if cmd.tail.is_empty() { cmd.head.iter().collect() } else { cmd.tail.iter().collect() };
                let skip = kept.len().saturating_sub(n);
                lines.extend(kept.into_iter().skip(skip).map(|l| plain_line(l)));
                lines.extend(self.live_rows(false));
                lines
            }
            None if self.integration => Vec::new(),
            None => self.last_lines(n, false).0,
        }
    }

    /// The visible screen as text, trailing blank rows removed.
    pub fn screen_text(&self, styled: bool) -> Vec<String> {
        let mut rows = self.term.screen(styled);
        while rows.last().is_some_and(|l| ctx_store::ansi::strip(l).trim().is_empty()) {
            rows.pop();
        }
        rows
    }

    pub fn resize(&mut self, columns: u16, rows: u16, now_ms: i64) {
        if columns == self.columns && rows == self.rows {
            return;
        }
        self.columns = columns;
        self.rows = rows;
        self.term.resize(columns, rows);
        crate::sys::set_window_size(self.fd(), columns, rows);
        self.log.push(&Record::Resize { at_ms: now_ms, columns, rows });
    }

    /// The process in front of the terminal and its name.
    pub fn foreground(&self) -> Option<(i32, String)> {
        let pg = crate::sys::foreground_pgid(self.fd())?;
        let info = crate::sys::proc_info(pg)?;
        Some((pg, info.name))
    }

    /// Whether the shell itself is in front (nothing else running).
    pub fn shell_in_front(&self) -> bool {
        match self.foreground() {
            Some((_, name)) => {
                let n = name.trim_start_matches('-');
                n == self.shell_name || ["zsh", "bash", "sh", "fish"].contains(&n)
            }
            None => true,
        }
    }

    /// Exit status of a recently finished command.
    pub fn last_exit_for(&self, number: u64) -> Option<i32> {
        self.finished.iter().find(|r| r.number == number).and_then(|r| r.exit_status)
    }

    pub fn display_name(&self) -> String {
        match &self.name {
            Some(n) => format!("tab {} \"{n}\"", self.number),
            None => format!("tab {}", self.number),
        }
    }
}

/// Replace the home folder with `~`.
pub fn tilde(path: &str, home: Option<&str>) -> String {
    let home = home.map(str::to_string).unwrap_or_else(crate::sys::home);
    if path == home {
        "~".into()
    } else if let Some(rest) = path.strip_prefix(&format!("{home}/")) {
        format!("~/{rest}")
    } else {
        path.to_string()
    }
}

/// A stored line as plain text: styles removed, event marker removed.
pub fn plain_line(l: &str) -> String {
    let l = l.strip_prefix(EVENT_MARK).unwrap_or(l);
    ctx_store::ansi::strip(l).trim_end().to_string()
}

/// A stored line for display: event marker turned into dim styling when styled.
pub fn display_line(l: &str, styled: bool) -> String {
    match l.strip_prefix(EVENT_MARK) {
        Some(ev) if styled => format!("\x1b[2m{}\x1b[0m", ctx_store::ansi::strip(ev)),
        Some(ev) => ctx_store::ansi::strip(ev),
        None if styled => l.to_string(),
        None => ctx_store::ansi::strip(l).trim_end().to_string(),
    }
}
