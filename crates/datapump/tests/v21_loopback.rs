//! Two V.21 modems calling each other, as data modems rather than as the
//! carrier for T.30 and V.8.
//!
//! The same modem as Bell 103 with V.21's tones in it, so what is worth
//! proving is what differs: the bands, which way up mark and space are, and
//! the answering end's V.25 procedure in front of its carrier.

use datapump::bell103::{self, Modem, Role, Status};
use datapump::v21;

const FS: f64 = 16_000.0;

/// The same two-wire line as `bell103_loopback.rs`: each end hears the far
/// one quietly and its own transmitter loudly.
const ECHO: f64 = 0.251;
const FAR: f64 = 0.1;

struct Pair {
    caller: Modem,
    host: Modem,
    from_caller: f64,
    from_host: f64,
    at_caller: Vec<u8>,
    at_host: Vec<u8>,
    connected_at: f64,
    samples: usize,
}

impl Pair {
    fn new() -> Self {
        Self::between(v21::DATA, v21::DATA)
    }

    fn between(caller: bell103::Plan, host: bell103::Plan) -> Self {
        Self {
            caller: Modem::with_plan(caller, Role::Originate, FS),
            host: Modem::with_plan(host, Role::Answer, FS),
            from_caller: 0.0,
            from_host: 0.0,
            at_caller: Vec::new(),
            at_host: Vec::new(),
            connected_at: f64::NAN,
            samples: 0,
        }
    }

    fn run(&mut self, seconds: f64) {
        for _ in 0..(seconds * FS) as usize {
            let (a, b) = (self.from_caller, self.from_host);
            self.from_caller = self.caller.step(b * FAR + a * ECHO);
            self.from_host = self.host.step(a * FAR + b * ECHO);
            self.at_caller.extend(self.caller.take_bytes());
            self.at_host.extend(self.host.take_bytes());
            self.samples += 1;
            if self.connected_at.is_nan() && self.up() {
                self.connected_at = self.elapsed();
            }
        }
    }

    fn elapsed(&self) -> f64 {
        self.samples as f64 / FS
    }

    fn up(&self) -> bool {
        matches!(self.caller.status(), Status::Connected(_))
            && matches!(self.host.status(), Status::Connected(_))
    }
}

#[test]
fn the_channels_are_the_ones_clause_3_gives() {
    // "For channel No. 1, the nominal mean frequency is 1080 Hz. For channel
    // No. 2, it is 1750 Hz. The frequency deviation is +/- 100 Hz. In each
    // channel, the higher characteristic frequency (F_A) corresponds to a
    // binary 0."
    for ((space, mark), centre) in [(v21::CHANNEL_1, 1080.0), (v21::CHANNEL_2, 1750.0)] {
        assert_eq!((space + mark) / 2.0, centre);
        assert_eq!(space - mark, 200.0, "space is F_A, the higher tone");
    }
    // Clause 7 a): channel 1 carries the caller's data.
    assert_eq!(v21::DATA.transmit_tones(Role::Originate), v21::CHANNEL_1);
    assert_eq!(v21::DATA.transmit_tones(Role::Answer), v21::CHANNEL_2);
}

#[test]
fn a_call_comes_up() {
    let mut p = Pair::new();
    p.run(9.0);
    assert!(
        p.up(),
        "the caller stopped at {} and the host at {}",
        p.caller.line_phase(),
        p.host.line_phase()
    );
    assert_eq!(p.caller.status(), Status::Connected(300));
    assert_eq!(p.caller.standard(), "V.21");
    println!("connected after {:.2} s", p.connected_at);
    // V.25: 1.8 to 2.5 s of silence, 3.3 s of tone and 75 ms of silence
    // before the answering end's carrier is even up. Then a moment of mark.
    assert!(
        (5.5..7.0).contains(&p.connected_at),
        "connected after {:.2} s",
        p.connected_at
    );
}

#[test]
fn typing_at_one_end_comes_out_at_the_other() {
    let mut p = Pair::new();
    p.run(9.0);
    assert!(p.up(), "never connected");

    let banner = b"\r\nWelcome to the Prestel gateway\r\nUser: ";
    let typed = b"guest\r";
    p.host.send(banner);
    p.caller.send(typed);
    p.run(3.0);

    assert_eq!(
        String::from_utf8_lossy(&p.at_caller),
        String::from_utf8_lossy(banner),
        "the calling end did not get the banner"
    );
    assert_eq!(
        String::from_utf8_lossy(&p.at_host),
        String::from_utf8_lossy(typed),
        "the answering end did not get what was typed"
    );
}

