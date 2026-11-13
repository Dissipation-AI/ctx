//! Accounts, the `ctx-agents` group and spaces: looking them up (anyone), and making and
//! removing them (root only; the helper does it).
//!
//! An agent is an account in the group [`AGENTS_GROUP`]. Agents' accounts are hidden from the
//! login window, standard (never administrators), have no password (nobody logs in as them;
//! the helper starts their processes), use `ctx-agents` as their primary group, and have their
//! home where every account's home is ([`home_of`]).
//!
//! A space is a folder some agents work in together: a group `ctx-space-<name>` (its members are
//! the agents in it) and a folder in [`spaces_dir`] that the group may read and write.

use std::ffi::{CStr, CString};
use std::process::Command;

/// The group every agent account is in. Being in it is what makes an account an agent.
pub const AGENTS_GROUP: &str = "ctx-agents";

/// Account names: lowercase letters, digits and `-`, starting with a letter, at most 31 long.
pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some('a'..='z'))
        && name.len() <= 31
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Where an account called `name` has its home: where every account's is.
pub fn home_of(name: &str) -> String {
    if cfg!(target_os = "macos") { format!("/Users/{name}") } else { format!("/home/{name}") }
}

/// The group name of a space.
pub const SPACE_PREFIX: &str = "ctx-space-";

/// Space names: as account names, at most 21 long (the group name is at most 31).
pub fn valid_space_name(name: &str) -> bool {
    valid_name(name) && name.len() <= 21 && name != "agents"
}

/// Where spaces' folders are: outside every home, so reaching one passes through no one's.
/// `CTX_SPACES_DIR` moves it (tests).
pub fn spaces_dir() -> std::path::PathBuf {
    if let Some(d) = std::env::var_os("CTX_SPACES_DIR").filter(|d| !d.is_empty()) {
        return d.into();
    }
    if cfg!(target_os = "macos") { "/Users/Shared/Spaces".into() } else { "/srv/spaces".into() }
}

/// A space: its name, group id, the agents in it and its folder.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Space {
    pub name: String,
    pub gid: u32,
    pub members: Vec<String>,
    pub folder: String,
}

pub fn space(name: &str) -> Option<Space> {
    let (gid, members) = group(&format!("{SPACE_PREFIX}{name}"))?;
    Some(Space { name: name.into(), gid, members, folder: spaces_dir().join(name).display().to_string() })
}

/// Every space, by name.
pub fn spaces() -> Vec<Space> {
    let mut names = Vec::new();
    // SAFETY: the getgrent walk; names copied out at once.
    unsafe {
        libc::setgrent();
        loop {
            let g = libc::getgrent();
            if g.is_null() {
                break;
            }
            let n = CStr::from_ptr((*g).gr_name).to_string_lossy().into_owned();
            if let Some(s) = n.strip_prefix(SPACE_PREFIX)
                && !names.iter().any(|x: &String| x == s)
            {
                names.push(s.to_string());
            }
        }
        libc::endgrent();
    }
    names.sort();
    names.iter().filter_map(|n| space(n)).collect()
}

/// The spaces `agent` is in.
pub fn spaces_of(agent: &str) -> Vec<Space> {
    spaces().into_iter().filter(|s| s.members.iter().any(|m| m == agent)).collect()
}

/// A space's group, made. Root only.
pub fn make_space_group(name: &str) -> Result<u32, String> {
    if !valid_space_name(name) {
        return Err(format!(
            "\"{name}\" can't be a space's name: up to 21 lowercase letters, digits and -, starting with a letter."
        ));
    }
    let g = format!("{SPACE_PREFIX}{name}");
    if group(&g).is_some() {
        return Err(format!("There's already a space \"{name}\"."));
    }
    if cfg!(target_os = "macos") {
        let gid = free_id(true)?;
        run("dseditgroup", &["-o", "create", "-r", &format!("CTX space {name}"), "-i", &gid.to_string(), &g])?;
    } else {
        run("groupadd", &["--system", &g])?;
    }
    appeared(|| group(&g).map(|(gid, _)| gid)).ok_or_else(|| format!("the group {g} didn't appear"))
}

pub fn remove_space_group(name: &str) -> Result<(), String> {
    let g = format!("{SPACE_PREFIX}{name}");
    if cfg!(target_os = "macos") {
        run("dseditgroup", &["-o", "delete", &g]).map(|_| ())
    } else {
        run("groupdel", &[&g]).map(|_| ())
    }
}

/// Put `agent` in the space or take it out. Root only.
pub fn set_space_member(name: &str, agent: &str, member: bool) -> Result<(), String> {
    let g = format!("{SPACE_PREFIX}{name}");
    if cfg!(target_os = "macos") {
        run("dseditgroup", &["-o", "edit", if member { "-a" } else { "-d" }, agent, "-t", "user", &g]).map(|_| ())
    } else {
        run("gpasswd", &[if member { "-a" } else { "-d" }, agent, &g]).map(|_| ())
    }
}

