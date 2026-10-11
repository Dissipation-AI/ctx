//! `ctx make launcher`, `list launchers`, `rebuild launcher`, `remove launcher`: small signed
//! binaries that hold one macOS permission for one program in one folder.

use crate::client;
use crate::exec::{Out, message};
use ctx_cli::parse::Format;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

const TEMPLATE: &str = include_str!("../../launchers/launcher.c");
const BUILD: &str = include_str!("../../launchers/build.sh");
/// Bumped whenever the template changes; `rebuild` says when a launcher is behind.
pub const TEMPLATE_VERSION: u32 = 1;

/// Programs that would turn a launcher into a general-purpose shell.
const INTERPRETERS: &[&str] = &[
    "sh",
    "bash",
    "zsh",
    "fish",
    "dash",
    "ksh",
    "tcsh",
    "csh",
    "python",
    "python3",
    "node",
    "deno",
    "bun",
    "perl",
    "ruby",
    "php",
    "osascript",
    "env",
    "lua",
    "tclsh",
    "pwsh",
    "nu",
    "xonsh",
];

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Info {
    pub name: String,
    pub program: String,
    pub folder: String,
    pub any_arguments: bool,
    /// `ad-hoc` or the signing identity.
    pub signed_by: String,
    /// How the program is checked: its code-signing requirement, or its SHA-256.
    pub requirement: String,
    pub sha256: String,
    pub template_version: u32,
    pub built_at: String,
    pub path: String,
}

fn dir_for(name: &str) -> PathBuf {
    client::paths().launchers().join(name)
}

fn bin_dir() -> PathBuf {
    client::paths().launchers().join("bin")
}

fn c_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn check_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 40
        || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        || !name.chars().next().unwrap().is_ascii_alphabetic()
    {
        return Err(format!(
            "A launcher's name is also its command, so it uses letters, digits, - and _ and starts with a letter (\"{name}\" doesn't)."
        ));
    }
    Ok(())
}

/// The code-signing requirement to check a program against, if it's signed with a real identity.
fn signing_requirement(resolved: &Path) -> Option<String> {
    let out = Command::new("/usr/bin/codesign").args(["-d", "-r-"]).arg(resolved).output().ok()?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let line = text.lines().find(|l| l.contains("designated =>"))?;
    let req = line.split("designated =>").nth(1)?.trim().to_string();
    // Ad-hoc signatures only have a cdhash requirement: that changes with every build, so the
    // file's hash is the honest check.
    if req.starts_with("cdhash") || req.is_empty() { None } else { Some(req) }
}

fn sha256(path: &Path) -> Result<String, String> {
    let out = Command::new("/usr/bin/shasum").args(["-a", "256"]).arg(path).output().map_err(|e| e.to_string())?;
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .map(str::to_string)
        .ok_or_else(|| "couldn't hash the program".into())
}

fn ctx_path() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.canonicalize().ok())
        .map(|p| p.display().to_string())
        .unwrap_or_default()
}

