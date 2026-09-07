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

/// How much of the mouse the far end has asked to hear about.
///
/// These are separate modes rather than a dial, and a board may have several
/// on at once; this is which of them is in charge, which is always the one
/// that reports the most.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tracking {
    /// Nothing is reported, and the mouse belongs to whatever is drawing the
    /// screen. The state every terminal starts in and returns to.
    #[default]
    Off,
    /// DECSET 9, the X10 original: presses, and nothing else. No releases, no
    /// modifier keys, and no way to say which button was let go of.
    Press,
    /// DECSET 1000: presses and releases, with modifiers.
    Normal,
    /// DECSET 1002: and movement, but only while a button is held.
    Drag,
    /// DECSET 1003: and movement whether a button is held or not.
    Any,
}

/// How the numbers in a report are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Coordinates {
    /// The original: one byte each, offset by 32 so that a report stays
    /// printable characters. Which puts the last reportable position at 223,
    /// and is the entire reason the other two exist.
    #[default]
    Legacy,
    /// DECSET 1006: decimal and separated, with a release told apart by the
    /// final byte rather than by a button code that throws away which button
    /// it was. What to prefer wherever the far end offers it.
    Sgr,
    /// DECSET 1015: decimal, but still offset by 32 and still unable to say
    /// which button was released.
    Urxvt,
}

/// Which button, as far as a report is concerned.
///
/// A wheel is a button in this protocol. It is pressed and never released,
/// which is as close as an encoding designed around three buttons could get
/// to a thing that only ever happens once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
}

/// What the mouse did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Press,
    Release,
    /// The pointer moved, with whatever was held while it did.
    Moved,
}

/// Which of the modifier keys were down at the time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

