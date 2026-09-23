//! Session descriptions: RFC 4566's syntax, and the offer/answer model of
//! RFC 3264, for a call that will only ever carry G.711.
//!
//! An ordinary softphone's SDP is a shopping list. It offers Opus, G.722,
//! GSM, G.729 and G.711, ranks them by what sounds best per kilobit, and is
//! happy with whatever comes back. This one offers two things and refuses
//! everything else, because every codec on that list except G.711 is a speech
//! coder: it fits a model of a voice tract to the input and transmits the
//! model. A modem signal is not a voice and does not survive the fitting. So
//! an answer that chose G.729 is not a call that will work slightly less well
//! -- it is a call that cannot carry a single bit -- and the honest thing to
//! do with it is to hang up and say why, rather than to connect and let the
//! modem spend forty seconds failing to train.
//!
//! That is why so much of this file is about refusing. `read_answer` returns
//! a sentence rather than a code because the thing above it will put that
//! sentence in front of a person who is wondering why the call dropped, and
//! "the far end answered with G.729" is an answer to that question in a way
//! that an error enum is not.
//!
//! The parsing is the other half. Real far ends -- Asterisk, Kamailio, a
//! wholesale trunk -- write SDP that is legal but not uniform: bare line
//! feeds instead of CRLF, `c=` at session level only or repeated per media,
//! no `a=rtpmap` at all for the static payload types RFC 3551 already fixed
//! the meaning of, and a scattering of vendor attributes nobody outside the
//! vendor has ever read. None of that is a reason to fail a call. So parsing
//! keeps what it does not understand and complains only when something it
//! actually needs is missing or contradicts itself.

use std::fmt;

use crate::g711::Law;

/// The MIME type an SDP body is carried as (RFC 4566 5).
pub const CONTENT_TYPE: &str = "application/sdp";

/// Twenty milliseconds: one packet per 160 samples, which is what every
/// telephone network device expects and what our own offer asks for.
pub const DEFAULT_PTIME_MS: u32 = 20;

/// A modem wants small packets. Every packet is a packet's worth of added
/// delay in each direction before its contents can be looked at, and this rig
/// already carries about 750 ms one way to the far end and back; V.32's echo
/// canceller and V.34's half-duplex phases both have opinions about that.
/// Below 10 ms the per-packet overhead starts to matter more than the delay
/// saved, and above 40 ms the far end is adding delay we cannot afford, so a
/// stated ptime outside this range is taken as a preference rather than an
/// instruction and clamped.
const MIN_PTIME_MS: u32 = 10;
const MAX_PTIME_MS: u32 = 40;

/// The direction a stream runs, from RFC 4566 6.7's four attributes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    SendReceive,
    SendOnly,
    ReceiveOnly,
    Inactive,
}

impl Direction {
    /// The same stream seen from the other end.
    ///
    /// This is the one place in offer/answer where copying the far end's words
    /// gives the wrong answer. `a=sendonly` in a description we received is a
    /// statement about the sender: *it* will send and will not listen. For us
    /// that is receive-only, and a rig that reads it as send-only will
    /// cheerfully transmit into an ear that was never listening -- which on a
    /// modem call looks exactly like a far end that has gone deaf.
    pub fn flipped(self) -> Self {
        match self {
            Self::SendReceive => Self::SendReceive,
            Self::SendOnly => Self::ReceiveOnly,
            Self::ReceiveOnly => Self::SendOnly,
            Self::Inactive => Self::Inactive,
        }
    }

    /// How it is written in an `a=` line.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SendReceive => "sendrecv",
            Self::SendOnly => "sendonly",
            Self::ReceiveOnly => "recvonly",
            Self::Inactive => "inactive",
        }
    }
}

/// One `m=` line and everything that belongs to it (RFC 4566 5.14).
#[derive(Debug, Clone)]
pub struct MediaLine {
    /// "audio", "image", "video": the media type, as written.
    pub kind: String,
    /// The port RTP is to be sent to. Zero is not a port: RFC 3264 6 gives it
    /// the separate meaning "this stream is rejected", and a far end declining
    /// an offer says so by answering the same `m=` line with port 0 rather
    /// than by leaving it out.
    pub port: u16,
    /// "RTP/AVP", "udptl", "RTP/SAVP" and so on.
    pub protocol: String,
    /// The payload type numbers, as written. Kept as text because for a
    /// non-RTP transport -- `udptl` -- the format is a name such as `t38` and
    /// not a number at all.
    pub formats: Vec<String>,
    /// A media-level `c=` line's value, if this stream had one of its own.
    /// Stored as written ("IN IP4 203.0.113.11"); use [`Sdp::address_for`] to
    /// get the address out of it with the session-level fallback applied.
    pub connection: Option<String>,
    /// The `a=` lines, in order, with their values where they had one.
    pub attributes: Vec<(String, Option<String>)>,
}

impl MediaLine {
    /// The value of the first `a=<name>:<value>` with this name, the name
    /// compared without regard to case (RFC 4566 5.13 makes attribute names
    /// case-insensitive, and far ends vary: `a=ptime` and `a=PTime` both
    /// arrive).
    ///
    /// Attributes of the same name with no value are skipped rather than
    /// ending the search, so a property-form attribute written before a
    /// value-form one of the same name does not hide it.
    pub fn attribute(&self, name: &str) -> Option<&str> {
        find_attribute(&self.attributes, name)
    }

    /// Whether the attribute is present at all, with or without a value. This
    /// is how the property-form attributes are asked about: "sendonly",
    /// "rtcp-mux", "inactive".
    pub fn has_attribute(&self, name: &str) -> bool {
        any_attribute(&self.attributes, name)
    }

    /// The `a=rtpmap` body for a payload type -- the "PCMU/8000" part -- if
    /// one was declared. Absence is not an error and is not unusual: RFC 3551
    /// 3 fixes the static numbers, so a far end need never map them.
    pub fn rtpmap(&self, payload_type: u8) -> Option<&str> {
        for (name, value) in &self.attributes {
            if !name.eq_ignore_ascii_case("rtpmap") {
                continue;
            }
            let Some(value) = value.as_deref() else {
                continue;
            };
            let value = value.trim();
            let Some((number, rest)) = value.split_once(char::is_whitespace) else {
                continue;
            };
            if number.trim().parse::<u8>() == Ok(payload_type) {
                return Some(rest.trim());
            }
        }
        None
    }

    /// The direction this stream runs, as the end that wrote it sees it.
    pub fn direction(&self) -> Direction {
        // RFC 4566 6.7 means these four to be exclusive, so a description with
        // two of them is already wrong and the only question is which way to
        // be wrong back. The most restrictive reading is the safe one: a
        // stream we wrongly believe is idle wastes a call, a stream we wrongly
        // believe is live puts our transmitter on top of somebody else's.
        if self.has_attribute("inactive") {
            Direction::Inactive
        } else if self.has_attribute("sendonly") {
            Direction::SendOnly
        } else if self.has_attribute("recvonly") {
            Direction::ReceiveOnly
        } else {
            // 6.7's default when none is written, which is the common case:
            // most far ends only say anything when putting a call on hold.
            Direction::SendReceive
        }
    }
}

/// A whole session description.
///
/// Only the fields this modem acts on are pulled apart. `o=` is kept as one
/// string rather than as five because nothing here reads its pieces -- but see
/// [`Sdp::origin_version`], which the layer above needs when it has to decide
/// whether a re-INVITE carries a new description or is just a session refresh.
#[derive(Debug, Clone)]
pub struct Sdp {
    /// The `o=` line's value, as written.
    pub origin: String,
    /// `s=`. Carries nothing a call depends on; kept so a description written
    /// back out looks like the one that arrived.
    pub session_name: String,
    /// The session-level `c=`, as written. Plenty of far ends send this and no
    /// media-level `c=` at all.
    pub connection: Option<String>,
    /// Session-level `a=` lines.
    pub attributes: Vec<(String, Option<String>)>,
    pub media: Vec<MediaLine>,
}

