//! What data mode carried: V.42's frames, and what was in them.
//!
//! One direction's bits at a time, as a receiver in data mode hands them up.
//! They are HDLC frames if the two ends agreed on error control, and then
//! each is LAPM's: XID with what was proposed, SABME and UA, and I frames
//! with the data. With no error control they are characters with a start
//! and a stop bit. What the characters are is looked at last: PPP if it is
//! framed as PPP, and text if not.

use datapump::framing::AsyncBits;
use ec::hdlc::{Decoder, Fcs};
use ec::{Frame, Role, Xid};
use ppp::control::{Code, ConfigOption, Message};
use ppp::frame::Deframer;

use crate::model::{End, Event, Layer};
use crate::v34::Stream;

/// What one direction's frames came to.
#[derive(Debug, Clone, Default)]
pub struct Summary {
    pub frames: usize,
    pub damaged: usize,
    pub information: usize,
    pub octets: usize,
    pub xid: Option<Xid>,
    pub compressed: bool,
}

fn hex(octets: &[u8]) -> String {
    octets.iter().map(|o| format!("{o:02X}")).collect::<Vec<_>>().join(" ")
}

/// Octets as a reader would want them in a line of text.
fn printable(octets: &[u8]) -> String {
    octets
        .iter()
        .map(|&o| match o {
            b'\r' => "\\r".to_owned(),
            b'\n' => "\\n".to_owned(),
            0x20..=0x7e => (o as char).to_string(),
            _ => ".".to_owned(),
        })
        .collect()
}

fn xid_fields(xid: &Xid) -> Vec<(String, String)> {
    let mut fields = Vec::new();
    let mut add = |name: &str, value: String| fields.push((name.to_owned(), value));
    if let Some(n) = xid.n401_transmit {
        add("largest frame it sends", format!("{n} octets"));
    }
    if let Some(n) = xid.n401_receive {
        add("largest frame it takes", format!("{n} octets"));
    }
    if let Some(k) = xid.window_transmit {
        add("window it sends in", format!("{k} frames"));
    }
    if let Some(k) = xid.window_receive {
        add("window it takes", format!("{k} frames"));
    }
    add("32-bit check sequence", if xid.fcs32 { "asked for" } else { "no" }.to_owned());
    add(
        "selective reject",
        match (xid.srej_single, xid.srej_multiple) {
            (_, true) => "with a span list",
            (true, false) => "one frame at a time",
            (false, false) => "no",
        }
        .to_owned(),
    );
    add("loop-back test", if xid.test_frame { "yes" } else { "no" }.to_owned());
    match xid.compression {
        Some(way) => add(
            "V.42 bis",
            format!(
                "{way:?}, {} codewords, strings to {}",
                xid.codewords.map_or_else(|| "?".to_owned(), |c| c.to_string()),
                xid.max_string.map_or_else(|| "?".to_owned(), |c| c.to_string())
            ),
        ),
        None => add("V.42 bis", "not mentioned".to_owned()),
    }
    match &xid.v44 {
        Some(offer) => add("V.44", format!("{offer:?}")),
        None => add("V.44", "not mentioned".to_owned()),
    }
    fields
}

fn compresses(xid: &Xid) -> bool {
    xid.compression.is_some_and(|c| c != ec::Compression::Neither)
        || xid.v44.as_ref().is_some_and(|offer| offer.compression != ec::Compression::Neither)
}

/// A PPP packet in a line, and its particulars.
fn ppp_event(protocol: u16, payload: &[u8]) -> (String, String, Vec<(String, String)>) {
    use ppp::protocol as p;
    let name = match protocol {
        p::LCP => "LCP",
        p::IPCP => "IPCP",
        p::PAP => "PAP",
        p::CHAP => "CHAP",
        p::IP => "IP",
        p::COMPRESSED_TCP => "VJ TCP",
        p::UNCOMPRESSED_TCP => "VJ TCP",
        0x80fd => "CCP",
        0x8057 => "IPv6CP",
        0x0057 => "IPv6",
        _ => "PPP",
    };
    let mut fields = vec![("protocol".to_owned(), format!("{protocol:04X}"))];
    let summary = match protocol {
        p::LCP | p::IPCP | 0x80fd | 0x8057 => match Message::parse(payload) {
            Some(message) => {
                let code = match message.code {
                    Code::Unknown(n) => format!("code {n}"),
                    known => format!("{known:?}"),
                };
                fields.push(("code".to_owned(), code.clone()));
                fields.push(("identifier".to_owned(), message.id.to_string()));
                let configure = matches!(
                    message.code,
                    Code::ConfigureRequest | Code::ConfigureAck | Code::ConfigureNak | Code::ConfigureReject
                );
                if configure && let Some(options) = ConfigOption::parse_all(&message.data) {
                    for option in &options {
                        fields.push((format!("option {}", option.kind), hex(&option.value)));
                    }
                    format!("{name} {code} {}, {} options", message.id, options.len())
                } else {
                    format!("{name} {code} {}", message.id)
                }
            }
            None => format!("{name}, {} octets", payload.len()),
        },
        p::IP if payload.len() >= 20 => {
            let ip = |at: usize| format!("{}.{}.{}.{}", payload[at], payload[at + 1], payload[at + 2], payload[at + 3]);
            let carried = match payload[9] {
                1 => "ICMP".to_owned(),
                6 => "TCP".to_owned(),
                17 => "UDP".to_owned(),
                n => format!("protocol {n}"),
            };
            fields.push(("from".to_owned(), ip(12)));
            fields.push(("to".to_owned(), ip(16)));
            fields.push(("carries".to_owned(), carried.clone()));
            format!("IP {} to {}, {carried}, {} octets", ip(12), ip(16), payload.len())
        }
        _ => format!("{name} {protocol:04X}, {} octets", payload.len()),
    };
    (name.to_owned(), summary, fields)
}

