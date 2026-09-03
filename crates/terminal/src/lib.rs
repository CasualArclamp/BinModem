//! An ANSI/CP437 terminal emulator, sized for BBS use.
//!
//! Headless and self-contained: bytes in, a character grid out. Keeping it free
//! of any drawing code means the whole of it is testable, which matters because
//! BBS ANSI art exercises corners of the escape-sequence grammar that a plain
//! line-oriented console never touches.
//!
//! The dialect targeted is ANSI.SYS as DOS-era boards assumed it, not xterm.
//! The two differ in places that matter here, most visibly `ED 2`: ANSI.SYS
//! homes the cursor after clearing and xterm does not, and boards were written
//! against the former.

pub mod cp437;

use std::collections::VecDeque;

pub const DEFAULT_COLS: usize = 80;
pub const DEFAULT_ROWS: usize = 24;

/// Default foreground: ANSI light grey, as ANSI.SYS started up.
pub const DEFAULT_FG: u8 = 7;
/// Default background: black.
pub const DEFAULT_BG: u8 = 0;

/// Character attributes. Colours are indices into the 16-colour ANSI palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attr {
    pub fg: u8,
    pub bg: u8,
    pub bold: bool,
    pub blink: bool,
    pub reverse: bool,
}

impl Default for Attr {
    fn default() -> Self {
        Self { fg: DEFAULT_FG, bg: DEFAULT_BG, bold: false, blink: false, reverse: false }
    }
}

impl Attr {
    /// Resolve to the pair of palette indices actually drawn.
    ///
    /// Bold brightens the foreground, which is how ANSI.SYS produced its top
    /// eight colours; reverse swaps the two afterwards.
    pub fn resolved(&self) -> (u8, u8) {
        let fg = if self.bold && self.fg < 8 { self.fg + 8 } else { self.fg };
        if self.reverse { (self.bg, fg) } else { (fg, self.bg) }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub attr: Attr,
}

impl Default for Cell {
    fn default() -> Self {
        Self { ch: ' ', attr: Attr::default() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    Escape,
    /// Inside a control sequence, collecting parameters.
    Csi,
    /// Inside an operating-system command, discarding until the terminator.
    Osc,
}

/// A terminal screen.
#[derive(Debug, Clone)]
pub struct Terminal {
    cols: usize,
    rows: usize,
    cells: Vec<Cell>,
    col: usize,
    row: usize,
    attr: Attr,
    saved: Option<(usize, usize, Attr)>,
    state: State,
    params: Vec<u32>,
    /// True when the sequence began `CSI ?`, marking a private mode.
    private: bool,
    scrollback: VecDeque<Vec<Cell>>,
    max_scrollback: usize,
    /// Deferred wrap: writing the last column parks the cursor there rather
    /// than wrapping at once, so a line that exactly fills the width does not
    /// consume the row below it. BBS art depends on this.
    wrap_pending: bool,
    pub autowrap: bool,
    pub cursor_visible: bool,
    /// Set when BEL arrives; the UI clears it after reacting.
    pub bell: bool,
}

impl Default for Terminal {
    fn default() -> Self {
        Self::new(DEFAULT_COLS, DEFAULT_ROWS)
    }
}

impl Terminal {
    pub fn new(cols: usize, rows: usize) -> Self {
        let cols = cols.max(1);
        let rows = rows.max(1);
        Self {
            cols,
            rows,
            cells: vec![Cell::default(); cols * rows],
            col: 0,
            row: 0,
            attr: Attr::default(),
            saved: None,
            state: State::Ground,
            params: Vec::new(),
            private: false,
            scrollback: VecDeque::new(),
            max_scrollback: 2000,
            wrap_pending: false,
            autowrap: true,
            cursor_visible: true,
            bell: false,
        }
    }

    pub fn size(&self) -> (usize, usize) {
        (self.cols, self.rows)
    }

    pub fn cursor(&self) -> (usize, usize) {
        (self.row, self.col)
    }

    pub fn cell(&self, row: usize, col: usize) -> Cell {
        self.cells
            .get(row * self.cols + col)
            .copied()
            .unwrap_or_default()
    }

    /// One row of the live screen.
    pub fn row_cells(&self, row: usize) -> &[Cell] {
        let start = row * self.cols;
        &self.cells[start..start + self.cols]
    }

    /// A row of scrollback, index 0 being the oldest retained.
    pub fn scrollback_row(&self, index: usize) -> Option<&[Cell]> {
        self.scrollback.get(index).map(|r| r.as_slice())
    }

    pub fn scrollback_len(&self) -> usize {
        self.scrollback.len()
    }

    pub fn feed_bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.feed(b);
        }
    }

