//! A V.90 call between the two modems here, over a simulated network.

use datapump::v34::info::{Info0, Info0d, Info1aPcm, Info1c, Probed, SymbolRate};
use datapump::v90::network::Network;
use datapump::v90::ucode::Law;
use datapump::v90::{analogue, digital};

const FS: f64 = 16_000.0;

fn server() -> Info0d {
    Info0d {
        v34: Info0 { constellation_1664: true, rate_3429: true, ..Info0::default() },
        nominal_power: 4,
        max_power: 23,
        power_at_codec: true,
        a_law: false,
        upstream_3429: false,
    }
}

/// Phase 2 as it would have come out.
fn settled() -> (analogue::Settings, digital::Settings) {
    let probed = [Probed { high_carrier: false, pre_emphasis: 0, max_rate: 12 }; 6];
    let info1d = Info1c { probed, ..Info1c::default() };
    let asked = Info1aPcm { md_length: 0, uinfo: 79, upstream: SymbolRate::S3200, frequency_offset: None };
    (
        analogue::Settings::new(&server(), &info1d, &asked, 0.02, true),
        digital::Settings::new(Law::Mu, &info1d, &asked, 0.02, true),
    )
}

struct Call {
    net: Network,
    analogue: analogue::Modem,
    digital: digital::Modem,
    up: Vec<f64>,
    ticks: u64,
}

impl Call {
    fn new(net: Network) -> Self {
        let (a, d) = settled();
        Self { net, analogue: analogue::Modem::new(a, FS), digital: digital::Modem::new(d), up: Vec::new(), ticks: 0 }
    }

    /// One network sample: 125 microseconds.
    fn tick(&mut self) {
        let to_digital = self.net.up(&self.up);
        self.up.clear();
        let from_digital = self.digital.step(to_digital);
        for x in self.net.down(from_digital) {
            self.up.push(self.analogue.step(x));
        }
        self.ticks += 1;
    }

    fn run_until(&mut self, seconds: f64, mut done: impl FnMut(&Self) -> bool) -> bool {
        let end = self.ticks + (seconds * 8000.0) as u64;
        let mut last = ("", "");
        while self.ticks < end {
            self.tick();
            let now = (self.analogue.phase(), self.digital.phase());
            if now != last {
                println!("{:7.3} s  analogue: {:28} digital: {}", self.ticks as f64 / 8000.0, now.0, now.1);
                last = now;
            }
            if done(self) {
                return true;
            }
        }
        false
    }

    fn connected(&self) -> bool {
        matches!(self.analogue.status(), analogue::Status::Connected { .. })
            && matches!(self.digital.status(), digital::Status::Connected { .. })
    }
}

fn check_connects(net: Network) -> Call {
    let mut call = Call::new(net);
    let ok = call.run_until(25.0, |c| {
        c.connected()
            || matches!(c.analogue.status(), analogue::Status::Failed(_))
            || matches!(c.digital.status(), digital::Status::Failed(_))
    });
    println!(
        "analogue {:?}, digital {:?}, trained {:.1} dB, choice {:?}",
        call.analogue.status(),
        call.digital.status(),
        call.analogue.receiver().trained_snr_db(),
        call.analogue.choice().map(|c| (c.data.drn, c.training.drn))
    );
    assert!(ok && call.connected(), "no connection");
    call
}

#[test]
fn phases_3_and_4_connect_over_a_clean_network() {
    let call = check_connects(Network::new(Law::Mu, FS).with_delay(0.010, FS).with_noise(1e-5));
    let analogue::Status::Connected { downstream, upstream } = call.analogue.status() else { unreachable!() };
    assert!(downstream >= 48_000, "downstream {downstream}");
    assert!(upstream >= 24_000, "upstream {upstream}");
    assert_eq!(call.digital.status(), digital::Status::Connected { downstream, upstream });
}

fn pattern(n: usize, seed: u64) -> Vec<bool> {
    let mut x = seed | 1;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x & 1 == 1
        })
        .collect()
}

/// Whether `sent` turns up whole in `got`.
fn contains(got: &[bool], sent: &[bool]) -> bool {
    got.windows(sent.len()).any(|w| w == sent)
}

#[test]
fn data_crosses_both_ways() {
    let mut call = check_connects(Network::new(Law::Mu, FS).with_delay(0.010, FS).with_noise(1e-5));
    println!("digital phase 3 SNR {:?}", call.digital.phase3_snr());
    let down = pattern(20_000, 7);
    let up = pattern(8_000, 11);
    call.digital.send_bits(&down);
    call.analogue.send_bits(&up);
    let (mut got_down, mut got_up) = (Vec::new(), Vec::new());
    call.run_until(3.0, |_| false);
    got_down.extend(call.analogue.take_bits());
    got_up.extend(call.digital.take_bits());
    println!("received {} down, {} up", got_down.len(), got_up.len());
    assert!(contains(&got_down, &down), "the downstream did not arrive whole");
    assert!(contains(&got_up, &up), "the upstream did not arrive whole");
}

/// The whole start-up, from the end of V.8.
struct FullCall {
    net: Network,
    analogue: datapump::v90::startup::Analogue,
    digital: datapump::v90::startup::Digital,
    up: Vec<f64>,
    ticks: u64,
}

impl FullCall {
    fn new(net: Network, server: Info0d) -> Self {
        Self {
            net,
            analogue: datapump::v90::startup::Analogue::new(FS),
            digital: datapump::v90::startup::Digital::new(server),
            up: Vec::new(),
            ticks: 0,
        }
    }

    fn run(&mut self, seconds: f64) -> bool {
        use datapump::v90::startup::Status;
        let end = self.ticks + (seconds * 8000.0) as u64;
        let mut last = ("", "");
        while self.ticks < end {
            let to_digital = self.net.up(&self.up);
            self.up.clear();
            let from_digital = self.digital.step(to_digital);
            for x in self.net.down(from_digital) {
                self.up.push(self.analogue.step(x));
            }
            self.ticks += 1;
            let now = (self.analogue.phase(), self.digital.phase());
            if now != last {
                println!("{:7.3} s  analogue: {:28} digital: {}", self.ticks as f64 / 8000.0, now.0, now.1);
                last = now;
            }
            let up = |s: Status| matches!(s, Status::Connected { .. });
            if up(self.analogue.status()) && up(self.digital.status()) {
                return true;
            }
            if matches!(self.analogue.status(), Status::Failed(_)) || matches!(self.digital.status(), Status::Failed(_)) {
                return false;
            }
        }
        false
    }
}

#[test]
fn a_whole_v90_start_up_from_phase_2_connects() {
    use datapump::v90::startup::Status;
    let mut call = FullCall::new(Network::new(Law::Mu, FS).with_delay(0.020, FS).with_noise(1e-5), server());
    let ok = call.run(30.0);
    println!("{:?} {:?}", call.analogue.status(), call.digital.status());
    assert!(ok, "no connection");
    assert!(call.analogue.is_v90());
    let Status::Connected { transmit, receive } = call.analogue.status() else { unreachable!() };
    assert!(receive >= 48_000 && transmit >= 24_000, "{receive} down, {transmit} up");
    // And the round trip phase 2 measured is the line's.
    let rtt = call.analogue.round_trip().unwrap();
    assert!((0.03..0.08).contains(&rtt), "round trip {rtt}");
}

fn connects(net: Network, server: Info0d, seconds: f64) -> FullCall {
    let mut call = FullCall::new(net, server);
    let ok = call.run(seconds);
    println!("{:?} {:?} ({})", call.analogue.status(), call.digital.status(), call.analogue.last_failure().unwrap_or(""));
    assert!(ok, "no connection: {} / {}", call.analogue.phase(), call.digital.phase());
    call
}

impl FullCall {
    /// Run until the network has carried `seconds` in all.
    fn run_until_seconds(&mut self, seconds: f64) {
        while (self.ticks as f64) < seconds * 8000.0 {
            let to_digital = self.net.up(&self.up);
            self.up.clear();
            let from_digital = self.digital.step(to_digital);
            for x in self.net.down(from_digital) {
                self.up.push(self.analogue.step(x));
            }
            self.ticks += 1;
        }
    }

    /// Send both ways for a while, and say whether every bit arrived.
    fn carries_data(&mut self, seconds: f64) -> (bool, bool) {
        let down = pattern(30_000, 3);
        let up = pattern(15_000, 5);
        self.analogue.take_bits();
        self.digital.take_bits();
        self.digital.send_bits(&down);
        self.analogue.send_bits(&up);
        let (mut got_down, mut got_up) = (Vec::new(), Vec::new());
        let end = self.ticks + (seconds * 8000.0) as u64;
        while self.ticks < end {
            let to_digital = self.net.up(&self.up);
            self.up.clear();
            let from_digital = self.digital.step(to_digital);
            for x in self.net.down(from_digital) {
                self.up.push(self.analogue.step(x));
            }
            self.ticks += 1;
            got_down.extend(self.analogue.take_bits());
            got_up.extend(self.digital.take_bits());
        }
        (contains(&got_down, &down), contains(&got_up, &up))
    }
}