impl Sdp {
    /// Read a body.
    ///
    /// Tolerant by design (see the module header): unknown line types are
    /// kept or skipped, never fatal. The three things worth refusing are a
    /// body that is not SDP at all, a version this is not written against,
    /// and an `m=` line whose port is not a number -- the last because a
    /// caller that got a port wrong would otherwise send RTP to nowhere and
    /// spend the call wondering why the far end is silent.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut origin: Option<String> = None;
        let mut session_name: Option<String> = None;
        let mut connection: Option<String> = None;
        let mut attributes: Vec<(String, Option<String>)> = Vec::new();
        let mut media: Vec<MediaLine> = Vec::new();
        let mut version_seen = false;
        let mut any_line = false;

        // RFC 4566 5: every line is "<type>=<value>", one character of type.
        // Bare line feeds are accepted as well as CRLF, because they arrive.
        for raw in text.split('\n') {
            let line = raw.strip_suffix('\r').unwrap_or(raw).trim();
            if line.is_empty() {
                continue;
            }
            let Some((kind, value)) = line.split_once('=') else {
                // Not "x=y". A body with a stray line in it is still readable,
                // so this is skipped rather than refused.
                continue;
            };
            if kind.len() != 1 {
                continue;
            }
            any_line = true;
            let value = value.trim();
            match kind.as_bytes()[0] {
                b'v' => {
                    // 5.1: this field is the version of SDP, not of the
                    // session, and only 0 has ever been defined.
                    if value != "0" {
                        return Err(format!(
                            "this SDP body announces version {value:?}, and RFC 4566 defines only version 0"
                        ));
                    }
                    version_seen = true;
                }
                b'o' => origin = Some(value.to_owned()),
                b's' => session_name = Some(value.to_owned()),
                b'c' => {
                    // 5.7: a c= before the first m= belongs to the session, one
                    // after it to the stream it follows, and the media-level
                    // one overrides.
                    match media.last_mut() {
                        Some(last) => last.connection = Some(value.to_owned()),
                        None => connection = Some(value.to_owned()),
                    }
                }
                b'a' => {
                    let attribute = parse_attribute(value);
                    match media.last_mut() {
                        Some(last) => last.attributes.push(attribute),
                        None => attributes.push(attribute),
                    }
                }
                b'm' => media.push(parse_media(value)?),
                // b=, t=, r=, z=, k=, i=, u=, e=, p=: legal, and none of them
                // changes where a packet goes or what is in it. Bandwidth and
                // timing lines in particular arrive on every call and mean
                // nothing to a two-party modem call that lasts as long as the
                // dialog does.
                _ => {}
            }
        }

        if !any_line {
            return Err("the SDP body was empty".to_owned());
        }
        if !version_seen {
            return Err("the SDP body has no v= line, so it is not a session description".to_owned());
        }
        let origin = origin.ok_or_else(|| {
            "the SDP body has no o= line, so there is no way to tell one of its descriptions from the next"
                .to_owned()
        })?;

        Ok(Self {
            origin,
            // 5.3 requires s= and forbids it being empty, but a missing one
            // costs a call nothing, so it is filled in rather than refused.
            session_name: session_name.unwrap_or_else(|| "-".to_owned()),
            connection,
            attributes,
            media,
        })
    }

    /// The first audio stream. There is never more than one on a call this
    /// agent places, and a far end that offers two gets an answer about the
    /// first.
    pub fn audio(&self) -> Option<&MediaLine> {
        self.media.iter().find(|m| m.kind.eq_ignore_ascii_case("audio"))
    }

    /// The first `m=image` stream, which in practice means T.38 fax.
    pub fn image(&self) -> Option<&MediaLine> {
        self.media.iter().find(|m| m.kind.eq_ignore_ascii_case("image"))
    }

    /// A session-level attribute's value. Some far ends put `a=ptime` or a
    /// direction attribute above the first `m=` line, where RFC 4566 5.13
    /// says it applies to every stream that does not override it.
    pub fn attribute(&self, name: &str) -> Option<&str> {
        find_attribute(&self.attributes, name)
    }

    pub fn has_attribute(&self, name: &str) -> bool {
        any_attribute(&self.attributes, name)
    }

    /// The address a media line's RTP should be sent to: its own `c=` if it
    /// had one, otherwise the session's (RFC 4566 5.7).
    ///
    /// The connection field is "IN IP4 203.0.113.11", so what comes back is
    /// its third token. A multicast address carries a TTL and a count after a
    /// slash ("224.2.1.1/127/3"); that is stripped, although nothing on a
    /// telephone trunk has ever sent one.
    ///
    /// The lifetime is spelled out because the answer comes from one of two
    /// places -- the stream or the session -- and the compiler cannot guess
    /// that both outlive the call.
    pub fn address_for<'a>(&'a self, media: &'a MediaLine) -> Option<&'a str> {
        let field = media.connection.as_deref().or(self.connection.as_deref())?;
        let address = field.split_whitespace().nth(2)?;
        Some(address.split('/').next().unwrap_or(address))
    }

    /// The version field of `o=` (RFC 4566 5.2's third token): the number a
    /// far end increments when the description it is sending has actually
    /// changed. A re-INVITE whose origin version is the one we already have is
    /// a session refresh and does not need renegotiating -- which matters here,
    /// because tearing the RTP path down and building it again mid-call would
    /// cost the modem its training.
    pub fn origin_version(&self) -> Option<u64> {
        self.origin.split_whitespace().nth(2)?.parse().ok()
    }
}

impl fmt::Display for Sdp {
    /// RFC 4566 5's order, with CRLF endings.
    ///
    /// The order is not decoration: 5 lists the field types in the sequence
    /// they must appear in, and far ends that hand-roll their parsers do
    /// assume it. CRLF because 5 says so, even though almost everything
    /// accepts bare line feeds.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "v=0\r\n")?;
        write!(f, "o={}\r\n", self.origin)?;
        write!(f, "s={}\r\n", self.session_name)?;
        if let Some(connection) = &self.connection {
            write!(f, "c={connection}\r\n")?;
        }
        // 5.9: "t=0 0" is an unbounded session. A call's life is the dialog's,
        // not the description's, so there is never anything else to say here.
        write!(f, "t=0 0\r\n")?;
        for (name, value) in &self.attributes {
            write_attribute(f, name, value.as_deref())?;
        }
        for media in &self.media {
            write!(f, "m={} {} {}", media.kind, media.port, media.protocol)?;
            for format in &media.formats {
                write!(f, " {format}")?;
            }
            write!(f, "\r\n")?;
            if let Some(connection) = &media.connection {
                write!(f, "c={connection}\r\n")?;
            }
            for (name, value) in &media.attributes {
                write_attribute(f, name, value.as_deref())?;
            }
        }
        Ok(())
    }
}

/// What the two ends agreed to, in the terms the rest of the crate needs.
#[derive(Debug, Clone)]
pub struct Negotiated {
    pub law: Law,
    /// The payload type number the far end used. It is 0 or 8 in every case
    /// anyone has seen, but it is read off the description rather than derived
    /// from the law, because a far end that numbers G.711 unusually is doing
    /// something legal and an RTP stream sent with the wrong number in it is
    /// discarded silently.
    pub payload_type: u8,
    /// Where RTP is to be sent.
    pub address: String,
    pub port: u16,
    /// Already clamped to something a modem can live with.
    pub ptime_ms: u32,
    /// Ours, not the far end's: [`Direction::flipped`] has been applied.
    pub direction: Direction,
    /// The far end's payload type for RFC 4733 telephone-event, if it offered
    /// one. Only useful for recognising and discarding the events it sends;
    /// see [`offer`] for why a modem does not want to send them.
    pub telephone_event: Option<u8>,
}

