//! `ctx move agents to accounts`: turn the `[agents.*]` sections of config.toml into agent
//! accounts, show the plan first, then take those sections out (a copy of config.toml is kept).

use ctx_accounts::accounts;
use ctx_cli::agent_sentences::{AgentCommand, Grantee, Target};
use ctx_protocol::agents::expand;
use ctx_protocol::config::Config;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq)]
pub enum Step {
    /// An existing account joins ctx-agents.
    Adopt { agent: String, account: String },
    /// A new account with its home in agents' homes; `from` is its folder in ~/Agents, moved
    /// there (a link to it stays in its place).
    Make { agent: String, name: String, home: PathBuf, from: Option<PathBuf> },
    /// A grant from the agent's `read`/`write` list.
    Grant { name: String, path: String, write: bool },
    /// Something the plan can't carry over.
    Skip { agent: String, why: String },
}

/// An agent name made into an account name: lowercase, letters, digits and `-`.
pub fn account_name(agent: &str) -> String {
    let mut s: String = agent
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    if !s.starts_with(|c: char| c.is_ascii_lowercase()) {
        s = format!("agent-{s}");
    }
    s.truncate(31);
    s
}

/// The steps for `config`'s agents. `agents_dir` is your ~/Agents, `homes` where agents' homes go.
pub fn plan(config: &Config, agents_dir: &Path, homes: &Path, home: &Path) -> Vec<Step> {
    let mut steps = Vec::new();
    for (agent, a) in &config.agents {
        let name = if !a.account.is_empty() {
            match accounts::by_name(&a.account) {
                Some(acct) if acct.uid == 0 || accounts::is_admin(&a.account) => {
                    steps.push(Step::Skip {
                        agent: agent.clone(),
                        why: format!(
                            "its account \"{}\" is an administrator; agents' accounts are standard",
                            a.account
                        ),
                    });
                    continue;
                }
                Some(acct) if accounts::is_agent(&acct) => a.account.clone(),
                Some(_) => {
                    steps.push(Step::Adopt { agent: agent.clone(), account: a.account.clone() });
                    a.account.clone()
                }
                None => {
                    steps.push(Step::Skip {
                        agent: agent.clone(),
                        why: format!("its account \"{}\" doesn't exist", a.account),
                    });
                    continue;
                }
            }
        } else {
            let name = account_name(agent);
            if accounts::by_name(&name).is_some() {
                steps.push(Step::Skip {
                    agent: agent.clone(),
                    why: format!(
                        "there's already an account \"{name}\"; adopt it with: ctx make agent from account \"{name}\""
                    ),
                });
                continue;
            }
            // Its folder in ~/Agents (not a link already) becomes its home, moved out of yours.
            let from = a
                .folders
                .iter()
                .map(|f| expand(f, home))
                .find(|f| f.parent() == Some(agents_dir) && std::fs::symlink_metadata(f).is_ok_and(|m| m.is_dir()));
            steps.push(Step::Make { agent: agent.clone(), name: name.clone(), home: homes.join(&name), from });
            name
        };
        for (list, write) in [(&a.read, false), (&a.write, true)] {
            for p in list {
                let path = expand(p, home);
                if path.exists() {
                    steps.push(Step::Grant { name: name.clone(), path: path.display().to_string(), write });
                }
            }
        }
    }
    steps
}

fn describe(s: &Step) -> String {
    match s {
        Step::Adopt { agent, account } => {
            format!("\"{agent}\" becomes the agent \"{account}\" (its account joins ctx-agents)")
        }
        Step::Make { agent, name, home, from: Some(from) } => {
            format!(
                "\"{agent}\" becomes the account \"{name}\"; {} moves to {} as its home, with a link left in its place (its files become the agent's; you keep full access)",
                from.display(),
                home.display()
            )
        }
        Step::Make { agent, name, home, from: None } => {
            format!("\"{agent}\" becomes the account \"{name}\" with a new home {}", home.display())
        }
        Step::Grant { name, path, write } => {
            format!("  \"{name}\" may {} {path}", if *write { "read and write" } else { "read" })
        }
        Step::Skip { agent, why } => format!("\"{agent}\" stays as it is: {why}"),
    }
}

