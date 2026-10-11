//! Messages between the `ctx` CLI (and CTX.app) and the `ctx` server, plus the output types that
//! `as json` and `as toon` print.
//!
//! Every message travels as a little-endian `u32` length followed by a `postcard` body. The first
//! message on a connection is a [`Hello`]; the server answers with a [`Welcome`] or refuses a
//! client whose [`PROTOCOL_VERSION`] differs.
//!
//! The output types are also the documented JSON schema (`docs/output.md`), so their field names
//! are whole English words. Postcard isn't self-describing, so no field is ever skipped when
//! serialising: absent values are `None` and print as `null` in JSON.

pub mod config;
pub mod time;
pub mod toon;

use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

/// Bumped whenever a message changes shape. Client and server must match exactly.
pub const PROTOCOL_VERSION: u32 = 1;
/// Version of the `as json` / `as toon` schema. Printed as `"schema": 1`.
pub const SCHEMA_VERSION: u32 = 1;
/// Largest message either side accepts (a whole `read everything` of a big tab fits).
pub const MAX_MESSAGE_BYTES: usize = 256 * 1024 * 1024;

// ---------------------------------------------------------------------------------------------
// Framing
// ---------------------------------------------------------------------------------------------

/// Write one length-prefixed message.
pub fn write_message<T: Serialize>(w: &mut impl Write, msg: &T) -> io::Result<()> {
    let body = encode(msg)?;
    w.write_all(&body)?;
    w.flush()
}

/// Encode one message, length prefix included.
pub fn encode<T: Serialize>(msg: &T) -> io::Result<Vec<u8>> {
    let body = postcard::to_stdvec(msg).map_err(|e| io::Error::other(e.to_string()))?;
    let mut out = Vec::with_capacity(body.len() + 4);
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// Read one length-prefixed message. `Ok(None)` on a clean end of stream.
pub fn read_message<T: for<'de> Deserialize<'de>>(r: &mut impl Read) -> io::Result<Option<T>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_MESSAGE_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "message too large"));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    decode(&body).map(Some)
}

/// Decode a message body (without its length prefix).
pub fn decode<T: for<'de> Deserialize<'de>>(body: &[u8]) -> io::Result<T> {
    postcard::from_bytes(body).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))
}

/// Splits complete frames out of a byte buffer that grows as data arrives (used by the server's
/// non-blocking loop).
#[derive(Default, Debug)]
pub struct FrameBuffer {
    buf: Vec<u8>,
}

impl FrameBuffer {
    pub fn extend(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// The next complete frame body, if one has fully arrived.
    pub fn next_frame(&mut self) -> io::Result<Option<Vec<u8>>> {
        if self.buf.len() < 4 {
            return Ok(None);
        }
        let len = u32::from_le_bytes(self.buf[..4].try_into().unwrap()) as usize;
        if len > MAX_MESSAGE_BYTES {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "message too large"));
        }
        if self.buf.len() < 4 + len {
            return Ok(None);
        }
        let body = self.buf[4..4 + len].to_vec();
        self.buf.drain(..4 + len);
        Ok(Some(body))
    }

    pub fn pending(&self) -> usize {
        self.buf.len()
    }
}

// ---------------------------------------------------------------------------------------------
// Handshake
// ---------------------------------------------------------------------------------------------

/// First message from every client.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Hello {
    pub protocol: u32,
    /// `ctx` version string of the client, for the mismatch message.
    pub client_version: String,
    /// What kind of client this is.
    pub kind: ClientKind,
    /// The client's current folder (used as the default folder for `open tab`).
    pub folder: String,
    /// `CTX_TAB` from the client's environment, if set. A claim, checked by the server against
    /// the client's real process tree before it's trusted.
    pub in_tab: Option<u64>,
    /// `CTX_AGENT_NAME` from the client's environment: a declared label, stored as claimed.
    pub declared_name: Option<String>,
    /// Whether a person could answer a question (stdin and stdout are terminals).
    pub interactive: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientKind {
    /// The `ctx` command line.
    Cli,
    /// CTX.app, the GUI.
    App,
    /// `ctx mcp`, serving an agent over MCP.
    Mcp,
    /// A generated scoped launcher asking who started it.
    Launcher,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Welcome {
    Accepted {
        server_version: String,
        server_pid: u32,
    },
    /// The server speaks a different protocol. `message` says what to do.
    VersionMismatch {
        server_version: String,
        server_protocol: u32,
        message: String,
        busy_tabs: Vec<u64>,
    },
}

// ---------------------------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------------------------

/// A request and the canonical sentence it came from (for the activity log).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Envelope {
    pub sentence: String,
    pub request: Request,
}

