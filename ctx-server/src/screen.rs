//! Screen updates for CTX.app: only the rows that changed, as runs of text with one style each.

use crate::tab::Tab;
use ctx_protocol::{CellStyle, RowUpdate, ScreenUpdate, StyledRun};
use ghostty_vt::RenderState;
use ghostty_vt::render::{CellIterator, Dirty, RowIterator};
use ghostty_vt::screen::CellWide;
use ghostty_vt::style::Underline;

pub struct Renderer {
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
    let number = tab.number;
    let full_screen = tab.term.full_screen();
    let term = tab.term.ghostty()?;
    let r = tab.render.as_mut()?;
    let snapshot = r.state.update(term).ok()?;
    let dirty = snapshot.dirty().ok()?;
    if dirty == Dirty::Clean && !full {
        return None;
    }
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
                    match runs.last_mut() {
                        Some(last) if last.style == style => last.text.push_str(&text),
                        _ => runs.push(StyledRun { text, style }),
                    }
                }
            }
            // Trailing default-styled spaces carry nothing.
            if let Some(last) = runs.last_mut()
                && last.style == CellStyle::default()
            {
                let trimmed = last.text.trim_end().len();
                last.text.truncate(trimmed);
                if last.text.is_empty() {
                    runs.pop();
                }
            }
            let _ = row.set_dirty(false);
            changed.push(RowUpdate { row: row_y, runs, event: false });
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
    })
}
