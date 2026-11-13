//! `ctx-helper`: starts an agent's tab as the agent's own account, and makes, adopts and removes
//! agents' accounts. See `ctx-protocol/src/helper.rs` for the rules and the wire format.
//!
//! Installed as a launchd daemon (root). It listens on one socket, answers only the account in
//! its root-owned `helper.toml`, and for each request: checks the agent and account against that
//! file (and an administrator's approval for root), forks, and in the child takes the attached
//! terminal, becomes the account (groups, gid, uid, in that order, checked), applies the agent's
//! sandbox and runs the program. The parent replies with the pid and goes back to listening.
//!
//! Usage: `ctx-helper` (by launchd). With `CTX_HELPER_SOCKET` and `CTX_HELPER_CONFIG` it can run
//! unprivileged for tests, where the only account it can become is its own.

use ctx_accounts::{accounts, acl};
use ctx_protocol::helper::{
    EntryFor, Grant, HELPER_VERSION, HelperConfig, HelperReply, HelperRequest, SpawnReply, SpawnRequest, StarterFile,
    config_path, socket,
};
use std::ffi::{CStr, CString};
use std::io::{BufRead, BufReader, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn sandbox_check(pid: libc::pid_t, operation: *const libc::c_char, filter: libc::c_int, ...) -> libc::c_int;
    fn sandbox_init(profile: *const libc::c_char, flags: u64, errorbuf: *mut *mut libc::c_char) -> libc::c_int;
    fn setiopolicy_np(iotype: libc::c_int, scope: libc::c_int, policy: libc::c_int) -> libc::c_int;
}

/// Linux has no administrator approval of this kind; root agents are refused there.
#[cfg(not(target_os = "macos"))]
mod auth {
    pub fn admin_approved(_ext: &[u8]) -> bool {
        false
    }
}

/// macOS Authorization, enough to check (and use up) an administrator's approval.
#[cfg(target_os = "macos")]
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

