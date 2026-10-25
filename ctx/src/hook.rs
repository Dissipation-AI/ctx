//! `ctx hook <agent>`: agents' pre-tool hooks for their shell tool (integrations/<agent>). Each
//! rewrites the command into `ctx run` in a tab named for the session, so the command runs in a
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

/// A tab name from an agent's prefix and its session id.
fn session_tab(prefix: &str, session_id: &str) -> String {
    let short: String = session_id.chars().filter(|c| c.is_ascii_alphanumeric()).take(8).collect();
    if short.is_empty() { format!("{prefix} session") } else { format!("{prefix} {short}") }
}

/// The tab a Claude Code session's commands run in.
pub fn tab_name(session_id: &str) -> String {
    session_tab("Claude", session_id)
}

/// The tab an Antigravity conversation's commands run in.
pub fn antigravity_tab_name(conversation_id: &str) -> String {
    session_tab("Antigravity", conversation_id)
}

/// Whether a command is a single ctx sentence, which runs as is rather than inside a tab. Anything
/// that chains, pipes, redirects or substitutes outside quotes (`ctx list tabs && rm x`) goes to the
/// tab like any other command, so it's recorded.
pub fn is_ctx_sentence(command: &str) -> bool {
    if command != "ctx" && !command.starts_with("ctx ") {
        return false;
    }
    let (mut single, mut double, mut escaped) = (false, false, false);
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' if !single => escaped = true,
            '\'' if !double => single = !single,
            '"' if !single => double = !double,
            '`' if !single => return false,
            '$' if !single && chars.peek() == Some(&'(') => return false,
            ';' | '&' | '|' | '<' | '>' | '\n' | '(' | ')' if !single && !double => return false,
            _ => {}
        }
    }
    !single && !double
}

/// The tab a call runs in, and its folder, or `None` if ctx isn't reachable (the caller then leaves
/// the command alone). That is the session's tab, opened in `folder` if there isn't one, unless it's
/// busy or the call runs in the background (`background` is then its command): those go to an idle
/// copy of the session tab, or a new one beside it, so the session tab never blocks the agent.
/// The tab is the agent's own: when a tab of that name belongs to someone else (the session started
/// before the agent was configured, or was resumed), the agent's is named `<tab> <agent>`.
fn session_tab_folder(tab: &str, folder: Option<String>, background: Option<&str>) -> Option<(String, String)> {
    let mut conn = Connection::open(ClientKind::Hook).ok()?;
    let list = match conn.request(&Envelope { sentence: "ctx list tabs".into(), request: Request::ListTabs }) {
        Ok(Response::Ok(Output::Tabs(list))) => list,
        _ => return None,
    };
    let tabs: Vec<&TabInfo> = list.windows.iter().flat_map(|w| &w.tabs).collect();
    let named = |name: &str| tabs.iter().filter(|t| t.name.as_deref() == Some(name)).copied().collect::<Vec<_>>();
    // You may drive every tab; an agent its own.
    let mine = |t: &&TabInfo| list.caller.is_none() || t.owner == list.caller;
    // Tab names take letters, digits, spaces, - and _.
    let who: String = list
        .caller
        .as_deref()
        .unwrap_or(ctx_protocol::agents::YOU)
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '_' { c } else { '-' })
        .collect();
    let own = format!("{tab} {who}");
    let session = [tab, own.as_str()].into_iter().find_map(|name| {
        named(name).into_iter().find(mine).map(|t| (name.to_string(), t.folder.clone(), t.state == "idle"))
    });
    let (session, session_folder, idle) = match session {
        Some(s) => s,
        None => {
            let name = if named(tab).is_empty() { tab.to_string() } else { own };
            let opened = conn.request(&Envelope {
                sentence: format!("ctx open tab named \"{name}\""),
                request: Request::OpenTab {
                    window: None,
                    folder: folder.clone(),
                    name: Some(name.clone()),
                    agent: None,
                    like: None,
                },
            });
            if !matches!(opened, Ok(Response::Ok(_))) {
                return None;
            }
            (name, folder.unwrap_or_default(), true)
        }
    };
    if idle && background.is_none() {
        return Some((session, session_folder));
    }
    let base = match background {
        Some(command) => format!("{session} - {}", background_label(command)),
        None => session.clone(),
    };
    // `<base>` (background calls only), then `<base> 2`, `<base> 3`…: the first that is ours and idle, or
    // the first free name, opened as a copy of the session tab.
    let first = if background.is_some() { 1 } else { 2 };
    for k in first..first + 50 {
        let name = if k == 1 { base.clone() } else { format!("{base} {k}") };
        match named(&name).first() {
            Some(t) if mine(t) && t.state == "idle" => return Some((name, t.folder.clone())),
            Some(_) => continue,
            None => {}
        }
        let opened = conn.request(&Envelope {
            sentence: format!("ctx open tab like \"{session}\" named \"{name}\""),
            request: Request::OpenTab {
                window: None,
                folder: None,
                name: Some(name.clone()),
                agent: None,
                like: Some(TabRef::Name(session.clone())),
            },
        });
        return match opened {
            Ok(Response::Ok(Output::Opened(o))) => Some((name, o.folder)),
            _ => None,
        };
    }
    None
}

