//! The virtual screen: a grid of cells plus the cursor, updated by the VT parser.
//!
//! Rows that scroll off the top of the main screen are kept as scrollback history.
//! The alternate screen (used by vim, less, ...) is a separate grid that is discarded
//! when the program leaves it, and the main screen is restored. A scroll region
//! (`CSI r`) confines scrolling to a band of rows.

use std::collections::VecDeque;

use unicode_width::UnicodeWidthChar;
use vte::{Params, Perform};

/// How mouse reports are encoded for the program (set by DECSET 1005, 1006 and 1015).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MouseEncoding {
    /// `ESC [ M` followed by three bytes, each offset by 32. Coordinates are limited to 223.
    Legacy,
    /// As legacy, but coordinates are UTF-8 encoded (mode 1005).
    Utf8,
    /// `ESC [ < b ; x ; y M` (press) or `m` (release) (mode 1006).
    Sgr,
    /// `ESC [ b ; x ; y M` with decimal numbers (mode 1015).
    Urxvt,
}

/// A colour as the program asked for it; the renderer maps it to real pixels.
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
    /// Combining marks drawn over `ch`. Up to two are kept.
    pub marks: [Option<char>; 2],
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub reverse: bool,
    pub underline: bool,
    pub strikethrough: bool,
}

