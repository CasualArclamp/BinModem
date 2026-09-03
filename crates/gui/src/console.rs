//! The DTE side: a BBS terminal wired to the AT command interpreter.
//!
//! Keystrokes go to the interpreter while in command state and to the line once
//! connected, which is exactly the split a real modem makes. The `+++` escape
//! sequence moves between the two.

use eframe::egui::{Align2, Color32, FontId, Painter, Rect, Sense, Ui, pos2, vec2};

use at::escape::EscapeDetector;
use at::result::ResultCode;
use at::{Action, Interpreter};
use terminal::{Cell, Terminal};

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

/// Paint a terminal, returning the response so the caller can manage focus.
pub fn view(ui: &mut Ui, term: &Terminal, font_size: f32) -> eframe::egui::Response {
    let font = FontId::monospace(font_size);
    let (cols, rows) = term.size();
    let (char_w, row_h) = ui.ctx().fonts_mut(|f| {
        // Every glyph is the same width in a monospace face, so one probe is
        // enough to lay out the whole grid.
        (f.glyph_width(&font, 'M'), f.row_height(&font))
    });

    let size = vec2(char_w * cols as f32, row_h * rows as f32);
    let (response, painter) = ui.allocate_painter(size, Sense::click());
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
    response
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
pub fn keys_to_bytes(ui: &Ui) -> Vec<u8> {
    use eframe::egui::{Event, Key};
    let mut out = Vec::new();
    ui.input(|i| {
        for event in &i.events {
            match event {
                Event::Text(t) => out.extend_from_slice(t.as_bytes()),
                Event::Key { key, pressed: true, modifiers, .. } => {
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
