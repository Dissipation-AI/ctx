//! System calls: pseudo-terminals, disclaimed spawning, process identity. Everything
//! platform-specific lives here: macOS first, with Linux equivalents (procfs, `SO_PEERCRED`) where
//! the calls differ.

#[cfg(target_os = "macos")]
use std::ffi::OsStr;
use std::ffi::{CStr, CString};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

fn cvt(r: libc::c_int) -> io::Result<libc::c_int> {
    if r < 0 { Err(io::Error::last_os_error()) } else { Ok(r) }
}

/// The per-user temporary folder (`/var/folders/…/T/`), from the OS rather than `$TMPDIR`,
/// which agents' sandboxes sometimes change.
#[cfg(target_os = "linux")]
pub fn user_temp_dir() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => std::env::temp_dir(),
    }
}

#[cfg(target_os = "macos")]
pub fn user_temp_dir() -> PathBuf {
    let mut buf = vec![0u8; 1024];
    // SAFETY: buf is valid for its length; confstr writes at most that many bytes.
    let n = unsafe { libc::confstr(libc::_CS_DARWIN_USER_TEMP_DIR, buf.as_mut_ptr().cast(), buf.len()) };
    if n > 0 && n <= buf.len() {
        buf.truncate(n - 1);
        return PathBuf::from(OsStr::from_bytes(&buf));
    }
    std::env::temp_dir()
}

pub fn uid() -> u32 {
    // SAFETY: getuid can't fail.
    unsafe { libc::getuid() }
}

/// Set close-on-exec and non-blocking.
pub fn set_cloexec_nonblock(fd: RawFd, nonblock: bool) -> io::Result<()> {
    // SAFETY: fcntl on a descriptor we own.
    unsafe {
        let flags = cvt(libc::fcntl(fd, libc::F_GETFD))?;
        cvt(libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC))?;
        if nonblock {
            let fl = cvt(libc::fcntl(fd, libc::F_GETFL))?;
            cvt(libc::fcntl(fd, libc::F_SETFL, fl | libc::O_NONBLOCK))?;
        }
    }
    Ok(())
}

/// A new pseudo-terminal of the given size: (master, slave).
pub fn open_pty(columns: u16, rows: u16) -> io::Result<(OwnedFd, OwnedFd)> {
    let mut master: libc::c_int = -1;
    let mut slave: libc::c_int = -1;
    let mut ws = libc::winsize { ws_row: rows, ws_col: columns, ws_xpixel: 0, ws_ypixel: 0 };
    // SAFETY: out-pointers are valid; name and termios may be null. (The size is a const pointer
    // on Linux and a mut one on macOS; a raw mut pointer suits both.)
    cvt(unsafe { libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null_mut(), &raw mut ws) })?;
    // SAFETY: openpty returned two fresh descriptors we now own.
    let (m, s) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
    set_cloexec_nonblock(m.as_raw_fd(), true)?;
    set_cloexec_nonblock(s.as_raw_fd(), false)?;
    Ok((m, s))
}

pub fn set_window_size(fd: RawFd, columns: u16, rows: u16) {
    let ws = libc::winsize { ws_row: rows, ws_col: columns, ws_xpixel: 0, ws_ypixel: 0 };
    // SAFETY: TIOCSWINSZ with a valid winsize.
    unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, &ws) };
}

/// Process group in the foreground of the terminal.
pub fn foreground_pgid(master: RawFd) -> Option<i32> {
    // SAFETY: tcgetpgrp on our master descriptor.
    let pg = unsafe { libc::tcgetpgrp(master) };
    if pg > 0 { Some(pg) } else { None }
}

/// The program in front is reading a password: line input with echo off (what `getpass`, `sudo`,
/// `ssh` and `read -s` set). A line editor (zsh, readline) or a full-screen program reads
/// keystroke by keystroke instead, so it doesn't count.
pub fn password_input(master: RawFd) -> bool {
    // SAFETY: tcgetattr on our master descriptor into a zeroed termios.
    let mut t: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(master, &mut t) } != 0 {
        return false;
    }
    t.c_lflag & libc::ECHO == 0 && t.c_lflag & libc::ICANON != 0
}

type DisclaimFn = unsafe extern "C" fn(*mut libc::posix_spawnattr_t, libc::c_int) -> libc::c_int;
type ResponsibleFn = unsafe extern "C" fn(libc::pid_t) -> libc::pid_t;

