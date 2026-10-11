//! `ctx help`, `docs/grammar.md` and the agent instruction block, all generated from the grammar
//! table.

use crate::grammar::{Group, Kind, Object, Slot, VERBS, Verb, sentence_words};

/// One page of example sentences, grouped.
pub fn page() -> String {
    let mut out = String::from("ctx: every command is an English sentence. Phrases can come in any order.\n");
    for g in Group::ALL {
        let verbs: Vec<&Verb> = VERBS.iter().filter(|v| v.group == g && !v.hidden).collect();
        if verbs.is_empty() {
            continue;
        }
        out.push_str(&format!("\n{}\n", g.title()));
        let examples: Vec<(&str, &str)> = verbs.iter().flat_map(|v| v.examples.iter().copied()).collect();
        let width = examples.iter().map(|(s, _)| s.len()).max().unwrap_or(0).min(60);
        for (s, what) in examples {
            if s.len() > width {
                out.push_str(&format!("  {s}\n  {:width$}  {what}\n", ""));
            } else {
                out.push_str(&format!("  {s:width$}  {what}\n"));
            }
        }
    }
    out.push_str(
        "\nAdd `as json` or `as toon` to any sentence for structured output. `ctx help <verb>` shows one verb.\n",
    );
    out
}

/// One verb in detail.
pub fn verb(name: &str) -> Option<String> {
    let v = VERBS.iter().find(|v| v.name == name.to_lowercase())?;
    let mut out = format!("ctx {}: {}\n\nSentence:\n  {}\n", v.name, v.summary, shape(v));
    let phrases: Vec<_> = v
        .order
        .iter()
        .filter_map(|s| match s {
            Slot::Phrase(p) => Some(p),
            Slot::Object => None,
        })
        .collect();
    if !phrases.is_empty() {
        out.push_str("\nPhrases (any order):\n");
        for p in phrases {
            let words = p.words.join(" ");
            let value = p.kind.placeholder();
            let after = p.after.first().map(|a| format!(" {a}")).unwrap_or_default();
            let phrase = if value.is_empty() { words } else { format!("{words} {value}{after}") };
            out.push_str(&format!("  {phrase:44} {}\n", p.meaning));
        }
    }
    if !v.examples.is_empty() {
        out.push_str("\nExamples:\n");
        for (s, what) in v.examples {
            out.push_str(&format!("  {s}\n      {what}\n"));
        }
    }
    Some(out)
}

/// `ctx run in "<tab>" "<text>" [with colors] …`
pub fn shape(v: &Verb) -> String {
    let mut parts = vec![format!("ctx {}", v.name)];
    for s in v.order {
        match s {
            Slot::Object => parts.push(object_shape(v)),
            Slot::Phrase(p) => {
                let value = p.kind.placeholder();
                let after = p.after.first().map(|a| format!(" {a}")).unwrap_or_default();
                let words = p.words.join(" ");
                parts.push(if value.is_empty() { format!("[{words}]") } else { format!("[{words} {value}{after}]") });
            }
        }
    }
    parts.join(" ")
}

fn object_shape(v: &Verb) -> String {
    match v.object {
        Object::None => String::new(),
        Object::Noun(n) => n.join("|"),
        Object::ReadSelector => {
            "[\"<tab>\" | everything | last <number> lines | last command | last <number> commands | command <number>]"
                .into()
        }
        Object::Kind(k) => k.placeholder().into(),
        Object::Optional(k) => format!("[{}]", k.placeholder()),
    }
}

