//! `ctx mcp`: one tool, `shell`, over standard input and output (JSON-RPC 2.0, one message per
//! line), for agents whose own shell tool can't be hooked (see `ctx hook`). Each command runs in a
//! tab named for the session, like a hooked agent's; ctx's other verbs are reached the same way any
//! agent reaches them, by running `ctx …` in that shell. Hand-written: the official Rust SDK needs
//! an async runtime, which ctx doesn't use.

use serde_json::{Value, json};
use std::io::{BufRead, Write};

/// What running a command printed.
pub struct Result {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
    /// The failure came from ctx itself, not from the command.
    pub ctx_error: bool,
}

/// One `shell` call.
pub struct Call<'a> {
    /// The client's name from `initialize` (`clientInfo.name`), for naming the session's tab.
    pub client: &'a str,
    pub command: &'a str,
    pub timeout_ms: u64,
}

const PROTOCOL: &str = "2025-06-18";

/// How long a call waits by default; the command keeps running after.
pub const DEFAULT_TIMEOUT_MS: u64 = 120_000;

/// Serve until standard input closes. `shell` runs one call.
pub fn serve(shell: &dyn Fn(&Call) -> Result) -> i32 {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut client = String::new();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => handle(&msg, &mut client, shell),
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

/// The one tool.
pub fn tool() -> Value {
    json!({
        "name": "shell",
        "description": "Run a command in a persistent zsh shell and return its output and exit status. The working directory and environment carry over between calls. A command still running when the timeout ends keeps running.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "The command to run" },
                "timeout": { "type": "integer", "minimum": 1, "description": "Seconds to wait for it (default 120)" },
            },
            "required": ["command"],
            "additionalProperties": false,
        },
    })
}

/// Handle one message; notifications get no reply. `client` keeps the name `initialize` gave.
pub fn handle(msg: &Value, client: &mut String, shell: &dyn Fn(&Call) -> Result) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let id = id?; // notifications (no id): nothing to answer
    Some(match method {
        "initialize" => {
            let asked = msg.pointer("/params/protocolVersion").and_then(Value::as_str).unwrap_or(PROTOCOL);
            let version = if ["2025-06-18", "2025-03-26", "2024-11-05"].contains(&asked) { asked } else { PROTOCOL };
            *client = msg.pointer("/params/clientInfo/name").and_then(Value::as_str).unwrap_or("").to_string();
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
        "tools/list" => result(id, json!({ "tools": [tool()] })),
        "tools/call" => {
            let name = msg.pointer("/params/name").and_then(Value::as_str).unwrap_or("");
            let args = msg.pointer("/params/arguments").cloned().unwrap_or(json!({}));
            let command = args.get("command").and_then(Value::as_str).unwrap_or("");
            if name != "shell" {
                return Some(text_result(id, &format!("Unknown tool {name}."), true));
            }
            if command.trim().is_empty() {
                return Some(text_result(id, "`command` is required.", true));
            }
            let timeout_ms = args
                .get("timeout")
                .and_then(Value::as_u64)
                .filter(|s| *s > 0)
                .map_or(DEFAULT_TIMEOUT_MS, |s| s.saturating_mul(1000));
            let r = shell(&Call { client, command, timeout_ms });
            let mut text = r.stdout.trim_end().to_string();
            if !r.stderr.trim().is_empty() {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(r.stderr.trim_end());
            }
            if !r.ctx_error {
                text.push_str(&format!("\n[exit status: {}]", r.code));
            }
            text_result(id, &text, r.ctx_error)
        }
        "resources/list" => result(id, json!({ "resources": [] })),
        "prompts/list" => result(id, json!({ "prompts": [] })),
        other => error(id, -32601, &format!("Method not found: {other}")),
    })
}

fn text_result(id: Value, text: &str, is_error: bool) -> Value {
    result(id, json!({ "content": [{ "type": "text", "text": text }], "isError": is_error }))
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

    fn call(client: &mut String, shell: &dyn Fn(&Call) -> Result, args: Value) -> Value {
        let msg =
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "shell", "arguments": args}});
        handle(&msg, client, shell).unwrap()["result"].clone()
    }

    #[test]
    fn one_tool() {
        let mut client = String::new();
        let shell = |_: &Call| Result { stdout: String::new(), stderr: String::new(), code: 0, ctx_error: false };
        let list = handle(&json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}), &mut client, &shell).unwrap();
        let tools = list["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "shell");
    }

    #[test]
    fn calls_carry_the_client_and_timeout() {
        let mut client = String::new();
        let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "clientInfo": {"name": "gemini-cli-mcp-client"}}});
        let shell = |c: &Call| Result {
            stdout: format!("{} {} {}\n", c.client, c.command, c.timeout_ms),
            stderr: String::new(),
            code: 3,
            ctx_error: false,
        };
        let r = handle(&init, &mut client, &shell).unwrap();
        assert_eq!(r["result"]["serverInfo"]["name"], "ctx");
        let r = call(&mut client, &shell, json!({"command": "false"}));
        assert_eq!(r["content"][0]["text"], "gemini-cli-mcp-client false 120000\n[exit status: 3]");
        assert_eq!(r["isError"], false);
        let r = call(&mut client, &shell, json!({"command": "sleep 9", "timeout": 5}));
        assert_eq!(r["content"][0]["text"], "gemini-cli-mcp-client sleep 9 5000\n[exit status: 3]");
    }

    #[test]
    fn ctx_errors_are_tool_errors() {
        let mut client = String::new();
        let shell =
            |_: &Call| Result { stdout: String::new(), stderr: "No server.\n".into(), code: 1, ctx_error: true };
        let r = call(&mut client, &shell, json!({"command": "ls"}));
        assert_eq!(r["content"][0]["text"], "No server.");
        assert_eq!(r["isError"], true);
        assert_eq!(call(&mut client, &shell, json!({"command": " "}))["isError"], true);
    }

    #[test]
    fn protocol_basics() {
        let mut client = String::new();
        let shell = |_: &Call| Result { stdout: String::new(), stderr: String::new(), code: 0, ctx_error: false };
        assert!(
            handle(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}), &mut client, &shell).is_none()
        );
        let missing = handle(&json!({"jsonrpc": "2.0", "id": 3, "method": "nope"}), &mut client, &shell).unwrap();
        assert_eq!(missing["error"]["code"], -32601);
    }

    #[test]
    fn splits_like_zsh() {
        assert_eq!(
            shell_words(r#"ctx run in "Dev server" 'echo "hi"'"#).unwrap(),
            ["ctx", "run", "in", "Dev server", "echo \"hi\""]
        );
        assert!(shell_words("echo 'x").is_err());
    }
}
