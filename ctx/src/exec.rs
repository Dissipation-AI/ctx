//! Running one sentence: parse it, do it, and collect what to print. The CLI prints the result;
//! `ctx mcp` returns it to the agent.

use crate::client::{self, Connection};
use crate::{ask, instructions, launchers};
use ctx_cli::parse::{self, Command, Format, Sentence};
use ctx_cli::render::{self, Style};
use ctx_cli::{grammar, help};
use ctx_protocol::*;
use std::io::{IsTerminal, Read, Write};

/// What a sentence printed, and its exit status.
#[derive(Debug, Default)]
pub struct Out {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
    /// The failure came from ctx itself (not from a command `run` ran).
    pub ctx_error: bool,
}

impl Out {
    pub fn line(&mut self, s: impl AsRef<str>) {
        self.stdout.push_str(s.as_ref());
        self.stdout.push('\n');
    }
    pub fn err(&mut self, s: impl AsRef<str>) {
        self.stderr.push_str(s.as_ref());
        self.stderr.push('\n');
    }
    pub fn code(mut self, c: i32) -> Out {
        self.code = c;
        self
    }
}

/// How a sentence is being run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Caller {
    /// The `ctx` command line; may ask a person to confirm, read standard input, attach.
    Cli,
    /// `ctx mcp`: no terminal, no prompts.
    Mcp,
}

pub fn context() -> parse::Context {
    parse::Context {
        folder: std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_else(|_| ctx_server::sys::home()),
        home: ctx_server::sys::home(),
        now_ms: ctx_protocol::time::now_ms(),
    }
}

pub fn style() -> Style {
    Style { tz: ctx_server::sys::tz_offset(), home: ctx_server::sys::home() }
}

/// Run a sentence given as words (argv without the program name).
pub fn run(args: &[String], caller: Caller) -> Out {
    // Asked by a scoped launcher inside a ctx tab; not a sentence.
    if caller == Caller::Cli && args.len() == 2 && args[0] == "launcher-check" {
        return launchers::check(&args[1]);
    }
    match parse::parse(args, &context()) {
        Ok(s) => execute(s, caller),
        Err(e) => not_parsed(args, e),
    }
}

/// The format asked for, even in a sentence that didn't parse.
fn format_in(args: &[String]) -> Format {
    for w in args.windows(2) {
        if w[0].eq_ignore_ascii_case("as") {
            match w[1].to_lowercase().as_str() {
                "json" => return Format::Json,
                "toon" => return Format::Toon,
                _ => {}
            }
        }
    }
    Format::Text
}

fn structured(v: &serde_json::Value, format: Format) -> String {
    if format == Format::Toon { toon::encode(v) } else { serde_json::to_string_pretty(v).unwrap_or_default() }
}

fn not_parsed(args: &[String], e: parse::ParseError) -> Out {
    let format = format_in(args);
    let mut out = Out::default();
    let (explanation, suggestion, by_model) = match &e.suggestion {
        Some(s) => (None, Some(s.clone()), false),
        None => match ask::assist(args, &e.message, &open_tab_names()) {
            Some((explanation, suggestion)) => (Some(explanation), suggestion, true),
            None => (None, None, false),
        },
    };
    match format {
        Format::Json | Format::Toon => {
            let v = with_schema(serde_json::json!({
                "error": "not_a_valid_command",
                "message": e.message,
                "explanation": explanation,
                "suggestion": suggestion,
            }));
            out.line(structured(&v, format));
        }
        Format::Text => {
            let mut msg = e.message.clone();
            match (&suggestion, by_model) {
                (Some(s), false) => {
                    if !msg.ends_with(':') {
                        msg.push_str(" Did you mean:");
                    }
                    out.err(format!("{msg}\n  {s}"));
                }
                (Some(s), true) => {
                    out.err(format!("That isn't a valid ctx command. Did you mean something like:\n  {s}\n(Suggested by your language model. Nothing was run.)"));
                    if let Some(x) = explanation.filter(|x| !x.is_empty()) {
                        out.err(x);
                    }
                }
                (None, true) => {
                    out.err(format!("That isn't a valid ctx command. {}", explanation.unwrap_or_default()));
                    out.err("(Explained by your language model. Nothing was run.) `ctx help` lists every sentence.");
                }
                (None, false) => {
                    out.err(msg);
                    out.err("`ctx help` lists every sentence.");
                }
            }
        }
    }
    out.ctx_error = true;
    out.code(1)
}

