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
    // Always built: a helper left from an older build wouldn't know newer requests.
    let ok = Command::new(env!("CARGO")).args(["build", "-p", "ctx-helper"]).status().unwrap().success();
    assert!(ok);
    common::ctx_bin().with_file_name("ctx-helper")
}

/// The root helper; stopped (and the agents' accounts removed) on drop.
struct RootHelper {
    child: Child,
    accounts: Vec<String>,
    /// root's folder holding agents' homes, removed at the end.
    sys: PathBuf,
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
        let _ = Command::new("sudo").args(["-n", "rm", "-rf"]).arg(&self.sys).status();
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
    // Agents' homes: made by the helper, outside your home, in a folder of root's (as
    // /Users/Shared and /home are).
    let sys = PathBuf::from(format!("/tmp/ctx-real-sys-{tag}"));
    assert!(Command::new("sudo").args(["-n", "mkdir", "-p", "-m", "755"]).arg(&sys).status().unwrap().success());
    let homes = sys.join("homes");
    let homes_env = format!("CTX_AGENT_HOMES={}", homes.display());
    let rules = dir.join("helper.toml");
    std::fs::write(&rules, format!("owner = \"{}\"\n", me())).unwrap();
    let child = Command::new("sudo")
        .args([
            "-n",
            "env",
            &format!("CTX_HELPER_SOCKET={sock}"),
            &format!("CTX_HELPER_CONFIG={}", rules.display()),
            &homes_env,
        ])
        .arg(helper_bin())
        .stdout(Stdio::null())
        .stderr(Stdio::from(std::fs::File::create(dir.join("helper.log")).unwrap()))
        .spawn()
        .unwrap();
    let _h = RootHelper { child, accounts: vec![a.clone(), b.clone(), format!("ctxt-c{tag}")], sys: sys.clone() };
    assert!(wait_for(|| Path::new(&sock).exists()), "the helper didn't start");

