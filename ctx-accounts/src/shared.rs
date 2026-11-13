//! Shared folders in `$CTX_AGENTS/Shared`: `everyone` for every agent, and `a+b` for a few.

use crate::acl::{self, Access, Who};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// Make `<agents>/Shared/<a+b…>` (names sorted) and let each uid read and write it. Run as its
/// owner (the person).
pub fn make(agents: &Path, names: &[String], uids: &[u32]) -> Result<PathBuf, String> {
    if names.is_empty() {
        return Err("A shared folder needs at least one agent.".into());
    }
    let mut sorted = names.to_vec();
    sorted.sort();
    let dir = agents.join("Shared").join(sorted.join("+"));
    std::fs::create_dir_all(&dir).map_err(|e| format!("couldn't make {}: {e}", dir.display()))?;
    {
        // Only its agents (through the access list) and you; on Linux new files keep the folder's
        // group.
        use std::os::unix::fs::PermissionsExt;
        let mode = if cfg!(target_os = "macos") { 0o700 } else { 0o2770 };
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(mode));
    }
    for &u in uids {
        acl::apply(&dir, Who::User(u), Some(Access::Write))
            .map_err(|e| format!("couldn't share {}: {e}", dir.display()))?;
        pass_through(&dir, Who::User(u));
    }
    Ok(dir)
}

/// Let `who` pass through the folders above `path` that the caller owns, toward `path` only:
/// everything else in those folders is closed to it (see [`acl::pass_only`]). Entries that name an
/// agent or the agents' group already are someone's grant and are left as they are.
pub fn pass_through(path: &Path, who: Who) {
    // SAFETY: getuid.
    let mine = unsafe { libc::getuid() };
    let agents_gid = crate::accounts::group(crate::accounts::AGENTS_GROUP).map(|(g, _)| g);
    let spare = |w: Who| match w {
        Who::Group(g) => Some(g) == agents_gid,
        Who::User(u) => crate::accounts::by_uid(u).is_some_and(|a| crate::accounts::is_agent(&a)),
    };
    let mut child = path;
    while let Some(p) = child.parent() {
        match std::fs::symlink_metadata(p) {
            Ok(m) if m.uid() == mine && m.is_dir() => {
                if let Some(name) = child.file_name() {
                    let _ = acl::pass_only(p, name, who, &spare);
                }
            }
            _ => break,
        }
        child = p;
    }
}

/// Take `who` off the way to `path`: every entry for it on the folders of the caller's above
/// `path` and on their other entries (for an account that's going away).
pub fn forget_passing(path: &Path, who: Who) {
    // SAFETY: getuid.
    let mine = unsafe { libc::getuid() };
    let mut child = path;
    while let Some(p) = child.parent() {
        match std::fs::symlink_metadata(p) {
            Ok(m) if m.uid() == mine && m.is_dir() => {
                let _ = acl::apply_one(p, who, None, false);
                for e in std::fs::read_dir(p).into_iter().flatten().flatten() {
                    let q = e.path();
                    if q != child
                        && std::fs::symlink_metadata(&q).is_ok_and(|m| !m.file_type().is_symlink() && m.uid() == mine)
                    {
                        let _ = acl::apply_one(&q, who, None, false);
                    }
                }
            }
            _ => break,
        }
        child = p;
    }
}
