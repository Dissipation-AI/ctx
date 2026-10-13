//! `index.sqlite`: tab metadata, command text and names, with full-text search. The records are
//! the truth; the index can always be rebuilt from them.

use crate::history::History;
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub struct TabRow {
    pub number: u64,
    pub name: Option<String>,
    pub folder: String,
    pub window: u64,
    pub opened_by: String,
    pub opened_ms: i64,
    pub closed_ms: Option<i64>,
    pub ended: Option<String>,
    pub last_active_ms: i64,
    pub commands: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CommandRow {
    pub tab: u64,
    pub number: u64,
    pub text: String,
    pub folder: String,
    pub location: String,
    pub ran_by: String,
    pub exit_status: Option<i32>,
    pub started_ms: i64,
    pub finished_ms: Option<i64>,
    pub approximate: bool,
}

#[derive(Debug, Default, Clone)]
pub struct Narrow {
    pub tab: Option<u64>,
    pub folder: Option<String>,
    /// `me` (matched as `you`) or an agent name, matched inside `ran_by`.
    pub by: Option<String>,
    pub since_ms: Option<i64>,
}

pub struct Index {
    conn: Connection,
}

impl std::fmt::Debug for Index {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Index")
    }
}

const SCHEMA: &str = "
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
CREATE TABLE IF NOT EXISTS counters (kind TEXT PRIMARY KEY, next INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS tabs (
    number INTEGER PRIMARY KEY,
    name TEXT,
    folder TEXT NOT NULL,
    window INTEGER NOT NULL,
    opened_by TEXT NOT NULL,
    opened_ms INTEGER NOT NULL,
    closed_ms INTEGER,
    ended TEXT,
    last_active_ms INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS tab_names (tab INTEGER NOT NULL, name TEXT NOT NULL, UNIQUE(tab, name));
CREATE TABLE IF NOT EXISTS commands (
    tab INTEGER NOT NULL,
    number INTEGER NOT NULL,
    text TEXT NOT NULL,
    folder TEXT NOT NULL,
    location TEXT NOT NULL,
    ran_by TEXT NOT NULL,
    exit_status INTEGER,
    started_ms INTEGER NOT NULL,
    finished_ms INTEGER,
    approximate INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (tab, number)
);
CREATE INDEX IF NOT EXISTS commands_started ON commands(started_ms);
CREATE VIRTUAL TABLE IF NOT EXISTS commands_fts USING fts5(text, content='commands', tokenize='trigram');
CREATE TRIGGER IF NOT EXISTS commands_ai AFTER INSERT ON commands BEGIN
    INSERT INTO commands_fts(rowid, text) VALUES (new.rowid, new.text);
END;
CREATE TRIGGER IF NOT EXISTS commands_ad AFTER DELETE ON commands BEGIN
    INSERT INTO commands_fts(commands_fts, rowid, text) VALUES ('delete', old.rowid, old.text);
END;
CREATE TRIGGER IF NOT EXISTS commands_au AFTER UPDATE OF text ON commands BEGIN
    INSERT INTO commands_fts(commands_fts, rowid, text) VALUES ('delete', old.rowid, old.text);
    INSERT INTO commands_fts(rowid, text) VALUES (new.rowid, new.text);
END;
";

fn row_to_tab(r: &rusqlite::Row) -> rusqlite::Result<TabRow> {
    Ok(TabRow {
        number: r.get::<_, i64>(0)? as u64,
        name: r.get(1)?,
        folder: r.get(2)?,
        window: r.get::<_, i64>(3)? as u64,
        opened_by: r.get(4)?,
        opened_ms: r.get(5)?,
        closed_ms: r.get(6)?,
        ended: r.get(7)?,
        last_active_ms: r.get(8)?,
        commands: r.get::<_, i64>(9)? as u64,
    })
}

const TAB_COLUMNS: &str = "t.number, t.name, t.folder, t.window, t.opened_by, t.opened_ms, t.closed_ms, t.ended, t.last_active_ms, (SELECT COUNT(*) FROM commands c WHERE c.tab = t.number)";

fn row_to_command(r: &rusqlite::Row) -> rusqlite::Result<CommandRow> {
    Ok(CommandRow {
        tab: r.get::<_, i64>(0)? as u64,
        number: r.get::<_, i64>(1)? as u64,
        text: r.get(2)?,
        folder: r.get(3)?,
        location: r.get(4)?,
        ran_by: r.get(5)?,
        exit_status: r.get(6)?,
        started_ms: r.get(7)?,
        finished_ms: r.get(8)?,
        approximate: r.get::<_, i64>(9)? != 0,
    })
}

const COMMAND_COLUMNS: &str = "c.tab, c.number, c.text, c.folder, c.location, c.ran_by, c.exit_status, c.started_ms, c.finished_ms, c.approximate";

type Result<T> = rusqlite::Result<T>;

/// `LIKE` pattern with `%`, `_` and `\` escaped.
fn like_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

/// Turn `by me` into the `ran_by` text it matches.
pub fn by_pattern(by: &str) -> String {
    if by.eq_ignore_ascii_case("me") { "you".into() } else { by.to_lowercase() }
}

impl Index {
    pub fn open(path: &Path) -> Result<Index> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(Index { conn })
    }

    pub fn in_memory() -> Result<Index> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        Ok(Index { conn })
    }

    /// The next never-reused number for `tab` or `window`, persisted.
    pub fn next_number(&self, kind: &str) -> Result<u64> {
        let tx = self.conn.unchecked_transaction()?;
        let cur: Option<i64> =
            tx.query_row("SELECT next FROM counters WHERE kind = ?1", [kind], |r| r.get(0)).optional()?;
        let mut n = cur.unwrap_or(1);
        if kind == "tab" {
            // Never below a number already used (e.g. after an index rebuild).
            let max: Option<i64> = tx.query_row("SELECT MAX(number) FROM tabs", [], |r| r.get(0))?;
            n = n.max(max.unwrap_or(0) + 1);
        }
        tx.execute(
            "INSERT INTO counters(kind, next) VALUES (?1, ?2) ON CONFLICT(kind) DO UPDATE SET next = ?2",
            params![kind, n + 1],
        )?;
        tx.commit()?;
        Ok(n as u64)
    }

    /// Make sure future numbers are above `n` (used when records exist beyond the counter).
    pub fn reserve_above(&self, kind: &str, n: u64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO counters(kind, next) VALUES (?1, ?2) ON CONFLICT(kind) DO UPDATE SET next = MAX(next, ?2)",
            params![kind, n as i64 + 1],
        )?;
        Ok(())
    }

    pub fn upsert_tab(&self, t: &TabRow) -> Result<()> {
        self.conn.execute(
            "INSERT INTO tabs(number, name, folder, window, opened_by, opened_ms, closed_ms, ended, last_active_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(number) DO UPDATE SET name = ?2, folder = ?3, window = ?4, closed_ms = ?7, ended = ?8,
                 last_active_ms = MAX(last_active_ms, ?9)",
            params![
                t.number as i64,
                t.name,
                t.folder,
                t.window as i64,
                t.opened_by,
                t.opened_ms,
                t.closed_ms,
                t.ended,
                t.last_active_ms
            ],
        )?;
        if let Some(n) = &t.name {
            self.conn
                .execute("INSERT OR IGNORE INTO tab_names(tab, name) VALUES (?1, ?2)", params![t.number as i64, n])?;
        }
        Ok(())
    }

    pub fn set_name(&self, tab: u64, name: Option<&str>) -> Result<()> {
        self.conn.execute("UPDATE tabs SET name = ?2 WHERE number = ?1", params![tab as i64, name])?;
        if let Some(n) = name {
            self.conn.execute("INSERT OR IGNORE INTO tab_names(tab, name) VALUES (?1, ?2)", params![tab as i64, n])?;
        }
        Ok(())
    }

    pub fn set_folder(&self, tab: u64, folder: &str, at_ms: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE tabs SET folder = ?2, last_active_ms = MAX(last_active_ms, ?3) WHERE number = ?1",
            params![tab as i64, folder, at_ms],
        )?;
        Ok(())
    }

    pub fn set_window(&self, tab: u64, window: u64) -> Result<()> {
        self.conn.execute("UPDATE tabs SET window = ?2 WHERE number = ?1", params![tab as i64, window as i64])?;
        Ok(())
    }

    pub fn touch(&self, tab: u64, at_ms: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE tabs SET last_active_ms = MAX(last_active_ms, ?2) WHERE number = ?1",
            params![tab as i64, at_ms],
        )?;
        Ok(())
    }

    pub fn set_closed(&self, tab: u64, at_ms: i64, ended: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE tabs SET closed_ms = ?2, ended = ?3, last_active_ms = MAX(last_active_ms, ?2) WHERE number = ?1",
            params![tab as i64, at_ms, ended],
        )?;
        Ok(())
    }

    pub fn set_reopened(&self, tab: u64, at_ms: i64, window: u64) -> Result<()> {
        self.conn.execute(
            "UPDATE tabs SET closed_ms = NULL, ended = NULL, window = ?3, last_active_ms = MAX(last_active_ms, ?2) WHERE number = ?1",
            params![tab as i64, at_ms, window as i64],
        )?;
        Ok(())
    }

    pub fn add_command(&self, c: &CommandRow) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO commands(tab, number, text, folder, location, ran_by, exit_status, started_ms, finished_ms, approximate)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![c.tab as i64, c.number as i64, c.text, c.folder, c.location, c.ran_by, c.exit_status, c.started_ms, c.finished_ms, c.approximate as i64],
        )?;
        self.touch(c.tab, c.started_ms)
    }

    pub fn finish_command(&self, tab: u64, number: u64, exit_status: Option<i32>, finished_ms: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE commands SET exit_status = ?3, finished_ms = ?4 WHERE tab = ?1 AND number = ?2",
            params![tab as i64, number as i64, exit_status, finished_ms],
        )?;
        self.touch(tab, finished_ms)
    }

    pub fn tab(&self, number: u64) -> Result<Option<TabRow>> {
        self.conn
            .query_row(&format!("SELECT {TAB_COLUMNS} FROM tabs t WHERE t.number = ?1"), [number as i64], row_to_tab)
            .optional()
    }

    /// Tabs that the index thinks are still open (used at startup to mark them interrupted).
    pub fn open_tabs(&self) -> Result<Vec<TabRow>> {
        let mut st = self
            .conn
            .prepare(&format!("SELECT {TAB_COLUMNS} FROM tabs t WHERE t.closed_ms IS NULL ORDER BY t.number"))?;
        st.query_map([], row_to_tab)?.collect()
    }

    pub fn closed_tabs(&self, limit: u32) -> Result<(Vec<TabRow>, u64)> {
        let mut st = self.conn.prepare(&format!("SELECT {TAB_COLUMNS} FROM tabs t WHERE t.closed_ms IS NOT NULL ORDER BY t.closed_ms DESC, t.number DESC LIMIT ?1"))?;
        let rows = st.query_map([limit as i64], row_to_tab)?.collect::<Result<Vec<_>>>()?;
        let total: i64 =
            self.conn.query_row("SELECT COUNT(*) FROM tabs WHERE closed_ms IS NOT NULL", [], |r| r.get(0))?;
        Ok((rows, total as u64))
    }

    /// Closed tabs whose current or past name matches (case-insensitive), newest first.
    pub fn closed_tabs_named(&self, name: &str) -> Result<Vec<TabRow>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {TAB_COLUMNS} FROM tabs t WHERE t.closed_ms IS NOT NULL AND t.number IN (SELECT tab FROM tab_names WHERE name = ?1 COLLATE NOCASE) ORDER BY t.closed_ms DESC"
        ))?;
        st.query_map([name], row_to_tab)?.collect()
    }

    pub fn commands(&self, tab: u64) -> Result<Vec<CommandRow>> {
        let mut st = self
            .conn
            .prepare(&format!("SELECT {COMMAND_COLUMNS} FROM commands c WHERE c.tab = ?1 ORDER BY c.number"))?;
        st.query_map([tab as i64], row_to_command)?.collect()
    }

    /// The first two commands you ran in each of the last `tabs` tabs, newest tab first:
    /// (tab, folder, command, started ms), in order within each tab.
    pub fn first_commands(&self, tabs: u32) -> Result<Vec<(u64, String, String, i64)>> {
        let mut st = self.conn.prepare(
            "SELECT c.tab, c.folder, c.text, c.started_ms FROM commands c
             WHERE c.ran_by = 'you' AND c.tab IN (SELECT number FROM tabs ORDER BY number DESC LIMIT ?1)
             AND (SELECT COUNT(*) FROM commands d WHERE d.tab = c.tab AND d.ran_by = 'you' AND d.number < c.number) < 2
             ORDER BY c.tab DESC, c.number",
        )?;
        st.query_map([tabs as i64], |r| Ok((r.get::<_, i64>(0)? as u64, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect()
    }

    /// Tabs that match the narrowing, most recently active first.
    pub fn candidate_tabs(&self, n: &Narrow) -> Result<Vec<TabRow>> {
        let mut sql = format!("SELECT {TAB_COLUMNS} FROM tabs t WHERE 1=1");
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(tab) = n.tab {
            sql.push_str(&format!(" AND t.number = ?{}", args.len() + 1));
            args.push(Box::new(tab as i64));
        }
        if let Some(since) = n.since_ms {
            sql.push_str(&format!(" AND t.last_active_ms >= ?{}", args.len() + 1));
            args.push(Box::new(since));
        }
        if let Some(folder) = &n.folder {
            let i = args.len() + 1;
            sql.push_str(&format!(
                " AND (t.folder = ?{i} OR t.folder LIKE ?{} ESCAPE '\\' OR EXISTS (SELECT 1 FROM commands c WHERE c.tab = t.number AND (c.folder = ?{i} OR c.folder LIKE ?{} ESCAPE '\\')))",
                i + 1,
                i + 1
            ));
            args.push(Box::new(folder.clone()));
            args.push(Box::new(format!("{}/%", like_escape(folder.trim_end_matches('/')))));
        }
        if let Some(by) = &n.by {
            sql.push_str(&format!(" AND EXISTS (SELECT 1 FROM commands c WHERE c.tab = t.number AND lower(c.ran_by) LIKE ?{} ESCAPE '\\')", args.len() + 1));
            args.push(Box::new(format!("%{}%", like_escape(&by_pattern(by)))));
        }
        sql.push_str(" ORDER BY t.last_active_ms DESC");
        let mut st = self.conn.prepare(&sql)?;
        let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|a| a.as_ref()).collect();
        st.query_map(refs.as_slice(), row_to_tab)?.collect()
    }

    /// Commands whose text contains `text` (case-insensitive), narrowed, newest first.
    pub fn search_commands(&self, text: &str, n: &Narrow, limit: u32) -> Result<Vec<CommandRow>> {
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        let mut sql = if text.chars().count() >= 3 {
            args.push(Box::new(format!("\"{}\"", text.replace('"', "\"\""))));
            format!(
                "SELECT {COMMAND_COLUMNS} FROM commands c JOIN commands_fts f ON f.rowid = c.rowid WHERE commands_fts MATCH ?1"
            )
        } else {
            args.push(Box::new(format!("%{}%", like_escape(text))));
            format!("SELECT {COMMAND_COLUMNS} FROM commands c WHERE c.text LIKE ?1 ESCAPE '\\'")
        };
        if let Some(tab) = n.tab {
            sql.push_str(&format!(" AND c.tab = ?{}", args.len() + 1));
            args.push(Box::new(tab as i64));
        }
        if let Some(since) = n.since_ms {
            sql.push_str(&format!(" AND c.started_ms >= ?{}", args.len() + 1));
            args.push(Box::new(since));
        }
        if let Some(folder) = &n.folder {
            let i = args.len() + 1;
            sql.push_str(&format!(" AND (c.folder = ?{i} OR c.folder LIKE ?{} ESCAPE '\\')", i + 1));
            args.push(Box::new(folder.clone()));
            args.push(Box::new(format!("{}/%", like_escape(folder.trim_end_matches('/')))));
        }
        if let Some(by) = &n.by {
            sql.push_str(&format!(" AND lower(c.ran_by) LIKE ?{} ESCAPE '\\'", args.len() + 1));
            args.push(Box::new(format!("%{}%", like_escape(&by_pattern(by)))));
        }
        sql.push_str(&format!(" ORDER BY c.started_ms DESC LIMIT ?{}", args.len() + 1));
        args.push(Box::new(limit as i64));
        let mut st = self.conn.prepare(&sql)?;
        let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|a| a.as_ref()).collect();
        st.query_map(refs.as_slice(), row_to_command)?.collect()
    }

    /// Tabs whose name (current or past) contains `text`.
    pub fn search_names(&self, text: &str, limit: u32) -> Result<Vec<TabRow>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {TAB_COLUMNS} FROM tabs t WHERE t.number IN (SELECT tab FROM tab_names WHERE name LIKE ?1 ESCAPE '\\') ORDER BY t.last_active_ms DESC LIMIT ?2"
        ))?;
        st.query_map(params![format!("%{}%", like_escape(text)), limit as i64], row_to_tab)?.collect()
    }

    pub fn forget(&self, tab: u64) -> Result<()> {
        self.conn.execute("DELETE FROM commands WHERE tab = ?1", [tab as i64])?;
        self.conn.execute("DELETE FROM tab_names WHERE tab = ?1", [tab as i64])?;
        self.conn.execute("DELETE FROM tabs WHERE number = ?1", [tab as i64])?;
        Ok(())
    }

    /// Closed tabs, oldest first (for retention pruning).
    pub fn closed_oldest_first(&self) -> Result<Vec<TabRow>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {TAB_COLUMNS} FROM tabs t WHERE t.closed_ms IS NOT NULL ORDER BY t.closed_ms ASC"
        ))?;
        st.query_map([], row_to_tab)?.collect()
    }

    /// Replace everything known about a tab with what its records say.
    pub fn index_history(&self, h: &History) -> Result<()> {
        self.conn.execute("DELETE FROM commands WHERE tab = ?1", [h.number as i64])?;
        self.upsert_tab(&TabRow {
            number: h.number,
            name: h.name.clone(),
            folder: h.folder.clone(),
            window: h.window,
            opened_by: h.opened_by.clone(),
            opened_ms: h.opened_ms,
            closed_ms: Some(h.closed_ms.unwrap_or(h.last_active_ms)),
            ended: Some(h.ended.clone().unwrap_or_else(|| "interrupted".into())),
            last_active_ms: h.last_active_ms,
            commands: 0,
        })?;
        for c in &h.commands {
            self.add_command(&CommandRow {
                tab: h.number,
                number: c.number,
                text: c.text.clone(),
                folder: c.folder.clone(),
                location: c.location.clone(),
                ran_by: c.ran_by.clone(),
                exit_status: c.exit_status,
                started_ms: c.started_ms,
                finished_ms: c.finished_ms,
                approximate: c.approximate,
            })?;
        }
        self.reserve_above("tab", h.number)
    }

    pub fn begin(&self) -> Result<()> {
        self.conn.execute_batch("BEGIN")
    }

    pub fn commit(&self) -> Result<()> {
        self.conn.execute_batch("COMMIT")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(n: u64, closed: Option<i64>) -> TabRow {
        TabRow {
            number: n,
            name: Some(format!("Tab {n}")),
            folder: "/code/site".into(),
            window: 1,
            opened_by: "you".into(),
            opened_ms: 1,
            closed_ms: closed,
            ended: closed.map(|_| "closed".into()),
            last_active_ms: closed.unwrap_or(1),
            commands: 0,
        }
    }

    fn cmd(tab: u64, number: u64, text: &str, by: &str, at: i64) -> CommandRow {
        CommandRow {
            tab,
            number,
            text: text.into(),
            folder: "/code/site/api".into(),
            location: "c@m:/code/site/api".into(),
            ran_by: by.into(),
            exit_status: Some(0),
            started_ms: at,
            finished_ms: Some(at + 1),
            approximate: false,
        }
    }

    #[test]
    fn numbers_are_never_reused() {
        let ix = Index::in_memory().unwrap();
        assert_eq!(ix.next_number("tab").unwrap(), 1);
        assert_eq!(ix.next_number("tab").unwrap(), 2);
        ix.upsert_tab(&tab(40, Some(5))).unwrap();
        assert_eq!(ix.next_number("tab").unwrap(), 41);
        assert_eq!(ix.next_number("window").unwrap(), 1);
    }

    #[test]
    fn search_and_narrow() {
        let ix = Index::in_memory().unwrap();
        ix.upsert_tab(&tab(1, Some(100))).unwrap();
        ix.upsert_tab(&tab(2, None)).unwrap();
        ix.add_command(&cmd(1, 1, "cargo publish --dry-run", "you", 10)).unwrap();
        ix.add_command(&cmd(2, 1, "npm run deploy", "claude (pid 5)", 20)).unwrap();
        ix.add_command(&cmd(2, 2, "ls", "you", 30)).unwrap();
        let all = Narrow::default();
        assert_eq!(ix.search_commands("PUBLISH", &all, 10).unwrap().len(), 1);
        assert_eq!(ix.search_commands("ls", &all, 10).unwrap().len(), 1);
        let by_claude = Narrow { by: Some("claude".into()), ..Default::default() };
        assert_eq!(ix.search_commands("deploy", &by_claude, 10).unwrap().len(), 1);
        let by_me = Narrow { by: Some("me".into()), ..Default::default() };
        assert_eq!(ix.search_commands("deploy", &by_me, 10).unwrap().len(), 0);
        let at = Narrow { folder: Some("/code/site".into()), ..Default::default() };
        assert_eq!(ix.search_commands("deploy", &at, 10).unwrap().len(), 1);
        let elsewhere = Narrow { folder: Some("/code/sit".into()), ..Default::default() };
        assert_eq!(ix.search_commands("deploy", &elsewhere, 10).unwrap().len(), 0);
        let since = Narrow { since_ms: Some(15), ..Default::default() };
        assert_eq!(ix.search_commands("cargo", &since, 10).unwrap().len(), 0);
        assert_eq!(ix.candidate_tabs(&by_claude).unwrap().iter().map(|t| t.number).collect::<Vec<_>>(), vec![2]);
        let (closed, total) = ix.closed_tabs(20).unwrap();
        assert_eq!((closed.len(), total), (1, 1));
        assert_eq!(closed[0].commands, 1);
        ix.forget(1).unwrap();
        assert_eq!(ix.search_commands("publish", &all, 10).unwrap().len(), 0);
    }
}
