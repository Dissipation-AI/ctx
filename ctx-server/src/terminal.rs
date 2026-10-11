//! Terminal emulation behind a small trait, so the emulator can be swapped (the spec's fallback
//! is `alacritty_terminal`). The implementation is Ghostty's `libghostty-vt`.
//!
//! Rows are addressed in "screen" coordinates: 0 is the oldest row the emulator still holds in
//! its scrollback, and the active screen is at the bottom. Lines that scroll far enough are
//! *harvested* exactly once, in order, so ctx can move them to its compressed store and its
//! saved records.

use ghostty_vt::fmt::{Format, Formatter, FormatterOptions};
use ghostty_vt::key::{self, Encoder, Key, Mods};
use ghostty_vt::screen::{Screen, TrackedGridRef};
use ghostty_vt::selection::Selection;
use ghostty_vt::terminal::{Mode, Point, PointCoordinate, PointSpace};
use ghostty_vt::{Event, TerminalOptions};

/// A row that left the emulator's view of the world.
#[derive(Debug, Clone, PartialEq)]
pub struct HarvestedRow {
    /// Text with SGR styles.
    pub text: String,
    /// The row soft-wraps into the next one.
    pub wraps: bool,
    /// An event line drawn by ctx.
    pub event: bool,
    /// Screen row it had when harvested (for comparison with pins).
    pub y: u64,
}

/// Something the emulator wants done.
#[derive(Debug, Clone, PartialEq)]
pub enum TermEvent {
    /// Bytes to send to the program (answers to queries).
    Reply(Vec<u8>),
    Title(String),
    Bell,
    /// OSC 52 clipboard write (reads are refused).
    Clipboard(String),
}

/// A row anchor that follows its content as the screen scrolls.
#[derive(Debug)]
pub struct Pin(TrackedGridRef);

pub trait Terminal: std::fmt::Debug {
    fn write(&mut self, data: &[u8]);
    fn take_events(&mut self) -> Vec<TermEvent>;
    fn resize(&mut self, columns: u16, rows: u16);
    fn size(&self) -> (u16, u16);
    /// Cursor (column, row within the active screen).
    fn cursor(&self) -> (u16, u16);
    fn cursor_visible(&self) -> bool;
    fn full_screen(&self) -> bool;
    fn bracketed_paste(&self) -> bool;
    fn title(&self) -> Option<String>;
    /// Rows held in scrollback (above the active screen).
    fn history_rows(&self) -> u64;
    /// One screen row as text (styled keeps SGR).
    fn row(&self, y: u64, styled: bool) -> String;
    fn row_wraps(&self, y: u64) -> bool;
    /// Screen row of the top of the active screen.
    fn active_top(&self) -> u64 {
        self.history_rows()
    }
    /// The visible screen, one string per row (trailing blank rows kept).
    fn screen(&self, styled: bool) -> Vec<String>;
    /// Rows that have scrolled into history since the last call, oldest first.
    fn harvest(&mut self) -> Vec<HarvestedRow>;
    /// Screen row of the next row harvest() will return.
    fn harvest_position(&self) -> u64;
    /// Pin the start of the cursor's row.
    fn pin_cursor_row(&mut self) -> Option<Pin>;
    fn pin_row(&self, pin: &Pin) -> Option<u64>;
    /// Draw a dimmed event line above the line at `prompt_top` (or the cursor's line) so it
    /// becomes part of the scrollback, without disturbing the program at the prompt.
    /// Returns where the prompt's top row is afterwards (screen row), when one was given.
    fn insert_event_line(&mut self, text: &str, prompt_top: Option<u64>) -> Option<u64>;
    /// Pin a screen row.
    fn pin_screen_row(&mut self, y: u64) -> Option<Pin>;
    fn encode_key(&mut self, key: &str) -> Option<Vec<u8>>;
    fn encode_paste(&self, text: &str) -> Vec<u8>;
    /// Put back every mode a program can leave on (mouse, keypad, alternate screen…).
    fn reset_program_modes(&mut self);
    /// Erase screen and scrollback.
    fn clear_all(&mut self);
    fn mouse_tracking(&self) -> bool;
    fn focus_reporting(&self) -> bool;
    /// Estimated bytes held.
    fn memory_estimate(&self) -> u64;
    fn ghostty(&self) -> Option<&ghostty_vt::Terminal> {
        None
    }
    fn ghostty_mut(&mut self) -> Option<&mut ghostty_vt::Terminal> {
        None
    }
}

