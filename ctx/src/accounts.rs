//! `ctx make agent`, `allow`, `forbid`, `switch to agent`, `limit`, `reset`, `show`, `remove`,
//! `rename`: agents as accounts; and spaces (`make space`, `add agent … to space`). Every change to files is made as whoever runs ctx, so the OS's
//! rule holds: only a file's owner shares it. Account changes go to the ctx helper (root), which
//! does them only for the person who owns the ctx server.

use crate::client::{self, Connection};
use crate::exec::Out;
use ctx_accounts::accounts::{self, Account};
use ctx_accounts::acl::{self, Access, Who};
use ctx_cli::agent_sentences::{AgentCommand, Grantee, LimitSpec, Target};
use ctx_cli::parse::{Format, quote};
use ctx_protocol::helper::{AGENT_FILE, AgentFile, EntryFor, Grant, HelperReply, HelperRequest, StarterFile};
use ctx_protocol::{ClientKind, Envelope, Output, Request, Response, TabRef};
use serde::{Deserialize, Serialize};
use std::io::{IsTerminal, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

pub fn run(cmd: AgentCommand, canonical: String, format: Format, interactive: bool) -> Out {
    let mut out = Out::default();
    let r = match cmd {
        AgentCommand::Make { name } => make(&name),
        AgentCommand::Remove { name, with_files } => remove(&name, with_files, interactive),
        AgentCommand::Rename { name, full_name } => rename(&name, &full_name),
        AgentCommand::Show { name } => return show(&name, format),
        AgentCommand::Grant { allow, to, write, on } => grant(allow, &to, write, &on),
        AgentCommand::Switch { name } => return switch(&name, canonical, interactive),
        AgentCommand::Limit { name, limit } => set_limit(&name, limit),
        AgentCommand::Reset { name } => reset(&name),
        AgentCommand::MakeSpace { name } => make_space(&name),
        AgentCommand::RemoveSpace { name, with_files } => remove_space(&name, with_files, interactive),
        AgentCommand::SpaceMember { agent, space, member } => space_member(&agent, &space, member),
        AgentCommand::ListSpaces => return list_spaces(format),
    };
    match r {
        Ok(lines) => {
            for l in lines {
                out.line(l);
            }
            out
        }
        Err(e) => {
            out.err(e);
            out.ctx_error = true;
            out.code(1)
        }
    }
}

type Lines = Result<Vec<String>, String>;

fn me() -> Account {
    // SAFETY: getuid.
    accounts::by_uid(unsafe { libc::getuid() }).expect("the account running ctx")
}

/// Whether ctx runs as an agent (an account in ctx-agents) rather than as the person.
fn running_as_agent() -> bool {
    accounts::is_agent(&me())
}

/// `$CTX_AGENTS`, as the tab exports it, else `[folders] agents` in config.toml, else ~/Agents.
pub fn agents_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("CTX_AGENTS").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    let home = ctx_server::sys::home();
    let configured = ctx_protocol::config::Config::load(&client::paths().config)
        .ok()
        .and_then(|c| c.folders.env(&home).into_iter().find(|(k, _)| *k == "CTX_AGENTS").map(|(_, v)| v));
    PathBuf::from(configured.unwrap_or_else(|| format!("{home}/Agents")))
}

/// The agent account called `name`.
fn agent(name: &str) -> Result<Account, String> {
    let a = accounts::by_name(name).ok_or_else(|| {
        format!(
            "There's no agent \"{name}\". `ctx list agents` shows them; `ctx make agent named \"{name}\"` makes one."
        )
    })?;
    if !accounts::is_agent(&a) {
        return Err(format!("\"{name}\" is an account, but not an agent's."));
    }
    Ok(a)
}

pub(crate) fn helper(req: HelperRequest) -> Result<String, String> {
    match ctx_protocol::helper::ask(&req)? {
        HelperReply::Done { message } => Ok(message),
        HelperReply::Refused { message } => Err(message),
        other => Err(format!("The ctx helper answered something unexpected: {other:?}")),
    }
}