/// One thing the mouse did, in cells.
#[derive(Debug, Clone, Copy)]
pub struct Mouse {
    pub motion: Motion,
    /// The button pressed or released, or the one held during a move. `None`
    /// for a move with nothing held.
    pub button: Option<Button>,
    /// Zero-based, as the screen is. The wire is one-based, and that
    /// conversion happens on the way out and nowhere else.
    pub col: usize,
    pub row: usize,
    pub modifiers: Modifiers,
}

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
    /// The four tracking modes, which are independent flags rather than one
    /// setting. A board that turns dragging off while normal tracking is still
    /// on means to go on hearing about buttons, so collapsing them into a
    /// single value here would silence a board that had not asked to be
    /// silenced.
    mouse_press: bool,
    mouse_normal: bool,
    mouse_drag: bool,
    mouse_any: bool,
    /// The two extended encodings, likewise independent.
    mouse_sgr: bool,
    mouse_urxvt: bool,
    /// The cell the pointer was last reported in.
    ///
    /// A mouse moves in pixels and this protocol speaks in cells, so most of
    /// what a pointer does is not news. Reporting it anyway would put dozens
    /// of six-byte messages on the line for one sweep across the screen --
    /// which matters here more than it does in a terminal emulator, because
    /// the line under this one may be carrying 300 bits a second.
    mouse_cell: Option<(usize, usize)>,
    /// Bytes the terminal owes the far end, waiting to be sent.
    ///
    /// A terminal is not only a screen. Some sequences are questions, and a
    /// board that asks one and hears nothing draws its own conclusion: the
    /// near-universal test for whether a caller can do ANSI is to ask where
    /// the cursor is and see whether anything comes back. Answer and you get
    /// colour; stay silent and you get "Graphics Mode -> 0" and forty years
    /// of ASCII art you cannot see.
    reply: Vec<u8>,
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
            mouse_press: false,
            mouse_normal: false,
            mouse_drag: false,
            mouse_any: false,
            mouse_sgr: false,
            mouse_urxvt: false,
            mouse_cell: None,
            reply: Vec::new(),
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
            // Device status report. 6 asks where the cursor is; anything
            // else that a board sends here is asking whether the terminal is
            // alive at all.
            b'n' => match self.param(0, 0) {
                6 => {
                    let (row, col) = (self.row + 1, self.col + 1);
                    self.answer(format!("[{row};{col}R"));
                }
                5 => self.answer("[0n".to_owned()),
                _ => {}
            },
            // Device attributes: what kind of terminal is this. The answer is
            // the one a VT100 with no options gives, which is what every
            // terminal program pretending to be one has said ever since and
            // what a board is expecting to be able to parse.
            b'c' if self.param(0, 0) == 0 => self.answer("[?1;0c".to_owned()),
            b'h' | b'l' => {
                let set = final_byte == b'h';
                if self.private {
                    // Every parameter, not only the first. A board that wants
                    // tracking and extended coordinates asks for both in one
                    // sequence, and reading only the first would leave it
                    // sending positions in an encoding nobody agreed to.
                    for i in 0..self.params.len() {
                        match self.param(i, 0) {
                            7 => self.autowrap = set,
                            25 => self.cursor_visible = set,
                            9 => self.mouse_press = set,
                            1000 => self.mouse_normal = set,
                            1002 => self.mouse_drag = set,
                            1003 => self.mouse_any = set,
                            1006 => self.mouse_sgr = set,
                            1015 => self.mouse_urxvt = set,
                            _ => {}
                        }
                    }
                    // Wherever the pointer was is no longer worth comparing
                    // against: the far end has changed its mind about what it
                    // wants to hear, and the first thing it hears should be
                    // where the pointer actually is.
                    self.mouse_cell = None;
                }
            }
            _ => {}
        }
    }

    /// Queue an escape sequence to go back to the far end.
    ///
    /// Capped, because the far end controls how many questions it asks and
    /// nothing here controls how quickly they are collected. At 300 bit/s a
    /// reply takes twenty milliseconds to send, so a board that asked
    /// faster than that could otherwise grow this without limit.
    fn answer(&mut self, csi: String) {
        self.answer_bytes(csi.as_bytes());
    }

    /// The same, for a sequence that is not text.
    ///
    /// The original mouse encoding offsets its numbers by 32 to keep them
    /// printable, which works as far as column 95 and then starts producing
    /// bytes that are not characters at all. It was never text; it only looked
    /// like it for the first ninety-five columns.
    fn answer_bytes(&mut self, csi: &[u8]) {
        const LIMIT: usize = 256;
        if self.reply.len() + csi.len() + 1 > LIMIT {
            return;
        }
        self.reply.push(0x1b);
        self.reply.extend_from_slice(csi);
    }

    /// Which tracking mode is in charge, if any.
    ///
    /// The most talkative of those enabled, because that is what each of them
    /// asked for and none of them asked for less.
    pub fn mouse_tracking(&self) -> Tracking {
        if self.mouse_any {
            Tracking::Any
        } else if self.mouse_drag {
            Tracking::Drag
        } else if self.mouse_normal {
            Tracking::Normal
        } else if self.mouse_press {
            Tracking::Press
        } else {
            Tracking::Off
        }
    }

    /// How reports are being written.
    pub fn mouse_coordinates(&self) -> Coordinates {
        if self.mouse_sgr {
            Coordinates::Sgr
        } else if self.mouse_urxvt {
            Coordinates::Urxvt
        } else {
            Coordinates::Legacy
        }
    }

    /// Tell the far end what the mouse did, if it asked to be told.
    ///
    /// Returns whether anything was actually sent. Everything the pointer does
    /// can be handed to this: what is not wanted, or is not news, is dropped
    /// here rather than in the caller, so that only one place has to know what
    /// each mode means.
    pub fn mouse(&mut self, event: Mouse) -> bool {
        let tracking = self.mouse_tracking();
        let wanted = match (tracking, event.motion) {
            (Tracking::Off, _) => false,
            (_, Motion::Press) => true,
            // X10 had presses and nothing else, and a board that asked for it
            // is not expecting to be told about anything else.
            (Tracking::Press, _) => false,
            (_, Motion::Release) => true,
            (Tracking::Normal, Motion::Moved) => false,
            (Tracking::Drag, Motion::Moved) => event.button.is_some(),
            (Tracking::Any, Motion::Moved) => true,
        };
        if !wanted {
            return false;
        }

        // Clamped rather than dropped: a pointer a fraction of a cell past the
        // edge is still pointing at the edge, and a board that laid something
        // out in the last column should be able to have it clicked on.
        let col = event.col.min(self.cols - 1);
        let row = event.row.min(self.rows - 1);
        if event.motion == Motion::Moved && self.mouse_cell == Some((col, row)) {
            return false;
        }
        self.mouse_cell = Some((col, row));

        let button = match event.button {
            Some(Button::Left) | None => 0,
            Some(Button::Middle) => 1,
            Some(Button::Right) => 2,
            // Bit 6, which is how a wheel was added to an encoding with room
            // for three buttons and no room for a fourth.
            Some(Button::WheelUp) => 64,
            Some(Button::WheelDown) => 65,
        };
        // A move with nothing held is button 3, which is also what a release
        // is: the low two bits ran out. Only the extended encoding can say
        // which button was released, and only because the final byte carries
        // the news instead.
        let mut code = match (event.motion, event.button) {
            (Motion::Release, _) if !self.mouse_sgr => 3,
            (Motion::Moved, None) => 3,
            _ => button,
        };
        // X10 had no modifier bits and no motion bit. Setting them would be
        // sending a board something it has no code to read.
        if tracking != Tracking::Press {
            if event.modifiers.shift {
                code |= 4;
            }
            if event.modifiers.alt {
                code |= 8;
            }
            if event.modifiers.ctrl {
                code |= 16;
            }
            if event.motion == Motion::Moved {
                code |= 32;
            }
        }

        // One-based on the wire, which is the only place it is.
        let (x, y) = (col + 1, row + 1);
        match self.mouse_coordinates() {
            Coordinates::Sgr => {
                let last = if event.motion == Motion::Release { 'm' } else { 'M' };
                self.answer(format!("[<{code};{x};{y}{last}"));
            }
            Coordinates::Urxvt => self.answer(format!("[{};{x};{y}M", code + 32)),
            Coordinates::Legacy => {
                // The offset that keeps a report printable also caps it. Past
                // 223 there is no byte left to say the number with, and a
                // report that wrapped round would put the click somewhere the
                // pointer has never been -- so there is nothing to send.
                if x > 223 || y > 223 {
                    return false;
                }
                self.answer_bytes(&[
                    b'[',
                    b'M',
                    32 + code as u8,
                    32 + x as u8,
                    32 + y as u8,
                ]);
            }
        }
        true
    }

    /// Take what the terminal owes the far end. Draining leaves it empty.
    pub fn take_reply(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.reply)
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
        // Including the mouse. A reset is a terminal saying it is starting
        // again, and a terminal that went on reporting to a board which had
        // just cleared everything would be answering a question nobody had
        // asked twice.
        self.mouse_press = false;
        self.mouse_normal = false;
        self.mouse_drag = false;
        self.mouse_any = false;
        self.mouse_sgr = false;
        self.mouse_urxvt = false;
        self.mouse_cell = None;
        // And anything still owed to the far end, for the same reason. A
        // board that asked where the cursor was before all this and is
        // answered after it is being told about a screen that no longer
        // exists. The bell goes with it: it was rung at a terminal that has
        // since started again.
        self.reply.clear();
        self.bell = false;
        self.private = false;
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
    #[test]
    fn a_cursor_report_is_answered_with_where_the_cursor_is() {
        // The question every board asks to find out whether it is talking to
        // something that can do ANSI. Answer it and you get colour; say
        // nothing and you get told "Graphics Mode -> 0" and the art is wasted.
        let mut t = Terminal::new(80, 25);
        t.feed_bytes(b"hello");
        t.feed_bytes(b"\x1b[6n");
        // One-based, row then column, which puts this at column six.
        assert_eq!(t.take_reply(), b"\x1b[1;6R");
        assert!(t.take_reply().is_empty(), "answered the same question twice");
    }

    #[test]
    fn the_question_itself_is_not_printed() {
        // It is a question, not text. A terminal that drew it would put
        // "[6n" in the middle of the board's own sentence.
        let mut t = Terminal::new(80, 25);
        t.feed_bytes(b"Detecting\x1b[6n emulation");
        let line: String = (0..80).map(|c| t.cell(0, c).ch).collect();
        assert!(
            line.starts_with("Detecting emulation"),
            "the probe was drawn on the screen: {line:?}"
        );
    }

    #[test]
    fn a_cursor_report_follows_the_cursor() {
        let mut t = Terminal::new(80, 25);
        t.feed_bytes(b"\x1b[12;40H\x1b[6n");
        assert_eq!(t.take_reply(), b"\x1b[12;40R");
    }

    #[test]
    fn the_terminal_says_what_it_is_when_asked() {
        let mut t = Terminal::new(80, 25);
        t.feed_bytes(b"\x1b[c");
        assert_eq!(t.take_reply(), b"\x1b[?1;0c");
    }

    #[test]
    fn a_terminal_nobody_asks_anything_owes_nothing() {
        let mut t = Terminal::new(80, 25);
        t.feed_bytes(b"ordinary text\r\nand more\x1b[2J\x1b[1;1H");
        assert!(t.take_reply().is_empty());
    }

    #[test]
    fn an_unanswered_pile_of_questions_stops_growing() {
        // The far end decides how often to ask and this end decides how often
        // to collect, and those are not the same clock. At 300 bit/s a reply
        // takes twenty milliseconds to put on the line.
        let mut t = Terminal::new(80, 25);
        for _ in 0..1000 {
            t.feed_bytes(b"\x1b[6n");
        }
        assert!(t.take_reply().len() <= 256);
    }

}