/// Write the source and build script, build, sign. Returns the binary's path.
fn build(info: &mut Info) -> Result<PathBuf, String> {
    let dir = dir_for(&info.name);
    std::fs::create_dir_all(&dir).map_err(|e| format!("couldn't create {}: {e}", dir.display()))?;
    let policy = if info.any_arguments {
        "passed through from anyone who runs the launcher (anything able to run it can use its permission)."
    } else {
        "accepted only when a person runs it at an interactive terminal, not through an agent (checked with the ctx server inside ctx tabs). Agents can start it with no arguments."
    };
    let path_now = std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin:/usr/sbin:/sbin".into());
    let source = TEMPLATE
        .replace("@NAME@", &c_string(&info.name))
        .replace("@TEMPLATE_VERSION@", &TEMPLATE_VERSION.to_string())
        .replace("@ARGUMENT_POLICY_TEXT@", policy)
        .replace("@PROGRAM@", &c_string(&info.program))
        .replace("@FOLDER@", &c_string(&info.folder))
        .replace("@REQUIREMENT@", &c_string(&info.requirement))
        .replace("@SHA256@", &info.sha256)
        .replace("@ANY_ARGUMENTS@", if info.any_arguments { "1" } else { "0" })
        .replace("@PATH@", &c_string(&path_now))
        .replace("@CTX@", &c_string(&ctx_path()));
    let identifier = format!("com.ctx.launcher.{}", info.name);
    let sign_flags = if info.signed_by == "ad-hoc" {
        "--sign -".to_string()
    } else {
        format!("--sign \"{}\" --options runtime --timestamp", info.signed_by.replace('"', ""))
    };
    let script =
        BUILD.replace("@NAME@", &info.name).replace("@SIGN_FLAGS@", &sign_flags).replace("@IDENTIFIER@", &identifier);
    std::fs::write(dir.join("launcher.c"), source).map_err(|e| e.to_string())?;
    let build_sh = dir.join("build.sh");
    std::fs::write(&build_sh, script).map_err(|e| e.to_string())?;
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&build_sh, std::fs::Permissions::from_mode(0o755));
    }
    if !Path::new("/usr/bin/clang").exists()
        || Command::new("/usr/bin/xcrun")
            .args(["--find", "clang"])
            .output()
            .map(|o| !o.status.success())
            .unwrap_or(true)
    {
        return Err(
            "Building a launcher needs the Xcode Command Line Tools. Install them with: xcode-select --install".into(),
        );
    }
    let out = Command::new("/bin/sh").arg(&build_sh).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "The build failed:\n{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let bin = dir.join(&info.name);
    info.path = bin.display().to_string();
    info.built_at = ctx_protocol::time::iso_utc(ctx_protocol::time::now_ms());
    info.template_version = TEMPLATE_VERSION;
    std::fs::write(dir.join("launcher.json"), serde_json::to_string_pretty(info).unwrap_or_default())
        .map_err(|e| e.to_string())?;
    // A stable command name on PATH in ctx tabs.
    let _ = std::fs::create_dir_all(bin_dir());
    let link = bin_dir().join(&info.name);
    let _ = std::fs::remove_file(&link);
    let _ = std::os::unix::fs::symlink(&bin, &link);
    Ok(bin)
}

fn load(name: &str) -> Option<Info> {
    serde_json::from_slice(&std::fs::read(dir_for(name).join("launcher.json")).ok()?).ok()
}

fn failed(msg: impl AsRef<str>) -> Out {
    let mut o = Out::default();
    o.err(msg);
    o.ctx_error = true;
    o.code(1)
}

fn home_short(p: &str) -> String {
    let home = ctx_server::sys::home();
    match p.strip_prefix(&format!("{home}/")) {
        Some(rest) => format!("~/{rest}"),
        None => p.to_string(),
    }
}

