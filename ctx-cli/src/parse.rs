//! The sentence parser: take the verb, drop fillers, peel off labelled phrases in any order, and
//! what's left is the object. Ambiguity is an error with a suggested rewrite, never a guess.

use crate::grammar::{self, AFTER_FILLER, FILLERS, Kind, Object, P, Phrase, Slot, VERBS, Verb};
use ctx_protocol::{Filter, ReadWhat, Request, TabRef, WaitCondition, WindowRef};
use std::collections::BTreeMap;

/// One word of the sentence, as the shell passed it. An argument with a space in it was quoted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub text: String,
}

impl Token {
    fn is_word(&self, w: &str) -> bool {
        !self.text.contains(char::is_whitespace) && self.text.eq_ignore_ascii_case(w)
    }

    fn lower(&self) -> String {
        self.text.to_lowercase()
    }
}

/// What the parser needs from the world: the current folder, home, and the time now.
#[derive(Clone, Debug)]
pub struct Context {
    pub folder: String,
    pub home: String,
    pub now_ms: i64,
}

/// Output format chosen with `as …`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Format {
    #[default]
    Text,
    Json,
    Toon,
}

/// A value taken from a phrase or the object.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Flag,
    Tab(TabRef),
    Window(WindowRef),
    Folder(String),
    Count(u64),
    /// Milliseconds, with the words it was written as.
    Duration(u64, String),
    /// Unix seconds, with the words it was written as.
    Time(i64, String),
    Text(String),
    Keys(Vec<String>),
    Format(Format),
    Who(String),
    Agent(String),
    Name(String),
    Condition(TabRef, WaitCondition),
    AllTabs,
    Program(String),
    Noun(String),
    Read(ReadWhat),
}

/// What a sentence asks for.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Sent to the server as is.
    Server(Request),
    Help(Option<String>),
    InstallInstructions {
        agent: String,
        folder: Option<String>,
    },
    RemoveInstructions {
        agent: String,
        folder: Option<String>,
    },
    MakeLauncher {
        name: String,
        program: String,
        folder: String,
        any_arguments: bool,
        signed_by: Option<String>,
    },
    RebuildLauncher {
        name: String,
    },
    RemoveLauncher {
        name: String,
    },
    ListLaunchers,
    Ask {
        question: String,
        about: AskAbout,
        within_ms: Option<u64>,
    },
    Attach {
        tab: TabRef,
    },
    Restart {
        without_asking: bool,
    },
    RunServer,
    Mcp,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AskAbout {
    Tab(TabRef),
    AllTabs(Filter),
}

/// A parsed sentence.
#[derive(Clone, Debug, PartialEq)]
pub struct Sentence {
    pub command: Command,
    pub format: Format,
    /// The sentence in canonical order, as help and the activity log print it.
    pub canonical: String,
}

/// A sentence that didn't parse: what was wrong, and the closest valid sentence if there's one
/// clear fix.
#[derive(Clone, Debug, PartialEq)]
pub struct ParseError {
    pub message: String,
    pub suggestion: Option<String>,
}

impl ParseError {
    fn new(message: impl Into<String>) -> Self {
        ParseError { message: message.into(), suggestion: None }
    }
}

/// Parsed phrases and object, before they become a [`Command`].
#[derive(Clone, Debug)]
struct Parts {
    verb: &'static Verb,
    phrases: BTreeMap<P, (Value, String)>,
    object: Option<(Value, String)>,
}

/// Parse `argv` (without the program name).
pub fn parse(args: &[String], ctx: &Context) -> Result<Sentence, ParseError> {
    let tokens: Vec<Token> =
        args.iter().map(|a| Token { text: a.trim().to_string() }).filter(|t| !t.text.is_empty()).collect();
    match parse_tokens(&tokens, ctx) {
        Ok(s) => Ok(s),
        Err(mut e) => {
            if e.suggestion.is_none() {
                e.suggestion = repair(&tokens, ctx);
            }
            Err(e)
        }
    }
}

fn parse_tokens(tokens: &[Token], ctx: &Context) -> Result<Sentence, ParseError> {
    let Some(first) = tokens.first() else {
        return Ok(Sentence { command: Command::Help(None), format: Format::Text, canonical: "ctx help".into() });
    };
    let verb_word = first.lower();
    let Some(verb) = grammar::verb(&verb_word) else {
        return Err(ParseError::new(format!("\"{}\" isn't a ctx verb.", first.text)));
    };
    let words = drop_fillers(&tokens[1..]);
    let parts = peel(verb, &words, ctx)?;
    build(parts, ctx)
}

/// Drop `the`, `a` and `an` where they're English filler: before a word the grammar knows.
fn drop_fillers(tokens: &[Token]) -> Vec<Token> {
    let mut out = Vec::with_capacity(tokens.len());
    for (i, t) in tokens.iter().enumerate() {
        let is_filler = FILLERS.iter().any(|f| t.is_word(f));
        let next_is_known =
            tokens.get(i + 1).is_some_and(|n| AFTER_FILLER.iter().any(|w| n.is_word(w)) || is_quoted_name(n));
        if is_filler && next_is_known {
            continue;
        }
        out.push(t.clone());
    }
    out
}

/// A token that can only be a quoted tab name (has a space and a letter): `the "Dev server" tab`.
fn is_quoted_name(t: &Token) -> bool {
    t.text.contains(' ') && t.text.chars().any(char::is_alphabetic) && t.text.len() <= 64
}

fn phrases_of(verb: &Verb) -> impl Iterator<Item = &Phrase> {
    verb.order.iter().filter_map(|s| match s {
        Slot::Phrase(p) => Some(p),
        Slot::Object => None,
    })
}

/// Peel labelled phrases off in any order; collect the rest as the object.
fn peel(verb: &'static Verb, tokens: &[Token], ctx: &Context) -> Result<Parts, ParseError> {
    peel_spans(verb, tokens, ctx).map(|(p, _)| p)
}

/// Like `peel`, and also returns each phrase's token range (for tests that shuffle phrases).
fn peel_spans(
    verb: &'static Verb,
    tokens: &[Token],
    ctx: &Context,
) -> Result<(Parts, Vec<(usize, usize)>), ParseError> {
    let mut spans = Vec::new();
    let mut phrases: BTreeMap<P, (Value, String)> = BTreeMap::new();
    let mut leftover: Vec<Token> = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        // Longest matching phrase first (`without asking` vs `without waiting`).
        let mut candidates: Vec<&Phrase> = phrases_of(verb)
            .filter(|p| p.words.iter().enumerate().all(|(k, w)| tokens.get(i + k).is_some_and(|t| t.is_word(w))))
            .collect();
        candidates.sort_by_key(|p| std::cmp::Reverse(p.words.len()));
        if let Some(p) = candidates.first() {
            let start = i + p.words.len();
            let (value, used, shown) = parse_value(p.kind, &tokens[start..], ctx, p)?;
            let mut end = start + used;
            if let Some(t) = tokens.get(end)
                && p.after.iter().any(|w| t.is_word(w))
            {
                end += 1;
            }
            if phrases.contains_key(&p.id) {
                return Err(ParseError::new(format!("`{}` appears twice.", p.words.join(" "))));
            }
            phrases.insert(p.id, (value, shown));
            spans.push((i, end));
            i = end;
            continue;
        }
        leftover.push(tokens[i].clone());
        i += 1;
    }
    let object = parse_object(verb, &leftover, ctx, &phrases)?;
    Ok((Parts { verb, phrases, object }, spans))
}

