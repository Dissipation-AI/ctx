//! The ctx helper: a small root daemon (`ctx-helper`), the only part of ctx with root. It starts
//! an agent's tab as the agent's own account, and makes and removes agents' accounts and spaces.
//!
//! Its one rule lives in a root-owned file, so nothing running as you can change it: which
//! account may ask (the one the ctx server runs as). It starts tabs only as agents' accounts
//! (in ctx-agents, standard, never the owner's or an administrator's).
//!
//! On the wire: one JSON line each way. The request comes with the tab's terminal as an attached
//! file descriptor (`SCM_RIGHTS`).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Where the helper listens, unless `CTX_HELPER_SOCKET` says otherwise (tests).
pub const SOCKET: &str = "/var/run/ctx-helper.sock";
/// The helper's rules, unless `CTX_HELPER_CONFIG` says otherwise (tests).
#[cfg(target_os = "macos")]
pub const CONFIG: &str = "/Library/Application Support/ctx/helper.toml";
#[cfg(not(target_os = "macos"))]
pub const CONFIG: &str = "/etc/ctx/helper.toml";
/// The launchd label (macOS) and systemd unit name (Linux) it's installed under.
pub const LABEL: &str = "com.ctx.helper";

pub fn socket() -> PathBuf {
    std::env::var_os("CTX_HELPER_SOCKET").filter(|s| !s.is_empty()).map(PathBuf::from).unwrap_or_else(|| SOCKET.into())
}

pub fn config_path() -> PathBuf {
    std::env::var_os("CTX_HELPER_CONFIG").filter(|s| !s.is_empty()).map(PathBuf::from).unwrap_or_else(|| CONFIG.into())
}

/// `helper.toml`.
#[derive(Debug, Clone, Deserialize, PartialEq, Default)]
#[serde(default, deny_unknown_fields)]
pub struct HelperConfig {
    /// The account allowed to ask (the one the ctx server runs as).
    pub owner: String,
}

impl HelperConfig {
    pub fn parse(text: &str) -> Result<HelperConfig, String> {
        let c: HelperConfig = toml::from_str(text).map_err(|e| e.to_string())?;
        if c.owner.is_empty() {
            return Err("owner = \"<the account the ctx server runs as>\" is missing".into());
        }
        Ok(c)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpawnRequest {
    /// The agent, whose account (of the same name) the tab runs as.
    pub agent: String,
    pub folder: String,
    pub program: String,
    /// argv, `argv[0]` included.
    pub argv: Vec<String>,
    /// The environment; the helper sets HOME, USER, LOGNAME, SHELL and TMPDIR for the account.
    pub env: Vec<(String, String)>,
    /// The agent's limits.
    pub limits: Limits,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SpawnReply {
    Started { pid: i32 },
    Refused { message: String },
}

/// Who an access-list entry names (mirrors `ctx_accounts::acl::Who`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum EntryFor {
    User(u32),
    Group(u32),
}

/// A grant's state (mirrors `ctx_accounts::acl::Access`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Grant {
    Read,
    Write,
    ReadOnly,
    Nothing,
}

/// A file the helper writes into a new agent's home.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StarterFile {
    /// Relative to the home folder.
    pub path: String,
    pub text: String,
    /// Owned by the asker (you) instead of the agent: `ctx-agent.toml`, which the agent must
    /// not be able to change.
    pub yours: bool,
}

