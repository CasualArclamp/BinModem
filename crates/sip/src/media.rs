//! The audio path: an RTP socket at one end, the modem's sample rate at the
//! other, and as little as possible in between.
//!
//! Two threads and two queues. A reader thread does nothing but pull datagrams
//! off the socket and put their payloads into the jitter buffer, because a
//! thread that is doing anything else is a thread that is not reading the
//! socket, and the operating system's receive buffer is the only thing
//! standing between a scheduling hiccup and a lost packet. A pacer thread
//! sends one packet every packet time, whatever else is happening.
//!
//! The pacer is the part worth arguing about. The alternative -- send a packet
//! whenever one arrives, so the far end's clock is ours -- is tempting and is
//! what a gateway effectively does. It is rejected here because it makes our
//! transmission stop if the far end's ever does, and a far end that applies
//! silence suppression does exactly that. So: our own clock, one packet every
//! twenty milliseconds, and when there is nothing to send we send silence and
//! count it. That is the same bargain `line::Duplex` makes with the sound
//! card, for the same reason -- a far end hears a dropout, which is what error
//! control is for, where a stall would be a line that has gone away.
//!
//! Two clocks that are not the same clock is the other half of that bargain.
//! The modem produces one octet for every octet it is handed, so the outgoing
//! queue is filled by the far end's sending clock and emptied by ours, and
//! nothing makes the two agree. What stands between them is the cushion:
//! [`OUTGOING_CUSHION`] packets put in front of the stream when the call comes
//! up, and put back when something has spent them. The putting back is the
//! part that was missing. A scheduling hiccup, a late arrival, a
//! resynchronisation in `jitter` that steps the stream on without handing over
//! any octets -- each takes a packet out of the cushion, and with nothing to
//! return it the first wobble of a call leaves every later tick one packet
//! short. That is not a worry, it is a measurement: 38 packets of silence, 760
//! ms of it, transmitted into a live V.8 handshake that then failed to train,
//! on a call whose inbound side had lost one packet in total.
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

/// How much audio may queue up waiting to go out before the oldest is thrown
/// away, in packets. Anything waiting here is delay added to a path that
/// already has about 750 ms of it, so the limit is small on purpose: a queue
/// that has grown past this is not going to drain, and keeping it would only
/// make the round trip worse for the rest of the call.
const OUTGOING_LIMIT: usize = 25;

/// Packets of silence the outgoing queue is primed with when a call comes up,
/// and the depth the pacer puts it back to once it has been spent.
///
/// Three, which is 60 ms. The pacer's tick is a `thread::sleep` on a machine
/// whose timer granularity is about 15 ms, so one tick arriving late is
/// ordinary and two in a row is not rare; three packets covers both and leaves
/// one in hand for the far end's clock being a little faster than ours. One
/// packet, which is what this was, covers none of them: the first wobble of
/// the call spends it and the rest of the call runs with no margin at all.
///
/// It is bought with delay, and delay is the thing this path can least afford
/// -- about 750 ms each way before anything here is counted. Sixty
/// milliseconds is eight per cent on top of that, paid once at the start of
/// the call and again only when the cushion has actually been spent. That is
/// the whole of the trade: 60 ms of one-way delay against a 20 ms hole in the
/// transmitted signal every time the two clocks slip past each other.
const OUTGOING_CUSHION: usize = 3;

/// Ticks in a row that must find less than a whole packet waiting before the
/// cushion is put back.
///
/// Three, to tell a slip from a pause. One short tick is the two clocks
/// beating against each other and is over by the next one; three in a row is
/// the cushion gone. Waiting for three also means the silence goes in at the
/// end of a stretch the far end has already spent hearing silence, which is
/// where it costs least: it lengthens a gap that is already there instead of
/// cutting a new one into the middle of a symbol.
const DRY_BEFORE_REBUILD: usize = 3;

