//! The virtual screen: a grid of cells plus the cursor, updated by the VT parser.
//!
//! Rows that scroll off the top of the main screen are kept as scrollback history.
//! The alternate screen (used by vim, less, ...) is a separate grid that is discarded
//! when the program leaves it, and the main screen is restored.

use std::collections::VecDeque;

use unicode_width::UnicodeWidthChar;
use vte::{Params, Perform};

/// A colour as the program asked for it; the GUI maps it to real pixels.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Color {
    /// The terminal's default foreground or background.
    Default,
    /// Entry in the 256-colour palette (0-15 are the basic/bright ANSI colours).
    Indexed(u8),
    Rgb(u8, u8, u8),
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Cell {
    /// The character shown. `'\0'` marks the second column of a wide character.
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
    pub reverse: bool,
    pub underline: bool,
    pub strikethrough: bool,
}

impl Default for Cell {
    fn default() -> Self {
        Cell {
            ch: ' ',
            fg: Color::Default,
            bg: Color::Default,
            bold: false,
            reverse: false,
            underline: false,
            strikethrough: false,
        }
    }
}

/// Scrollback rows kept when the config does not say otherwise.
pub const DEFAULT_HISTORY: usize = 10_000;

/// The main screen, saved while the alternate screen is active.
struct SavedMain {
    cells: Vec<Cell>,
    cx: usize,
    cy: usize,
}

pub struct Screen {
    pub cols: usize,
    pub rows: usize,
    cells: Vec<Cell>,
    cx: usize,
    cy: usize,
    saved: (usize, usize),
    /// Set after writing into the last column; the next printed char wraps.
    wrap_pending: bool,
    /// Current "pen": style applied to newly printed characters (set by SGR).
    pen: Cell,
    /// Bytes the terminal must send back to the shell (answers to queries such as
    /// device attributes and cursor position). Drained by the event loop.
    replies: Vec<u8>,
    /// Rows that scrolled off the top of the main screen, oldest first.
    history: VecDeque<Vec<Cell>>,
    history_limit: usize,
    /// How many history rows the view is scrolled up by (0 = showing the live screen).
    view_offset: usize,
    /// The main screen while the alternate screen is active.
    alt_saved: Option<SavedMain>,
}

impl Screen {
    pub fn new(cols: usize, rows: usize) -> Self {
        let (cols, rows) = (cols.max(1), rows.max(1));
        Screen {
            cols,
            rows,
            cells: vec![Cell::default(); cols * rows],
            cx: 0,
            cy: 0,
            saved: (0, 0),
            wrap_pending: false,
            pen: Cell::default(),
            replies: Vec::new(),
            history: VecDeque::new(),
            history_limit: DEFAULT_HISTORY,
            view_offset: 0,
            alt_saved: None,
        }
    }

