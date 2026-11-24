//! A V.34 call between two of these, each through its own `sip::Media`, with
//! an international route's worth of trouble on the way back to the caller:
//! jitter, bursts of loss, and stalls of 100 to 300 ms.
//!
//! What it is for. On live calls to NetZero our upstream -- V.34 under V.90,
//! PCM under V.92 -- was rejected in bursts, yet it decoded clean as it left
//! the modem. The difference was the SIP sender: every inbound stall longer
//! than its 60 ms cushion stopped the modem (which was clocked by the
//! arrivals) and put whole packets of silence into what it was sending. Now
//! the modem is stepped on a local clock and the stall goes into what it
//! hears instead. This test holds the sender to that: every octet of RTP the
//! caller sent after its modem began is checked against what its modem said,
//! put through the same rate conversion and law, and the host's receiver has
//! to decode the upstream text that rode on it.
//!
//! Two modems in real time want an optimised build, as `sip_pair` does:
//!
//! ```text
//! cargo test --release -p modem --test sip_jitter
//! ```

use std::collections::VecDeque;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use dsp::Resampler;
use modem::{Modem, State};
use sip::g711::{self, Law};
use sip::media::{Media, Stats};

const FS: f64 = 16_000.0;
/// As `modem-sip` drives the line.
const DRIVE: f32 = 0.1;
/// How long the caller types for once error control is up.
const TYPING: Duration = Duration::from_secs(10);

fn local(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}

/// What one end's modem loop leaves behind.
#[derive(Default)]
struct End {
    terminal: Vec<u8>,
    /// The G.711 codewords its modem's output should have become: what was
    /// handed to `Media`, put through a rate conversion and an encoder of our
    /// own that started in step with the media path's.
    said: Vec<u8>,
    stats: Option<Stats>,
}

/// A modem stepped the way `modem-sip` steps one, until `stop`.
fn run_modem(media: Media, commands: &'static str, typed: Arc<Mutex<VecDeque<u8>>>, connected: Arc<AtomicBool>, stop: Arc<AtomicBool>) -> End {
    let mut modem = Modem::new(FS);
    for b in commands.bytes() {
        modem.feed_dte(b);
    }
    modem.feed_dte(b'\r');
    let mut end = End::default();
    let mut down = Resampler::new(FS, 8000.0);
    let (mut from_line, mut from_network, mut to_line, mut scratch, mut narrow) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    while !stop.load(Ordering::Relaxed) {
        if let Ok(mut typed) = typed.lock() {
            for b in typed.drain(..) {
                modem.feed_dte(b);
            }
        }
        from_line.clear();
        from_network.clear();
        media.receive_both(&mut from_line, Some(&mut from_network));
        if from_line.is_empty() {
            thread::sleep(Duration::from_millis(2));
        } else {
            let levels: Vec<f64> = from_network.iter().map(|l| f64::from(*l)).collect();
            modem.set_network_a_law(false);
            modem.give_network_levels(&levels);
            to_line.clear();
            for &s in &from_line {
                let said = modem.step(f64::from(s)) as f32;
                let drive = if modem.exact_levels() { 1.0 } else { DRIVE };
                to_line.push(said * drive);
            }
            match modem.take_network_levels() {
                Some(levels) => {
                    let levels: Vec<f32> = levels.iter().map(|l| *l as f32).collect();
                    media.transmit_exact(&levels);
                    g711::encode_into(Law::Mu, &levels, &mut end.said);
                }
                None => {
                    media.transmit(&to_line);
                    narrow.clear();
                    for &s in &to_line {
                        scratch.clear();
                        down.process(f64::from(s), &mut scratch);
                        narrow.extend(scratch.iter().map(|s| *s as f32));
                    }
                    g711::encode_into(Law::Mu, &narrow, &mut end.said);
                }
            }
        }
        end.terminal.extend(modem.take_dte());
        if modem.state() == State::Data {
            connected.store(true, Ordering::Relaxed);
        }
    }
    end.stats = Some(media.stats());
    end
}

/// A packet from the host on its way to the caller, and when it may go.
struct Held {
    at: Instant,
    datagram: Vec<u8>,
}

