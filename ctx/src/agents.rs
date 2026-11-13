//! `ctx list agents`: the agents' accounts (accounts in ctx-agents).

use crate::exec::Out;
use ctx_cli::parse::Format;
use serde::Serialize;

#[derive(Serialize)]
struct AgentList {
    schema: u32,
    agents: Vec<crate::accounts::AgentShown>,
    /// Whether the ctx helper (which starts agents as their accounts) answers.
    helper_running: bool,
}

pub fn list(format: Format) -> Out {
    let mut out = Out::default();
    let list = AgentList {
        schema: ctx_protocol::SCHEMA_VERSION,
        agents: ctx_accounts::accounts::agents().iter().map(crate::accounts::shown).collect(),
        helper_running: std::os::unix::net::UnixStream::connect(ctx_protocol::helper::socket()).is_ok(),
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
    for a in &list.agents {
        out.line(crate::accounts::summary(a));
    }
    if list.agents.is_empty() {
        out.line("No agents yet. Make one: ctx make agent named \"<name>\" (its own account, linked in ~/Agents).");
    } else if !list.helper_running {
        out.line("The ctx helper isn't running, so agents' tabs can't start. An administrator installs it once: sudo scripts/install-helper.sh <you>");
    }
    out
}
