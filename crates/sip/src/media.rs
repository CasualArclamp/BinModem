//! The audio path: an RTP socket at one end, the modem's sample rate at the
//! other, and as little as possible in between.
//!
//! Two threads and two queues. A reader thread does nothing but pull datagrams
//! off the socket and put their payloads into the jitter buffer, because a
//! thread that is doing anything else is a thread that is not reading the
//! socket, and the operating system's receive buffer is the only thing
//! standing between a scheduling hiccup and a lost packet. A pacer thread
//! sends each packet of what the modem says as soon as it is complete, and
//! silence only when the modem has nothing to say.
//!
//! The pacer is the part worth arguing about. The alternative -- send a packet
//! whenever one arrives, so the far end's clock is ours -- is tempting and is
//! what a gateway effectively does. It is rejected here because it makes our
//! transmission stop if the far end's ever does, and a far end that applies
//! silence suppression does exactly that.
//!
//! # One clock, and it is ours
//!
//! The modem produces one sample for every sample it is handed, so whatever
//! clocks what it hears clocks what it says. This used to be the far end's
//! arrivals: `receive` handed over whatever had come in, the modem was stepped
//! for each sample, and the pacer sent on a timer of its own. Two clocks, and
//! between them a 60 ms cushion of silence. Every inbound stall longer than
//! the cushion -- and an international route has them by the dozen, 100 to
//! 300 ms at a time -- stopped the modem, emptied the outgoing queue and put
//! whole packets of silence into the middle of what the modem was sending.
//! Each one moves everything after it by 20 ms, which to the far end's data
//! receiver is a burst of junk symbols and a lost frame: LAPM SREJs in
//! bursts, and a V.92 server retraining the moment our data arrives.
//!
//! So the modem is now stepped on a clock of its own: `receive` hands over
//! exactly as many samples as a steady local clock says are due since the
//! stream began, whether they have arrived or not (see `jitter::Jitter::play`
//! and `Playout`). A packet that is late is concealed in what the modem hears
//! and dropped when it turns up; what the modem says flows on unbroken, and
//! the pacer sends each packet of it the moment it is complete. There is no
//! cushion, nothing is ever spliced into the outgoing stream while the modem
//! is talking, and nothing it said is thrown away. The hole goes in what we
//! hear, because our receiver is ours to make robust and the far end's is
//! not -- and a hole that keeps the timeline's length is what a real line
//! does when it drops out.
//!
//! The local clock is slewed by at most [`MAX_SLEW_PPM`] to keep the jitter
//! buffer at its depth, so over a long call it runs at the far end's rate and
//! neither sheds nor starves. No audio is resampled for it: the modem simply
//! hears the far end's samples a few parts per million faster or slower, and
//! talks at that rate.
//!
//! What is deliberately absent: no gain control, no voice activity detection,
//! no comfort noise, no packet loss concealment beyond silence, and no
//! adaptive anything. Every one of those improves a telephone call and damages
//! this one. Anything arriving that is not the audio we negotiated -- an RFC
//! 4733 telephone-event, comfort noise, a codec the far end switched to on its
//! own -- is dropped here rather than decoded, and counted in `off_codec`:
//! this is the only layer that knows what was agreed, and a four-octet event
//! packet decoded as G.711 is 20 ms of the timeline gone.

use std::collections::VecDeque;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use dsp::Resampler;

use crate::g711::{self, Law};
use crate::jitter::{Counters, Jitter};
use crate::rtp;

/// What the network side runs at, always. G.711 is 8000 samples a second and
/// there is no other option in this crate.
pub const NETWORK_RATE: f64 = 8000.0;

/// The rate of a linear call's samples unless the call says otherwise: L16
/// at 16 kHz, the first linear coding this agent had. What a call agreed
/// to is [`Media::linear_rate`] (see [`crate::sdp::LINEAR_FORMATS`]).
pub const LINEAR_RATE: f64 = 16_000.0;

/// Levels to L16: sixteen bits, most significant octet first, and the same
/// sample on every one of `channels`. A call with two channels carries one
/// signal twice; see [`Media::connect_linear`].
fn encode_linear(levels: &[f32], channels: usize, out: &mut Vec<u8>) {
    for &level in levels {
        let sample = (f64::from(level) * 32768.0).round().clamp(-32768.0, 32767.0) as i16;
        for _ in 0..channels.max(1) {
            out.extend_from_slice(&sample.to_be_bytes());
        }
    }
}

/// How much audio may queue up waiting to go out before the oldest is thrown
/// away, in packets.
///
/// The modem and the pacer run on one clock, so the queue holds at most a
/// packet or two plus whatever a late turn of the modem loop delivered at
/// once. This is only a bound for a pacer thread that has stopped being
/// scheduled at all: two seconds, and every octet past it is counted in
/// `dropped_out`.
const OUTGOING_LIMIT: usize = 100;

/// How long the modem must have said nothing before the pacer sends silence
/// in its place.
///
/// The modem is stepped on the play-out clock and produces a sample for every
/// one it hears, so a gap in what it says is the loop driving it not running
/// -- a hold, the end of a call, or a machine that stopped scheduling it.
/// Sending silence into that is a splice, so it is put off until the gap is
/// past anything a loop that is merely late produces: a second. Until then a
/// late packet goes late, which the far end's jitter buffer is for. Every
/// octet sent past this is counted in `inserted`.
const DRY_LIMIT: Duration = Duration::from_secs(1);

/// Packets of delay the jitter buffer holds before it hands anything over,
/// unless `BINMODEM_SIP_JITTER` asks for another number (1 to 25).
///
/// Fifteen, which is 300 ms. An inbound stall shorter than this costs nothing
/// at all; a longer one costs the modem's input that much less silence. It
/// used to be two, with a 60 ms cushion on the outgoing side, then five once
/// the cushion went and the modem and the pacer shared a clock. Five was
/// still too few on an international route: live V.90 calls to a US server
/// from Australia (round trip about 0.9 s) lost 20 ms holes in the middle of
/// data mode, packets that arrived too late rather than not at all, and each
/// one is a thousand bits of PCM gone. At fifteen those calls were clearly
/// better. A modem minds 200 ms more delay far less than a hole: the
/// handshake measures the round trip whatever it is. A telephone would
/// stretch the audio to manage jitter; that is wrong for a modem.
const JITTER_TARGET: usize = 15;
/// And how many it will hold before shedding, which is where a far end whose
/// clock runs fast ends up if the slew cannot keep up.
const JITTER_CAPACITY: usize = 50;

/// How hard the play-out clock leans on the jitter buffer's depth: this many
/// parts per million for each packet it stands away from its target.
const SLEW_PPM_PER_PACKET: f64 = 100.0;
/// The most the play-out clock is ever slewed from 8000 samples a second.
///
/// A hundred parts per million, which is V.34's own tolerance on its symbol
/// rate: what the modem sends is clocked by this, so the far end's receiver
/// must never see it further off than a real modem may be. Telephone clocks
/// agree to within tens of parts per million, so this is a bound, not a
/// working range.
pub const MAX_SLEW_PPM: f64 = 100.0;
/// Seconds over which the jitter buffer's depth is averaged before the clock
/// is slewed by it, so that packet arrivals, stalls and bursts do not move it.
const DEPTH_SMOOTHING: f64 = 5.0;
/// The least the play-out clock hands over at once, in seconds: 2 ms, so a
/// loop that asks very often is not handed one sample at a time.
const LEAST_HANDED: f64 = 0.002;
/// The most it hands over at once, in seconds, when a loop has fallen behind
/// and is catching up; the rest comes on the next turns.
const MOST_HANDED: f64 = 0.4;
/// How far behind the play-out clock the loop may fall before the clock gives
/// up on the difference rather than handing it over: a second. The jitter
/// buffer sheds past its capacity anyway, so what is skipped is time in
/// which nothing could have been heard. Counted in `fell_behind`.
const MOST_BEHIND: f64 = 1.0;

/// The jitter target in force.
fn jitter_target() -> usize {
    std::env::var("BINMODEM_SIP_JITTER")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|n| (1..=25).contains(n))
        .unwrap_or(JITTER_TARGET)
}

/// The modem's clock: a steady local count of samples due since the far
/// end's stream was first ready, slewed to keep the jitter buffer at its
/// depth. Only the thread calling `receive` touches it.
#[derive(Debug, Default)]
struct Playout {
    /// When the clock last moved; None until the stream is ready.
    last: Option<Instant>,
    /// Octets due and not yet handed over.
    due: f64,
    /// The jitter buffer's depth in octets, smoothed.
    depth: f64,
    /// The slew in force.
    ppm: f64,
}