pub fn make(name: &str, program: &str, folder: &str, any: bool, signed_by: Option<&str>, format: Format) -> Out {
    if let Err(e) = check_name(name) {
        return failed(e);
    }
    if load(name).is_some() {
        return failed(format!(
            "There's already a launcher named \"{name}\". `ctx rebuild launcher \"{name}\"` rebuilds it; `ctx remove launcher \"{name}\"` deletes it."
        ));
    }
    let Ok(resolved) = std::fs::canonicalize(program) else {
        return failed(format!("There's no program at {program}."));
    };
    let base = resolved.file_name().map(|b| b.to_string_lossy().to_lowercase()).unwrap_or_default();
    let given_base = Path::new(program).file_name().map(|b| b.to_string_lossy().to_lowercase()).unwrap_or_default();
    if INTERPRETERS
        .iter()
        .any(|i| base == *i || given_base == *i || base.starts_with(&format!("{i}3")) || base.starts_with("python"))
    {
        return failed(format!(
            "{program} is a general-purpose interpreter: a launcher for it would hand its permission to any script. Make a launcher for the specific program instead."
        ));
    }
    if !Path::new(folder).is_dir() {
        return failed(format!("There's no folder at {folder}."));
    }
    let requirement = signing_requirement(&resolved).unwrap_or_default();
    let sha = if requirement.is_empty() {
        match sha256(&resolved) {
            Ok(s) => s,
            Err(e) => return failed(e),
        }
    } else {
        String::new()
    };
    let mut info = Info {
        name: name.into(),
        program: program.into(),
        folder: folder.into(),
        any_arguments: any,
        signed_by: signed_by.unwrap_or("ad-hoc").into(),
        requirement,
        sha256: sha,
        template_version: TEMPLATE_VERSION,
        built_at: String::new(),
        path: String::new(),
    };
    let bin = match build(&mut info) {
        Ok(b) => b,
        Err(e) => return failed(e),
    };
    let mut out = Out::default();
    let signing =
        if info.signed_by == "ad-hoc" { "signed, ad-hoc".to_string() } else { format!("signed by {}", info.signed_by) };
    let check = if info.requirement.is_empty() {
        "its file's SHA-256 (a changed file won't run)"
    } else {
        "its code signature (updates by the same developer still run)"
    };
    let args = if any {
        "Arguments: passed through. Anything that can run this launcher can use its permission.".to_string()
    } else {
        "Arguments: accepted only when a person runs it at an interactive terminal, not from agents.".to_string()
    };
    let text = format!(
        "Made {} ({signing}).\nIt starts {} in {} under its own macOS identity, after checking {check}.\n{args}\n\n\
To give it Full Disk Access: System Settings → Privacy & Security → Full Disk Access → + → choose the file above.\n\
Start it in a tab with: ctx run in \"<tab>\" \"{name}\"",
        home_short(&bin.display().to_string()),
        home_short(program),
        home_short(folder),
    );
    message(&mut out, &text, format);
    if format == Format::Text
        && std::io::IsTerminal::is_terminal(&std::io::stdout())
        && !ctx_protocol::config::Paths::is_custom()
    {
        let _ = Command::new("/usr/bin/open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")
            .spawn();
        out.line("(Opening that pane now.)");
    }
    out
}

pub fn rebuild(name: &str, format: Format) -> Out {
    let Some(mut info) = load(name) else {
        return failed(format!("There's no launcher named \"{name}\". `ctx list launchers` shows them."));
    };
    // The program may have been updated: take its current signature requirement or hash.
    if let Ok(resolved) = std::fs::canonicalize(&info.program) {
        let req = signing_requirement(&resolved).unwrap_or_default();
        info.sha256 = if req.is_empty() { sha256(&resolved).unwrap_or_default() } else { String::new() };
        info.requirement = req;
    }
    match build(&mut info) {
        Ok(bin) => {
            let mut out = Out::default();
            let regrant = if info.signed_by == "ad-hoc" {
                "It's ad-hoc signed, so macOS sees a new program: grant it again in System Settings (remove the old entry first)."
            } else {
                "It's signed with a Developer ID, so its existing grant still applies."
            };
            message(
                &mut out,
                &format!(
                    "Rebuilt {} from template version {TEMPLATE_VERSION}. {regrant}",
                    home_short(&bin.display().to_string())
                ),
                format,
            );
            out
        }
        Err(e) => failed(e),
    }
}

pub fn remove(name: &str, format: Format) -> Out {
    let Some(info) = load(name) else {
        return failed(format!("There's no launcher named \"{name}\". `ctx list launchers` shows them."));
    };
    let _ = std::fs::remove_file(bin_dir().join(name));
    if let Err(e) = std::fs::remove_dir_all(dir_for(name)) {
        return failed(format!("Couldn't delete {}: {e}", dir_for(name).display()));
    }
    let mut out = Out::default();
    message(
        &mut out,
        &format!(
            "Deleted launcher \"{name}\" ({}). Also remove its entry in System Settings → Privacy & Security (Full Disk Access or wherever you granted it).",
            home_short(&info.path)
        ),
        format,
    );
    out
}

