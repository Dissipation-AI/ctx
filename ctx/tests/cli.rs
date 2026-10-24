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
    s.ctx(&["run", "in", "Stay", "export STAY=1; cd /usr"]).ok();
    s.ctx(&["restart", "server", "without", "asking"]).ok();
    let o = s.ctx(&["run", "in", "Stay", "echo $STAY; pwd"]).ok().stdout.clone();
    assert_eq!(o, "1\n/usr\n");
}

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
fn agents_see_and_drive_only_their_own_tabs_and_run_sandboxed() {
    let s = Server::start("agents");
    std::fs::create_dir_all(s.home.join("Secret")).unwrap();
    std::fs::write(s.home.join("Secret/key"), "hunter2").unwrap();
    std::fs::create_dir_all(s.home.join("Work")).unwrap();
    std::fs::write(
        s.dir.join("config.toml"),
        "shell = \"/bin/zsh\"\nagent_programs = [\"fakeagent\"]\n[agents.dev]\nfolders = [\"~/Work\"]\nwrite = [\"~/Work\"]\n",
    )
    .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2500)); // the server rereads its config
    s.ctx(&["open", "tab", "named", "Mine", "at", "/tmp"]).ok();
    let dev = |args: &[&str]| s.ctx_with(&[("CTX_AGENT_NAME", "dev")], args, None);

    // Your tab doesn't exist for the agent.
    assert!(!dev(&["list", "tabs"]).ok().stdout.contains("Mine"));
    let o = dev(&["run", "in", "Mine", "echo hi"]);
    assert_eq!(o.code, 1);
    assert!(o.stderr.contains("There's no tab named \"Mine\""), "{}", o.stderr);

    // Its own tab runs in its sandbox: your home is closed except what it was given.
    let work = s.home.join("Work").display().to_string();
    dev(&["open", "tab", "named", "Dev work", "at", &work]).ok();
    let o = dev(&["run", "in", "Dev work", "cat ~/Secret/key"]);
    assert!(o.stdout.contains("Operation not permitted") && !o.stdout.contains("hunter2"), "{}", o.stdout);
    assert_eq!(dev(&["run", "in", "Dev work", "echo made > ~/Work/out && cat ~/Work/out"]).ok().stdout, "made\n");

    // Inside the sandbox ctx knows it's the agent, whatever the process says about itself.
    let ctx = common::ctx_bin().display().to_string();
    let o = dev(&["run", "in", "Dev work", &format!("env -u CTX_AGENT_NAME '{ctx}' list tabs")]).ok().stdout.clone();
    assert!(o.contains("Dev work") && !o.contains("Mine"), "{o}");
    let o = dev(&["run", "in", "Dev work", &format!("env -u CTX_AGENT_NAME '{ctx}' run in tab 1 'echo escaped'")]);
    assert!(o.stdout.contains("There is no tab 1"), "{}", o.stdout);

    // You see everything, with its owner, and can drive the agent's tab.
    let o = s.ctx(&["list", "tabs"]).ok().stdout.clone();
    assert!(o.contains("Mine") && o.contains("\"Dev work\"") && o.contains("belongs to the agent \"dev\""), "{o}");
    assert_eq!(s.ctx(&["run", "in", "Dev work", "echo yours"]).ok().stdout, "yours\n");
    // An agent program started in your tab runs as the agent for its folder.
    std::fs::create_dir_all(s.home.join("bin")).unwrap();
    let fake = s.home.join("bin/fakeagent");
    std::fs::write(&fake, "#!/bin/sh\necho \"agent=$CTX_AGENT\"; cat ~/Secret/key\n").unwrap();
    std::process::Command::new("chmod").arg("755").arg(&fake).status().unwrap();
    s.ctx(&["run", "in", "Mine", "export PATH=~/bin:$PATH"]).ok();
    let o = s.ctx(&["run", "in", "Mine", "cd ~/Work && fakeagent"]).stdout;
    assert!(o.contains("agent=dev") && o.contains("Operation not permitted") && !o.contains("hunter2"), "{o}");
    let o = s.ctx(&["run", "in", "Mine", "cd /tmp && fakeagent"]).ok().stdout.clone();
    assert!(o.starts_with("agent=\nhunter2"), "outside every agent's folders it runs as you: {o}");

    // The log names the agent.
    let o = s.ctx(&["list", "activity", "by", "dev", "as", "json"]).ok().stdout.clone();
    let v: serde_json::Value = serde_json::from_str(&o).unwrap();
    let clients: Vec<&str> = v["entries"].as_array().unwrap().iter().map(|e| e["client"].as_str().unwrap()).collect();
    assert!(!clients.is_empty() && clients.iter().all(|c| c.starts_with("dev: ")), "{clients:?}");
}

