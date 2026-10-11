//! `ctx mcp`: ctx's verbs as MCP tools, over standard input and output (JSON-RPC 2.0, one
//! message per line). Hand-written: the official Rust SDK needs an async runtime, which ctx
//! doesn't use.
//!
//! Every tool builds the same sentence the CLI would parse and runs it through the same code, so
//! an agent using MCP and one using the command line get identical behaviour and the activity log
//! shows the same canonical sentences.

use ctx_cli::grammar::{Kind, Object, P, Slot, VERBS, Verb};
use serde_json::{Value, json};
use std::io::{BufRead, Write};

/// What running a sentence printed.
pub struct Result {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
    /// The failure came from ctx itself, not from the command a `run` ran.
    pub ctx_error: bool,
}

/// Verbs agents don't get as tools: program modes, terminal-only and permission-changing ones.
const NOT_TOOLS: &[&str] = &["server", "mcp", "attach", "make", "rebuild", "remove", "install", "restart", "help"];

const PROTOCOL: &str = "2025-06-18";

/// Serve until standard input closes. `run` executes one sentence (argv words).
pub fn serve(run: &dyn Fn(&[String]) -> Result) -> i32 {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => handle(&msg, run),
            Err(e) => Some(error(Value::Null, -32700, &format!("Parse error: {e}"))),
        };
        if let Some(r) = reply {
            let _ = writeln!(stdout, "{r}");
            let _ = stdout.flush();
        }
    }
    0
}

fn error(id: Value, code: i32, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn result(id: Value, r: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": r })
}

/// Handle one message; notifications get no reply.
pub fn handle(msg: &Value, run: &dyn Fn(&[String]) -> Result) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let id = id?; // notifications (no id): nothing to answer
    Some(match method {
        "initialize" => {
            let asked = msg.pointer("/params/protocolVersion").and_then(Value::as_str).unwrap_or(PROTOCOL);
            let version = if ["2025-06-18", "2025-03-26", "2024-11-05"].contains(&asked) { asked } else { PROTOCOL };
            result(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "ctx", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": ctx_cli::help::agent_instructions(),
                }),
            )
        }
        "ping" => result(id, json!({})),
        "tools/list" => result(id, json!({ "tools": tools() })),
        "tools/call" => {
            let name = msg.pointer("/params/name").and_then(Value::as_str).unwrap_or("");
            let args = msg.pointer("/params/arguments").cloned().unwrap_or(json!({}));
            match sentence_for(name, &args) {
                Ok(words) => {
                    let r = run(&words);
                    let mut text = r.stdout.trim_end().to_string();
                    let is_run = name == "ctx_run";
                    if !r.stderr.trim().is_empty() {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str(r.stderr.trim_end());
                    }
                    if is_run && !r.ctx_error {
                        text.push_str(&format!("\n[exit status: {}]", r.code));
                    }
                    let is_error = if is_run { r.ctx_error } else { r.code != 0 };
                    result(id, json!({ "content": [{ "type": "text", "text": text }], "isError": is_error }))
                }
                Err(e) => result(id, json!({ "content": [{ "type": "text", "text": e }], "isError": true })),
            }
        }
        "resources/list" => result(id, json!({ "resources": [] })),
        "prompts/list" => result(id, json!({ "prompts": [] })),
        other => error(id, -32601, &format!("Method not found: {other}")),
    })
}

fn tool_verbs() -> impl Iterator<Item = &'static Verb> {
    VERBS.iter().filter(|v| !v.hidden && !NOT_TOOLS.contains(&v.name))
}

/// The property name a phrase gets in a tool's input.
fn property(p: P) -> &'static str {
    match p {
        P::In | P::Into => "tab",
        P::At => "folder",
        P::To => "to",
        P::Named => "name",
        P::Until => "until",
        P::By => "by",
        P::Since => "since",
        P::Within => "within",
        P::UpTo => "up_to",
        P::Colors => "with_colors",
        P::As => "as",
        P::WithoutAsking => "without_asking",
        P::WithoutWaiting => "without_waiting",
        P::ThenPress => "then_press",
        P::About => "about",
        P::For => "for",
        P::Position => "position",
        P::AllowingAnyArguments => "allowing_any_arguments",
        P::SignedBy => "signed_by",
    }
}