/// Every request besides starting a tab (which is a bare [`SpawnRequest`] on the wire, so older
/// servers and helpers keep understanding each other). Only the owner may ask.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum HelperRequest {
    /// What this helper can do.
    Version,
    /// A new agent account, its home where every account's is (made new: it mustn't exist).
    MakeAccount {
        name: String,
        full_name: String,
        files: Vec<StarterFile>,
    },
    /// A space: its group, and its folder (yours; the group may read and write everything in it).
    MakeSpace {
        name: String,
    },
    /// Delete a space's group. With `trash_folder`, its folder goes to your Trash.
    RemoveSpace {
        name: String,
        trash_folder: bool,
    },
    /// Put an agent in a space, or take it out.
    SetSpaceMember {
        space: String,
        agent: String,
        member: bool,
    },
    /// Delete an agent's account. With `trash_home`, its home goes to your Trash.
    RemoveAccount {
        name: String,
        trash_home: bool,
    },
    SetFullName {
        name: String,
        full_name: String,
    },
    /// Set an access-list entry on `path` and everything in it, as root: for files inside a
    /// folder of yours that agents own. Only files owned by you or an agent, never through links,
    /// never a file with other hard links.
    Share {
        path: String,
        entry: EntryFor,
        grant: Option<Grant>,
    },
    /// Whether an agent's account may read (or write) `path`, as the OS would answer it.
    MayAccess {
        account: String,
        path: String,
        write: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum HelperReply {
    Version { version: u32 },
    Done { message: String },
    Answer { yes: bool },
    Refused { message: String },
}

/// The helper's own version: what it understands. 1 started tabs; 2 makes and removes accounts.
pub const HELPER_VERSION: u32 = 3;

/// Ask the helper something (no terminal attached). Only the owner's account gets answers.
pub fn ask(req: &HelperRequest) -> Result<HelperReply, String> {
    use std::io::{BufRead, Write};
    let path = socket();
    let mut stream = std::os::unix::net::UnixStream::connect(&path).map_err(|e| {
        format!(
            "the ctx helper isn't installed or isn't running ({}: {e}). An administrator installs it once: sudo scripts/install.sh <you>",
            path.display()
        )
    })?;
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(60)));
    let mut line = serde_json::to_vec(req).map_err(|e| e.to_string())?;
    line.push(b'\n');
    stream.write_all(&line).map_err(|e| format!("couldn't ask the ctx helper: {e}"))?;
    let mut answer = String::new();
    std::io::BufReader::new(&stream)
        .read_line(&mut answer)
        .map_err(|e| format!("the ctx helper didn't answer: {e}"))?;
    // An older helper answers anything it doesn't know with a spawn refusal.
    if let Ok(r) = serde_json::from_str::<HelperReply>(&answer) {
        return Ok(r);
    }
    match serde_json::from_str::<SpawnReply>(&answer) {
        Ok(SpawnReply::Refused { message }) => Ok(HelperReply::Refused {
            message: format!(
                "{message} (The installed helper is older than this ctx; an administrator reinstalls it: sudo scripts/install.sh <you>.)"
            ),
        }),
        _ => Err(format!("the ctx helper's answer didn't make sense: {}", answer.trim())),
    }
}

/// `ctx-agent.toml` in an agent's home: what the OS has no place for. It belongs to you (or an
/// administrator of the agent), never to the agent itself.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Default)]
#[serde(default, deny_unknown_fields)]
pub struct AgentFile {
    /// The agent's colour in CTX.app: a name (`teal`, `orange`…) or `#rrggbb`. Empty: one is
    /// picked from the name.
    pub colour: String,
    pub limits: Limits,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    /// Memory in megabytes across the agent's tabs (0: none).
    pub memory_mb: u64,
    /// Processes the account may run at once (0: none).
    pub processes: u64,
    /// Run at low priority.
    pub low_priority: bool,
}

/// The file's name in an agent's home.
pub const AGENT_FILE: &str = "ctx-agent.toml";

impl AgentFile {
    pub fn parse(text: &str) -> Result<AgentFile, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }

    pub fn to_text(&self) -> String {
        let mut s = String::from(
            "# This agent's settings in ctx. It belongs to you (and the agent's administrators); the agent\n# itself can't change it. `ctx limit agent`, `ctx reset agent` and CTX.app's Agents window edit it.\n",
        );
        s.push_str(&toml::to_string(self).unwrap_or_default());
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_file_round_trips() {
        let mut f = AgentFile { colour: "teal".into(), ..Default::default() };
        f.limits.memory_mb = 8192;
        f.limits.low_priority = true;
        assert_eq!(AgentFile::parse(&f.to_text()).unwrap(), f);
        assert_eq!(AgentFile::parse("").unwrap(), AgentFile::default());
        assert!(AgentFile::parse("color = 1").is_err());
    }

    #[test]
    fn rules_parse() {
        assert_eq!(HelperConfig::parse("owner = \"c\"\n").unwrap().owner, "c");
        assert!(HelperConfig::parse("").is_err());
        assert!(HelperConfig::parse("owner = \"c\"\n[accounts]\ndev = \"x\"\n").is_err(), "no other rules");
    }
}