fn lookup(name: &CStr) -> *mut libc::c_void {
    // SAFETY: dlsym with RTLD_DEFAULT and a NUL-terminated name.
    unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr()) }
}

/// Whether this macOS lets a spawned process disclaim responsibility.
pub fn can_disclaim() -> bool {
    !lookup(c"responsibility_spawnattrs_setdisclaim").is_null()
}

/// Spawn `program` with `args` (argv[0] included) and `env`, with `stdio` as its standard input,
/// output and error and every other descriptor closed. With `disclaim`, the new process is its
/// own "responsible process" for macOS privacy checks instead of inheriting ours.
pub fn spawn(
    program: &std::path::Path,
    args: &[String],
    env: &[(String, String)],
    stdio: Option<RawFd>,
    disclaim: bool,
) -> io::Result<libc::pid_t> {
    let prog = CString::new(program.as_os_str().as_bytes()).map_err(|_| io::Error::other("bad program path"))?;
    let cargs: Vec<CString> = args.iter().map(|a| CString::new(a.as_bytes()).unwrap_or_default()).collect();
    let mut argv: Vec<*mut libc::c_char> = cargs.iter().map(|a| a.as_ptr() as *mut _).collect();
    argv.push(std::ptr::null_mut());
    let cenv: Vec<CString> = env.iter().filter_map(|(k, v)| CString::new(format!("{k}={v}")).ok()).collect();
    let mut envp: Vec<*mut libc::c_char> = cenv.iter().map(|a| a.as_ptr() as *mut _).collect();
    envp.push(std::ptr::null_mut());

    // SAFETY: the attr and file-action objects are initialised before use and destroyed after;
    // all pointers stay alive for the posix_spawn call.
    unsafe {
        let mut attr: libc::posix_spawnattr_t = std::mem::zeroed();
        cvt_spawn(libc::posix_spawnattr_init(&mut attr))?;
        let mut actions: libc::posix_spawn_file_actions_t = std::mem::zeroed();
        cvt_spawn(libc::posix_spawn_file_actions_init(&mut actions))?;
        let result = (|| {
            let mut sigs: libc::sigset_t = std::mem::zeroed();
            libc::sigfillset(&mut sigs);
            libc::posix_spawnattr_setsigdefault(&mut attr, &sigs);
            let mut empty: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut empty);
            libc::posix_spawnattr_setsigmask(&mut attr, &empty);
            #[cfg(target_os = "macos")]
            let flags = libc::POSIX_SPAWN_SETSIGDEF | libc::POSIX_SPAWN_SETSIGMASK | libc::POSIX_SPAWN_CLOEXEC_DEFAULT;
            // Linux has no close-everything flag; our own descriptors are all close-on-exec.
            #[cfg(target_os = "linux")]
            let flags = libc::POSIX_SPAWN_SETSIGDEF | libc::POSIX_SPAWN_SETSIGMASK;
            cvt_spawn(libc::posix_spawnattr_setflags(&mut attr, flags as _))?;
            if disclaim && cfg!(target_os = "macos") {
                let f = lookup(c"responsibility_spawnattrs_setdisclaim");
                if f.is_null() {
                    return Err(io::Error::other("this macOS can't disclaim responsibility"));
                }
                let f: DisclaimFn = std::mem::transmute(f);
                cvt_spawn(f(&mut attr, 1))?;
            }
            match stdio {
                Some(fd) => {
                    for target in 0..3 {
                        cvt_spawn(libc::posix_spawn_file_actions_adddup2(&mut actions, fd, target))?;
                    }
                }
                None => {
                    for target in 0..3 {
                        cvt_spawn(libc::posix_spawn_file_actions_addopen(
                            &mut actions,
                            target,
                            c"/dev/null".as_ptr(),
                            libc::O_RDWR,
                            0,
                        ))?;
                    }
                }
            }
            let mut pid: libc::pid_t = 0;
            cvt_spawn(libc::posix_spawn(&mut pid, prog.as_ptr(), &actions, &attr, argv.as_ptr(), envp.as_ptr()))?;
            Ok(pid)
        })();
        libc::posix_spawn_file_actions_destroy(&mut actions);
        libc::posix_spawnattr_destroy(&mut attr);
        result
    }
}

