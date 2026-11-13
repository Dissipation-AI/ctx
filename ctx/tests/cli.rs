//! End-to-end tests: a real server, real zsh tabs, the `ctx` binary.

mod common;

use common::Server;

#[test]
fn run_read_wait_and_exit_statuses() {
    let s = Server::start("run");
    s.ctx(&["open", "tab", "named", "Work", "at", "/tmp"]).ok();
    let o = s.ctx(&["run", "in", "Work", "echo hello; echo oops >&2"]);
    assert_eq!(o.code, 0);
    assert_eq!(o.stdout, "hello\noops\n");
    assert_eq!(s.ctx(&["run", "in", "Work", "(exit 7)"]).code, 7);
    // Phrases in any order give the same result.
    assert_eq!(s.ctx(&["run", "echo same", "in", "Work"]).stdout, "same\n");
    // A command from standard input (a heredoc).
    let o = s.ctx_in(&["run", "in", "Work"], Some("for i in 1 2; do\n  echo line $i\ndone\n"));
    assert_eq!(o.stdout, "line 1\nline 2\n", "{}", o.stderr);
    // Long-running command, then wait for its output and for it to finish.
    s.ctx(&["run", "in", "Work", "sleep 0.5; echo Listening on 4000; sleep 0.5", "without", "waiting"]).ok();
    let o = s.ctx(&["wait", "until", "Work", "shows", "Listening on", "within", "10", "seconds"]).ok().stdout.clone();
    assert_eq!(o.trim(), "Listening on 4000");
    s.ctx(&["wait", "until", "Work", "is", "idle", "within", "10", "seconds"]).ok();
    // read last N commands carries outputs and exit statuses.
    let o = s.ctx(&["read", "last", "2", "commands", "in", "Work", "as", "json"]).ok().stdout.clone();
    let v: serde_json::Value = serde_json::from_str(&o).unwrap();
    assert_eq!(v["schema"], 1);
    assert_eq!(v["commands"][1]["output"][0], "Listening on 4000");
    assert_eq!(v["commands"][1]["exit_status"], 0);
    assert!(v["commands"][1]["ran_by"].as_str().unwrap().contains("pid"));
    // A time limit on run: the command keeps going, exit status 3.
    let o = s.ctx(&["run", "in", "Work", "sleep 2; echo late", "within", "1", "second"]);
    assert_eq!(o.code, 3, "{}", o.stderr);
    assert!(o.stderr.contains("still running in tab"));
    let o = s.ctx(&["wait", "until", "Work", "is", "idle", "within", "10", "seconds"]).ok().stdout.clone();
    assert!(o.contains("exit status: 0"), "{o}");
}

#[test]
fn refusals_say_what_to_do_instead() {
    let s = Server::start("refuse");
    s.ctx(&["open", "tab", "named", "Busy", "at", "/tmp"]).ok();
    s.ctx(&["run", "in", "Busy", "sleep 5", "without", "waiting"]).ok();
    let o = s.ctx(&["run", "in", "Busy", "echo hi"]);
    assert_eq!(o.code, 1);
    assert!(o.stderr.contains("is running \"sleep 5\""), "{}", o.stderr);
    s.ctx(&["press", "ctrl-c", "in", "Busy"]).ok();
    s.ctx(&["wait", "until", "Busy", "is", "idle", "within", "5", "seconds"]).ok();
    // Typing then Enter at an idle prompt is refused in favour of run.
    let o = s.ctx(&["type", "echo hi", "into", "Busy", "then", "press", "enter"]);
    assert!(o.stderr.contains("To run a shell command, use: ctx run in \"Busy\" \"echo hi\""), "{}", o.stderr);
    // Pre-filling is allowed, and then run refuses to mangle it.
    s.ctx(&["type", "echo half", "into", "Busy"]).ok();
    std::thread::sleep(std::time::Duration::from_millis(300));
    let o = s.ctx(&["run", "in", "Busy", "echo x"]);
    assert!(o.stderr.contains("Someone is typing"), "{}", o.stderr);
    // Unknown tabs list what exists.
    let o = s.ctx(&["read", "tab", "99"]);
    assert_eq!(o.code, 1);
    assert!(o.stderr.contains("There is no tab 99. The open tab is"), "{}", o.stderr);
    // Close asks first when something runs (no terminal here, so it refuses with the sentence).
    s.ctx(&["press", "ctrl-u", "in", "Busy"]).ok();
    s.ctx(&["run", "in", "Busy", "sleep 30", "without", "waiting"]).ok();
    let o = s.ctx(&["close", "Busy"]);
    assert!(o.stderr.contains("without asking"), "{}", o.stderr);
    s.ctx(&["close", "Busy", "without", "asking"]).ok();
    assert!(s.ctx(&["list", "tabs"]).stdout.contains("No tabs are open"));
}