#[test]
#[cfg_attr(debug_assertions, ignore = "two modems in real time: run it with --release")]
fn a_v34_upstream_reaches_the_wire_intact_through_inbound_jitter_loss_and_stalls() {
    // The route: one socket facing each end.
    let facing_caller = UdpSocket::bind("127.0.0.1:0").unwrap();
    let facing_host = UdpSocket::bind("127.0.0.1:0").unwrap();
    facing_caller.set_read_timeout(Some(Duration::from_millis(1))).unwrap();
    facing_host.set_read_timeout(Some(Duration::from_millis(1))).unwrap();

    let caller_media = Media::open(0, FS).unwrap();
    let host_media = Media::open(0, FS).unwrap();
    let (caller_port, host_port) = (caller_media.local_port(), host_media.local_port());
    caller_media.connect(facing_caller.local_addr().unwrap(), Law::Mu, g711::PCMU, 20);
    host_media.connect(facing_host.local_addr().unwrap(), Law::Mu, g711::PCMU, 20);

    let stop = Arc::new(AtomicBool::new(false));
    // Set once both ends are online: the heavy trouble starts then, so that
    // what is measured is the data, not whether training survives it.
    let impair = Arc::new(AtomicBool::new(false));
    let start = Instant::now();

    // Caller to host: untouched, and every packet kept.
    let upstream = {
        let (from, to, stop) = (facing_caller.try_clone().unwrap(), facing_host.try_clone().unwrap(), Arc::clone(&stop));
        thread::spawn(move || {
            let mut packets = Vec::new();
            let mut buffer = [0u8; 2048];
            while !stop.load(Ordering::Relaxed) {
                if let Ok(n) = from.recv(&mut buffer) {
                    let _ = to.send_to(&buffer[..n], local(host_port));
                    if let Some(packet) = sip::rtp::Packet::parse(&buffer[..n]) {
                        packets.push(packet);
                    }
                }
            }
            packets
        })
    };

    // Host to caller: 0-30 ms of jitter always; once online, a stall of
    // 100-300 ms every 1.5 s and a burst of two or three lost packets every
    // 2.5 s. A stall holds everything that arrives in it and lets it go in
    // one burst at its end, which is what an inbound route does.
    let downstream = {
        let (from, to, stop, impair) = (facing_host.try_clone().unwrap(), facing_caller.try_clone().unwrap(), Arc::clone(&stop), Arc::clone(&impair));
        thread::spawn(move || {
            let mut held: VecDeque<Held> = VecDeque::new();
            let mut buffer = [0u8; 2048];
            let mut state = 12_345u32;
            let mut random = move || {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                state >> 8
            };
            let mut since: Option<Instant> = None;
            let (mut stalls, mut losses) = (0u32, 0u32);
            let mut stall_until = start;
            let mut dropping = 0;
            let mut last_release = start;
            while !stop.load(Ordering::Relaxed) {
                let now = Instant::now();
                if let Ok(n) = from.recv(&mut buffer) {
                    let mut at = now + Duration::from_millis(u64::from(random() % 31));
                    if impair.load(Ordering::Relaxed) {
                        let since = *since.get_or_insert(now);
                        let t = now.duration_since(since);
                        if t >= Duration::from_millis(1500) * (stalls + 1) {
                            stalls += 1;
                            stall_until = now + Duration::from_millis(100 + u64::from(random() % 201));
                        }
                        if t >= Duration::from_millis(2500) * (losses + 1) {
                            losses += 1;
                            dropping = 2 + random() % 2;
                        }
                        at = at.max(stall_until);
                    }
                    if dropping > 0 {
                        dropping -= 1;
                    } else {
                        // In order: a packet never overtakes the one before.
                        at = at.max(last_release);
                        last_release = at;
                        held.push_back(Held { at, datagram: buffer[..n].to_vec() });
                    }
                }
                while held.front().is_some_and(|h| h.at <= now) {
                    let h = held.pop_front().unwrap();
                    let _ = to.send_to(&h.datagram, local(caller_port));
                }
            }
            (stalls, losses)
        })
    };

    let caller_typed = Arc::new(Mutex::new(VecDeque::new()));
    let (caller_online, host_online) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
    let host = {
        let (stop, online) = (Arc::clone(&stop), Arc::clone(&host_online));
        thread::spawn(move || run_modem(host_media, "AT+MS=V34;A", Arc::new(Mutex::new(VecDeque::new())), online, stop))
    };
    let caller = {
        let (stop, online, typed) = (Arc::clone(&stop), Arc::clone(&caller_online), Arc::clone(&caller_typed));
        thread::spawn(move || run_modem(caller_media, "AT+MS=V34;D5551234", typed, online, stop))
    };

    // Wait for both ends, let error control settle, then type for a while
    // with the trouble on.
    let deadline = Instant::now() + Duration::from_secs(30);
    while !(caller_online.load(Ordering::Relaxed) && host_online.load(Ordering::Relaxed)) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(50));
    }
    let online = caller_online.load(Ordering::Relaxed) && host_online.load(Ordering::Relaxed);
    let mut typed = Vec::new();
    if online {
        thread::sleep(Duration::from_secs(2));
        impair.store(true, Ordering::Relaxed);
        let typing = Instant::now();
        let mut line = 0;
        while typing.elapsed() < TYPING {
            let text = format!("line {line:04}: the quick brown fox jumps over the lazy dog\r\n");
            typed.extend_from_slice(text.as_bytes());
            caller_typed.lock().unwrap().extend(text.bytes());
            line += 1;
            thread::sleep(Duration::from_millis(100));
        }
        // Time for the last of it to cross, retransmissions and all.
        thread::sleep(Duration::from_secs(4));
    }
    stop.store(true, Ordering::Relaxed);
    let (caller, host) = (caller.join().unwrap(), host.join().unwrap());
    let sent = upstream.join().unwrap();
    let (stalls, losses) = downstream.join().unwrap();
    let stats = caller.stats.clone().unwrap();
    let saw = |end: &End| String::from_utf8_lossy(&end.terminal).into_owned();
    assert!(online, "the call never came up\ncaller: {:?}\nhost: {:?}", saw(&caller), saw(&host));
    assert!(stalls >= 6 && losses >= 4, "the route was not impaired: {stalls} stalls, {losses} losses");

    // The caller heard the trouble: stalls became silence in what it heard.
    assert!(stats.jitter.starved > 0 && stats.jitter.late > 0, "no stall reached the caller: {stats:?}");

    // Every packet the caller sent, numbered and stamped without a break.
    for pair in sent.windows(2) {
        assert_eq!(pair[1].sequence, pair[0].sequence.wrapping_add(1), "the route lost an upstream packet");
        assert_eq!(pair[1].timestamp.wrapping_sub(pair[0].timestamp), 160);
    }
    // And what was in them, against what the caller's modem said.
    let wire: Vec<u8> = sent.iter().flat_map(|p| p.payload.iter().copied()).collect();
    let quiet = Law::Mu.encode(0);
    let said = &caller.said;
    let in_said = said.iter().position(|&c| c != quiet).expect("the caller's modem said nothing");
    let on_wire = wire.iter().position(|&c| c != quiet).expect("nothing but silence went out");
    assert!(on_wire >= in_said, "the wire led the modem");
    let offset = on_wire - in_said;
    let wire = &wire[offset..];
    assert!(wire.len() + 2 * 160 >= said.len(), "{} of the {} octets the modem said went out", wire.len(), said.len());
    let n = wire.len().min(said.len());
    if let Some(at) = (0..n).find(|&i| wire[i] != said[i]) {
        panic!(
            "the upstream parted from what the modem said {:.3} s into it, with {} octets of silence there\n{stats:?}",
            at as f64 / 8000.0,
            wire[at..].iter().take_while(|&&o| o == quiet).count()
        );
    }
    assert_eq!((stats.underruns_mid_call, stats.inserted, stats.dropped_out), (0, 0, 0), "{stats:?}");

    // And the host's receiver decoded the upstream that rode on it.
    let heard = saw(&host);
    let typed = String::from_utf8_lossy(&typed).into_owned();
    assert!(heard.contains(&typed), "the host did not get what the caller typed\ncaller: {:?}\nhost: {heard:?}", saw(&caller));
    eprintln!(
        "{:.1} s, {} upstream octets checked, {stalls} stalls and {losses} losses inbound; caller: {stats:?}",
        start.elapsed().as_secs_f64(),
        n
    );
}