pub fn list(format: Format) -> Out {
    let mut infos: Vec<Info> = std::fs::read_dir(client::paths().launchers())
        .map(|d| d.filter_map(|e| e.ok()).filter_map(|e| load(&e.file_name().to_string_lossy())).collect())
        .unwrap_or_default();
    infos.sort_by(|a, b| a.name.cmp(&b.name));
    // Which tabs run them now (the server sees each tab's responsible process).
    let mut running: Vec<(String, u64)> = Vec::new();
    if let Ok(mut c) = client::Connection::open_quiet()
        && let Ok(ctx_protocol::Response::Ok(ctx_protocol::Output::Tabs(list))) = c.request(&ctx_protocol::Envelope {
            sentence: "ctx list launchers".into(),
            request: ctx_protocol::Request::ListTabs,
        })
    {
        for t in list.windows.iter().flat_map(|w| w.tabs.iter()) {
            if let Some(p) = &t.permissions_of {
                running.push((p.clone(), t.number));
            }
        }
    }
    let mut out = Out::default();
    match format {
        Format::Text => {
            if infos.is_empty() {
                out.line("No launchers yet. `ctx make launcher named \"<name>\" for <program> at <folder>` makes one.");
            }
            for i in &infos {
                let tabs: Vec<String> =
                    running.iter().filter(|(n, _)| *n == i.name).map(|(_, t)| format!("tab {t}")).collect();
                let now = if tabs.is_empty() { String::new() } else { format!(" Running now in {}.", tabs.join(", ")) };
                let behind = if i.template_version < TEMPLATE_VERSION {
                    " Built from an older template: ctx rebuild launcher \"name\"."
                } else {
                    ""
                };
                out.line(format!(
                    "{}: starts {} in {}; arguments {}; {}.{now}{behind}",
                    i.name,
                    home_short(&i.program),
                    home_short(&i.folder),
                    if i.any_arguments { "passed through" } else { "only from a person" },
                    if i.signed_by == "ad-hoc" {
                        "ad-hoc signed".to_string()
                    } else {
                        format!("signed by {}", i.signed_by)
                    },
                ));
            }
            if !infos.is_empty() {
                out.line("Permissions can't be read from here: check System Settings → Privacy & Security.");
            }
        }
        _ => {
            let v = ctx_protocol::with_schema(serde_json::json!({
                "launchers": infos.iter().map(|i| serde_json::json!({
                    "name": i.name, "program": i.program, "folder": i.folder,
                    "any_arguments": i.any_arguments, "signed_by": i.signed_by,
                    "path": i.path, "template_version": i.template_version,
                    "running_in_tabs": running.iter().filter(|(n, _)| *n == i.name).map(|(_, t)| *t).collect::<Vec<_>>(),
                })).collect::<Vec<_>>()
            }));
            out.line(if format == Format::Toon {
                ctx_protocol::toon::encode(&v)
            } else {
                serde_json::to_string_pretty(&v).unwrap_or_default()
            });
        }
    }
    out
}

/// `ctx launcher-check <pid>`: asked by a launcher inside a ctx tab. Prints `person` when only
/// a person typed and ran the command that started it, else `agent`.
pub fn check(pid: &str) -> Out {
    let mut out = Out::default();
    let Ok(pid) = pid.parse::<i32>() else { return out.code(1) };
    let answer = match client::Connection::open_quiet() {
        Ok(mut c) => match c.request(&ctx_protocol::Envelope {
            sentence: "ctx launcher-check".into(),
            request: ctx_protocol::Request::WhoRan { pid },
        }) {
            Ok(ctx_protocol::Response::Ok(ctx_protocol::Output::RanBy(r))) if r.tab.is_some() && !r.agent_involved => {
                "person"
            }
            _ => "agent",
        },
        Err(_) => "agent",
    };
    out.line(answer);
    out
}