/// Packets of delay the jitter buffer holds before it hands anything over.
///
/// Two, which is 40 ms. Enough to put back in order the pair-swaps and small
/// late arrivals that an ordinary path produces, and short enough that it is
/// nothing beside the delay already in the line. A telephone would use ten
/// times this and stretch the audio to manage it; both halves of that are
/// wrong for a modem.
const JITTER_TARGET: usize = 2;
/// And how many it will hold before shedding, which is where a far end whose
/// clock runs fast ends up.
const JITTER_CAPACITY: usize = 50;

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
    /// has arrived and been through the jitter buffer -- the modem is clocked
    /// by the line, so until then it has not been stepped and has nothing to
    /// say, and the silence we send is not a hole in anything. Counting the
    /// two together made a healthy call read as a failing one and buried the
    /// number that matters: 38 gaps sent looks the same either way, and only
    /// this one of them says the handshake was being cut up while it ran.
    ///
    /// A hold counts here, for what it is worth: `line::Line` stops handing
    /// the modem's samples over while the far end says it is not listening,
    /// and from down here that is a modem that has stopped producing.
    pub underruns_mid_call: u64,
    /// Ticks where the pacer sent nothing at all, because the queue was a few
    /// octets short of a whole packet and waiting one tick was better than
    /// padding the packet out.
    ///
    /// Padding splices silence into the middle of a symbol, which is a phase
    /// step in the middle of the far end's equaliser. Waiting moves the same
    /// audio 20 ms later with no seam in it: the RTP sequence and timestamp
    /// are only advanced by packets we actually send, so the far end sees one
    /// late packet rather than a gap, and the stream stays sample-continuous.
    /// Never twice in a row -- two would be the beginning of a stall, and a
    /// stall is the one thing this pacer must not do.
    pub deferred: u64,
    /// Octets of silence put into the outgoing stream on purpose: the cushion
    /// at the start of a call, and what was put back after a wobble spent it.
    ///
    /// It is silence the far end hears, so it is counted like any other.
    /// [`OUTGOING_CUSHION`] packets of it go in at the start of every call,
    /// where nothing is cut into because the modem has not started; the rest
    /// went in during a call, at the end of a stretch of underruns, and is
    /// margin bought back at the price of that much more delay in the path.
    pub cushioned: u64,
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
    deferred: AtomicU64,
    cushioned: AtomicU64,
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
    codewords: Vec<u8>,
    at_network_rate: Vec<f32>,
    scratch: Vec<f64>,
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
            incoming: Mutex::new(Jitter::new(JITTER_TARGET, JITTER_CAPACITY)),
            payload_type: AtomicU8::new(g711::PCMU),
            packet_octets: AtomicU64::new(160),
            running: AtomicBool::new(true),
            connected: AtomicBool::new(false),
            spoke: AtomicBool::new(false),
            packets_sent: AtomicU64::new(0),
            packets_received: AtomicU64::new(0),
            ignored: AtomicU64::new(0),
            off_codec: AtomicU64::new(0),
            underruns: AtomicU64::new(0),
            underruns_mid_call: AtomicU64::new(0),
            deferred: AtomicU64::new(0),
            cushioned: AtomicU64::new(0),
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
                codewords: Vec::with_capacity(2048),
                at_network_rate: Vec::with_capacity(2048),
                scratch: Vec::with_capacity(64),
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
        {
            return;
        }

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

            // The cushion, which is the whole of this crate's answer to a
            // question worth stating.
            //
            // The modem only produces samples when a packet arrives, so the
            // queue starts empty and the pacer's phase against the far end's
            // arrivals is whatever it happens to be. Without this, any
            // wobble in the inbound direction leaves the queue empty at the
            // moment the pacer looks, and 20 ms of invented silence goes out
            // -- honestly counted as an `underrun`, but an underrun that
            // nothing here did wrong. `underruns` then reads non-zero on a
            // healthy call and means "the modem loop is not keeping up",
            // which is the opposite of the truth, and the counter stops being
            // worth looking at. A cushion removes the whole class, at
            // OUTGOING_CUSHION packets of one-way delay, and the octets it
            // sends are the same silence an underrun would have sent -- the
            // difference is that these are ours, sent on purpose and counted
            // as such, and a later underrun then means what it says.
            //
            // The pacer puts it back when it has been spent; see `pace`. This
            // is only where it starts.
            let quiet = Law::from_payload_type(payload_type).unwrap_or(Law::Mu).encode(0);
            let primed = octets as usize * OUTGOING_CUSHION;
            for _ in 0..primed {
                queue.push_back(quiet);
            }
            self.shared
                .cushioned
                .fetch_add(primed as u64, Ordering::Relaxed);
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

    /// Everything that has arrived, at the modem's rate.
    ///
    /// This is the line's clock, exactly as the input device is for a sound
    /// card: samples arrive, the modem is stepped once for each, and what it
    /// says goes back. Nothing here paces itself, because the far end's
    /// packets already do.
    pub fn receive(&self, into: &mut Vec<f32>) {
        let law = self.law();
        let Ok(mut conversion) = self.conversion.lock() else {
            return;
        };
        let conversion = &mut *conversion;
        conversion.codewords.clear();
        let quiet = law.encode(0);
        if let Ok(mut jitter) = self.shared.incoming.lock() {
            jitter.drain(&mut conversion.codewords, quiet);
        }
        if conversion.codewords.is_empty() {
            return;
        }
        conversion.at_network_rate.clear();
        g711::decode_into(law, &conversion.codewords, &mut conversion.at_network_rate);
        for &sample in conversion.at_network_rate.iter() {
            conversion.scratch.clear();
            conversion.up.process(f64::from(sample), &mut conversion.scratch);
            into.extend(conversion.scratch.iter().map(|s| *s as f32));
        }
    }

    /// Samples for the line, at the modem's rate.
    pub fn transmit(&self, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }
        let law = self.law();
        let Ok(mut conversion) = self.conversion.lock() else {
            return;
        };
        let conversion = &mut *conversion;
        conversion.at_network_rate.clear();
        for &sample in samples {
            conversion.scratch.clear();
            conversion
                .down
                .process(f64::from(sample), &mut conversion.scratch);
            conversion
                .at_network_rate
                .extend(conversion.scratch.iter().map(|s| *s as f32));
        }
        conversion.codewords.clear();
        g711::encode_into(law, &conversion.at_network_rate, &mut conversion.codewords);

        let limit = self.shared.packet_octets.load(Ordering::Relaxed) as usize * OUTGOING_LIMIT;
        let Ok(mut queue) = self.shared.outgoing.lock() else {
            return;
        };
        if !conversion.codewords.is_empty() {
            // The modem has said something on this call, so an underrun from
            // here on is a hole in what it was saying rather than the quiet
            // before it started.
            self.shared.spoke.store(true, Ordering::Relaxed);
        }
        for &code in conversion.codewords.iter() {
            if queue.len() >= limit {
                queue.pop_front();
                self.shared.dropped_out.fetch_add(1, Ordering::Relaxed);
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
            law: Some(self.law().encoding_name()),
            packets_sent: self.shared.packets_sent.load(Ordering::Relaxed),
            packets_received: self.shared.packets_received.load(Ordering::Relaxed),
            ignored: self.shared.ignored.load(Ordering::Relaxed),
            off_codec: self.shared.off_codec.load(Ordering::Relaxed),
            underruns: self.shared.underruns.load(Ordering::Relaxed),
            underruns_mid_call: self.shared.underruns_mid_call.load(Ordering::Relaxed),
            deferred: self.shared.deferred.load(Ordering::Relaxed),
            cushioned: self.shared.cushioned.load(Ordering::Relaxed),
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
    let mut buffer = vec![0u8; 2048];
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
        if let Ok(mut jitter) = shared.incoming.lock() {
            jitter.push(packet.sequence, packet.payload_type, packet.payload);
        }
    }
}