fn cvt_spawn(r: libc::c_int) -> io::Result<()> {
    if r == 0 { Ok(()) } else { Err(io::Error::from_raw_os_error(r)) }
}

/// The process whose code signature macOS checks for privacy permissions on `pid`'s behalf.
pub fn responsible_pid(pid: i32) -> Option<i32> {
    let f = lookup(c"responsibility_get_pid_responsible_for_pid");
    if f.is_null() {
        return None;
    }
    // SAFETY: the symbol has this signature on every macOS that exports it.
    let f: ResponsibleFn = unsafe { std::mem::transmute(f) };
    // SAFETY: plain call with a pid.
    let r = unsafe { f(pid) };
    if r > 0 { Some(r) } else { None }
}

#[cfg(target_os = "linux")]
fn peer_cred(fd: RawFd) -> Option<libc::ucred> {
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: getsockopt with a correctly sized out-buffer.
    let r = unsafe {
        libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_PEERCRED, (&mut cred as *mut libc::ucred).cast(), &mut len)
    };
    if r == 0 { Some(cred) } else { None }
}

#[cfg(target_os = "linux")]
pub fn peer_pid(fd: RawFd) -> Option<i32> {
    peer_cred(fd).map(|c| c.pid).filter(|p| *p > 0)
}

#[cfg(target_os = "linux")]
pub fn peer_uid(fd: RawFd) -> Option<u32> {
    peer_cred(fd).map(|c| c.uid)
}

/// Process id of the other end of a Unix socket.
#[cfg(target_os = "macos")]
pub fn peer_pid(fd: RawFd) -> Option<i32> {
    let mut pid: libc::pid_t = 0;
    let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    // SAFETY: getsockopt with a correctly sized out-buffer.
    let r = unsafe {
        libc::getsockopt(fd, libc::SOL_LOCAL, libc::LOCAL_PEERPID, (&mut pid as *mut libc::pid_t).cast(), &mut len)
    };
    if r == 0 && pid > 0 { Some(pid) } else { None }
}

/// User id of the other end of a Unix socket.
#[cfg(target_os = "macos")]
pub fn peer_uid(fd: RawFd) -> Option<u32> {
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    // SAFETY: getpeereid with valid out-pointers.
    let r = unsafe { libc::getpeereid(fd, &mut uid, &mut gid) };
    if r == 0 { Some(uid) } else { None }
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn sandbox_check(pid: libc::pid_t, operation: *const libc::c_char, filter: libc::c_int, ...) -> libc::c_int;
}

/// Whether a process runs in a Seatbelt sandbox.
#[cfg(target_os = "macos")]
pub fn sandboxed(pid: i32) -> bool {
    // SAFETY: a null operation asks only whether the process is sandboxed.
    unsafe { sandbox_check(pid, std::ptr::null(), 0) == 1 }
}

#[cfg(not(target_os = "macos"))]
pub fn sandboxed(_pid: i32) -> bool {
    false
}

/// Whether a sandboxed process may read `path` (its profile decides; the file needn't exist).
#[cfg(target_os = "macos")]
pub fn sandbox_allows_read(pid: i32, path: &Path) -> bool {
    const SANDBOX_FILTER_PATH: libc::c_int = 1;
    const SANDBOX_CHECK_NO_REPORT: libc::c_int = 0x4000_0000;
    let Ok(p) = CString::new(path.as_os_str().as_encoded_bytes()) else { return false };
    // SAFETY: valid C strings for the operation and the path filter argument.
    unsafe {
        sandbox_check(pid, c"file-read-data".as_ptr(), SANDBOX_FILTER_PATH | SANDBOX_CHECK_NO_REPORT, p.as_ptr()) == 0
    }
}

#[cfg(not(target_os = "macos"))]
pub fn sandbox_allows_read(_pid: i32, _path: &Path) -> bool {
    false
}

/// A macOS account's user id and home folder.
pub fn account(name: &str) -> Option<(u32, PathBuf)> {
    let n = CString::new(name).ok()?;
    // SAFETY: getpwnam returns a pointer to static storage or null; read it at once.
    unsafe {
        let pw = libc::getpwnam(n.as_ptr());
        if pw.is_null() {
            return None;
        }
        Some(((*pw).pw_uid, PathBuf::from(CStr::from_ptr((*pw).pw_dir).to_string_lossy().into_owned())))
    }
}