/// One JSON line and the descriptor attached to it, if any.
fn receive(stream: &UnixStream) -> Option<(Vec<u8>, Option<OwnedFd>)> {
    let mut buf = vec![0u8; 1 << 20];
    let mut cmsg = [0u8; 64];
    let mut iov = libc::iovec { iov_base: buf.as_mut_ptr().cast(), iov_len: buf.len() };
    // SAFETY: msghdr points at live buffers for the duration of recvmsg.
    let (len, fd) = unsafe {
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
        let fd = if !c.is_null() && (*c).cmsg_level == libc::SOL_SOCKET && (*c).cmsg_type == libc::SCM_RIGHTS {
            Some(OwnedFd::from_raw_fd(*(libc::CMSG_DATA(c) as *const RawFd)))
        } else {
            None
        };
        (n as usize, fd)
    };
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

fn answer(mut stream: &UnixStream, r: &HelperReply) {
    if let Ok(mut s) = serde_json::to_vec(r) {
        s.push(b'\n');
        let _ = stream.write_all(&s);
    }
}

fn reply(mut stream: &UnixStream, r: &SpawnReply) {
    if let Ok(mut s) = serde_json::to_vec(r) {
        s.push(b'\n');
        let _ = stream.write_all(&s);
    }
}

/// Check a request against the rules. `Ok` gives the account to become.
fn check(req: &SpawnRequest, rules: &HelperConfig) -> Result<Account, String> {
    // An agent that is an account: its name is the account's, and the account is in ctx-agents,
    // standard, and not the owner's.
    if !rules.accounts.contains_key(&req.agent) && req.agent == req.account {
        let a = account(&req.account).ok_or_else(|| format!("There's no account \"{}\".", req.account))?;
        let info = accounts::by_name(&req.account).ok_or_else(|| format!("There's no account \"{}\".", req.account))?;
        if !accounts::is_agent(&info) {
            return Err(format!(
                "\"{}\" isn't an agent's account (it isn't in {}).",
                req.account,
                accounts::AGENTS_GROUP
            ));
        }
        if a.uid == 0 || accounts::is_admin(&req.account) || account(&rules.owner).is_some_and(|o| o.uid == a.uid) {
            return Err(format!(
                "\"{}\" is an administrator or the owner; agents' accounts are standard.",
                req.account
            ));
        }
        if req.program.is_empty() || req.argv.is_empty() {
            return Err("No program to start.".into());
        }
        return Ok(a);
    }
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
        // Limits, set while still root so the agent can't raise them back.
        if req.limits.processes > 0 {
            let l = libc::rlimit { rlim_cur: req.limits.processes as _, rlim_max: req.limits.processes as _ };
            libc::setrlimit(libc::RLIMIT_NPROC, &l);
        }
        if req.limits.low_priority {
            libc::setpriority(libc::PRIO_PROCESS, 0, 10);
            #[cfg(target_os = "macos")]
            // IOPOL_TYPE_DISK, IOPOL_SCOPE_PROCESS, IOPOL_THROTTLE.
            setiopolicy_np(0, 0, 3);
        }
        // Group-writable new files, so access-list masks on Linux keep write for shared folders.
        #[cfg(not(target_os = "macos"))]
        libc::umask(0o002);
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
    env.push(("TMPDIR".into(), user_temp()));
    let folder = CString::new(req.folder.clone()).unwrap_or_default();
    let home = CString::new(a.home.clone()).unwrap_or_default();
    // SAFETY: valid C strings.
    unsafe {
        if libc::chdir(folder.as_ptr()) != 0 {
            libc::chdir(home.as_ptr());
        }
    }
    if !req.sandbox.is_empty() {
        #[cfg(target_os = "macos")]
        {
            let Ok(profile) = CString::new(req.sandbox.clone()) else { return "the sandbox has a NUL byte".into() };
            let mut err: *mut libc::c_char = std::ptr::null_mut();
            // SAFETY: a valid C string and an out-pointer.
            if unsafe { sandbox_init(profile.as_ptr(), 0, &mut err) } != 0 {
                return "couldn't apply the agent's sandbox".into();
            }
        }
        #[cfg(not(target_os = "macos"))]
        return "the agent sandbox is macOS only".into();
    }
    use std::os::unix::process::CommandExt;
    let e =
        std::process::Command::new(&req.program).arg0(&req.argv[0]).args(&req.argv[1..]).env_clear().envs(env).exec();
    format!("couldn't start {}: {e}", req.program)
}

/// The account's own temporary folder (the helper has already become the account).
fn user_temp() -> String {
    #[cfg(target_os = "macos")]
    {
        let mut tmp = [0u8; 1024];
        // SAFETY: confstr writes at most the buffer's length.
        let n = unsafe { libc::confstr(libc::_CS_DARWIN_USER_TEMP_DIR, tmp.as_mut_ptr().cast(), tmp.len()) };
        if n > 0 && n <= tmp.len() {
            return String::from_utf8_lossy(&tmp[..n - 1]).into_owned();
        }
    }
    "/tmp".into()
}

/// The process on the other end of the socket (as it was when it connected).
#[cfg(target_os = "macos")]
fn peer_pid(fd: RawFd) -> Option<libc::pid_t> {
    const SOL_LOCAL: libc::c_int = 0;
    const LOCAL_PEERPID: libc::c_int = 2;
    let mut pid: libc::pid_t = 0;
    let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    // SAFETY: getsockopt with a pid-sized out-buffer.
    let r = unsafe { libc::getsockopt(fd, SOL_LOCAL, LOCAL_PEERPID, (&mut pid as *mut libc::pid_t).cast(), &mut len) };
    (r == 0 && pid > 0).then_some(pid)
}

/// Whether the caller is inside a Seatbelt sandbox. Agents that run as the owner's account do,
/// and must not reach the helper: they would start themselves as another account, outside every
/// sandbox, without the server. A sandbox can't be left, so a sandboxed caller stays one.
/// Unknown counts as sandboxed.
#[cfg(target_os = "macos")]
fn caller_sandboxed(fd: RawFd) -> bool {
    let Some(pid) = peer_pid(fd) else { return true };
    // SAFETY: a null operation asks only whether the process is sandboxed.
    unsafe { sandbox_check(pid, std::ptr::null(), 0) != 0 }
}

#[cfg(not(target_os = "macos"))]
fn caller_sandboxed(_fd: RawFd) -> bool {
    false
}

/// The account on the other end of the socket.
fn peer_uid(fd: RawFd) -> Option<libc::uid_t> {
    #[cfg(target_os = "macos")]
    {
        let (mut uid, mut gid) = (0, 0);
        // SAFETY: getpeereid with valid out-pointers.
        (unsafe { libc::getpeereid(fd, &mut uid, &mut gid) } == 0).then_some(uid)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: SO_PEERCRED fills a ucred.
        let r = unsafe {
            libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_PEERCRED, (&mut cred as *mut libc::ucred).cast(), &mut len)
        };
        (r == 0).then_some(cred.uid)
    }
}