#[cfg(test)]
mod mouse_tests {
    use super::*;

    fn at(motion: Motion, button: Option<Button>, col: usize, row: usize) -> Mouse {
        Mouse { motion, button, col, row, modifiers: Modifiers::default() }
    }

    fn press(col: usize, row: usize) -> Mouse {
        at(Motion::Press, Some(Button::Left), col, row)
    }

    fn enabled(modes: &str) -> Terminal {
        let mut t = Terminal::new(DEFAULT_COLS, DEFAULT_ROWS);
        t.feed_bytes(format!("\x1b[?{modes}h").as_bytes());
        t
    }

    #[test]
    fn a_terminal_says_nothing_about_the_mouse_until_it_is_asked() {
        // The default matters. A board that never asked and gets a report
        // reads it as somebody typing an escape sequence at its menu.
        let mut t = Terminal::new(DEFAULT_COLS, DEFAULT_ROWS);
        assert_eq!(t.mouse_tracking(), Tracking::Off);
        assert!(!t.mouse(press(0, 0)));
        assert!(t.take_reply().is_empty());
    }

    #[test]
    fn normal_tracking_reports_a_press_where_it_happened() {
        // The original encoding: button, then column, then row, each offset by
        // 32, and both numbers one-based.
        let mut t = enabled("1000");
        assert_eq!(t.mouse_tracking(), Tracking::Normal);
        assert!(t.mouse(press(0, 0)));
        assert_eq!(t.take_reply(), vec![0x1b, b'[', b'M', 32, 33, 33]);

        assert!(t.mouse(press(4, 9)));
        assert_eq!(t.take_reply(), vec![0x1b, b'[', b'M', 32, 37, 42]);
    }

