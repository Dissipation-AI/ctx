//! `ctx install hooks for …`, `ctx remove hooks for …`: ctx's plugin for an agent, installed
//! through the agent's own plugin registry (`claude plugin`, `agy plugin`, `codex plugin`,
//! `pi install`, `opencode plugin`; for Cursor, which has none, its `hooks.json`). It never touches
//! the agent's instruction files; the hook is a transparent shim between the agent and its shell.
//!
//! The plugins in `integrations/` are built in, written to `<data>/integrations` (with this `ctx`'s
//! own path in place of the bare `ctx`, so hooks work where `ctx` isn't on the agent's PATH), and
//! registered from there. Without `at`, the plugin is installed for you; with `at <folder>`, for
//! that project only.

use crate::client;
use crate::exec::{Out, message};
use ctx_cli::parse::Format;
use serde_json::{Value, json};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use ctx_cli::parse::HOOK_AGENTS as AGENTS;

macro_rules! files {
    ($($rel:literal),* $(,)?) => { &[$(($rel, include_str!(concat!("../../integrations/", $rel)))),*] };
}

const FILES: &[(&str, &str)] = files![
    ".claude-plugin/marketplace.json",
    "claude/.claude-plugin/plugin.json",
    "claude/hooks/hooks.json",
    "ctx-skills/.claude-plugin/plugin.json",
    "ctx-skills/plugin.json",
    "ctx-skills/.codex-plugin/plugin.json",
    "ctx-skills/package.json",
    "ctx-skills/skills/using-ctx/SKILL.md",
    "ctx-skills/skills/using-ctx/reference/troubleshooting.md",
    "ctx-skills/skills/using-ctx/reference/auto-mode.md",
    "ctx-skills/skills/using-ctx/reference/layout-and-records.md",
    "ctx-skills/skills/interactive-programs/SKILL.md",
    "ctx-skills/skills/interactive-programs/reference/python.md",
    "ctx-skills/skills/interactive-programs/reference/native-debuggers.md",
    "ctx-skills/skills/interactive-programs/reference/ssh.md",
    "ctx-skills/skills/interactive-programs/reference/adb.md",
    "ctx-skills/skills/interactive-programs/reference/databases-and-repls.md",
    "ctx-skills/skills/interactive-programs/reference/tabs-and-jobs.md",
    "antigravity/plugin.json",
    "antigravity/hooks.json",
    ".agents/plugins/marketplace.json",
    "codex/.codex-plugin/plugin.json",
    "codex/hooks/hooks.json",
    "cursor/hooks.json",
    "pi/package.json",
    "pi/ctx.ts",
    "opencode/package.json",
    "opencode/index.js",
];

/// The file with `ctx` (as a command) replaced by this binary's path.
fn with_ctx_path(rel: &str, text: &str, ctx: &str) -> String {
    // A JSON string's contents, without the surrounding quotes.
    let inner = |s: &str| {
        let quoted = Value::String(s.into()).to_string();
        quoted[1..quoted.len() - 1].to_string()
    };
    if rel.ends_with(".json") {
        text.replace("\"command\": \"ctx\"", &format!("\"command\": \"{}\"", inner(ctx))).replace(
            "\"command\": \"ctx hook ",
            &format!("\"command\": \"{} hook ", inner(&crate::hook::shell_quote(ctx))),
        )
    } else {
        text.replace("const CTX = \"ctx\";", &format!("const CTX = {};", Value::String(ctx.into())))
    }
}

/// Write the built-in plugins to `<data>/integrations`, returning that folder.
fn stage() -> Result<PathBuf, String> {
    let root = client::paths().data.join("integrations");
    let ctx = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| "ctx".into());
    for (rel, text) in FILES {
        let path = root.join(rel);
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("Couldn't create {}: {e}", d.display()))?;
        }
        std::fs::write(&path, with_ctx_path(rel, text, &ctx))
            .map_err(|e| format!("Couldn't write {}: {e}", path.display()))?;
    }
    Ok(root)
}

fn staged_root() -> PathBuf {
    client::paths().data.join("integrations")
}

