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
    #[cfg(not(target_os = "macos"))]
    {
        // New files keep the folder's group.
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o2770));
    }
    for &u in uids {
        acl::apply(&dir, Who::User(u), Some(Access::Write))
            .map_err(|e| format!("couldn't share {}: {e}", dir.display()))?;
        pass_through(&dir, Who::User(u));
    }
    Ok(dir)
}

/// Let `who` pass through the folders above `path` that the caller owns.
pub fn pass_through(path: &Path, who: Who) {
    // SAFETY: getuid.
    let mine = unsafe { libc::getuid() };
    let mut up = path.parent();
    while let Some(p) = up {
        match std::fs::symlink_metadata(p) {
            Ok(m) if m.uid() == mine && m.is_dir() => {
                let _ = acl::allow_passing(p, who);
            }
            _ => break,
        }
        up = p.parent();
    }
}
