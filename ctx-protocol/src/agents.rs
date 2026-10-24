//! Agents: the principals other than you. Each one is configured in `config.toml` under
//! `[agents.<name>]`, is what the activity log names, sees and drives only the tabs it's allowed
//! to, and runs under its own profile: a Seatbelt sandbox built here, and optionally its own
//! macOS account.
//!
//! The sandbox closes your home folder, then gives back what the agent's `read`, `write` and
//! `deny` lists say. Rules are ordered from the shortest path to the longest, and Seatbelt lets
//! the last matching rule decide, so the most specific entry wins whatever its kind
//! (`write = ["~"]` with `deny = ["~/Library"]` and `write = ["~/Library/Caches"]` does what it
//! reads as). Every profile also carries a marker, a path only that agent may read, which is how
//! the server tells which agent a sandboxed process belongs to.

use crate::config::{Agent, Config};
use std::path::{Path, PathBuf};

/// The principal for you: CTX.app, and processes no agent rule matches.
pub const YOU: &str = "you";

/// Paths the profile needs that aren't in the agent's own config.
#[derive(Debug, Clone)]
pub struct Places {
    /// Your home folder (the one closed by default).
    pub home: PathBuf,
    /// The agent account's own home, when it runs as one.
    pub agent_home: Option<PathBuf>,
    /// ctx's data folder, config folder and runtime folder: never writable by an agent.
    pub ctx_data: PathBuf,
    pub ctx_config: PathBuf,
    pub ctx_runtime: PathBuf,
    /// Files in ctx's data folder this sandbox may write (the tab's shell history).
    pub own_files: Vec<PathBuf>,
    /// The program a shim starts: readable (it and its link target) even if it's in your home.
    pub programs: Vec<PathBuf>,
}

/// The folder holding every agent's marker; `<runtime>/agents/<name>` is readable only by `name`.
pub fn markers(runtime: &Path) -> PathBuf {
    runtime.join("agents")
}

pub fn marker(runtime: &Path, agent: &str) -> PathBuf {
    markers(runtime).join(agent)
}

/// Create an agent's marker file. Seatbelt only answers for paths that exist, so the server can
/// recognise the agent's processes once this file is there.
pub fn ensure_marker(runtime: &Path, agent: &str) -> std::io::Result<()> {
    let m = marker(runtime, agent);
    if !m.exists() {
        std::fs::create_dir_all(markers(runtime))?;
        std::fs::write(&m, "")?;
    }
    Ok(())
}

/// Where the shims live: `<runtime>/shims`.
pub fn shims_dir(runtime: &Path) -> PathBuf {
    runtime.join("shims")
}

/// One shim's text: it runs `ctx shim <program>` with the same arguments.
pub fn shim_script(ctx: &Path, program: &str) -> String {
    use std::os::unix::ffi::OsStrExt;
    let q = |s: &[u8]| format!("'{}'", String::from_utf8_lossy(s).replace('\'', "'\\''"));
    format!(
        "#!/bin/sh\n# ctx: {program} runs as the agent for its folder (config.toml [agents]).\nexec {} shim {} \"$@\"\n",
        q(ctx.as_os_str().as_bytes()),
        q(program.as_bytes())
    )
}

/// `~` and `~/…` against `home`; other paths as given.
pub fn expand(path: &str, home: &Path) -> PathBuf {
    if path == "~" {
        home.to_path_buf()
    } else if let Some(rest) = path.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(path)
    }
}

/// The path Seatbelt sees: symbolic links resolved (`/tmp` is `/private/tmp`), and for a path
/// that doesn't exist yet, its nearest existing parent resolved.
pub fn real(path: &Path) -> PathBuf {
    if let Ok(p) = std::fs::canonicalize(path) {
        return p;
    }
    let mut rest = Vec::new();
    let mut p = path.to_path_buf();
    while let Some(parent) = p.parent().map(Path::to_path_buf) {
        if let Some(name) = p.file_name() {
            rest.push(name.to_os_string());
        }
        if let Ok(mut r) = std::fs::canonicalize(&parent) {
            for n in rest.iter().rev() {
                r.push(n);
            }
            return r;
        }
        p = parent;
    }
    path.to_path_buf()
}

/// The agent an agent program started in `folder` becomes: the agent whose `folders` holds it
/// most specifically, else `default_agent`. `None` when neither applies (it stays you).
pub fn agent_for_folder<'a>(config: &'a Config, folder: &Path, home: &Path) -> Option<&'a str> {
    let folder = real(folder);
    let mut best: Option<(&str, usize)> = None;
    for (name, a) in &config.agents {
        for f in &a.folders {
            let f = real(&expand(f, home));
            if folder.starts_with(&f) {
                let depth = f.components().count();
                if best.is_none_or(|(_, d)| depth > d) {
                    best = Some((name, depth));
                }
            }
        }
    }
    if let Some((name, _)) = best {
        return Some(name);
    }
    let d = config.default_agent.as_str();
    (!d.is_empty() && config.agents.contains_key(d)).then_some(d)
}

