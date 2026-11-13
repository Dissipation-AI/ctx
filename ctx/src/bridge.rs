//! `ctx bridge open|xdg-open|pbcopy|pbpaste …`: what those names run in an agent's tab. An
//! agent's account has no session on your screen, so its `open` and `pbcopy` can't reach yours;
//! these hand the link, file or text to the ctx server, which acts in your session.
//!
//! A file is read here, as the agent, and sent as its contents: the agent can only hand over
//! what it can read, and no path is left for it to swap before the server opens it.

use crate::client::{self, Connection};
use crate::exec::Out;
use ctx_protocol::{ClientKind, Envelope, Handed, Output, Request, Response};
use std::io::Read;

/// Files over this size aren't handed over.
const MAX_FILE: u64 = 64 * 1024 * 1024;

pub fn run(args: &[String]) -> Out {
    let mut out = Out::default();
    let Some((name, rest)) = args.split_first() else {
        out.err("usage: ctx bridge open|xdg-open|pbcopy|pbpaste [arguments…]");
        return out.code(2);
    };
    let what = match name.as_str() {
        "pbpaste" => {
            out.err("pbpaste: the person's clipboard isn't readable by agents. Ask them to paste what you need.");
            return out.code(1);
        }
        "pbcopy" => {
            let mut text = String::new();
            if std::io::stdin().read_to_string(&mut text).is_err() {
                out.err("pbcopy: the text isn't UTF-8.");
                return out.code(1);
            }
            vec![Handed::Clipboard(text)]
        }
        "open" | "xdg-open" => {
            let targets: Vec<&String> = rest.iter().collect();
            if targets.is_empty() || rest.iter().any(|a| a.starts_with('-')) {
                out.err(format!(
                    "{name}: in an agent's tab, {name} takes links and files only (no options): {name} <link or file>…"
                ));
                return out.code(2);
            }
            let mut v = Vec::new();
            for t in targets {
                if is_link(t) {
                    v.push(Handed::Link(t.clone()));
                    continue;
                }
                match read_file(t) {
                    Ok(h) => v.push(h),
                    Err(e) => {
                        out.err(format!("{name}: {e}"));
                        return out.code(1);
                    }
                }
            }
            v
        }
        other => {
            out.err(format!("ctx bridge: there's no bridge for \"{other}\"."));
            return out.code(2);
        }
    };
    let mut conn = match Connection::open(ClientKind::Cli) {
        Ok(c) => c,
        Err(client::NoServer(m)) => {
            out.err(m);
            return out.code(2);
        }
    };
    for h in what {
        let req = Request::Hand { what: h };
        match conn.request(&Envelope { sentence: format!("{name} (an agent's)"), request: req }) {
            Ok(Response::Ok(Output::Done(_))) => {}
            Ok(Response::Error(e)) => {
                out.err(format!("{name}: {}", e.message));
                return out.code(1);
            }
            Ok(_) => {}
            Err(e) => {
                out.err(format!("{name}: lost the connection to the ctx server: {e}"));
                return out.code(2);
            }
        }
    }
    out
}

fn is_link(s: &str) -> bool {
    let l = s.to_ascii_lowercase();
    l.starts_with("http://") || l.starts_with("https://") || l.starts_with("mailto:")
}

fn read_file(path: &str) -> Result<Handed, String> {
    let f = std::fs::File::open(path).map_err(|e| format!("{path}: {e}"))?;
    let m = f.metadata().map_err(|e| format!("{path}: {e}"))?;
    if !m.is_file() {
        return Err(format!("{path} isn't a file; only files and links can be opened in the person's session."));
    }
    if m.len() > MAX_FILE {
        return Err(format!("{path} is over 64 MB; put it in a shared folder instead."));
    }
    let mut bytes = Vec::with_capacity(m.len() as usize);
    f.take(MAX_FILE).read_to_end(&mut bytes).map_err(|e| format!("{path}: {e}"))?;
    let name = std::path::Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(Handed::File { name, bytes })
}