#[test]
fn a_robbed_bit_route_connects_and_carries_data() {
    let mut call = connects(Network::new(Law::Mu, FS).with_delay(0.020, FS).with_noise(1e-5).with_robbed_bit(2), server(), 30.0);
    let v90 = call.analogue.v90().expect("not V.90");
    // What the DIL made of the robbed interval: half its codewords arrive
    // as a neighbour.
    let route = v90.route().unwrap();
    let moved: Vec<usize> = (0..6)
        .map(|i| {
            (1..127u8)
                .filter(|&u| route.readings[i][usize::from(u)] > 0)
                .filter(|&u| {
                    let level = |u: u8| datapump::v90::ucode::level(Law::Mu, u);
                    let step = level(u + 1) - level(u);
                    (route.levels[i][usize::from(u)] - level(u)).abs() > 0.4 * step
                })
                .count()
        })
        .collect();
    println!("codewords moved in each interval {moved:?}");
    assert_eq!(moved.iter().filter(|&&n| n > 30).count(), 1, "{moved:?}");
    assert_eq!(call.carries_data(4.0), (true, true));
}

#[test]
fn an_a_law_network_connects() {
    let mut a_law = server();
    a_law.a_law = true;
    connects(Network::new(Law::A, FS).with_delay(0.020, FS).with_noise(1e-5), a_law, 30.0);
}

#[test]
fn a_voip_length_round_trip_connects() {
    // 0.6 s each way: the round trip Rory's SIP trunk measures.
    let call = connects(Network::new(Law::Mu, FS).with_delay(0.600, FS).with_noise(1e-5), server(), 40.0);
    let rtt = call.analogue.round_trip().unwrap();
    assert!((1.15..1.3).contains(&rtt), "round trip {rtt}");
}

#[test]
fn a_noisy_loop_connects_slower() {
    let call = connects(Network::new(Law::Mu, FS).with_delay(0.020, FS).with_noise(3e-3), server(), 30.0);
    let datapump::v90::startup::Status::Connected { receive, .. } = call.analogue.status() else { unreachable!() };
    assert!(receive < 50_000, "{receive} on a noisy loop");
}

/// A softphone's jitter buffer slipping twenty milliseconds of the
/// downstream, once each way, in the middle of data mode: what is lost with
/// it is lost, and what is sent after it arrives.
#[test]
fn a_slip_in_data_mode_is_followed_and_data_after_it_arrives() {
    for inserted in [true, false] {
        let net = Network::new(Law::Mu, FS).with_delay(0.020, FS).with_noise(1e-5).with_slips(9.0, inserted);
        let mut call = connects(net, server(), 30.0);
        // Past the first slip, which is at nine seconds.
        call.run_until_seconds(10.0);
        assert_eq!(call.net.slips(), 1, "no slip happened");
        let v90 = call.analogue.v90().unwrap();
        println!("inserted {inserted}: frames moved {}, receiver lost {}", v90.frames_moved(), v90.receiver().slips());
        assert!(v90.frames_moved() >= 1, "the frames were never found again");
        assert_eq!(call.carries_data(4.0), (true, true), "inserted {inserted}");
    }
}

/// A retrain from data mode, from either end (9.5): both go back through
/// V.90's phase 2, train again, and carry data again.
#[test]
fn a_retrain_from_either_end_comes_back_up() {
    use datapump::v90::startup::Status;
    for from_server in [true, false] {
        let mut call = connects(Network::new(Law::Mu, FS).with_delay(0.020, FS).with_noise(1e-5), server(), 30.0);
        let up = |s: Status| matches!(s, Status::Connected { .. });
        assert!(if from_server { call.digital.retrain() } else { call.analogue.retrain() });
        // Down, then up again.
        let start = call.ticks;
        let mut went_down = false;
        while call.ticks < start + 30 * 8000 {
            call.run_until_seconds((call.ticks + 800) as f64 / 8000.0);
            if !up(call.analogue.status()) {
                went_down = true;
            }
            if went_down && up(call.analogue.status()) && up(call.digital.status()) {
                break;
            }
        }
        println!("from the server {from_server}: {:?} {:?}", call.analogue.status(), call.digital.status());
        assert!(went_down, "the call never left data mode");
        assert!(up(call.analogue.status()) && up(call.digital.status()), "the retrain never came back up");
        assert!(call.analogue.is_v90());
        assert_eq!(call.carries_data(3.0), (true, true), "from the server {from_server}");
    }
}

impl FullCall {
    /// Run until both ends are in data mode again, having left it; false if
    /// that takes more than `seconds`.
    fn comes_back_up(&mut self, seconds: f64) -> bool {
        use datapump::v90::startup::Status;
        let up = |s: Status| matches!(s, Status::Connected { .. });
        let end = self.ticks + (seconds * 8000.0) as u64;
        let mut went_down = false;
        while self.ticks < end {
            self.run_until_seconds((self.ticks + 80) as f64 / 8000.0);
            if !up(self.analogue.status()) || !up(self.digital.status()) {
                went_down = true;
            } else if went_down {
                return true;
            }
        }
        false
    }

    /// The downstream rate, or nothing while the call is between rates.
    fn rate_now(&self) -> Option<u32> {
        use datapump::v90::startup::Status;
        match self.analogue.status() {
            Status::Connected { receive, .. } => Some(receive),
            _ => None,
        }
    }

    fn rates(&self) -> (u32, u32) {
        use datapump::v90::startup::Status;
        match self.analogue.status() {
            Status::Connected { transmit, receive } => (receive, transmit),
            other => panic!("not connected: {other:?}"),
        }
    }
}

/// A rate renegotiation from data mode (9.6), from either end: back through
/// phase 4 at the rates asked for, with no retrain, and data after it.
#[test]
fn a_rate_renegotiation_from_either_end_settles_the_rates_asked_for() {
    // Over a short line, and over a VoIP call's 600 ms each way.
    for (from_server, delay) in [(true, 0.020), (false, 0.020), (true, 0.6), (false, 0.6)] {
        let mut call = connects(Network::new(Law::Mu, FS).with_delay(delay, FS).with_noise(1e-5), server(), 40.0);
        let (down, up) = call.rates();
        let began = call.ticks;
        if from_server {
            assert!(call.digital.renegotiate(8));
        } else {
            assert!(call.analogue.renegotiate(40_000));
        }
        assert!(call.comes_back_up(10.0), "from the server {from_server}: {} / {}", call.analogue.phase(), call.digital.phase());
        let (new_down, new_up) = call.rates();
        println!("from the server {from_server}, {delay} s each way: {down}/{up} became {new_down}/{new_up} in {:.2} s", (call.ticks - began) as f64 / 8000.0);
        if from_server {
            assert_eq!(new_up, 19_200);
            assert_eq!(new_down, down);
        } else {
            assert!((36_000..=40_000).contains(&new_down), "downstream {new_down}");
            assert_eq!(new_up, up);
        }
        assert!(call.analogue.is_v90());
        assert_eq!(call.analogue.retrains(), 0, "a retrain happened");
        assert_eq!(call.analogue.renegotiations(), 1);
        assert_eq!(call.digital.v90().map(|m| m.renegotiations()), Some(1));
        assert_eq!(call.carries_data(3.0), (true, true), "from the server {from_server}");
        // And again, the other way round.
        if from_server {
            assert!(call.analogue.renegotiate(60_000));
        } else {
            assert!(call.digital.renegotiate(14));
        }
        assert!(call.comes_back_up(10.0), "second, from the server {}", !from_server);
        assert_eq!(call.carries_data(3.0), (true, true), "second, from the server {}", !from_server);
    }
}

/// A line that gets noisier in data mode: the analogue modem sees its
/// levels are too close for the errors it is making, and renegotiates to a
/// slower rate that carries data cleanly (9.6.2.1), with no retrain.
#[test]
fn a_line_gone_noisy_is_renegotiated_down() {
    let mut call = connects(Network::new(Law::Mu, FS).with_delay(0.020, FS).with_noise(1e-5), server(), 30.0);
    let (down, _) = call.rates();
    call.net.set_noise(1e-3);
    assert!(call.comes_back_up(10.0), "{} / {}", call.analogue.phase(), call.digital.phase());
    let (slower, _) = call.rates();
    println!("{down} became {slower} after {} renegotiations", call.analogue.renegotiations());
    assert!(slower < down);
    // Settled: the rate holds, and data crosses.
    call.run_until_seconds(call.ticks as f64 / 8000.0 + 4.0);
    assert_eq!(call.rates().0, slower, "{} renegotiations", call.analogue.renegotiations());
    assert_eq!(call.carries_data(3.0), (true, true));
    assert_eq!(call.analogue.retrains(), 0);
}

/// 9.7: a cleardown from either end ends the call at both.
#[test]
fn a_cleardown_from_either_end_ends_the_call_at_both() {
    use datapump::v90::startup::Status;
    for from_server in [true, false] {
        let mut call = connects(Network::new(Law::Mu, FS).with_delay(0.020, FS), server(), 30.0);
        assert!(if from_server { call.digital.clear_down() } else { call.analogue.clear_down() });
        let start = call.ticks;
        while call.ticks < start + 3 * 8000 {
            call.run_until_seconds((call.ticks + 80) as f64 / 8000.0);
            if call.analogue.status() == Status::ClearedDown && call.digital.status() == Status::ClearedDown {
                break;
            }
        }
        println!("from the server {from_server}: {:?} {:?} after {:.2} s", call.analogue.status(), call.digital.status(), (call.ticks - start) as f64 / 8000.0);
        assert_eq!(call.analogue.status(), Status::ClearedDown, "from the server {from_server}");
        assert_eq!(call.digital.status(), Status::ClearedDown, "from the server {from_server}");
    }
}