/// Ask the ctx helper to start a tab's program as an agent's account, handing it the terminal.
/// Returns the new process's pid.
pub fn helper_spawn(req: &ctx_protocol::helper::SpawnRequest, tty: RawFd) -> Result<i32, String> {
    use ctx_protocol::helper::{SpawnReply, socket};
    use std::io::BufRead;
    let path = socket();
    let stream = std::os::unix::net::UnixStream::connect(&path)
        .map_err(|e| format!("the ctx helper isn't running ({}: {e}); see `ctx help agents`", path.display()))?;
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(10)));
    let mut line = serde_json::to_vec(req).map_err(|e| e.to_string())?;
    line.push(b'\n');
    let mut cmsg = [0u8; 64];
    let mut iov = libc::iovec { iov_base: line.as_mut_ptr().cast(), iov_len: line.len() };
    // SAFETY: msghdr and its control buffer stay alive for sendmsg; one descriptor is attached.
    let sent = unsafe {
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = cmsg.as_mut_ptr().cast();
        msg.msg_controllen = libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as u32) as _;
        let c = libc::CMSG_FIRSTHDR(&msg);
        (*c).cmsg_level = libc::SOL_SOCKET;
        (*c).cmsg_type = libc::SCM_RIGHTS;
        (*c).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<RawFd>() as u32) as _;
        *(libc::CMSG_DATA(c) as *mut RawFd) = tty;
        libc::sendmsg(stream.as_raw_fd(), &msg, 0)
    };
    if sent < 0 || sent as usize != line.len() {
        return Err(format!("couldn't send the request to the ctx helper: {}", io::Error::last_os_error()));
    }
    let mut answer = String::new();
    io::BufReader::new(&stream).read_line(&mut answer).map_err(|e| format!("the ctx helper didn't answer: {e}"))?;
    match serde_json::from_str::<SpawnReply>(&answer) {
        Ok(SpawnReply::Started { pid }) => Ok(pid),
        Ok(SpawnReply::Refused { message }) => Err(format!("the ctx helper refused: {message}")),
        Err(e) => Err(format!("the ctx helper's answer didn't make sense: {e}")),
    }
}

/// An administrator's approval (`system.privilege.admin`): macOS asks for an admin's password or
/// Touch ID. Returns the approval's 32-byte external form, for the ctx helper.
#[cfg(target_os = "macos")]
pub fn admin_approval() -> Result<Vec<u8>, String> {
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
        fn AuthorizationCreate(r: *const Rights, e: *const libc::c_void, f: u32, a: *mut *mut libc::c_void) -> i32;
        fn AuthorizationCopyRights(
            a: *mut libc::c_void,
            r: *const Rights,
            e: *const libc::c_void,
            f: u32,
            o: *mut *mut Rights,
        ) -> i32;
        fn AuthorizationMakeExternalForm(a: *mut libc::c_void, ext: *mut [u8; 32]) -> i32;
        fn AuthorizationFree(a: *mut libc::c_void, f: u32) -> i32;
    }
    const INTERACTION_ALLOWED: u32 = 1;
    const EXTEND_RIGHTS: u32 = 1 << 1;
    const PRE_AUTHORIZE: u32 = 1 << 4;
    let mut a: *mut libc::c_void = std::ptr::null_mut();
    // SAFETY: Authorization calls with valid pointers; the reference is freed (rights kept, so the
    // external form stays usable) before returning.
    unsafe {
        if AuthorizationCreate(std::ptr::null(), std::ptr::null(), 0, &mut a) != 0 {
            return Err("couldn't ask macOS for an administrator's approval".into());
        }
        let mut item =
            Item { name: c"system.privilege.admin".as_ptr(), value_length: 0, value: std::ptr::null_mut(), flags: 0 };
        let rights = Rights { count: 1, items: &mut item };
        let flags = INTERACTION_ALLOWED | EXTEND_RIGHTS | PRE_AUTHORIZE;
        let r = AuthorizationCopyRights(a, &rights, std::ptr::null(), flags, std::ptr::null_mut());
        if r != 0 {
            AuthorizationFree(a, 0);
            return Err(if r == -60006 {
                "an administrator didn't approve it".into()
            } else {
                format!("macOS didn't approve it (error {r})")
            });
        }
        let mut ext = [0u8; 32];
        let r = AuthorizationMakeExternalForm(a, &mut ext);
        AuthorizationFree(a, 0);
        if r != 0 {
            return Err(format!("couldn't pass the approval on (error {r})"));
        }
        Ok(ext.to_vec())
    }
}

