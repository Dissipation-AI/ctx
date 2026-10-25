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
    "ctx-skills/skills/using-ctx/SKILL.md",
    "ctx-skills/skills/persistent-terminals/SKILL.md",
    "ctx-skills/skills/persistent-terminals/reference/python.md",
    "ctx-skills/skills/persistent-terminals/reference/native-debuggers.md",
    "ctx-skills/skills/persistent-terminals/reference/ssh.md",
    "ctx-skills/skills/persistent-terminals/reference/adb.md",
    "ctx-skills/skills/persistent-terminals/reference/databases-and-repls.md",
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

/// A project's `.agents/plugins.json` with (or without) the entry for ctx's Antigravity plugin.
fn plugins_json(existing: &str, staged: &Path, add: bool) -> Result<String, String> {
    let mut config = parse_object(existing)?;
    let path = staged.to_string_lossy();
    let entries = config
        .as_object_mut()
        .and_then(|o| o.entry("entries").or_insert_with(|| json!([])).as_array_mut())
        .ok_or("its `entries` isn't a list")?;
    entries.retain(|e| e.get("path").and_then(Value::as_str) != Some(&path));
    if add {
        entries.push(json!({"path": path, "include_only": ["antigravity"]}));
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

/// opencode's config files, for you (`~/.config/opencode`) or a project.
fn opencode_configs(folder: Option<&str>) -> Vec<PathBuf> {
    let dirs = match folder {
        Some(f) => vec![PathBuf::from(f), PathBuf::from(f).join(".opencode")],
        None => {
            let config =
                std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".config"));
            vec![config.join("opencode")]
        }
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
            edit_json(&path, |e| plugins_json(e, &root, true))?;
            Ok(format!(
                "Registered ctx's Antigravity plugin in {}. New conversations there run commands in ctx tabs.",
                path.display()
            ))
        }
        ("codex", None) => {
            // Adding a marketplace that's already there fails; installing from it is what matters.
            let added = run("codex", &["plugin", "marketplace", "add", &marketplace], None);
            run("codex", &["plugin", "add", "ctx@ctx"], None).map_err(|e| match added {
                Err(a) => format!("{a}\n{e}"),
                Ok(_) => e,
            })?;
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
            edit_json(&path, |e| plugins_json(e, &root, false))?;
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

pub fn install(agent: &str, folder: Option<&str>, format: Format) -> Out {
    finish(install_hooks(agent, folder), format)
}

pub fn remove(agent: &str, folder: Option<&str>, format: Format) -> Out {
    finish(remove_hooks(agent, folder), format)
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
        let added = plugins_json(theirs, staged, true).unwrap();
        assert_eq!(added, plugins_json(&added, staged, true).unwrap());
        let v: Value = serde_json::from_str(&added).unwrap();
        assert_eq!(v["entries"][1], json!({"path": "/data/integrations", "include_only": ["antigravity"]}));
        let removed: Value = serde_json::from_str(&plugins_json(&added, staged, false).unwrap()).unwrap();
        assert_eq!(removed, serde_json::from_str::<Value>(theirs).unwrap());
        assert!(plugins_json("[]", staged, true).is_err());
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
