//! Access lists: who besides a file's owner may read or write it.
//!
//! ctx expresses every grant as one of four states for an account or a group on a file or folder
//! and everything inside it ([`Access`]). Folders also pass the entry on to what's made inside
//! them later. macOS uses its extended access lists through the system's `acl_*` calls (the
//! same entries `chmod +a` makes and Finder shows); Linux uses POSIX access lists through
//! `setfacl`.
//!
//! Only a file's owner (or root) may change its access list. [`apply`] changes what it can and
//! returns the paths it couldn't, so the caller can ask the helper about those.

use std::path::{Path, PathBuf};

/// Who an entry is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Who {
    User(u32),
    Group(u32),
}

/// What an account or group may do with a file or folder and what's inside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Access {
    /// Read files, list folders and pass through them.
    Read,
    /// Read, and also write, make and delete inside.
    Write,
    /// Read, and never write even where the file's own mode would allow it.
    ReadOnly,
    /// Nothing, even where the file's own mode would allow it (a hole inside a shared folder).
    Nothing,
}

/// What [`apply`] did.
#[derive(Debug, Default)]
pub struct Applied {
    /// Files and folders whose access list now says what was asked.
    pub changed: usize,
    /// Ones it couldn't change because someone else owns them.
    pub not_owned: Vec<PathBuf>,
}

/// Set `who`'s access to `path` and, for a folder, everything inside it (links aren't
/// followed). `None` removes `who`'s entries instead. Errors stop only for `path` itself.
pub fn apply(path: &Path, who: Who, access: Option<Access>) -> std::io::Result<Applied> {
    apply_counting(path, who, access, &mut |_| {})
}

/// [`apply`], calling `seen` with the number of files and folders done so far after each one
/// (a large tree takes minutes).
pub fn apply_counting(
    path: &Path,
    who: Who,
    access: Option<Access>,
    seen: &mut dyn FnMut(usize),
) -> std::io::Result<Applied> {
    let mut done = Applied::default();
    imp::set(path, who, access, false)?;
    done.changed += 1;
    let meta = std::fs::symlink_metadata(path)?;
    if meta.is_dir() {
        let mut n = 1;
        walk(path, &mut |p| {
            match imp::set(p, who, access, true) {
                Ok(()) => done.changed += 1,
                Err(_) => done.not_owned.push(p.to_path_buf()),
            }
            n += 1;
            seen(n);
        });
    }
    Ok(done)
}

/// Set `who`'s access on `path` alone (not what's inside). `inside` marks the entry as passed on
/// from a parent, as the OS marks entries a new file takes from its folder.
pub fn apply_one(path: &Path, who: Who, access: Option<Access>, inside: bool) -> std::io::Result<()> {
    imp::set(path, who, access, inside)
}

/// Every file and folder under `dir` (not `dir` itself), depth first, without following links.
pub fn each_inside(dir: &Path, f: &mut dyn FnMut(&Path)) {
    walk(dir, f)
}

/// Let `who` pass through `folder` (search, no listing), without passing it on to what's inside.
pub fn allow_passing(folder: &Path, who: Who) -> std::io::Result<()> {
    imp::allow_passing(folder, who)
}

/// Let `who` pass through `folder` to its entry `toward` and to nothing else in it.
///
/// A folder `who` may pass through lets it reach, by name, anything inside that other accounts
/// may read (most files are made readable to everyone), so passing alone would open far more
/// than what was shared. Here `who` gets: search on `folder` (unless it has a grant there
/// already); an explicit "nothing" on each other entry of `folder` that's the caller's and whose
/// access list names neither `who` nor anyone `spare` picks (other agents' grants are left as
/// they are); and the same "nothing" on entries made in `folder` later.
pub fn pass_only(
    folder: &Path,
    toward: &std::ffi::OsStr,
    who: Who,
    spare: &dyn Fn(Who) -> bool,
) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    imp::pass_here(folder, who)?;
    // SAFETY: getuid.
    let mine = unsafe { libc::getuid() };
    for e in std::fs::read_dir(folder)?.flatten() {
        if e.file_name() == toward {
            continue;
        }
        let p = e.path();
        let Ok(m) = std::fs::symlink_metadata(&p) else { continue };
        if m.file_type().is_symlink() || m.uid() != mine {
            continue;
        }
        let named = imp::named_on(&p).unwrap_or_default();
        if named.iter().any(|w| *w == who || spare(*w)) {
            continue;
        }
        imp::set(&p, who, Some(Access::Nothing), false)?;
    }
    Ok(())
}