/// A session whose tab is yours (it started before the agent was configured, or was resumed) gets a
/// tab of the agent's own instead of commands it isn't allowed to run.
#[test]
fn an_agents_hook_never_reuses_a_tab_it_cant_drive() {
    let s = Server::start("hook-owner");
    std::fs::write(s.dir.join("config.toml"), "shell = \"/bin/zsh\"\n[agents.dev]\nsee_tabs_of = [\"you\"]\n").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2500)); // the server rereads its config
    s.ctx(&["open", "tab", "named", "Claude abc12345", "at", "/tmp"]).ok();
    let call =
        r#"{"session_id": "abc12345-xyz", "tool_name": "Bash", "tool_input": {"command": "echo hi"}, "cwd": "/tmp"}"#;
    let hook = |env: &[(&str, &str)]| {
        let o = s.ctx_with(env, &["hook", "claude"], Some(call)).ok().stdout.clone();
        let v: serde_json::Value = serde_json::from_str(&o).unwrap();
        v["hookSpecificOutput"]["updatedInput"]["command"].as_str().unwrap().to_string()
    };
    let dev = [("CTX_AGENT_NAME", "dev")];
    let sentence = hook(&dev);
    assert!(sentence.starts_with("ctx run in 'Claude abc12345 dev' "), "{sentence}");
    let o = s.ctx(&["list", "tabs"]).ok().stdout.clone();
    assert!(o.contains("\"Claude abc12345 dev\"") && o.contains("belongs to the agent \"dev\""), "{o}");
    // The next call reuses it, and you keep yours.
    assert_eq!(hook(&dev), sentence);
    assert!(hook(&[]).starts_with("ctx run in 'Claude abc12345' "));
    let words: Vec<String> = ctx_mcp::shell_words(&sentence).unwrap();
    let args: Vec<&str> = words[1..].iter().map(String::as_str).collect();
    assert_eq!(s.ctx_with(&dev, &args, None).ok().stdout, "hi\n");
}

/// The helper, unprivileged: the only account it can become is this one, which is enough to see
/// the whole path (rules, the attached terminal, the door, the sandbox) work.
#[test]
fn agent_accounts_start_through_the_helper() {
    let helper = common::ctx_bin().with_file_name("ctx-helper");
    if !helper.exists() {
        assert!(
            std::process::Command::new(env!("CARGO")).args(["build", "-p", "ctx-helper"]).status().unwrap().success()
        );
    }
    let me = std::env::var("USER").unwrap();
    let rules_dir = std::env::temp_dir().join(format!("ctx-it-helper-{}", std::process::id()));
    std::fs::create_dir_all(&rules_dir).unwrap();
    let rules = rules_dir.join("helper.toml");
    std::fs::write(&rules, format!("owner = \"{me}\"\n[accounts]\ndev = \"{me}\"\n")).unwrap();
    let sock = format!("/tmp/ctx-it-helper-{}.sock", std::process::id());
    let mut h = std::process::Command::new(&helper)
        .env("CTX_HELPER_SOCKET", &sock)
        .env("CTX_HELPER_CONFIG", &rules)
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    while !std::path::Path::new(&sock).exists() {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let s = Server::start_with("accounts", &[("CTX_HELPER_SOCKET", &sock)]);
    std::fs::create_dir_all(s.home.join("Secret")).unwrap();
    std::fs::write(s.home.join("Secret/key"), "hunter2").unwrap();
    std::fs::create_dir_all(s.home.join("Work")).unwrap();
    std::fs::write(
        s.dir.join("config.toml"),
        format!("shell = \"/bin/zsh\"\n[agents.dev]\naccount = \"{me}\"\nwrite = [\"~/Work\"]\n"),
    )
    .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2500));
    let work = s.home.join("Work").display().to_string();
    s.ctx_with(&[("CTX_AGENT_NAME", "dev")], &["open", "tab", "named", "Acct", "at", &work], None).ok();
    // The helper started the shell, which reaches the server through the door, in the sandbox.
    // (The account's home is its own, so the secret goes by its full path.)
    let secret = s.home.join("Secret/key").display().to_string();
    let o = s.ctx(&["run", "in", "Acct", &format!("echo $PPID; echo $CTX_SOCKET; cat '{secret}'")]).stdout;
    let lines: Vec<&str> = o.lines().collect();
    assert_eq!(lines[0].trim(), h.id().to_string(), "{o}");
    assert!(lines[1].ends_with("server.sock"), "{o}");
    assert!(o.contains("Operation not permitted") && !o.contains("hunter2"), "{o}");
    let ctx = common::ctx_bin().display().to_string();
    let o = s.ctx(&["run", "in", "Acct", &format!("env -u CTX_AGENT_NAME '{ctx}' list tabs")]).ok().stdout.clone();
    assert!(o.contains("\"Acct\""), "{o}");
    // A request for an agent the rules don't have is refused, and says so.
    std::fs::write(
        s.dir.join("config.toml"),
        format!("shell = \"/bin/zsh\"\n[agents.dev]\naccount = \"{me}\"\n[agents.mail]\naccount = \"{me}\"\n"),
    )
    .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2500));
    let o = s.ctx_with(&[("CTX_AGENT_NAME", "mail")], &["open", "tab", "named", "Mail"], None);
    assert!(o.stderr.contains("don't have the agent \"mail\""), "{}", o.stderr);
    let _ = h.kill();
    let _ = h.wait();
    let _ = std::fs::remove_file(&sock);
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