    pub fn feed(&mut self, byte: u8) {
        match self.state {
            State::Ground => self.ground(byte),
            State::Escape => self.escape(byte),
            State::Csi => self.csi(byte),
            State::Osc => {
                // Runs until BEL or the ST that follows an ESC; the content is
                // a window title or palette change, neither of which we honour.
                if byte == 0x07 || byte == 0x5c {
                    self.state = State::Ground;
                }
            }
        }
    }

    fn ground(&mut self, byte: u8) {
        match byte {
            0x07 => self.bell = true,
            0x08 => {
                self.wrap_pending = false;
                self.col = self.col.saturating_sub(1);
            }
            0x09 => {
                // Tab stops every eight columns.
                let next = ((self.col / 8) + 1) * 8;
                self.col = next.min(self.cols - 1);
                self.wrap_pending = false;
            }
            0x0a => self.line_feed(),
            0x0b | 0x0c => self.line_feed(),
            0x0d => {
                self.col = 0;
                self.wrap_pending = false;
            }
            0x1b => {
                self.state = State::Escape;
                self.params.clear();
                self.private = false;
            }
            _ => self.put(cp437::decode(byte)),
        }
    }

    fn escape(&mut self, byte: u8) {
        match byte {
            b'[' => {
                self.state = State::Csi;
                self.params.clear();
                self.params.push(0);
                self.private = false;
            }
            b']' => self.state = State::Osc,
            // Save and restore cursor, the non-CSI spellings.
            b'7' => {
                self.saved = Some((self.row, self.col, self.attr));
                self.state = State::Ground;
            }
            b'8' => {
                self.restore_cursor();
                self.state = State::Ground;
            }
            // Index and reverse index.
            b'D' => {
                self.line_feed();
                self.state = State::Ground;
            }
            b'M' => {
                if self.row == 0 {
                    self.scroll_down();
                } else {
                    self.row -= 1;
                }
                self.state = State::Ground;
            }
            b'E' => {
                self.col = 0;
                self.line_feed();
                self.state = State::Ground;
            }
            b'c' => {
                self.reset();
                self.state = State::Ground;
            }
            // Character-set selection takes one more byte, which we discard.
            b'(' | b')' | b'*' | b'+' => self.state = State::Escape,
            _ => self.state = State::Ground,
        }
    }

    fn csi(&mut self, byte: u8) {
        match byte {
            b'0'..=b'9' => {
                let last = self.params.last_mut().expect("params seeded on entry");
                *last = last.saturating_mul(10).saturating_add(u32::from(byte - b'0'));
            }
            b';' => self.params.push(0),
            b'?' => self.private = true,
            // Intermediate bytes, none of which we act on.
            0x20..=0x2f => {}
            0x40..=0x7e => {
                self.dispatch(byte);
                self.state = State::Ground;
            }
            _ => self.state = State::Ground,
        }
    }

    /// Parameter `n`, defaulting to `default` when absent or written as zero.
    fn param(&self, n: usize, default: u32) -> u32 {
        match self.params.get(n) {
            Some(0) | None => default,
            Some(v) => *v,
        }
    }