    /// Returns (and clears) any bytes the shell should receive as replies to queries.
    pub fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.replies)
    }

    /// Cursor position as (column, row) on the live screen.
    pub fn cursor(&self) -> (usize, usize) {
        (self.cx, self.cy)
    }

    /// Set how many scrollback rows to keep. 0 disables scrollback.
    pub fn set_history_limit(&mut self, limit: usize) {
        self.history_limit = limit;
        while self.history.len() > limit {
            self.history.pop_front();
        }
        self.view_offset = self.view_offset.min(self.history.len());
    }

    pub fn in_alt_screen(&self) -> bool {
        self.alt_saved.is_some()
    }

    /// Scroll the view up (positive) or down (negative) through the history.
    /// Has no effect on the alternate screen.
    pub fn scroll_view(&mut self, lines: isize) {
        if self.in_alt_screen() {
            return;
        }
        let next = self.view_offset as isize + lines;
        self.view_offset = next.clamp(0, self.history.len() as isize) as usize;
    }

    /// Jump back to the live screen.
    pub fn reset_view(&mut self) {
        self.view_offset = 0;
    }

    /// Whether the view shows the live screen (not scrolled back).
    pub fn at_bottom(&self) -> bool {
        self.view_offset == 0
    }

    /// The logical row shown at the top of the view. Logical rows count the history
    /// first, then the live screen, so a selection stays put while the view scrolls.
    pub fn first_visible_row(&self) -> usize {
        self.history.len() - self.view_offset
    }

    /// The cell shown at column `x`, row `y` of the view.
    pub fn visible_cell(&self, x: usize, y: usize) -> Cell {
        self.cell_at(self.first_visible_row() + y, x)
    }

    /// The cell at a logical row and column.
    pub fn cell_at(&self, row: usize, col: usize) -> Cell {
        if row < self.history.len() {
            self.history[row].get(col).copied().unwrap_or_default()
        } else {
            let y = row - self.history.len();
            if y < self.rows && col < self.cols {
                self.cells[y * self.cols + col]
            } else {
                Cell::default()
            }
        }
    }

    /// The plain text between two logical positions (inclusive), one line per row.
    /// Trailing spaces on each line are dropped. The two positions may come in either order.
    pub fn text_between(&self, a: (usize, usize), b: (usize, usize)) -> String {
        let (start, end) = if a <= b { (a, b) } else { (b, a) };
        let mut lines = Vec::new();
        for row in start.0..=end.0 {
            let first = if row == start.0 { start.1 } else { 0 };
            let last = if row == end.0 { end.1 } else { self.cols.saturating_sub(1) };
            let mut line = String::new();
            for col in first..=last {
                let c = self.cell_at(row, col);
                if c.ch != '\0' {
                    line.push(c.ch);
                }
            }
            lines.push(line.trim_end().to_string());
        }
        lines.join("\n")
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        let (cols, rows) = (cols.max(1), rows.max(1));
        self.cells = reflow(&self.cells, self.cols, self.rows, cols, rows);
        if let Some(saved) = self.alt_saved.as_mut() {
            saved.cells = reflow(&saved.cells, self.cols, self.rows, cols, rows);
            saved.cx = saved.cx.min(cols - 1);
            saved.cy = saved.cy.min(rows - 1);
        }
        self.cols = cols;
        self.rows = rows;
        self.cx = self.cx.min(cols - 1);
        self.cy = self.cy.min(rows - 1);
        self.wrap_pending = false;
    }

    fn blank(&self) -> Cell {
        // Erased cells take the current background colour, like real terminals do.
        Cell { ch: ' ', bg: self.pen.bg, ..Cell::default() }
    }

    fn fill(&mut self, range: std::ops::Range<usize>) {
        let blank = self.blank();
        for c in &mut self.cells[range] {
            *c = blank;
        }
    }

    fn scroll_up(&mut self) {
        let top: Vec<Cell> = self.cells[..self.cols].to_vec();
        self.cells.drain(0..self.cols);
        let blank = self.blank();
        self.cells.extend(std::iter::repeat(blank).take(self.cols));

        // The alternate screen has no scrollback.
        if !self.in_alt_screen() && self.history_limit > 0 {
            self.history.push_back(top);
            if self.history.len() > self.history_limit {
                self.history.pop_front();
            }
            // Keep a scrolled-back view showing the same text as new rows arrive.
            if self.view_offset > 0 {
                self.view_offset = (self.view_offset + 1).min(self.history.len());
            }
        }
    }

    fn newline(&mut self) {
        if self.cy + 1 >= self.rows {
            self.scroll_up();
        } else {
            self.cy += 1;
        }
    }

    fn put_char(&mut self, ch: char) {
        // Wide characters (CJK, most emoji) take two columns; zero-width ones are dropped.
        let width = match UnicodeWidthChar::width(ch) {
            Some(0) | None => return,
            Some(w) => w.min(self.cols).min(2),
        };
        if self.wrap_pending || self.cx + width > self.cols {
            self.cx = 0;
            self.newline();
            self.wrap_pending = false;
        }
        let idx = self.cy * self.cols + self.cx;
        self.cells[idx] = Cell { ch, ..self.pen };
        if width == 2 {
            self.cells[idx + 1] = Cell { ch: '\0', ..self.pen };
        }
        let next = self.cx + width;
        if next < self.cols {
            self.cx = next;
        } else {
            self.cx = self.cols - 1;
            self.wrap_pending = true;
        }
    }

    fn enter_alt_screen(&mut self) {
        if self.in_alt_screen() {
            return;
        }
        let main = std::mem::replace(&mut self.cells, vec![Cell::default(); self.cols * self.rows]);
        self.alt_saved = Some(SavedMain { cells: main, cx: self.cx, cy: self.cy });
        self.cx = 0;
        self.cy = 0;
        self.view_offset = 0;
    }

    fn leave_alt_screen(&mut self) {
        if let Some(saved) = self.alt_saved.take() {
            self.cells = saved.cells;
            self.cx = saved.cx;
            self.cy = saved.cy;
        }
    }

    fn sgr(&mut self, params: &Params) {
        let vals: Vec<u16> = params.iter().flat_map(|s| s.iter().copied()).collect();
        if vals.is_empty() {
            self.pen = Cell::default();
            return;
        }
        let mut i = 0;
        while i < vals.len() {
            let v = vals[i];
            match v {
                0 => self.pen = Cell::default(),
                1 => self.pen.bold = true,
                22 => self.pen.bold = false,
                4 => self.pen.underline = true,
                24 => self.pen.underline = false,
                7 => self.pen.reverse = true,
                27 => self.pen.reverse = false,
                9 => self.pen.strikethrough = true,
                29 => self.pen.strikethrough = false,
                30..=37 => self.pen.fg = basic_color((v - 30) as u8, false),
                39 => self.pen.fg = Color::Default,
                40..=47 => self.pen.bg = basic_color((v - 40) as u8, false),
                49 => self.pen.bg = Color::Default,
                90..=97 => self.pen.fg = basic_color((v - 90) as u8, true),
                100..=107 => self.pen.bg = basic_color((v - 100) as u8, true),
                38 | 48 => {
                    let color = match vals.get(i + 1) {
                        Some(5) => {
                            i += 2;
                            vals.get(i).map(|&n| Color::Indexed(n as u8))
                        }
                        Some(2) => {
                            let rgb = (vals.get(i + 2), vals.get(i + 3), vals.get(i + 4));
                            i += 4;
                            match rgb {
                                (Some(&r), Some(&g), Some(&b)) => {
                                    Some(Color::Rgb(r as u8, g as u8, b as u8))
                                }
                                _ => None,
                            }
                        }
                        _ => None,
                    };
                    if let Some(c) = color {
                        if v == 38 {
                            self.pen.fg = c;
                        } else {
                            self.pen.bg = c;
                        }
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }
}

/// Copy a grid into a new size, keeping the cells that still fit.
fn reflow(old: &[Cell], old_cols: usize, old_rows: usize, cols: usize, rows: usize) -> Vec<Cell> {
    let mut new = vec![Cell::default(); cols * rows];
    for y in 0..rows.min(old_rows) {
        for x in 0..cols.min(old_cols) {
            new[y * cols + x] = old[y * old_cols + x];
        }
    }
    new
}

fn basic_color(n: u8, bright: bool) -> Color {
    Color::Indexed(if bright { n + 8 } else { n })
}

impl Perform for Screen {
    fn print(&mut self, c: char) {
        self.put_char(c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' | 0x0b | 0x0c => {
                self.newline();
                self.wrap_pending = false;
            }
            b'\r' => {
                self.cx = 0;
                self.wrap_pending = false;
            }
            0x08 => self.cx = self.cx.saturating_sub(1), // backspace
            b'\t' => self.cx = (((self.cx / 8) + 1) * 8).min(self.cols - 1),
            _ => {} // BEL and other C0 controls are ignored
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        let p: Vec<usize> = params.iter().map(|s| s.first().copied().unwrap_or(0) as usize).collect();
        // Parameter with a default; zero or missing means the default.
        let arg = |i: usize, default: usize| p.get(i).copied().filter(|&v| v != 0).unwrap_or(default);
        self.wrap_pending = false;

        match action {
            'A' => self.cy = self.cy.saturating_sub(arg(0, 1)),
            'B' => self.cy = (self.cy + arg(0, 1)).min(self.rows - 1),
            'C' => self.cx = (self.cx + arg(0, 1)).min(self.cols - 1),
            'D' => self.cx = self.cx.saturating_sub(arg(0, 1)),
            'G' => self.cx = (arg(0, 1) - 1).min(self.cols - 1),
            'H' | 'f' => {
                self.cy = (arg(0, 1) - 1).min(self.rows - 1);
                self.cx = (arg(1, 1) - 1).min(self.cols - 1);
            }
            'J' => {
                let start = self.cy * self.cols + self.cx;
                let len = self.cells.len();
                match p.first().copied().unwrap_or(0) {
                    0 => self.fill(start..len),
                    1 => self.fill(0..start + 1),
                    _ => self.fill(0..len),
                }
            }
            'K' => {
                let row = self.cy * self.cols;
                match p.first().copied().unwrap_or(0) {
                    0 => self.fill(row + self.cx..row + self.cols),
                    1 => self.fill(row..row + self.cx + 1),
                    _ => self.fill(row..row + self.cols),
                }
            }
            'm' => self.sgr(params),
            'c' => {
                // Device attributes. Shells such as fish send this at startup and wait
                // for an answer; reply as a VT220 with ANSI colour support.
                if intermediates.is_empty() {
                    self.replies.extend_from_slice(b"\x1b[?62;22c");
                } else if intermediates == [b'>'] {
                    // Secondary attributes: terminal type 1 (VT220), version 0.
                    self.replies.extend_from_slice(b"\x1b[>1;0;0c");
                }
            }
            'n' => match p.first().copied().unwrap_or(0) {
                5 => self.replies.extend_from_slice(b"\x1b[0n"), // device status: OK
                6 => {
                    // Cursor position report (1-based row;col).
                    let report = format!("\x1b[{};{}R", self.cy + 1, self.cx + 1);
                    self.replies.extend_from_slice(report.as_bytes());
                }
                _ => {}
            },
            'h' | 'l' if intermediates == [b'?'] && p.contains(&1049) => {
                if action == 'h' {
                    self.enter_alt_screen();
                } else {
                    self.leave_alt_screen();
                }
            }
            _ => {} // other modes/sequences are accepted and ignored
        }
    }

    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, byte: u8) {
        match byte {
            b'7' => self.saved = (self.cx, self.cy),
            b'8' => (self.cx, self.cy) = self.saved,
            b'M' => self.cy = self.cy.saturating_sub(1), // reverse index
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(s: &mut Screen, bytes: &[u8]) {
        let mut p = vte::Parser::new();
        for &b in bytes {
            p.advance(s, b);
        }
    }

    fn row(s: &Screen, y: usize) -> String {
        (0..s.cols).map(|x| s.visible_cell(x, y).ch).collect()
    }

    #[test]
    fn prints_and_wraps() {
        let mut s = Screen::new(4, 3);
        feed(&mut s, b"abcdef");
        assert_eq!(row(&s, 0), "abcd");
        assert_eq!(row(&s, 1), "ef  ");
    }

    #[test]
    fn newline_scrolls_at_bottom() {
        let mut s = Screen::new(3, 2);
        feed(&mut s, b"aa\r\nbb\r\ncc");
        assert_eq!(row(&s, 0), "bb ");
        assert_eq!(row(&s, 1), "cc ");
    }

    #[test]
    fn cursor_position_and_erase() {
        let mut s = Screen::new(5, 3);
        feed(&mut s, b"hello\x1b[1;1H\x1b[2Kx");
        assert_eq!(row(&s, 0), "x    ");
    }

    #[test]
    fn colours_are_applied() {
        let mut s = Screen::new(4, 1);
        feed(&mut s, b"\x1b[31;1mR\x1b[0mN");
        assert_eq!(s.visible_cell(0, 0).fg, Color::Indexed(1));
        assert!(s.visible_cell(0, 0).bold);
        assert_eq!(s.visible_cell(1, 0).fg, Color::Default);
        assert!(!s.visible_cell(1, 0).bold);
    }

    #[test]
    fn resize_keeps_content() {
        let mut s = Screen::new(3, 2);
        feed(&mut s, b"abc");
        s.resize(5, 3);
        assert_eq!(row(&s, 0), "abc  ");
        assert_eq!((s.cols, s.rows), (5, 3));
    }

    #[test]
    fn answers_device_attributes_and_cursor_position() {
        let mut s = Screen::new(10, 5);
        feed(&mut s, b"\x1b[c");
        assert_eq!(s.take_replies(), b"\x1b[?62;22c");
        feed(&mut s, b"ab\r\ncd\x1b[6n");
        assert_eq!(s.take_replies(), b"\x1b[2;3R");
        feed(&mut s, b"\x1b[5n");
        assert_eq!(s.take_replies(), b"\x1b[0n");
        assert!(s.take_replies().is_empty());
    }

    #[test]
    fn scrollback_keeps_lines_and_can_be_viewed() {
        let mut s = Screen::new(3, 2);
        feed(&mut s, b"aa\r\nbb\r\ncc");
        // "aa" scrolled off; the live screen shows bb / cc.
        assert_eq!(s.history.len(), 1);
        s.scroll_view(1);
        assert_eq!(row(&s, 0), "aa ");
        assert_eq!(row(&s, 1), "bb ");
        s.scroll_view(10); // clamped at the top of history
        assert_eq!(s.view_offset, 1);
        s.reset_view();
        assert_eq!(row(&s, 0), "bb ");
    }

    #[test]
    fn history_limit_is_enforced() {
        let mut s = Screen::new(3, 1);
        s.set_history_limit(2);
        feed(&mut s, b"a\nb\nc\nd\n");
        assert_eq!(s.history.len(), 2);
    }

    #[test]
    fn alternate_screen_is_separate_and_restored() {
        let mut s = Screen::new(4, 2);
        feed(&mut s, b"main");
        feed(&mut s, b"\x1b[?1049h");
        assert!(s.in_alt_screen());
        assert_eq!(row(&s, 0), "    ");
        feed(&mut s, b"vim!");
        assert_eq!(row(&s, 0), "vim!");
        feed(&mut s, b"\x1b[?1049l");
        assert!(!s.in_alt_screen());
        assert_eq!(row(&s, 0), "main");
    }

    #[test]
    fn alternate_screen_has_no_scrollback() {
        let mut s = Screen::new(3, 2);
        feed(&mut s, b"\x1b[?1049haa\r\nbb\r\ncc");
        assert_eq!(s.history.len(), 0);
    }

    #[test]
    fn wide_characters_take_two_columns() {
        let mut s = Screen::new(5, 1);
        feed(&mut s, "a漢b".as_bytes());
        assert_eq!(s.visible_cell(1, 0).ch, '漢');
        assert_eq!(s.visible_cell(2, 0).ch, '\0');
        assert_eq!(s.visible_cell(3, 0).ch, 'b');
        assert_eq!(s.text_between((0, 0), (0, 4)), "a漢b");
    }

    #[test]
    fn underline_and_strikethrough() {
        let mut s = Screen::new(2, 1);
        feed(&mut s, b"\x1b[4;9mU\x1b[24;29mN");
        assert!(s.visible_cell(0, 0).underline && s.visible_cell(0, 0).strikethrough);
        assert!(!s.visible_cell(1, 0).underline && !s.visible_cell(1, 0).strikethrough);
    }

    #[test]
    fn text_between_spans_rows_and_trims() {
        let mut s = Screen::new(4, 2);
        feed(&mut s, b"ab  \r\ncdef");
        assert_eq!(s.text_between((0, 0), (1, 2)), "ab\ncde");
        assert_eq!(s.text_between((1, 2), (0, 0)), "ab\ncde");
    }
}
