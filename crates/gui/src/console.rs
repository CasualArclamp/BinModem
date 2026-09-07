//! The DTE side: a BBS terminal wired to the AT command interpreter.
//!
//! Keystrokes go to the interpreter while in command state and to the line once
//! connected, which is exactly the split a real modem makes. The `+++` escape
//! sequence moves between the two.

use eframe::egui::{
    Align2, Color32, FontId, Painter, Pos2, Rect, Sense, Ui, Vec2, pos2, vec2,
};

use at::escape::EscapeDetector;
use at::result::ResultCode;
use at::{Action, Interpreter};
use terminal::{Button, Cell, Modifiers, Motion, Mouse, Terminal, Tracking};

/// The IBM PC / ANSI.SYS 16-colour palette, which is what BBS art was drawn
/// against. Using a modern terminal palette here makes period art look wrong.
pub const PALETTE: [Color32; 16] = [
    Color32::from_rgb(0, 0, 0),       // 0 black
    Color32::from_rgb(170, 0, 0),     // 1 red
    Color32::from_rgb(0, 170, 0),     // 2 green
    Color32::from_rgb(170, 85, 0),    // 3 brown
    Color32::from_rgb(0, 0, 170),     // 4 blue
    Color32::from_rgb(170, 0, 170),   // 5 magenta
    Color32::from_rgb(0, 170, 170),   // 6 cyan
    Color32::from_rgb(170, 170, 170), // 7 light grey
    Color32::from_rgb(85, 85, 85),    // 8 dark grey
    Color32::from_rgb(255, 85, 85),   // 9 bright red
    Color32::from_rgb(85, 255, 85),   // 10 bright green
    Color32::from_rgb(255, 255, 85),  // 11 yellow
    Color32::from_rgb(85, 85, 255),   // 12 bright blue
    Color32::from_rgb(255, 85, 255),  // 13 bright magenta
    Color32::from_rgb(85, 255, 255),  // 14 bright cyan
    Color32::from_rgb(255, 255, 255), // 15 white
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Accepting AT commands.
    Command,
    /// Connected: keystrokes go to the line.
    Online,
}

pub struct Console {
    pub term: Terminal,
    at: Interpreter,
    pub mode: Mode,
    escape: EscapeDetector,
    /// Bytes waiting to be sent to the far end.
    pending_tx: Vec<u8>,
}

impl Default for Console {
    fn default() -> Self {
        Self::new()
    }
}

impl Console {
    /// A console for a live line, whose modem answers for itself.
    pub fn live() -> Self {
        let mut console = Self::new();
        console.term = Terminal::new(terminal::DEFAULT_COLS, terminal::DEFAULT_ROWS);
        console.term.feed_bytes(b"dialupmodem2\r\n");
        console.term.feed_bytes(
            b"Open a line above, then AT+MS to choose a modulation and ATD or ATA."
        );
        console.term.feed_bytes(b"\r\n\r\n");
        console
    }

    /// A console onto a board over a socket, with no modem between.
    ///
    /// The same terminal, fed the same bytes, with everything that could have
    /// mangled them on the way taken out. What is on this screen is exactly
    /// what the board sent, so anything wrong with it is this code.
    pub fn telnet() -> Self {
        let mut console = Self::new();
        console.term = Terminal::new(terminal::DEFAULT_COLS, terminal::DEFAULT_ROWS);
        console.term.feed_bytes(b"dialupmodem2 - terminal over telnet\r\n");
        console.term.feed_bytes(b"No modem and no line: every byte arrives.\r\n");
        console.term.feed_bytes(b"Pick a board above and connect.\r\n\r\n");
        console
    }

