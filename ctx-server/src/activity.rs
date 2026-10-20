//! Who is asking: the client's identity, verified from the OS (socket peer pid and its parent
//! chain), and the activity log of everything clients did.

use crate::sys;
use ctx_protocol::ActivityEntry;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

/// Programs ctx recognises as agents when it walks a client's parent chain.
pub const AGENTS: &[&str] =
    &["claude", "codex", "gemini", "agy", "antigravity", "cursor-agent", "aider", "opencode", "goose"];

/// A client as the OS reports it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Identity {
    /// The nearest recognisable program: an agent, CTX.app, or the client itself.
    pub program: String,
    pub pid: i32,
    pub start_us: u64,
    /// Tab the client runs in (verified from its process tree).
    pub in_tab: Option<u64>,
    /// `CTX_AGENT_NAME`, claimed by the client.
    pub declared: Option<String>,
    pub is_agent: bool,
    pub is_app: bool,
    /// The client process itself.
    pub client_pid: i32,
}

impl Identity {
    /// `claude (pid 5512) in tab 2`.
    pub fn describe(&self) -> String {
        let mut s = format!("{} (pid {})", self.program, self.pid);
        if let Some(t) = self.in_tab {
            s.push_str(&format!(" in tab {t}"));
        }
        if let Some(d) = &self.declared {
            s.push_str(&format!(", calling itself \"{d}\""));
        }
        s
    }

    /// What `ran_by` records: `you` for a person, else `claude (pid 5512)`.
    pub fn ran_by(&self) -> String {
        if self.is_agent { format!("{} (pid {})", self.program, self.pid) } else { "you".into() }
    }

    pub fn unknown() -> Identity {
        Identity {
            program: "unknown".into(),
            pid: 0,
            start_us: 0,
            in_tab: None,
            declared: None,
            is_agent: false,
            is_app: false,
            client_pid: 0,
        }
    }
}

fn matches_agent(word: &str) -> Option<&'static str> {
    let w = word.to_lowercase();
    AGENTS.iter().copied().find(|a| w == *a || w.starts_with(&format!("{a}-")) || w.starts_with(&format!("{a}.")))
}

/// The agent a process is, judged by its name, its argv[0] and its executable's path (Claude
/// Code's binary is named after its version, e.g. `~/.local/share/claude/versions/2.1.296`).
fn agent_name(p: &sys::ProcInfo) -> Option<String> {
    if let Some(a) = matches_agent(&p.name) {
        return Some(a.to_string());
    }
    let args = sys::proc_args(p.pid).unwrap_or_default();
    if let Some(arg0) = args.first()
        && let Some(a) = matches_agent(arg0.rsplit('/').next().unwrap_or(arg0))
    {
        return Some(a.to_string());
    }
    if let Some(path) = sys::proc_path(p.pid) {
        let path = path.display().to_string().to_lowercase();
        for a in AGENTS {
            if path.contains(&format!("/{a}/versions/")) || path.ends_with(&format!("/{a}")) {
                return Some(a.to_string());
            }
        }
    }
    // Agents that run as node, bun or python scripts: look at the script.
    let name = p.name.to_lowercase();
    if ["node", "bun", "python", "python3", "deno"].contains(&name.as_str()) {
        for a in args.iter().skip(1).take(3) {
            let base = a.rsplit('/').next().unwrap_or(a);
            if let Some(agent) = matches_agent(base) {
                return Some(agent.to_string());
            }
            if a.contains("gemini-cli") {
                return Some("gemini".into());
            }
            for ag in AGENTS {
                if a.contains(&format!("/{ag}/")) || a.contains(&format!("@{ag}")) {
                    return Some(ag.to_string());
                }
            }
        }
    }
    None
}

