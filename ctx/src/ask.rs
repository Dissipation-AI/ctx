//! `ctx ask` and assist: the parts of ctx that use a language model.

use ctx_cli::parse::{AskAbout, Format};

pub fn run(_question: &str, _about: AskAbout, _within_ms: u64, _format: Format) -> i32 {
    eprintln!("ctx ask isn't built yet.");
    1
}

pub fn assist(_args: &[String], _error: &str, _tabs: &[String]) -> Option<(String, Option<String>)> {
    None
}
