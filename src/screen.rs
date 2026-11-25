//! The virtual screen: a grid of cells plus the cursor, updated by the VT parser.

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
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
    pub reverse: bool,
}

impl Default for Cell {
    fn default() -> Self {
        Cell { ch: ' ', fg: Color::Default, bg: Color::Default, bold: false, reverse: false }
    }
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
        }
    }

    /// Returns (and clears) any bytes the shell should receive as replies to queries.
    pub fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.replies)
    }

    /// The cell at column `x`, row `y`.
    pub fn cell(&self, x: usize, y: usize) -> Cell {
        self.cells[y * self.cols + x]
    }

    /// Cursor position as (column, row).
    pub fn cursor(&self) -> (usize, usize) {
        (self.cx, self.cy)
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        let (cols, rows) = (cols.max(1), rows.max(1));
        let mut new = vec![Cell::default(); cols * rows];
        for y in 0..rows.min(self.rows) {
            for x in 0..cols.min(self.cols) {
                new[y * cols + x] = self.cells[y * self.cols + x];
            }
        }
        self.cells = new;
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
        self.cells.drain(0..self.cols);
        let blank = self.blank();
        self.cells.extend(std::iter::repeat(blank).take(self.cols));
    }

    fn newline(&mut self) {
        if self.cy + 1 >= self.rows {
            self.scroll_up();
        } else {
            self.cy += 1;
        }
    }

    fn put_char(&mut self, ch: char) {
        if self.wrap_pending {
            self.cx = 0;
            self.newline();
            self.wrap_pending = false;
        }
        let idx = self.cy * self.cols + self.cx;
        self.cells[idx] = Cell { ch, ..self.pen };
        if self.cx + 1 < self.cols {
            self.cx += 1;
        } else {
            self.wrap_pending = true;
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
                7 => self.pen.reverse = true,
                27 => self.pen.reverse = false,
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
                // Alternate screen (vim, less, ...): start clean. We don't keep a second buffer.
                if action == 'h' {
                    self.fill(0..self.cells.len());
                    self.cx = 0;
                    self.cy = 0;
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
        s.cells[y * s.cols..(y + 1) * s.cols].iter().map(|c| c.ch).collect()
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
        assert_eq!(s.cells[0].fg, Color::Indexed(1));
        assert!(s.cells[0].bold);
        assert_eq!(s.cells[1].fg, Color::Default);
        assert!(!s.cells[1].bold);
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
}
