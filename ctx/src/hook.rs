//! `ctx hook claude`: Claude Code's PreToolUse hook for Bash (integrations/claude). It rewrites
//! the command into `ctx run` in a tab named for the session, so the command runs in a
//! persistent tab a person can watch. If ctx isn't reachable, the command runs as it would have.

use crate::client::Connection;
use crate::exec::Out;
use ctx_protocol::*;
use serde_json::{Value, json};
use std::io::Read;

/// Quote for zsh: single quotes, with `'` written as `'\''`.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The tab a Claude Code session's commands run in.
pub fn tab_name(session_id: &str) -> String {
    let short: String = session_id.chars().filter(|c| c.is_ascii_alphanumeric()).take(8).collect();
    if short.is_empty() { "Claude session".into() } else { format!("Claude {short}") }
}

/// The rewritten Bash input, or `None` to leave the call alone.
pub fn rewrite(input: &Value, tab: &str) -> Option<Value> {
    let tool_input = input.get("tool_input")?;
    let command = tool_input.get("command")?.as_str()?.trim();
    if command.is_empty() || command == "ctx" || command.starts_with("ctx ") {
        return None;
    }
    let background = tool_input.get("run_in_background").and_then(Value::as_bool).unwrap_or(false);
    let mut sentence = format!("ctx run in {} {}", shell_quote(tab), shell_quote(command));
    if background {
        sentence.push_str(" without waiting");
    } else {
        // Stop waiting a little before the Bash tool would give up; the command keeps running.
        let timeout_ms = tool_input.get("timeout").and_then(Value::as_u64).unwrap_or(120_000);
        let secs = (timeout_ms / 1000).saturating_sub(5).max(5);
        sentence.push_str(&format!(" within {secs} seconds"));
    }
    let mut updated = tool_input.clone();
    updated["command"] = Value::String(sentence);
    updated["run_in_background"] = Value::Bool(false);
    Some(updated)
}

pub fn claude() -> Out {
    let mut out = Out::default();
    let mut text = String::new();
    if std::io::stdin().read_to_string(&mut text).is_err() {
        return out;
    }
    let Ok(input) = serde_json::from_str::<Value>(&text) else { return out };
    if input.get("tool_name").and_then(Value::as_str) != Some("Bash") {
        return out;
    }
    if std::env::var("CTX_CLAUDE_HOOK").is_ok_and(|v| v == "deny") {
        out.line(
            json!({"hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": "Shell commands run in ctx tabs here: use the ctx_run tool (or `ctx run in \"<tab>\" \"<command>\"`)."
            }})
            .to_string(),
        );
        return out;
    }
    let session = input.get("session_id").and_then(Value::as_str).unwrap_or("");
    let tab = tab_name(session);
    let Some(updated) = rewrite(&input, &tab) else { return out };
    // Make sure the session's tab exists (in the session's folder); if ctx isn't reachable, leave
    // the command alone.
    let Ok(mut conn) = Connection::open(ClientKind::Cli) else { return out };
    let exists = match conn.request(&Envelope { sentence: "ctx list tabs".into(), request: Request::ListTabs }) {
        Ok(Response::Ok(Output::Tabs(list))) => {
            list.windows.iter().flat_map(|w| &w.tabs).any(|t| t.name.as_deref() == Some(tab.as_str()))
        }
        _ => return out,
    };
    if !exists {
        let folder = input.get("cwd").and_then(Value::as_str).map(str::to_string);
        let opened = conn.request(&Envelope {
            sentence: format!("ctx open tab named \"{tab}\""),
            request: Request::OpenTab { window: None, folder, name: Some(tab.clone()) },
        });
        if !matches!(opened, Ok(Response::Ok(_))) {
            return out;
        }
    }
    out.line(json!({"hookSpecificOutput": {"hookEventName": "PreToolUse", "updatedInput": updated}}).to_string());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_into_ctx_run() {
        let input = json!({"session_id": "abc12345-xyz", "tool_name": "Bash", "tool_input": {"command": "echo 'hi' && ls", "timeout": 60000, "description": "d"}});
        let tab = tab_name("abc12345-xyz");
        assert_eq!(tab, "Claude abc12345");
        let out = rewrite(&input, &tab).unwrap();
        assert_eq!(out["command"], "ctx run in 'Claude abc12345' 'echo '\\''hi'\\'' && ls' within 55 seconds");
        assert_eq!(out["description"], "d");
        let bg = json!({"tool_input": {"command": "npm run dev", "run_in_background": true}});
        assert_eq!(rewrite(&bg, "T").unwrap()["command"], "ctx run in 'T' 'npm run dev' without waiting");
        assert!(rewrite(&json!({"tool_input": {"command": "ctx list tabs"}}), "T").is_none());
    }
}
