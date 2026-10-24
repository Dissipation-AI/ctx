//! `ctx list agents`: the agents in config.toml and what each may reach (see
//! `ctx-protocol/src/agents.rs` for how they're enforced).

use crate::exec::Out;
use ctx_cli::parse::Format;
use ctx_protocol::config::Config;
use serde::Serialize;

#[derive(Serialize)]
struct AgentInfo {
    name: String,
    folders: Vec<String>,
    launcher: Option<String>,
    /// The macOS account it runs as; `None`: as you.
    account: Option<String>,
    /// Whether that account exists on this Mac.
    account_exists: Option<bool>,
    sandbox: bool,
    read: Vec<String>,
    write: Vec<String>,
    deny: Vec<String>,
    network: bool,
    see_tabs_of: Vec<String>,
    drive_tabs_of: Vec<String>,
}

#[derive(Serialize)]
struct AgentList {
    schema: u32,
    agents: Vec<AgentInfo>,
    default_agent: Option<String>,
    agent_programs: Vec<String>,
    /// Whether the ctx helper (which starts agents as their accounts) answers.
    helper_running: bool,
}

pub fn list(format: Format) -> Out {
    let mut out = Out::default();
    let config = match Config::load(&crate::client::paths().config) {
        Ok(c) => c,
        Err(e) => {
            out.err(e);
            return out.code(1);
        }
    };
    let helper_running = std::os::unix::net::UnixStream::connect(ctx_protocol::helper::socket()).is_ok();
    let list = AgentList {
        schema: ctx_protocol::SCHEMA_VERSION,
        agents: config
            .agents
            .iter()
            .map(|(name, a)| AgentInfo {
                name: name.clone(),
                folders: a.folders.clone(),
                launcher: Some(a.launcher.clone()).filter(|l| !l.is_empty()),
                account: Some(a.account.clone()).filter(|l| !l.is_empty()),
                account_exists: Some(&a.account)
                    .filter(|l| !l.is_empty())
                    .map(|acct| ctx_server::sys::account(acct).is_some()),
                sandbox: a.sandbox,
                read: a.read.clone(),
                write: a.write.clone(),
                deny: a.deny.clone(),
                network: a.network,
                see_tabs_of: a.see_tabs_of.clone(),
                drive_tabs_of: a.drive_tabs_of.clone(),
            })
            .collect(),
        default_agent: Some(config.default_agent.clone()).filter(|d| !d.is_empty()),
        agent_programs: config.agent_programs.clone(),
        helper_running,
    };
    if format != Format::Text {
        let v = serde_json::to_value(&list).unwrap_or_default();
        out.line(if format == Format::Toon {
            ctx_protocol::toon::encode(&v)
        } else {
            serde_json::to_string_pretty(&v).unwrap_or_default()
        });
        return out;
    }
    if list.agents.is_empty() {
        out.line(
            "No agents yet: agent programs run as you. Add [agents.<name>] to config.toml (see README, \"Agents\").",
        );
        return out;
    }
    for a in &list.agents {
        let mut head = a.name.clone();
        if list.default_agent.as_deref() == Some(&a.name) {
            head.push_str(" (the default)");
        }
        out.line(head);
        if !a.folders.is_empty() {
            out.line(format!("  works in {}", a.folders.join(", ")));
        }
        if let Some(l) = &a.launcher {
            out.line(format!("  started by the launcher \"{l}\""));
        }
        match (&a.account, a.account_exists) {
            (Some(acct), Some(true)) => out.line(format!(
                "  runs as the account {acct}{}",
                if list.helper_running { "" } else { " (the ctx helper isn't running, so its tabs can't start)" }
            )),
            (Some(acct), _) => out.line(format!("  runs as the account {acct}, which doesn't exist on this Mac")),
            (None, _) => out.line("  runs as you"),
        }
        if a.sandbox {
            let mut parts = Vec::new();
            for (verb, list) in [("reads", &a.read), ("writes", &a.write), ("never touches", &a.deny)] {
                if !list.is_empty() {
                    parts.push(format!("{verb} {}", list.join(", ")));
                }
            }
            let grants = if parts.is_empty() { "nothing".into() } else { parts.join("; ") };
            out.line(format!("  sandbox, in your home (plus [agent_base]): {grants}"));
            if !a.network {
                out.line("  no network");
            }
        } else {
            out.line("  no sandbox");
        }
        let whose = |v: &[String]| -> Option<String> {
            let names: Vec<String> = v
                .iter()
                .map(|o| match o.as_str() {
                    "*" => "everyone's".to_string(),
                    "you" => "yours".to_string(),
                    o => format!("{o}'s"),
                })
                .collect();
            (!names.is_empty()).then(|| names.join(", "))
        };
        let mut tabs = "  tabs: its own".to_string();
        if let Some(w) = whose(&a.drive_tabs_of) {
            tabs.push_str(&format!("; drives {w}"));
        }
        if let Some(w) = whose(&a.see_tabs_of) {
            tabs.push_str(&format!("; sees {w}"));
        }
        out.line(tabs);
    }
    out.line(format!(
        "Agent programs ({}) started in your tabs run as the agent for their folder{}.",
        list.agent_programs.join(", "),
        match &list.default_agent {
            Some(d) => format!(", else as {d}"),
            None => ", else as you".into(),
        }
    ));
    out
}