    fn dispatch(&mut self, final_byte: u8) {
        let p0 = self.param(0, 1) as usize;
        match final_byte {
            b'A' => {
                self.row = self.row.saturating_sub(p0);
                self.wrap_pending = false;
            }
            b'B' => {
                self.row = (self.row + p0).min(self.rows - 1);
                self.wrap_pending = false;
            }
            b'C' => {
                self.col = (self.col + p0).min(self.cols - 1);
                self.wrap_pending = false;
            }
            b'D' => {
                self.col = self.col.saturating_sub(p0);
                self.wrap_pending = false;
            }
            b'E' => {
                self.row = (self.row + p0).min(self.rows - 1);
                self.col = 0;
                self.wrap_pending = false;
            }
            b'F' => {
                self.row = self.row.saturating_sub(p0);
                self.col = 0;
                self.wrap_pending = false;
            }
            b'G' => {
                self.col = (p0 - 1).min(self.cols - 1);
                self.wrap_pending = false;
            }
            b'd' => {
                self.row = (p0 - 1).min(self.rows - 1);
                self.wrap_pending = false;
            }
            b'H' | b'f' => {
                let r = self.param(0, 1) as usize;
                let c = self.param(1, 1) as usize;
                self.row = (r - 1).min(self.rows - 1);
                self.col = (c - 1).min(self.cols - 1);
                self.wrap_pending = false;
            }
            b'J' => self.erase_display(self.param(0, 0)),
            b'K' => self.erase_line(self.param(0, 0)),
            b'L' => self.insert_lines(p0),
            b'M' => self.delete_lines(p0),
            b'P' => self.delete_chars(p0),
            b'X' => self.erase_chars(p0),
            b'@' => self.insert_chars(p0),
            b'm' => self.select_graphic_rendition(),
            b's' => self.saved = Some((self.row, self.col, self.attr)),
            b'u' => self.restore_cursor(),
            b'h' | b'l' => {
                let set = final_byte == b'h';
                if self.private {
                    match self.param(0, 0) {
                        7 => self.autowrap = set,
                        25 => self.cursor_visible = set,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    fn select_graphic_rendition(&mut self) {
        if self.params.is_empty() {
            self.attr = Attr::default();
            return;
        }
        // A bare "CSI m" is a reset, which the seeded zero already expresses.
        for i in 0..self.params.len() {
            match self.params[i] {
                0 => self.attr = Attr::default(),
                1 => self.attr.bold = true,
                5 => self.attr.blink = true,
                7 => self.attr.reverse = true,
                22 => self.attr.bold = false,
                25 => self.attr.blink = false,
                27 => self.attr.reverse = false,
                v @ 30..=37 => self.attr.fg = (v - 30) as u8,
                39 => self.attr.fg = DEFAULT_FG,
                v @ 40..=47 => self.attr.bg = (v - 40) as u8,
                49 => self.attr.bg = DEFAULT_BG,
                v @ 90..=97 => self.attr.fg = (v - 90) as u8 + 8,
                v @ 100..=107 => self.attr.bg = (v - 100) as u8 + 8,
                _ => {}
            }
        }
    }

    fn restore_cursor(&mut self) {
        if let Some((r, c, a)) = self.saved {
            self.row = r.min(self.rows - 1);
            self.col = c.min(self.cols - 1);
            self.attr = a;
            self.wrap_pending = false;
        }
    }

    fn put(&mut self, ch: char) {
        if self.wrap_pending && self.autowrap {
            self.col = 0;
            self.line_feed();
        }
        self.wrap_pending = false;

        let index = self.row * self.cols + self.col;
        self.cells[index] = Cell { ch, attr: self.attr };

        if self.col + 1 >= self.cols {
            // Park here; the next printable triggers the wrap.
            self.wrap_pending = true;
        } else {
            self.col += 1;
        }
    }

    fn line_feed(&mut self) {
        self.wrap_pending = false;
        if self.row + 1 >= self.rows {
            self.scroll_up();
        } else {
            self.row += 1;
        }
    }

    fn scroll_up(&mut self) {
        let top: Vec<Cell> = self.row_cells(0).to_vec();
        if self.max_scrollback > 0 {
            if self.scrollback.len() == self.max_scrollback {
                self.scrollback.pop_front();
            }
            self.scrollback.push_back(top);
        }
        self.cells.copy_within(self.cols.., 0);
        let last = (self.rows - 1) * self.cols;
        // A scrolled-in row takes the current background, not the default, so
        // a coloured screen scrolls in its own colour.
        let blank = Cell { ch: ' ', attr: self.blank_attr() };
        self.cells[last..].fill(blank);
    }

    fn scroll_down(&mut self) {
        let last = (self.rows - 1) * self.cols;
        self.cells.copy_within(..last, self.cols);
        let blank = Cell { ch: ' ', attr: self.blank_attr() };
        self.cells[..self.cols].fill(blank);
    }

    /// Attributes used to fill erased or scrolled-in cells.
    fn blank_attr(&self) -> Attr {
        Attr { bg: self.attr.bg, ..Attr::default() }
    }

    fn erase_display(&mut self, mode: u32) {
        let blank = Cell { ch: ' ', attr: self.blank_attr() };
        let cursor = self.row * self.cols + self.col;
        match mode {
            0 => self.cells[cursor..].fill(blank),
            1 => self.cells[..=cursor].fill(blank),
            _ => {
                self.cells.fill(blank);
                // ANSI.SYS homes the cursor here and BBS art relies on it.
                self.row = 0;
                self.col = 0;
                self.wrap_pending = false;
            }
        }
    }

    fn erase_line(&mut self, mode: u32) {
        let blank = Cell { ch: ' ', attr: self.blank_attr() };
        let start = self.row * self.cols;
        let end = start + self.cols;
        let cursor = start + self.col;
        match mode {
            0 => self.cells[cursor..end].fill(blank),
            1 => self.cells[start..=cursor].fill(blank),
            _ => self.cells[start..end].fill(blank),
        }
    }

    fn erase_chars(&mut self, count: usize) {
        let blank = Cell { ch: ' ', attr: self.blank_attr() };
        let start = self.row * self.cols + self.col;
        let end = (start + count).min((self.row + 1) * self.cols);
        self.cells[start..end].fill(blank);
    }

    fn insert_chars(&mut self, count: usize) {
        let row_start = self.row * self.cols;
        let row_end = row_start + self.cols;
        let from = row_start + self.col;
        let count = count.min(row_end - from);
        self.cells.copy_within(from..row_end - count, from + count);
        let blank = Cell { ch: ' ', attr: self.blank_attr() };
        self.cells[from..from + count].fill(blank);
    }

    fn delete_chars(&mut self, count: usize) {
        let row_start = self.row * self.cols;
        let row_end = row_start + self.cols;
        let from = row_start + self.col;
        let count = count.min(row_end - from);
        self.cells.copy_within(from + count..row_end, from);
        let blank = Cell { ch: ' ', attr: self.blank_attr() };
        self.cells[row_end - count..row_end].fill(blank);
    }

    fn insert_lines(&mut self, count: usize) {
        let count = count.min(self.rows - self.row);
        let from = self.row * self.cols;
        let end = self.rows * self.cols;
        self.cells.copy_within(from..end - count * self.cols, from + count * self.cols);
        let blank = Cell { ch: ' ', attr: self.blank_attr() };
        self.cells[from..from + count * self.cols].fill(blank);
    }

    fn delete_lines(&mut self, count: usize) {
        let count = count.min(self.rows - self.row);
        let from = self.row * self.cols;
        let end = self.rows * self.cols;
        self.cells.copy_within(from + count * self.cols..end, from);
        let blank = Cell { ch: ' ', attr: self.blank_attr() };
        self.cells[end - count * self.cols..end].fill(blank);
    }

    pub fn reset(&mut self) {
        self.cells.fill(Cell::default());
        self.col = 0;
        self.row = 0;
        self.attr = Attr::default();
        self.saved = None;
        self.state = State::Ground;
        self.params.clear();
        self.wrap_pending = false;
        self.autowrap = true;
        self.cursor_visible = true;
    }

    pub fn clear_scrollback(&mut self) {
        self.scrollback.clear();
    }

    /// Resize, preserving as much of the screen as still fits.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let cols = cols.max(1);
        let rows = rows.max(1);
        if cols == self.cols && rows == self.rows {
            return;
        }
        let mut next = vec![Cell::default(); cols * rows];
        for r in 0..rows.min(self.rows) {
            for c in 0..cols.min(self.cols) {
                next[r * cols + c] = self.cells[r * self.cols + c];
            }
        }
        self.cells = next;
        self.cols = cols;
        self.rows = rows;
        self.row = self.row.min(rows - 1);
        self.col = self.col.min(cols - 1);
        self.wrap_pending = false;
    }

    /// The visible screen as text, trailing blanks trimmed. For tests and for
    /// copying a screen out.
    pub fn text(&self) -> String {
        (0..self.rows)
            .map(|r| {
                let line: String = self.row_cells(r).iter().map(|c| c.ch).collect();
                line.trim_end().to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn term() -> Terminal {
        Terminal::new(20, 5)
    }

    fn feed(t: &mut Terminal, s: &str) {
        t.feed_bytes(s.as_bytes());
    }

    #[test]
    fn writes_plain_text() {
        let mut t = term();
        feed(&mut t, "HELLO");
        assert_eq!(t.text().lines().next().unwrap(), "HELLO");
        assert_eq!(t.cursor(), (0, 5));
    }

    #[test]
    fn carriage_return_and_line_feed() {
        let mut t = term();
        feed(&mut t, "AB\r\nCD");
        let screen = t.text();
        let lines: Vec<&str> = screen.lines().collect();
        assert_eq!(lines[0], "AB");
        assert_eq!(lines[1], "CD");
    }

    #[test]
    fn backspace_moves_left_without_erasing() {
        let mut t = term();
        feed(&mut t, "AB\x08C");
        assert_eq!(t.text().lines().next().unwrap(), "AC");
    }

    #[test]
    fn tab_advances_to_the_next_eight_column_stop() {
        let mut t = term();
        feed(&mut t, "A\tB");
        assert_eq!(t.cursor().1, 9);
        assert_eq!(t.cell(0, 8).ch, 'B');
    }

    #[test]
    fn cursor_positioning() {
        let mut t = term();
        feed(&mut t, "\x1b[3;5HX");
        // CSI H is one-based.
        assert_eq!(t.cell(2, 4).ch, 'X');
    }

    #[test]
    fn cursor_home_with_no_parameters() {
        let mut t = term();
        feed(&mut t, "hello\x1b[HX");
        assert_eq!(t.cell(0, 0).ch, 'X');
    }

    #[test]
    fn relative_cursor_movement() {
        let mut t = term();
        feed(&mut t, "\x1b[2;2H\x1b[2C\x1b[1BX");
        assert_eq!(t.cell(2, 3).ch, 'X');
    }

    #[test]
    fn erase_display_two_clears_and_homes() {
        // ANSI.SYS behaviour, which BBS art assumes; xterm would not home.
        let mut t = term();
        feed(&mut t, "\x1b[3;3Hjunk\x1b[2J");
        assert_eq!(t.cursor(), (0, 0));
        assert!(t.text().trim().is_empty());
    }

    #[test]
    fn erase_to_end_of_line() {
        let mut t = term();
        feed(&mut t, "ABCDEF\x1b[1;3H\x1b[K");
        assert_eq!(t.text().lines().next().unwrap(), "AB");
    }

    #[test]
    fn erase_from_start_of_line() {
        let mut t = term();
        feed(&mut t, "ABCDEF\x1b[1;3H\x1b[1K");
        assert_eq!(t.cell(0, 0).ch, ' ');
        assert_eq!(t.cell(0, 2).ch, ' ');
        assert_eq!(t.cell(0, 3).ch, 'D');
    }

    #[test]
    fn colours_are_recorded() {
        let mut t = term();
        feed(&mut t, "\x1b[31;44mR");
        let c = t.cell(0, 0);
        assert_eq!(c.attr.fg, 1);
        assert_eq!(c.attr.bg, 4);
    }

    #[test]
    fn bold_brightens_the_foreground() {
        let mut t = term();
        feed(&mut t, "\x1b[1;32mG");
        let (fg, bg) = t.cell(0, 0).attr.resolved();
        assert_eq!(fg, 10, "bold green should resolve to bright green");
        assert_eq!(bg, 0);
    }

    #[test]
    fn reverse_swaps_the_pair() {
        let mut t = term();
        feed(&mut t, "\x1b[7;31;40mX");
        let (fg, bg) = t.cell(0, 0).attr.resolved();
        assert_eq!((fg, bg), (0, 1));
    }

    #[test]
    fn sgr_zero_resets() {
        let mut t = term();
        feed(&mut t, "\x1b[31;1m\x1b[0mX");
        assert_eq!(t.cell(0, 0).attr, Attr::default());
    }

    #[test]
    fn bare_sgr_is_a_reset() {
        let mut t = term();
        feed(&mut t, "\x1b[31m\x1b[mX");
        assert_eq!(t.cell(0, 0).attr.fg, DEFAULT_FG);
    }

    #[test]
    fn bright_colour_codes_work() {
        let mut t = term();
        feed(&mut t, "\x1b[93mX");
        assert_eq!(t.cell(0, 0).attr.fg, 11);
    }

    #[test]
    fn save_and_restore_cursor() {
        let mut t = term();
        feed(&mut t, "\x1b[2;2H\x1b[s\x1b[5;10H\x1b[uX");
        assert_eq!(t.cell(1, 1).ch, 'X');
    }

    #[test]
    fn deferred_wrap_does_not_waste_a_row() {
        // A line exactly filling the width must not consume the row below until
        // another character actually arrives.
        let mut t = Terminal::new(5, 3);
        feed(&mut t, "ABCDE");
        assert_eq!(t.cursor(), (0, 4), "cursor parks on the last column");
        feed(&mut t, "F");
        assert_eq!(t.cursor(), (1, 1));
        assert_eq!(t.cell(1, 0).ch, 'F');
    }

    #[test]
    fn autowrap_can_be_disabled() {
        let mut t = Terminal::new(5, 3);
        feed(&mut t, "\x1b[?7lABCDEFGH");
        assert_eq!(t.text().lines().next().unwrap(), "ABCDH");
        assert_eq!(t.cursor().0, 0, "should never have left the first row");
    }

    #[test]
    fn scrolling_pushes_rows_into_scrollback() {
        let mut t = Terminal::new(10, 2);
        feed(&mut t, "one\r\ntwo\r\nthree");
        assert_eq!(t.scrollback_len(), 1);
        let first: String = t.scrollback_row(0).unwrap().iter().map(|c| c.ch).collect();
        assert_eq!(first.trim_end(), "one");
        let screen = t.text();
        let lines: Vec<&str> = screen.lines().collect();
        assert_eq!(lines[0], "two");
        assert_eq!(lines[1], "three");
    }

    #[test]
    fn scrolled_in_rows_take_the_current_background() {
        let mut t = Terminal::new(4, 2);
        feed(&mut t, "\x1b[44ma\r\nb\r\nc");
        assert_eq!(t.cell(1, 3).attr.bg, 4, "new row should carry the blue background");
    }

    #[test]
    fn cp437_high_bytes_become_box_drawing() {
        let mut t = term();
        t.feed_bytes(&[0xC9, 0xCD, 0xBB]);
        assert_eq!(t.text().lines().next().unwrap(), "╔═╗");
    }

    #[test]
    fn insert_and_delete_characters() {
        let mut t = term();
        feed(&mut t, "ABCDE\x1b[1;2H\x1b[2@");
        assert_eq!(t.text().lines().next().unwrap(), "A  BCDE");
        feed(&mut t, "\x1b[1;2H\x1b[2P");
        assert_eq!(t.text().lines().next().unwrap(), "ABCDE");
    }

    #[test]
    fn delete_lines_pulls_the_rest_up() {
        let mut t = Terminal::new(10, 4);
        feed(&mut t, "a\r\nb\r\nc\r\nd\x1b[2;1H\x1b[1M");
        let screen = t.text();
        let lines: Vec<&str> = screen.lines().collect();
        assert_eq!(lines[0], "a");
        assert_eq!(lines[1], "c");
        assert_eq!(lines[2], "d");
    }

    #[test]
    fn unknown_sequences_are_swallowed_not_printed() {
        let mut t = term();
        feed(&mut t, "\x1b[999ZA");
        assert_eq!(t.text().lines().next().unwrap(), "A");
    }

    #[test]
    fn operating_system_commands_are_discarded() {
        let mut t = term();
        feed(&mut t, "\x1b]0;a window title\x07X");
        assert_eq!(t.text().lines().next().unwrap(), "X");
    }

    #[test]
    fn private_mode_hides_the_cursor() {
        let mut t = term();
        feed(&mut t, "\x1b[?25l");
        assert!(!t.cursor_visible);
        feed(&mut t, "\x1b[?25h");
        assert!(t.cursor_visible);
    }

    #[test]
    fn bell_is_flagged() {
        let mut t = term();
        t.feed(0x07);
        assert!(t.bell);
    }

    #[test]
    fn resize_preserves_what_still_fits() {
        let mut t = Terminal::new(10, 3);
        feed(&mut t, "hello");
        t.resize(20, 5);
        assert_eq!(t.text().lines().next().unwrap(), "hello");
        assert_eq!(t.size(), (20, 5));
    }

    #[test]
    fn cursor_stays_inside_after_shrinking() {
        let mut t = Terminal::new(20, 10);
        feed(&mut t, "\x1b[9;19HX");
        t.resize(5, 3);
        let (r, c) = t.cursor();
        assert!(r < 3 && c < 5, "cursor at {r},{c} escaped the new size");
    }

    /// A miniature BBS screen: clear, position, colour and box drawing all at
    /// once, which is what a real board's login banner does.
    #[test]
    fn renders_a_bbs_style_banner() {
        let mut t = Terminal::new(12, 4);
        t.feed_bytes(b"\x1b[2J\x1b[1;1m");
        t.feed_bytes(&[0xC9]);
        for _ in 0..4 {
            t.feed_bytes(&[0xCD]);
        }
        t.feed_bytes(&[0xBB]);
        t.feed_bytes(b"\x1b[2;1H");
        t.feed_bytes(&[0xBA]);
        t.feed_bytes(b"\x1b[32mBBS\x1b[0m");
        t.feed_bytes(&[0xBA]);
        let screen = t.text();
        let lines: Vec<&str> = screen.lines().collect();
        assert_eq!(lines[0], "╔════╗");
        assert_eq!(lines[1], "║BBS║");
        assert_eq!(t.cell(1, 1).attr.fg, 2, "the B should be green");
    }
}
