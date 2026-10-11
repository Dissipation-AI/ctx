//! Plain-text answers: readable sentences and labelled lines, whole words, numbers always
//! labelled. Notes about capped output go to standard error so standard output stays clean.

use ctx_protocol::time::{duration_words, local_minutes, parse_iso};
use ctx_protocol::*;

/// Text for standard output and notes for standard error.
#[derive(Debug, Default, PartialEq)]
pub struct Printed {
    pub out: String,
    pub notes: Vec<String>,
}

impl Printed {
    fn line(&mut self, s: impl AsRef<str>) {
        self.out.push_str(s.as_ref());
        self.out.push('\n');
    }
    fn note(&mut self, s: impl Into<String>) {
        self.notes.push(s.into());
    }
}

pub struct Style {
    /// Local time zone offset east of UTC, in seconds.
    pub tz: i64,
    /// Home folder, shown as `~`.
    pub home: String,
}

impl Style {
    fn when(&self, iso: &str) -> String {
        parse_iso(iso).map(|ms| local_minutes(ms, self.tz)).unwrap_or_else(|| iso.to_string())
    }

    fn folder(&self, f: &str) -> String {
        if f == self.home {
            "~".into()
        } else if let Some(rest) = f.strip_prefix(&format!("{}/", self.home)) {
            format!("~/{rest}")
        } else {
            f.to_string()
        }
    }
}

fn tab_words(number: u64, name: &Option<String>) -> String {
    match name {
        Some(n) => format!("tab {number} \"{n}\""),
        None => format!("tab {number}"),
    }
}

pub fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn plural(n: u64, word: &str) -> String {
    if n == 1 { format!("1 {word}") } else { format!("{} {word}s", thousands(n)) }
}

pub fn megabytes(bytes: u64) -> String {
    let mb = bytes as f64 / (1024.0 * 1024.0);
    if mb < 10.0 { format!("{:.1} MB", mb) } else { format!("{:.0} MB", mb) }
}