/// Names that are never agents.
const RESERVED: &[&str] = &["root", "admin", "daemon", "nobody", "you", "everyone", "staff", "wheel", "shared"];

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Account {
    pub name: String,
    pub uid: u32,
    pub gid: u32,
    pub home: String,
    pub shell: String,
    /// The full name (macOS RealName, Linux GECOS).
    pub full_name: String,
}

fn from_passwd(pw: *const libc::passwd) -> Option<Account> {
    if pw.is_null() {
        return None;
    }
    // SAFETY: a non-null passwd entry from getpw*; copied out at once.
    unsafe {
        let s = |p: *const libc::c_char| {
            if p.is_null() { String::new() } else { CStr::from_ptr(p).to_string_lossy().into_owned() }
        };
        let gecos = s((*pw).pw_gecos);
        Some(Account {
            name: s((*pw).pw_name),
            uid: (*pw).pw_uid,
            gid: (*pw).pw_gid,
            home: s((*pw).pw_dir),
            shell: s((*pw).pw_shell),
            full_name: gecos.split(',').next().unwrap_or("").to_string(),
        })
    }
}

pub fn by_name(name: &str) -> Option<Account> {
    let n = CString::new(name).ok()?;
    // SAFETY: getpwnam with a valid C string.
    from_passwd(unsafe { libc::getpwnam(n.as_ptr()) })
}

pub fn by_uid(uid: u32) -> Option<Account> {
    // SAFETY: getpwuid.
    from_passwd(unsafe { libc::getpwuid(uid) })
}

/// A group's id and its listed members (accounts whose primary group it is aren't listed).
pub fn group(name: &str) -> Option<(u32, Vec<String>)> {
    let n = CString::new(name).ok()?;
    // SAFETY: getgrnam; the entry is copied out at once.
    unsafe {
        let g = libc::getgrnam(n.as_ptr());
        if g.is_null() {
            return None;
        }
        let mut members = Vec::new();
        // macOS packs the entry into one buffer, so `gr_mem` may not be pointer-aligned.
        let mut m = (*g).gr_mem;
        while !m.is_null() {
            let p = m.read_unaligned();
            if p.is_null() {
                break;
            }
            members.push(CStr::from_ptr(p).to_string_lossy().into_owned());
            m = m.add(1);
        }
        Some(((*g).gr_gid, members))
    }
}

/// Whether `account` is an agent: listed in `ctx-agents`, or has it as its primary group.
pub fn is_agent(account: &Account) -> bool {
    match group(AGENTS_GROUP) {
        Some((gid, members)) => account.gid == gid || members.iter().any(|m| m == &account.name),
        None => false,
    }
}

