//! `ctx-helper`: starts an agent's tab as the agent's own macOS account. See
//! `ctx-protocol/src/helper.rs` for the rules and the wire format.
//!
//! Installed as a launchd daemon (root). It listens on one socket, answers only the account in
//! its root-owned `helper.toml`, and for each request: checks the agent and account against that
//! file (and an administrator's approval for root), forks, and in the child takes the attached
//! terminal, becomes the account (groups, gid, uid, in that order, checked), applies the agent's
//! sandbox and runs the program. The parent replies with the pid and goes back to listening.
//!
//! Usage: `ctx-helper` (by launchd). With `CTX_HELPER_SOCKET` and `CTX_HELPER_CONFIG` it can run
//! unprivileged for tests, where the only account it can become is its own.

use ctx_protocol::helper::{HelperConfig, SpawnReply, SpawnRequest, config_path, socket};
use std::ffi::{CStr, CString};
use std::io::{BufRead, BufReader, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};

unsafe extern "C" {
    fn sandbox_init(profile: *const libc::c_char, flags: u64, errorbuf: *mut *mut libc::c_char) -> libc::c_int;
}

/// macOS Authorization, enough to check (and use up) an administrator's approval.
mod auth {
    #[repr(C)]
    struct Item {
        name: *const libc::c_char,
        value_length: usize,
        value: *mut libc::c_void,
        flags: u32,
    }
    #[repr(C)]
    struct Rights {
        count: u32,
        items: *mut Item,
    }
    #[link(name = "Security", kind = "framework")]
    unsafe extern "C" {
        fn AuthorizationCreateFromExternalForm(ext: *const [u8; 32], auth: *mut *mut libc::c_void) -> i32;
        fn AuthorizationCopyRights(
            auth: *mut libc::c_void,
            rights: *const Rights,
            env: *const libc::c_void,
            flags: u32,
            out: *mut *mut Rights,
        ) -> i32;
        fn AuthorizationFree(auth: *mut libc::c_void, flags: u32) -> i32;
    }
    const EXTEND_RIGHTS: u32 = 1 << 1;
    const DESTROY_RIGHTS: u32 = 1 << 3;

    /// Whether `ext` holds `system.privilege.admin` already granted (no new prompt). The rights
    /// are destroyed afterwards, so each approval starts one agent.
    pub fn admin_approved(ext: &[u8]) -> bool {
        let Ok(ext): Result<[u8; 32], _> = ext.try_into() else { return false };
        let mut a: *mut libc::c_void = std::ptr::null_mut();
        // SAFETY: a 32-byte external form and an out-pointer.
        if unsafe { AuthorizationCreateFromExternalForm(&ext, &mut a) } != 0 {
            return false;
        }
        let mut item =
            Item { name: c"system.privilege.admin".as_ptr(), value_length: 0, value: std::ptr::null_mut(), flags: 0 };
        let rights = Rights { count: 1, items: &mut item };
        // SAFETY: valid rights for a live authorization; no interaction is allowed here.
        let ok =
            unsafe { AuthorizationCopyRights(a, &rights, std::ptr::null(), EXTEND_RIGHTS, std::ptr::null_mut()) } == 0;
        // SAFETY: frees the authorization we created.
        unsafe { AuthorizationFree(a, DESTROY_RIGHTS) };
        ok
    }
}

#[derive(Debug)]
struct Account {
    name: CString,
    uid: libc::uid_t,
    gid: libc::gid_t,
    home: String,
    shell: String,
}

fn account(name: &str) -> Option<Account> {
    let n = CString::new(name).ok()?;
    // SAFETY: getpwnam returns static storage or null; copied out at once.
    unsafe {
        let pw = libc::getpwnam(n.as_ptr());
        if pw.is_null() {
            return None;
        }
        let s = |p: *const libc::c_char| {
            if p.is_null() { String::new() } else { CStr::from_ptr(p).to_string_lossy().into_owned() }
        };
        Some(Account { name: n, uid: (*pw).pw_uid, gid: (*pw).pw_gid, home: s((*pw).pw_dir), shell: s((*pw).pw_shell) })
    }
}

fn account_name(uid: libc::uid_t) -> Option<String> {
    // SAFETY: getpwuid returns static storage or null; copied out at once.
    unsafe {
        let pw = libc::getpwuid(uid);
        if pw.is_null() { None } else { Some(CStr::from_ptr((*pw).pw_name).to_string_lossy().into_owned()) }
    }
}

