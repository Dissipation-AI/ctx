//! `docs/output.md`, generated from these types: one example of every answer in JSON and in
//! TOON, with its fields. A test regenerates the file and fails if it was out of date.

use crate::*;
use serde_json::Value;

fn tab() -> TabInfo {
    TabInfo {
        number: 43,
        name: Some("Dev server".into()),
        folder: "/Users/c/code/site".into(),
        window: 2,
        opened_by: "claude (pid 5512)".into(),
        title: None,
        state: "busy".into(),
        running: Some("npm run dev".into()),
        pid: Some(81234),
        started: Some("2026-10-10T17:52:01Z".into()),
        unread_lines: 12,
        columns: 120,
        rows: 40,
        shell_integration: true,
        permissions_of: None,
        remote_host: None,
    }
}

fn command(n: u64, text: &str, exit: i32, output: &[&str]) -> CommandRecord {
    CommandRecord {
        number: n,
        command: text.into(),
        location: "c@macbook:~/code/site".into(),
        ran_by: if n == 13 { "claude (pid 5512)".into() } else { "you".into() },
        exit_status: Some(exit),
        started_at: "2026-10-10T17:52:01Z".into(),
        finished_at: Some("2026-10-10T17:52:40Z".into()),
        took_seconds: Some(39.0),
        approximate: false,
        full_screen: false,
        output: output.iter().map(|s| s.to_string()).collect(),
        events: vec![],
        output_lines: output.len() as u64,
        note: None,
    }
}

/// (sentence, what it is, example answer)
pub fn examples() -> Vec<(&'static str, &'static str, Output)> {
    vec![
        (
            "ctx list tabs",
            "Open windows and their tabs.",
            Output::Tabs(TabList {
                windows: vec![WindowInfo { number: 2, name: Some("Work".into()), tabs: vec![tab()] }],
            }),
        ),
        (
            "ctx list closed tabs",
            "Saved tabs that aren't running any more, newest first.",
            Output::ClosedTabs(ClosedTabList {
                tabs: vec![ClosedTab {
                    number: 40,
                    name: Some("Sysadmin agent".into()),
                    folder: "/Users/c/agents/sysadmin".into(),
                    opened_by: "you".into(),
                    opened_at: "2026-10-09T20:01:00Z".into(),
                    closed_at: "2026-10-09T23:10:00Z".into(),
                    ended: "closed".into(),
                    commands: 31,
                }],
                total: 1,
            }),
        ),
        (
            "ctx list commands in \"Dev server\"",
            "A tab's commands without their output.",
            Output::Commands(CommandList {
                tab: 43,
                tab_name: Some("Dev server".into()),
                commands: vec![CommandInfo {
                    number: 13,
                    command: "cargo build".into(),
                    location: "c@macbook:~/code/site".into(),
                    ran_by: "claude (pid 5512)".into(),
                    exit_status: Some(0),
                    started_at: "2026-10-10T17:52:01Z".into(),
                    finished_at: Some("2026-10-10T17:52:40Z".into()),
                    took_seconds: Some(39.0),
                    approximate: false,
                }],
                total: 1,
            }),
        ),
        (
            "ctx read last 3 commands in \"Dev server\"",
            "Commands with their output (also `read command N`). Output lines are numbered from 1 within each command: line 3 is `output[2]`.",
            Output::CommandsWithOutput(CommandOutputs {
                tab: 212,
                tab_name: None,
                commands: vec![
                    command(13, "cargo build", 0, &["   Compiling site v0.1.0", "    Finished dev profile"]),
                    command(14, "cargo test -p auth", 101, &["test result: FAILED. 13 passed; 1 failed"]),
                ],
            }),
        ),
        (
            "ctx read last 200 lines in \"Dev server\"",
            "Text: the screen, the last N lines, or everything.",
            Output::Text(TextOutput {
                tab: 43,
                tab_name: Some("Dev server".into()),
                lines: vec![
                    "[renamed \"vite\" → \"Dev server\"]".into(),
                    "  VITE v5.4.2  ready in 312 ms".into(),
                    "  ➜  Local:   http://localhost:5173/".into(),
                ],
                events: vec![EventLine {
                    line: 1,
                    event: r#"{"event":"renamed","at":"2026-10-10T17:50:00Z","from":"vite","to":"Dev server"}"#.into(),
                }],
                total_lines: 12503,
                note: Some("showing the last 2 of 12,503 lines; ctx read everything in \"Dev server\" for all".into()),
            }),
        ),
        (
            "ctx run in \"Dev server\" \"npm test\"",
            "What a command printed and its exit status. `still_running` is true after `without waiting`, or when a time limit ran out (exit status 3).",
            Output::Ran(RunResult {
                tab: 43,
                tab_name: Some("Dev server".into()),
                command_number: Some(15),
                command: "npm test".into(),
                exit_status: Some(0),
                still_running: false,
                took_seconds: 2.4,
                output: vec!["Tests: 14 passed, 14 total".into()],
                events: vec![],
                output_lines: 1,
                note: None,
            }),
        ),
        (
            "ctx wait until \"Dev server\" shows \"Listening on\"",
            "What was waited for. `condition` is `idle`, `shows` or `quiet`.",
            Output::Waited(WaitResult {
                tab: 43,
                tab_name: Some("Dev server".into()),
                condition: "shows".into(),
                exit_status: None,
                line: Some("Listening on http://localhost:4000".into()),
                waited_seconds: 1.8,
            }),
        ),
        (
            "ctx find \"deploy\" by claude",
            "Matches in names, commands and output, newest first, with two lines of context.",
            Output::Found(FindResults {
                text: "deploy".into(),
                hits: vec![FindHit {
                    tab: 212,
                    tab_name: Some("Claude - release".into()),
                    found_in: "output".into(),
                    command: Some(9),
                    command_text: Some("npm run deploy".into()),
                    line: Some(4),
                    ran_by: Some("claude (pid 5512)".into()),
                    at: "2026-10-08T11:20:00Z".into(),
                    text: "deploy finished in 41s".into(),
                    before: vec!["uploading assets".into(), "invalidating cache".into()],
                    after: vec![],
                }],
                total: 1,
                note: None,
            }),
        ),
        (
            "ctx list activity by claude since 1 hour ago",
            "The activity log, newest first. `client` is verified from the OS; `declared_name` is what the client claimed.",
            Output::Activity(ActivityList {
                entries: vec![ActivityEntry {
                    at: "2026-10-10T17:53:02Z".into(),
                    client: "claude (pid 5512) in tab 2".into(),
                    program: "claude".into(),
                    pid: 5512,
                    declared_name: Some("Claude - auth refactor".into()),
                    sentence: "ctx run in \"Dev server\" \"npm test\"".into(),
                    tab: Some(43),
                    typed: Some("npm test".into()),
                    typed_hidden: false,
                }],
            }),
        ),
        (
            "ctx status",
            "The server.",
            Output::Status(Status {
                version: "0.1.0".into(),
                protocol: PROTOCOL_VERSION,
                pid: 81200,
                uptime_seconds: 3600,
                socket: "/var/folders/xx/T/ctx-501/server.sock".into(),
                memory_bytes: 21_000_000,
                saved_bytes: 12_000_000,
                tabs: vec![TabMemory {
                    number: 43,
                    name: Some("Dev server".into()),
                    memory_bytes: 1_200_000,
                    scrollback_lines: 1034,
                }],
            }),
        ),
        (
            "ctx open tab named \"Dev server\"",
            "A new tab (or window, with `new_window`). `not_restored` lists variables kept out by `dont_save_env` when reopening.",
            Output::Opened(Opened {
                tab: Some(43),
                window: 2,
                new_window: false,
                window_name: None,
                name: Some("Dev server".into()),
                folder: "/Users/c/code/site".into(),
                not_restored: vec![],
            }),
        ),
        ("ctx close \"Dev server\"", "Done, in a sentence.", Output::Done(Done { message: "Closed tab 43.".into() })),
    ]
}

