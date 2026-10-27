//! `ctx-launch`: starts one tab's shell.
//!
//! The ctx server spawns this program with macOS "responsibility" disclaimed, so it is its own
//! responsible process: nothing CTX.app or the server was granted (and nothing the terminal that
//! started the server holds) reaches the tab. Its standard input, output and error are the tab's
//! pseudo-terminal. It forks; the child starts a new session, takes the terminal as its
//! controlling terminal, moves to the tab's folder and becomes the shell. The parent stays as the
//! shell's parent (so the shell keeps this process as its responsible process), holds no
//! terminal, and exits with the shell's exit status.
//!
//! An agent's tab starts with `--sandbox <profile file>`: the Seatbelt profile is applied before
//! anything else runs, so the shell and everything it starts stay inside it. If it can't be
//! applied the shell doesn't start.
//!
//! Usage (by the server only): `ctx-launch [--sandbox <file>] <folder> <program> <argv0> [arguments…]`

use std::ffi::{CString, OsStr};
use std::os::unix::ffi::OsStrExt;

fn cstring(s: &OsStr) -> CString {
    CString::new(s.as_bytes()).unwrap_or_else(|_| CString::new("?").unwrap())
}

fn fail(msg: &str) -> ! {
    eprintln!("ctx-launch: {msg}");
    std::process::exit(127);
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn sandbox_init(profile: *const libc::c_char, flags: u64, errorbuf: *mut *mut libc::c_char) -> libc::c_int;
}

/// Apply a Seatbelt profile (its text) to this process and everything it starts.
#[cfg(target_os = "macos")]
fn apply_sandbox(file: &OsStr) {
    let text = std::fs::read(file).unwrap_or_else(|e| fail(&format!("couldn't read the agent's sandbox: {e}")));
    let profile = CString::new(text).unwrap_or_else(|_| fail("the agent's sandbox has a NUL byte"));
    let mut err: *mut libc::c_char = std::ptr::null_mut();
    // SAFETY: a valid C string and an out-pointer for the error message.
    let r = unsafe { sandbox_init(profile.as_ptr(), 0, &mut err) };
    if r != 0 {
        let msg = if err.is_null() {
            "unknown error".to_string()
        } else {
            // SAFETY: sandbox_init sets a C string on failure.
            unsafe { std::ffi::CStr::from_ptr(err).to_string_lossy().into_owned() }
        };
        fail(&format!("couldn't apply the agent's sandbox, so the shell didn't start: {msg}"));
    }
}

/// The agent sandbox is Seatbelt, so on other systems a sandboxed tab doesn't start.
#[cfg(not(target_os = "macos"))]
fn apply_sandbox(_file: &OsStr) {
    fail("the agent's sandbox is macOS only, so the shell didn't start");
}

fn main() {
    let mut args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    if args.len() >= 2 && args[0] == "--sandbox" {
        apply_sandbox(&args[1]);
        args.drain(..2);
    }
    if args.len() < 3 {
        fail("usage: ctx-launch <folder> <program> <argv0> [arguments…] (started by the ctx server)");
    }
    let folder = cstring(&args[0]);
    let program = cstring(&args[1]);
    let argv: Vec<CString> = args[2..].iter().map(|a| cstring(a)).collect();
    let mut argv_ptrs: Vec<*const libc::c_char> = argv.iter().map(|a| a.as_ptr()).collect();
    argv_ptrs.push(std::ptr::null());
    let home = std::env::var_os("HOME").map(|h| cstring(&h));

    // SAFETY: fork in a single-threaded program; the child only calls async-signal-safe
    // functions (setsid, ioctl, chdir, signal, execv, _exit) before exec.
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        fail("couldn't start the shell (fork failed)");
    }
    if pid == 0 {
        // SAFETY: async-signal-safe calls only, on valid pointers that outlive them.
        unsafe {
            libc::setsid();
            libc::ioctl(0, libc::TIOCSCTTY as _, 0);
            if libc::chdir(folder.as_ptr()) != 0
                && let Some(h) = &home
            {
                libc::chdir(h.as_ptr());
            }
            for sig in [
                libc::SIGINT,
                libc::SIGQUIT,
                libc::SIGHUP,
                libc::SIGTERM,
                libc::SIGPIPE,
                libc::SIGCHLD,
                libc::SIGTSTP,
                libc::SIGTTIN,
                libc::SIGTTOU,
            ] {
                libc::signal(sig, libc::SIG_DFL);
            }
            let mut empty: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut empty);
            libc::sigprocmask(libc::SIG_SETMASK, &empty, std::ptr::null_mut());
            libc::execv(program.as_ptr(), argv_ptrs.as_ptr());
            let msg = b"ctx-launch: couldn't start the shell\n";
            libc::write(2, msg.as_ptr().cast(), msg.len());
            libc::_exit(127);
        }
    }

    // Parent: let go of the terminal so the tab ends when the shell does, then wait.
    // SAFETY: plain syscalls on our own descriptors and child.
    unsafe {
        let null = libc::open(c"/dev/null".as_ptr(), libc::O_RDWR);
        if null >= 0 {
            libc::dup2(null, 0);
            libc::dup2(null, 1);
            libc::dup2(null, 2);
            if null > 2 {
                libc::close(null);
            }
        }
        libc::signal(libc::SIGINT, libc::SIG_IGN);
        libc::signal(libc::SIGQUIT, libc::SIG_IGN);
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
        libc::signal(libc::SIGTSTP, libc::SIG_IGN);
        loop {
            let mut status = 0;
            let r = libc::waitpid(pid, &mut status, 0);
            if r < 0 {
                if std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                    continue;
                }
                libc::_exit(1);
            }
            if libc::WIFEXITED(status) {
                libc::_exit(libc::WEXITSTATUS(status));
            }
            if libc::WIFSIGNALED(status) {
                libc::_exit(128 + libc::WTERMSIG(status));
            }
        }
    }
}