/// `docs/grammar.md`.
pub fn grammar_markdown() -> String {
    let mut out = String::from(
        "# ctx grammar\n\n_Generated from `ctx-cli/src/grammar.rs`. Don't edit by hand._\n\n\
Every `ctx` command is a sentence: a verb, then phrases, each introduced by its word, in any order.\n\
The one unlabelled value is the verb's object. `the`, `a` and `an` are ignored before words ctx knows.\n\
Help, errors and the activity log print the canonical order shown here.\n\n\
A tab is its name in quotes, `tab <number>`, or `this tab`. A window is `window <number>` or `window \"<name>\"`.\n\n",
    );
    for g in Group::ALL {
        let verbs: Vec<&Verb> = VERBS.iter().filter(|v| v.group == g && !v.hidden).collect();
        if verbs.is_empty() {
            continue;
        }
        out.push_str(&format!("## {}\n\n", g.title()));
        for v in verbs {
            out.push_str(&format!("### `{}`\n\n{}\n\n```\n{}\n```\n\n", v.name, v.summary, shape(v)));
            for (s, what) in v.examples {
                out.push_str(&format!("- `{s}`: {what}\n"));
            }
            out.push('\n');
        }
    }
    out.push_str("## Values\n\n");
    for k in [
        Kind::Tab,
        Kind::Window,
        Kind::Folder,
        Kind::Count,
        Kind::Duration,
        Kind::Time,
        Kind::Text,
        Kind::Key,
        Kind::Format,
        Kind::Who,
    ] {
        out.push_str(&format!("- {}\n", k.placeholder()));
    }
    out.push_str("\nKeys: enter, escape, tab, backspace, delete, space, up, down, left, right, home, end, pageup, pagedown, insert, f1–f12, any single character, with `ctrl-`, `alt-`, `shift-` or `cmd-` in front (`ctrl-c`, `shift-tab`).\n\n");
    out.push_str(
        "## Reserved words\n\nTab and window names must contain a letter and can't be exactly one of these words:\n\n",
    );
    out.push_str(&sentence_words().iter().map(|w| format!("`{w}`")).collect::<Vec<_>>().join(", "));
    out.push_str("\n\n## Exit status\n\n0 on success, 1 for something you can fix (no such tab, a sentence that didn't parse), 2 if the server couldn't be reached, 3 if a time limit ran out. `ctx run` exits with the command's own exit status.\n");
    out
}

/// The instruction block written into agents' instruction files. Kept short: it's paid for in
/// every session.
pub fn agent_instructions() -> String {
    "## Shell commands: use ctx tabs\n\n\
Run shell commands in a persistent, named ctx tab instead of a throwaway shell, so state persists and a person can watch.\n\n\
- First `ctx list tabs`: reuse an existing tab (e.g. a running dev server) instead of starting another.\n\
- Open a tab named for your task: `ctx open tab named \"Claude - <task>\"` (prints its number).\n\
- Run: `ctx run in \"Claude - <task>\" \"cargo test\"`. It waits, prints the output and exits with the command's exit status. For long commands, add `within 10 minutes`; the command keeps running if you stop waiting.\n\
- Servers and watchers: `ctx run in \"Dev server\" \"npm run dev\" without waiting`, then `ctx wait until \"Dev server\" shows \"ready\"`.\n\
- Read with bounded commands: `ctx read last 3 commands in \"<tab>\"`, `ctx find \"<text>\"`, `ctx ask \"<question>\" about tab <n>`. Never `read everything` on a big tab.\n\
- Interactive programs: `ctx type \"…\" into \"<tab>\" then press enter`, `ctx press ctrl-c in \"<tab>\"`.\n\
- Add `as toon` or `as json` for structured answers. `ctx help` lists every sentence.\n\
- Leave tabs you didn't open alone unless asked.\n"
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_lists_every_visible_verb() {
        let p = page();
        for v in VERBS.iter().filter(|v| !v.hidden) {
            assert!(p.contains(&format!("ctx {}", v.name)), "{}", v.name);
            assert!(verb(v.name).is_some());
        }
    }

    #[test]
    fn instructions_stay_short() {
        // ~4 characters per token: under ~400 tokens.
        assert!(agent_instructions().len() < 1700, "{}", agent_instructions().len());
    }

    #[test]
    fn grammar_doc_is_current() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../docs/grammar.md");
        let current = std::fs::read_to_string(path).unwrap_or_default();
        if current != grammar_markdown() {
            std::fs::write(path, grammar_markdown()).unwrap();
            panic!("docs/grammar.md was out of date and has been regenerated; commit it");
        }
    }
}