/// A tab-name-safe label for a background command: its first words, up to about 30 characters
/// (`npm run dev`, `sleep 1800 gh run watch`).
fn background_label(command: &str) -> String {
    let cleaned: String = command
        .lines()
        .next()
        .unwrap_or("")
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { ' ' })
        .collect();
    let mut label = String::new();
    for word in cleaned.split_whitespace() {
        if !label.is_empty() && label.len() + 1 + word.len() > 30 {
            break;
        }
        if !label.is_empty() {
            label.push(' ');
        }
        label.push_str(&word.chars().take(30).collect::<String>());
    }
    if label.chars().any(char::is_alphabetic) { label } else { "background".into() }
}

/// The rewritten Bash input, or `None` to leave the call alone.
pub fn rewrite(input: &Value, tab: &str) -> Option<Value> {
    let tool_input = input.get("tool_input")?;
    let command = tool_input.get("command")?.as_str()?.trim();
    if command.is_empty() || is_ctx_sentence(command) {
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
    let session = input.get("session_id").and_then(Value::as_str).unwrap_or("");
    if rewrite(&input, "").is_none() {
        return out;
    }
    let folder = input.get("cwd").and_then(Value::as_str).map(str::to_string);
    let tool_input = &input["tool_input"];
    let background = tool_input
        .get("run_in_background")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        .then(|| str_at(tool_input, "command"))
        .flatten();
    let Some((tab, _)) = session_tab_folder(&tab_name(session), folder, background) else { return out };
    let Some(updated) = rewrite(&input, &tab) else { return out };
    out.line(json!({"hookSpecificOutput": {"hookEventName": "PreToolUse", "updatedInput": updated}}).to_string());
    out
}

/// A shell call, whichever agent made it.
#[derive(Debug, Default, PartialEq)]
pub struct ShellCall<'a> {
    pub session: &'a str,
    pub command: &'a str,
    /// The folder the agent wants the command run in.
    pub cwd: Option<&'a str>,
    pub background: bool,
    /// How long the agent's tool waits before giving up.
    pub timeout_ms: Option<u64>,
}

/// The `ctx run` sentence for a call, given the tab and the folder it's in now. The tab keeps its
/// folder between commands, so the command changes folder first when the call names another one.
/// A foreground command stops being waited on a little before the agent's tool would give up; it
/// keeps running in the tab.
pub fn sentence(tab: &str, tab_folder: &str, call: &ShellCall) -> String {
    let command = match call.cwd {
        Some(cwd) if !cwd.is_empty() && cwd.trim_end_matches('/') != tab_folder.trim_end_matches('/') => {
            format!("cd {} && {}", shell_quote(cwd), call.command)
        }
        _ => call.command.to_string(),
    };
    let mut sentence = format!("ctx run in {} {}", shell_quote(tab), shell_quote(&command));
    if call.background {
        sentence.push_str(" without waiting");
    } else if let Some(ms) = call.timeout_ms.filter(|ms| *ms > 0) {
        sentence.push_str(&format!(" within {} seconds", (ms / 1000).saturating_sub(5).max(5)));
    }
    sentence
}

/// The rewritten command for a call (making sure its tab exists), or `None` to leave it alone.
fn route(prefix: &str, call: &ShellCall) -> Option<String> {
    let command = call.command.trim();
    if command.is_empty() || is_ctx_sentence(command) {
        return None;
    }
    let background = call.background.then_some(command);
    let (tab, folder) =
        session_tab_folder(&session_tab(prefix, call.session), call.cwd.map(str::to_string), background)?;
    Some(sentence(&tab, &folder, &ShellCall { command, ..*call }))
}