/// Run an agent's CLI, returning its output, or what went wrong.
fn run(program: &str, args: &[&str], folder: Option<&str>) -> Result<String, String> {
    let mut cmd = Command::new(program);
    cmd.args(args);
    if let Some(f) = folder {
        cmd.current_dir(f);
    }
    match cmd.output() {
        Ok(o) if o.status.success() => Ok(String::from_utf8_lossy(&o.stdout).trim().to_string()),
        Ok(o) => {
            let mut text = String::from_utf8_lossy(&o.stderr).trim().to_string();
            if text.is_empty() {
                text = String::from_utf8_lossy(&o.stdout).trim().to_string();
            }
            Err(format!("`{program} {}` failed: {text}", args.join(" ")))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(format!("`{program}` isn't on your PATH, so ctx can't reach its plugin registry."))
        }
        Err(e) => Err(format!("Couldn't run `{program}`: {e}")),
    }
}

fn parse_object(existing: &str) -> Result<Value, String> {
    let v: Value = if existing.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(existing).map_err(|e| format!("it isn't plain JSON ({e})"))?
    };
    if v.is_object() { Ok(v) } else { Err("it isn't a JSON object".into()) }
}

/// A project's `.agents/plugins.json` with `plugin` (one of the staged plugins: `antigravity`, the
/// hook, or `ctx-skills`) added to or removed from ctx's entry, which goes once it lists none.
fn plugins_json(existing: &str, staged: &Path, plugin: &str, add: bool) -> Result<String, String> {
    let mut config = parse_object(existing)?;
    let path = staged.to_string_lossy();
    let entries = config
        .as_object_mut()
        .and_then(|o| o.entry("entries").or_insert_with(|| json!([])).as_array_mut())
        .ok_or("its `entries` isn't a list")?;
    let ours = |e: &Value| e.get("path").and_then(Value::as_str) == Some(&path);
    let mut names: Vec<String> = entries
        .iter()
        .filter(|e| ours(e))
        .flat_map(|e| e["include_only"].as_array().into_iter().flatten())
        .filter_map(|n| n.as_str().filter(|n| *n != plugin).map(String::from))
        .collect();
    entries.retain(|e| !ours(e));
    if add {
        names.push(plugin.into());
    }
    names.dedup();
    if !names.is_empty() {
        entries.push(json!({"path": path, "include_only": names}));
    }
    Ok(serde_json::to_string_pretty(&config).unwrap_or_default() + "\n")
}

/// A `hooks.json` (Cursor's, or Codex's project one) with ctx's entries from `ours` added or
/// removed, leaving every other hook alone. Ours are the entries that run `hook <agent>`.
fn hooks_json(existing: &str, ours: &Value, agent: &str, add: bool) -> Result<String, String> {
    let mut config = parse_object(existing)?;
    let marker = format!("hook {agent}");
    let is_ours = |e: &Value| e.to_string().contains(&marker);
    if let Some(version) = ours.get("version")
        && config.get("version").is_none()
    {
        config["version"] = version.clone();
    }
    let hooks = config
        .as_object_mut()
        .and_then(|o| o.entry("hooks").or_insert_with(|| json!({})).as_object_mut())
        .ok_or("its `hooks` isn't an object")?;
    for (event, entries) in ours["hooks"].as_object().into_iter().flatten() {
        let list = hooks.entry(event.clone()).or_insert_with(|| json!([]));
        let list = list.as_array_mut().ok_or(format!("its `hooks.{event}` isn't a list"))?;
        list.retain(|e| !is_ours(e));
        if add {
            list.extend(entries.as_array().into_iter().flatten().cloned());
        }
        if list.is_empty() {
            hooks.remove(event);
        }
    }
    Ok(serde_json::to_string_pretty(&config).unwrap_or_default() + "\n")
}

/// Rewrite a JSON config file with `edit`, creating it (and its folder) if need be.
fn edit_json(path: &Path, edit: impl FnOnce(&str) -> Result<String, String>) -> Result<(), String> {
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let updated = edit(&existing).map_err(|e| format!("Couldn't edit {}: {e}", path.display()))?;
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("Couldn't create {}: {e}", d.display()))?;
    }
    std::fs::write(path, updated).map_err(|e| format!("Couldn't write {}: {e}", path.display()))
}