/// Lines of text, or PPP packets, out of the octets one direction carried.
struct Carried {
    who: End,
    deframer: Deframer,
    /// Whether anything framed as PPP has been seen.
    ppp: bool,
    line: Vec<u8>,
    line_from: f64,
    events: Vec<Event>,
}

impl Carried {
    fn new(who: End) -> Self {
        Self { who, deframer: Deframer::new(), ppp: false, line: Vec::new(), line_from: 0.0, events: Vec::new() }
    }

    fn feed(&mut self, octets: &[u8], start: f64, end: f64) {
        for &octet in octets {
            if let Ok(Some(packet)) = self.deframer.feed(octet) {
                self.ppp = true;
                self.line.clear();
                let (tag, summary, fields) = ppp_event(packet.protocol, &packet.payload);
                self.events.push(
                    Event::new(start, end, self.who, Layer::Payload, tag)
                        .summary(summary)
                        .fields(fields)
                        .field("octets", hex(&packet.payload))
                        .octets(&packet.payload),
                );
            }
            if self.ppp {
                continue;
            }
            if self.line.is_empty() {
                self.line_from = start;
            }
            self.line.push(octet);
            if octet == b'\n' || self.line.len() >= 160 {
                self.flush(end);
            }
        }
    }

    fn flush(&mut self, end: f64) {
        if self.line.is_empty() || self.ppp {
            self.line.clear();
            return;
        }
        let line = std::mem::take(&mut self.line);
        // A PPP flag at the front is not text, and not yet a packet either.
        if line.iter().all(|&o| o == 0x7e) {
            return;
        }
        self.events.push(
            Event::new(self.line_from, end.max(self.line_from), self.who, Layer::Payload, "text")
                .summary(printable(&line))
                .field("text", printable(&line))
                .field("octets", hex(&line))
                .octets(&line),
        );
    }
}

