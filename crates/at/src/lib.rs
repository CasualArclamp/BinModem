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
    /// `AT+MS=` — which modulation to use on the next call (V.250 6.4.1).
    SelectModulation(Modulation),
    /// `AT+ES=` — how error control should be attempted (V.250 6.5.1).
    SelectErrorControl(ErrorControl),
    /// `AT+DS=` — whether to negotiate V.42bis (V.250 6.6.1).
    SelectCompression(Compression),
}

/// What `+MS` asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Modulation {
    /// One of the names in V.250 Table 13, or a manufacturer's own.
    pub carrier: String,
    /// Whether the DCE may fall back to another modulation on its own.
    pub automode: bool,
    /// The lowest and highest line rates the connection may use.
    ///
    /// Zero in either is not a rate. 6.4.1: "if unspecified (set to 0), they
    /// are determined by the modulation means selected in the `<carrier>` and
    /// `<automode>` settings" -- so zero is the absence of a limit, and
    /// anything comparing against these has to know that. An earlier default
    /// of 4800 here was a number nobody had asked for, and it quietly held
    /// V.32 to its slower rate for every terminal that had not said otherwise.
    pub min_rate: u32,
    pub max_rate: u32,
}

/// What `+ES` asked for, in the terms of V.250 Table 20.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ErrorControl {
    /// `<orig_rqst>`: 0 direct, 1 buffered only, 2 V.42 without the detection
    /// phase, 3 V.42 with it.
    pub request: u8,
    /// `<orig_fbk>`: 0 and 1 make error control optional, 2 and above require
    /// it and hang up if it cannot be established.
    pub fallback: u8,
}

impl Default for Modulation {
    fn default() -> Self {
        // V.250 6.4.1: automode on, and both rates unspecified. "If
        // unspecified (set to 0), they are determined by the modulation means
        // selected", so zero is the absence of a limit rather than a slow one.
        Self { carrier: "V22B".into(), automode: true, min_rate: 0, max_rate: 0 }
    }
}

impl ErrorControl {
    /// Whether V.42 should be attempted at all.
    pub fn wanted(self) -> bool {
        self.request >= 2
    }

    /// Whether the detection phase of V.42 7.2.1 should be run.
    ///
    /// Skipping it is what `<orig_rqst>` of 2 means: a DTE that already knows
    /// the far end does V.42 can save the three quarters of a second the
    /// detection phase costs.
    pub fn detect(self) -> bool {
        self.request >= 3
    }

    /// Whether to hang up if error control cannot be established.
    pub fn required(self) -> bool {
        self.fallback >= 2
    }
}

/// What `+DS` asked for, in the terms of V.250 Table 27.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Compression {
    /// `<direction>`: 0 none, 1 transmit only, 2 receive only, 3 both.
    ///
    /// V.42bis P0. Only 0 and 3 are offered: it is negotiated as a pair, and
    /// offering half of it is a promise this DCE cannot keep.
    pub direction: u8,
    /// `<compression_negotiation>`: 1 disconnects if the far end will not.
    pub required: bool,
    /// `<max_dict>`: V.42bis P1, the number of codewords, 512 to 65535.
    pub max_dict: u16,
    /// `<max_string>`: V.42bis P2, the longest string, 6 to 250.
    pub max_string: u8,
}

impl Compression {
    /// Whether compression should be asked for at all.
    pub fn wanted(self) -> bool {
        self.direction != 0
    }
}

impl Default for Compression {
    fn default() -> Self {
        // V.250 6.6.1 leaves the default `<max_dict>` to the manufacturer and
        // points at Appendix II/V.42 bis, which says 2048 outright: "a value
        // for N2 of 2048 provides good compression performance across a wide
        // range of data types".
        //
        // `<max_string>` is a departure. V.250 recommends 6, which is also
        // V.42bis's minimum, and V.42bis 6.4 settles the parameter by taking
        // the lower of the two proposals -- so proposing the floor does not
        // protect anything, it decides the matter for both ends and decides it
        // badly. 250 is the top of the permitted range and a far end that can
        // only manage 6 still gets 6.
        Self { direction: 3, required: false, max_dict: 2048, max_string: 250 }
    }
}

