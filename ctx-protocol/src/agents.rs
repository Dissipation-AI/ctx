//! Agents: the principals other than you. Each is an account in `ctx-agents` (see
//! `ctx-accounts`); the activity log names it, and it sees and drives only the tabs the OS lets
//! its account reach.

use std::path::{Path, PathBuf};

/// The principal for you: CTX.app, and every process that runs as your account.
pub const YOU: &str = "you";

/// A path with its symbolic links resolved (`/tmp` is `/private/tmp`), and for a path
/// that doesn't exist yet, its nearest existing parent resolved.
pub fn real(path: &Path) -> PathBuf {
    if let Ok(p) = std::fs::canonicalize(path) {
        return p;
    }
    let mut rest = Vec::new();
    let mut p = path.to_path_buf();
    while let Some(parent) = p.parent().map(Path::to_path_buf) {
        if let Some(name) = p.file_name() {
            rest.push(name.to_os_string());
        }
        if let Ok(mut r) = std::fs::canonicalize(&parent) {
            for n in rest.iter().rev() {
                r.push(n);
            }
            return r;
        }
        p = parent;
    }
    path.to_path_buf()
}
