//! Where ctx keeps things, and `~/.config/ctx/config.toml`.
//!
//! `CTX_HOME`, when set, puts everything (data, socket, config) under one folder. It's for tests
//! and for trying ctx without touching your real tabs.

use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Paths {
    /// `~/Library/Application Support/ctx`: saved tabs, index, activity log, launchers.
    pub data: PathBuf,
    /// `<user temp>/ctx-<uid>`: the socket and the shell-integration shim.
    pub runtime: PathBuf,
    pub config: PathBuf,
}

impl Paths {
    /// The server's socket. Unix socket paths are limited to about 100 bytes, so a long runtime
    /// folder (a deep `CTX_HOME`) gets a short stand-in under `/tmp` named after it.
    pub fn socket(&self) -> PathBuf {
        let p = self.runtime.join("server.sock");
        if p.as_os_str().len() < 100 {
            return p;
        }
        let mut h: u64 = 0xcbf29ce484222325;
        for b in self.runtime.as_os_str().as_encoded_bytes() {
            h = (h ^ *b as u64).wrapping_mul(0x100000001b3);
        }
        PathBuf::from(format!("/tmp/ctx-{h:016x}")).join("server.sock")
    }
    pub fn tabs(&self) -> PathBuf {
        self.data.join("tabs")
    }
    pub fn tab_log(&self, number: u64) -> PathBuf {
        self.tabs().join(format!("{number}.ctxlog"))
    }
    pub fn tab_history(&self, number: u64) -> PathBuf {
        self.tabs().join(format!("{number}.zsh_history"))
    }
    pub fn index(&self) -> PathBuf {
        self.data.join("index.sqlite")
    }
    pub fn activity_log(&self) -> PathBuf {
        self.data.join("activity.log")
    }
    pub fn launchers(&self) -> PathBuf {
        self.data.join("launchers")
    }
    pub fn ask_cache(&self) -> PathBuf {
        self.data.join("ask-cache")
    }
    /// Server state between runs (open tabs at shutdown, for "reopen N tabs from last time").
    pub fn state(&self) -> PathBuf {
        self.data.join("server-state.json")
    }
    pub fn server_log(&self) -> PathBuf {
        self.data.join("server.log")
    }

    /// Whether these paths come from `CTX_HOME` (a test or trial setup).
    pub fn is_custom() -> bool {
        std::env::var_os("CTX_HOME").is_some_and(|h| !h.is_empty())
    }

    /// Resolve paths given the per-user temporary folder and home folder.
    pub fn resolve(user_temp: PathBuf, uid: u32, home: &str) -> Paths {
        if let Some(h) = std::env::var_os("CTX_HOME").filter(|h| !h.is_empty()) {
            let h = PathBuf::from(h);
            return Paths { data: h.join("data"), runtime: h.join("run"), config: h.join("config.toml") };
        }
        let config_home = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(home).join(".config"));
        Paths {
            data: PathBuf::from(home).join("Library/Application Support/ctx"),
            runtime: user_temp.join(format!("ctx-{uid}")),
            config: config_home.join("ctx/config.toml"),
        }
    }
}