    // The test's ctx server lives under /tmp, not the per-user temporary folder (mode 700). ctx
    // itself is installed where agents can run it: a folder of root's, as an install puts it
    // (sharing a file of yours closes your other folders on the way to it, so ctx can't be in
    // one of those).
    let open_dir = PathBuf::from(format!("/tmp/ctx-real-{tag}"));
    let _ = std::fs::remove_dir_all(&open_dir);
    std::fs::create_dir_all(&open_dir).unwrap();
    std::fs::set_permissions(&open_dir, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let ctx_copy = sys.join("bin/ctx");
    let ok =
        Command::new("sudo").args(["-n", "mkdir", "-p", "-m", "755"]).arg(sys.join("bin")).status().unwrap().success()
            && Command::new("sudo")
                .args(["-n", "install", "-m", "755"])
                .arg(common::ctx_bin())
                .arg(&ctx_copy)
                .status()
                .unwrap()
                .success();
    assert!(ok, "couldn't install ctx for the agents");
    // SAFETY: the only test in this file; nothing else reads the environment concurrently.
    unsafe { std::env::set_var("TMPDIR", &open_dir) };
    let s = Server::start_with("real", &[("CTX_HELPER_SOCKET", &sock), ("CTX_AGENT_HOMES", homes.to_str().unwrap())]);
    // A home other accounts can enter is reported; a real home is private (macOS 700, Linux 750).
    std::fs::set_permissions(&s.home, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let o = s.ctx(&["make", "agent", "named", &a]).ok().stdout.clone();
    assert!(o.contains("lets every account in"), "{o}");
    s.ctx(&["remove", "agent", &a, "with", "its", "files"]).ok();
    std::fs::set_permissions(&s.home, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
    let agents = s.home.join("Agents");
    let ctx_path = ctx_copy.display().to_string();

    // Making agents: accounts, homes outside yours with the starter files (~/Agents links to
    // them), and a shared folder.
    let o = s.ctx(&["make", "agent", "named", &a]).ok().stdout.clone();
    assert!(o.contains(&format!("Made the agent \"{a}\"")) && o.contains("ctx switch to agent"), "{o}");
    assert!(!o.contains("lets every account in"), "{o}");
    s.ctx(&["make", "agent", "named", &b]).ok();
    for f in ["CLAUDE.md", "AGENTS.md", "ctx-agent.toml"] {
        assert!(homes.join(&a).join(f).exists(), "{f}");
    }
    assert_eq!(std::fs::read_link(agents.join(&a)).unwrap(), homes.join(&a));
    // A folder of yours as a home moves out of your home, leaving a link.
    let c = format!("ctxt-c{tag}");
    std::fs::create_dir_all(s.home.join("old-c")).unwrap();
    std::fs::write(s.home.join("old-c/notes.md"), "kept").unwrap();
    let o =
        s.ctx(&["make", "agent", "named", &c, "from", &s.home.join("old-c").display().to_string()]).ok().stdout.clone();
    assert!(o.contains("Moved"), "{o}");
    assert_eq!(std::fs::read_link(s.home.join("old-c")).unwrap(), homes.join(&c));
    assert_eq!(std::fs::read_to_string(homes.join(&c).join("notes.md")).unwrap(), "kept");
    s.ctx(&["remove", "agent", &c, "with", "its", "files"]).ok();
    assert!(agents.join("Shared/everyone").is_dir());
    let o = s.ctx(&["show", "agent", &a]).ok().stdout.clone();
    assert!(o.contains("you may read and write its home"), "{o}");

    // Switching: a tab as the account, in its home.
    let ta = opened(&s.ctx(&["switch", "to", "agent", &a]).ok().stdout);
    let tb = opened(&s.ctx(&["switch", "to", "agent", &b]).ok().stdout);
    s.ctx(&["open", "tab", "named", "Mine", "at", "/tmp"]).ok();
    let o = s.ctx(&["run", "in", "tab", &ta, "id -un; pwd; echo $CTX_AGENTS"]).ok().stdout.clone();
    let lines: Vec<&str> = o.lines().collect();
    assert_eq!(lines.first(), Some(&a.as_str()), "{o}");
    assert_eq!(lines.get(1).copied(), Some(homes.join(&a).to_str().unwrap()), "{o}");
    assert_eq!(lines.get(2).copied(), Some(homes.to_str().unwrap()), "{o}");
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

    // Your files: closed until shared, and only while shared. Files beside what's shared stay
    // closed even though anyone may read them by their mode.
    std::fs::create_dir_all(s.home.join("Context/career")).unwrap();
    std::fs::write(s.home.join("Context/career/cv.md"), "my cv").unwrap();
    std::fs::write(s.home.join("Context/passport.md"), "beside").unwrap();
    std::fs::write(s.home.join("notes.md"), "beside").unwrap();
    let cv = s.home.join("Context/career/cv.md").display().to_string();
    let cat = || s.ctx(&["run", "in", "tab", &ta, &format!("cat '{cv}'")]).stdout;
    assert!(!cat().contains("my cv"));
    let career = s.home.join("Context/career").display().to_string();
    s.ctx(&["allow", "agent", &a, "to", "read", &career]).ok();
    assert!(cat().contains("my cv"));
    for beside in [s.home.join("Context/passport.md"), s.home.join("notes.md")] {
        let o = s.ctx(&["run", "in", "tab", &ta, &format!("cat '{}'", beside.display())]).stdout;
        assert!(!o.contains("beside"), "{}: {o}", beside.display());
    }
    // Made later beside it: closed too.
    std::fs::write(s.home.join("Context/later.md"), "beside").unwrap();
    let o = s.ctx(&["run", "in", "tab", &ta, &format!("cat '{}/Context/later.md'", s.home.display())]).stdout;
    assert!(!o.contains("beside"), "{o}");
    let o = s.ctx(&["run", "in", "tab", &ta, &format!("echo x > '{career}/new.md'")]);
    assert!(o.stdout.contains("ermission denied"), "read isn't write: {} [{}] [{}]", o.code, o.stdout, o.stderr);
    s.ctx(&["forbid", "agent", &a, "to", "read", &career]).ok();
    assert!(!cat().contains("my cv"));

    // An agent can't share what isn't its own, and can't make agents.
    // (What a command in a tab prints comes back as the tab's output.)
    let o = in_a(&format!("allow agent {a} to read '{career}'"));
    assert!(o.code != 0 && o.stdout.contains("only"), "{} {}: {}", o.code, o.stdout, o.stderr);
    let o = in_a("make agent named sneaky");
    assert!(o.code != 0, "{}", o.stdout);

    // ctx-agent.toml: yours; the agent can't change it, and a replaced one isn't believed.
    let o = s.ctx(&["run", "in", "tab", &ta, "echo 'colour = \"red\"' >> ctx-agent.toml"]);
    assert!(o.stdout.contains("ermission denied"), "{} [{}] [{}]", o.code, o.stdout, o.stderr);
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
    let shared = homes.join("Shared").join({
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
    assert!(std::fs::symlink_metadata(agents.join(&a)).is_err(), "its link goes with its home");
    assert!(agents.join(&b).exists(), "a kept home keeps its link");
}