/// `who`'s access to `path` as its access list states it (`None`: no entry for `who`).
pub fn access_of(path: &Path, who: Who) -> std::io::Result<Option<Access>> {
    imp::access_of(path, who)
}

/// Everyone `path`'s access list names, with their access (entries that only let them pass
/// through a folder are left out).
pub fn entries(path: &Path) -> std::io::Result<Vec<(Who, Access)>> {
    imp::entries(path)
}

/// Every file and folder under `dir` (not `dir` itself), depth first, without following links.
fn walk(dir: &Path, f: &mut dyn FnMut(&Path)) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        f(&p);
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            walk(&p, f);
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::{Access, Who};
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    type AclT = *mut libc::c_void;
    type EntryT = *mut libc::c_void;
    type PermsetT = *mut libc::c_void;
    type FlagsetT = *mut libc::c_void;

    const ACL_TYPE_EXTENDED: libc::c_int = 0x100;
    const ACL_EXTENDED_ALLOW: libc::c_int = 1;
    const ACL_EXTENDED_DENY: libc::c_int = 2;
    const ACL_FIRST_ENTRY: libc::c_int = 0;
    const ACL_NEXT_ENTRY: libc::c_int = -1;

    // Permissions (a folder's list/add_file/search/add_subdirectory are the same bits as a file's
    // read/write/execute/append).
    const READ_DATA: u32 = 1 << 1;
    const WRITE_DATA: u32 = 1 << 2;
    const EXECUTE: u32 = 1 << 3;
    const DELETE: u32 = 1 << 4;
    const APPEND_DATA: u32 = 1 << 5;
    const DELETE_CHILD: u32 = 1 << 6;
    const READ_ATTRIBUTES: u32 = 1 << 7;
    const WRITE_ATTRIBUTES: u32 = 1 << 8;
    const READ_EXTATTRIBUTES: u32 = 1 << 9;
    const WRITE_EXTATTRIBUTES: u32 = 1 << 10;
    const READ_SECURITY: u32 = 1 << 11;
    const ALL_PERMS: [u32; 11] = [
        READ_DATA,
        WRITE_DATA,
        EXECUTE,
        DELETE,
        APPEND_DATA,
        DELETE_CHILD,
        READ_ATTRIBUTES,
        WRITE_ATTRIBUTES,
        READ_EXTATTRIBUTES,
        WRITE_EXTATTRIBUTES,
        READ_SECURITY,
    ];
    const READ: u32 = READ_DATA | EXECUTE | READ_ATTRIBUTES | READ_EXTATTRIBUTES | READ_SECURITY;
    const WRITE: u32 = WRITE_DATA | APPEND_DATA | DELETE | DELETE_CHILD | WRITE_ATTRIBUTES | WRITE_EXTATTRIBUTES;

    const ENTRY_INHERITED: libc::c_int = 1 << 4;
    const ENTRY_FILE_INHERIT: libc::c_int = 1 << 5;
    const ENTRY_DIRECTORY_INHERIT: libc::c_int = 1 << 6;
    const ENTRY_ONLY_INHERIT: libc::c_int = 1 << 8;

    const ID_TYPE_UID: libc::c_int = 0;
    const ID_TYPE_GID: libc::c_int = 1;

    unsafe extern "C" {
        fn acl_get_link_np(path: *const libc::c_char, ty: libc::c_int) -> AclT;
        fn acl_set_link_np(path: *const libc::c_char, ty: libc::c_int, acl: AclT) -> libc::c_int;
        fn acl_init(count: libc::c_int) -> AclT;
        fn acl_free(p: *mut libc::c_void) -> libc::c_int;
        fn acl_get_entry(acl: AclT, id: libc::c_int, entry: *mut EntryT) -> libc::c_int;
        fn acl_create_entry_np(acl: *mut AclT, entry: *mut EntryT, index: libc::c_int) -> libc::c_int;
        fn acl_delete_entry(acl: AclT, entry: EntryT) -> libc::c_int;
        fn acl_get_tag_type(entry: EntryT, tag: *mut libc::c_int) -> libc::c_int;
        fn acl_set_tag_type(entry: EntryT, tag: libc::c_int) -> libc::c_int;
        fn acl_get_qualifier(entry: EntryT) -> *mut libc::c_void;
        fn acl_set_qualifier(entry: EntryT, q: *const libc::c_void) -> libc::c_int;
        fn acl_get_permset(entry: EntryT, p: *mut PermsetT) -> libc::c_int;
        fn acl_set_permset(entry: EntryT, p: PermsetT) -> libc::c_int;
        fn acl_clear_perms(p: PermsetT) -> libc::c_int;
        fn acl_add_perm(p: PermsetT, perm: u32) -> libc::c_int;
        fn acl_get_perm_np(p: PermsetT, perm: u32) -> libc::c_int;
        fn acl_get_flagset_np(obj: *mut libc::c_void, f: *mut FlagsetT) -> libc::c_int;
        fn acl_set_flagset_np(obj: *mut libc::c_void, f: FlagsetT) -> libc::c_int;
        fn acl_clear_flags_np(f: FlagsetT) -> libc::c_int;
        fn acl_add_flag_np(f: FlagsetT, flag: libc::c_int) -> libc::c_int;
        fn acl_get_flag_np(f: FlagsetT, flag: libc::c_int) -> libc::c_int;
        fn mbr_uid_to_uuid(uid: libc::uid_t, uu: *mut u8) -> libc::c_int;
        fn mbr_gid_to_uuid(gid: libc::gid_t, uu: *mut u8) -> libc::c_int;
        fn mbr_uuid_to_id(uu: *const u8, id: *mut libc::id_t, ty: *mut libc::c_int) -> libc::c_int;
    }

    fn c(path: &Path) -> std::io::Result<CString> {
        CString::new(path.as_os_str().as_bytes()).map_err(|_| std::io::ErrorKind::InvalidInput.into())
    }

    fn uuid_of(who: Who) -> std::io::Result<[u8; 16]> {
        let mut u = [0u8; 16];
        // SAFETY: a 16-byte buffer for the uuid.
        let r = unsafe {
            match who {
                Who::User(uid) => mbr_uid_to_uuid(uid, u.as_mut_ptr()),
                Who::Group(gid) => mbr_gid_to_uuid(gid, u.as_mut_ptr()),
            }
        };
        if r != 0 { Err(std::io::Error::from_raw_os_error(r)) } else { Ok(u) }
    }

    fn who_of(entry: EntryT) -> Option<Who> {
        // SAFETY: the qualifier is a guid_t owned by us until acl_free.
        unsafe {
            let q = acl_get_qualifier(entry);
            if q.is_null() {
                return None;
            }
            let mut id: libc::id_t = 0;
            let mut ty: libc::c_int = -1;
            let r = mbr_uuid_to_id(q as *const u8, &mut id, &mut ty);
            acl_free(q);
            match (r, ty) {
                (0, ID_TYPE_UID) => Some(Who::User(id)),
                (0, ID_TYPE_GID) => Some(Who::Group(id)),
                _ => None,
            }
        }
    }

    /// The file's extended access list, or a new empty one.
    fn get(p: &CString) -> std::io::Result<AclT> {
        // SAFETY: a valid path; null means none (ENOENT) or an error.
        let acl = unsafe { acl_get_link_np(p.as_ptr(), ACL_TYPE_EXTENDED) };
        if !acl.is_null() {
            return Ok(acl);
        }
        let e = std::io::Error::last_os_error();
        if e.raw_os_error() != Some(libc::ENOENT) {
            // A file that doesn't exist is ENOENT too; check that separately.
            return Err(e);
        }
        if std::fs::symlink_metadata(std::ffi::OsStr::from_bytes(p.as_bytes())).is_err() {
            return Err(std::io::ErrorKind::NotFound.into());
        }
        // SAFETY: plain allocation.
        let acl = unsafe { acl_init(1) };
        if acl.is_null() { Err(std::io::Error::last_os_error()) } else { Ok(acl) }
    }

    /// Remove every entry for `who`.
    fn strip(acl: AclT, who: Who) {
        // SAFETY: walking a live acl; deleting restarts the walk.
        unsafe {
            loop {
                let mut entry: EntryT = std::ptr::null_mut();
                let mut id = ACL_FIRST_ENTRY;
                let mut found = None;
                while acl_get_entry(acl, id, &mut entry) == 0 {
                    id = ACL_NEXT_ENTRY;
                    if who_of(entry) == Some(who) {
                        found = Some(entry);
                        break;
                    }
                }
                match found {
                    Some(e) => {
                        acl_delete_entry(acl, e);
                    }
                    None => break,
                }
            }
        }
    }

    /// Add an entry at the front (explicit entries come before inherited ones).
    fn add(acl: &mut AclT, who: Who, allow: bool, perms: u32, dir: bool, inherited: bool) -> std::io::Result<()> {
        let uuid = uuid_of(who)?;
        // SAFETY: creating and filling an entry of a live acl.
        unsafe {
            let mut e: EntryT = std::ptr::null_mut();
            if acl_create_entry_np(acl, &mut e, ACL_FIRST_ENTRY) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            acl_set_tag_type(e, if allow { ACL_EXTENDED_ALLOW } else { ACL_EXTENDED_DENY });
            acl_set_qualifier(e, uuid.as_ptr().cast());
            let mut ps: PermsetT = std::ptr::null_mut();
            acl_get_permset(e, &mut ps);
            acl_clear_perms(ps);
            for bit in ALL_PERMS {
                if perms & bit != 0 {
                    acl_add_perm(ps, bit);
                }
            }
            acl_set_permset(e, ps);
            let mut fs: FlagsetT = std::ptr::null_mut();
            acl_get_flagset_np(e, &mut fs);
            acl_clear_flags_np(fs);
            if dir {
                acl_add_flag_np(fs, ENTRY_FILE_INHERIT);
                acl_add_flag_np(fs, ENTRY_DIRECTORY_INHERIT);
            }
            if inherited {
                acl_add_flag_np(fs, ENTRY_INHERITED);
            }
            acl_set_flagset_np(e, fs);
        }
        Ok(())
    }

    fn write(p: &CString, acl: AclT) -> std::io::Result<()> {
        // SAFETY: a valid path and acl.
        if unsafe { acl_set_link_np(p.as_ptr(), ACL_TYPE_EXTENDED, acl) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    pub fn set(path: &Path, who: Who, access: Option<Access>, inherited: bool) -> std::io::Result<()> {
        let p = c(path)?;
        let dir = std::fs::symlink_metadata(path)?.is_dir();
        let mut acl = get(&p)?;
        strip(acl, who);
        let r = (|| {
            match access {
                None => {}
                Some(Access::Read) => add(&mut acl, who, true, READ, dir, inherited)?,
                Some(Access::Write) => add(&mut acl, who, true, READ | WRITE, dir, inherited)?,
                Some(Access::ReadOnly) => {
                    add(&mut acl, who, true, READ, dir, inherited)?;
                    // Deny comes first.
                    add(&mut acl, who, false, WRITE, dir, inherited)?;
                }
                Some(Access::Nothing) => add(&mut acl, who, false, READ | WRITE, dir, inherited)?,
            }
            write(&p, acl)
        })();
        // SAFETY: freeing the acl we got or made.
        unsafe { acl_free(acl) };
        r
    }

    pub fn allow_passing(folder: &Path, who: Who) -> std::io::Result<()> {
        let p = c(folder)?;
        let mut acl = get(&p)?;
        // Keep any stronger entry for `who`; add search only when there's none.
        let r = (|| {
            if entries_for(acl, who).is_empty() {
                add(&mut acl, who, true, EXECUTE, false, false)?;
                write(&p, acl)?;
            }
            Ok(())
        })();
        // SAFETY: freeing the acl.
        unsafe { acl_free(acl) };
        r
    }

    /// Search on `folder` for `who`, and "nothing" passed on to what's made in it later (an
    /// entry that applies only to new entries, not the folder). A grant `who` has on the folder
    /// is kept as it is.
    pub fn pass_here(folder: &Path, who: Who) -> std::io::Result<()> {
        let p = c(folder)?;
        let mut acl = get(&p)?;
        let r = (|| {
            if classify(&entries_for(acl, who)).is_some() {
                return Ok(());
            }
            strip(acl, who);
            add(&mut acl, who, true, EXECUTE, false, false)?;
            add(&mut acl, who, false, READ | WRITE, true, false)?;
            // The deny is for what's made inside only.
            // SAFETY: the entry just added is first; setting a flag on it.
            unsafe {
                let mut e: EntryT = std::ptr::null_mut();
                if acl_get_entry(acl, ACL_FIRST_ENTRY, &mut e) == 0 {
                    let mut fs: FlagsetT = std::ptr::null_mut();
                    acl_get_flagset_np(e, &mut fs);
                    acl_add_flag_np(fs, ENTRY_ONLY_INHERIT);
                    acl_set_flagset_np(e, fs);
                }
            }
            write(&p, acl)
        })();
        // SAFETY: freeing the acl.
        unsafe { acl_free(acl) };
        r
    }

    /// Everyone `path`'s access list names (any entry).
    pub fn named_on(path: &Path) -> std::io::Result<Vec<Who>> {
        let p = c(path)?;
        let acl = get(&p)?;
        let out = named(acl);
        // SAFETY: freeing the acl.
        unsafe { acl_free(acl) };
        Ok(out)
    }

    /// `(allow, perms)` of each entry for `who` that applies to the file itself (entries only
    /// passed on to new entries of a folder are left out).
    fn entries_for(acl: AclT, who: Who) -> Vec<(bool, u32)> {
        let mut out = Vec::new();
        // SAFETY: walking a live acl.
        unsafe {
            let mut entry: EntryT = std::ptr::null_mut();
            let mut id = ACL_FIRST_ENTRY;
            while acl_get_entry(acl, id, &mut entry) == 0 {
                id = ACL_NEXT_ENTRY;
                if who_of(entry) != Some(who) {
                    continue;
                }
                let mut fs: FlagsetT = std::ptr::null_mut();
                acl_get_flagset_np(entry, &mut fs);
                if acl_get_flag_np(fs, ENTRY_ONLY_INHERIT) == 1 {
                    continue;
                }
                let mut tag = 0;
                acl_get_tag_type(entry, &mut tag);
                let mut ps: PermsetT = std::ptr::null_mut();
                acl_get_permset(entry, &mut ps);
                let perms = ALL_PERMS.iter().filter(|b| acl_get_perm_np(ps, **b) == 1).fold(0, |a, b| a | b);
                out.push((tag == ACL_EXTENDED_ALLOW, perms));
            }
        }
        out
    }

    /// Everyone named, in order of first appearance.
    fn named(acl: AclT) -> Vec<Who> {
        let mut out = Vec::new();
        // SAFETY: walking a live acl.
        unsafe {
            let mut entry: EntryT = std::ptr::null_mut();
            let mut id = ACL_FIRST_ENTRY;
            while acl_get_entry(acl, id, &mut entry) == 0 {
                id = ACL_NEXT_ENTRY;
                if let Some(w) = who_of(entry)
                    && !out.contains(&w)
                {
                    out.push(w);
                }
            }
        }
        out
    }

    pub fn entries(path: &Path) -> std::io::Result<Vec<(Who, Access)>> {
        let p = c(path)?;
        let acl = get(&p)?;
        let out = named(acl).into_iter().filter_map(|w| classify(&entries_for(acl, w)).map(|a| (w, a))).collect();
        // SAFETY: freeing the acl.
        unsafe { acl_free(acl) };
        Ok(out)
    }

    pub fn access_of(path: &Path, who: Who) -> std::io::Result<Option<Access>> {
        let p = c(path)?;
        let acl = get(&p)?;
        let es = entries_for(acl, who);
        // SAFETY: freeing the acl.
        unsafe { acl_free(acl) };
        Ok(classify(&es))
    }

    fn classify(es: &[(bool, u32)]) -> Option<Access> {
        let allowed = es.iter().filter(|(a, _)| *a).fold(0, |a, (_, p)| a | p);
        let denied = es.iter().filter(|(a, _)| !*a).fold(0, |a, (_, p)| a | p);
        if es.is_empty() {
            None
        } else if denied & READ_DATA != 0 {
            Some(Access::Nothing)
        } else if allowed & WRITE_DATA != 0 && denied & WRITE_DATA == 0 {
            Some(Access::Write)
        } else if denied & WRITE_DATA != 0 {
            Some(Access::ReadOnly)
        } else if allowed & READ_DATA != 0 {
            Some(Access::Read)
        } else {
            // Only search on a folder (a parent of something shared): not a grant of its own.
            None
        }
    }

    #[allow(dead_code)]
    fn flag(fs: FlagsetT, f: libc::c_int) -> bool {
        // SAFETY: a live flagset.
        unsafe { acl_get_flag_np(fs, f) == 1 }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    //! Linux: POSIX access lists through `setfacl`/`getfacl` (the `acl` package).
    use super::{Access, Who};
    use std::path::Path;
    use std::process::Command;

    fn name(who: Who) -> String {
        match who {
            Who::User(u) => format!("u:{u}"),
            Who::Group(g) => format!("g:{g}"),
        }
    }

    fn perms(a: Access) -> &'static str {
        match a {
            Access::Read | Access::ReadOnly => "rX",
            Access::Write => "rwX",
            Access::Nothing => "---",
        }
    }

    fn setfacl(args: &[&str], path: &Path) -> std::io::Result<()> {
        let o = Command::new("setfacl").args(args).arg("--").arg(path).output()?;
        if o.status.success() {
            return Ok(());
        }
        let msg = String::from_utf8_lossy(&o.stderr);
        if msg.contains("Operation not permitted") {
            return Err(std::io::Error::from_raw_os_error(libc::EPERM));
        }
        Err(std::io::Error::other(msg.trim().to_string()))
    }

    pub fn set(path: &Path, who: Who, access: Option<Access>, _inherited: bool) -> std::io::Result<()> {
        let dir = std::fs::symlink_metadata(path)?.is_dir();
        let n = name(who);
        // Remove first (missing entries are fine), then set.
        let _ = setfacl(&["-x", &n], path);
        if dir {
            let _ = setfacl(&["-x", &format!("d:{n}")], path);
        }
        let Some(a) = access else { return Ok(()) };
        let entry = format!("{n}:{}", perms(a));
        setfacl(&["-m", &entry], path)?;
        if dir {
            setfacl(&["-m", &format!("d:{entry}")], path)?;
        }
        Ok(())
    }

    pub fn allow_passing(folder: &Path, who: Who) -> std::io::Result<()> {
        if access_of(folder, who)?.is_some() {
            return Ok(());
        }
        setfacl(&["-m", &format!("{}:--x", name(who))], folder)
    }

    pub fn pass_here(folder: &Path, who: Who) -> std::io::Result<()> {
        if access_of(folder, who)?.is_some() {
            return Ok(());
        }
        let n = name(who);
        setfacl(&["-m", &format!("{n}:--x,d:{n}:---")], folder)
    }

    pub fn named_on(path: &Path) -> std::io::Result<Vec<Who>> {
        let mut out = Vec::new();
        for l in getfacl(path)?.lines() {
            let l = l.strip_prefix("default:").unwrap_or(l);
            let mut f = l.split(':');
            let (Some(kind), Some(id)) = (f.next(), f.next()) else { continue };
            let Ok(id) = id.parse::<u32>() else { continue };
            let w = match kind {
                "user" => Who::User(id),
                "group" => Who::Group(id),
                _ => continue,
            };
            if !out.contains(&w) {
                out.push(w);
            }
        }
        Ok(out)
    }

    pub fn entries(path: &Path) -> std::io::Result<Vec<(Who, Access)>> {
        let text = getfacl(path)?;
        let mut out = Vec::new();
        for l in text.lines() {
            let mut f = l.split(':');
            let (Some(kind), Some(id), Some(p)) = (f.next(), f.next(), f.next()) else { continue };
            let Ok(id) = id.parse::<u32>() else { continue };
            let who = match kind {
                "user" => Who::User(id),
                "group" => Who::Group(id),
                _ => continue,
            };
            if let Some(a) = classify(p.split_whitespace().next().unwrap_or("")) {
                out.push((who, a));
            }
        }
        Ok(out)
    }

    fn classify(p: &str) -> Option<Access> {
        match p {
            "---" => Some(Access::Nothing),
            "--x" => None,
            p if p.contains('w') => Some(Access::Write),
            p if p.starts_with('r') => Some(Access::Read),
            _ => None,
        }
    }

    fn getfacl(path: &Path) -> std::io::Result<String> {
        let o = Command::new("getfacl").args(["-n", "-p", "--omit-header", "--"]).arg(path).output()?;
        if !o.status.success() {
            return Err(std::io::Error::other(String::from_utf8_lossy(&o.stderr).trim().to_string()));
        }
        Ok(String::from_utf8_lossy(&o.stdout).into_owned())
    }

    pub fn access_of(path: &Path, who: Who) -> std::io::Result<Option<Access>> {
        let o = Command::new("getfacl").args(["-n", "-p", "--omit-header", "--"]).arg(path).output()?;
        if !o.status.success() {
            return Err(std::io::Error::other(String::from_utf8_lossy(&o.stderr).trim().to_string()));
        }
        let want = match who {
            Who::User(u) => format!("user:{u}:"),
            Who::Group(g) => format!("group:{g}:"),
        };
        let text = String::from_utf8_lossy(&o.stdout);
        let Some(line) = text.lines().find(|l| l.starts_with(&want)) else { return Ok(None) };
        Ok(classify(line[want.len()..].split_whitespace().next().unwrap_or("")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ctx-acl-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::write(d.join("f"), "x").unwrap();
        std::fs::write(d.join("sub/g"), "y").unwrap();
        d
    }

    /// An account to name in entries: `nobody` exists on both systems and owns nothing here.
    fn nobody() -> Who {
        // SAFETY: getpwnam returns static storage or null.
        let uid = unsafe {
            let pw = libc::getpwnam(c"nobody".as_ptr());
            assert!(!pw.is_null());
            (*pw).pw_uid
        };
        Who::User(uid)
    }

    #[test]
    fn passing_toward_one_entry_closes_the_rest() {
        let d = tmp("pass");
        let who = nobody();
        std::fs::create_dir_all(d.join("other")).unwrap();
        pass_only(&d, std::ffi::OsStr::new("sub"), who, &|_| false).unwrap();
        // The folder: search only, which isn't a grant.
        assert_eq!(access_of(&d, who).unwrap(), None);
        assert_eq!(access_of(&d.join("sub"), who).unwrap(), None);
        assert_eq!(access_of(&d.join("f"), who).unwrap(), Some(Access::Nothing));
        assert_eq!(access_of(&d.join("other"), who).unwrap(), Some(Access::Nothing));
        // Made later: closed too.
        std::fs::write(d.join("later"), "z").unwrap();
        assert_eq!(access_of(&d.join("later"), who).unwrap(), Some(Access::Nothing));
        // A grant there stays: passing again doesn't close it.
        apply(&d.join("other"), who, Some(Access::Read)).unwrap();
        pass_only(&d, std::ffi::OsStr::new("sub"), who, &|_| false).unwrap();
        assert_eq!(access_of(&d.join("other"), who).unwrap(), Some(Access::Read));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn states_apply_to_everything_inside_and_come_back_out() {
        if cfg!(not(target_os = "macos")) && std::process::Command::new("setfacl").arg("--version").output().is_err() {
            eprintln!("setfacl isn't installed; skipping");
            return;
        }
        let d = tmp("states");
        let who = nobody();
        for a in [Access::Read, Access::Write, Access::ReadOnly, Access::Nothing] {
            let done = apply(&d, who, Some(a)).unwrap();
            // POSIX access lists can't deny write separately; a named entry without write already
            // wins over the group and other bits.
            let a_read = if cfg!(target_os = "macos") || a != Access::ReadOnly { a } else { Access::Read };
            assert!(done.not_owned.is_empty(), "{:?}", done.not_owned);
            assert_eq!(done.changed, 4);
            for p in [d.clone(), d.join("f"), d.join("sub"), d.join("sub/g")] {
                assert_eq!(access_of(&p, who).unwrap(), Some(a_read), "{a:?} on {}", p.display());
            }
            // New files inside take the folder's entry.
            let n = d.join(format!("new-{a:?}"));
            std::fs::write(&n, "z").unwrap();
            assert_eq!(access_of(&n, who).unwrap(), Some(a_read), "{a:?} inherited by a new file");
            std::fs::remove_file(&n).unwrap();
        }
        assert_eq!(entries(&d).unwrap(), vec![(who, Access::Nothing)]);
        apply(&d, who, None).unwrap();
        assert!(entries(&d).unwrap().is_empty());
        for p in [d.clone(), d.join("f"), d.join("sub/g")] {
            assert_eq!(access_of(&p, who).unwrap(), None);
        }
        // Passing through adds search only, and isn't a grant.
        allow_passing(&d, who).unwrap();
        assert_eq!(access_of(&d, who).unwrap(), None);
        let _ = std::fs::remove_dir_all(&d);
    }
}
