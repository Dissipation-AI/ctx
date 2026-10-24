//! The ctx helper: a small root daemon (`ctx-helper`) that starts an agent's tab as the agent's
//! own macOS account. It's the only part of ctx with root, and it does one thing.
//!
//! Its rules live in a root-owned file, so nothing running as you can widen them: which account
//! may ask (the one the ctx server runs as) and which account each agent runs as. An agent that
//! runs as root also needs an administrator's approval every time it starts: the request carries
//! an Authorization (macOS asks an admin for their password or Touch ID), and the helper uses
//! it up.
//!
//! On the wire: one JSON line each way. The request comes with the tab's terminal as an attached
//! file descriptor (`SCM_RIGHTS`).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Where the helper listens, unless `CTX_HELPER_SOCKET` says otherwise (tests).
pub const SOCKET: &str = "/var/run/ctx-helper.sock";
/// The helper's rules, unless `CTX_HELPER_CONFIG` says otherwise (tests).
pub const CONFIG: &str = "/Library/Application Support/ctx/helper.toml";
/// The launchd label it's installed under.
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
    /// Agent name → the account it runs as.
    pub accounts: BTreeMap<String, String>,
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
    pub agent: String,
    /// The account the server expects; the helper checks it against its own rules.
    pub account: String,
    pub folder: String,
    pub program: String,
    /// argv, `argv[0]` included.
    pub argv: Vec<String>,
    /// The environment; the helper sets HOME, USER, LOGNAME, SHELL and TMPDIR for the account.
    pub env: Vec<(String, String)>,
    /// A Seatbelt profile to apply before the program starts (empty: none).
    pub sandbox: String,
    /// An Authorization's external form (32 bytes) granting `system.privilege.admin`, needed for
    /// root.
    pub authorization: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SpawnReply {
    Started { pid: i32 },
    Refused { message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_parse() {
        let c = HelperConfig::parse("owner = \"c\"\n[accounts]\ndev = \"vibe-daemon\"\nsysadmin = \"root\"\n").unwrap();
        assert_eq!(c.accounts["dev"], "vibe-daemon");
        assert!(HelperConfig::parse("[accounts]\n").is_err());
    }
}