/// A sentence cut into its verb, its object words and its phrases (each phrase's words kept
/// together), so tests can reorder phrases. `None` if it doesn't parse.
pub fn chunks(args: &[String], ctx: &Context) -> Option<(String, Vec<String>, Vec<Vec<String>>)> {
    let tokens: Vec<Token> =
        args.iter().map(|a| Token { text: a.trim().to_string() }).filter(|t| !t.text.is_empty()).collect();
    let verb = grammar::verb(&tokens.first()?.lower())?;
    let words = drop_fillers(&tokens[1..]);
    let (_, spans) = peel_spans(verb, &words, ctx).ok()?;
    let mut object = Vec::new();
    let mut phrases = Vec::new();
    let mut i = 0;
    for (a, b) in spans {
        object.extend(words[i..a].iter().map(|t| t.text.clone()));
        phrases.push(words[a..b].iter().map(|t| t.text.clone()).collect());
        i = b;
    }
    object.extend(words[i..].iter().map(|t| t.text.clone()));
    Some((tokens[0].text.clone(), object, phrases))
}

fn missing(p: &Phrase) -> ParseError {
    let words = p.words.join(" ");
    ParseError::new(format!("`{words}` needs {} after it.", describe(p.kind)))
}

fn describe(kind: Kind) -> &'static str {
    match kind {
        Kind::Flag => "nothing",
        Kind::Tab => "a tab: its name in quotes, or tab and its number",
        Kind::Window => "a window: window and its number or name",
        Kind::TabOrWindow => "a tab or a window",
        Kind::Folder => "a folder",
        Kind::Count => "a number",
        Kind::Duration => "a length of time, such as 10 minutes",
        Kind::Time => "a time, such as 30 days ago or yesterday",
        Kind::Text => "some text in quotes",
        Kind::Key | Kind::Keys => "a key, such as enter or ctrl-c",
        Kind::Format => "json or toon",
        Kind::Who => "me or an agent's name, such as claude",
        Kind::Agent => "claude, codex, gemini or agents",
        Kind::Name => "a name in quotes",
        Kind::Condition => "a tab and what to wait for, such as \"Dev server\" is idle",
        Kind::TabOrAllTabs => "a tab, or all tabs",
        Kind::Program => "a program's path",
    }
}

/// Parse one value of `kind` from the start of `tokens`. Returns the value, how many tokens it
/// used, and how it's shown in the canonical sentence.
fn parse_value(kind: Kind, tokens: &[Token], ctx: &Context, p: &Phrase) -> Result<(Value, usize, String), ParseError> {
    if kind == Kind::Flag {
        return Ok((Value::Flag, 0, String::new()));
    }
    let first = tokens.first().ok_or_else(|| missing(p))?;
    match kind {
        Kind::Flag => Ok((Value::Flag, 0, String::new())),
        Kind::Tab => parse_tab(tokens).map(|(t, n)| (Value::Tab(t.clone()), n, show_tab(&t))).ok_or_else(|| missing(p)),
        Kind::Window => {
            parse_window(tokens).map(|(w, n)| (Value::Window(w.clone()), n, show_window(&w))).ok_or_else(|| missing(p))
        }
        Kind::TabOrWindow => {
            if let Some((w, n)) = parse_window(tokens) {
                Ok((Value::Window(w.clone()), n, show_window(&w)))
            } else {
                parse_tab(tokens).map(|(t, n)| (Value::Tab(t.clone()), n, show_tab(&t))).ok_or_else(|| missing(p))
            }
        }
        Kind::Folder | Kind::Program => {
            if grammar::is_sentence_word(&first.text) && !first.text.contains(['/', '~', '.']) {
                return Err(missing(p));
            }
            let path = resolve_path(&first.text, ctx);
            let shown = show_text_if_needed(&first.text);
            let value = if kind == Kind::Folder { Value::Folder(path) } else { Value::Program(path) };
            Ok((value, 1, shown))
        }
        Kind::Count => parse_count(&first.text).map(|n| (Value::Count(n), 1, n.to_string())).ok_or_else(|| missing(p)),
        Kind::Duration => parse_duration(tokens)
            .map(|(ms, n, shown)| (Value::Duration(ms, shown.clone()), n, shown))
            .ok_or_else(|| missing(p)),
        Kind::Time => parse_time(tokens, ctx)
            .map(|(s, n, shown)| (Value::Time(s, shown.clone()), n, shown))
            .ok_or_else(|| missing(p)),
        Kind::Text => Ok((Value::Text(first.text.clone()), 1, quote(&first.text))),
        Kind::Key => {
            if grammar::is_key(&first.text) {
                Ok((Value::Keys(vec![first.lower()]), 1, first.lower()))
            } else {
                Err(ParseError::new(format!(
                    "\"{}\" isn't a key ctx knows. Keys are named like enter, escape, up, ctrl-c, f5.",
                    first.text
                )))
            }
        }
        Kind::Keys => parse_keys(tokens).map(|(k, n)| (Value::Keys(k.clone()), n, k.join(" then "))),
        Kind::Format => match first.lower().as_str() {
            "json" => Ok((Value::Format(Format::Json), 1, "json".into())),
            "toon" => Ok((Value::Format(Format::Toon), 1, "toon".into())),
            _ => Err(ParseError::new(format!("`as` takes json or toon, not \"{}\".", first.text))),
        },
        Kind::Who => {
            let who = first.lower();
            if grammar::is_sentence_word(&who) && who != "me" {
                return Err(missing(p));
            }
            Ok((Value::Who(who.clone()), 1, who))
        }
        Kind::Agent => {
            let agent = first.lower();
            if ["claude", "codex", "gemini", "agents"].contains(&agent.as_str()) {
                Ok((Value::Agent(agent.clone()), 1, agent))
            } else {
                Err(ParseError::new(format!(
                    "ctx has instructions for claude, codex, gemini, or agents at a project folder, not \"{}\".",
                    first.text
                )))
            }
        }
        Kind::Name => {
            if grammar::is_sentence_word(&first.text) {
                return Err(ParseError::new(format!(
                    "\"{}\" is one of ctx's sentence words, so it can't be a name. Try a longer name, such as \"{} tab\".",
                    first.text, first.text
                )));
            }
            Ok((Value::Name(first.text.clone()), 1, quote(&first.text)))
        }
        Kind::Condition => parse_condition(tokens).map(|(t, c, n, shown)| (Value::Condition(t, c), n, shown)),
        Kind::TabOrAllTabs => {
            if first.is_word("all") && tokens.get(1).is_some_and(|t| t.is_word("tabs")) {
                Ok((Value::AllTabs, 2, "all tabs".into()))
            } else {
                parse_tab(tokens).map(|(t, n)| (Value::Tab(t.clone()), n, show_tab(&t))).ok_or_else(|| missing(p))
            }
        }
    }
}

/// `tab 42`, `this tab`, or a name (one token).
fn parse_tab(tokens: &[Token]) -> Option<(TabRef, usize)> {
    let first = tokens.first()?;
    if first.is_word("tab") {
        let n = parse_count(&tokens.get(1)?.text)?;
        return Some((TabRef::Number(n), 2));
    }
    if first.is_word("this") && tokens.get(1).is_some_and(|t| t.is_word("tab")) {
        return Some((TabRef::This, 2));
    }
    if grammar::is_sentence_word(&first.text) || !first.text.chars().any(char::is_alphabetic) {
        return None;
    }
    // `"Dev server" tab`: a trailing `tab` after a name is English, not a new phrase.
    let used = if tokens.get(1).is_some_and(|t| t.is_word("tab"))
        && !tokens.get(2).is_some_and(|t| parse_count(&t.text).is_some())
    {
        2
    } else {
        1
    };
    Some((TabRef::Name(first.text.clone()), used))
}