    pub fn new() -> Self {
        let mut at = Interpreter::new();
        // A terminal renders the echo itself only if the modem sends it, so
        // leave E1 as the Recommendation's default and let the DCE echo.
        at.identity.model = "DIALUPMODEM2".into();
        let mut term = Terminal::new(terminal::DEFAULT_COLS, terminal::DEFAULT_ROWS);
        term.feed_bytes(b"dialupmodem2 console\r\n");
        term.feed_bytes(b"AT commands accepted. ATD to replay the capture.\r\n\r\n");
        Self {
            term,
            at,
            mode: Mode::Command,
            escape: EscapeDetector::new(),
            pending_tx: Vec::new(),
        }
    }

    /// Bytes typed at the keyboard. Returns any actions the modem must perform.
    pub fn typed(&mut self, bytes: &[u8]) -> Vec<Action> {
        match self.mode {
            Mode::Command => {
                for &b in bytes {
                    self.at.feed(b);
                }
                let out = self.at.take_output();
                self.term.feed_bytes(&out);
                self.at.take_actions()
            }
            Mode::Online => {
                // Watch for the escape sequence, then queue for transmission.
                for &b in bytes {
                    self.escape.data(b, &self.at.regs);
                }
                self.pending_tx.extend_from_slice(bytes);
                Vec::new()
            }
        }
    }

    /// Advance the escape-sequence guard timers with no data sent.
    ///
    /// Returns true when the sequence completes and the modem should return to
    /// command state without dropping the call.
    pub fn idle(&mut self, dt_ms: u32) -> bool {
        if self.mode != Mode::Online {
            return false;
        }
        if self.escape.idle(dt_ms, &self.at.regs) {
            self.mode = Mode::Command;
            self.emit(ResultCode::Ok);
            true
        } else {
            false
        }
    }

    /// Take whatever is queued for the far end.
    pub fn take_tx(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.pending_tx)
    }

    /// Bytes recovered from the far end. Rendered only while connected.
    pub fn line_rx(&mut self, bytes: &[u8]) {
        if self.mode == Mode::Online {
            self.term.feed_bytes(bytes);
        }
    }

    /// Put bytes on the screen whatever state this console thinks it is in.
    ///
    /// For a live call there is no state to be in here. The modem on the other
    /// side of the window owns the command and online distinction and answers
    /// for itself, right down to echoing what was typed, so everything it says
    /// goes to the screen exactly as it arrives and nothing is interpreted on
    /// the way.
    pub fn feed_screen(&mut self, bytes: &[u8]) {
        self.term.feed_bytes(bytes);
    }

    /// Follow a modem that is keeping the state instead of keeping it here.
    pub fn follow(&mut self, online: bool) {
        self.mode = if online { Mode::Online } else { Mode::Command };
    }

    /// Report a completed connection and enter online state.
    pub fn connect(&mut self, detail: &str) {
        self.emit(ResultCode::ConnectText(detail.into()));
        self.mode = Mode::Online;
        self.escape.reset();
    }

    /// Report a lost or refused call and return to command state.
    pub fn disconnect(&mut self, code: ResultCode) {
        self.emit(code);
        self.mode = Mode::Command;
        self.escape.reset();
    }

    /// Return to online state after an escape, as ATO does.
    pub fn resume_online(&mut self) {
        self.mode = Mode::Online;
        self.escape.reset();
    }

    fn emit(&mut self, code: ResultCode) {
        self.at.emit(code);
        let out = self.at.take_output();
        self.term.feed_bytes(&out);
    }

    /// Write text straight to the screen, for local notices.
    pub fn notice(&mut self, text: &str) {
        self.term.feed_bytes(b"\r\n");
        self.term.feed_bytes(text.as_bytes());
        self.term.feed_bytes(b"\r\n");
    }
}

/// A painted terminal, and what the pointer did over it.
///
/// The mouse comes back rather than being applied here because painting takes
/// the screen by shared reference and reporting changes it. Handing the events
/// to the caller keeps the one function that draws from also being a function
/// that writes.
#[derive(Debug)]
pub struct View {
    pub response: eframe::egui::Response,
    /// What the pointer did, in cells, in the order it did it. Empty unless
    /// the far end has asked to be told.
    pub mouse: Vec<Mouse>,
}