/// Our offer: G.711 and nothing else, in the order given, plus an RFC 4733
/// telephone-event at `dtmf`'s payload type if one is asked for.
///
/// On telephone-event. A modem wants its DTMF as audio. RFC 4733 1 replaces
/// the tones with events and has the far end regenerate them, with the far
/// end's choice of level, duration and inter-digit gap -- so the digits that
/// reach the switch are not the digits that left here, and a dial string timed
/// to get past an IVR stops being timed at all. So this is not offered by
/// default. It exists because some trunks refuse a call outright when the
/// offer has no telephone-event in it, and a call placed with events we then
/// ignore is better than no call. Offering it does not mean sending it.
pub fn offer(address: &str, port: u16, laws: &[Law], ptime_ms: u32, dtmf: Option<u8>) -> String {
    // A caller that asks for no law at all gets both, mu-law first, rather
    // than an m= line with an empty format list -- which is malformed, and
    // which a far end would answer with a 488 rather than a question.
    let mut chosen: Vec<Law> = Vec::new();
    for law in laws.iter().copied().chain([Law::Mu, Law::A]) {
        if !chosen.contains(&law) {
            chosen.push(law);
        }
        if !laws.is_empty() && chosen.len() == laws.len() {
            break;
        }
    }

    let mut formats: Vec<String> = chosen.iter().map(|l| l.payload_type().to_string()).collect();
    let mut attributes: Vec<(String, Option<String>)> = Vec::new();
    for law in &chosen {
        // RFC 3551 3 makes an rtpmap for a static number redundant. It is
        // written anyway because far ends exist that look at rtpmap first and
        // at the static table never, and a redundant line has never offended
        // one that does it the other way round.
        attributes.push((
            "rtpmap".to_owned(),
            Some(format!("{} {}/8000", law.payload_type(), law.encoding_name())),
        ));
    }
    if let Some(pt) = dtmf {
        formats.push(pt.to_string());
        attributes.push((
            "rtpmap".to_owned(),
            Some(format!("{pt} telephone-event/8000")),
        ));
        // RFC 4733 3.2 and its Table 7: events 0 to 15 are the twelve keys of
        // a telephone plus A to D. Nothing beyond those is wanted here.
        attributes.push(("fmtp".to_owned(), Some(format!("{pt} 0-15"))));
    }
    attributes.push(("ptime".to_owned(), Some(clamp_ptime(ptime_ms).to_string())));
    // 6.7's default, written out. Silence would mean the same thing, but a
    // far end putting a call on hold reads our sendrecv back as a statement
    // about what we will do when the hold ends.
    attributes.push((Direction::SendReceive.as_str().to_owned(), None));

    Sdp {
        origin: origin_line(address),
        session_name: "-".to_owned(),
        connection: Some(connection_line(address)),
        attributes: Vec::new(),
        media: vec![MediaLine {
            kind: "audio".to_owned(),
            port,
            protocol: "RTP/AVP".to_owned(),
            formats,
            connection: None,
            attributes,
        }],
    }
    .to_string()
}

/// Answer somebody else's offer, and say what the answer committed us to.
///
/// RFC 3264 6: an answer has the same number of `m=` lines as the offer, in
/// the same order, and a stream that cannot be used is answered with port 0
/// rather than left out. Only one stream is ever offered to this agent that
/// it can do anything with, so what comes back is one audio line -- with one
/// payload type on it, because a modem has no use for the far end's freedom to
/// change codec mid-call and every use for knowing exactly what will arrive.
pub fn answer(
    offer: &Sdp,
    address: &str,
    port: u16,
    acceptable: &[Law],
    ptime_ms: u32,
) -> Result<(String, Negotiated), String> {
    let Some(media) = offer.audio() else {
        if wants_t38(offer) {
            // Where the T.38 answer would eventually be built. It would be an
            // m=image line over udptl carrying the format t38, with
            // a=T38FaxVersion:0, a=T38MaxBitRate matching the fax modes this
            // workspace already has, a=T38FaxRateManagement:transferredTCF and
            // a=T38FaxUdpEC:t38UDPRedundancy -- plus a UDPTL transport under
            // it, which is a different packet format from RTP and is not
            // written yet. Until then the caller turns this sentence into a
            // 488 and the far end falls back to sending the fax as audio,
            // which the V.17 and V.29 receivers here can already do.
            return Err(
                "the far end asked for a T.38 fax session on an m=image line, and this agent carries audio only"
                    .to_owned(),
            );
        }
        return Err("the offer has no m=audio line, so there is no call in it to answer".to_owned());
    };

    if media.port == 0 {
        return Err(
            "the offer's audio stream has port 0, which RFC 3264 6 means as a stream already withdrawn"
                .to_owned(),
        );
    }

    let Some(far_address) = offer.address_for(media) else {
        return Err(
            "the offer has no c= line at session or media level, so there is nowhere to send RTP"
                .to_owned(),
        );
    };

    // Chosen by walking the *offer's* list rather than ours. The far end's
    // first choice is nearly always the law its own trunk carries natively,
    // and picking the other one puts a transcode in the path -- which is
    // exactly the kind of thing that turned a clean V.90 downstream into
    // 37 dB of noise on the Crazytel trunk. Among two laws that are both
    // lossless codeword paths, matching the far end is worth more than our
    // own preference.
    let mut law = None;
    for format in &media.formats {
        if let Some(found) = law_of(media, format)
            && acceptable.contains(&found)
            && let Ok(pt) = format.trim().parse::<u8>()
        {
            law = Some((found, pt));
            break;
        }
    }
    let Some((law, payload_type)) = law else {
        return Err(format!(
            "the offer has no G.711 in it -- it offers {} -- and this modem carries nothing else, because a speech coder destroys a modem signal",
            describe_formats(media)
        ));
    };

    let ptime = clamp_ptime(stated_ptime(offer, media).unwrap_or(ptime_ms));
    let telephone_event = telephone_event_of(media);
    let direction = direction_of(offer, media, far_address).flipped();

    let mut formats = vec![payload_type.to_string()];
    let mut attributes = vec![(
        "rtpmap".to_owned(),
        Some(format!("{payload_type} {}/8000", law.encoding_name())),
    )];
    // RFC 3264 6.1: an answer may only contain payload types the offer had.
    // Telephone-event is mirrored when offered because refusing it makes some
    // trunks drop the call, and because an event we did not agree to still
    // arrives -- agreeing at least tells us which payload type to throw away.
    if let Some(pt) = telephone_event {
        formats.push(pt.to_string());
        attributes.push((
            "rtpmap".to_owned(),
            Some(format!("{pt} telephone-event/8000")),
        ));
        attributes.push(("fmtp".to_owned(), Some(format!("{pt} 0-15"))));
    }
    attributes.push(("ptime".to_owned(), Some(ptime.to_string())));
    // Our direction, which is the flip of theirs: 3264 6.1 requires an answer
    // to a sendonly offer to be recvonly and the other way about.
    attributes.push((direction.as_str().to_owned(), None));

    let body = Sdp {
        origin: origin_line(address),
        session_name: "-".to_owned(),
        connection: Some(connection_line(address)),
        attributes: Vec::new(),
        media: vec![MediaLine {
            kind: "audio".to_owned(),
            port,
            protocol: "RTP/AVP".to_owned(),
            formats,
            connection: None,
            attributes,
        }],
    }
    .to_string();

    Ok((
        body,
        Negotiated {
            law,
            payload_type,
            address: far_address.to_owned(),
            port: media.port,
            ptime_ms: ptime,
            direction,
            telephone_event,
        },
    ))
}