fn parse_window(tokens: &[Token]) -> Option<(WindowRef, usize)> {
    if tokens.first()?.is_word("new") && tokens.get(1).is_some_and(|t| t.is_word("window")) {
        return Some((WindowRef::New, 2));
    }
    if !tokens.first()?.is_word("window") {
        return None;
    }
    let second = tokens.get(1)?;
    if let Some(n) = parse_count(&second.text) {
        return Some((WindowRef::Number(n), 2));
    }
    if grammar::is_sentence_word(&second.text) {
        return None;
    }
    Some((WindowRef::Name(second.text.clone()), 2))
}

pub fn parse_count(s: &str) -> Option<u64> {
    if s.is_empty() || !s.chars().next()?.is_ascii_digit() {
        return None;
    }
    let digits: String = s.chars().filter(|c| *c != ',').collect();
    if !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

fn unit_ms(unit: &str) -> Option<u64> {
    Some(match unit.trim_end_matches('s') {
        "second" => 1000,
        "minute" => 60_000,
        "hour" => 3_600_000,
        "day" => 86_400_000,
        "week" => 7 * 86_400_000,
        "month" => 30 * 86_400_000,
        _ => return None,
    })
}

/// `10 minutes`, `a minute`, `90 seconds`, also `1 hour 30 minutes`.
fn parse_duration(tokens: &[Token]) -> Option<(u64, usize, String)> {
    let mut total = 0u64;
    let mut used = 0;
    let mut shown = Vec::new();
    while used + 1 < tokens.len() + 1 {
        let n = match tokens.get(used)?.lower().as_str() {
            "a" | "an" | "one" => 1,
            other => match parse_count(other) {
                Some(n) => n,
                None => break,
            },
        };
        let Some(unit_tok) = tokens.get(used + 1) else { break };
        let Some(ms) = unit_ms(&unit_tok.lower()) else { break };
        total += n * ms;
        let unit = unit_tok.lower().trim_end_matches('s').to_string();
        shown.push(if n == 1 { format!("1 {unit}") } else { format!("{n} {unit}s") });
        used += 2;
        if tokens.get(used).is_none() {
            break;
        }
    }
    if used == 0 { None } else { Some((total, used, shown.join(" "))) }
}

/// `30 days ago`, `1 hour ago`, `yesterday`, `today`, `2026-10-01`.
fn parse_time(tokens: &[Token], ctx: &Context) -> Option<(i64, usize, String)> {
    let first = tokens.first()?.lower();
    let now = ctx.now_ms / 1000;
    match first.as_str() {
        "today" => return Some((now - now.rem_euclid(86_400), 1, "today".into())),
        "yesterday" => return Some((now - now.rem_euclid(86_400) - 86_400, 1, "yesterday".into())),
        _ => {}
    }
    if let Some(ms) = ctx_protocol::time::parse_iso(&first) {
        return Some((ms / 1000, 1, first));
    }
    let (ms, used, shown) = parse_duration(tokens)?;
    if !tokens.get(used).is_some_and(|t| t.is_word("ago")) {
        return None;
    }
    Some((now - (ms / 1000) as i64, used + 1, format!("{shown} ago")))
}

fn parse_keys(tokens: &[Token]) -> Result<(Vec<String>, usize), ParseError> {
    let mut keys = Vec::new();
    let mut i = 0;
    while let Some(t) = tokens.get(i) {
        if !grammar::is_key(&t.text) || t.text.contains(' ') {
            return Err(ParseError::new(format!(
                "\"{}\" isn't a key ctx knows. Keys are named like enter, escape, up, ctrl-c, f5.",
                t.text
            )));
        }
        keys.push(t.lower());
        i += 1;
        if tokens.get(i).is_some_and(|t| t.is_word("then")) && tokens.get(i + 1).is_some() {
            i += 1;
        } else {
            break;
        }
    }
    if keys.is_empty() {
        return Err(ParseError::new("Which key? Keys are named like enter, escape, up, ctrl-c, f5."));
    }
    Ok((keys, i))
}

fn parse_condition(tokens: &[Token]) -> Result<(TabRef, WaitCondition, usize, String), ParseError> {
    let err = || {
        ParseError::new(
            "`until` needs a tab and what to wait for: \"<tab>\" is idle, \"<tab>\" shows \"<text>\", or \"<tab>\" is quiet for 2 seconds.",
        )
    };
    // The tab may be omitted inside CTX: `wait until idle`.
    let (tab, used) = if tokens.first().is_some_and(|t| t.is_word("is") || t.is_word("shows")) {
        (TabRef::This, 0)
    } else {
        parse_tab(tokens).ok_or_else(err)?
    };
    let rest = &tokens[used..];
    let shown_tab = if tab == TabRef::This && used == 0 { String::new() } else { format!("{} ", show_tab(&tab)) };
    if rest.first().is_some_and(|t| t.is_word("shows")) {
        let text = rest.get(1).ok_or_else(err)?;
        return Ok((
            tab,
            WaitCondition::Shows(text.text.clone()),
            used + 2,
            format!("{shown_tab}shows {}", quote(&text.text)),
        ));
    }
    if rest.first().is_some_and(|t| t.is_word("is")) {
        if rest.get(1).is_some_and(|t| t.is_word("idle")) {
            return Ok((tab, WaitCondition::Idle, used + 2, format!("{shown_tab}is idle")));
        }
        if rest.get(1).is_some_and(|t| t.is_word("quiet")) && rest.get(2).is_some_and(|t| t.is_word("for")) {
            let (ms, n, shown) = parse_duration(&rest[3..]).ok_or_else(err)?;
            return Ok((tab, WaitCondition::QuietFor(ms), used + 3 + n, format!("{shown_tab}is quiet for {shown}")));
        }
    }
    Err(err())
}

/// The object: whatever's left after the phrases.
fn parse_object(
    verb: &Verb,
    left: &[Token],
    ctx: &Context,
    phrases: &BTreeMap<P, (Value, String)>,
) -> Result<Option<(Value, String)>, ParseError> {
    match verb.object {
        Object::None => {
            if let Some(t) = left.first() {
                return Err(ParseError::new(format!(
                    "I don't know what \"{}\" means after `ctx {}`.",
                    t.text, verb.name
                )));
            }
            Ok(None)
        }
        Object::Noun(nouns) => {
            let words: Vec<String> = left.iter().map(Token::lower).collect();
            // `remove launcher "sysadmin"`, `rebuild launcher "sysadmin"`: the noun, then a name.
            if (verb.name == "remove" || verb.name == "rebuild")
                && words.first().map(String::as_str) == Some("launcher")
            {
                let name = left.get(1).ok_or_else(|| {
                    ParseError::new(format!(
                        "Which launcher? `ctx {} launcher \"<name>\"`. `ctx list launchers` shows them.",
                        verb.name
                    ))
                })?;
                if left.len() > 2 {
                    return Err(ParseError::new(format!("I don't know what \"{}\" means here.", left[2].text)));
                }
                return Ok(Some((Value::Name(name.text.clone()), format!("launcher {}", quote(&name.text)))));
            }
            let joined = words.join(" ");
            if let Some(n) = nouns.iter().find(|n| **n == joined) {
                return Ok(Some((Value::Noun(n.to_string()), n.to_string())));
            }
            let options = nouns.iter().map(|n| format!("`ctx {} {n}`", verb.name)).collect::<Vec<_>>().join(", ");
            if left.is_empty() {
                return Err(ParseError::new(format!("{} what? Say {options}.", capitalise(verb.name))));
            }
            Err(ParseError::new(format!(
                "I don't know how to {} \"{}\". You can say {options}.",
                verb.name,
                left.iter().map(|t| t.text.as_str()).collect::<Vec<_>>().join(" ")
            )))
        }
        Object::ReadSelector => parse_read_selector(left, phrases).map(Some),
        Object::Kind(kind) | Object::Optional(kind) => {
            if left.is_empty() {
                if matches!(verb.object, Object::Optional(_)) {
                    return Ok(None);
                }
                return Err(ParseError::new(format!("`ctx {}` needs {}.", verb.name, describe(kind))));
            }
            let fake = Phrase { id: P::In, words: &[], kind, after: &[], meaning: "" };
            let (value, used, shown) = parse_value(kind, left, ctx, &fake)?;
            if used != left.len() {
                return Err(too_many_words(verb, left));
            }
            Ok(Some((value, shown)))
        }
    }
}

fn too_many_words(verb: &Verb, left: &[Token]) -> ParseError {
    let what = match verb.name {
        "run" => "the command",
        "type" => "the text to type",
        "find" | "ask" => "the text",
        "close" | "rename" => "the tab",
        _ => "the object",
    };
    ParseError::new(format!(
        "I couldn't tell which part is {what} (\"{}\"). Put it in quotes:",
        left.iter().map(|t| t.text.as_str()).collect::<Vec<_>>().join(" ")
    ))
}

fn parse_read_selector(left: &[Token], phrases: &BTreeMap<P, (Value, String)>) -> Result<(Value, String), ParseError> {
    let words: Vec<String> = left.iter().map(Token::lower).collect();
    let w: Vec<&str> = words.iter().map(String::as_str).collect();
    let selector = match w.as_slice() {
        [] => Some((ReadWhat::Screen, String::new(), 0)),
        ["everything", ..] => Some((ReadWhat::Everything, "everything".into(), 1)),
        ["screen", ..] => Some((ReadWhat::Screen, String::new(), 1)),
        ["last", "command", ..] => Some((ReadWhat::LastCommands(1), "last command".into(), 2)),
        ["last", n, "commands" | "command", ..] if parse_count(n).is_some() => {
            let n = parse_count(n).unwrap() as u32;
            Some((
                ReadWhat::LastCommands(n),
                if n == 1 { "last command".into() } else { format!("last {n} commands") },
                3,
            ))
        }
        ["last", n, "lines" | "line", ..] if parse_count(n).is_some() => {
            let n = parse_count(n).unwrap() as u32;
            Some((ReadWhat::LastLines(n), if n == 1 { "last line".into() } else { format!("last {n} lines") }, 3))
        }
        ["last", "line", ..] => Some((ReadWhat::LastLines(1), "last line".into(), 2)),
        ["command", n, ..] if parse_count(n).is_some() => {
            let n = parse_count(n).unwrap();
            Some((ReadWhat::Command(n), format!("command {n}"), 2))
        }
        _ => None,
    };
    let has_in = phrases.contains_key(&P::In);
    match selector {
        Some((what, shown, used)) => {
            let rest = &left[used..];
            if rest.is_empty() {
                return Ok((Value::Read(what), shown));
            }
            // `read last 3 commands "Dev server"`: a tab without `in`.
            if !has_in
                && let Some((tab, n)) = parse_tab(rest)
                && n == rest.len()
            {
                let _ = tab;
                return Err(ParseError::new("Put `in` before the tab:"));
            }
            Err(ParseError::new(format!(
                "I don't know what \"{}\" means here.",
                rest.iter().map(|t| t.text.as_str()).collect::<Vec<_>>().join(" ")
            )))
        }
        None => {
            // `read "Dev server"`: the object is the tab itself.
            if let Some((tab, n)) = parse_tab(left)
                && n == left.len()
            {
                if has_in {
                    return Err(ParseError::new(
                        "Name the tab once: `ctx read \"<tab>\"` or `ctx read last 20 lines in \"<tab>\"`.",
                    ));
                }
                return Ok((Value::Tab(tab.clone()), show_tab(&tab)));
            }
            Err(ParseError::new(format!(
                "I don't know how to read \"{}\". Say a tab, or everything, last 200 lines, last command, last 3 commands, or command 14, followed by in \"<tab>\".",
                left.iter().map(|t| t.text.as_str()).collect::<Vec<_>>().join(" ")
            )))
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Building the command
// ---------------------------------------------------------------------------------------------

fn build(parts: Parts, _ctx: &Context) -> Result<Sentence, ParseError> {
    let canonical = canonical(&parts);
    let format = match parts.phrases.get(&P::As) {
        Some((Value::Format(f), _)) => *f,
        _ => Format::Text,
    };
    let command = to_command(&parts)?;
    Ok(Sentence { command, format, canonical })
}

/// Print the sentence in the verb's canonical order.
fn canonical(parts: &Parts) -> String {
    let mut out = vec!["ctx".to_string(), parts.verb.name.to_string()];
    for slot in parts.verb.order {
        match slot {
            Slot::Object => {
                if let Some((_, shown)) = &parts.object
                    && !shown.is_empty()
                {
                    out.push(shown.clone());
                }
            }
            Slot::Phrase(p) => {
                if let Some((_, shown)) = parts.phrases.get(&p.id) {
                    out.push(p.words.join(" "));
                    if !shown.is_empty() {
                        out.push(shown.clone());
                    }
                    if let Some(after) = p.after.first() {
                        out.push(after.to_string());
                    }
                }
            }
        }
    }
    out.join(" ")
}

fn tab_phrase(parts: &Parts, id: P) -> TabRef {
    match parts.phrases.get(&id) {
        Some((Value::Tab(t), _)) => t.clone(),
        _ => TabRef::This,
    }
}

fn flag(parts: &Parts, id: P) -> bool {
    parts.phrases.contains_key(&id)
}

fn count(parts: &Parts, id: P) -> Option<u64> {
    match parts.phrases.get(&id) {
        Some((Value::Count(n), _)) => Some(*n),
        _ => None,
    }
}

fn duration(parts: &Parts, id: P) -> Option<u64> {
    match parts.phrases.get(&id) {
        Some((Value::Duration(ms, _), _)) => Some(*ms),
        _ => None,
    }
}

fn text_of(v: &Option<(Value, String)>) -> Option<String> {
    match v {
        Some((Value::Text(t) | Value::Name(t), _)) => Some(t.clone()),
        _ => None,
    }
}

fn filter(parts: &Parts) -> Filter {
    Filter {
        tab: match parts.phrases.get(&P::In) {
            Some((Value::Tab(t), _)) => Some(t.clone()),
            _ => None,
        },
        folder: match parts.phrases.get(&P::At) {
            Some((Value::Folder(f), _)) => Some(f.clone()),
            _ => None,
        },
        by: match parts.phrases.get(&P::By) {
            Some((Value::Who(w), _)) => Some(w.clone()),
            _ => None,
        },
        since: match parts.phrases.get(&P::Since) {
            Some((Value::Time(s, _), _)) => Some(*s),
            _ => None,
        },
    }
}

fn noun(parts: &Parts) -> &str {
    match &parts.object {
        Some((Value::Noun(n), _)) => n.as_str(),
        _ => "",
    }
}

fn name_phrase(parts: &Parts, id: P) -> Option<String> {
    match parts.phrases.get(&id) {
        Some((Value::Name(n), _)) => Some(n.clone()),
        _ => None,
    }
}

fn folder_phrase(parts: &Parts) -> Option<String> {
    match parts.phrases.get(&P::At) {
        Some((Value::Folder(f), _)) => Some(f.clone()),
        _ => None,
    }
}

fn to_command(parts: &Parts) -> Result<Command, ParseError> {
    let verb = parts.verb.name;
    let not_with = |id: P, why: &str| -> Result<(), ParseError> {
        if parts.phrases.contains_key(&id) { Err(ParseError::new(why.to_string())) } else { Ok(()) }
    };
    Ok(match verb {
        "list" => match noun(parts) {
            "tabs" => {
                for id in [P::In, P::By, P::Since, P::UpTo] {
                    not_with(id, "`ctx list tabs` lists every open tab; it takes no other phrases.")?;
                }
                Command::Server(Request::ListTabs)
            }
            "closed tabs" => {
                for id in [P::In, P::By, P::Since] {
                    not_with(
                        id,
                        "`ctx list closed tabs` takes only `up to <number>`. Use `ctx find` to search closed tabs.",
                    )?;
                }
                Command::Server(Request::ListClosedTabs { limit: count(parts, P::UpTo).unwrap_or(20) as u32 })
            }
            "commands" => {
                not_with(P::By, "`by` works with `ctx find` and `ctx list activity`.")?;
                not_with(P::Since, "`since` works with `ctx find` and `ctx list activity`.")?;
                Command::Server(Request::ListCommands {
                    tab: tab_phrase(parts, P::In),
                    limit: count(parts, P::UpTo).map(|n| n as u32),
                })
            }
            "activity" => Command::Server(Request::ListActivity {
                filter: filter(parts),
                limit: count(parts, P::UpTo).unwrap_or(50) as u32,
            }),
            "launchers" => Command::ListLaunchers,
            _ => unreachable!(),
        },
        "read" => {
            let (tab, what) = match &parts.object {
                Some((Value::Tab(t), _)) => (t.clone(), ReadWhat::Screen),
                Some((Value::Read(w), _)) => (tab_phrase(parts, P::In), w.clone()),
                _ => (tab_phrase(parts, P::In), ReadWhat::Screen),
            };
            Command::Server(Request::Read {
                tab,
                what,
                colors: flag(parts, P::Colors),
                max_lines: count(parts, P::UpTo).map(|n| n as u32),
            })
        }
        "find" => Command::Server(Request::Find {
            text: text_of(&parts.object).unwrap_or_default(),
            filter: filter(parts),
            limit: count(parts, P::UpTo).unwrap_or(20) as u32,
        }),
        "ask" => {
            let question = text_of(&parts.object).unwrap_or_default();
            let about = match parts.phrases.get(&P::About) {
                Some((Value::AllTabs, _)) => AskAbout::AllTabs(Filter { tab: None, ..filter(parts) }),
                Some((Value::Tab(t), _)) => {
                    if parts.phrases.contains_key(&P::At)
                        || parts.phrases.contains_key(&P::Since)
                        || parts.phrases.contains_key(&P::By)
                    {
                        return Err(ParseError::new(
                            "`at`, `by` and `since` narrow `about all tabs`; with one tab, leave them out.",
                        ));
                    }
                    AskAbout::Tab(t.clone())
                }
                _ => AskAbout::Tab(TabRef::This),
            };
            Command::Ask { question, about, within_ms: duration(parts, P::Within) }
        }
        "status" => Command::Server(Request::Status),
        "open" => {
            let folder = folder_phrase(parts);
            let name = name_phrase(parts, P::Named);
            match noun(parts) {
                "window" => {
                    not_with(
                        P::In,
                        "A new window isn't in another window. To add a tab to a window: ctx open tab in window 2",
                    )?;
                    Command::Server(Request::OpenWindow { folder, name })
                }
                _ => {
                    let window = match parts.phrases.get(&P::In) {
                        Some((Value::Window(w), _)) => Some(w.clone()),
                        _ => None,
                    };
                    Command::Server(Request::OpenTab { window, folder, name })
                }
            }
        }
        "close" => match &parts.object {
            Some((Value::Window(w), _)) => Command::Server(Request::CloseWindow {
                window: w.clone(),
                without_asking: flag(parts, P::WithoutAsking),
            }),
            Some((Value::Tab(t), _)) => {
                Command::Server(Request::CloseTab { tab: t.clone(), without_asking: flag(parts, P::WithoutAsking) })
            }
            _ => unreachable!(),
        },
        "rename" => {
            let to = name_phrase(parts, P::To)
                .ok_or_else(|| ParseError::new("Rename to what? `ctx rename \"<tab>\" to \"<new name>\"`."))?;
            match &parts.object {
                Some((Value::Window(w), _)) => Command::Server(Request::RenameWindow { window: w.clone(), to }),
                Some((Value::Tab(t), _)) => Command::Server(Request::RenameTab { tab: t.clone(), to }),
                _ => unreachable!(),
            }
        }
        "move" => {
            let to = match parts.phrases.get(&P::To) {
                Some((Value::Window(w), _)) => w.clone(),
                _ => return Err(ParseError::new("Move it where? `ctx move \"<tab>\" to window 2`.")),
            };
            let tab = object_tab(parts);
            Command::Server(Request::Move { tab, to, position: count(parts, P::Position).map(|n| n as u32) })
        }
        "clear" => Command::Server(Request::Clear { tab: object_tab(parts) }),
        "reopen" | "forget" => {
            let n = match object_tab(parts) {
                TabRef::Number(n) => n,
                _ => {
                    return Err(ParseError::new(format!(
                        "Closed tabs are named by number: `ctx {verb} tab 40`. `ctx list closed tabs` shows them."
                    )));
                }
            };
            if verb == "reopen" {
                Command::Server(Request::Reopen { tab: n })
            } else {
                Command::Server(Request::Forget { tab: n, without_asking: flag(parts, P::WithoutAsking) })
            }
        }
        "run" => {
            if flag(parts, P::WithoutWaiting) && parts.phrases.contains_key(&P::Within) {
                return Err(ParseError::new(
                    "`without waiting` returns at once, so `within` has nothing to limit. Use one or the other.",
                ));
            }
            Command::Server(Request::Run {
                tab: tab_phrase(parts, P::In),
                command: text_of(&parts.object).unwrap_or_default(),
                colors: flag(parts, P::Colors),
                max_lines: count(parts, P::UpTo).map(|n| n as u32),
                within_ms: duration(parts, P::Within),
                wait: !flag(parts, P::WithoutWaiting),
            })
        }
        "type" => Command::Server(Request::Type {
            tab: tab_phrase(parts, P::Into),
            text: text_of(&parts.object).unwrap_or_default(),
            then_press: match parts.phrases.get(&P::ThenPress) {
                Some((Value::Keys(k), _)) => k.first().cloned(),
                _ => None,
            },
        }),
        "press" => {
            let keys = match &parts.object {
                Some((Value::Keys(k), _)) => k.clone(),
                _ => vec![],
            };
            Command::Server(Request::Press { tab: tab_phrase(parts, P::In), keys })
        }
        "wait" => {
            let (tab, until) = match parts.phrases.get(&P::Until) {
                Some((Value::Condition(t, c), _)) => (t.clone(), c.clone()),
                _ => {
                    return Err(ParseError::new(
                        "Wait for what? `ctx wait until \"<tab>\" is idle`, `… shows \"<text>\"` or `… is quiet for 2 seconds`.",
                    ));
                }
            };
            Command::Server(Request::Wait { tab, until, within_ms: duration(parts, P::Within).unwrap_or(5 * 60_000) })
        }
        "show" => Command::Server(Request::Show { tab: object_tab(parts) }),
        "attach" => Command::Attach { tab: tab_phrase(parts, P::To) },
        "install" | "remove" if noun(parts) == "instructions" || verb == "install" => {
            let agent = match parts.phrases.get(&P::For) {
                Some((Value::Agent(a), _)) => a.clone(),
                _ => {
                    return Err(ParseError::new(format!(
                        "Instructions for which agent? `ctx {verb} instructions for claude` (or codex, gemini, or agents at a project folder)."
                    )));
                }
            };
            let folder = folder_phrase(parts);
            if agent == "agents" && folder.is_none() {
                return Err(ParseError::new(format!(
                    "Which project? `ctx {verb} instructions for agents at ~/code/site`."
                )));
            }
            if agent != "agents" && folder.is_some() {
                return Err(ParseError::new(format!(
                    "`at` is for project instructions: `ctx {verb} instructions for agents at <folder>`."
                )));
            }
            if verb == "install" {
                Command::InstallInstructions { agent, folder }
            } else {
                Command::RemoveInstructions { agent, folder }
            }
        }
        "remove" => Command::RemoveLauncher { name: text_of(&parts.object).unwrap_or_default() },
        "rebuild" => match &parts.object {
            Some((Value::Name(n), _)) => Command::RebuildLauncher { name: n.clone() },
            _ => {
                return Err(ParseError::new(
                    "Which launcher? `ctx rebuild launcher \"<name>\"`. `ctx list launchers` shows them.",
                ));
            }
        },
        "make" => {
            let name = name_phrase(parts, P::Named).ok_or_else(|| {
                ParseError::new("Name the launcher: `ctx make launcher named \"<name>\" for <program> at <folder>`.")
            })?;
            let program = match parts.phrases.get(&P::For) {
                Some((Value::Program(p), _)) => p.clone(),
                _ => return Err(ParseError::new("Which program should it start? `… for ~/.local/bin/claude`.")),
            };
            let folder = folder_phrase(parts)
                .ok_or_else(|| ParseError::new("Which folder should it start in? `… at ~/Agents/sys-admin`."))?;
            Command::MakeLauncher {
                name,
                program,
                folder,
                any_arguments: flag(parts, P::AllowingAnyArguments),
                signed_by: name_phrase(parts, P::SignedBy),
            }
        }
        "restart" => Command::Restart { without_asking: flag(parts, P::WithoutAsking) },
        "help" => Command::Help(text_of(&parts.object)),
        "server" => Command::RunServer,
        "mcp" => Command::Mcp,
        other => return Err(ParseError::new(format!("\"{other}\" isn't a ctx verb."))),
    })
}

fn object_tab(parts: &Parts) -> TabRef {
    match &parts.object {
        Some((Value::Tab(t), _)) => t.clone(),
        _ => TabRef::This,
    }
}

// ---------------------------------------------------------------------------------------------
// Showing values
// ---------------------------------------------------------------------------------------------

pub fn show_tab(t: &TabRef) -> String {
    match t {
        TabRef::Number(n) => format!("tab {n}"),
        TabRef::Name(n) => quote(n),
        TabRef::This => "this tab".into(),
    }
}

pub fn show_window(w: &WindowRef) -> String {
    match w {
        WindowRef::Number(n) => format!("window {n}"),
        WindowRef::Name(n) => format!("window {}", quote(n)),
        WindowRef::New => "new window".to_string(),
    }
}

/// Quote text for a sentence, as a person would type it in zsh.
pub fn quote(s: &str) -> String {
    if s.contains('"') && !s.contains('\'') {
        return format!("'{s}'");
    }
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"").replace('$', "\\$").replace('`', "\\`");
    format!("\"{escaped}\"")
}

fn show_text_if_needed(s: &str) -> String {
    if s.chars().any(|c| c.is_whitespace() || "\"'$`\\;&|<>()".contains(c)) { quote(s) } else { s.to_string() }
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// `~/x` → `/Users/c/x`; relative paths are made absolute against the current folder.
pub fn resolve_path(p: &str, ctx: &Context) -> String {
    let expanded = if p == "~" {
        ctx.home.clone()
    } else if let Some(rest) = p.strip_prefix("~/") {
        format!("{}/{rest}", ctx.home.trim_end_matches('/'))
    } else {
        p.to_string()
    };
    let abs =
        if expanded.starts_with('/') { expanded } else { format!("{}/{expanded}", ctx.folder.trim_end_matches('/')) };
    normalise(&abs)
}

fn normalise(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    format!("/{}", parts.join("/"))
}

// ---------------------------------------------------------------------------------------------
// Repair: the deterministic half of assist
// ---------------------------------------------------------------------------------------------

/// Edit distance counting a swap of two neighbouring letters as one edit (optimal string
/// alignment), for typos in verbs and phrase words.
pub fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[a.len()][b.len()]
}

/// Known words within a small edit distance of `word`, closest first. Empty if `word` is itself
/// one of them.
fn close_words<'a>(word: &str, options: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
    let lower = word.to_lowercase();
    let max = if lower.chars().count() <= 4 { 1 } else { 2 };
    let mut found: Vec<(&str, usize)> = Vec::new();
    for o in options {
        let d = edit_distance(&lower, o);
        if d == 0 {
            return Vec::new();
        }
        if d <= max && !found.iter().any(|(f, _)| *f == o) {
            found.push((o, d));
        }
    }
    found.sort_by_key(|(_, d)| *d);
    found.into_iter().map(|(o, _)| o).collect()
}

/// Try each replacement for token `i`; the first that parses wins.
fn try_replacements(tokens: &[Token], i: usize, options: &[&str], ctx: &Context) -> Option<String> {
    for fixed in options {
        let mut ts = tokens.to_vec();
        ts[i] = Token { text: fixed.to_string() };
        if let Ok(s) = parse_tokens(&ts, ctx) {
            return Some(s.canonical);
        }
    }
    None
}

fn sentence_of(tokens: &[Token]) -> Vec<String> {
    tokens.iter().map(|t| t.text.clone()).collect()
}

/// Try simple, model-free fixes; return the canonical sentence if exactly one works.
pub fn repair(tokens: &[Token], ctx: &Context) -> Option<String> {
    let attempt = |ts: &[Token]| parse_tokens(ts, ctx).ok().map(|s| s.canonical);
    let first = tokens.first()?;

    // 1. A typo in the verb.
    if grammar::verb(&first.lower()).is_none() {
        for fixed in close_words(&first.text, VERBS.iter().filter(|v| !v.hidden).map(|v| v.name)) {
            let mut ts = tokens.to_vec();
            ts[0] = Token { text: fixed.to_string() };
            if let Some(s) = attempt(&ts).or_else(|| repair(&ts, ctx)) {
                return Some(s);
            }
        }
        return None;
    }
    let verb = grammar::verb(&first.lower())?;

    // 2. A typo in a noun (`list tab`), then in a phrase word.
    if let Object::Noun(nouns) = verb.object {
        let mut noun_words: Vec<&str> = nouns.iter().flat_map(|n| n.split(' ')).collect();
        noun_words.sort_unstable();
        noun_words.dedup();
        for (i, t) in tokens.iter().enumerate().skip(1) {
            if t.text.contains(' ') || noun_words.iter().any(|w| t.is_word(w)) {
                continue;
            }
            if let Some(s) = try_replacements(tokens, i, &close_words(&t.text, noun_words.iter().copied()), ctx) {
                return Some(s);
            }
        }
    }
    let mut words: Vec<&str> =
        phrases_of(verb).flat_map(|p| p.words.iter().copied().chain(p.after.iter().copied())).collect();
    if let Object::Noun(nouns) = verb.object {
        words.extend(nouns.iter().flat_map(|n| n.split(' ')));
    }
    words.extend([
        "last",
        "lines",
        "commands",
        "command",
        "everything",
        "tab",
        "window",
        "is",
        "idle",
        "shows",
        "quiet",
        "then",
        "press",
    ]);
    for (i, t) in tokens.iter().enumerate().skip(1) {
        if t.text.contains(' ') {
            continue;
        }
        if let Some(s) = try_replacements(tokens, i, &close_words(&t.text, words.iter().copied()), ctx) {
            return Some(s);
        }
    }

    // 3. Missing quotes: join runs of unquoted words into one value, and see if exactly one
    //    grouping parses.
    let mut found: Vec<String> = Vec::new();
    let n = tokens.len();
    for start in 1..n {
        for end in (start + 2)..=n {
            let run = &tokens[start..end];
            if run.iter().any(|t| grammar::is_sentence_word(&t.text) && !t.text.contains(' ')) {
                continue;
            }
            let joined = run.iter().map(|t| t.text.as_str()).collect::<Vec<_>>().join(" ");
            let mut ts: Vec<Token> = tokens[..start].to_vec();
            ts.push(Token { text: joined });
            ts.extend_from_slice(&tokens[end..]);
            if let Some(s) = attempt(&ts)
                && !found.contains(&s)
            {
                found.push(s);
            }
        }
    }
    // The object usually absorbs a run that has phrase words in it too: `ctx run cargo test in
    // auth` → the run `cargo test`.
    if found.len() == 1 {
        return found.pop();
    }
    if found.len() > 1 {
        // Prefer the grouping that leaves the most phrases intact: the shortest canonical
        // sentences tend to be the over-eager joins, so pick the one with the most words.
        found.sort_by_key(|s| std::cmp::Reverse(s.split(' ').count()));
        return found.into_iter().next();
    }

    // 4. A tab given without `in` (`read last 3 commands "Dev server"`).
    for i in 1..tokens.len() {
        let mut ts = tokens.to_vec();
        ts.insert(i, Token { text: "in".into() });
        if let Some(s) = attempt(&ts) {
            return Some(s);
        }
    }
    let _ = sentence_of(tokens);
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> Context {
        Context { folder: "/Users/c/code/site".into(), home: "/Users/c".into(), now_ms: 1_791_654_721_000 }
    }

    fn words(s: &str) -> Vec<String> {
        // A tiny shell-like splitter for tests: double quotes group words.
        let mut out = Vec::new();
        let mut cur = String::new();
        let mut in_q = false;
        let mut had = false;
        for c in s.chars() {
            match c {
                '"' => {
                    in_q = !in_q;
                    had = true;
                }
                ' ' if !in_q => {
                    if had || !cur.is_empty() {
                        out.push(std::mem::take(&mut cur));
                    }
                    had = false;
                }
                c => cur.push(c),
            }
        }
        if had || !cur.is_empty() {
            out.push(cur);
        }
        out
    }

    fn p(s: &str) -> Sentence {
        parse(&words(s), &ctx()).unwrap_or_else(|e| panic!("{s}: {e:?}"))
    }

    fn req(s: &str) -> Request {
        match p(s).command {
            Command::Server(r) => r,
            other => panic!("{s}: not a server request: {other:?}"),
        }
    }

    fn err(s: &str) -> ParseError {
        parse(&words(s), &ctx()).expect_err(s)
    }

    #[test]
    fn spec_examples_parse() {
        assert_eq!(
            req(r#"run in "Claude - auth refactor" "cargo test""#),
            Request::Run {
                tab: TabRef::Name("Claude - auth refactor".into()),
                command: "cargo test".into(),
                colors: false,
                max_lines: None,
                within_ms: None,
                wait: true
            }
        );
        assert_eq!(
            req(r#"open tab in window 2 at ~/code/site named "Dev server""#),
            Request::OpenTab {
                window: Some(WindowRef::Number(2)),
                folder: Some("/Users/c/code/site".into()),
                name: Some("Dev server".into())
            }
        );
        assert_eq!(
            req(r#"read last 3 commands in "Dev server""#),
            Request::Read {
                tab: TabRef::Name("Dev server".into()),
                what: ReadWhat::LastCommands(3),
                colors: false,
                max_lines: None
            }
        );
        assert_eq!(
            req(r#"wait until "Dev server" shows "Listening on""#),
            Request::Wait {
                tab: TabRef::Name("Dev server".into()),
                until: WaitCondition::Shows("Listening on".into()),
                within_ms: 300_000
            }
        );
        assert_eq!(
            req("close window 2 without asking"),
            Request::CloseWindow { window: WindowRef::Number(2), without_asking: true }
        );
        assert_eq!(p("list tabs as json").format, Format::Json);
        assert_eq!(req("list tabs as toon"), Request::ListTabs);
    }

    #[test]
    fn phrases_in_any_order_give_the_same_request_and_canonical_form() {
        let a = p(r#"run "cargo test" in "Dev server""#);
        let b = p(r#"run in "Dev server" "cargo test""#);
        assert_eq!(a, b);
        assert_eq!(a.canonical, r#"ctx run in "Dev server" "cargo test""#);
        let c = p(r#"find "deploy" since 30 days ago by claude at ~/code/site"#);
        assert_eq!(
            c.canonical,
            r#"ctx find "deploy" at /Users/c/code/site by claude since 30 days ago"#
                .replace("/Users/c/code/site", "~/code/site")
        );
    }

    #[test]
    fn fillers_are_ignored() {
        assert_eq!(req(r#"read the last command in "Dev server""#), req(r#"read last command in "Dev server""#));
        assert_eq!(req("open a tab"), req("open tab"));
        // …but not when they're the thing being searched for.
        assert_eq!(req(r#"find "a""#), Request::Find { text: "a".into(), filter: Filter::default(), limit: 20 });
    }

    #[test]
    fn read_forms() {
        assert_eq!(
            req(r#"read "Dev server""#),
            Request::Read {
                tab: TabRef::Name("Dev server".into()),
                what: ReadWhat::Screen,
                colors: false,
                max_lines: None
            }
        );
        assert_eq!(
            req("read everything in tab 4 with colors"),
            Request::Read { tab: TabRef::Number(4), what: ReadWhat::Everything, colors: true, max_lines: None }
        );
        assert_eq!(
            req("read command 14 in tab 212"),
            Request::Read { tab: TabRef::Number(212), what: ReadWhat::Command(14), colors: false, max_lines: None }
        );
        assert_eq!(
            req("read last 200 lines in this tab"),
            Request::Read { tab: TabRef::This, what: ReadWhat::LastLines(200), colors: false, max_lines: None }
        );
        assert_eq!(
            req("read last 1,000 lines"),
            Request::Read { tab: TabRef::This, what: ReadWhat::LastLines(1000), colors: false, max_lines: None }
        );
    }

    #[test]
    fn durations_and_times() {
        assert_eq!(
            req(r#"run "make" within 10 minutes"#),
            Request::Run {
                tab: TabRef::This,
                command: "make".into(),
                colors: false,
                max_lines: None,
                within_ms: Some(600_000),
                wait: true
            }
        );
        assert_eq!(
            req(r#"wait until "Dev server" is quiet for 2 seconds within a minute"#),
            Request::Wait {
                tab: TabRef::Name("Dev server".into()),
                until: WaitCondition::QuietFor(2000),
                within_ms: 60_000
            }
        );
        let r = req("list activity by claude since 1 hour ago");
        let Request::ListActivity { filter, .. } = r else { panic!() };
        assert_eq!(filter.since, Some(1_791_654_721 - 3600));
        assert_eq!(filter.by.as_deref(), Some("claude"));
    }

    #[test]
    fn type_and_press() {
        assert_eq!(
            req(r#"type "yes" into "Dev server" then press enter"#),
            Request::Type {
                tab: TabRef::Name("Dev server".into()),
                text: "yes".into(),
                then_press: Some("enter".into())
            }
        );
        assert_eq!(
            req(r#"press ctrl-c in "Dev server""#),
            Request::Press { tab: TabRef::Name("Dev server".into()), keys: vec!["ctrl-c".into()] }
        );
        assert_eq!(
            req("press escape then enter"),
            Request::Press { tab: TabRef::This, keys: vec!["escape".into(), "enter".into()] }
        );
    }

    #[test]
    fn single_word_names_need_no_quotes() {
        assert_eq!(req("close auth"), Request::CloseTab { tab: TabRef::Name("auth".into()), without_asking: false });
        assert_eq!(
            req("rename auth to Work"),
            Request::RenameTab { tab: TabRef::Name("auth".into()), to: "Work".into() }
        );
    }

    #[test]
    fn local_commands() {
        assert_eq!(
            p("install instructions for claude").command,
            Command::InstallInstructions { agent: "claude".into(), folder: None }
        );
        assert_eq!(
            p("install instructions for agents at ~/code/site").command,
            Command::InstallInstructions { agent: "agents".into(), folder: Some("/Users/c/code/site".into()) }
        );
        assert_eq!(
            p(r#"make launcher named "sysadmin" for ~/.local/bin/claude at ~/Agents/sys-admin"#).command,
            Command::MakeLauncher {
                name: "sysadmin".into(),
                program: "/Users/c/.local/bin/claude".into(),
                folder: "/Users/c/Agents/sys-admin".into(),
                any_arguments: false,
                signed_by: None
            }
        );
        assert_eq!(p(r#"remove launcher "sysadmin""#).command, Command::RemoveLauncher { name: "sysadmin".into() });
        assert_eq!(p(r#"rebuild launcher sysadmin"#).command, Command::RebuildLauncher { name: "sysadmin".into() });
        assert_eq!(p("restart server").command, Command::Restart { without_asking: false });
        assert_eq!(p("attach to tab 4").command, Command::Attach { tab: TabRef::Number(4) });
        assert_eq!(
            p(r#"ask "why?" about tab 212"#).command,
            Command::Ask { question: "why?".into(), about: AskAbout::Tab(TabRef::Number(212)), within_ms: None }
        );
        assert_eq!(p("").command, Command::Help(None));
        assert_eq!(p("help run").command, Command::Help(Some("run".into())));
    }

    #[test]
    fn missing_quotes_are_suggested() {
        let e = err("run cargo test in auth");
        assert_eq!(e.suggestion.as_deref(), Some(r#"ctx run in "auth" "cargo test""#));
        let e = err(r#"read last 3 commands "Dev server""#);
        assert_eq!(e.suggestion.as_deref(), Some(r#"ctx read last 3 commands in "Dev server""#));
    }

    #[test]
    fn typos_are_suggested() {
        assert_eq!(err("lsit tabs").suggestion.as_deref(), Some("ctx list tabs"));
        assert_eq!(
            err(r#"run "ls" withn 10 minutes"#).suggestion.as_deref(),
            Some(r#"ctx run "ls" within 10 minutes"#)
        );
        assert_eq!(err("list tab").suggestion.as_deref(), Some("ctx list tabs"));
    }

    #[test]
    fn errors_say_what_to_do() {
        assert!(err("open").message.contains("ctx open window"));
        assert!(err("list").message.contains("ctx list tabs"));
        assert!(err("as json").message.contains("isn't a ctx verb"));
        assert!(err(r#"press hyper-x in "x""#).message.contains("isn't a key"));
    }

    #[test]
    fn paths_resolve() {
        let c = ctx();
        assert_eq!(resolve_path("~/x", &c), "/Users/c/x");
        assert_eq!(resolve_path("../other", &c), "/Users/c/code/other");
        assert_eq!(resolve_path("/a/./b/", &c), "/a/b");
    }

    #[test]
    fn quoting() {
        assert_eq!(quote("cargo test"), "\"cargo test\"");
        assert_eq!(quote("echo $HOME"), "\"echo \\$HOME\"");
        assert_eq!(quote(r#"say "hi""#), r#"'say "hi"'"#);
    }

    use proptest::prelude::*;

    /// Every help example, as argv words.
    fn examples() -> Vec<Vec<String>> {
        crate::grammar::VERBS
            .iter()
            .flat_map(|v| v.examples.iter())
            .map(|(s, _)| crate::parse::tests::words(s.strip_prefix("ctx ").unwrap_or(s)))
            .collect()
    }

    #[test]
    fn every_help_example_parses_and_its_canonical_form_parses_the_same() {
        for ex in examples() {
            let a = parse(&ex, &ctx()).unwrap_or_else(|e| panic!("{ex:?}: {e:?}"));
            let canonical = words(a.canonical.strip_prefix("ctx ").unwrap());
            let b =
                parse(&canonical, &ctx()).unwrap_or_else(|e| panic!("canonical of {ex:?}: {:?}: {e:?}", a.canonical));
            assert_eq!(a.command, b.command, "{ex:?}");
            assert_eq!(a.canonical, b.canonical);
        }
    }

    proptest! {
        #[test]
        fn phrases_in_any_order_mean_the_same(seed in any::<u64>(), which in 0usize..64) {
            let all = examples();
            let ex = &all[which % all.len()];
            let (verb, object, mut phrases) = chunks(ex, &ctx()).unwrap();
            // A deterministic shuffle from the seed, with the object placed anywhere.
            let mut state = seed | 1;
            for i in (1..phrases.len()).rev() {
                state ^= state << 13; state ^= state >> 7; state ^= state << 17;
                phrases.swap(i, (state % (i as u64 + 1)) as usize);
            }
            let at = if phrases.is_empty() { 0 } else { (seed as usize) % (phrases.len() + 1) };
            let mut shuffled = vec![verb];
            for (i, p) in phrases.iter().enumerate() {
                if i == at { shuffled.extend(object.iter().cloned()); }
                shuffled.extend(p.iter().cloned());
            }
            if at >= phrases.len() { shuffled.extend(object.iter().cloned()); }
            let a = parse(ex, &ctx()).unwrap();
            let b = parse(&shuffled, &ctx());
            prop_assert!(b.is_ok(), "{:?} → {:?}: {:?}", ex, shuffled, b.err());
            let b = b.unwrap();
            prop_assert_eq!(a.command, b.command);
            prop_assert_eq!(a.canonical, b.canonical);
        }
    }
}