/// The tab prefix for an MCP client: the first word of its name, capitalized (`gemini-cli-mcp-client`
/// → `Gemini`).
fn client_prefix(client: &str) -> String {
    let word: String = client.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
        None => "Agent".into(),
    }
}

/// `ctx mcp`'s `shell` tool: the command runs in a tab named for the client and `session` (opened in
/// `folder` on first use); a single ctx sentence runs as is, as with the hooks.
pub fn mcp(call: &ctx_mcp::Call, session: &str, folder: &str) -> Out {
    let command = call.command.trim();
    let words = if is_ctx_sentence(command) {
        match ctx_mcp::shell_words(command) {
            Ok(w) => w[1..].to_vec(),
            Err(e) => {
                let mut out = Out::default();
                out.err(e);
                out.ctx_error = true;
                return out.code(2);
            }
        }
    } else {
        let Some((tab, _)) =
            session_tab_folder(&session_tab(&client_prefix(call.client), session), Some(folder.to_string()), None)
        else {
            let mut out = Out::default();
            out.err("ctx's server isn't reachable, so the command didn't run.");
            out.ctx_error = true;
            return out.code(1);
        };
        let secs = (call.timeout_ms / 1000).max(1);
        ["run", "in", &tab, command, "within", &secs.to_string(), "seconds"].map(str::to_string).to_vec()
    };
    crate::exec::run(&words, crate::exec::Caller::Mcp)
}

fn stdin_json() -> Value {
    let mut text = String::new();
    match std::io::stdin().read_to_string(&mut text) {
        Ok(_) => serde_json::from_str(&text).unwrap_or(Value::Null),
        Err(_) => Value::Null,
    }
}

fn str_at<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

/// The OpenAI Codex CLI's PreToolUse hook (Claude's format). Codex takes only `command` from
/// `updatedInput`, and only alongside `allow`, which doesn't skip its approval or sandbox.
pub fn codex() -> Out {
    let mut out = Out::default();
    let input = stdin_json();
    let tool_input = &input["tool_input"];
    if str_at(&input, "tool_name") != Some("Bash") {
        return out;
    }
    let Some(command) = str_at(tool_input, "command") else { return out };
    let call = ShellCall {
        session: str_at(&input, "session_id").unwrap_or(""),
        command,
        cwd: str_at(tool_input, "workdir").or_else(|| str_at(&input, "cwd")),
        ..Default::default()
    };
    if let Some(command) = route("Codex", &call) {
        out.line(
            json!({"hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "allow",
                "updatedInput": {"command": command}
            }})
            .to_string(),
        );
    }
    out
}

/// The Cursor CLI's `preToolUse` hook for its Shell tool. It never sets `permission`, so Cursor's
/// own approval still applies.
pub fn cursor() -> Out {
    let mut out = Out::default();
    let input = stdin_json();
    let tool_input = &input["tool_input"];
    let routed = (str_at(&input, "tool_name") == Some("Shell"))
        .then(|| str_at(tool_input, "command"))
        .flatten()
        .and_then(|command| {
            route(
                "Cursor",
                &ShellCall {
                    session: str_at(&input, "conversation_id").unwrap_or(""),
                    command,
                    cwd: str_at(tool_input, "working_directory").or_else(|| str_at(&input, "cwd")),
                    background: tool_input.get("is_background").and_then(Value::as_bool).unwrap_or(false),
                    timeout_ms: tool_input.get("timeout").and_then(Value::as_u64),
                },
            )
        });
    match routed {
        Some(command) => {
            let mut updated = tool_input.clone();
            updated["command"] = Value::String(command);
            if updated.get("is_background").is_some() {
                updated["is_background"] = Value::Bool(false);
            }
            out.line(json!({ "updated_input": updated }).to_string());
        }
        None => out.line("{}"),
    }
    out
}