#[test]
fn a_sound_card_clock_120_ppm_off_is_followed_through_ten_seconds_of_data() {
    let mut call = connects(Network::new(Law::Mu, FS).with_delay(0.020, FS).with_noise(1e-5).with_clock(120.0), server(), 30.0);
    let drift = call.analogue.v90().unwrap().receiver().drift_ppm();
    println!("drift read as {drift:.1} ppm");
    assert_eq!(call.carries_data(10.0), (true, true));
    let drift = call.analogue.v90().unwrap().receiver().drift_ppm();
    assert!((drift.abs() - 120.0).abs() < 20.0, "drift read as {drift:.1} ppm");
}
/// A softphone's jitter buffer slipping twenty milliseconds of the
/// downstream every few seconds, over a VoIP call's round trip, landing in
/// phase 3's training, the DIL and phase 4: each is followed, and the start-up
/// connects the first time.
#[test]
fn slips_during_the_start_up_are_followed() {
    let (mut dil_moved, mut frames_moved) = (0, 0);
    for (period, inserted) in [(5.9, false), (2.9, true), (4.3, true), (3.1, false), (2.3, true)] {
        let net = Network::new(Law::Mu, FS).with_delay(0.6, FS).with_noise(1e-5).with_slips(period, inserted);
        let mut call = FullCall::new(net, server());
        let ok = call.run(25.0);
        let v90 = call.analogue.v90();
        println!(
            "slips every {period} s, inserted {inserted}: {} slips, DIL moved {:?}, frames moved {:?}",
            call.net.slips(),
            v90.map(|m| m.dil_moved()),
            v90.map(|m| m.frames_moved())
        );
        assert!(ok, "slips every {period} s: {} / {} ({:?})", call.analogue.phase(), call.digital.phase(), call.analogue.last_failure());
        assert_eq!(call.analogue.retrains(), 0, "slips every {period} s: {:?}", call.analogue.last_failure());
        assert!(call.net.slips() >= 2);
        dil_moved += v90.map_or(0, |m| m.dil_moved());
        frames_moved += v90.map_or(0, |m| m.frames_moved());
    }
    assert!(dil_moved > 0, "no slip landed in a DIL");
    assert!(frames_moved > 0, "no slip moved the frames in phase 4");
}

/// A line too noisy for PCM: the DIL says so, and the analogue modem asks for
/// V.34 in the retrain's INFO1a (9.2.2.1.9), and gets it.
#[test]
fn a_line_that_will_not_carry_pcm_comes_up_as_v34() {
    let mut call = FullCall::new(Network::new(Law::Mu, FS).with_delay(0.020, FS).with_noise(2e-2), server());
    let ok = call.run(60.0);
    println!("{:?} {:?}, {} retrains, last failure {:?}", call.analogue.status(), call.digital.status(), call.analogue.retrains(), call.analogue.last_failure());
    assert!(ok, "no connection: {} / {}", call.analogue.phase(), call.digital.phase());
    assert!(!call.analogue.is_v90());
    assert_eq!(call.analogue.last_failure(), Some("the route cannot carry V.90's slowest rate"));
    assert_eq!(call.analogue.retrains(), 1);
    assert_eq!(call.carries_data(3.0), (true, true));
}

/// What a live call through a softphone did to the start-up: a gain control
/// that held anything much above a third of full scale down and took a third
/// of a second to recover, and a jitter buffer cutting ten milliseconds out
/// wherever the audio repeated itself. The DIL asks for nothing loud enough to
/// set the gain control off, and a cut in it is found again.
#[test]
fn a_softphone_with_a_gain_control_and_a_hasty_jitter_buffer_is_followed() {
    for (period, inserted) in [(0.7, false), (2.9, true), (3.1, false)] {
        let net = Network::new(Law::Mu, FS)
            .with_delay(0.6, FS)
            .with_noise(1e-5)
            .with_gain_control(0.8, 0.3)
            .with_slips(period, inserted);
        let mut call = FullCall::new(net, server());
        let ok = call.run(40.0);
        let v90 = call.analogue.v90();
        println!(
            "slips every {period} s, inserted {inserted}: {:?}, {} retrains, DIL moved {:?}",
            call.analogue.status(),
            call.analogue.retrains(),
            v90.map(|m| m.dil_moved())
        );
        assert!(ok, "slips every {period} s: {} / {}", call.analogue.phase(), call.digital.phase());
        assert!(call.analogue.is_v90(), "slips every {period} s: {:?}", call.analogue.last_failure());
    }
}

/// What a live server did in phase 3: four seconds of TRN1d, Jd at the last
/// moment 9.3.1.4 allows, and a wait for S that did not allow for the round
/// trip. Over a VoIP call a second there and back, S that waited to read Jd
/// arrived after the server had given up; S sent ahead of Jd, to arrive just
/// after the latest the server can have begun it, does not.
#[test]
fn a_server_that_sends_jd_at_the_last_moment_hears_s_in_time() {
    use datapump::v90::digital::Habits;
    for delay in [0.3, 0.6] {
        let net = Network::new(Law::Mu, FS).with_delay(delay, FS).with_noise(1e-5);
        let mut call = FullCall::new(net, server());
        call.digital = datapump::v90::startup::Digital::new(server()).with_habits(Habits::LIVE_SERVER);
        let ok = call.run(40.0);
        println!("{delay} s each way: {:?}, {} retrains, {:?}", call.analogue.status(), call.analogue.retrains(), call.analogue.last_failure());
        assert!(ok, "{delay} s each way: {} / {}", call.analogue.phase(), call.digital.phase());
        assert!(call.analogue.is_v90());
        assert_eq!(call.analogue.retrains(), 0, "{delay} s each way");
    }
}

/// A jitter buffer cutting ten milliseconds out of the first fifth of a
/// second of a four-second TRN1d, which the receiver trains on: the next
/// stretch is found where the cut moved it, and trained on instead.
#[test]
fn a_cut_in_the_training_stretch_is_trained_past() {
    use datapump::v90::digital::Habits;
    let net = || Network::new(Law::Mu, FS).with_delay(0.3, FS).with_noise(1e-5);
    // Where TRN1d begins, in the network's own time.
    let mut call = FullCall::new(net(), server());
    call.digital = datapump::v90::startup::Digital::new(server()).with_habits(Habits::LIVE_SERVER);
    while call.digital.phase() != "V.90 phase 3: Jd" {
        call.run_until_seconds((call.ticks + 8) as f64 / 8000.0);
    }
    // Sd and S-bar-d are 432 symbols, and the cut is a tenth of a second in.
    let cut = call.ticks as f64 / 8000.0 + 0.054 + 0.1;
    for inserted in [false, true] {
        let mut call = FullCall::new(net().with_slip_at(cut, inserted), server());
        call.digital = datapump::v90::startup::Digital::new(server()).with_habits(Habits::LIVE_SERVER);
        let ok = call.run(40.0);
        println!("inserted {inserted}: {:?}, {} retrains, {:?}", call.analogue.status(), call.analogue.retrains(), call.analogue.last_failure());
        assert_eq!(call.net.slips(), 1);
        assert!(ok, "inserted {inserted}: {} / {}", call.analogue.phase(), call.digital.phase());
        assert_eq!(call.analogue.retrains(), 0, "inserted {inserted}");
    }
}

impl FullCall {
    /// Run with one direction silenced, as a far end that has hung up leaves
    /// it, and say how long the other end took to notice: the analogue
    /// modem when the server stops, the digital modem when the client does.
    fn notices_silence(&mut self, server_stops: bool, seconds: f64) -> Option<f64> {
        let start = self.ticks;
        let end = self.ticks + (seconds * 8000.0) as u64;
        while self.ticks < end {
            let up: Vec<f64> = if server_stops { self.up.clone() } else { vec![0.0; self.up.len()] };
            let to_digital = self.net.up(&up);
            self.up.clear();
            let from_digital = self.digital.step(to_digital);
            for x in self.net.down(if server_stops { 0.0 } else { from_digital }) {
                self.up.push(self.analogue.step(x));
            }
            self.ticks += 1;
            let (gone, other) = if server_stops {
                (!self.analogue.carrier(), self.digital.carrier())
            } else {
                (!self.digital.carrier(), self.analogue.carrier())
            };
            // The end still being sent to has nothing to notice.
            assert!(other, "the end still hearing its far end lost it");
            if gone {
                let went = if server_stops {
                    self.analogue.v90().map(|m| m.far_end_went())
                } else {
                    self.digital.v90().map(|m| m.far_end_went())
                };
                assert_eq!(went, Some(true), "ended, but not for the far end's silence");
                return Some((self.ticks - start) as f64 / 8000.0);
            }
        }
        None
    }
}

/// A far end that hangs up in data mode stops sending, and says nothing
/// first. V.90 has no carrier detector of its own, and a receiver that reads
/// silence as the quietest codewords never counts itself lost, so the end left
/// behind stayed in data mode for as long as anyone let it.
#[test]
fn a_far_end_that_stops_sending_is_noticed_at_either_end() {
    for server_stops in [true, false] {
        let mut call = connects(Network::new(Law::Mu, FS).with_delay(0.6, FS).with_noise(1e-5), server(), 40.0);
        assert_eq!(call.carries_data(2.0), (true, true));
        assert!(call.analogue.carrier() && call.digital.carrier());
        let after = call.notices_silence(server_stops, 6.0);
        println!("server stops {server_stops}: noticed after {after:?} s");
        let after = after.unwrap_or_else(|| panic!("server stops {server_stops}: never noticed"));
        // Two seconds of quiet once the silence has crossed the line, which
        // takes 0.6 s upstream here.
        assert!(after < 3.5, "server stops {server_stops}: {after} s");
    }
}