fn describe_fields(v: &Value, prefix: &str, out: &mut Vec<String>) {
    if let Value::Object(map) = v {
        for (k, val) in map {
            let path = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
            let kind = match val {
                Value::Null => "nullable",
                Value::Bool(_) => "boolean",
                Value::Number(_) => "number",
                Value::String(_) => "text",
                Value::Array(_) => "list",
                Value::Object(_) => "object",
            };
            out.push(format!("`{path}` ({kind})"));
            match val {
                Value::Object(_) => describe_fields(val, &path, out),
                Value::Array(items) => {
                    if let Some(first) = items.first()
                        && first.is_object()
                    {
                        describe_fields(first, &format!("{path}[]"), out);
                    }
                }
                _ => {}
            }
        }
    }
}

pub fn output_markdown() -> String {
    let mut out = format!(
        "# ctx output schemas\n\n_Generated from `ctx-protocol/src/docs.rs` and the types in `ctx-protocol`. Don't edit by hand._\n\n\
Every sentence can answer `as json` or `as toon`; both carry exactly the same fields. The schema version is `{SCHEMA_VERSION}`\n\
and comes first in every answer. Field names are whole words; times are ISO 8601 in UTC; a value that doesn't apply is `null`.\n\n\
Errors have the same shape everywhere:\n\n```json\n{}\n```\n\n\
`error` is one of: `not_a_valid_command`, `no_such_tab`, `no_such_window`, `not_allowed`, `busy`, `someone_typing`,\n\
`needs_confirmation`, `time_limit`, `unsupported`, `failed`. A sentence that doesn't parse also carries `suggestion` and\n\
`explanation`.\n\n",
        serde_json::to_string_pretty(
            &ErrorReply::new(ErrorKind::NoSuchTab, "There is no tab 9. Open tabs are 41, 42 and 43.").to_json()
        )
        .unwrap()
    );
    for (sentence, what, o) in examples() {
        let json = o.to_json();
        let mut fields = Vec::new();
        describe_fields(&json, "", &mut fields);
        out.push_str(&format!(
            "## `{sentence}`\n\n{what}\n\nFields: {}.\n\n```json\n{}\n```\n\n```\n{}\n```\n\n",
            fields.join(", "),
            serde_json::to_string_pretty(&json).unwrap(),
            toon::encode(&json)
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn output_doc_is_current() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../docs/output.md");
        let want = super::output_markdown();
        if std::fs::read_to_string(path).unwrap_or_default() != want {
            std::fs::write(path, want).unwrap();
            panic!("docs/output.md was out of date and has been regenerated; commit it");
        }
    }
}
