//! The cold scrollback store: lines that left the emulator's hot region, packed as text (with
//! their SGR styles) in blocks of 256 lines, each block `lz4`-compressed once full. Budgets are
//! in bytes; the oldest blocks go first. Cold blocks stay in RAM.

use crate::terminal::HarvestedRow;
use ctx_store::record::event_line;
use std::collections::VecDeque;

pub const BLOCK_LINES: usize = 256;

#[derive(Debug)]
struct Block {
    compressed: Vec<u8>,
    lines: usize,
}

#[derive(Debug, Default)]
pub struct ColdStore {
    blocks: VecDeque<Block>,
    open: Vec<String>,
    open_bytes: usize,
    /// A soft-wrapped line still being joined (and its event JSON, if it's an event line).
    partial: Option<(String, Option<String>)>,
    /// Lines dropped to stay within budget.
    pub dropped_lines: u64,
    compressed_bytes: usize,
}

impl ColdStore {
    /// Add a harvested row; soft-wrapped rows are joined into one logical line.
    pub fn push_row(&mut self, row: &HarvestedRow) {
        let (mut text, event) = self.partial.take().unwrap_or_default();
        text.push_str(&row.text);
        let event = event.or_else(|| row.event.clone());
        if row.wraps {
            self.partial = Some((text, event));
            return;
        }
        let line = match event {
            Some(json) => event_line(&json, &text),
            None => text,
        };
        self.push_line(line);
    }

    pub fn push_line(&mut self, line: String) {
        self.open_bytes += line.len() + 1;
        self.open.push(line);
        if self.open.len() >= BLOCK_LINES {
            let joined = self.open.join("\n");
            let compressed = lz4_flex::block::compress_prepend_size(joined.as_bytes());
            self.compressed_bytes += compressed.len();
            self.blocks.push_back(Block { compressed, lines: self.open.len() });
            self.open.clear();
            self.open_bytes = 0;
        }
    }

    /// Lines held, oldest first.
    pub fn len(&self) -> usize {
        self.blocks.iter().map(|b| b.lines).sum::<usize>() + self.open.len() + usize::from(self.partial.is_some())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Bytes held in memory.
    pub fn bytes(&self) -> usize {
        self.compressed_bytes + self.open_bytes + self.partial.as_ref().map(|p| p.0.len()).unwrap_or(0)
    }

    /// Drop the oldest block. Returns the bytes freed.
    pub fn drop_oldest(&mut self) -> usize {
        match self.blocks.pop_front() {
            Some(b) => {
                self.compressed_bytes -= b.compressed.len();
                self.dropped_lines += b.lines as u64;
                b.compressed.len()
            }
            None => 0,
        }
    }

    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    /// Keep the store within `max_bytes` by dropping the oldest blocks.
    pub fn enforce(&mut self, max_bytes: usize) {
        while self.bytes() > max_bytes && !self.blocks.is_empty() {
            self.drop_oldest();
        }
    }

    fn block_lines(b: &Block) -> Vec<String> {
        match lz4_flex::block::decompress_size_prepended(&b.compressed) {
            Ok(raw) => String::from_utf8_lossy(&raw).split('\n').map(str::to_string).collect(),
            Err(_) => vec![String::new(); b.lines],
        }
    }

    /// The last `n` lines (fewer if there aren't that many), oldest first.
    pub fn last(&self, n: usize) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        if let Some((p, e)) = &self.partial {
            out.push(match e {
                Some(json) => event_line(json, p),
                None => p.clone(),
            });
        }
        for l in self.open.iter().rev() {
            if out.len() >= n {
                break;
            }
            out.push(l.clone());
        }
        for b in self.blocks.iter().rev() {
            if out.len() >= n {
                break;
            }
            let lines = Self::block_lines(b);
            for l in lines.into_iter().rev() {
                if out.len() >= n {
                    break;
                }
                out.push(l);
            }
        }
        out.reverse();
        out
    }

    /// Lines `a..b` counted from the oldest line held (decompressing only the blocks needed).
    pub fn range(&self, a: usize, b: usize) -> Vec<String> {
        let mut out = Vec::new();
        let mut at = 0usize;
        for blk in &self.blocks {
            let end = at + blk.lines;
            if end > a && at < b {
                let lines = Self::block_lines(blk);
                for (i, l) in lines.into_iter().enumerate() {
                    if (a..b).contains(&(at + i)) {
                        out.push(l);
                    }
                }
            }
            at = end;
            if at >= b {
                return out;
            }
        }
        for l in &self.open {
            if (a..b).contains(&at) {
                out.push(l.clone());
            }
            at += 1;
        }
        if let Some((p, e)) = &self.partial
            && (a..b).contains(&at)
        {
            out.push(match e {
                Some(json) => event_line(json, p),
                None => p.clone(),
            });
        }
        out
    }

    /// Every line, oldest first.
    pub fn all(&self) -> Vec<String> {
        self.last(usize::MAX)
    }

    /// Clear everything (`ctx clear`).
    pub fn clear(&mut self) {
        *self = ColdStore::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(t: &str, wraps: bool) -> HarvestedRow {
        HarvestedRow { text: t.into(), wraps, event: None, y: 0 }
    }

    #[test]
    fn stores_and_returns_lines() {
        let mut c = ColdStore::default();
        for i in 0..1024 {
            c.push_row(&row(&format!("line {i} with some typical output text"), false));
        }
        assert_eq!(c.len(), 1024);
        assert_eq!(
            c.last(3),
            vec![
                "line 1021 with some typical output text",
                "line 1022 with some typical output text",
                "line 1023 with some typical output text"
            ]
        );
        assert_eq!(c.all().len(), 1024);
        assert_eq!(c.all()[0], "line 0 with some typical output text");
        // Compression: typical output shrinks several times.
        let raw: usize = (0..1024).map(|i| format!("line {i} with some typical output text").len() + 1).sum();
        assert!(c.bytes() * 3 < raw, "{} vs {raw}", c.bytes());
    }

    #[test]
    fn range_spans_blocks() {
        let mut c = ColdStore::default();
        for i in 0..600 {
            c.push_line(format!("l{i}"));
        }
        let r = c.range(250, 260);
        assert_eq!(r.len(), 10);
        assert_eq!(r[0], "l250");
        assert_eq!(r[9], "l259");
        assert_eq!(c.range(590, 700).len(), 10);
        c.drop_oldest();
        assert_eq!(c.range(0, 1), vec!["l256".to_string()]);
    }

    #[test]
    fn joins_wrapped_rows_and_marks_events() {
        let mut c = ColdStore::default();
        c.push_row(&row("abc", true));
        c.push_row(&row("def", false));
        c.push_row(&HarvestedRow { text: "[moved]".into(), wraps: false, event: Some("{}".into()), y: 0 });
        assert_eq!(c.all(), vec!["abcdef".to_string(), event_line("{}", "[moved]")]);
    }

    #[test]
    fn budget_drops_oldest_blocks() {
        let mut c = ColdStore::default();
        for i in 0..5000 {
            c.push_row(&row(&format!("{i:08} {}", "x".repeat(i % 50)), false));
        }
        let before = c.len();
        c.enforce(10_000);
        assert!(c.bytes() <= 10_000 || c.block_count() == 0);
        assert!(c.len() < before);
        assert_eq!(c.dropped_lines as usize, before - c.len());
        // The newest lines survive.
        assert!(c.last(1)[0].starts_with("00004999"));
    }
}