/// And a far end that is still there is never taken for one that has gone:
/// a softphone's gain control and jitter buffer, a VoIP round trip, a
/// renegotiation from each end, and data all the while.
#[test]
fn a_softphone_line_keeps_its_carrier_through_data_and_renegotiations() {
    let net = Network::new(Law::Mu, FS)
        .with_delay(0.6, FS)
        .with_noise(1e-5)
        .with_gain_control(0.8, 0.3)
        .with_slips(2.9, true);
    let mut call = connects(net, server(), 40.0);
    let watch = |call: &mut FullCall, seconds: f64| {
        let end = call.ticks + (seconds * 8000.0) as u64;
        while call.ticks < end {
            call.run_until_seconds((call.ticks + 1) as f64 / 8000.0);
            assert!(call.analogue.carrier(), "the analogue modem lost a server that is there, at {} s", call.ticks / 8000);
            assert!(call.digital.carrier(), "the server lost a client that is there, at {} s", call.ticks / 8000);
        }
    };
    watch(&mut call, 8.0);
    assert!(call.digital.renegotiate(8));
    watch(&mut call, 8.0);
    assert!(call.analogue.renegotiate(40_000));
    watch(&mut call, 8.0);
    assert!(call.analogue.is_v90());
}

impl FullCall {
    /// Run for `seconds`, and say what share of the digital modem's output
    /// power lay above 3.8 kHz.
    fn top_of_band(&mut self, seconds: f64) -> f64 {
        const BLOCK: usize = 256;
        let mut sent = Vec::new();
        let end = self.ticks + (seconds * 8000.0) as u64;
        while self.ticks < end {
            let to_digital = self.net.up(&self.up);
            self.up.clear();
            let from_digital = self.digital.step(to_digital);
            sent.push(from_digital);
            for x in self.net.down(from_digital) {
                self.up.push(self.analogue.step(x));
            }
            self.ticks += 1;
        }
        let (mut top, mut all) = (0.0, 0.0);
        for block in sent.as_chunks::<BLOCK>().0 {
            for k in 0..=BLOCK / 2 {
                let (mut re, mut im) = (0.0, 0.0);
                for (n, x) in block.iter().enumerate() {
                    let w = 2.0 * std::f64::consts::PI * (k * n) as f64 / BLOCK as f64;
                    re += x * w.cos();
                    im -= x * w.sin();
                }
                let power = re * re + im * im;
                all += power;
                if k as f64 * 8000.0 / BLOCK as f64 >= 3800.0 {
                    top += power;
                }
            }
        }
        top / all
    }
}

fn plain_line() -> Network {
    Network::new(Law::Mu, FS).with_delay(0.020, FS).with_noise(1e-5)
}

/// A path that takes the top of the downstream's band away, as a live call
/// over a VoIP provider's did (live-1789732858). No equaliser gives back a
/// band that is not there, and what the equaliser cannot undo rings on in
/// every decision: unshaped, the route read twice as noisy as a clean one and
/// came up at 44 000, five rungs short of a clean line's 50 666.
///
/// So the analogue modem asks for spectral shaping (5.4.5): signs spent so
/// that the digital modem sends next to nothing where the ring is, with a
/// filter whose zero is at 4 kHz, in CP and CPt both, and the look-ahead the
/// digital modem's Jd offers. The digital modem sends by it, the decisions
/// ring far less, and the downstream comes up faster than it did unshaped --
/// and far faster than the V.34 it would have fallen back to.
#[test]
fn a_band_edge_cut_is_shaped_away() {
    use datapump::v90::shaping::Shaping;
    use datapump::v90::sign::Redundancy;
    let mut call = connects(plain_line().with_band_edge_cut(), server(), 30.0);
    assert!(call.analogue.is_v90());
    assert_eq!(call.analogue.retrains(), 0);
    let (down, _) = call.rates();
    let v90 = call.analogue.v90().unwrap();
    let (asked, left) = v90.shaping();
    let v34 = v90.settings().v34_receive;
    println!("{down} down with {asked:?}, expected to leave {left:.2} of the error; V.34 would carry {v34}");
    assert_ne!(asked.redundancy, Redundancy::None);
    assert!(asked.filter[0] <= -56, "no zero at 4 kHz: {:?}", asked.filter);
    assert_eq!(asked.lookahead, 1, "not the look-ahead our digital modem's Jd offers");
    assert!(down >= 48_000, "{down}");
    assert!(down > v34.max(33_600));
    // The CP and the CPt that went out ask for it, and the digital modem
    // took them at their word.
    let digital = call.digital.v90().unwrap();
    assert_eq!(digital.cp().map(Shaping::of), Some(asked));
    assert_eq!(digital.cpt().map(Shaping::of), Some(asked));
    // What goes down has next to nothing at the top of the band: a white
    // signal has a twentieth of its power above 3.8 kHz.
    let top = call.top_of_band(1.0);
    println!("{top:.3} of the power above 3.8 kHz");
    assert!(top < 0.025, "{top:.3} above 3.8 kHz");
    // And the decisions are the better for it: data mode reads more cleanly
    // than TRN1d, unshaped, did.
    let rx = call.analogue.v90().unwrap().receiver();
    println!("trained {:.1} dB, data mode {:.1} dB", rx.trained_snr_db(), rx.snr_db());
    assert!(rx.snr_db() > rx.trained_snr_db() + 2.0);
    assert_eq!(call.carries_data(4.0), (true, true));
}

/// The shaping holds through a rate renegotiation from either end (9.6):
/// each new CP asks for what the first did, TRN2d, MP and Ed go out with it
/// (8.6), and data after.
#[test]
fn a_shaped_call_renegotiates_from_either_end() {
    use datapump::v90::shaping::Shaping;
    let mut call = connects(plain_line().with_band_edge_cut(), server(), 30.0);
    let (asked, _) = call.analogue.v90().unwrap().shaping();
    let (down, _) = call.rates();
    assert!(call.analogue.renegotiate(down - 4000));
    assert!(call.comes_back_up(10.0), "{} / {}", call.analogue.phase(), call.digital.phase());
    let (slower, _) = call.rates();
    assert!(slower < down, "{slower} after asking for less than {down}");
    assert_eq!(call.digital.v90().unwrap().cp().map(Shaping::of), Some(asked));
    assert_eq!(call.carries_data(3.0), (true, true));
    assert!(call.digital.renegotiate(8));
    assert!(call.comes_back_up(10.0), "{} / {}", call.analogue.phase(), call.digital.phase());
    assert_eq!(call.carries_data(3.0), (true, true));
    assert_eq!(call.analogue.retrains(), 0);
}

/// A clean line leaves nothing at the top of the band worth a sign a frame:
/// no shaping is asked for -- CP's Sr is 0, "spectral shaping is disabled"
/// (5.4.5) -- and the rate is what it always was.
#[test]
fn a_clean_line_asks_for_no_shaping() {
    use datapump::v90::shaping::Shaping;
    let call = connects(plain_line(), server(), 30.0);
    assert_eq!(call.analogue.v90().unwrap().shaping().0, Shaping::NONE);
    let digital = call.digital.v90().unwrap();
    assert_eq!(digital.cp().map(Shaping::of), Some(Shaping::NONE));
    assert_eq!(digital.cpt().map(Shaping::of), Some(Shaping::NONE));
    assert_eq!(call.rates().0, 50_666);
}

/// Known data for the downstream: a sequence in which every bit is the
/// exclusive or of the bits 28 and 31 before it, so that what arrives is
/// checked against itself, bit by bit. Nothing has to be lined up, and a
/// stretch a renegotiation drops spoils only the blocks either side of it.
#[derive(Debug, Clone)]
struct Known(u32);

impl Known {
    fn next(&mut self) -> bool {
        let bit = ((self.0 >> 27) ^ (self.0 >> 30)) & 1 == 1;
        self.0 = ((self.0 << 1) | u32::from(bit)) & 0x7fff_ffff;
        bit
    }
}

/// Bits the known data is checked in, 128 octets' worth: a block with any
/// bit wrong is errored, as a frame carrying it would be lost.
const BLOCK_BITS: u64 = 1024;

/// What has arrived of the known data.
#[derive(Debug, Clone, Copy, Default)]
struct Checked {
    /// The last 31 bits, newest lowest, and how many there have been.
    last: u32,
    have: u32,
    /// Bits checked, blocks of them, blocks with a bit the bits before it
    /// said should have been otherwise, and whether the block under way has
    /// one.
    bits: u64,
    blocks: u64,
    errored: u64,
    wrong: bool,
}

impl Checked {
    fn feed(&mut self, bit: bool) {
        if self.have == 31 {
            let expected = ((self.last >> 27) ^ (self.last >> 30)) & 1 == 1;
            self.wrong |= bit != expected;
            self.bits += 1;
            if self.bits.is_multiple_of(BLOCK_BITS) {
                self.blocks += 1;
                self.errored += u64::from(self.wrong);
                self.wrong = false;
            }
        } else {
            self.have += 1;
        }
        self.last = ((self.last << 1) | u32::from(bit)) & 0x7fff_ffff;
    }

    /// Errored blocks, and blocks, since `before`.
    fn since(&self, before: &Self) -> (u64, u64) {
        (self.errored - before.errored, self.blocks - before.blocks)
    }
}

/// The downstream's known data: what goes, and what has arrived of it.
#[derive(Debug, Clone)]
struct Downstream {
    known: Known,
    checked: Checked,
}

