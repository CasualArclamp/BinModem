# WP-G: the half-duplex modem (V.34 clause 12), one end of a Super G3 call

Branch `wp/g`, on `superg3` with D merged. `crates/datapump/src/v34/halfduplex.rs`, `halfduplex/tests.rs`, the
`pub mod` line in `v34.rs`, and this note. **No other module was changed**, additively or otherwise.

**The API** (`v34::halfduplex`).
- `Modem::new(role, source: bool, fs)` from the 75 ms of silence that end V.8: INFO0 on the first step, then phase 2
  (`phase2h`), phase 3 and the control channel start-up by themselves. `Modem::after_phase2(role, source, fs,
  Setup { infoh, ours, far })` starts at phase 3 for whoever ran phase 2 elsewhere (the tests, mostly).
- `step(input) -> output` every sample: the control transmitter, the primary source and phase 2's or a recovery's
  tone, summed. `state()`: `Starting | Control | ToPrimary | Primary | ToControl | Retraining | Failed` (plan 10.2);
  `phase()` for the window; `failure()`. `event()`: `Phase2Over`, `Phase3Over { well }`, `ControlUp` (T.30's
  `control_restarted()`, before any bit is taken), `PageStarted { b1_errors }` (109 on), `PageEnded` (109 off),
  `Retraining` (either channel, either end), `Failed(why)`.
- Control: `send_control_bits(&[bool]) -> bool` (false, none taken, off the channel), `pending_control_bits()`,
  `take_control_bits()` (descrambled, after the far E, only while up), `control_carrier()`, `far_silent()` (the far
  carrier 12 dB under its level on the channel, or off, for 100 ms). Primary: `send_page_bits -> bool`,
  `pending_page_bits()` (source); `take_page_bits()`, `page_carrier()` (recipient).
