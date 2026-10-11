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
    let output = std::thread::spawn(move || {
        let mut r = reader;
        loop {
            match read_message::<StreamOut>(&mut r) {
                Ok(Some(StreamOut::Bytes(b))) => {
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
    let _ = stdin_fd;
    let mut detached = false;
    loop {
        if output.is_finished() {
            break;
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
    print!("\x1b[?1049l\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?2004l\x1b[?25h\x1b[0m\r\n");
    let _ = std::io::stdout().flush();
    if detached {
        eprintln!("Detached from {label}; it keeps running. `ctx attach to tab {number}` comes back.");
    } else if let Ok(Some(msg)) = output.join() {
        eprintln!("{msg}");
    }
    0
}