pub struct Ghostty {
    t: ghostty_vt::Terminal,
    /// Next row to harvest; None until there's any history.
    harvest_pin: Option<TrackedGridRef>,
    /// Event lines not yet harvested.
    event_pins: Vec<TrackedGridRef>,
    encoder: Encoder,
    events: Vec<TermEvent>,
}

impl std::fmt::Debug for Ghostty {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Ghostty")
    }
}

/// Bytes of scrollback the emulator keeps before ctx's own store takes over: about 1,000 lines
/// at typical widths (Ghostty prunes in ~400 KiB pages).
pub const HOT_SCROLLBACK_LINES: usize = 1000;

impl Ghostty {
    pub fn new(columns: u16, rows: u16) -> Ghostty {
        let mut t = ghostty_vt::Terminal::new(TerminalOptions {
            cols: columns.max(2),
            rows: rows.max(2),
            max_scrollback: 64 * 1024 * 1024,
        })
        .expect("terminal");
        let _ = t.set_scrollback_max_lines(None);
        let _ = t.set_terminfo_name(Some("xterm-256color"));
        let _ = t.set_clipboard_write_max_bytes(Some(1024 * 1024));
        t.set_clipboard_write_policy(ghostty_vt::terminal::ClipboardWritePolicy::Allow);
        let _ = t.set_title_report(false);
        t.set_xtversion(Some(format!("ctx {}", env!("CARGO_PKG_VERSION"))));
        Ghostty {
            t,
            harvest_pin: None,
            event_pins: Vec::new(),
            encoder: Encoder::new().expect("key encoder"),
            events: Vec::new(),
        }
    }

    fn total_rows(&self) -> u64 {
        self.t.total_rows().unwrap_or(0) as u64
    }

    fn format_rows(&self, y0: u64, y1: u64, styled: bool) -> String {
        let cols = self.t.cols().unwrap_or(80);
        self.format_points(
            Point::Screen(PointCoordinate { x: 0, y: y0 as u32 }),
            Point::Screen(PointCoordinate { x: cols - 1, y: y1 as u32 }),
            styled,
        )
    }

    /// One row of the active area (the program's screen, whichever buffer is in front).
    pub fn active_row(&self, y: u16, styled: bool) -> String {
        let cols = self.t.cols().unwrap_or(80);
        self.format_points(
            Point::Active(PointCoordinate { x: 0, y: y as u32 }),
            Point::Active(PointCoordinate { x: cols - 1, y: y as u32 }),
            styled,
        )
    }

    fn format_points(&self, a: Point, b: Point, styled: bool) -> String {
        let (Ok(a), Ok(b)) = (self.t.grid_ref(a), self.t.grid_ref(b)) else {
            return String::new();
        };
        let sel = Selection::new(a, b, false);
        let fmt = if styled { Format::Vt } else { Format::Plain };
        // A formatter rather than the one-shot selection formatter: that one also emits the
        // whole colour palette (OSC 4) in front of every row.
        let opts = FormatterOptions::new().with_format(fmt).with_trim(true).with_unwrap(false).with_selection(&sel);
        let out = Formatter::new(&self.t, opts).and_then(|mut f| f.format_alloc());
        match out {
            Ok(bytes) => {
                let mut s = String::from_utf8_lossy(&bytes).into_owned();
                while s.ends_with('\n') || s.ends_with('\r') {
                    s.pop();
                }
                s
            }
            _ => String::new(),
        }
    }
}