fn home() -> PathBuf {
    PathBuf::from(ctx_server::sys::home())
}

fn staged_hooks(root: &Path, rel: &str) -> Result<Value, String> {
    let path = root.join(rel);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("Couldn't read {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("Couldn't read {}: {e}", path.display()))
}

/// Where Cursor or Codex keep hooks for you, or for a project.
fn hooks_file(agent: &str, folder: Option<&str>) -> PathBuf {
    let base = folder.map(PathBuf::from).unwrap_or_else(home);
    match agent {
        "cursor" => base.join(".cursor/hooks.json"),
        _ => base.join(".codex/hooks.json"),
    }
}

/// opencode's own config folder, `~/.config/opencode`.
fn opencode_dir() -> PathBuf {
    let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".config"));
    config.join("opencode")
}

/// opencode's config files, for you (`~/.config/opencode`) or a project.
fn opencode_configs(folder: Option<&str>) -> Vec<PathBuf> {
    let dirs = match folder {
        Some(f) => vec![PathBuf::from(f), PathBuf::from(f).join(".opencode")],
        None => vec![opencode_dir()],
    };
    dirs.iter()
        .flat_map(|d| ["opencode.json", "opencode.jsonc", "config.json"].map(|f| d.join(f)))
        .filter(|p| p.exists())
        .collect()
}

/// An opencode config without ctx's plugin entry, or `None` if it has none.
fn without_opencode_plugin(existing: &str, staged: &Path) -> Result<Option<String>, String> {
    let mut config = parse_object(existing)?;
    let path = staged.to_string_lossy().into_owned();
    let Some(list) = config.get_mut("plugin").and_then(Value::as_array_mut) else { return Ok(None) };
    let before = list.len();
    list.retain(|p| p.as_str().is_none_or(|s| s.trim_start_matches("file://").trim_end_matches('/') != path));
    if list.len() == before {
        return Ok(None);
    }
    Ok(Some(serde_json::to_string_pretty(&config).unwrap_or_default() + "\n"))
}

fn for_whom(folder: Option<&str>) -> String {
    folder.map(|f| format!("for {f}")).unwrap_or_else(|| "for you".into())
}

fn install_hooks(agent: &str, folder: Option<&str>) -> Result<String, String> {
    let root = stage()?;
    let marketplace = root.to_string_lossy().into_owned();
    let plugin = |name: &str| root.join(name).to_string_lossy().into_owned();
    let whom = for_whom(folder);
    match (agent, folder) {
        ("claude", _) => {
            let scope = if folder.is_some() { "local" } else { "user" };
            run("claude", &["plugin", "install", "ctx", "--marketplace", &marketplace, "--scope", scope], folder)?;
            Ok(format!("Installed ctx's Claude Code plugin {whom}. New sessions run Bash in ctx tabs."))
        }
        ("antigravity", None) => {
            run("agy", &["plugin", "install", &plugin("antigravity")], None)?;
            Ok("Installed ctx's Antigravity plugin for you. New conversations run commands in ctx tabs.".into())
        }
        ("antigravity", Some(f)) => {
            let path = Path::new(f).join(".agents/plugins.json");
            edit_json(&path, |e| plugins_json(e, &root, "antigravity", true))?;
            Ok(format!(
                "Registered ctx's Antigravity plugin in {}. New conversations there run commands in ctx tabs.",
                path.display()
            ))
        }
        ("codex", None) => {
            codex_add(&marketplace, "ctx@ctx")?;
            Ok("Installed ctx's Codex plugin for you. Trust its hook once with /hooks; then new sessions run shell commands in ctx tabs.".into())
        }
        ("codex" | "cursor", _) => {
            let ours =
                staged_hooks(&root, if agent == "cursor" { "cursor/hooks.json" } else { "codex/hooks/hooks.json" })?;
            let path = hooks_file(agent, folder);
            edit_json(&path, |e| hooks_json(e, &ours, agent, true))?;
            let trust = if agent == "codex" { " Trust it once with /hooks;" } else { "" };
            Ok(format!("Added ctx's hook to {}.{trust} New sessions run shell commands in ctx tabs.", path.display()))
        }
        ("pi", _) => {
            let p = plugin("pi");
            let mut args = vec!["install", p.as_str()];
            if folder.is_some() {
                args.push("-l");
            }
            run("pi", &args, folder)?;
            Ok(format!("Installed ctx's pi package {whom}. New sessions run bash in ctx tabs."))
        }
        ("opencode", _) => {
            let p = plugin("opencode");
            let mut args = vec!["plugin", &p, "--force"];
            if folder.is_none() {
                args.push("--global");
            }
            run("opencode", &args, folder)?;
            Ok(format!("Installed ctx's opencode plugin {whom}. New sessions run bash in ctx tabs."))
        }
        _ => Err(format!("ctx has hooks for {}, not {agent}.", AGENTS.join(", "))),
    }
}