/// One JSON line and the descriptor attached to it.
fn receive(stream: &UnixStream) -> Option<(Vec<u8>, OwnedFd)> {
    let mut buf = vec![0u8; 1 << 20];
    let mut cmsg = [0u8; 64];
    let mut iov = libc::iovec { iov_base: buf.as_mut_ptr().cast(), iov_len: buf.len() };
    // SAFETY: msghdr points at live buffers for the duration of recvmsg.
    let n = unsafe {
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = cmsg.as_mut_ptr().cast();
        msg.msg_controllen = cmsg.len() as _;
        let n = libc::recvmsg(stream.as_raw_fd(), &mut msg, 0);
        if n <= 0 {
            return None;
        }
        let c = libc::CMSG_FIRSTHDR(&msg);
        if c.is_null() || (*c).cmsg_level != libc::SOL_SOCKET || (*c).cmsg_type != libc::SCM_RIGHTS {
            return None;
        }
        let fd = *(libc::CMSG_DATA(c) as *const RawFd);
        (n as usize, OwnedFd::from_raw_fd(fd))
    };
    let (len, fd) = n;
    buf.truncate(len);
    // The rest of the line, if it didn't come in one piece.
    let mut reader = BufReader::new(stream);
    if !buf.ends_with(b"\n") {
        let mut more = Vec::new();
        reader.read_until(b'\n', &mut more).ok()?;
        buf.extend(more);
    }
    Some((buf, fd))
}

fn reply(mut stream: &UnixStream, r: &SpawnReply) {
    if let Ok(mut s) = serde_json::to_vec(r) {
        s.push(b'\n');
        let _ = stream.write_all(&s);
    }
}

/// Check a request against the rules. `Ok` gives the account to become.
fn check(req: &SpawnRequest, rules: &HelperConfig) -> Result<Account, String> {
    let Some(want) = rules.accounts.get(&req.agent) else {
        return Err(format!(
            "The helper's rules ({}) don't have the agent \"{}\".",
            config_path().display(),
            req.agent
        ));
    };
    if *want != req.account {
        return Err(format!(
            "The helper's rules say the agent \"{}\" runs as \"{want}\", not \"{}\".",
            req.agent, req.account
        ));
    }
    let a = account(want).ok_or_else(|| format!("There's no account \"{want}\" on this Mac."))?;
    if a.uid == 0 && !req.authorization.as_deref().is_some_and(auth::admin_approved) {
        return Err(format!(
            "The agent \"{}\" runs as root, and an administrator didn't approve this start.",
            req.agent
        ));
    }
    if req.program.is_empty() || req.argv.is_empty() {
        return Err("No program to start.".into());
    }
    Ok(a)
}

/// The child: take the terminal, become the account, apply the sandbox, run the program.
/// Only returns (with a message) if something failed; the caller exits.
fn become_and_exec(req: &SpawnRequest, a: &Account, tty: RawFd) -> String {
    // SAFETY: plain system calls in a freshly forked, single-threaded child.
    unsafe {
        libc::signal(libc::SIGCHLD, libc::SIG_DFL);
        libc::setsid();
        if libc::ioctl(tty, libc::TIOCSCTTY as _, 0) != 0 {
            return "couldn't take the tab's terminal".into();
        }
        for target in 0..3 {
            libc::dup2(tty, target);
        }
        if tty > 2 {
            libc::close(tty);
        }
        // (Already that account: an unprivileged test run.)
        if libc::getuid() != a.uid
            && (libc::initgroups(a.name.as_ptr(), a.gid as _) != 0
                || libc::setgid(a.gid) != 0
                || libc::setuid(a.uid) != 0)
        {
            return format!("couldn't become {}", a.name.to_string_lossy());
        }
        // No way back: a uid that could return to ours would be no boundary.
        if libc::getuid() != a.uid || libc::geteuid() != a.uid || (a.uid != 0 && libc::setuid(0) == 0) {
            return "the account switch didn't hold".into();
        }
    }
    let mut env: Vec<(String, String)> = req
        .env
        .iter()
        .filter(|(k, _)| {
            !k.starts_with("DYLD_") && !matches!(k.as_str(), "HOME" | "USER" | "LOGNAME" | "SHELL" | "TMPDIR")
        })
        .cloned()
        .collect();
    let user = a.name.to_string_lossy().into_owned();
    env.push(("HOME".into(), a.home.clone()));
    env.push(("USER".into(), user.clone()));
    env.push(("LOGNAME".into(), user));
    env.push(("SHELL".into(), if a.shell.is_empty() { "/bin/zsh".into() } else { a.shell.clone() }));
    let mut tmp = [0u8; 1024];
    // SAFETY: confstr writes at most the buffer's length.
    let n = unsafe { libc::confstr(libc::_CS_DARWIN_USER_TEMP_DIR, tmp.as_mut_ptr().cast(), tmp.len()) };
    if n > 0 && n <= tmp.len() {
        env.push(("TMPDIR".into(), String::from_utf8_lossy(&tmp[..n - 1]).into_owned()));
    }
    let folder = CString::new(req.folder.clone()).unwrap_or_default();
    let home = CString::new(a.home.clone()).unwrap_or_default();
    // SAFETY: valid C strings.
    unsafe {
        if libc::chdir(folder.as_ptr()) != 0 {
            libc::chdir(home.as_ptr());
        }
    }
    if !req.sandbox.is_empty() {
        let Ok(profile) = CString::new(req.sandbox.clone()) else { return "the sandbox has a NUL byte".into() };
        let mut err: *mut libc::c_char = std::ptr::null_mut();
        // SAFETY: a valid C string and an out-pointer.
        if unsafe { sandbox_init(profile.as_ptr(), 0, &mut err) } != 0 {
            return "couldn't apply the agent's sandbox".into();
        }
    }
    use std::os::unix::process::CommandExt;
    let e =
        std::process::Command::new(&req.program).arg0(&req.argv[0]).args(&req.argv[1..]).env_clear().envs(env).exec();
    format!("couldn't start {}: {e}", req.program)
}