impl Default for Cell {
    fn default() -> Self {
        Cell {
            ch: ' ',
            marks: [None; 2],
            fg: Color::Default,
            bg: Color::Default,
            bold: false,
            dim: false,
            italic: false,
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
    wrapped: Vec<bool>,
    cx: usize,
    cy: usize,
}

pub struct Screen {
    pub cols: usize,
    pub rows: usize,
    cells: Vec<Cell>,
    /// Per live row: true if the row continues on the next one (the text autowrapped).
    wrapped: Vec<bool>,
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
    /// The `wrapped` flag of each history row, in step with `history`.
    history_wrapped: VecDeque<bool>,
    history_limit: usize,
    /// How many history rows the view is scrolled up by (0 = showing the live screen).
    view_offset: usize,
    /// The main screen while the alternate screen is active.
    alt_saved: Option<SavedMain>,
    /// Top and bottom rows (inclusive) of the scroll region.
    scroll_top: usize,
    scroll_bottom: usize,
    /// Mouse tracking mode requested by the program: 0 (off), 1000, 1002 or 1003.
    mouse_tracking: u16,
    /// How mouse reports are encoded.
    mouse_encoding: MouseEncoding,
}

impl Screen {
    pub fn new(cols: usize, rows: usize) -> Self {
        let (cols, rows) = (cols.max(1), rows.max(1));
        Screen {
            cols,
            rows,
            cells: vec![Cell::default(); cols * rows],
            wrapped: vec![false; rows],
            cx: 0,
            cy: 0,
            saved: (0, 0),
            wrap_pending: false,
            pen: Cell::default(),
            replies: Vec::new(),
            history: VecDeque::new(),
            history_wrapped: VecDeque::new(),
            history_limit: DEFAULT_HISTORY,
            view_offset: 0,
            alt_saved: None,
            scroll_top: 0,
            scroll_bottom: rows - 1,
            mouse_tracking: 0,
            mouse_encoding: MouseEncoding::Legacy,
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
        self.trim_history();
        self.view_offset = self.view_offset.min(self.history.len());
    }

    fn trim_history(&mut self) {
        while self.history.len() > self.history_limit {
            self.history.pop_front();
            self.history_wrapped.pop_front();
        }
    }

    pub fn in_alt_screen(&self) -> bool {
        self.alt_saved.is_some()
    }

    /// The mouse tracking mode the program asked for (0 = none).
    pub fn mouse_tracking(&self) -> u16 {
        self.mouse_tracking
    }

    /// How mouse reports should be encoded.
    pub fn mouse_encoding(&self) -> MouseEncoding {
        self.mouse_encoding
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

    /// Number of logical rows: history plus the live screen.
    pub fn total_rows(&self) -> usize {
        self.history.len() + self.rows
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

    /// Whether a logical row continues onto the next one (it was wrapped, not ended).
    pub fn row_wrapped(&self, row: usize) -> bool {
        if row < self.history.len() {
            self.history_wrapped[row]
        } else {
            let y = row - self.history.len();
            y < self.rows && self.wrapped[y]
        }
    }

    /// The plain text between two logical positions (inclusive). Rows that were wrapped
    /// are joined without a newline; other rows end with one. Trailing spaces on a line
    /// are dropped. The two positions may come in either order.
    pub fn text_between(&self, a: (usize, usize), b: (usize, usize)) -> String {
        let (start, end) = if a <= b { (a, b) } else { (b, a) };
        let mut out = String::new();
        for row in start.0..=end.0 {
            let first = if row == start.0 { start.1 } else { 0 };
            let last = if row == end.0 { end.1 } else { self.cols.saturating_sub(1) };
            let mut line = String::new();
            for col in first..=last {
                let c = self.cell_at(row, col);
                if c.ch != '\0' {
                    line.push(c.ch);
                    for mark in c.marks.iter().flatten() {
                        line.push(*mark);
                    }
                }
            }
            if self.row_wrapped(row) {
                out.push_str(&line);
            } else {
                out.push_str(line.trim_end());
                if row != end.0 {
                    out.push('\n');
                }
            }
        }
        out
    }

    /// The text of a rectangular selection: the same columns on each row, one line per
    /// row, with trailing spaces dropped. The two positions may come in either order.
    pub fn block_text(&self, a: (usize, usize), b: (usize, usize)) -> String {
        let (r0, r1) = (a.0.min(b.0), a.0.max(b.0));
        let (c0, c1) = (a.1.min(b.1), a.1.max(b.1));
        let mut lines = Vec::new();
        for row in r0..=r1 {
            let mut line = String::new();
            for col in c0..=c1 {
                let c = self.cell_at(row, col);
                if c.ch != '\0' {
                    line.push(c.ch);
                    for mark in c.marks.iter().flatten() {
                        line.push(*mark);
                    }
                }
            }
            lines.push(line.trim_end().to_string());
        }
        lines.join("\n")
    }

    /// The span of the word at a logical position: a run of letters and digits, a run of
    /// spaces, or a run of other characters. Returned as inclusive (row, column) bounds.
    pub fn word_bounds(&self, row: usize, col: usize) -> ((usize, usize), (usize, usize)) {
        // Wide characters' second column counts as part of the character before it.
        let class = |c: char| match c {
            ' ' => 0,
            c if c.is_alphanumeric() || c == '_' || c == '\0' => 1,
            _ => 2,
        };
        let target = class(self.cell_at(row, col).ch);
        let mut start = col;
        while start > 0 && class(self.cell_at(row, start - 1).ch) == target {
            start -= 1;
        }
        let mut end = col;
        while end + 1 < self.cols && class(self.cell_at(row, end + 1).ch) == target {
            end += 1;
        }
        ((row, start), (row, end))
    }

    /// The span of the whole logical line at a row, including any rows it wraps onto.
    pub fn line_bounds(&self, row: usize) -> ((usize, usize), (usize, usize)) {
        let mut first = row;
        while first > 0 && self.row_wrapped(first - 1) {
            first -= 1;
        }
        let mut last = row;
        while self.row_wrapped(last) && last + 1 < self.total_rows() {
            last += 1;
        }
        ((first, 0), (last, self.cols - 1))
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        let (cols, rows) = (cols.max(1), rows.max(1));
        self.cells = reflow(&self.cells, self.cols, self.rows, cols, rows);
        self.wrapped.resize(rows, false);
        if let Some(saved) = self.alt_saved.as_mut() {
            saved.cells = reflow(&saved.cells, self.cols, self.rows, cols, rows);
            saved.wrapped.resize(rows, false);
            saved.cx = saved.cx.min(cols - 1);
            saved.cy = saved.cy.min(rows - 1);
        }
        self.cols = cols;
        self.rows = rows;
        self.cx = self.cx.min(cols - 1);
        self.cy = self.cy.min(rows - 1);
        self.wrap_pending = false;
        self.scroll_top = 0;
        self.scroll_bottom = rows - 1;
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

    fn region_is_full(&self) -> bool {
        self.scroll_top == 0 && self.scroll_bottom + 1 == self.rows
    }

    /// Move rows `top+1..=bottom` up by one, clearing row `bottom`.
    fn shift_up(&mut self, top: usize, bottom: usize) {
        let cols = self.cols;
        self.cells.copy_within((top + 1) * cols..(bottom + 1) * cols, top * cols);
        self.wrapped.remove(top);
        self.wrapped.insert(bottom, false);
        let blank = self.blank();
        self.cells[bottom * cols..(bottom + 1) * cols].fill(blank);
    }

    /// Move rows `top..bottom` down by one, clearing row `top`.
    fn shift_down(&mut self, top: usize, bottom: usize) {
        let cols = self.cols;
        self.cells.copy_within(top * cols..bottom * cols, (top + 1) * cols);
        self.wrapped.remove(bottom);
        self.wrapped.insert(top, false);
        let blank = self.blank();
        self.cells[top * cols..(top + 1) * cols].fill(blank);
    }

    /// Scroll the scroll region up one line. Rows that leave a full-screen region go
    /// into the scrollback history.
    fn scroll_up(&mut self) {
        let (top, bottom, cols) = (self.scroll_top, self.scroll_bottom, self.cols);
        let removed = self.cells[top * cols..(top + 1) * cols].to_vec();
        let removed_wrapped = self.wrapped[top];
        self.shift_up(top, bottom);

        // The alternate screen has no scrollback.
        if self.region_is_full() && !self.in_alt_screen() && self.history_limit > 0 {
            self.history.push_back(removed);
            self.history_wrapped.push_back(removed_wrapped);
            self.trim_history();
            // Keep a scrolled-back view showing the same text as new rows arrive.
            if self.view_offset > 0 {
                self.view_offset = (self.view_offset + 1).min(self.history.len());
            }
        }
    }

    fn newline(&mut self) {
        if self.cy == self.scroll_bottom {
            self.scroll_up();
        } else if self.cy + 1 < self.rows {
            self.cy += 1;
        }
    }

    /// Reverse index (ESC M): move up, scrolling the region down at its top.
    fn reverse_index(&mut self) {
        if self.cy == self.scroll_top {
            self.shift_down(self.scroll_top, self.scroll_bottom);
        } else {
            self.cy = self.cy.saturating_sub(1);
        }
    }

    fn set_wrapped(&mut self, wrapped: bool) {
        self.wrapped[self.cy] = wrapped;
    }

    fn put_char(&mut self, ch: char) {
        // Wide characters (CJK, most emoji) take two columns. Zero-width characters
        // (combining marks) attach to the character before them.
        let width = match UnicodeWidthChar::width(ch) {
            None => return,
            Some(0) => {
                self.add_mark(ch);
                return;
            }
            Some(w) => w.min(self.cols).min(2),
        };
        if self.wrap_pending || self.cx + width > self.cols {
            self.set_wrapped(true);
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

    fn add_mark(&mut self, ch: char) {
        let col = if self.wrap_pending {
            self.cx
        } else if self.cx > 0 {
            self.cx - 1
        } else {
            return;
        };
        let mut idx = self.cy * self.cols + col;
        // If the character is the right half of a wide one, mark the left half.
        if self.cells[idx].ch == '\0' && col > 0 {
            idx -= 1;
        }
        if let Some(slot) = self.cells[idx].marks.iter_mut().find(|m| m.is_none()) {
            *slot = Some(ch);
        }
    }

    fn enter_alt_screen(&mut self) {
        if self.in_alt_screen() {
            return;
        }
        let cells = std::mem::replace(&mut self.cells, vec![Cell::default(); self.cols * self.rows]);
        let wrapped = std::mem::replace(&mut self.wrapped, vec![false; self.rows]);
        self.alt_saved = Some(SavedMain { cells, wrapped, cx: self.cx, cy: self.cy });
        self.cx = 0;
        self.cy = 0;
        self.view_offset = 0;
        self.scroll_top = 0;
        self.scroll_bottom = self.rows - 1;
    }

    fn leave_alt_screen(&mut self) {
        if let Some(saved) = self.alt_saved.take() {
            self.cells = saved.cells;
            self.wrapped = saved.wrapped;
            self.cx = saved.cx;
            self.cy = saved.cy;
            self.scroll_top = 0;
            self.scroll_bottom = self.rows - 1;
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
                2 => self.pen.dim = true,
                3 => self.pen.italic = true,
                22 => {
                    self.pen.bold = false;
                    self.pen.dim = false;
                }
                23 => self.pen.italic = false,
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
                // A line feed ends the row: it is not a wrap.
                self.set_wrapped(false);
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
                    _ => {
                        self.fill(0..len);
                        self.wrapped.iter_mut().for_each(|w| *w = false);
                    }
                }
            }
            'K' => {
                let row = self.cy * self.cols;
                match p.first().copied().unwrap_or(0) {
                    0 => self.fill(row + self.cx..row + self.cols),
                    1 => self.fill(row..row + self.cx + 1),
                    _ => {
                        self.fill(row..row + self.cols);
                        self.wrapped[self.cy] = false;
                    }
                }
            }
            'L' | 'M' if self.cy >= self.scroll_top && self.cy <= self.scroll_bottom => {
                // Insert or delete lines at the cursor, within the scroll region.
                let n = arg(0, 1).min(self.scroll_bottom - self.cy + 1);
                for _ in 0..n {
                    if action == 'L' {
                        self.shift_down(self.cy, self.scroll_bottom);
                    } else {
                        self.shift_up(self.cy, self.scroll_bottom);
                    }
                }
                self.cx = 0;
            }
            'r' if intermediates.is_empty() => {
                // Set the scroll region (DECSTBM). Invalid regions are ignored.
                let top = arg(0, 1) - 1;
                let bottom = arg(1, self.rows) - 1;
                if top < bottom && bottom < self.rows {
                    self.scroll_top = top;
                    self.scroll_bottom = bottom;
                    self.cx = 0;
                    self.cy = 0;
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
            'h' | 'l' if intermediates == [b'?'] => {
                let set = action == 'h';
                for &mode in &p {
                    match mode {
                        1049 => {
                            if set {
                                self.enter_alt_screen();
                            } else {
                                self.leave_alt_screen();
                            }
                        }
                        1000 | 1002 | 1003 => {
                            if set {
                                self.mouse_tracking = mode as u16;
                            } else if self.mouse_tracking == mode as u16 {
                                self.mouse_tracking = 0;
                            }
                        }
                        1005 | 1006 | 1015 => {
                            let wanted = match mode {
                                1005 => MouseEncoding::Utf8,
                                1006 => MouseEncoding::Sgr,
                                _ => MouseEncoding::Urxvt,
                            };
                            if set {
                                self.mouse_encoding = wanted;
                            } else if self.mouse_encoding == wanted {
                                self.mouse_encoding = MouseEncoding::Legacy;
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {} // other modes/sequences are accepted and ignored
        }
    }

    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, byte: u8) {
        match byte {
            b'7' => self.saved = (self.cx, self.cy),
            b'8' => (self.cx, self.cy) = self.saved,
            b'M' => self.reverse_index(),
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

    #[test]
    fn copying_a_wrapped_line_has_no_newline() {
        let mut s = Screen::new(3, 2);
        feed(&mut s, b"abcdef\r\nxy");
        // "abc" wrapped onto "def" (one logical line); "xy" is a separate line.
        assert!(s.row_wrapped(0));
        assert!(!s.row_wrapped(1));
        assert_eq!(s.text_between((0, 0), (2, 2)), "abcdef\nxy");
    }

    #[test]
    fn word_and_line_bounds() {
        let mut s = Screen::new(10, 1);
        feed(&mut s, b"foo bar_1");
        assert_eq!(s.word_bounds(0, 5), ((0, 4), (0, 8)));
        assert_eq!(s.word_bounds(0, 1), ((0, 0), (0, 2)));
        assert_eq!(s.word_bounds(0, 3), ((0, 3), (0, 3)));

        let mut w = Screen::new(3, 2);
        feed(&mut w, b"abcdef\r\nxy");
        assert_eq!(w.line_bounds(0), ((0, 0), (1, 2)));
    }

    #[test]
    fn combining_marks_attach_to_the_previous_character() {
        let mut s = Screen::new(3, 1);
        feed(&mut s, "e\u{301}x".as_bytes());
        assert_eq!(s.visible_cell(0, 0).marks, [Some('\u{301}'), None]);
        assert_eq!(s.visible_cell(1, 0).ch, 'x');
        assert_eq!(s.text_between((0, 0), (0, 2)), "e\u{301}x");
    }

    #[test]
    fn reverse_index_at_top_scrolls_down() {
        let mut s = Screen::new(3, 2);
        feed(&mut s, b"aa\r\nbb\x1b[1;1H\x1bM");
        assert_eq!(row(&s, 0), "   ");
        assert_eq!(row(&s, 1), "aa ");
    }

    #[test]
    fn scroll_region_confines_scrolling() {
        let mut s = Screen::new(3, 4);
        feed(&mut s, b"aaa\r\nbbb\r\nccc\r\nddd");
        feed(&mut s, b"\x1b[2;3r\x1b[3;1H\n");
        assert_eq!(row(&s, 0), "aaa");
        assert_eq!(row(&s, 1), "ccc");
        assert_eq!(row(&s, 2), "   ");
        assert_eq!(row(&s, 3), "ddd");
        assert_eq!(s.history.len(), 0);
    }

    #[test]
    fn insert_and_delete_lines() {
        let mut s = Screen::new(2, 3);
        feed(&mut s, b"aa\r\nbb\r\ncc\x1b[2;1H\x1b[L");
        assert_eq!(row(&s, 0), "aa");
        assert_eq!(row(&s, 1), "  ");
        assert_eq!(row(&s, 2), "bb");
        feed(&mut s, b"\x1b[1;1H\x1b[M");
        assert_eq!(row(&s, 0), "  ");
        assert_eq!(row(&s, 1), "bb");
    }

    #[test]
    fn block_text_takes_the_same_columns_on_each_row() {
        let mut s = Screen::new(4, 2);
        feed(&mut s, b"abcd\r\nefgh");
        assert_eq!(s.block_text((0, 1), (1, 2)), "bc\nfg");
        assert_eq!(s.block_text((1, 2), (0, 1)), "bc\nfg");
    }

    #[test]
    fn mouse_modes_are_tracked() {
        let mut s = Screen::new(3, 1);
        feed(&mut s, b"\x1b[?1002h\x1b[?1006h");
        assert_eq!(s.mouse_tracking(), 1002);
        assert_eq!(s.mouse_encoding(), MouseEncoding::Sgr);
        feed(&mut s, b"\x1b[?1002l");
        assert_eq!(s.mouse_tracking(), 0);
        assert_eq!(s.mouse_encoding(), MouseEncoding::Sgr);
        feed(&mut s, b"\x1b[?1006l");
        assert_eq!(s.mouse_encoding(), MouseEncoding::Legacy);
    }
}