fn open_tab_names() -> Vec<String> {
    let Ok(mut c) = Connection::open_quiet() else { return Vec::new() };
    match c.request(&Envelope { sentence: "ctx list tabs".into(), request: Request::ListTabs }) {
        Ok(Response::Ok(Output::Tabs(list))) => list
            .windows
            .iter()
            .flat_map(|w| w.tabs.iter())
            .map(|t| match &t.name {
                Some(n) => format!("tab {} \"{n}\"", t.number),
                None => format!("tab {}", t.number),
            })
            .collect(),
        _ => Vec::new(),
    }
}

pub fn print_output(out: &mut Out, o: &Output, format: Format) {
    match format {
        Format::Json => out.line(render::render_json(o)),
        Format::Toon => out.line(render::render_toon(o)),
        Format::Text => {
            let p = render::render(o, &style());
            out.stdout.push_str(&p.out);
            for n in p.notes {
                out.err(format!("({n})"));
            }
        }
    }
}

pub fn print_error(out: &mut Out, e: &ErrorReply, format: Format) -> i32 {
    out.ctx_error = true;
    match format {
        Format::Json | Format::Toon => out.line(structured(&e.to_json(), format)),
        Format::Text => out.err(&e.message),
    }
    e.error.exit_status()
}

/// A message for a local command (instructions, launchers) in the asked-for format.
pub fn message(out: &mut Out, msg: &str, format: Format) {
    match format {
        Format::Text => out.line(msg),
        _ => out.line(structured(&with_schema(serde_json::json!({ "message": msg })), format)),
    }
}

fn no_server(msg: &str, format: Format) -> Out {
    let mut out = Out::default();
    let e = ErrorReply::new(ErrorKind::Failed, msg);
    print_error(&mut out, &e, format);
    out.code(2)
}

fn execute(s: Sentence, caller: Caller) -> Out {
    let format = s.format;
    let mut out = Out::default();
    match s.command {
        Command::RunServer => {
            if caller == Caller::Mcp {
                out.err("The server is started by launchd, not by an agent.");
                return out.code(1);
            }
            match ctx_server::server::run() {
                Ok(()) => out,
                Err(e) => {
                    out.err(format!("ctx server: {e}"));
                    out.code(1)
                }
            }
        }
        Command::Help(None) => {
            out.stdout.push_str(&help::page());
            out
        }
        Command::Help(Some(v)) => match help::verb(&v) {
            Some(t) => {
                out.stdout.push_str(&t);
                out
            }
            None => {
                let verbs: Vec<&str> = grammar::VERBS.iter().filter(|v| !v.hidden).map(|v| v.name).collect();
                out.err(format!("There's no verb \"{v}\". The verbs are: {}.", verbs.join(", ")));
                out.code(1)
            }
        },
        Command::Server(req) => server_request(s.canonical, req, format, caller),
        Command::Attach { tab } => {
            if caller == Caller::Mcp {
                out.err("attach connects a terminal to a tab; agents use run, type, press and read instead.");
                return out.code(1);
            }
            match Connection::open(ClientKind::Cli) {
                Ok(c) => out.code(crate::attach::attach(c, s.canonical, tab)),
                Err(client::NoServer(m)) => no_server(&m, format),
            }
        }
        Command::Restart { without_asking } => {
            server_request(s.canonical, Request::Restart { without_asking }, format, caller)
        }
        Command::InstallInstructions { agent, folder } => instructions::install(&agent, folder.as_deref(), format),
        Command::RemoveInstructions { agent, folder } => instructions::remove(&agent, folder.as_deref(), format),
        Command::MakeLauncher { name, program, folder, any_arguments, signed_by } => {
            launchers::make(&name, &program, &folder, any_arguments, signed_by.as_deref(), format)
        }
        Command::RebuildLauncher { name } => launchers::rebuild(&name, format),
        Command::RemoveLauncher { name } => launchers::remove(&name, format),
        Command::ListLaunchers => launchers::list(format),
        Command::Ask { question, about, within_ms } => ask::run(&question, about, within_ms, format),
        Command::Mcp => {
            if caller == Caller::Mcp {
                out.err("Already serving MCP.");
                return out.code(1);
            }
            out.code(ctx_mcp::serve(&|args: &[String]| {
                let o = run(args, Caller::Mcp);
                ctx_mcp::Result { stdout: o.stdout, stderr: o.stderr, code: o.code, ctx_error: o.ctx_error }
            }))
        }
    }
}