fn schema_for(kind: Kind, meaning: &str) -> Value {
    match kind {
        Kind::Flag => json!({ "type": "boolean", "description": meaning }),
        Kind::Count => json!({ "type": "integer", "minimum": 0, "description": meaning }),
        Kind::Format => json!({ "type": "string", "enum": ["json", "toon"], "description": meaning }),
        Kind::Tab => json!({ "type": "string", "description": format!("{meaning}: the tab's name, or \"tab 42\"") }),
        Kind::Window => {
            json!({ "type": "string", "description": format!("{meaning}: \"window 2\" or a window's name") })
        }
        Kind::Duration => {
            json!({ "type": "string", "description": format!("{meaning}, in words: \"10 minutes\", \"30 seconds\"") })
        }
        Kind::Time => {
            json!({ "type": "string", "description": format!("{meaning}, in words: \"1 hour ago\", \"30 days ago\", \"yesterday\"") })
        }
        Kind::Condition => {
            json!({ "type": "string", "description": format!("{meaning}: \"<tab> is idle\", \"<tab> shows <text>\" or \"<tab> is quiet for 2 seconds\" (quote a tab name with spaces: '\"Dev server\" is idle')") })
        }
        Kind::TabOrAllTabs => {
            json!({ "type": "string", "description": format!("{meaning}: a tab's name, \"tab 42\", or \"all tabs\"") })
        }
        Kind::Who => {
            json!({ "type": "string", "description": format!("{meaning}: \"me\" or an agent such as \"claude\"") })
        }
        _ => json!({ "type": "string", "description": meaning }),
    }
}

/// Tool definitions, generated from the grammar table.
pub fn tools() -> Vec<Value> {
    let mut out = Vec::new();
    for v in tool_verbs() {
        let mut props = serde_json::Map::new();
        let mut required: Vec<&str> = Vec::new();
        match v.object {
            Object::None => {}
            Object::Noun(nouns) => {
                props
                    .insert("what".into(), json!({ "type": "string", "enum": nouns, "description": "what to act on" }));
                required.push("what");
            }
            Object::ReadSelector => {
                props.insert(
                    "what".into(),
                    json!({ "type": "string", "description": "what to read: \"screen\" (default), \"last 200 lines\", \"everything\", \"last command\", \"last 3 commands\" or \"command 14\"" }),
                );
            }
            Object::Kind(k) | Object::Optional(k) => {
                let name = object_property(v);
                props.insert(name.into(), schema_for(k, object_meaning(v)));
                if matches!(v.object, Object::Kind(_)) {
                    required.push(name);
                }
            }
        }
        for slot in v.order {
            if let Slot::Phrase(p) = slot {
                let name = property(p.id);
                if props.contains_key(name) {
                    continue;
                }
                props.insert(name.into(), schema_for(p.kind, p.meaning));
            }
        }
        if v.name == "wait" {
            required.push("until");
        }
        let examples: Vec<String> = v.examples.iter().take(2).map(|(s, _)| format!("`{s}`")).collect();
        out.push(json!({
            "name": format!("ctx_{}", v.name),
            "description": format!("{} Same as {}.", v.summary, examples.join(", ")),
            "inputSchema": { "type": "object", "properties": props, "required": required, "additionalProperties": false },
        }));
    }
    out.push(json!({
        "name": "ctx",
        "description": "Run any ctx sentence, written as on the command line without the leading `ctx`, e.g. `read last 3 commands in \"Dev server\"`. `help` lists them all.",
        "inputSchema": { "type": "object", "properties": { "sentence": { "type": "string" } }, "required": ["sentence"], "additionalProperties": false },
    }));
    out
}

fn object_property(v: &Verb) -> &'static str {
    match v.name {
        "run" => "command",
        "type" => "text",
        "press" => "keys",
        "ask" => "question",
        "find" => "text",
        "close" | "rename" => "target",
        _ => "tab",
    }
}

fn object_meaning(v: &Verb) -> &'static str {
    match v.name {
        "run" => "the shell command (omit to read it from nothing; agents always give it)",
        "type" => "text to type",
        "press" => "key names joined by \" then \": \"ctrl-c\", \"escape then enter\"",
        "ask" => "the question",
        "find" => "the text to search for",
        "close" | "rename" => "the tab (name or \"tab 42\") or window (\"window 2\")",
        _ => "the tab: its name, or \"tab 42\"",
    }
}