#[cfg(not(target_os = "macos"))]
pub fn admin_approval() -> Result<Vec<u8>, String> {
    Err("administrator approval needs macOS".into())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcInfo {
    pub pid: i32,
    pub ppid: i32,
    pub pgid: i32,
    /// Short process name.
    pub name: String,
    /// Start time in microseconds since the epoch (tells two processes with the same pid apart).
    pub start_us: u64,
}

#[cfg(target_os = "macos")]
fn c_chars(buf: &[libc::c_char]) -> String {
    let bytes: Vec<u8> = buf.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

#[cfg(target_os = "linux")]
pub fn proc_info(pid: i32) -> Option<ProcInfo> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // `pid (name) state ppid pgrp …`; the name may itself contain parentheses.
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    let comm = stat[open + 1..close].to_string();
    let rest: Vec<&str> = stat[close + 1..].split_whitespace().collect();
    // rest[0] is the state (field 3), so field n is rest[n - 3].
    let ppid = rest.get(1)?.parse().ok()?;
    let pgid = rest.get(2)?.parse().ok()?;
    let ticks: u64 = rest.get(19)?.parse().ok()?;
    // SAFETY: sysconf has no preconditions.
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) }.max(1) as u64;
    let start_us = boot_time_secs().max(0) as u64 * 1_000_000 + ticks * 1_000_000 / hz;
    // `comm` is cut to 15 bytes; the executable's name is the long form.
    let name = proc_path(pid)
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .filter(|n| n.starts_with(&comm) && n.len() > comm.len())
        .unwrap_or(comm);
    Some(ProcInfo { pid, ppid, pgid, name, start_us })
}

#[cfg(target_os = "macos")]
pub fn proc_info(pid: i32) -> Option<ProcInfo> {
    // SAFETY: proc_bsdinfo is plain old data; proc_pidinfo fills at most its size.
    unsafe {
        let mut info: libc::proc_bsdinfo = std::mem::zeroed();
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let n = libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&mut info as *mut libc::proc_bsdinfo).cast(), size);
        if n != size {
            return None;
        }
        let name = {
            let long = c_chars(&info.pbi_name);
            if long.is_empty() { c_chars(&info.pbi_comm) } else { long }
        };
        Some(ProcInfo {
            pid,
            ppid: info.pbi_ppid as i32,
            pgid: info.pbi_pgid as i32,
            name,
            start_us: info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec,
        })
    }
}

#[cfg(target_os = "linux")]
pub fn proc_path(pid: i32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/exe")).ok()
}

#[cfg(target_os = "linux")]
pub fn proc_args(pid: i32) -> Option<Vec<String>> {
    let raw = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let raw = raw.strip_suffix(&[0]).unwrap_or(&raw);
    if raw.is_empty() {
        return None;
    }
    Some(raw.split(|b| *b == 0).map(|p| String::from_utf8_lossy(p).into_owned()).collect())
}

/// Full path of a process's executable.
#[cfg(target_os = "macos")]
pub fn proc_path(pid: i32) -> Option<PathBuf> {
    let mut buf = vec![0u8; 4096];
    // SAFETY: buf is valid for its length.
    let n = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if n <= 0 {
        return None;
    }
    buf.truncate(n as usize);
    Some(PathBuf::from(OsStr::from_bytes(&buf)))
}

/// Arguments of a process (argv), from `KERN_PROCARGS2`.
#[cfg(target_os = "macos")]
pub fn proc_args(pid: i32) -> Option<Vec<String>> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let mut size: libc::size_t = 0;
    // SAFETY: first call asks for the size; second fills a buffer of that size.
    unsafe {
        if libc::sysctl(mib.as_mut_ptr(), 3, std::ptr::null_mut(), &mut size, std::ptr::null_mut(), 0) != 0 {
            return None;
        }
        let mut buf = vec![0u8; size];
        if libc::sysctl(mib.as_mut_ptr(), 3, buf.as_mut_ptr().cast(), &mut size, std::ptr::null_mut(), 0) != 0 {
            return None;
        }
        buf.truncate(size);
        if buf.len() < 4 {
            return None;
        }
        let argc = i32::from_ne_bytes(buf[..4].try_into().ok()?) as usize;
        let mut rest = &buf[4..];
        // Skip the executable path and its padding NULs.
        let end = rest.iter().position(|b| *b == 0)?;
        rest = &rest[end..];
        let start = rest.iter().position(|b| *b != 0)?;
        rest = &rest[start..];
        let mut args = Vec::new();
        for part in rest.split(|b| *b == 0).take(argc) {
            args.push(String::from_utf8_lossy(part).into_owned());
        }
        Some(args)
    }
}

