//! Spectral shaping from the analogue modem's side (V.90 5.4.5, 8.5.2):
//! whether to ask the digital modem for it, and what to ask for.
//!
//! A downstream codeword goes out with a sign, and the signs are where V.90
//! keeps a little room. With shaping on, Sr of each data frame's six signs
//! carry no data (5.4.5), and the digital modem spends them choosing, frame by
//! frame, between sign patterns that carry the same data -- whichever keeps
//! the signal weakest through a filter the analogue modem names. Appendix I
//! says what for: "to help the analogue modem combat the effects of the
//! transformers and filters used in the digital-to-analogue conversion", and
//! "it is the analogue modem that requests the spectral shaping parameters ...
//! and so the optimum spectral shape is implementation dependent".
//!
//! What this end has to go on is what its own equaliser leaves of TRN1d and
//! Jd ([`super::pcm::Residue`]): a response, lag by lag, of the error to the
//! symbols sent, and noise. A path that takes the top of the band away leaves
//! a response that rings at the top of the band -- no equaliser gives back a
//! band that is not there -- and a signal with next to nothing up there
//! excites it next to nothing. So each shape worth asking for is tried the
//! only way that answers the question: the digital modem's own encoder,
//! shaper and all, makes a stretch of what it would send, and the stretch is
//! put through the response. What comes out is what that shape would leave,
//! against what TRN1d left.
//!
//! None of it is free. Every sign spent on shaping is a bit a frame the data
//! does not get -- a step down the rate ladder, if the constellations cannot
//! make it up -- so the choice is made on rates in the end: the fastest the
//! route carries with the error the shape would leave, against the fastest it
//! carries without.

use super::INTERVALS;
use super::dil::{self, Choice, Route};
use super::encoder::{Encoder, Mapping};
use super::pcm::Leftover;
use super::sequences::{self, Cp};
use super::sign::Redundancy;
use super::ucode::{self, Law};

/// What CP and CPt ask of the digital modem's shaper: Sr in bits 31:32, ld in
/// 49:50, and a1, a2, b1 and b2 of 5.4.5.6's filter in 69:76, 77:84, 86:93
/// and 94:101, "signed Q1.6" (Table 14).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Shaping {
    pub redundancy: Redundancy,
    pub lookahead: u8,
    pub filter: [i8; 4],
}

impl Shaping {
    /// "When Sr = 0, spectral shaping is disabled" (5.4.5).
    pub const NONE: Self = Self { redundancy: Redundancy::None, lookahead: 0, filter: [0; 4] };

    /// Put this request into a CP or a CPt.
    pub fn apply(&self, cp: &mut Cp) {
        cp.redundancy = self.redundancy;
        cp.lookahead = self.lookahead;
        cp.shaping = self.filter;
    }

    /// The request a CP makes.
    pub fn of(cp: &Cp) -> Self {
        Self { redundancy: cp.redundancy, lookahead: cp.lookahead, filter: cp.shaping }
    }
}

/// Data frames of the digital modem's output each shape is tried on: half a
/// second, over which what the response makes of it settles to a few percent.
const TRIED_FRAMES: usize = 700;

/// The shaping filters tried, as a1, a2, b1 and b2 in Q1.6 (5.4.5.6).
///
/// T(z) = (1 - a1 z^-1)(1 - a2 z^-1) / ((1 - b1 z^-1)(1 - b2 z^-1)) is the
/// shape asked for, and each of these has its zero where a band-edge cut
/// takes the band away: a1 = -1, z = -1, which is 4 kHz, or just inside it.
/// A pole behind the zero, b1, keeps the notch to the top of the band; the
/// wider notch without one takes more away from the band the line does
/// carry. A second zero at 4 kHz, a2 = -1 too, asks for more than a sign in
/// six can give, and leaves more than one zero does.
const FILTERS: [[i8; 4]; 4] = [[-64, 0, 0, 0], [-64, 0, -32, 0], [-64, 0, -48, 0], [-60, 0, -40, 0]];

/// What a request would cost and give: the choice it makes, and how much of
/// the error TRN1d showed it would leave, as a share.
#[derive(Debug, Clone, PartialEq)]
pub struct Asked {
    pub choice: Choice,
    pub shaping: Shaping,
    pub left: f64,
}

impl Asked {
    /// The downstream rate it asks for.
    pub fn rate(&self) -> u32 {
        sequences::data_rate(self.choice.data.drn).unwrap_or(0)
    }
}