fn key_for(name: &str) -> Option<(Key, Option<String>)> {
    let k = match name {
        "enter" | "return" => Key::Enter,
        "escape" => Key::Escape,
        "tab" => Key::Tab,
        "backspace" => Key::Backspace,
        "delete" => Key::Delete,
        "space" => return Some((Key::Space, Some(" ".into()))),
        "up" => Key::ArrowUp,
        "down" => Key::ArrowDown,
        "left" => Key::ArrowLeft,
        "right" => Key::ArrowRight,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "insert" => Key::Insert,
        "f1" => Key::F1,
        "f2" => Key::F2,
        "f3" => Key::F3,
        "f4" => Key::F4,
        "f5" => Key::F5,
        "f6" => Key::F6,
        "f7" => Key::F7,
        "f8" => Key::F8,
        "f9" => Key::F9,
        "f10" => Key::F10,
        "f11" => Key::F11,
        "f12" => Key::F12,
        other => {
            let mut chars = other.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            let key = match c.to_ascii_lowercase() {
                'a' => Key::A,
                'b' => Key::B,
                'c' => Key::C,
                'd' => Key::D,
                'e' => Key::E,
                'f' => Key::F,
                'g' => Key::G,
                'h' => Key::H,
                'i' => Key::I,
                'j' => Key::J,
                'k' => Key::K,
                'l' => Key::L,
                'm' => Key::M,
                'n' => Key::N,
                'o' => Key::O,
                'p' => Key::P,
                'q' => Key::Q,
                'r' => Key::R,
                's' => Key::S,
                't' => Key::T,
                'u' => Key::U,
                'v' => Key::V,
                'w' => Key::W,
                'x' => Key::X,
                'y' => Key::Y,
                'z' => Key::Z,
                '0' => Key::Digit0,
                '1' => Key::Digit1,
                '2' => Key::Digit2,
                '3' => Key::Digit3,
                '4' => Key::Digit4,
                '5' => Key::Digit5,
                '6' => Key::Digit6,
                '7' => Key::Digit7,
                '8' => Key::Digit8,
                '9' => Key::Digit9,
                '-' => Key::Minus,
                '=' => Key::Equal,
                '[' => Key::BracketLeft,
                ']' => Key::BracketRight,
                '\\' => Key::Backslash,
                ';' => Key::Semicolon,
                '\'' => Key::Quote,
                ',' => Key::Comma,
                '.' => Key::Period,
                '/' => Key::Slash,
                '`' => Key::Backquote,
                _ => Key::Unidentified,
            };
            return Some((key, Some(c.to_string())));
        }
    };
    Some((k, None))
}

/// Split `ctrl-shift-up` into modifiers and the key name.
pub fn split_key(name: &str) -> (Mods, String) {
    let lower = name.to_lowercase();
    let (mods_part, key) = if lower == "-" || lower.ends_with("--") {
        (lower.trim_end_matches('-').trim_end_matches('-'), "-".to_string())
    } else {
        match lower.rsplit_once('-') {
            Some((m, k)) => (m, k.to_string()),
            None => ("", lower.clone()),
        }
    };
    let mut mods = Mods::empty();
    for m in mods_part.split('-').filter(|m| !m.is_empty()) {
        match m {
            "ctrl" => mods |= Mods::CTRL,
            "alt" => mods |= Mods::ALT,
            "shift" => mods |= Mods::SHIFT,
            "cmd" => mods |= Mods::SUPER,
            _ => {}
        }
    }
    (mods, key)
}

impl Terminal for Ghostty {
    fn write(&mut self, data: &[u8]) {
        self.t.vt_write(data);
        for e in self.t.events() {
            match e {
                Event::PtyWrite(b) => self.events.push(TermEvent::Reply(b)),
                Event::Bell => self.events.push(TermEvent::Bell),
                Event::TitleChanged(t) => self.events.push(TermEvent::Title(t)),
                Event::ClipboardWrite(w) => {
                    if let Some(c) = w.contents.first() {
                        self.events.push(TermEvent::Clipboard(String::from_utf8_lossy(&c.data).into_owned()));
                    }
                }
                _ => {}
            }
        }
    }

    fn take_events(&mut self) -> Vec<TermEvent> {
        std::mem::take(&mut self.events)
    }

    fn resize(&mut self, columns: u16, rows: u16) {
        let _ = self.t.resize(columns.max(2), rows.max(2), 0, 0);
    }

    fn size(&self) -> (u16, u16) {
        (self.t.cols().unwrap_or(80), self.t.rows().unwrap_or(24))
    }

    fn cursor(&self) -> (u16, u16) {
        (self.t.cursor_x().unwrap_or(0), self.t.cursor_y().unwrap_or(0))
    }

    fn cursor_visible(&self) -> bool {
        self.t.is_cursor_visible().unwrap_or(true)
    }

    fn full_screen(&self) -> bool {
        matches!(self.t.active_screen(), Ok(Screen::Alternate))
    }

