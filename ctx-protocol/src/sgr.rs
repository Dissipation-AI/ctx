//! Text with SGR escape sequences (as stored in scrollback and saved records) turned into styled
//! runs, with palette colours resolved through a viewer's palette. Shared by the server (which
//! applies CTX.app's colours) and CTX.app (saved tabs in the History window).

use crate::{CellStyle, StyledRun};

/// The xterm 256-colour palette built from 16 ANSI colours: the 6×6×6 cube and 24 greys follow.
pub fn full_palette(ansi: &[[u8; 3]]) -> [[u8; 3]; 256] {
    let mut p = [[0u8; 3]; 256];
    for (i, c) in DEFAULT_ANSI.iter().enumerate() {
        p[i] = ansi.get(i).copied().unwrap_or(*c);
    }
    let level = |v: usize| if v == 0 { 0u8 } else { (55 + v * 40) as u8 };
    for i in 0..216 {
        p[16 + i] = [level(i / 36), level((i / 6) % 6), level(i % 6)];
    }
    for i in 0..24 {
        let g = (8 + i * 10) as u8;
        p[232 + i] = [g, g, g];
    }
    p
}

/// Terminal.app's "Basic" ANSI colours.
pub const DEFAULT_ANSI: [[u8; 3]; 16] = [
    [0, 0, 0],
    [153, 0, 0],
    [0, 166, 0],
    [153, 153, 0],
    [0, 0, 178],
    [178, 0, 178],
    [0, 166, 178],
    [191, 191, 191],
    [102, 102, 102],
    [229, 0, 0],
    [0, 217, 0],
    [229, 229, 0],
    [0, 0, 255],
    [229, 0, 229],
    [0, 229, 229],
    [229, 229, 229],
];

/// Parse `text` into runs. Escape sequences other than SGR are dropped.
pub fn runs(text: &str, palette: &[[u8; 3]; 256]) -> Vec<StyledRun> {
    let mut out: Vec<StyledRun> = Vec::new();
    let mut style = CellStyle::default();
    let mut chars = text.char_indices().peekable();
    let push = |out: &mut Vec<StyledRun>, style: CellStyle, s: &str| {
        if s.is_empty() {
            return;
        }
        match out.last_mut() {
            Some(last) if last.style == style => last.text.push_str(s),
            _ => out.push(StyledRun { text: s.to_string(), style, cells: Vec::new() }),
        }
    };
    let mut start = 0usize;
    while let Some((i, c)) = chars.next() {
        if c != '\x1b' {
            continue;
        }
        push(&mut out, style, &text[start..i]);
        match chars.peek().map(|(_, c)| *c) {
            Some('[') => {
                chars.next();
                let mut params = String::new();
                let mut fin = None;
                for (_, c) in chars.by_ref() {
                    if ('\x40'..='\x7e').contains(&c) {
                        fin = Some(c);
                        break;
                    }
                    params.push(c);
                }
                if fin == Some('m') {
                    apply(&mut style, &params, palette);
                }
            }
            Some(']') => {
                // OSC: skip to BEL or ST.
                chars.next();
                while let Some((_, c)) = chars.next() {
                    if c == '\x07' {
                        break;
                    }
                    if c == '\x1b' {
                        chars.next();
                        break;
                    }
                }
            }
            Some(_) => {
                chars.next();
            }
            None => {}
        }
        start = chars.peek().map(|(j, _)| *j).unwrap_or(text.len());
    }
    push(&mut out, style, &text[start..]);
    out
}