#[test]
fn unparseable_sentences_get_the_closest_valid_one() {
    let s = Server::start("parse");
    let o = s.ctx(&["run", "cargo", "test", "in", "auth"]);
    assert_eq!(o.code, 1);
    assert!(o.stderr.contains("ctx run in \"auth\" \"cargo test\""), "{}", o.stderr);
    let o = s.ctx(&["lsit", "tabs", "as", "json"]);
    let v: serde_json::Value = serde_json::from_str(&o.stdout).unwrap();
    assert_eq!(v["error"], "not_a_valid_command");
    assert_eq!(v["suggestion"], "ctx list tabs as json");
}

#[test]
fn saved_tabs_survive_a_crash_and_reopen() {
    let mut s = Server::start("crash");
    s.ctx(&["open", "tab", "named", "Keep", "at", "/tmp"]).ok();
    s.ctx(&["run", "in", "Keep", "export KEEP_ME=kept; cd /usr; echo before-crash"]).ok();
    s.ctx(&["run", "in", "Keep", "sleep 600", "without", "waiting"]).ok();
    std::thread::sleep(std::time::Duration::from_millis(300));
    s.kill_hard();
    s.spawn();
    let closed = s.ctx(&["list", "closed", "tabs"]).ok().stdout.clone();
    assert!(closed.contains("tab 1 \"Keep\" at /usr") && closed.contains("(interrupted)"), "{closed}");
    // Closed tabs can be read and searched.
    assert!(s.ctx(&["read", "last", "20", "lines", "in", "tab", "1"]).ok().stdout.contains("before-crash"));
    assert!(s.ctx(&["find", "before-crash"]).ok().stdout.contains("tab 1 \"Keep\""));
    // Reopen: same number, folder and environment; the interrupted command is pre-filled.
    s.ctx(&["reopen", "tab", "1"]).ok();
    let screen = s.ctx(&["read", "tab", "1"]).ok().stdout.clone();
    assert!(screen.contains("[reopened") && screen.contains("before-crash"), "{screen}");
    assert!(screen.lines().last().unwrap_or("").contains("sleep 600"), "{screen}");
    s.ctx(&["press", "ctrl-u", "in", "tab", "1"]).ok();
    std::thread::sleep(std::time::Duration::from_millis(300));
    let o = s.ctx(&["run", "in", "tab", "1", "echo $KEEP_ME; pwd"]).ok().stdout.clone();
    assert_eq!(o, "kept\n/usr\n");
    // Numbers are never reused.
    let o = s.ctx(&["open", "tab"]).ok().stdout.clone();
    assert!(o.contains("tab 2"), "{o}");
}

#[test]
fn events_names_windows_and_toon() {
    let s = Server::start("events");
    s.ctx(&["open", "window", "named", "Work", "at", "/tmp"]).ok();
    s.ctx(&["rename", "tab", "1", "to", "First one"]).ok();
    s.ctx(&["open", "tab", "in", "window", "Work", "named", "Other"]).ok();
    s.ctx(&["move", "Other", "to", "window", "Work", "position", "1"]).ok();
    let screen = s.ctx(&["read", "First one"]).ok().stdout.clone();
    assert!(screen.contains("[named \"First one\"]"), "{screen}");
    let toon = s.ctx(&["list", "tabs", "as", "toon"]).ok().stdout.clone();
    assert!(toon.contains("tabs[2]{number,name,folder,window,opened_by"), "{toon}");
    let o = s.ctx(&["rename", "Other", "to", "tab"]);
    assert!(o.stderr.contains("sentence words"), "{}", o.stderr);
    let o = s.ctx(&["rename", "Other", "to", "First one"]);
    assert!(o.stderr.contains("already named"), "{}", o.stderr);
    let o = s.ctx(&["rename", "Other", "to", "x/y"]);
    assert!(o.stderr.contains("not \"/\""), "{}", o.stderr);
}

#[test]
fn activity_log_records_who_did_what() {
    let s = Server::start("activity");
    s.ctx(&["open", "tab", "named", "Logged", "at", "/tmp"]).ok();
    s.ctx(&["type", "secret-looking text", "into", "Logged"]).ok();
    // Typed text is logged once it shows on screen.
    screen_shows(&s, "Logged", "secret-looking text");
    let o = s.ctx(&["list", "activity", "in", "Logged", "as", "json"]).ok().stdout.clone();
    let v: serde_json::Value = serde_json::from_str(&o).unwrap();
    let entries = v["entries"].as_array().unwrap();
    let typed = entries.iter().find(|e| e["typed"] == "secret-looking text").expect("typed text logged");
    assert_eq!(typed["declared_name"], "test agent");
    assert!(typed["sentence"].as_str().unwrap().starts_with("ctx type"));
}