/// What the media path has been doing.
#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub connected: bool,
    pub remote: Option<String>,
    pub law: Option<&'static str>,
    pub packets_sent: u64,
    pub packets_received: u64,
    /// Datagrams that arrived and were not RTP we could use.
    pub ignored: u64,
    /// RTP packets that parsed but carried a payload type we did not
    /// negotiate, and were dropped rather than decoded.
    ///
    /// Three things do this and all three are ordinary. An RFC 4733
    /// telephone-event, which is how a far end sends a dialled digit: four
    /// octets, and one digit is a burst of ten to twenty of them. Comfort
    /// noise, payload type 13: one octet. And a transcoder that has changed
    /// codec under the call. Every one of them used to be decoded as G.711
    /// and handed to the modem, so four octets went in where a hundred and
    /// sixty belonged -- a 20 ms deletion of the audio timeline, uncounted,
    /// which is precisely the phase step this crate exists to remove.
    ///
    /// The timeline still keeps its length, because the sequence number the
    /// event used is now a gap the jitter buffer conceals; so a dialled digit
    /// shows up here and in `concealed` together, and that pair is how to
    /// read it. A count that climbs steadily with no audio behind it is a far
    /// end that has switched codec, and that call is over whatever we do.
    pub off_codec: u64,
    /// Packets we sent as silence because the modem had not produced anything.
    /// The number that says whether a call failed at our end.
    pub underruns: u64,
    /// The underruns that cut into something: the ones that happened after the
    /// modem had already put audio on this call.
    ///
    /// The rest are the start of the call, before the far end's first packet
    /// has arrived and been through the jitter buffer -- the modem's clock
    /// starts with the far end's stream, so until then it has not been
    /// stepped and has nothing to say, and the silence we send is not a hole
    /// in anything. Since the modem and the pacer share a clock, an inbound
    /// stall no longer lands here at all: only a modem that stopped for
    /// [`DRY_LIMIT`] does. Counting the
    /// two together made a healthy call read as a failing one and buried the
    /// number that matters: 38 gaps sent looks the same either way, and only
    /// this one of them says the handshake was being cut up while it ran.
    ///
    /// A hold counts here, for what it is worth: `line::Line` stops handing
    /// the modem's samples over while the far end says it is not listening,
    /// and from down here that is a modem that has stopped producing.
    pub underruns_mid_call: u64,
    /// Octets of silence sent into the middle of what the modem was saying,
    /// because it had said nothing for [`DRY_LIMIT`]: a hold, the end of a
    /// call, or a modem loop that stopped running. Each is a splice in the
    /// outgoing stream. Zero on a call where the loop kept up, whatever the
    /// inbound side did.
    pub inserted: u64,
    /// Packets sent while another whole packet was already waiting: the modem
    /// loop ran late and delivered more than a packet at once, so these went
    /// late and close together. No audio is lost or added; the far end sees
    /// jitter.
    pub sent_behind: u64,
    /// Octets of play-out time skipped because the modem loop fell more than
    /// [`MOST_BEHIND`] behind its clock.
    pub fell_behind: u64,
    /// How far the play-out clock is slewed from 8000 samples a second, in
    /// parts per million, to keep the jitter buffer at its depth. Positive
    /// means the far end's clock runs fast of ours.
    pub clock_ppm: f64,
    /// Octets dropped from the outgoing queue because nothing was draining it
    /// fast enough.
    pub dropped_out: u64,
    /// Octets taken out of the outgoing queue because a call started or ended,
    /// rather than because the queue overflowed: audio the modem produced that
    /// never left this machine.
    ///
    /// At a disconnect it is the tail of the call and means nothing. During a
    /// call it means the media path was started again underneath it -- a
    /// re-INVITE, or a 200 OK that arrived twice -- and it used to happen in
    /// silence, which is the same lie as audio inserted in silence: the call
    /// had a discontinuity in it and every counter said it was clean.
    pub flushed_out: u64,
    /// Times the far end's RTP came from somewhere other than the address it
    /// gave us, and we followed it.
    pub latched: u64,
    /// Packets from an address other than the call's that were not the
    /// call's stream moving there, and so were dropped rather than followed.
    ///
    /// Anything that can reach the port can send RTP to it, and following
    /// every new source let one packet from anywhere take the call's audio
    /// over in both directions: its payload went to the modem, and the
    /// modem's went back to it. A count here on an ordinary call is somebody
    /// else's stream, most often the last call's still arriving.
    pub strangers: u64,
    pub jitter: Counters,
}

/// The stream the call's audio is arriving on, once one has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Source {
    address: SocketAddr,
    ssrc: u32,
    /// The newest sequence number heard on it.
    sequence: u16,
}

/// How far ahead of the stream's newest packet one arriving from a new
/// address may be and still be the same stream moving there: two seconds of
/// 20 ms packets, which a router rebinding a port does not take.
const FOLLOW_WITHIN: u16 = 100;

impl Source {
    /// Whether a packet from another address is this stream moving there.
    ///
    /// RFC 3550 8.2 has a receiver "avoid switching" on a packet that merely
    /// claims to be a known source, and A.1's continuity check is what tells
    /// the stream from a stranger: the same SSRC, carrying on from where the
    /// stream had got to. Behind a router that rebinds, this is exactly what
    /// arrives; from anywhere else, guessing both at once is not.
    fn moved_to(&self, ssrc: u32, sequence: u16) -> bool {
        let ahead = sequence.wrapping_sub(self.sequence);
        ssrc == self.ssrc && (1..=FOLLOW_WITHIN).contains(&ahead)
    }
}

#[derive(Debug)]
struct Shared {
    /// Where to send. None until the call is answered.
    remote: Mutex<Option<SocketAddr>>,
    /// The address [`Media::connect`] was last given, which is not always
    /// where the audio is going: symmetric RTP follows the source. Kept apart
    /// from `remote` so that a second connect can be recognised as the same
    /// call -- comparing against `remote` would call a call that had latched
    /// to a far end's real address a different call, and throw its buffer
    /// away for it.
    dialled: Mutex<Option<SocketAddr>>,
    /// The stream the call's audio has been arriving on. None until the first
    /// packet of a call, which is followed wherever it came from (symmetric
    /// RTP); after that, a packet from anywhere else has to be that stream
    /// moving (see [`Source::moved_to`]).
    source: Mutex<Option<Source>>,
    /// Codewords waiting to go out, already companded.
    outgoing: Mutex<VecDeque<u8>>,
    /// Codewords that have arrived, in order, waiting to be read.
    incoming: Mutex<Jitter>,
    /// The law in force, as a payload type so it can live in an atomic.
    payload_type: AtomicU8,
    /// How many codewords go in one packet.
    packet_octets: AtomicU64,
    running: AtomicBool,
    /// The call is L16 and not G.711: two octets to a sample, most
    /// significant first, and no law.
    linear: AtomicBool,
    /// A linear call's samples a second, and its channels.
    linear_rate: AtomicU64,
    linear_channels: AtomicU8,
    connected: AtomicBool,
    /// Whether the modem has put anything on this call yet, which is what
    /// tells an underrun that cut into a handshake from one sent while the
    /// line was still waiting for its first packet. Cleared by `connect` and
    /// `disconnect`, because it is a fact about the call and not about the
    /// line.
    spoke: AtomicBool,
    packets_sent: AtomicU64,
    packets_received: AtomicU64,
    ignored: AtomicU64,
    off_codec: AtomicU64,
    underruns: AtomicU64,
    underruns_mid_call: AtomicU64,
    inserted: AtomicU64,
    sent_behind: AtomicU64,
    fell_behind: AtomicU64,
    /// [`Stats::clock_ppm`], as an `f64`'s bits.
    clock_ppm: AtomicU64,
    /// Packets the jitter buffer holds, which the play-out clock steers to.
    jitter_target: usize,
    dropped_out: AtomicU64,
    flushed_out: AtomicU64,
    latched: AtomicU64,
    strangers: AtomicU64,
}

/// Rate conversion, which only the thread calling `receive` and `transmit`
/// ever touches.
#[derive(Debug)]
struct Conversion {
    up: Resampler,
    down: Resampler,
    /// The same two for a linear call, whose network rate is 16 kHz, and
    /// whether they are needed at all: a modem that runs at 16 kHz is handed
    /// the samples as they are.
    linear_up: Resampler,
    linear_down: Resampler,
    same_rate: bool,
    /// The modem's own rate, to make those two again for a call at another
    /// linear rate.
    modem_rate: f64,
    codewords: Vec<u8>,
    at_network_rate: Vec<f32>,
    scratch: Vec<f64>,
    playout: Playout,
}

/// The media path for one call.
#[derive(Debug)]
pub struct Media {
    shared: Arc<Shared>,
    conversion: Mutex<Conversion>,
    socket: UdpSocket,
    local_port: u16,
    threads: Vec<JoinHandle<()>>,
}

impl Media {
    /// Bind the RTP port and start both threads. Nothing is sent until
    /// [`Media::connect`] is called, because until the call is answered there
    /// is nowhere to send it.
    ///
    /// `port` of zero lets the system choose, which is normal; the chosen one
    /// is what goes into the SDP offer.
    pub fn open(port: u16, modem_rate: f64) -> Result<Self, String> {
        Self::open_with_jitter(port, modem_rate, jitter_target())
    }

    /// The same with the jitter buffer's depth, in packets, given rather
    /// than taken from the environment: for the tests, which are about
    /// packets and stalls, not about the default.
    fn open_with_jitter(port: u16, modem_rate: f64, target: usize) -> Result<Self, String> {
        let socket = UdpSocket::bind(("0.0.0.0", port))
            .map_err(|e| format!("could not bind the RTP port: {e}"))?;
        socket
            .set_read_timeout(Some(Duration::from_millis(20)))
            .map_err(|e| format!("could not set a read timeout: {e}"))?;
        let local_port = socket
            .local_addr()
            .map_err(|e| e.to_string())?
            .port();

        let shared = Arc::new(Shared {
            remote: Mutex::new(None),
            dialled: Mutex::new(None),
            source: Mutex::new(None),
            outgoing: Mutex::new(VecDeque::new()),
            incoming: Mutex::new(Jitter::new(target, JITTER_CAPACITY)),
            payload_type: AtomicU8::new(g711::PCMU),
            packet_octets: AtomicU64::new(160),
            running: AtomicBool::new(true),
            linear: AtomicBool::new(false),
            linear_rate: AtomicU64::new(LINEAR_RATE as u64),
            linear_channels: AtomicU8::new(1),
            connected: AtomicBool::new(false),
            spoke: AtomicBool::new(false),
            packets_sent: AtomicU64::new(0),
            packets_received: AtomicU64::new(0),
            ignored: AtomicU64::new(0),
            off_codec: AtomicU64::new(0),
            underruns: AtomicU64::new(0),
            underruns_mid_call: AtomicU64::new(0),
            inserted: AtomicU64::new(0),
            sent_behind: AtomicU64::new(0),
            fell_behind: AtomicU64::new(0),
            clock_ppm: AtomicU64::new(0),
            jitter_target: target,
            dropped_out: AtomicU64::new(0),
            flushed_out: AtomicU64::new(0),
            latched: AtomicU64::new(0),
            strangers: AtomicU64::new(0),
        });

        let reader = {
            let shared = Arc::clone(&shared);
            let socket = socket.try_clone().map_err(|e| e.to_string())?;
            thread::spawn(move || read(shared, socket))
        };
        let pacer = {
            let shared = Arc::clone(&shared);
            let socket = socket.try_clone().map_err(|e| e.to_string())?;
            thread::spawn(move || pace(shared, socket))
        };

        Ok(Self {
            shared,
            conversion: Mutex::new(Conversion {
                up: Resampler::new(NETWORK_RATE, modem_rate),
                down: Resampler::new(modem_rate, NETWORK_RATE),
                linear_up: Resampler::new(LINEAR_RATE, modem_rate),
                linear_down: Resampler::new(modem_rate, LINEAR_RATE),
                same_rate: (modem_rate - LINEAR_RATE).abs() < 0.5,
                modem_rate,
                codewords: Vec::with_capacity(2048),
                at_network_rate: Vec::with_capacity(2048),
                scratch: Vec::with_capacity(64),
                playout: Playout::default(),
            }),
            socket,
            local_port,
            threads: vec![reader, pacer],
        })
    }

