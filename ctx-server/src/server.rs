//! The server: one single-threaded event loop owning every tab and every client connection.

use crate::activity::{ActivityLog, Identity, LogLine, identify};
use crate::cold::ColdStore;
use crate::scan::Scanner;
use crate::sys;
use crate::tab::{Ctx, Effects, Tab, display_line, plain_line};
use crate::terminal::{Ghostty, Terminal};
use ctx_protocol::config::{Config, Paths};
use ctx_protocol::time::{iso_utc, now_ms};
use ctx_protocol::*;
use ctx_store::history::{History, search_lines};
use ctx_store::index::{Index, Narrow, TabRow};
use ctx_store::record::{EVENT_MARK, Event, Record, TabLog};
use std::collections::{BTreeMap, HashMap};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const LISTENER: usize = 0;
const SIGNALS: usize = 1;
const TAB_KEY_BASE: usize = 1 << 40;
const ZSHENV: &str = include_str!("../../shell/zsh/.zshenv");
const CTX_ZSH: &str = include_str!("../../shell/ctx.zsh");
const ZSH_COMPLETION: &str = include_str!("../../shell/_ctx");
const REMOTE_BOOTSTRAP: &str = include_str!("../../shell/remote/bootstrap.sh");
const REMOTE_BASH: &str = include_str!("../../shell/remote/ctx.bash");
const LOCAL_BASHRC: &str = include_str!("../../shell/ctx.bashrc");

/// The remote command ssh gets for an interactive session: the bootstrap, base64-encoded so no
/// quoting survives the trip, decoded and run by the remote `sh`.
pub fn ssh_command() -> String {
    let script = REMOTE_BOOTSTRAP.replace("@ZSH@", CTX_ZSH).replace("@BASH@", REMOTE_BASH);
    let b64 = base64(script.as_bytes());
    format!("sh -c 'x={b64}; eval \"$(printf %s \"$x\" | base64 -d 2>/dev/null || printf %s \"$x\" | base64 -D)\"'")
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}
/// Raw frame a client sends after a version mismatch to ask for a restart (never changes).
pub const RESTART_MAGIC: &[u8] = b"ctx: restart the server";

#[derive(Debug, Clone)]
pub struct Window {
    pub number: u64,
    pub name: Option<String>,
    pub tabs: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq)]
enum Mode {
    Requests,
    Attached(u64),
    App(Vec<u64>),
}

struct Client {
    stream: UnixStream,
    inbuf: FrameBuffer,
    out: Vec<u8>,
    hello: Option<Hello>,
    identity: Identity,
    mode: Mode,
    /// A reply is pending (run, wait).
    waiting: bool,
    close_after_flush: bool,
    mismatched: bool,
}

#[derive(Debug)]
enum WaitKind {
    Run {
        command: String,
        number: Option<u64>,
        max_lines: usize,
        colors: bool,
        wait: bool,
        sent_at: Instant,
    },
    Idle,
    Shows {
        text: String,
        seen: u64,
    },
    Quiet {
        ms: u64,
    },
    /// A new tab: answer once its shell shows its first prompt.
    Ready {
        opened: Opened,
    },
}

#[derive(Debug)]
struct Waiter {
    client: usize,
    tab: u64,
    kind: WaitKind,
    deadline: Option<Instant>,
    started: Instant,
}

enum Handled {
    Reply(Response),
    Deferred,
}

fn ok(o: Output) -> Handled {
    Handled::Reply(Response::Ok(o))
}

fn err(kind: ErrorKind, msg: impl Into<String>) -> Handled {
    Handled::Reply(Response::Error(ErrorReply::new(kind, msg)))
}