/// Everything in `config.toml`, with the documented defaults.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Shell for new tabs. Empty: your login shell.
    pub shell: String,
    pub scrollback_mb_per_tab: f64,
    pub scrollback_mb_total: f64,
    pub keep_tabs_when_app_quits: bool,
    pub log_reads: bool,

    pub keep_closed_tabs_days: u64,
    pub saved_total_gb: f64,
    pub saved_output_mb_per_command: f64,
    pub reopen_on_launch: String,
    pub dont_save_env: Vec<String>,

    pub allow_programs_to_clear_scrollback: bool,

    pub ssh_integration: bool,
    pub ssh_integration_skip: Vec<String>,

    pub run_max_lines: u32,
    pub agent_window_name: String,

    pub assist: bool,
    pub assist_command: String,

    pub ask: AskConfig,

    /// CTX.app's own settings. The server ignores them; the app reads them.
    pub app: toml::Table,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct AskConfig {
    pub endpoint: String,
    pub model: String,
    pub api_key_env: String,
    pub max_context_tokens: u32,
    pub max_model_calls: u32,
    pub time_limit: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            shell: String::new(),
            scrollback_mb_per_tab: 8.0,
            scrollback_mb_total: 200.0,
            keep_tabs_when_app_quits: true,
            log_reads: true,
            keep_closed_tabs_days: 365,
            saved_total_gb: 5.0,
            saved_output_mb_per_command: 5.0,
            reopen_on_launch: "ask".into(),
            dont_save_env: Vec::new(),
            allow_programs_to_clear_scrollback: false,
            ssh_integration: true,
            ssh_integration_skip: Vec::new(),
            run_max_lines: 400,
            agent_window_name: "Agents".into(),
            assist: true,
            assist_command: "bro".into(),
            ask: AskConfig::default(),
            app: toml::Table::new(),
        }
    }
}

impl Default for AskConfig {
    fn default() -> Self {
        AskConfig {
            endpoint: String::new(),
            model: String::new(),
            api_key_env: String::new(),
            max_context_tokens: 32_000,
            max_model_calls: 20,
            time_limit: "1 minute".into(),
        }
    }
}

impl Config {
    /// Load the config file. A missing file gives the defaults; a broken one gives an error that
    /// says which line is wrong.
    pub fn load(path: &std::path::Path) -> Result<Config, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => Config::parse(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    pub fn parse(text: &str) -> Result<Config, String> {
        let c: Config = toml::from_str(text).map_err(|e| e.to_string())?;
        if !["ask", "always", "never"].contains(&c.reopen_on_launch.as_str()) {
            return Err("reopen_on_launch must be \"ask\", \"always\" or \"never\"".into());
        }
        Ok(c)
    }

    /// Whether an environment variable name matches a `dont_save_env` pattern (`*` wildcards).
    pub fn env_excluded(&self, name: &str) -> bool {
        self.dont_save_env.iter().any(|p| glob_match(p, name))
    }
}

/// `*` matches any run of characters, `?` one character. Case-sensitive.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_example_config_parses() {
        let text = r#"
shell = "/bin/zsh"
scrollback_mb_per_tab = 8
scrollback_mb_total = 200
keep_tabs_when_app_quits = true
log_reads = true
keep_closed_tabs_days = 365
saved_total_gb = 5
saved_output_mb_per_command = 5
reopen_on_launch = "ask"
dont_save_env = ["*_TOKEN", "AWS_SECRET_*"]
allow_programs_to_clear_scrollback = false
ssh_integration = true
ssh_integration_skip = []
run_max_lines = 400
agent_window_name = "Agents"
assist = true
assist_command = "bro"

[ask]
endpoint = ""
model = ""
api_key_env = ""
max_context_tokens = 32000
max_model_calls = 20
time_limit = "1 minute"
"#;
        let c = Config::parse(text).unwrap();
        assert!(c.env_excluded("GITHUB_TOKEN"));
        assert!(c.env_excluded("AWS_SECRET_ACCESS_KEY"));
        assert!(!c.env_excluded("PATH"));
        assert_eq!(Config::parse("").unwrap(), Config::default());
        assert!(Config::parse("nonsense = 1").is_err());
        let with_app = Config::parse("[app]\nfont = \"SF Mono\"\nfont_size = 13\n").unwrap();
        assert_eq!(with_app.app.get("font_size").and_then(|v| v.as_integer()), Some(13));
    }

    #[test]
    fn globs() {
        assert!(glob_match("*.prod", "web.prod"));
        assert!(glob_match("router", "router"));
        assert!(!glob_match("router", "router2"));
        assert!(glob_match("a*b*c", "aXXbYc"));
    }
}