/// One direction's bits, read.
pub fn read(stream: &Stream) -> (Vec<Event>, Summary) {
    let mut events = Vec::new();
    let mut summary = Summary::default();
    let Some(who) = stream.from else { return (events, summary) };
    if stream.bits.is_empty() {
        return (events, summary);
    }
    // A frame sent by the caller is one the answerer receives, and the
    // command and response bit is the receiver's to read.
    let receiver = if who == End::Caller { Role::Answerer } else { Role::Originator };
    let mut hdlc = Decoder::new(Fcs::Bits16);
    hdlc.accept_either();
    let mut carried = Carried::new(who);
    let mut expected: Option<u8> = None;
    let mut last_end = 0usize;

    for (index, &bit) in stream.bits.iter().enumerate() {
        let Some(frame) = hdlc.feed(bit) else { continue };
        let body = match frame {
            Ok(body) => body,
            Err(_) => {
                summary.damaged += 1;
                last_end = index;
                continue;
            }
        };
        // Its bits, stuffing and flags and check sequence and all, back
        // from where it ended; but not into the frame before it.
        let length = (body.len() + 4) * 8 + body.len() / 4;
        let first = index.saturating_sub(length).max(last_end);
        last_end = index;
        let (start, end) = (stream.time_of(first), stream.time_of(index));
        let Ok((_, frame)) = Frame::decode(&body, receiver) else {
            summary.damaged += 1;
            continue;
        };
        summary.frames += 1;
        let event = |tag: &str| Event::new(start, end, who, Layer::Link, tag).field("octets", hex(&body)).octets(&body);
        let flag = |on: bool| if on { "yes" } else { "no" }.to_owned();
        let made = match &frame {
            Frame::I { ns, nr, poll, info } => {
                summary.information += 1;
                // A frame sent again is not more data.
                let fresh = expected.is_none_or(|want| want == *ns);
                if fresh {
                    expected = Some((ns + 1) % 128);
                    summary.octets += info.len();
                    if !summary.compressed {
                        carried.feed(info, start, end);
                    }
                }
                event("I")
                    .summary(format!(
                        "I {ns}, {} octets{}: {}",
                        info.len(),
                        if fresh { "" } else { ", sent again" },
                        printable(&info[..info.len().min(48)])
                    ))
                    .field("N(S)", ns.to_string())
                    .field("N(R)", nr.to_string())
                    .field("poll", flag(*poll))
                    .field("information", format!("{} octets", info.len()))
                    .field("as text", printable(info))
            }
            Frame::Rr { nr, pf } => event("RR").summary(format!("receive ready, next wanted {nr}")).field("N(R)", nr.to_string()).field("poll or final", flag(*pf)),
            Frame::Rnr { nr, pf } => event("RNR").summary(format!("receive not ready, next wanted {nr}")).field("N(R)", nr.to_string()).field("poll or final", flag(*pf)),
            Frame::Rej { nr, pf } => event("REJ").summary(format!("reject: send again from {nr}")).field("N(R)", nr.to_string()).field("poll or final", flag(*pf)),
            Frame::Srej { nr } => event("SREJ").summary(format!("selective reject: send {nr} again")).field("N(R)", nr.to_string()),
            Frame::Sabme { poll } => {
                expected = None;
                event("SABME").summary("set asynchronous balanced mode: the link is to start".to_owned()).field("poll", flag(*poll))
            }
            Frame::Disc { poll } => event("DISC").summary("disconnect".to_owned()).field("poll", flag(*poll)),
            Frame::Ua { final_bit } => event("UA").summary("unnumbered acknowledgement".to_owned()).field("final", flag(*final_bit)),
            Frame::Dm { final_bit } => event("DM").summary("disconnected mode".to_owned()).field("final", flag(*final_bit)),
            Frame::Frmr { final_bit, info } => event("FRMR").summary(format!("frame reject: {}", hex(info))).field("final", flag(*final_bit)),
            Frame::Ui { info, .. } => event("UI").summary(format!("unnumbered information, {} octets", info.len())),
            Frame::Test { info, .. } => event("TEST").summary(format!("loop-back test, {} octets", info.len())),
            Frame::Xid { pf, info } => match Xid::decode(info) {
                Ok(xid) => {
                    let fields = xid_fields(&xid);
                    // Compression that is named and set to neither way is
                    // a refusal, not an offer.
                    let mut what = Vec::new();
                    if xid.compression.is_some_and(|c| c != ec::Compression::Neither) {
                        what.push("V.42 bis");
                    }
                    if xid.v44.as_ref().is_some_and(|offer| offer.compression != ec::Compression::Neither) {
                        what.push("V.44");
                    }
                    summary.compressed = summary.compressed || compresses(&xid);
                    summary.xid = Some(xid);
                    event("XID")
                        .summary(format!(
                            "XID: what this end proposes for the link{}",
                            if what.is_empty() { String::new() } else { format!(", with {}", what.join(" and ")) }
                        ))
                        .field("poll or final", flag(*pf))
                        .fields(fields)
                }
                Err(error) => event("XID").summary(format!("XID that does not read: {error:?}")),
            },
        };
        events.push(made);
    }
    carried.flush(stream.time_of(stream.bits.len()));

    // No frames at all: no error control, and the bits are characters with
    // a start and a stop bit each (V.14).
    if summary.frames == 0 {
        let mut chars = AsyncBits::new(8);
        let mut plain = Carried::new(who);
        let mut any = 0usize;
        for (index, &bit) in stream.bits.iter().enumerate() {
            if let Some(octet) = chars.feed(bit) {
                any += 1;
                let at = stream.time_of(index);
                plain.feed(&[octet], at, at);
            }
        }
        plain.flush(stream.time_of(stream.bits.len()));
        if any > 0 {
            summary.octets = any;
            events.extend(plain.events);
        }
    } else {
        events.extend(carried.events);
    }
    // XIDs and their kin go round until they are answered: the same one
    // back to back is one event.
    let mut merged: Vec<Event> = Vec::new();
    for event in events {
        match merged.last_mut() {
            Some(last)
                if last.layer == Layer::Link
                    && last.tag == event.tag
                    && last.octets == event.octets
                    && event.tag != "I"
                    && event.start - last.end < 1.0 =>
            {
                last.end = event.end;
                match last.fields.iter_mut().find(|(name, _)| name == "sent") {
                    Some((_, value)) => {
                        let n: usize = value.split(' ').next().and_then(|v| v.parse().ok()).unwrap_or(1);
                        *value = format!("{} times", n + 1);
                    }
                    None => last.fields.push(("sent".to_owned(), "2 times".to_owned())),
                }
            }
            _ => merged.push(event),
        }
    }
    (merged, summary)
}
