//! x2 from the end of phase 2 to data and through a rate change, between the
//! server and the simulated client, on a digital line.
//!
//! The client is written from the same facts as the server, so this shows
//! that the pieces fit together, not that they are x2.

use datapump::v34::info::X2Marker;
use datapump::x2::client::Client;
use datapump::x2::server::{Modem, Settings, Status};

/// Each way's delay in samples: 10 ms.
const DELAY: usize = 80;

struct Call {
    server: Modem,
    client: Client,
    to_server: std::collections::VecDeque<f64>,
    to_client: std::collections::VecDeque<f64>,
    now: usize,
}

impl Call {
    fn new() -> Self {
        let marker = X2Marker::from_body(0x4d);
        Self {
            server: Modem::new(Settings { marker, round_trip: 2.0 * DELAY as f64 / 8000.0 }),
            client: Client::new(),
            to_server: vec![0.0; DELAY].into(),
            to_client: vec![0.0; DELAY].into(),
            now: 0,
        }
    }

    fn run(&mut self, seconds: f64, until: impl Fn(&Self) -> bool) {
        for _ in 0..(seconds * 8000.0) as usize {
            let s = self.server.step(self.to_server.pop_front().unwrap());
            let c = self.client.step(self.to_client.pop_front().unwrap());
            self.to_client.push_back(s);
            self.to_server.push_back(c);
            self.now += 1;
            if until(self) {
                return;
            }
            if let Status::Failed(why) = self.server.status() {
                panic!(
                    "the server failed at {:.2} s, {}: {why}; the client read {} records, {} bad frames",
                    self.now as f64 / 8000.0,
                    self.server.phase(),
                    self.client.records_heard,
                    self.client.bad_frames
                );
            }
        }
    }

    /// Data both ways: what each end sends the other reads.
    fn carries_data(&mut self, n: usize) {
        let down: Vec<bool> = (0..n).map(|k| (k * 7919) % 11 < 5).collect();
        let up: Vec<bool> = (0..n).map(|k| (k * 104_729) % 13 < 6).collect();
        self.server.take_bits();
        self.client.take_bits();
        self.server.send_bits(&down);
        self.client.send_bits(&up);
        let (mut got_down, mut got_up) = (Vec::new(), Vec::new());
        for _ in 0..(4.0 * 8000.0) as usize {
            self.run(0.000_125, |_| false);
            got_down.extend(self.client.take_bits());
            got_up.extend(self.server.take_bits());
        }
        // Idle is ones: the data is in there, after whatever ones led up to it.
        let find = |got: &[bool], sent: &[bool]| got.windows(sent.len()).any(|w| w == sent);
        assert!(find(&got_down, &down), "downstream data did not arrive ({} bits read)", got_down.len());
        assert!(find(&got_up, &up), "upstream data did not arrive ({} bits read)", got_up.len());
    }
}

#[test]
fn an_x2_call_reaches_data_at_32000_and_carries_it_both_ways() {
    let mut call = Call::new();
    call.run(25.0, |c| matches!(c.server.status(), Status::Connected { .. }) && c.client.in_data());
    assert_eq!(call.server.status(), Status::Connected { downstream: 32_000, upstream: 28_800 }, "{}", call.server.phase());
    assert!(call.client.in_data());
    assert_eq!(call.client.bad_frames, 0);
    call.carries_data(4000);
}

#[test]
fn the_clients_rate_change_is_followed_to_the_rate_it_asks_for() {
    let mut call = Call::new();
    call.run(25.0, |c| matches!(c.server.status(), Status::Connected { .. }) && c.client.in_data());
    call.run(1.0, |_| false);
    call.client.change_rate(14);
    call.run(8.0, |c| c.server.rate_changes() == 1 && matches!(c.server.status(), Status::Connected { .. }) && c.client.in_data());
    assert_eq!(call.server.status(), Status::Connected { downstream: 54_666, upstream: 28_800 }, "{}", call.server.phase());
    assert_eq!(call.client.bad_frames, 0);
    call.carries_data(4000);
}

/// The upstream is ones from E on, while nothing is sent: B1 and idle.
///
/// The client's upstream is non-linearly encoded, as a Courier's is, and the
/// receiver's loops were deciding its points against the straight grid: the
/// outer points of the shaped signal landed between grid points, and after
/// a third of a second of clean data some 40 % of the bits were wrong for a
/// quarter of a second until the loops came back.
#[test]
fn the_upstream_has_no_errors_from_e_on() {
    let mut call = Call::new();
    call.run(25.0, |c| matches!(c.server.status(), Status::Connected { .. }) && c.client.in_data());
    let mut bits = call.server.take_bits();
    call.run(1.0, |_| false);
    bits.extend(call.server.take_bits());
    assert!(bits.len() > 28_000, "{} bits", bits.len());
    assert_eq!(bits.iter().filter(|&&b| !b).count(), 0, "zeros in {} bits of idle", bits.len());
}
