//! The DTE-facing AT command layer (ITU-T V.250).
//!
//! This crate owns command-line assembly, parsing, execution and response
//! formatting. It performs no telephony itself: commands that need the modem to
//! do something emit an [`Action`] for the caller to carry out, which keeps the
//! whole layer testable without audio, a line, or a serial port.

pub mod escape;
pub mod parse;
pub mod registers;
pub mod result;

use parse::{Command, ExtOp, ParseError, parse_body};
use registers::{RegError, Registers};
use result::{Formatter, ResultCode};

/// Something the modem must do that the AT layer cannot do itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// `ATD` — dial. The string is verbatim, dial modifiers included.
    Dial(String),
    /// `ATA` — answer an incoming call.
    Answer,
    /// `ATH0` — go on hook.
    HangUp,
    /// `ATH1` — go off hook without dialling.
    OffHook,
    /// `ATO` — return from online command state to online data state.
    ReturnOnline,
    /// `ATZ<n>` — reset to stored profile `n`.
    ResetProfile(u8),
    /// `AT&F<n>` — restore factory configuration `n`.
    FactoryDefaults(u8),
}

impl Action {
    /// True when the result code arrives later rather than immediately.
    ///
    /// Dialling, answering and returning online all move the DCE out of command
    /// state, so their outcome is reported as CONNECT or NO CARRIER once the
    /// call resolves (V.250 5.7.1). Everything else completes at once: V.250
    /// 6.1.1 is explicit that Z finishes all its work before issuing OK.
    pub fn defers_result(&self) -> bool {
        matches!(self, Self::Dial(_) | Self::Answer | Self::ReturnOnline)
    }

    /// True when the remainder of the command line must not be executed.
    ///
    /// V.250 5.3.1 for A, 6.3.1 for D (the dial string consumes the line), and
    /// 6.1.1 for Z ("commands ... after the Z command ... may be ignored").
    /// `&F` is deliberately absent: `AT&F&C1&D2` is a common initialisation
    /// string and the settings after `&F` must take effect.
    pub fn terminates_line(&self) -> bool {
        matches!(
            self,
            Self::Dial(_) | Self::Answer | Self::ReturnOnline | Self::ResetProfile(_)
        )
    }
}

/// Identification strings reported by `ATI` and the `+G` commands.
#[derive(Debug, Clone)]
pub struct Identity {
    pub manufacturer: String,
    pub model: String,
    pub revision: String,
    pub serial: String,
}

impl Default for Identity {
    fn default() -> Self {
        Self {
            manufacturer: "dialupmodem2".into(),
            model: "SOFTMODEM".into(),
            revision: env!("CARGO_PKG_VERSION").into(),
            serial: "0".into(),
        }
    }
}

/// Settings that survive within a session but are not S-parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// E — echo command characters back to the DTE (V.250 6.2.4).
    pub echo: bool,
    /// X — result code selection and call progress monitoring (V.250 6.2.7).
    pub x: u8,
    /// &C — circuit 109 (DCD) behaviour (V.250 6.2.8).
    pub dcd: u8,
    /// &D — circuit 108 (DTR) behaviour (V.250 6.2.9).
    pub dtr: u8,
    /// Speaker loudness, L (V.250 6.3.13). Stored; this DCE has no speaker.
    pub speaker_volume: u8,
    /// Speaker mode, M (V.250 6.3.14).
    pub speaker_mode: u8,
    /// Whether P or T last selected the default dialling method.
    pub pulse_dialling: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            echo: true,
            x: 4,
            dcd: 1,
            dtr: 2,
            speaker_volume: 2,
            speaker_mode: 1,
            pulse_dialling: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineState {
    /// Waiting for the `A` of a command line prefix.
    Idle,
    /// Seen `A`; expecting `T` or `/`.
    GotA,
    /// Inside the command line body.
    Body,
}

/// V.250 5.2.1 requires at least 40 body characters; we accept far more, and
/// report ERROR once the line is terminated if this is exceeded (V.250 5.5).
const MAX_BODY: usize = 256;

/// The AT command interpreter.
#[derive(Debug)]
pub struct Interpreter {
    pub regs: Registers,
    pub fmt: Formatter,
    pub config: Config,
    pub identity: Identity,
    state: LineState,
    body: Vec<u8>,
    last_body: Vec<u8>,
    overflowed: bool,
    out: Vec<u8>,
    actions: Vec<Action>,
}

impl Default for Interpreter {
    fn default() -> Self {
        Self::new()
    }
}

impl Interpreter {
    pub fn new() -> Self {
        Self {
            regs: Registers::default(),
            fmt: Formatter::default(),
            config: Config::default(),
            identity: Identity::default(),
            state: LineState::Idle,
            body: Vec::new(),
            last_body: Vec::new(),
            overflowed: false,
            out: Vec::new(),
            actions: Vec::new(),
        }
    }