/// Every process we can see.
#[cfg(target_os = "linux")]
pub fn all_processes() -> Vec<ProcInfo> {
    let Ok(rd) = std::fs::read_dir("/proc") else { return Vec::new() };
    rd.flatten()
        .filter_map(|e| e.file_name().to_str().and_then(|n| n.parse::<i32>().ok()))
        .filter_map(proc_info)
        .collect()
}

/// Every process we can see.
#[cfg(target_os = "macos")]
pub fn all_processes() -> Vec<ProcInfo> {
    // SAFETY: the first call asks for the count; the second fills a buffer with room for it.
    unsafe {
        let n = libc::proc_listallpids(std::ptr::null_mut(), 0);
        if n <= 0 {
            return Vec::new();
        }
        let mut pids = vec![0 as libc::pid_t; n as usize + 64];
        let got = libc::proc_listallpids(
            pids.as_mut_ptr().cast(),
            (pids.len() * std::mem::size_of::<libc::pid_t>()) as libc::c_int,
        );
        pids.truncate(got.max(0) as usize);
        pids.into_iter().filter(|p| *p > 0).filter_map(proc_info).collect()
    }
}

/// The processes under `root` (not `root` itself), in tree order with their depth below it.
pub fn descendants(root: i32) -> Vec<(ProcInfo, usize)> {
    let all = all_processes();
    let mut out = Vec::new();
    fn walk(all: &[ProcInfo], parent: i32, depth: usize, out: &mut Vec<(ProcInfo, usize)>) {
        let mut kids: Vec<&ProcInfo> = all.iter().filter(|p| p.ppid == parent && p.pid != parent).collect();
        kids.sort_by_key(|p| p.start_us);
        for k in kids {
            out.push((k.clone(), depth));
            if depth < 16 {
                walk(all, k.pid, depth + 1, out);
            }
        }
    }
    walk(&all, root, 0, &mut out);
    out
}

/// The chain of parents from `pid` up to launchd (pid 1 excluded), nearest first.
pub fn ancestors(pid: i32) -> Vec<ProcInfo> {
    let mut out = Vec::new();
    let mut cur = pid;
    for _ in 0..64 {
        if cur <= 1 {
            break;
        }
        let Some(info) = proc_info(cur) else { break };
        cur = info.ppid;
        out.push(info);
    }
    out
}

/// Resident memory of a process, less what it shares with others (file-backed pages).
#[cfg(target_os = "linux")]
pub fn memory_footprint(pid: i32) -> u64 {
    let Ok(statm) = std::fs::read_to_string(format!("/proc/{pid}/statm")) else { return 0 };
    let f: Vec<u64> = statm.split_whitespace().filter_map(|x| x.parse().ok()).collect();
    // SAFETY: sysconf has no preconditions.
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(1) as u64;
    match (f.get(1), f.get(2)) {
        (Some(rss), Some(shared)) => rss.saturating_sub(*shared) * page,
        _ => 0,
    }
}

/// Physical memory footprint of a process (what Activity Monitor shows as Memory).
#[cfg(target_os = "macos")]
pub fn memory_footprint(pid: i32) -> u64 {
    // SAFETY: rusage_info_v2 is plain old data and the flavour matches it.
    unsafe {
        let mut info: libc::rusage_info_v2 = std::mem::zeroed();
        let r = libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V2, (&mut info as *mut libc::rusage_info_v2).cast());
        if r == 0 { info.ri_phys_footprint } else { 0 }
    }
}

/// Local time zone offset east of UTC, in seconds, now.
pub fn tz_offset() -> i64 {
    // SAFETY: time and localtime_r with valid pointers.
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut tm);
        tm.tm_gmtoff
    }
}

