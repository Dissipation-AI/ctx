//! Screen updates for CTX.app: only the rows that changed, as runs of text with one style each.

use crate::tab::Tab;
use ctx_protocol::{CellStyle, CursorShape, RowUpdate, ScreenUpdate, StyledRun};
use ghostty_vt::RenderState;
use ghostty_vt::render::{CellIterator, CursorVisualStyle, Dirty, RowIterator};
use ghostty_vt::screen::CellWide;
use ghostty_vt::style::Underline;

pub struct Renderer {
    /// Send every row on the next update (colours changed).
    pub force_full: bool,
    /// The cursor as last sent: moving only the cursor (arrow keys at a prompt) changes no row,
    /// but still needs an update.
    last_cursor: Option<(Option<[u16; 2]>, bool, CursorShape, bool)>,
    state: RenderState,
    rows: RowIterator,
    cells: CellIterator,
}

impl std::fmt::Debug for Renderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Renderer")
    }
}

impl Renderer {
    pub fn new() -> Option<Renderer> {
        Some(Renderer {
            force_full: false,
            last_cursor: None,
            state: RenderState::new().ok()?,
            rows: RowIterator::new().ok()?,
            cells: CellIterator::new().ok()?,
        })
    }
}

/// The rows of `tab` that changed since the last update (all rows when `full`).
pub fn update(tab: &mut Tab, full: bool) -> Option<ScreenUpdate> {
    if tab.render.is_none() {
        tab.render = Renderer::new();
    }
    let title = tab.title.clone();
    let top = tab.term.active_top();
    let event_rows: Vec<u16> = if tab.term.full_screen() {
        Vec::new()
    } else {
        (0..tab.rows).filter(|y| tab.term.event_at(top + *y as u64).is_some()).collect()
    };
    let number = tab.number;
    let full_screen = tab.term.full_screen();
    let mouse_tracking = tab.term.mouse_tracking();
    let at_prompt = tab.at_prompt && tab.running.is_empty();
    let history_first = tab.cold.dropped_lines;
    let history_end = history_first + tab.cold.len() as u64;
    let term = tab.term.ghostty()?;
    let r = tab.render.as_mut()?;
    let full = full || std::mem::take(&mut r.force_full);
    let snapshot = r.state.update(term).ok()?;
    let dirty = snapshot.dirty().ok()?;
    let cursor_now = snapshot.cursor().ok()?;
    let cursor_key = (
        cursor_now.viewport.map(|v| [v.x, v.y]),
        cursor_now.visible,
        shape_of(cursor_now.visual_style),
        cursor_now.blinking,
    );
    if dirty == Dirty::Clean && !full && r.last_cursor == Some(cursor_key) {
        return None;
    }
    r.last_cursor = Some(cursor_key);
    let columns = snapshot.cols().ok()?;
    let rows = snapshot.rows().ok()?;
    let colors = snapshot.colors().ok()?;
    let cursor = snapshot.cursor().ok()?;
    let mut changed = Vec::new();
    {
        let mut iter = r.rows.update(&snapshot).ok()?;
        let mut y: u16 = 0;
        loop {
            let (row_y, row) = if full || dirty == Dirty::Full {
                match iter.next() {
                    Some(row) => {
                        let ry = y;
                        y += 1;
                        (ry, row)
                    }
                    None => break,
                }
            } else {
                match iter.next_dirty() {
                    Some((ry, row)) => (ry, row),
                    None => break,
                }
            };
            let mut runs: Vec<StyledRun> = Vec::new();
            if let Ok(mut cells) = r.cells.update(row) {
                while let Some(cell) = cells.next() {
                    let wide = cell.raw_cell().ok().and_then(|c| c.wide().ok());
                    if matches!(wide, Some(CellWide::SpacerTail) | Some(CellWide::SpacerHead)) {
                        continue;
                    }
                    let text: String = match cell.graphemes() {
                        Ok(g) if !g.is_empty() => g.into_iter().collect(),
                        _ => " ".into(),
                    };
                    let mut style = CellStyle::default();
                    if let Ok(s) = cell.style() {
                        let mut flags = 0u16;
                        if s.bold {
                            flags |= 1;
                        }
                        if s.italic {
                            flags |= 2;
                        }
                        if s.faint {
                            flags |= 4;
                        }
                        if s.underline != Underline::None {
                            flags |= 8;
                        }
                        if s.inverse {
                            flags |= 16;
                        }
                        if s.strikethrough {
                            flags |= 32;
                        }
                        if s.invisible {
                            flags |= 64;
                        }
                        if s.blink {
                            flags |= 128;
                        }
                        style.flags = flags;
                    }
                    if matches!(wide, Some(CellWide::Wide)) {
                        style.flags |= 256;
                    }
                    style.foreground = cell.fg_color().ok().flatten().map(|c| [c.r, c.g, c.b]);
                    style.background = cell.bg_color().ok().flatten().map(|c| [c.r, c.g, c.b]);
                    let width: u16 = if matches!(wide, Some(CellWide::Wide)) { 2 } else { 1 };
                    match runs.last_mut() {
                        Some(last) if last.style == style => {
                            let at = last.text.len() as u16;
                            last.text.push_str(&text);
                            last.cells.push([at, width]);
                        }
                        _ => runs.push(StyledRun { text, style, cells: vec![[0, width]] }),
                    }
                }
            }
            // Trailing default-styled spaces carry nothing.
            if let Some(last) = runs.last_mut()
                && last.style == CellStyle::default()
            {
                let trimmed = last.text.trim_end().len();
                last.text.truncate(trimmed);
                last.cells.retain(|c| (c[0] as usize) < trimmed);
                if last.text.is_empty() {
                    runs.pop();
                }
            }
            for r in &mut runs {
                drop_plain_cells(r);
            }
            let wraps = row.raw_row().ok().and_then(|r| r.is_wrapped().ok()).unwrap_or(false);
            let _ = row.set_dirty(false);
            changed.push(RowUpdate { row: row_y, runs, event: event_rows.contains(&row_y), wraps });
        }
    }
    let _ = snapshot.set_dirty(Dirty::Clean);
    Some(ScreenUpdate {
        tab: number,
        columns,
        rows,
        changed,
        cursor: cursor.viewport.map(|v| [v.x, v.y]),
        cursor_visible: cursor.visible,
        title,
        full_screen,
        default_foreground: [colors.foreground.r, colors.foreground.g, colors.foreground.b],
        default_background: [colors.background.r, colors.background.g, colors.background.b],
        cursor_shape: shape_of(cursor.visual_style),
        cursor_blink: cursor.blinking,
        mouse_tracking,
        at_prompt,
        history_first,
        history_end,
    })
}