fn remove_hooks(agent: &str, folder: Option<&str>) -> Result<String, String> {
    let root = staged_root();
    let plugin = |name: &str| root.join(name).to_string_lossy().into_owned();
    match (agent, folder) {
        ("claude", _) => {
            let scope = if folder.is_some() { "local" } else { "user" };
            run("claude", &["plugin", "uninstall", "ctx@ctx", "--scope", scope], folder)?;
            Ok("Removed ctx's Claude Code plugin. New sessions use Claude's own shell.".into())
        }
        ("antigravity", None) => {
            run("agy", &["plugin", "uninstall", "ctx"], None)?;
            Ok("Removed ctx's Antigravity plugin. New conversations use Antigravity's own shell.".into())
        }
        ("antigravity", Some(f)) => {
            let path = Path::new(f).join(".agents/plugins.json");
            edit_json(&path, |e| plugins_json(e, &root, "antigravity", false))?;
            Ok(format!("Removed ctx's Antigravity plugin from {}.", path.display()))
        }
        ("codex", None) => {
            run("codex", &["plugin", "remove", "ctx@ctx"], None)?;
            Ok("Removed ctx's Codex plugin. New sessions use Codex's own shell.".into())
        }
        ("codex" | "cursor", _) => {
            let path = hooks_file(agent, folder);
            let ours = json!({"hooks": {if agent == "cursor" { "preToolUse" } else { "PreToolUse" }: []}});
            if !path.exists() {
                return Ok(format!("{} doesn't exist, so there's nothing to remove.", path.display()));
            }
            edit_json(&path, |e| hooks_json(e, &ours, agent, false))?;
            Ok(format!("Removed ctx's hook from {}.", path.display()))
        }
        ("pi", _) => {
            let p = plugin("pi");
            let mut args = vec!["remove", p.as_str()];
            if folder.is_some() {
                args.push("-l");
            }
            run("pi", &args, folder)?;
            Ok("Removed ctx's pi package. New sessions use pi's own bash.".into())
        }
        ("opencode", _) => {
            let staged = root.join("opencode");
            let mut removed = Vec::new();
            for path in opencode_configs(folder) {
                let existing = std::fs::read_to_string(&path).unwrap_or_default();
                let updated = without_opencode_plugin(&existing, &staged).map_err(|e| {
                    format!("Couldn't edit {}: {e}. Remove ctx's entry from its `plugin` list.", path.display())
                })?;
                if let Some(updated) = updated {
                    std::fs::write(&path, updated).map_err(|e| format!("Couldn't write {}: {e}", path.display()))?;
                    removed.push(path.display().to_string());
                }
            }
            if removed.is_empty() {
                return Ok("opencode's config has no ctx plugin, so there's nothing to remove.".into());
            }
            Ok(format!("Removed ctx's opencode plugin from {}.", removed.join(" and ")))
        }
        _ => Err(format!("ctx has hooks for {}, not {agent}.", AGENTS.join(", "))),
    }
}