fn done(msg: impl Into<String>) -> Handled {
    ok(Output::Done(Done { message: msg.into() }))
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct SavedState {
    /// Tabs to bring back when the server starts (after `ctx restart server`).
    reopen: Vec<u64>,
    windows: Vec<(u64, Option<String>, Vec<u64>)>,
    /// Tabs that were open when the server last stopped (for "reopen N tabs from last time").
    last_open: Vec<u64>,
}

pub struct Server {
    paths: Paths,
    config: Config,
    config_mtime: Option<std::time::SystemTime>,
    poller: sys::Poller,
    listener: UnixListener,
    signals: OwnedFd,
    tabs: BTreeMap<u64, Tab>,
    windows: Vec<Window>,
    clients: HashMap<usize, Client>,
    next_key: usize,
    index: Index,
    activity: ActivityLog,
    started: Instant,
    tz: i64,
    last_window: Option<u64>,
    waiters: Vec<Waiter>,
    launch_path: PathBuf,
    integration: PathBuf,
    zdotdir: PathBuf,
    quit: bool,
    restart: bool,
    last_maintenance: Instant,
    last_config_check: Instant,
    pending_show: Option<u64>,
    /// Tabs whose screens changed since the last update to CTX.app.
    dirty: std::collections::BTreeSet<u64>,
    last_frame: Instant,
    server_exe: PathBuf,
    /// Requests for tabs still starting, replayed at their first prompt.
    starting: Vec<(usize, Envelope, Instant)>,
    /// When free memory was last handed back.
    relieved_at: i64,
}

/// Run the server until it's told to stop. Returns an error message if it can't start.
pub fn run() -> Result<(), String> {
    // macOS's allocator keeps freed large blocks for reuse; after a flood of output that's tens of
    // megabytes held for nothing. Space-efficient mode hands them back. It's read when the
    // process starts, so set it and start again (once).
    if std::env::var_os("MallocSpaceEfficient").is_none() {
        // SAFETY of set_var: single-threaded at this point, before anything reads the environment.
        unsafe { std::env::set_var("MallocSpaceEfficient", "1") };
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let err = exec_self(&exe);
        return Err(format!("couldn't start the server: {err}"));
    }
    let mut server = Server::start()?;
    server.event_loop();
    server.shutdown();
    if server.restart {
        let exe = server.server_exe.clone();
        drop(server);
        let err = exec_self(&exe);
        return Err(format!("couldn't restart the server: {err}"));
    }
    Ok(())
}

fn exec_self(exe: &std::path::Path) -> io::Error {
    use std::os::unix::process::CommandExt;
    std::process::Command::new(exe).arg("server").exec()
}

impl Server {
    fn start() -> Result<Server, String> {
        let paths = Paths::resolve(sys::user_temp_dir(), sys::uid(), &sys::home());
        for d in [&paths.data, &paths.runtime, &paths.tabs()] {
            std::fs::create_dir_all(d).map_err(|e| format!("couldn't create {}: {e}", d.display()))?;
            set_mode(d, 0o700);
        }
        let config = Config::load(&paths.config).unwrap_or_else(|e| {
            eprintln!("ctx server: {e}; using defaults");
            Config::default()
        });
        let socket = paths.socket();
        if let Some(d) = socket.parent() {
            let _ = std::fs::create_dir_all(d);
            set_mode(d, 0o700);
        }
        if UnixStream::connect(&socket).is_ok() {
            return Err(format!("a ctx server is already running on {}", socket.display()));
        }
        let _ = std::fs::remove_file(&socket);
        let listener =
            UnixListener::bind(&socket).map_err(|e| format!("couldn't listen on {}: {e}", socket.display()))?;
        set_mode(&socket, 0o600);
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        sys::set_cloexec_nonblock(listener.as_raw_fd(), true).map_err(|e| e.to_string())?;

        let (sig_r, sig_w) = sys::signal_pipe().map_err(|e| e.to_string())?;
        sys::install_signal_handlers(sig_w.as_raw_fd());
        std::mem::forget(sig_w);

        let mut poller = sys::Poller::new();
        poller.add(listener.as_raw_fd(), LISTENER, false);
        poller.add(sig_r.as_raw_fd(), SIGNALS, false);

        let index = Index::open(&paths.index()).map_err(|e| format!("couldn't open the index: {e}"))?;
        let activity = ActivityLog::open(&paths.activity_log());

        // Shell integration files.
        let zdotdir = paths.runtime.join("zsh");
        std::fs::create_dir_all(&zdotdir).map_err(|e| e.to_string())?;
        std::fs::write(zdotdir.join(".zshenv"), ZSHENV).map_err(|e| e.to_string())?;
        let integration = paths.runtime.join("ctx.zsh");
        std::fs::write(&integration, CTX_ZSH).map_err(|e| e.to_string())?;
        std::fs::write(paths.runtime.join("_ctx"), ZSH_COMPLETION).map_err(|e| e.to_string())?;
        // bash tabs: the login files, then the same marks as the remote bash integration.
        std::fs::write(paths.runtime.join("ctx.bashrc"), format!("{LOCAL_BASHRC}\n{REMOTE_BASH}"))
            .map_err(|e| e.to_string())?;

        let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("ctx"));
        let launch_path = exe.parent().map(|d| d.join("ctx-launch")).unwrap_or_else(|| PathBuf::from("ctx-launch"));

        let mut server = Server {
            config_mtime: std::fs::metadata(&paths.config).and_then(|m| m.modified()).ok(),
            paths,
            config,
            poller,
            listener,
            signals: sig_r,
            tabs: BTreeMap::new(),
            windows: Vec::new(),
            clients: HashMap::new(),
            next_key: 2,
            index,
            activity,
            started: Instant::now(),
            tz: sys::tz_offset(),
            last_window: None,
            waiters: Vec::new(),
            launch_path,
            integration,
            zdotdir,
            quit: false,
            restart: false,
            last_maintenance: Instant::now() - Duration::from_secs(86_400),
            last_config_check: Instant::now(),
            pending_show: None,
            dirty: Default::default(),
            last_frame: Instant::now(),
            server_exe: exe,
            starting: Vec::new(),
            relieved_at: 0,
        };
        server.recover();
        Ok(server)
    }

    /// After a crash, reboot or restart: tabs the index thinks are open are marked
    /// interrupted, and tabs that should come back are reopened.
    fn recover(&mut self) {
        let state: SavedState =
            std::fs::read(self.paths.state()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let _ = std::fs::remove_file(self.paths.state());
        let boot = sys::boot_time_secs() * 1000;
        let mut interrupted: Vec<(u64, i64)> = Vec::new();
        for t in self.index.open_tabs().unwrap_or_default() {
            let at = t.last_active_ms;
            if let Ok(mut log) = TabLog::open(&self.paths.tab_log(t.number)) {
                log.push(&Record::Closed { at_ms: at, reason: "interrupted".into() });
                let _ = log.flush();
            }
            let _ = self.index.set_closed(t.number, at, "interrupted");
            for c in self.index.commands(t.number).unwrap_or_default() {
                if c.finished_ms.is_none() {
                    let _ = self.index.finish_command(t.number, c.number, None, at.max(c.started_ms));
                }
            }
            interrupted.push((t.number, at));
        }
        // Windows as they were.
        for (n, name, _) in &state.windows {
            let _ = self.index.reserve_above("window", *n);
            let _ = name;
        }
        let mut to_reopen: Vec<(u64, String)> =
            state.reopen.iter().map(|n| (*n, "server restart".to_string())).collect();
        if to_reopen.is_empty() && self.config.reopen_on_launch == "always" {
            for (n, at) in &interrupted {
                let reason = if *at < boot { "reboot" } else { "server restart" };
                to_reopen.push((*n, reason.into()));
            }
        }
        for (n, reason) in to_reopen {
            let window =
                state.windows.iter().find(|(_, _, tabs)| tabs.contains(&n)).map(|(w, name, _)| (*w, name.clone()));
            let window = match window {
                Some((w, name)) => {
                    if !self.windows.iter().any(|x| x.number == w) {
                        self.windows.push(Window { number: w, name, tabs: Vec::new() });
                    }
                    w
                }
                None => self.new_window(None),
            };
            if let Err(e) = self.reopen_tab(n, window, Some(reason), "you") {
                eprintln!("ctx server: couldn't reopen tab {n}: {}", e.message);
            }
        }
        // Keep each window's tab order.
        for (w, _, order) in &state.windows {
            if let Some(win) = self.windows.iter_mut().find(|x| x.number == *w) {
                win.tabs.sort_by_key(|t| order.iter().position(|o| o == t).unwrap_or(usize::MAX));
            }
        }
        self.windows.retain(|w| !w.tabs.is_empty());
    }

    fn cx(&self) -> (i64, i64) {
        (now_ms(), self.tz)
    }

    // -----------------------------------------------------------------------------------------
    // Event loop
    // -----------------------------------------------------------------------------------------

    fn event_loop(&mut self) {
        while !self.quit {
            let timeout = self.next_timeout();
            let ready = match self.poller.wait(timeout) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("ctx server: poll failed: {e}");
                    break;
                }
            };
            for (key, readable, writable) in ready {
                match key {
                    LISTENER => self.accept(),
                    SIGNALS => self.on_signal(),
                    k if k >= TAB_KEY_BASE => {
                        let n = (k - TAB_KEY_BASE) as u64;
                        if readable {
                            self.on_tab_readable(n);
                        }
                        if writable {
                            self.flush_tab(n);
                        }
                    }
                    k => {
                        if readable {
                            self.on_client_readable(k);
                        }
                        if writable {
                            self.flush_client(k);
                        }
                    }
                }
            }
            self.timers();
            self.send_screen_updates();
            self.flush_all_clients();
        }
    }

    fn next_timeout(&self) -> Duration {
        let mut t = Duration::from_millis(1000);
        let now = Instant::now();
        for w in &self.waiters {
            if let Some(d) = w.deadline {
                t = t.min(d.saturating_duration_since(now));
            }
            if let WaitKind::Quiet { ms } = w.kind
                && let Some(tab) = self.tabs.get(&w.tab)
            {
                let due = tab.last_output_ms + ms as i64 - now_ms();
                t = t.min(Duration::from_millis(due.max(0) as u64));
            }
        }
        if !self.dirty.is_empty() {
            t = t.min(Duration::from_millis(8));
        }
        if self.tabs.values().any(|x| x.pending_enter.is_some()) || !self.starting.is_empty() {
            t = t.min(Duration::from_millis(50));
        }
        t.max(Duration::from_millis(1))
    }

    fn on_signal(&mut self) {
        let mut buf = [0u8; 64];
        while let Ok(n) = sys::read_fd(self.signals.as_raw_fd(), &mut buf) {
            if n == 0 {
                break;
            }
            if buf[..n].iter().any(|b| [libc::SIGTERM, libc::SIGINT, libc::SIGHUP].contains(&(*b as i32))) {
                TERMINATE.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
        let reaped = sys::reap();
        for (pid, _code) in reaped {
            if let Some(n) = self.tabs.values().find(|t| t.launch_pid == pid).map(|t| t.number) {
                // Drain what's left on the terminal, then end the tab.
                self.on_tab_readable(n);
                if self.tabs.contains_key(&n) {
                    self.end_tab(n, "closed");
                }
            }
        }
        if TERMINATE.load(std::sync::atomic::Ordering::Relaxed) {
            self.quit = true;
        }
    }

    /// Replay held requests whose tab is ready (or all of them after 10 seconds).
    fn replay_starting(&mut self, timed_out_only: bool) {
        let held = std::mem::take(&mut self.starting);
        for (key, env, at) in held {
            let target = self.request_tab(key, &env.request);
            let ready = target.and_then(|t| self.tabs.get(&t)).is_none_or(|t| t.at_prompt || !t.integration_expected);
            let expired = at.elapsed() >= Duration::from_secs(10);
            if (ready && !timed_out_only) || expired {
                if self.clients.contains_key(&key) {
                    self.on_request(key, env);
                }
            } else {
                self.starting.push((key, env, at));
            }
        }
    }

    fn timers(&mut self) {
        if !self.starting.is_empty() {
            self.replay_starting(true);
        }
        let held: Vec<u64> = self.tabs.values().filter(|t| t.pending_enter.is_some()).map(|t| t.number).collect();
        for n in held {
            self.release_enter(n, true);
        }
        let now = Instant::now();
        let ms = now_ms();
        // Waiters: time limits and quiet periods.
        let mut fire: Vec<usize> = Vec::new();
        for (i, w) in self.waiters.iter().enumerate() {
            if w.deadline.is_some_and(|d| now >= d) {
                fire.push(i);
                continue;
            }
            if let WaitKind::Quiet { ms: q } = w.kind
                && let Some(tab) = self.tabs.get(&w.tab)
                && ms - tab.last_output_ms.max(w_started_ms(w)) >= q as i64
            {
                fire.push(i);
            }
        }
        for i in fire.into_iter().rev() {
            let w = self.waiters.remove(i);
            self.fire_waiter(w, ms);
        }
        // Idle tabs give memory back (bounded work per turn), and once everything has been quiet
        // for a while after output, the allocator returns what it no longer uses.
        for tab in self.tabs.values_mut() {
            if ms - tab.last_output_ms >= 2_000 {
                tab.term.compact();
            }
        }
        let last_output = self.tabs.values().map(|t| t.last_output_ms).max().unwrap_or(0);
        if last_output > self.relieved_at && ms - last_output >= 2_000 {
            self.relieved_at = ms;
            sys::release_free_memory();
        }
        // Flush records every 30 seconds while output keeps coming.
        for tab in self.tabs.values_mut() {
            if ms - tab.last_flush_ms >= 30_000
                && (tab.log.has_pending()
                    || !tab.unmarked.is_empty()
                    || tab.running.iter().any(|r| !r.unflushed.is_empty()))
            {
                tab.flush(ms);
            }
        }
        // Config reload.
        if now.duration_since(self.last_config_check) >= Duration::from_secs(2) {
            self.last_config_check = now;
            let mtime = std::fs::metadata(&self.paths.config).and_then(|m| m.modified()).ok();
            if mtime != self.config_mtime {
                self.config_mtime = mtime;
                match Config::load(&self.paths.config) {
                    Ok(c) => self.config = c,
                    Err(e) => eprintln!("ctx server: {e}; keeping the previous settings"),
                }
            }
            self.tz = sys::tz_offset();
        }
        // Daily retention.
        if now.duration_since(self.last_maintenance) >= Duration::from_secs(86_400) {
            self.last_maintenance = now;
            self.prune();
        }
    }

    // -----------------------------------------------------------------------------------------
    // Clients
    // -----------------------------------------------------------------------------------------

    fn accept(&mut self) {
        loop {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    let fd = stream.as_raw_fd();
                    // Only our own user (the socket is 0600 too).
                    if sys::peer_uid(fd) != Some(sys::uid()) {
                        continue;
                    }
                    let _ = stream.set_nonblocking(true);
                    let _ = sys::set_cloexec_nonblock(fd, true);
                    let key = self.next_key;
                    self.next_key += 1;
                    self.poller.add(fd, key, false);
                    self.clients.insert(
                        key,
                        Client {
                            stream,
                            inbuf: FrameBuffer::default(),
                            out: Vec::new(),
                            hello: None,
                            identity: Identity::unknown(),
                            mode: Mode::Requests,
                            waiting: false,
                            close_after_flush: false,
                            mismatched: false,
                        },
                    );
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
    }

    fn drop_client(&mut self, key: usize) {
        if let Some(c) = self.clients.remove(&key) {
            self.poller.delete(c.stream.as_raw_fd());
            match c.mode {
                Mode::Attached(t) => {
                    if let Some(tab) = self.tabs.get_mut(&t) {
                        tab.viewers = tab.viewers.saturating_sub(1);
                    }
                }
                Mode::App(tabs) => {
                    for t in tabs {
                        if let Some(tab) = self.tabs.get_mut(&t) {
                            tab.viewers = tab.viewers.saturating_sub(1);
                        }
                    }
                }
                Mode::Requests => {}
            }
        }
        self.waiters.retain(|w| w.client != key);
    }

    fn on_client_readable(&mut self, key: usize) {
        let mut buf = [0u8; 65536];
        let mut closed = false;
        {
            let Some(c) = self.clients.get_mut(&key) else { return };
            loop {
                match c.stream.read(&mut buf) {
                    Ok(0) => {
                        closed = true;
                        break;
                    }
                    Ok(n) => c.inbuf.extend(&buf[..n]),
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => {
                        closed = true;
                        break;
                    }
                }
            }
        }
        loop {
            let frame = match self.clients.get_mut(&key).map(|c| c.inbuf.next_frame()) {
                Some(Ok(Some(f))) => f,
                Some(Ok(None)) => break,
                _ => {
                    closed = true;
                    break;
                }
            };
            self.on_frame(key, frame);
            if !self.clients.contains_key(&key) {
                return;
            }
        }
        if closed {
            self.drop_client(key);
        }
    }

    fn send(&mut self, key: usize, msg: &impl serde::Serialize) {
        if let Some(c) = self.clients.get_mut(&key)
            && let Ok(bytes) = encode(msg)
        {
            c.out.extend_from_slice(&bytes);
        }
    }

    fn reply(&mut self, key: usize, r: Response) {
        let in_app = matches!(self.clients.get(&key).map(|c| &c.mode), Some(Mode::App(_)));
        if in_app {
            self.send(key, &StreamOut::Reply(r));
        } else {
            self.send(key, &r);
        }
        if let Some(c) = self.clients.get_mut(&key) {
            c.waiting = false;
        }
    }

    fn flush_client(&mut self, key: usize) {
        let mut drop_it = false;
        if let Some(c) = self.clients.get_mut(&key) {
            while !c.out.is_empty() {
                match c.stream.write(&c.out) {
                    Ok(0) => {
                        drop_it = true;
                        break;
                    }
                    Ok(n) => {
                        c.out.drain(..n);
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => {
                        drop_it = true;
                        break;
                    }
                }
            }
            let want_write = !c.out.is_empty();
            self.poller.modify(c.stream.as_raw_fd(), want_write);
            if c.out.is_empty() && c.close_after_flush {
                drop_it = true;
            }
        }
        if drop_it {
            self.drop_client(key);
        }
    }

    fn flush_all_clients(&mut self) {
        let keys: Vec<usize> =
            self.clients.iter().filter(|(_, c)| !c.out.is_empty() || c.close_after_flush).map(|(k, _)| *k).collect();
        for k in keys {
            self.flush_client(k);
        }
    }

    fn tab_of_pid(&self, pid: i32) -> Option<u64> {
        self.tabs.values().find(|t| t.launch_pid == pid).map(|t| t.number)
    }

    fn on_frame(&mut self, key: usize, frame: Vec<u8>) {
        let has_hello = self.clients.get(&key).is_some_and(|c| c.hello.is_some());
        let mismatched = self.clients.get(&key).is_some_and(|c| c.mismatched);
        if mismatched {
            if frame == RESTART_MAGIC {
                let busy: Vec<u64> = self.tabs.values().filter(|t| t.busy()).map(|t| t.number).collect();
                if busy.is_empty() {
                    self.restart = true;
                    self.quit = true;
                }
            }
            if let Some(c) = self.clients.get_mut(&key) {
                c.close_after_flush = true;
            }
            return;
        }
        if !has_hello {
            let Ok(hello) = decode::<Hello>(&frame) else {
                self.drop_client(key);
                return;
            };
            if hello.protocol != PROTOCOL_VERSION {
                let busy: Vec<u64> = self.tabs.values().filter(|t| t.busy()).map(|t| t.number).collect();
                let message = format!(
                    "The running ctx server is version {} (protocol {}), and this ctx is version {} (protocol {}). {}",
                    env!("CARGO_PKG_VERSION"),
                    PROTOCOL_VERSION,
                    hello.client_version,
                    hello.protocol,
                    if busy.is_empty() {
                        "Restarting the server picks up the new version."
                    } else {
                        "Restart the server to use the new version: ctx restart server"
                    }
                );
                self.send(
                    key,
                    &Welcome::VersionMismatch {
                        server_version: env!("CARGO_PKG_VERSION").into(),
                        server_protocol: PROTOCOL_VERSION,
                        message,
                        busy_tabs: busy,
                    },
                );
                if let Some(c) = self.clients.get_mut(&key) {
                    c.mismatched = true;
                }
                return;
            }
            let fd = self.clients.get(&key).map(|c| c.stream.as_raw_fd()).unwrap_or(-1);
            let pid = sys::peer_pid(fd).unwrap_or(0);
            let identity = {
                let tabs = &self.tabs;
                identify(pid, hello.declared_name.clone(), &|p| {
                    tabs.values().find(|t| t.launch_pid == p).map(|t| t.number)
                })
            };
            let is_app = hello.kind == ClientKind::App;
            if let Some(c) = self.clients.get_mut(&key) {
                c.identity = identity;
                c.identity.is_app = c.identity.is_app || is_app;
                c.hello = Some(hello);
                // CTX.app talks in stream messages from the start.
                if is_app {
                    c.mode = Mode::App(Vec::new());
                }
            }
            self.send(
                key,
                &Welcome::Accepted { server_version: env!("CARGO_PKG_VERSION").into(), server_pid: std::process::id() },
            );
            return;
        }
        let mode = self.clients.get(&key).map(|c| c.mode.clone()).unwrap_or(Mode::Requests);
        match mode {
            Mode::Requests => {
                let Ok(env) = decode::<Envelope>(&frame) else {
                    self.reply(
                        key,
                        Response::Error(ErrorReply::new(ErrorKind::Failed, "The server couldn't read that request.")),
                    );
                    return;
                };
                self.on_request(key, env);
            }
            Mode::Attached(tab) => {
                let Ok(msg) = decode::<StreamIn>(&frame) else { return };
                self.on_attached_input(key, tab, msg);
            }
            Mode::App(_) => {
                let Ok(msg) = decode::<StreamIn>(&frame) else { return };
                match msg {
                    StreamIn::Request(env) => self.on_request(key, env),
                    StreamIn::Detach => self.drop_client(key),
                    _ => {}
                }
            }
        }
    }

    fn log_request(&mut self, key: usize, env: &Envelope, tab: Option<u64>) {
        let Some(c) = self.clients.get(&key) else { return };
        if c.identity.is_app {
            return;
        }
        let is_read = matches!(
            env.request,
            Request::Read { .. }
                | Request::ListCommands { .. }
                | Request::Find { .. }
                | Request::Records { .. }
                | Request::ListTabs
                | Request::ListClosedTabs { .. }
                | Request::ListActivity { .. }
                | Request::Status
        );
        if is_read && !self.config.log_reads {
            return;
        }
        let typed = match &env.request {
            Request::Type { text, .. } => Some(text.clone()),
            Request::Run { command, .. } => Some(command.clone()),
            _ => None,
        };
        let ms = now_ms();
        let line = LogLine {
            at: iso_utc(ms),
            at_ms: ms,
            client: c.identity.describe(),
            program: c.identity.program.clone(),
            pid: c.identity.pid,
            start_us: c.identity.start_us,
            declared_name: c.identity.declared.clone(),
            ran_by: c.identity.ran_by(),
            sentence: env.sentence.clone(),
            tab,
            typed,
        };
        self.activity.append(&line);
        if let Some(t) = tab
            && let Some(tab) = self.tabs.get(&t)
        {
            let _ = tab;
            self.dirty.insert(t);
        }
    }

    fn on_request(&mut self, key: usize, env: Envelope) {
        let target = self.request_tab(key, &env.request);
        // A tab whose shell hasn't shown its first prompt yet: hold the request until it has.
        if let Some(t) = target
            && let Some(tab) = self.tabs.get(&t)
            && tab.integration_expected
            && !tab.at_prompt
            && tab.running.is_empty()
            && tab.last_finished.is_none()
            && now_ms() - tab.created_ms < 10_000
            && !matches!(
                env.request,
                Request::ListCommands { .. }
                    | Request::Read { .. }
                    | Request::Records { .. }
                    | Request::CloseTab { .. }
            )
        {
            self.starting.push((key, env, Instant::now()));
            return;
        }
        self.log_request(key, &env, target);
        if let Some(c) = self.clients.get_mut(&key) {
            c.waiting = true;
        }
        let handled = self.handle(key, env.request);
        match handled {
            Handled::Reply(r) => self.reply(key, r),
            Handled::Deferred => {}
        }
    }

    /// The tab a request is about, if it resolves (for the activity log).
    fn request_tab(&self, key: usize, r: &Request) -> Option<u64> {
        let t = match r {
            Request::ListCommands { tab, .. }
            | Request::Read { tab, .. }
            | Request::CloseTab { tab, .. }
            | Request::RenameTab { tab, .. }
            | Request::Move { tab, .. }
            | Request::Clear { tab }
            | Request::Run { tab, .. }
            | Request::Type { tab, .. }
            | Request::Press { tab, .. }
            | Request::Wait { tab, .. }
            | Request::Show { tab }
            | Request::Attach { tab, .. }
            | Request::Records { tab } => tab,
            Request::Reopen { tab } | Request::Forget { tab, .. } => return Some(*tab),
            Request::Input { tab, .. } => return Some(*tab),
            _ => return None,
        };
        self.resolve_any(key, t).ok()
    }

    // -----------------------------------------------------------------------------------------
    // Resolving names
    // -----------------------------------------------------------------------------------------

    fn client_tab(&self, key: usize) -> Option<u64> {
        let c = self.clients.get(&key)?;
        c.identity.in_tab.filter(|t| self.tabs.contains_key(t))
    }

    fn open_tab_list(&self) -> String {
        let nums: Vec<String> = self.tabs.keys().map(|n| n.to_string()).collect();
        match nums.len() {
            0 => "No tabs are open.".into(),
            1 => format!("The open tab is {}.", nums[0]),
            _ => format!("Open tabs are {} and {}.", nums[..nums.len() - 1].join(", "), nums[nums.len() - 1]),
        }
    }

    /// An open tab.
    fn resolve(&self, key: usize, t: &TabRef) -> Result<u64, ErrorReply> {
        match t {
            TabRef::This => self.client_tab(key).ok_or_else(|| {
                ErrorReply::new(
                    ErrorKind::NoSuchTab,
                    format!("Which tab? This isn't running inside a CTX tab, so say which one: in \"<name>\" or in tab <number>. {} `ctx list tabs` shows them.", self.open_tab_list()),
                )
            }),
            TabRef::Number(n) => {
                if self.tabs.contains_key(n) {
                    Ok(*n)
                } else if self.index.tab(*n).ok().flatten().is_some() {
                    Err(ErrorReply::new(ErrorKind::NoSuchTab, format!("Tab {n} is closed. `ctx read last 20 lines in tab {n}` reads it, and `ctx reopen tab {n}` brings it back.")))
                } else {
                    Err(ErrorReply::new(
                        ErrorKind::NoSuchTab,
                        format!("There is no tab {n}. {} `ctx list tabs` shows them all, and `ctx list closed tabs` shows ones that have been closed.", self.open_tab_list()),
                    ))
                }
            }
            TabRef::Name(name) => {
                if let Some(t) = self.tabs.values().find(|t| t.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(name))) {
                    return Ok(t.number);
                }
                // Closest open name, for a near miss.
                let lower = name.to_lowercase();
                let mut best: Option<(&Tab, usize)> = None;
                for t in self.tabs.values() {
                    if let Some(n) = &t.name {
                        let d = ctx_distance(&lower, &n.to_lowercase());
                        if d <= 2.max(lower.len() / 4) && best.is_none_or(|(_, bd)| d < bd) {
                            best = Some((t, d));
                        }
                    }
                }
                if let Some((t, _)) = best {
                    return Err(ErrorReply::new(ErrorKind::NoSuchTab, format!("There's no open tab named \"{name}\". Did you mean \"{}\" (tab {})?", t.name.clone().unwrap_or_default(), t.number)));
                }
                if let Ok(closed) = self.index.closed_tabs_named(name)
                    && let Some(c) = closed.first()
                {
                    return Err(ErrorReply::new(
                        ErrorKind::NoSuchTab,
                        format!("The tab named \"{name}\" is closed (tab {n}). `ctx read last 20 lines in tab {n}` reads it, and `ctx reopen tab {n}` brings it back.", n = c.number),
                    ));
                }
                let names: Vec<String> = self.tabs.values().filter_map(|t| t.name.as_ref().map(|n| format!("\"{n}\""))).collect();
                let listing = if names.is_empty() { self.open_tab_list() } else { format!("Open tabs with names: {}.", names.join(", ")) };
                Err(ErrorReply::new(ErrorKind::NoSuchTab, format!("There's no tab named \"{name}\". {listing} `ctx list tabs` shows them all.")))
            }
        }
    }

    /// An open tab, or a closed one (by number or past name).
    fn resolve_any(&self, key: usize, t: &TabRef) -> Result<u64, ErrorReply> {
        match self.resolve(key, t) {
            Ok(n) => Ok(n),
            Err(e) => match t {
                TabRef::Number(n) if self.index.tab(*n).ok().flatten().is_some() => Ok(*n),
                TabRef::Name(name) => match self.index.closed_tabs_named(name).ok().and_then(|v| v.first().cloned()) {
                    Some(row) => Ok(row.number),
                    None => Err(e),
                },
                _ => Err(e),
            },
        }
    }

    fn resolve_window(&self, w: &WindowRef) -> Result<u64, ErrorReply> {
        let found = match w {
            WindowRef::Number(n) => self.windows.iter().find(|x| x.number == *n),
            WindowRef::Name(name) => {
                self.windows.iter().find(|x| x.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(name)))
            }
        };
        found.map(|w| w.number).ok_or_else(|| {
            let list: Vec<String> = self
                .windows
                .iter()
                .map(|w| match &w.name {
                    Some(n) => format!("window {} \"{n}\"", w.number),
                    None => format!("window {}", w.number),
                })
                .collect();
            let shown = match w {
                WindowRef::Number(n) => format!("window {n}"),
                WindowRef::Name(n) => format!("window \"{n}\""),
            };
            ErrorReply::new(
                ErrorKind::NoSuchWindow,
                format!(
                    "There is no {shown}. {}",
                    if list.is_empty() {
                        "No windows are open.".to_string()
                    } else {
                        format!("Open windows: {}.", list.join(", "))
                    }
                ),
            )
        })
    }

    fn validate_name(&self, name: &str, except_tab: Option<u64>, for_window: bool) -> Result<String, ErrorReply> {
        let name = name.trim().to_string();
        if let Err(msg) = check_name(&name) {
            return Err(ErrorReply::new(ErrorKind::NotAllowed, msg));
        }
        if !for_window
            && let Some(t) = self.tabs.values().find(|t| {
                Some(t.number) != except_tab && t.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(&name))
            })
        {
            return Err(ErrorReply::new(
                ErrorKind::NotAllowed,
                format!("Tab {} is already named \"{name}\". Names must be different among open tabs.", t.number),
            ));
        }
        Ok(name)
    }

    // -----------------------------------------------------------------------------------------
    // Requests
    // -----------------------------------------------------------------------------------------

    fn handle(&mut self, key: usize, req: Request) -> Handled {
        match req {
            Request::ListTabs => ok(Output::Tabs(self.tab_list())),
            Request::ListClosedTabs { limit } => self.list_closed(limit),
            Request::ListCommands { tab, limit } => self.list_commands(key, &tab, limit),
            Request::ListActivity { filter, limit } => self.list_activity(key, filter, limit),
            Request::Read { tab, what, colors, max_lines } => self.read(key, &tab, what, colors, max_lines),
            Request::Find { text, filter, limit } => self.find(key, &text, filter, limit),
            Request::Status => ok(Output::Status(self.status())),
            Request::OpenWindow { folder, name } => self.open_window(key, folder, name),
            Request::OpenTab { window, folder, name } => self.open_tab(key, window, folder, name),
            Request::CloseTab { tab, without_asking } => self.close_tab(key, &tab, without_asking),
            Request::CloseWindow { window, without_asking } => self.close_window(&window, without_asking),
            Request::RenameTab { tab, to } => self.rename_tab(key, &tab, &to),
            Request::RenameWindow { window, to } => self.rename_window(&window, &to),
            Request::Move { tab, to, position } => self.move_tab(key, &tab, &to, position),
            Request::Clear { tab } => self.clear(key, &tab),
            Request::Reopen { tab } => {
                let window = self.default_window(key);
                let by = self.clients.get(&key).map(|c| c.identity.ran_by()).unwrap_or_else(|| "you".into());
                match self.reopen_tab(tab, window, None, &by) {
                    Ok(o) => self.when_ready(key, o),
                    Err(e) => {
                        self.windows.retain(|w| !w.tabs.is_empty());
                        Handled::Reply(Response::Error(e))
                    }
                }
            }
            Request::Forget { tab, without_asking } => self.forget(key, tab, without_asking),
            Request::Run { tab, command, colors, max_lines, within_ms, wait } => {
                self.run_command(key, &tab, command, colors, max_lines, within_ms, wait)
            }
            Request::Type { tab, text, then_press } => self.type_text(key, &tab, &text, then_press),
            Request::Press { tab, keys } => self.press(key, &tab, &keys),
            Request::Wait { tab, until, within_ms } => self.wait(key, &tab, until, within_ms),
            Request::Show { tab } => self.show(key, &tab),
            Request::Attach { tab, columns, rows } => self.attach(key, &tab, columns, rows),
            Request::Subscribe { tabs } => self.subscribe(key, tabs),
            Request::Input { tab, input } => self.app_input(key, tab, input),
            Request::Records { tab } => self.records(key, &tab),
            Request::MatchingTabs { filter, limit } => self.matching_tabs(key, filter, limit),
            Request::WhoRan { pid } => self.who_ran(pid),
            Request::Restart { without_asking } => self.restart_request(without_asking),
        }
    }

    fn tab_info(&self, t: &Tab) -> TabInfo {
        let fg = t.foreground();
        let running = t.current().map(|r| r.text.clone()).filter(|s| !s.is_empty()).or_else(|| {
            if !t.integration
                && let Some((_, name)) = &fg
                && !t.shell_in_front()
            {
                return Some(name.clone());
            }
            None
        });
        let state = if t.typing && t.at_prompt {
            "typing"
        } else if t.busy() || (!t.integration && !t.shell_in_front()) {
            "busy"
        } else {
            "idle"
        };
        let permissions_of = fg.as_ref().and_then(|(pid, _)| {
            let resp = sys::responsible_pid(*pid)?;
            if resp == t.launch_pid || resp == *pid && t.shell_in_front() {
                return None;
            }
            if resp == t.launch_pid {
                return None;
            }
            sys::proc_info(resp).map(|p| p.name)
        });
        TabInfo {
            number: t.number,
            name: t.name.clone(),
            folder: t.folder.clone(),
            window: t.window,
            opened_by: t.opened_by.clone(),
            title: t.title.clone(),
            state: state.into(),
            running,
            pid: fg.map(|(p, _)| p),
            started: t.current().map(|r| iso_utc(r.started_ms)),
            unread_lines: t.unread_lines,
            columns: t.columns,
            rows: t.rows,
            shell_integration: t.integration,
            permissions_of,
            remote_host: t.ssh_target.clone(),
        }
    }

    fn tab_list(&self) -> TabList {
        TabList {
            windows: self
                .windows
                .iter()
                .map(|w| WindowInfo {
                    number: w.number,
                    name: w.name.clone(),
                    tabs: w.tabs.iter().filter_map(|n| self.tabs.get(n)).map(|t| self.tab_info(t)).collect(),
                })
                .collect(),
        }
    }

    fn list_closed(&self, limit: u32) -> Handled {
        let (rows, total) = self.index.closed_tabs(limit).unwrap_or_default();
        ok(Output::ClosedTabs(ClosedTabList {
            tabs: rows
                .into_iter()
                .map(|r| ClosedTab {
                    number: r.number,
                    name: r.name,
                    folder: r.folder,
                    opened_by: r.opened_by,
                    opened_at: iso_utc(r.opened_ms),
                    closed_at: iso_utc(r.closed_ms.unwrap_or(r.last_active_ms)),
                    ended: r.ended.unwrap_or_else(|| "closed".into()),
                    commands: r.commands,
                })
                .collect(),
            total,
        }))
    }

    fn list_commands(&mut self, key: usize, tab: &TabRef, limit: Option<u32>) -> Handled {
        let n = match self.resolve_any(key, tab) {
            Ok(n) => n,
            Err(e) => return Handled::Reply(Response::Error(e)),
        };
        let rows = self.index.commands(n).unwrap_or_default();
        let total = rows.len() as u64;
        let skip = limit.map(|l| rows.len().saturating_sub(l as usize)).unwrap_or(0);
        let commands = rows
            .into_iter()
            .skip(skip)
            .map(|c| CommandInfo {
                number: c.number,
                command: c.text,
                location: c.location,
                ran_by: c.ran_by,
                exit_status: c.exit_status,
                started_at: iso_utc(c.started_ms),
                finished_at: c.finished_ms.map(iso_utc),
                took_seconds: c.finished_ms.map(|f| (f - c.started_ms) as f64 / 1000.0),
                approximate: c.approximate,
            })
            .collect();
        ok(Output::Commands(CommandList { tab: n, tab_name: self.tab_name(n), commands, total }))
    }

    fn tab_name(&self, n: u64) -> Option<String> {
        match self.tabs.get(&n) {
            Some(t) => t.name.clone(),
            None => self.index.tab(n).ok().flatten().and_then(|r| r.name),
        }
    }

    fn list_activity(&self, key: usize, filter: Filter, limit: u32) -> Handled {
        let tab = match &filter.tab {
            Some(t) => match self.resolve_any(key, t) {
                Ok(n) => Some(n),
                Err(e) => return Handled::Reply(Response::Error(e)),
            },
            None => None,
        };
        let since_ms = filter.since.map(|s| s * 1000);
        let by = filter.by.clone().map(|b| b.to_lowercase());
        let entries = self.activity.read(limit as usize, &|l| {
            if let Some(t) = tab
                && l.tab != Some(t)
            {
                return false;
            }
            if let Some(s) = since_ms
                && l.at_ms < s
            {
                return false;
            }
            if let Some(b) = &by {
                let matches = if b == "me" {
                    l.ran_by == "you"
                } else {
                    l.program.to_lowercase().contains(b.as_str())
                        || l.declared_name.as_ref().is_some_and(|d| d.to_lowercase().contains(b.as_str()))
                };
                if !matches {
                    return false;
                }
            }
            true
        });
        ok(Output::Activity(ActivityList { entries: entries.iter().map(|l| l.entry()).collect() }))
    }

    fn status(&self) -> Status {
        let tabs = self
            .tabs
            .values()
            .map(|t| TabMemory {
                number: t.number,
                name: t.name.clone(),
                memory_bytes: t.term.memory_estimate() + t.cold.bytes() as u64,
                scrollback_lines: t.cold.len() as u64 + t.term.history_rows(),
            })
            .collect();
        let saved_bytes = std::fs::read_dir(self.paths.tabs())
            .map(|d| d.filter_map(|e| e.ok()?.metadata().ok()).map(|m| m.len()).sum())
            .unwrap_or(0)
            + std::fs::metadata(self.paths.index()).map(|m| m.len()).unwrap_or(0);
        Status {
            version: env!("CARGO_PKG_VERSION").into(),
            protocol: PROTOCOL_VERSION,
            pid: std::process::id(),
            uptime_seconds: self.started.elapsed().as_secs(),
            socket: self.paths.socket().display().to_string(),
            memory_bytes: sys::memory_footprint(std::process::id() as i32),
            saved_bytes,
            tabs,
        }
    }

    // ----- opening -----

    fn new_window(&mut self, name: Option<String>) -> u64 {
        let n = self
            .index
            .next_number("window")
            .unwrap_or_else(|_| self.windows.iter().map(|w| w.number).max().unwrap_or(0) + 1);
        self.windows.push(Window { number: n, name, tabs: Vec::new() });
        n
    }

    /// Where a new tab goes when no window is named.
    fn default_window(&mut self, key: usize) -> u64 {
        if let Some(t) = self.client_tab(key)
            && let Some(tab) = self.tabs.get(&t)
        {
            return tab.window;
        }
        let is_agent = self.clients.get(&key).is_some_and(|c| c.identity.is_agent && !c.identity.is_app);
        if is_agent {
            let name = self.config.agent_window_name.clone();
            if let Some(w) = self.windows.iter().find(|w| w.name.as_deref() == Some(name.as_str())) {
                return w.number;
            }
            return self.new_window(Some(name));
        }
        if let Some(w) = self.last_window.filter(|w| self.windows.iter().any(|x| x.number == *w)) {
            return w;
        }
        if let Some(w) = self
            .windows
            .iter()
            .find(|w| Some(w.name.clone().unwrap_or_default()) != Some(self.config.agent_window_name.clone()))
        {
            return w.number;
        }
        self.new_window(None)
    }

    /// Answer `open` once the new shell is ready for commands (its first prompt), so a `run`
    /// right after `open` never races the shell's startup.
    fn when_ready(&mut self, key: usize, o: Opened) -> Handled {
        let ready = o.tab.and_then(|t| self.tabs.get(&t)).is_none_or(|t| !t.integration_expected || t.integration);
        if ready {
            return ok(Output::Opened(o));
        }
        let tab = o.tab.unwrap_or(0);
        self.waiters.push(Waiter {
            client: key,
            tab,
            kind: WaitKind::Ready { opened: o },
            deadline: Some(Instant::now() + Duration::from_secs(10)),
            started: Instant::now(),
        });
        Handled::Deferred
    }

    fn open_window(&mut self, key: usize, folder: Option<String>, name: Option<String>) -> Handled {
        let name = match name {
            Some(n) => match self.validate_name(&n, None, true) {
                Ok(n) => {
                    if self.windows.iter().any(|w| w.name.as_deref().is_some_and(|x| x.eq_ignore_ascii_case(&n))) {
                        return err(ErrorKind::NotAllowed, format!("A window is already named \"{n}\"."));
                    }
                    Some(n)
                }
                Err(e) => return Handled::Reply(Response::Error(e)),
            },
            None => None,
        };
        let folder = folder.unwrap_or_else(sys::home);
        let w = self.new_window(name.clone());
        let by = self.clients.get(&key).map(|c| c.identity.ran_by()).unwrap_or_else(|| "you".into());
        match self.spawn_tab(None, w, &folder, None, &by, None) {
            Ok(mut o) => {
                self.last_window = Some(w);
                o.new_window = true;
                o.window_name = name;
                self.when_ready(key, o)
            }
            Err(e) => {
                self.windows.retain(|x| x.number != w);
                Handled::Reply(Response::Error(e))
            }
        }
    }

    fn open_tab(
        &mut self,
        key: usize,
        window: Option<WindowRef>,
        folder: Option<String>,
        name: Option<String>,
    ) -> Handled {
        let name = match name {
            Some(n) => match self.validate_name(&n, None, false) {
                Ok(n) => Some(n),
                Err(e) => return Handled::Reply(Response::Error(e)),
            },
            None => None,
        };
        let w = match &window {
            Some(wr) => match self.resolve_window(wr) {
                Ok(w) => w,
                Err(e) => return Handled::Reply(Response::Error(e)),
            },
            None => self.default_window(key),
        };
        let folder = folder.unwrap_or_else(|| {
            if let Some(t) = self.client_tab(key).and_then(|t| self.tabs.get(&t)) {
                return t.folder.clone();
            }
            self.clients
                .get(&key)
                .and_then(|c| c.hello.as_ref())
                .map(|h| h.folder.clone())
                .filter(|f| std::path::Path::new(f).is_dir())
                .unwrap_or_else(sys::home)
        });
        let by = self.clients.get(&key).map(|c| c.identity.ran_by()).unwrap_or_else(|| "you".into());
        match self.spawn_tab(None, w, &folder, name, &by, None) {
            Ok(o) => self.when_ready(key, o),
            Err(e) => {
                self.windows.retain(|w| !w.tabs.is_empty());
                Handled::Reply(Response::Error(e))
            }
        }
    }

    /// Start a tab's shell. `reopen` carries what to restore.
    fn spawn_tab(
        &mut self,
        number: Option<u64>,
        window: u64,
        folder: &str,
        name: Option<String>,
        opened_by: &str,
        reopen: Option<Reopening>,
    ) -> Result<Opened, ErrorReply> {
        let failed = |m: String| ErrorReply::new(ErrorKind::Failed, m);
        if !self.launch_path.exists() {
            return Err(failed(format!(
                "ctx-launch is missing (expected at {}). Reinstall ctx.",
                self.launch_path.display()
            )));
        }
        let number = match number {
            Some(n) => n,
            None => self.index.next_number("tab").map_err(|e| failed(format!("couldn't number the new tab: {e}")))?,
        };
        let (columns, rows) =
            reopen.as_ref().map(|r| (r.columns, r.rows)).filter(|(c, r)| *c >= 20 && *r >= 5).unwrap_or((120, 40));
        let (master, slave) =
            sys::open_pty(columns, rows).map_err(|e| failed(format!("couldn't open a terminal: {e}")))?;
        let shell = if self.config.shell.is_empty() { sys::login_shell() } else { self.config.shell.clone() };
        let shell_name = shell.rsplit('/').next().unwrap_or("zsh").to_string();
        let is_zsh = shell_name == "zsh";
        let folder = if std::path::Path::new(folder).is_dir() { folder.to_string() } else { sys::home() };

        let mut env = self.shell_env(number, window, is_zsh);
        let mut not_restored = Vec::new();
        if let Some(r) = &reopen {
            for (k, v) in &r.env {
                if skip_on_reopen(k) {
                    continue;
                }
                if self.config.env_excluded(k) {
                    not_restored.push(k.clone());
                    continue;
                }
                env.retain(|(ek, _)| ek != k);
                env.push((k.clone(), v.clone()));
            }
            for p in &self.config.dont_save_env {
                if !not_restored.contains(p) && !r.env.keys().any(|k| ctx_protocol::config::glob_match(p, k)) {
                    continue;
                }
            }
            if let Some(h) = &r.history_file {
                env.push(("CTX_REOPEN_HISTORY".into(), h.clone()));
            }
            if let Some(p) = &r.prefill {
                env.push(("CTX_PREFILL".into(), p.clone()));
            }
        }
        let mut args = vec!["ctx-launch".to_string(), folder.clone(), shell.clone()];
        if shell_name == "bash" {
            // A login bash can't take an rc file; this one reads the login files itself.
            let rc = self.paths.runtime.join("ctx.bashrc").display().to_string();
            args.extend(["bash".to_string(), "--rcfile".into(), rc, "-i".into()]);
        } else {
            args.push(format!("-{shell_name}"));
        }
        let disclaim = sys::can_disclaim() && std::env::var_os("CTX_NO_DISCLAIM").is_none();
        let pid = sys::spawn(&self.launch_path, &args, &env, Some(slave.as_raw_fd()), disclaim)
            .map_err(|e| failed(format!("couldn't start the shell: {e}")))?;
        drop(slave);

        let log = TabLog::open(&self.paths.tab_log(number))
            .map_err(|e| failed(format!("couldn't open the tab's record: {e}")))?;
        let now = now_ms();
        let mut term: Box<dyn Terminal> = Box::new(Ghostty::new(columns, rows));
        let mut cold = ColdStore::default();
        // Replay the saved output of a reopened tab.
        if let Some(r) = &reopen {
            let keep = r.transcript.len().saturating_sub(5000);
            for l in &r.transcript[..keep] {
                cold.push_line(l.clone());
            }
            for l in &r.transcript[keep..] {
                let shown = display_line(l, true);
                term.write(shown.as_bytes());
                term.write(b"\r\n");
            }
            let _ = term.take_events();
        }
        let mut tab = Tab {
            number,
            name: name.clone(),
            window,
            folder: folder.clone(),
            host: sys::hostname(),
            location: format!("{}@{}", sys::username(), sys::hostname()),
            opened_by: reopen.as_ref().map(|r| r.opened_by.clone()).unwrap_or_else(|| opened_by.to_string()),
            opened_ms: reopen.as_ref().map(|r| r.opened_ms).unwrap_or(now),
            master,
            launch_pid: pid,
            term,
            scanner: Scanner::default(),
            cold,
            log,
            to_pty: Vec::new(),
            integration_expected: is_zsh || shell_name == "bash",
            integration: false,
            at_prompt: false,
            prompt_pin: None,
            typing: false,
            pending_text: None,
            running: Vec::new(),
            finished: Default::default(),
            next_command: reopen.as_ref().map(|r| r.next_command).unwrap_or(1),
            last_exit: None,
            last_finished: None,
            contributors: Vec::new(),
            env: Default::default(),
            env_reported: false,
            title: None,
            last_output_ms: now,
            unread_lines: 0,
            event_queue: Vec::new(),
            viewers: 0,
            unmarked: Vec::new(),
            unmarked_partial: None,
            last_flush_ms: now,
            columns,
            rows,
            ssh_target: None,
            remote_folder: None,
            remote_location: None,
            remote_marks: false,
            pending_enter: None,
            output_seq: 0,
            ended: false,
            shell_name,
            created_ms: now,
            render: None,
        };
        tab.log.push(&Record::Opened {
            number,
            at_ms: now,
            window,
            name: name.clone(),
            folder: folder.clone(),
            opened_by: tab.opened_by.clone(),
            columns,
            rows,
        });
        let _ = self.index.upsert_tab(&TabRow {
            number,
            name: name.clone(),
            folder: folder.clone(),
            window,
            opened_by: tab.opened_by.clone(),
            opened_ms: tab.opened_ms,
            closed_ms: None,
            ended: None,
            last_active_ms: now,
            commands: 0,
        });
        if reopen.is_some() {
            let _ = self.index.set_reopened(number, now, window);
        }
        let (n, tz) = self.cx();
        let event = match &reopen {
            Some(r) => Event::Reopened { reason: r.reason.clone(), last_active_ms: r.last_active_ms },
            None => Event::Opened { by: tab.opened_by.clone() },
        };
        {
            let cx = Ctx { index: &self.index, config: &self.config, tz, now_ms: n };
            tab.add_event(event, &cx);
        }
        let _ = tab.log.flush();
        self.poller.add(tab.fd(), TAB_KEY_BASE + number as usize, false);
        self.tabs.insert(number, tab);
        if let Some(w) = self.windows.iter_mut().find(|w| w.number == window) {
            w.tabs.push(number);
        }
        self.broadcast_tabs();
        Ok(Opened { tab: Some(number), window, new_window: false, window_name: None, name, folder, not_restored })
    }

    fn shell_env(&self, tab: u64, window: u64, is_zsh: bool) -> Vec<(String, String)> {
        let keep = [
            "PATH",
            "HOME",
            "USER",
            "LOGNAME",
            "SHELL",
            "TMPDIR",
            "LANG",
            "SSH_AUTH_SOCK",
            "__CF_USER_TEXT_ENCODING",
            "CTX_HOME",
            "CTX_NO_DISCLAIM",
        ];
        let mut env: Vec<(String, String)> =
            std::env::vars().filter(|(k, _)| keep.contains(&k.as_str()) || k.starts_with("LC_")).collect();
        let set = |env: &mut Vec<(String, String)>, k: &str, v: String| {
            env.retain(|(ek, _)| ek != k);
            env.push((k.to_string(), v));
        };
        if !env.iter().any(|(k, _)| k == "LANG") {
            set(&mut env, "LANG", "en_US.UTF-8".into());
        }
        if !env.iter().any(|(k, _)| k == "PATH") {
            set(&mut env, "PATH", "/usr/bin:/bin:/usr/sbin:/sbin".into());
        }
        // Scoped launchers made with `ctx make launcher` run by name.
        let launchers = self.paths.launchers().join("bin").display().to_string();
        if let Some((_, p)) = env.iter_mut().find(|(k, _)| k == "PATH") {
            *p = format!("{p}:{launchers}");
        }
        set(&mut env, "CTX_LAUNCHERS", launchers);
        if self.config.ssh_integration {
            set(&mut env, "CTX_SSH_COMMAND", ssh_command());
        }
        if !self.config.ssh_integration_skip.is_empty() {
            set(&mut env, "CTX_SSH_SKIP", self.config.ssh_integration_skip.join(" "));
        }
        if !env.iter().any(|(k, _)| k == "HOME") {
            set(&mut env, "HOME", sys::home());
        }
        if !env.iter().any(|(k, _)| k == "USER") {
            set(&mut env, "USER", sys::username());
        }
        set(&mut env, "TERM", "xterm-256color".into());
        set(&mut env, "COLORTERM", "truecolor".into());
        set(&mut env, "TERM_PROGRAM", "ctx".into());
        set(&mut env, "TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION").into());
        set(&mut env, "CTX_TAB", tab.to_string());
        set(&mut env, "CTX_WINDOW", window.to_string());
        if is_zsh {
            if let Ok(z) = std::env::var("ZDOTDIR") {
                set(&mut env, "CTX_USER_ZDOTDIR", z);
            }
            set(&mut env, "ZDOTDIR", self.zdotdir.display().to_string());
            set(&mut env, "CTX_INTEGRATION", self.integration.display().to_string());
            set(&mut env, "CTX_COMPLETION", self.paths.runtime.join("_ctx").display().to_string());
        }
        env
    }

    // ----- closing -----

    fn close_tab(&mut self, key: usize, tab: &TabRef, without_asking: bool) -> Handled {
        let n = match self.resolve(key, tab) {
            Ok(n) => n,
            Err(e) => return Handled::Reply(Response::Error(e)),
        };
        if !without_asking && let Some(what) = self.running_description(n) {
            let t = &self.tabs[&n];
            return err(
                ErrorKind::NeedsConfirmation,
                format!(
                    "{} is running {what}. Close it anyway? `ctx close {} without asking`",
                    capital(&t.display_name()),
                    tab_ref_words(t)
                ),
            );
        }
        self.close_now(n);
        done(format!("Closed tab {n}."))
    }

    /// What's running in a tab, for the "asks first" rule.
    fn running_description(&self, n: u64) -> Option<String> {
        let t = self.tabs.get(&n)?;
        if let Some(r) = t.current() {
            return Some(format!("\"{}\"", r.text));
        }
        if !t.shell_in_front() {
            return t.foreground().map(|(_, name)| format!("\"{name}\""));
        }
        None
    }

    fn close_now(&mut self, n: u64) {
        if let Some(t) = self.tabs.get(&n) {
            if let Some(pg) = sys::foreground_pgid(t.fd()) {
                sys::kill_group(pg, libc::SIGHUP);
            }
            sys::kill(t.launch_pid, libc::SIGHUP);
        }
        self.end_tab(n, "closed");
    }

    /// The tab's shell is gone (or it was closed): finish its record and remove it.
    fn end_tab(&mut self, n: u64, reason: &str) {
        let Some(mut tab) = self.tabs.remove(&n) else { return };
        self.poller.delete(tab.fd());
        let (ms, tz) = self.cx();
        {
            let cx = Ctx { index: &self.index, config: &self.config, tz, now_ms: ms };
            // Commands still running end without an exit status.
            while !tab.running.is_empty() {
                let mut fx = Effects::default();
                let _ = &mut fx;
                tab.flush(ms);
                let cmd = tab.running.pop().unwrap();
                tab.log.push(&Record::CommandEnd {
                    number: cmd.number,
                    at_ms: ms,
                    exit_status: None,
                    dropped_lines: cmd.dropped_lines,
                });
            }
            tab.flush(ms);
            if reason == "closed" {
                tab.log.push(&Record::Event { at_ms: ms, event: Event::Closed });
            }
            let _ = cx;
        }
        tab.log.push(&Record::Closed { at_ms: ms, reason: reason.into() });
        let _ = tab.log.flush();
        let _ = self.index.set_closed(n, ms, reason);
        let _ = tz;
        for w in self.windows.iter_mut() {
            w.tabs.retain(|t| *t != n);
        }
        self.windows.retain(|w| !w.tabs.is_empty());
        // Waiters on this tab get their answer now.
        let mine: Vec<Waiter> = {
            let (a, b): (Vec<Waiter>, Vec<Waiter>) =
                std::mem::take(&mut self.waiters).into_iter().partition(|w| w.tab == n);
            self.waiters = b;
            a
        };
        for w in mine {
            let r = Response::Error(ErrorReply::new(ErrorKind::Failed, format!("Tab {n} closed while waiting.")));
            self.reply(w.client, r);
        }
        // Attached terminals are told.
        let attached: Vec<usize> =
            self.clients.iter().filter(|(_, c)| c.mode == Mode::Attached(n)).map(|(k, _)| *k).collect();
        for k in attached {
            self.send(k, &StreamOut::Ended { message: format!("Tab {n} ended.") });
            if let Some(c) = self.clients.get_mut(&k) {
                c.close_after_flush = true;
            }
        }
        self.broadcast_tabs();
    }

    fn close_window(&mut self, w: &WindowRef, without_asking: bool) -> Handled {
        let wn = match self.resolve_window(w) {
            Ok(w) => w,
            Err(e) => return Handled::Reply(Response::Error(e)),
        };
        let tabs = self.windows.iter().find(|x| x.number == wn).map(|x| x.tabs.clone()).unwrap_or_default();
        if !without_asking {
            let busy: Vec<String> = tabs
                .iter()
                .filter_map(|n| self.running_description(*n).map(|d| format!("tab {n} is running {d}")))
                .collect();
            if !busy.is_empty() {
                return err(
                    ErrorKind::NeedsConfirmation,
                    format!(
                        "In window {wn}, {}. Close it anyway? `ctx close window {wn} without asking`",
                        busy.join(", and ")
                    ),
                );
            }
        }
        for n in tabs {
            self.close_now(n);
        }
        self.windows.retain(|x| x.number != wn);
        done(format!("Closed window {wn}."))
    }

    fn rename_tab(&mut self, key: usize, tab: &TabRef, to: &str) -> Handled {
        let n = match self.resolve(key, tab) {
            Ok(n) => n,
            Err(e) => return Handled::Reply(Response::Error(e)),
        };
        let to = match self.validate_name(to, Some(n), false) {
            Ok(t) => t,
            Err(e) => return Handled::Reply(Response::Error(e)),
        };
        let (ms, tz) = self.cx();
        let cx = Ctx { index: &self.index, config: &self.config, tz, now_ms: ms };
        let t = self.tabs.get_mut(&n).unwrap();
        let from = t.name.replace(to.clone());
        t.log.push(&Record::Name { at_ms: ms, name: Some(to.clone()) });
        t.add_event(Event::Renamed { from, to: to.clone() }, &cx);
        let _ = t.log.flush();
        let _ = self.index.set_name(n, Some(&to));
        self.broadcast_tabs();
        done(format!("Tab {n} is now named \"{to}\"."))
    }

    fn rename_window(&mut self, w: &WindowRef, to: &str) -> Handled {
        let wn = match self.resolve_window(w) {
            Ok(w) => w,
            Err(e) => return Handled::Reply(Response::Error(e)),
        };
        let to = match self.validate_name(to, None, true) {
            Ok(t) => t,
            Err(e) => return Handled::Reply(Response::Error(e)),
        };
        if self.windows.iter().any(|x| x.number != wn && x.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(&to)))
        {
            return err(ErrorKind::NotAllowed, format!("Another window is already named \"{to}\"."));
        }
        if let Some(x) = self.windows.iter_mut().find(|x| x.number == wn) {
            x.name = Some(to.clone());
        }
        self.broadcast_tabs();
        done(format!("Window {wn} is now named \"{to}\"."))
    }

    fn move_tab(&mut self, key: usize, tab: &TabRef, to: &WindowRef, position: Option<u32>) -> Handled {
        let n = match self.resolve(key, tab) {
            Ok(n) => n,
            Err(e) => return Handled::Reply(Response::Error(e)),
        };
        let wn = match self.resolve_window(to) {
            Ok(w) => w,
            Err(e) => return Handled::Reply(Response::Error(e)),
        };
        for w in self.windows.iter_mut() {
            w.tabs.retain(|t| *t != n);
        }
        let mut wname = None;
        if let Some(w) = self.windows.iter_mut().find(|x| x.number == wn) {
            let pos = position.map(|p| (p.max(1) as usize - 1).min(w.tabs.len())).unwrap_or(w.tabs.len());
            w.tabs.insert(pos, n);
            wname = w.name.clone();
        }
        self.windows.retain(|w| !w.tabs.is_empty());
        let (ms, tz) = self.cx();
        let cx = Ctx { index: &self.index, config: &self.config, tz, now_ms: ms };
        if let Some(t) = self.tabs.get_mut(&n) {
            t.window = wn;
            t.log.push(&Record::Window { at_ms: ms, window: wn });
            t.add_event(Event::Moved { window: wn, window_name: wname }, &cx);
            let _ = t.log.flush();
        }
        let _ = self.index.set_window(n, wn);
        self.broadcast_tabs();
        done(format!("Moved tab {n} to window {wn}."))
    }

    fn clear(&mut self, key: usize, tab: &TabRef) -> Handled {
        let n = match self.resolve(key, tab) {
            Ok(n) => n,
            Err(e) => return Handled::Reply(Response::Error(e)),
        };
        let t = self.tabs.get_mut(&n).unwrap();
        t.flush(now_ms());
        t.term.clear_all();
        t.cold.clear();
        t.prompt_pin = None;
        if t.at_prompt && t.shell_in_front() {
            // Let the shell draw its prompt again at the top.
            t.to_pty.extend_from_slice(b"\x0c");
        }
        self.flush_tab(n);
        self.dirty.insert(n);
        done(format!("Cleared tab {n}. The saved record keeps everything."))
    }

    fn forget(&mut self, key: usize, n: u64, without_asking: bool) -> Handled {
        if self.tabs.contains_key(&n) {
            return err(ErrorKind::NotAllowed, format!("Tab {n} is open. Close it first: `ctx close tab {n}`."));
        }
        if self.index.tab(n).ok().flatten().is_none() && !self.paths.tab_log(n).exists() {
            return err(ErrorKind::NoSuchTab, format!("There is no saved tab {n}. `ctx list closed tabs` shows them."));
        }
        let interactive = self.clients.get(&key).and_then(|c| c.hello.as_ref()).is_some_and(|h| h.interactive);
        if !without_asking {
            let _ = interactive;
            return err(
                ErrorKind::NeedsConfirmation,
                format!(
                    "This permanently deletes tab {n}'s saved record. Delete it? `ctx forget tab {n} without asking`"
                ),
            );
        }
        let _ = std::fs::remove_file(self.paths.tab_log(n));
        let _ = std::fs::remove_file(self.paths.tab_history(n));
        let _ = self.index.forget(n);
        done(format!("Forgot tab {n}. Its saved record is deleted."))
    }

    // ----- reopening -----

    fn reopen_tab(&mut self, n: u64, window: u64, reason: Option<String>, by: &str) -> Result<Opened, ErrorReply> {
        if self.tabs.contains_key(&n) {
            return Err(ErrorReply::new(ErrorKind::NotAllowed, format!("Tab {n} is already open.")));
        }
        let path = self.paths.tab_log(n);
        if !path.exists() {
            return Err(ErrorReply::new(
                ErrorKind::NoSuchTab,
                format!("There is no saved tab {n}. `ctx list closed tabs` shows them."),
            ));
        }
        let h = History::load(&path, self.tz)
            .map_err(|e| ErrorReply::new(ErrorKind::Failed, format!("couldn't read tab {n}'s record: {e}")))?;
        // A name now used by an open tab isn't taken back.
        let name = h
            .name
            .clone()
            .filter(|nm| !self.tabs.values().any(|t| t.name.as_deref().is_some_and(|o| o.eq_ignore_ascii_case(nm))));
        // The tab's own commands become its up-arrow history.
        let hist_path = self.paths.tab_history(n);
        let mut hist = String::new();
        for c in &h.commands {
            if !c.text.is_empty() {
                hist.push_str(&format!(": {}:0;{}\n", c.started_ms / 1000, c.text.replace('\n', "\\\n")));
            }
        }
        let history_file = if hist.is_empty() {
            None
        } else {
            let _ = std::fs::write(&hist_path, hist);
            set_mode(&hist_path, 0o600);
            Some(hist_path.display().to_string())
        };
        let prefill = h
            .unfinished
            .and_then(|u| h.command(u))
            .map(|c| c.text.clone())
            .filter(|t| !t.is_empty())
            .or_else(|| h.ssh_target.as_ref().map(|t| format!("ssh {t}")));
        let boot = sys::boot_time_secs() * 1000;
        let reason = reason.or_else(|| {
            if h.ended.as_deref() == Some("interrupted") {
                Some(if h.last_active_ms < boot { "reboot".into() } else { "server restart".into() })
            } else {
                None
            }
        });
        let mut transcript = h.transcript.clone();
        // Unfinished commands show what they printed before the tab ended.
        while transcript.last().is_some_and(|l| plain_line(l).trim().is_empty()) {
            transcript.pop();
        }
        let r = Reopening {
            env: h.env.clone(),
            transcript,
            reason,
            last_active_ms: h.last_active_ms,
            history_file,
            prefill,
            next_command: h.next_command_number(),
            opened_by: h.opened_by.clone(),
            opened_ms: h.opened_ms,
            columns: h.columns,
            rows: h.rows,
        };
        let _ = by;
        self.spawn_tab(Some(n), window, &h.folder, name, by, Some(r))
    }

    // ----- doing things -----

    fn client_ran_by(&self, key: usize) -> String {
        self.clients
            .get(&key)
            .map(|c| if c.identity.is_app { "you".into() } else { c.identity.ran_by() })
            .unwrap_or_else(|| "you".into())
    }

    #[allow(clippy::too_many_arguments)]
    fn run_command(
        &mut self,
        key: usize,
        tab: &TabRef,
        command: String,
        colors: bool,
        max_lines: Option<u32>,
        within_ms: Option<u64>,
        wait: bool,
    ) -> Handled {
        let n = match self.resolve(key, tab) {
            Ok(n) => n,
            Err(e) => return Handled::Reply(Response::Error(e)),
        };
        if command.trim().is_empty() {
            return err(
                ErrorKind::NotAllowed,
                "There's no command to run. Put it in quotes: ctx run in \"<tab>\" \"<command>\"",
            );
        }
        let by = self.client_ran_by(key);
        let pending = self.waiters.iter().any(|w| w.tab == n && matches!(w.kind, WaitKind::Run { number: None, .. }));
        let t = self.tabs.get_mut(&n).unwrap();
        let tab_words = tab_ref_words(t);
        if !t.integration {
            return err(
                ErrorKind::Unsupported,
                format!(
                    "{} has no shell-integration marks (it's inside ssh without ctx's remote integration, or its shell isn't zsh), so ctx can't tell where a command's output ends. Use: ctx type {} into {tab_words} then press enter, then: ctx wait until {tab_words} is quiet for 2 seconds",
                    capital(&t.display_name()),
                    quote(&command)
                ),
            );
        }
        let at_remote_prompt = t.at_prompt && t.ssh_target.is_some();
        if pending
            || !t.at_prompt
            || (!t.running.is_empty() && !at_remote_prompt)
            || (!at_remote_prompt && !t.shell_in_front())
        {
            let what = t
                .current()
                .map(|r| r.text.clone())
                .filter(|s| !s.is_empty())
                .or_else(|| t.foreground().map(|f| f.1))
                .unwrap_or_else(|| "a command".into());
            if t.term.full_screen() || (!t.shell_in_front() && t.running.is_empty()) {
                return err(
                    ErrorKind::Busy,
                    format!(
                        "{} has \"{what}\" in front, not the shell. To talk to it, use: ctx type \"…\" into {tab_words}",
                        capital(&t.display_name())
                    ),
                );
            }
            return err(
                ErrorKind::Busy,
                format!(
                    "{} is running \"{what}\"; open another tab or use: ctx wait until {tab_words} is idle",
                    capital(&t.display_name())
                ),
            );
        }
        if t.typing {
            return err(
                ErrorKind::SomeoneTyping,
                format!(
                    "Someone is typing in {}. ctx won't add to a half-typed command; try again when the prompt is empty.",
                    t.display_name()
                ),
            );
        }
        t.contributors.push(by);
        let mut bytes = t.term.encode_paste(&command);
        bytes.push(b'\r');
        t.to_pty.extend_from_slice(&bytes);
        t.unread_lines = 0;
        let max = max_lines.unwrap_or(self.config.run_max_lines) as usize;
        self.waiters.push(Waiter {
            client: key,
            tab: n,
            kind: WaitKind::Run { command, number: None, max_lines: max, colors, wait, sent_at: Instant::now() },
            deadline: within_ms.map(|ms| Instant::now() + Duration::from_millis(ms)),
            started: Instant::now(),
        });
        self.flush_tab(n);
        Handled::Deferred
    }

    fn type_text(&mut self, key: usize, tab: &TabRef, text: &str, then_press: Option<String>) -> Handled {
        let n = match self.resolve(key, tab) {
            Ok(n) => n,
            Err(e) => return Handled::Reply(Response::Error(e)),
        };
        let by = self.client_ran_by(key);
        let t = self.tabs.get_mut(&n).unwrap();
        let enter = then_press.as_deref().is_some_and(|k| k == "enter" || k == "return");
        if enter && t.integration && t.at_prompt && t.running.is_empty() && t.shell_in_front() {
            return err(
                ErrorKind::NotAllowed,
                format!("To run a shell command, use: ctx run in {} {}", tab_ref_words(t), quote(text)),
            );
        }
        // Typed exactly as keystrokes; multi-line text goes as a bracketed paste (when the
        // program asked for that) so it doesn't run line by line.
        let mut bytes = if text.contains('\n') { t.term.encode_paste(text) } else { text.as_bytes().to_vec() };
        if let Some(k) = &then_press {
            match t.term.encode_key(k) {
                Some(b) => bytes.extend(b),
                None => return err(ErrorKind::NotAllowed, format!("\"{k}\" isn't a key ctx knows.")),
            }
        }
        t.contributors.push(by);
        if enter && t.in_ssh() && !t.remote_marks {
            // Recording from the screen: let the remote echo the text before Enter is noted.
            let enter_len = t.term.encode_key("enter").map(|b| b.len()).unwrap_or(1);
            let (text_bytes, enter_bytes) = bytes.split_at(bytes.len() - enter_len);
            t.to_pty.extend_from_slice(text_bytes);
            t.pending_enter = Some((enter_bytes.to_vec(), text.to_string(), Instant::now()));
        } else {
            t.to_pty.extend_from_slice(&bytes);
        }
        self.flush_tab(n);
        done(format!(
            "Typed into tab {n}{}.",
            match &then_press {
                Some(k) => format!(" and pressed {k}"),
                None => String::new(),
            }
        ))
    }

    fn press(&mut self, key: usize, tab: &TabRef, keys: &[String]) -> Handled {
        let n = match self.resolve(key, tab) {
            Ok(n) => n,
            Err(e) => return Handled::Reply(Response::Error(e)),
        };
        let by = self.client_ran_by(key);
        let t = self.tabs.get_mut(&n).unwrap();
        let mut bytes = Vec::new();
        for k in keys {
            match t.term.encode_key(k) {
                Some(b) => bytes.extend(b),
                None => {
                    return err(
                        ErrorKind::NotAllowed,
                        format!("\"{k}\" isn't a key ctx knows. Keys are named like enter, escape, up, ctrl-c, f5."),
                    );
                }
            }
        }
        let has_enter = keys.iter().any(|k| k == "enter" || k == "return");
        if has_enter {
            t.contributors.push(by);
            self.enter_pressed(n);
        }
        let Some(t) = self.tabs.get_mut(&n) else { return done("") };
        t.to_pty.extend_from_slice(&bytes);
        self.flush_tab(n);
        done(format!("Pressed {} in tab {n}.", keys.join(" then ")))
    }

    fn wait(&mut self, key: usize, tab: &TabRef, until: WaitCondition, within_ms: u64) -> Handled {
        let n = match self.resolve(key, tab) {
            Ok(n) => n,
            Err(e) => return Handled::Reply(Response::Error(e)),
        };
        let t = &self.tabs[&n];
        let kind = match until {
            WaitCondition::Idle => {
                if !t.integration && t.shell_in_front() {
                    return ok(Output::Waited(WaitResult {
                        tab: n,
                        tab_name: t.name.clone(),
                        condition: "idle".into(),
                        exit_status: t.last_exit,
                        line: None,
                        waited_seconds: 0.0,
                    }));
                }
                if !t.integration {
                    return err(
                        ErrorKind::Unsupported,
                        format!(
                            "{} has no shell-integration marks, so ctx can't tell when a command finishes. Use: ctx wait until {} is quiet for 2 seconds",
                            capital(&t.display_name()),
                            tab_ref_words(t)
                        ),
                    );
                }
                if !t.busy()
                    && !self.waiters.iter().any(|w| w.tab == n && matches!(w.kind, WaitKind::Run { number: None, .. }))
                {
                    return ok(Output::Waited(WaitResult {
                        tab: n,
                        tab_name: t.name.clone(),
                        condition: "idle".into(),
                        exit_status: t.last_exit,
                        line: None,
                        waited_seconds: 0.0,
                    }));
                }
                WaitKind::Idle
            }
            WaitCondition::Shows(text) => {
                // A running command's output so far counts; when idle, only new output does.
                if let Some(cmd) = t.current() {
                    let (lines, _) = cmd.kept_output();
                    let live = t.live_rows(false);
                    let needle = text.to_lowercase();
                    if let Some(l) =
                        lines.iter().map(|l| plain_line(l)).chain(live).find(|l| l.to_lowercase().contains(&needle))
                    {
                        return ok(Output::Waited(WaitResult {
                            tab: n,
                            tab_name: t.name.clone(),
                            condition: "shows".into(),
                            exit_status: None,
                            line: Some(l),
                            waited_seconds: 0.0,
                        }));
                    }
                }
                WaitKind::Shows { text, seen: t.output_seq }
            }
            WaitCondition::QuietFor(ms) => WaitKind::Quiet { ms },
        };
        self.waiters.push(Waiter {
            client: key,
            tab: n,
            kind,
            deadline: Some(Instant::now() + Duration::from_millis(within_ms)),
            started: Instant::now(),
        });
        Handled::Deferred
    }

    fn show(&mut self, key: usize, tab: &TabRef) -> Handled {
        let n = match self.resolve(key, tab) {
            Ok(n) => n,
            Err(e) => return Handled::Reply(Response::Error(e)),
        };
        let apps: Vec<usize> =
            self.clients.iter().filter(|(_, c)| matches!(c.mode, Mode::App(_))).map(|(k, _)| *k).collect();
        if let Some(t) = self.tabs.get_mut(&n) {
            t.unread_lines = 0;
        }
        if apps.is_empty() {
            self.pending_show = Some(n);
            let app = std::path::Path::new("/Applications/CTX.app");
            if app.exists() {
                let _ = std::process::Command::new("/usr/bin/open").arg("-a").arg(app).spawn();
                return done(format!("Opening CTX.app at tab {n}."));
            }
            return err(
                ErrorKind::Unsupported,
                format!("CTX.app isn't running or installed. To use tab {n} from this terminal: ctx attach to tab {n}"),
            );
        }
        for k in apps {
            self.send(k, &StreamOut::Show { tab: n });
        }
        done(format!("Showing tab {n} in CTX.app."))
    }

    fn records(&mut self, key: usize, tab: &TabRef) -> Handled {
        let n = match self.resolve_any(key, tab) {
            Ok(n) => n,
            Err(e) => return Handled::Reply(Response::Error(e)),
        };
        let commands = self.command_records(n, None, false, usize::MAX, false);
        ok(Output::Records(CommandOutputs { tab: n, tab_name: self.tab_name(n), commands }))
    }

    fn matching_tabs(&mut self, key: usize, filter: Filter, limit: u32) -> Handled {
        let tab = match &filter.tab {
            Some(t) => match self.resolve_any(key, t) {
                Ok(n) => Some(n),
                Err(e) => return Handled::Reply(Response::Error(e)),
            },
            None => None,
        };
        let narrow = Narrow { tab, folder: filter.folder, by: filter.by, since_ms: filter.since.map(|s| s * 1000) };
        let tabs: Vec<u64> = self
            .index
            .candidate_tabs(&narrow)
            .unwrap_or_default()
            .into_iter()
            .map(|t| t.number)
            .take(limit as usize)
            .collect();
        ok(Output::TabNumbers(tabs))
    }

    fn who_ran(&self, pid: i32) -> Handled {
        // The tab whose process tree contains this pid.
        let chain = sys::ancestors(pid);
        let tab = chain.iter().find_map(|p| self.tab_of_pid(p.pid));
        let Some(n) = tab else {
            return ok(Output::RanBy(RanBy { tab: None, ran_by: "unknown".into(), agent_involved: false }));
        };
        let t = &self.tabs[&n];
        let ran_by = t.current().map(|r| r.ran_by.clone()).unwrap_or_else(|| {
            let mut by = t.contributors.clone();
            by.dedup();
            if by.is_empty() { "you".into() } else { by.join(" and ") }
        });
        let agent_involved = ran_by.split(" and ").any(|p| p != "you");
        ok(Output::RanBy(RanBy { tab: Some(n), ran_by, agent_involved }))
    }

    fn restart_request(&mut self, without_asking: bool) -> Handled {
        if !without_asking {
            let busy: Vec<String> = self
                .tabs
                .keys()
                .filter_map(|n| self.running_description(*n).map(|d| format!("tab {n} is running {d}")))
                .collect();
            if !busy.is_empty() {
                return err(
                    ErrorKind::NeedsConfirmation,
                    format!(
                        "Restarting stops what's running: {}. Open tabs come back afterwards, with their output and folders. Restart anyway? `ctx restart server without asking`",
                        busy.join(", ")
                    ),
                );
            }
        }
        self.restart = true;
        self.quit = true;
        let n = self.tabs.len();
        done(if n == 0 {
            "Restarting the server.".to_string()
        } else {
            format!("Restarting the server. The {n} open tab{} will be reopened.", if n == 1 { "" } else { "s" })
        })
    }

    // ----- read -----

    fn read(&mut self, key: usize, tab: &TabRef, what: ReadWhat, colors: bool, max_lines: Option<u32>) -> Handled {
        let n = match self.resolve_any(key, tab) {
            Ok(n) => n,
            Err(e) => return Handled::Reply(Response::Error(e)),
        };
        let name = self.tab_name(n);
        let words = match &name {
            Some(nm) => quote(nm),
            None => format!("tab {n}"),
        };
        match what {
            ReadWhat::Screen | ReadWhat::LastLines(_) | ReadWhat::Everything => {
                let (lines, total) = if let Some(t) = self.tabs.get_mut(&n) {
                    t.unread_lines = 0;
                    match what {
                        ReadWhat::Screen => {
                            let s = t.screen_text(colors);
                            let len = s.len() as u64;
                            (s, len)
                        }
                        ReadWhat::LastLines(k) => t.last_lines(k as usize, true),
                        _ => t.last_lines(usize::MAX, true),
                    }
                } else {
                    let h = match History::load(&self.paths.tab_log(n), self.tz) {
                        Ok(h) => h,
                        Err(e) => return err(ErrorKind::Failed, format!("couldn't read tab {n}'s record: {e}")),
                    };
                    let total = h.transcript.len() as u64;
                    let take = match what {
                        ReadWhat::Screen => h.rows.max(24) as usize,
                        ReadWhat::LastLines(k) => k as usize,
                        _ => usize::MAX,
                    };
                    let skip = h.transcript.len().saturating_sub(take);
                    (h.transcript[skip..].to_vec(), total)
                };
                let shown = lines.len() as u64;
                let (lines, note) =
                    cap_lines(lines, max_lines.map(|m| m as usize), &format!("ctx read everything in {words}"));
                let (lines, events) = present(lines, colors);
                let note = note.or_else(|| {
                    if matches!(what, ReadWhat::LastLines(_)) && total > shown {
                        Some(format!(
                            "showing the last {} of {} lines; ctx read everything in {words} for all",
                            thousands(shown),
                            thousands(total)
                        ))
                    } else {
                        None
                    }
                });
                ok(Output::Text(TextOutput { tab: n, tab_name: name, lines, events, total_lines: total, note }))
            }
            ReadWhat::LastCommands(k) => {
                let max = max_lines.unwrap_or(self.config.run_max_lines) as usize;
                let commands = self.command_records(n, Some(k as usize), colors, max, true);
                if commands.is_empty() {
                    return err(
                        ErrorKind::NoSuchTab,
                        format!(
                            "No commands have run in tab {n} yet{}.",
                            if self.tabs.get(&n).is_some_and(|t| !t.integration) {
                                " that ctx could see (no shell-integration marks in this tab)"
                            } else {
                                ""
                            }
                        ),
                    );
                }
                ok(Output::CommandsWithOutput(CommandOutputs { tab: n, tab_name: name, commands }))
            }
            ReadWhat::Command(c) => {
                let max = max_lines.unwrap_or(self.config.run_max_lines) as usize;
                let all = self.command_records(n, None, colors, max, true);
                match all.into_iter().find(|r| r.number == c) {
                    Some(r) => {
                        ok(Output::CommandsWithOutput(CommandOutputs { tab: n, tab_name: name, commands: vec![r] }))
                    }
                    None => err(
                        ErrorKind::NoSuchTab,
                        format!("Tab {n} has no command {c}. `ctx list commands in tab {n}` lists them."),
                    ),
                }
            }
        }
    }

    /// Commands with their outputs: from the record on disk, plus whatever is running now.
    /// `last`: only the last N. Outputs longer than `max` lines are cut to head and tail.
    fn command_records(
        &mut self,
        n: u64,
        last: Option<usize>,
        colors: bool,
        max: usize,
        events: bool,
    ) -> Vec<CommandRecord> {
        if let Some(t) = self.tabs.get_mut(&n) {
            t.flush(now_ms());
        }
        let Ok(h) = History::load(&self.paths.tab_log(n), self.tz) else { return Vec::new() };
        let tab_words = match self.tab_name(n) {
            Some(nm) => quote(&nm),
            None => format!("tab {n}"),
        };
        let mut out: Vec<CommandRecord> = Vec::new();
        let running_now: Vec<u64> =
            self.tabs.get(&n).map(|t| t.running.iter().map(|r| r.number).collect()).unwrap_or_default();
        let skip = last.map(|l| h.commands.len().saturating_sub(l)).unwrap_or(0);
        for c in h.commands.iter().skip(skip) {
            let mut output: Vec<String> = c.output.clone();
            // Live rows of the command running now.
            if running_now.contains(&c.number)
                && let Some(t) = self.tabs.get(&n)
                && t.current().is_some_and(|r| r.number == c.number)
            {
                if let Some(r) = t.current() {
                    output.extend(r.saved_tail.lines());
                }
                output.extend(t.live_rows(true));
            }
            let total = output.iter().filter(|l| !l.starts_with(EVENT_MARK)).count() as u64;
            let raw: Vec<String> = output.into_iter().filter(|l| events || !l.starts_with(EVENT_MARK)).collect();
            let (raw, mut note) = cap_lines(raw, Some(max), &format!("ctx read command {} in {tab_words}", c.number));
            let (shown, event_lines) = present(raw, colors);
            if c.dropped_lines > 0 {
                let extra = format!(
                    "{} lines in the middle weren't saved (the per-command limit is {} MB)",
                    thousands(c.dropped_lines),
                    self.config.saved_output_mb_per_command
                );
                note = Some(match note {
                    Some(n) => format!("{n}; {extra}"),
                    None => extra,
                });
            }
            out.push(CommandRecord {
                number: c.number,
                command: c.text.clone(),
                location: c.location.clone(),
                ran_by: c.ran_by.clone(),
                exit_status: c.exit_status,
                started_at: iso_utc(c.started_ms),
                finished_at: c.finished_ms.map(iso_utc),
                took_seconds: c.finished_ms.map(|f| (f - c.started_ms) as f64 / 1000.0),
                approximate: c.approximate,
                full_screen: c.full_screen,
                output: shown,
                events: event_lines,
                output_lines: total,
                note,
            });
        }
        out
    }

    // ----- find -----

    fn find(&mut self, key: usize, text: &str, filter: Filter, limit: u32) -> Handled {
        if text.trim().is_empty() {
            return err(ErrorKind::NotAllowed, "Find what? ctx find \"<text>\"");
        }
        let tab = match &filter.tab {
            Some(t) => match self.resolve_any(key, t) {
                Ok(n) => Some(n),
                Err(e) => return Handled::Reply(Response::Error(e)),
            },
            None => None,
        };
        let ms = now_ms();
        for t in self.tabs.values_mut() {
            t.flush(ms);
        }
        let narrow = Narrow {
            tab,
            folder: filter.folder.clone(),
            by: filter.by.clone(),
            since_ms: filter.since.map(|s| s * 1000),
        };
        let needle = text.to_lowercase();
        let limit = limit.max(1) as usize;
        let mut hits: Vec<(i64, FindHit)> = Vec::new();
        let mut total = 0u64;
        // Names.
        if narrow.by.is_none() && narrow.folder.is_none() {
            for row in self.index.search_names(text, 50).unwrap_or_default() {
                if tab.is_some_and(|t| t != row.number) || narrow.since_ms.is_some_and(|s| row.last_active_ms < s) {
                    continue;
                }
                total += 1;
                hits.push((
                    row.last_active_ms,
                    FindHit {
                        tab: row.number,
                        tab_name: row.name.clone(),
                        found_in: "name".into(),
                        command: None,
                        command_text: None,
                        line: None,
                        ran_by: None,
                        at: iso_utc(row.last_active_ms),
                        text: row.name.clone().unwrap_or_default(),
                        before: vec![],
                        after: vec![],
                    },
                ));
            }
        }
        // Command text.
        for c in self.index.search_commands(text, &narrow, 200).unwrap_or_default() {
            total += 1;
            hits.push((
                c.started_ms,
                FindHit {
                    tab: c.tab,
                    tab_name: self.tab_name(c.tab),
                    found_in: "command".into(),
                    command: Some(c.number),
                    command_text: Some(c.text.clone()),
                    line: None,
                    ran_by: Some(c.ran_by.clone()),
                    at: iso_utc(c.started_ms),
                    text: format!("{}$ {}", c.location, c.text),
                    before: vec![],
                    after: vec![],
                },
            ));
        }
        // Output, newest tabs first.
        let by_pat = narrow.by.as_ref().map(|b| ctx_store::index::by_pattern(b));
        for row in self.index.candidate_tabs(&narrow).unwrap_or_default() {
            let cmds: HashMap<u64, ctx_store::index::CommandRow> =
                self.index.commands(row.number).unwrap_or_default().into_iter().map(|c| (c.number, c)).collect();
            let mut tab_hits: Vec<(i64, FindHit)> = Vec::new();
            let path = self.paths.tab_log(row.number);
            let _ = search_lines(&path, &needle, |h| {
                let cmd = h.command.and_then(|c| cmds.get(&c));
                if let Some(p) = &by_pat
                    && !cmd.is_some_and(|c| c.ran_by.to_lowercase().contains(p.as_str()))
                {
                    return true;
                }
                if let Some(f) = &narrow.folder
                    && !cmd.is_some_and(|c| {
                        c.folder == *f || c.folder.starts_with(&format!("{}/", f.trim_end_matches('/')))
                    })
                {
                    return true;
                }
                let at = cmd.map(|c| c.started_ms).unwrap_or(row.last_active_ms);
                if narrow.since_ms.is_some_and(|s| at < s) {
                    return true;
                }
                tab_hits.push((
                    at,
                    FindHit {
                        tab: row.number,
                        tab_name: row.name.clone(),
                        found_in: "output".into(),
                        command: h.command,
                        command_text: cmd.map(|c| c.text.clone()),
                        line: Some(h.line),
                        ran_by: cmd.map(|c| c.ran_by.clone()),
                        at: iso_utc(at),
                        text: h.text,
                        before: h.before,
                        after: h.after,
                    },
                ));
                true
            });
            total += tab_hits.len() as u64;
            hits.extend(tab_hits);
            if hits.len() > limit * 20 {
                break;
            }
        }
        hits.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.line.cmp(&a.1.line)));
        let shown = hits.len().min(limit);
        let note = if total as usize > shown {
            Some(format!(
                "showing the newest {} of {} matches; narrow with in \"<tab>\", at <folder>, by <who> or since <time>, or say up to {} results",
                shown,
                thousands(total),
                (limit * 5).min(1000)
            ))
        } else {
            None
        };
        ok(Output::Found(FindResults {
            text: text.to_string(),
            hits: hits.into_iter().take(limit).map(|(_, h)| h).collect(),
            total,
            note,
        }))
    }

    // ----- attach and CTX.app -----

    fn attach(&mut self, key: usize, tab: &TabRef, columns: u16, rows: u16) -> Handled {
        let n = match self.resolve(key, tab) {
            Ok(n) => n,
            Err(e) => return Handled::Reply(Response::Error(e)),
        };
        let name = self.tab_name(n);
        self.reply(key, Response::Ok(Output::Attached(Attached { tab: n, tab_name: name })));
        let ms = now_ms();
        let redraw = {
            let t = self.tabs.get_mut(&n).unwrap();
            t.viewers += 1;
            t.unread_lines = 0;
            t.resize(columns.max(20), rows.max(5), ms);
            redraw_bytes(t.term.as_ref())
        };
        if let Some(c) = self.clients.get_mut(&key) {
            c.mode = Mode::Attached(n);
        }
        self.send(key, &StreamOut::Bytes(redraw));
        Handled::Deferred
    }

    /// Send a held-back Enter once the typed text is on screen, or the output has paused for
    /// 100 ms (no echo, e.g. a password prompt), or after 600 ms.
    fn release_enter(&mut self, n: u64, _timer: bool) {
        let due = match self.tabs.get(&n) {
            Some(t) => match &t.pending_enter {
                Some((_, text, at)) => {
                    let echoed = t.cursor_line().ends_with(text.trim_end());
                    let quiet = now_ms() - t.last_output_ms >= 100 && at.elapsed() >= Duration::from_millis(100);
                    echoed || quiet || at.elapsed() >= Duration::from_millis(600)
                }
                None => false,
            },
            None => false,
        };
        if !due {
            return;
        }
        let Some((bytes, _, _)) = self.tabs.get_mut(&n).and_then(|t| t.pending_enter.take()) else { return };
        self.enter_pressed(n);
        if let Some(t) = self.tabs.get_mut(&n) {
            t.to_pty.extend_from_slice(&bytes);
        }
        self.flush_tab(n);
    }

    /// Enter was pressed in tab `n` (before the bytes reach the program).
    fn enter_pressed(&mut self, n: u64) {
        let (ms, tz) = self.cx();
        let cx = Ctx { index: &self.index, config: &self.config, tz, now_ms: ms };
        if let Some(t) = self.tabs.get_mut(&n) {
            t.note_enter(&cx);
        }
    }

    fn on_attached_input(&mut self, key: usize, n: u64, msg: StreamIn) {
        match msg {
            StreamIn::Bytes(b) => {
                if b.contains(&b'\r') {
                    self.enter_pressed(n);
                }
                if let Some(t) = self.tabs.get_mut(&n) {
                    if b.contains(&b'\r') || b.contains(&b'\n') || !b.is_empty() {
                        t.contributors.push("you".into());
                    }
                    t.to_pty.extend_from_slice(&b);
                }
                self.flush_tab(n);
            }
            StreamIn::Resize { columns, rows } => {
                if let Some(t) = self.tabs.get_mut(&n) {
                    t.resize(columns.max(20), rows.max(5), now_ms());
                }
            }
            StreamIn::Detach => {
                if let Some(t) = self.tabs.get_mut(&n) {
                    t.viewers = t.viewers.saturating_sub(1);
                }
                if let Some(c) = self.clients.get_mut(&key) {
                    c.mode = Mode::Requests;
                    c.close_after_flush = true;
                }
            }
            StreamIn::Request(_) => {}
        }
    }

    fn subscribe(&mut self, key: usize, tabs: Vec<u64>) -> Handled {
        let old = match self.clients.get(&key).map(|c| c.mode.clone()) {
            Some(Mode::App(old)) => old,
            _ => Vec::new(),
        };
        for t in &old {
            if let Some(tab) = self.tabs.get_mut(t) {
                tab.viewers = tab.viewers.saturating_sub(1);
            }
        }
        for t in &tabs {
            if let Some(tab) = self.tabs.get_mut(t) {
                tab.viewers += 1;
                tab.unread_lines = 0;
            }
            self.dirty.insert(*t);
        }
        if let Some(c) = self.clients.get_mut(&key) {
            c.mode = Mode::App(tabs.clone());
        }
        // First update of each subscribed tab is a full screen.
        for t in &tabs {
            if let Some(update) = self.tabs.get_mut(t).and_then(|tab| crate::screen::update(tab, true)) {
                self.send(key, &StreamOut::Screen(update));
            }
        }
        if let Some(n) = self.pending_show.take() {
            self.send(key, &StreamOut::Show { tab: n });
        }
        let list = self.tab_list();
        self.send(key, &StreamOut::TabsChanged(list));
        done("Subscribed.")
    }

    fn app_input(&mut self, _key: usize, n: u64, input: AppInput) -> Handled {
        let Some(t) = self.tabs.get_mut(&n) else {
            return err(ErrorKind::NoSuchTab, format!("Tab {n} isn't open."));
        };
        match input {
            AppInput::Key { key, text } if key == "enter" || key == "return" => {
                let bytes = t.term.encode_key(&key).or_else(|| text.map(|s| s.into_bytes())).unwrap_or_default();
                t.contributors.push("you".into());
                self.enter_pressed(n);
                if let Some(t) = self.tabs.get_mut(&n) {
                    t.to_pty.extend_from_slice(&bytes);
                }
            }
            AppInput::Key { key, text } => {
                let bytes = t.term.encode_key(&key).or_else(|| text.map(|s| s.into_bytes())).unwrap_or_default();
                if key == "enter" || key == "return" || !bytes.is_empty() {
                    t.contributors.push("you".into());
                }
                t.to_pty.extend_from_slice(&bytes);
            }
            AppInput::Text(s) => {
                t.contributors.push("you".into());
                t.to_pty.extend_from_slice(s.as_bytes());
            }
            AppInput::Paste(s) => {
                t.contributors.push("you".into());
                let b = t.term.encode_paste(&s);
                t.to_pty.extend_from_slice(&b);
            }
            AppInput::Resize { columns, rows } => t.resize(columns.max(20), rows.max(5), now_ms()),
            AppInput::Focus(f) => {
                if t.term.focus_reporting() {
                    t.to_pty.extend_from_slice(if f { b"\x1b[I" } else { b"\x1b[O" });
                }
            }
            AppInput::Raw(b) => t.to_pty.extend_from_slice(&b),
            AppInput::Scroll(_) => {}
        }
        self.flush_tab(n);
        self.dirty.insert(n);
        Handled::Reply(Response::Ok(Output::Done(Done { message: String::new() })))
    }

    fn broadcast_tabs(&mut self) {
        let apps: Vec<usize> =
            self.clients.iter().filter(|(_, c)| matches!(c.mode, Mode::App(_))).map(|(k, _)| *k).collect();
        if apps.is_empty() {
            return;
        }
        let list = self.tab_list();
        for k in apps {
            self.send(k, &StreamOut::TabsChanged(list.clone()));
        }
    }

    fn send_screen_updates(&mut self) {
        if self.dirty.is_empty() || self.last_frame.elapsed() < Duration::from_millis(8) {
            return;
        }
        self.last_frame = Instant::now();
        let dirty: Vec<u64> = std::mem::take(&mut self.dirty).into_iter().collect();
        let apps: Vec<(usize, Vec<u64>)> = self
            .clients
            .iter()
            .filter_map(|(k, c)| if let Mode::App(t) = &c.mode { Some((*k, t.clone())) } else { None })
            .collect();
        if apps.is_empty() {
            return;
        }
        for n in dirty {
            let Some(update) = self.tabs.get_mut(&n).and_then(|t| crate::screen::update(t, false)) else { continue };
            for (k, tabs) in &apps {
                if tabs.contains(&n) {
                    self.send(*k, &StreamOut::Screen(update.clone()));
                }
            }
        }
    }

    // -----------------------------------------------------------------------------------------
    // Tab I/O
    // -----------------------------------------------------------------------------------------

    fn on_tab_readable(&mut self, n: u64) {
        let mut buf = vec![0u8; 16384];
        let mut ended = false;
        let mut total = 0usize;
        loop {
            let Some(fd) = self.tabs.get(&n).map(|t| t.fd()) else { return };
            match sys::read_fd(fd, &mut buf) {
                Ok(0) => {
                    ended = true;
                    break;
                }
                Ok(k) => {
                    total += k;
                    self.on_tab_output(n, &buf[..k]);
                    // Be fair to other tabs: at most 32 KB per turn.
                    if total >= 32 * 1024 {
                        break;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    // EIO: every process holding the terminal has exited.
                    ended = true;
                    break;
                }
            }
        }
        if ended && self.tabs.contains_key(&n) {
            self.end_tab(n, "closed");
        }
    }

    fn on_tab_output(&mut self, n: u64, data: &[u8]) {
        let (ms, tz) = self.cx();
        let fx = {
            let Some(t) = self.tabs.get_mut(&n) else { return };
            let cx = Ctx { index: &self.index, config: &self.config, tz, now_ms: ms };
            t.process_output(data, &cx)
        };
        // Attached terminals get the raw output (minus ctx's private reports).
        let attached: Vec<usize> =
            self.clients.iter().filter(|(_, c)| c.mode == Mode::Attached(n)).map(|(k, _)| *k).collect();
        if !attached.is_empty() {
            let clean = strip_private(data);
            for k in attached {
                self.send(k, &StreamOut::Bytes(clean.clone()));
            }
        }
        if !fx.replies.is_empty()
            && let Some(t) = self.tabs.get_mut(&n)
        {
            t.to_pty.extend_from_slice(&fx.replies);
        }
        if fx.title_changed || fx.folder_changed || !fx.started.is_empty() || !fx.finished.is_empty() || fx.prompt {
            self.broadcast_tabs();
        }
        if let Some(c) = &fx.clipboard {
            let apps: Vec<usize> =
                self.clients.iter().filter(|(_, cl)| matches!(cl.mode, Mode::App(_))).map(|(k, _)| *k).collect();
            for k in apps {
                self.send(k, &StreamOut::Clipboard { tab: n, text: c.clone() });
            }
        }
        self.dirty.insert(n);
        self.enforce_memory(n);
        self.check_waiters(n, &fx);
        self.release_enter(n, false);
        if fx.prompt {
            self.replay_starting(false);
        }
        self.flush_tab(n);
    }

    fn flush_tab(&mut self, n: u64) {
        let Some(t) = self.tabs.get_mut(&n) else { return };
        while !t.to_pty.is_empty() {
            match sys::write_fd(t.fd(), &t.to_pty) {
                Ok(k) => {
                    t.to_pty.drain(..k);
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        let want_write = !t.to_pty.is_empty();
        self.poller.modify(t.fd(), want_write);
    }

    /// Keep scrollback within its budgets: per tab, then in total (least recently viewed first).
    fn enforce_memory(&mut self, n: u64) {
        let per_tab = (self.config.scrollback_mb_per_tab * 1024.0 * 1024.0) as usize;
        if let Some(t) = self.tabs.get_mut(&n) {
            t.cold.enforce(per_tab);
        }
        let total_max = (self.config.scrollback_mb_total * 1024.0 * 1024.0) as usize;
        let mut total: usize = self.tabs.values().map(|t| t.cold.bytes()).sum();
        if total <= total_max {
            return;
        }
        let mut order: Vec<(i64, u64)> =
            self.tabs.values().map(|t| (if t.viewers > 0 { i64::MAX } else { t.last_output_ms }, t.number)).collect();
        order.sort();
        for (_, tn) in order {
            while total > total_max {
                let Some(t) = self.tabs.get_mut(&tn) else { break };
                let freed = t.cold.drop_oldest();
                if freed == 0 {
                    break;
                }
                total -= freed;
            }
            if total <= total_max {
                break;
            }
        }
    }

    // -----------------------------------------------------------------------------------------
    // Waiters
    // -----------------------------------------------------------------------------------------

    fn check_waiters(&mut self, n: u64, fx: &Effects) {
        let mut i = 0;
        while i < self.waiters.len() {
            if self.waiters[i].tab != n {
                i += 1;
                continue;
            }
            let mut fire = false;
            {
                let t = &self.tabs[&n];
                let w = &mut self.waiters[i];
                match &mut w.kind {
                    WaitKind::Run { number, wait, sent_at, .. } => {
                        if number.is_none() {
                            if let Some(s) = fx.started.first() {
                                *number = Some(*s);
                                if !*wait {
                                    fire = true;
                                }
                            } else if fx.prompt_without_command.is_some()
                                && sent_at.elapsed() < Duration::from_secs(3600)
                            {
                                // The shell refused the line (syntax error) and is back at the prompt.
                                fire = true;
                            }
                        }
                        if let Some(num) = number
                            && fx.finished.iter().any(|(f, _)| f == num)
                        {
                            fire = true;
                        }
                    }
                    WaitKind::Idle => {
                        if !fx.finished.is_empty() && t.running.is_empty() {
                            fire = true;
                        }
                    }
                    WaitKind::Shows { text, seen } => {
                        if t.output_seq > *seen {
                            let needle = text.to_lowercase();
                            // Look at recent lines (new output is near the bottom).
                            let lines = t.recent_output(t.rows as usize * 2 + fx.new_lines as usize);
                            if lines.iter().any(|l| l.to_lowercase().contains(&needle)) {
                                fire = true;
                            }
                        }
                    }
                    WaitKind::Quiet { .. } => {}
                    WaitKind::Ready { .. } => fire = fx.prompt || t.integration,
                }
            }
            if fire {
                let w = self.waiters.remove(i);
                self.fire_waiter_with(w, now_ms(), fx);
            } else {
                i += 1;
            }
        }
    }

    fn fire_waiter(&mut self, w: Waiter, ms: i64) {
        self.fire_waiter_with(w, ms, &Effects::default());
    }

    fn fire_waiter_with(&mut self, w: Waiter, _ms: i64, fx: &Effects) {
        let timed_out = w.deadline.is_some_and(|d| Instant::now() >= d);
        let waited = w.started.elapsed().as_secs_f64();
        let Some(t) = self.tabs.get(&w.tab) else {
            self.reply(
                w.client,
                Response::Error(ErrorReply::new(ErrorKind::Failed, format!("Tab {} closed while waiting.", w.tab))),
            );
            return;
        };
        let words = tab_ref_words(t);
        let name = t.name.clone();
        let response = match w.kind {
            WaitKind::Ready { opened } => Response::Ok(Output::Opened(opened)),
            WaitKind::Run { command, number, max_lines, colors, wait, .. } => {
                let tab_n = w.tab;
                // The caller sees this output now.
                if let Some(tm) = self.tabs.get_mut(&tab_n) {
                    tm.unread_lines = 0;
                }
                let Some(t) = self.tabs.get(&tab_n) else { return };
                match number {
                    None if timed_out => Response::Error(ErrorReply::new(
                        ErrorKind::TimeLimit,
                        format!(
                            "The command didn't start in tab {tab_n} within the time limit. ctx read {words} shows what's on screen."
                        ),
                    )),
                    None => {
                        // Back at the prompt without a command: show what the shell said.
                        let (lines, _) = t.last_lines(t.rows as usize, colors);
                        let status = fx.prompt_without_command.flatten();
                        let lines: Vec<String> =
                            lines.into_iter().rev().take(10).collect::<Vec<_>>().into_iter().rev().collect();
                        Response::Ok(Output::Ran(RunResult {
                            tab: tab_n,
                            tab_name: name,
                            command_number: None,
                            command,
                            exit_status: status.or(Some(1)),
                            still_running: false,
                            took_seconds: waited,
                            output_lines: lines.len() as u64,
                            output: lines,
                            events: vec![],
                            note: Some("the shell didn't run it (see its message above)".into()),
                        }))
                    }
                    Some(num) => {
                        let finished = t.finished.iter().find(|r| r.number == num);
                        let running = t.running.iter().find(|r| r.number == num);
                        let (mut lines, gap, total, exit, still) = match (finished, running) {
                            (Some(f), _) => {
                                let (l, gap) = f.kept_output();
                                (l, gap, f.total_lines, t.last_exit_for(num), false)
                            }
                            (None, Some(r)) => {
                                let (mut l, gap) = r.kept_output();
                                let live = t.live_rows(true);
                                let extra = live.len() as u64;
                                l.extend(live);
                                (l, gap, r.total_lines + extra, None, true)
                            }
                            _ => (Vec::new(), false, 0, None, false),
                        };
                        let read_all = format!("ctx read command {num} in {words}");
                        let (raw, mut note) = cap_lines(std::mem::take(&mut lines), Some(max_lines), &read_all);
                        let (lines, events) = present(raw, colors);
                        if gap && note.is_none() {
                            note = Some(format!("some lines in the middle are not shown; {read_all} for all"));
                        }
                        let started_only = !wait && still;
                        if still {
                            let msg = if started_only {
                                format!(
                                    "started in tab {tab_n} as command {num}; check on it with: ctx read last command in {words}, or ctx wait until {words} is idle"
                                )
                            } else {
                                format!(
                                    "still running in tab {tab_n}; use ctx wait until {words} is idle, or ctx read last command in {words}"
                                )
                            };
                            note = Some(match note {
                                Some(n) => format!("{n}; {msg}"),
                                None => msg,
                            });
                        }
                        let took = finished.map(|f| f.total_time()).unwrap_or(waited);
                        Response::Ok(Output::Ran(RunResult {
                            tab: tab_n,
                            tab_name: name,
                            command_number: Some(num),
                            command,
                            exit_status: exit,
                            still_running: still,
                            took_seconds: took,
                            output: lines,
                            events,
                            output_lines: total,
                            note,
                        }))
                    }
                }
            }
            WaitKind::Idle => {
                if timed_out {
                    Response::Error(ErrorReply::new(
                        ErrorKind::TimeLimit,
                        format!(
                            "{} is still busy after {}. It keeps running; ctx read last command in {words} shows its output so far.",
                            capital(&t.display_name()),
                            ctx_protocol::time::duration_words(waited)
                        ),
                    ))
                } else {
                    Response::Ok(Output::Waited(WaitResult {
                        tab: w.tab,
                        tab_name: name,
                        condition: "idle".into(),
                        exit_status: t.last_exit,
                        line: None,
                        waited_seconds: waited,
                    }))
                }
            }
            WaitKind::Shows { text, .. } => {
                if timed_out {
                    Response::Error(ErrorReply::new(
                        ErrorKind::TimeLimit,
                        format!(
                            "\"{text}\" didn't appear in {} within {}.",
                            t.display_name(),
                            ctx_protocol::time::duration_words(waited)
                        ),
                    ))
                } else {
                    let needle = text.to_lowercase();
                    let lines = t.recent_output(t.rows as usize * 2 + fx.new_lines as usize);
                    let line = lines.into_iter().rev().find(|l| l.to_lowercase().contains(&needle));
                    Response::Ok(Output::Waited(WaitResult {
                        tab: w.tab,
                        tab_name: name,
                        condition: "shows".into(),
                        exit_status: None,
                        line,
                        waited_seconds: waited,
                    }))
                }
            }
            WaitKind::Quiet { ms } => {
                let quiet_for = now_ms() - t.last_output_ms.max(w_started_ms(&w)) >= ms as i64;
                if timed_out && !quiet_for {
                    Response::Error(ErrorReply::new(
                        ErrorKind::TimeLimit,
                        format!(
                            "{} kept printing for {}.",
                            capital(&t.display_name()),
                            ctx_protocol::time::duration_words(waited)
                        ),
                    ))
                } else {
                    Response::Ok(Output::Waited(WaitResult {
                        tab: w.tab,
                        tab_name: name,
                        condition: "quiet".into(),
                        exit_status: None,
                        line: None,
                        waited_seconds: waited,
                    }))
                }
            }
        };
        self.reply(w.client, response);
    }

    // -----------------------------------------------------------------------------------------
    // Maintenance and shutdown
    // -----------------------------------------------------------------------------------------

    /// Retention: closed tabs older than `keep_closed_tabs_days`, then the oldest until the
    /// saved total fits `saved_total_gb`.
    fn prune(&mut self) {
        let now = now_ms();
        let max_age = self.config.keep_closed_tabs_days as i64 * 86_400_000;
        let max_bytes = (self.config.saved_total_gb * 1024.0 * 1024.0 * 1024.0) as u64;
        let closed = self.index.closed_oldest_first().unwrap_or_default();
        let size_of = |n: u64| std::fs::metadata(self.paths.tab_log(n)).map(|m| m.len()).unwrap_or(0);
        let mut total: u64 = std::fs::read_dir(self.paths.tabs())
            .map(|d| d.filter_map(|e| e.ok()?.metadata().ok()).map(|m| m.len()).sum())
            .unwrap_or(0);
        for row in closed {
            let too_old = now - row.closed_ms.unwrap_or(row.last_active_ms) > max_age;
            if too_old || total > max_bytes {
                total = total.saturating_sub(size_of(row.number));
                let _ = std::fs::remove_file(self.paths.tab_log(row.number));
                let _ = std::fs::remove_file(self.paths.tab_history(row.number));
                let _ = self.index.forget(row.number);
            }
        }
    }

    fn shutdown(&mut self) {
        let state = SavedState {
            reopen: if self.restart { self.tabs.keys().copied().collect() } else { Vec::new() },
            windows: self.windows.iter().map(|w| (w.number, w.name.clone(), w.tabs.clone())).collect(),
            last_open: self.tabs.keys().copied().collect(),
        };
        if let Ok(b) = serde_json::to_vec(&state) {
            let _ = std::fs::write(self.paths.state(), b);
        }
        let numbers: Vec<u64> = self.tabs.keys().copied().collect();
        let reason = "interrupted";
        for n in numbers {
            if let Some(t) = self.tabs.get(&n) {
                sys::kill(t.launch_pid, libc::SIGHUP);
                if let Some(pg) = sys::foreground_pgid(t.fd()) {
                    sys::kill_group(pg, libc::SIGHUP);
                }
            }
            self.end_tab(n, reason);
        }
        self.flush_all_clients();
        let _ = std::fs::remove_file(self.paths.socket());
    }
}

pub static TERMINATE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn w_started_ms(w: &Waiter) -> i64 {
    now_ms() - w.started.elapsed().as_millis() as i64
}

struct Reopening {
    env: BTreeMap<String, String>,
    transcript: Vec<String>,
    reason: Option<String>,
    last_active_ms: i64,
    history_file: Option<String>,
    prefill: Option<String>,
    next_command: u64,
    opened_by: String,
    opened_ms: i64,
    columns: u16,
    rows: u16,
}

/// Variables that belong to the old process and aren't restored.
fn skip_on_reopen(k: &str) -> bool {
    matches!(k, "PWD" | "OLDPWD" | "SHLVL" | "TMPDIR" | "SSH_AUTH_SOCK" | "_" | "ZDOTDIR")
        || k.starts_with("TERM")
        || k.starts_with("CTX_")
        || k.starts_with("XPC_")
        || k.starts_with("__CF")
        || k == "COLORTERM"
}

/// Check a tab or window name against the rules.
pub fn check_name(name: &str) -> Result<(), String> {
    let n = name.trim();
    if n.is_empty() || n.chars().count() > 64 {
        return Err("A name is 1 to 64 characters long.".into());
    }
    if let Some(bad) = n.chars().find(|c| !(c.is_alphanumeric() || *c == ' ' || *c == '-' || *c == '_')) {
        return Err(format!("Names can use letters, digits, spaces, - and _, but not \"{bad}\"."));
    }
    if !n.chars().any(char::is_alphabetic) {
        return Err("A name needs at least one letter, so it can't be mistaken for a number.".into());
    }
    if ctx_cli::grammar::is_sentence_word(n) {
        return Err(format!(
            "\"{n}\" is one of ctx's sentence words, so it can't be a name. Try a longer name, such as \"{n} tab\"."
        ));
    }
    Ok(())
}

fn ctx_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + usize::from(a[i - 1] != b[j - 1]));
        }
        prev = cur;
    }
    prev[b.len()]
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// How to name a tab in a suggested sentence.
fn tab_ref_words(t: &Tab) -> String {
    match &t.name {
        Some(n) => quote(n),
        None => format!("tab {}", t.number),
    }
}

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

