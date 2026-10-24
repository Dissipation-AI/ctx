//! The grammar table: every verb, the phrases it takes and the word that introduces each one.
//!
//! This one table is the source for the parser, `ctx help`, `docs/grammar.md`, the MCP tool
//! schemas, the agent instruction block and the canonical sentences in the activity log.

/// What kind of value follows a phrase's words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// No value: the words alone are the option (`with colors`, `without asking`).
    Flag,
    /// `"Dev server"`, `tab 42` or `this tab`.
    Tab,
    /// `window 2` or `window "Work"`.
    Window,
    /// A tab or a window.
    TabOrWindow,
    /// A folder path.
    Folder,
    /// A whole number.
    Count,
    /// `10 minutes`, `2 seconds`.
    Duration,
    /// `30 days ago`, `yesterday`, `2026-10-01`.
    Time,
    /// One quoted piece of text.
    Text,
    /// A key name (`enter`, `ctrl-c`).
    Key,
    /// One or more key names joined by `then`.
    Keys,
    /// `json` or `toon`.
    Format,
    /// `me` or an agent's name.
    Who,
    /// `claude`, `codex`, `gemini` or `agents`.
    Agent,
    /// A tab or window name, or a launcher name.
    Name,
    /// `"Dev server" is idle`, `"Dev server" shows "text"`, `"Dev server" is quiet for 2 seconds`.
    Condition,
    /// A tab, or `all tabs`.
    TabOrAllTabs,
    /// A program path.
    Program,
}

impl Kind {
    /// How the value is shown in help: `"<tab>"`, `<duration>` …
    pub fn placeholder(self) -> &'static str {
        match self {
            Kind::Flag => "",
            Kind::Tab => "\"<tab>\"",
            Kind::Window => "window <number or name>",
            Kind::TabOrWindow => "\"<tab>\" or window <number>",
            Kind::Folder => "<folder>",
            Kind::Count => "<number>",
            Kind::Duration => "<number> seconds|minutes|hours",
            Kind::Time => "<number> minutes|hours|days ago",
            Kind::Text => "\"<text>\"",
            Kind::Key => "<key>",
            Kind::Keys => "<key> [then <key>…]",
            Kind::Format => "json|toon",
            Kind::Who => "me|<agent>",
            Kind::Agent => "claude|codex|gemini|antigravity|cursor|pi|opencode|agents",
            Kind::Name => "\"<name>\"",
            Kind::Condition => "\"<tab>\" is idle|shows \"<text>\"|is quiet for <duration>",
            Kind::TabOrAllTabs => "\"<tab>\" or all tabs",
            Kind::Program => "<program>",
        }
    }
}

/// Every phrase that any verb uses. A phrase is identified by this id; its words come from the
/// verb's table entry, so a word means one thing within a verb.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum P {
    In,
    Into,
    At,
    To,
    Named,
    Until,
    By,
    Since,
    Within,
    UpTo,
    Colors,
    As,
    WithoutAsking,
    WithoutWaiting,
    ThenPress,
    About,
    For,
    Position,
    AllowingAnyArguments,
    SignedBy,
}

#[derive(Clone, Copy, Debug)]
pub struct Phrase {
    pub id: P,
    /// The words that introduce it, e.g. `["up", "to"]`.
    pub words: &'static [&'static str],
    pub kind: Kind,
    /// Optional words accepted after the value (`up to 1000 lines`). The first is canonical.
    pub after: &'static [&'static str],
    /// One line for help.
    pub meaning: &'static str,
}

const fn ph(id: P, words: &'static [&'static str], kind: Kind, meaning: &'static str) -> Phrase {
    Phrase { id, words, kind, after: &[], meaning }
}

const fn ph_after(
    id: P,
    words: &'static [&'static str],
    kind: Kind,
    after: &'static [&'static str],
    meaning: &'static str,
) -> Phrase {
    Phrase { id, words, kind, after, meaning }
}

const AS: Phrase = ph(P::As, &["as"], Kind::Format, "answer as JSON or TOON instead of plain text");

/// The verb's object: the one unlabelled value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Object {
    None,
    /// One of these fixed nouns (`tabs`, `closed tabs`…).
    Noun(&'static [&'static str]),
    /// What to read: nothing (the screen), `everything`, `last N lines`, `last N commands`,
    /// `command N`, or a tab.
    ReadSelector,
    Kind(Kind),
    /// Optional value of this kind.
    Optional(Kind),
}