/// Paint a terminal, returning the response so the caller can manage focus.
pub fn view(ui: &mut Ui, term: &Terminal, font_size: f32) -> View {
    let font = FontId::monospace(font_size);
    let (cols, rows) = term.size();
    let (char_w, row_h) = ui.ctx().fonts_mut(|f| {
        // Every glyph is the same width in a monospace face, so one probe is
        // enough to lay out the whole grid.
        (f.glyph_width(&font, 'M'), f.row_height(&font))
    });

    let size = vec2(char_w * cols as f32, row_h * rows as f32);
    // Dragging is only claimed when somebody is listening for it. Left on
    // permanently it would take the drag away from the scroll area this sits
    // in, so a screen too large for the window could no longer be pushed
    // around with the mouse.
    let tracking = term.mouse_tracking();
    let sense = if tracking == Tracking::Off {
        Sense::click()
    } else {
        Sense::click_and_drag()
    };
    let (response, painter) = ui.allocate_painter(size, sense);
    let origin = response.rect.min;
    painter.rect_filled(response.rect, 0.0, PALETTE[0]);

    for row in 0..rows {
        let cells = term.row_cells(row);
        let y = origin.y + row_h * row as f32;
        let mut col = 0usize;
        while col < cols {
            // Batch the longest run sharing one attribute into a single draw:
            // a full screen is 1920 cells and per-cell painting is wasteful.
            let (fg, bg) = cells[col].attr.resolved();
            let mut end = col + 1;
            while end < cols && cells[end].attr.resolved() == (fg, bg) {
                end += 1;
            }
            let x = origin.x + char_w * col as f32;
            let run = Rect::from_min_size(pos2(x, y), vec2(char_w * (end - col) as f32, row_h));
            if bg != 0 {
                painter.rect_filled(run, 0.0, PALETTE[bg as usize & 15]);
            }
            let text: String = cells[col..end].iter().map(|c: &Cell| c.ch).collect();
            if text.trim().is_empty() {
                col = end;
                continue;
            }
            painter.text(
                pos2(x, y),
                Align2::LEFT_TOP,
                text,
                font.clone(),
                PALETTE[fg as usize & 15],
            );
            col = end;
        }
    }

    if term.cursor_visible {
        let (r, c) = term.cursor();
        let cursor = Rect::from_min_size(
            pos2(origin.x + char_w * c as f32, origin.y + row_h * r as f32),
            vec2(char_w, row_h),
        );
        // A hollow box rather than a solid block, so the character under the
        // cursor stays readable.
        painter.rect_stroke(
            cursor,
            0.0,
            eframe::egui::Stroke::new(1.0, PALETTE[7]),
            eframe::egui::StrokeKind::Inside,
        );
    }

    if !response.has_focus() {
        hint(&painter, response.rect);
    }

    let mouse = if tracking == Tracking::Off {
        Vec::new()
    } else {
        gather(ui, response.rect, char_w, row_h, cols, rows)
    };
    View { response, mouse }
}

