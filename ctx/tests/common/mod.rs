//! A private ctx server for each test: its own CTX_HOME and HOME (so no real tabs, config or
//! dotfiles are touched), started directly (tests may; the real CLI never does).

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub struct Server {
    pub dir: PathBuf,
    pub home: PathBuf,
    child: Option<Child>,
}

pub struct Out {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
}

impl Out {
    pub fn ok(&self) -> &Self {
        assert_eq!(self.code, 0, "stdout: {}\nstderr: {}", self.stdout, self.stderr);
        self
    }
}

pub fn ctx_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ctx"))
}

/// `ctx-launch` must sit next to `ctx`; build it if this test run didn't.
fn ensure_launcher() {
    let launch = ctx_bin().with_file_name("ctx-launch");
    if !launch.exists() {
        let status = Command::new(env!("CARGO")).args(["build", "-p", "ctx-launch"]).status().expect("cargo build");
        assert!(status.success());
    }
}

impl Server {
    pub fn start(name: &str) -> Server {
        ensure_launcher();
        let dir = std::env::temp_dir().join(format!("ctx-it-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let home = dir.join("home");
        std::fs::create_dir_all(&home).unwrap();
        // ctx's local shell integration is for zsh; CI runners log in with bash.
        std::fs::write(dir.join("config.toml"), "shell = \"/bin/zsh\"\n").unwrap();
        let mut s = Server { dir, home, child: None };
        s.spawn();
        s
    }

    fn envs(&self, cmd: &mut Command) {
        cmd.env("CTX_HOME", &self.dir)
            .env("HOME", &self.home)
            .env("CTX_ASSIST", "off")
            .env("CTX_AGENT_NAME", "test agent")
            .env_remove("CTX_TAB")
            .env_remove("ZDOTDIR")
            .env_remove("CTX_WINDOW");
    }

    pub fn spawn(&mut self) {
        let mut cmd = Command::new(ctx_bin());
        cmd.arg("server")
            .stdout(Stdio::null())
            .stderr(Stdio::from(std::fs::File::create(self.dir.join("server.log")).unwrap()));
        self.envs(&mut cmd);
        self.child = Some(cmd.spawn().expect("start server"));
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(10) {
            if self.ctx(&["status"]).code == 0 {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("server didn't start: {}", std::fs::read_to_string(self.dir.join("server.log")).unwrap_or_default());
    }

    /// Kill the server without letting it clean up (a crash).
    pub fn kill_hard(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        // The re-exec'd server keeps the child's pid, so the kill above reached it.
        std::thread::sleep(Duration::from_millis(300));
    }

    pub fn ctx(&self, args: &[&str]) -> Out {
        self.ctx_in(args, None)
    }

    pub fn ctx_in(&self, args: &[&str], stdin: Option<&str>) -> Out {
        let mut cmd = Command::new(ctx_bin());
        cmd.args(args)
            .current_dir(&self.home)
            .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        self.envs(&mut cmd);
        let mut child = cmd.spawn().expect("run ctx");
        if let Some(input) = stdin {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
        }
        let o = child.wait_with_output().unwrap();
        Out {
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
            code: o.status.code().unwrap_or(-1),
        }
    }

    pub fn path(&self, p: &str) -> PathBuf {
        self.dir.join(p)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take() {
            // A polite stop: tabs are marked interrupted and shells get SIGHUP.
            unsafe_kill(c.id() as i32);
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(5) {
                if let Ok(Some(_)) = c.try_wait() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            let _ = c.kill();
            let _ = c.wait();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn unsafe_kill(pid: i32) {
    let _ = Command::new("/bin/kill").args(["-TERM", &pid.to_string()]).status();
}

pub fn exists(p: &Path) -> bool {
    p.exists()
}
