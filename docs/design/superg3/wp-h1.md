# WP-H1: V.8 for a fax that does V.34

Branch `wp/h1`. T.30 clause 6 in `FaxCall`, on `datapump::v8`'s line modem:
ANSam for the called tone when answering, a call menu on ANSam when dialling,
a joint menu with V.34 half-duplex, and T.30 Annex F (`Call::start_annex_f`)
once V.8 agreed it. Behind `FaxCall::with_v34(bool)` / `Modem::fax_v34`, **off
by default**: the join has no half-duplex modem yet (packages G, H3), so a call
that agreed V.34 stops at the hand-over point, the line quiet. H3 turns it on.
Off, a call is what it was; on, a call to a plain fax is the same to the sample.

## The API

- `FaxCall::with_v34(on)` / `Modem::fax_v34` (plumbed in `place_call`);
  `FaxCall::v34_agreed()`: V.8 agreed V.34 half-duplex and the call is in
  Annex F. `joint_menu()` (7.4/V.8's JM, from either end) and `far_menu()`
  (answering: the CM; dialling: the JM) for the far-end panel; `standard()`
  says "V.34" there, and `phase_name()` says what V.8 is doing with the line.
- `datapump::v8::Modem`: `with_ansam_seconds` (6.1.1/T.30's 2.6-4.0 s;
  `faxcall::ANSAM_SECONDS` is 3.8, the long end, for a VoIP round trip; the
  data modems keep 8.2.2's 5 s), `answering_only_its_function`, `heard_ci` and
  `ansam_again` (6.1.4), `joint_menu`, `has_the_line` for every role.
  `Call::set_v8_capable` and `t30::say_v8_capable`: DIS bit 6 (6.1.3), set
  only on the DIS that follows an ANSam time-out.

## What happens, and the departures

- Answering: 0.2 s of silence, ANSam; two identical CMs for "transmit
  facsimile from call terminal" -> a JM of the intersection with ours plus V.34
  half-duplex (modn0 b7 alone, never duplex: plan 8.5) until CJ, 75 ms, then
  `start_annex_f()` (6.1.5) or, with no V.34 in common, the DIS as the
  overhearing path sends it (6.1.6). ANSam out with no CM (6.1.3): 75 ms, the
  DIS on V.21 with bit 6, no called tone; the V.8 modem stays as an ear, and two
  CIs before the DIS is answered bring ANSam back, the DIS cut (6.1.4). T.30
  stands still, clocks included, while V.8 has the line.
- Dialling: CNG and T.30 as before while V.8 listens; ANSam believed -> CNG
  stops (8.1.1), Te, CM with our modulations and V.34 half-duplex until two
  identical JMs, CJ, 75 ms, the same rule. After a JM without V.34 the CNG
  rhythm resumes until the DIS (clause 5's phase A); the plain tone drops V.8.
- A CM for polling is left unanswered rather than given 8.2.3's zero JM: the
  DIS after the time-out says "nothing to poll" in the form a caller acts on.
- 6.1.4's CI from the calling end (on a DIS with bit 6, no ANSam heard) is
  **not sent**: "may", and our caller hears ANSam when it is there. Bit 6 is
  clear on a DIS after a JM: the V.8 it invites has been had.

## For H3 (the join)

`faxcall::HalfDuplex` has one variant, `Missing`; add one holding
`halfduplex::Modem` and the compiler shows every arm to fill: `talk` and
`listen` for the `Line::V34*` words (plan 10.1), `phase_name`. Until then those
lines produce silence, take no bit and hand none up. At the hand-over point
(`v34_agreed()` first true, 75 ms after CJ at both ends, INFO0 due next): `v8`
is `None`, V.21 sender and receiver fresh, `line` `Quiet`, `start_annex_f()`
run -- the answerer in `Phase::Identifying`, CSI and DIS queued behind
`Line::V34Control` and never asked for a bit; the caller in `Phase::Listening`
on `Line::V34Listen`, T1 counting -- and `tick(idle = true)` runs every sample.
Call `control_restarted()` once the channel is up, before taking bits.