fn shape_of(v: CursorVisualStyle) -> CursorShape {
    match v {
        CursorVisualStyle::Bar => CursorShape::Bar,
        CursorVisualStyle::Underline => CursorShape::Underline,
        CursorVisualStyle::BlockHollow => CursorShape::Hollow,
        _ => CursorShape::Block,
    }
}

/// Plain runs (every character one cell of the run's width, as ASCII always is) don't need their
/// cells spelled out.
fn drop_plain_cells(r: &mut StyledRun) {
    let w = if r.style.flags & 256 != 0 { 2 } else { 1 };
    let plain = r.text.is_ascii() && r.cells.len() == r.text.len() && r.cells.iter().all(|c| c[1] == w);
    if plain {
        r.cells.clear();
    }
}

/// The cells of runs made from stored text (scrollback lines), worked out with the terminal's
/// own grapheme and width rules, so they line up exactly as the screen did.
pub fn measure_cells(runs: &mut [StyledRun]) {
    for r in runs.iter_mut() {
        if r.text.is_ascii() {
            r.cells.clear();
            continue;
        }
        let chars: Vec<(usize, char)> = r.text.char_indices().collect();
        let only: Vec<char> = chars.iter().map(|c| c.1).collect();
        let mut cells = Vec::new();
        let mut i = 0;
        while i < only.len() {
            let (consumed, width) = ghostty_vt::unicode::grapheme_width(&only[i..]);
            if consumed == 0 {
                break;
            }
            // A zero-width cluster joins the cell before it.
            if width > 0 || cells.is_empty() {
                cells.push([chars[i].0 as u16, width.max(1) as u16]);
            }
            i += consumed;
        }
        r.cells = cells;
        drop_plain_cells(r);
    }
}

#[cfg(test)]
mod cell_tests {
    use super::*;

    fn run(t: &str) -> StyledRun {
        StyledRun { text: t.into(), ..Default::default() }
    }

    #[test]
    fn stored_lines_get_the_terminals_cells() {
        let mut rs = vec![run("ls -la"), run("a中b"), run("👍🏽x"), run("e\u{301}!")];
        measure_cells(&mut rs);
        assert!(rs[0].cells.is_empty(), "ASCII needs no cells");
        assert_eq!(rs[1].cells, vec![[0, 1], [1, 2], [4, 1]]);
        // An emoji with a skin tone is one cell, two wide.
        assert_eq!(rs[2].cells, vec![[0, 2], [8, 1]]);
        // A combining accent stays with its letter.
        assert_eq!(rs[3].cells, vec![[0, 1], [3, 1]]);
    }
}
