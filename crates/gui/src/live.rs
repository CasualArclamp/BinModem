//! A real modem on a real line, published to the scope.
//!
//! The sibling of [`crate::engine`], which replays a capture. The loop here has
//! the same shape and does the same publishing; what differs is where the
//! samples come from and that there is somewhere for them to go back to. A
//! capture is a recording of somebody else's call and can only be watched. This
//! is a call of our own, and the terminal on the other side of the window is
//! the DTE: what is typed there goes to `feed_dte` and is answered by the AT
//! interpreter inside the modem, exactly as if it had arrived down a wire.
//!
//! The line is clocked by the input device. A sample arrives, the modem takes
//! one step, and what it hands back goes out. Nothing here paces itself against
//! a wall clock, because the sound card already is one.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use dsp::Spectrum;
use line::AudioSink;
use modem::{Modem, Role, State};
use telemetry::{CallState, Direction, Leds, Publisher};

use crate::engine::{Control, FFT_SIZE, Ring, SCOPE_LEN, SPECTRUM_BINS, SYMBOL_HISTORY};

/// The rate the modem runs at, whatever the sound card is doing.
///
/// Enough for a 3400 Hz channel several times over, and the rate every data
/// pump in this workspace has been tested at. The device's own rate is
/// converted to and from this inside [`line::Duplex`].
const FS: f64 = 16_000.0;

/// What the terminal has typed and the line has not yet taken.
///
/// The UI thread writes and the line thread reads. A mutex rather than a
/// channel because the interesting operation is "take everything", and because
/// a keystroke queue that has fallen behind is a bug rather than a thing to
/// buffer around.
#[derive(Debug, Default)]
pub struct Keyboard {
    typed: Mutex<Vec<u8>>,
}

impl Keyboard {
    pub fn type_bytes(&self, bytes: &[u8]) {
        if let Ok(mut q) = self.typed.lock() {
            q.extend_from_slice(bytes);
        }
    }

    fn take(&self) -> Vec<u8> {
        self.typed.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default()
    }
}

/// Open the line and start a modem on it.
///
/// Both device names are required and neither defaults. The default output on
/// a desktop machine is whatever the speakers are plugged into, and a modem
/// handshake played through speakers is both useless and unpleasant.
pub fn spawn(
    input: String,
    output: String,
    tx: Publisher,
    control: Arc<Control>,
    keyboard: Arc<Keyboard>,
    sink: Arc<AudioSink>,
) -> Result<JoinHandle<()>, String> {
    // The audio streams are opened on the thread that will own them, because
    // that is where they have to live; the result comes back here so a bad
    // device name is an error at start-up rather than a silent nothing.
    let (ready, opened) = std::sync::mpsc::channel();
    let handle = thread::spawn(move || {
        let audio = match line::Duplex::open(Some(&input), Some(&output), FS) {
            Ok(a) => {
                let _ = ready.send(Ok(format!(
                    "line open: out {} at {} Hz, in {} at {} Hz",
                    a.output_device, a.output_rate, a.input_device, a.input_rate
                )));
                a
            }
            Err(e) => {
                let _ = ready.send(Err(e));
                return;
            }
        };
        run(audio, tx, control, keyboard, sink);
    });

    match opened.recv() {
        Ok(Ok(_note)) => Ok(handle),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("the line thread stopped before it opened anything".into()),
    }
}

