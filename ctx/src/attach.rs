//! `ctx attach to tab N`: connect this terminal to a tab. ctrl-] detaches.

use crate::client::Connection;
use ctx_protocol::*;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;

const DETACH: u8 = 0x1d; // ctrl-]

fn term_size() -> (u16, u16) {
    let mut ws = libc_winsize();
    // SAFETY: TIOCGWINSZ fills a winsize.
    unsafe { libc::ioctl(1, libc::TIOCGWINSZ, &mut ws) };
    if ws.ws_col == 0 { (80, 24) } else { (ws.ws_col, ws.ws_row) }
}

fn libc_winsize() -> libc::winsize {
    libc::winsize { ws_row: 0, ws_col: 0, ws_xpixel: 0, ws_ypixel: 0 }
}

struct RawMode(libc::termios);

impl RawMode {
    fn enter() -> Option<RawMode> {
        // SAFETY: tcgetattr/tcsetattr on stdin with a valid termios.
        unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(0, &mut t) != 0 {
                return None;
            }
            let saved = t;
            libc::cfmakeraw(&mut t);
            libc::tcsetattr(0, libc::TCSANOW, &t);
            Some(RawMode(saved))
        }
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        // SAFETY: restoring the saved settings.
        unsafe { libc::tcsetattr(0, libc::TCSANOW, &self.0) };
    }
}

pub fn attach(mut conn: Connection, sentence: String, tab: TabRef) -> i32 {
    let (columns, rows) = term_size();
    let reply = conn.request(&Envelope { sentence, request: Request::Attach { tab, columns, rows } });
    let (number, name) = match reply {
        Ok(Response::Ok(Output::Attached(a))) => (a.tab, a.tab_name),
        Ok(Response::Error(e)) => {
            eprintln!("{}", e.message);
            return 1;
        }
        Ok(_) => return 1,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let label = match name {
        Some(n) => format!("tab {number} \"{n}\""),
        None => format!("tab {number}"),
    };
    let Some(raw) = RawMode::enter() else {
        eprintln!("ctx attach needs a terminal.");
        return 1;
    };
    let mut out = std::io::stdout();
    let reader = conn.stream.try_clone().expect("clone socket");
    let writer_stream = conn.stream.try_clone().expect("clone socket");
    // Output: server → this terminal.
    // Whether the tab's program has this terminal on the alternate screen, so leaving restores it
    // only then (leaving when it was never entered moves the cursor back over the output).
    let alt = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let alt_seen = alt.clone();
    let output = std::thread::spawn(move || {
        let mut r = reader;
        loop {
            match read_message::<StreamOut>(&mut r) {
                Ok(Some(StreamOut::Bytes(b))) => {
                    if let Some(on) = alt_screen_switch(&b) {
                        alt_seen.store(on, std::sync::atomic::Ordering::Relaxed);
                    }
                    let _ = out.write_all(&b);
                    let _ = out.flush();
                }
                Ok(Some(StreamOut::Ended { message })) => return Some(message),
                Ok(Some(_)) => {}
                _ => return None,
            }
        }
    });
    // Resizes.
    let size_stream = conn.stream.try_clone().expect("clone socket");
    std::thread::spawn(move || {
        let mut last = term_size();
        let mut s = size_stream;
        loop {
            std::thread::sleep(std::time::Duration::from_millis(250));
            let now = term_size();
            if now != last {
                last = now;
                if write_message(&mut s, &StreamIn::Resize { columns: now.0, rows: now.1 }).is_err() {
                    break;
                }
            }
        }
    });
    // Input: this terminal → server.
    let mut w = writer_stream;
    let mut stdin = std::io::stdin();
    let mut buf = [0u8; 4096];
    let stdin_fd = stdin.as_raw_fd();
    let mut detached = false;
    loop {
        if output.is_finished() {
            break;
        }
        // Wait for a key without blocking forever, so a tab that ends is noticed at once.
        let mut pfd = libc::pollfd { fd: stdin_fd, events: libc::POLLIN, revents: 0 };
        // SAFETY: one valid pollfd.
        if unsafe { libc::poll(&mut pfd, 1, 100) } == 0 {
            continue;
        }
        let n = match stdin.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        if let Some(pos) = buf[..n].iter().position(|b| *b == DETACH) {
            if pos > 0 {
                let _ = write_message(&mut w, &StreamIn::Bytes(buf[..pos].to_vec()));
            }
            let _ = write_message(&mut w, &StreamIn::Detach);
            detached = true;
            break;
        }
        if write_message(&mut w, &StreamIn::Bytes(buf[..n].to_vec())).is_err() {
            break;
        }
    }
    drop(raw);
    // Leave the screen tidy: reset modes the tab's program may have turned on.
    if alt.load(std::sync::atomic::Ordering::Relaxed) {
        print!("\x1b[?1049l");
    }
    print!("\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?2004l\x1b[?25h\x1b[0m\r\n");
    let _ = std::io::stdout().flush();
    if detached {
        eprintln!("Detached from {label}; it keeps running. `ctx attach to tab {number}` comes back.");
    } else if let Ok(Some(msg)) = output.join() {
        eprintln!("{msg}");
    }
    0
}

/// The last alternate-screen switch in `bytes`: `Some(true)` entered, `Some(false)` left.
fn alt_screen_switch(bytes: &[u8]) -> Option<bool> {
    let mut last = None;
    for (i, w) in bytes.windows(3).enumerate() {
        if w != b"\x1b[?" {
            continue;
        }
        let rest = &bytes[i + 3..];
        for mode in [&b"1049"[..], b"1047", b"47"] {
            if rest.starts_with(mode) {
                match rest.get(mode.len()) {
                    Some(b'h') => last = Some(true),
                    Some(b'l') => last = Some(false),
                    _ => {}
                }
            }
        }
    }
    last
}

#[cfg(test)]
mod tests {
    #[test]
    fn alternate_screen_switches_are_seen() {
        assert_eq!(super::alt_screen_switch(b"hi"), None);
        assert_eq!(super::alt_screen_switch(b"\x1b[?1049hvim"), Some(true));
        assert_eq!(super::alt_screen_switch(b"\x1b[?1049h..\x1b[?1049l"), Some(false));
        assert_eq!(super::alt_screen_switch(b"\x1b[?47h"), Some(true));
    }
}