impl Downstream {
    fn new() -> Self {
        Self { known: Known(0x1234_5678), checked: Checked::default() }
    }
}

impl FullCall {
    fn seconds(&self) -> f64 {
        self.ticks as f64 / 8000.0
    }

    /// Carry on until `done`, or for `seconds`, with known data going down
    /// all the while and what arrives of it checked. Whether `done` came.
    fn known_data_until(&mut self, seconds: f64, data: &mut Downstream, mut done: impl FnMut(&Self) -> bool) -> bool {
        let end = self.ticks + (seconds * 8000.0) as u64;
        while self.ticks < end {
            // Kept topped up: a digital modem with nothing to send sends ones.
            while self.digital.accepts_bits() && self.digital.pending_bits() < 4 * BLOCK_BITS as usize {
                let bits: Vec<bool> = (0..BLOCK_BITS).map(|_| data.known.next()).collect();
                self.digital.send_bits(&bits);
            }
            let to_digital = self.net.up(&self.up);
            self.up.clear();
            let from_digital = self.digital.step(to_digital);
            for x in self.net.down(from_digital) {
                self.up.push(self.analogue.step(x));
            }
            self.ticks += 1;
            for bit in self.analogue.take_bits() {
                data.checked.feed(bit);
            }
            self.digital.take_bits();
            if done(self) {
                return true;
            }
        }
        false
    }

    fn known_data(&mut self, seconds: f64, data: &mut Downstream) {
        self.known_data_until(seconds, data, |_| false);
    }

    /// Carry on with known data until both ends are back in data mode,
    /// having left it; false if that takes more than `seconds`.
    fn comes_back_up_with(&mut self, seconds: f64, data: &mut Downstream) -> bool {
        use datapump::v90::startup::Status;
        let up = |s: Status| matches!(s, Status::Connected { .. });
        let mut went_down = false;
        self.known_data_until(seconds, data, |call| {
            let both = up(call.analogue.status()) && up(call.digital.status());
            went_down |= !both;
            went_down && both
        })
    }
}

/// When the disturbances below begin: well into data mode, which a line
/// 20 ms each way reaches in under six seconds.
const DISTURBED_FROM: f64 = 8.0;

/// Seconds of known data a disturbed call is judged on.
const JUDGED: f64 = 15.0;

/// Noise that comes and goes: a tenth of a second of it every second and a
/// half, about ten decibels over the error a clean line leaves in the
/// decisions.
fn bursty_line() -> Network {
    plain_line().with_bursts(DISTURBED_FROM, 1.5, 0.1, 1e-3)
}

/// A floor that steps up, to about twice the error a clean line leaves in
/// the decisions: where it makes errors every few seconds at the rate a clean
/// line came up at.
fn stepped_line() -> Network {
    plain_line().with_rising_noise(DISTURBED_FROM, 0.0, 6e-4)
}

/// A disturbed call: connected, and carrying known data clean until the
/// disturbance begins.
fn disturbed(net: Network) -> (FullCall, Downstream) {
    let mut call = connects(net, server(), 30.0);
    assert!(call.seconds() < DISTURBED_FROM - 1.0, "connected at {:.1} s", call.seconds());
    let mut data = Downstream::new();
    call.known_data(DISTURBED_FROM - call.seconds(), &mut data);
    (call, data)
}

/// Known data over `JUDGED` seconds: errored blocks, and blocks.
fn judged(call: &mut FullCall, data: &mut Downstream) -> (u64, u64) {
    let before = data.checked;
    call.known_data(JUDGED, data);
    data.checked.since(&before)
}

/// A disturbed call left to the analogue modem: it renegotiates (9.6.2.1) of
/// its own accord, within `JUDGED` seconds of the disturbance beginning, and
/// is then judged on known data at the rate it settled on. The rate before,
/// the seconds it took, the rate after, and errored blocks and blocks there.
fn falls_back(call: &mut FullCall, data: &mut Downstream) -> (u32, f64, u32, u64, u64) {
    let (fast, _) = call.rates();
    let began = call.seconds();
    assert!(call.comes_back_up_with(JUDGED, data), "never renegotiated: {} / {}", call.analogue.phase(), call.digital.phase());
    let took = call.seconds() - began;
    // What the renegotiation dropped is not the line's doing.
    call.known_data(1.0, data);
    let (errored, blocks) = judged(call, data);
    (fast, took, call.rates().0, errored, blocks)
}

/// Noise that comes and goes -- a tenth of a second of it every second and a
/// half -- is seen: each burst's misses add to the evidence, and within a few
/// bursts the analogue modem renegotiates, once, to a rate chosen for the
/// worst of them, where the same bursts spoil nothing and ask for nothing
/// more. At the rate the call came up at, fifteen seconds of them spoiled 35
/// of 742 blocks of known data, and no renegotiation came.
#[test]
fn noise_that_comes_and_goes_is_renegotiated_down_to_a_rate_that_reads_it() {
    let (mut call, mut data) = disturbed(bursty_line());
    let (fast, took, slower, errored, blocks) = falls_back(&mut call, &mut data);
    println!("{fast} became {slower} after {took:.1} s of bursts; then {errored} of {blocks} blocks errored");
    assert!(slower < fast, "{slower} against {fast}");
    assert_eq!(errored, 0, "{errored} of {blocks} blocks errored at {slower}");
    assert_eq!(call.analogue.renegotiations(), 1);
    assert_eq!(call.analogue.retrains(), 0);
    assert_eq!(call.rates().0, slower);
}

/// A burst shorter than the 32 ms block the rate is chosen over is still the
/// line's own, and is still held against the rate: the block only dilutes
/// what it asks for, since the power of a burst that fills a third of it
/// reads as a third of the burst's. Thirty milliseconds of noise every second
/// and a half takes 50 666 to 41 333 over a 20 ms round trip, and ten
/// milliseconds of it takes 54 666 to 50 666 over a 0.6 s one.
///
/// Five milliseconds asks for nothing, at either round trip, and has nothing
/// to ask for: it errors 6 of 1484 blocks in thirty seconds and 23 of 1602,
/// where a clean line over the round trip errors 2 of 1602 and the packets a
/// jitter buffer loses error 16 to 56.
#[test]
fn a_burst_shorter_than_the_block_is_still_held_against_the_rate() {
    let (mut call, mut data) = disturbed(plain_line().with_bursts(DISTURBED_FROM, 1.5, 0.03, 1e-3));
    let (fast, took, slower, errored, blocks) = falls_back(&mut call, &mut data);
    println!("{fast} became {slower} {took:.1} s into bursts of 30 ms; then {errored} of {blocks} blocks errored");
    assert!(slower < fast, "{slower} against {fast}");
    assert_eq!(call.analogue.renegotiations(), 1);
    assert_eq!(call.analogue.retrains(), 0);
    // Ten milliseconds of it over the round trip is seen as well.
    falls_back_over_the_round_trip(voip_line().with_bursts(VOIP_DISTURBED_FROM, 1.5, 0.01, 1e-3));
    // Five is not, at either round trip.
    assert_eq!(left_alone(plain_line().with_bursts(DISTURBED_FROM, 1.5, 0.005, 1e-3), WATCHED), (0, 0), "5 ms, 20 ms each way");
    assert_eq!(left_alone(voip_line().with_bursts(VOIP_DISTURBED_FROM, 1.5, 0.005, 1e-3), WATCHED), (0, 0), "5 ms, 0.6 s each way");
}

/// A floor that steps up to where the levels stand only about seven RMS
/// errors apart -- right on the line the old watch on the averaged error drew,
/// so that it never saw it -- makes a miss or two in every look, and errors
/// every few seconds. The misses add up, and the analogue modem renegotiates
/// once, to a rate that reads the new floor cleanly. At the rate the call
/// came up at, fifteen seconds of it spoiled 5 of 742 blocks.
#[test]
fn a_floor_that_steps_up_is_renegotiated_down_to_a_rate_that_reads_it() {
    let (mut call, mut data) = disturbed(stepped_line());
    let (fast, took, slower, errored, blocks) = falls_back(&mut call, &mut data);
    println!("{fast} became {slower} {took:.1} s after the step; then {errored} of {blocks} blocks errored");
    assert!(slower < fast, "{slower} against {fast}");
    assert_eq!(errored, 0, "{errored} of {blocks} blocks errored at {slower}");
    assert_eq!(call.analogue.renegotiations(), 1);
    assert_eq!(call.analogue.retrains(), 0);
    assert_eq!(call.rates().0, slower);
}

/// Seconds of data mode an undisturbed call is watched for.
const WATCHED: f64 = 30.0;

/// Run `seconds` of data mode with known data, and say how many
/// renegotiations and retrains there were.
fn left_alone(net: Network, seconds: f64) -> (u32, u32) {
    left_alone_with(net, server(), seconds)
}

/// The same, against a server of one's own choosing.
fn left_alone_with(net: Network, server: Info0d, seconds: f64) -> (u32, u32) {
    let mut call = connects(net, server, 40.0);
    let (rate, _) = call.rates();
    let mut data = Downstream::new();
    // What arrived before the known data did is not the line's doing.
    call.known_data(1.0, &mut data);
    let before = data.checked;
    call.known_data(seconds, &mut data);
    let (errored, blocks) = data.checked.since(&before);
    let (renegotiations, retrains) = (call.analogue.renegotiations(), call.analogue.retrains());
    println!("{rate}: {renegotiations} renegotiations, {retrains} retrains in {seconds} s; {errored} of {blocks} blocks errored, {} slips", call.net.slips());
    (renegotiations, retrains)
}