/// Add the staged marketplace to Codex and install `plugin` (`name@ctx`) from it.
fn codex_add(marketplace: &str, plugin: &str) -> Result<String, String> {
    // Adding a marketplace that's already there fails; installing from it is what matters.
    let added = run("codex", &["plugin", "marketplace", "add", marketplace], None);
    run("codex", &["plugin", "add", plugin], None).map_err(|e| match added {
        Err(a) => format!("{a}\n{e}"),
        Ok(_) => e,
    })
}

/// ctx's skills: each folder under `ctx-skills/skills/`, with its files' paths inside it.
fn skills() -> Vec<(&'static str, Vec<(&'static str, &'static str)>)> {
    let mut out: Vec<(&str, Vec<(&str, &str)>)> = Vec::new();
    for (rel, text) in FILES {
        let Some((name, file)) = rel.strip_prefix("ctx-skills/skills/").and_then(|r| r.split_once('/')) else {
            continue;
        };
        match out.iter_mut().find(|(n, _)| *n == name) {
            Some((_, files)) => files.push((file, text)),
            None => out.push((name, vec![(file, text)])),
        }
    }
    out
}

/// Whether `dir` holds the skill `name` (its SKILL.md says so), so it's ours to replace or delete.
fn is_skill(dir: &Path, name: &str) -> bool {
    let text = std::fs::read_to_string(dir.join("SKILL.md")).unwrap_or_default();
    let front = text.strip_prefix("---").and_then(|t| t.split("\n---").next()).unwrap_or_default();
    front.lines().any(|l| l.strip_prefix("name:").is_some_and(|v| v.trim().trim_matches(['"', '\'']) == name))
}

/// Copy ctx's skills into an agent's skills folder, replacing earlier copies of them.
fn copy_skills(dest: &Path) -> Result<(), String> {
    for (name, files) in skills() {
        let dir = dest.join(name);
        if dir.exists() {
            if !is_skill(&dir, name) {
                return Err(format!("{} exists and isn't ctx's {name} skill, so ctx left it alone.", dir.display()));
            }
            std::fs::remove_dir_all(&dir).map_err(|e| format!("Couldn't replace {}: {e}", dir.display()))?;
        }
        for (file, text) in files {
            let path = dir.join(file);
            if let Some(d) = path.parent() {
                std::fs::create_dir_all(d).map_err(|e| format!("Couldn't create {}: {e}", d.display()))?;
            }
            std::fs::write(&path, text).map_err(|e| format!("Couldn't write {}: {e}", path.display()))?;
        }
    }
    Ok(())
}

/// Delete ctx's skills from an agent's skills folder, returning the folders deleted.
fn delete_skills(dest: &Path) -> Result<Vec<String>, String> {
    let mut deleted = Vec::new();
    for (name, _) in skills() {
        let dir = dest.join(name);
        if is_skill(&dir, name) {
            std::fs::remove_dir_all(&dir).map_err(|e| format!("Couldn't delete {}: {e}", dir.display()))?;
            deleted.push(dir.display().to_string());
        }
    }
    Ok(deleted)
}

/// The skills folder of an agent that reads `SKILL.md` folders but has no plugin registry for
/// them (or, for Codex in a project, none that works per project).
fn skills_dir(agent: &str, folder: Option<&str>) -> Option<PathBuf> {
    match (agent, folder) {
        ("codex", Some(f)) => Some(Path::new(f).join(".agents/skills")),
        ("cursor", Some(f)) => Some(Path::new(f).join(".cursor/skills")),
        ("cursor", None) => Some(home().join(".cursor/skills")),
        ("opencode", Some(f)) => Some(Path::new(f).join(".opencode/skills")),
        ("opencode", None) => Some(opencode_dir().join("skills")),
        _ => None,
    }
}

const SKILLS_LOAD: &str = "load them when a task needs them";