/// Words of a tab or window reference: `tab 42` and `window 2` stay words, names are one word.
fn reference_words(s: &str) -> Vec<String> {
    let t = s.trim();
    let lower = t.to_lowercase();
    let parts: Vec<&str> = t.split_whitespace().collect();
    if (lower.starts_with("tab ") || lower.starts_with("window ")) && parts.len() == 2 {
        return parts.iter().map(|p| p.to_string()).collect();
    }
    if lower == "this tab" || lower == "all tabs" {
        return parts.iter().map(|p| p.to_string()).collect();
    }
    if let Some(rest) = t.strip_prefix("window ").or_else(|| t.strip_prefix("Window ")) {
        return vec!["window".into(), rest.trim_matches('"').to_string()];
    }
    vec![t.trim_matches('"').to_string()]
}

fn words(s: &str) -> Vec<String> {
    s.split_whitespace().map(str::to_string).collect()
}

/// Build the sentence (as argv words) for a tool call.
pub fn sentence_for(tool: &str, args: &Value) -> std::result::Result<Vec<String>, String> {
    let get = |k: &str| args.get(k);
    let text = |k: &str| get(k).and_then(Value::as_str).map(str::to_string);
    if tool == "ctx" {
        let s = text("sentence").ok_or("`sentence` is required.")?;
        let w = shell_words(&s)?;
        let w = if w.first().map(|x| x.as_str()) == Some("ctx") { w[1..].to_vec() } else { w };
        if let Some(first) = w.first()
            && NOT_TOOLS.contains(&first.as_str())
            && first != "help"
        {
            return Err(format!("`{first}` isn't available to agents over MCP."));
        }
        return Ok(w);
    }
    let verb_name = tool.strip_prefix("ctx_").ok_or_else(|| format!("Unknown tool {tool}"))?;
    let v = tool_verbs().find(|v| v.name == verb_name).ok_or_else(|| format!("Unknown tool {tool}"))?;
    let mut out = vec![v.name.to_string()];
    for slot in v.order {
        match slot {
            Slot::Object => match v.object {
                Object::Noun(_) => out.extend(words(&text("what").ok_or("`what` is required.")?)),
                Object::ReadSelector => {
                    let what = text("what").unwrap_or_default();
                    let w = what.trim().to_lowercase();
                    if !(w.is_empty() || w == "screen") {
                        out.extend(words(&w));
                    }
                }
                Object::Kind(k) | Object::Optional(k) => {
                    let name = object_property(v);
                    if let Some(val) = text(name) {
                        match k {
                            Kind::Keys => out.extend(words(&val)),
                            Kind::Tab | Kind::TabOrWindow => out.extend(reference_words(&val)),
                            _ => out.push(val),
                        }
                    } else if matches!(v.object, Object::Kind(_)) {
                        return Err(format!("`{name}` is required."));
                    }
                }
                Object::None => {}
            },
            Slot::Phrase(p) => {
                let name = property(p.id);
                let Some(val) = get(name) else { continue };
                let phrase: Vec<String> = p.words.iter().map(|w| w.to_string()).collect();
                match p.kind {
                    Kind::Flag => {
                        if val.as_bool() == Some(true) {
                            out.extend(phrase);
                        }
                    }
                    Kind::Count => {
                        let n = val
                            .as_u64()
                            .or_else(|| val.as_str().and_then(|s| s.parse().ok()))
                            .ok_or(format!("`{name}` must be a number."))?;
                        out.extend(phrase);
                        out.push(n.to_string());
                        if let Some(a) = p.after.first() {
                            out.push(a.to_string());
                        }
                    }
                    Kind::Tab | Kind::Window | Kind::TabOrAllTabs => {
                        out.extend(phrase);
                        out.extend(reference_words(val.as_str().unwrap_or_default()));
                    }
                    Kind::Duration | Kind::Time | Kind::Keys | Kind::Key => {
                        out.extend(phrase);
                        out.extend(words(val.as_str().unwrap_or_default()));
                    }
                    Kind::Condition => {
                        out.extend(phrase);
                        out.extend(shell_words(val.as_str().unwrap_or_default())?);
                    }
                    _ => {
                        out.extend(phrase);
                        out.push(val.as_str().map(str::to_string).unwrap_or_else(|| val.to_string()));
                    }
                }
            }
        }
    }
    Ok(out)
}