    /// The port that went into the offer.
    pub fn local_port(&self) -> u16 {
        self.local_port
    }

    /// The call is up: start sending here, in this law.
    ///
    /// Calling this a second time with the same call is deliberately nothing
    /// at all. It used to restart the jitter buffer and empty the outgoing
    /// queue every time -- up to fifty packets in and twenty-five out, thrown
    /// away without a word -- and the path that reaches it is not rare: a 200
    /// OK that arrives twice, or any re-INVITE, sends `line::Line::poll` back
    /// through here in the middle of a call that nothing was wrong with. On
    /// this rig that is the twenty-millisecond discontinuity that costs a
    /// V.34 receiver its training, arriving at the one moment every counter
    /// says the line is clean.
    pub fn connect(&self, remote: SocketAddr, law: Law, payload_type: u8, ptime_ms: u32) {
        let octets = ((NETWORK_RATE * f64::from(ptime_ms) / 1000.0).round() as u64).max(80);
        // The law is remembered through the payload type, which is what
        // arrives on every packet anyway; this keeps one source of truth
        // rather than two that can disagree mid-call.
        let _ = law;
        self.connect_as(remote, payload_type, octets, false);
    }

    /// [`Self::connect`] for a call that agreed on L16 at `rate` on
    /// `channels` channels: linear samples of two octets, which go and come
    /// exactly as they are.
    ///
    /// Two channels carry one signal. What is sent is the same sample on
    /// both, and what is heard is their mean, which for a far end doing the
    /// same is the sample it sent, to the bit.
    ///
    /// A packet is the packet time whatever the rate, as it is for every
    /// softphone that has these codings, so at 44.1 and 48 kHz it is more
    /// octets than an Ethernet frame holds and the network sends it in
    /// pieces.
    pub fn connect_linear(&self, remote: SocketAddr, payload_type: u8, ptime_ms: u32, rate: u32, channels: u8) {
        let channels = channels.clamp(1, 2);
        let rate = rate.max(8000);
        let samples = ((f64::from(rate) * f64::from(ptime_ms) / 1000.0).round() as u64).max(80);
        let was = (self.shared.linear_rate.swap(u64::from(rate), Ordering::Relaxed), self.linear());
        self.shared.linear_channels.store(channels, Ordering::Relaxed);
        if was != (u64::from(rate), true)
            && let Ok(mut conversion) = self.conversion.lock()
        {
            let modem_rate = conversion.modem_rate;
            conversion.linear_up = Resampler::new(f64::from(rate), modem_rate);
            conversion.linear_down = Resampler::new(modem_rate, f64::from(rate));
            conversion.same_rate = (modem_rate - f64::from(rate)).abs() < 0.5;
        }
        self.connect_as(remote, payload_type, samples * 2 * u64::from(channels), true);
    }

    /// Whether the call is linear rather than G.711.
    pub fn linear(&self) -> bool {
        self.shared.linear.load(Ordering::Relaxed)
    }

    /// Samples a second of a linear call, on each of its channels.
    pub fn linear_rate(&self) -> f64 {
        self.shared.linear_rate.load(Ordering::Relaxed) as f64
    }

    /// Channels of a linear call.
    pub fn linear_channels(&self) -> usize {
        usize::from(self.shared.linear_channels.load(Ordering::Relaxed))
    }

    fn connect_as(&self, remote: SocketAddr, payload_type: u8, octets: u64, linear: bool) {

        let same_place = self
            .shared
            .dialled
            .lock()
            .map(|dialled| *dialled == Some(remote))
            .unwrap_or(false);
        if self.connected()
            && same_place
            && self.shared.payload_type.load(Ordering::Relaxed) == payload_type
            && self.shared.packet_octets.load(Ordering::Relaxed) == octets
            && self.shared.linear.load(Ordering::Relaxed) == linear
        {
            return;
        }
        self.shared.linear.store(linear, Ordering::Relaxed);

        self.shared.packet_octets.store(octets, Ordering::Relaxed);
        self.shared
            .payload_type
            .store(payload_type, Ordering::Relaxed);
        if let Ok(mut slot) = self.shared.dialled.lock() {
            *slot = Some(remote);
        }
        if let Ok(mut slot) = self.shared.remote.lock() {
            *slot = Some(remote);
        }
        // A different call, or this one moved, so its stream is learned again
        // from the first packet that arrives.
        if let Ok(mut slot) = self.shared.source.lock() {
            *slot = None;
        }
        if let Ok(mut jitter) = self.shared.incoming.lock() {
            // Whatever was still waiting is counted into `abandoned` on the
            // way out, so that a call which really did change underneath us
            // says how much audio that cost.
            jitter.restart();
        }
        if let Ok(mut queue) = self.shared.outgoing.lock() {
            self.shared
                .flushed_out
                .fetch_add(queue.len() as u64, Ordering::Relaxed);
            queue.clear();
            // No cushion: the modem and the pacer share a clock now, so
            // there is no slip between them for one to absorb.
        }
        // The modem's clock starts again with the new call's stream.
        if let Ok(mut conversion) = self.conversion.lock() {
            conversion.playout = Playout::default();
        }
        // A new call, so nothing has been said on it yet, whatever the modem
        // was doing on the last one.
        self.shared.spoke.store(false, Ordering::Relaxed);
        self.shared.connected.store(true, Ordering::Relaxed);
    }

    /// The call is over. The socket stays bound: the next call on this line
    /// reuses it, and a port that moves between calls is a port some far end
    /// is still sending the last call to.
    pub fn disconnect(&self) {
        self.shared.connected.store(false, Ordering::Relaxed);
        self.shared.spoke.store(false, Ordering::Relaxed);
        if let Ok(mut conversion) = self.conversion.lock() {
            conversion.playout = Playout::default();
        }
        if let Ok(mut slot) = self.shared.remote.lock() {
            *slot = None;
        }
        if let Ok(mut slot) = self.shared.dialled.lock() {
            *slot = None;
        }
        if let Ok(mut slot) = self.shared.source.lock() {
            *slot = None;
        }
        if let Ok(mut queue) = self.shared.outgoing.lock() {
            // The tail of the call, counted rather than dropped quietly. It
            // is ordinarily a few tens of milliseconds of whatever the modem
            // was saying when the far end hung up, and the only way to tell
            // that from a call cut off mid-sentence is the number.
            self.shared
                .flushed_out
                .fetch_add(queue.len() as u64, Ordering::Relaxed);
            queue.clear();
        }
    }

    pub fn connected(&self) -> bool {
        self.shared.connected.load(Ordering::Relaxed)
    }

    /// What the line has carried since the last call, at the modem's rate.
    ///
    /// This is the modem's clock, as the input device is for a sound card:
    /// the modem is stepped once for each sample, and what it says goes back.
    /// Nothing comes until the far end's stream has filled the jitter buffer
    /// -- so a modem is never started into a call that has not answered --
    /// and from then on exactly as many samples as a steady local clock says
    /// are due, whatever has arrived. Audio that is late is silence here; see
    /// the module header for why that is the right end to put the hole in.
    pub fn receive(&self, into: &mut Vec<f32>) {
        self.receive_both(into, None);
    }

    /// [`Self::receive`], and beside it the same audio as it came off the
    /// network: one level for every octet, before the rate conversion. A
    /// V.90 or V.92 digital modem's receiver wants those (see
    /// `Modem::give_network_levels`).
    pub fn receive_both(&self, into: &mut Vec<f32>, mut network: Option<&mut Vec<f32>>) {
        let law = self.law();
        let linear = self.linear();
        let Ok(mut conversion) = self.conversion.lock() else {
            return;
        };
        let conversion = &mut *conversion;
        conversion.codewords.clear();
        let quiet = if linear { 0 } else { law.encode(0) };
        let frame = if linear { 2 * self.linear_channels() } else { 1 };
        let octet_rate = if linear { frame as f64 * self.linear_rate() } else { NETWORK_RATE };
        if let Ok(mut jitter) = self.shared.incoming.lock() {
            self.play(&mut jitter, &mut conversion.playout, &mut conversion.codewords, quiet, frame, octet_rate);
        }
        if conversion.codewords.is_empty() {
            return;
        }
        conversion.at_network_rate.clear();
        if linear {
            // Two octets a sample, most significant first (RFC 3551 4.5.11).
            // A sixteen-bit integer over 32768 is exact in a float, so what
            // comes out here is what the far end put in, to the bit.
            if self.linear_channels() == 2 {
                // One signal on two channels: their mean, which is exact
                // when they are the same and the best there is when not.
                for four in conversion.codewords.as_chunks::<4>().0 {
                    let left = i32::from(i16::from_be_bytes([four[0], four[1]]));
                    let right = i32::from(i16::from_be_bytes([four[2], four[3]]));
                    conversion.at_network_rate.push(((left + right) as f32 / 2.0) / 32768.0);
                }
            } else {
                for pair in conversion.codewords.as_chunks::<2>().0 {
                    conversion.at_network_rate.push(f32::from(i16::from_be_bytes(*pair)) / 32768.0);
                }
            }
        } else {
            g711::decode_into(law, &conversion.codewords, &mut conversion.at_network_rate);
        }
        if let Some(network) = network.as_mut() {
            network.extend_from_slice(&conversion.at_network_rate);
        }
        if linear && conversion.same_rate {
            into.extend_from_slice(&conversion.at_network_rate);
            return;
        }
        let up = if linear { &mut conversion.linear_up } else { &mut conversion.up };
        for &sample in conversion.at_network_rate.iter() {
            conversion.scratch.clear();
            up.process(f64::from(sample), &mut conversion.scratch);
            into.extend(conversion.scratch.iter().map(|s| *s as f32));
        }
    }