pub fn hostname() -> String {
    let mut buf = vec![0u8; 256];
    // SAFETY: buf is valid for its length.
    unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    let host = String::from_utf8_lossy(&buf[..end]).into_owned();
    host.trim_end_matches(".local").to_string()
}

pub fn username() -> String {
    std::env::var("USER").ok().filter(|u| !u.is_empty()).unwrap_or_else(|| {
        // SAFETY: getpwuid returns a pointer to static storage or null.
        unsafe {
            let pw = libc::getpwuid(libc::getuid());
            if pw.is_null() { "user".into() } else { CStr::from_ptr((*pw).pw_name).to_string_lossy().into_owned() }
        }
    })
}

/// The user's login shell, from the directory service.
pub fn login_shell() -> String {
    // SAFETY: getpwuid returns a pointer to static storage or null.
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if !pw.is_null() && !(*pw).pw_shell.is_null() {
            let s = CStr::from_ptr((*pw).pw_shell).to_string_lossy().into_owned();
            if !s.is_empty() {
                return s;
            }
        }
    }
    "/bin/zsh".into()
}

pub fn home() -> String {
    std::env::var("HOME").ok().filter(|h| !h.is_empty()).unwrap_or_else(|| {
        // SAFETY: getpwuid returns a pointer to static storage or null.
        unsafe {
            let pw = libc::getpwuid(libc::getuid());
            if pw.is_null() { "/".into() } else { CStr::from_ptr((*pw).pw_dir).to_string_lossy().into_owned() }
        }
    })
}

/// Send a signal to a process group.
pub fn kill_group(pgid: i32, sig: i32) {
    // SAFETY: plain syscall.
    unsafe { libc::killpg(pgid, sig) };
}

pub fn kill(pid: i32, sig: i32) {
    // SAFETY: plain syscall.
    unsafe { libc::kill(pid, sig) };
}

/// Reap any exited children without blocking; returns (pid, exit status).
pub fn reap() -> Vec<(i32, i32)> {
    let mut out = Vec::new();
    loop {
        let mut status = 0;
        // SAFETY: waitpid with WNOHANG.
        let pid = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
        if pid <= 0 {
            break;
        }
        let code = if libc::WIFEXITED(status) { libc::WEXITSTATUS(status) } else { 128 + libc::WTERMSIG(status) };
        out.push((pid, code));
    }
    out
}

/// Read without blocking. `Ok(0)` is end of file; `WouldBlock` means nothing yet.
pub fn read_fd(fd: RawFd, buf: &mut [u8]) -> io::Result<usize> {
    // SAFETY: buf is valid for its length.
    let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
    if n < 0 { Err(io::Error::last_os_error()) } else { Ok(n as usize) }
}

pub fn write_fd(fd: RawFd, buf: &[u8]) -> io::Result<usize> {
    // SAFETY: buf is valid for its length.
    let n = unsafe { libc::write(fd, buf.as_ptr().cast(), buf.len()) };
    if n < 0 { Err(io::Error::last_os_error()) } else { Ok(n as usize) }
}

/// A pipe whose write end a signal handler can poke (self-pipe trick for SIGCHLD).
pub fn signal_pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0; 2];
    // SAFETY: fds has room for two descriptors.
    cvt(unsafe { libc::pipe(fds.as_mut_ptr()) })?;
    // SAFETY: pipe returned two fresh descriptors.
    let (r, w) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    set_cloexec_nonblock(r.as_raw_fd(), true)?;
    set_cloexec_nonblock(w.as_raw_fd(), true)?;
    Ok((r, w))
}

static mut SIGNAL_FD: RawFd = -1;

extern "C" fn on_signal(sig: libc::c_int) {
    // SAFETY: write is async-signal-safe; SIGNAL_FD is set once before the handler is installed.
    unsafe {
        let fd = SIGNAL_FD;
        if fd >= 0 {
            let b = sig as u8;
            libc::write(fd, (&b as *const u8).cast(), 1);
        }
    }
}

/// Route SIGCHLD, SIGTERM and SIGHUP to a byte on `fd`, and ignore SIGPIPE.
pub fn install_signal_handlers(fd: RawFd) {
    // SAFETY: called once at startup, before other threads exist.
    unsafe {
        SIGNAL_FD = fd;
        for sig in [libc::SIGCHLD, libc::SIGTERM, libc::SIGHUP, libc::SIGINT] {
            libc::signal(sig, on_signal as *const () as libc::sighandler_t);
        }
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
}

/// Whether a process still exists.
pub fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks.
    unsafe { libc::kill(pid, 0) == 0 }
}