fn run(
    audio: line::Duplex,
    tx: Publisher,
    control: Arc<Control>,
    keyboard: Arc<Keyboard>,
    sink: Arc<AudioSink>,
) {
    tx.log(
        Direction::Note,
        format!(
            "line open: out {} at {} Hz, in {} at {} Hz",
            audio.output_device, audio.output_rate, audio.input_device, audio.input_rate
        ),
    );
    tx.log(Direction::Note, "type AT commands; ATD to dial, +++ to escape");

    let mut modem = Modem::new(FS);
    let mut spectrum = Spectrum::new(FFT_SIZE, FS);
    let mut waveform = Ring::new(SCOPE_LEN);
    let mut baseband = Ring::new(SCOPE_LEN);
    let mut bins = vec![0.0f64; SPECTRUM_BINS];
    let mut symbols: std::collections::VecDeque<f32> =
        std::collections::VecDeque::with_capacity(SYMBOL_HISTORY);
    let mut points: std::collections::VecDeque<(f32, f32)> =
        std::collections::VecDeque::with_capacity(SYMBOL_HISTORY);

    let mut from_line: Vec<f32> = Vec::with_capacity(4096);
    let mut to_line: Vec<f32> = Vec::with_capacity(4096);
    let mut rx_bytes = 0u64;
    let mut tx_bytes = 0u64;
    let mut typed_recently = Instant::now() - Duration::from_secs(1);
    let mut heard_recently = typed_recently;
    let mut last_state = State::Command;

    let publish_every = Duration::from_millis(16);
    let mut next_publish = Instant::now();

    while !control.quit.load(Ordering::Relaxed) {
        // Everything the terminal has typed since last time. This goes in
        // whether or not the line has samples for us: a modem answers `AT`
        // with `OK` while completely idle, and a terminal that had to wait for
        // audio before its own modem would talk to it would feel broken.
        let typed = keyboard.take();
        if !typed.is_empty() {
            typed_recently = Instant::now();
            tx_bytes += typed.len() as u64;
            for b in &typed {
                modem.feed_dte(*b);
            }
        }

        from_line.clear();
        audio.receive(&mut from_line);
        if from_line.is_empty() {
            // Nothing has arrived, so nothing can be stepped: the line is the
            // clock. Still hand the terminal whatever the modem said in the
            // meantime, which is how `OK` gets back before a call exists.
            drain_dte(&mut modem, &tx, &mut rx_bytes, &mut heard_recently);
            thread::sleep(Duration::from_millis(2));
            continue;
        }

        to_line.clear();
        for &s in &from_line {
            let heard = f64::from(s);
            to_line.push(modem.step(heard) as f32);

            if let Some(sym) = modem.take_symbol() {
                if symbols.len() == SYMBOL_HISTORY {
                    symbols.pop_front();
                }
                symbols.push_back(sym as f32);
            }
            if let Some(p) = modem.constellation_point() {
                let p = (p.0 as f32, p.1 as f32);
                // Only when it moves, so a motionless constellation is not
                // filled with copies of one point.
                if points.back() != Some(&p) {
                    if points.len() == SYMBOL_HISTORY {
                        points.pop_front();
                    }
                    points.push_back(p);
                }
            }
            baseband.push(modem.discriminator().unwrap_or(0.0) as f32);
            spectrum.push(heard);
            waveform.push(s);
        }
        audio.transmit(&to_line);
        // What the monitor plays is what the modem heard, so the ear and the
        // scopes are looking at the same thing.
        sink.push(&from_line);

        drain_dte(&mut modem, &tx, &mut rx_bytes, &mut heard_recently);

        let state = modem.state();
        if state != last_state {
            let note = match state {
                State::Command => "on hook".to_owned(),
                State::Handshaking => "handshaking".to_owned(),
                // The one moment worth reporting in full. Everything here is
                // settled by then and none of it is visible from the terminal
                // side, which sees a CONNECT and a rate and nothing else.
                State::Data => {
                    let mut s = format!(
                        "connected: {} at {} bit/s, error control {}",
                        modem.standard(),
                        modem.rate().unwrap_or(0),
                        if modem.error_controlled() { "on" } else { "off" }
                    );
                    if let Some(r) = modem.reflection() {
                        s.push_str(&format!(
                            ", echo found {:.0} ms away at {:.2} of the line",
                            r.delay as f64 / FS * 1000.0,
                            r.strength
                        ));
                    }
                    s
                }
                State::OnlineCommand => "escaped to command state".to_owned(),
            };
            tx.log(Direction::Note, note);
            last_state = state;
        }

        if Instant::now() >= next_publish {
            next_publish = Instant::now() + publish_every;
            if spectrum.ready() {
                spectrum.magnitudes_db(&mut bins);
            }
            let level = {
                let n = waveform.data.len().max(1);
                (waveform.data.iter().map(|v| f64::from(*v) * f64::from(*v)).sum::<f64>()
                    / n as f64)
                    .sqrt()
            };
            let rate = modem.rate();
            let recent = |at: Instant| at.elapsed() < Duration::from_millis(250);

            tx.publish(|f| {
                f.sample_rate = FS;
                waveform.copy_into(&mut f.waveform);
                baseband.copy_into(&mut f.baseband);
                for (slot, &v) in f.spectrum_db.iter_mut().zip(bins.iter()) {
                    *slot = v as f32;
                }
                f.hz_per_bin = FS / FFT_SIZE as f64;
                f.rx_level_db = (20.0 * (level + 1e-9).log10()) as f32;
                f.carrier = modem.carrier();
                f.state = match state {
                    State::Command if modem.off_hook() => CallState::OffHook,
                    State::Command => CallState::Idle,
                    State::Handshaking => CallState::Negotiating,
                    State::Data | State::OnlineCommand => CallState::Connected,
                };
                f.modulation = modem.standard();
                f.bit_rate = rate;
                f.rx_bytes = rx_bytes;
                f.tx_bytes = tx_bytes;
                f.tones = modem.states();
                f.symbol_label = modem.shape();
                f.snr_db = modem.residual_error().map(|e| {
                    // The decision margin as decibels, so a tighter
                    // constellation reads as a larger number.
                    (-20.0 * e.max(1e-3).log10()) as f32
                });
                f.symbols.clear();
                f.symbols.extend(symbols.iter().copied());
                f.constellation.clear();
                f.constellation.extend(points.iter().copied());
                f.leds = Leds {
                    mr: true,
                    tr: true,
                    sd: recent(typed_recently),
                    rd: recent(heard_recently),
                    cd: modem.carrier(),
                    oh: modem.off_hook(),
                    aa: modem.role() == Role::Answering,
                    hs: rate.is_some_and(|r| r >= 9600),
                    ec: modem.error_controlled(),
                };
            });
        }
    }

    let lost = audio.dropped_in();
    if lost > 0 {
        tx.log(
            Direction::Note,
            format!("{lost} samples lost coming in: the line outran the modem"),
        );
    }
}

/// Hand the terminal everything the modem has to say.
fn drain_dte(modem: &mut Modem, tx: &Publisher, rx_bytes: &mut u64, heard: &mut Instant) {
    let out = modem.take_dte();
    if out.is_empty() {
        return;
    }
    *rx_bytes += out.len() as u64;
    *heard = Instant::now();
    tx.line_data(&out);
}