pub fn render(o: &Output, st: &Style) -> Printed {
    let mut p = Printed::default();
    match o {
        Output::Tabs(list) => {
            if list.windows.iter().all(|w| w.tabs.is_empty()) {
                p.line("No tabs are open. `ctx open tab` opens one.");
            }
            for w in &list.windows {
                match &w.name {
                    Some(n) => p.line(format!("window {} \"{n}\"", w.number)),
                    None => p.line(format!("window {}", w.number)),
                }
                for t in &w.tabs {
                    let mut s = format!("  {} at {}", tab_words(t.number, &t.name), st.folder(&t.folder));
                    match t.state.as_str() {
                        "busy" => match &t.running {
                            Some(r) => s.push_str(&format!(" — busy: {r}")),
                            None => s.push_str(" — busy"),
                        },
                        "typing" => s.push_str(" — idle, someone is typing at the prompt"),
                        _ => s.push_str(" — idle"),
                    }
                    if let Some(h) = &t.remote_host {
                        s.push_str(&format!(", in ssh to {h}"));
                    }
                    if t.unread_lines > 0 {
                        s.push_str(&format!(
                            ", {} unread {}",
                            thousands(t.unread_lines),
                            if t.unread_lines == 1 { "line" } else { "lines" }
                        ));
                    }
                    if let Some(l) = &t.permissions_of {
                        s.push_str(&format!(", running with the permissions of {l}"));
                    }
                    if let Some(title) = &t.title
                        && Some(title) != t.running.as_ref()
                    {
                        s.push_str(&format!(" (title: {title})"));
                    }
                    p.line(s);
                }
            }
        }
        Output::ClosedTabs(list) => {
            if list.tabs.is_empty() {
                p.line("No closed tabs are saved.");
            }
            for t in &list.tabs {
                let mut s = format!(
                    "{} at {}, closed {}",
                    tab_words(t.number, &t.name),
                    st.folder(&t.folder),
                    st.when(&t.closed_at)
                );
                if t.ended == "interrupted" {
                    s.push_str(" (interrupted)");
                }
                s.push_str(&format!(", {}", plural(t.commands, "command")));
                if t.opened_by != "you" {
                    s.push_str(&format!(", opened by {}", t.opened_by));
                }
                p.line(s);
            }
            if list.total > list.tabs.len() as u64 {
                p.note(format!(
                    "showing the newest {} of {} closed tabs; ctx list closed tabs up to {} for more",
                    list.tabs.len(),
                    thousands(list.total),
                    (list.tabs.len() * 5).max(100)
                ));
            }
        }
        Output::Commands(list) => {
            if list.commands.is_empty() {
                p.line(format!("No commands have run in {} yet.", tab_words(list.tab, &list.tab_name)));
            }
            for c in &list.commands {
                p.line(command_heading(c.number, &c.location, &c.command, c.approximate));
                p.line(format!("    {}", outcome(c.exit_status, c.took_seconds, &c.started_at, st)));
            }
            if list.total > list.commands.len() as u64 {
                p.note(format!("showing the last {} of {} commands", list.commands.len(), thousands(list.total)));
            }
        }
        Output::Activity(list) => {
            if list.entries.is_empty() {
                p.line("Nothing in the activity log matches.");
            }
            for e in &list.entries {
                let mut s = format!("{} {}: {}", st.when(&e.at), e.client, e.sentence);
                if let Some(t) = &e.typed
                    && !e.sentence.contains(t.as_str())
                {
                    s.push_str(&format!(" (typed: {t})"));
                }
                p.line(s);
            }
        }
        Output::Text(t) => {
            for l in &t.lines {
                p.line(l);
            }
            if let Some(n) = &t.note {
                p.note(n.clone());
            }
        }
        Output::CommandsWithOutput(c) | Output::Records(c) => {
            for (i, r) in c.commands.iter().enumerate() {
                if i > 0 {
                    p.line("");
                }
                p.line(command_heading(r.number, &r.location, &r.command, r.approximate));
                for l in &r.output {
                    p.line(l);
                }
                let mut tail = format!(
                    "[{} · command {} in {}",
                    outcome(r.exit_status, r.took_seconds, &r.started_at, st),
                    r.number,
                    tab_words(c.tab, &c.tab_name)
                );
                if r.full_screen {
                    tail.push_str(" · full-screen program, final screen shown");
                }
                tail.push(']');
                p.line(tail);
                if let Some(n) = &r.note {
                    p.note(format!("command {}: {n}", r.number));
                }
            }
        }
        Output::Found(f) => {
            if f.hits.is_empty() {
                p.line(format!("Nothing found for \"{}\".", f.text));
            }
            for h in &f.hits {
                let where_ = tab_words(h.tab, &h.tab_name);
                match h.found_in.as_str() {
                    "name" => p.line(format!("{where_}: its name matches")),
                    "command" => {
                        p.line(format!("{where_}, command {} ({}): {}", h.command.unwrap_or(0), st.when(&h.at), h.text))
                    }
                    _ => {
                        let head = match (h.command, h.line) {
                            (Some(c), Some(l)) => format!("{where_}, command {c}, line {l}"),
                            (None, Some(l)) => format!("{where_}, output line {l}"),
                            _ => where_,
                        };
                        let cmd = h.command_text.as_ref().map(|c| format!(" in `{c}`")).unwrap_or_default();
                        p.line(format!("{head} ({}){cmd}:", st.when(&h.at)));
                        for b in &h.before {
                            p.line(format!("    {b}"));
                        }
                        p.line(format!("  > {}", h.text));
                        for a in &h.after {
                            p.line(format!("    {a}"));
                        }
                    }
                }
            }
            if let Some(n) = &f.note {
                p.note(n.clone());
            }
        }
        Output::Status(s) => {
            p.line(format!(
                "ctx server {} (protocol {}), pid {}, up {}",
                s.version,
                s.protocol,
                s.pid,
                duration_words(s.uptime_seconds as f64)
            ));
            p.line(format!("socket: {}", s.socket));
            p.line(format!("memory: {} in total", megabytes(s.memory_bytes)));
            for t in &s.tabs {
                p.line(format!(
                    "  {}: about {}, {} of scrollback",
                    tab_words(t.number, &t.name),
                    megabytes(t.memory_bytes),
                    plural(t.scrollback_lines, "line")
                ));
            }
            p.line(format!("saved tabs on disk: {}", megabytes(s.saved_bytes)));
        }
        Output::Opened(o) => {
            let tab = match o.tab {
                Some(n) => format!("tab {n}{}", o.name.as_ref().map(|n| format!(" \"{n}\"")).unwrap_or_default()),
                None => "a tab".into(),
            };
            if o.new_window {
                let w = match &o.window_name {
                    Some(n) => format!("window {} \"{n}\"", o.window),
                    None => format!("window {}", o.window),
                };
                p.line(format!("Opened {w} with {tab} at {}.", st.folder(&o.folder)));
            } else {
                p.line(format!("Opened {tab} in window {} at {}.", o.window, st.folder(&o.folder)));
            }
            if !o.not_restored.is_empty() {
                p.line(format!("Not restored (dont_save_env): {}.", o.not_restored.join(", ")));
            }
        }
        Output::Done(d) => {
            if !d.message.is_empty() {
                p.line(&d.message);
            }
        }
        Output::Ran(r) => {
            for l in &r.output {
                p.line(l);
            }
            if let Some(n) = &r.note {
                p.note(n.clone());
            }
        }
        Output::Waited(w) => {
            let who = tab_words(w.tab, &w.tab_name);
            match w.condition.as_str() {
                "idle" => match w.exit_status {
                    Some(s) => p.line(format!("{who} is idle; the last command's exit status: {s}")),
                    None => p.line(format!("{who} is idle")),
                },
                "shows" => match &w.line {
                    Some(l) => p.line(l),
                    None => p.line(format!("{who} shows it")),
                },
                _ => p.line(format!("{who} has been quiet; waited {}", duration_words(w.waited_seconds))),
            }
        }
        Output::Attached(_) => {}
        Output::TabNumbers(v) => p.line(v.iter().map(|n| format!("tab {n}")).collect::<Vec<_>>().join(", ")),
        Output::RanBy(r) => p.line(&r.ran_by),
    }
    p
}