/// Turn what the pointer did into cells.
///
/// Read from the raw event stream rather than from the response, because the
/// response reports the gestures egui recognises -- a click, a drag -- and what
/// a board wants is the buttons and the movement underneath them. Everything
/// outside the screen is dropped here; everything else is handed on, and the
/// terminal decides which of it the far end actually asked for.
fn gather(
    ui: &Ui,
    rect: Rect,
    char_w: f32,
    row_h: f32,
    cols: usize,
    rows: usize,
) -> Vec<Mouse> {
    use eframe::egui::{Event, MouseWheelUnit, PointerButton};

    let cell = |pos: Pos2| -> Option<(usize, usize)> {
        if !rect.contains(pos) {
            return None;
        }
        let col = ((pos.x - rect.min.x) / char_w) as usize;
        let row = ((pos.y - rect.min.y) / row_h) as usize;
        Some((col.min(cols - 1), row.min(rows - 1)))
    };
    let button = |b: PointerButton| match b {
        PointerButton::Primary => Some(Button::Left),
        PointerButton::Middle => Some(Button::Middle),
        PointerButton::Secondary => Some(Button::Right),
        // The back and forward buttons, which this protocol has no number for.
        _ => None,
    };
    let mods = |m: &eframe::egui::Modifiers| Modifiers {
        shift: m.shift,
        alt: m.alt,
        ctrl: m.ctrl,
    };

    let mut out = Vec::new();
    let mut wheeled = false;
    ui.input(|i| {
        let mut held = if i.pointer.button_down(PointerButton::Primary) {
            Some(Button::Left)
        } else if i.pointer.button_down(PointerButton::Middle) {
            Some(Button::Middle)
        } else if i.pointer.button_down(PointerButton::Secondary) {
            Some(Button::Right)
        } else {
            None
        };
        for event in &i.events {
            match event {
                Event::PointerButton { pos, button: b, pressed, modifiers } => {
                    let Some(b) = button(*b) else { continue };
                    held = if *pressed { Some(b) } else { None };
                    let Some((col, row)) = cell(*pos) else { continue };
                    out.push(Mouse {
                        motion: if *pressed { Motion::Press } else { Motion::Release },
                        button: Some(b),
                        col,
                        row,
                        modifiers: mods(modifiers),
                    });
                }
                Event::PointerMoved(pos) => {
                    let Some((col, row)) = cell(*pos) else { continue };
                    out.push(Mouse {
                        motion: Motion::Moved,
                        button: held,
                        col,
                        row,
                        modifiers: mods(&i.modifiers),
                    });
                }
                Event::MouseWheel { unit, delta, modifiers, .. } => {
                    let Some(pos) = i.pointer.hover_pos() else { continue };
                    let Some((col, row)) = cell(pos) else { continue };
                    if delta.y == 0.0 {
                        continue;
                    }
                    // A wheel is a button in this protocol, so a scroll has to
                    // become a whole number of presses. A trackpad reports
                    // pixels and would otherwise produce dozens of them for
                    // one flick of two fingers.
                    let notches = match unit {
                        MouseWheelUnit::Line => delta.y.abs().round(),
                        MouseWheelUnit::Page => rows as f32,
                        MouseWheelUnit::Point => (delta.y.abs() / row_h).round(),
                    };
                    let notches = (notches as usize).clamp(1, 5);
                    let b = if delta.y > 0.0 { Button::WheelUp } else { Button::WheelDown };
                    for _ in 0..notches {
                        out.push(Mouse {
                            motion: Motion::Press,
                            button: Some(b),
                            col,
                            row,
                            modifiers: mods(modifiers),
                        });
                    }
                    wheeled = true;
                }
                _ => {}
            }
        }
    });

    // Taken rather than shared: a board that asked for the wheel is using it
    // to scroll something of its own, and having the pane underneath scroll at
    // the same time would move the screen out from under the pointer.
    if wheeled {
        ui.input_mut(|i| i.smooth_scroll_delta = Vec2::ZERO);
    }
    out
}

fn hint(painter: &Painter, rect: Rect) {
    painter.text(
        pos2(rect.center().x, rect.bottom() - 6.0),
        Align2::CENTER_BOTTOM,
        "click to type",
        FontId::proportional(11.0),
        Color32::from_rgb(120, 125, 140),
    );
}

/// Translate egui keyboard events into the bytes a DTE would send.
/// What a pasted string puts on the line.
///
/// Line endings, and only line endings. A terminal ends a line with a carriage
/// return, and what is on a clipboard is whatever the machine it was copied
/// from uses -- so a paste from anywhere but a terminal arrives as line feeds,
/// and a board reading it sees one enormous line that never ends. Everything
/// else goes through untouched, including escape sequences: a terminal sends
/// what it is given, and deciding otherwise here would be this program editing
/// somebody's message on the way past.
pub fn paste_bytes(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                // A CRLF is one line ending, not two.
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push(b'\r');
            }
            '\n' => out.push(b'\r'),
            _ => {
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
    out
}