/// A clean line, and one whose sound card runs 120 ppm off the network's
/// clock, give the watch on the margin nothing: no renegotiation, as none
/// before the watch counted misses.
#[test]
fn a_clean_line_and_a_drifting_clock_are_left_at_their_rates() {
    assert_eq!(left_alone(plain_line(), WATCHED), (0, 0));
    assert_eq!(left_alone(plain_line().with_clock(120.0), WATCHED), (0, 0));
}

/// A softphone's jitter buffer slips twenty milliseconds every few seconds,
/// made up or dropped, and each slip is a burst of garbage no slower rate
/// reads any better; the stretch of misses says a packet did it, and it is
/// not held against the rate. Nor is the margin a call over a VoIP round trip
/// comes up with, whose errors are half a minute apart. No renegotiation, as
/// none before.
///
/// The softphone's gain control is in the route and takes no part in it, and
/// the name no longer says it does. Data mode never gets near the ceiling one
/// sets: at 0.8 of full scale, which is where a live call's sat, the gain
/// never leaves 1 at all, and at 0.5 it moves only in the start-up -- where
/// the DIL sweeps every codeword, louder than anything data mode sends -- and
/// then not once in thirty seconds of data mode. So that is what is asserted,
/// rather than that a gain control which never engaged was not held against
/// the rate.
#[test]
fn a_softphone_s_slips_are_left_at_their_rates_and_its_gain_control_never_engages() {
    for (period, inserted, ceiling, in_the_start_up) in [(2.9, true, 0.8, false), (3.1, false, 0.8, false), (2.9, true, 0.5, true)] {
        let net = Network::new(Law::Mu, FS)
            .with_delay(0.6, FS)
            .with_noise(1e-5)
            .with_gain_control(ceiling, 0.3)
            .with_slips(period, inserted);
        let mut call = connects(net, server(), 40.0);
        let (rate, _) = call.rates();
        let started = call.net.quietest_gain();
        let mut data = Downstream::new();
        // What arrived before the known data did is not the line's doing.
        call.known_data(1.0, &mut data);
        let before = data.checked;
        call.known_data(WATCHED, &mut data);
        let (errored, blocks) = data.checked.since(&before);
        let (renegotiations, retrains) = (call.analogue.renegotiations(), call.analogue.retrains());
        println!(
            "{rate}, ceiling {ceiling}, slips every {period} s: {renegotiations} renegotiations, {retrains} retrains in {WATCHED} s; {errored} of {blocks} blocks errored; the gain control went to {started} in the start-up and to {} in data mode",
            call.net.quietest_gain()
        );
        let case = format!("ceiling {ceiling}, slips every {period} s, inserted {inserted}");
        assert_eq!((renegotiations, retrains), (0, 0), "{case}");
        assert_eq!(call.net.quietest_gain(), started, "{case}: the gain control engaged in data mode");
        assert_eq!(started < 1.0, in_the_start_up, "{case}: the start-up left the gain at {started}");
    }
}

/// A packet of the downstream lost and concealed where it was, every three
/// seconds: the buffer plays the last packet over again, fading, or nothing
/// at all, in the place the lost one would have filled.
fn dropped_line(every: f64, repeat: bool) -> Network {
    dropped_line_of(every, 0.02, repeat)
}

/// The same, of a packet of any length: a softphone carries ten, twenty or
/// thirty milliseconds of G.711 in one.
fn dropped_line_of(every: f64, length: f64, repeat: bool) -> Network {
    voip_line().with_dropout(VOIP_DISTURBED_FROM, every, length, repeat)
}

/// A VoIP call's round trip: 0.6 s each way, which comes up at 54 666 -- the
/// rate Rory's own line comes up at, and the one with least margin to spare.
fn voip_line() -> Network {
    Network::new(Law::Mu, FS).with_delay(0.6, FS).with_noise(1e-5)
}

/// When a disturbance over that round trip begins: a start-up 0.6 s each way
/// takes a dozen seconds, and what lands in one is the start-up's business,
/// not data mode's.
const VOIP_DISTURBED_FROM: f64 = 20.0;


/// A packet of the downstream slipped every three seconds, thirty
/// milliseconds of it: 240 codewords, which is 40 whole frames.
fn slipped_line(inserted: bool) -> Network {
    voip_line().with_slips_of(3.0, 240, inserted)
}

/// A packet lost and concealed where it was is not held against the rate.
/// Made-up audio is garbage however much of it there is, and a slower rate
/// reads it no better: the same bits are lost at 28 000 as at 54 666, and the
/// rest of the call pays for it. Nothing moves and nothing goes quiet, so
/// neither of the things that used to mark a look as not the line's happens
/// here -- the garbage itself has to say so.
///
/// Every length a softphone carries in a packet, since the length is the one
/// thing the rule may not depend on: ten, twenty and thirty milliseconds,
/// concealed by a fading repeat and by comfort noise, every second and a half
/// and every three seconds. At ten milliseconds and a fading repeat this cost
/// a renegotiation before the garbage was weighed by what its misses carry --
/// 54 666 to 50 666, which the packets were no better read at.
#[test]
fn a_packet_lost_and_concealed_in_place_is_left_at_its_rate() {
    for length in [0.010, 0.020, 0.030] {
        for (every, repeat) in [(1.5, true), (3.0, true), (1.5, false), (3.0, false)] {
            let case = format!("{} ms every {every} s, repeat {repeat}", length * 1000.0);
            assert_eq!(left_alone(dropped_line_of(every, length, repeat), WATCHED), (0, 0), "{case}");
        }
    }
}


/// A packet lost and filled with digital silence is not held against the
/// rate either. Several softphones play zeroes rather than conceal, and a
/// hole is no more the line's than made-up audio is: no rate reads a codeword
/// that never arrived. Nothing is made up, so the sound the misses carry
/// cannot say so -- silence carries none -- and the silence itself has to.
///
/// Over the 20 ms round trip, and over A-law's 0.6 s one, the hole costs the
/// call only the packets it lost: no renegotiation, no retrain, and 39 to 41
/// blocks of about 1500 errored in thirty seconds, which is what main errors
/// over the same audio.
#[test]
fn a_packet_lost_and_filled_with_silence_is_left_at_its_rate() {
    let short = plain_line().with_silent_dropout(DISTURBED_FROM, 1.5, 0.02);
    assert_eq!(left_alone(short, WATCHED), (0, 0), "20 ms each way");
    let long = Network::new(Law::A, FS).with_delay(0.6, FS).with_noise(1e-5).with_silent_dropout(VOIP_DISTURBED_FROM, 1.5, 0.02);
    assert_eq!(left_alone_with(long, a_law_server(), WATCHED), (0, 0), "A-law, 0.6 s each way");
}

/// A hole does more than lose its own packet on one route: mu-law at 54 666
/// over the 0.6 s round trip, where the levels stand closest together, the
/// receiver loses the constellation on the first hole and does not get it
/// back. That is a receiver to train again and not a rate to drop -- a
/// slower rate does not hand back a lost constellation -- and the watch on
/// the decisions has nothing to say about it either way, since a receiver
/// that is not reading the constellation is no judge of what the line would
/// carry.
///
/// So it retrains, once, and comes back slower: 54 666 to 44 000, with no
/// renegotiation, and then reads the same holes at 44 000 with 24 of 644
/// blocks errored. Over thirty seconds of it main errors 140 of 619 and 65
/// of 378 in the last ten, and this errors the same; before the hole was
/// judged at all, the branch renegotiated as well, never came back up, and
/// errored 432 of 598 with every one of the last 96 gone. What the receiver
/// does with a hole is worth mending, and that it is not mended here is not
/// this watch's doing.
#[test]
fn a_hole_that_costs_the_receiver_its_constellation_is_retrained_and_not_slowed() {
    let net = voip_line().with_silent_dropout(VOIP_DISTURBED_FROM, 1.5, 0.02);
    let mut call = connects(net, server(), 40.0);
    let (fast, _) = call.rates();
    let mut data = Downstream::new();
    call.known_data(VOIP_DISTURBED_FROM - call.seconds(), &mut data);
    let before = data.checked;
    assert!(call.comes_back_up_with(WATCHED, &mut data), "never came back: {} / {}", call.analogue.phase(), call.digital.phase());
    let (slower, _) = call.rates();
    let (renegotiations, retrains) = (call.analogue.renegotiations(), call.analogue.retrains());
    let (through, whole) = data.checked.since(&before);
    // And then, at the slower rate, with the holes still coming.
    let after = data.checked;
    call.known_data(JUDGED, &mut data);
    let (errored, blocks) = data.checked.since(&after);
    println!("{fast} became {slower}; {through} of {whole} blocks errored getting there, then {errored} of {blocks}");
    assert!(slower < fast, "{slower} against {fast}");
    assert_eq!((renegotiations, retrains), (0, 1));
    assert!(errored * 10 < blocks, "{errored} of {blocks} blocks errored at {slower}");
}

/// The same server, A-law.
fn a_law_server() -> Info0d {
    let mut server = server();
    server.a_law = true;
    server
}

