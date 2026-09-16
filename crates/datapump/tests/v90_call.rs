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

#[test]
fn a_sound_card_clock_120_ppm_off_is_followed_through_ten_seconds_of_data() {
    let mut call = connects(Network::new(Law::Mu, FS).with_delay(0.020, FS).with_noise(1e-5).with_clock(120.0), server(), 30.0);
    let drift = call.analogue.v90().unwrap().receiver().drift_ppm();
    println!("drift read as {drift:.1} ppm");
    assert_eq!(call.carries_data(10.0), (true, true));
    let drift = call.analogue.v90().unwrap().receiver().drift_ppm();
    assert!((drift.abs() - 120.0).abs() < 20.0, "drift read as {drift:.1} ppm");
}
