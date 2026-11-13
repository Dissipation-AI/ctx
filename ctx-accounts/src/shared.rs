//! The way to something shared: the folders above it are opened for passing through, toward it
//! only.

use crate::acl::{self, Who};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

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