/// Every agent account, by name.
pub fn agents() -> Vec<Account> {
    let Some((gid, members)) = group(AGENTS_GROUP) else { return Vec::new() };
    let mut out: Vec<Account> = members.iter().filter_map(|m| by_name(m)).collect();
    // Accounts whose primary group it is.
    // SAFETY: the getpwent walk, single-threaded use; entries copied out at once.
    unsafe {
        libc::setpwent();
        loop {
            let pw = libc::getpwent();
            if pw.is_null() {
                break;
            }
            if (*pw).pw_gid == gid
                && let Some(a) = from_passwd(pw)
                && !out.iter().any(|o| o.name == a.name)
            {
                out.push(a);
            }
        }
        libc::endpwent();
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Whether an account is an administrator (macOS group `admin`, Linux `sudo` or `wheel`).
pub fn is_admin(name: &str) -> bool {
    ["admin", "sudo", "wheel"].iter().any(|g| group(g).is_some_and(|(_, m)| m.iter().any(|x| x == name)))
}

fn run(program: &str, args: &[&str]) -> Result<String, String> {
    let o = Command::new(program).args(args).output().map_err(|e| format!("couldn't run {program}: {e}"))?;
    if o.status.success() {
        Ok(String::from_utf8_lossy(&o.stdout).into_owned())
    } else {
        Err(format!("{program} {}: {}", args.join(" "), String::from_utf8_lossy(&o.stderr).trim()))
    }
}

/// The `ctx-agents` group, made if it's missing. Root only.
pub fn ensure_agents_group() -> Result<u32, String> {
    if let Some((gid, _)) = group(AGENTS_GROUP) {
        return Ok(gid);
    }
    if cfg!(target_os = "macos") {
        let gid = free_id(true)?;
        run("dseditgroup", &["-o", "create", "-r", "CTX agents", "-i", &gid.to_string(), AGENTS_GROUP])?;
    } else {
        run("groupadd", &["--system", AGENTS_GROUP])?;
    }
    appeared(|| group(AGENTS_GROUP).map(|(g, _)| g)).ok_or_else(|| "the ctx-agents group didn't appear".into())
}

/// An unused id from 600 up (below macOS's 501+ people range is taken by the system; 600-899 is
/// left alone by both systems' account tools).
#[cfg(target_os = "macos")]
fn free_id(for_group: bool) -> Result<u32, String> {
    let (path, key) = if for_group { ("/Groups", "PrimaryGroupID") } else { ("/Users", "UniqueID") };
    let used: Vec<u32> = run("dscl", &[".", "-list", path, key])?
        .lines()
        .filter_map(|l| l.split_whitespace().last()?.parse().ok())
        .collect();
    (600..900).find(|i| !used.contains(i)).ok_or_else(|| "no free id between 600 and 899".into())
}

#[cfg(not(target_os = "macos"))]
fn free_id(_for_group: bool) -> Result<u32, String> {
    Err("not used on Linux".into())
}

/// Make a new agent account. Root only. The home folder itself is made by the caller.
pub fn make(name: &str, full_name: &str) -> Result<Account, String> {
    let home = home_of(name);
    let home = home.as_str();
    if !valid_name(name) || RESERVED.contains(&name) {
        return Err(format!(
            "\"{name}\" can't be an agent's name: use lowercase letters, digits and -, starting with a letter."
        ));
    }
    if by_name(name).is_some() {
        return Err(format!("There's already an account \"{name}\"; choose another name."));
    }
    let gid = ensure_agents_group()?;
    let full = if full_name.is_empty() { name } else { full_name };
    if cfg!(target_os = "macos") {
        let uid = free_id(false)?.to_string();
        let user = format!("/Users/{name}");
        let set = |k: &str, v: &str| run("dscl", &[".", "-create", &user, k, v]);
        run("dscl", &[".", "-create", &user])?;
        let r = (|| {
            set("UniqueID", &uid)?;
            set("PrimaryGroupID", &gid.to_string())?;
            set("UserShell", "/bin/zsh")?;
            set("NFSHomeDirectory", home)?;
            set("RealName", full)?;
            set("IsHidden", "1")?;
            // No password: nobody logs in as an agent.
            set("Password", "*")?;
            run("dseditgroup", &["-o", "edit", "-a", name, "-t", "user", AGENTS_GROUP]).map(|_| ())
        })();
        if let Err(e) = r {
            let _ = run("dscl", &[".", "-delete", &user]);
            return Err(e);
        }
    } else {
        run(
            "useradd",
            &[
                "--system",
                "--no-create-home",
                "--home-dir",
                home,
                "--shell",
                "/bin/bash",
                "--gid",
                AGENTS_GROUP,
                "--comment",
                full,
                name,
            ],
        )?;
        let _ = run("usermod", &["-aG", AGENTS_GROUP, name]);
    }
    appeared(|| by_name(name)).ok_or_else(|| format!("the account \"{name}\" didn't appear"))
}

/// What `look` finds once a new account or group shows up. macOS's directory service can take a
/// moment to answer for a record it has just made (its lookup cache), so this flushes the cache
/// and asks again for up to ten seconds.
fn appeared<T>(look: impl Fn() -> Option<T>) -> Option<T> {
    let start = std::time::Instant::now();
    loop {
        if let Some(t) = look() {
            return Some(t);
        }
        if start.elapsed() > std::time::Duration::from_secs(10) {
            return None;
        }
        if cfg!(target_os = "macos") {
            let _ = Command::new("dscacheutil").arg("-flushcache").status();
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

/// Make an existing standard account an agent. Root only.
pub fn remove(name: &str) -> Result<(), String> {
    let a = by_name(name).ok_or_else(|| format!("There's no account \"{name}\"."))?;
    if !is_agent(&a) {
        return Err(format!("\"{name}\" isn't an agent's account; ctx removes only those."));
    }
    if cfg!(target_os = "macos") {
        let _ = run("dseditgroup", &["-o", "edit", "-d", name, "-t", "user", AGENTS_GROUP]);
        run("dscl", &[".", "-delete", &format!("/Users/{name}")])?;
    } else {
        run("userdel", &[name])?;
    }
    Ok(())
}

/// Change the full name shown beside the account name. Root only.
pub fn set_full_name(name: &str, full_name: &str) -> Result<(), String> {
    if cfg!(target_os = "macos") {
        run("dscl", &[".", "-create", &format!("/Users/{name}"), "RealName", full_name]).map(|_| ())
    } else {
        run("usermod", &["--comment", full_name, name]).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert!(valid_name("email-agent"));
        assert!(valid_name("a1"));
        assert!(!valid_name("Email"));
        assert!(!valid_name("1a"));
        assert!(!valid_name("a b"));
        assert!(!valid_name(""));
        assert!(!valid_name(&"a".repeat(32)));
    }

    #[test]
    fn space_names_fit_their_group() {
        assert!(valid_space_name("research"));
        assert!(!valid_space_name("agents"));
        assert!(!valid_space_name(&"a".repeat(22)));
        assert!((SPACE_PREFIX.len() + 21) <= 31);
    }

    #[test]
    fn lookups() {
        let me = by_uid(unsafe { libc::getuid() }).unwrap();
        assert_eq!(by_name(&me.name).unwrap().uid, me.uid);
        assert!(by_name("no-such-account-ctx").is_none());
        assert!(!is_agent(&me) || group(AGENTS_GROUP).is_some());
    }
}