/// Read the far end's answer to an offer of ours.
///
/// Every `Err` here ends a call, and is meant to. `offered` is the list this
/// agent put in its own offer; an answer outside it is either a far end that
/// did not read the offer or a media gateway that substituted its own idea of
/// a codec, and neither can be talked round.
pub fn read_answer(answer: &Sdp, offered: &[Law]) -> Result<Negotiated, String> {
    let Some(media) = answer.audio() else {
        if wants_t38(answer) {
            return Err(
                "the far end answered with a T.38 fax session instead of audio, which this agent cannot carry"
                    .to_owned(),
            );
        }
        return Err(
            "the far end's answer has no m=audio line in it, so it has not agreed to carry anything"
                .to_owned(),
        );
    };

    if media.port == 0 {
        return Err(
            "the far end answered with audio port 0, which RFC 3264 6 means as refusing the stream outright"
                .to_owned(),
        );
    }

    let Some(address) = answer.address_for(media) else {
        return Err(
            "the far end's answer has no c= line at session or media level, so there is nowhere to send RTP"
                .to_owned(),
        );
    };

    // An answer over RTP/AVP is allowed more than one payload type (3264 6.1:
    // it means the answerer will accept any of them), and telephone-event is
    // commonly one of them, so the list is walked rather than assumed to have
    // one entry. The first entry that is a law we offered is the agreement.
    let mut chosen = None;
    for format in &media.formats {
        if let Some(law) = law_of(media, format)
            && offered.contains(&law)
            && let Ok(pt) = format.trim().parse::<u8>()
        {
            chosen = Some((law, pt));
            break;
        }
    }
    let Some((law, payload_type)) = chosen else {
        return Err(format!(
            "the far end answered with {}, which this modem did not offer and cannot carry: G.711 is the only coding that leaves a modem signal intact",
            describe_formats(media)
        ));
    };

    let ptime = clamp_ptime(stated_ptime(answer, media).unwrap_or(DEFAULT_PTIME_MS));

    Ok(Negotiated {
        law,
        payload_type,
        address: address.to_owned(),
        port: media.port,
        ptime_ms: ptime,
        direction: direction_of(answer, media, address).flipped(),
        telephone_event: telephone_event_of(media),
    })
}

/// Whether this description is asking for T.38 fax rather than audio.
///
/// It exists so that a far end's re-INVITE to fax is recognised for what it
/// is and declined with a 488, rather than being taken for an audio
/// description with nothing in it that we understand and answered with
/// something that leaves both ends waiting. No UDPTL is implemented here and
/// none is planned in this change.
pub fn wants_t38(sdp: &Sdp) -> bool {
    sdp.media.iter().any(|m| {
        // Port 0 is a stream being withdrawn, which is the *end* of a fax
        // attempt rather than the start of one.
        m.port != 0
            && m.kind.eq_ignore_ascii_case("image")
            && (m.protocol.to_ascii_lowercase().contains("udptl")
                || m.formats.iter().any(|f| f.eq_ignore_ascii_case("t38")))
    })
}

// ---- the small shared pieces -----------------------------------------

fn find_attribute<'a>(attributes: &'a [(String, Option<String>)], name: &str) -> Option<&'a str> {
    attributes
        .iter()
        .filter(|(n, _)| n.eq_ignore_ascii_case(name))
        .find_map(|(_, v)| v.as_deref())
}

fn any_attribute(attributes: &[(String, Option<String>)], name: &str) -> bool {
    attributes.iter().any(|(n, _)| n.eq_ignore_ascii_case(name))
}

/// RFC 4566 5.13: an attribute is either "a=<name>:<value>" or a bare
/// "a=<name>", and the bare form is a property rather than a missing value.
fn parse_attribute(value: &str) -> (String, Option<String>) {
    match value.split_once(':') {
        Some((name, rest)) => (name.trim().to_owned(), Some(rest.trim().to_owned())),
        None => (value.trim().to_owned(), None),
    }
}

fn write_attribute(f: &mut fmt::Formatter<'_>, name: &str, value: Option<&str>) -> fmt::Result {
    match value {
        Some(value) => write!(f, "a={name}:{value}\r\n"),
        None => write!(f, "a={name}\r\n"),
    }
}

/// RFC 4566 5.14's "m=<media> <port> <proto> <fmt> ...".
fn parse_media(value: &str) -> Result<MediaLine, String> {
    let mut parts = value.split_whitespace();
    let kind = parts
        .next()
        .ok_or_else(|| "an m= line in the SDP body has nothing on it".to_owned())?;
    let port_field = parts
        .next()
        .ok_or_else(|| format!("the m={kind} line has no port on it"))?;
    // 5.14 allows "<port>/<number of ports>" for a stream that occupies
    // several. Nothing on a telephone trunk does, but reading the first half
    // is right either way.
    let port_text = port_field.split('/').next().unwrap_or(port_field);
    let port: u16 = port_text.parse().map_err(|_| {
        format!("the m={kind} line's port field {port_field:?} is not a port number")
    })?;
    Ok(MediaLine {
        kind: kind.to_owned(),
        port,
        // A missing transport is malformed, but an empty one is readable and
        // the stream is refused later for having no format we know rather
        // than here for being untidy.
        protocol: parts.next().unwrap_or("").to_owned(),
        formats: parts.map(str::to_owned).collect(),
        connection: None,
        attributes: Vec::new(),
    })
}

/// Which companding law a format on this media line means.
///
/// Two sources, and they can disagree. `a=rtpmap` says so explicitly; RFC
/// 3551 Table 4 fixes 0 as PCMU/8000 and 8 as PCMA/8000 for when nothing says
/// anything, which is the common case on a trunk. When there is an rtpmap and
/// it contradicts the static number -- `a=rtpmap:8 PCMU/8000` -- the rtpmap
/// wins: the static table is a default, and a far end that wrote a mapping
/// wrote it on purpose. Believing the table there would decode every sample of
/// the call through the wrong law, which sounds like a badly distorted line
/// and trains no modem at all.
fn law_of(media: &MediaLine, format: &str) -> Option<Law> {
    let payload_type: u8 = format.trim().parse().ok()?;
    let Some(map) = media.rtpmap(payload_type) else {
        return Law::from_payload_type(payload_type);
    };
    let mut fields = map.split('/');
    let name = fields.next()?.trim();
    // The clock rate has to be 8000. "PCMU/16000" is a legal thing to write
    // and is not the coding a telephone network carries, so it is not ours.
    if let Some(rate) = fields.next()
        && rate.trim() != "8000"
    {
        return None;
    }
    if name.eq_ignore_ascii_case("PCMU") {
        Some(Law::Mu)
    } else if name.eq_ignore_ascii_case("PCMA") {
        Some(Law::A)
    } else {
        None
    }
}

/// The payload type an RFC 4733 telephone-event was given, if there is one.
/// It is always mapped: 4733 uses a dynamic number, so there is no static
/// table to fall back on.
fn telephone_event_of(media: &MediaLine) -> Option<u8> {
    media.formats.iter().find_map(|format| {
        let payload_type: u8 = format.trim().parse().ok()?;
        let map = media.rtpmap(payload_type)?;
        let name = map.split('/').next().unwrap_or(map).trim();
        name.eq_ignore_ascii_case("telephone-event")
            .then_some(payload_type)
    })
}