/// One slot in a verb's canonical order: the object, or a phrase.
#[derive(Clone, Copy, Debug)]
pub enum Slot {
    Object,
    Phrase(Phrase),
}

#[derive(Clone, Copy, Debug)]
pub struct Verb {
    pub name: &'static str,
    /// Help group.
    pub group: Group,
    pub summary: &'static str,
    pub object: Object,
    /// Canonical order of the object and phrases.
    pub order: &'static [Slot],
    /// Example sentences with what they do (the first is the main one).
    pub examples: &'static [(&'static str, &'static str)],
    /// Hidden from `ctx help` (program modes started by other programs).
    pub hidden: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Seeing,
    Arranging,
    Doing,
    Agents,
    Launchers,
    Other,
}

impl Group {
    pub fn title(self) -> &'static str {
        match self {
            Group::Seeing => "Seeing what's there",
            Group::Arranging => "Opening, closing and arranging",
            Group::Doing => "Doing things in a tab",
            Group::Agents => "For agents",
            Group::Launchers => "Permissions: scoped launchers",
            Group::Other => "Other",
        }
    }

    pub const ALL: [Group; 6] =
        [Group::Seeing, Group::Arranging, Group::Doing, Group::Agents, Group::Launchers, Group::Other];
}

use Slot::{Object as O, Phrase as Ph};