    #[test]
    fn the_original_encoding_cannot_say_which_button_was_released() {
        // Button three, whichever it was: the low two bits ran out. Worth a
        // test because it looks like a bug every time it is read.
        let mut t = enabled("1000");
        t.mouse(at(Motion::Press, Some(Button::Right), 0, 0));
        t.take_reply();
        assert!(t.mouse(at(Motion::Release, Some(Button::Right), 0, 0)));
        assert_eq!(t.take_reply(), vec![0x1b, b'[', b'M', 35, 33, 33]);
    }

    #[test]
    fn the_extended_encoding_keeps_the_button_and_says_so_in_the_final_byte() {
        let mut t = enabled("1000;1006");
        assert_eq!(t.mouse_coordinates(), Coordinates::Sgr);
        t.mouse(at(Motion::Press, Some(Button::Right), 0, 0));
        assert_eq!(t.take_reply(), b"\x1b[<2;1;1M");
        t.mouse(at(Motion::Release, Some(Button::Right), 0, 0));
        assert_eq!(t.take_reply(), b"\x1b[<2;1;1m");
    }

    #[test]
    fn one_sequence_can_turn_on_both_tracking_and_the_encoding() {
        // Boards ask for them together, and reading only the first parameter
        // would leave one sending positions in an encoding nobody agreed to.
        let t = enabled("1000;1006");
        assert_eq!(t.mouse_tracking(), Tracking::Normal);
        assert_eq!(t.mouse_coordinates(), Coordinates::Sgr);
    }