fn apply(style: &mut CellStyle, params: &str, palette: &[[u8; 3]; 256]) {
    if params.is_empty() {
        *style = CellStyle::default();
        return;
    }
    // Parameters separated by ';'; sub-parameters by ':' (e.g. 4:3, 38:2::r:g:b).
    let groups: Vec<Vec<Option<u32>>> =
        params.split(';').map(|g| g.split(':').map(|p| p.parse().ok()).collect()).collect();
    let mut i = 0;
    while i < groups.len() {
        let g = &groups[i];
        let code = g.first().copied().flatten().unwrap_or(0);
        match code {
            0 => *style = CellStyle::default(),
            1 => style.flags |= 1,
            2 => style.flags |= 4,
            3 => style.flags |= 2,
            4 => {
                if g.get(1).copied().flatten() == Some(0) {
                    style.flags &= !8;
                } else {
                    style.flags |= 8;
                }
            }
            5 | 6 => style.flags |= 128,
            7 => style.flags |= 16,
            8 => style.flags |= 64,
            9 => style.flags |= 32,
            21 => style.flags |= 8,
            22 => style.flags &= !(1 | 4),
            23 => style.flags &= !2,
            24 => style.flags &= !8,
            25 => style.flags &= !128,
            27 => style.flags &= !16,
            28 => style.flags &= !64,
            29 => style.flags &= !32,
            30..=37 => style.foreground = Some(palette[(code - 30) as usize]),
            39 => style.foreground = None,
            40..=47 => style.background = Some(palette[(code - 40) as usize]),
            49 => style.background = None,
            90..=97 => style.foreground = Some(palette[(code - 90 + 8) as usize]),
            100..=107 => style.background = Some(palette[(code - 100 + 8) as usize]),
            38 | 48 | 58 => {
                let (color, used) = if g.len() > 1 {
                    (extended(&g[1..], palette), 0)
                } else {
                    let rest: Vec<Option<u32>> = groups[i + 1..].iter().map(|g| g.first().copied().flatten()).collect();
                    let c = extended(&rest, palette);
                    let used = match rest.first().copied().flatten() {
                        Some(5) => 2,
                        Some(2) => 4,
                        _ => 0,
                    };
                    (c, used)
                };
                match code {
                    38 => style.foreground = color,
                    48 => style.background = color,
                    _ => {}
                }
                i += used;
            }
            _ => {}
        }
        i += 1;
    }
}

/// `5;n` or `2;r;g;b` (also `2::r:g:b`, with an empty colour-space id).
fn extended(p: &[Option<u32>], palette: &[[u8; 3]; 256]) -> Option<[u8; 3]> {
    match p.first().copied().flatten()? {
        5 => p.get(1).copied().flatten().map(|n| palette[(n as usize).min(255)]),
        2 => {
            let vals: Vec<u32> = p[1..].iter().map(|v| v.unwrap_or(0)).collect();
            let rgb = if vals.len() >= 4 { &vals[vals.len() - 3..] } else { &vals[..] };
            if rgb.len() == 3 { Some([rgb[0] as u8, rgb[1] as u8, rgb[2] as u8]) } else { None }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pal() -> [[u8; 3]; 256] {
        full_palette(&DEFAULT_ANSI)
    }

    #[test]
    fn plain_text_is_one_run() {
        let r = runs("hello world", &pal());
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].text, "hello world");
        assert_eq!(r[0].style, CellStyle::default());
    }

    #[test]
    fn colours_and_resets() {
        let r = runs("a\x1b[1;31mb\x1b[0mc\x1b[38;2;1;2;3md\x1b[48;5;196me", &pal());
        let texts: Vec<&str> = r.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(texts, ["a", "b", "c", "d", "e"]);
        assert_eq!(r[1].style.flags & 1, 1);
        assert_eq!(r[1].style.foreground, Some(DEFAULT_ANSI[1]));
        assert_eq!(r[2].style, CellStyle::default());
        assert_eq!(r[3].style.foreground, Some([1, 2, 3]));
        assert_eq!(r[4].style.background, Some(pal()[196]));
    }

    #[test]
    fn colon_forms_and_other_sequences() {
        let r = runs("\x1b[38:2::9:8:7mx\x1b]8;;http://a\x07y\x1b[4:3mz", &pal());
        assert_eq!(r[0].text, "xy");
        assert_eq!(r[0].style.foreground, Some([9, 8, 7]));
        assert_eq!(r[1].style.flags & 8, 8);
    }

    #[test]
    fn palette_cube() {
        let p = pal();
        assert_eq!(p[16], [0, 0, 0]);
        assert_eq!(p[231], [255, 255, 255]);
        assert_eq!(p[232], [8, 8, 8]);
    }
}
