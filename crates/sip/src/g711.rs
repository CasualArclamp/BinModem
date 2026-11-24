//! G.711: the eight-bit companded samples a telephone network actually carries.
//!
//! Every other codec in a softphone's list is a speech coder -- it models a
//! voice tract and throws away what a voice would not have put there -- and a
//! modem signal put through one arrives as noise. G.711 is not a speech coder.
//! It is a logarithmic requantisation of a linear sample, one codeword per
//! sample at 8000 a second, and it is what the trunk carries end to end. So
//! these two laws are the only payloads this modem will ever offer.
//!
//! Which matters more here than in a telephone. A V.90 server chooses its
//! output *from this alphabet*: the levels it sends are codewords, and a
//! receiver that gets the codewords back unaltered is reading the far end's
//! own symbols rather than a reconstruction of them. Every stage between the
//! network and the modem that resamples, mixes or re-encodes destroys that,
//! which is the whole argument for this crate existing.
//!
//! The decoders here are the segment reconstruction the tables in G.711 5.1
//! and 5.2 amount to; the encoders are its inverse. `tests` checks the pairing
//! exhaustively in both directions rather than checking either against a table
//! copied from somewhere, because the property that matters is that a codeword
//! survives a round trip unchanged.

/// The mu-law payload type, fixed by RFC 3551 Table 4.
pub const PCMU: u8 = 0;
/// And A-law's.
pub const PCMA: u8 = 8;

/// Which companding law a call settled on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Law {
    /// mu-law: North America and Japan, and what a V.90 server's codewords are.
    Mu,
    /// A-law: most of the rest of the world, Australia included.
    A,
}

impl Law {
    /// The RFC 3551 payload type number this law is carried as.
    pub fn payload_type(self) -> u8 {
        match self {
            Self::Mu => PCMU,
            Self::A => PCMA,
        }
    }

    /// The name it goes by in an SDP rtpmap.
    pub fn encoding_name(self) -> &'static str {
        match self {
            Self::Mu => "PCMU",
            Self::A => "PCMA",
        }
    }

    /// The law a static payload type number means, if it is one of ours.
    pub fn from_payload_type(pt: u8) -> Option<Self> {
        match pt {
            PCMU => Some(Self::Mu),
            PCMA => Some(Self::A),
            _ => None,
        }
    }

    pub fn decode(self, code: u8) -> i16 {
        match self {
            Self::Mu => ulaw_decode(code),
            Self::A => alaw_decode(code),
        }
    }

    pub fn encode(self, sample: i16) -> u8 {
        match self {
            Self::Mu => ulaw_encode(sample),
            Self::A => alaw_encode(sample),
        }
    }
}

/// The value added before the segment is found and taken off after, which is
/// what makes the first segment's steps line up with the second's. G.711 does
/// not name it; every implementation calls it the bias.
const BIAS: i32 = 0x84;
/// The largest magnitude mu-law can represent, so the largest worth offering
/// it. Above this the codeword is the same and only the error grows.
const MU_CLIP: i32 = 32635;
/// A-law's, which is the whole range: its segments start coarser and so reach
/// further with the same eight bits.
const A_CLIP: i32 = 32767;

/// Which segment a magnitude falls in: the exponent, found by asking which
/// doubling of the first segment's width it has got past.
fn segment(magnitude: i32, first: i32) -> usize {
    let mut edge = first;
    for seg in 0..8 {
        if magnitude <= edge {
            return seg;
        }
        edge = (edge << 1) + 1;
    }
    7
}

/// mu-law, G.711 Table 1. The codeword is stored inverted, which is not
/// decoration: it puts the quiet samples -- the frequent ones -- at codewords
/// with the most one bits, so an idle line carries mostly ones and the
/// transmission system under it keeps its timing.
pub fn ulaw_encode(sample: i16) -> u8 {
    let sign = if sample < 0 { 0x80u8 } else { 0 };
    // Negated as an i32: -32768 has no positive counterpart in an i16 and
    // negating it in place would wrap back to itself.
    let mut magnitude = if sample < 0 { -(sample as i32) } else { sample as i32 };
    if magnitude > MU_CLIP {
        magnitude = MU_CLIP;
    }
    magnitude += BIAS;
    let seg = segment(magnitude, 0xFF);
    let quantised = ((magnitude >> (seg + 3)) & 0x0F) as u8;
    // Inverted on the way out, hence the complement.
    !(sign | ((seg as u8) << 4) | quantised)
}

pub fn ulaw_decode(code: u8) -> i16 {
    let code = !code;
    let magnitude = (((code & 0x0F) as i32) << 3) + BIAS;
    let magnitude = (magnitude << ((code & 0x70) >> 4)) - BIAS;
    if code & 0x80 != 0 {
        -(magnitude as i16)
    } else {
        magnitude as i16
    }
}

/// A-law, G.711 Table 2. Alternate bits are inverted rather than all of them,
/// to the same end: a long run of one sample value does not become a long run
/// of one bit pattern.
pub fn alaw_encode(sample: i16) -> u8 {
    // A-law's sign bit is set for *positive*, the opposite of mu-law's. That
    // is the law and not an implementation's choice.
    let sign = if sample < 0 { 0x00u8 } else { 0x80 };
    let mut magnitude = if sample < 0 { -(sample as i32) } else { sample as i32 };
    if magnitude > A_CLIP {
        magnitude = A_CLIP;
    }
    let seg = segment(magnitude, 0xFF);
    let quantised = if seg == 0 {
        ((magnitude >> 4) & 0x0F) as u8
    } else {
        ((magnitude >> (seg + 3)) & 0x0F) as u8
    };
    (sign | ((seg as u8) << 4) | quantised) ^ 0x55
}