fn serve_one(stream: UnixStream, rules: &HelperConfig, owner_uid: libc::uid_t) {
    if peer_uid(stream.as_raw_fd()) != Some(owner_uid) {
        return reply(&stream, &SpawnReply::Refused { message: "Only the ctx server's account may ask.".into() });
    }
    if caller_sandboxed(stream.as_raw_fd()) {
        return reply(
            &stream,
            &SpawnReply::Refused {
                message: "Processes in an agent's sandbox can't use the helper; the ctx server asks it.".into(),
            },
        );
    }
    let Some((line, fd)) = receive(&stream) else { return };
    if let Ok(req) = serde_json::from_slice::<HelperRequest>(&line) {
        return answer(&stream, &handle(req, owner_uid));
    }
    let Some(tty) = fd else {
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

// ---------------------------------------------------------------------------------------------
// Accounts
// ---------------------------------------------------------------------------------------------

fn handle(req: HelperRequest, owner: libc::uid_t) -> HelperReply {
    let r = match req {
        HelperRequest::Version => return HelperReply::Version { version: HELPER_VERSION },
        HelperRequest::MakeAgentHomes => make_agent_homes(owner),
        HelperRequest::MakeAccount { name, full_name, home, files } => {
            make_account(&name, &full_name, &home, &files, owner)
        }
        HelperRequest::AdoptAccount { name } => {
            accounts::adopt(&name).map(|a| format!("\"{}\" is an agent now.", a.name))
        }
        HelperRequest::RemoveAccount { name, trash_home } => remove_account(&name, trash_home, owner),
        HelperRequest::SetFullName { name, full_name } => agent(&name, owner)
            .and_then(|_| accounts::set_full_name(&name, &full_name))
            .map(|_| format!("\"{name}\" is called \"{full_name}\" now.")),
        HelperRequest::Share { path, entry, grant } => share(Path::new(&path), entry, grant, owner),
        HelperRequest::MayAccess { account, path, write } => {
            return match agent(&account, owner) {
                Ok(a) => HelperReply::Answer { yes: may_access(&a, &path, write) },
                Err(message) => HelperReply::Refused { message },
            };
        }
    };
    match r {
        Ok(message) => HelperReply::Done { message },
        Err(message) => HelperReply::Refused { message },
    }
}

/// An agent's account (in ctx-agents, standard, not the owner).
fn agent(name: &str, owner: libc::uid_t) -> Result<accounts::Account, String> {
    let a = accounts::by_name(name).ok_or_else(|| format!("There's no account \"{name}\"."))?;
    if !accounts::is_agent(&a) || a.uid == 0 || a.uid == owner || accounts::is_admin(name) {
        return Err(format!("\"{name}\" isn't an agent's account."));
    }
    Ok(a)
}

fn lstat(p: &Path) -> Option<std::fs::Metadata> {
    std::fs::symlink_metadata(p).ok()
}

/// A real folder (not a link) owned by `uid`.
fn owned_folder(p: &Path, uid: libc::uid_t) -> bool {
    use std::os::unix::fs::MetadataExt;
    lstat(p).is_some_and(|m| m.is_dir() && m.uid() == uid)
}

fn chown(p: &Path, uid: u32, gid: u32) -> std::io::Result<()> {
    std::os::unix::fs::lchown(p, Some(uid), Some(gid))
}

/// The folder agents' homes go in: yours, others may pass through it but not list it.
fn make_agent_homes(owner: libc::uid_t) -> Result<String, String> {
    use std::os::unix::fs::PermissionsExt;
    let dir = ctx_protocol::helper::agent_homes();
    if let Some(m) = lstat(&dir) {
        return if m.is_dir() && std::os::unix::fs::MetadataExt::uid(&m) == owner {
            Ok(format!("{} is there.", dir.display()))
        } else {
            Err(format!("{} exists and isn't a folder of yours.", dir.display()))
        };
    }
    std::fs::create_dir(&dir).map_err(|e| format!("couldn't make {}: {e}", dir.display()))?;
    let gid = accounts::by_uid(owner).map(|o| o.gid).unwrap_or(0);
    chown(&dir, owner, gid).map_err(|e| format!("couldn't give {} to you: {e}", dir.display()))?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o711))
        .map_err(|e| format!("couldn't set {}'s permissions: {e}", dir.display()))?;
    Ok(format!("Made {} for agents' homes.", dir.display()))
}

