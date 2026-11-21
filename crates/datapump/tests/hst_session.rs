//! Two HST ends in a call: MNP over both channels, and the handover.
//!
//! The line is `hst_duplex.rs`'s: loss, hybrid echo and noise on a two-wire
//! pair, each end hearing the other and itself.

use std::f64::consts::TAU;

use datapump::hst::session::Event;
use datapump::hst::{BackRate, Config, Holding, Rate, Session, Side};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn gaussian(&mut self) -> f64 {
        let u1 = self.unit().max(1e-18);
        (-2.0 * u1.ln()).sqrt() * (TAU * self.unit()).cos()
    }
    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| (self.next() >> 56) as u8).collect()
    }
}

const FS: f64 = 16_000.0;
/// Noise for the fallback tests: too much for 14 400.
const NOISY: f64 = 0.035;

struct Call {
    answerer: Session,
    caller: Session,
    rng: Rng,
    loss: f64,
    echo: f64,
    noise: f64,
    a_out: f64,
    c_out: f64,
    /// Everything each end has received, and every event, in order.
    at_answerer: Vec<u8>,
    at_caller: Vec<u8>,
    answerer_events: Vec<Event>,
    caller_events: Vec<Event>,
    seconds: f64,
}

impl Call {
    fn new(answerer: Config, caller: Config) -> Self {
        Self {
            answerer: Session::new(FS, Side::Answerer, answerer),
            caller: Session::new(FS, Side::Caller, caller),
            rng: Rng(0xCA11 | 1),
            loss: 0.3,
            echo: 0.5,
            noise: 0.006,
            a_out: 0.0,
            c_out: 0.0,
            at_answerer: Vec::new(),
            at_caller: Vec::new(),
            answerer_events: Vec::new(),
            caller_events: Vec::new(),
            seconds: 0.0,
        }
    }

    fn run(&mut self, seconds: f64) {
        for _ in 0..(seconds * FS) as usize {
            let to_caller = self.loss * self.a_out + self.echo * self.c_out + self.noise * self.rng.gaussian();
            let to_answerer = self.loss * self.c_out + self.echo * self.a_out + self.noise * self.rng.gaussian();
            self.caller.feed(to_caller);
            self.answerer.feed(to_answerer);
            self.a_out = self.answerer.next_sample();
            self.c_out = self.caller.next_sample();
        }
        self.seconds += seconds;
        self.at_answerer.extend(self.answerer.read());
        self.at_caller.extend(self.caller.read());
        self.answerer_events.extend(std::iter::from_fn(|| self.answerer.poll_event()));
        self.caller_events.extend(std::iter::from_fn(|| self.caller.poll_event()));
    }

    /// Run until `done`, a second at a time, for at most `limit` seconds.
    fn run_until(&mut self, limit: f64, done: &dyn Fn(&Call) -> bool) -> bool {
        let start = self.seconds;
        while self.seconds - start < limit {
            self.run(0.5);
            if done(self) {
                return true;
            }
        }
        false
    }
}

fn connected(events: &[Event]) -> Option<Rate> {
    events.iter().find_map(|e| match e {
        Event::Connected { rate, .. } => Some(*rate),
        _ => None,
    })
}

#[test]
fn the_ends_connect_and_agree_the_fastest_common_rate() {
    let mut call = Call::new(
        Config::default(),
        Config { rates: &[Rate::R7200, Rate::R9600], ..Config::default() },
    );
    let up = call.run_until(10.0, &|c| c.answerer.is_connected() && c.caller.is_connected());
    println!("connected after {:.1} s: {:?} / {:?}", call.seconds, call.answerer_events, call.caller_events);
    assert!(up, "no connection");
    assert_eq!(connected(&call.answerer_events), Some(Rate::R9600));
    assert_eq!(connected(&call.caller_events), Some(Rate::R9600));
    assert_eq!(call.answerer.holding(), Holding::Forward);
    assert_eq!(call.caller.holding(), Holding::Back);
}

#[test]
fn a_file_goes_fast_from_the_answerer_and_typing_comes_back_slow() {
    let mut call = Call::new(Config::default(), Config::default());
    assert!(call.run_until(10.0, &|c| c.answerer.is_connected() && c.caller.is_connected()));
    let file = Rng(77).bytes(12_000);
    let typing = b"ATDT 555-1212 and some typing that should stay on the back channel".to_vec();
    call.answerer.send(&file);
    call.caller.send(&typing);
    let t0 = call.seconds;
    let done = call.run_until(30.0, &|c| c.at_caller.len() >= 12_000 && c.at_answerer.len() >= typing.len());
    let took = call.seconds - t0;
    println!(
        "file {} of {} octets and typing {} of {} in {:.1} s; rate {:?}",
        call.at_caller.len(),
        file.len(),
        call.at_answerer.len(),
        typing.len(),
        took,
        call.answerer.rate()
    );
    assert!(done, "transfer did not finish");
    assert!(call.at_caller == file, "file corrupted");
    assert_eq!(call.at_answerer, typing);
    // Typing never asked for the fast channel.
    assert_eq!(call.caller.holding(), Holding::Back);
}