/// The choice at the end of the DIL: [`choose`]'s, one rung slower at a time
/// until its levels stand [`dil::SLACK`] times [`dil::SPACING`] of the error
/// the route leads data mode to expect -- its spread at data mode's power,
/// and of that what the shaping asked for is to leave.
///
/// [`choose`] stands them [`dil::SPACING`] apart and then as far apart as
/// Table 15's power lets it, which on every simulated line here is further;
/// but a route whose ladder runs out before its power does -- every rung in
/// use, at exactly that spacing -- comes out with none of that to spare, and
/// everything then rests on a single 0.44 s pass of the DIL having read the
/// spread data mode will meet. Only here: a renegotiation is chosen on what
/// data mode itself has found of the line.
///
/// None if nothing slow enough has the room, or nothing carries V.90's
/// slowest rate at all.
pub fn choose_with_slack(
    route: &Route,
    law: Law,
    limit: u32,
    enabled: impl Fn(u8) -> bool,
    most_lookahead: u8,
    leftover: Option<&Leftover>,
) -> Option<Asked> {
    let noise = route.noise_at(law, f64::from(limit) / 32768.0);
    let mut below = u8::MAX;
    loop {
        let asked = choose(route, law, limit, |drn| drn < below && enabled(drn), most_lookahead, leftover)?;
        let expected = noise * asked.left.sqrt();
        if dil::least_gap(&asked.choice.data, route) >= dil::SLACK * dil::SPACING * expected {
            return Some(asked);
        }
        below = asked.choice.data.drn;
    }
}

/// A route whose errors are `share` of the power they were.
pub fn scaled(route: &Route, share: f64) -> Route {
    let mut scaled = route.clone();
    for spread in scaled.spread.iter_mut() {
        *spread *= share.sqrt();
    }
    scaled
}

/// Choose CP and CPt for a route, shaped or not: whichever carries the most.
///
/// Unshaped, the route is taken as the DIL read it. Shaped, its errors are
/// taken to shrink as TRN1d's would: the noise stays, and what the symbols
/// carried is what the shaped signal would carry through `leftover`'s
/// response. Each Sr is tried with the filter that leaves least, at the
/// deepest look-ahead the digital modem's Jd offers (`most_lookahead`, bits
/// 49:50 of Table 13; CP's ld "shall be consistent with" it). Shaping wins
/// only by carrying more; on a tie, the signs go to data.
///
/// None if nothing carries V.90's slowest rate.
pub fn choose(
    route: &Route,
    law: Law,
    limit: u32,
    enabled: impl Fn(u8) -> bool,
    most_lookahead: u8,
    leftover: Option<&Leftover>,
) -> Option<Asked> {
    let unshaped = dil::choose(route, law, limit, &enabled).map(|choice| Asked { choice, shaping: Shaping::NONE, left: 1.0 });
    let Some(leftover) = leftover else { return unshaped };
    // The magnitudes the shaper would be choosing signs for: the unshaped
    // choice's, or failing that, what a route half as noisy would carry.
    let base = unshaped.as_ref().map(|a| a.choice.clone()).or_else(|| dil::choose(&scaled(route, 0.25), law, limit, &enabled))?;
    let base = Mapping::from_cp(&base.data)?;
    let lookahead = most_lookahead.min(3);
    let power = leftover.power;
    // What the response leaves of a signal, less what reading it put there.
    let carried = |mapping: &Mapping| (left(&leftover.response, mapping, law) - leftover.misread).max(0.0) * power;
    let before = leftover.noise + carried(&base);
    if before <= 0.0 {
        return unshaped;
    }
    let mut best = unshaped;
    for redundancy in [Redundancy::One, Redundancy::Two, Redundancy::Three] {
        let tried = FILTERS.map(|filter| {
            let shaping = Shaping { redundancy, lookahead, filter };
            let mapping = Mapping { redundancy, lookahead: usize::from(lookahead), shaping: filter, ..base.clone() };
            (shaping, (leftover.noise + carried(&mapping)) / before)
        });
        let Some(&(shaping, share)) = tried.iter().min_by(|a, b| a.1.total_cmp(&b.1)) else { continue };
        if share >= 1.0 {
            continue;
        }
        let Some(choice) = dil::choose_shaped(&scaled(route, share), law, limit, &enabled, shaping) else { continue };
        let asked = Asked { choice, shaping, left: share };
        if best.as_ref().is_none_or(|b| asked.rate() > b.rate()) {
            best = Some(asked);
        }
    }
    best
}

