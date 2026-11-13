//! Accounts and the `ctx-agents` group: looking them up (anyone), and making, adopting and
//! removing them (root only; the helper does it).
//!
//! An agent is an account in the group [`AGENTS_GROUP`]. Agents' accounts are hidden from the
//! login window, standard (never administrators), have no password (nobody logs in as them;
//! the helper starts their processes), and use `ctx-agents` as their primary group.

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
        let mut m = (*g).gr_mem;
        while !m.is_null() && !(*m).is_null() {
            members.push(CStr::from_ptr(*m).to_string_lossy().into_owned());
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
    group(AGENTS_GROUP).map(|(g, _)| g).ok_or_else(|| "the ctx-agents group didn't appear".into())
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
pub fn make(name: &str, full_name: &str, home: &str) -> Result<Account, String> {
    if !valid_name(name) || RESERVED.contains(&name) {
        return Err(format!(
            "\"{name}\" can't be an agent's name: use lowercase letters, digits and -, starting with a letter."
        ));
    }
    if by_name(name).is_some() {
        return Err(format!(
            "There's already an account \"{name}\". To make it an agent: ctx make agent from account \"{name}\"."
        ));
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
    by_name(name).ok_or_else(|| format!("the account \"{name}\" didn't appear"))
}

/// Make an existing standard account an agent. Root only.
pub fn adopt(name: &str) -> Result<Account, String> {
    let a = by_name(name).ok_or_else(|| format!("There's no account \"{name}\"."))?;
    if a.uid == 0 || is_admin(name) {
        return Err(format!(
            "\"{name}\" is an administrator. An agent's account is standard; make it standard first, or make a new agent."
        ));
    }
    ensure_agents_group()?;
    if cfg!(target_os = "macos") {
        run("dseditgroup", &["-o", "edit", "-a", name, "-t", "user", AGENTS_GROUP])?;
    } else {
        run("usermod", &["-aG", AGENTS_GROUP, name])?;
    }
    Ok(a)
}

/// Delete an agent account (not its home). Root only.
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
    fn lookups() {
        let me = by_uid(unsafe { libc::getuid() }).unwrap();
        assert_eq!(by_name(&me.name).unwrap().uid, me.uid);
        assert!(by_name("no-such-account-ctx").is_none());
        assert!(!is_agent(&me) || group(AGENTS_GROUP).is_some());
    }
}