fn make_account(
    name: &str,
    full_name: &str,
    home: &str,
    files: &[StarterFile],
    owner: libc::uid_t,
) -> Result<String, String> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let home_path = PathBuf::from(home);
    if !home_path.is_absolute() || home.contains("/../") || home.ends_with("/..") {
        return Err(format!("The home folder must be a full path, not \"{home}\"."));
    }
    let parent = home_path.parent().ok_or("The home folder needs a parent folder.")?;
    let your_home = accounts::by_uid(owner).map(|o| PathBuf::from(o.home)).unwrap_or_default();
    if !your_home.as_os_str().is_empty() && home_path.starts_with(&your_home) {
        return Err(format!(
            "An agent's home can't be inside your home folder ({}): the agent would have to pass through it, and could then read anything in it that other accounts may. Agents' homes go in {}.",
            your_home.display(),
            ctx_protocol::helper::agent_homes().display()
        ));
    }
    if !owned_folder(parent, owner) {
        return Err(format!("{} must be a folder of yours.", parent.display()));
    }
    let existing = lstat(&home_path);
    if let Some(m) = &existing
        && !(m.is_dir() && m.uid() == owner)
    {
        return Err(format!("{home} exists and isn't a folder of yours."));
    }
    let a = accounts::make(name, full_name, home)?;
    let undo = |e: String| {
        let _ = accounts::remove(name);
        e
    };
    if existing.is_none() {
        // Made here, as root: it's the agent's from the start.
        std::fs::create_dir(&home_path).map_err(|e| undo(format!("couldn't make {home}: {e}")))?;
        chown(&home_path, a.uid, a.gid).map_err(|e| undo(format!("couldn't give {home} to the agent: {e}")))?;
    }
    // Your files in it become the agent's: only files you own, not through links, and no file
    // with other hard links (it would be someone else's file too).
    let mut skipped = 0;
    let mut give = |p: &Path| {
        if let Some(m) = lstat(p)
            && m.uid() == owner
            && (m.is_dir() || m.nlink() == 1)
        {
            let _ = chown(p, a.uid, a.gid);
        } else {
            skipped += 1;
        }
    };
    if existing.is_some() {
        give(&home_path);
    }
    acl::each_inside(&home_path, &mut give);
    let _ = std::fs::set_permissions(&home_path, std::fs::Permissions::from_mode(0o700));
    for f in files {
        let rel = Path::new(&f.path);
        if rel.is_absolute() || rel.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
            continue;
        }
        let p = home_path.join(rel);
        if lstat(&p).is_some() {
            continue;
        }
        if let Some(dir) = p.parent()
            && lstat(dir).is_none()
        {
            let _ = std::fs::create_dir_all(dir);
            let _ = chown(dir, a.uid, a.gid);
        }
        if std::fs::write(&p, &f.text).is_ok() {
            let (u, g) =
                if f.yours { (owner, accounts::by_uid(owner).map(|o| o.gid).unwrap_or(0)) } else { (a.uid, a.gid) };
            let _ = chown(&p, u, g);
            let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644));
        }
    }
    // You keep full access to everything in it, now and later.
    acl::apply(&home_path, acl::Who::User(owner), Some(acl::Access::Write))
        .map_err(|e| undo(format!("couldn't give you access to {home}: {e}")))?;
    // Agents pass through the folders of yours above it (never your home: see above).
    let gid = accounts::group(accounts::AGENTS_GROUP).map(|(g, _)| g).unwrap_or(a.gid);
    let mut up = parent.to_path_buf();
    loop {
        if !owned_folder(&up, owner) || up == your_home {
            break;
        }
        let _ = acl::allow_passing(&up, acl::Who::Group(gid));
        match up.parent() {
            Some(p) => up = p.to_path_buf(),
            None => break,
        }
    }
    let mut msg = format!("Made the agent \"{name}\" (account {}, uid {}) with its home at {home}.", a.name, a.uid);
    if skipped > 0 {
        msg.push_str(&format!(" {skipped} files in it weren't yours (or had other links) and kept their owner."));
    }
    Ok(msg)
}