/// The pacer: one packet every packet time, silence when there is nothing.
fn pace(shared: Arc<Shared>, socket: UdpSocket) {
    let mut stream: Option<rtp::Stream> = None;
    let mut datagram = Vec::with_capacity(256);
    let mut payload: Vec<u8> = Vec::with_capacity(256);
    let mut first = true;
    let mut due = Instant::now();
    // Ticks in a row that have not found a whole packet waiting, which is what
    // says the cushion has been spent rather than merely dipped into.
    let mut dry = 0;
    // Whether the tick before this one waited rather than sending. At most one
    // in a row: see `Stats::deferred`.
    let mut waited = false;

    while shared.running.load(Ordering::Relaxed) {
        let octets = shared.packet_octets.load(Ordering::Relaxed) as usize;
        let interval = Duration::from_secs_f64(octets as f64 / NETWORK_RATE);
        let now = Instant::now();
        if now < due {
            thread::sleep((due - now).min(Duration::from_millis(20)));
            continue;
        }
        due += interval;
        if due < now {
            // Badly behind -- the machine was busy elsewhere. Start again
            // from now rather than sending a burst to catch up: a burst
            // arrives at the far end as jitter, which is the thing this whole
            // crate exists to avoid inflicting.
            due = now + interval;
        }

        if !shared.connected.load(Ordering::Relaxed) {
            stream = None;
            first = true;
            dry = 0;
            waited = false;
            continue;
        }
        let Some(remote) = shared.remote.lock().ok().and_then(|r| *r) else {
            continue;
        };
        let payload_type = shared.payload_type.load(Ordering::Relaxed);
        // Silence in the law in force, so the far end hears a quiet line
        // rather than a click.
        let quiet = Law::from_payload_type(payload_type).unwrap_or(Law::Mu).encode(0);
        let sender = stream.get_or_insert_with(|| rtp::Stream::new(payload_type));

        payload.clear();
        let mut ran_dry = false;
        let mut wait = false;
        if let Ok(mut queue) = shared.outgoing.lock() {
            if dry >= DRY_BEFORE_REBUILD && !queue.is_empty() {
                // The cushion has been spent and the modem is producing
                // again: put it back, in front of what it has just produced.
                //
                // Here rather than at the moment it was spent, for two
                // reasons. The far end has just had `DRY_BEFORE_REBUILD`
                // packets of silence from the ticks before this one, so this
                // lengthens a gap it is already in the middle of instead of
                // cutting a new one; and a queue that is still empty is a
                // modem that is not producing, where a cushion would be spent
                // again on the next tick and every tick after it, for
                // nothing. Topping up to a depth rather than adding a fixed
                // amount is what bounds it: however long the dry stretch was,
                // the stream ends up `OUTGOING_CUSHION` packets in front of
                // the pacer and no further, so the delay this can add to a
                // call is the cushion and not the sum of its wobbles.
                let want = octets * OUTGOING_CUSHION;
                let put = want.saturating_sub(queue.len());
                for _ in 0..put {
                    queue.push_front(quiet);
                }
                shared.cushioned.fetch_add(put as u64, Ordering::Relaxed);
                dry = 0;
            }

            if queue.len() >= octets {
                payload.extend(queue.drain(..octets));
                dry = 0;
            } else if !waited && queue.len() + octets / 4 >= octets {
                // A few octets short of a packet: the two clocks have drifted
                // past each other by less than a quarter of a packet time.
                // Wait one tick rather than pad, and the shortfall is gone for
                // the rest of the call -- next tick has this packet and the
                // one the modem produced meanwhile, so the queue is a whole
                // packet deeper at every tick from here on. It costs 20 ms of
                // delay and no silence at all, which is the cheapest cushion
                // there is.
                wait = true;
            } else {
                // The modem has not produced this packet's worth. What there
                // is goes out with silence behind it.
                payload.extend(queue.drain(..));
                payload.resize(octets, quiet);
                ran_dry = true;
                dry += 1;
            }
        }
        waited = wait;
        if wait {
            shared.deferred.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        if ran_dry {
            shared.underruns.fetch_add(1, Ordering::Relaxed);
            if shared.spoke.load(Ordering::Relaxed) {
                shared.underruns_mid_call.fetch_add(1, Ordering::Relaxed);
            }
        }

        sender.next(&payload, first, &mut datagram);
        first = false;
        if socket.send_to(&datagram, remote).is_ok() {
            shared.packets_sent.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Opening binds a port and hands it back, which is what the SDP offer
    /// needs before there is any call to offer.
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
        let media = Media::open(0, NETWORK_RATE).unwrap();
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
        // Long enough for the reader to have taken them all.
        thread::sleep(Duration::from_millis(100));
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
        let media = Media::open(0, NETWORK_RATE).expect("a media path");
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
        for sequence in 3..6u16 {
            send_rtp(&far, ours, sequence, g711::PCMU, vec![0x7F; 160]);
        }
        thread::sleep(Duration::from_millis(150));

        let mut out = Vec::new();
        media.receive(&mut out);
        assert!(!out.is_empty(), "no audio came through at all");

        let stats = media.stats();
        assert_eq!(stats.off_codec, 1, "the event packet was decoded as audio: {stats:?}");
        assert_eq!(stats.packets_received, 5, "the event was counted as audio: {stats:?}");
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

    /// The outgoing queue starts with the cushion in it, so the first packets
    /// of the call are ones we meant to send.
    ///
    /// Everything after it here is an underrun because this test never hands
    /// the modem's side anything, and that is the point: exactly
    /// `OUTGOING_CUSHION` packets of this call came out of the queue. Without
    /// the priming that number is zero, the pacer's phase against the far
    /// end's arrivals is arbitrary, and any inbound wobble at all turns into
    /// an `underrun` -- which reads as "the modem loop is not keeping up" on a
    /// call where nothing is wrong.
    #[test]
    fn the_outgoing_queue_is_primed_so_the_first_packets_are_not_invented() {
        let (_far, media, _ours) = connected_to_a_quiet_far_end();
        thread::sleep(Duration::from_millis(200));
        let stats = media.stats();
        assert!(stats.packets_sent >= 4, "the pacer sent nothing: {stats:?}");
        assert_eq!(
            stats.packets_sent - stats.underruns,
            OUTGOING_CUSHION as u64,
            "the queue was not primed with {OUTGOING_CUSHION} packets: {stats:?}"
        );
        // Counted, because it is silence the far end hears like any other.
        assert_eq!(stats.cushioned, 160 * OUTGOING_CUSHION as u64, "{stats:?}");
        // And nothing was put back on top of it: there is no modem here for a
        // cushion to go in front of.
        assert_eq!(stats.underruns_mid_call, 0, "{stats:?}");
    }

    /// The cushion is put back after it has been spent, which is the half of
    /// this that was missing.
    ///
    /// What went wrong: the queue was primed once at `connect` and nothing
    /// ever refilled it. The modem produces one octet for every octet it is
    /// handed, so the cushion cannot grow back by itself -- every wobble took
    /// a packet out of it and nothing put one back, and from the first wobble
    /// onwards every slip between the far end's clock and the pacer's sent
    /// another 20 ms of silence into the call. On the call this was found on
    /// that came to 38 packets, 760 ms of silence, transmitted into a V.8
    /// handshake that then failed to train.
    #[test]
    fn a_cushion_spent_while_the_modem_was_quiet_is_put_back_when_it_speaks() {
        let (_far, media, _ours) = connected_to_a_quiet_far_end();
        let primed = 160 * OUTGOING_CUSHION as u64;

        // Long enough for the cushion to have gone out and for the pacer to
        // have been dry for several ticks after it.
        thread::sleep(Duration::from_millis(200));
        let spent = media.stats();
        assert!(
            spent.underruns >= DRY_BEFORE_REBUILD as u64,
            "the pacer never ran dry: {spent:?}"
        );
        assert_eq!(
            spent.cushioned, primed,
            "silence was put back before anything had been spent: {spent:?}"
        );

        // And now the modem says something: one packet's worth, which is what
        // one arriving packet is worth on the way back.
        media.transmit(&[0.5; 160]);
        thread::sleep(Duration::from_millis(100));

        let after = media.stats();
        assert!(
            after.cushioned > primed,
            "the cushion was spent and never put back: {after:?}"
        );
        assert!(
            after.cushioned <= 2 * primed,
            "more went back in than the cushion is deep: {after:?}"
        );
    }

    /// Silence sent before the modem has said anything is not the same fault
    /// as silence sent into the middle of what it is saying, and the two are
    /// counted apart.
    ///
    /// The modem is clocked by the line: until the far end's first packet has
    /// arrived and been through the jitter buffer it has not been stepped and
    /// has nothing to say, so the silence the pacer sends in that stretch is
    /// not a hole in anything. Counted together, a healthy call reads as a
    /// failing one -- and the number that says the handshake was being cut up
    /// while it ran is buried in it.
    #[test]
    fn an_underrun_before_the_modem_speaks_is_told_from_one_that_cuts_into_it() {
        let (_far, media, _ours) = connected_to_a_quiet_far_end();
        thread::sleep(Duration::from_millis(200));
        let before = media.stats();
        assert!(before.underruns > 0, "the pacer never ran dry: {before:?}");
        assert_eq!(
            before.underruns_mid_call, 0,
            "the quiet before the call carried anything was charged to the modem: {before:?}"
        );

        // The modem produces a little and then stops, which is what a stall in
        // the loop above looks like from down here.
        media.transmit(&[0.5; 320]);
        thread::sleep(Duration::from_millis(200));

        let after = media.stats();
        assert!(
            after.underruns_mid_call > 0,
            "silence sent into the middle of the call was not counted: {after:?}"
        );
        assert!(
            after.underruns_mid_call < after.underruns,
            "the quiet at the start was counted twice: {after:?}"
        );
    }

    /// A packet a few octets short of a whole one waits a tick instead of
    /// being padded out with silence.
    ///
    /// Padding is a seam in the middle of a symbol, which is a phase step in
    /// the far end's equaliser. Waiting is the same audio 20 ms later with
    /// nothing spliced into it -- and it buys the cushion back for nothing,
    /// because the tick after the wait finds this packet and the one the modem
    /// produced meanwhile.
    #[test]
    fn a_packet_a_few_octets_short_waits_a_tick_rather_than_being_padded() {
        let far = UdpSocket::bind("127.0.0.1:0").expect("a far end socket");
        far.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let far_address = far.local_addr().unwrap();
        let media = Media::open(0, NETWORK_RATE).expect("a media path");

        // The rate conversion swallows its first half-kernel, so the first
        // block through it comes out short by however wide that is. Put that
        // block through before the call, so that what goes into the queue
        // afterwards is exactly what is asked for; `connect` clears whatever
        // this produced.
        media.transmit(&[0.0; 64]);
        media.connect(far_address, Law::Mu, g711::PCMU, 20);

        // The queue now holds a whole number of packets -- the cushion -- and
        // the pacer only ever takes whole packets out of it, so 140 octets put
        // in leaves it 140 octets past a packet boundary for as long as it
        // lasts. Three packets later the pacer finds those 140 waiting, which
        // is inside a quarter of a packet of a whole one.
        media.transmit(&[0.5; 140]);
        thread::sleep(Duration::from_millis(250));

        let stats = media.stats();
        assert_eq!(
            stats.deferred, 1,
            "the short packet was padded out with silence: {stats:?}"
        );
        // And never twice over: the tick after a wait sends what there is,
        // because waiting again is how a pacer turns into a stall.
        assert!(stats.packets_sent >= 8, "the pacer stopped sending: {stats:?}");
    }

    /// The caller's real cadence, with the far end going quiet in the middle
    /// of it: a packet in every 20 ms, ten glances at `receive` per packet
    /// interval, and every octet the modem is handed going straight back out
    /// -- which is what a modem does, one octet for one octet.
    ///
    /// The pause is the VoIP fault this crate was written for. The far end
    /// says nothing for a fifth of a second and then carries on with the same
    /// sequence numbering, so nothing was lost and the jitter buffer has
    /// nothing to conceal -- and because the modem is clocked by the line, it
    /// produces nothing for as long as the pause lasts, the outgoing queue
    /// empties, and the pacer sends silence. What must not happen is what used
    /// to: the cushion gone for good, and every later slip between the two
    /// clocks putting another 20 ms of silence into the call. That is the
    /// exact shape of the live call this came from -- one packet lost inbound,
    /// 38 packets of silence sent out.
    #[test]
    fn a_pause_at_the_far_end_does_not_cost_the_cushion_for_the_rest_of_the_call() {
        let (far, media, ours) = connected_to_a_quiet_far_end();
        let primed = 160 * OUTGOING_CUSHION as u64;

        let start = Instant::now();
        let mut sequence = 0u16;
        let mut from_line = Vec::new();
        let mut at_resume = None;
        loop {
            let ms = start.elapsed().as_millis();
            if ms >= 700 {
                break;
            }
            // The far end's clock, less the stretch where it said nothing. A
            // pause and not loss: the numbering carries straight on, so there
            // is no gap for the buffer to find.
            let owed = if ms < 200 {
                ms / 20
            } else if ms < 400 {
                10
            } else {
                (ms - 200) / 20
            };
            while u128::from(sequence) < owed {
                send_rtp(&far, ours, sequence, g711::PCMU, vec![0x7F; 160]);
                sequence += 1;
            }

            from_line.clear();
            media.receive(&mut from_line);
            media.transmit(&from_line);
            if at_resume.is_none() && ms >= 500 {
                at_resume = Some(media.stats());
            }
            thread::sleep(Duration::from_millis(2));
        }

        let end = media.stats();
        // The inbound side was clean, which is what makes this a test about
        // the outbound one.
        assert_eq!(end.jitter.lost, 0, "the loopback lost a packet: {end:?}");
        assert_eq!(end.jitter.concealed, 0, "{end:?}");
        assert!(end.packets_received >= 20, "the far end's audio never arrived: {end:?}");

        // The pause cost silence, and it is counted as silence sent into the
        // middle of the call rather than as the quiet before it started.
        assert!(end.underruns_mid_call > 0, "the pause cost nothing at all: {end:?}");
        // And the cushion was put back rather than being gone for good.
        assert!(end.cushioned > primed, "the cushion was never put back: {end:?}");

        // From well after the far end resumed to the end of the run, the line
        // is clean again: the cushion is there and the slips between the two
        // clocks fall into it instead of into the call.
        let resumed = at_resume.expect("the run never got as far as the far end resuming");
        let after = end.underruns - resumed.underruns;
        assert!(
            after <= 2,
            "the call went on sending silence after the cushion was put back: \
             {after} packets of it in the last 200 ms\n{end:?}"
        );
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