/// And a slip of a whole number of frames is not held against it either.
/// 240 codewords is 40 of V.90's six-codeword frames (7.1), so the frames are
/// found exactly where they were left and `frames_moved` never changes: as
/// with a packet concealed in place, only the garbage says it happened.
/// Before the garbage was judged, a 30 ms slip every three seconds cost the
/// call one renegotiation and one retrain in thirty seconds, and 22 of 1119
/// blocks of known data.
#[test]
fn a_slip_of_a_whole_number_of_frames_is_left_at_its_rate() {
    for inserted in [true, false] {
        assert_eq!(left_alone(slipped_line(inserted), WATCHED), (0, 0), "inserted {inserted}");
    }
}

/// A call over the round trip, disturbed from [`VOIP_DISTURBED_FROM`]: it
/// renegotiates once, to a slower rate, with no retrain. The rate before and
/// the rate after.
fn falls_back_over_the_round_trip(net: Network) -> (u32, u32) {
    let mut call = connects(net, server(), 40.0);
    assert!(call.seconds() < VOIP_DISTURBED_FROM - 1.0, "connected at {:.1} s", call.seconds());
    let (fast, _) = call.rates();
    let mut data = Downstream::new();
    call.known_data(VOIP_DISTURBED_FROM - call.seconds(), &mut data);
    assert!(call.comes_back_up_with(20.0, &mut data), "never renegotiated: {} / {}", call.analogue.phase(), call.digital.phase());
    let (slower, _) = call.rates();
    // What the renegotiation dropped is not the line's doing.
    call.known_data(1.0, &mut data);
    let before = data.checked;
    call.known_data(JUDGED, &mut data);
    let (errored, blocks) = data.checked.since(&before);
    println!("{fast} became {slower}; then {errored} of {blocks} blocks errored, {} slips", call.net.slips());
    assert!(slower < fast, "{slower} against {fast}");
    assert_eq!(call.analogue.renegotiations(), 1);
    assert_eq!(call.analogue.retrains(), 0);
    (fast, slower)
}

/// And noise that comes and goes between the lost packets is still seen: a
/// hundred milliseconds of it every second and a half is the line's own, is
/// nothing like a packet, and the analogue modem renegotiates once for it.
#[test]
fn noise_between_lost_packets_is_still_seen() {
    falls_back_over_the_round_trip(dropped_line(3.0, true).with_bursts(VOIP_DISTURBED_FROM, 1.5, 0.1, 1e-3));
}


/// The same between slipped frames.
#[test]
fn noise_between_slipped_frames_is_still_seen() {
    falls_back_over_the_round_trip(slipped_line(true).with_bursts(VOIP_DISTURBED_FROM, 1.5, 0.1, 1e-3));
}

/// Bursts of noise between a softphone's slips: the slips are still not
/// held against the rate, and the bursts still are -- one renegotiation.
#[test]
fn bursts_of_noise_between_slips_are_still_seen() {
    let net = Network::new(Law::Mu, FS)
        .with_delay(0.6, FS)
        .with_noise(1e-5)
        .with_slips(2.9, true)
        .with_bursts(20.0, 1.5, 0.1, 1e-3);
    let mut call = connects(net, server(), 40.0);
    assert!(call.seconds() < 19.0, "connected at {:.1} s", call.seconds());
    let (fast, _) = call.rates();
    let mut data = Downstream::new();
    call.known_data(20.0 - call.seconds(), &mut data);
    assert!(call.comes_back_up_with(20.0, &mut data), "never renegotiated: {} / {}", call.analogue.phase(), call.digital.phase());
    let (slower, _) = call.rates();
    call.known_data(JUDGED, &mut data);
    println!("{fast} became {slower}; {} slips", call.net.slips());
    assert!(slower < fast, "{slower} against {fast}");
    assert_eq!(call.analogue.renegotiations(), 1);
    assert_eq!(call.analogue.retrains(), 0);
}

/// A renegotiation may not take more than eight bits a frame -- 10 666 bit/s
/// -- off the downstream in one go. A line that has suddenly become far
/// worse than the DIL found it is stepped down that far, measured again at
/// the new rate, and stepped again if it really is that bad, rather than
/// falling as far as one 32 ms block said in a single renegotiation.
///
/// Two thousandths of full scale, which is about a hundred times the noise
/// the call came up on: enough that one step cannot reach the rate it wants
/// and a second follows, and not so much that the receiver loses the
/// constellation, which is a retrain and no business of this rule's.
#[test]
fn a_renegotiation_steps_the_rate_down_by_no_more_than_eight_bits_a_frame() {
    let mut call = connects(plain_line(), server(), 30.0);
    let (down, _) = call.rates();
    call.net.set_noise(2e-3);
    assert!(call.comes_back_up(10.0), "{} / {}", call.analogue.phase(), call.digital.phase());
    let (stepped, _) = call.rates();
    println!("{down} became {stepped} in one renegotiation");
    assert_eq!(call.analogue.renegotiations(), 1);
    assert_eq!(down - stepped, 10_666, "{down} to {stepped} in one go");
    // And the line really is that much worse: the next renegotiation goes
    // further, with no retrain in between.
    assert!(call.comes_back_up(10.0), "{} / {}", call.analogue.phase(), call.digital.phase());
    let (slower, _) = call.rates();
    println!("and then {slower}, after {} renegotiations", call.analogue.renegotiations());
    assert!(slower < stepped, "{slower} against {stepped}");
    assert_eq!(call.analogue.retrains(), 0);
    assert_eq!(call.carries_data(3.0), (true, true));
}


/// How long a hole is watched for when what is being counted is the holes
/// themselves: two minutes, eight times the shortest stretch a call takes to
/// settle, so that a rule that only holds at first shows here.
const SOAKED: f64 = 120.0;

/// A hole shows on the line in front of the equaliser, and nothing else
/// does.
///
/// A hole is the one disturbance that leaves no mark in the decisions: the
/// equaliser is 63 half symbols of line and a feedback filter of its own
/// past decisions, so it goes on putting out codeword-sized numbers while
/// nothing at all arrives. Measured over two minutes of twenty-millisecond
/// holes, the longest run of decisions under a ten-thousandth of the
/// decisions' own level was 2, where a clean line's was 1 and a concealer's
/// fading repeat gave 7 -- there is nothing there to tell a hole by. So the
/// line itself is what is counted, and this is what makes that worth doing:
/// every hole the network makes is seen, and nothing that is not a hole is
/// ever taken for one.
///
/// A hole every second and a half for a hundred and twenty seconds is eighty
/// of them; the count is short of that by the ones that land while the call
/// is retraining, when there is no data mode to watch.
#[test]
fn every_hole_is_seen_on_the_line_and_nothing_else_is_ever_taken_for_one() {
    for length in [0.010, 0.020, 0.030, 0.040, 0.060] {
        let net = plain_line().with_silent_dropout(DISTURBED_FROM, 1.5, length);
        let holes = holes_seen(net, server(), SOAKED);
        let made = (SOAKED / 1.5) as u32;
        assert!(holes >= made - 2, "{} ms: {holes} holes of {made}", length * 1000.0);
    }
    // And everything else the line does, for as long: made-up audio of both
    // kinds, noise that comes and goes, a floor that steps up, a slip, and
    // nothing at all.
    let quiet: [(&str, Network); 8] = [
        ("nothing", plain_line()),
        ("a fading repeat, 20 ms", dropped_line_of(1.5, 0.020, true)),
        ("a fading repeat, 60 ms", dropped_line_of(1.5, 0.060, true)),
        ("comfort noise, 20 ms", dropped_line_of(1.5, 0.020, false)),
        ("comfort noise, 60 ms", dropped_line_of(1.5, 0.060, false)),
        ("bursts of noise", bursty_line()),
        ("a floor that steps up", stepped_line()),
        ("a slip of whole frames", slipped_line(true)),
    ];
    for (what, net) in quiet {
        assert_eq!(holes_seen(net, server(), SOAKED), 0, "{what}");
    }
}

/// Holes seen in `seconds` of data mode.
fn holes_seen(net: Network, server: Info0d, seconds: f64) -> u32 {
    let mut call = connects(net, server, 40.0);
    let mut data = Downstream::new();
    call.known_data(seconds, &mut data);
    let holes = call.analogue.holes();
    println!("{holes} holes in {seconds} s at {}", call.rates().0);
    holes
}

/// And a call through holes never asks for a slower rate, however long it
/// goes on.
///
/// Thirty seconds was not long enough to show what was wrong here. The rule
/// that was meant to catch a hole read the decisions, where a hole leaves no
/// mark, and never fired; the holes built evidence like any other errors,
/// and the call renegotiated -- not once but twice, the second a good minute
/// in, ending slower than the same audio leaves a tree without the rule at
/// all. Two minutes, in ten-second stretches, is what shows it: the rate is
/// the same in the last stretch as in the first, and the errors are the
/// holes' own packets and nothing more.
///
/// Measured over the mu-law 0.6 s round trip: at twenty milliseconds the
/// call retrains once -- the receiver loses the constellation, which is its
/// own business and not the rate's, and is what
/// [`a_hole_that_costs_the_receiver_its_constellation_is_retrained_and_not_slowed`]
/// is about -- and settles at 44 000, where main settles too, erring 11 to
/// 17 of about 430 blocks in every stretch afterwards. At thirty it settles
/// at 42 666 against main's 42 666, erring 13 to 20 of about 417. Neither
/// renegotiates at all.
#[test]
fn a_call_through_holes_never_renegotiates_however_long_it_goes_on() {
    for length in [0.020, 0.030] {
        let net = voip_line().with_silent_dropout(VOIP_DISTURBED_FROM, 1.5, length);
        let mut call = connects(net, server(), 40.0);
        let mut data = Downstream::new();
        call.known_data(VOIP_DISTURBED_FROM - call.seconds(), &mut data);
        let mut settled = None;
        let mut worst = (0, 0);
        for stretch in 0..(SOAKED / 10.0) as u32 {
            let before = data.checked;
            call.known_data(10.0, &mut data);
            let (errored, blocks) = data.checked.since(&before);
            // Once it has settled, it stays there: the rate never moves
            // again and the errors never grow.
            if let Some(rate) = settled {
                assert_eq!(call.rate_now(), Some(rate), "{} ms, stretch {stretch}", length * 1000.0);
                assert!(errored * 10 < blocks, "{} ms, stretch {stretch}: {errored} of {blocks}", length * 1000.0);
                worst = worst.max((errored, blocks));
            } else if call.analogue.retrains() > 0 && let Some(rate) = call.rate_now() {
                settled = Some(rate);
            }
        }
        let settled = settled.expect("never settled");
        println!("{} ms: settled at {settled}, worst stretch {}/{}", length * 1000.0, worst.0, worst.1);
        assert_eq!(call.analogue.renegotiations(), 0, "{} ms", length * 1000.0);
    }
}