- Turns: `to_primary()` (12.6.3's 4T, then 12.5.1 or listening); `to_control(renegotiate)` (12.5.3.1, then 12.6.1 or
  12.4.1.1; at the recipient a wish for a change, kept until the source's Sh is answered, 12.6.2.3). Retrains:
  `retrain_control()` (12.8.1), `retrain_primary()` (12.7, from the control channel only). Knobs: `limit_rate(bps)`,
  `ask_control_rate(ControlRate)`, `send_pph_as(Reading)` (plan 8.1). Readings: `primary_rate()` (after every MPh),
  `data_mode()`, `control_rates()`, `mph()`, `infoh()`, `channel()`, `capabilities()`, `far_capabilities()`,
  `phase2()`, `control()`, `primary_source()`, `primary_recipient()`, `primary_snr_db()`, `recoveries()` (12.3.3),
  `retrains()`.

**The state machine.** `Stage`: `Phase2 -> Phase3 -> Awaiting(FirstPph) -> Mph -> AwaitE -> Control -> TurningOff
-> Page -> Awaiting(ShOrPph) -> AwaitE -> Control ...`, with `Recovering` between phase 3 and the tones (12.3.3,
12.4.3.1), `Awaiting(Pph)` for a start-up after a page (12.6.1.1 -> 12.4.1.1), `Awaiting(ChangePph)` for a recipient
that answered Sh with PPh (12.6.2.3), `Awaiting(Ac | Responding)` for 12.8, `Failed`. `Turn` (`First | Page |
Retrain`) is what the join is told a start-up is. Every wait in C.0's table has its deadline: PPh, MPh, E, Sh within
3 s, then AC (12.8.1); AC unanswered three rounds of 3 s, then `Failed` (plan 8.4); TRN not well or no S in 2 s,
the recipient's tone (12.3.3) up to three times; the source waits 5 s for INFOh after answering the tone.

**Departures, and why.**
1. **The 12.7 initiator is deaf until the far control carrier has fallen 12 dB** below its level on the channel. The
   far end sends for a round trip more, four-point symbols three transitions in four of which turn the phase a
   quarter or not at all: `phase2h`'s `Steady` took them for a tone and a half turn for its reversal, and the source
   began its probe before the far tone had reached it. 12 dB is 28 ms after the far end stops, 40 before its tone,
   past any 20 ms jitter hole; the absolute carrier-off (V.32's 59 dB) was 100 ms, past the tone and its reversal.
   A far end that never answers leaves phase 2 to its 20 s cap, and the modem `Failed`.
2. **`far_silent()` is judged against the far carrier's own level** too, as the page's end is (wp-e): on a noisy line
   (27 dB here) the absolute carrier-off never came. Held 100 ms (128 ms after the far end stops), not 50: a far end
   beginning 12.7 is silent 70 ± 5 ms first, which at 50 read as the recipient's turn: a page went into a retrain.
3. **The far 12.7 tone is judged by the control receiver's watch** (`Hearing::Tone` held 50 ms), not a `Steady`: the
   watch wants lags 2, 4 and 8 periodic and lag 1 at +1, which scrambled data all but never gives; answered 84 ms
   after the tone arrives. Only on the control channel and in start-ups and resyncs after a page: straight after
   phase 3 the tone is 12.3.3's recovery.
4. **Type 0 MPh from both ends**, the recipient's asking neither precoding nor non-linear encoding (wp-e item 4);
   `max_rate` never below the least rate Table 8 has at the symbol rate (a cap of 2400 at 3200 Bd asks 4800).
5. E sequences side by side (Fig. 27 inset), the source's only after the recipient's Sh and S-bar-h; a PPh in data
   with no AC first answered as a start-up; `State::Retraining` from a retrain's first silence to `ControlUp`.
6. **Ignored test, a receiver corner** (`receiver.rs` or `primary.rs`): 3000 Bd, high carrier, exactly 30.000 ms each
   way, 40 ppm, 44 dB: the third page comes with a mapping frame of ones in front; 1 ms more delay and it is whole.

**Tests** (`halfduplex::tests`: 17, one ignored; 8 s in release). Pairs driven by a stand-in for Annex F (frames both
ways, forty ones, the page, round again), every page bit checked: from phase 3 at every symbol rate, either end the
source, three pages of 30 000 bits at 21 600 to 33 600, 40 ppm, 41-46 dB; the VoIP line (750 ms each way, own echo
10 dB down, 50 ppm) at 33 600 both ways, no frame sent twice; from V.8's silence through phase 2 on both lines (phase
2 over at 1.13 / 5.44 s, channel up at 2.95 / 10.17 s, 3429 Bd, 16-point TRN, 33 600) and on a noisy one (27 dB:
trained to 31.2, 26 400, every page whole); a 20 ms slip in a page (dropped, inserted) and in the control channel;
`to_control(true)` (33 600 -> 31 200); the recipient's change by PPh (28 800 -> 21 600); 12.8 from either end and
both at once; 12.7 from either end, either modem the source, on both lines (channel back 3.1 / 10.3 s after); 12.3.3
through a hole that eats S; a page that never trained ending in the Sh the control receiver hears; 2400 bit/s control
when both ask; AC unanswered -> `Failed` at 3.5 + 9 s; a cap under the floor. `far_silent()` 40-300 ms after the
recipient's turn plus the delay; no control bit delivered off the channel or before `ControlUp`.

**For H3 (the join).** At `v34_agreed()` (wp-h1.md: 75 ms after CJ at both ends, `start_annex_f()` run) build
`halfduplex::Modem::new(Role::Call | Role::Answer, source, fs)` -- `source` the calling end unless it polls, `fs`
the live path's 16 000 (the only rate tested) -- and the modem owns the line: `out = modem.step(in)` every sample.
Each tick: drain `event()` (`ControlUp` -> `control_restarted()`, `set_primary_rate(primary_rate())`; `PageStarted` /
`PageEnded` -> `set_fast_carrier`; `Failed` -> hang up); in `State::Control` with `pending_control_bits() < 16`,
`next_control_bit` into `send_control_bits`, never a bit it would refuse; `take_control_bits()` to `control_bit`;
`set_far_silent(far_silent())`. Source: `V34Ones -> V34Primary` is `to_primary()` (better on `far_silent()` than on
T.30's flags-gone, which can fire inside a far 12.7 retrain's 70 ms of silence); once `state() == Primary`,
`send_page_bits` from `next_fast_bit`, a few hundred pending; `V34Primary -> V34Control` is
`to_control(call.renegotiate())` after the last bit. Recipient: `V34PrimaryListen` is `to_primary()` once;
`take_page_bits()` -> `fast_bits` while `Primary`; the channel comes back by itself; a new rate: `limit_rate` and
`to_control(true)` at `PageEnded`; a DCN while the channel is down: `retrain_control()`.