fn command_heading(number: u64, location: &str, command: &str, approximate: bool) -> String {
    // Found from the screen (no marks): the line already has the remote prompt in it.
    if approximate {
        return format!("~ {command}");
    }
    let approx = "";
    let mut lines = command.lines();
    let first = lines.next().unwrap_or("");
    let more = lines.count();
    let rest = if more > 0 { format!(" (and {} more)", plural(more as u64, "line")) } else { String::new() };
    let _ = number;
    format!("{approx}{location}$ {first}{rest}")
}

fn outcome(exit: Option<i32>, took: Option<f64>, started: &str, st: &Style) -> String {
    let mut parts = Vec::new();
    match (exit, took) {
        (Some(e), _) => parts.push(format!("exit status: {e}")),
        (None, Some(_)) => parts.push("ended without an exit status".into()),
        (None, None) => parts.push("still running".into()),
    }
    if let Some(t) = took {
        parts.push(format!("took {}", duration_words(t)));
    }
    parts.push(format!("started {}", st.when(started)));
    parts.join(" · ")
}

pub fn render_json(o: &Output) -> String {
    serde_json::to_string_pretty(&o.to_json()).unwrap_or_default()
}

pub fn render_toon(o: &Output) -> String {
    toon::encode(&o.to_json())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st() -> Style {
        Style { tz: 0, home: "/Users/c".into() }
    }

    #[test]
    fn tabs_read_as_sentences() {
        let o = Output::Tabs(TabList {
            windows: vec![WindowInfo {
                number: 1,
                name: Some("Work".into()),
                tabs: vec![TabInfo {
                    number: 43,
                    name: Some("Dev server".into()),
                    folder: "/Users/c/code/site".into(),
                    window: 1,
                    opened_by: "you".into(),
                    title: None,
                    state: "busy".into(),
                    running: Some("npm run dev".into()),
                    pid: Some(1),
                    started: None,
                    unread_lines: 12,
                    size: [120, 40],
                    shell_integration: true,
                    permissions_of: None,
                    remote_host: None,
                }],
            }],
        });
        let p = render(&o, &st());
        assert_eq!(
            p.out,
            "window 1 \"Work\"\n  tab 43 \"Dev server\" at ~/code/site — busy: npm run dev, 12 unread lines\n"
        );
    }

    #[test]
    fn commands_with_output() {
        let o = Output::CommandsWithOutput(CommandOutputs {
            tab: 212,
            tab_name: None,
            commands: vec![CommandRecord {
                number: 14,
                command: "cargo test -p auth".into(),
                location: "c@macbook:~/code/site".into(),
                ran_by: "you".into(),
                exit_status: Some(101),
                started_at: "2026-10-10T17:53:02Z".into(),
                finished_at: Some("2026-10-10T17:53:11Z".into()),
                took_seconds: Some(9.0),
                approximate: false,
                full_screen: false,
                output: vec!["test result: FAILED".into()],
                output_lines: 1,
                note: None,
            }],
        });
        let p = render(&o, &st());
        assert_eq!(
            p.out,
            "c@macbook:~/code/site$ cargo test -p auth\ntest result: FAILED\n[exit status: 101 · took 9 seconds · started 2026-10-10 17:53 · command 14 in tab 212]\n"
        );
    }

    #[test]
    fn numbers_get_separators() {
        assert_eq!(thousands(12503), "12,503");
        assert_eq!(thousands(999), "999");
    }
}