/// Identify a client from its pid. `tab_of` maps a process id to the tab whose launcher it is
/// (or whose shell it is).
pub fn identify(client_pid: i32, declared: Option<String>, tab_of: &dyn Fn(i32) -> Option<u64>) -> Identity {
    let chain = sys::ancestors(client_pid);
    let mut in_tab = None;
    let mut agent: Option<(String, i32, u64)> = None;
    let mut is_app = false;
    for p in &chain {
        if let Some(t) = tab_of(p.pid) {
            in_tab = Some(t);
            break;
        }
        if agent.is_none()
            && let Some(a) = agent_name(p)
        {
            agent = Some((a, p.pid, p.start_us));
        }
        if p.name == "CTX" || p.name.starts_with("CTX.") {
            is_app = true;
        }
    }
    let me = chain.first().cloned();
    match agent {
        Some((program, pid, start_us)) => {
            Identity { program, pid, start_us, in_tab, declared, is_agent: true, is_app: false, client_pid }
        }
        None => {
            let (program, pid, start_us) = match &me {
                Some(p) => (p.name.clone(), p.pid, p.start_us),
                None => ("unknown".into(), client_pid, 0),
            };
            // A declared agent name counts as an agent even without a recognised program.
            let is_agent = declared.is_some();
            Identity { program, pid, start_us, in_tab, declared, is_agent, is_app, client_pid }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogLine {
    pub at: String,
    pub at_ms: i64,
    pub client: String,
    pub program: String,
    pub pid: i32,
    pub start_us: u64,
    pub declared_name: Option<String>,
    pub ran_by: String,
    pub sentence: String,
    pub tab: Option<u64>,
    pub typed: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub typed_hidden: bool,
}

impl LogLine {
    pub fn entry(&self) -> ActivityEntry {
        ActivityEntry {
            at: self.at.clone(),
            client: self.client.clone(),
            program: self.program.clone(),
            pid: self.pid,
            declared_name: self.declared_name.clone(),
            sentence: self.sentence.clone(),
            tab: self.tab,
            typed: self.typed.clone(),
            typed_hidden: self.typed_hidden,
        }
    }
}

/// `activity.log`: JSON lines, rotated at 10 MB (the old file is kept as `activity.log.1`).
#[derive(Debug)]
pub struct ActivityLog {
    path: PathBuf,
    file: Option<File>,
    size: u64,
}

const ROTATE_BYTES: u64 = 10 * 1024 * 1024;

impl ActivityLog {
    pub fn open(path: &Path) -> ActivityLog {
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let file = OpenOptions::new().create(true).append(true).mode(0o600).open(path).ok();
        let size = file.as_ref().and_then(|f| f.metadata().ok()).map(|m| m.len()).unwrap_or(0);
        ActivityLog { path: path.to_path_buf(), file, size }
    }

    pub fn append(&mut self, line: &LogLine) {
        let Ok(mut text) = serde_json::to_string(line) else { return };
        text.push('\n');
        if self.size + text.len() as u64 > ROTATE_BYTES {
            let _ = std::fs::rename(&self.path, self.path.with_extension("log.1"));
            self.file = OpenOptions::new().create(true).append(true).mode(0o600).open(&self.path).ok();
            self.size = 0;
        }
        if let Some(f) = &mut self.file
            && f.write_all(text.as_bytes()).is_ok()
        {
            self.size += text.len() as u64;
        }
    }

    /// Entries, newest first, that pass `keep`.
    pub fn read(&self, limit: usize, keep: &dyn Fn(&LogLine) -> bool) -> Vec<LogLine> {
        let mut out = Vec::new();
        for path in [self.path.clone(), self.path.with_extension("log.1")] {
            let Ok(f) = File::open(&path) else { continue };
            let mut lines: Vec<LogLine> =
                BufReader::new(f).lines().map_while(Result::ok).filter_map(|l| serde_json::from_str(&l).ok()).collect();
            lines.reverse();
            for l in lines {
                if keep(&l) {
                    out.push(l);
                    if out.len() >= limit {
                        return out;
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifies_this_process() {
        let me = std::process::id() as i32;
        let id = identify(me, None, &|_| None);
        assert_eq!(id.client_pid, me);
        assert!(id.pid > 0);
        let in_tab = identify(me, Some("Claude - x".into()), &|p| if p == me { Some(7) } else { None });
        assert_eq!(in_tab.in_tab, Some(7));
        assert!(in_tab.is_agent, "a declared name makes it an agent");
        assert!(in_tab.describe().contains("calling itself \"Claude - x\""));
    }

    #[test]
    fn log_round_trip_and_filter() {
        let dir = std::env::temp_dir().join(format!("ctx-activity-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut log = ActivityLog::open(&dir.join("activity.log"));
        for i in 0..5 {
            log.append(&LogLine {
                at: format!("t{i}"),
                at_ms: i,
                client: "claude (pid 1)".into(),
                program: if i % 2 == 0 { "claude".into() } else { "zsh".into() },
                pid: 1,
                start_us: 0,
                declared_name: None,
                ran_by: "x".into(),
                sentence: format!("ctx list tabs {i}"),
                tab: Some(i as u64),
                typed: None,
                typed_hidden: false,
            });
        }
        let got = log.read(10, &|l| l.program == "claude");
        assert_eq!(got.iter().map(|l| l.at_ms).collect::<Vec<_>>(), vec![4, 2, 0]);
    }
}
