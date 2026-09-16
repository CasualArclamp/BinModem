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
    up: [f64; 2],
    ticks: u64,
}

impl Call {
    fn new(net: Network) -> Self {
        let (a, d) = settled();
        Self { net, analogue: analogue::Modem::new(a, FS), digital: digital::Modem::new(d), up: [0.0; 2], ticks: 0 }
    }

    /// One network sample: 125 microseconds.
    fn tick(&mut self) {
        let to_digital = self.net.up(&self.up);
        let from_digital = self.digital.step(to_digital);
        for (k, x) in self.net.down(from_digital).into_iter().enumerate() {
            self.up[k] = self.analogue.step(x);
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
