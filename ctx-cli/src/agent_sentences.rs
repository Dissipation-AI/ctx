//! Sentences about agents' accounts: making, sharing with, limiting and switching to agents.
//!
//! These share verbs with the main grammar (`make`, `remove`, `rename`, `show`) but have their
//! own shape, `<verb> agent "<name>" …`, so they're read here first and fall through to the main
//! grammar when the words don't start that way.

use crate::parse::{Format, ParseError, Token, quote, resolve_path};

/// An agent's name or every agent (the `ctx-agents` group).
#[derive(Clone, Debug, PartialEq)]
pub enum Grantee {
    Agent(String),
    Every,
}

/// What a grant is on: a file or folder, or an agent's home.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Path(String),
    Agent(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum LimitSpec {
    MemoryMb(u64),
    Processes(u64),
    LowPriority,
    NormalPriority,
    Nothing,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AgentCommand {
    /// A new account; `home` is an existing folder to use as its home.
    Make {
        name: String,
        home: Option<String>,
    },
    /// An existing standard account becomes an agent.
    Adopt {
        account: String,
    },
    Remove {
        name: String,
        with_files: bool,
    },
    Rename {
        name: String,
        full_name: String,
    },
    Show {
        name: String,
    },
    /// `allow` (allow = true) or `forbid` (false) reading or writing.
    Grant {
        allow: bool,
        to: Grantee,
        write: bool,
        on: Target,
    },
    Switch {
        name: String,
    },
    Limit {
        name: String,
        limit: LimitSpec,
    },
    Reset {
        name: String,
    },
    MakeShared {
        with: Vec<String>,
    },
    MoveToAccounts,
}

/// Example sentences with what they do, for `ctx help`, docs/grammar.md and tests.
pub const EXAMPLES: &[(&str, &str)] = &[
    ("ctx make agent named \"email-agent\"", "A new agent: its own account, with its home in ~/Agents."),
    (
        "ctx make agent named \"email-agent\" from ~/Agents/email-agent",
        "The same, with an existing folder as its home.",
    ),
    ("ctx make agent from account \"vibe-daemon\"", "An existing standard account becomes an agent."),
    (
        "ctx switch to agent \"email-agent\"",
        "Open a tab as the agent, in its home (like su). Run claude, codex, pi… there.",
    ),
    ("ctx allow agent \"email-agent\" to read ~/Context/career", "Let it read that folder (an access-list entry)."),
    ("ctx allow agent \"foreman\" to write agent \"email-agent\"", "Make foreman an administrator of email-agent."),
    ("ctx allow every agent to read ~/.local/bin", "Every agent may read it."),
    ("ctx forbid agent \"email-agent\" to write ~/Context/career", "Take write back; reading stays."),
    ("ctx forbid agent \"email-agent\" to read ~/Context/career", "Take all access back."),
    ("ctx show agent \"email-agent\"", "Its account, administrators, grants, limits and tabs."),
    ("ctx limit agent \"email-agent\" to 8 GB of memory", "Also: 200 processes, low priority, normal priority, nothing."),
    ("ctx rename agent \"email-agent\" to \"Email\"", "The full name shown beside the account name."),
    ("ctx reset agent \"email-agent\"", "Rewrite its ctx-agent.toml as yours, with the defaults."),
    ("ctx remove agent \"email-agent\"", "Delete the account; its home stays. Add `with its files` to trash it too."),
    (
        "ctx make shared folder with agent \"email-agent\"",
        "A folder in ~/Agents/Shared for the agent running this, that agent and you.",
    ),
    ("ctx move agents to accounts", "Turn the [agents] sections of config.toml into accounts. Shows the plan first."),
];

pub struct Parsed {
    pub command: AgentCommand,
    pub format: Format,
    pub canonical: String,
}

fn words(tokens: &[Token]) -> Vec<String> {
    tokens.iter().map(|t| t.text.to_lowercase()).collect()
}

/// Take `as json`/`as toon` off the end.
fn take_format(tokens: &mut Vec<Token>) -> Result<Format, ParseError> {
    let n = tokens.len();
    if n >= 2 && tokens[n - 2].text.eq_ignore_ascii_case("as") {
        let f = match tokens[n - 1].text.to_lowercase().as_str() {
            "json" => Format::Json,
            "toon" => Format::Toon,
            "text" => Format::Text,
            other => return Err(ParseError::new(format!("`as` takes json or toon, not \"{other}\"."))),
        };
        tokens.truncate(n - 2);
        return Ok(f);
    }
    Ok(Format::Text)
}

fn name_at(tokens: &[Token], i: usize, what: &str, example: &str) -> Result<String, ParseError> {
    match tokens.get(i) {
        Some(t) if !t.text.is_empty() => Ok(t.text.clone()),
        _ => Err(ParseError::new(format!("Which {what}? `{example}`"))),
    }
}

fn no_more(tokens: &[Token], from: usize) -> Result<(), ParseError> {
    match tokens.get(from) {
        None => Ok(()),
        Some(t) => Err(ParseError::new(format!("I don't know what \"{}\" means here.", t.text))),
    }
}

/// `agent "b"` → b, or `every agent`; returns what and how many tokens it took.
fn grantee(tokens: &[Token], w: &[String]) -> Result<(Grantee, usize), ParseError> {
    match w.first().map(String::as_str) {
        Some("every" | "all") if w.get(1).is_some_and(|x| x == "agent" || x == "agents") => Ok((Grantee::Every, 2)),
        Some("agent") => {
            Ok((Grantee::Agent(name_at(tokens, 1, "agent", "ctx allow agent \"b\" to read ~/folder")?), 2))
        }
        _ => Err(ParseError::new("Whom? `agent \"<name>\"` or `every agent`.")),
    }
}

fn target(tokens: &[Token], w: &[String], cwd: &crate::parse::Context) -> Result<(Target, usize), ParseError> {
    match w.first().map(String::as_str) {
        Some("agent") => {
            Ok((Target::Agent(name_at(tokens, 1, "agent", "ctx allow agent \"a\" to read agent \"b\"")?), 2))
        }
        Some(_) => Ok((Target::Path(resolve_path(&tokens[0].text, cwd)), 1)),
        None => Err(ParseError::new("On what? A file or folder (`~/Context/career`) or `agent \"<name>\"`.")),
    }
}

fn show_grantee(g: &Grantee) -> String {
    match g {
        Grantee::Agent(a) => format!("agent {}", quote(a)),
        Grantee::Every => "every agent".into(),
    }
}

fn show_target(t: &Target) -> String {
    match t {
        Target::Agent(a) => format!("agent {}", quote(a)),
        Target::Path(p) => quote(p),
    }
}

/// A size in megabytes: `8 GB`, `512 MB`.
fn megabytes(n: &str, unit: &str) -> Option<u64> {
    let v: f64 = n.parse().ok()?;
    let mb = match unit {
        "gb" | "g" | "gigabytes" | "gigabyte" => v * 1024.0,
        "mb" | "m" | "megabytes" | "megabyte" => v,
        _ => return None,
    };
    (mb >= 1.0).then_some(mb as u64)
}

/// Read an agent sentence; `None` when the words aren't one (the main grammar takes them).
pub fn parse(tokens: &[Token], cwd: &crate::parse::Context) -> Option<Result<Parsed, ParseError>> {
    let w = words(tokens);
    let first = w.first()?.as_str();
    let second = w.get(1).map(String::as_str);
    let third = w.get(2).map(String::as_str);
    let is_ours = match first {
        "make" => second == Some("agent") || (second == Some("shared") && third == Some("folder")),
        "remove" | "rename" | "show" | "limit" | "reset" => second == Some("agent"),
        "allow" | "forbid" => true,
        "switch" => true,
        "move" => second == Some("agents") && third == Some("to"),
        _ => false,
    };
    if !is_ours {
        return None;
    }
    let mut tokens = tokens.to_vec();
    Some(take_format(&mut tokens).and_then(|format| {
        let w = words(&tokens);
        let (command, canonical) = sentence(&tokens, &w, cwd)?;
        Ok(Parsed { command, format, canonical })
    }))
}

fn sentence(t: &[Token], w: &[String], cwd: &crate::parse::Context) -> Result<(AgentCommand, String), ParseError> {
    let s = |i: usize| w.get(i).map(String::as_str);
    match s(0).unwrap_or("") {
        "make" if s(1) == Some("shared") => {
            // make shared folder with agent "b" [and agent "c"]…
            let mut with = Vec::new();
            let mut i = 3;
            if s(i) != Some("with") {
                return Err(ParseError::new("With whom? `ctx make shared folder with agent \"<name>\"`."));
            }
            i += 1;
            loop {
                if s(i) != Some("agent") {
                    return Err(ParseError::new("With whom? `ctx make shared folder with agent \"<name>\"`."));
                }
                with.push(name_at(t, i + 1, "agent", "ctx make shared folder with agent \"b\"")?);
                i += 2;
                if s(i) == Some("and") {
                    i += 1;
                    continue;
                }
                break;
            }
            no_more(t, i)?;
            let shown = with.iter().map(|a| format!("agent {}", quote(a))).collect::<Vec<_>>().join(" and ");
            Ok((AgentCommand::MakeShared { with }, format!("ctx make shared folder with {shown}")))
        }
        "make" => {
            // make agent named "b" [from <folder>] | make agent "b" | make agent from account "x"
            if s(2) == Some("from") && s(3) == Some("account") {
                let account = name_at(t, 4, "account", "ctx make agent from account \"vibe-daemon\"")?;
                no_more(t, 5)?;
                let c = format!("ctx make agent from account {}", quote(&account));
                return Ok((AgentCommand::Adopt { account }, c));
            }
            let at = if s(2) == Some("named") { 3 } else { 2 };
            let name = name_at(t, at, "name", "ctx make agent named \"email-agent\"")?;
            let mut home = None;
            let mut i = at + 1;
            if s(i) == Some("from") {
                let f = t.get(i + 1).ok_or_else(|| ParseError::new("From which folder? `from ~/Agents/<name>`."))?;
                home = Some(resolve_path(&f.text, cwd));
                i += 2;
            }
            no_more(t, i)?;
            let mut c = format!("ctx make agent named {}", quote(&name));
            if let Some(h) = &home {
                c.push_str(&format!(" from {}", quote(h)));
            }
            Ok((AgentCommand::Make { name, home }, c))
        }
        "remove" => {
            let name = name_at(t, 2, "agent", "ctx remove agent \"email-agent\"")?;
            let with_files = w[3..].join(" ") == "with its files" || w[3..].join(" ") == "with files";
            if !with_files {
                no_more(t, 3)?;
            }
            let mut c = format!("ctx remove agent {}", quote(&name));
            if with_files {
                c.push_str(" with its files");
            }
            Ok((AgentCommand::Remove { name, with_files }, c))
        }
        "rename" => {
            let name = name_at(t, 2, "agent", "ctx rename agent \"email-agent\" to \"Email\"")?;
            if s(3) != Some("to") {
                return Err(ParseError::new(format!(
                    "To what? `ctx rename agent {} to \"<full name>\"`.",
                    quote(&name)
                )));
            }
            let full_name = name_at(t, 4, "name", "ctx rename agent \"email-agent\" to \"Email\"")?;
            no_more(t, 5)?;
            let c = format!("ctx rename agent {} to {}", quote(&name), quote(&full_name));
            Ok((AgentCommand::Rename { name, full_name }, c))
        }
        "show" => {
            let name = name_at(t, 2, "agent", "ctx show agent \"email-agent\"")?;
            no_more(t, 3)?;
            let c = format!("ctx show agent {}", quote(&name));
            Ok((AgentCommand::Show { name }, c))
        }
        "reset" => {
            let name = name_at(t, 2, "agent", "ctx reset agent \"email-agent\"")?;
            no_more(t, 3)?;
            let c = format!("ctx reset agent {}", quote(&name));
            Ok((AgentCommand::Reset { name }, c))
        }
        "switch" => {
            // switch to agent "b"
            if s(1) != Some("to") || s(2) != Some("agent") {
                return Err(ParseError::new("Switch to whom? `ctx switch to agent \"<name>\"`."));
            }
            let name = name_at(t, 3, "agent", "ctx switch to agent \"email-agent\"")?;
            no_more(t, 4)?;
            let c = format!("ctx switch to agent {}", quote(&name));
            Ok((AgentCommand::Switch { name }, c))
        }
        "move" => {
            if w[1..].join(" ") != "agents to accounts" {
                return Err(ParseError::new("`ctx move agents to accounts`."));
            }
            Ok((AgentCommand::MoveToAccounts, "ctx move agents to accounts".into()))
        }
        "limit" => {
            let name = name_at(t, 2, "agent", "ctx limit agent \"email-agent\" to 8 GB of memory")?;
            if s(3) != Some("to") {
                return Err(ParseError::new(format!(
                    "To what? `ctx limit agent {} to 8 GB of memory` (or 200 processes, low priority, nothing).",
                    quote(&name)
                )));
            }
            let rest: Vec<&str> = w[4..].iter().map(String::as_str).collect();
            // "8gb" as one word too.
            let split: Vec<String> = match rest.first() {
                Some(x) if x.ends_with("gb") || x.ends_with("mb") => {
                    let (n, u) = x.split_at(x.len() - 2);
                    std::iter::once(n.to_string())
                        .chain(std::iter::once(u.to_string()))
                        .chain(rest[1..].iter().map(|s| s.to_string()))
                        .collect()
                }
                _ => rest.iter().map(|s| s.to_string()).collect(),
            };
            let r: Vec<&str> = split.iter().map(String::as_str).collect();
            let limit = match r.as_slice() {
                ["nothing"] | ["no", "limits"] => LimitSpec::Nothing,
                ["low", "priority"] => LimitSpec::LowPriority,
                ["normal", "priority"] => LimitSpec::NormalPriority,
                [n, "processes"] => LimitSpec::Processes(
                    n.parse()
                        .ok()
                        .filter(|n| *n > 0)
                        .ok_or_else(|| ParseError::new(format!("\"{n}\" isn't a number of processes.")))?,
                ),
                [n, unit, "of", "memory"] | [n, unit, "memory"] => {
                    LimitSpec::MemoryMb(megabytes(n, unit).ok_or_else(|| {
                        ParseError::new(format!("\"{n} {unit}\" isn't an amount of memory; say 8 GB or 512 MB."))
                    })?)
                }
                _ => {
                    return Err(ParseError::new(
                        "Limit it to what? `8 GB of memory`, `200 processes`, `low priority`, `normal priority` or `nothing`.",
                    ));
                }
            };
            let shown = match &limit {
                LimitSpec::Nothing => "nothing".to_string(),
                LimitSpec::LowPriority => "low priority".into(),
                LimitSpec::NormalPriority => "normal priority".into(),
                LimitSpec::Processes(n) => format!("{n} processes"),
                LimitSpec::MemoryMb(m) if m.is_multiple_of(1024) => format!("{} GB of memory", m / 1024),
                LimitSpec::MemoryMb(m) => format!("{m} MB of memory"),
            };
            let c = format!("ctx limit agent {} to {shown}", quote(&name));
            Ok((AgentCommand::Limit { name, limit }, c))
        }
        verb @ ("allow" | "forbid") => {
            // allow agent "a" to read|write <target>
            let (to, used) = grantee(&t[1..], &w[1..])?;
            let i = 1 + used;
            if s(i) != Some("to") || !matches!(s(i + 1), Some("read" | "write")) {
                return Err(ParseError::new(format!(
                    "{} it to do what? `ctx {verb} {} to read <folder>` (or `to write`).",
                    if verb == "allow" { "Allow" } else { "Forbid" },
                    show_grantee(&to)
                )));
            }
            let write = s(i + 1) == Some("write");
            let (on, used) = target(&t[i + 2..], &w[i + 2..], cwd)?;
            no_more(t, i + 2 + used)?;
            let c = format!(
                "ctx {verb} {} to {} {}",
                show_grantee(&to),
                if write { "write" } else { "read" },
                show_target(&on)
            );
            Ok((AgentCommand::Grant { allow: verb == "allow", to, write, on }, c))
        }
        _ => Err(ParseError::new("That isn't a sentence about agents.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{Context, Token};

    fn ctx() -> Context {
        Context { folder: "/Users/c/code".into(), home: "/Users/c".into(), now_ms: 0 }
    }

    fn p(s: &str) -> Result<Parsed, ParseError> {
        let toks: Vec<Token> = crate::parse::split_for_tests(s).into_iter().map(|text| Token { text }).collect();
        parse(&toks, &ctx()).expect("an agent sentence")
    }

    #[test]
    fn every_example_parses_and_its_canonical_form_parses_the_same() {
        for (s, _) in EXAMPLES {
            let a = p(s.trim_start_matches("ctx ")).unwrap_or_else(|e| panic!("{s}: {}", e.message));
            let b =
                p(a.canonical.trim_start_matches("ctx ")).unwrap_or_else(|e| panic!("{}: {}", a.canonical, e.message));
            assert_eq!(a.command, b.command, "{s}");
            assert_eq!(a.canonical, b.canonical, "{s}");
        }
    }

    #[test]
    fn forms() {
        assert_eq!(p("make agent named b").unwrap().command, AgentCommand::Make { name: "b".into(), home: None });
        assert_eq!(
            p("make agent b from ~/Agents/b").unwrap().command,
            AgentCommand::Make { name: "b".into(), home: Some("/Users/c/Agents/b".into()) }
        );
        assert_eq!(
            p("allow every agent to read ~/.local/bin").unwrap().command,
            AgentCommand::Grant {
                allow: true,
                to: Grantee::Every,
                write: false,
                on: Target::Path("/Users/c/.local/bin".into())
            }
        );
        assert_eq!(
            p("forbid agent a to write agent b").unwrap().command,
            AgentCommand::Grant {
                allow: false,
                to: Grantee::Agent("a".into()),
                write: true,
                on: Target::Agent("b".into())
            }
        );
        assert_eq!(
            p("limit agent b to 8gb of memory").unwrap().command,
            AgentCommand::Limit { name: "b".into(), limit: LimitSpec::MemoryMb(8192) }
        );
        assert_eq!(
            p("limit agent b to 200 processes").unwrap().command,
            AgentCommand::Limit { name: "b".into(), limit: LimitSpec::Processes(200) }
        );
        assert_eq!(
            p("remove agent b with its files").unwrap().command,
            AgentCommand::Remove { name: "b".into(), with_files: true }
        );
        assert_eq!(
            p("make shared folder with agent b and agent c").unwrap().command,
            AgentCommand::MakeShared { with: vec!["b".into(), "c".into()] }
        );
        assert_eq!(p("show agent b as json").unwrap().format, Format::Json);
        assert!(p("limit agent b to lots").is_err());
        assert!(p("allow agent b to eat ~/x").is_err());
        assert!(p("switch agent b").is_err());
        // Not ours: the main grammar's sentences.
        let toks: Vec<Token> = crate::parse::split_for_tests("make launcher named x for /bin/ls at ~")
            .into_iter()
            .map(|text| Token { text })
            .collect();
        assert!(parse(&toks, &ctx()).is_none());
        let toks: Vec<Token> =
            crate::parse::split_for_tests("remove hooks for claude").into_iter().map(|text| Token { text }).collect();
        assert!(parse(&toks, &ctx()).is_none());
    }
}