    fn bracketed_paste(&self) -> bool {
        self.t.mode(Mode::BRACKETED_PASTE).unwrap_or(false)
    }

    fn title(&self) -> Option<String> {
        self.t.title().ok().filter(|t| !t.is_empty()).map(str::to_string)
    }

    fn history_rows(&self) -> u64 {
        self.t.scrollback_rows().unwrap_or(0) as u64
    }

    fn row(&self, y: u64, styled: bool) -> String {
        if y >= self.total_rows() {
            return String::new();
        }
        self.format_rows(y, y, styled)
    }

    fn screen(&self, styled: bool) -> Vec<String> {
        (0..self.size().1).map(|i| self.active_row(i, styled)).collect()
    }

    fn row_wraps(&self, y: u64) -> bool {
        self.t
            .grid_ref(Point::Screen(PointCoordinate { x: 0, y: y as u32 }))
            .ok()
            .and_then(|g| g.row().ok())
            .and_then(|r| r.is_wrapped().ok())
            .unwrap_or(false)
    }

    fn harvest(&mut self) -> Vec<HarvestedRow> {
        let mut out = Vec::new();
        if self.full_screen() {
            return out;
        }
        // The pin sits on the last row harvested; the next row is the one after it. If that row
        // has been pruned, everything before the oldest remaining row was harvested already.
        let history = self.history_rows();
        let mut next = match &self.harvest_pin {
            None => 0,
            Some(pin) => match pin.point(PointSpace::Screen) {
                Ok(Some(p)) => p.y as u64 + 1,
                _ => 0,
            },
        };
        while next < history {
            let y = next;
            let text = self.format_rows(y, y, true);
            let wraps = self.row_wraps(y);
            let mut event = false;
            self.event_pins.retain(|p| match p.point(PointSpace::Screen) {
                Ok(Some(pp)) if pp.y as u64 == y => {
                    event = true;
                    false
                }
                Ok(Some(_)) => true,
                _ => false,
            });
            out.push(HarvestedRow { text, wraps, event, y });
            next += 1;
        }
        // Prune only rows already harvested: the emulator has no line limit of its own (it prunes
        // in whole pages, which could hold rows not yet harvested), so trim it back here.
        if !out.is_empty() && self.history_rows() > (HOT_SCROLLBACK_LINES * 2) as u64 {
            let _ = self.t.set_scrollback_max_lines(Some(HOT_SCROLLBACK_LINES));
            let _ = self.t.set_scrollback_max_lines(None);
            // Event pins in pruned rows are gone with them.
            self.event_pins.retain(|p| matches!(p.point(PointSpace::Screen), Ok(Some(_))));
            let remaining = self.history_rows();
            if remaining > 0 {
                let point = Point::Screen(PointCoordinate { x: 0, y: (remaining - 1) as u32 });
                self.harvest_pin = self.t.track_grid_ref(point).ok();
            } else {
                self.harvest_pin = None;
            }
            return out;
        }
        if let Some(last) = out.last() {
            let point = Point::Screen(PointCoordinate { x: 0, y: last.y as u32 });
            match self.harvest_pin.as_mut() {
                Some(pin) => {
                    let _ = pin.set(&mut self.t, point);
                }
                None => self.harvest_pin = self.t.track_grid_ref(point).ok(),
            }
        }
        out
    }

    fn harvest_position(&self) -> u64 {
        match &self.harvest_pin {
            Some(p) => p.point(PointSpace::Screen).ok().flatten().map(|p| p.y as u64 + 1).unwrap_or(0),
            None => 0,
        }
    }

    fn pin_cursor_row(&mut self) -> Option<Pin> {
        let (_, cy) = self.cursor();
        let y = self.history_rows() + cy as u64;
        self.t.track_grid_ref(Point::Screen(PointCoordinate { x: 0, y: y as u32 })).ok().map(Pin)
    }

    fn pin_row(&self, pin: &Pin) -> Option<u64> {
        pin.0.point(PointSpace::Screen).ok().flatten().map(|p| p.y as u64)
    }

    fn pin_screen_row(&mut self, y: u64) -> Option<Pin> {
        self.t.track_grid_ref(Point::Screen(PointCoordinate { x: 0, y: y as u32 })).ok().map(Pin)
    }