/// The agent a scoped launcher starts, if one names it.
pub fn agent_for_launcher<'a>(config: &'a Config, launcher: &str) -> Option<&'a str> {
    config.agents.iter().find(|(_, a)| a.launcher == launcher).map(|(n, _)| n.as_str())
}

/// An agent's profile with `agent_base` added: its sandbox gets both.
pub fn profile(config: &Config, name: &str) -> Option<Agent> {
    let mut a = config.agents.get(name)?.clone();
    let b = &config.agent_base;
    for (list, base) in [(&mut a.read, &b.read), (&mut a.write, &b.write), (&mut a.deny, &b.deny)] {
        list.extend(base.iter().filter(|p| !list.contains(p)).cloned().collect::<Vec<_>>());
    }
    Some(a)
}

/// Whether a principal's rule list (`see_tabs_of`, `drive_tabs_of`) covers `owner`.
pub fn covers(list: &[String], owner: &str) -> bool {
    list.iter().any(|x| x == "*" || x.eq_ignore_ascii_case(owner))
}

/// Seatbelt's string syntax.
fn quote(p: &Path) -> String {
    let s = p.display().to_string();
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// A filter for a configured path: `(subpath …)`, or `(regex …)` when it has `*` (one level) or
/// `**` (any depth). Either way it covers everything under the match.
fn filter(path: &str, home: &Path) -> (String, usize) {
    let expanded = expand(path, home);
    let depth = expanded.components().count();
    if !path.contains('*') {
        return (format!("(subpath {})", quote(&real(&expanded))), depth);
    }
    // Resolve the part before the first wildcard, then turn the rest into a pattern.
    let text = expanded.display().to_string();
    let cut = text[..text.find('*').unwrap_or(text.len())].rfind('/').unwrap_or(0);
    let base = real(Path::new(&text[..cut.max(1)])).display().to_string();
    let mut re = String::from("^");
    for c in base.chars().chain(text[cut..].chars()) {
        match c {
            '.' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|' | '\\' => {
                re.push('\\');
                re.push(c);
            }
            '"' => re.push_str("\\\""),
            _ => re.push(c),
        }
    }
    let re = re.replace("**", "\u{1}").replace('*', "[^/]*").replace('\u{1}', ".*");
    (format!("(regex #\"{re}(/.*)?$\")", re = re.trim_end_matches('/')), depth)
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Read,
    Write,
    Deny,
}