/// For the pi and opencode plugins (integrations/pi, integrations/opencode), which hand over
/// `{session_id, cwd, command, background, timeout_ms}` and get back `{command}` (or `{}`).
pub fn plugin(prefix: &str) -> Out {
    let mut out = Out::default();
    let input = stdin_json();
    let routed = str_at(&input, "command").and_then(|command| {
        route(
            prefix,
            &ShellCall {
                session: str_at(&input, "session_id").unwrap_or(""),
                command,
                cwd: str_at(&input, "cwd"),
                background: input.get("background").and_then(Value::as_bool).unwrap_or(false),
                timeout_ms: input.get("timeout_ms").and_then(Value::as_u64),
            },
        )
    });
    match routed {
        Some(command) => out.line(json!({ "command": command }).to_string()),
        None => out.line("{}"),
    }
    out
}

/// `ctx hook <agent>`, or `None` for an agent ctx has no hook for.
pub fn run(agent: &str) -> Option<Out> {
    Some(match agent {
        "claude" => claude(),
        "antigravity" => antigravity(),
        "codex" => codex(),
        "cursor" => cursor(),
        "pi" => plugin("Pi"),
        "opencode" => plugin("OpenCode"),
        _ => return None,
    })
}

/// The command in an Antigravity `run_command` call, if it should go to a tab.
fn antigravity_command(input: &Value) -> Option<&str> {
    let call = input.get("toolCall")?;
    if call.get("name").and_then(Value::as_str) != Some("run_command") {
        return None;
    }
    let command = call.get("args")?.get("CommandLine")?.as_str()?.trim();
    (!command.is_empty() && !is_ctx_sentence(command)).then_some(command)
}

/// The arguments to overwrite in an Antigravity `run_command` call. Each call names its own `Cwd`,
/// but the tab keeps its folder between commands, so the command changes folder first when they
/// differ. Non-blocking calls (servers, watchers) return at once. The command runs in the tab, outside
/// Antigravity's sandbox whatever the call asks, and the sandbox only keeps `ctx` from reaching its
/// server, so the call says so (`BypassSandbox`); the person's approval is the check.
pub fn rewrite_antigravity(input: &Value, tab: &str, tab_folder: &str) -> Option<Value> {
    let command = antigravity_command(input)?;
    let args = &input["toolCall"]["args"];
    let command = match args.get("Cwd").and_then(Value::as_str) {
        Some(cwd) if !cwd.is_empty() && cwd.trim_end_matches('/') != tab_folder.trim_end_matches('/') => {
            format!("cd {} && {command}", shell_quote(cwd))
        }
        _ => command.to_string(),
    };
    let mut sentence = format!("ctx run in {} {}", shell_quote(tab), shell_quote(&command));
    if args.get("Blocking").and_then(Value::as_bool) == Some(false) {
        sentence.push_str(" without waiting");
    }
    Some(json!({ "CommandLine": sentence, "BypassSandbox": true }))
}

/// Antigravity treats a reply without a decision as a denial, and `allow` skips the person's
/// approval, so every reply is `ask`: prompt as usual, honouring what they've already allowed.
const ASK: &str = "ask";

