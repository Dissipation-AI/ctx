//! Golden screens: recorded sessions of real programs (vim, less, top, a Python REPL, an
//! Ink-style redrawing TUI) replayed through ctx's scanner and emulator; the screen must match
//! the reviewed `.txt`. Record new ones with `tests/screens/record.py`; set
//! `CTX_UPDATE_GOLDEN=1` to rewrite the expected screens after an intended change.

use ctx_server::scan::{Piece, Scanner};
use ctx_server::terminal::{Ghostty, Terminal};

fn screen_of(bytes: &[u8]) -> String {
    let mut t = Ghostty::new(80, 24);
    let mut scan = Scanner::default();
    for piece in scan.feed(bytes) {
        if let Piece::Data(d) = piece {
            t.write(&d);
        }
    }
    let mut rows = t.screen(false);
    while rows.last().is_some_and(|r| r.trim().is_empty()) {
        rows.pop();
    }
    rows.join("\n") + "\n"
}

#[test]
fn recorded_programs_render_like_a_terminal() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/screens");
    let mut checked = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("bin") {
            continue;
        }
        let got = screen_of(&std::fs::read(&path).unwrap());
        let expected = path.with_extension("txt");
        if std::env::var_os("CTX_UPDATE_GOLDEN").is_some() || !expected.exists() {
            std::fs::write(&expected, &got).unwrap();
        }
        assert_eq!(got, std::fs::read_to_string(&expected).unwrap(), "{}", path.display());
        checked += 1;
    }
    assert!(checked >= 5, "only {checked} recordings");
}