pub const VERBS: &[Verb] = &[
    Verb {
        name: "list",
        group: Group::Seeing,
        summary: "List open tabs, closed tabs, a tab's commands, the activity log or launchers.",
        object: Object::Noun(&["tabs", "closed tabs", "commands", "activity", "launchers"]),
        order: &[
            O,
            Ph(ph(P::In, &["in"], Kind::Tab, "which tab (for commands and activity)")),
            Ph(ph(P::By, &["by"], Kind::Who, "who did it (activity)")),
            Ph(ph(P::Since, &["since"], Kind::Time, "only after this time (activity)")),
            Ph(ph_after(P::UpTo, &["up", "to"], Kind::Count, &[], "show up to this many")),
            Ph(AS),
        ],
        examples: &[
            (
                "ctx list tabs",
                "Every open window and its tabs: number, name, folder, what's running, idle or busy, unread output.",
            ),
            ("ctx list closed tabs", "Saved tabs that aren't running any more. Newest 20; `up to 100` for more."),
            (
                "ctx list commands in \"Dev server\"",
                "Commands run in that tab: where, exit status, start time, duration. No output.",
            ),
            (
                "ctx list activity by claude since 1 hour ago",
                "The activity log: who did what to which tab, newest first.",
            ),
            ("ctx list launchers", "Scoped launchers ctx made, and which tabs are running them now."),
        ],
        hidden: false,
    },
    Verb {
        name: "read",
        group: Group::Seeing,
        summary: "Print what's in a tab: the screen, recent lines, or commands with their output.",
        object: Object::ReadSelector,
        order: &[
            O,
            Ph(ph(P::In, &["in"], Kind::Tab, "which tab")),
            Ph(ph(P::Colors, &["with", "colors"], Kind::Flag, "keep colours and styles")),
            Ph(ph_after(P::UpTo, &["up", "to"], Kind::Count, &["lines"], "cap each output at this many lines")),
            Ph(AS),
        ],
        examples: &[
            ("ctx read \"Dev server\"", "What's on screen in that tab right now, as plain text."),
            ("ctx read last 200 lines in \"Dev server\"", "The last 200 lines, including scrollback."),
            ("ctx read everything in \"Dev server\"", "All of it."),
            ("ctx read last command in \"Dev server\"", "Only the most recent command and its output."),
            (
                "ctx read last 3 commands in \"Dev server\"",
                "The last 3 commands with their outputs, exit statuses and times.",
            ),
            (
                "ctx read command 14 in tab 212",
                "One specific command and its output. Cite a line as `tab 212, command 14, line 3`.",
            ),
        ],
        hidden: false,
    },
    Verb {
        name: "find",
        group: Group::Seeing,
        summary: "Search every tab, open and closed: names, commands and output.",
        object: Object::Kind(Kind::Text),
        order: &[
            O,
            Ph(ph(P::In, &["in"], Kind::Tab, "only in this tab")),
            Ph(ph(P::At, &["at"], Kind::Folder, "only commands run in this folder or below")),
            Ph(ph(P::By, &["by"], Kind::Who, "only commands run by me or by an agent")),
            Ph(ph(P::Since, &["since"], Kind::Time, "only after this time")),
            Ph(ph_after(P::UpTo, &["up", "to"], Kind::Count, &["results"], "show up to this many results")),
            Ph(AS),
        ],
        examples: &[
            ("ctx find \"cargo publish\"", "Search every tab, open and closed. Newest first, with surrounding lines."),
            ("ctx find \"deploy\" in \"Dev server\"", "…only in that tab."),
            (
                "ctx find \"deploy\" at ~/code/site by claude since 30 days ago",
                "…narrowed by folder, by who ran the command, and by date.",
            ),
        ],
        hidden: false,
    },
    Verb {
        name: "ask",
        group: Group::Seeing,
        summary: "Have a language model read a tab and answer with the exact lines that support it.",
        object: Object::Kind(Kind::Text),
        order: &[
            O,
            Ph(ph(P::About, &["about"], Kind::TabOrAllTabs, "which tab, or all tabs")),
            Ph(ph(P::At, &["at"], Kind::Folder, "with all tabs: only tabs in this folder")),
            Ph(ph(P::By, &["by"], Kind::Who, "with all tabs: only tabs with commands by me or an agent")),
            Ph(ph(P::Since, &["since"], Kind::Time, "with all tabs: only tabs active after this time")),
            Ph(ph(P::Within, &["within"], Kind::Duration, "time limit (default 1 minute)")),
            Ph(AS),
        ],
        examples: &[
            (
                "ctx ask \"where did this session discover why the auth test fails?\" about tab 212",
                "A short answer plus the exact, checked lines that support it.",
            ),
            (
                "ctx ask \"which command fixed the build?\" about all tabs at ~/code/site since 7 days ago",
                "Ask about several tabs at once.",
            ),
        ],
        hidden: false,
    },
    Verb {
        name: "status",
        group: Group::Seeing,
        summary: "Server version, uptime, memory per tab, socket path.",
        object: Object::None,
        order: &[Ph(AS)],
        examples: &[("ctx status", "Server version, uptime, memory per tab, socket path.")],
        hidden: false,
    },
    Verb {
        name: "open",
        group: Group::Arranging,
        summary: "Open a window or a tab.",
        object: Object::Noun(&["window", "tab"]),
        order: &[
            O,
            Ph(ph(P::In, &["in"], Kind::Window, "which window (tabs only)")),
            Ph(ph(P::At, &["at"], Kind::Folder, "starting folder")),
            Ph(ph(P::Named, &["named"], Kind::Name, "a name for the new tab or window")),
            Ph(AS),
        ],
        examples: &[
            ("ctx open window", "A new window with one tab in your home folder."),
            ("ctx open window at ~/code/site", "…starting in that folder."),
            ("ctx open tab", "A new tab in the current window (the Agents window for agents)."),
            (
                "ctx open tab in window 2 at ~/code/site named \"Dev server\"",
                "Any combination of window, folder and name. Prints the new tab's number.",
            ),
        ],
        hidden: false,
    },
    Verb {
        name: "close",
        group: Group::Arranging,
        summary: "Close a tab or a window. Asks first if something is running.",
        object: Object::Kind(Kind::TabOrWindow),
        order: &[O, Ph(ph(P::WithoutAsking, &["without", "asking"], Kind::Flag, "don't ask first")), Ph(AS)],
        examples: &[
            ("ctx close \"Dev server\"", "Close that tab. Asks first if something is running."),
            ("ctx close window 2", "Close a window and all its tabs. Same rule."),
            ("ctx close \"Dev server\" without asking", "Don't ask."),
        ],
        hidden: false,
    },
    Verb {
        name: "rename",
        group: Group::Arranging,
        summary: "Name or rename a tab or a window.",
        object: Object::Kind(Kind::TabOrWindow),
        order: &[O, Ph(ph(P::To, &["to"], Kind::Name, "the new name")), Ph(AS)],
        examples: &[
            ("ctx rename \"auth\" to \"Claude - auth refactor\"", "Name or rename a tab."),
            ("ctx rename window 2 to \"Work\"", "Name a window."),
        ],
        hidden: false,
    },
    Verb {
        name: "move",
        group: Group::Arranging,
        summary: "Move a tab to another window.",
        object: Object::Kind(Kind::Tab),
        order: &[
            O,
            Ph(ph(P::To, &["to"], Kind::Window, "the window to move it to")),
            Ph(ph(P::Position, &["position"], Kind::Count, "its place in that window, from 1")),
            Ph(AS),
        ],
        examples: &[
            ("ctx move \"Dev server\" to window 2", "Move a tab to the end of another window."),
            ("ctx move \"Dev server\" to window 2 position 1", "…and choose its place."),
            ("ctx move \"Dev server\" to new window", "Give a tab a window of its own."),
        ],
        hidden: false,
    },
    Verb {
        name: "clear",
        group: Group::Arranging,
        summary: "Clear a tab's screen and scrollback (the saved record keeps everything).",
        object: Object::Kind(Kind::Tab),
        order: &[O, Ph(AS)],
        examples: &[(
            "ctx clear \"Dev server\"",
            "Clear the screen and scrollback. The saved record keeps everything.",
        )],
        hidden: false,
    },
    Verb {
        name: "reopen",
        group: Group::Arranging,
        summary: "Bring back a closed tab with its number, name, folder and environment.",
        object: Object::Kind(Kind::Tab),
        order: &[O, Ph(AS)],
        examples: &[(
            "ctx reopen tab 40",
            "Bring back closed tab 40 as tab 40, with its name, folder and environment.",
        )],
        hidden: false,
    },
    Verb {
        name: "forget",
        group: Group::Arranging,
        summary: "Permanently delete a closed tab's saved record.",
        object: Object::Kind(Kind::Tab),
        order: &[O, Ph(ph(P::WithoutAsking, &["without", "asking"], Kind::Flag, "don't ask first")), Ph(AS)],
        examples: &[("ctx forget tab 40", "Permanently delete a closed tab's saved record. Asks first.")],
        hidden: false,
    },
    Verb {
        name: "run",
        group: Group::Doing,
        summary: "Run a shell command at a tab's prompt, wait for it, print its output and exit with its exit status.",
        object: Object::Optional(Kind::Text),
        order: &[
            Ph(ph(P::In, &["in"], Kind::Tab, "which tab")),
            O,
            Ph(ph(P::Colors, &["with", "colors"], Kind::Flag, "keep colours and styles")),
            Ph(ph_after(
                P::UpTo,
                &["up", "to"],
                Kind::Count,
                &["lines"],
                "print at most this many lines (default 400)",
            )),
            Ph(ph(P::Within, &["within"], Kind::Duration, "stop waiting after this long (the command keeps running)")),
            Ph(ph(
                P::WithoutWaiting,
                &["without", "waiting"],
                Kind::Flag,
                "start it and return at once (servers, watchers)",
            )),
            Ph(AS),
        ],
        examples: &[
            (
                "ctx run in \"Dev server\" \"npm test\"",
                "The way to run a shell command: runs it at the tab's prompt, waits, prints its output, exits with its exit status.",
            ),
            (
                "ctx run in \"Dev server\" \"npm run dev\" without waiting",
                "Start a long-running command and return immediately.",
            ),
            (
                "ctx run in \"Build\" \"cargo build\" within 10 minutes",
                "Stop waiting after 10 minutes; the command keeps running in the tab.",
            ),
        ],
        hidden: false,
    },
    Verb {
        name: "type",
        group: Group::Doing,
        summary: "Type text into the program running in a tab, exactly as if typed.",
        object: Object::Kind(Kind::Text),
        order: &[
            O,
            Ph(ph(P::Into, &["into"], Kind::Tab, "which tab")),
            Ph(ph(P::ThenPress, &["then", "press"], Kind::Key, "press a key afterwards")),
            Ph(AS),
        ],
        examples: &[
            (
                "ctx type \"print(df.head())\" into \"Python\"",
                "Type into whatever program is running in the tab. Doesn't press Enter.",
            ),
            ("ctx type \"yes\" into \"Dev server\" then press enter", "…and press Enter."),
        ],
        hidden: false,
    },
    Verb {
        name: "press",
        group: Group::Doing,
        summary: "Press a key or a combination in a tab.",
        object: Object::Kind(Kind::Keys),
        order: &[O, Ph(ph(P::In, &["in"], Kind::Tab, "which tab")), Ph(AS)],
        examples: &[
            (
                "ctx press ctrl-c in \"Dev server\"",
                "Press a key or combination: enter, escape, tab, up, ctrl-c, ctrl-d, f5…",
            ),
            ("ctx press escape then enter in \"Editor\"", "Several keys in order."),
        ],
        hidden: false,
    },
    Verb {
        name: "wait",
        group: Group::Doing,
        summary: "Wait for something to happen in a tab instead of polling.",
        object: Object::None,
        order: &[
            Ph(ph(P::Until, &["until"], Kind::Condition, "what to wait for")),
            Ph(ph(P::Within, &["within"], Kind::Duration, "give up after this long (default 5 minutes)")),
            Ph(AS),
        ],
        examples: &[
            (
                "ctx wait until \"Dev server\" is idle",
                "Wait for the running command to finish. Prints its exit status.",
            ),
            ("ctx wait until \"Dev server\" shows \"Listening on\"", "Wait for that text to appear in the output."),
            (
                "ctx wait until \"Dev server\" is quiet for 2 seconds",
                "Wait until there's been no new output for 2 seconds.",
            ),
        ],
        hidden: false,
    },
    Verb {
        name: "show",
        group: Group::Doing,
        summary: "Bring a tab to the front in CTX.app.",
        object: Object::Kind(Kind::Tab),
        order: &[O, Ph(AS)],
        examples: &[(
            "ctx show \"Dev server\"",
            "Bring that tab to the front in CTX.app (opening its window if needed).",
        )],
        hidden: false,
    },
    Verb {
        name: "attach",
        group: Group::Doing,
        summary: "Connect this terminal to a tab (no GUI needed). Press ctrl-] to detach.",
        object: Object::None,
        order: &[Ph(ph(P::To, &["to"], Kind::Tab, "which tab"))],
        examples: &[(
            "ctx attach to tab 4",
            "Use a tab from this terminal. Press ctrl-] to detach; the tab keeps running.",
        )],
        hidden: false,
    },
    Verb {
        name: "install",
        group: Group::Agents,
        summary: "Route an agent's shell tool through ctx tabs (hooks), or write ctx's instruction block.",
        object: Object::Noun(&["hooks", "instructions"]),
        order: &[
            O,
            Ph(ph(P::For, &["for"], Kind::Agent, "which agent")),
            Ph(ph(P::At, &["at"], Kind::Folder, "a project folder (hooks for one project, or `for agents`)")),
            Ph(AS),
        ],
        examples: &[
            (
                "ctx install hooks for claude",
                "Installs ctx's Claude Code plugin for you (`claude plugin install`): Bash runs in a tab per session.",
            ),
            (
                "ctx install hooks for antigravity",
                "Installs ctx's Antigravity plugin (`agy plugin install`): run_command runs in a tab per conversation.",
            ),
            (
                "ctx install hooks for codex",
                "Also cursor, pi and opencode, each through its own plugin registry (Cursor: its hooks.json).",
            ),
            ("ctx install hooks for claude at ~/code/site", "Only for that project (Claude's local scope)."),
            ("ctx install instructions for claude", "Writes to ~/.claude/CLAUDE.md, between ctx's markers."),
            ("ctx install instructions for codex", "Writes to ~/.codex/AGENTS.md."),
            ("ctx install instructions for gemini", "Writes to ~/.gemini/GEMINI.md."),
            (
                "ctx install instructions for agents at ~/code/site",
                "Writes to that project's AGENTS.md (or CLAUDE.md if that's what it uses).",
            ),
        ],
        hidden: false,
    },
    Verb {
        name: "remove",
        group: Group::Agents,
        summary: "Remove ctx's hooks or instruction block, or delete a launcher.",
        object: Object::Noun(&["hooks", "instructions", "launcher"]),
        order: &[
            O,
            Ph(ph(P::For, &["for"], Kind::Agent, "which agent")),
            Ph(ph(P::At, &["at"], Kind::Folder, "a project folder (hooks for one project, or `for agents`)")),
            Ph(AS),
        ],
        examples: &[
            ("ctx remove hooks for antigravity", "Uninstall ctx's plugin; the agent's shell is its own again."),
            ("ctx remove instructions for claude", "Remove the block; nothing outside ctx's markers is touched."),
            (
                "ctx remove launcher \"sysadmin\"",
                "Delete a launcher, and remind you to remove its entry in System Settings.",
            ),
        ],
        hidden: false,
    },
    Verb {
        name: "mcp",
        group: Group::Agents,
        summary: "Serve ctx to an agent over MCP (stdin and stdout). Started by the agent, not typed.",
        object: Object::None,
        order: &[],
        examples: &[("ctx mcp", "Run the MCP server: one `shell` tool whose commands run in a ctx tab.")],
        hidden: false,
    },
    Verb {
        name: "make",
        group: Group::Launchers,
        summary: "Build a small signed launcher that holds a macOS permission for one program in one folder.",
        object: Object::Noun(&["launcher"]),
        order: &[
            O,
            Ph(ph(P::Named, &["named"], Kind::Name, "the launcher's name (and command)")),
            Ph(ph(P::For, &["for"], Kind::Program, "the program it starts")),
            Ph(ph(P::At, &["at"], Kind::Folder, "the folder it starts the program in")),
            Ph(ph(
                P::AllowingAnyArguments,
                &["allowing", "any", "arguments"],
                Kind::Flag,
                "pass any arguments through (anything that can run it can then use its permission)",
            )),
            Ph(ph(P::SignedBy, &["signed", "by"], Kind::Name, "a Developer ID signing identity (default: ad-hoc)")),
            Ph(AS),
        ],
        examples: &[
            (
                "ctx make launcher named \"sysadmin\" for ~/.local/bin/claude at ~/Agents/sys-admin",
                "Generate, build and sign a launcher for that program and folder.",
            ),
            (
                "ctx make launcher named \"sysadmin\" for ~/.local/bin/claude at ~/Agents/sys-admin allowing any arguments",
                "Pass arguments through.",
            ),
        ],
        hidden: false,
    },
    Verb {
        name: "rebuild",
        group: Group::Launchers,
        summary: "Rebuild a launcher from the current template.",
        object: Object::Noun(&["launcher"]),
        order: &[O, Ph(AS)],
        examples: &[(
            "ctx rebuild launcher \"sysadmin\"",
            "Rebuild after a ctx update. Says whether a re-grant is needed.",
        )],
        hidden: false,
    },
    Verb {
        name: "restart",
        group: Group::Other,
        summary: "Restart the ctx server (after an update). Open tabs are interrupted and can be reopened.",
        object: Object::Noun(&["server"]),
        order: &[O, Ph(ph(P::WithoutAsking, &["without", "asking"], Kind::Flag, "don't ask first")), Ph(AS)],
        examples: &[(
            "ctx restart server",
            "Restart the server, e.g. after installing a new ctx. Asks first if tabs are open.",
        )],
        hidden: false,
    },
    Verb {
        name: "help",
        group: Group::Other,
        summary: "Example sentences for every verb, or one verb in detail.",
        object: Object::Optional(Kind::Text),
        order: &[O],
        examples: &[("ctx help", "One page of example sentences."), ("ctx help run", "One verb in detail.")],
        hidden: false,
    },
    Verb {
        name: "server",
        group: Group::Other,
        summary: "Run the ctx server. Started by launchd, never typed by hand.",
        object: Object::None,
        order: &[],
        examples: &[],
        hidden: true,
    },
];