    #[test]
    fn x10_tracking_reports_presses_and_nothing_else() {
        let mut t = enabled("9");
        assert_eq!(t.mouse_tracking(), Tracking::Press);
        assert!(t.mouse(press(0, 0)));
        t.take_reply();
        assert!(!t.mouse(at(Motion::Release, Some(Button::Left), 0, 0)));
        assert!(!t.mouse(at(Motion::Moved, Some(Button::Left), 1, 0)));
        assert!(t.take_reply().is_empty());
    }

    #[test]
    fn x10_sends_no_modifier_bits() {
        // It has no code to read them. A shift bit set here would be a button
        // the far end has never heard of.
        let mut t = enabled("9");
        let mut event = press(0, 0);
        event.modifiers = Modifiers { shift: true, alt: true, ctrl: true };
        t.mouse(event);
        assert_eq!(t.take_reply(), vec![0x1b, b'[', b'M', 32, 33, 33]);
    }

    #[test]
    fn the_modifier_keys_are_bits_four_eight_and_sixteen() {
        let mut t = enabled("1000;1006");
        let mut event = press(0, 0);
        event.modifiers = Modifiers { shift: true, alt: false, ctrl: true };
        t.mouse(event);
        assert_eq!(t.take_reply(), b"\x1b[<20;1;1M");
    }

    #[test]
    fn a_wheel_is_a_button_that_is_never_released() {
        // Bit six, because the encoding had room for three buttons and a wheel
        // is not one of them.
        let mut t = enabled("1000;1006");
        t.mouse(at(Motion::Press, Some(Button::WheelUp), 2, 3));
        assert_eq!(t.take_reply(), b"\x1b[<64;3;4M");
        t.mouse(at(Motion::Press, Some(Button::WheelDown), 2, 3));
        assert_eq!(t.take_reply(), b"\x1b[<65;3;4M");
    }

    #[test]
    fn dragging_is_reported_only_while_a_button_is_held() {
        let mut t = enabled("1002;1006");
        assert_eq!(t.mouse_tracking(), Tracking::Drag);
        assert!(!t.mouse(at(Motion::Moved, None, 1, 0)), "reported a hover");
        assert!(t.mouse(at(Motion::Moved, Some(Button::Left), 2, 0)));
        // Bit 32 marks it as a move rather than a press.
        assert_eq!(t.take_reply(), b"\x1b[<32;3;1M");
    }

    #[test]
    fn any_event_tracking_reports_a_hover_too() {
        let mut t = enabled("1003;1006");
        assert_eq!(t.mouse_tracking(), Tracking::Any);
        assert!(t.mouse(at(Motion::Moved, None, 1, 0)));
        // Button three and the motion bit: nothing was held.
        assert_eq!(t.take_reply(), b"\x1b[<35;2;1M");
    }

    #[test]
    fn a_pointer_that_has_not_left_its_cell_is_not_news() {
        // The one that decides whether this is usable at 300 bit/s. A mouse
        // moves in pixels and this protocol speaks in cells, so most of what a
        // pointer does is the same answer again.
        let mut t = enabled("1003;1006");
        assert!(t.mouse(at(Motion::Moved, None, 4, 4)));
        t.take_reply();
        assert!(!t.mouse(at(Motion::Moved, None, 4, 4)));
        assert!(t.take_reply().is_empty());
        assert!(t.mouse(at(Motion::Moved, None, 5, 4)));
    }

    #[test]
    fn a_press_in_the_same_cell_is_always_news() {
        // Two clicks in one place are two clicks, however still the pointer
        // was between them.
        let mut t = enabled("1000;1006");
        assert!(t.mouse(press(4, 4)));
        t.take_reply();
        assert!(t.mouse(press(4, 4)));
        assert_eq!(t.take_reply(), b"\x1b[<0;5;5M");
    }