/// The direction as the far end stated it, media level first and session level
/// behind it (RFC 4566 5.13), with the old hold convention on top.
fn direction_of(sdp: &Sdp, media: &MediaLine, address: &str) -> Direction {
    // RFC 3264 8.4: a connection address of 0.0.0.0 was how hold was signalled
    // before a=inactive existed, and far ends still send it. It is not an
    // address, it is a statement that nothing is to be sent anywhere, so it
    // outranks whatever the direction attributes say -- and a rig that takes
    // it at face value spends the hold transmitting RTP into the void and
    // counting the silence coming back as a far end that died.
    if address == "0.0.0.0" || address == "::" {
        return Direction::Inactive;
    }
    for name in ["inactive", "sendonly", "recvonly", "sendrecv"] {
        if media.has_attribute(name) {
            return media.direction();
        }
    }
    if sdp.has_attribute("inactive") {
        Direction::Inactive
    } else if sdp.has_attribute("sendonly") {
        Direction::SendOnly
    } else if sdp.has_attribute("recvonly") {
        Direction::ReceiveOnly
    } else {
        Direction::SendReceive
    }
}

/// The ptime the far end asked for, media level first, then session level.
fn stated_ptime(sdp: &Sdp, media: &MediaLine) -> Option<u32> {
    media
        .attribute("ptime")
        .or_else(|| sdp.attribute("ptime"))
        .and_then(|v| v.trim().parse().ok())
}

fn clamp_ptime(ms: u32) -> u32 {
    ms.clamp(MIN_PTIME_MS, MAX_PTIME_MS)
}

/// Every format on a media line, named the way a person would recognise it,
/// for the sentence that goes in front of one when a call is refused.
fn describe_formats(media: &MediaLine) -> String {
    let named: Vec<String> = media
        .formats
        .iter()
        .map(|format| describe_format(media, format))
        .collect();
    if named.is_empty() {
        "nothing at all".to_owned()
    } else {
        named.join(", ")
    }
}

fn describe_format(media: &MediaLine, format: &str) -> String {
    let Ok(payload_type) = format.trim().parse::<u8>() else {
        // A non-RTP transport names its format rather than numbering it.
        return format.trim().to_owned();
    };
    if let Some(map) = media.rtpmap(payload_type) {
        let name = map.split('/').next().unwrap_or(map).trim();
        return format!("{name} (payload type {payload_type})");
    }
    match static_name(payload_type) {
        Some(name) => format!("{name} (payload type {payload_type})"),
        None => format!("payload type {payload_type}"),
    }
}

/// RFC 3551 Table 4's assignments, for naming a codec in an error message.
/// Only the audio half, and only so that "the far end answered with G.729"
/// can be written instead of "the far end answered with 18".
fn static_name(payload_type: u8) -> Option<&'static str> {
    Some(match payload_type {
        0 => "PCMU",
        3 => "GSM",
        4 => "G723",
        5 | 6 | 16 | 17 => "DVI4",
        7 => "LPC",
        8 => "PCMA",
        9 => "G722",
        10 | 11 => "L16",
        12 => "QCELP",
        13 => "CN",
        14 => "MPA",
        15 => "G728",
        18 => "G729",
        _ => return None,
    })
}

/// "IN IP4 <address>", or IP6 when the address has a colon in it. Nothing
/// here resolves names: a c= line with a host name in it is legal and no
/// trunk sends one, and a modem that had to wait on a resolver mid-call would
/// deserve what it got.
fn connection_line(address: &str) -> String {
    format!("IN {} {address}", address_type(address))
}

fn address_type(address: &str) -> &'static str {
    if address.contains(':') { "IP6" } else { "IP4" }
}