/// Words dropped wherever they're used as English filler (`read the last command`).
pub const FILLERS: &[&str] = &["the", "a", "an"];

/// Words that may follow a filler for it to be treated as filler.
pub const AFTER_FILLER: &[&str] = &[
    "last",
    "tab",
    "tabs",
    "window",
    "command",
    "commands",
    "everything",
    "server",
    "closed",
    "activity",
    "launcher",
    "launchers",
    "instructions",
    "hooks",
    "screen",
    "output",
];

/// Words that can follow a reference to a tab inside a selector or condition.
pub const SELECTOR_WORDS: &[&str] = &["last", "everything", "command", "commands", "line", "lines", "screen"];
pub const CONDITION_WORDS: &[&str] = &["is", "idle", "shows", "quiet"];
pub const TIME_UNITS: &[&str] =
    &["second", "seconds", "minute", "minutes", "hour", "hours", "day", "days", "week", "weeks", "month", "months"];
pub const OTHER_WORDS: &[&str] = &[
    "this",
    "all",
    "me",
    "ago",
    "yesterday",
    "today",
    "json",
    "toon",
    "results",
    "agents",
    "antigravity",
    "cursor",
    "pi",
    "opencode",
];

pub fn verb(name: &str) -> Option<&'static Verb> {
    VERBS.iter().find(|v| v.name == name)
}

