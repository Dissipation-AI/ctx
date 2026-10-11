//! Lines are kept with their SGR (colour and style) sequences. These helpers turn them into plain
//! text and cut them to a number of visible characters.

/// Remove escape sequences (CSI, OSC such as OSC 8 hyperlinks, and lone ESC pairs).
pub fn strip(s: &str) -> String {
    if !s.contains('\u{1b}') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('[') => {
                // CSI: parameters then a final byte in @..~
                for d in chars.by_ref() {
                    if ('@'..='~').contains(&d) {
                        break;
                    }
                }
            }
            Some(']') => {
                // OSC: until BEL or ESC \
                while let Some(d) = chars.next() {
                    if d == '\u{7}' {
                        break;
                    }
                    if d == '\u{1b}' {
                        if chars.peek() == Some(&'\\') {
                            chars.next();
                        }
                        break;
                    }
                }
            }
            Some(_) | None => {}
        }
    }
    out
}

/// Whether a line has any escape sequences.
pub fn is_styled(s: &str) -> bool {
    s.contains('\u{1b}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_sgr_and_hyperlinks() {
        assert_eq!(strip("\u{1b}[1;32mok\u{1b}[0m done"), "ok done");
        assert_eq!(strip("\u{1b}]8;;https://x\u{1b}\\link\u{1b}]8;;\u{1b}\\"), "link");
        assert_eq!(strip("plain"), "plain");
    }
}