/// How a sentence names a tab.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
pub enum TabRef {
    Number(u64),
    Name(String),
    /// The tab the command is run from (`this tab`, or no tab given inside CTX).
    This,
}

/// How a sentence names a window.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
pub enum WindowRef {
    Number(u64),
    Name(String),
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum ReadWhat {
    /// What's on screen now.
    Screen,
    /// The last N lines, scrollback included.
    LastLines(u32),
    /// All of it.
    Everything,
    /// The last N commands with their outputs.
    LastCommands(u32),
    /// One numbered command.
    Command(u64),
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum WaitCondition {
    /// The running command finished and the prompt is back.
    Idle,
    /// This text appeared in new output.
    Shows(String),
    /// No new output for this many milliseconds.
    QuietFor(u64),
}

/// Narrowing used by `find`, `list activity` and `ask about all tabs`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct Filter {
    pub tab: Option<TabRef>,
    /// Absolute folder; matches the folder and anything under it.
    pub folder: Option<String>,
    /// `me` or an agent's name (`claude`, `codex`, …), matched against `ran_by`.
    pub by: Option<String>,
    /// Unix time in seconds.
    pub since: Option<i64>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Request {
    ListTabs,
    ListClosedTabs {
        limit: u32,
    },
    ListCommands {
        tab: TabRef,
        limit: Option<u32>,
    },
    ListActivity {
        filter: Filter,
        limit: u32,
    },
    Read {
        tab: TabRef,
        what: ReadWhat,
        colors: bool,
        max_lines: Option<u32>,
    },
    Find {
        text: String,
        filter: Filter,
        limit: u32,
    },
    Status,

    OpenWindow {
        folder: Option<String>,
        name: Option<String>,
    },
    OpenTab {
        window: Option<WindowRef>,
        folder: Option<String>,
        name: Option<String>,
    },
    CloseTab {
        tab: TabRef,
        without_asking: bool,
    },
    CloseWindow {
        window: WindowRef,
        without_asking: bool,
    },
    RenameTab {
        tab: TabRef,
        to: String,
    },
    RenameWindow {
        window: WindowRef,
        to: String,
    },
    Move {
        tab: TabRef,
        to: WindowRef,
        position: Option<u32>,
    },
    Clear {
        tab: TabRef,
    },
    Reopen {
        tab: u64,
    },
    Forget {
        tab: u64,
        without_asking: bool,
    },

    Run {
        tab: TabRef,
        command: String,
        colors: bool,
        max_lines: Option<u32>,
        within_ms: Option<u64>,
        wait: bool,
    },
    Type {
        tab: TabRef,
        text: String,
        then_press: Option<String>,
    },
    Press {
        tab: TabRef,
        keys: Vec<String>,
    },
    Wait {
        tab: TabRef,
        until: WaitCondition,
        within_ms: u64,
    },
    Show {
        tab: TabRef,
    },

    /// Connect this terminal to a tab. After the reply the connection carries [`StreamIn`] and
    /// [`StreamOut`] frames.
    Attach {
        tab: TabRef,
        columns: u16,
        rows: u16,
    },
    /// CTX.app: start receiving screen updates for these tabs (replaces earlier subscriptions).
    Subscribe {
        tabs: Vec<u64>,
    },
    /// CTX.app: input for a tab.
    Input {
        tab: u64,
        input: AppInput,
    },

    /// Full records of commands, for `ctx ask`: every command of the tab with its output lines.
    Records {
        tab: TabRef,
    },
    /// Several tabs' records at once (`ask about all tabs …`): matching tab numbers.
    MatchingTabs {
        filter: Filter,
        limit: u32,
    },
    /// From a scoped launcher: who ran the command in the tab that this process belongs to.
    WhoRan {
        pid: i32,
    },
    /// Tabs whose foreground process runs under another responsible process (launchers).
    Restart {
        without_asking: bool,
    },
}

// ---------------------------------------------------------------------------------------------
// Responses
// ---------------------------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Response {
    Ok(Output),
    Error(ErrorReply),
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ErrorReply {
    pub error: ErrorKind,
    /// An English sentence saying what went wrong and what to type instead.
    pub message: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    NotAValidCommand,
    NoSuchTab,
    NoSuchWindow,
    NotAllowed,
    Busy,
    SomeoneTyping,
    /// The action would end running programs; the caller must confirm (`without asking`).
    NeedsConfirmation,
    TimeLimit,
    Unsupported,
    Failed,
}

impl ErrorKind {
    /// The `ctx` exit status for this error.
    pub fn exit_status(self) -> i32 {
        match self {
            ErrorKind::TimeLimit => 3,
            _ => 1,
        }
    }

    /// The snake_case name used in JSON.
    pub fn word(self) -> &'static str {
        match self {
            ErrorKind::NotAValidCommand => "not_a_valid_command",
            ErrorKind::NoSuchTab => "no_such_tab",
            ErrorKind::NoSuchWindow => "no_such_window",
            ErrorKind::NotAllowed => "not_allowed",
            ErrorKind::Busy => "busy",
            ErrorKind::SomeoneTyping => "someone_typing",
            ErrorKind::NeedsConfirmation => "needs_confirmation",
            ErrorKind::TimeLimit => "time_limit",
            ErrorKind::Unsupported => "unsupported",
            ErrorKind::Failed => "failed",
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Output {
    Tabs(TabList),
    ClosedTabs(ClosedTabList),
    Commands(CommandList),
    Activity(ActivityList),
    Text(TextOutput),
    CommandsWithOutput(CommandOutputs),
    Found(FindResults),
    Status(Status),
    Opened(Opened),
    Done(Done),
    Ran(RunResult),
    Waited(WaitResult),
    Attached(Attached),
    Records(CommandOutputs),
    TabNumbers(Vec<u64>),
    RanBy(RanBy),
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct TabList {
    pub windows: Vec<WindowInfo>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct WindowInfo {
    pub number: u64,
    pub name: Option<String>,
    pub tabs: Vec<TabInfo>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct TabInfo {
    pub number: u64,
    pub name: Option<String>,
    pub folder: String,
    pub window: u64,
    /// `you`, or an agent such as `claude (pid 5512)`.
    pub opened_by: String,
    /// The title a program set (OSC 0/2), shown second to the name.
    pub title: Option<String>,
    /// `idle`, `busy` or `typing` (someone has typed at the prompt without pressing Enter).
    pub state: String,
    /// The command running now, if any.
    pub running: Option<String>,
    /// Process id of the foreground process.
    pub pid: Option<i32>,
    /// When the running command started.
    pub started: Option<String>,
    pub unread_lines: u64,
    /// Columns and rows.
    pub size: [u16; 2],
    /// Whether shell integration marks are active (needed by `run`, `wait until … is idle`).
    pub shell_integration: bool,
    /// Name of the program whose macOS permissions the foreground process runs with, when that
    /// isn't the tab's own shell (a scoped launcher).
    pub permissions_of: Option<String>,
    /// Remote host when the tab is inside ssh.
    pub remote_host: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ClosedTabList {
    pub tabs: Vec<ClosedTab>,
    /// How many closed tabs exist in total.
    pub total: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ClosedTab {
    pub number: u64,
    pub name: Option<String>,
    pub folder: String,
    pub opened_by: String,
    pub opened_at: String,
    pub closed_at: String,
    /// `closed`, or `interrupted` when it ended with a reboot or server crash.
    pub ended: String,
    pub commands: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct CommandList {
    pub tab: u64,
    pub tab_name: Option<String>,
    pub commands: Vec<CommandInfo>,
    pub total: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct CommandInfo {
    pub number: u64,
    pub command: String,
    /// `user@host:folder` where it ran.
    #[serde(rename = "where")]
    pub location: String,
    /// Who ran it: `you`, an agent such as `claude (pid 5512)`, or both joined by ` and `.
    pub ran_by: String,
    pub exit_status: Option<i32>,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub took_seconds: Option<f64>,
    /// True when found from the screen without shell-integration marks (best effort).
    pub approximate: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ActivityList {
    pub entries: Vec<ActivityEntry>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ActivityEntry {
    pub at: String,
    /// Readable identity: `claude (pid 5512) in tab 2`.
    pub client: String,
    /// Process name of the nearest recognisable program.
    pub program: String,
    pub pid: i32,
    /// A name the client declared with `CTX_AGENT_NAME` (claimed, not verified).
    pub declared_name: Option<String>,
    /// The request, as a canonical sentence.
    pub sentence: String,
    pub tab: Option<u64>,
    /// Text typed into a tab, in full.
    pub typed: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct TextOutput {
    pub tab: u64,
    pub tab_name: Option<String>,
    pub lines: Vec<String>,
    /// Total lines available for this request before any cap.
    pub total_lines: u64,
    /// Said when output was capped, with the command that shows everything.
    pub note: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct CommandOutputs {
    pub tab: u64,
    pub tab_name: Option<String>,
    pub commands: Vec<CommandRecord>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct CommandRecord {
    pub number: u64,
    pub command: String,
    #[serde(rename = "where")]
    pub location: String,
    pub ran_by: String,
    pub exit_status: Option<i32>,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub took_seconds: Option<f64>,
    pub approximate: bool,
    /// Whether this was a full-screen program (only its final screen is kept).
    pub full_screen: bool,
    pub output: Vec<String>,
    /// Lines of output in total (before any cap).
    pub output_lines: u64,
    pub note: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct FindResults {
    pub text: String,
    pub hits: Vec<FindHit>,
    /// Hits found before the cap.
    pub total: u64,
    pub note: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct FindHit {
    pub tab: u64,
    pub tab_name: Option<String>,
    /// `name`, `command` or `output`.
    pub found_in: String,
    pub command: Option<u64>,
    pub command_text: Option<String>,
    pub line: Option<u64>,
    pub ran_by: Option<String>,
    pub at: String,
    pub text: String,
    pub before: Vec<String>,
    pub after: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Status {
    pub version: String,
    pub protocol: u32,
    pub pid: u32,
    pub uptime_seconds: u64,
    pub socket: String,
    pub memory_bytes: u64,
    pub saved_bytes: u64,
    pub tabs: Vec<TabMemory>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct TabMemory {
    pub number: u64,
    pub name: Option<String>,
    /// Estimated bytes held for this tab (screen, recent lines, compressed older lines).
    pub memory_bytes: u64,
    pub scrollback_lines: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Opened {
    pub tab: Option<u64>,
    pub window: u64,
    /// Set when a new window was opened (with its name, if given).
    pub new_window: bool,
    pub window_name: Option<String>,
    pub name: Option<String>,
    pub folder: String,
    /// Environment variables that were not restored (`reopen`).
    pub not_restored: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Done {
    pub message: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct RunResult {
    pub tab: u64,
    pub tab_name: Option<String>,
    pub command_number: Option<u64>,
    pub command: String,
    pub exit_status: Option<i32>,
    /// True when the command was left running (`without waiting`, or the time limit ran out).
    pub still_running: bool,
    pub took_seconds: f64,
    pub output: Vec<String>,
    pub output_lines: u64,
    pub note: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct WaitResult {
    pub tab: u64,
    pub tab_name: Option<String>,
    /// `idle`, `shows` or `quiet`.
    pub condition: String,
    pub exit_status: Option<i32>,
    /// The line where the awaited text appeared.
    pub line: Option<String>,
    pub waited_seconds: f64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Attached {
    pub tab: u64,
    pub tab_name: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct RanBy {
    pub tab: Option<u64>,
    pub ran_by: String,
    /// True when any agent took part in typing or running the current command.
    pub agent_involved: bool,
}

// ---------------------------------------------------------------------------------------------
// Streams: attach and CTX.app subscriptions
// ---------------------------------------------------------------------------------------------

/// Client → server after `Attach` or `Subscribe`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum StreamIn {
    /// Bytes typed in the attached terminal.
    Bytes(Vec<u8>),
    Resize {
        columns: u16,
        rows: u16,
    },
    Detach,
    /// CTX.app sends further requests on the same connection.
    Request(Envelope),
}

/// Server → client after `Attach` or `Subscribe`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum StreamOut {
    /// Raw terminal output for an attached terminal.
    Bytes(Vec<u8>),
    /// The tab ended (its shell exited or it was closed).
    Ended { message: String },
    /// Changed rows of a subscribed tab.
    Screen(ScreenUpdate),
    /// Reply to a `StreamIn::Request`.
    Reply(Response),
    /// The list of windows and tabs changed.
    TabsChanged(TabList),
    /// Bring this tab to the front (`ctx show`).
    Show { tab: u64 },
    /// A program wrote to the clipboard (OSC 52); the app decides whether to accept it.
    Clipboard { tab: u64, text: String },
}

/// One coalesced frame of changes to a tab's screen.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ScreenUpdate {
    pub tab: u64,
    pub columns: u16,
    pub rows: u16,
    /// Rows that changed (all rows on the first update).
    pub changed: Vec<RowUpdate>,
    pub cursor: Option<[u16; 2]>,
    pub cursor_visible: bool,
    pub title: Option<String>,
    /// Whether a program uses the alternate (full-screen) buffer.
    pub full_screen: bool,
    pub default_foreground: [u8; 3],
    pub default_background: [u8; 3],
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct RowUpdate {
    pub row: u16,
    pub runs: Vec<StyledRun>,
    /// An event line drawn by ctx, not by a program.
    pub event: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct StyledRun {
    pub text: String,
    pub style: CellStyle,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CellStyle {
    pub foreground: Option<[u8; 3]>,
    pub background: Option<[u8; 3]>,
    /// Bit flags: 1 bold, 2 italic, 4 faint, 8 underline, 16 inverse, 32 strikethrough,
    /// 64 invisible, 128 blink, 256 wide (each character takes two cells).
    pub flags: u16,
    pub hyperlink: Option<u32>,
}

/// Input from CTX.app for a tab.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum AppInput {
    /// A key with modifiers, named the way `ctx press` names keys (`enter`, `ctrl-c`, `a`).
    Key {
        key: String,
        text: Option<String>,
    },
    Text(String),
    Paste(String),
    Resize {
        columns: u16,
        rows: u16,
    },
    Focus(bool),
    /// Raw bytes already encoded by the app (mouse reports).
    Raw(Vec<u8>),
    /// Scroll the tab's view: positive = towards older lines.
    Scroll(i32),
}

// ---------------------------------------------------------------------------------------------
// JSON
// ---------------------------------------------------------------------------------------------

impl Output {
    /// The JSON value for `as json`, with `"schema": 1` first.
    pub fn to_json(&self) -> serde_json::Value {
        let inner = match self {
            Output::Tabs(v) => serde_json::to_value(v),
            Output::ClosedTabs(v) => serde_json::to_value(v),
            Output::Commands(v) => serde_json::to_value(v),
            Output::Activity(v) => serde_json::to_value(v),
            Output::Text(v) => serde_json::to_value(v),
            Output::CommandsWithOutput(v) | Output::Records(v) => serde_json::to_value(v),
            Output::Found(v) => serde_json::to_value(v),
            Output::Status(v) => serde_json::to_value(v),
            Output::Opened(v) => serde_json::to_value(v),
            Output::Done(v) => serde_json::to_value(v),
            Output::Ran(v) => serde_json::to_value(v),
            Output::Waited(v) => serde_json::to_value(v),
            Output::Attached(v) => serde_json::to_value(v),
            Output::TabNumbers(v) => serde_json::to_value(serde_json::json!({ "tabs": v })),
            Output::RanBy(v) => serde_json::to_value(v),
        }
        .expect("output types always serialise");
        with_schema(inner)
    }
}

/// Put `"schema": 1` in front of an object's fields.
pub fn with_schema(value: serde_json::Value) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    map.insert("schema".into(), SCHEMA_VERSION.into());
    if let serde_json::Value::Object(fields) = value {
        for (k, v) in fields {
            map.insert(k, v);
        }
    } else {
        map.insert("value".into(), value);
    }
    serde_json::Value::Object(map)
}

impl ErrorReply {
    pub fn new(error: ErrorKind, message: impl Into<String>) -> Self {
        ErrorReply { error, message: message.into() }
    }

    pub fn to_json(&self) -> serde_json::Value {
        with_schema(serde_json::json!({ "error": self.error.word(), "message": self.message }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip<T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug>(v: T) {
        let bytes = encode(&v).unwrap();
        let back: T = read_message(&mut &bytes[..]).unwrap().unwrap();
        assert_eq!(v, back);
    }

    #[test]
    fn every_request_round_trips() {
        let tab = TabRef::Name("Dev server".into());
        let filter = Filter {
            tab: Some(TabRef::Number(4)),
            folder: Some("/x".into()),
            by: Some("claude".into()),
            since: Some(5),
        };
        let requests = vec![
            Request::ListTabs,
            Request::ListClosedTabs { limit: 20 },
            Request::ListCommands { tab: tab.clone(), limit: Some(3) },
            Request::ListActivity { filter: filter.clone(), limit: 50 },
            Request::Read { tab: TabRef::This, what: ReadWhat::LastLines(200), colors: true, max_lines: None },
            Request::Read { tab: tab.clone(), what: ReadWhat::Command(14), colors: false, max_lines: Some(9) },
            Request::Find { text: "deploy".into(), filter: filter.clone(), limit: 20 },
            Request::Status,
            Request::OpenWindow { folder: Some("/tmp".into()), name: None },
            Request::OpenTab { window: Some(WindowRef::Name("Work".into())), folder: None, name: Some("x".into()) },
            Request::CloseTab { tab: tab.clone(), without_asking: true },
            Request::CloseWindow { window: WindowRef::Number(2), without_asking: false },
            Request::RenameTab { tab: tab.clone(), to: "y".into() },
            Request::RenameWindow { window: WindowRef::Number(2), to: "Work".into() },
            Request::Move { tab: tab.clone(), to: WindowRef::Number(2), position: Some(1) },
            Request::Clear { tab: tab.clone() },
            Request::Reopen { tab: 40 },
            Request::Forget { tab: 40, without_asking: false },
            Request::Run {
                tab: tab.clone(),
                command: "npm test".into(),
                colors: false,
                max_lines: Some(1000),
                within_ms: Some(600_000),
                wait: true,
            },
            Request::Type { tab: tab.clone(), text: "yes".into(), then_press: Some("enter".into()) },
            Request::Press { tab: tab.clone(), keys: vec!["ctrl-c".into()] },
            Request::Wait { tab: tab.clone(), until: WaitCondition::Shows("ready".into()), within_ms: 300_000 },
            Request::Wait { tab: tab.clone(), until: WaitCondition::QuietFor(2000), within_ms: 1 },
            Request::Show { tab: tab.clone() },
            Request::Attach { tab: tab.clone(), columns: 80, rows: 24 },
            Request::Subscribe { tabs: vec![1, 2] },
            Request::Input { tab: 1, input: AppInput::Key { key: "ctrl-c".into(), text: None } },
            Request::Records { tab: tab.clone() },
            Request::MatchingTabs { filter, limit: 10 },
            Request::WhoRan { pid: 99 },
            Request::Restart { without_asking: true },
        ];
        for r in requests {
            round_trip(Envelope { sentence: "ctx test".into(), request: r });
        }
    }

    #[test]
    fn responses_round_trip() {
        round_trip(Response::Error(ErrorReply::new(ErrorKind::NoSuchTab, "There is no tab 9.")));
        round_trip(Response::Ok(Output::Done(Done { message: "ok".into() })));
        round_trip(Welcome::VersionMismatch {
            server_version: "0".into(),
            server_protocol: 0,
            message: "m".into(),
            busy_tabs: vec![],
        });
        round_trip(StreamOut::Bytes(vec![1, 2, 3]));
        round_trip(StreamIn::Resize { columns: 1, rows: 2 });
    }

    #[test]
    fn frame_buffer_splits_frames() {
        let mut fb = FrameBuffer::default();
        let a = encode(&Request::Status).unwrap();
        let b = encode(&Request::ListTabs).unwrap();
        let mut all = a.clone();
        all.extend_from_slice(&b);
        fb.extend(&all[..3]);
        assert!(fb.next_frame().unwrap().is_none());
        fb.extend(&all[3..]);
        let f1: Request = decode(&fb.next_frame().unwrap().unwrap()).unwrap();
        let f2: Request = decode(&fb.next_frame().unwrap().unwrap()).unwrap();
        assert_eq!((f1, f2), (Request::Status, Request::ListTabs));
        assert!(fb.next_frame().unwrap().is_none());
    }

    #[test]
    fn hello_with_other_version_is_detectable() {
        let hello = Hello {
            protocol: PROTOCOL_VERSION + 1,
            client_version: "9".into(),
            kind: ClientKind::Cli,
            folder: "/".into(),
            in_tab: None,
            declared_name: None,
            interactive: false,
        };
        let bytes = encode(&hello).unwrap();
        let back: Hello = read_message(&mut &bytes[..]).unwrap().unwrap();
        assert_ne!(back.protocol, PROTOCOL_VERSION);
    }

    #[test]
    fn json_has_schema_first() {
        let v = Output::Done(Done { message: "hi".into() }).to_json();
        let s = serde_json::to_string(&v).unwrap();
        assert!(s.starts_with("{\"schema\":1"), "{s}");
    }
}