/// Every word ctx's sentences use. Tab and window names can't be exactly one of these.
pub fn sentence_words() -> Vec<&'static str> {
    let mut words: Vec<&'static str> = Vec::new();
    for v in VERBS {
        words.push(v.name);
        if let Object::Noun(nouns) = v.object {
            for n in nouns {
                words.extend(n.split(' '));
            }
        }
        for slot in v.order {
            if let Slot::Phrase(p) = slot {
                words.extend(p.words.iter().copied());
                words.extend(p.after.iter().copied());
            }
        }
    }
    words.extend(FILLERS);
    words.extend(SELECTOR_WORDS);
    words.extend(CONDITION_WORDS);
    words.extend(TIME_UNITS);
    words.extend(OTHER_WORDS);
    words.extend(["tab", "window", "then", "press"]);
    words.sort_unstable();
    words.dedup();
    words
}

pub fn is_sentence_word(word: &str) -> bool {
    let lower = word.to_lowercase();
    sentence_words().contains(&lower.as_str())
}

/// Key names `press` and `then press` accept.
pub const KEYS: &[&str] = &[
    "enter",
    "return",
    "escape",
    "tab",
    "backspace",
    "delete",
    "space",
    "up",
    "down",
    "left",
    "right",
    "home",
    "end",
    "pageup",
    "pagedown",
    "insert",
    "f1",
    "f2",
    "f3",
    "f4",
    "f5",
    "f6",
    "f7",
    "f8",
    "f9",
    "f10",
    "f11",
    "f12",
];