pub fn keys_to_bytes(ui: &Ui) -> Vec<u8> {
    use eframe::egui::{Event, Key};
    let mut out = Vec::new();
    ui.input(|i| {
        for event in &i.events {
            match event {
                Event::Text(t) => out.extend_from_slice(t.as_bytes()),
                Event::Paste(text) => out.extend_from_slice(&paste_bytes(text)),
                Event::Key { key, pressed: true, modifiers, .. } => {
                    // Ctrl+V is a paste and not a SYN. A terminal would send
                    // 0x16 for it, and egui delivers the paste as well -- so
                    // without this a person pasting into a login prompt sends
                    // a control character in front of it, and no board has
                    // ever wanted 0x16. Ctrl+C is left alone, because on a
                    // board it is the way to stop a listing and there is
                    // nothing else it could mean here.
                    if modifiers.ctrl && *key == Key::V {
                        continue;
                    }
                    // Control codes are not delivered as Text, so build them here.
                    if modifiers.ctrl && let Some(c) = key.name().chars().next() {
                        let c = c.to_ascii_uppercase();
                        if c.is_ascii_uppercase() {
                            out.push(c as u8 - b'A' + 1);
                            continue;
                        }
                    }
                    match key {
                        Key::Enter => out.push(b'\r'),
                        Key::Backspace => out.push(0x08),
                        Key::Tab => out.push(b'\t'),
                        Key::Escape => out.push(0x1b),
                        Key::Delete => out.push(0x7f),
                        // Cursor keys as ANSI, which is what boards expect.
                        Key::ArrowUp => out.extend_from_slice(b"\x1b[A"),
                        Key::ArrowDown => out.extend_from_slice(b"\x1b[B"),
                        Key::ArrowRight => out.extend_from_slice(b"\x1b[C"),
                        Key::ArrowLeft => out.extend_from_slice(b"\x1b[D"),
                        Key::Home => out.extend_from_slice(b"\x1b[H"),
                        Key::End => out.extend_from_slice(b"\x1b[F"),
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::paste_bytes;

    /// A clipboard is not a keyboard, and its line endings are not a terminal's.
    ///
    /// Whatever a paste was copied from decides how its lines end, and only one
    /// of the three ways is what goes down a line to a board. A message pasted
    /// into a board's editor with bare line feeds in it is one line as far as
    /// the board is concerned, however it looked in the window it came from.
    #[test]
    fn a_paste_ends_its_lines_the_way_a_terminal_does() {
        assert_eq!(paste_bytes("one\r\ntwo"), b"one\rtwo");
        assert_eq!(paste_bytes("one\ntwo"), b"one\rtwo");
        assert_eq!(paste_bytes("one\rtwo"), b"one\rtwo");
        // A trailing one is still one, and an empty line in the middle stays.
        assert_eq!(paste_bytes("a\r\n\r\nb\r\n"), b"a\r\rb\r");
    }

    /// Everything else goes through as it was.
    ///
    /// Including escape sequences. A terminal sends what it is handed, and
    /// filtering here would be this program quietly editing somebody's message
    /// on its way to the line.
    #[test]
    fn a_paste_is_otherwise_left_alone() {
        assert_eq!(paste_bytes("\x1b[31mred\x1b[0m"), b"\x1b[31mred\x1b[0m");
        assert_eq!(paste_bytes("\t \x07"), b"\t \x07");
        // Not everything a clipboard holds is ASCII.
        assert_eq!(paste_bytes("caf\u{e9}"), "caf\u{e9}".as_bytes());
    }
}