    fn insert_event_line(&mut self, text: &str, prompt_top: Option<u64>) -> Option<u64> {
        let (_, cy) = self.cursor();
        let (_, rows) = self.size();
        let top = self.history_rows();
        // Active-screen row (0-based) that the event goes directly above.
        let anchor = match prompt_top {
            Some(y) if y >= top && y < top + rows as u64 => (y - top) as u16,
            _ => cy,
        };
        let line = format!("\x1b[0;2m{text}\x1b[0m");
        let bottom_free = cy + 1 < rows && self.active_row(rows - 1, false).trim().is_empty();
        let (seq, event_row) = if bottom_free {
            // Room below: insert a line at the anchor, pushing the prompt (and cursor) down.
            (format!("\x1b7\x1b[{r};1H\x1b[L\x1b[2K{line}\x1b8\x1b[B", r = anchor + 1), anchor)
        } else if anchor == 0 {
            // Prompt at the top of a full screen: nothing above to keep; insert anyway.
            (format!("\x1b7\x1b[1;1H\x1b[L\x1b[2K{line}\x1b8\x1b[B"), 0)
        } else {
            // Screen full: scroll everything up one row (the top row goes to scrollback), then
            // insert a line just above the prompt, which brings the prompt back to its row.
            (format!("\x1b7\x1b[{rows};1H\n\x1b[{r};1H\x1b[L\x1b[2K{line}\x1b8", r = anchor), anchor - 1)
        };
        self.t.vt_write(seq.as_bytes());
        let _ = self.t.events().count();
        let top = self.history_rows();
        let event_row = top + event_row as u64;
        if let Ok(p) = self.t.track_grid_ref(Point::Screen(PointCoordinate { x: 0, y: event_row as u32 })) {
            self.event_pins.push(p);
        }
        prompt_top.map(|_| event_row + 1)
    }

    fn encode_key(&mut self, key_name: &str) -> Option<Vec<u8>> {
        let (mods, name) = split_key(key_name);
        let (key, text) = key_for(&name)?;
        self.encoder.set_options_from_terminal(&self.t);
        let mut ev = key::Event::new().ok()?;
        ev.set_action(key::Action::Press).set_key(key).set_mods(mods);
        let mut unshifted = None;
        if let Some(t) = &text {
            let c = t.chars().next().unwrap();
            unshifted = Some(c.to_ascii_lowercase());
            let shown = if mods.contains(Mods::SHIFT) { c.to_ascii_uppercase().to_string() } else { t.clone() };
            if !mods.intersects(Mods::CTRL | Mods::ALT | Mods::SUPER) {
                ev.set_utf8(Some(shown));
            }
        }
        if let Some(u) = unshifted {
            ev.set_unshifted_codepoint(u);
        }
        let mut out = Vec::new();
        self.encoder.encode_to_vec(&ev, &mut out).ok()?;
        if out.is_empty() {
            // Keys the encoder leaves to the caller: plain characters, and ctrl-<letter>.
            if let Some(t) = text {
                let c = t.chars().next().unwrap();
                if mods.contains(Mods::CTRL) && c.is_ascii_alphabetic() {
                    out.push((c.to_ascii_lowercase() as u8) & 0x1f);
                } else {
                    if mods.contains(Mods::ALT) {
                        out.push(0x1b);
                    }
                    out.extend_from_slice(t.as_bytes());
                }
            }
        }
        Some(out)
    }

    fn encode_paste(&self, text: &str) -> Vec<u8> {
        let clean: String = text.chars().filter(|c| *c != '\u{1b}').collect();
        if self.bracketed_paste() {
            let mut v = b"\x1b[200~".to_vec();
            v.extend_from_slice(clean.as_bytes());
            v.extend_from_slice(b"\x1b[201~");
            v
        } else {
            clean.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
        }
    }