/// config.toml without its `[agents.*]` tables.
pub fn without_agents(text: &str) -> String {
    let mut out = String::new();
    let mut skipping = false;
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with('[') {
            let header = t.trim_start_matches('[').trim_start();
            skipping = header.starts_with("agents.") || header.starts_with("agents]");
        }
        if !skipping {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

pub fn run(interactive: bool) -> Result<Vec<String>, String> {
    let paths = crate::client::paths();
    let config = Config::load(&paths.config)?;
    if config.agents.is_empty() {
        return Ok(vec!["config.toml has no [agents] sections; there's nothing to move.".into()]);
    }
    let home = PathBuf::from(ctx_server::sys::home());
    let agents_dir = crate::accounts::agents_dir();
    let homes = crate::accounts::agent_homes();
    let steps = plan(&config, &agents_dir, &homes, &home);
    let mut lines = vec!["The plan:".to_string()];
    lines.extend(steps.iter().map(describe));
    if !interactive || !std::io::stdin().is_terminal() {
        lines.push("Run this at a terminal to carry it out; it asks first.".into());
        return Ok(lines);
    }
    for l in &lines {
        eprintln!("{l}");
    }
    eprint!("Carry it out and take the [agents] sections out of config.toml (a copy is kept)? [y/N] ");
    let _ = std::io::stderr().flush();
    let mut answer = String::new();
    let _ = std::io::stdin().read_line(&mut answer);
    if !answer.trim().eq_ignore_ascii_case("y") {
        return Ok(vec!["Nothing changed.".into()]);
    }
    let mut done = Vec::new();
    let mut failed = false;
    for s in &steps {
        let cmd = match s {
            Step::Adopt { account, .. } => AgentCommand::Adopt { account: account.clone() },
            Step::Make { name, home, from, .. } => {
                // `from` moves beside other agents' homes as it's made.
                AgentCommand::Make {
                    name: name.clone(),
                    home: Some(from.as_ref().unwrap_or(home).display().to_string()),
                }
            }
            Step::Grant { name, path, write } => AgentCommand::Grant {
                allow: true,
                to: Grantee::Agent(name.clone()),
                write: *write,
                on: Target::Path(path.clone()),
            },
            Step::Skip { .. } => continue,
        };
        let o = crate::accounts::run(cmd, String::new(), ctx_cli::parse::Format::Text, false);
        done.extend(o.stdout.lines().map(str::to_string));
        if o.code != 0 {
            failed = true;
            done.push(format!("Stopped: {}", o.stderr.trim()));
            break;
        }
    }
    if failed {
        done.push("config.toml wasn't changed. Fix that and run `ctx move agents to accounts` again; finished steps are skipped.".into());
        return Ok(done);
    }
    let text = std::fs::read_to_string(&paths.config).map_err(|e| e.to_string())?;
    let copy = paths.config.with_file_name("config.toml.before-accounts");
    std::fs::write(&copy, &text).map_err(|e| format!("couldn't keep a copy of config.toml: {e}"))?;
    std::fs::write(&paths.config, without_agents(&text)).map_err(|e| format!("couldn't update config.toml: {e}"))?;
    done.push(format!("Took the [agents] sections out of config.toml; the old file is {}.", copy.display()));
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(account_name("Email Agent"), "email-agent");
        assert_eq!(account_name("sys_admin"), "sys-admin");
        assert_eq!(account_name("2fa"), "agent-2fa");
    }

    #[test]
    fn agents_tables_come_out() {
        let t = "shell = \"zsh\"\n[agents.dev]\nfolders = [\"~/x\"]\n[agents.dev.extra]\na = 1\n[ask]\nmodel = \"m\"\n";
        assert_eq!(without_agents(t), "shell = \"zsh\"\n[ask]\nmodel = \"m\"\n");
    }

    #[test]
    fn a_folder_in_agents_becomes_the_home() {
        let d = std::env::temp_dir().join(format!("ctx-migrate-{}", std::process::id()));
        let agents_dir = d.join("Agents");
        std::fs::create_dir_all(agents_dir.join("mail")).unwrap();
        std::fs::create_dir_all(d.join("Context/career")).unwrap();
        let text = format!(
            "[agents.Mail_bot]\nfolders = [\"{}\"]\nread = [\"~/Context/career\", \"~/missing\"]\n",
            agents_dir.join("mail").display()
        );
        let config = Config::parse(&text).unwrap();
        let steps = plan(&config, &agents_dir, &d.join("homes"), &d);
        assert_eq!(
            steps,
            vec![
                Step::Make {
                    agent: "Mail_bot".into(),
                    name: "mail-bot".into(),
                    home: d.join("homes/mail-bot"),
                    from: Some(agents_dir.join("mail"))
                },
                Step::Grant {
                    name: "mail-bot".into(),
                    path: d.join("Context/career").display().to_string(),
                    write: false
                },
            ]
        );
        let _ = std::fs::remove_dir_all(&d);
    }
}
