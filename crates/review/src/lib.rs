//! A recorded call, read by a modem that takes no part in it.
//!
//! The modem proper is one end of a call: what it hears it answers, and what
//! it can read of the far end depends on what it sent itself. This reads a
//! recording of somebody else's call instead. It sends nothing, it has the
//! whole recording before it starts, and it is after a different thing:
//! not the data, or not only the data, but everything the two ends said to
//! each other on the way to it, and when.
//!
//! A recording is one of two kinds, and what can be had from it differs.
//! With an end a channel -- a capture of the two RTP streams of a call, or
//! the modem's own recordings -- each end's signal is nearly alone and can be
//! followed as far as data mode and through it. With both ends on one
//! channel, as a tap on the pair or a microphone at the speaker has it,
//! everything up to the point where both send in the same band can be read
//! and nothing after it can: there the two are told apart only by each end
//! subtracting what it sent, and a tap sent nothing.

pub mod model;
pub mod pcm;
pub mod phase2;
pub mod picture;
pub mod setup;

use line::wav::Wav;

pub use model::{Channel, End, Event, Layer, Plot, Report, Tap, Track, Waterfall};

/// Read a recording.
pub fn analyse(wav: &Wav, name: &str) -> Report {
    let fs = wav.sample_rate as f64;
    let mut streams: Vec<Vec<f32>> = (0..wav.channels.max(1) as usize).map(|c| wav.channel(c)).collect();
    // A two-wire tap saved as stereo is the same thing twice.
    if streams.len() >= 2 && same(&streams[0], &streams[1]) {
        streams.truncate(1);
    }
    streams.truncate(2);

    let mut caveats = Vec::new();
    let found: Vec<Vec<setup::Found>> = streams.iter().map(|s| setup::listen(s, fs)).collect();

    let (tap, mut events) = if streams.len() == 2 {
        let caller = caller_channel(&found, &streams, &mut caveats);
        let mut events = Vec::new();
        for (channel, list) in found.iter().enumerate() {
            let end = if channel == caller { End::Caller } else { End::Answerer };
            for f in list {
                let mut event = f.event.clone();
                match event.from {
                    // Either end's to send, so it is the channel that says
                    // whose: unless the other channel has the same thing at
                    // the same moment and louder, and this is its echo.
                    End::Line => {
                        if found[1 - channel].iter().any(|g| echo_of(f, g)) {
                            continue;
                        }
                        event.from = end;
                    }
                    // The other end's signal, come round as echo.
                    other if other != end => continue,
                    _ => {}
                }
                events.push(event);
            }
        }
        (Tap::Split { caller }, events)
    } else {
        caveats.push(
            "both ends are on one channel: the call can be followed until both send in the same band, and no further"
                .to_owned(),
        );
        (Tap::Summed, found.into_iter().flatten().map(|f| f.event).collect())
    };
    let settled = second_phase(&streams, fs, tap, &mut events);
    let mut pcm = None;
    if let Tap::Split { caller } = tap {
        // The analogue modem is the one that sent INFO1a.
        let analogue = events.iter().find(|e| e.tag == "INFO1a").map(|e| e.from);
        if let Some(who) = analogue {
            let channel = |end: End| if end == End::Caller { caller } else { 1 - caller };
            if let Some((found, outcome)) = pcm::follow(
                (&streams[channel(who)], who),
                (&streams[channel(who.other())], who.other()),
                fs,
                &settled,
            ) {
                events.extend(found);
                pcm = Some(outcome);
            }
        }
    } else if settled.info1a_pcm.is_some() {
        caveats.push(
            "a V.90 call on one channel: past phase 2 the server's signal covers the modem's, and neither is followed"
                .to_owned(),
        );
    }
    events.sort_by(|a, b| a.start.total_cmp(&b.start));

    let channels = streams
        .iter()
        .enumerate()
        .map(|(index, samples)| Channel {
            from: match tap {
                Tap::Summed => End::Line,
                Tap::Split { caller } if caller == index => End::Caller,
                Tap::Split { .. } => End::Answerer,
            },
            waterfall: picture::waterfall(samples, fs, 4000.0),
            level: picture::level(samples, fs),
            samples: samples.clone(),
        })
        .collect();

    let mut report = Report {
        name: name.to_owned(),
        sample_rate: wav.sample_rate,
        seconds: streams.first().map_or(0.0, |s| s.len() as f64 / fs),
        tap,
        channels,
        events,
        verdict: Vec::new(),
        caveats,
    };
    verdict(&mut report);
    if let Some(outcome) = pcm {
        if let Some(rate) = outcome.downstream_bps {
            report.verdict.push(("downstream".to_owned(), format!("{rate} bit/s, from CP")));
        }
        if let Some(mask) = outcome.upstream_rates {
            let top = (0..13).rev().find(|bit| mask >> bit & 1 == 1).map(|bit| 4800 + 2400 * bit);
            if let Some(top) = top {
                report.verdict.push(("upstream".to_owned(), format!("up to {top} bit/s enabled in CP")));
            }
        }
        report.verdict.push((
            "data mode".to_owned(),
            if outcome.reached_data { "reached".to_owned() } else { "not reached on the recording".to_owned() },
        ));
    }
    report
}

