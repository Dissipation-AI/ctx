//! `ctx`: the CLI (`ctx <sentence>`) and the server (`ctx server`) in one binary.

mod ask;
mod attach;
mod client;
mod exec;
mod hook;
mod instructions;
mod integrations;
mod launchers;
mod shim;

use std::io::Write;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("server") {
        ctx_server::sys::default_sigpipe();
    }
    let out = exec::run(&args, exec::Caller::Cli);
    let mut stdout = std::io::stdout();
    let _ = stdout.write_all(out.stdout.as_bytes());
    let _ = stdout.flush();
    if !out.stderr.is_empty() {
        let _ = std::io::stderr().write_all(out.stderr.as_bytes());
    }
    std::process::exit(out.code);
}