    /// Move the play-out clock on to now and hand over what it says is due.
    fn play(&self, jitter: &mut Jitter, playout: &mut Playout, out: &mut Vec<u8>, quiet: u8, frame: usize, octet_rate: f64) {
        let now = Instant::now();
        let Some(last) = playout.last else {
            if jitter.ready() {
                // The stream has its cushion: the clock starts here.
                playout.last = Some(now);
                playout.depth = jitter.queued() as f64;
            }
            return;
        };
        let dt = now.saturating_duration_since(last).as_secs_f64();
        playout.last = Some(now);
        playout.due += dt * octet_rate * (1.0 + playout.ppm * 1e-6);
        let most_behind = MOST_BEHIND * octet_rate;
        if playout.due > most_behind {
            let skipped = playout.due - most_behind;
            self.shared.fell_behind.fetch_add(skipped as u64, Ordering::Relaxed);
            playout.due = most_behind;
        }

        // Steer the clock to keep the buffer at its depth. The depth swings
        // by a packet as each arrives and is played, and to nothing in a
        // stall; averaged over seconds, what is left is the two clocks'
        // difference.
        let packet = match jitter.packet_len() {
            0 => self.shared.packet_octets.load(Ordering::Relaxed) as f64,
            n => n as f64,
        };
        let k = (dt / DEPTH_SMOOTHING).min(1.0);
        playout.depth += (jitter.queued() as f64 - playout.depth) * k;
        let target = (self.shared.jitter_target as f64 + 0.5) * packet;
        playout.ppm = ((playout.depth - target) / packet.max(1.0) * SLEW_PPM_PER_PACKET).clamp(-MAX_SLEW_PPM, MAX_SLEW_PPM);
        self.shared.clock_ppm.store(playout.ppm.to_bits(), Ordering::Relaxed);

        let least = ((LEAST_HANDED * octet_rate) as usize).max(1).div_ceil(frame) * frame;
        let most = ((MOST_HANDED * octet_rate) as usize / frame).max(1) * frame;
        let take = (playout.due as usize / frame * frame).min(most);
        if take < least {
            return;
        }
        playout.due -= take as f64;
        jitter.play(out, take, quiet, frame);
    }

    /// Samples for the line, at the modem's rate.
    pub fn transmit(&self, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }
        let law = self.law();
        let linear = self.linear();
        let Ok(mut conversion) = self.conversion.lock() else {
            return;
        };
        let conversion = &mut *conversion;
        conversion.at_network_rate.clear();
        if linear && conversion.same_rate {
            conversion.at_network_rate.extend_from_slice(samples);
        } else {
            let down = if linear { &mut conversion.linear_down } else { &mut conversion.down };
            for &sample in samples {
                conversion.scratch.clear();
                down.process(f64::from(sample), &mut conversion.scratch);
                conversion
                    .at_network_rate
                    .extend(conversion.scratch.iter().map(|s| *s as f32));
            }
        }
        conversion.codewords.clear();
        if linear {
            encode_linear(&conversion.at_network_rate, self.linear_channels(), &mut conversion.codewords);
        } else {
            g711::encode_into(law, &conversion.at_network_rate, &mut conversion.codewords);
        }
        self.queue(&conversion.codewords);
    }

    /// Samples for the line that are at the network's rate already: one in,
    /// one octet out, and nothing between them but the encoder.
    ///
    /// For a V.90 or V.92 digital modem, whose levels are codewords it chose
    /// one at a time. Through [`Self::transmit`] they would be filtered on
    /// their way down from the modem's rate, and a level that has been
    /// filtered encodes to its neighbour as often as to itself.
    pub fn transmit_exact(&self, levels: &[f32]) {
        if levels.is_empty() {
            return;
        }
        let law = self.law();
        let Ok(mut conversion) = self.conversion.lock() else {
            return;
        };
        let conversion = &mut *conversion;
        conversion.codewords.clear();
        if self.linear() {
            encode_linear(levels, self.linear_channels(), &mut conversion.codewords);
        } else {
            g711::encode_into(law, levels, &mut conversion.codewords);
        }
        self.queue(&conversion.codewords);
    }

    /// Octets for the pacer, oldest out first when there are too many.
    fn queue(&self, codewords: &[u8]) {
        let packet = (self.shared.packet_octets.load(Ordering::Relaxed) as usize).max(1);
        let limit = packet * OUTGOING_LIMIT;
        let Ok(mut queue) = self.shared.outgoing.lock() else {
            return;
        };
        if !codewords.is_empty() {
            // The modem has said something on this call, so an underrun from
            // here on is a hole in what it was saying rather than the quiet
            // before it started.
            self.shared.spoke.store(true, Ordering::Relaxed);
        }
        for &code in codewords.iter() {
            if queue.len() >= limit {
                // A whole packet of the oldest, never an octet at a time: a
                // stretch cut out of the stream moves what follows it by its
                // own length, and only a whole packet moves the line's tones
                // by a whole number of cycles (see the dry tick in `pace`).
                let gone = packet.min(queue.len());
                queue.drain(..gone);
                self.shared.dropped_out.fetch_add(gone as u64, Ordering::Relaxed);
            }
            queue.push_back(code);
        }
    }

    /// The law in force, read back from the payload type.
    pub fn law(&self) -> Law {
        Law::from_payload_type(self.shared.payload_type.load(Ordering::Relaxed)).unwrap_or(Law::Mu)
    }

    pub fn stats(&self) -> Stats {
        let jitter = self
            .shared
            .incoming
            .lock()
            .map(|j| j.counters().clone())
            .unwrap_or_default();
        Stats {
            connected: self.connected(),
            remote: self
                .shared
                .remote
                .lock()
                .ok()
                .and_then(|r| r.map(|a| a.to_string())),
            law: Some(if self.linear() { "L16" } else { self.law().encoding_name() }),
            packets_sent: self.shared.packets_sent.load(Ordering::Relaxed),
            packets_received: self.shared.packets_received.load(Ordering::Relaxed),
            ignored: self.shared.ignored.load(Ordering::Relaxed),
            off_codec: self.shared.off_codec.load(Ordering::Relaxed),
            underruns: self.shared.underruns.load(Ordering::Relaxed),
            underruns_mid_call: self.shared.underruns_mid_call.load(Ordering::Relaxed),
            inserted: self.shared.inserted.load(Ordering::Relaxed),
            sent_behind: self.shared.sent_behind.load(Ordering::Relaxed),
            fell_behind: self.shared.fell_behind.load(Ordering::Relaxed),
            clock_ppm: f64::from_bits(self.shared.clock_ppm.load(Ordering::Relaxed)),
            dropped_out: self.shared.dropped_out.load(Ordering::Relaxed),
            flushed_out: self.shared.flushed_out.load(Ordering::Relaxed),
            latched: self.shared.latched.load(Ordering::Relaxed),
            strangers: self.shared.strangers.load(Ordering::Relaxed),
            jitter,
        }
    }
}