/// RFC 4566 5.2's o= line.
///
/// The session id must be unique for this username and host; 5.2 suggests an
/// NTP timestamp, and seconds since the Unix epoch are that minus a constant.
/// A counter is folded in because two calls placed in the same second must
/// still differ. The version starts equal to the id; a re-INVITE that carries
/// a changed description has to increment it, and since this function returns
/// a finished body it is the layer above that has to do that -- see
/// [`Sdp::origin_version`].
fn origin_line(address: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNT: AtomicU64 = AtomicU64::new(0);
    /// Seconds from the NTP epoch of 1900 to the Unix one of 1970.
    const NTP_EPOCH_OFFSET: u64 = 2_208_988_800;
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let id = seconds
        .wrapping_add(NTP_EPOCH_OFFSET)
        .wrapping_add(COUNT.fetch_add(1, Ordering::Relaxed));
    // The username is "-": 5.2 allows it where the originating host has no
    // notion of a user, and putting a real one there tells a trunk operator
    // something about this machine that it has no business knowing.
    format!("- {id} {id} IN {} {address}", address_type(address))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Asterisk, the way it writes an offer: rtpmap for everything including
    /// the static numbers, telephone-event at 101, a maxptime nobody reads,
    /// CRLF throughout.
    const ASTERISK_OFFER: &str = concat!(
        "v=0\r\n",
        "o=root 1899373098 1899373098 IN IP4 203.0.113.11\r\n",
        "s=Asterisk PBX 18.10.0\r\n",
        "c=IN IP4 203.0.113.11\r\n",
        "t=0 0\r\n",
        "m=audio 14884 RTP/AVP 0 8 101\r\n",
        "a=rtpmap:0 PCMU/8000\r\n",
        "a=rtpmap:8 PCMA/8000\r\n",
        "a=rtpmap:101 telephone-event/8000\r\n",
        "a=fmtp:101 0-16\r\n",
        "a=ptime:20\r\n",
        "a=maxptime:150\r\n",
        "a=sendrecv\r\n",
    );

    /// A wholesale trunk of the Crazytel sort: A-law first because the
    /// network under it is A-law, no rtpmap at all for the static numbers,
    /// bare line feeds, and two vendor attributes that mean nothing here.
    const TRUNK_OFFER: &str = concat!(
        "v=0\n",
        "o=- 8000015 8000015 IN IP4 103.28.12.9\n",
        "s=-\n",
        "c=IN IP4 103.28.12.9\n",
        "t=0 0\n",
        "b=AS:84\n",
        "m=audio 21384 RTP/AVP 8 0 101\n",
        "a=rtpmap:101 telephone-event/8000\n",
        "a=fmtp:101 0-15\n",
        "a=ptime:20\n",
        "a=maxptime:40\n",
        "a=sendrecv\n",
        "a=X-sqn:0\n",
        "a=X-cap:1 audio RTP/AVP 100\n",
    );

    /// A Kamailio-fronted answer with the media address on the media line and
    /// a different one at session level, which is what a proxy that relays
    /// only some streams produces.
    const RELAYED_ANSWER: &str = concat!(
        "v=0\r\n",
        "o=- 1690000001 1690000001 IN IP4 198.51.100.7\r\n",
        "s=Kamailio\r\n",
        "c=IN IP4 198.51.100.7\r\n",
        "t=0 0\r\n",
        "m=audio 35022 RTP/AVP 0 101\r\n",
        "c=IN IP4 198.51.100.44\r\n",
        "a=rtpmap:101 telephone-event/8000\r\n",
        "a=ptime:20\r\n",
        "a=sendrecv\r\n",
        "a=rtcp-mux\r\n",
    );

    #[test]
    fn an_asterisk_offer_is_read_the_way_asterisk_meant_it() {
        let sdp = Sdp::parse(ASTERISK_OFFER).unwrap();
        assert_eq!(sdp.origin, "root 1899373098 1899373098 IN IP4 203.0.113.11");
        assert_eq!(sdp.origin_version(), Some(1899373098));
        assert_eq!(sdp.session_name, "Asterisk PBX 18.10.0");
        let audio = sdp.audio().unwrap();
        assert_eq!(audio.port, 14884);
        assert_eq!(audio.protocol, "RTP/AVP");
        assert_eq!(audio.formats, ["0", "8", "101"]);
        assert_eq!(audio.rtpmap(8), Some("PCMA/8000"));
        assert_eq!(audio.attribute("maxptime"), Some("150"));
        assert_eq!(sdp.address_for(audio), Some("203.0.113.11"));
        assert!(sdp.image().is_none());
        assert!(!wants_t38(&sdp));

        let (body, agreed) = answer(&sdp, "192.0.2.4", 40000, &[Law::Mu, Law::A], 20).unwrap();
        // The offer's own first choice, not ours.
        assert_eq!(agreed.law, Law::Mu);
        assert_eq!(agreed.payload_type, 0);
        assert_eq!(agreed.address, "203.0.113.11");
        assert_eq!(agreed.port, 14884);
        assert_eq!(agreed.ptime_ms, 20);
        assert_eq!(agreed.direction, Direction::SendReceive);
        assert_eq!(agreed.telephone_event, Some(101));
        assert!(body.contains("m=audio 40000 RTP/AVP 0 101\r\n"));
        assert!(body.contains("a=rtpmap:0 PCMU/8000\r\n"));
        assert!(!body.contains("PCMA"));
    }

    #[test]
    fn a_trunk_that_omits_rtpmap_for_static_payload_types_is_still_understood() {
        let sdp = Sdp::parse(TRUNK_OFFER).unwrap();
        let audio = sdp.audio().unwrap();
        // Nothing maps 8 or 0; RFC 3551 Table 4 does.
        assert!(audio.rtpmap(8).is_none());
        assert_eq!(audio.attribute("X-sqn"), Some("0"));

        let (body, agreed) = answer(&sdp, "192.0.2.4", 40002, &[Law::Mu, Law::A], 20).unwrap();
        // A-law, because the trunk asked for A-law first and it is the law its
        // own network carries.
        assert_eq!(agreed.law, Law::A);
        assert_eq!(agreed.payload_type, 8);
        assert_eq!(agreed.address, "103.28.12.9");
        assert_eq!(agreed.telephone_event, Some(101));
        assert!(body.contains("a=rtpmap:8 PCMA/8000\r\n"));
        assert!(body.contains("a=rtpmap:101 telephone-event/8000\r\n"));
    }

    #[test]
    fn an_rtpmap_that_contradicts_the_static_number_wins() {
        let text = concat!(
            "v=0\r\n",
            "o=- 1 1 IN IP4 203.0.113.30\r\n",
            "s=-\r\n",
            "c=IN IP4 203.0.113.30\r\n",
            "t=0 0\r\n",
            "m=audio 9000 RTP/AVP 8\r\n",
            "a=rtpmap:8 PCMU/8000\r\n",
        );
        let sdp = Sdp::parse(text).unwrap();
        let agreed = read_answer(&sdp, &[Law::Mu, Law::A]).unwrap();
        assert_eq!(agreed.law, Law::Mu);
        // The number stays the far end's, whatever it means by it.
        assert_eq!(agreed.payload_type, 8);
    }

    #[test]
    fn a_law_at_the_wrong_clock_rate_is_not_a_law_we_know() {
        let text = concat!(
            "v=0\r\n",
            "o=- 1 1 IN IP4 203.0.113.30\r\n",
            "s=-\r\n",
            "c=IN IP4 203.0.113.30\r\n",
            "t=0 0\r\n",
            "m=audio 9000 RTP/AVP 0\r\n",
            "a=rtpmap:0 PCMU/16000\r\n",
        );
        let sdp = Sdp::parse(text).unwrap();
        let refused = read_answer(&sdp, &[Law::Mu, Law::A]).unwrap_err();
        assert!(refused.contains("PCMU"), "{refused}");
    }

    #[test]
    fn an_answer_that_chose_a_law_we_did_not_offer_is_refused() {
        let text = concat!(
            "v=0\r\n",
            "o=- 22 22 IN IP4 203.0.113.9\r\n",
            "s=-\r\n",
            "c=IN IP4 203.0.113.9\r\n",
            "t=0 0\r\n",
            "m=audio 19002 RTP/AVP 8\r\n",
            "a=ptime:20\r\n",
        );
        let sdp = Sdp::parse(text).unwrap();
        // We offered mu-law only; it answered A-law.
        let refused = read_answer(&sdp, &[Law::Mu]).unwrap_err();
        assert!(refused.contains("PCMA"), "{refused}");
        assert!(refused.contains("G.711"), "{refused}");
        // And the same body is fine when A-law was on the offer.
        assert_eq!(read_answer(&sdp, &[Law::Mu, Law::A]).unwrap().law, Law::A);
    }

    #[test]
    fn an_answer_of_g729_names_g729_in_the_refusal() {
        let text = concat!(
            "v=0\r\n",
            "o=- 5 5 IN IP4 203.0.113.60\r\n",
            "s=-\r\n",
            "c=IN IP4 203.0.113.60\r\n",
            "t=0 0\r\n",
            "m=audio 30000 RTP/AVP 18 101\r\n",
            "a=rtpmap:101 telephone-event/8000\r\n",
        );
        let sdp = Sdp::parse(text).unwrap();
        let refused = read_answer(&sdp, &[Law::Mu, Law::A]).unwrap_err();
        assert!(refused.contains("G729"), "{refused}");
        assert!(refused.contains("telephone-event"), "{refused}");
    }

    #[test]
    fn an_answer_with_a_zero_port_is_refused() {
        let text = concat!(
            "v=0\r\n",
            "o=- 7 7 IN IP4 203.0.113.9\r\n",
            "s=-\r\n",
            "c=IN IP4 203.0.113.9\r\n",
            "t=0 0\r\n",
            "m=audio 0 RTP/AVP 0\r\n",
        );
        let sdp = Sdp::parse(text).unwrap();
        let refused = read_answer(&sdp, &[Law::Mu]).unwrap_err();
        assert!(refused.contains("port 0"), "{refused}");
    }

    #[test]
    fn an_answer_with_no_audio_at_all_is_refused() {
        let text = concat!(
            "v=0\r\n",
            "o=- 9 9 IN IP4 203.0.113.9\r\n",
            "s=-\r\n",
            "c=IN IP4 203.0.113.9\r\n",
            "t=0 0\r\n",
            "m=video 40000 RTP/AVP 96\r\n",
            "a=rtpmap:96 H264/90000\r\n",
        );
        let sdp = Sdp::parse(text).unwrap();
        let refused = read_answer(&sdp, &[Law::Mu]).unwrap_err();
        assert!(refused.contains("m=audio"), "{refused}");
    }

    #[test]
    fn an_answer_with_no_connection_address_is_refused() {
        let text = concat!(
            "v=0\r\n",
            "o=- 11 11 IN IP4 203.0.113.9\r\n",
            "s=-\r\n",
            "t=0 0\r\n",
            "m=audio 19004 RTP/AVP 0\r\n",
        );
        let sdp = Sdp::parse(text).unwrap();
        let refused = read_answer(&sdp, &[Law::Mu]).unwrap_err();
        assert!(refused.contains("c="), "{refused}");
    }

    #[test]
    fn the_far_ends_sendonly_is_our_receive_only() {
        let text = concat!(
            "v=0\r\n",
            "o=- 13 14 IN IP4 203.0.113.9\r\n",
            "s=-\r\n",
            "c=IN IP4 203.0.113.9\r\n",
            "t=0 0\r\n",
            "m=audio 19006 RTP/AVP 0\r\n",
            "a=sendonly\r\n",
        );
        let sdp = Sdp::parse(text).unwrap();
        assert_eq!(sdp.audio().unwrap().direction(), Direction::SendOnly);
        let agreed = read_answer(&sdp, &[Law::Mu]).unwrap();
        assert_eq!(agreed.direction, Direction::ReceiveOnly);
    }

    #[test]
    fn a_session_level_direction_applies_to_a_stream_that_says_nothing() {
        let text = concat!(
            "v=0\r\n",
            "o=- 15 16 IN IP4 203.0.113.9\r\n",
            "s=-\r\n",
            "c=IN IP4 203.0.113.9\r\n",
            "t=0 0\r\n",
            "a=recvonly\r\n",
            "m=audio 19008 RTP/AVP 0\r\n",
            "a=ptime:20\r\n",
        );
        let sdp = Sdp::parse(text).unwrap();
        assert!(sdp.has_attribute("recvonly"));
        let agreed = read_answer(&sdp, &[Law::Mu]).unwrap();
        assert_eq!(agreed.direction, Direction::SendOnly);
    }

    #[test]
    fn a_connection_of_all_zeroes_is_the_old_way_of_saying_hold() {
        let text = concat!(
            "v=0\r\n",
            "o=- 17 18 IN IP4 0.0.0.0\r\n",
            "s=-\r\n",
            "c=IN IP4 0.0.0.0\r\n",
            "t=0 0\r\n",
            "m=audio 19010 RTP/AVP 0\r\n",
            "a=sendrecv\r\n",
        );
        let sdp = Sdp::parse(text).unwrap();
        let agreed = read_answer(&sdp, &[Law::Mu]).unwrap();
        // The a=sendrecv is overruled by the address, and the address is still
        // reported so a caller can see what it was told.
        assert_eq!(agreed.direction, Direction::Inactive);
        assert_eq!(agreed.address, "0.0.0.0");
    }

    #[test]
    fn a_media_level_connection_beats_the_session_level_one() {
        let sdp = Sdp::parse(RELAYED_ANSWER).unwrap();
        let audio = sdp.audio().unwrap();
        assert_eq!(audio.connection.as_deref(), Some("IN IP4 198.51.100.44"));
        assert_eq!(sdp.address_for(audio), Some("198.51.100.44"));
        let agreed = read_answer(&sdp, &[Law::Mu, Law::A]).unwrap();
        assert_eq!(agreed.address, "198.51.100.44");
        assert_eq!(agreed.port, 35022);
        assert_eq!(agreed.telephone_event, Some(101));
    }

    #[test]
    fn our_offer_mentions_nothing_but_g711() {
        let body = offer("192.0.2.4", 40004, &[Law::Mu, Law::A], 20, None);
        assert!(body.starts_with("v=0\r\n"));
        assert!(body.contains("c=IN IP4 192.0.2.4\r\n"));
        assert!(body.contains("t=0 0\r\n"));
        assert!(body.contains("m=audio 40004 RTP/AVP 0 8\r\n"));
        assert!(body.contains("a=rtpmap:0 PCMU/8000\r\n"));
        assert!(body.contains("a=rtpmap:8 PCMA/8000\r\n"));
        assert!(body.contains("a=ptime:20\r\n"));
        assert!(body.contains("a=sendrecv\r\n"));
        // Nothing that models a voice tract.
        for speech in ["G729", "GSM", "opus", "G722", "telephone-event"] {
            assert!(!body.contains(speech), "the offer mentions {speech}");
        }
    }

    #[test]
    fn the_offers_order_is_the_order_it_was_asked_for() {
        let body = offer("192.0.2.4", 40006, &[Law::A], 20, None);
        assert!(body.contains("m=audio 40006 RTP/AVP 8\r\n"), "{body}");
        assert!(!body.contains("rtpmap:0"), "{body}");
        let both = offer("192.0.2.4", 40006, &[Law::A, Law::Mu], 20, None);
        assert!(both.contains("m=audio 40006 RTP/AVP 8 0\r\n"), "{both}");
        // Asking for nothing gets both rather than an unusable m= line.
        let neither = offer("192.0.2.4", 40006, &[], 20, None);
        assert!(neither.contains("m=audio 40006 RTP/AVP 0 8\r\n"), "{neither}");
    }

    #[test]
    fn telephone_event_is_offered_only_when_it_is_asked_for() {
        let without = offer("192.0.2.4", 40008, &[Law::Mu], 20, None);
        assert!(!without.contains("telephone-event"));
        let with = offer("192.0.2.4", 40008, &[Law::Mu], 20, Some(101));
        assert!(with.contains("m=audio 40008 RTP/AVP 0 101\r\n"), "{with}");
        assert!(with.contains("a=rtpmap:101 telephone-event/8000\r\n"));
        assert!(with.contains("a=fmtp:101 0-15\r\n"));
    }

    #[test]
    fn an_offer_of_g729_alone_leaves_us_nothing_to_answer_with() {
        let text = concat!(
            "v=0\r\n",
            "o=- 19 19 IN IP4 203.0.113.70\r\n",
            "s=-\r\n",
            "c=IN IP4 203.0.113.70\r\n",
            "t=0 0\r\n",
            "m=audio 25000 RTP/AVP 18\r\n",
            "a=rtpmap:18 G729/8000\r\n",
            "a=fmtp:18 annexb=no\r\n",
        );
        let sdp = Sdp::parse(text).unwrap();
        let refused = answer(&sdp, "192.0.2.4", 40010, &[Law::Mu, Law::A], 20).unwrap_err();
        assert!(refused.contains("G729"), "{refused}");
        assert!(refused.contains("speech coder"), "{refused}");
    }

    #[test]
    fn a_rejected_audio_stream_in_an_offer_leaves_nothing_to_answer() {
        let text = concat!(
            "v=0\r\n",
            "o=- 21 22 IN IP4 203.0.113.9\r\n",
            "s=-\r\n",
            "c=IN IP4 203.0.113.9\r\n",
            "t=0 0\r\n",
            "m=audio 0 RTP/AVP 0\r\n",
        );
        let sdp = Sdp::parse(text).unwrap();
        let refused = answer(&sdp, "192.0.2.4", 40012, &[Law::Mu], 20).unwrap_err();
        assert!(refused.contains("port 0"), "{refused}");
    }

    #[test]
    fn an_absurd_ptime_is_brought_back_to_something_a_modem_can_use() {
        let long = concat!(
            "v=0\r\n",
            "o=- 23 23 IN IP4 203.0.113.9\r\n",
            "s=-\r\n",
            "c=IN IP4 203.0.113.9\r\n",
            "t=0 0\r\n",
            "m=audio 19012 RTP/AVP 0\r\n",
            "a=ptime:60\r\n",
        );
        let sdp = Sdp::parse(long).unwrap();
        assert_eq!(read_answer(&sdp, &[Law::Mu]).unwrap().ptime_ms, 40);

        let short = long.replace("a=ptime:60", "a=ptime:5");
        let sdp = Sdp::parse(&short).unwrap();
        assert_eq!(read_answer(&sdp, &[Law::Mu]).unwrap().ptime_ms, 10);

        // Ours is clamped on the way out too, so a caller cannot ask for a
        // packet length the far end would have to argue with.
        assert!(offer("192.0.2.4", 40014, &[Law::Mu], 200, None).contains("a=ptime:40\r\n"));
    }

    #[test]
    fn bare_line_feeds_parse_the_same_as_crlf() {
        let crlf = Sdp::parse(&TRUNK_OFFER.replace('\n', "\r\n")).unwrap();
        let lf = Sdp::parse(TRUNK_OFFER).unwrap();
        assert_eq!(crlf.origin, lf.origin);
        assert_eq!(crlf.audio().unwrap().formats, lf.audio().unwrap().formats);
        assert_eq!(crlf.media.len(), lf.media.len());
    }

    #[test]
    fn lines_and_attributes_we_do_not_know_are_carried_past_without_complaint() {
        let text = concat!(
            "v=0\r\n",
            "o=- 25 25 IN IP4 203.0.113.9\r\n",
            "s=-\r\n",
            "i=a session information line\r\n",
            "u=http://example.net/\r\n",
            "c=IN IP4 203.0.113.9\r\n",
            "b=AS:84\r\n",
            "b=TIAS:64000\r\n",
            "t=0 0\r\n",
            "r=7d 1h 0 25h\r\n",
            "a=tool:something nobody here has heard of\r\n",
            "m=audio 19014 RTP/AVP 0\r\n",
            "a=rtcp:19015 IN IP4 203.0.113.9\r\n",
            "a=X-vendor-private\r\n",
            "a=ssrc:2890844526 cname:x@y\r\n",
            "a=ptime:20\r\n",
            "nonsense with no equals sign in it\r\n",
        );
        let sdp = Sdp::parse(text).unwrap();
        assert_eq!(sdp.attribute("tool"), Some("something nobody here has heard of"));
        let agreed = read_answer(&sdp, &[Law::Mu]).unwrap();
        assert_eq!(agreed.law, Law::Mu);
        assert_eq!(agreed.port, 19014);
    }

    #[test]
    fn an_attribute_with_no_value_is_still_an_attribute() {
        let sdp = Sdp::parse(RELAYED_ANSWER).unwrap();
        let audio = sdp.audio().unwrap();
        assert!(audio.has_attribute("rtcp-mux"));
        assert!(audio.has_attribute("RTCP-MUX"), "names are case-insensitive");
        assert_eq!(audio.attribute("rtcp-mux"), None);
        assert!(!audio.has_attribute("rtcp-fb"));
    }

    #[test]
    fn a_body_that_is_not_sdp_at_all_is_refused_with_a_sentence() {
        assert!(Sdp::parse("").unwrap_err().contains("empty"));
        let no_version = "o=- 1 1 IN IP4 203.0.113.9\r\ns=-\r\nm=audio 1 RTP/AVP 0\r\n";
        assert!(Sdp::parse(no_version).unwrap_err().contains("v="));
        let no_origin = "v=0\r\ns=-\r\nm=audio 1 RTP/AVP 0\r\n";
        assert!(Sdp::parse(no_origin).unwrap_err().contains("o="));
        let wrong_version = "v=1\r\no=- 1 1 IN IP4 203.0.113.9\r\ns=-\r\n";
        assert!(Sdp::parse(wrong_version).unwrap_err().contains("version"));
        let bad_port = "v=0\r\no=- 1 1 IN IP4 203.0.113.9\r\ns=-\r\nm=audio RTP/AVP 0\r\n";
        assert!(Sdp::parse(bad_port).unwrap_err().contains("port"));
    }

    #[test]
    fn a_reinvite_to_t38_is_recognised_as_fax() {
        let text = concat!(
            "v=0\r\n",
            "o=root 1899373098 1899373099 IN IP4 203.0.113.11\r\n",
            "s=Asterisk PBX 18.10.0\r\n",
            "c=IN IP4 203.0.113.11\r\n",
            "t=0 0\r\n",
            "m=image 4000 udptl t38\r\n",
            "a=T38FaxVersion:0\r\n",
            "a=T38MaxBitRate:14400\r\n",
            "a=T38FaxRateManagement:transferredTCF\r\n",
            "a=T38FaxMaxDatagram:400\r\n",
            "a=T38FaxUdpEC:t38UDPRedundancy\r\n",
        );
        let sdp = Sdp::parse(text).unwrap();
        assert!(wants_t38(&sdp));
        let image = sdp.image().unwrap();
        assert_eq!(image.formats, ["t38"]);
        assert_eq!(image.attribute("T38FaxVersion"), Some("0"));
        assert_eq!(image.attribute("t38maxbitrate"), Some("14400"));
        // The version went up, which is how the re-INVITE is known to be a new
        // description rather than a refresh.
        assert_eq!(sdp.origin_version(), Some(1899373099));
        let refused = answer(&sdp, "192.0.2.4", 40016, &[Law::Mu, Law::A], 20).unwrap_err();
        assert!(refused.contains("T.38"), "{refused}");
    }

    #[test]
    fn a_withdrawn_fax_stream_is_not_a_fax_being_asked_for() {
        let text = concat!(
            "v=0\r\n",
            "o=- 27 28 IN IP4 203.0.113.9\r\n",
            "s=-\r\n",
            "c=IN IP4 203.0.113.9\r\n",
            "t=0 0\r\n",
            "m=image 0 udptl t38\r\n",
            "m=audio 19016 RTP/AVP 0\r\n",
        );
        let sdp = Sdp::parse(text).unwrap();
        assert!(!wants_t38(&sdp));
        assert_eq!(read_answer(&sdp, &[Law::Mu]).unwrap().port, 19016);
    }

    #[test]
    fn what_we_write_parses_back_to_what_we_meant() {
        let body = offer("192.0.2.4", 40018, &[Law::A, Law::Mu], 20, Some(101));
        let sdp = Sdp::parse(&body).unwrap();
        assert_eq!(sdp.session_name, "-");
        assert_eq!(sdp.connection.as_deref(), Some("IN IP4 192.0.2.4"));
        let audio = sdp.audio().unwrap();
        assert_eq!(audio.formats, ["8", "0", "101"]);
        assert_eq!(audio.direction(), Direction::SendReceive);
        assert_eq!(audio.attribute("ptime"), Some("20"));
        assert_eq!(telephone_event_of(audio), Some(101));
        // And written out again it is byte for byte the same body.
        assert_eq!(sdp.to_string(), body);
    }

    #[test]
    fn the_answer_we_write_keeps_the_far_ends_numbering() {
        // A far end that numbers telephone-event 96 rather than 101, which is
        // legal and happens.
        let text = concat!(
            "v=0\r\n",
            "o=- 29 29 IN IP4 203.0.113.80\r\n",
            "s=-\r\n",
            "c=IN IP4 203.0.113.80\r\n",
            "t=0 0\r\n",
            "m=audio 27000 RTP/AVP 8 96\r\n",
            "a=rtpmap:96 telephone-event/8000\r\n",
            "a=sendonly\r\n",
        );
        let sdp = Sdp::parse(text).unwrap();
        let (body, agreed) = answer(&sdp, "192.0.2.4", 40020, &[Law::A], 20).unwrap();
        assert_eq!(agreed.payload_type, 8);
        assert_eq!(agreed.telephone_event, Some(96));
        assert_eq!(agreed.direction, Direction::ReceiveOnly);
        assert!(body.contains("m=audio 40020 RTP/AVP 8 96\r\n"), "{body}");
        assert!(body.contains("a=rtpmap:96 telephone-event/8000\r\n"), "{body}");
        // 3264 6.1: a sendonly offer is answered recvonly.
        assert!(body.contains("a=recvonly\r\n"), "{body}");
        // And what we wrote reads back as an answer we would accept.
        let back = Sdp::parse(&body).unwrap();
        assert_eq!(read_answer(&back, &[Law::A]).unwrap().law, Law::A);
    }

    #[test]
    fn two_calls_do_not_share_a_session_id() {
        let first = Sdp::parse(&offer("192.0.2.4", 1, &[Law::Mu], 20, None)).unwrap();
        let second = Sdp::parse(&offer("192.0.2.4", 2, &[Law::Mu], 20, None)).unwrap();
        assert_ne!(first.origin, second.origin);
    }

    #[test]
    fn an_ipv6_address_is_written_as_ipv6() {
        let body = offer("2001:db8::4", 40022, &[Law::Mu], 20, None);
        assert!(body.contains("c=IN IP6 2001:db8::4\r\n"), "{body}");
        assert!(body.contains("o=- "), "{body}");
        let sdp = Sdp::parse(&body).unwrap();
        assert_eq!(sdp.address_for(sdp.audio().unwrap()), Some("2001:db8::4"));
    }
}