fn install_skills(agent: &str, folder: Option<&str>) -> Result<String, String> {
    let root = stage()?;
    let marketplace = root.to_string_lossy().into_owned();
    let plugin = root.join("ctx-skills").to_string_lossy().into_owned();
    let whom = for_whom(folder);
    if let Some(dir) = skills_dir(agent, folder) {
        copy_skills(&dir)?;
        return Ok(format!(
            "Copied ctx's skills into {}. New sessions {SKILLS_LOAD}; run this again after updating ctx to refresh them.",
            dir.display()
        ));
    }
    match (agent, folder) {
        ("claude", _) => {
            let scope = if folder.is_some() { "local" } else { "user" };
            run(
                "claude",
                &["plugin", "install", "ctx-skills", "--marketplace", &marketplace, "--scope", scope],
                folder,
            )?;
        }
        ("antigravity", None) => {
            run("agy", &["plugin", "install", &plugin], None)?;
        }
        ("antigravity", Some(f)) => {
            let path = Path::new(f).join(".agents/plugins.json");
            edit_json(&path, |e| plugins_json(e, &root, "ctx-skills", true))?;
            return Ok(format!(
                "Registered ctx's skills in {}. New conversations there {SKILLS_LOAD}.",
                path.display()
            ));
        }
        ("codex", None) => {
            codex_add(&marketplace, "ctx-skills@ctx")?;
        }
        ("pi", _) => {
            let mut args = vec!["install", plugin.as_str()];
            if folder.is_some() {
                args.push("-l");
            }
            run("pi", &args, folder)?;
        }
        _ => return Err(format!("ctx has skills for {}, not {agent}.", AGENTS.join(", "))),
    }
    Ok(format!("Installed ctx's skills (ctx-skills) {whom}. New sessions {SKILLS_LOAD}."))
}

fn remove_skills(agent: &str, folder: Option<&str>) -> Result<String, String> {
    let root = staged_root();
    let plugin = root.join("ctx-skills").to_string_lossy().into_owned();
    if let Some(dir) = skills_dir(agent, folder) {
        let deleted = delete_skills(&dir)?;
        if deleted.is_empty() {
            return Ok(format!("{} has no ctx skills, so there's nothing to remove.", dir.display()));
        }
        return Ok(format!("Deleted {}.", deleted.join(" and ")));
    }
    match (agent, folder) {
        ("claude", _) => {
            let scope = if folder.is_some() { "local" } else { "user" };
            run("claude", &["plugin", "uninstall", "ctx-skills@ctx", "--scope", scope], folder)?;
        }
        ("antigravity", None) => {
            run("agy", &["plugin", "uninstall", "ctx-skills"], None)?;
        }
        ("antigravity", Some(f)) => {
            let path = Path::new(f).join(".agents/plugins.json");
            edit_json(&path, |e| plugins_json(e, &root, "ctx-skills", false))?;
            return Ok(format!("Removed ctx's skills from {}.", path.display()));
        }
        ("codex", None) => {
            run("codex", &["plugin", "remove", "ctx-skills@ctx"], None)?;
        }
        ("pi", _) => {
            let mut args = vec!["remove", plugin.as_str()];
            if folder.is_some() {
                args.push("-l");
            }
            run("pi", &args, folder)?;
        }
        _ => return Err(format!("ctx has skills for {}, not {agent}.", AGENTS.join(", "))),
    }
    Ok("Removed ctx's skills (ctx-skills).".into())
}

/// The `ctx … skills for <agent> [at <folder>]` sentence, for messages.
fn skills_sentence(verb: &str, agent: &str, folder: Option<&str>) -> String {
    let at = match folder {
        Some(f) if f.contains(|c: char| c.is_whitespace() || c == '"') => format!(" at {}", Value::String(f.into())),
        Some(f) => format!(" at {f}"),
        None => String::new(),
    };
    format!("ctx {verb} skills for {agent}{at}")
}