/// Phase 2, on whichever channel each end is.
fn second_phase(streams: &[Vec<f32>], fs: f64, tap: Tap, events: &mut Vec<Event>) -> phase2::Settled {
    use datapump::v34::dpsk::Side;
    // Nothing of phase 2 comes before V.8 is over.
    let from = events
        .iter()
        .filter(|e| matches!(e.layer, Layer::V8 | Layer::V8bis | Layer::Tone))
        .map(|e| e.end)
        .fold(0.0, f64::max)
        - 0.5;
    let from = from.max(0.0);
    let offered_pcm = |tag: &str, what: &str| {
        events.iter().any(|e| e.tag == tag && e.fields.iter().any(|(n, v)| n == "PCM modem" && v.contains(what)))
    };
    let quick = events.iter().any(|e| e.tag.starts_with("QC") || e.tag == "ANSpcm" || e.tag == "V.92");
    // Which end is the digital modem, if either: it takes the call modem's
    // carrier in phase 2 whoever dialled.
    let caller_digital = offered_pcm("CM", "digital") && offered_pcm("JM", "analogue");
    let pcm = quick || offered_pcm("JM", "digital") || caller_digital;
    let caller_side = if pcm && !caller_digital { Side::Answer } else { Side::Call };

    let run = |caller_side: Side, pcm: bool| {
        let mut found = Vec::new();
        let mut settled = phase2::Settled::default();
        let other = |s: Side| if s == Side::Call { Side::Answer } else { Side::Call };
        match tap {
            Tap::Split { caller } => {
                for (channel, samples) in streams.iter().enumerate() {
                    let (side, who) = if channel == caller {
                        (caller_side, End::Caller)
                    } else {
                        (other(caller_side), End::Answerer)
                    };
                    let (e, s) = phase2::listen(samples, fs, side, who, pcm, from);
                    found.extend(e);
                    settled.merge(s);
                }
            }
            Tap::Summed => {
                for (side, who) in [(caller_side, End::Caller), (other(caller_side), End::Answerer)] {
                    let (e, s) = phase2::listen(&streams[0], fs, side, who, pcm, from);
                    found.extend(e);
                    settled.merge(s);
                }
            }
        }
        (found, settled)
    };
    let (mut found, mut settled) = run(caller_side, pcm);
    // V.8 did not say, or said it where it was not heard: an INFO0d on the
    // answerer's channel is a digital modem whatever was offered.
    let read = |list: &[Event]| list.iter().filter(|e| e.tag.starts_with("INFO")).count();
    if read(&found) == 0 {
        let (again, s) = run(if caller_side == Side::Call { Side::Answer } else { Side::Call }, true);
        if read(&again) > 0 {
            found = again;
            settled = s;
        }
    }
    // On one channel both listeners hear the same probing. The modem on
    // the answering modem's carrier probes first, and the other after it.
    if tap == Tap::Summed {
        let mut starts: Vec<f64> = found.iter().filter(|e| e.tag == "L1").map(|e| e.start).collect();
        starts.sort_by(f64::total_cmp);
        starts.dedup_by(|a, b| (*a - *b).abs() < 0.05);
        let first_prober = if caller_side == Side::Answer { End::Caller } else { End::Answerer };
        found.retain(|e| {
            if !matches!(e.tag.as_str(), "L1" | "L2") {
                return true;
            }
            let turn = starts.iter().position(|s| e.start >= *s - 0.05 && e.start < *s + 0.25).unwrap_or(0);
            let whose = if turn % 2 == 0 { first_prober } else { first_prober.other() };
            e.from == whose
        });
    }
    // The probing fills the band, so each end's is on the other's channel
    // as echo and reads as probing there too. The fainter of two at once is
    // the echo.
    let probes: Vec<(f64, End, f64)> =
        found.iter().filter(|e| e.tag == "L1").map(|e| (e.start, e.from, e.level)).collect();
    let echoes: Vec<(f64, End)> = probes
        .iter()
        .filter(|(at, from, level)| {
            probes.iter().any(|(other, whose, louder)| whose != from && (other - at).abs() < 0.3 && louder > level)
        })
        .map(|&(at, from, _)| (at, from))
        .collect();
    found.retain(|e| {
        !(matches!(e.tag.as_str(), "L1" | "L2")
            && echoes.iter().any(|&(at, from)| from == e.from && e.start >= at - 0.01 && e.start < at + 0.2))
    });
    events.extend(found);
    settled
}