#[test]
fn the_caller_takes_the_fast_channel_for_an_upload_and_gives_it_back() {
    let mut call = Call::new(Config::default(), Config::default());
    assert!(call.run_until(10.0, &|c| c.answerer.is_connected() && c.caller.is_connected()));
    // Let the minimum hold since the start run out.
    call.run(3.0);
    let upload = Rng(91).bytes(12_000);
    call.caller.send(&upload);
    let got_it = call.run_until(40.0, &|c| c.at_answerer.len() >= 12_000);
    println!(
        "upload {} of {} octets by {:.1} s; caller events {:?}; answerer events {:?}",
        call.at_answerer.len(),
        upload.len(),
        call.seconds,
        call.caller_events,
        call.answerer_events
    );
    assert!(got_it, "upload did not finish");
    assert!(call.at_answerer == upload, "upload corrupted");
    assert!(call.caller_events.contains(&Event::Holding(Holding::Forward)), "caller never took the forward channel");
    assert!(call.answerer_events.iter().filter(|e| **e == Event::Holding(Holding::Back)).count() >= 1);

    // And back the other way: a download makes the answerer ask for it.
    let download = Rng(5).bytes(8000);
    call.answerer.send(&download);
    let back = call.run_until(40.0, &|c| c.at_caller.len() >= 8000);
    println!("download {} of {} by {:.1} s; holding: answerer {:?}, caller {:?}", call.at_caller.len(), download.len(), call.seconds, call.answerer.holding(), call.caller.holding());
    assert!(back, "download did not finish");
    assert!(call.at_caller == download, "download corrupted");
    assert_eq!(call.answerer.holding(), Holding::Forward);
    assert_eq!(call.caller.holding(), Holding::Back);
}

#[test]
fn the_300_bit_back_channel_is_agreed_when_either_end_wants_it() {
    let mut call = Call::new(Config::default(), Config { back: BackRate::B300, ..Config::default() });
    let up = call.run_until(10.0, &|c| c.answerer.is_connected() && c.caller.is_connected());
    println!("300: up {up}; answerer {} {:?}; caller {} {:?}", call.answerer.is_connected(), call.answerer_events, call.caller.is_connected(), call.caller_events);
    assert!(up);
    let back_of = |events: &[Event]| {
        events.iter().find_map(|e| match e {
            Event::Connected { back, .. } => Some(*back),
            _ => None,
        })
    };
    assert_eq!(back_of(&call.answerer_events), Some(BackRate::B300));
    assert_eq!(back_of(&call.caller_events), Some(BackRate::B300));
    call.caller.send(b"short");
    assert!(call.run_until(10.0, &|c| c.at_answerer.len() >= 5));
    assert_eq!(call.at_answerer, b"short");
}

#[test]
fn a_noisy_line_falls_back_and_error_control_puts_lost_frames_back() {
    // Noise the agreed 14 400 cannot carry: the receiving end has to ask for
    // a slower rate, and the frames lost before it does come again.
    let mut call = Call::new(Config::default(), Config::default());
    call.noise = NOISY;
    assert!(call.run_until(10.0, &|c| c.answerer.is_connected() && c.caller.is_connected()));
    let file = Rng(3).bytes(8000);
    call.answerer.send(&file);
    let t0 = call.seconds;
    let done = call.run_until(90.0, &|c| c.at_caller.len() >= 8000);
    println!(
        "noisy: {} of {} octets in {:.1} s, now at {:?}, {} frames sent again; caller events {:?}",
        call.at_caller.len(),
        file.len(),
        call.seconds - t0,
        call.answerer.rate(),
        call.answerer.retransmitted(),
        call.caller_events
    );
    assert!(done, "transfer did not finish");
    assert!(call.at_caller == file, "file corrupted");
    assert!(call.answerer.rate() != Rate::R14400, "never fell back");
    assert_eq!(call.answerer.rate(), call.caller.rate());
}

#[test]
fn without_fallback_the_same_line_barely_carries_anything() {
    let off = Config { fallback: false, ..Config::default() };
    let mut call = Call::new(off, off);
    call.noise = NOISY;
    assert!(call.run_until(10.0, &|c| c.answerer.is_connected() && c.caller.is_connected()));
    call.answerer.send(&Rng(3).bytes(2000));
    call.run(15.0);
    println!("no fallback: {} octets in 15 s", call.at_caller.len());
    assert!(call.at_caller.len() < 2000);
}

#[test]
fn when_the_line_clears_the_rate_climbs_back() {
    let mut call = Call::new(Config::default(), Config::default());
    call.noise = NOISY;
    assert!(call.run_until(10.0, &|c| c.answerer.is_connected() && c.caller.is_connected()));
    assert!(call.run_until(15.0, &|c| c.caller.rate() != Rate::R14400), "never fell back");
    let low = call.caller.rate();
    call.noise = 0.004;
    let climbed = call.run_until(30.0, &|c| c.caller.rate() == Rate::R14400 && c.answerer.rate() == Rate::R14400);
    println!("fell to {low:?}, climbed back: {climbed}; caller events {:?}", call.caller_events);
    assert!(climbed);
    // And it still carries data.
    let file = Rng(8).bytes(4000);
    call.answerer.send(&file);
    assert!(call.run_until(20.0, &|c| c.at_caller.len() >= 4000));
    assert!(call.at_caller == file);
}