/// A floor that steps up is read right the first time, over the round trip
/// as well.
///
/// The rate is chosen from the worst 32 ms block the recent looks reached,
/// which is what a burst wants; but a decision's error there is its distance
/// from the nearer of the two levels either side of it, so one that has
/// crossed a boundary is measured to the wrong level and can never be out by
/// more than half a gap. On a floor stepped up until it errors every few
/// seconds that reads the line better than it is: the worst block came to
/// 0.000325 where the receiver's own averaged error -- the same measurement
/// read a second way, and the one main uses -- came to 0.000518. The rate
/// chosen from the first was 50 666, which still errored 4 of 495 blocks and
/// then 4 of 357, and had to be asked again; it ended at 44 000, a rung
/// below the 45 333 main reaches in one go, for nothing.
///
/// Taking the larger of the two lands at 45 333 first time, and nothing
/// errors at all in any ten-second stretch of the two minutes after it.
#[test]
fn a_floor_that_steps_up_over_the_round_trip_is_read_right_the_first_time() {
    let mut call = connects(voip_line().with_rising_noise(VOIP_DISTURBED_FROM, 0.0, 6e-4), server(), 40.0);
    let mut data = Downstream::new();
    call.known_data(VOIP_DISTURBED_FROM - call.seconds(), &mut data);
    assert!(call.comes_back_up_with(20.0, &mut data), "never renegotiated: {}", call.analogue.phase());
    let slower = call.rates().0;
    // What the renegotiation itself dropped is not the line's doing.
    call.known_data(1.0, &mut data);
    for stretch in 0..(SOAKED / 10.0) as u32 {
        let before = data.checked;
        call.known_data(10.0, &mut data);
        let (errored, blocks) = data.checked.since(&before);
        assert_eq!(errored, 0, "stretch {stretch}: {errored} of {blocks} blocks errored at {slower}");
    }
    println!("stepped to 6e-4 over the round trip: 54666 became {slower}, nothing errored in {SOAKED} s");
    assert_eq!((call.analogue.renegotiations(), call.analogue.retrains()), (1, 0));
    assert_eq!(call.rates().0, slower);
}

/// A retrain storm is come out of no worse than main comes out of it.
///
/// Forty milliseconds of digital silence every three seconds over the
/// mu-law 0.6 s round trip costs the receiver its constellation again and
/// again, and both this and a tree without any of this watch fall into a
/// storm of retrains for it. That is the receiver's to mend and not the
/// watch's, and nothing here pretends to mend it. What the watch must not do
/// is make it worse.
///
/// It did. A receiver holding its loops is already on its way either back or
/// out -- back, and it was a burst it rode out on what it last knew; out,
/// and it is retrained three seconds later anyway -- so a retrain asked for
/// from the looks that made it hold is a second retrain landing in the
/// middle of the first one's recovery. Main takes five retrains here and is
/// back at 52 000 by about 105 seconds; this took a sixth on top of them and
/// never came back at all. With the count held while the receiver holds, the
/// two agree stretch for stretch: five retrains, back at 52 000 by 105 s,
/// then 9 to 14 of about 508 blocks errored in every ten-second stretch
/// after.
///
/// Only the count stands down, and that matters: a long burst of real noise
/// makes the receiver hold its loops as well, and the looks gathered through
/// one are exactly what say the line is bad. Standing those down too cost
/// three hundred milliseconds of noise every second and a half its fall back
/// altogether, leaving it at 54 666 for ever where it should reach 40 000
/// and error nothing after (see
/// [`noise_that_comes_and_goes_is_renegotiated_down_to_a_rate_that_reads_it`]).
#[test]
fn a_retrain_storm_is_come_out_of_and_the_watch_stands_down_inside_it() {
    let net = voip_line().with_silent_dropout(VOIP_DISTURBED_FROM, 3.0, 0.040);
    let mut call = connects(net, server(), 40.0);
    let mut data = Downstream::new();
    call.known_data(VOIP_DISTURBED_FROM - call.seconds(), &mut data);
    let mut last = (0, 0);
    let mut back_at = None;
    for _ in 0..(SOAKED / 10.0) as u32 {
        let before = data.checked;
        call.known_data(10.0, &mut data);
        last = data.checked.since(&before);
        if back_at.is_none() && call.rate_now().is_some() && call.analogue.retrains() >= 5 {
            back_at = Some(call.seconds());
        }
    }
    let rate = call.rate_now().expect("never came back out of the storm");
    let back_at = back_at.expect("never came back out of the storm");
    println!("out of the storm at {back_at:.0} s and {rate}, {} retrains; last stretch {}/{}", call.analogue.retrains(), last.0, last.1);
    assert!(back_at < VOIP_DISTURBED_FROM + 100.0, "came back only at {back_at:.0} s");
    assert!(last.0 * 10 < last.1, "{} of {} blocks errored at {rate}", last.0, last.1);
    assert_eq!(call.analogue.renegotiations(), 0);
}

/// A burst long enough that the receiver holds its loops through it is still
/// the line's, and is still fallen back for.
///
/// Three hundred milliseconds of noise every second and a half leaves the
/// receiver holding its loops about a fifth of the time. That is the case
/// that says what may stand down while it holds and what may not: the looks
/// gathered through such a burst are exactly what say the line is bad, and a
/// watch that threw them away because the receiver was holding never fell
/// back at all, sitting at 54 666 for ever with 107 to 145 of 534 blocks
/// errored in every stretch. Only the counting towards a retrain stands down
/// (see [`a_retrain_storm_is_come_out_of_and_the_watch_stands_down_inside_it`]).
///
/// Over the 0.6 s round trip it settles at 40 000, which is further than
/// [`MOST_DROPPED`] lets one renegotiation go, so it takes two: 54 666 to
/// 50 666, which still errors, and then to 40 000, where nothing errors in
/// any ten-second stretch of the two minutes after the one the last
/// renegotiation itself fell in. A robbed bit settles at
/// 40 000 too and A-law at 44 000, and no retrain comes of any of it.
#[test]
fn a_burst_the_receiver_holds_its_loops_through_is_still_fallen_back_for() {
    let cases: [(&str, Network, Info0d); 3] = [
        ("0.6 s each way", voip_line().with_bursts(VOIP_DISTURBED_FROM, 1.5, 0.3, 1e-3), server()),
        ("0.6 s each way, a robbed bit", voip_line().with_robbed_bit(0).with_bursts(VOIP_DISTURBED_FROM, 1.5, 0.3, 1e-3), server()),
        (
            "A-law, 0.6 s each way",
            Network::new(Law::A, FS).with_delay(0.6, FS).with_noise(1e-5).with_bursts(VOIP_DISTURBED_FROM, 1.5, 0.3, 1e-3),
            a_law_server(),
        ),
    ];
    for (what, net, server) in cases {
        let mut call = connects(net, server, 40.0);
        let fast = call.rates().0;
        let mut data = Downstream::new();
        call.known_data(VOIP_DISTURBED_FROM - call.seconds(), &mut data);
        // Two minutes of it in ten-second stretches, from where the bursts
        // begin: the rate it reached in each, and what errored there.
        let mut stretches = Vec::new();
        for _ in 0..(SOAKED / 10.0) as u32 {
            let before = data.checked;
            call.known_data(10.0, &mut data);
            stretches.push((call.rate_now(), data.checked.since(&before)));
        }
        // Once it has settled it stays there, and nothing errors again.
        let settled = stretches.last().expect("stretches").0.expect("never came back up");
        // The stretch the last renegotiation itself fell in still carries
        // what it dropped, which is not the line's doing.
        let after: Vec<_> = stretches.iter().skip_while(|(rate, _)| *rate != Some(settled)).skip(1).collect();
        println!("{what}: {fast} became {settled}, settled for the last {} stretches of {SOAKED} s", after.len());
        assert!(after.len() >= 8, "{what}: settled at {settled} only for {} stretches", after.len());
        for (n, (rate, (errored, blocks))) in after.iter().enumerate() {
            assert_eq!(*rate, Some(settled), "{what}, stretch {n} after settling");
            assert_eq!(*errored, 0, "{what}, stretch {n}: {errored} of {blocks} at {settled}");
        }
        assert!(settled < fast, "{what}: {settled} against {fast}");
        assert_eq!(call.analogue.retrains(), 0, "{what}");
    }
}