/// Install the hook, then offer ctx's skills: asked on a terminal (yes by default), and left to
/// `ctx install skills` otherwise, so a script or agent installing the hook doesn't also change
/// which skills the agent loads.
pub fn install(agent: &str, folder: Option<&str>, format: Format) -> Out {
    let hooked = match install_hooks(agent, folder) {
        Ok(text) => text,
        Err(e) => return finish(Err(e), format),
    };
    let interactive =
        matches!(format, Format::Text) && std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
    if !interactive {
        let hint = format!(
            "ctx's skills (using ctx, and running interactive programs in tabs) are separate: `{}`.",
            skills_sentence("install", agent, folder)
        );
        return finish(Ok(format!("{hooked}\n{hint}")), format);
    }
    println!("{hooked}");
    eprint!(
        "Also install ctx's skills for {agent}? They teach it to use ctx tabs and drive interactive programs (REPLs, debuggers, ssh, setup wizards) in them. [Y/n] "
    );
    let _ = std::io::stderr().flush();
    let mut answer = String::new();
    let _ = std::io::stdin().read_line(&mut answer);
    if matches!(answer.trim().to_lowercase().as_str(), "n" | "no") {
        return finish(
            Ok(format!("Skipped the skills; `{}` adds them later.", skills_sentence("install", agent, folder))),
            format,
        );
    }
    finish(install_skills(agent, folder), format)
}

pub fn remove(agent: &str, folder: Option<&str>, format: Format) -> Out {
    finish(remove_hooks(agent, folder), format)
}

pub fn install_skills_for(agent: &str, folder: Option<&str>, format: Format) -> Out {
    finish(install_skills(agent, folder), format)
}

pub fn remove_skills_for(agent: &str, folder: Option<&str>, format: Format) -> Out {
    finish(remove_skills(agent, folder), format)
}