/// Modifier prefixes in key names: `ctrl-c`, `alt-x`, `shift-tab`, `cmd-k`.
pub const MODIFIERS: &[&str] = &["ctrl", "alt", "shift", "cmd"];

/// Whether `name` is a key `ctx press` understands: a named key or one character, optionally
/// with modifier prefixes (`ctrl-c`, `ctrl-shift-up`).
pub fn is_key(name: &str) -> bool {
    let lower = name.to_lowercase();
    let mut parts: Vec<&str> = lower.split('-').collect();
    // `-` itself, or `ctrl--`.
    if lower.ends_with("--") || lower == "-" {
        parts.pop();
        if let Some(last) = parts.last_mut() {
            *last = "-";
        }
    }
    let Some(key) = parts.pop() else { return false };
    if !parts.iter().all(|m| MODIFIERS.contains(m)) {
        return false;
    }
    KEYS.contains(&key) || key.chars().count() == 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys() {
        for k in ["enter", "ctrl-c", "CTRL-C", "f5", "ctrl-shift-up", "a", "-", "alt-x"] {
            assert!(is_key(k), "{k}");
        }
        for k in ["ctrl-", "hyper-c", "enterr", ""] {
            assert!(!is_key(k), "{k}");
        }
    }

    #[test]
    fn reserved_words_include_the_basics() {
        for w in ["tab", "window", "in", "this", "last", "all", "the", "everything", "as", "until"] {
            assert!(is_sentence_word(w), "{w}");
        }
        assert!(!is_sentence_word("Dev"));
    }

    #[test]
    fn every_verb_has_its_object_in_order_when_it_takes_one() {
        for v in VERBS {
            let has_object_slot = v.order.iter().any(|s| matches!(s, Slot::Object));
            assert_eq!(has_object_slot, v.object != Object::None, "{}", v.name);
        }
    }
}
