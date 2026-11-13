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
    spaces: Vec<String>,
    /// root's folder for the test's spaces and ctx, removed at the end.
    sys: PathBuf,
}

impl Drop for RootHelper {
    fn drop(&mut self) {
        for a in &self.accounts {
            if cfg!(target_os = "macos") {
                let _ = Command::new("sudo").args(["-n", "dscl", ".", "-delete", &format!("/Users/{a}")]).status();
                let _ = Command::new("sudo").args(["-n", "rm", "-rf", &format!("/Users/{a}")]).status();
            } else {
                let _ = Command::new("sudo").args(["-n", "userdel", a]).status();
                let _ = Command::new("sudo").args(["-n", "rm", "-rf", &format!("/home/{a}")]).status();
            }
        }
        for g in &self.spaces {
            let g = format!("ctx-space-{g}");
            if cfg!(target_os = "macos") {
                let _ = Command::new("sudo").args(["-n", "dseditgroup", "-o", "delete", &g]).status();
            } else {
                let _ = Command::new("sudo").args(["-n", "groupdel", &g]).status();
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
    // Agents' homes are where every account's is; spaces go in a folder of root's for the test
    // (as /Users/Shared and /srv are root's).
    let sys = PathBuf::from(format!("/tmp/ctx-real-sys-{tag}"));
    assert!(Command::new("sudo").args(["-n", "mkdir", "-p", "-m", "755"]).arg(&sys).status().unwrap().success());
    let spaces = sys.join("spaces");
    let spaces_env = format!("CTX_SPACES_DIR={}", spaces.display());
    let home_of =
        |n: &str| PathBuf::from(if cfg!(target_os = "macos") { format!("/Users/{n}") } else { format!("/home/{n}") });
    let space = format!("t{tag}");
    let rules = dir.join("helper.toml");
    std::fs::write(&rules, format!("owner = \"{}\"\n", me())).unwrap();
    let child = Command::new("sudo")
        .args([
            "-n",
            "env",
            &format!("CTX_HELPER_SOCKET={sock}"),
            &format!("CTX_HELPER_CONFIG={}", rules.display()),
            &spaces_env,
        ])
        .arg(helper_bin())
        .stdout(Stdio::null())
        .stderr(Stdio::from(std::fs::File::create(dir.join("helper.log")).unwrap()))
        .spawn()
        .unwrap();
    let _h = RootHelper { child, accounts: vec![a.clone(), b.clone()], spaces: vec![space.clone()], sys: sys.clone() };
    assert!(wait_for(|| Path::new(&sock).exists()), "the helper didn't start");
    let helper_log = dir.join("helper.log");
    // What the helper said, when a step fails.
    let log = || std::fs::read_to_string(&helper_log).unwrap_or_default();

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
    let s = Server::start_with("real", &[("CTX_HELPER_SOCKET", &sock), ("CTX_SPACES_DIR", spaces.to_str().unwrap())]);
    // A home other accounts can enter is reported; a real home is private (macOS 700, Linux 750).
    std::fs::set_permissions(&s.home, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let o = s.ctx(&["make", "agent", "named", &a]);
    assert!(o.code == 0 && o.stdout.contains("lets every account in"), "{} {}\nhelper: {}", o.stdout, o.stderr, log());
    s.ctx(&["remove", "agent", &a, "with", "its", "files"]).ok();
    std::fs::set_permissions(&s.home, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
    let agents = s.home.join("Agents");
    let ctx_path = ctx_copy.display().to_string();

    // Making agents: accounts, private homes where every account's is, with the starter files
    // (~/Agents links to them).
    let o = s.ctx(&["make", "agent", "named", &a]).ok().stdout.clone();
    assert!(o.contains(&format!("Made the agent \"{a}\"")) && o.contains("ctx switch to agent"), "{o}");
    assert!(!o.contains("lets every account in"), "{o}");
    s.ctx(&["make", "agent", "named", &b]).ok();
    for f in ["CLAUDE.md", "AGENTS.md", "ctx-agent.toml"] {
        assert!(home_of(&a).join(f).exists(), "{f}");
    }
    assert_eq!(std::fs::read_link(agents.join(&a)).unwrap(), home_of(&a));
    let o = s.ctx(&["show", "agent", &a]).ok().stdout.clone();
    assert!(o.contains("you may read and write its home"), "{o}");

    // Switching: a tab as the account, in its home.
    let ta = opened(&s.ctx(&["switch", "to", "agent", &a]).ok().stdout);
    let tb = opened(&s.ctx(&["switch", "to", "agent", &b]).ok().stdout);
    s.ctx(&["open", "tab", "named", "Mine", "at", "/tmp"]).ok();
    let o = s.ctx(&["run", "in", "tab", &ta, "id -un; pwd; echo $CTX_SPACES"]).ok().stdout.clone();
    let lines: Vec<&str> = o.lines().collect();
    assert_eq!(lines.first(), Some(&a.as_str()), "{o}");
    assert_eq!(lines.get(1).copied(), Some(home_of(&a).to_str().unwrap()), "{o}");
    assert_eq!(lines.get(2).copied(), Some(spaces.to_str().unwrap()), "{o}");
    // Homes are private: b can't look in a's.
    let o = s.ctx(&["run", "in", "tab", &tb, "id -un"]).ok().stdout.clone();
    assert_eq!(o.trim(), b, "b's tab runs as b");
    let o = s.ctx(&["run", "in", "tab", &tb, &format!("cat '{}/CLAUDE.md'", home_of(&a).display())]).stdout;
    let why = || {
        let h = home_of(&a);
        let acl = if cfg!(target_os = "macos") {
            format!("ls -lde '{}'", h.display())
        } else {
            format!("ls -ld '{0}'; getfacl '{0}'; id", h.display())
        };
        String::from_utf8_lossy(&Command::new("sh").args(["-c", &format!("{acl}; id {b}")]).output().unwrap().stdout)
            .into_owned()
    };
    assert!(o.contains("ermission denied"), "{o}\n{}", why());
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

    // Nothing more through ctx: no restart, no copy of your tab, nothing about tabs it can't see,
    // and a copy you make of its tab stays its own (its exported variables could be planted).
    let o = in_a("restart server without asking");
    assert!(o.code != 0 && o.stdout.contains("only you"), "{} {}", o.code, o.stdout);
    assert!(in_a("reopen tabs from last session").code != 0);
    assert!(in_a("open tab like Mine").code != 0, "a copy of your tab would be yours");
    let mine = s.ctx(&["list", "tabs"]).ok().stdout.clone();
    let mine = mine.lines().find(|l| l.contains("Mine")).and_then(|l| l.split_whitespace().nth(1)).unwrap().to_string();
    let o = in_a(&format!("run in tab {mine} 'echo hi'")).stdout;
    assert!(o.contains(&format!("There is no tab {mine}")) && !o.contains("Open tabs are"), "{o}");
    s.ctx(&["run", "in", "tab", &ta, "export PLANTED=1"]).ok();
    s.ctx(&["open", "tab", "like", "tab", &ta, "named", "My copy"]).ok();
    let o = s.ctx(&["list", "tabs"]).ok().stdout.clone();
    assert!(o.lines().any(|l| l.contains("My copy") && l.contains(&format!("as {a}"))), "{o}");
    let o = s.ctx(&["run", "in", "My copy", "id -un; echo $PLANTED"]).ok().stdout.clone();
    assert_eq!(o, format!("{a}\n1\n"));
    s.ctx(&["close", "My copy", "without", "asking"]).ok();

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

    // Spaces: a group and its folder. Only agents in it (and you) may use it; agents can't make
    // spaces or add themselves.
    let o = s.ctx(&["make", "space", "named", &space]);
    assert_eq!(o.code, 0, "{}\nhelper: {}", o.stderr, log());
    let folder = spaces.join(&space);
    assert!(folder.is_dir());
    assert!(in_a(&format!("make space named {space}x")).code != 0);
    assert!(in_a(&format!("add agent {a} to space {space}")).code != 0);
    for who in [&a, &b] {
        s.ctx(&["add", "agent", who, "to", "space", &space]).ok();
    }
    let o = s.ctx(&["show", "agent", &a]).ok().stdout.clone();
    assert!(o.contains(&format!("In the spaces: {space}")), "{o}");
    // Group membership counts in tabs started after it.
    let ta2 = opened(&s.ctx(&["switch", "to", "agent", &a]).ok().stdout);
    let tb2 = opened(&s.ctx(&["switch", "to", "agent", &b]).ok().stdout);
    let o = s.ctx(&[
        "run",
        "in",
        "tab",
        &tb2,
        &format!("echo hi > '{}/note' && cat '{}/note'", folder.display(), folder.display()),
    ]);
    assert_eq!(o.ok().stdout, "hi\n");
    let o = s.ctx(&[
        "run",
        "in",
        "tab",
        &ta2,
        &format!("echo more >> '{}/note' && cat '{}/note'", folder.display(), folder.display()),
    ]);
    assert_eq!(o.ok().stdout, "hi\nmore\n", "an agent in the space writes what another made");
    assert_eq!(std::fs::read_to_string(folder.join("note")).unwrap(), "hi\nmore\n", "you may read it");
    s.ctx(&["take", "agent", &a, "out", "of", "space", &space]).ok();
    let ta3 = opened(&s.ctx(&["switch", "to", "agent", &a]).ok().stdout);
    let o = s.ctx(&["run", "in", "tab", &ta3, &format!("cat '{}/note'", folder.display())]).stdout;
    assert!(o.contains("ermission denied"), "out of the space: {o}");
    let o = s.ctx(&["list", "spaces"]).ok().stdout.clone();
    assert!(o.contains(&space) && o.contains(&b) && !o.contains(&a), "{o}");
    s.ctx(&["remove", "space", &space, "with", "its", "files"]).ok();
    assert!(!folder.exists());

    // Removing: accounts go, homes to the Trash with `with its files`.
    s.ctx(&["remove", "agent", &a, "with", "its", "files"]).ok();
    s.ctx(&["remove", "agent", &b]).ok();
    assert!(Command::new("id").arg(&a).output().unwrap().stdout.is_empty());
    assert!(!home_of(&a).exists(), "its home is in the Trash");
    assert!(std::fs::symlink_metadata(agents.join(&a)).is_err(), "its link goes with its home");
    assert!(home_of(&b).exists() && agents.join(&b).exists(), "a kept home keeps its link");
}