#[test]
fn instructions_install_update_and_remove() {
    let s = Server::start("instructions");
    let file = s.home.join(".claude/CLAUDE.md");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "# Mine\n\nKeep me.\n").unwrap();
    s.ctx(&["install", "instructions", "for", "claude"]).ok();
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.starts_with("# Mine\n\nKeep me.\n") && text.contains("<!-- ctx instructions: begin -->"));
    s.ctx(&["install", "instructions", "for", "claude"]).ok();
    assert_eq!(std::fs::read_to_string(&file).unwrap().matches("ctx instructions: begin").count(), 1);
    s.ctx(&["remove", "instructions", "for", "claude"]).ok();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "# Mine\n\nKeep me.\n");
}

#[test]
fn a_call_right_after_a_command_waits_for_the_prompt() {
    // The command-end mark comes in precmd and the prompt mark in PS1, so a slow precmd leaves the
    // tab busy with nothing running. The next call still belongs in the session tab: an unexported
    // variable is only there, not in a copy.
    let s = Server::start("mcp-gap");
    let input = [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","clientInfo":{"name":"test-client"}}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"shell","arguments":{"command":"slow() { sleep 1 }; precmd_functions+=(slow); KEPT=1"}}}"#,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"shell","arguments":{"command":"echo $KEPT"}}}"#,
    ]
    .join("\n");
    let o = s.ctx_in(&["mcp"], Some(&(input + "\n")));
    let replies: Vec<serde_json::Value> = o.stdout.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(replies.len(), 3);
    assert_eq!(replies[2]["result"]["content"][0]["text"], "1\n[exit status: 0]");
}

#[test]
fn run_right_after_a_command_waits_for_the_prompt() {
    // The same gap for `ctx run`: held until the prompt, not refused as busy.
    let s = Server::start("run-gap");
    s.ctx(&["open", "tab", "named", "Gap"]).ok();
    s.ctx(&["run", "in", "Gap", "slow() { sleep 1 }; precmd_functions+=(slow); KEPT=1"]).ok();
    let o = s.ctx(&["run", "in", "Gap", "echo $KEPT"]).ok().stdout.clone();
    assert_eq!(o.trim_end(), "1", "{o}");
}

#[test]
fn mcp_shell_runs_in_the_sessions_tab() {
    let s = Server::start("mcp");
    let input = [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","clientInfo":{"name":"test-client"}}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"shell","arguments":{"command":"export KEPT=1; echo via mcp; false"}}}"#,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"shell","arguments":{"command":"echo $KEPT"}}}"#,
        r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"shell","arguments":{"command":"ctx list tabs"}}}"#,
    ]
    .join("\n");
    let o = s.ctx_in(&["mcp"], Some(&(input + "\n")));
    let replies: Vec<serde_json::Value> = o.stdout.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(replies.len(), 4);
    assert_eq!(replies[1]["result"]["content"][0]["text"], "via mcp\n[exit status: 1]");
    assert_eq!(replies[1]["result"]["isError"], false);
    assert_eq!(replies[2]["result"]["content"][0]["text"], "1\n[exit status: 0]");
    // A ctx sentence runs as is, and sees the session's tab.
    assert!(replies[3]["result"]["content"][0]["text"].as_str().unwrap().contains("\"Test "), "{}", replies[3]);
}

#[test]
fn restart_keeps_tabs() {
    let s = Server::start("restart");
    s.ctx(&["open", "tab", "named", "Stay", "at", "/tmp"]).ok();
    // An empty variable among them must not shift the others (zsh drops empty words unquoted).
    s.ctx(&["run", "in", "Stay", "export AAA_EMPTY= STAY=1 ZZZ_EMPTY=; cd /usr"]).ok();
    s.ctx(&["restart", "server", "without", "asking"]).ok();
    let o = s.ctx(&["run", "in", "Stay", "echo $STAY; pwd"]).ok().stdout.clone();
    assert_eq!(o, "1\n/usr\n");
}

#[cfg(not(target_os = "macos"))]
#[test]
fn launchers_are_macos_only() {
    let s = Server::start("launcher");
    let o = s.ctx(&["make", "launcher", "named", "echoer", "for", "/bin/echo", "at", "/tmp"]);
    assert!(o.code != 0 && o.stderr.contains("macOS"), "{}", o.stderr);
}

