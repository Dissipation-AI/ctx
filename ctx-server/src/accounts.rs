//! Agents that are accounts: who they are, their `ctx-agent.toml`, and what the OS lets one
//! account do with another's home (which decides who sees and drives whose tabs).

use ctx_accounts::accounts::{self, Account};
use ctx_protocol::helper::{AGENT_FILE, AgentFile, HelperReply, HelperRequest};
use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// How long an answer about access is reused. Grants change rarely, and each answer costs the
/// helper a fork.
const FRESH: Duration = Duration::from_secs(5);

#[derive(Default)]
pub struct Access {
    answers: HashMap<(String, String, bool), (bool, Instant)>,
}

/// The agent account called `name`: in `ctx-agents`, standard, not you.
pub fn agent(name: &str) -> Option<Account> {
    let a = accounts::by_name(name)?;
    usable(a)
}

/// The agent account with this uid.
pub fn agent_by_uid(uid: u32) -> Option<Account> {
    usable(accounts::by_uid(uid)?)
}

fn usable(a: Account) -> Option<Account> {
    (a.uid != 0 && a.uid != crate::sys::uid() && accounts::is_agent(&a) && !accounts::is_admin(&a.name)).then_some(a)
}

/// Every agent account.
pub fn all() -> Vec<Account> {
    accounts::agents().into_iter().filter_map(usable).collect()
}

pub fn file_path(a: &Account) -> PathBuf {
    PathBuf::from(&a.home).join(AGENT_FILE)
}

/// The agent's `ctx-agent.toml`, if it can be believed: missing means the defaults; a file the
/// agent owns (it replaced yours) can't be.
pub fn file(a: &Account) -> Result<AgentFile, String> {
    let p = file_path(a);
    let meta = match std::fs::symlink_metadata(&p) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(AgentFile::default()),
        Err(e) => return Err(format!("ctx can't read {}: {e}", p.display())),
    };
    if !meta.is_file() || meta.uid() == a.uid {
        return Err(format!(
            "{} belongs to the agent \"{}\" itself, so ctx doesn't believe it. An administrator of the agent runs: ctx reset agent \"{}\"",
            p.display(),
            a.name,
            a.name
        ));
    }
    let text = std::fs::read_to_string(&p).map_err(|e| format!("ctx can't read {}: {e}", p.display()))?;
    AgentFile::parse(&text).map_err(|e| format!("{} has a mistake: {e}", p.display()))
}

impl Access {
    /// Whether `who` may read (or write) `whose` home, as the OS answers it.
    pub fn may(&mut self, who: &Account, whose: &Account, write: bool) -> bool {
        if who.uid == whose.uid {
            return true;
        }
        let k = (who.name.clone(), whose.home.clone(), write);
        if let Some((yes, at)) = self.answers.get(&k)
            && at.elapsed() < FRESH
        {
            return *yes;
        }
        let yes = matches!(
            ctx_protocol::helper::ask(&HelperRequest::MayAccess {
                account: who.name.clone(),
                path: whose.home.clone(),
                write,
            }),
            Ok(HelperReply::Answer { yes: true })
        );
        self.answers.insert(k, (yes, Instant::now()));
        yes
    }

    /// Forget every answer (after a grant changes).
    pub fn forget(&mut self) {
        self.answers.clear();
    }
}