impl Drop for Media {
    fn drop(&mut self) {
        self.shared.running.store(false, Ordering::Relaxed);
        self.shared.connected.store(false, Ordering::Relaxed);
        // Waking the reader out of its timeout rather than waiting for it:
        // the timeout is short, but a socket closed from under a blocked
        // recv is an error report on some systems and a hang on others. An
        // empty datagram is not RTP and is discarded as soon as it is looked
        // at, which is all this needs it to be.
        let wake = SocketAddr::from(([127, 0, 0, 1], self.local_port));
        let _ = self.socket.send_to(&[], wake);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

/// The reader: datagrams in, payloads into the buffer, nothing else.
fn read(shared: Arc<Shared>, socket: UdpSocket) {
    // A linear call at 48 kHz on two channels is 3840 octets every 20 ms.
    let mut buffer = vec![0u8; 16384];
    while shared.running.load(Ordering::Relaxed) {
        let Ok((n, from)) = socket.recv_from(&mut buffer) else {
            continue;
        };
        if !shared.connected.load(Ordering::Relaxed) {
            // Audio for a call that is not up: somebody else's, or the last
            // call's. Not counted as ignored, because it is not a fault.
            continue;
        }
        let datagram = &buffer[..n];
        if rtp::is_rtcp(datagram) {
            // Reports and sender statistics, which arrive on this port when a
            // far end does not honour the port pair. Nothing here acts on
            // them; decoding one as audio would be a burst of noise.
            continue;
        }
        let Some(packet) = rtp::Packet::parse(datagram) else {
            shared.ignored.fetch_add(1, Ordering::Relaxed);
            continue;
        };

        // Only the audio we agreed on goes any further.
        //
        // This used to let anything that parsed through, and the jitter
        // buffer decodes whatever it is handed as G.711. So an RFC 4733
        // telephone-event -- four octets, and our SDP answer mirrors the far
        // end's offer of them, so an incoming call has told the far end it
        // may send them -- took a sequence number and handed the modem four
        // octets where a hundred and sixty belonged. That is a 20 ms deletion
        // of the audio timeline for each one, and one dialled digit is ten to
        // twenty of them. Comfort noise, payload type 13, is one octet and
        // does the same thing.
        //
        // Dropped here rather than in the buffer because this is the only
        // layer that knows what was negotiated: `jitter` is handed a stream
        // and cannot tell an agreed codec from an unexpected one. The
        // sequence number the packet used is left as a gap, which the buffer
        // conceals to the right length, so the timeline keeps its length and
        // the two counters together say what happened.
        let agreed = shared.payload_type.load(Ordering::Relaxed);
        if packet.payload_type != agreed {
            shared.off_codec.fetch_add(1, Ordering::Relaxed);
            continue;
        }

        // Symmetric RTP: follow the address the audio actually came from.
        //
        // The address in the far end's SDP is the address it believes it has,
        // which behind a router is not the address its packets arrive from,
        // and sending to the SDP address is the classic one-way-audio call.
        // Following the source is what every other agent does and what every
        // trunk expects.
        //
        // Once, and then only the stream itself. Following every new source
        // let a single packet from anywhere take the call over: its payload
        // went to the modem and every packet the modem sent went back to it.
        // So the first packet of a call says where the stream is; from then
        // on the same address is the stream, whatever SSRC it carries, and
        // another address is the stream only if it carries on from where the
        // stream had got to (RFC 3550 8.2, A.1).
        let follow = match shared.source.lock() {
            Ok(mut slot) => match *slot {
                None => {
                    *slot = Some(Source { address: from, ssrc: packet.ssrc, sequence: packet.sequence });
                    true
                }
                Some(ref mut source) if source.address == from => {
                    if source.ssrc != packet.ssrc || packet.sequence.wrapping_sub(source.sequence) < 0x8000 {
                        source.sequence = packet.sequence;
                    }
                    source.ssrc = packet.ssrc;
                    false
                }
                Some(ref mut source) if source.moved_to(packet.ssrc, packet.sequence) => {
                    source.address = from;
                    source.sequence = packet.sequence;
                    true
                }
                Some(_) => {
                    shared.strangers.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
            },
            Err(_) => false,
        };
        if follow
            && let Ok(mut slot) = shared.remote.lock()
            && *slot != Some(from)
        {
            if slot.is_some() {
                shared.latched.fetch_add(1, Ordering::Relaxed);
            }
            *slot = Some(from);
        }

        shared.packets_received.fetch_add(1, Ordering::Relaxed);
        let mut payload = packet.payload;
        if let Some(robbed) = robbed_bits()
            && matches!(packet.payload_type, 0 | 8)
        {
            rob(&mut payload, packet.timestamp, robbed);
        }
        if let Ok(mut jitter) = shared.incoming.lock() {
            jitter.push(packet.sequence, packet.payload_type, payload);
        }
    }
}

/// The pacer: each packet of what the modem says, the moment it is complete.
///
/// The modem is stepped on the play-out clock (see `Media::play`), so its
/// audio arrives here at that clock's rate, a turn of the modem loop at a
/// time, and a packet is complete exactly when the clock has run a packet
/// time since the last. Sending it then is sending on the play-out clock's
/// schedule; there is no second clock here to slip against it, and so no
/// cushion and no silence while the modem is talking.
///
/// Silence goes out in two places only. Before the modem has said anything
/// on the call -- the far end's stream has not filled the jitter buffer yet --
/// one packet every packet time, which is a hole in nothing; and when the
/// modem has said nothing for [`DRY_LIMIT`], which is a splice and counted
/// as one in `inserted`.
fn pace(shared: Arc<Shared>, socket: UdpSocket) {
    let mut stream: Option<rtp::Stream> = None;
    let mut datagram = Vec::with_capacity(256);
    let mut payload: Vec<u8> = Vec::with_capacity(256);
    let mut first = true;
    // When the next packet of silence is due, before the modem speaks.
    let mut due = Instant::now();
    // When the last packet went, and whether it was silence sent because the
    // modem had stopped.
    let mut last_sent = Instant::now();
    let mut dry = false;
    // Whether the modem's audio has begun on this call.
    let mut speaking = false;

    while shared.running.load(Ordering::Relaxed) {
        let octets = (shared.packet_octets.load(Ordering::Relaxed) as usize).max(1);
        let linear = shared.linear.load(Ordering::Relaxed);
        // Octets a second: one a sample at 8 kHz, or two a sample a channel
        // at the linear call's rate.
        let frame = 2 * usize::from(shared.linear_channels.load(Ordering::Relaxed)).max(1);
        let octet_rate = if linear {
            frame as f64 * shared.linear_rate.load(Ordering::Relaxed) as f64
        } else {
            NETWORK_RATE
        };
        let interval = Duration::from_secs_f64(octets as f64 / octet_rate);
        let now = Instant::now();

        if !shared.connected.load(Ordering::Relaxed) {
            stream = None;
            first = true;
            speaking = false;
            dry = false;
            due = now;
            thread::sleep(Duration::from_millis(1));
            continue;
        }
        let Some(remote) = shared.remote.lock().ok().and_then(|r| *r) else {
            thread::sleep(Duration::from_millis(1));
            continue;
        };
        let payload_type = shared.payload_type.load(Ordering::Relaxed);
        // Silence in the law in force, so the far end hears a quiet line
        // rather than a click.
        let quiet = if linear { 0 } else { Law::from_payload_type(payload_type).unwrap_or(Law::Mu).encode(0) };
        // A stream is one coding from its first packet to its last, so a
        // call that has changed coding starts another.
        if stream.as_ref().is_some_and(|s| s.payload_type() != payload_type & 0x7f) {
            stream = None;
            first = true;
        }

        payload.clear();
        // What goes, if anything: the modem's audio, silence before it, or
        // silence in place of it.
        let mut silence = None;
        if let Ok(mut queue) = shared.outgoing.lock() {
            let spoke = shared.spoke.load(Ordering::Relaxed);
            if queue.len() >= octets && (speaking || spoke) {
                // A whole packet of the modem's audio. Never faster than
                // twice the packet rate, so a loop that delivered several at
                // once after running late spreads them out rather than
                // landing them on the far end in one burst.
                if !speaking || dry || now >= last_sent + interval / 2 {
                    speaking = true;
                    dry = false;
                    payload.extend(queue.drain(..octets));
                    if queue.len() >= octets {
                        shared.sent_behind.fetch_add(1, Ordering::Relaxed);
                    }
                }
            } else if !speaking {
                if now >= due {
                    payload.resize(octets, quiet);
                    silence = Some(false);
                }
            } else if now >= last_sent + if dry { interval } else { DRY_LIMIT } {
                // The modem has stopped. What part of a packet it left goes
                // out padded, and silence after it every packet time.
                payload.extend(queue.drain(..));
                shared.inserted.fetch_add((octets - payload.len()) as u64, Ordering::Relaxed);
                payload.resize(octets, quiet);
                dry = true;
                silence = Some(true);
            }
        }
        if payload.is_empty() {
            thread::sleep(Duration::from_millis(1));
            continue;
        }
        match silence {
            Some(mid_call) => {
                shared.underruns.fetch_add(1, Ordering::Relaxed);
                if mid_call {
                    shared.underruns_mid_call.fetch_add(1, Ordering::Relaxed);
                }
                due += interval;
                if due < now {
                    due = now + interval;
                }
            }
            None => due = now + interval,
        }

        let sender = stream.get_or_insert_with(|| rtp::Stream::new(payload_type));
        let samples = if linear { payload.len() / frame } else { payload.len() };
        sender.next_of(&payload, first, samples as u32, &mut datagram);
        first = false;
        last_sent = now;
        if socket.send_to(&datagram, remote).is_ok() {
            shared.packets_sent.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// A test line with robbed-bit signalling on it, asked for with
/// `BINMODEM_ROB_BIT`: which of every six octets of G.711 arriving here have
/// their least significant bit overwritten, and with what.
///
/// A T1 trunk without a signalling channel of its own takes that bit of
/// every sixth frame to say whether the line is on hook. Speech does not
/// notice. A V.90 or V.92 modem does, because the octet is its data: in
/// that one position of six, each pair of neighbouring codewords arrives as
/// one. No call over SIP ever has this done to it, so this is the only way
/// to see what the modem makes of it.
///
/// `3` robs position 3 and writes a one; `0,3:0` robs positions 0 and 3,
/// as a call across two trunks has it, and writes a nought. The positions
/// are counted from the RTP timestamp, so a lost packet does not move
/// them.
fn robbed_bits() -> Option<([bool; 6], u8)> {
    static ASKED: std::sync::OnceLock<Option<([bool; 6], u8)>> = std::sync::OnceLock::new();
    *ASKED.get_or_init(|| parse_robbed(&std::env::var("BINMODEM_ROB_BIT").ok()?))
}

fn parse_robbed(asked: &str) -> Option<([bool; 6], u8)> {
    let (positions, bit) = asked.split_once(':').unwrap_or((asked, "1"));
    let mut which = [false; 6];
    for position in positions.split(',') {
        *which.get_mut(position.trim().parse::<usize>().ok()?)? = true;
    }
    let bit = match bit.trim() {
        "0" => 0,
        "1" => 1,
        _ => return None,
    };
    which.contains(&true).then_some((which, bit))
}

/// Overwrite the robbed bits of a packet whose first octet is sample
/// `timestamp` of the call.
fn rob(payload: &mut [u8], timestamp: u32, (which, bit): ([bool; 6], u8)) {
    for (i, octet) in payload.iter_mut().enumerate() {
        if which[(timestamp as usize).wrapping_add(i) % 6] {
            *octet = (*octet & 0xFE) | bit;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The jitter buffer's depth the packet tests are written for: five,
    /// the default before it became fifteen. Their timings and the stall
    /// test's 100 to 300 ms stalls are sized against it.
    const TEST_JITTER: usize = 5;

    #[test]
    fn a_robbed_bit_is_one_octet_in_six_whatever_packet_it_is_in() {
        let asked = parse_robbed("3").unwrap();
        let mut first = vec![0u8; 160];
        let mut second = vec![0u8; 160];
        rob(&mut first, 0, asked);
        rob(&mut second, 160, asked);
        let all: Vec<u8> = first.into_iter().chain(second).collect();
        for (i, octet) in all.iter().enumerate() {
            assert_eq!(*octet, u8::from(i % 6 == 3), "octet {i}");
        }
        let mut ones = vec![0xFFu8; 12];
        rob(&mut ones, 0, parse_robbed("0,3:0").unwrap());
        assert_eq!(ones.iter().filter(|o| **o == 0xFE).count(), 4);
        assert!(parse_robbed("6").is_none() && parse_robbed("").is_none() && parse_robbed("1:2").is_none());
    }

    /// Opening binds a port and hands it back, which is what the SDP offer
    /// needs before there is any call to offer.
    /// Two of these facing each other on a linear call: every sample that
    /// goes in at one end comes out of the other as the same sixteen bits,
    /// at 16 kHz, with no law and no rate conversion anywhere between.
    #[test]
    fn a_linear_call_carries_every_sample_exactly() {
        exactly(16_000, 1);
    }

    /// And at the other rates, and on two channels, where a packet at
    /// 48 kHz is more than one Ethernet frame.
    #[test]
    fn a_linear_call_at_any_of_its_rates_carries_every_sample_exactly() {
        for (rate, channels) in [(8_000, 2), (44_100, 1), (48_000, 1), (48_000, 2)] {
            exactly(rate, channels);
        }
    }

    fn exactly(rate: u32, channels: u8) {
        let near = Media::open(0, f64::from(rate)).unwrap();
        let far = Media::open(0, f64::from(rate)).unwrap();
        let to = |m: &Media| SocketAddr::from(([127, 0, 0, 1], m.local_port()));
        near.connect_linear(to(&far), 96, 20, rate, channels);
        far.connect_linear(to(&near), 96, 20, rate, channels);
        assert_eq!((far.linear_rate(), far.linear_channels()), (f64::from(rate), usize::from(channels)));
        let packet = rate as usize / 50;
        assert!(near.linear() && far.linear());
        assert_eq!(far.stats().law, Some("L16"));

        // Every sixteen-bit value there is, twice round, in an order that
        // would not survive any filter -- and on to the end of that packet.
        // A last part packet is kept back behind silence until the pacer has
        // been dry for `DRY_BEFORE_FLUSH` ticks, which is right for a modem
        // that has stalled and not what this is testing.
        let whole = 131_072u32.div_ceil(packet as u32) * packet as u32;
        let sent: Vec<i16> = (0..whole).map(|i| (i.wrapping_mul(40_503) & 0xffff) as u16 as i16).collect();
        let levels: Vec<f32> = sent.iter().map(|&v| f32::from(v) / 32768.0).collect();
        let mut got: Vec<i16> = Vec::new();
        let (mut modem, mut network) = (Vec::new(), Vec::new());
        let started = Instant::now();
        let mut offered = 0;
        while got.len() < sent.len() + rate as usize && started.elapsed() < Duration::from_secs(30) {
            // A packet's worth at a time, at the pace a call would go.
            if offered < levels.len() && offered as f64 <= started.elapsed().as_secs_f64() * f64::from(rate) {
                let end = (offered + packet).min(levels.len());
                near.transmit_exact(&levels[offered..end]);
                offered = end;
            }
            modem.clear();
            network.clear();
            far.receive_both(&mut modem, Some(&mut network));
            // A modem at the same rate is handed the network's own samples.
            assert_eq!(modem, network);
            got.extend(network.iter().map(|l| (l * 32768.0) as i16));
            thread::sleep(Duration::from_millis(2));
            if offered == levels.len() && got.len() >= sent.len() && got.ends_with(&vec![0; 2 * packet]) {
                break;
            }
        }
        // Silence before the first sample and after the last, and between
        // them exactly what was sent.
        let first = got.iter().position(|&v| v == sent[1]).expect("the stream arrived") - 1;
        assert!(got.len() >= first + sent.len(), "{rate}/{channels}: {} of {} samples", got.len() - first, sent.len());
        assert!(got[first..first + sent.len()] == sent[..], "{rate}/{channels}");
        assert_eq!(far.stats().jitter.lost, 0);
    }

    #[test]
    fn a_linear_packet_is_a_packet_of_samples_not_of_octets() {
        let media = Media::open(0, 16_000.0).unwrap();
        let far = UdpSocket::bind("127.0.0.1:0").unwrap();
        far.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        media.connect_linear(far.local_addr().unwrap(), 96, 20, 16_000, 1);
        let mut buf = [0u8; 2048];
        let mut stamps = Vec::new();
        for _ in 0..3 {
            let (n, _) = far.recv_from(&mut buf).unwrap();
            let packet = rtp::Packet::parse(&buf[..n]).unwrap();
            assert_eq!(packet.payload_type, 96);
            // Twenty milliseconds at 16 kHz: 320 samples, 640 octets.
            assert_eq!(packet.payload.len(), 640);
            stamps.push(packet.timestamp);
        }
        assert_eq!(stamps[1].wrapping_sub(stamps[0]), 320);
        assert_eq!(stamps[2].wrapping_sub(stamps[1]), 320);
    }

    #[test]
    fn opening_chooses_a_port() {
        let media = Media::open(0, 16_000.0).unwrap();
        assert!(media.local_port() > 0);
        assert!(!media.connected());
    }

    /// Nothing is sent before the call is answered, however much the modem
    /// says. A trunk that receives RTP before it has sent a 200 sometimes
    /// treats the call as answered, which would bill for a call that never
    /// happened.
    #[test]
    fn nothing_leaves_before_the_call_is_up() {
        let media = Media::open(0, 8000.0).unwrap();
        media.transmit(&[0.5; 800]);
        thread::sleep(Duration::from_millis(80));
        assert_eq!(media.stats().packets_sent, 0);
    }

    /// The whole path, in both directions, through a real socket: a packet
    /// sent to the media port comes back out as samples, and samples handed
    /// in arrive at the far end as G.711.
    #[test]
    fn a_packet_crosses_and_comes_back() {
        let far = UdpSocket::bind("127.0.0.1:0").unwrap();
        far.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let far_address = far.local_addr().unwrap();

        // The modem rate is the network rate here on purpose: the rate
        // conversion is tested in `dsp`, and leaving it out makes this test
        // about the packets rather than about the filter.
        let media = Media::open_with_jitter(0, NETWORK_RATE, TEST_JITTER).unwrap();
        let ours = SocketAddr::from(([127, 0, 0, 1], media.local_port()));
        media.connect(far_address, Law::Mu, g711::PCMU, 20);

        // Six packets in for four out: the buffer keeps `JITTER_TARGET` of
        // them in hand at all times, which is the delay it exists to spend.
        let mut out = Vec::new();
        for sequence in 0..6u16 {
            let payload: Vec<u8> = (0..160).map(|n| g711::ulaw_encode(n * 100)).collect();
            let packet = rtp::Packet {
                payload_type: g711::PCMU,
                marker: sequence == 0,
                sequence,
                timestamp: u32::from(sequence) * 160,
                ssrc: 0x1234_5678,
                payload,
            };
            let mut datagram = Vec::new();
            packet.write(&mut datagram);
            far.send_to(&datagram, ours).unwrap();
        }
        // Long enough for the reader to have taken them all; the first look
        // starts the play-out clock and the second is handed what it says.
        thread::sleep(Duration::from_millis(100));
        media.receive(&mut out);
        thread::sleep(Duration::from_millis(50));
        media.receive(&mut out);
        assert!(
            out.len() >= 320,
            "expected at least two packets of samples, got {}",
            out.len()
        );

        // And the other way: what the modem says turns up as RTP.
        media.transmit(&[0.25; 320]);
        let mut heard = vec![0u8; 2048];
        let (n, _) = far.recv_from(&mut heard).unwrap();
        let packet = rtp::Packet::parse(&heard[..n]).expect("that was not RTP");
        assert_eq!(packet.payload_type, g711::PCMU);
        assert_eq!(packet.payload.len(), 160);
    }

    /// A far end that sends from somewhere other than the address it
    /// advertised is followed there, which is the difference between audio
    /// and one-way audio on a home connection.
    #[test]
    fn the_far_end_is_followed_to_where_it_actually_is() {
        let real = UdpSocket::bind("127.0.0.1:0").unwrap();
        real.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let media = Media::open(0, NETWORK_RATE).unwrap();
        let ours = SocketAddr::from(([127, 0, 0, 1], media.local_port()));
        // Told an address nothing is listening on.
        media.connect(SocketAddr::from(([127, 0, 0, 1], 9)), Law::Mu, g711::PCMU, 20);

        let packet = rtp::Packet {
            payload_type: g711::PCMU,
            marker: true,
            sequence: 1,
            timestamp: 160,
            ssrc: 1,
            payload: vec![0xFF; 160],
        };
        let mut datagram = Vec::new();
        packet.write(&mut datagram);
        real.send_to(&datagram, ours).unwrap();
        thread::sleep(Duration::from_millis(60));

        media.transmit(&[0.0; 320]);
        let mut heard = vec![0u8; 2048];
        let (n, from) = real.recv_from(&mut heard).expect("nothing came back");
        assert!(rtp::Packet::parse(&heard[..n]).is_some());
        assert_eq!(from, ours);
        assert_eq!(media.stats().latched, 1);
    }

    /// A media path with a far end that has a socket and says nothing: the
    /// setup three of the tests below all need.
    fn connected_to_a_quiet_far_end() -> (UdpSocket, Media, SocketAddr) {
        let far = UdpSocket::bind("127.0.0.1:0").expect("a far end socket");
        far.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let far_address = far.local_addr().unwrap();
        let media = Media::open_with_jitter(0, NETWORK_RATE, TEST_JITTER).expect("a media path");
        let ours = SocketAddr::from(([127, 0, 0, 1], media.local_port()));
        media.connect(far_address, Law::Mu, g711::PCMU, 20);
        (far, media, ours)
    }

    /// One packet from the far end, of whatever type it likes.
    fn send_rtp(far: &UdpSocket, to: SocketAddr, sequence: u16, payload_type: u8, payload: Vec<u8>) {
        let packet = rtp::Packet {
            payload_type,
            marker: sequence == 0,
            sequence,
            timestamp: u32::from(sequence) * 160,
            ssrc: 0x5EED_5EED,
            payload,
        };
        let mut datagram = Vec::new();
        packet.write(&mut datagram);
        far.send_to(&datagram, to).expect("the far end could not send");
    }

    /// A dialled digit arrives as RFC 4733 events, and not one octet of it
    /// reaches the modem.
    ///
    /// What went wrong: `read` filtered RTCP and nothing else, so a
    /// four-octet event packet went into the buffer at the next sequence
    /// number and was handed to the modem as four octets of G.711 -- 20 ms of
    /// the audio timeline deleted, uncounted, which is the exact fault this
    /// crate was written to remove from the path. It is reachable today,
    /// because our SDP answer mirrors a far end's telephone-event offer and
    /// so tells it that it may send them.
    #[test]
    fn a_telephone_event_never_reaches_the_modem() {
        let (far, media, ours) = connected_to_a_quiet_far_end();

        // Audio, one event packet in the middle of it -- 101 is the type
        // every trunk offers for RFC 4733 -- and audio again.
        for sequence in 0..2u16 {
            send_rtp(&far, ours, sequence, g711::PCMU, vec![0x7F; 160]);
        }
        send_rtp(&far, ours, 2, 101, vec![0x05, 0x0A, 0x01, 0x40]);
        // Enough behind it to fill the jitter buffer.
        for sequence in 3..9u16 {
            send_rtp(&far, ours, sequence, g711::PCMU, vec![0x7F; 160]);
        }
        thread::sleep(Duration::from_millis(150));

        // The first look starts the play-out clock; 60 ms later it hands
        // over three packets' worth, the event's among them.
        let mut out = Vec::new();
        media.receive(&mut out);
        thread::sleep(Duration::from_millis(60));
        media.receive(&mut out);
        assert!(!out.is_empty(), "no audio came through at all");

        let stats = media.stats();
        assert_eq!(stats.off_codec, 1, "the event packet was decoded as audio: {stats:?}");
        assert_eq!(stats.packets_received, 8, "the event was counted as audio: {stats:?}");
        // The 20 ms it stood in for is a gap like any other, so the timeline
        // keeps its length -- and the silence is a whole packet long rather
        // than the four octets the event happened to carry.
        assert_eq!(stats.jitter.lost, 1, "{stats:?}");
        assert_eq!(
            stats.jitter.concealed, 160,
            "the concealment was sized from the event packet: {stats:?}"
        );
    }

    /// The same call answered twice over does not cost the call its audio.
    ///
    /// A 200 OK that arrives again, or any re-INVITE, sends `line::Line::poll`
    /// back through `connect` mid-call. That used to restart the jitter buffer
    /// and empty the outgoing queue -- a discontinuity in both directions, in
    /// the middle of a call that nothing was wrong with, and silent.
    #[test]
    fn connecting_again_to_the_same_call_does_not_disturb_it() {
        let (far, media, ours) = connected_to_a_quiet_far_end();
        let far_address = far.local_addr().unwrap();
        for sequence in 0..4u16 {
            send_rtp(&far, ours, sequence, g711::PCMU, vec![0x7F; 160]);
        }
        thread::sleep(Duration::from_millis(150));
        media.transmit(&[0.5; 1600]);
        let depth = media.shared.incoming.lock().unwrap().depth();
        assert_eq!(depth, 4, "the far end's audio never arrived");

        media.connect(far_address, Law::Mu, g711::PCMU, 20);

        let stats = media.stats();
        assert_eq!(stats.jitter.abandoned, 0, "the buffer was thrown away: {stats:?}");
        assert_eq!(stats.flushed_out, 0, "the outgoing queue was thrown away: {stats:?}");
        assert_eq!(media.shared.incoming.lock().unwrap().depth(), depth);
    }

    /// And a connect that really is a different call does throw both away --
    /// but says how much audio went with them, which is the difference
    /// between a discontinuity and an unexplained one.
    #[test]
    fn a_connect_that_changes_the_call_counts_what_it_throws_away() {
        let (far, media, ours) = connected_to_a_quiet_far_end();
        let far_address = far.local_addr().unwrap();
        for sequence in 0..4u16 {
            send_rtp(&far, ours, sequence, g711::PCMU, vec![0x7F; 160]);
        }
        thread::sleep(Duration::from_millis(150));
        media.transmit(&[0.5; 1600]);

        // A different packet time is a different call.
        media.connect(far_address, Law::Mu, g711::PCMU, 40);

        let stats = media.stats();
        assert!(stats.jitter.abandoned >= 320, "the buffer went quietly: {stats:?}");
        assert!(stats.flushed_out >= 1000, "the queue went quietly: {stats:?}");
    }

    /// Before the modem has said anything, silence goes out on the packet
    /// clock -- a hole in nothing -- and is counted apart from silence sent
    /// into what the modem was saying.
    #[test]
    fn before_the_modem_speaks_silence_goes_out_on_the_packet_clock() {
        let (_far, media, _ours) = connected_to_a_quiet_far_end();
        thread::sleep(Duration::from_millis(200));
        // Put the call down and let the tick in flight finish before looking.
        media.disconnect();
        thread::sleep(Duration::from_millis(30));
        let stats = media.stats();
        assert!((8..=12).contains(&stats.packets_sent), "not one packet every 20 ms: {stats:?}");
        assert_eq!(stats.underruns, stats.packets_sent, "{stats:?}");
        assert_eq!(stats.underruns_mid_call, 0, "{stats:?}");
        assert_eq!(stats.inserted, 0, "{stats:?}");
    }

    /// The modem's audio goes the moment a packet of it is complete, and a
    /// part packet waits for the rest rather than being padded: the stream
    /// that reaches the far end is the modem's, octet for octet, with no
    /// silence between its pieces however late the second one is.
    #[test]
    fn a_part_packet_waits_for_the_rest_and_nothing_is_spliced_between() {
        let far = UdpSocket::bind("127.0.0.1:0").expect("a far end socket");
        far.set_read_timeout(Some(Duration::from_millis(300))).unwrap();
        let media = Media::open(0, NETWORK_RATE).expect("a media path");
        media.connect(far.local_addr().unwrap(), Law::Mu, g711::PCMU, 20);

        let (first, second) = (0.5, -0.5);
        media.transmit_exact(&[first; 100]);
        thread::sleep(Duration::from_millis(160));
        media.transmit_exact(&[second; 160]);
        thread::sleep(Duration::from_millis(60));

        let mut stream = Vec::new();
        let mut datagram = [0u8; 2048];
        while let Ok(n) = far.recv(&mut datagram) {
            if let Some(packet) = rtp::Packet::parse(&datagram[..n]) {
                stream.extend_from_slice(&packet.payload);
            }
            if stream.len() > 160 * 30 {
                break;
            }
        }
        let code = |level: f32| {
            let mut out = Vec::new();
            g711::encode_into(Law::Mu, &[level], &mut out);
            out[0]
        };
        let (a, b) = (code(first), code(second));
        let start = stream.iter().position(|&o| o == a).expect("the first part never went out");
        assert_eq!(start % 160, 0, "the first part did not start a packet");
        assert!(stream[start..start + 100].iter().all(|&o| o == a), "the first part was cut up");
        assert!(stream[start + 100..start + 160].iter().all(|&o| o == b), "silence was spliced between the two parts");
        let stats = media.stats();
        assert_eq!((stats.inserted, stats.underruns_mid_call), (0, 0), "{stats:?}");
    }

    /// A modem that really has stopped -- a hold, the end of a call -- gets
    /// silence sent in its place after [`DRY_LIMIT`], and every octet of it
    /// is counted as a splice.
    #[test]
    fn a_modem_that_has_stopped_has_silence_sent_and_counted() {
        let (_far, media, _ours) = connected_to_a_quiet_far_end();
        // A packet and a part: the packet goes, the part waits for more.
        media.transmit_exact(&[0.5; 260]);
        thread::sleep(DRY_LIMIT + Duration::from_millis(200));
        let stats = media.stats();
        assert!(stats.underruns_mid_call >= 2, "{stats:?}");
        assert_eq!(
            stats.inserted,
            60 + 160 * (stats.underruns_mid_call - 1),
            "the part packet's padding and every packet after it: {stats:?}"
        );
    }

    /// A pseudo-random mu-law codeword that is never silence (0xFF) or the
    /// other zero (0x7F), so it survives decoding and encoding again
    /// unchanged and no stretch of the modem's audio can be taken for the
    /// silence around it.
    fn codeword(state: &mut u32) -> u8 {
        loop {
            *state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let c = (*state >> 24) as u8;
            if c != 0xFF && c != 0x7F {
                return c;
            }
        }
    }

    /// When each inbound packet reaches us, with jitter, two bursts of loss
    /// and three stalls of 150, 300 and 100 ms; packets are numbered from 0.
    fn impaired_arrivals(packets: u16) -> Vec<(u64, u16)> {
        let stalls = [(600u64, 150u64), (1300, 300), (2200, 100)];
        let lost = |seq: u16| (30..=32).contains(&seq) || (100..=103).contains(&seq);
        let mut state = 7u32;
        let mut arrivals: Vec<(u64, u16)> = (0..packets)
            .filter(|&seq| !lost(seq))
            .map(|seq| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let mut at = u64::from(seq) * 20 + u64::from(state >> 27);
                for (from, long) in stalls {
                    if (from..from + long).contains(&at) {
                        at = from + long;
                    }
                }
                (at, seq)
            })
            .collect();
        arrivals.sort();
        arrivals
    }

    /// What this crate exists for, end to end through real sockets: the far
    /// end's audio arrives with jitter, bursts of loss and stalls of 100 to
    /// 300 ms; the modem is stepped on what `receive` hands over and says one
    /// octet for each; and what reaches the far end is exactly what the modem
    /// said, sample for sample, behind a fixed delay. Before, every stall
    /// past a 60 ms cushion put whole packets of silence into it.
    #[test]
    fn inbound_jitter_loss_and_stalls_never_reach_what_the_modem_sends() {
        let far = UdpSocket::bind("127.0.0.1:0").expect("a far end socket");
        far.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
        let media = Media::open_with_jitter(0, NETWORK_RATE, TEST_JITTER).expect("a media path");
        let ours = SocketAddr::from(([127, 0, 0, 1], media.local_port()));
        media.connect(far.local_addr().unwrap(), Law::Mu, g711::PCMU, 20);

        let listener = far.try_clone().unwrap();
        let done = Arc::new(AtomicBool::new(false));
        let heard_by_far = {
            let done = Arc::clone(&done);
            thread::spawn(move || {
                let mut packets = Vec::new();
                let mut datagram = [0u8; 2048];
                while !done.load(Ordering::Relaxed) {
                    if let Ok(n) = listener.recv(&mut datagram)
                        && let Some(packet) = rtp::Packet::parse(&datagram[..n])
                    {
                        packets.push(packet);
                    }
                }
                packets
            })
        };

        let packets = 160u16;
        let arrivals = impaired_arrivals(packets);
        let mut next_arrival = 0;
        let mut state = 1u32;
        let mut said: Vec<u8> = Vec::new();
        let (mut from_line, mut levels) = (Vec::new(), Vec::new());
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(u64::from(packets) * 20 + 400) {
            let ms = start.elapsed().as_millis() as u64;
            while next_arrival < arrivals.len() && arrivals[next_arrival].0 <= ms {
                let seq = arrivals[next_arrival].1;
                send_rtp(&far, ours, seq, g711::PCMU, vec![(seq % 250) as u8; 160]);
                next_arrival += 1;
            }
            from_line.clear();
            media.receive(&mut from_line);
            // The modem: one octet out for every sample in.
            let codes: Vec<u8> = from_line.iter().map(|_| codeword(&mut state)).collect();
            levels.clear();
            g711::decode_into(Law::Mu, &codes, &mut levels);
            media.transmit_exact(&levels);
            said.extend_from_slice(&codes);
            thread::sleep(Duration::from_millis(2));
        }
        thread::sleep(Duration::from_millis(80));
        done.store(true, Ordering::Relaxed);
        let mut sent = heard_by_far.join().unwrap();
        let stats = media.stats();

        // The inbound side really was impaired, and the modem heard it.
        assert!(stats.jitter.starved > 0 && stats.jitter.late > 0, "no stall reached the modem: {stats:?}");
        assert!(stats.jitter.lost >= 3, "the loss bursts were not concealed: {stats:?}");
        // The modem was stepped on our clock throughout, stalls and all.
        let elapsed = start.elapsed().as_secs_f64();
        assert!(said.len() as f64 > (elapsed - 0.5) * NETWORK_RATE, "the modem was not stepped through the stalls: {} samples in {elapsed:.2} s", said.len());

        // Every packet arrived, in order, with continuous numbering and
        // timestamps.
        let base = sent.first().expect("nothing reached the far end").sequence;
        sent.sort_by_key(|p| p.sequence.wrapping_sub(base));
        for pair in sent.windows(2) {
            assert_eq!(pair[1].sequence, pair[0].sequence.wrapping_add(1), "a packet is missing");
            assert_eq!(pair[1].timestamp.wrapping_sub(pair[0].timestamp), 160);
        }
        let stream: Vec<u8> = sent.iter().flat_map(|p| p.payload.iter().copied()).collect();
        let quiet = Law::Mu.encode(0);
        let first = stream.iter().position(|&o| o != quiet).expect("the modem never reached the far end");
        assert_eq!(first % 160, 0, "the modem's audio did not start a packet");
        let carried = &stream[first..];
        assert!(carried.len() + 2 * 160 >= said.len(), "{} of {} octets the modem said went out", carried.len(), said.len());
        let n = carried.len().min(said.len());
        if let Some(at) = (0..n).find(|&i| carried[i] != said[i]) {
            panic!(
                "what went out parted from what the modem said at octet {at} ({:.0} ms in): {} octets of silence next\n{stats:?}",
                at as f64 / 8.0,
                carried[at..].iter().take_while(|&&o| o == quiet).count()
            );
        }
        assert_eq!((stats.underruns_mid_call, stats.inserted, stats.dropped_out), (0, 0, 0), "{stats:?}");
    }

    /// The tail of a call is counted on the way out rather than going
    /// quietly, because "nothing was concealed" is only the whole truth if
    /// nothing was discarded either.
    #[test]
    fn the_tail_thrown_away_at_a_disconnect_is_counted() {
        let (_far, media, _ours) = connected_to_a_quiet_far_end();
        media.transmit(&[0.5; 1600]);
        media.disconnect();
        let stats = media.stats();
        assert!(stats.flushed_out >= 1000, "the tail of the call went quietly: {stats:?}");
    }

    /// One packet with a chosen SSRC, from a chosen socket.
    fn send_from(socket: &UdpSocket, to: SocketAddr, ssrc: u32, sequence: u16) {
        let packet = rtp::Packet {
            payload_type: g711::PCMU,
            marker: false,
            sequence,
            timestamp: u32::from(sequence) * 160,
            ssrc,
            payload: vec![0x7F; 160],
        };
        let mut datagram = Vec::new();
        packet.write(&mut datagram);
        socket.send_to(&datagram, to).expect("could not send");
    }

    /// Whether a packet of what the modem says reaches `socket`.
    fn hears(media: &Media, socket: &UdpSocket) -> bool {
        socket.set_read_timeout(Some(Duration::from_millis(300))).unwrap();
        let mut heard = vec![0u8; 2048];
        // Drain what was already on its way before the question was asked.
        for _ in 0..3 {
            media.transmit(&[0.0; 320]);
        }
        (0..10).any(|_| socket.recv_from(&mut heard).is_ok())
    }

    /// Anything that can reach the port can send it RTP, and a packet from
    /// anywhere used to be followed: one datagram from a stranger took the
    /// call's audio in both directions. Once a call's stream is known, a
    /// packet from elsewhere that is not that stream is dropped and counted.
    #[test]
    fn a_stranger_cannot_take_the_call_over() {
        let (far, media, ours) = connected_to_a_quiet_far_end();
        for sequence in 0..3u16 {
            send_from(&far, ours, 0x5EED_5EED, sequence);
        }
        thread::sleep(Duration::from_millis(60));

        let stranger = UdpSocket::bind("127.0.0.1:0").unwrap();
        // Another stream, and the call's own SSRC with a sequence nowhere
        // near where the call has got to.
        send_from(&stranger, ours, 0xBAD0_BAD0, 3);
        send_from(&stranger, ours, 0x5EED_5EED, 3_000);
        thread::sleep(Duration::from_millis(60));

        let stats = media.stats();
        assert_eq!(stats.strangers, 2, "{stats:?}");
        assert_eq!(stats.latched, 0, "the call was followed to a stranger: {stats:?}");
        assert_eq!(stats.packets_received, 3, "a stranger's audio went to the modem: {stats:?}");
        assert!(!hears(&media, &stranger), "the modem's audio went to the stranger");
        assert!(hears(&media, &far), "the modem's audio stopped going to the far end");
    }

    /// And the call's own stream moving -- a router behind the far end
    /// rebinding its port, say -- is still followed, because it carries on
    /// from where it was.
    #[test]
    fn the_calls_own_stream_moving_is_followed() {
        let (far, media, ours) = connected_to_a_quiet_far_end();
        for sequence in 10..13u16 {
            send_from(&far, ours, 0x5EED_5EED, sequence);
        }
        thread::sleep(Duration::from_millis(60));

        let moved = UdpSocket::bind("127.0.0.1:0").unwrap();
        for sequence in 14..17u16 {
            send_from(&moved, ours, 0x5EED_5EED, sequence);
        }
        thread::sleep(Duration::from_millis(60));

        let stats = media.stats();
        assert_eq!(stats.latched, 1, "{stats:?}");
        assert_eq!(stats.strangers, 0, "{stats:?}");
        assert_eq!(stats.packets_received, 6, "{stats:?}");
        assert!(hears(&media, &moved), "the modem's audio did not follow the stream");
    }

    /// A new call learns its stream afresh, whoever the last one's was.
    #[test]
    fn a_new_call_learns_its_stream_again() {
        let (far, media, ours) = connected_to_a_quiet_far_end();
        send_from(&far, ours, 1, 1);
        thread::sleep(Duration::from_millis(40));
        media.disconnect();

        let next = UdpSocket::bind("127.0.0.1:0").unwrap();
        media.connect(next.local_addr().unwrap(), Law::Mu, g711::PCMU, 20);
        send_from(&next, ours, 2, 500);
        thread::sleep(Duration::from_millis(40));
        let stats = media.stats();
        assert_eq!(stats.strangers, 0, "the new call's stream was taken for a stranger: {stats:?}");
        assert!(hears(&media, &next));
    }
}