/// End every process of the account: as the account, signal everything it may signal (only its
/// own processes), politely and then not; wait for them to be gone.
fn stop_everything_of(a: &accounts::Account) {
    for (sig, settle) in [(libc::SIGHUP, 500), (libc::SIGKILL, 1500)] {
        // SAFETY: fork; the child only makes system calls and exits.
        unsafe {
            let pid = libc::fork();
            if pid == 0 {
                if libc::setgid(a.gid) == 0 && libc::setuid(a.uid) == 0 {
                    libc::kill(-1, sig);
                }
                libc::_exit(0);
            }
            if pid > 0 {
                let mut status = 0;
                libc::waitpid(pid, &mut status, 0);
            }
        }
        let start = std::time::Instant::now();
        while start.elapsed() < std::time::Duration::from_millis(settle) {
            if !runs_anything(a.uid) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

/// Whether any process runs as `uid` (signal 0 as that account, in a child).
fn runs_anything(uid: libc::uid_t) -> bool {
    // SAFETY: fork; the child only makes system calls and exits.
    unsafe {
        let pid = libc::fork();
        if pid == 0 {
            // kill(-1, 0) as the account: success means something of its own is there to signal.
            let found = libc::setuid(uid) == 0 && libc::kill(-1, 0) == 0;
            libc::_exit(if found { 0 } else { 1 });
        }
        let mut status = 0;
        pid > 0 && libc::waitpid(pid, &mut status, 0) > 0 && libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0
    }
}

fn remove_account(name: &str, trash_home: bool, owner: libc::uid_t) -> Result<String, String> {
    use std::os::unix::fs::MetadataExt;
    let a = agent(name, owner)?;
    // Its programs stop first (Linux won't delete an account that still runs any).
    stop_everything_of(&a);
    let mut msg = String::new();
    let home = PathBuf::from(&a.home);
    if trash_home && lstat(&home).is_some_and(|m| m.is_dir() && m.uid() == a.uid) {
        let you = accounts::by_uid(owner).ok_or("There's no owner account.")?;
        if !home.parent().is_some_and(|p| owned_folder(p, owner)) {
            return Err(format!("{} isn't in a folder of yours, so ctx won't move it.", home.display()));
        }
        let trash = PathBuf::from(&you.home).join(".Trash");
        let _ = std::fs::create_dir_all(&trash);
        let stamp =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let to = trash.join(format!("{name}-{stamp}"));
        std::fs::rename(&home, &to).map_err(|e| format!("couldn't move {} to the Trash: {e}", home.display()))?;
        // Everything the agent owned in it becomes yours.
        let mut take = |p: &Path| {
            if lstat(p).is_some_and(|m| m.uid() == a.uid && (m.is_dir() || m.nlink() == 1)) {
                let _ = chown(p, owner, you.gid);
            }
        };
        take(&to);
        acl::each_inside(&to, &mut take);
        msg = format!(" Its home is in your Trash ({}).", to.display());
    }
    accounts::remove(name)?;
    Ok(format!("Removed the agent \"{name}\".{msg}"))
}

fn share(path: &Path, entry: EntryFor, grant: Option<Grant>, owner: libc::uid_t) -> Result<String, String> {
    use std::os::unix::fs::MetadataExt;
    let Some(top) = lstat(path) else { return Err(format!("There's no {}.", path.display())) };
    let agents: Vec<u32> = accounts::agents().iter().map(|a| a.uid).collect();
    // Yours, or an agent's (you have full access to every agent's home): never anyone else's.
    if !(top.uid() == owner || agents.contains(&top.uid())) || top.file_type().is_symlink() {
        return Err(format!("{} isn't yours or an agent's, so ctx won't share it.", path.display()));
    }
    let who = match entry {
        EntryFor::User(u) => acl::Who::User(u),
        EntryFor::Group(g) => acl::Who::Group(g),
    };
    let access = grant.map(|g| match g {
        Grant::Read => acl::Access::Read,
        Grant::Write => acl::Access::Write,
        Grant::ReadOnly => acl::Access::ReadOnly,
        Grant::Nothing => acl::Access::Nothing,
    });
    let (mut changed, mut skipped) = (0, 0);
    let mut one = |p: &Path, inside: bool| {
        let ok = lstat(p).is_some_and(|m| {
            !m.file_type().is_symlink()
                && (m.uid() == owner || agents.contains(&m.uid()))
                && (m.is_dir() || m.nlink() == 1)
        });
        if ok && acl::apply_one(p, who, access, inside).is_ok() {
            changed += 1;
        } else {
            skipped += 1;
        }
    };
    one(path, false);
    if top.is_dir() {
        acl::each_inside(path, &mut |p| one(p, true));
    }
    Ok(format!("{changed} changed, {skipped} left alone (not yours or an agent's, links, or files with other links)."))
}

/// Ask the OS as the account: fork, become it, `access(2)`.
fn may_access(a: &accounts::Account, path: &str, write: bool) -> bool {
    let Ok(p) = CString::new(path) else { return false };
    let Ok(n) = CString::new(a.name.clone()) else { return false };
    // SAFETY: fork; the child only makes system calls and exits.
    unsafe {
        let pid = libc::fork();
        if pid == 0 {
            let ok = libc::initgroups(n.as_ptr(), a.gid as _) == 0
                && libc::setgid(a.gid) == 0
                && libc::setuid(a.uid) == 0
                && libc::access(p.as_ptr(), if write { libc::W_OK } else { libc::R_OK }) == 0;
            libc::_exit(if ok { 0 } else { 1 });
        }
        let mut status = 0;
        let waited = pid > 0
            && loop {
                if libc::waitpid(pid, &mut status, 0) >= 0 {
                    break true;
                }
                if std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
                    break false;
                }
            };
        waited && libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0
    }
}

/// Collect the exit status of tabs that have ended (so they don't stay as zombies).
fn reap_ended_tabs() {
    let mut status = 0;
    // SAFETY: a non-blocking wait for any child.
    while unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) } > 0 {}
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
    // SIGCHLD stays at its default: ignoring it would have the system reap tabs' processes, but
    // also take the exit status of every program the helper runs itself (dscl, useradd, setfacl),
    // which then fail with ECHILD. Requests are served one at a time, so tabs that have ended are
    // reaped before each one, while none of the helper's own programs are running.
    for stream in listener.incoming().flatten() {
        reap_ended_tabs();
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
            limits: Default::default(),
        };
        assert!(check(&req("dev", &me), &rules).is_ok());
        // An agent account must be in ctx-agents: this account (the test's) isn't.
        assert!(check(&req(&me, &me), &rules).is_err());
        assert!(check(&req("dev", "root"), &rules).unwrap_err().contains("runs as"));
        assert!(check(&req("email", &me), &rules).unwrap_err().contains("don't have the agent"));
        let root = check(&req("sysadmin", "root"), &rules).err().unwrap();
        assert!(root.contains("administrator didn't approve"), "{root}");
        let mut fake = req("sysadmin", "root");
        fake.authorization = Some(vec![0; 32]);
        assert!(check(&fake, &rules).is_err(), "a made-up authorization isn't an approval");
    }
}
