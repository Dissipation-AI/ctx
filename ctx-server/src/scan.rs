//! Watches a tab's output stream for the sequences ctx acts on, at the exact place they occur:
//! shell-integration marks (OSC 133), the current folder (OSC 7), ctx's private reports (OSC
//! 6973), switches to and from the full-screen buffer, and "erase scrollback" (`ESC[3J`).
//!
//! Everything else passes through untouched, in order, to the terminal emulator.

/// Private OSC number for ctx's shell integration (command text, environment, typing state).
pub const CTX_OSC: &str = "6973";

/// Sequences longer than this that ctx doesn't care about are streamed through rather than held.
const MAX_HELD: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mark {
    /// OSC 133;A — a prompt starts.
    PromptStart,
    /// OSC 133;B — the prompt ended; input starts.
    InputStart,
    /// OSC 133;C — the command's output starts.
    OutputStart,
    /// OSC 133;D[;status] — the command finished.
    CommandEnd(Option<i32>),
    /// OSC 7 — the shell's current folder.
    Folder { host: String, path: String },
    /// OSC 6973 — a private report from ctx's shell integration (not shown).
    Ctx(String),
    /// `ESC[3J`, erase scrollback. Dropped unless programs are allowed to clear scrollback.
    ClearScrollback,
    /// About to leave the full-screen (alternate) buffer: the screen is still the program's.
    LeavingFullScreen,
    /// Entered the full-screen buffer.
    EnteredFullScreen,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece {
    /// Bytes for the emulator.
    Data(Vec<u8>),
    Mark(Mark),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    Esc,
    Csi,
    Osc,
    OscEsc,
    /// DCS, APC, PM, SOS: a string ended by ST.
    Str,
    StrEsc,
    /// A long, uninteresting sequence being streamed through until its terminator.
    PassOsc,
    PassOscEsc,
    PassStr,
    PassStrEsc,
}

#[derive(Debug)]
pub struct Scanner {
    state: State,
    held: Vec<u8>,
    /// Let `ESC[3J` through to the emulator.
    pub allow_clear_scrollback: bool,
}

impl Default for Scanner {
    fn default() -> Self {
        Scanner { state: State::Ground, held: Vec::new(), allow_clear_scrollback: false }
    }
}

impl Scanner {
    /// Split `input` into emulator data and marks, in order.
    pub fn feed(&mut self, input: &[u8]) -> Vec<Piece> {
        let mut out: Vec<Piece> = Vec::new();
        let mut data: Vec<u8> = Vec::with_capacity(input.len());
        let flush = |data: &mut Vec<u8>, out: &mut Vec<Piece>| {
            if !data.is_empty() {
                out.push(Piece::Data(std::mem::take(data)));
            }
        };
        let mut i = 0;
        while i < input.len() {
            let b = input[i];
            i += 1;
            match self.state {
                State::Ground => {
                    if b == 0x1b {
                        self.state = State::Esc;
                        self.held.clear();
                        self.held.push(b);
                    } else {
                        data.push(b);
                    }
                }
                State::Esc => {
                    self.held.push(b);
                    match b {
                        b'[' => self.state = State::Csi,
                        b']' => self.state = State::Osc,
                        b'P' | b'_' | b'^' | b'X' => self.state = State::Str,
                        0x1b => {
                            // ESC ESC: pass the first, start again.
                            data.push(0x1b);
                            self.held.clear();
                            self.held.push(0x1b);
                        }
                        _ => {
                            // Two-byte (or intermediate) sequences: pass through. Intermediates
                            // (0x20..0x2f) take one more byte.
                            if (0x20..=0x2f).contains(&b) {
                                // Stay in Esc to collect the final byte.
                            } else {
                                data.extend_from_slice(&self.held);
                                self.held.clear();
                                self.state = State::Ground;
                            }
                        }
                    }
                }
                State::Csi => {
                    self.held.push(b);
                    if (0x40..=0x7e).contains(&b) {
                        self.state = State::Ground;
                        let seq = std::mem::take(&mut self.held);
                        self.finish_csi(seq, &mut data, &mut out);
                    } else if self.held.len() > 256 {
                        data.extend_from_slice(&self.held);
                        self.held.clear();
                        self.state = State::Ground;
                    }
                }
                State::Osc => {
                    if b == 0x07 {
                        self.held.push(b);
                        self.state = State::Ground;
                        let seq = std::mem::take(&mut self.held);
                        self.finish_osc(seq, 1, &mut data, &mut out, &flush);
                    } else if b == 0x1b {
                        self.held.push(b);
                        self.state = State::OscEsc;
                    } else {
                        self.held.push(b);
                        if self.held.len() > MAX_HELD && !self.osc_is_interesting() {
                            data.extend_from_slice(&self.held);
                            self.held.clear();
                            self.state = State::PassOsc;
                        }
                    }
                }
                State::OscEsc => {
                    self.held.push(b);
                    if b == b'\\' {
                        self.state = State::Ground;
                        let seq = std::mem::take(&mut self.held);
                        self.finish_osc(seq, 2, &mut data, &mut out, &flush);
                    } else {
                        // ESC inside an OSC that isn't ST: the OSC was cut short. Pass what we
                        // held and scan this ESC again as the start of a new sequence.
                        let mut seq = std::mem::take(&mut self.held);
                        seq.truncate(seq.len() - 2);
                        data.extend_from_slice(&seq);
                        self.held.push(0x1b);
                        self.state = State::Esc;
                        i -= 1;
                    }
                }
                State::Str => {
                    self.held.push(b);
                    if b == 0x1b {
                        self.state = State::StrEsc;
                    } else if self.held.len() > MAX_HELD {
                        data.extend_from_slice(&self.held);
                        self.held.clear();
                        self.state = State::PassStr;
                    }
                }
                State::StrEsc => {
                    self.held.push(b);
                    if b == b'\\' {
                        data.extend_from_slice(&self.held);
                        self.held.clear();
                        self.state = State::Ground;
                    } else {
                        self.state = State::Str;
                    }
                }
                State::PassOsc => {
                    data.push(b);
                    if b == 0x07 {
                        self.state = State::Ground;
                    } else if b == 0x1b {
                        self.state = State::PassOscEsc;
                    }
                }
                State::PassOscEsc => {
                    data.push(b);
                    self.state = if b == b'\\' { State::Ground } else { State::PassOsc };
                }
                State::PassStr => {
                    data.push(b);
                    if b == 0x1b {
                        self.state = State::PassStrEsc;
                    }
                }
                State::PassStrEsc => {
                    data.push(b);
                    self.state = if b == b'\\' { State::Ground } else { State::PassStr };
                }
            }
        }
        flush(&mut data, &mut out);
        out
    }

    fn osc_is_interesting(&self) -> bool {
        let body = &self.held[2..];
        body.starts_with(b"133;") || body.starts_with(b"7;") || body.starts_with(CTX_OSC.as_bytes())
    }

    fn finish_csi(&mut self, seq: Vec<u8>, data: &mut Vec<u8>, out: &mut Vec<Piece>) {
        let body = &seq[2..seq.len() - 1];
        let fin = *seq.last().unwrap();
        // Erase scrollback.
        if fin == b'J' && body == b"3" {
            if !data.is_empty() {
                out.push(Piece::Data(std::mem::take(data)));
            }
            out.push(Piece::Mark(Mark::ClearScrollback));
            if self.allow_clear_scrollback {
                data.extend_from_slice(&seq);
            }
            return;
        }
        // Alternate screen on/off: ?1049, ?1047, ?47.
        if (fin == b'h' || fin == b'l') && body.first() == Some(&b'?') {
            let alt = body[1..].split(|c| *c == b';').any(|p| p == b"1049" || p == b"1047" || p == b"47");
            if alt {
                if fin == b'l' {
                    if !data.is_empty() {
                        out.push(Piece::Data(std::mem::take(data)));
                    }
                    out.push(Piece::Mark(Mark::LeavingFullScreen));
                    data.extend_from_slice(&seq);
                } else {
                    data.extend_from_slice(&seq);
                    out.push(Piece::Data(std::mem::take(data)));
                    out.push(Piece::Mark(Mark::EnteredFullScreen));
                }
                return;
            }
        }
        data.extend_from_slice(&seq);
    }

    fn finish_osc(
        &mut self,
        seq: Vec<u8>,
        term_len: usize,
        data: &mut Vec<u8>,
        out: &mut Vec<Piece>,
        flush: &impl Fn(&mut Vec<u8>, &mut Vec<Piece>),
    ) {
        let body = &seq[2..seq.len() - term_len];
        let text = String::from_utf8_lossy(body);
        if let Some(rest) = text.strip_prefix(CTX_OSC).and_then(|r| r.strip_prefix(';')) {
            // Private: handled, never shown.
            flush(data, out);
            out.push(Piece::Mark(Mark::Ctx(rest.to_string())));
            return;
        }
        let mark = if let Some(rest) = text.strip_prefix("133;") {
            let mut parts = rest.split(';');
            match parts.next() {
                Some("A") => Some(Mark::PromptStart),
                Some("B") => Some(Mark::InputStart),
                Some("C") => Some(Mark::OutputStart),
                Some("D") => Some(Mark::CommandEnd(parts.next().and_then(|s| s.trim().parse().ok()))),
                _ => None,
            }
        } else if let Some(rest) = text.strip_prefix("7;") {
            parse_file_url(rest).map(|(host, path)| Mark::Folder { host, path })
        } else {
            None
        };
        // Pass the sequence on (the emulator tracks prompts and folders too), then the mark.
        data.extend_from_slice(&seq);
        if let Some(m) = mark {
            flush(data, out);
            out.push(Piece::Mark(m));
        }
    }
}

/// `file://host/path%20with%20spaces` → (host, path).
pub fn parse_file_url(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("file://").or_else(|| url.strip_prefix("kitty-shell-cwd://"))?;
    let slash = rest.find('/')?;
    let host = rest[..slash].to_string();
    let path = percent_decode(&rest[slash..]);
    Some((host, path))
}

pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = bytes.get(i + 1..i + 3)
            && let Ok(v) = u8::from_str_radix(std::str::from_utf8(hex).unwrap_or("zz"), 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(s: &mut Scanner, input: &[u8]) -> (Vec<u8>, Vec<Mark>) {
        let mut data = Vec::new();
        let mut marks = Vec::new();
        for p in s.feed(input) {
            match p {
                Piece::Data(d) => data.extend(d),
                Piece::Mark(m) => marks.push(m),
            }
        }
        (data, marks)
    }

    #[test]
    fn marks_and_passthrough() {
        let mut s = Scanner::default();
        let input = b"hi\x1b]133;A\x07$ \x1b]133;B\x07ls\r\n\x1b]6973;cmd;bHM=\x07\x1b]133;C\x07out\x1b]133;D;2\x1b\\\x1b]7;file://mac/Users/c/my%20dir\x07";
        let (data, marks) = run(&mut s, input);
        assert_eq!(
            marks,
            vec![
                Mark::PromptStart,
                Mark::InputStart,
                Mark::Ctx("cmd;bHM=".into()),
                Mark::OutputStart,
                Mark::CommandEnd(Some(2)),
                Mark::Folder { host: "mac".into(), path: "/Users/c/my dir".into() }
            ]
        );
        // The private OSC is gone; everything else is still there.
        let text = String::from_utf8_lossy(&data);
        assert!(!text.contains("6973"));
        assert!(text.contains("133;A") && text.contains("out") && text.contains("7;file"));
    }

    #[test]
    fn split_across_reads() {
        let mut s = Scanner::default();
        let input = b"a\x1b]133;D;0\x07b\x1b[31mc";
        let mut all_marks = Vec::new();
        let mut all_data = Vec::new();
        for b in input {
            let (d, m) = run(&mut s, &[*b]);
            all_data.extend(d);
            all_marks.extend(m);
        }
        assert_eq!(all_marks, vec![Mark::CommandEnd(Some(0))]);
        assert_eq!(all_data, input.to_vec());
    }

    #[test]
    fn clear_scrollback_is_dropped_unless_allowed() {
        let mut s = Scanner::default();
        let (data, marks) = run(&mut s, b"\x1b[H\x1b[2J\x1b[3J");
        assert_eq!(marks, vec![Mark::ClearScrollback]);
        assert_eq!(data, b"\x1b[H\x1b[2J".to_vec());
        s.allow_clear_scrollback = true;
        let (data, _) = run(&mut s, b"\x1b[3J");
        assert_eq!(data, b"\x1b[3J".to_vec());
    }

    #[test]
    fn full_screen_marks_are_placed_around_the_switch() {
        let mut s = Scanner::default();
        let out = s.feed(b"x\x1b[?1049hvim\x1b[?1049ly");
        assert_eq!(
            out,
            vec![
                Piece::Data(b"x\x1b[?1049h".to_vec()),
                Piece::Mark(Mark::EnteredFullScreen),
                Piece::Data(b"vim".to_vec()),
                Piece::Mark(Mark::LeavingFullScreen),
                Piece::Data(b"\x1b[?1049ly".to_vec()),
            ]
        );
    }

    #[test]
    fn huge_osc_streams_through() {
        let mut s = Scanner::default();
        let mut input = b"\x1b]52;c;".to_vec();
        input.extend(std::iter::repeat_n(b'A', 100_000));
        input.push(0x07);
        input.extend(b"after");
        let (data, marks) = run(&mut s, &input);
        assert!(marks.is_empty());
        assert_eq!(data, input);
    }

    #[test]
    fn dcs_contents_are_not_scanned() {
        let mut s = Scanner::default();
        let input = b"\x1bPq]133;A\x07\x1b\\done";
        let (data, marks) = run(&mut s, input);
        assert!(marks.is_empty());
        assert_eq!(data, input.to_vec());
    }
}