impl Default for ErrorControl {
    fn default() -> Self {
        // V.42 with the detection phase, and a connection without it is still
        // acceptable, which is what almost every modem shipped configured for.
        Self { request: 3, fallback: 0 }
    }
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
    /// +ER — report the error control that was negotiated (V.250 6.5.5).
    pub report_error_control: bool,
    /// +DR — report the compression that was negotiated (V.250 6.6.3).
    pub report_compression: bool,
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
            // V.250 6.5.5 and 6.6.3 both recommend a default of 0. A terminal
            // that wants to be told asks to be told.
            report_error_control: false,
            report_compression: false,
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
    /// Modulations this DCE can actually use, most capable first.
    ///
    /// Held rather than hard-coded because what a DCE can do is a property of
    /// the thing underneath it, and a command interpreter that guessed would
    /// be advertising capabilities the modem does not have. Which is precisely
    /// what `+GCAP` was doing until these commands existed.
    pub modulations: Vec<String>,
    /// What `+MS` last selected.
    pub modulation: Modulation,
    /// What `+ES` last selected.
    pub error_control: ErrorControl,
    /// What `+DS` last selected: whether V.42bis may be negotiated.
    pub compression: Compression,
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
            // What this DCE can originate. Plain V.22 is absent because
            // V.22bis at 1200 bit/s *is* V.22, and answering to both names
            // would be two entries for one thing. V.21 is absent because the
            // tones implemented are Bell 103's, and a modem that claimed V.21
            // and whistled at 1270 Hz would be lying to whoever asked.
            modulations: ["V32", "V22B", "B103"]
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            modulation: Modulation::default(),
            error_control: ErrorControl::default(),
            compression: Compression::default(),
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
    /// `AT&F`: everything V.250 Table 4 marks as restored by it.
    ///
    /// Which is more than the S-parameters and the format. The table's
    /// "factory-defined configuration" column covers +MS, +ES, +DS, +ER and
    /// +DR as well, and leaving those alone made `&F` a reset that reset some
    /// of the modem -- so a session picked up settings from the one before it
    /// and there was no way to get back to a known state short of restarting
    /// the program.
    fn restore_defaults(&mut self) {
        self.regs = Registers::default();
        self.fmt = Formatter::default();
        self.config = Config::default();
        self.modulation = Modulation::default();
        self.error_control = ErrorControl::default();
        self.compression = Compression::default();
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

    /// `+FCLASS` — which service class is in use (V.250 6.1.10).
    ///
    /// Zero is data, and it is the only one here: facsimile is a different
    /// recommendation and this DCE does not implement it.
    fn fclass(&mut self, op: &ExtOp) -> Result<Option<Action>, ResultCode> {
        match op {
            ExtOp::Read => {
                self.fmt.info("+FCLASS: 0", &self.regs, &mut self.out);
                Ok(None)
            }
            ExtOp::Test => {
                self.fmt.info("+FCLASS: (0)", &self.regs, &mut self.out);
                Ok(None)
            }
            ExtOp::Set(v) if v.trim() == "0" => Ok(None),
            _ => Err(ResultCode::Error),
        }
    }

    /// `+MS` — modulation selection (V.250 6.4.1).
    fn modulation_select(&mut self, op: &ExtOp) -> Result<Option<Action>, ResultCode> {
        match op {
            ExtOp::Read => {
                let m = &self.modulation;
                let text = format!(
                    "+MS: {},{},{},{}",
                    m.carrier,
                    u8::from(m.automode),
                    m.min_rate,
                    m.max_rate
                );
                self.fmt.info(&text, &self.regs, &mut self.out);
                Ok(None)
            }
            ExtOp::Test => {
                let text = format!(
                    "+MS: ({}),(0,1),(300-4800),(300-4800)",
                    self.modulations.join(",")
                );
                self.fmt.info(&text, &self.regs, &mut self.out);
                Ok(None)
            }
            ExtOp::Set(value) => {
                let mut parts = value.split(',');
                let carrier = parts.next().unwrap_or("").trim().to_ascii_uppercase();
                if !self.modulations.contains(&carrier) {
                    // V.250 5.4.2: a subparameter outside the range the DCE
                    // reported for it is an error, and reporting one thing in
                    // +MS=? and accepting another is how a DTE ends up
                    // believing a connection is something it is not.
                    return Err(ResultCode::Error);
                }
                let number = |p: Option<&str>, default: u32| -> Result<u32, ResultCode> {
                    match p.map(str::trim) {
                        None | Some("") => Ok(default),
                        Some(v) => v.parse().map_err(|_| ResultCode::Error),
                    }
                };
                let automode = number(parts.next(), 1)?;
                if automode > 1 {
                    return Err(ResultCode::Error);
                }
                // Omitted is unspecified, which is zero, which is no limit.
                let min_rate = number(parts.next(), 0)?;
                let max_rate = number(parts.next(), 0)?;
                if max_rate != 0 && min_rate > max_rate {
                    return Err(ResultCode::Error);
                }
                self.modulation = Modulation {
                    carrier,
                    automode: automode == 1,
                    min_rate,
                    max_rate,
                };
                Ok(Some(Action::SelectModulation(self.modulation.clone())))
            }
            ExtOp::Execute => Err(ResultCode::Error),
        }
    }

    /// `+ES` — error control selection (V.250 6.5.1, Table 20).
    fn error_control_select(&mut self, op: &ExtOp) -> Result<Option<Action>, ResultCode> {
        match op {
            ExtOp::Read => {
                let e = self.error_control;
                let text = format!("+ES: {},{}", e.request, e.fallback);
                self.fmt.info(&text, &self.regs, &mut self.out);
                Ok(None)
            }
            ExtOp::Test => {
                // Only the values this DCE can actually honour. The
                // alternative protocol of 4 is MNP, which is not implemented.
                self.fmt
                    .info("+ES: (0-3),(0-3),(0-3)", &self.regs, &mut self.out);
                Ok(None)
            }
            ExtOp::Set(value) => {
                let mut parts = value.split(',');
                let number = |p: Option<&str>, default: u8| -> Result<u8, ResultCode> {
                    match p.map(str::trim) {
                        None | Some("") => Ok(default),
                        Some(v) => v.parse().map_err(|_| ResultCode::Error),
                    }
                };
                let request = number(parts.next(), 3)?;
                let fallback = number(parts.next(), 0)?;
                if request > 3 || fallback > 3 {
                    return Err(ResultCode::Error);
                }
                self.error_control = ErrorControl { request, fallback };
                Ok(Some(Action::SelectErrorControl(self.error_control)))
            }
            ExtOp::Execute => Err(ResultCode::Error),
        }
    }

    /// `+ER` and `+DR` — reporting parameters (V.250 6.5.5, 6.6.3).
    ///
    /// Identical in shape: one numeric parameter, 0 or 1, off by default. What
    /// differs is only which intermediate result code it lets out, and that is
    /// the caller's business rather than this one's.
    fn reporting(
        &mut self,
        op: &ExtOp,
        name: &str,
        current: bool,
    ) -> Result<Option<bool>, ResultCode> {
        match op {
            ExtOp::Read => {
                let text = format!("+{name}: {}", u8::from(current));
                self.fmt.info(&text, &self.regs, &mut self.out);
                Ok(None)
            }
            ExtOp::Test => {
                self.fmt.info(&format!("+{name}: (0,1)"), &self.regs, &mut self.out);
                Ok(None)
            }
            ExtOp::Set(value) => match value.trim() {
                // V.250 5.4.2.1: an omitted subparameter takes its default,
                // and the recommended default here is 0.
                "" | "0" => Ok(Some(false)),
                "1" => Ok(Some(true)),
                _ => Err(ResultCode::Error),
            },
            ExtOp::Execute => Err(ResultCode::Error),
        }
    }

    /// `+DS` — data compression selection (V.250 6.6.1, Table 27).
    fn compression_select(&mut self, op: &ExtOp) -> Result<Option<Action>, ResultCode> {
        match op {
            ExtOp::Read => {
                let c = self.compression;
                let text = format!(
                    "+DS: {},{},{},{}",
                    c.direction,
                    u8::from(c.required),
                    c.max_dict,
                    c.max_string
                );
                self.fmt.info(&text, &self.regs, &mut self.out);
                Ok(None)
            }
            ExtOp::Test => {
                // Only what this DCE can actually honour. The one-directional
                // values of 1 and 2 are absent because V.42bis is negotiated
                // as a pair and offering half of it would be a promise this
                // DCE cannot keep; the ranges are V.42bis 6.4's own.
                self.fmt.info(
                    "+DS: (0,3),(0,1),(512-65535),(6-250)",
                    &self.regs,
                    &mut self.out,
                );
                Ok(None)
            }
            ExtOp::Set(value) => {
                let mut parts = value.split(',');
                let field = |p: Option<&str>| -> Option<Option<u32>> {
                    match p.map(str::trim) {
                        // 5.4.2.1: an omitted subparameter keeps its value.
                        None | Some("") => Some(None),
                        Some(v) => v.parse().ok().map(Some),
                    }
                };
                let mut next = || field(parts.next()).ok_or(ResultCode::Error);
                let direction = next()?;
                let required = next()?;
                let max_dict = next()?;
                let max_string = next()?;
                if parts.next().is_some() {
                    return Err(ResultCode::Error);
                }

                let mut c = self.compression;
                if let Some(d) = direction {
                    // Table 27: 0 is no compression, 3 is both directions.
                    if d != 0 && d != 3 {
                        return Err(ResultCode::Error);
                    }
                    c.direction = d as u8;
                }
                if let Some(r) = required {
                    if r > 1 {
                        return Err(ResultCode::Error);
                    }
                    c.required = r == 1;
                }
                if let Some(n) = max_dict {
                    // V.42bis 6.4: "P1 shall have a default value of 512,
                    // which is its minimum value ... any attempt to specify
                    // less than the minimum value shall be considered a
                    // procedural error".
                    c.max_dict = u16::try_from(n)
                        .ok()
                        .filter(|n| *n >= 512)
                        .ok_or(ResultCode::Error)?;
                }
                if let Some(n) = max_string {
                    // 6.4 again: "the permitted range is from 6 to 250. The
                    // values outside this range are invalid".
                    c.max_string = u8::try_from(n)
                        .ok()
                        .filter(|n| (6..=250).contains(n))
                        .ok_or(ResultCode::Error)?;
                }
                self.compression = c;
                Ok(Some(Action::SelectCompression(c)))
            }
            ExtOp::Execute => Err(ResultCode::Error),
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
            // Everything named here is answered below; a DCE that lists a
            // command it does not implement is worse than one that lists
            // nothing, because a DTE will believe it.
            "GCAP" => "+GCAP: +FCLASS,+MS,+ES,+ER,+DS,+DR".into(),
            "FCLASS" => return self.fclass(op),
            "MS" => return self.modulation_select(op),
            "ES" => return self.error_control_select(op),
            "DS" => return self.compression_select(op),
            // V.250 6.5.5 and 6.6.3. The same parameter twice over: one bit,
            // defaulting to off, saying whether the DCE should report what it
            // negotiated with the far end before it says CONNECT.
            "ER" => {
                if let Some(on) = self.reporting(op, "ER", self.config.report_error_control)? {
                    self.config.report_error_control = on;
                }
                return Ok(None);
            }
            "DR" => {
                if let Some(on) = self.reporting(op, "DR", self.config.report_compression)? {
                    self.config.report_compression = on;
                }
                return Ok(None);
            }
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
