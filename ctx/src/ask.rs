//! `ctx ask` and assist: the parts of ctx that use a language model. `ask` uses its own model
//! (the `[ask]` section of the config); assist asks `bro`.

use crate::client::{self, Connection};
use crate::exec::{Out, message};
use ctx_ask::model::Endpoint;
use ctx_ask::{Answer, Settings, TabRecord};
use ctx_cli::parse::{self, AskAbout, Format};
use ctx_protocol::config::Config;
use ctx_protocol::*;
use std::time::{Duration, Instant};

fn config() -> Config {
    Config::load(&client::paths().config).unwrap_or_default()
}

fn failed(out: &mut Out, msg: &str) -> i32 {
    out.err(msg);
    out.ctx_error = true;
    1
}

/// "1 minute", "90 seconds" → milliseconds.
fn time_limit(s: &str) -> Option<u64> {
    let w: Vec<&str> = s.split_whitespace().collect();
    let n: u64 = match w.first()? {
        &"a" | &"an" | &"one" => 1,
        x => x.parse().ok()?,
    };
    let unit = w.get(1)?.trim_end_matches('s');
    Some(
        n * match unit {
            "second" => 1000,
            "minute" => 60_000,
            "hour" => 3_600_000,
            _ => return None,
        },
    )
}

fn records(conn: &mut Connection, tab: TabRef) -> Result<TabRecord, String> {
    match conn.request(&Envelope {
        sentence: format!("ctx ask about {}", parse::show_tab(&tab)),
        request: Request::Records { tab },
    }) {
        Ok(Response::Ok(Output::Records(c))) => Ok(TabRecord {
            tab: c.tab,
            name: c.tab_name,
            commands: c
                .commands
                .into_iter()
                .map(|r| ctx_ask::Command {
                    number: r.number,
                    text: r.command,
                    location: r.location,
                    exit_status: r.exit_status,
                    output: r.output,
                })
                .collect(),
        }),
        Ok(Response::Error(e)) => Err(e.message),
        Ok(_) => Err("unexpected answer from the server".into()),
        Err(e) => Err(e.to_string()),
    }
}