pub fn alaw_decode(code: u8) -> i16 {
    let code = code ^ 0x55;
    let mut magnitude = ((code & 0x0F) as i32) << 4;
    let seg = i32::from((code & 0x70) >> 4);
    match seg {
        0 => magnitude += 8,
        1 => magnitude += 0x108,
        _ => {
            magnitude += 0x108;
            magnitude <<= seg - 1;
        }
    }
    if code & 0x80 != 0 {
        magnitude as i16
    } else {
        -(magnitude as i16)
    }
}

/// Decode a packet's worth of codewords onto the scale the modem works in,
/// where one is full scale.
pub fn decode_into(law: Law, payload: &[u8], out: &mut Vec<f32>) {
    out.reserve(payload.len());
    for &code in payload {
        out.push(f32::from(law.decode(code)) / 32768.0);
    }
}

/// And back, clipping rather than wrapping: a modem driven past full scale
/// should sound loud and distorted, the way an overdriven line does, rather
/// than invert its own signal.
pub fn encode_into(law: Law, samples: &[f32], out: &mut Vec<u8>) {
    out.reserve(samples.len());
    for &s in samples {
        let scaled = (f64::from(s) * 32768.0).round().clamp(-32768.0, 32767.0) as i16;
        out.push(law.encode(scaled));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The property that matters on this rig: a codeword that arrives is the
    /// codeword that was sent. A V.90 receiver reading the far end's levels
    /// depends on it exactly, and nothing else in the path can restore it
    /// once it is lost.
    ///
    /// With one exception, which is the law's and not ours. mu-law has two
    /// codewords for zero -- 0xFF and 0x7F, positive and negative -- and they
    /// decode to the same sample, so the pair cannot both come back. The
    /// encoder chooses 0xFF, and 0x7F is the one codeword in the alphabet
    /// that a round trip changes. Worth knowing before reading it as a fault:
    /// a V.90 server that sends 0x7F gets 0xFF back from anything that
    /// decodes and re-encodes, which is one more reason not to have anything
    /// in the path that does.
    #[test]
    fn every_codeword_survives_a_round_trip() {
        /// mu-law's negative zero.
        const MINUS_ZERO: u8 = 0x7F;
        for code in 0..=255u8 {
            if code != MINUS_ZERO {
                assert_eq!(ulaw_encode(ulaw_decode(code)), code, "mu-law {code:#04x}");
            }
            assert_eq!(alaw_encode(alaw_decode(code)), code, "A-law {code:#04x}");
        }
        assert_eq!(ulaw_decode(MINUS_ZERO), 0);
        assert_eq!(ulaw_encode(0), 0xFF);
    }

    /// And the other direction: for every sample an i16 can hold, the level
    /// the encoder chose is either the closest the law has or the one next to
    /// it. Checked against a search over the decoder rather than against a
    /// table, so the two halves cannot agree on being wrong together.
    ///
    /// Not "the closest", because it is not. Inside a segment the encoder
    /// truncates where a search rounds, so near the top of a segment it picks
    /// the level below rather than the one above; both are a step of the
    /// law's own quantisation away and neither is an error in the encoder.
    /// What would be an error is skipping a level, and that is what this
    /// rules out: the two codewords are adjacent in the ordered alphabet.
    #[test]
    fn encoding_picks_a_neighbouring_codeword() {
        for law in [Law::Mu, Law::A] {
            let mut levels: Vec<i32> = (0..=255u8).map(|c| i32::from(law.decode(c))).collect();
            levels.sort_unstable();
            levels.dedup();
            let place = |level: i32| levels.iter().position(|l| *l == level).unwrap();
            for sample in i16::MIN..=i16::MAX {
                let chosen = i32::from(law.decode(law.encode(sample)));
                let best = *levels
                    .iter()
                    .min_by_key(|l| (**l - i32::from(sample)).abs())
                    .unwrap();
                let apart = place(chosen).abs_diff(place(best));
                assert!(
                    apart <= 1,
                    "{law:?} sample {sample}: chose {chosen}, {apart} levels from {best}"
                );
            }
        }
    }

    /// Silence is silence. Not quite true of A-law, whose quietest codeword
    /// decodes to eight rather than nothing: the law has no exact zero, and a
    /// silent line through it carries a very small square wave. Worth knowing
    /// before mistaking it for a fault on a capture.
    #[test]
    fn quiet_is_quiet() {
        assert_eq!(ulaw_decode(ulaw_encode(0)), 0);
        assert!(alaw_decode(alaw_encode(0)).abs() <= 8);
    }

    /// Loud in, loud out, inside the clipping each law does.
    #[test]
    fn loud_stays_loud() {
        assert!(ulaw_decode(ulaw_encode(32000)) > 30000);
        assert!(alaw_decode(alaw_encode(32000)) > 30000);
        assert!(ulaw_decode(ulaw_encode(-32000)) < -30000);
        assert!(alaw_decode(alaw_encode(-32000)) < -30000);
    }

    #[test]
    fn the_float_path_is_the_same_path() {
        let samples: Vec<f32> = (0..64).map(|n| (n as f32 / 32.0 - 1.0) * 0.9).collect();
        let mut codes = Vec::new();
        encode_into(Law::Mu, &samples, &mut codes);
        let mut back = Vec::new();
        decode_into(Law::Mu, &codes, &mut back);
        for (a, b) in samples.iter().zip(back.iter()) {
            assert!((a - b).abs() < 0.02, "{a} became {b}");
        }
    }
}
