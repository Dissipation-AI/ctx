//! The model half of assist: ask `bro` (if installed) for a corrected ctx sentence. `bro` doesn't
//! know who's calling; the request carries ctx's grammar. Never runs anything.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const PROMPT: &str = include_str!("../prompts/assist.md");
pub const TIME_LIMIT: Duration = Duration::from_secs(3);

/// What bro said: an explanation, and a command if it gave one.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub command: Option<String>,
    pub explanation: String,
}

/// The request piped to `bro --json`.
pub fn request(sentence: &str, error: &str, grammar: &str, tabs: &[String]) -> String {
    let tabs = if tabs.is_empty() { "(none open)".to_string() } else { tabs.join(", ") };
    format!(
        "{PROMPT}\nWhat they typed: ctx {sentence}\nWhy it didn't parse: {error}\nOpen tabs: {tabs}\n\nctx grammar (every sentence starts with `ctx`):\n{grammar}"
    )
}

/// Run the assist command with the request on stdin; give up after the time limit.
pub fn ask_bro(program: &str, request: &str) -> Option<Reply> {
    let mut child = Command::new(program)
        .arg("--json")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    {
        let mut stdin = child.stdin.take()?;
        let _ = stdin.write_all(request.as_bytes());
    }
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() > TIME_LIMIT => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => return None,
        }
    }
    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    let v: serde_json::Value = serde_json::from_str(out.trim()).ok()?;
    if v.get("error").is_some() {
        return None;
    }
    let command = v["command"].as_str().map(str::trim).filter(|c| !c.is_empty()).map(str::to_string);
    let explanation = v["explanation"].as_str().unwrap_or("").trim().to_string();
    Some(Reply { command, explanation })
}

/// Split a suggested command line into words (double and single quotes group).
pub fn words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut started = false;
    let mut quote: Option<char> = None;
    for c in s.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started {
                    out.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            (None, c) => {
                cur.push(c);
                started = true;
            }
        }
    }
    if started {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_has_what_bro_needs() {
        let r = request(
            "kill the dev server tab",
            "\"kill\" isn't a ctx verb.",
            "ctx close \"<tab>\"",
            &["tab 43 \"Dev server\"".into()],
        );
        assert!(
            r.contains("ctx kill the dev server tab") && r.contains("tab 43 \"Dev server\"") && r.contains("ctx close")
        );
    }

    #[test]
    fn splits_suggestions() {
        assert_eq!(words(r#"ctx close "Dev server""#), vec!["ctx", "close", "Dev server"]);
    }

    #[test]
    fn missing_program_is_no_answer() {
        assert_eq!(ask_bro("/nonexistent/bro", "x"), None);
    }
}
