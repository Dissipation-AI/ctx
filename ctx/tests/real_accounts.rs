//! Agents as real OS accounts, end to end: a root helper (through `sudo -n`), a private ctx
//! server, and two throwaway agent accounts. It makes and deletes real accounts and a group, so
//! it runs only when `CTX_REAL_ACCOUNTS=1` and sudo needs no password (CI runners).

mod common;
use common::Server;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn enabled() -> bool {
    std::env::var("CTX_REAL_ACCOUNTS").as_deref() == Ok("1")
        && Command::new("sudo").args(["-n", "true"]).status().is_ok_and(|s| s.success())
}

fn helper_bin() -> PathBuf {
    let p = common::ctx_bin().with_file_name("ctx-helper");
    if !p.exists() {
        let ok = Command::new(env!("CARGO")).args(["build", "-p", "ctx-helper"]).status().unwrap().success();
        assert!(ok);
    }
    p
}

/// The root helper; stopped (and the agents' accounts removed) on drop.
struct RootHelper {
    child: Child,
    accounts: Vec<String>,
}

impl Drop for RootHelper {
    fn drop(&mut self) {
        for a in &self.accounts {
            if cfg!(target_os = "macos") {
                let _ = Command::new("sudo").args(["-n", "dscl", ".", "-delete", &format!("/Users/{a}")]).status();
            } else {
                let _ = Command::new("sudo").args(["-n", "userdel", a]).status();
            }
        }
        let _ = Command::new("sudo").args(["-n", "kill", &self.child.id().to_string()]).status();
        let _ = self.child.wait();
    }
}

fn me() -> String {
    String::from_utf8(Command::new("id").arg("-un").output().unwrap().stdout).unwrap().trim().to_string()
}

/// The tab number in "Opened tab N as the agent …".
fn opened(o: &str) -> String {
    o.split_whitespace().nth(2).unwrap_or_default().to_string()
}