    fn reset_program_modes(&mut self) {
        let mut seq = String::new();
        let checks: &[(Mode, &str)] = &[
            (Mode::ALT_SCREEN_SAVE, "\x1b[?1049l"),
            (Mode::ALT_SCREEN, "\x1b[?1047l"),
            (Mode::ALT_SCREEN_LEGACY, "\x1b[?47l"),
            (Mode::X10_MOUSE, "\x1b[?9l"),
            (Mode::NORMAL_MOUSE, "\x1b[?1000l"),
            (Mode::BUTTON_MOUSE, "\x1b[?1002l"),
            (Mode::ANY_MOUSE, "\x1b[?1003l"),
            (Mode::SGR_MOUSE, "\x1b[?1006l"),
            (Mode::URXVT_MOUSE, "\x1b[?1015l"),
            (Mode::UTF8_MOUSE, "\x1b[?1005l"),
            (Mode::SGR_PIXELS_MOUSE, "\x1b[?1016l"),
            (Mode::FOCUS_EVENT, "\x1b[?1004l"),
            (Mode::DECCKM, "\x1b[?1l"),
            (Mode::KEYPAD_KEYS, "\x1b[?66l"),
            (Mode::ORIGIN, "\x1b[?6l"),
            (Mode::INSERT, "\x1b[4l"),
            (Mode::SYNC_OUTPUT, "\x1b[?2026l"),
        ];
        for (mode, off) in checks {
            if self.t.mode(*mode).unwrap_or(false) {
                seq.push_str(off);
            }
        }
        if !self.t.mode(Mode::CURSOR_VISIBLE).unwrap_or(true) {
            seq.push_str("\x1b[?25h");
        }
        if !self.t.mode(Mode::WRAPAROUND).unwrap_or(true) {
            seq.push_str("\x1b[?7h");
        }
        if self.t.kitty_keyboard_flags().map(|f| !f.is_empty()).unwrap_or(false) {
            // Pop every pushed kitty keyboard mode.
            seq.push_str("\x1b[<99u");
        }
        if !seq.is_empty() {
            // Application keypad off (DECKPNM) and character sets back to normal.
            seq.push_str("\x1b>\x1b(B");
            self.t.vt_write(seq.as_bytes());
            let _ = self.t.events().count();
        }
    }

    fn clear_all(&mut self) {
        self.t.vt_write(b"\x1b[H\x1b[2J\x1b[3J");
        let _ = self.t.events().count();
        self.harvest_pin = None;
        self.event_pins.clear();
    }

    fn mouse_tracking(&self) -> bool {
        self.t.is_mouse_tracking().unwrap_or(false)
    }

    fn focus_reporting(&self) -> bool {
        self.t.mode(Mode::FOCUS_EVENT).unwrap_or(false)
    }

    fn memory_estimate(&self) -> u64 {
        // Ghostty pages: measured ~1.2 KB per row at 120 columns (see Phase 0 notes).
        let (cols, _) = self.size();
        self.total_rows() * (cols as u64 * 10 + 64)
    }

    fn ghostty(&self) -> Option<&ghostty_vt::Terminal> {
        Some(&self.t)
    }