/// Whether `this` is `that` again, fainter, on the other channel.
fn echo_of(this: &setup::Found, that: &setup::Found) -> bool {
    this.event.tag == that.event.tag
        && this.event.octets == that.event.octets
        && (this.event.start - that.event.start).abs() < 0.4
        && this.level < that.level
}

/// Whether two channels are one signal recorded twice.
fn same(a: &[f32], b: &[f32]) -> bool {
    let n = a.len().min(b.len());
    if n == 0 {
        return true;
    }
    let (mut aa, mut bb, mut ab) = (0.0f64, 0.0f64, 0.0f64);
    for (&x, &y) in a.iter().zip(b) {
        aa += (x as f64) * (x as f64);
        bb += (y as f64) * (y as f64);
        ab += (x as f64) * (y as f64);
    }
    aa > 0.0 && bb > 0.0 && ab / (aa * bb).sqrt() > 0.98
}

/// Which channel is the calling modem's.
///
/// By what was heard and how loud: the answer tone and JM belong to the
/// answering modem, and are loudest on its own channel however much of them
/// comes back on the other as echo; CI, CM and CJ likewise the caller's.
fn caller_channel(found: &[Vec<setup::Found>], streams: &[Vec<f32>], caveats: &mut Vec<String>) -> usize {
    // How much more channel 0 looks like the answerer than channel 1 does.
    let mut score = 0.0;
    for (channel, list) in found.iter().enumerate() {
        for f in list {
            let weight = f.level * f.event.duration().clamp(0.05, 2.0);
            let sign = match (f.event.from, channel) {
                (End::Answerer, 0) | (End::Caller, 1) => 1.0,
                (End::Answerer, _) | (End::Caller, _) => -1.0,
                (End::Line, _) => 0.0,
            };
            score += sign * weight;
        }
    }
    if score > 0.0 {
        return 1;
    }
    if score < 0.0 {
        return 0;
    }
    // Nothing to go by: the end that makes the first sound placed the call
    // more often than not.
    let first = |s: &Vec<f32>| s.iter().position(|x| x.abs() > 0.01).unwrap_or(usize::MAX);
    caveats.push("no answer tone or V.8 to tell the ends apart by: the caller is a guess".to_owned());
    if first(&streams[1]) < first(&streams[0]) { 1 } else { 0 }
}

/// The call in a few lines, from the events.
fn verdict(report: &mut Report) {
    let mut lines: Vec<(String, String)> = Vec::new();
    let mut add = |name: &str, value: String| lines.push((name.to_owned(), value));
    if let Some(e) = report.events.iter().find(|e| e.layer == Layer::Tone && e.from == End::Answerer) {
        add("answer tone", format!("{} at {:.2} s, {:.2} s long", e.tag, e.start, e.duration()));
    }
    let bis: Vec<&str> = report.events.iter().filter(|e| e.layer == Layer::V8bis).map(|e| e.tag.as_str()).collect();
    add("V.8 bis", if bis.is_empty() { "none".to_owned() } else { bis.join(", ") });
    for (tag, name) in [("CM", "caller offers"), ("JM", "answerer takes")] {
        if let Some(e) = report.find(tag) {
            add(name, e.summary.clone());
        }
    }
    for tag in ["QC1a", "QC1d", "QCA1a", "QCA1d"] {
        if let Some(e) = report.find(tag) {
            add("quick connect", format!("{tag}: {}", e.summary));
        }
    }
    for tag in ["INFO0d", "INFO0c", "INFO0a", "INFO1a"] {
        if let Some(e) = report.find(tag) {
            add(tag, e.summary.clone());
        }
    }
    // Tone A turns, and tone B turns 40 ms after the turn reaches it: so
    // from one to the other on the recording, less the 40 ms, is the way
    // from the tap to the modem sending B and back.
    let turn = |tag: &str, after: f64| report.events.iter().find(|e| e.tag == tag && e.start > after).map(|e| e.start);
    if let Some(a) = turn("A rev", 0.0)
        && let Some(b) = turn("B rev", a)
        && b - a < 2.0
    {
        add(
            "round trip",
            format!("{:.0} ms from the recording point to the modem sending tone B and back", 1000.0 * (b - a - 0.040)),
        );
    }
    if let Some(e) = report.events.iter().find(|e| e.tag == "L2" && e.summary.contains("median")) {
        add("probing", e.summary.clone());
    }
    report.verdict = lines;
}
