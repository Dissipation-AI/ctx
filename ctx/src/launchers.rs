//! `ctx make launcher` and friends.

use ctx_cli::parse::Format;

pub fn make(_name: &str, _program: &str, _folder: &str, _any: bool, _signed_by: Option<&str>, _format: Format) -> i32 {
    eprintln!("not built yet");
    1
}
pub fn rebuild(_name: &str, _format: Format) -> i32 {
    eprintln!("not built yet");
    1
}
pub fn remove(_name: &str, _format: Format) -> i32 {
    eprintln!("not built yet");
    1
}
pub fn list(_format: Format) -> i32 {
    eprintln!("not built yet");
    1
}