#[test]
fn the_answering_end_follows_v25() {
    // What goes down the line from an answering modem with nobody there:
    // silence, then 2100 Hz, then silence, then channel 2's mark.
    let mut host = Modem::with_plan(v21::DATA, Role::Answer, FS);
    let line: Vec<f64> = (0..(7.0 * FS) as usize).map(|_| host.step(0.0)).collect();

    // Where the signal is, and how loud, in 10 ms windows.
    let window = (0.010 * FS) as usize;
    let loud: Vec<bool> = line
        .chunks(window)
        .map(|w| w.iter().map(|x| x * x).sum::<f64>() / w.len() as f64 > 0.1)
        .collect();
    let first_sound = loud.iter().position(|&l| l).expect("never made a sound") as f64 * 0.010;
    // 4.3: "maintains silence on the line for a period between 1.8 and 2.5 s".
    assert!((1.8..2.5).contains(&first_sound), "the tone began at {first_sound:.2} s");

    let tone_end = first_sound
        + loud[(first_sound / 0.010) as usize..]
            .iter()
            .position(|&l| !l)
            .expect("the tone never ended") as f64
            * 0.010;
    // 2.2 note 1: 3.3 +/- 0.7 s.
    let tone = tone_end - first_sound;
    assert!((2.6..4.0).contains(&tone), "the tone lasted {tone:.2} s");

    // And it was 2100 Hz: count zero crossings over the middle second of it.
    let middle = &line[((first_sound + 1.0) * FS) as usize..((first_sound + 2.0) * FS) as usize];
    let crossings = middle.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
    let hz = crossings as f64 / 2.0;
    assert!((hz - 2100.0).abs() <= 15.0, "the answering tone was {hz} Hz");

    // 4.4: "a silent period for 75 +/- 20 ms", then the carrier.
    let carrier = tone_end
        + loud[(tone_end / 0.010) as usize..]
            .iter()
            .position(|&l| l)
            .expect("no carrier after the tone") as f64
            * 0.010;
    let gap = carrier - tone_end;
    assert!((0.055..=0.105).contains(&gap), "{:.0} ms of silence after the tone", gap * 1000.0);

    // Channel 2's mark is F_Z, 1650 Hz.
    let start = ((carrier + 0.2) * FS) as usize;
    let after = &line[start..start + FS as usize / 2];
    let crossings = after.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
    let hz = crossings as f64;
    assert!((hz - 1650.0).abs() <= 6.0, "the idle carrier was {hz} Hz");
}

#[test]
fn an_answer_tone_is_waited_out_rather_than_talked_over() {
    // 2100 Hz is above channel 2's space at 1850, so whatever the receiver
    // makes of it, it is not the idle mark that says the far end is ready.
    let mut caller = Modem::with_plan(v21::DATA, Role::Originate, FS);
    let mut tone = v21::Tone::new(v21::ANSWER_TONE, FS);
    for _ in 0..(3.3 * FS) as usize {
        caller.step(FAR * tone.next_sample());
    }
    assert_eq!(
        caller.status(),
        Status::Negotiating,
        "connected to an answer tone, at {}",
        caller.line_phase()
    );
}

#[test]
fn a_call_to_nobody_gives_up_rather_than_waiting_for_ever() {
    let mut caller = Modem::with_plan(v21::DATA, Role::Originate, FS);
    let mut status = Status::Negotiating;
    for _ in 0..(62.0 * FS) as usize {
        caller.step(0.0);
        status = caller.status();
        if status == Status::Failed {
            break;
        }
    }
    assert_eq!(status, Status::Failed);
}

#[test]
fn a_bell_103_and_a_v21_do_not_mistake_each_other() {
    // Neither one's carrier falls in the band the other listens to, so the
    // call fails rather than connecting to nonsense.
    for (caller, host) in [(v21::DATA, bell103::BELL_103), (bell103::BELL_103, v21::DATA)] {
        let mut p = Pair::between(caller, host);
        p.run(10.0);
        assert!(
            !p.up(),
            "a {} caller connected to a {} host",
            caller.name,
            host.name
        );
    }
}