pub fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Cut long output to head and tail, saying how much was skipped and how to see it all.
fn cap_lines(lines: Vec<String>, max: Option<usize>, read_all: &str) -> (Vec<String>, Option<String>) {
    let Some(max) = max else { return (lines, None) };
    if lines.len() <= max || max == 0 {
        return (lines, None);
    }
    let head = max / 2;
    let tail = max - head;
    let skipped = lines.len() - head - tail;
    let mut out: Vec<String> = lines[..head].to_vec();
    out.push(format!("… {} lines skipped …", thousands(skipped as u64)));
    out.extend_from_slice(&lines[lines.len() - tail..]);
    let note = format!(
        "showing the first {} and last {} of {} lines; {read_all} for all",
        thousands(head as u64),
        thousands(tail as u64),
        thousands(lines.len() as u64)
    );
    (out, Some(note))
}

/// Stored lines → lines to show (styled or plain) and where the event lines are.
fn present(raw: Vec<String>, colors: bool) -> (Vec<String>, Vec<EventLine>) {
    let mut events = Vec::new();
    let mut shown = Vec::with_capacity(raw.len());
    for (i, l) in raw.iter().enumerate() {
        if let Some((json, _)) = ctx_store::record::split_event_line(l) {
            events.push(EventLine { line: i as u64 + 1, event: json.to_string() });
        }
        shown.push(display_line(l, colors));
    }
    (shown, events)
}