pub fn run(question: &str, about: AskAbout, within_ms: Option<u64>, format: Format) -> Out {
    let mut out = Out::default();
    let cfg = config();
    let ask = &cfg.ask;
    if ask.endpoint.trim().is_empty() || ask.model.trim().is_empty() {
        let code = failed(
            &mut out,
            &format!(
                "ctx ask needs a language model of its own (it doesn't use bro's). Add this to {}:\n\n[ask]\nendpoint = \"http://localhost:8080/v1\"   # any OpenAI-compatible endpoint\nmodel = \"<model name>\"\napi_key_env = \"\"                         # for a cloud endpoint: the name of the variable holding its key\n\nA larger model with a big context reads long tabs faster and better.",
                client::paths().config.display()
            ),
        );
        return out.code(code);
    }
    let api_key =
        if ask.api_key_env.is_empty() { None } else { std::env::var(&ask.api_key_env).ok().filter(|k| !k.is_empty()) };
    if !ask.api_key_env.is_empty() && api_key.is_none() {
        let code = failed(&mut out, &format!("ctx ask's key is read from ${}, which isn't set here.", ask.api_key_env));
        return out.code(code);
    }
    let endpoint = Endpoint { url: ask.endpoint.clone(), model: ask.model.clone(), api_key };
    // Tell the person once that tab contents leave the machine.
    if !endpoint.is_local() {
        let flag = client::paths().data.join(format!("ask-warned-{}", endpoint.url.replace(['/', ':'], "_")));
        if !flag.exists() {
            out.err(format!(
                "Note: ctx ask sends tab contents (after redaction) to {}. This is said once.",
                endpoint.url
            ));
            let _ = std::fs::create_dir_all(client::paths().data);
            let _ = std::fs::write(&flag, "");
        }
    }
    let mut conn = match Connection::open(ClientKind::Cli) {
        Ok(c) => c,
        Err(client::NoServer(m)) => {
            out.err(m);
            out.ctx_error = true;
            return out.code(2);
        }
    };
    let tabs: Vec<TabRecord> = match about {
        AskAbout::Tab(t) => match records(&mut conn, t) {
            Ok(r) => vec![r],
            Err(e) => {
                let code = failed(&mut out, &e);
                return out.code(code);
            }
        },
        AskAbout::AllTabs(filter) => {
            let numbers = match conn.request(&Envelope {
                sentence: "ctx ask about all tabs".into(),
                request: Request::MatchingTabs { filter, limit: 20 },
            }) {
                Ok(Response::Ok(Output::TabNumbers(n))) => n,
                Ok(Response::Error(e)) => {
                    let code = failed(&mut out, &e.message);
                    return out.code(code);
                }
                _ => Vec::new(),
            };
            let mut v = Vec::new();
            for n in numbers {
                if let Ok(r) = records(&mut conn, TabRef::Number(n)) {
                    v.push(r);
                }
            }
            v
        }
    };
    if tabs.iter().all(|t| t.commands.is_empty()) {
        let code = failed(&mut out, "There are no commands to read in that tab (or tabs) yet.");
        return out.code(code);
    }
    let cache_dir = client::paths().ask_cache();
    let key = ctx_ask::cache_key(&tabs, question);
    let cached: Option<Answer> = std::fs::read(cache_dir.join(&key)).ok().and_then(|b| serde_json::from_slice(&b).ok());
    let answer = match cached {
        Some(mut a) => {
            a.cached = true;
            a
        }
        None => {
            let limit = within_ms.or_else(|| time_limit(&ask.time_limit)).unwrap_or(60_000);
            let settings = Settings {
                max_context_tokens: ask.max_context_tokens.max(2000),
                max_model_calls: ask.max_model_calls.max(1),
                deadline: Instant::now() + Duration::from_millis(limit),
            };
            match ctx_ask::ask(question, &tabs, &endpoint, &settings) {
                Ok(a) => {
                    let _ = std::fs::create_dir_all(&cache_dir);
                    let _ = std::fs::write(cache_dir.join(&key), serde_json::to_vec(&a).unwrap_or_default());
                    a
                }
                Err(e) => {
                    let code = failed(&mut out, &format!("ctx ask couldn't answer: {e}"));
                    return out.code(code);
                }
            }
        }
    };
    match format {
        Format::Text => out.stdout.push_str(&ctx_ask::render(&answer, tabs.len() > 1)),
        _ => {
            let v = with_schema(serde_json::to_value(&answer).unwrap_or_default());
            message(&mut out, "", Format::Text);
            out.stdout.clear();
            out.line(if format == Format::Toon {
                toon::encode(&v)
            } else {
                serde_json::to_string_pretty(&v).unwrap_or_default()
            });
        }
    }
    out
}

/// Find a program on PATH.
fn which(program: &str) -> Option<String> {
    if program.contains('/') {
        return std::path::Path::new(program).exists().then(|| program.to_string());
    }
    std::env::var("PATH").ok()?.split(':').map(|d| format!("{d}/{program}")).find(|p| std::path::Path::new(p).is_file())
}

/// Ask bro for a corrected sentence. Returns (explanation, suggestion) if it said anything useful.
pub fn assist(args: &[String], error: &str, tabs: &[String]) -> Option<(String, Option<String>)> {
    if std::env::var("CTX_ASSIST")
        .is_ok_and(|v| v.eq_ignore_ascii_case("off") || v == "0" || v.eq_ignore_ascii_case("false"))
    {
        return None;
    }
    let cfg = config();
    if !cfg.assist {
        return None;
    }
    let program = which(&cfg.assist_command)?;
    let sentence = args.join(" ");
    let grammar = ctx_cli::help::assist_summary();
    let cx = crate::exec::context();
    let mut err = error.to_string();
    let mut explanation = String::new();
    let start = Instant::now();
    for _ in 0..2 {
        if start.elapsed() > ctx_ask::assist::TIME_LIMIT {
            break;
        }
        let reply = ctx_ask::assist::ask_bro(&program, &ctx_ask::assist::request(&sentence, &err, &grammar, tabs))?;
        explanation = reply.explanation.clone();
        let Some(cmd) = reply.command else { break };
        let words = ctx_ask::assist::words(&cmd);
        if words.first().map(String::as_str) != Some("ctx") {
            break;
        }
        match parse::parse(&words[1..], &cx) {
            Ok(s) => return Some((explanation, Some(s.canonical))),
            Err(e) => err = e.message,
        }
    }
    if explanation.is_empty() { None } else { Some((explanation, None)) }
}