    /// Bytes queued for the DTE. Draining leaves the queue empty.
    pub fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.out)
    }

    /// Actions the completed command line asked for, in the order written.
    ///
    /// Usually empty or a single entry, but a line such as `AT&FH0` legitimately
    /// produces two, so this is a queue rather than a single value.
    pub fn take_actions(&mut self) -> Vec<Action> {
        std::mem::take(&mut self.actions)
    }

    /// Restore the factory configuration (V.250 6.1.1, 6.1.2).
    fn restore_defaults(&mut self) {
        self.regs = Registers::default();
        self.fmt = Formatter::default();
        self.config = Config::default();
    }

    /// Queue an unsolicited or deferred result code, such as `RING` or the
    /// `CONNECT` that follows a successful dial.
    pub fn emit(&mut self, code: ResultCode) {
        self.fmt.result(&code, &self.regs, &mut self.out);
    }

    /// Feed one byte received from the DTE in command state.
    ///
    /// Responses are queued for [`take_output`](Self::take_output) and any
    /// requested actions for [`take_actions`](Self::take_actions).
    pub fn feed(&mut self, byte: u8) {
        // V.250 5.1: only the low seven bits are significant.
        let c = byte & 0x7f;

        match self.state {
            LineState::Idle => {
                if c.eq_ignore_ascii_case(&b'A') {
                    self.state = LineState::GotA;
                    self.echo(c);
                }
                // V.250 5.5: characters that are not part of a properly
                // formatted command line are ignored.
            }
            LineState::GotA => {
                if c.eq_ignore_ascii_case(&b'T') {
                    self.state = LineState::Body;
                    self.body.clear();
                    self.overflowed = false;
                    self.echo(c);
                } else if c == b'/' {
                    // V.250 5.2.4: "A/" immediately repeats the previous line.
                    // No termination character is needed.
                    self.echo(c);
                    self.state = LineState::Idle;
                    let body = self.last_body.clone();
                    self.execute_line(&body);
                } else if c.eq_ignore_ascii_case(&b'A') {
                    self.echo(c);
                } else {
                    self.state = LineState::Idle;
                }
            }
            LineState::Body => self.feed_body(c),
        }
    }

    fn feed_body(&mut self, c: u8) {
        // V.250 5.2.2: S3 is checked before S5, so if they are set to the same
        // value the character terminates the line rather than editing it.
        if c == self.regs.terminator() {
            self.echo(c);
            self.state = LineState::Idle;
            let body = std::mem::take(&mut self.body);
            self.last_body = body.clone();
            if self.overflowed {
                // V.250 5.5: exceeding the maximum body length is reported once
                // the line has been terminated.
                self.emit(ResultCode::Error);
                return;
            }
            self.execute_line(&body);
            return;
        }
        if c == self.regs.editor() {
            self.echo(c);
            self.body.pop();
            return;
        }
        self.echo(c);
        if self.body.len() < MAX_BODY {
            self.body.push(c);
        } else {
            self.overflowed = true;
        }
    }

    fn echo(&mut self, c: u8) {
        // V.250 5.2.3: echo during command state is controlled by E.
        if self.config.echo {
            self.out.push(c);
        }
    }

    fn execute_line(&mut self, body: &[u8]) {
        // V.250 5.2.2: control characters remaining in the line are ignored.
        let text: String = body
            .iter()
            .copied()
            .filter(|b| !(*b < 0x20 || *b == 0x7f))
            .map(char::from)
            .collect();

        // An empty body is legal and simply acknowledges (V.250 5.2.4).
        if text.trim().is_empty() {
            self.emit(ResultCode::Ok);
            return;
        }

        let commands = match parse_body(&text) {
            Ok(c) => c,
            Err(_e) => {
                self.emit(ResultCode::Error);
                return;
            }
        };

        let mut deferred = false;
        for cmd in &commands {
            match self.execute(cmd) {
                Ok(None) => {}
                Ok(Some(action)) => {
                    let stop = action.terminates_line();
                    deferred |= action.defers_result();
                    self.actions.push(action);
                    if stop {
                        break;
                    }
                }
                Err(code) => {
                    // A failed command abandons the rest of the line, and any
                    // actions already queued are discarded with it.
                    self.actions.clear();
                    self.emit(code);
                    return;
                }
            }
        }

        // Commands that leave command state report CONNECT or NO CARRIER when
        // the call resolves; everything else acknowledges now.
        if !deferred {
            self.emit(ResultCode::Ok);
        }
    }

    /// Execute one command. `Err` carries the result code to report.
    fn execute(&mut self, cmd: &Command) -> Result<Option<Action>, ResultCode> {
        match cmd {
            Command::Dial(s) => Ok(Some(Action::Dial(s.clone()))),
            Command::ReadS(n) => {
                let v = self.regs.get(*n).map_err(reg_error)?;
                // V.250 5.3.2: the text is exactly three characters, in decimal
                // with leading zeroes included.
                let text = format!("{v:03}");
                self.fmt.info(&text, &self.regs, &mut self.out);
                Ok(None)
            }
            Command::SetS(n, value) => {
                // V.250 5.3.2 permits treating a missing value as 0 or as an
                // error. Taking it as 0 and letting the range check decide gives
                // both: S0= is accepted, S7= is rejected because 0 is below S7's
                // minimum of 1.
                self.regs.set(*n, value.unwrap_or(0)).map_err(reg_error)?;
                Ok(None)
            }
            Command::Basic { amp, letter, number } => self.basic(*amp, *letter, *number),
            Command::Extended { name, op } => self.extended(name, op),
        }
    }

    fn basic(&mut self, amp: bool, letter: char, number: Option<u32>) -> Result<Option<Action>, ResultCode> {
        // V.250 5.3.1: a missing <number> means zero.
        let n = number.unwrap_or(0);
        let small = u8::try_from(n).map_err(|_| ResultCode::Error)?;

        if amp {
            return match letter {
                // V.250 6.2.8 / 6.2.9.
                'C' if n <= 1 => { self.config.dcd = small; Ok(None) }
                'D' if n <= 2 => { self.config.dtr = small; Ok(None) }
                // V.250 6.1.2. The reset applies here so that later commands
                // on the same line, as in "AT&F&C1&D2", act on the fresh state.
                'F' if n == 0 => {
                    self.restore_defaults();
                    Ok(Some(Action::FactoryDefaults(0)))
                }
                _ => Err(ResultCode::Error),
            };
        }

        match letter {
            // V.250 6.3.5: answer. The rest of the line is ignored.
            'A' => Ok(Some(Action::Answer)),
            // V.250 6.2.4.
            'E' if n <= 1 => { self.config.echo = n == 1; Ok(None) }
            // V.250 6.3.6.
            'H' if n == 0 => Ok(Some(Action::HangUp)),
            'H' if n == 1 => Ok(Some(Action::OffHook)),
            // V.250 6.1.3.
            'I' => { let t = self.identify(small); self.fmt.info(&t, &self.regs, &mut self.out); Ok(None) }
            // V.250 6.3.13 / 6.3.14: accepted and stored; this DCE has no speaker.
            'L' if n <= 3 => { self.config.speaker_volume = small; Ok(None) }
            'M' if n <= 3 => { self.config.speaker_mode = small; Ok(None) }
            // V.250 6.3.7.
            'O' if n == 0 => Ok(Some(Action::ReturnOnline)),
            // V.250 6.3.3 / 6.3.2.
            'P' => { self.config.pulse_dialling = true; Ok(None) }
            'T' => { self.config.pulse_dialling = false; Ok(None) }
            // V.250 6.2.5.
            'Q' if n <= 1 => { self.fmt.quiet = n == 1; Ok(None) }
            // V.250 6.2.6.
            'V' if n <= 1 => { self.fmt.verbose = n == 1; Ok(None) }
            // V.250 6.2.7.
            'X' if n <= 4 => { self.config.x = small; Ok(None) }
            // V.250 6.1.1. The OK that follows must use the new Q, V, S3 and
            // S4 values, which it does because the reset happens here and the
            // result code is formatted afterwards.
            'Z' if n <= 1 => {
                self.restore_defaults();
                Ok(Some(Action::ResetProfile(small)))
            }
            _ => Err(ResultCode::Error),
        }
    }

    fn extended(&mut self, name: &str, op: &ExtOp) -> Result<Option<Action>, ResultCode> {
        // V.250 6.1.4 to 6.1.9. These are all read-only identification actions,
        // so Execute and Read behave alike and Test reports support.
        let value = match name {
            "GMI" => self.identity.manufacturer.clone(),
            "GMM" => self.identity.model.clone(),
            "GMR" => self.identity.revision.clone(),
            "GSN" => self.identity.serial.clone(),
            // V.250 6.1.9: the list of capability commands this DCE supports.
            "GCAP" => "+GCAP: +FCLASS,+MS,+ES,+DS".into(),
            _ => return Err(ResultCode::Error),
        };
        match op {
            ExtOp::Execute | ExtOp::Read => {
                self.fmt.info(&value, &self.regs, &mut self.out);
                Ok(None)
            }
            ExtOp::Test => {
                self.fmt.info(&format!("+{name}: (0)"), &self.regs, &mut self.out);
                Ok(None)
            }
            ExtOp::Set(_) => Err(ResultCode::Error),
        }
    }

    /// `ATI<n>` (V.250 6.1.3). The content of each value is manufacturer-specific.
    fn identify(&self, n: u8) -> String {
        match n {
            0 => self.identity.model.clone(),
            1 => self.identity.revision.clone(),
            2 => self.identity.manufacturer.clone(),
            3 => self.identity.serial.clone(),
            _ => "0".into(),
        }
    }
}

fn reg_error(_e: RegError) -> ResultCode {
    // V.250 5.3.2 and 5.6.2 both call for ERROR.
    ResultCode::Error
}

/// Convenience for tests and callers that want a parse diagnostic.
pub fn parse_line(body: &str) -> Result<Vec<Command>, ParseError> {
    parse_body(body)
}