/// The share of a signal's power that `response` (lag `-L` first) leaves in
/// the error, for the signal the digital modem would send with `mapping`.
///
/// The digital modem's encoder makes the signal, from scrambled data, so that
/// the shaper sees the magnitudes it would see and chooses as it would choose.
pub fn left(response: &[f64], mapping: &Mapping, law: Law) -> f64 {
    let lags = response.len() / 2;
    let mut encoder = Encoder::new(mapping.clone(), law);
    let mut x = 0x2545_f491_4f6c_dd1du64;
    let mut sent: Vec<f64> = Vec::with_capacity(TRIED_FRAMES * INTERVALS);
    for _ in 0..TRIED_FRAMES {
        let frame = encoder.next_frame(|| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x & 1 == 1
        });
        for i in 0..INTERVALS {
            let level = ucode::level(law, frame.ucodes[i]);
            sent.push(if frame.positive[i] { level } else { -level });
        }
    }
    let (mut error, mut power) = (0.0, 0.0);
    for m in lags..sent.len() - lags {
        // response[j] is lag j - L: the symbol j - L before this one.
        let e: f64 = response.iter().enumerate().map(|(j, c)| c * sent[m + lags - j]).sum();
        error += e * e;
        power += sent[m].powi(2);
    }
    if power > 0.0 { error / power } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v90::modulus::Constellation;
    use crate::v90::pcm::RESIDUE_LAGS;

    /// A response that rings at the top of the band, as what an equaliser
    /// leaves of a band-edge cut does: a tone at 4 kHz dying away either side.
    fn ring(size: f64) -> Vec<f64> {
        let lags = RESIDUE_LAGS as i64;
        (-lags..=lags).map(|k| size * if k % 2 == 0 { 1.0 } else { -1.0 } * (-(k.abs() as f64) / 6.0).exp()).collect()
    }

    fn mapping(redundancy: Redundancy, lookahead: usize, filter: [i8; 4]) -> Mapping {
        let sets: [Constellation; INTERVALS] = std::array::from_fn(|_| Constellation::new((30..100).step_by(2).collect()));
        Mapping { redundancy, lookahead, shaping: filter, ..Mapping::best(sets, Redundancy::None) }
    }

    /// 5.4.5.6: with a1 = -1 the filter the digital modem's metric runs on
    /// is strongest at 4 kHz, and the signs it chooses leave the top of the
    /// band nearly empty -- which is where a band-edge cut rings.
    #[test]
    fn a_zero_at_four_kilohertz_leaves_little_of_a_ring_there() {
        let response = ring(0.01);
        let unshaped = left(&response, &mapping(Redundancy::None, 0, [0; 4]), Law::Mu);
        let white: f64 = response.iter().map(|c| c * c).sum();
        assert!((unshaped / white - 1.0).abs() < 0.1, "{unshaped} against {white}");
        for redundancy in [Redundancy::One, Redundancy::Two, Redundancy::Three] {
            let shaped = left(&response, &mapping(redundancy, 1, [-64, 0, -32, 0]), Law::Mu);
            println!("Sr {}: {:.1} dB", redundancy.spent(), 10.0 * (shaped / unshaped).log10());
            assert!(shaped < 0.5 * unshaped, "Sr {}: {shaped} against {unshaped}", redundancy.spent());
        }
    }

    /// Signs are all shaping moves: what a response leaves at lag 0 alone --
    /// a gain a little out -- is the same share of the signal however the
    /// signs are chosen.
    #[test]
    fn shaping_cannot_move_what_is_left_at_lag_zero() {
        let mut response = vec![0.0; 2 * RESIDUE_LAGS + 1];
        response[RESIDUE_LAGS] = 0.02;
        let unshaped = left(&response, &mapping(Redundancy::None, 0, [0; 4]), Law::Mu);
        let shaped = left(&response, &mapping(Redundancy::Two, 1, [-64, 0, -32, 0]), Law::Mu);
        assert!((shaped / unshaped - 1.0).abs() < 1e-9, "{shaped} against {unshaped}");
    }

    fn leftover(response: Vec<f64>, noise: f64) -> Leftover {
        Leftover { response, noise, power: ucode::level(Law::Mu, 79).powi(2), misread: 0.0 }
    }

    /// Nothing measured, or nothing but noise: nothing asked for, and the
    /// choice the unshaped one.
    #[test]
    fn noise_alone_asks_for_no_shaping() {
        let route = Route::clean(Law::Mu, 3e-4);
        let unshaped = dil::choose(&route, Law::Mu, 15124, |_| true).unwrap();
        let none = choose(&route, Law::Mu, 15124, |_| true, 1, None).unwrap();
        assert_eq!(none.choice, unshaped);
        assert_eq!(none.shaping, Shaping::NONE);
        let noise = leftover(vec![0.0; 2 * RESIDUE_LAGS + 1], 1e-7);
        let quiet = choose(&route, Law::Mu, 15124, |_| true, 1, Some(&noise)).unwrap();
        assert_eq!(quiet.choice, unshaped);
        assert_eq!(quiet.shaping, Shaping::NONE);
    }

    /// A route whose error is mostly a ring at the top of the band: shaping
    /// is asked for, with a zero at or next to 4 kHz and the look-ahead the
    /// digital modem offers, and it carries more than the signs it costs.
    #[test]
    fn a_ring_at_the_top_of_the_band_is_shaped_away() {
        let route = Route::clean(Law::Mu, 7e-4);
        let unshaped = dil::choose(&route, Law::Mu, 4024, |_| true).unwrap();
        let rate = |c: &Choice| sequences::data_rate(c.data.drn).unwrap();
        let ringing = leftover(ring(0.006), 2e-9);
        let asked = choose(&route, Law::Mu, 4024, |_| true, 1, Some(&ringing)).unwrap();
        println!("{} unshaped, {} with {:?}, leaving {:.2}", rate(&unshaped), asked.rate(), asked.shaping, asked.left);
        assert_ne!(asked.shaping.redundancy, Redundancy::None);
        assert!(asked.shaping.filter[0] <= -56, "{:?}", asked.shaping);
        assert_eq!(asked.shaping.lookahead, 1);
        assert!(asked.rate() > rate(&unshaped));
        // Both CPs ask for it, and each is a mapping the digital modem can
        // send: S + Sr = 6, and K as Table 2 and Table 17 have it.
        for cp in [&asked.choice.data, &asked.choice.training] {
            assert_eq!(Shaping::of(cp), asked.shaping);
            let mapping = Mapping::from_cp(cp).expect("not a mapping");
            assert_eq!(mapping.redundancy, asked.shaping.redundancy);
        }
        assert!(Mapping::from_cp(&asked.choice.data).unwrap().valid());
    }

    /// The choice at the end of the DIL leaves room: where [`choose`]'s pick
    /// has its levels [`dil::SLACK`] times [`dil::SPACING`] of the expected
    /// error apart it stands, and where it has less -- a ladder that ran out
    /// at the spacing, with no power to spare it more -- the rate comes down
    /// until the room is there, and no further than the first rate that has
    /// it. Across a sweep of clean routes, which land on either side.
    #[test]
    fn the_choice_at_the_end_of_the_dil_leaves_room_or_comes_down_until_it_does() {
        let (mut kept, mut lowered) = (0, 0);
        for step in 0..60 {
            let noise = 1e-4 * 1.03f64.powi(step);
            let route = Route::clean(Law::Mu, noise);
            let room = |a: &Asked| dil::least_gap(&a.choice.data, &route) / (dil::SPACING * route.noise_at(Law::Mu, 4024.0 / 32768.0) * a.left.sqrt());
            let Some(best) = choose(&route, Law::Mu, 4024, |_| true, 1, None) else { continue };
            let slack = choose_with_slack(&route, Law::Mu, 4024, |_| true, 1, None).expect("nothing with room");
            assert!(room(&slack) >= dil::SLACK, "noise {noise:.2e}: {} with room {:.3}", slack.rate(), room(&slack));
            if room(&best) >= dil::SLACK {
                assert_eq!(slack, best, "noise {noise:.2e}: {} had room {:.3}", best.rate(), room(&best));
                kept += 1;
            } else {
                assert!(slack.rate() < best.rate(), "noise {noise:.2e}");
                // The first rate down that has the room, not one further.
                let between = choose(&route, Law::Mu, 4024, |drn| drn > slack.choice.data.drn && drn < best.choice.data.drn, 1, None);
                assert!(between.as_ref().is_none_or(|b| room(b) < dil::SLACK), "noise {noise:.2e}: {:?} had room too", between.map(|b| b.rate()));
                lowered += 1;
            }
        }
        println!("{kept} routes kept their rate and {lowered} came down");
        assert!(kept > 0 && lowered > 0, "{kept} kept, {lowered} came down");
    }
}