/// Split a sentence the way zsh would: whitespace separates words; double or single quotes group;
/// backslash escapes the next character outside single quotes.
pub fn shell_words(s: &str) -> std::result::Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut started = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                started = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(x) => cur.push(x),
                        None => return Err("A single quote isn't closed.".into()),
                    }
                }
            }
            '"' => {
                started = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(x @ ('"' | '\\' | '$' | '`')) => cur.push(x),
                            Some(x) => {
                                cur.push('\\');
                                cur.push(x);
                            }
                            None => return Err("A double quote isn't closed.".into()),
                        },
                        Some(x) => cur.push(x),
                        None => return Err("A double quote isn't closed.".into()),
                    }
                }
            }
            '\\' => {
                started = true;
                if let Some(x) = chars.next() {
                    cur.push(x);
                }
            }
            c if c.is_whitespace() => {
                if started {
                    out.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            c => {
                started = true;
                cur.push(c);
            }
        }
    }
    if started {
        out.push(cur);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_cover_the_agent_verbs() {
        let t = tools();
        let names: Vec<&str> = t.iter().map(|x| x["name"].as_str().unwrap()).collect();
        for n in ["ctx_run", "ctx_read", "ctx_list", "ctx_find", "ctx_wait", "ctx_type", "ctx_press", "ctx_open", "ctx"]
        {
            assert!(names.contains(&n), "{n}");
        }
        assert!(!names.contains(&"ctx_server"));
    }

    #[test]
    fn sentences_from_arguments() {
        let s = sentence_for(
            "ctx_run",
            &json!({"tab": "Dev server", "command": "npm test", "within": "10 minutes", "up_to": 50}),
        )
        .unwrap();
        assert_eq!(
            s,
            vec!["run", "in", "Dev server", "npm test", "up", "to", "50", "lines", "within", "10", "minutes"]
        );
        let s = sentence_for("ctx_read", &json!({"what": "last 3 commands", "tab": "tab 42", "as": "toon"})).unwrap();
        assert_eq!(s, vec!["read", "last", "3", "commands", "in", "tab", "42", "as", "toon"]);
        let s = sentence_for("ctx_wait", &json!({"until": "\"Dev server\" shows \"ready\""})).unwrap();
        assert_eq!(s, vec!["wait", "until", "Dev server", "shows", "ready"]);
        let s = sentence_for("ctx_list", &json!({"what": "closed tabs"})).unwrap();
        assert_eq!(s, vec!["list", "closed", "tabs"]);
        let s = sentence_for("ctx_close", &json!({"target": "window 2", "without_asking": true})).unwrap();
        assert_eq!(s, vec!["close", "window", "2", "without", "asking"]);
        let s = sentence_for("ctx", &json!({"sentence": "ctx run in \"Dev server\" 'echo \"hi\"'"})).unwrap();
        assert_eq!(s, vec!["run", "in", "Dev server", "echo \"hi\""]);
        assert!(sentence_for("ctx", &json!({"sentence": "restart server"})).is_err());
    }

    #[test]
    fn sentences_parse() {
        let cx = ctx_cli::parse::Context { folder: "/".into(), home: "/Users/x".into(), now_ms: 0 };
        for (tool, args) in [
            ("ctx_run", json!({"tab": "Dev server", "command": "npm test", "without_waiting": true})),
            ("ctx_read", json!({"what": "everything", "tab": "Dev"})),
            ("ctx_find", json!({"text": "deploy", "by": "claude", "since": "30 days ago"})),
            ("ctx_open", json!({"what": "tab", "name": "Claude - x", "folder": "/tmp"})),
            ("ctx_type", json!({"text": "yes", "tab": "Dev", "then_press": "enter"})),
            ("ctx_press", json!({"keys": "ctrl-c", "tab": "Dev"})),
            ("ctx_move", json!({"tab": "Dev", "to": "window 2", "position": 1})),
            ("ctx_ask", json!({"question": "why?", "about": "tab 4"})),
        ] {
            let w = sentence_for(tool, &args).unwrap();
            ctx_cli::parse::parse(&w, &cx).unwrap_or_else(|e| panic!("{tool}: {w:?}: {e:?}"));
        }
    }

    #[test]
    fn protocol_basics() {
        let run = |_: &[String]| Result { stdout: "ok\n".into(), stderr: String::new(), code: 0, ctx_error: false };
        let init = handle(
            &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18"}}),
            &run,
        )
        .unwrap();
        assert_eq!(init["result"]["serverInfo"]["name"], "ctx");
        assert!(handle(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}), &run).is_none());
        let call = handle(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "ctx_list", "arguments": {"what": "tabs"}}}), &run).unwrap();
        assert_eq!(call["result"]["content"][0]["text"], "ok");
        assert_eq!(call["result"]["isError"], false);
        let missing = handle(&json!({"jsonrpc": "2.0", "id": 3, "method": "nope"}), &run).unwrap();
        assert_eq!(missing["error"]["code"], -32601);
    }
}