    #[test]
    fn turning_off_dragging_leaves_the_buttons_reported() {
        // The modes are separate flags, not a dial. A board that stops wanting
        // movement has not stopped wanting clicks, and collapsing these into
        // one setting would silence it.
        let mut t = enabled("1000;1002");
        t.feed_bytes(b"\x1b[?1002l");
        assert_eq!(t.mouse_tracking(), Tracking::Normal);
        assert!(t.mouse(press(0, 0)));
    }

    #[test]
    fn turning_tracking_off_stops_the_reports() {
        let mut t = enabled("1000");
        t.feed_bytes(b"\x1b[?1000l");
        assert_eq!(t.mouse_tracking(), Tracking::Off);
        assert!(!t.mouse(press(0, 0)));
        assert!(t.take_reply().is_empty());
    }

    #[test]
    fn a_position_the_original_encoding_cannot_write_is_not_guessed_at() {
        // Past 223 there is no byte left to say the number with. Sending a
        // wrapped one would put the click somewhere the pointer has never
        // been, which is worse than sending nothing.
        let mut t = Terminal::new(300, 24);
        t.feed_bytes(b"\x1b[?1000h");
        assert!(!t.mouse(press(240, 0)));
        assert!(t.take_reply().is_empty());
        // And is exactly what the extended encoding was added for.
        t.feed_bytes(b"\x1b[?1006h");
        assert!(t.mouse(press(240, 0)));
        assert_eq!(t.take_reply(), b"\x1b[<0;241;1M");
    }

    #[test]
    fn a_pointer_past_the_edge_is_still_pointing_at_the_edge() {
        let mut t = enabled("1000;1006");
        t.mouse(press(999, 999));
        assert_eq!(t.take_reply(), b"\x1b[<0;80;24M");
    }

    #[test]
    fn a_reset_stops_the_reporting_as_well_as_clearing_the_screen() {
        let mut t = enabled("1003;1006");
        t.reset();
        assert_eq!(t.mouse_tracking(), Tracking::Off);
        assert_eq!(t.mouse_coordinates(), Coordinates::Legacy);
        assert!(!t.mouse(press(0, 0)));
    }

    /// What the reset button on the window is for.
    ///
    /// A board draws with ANSI and then stops talking -- times the caller out,
    /// drops the line, or simply gets cut off mid-sequence -- and what it
    /// leaves is a terminal halfway through being set up. All of that state is
    /// at this end, so none of it needs the far end's help to undo.
    #[test]
    fn a_reset_puts_back_everything_a_board_can_leave_behind() {
        let mut t = Terminal::new(80, 24);
        // Colour, the cursor parked, wrapping off, the cursor hidden, and then
        // a sequence cut off half way through.
        t.feed_bytes(b"[31;44m[10;40HXYZ[?7l[?25l[6n[1;2");
        t.reset();

        assert_eq!(t.cursor(), (0, 0));
        assert!(t.autowrap, "wrapping stayed off");
        assert!(t.cursor_visible, "the cursor stayed hidden");
        assert!(!t.bell, "a bell rung before the reset");
        assert!(t.take_reply().is_empty(), "answered a question from before it");
        assert_eq!(t.cell(9, 39).ch, ' ', "the screen was not cleared");

        // And the parser is not still waiting for the rest of that sequence,
        // which would swallow whatever the next board sends first.
        t.feed_bytes(b"A");
        assert_eq!(t.cell(0, 0).ch, 'A');
        assert_eq!(t.cell(0, 0).attr, Attr::default(), "the colour survived");
    }

    #[test]
    fn a_board_that_never_reads_cannot_grow_the_queue_without_limit() {
        // The same cap the answerback has, and it matters more here: at 300
        // bit/s a six-byte report takes a fifth of a second to send, and a
        // hand on a mouse produces them faster than that indefinitely.
        let mut t = enabled("1003;1006");
        for i in 0..2000 {
            t.mouse(at(Motion::Moved, None, i % 80, (i / 80) % 24));
        }
        assert!(t.take_reply().len() <= 256);
    }

    #[test]
    fn the_urxvt_encoding_is_decimal_and_still_offset() {
        let mut t = enabled("1000;1015");
        assert_eq!(t.mouse_coordinates(), Coordinates::Urxvt);
        t.mouse(press(0, 0));
        assert_eq!(t.take_reply(), b"\x1b[32;1;1M");
    }
}