fn serve_one(stream: UnixStream, rules: &HelperConfig, owner_uid: libc::uid_t) {
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    // SAFETY: getpeereid with valid out-pointers.
    if unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } != 0 || uid != owner_uid {
        return reply(&stream, &SpawnReply::Refused { message: "Only the ctx server's account may ask.".into() });
    }
    let Some((line, tty)) = receive(&stream) else {
        return reply(
            &stream,
            &SpawnReply::Refused { message: "The request needs the tab's terminal attached.".into() },
        );
    };
    let req: SpawnRequest = match serde_json::from_slice(&line) {
        Ok(r) => r,
        Err(e) => {
            return reply(
                &stream,
                &SpawnReply::Refused { message: format!("The helper couldn't read that request: {e}") },
            );
        }
    };
    let a = match check(&req, rules) {
        Ok(a) => a,
        Err(message) => return reply(&stream, &SpawnReply::Refused { message }),
    };
    // SAFETY: fork in this single-threaded program; the child execs or exits.
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return reply(&stream, &SpawnReply::Refused { message: "couldn't start a process (fork failed)".into() });
    }
    if pid == 0 {
        drop(stream);
        let msg = become_and_exec(&req, &a, tty.as_raw_fd());
        let _ = writeln!(std::io::stderr(), "ctx-helper: {msg}");
        // SAFETY: leave the child without running the parent's destructors.
        unsafe { libc::_exit(127) };
    }
    drop(tty);
    reply(&stream, &SpawnReply::Started { pid });
}

fn main() {
    let rules =
        match std::fs::read_to_string(config_path()).map_err(|e| e.to_string()).and_then(|t| HelperConfig::parse(&t)) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("ctx-helper: {}: {e}", config_path().display());
                std::process::exit(1);
            }
        };
    let Some(owner) = account(&rules.owner) else {
        eprintln!("ctx-helper: there's no account \"{}\"", rules.owner);
        std::process::exit(1);
    };
    // SAFETY: getuid can't fail.
    let me = unsafe { libc::getuid() };
    if me == 0 && rules.accounts.values().any(|a| *a == rules.owner) {
        eprintln!("ctx-helper: an agent can't run as the owner's own account ({}); fix the rules", rules.owner);
        std::process::exit(1);
    }
    if me != 0 && rules.accounts.values().any(|a| account(a).is_none_or(|x| x.uid != me)) {
        eprintln!(
            "ctx-helper: running as {} (not root), so it can only start processes as itself; check the rules",
            account_name(me).unwrap_or_default()
        );
    }
    let path = socket();
    let _ = std::fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("ctx-helper: couldn't listen on {}: {e}", path.display());
            std::process::exit(1);
        }
    };
    // Anyone may connect; only the owner gets an answer other than no.
    let _ = std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o666));
    // Children are reaped by the system; each child puts SIGCHLD back before it execs.
    // SAFETY: setting a signal disposition.
    unsafe { libc::signal(libc::SIGCHLD, libc::SIG_IGN) };
    for stream in listener.incoming().flatten() {
        serve_one(stream, &rules, owner.uid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_checked_against_the_rules() {
        let me = account_name(unsafe { libc::getuid() }).unwrap();
        let rules =
            HelperConfig::parse(&format!("owner = \"nobody\"\n[accounts]\ndev = \"{me}\"\nsysadmin = \"root\"\n"))
                .unwrap();
        let req = |agent: &str, account: &str| SpawnRequest {
            agent: agent.into(),
            account: account.into(),
            folder: "/tmp".into(),
            program: "/bin/sh".into(),
            argv: vec!["-sh".into()],
            env: vec![],
            sandbox: String::new(),
            authorization: None,
        };
        assert!(check(&req("dev", &me), &rules).is_ok());
        assert!(check(&req("dev", "root"), &rules).unwrap_err().contains("runs as"));
        assert!(check(&req("email", &me), &rules).unwrap_err().contains("don't have the agent"));
        let root = check(&req("sysadmin", "root"), &rules).err().unwrap();
        assert!(root.contains("administrator didn't approve"), "{root}");
        let mut fake = req("sysadmin", "root");
        fake.authorization = Some(vec![0; 32]);
        assert!(check(&fake, &rules).is_err(), "a made-up authorization isn't an approval");
    }
}