fn server_request(sentence: String, mut req: Request, format: Format, caller: Caller) -> Out {
    let mut out = Out::default();
    // `ctx run` with no command reads it from standard input (a heredoc).
    if let Request::Run { command, .. } = &mut req
        && command.is_empty()
    {
        if caller == Caller::Mcp || std::io::stdin().is_terminal() {
            out.err("Which command? Put it in quotes: ctx run in \"<tab>\" \"<command>\" (or pipe a script in).");
            return out.code(1);
        }
        let mut text = String::new();
        if std::io::stdin().read_to_string(&mut text).is_err() || text.trim().is_empty() {
            out.err("There was no command on standard input.");
            return out.code(1);
        }
        *command = text.trim_end_matches('\n').to_string();
    }
    let is_restart = matches!(req, Request::Restart { .. });
    let kind = if caller == Caller::Mcp { ClientKind::Mcp } else { ClientKind::Cli };
    let mut conn = match Connection::open(kind) {
        Ok(c) => c,
        Err(client::NoServer(m)) => return no_server(&m, format),
    };
    let mut env = Envelope { sentence, request: req };
    loop {
        let reply = match conn.request(&env) {
            Ok(r) => r,
            Err(e) => return no_server(&format!("Lost the connection to the ctx server: {e}"), format),
        };
        match reply {
            Response::Ok(o) => {
                print_output(&mut out, &o, format);
                if is_restart {
                    client::wait_for_new_server();
                }
                let code = exit_code(&o, &env.request);
                return out.code(code);
            }
            Response::Error(e)
                if e.error == ErrorKind::NeedsConfirmation
                    && caller == Caller::Cli
                    && format == Format::Text
                    && std::io::stdin().is_terminal()
                    && std::io::stderr().is_terminal() =>
            {
                let question = e.message.split(" `ctx").next().unwrap_or(&e.message).to_string();
                eprint!("{question} [y/N] ");
                let _ = std::io::stderr().flush();
                let mut answer = String::new();
                let _ = std::io::stdin().read_line(&mut answer);
                let a = answer.trim().to_lowercase();
                if a != "y" && a != "yes" {
                    out.err("Nothing was changed.");
                    return out.code(1);
                }
                env.request = confirm(env.request);
                env.sentence.push_str(" without asking");
                conn = match Connection::open(kind) {
                    Ok(c) => c,
                    Err(client::NoServer(m)) => return no_server(&m, format),
                };
            }
            Response::Error(e) => {
                let code = print_error(&mut out, &e, format);
                return out.code(code);
            }
        }
    }
}

/// The same request, confirmed.
fn confirm(r: Request) -> Request {
    match r {
        Request::CloseTab { tab, .. } => Request::CloseTab { tab, without_asking: true },
        Request::CloseWindow { window, .. } => Request::CloseWindow { window, without_asking: true },
        Request::Forget { tab, .. } => Request::Forget { tab, without_asking: true },
        Request::Restart { .. } => Request::Restart { without_asking: true },
        other => other,
    }
}

fn exit_code(o: &Output, req: &Request) -> i32 {
    match o {
        Output::Ran(r) => {
            let waited = matches!(req, Request::Run { wait: true, .. });
            if r.still_running { if waited { 3 } else { 0 } } else { r.exit_status.unwrap_or(1) }
        }
        _ => 0,
    }
}