#[cfg(target_os = "macos")]
#[test]
fn launchers_are_built_and_refuse_agents_arguments() {
    if !std::path::Path::new("/usr/bin/clang").exists() {
        return;
    }
    let s = Server::start("launcher");
    s.ctx(&["make", "launcher", "named", "echoer", "for", "/bin/echo", "at", "/tmp"]).ok();
    assert!(
        s.ctx(&["make", "launcher", "named", "shell", "for", "/bin/zsh", "at", "/tmp"])
            .stderr
            .contains("general-purpose")
    );
    s.ctx(&["open", "tab", "named", "L", "at", "/tmp"]).ok();
    let o = s.ctx(&["run", "in", "L", "echoer from an agent"]);
    assert_eq!(o.code, 2, "{} {}", o.stdout, o.stderr);
    assert!(o.stdout.contains("arguments are accepted only when a person runs this launcher"), "{}", o.stdout);
    assert_eq!(s.ctx(&["run", "in", "L", "echoer"]).code, 0);
    s.ctx(&["remove", "launcher", "echoer"]).ok();
}

/// A user-level sshd on a free port with throwaway keys; `None` if it can't run here.
fn local_sshd(dir: &std::path::Path) -> Option<(std::process::Child, std::path::PathBuf)> {
    use std::process::{Command, Stdio};
    if !std::path::Path::new("/usr/sbin/sshd").exists() {
        return None;
    }
    let d = dir.join("sshd");
    std::fs::create_dir_all(&d).ok()?;
    for key in ["host_key", "user_key"] {
        let ok = Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(d.join(key))
            .status()
            .ok()?
            .success();
        if !ok {
            return None;
        }
    }
    std::fs::copy(d.join("user_key.pub"), d.join("authorized_keys")).ok()?;
    let port = 20000 + (std::process::id() % 20000) as u16;
    let cfg = format!(
        "Port {port}\nListenAddress 127.0.0.1\nHostKey {d}/host_key\nAuthorizedKeysFile {d}/authorized_keys\nPidFile {d}/sshd.pid\nStrictModes no\nUsePAM no\nPasswordAuthentication no\nKbdInteractiveAuthentication no\n",
        d = d.display()
    );
    std::fs::write(d.join("sshd_config"), cfg).ok()?;
    let client = format!(
        "Host testhost\n  HostName 127.0.0.1\n  Port {port}\n  IdentityFile {d}/user_key\n  StrictHostKeyChecking no\n  UserKnownHostsFile /dev/null\n  LogLevel ERROR\n",
        d = d.display()
    );
    std::fs::write(d.join("ssh_config"), client).ok()?;
    let child = Command::new("/usr/sbin/sshd")
        .args(["-D", "-e", "-f"])
        .arg(d.join("sshd_config"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    std::thread::sleep(std::time::Duration::from_millis(800));
    let probe = Command::new("ssh")
        .arg("-F")
        .arg(d.join("ssh_config"))
        .args(["testhost", "true"])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !probe.status.success() {
        let mut c = child;
        let _ = c.kill();
        return None;
    }
    Some((child, d.join("ssh_config")))
}

#[test]
fn ssh_sessions_are_recorded_with_remote_marks() {
    let s = Server::start("ssh");
    let Some((sshd, cfg)) = local_sshd(&s.dir) else {
        eprintln!("skipping: no usable sshd here");
        return;
    };
    // Stopped even if an assertion fails.
    struct Kill(std::process::Child);
    impl Drop for Kill {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let _sshd = Kill(sshd);
    s.ctx(&["open", "tab", "named", "Remote", "at", "/tmp"]).ok();
    s.ctx(&["run", "in", "Remote", &format!("ssh -F {} testhost", cfg.display()), "without", "waiting"]).ok();
    // Wait for the remote prompt, then run commands there as if local.
    s.ctx(&["wait", "until", "Remote", "is", "quiet", "for", "2", "seconds", "within", "20", "seconds"]).ok();
    let o = s.ctx(&["run", "in", "Remote", "cd /usr && echo remote-$CTX_REMOTE && pwd"]);
    assert_eq!(o.stdout, "remote-1\n/usr\n", "{}", o.stderr);
    s.ctx(&["run", "in", "Remote", "exit", "within", "5", "seconds"]);
    s.ctx(&["wait", "until", "Remote", "is", "idle", "within", "10", "seconds"]).ok();
    let o = s.ctx(&["list", "commands", "in", "Remote", "as", "json"]).ok().stdout.clone();
    let v: serde_json::Value = serde_json::from_str(&o).unwrap();
    let remote =
        v["commands"].as_array().unwrap().iter().find(|c| c["command"].as_str().unwrap().contains("remote-")).unwrap();
    // Where it ran: the remote shell's own user, host and folder.
    assert!(
        remote["where"].as_str().unwrap().contains('@') && !remote["where"].as_str().unwrap().ends_with("/tmp"),
        "{remote}"
    );
    let screen = s.ctx(&["read", "Remote"]).ok().stdout.clone();
    assert!(screen.contains("[ssh to testhost started") && screen.contains("[ssh to testhost ended"), "{screen}");
}

#[test]
fn ctx_app_subscription_gets_screens_and_sends_input() {
    use ctx_protocol::*;
    use std::os::unix::net::UnixStream;
    let s = Server::start("app");
    s.ctx(&["open", "tab", "named", "Shown", "at", "/tmp"]).ok();
    // Connect as CTX.app would.
    let paths = {
        // SAFETY of set_var in a test: this test reads its own environment only.
        unsafe { std::env::set_var("CTX_HOME", &s.dir) };
        ctx_protocol::config::Paths::resolve(std::env::temp_dir(), 0, s.home.to_str().unwrap())
    };
    let mut c = UnixStream::connect(paths.socket()).expect("connect");
    let hello = Hello {
        protocol: PROTOCOL_VERSION,
        client_version: "test".into(),
        kind: ClientKind::App,
        folder: "/tmp".into(),
        in_tab: None,
        declared_name: None,
        interactive: false,
    };
    write_message(&mut c, &hello).unwrap();
    assert!(matches!(read_message::<Welcome>(&mut c).unwrap(), Some(Welcome::Accepted { .. })));
    let sub = StreamIn::Request {
        id: 1,
        envelope: Envelope { sentence: "subscribe".into(), request: Request::Subscribe { tabs: vec![1] } },
    };
    write_message(&mut c, &sub).unwrap();
    c.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
    let mut full = None;
    let mut tabs_listed = false;
    for _ in 0..10 {
        match read_message::<StreamOut>(&mut c).unwrap() {
            Some(StreamOut::Screen(u)) if full.is_none() => full = Some(u),
            Some(StreamOut::TabsChanged(list)) => {
                tabs_listed = list.windows[0].tabs[0].name.as_deref() == Some("Shown")
            }
            Some(StreamOut::Reply { id, .. }) => {
                assert_eq!(id, 1, "replies carry the request's id");
                break;
            }
            _ => {}
        }
    }
    let full = full.expect("a first screen");
    assert_eq!(full.changed.len(), full.rows as usize, "the first update is the whole screen");
    assert!(tabs_listed);
    let text = |u: &ScreenUpdate| {
        u.changed
            .iter()
            .map(|r| r.runs.iter().map(|x| x.text.as_str()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert!(text(&full).contains("[opened"), "{}", text(&full));
    assert!(full.changed.iter().any(|r| r.event), "event lines are marked");
    // Typing from the app reaches the shell; only changed rows come back.
    for k in ["e", "c", "h", "o", "space", "h", "i", "enter"] {
        let input = StreamIn::Request {
            id: 2,
            envelope: Envelope {
                sentence: String::new(),
                request: Request::Input { tab: 1, input: AppInput::Key { key: k.into(), text: None } },
            },
        };
        write_message(&mut c, &input).unwrap();
    }
    let start = std::time::Instant::now();
    let mut saw_output = false;
    while start.elapsed() < std::time::Duration::from_secs(5) && !saw_output {
        if let Ok(Some(StreamOut::Screen(u))) = read_message::<StreamOut>(&mut c) {
            assert!(u.changed.len() < u.rows as usize, "only changed rows");
            saw_output = text(&u).lines().any(|l| l.trim() == "hi");
        }
    }
    assert!(saw_output, "the app saw the command's output");
    // Finding text in the tab, and scrollback lines for scrolling, answer by id.
    let ask = |c: &mut UnixStream, id: u64, request: Request| -> Response {
        write_message(c, &StreamIn::Request { id, envelope: Envelope { sentence: String::new(), request } }).unwrap();
        loop {
            if let Some(StreamOut::Reply { id: got, response }) = read_message::<StreamOut>(c).unwrap()
                && got == id
            {
                return response;
            }
        }
    };
    match ask(&mut c, 7, Request::SearchTab { tab: 1, text: "echo hi".into(), limit: 10 }) {
        Response::Ok(Output::TabMatches(m)) => assert!(!m.matches.is_empty(), "{m:?}"),
        other => panic!("{other:?}"),
    }
    match ask(&mut c, 8, Request::History { tab: 1, from: None, count: 50 }) {
        Response::Ok(Output::History(h)) => assert!(h.end >= h.first && h.lines.len() as u64 == h.end - h.from),
        other => panic!("{other:?}"),
    }
    let o = s.ctx(&["list", "commands", "in", "Shown", "as", "json"]).ok().stdout.clone();
    assert!(o.contains("\"ran_by\": \"you\""), "keys from the app are the person's: {o}");
}

#[test]
fn dont_save_env_keeps_values_out_and_says_so_on_reopen() {
    let s = Server::start("hidden-env");
    std::fs::write(s.dir.join("config.toml"), "shell = \"/bin/zsh\"\ndont_save_env = [\"*_TOKEN\"]\n").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2500)); // the server rereads its config
    s.ctx(&["open", "tab", "named", "Secretive", "at", "/tmp"]).ok();
    s.ctx(&["run", "in", "Secretive", "export MY_TOKEN=$(printf hunt)er2; export PLAIN=visible"]).ok();
    s.ctx(&["close", "Secretive", "without", "asking"]).ok();
    // The value is in no record (the command line didn't contain it either).
    let h = ctx_store_history(&s.path("data/tabs/1.ctxlog"));
    assert!(!h.contains("hunter2"), "{h}");
    assert!(h.contains("PLAIN"));
    let o = s.ctx(&["reopen", "tab", "1"]).ok().stdout.clone();
    assert!(o.contains("Not restored (dont_save_env): MY_TOKEN."), "{o}");
    assert_eq!(s.ctx(&["run", "in", "tab", "1", "echo \"[$MY_TOKEN] $PLAIN\""]).ok().stdout, "[] visible\n");
}

#[test]
fn passwords_typed_by_agents_are_never_saved() {
    let s = Server::start("passwords");
    s.ctx(&["open", "tab", "named", "Login", "at", "/tmp"]).ok();
    // A password prompt: line input with echo off, as sudo and ssh set it. (The prompt is built
    // so it only shows once `read` is waiting.)
    s.ctx(&[
        "run",
        "in",
        "Login",
        "read -s \"pw?$(printf Pass%s word): \"; echo; echo \"got ${#pw}\"",
        "without",
        "waiting",
    ])
    .ok();
    screen_shows(&s, "Login", "Password:");
    s.ctx(&["type", "hunter2", "into", "Login", "then", "press", "enter"]).ok();
    screen_shows(&s, "Login", "got 7");
    // A field that reads keystrokes without showing them (a TUI drawing `***`, a remote sudo).
    s.ctx(&[
        "run",
        "in",
        "Login",
        "stty raw -echo; head -c 8 >/dev/null; stty sane; echo raw-done",
        "without",
        "waiting",
    ])
    .ok();
    std::thread::sleep(std::time::Duration::from_millis(500));
    s.ctx(&["type", "swordfsh", "into", "Login"]).ok();
    screen_shows(&s, "Login", "raw-done");
    // Text that shows on screen is logged in full.
    s.ctx(&["run", "in", "Login", "cat", "without", "waiting"]).ok();
    std::thread::sleep(std::time::Duration::from_millis(500));
    s.ctx(&["type", "plain words", "into", "Login", "then", "press", "enter"]).ok();
    screen_shows(&s, "Login", "plain words");
    s.ctx(&["press", "ctrl-c", "in", "Login"]).ok();
    std::thread::sleep(std::time::Duration::from_millis(2500)); // unseen text is hidden after 2 s

    let o = s.ctx(&["list", "activity", "in", "Login", "as", "json"]).ok().stdout.clone();
    let v: serde_json::Value = serde_json::from_str(&o).unwrap();
    let entries = v["entries"].as_array().unwrap();
    let hidden: Vec<&str> =
        entries.iter().filter(|e| e["typed_hidden"] == true).map(|e| e["sentence"].as_str().unwrap()).collect();
    assert_eq!(hidden.len(), 2, "{o}");
    assert!(hidden.contains(&"ctx type [hidden] into tab 1 then press enter"), "{o}");
    assert!(hidden.contains(&"ctx type [hidden] into tab 1"), "{o}");
    assert!(entries.iter().any(|e| e["typed"] == "plain words"), "{o}");
    s.ctx(&["close", "Login", "without", "asking"]).ok();
    // Nowhere on disk: not the activity log, not the tab's record.
    let log = std::fs::read_to_string(s.path("data/activity.log")).unwrap();
    let h = ctx_store_history(&s.path("data/tabs/1.ctxlog"));
    for secret in ["hunter2", "swordfsh"] {
        assert!(!o.contains(secret) && !log.contains(secret) && !h.contains(secret), "{secret} was saved");
    }
    assert!(log.contains("plain words"));
}

/// Wait (up to 5 s) until the tab's screen shows `text`, which may be there already.
fn screen_shows(s: &Server, tab: &str, text: &str) {
    for _ in 0..50 {
        if s.ctx(&["read", "screen", "in", tab]).stdout.contains(text) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    panic!("{text:?} didn't appear in {tab}: {}", s.ctx(&["read", "screen", "in", tab]).stdout);
}

/// Every record of a tab, decoded, as text (for checking what was saved).
fn ctx_store_history(path: &std::path::Path) -> String {
    let (records, _) = ctx_store::record::read_records(path).unwrap();
    format!("{records:?}")
}

#[test]
fn a_copy_keeps_folder_exported_variables_and_history() {
    let s = Server::start("copy");
    s.ctx(&["open", "tab", "named", "Src", "at", "/tmp"]).ok();
    s.ctx(&["open", "tab", "named", "Other"]).ok();
    s.ctx(&["run", "in", "Src", "export KEEP=kept; LOCAL=not-exported; cd /usr; echo first-command"]).ok();
    s.ctx(&["run", "in", "Src", "sleep 600", "without", "waiting"]).ok();
    // A copy of a busy tab: same folder and exported variables, its history, nothing running.
    let o = s.ctx(&["open", "tab", "like", "Src"]).ok().stdout.clone();
    assert!(o.contains("tab 3 \"Src 2\"") && o.contains("Copied from tab 1"), "{o}");
    let o = s.ctx(&["run", "in", "Src 2", "echo $KEEP-$LOCAL; pwd"]).ok().stdout.clone();
    assert_eq!(o, "kept-\n/usr\n");
    // Tabs an agent opens (the tests run as one) have no pager to wait on.
    assert_eq!(s.ctx(&["run", "in", "Src 2", "echo $PAGER $GIT_PAGER"]).ok().stdout, "cat cat\n");
    let o = s.ctx(&["run", "in", "Src 2", "fc -ln 1"]).ok().stdout.clone();
    assert!(o.contains("echo first-command") && o.contains("sleep 600"), "{o}");
    let screen = s.ctx(&["read", "Src 2"]).ok().stdout.clone();
    assert!(screen.contains("like tab 1 \"Src\": same folder, exported variables and history"), "{screen}");
    // Beside the tab it copies; a copy of a copy takes the next number.
    let list = s.ctx(&["list", "tabs"]).ok().stdout.clone();
    let at = |n: &str| list.find(n).unwrap();
    assert!(at("\"Src\"") < at("\"Src 2\"") && at("\"Src 2\"") < at("\"Other\""), "{list}");
    assert!(s.ctx(&["open", "tab", "like", "Src 2"]).ok().stdout.contains("\"Src 3\""));
    // A name and a folder can still be given; a window can't be a copy.
    let o = s.ctx(&["open", "tab", "like", "tab", "1", "named", "Elsewhere", "at", "/"]).ok().stdout.clone();
    assert!(o.contains("\"Elsewhere\"") && o.contains(" at /."), "{o}");
    assert_ne!(s.ctx(&["open", "window", "like", "Src"]).code, 0);
    s.ctx(&["press", "ctrl-c", "in", "Src"]).ok();
}

#[test]
fn the_hook_sends_background_and_busy_calls_to_copies_of_the_session_tab() {
    let s = Server::start("hook-copies");
    let call = |command: &str, background: bool| {
        let input = serde_json::json!({
            "session_id": "abc12345-xyz", "tool_name": "Bash", "cwd": "/tmp",
            "tool_input": {"command": command, "run_in_background": background, "timeout": 20000},
        });
        let o = s.ctx_in(&["hook", "claude"], Some(&input.to_string())).ok().stdout.clone();
        let v: serde_json::Value = serde_json::from_str(&o).unwrap();
        let sentence = v["hookSpecificOutput"]["updatedInput"]["command"].as_str().unwrap().to_string();
        let words: Vec<String> = ctx_mcp::shell_words(&sentence).unwrap();
        (sentence, s.ctx(&words[1..].iter().map(String::as_str).collect::<Vec<_>>()))
    };
    let (sentence, o) = call("export SESSION_VAR=1; echo ready", false);
    assert!(sentence.starts_with("ctx run in 'Claude abc12345' "), "{sentence}");
    assert_eq!(o.stdout, "ready\n");
    // The session tab stands in for the agent's own shell, which has no job control: no job notices.
    let (_, o) = call("sleep 0.2 & wait; echo after", false);
    assert_eq!(o.stdout, "after\n", "no \"[1] <pid>\" or done lines");
    // A background call gets its own copy, named for the command; the session tab stays free.
    let (sentence, _) = call("sleep 600; echo never", true);
    assert!(sentence.starts_with("ctx run in 'Claude abc12345 - sleep 600 echo never' "), "{sentence}");
    assert!(sentence.ends_with(" without waiting"), "{sentence}");
    let (sentence, o) = call("echo $SESSION_VAR", false);
    assert!(sentence.starts_with("ctx run in 'Claude abc12345' "), "{sentence}");
    assert_eq!(o.stdout, "1\n");
    // A foreground call that outlives its wait leaves the session tab busy; the next call goes to a
    // copy (with the session's variables) instead of failing.
    let (_, o) = call("sleep 600", false);
    assert_eq!(o.code, 3, "{}", o.stderr);
    let (sentence, o) = call("echo $SESSION_VAR", false);
    assert!(sentence.starts_with("ctx run in 'Claude abc12345 2' "), "{sentence}");
    assert_eq!(o.stdout, "1\n");
    // …and keeps going there while it's free.
    assert!(call("true", false).0.starts_with("ctx run in 'Claude abc12345 2' "));
    for t in ["Claude abc12345", "Claude abc12345 - sleep 600 echo never"] {
        s.ctx(&["press", "ctrl-c", "in", t]).ok();
    }
}

#[test]
fn reopen_tabs_from_last_session_after_a_crash_and_a_clean_stop() {
    let mut s = Server::start("last-session");
    s.ctx(&["open", "window", "named", "Work", "at", "/tmp"]).ok();
    s.ctx(&["open", "tab", "named", "One", "at", "/usr", "in", "window", "Work"]).ok();
    s.ctx(&["open", "tab", "named", "Two", "at", "/tmp", "in", "window", "Work"]).ok();
    s.ctx(&["run", "in", "One", "export SESSION_KEEP=yes"]).ok();
    s.ctx(&["run", "in", "One", "true"]).ok();
    let before = s.ctx(&["list", "tabs"]).ok().stdout.clone();
    // A crash: nothing comes back by itself (reopen_on_launch = ask).
    s.kill_hard();
    s.spawn();
    let after_crash = s.ctx(&["list", "tabs"]).ok().stdout.clone();
    assert!(!after_crash.contains("\"One\""), "{after_crash}");
    let o = s.ctx(&["reopen", "tabs", "from", "last", "session"]).ok().stdout.clone();
    assert!(o.starts_with("Reopened 3 tabs from the last session"), "{o}");
    let again = s.ctx(&["list", "tabs"]).ok().stdout.clone();
    // (A reopened tab shows its recorded folder until its shell reports where it is.)
    let same = |s: &str| s.replace("/private/tmp", "/tmp");
    assert_eq!(same(&again), same(&before), "same tabs, numbers, names, folders and window");
    assert_eq!(s.ctx(&["run", "in", "One", "echo $SESSION_KEEP"]).ok().stdout, "yes\n");
    // Running it again finds nothing to do.
    let o = s.ctx(&["reopen", "tabs", "from", "the", "last", "session"]).ok().stdout.clone();
    assert_eq!(o.trim(), "Every tab from the last session is already open.");
    // One tab closed by hand isn't part of the next session; a clean stop (SIGTERM) is.
    s.ctx(&["close", "Two", "without", "asking"]).ok();
    s.stop();
    s.spawn();
    let o = s.ctx(&["reopen", "tabs", "from", "last", "session"]).ok().stdout.clone();
    assert!(o.starts_with("Reopened 2 tabs from the last session"), "{o}");
    assert!(!s.ctx(&["list", "tabs"]).ok().stdout.contains("\"Two\""));
    // …and a tab closed by hand comes back on its own.
    s.ctx(&["reopen", "tab", "3"]).ok();
    assert!(s.ctx(&["list", "tabs"]).ok().stdout.contains("tab 3 \"Two\""));
}

#[test]
fn ctx_folders_are_made_and_in_every_tabs_environment() {
    let s = Server::start("folders");
    for f in ["Agents", "Context", "Workspaces"] {
        assert!(s.home.join(f).is_dir(), "{f} wasn't made");
    }
    s.ctx(&["open", "tab", "named", "Env", "at", "/tmp"]).ok();
    let o = s.ctx(&["run", "in", "Env", "echo $CTX_AGENTS $CTX_CONTEXT $CTX_WORKSPACES"]).ok().stdout.clone();
    let h = s.home.display();
    assert_eq!(o, format!("{h}/Agents {h}/Context {h}/Workspaces\n"));
}