/// The Seatbelt profile for agent `name`.
pub fn sandbox_profile(name: &str, agent: &Agent, places: &Places) -> String {
    let home = real(&places.home);
    let mut p = String::new();
    p.push_str(&format!(";; ctx agent \"{name}\"\n(version 1)\n(allow default)\n"));

    // Writes: nowhere but temporary folders and devices, until the agent's grants.
    p.push_str("(deny file-write* (subpath \"/\"))\n");
    p.push_str(
        "(allow file-write* (subpath \"/private/tmp\") (subpath \"/private/var/folders\") (subpath \"/dev\"))\n",
    );
    if let Some(h) = &places.agent_home {
        p.push_str(&format!("(allow file-read* file-write* (subpath {}))\n", quote(&real(h))));
    }

    // Your home: closed. Its own folder entry stays visible so paths through it resolve.
    p.push_str(&format!("(deny file-read* file-write* (subpath {}))\n", quote(&home)));
    p.push_str(&format!("(allow file-read-metadata (literal {}))\n", quote(&home)));

    // The agent's grants, most specific last.
    let mut rules: Vec<(usize, Kind, String, String)> = Vec::new();
    for (list, kind) in [(&agent.read, Kind::Read), (&agent.write, Kind::Write), (&agent.deny, Kind::Deny)] {
        for path in list {
            let (f, depth) = filter(path, &places.home);
            rules.push((depth, kind, f, path.clone()));
        }
    }
    rules.sort_by_key(|a| (a.0, a.1));
    let mut parents = std::collections::BTreeSet::new();
    for (_, kind, f, path) in &rules {
        let rule = match kind {
            Kind::Read => format!("(allow file-read* {f})\n(deny file-write* {f})\n"),
            Kind::Write => format!("(allow file-read* file-write* {f})\n"),
            Kind::Deny => format!("(deny file-read* file-write* {f})\n"),
        };
        p.push_str(&format!(";; {path}\n{rule}"));
        // Folders on the way to a grant inside your home can be looked up, not listed.
        if *kind != Kind::Deny {
            let mut d = real(&expand(path.split('*').next().unwrap_or(path), &places.home));
            while let Some(parent) = d.parent().map(Path::to_path_buf) {
                if !parent.starts_with(&home) || parent == home {
                    break;
                }
                parents.insert(parent.clone());
                d = parent;
            }
        }
    }
    for d in parents {
        p.push_str(&format!("(allow file-read-metadata (literal {}))\n", quote(&d)));
    }

    if !agent.network {
        p.push_str("(deny network-outbound (remote ip))\n(deny network-inbound (local ip))\n");
    }

    // Always, whatever the grants say: ctx's own files stay out of reach (they decide what agents
    // may do, and hold every tab's record), and the ways to start work outside the sandbox are
    // closed (Apple Events, and Launch Services behind `open`). `launchctl submit` is refused to
    // every sandboxed process already.
    for d in [&places.ctx_data, &places.ctx_config, &places.ctx_runtime] {
        p.push_str(&format!("(deny file-write* (subpath {}))\n", quote(&real(d))));
    }
    p.push_str(&format!("(deny file-read* (subpath {}))\n", quote(&real(&places.ctx_data))));
    for f in &places.programs {
        let mut files = vec![f.clone(), real(f)];
        files.dedup();
        for file in files {
            p.push_str(&format!("(allow file-read* (literal {}))\n", quote(&file)));
        }
    }
    for f in &places.own_files {
        p.push_str(&format!("(allow file-read* file-write* (literal {}))\n", quote(&real(f))));
    }
    let markers = markers(&places.ctx_runtime);
    p.push_str(&format!("(deny file-read* (subpath {}))\n", quote(&real(&markers))));
    p.push_str(&format!("(allow file-read* (literal {}))\n", quote(&real(&markers).join(name))));
    p.push_str("(deny appleevent-send)\n");
    p.push_str("(deny mach-lookup (global-name \"com.apple.coreservices.launchservicesd\") (global-name-prefix \"com.apple.lsd.\"))\n");
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn places(root: &Path) -> Places {
        Places {
            home: root.join("home"),
            agent_home: None,
            ctx_data: root.join("home/Library/Application Support/ctx"),
            ctx_config: root.join("home/.config/ctx"),
            ctx_runtime: root.join("run"),
            own_files: vec![],
            programs: vec![],
        }
    }

    #[test]
    fn folders_pick_the_most_specific_agent() {
        let c = Config::parse(
            "default_agent = \"dev\"\n[agents.dev]\nfolders = [\"~/Workspaces\"]\n[agents.econ]\nfolders = [\"~/Agents/econ\"]\n[agents.inner]\nfolders = [\"~/Workspaces/inner\"]\n",
        )
        .unwrap();
        let home = Path::new("/nonexistent-home");
        assert_eq!(agent_for_folder(&c, &home.join("Agents/econ/work"), home), Some("econ"));
        assert_eq!(agent_for_folder(&c, &home.join("Workspaces/inner/x"), home), Some("inner"));
        assert_eq!(agent_for_folder(&c, &home.join("Workspaces/other"), home), Some("dev"));
        assert_eq!(agent_for_folder(&c, Path::new("/opt"), home), Some("dev"));
        let none = Config::parse("[agents.dev]\nfolders = [\"~/Workspaces\"]\n").unwrap();
        assert_eq!(agent_for_folder(&none, Path::new("/opt"), home), None);
    }

    #[test]
    fn rules_run_from_general_to_specific() {
        let a = Agent {
            write: vec!["~".into(), "~/Library/Caches".into()],
            deny: vec!["~/Library".into(), "~/Workspaces/*/.git/hooks".into()],
            ..Agent::default()
        };
        let p = sandbox_profile("dev", &a, &places(Path::new("/nonexistent")));
        let at = |s: &str| p.find(s).unwrap_or_else(|| panic!("{s} missing from\n{p}"));
        assert!(at(";; ~\n") < at(";; ~/Library\n"));
        assert!(at(";; ~/Library\n") < at(";; ~/Library/Caches\n"));
        assert!(p.contains(r#"(regex #"^/nonexistent/home/Workspaces/[^/]*/\.git/hooks(/.*)?$")"#), "{p}");
        // ctx's own rules come after every grant.
        assert!(at(";; ~/Library/Caches\n") < at("(deny appleevent-send)"));
        assert!(p.contains("(allow file-read* (literal \"/nonexistent/run/agents/dev\"))"));
        assert!(!p.contains("network-outbound"), "network stays on by default");
    }
}