fn wait_for(f: impl Fn() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

#[test]
fn agents_are_accounts_and_the_os_decides_what_they_reach() {
    if !enabled() {
        eprintln!("CTX_REAL_ACCOUNTS isn't 1 or sudo asks for a password; skipping");
        return;
    }
    let tag = std::process::id() % 100000;
    let (a, b) = (format!("ctxt-a{tag}"), format!("ctxt-b{tag}"));
    let dir = std::env::temp_dir().join(format!("ctx-real-{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let sock = format!("/tmp/ctx-real-{tag}.sock");
    let rules = dir.join("helper.toml");
    std::fs::write(&rules, format!("owner = \"{}\"\n", me())).unwrap();
    let child = Command::new("sudo")
        .args(["-n", "env", &format!("CTX_HELPER_SOCKET={sock}"), &format!("CTX_HELPER_CONFIG={}", rules.display())])
        .arg(helper_bin())
        .stdout(Stdio::null())
        .stderr(Stdio::from(std::fs::File::create(dir.join("helper.log")).unwrap()))
        .spawn()
        .unwrap();
    let _h = RootHelper { child, accounts: vec![a.clone(), b.clone()] };
    assert!(wait_for(|| Path::new(&sock).exists()), "the helper didn't start");

    // Everything the agents reach lives where other accounts can pass: /tmp, not the per-user
    // temporary folder (mode 700), and a copy of ctx they can run.
    let open_dir = PathBuf::from(format!("/tmp/ctx-real-{tag}"));
    let _ = std::fs::remove_dir_all(&open_dir);
    std::fs::create_dir_all(open_dir.join("bin")).unwrap();
    for d in [&open_dir, &open_dir.join("bin")] {
        std::fs::set_permissions(d, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    }
    let ctx_copy = open_dir.join("bin/ctx");
    std::fs::copy(common::ctx_bin(), &ctx_copy).unwrap();
    // SAFETY: the only test in this file; nothing else reads the environment concurrently.
    unsafe { std::env::set_var("TMPDIR", &open_dir) };
    let s = Server::start_with("real", &[("CTX_HELPER_SOCKET", &sock)]);
    let agents = s.home.join("Agents");
    let ctx_path = ctx_copy.display().to_string();

    // Making agents: accounts, homes in ~/Agents with the starter files, and a shared folder.
    let o = s.ctx(&["make", "agent", "named", &a]).ok().stdout.clone();
    assert!(o.contains(&format!("Made the agent \"{a}\"")) && o.contains("ctx switch to agent"), "{o}");
    s.ctx(&["make", "agent", "named", &b]).ok();
    for f in ["CLAUDE.md", "AGENTS.md", "ctx-agent.toml"] {
        assert!(agents.join(&a).join(f).exists(), "{f}");
    }
    assert!(agents.join("Shared/everyone").is_dir());
    let o = s.ctx(&["show", "agent", &a]).ok().stdout.clone();
    assert!(o.contains("you may read and write its home"), "{o}");

    // Switching: a tab as the account, in its home.
    let ta = opened(&s.ctx(&["switch", "to", "agent", &a]).ok().stdout);
    let tb = opened(&s.ctx(&["switch", "to", "agent", &b]).ok().stdout);
    s.ctx(&["open", "tab", "named", "Mine", "at", "/tmp"]).ok();
    let o = s.ctx(&["run", "in", "tab", &ta, "id -un; pwd; echo $CTX_AGENTS"]).ok().stdout.clone();
    assert_eq!(o.lines().next(), Some(a.as_str()), "{o}");
    assert!(o.contains(&format!("Agents/{a}")), "{o}");
    let list = s.ctx(&["list", "tabs"]).ok().stdout.clone();
    assert!(list.contains(&format!("as {a}")), "{list}");

    // From a's tab: only its own tab, never yours, not b's until it may read b's home.
    let in_a = |cmd: &str| s.ctx(&["run", "in", "tab", &ta, &format!("'{ctx_path}' {cmd}")]);
    let o = in_a("list tabs").stdout;
    assert!(o.contains(&format!("tab {ta}")) && !o.contains("Mine") && !o.contains(&format!("tab {tb} ")), "{o}");
    assert!(in_a(&format!("run in tab {tb} 'id -un'")).code != 0);
    s.ctx(&["allow", "agent", &a, "to", "read", "agent", &b]).ok();
    assert!(wait_for(|| in_a("list tabs").stdout.contains(&format!("as {b}"))), "a should see b's tab");
    assert!(in_a(&format!("run in tab {tb} 'id -un'")).code != 0, "reading isn't driving");
    s.ctx(&["allow", "agent", &a, "to", "write", "agent", &b]).ok();
    assert!(wait_for(|| in_a(&format!("run in tab {tb} 'id -un'")).stdout.contains(&b)), "a administers b now");
    let o = s.ctx(&["show", "agent", &b]).ok().stdout.clone();
    assert!(o.contains(&format!("{a} may read and write its home (administers it)")), "{o}");
    s.ctx(&["forbid", "agent", &a, "to", "read", "agent", &b]).ok();
    assert!(wait_for(|| !in_a("list tabs").stdout.contains(&format!("as {b}"))), "a lost b's tabs");
    assert!(!in_a("list tabs").stdout.contains("Mine"), "no agent ever sees your tabs");

    // Your files: closed until shared, and only while shared.
    std::fs::create_dir_all(s.home.join("Context/career")).unwrap();
    std::fs::write(s.home.join("Context/career/cv.md"), "my cv").unwrap();
    let cv = s.home.join("Context/career/cv.md").display().to_string();
    let cat = || s.ctx(&["run", "in", "tab", &ta, &format!("cat '{cv}'")]).stdout;
    assert!(!cat().contains("my cv"));
    let career = s.home.join("Context/career").display().to_string();
    s.ctx(&["allow", "agent", &a, "to", "read", &career]).ok();
    assert!(cat().contains("my cv"));
    let o = s.ctx(&["run", "in", "tab", &ta, &format!("echo x > '{career}/new.md'")]).stdout;
    assert!(o.contains("ermission denied"), "read isn't write: {o}");
    s.ctx(&["forbid", "agent", &a, "to", "read", &career]).ok();
    assert!(!cat().contains("my cv"));

    // An agent can't share what isn't its own, and can't make agents.
    let o = in_a(&format!("allow agent {a} to read '{career}'"));
    assert!(o.code != 0 && o.stderr.contains("only"), "{}", o.stderr);
    let o = in_a("make agent named sneaky");
    assert!(o.code != 0, "{}", o.stdout);

    // ctx-agent.toml: yours; the agent can't change it, and a replaced one isn't believed.
    let o = s.ctx(&["run", "in", "tab", &ta, "echo 'colour = \"red\"' >> ctx-agent.toml"]).stdout;
    assert!(o.contains("ermission denied"), "{o}");
    s.ctx(&["limit", "agent", &a, "to", "50", "processes"]).ok();
    let o = s.ctx(&["switch", "to", "agent", &a]).ok().stdout.clone();
    let t2 = opened(&o);
    let o = s.ctx(&["run", "in", "tab", &t2, "ulimit -u"]).ok().stdout.clone();
    assert_eq!(o.trim(), "50");
    s.ctx(&["run", "in", "tab", &ta, "rm -f ctx-agent.toml && echo 'colour = \"red\"' > ctx-agent.toml"]).ok();
    let o = s.ctx(&["switch", "to", "agent", &a]);
    assert!(o.code != 0 && o.stderr.contains("ctx reset agent"), "{}", o.stderr);
    s.ctx(&["reset", "agent", &a]).ok();
    s.ctx(&["switch", "to", "agent", &a]).ok();

    // A shared folder an agent asks for: made by the server, for it, b and you.
    let o = in_a(&format!("make shared folder with agent {b}"));
    assert_eq!(o.code, 0, "{}", o.stderr);
    let shared = agents.join("Shared").join({
        let mut v = [a.clone(), b.clone()];
        v.sort();
        v.join("+")
    });
    assert!(shared.is_dir(), "{}", o.stdout);
    let o = s.ctx(&[
        "run",
        "in",
        "tab",
        &tb,
        &format!("echo hi > '{}/note' && cat '{}/note'", shared.display(), shared.display()),
    ]);
    assert_eq!(o.ok().stdout, "hi\n");

    // Removing: accounts go, homes to the Trash with `with its files`.
    for t in [&ta, &tb] {
        s.ctx(&["close", "tab", t, "without", "asking"]);
    }
    s.ctx(&["remove", "agent", &a, "with", "its", "files"]).ok();
    s.ctx(&["remove", "agent", &b]).ok();
    assert!(Command::new("id").arg(&a).output().unwrap().stdout.is_empty());
    assert!(!agents.join(&a).exists() && agents.join(&b).exists());
}