/// Remove ctx's private OSC reports from bytes going to an attached terminal.
fn strip_private(data: &[u8]) -> Vec<u8> {
    let marker = b"\x1b]6973;";
    if !data.windows(marker.len()).any(|w| w == marker) {
        return data.to_vec();
    }
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if data[i..].starts_with(marker) {
            let mut j = i + marker.len();
            while j < data.len() && data[j] != 0x07 && !(data[j] == 0x1b && data.get(j + 1) == Some(&b'\\')) {
                j += 1;
            }
            i = if j < data.len() && data[j] == 0x07 { j + 1 } else { j + 2 };
            continue;
        }
        out.push(data[i]);
        i += 1;
    }
    out
}

/// Bytes that redraw a tab's whole state in another terminal (for `attach`).
fn redraw_bytes(t: &dyn Terminal) -> Vec<u8> {
    let mut out = b"\x1b[H\x1b[2J\x1b[3J".to_vec();
    if let Some(g) = t.ghostty() {
        let opts = ghostty_vt::fmt::FormatterOptions::new()
            .with_format(ghostty_vt::fmt::Format::Vt)
            .with_unwrap(false)
            .with_trim(false)
            .with_style(true)
            .with_cursor(true)
            .with_modes(true)
            .with_scrolling_region(true)
            .with_keyboard(true)
            .with_pwd(false)
            .with_hyperlink(true)
            .with_charsets(true)
            .with_tabstops(true);
        if let Ok(mut f) = ghostty_vt::fmt::Formatter::new(g, opts)
            && let Ok(bytes) = f.format_alloc()
        {
            out.extend_from_slice(&bytes);
            return out;
        }
    }
    for (i, row) in t.screen(true).iter().enumerate() {
        out.extend_from_slice(format!("\x1b[{};1H{row}", i + 1).as_bytes());
    }
    out
}

fn set_mode(path: &std::path::Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}