fn finish(result: Result<String, String>, format: Format) -> Out {
    let mut out = Out::default();
    match result {
        Ok(text) => {
            message(&mut out, &text, format);
            out
        }
        Err(e) => {
            out.err(e);
            out.code(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn staged(rel: &str) -> String {
        let text = FILES.iter().find(|(r, _)| *r == rel).unwrap().1;
        with_ctx_path(rel, text, "/a b/ctx")
    }

    fn staged_json(rel: &str) -> Value {
        serde_json::from_str(&staged(rel)).unwrap()
    }

    #[test]
    fn every_skill_file_is_staged() {
        // The staged marketplace lists ctx-skills, so a file left out of FILES would be missing there.
        let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../integrations"));
        let mut dirs = vec![root.join("ctx-skills")];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.file_name() != Some("README.md".as_ref()) {
                    let rel = path.strip_prefix(root).unwrap().to_string_lossy().into_owned();
                    assert!(FILES.iter().any(|(r, _)| *r == rel), "{rel} isn't in FILES");
                }
            }
        }
    }

    #[test]
    fn staged_plugins_name_this_ctx() {
        assert_eq!(
            staged_json("antigravity/hooks.json")["ctx"]["PreToolUse"][0]["hooks"][0]["command"],
            "'/a b/ctx' hook antigravity"
        );
        assert_eq!(staged_json("claude/hooks/hooks.json")["hooks"]["PreToolUse"][0]["hooks"][0]["command"], "/a b/ctx");
        assert_eq!(
            staged_json("codex/hooks/hooks.json")["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
            "'/a b/ctx' hook codex"
        );
        assert_eq!(staged_json("cursor/hooks.json")["hooks"]["preToolUse"][0]["command"], "'/a b/ctx' hook cursor");
        assert!(staged("pi/ctx.ts").contains("const CTX = \"/a b/ctx\";"));
        assert!(staged("opencode/index.js").contains("const CTX = \"/a b/ctx\";"));
    }

    #[test]
    fn plugins_json_entry_is_added_once_and_removed_without_touching_the_rest() {
        let staged = Path::new("/data/integrations");
        let theirs = r#"{"entries": [{"path": "tools/plugins"}], "inherits": []}"#;
        let added = plugins_json(theirs, staged, "antigravity", true).unwrap();
        assert_eq!(added, plugins_json(&added, staged, "antigravity", true).unwrap());
        let both = plugins_json(&added, staged, "ctx-skills", true).unwrap();
        let v: Value = serde_json::from_str(&both).unwrap();
        assert_eq!(
            v["entries"][1],
            json!({"path": "/data/integrations", "include_only": ["antigravity", "ctx-skills"]})
        );
        let skills: Value = serde_json::from_str(&plugins_json(&both, staged, "antigravity", false).unwrap()).unwrap();
        assert_eq!(skills["entries"][1]["include_only"], json!(["ctx-skills"]));
        let removed = plugins_json(&skills.to_string(), staged, "ctx-skills", false).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&removed).unwrap(), serde_json::from_str::<Value>(theirs).unwrap());
        assert!(plugins_json("[]", staged, "antigravity", true).is_err());
    }

    #[test]
    fn skills_are_copied_and_deleted_without_touching_others() {
        let dest = std::env::temp_dir().join(format!("ctx-skills-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dest);
        std::fs::create_dir_all(dest.join("theirs")).unwrap();
        std::fs::write(dest.join("theirs/SKILL.md"), "---\nname: theirs\n---\n").unwrap();
        copy_skills(&dest).unwrap();
        copy_skills(&dest).unwrap();
        let names: Vec<_> = skills().into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["using-ctx", "interactive-programs"]);
        for name in &names {
            assert!(is_skill(&dest.join(name), name));
        }
        assert!(dest.join("interactive-programs/reference/ssh.md").exists());
        assert_eq!(delete_skills(&dest).unwrap().len(), 2);
        assert!(delete_skills(&dest).unwrap().is_empty());
        assert!(dest.join("theirs/SKILL.md").exists());
        // A folder of the same name that isn't ours is left alone.
        std::fs::create_dir_all(dest.join("using-ctx")).unwrap();
        assert!(copy_skills(&dest).is_err());
        assert!(delete_skills(&dest).unwrap().is_empty());
        std::fs::remove_dir_all(&dest).unwrap();
    }

    #[test]
    fn skills_sentence_quotes_folders_with_spaces() {
        assert_eq!(skills_sentence("install", "pi", None), "ctx install skills for pi");
        assert_eq!(skills_sentence("install", "pi", Some("~/a")), "ctx install skills for pi at ~/a");
        assert_eq!(skills_sentence("remove", "pi", Some("/a b")), "ctx remove skills for pi at \"/a b\"");
    }

    #[test]
    fn hooks_json_entry_is_added_once_and_removed_without_touching_the_rest() {
        let ours = staged_json("cursor/hooks.json");
        let theirs =
            r#"{"version": 1, "hooks": {"preToolUse": [{"command": "./audit.sh"}], "stop": [{"command": "x"}]}}"#;
        let added = hooks_json(theirs, &ours, "cursor", true).unwrap();
        assert_eq!(added, hooks_json(&added, &ours, "cursor", true).unwrap());
        let v: Value = serde_json::from_str(&added).unwrap();
        assert_eq!(v["hooks"]["preToolUse"].as_array().unwrap().len(), 2);
        let removed: Value = serde_json::from_str(&hooks_json(&added, &ours, "cursor", false).unwrap()).unwrap();
        assert_eq!(removed, serde_json::from_str::<Value>(theirs).unwrap());
        let fresh: Value = serde_json::from_str(&hooks_json("", &ours, "cursor", true).unwrap()).unwrap();
        assert_eq!(fresh["version"], 1);
        let gone: Value = serde_json::from_str(&hooks_json(&added, &ours, "cursor", false).unwrap()).unwrap();
        assert!(gone["hooks"]["preToolUse"].as_array().unwrap().iter().all(|e| e["command"] == "./audit.sh"));
    }

    #[test]
    fn opencode_entry_is_removed_by_path_or_url() {
        let staged = Path::new("/data/integrations/opencode");
        let config = r#"{"plugin": ["opencode-wakatime", "file:///data/integrations/opencode"], "model": "x"}"#;
        let out: Value = serde_json::from_str(&without_opencode_plugin(config, staged).unwrap().unwrap()).unwrap();
        assert_eq!(out, json!({"plugin": ["opencode-wakatime"], "model": "x"}));
        assert!(without_opencode_plugin(r#"{"plugin": ["a"]}"#, staged).unwrap().is_none());
    }
}
