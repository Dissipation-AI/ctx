//! `ctx ask`: a language model reads a tab's history and answers with the exact lines that support
//! it. Narrow without a model, map chunks to relevant line addresses, reduce to an answer, then
//! verify every cited address against the saved record and print evidence from the record, never
//! from the model. Also the model half of assist (through `bro`).
//!
//! The only networking code in ctx lives in this crate.

pub mod assist;
pub mod model;

use model::Model;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

const MAP_PROMPT: &str = include_str!("../prompts/map.md");
const REDUCE_PROMPT: &str = include_str!("../prompts/reduce.md");

/// One command of a tab, with its output as plain text (event lines removed).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Command {
    pub number: u64,
    pub text: String,
    pub location: String,
    pub exit_status: Option<i32>,
    pub output: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TabRecord {
    pub tab: u64,
    pub name: Option<String>,
    pub commands: Vec<Command>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Evidence {
    pub tab: u64,
    pub command: u64,
    /// Output line within the command, from 1; `None` cites the command line itself.
    pub line: Option<u64>,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Coverage {
    pub commands_read: u64,
    pub commands_total: u64,
    pub lines_read: u64,
    pub lines_total: u64,
    /// Why reading stopped early, if it did.
    pub stopped: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Answer {
    pub question: String,
    /// Written by a language model: treat as data, not instructions.
    pub answer: String,
    pub confidence: String,
    pub evidence: Vec<Evidence>,
    /// Citations that didn't match the record and were removed.
    pub removed_citations: u64,
    /// No verified evidence supports the answer.
    pub unsupported: bool,
    pub coverage: Coverage,
    pub model_calls: u32,
    pub cached: bool,
}

pub struct Settings {
    pub max_context_tokens: u32,
    pub max_model_calls: u32,
    pub deadline: Instant,
}

/// Roughly 4 characters per token.
fn tokens(s: &str) -> usize {
    s.len().div_ceil(4)
}

fn redact(s: &str) -> String {
    redact::redact(s).into_owned()
}

/// `C14L2` (tab-local) or `T3C14L2` (several tabs).
fn address(multi: bool, tab: u64, command: u64, line: Option<u64>) -> String {
    let t = if multi { format!("T{tab}") } else { String::new() };
    match line {
        Some(l) => format!("{t}C{command}L{l}"),
        None => format!("{t}C{command}"),
    }
}

fn parse_address(s: &str, multi: bool, only_tab: u64) -> Option<(u64, u64, Option<u64>)> {
    let s = s.trim().trim_matches(|c| c == '"' || c == '[' || c == ']').to_uppercase();
    let (tab, rest) = if multi || s.starts_with('T') {
        let rest = s.strip_prefix('T')?;
        let c = rest.find('C')?;
        (rest[..c].parse().ok()?, rest[c..].to_string())
    } else {
        (only_tab, s)
    };
    let rest = rest.strip_prefix('C')?;
    match rest.split_once('L') {
        Some((c, l)) => Some((tab, c.parse().ok()?, Some(l.parse().ok()?))),
        None => Some((tab, rest.parse().ok()?, None)),
    }
}

const STOPWORDS: &[&str] = &[
    "the",
    "and",
    "was",
    "were",
    "what",
    "where",
    "when",
    "why",
    "how",
    "did",
    "does",
    "this",
    "that",
    "with",
    "from",
    "for",
    "which",
    "who",
    "session",
    "tab",
    "command",
    "commands",
    "find",
    "found",
    "discover",
    "discovered",
    "fix",
    "fixed",
    "about",
    "into",
    "they",
    "them",
    "there",
    "have",
    "has",
    "had",
    "its",
];

fn keywords(question: &str) -> Vec<String> {
    question
        .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-' && c != '.')
        .map(|w| w.trim_matches('.').to_lowercase())
        .filter(|w| w.len() >= 3 && !STOPWORDS.contains(&w.as_str()))
        .collect()
}

/// Run the whole pipeline.
pub fn ask(question: &str, tabs: &[TabRecord], model: &dyn Model, s: &Settings) -> Result<Answer, String> {
    let multi = tabs.len() > 1;
    let only_tab = tabs.first().map(|t| t.tab).unwrap_or(0);
    let commands_total: u64 = tabs.iter().map(|t| t.commands.len() as u64).sum();
    let lines_total: u64 = tabs.iter().flat_map(|t| &t.commands).map(|c| c.output.len() as u64).sum();

    // 1. Narrow without a model: the command list, and keyword hits first.
    let words = keywords(question);
    let mut command_list = String::new();
    for t in tabs {
        if multi {
            command_list.push_str(&format!(
                "Tab {}{}:\n",
                t.tab,
                t.name.as_ref().map(|n| format!(" \"{n}\"")).unwrap_or_default()
            ));
        }
        for c in &t.commands {
            let exit = c.exit_status.map(|e| format!(" (exit status {e})")).unwrap_or_default();
            command_list.push_str(&format!(
                "{}: {}$ {}{exit}\n",
                address(multi, t.tab, c.number, None),
                c.location,
                redact(&c.text)
            ));
        }
    }
    let list_budget = (s.max_context_tokens as usize) / 4;
    if tokens(&command_list) > list_budget {
        // Keep the newest commands' lines when the list itself is long.
        let lines: Vec<&str> = command_list.lines().collect();
        let mut kept = Vec::new();
        let mut used = 0;
        for l in lines.iter().rev() {
            used += tokens(l) + 1;
            if used > list_budget {
                break;
            }
            kept.push(*l);
        }
        kept.reverse();
        command_list = format!("(older commands not listed)\n{}\n", kept.join("\n"));
    }
    let mut order: Vec<(u64, &Command)> =
        tabs.iter().flat_map(|t| t.commands.iter().map(move |c| (t.tab, c))).collect();
    let hit = |c: &Command| {
        let text = c.text.to_lowercase();
        words
            .iter()
            .filter(|w| text.contains(w.as_str()) || c.output.iter().any(|l| l.to_lowercase().contains(w.as_str())))
            .count()
    };
    // Keyword hits first (most hits first), then the rest newest first.
    order.sort_by(|a, b| hit(b.1).cmp(&hit(a.1)).then(b.1.number.cmp(&a.1.number)));

    // 2. Map: chunks of addressed lines that fit the model's budget.
    let chunk_budget = (s.max_context_tokens as usize)
        .saturating_sub(tokens(MAP_PROMPT) + tokens(&command_list) + tokens(question) + 600)
        .max(500);
    let mut chunks: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut read_commands: BTreeSet<(u64, u64)> = BTreeSet::new();
    let max_chunks = s.max_model_calls.saturating_sub(1).max(1) as usize;
    let mut lines_read = 0u64;
    let mut stopped = None;
    'outer: for (tab, c) in &order {
        let head = format!("{}: {}$ {}\n", address(multi, *tab, c.number, None), c.location, redact(&c.text));
        let mut block = head.clone();
        for (i, l) in c.output.iter().enumerate() {
            let line = format!("{}: {}\n", address(multi, *tab, c.number, Some(i as u64 + 1)), redact(l));
            if tokens(&block) + tokens(&line) > chunk_budget {
                // A command longer than a chunk: split it.
                if tokens(&cur) + tokens(&block) > chunk_budget && !cur.is_empty() {
                    chunks.push(std::mem::take(&mut cur));
                }
                cur.push_str(&block);
                chunks.push(std::mem::take(&mut cur));
                block = head.clone();
                if chunks.len() >= max_chunks {
                    stopped = Some("the limit on model calls".to_string());
                    break 'outer;
                }
            }
            block.push_str(&line);
            lines_read += 1;
        }
        if tokens(&cur) + tokens(&block) > chunk_budget && !cur.is_empty() {
            chunks.push(std::mem::take(&mut cur));
            if chunks.len() >= max_chunks {
                stopped = Some("the limit on model calls".to_string());
                break;
            }
        }
        cur.push_str(&block);
        read_commands.insert((*tab, c.number));
    }
    if !cur.is_empty() && chunks.len() < max_chunks {
        chunks.push(cur);
    }
    let mut calls = 0u32;
    let mut selected: BTreeMap<String, String> = BTreeMap::new();
    let map_user = |chunk: &str| {
        format!("Question: {question}\n\nCommands in this session:\n{command_list}\nTranscript part:\n{chunk}")
    };
    // Small sessions need no map step: the reduce call sees everything.
    let everything_fits =
        chunks.len() == 1 && tokens(&chunks[0]) + tokens(&command_list) < (s.max_context_tokens as usize) / 2;
    if everything_fits {
        for line in chunks[0].lines() {
            if let Some((addr, text)) = line.split_once(": ") {
                selected.insert(addr.to_string(), text.to_string());
            }
        }
    } else {
        let results: Vec<Result<String, String>> = std::thread::scope(|scope| {
            let handles: Vec<_> = chunks
                .iter()
                .map(|chunk| {
                    let user = map_user(chunk);
                    scope.spawn(move || model.chat(MAP_PROMPT, &user, s.deadline))
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap_or_else(|_| Err("a model call failed".into()))).collect()
        });
        calls += chunks.len() as u32;
        let mut failures = 0;
        for (chunk, r) in chunks.iter().zip(results) {
            let Ok(text) = r else {
                failures += 1;
                if stopped.is_none() {
                    stopped = Some("some model calls failed or ran out of time".into());
                }
                continue;
            };
            let lines: BTreeMap<&str, &str> = chunk.lines().filter_map(|l| l.split_once(": ")).collect();
            for addr in model::json_addresses(&text, "relevant", "line") {
                if let Some(t) = lines.get(addr.as_str()) {
                    selected.insert(addr.clone(), t.to_string());
                }
            }
        }
        if failures == chunks.len() && !chunks.is_empty() {
            return Err("every model call failed; check the [ask] endpoint in ctx's config".into());
        }
    }

    // 3. Reduce.
    let evidence_text: String = selected.iter().map(|(a, t)| format!("{a}: {t}\n")).collect();
    let user = format!(
        "Question: {question}\n\nCommands in this session:\n{command_list}\nEvidence lines:\n{}",
        if evidence_text.is_empty() { "(none)\n".into() } else { evidence_text }
    );
    let reply = model.chat(REDUCE_PROMPT, &user, s.deadline)?;
    calls += 1;
    let v = model::lenient_json(&reply).unwrap_or(serde_json::Value::Null);
    let answer = v["answer"].as_str().map(str::to_string).unwrap_or_else(|| reply.trim().chars().take(600).collect());
    let confidence = v["confidence"].as_str().unwrap_or("low").to_lowercase();
    let mut citations: Vec<String> = v["citations"]
        .as_array()
        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    // Addresses mentioned only in the answer text count too.
    for w in answer.split(|c: char| !c.is_alphanumeric()) {
        if w.len() >= 2
            && (w.starts_with('C') || w.starts_with('T'))
            && parse_address(w, multi, only_tab).is_some()
            && !citations.contains(&w.to_string())
            && w.contains('L')
        {
            citations.push(w.to_string());
        }
    }

    // 4. Verify against the record; evidence comes from the record, never the model.
    let mut evidence = Vec::new();
    let mut removed = 0u64;
    let mut seen = BTreeSet::new();
    for c in citations {
        let Some((tab, cmd, line)) = parse_address(&c, multi, only_tab) else {
            removed += 1;
            continue;
        };
        if !seen.insert((tab, cmd, line)) {
            continue;
        }
        let found =
            tabs.iter().find(|t| t.tab == tab).and_then(|t| t.commands.iter().find(|x| x.number == cmd)).and_then(
                |x| match line {
                    Some(l) if l >= 1 => x.output.get(l as usize - 1).cloned(),
                    Some(_) => None,
                    None => Some(format!("{}$ {}", x.location, x.text)),
                },
            );
        match found {
            Some(text) => evidence.push(Evidence { tab, command: cmd, line, text: redact(&text) }),
            None => removed += 1,
        }
    }
    evidence.sort_by_key(|e| (e.tab, e.command, e.line));
    let unsupported = evidence.is_empty();
    Ok(Answer {
        question: question.to_string(),
        answer: redact(&answer),
        confidence,
        evidence,
        removed_citations: removed,
        unsupported,
        coverage: Coverage {
            commands_read: read_commands.len() as u64,
            commands_total,
            lines_read,
            lines_total,
            stopped,
        },
        model_calls: calls,
        cached: false,
    })
}

fn thousands(n: u64) -> String {
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

/// The plain-text report.
pub fn render(a: &Answer, multi: bool) -> String {
    let mut out = String::from("Answer (written by a language model):\n");
    for line in a.answer.lines() {
        out.push_str(&format!("  {line}\n"));
    }
    out.push('\n');
    if a.unsupported {
        out.push_str("No evidence: none of the lines the model cited could be checked against the saved record, so treat this answer as unsupported.\n");
    } else {
        out.push_str("Evidence (each line checked against the saved record):\n");
        let labels: Vec<String> = a
            .evidence
            .iter()
            .map(|e| {
                let t = if multi || true { format!("tab {}, ", e.tab) } else { String::new() };
                match e.line {
                    Some(l) => format!("{t}command {}, line {l}:", e.command),
                    None => format!("{t}command {}:", e.command),
                }
            })
            .collect();
        let width = labels.iter().map(|l| l.chars().count()).max().unwrap_or(0);
        for (label, e) in labels.iter().zip(&a.evidence) {
            out.push_str(&format!("  {label:width$}   {}\n", e.text));
        }
    }
    if a.removed_citations > 0 {
        let n = a.removed_citations;
        out.push_str(&format!(
            "{} citation{} could not be verified and {} removed.\n",
            n,
            if n == 1 { "" } else { "s" },
            if n == 1 { "was" } else { "were" }
        ));
    }
    out.push('\n');
    let c = &a.coverage;
    let coverage = if c.commands_read >= c.commands_total {
        format!("read all {} commands ({} lines)", thousands(c.commands_total), thousands(c.lines_total))
    } else {
        format!(
            "read {} of {} commands; the rest were skipped{}",
            thousands(c.commands_read),
            thousands(c.commands_total),
            c.stopped.as_ref().map(|s| format!(" ({s})")).unwrap_or_default()
        )
    };
    let negative = if a.unsupported && c.commands_read >= c.commands_total {
        " \"Not found\" is the model's judgement."
    } else {
        ""
    };
    out.push_str(&format!(
        "Coverage: {coverage}. Confidence: {}.{negative}{}\n",
        a.confidence,
        if a.cached { " (cached answer)" } else { "" }
    ));
    if let Some(e) = a.evidence.first() {
        out.push_str(&format!("To see more: ctx read command {} in tab {}\n", e.command, e.tab));
    }
    out
}

/// Cache file name for an answer: per tabs (and their record lengths) and question.
pub fn cache_key(tabs: &[TabRecord], question: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    let mut feed = |b: &[u8]| {
        for x in b {
            h = (h ^ *x as u64).wrapping_mul(0x100000001b3);
        }
    };
    for t in tabs {
        feed(&t.tab.to_le_bytes());
        let lines: u64 = t.commands.iter().map(|c| c.output.len() as u64 + 1).sum();
        feed(&lines.to_le_bytes());
        feed(&(t.commands.len() as u64).to_le_bytes());
    }
    feed(question.trim().to_lowercase().as_bytes());
    format!("{h:016x}.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Fake {
        replies: Mutex<Vec<String>>,
        seen: Mutex<Vec<String>>,
    }

    impl Model for Fake {
        fn chat(&self, _system: &str, user: &str, _deadline: Instant) -> Result<String, String> {
            self.seen.lock().unwrap().push(user.to_string());
            let mut r = self.replies.lock().unwrap();
            if r.is_empty() { Ok("{}".into()) } else { Ok(r.remove(0)) }
        }
    }

    fn tab() -> TabRecord {
        TabRecord {
            tab: 212,
            name: None,
            commands: vec![
                Command {
                    number: 14,
                    text: "rg element_x src/".into(),
                    location: "c@m:~/site".into(),
                    exit_status: Some(0),
                    output: vec![
                        "src/ui/panel.rs:88: registry.add(element_x());".into(),
                        "src/ui/mod.rs:41: registry.add(element_x());".into(),
                    ],
                },
                Command {
                    number: 21,
                    text: "cargo test -p ui".into(),
                    location: "c@m:~/site".into(),
                    exit_status: Some(0),
                    output: vec![
                        "running 37 tests".into(),
                        "test result: ok. 37 passed; 0 failed".into(),
                        "export OPENAI_API_KEY=sk-proj-Qk3vT9xLm2Rp8WzA4bN6cY1dF7gH0jKs".into(),
                    ],
                },
            ],
        }
    }

    fn settings() -> Settings {
        Settings {
            max_context_tokens: 32_000,
            max_model_calls: 20,
            deadline: Instant::now() + std::time::Duration::from_secs(10),
        }
    }

    #[test]
    fn verified_evidence_and_removed_citations() {
        let fake = Fake {
            replies: Mutex::new(vec![r#"{"answer": "Found in command 14; fixed by 21.", "citations": ["C14L1", "C14L2", "C21L2", "C99L1", "C14L9"], "confidence": "high"}"#.into()]),
            seen: Mutex::new(vec![]),
        };
        let a = ask("where did it discover element_x registered twice?", &[tab()], &fake, &settings()).unwrap();
        assert_eq!(a.evidence.len(), 3);
        assert_eq!(a.evidence[0].text, "src/ui/panel.rs:88: registry.add(element_x());");
        assert_eq!(a.removed_citations, 2);
        assert!(!a.unsupported);
        assert_eq!(a.coverage.commands_read, 2);
        let text = render(&a, false);
        assert!(text.contains("tab 212, command 14, line 1:"), "{text}");
        assert!(text.contains("2 citations could not be verified and were removed"));
        assert!(text.contains("Coverage: read all 2 commands (5 lines). Confidence: high."));
    }

    #[test]
    fn secrets_never_reach_the_model_or_the_evidence() {
        let fake = Fake {
            replies: Mutex::new(vec![
                r#"{"answer": "The key is in C21L3", "citations": ["C21L3"], "confidence": "low"}"#.into(),
            ]),
            seen: Mutex::new(vec![]),
        };
        let a = ask("what key was exported?", &[tab()], &fake, &settings()).unwrap();
        for prompt in fake.seen.lock().unwrap().iter() {
            assert!(!prompt.contains("sk-proj-Qk3vT9"), "secret sent to the model");
        }
        assert!(a.evidence[0].text.contains("[redacted]"), "{:?}", a.evidence);
    }

    #[test]
    fn no_verified_evidence_is_unsupported() {
        let fake = Fake {
            replies: Mutex::new(vec![
                r#"{"answer": "It was in C7L3", "citations": ["C7L3"], "confidence": "high"}"#.into(),
            ]),
            seen: Mutex::new(vec![]),
        };
        let a = ask("anything?", &[tab()], &fake, &settings()).unwrap();
        assert!(a.unsupported);
        assert!(render(&a, false).contains("unsupported"));
    }

    #[test]
    fn chunks_stay_within_budget_and_calls_are_capped() {
        let mut big = tab();
        big.commands[1].output = (0..5000).map(|i| format!("log line {i} with some ordinary text in it")).collect();
        let fake = Fake { replies: Mutex::new(vec![]), seen: Mutex::new(vec![]) };
        let s = Settings {
            max_context_tokens: 4000,
            max_model_calls: 5,
            deadline: Instant::now() + std::time::Duration::from_secs(10),
        };
        let a = ask("what happened?", &[big], &fake, &s).unwrap();
        assert!(a.model_calls <= 5, "{}", a.model_calls);
        for p in fake.seen.lock().unwrap().iter() {
            assert!(tokens(p) + tokens(MAP_PROMPT) <= 4000 + 200, "prompt of {} tokens", tokens(p));
        }
        assert!(a.coverage.lines_read < a.coverage.lines_total);
        assert!(render(&a, false).contains("the rest were skipped"));
    }

    #[test]
    fn addresses() {
        assert_eq!(parse_address("C14L2", false, 5), Some((5, 14, Some(2))));
        assert_eq!(parse_address("T3C14", true, 5), Some((3, 14, None)));
        assert_eq!(parse_address("garbage", false, 5), None);
    }
}