/// Antigravity's PreToolUse hook.
pub fn antigravity() -> Out {
    let mut out = Out::default();
    let mut text = String::new();
    let input = match std::io::stdin().read_to_string(&mut text) {
        Ok(_) => serde_json::from_str::<Value>(&text).unwrap_or(Value::Null),
        Err(_) => Value::Null,
    };
    if antigravity_command(&input).is_none() {
        out.line(json!({ "decision": ASK }).to_string());
        return out;
    }
    if std::env::var("CTX_ANTIGRAVITY_HOOK").is_ok_and(|v| v == "deny") {
        out.line(
            json!({
                "decision": "deny",
                "reason": "Shell commands run in ctx tabs here: use `ctx run in \"<tab>\" \"<command>\"`."
            })
            .to_string(),
        );
        return out;
    }
    let conversation = input.get("conversationId").and_then(Value::as_str).unwrap_or("");
    let tab = antigravity_tab_name(conversation);
    let folder = input["toolCall"]["args"]
        .get("Cwd")
        .or_else(|| input.get("workspacePaths").and_then(|w| w.get(0)))
        .and_then(Value::as_str)
        .map(str::to_string);
    let args = &input["toolCall"]["args"];
    let background =
        (args.get("Blocking").and_then(Value::as_bool) == Some(false)).then(|| antigravity_command(&input)).flatten();
    let overwrite =
        session_tab_folder(&tab, folder, background).and_then(|(tab, f)| rewrite_antigravity(&input, &tab, &f));
    match overwrite {
        Some(overwrite) => out.line(json!({ "decision": ASK, "overwrite": overwrite }).to_string()),
        None => out.line(json!({ "decision": ASK }).to_string()),
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_tabs_are_named_for_their_command() {
        assert_eq!(background_label("npm run dev"), "npm run dev");
        assert_eq!(background_label("sleep 1800 && gh run watch 123 --exit-status"), "sleep 1800 gh run watch 123");
        assert_eq!(background_label("cd /w/site && python3 -m http.server 8000"), "cd w site python3 -m http");
        assert_eq!(background_label("./a.out\necho done"), "a out");
        assert_eq!(background_label("123 456"), "background");
    }

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

    #[test]
    fn mcp_tabs_are_named_for_the_client() {
        assert_eq!(client_prefix("gemini-cli-mcp-client"), "Gemini");
        assert_eq!(client_prefix("claude-ai"), "Claude");
        assert_eq!(client_prefix(""), "Agent");
    }

    #[test]
    fn only_single_ctx_sentences_skip_the_tab() {
        assert!(is_ctx_sentence("ctx list tabs"));
        assert!(is_ctx_sentence(r#"ctx run in "Build" "cargo build && cargo test""#));
        assert!(is_ctx_sentence("ctx run in 'T' 'a | b; c'"));
        assert!(!is_ctx_sentence("ctx list tabs && rm -rf build"));
        assert!(!is_ctx_sentence("ctx list tabs | head"));
        assert!(!is_ctx_sentence("ctx list tabs > /tmp/x"));
        assert!(!is_ctx_sentence(r#"ctx run in "T" "$(curl x)""#));
        assert!(!is_ctx_sentence("ctx list tabs\nrm x"));
        assert!(!is_ctx_sentence("ctxfoo"));
    }

    fn call(args: Value) -> Value {
        json!({"conversationId": "0105457a-940a-46cf", "toolCall": {"name": "run_command", "args": args}})
    }

    #[test]
    fn antigravity_run_command_is_rewritten_into_ctx_run() {
        assert_eq!(antigravity_tab_name("0105457a-940a-46cf-ba06"), "Antigravity 0105457a");
        let same = call(json!({"CommandLine": "echo 'hi' && pwd", "Cwd": "/w/", "Blocking": true}));
        assert_eq!(
            rewrite_antigravity(&same, "T", "/w").unwrap(),
            json!({"CommandLine": "ctx run in 'T' 'echo '\\''hi'\\'' && pwd'", "BypassSandbox": true})
        );
        let elsewhere = call(json!({"CommandLine": "cargo test", "Cwd": "/w/sub"}));
        assert_eq!(
            rewrite_antigravity(&elsewhere, "T", "/w").unwrap()["CommandLine"],
            "ctx run in 'T' 'cd '\\''/w/sub'\\'' && cargo test'"
        );
        let server = call(json!({"CommandLine": "npm start", "Cwd": "/w", "Blocking": false}));
        assert_eq!(
            rewrite_antigravity(&server, "T", "/w").unwrap()["CommandLine"],
            "ctx run in 'T' 'npm start' without waiting"
        );
        assert!(rewrite_antigravity(&call(json!({"CommandLine": "ctx list tabs"})), "T", "/w").is_none());
        assert!(rewrite_antigravity(&call(json!({"CommandLine": "ctx list tabs && echo x"})), "T", "/w").is_some());
        let other = json!({"toolCall": {"name": "view_file", "args": {"CommandLine": "x"}}});
        assert!(rewrite_antigravity(&other, "T", "/w").is_none());
    }

    #[test]
    fn sentences_follow_the_calls_folder_background_and_timeout() {
        let call = ShellCall { session: "s", command: "make", cwd: Some("/w/"), ..Default::default() };
        assert_eq!(sentence("T", "/w", &call), "ctx run in 'T' 'make'");
        let elsewhere = ShellCall { cwd: Some("/w/sub"), ..call };
        assert_eq!(sentence("T", "/w", &elsewhere), "ctx run in 'T' 'cd '\\''/w/sub'\\'' && make'");
        let bg = ShellCall { background: true, timeout_ms: Some(60_000), ..call };
        assert_eq!(sentence("T", "/w", &bg), "ctx run in 'T' 'make' without waiting");
        let timed = ShellCall { timeout_ms: Some(60_000), ..call };
        assert_eq!(sentence("T", "/w", &timed), "ctx run in 'T' 'make' within 55 seconds");
    }
}