fn only_you(what: &str) -> Result<(), String> {
    if running_as_agent() {
        return Err(format!(
            "Only the person this Mac belongs to can {what}: accounts need an administrator, and the ctx helper does it only for them."
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Making and removing
// ---------------------------------------------------------------------------------------------

/// The starter instructions for a new agent.
pub fn starter_instructions(name: &str) -> String {
    format!(
        "# {name}\n\n\
You are the agent \"{name}\". You run as your own account, `{name}`, and this folder is your home. Write your instructions for yourself here.\n\n\
## Where things are\n\n\
- Your home is private: no other agent may use it unless the person lets one.\n\
- `$CTX_SPACES`: spaces, folders you work in with other agents. You may use the ones the person added you to (`ctx show agent \"{name}\"` lists them).\n\
- `$CTX_CONTEXT` (~/Context): the person's documents. You see only what they shared with you.\n\
- `$CTX_WORKSPACES` (~/Workspaces): code projects.\n\n\
## Permissions\n\n\
What you can read and write is decided by the operating system's file permissions. If something is refused, ask the person to share it: `ctx allow agent \"{name}\" to read <folder>`. You can share your own files with another agent the same way. `ctx show agent \"{name}\"` shows what you were given.\n\n\
`open`, `pbcopy` and `xdg-open` hand links, files and text to the person's session. Their clipboard isn't readable.\n\n\
## Terminals\n\n\
Run commands through ctx (`ctx help`). Your tabs are yours; you see other agents' tabs only where you can read their home.\n"
    )
}

fn starter_files(name: &str) -> Vec<StarterFile> {
    let text = starter_instructions(name);
    vec![
        StarterFile { path: "CLAUDE.md".into(), text: text.clone(), yours: false },
        StarterFile { path: "AGENTS.md".into(), text, yours: false },
        StarterFile { path: AGENT_FILE.into(), text: AgentFile::default().to_text(), yours: true },
    ]
}

fn make(name: &str) -> Lines {
    only_you("make agents")?;
    let mut lines = vec![helper(HelperRequest::MakeAccount {
        name: name.into(),
        full_name: name.into(),
        files: starter_files(name),
    })?];
    let home = accounts::by_name(name).map(|a| a.home).unwrap_or_else(|| accounts::home_of(name));
    lines.extend(link_in_agents(name, Path::new(&home)));
    lines.extend(open_home());
    lines.push(format!("Talk to it: ctx switch to agent {}", quote(name)));
    Ok(lines)
}

/// A warning when your home lets every account in: then agents read your files (any that
/// others may read) without being granted them, and access lists can't stop it.
fn open_home() -> Option<String> {
    use std::os::unix::fs::PermissionsExt;
    let home = std::env::var_os("HOME")?;
    let mode = std::fs::metadata(&home).ok()?.permissions().mode();
    (mode & 0o005 != 0).then(|| {
        format!(
            "Your home folder ({}) lets every account in, so agents can read your files without being given them. Make it private: chmod go-rwx \"$HOME\"",
            Path::new(&home).display()
        )
    })
}

/// `~/Agents/<name>` links to the agent's home (what's there already is left alone).
fn link_in_agents(name: &str, home: &Path) -> Option<String> {
    let dir = agents_dir();
    let link = dir.join(name);
    if link == home || std::fs::symlink_metadata(&link).is_ok() {
        return None;
    }
    let made = std::fs::create_dir_all(&dir).and_then(|_| std::os::unix::fs::symlink(home, &link));
    Some(match made {
        Ok(()) => format!("{} links to it.", link.display()),
        Err(e) => format!("couldn't link {} to it: {e}", link.display()),
    })
}

/// The `~/Agents/<name>` link to `home`, if that's what it is.
fn unlink_in_agents(name: &str, home: &str) {
    let link = agents_dir().join(name);
    if std::fs::read_link(&link).is_ok_and(|to| to == Path::new(home)) {
        let _ = std::fs::remove_file(link);
    }
}

fn pass_through(path: &Path, who: Who) {
    ctx_accounts::shared::pass_through(path, who)
}

fn remove(name: &str, with_files: bool, interactive: bool) -> Lines {
    only_you("remove agents")?;
    let a = agent(name)?;
    if interactive && std::io::stdin().is_terminal() {
        let what = if with_files {
            format!("the account \"{name}\", and move its home {} to your Trash", a.home)
        } else {
            format!("the account \"{name}\" (its home {} stays)", a.home)
        };
        if !confirm(&format!("Delete {what}?")) {
            return Ok(vec!["Kept it.".into()]);
        }
    }
    // Take back the grants ctx made for it while its uid still names it.
    let mut lines = Vec::new();
    let notes = Notes::load();
    let mut left = Notes::default();
    for n in notes.grants {
        if n.to == grantee_key(&Grantee::Agent(name.into())) {
            let _ = acl::apply(Path::new(&n.path), Who::User(a.uid), None);
            // Its way there too: the account goes, so nothing should name its uid.
            ctx_accounts::shared::forget_passing(Path::new(&n.path), Who::User(a.uid));
        } else {
            left.grants.push(n);
        }
    }
    left.save();
    lines.push(helper(HelperRequest::RemoveAccount { name: name.into(), trash_home: with_files })?);
    if with_files {
        unlink_in_agents(name, &a.home);
    }
    Ok(lines)
}

fn rename(name: &str, full_name: &str) -> Lines {
    only_you("rename agents")?;
    agent(name)?;
    Ok(vec![helper(HelperRequest::SetFullName { name: name.into(), full_name: full_name.into() })?])
}

// ---------------------------------------------------------------------------------------------
// Grants
// ---------------------------------------------------------------------------------------------

fn grantee_key(g: &Grantee) -> String {
    match g {
        Grantee::Agent(a) => format!("agent {}", quote(a)),
        Grantee::Every => "every agent".into(),
    }
}

/// A grant ctx made, noted so `show agent` can list it and check it still holds.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct Note {
    to: String,
    path: String,
    write: bool,
}

#[derive(Serialize, Deserialize, Default, Debug)]
struct Notes {
    grants: Vec<Note>,
}

impl Notes {
    fn path() -> PathBuf {
        client::paths().config.with_file_name("grants.json")
    }
    fn load() -> Notes {
        std::fs::read_to_string(Self::path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }
    fn save(&self) {
        let p = Self::path();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(&p, serde_json::to_string_pretty(self).unwrap_or_default());
    }
    fn set(&mut self, to: &Grantee, path: &Path, write: Option<bool>) {
        let to = grantee_key(to);
        let path = path.display().to_string();
        self.grants.retain(|n| !(n.to == to && n.path == path));
        if let Some(write) = write {
            self.grants.push(Note { to, path, write });
        }
    }
}

fn who_of(g: &Grantee) -> Result<(Who, String), String> {
    match g {
        Grantee::Agent(a) => {
            let acct = agent(a)?;
            Ok((Who::User(acct.uid), format!("the agent \"{a}\"")))
        }
        Grantee::Every => {
            let (gid, _) = accounts::group(accounts::AGENTS_GROUP)
                .ok_or("There are no agents yet (no ctx-agents group). `ctx make agent named \"<name>\"` makes one.")?;
            Ok((Who::Group(gid), "every agent".into()))
        }
    }
}

fn grant(allow: bool, to: &Grantee, write: bool, on: &Target) -> Lines {
    let (who, whom) = who_of(to)?;
    let path = match on {
        Target::Path(p) => PathBuf::from(p),
        Target::Agent(b) => PathBuf::from(agent(b)?.home),
    };
    let what = match on {
        Target::Path(p) => p.clone(),
        Target::Agent(b) => format!("the agent \"{b}\"'s home ({})", path.display()),
    };
    let meta = std::fs::symlink_metadata(&path).map_err(|_| format!("There's no {}.", path.display()))?;
    let mine = me().uid;
    // allow read → Read; allow write → Write; forbid write → Read; forbid read → nothing at all.
    let access = match (allow, write) {
        (true, true) => Some(Access::Write),
        (true, false) | (false, true) => Some(Access::Read),
        (false, false) => None,
    };
    let as_helper = |p: &Path| {
        let entry = match who {
            Who::User(u) => EntryFor::User(u),
            Who::Group(g) => EntryFor::Group(g),
        };
        let g = access.map(|a| match a {
            Access::Read => Grant::Read,
            Access::Write => Grant::Write,
            Access::ReadOnly => Grant::ReadOnly,
            Access::Nothing => Grant::Nothing,
        });
        helper(HelperRequest::Share { path: p.display().to_string(), entry, grant: g })
    };
    let owner_is_agent = accounts::by_uid(meta.uid()).is_some_and(|a| accounts::is_agent(&a));
    let mut left = 0;
    if meta.uid() == mine {
        let done = acl::apply(&path, who, access).map_err(|e| format!("couldn't change {}: {e}", path.display()))?;
        left = done.not_owned.len();
        // Files inside that agents wrote: the helper updates those, for the person only.
        if left > 0 && !running_as_agent() && as_helper(&path).is_ok() {
            left = 0;
        }
    } else if owner_is_agent && !running_as_agent() {
        // An agent's own files: you have full access to them, and the helper changes them for you.
        as_helper(&path)?;
    } else {
        let owner = accounts::by_uid(meta.uid()).map(|a| a.name).unwrap_or_else(|| meta.uid().to_string());
        return Err(format!(
            "{} belongs to {owner}, so only {owner} can share it (the OS lets only a file's owner change who may use it).",
            path.display()
        ));
    }
    if allow {
        pass_through(&path, who);
    } else if !write && meta.uid() == mine {
        // Its way here may still be open (a folder it passes through toward something else), and
        // the file's own mode may let anyone read it: close it outright.
        acl::apply_one(&path, who, Some(Access::Nothing), false)
            .map_err(|e| format!("couldn't close {} to {whom}: {e}", path.display()))?;
    }
    let mut lines = Vec::new();
    let mut notes = Notes::load();
    notes.set(to, &path, access.map(|a| a == Access::Write));
    notes.save();
    lines.push(match (allow, write) {
        (true, true) => format!("{} may now read and write {what}.", capitalise(&whom)),
        (true, false) => format!("{} may now read {what}.", capitalise(&whom)),
        (false, true) => format!("{} may now only read {what}.", capitalise(&whom)),
        (false, false) => format!("{} may no longer use {what}.", capitalise(&whom)),
    });
    if left > 0 {
        lines.push(format!(
            "{left} files inside belong to someone else and weren't changed; their owners can share them."
        ));
    }
    Ok(lines)
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

// ---------------------------------------------------------------------------------------------
// The agent's file: limits and reset
// ---------------------------------------------------------------------------------------------

fn agent_file(a: &Account) -> Result<(PathBuf, AgentFile), String> {
    let p = PathBuf::from(&a.home).join(AGENT_FILE);
    match std::fs::symlink_metadata(&p) {
        Err(_) => Ok((p, AgentFile::default())),
        Ok(m) if m.uid() == a.uid => Err(format!(
            "{} belongs to the agent \"{}\" itself, so ctx doesn't believe it. Run: ctx reset agent \"{}\"",
            p.display(),
            a.name,
            a.name
        )),
        Ok(_) => {
            let t = std::fs::read_to_string(&p).map_err(|e| format!("couldn't read {}: {e}", p.display()))?;
            Ok((p.clone(), AgentFile::parse(&t).map_err(|e| format!("{} has a mistake: {e}", p.display()))?))
        }
    }
}

fn write_agent_file(a: &Account, p: &Path, f: &AgentFile) -> Result<(), String> {
    if me().uid == a.uid {
        return Err("An agent can't change its own ctx-agent.toml; its administrators can.".into());
    }
    std::fs::write(p, f.to_text())
        .map_err(|e| format!("couldn't write {}: {e}. Only the agent's administrators (and you) can.", p.display()))
}

fn set_limit(name: &str, limit: LimitSpec) -> Lines {
    let a = agent(name)?;
    let (p, mut f) = agent_file(&a)?;
    let said = match limit {
        LimitSpec::Nothing => {
            f.limits = Default::default();
            "no limits".to_string()
        }
        LimitSpec::LowPriority => {
            f.limits.low_priority = true;
            "low priority".into()
        }
        LimitSpec::NormalPriority => {
            f.limits.low_priority = false;
            "normal priority".into()
        }
        LimitSpec::Processes(n) => {
            f.limits.processes = n;
            format!("at most {n} processes")
        }
        LimitSpec::MemoryMb(m) => {
            f.limits.memory_mb = m;
            format!("at most {} of memory across its tabs", mb(m))
        }
    };
    write_agent_file(&a, &p, &f)?;
    Ok(vec![format!("The agent \"{name}\" has {said}. New tabs start with it; open tabs keep what they started with.")])
}

fn mb(m: u64) -> String {
    if m.is_multiple_of(1024) { format!("{} GB", m / 1024) } else { format!("{m} MB") }
}

fn reset(name: &str) -> Lines {
    let a = agent(name)?;
    let p = PathBuf::from(&a.home).join(AGENT_FILE);
    if me().uid == a.uid {
        return Err("An agent can't reset its own ctx-agent.toml; its administrators can.".into());
    }
    if std::fs::symlink_metadata(&p).is_ok() {
        std::fs::remove_file(&p).map_err(|e| format!("couldn't replace {}: {e}", p.display()))?;
    }
    write_agent_file(&a, &p, &AgentFile::default())?;
    Ok(vec![format!("{} is yours again, with the defaults.", p.display())])
}

// ---------------------------------------------------------------------------------------------
// Showing
// ---------------------------------------------------------------------------------------------

#[derive(Serialize)]
struct Holder {
    /// An agent's name, `you`, another account's name, or a group (`every agent`).
    who: String,
    access: String,
}

#[derive(Serialize)]
pub struct AgentShown {
    name: String,
    full_name: String,
    uid: u32,
    home: String,
    /// Who may read or write its home, from the home's access list.
    home_access: Vec<Holder>,
    /// What it was granted with ctx, each checked against the OS now.
    grants: Vec<GrantShown>,
    limits: Option<ctx_protocol::helper::Limits>,
    /// The spaces it's in.
    spaces: Vec<String>,
    colour: Option<String>,
    /// Problems: a ctx-agent.toml the agent replaced, a grant that no longer holds.
    warnings: Vec<String>,
    tabs: Vec<u64>,
}

#[derive(Serialize)]
struct GrantShown {
    path: String,
    write: bool,
    holds: bool,
}

fn holder_name(w: Who) -> String {
    let mine = me().uid;
    match w {
        Who::User(u) if u == mine => "you".into(),
        Who::User(u) => accounts::by_uid(u).map(|a| a.name).unwrap_or_else(|| format!("uid {u}")),
        Who::Group(g) => match accounts::group(accounts::AGENTS_GROUP) {
            Some((gid, _)) if gid == g => "every agent".into(),
            _ => format!("group {g}"),
        },
    }
}

fn access_word(a: Access) -> &'static str {
    match a {
        Access::Read => "read",
        Access::Write => "read and write",
        Access::ReadOnly => "read only",
        Access::Nothing => "nothing",
    }
}

pub fn shown(a: &Account) -> AgentShown {
    let mut warnings: Vec<String> = open_home().into_iter().collect();
    let home_access = acl::entries(Path::new(&a.home))
        .map(|es| {
            es.into_iter().map(|(w, acc)| Holder { who: holder_name(w), access: access_word(acc).into() }).collect()
        })
        .unwrap_or_default();
    let (limits, colour) = match agent_file(a) {
        Ok((_, f)) => (Some(f.limits), Some(f.colour).filter(|c| !c.is_empty())),
        Err(e) => {
            warnings.push(e);
            (None, None)
        }
    };
    let key = grantee_key(&Grantee::Agent(a.name.clone()));
    let grants = Notes::load()
        .grants
        .into_iter()
        .filter(|n| n.to == key)
        .map(|n| {
            let now = acl::access_of(Path::new(&n.path), Who::User(a.uid)).ok().flatten();
            let holds = matches!(
                (n.write, now),
                (true, Some(Access::Write)) | (false, Some(Access::Read | Access::ReadOnly | Access::Write))
            );
            if !holds {
                warnings.push(format!("The grant on {} no longer holds (changed outside ctx).", n.path));
            }
            GrantShown { path: n.path, write: n.write, holds }
        })
        .collect();
    let tabs = tabs_of(&a.name);
    AgentShown {
        name: a.name.clone(),
        full_name: a.full_name.clone(),
        uid: a.uid,
        home: a.home.clone(),
        home_access,
        grants,
        limits,
        spaces: accounts::spaces_of(&a.name).into_iter().map(|s| s.name).collect(),
        colour,
        warnings,
        tabs,
    }
}

/// One line for `ctx list agents`.
pub fn summary(s: &AgentShown) -> String {
    let full =
        if s.full_name.is_empty() || s.full_name == s.name { String::new() } else { format!(" ({})", s.full_name) };
    let admins: Vec<&str> =
        s.home_access.iter().filter(|h| h.access.contains("write") && h.who != "you").map(|h| h.who.as_str()).collect();
    let mut line = format!("{}{full}: home {}", s.name, s.home);
    if !admins.is_empty() {
        line.push_str(&format!(", administered by {}", admins.join(", ")));
    }
    match s.tabs.len() {
        0 => {}
        1 => line.push_str(", 1 tab"),
        n => line.push_str(&format!(", {n} tabs")),
    }
    if !s.warnings.is_empty() {
        line.push_str(&format!(" ({} warnings: ctx show agent {})", s.warnings.len(), quote(&s.name)));
    }
    line
}

fn tabs_of(name: &str) -> Vec<u64> {
    let Ok(mut c) = Connection::open(ClientKind::Cli) else { return Vec::new() };
    let env = Envelope { sentence: "ctx list tabs".into(), request: Request::ListTabs };
    match c.request(&env) {
        Ok(Response::Ok(Output::Tabs(list))) => list
            .windows
            .iter()
            .flat_map(|w| w.tabs.iter())
            .filter(|t| t.owner.as_deref() == Some(name))
            .map(|t| t.number)
            .collect(),
        _ => Vec::new(),
    }
}

fn show(name: &str, format: Format) -> Out {
    let mut out = Out::default();
    let a = match agent(name) {
        Ok(a) => a,
        Err(e) => {
            out.err(e);
            return out.code(1);
        }
    };
    let s = shown(&a);
    match format {
        Format::Json => out.line(serde_json::to_string_pretty(&s).unwrap_or_default()),
        Format::Toon => out.line(ctx_protocol::toon::encode(&serde_json::to_value(&s).unwrap_or_default())),
        Format::Text => {
            let full = if s.full_name.is_empty() || s.full_name == s.name {
                String::new()
            } else {
                format!(" ({})", s.full_name)
            };
            out.line(format!("Agent \"{}\"{full}: account {}, uid {}, home {}", s.name, s.name, s.uid, s.home));
            if s.home_access.is_empty() {
                out.line("  Nobody else may use its home.");
            }
            for h in &s.home_access {
                out.line(format!(
                    "  {} may {} its home{}",
                    h.who,
                    h.access,
                    if h.access.contains("write") { " (administers it)" } else { "" }
                ));
            }
            for g in &s.grants {
                out.line(format!(
                    "  may {} {}{}",
                    if g.write { "read and write" } else { "read" },
                    g.path,
                    if g.holds { "" } else { " (no longer holds)" }
                ));
            }
            if !s.spaces.is_empty() {
                out.line(format!("  In the spaces: {}", s.spaces.join(", ")));
            }
            if let Some(l) = &s.limits {
                let mut parts = Vec::new();
                if l.memory_mb > 0 {
                    parts.push(format!("{} of memory", mb(l.memory_mb)));
                }
                if l.processes > 0 {
                    parts.push(format!("{} processes", l.processes));
                }
                if l.low_priority {
                    parts.push("low priority".into());
                }
                out.line(format!("  Limits: {}", if parts.is_empty() { "none".into() } else { parts.join(", ") }));
            }
            if !s.tabs.is_empty() {
                out.line(format!("  Tabs: {}", s.tabs.iter().map(|t| t.to_string()).collect::<Vec<_>>().join(", ")));
            }
            for w in &s.warnings {
                out.line(format!("  Warning: {w}"));
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Switching
// ---------------------------------------------------------------------------------------------

fn switch(name: &str, canonical: String, interactive: bool) -> Out {
    let mut out = Out::default();
    if let Err(e) = agent(name) {
        out.err(e);
        return out.code(1);
    }
    let mut conn = match Connection::open(ClientKind::Cli) {
        Ok(c) => c,
        Err(client::NoServer(m)) => {
            out.err(m);
            return out.code(2);
        }
    };
    let req = Request::OpenTab { window: None, folder: None, name: None, agent: Some(name.into()), like: None };
    let opened = match conn.request(&Envelope { sentence: canonical.clone(), request: req }) {
        Ok(Response::Ok(Output::Opened(o))) => o,
        Ok(Response::Error(e)) => {
            out.err(e.message);
            return out.code(1);
        }
        Ok(other) => {
            out.err(format!("The server answered something unexpected: {other:?}"));
            return out.code(1);
        }
        Err(e) => {
            out.err(format!("Lost the connection to the ctx server: {e}"));
            return out.code(2);
        }
    };
    let n = opened.tab.unwrap_or(0);
    // In a ctx tab, CTX.app shows the new tab beside this one. Anywhere else, this terminal
    // becomes the agent's tab until its shell exits.
    if std::env::var_os("CTX_TAB").is_some() || !interactive || !std::io::stdout().is_terminal() {
        out.line(format!("Opened tab {n} as the agent \"{name}\", in {}.", opened.folder));
        return out;
    }
    drop(conn);
    match Connection::open(ClientKind::Cli) {
        Ok(c) => out.code(crate::attach::attach(c, canonical, TabRef::Number(n))),
        Err(client::NoServer(m)) => {
            out.err(m);
            out.code(2)
        }
    }
}

fn confirm(question: &str) -> bool {
    eprint!("{question} [y/N] ");
    let _ = std::io::stderr().flush();
    let mut answer = String::new();
    let _ = std::io::stdin().read_line(&mut answer);
    answer.trim().eq_ignore_ascii_case("y")
}

// ---------------------------------------------------------------------------------------------
// Spaces
// ---------------------------------------------------------------------------------------------

fn make_space(name: &str) -> Lines {
    only_you("make spaces")?;
    let mut lines = vec![helper(HelperRequest::MakeSpace { name: name.into() })?];
    // ~/Agents/Spaces shows them all.
    lines.extend(link_in_agents("Spaces", &accounts::spaces_dir()));
    Ok(lines)
}

fn remove_space(name: &str, with_files: bool, interactive: bool) -> Lines {
    only_you("remove spaces")?;
    let s =
        accounts::space(name).ok_or_else(|| format!("There's no space \"{name}\". `ctx list spaces` shows them."))?;
    if interactive && std::io::stdin().is_terminal() {
        let what = if with_files {
            format!("the space \"{name}\", and move its folder {} to your Trash", s.folder)
        } else {
            format!("the space \"{name}\" (its folder {} stays, yours only)", s.folder)
        };
        if !confirm(&format!("Delete {what}?")) {
            return Ok(vec!["Kept it.".into()]);
        }
    }
    Ok(vec![helper(HelperRequest::RemoveSpace { name: name.into(), trash_folder: with_files })?])
}

fn space_member(who: &str, space: &str, member: bool) -> Lines {
    only_you("change who is in a space")?;
    agent(who)?;
    Ok(vec![helper(HelperRequest::SetSpaceMember { space: space.into(), agent: who.into(), member })?])
}

fn list_spaces(format: Format) -> Out {
    let mut out = Out::default();
    let spaces = accounts::spaces();
    match format {
        Format::Json => out.line(serde_json::to_string_pretty(&spaces).unwrap_or_default()),
        Format::Toon => out.line(ctx_protocol::toon::encode(&serde_json::to_value(&spaces).unwrap_or_default())),
        Format::Text if spaces.is_empty() => out.line("No spaces. `ctx make space named \"<name>\"` makes one."),
        Format::Text => {
            for s in &spaces {
                let who = if s.members.is_empty() { "no agents yet".to_string() } else { s.members.join(", ") };
                out.line(format!("{}: {} ({who})", s.name, s.folder));
            }
        }
    }
    out
}