    fn ghostty_mut(&mut self) -> Option<&mut ghostty_vt::Terminal> {
        Some(&mut self.t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harvest_returns_every_line_once_in_order() {
        let mut t = Ghostty::new(40, 5);
        let mut got = Vec::new();
        for i in 0..3000 {
            t.write(format!("line {i}\r\n").as_bytes());
            if i % 13 == 0 {
                got.extend(t.harvest());
            }
        }
        got.extend(t.harvest());
        let plain: Vec<String> = got.iter().map(|r| ctx_store::ansi::strip(&r.text)).collect();
        for (i, l) in plain.iter().enumerate() {
            assert_eq!(l, &format!("line {i}"), "at {i} of {}", plain.len());
        }
        assert!(plain.len() > 2990, "{}", plain.len());
    }

    #[test]
    fn styled_rows_keep_colours() {
        let mut t = Ghostty::new(40, 5);
        t.write(b"\x1b[31mred\x1b[0m plain\r\n");
        let row = t.row(0, true);
        assert!(row.contains("\x1b[") && row.contains("red"), "{row:?}");
        assert_eq!(t.row(0, false), "red plain");
    }

    #[test]
    fn event_lines_are_harvested_as_events() {
        let mut t = Ghostty::new(40, 4);
        t.write(b"one\r\ntwo\r\n$ ");
        t.insert_event_line("[renamed \"a\" -> \"b\"]", None);
        // The prompt stays on the cursor's row with the event just above it.
        let screen = t.screen(false);
        let cy = t.cursor().1 as usize;
        assert_eq!(screen[cy], "$");
        assert_eq!(screen[cy - 1], "[renamed \"a\" -> \"b\"]");
        for _ in 0..6 {
            t.write(b"\r\nmore");
        }
        let rows = t.harvest();
        let events: Vec<_> = rows.iter().filter(|r| r.event).map(|r| ctx_store::ansi::strip(&r.text)).collect();
        assert_eq!(events, vec!["[renamed \"a\" -> \"b\"]".to_string()]);
        let plain: Vec<String> = rows.iter().map(|r| ctx_store::ansi::strip(&r.text)).collect();
        assert_eq!(&plain[..3], &["one", "two", "[renamed \"a\" -> \"b\"]"]);
    }

    #[test]
    fn two_events_above_a_prompt_keep_their_order() {
        let mut t = Ghostty::new(40, 6);
        t.write(b"out 1\r\nout 2\r\n");
        let top = Some(t.history_rows() + t.cursor().1 as u64);
        t.write(b"$ ");
        let top = t.insert_event_line("[first]", top);
        t.insert_event_line("[second]", top);
        let screen = t.screen(false);
        let cy = t.cursor().1 as usize;
        assert_eq!(screen[cy], "$");
        assert_eq!(screen[cy - 1], "[second]");
        assert_eq!(screen[cy - 2], "[first]");
        assert_eq!(screen[cy - 3], "out 2");
    }

    #[test]
    fn events_above_a_prompt_on_the_second_row_and_at_the_bottom() {
        let mut t = Ghostty::new(40, 4);
        t.write(b"[opened]\r\n");
        let top = Some(t.history_rows() + t.cursor().1 as u64);
        t.write(b"$ ");
        let top = t.insert_event_line("[renamed]", top);
        let _ = t.insert_event_line("[moved]", top);
        let (all, _) = (0..t.history_rows() + 4).map(|y| t.row(y, false)).fold((Vec::new(), ()), |(mut v, _), r| {
            v.push(r);
            (v, ())
        });
        let all: Vec<String> = all.into_iter().filter(|l| !l.is_empty()).collect();
        assert_eq!(all, vec!["[opened]", "[renamed]", "[moved]", "$"]);
        // Prompt on the last row: earlier lines scroll into history.
        let mut t = Ghostty::new(40, 3);
        t.write(b"a\r\nb\r\n");
        let top = Some(t.history_rows() + t.cursor().1 as u64);
        t.write(b"$ ");
        t.insert_event_line("[e]", top);
        let all: Vec<String> = (0..t.history_rows() + 3).map(|y| t.row(y, false)).collect();
        assert_eq!(all, vec!["a", "b", "[e]", "$"]);
        assert_eq!(t.cursor(), (2, 2));
    }

    #[test]
    fn event_line_on_first_row() {
        let mut t = Ghostty::new(40, 4);
        t.write(b"$ ");
        t.insert_event_line("[opened]", None);
        let screen = t.screen(false);
        assert_eq!(screen[0], "[opened]");
        assert_eq!(screen[1], "$");
        assert_eq!(t.cursor(), (2, 1));
    }

    #[test]
    fn keys_encode() {
        let mut t = Ghostty::new(40, 4);
        assert_eq!(t.encode_key("enter").unwrap(), b"\r");
        assert_eq!(t.encode_key("ctrl-c").unwrap(), vec![3]);
        assert_eq!(t.encode_key("escape").unwrap(), vec![0x1b]);
        assert_eq!(t.encode_key("up").unwrap(), b"\x1b[A");
        assert_eq!(t.encode_key("a").unwrap(), b"a");
        t.write(b"\x1b[?1h");
        assert_eq!(t.encode_key("up").unwrap(), b"\x1bOA");
    }

    #[test]
    fn paste_brackets_when_asked() {
        let mut t = Ghostty::new(40, 4);
        assert_eq!(t.encode_paste("a\nb"), b"a\rb");
        t.write(b"\x1b[?2004h");
        assert_eq!(t.encode_paste("a\nb"), b"\x1b[200~a\nb\x1b[201~");
    }

    #[test]
    fn modes_reset() {
        let mut t = Ghostty::new(40, 4);
        t.write(b"\x1b[?1049h\x1b[?1000h\x1b[?1006h\x1b[?25l");
        assert!(t.full_screen() && t.mouse_tracking());
        t.reset_program_modes();
        assert!(!t.full_screen() && !t.mouse_tracking() && t.cursor_visible());
    }
}
