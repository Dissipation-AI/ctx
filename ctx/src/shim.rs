//! `ctx shim <program> [arguments…]`: what an agent program's name runs in your tabs.
//!
//! The server puts a small script for each of `agent_programs` (claude, codex, agy, …) first on
//! a tab's PATH. Started in one of your tabs, the program becomes the agent for its folder
//! (`[agents.<name>]` in config.toml): the sandbox is applied to this process, then the real
//! program replaces it, so the program and everything it starts stay inside. Already inside an
//! agent's tab or sandbox, or in a folder no agent covers, the real program just runs.

use crate::exec::Out;
use ctx_protocol::agents;
use ctx_protocol::config::Config;
use std::ffi::CString;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

unsafe extern "C" {
    fn sandbox_init(profile: *const libc::c_char, flags: u64, errorbuf: *mut *mut libc::c_char) -> libc::c_int;
}

/// The program on PATH, skipping ctx's shims.
fn real_program(name: &str, shims: &Path) -> Option<PathBuf> {
    let shims = agents::real(shims);
    std::env::var_os("PATH")?.to_str()?.split(':').filter(|d| !d.is_empty()).find_map(|d| {
        if agents::real(Path::new(d)) == shims {
            return None;
        }
        let p = Path::new(d).join(name);
        let m = std::fs::metadata(&p).ok()?;
        use std::os::unix::fs::PermissionsExt;
        (m.is_file() && m.permissions().mode() & 0o111 != 0).then_some(p)
    })
}

fn exec(program: &Path, name: &str, args: &[String]) -> Out {
    use std::os::unix::process::CommandExt;
    let e = std::process::Command::new(program).arg0(name).args(args).exec();
    let mut out = Out::default();
    out.err(format!("ctx: couldn't start {}: {e}", program.display()));
    out.code(127)
}

pub fn run(args: &[String]) -> Out {
    let mut out = Out::default();
    let Some((name, rest)) = args.split_first() else {
        out.err("usage: ctx shim <program> [arguments…] (run by ctx's shims)");
        return out.code(2);
    };
    let paths = crate::client::paths();
    let Some(program) = real_program(name, &agents::shims_dir(&paths.runtime)) else {
        out.err(format!("ctx: {name} isn't installed (no {name} on PATH besides ctx's shim)."));
        return out.code(127);
    };
    // Inside an agent's tab or sandbox already: it's that agent.
    // SAFETY: getpid can't fail.
    if std::env::var_os("CTX_AGENT").is_some() || ctx_server::sys::sandboxed(unsafe { libc::getpid() }) {
        return exec(&program, name, rest);
    }
    let config = Config::load(&paths.config).unwrap_or_default();
    let home = PathBuf::from(ctx_server::sys::home());
    let folder = std::env::current_dir().unwrap_or_else(|_| home.clone());
    let Some(agent_name) = agents::agent_for_folder(&config, &folder, &home).map(str::to_string) else {
        return exec(&program, name, rest);
    };
    let Some(agent) = &agents::profile(&config, &agent_name) else { return exec(&program, name, rest) };
    if !agent.account.is_empty() {
        return in_agent_tab(&agent_name, name, rest, &folder);
    }
    // SAFETY: setenv before exec, in this single-threaded process.
    unsafe { std::env::set_var("CTX_AGENT", &agent_name) };
    if agent.sandbox {
        if let Err(e) = agents::ensure_marker(&paths.runtime, &agent_name) {
            out.err(format!("ctx: couldn't set up the agent \"{agent_name}\": {e}. {name} didn't start."));
            return out.code(126);
        }
        let places = agents::Places {
            home: home.clone(),
            agent_home: None,
            ctx_data: paths.data.clone(),
            ctx_config: paths.config.parent().map(Path::to_path_buf).unwrap_or_default(),
            ctx_runtime: paths.runtime.clone(),
            own_files: vec![],
            programs: vec![program.clone()],
        };
        let profile = agents::sandbox_profile(&agent_name, agent, &places);
        if let Err(e) = apply(&profile) {
            out.err(format!("ctx: couldn't apply the agent \"{agent_name}\"'s sandbox ({e}), so {name} didn't start."));
            return out.code(126);
        }
    }
    if std::io::stderr().is_terminal() {
        eprintln!(
            "ctx: {name} runs as the agent \"{agent_name}\"{}.",
            if agent.sandbox { ", in its sandbox" } else { "" }
        );
    }
    exec(&program, name, rest)
}

fn apply(profile: &str) -> Result<(), String> {
    let c = CString::new(profile.as_bytes()).map_err(|e| e.to_string())?;
    let mut err: *mut libc::c_char = std::ptr::null_mut();
    // SAFETY: a valid C string and an out-pointer for the error message.
    if unsafe { sandbox_init(c.as_ptr(), 0, &mut err) } == 0 {
        return Ok(());
    }
    if err.is_null() {
        return Err("unknown error".into());
    }
    // SAFETY: sandbox_init sets a C string on failure.
    Err(unsafe { std::ffi::CStr::from_ptr(err) }.to_string_lossy().into_owned())
}

/// An agent with its own account: this process can't become it, so the program runs in a new
/// tab of the agent's (started by the ctx helper as that account), shown in this terminal until
/// it exits.
fn in_agent_tab(agent: &str, name: &str, args: &[String], folder: &Path) -> Out {
    use crate::client::Connection;
    use crate::hook::shell_quote;
    use ctx_protocol::*;
    let mut out = Out::default();
    let mut conn = match Connection::open(ClientKind::Cli) {
        Ok(c) => c,
        Err(crate::client::NoServer(m)) => {
            out.err(format!("ctx: {name} here runs as the agent \"{agent}\" in a tab of its own, and {m}"));
            return out.code(126);
        }
    };
    // SAFETY: getpid can't fail.
    let tab_name = format!("{agent} {name} {}", unsafe { libc::getpid() });
    let opened = conn.request(&Envelope {
        sentence: format!("ctx open tab named {} for {agent}", shell_quote(&tab_name)),
        request: Request::OpenTab {
            window: None,
            folder: Some(folder.display().to_string()),
            name: Some(tab_name.clone()),
            agent: Some(agent.to_string()),
        },
    });
    let n = match opened {
        Ok(Response::Ok(Output::Opened(o))) => o.tab.unwrap_or_default(),
        Ok(Response::Error(e)) => {
            out.err(format!("ctx: {name} didn't start: {}", e.message));
            return out.code(126);
        }
        _ => {
            out.err(format!("ctx: {name} didn't start: the ctx server didn't open the agent's tab."));
            return out.code(126);
        }
    };
    let mut command = format!("exec {}", shell_quote(name));
    for a in args {
        command.push(' ');
        command.push_str(&shell_quote(a));
    }
    let ran = conn.request(&Envelope {
        sentence: format!("ctx run in tab {n} {} without waiting", shell_quote(&command)),
        request: Request::Run {
            tab: TabRef::Number(n),
            command,
            colors: false,
            max_lines: None,
            within_ms: None,
            wait: false,
        },
    });
    if let Ok(Response::Error(e)) = ran {
        out.err(format!("ctx: {name} didn't start in tab {n}: {}", e.message));
        return out.code(126);
    }
    let code = crate::attach::attach(conn, format!("ctx attach to tab {n}"), TabRef::Number(n));
    out.code(code)
}
