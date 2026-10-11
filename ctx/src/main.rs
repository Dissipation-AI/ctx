//! `ctx`: the CLI (`ctx <sentence>`) and the server (`ctx server`) in one binary.

mod ask;
mod attach;
mod client;
mod instructions;
mod launchers;

use client::Connection;
use ctx_cli::parse::{self, Command, Format, Sentence};
use ctx_cli::render::{self, Style};
use ctx_cli::{grammar, help};
use ctx_protocol::*;
use std::io::{IsTerminal, Read, Write};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = run(args);
    let _ = std::io::stdout().flush();
    std::process::exit(code);
}

fn context() -> parse::Context {
    parse::Context {
        folder: std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_else(|_| ctx_server::sys::home()),
        home: ctx_server::sys::home(),
        now_ms: ctx_protocol::time::now_ms(),
    }
}

fn style() -> Style {
    Style { tz: ctx_server::sys::tz_offset(), home: ctx_server::sys::home() }
}

fn run(args: Vec<String>) -> i32 {
    let cx = context();
    match parse::parse(&args, &cx) {
        Ok(s) => execute(s),
        Err(e) => not_parsed(&args, e),
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

fn not_parsed(args: &[String], e: parse::ParseError) -> i32 {
    let format = format_in(args);
    let (explanation, suggestion, by_model) = match &e.suggestion {
        Some(s) => (None, Some(s.clone()), false),
        None => match assist(args, &e.message) {
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
            if format == Format::Json {
                println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            } else {
                println!("{}", toon::encode(&v));
            }
        }
        Format::Text => {
            let mut msg = e.message.clone();
            match (&suggestion, by_model) {
                (Some(s), false) => {
                    if !msg.ends_with(':') {
                        msg.push_str(" Did you mean:");
                    }
                    eprintln!("{msg}\n  {s}");
                }
                (Some(s), true) => {
                    eprintln!(
                        "That isn't a valid ctx command. Did you mean something like:\n  {s}\n(Suggested by your language model. Nothing was run.)"
                    );
                    if let Some(x) = explanation.filter(|x| !x.is_empty()) {
                        eprintln!("{x}");
                    }
                }
                (None, true) => {
                    eprintln!("That isn't a valid ctx command. {}", explanation.unwrap_or_default());
                    eprintln!("(Explained by your language model. Nothing was run.) `ctx help` lists every sentence.");
                }
                (None, false) => {
                    eprintln!("{msg}");
                    eprintln!("`ctx help` lists every sentence.");
                }
            }
        }
    }
    1
}

/// The model half of assist: ask `bro` for a corrected sentence. Never runs anything.
fn assist(args: &[String], error: &str) -> Option<(String, Option<String>)> {
    ask::assist(args, error, &open_tab_names())
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

fn print_output(o: &Output, format: Format) {
    match format {
        Format::Json => println!("{}", render::render_json(o)),
        Format::Toon => println!("{}", render::render_toon(o)),
        Format::Text => {
            let p = render::render(o, &style());
            print!("{}", p.out);
            let _ = std::io::stdout().flush();
            for n in p.notes {
                eprintln!("({n})");
            }
        }
    }
}

fn print_error(e: &ErrorReply, format: Format) -> i32 {
    match format {
        Format::Json => println!("{}", serde_json::to_string_pretty(&e.to_json()).unwrap_or_default()),
        Format::Toon => println!("{}", toon::encode(&e.to_json())),
        Format::Text => eprintln!("{}", e.message),
    }
    e.error.exit_status()
}

fn no_server(msg: &str, format: Format) -> i32 {
    let e = ErrorReply::new(ErrorKind::Failed, msg);
    match format {
        Format::Text => eprintln!("{msg}"),
        _ => {
            print_error(&e, format);
        }
    }
    2
}

fn execute(s: Sentence) -> i32 {
    let format = s.format;
    match s.command {
        Command::RunServer => match ctx_server::server::run() {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("ctx server: {e}");
                1
            }
        },
        Command::Help(None) => {
            print!("{}", help::page());
            0
        }
        Command::Help(Some(v)) => match help::verb(&v) {
            Some(t) => {
                print!("{t}");
                0
            }
            None => {
                let verbs: Vec<&str> = grammar::VERBS.iter().filter(|v| !v.hidden).map(|v| v.name).collect();
                eprintln!("There's no verb \"{v}\". The verbs are: {}.", verbs.join(", "));
                1
            }
        },
        Command::Server(req) => server_request(s.canonical, req, format),
        Command::Attach { tab } => match Connection::open(ClientKind::Cli) {
            Ok(c) => attach::attach(c, s.canonical, tab),
            Err(client::NoServer(m)) => no_server(&m, format),
        },
        Command::Restart { without_asking } => server_request(s.canonical, Request::Restart { without_asking }, format),
        Command::InstallInstructions { agent, folder } => instructions::install(&agent, folder.as_deref(), format),
        Command::RemoveInstructions { agent, folder } => instructions::remove(&agent, folder.as_deref(), format),
        Command::MakeLauncher { name, program, folder, any_arguments, signed_by } => {
            launchers::make(&name, &program, &folder, any_arguments, signed_by.as_deref(), format)
        }
        Command::RebuildLauncher { name } => launchers::rebuild(&name, format),
        Command::RemoveLauncher { name } => launchers::remove(&name, format),
        Command::ListLaunchers => launchers::list(format),
        Command::Ask { question, about, within_ms } => ask::run(&question, about, within_ms, format),
        Command::Mcp => ctx_mcp::serve(),
    }
}

fn server_request(sentence: String, mut req: Request, format: Format) -> i32 {
    // `ctx run` with no command reads it from standard input (a heredoc).
    if let Request::Run { command, .. } = &mut req
        && command.is_empty()
    {
        if std::io::stdin().is_terminal() {
            eprintln!("Which command? Put it in quotes: ctx run in \"<tab>\" \"<command>\" (or pipe a script in).");
            return 1;
        }
        let mut text = String::new();
        if std::io::stdin().read_to_string(&mut text).is_err() || text.trim().is_empty() {
            eprintln!("There was no command on standard input.");
            return 1;
        }
        *command = text.trim_end_matches('\n').to_string();
    }
    let is_restart = matches!(req, Request::Restart { .. });
    let mut conn = match Connection::open(ClientKind::Cli) {
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
                print_output(&o, format);
                if is_restart {
                    client::wait_for_new_server();
                }
                return exit_code(&o, &env.request);
            }
            Response::Error(e)
                if e.error == ErrorKind::NeedsConfirmation
                    && format == Format::Text
                    && std::io::stdin().is_terminal()
                    && std::io::stderr().is_terminal() =>
            {
                let question = e.message.split(" `ctx").next().unwrap_or(&e.message).to_string();
                eprint!("{question} [y/N] ");
                let _ = std::io::stderr().flush();
                let mut answer = String::new();
                let _ = std::io::stdin().read_line(&mut answer);
                if !answer.trim().eq_ignore_ascii_case("y") && !answer.trim().eq_ignore_ascii_case("yes") {
                    eprintln!("Nothing was changed.");
                    return 1;
                }
                env.request = confirm(env.request);
                env.sentence.push_str(" without asking");
                conn = match Connection::open(ClientKind::Cli) {
                    Ok(c) => c,
                    Err(client::NoServer(m)) => return no_server(&m, format),
                };
            }
            Response::Error(e) => return print_error(&e, format),
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