#[cfg(target_os = "linux")]
pub fn boot_time_secs() -> i64 {
    std::fs::read_to_string("/proc/stat")
        .ok()
        .and_then(|s| s.lines().find_map(|l| l.strip_prefix("btime ").and_then(|v| v.trim().parse().ok())))
        .unwrap_or(0)
}

#[cfg(target_os = "macos")]
pub fn boot_time_secs() -> i64 {
    let mut tv = libc::timeval { tv_sec: 0, tv_usec: 0 };
    let mut size = std::mem::size_of::<libc::timeval>();
    let mut mib = [libc::CTL_KERN, libc::KERN_BOOTTIME];
    // SAFETY: sysctl with a correctly sized out-buffer.
    unsafe {
        libc::sysctl(mib.as_mut_ptr(), 2, (&mut tv as *mut libc::timeval).cast(), &mut size, std::ptr::null_mut(), 0)
    };
    tv.tv_sec
}

/// A readiness loop over `poll(2)`. kqueue (what mio and polling use on macOS) doesn't report
/// readiness on terminal devices, so pseudo-terminals need plain poll, as tmux does.
#[derive(Debug, Default)]
pub struct Poller {
    fds: Vec<libc::pollfd>,
    keys: Vec<usize>,
}

impl Poller {
    pub fn new() -> Poller {
        Poller::default()
    }

    /// Watch `fd` for reading, and for writing when `write`.
    pub fn add(&mut self, fd: RawFd, key: usize, write: bool) {
        self.delete(fd);
        self.fds.push(libc::pollfd { fd, events: events(write), revents: 0 });
        self.keys.push(key);
    }

    pub fn modify(&mut self, fd: RawFd, write: bool) {
        if let Some(p) = self.fds.iter_mut().find(|p| p.fd == fd) {
            p.events = events(write);
        }
    }

    pub fn delete(&mut self, fd: RawFd) {
        if let Some(i) = self.fds.iter().position(|p| p.fd == fd) {
            self.fds.swap_remove(i);
            self.keys.swap_remove(i);
        }
    }

    /// Wait up to `timeout`; returns (key, readable, writable) for each ready descriptor.
    pub fn wait(&mut self, timeout: std::time::Duration) -> io::Result<Vec<(usize, bool, bool)>> {
        let ms = timeout.as_millis().min(i32::MAX as u128) as libc::c_int;
        // SAFETY: fds is a valid array of pollfd for its length.
        let n = unsafe { libc::poll(self.fds.as_mut_ptr(), self.fds.len() as libc::nfds_t, ms) };
        if n < 0 {
            let e = io::Error::last_os_error();
            return if e.kind() == io::ErrorKind::Interrupted { Ok(Vec::new()) } else { Err(e) };
        }
        let mut out = Vec::new();
        for (p, k) in self.fds.iter().zip(&self.keys) {
            if p.revents != 0 {
                let readable = p.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0;
                let writable = p.revents & libc::POLLOUT != 0;
                out.push((*k, readable, writable));
            }
        }
        Ok(out)
    }
}

fn events(write: bool) -> libc::c_short {
    if write { libc::POLLIN | libc::POLLOUT } else { libc::POLLIN }
}

/// Let a broken pipe end the program quietly, as command-line tools do (`ctx read … | head`).
pub fn default_sigpipe() {
    // SAFETY: setting a signal disposition at startup.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
}

#[cfg(target_os = "linux")]
pub fn release_free_memory() -> usize {
    // SAFETY: malloc_trim has no preconditions.
    unsafe { libc::malloc_trim(0) };
    0
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn malloc_zone_pressure_relief(zone: *mut libc::c_void, goal: libc::size_t) -> libc::size_t;
}

/// Hand memory the allocator holds but doesn't use back to the system (after a flood of output).
/// Returns the bytes released.
#[cfg(target_os = "macos")]
pub fn release_free_memory() -> usize {
    // SAFETY: a null zone means all zones; goal 0 means as much as possible.
    unsafe { malloc_zone_pressure_relief(std::ptr::null_mut(), 0) }
}
