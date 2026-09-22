# Reworking the older receivers, mode by mode, worst first

The judgement this starts from is Rory's: *"the 33.6 QAM modes are locked in
place, but some of the older modes are using old code."* `v34-reference.md`
turned that into a list, and `before.md` turned the list into numbers. This is
the plan that turns the numbers into packages.

Every package below names the file or files it may touch, the tests that prove
it, and **the figure in `before.md` it must move, and to what**. Packages in the
same wave never touch the same file, so a wave can be given to as many agents as
it has packages.

---

## What "locked like V.34" means, concretely

`v34-reference.md` §5 is the whole argument in one table. Four rows say "none"
on the older side, and those four rows are the plan:

| mechanism | V.34 | the four `dsp::Equalizer` users |
|---|---|---|
| loss detection, relative to what this line actually reads | `receiver.rs:180-196` | **none** |
| rewind after learning from bad decisions | `receiver.rs:112-113, 997-1013` | **none** |
| resync after a jump | `receiver.rs:1027-1211` | **none** |
| slip counting that the layer above can see | `receiver.rs:612` | **none** |

and three more that are present on both sides but wrong on one:

| | V.34 | the older modes |
|---|---|---|
| confidence gate on every loop update | `squared < 0.25·min_distance²` (`receiver.rs:928, 947`) | no gate at all |
| carrier loop damping | ζ = 1.0 exactly, `Ki = (Kp/2)²` (`receiver.rs:130-131`) | ζ = 2.59 everywhere, four hand-tuned copies |
| equaliser | 31 taps at **T/2**, NLMS, adapting in its own frame | 21–31 taps at **T**, plain LMS, adapting after derotation |

The rule for this work, from the brief: **prefer moving a proven V.34 mechanism
into `crates/dsp` and reusing it.** Three of the packages below do exactly that
(`dsp::CarrierLoop`, `dsp::Lock`, `dsp::Interpolator` + `dsp::HalfSpacedEqualizer`).
Two deliberately do not, and say why — an FSK discriminator is not a QAM carrier
loop, and a half-duplex fax receiver's problem is a *carrier detector clause*,
not a tracking loop.

---

## Rules every package obeys

1. **Specs and real captures only.** Nothing is read from, ported from, built
   from or compared against any other implementation. Every mechanism here is
   either already in this tree (V.34's receiver) or read from a rendered
   Recommendation page.
2. **Every constant cites a clause or a measurement.** A number that cannot be
   traced to a rendered page, to a constellation in `v32/trellis.rs`, or to a
   figure in `before.md` / one of the five notes beside it, is not allowed in.
   In particular: no constant chosen because it made the sweep pass.
3. **`docs/specs/text/*.txt` locates clauses; the rendered PDF page gives the
   value.** Render with PyMuPDF to the scratch folder and read the PNG.
4. **V.34, V.90 and the fax modes keep passing every existing test, unedited.**
   V.34 does not import `dsp::Equalizer` at all and nothing here touches
   `shaping.rs`'s RRC or `fir_lowpass`, so the shared-crate blast radius is
   `Gardner` (V.22 bis, V.27 ter, V.29, V.32) and `Equalizer` (the same four).
   **Every shared-crate change is additive** — a new constructor or setter, with
   `Gardner::new` and `Equalizer::new` unchanged — or it is not allowed.
5. **The baseline must survive.** `lock_sweep.rs` rewrites
   `docs/design/slow-modes/before.md` on every run. Before the first package
   runs the sweep, copy it to `before-48b9087.md` and never overwrite that.
6. **Above 9600 bit/s a single row is not evidence.** The clean line only
   carries the payload at some of the eight arrival phases, so the acceptance
   for anything above 9600 is the **phase count** from `one_mode_clean`, not the
   phase-0 row.

---

## The order, and why

`before.md`'s own ranking, worst first:

| rank | mode | phases | SNR floor | cases failed |
|---|---|---|---|---|
| 1 | V.32bis 14400T | 2/8 | 24 dB | 15 |
| 2 | V.32 9600T | 3/8 | 18 dB | 13 |
| 3 | V.32bis 12000T | 3/8 | 21 dB | 13 |
| 4 | V.32bis 7200T | 7/8 | 12 dB | 12 |
| 5 | V.32 9600 | 6/8 | 18 dB | 8 |
| 6 | V.22 1200 | 7/8 | 6 dB | 4 |
| 7 | V.22bis 2400 | 8/8 | 12 dB | 6 |
| 8 | V.29 9600 | 8/8 | 21 dB | 4 |
| 9 | V.22bis 1200 | 8/8 | 6 dB | 3 |
| 10 | V.32 4800 | 8/8 | 12 dB | 3 |
| 11 | V.29 7200 | 8/8 | 24 dB | 3 |
| 12–13 | V.27ter 2400 / 4800 | 8/8 | 15 dB | 2 |
| 14–15 | Bell 103 / V.21 | 8/8 | 6 dB | 1 |

The top five are **one receiver**: `crates/datapump/src/v32.rs` serves every
V.32 and V.32bis rate. So the spine of this plan is a chain of five packages on
that one file, and because it is one file the chain is strictly sequential —
one package per wave. The other modes' packages are scheduled into the same
waves because their files are disjoint, and they are ordered worst-first among
themselves. That is the only honest way to have both "worst mode first" and
more than one agent working.

| wave | V.32 spine | riding alongside | what is finished at the end |
|---|---|---|---|
| 1 | `v32-front-end` | `v22bis-rate-latch` | — |
| 2 | `v32-equaliser` | `v29-detector` | — |
| 3 | `v32-carrier` | `v27ter-hold` | **V.27 ter** (2400 and 4800) |
| 4 | `v32-lock` | `v22bis-carrier`, `fsk-hold` | **V.32 / V.32bis**, **Bell 103**, **V.21** |
| 5 | `v32-fractional` | `v22bis-lock`, `v29-lock` | **V.22 bis / V.22**, **V.29** |

---

## Three columns nobody should chase

Write these down before any agent starts, so none of them spends a day on an
impairment that is arithmetic rather than code.

1. **`echo 0.57 @ 120 ms` on the V.32 rows.** `before.md` says it plainly: there
   is no echo canceller anywhere in the harness, and V.32 is the one mode that
   puts both directions in the same band. A real V.32 modem cancels before the
   receiver sees anything (`v32/startup.rs`, tested at `v32_loopback.rs:157`).
   These rows measure the bare receiver and understate the modem. **Not a target.**
   The echo canceller's real faults are §8.4 and §8.11 of `v32.md` and they are
   start-up work, listed under "not in this plan".
2. **`echo 0.57 @ 120 ms` on the V.29 and V.27 ter rows.** As a *listener* echo
   that is a reflection 4.9 dB down at 144 to 288 symbol periods. The longest
   equaliser in this tree reaches ±17 ms; a 31-tap T/2 filter reaches 15.5
   symbols. Nothing proposed here spans it. **Not a target.**
3. **FSK `OFF→ON` timing.** V.21 Table 2 asks 300–700 ms and the code answers in
   1.4–3.4 ms. Do **not** "fix" it: T.30 turn-arounds are built on the fast
   answer, and `before.md` shows no failure from it. Record the deviation in the
   code comment with the clause, and leave it.

---

# Wave 1

## P1 `v32-front-end` — stop learning from silence, and from your own gain

**Files:** `crates/datapump/src/v32.rs`, `crates/dsp/src/shaping.rs`.
**Size:** S. **Depends on:** nothing.

**The fault.** Every V.32 rate fails `round trip 1.1 s`, including 4800, which
is otherwise eight phases out of eight on everything. `before.md`'s probe table
pins it: the far end's signal is unchanged and the only difference is how long
the receiver listened to nothing first. At 300 ms, V.32 4800 goes from 8/8 to
0/8. V.22 bis, run beside it as the control, does not move.

The mechanism is in three lines of `v32.rs`:

- `on_symbol` (`v32.rs:997-1004`) runs the AGC whenever `self.adapting`, and
  `adapting` defaults to `true` (`v32.rs:928`). Nothing calls `set_adapting`
  (`v32.rs:1192`) unless a whole `Modem` is driving the receiver, which the
  sweep — and any bare user of `Receiver` — is not. Over 1.1 s of digital
  silence the 50 ms pole (120 symbols) drives `mean_power` to the `1e-9` floor
  and the gain to its `MAX_GAIN = 400` clamp. The carrier flag comes up 20 ms
  after the signal arrives; the AGC needs 50 ms to come back. In between, the
  equaliser is fed points up to forty times too large.
- The equaliser's guard is `self.symbols > 64` (`v32.rs:1073`) — 64 symbols from
  the receiver being *constructed*, which at 2400 baud is 27 ms, shorter than
  the AGC's own recovery. V.22 bis does not have this fault because its guard is
  `since_carrier > 64` and `since_carrier` is reset on a carrier edge
  (`v22bis.rs:642-647, 753`); 64 symbols at 600 baud is 107 ms, longer than its
  AGC's 50 ms.
- `Gardner`'s error normaliser starts at `mean_power = 1.0` (`shaping.rs:282`)
  and moves 2 % a symbol. V.27 ter and V.29 already work around it by
  pre-scaling the loop's input by `1/level` (`v27ter.rs:843-845`,
  `v29.rs:832-839`); V.22 bis and V.32 feed the raw matched-filter output
  (`v22bis.rs:667-668`, `v32.rs:985-986`). `shared-dsp.md` §3.2 has the
  arithmetic: 30 dB down the loop is a thousand times too timid and stays half a
  symbol out of step.

**What to do.**

1. Hold the AGC and `Gardner` while `!self.carrier`, exactly as `v27ter.rs:905-911`
   and `v29.rs:820-835` already do. `Gardner::set_adapting` (`shaping.rs:299`)
   exists and keeps producing symbols while it is held — use it. Keep
   `set_adapting` as the outer, modem-level hold; the carrier gate is a second,
   independent one, because a bare `Receiver` never gets the outer one.
2. Add `since_carrier`, reset on a carrier edge, and gate the equaliser on it
   instead of on `symbols`. The precedent and the number are `v22bis.rs:642-647`.
3. Add `Gardner::with_power(f64)` (additive; `Gardner::new` unchanged) and seed
   V.32's loop with the power the matched filter actually produces at nominal
   level. **Measure it, do not guess:** mean `|matched|²` over a full-level 4800
   burst on a clean line, printed once and written into the comment beside the
   constant. Do the same for V.22 bis in `v22bis-carrier` (wave 4).

**Figures it must move** (`before.md`, `round trip 1.1 s` row of each V.32 table,
and the "silence before the carrier" probe):

| where | now | must be |
|---|---|---|
| V.32 4800 `round trip 1.1 s` | lock never, BER 0.4898 | lock ≤ 40 ms, BER < 1e-3 |
| V.32bis 7200T `round trip 1.1 s` | never, BER 0.4997 | BER < 1e-3 |
| V.32 9600T `round trip 1.1 s` | never, BER 0.4963 | BER < 1e-3 |
| V.32bis 12000T `round trip 1.1 s` | never, BER 0.4960 | BER < 1e-3 |
| V.32bis 14400T `round trip 1.1 s` | never, BER 0.5011 | BER < 1e-3 |
| V.32 9600 `round trip 1.1 s` | lock 905 ms | lock ≤ 200 ms |
| probe, V.32 4800 at 300 ms silence | 0/8 phases, BER 0.4913 | 8/8, BER 0 |
| probe, V.32 4800 at 400/600/1100 ms | 5/8, BER ≈ 0.490 | 8/8, BER 0 |

**Tests.** `lock_sweep::two_things_worth_a_closer_look` and
`every_slow_mode_against_every_impairment` (release, `--ignored`); every
existing V.32 test **unedited** — `v32_loopback`, `v32_call`, `v32_both_ends`,
`v32_startup`, `v32_signals`, `v32_rate_framing`, `v32_bits`, `v32_vector`,
`v32_who`, `v32_replay`; `cargo test -p dsp` (18 `shaping` tests must still
pass, and `Gardner::new`'s behaviour must be bit-identical).

---

## P2 `v22bis-rate-latch` — a rate decision that does not change its mind

**Files:** `crates/datapump/src/v22bis.rs`. **Size:** S. **Depends on:** nothing.

**The fault.** `before.md` runs two rows that put the *same waveform* on the
line — "V.22 1200" and "V.22 bis 1200" are the same signal (2.5.2.2: the point
`01` "irrespective of the quadrant concerned … This ensure compatibility with
Recommendation V.22"). The only difference is that the V.22 row leaves the
receiver to work the rate out. That one difference costs it two cells:

- `clock +120 ppm`: BER 0.4688 where V.22 bis 1200, told its rate, reads 0.
- arrival phases 7/8 against 8/8.

The rate is judged from the power variance of the pre-equaliser point over
128-symbol windows, `variance < 0.16·mean²` (`v22bis.rs:843-855`). The reasoning
in the comments above it (`v22bis.rs:773-829`) is sound and hard-won and should
not be replaced. What is missing is that the test **is not latched and has no
hysteresis** — 128 symbols after `set_rate`, it runs again and can overrule the
negotiation (`v22bis.rs:920-925` only resets the counters). `v22-and-bell.md`
W7 records what that costs on the tree's own ground truth: one direction of
`tests/vectors/v22bis-2400.wav` read 2400 for a 1200 call for six seconds,
about 3600 symbols of wrong decisions into the equaliser, and its residual sat
at 0.19 for the remaining eleven seconds against 0.031 in the other direction.

**What to do.** Two changes, both small.

1. **Hysteresis, not a threshold.** A rate already decided needs more evidence to
   be abandoned than to be chosen: keep `0.16·mean²` for the first decision and
   require the opposite side of a band around it — with the width taken from the
   measured variance of the two constellations (`v22bis.rs:806-809` states them:
   16-QAM's power variance is 32 about a mean of 10, the single ring's is 0) —
   for a change, plus N consecutive windows agreeing. N and the band come from a
   sweep of the two clean waveforms, printed and written into the comment.
2. **`set_rate` latches.** When the handshake has negotiated a rate
   (`handshake.rs:271, 334`), the variance test becomes a *disagreement report*,
   not an override. Keep reporting it — it is what a future 6.4 retrain will
   trigger on — but stop acting on it silently.

**Figures it must move:**

| where | now | must be |
|---|---|---|
| V.22 1200 `clock +120 ppm` | BER 0.4688, lock never | BER < 1e-3, lock ≤ 500 ms |
| V.22 1200 arrival phases, clean | 7/8 | 8/8 |
| V.22bis 1200 and 2400, every row | as printed | unchanged or better |

**Tests.** `lock_sweep::one_mode_clean` for the three V.22 rows and the full
sweep; `v22bis_handshake` (8), `v22bis_loopback` (22), `v22bis_vector` — all
unedited; `v22bis_capture` with `V22_CAPTURE` set, which is where the
six-second misread lives.

---

# Wave 2

## P3 `v32-equaliser` — tell the equaliser which constellation it is on

**Files:** `crates/dsp/src/equalizer.rs`, `crates/datapump/src/v32.rs`.
**Size:** M. **Depends on:** `v32-front-end`.

**The fault.** `Equalizer::new(21, 1.0)` is built once in `Receiver::new`
(`v32.rs:909`) and `follow()` (`v32.rs:949-959`) — the one function that knows
the rate changed — never touches it. Two constants are then wrong for five of
the six rates.

*The constant-modulus target.* `equalizer.rs:41` documents `modulus` as
E|a|⁴/E|a|² for the constellation in use. `shared-dsp.md` §3.6 computes the true
values from `v32/trellis.rs`'s own tables: 1.000 at 4800, **1.310** at 9600
coded, **1.381** at 12 000, **1.343** at 14 400. The blind stage settles where
`E|y|⁴ = R₂·E|y|²`, so a target of 1.0 against a true 1.31 hands over a
constellation `√(1/1.31)` = 12.6 % small, −1.18 dB.

*The hand-over threshold.* `equalizer.rs:121` leaves the blind stage when the
running mean decision error falls below a bare `0.25`, on a scale that means
something different at every rate. Against the distance between neighbouring
points at unit mean power (`shared-dsp.md` §3.6): 18 % at 4800, 56 % at 9600
coded, 81 % at 12 000, **113 % at 14 400**. At 14 400 the blind stage hands over
to decision direction while the mean error is still larger than the whole
distance between points — and `equalizer.rs:15-18` says exactly why that is
fatal: *"Starting decision-directed on a closed eye simply reinforces whatever
nonsense it first decides."*

This is the best explanation available for the clean-line arrival-phase counts:
2/8 at 14 400, 3/8 at 12 000 and at 9600 trellis, 6/8 at 9600 uncoded, 8/8 at
4800 — a ladder that follows the table above rung for rung.

**What to do.**

1. Three additive methods on `Equalizer`: `set_modulus(f64)`,
   `set_handover(f64)`, `restart_blind()`. `Equalizer::new` keeps its exact
   present behaviour so V.22 bis, V.27 ter, V.29 and their tests do not move.
   `restart_blind` is the piece `v22-and-bell.md` open question 3 asks for: today
   `equalizer.rs:121` is one-way and only `reset()` (a NaN or 1e4 of tap energy)
   can undo it.
2. `V32::Receiver::follow` sets the modulus from the constellation actually in
   use, computed at runtime from `trellis.rs:416-440`'s own table rather than
   written as a literal, and calls `restart_blind()` when the constellation
   changes under it — which at the `Connected` handover is a jump from 45° of
   rotation margin to 5.1° in one symbol (`v32.md` §4).
3. The hand-over threshold becomes a fraction of **this** constellation's point
   spacing, using `point_spacing_at` (`v32.rs:366-376`), which already exists and
   already contradicts the comment that justifies the present behaviour. Anchor
   it at the one rate where 0.25 is defensible — 4800, where it is 18 % of the
   spacing — so the fraction is 0.25/√2 = 0.177 and every other rate inherits it
   proportionally. Say so in the comment, with the table.

**Figures it must move** (arrival phases from `one_mode_clean`; the level rows
from the sweep):

| where | now | must be |
|---|---|---|
| V.32bis 14400T arrival phases | 2/8 | ≥ 6/8 |
| V.32 9600T arrival phases | 3/8 | ≥ 6/8 |
| V.32bis 12000T arrival phases | 3/8 | ≥ 6/8 |
| V.32 9600 arrival phases | 6/8 | 8/8 |
| V.32bis 7200T arrival phases | 7/8 | 8/8 |
| V.32 9600 `level -6 dB` | BER 0.0306 | < 1e-3 |
| V.32 9600T `level -6 dB` | BER 0.0266 | < 1e-3 |
| V.32bis 12000T `level -6 dB` | BER 0.0308 | < 1e-3 |
| V.32bis 14400T `level -6 dB` / `level +6 dB` | BER 0.0394 / 0.0944 | < 1e-3 |
| V.32bis 14400T `clean` slicer SNR | 30.7 dB | ≥ 30.7 dB (must not fall) |

**Tests.** The sweep and `one_mode_clean`; every V.32 test unedited; `cargo test
-p dsp` — `equalizer.rs`'s 5 tests must pass **unchanged**, which is the proof
that the additions are additive; the four other `Equalizer` users
(`v22bis_loopback`, `v27ter`/`v29` in `faxcall`, `v32_loopback`) unedited.

---

## P4 `v29-detector` — a carrier detector that cannot latch, and an acquisition that re-arms

**Files:** `crates/datapump/src/v29.rs`. **Size:** M. **Depends on:** nothing.

**The fault.** V.29's signal-to-noise floor is **21 dB at 9600 and 24 dB at
7200**, against V.27 ter's 15 dB and V.32 4800's 12 dB, and the way it fails is
a cliff rather than a slope: at 9600 the `SNR 18 dB` row locks at 1300 ms with
slicer SNR 11.7 dB, and 15 dB never locks at all. `fax-qam.md` §3.1 and §3.2
name the mechanism and it is not the demodulator:

- the on-threshold is `max(4·floor, 1.0e-3)` with `floor` starting at zero
  (`v29.rs:641`), so a fresh receiver has only the fixed `1.0e-3`, which is
  **53 dB below a burst**;
- white noise of standard deviation σ reads 0.39 σ on the same meter, so any
  line whose hiss is above σ = 2.56e-3 fires it;
- the trip calls `new_burst` (`v29.rs:789-791`), the only thing that resets
  `burst_symbols`, so the one-shot acquisition window of §2.5 is spent on hiss;
- the detector then cannot fall again, because the off-threshold is a quarter of
  `loudest` and `loudest` is by then the noise itself;
- and `floor` only adapts while the detector says there is no carrier
  (`v29.rs:776-782`), so the estimate it needs in order to notice can never be
  taken.

In the sweep the lead-in is 0.5 s of line before the far end starts
(`lock_sweep.rs`, `Mode::V29 => (0.50, 0.80)`), and at 15 dB the noise on that
line reads about 0.08 on this meter — eighty times the threshold. The cliff in
`before.md` is this latch, measured.

The `dropout 20 ms` row is the same file's other fault seen from the other end:
the detector drops correctly after 12–13 ms and *that is what ends the burst*,
because a half-duplex receiver has one training sequence in front of it and no
way back to it.

**What to do.**

1. **The carrier-off decision is out of spec and that is the cheap half of the
   dropout row.** V.29 5.2.2 asks for **30 ± 9 ms** and notes the figure "should
   be suitably chosen … to ensure that all valid data bits have appeared on
   circuit 104"; the measured figure today is 17.6 ms (`fax-qam.md` §2.2, §3.7).
   Re-read 5.2.2 from the rendered page, put the decision inside the window, and
   a 20 ms hole stops ending the burst.
2. **Let the floor keep falling.** The floor's one-way gate (`v29.rs:776-782`) is
   what makes the latch permanent. Let it continue to fall while the detector is
   on *and* the acquisition has not been confirmed, so a detector that fired on
   hiss can still discover the hiss.
3. **Re-arm the acquisition.** `acquire` runs once, is never checked and is never
   repeated (`fax-qam.md` §3.2). Give it the acceptance test V.34 already uses in
   `resync` (`v34/receiver.rs:1199-1206`): a reading must be both absolutely good
   *and* better than 0.6 × the median of its own competitors — a self-calibrating
   threshold, not a constant. Segment 2's alternation of two known points
   (`v29.rs:892-935`) is the competitor set, so this is cheap here; re-run it on
   the first 64 credible alternations after any edge.
4. While in the file, fix the 4800 A-first/B-first tie (`fax-qam.md` §3.3,
   `v29.rs:910` uses a strict `>` where the two hypotheses are exactly equal at
   4800). No row in `before.md` covers V.29 4800 — T.30 never commands it — but
   it is a landmine and a warning for V.17, and the measurement is already
   written down: the page was lost at 7 of 20 start offsets.

**Figures it must move:**

| where | now | must be |
|---|---|---|
| V.29 9600 SNR floor | 21 dB | ≤ 15 dB |
| V.29 7200 SNR floor | 24 dB | ≤ 15 dB |
| V.29 9600 `SNR 18 dB` | lock 1300 ms, BER 0.0084 | lock ≤ 350 ms, BER < 1e-3 |
| V.29 7200 `SNR 24 dB` | slicer SNR 15.7, lock 804 ms | ≥ 26 dB, lock ≤ 300 ms |
| V.29 7200 `SNR 21 dB` | BER 0.0012 | < 1e-3 |
| V.29 7200 / 9600 `dropout 20 ms` | carrier lost 12 / 13 ms, BER 0.2241 / 0.2238 | carrier held, BER < 1e-3 |

**Tests.** The sweep; `cargo test -p fax` and the whole `faxcall` suite
unedited — in particular `faxcall.rs:690-705`, the one existing noise test,
which today passes while the call silently drops a rate, and which must still
pass; `v29.rs`'s own unit tests including the gain test at `v29.rs:1243-1284`
and the offset sweep at `v29.rs:1293`.

---

# Wave 3

## P5 `v32-carrier` — one shared carrier loop, critically damped, and seeded where a seed exists

**Files:** `crates/dsp/src/carrier.rs` (new), `crates/dsp/src/lib.rs`,
`crates/dsp/src/tone.rs`, `crates/datapump/src/v32.rs`,
`crates/datapump/src/v32/startup.rs`. **Size:** L.
**Depends on:** `v32-equaliser`.

**The fault.** Every V.32 rate above 4800 fails carrier offsets the
Recommendation requires it to take. V.32 2.1, rendered: *"The carrier frequency
is to be 1800 ± 1 Hz … The receiver must be able to operate with received
frequency offsets of up to ± 7 Hz."* `before.md`'s phase table is unambiguous
that this is the impairment and not the phase lottery: at 12 000 and 14 400
every one of the six offset columns is **0/8**.

Three separate things are wrong, and they are all in `v32.rs:1013-1047`.

1. **ζ = 2.59 everywhere.** `shared-dsp.md` §3.1 has the pull-in times computed
   from the exact difference equations and the real constellations: 9600 coded
   takes 85 s to pull in 7 Hz, 12 000 takes 83 s to pull in 3 Hz, and 14 400
   takes 51 s to pull in **one** hertz. V.34's loop is critically damped by
   construction — `Kp = 0.04`, `Ki = 4e-4`, so `Ki = (Kp/2)²` exactly and ζ = 1.0
   — with a natural frequency of 10.9 Hz and a one-sided noise bandwidth of
   6.8 Hz (`v34/receiver.rs:130-131`, `v34-reference.md` §3.3).
2. **9600 uncoded runs at the 4800 gain.** `loop_bandwidth` (`v32.rs:270-277`)
   only scales for the trellis codings, and `coding_for(9600, Uncoded)` returns
   `None`, so 2.4.1.1's sixteen points get `bw = 1.0` — the same as 4800's four.
   `v32.md` §8.2 has the measured rotation margins: 45.0° at 4800, **16.9° at
   9600 uncoded**, 16.9° at 7200, 10.9° at 9600 trellis, 7.7° at 12 000, 5.1° at
   14 400. Every trellis rate lands at margin ÷ bw between 20.7 and 23.9; 9600
   uncoded is the sole outlier at 16.9. The comment that justifies it
   (`v32.rs:274-275`, "both two units apart") is contradicted by
   `point_spacing_at` twelve lines below it.
3. **A 9.49-symbol pole sits inside the loop.** `TRACK_SMOOTHING = 0.1`
   (`v32.rs:285, 1032`). V.34 has no such filter (`v34/receiver.rs:957-959`), and
   a lag inside a second-order loop is what forces the damping up in the first
   place.

Two smaller ones, both worth taking while the file is open: the decision-power
floor is an absolute `MIN_DECISION_POWER = CONSTELLATION_MEAN_POWER/2 = 5.0`
(`v32.rs:224`), ten times harsher than V.34's `max(|target|², 0.1)` at unit
power, and it down-weights the inner ring rather than protecting against it
(`v32.md` §10's closing note); and the loop is driven by a decision on the
**unequalised** symbol, which at sixteen points is a full 16-point slice of a
signal the equaliser has not cleaned — decide (3,1) as (1,1) and the error comes
out at −57° (`v32.md` §8.2).

**What to do.**

1. **`crates/dsp/src/carrier.rs`: one second-order carrier loop, V.34's shape,
   shared.** Error `Im(z·conj(target))/max(|target|², floor)`;
   `rotation += Kp·wrong`, `turn += Ki·wrong`, and `rotation += turn` every
   symbol *before* any gate, so the frequency estimate keeps running the phase
   forward while everything else is held — that ordering is
   `v34/receiver.rs:946` and it is the reason a held loop does not lose the far
   end's oscillator. The constructor takes **one** number, the loop's natural
   frequency in hertz, and derives `Kp` and `Ki` from it with `Ki = (Kp/2)²`, so
   ζ = 1 cannot be got wrong per mode. Document the closed-loop roots, the noise
   bandwidth and the settling time the way `v34-reference.md` §3.3 does.
2. **V.32 chooses its bandwidth from its own rotation margin.** One table, taken
   from `point_spacing_at` and the measured margins in `v32.md` §8.2, with 9600
   uncoded in it. State the target ratio (margin ÷ bandwidth ≈ 22, which is what
   every trellis rate already is) and let the numbers fall out of it rather than
   writing six constants.
3. **Drop `TRACK_SMOOTHING`** and the absolute decision-power floor; use V.34's
   proportional one.
4. **Seed the loop where a seed exists.** `ReversalDetector` already measures the
   far end's frequency offset and throws it away (`tone.rs:353-360`), and V.32's
   start-up already runs three of them on the carrier and both sidebands
   (`v32/startup.rs:994-996`). `shared-dsp.md` §4.7 calls this "the cheapest
   large win in the tree": expose `drift_hz()` and seed `Receiver::frequency`
   from it before the first data symbol. Note also §6.9 — the detector currently
   *refuses* above 7.85 Hz, which is barely over the 7 Hz V.32 2.1 requires, so
   the refusal threshold has to move with it.

**Note on what the sweep can and cannot see.** `lock_sweep.rs` drives the bare
`Receiver` with data from the first symbol (`transmit`, `Mode::V32` arm) — there
is no S, no S̄, no TRN and no start-up. **So item 4 moves nothing in `before.md`
by construction.** It is in this package because it belongs with the loop and
because it is what a real call gets; it is proved by `v32_call` and `v32_startup`
with a carrier offset applied, not by the sweep. Items 1–3 are what the sweep
measures.

**Figures it must move:**

| where | now | must be |
|---|---|---|
| V.32 9600 `carrier -7/-1/+1/+3/+7 Hz` | BER 0.340 / 0.495 / 0.500 / 0.024 / 0.262 | all < 1e-3 |
| V.32bis 7200T, all six carrier rows | BER 0.335 … 0.500 | all < 1e-3 |
| V.32 9600T `carrier -7/-3/+3/+7 Hz` | BER 0.388 / 0.361 / 0.485 / 0.385 | all < 1e-3 |
| V.32bis 12000T, all six carrier rows | BER 0.455 … 0.501 | all < 1e-3 |
| V.32bis 14400T, all six carrier rows | BER 0.431 … 0.500 | all < 1e-3 |
| phase table, 14400T `-7 … +7 Hz` columns | 0/8 | ≥ 5/8 |
| phase table, 12000T `-7 … +7 Hz` columns | 0/8 | ≥ 5/8 |
| V.32 9600 SNR floor | 18 dB | ≤ 15 dB |
| V.32 9600T SNR floor | 18 dB | ≤ 15 dB |
| V.32bis 14400T SNR floor | 24 dB | ≤ 21 dB |

**Tests.** The sweep and `one_mode_clean`; `cargo test -p dsp` including the new
`carrier.rs` unit tests (the closed-loop roots, the settling time and the ζ = 1
identity asserted, not just a lock); every V.32 test unedited, plus
`v32_reversals` and `v32_startup` for the `tone.rs` change; `v32_call` with an
offset applied, which is where item 4 is proved.

---

## P6 `v27ter-hold` — finish V.27 ter

**Files:** `crates/datapump/src/v27ter.rs`. **Size:** M. **Depends on:** nothing.

**The fault.** V.27 ter fails exactly two cases at each rate, and one of them is
the echo column that nobody should chase. The other is `dropout 20 ms`, and it
is catastrophic rather than proportional: BER 0.487 at 2400 and 0.492 at 4800,
"carrier lost, 11 ms", lock **never**. The dropout table shows the shape: 10 ms
costs 0.0048 and 20 ms costs 0.4947. A carrier detector that drops ends the
burst, and a half-duplex receiver has one training sequence in front of it and
no way back.

Three things, in descending order of what they are worth.

1. **The carrier-off decision is out of spec.** V.27 ter 3.6, via Tables 7 and 8,
   asks **30 to 50 ms**; the measured figure is 17.6 ms (`fax-qam.md` §2.2, §3.7).
   Re-read it from the rendered page and put the decision inside the window. On
   its own this should carry the whole 20 ms row, because the data either side of
   a 20 ms hole is intact — `before.md`'s own dropout table shows 10 ms costing
   0.0048 with the carrier held.
2. **A hole longer than the clause still ends the burst, and it need not.** The
   50 ms row (BER 0.4769) needs the receiver to re-find the instant and the
   rotation without a training sequence. The mechanism already exists in this
   tree: V.34's `resync` (`v34/receiver.rs:1148-1211`) re-reads a window from the
   raw history at sixteen sub-symbol shifts, takes the rotation from the fourth
   power to within a quarter turn and refines it decision-directed, and accepts
   only a reading that is both absolutely good and better than 0.6 × the median
   of its own competitors. On an 8-PSK constellation the fourth power is not the
   right invariant — **use the eighth** — and say so in the comment, with the
   reason. This is the one place in the plan where a V.34 mechanism has to be
   adapted rather than copied.
3. **Stop slicing the carrier error over eight phases at 2400.** `nearest_eighth`
   (`v27ter.rs:928, 982-985`) is used at both rates, and the comment that
   justifies it (`v27ter.rs:922-927`) is wrong on its own terms: at 2400
   `DIBIT_TURN` is `[0, 2, 6, 4]` (`v27ter.rs:134`), segment 3's reversal is
   `turn(4)` and segment 4's is `turn(4)` or `turn(0)`, so **every phase the 2400
   end can put on the line is one of the four**. Slicing over eight halves the
   phase detector's linear range from ±45° to ±22.5° and throws a 45° spike into
   the loop for every mis-slice. While in the file: the select filter stays at
   1400 Hz when the rate drops to 2400 (`v27ter.rs:669`; `set_rate` at
   `v27ter.rs:700-710` rebuilds three other things and not this), where the
   signal only reaches 900 Hz — 1.9 dB of noise into the level meter and the
   timing loop for nothing.

**Figures it must move:**

| where | now | must be |
|---|---|---|
| V.27ter 2400 `dropout 20 ms` | lock never, BER 0.4873, carrier lost 11 ms | carrier held, BER < 1e-3 |
| V.27ter 4800 `dropout 20 ms` | lock never, BER 0.4918, carrier lost 11 ms | carrier held, BER < 1e-3 |
| dropout table, V.27ter 4800 at 50 ms | BER 0.4769, carrier lost | BER < 1e-3, one slip reported |
| V.27ter 2400 SNR floor | 15 dB | ≤ 12 dB |
| V.27ter 2400 `SNR 12 dB` / `SNR 9 dB` | BER 0.0011 / 0.0321 | < 1e-3 / < 1e-3 |

**Tests.** The sweep and `two_things_worth_a_closer_look`; the whole fax suite
unedited (`cargo test -p fax`, `faxcall`), including the short turn-on paths;
`v27ter.rs`'s own unit tests and its offset sweep at `v27ter.rs:1235`.

At the end of this wave V.27 ter has one failing cell left, and it is the echo
column that is arithmetic.

---

# Wave 4

## P7 `v32-lock` — a confidence gate, a loss detector, a hold and a rewind

**Files:** `crates/dsp/src/lock.rs` (new), `crates/dsp/src/lib.rs`,
`crates/datapump/src/v32.rs`. **Size:** L. **Depends on:** `v32-carrier`.

**The fault.** `v32.rs:1031-1042` updates the carrier loop and `v32.rs:1073-1081`
the equaliser on **every** symbol while connected, however improbable the
decision was. There is no confidence gate, no loss detector, and no way back.
`v32.md` §8.3 states it and `before.md` measures it: every V.32 rate fails
`dropout 20 ms`, and the dropout table shows the cost rising smoothly with the
length of the hole (0.0013 at 2 ms, 0.0161 at 50 ms) because nothing above the
receiver is ever told that anything happened — `slips` reads 0 in every row.

On Rory's rig this is not a hypothetical: a VoIP concealment insert is about
20 ms (memory `voip-jitter-slips`), which at 2400 baud is 48 symbols of garbage
into a loop whose slow mode at 14 400 is 865 ms.

**What to do.**

1. **`crates/dsp/src/lock.rs`: V.34 §3.1 and §4.1, shared.** Constructed from the
   constellation's minimum distance — the same number `v32-equaliser` already
   needs — and holding:
   - the gate, `doubtful = 0.25 · min_distance²` (`v34/receiver.rs:928`), which is
     half the distance to the decision boundary: above it a decision is more
     likely wrong than right, so nothing learns from it;
   - `settled`, an EMA at 0.01 of the *passing* symbols' error
     (`v34/receiver.rs:968`), so the threshold is always relative to what this
     particular line actually reads;
   - `recent`, a window of the last *n* squared errors, *n* per constellation
     size (8 for four points, 32 for the dense ones, `v34/receiver.rs:210-215`);
   - `lost_threshold = max(k · settled, min_distance²/12)`, `k` = 8 for few points
     and 2 for many (`v34/receiver.rs:180-196`). The `/12` floor is deliberate and
     the reason is worth carrying over into the comment: a sample landing
     uniformly within ±1 unit of some point has mean squared error
     `min_distance²/6`, so `/12` is 3 dB of margin against pure garbage.
2. **Rewind, in `v32.rs`.** `shared-dsp.md` §4.5 is right that the snapshot itself
   should not be shared — it is a struct of one receiver's own fields — and right
   that V.32 is the one slow mode that runs long enough for it to matter. Snapshot
   the taps, `phase`, `frequency`, the Gardner phase and integral, the AGC value
   and `settled` every 16 symbols, keep 24 (`v34/receiver.rs:112-113`), and on a
   declared loss restore the newest snapshot at least `n + 16` symbols old and
   carry the phase forward by `frequency · elapsed` (`v34/receiver.rs:997-1013`).
   The problem it solves is stated there and applies here word for word: by the
   time the error average has risen far enough to declare a loss, every loop has
   already spent the whole window learning from wrong decisions.
3. **Count and report slips.** `slips()` in V.34 (`v34/receiver.rs:612`) is read by
   the layer above to restart a frame search (`training.rs:1251-1256`). V.32's
   equivalent consumer is the V.42 framer; at minimum, expose the count so the
   sweep's `slips` column stops reading 0 when a hole has plainly cost symbols.
   This is also the signal `v32.md` §8.3 says V.42 never gets today.
4. While the gate is being added, it subsumes `v32.md` §8.9's second half: today
   `error_average` freezes at its last value if the far end drops below
   `CARRIER_OFF`, so a far end that goes away quietly leaves the pump in
   `Connected` reporting a healthy residual for ever.

**Figures it must move:**

| where | now | must be |
|---|---|---|
| V.32 4800 `dropout 20 ms` | BER 0.0082, slips 0 | < 1e-3, slips ≥ 1 |
| V.32 9600 `dropout 20 ms` | BER 0.0075 | < 1e-3 |
| V.32bis 7200T `dropout 20 ms` | BER 0.0070 | < 1e-3 |
| V.32 9600T `dropout 20 ms` | BER 0.0072 | < 1e-3 |
| V.32bis 12000T `dropout 20 ms` | BER 0.0081 | < 1e-3 |
| V.32bis 14400T `dropout 20 ms` | BER 0.0125 | < 1e-3 |
| dropout table, V.32 4800 at 50 ms | BER 0.0161, carrier held, no slip | < 1e-3, slip reported |
| V.32bis 14400T `clock -200/-50/+50/+200 ppm` | BER 0.385 / 0.496 / 0.0045 / 0.370 | all < 1e-3 |
| V.32bis 12000T `clock -50/+50 ppm` | BER 0.4946 / 0.0089 | < 1e-3 |

**Tests.** The sweep, `one_mode_clean` and `two_things_worth_a_closer_look`;
`cargo test -p dsp` with `lock.rs`'s own unit tests — in particular that the
`/12` floor reads 3 dB below what uniform garbage produces, asserted
numerically; every V.32 test unedited.

At the end of this wave V.32 and V.32bis have one failing column left below
14 400 — the echo column — and `v32-fractional` is what remains for the top two
rates.

---

## P8 `v22bis-carrier` — ±7 Hz, as 2.6 requires

**Files:** `crates/datapump/src/v22bis.rs`. **Size:** M.
**Depends on:** `v32-carrier` (for `dsp::CarrierLoop`), `v32-equaliser` (for
`Equalizer::set_modulus`/`restart_blind`), `v22bis-rate-latch` (same file).

**The fault.** V.22 bis 2.6, rendered page 4: *"The receiver shall be able to
operate with received frequency offsets of up to ± 7 Hz."* The measured
cold-acquisition edge is **±2.2 Hz at 2400** and **+6 Hz at 1200**
(`v22-and-bell.md` §3.3), and `before.md` agrees: at 2400, ±3 Hz and ±7 Hz all
fail, and the phase table reads 0/8 at every offset except ±1 Hz.

The loop is the same second-order shape as V.32's with the same ζ = 2.59 and no
open-loop estimate at all (`v22bis.rs:710-731`). Before the integrator has
absorbed anything, the proportional term alone leaves **11.93° per hertz** of
standing error against a 16-QAM angular margin of +20.8°/−18.4° — so 1.5 Hz eats
the whole margin on a clean line. `shared-dsp.md` §3.1 has the pull-in times:
2.5 s at 1 Hz, **41.5 s at 7 Hz**.

**What to do.**

1. Replace the hand-rolled loop with `dsp::CarrierLoop` at ζ = 1, with the
   bandwidth chosen from the 16-point constellation's own rotation margin the
   same way V.32's rates are — one rule, two modes.
2. Rebuild the equaliser's modulus when the rate changes: `shared-dsp.md` §6.7 —
   V.22 bis passes 1.32 and never rebuilds it, so at 1200, where the true R₂ is
   **1.000** (one point, `v22bis.rs:62`, 2.5.2.2), the blind stage aims 15 %
   high. `Equalizer::set_modulus` and `restart_blind` arrive with
   `v32-equaliser`; `v22bis.rs:920-925` is where they belong.
3. Seed `Gardner`'s normaliser with `Gardner::with_power` as `v32-front-end` did
   — `v22bis.rs:667-668` feeds the raw matched-filter output, and at 600 baud the
   1.0 start costs 575 ms of a 1000 ms handshake (`shared-dsp.md` §3.2).

**Note on what the handshake hides.** Run these offsets through the *full*
two-modem handshake and every one connects, because everything before
`Rising2400` runs at 1200 where the decision regions are 90° wide and the
converged integrator is handed to the 2400 stage (`v22-and-bell.md` §3.3). So a
green `v22bis_handshake` is **not** proof of this package. The proof is the
sweep, which acquires cold at 2400, and the live capture.

**Figures it must move:**

| where | now | must be |
|---|---|---|
| V.22bis 2400 `carrier -7/-3/+3/+7 Hz` | BER 0.354 / 0.313 / 0.326 / 0.359 | all < 1e-3 |
| phase table, V.22bis 2400 `-7 … +7 Hz` | 0/8, 0/8, 3/8, 5/8, 0/8, 0/8 | ≥ 6/8 in every column |
| V.22 1200 `carrier ±7 Hz` | BER 0.1093 / 0.1084 | < 1e-3 |
| V.22bis 1200 `carrier ±7 Hz` | BER 0.0609 / 0.0651 | < 1e-3 |
| V.22bis 2400 SNR floor | 12 dB | ≤ 9 dB |
| V.22bis 2400 `clean` lock | 175 ms | ≤ 175 ms (must not grow) |

**Tests.** The sweep and `one_mode_clean`; `v22bis_handshake`,
`v22bis_loopback`, `v22bis_vector` unedited; `v22bis_capture` on
`live-1788613347.wav`, where the residual runs 0.24 → 0.52 today.

---

## P9 `fsk-hold` — finish Bell 103 and V.21

**Files:** `crates/dsp/src/fsk.rs`, `crates/datapump/src/framing.rs`,
`crates/datapump/src/v21.rs`. **Size:** M. **Depends on:** nothing.

**The fault.** Both FSK modes fail exactly one case, and it is the same one:
`dropout 20 ms`. Bell 103 loses 3.3 % of characters and slips twice; V.21's
slicer SNR falls from 25.5 dB to 14.5 dB. The discriminator is not at fault —
`atan2` of a ratio is amplitude-blind and a steady mark still reads +1.000. What
fails is the **carrier flag**, and what the carrier flag controls is *state*:
`AsyncFramer::feed` throws away the character in flight (`framing.rs:52-57`) and
`v21::Receiver::feed` restarts the bit clock (`v21.rs:176-180`), which re-phases
HDLC and costs the frame.

The flag drops after `0.005·ln(L/5.62e-4)` seconds of quiet — **11.5 ms** from
20 dB above threshold, 16 ms from a −30 dBFS line (`v22-and-bell.md` §2.4). V.21
Table 2, rendered page 4, requires circuit 109 to go **ON→OFF in 20 to 80 ms**.
So the 20 ms hole that costs a character is one the Recommendation says should
not have been noticed at all.

**What to do.**

1. **Meet Table 2.** Lengthen the ON→OFF decision so it lands inside 20–80 ms at
   every level, rather than being a level-dependent 13–36 ms. Do it by timing the
   decision, not by moving the threshold — the threshold has a separate job
   (V.21 8.3, −43/−48 dBm) and a separate fault (W19, uncalibrated), which this
   package does not take on. Leave OFF→ON alone and record why in the comment
   (see "Three columns nobody should chase").
2. **A drop should not throw away what is in flight.** Bell 103 slipped twice on
   a 20 ms hole. Even with (1), a longer hole will drop the flag, and the framer
   should resume on the next start bit rather than lose alignment; V.21's clock
   should hold its phase across a short gap rather than free-run.
3. While the file is open, two things `v22-and-bell.md` measured and nothing in
   `before.md` reaches: **W15**, the hard-zero slicer (`fsk.rs:114`,
   `framing.rs:86`, `v21.rs:208`) gives away 1.1 dB at the ±12 Hz V.21 clause 3
   requires, and a running mean of the discriminator output removes the bias
   exactly; and **W5**, the V.21 bit clock is first order (`v21.rs:156, 195-198`)
   and leaves a standing offset of `8·sps·ε/d` = 1707·ε samples on a flag stream,
   which measured out between 1.0 % and 1.5 %. Neither has a cell in the sweep —
   ±200 ppm is 0.02 % — so both are riders, proved by the measurements already
   written down, not by `before.md`.
4. Delete `slow_env` (`fsk.rs:81, 105`), fed and never read.

**Figures it must move:**

| where | now | must be |
|---|---|---|
| Bell 103 `dropout 20 ms` | SER 0.0327, BER 0.0155, slips 2 | BER < 1e-3, slips 0 |
| V.21 `dropout 20 ms` | BER 0.0012, slicer SNR 14.5 | BER < 1e-3, slicer SNR ≥ 22 |
| Bell 103 and V.21, every other row | as printed | unchanged |

**Tests.** The sweep; `bell103_loopback` (4), `bell103_vector`, `v22bis_*`
untouched; `cargo test -p dsp` (`fsk.rs`'s 5 tests, including the sign test at
`fsk.rs:150-173` which is what stops every V.21 bit inverting); `cargo test -p
fax` for the V.21 half of T.30.

---

# Wave 5

## P10 `v32-fractional` — T/2 equalisation and a real interpolator

**Files:** `crates/dsp/src/interp.rs` (new), `crates/dsp/src/halfspaced.rs`
(new), `crates/dsp/src/lib.rs`, `crates/datapump/src/v32.rs`. **Size:** L.
**Depends on:** `v32-lock`.

This is the structural one, and it is deliberately last so that a failure here
does not block the four wins before it.

**The fault, in two parts.**

*The interpolator.* All four slow modes hit the wanted sampling instant with
two-point linear interpolation, in four identical copies (`v22bis.rs:662-666`,
`v27ter.rs:830-834`, `v29.rs:814-818`, `v32.rs:980-984`). Measured error power
against exact interpolation (`shared-dsp.md` §3.3): **−37.9 dB for V.32 and
V.29** at 6.667 samples per symbol. `v32.md` §3.1 computes the worst case as an
SNR ceiling of **27.3 dB that no amount of line quality can lift**, which against
half the distance between neighbouring points is 19 % at 9600 trellis, 28 % at
12 000 and **39 % at 14 400**. V.34 does not do this: 64 taps, 256 fractional
phases, Kaiser β = 8 (`v34/receiver.rs:457-472, 662-678`).

*The spacing.* `Equalizer` is symbol-spaced and can only correct the aliased
folded channel; it cannot compensate a sampling phase. At the worst timing phase
the fold puts an exact null at the band edge, which the equaliser must invert
with unbounded gain. That is why the slow modes depend utterly on Gardner having
found the right instant, and it is the other half of the arrival-phase lottery
that `v32-equaliser` only half fixes. V.34's is 31 taps at T/2, and its own
comment (`v34/receiver.rs:31-33`) says why: a T/2 equaliser is indifferent to
where inside the symbol the sampling falls, so timing only has to stop drift,
not find the eye.

**What to do.** `shared-dsp.md` §4.1 and §4.2 cost it out; follow them.

1. `dsp::Interpolator` — 64 taps, 256 phases, Kaiser β = 8, cutoff
   `0.5·baud·(1+β_rrc) + 300` capped at `0.45·fs`, normalised per phase, owning
   its own bounded history and answering "what is the signal at time *t*",
   returning `None` when the filter would reach past either end. `resample.rs`
   has the windowed-sinc arithmetic in the wrong shape (it pushes output at a
   fixed ratio, and windows with Blackman), so this is a new module beside it,
   not a change to it.
2. `dsp::HalfSpacedEqualizer` — 31 complex taps at T/2, NLMS with the step
   divided by the row energy (`v34/receiver.rs:950-951`), the error rotated back
   into the equaliser's own frame with `e · spin.conj()` **before** the tap
   update so the taps stay a static channel inverse and stop fighting the carrier
   loop for the same degree of freedom (`shared-dsp.md` §3.7,
   `v34/receiver.rs:947-954`), and adaptation gated on `dsp::Lock`'s `doubtful`
   from wave 4. Fed twice a symbol, sampled once — an `Option` return, the shape
   `Gardner::feed` already has.
3. V.32's timing detector becomes V.34's: the same taps applied to the central
   differences of the half-symbol samples, giving the output's rate of change,
   and the error's component along it as lateness in half symbols
   (`v34/receiver.rs:914-920, 959-967`). Second order, exactly critically damped
   (`Kp² = 8·Ki` to the digit), drift clamped and **reported in ppm**, which no
   slow mode can do today. `shared-dsp.md` §4.3 is right that this is a rider on
   the equaliser and not separable: there is nothing to take the derivative of
   until the T/2 filter exists.
4. `Equalizer` (symbol-spaced) stays exactly as it is, still used by V.22 bis,
   V.27 ter and V.29, so nothing else moves in this wave.

**Figures it must move:**

| where | now | must be |
|---|---|---|
| V.32 4800 `clean` slicer SNR | 36.5 dB | ≥ 42 dB (the 27–38 dB ceiling gone) |
| V.32 9600 `clean` slicer SNR | 31.3 dB | ≥ 36 dB |
| V.32bis 14400T `clean` slicer SNR | 30.7 dB | ≥ 36 dB |
| V.32bis 14400T arrival phases | 2/8 (≥ 6/8 after wave 2) | 8/8 |
| V.32bis 12000T arrival phases | 3/8 (≥ 6/8 after wave 2) | 8/8 |
| V.32bis 14400T SNR floor | 24 dB (≤ 21 after wave 3) | ≤ 18 dB |
| V.32bis 12000T SNR floor | 21 dB | ≤ 15 dB |
| phase table, 14400T `-200/+200 ppm` | 0/8 | 8/8 |

**Tests.** The whole sweep, and `one_mode_clean` for all six V.32 rows; `cargo
test -p dsp` with the new modules' own tests — for `interp.rs`, the measured
error power against exact interpolation at 6.667 samples per symbol asserted to
be better than −60 dB, which is the number `shared-dsp.md` §3.3 computes the
linear interpolator at −37.9 dB against; every V.32 test unedited; and, because
this rewrites the `feed` loop, `v32_vector` and `v32_replay` specifically.

---

## P11 `v22bis-lock` — finish V.22 bis and V.22

**Files:** `crates/datapump/src/v22bis.rs`. **Size:** S.
**Depends on:** `v32-lock` (for `dsp::Lock`), `v22bis-carrier` (same file).

**The fault.** With the rate latched (wave 1) and the carrier loop fixed (wave
4), V.22 bis has two cells left: `dropout 20 ms` at all three rows and
`level -6 dB` at 2400. Both are the same missing mechanism — nothing gates the
loops on the decision being credible, and nothing notices that the line went
away. `v22-and-bell.md` §3.8 measured the consequence directly: **if the noise
on a dropped line is loud enough to hold the carrier flag on — about 42 dB below
the signal that was there — nothing stops the equaliser adapting on it**, and
one second of noise 35 dB down takes the residual from 0.049 to 0.25 and it does
not come back in ten seconds. `since_carrier` is only reset on a carrier *edge*
(`v22bis.rs:642-647`) and `SQUELCH = 1e-7` (`v22bis.rs:117`) is the only other
gate, by which time the AGC has already amplified the noise to mean power 10.

**What to do.** Fit `dsp::Lock` to the V.22 bis slicers (`v22bis.rs:1006-1057`)
— four points at 1200, sixteen at 2400 — and gate the carrier loop, the
equaliser and the AGC on `doubtful`. No rewind: V.22 bis at 600 baud does not
run long enough between handshakes for the snapshot machinery to earn its
keep, and `shared-dsp.md` §4.5 says so. Report slips.

**Figures it must move:**

| where | now | must be |
|---|---|---|
| V.22 1200 `dropout 20 ms` | BER 0.0120 | < 1e-3 |
| V.22bis 1200 `dropout 20 ms` | BER 0.0120 | < 1e-3 |
| V.22bis 2400 `dropout 20 ms` | BER 0.0092 | < 1e-3 |
| V.22bis 2400 `level -6 dB` | BER 0.0173 | < 1e-3 |
| dropout table, V.22bis 2400 at 50 ms | BER 0.0177, carrier held | < 1e-3, slip reported |

**Tests.** The sweep and `two_things_worth_a_closer_look`; all four V.22 bis
test files unedited; `v22bis_capture`.

---

## P12 `v29-lock` — finish V.29

**Files:** `crates/datapump/src/v29.rs`. **Size:** S.
**Depends on:** `v32-lock` (for `dsp::Lock`), `v29-detector` (same file).

**The fault.** After wave 2, V.29 has the two level rows left. V.29's gain
control is a plain mean over the first 32 symbols of segment 2 and then a
71.5-symbol pole (`v29.rs:855-871`) — excellent for a burst that starts at a
known level, and 30 ms behind a step that happens mid-page, during which the
equaliser learns the wrong scale. The gate is what stops it.

**Figures it must move:**

| where | now | must be |
|---|---|---|
| V.29 9600 `level +6 dB` | BER 0.0016 | < 1e-3 |
| V.29 9600 `level -6 dB` | BER 0.0143 | < 1e-3 |
| V.29 7200 `level -6 dB` | BER 0.0228 | < 1e-3 |
| dropout table, V.29 9600 at 50 ms | BER 0.2176, carrier lost | < 1e-3, slip reported |

**Tests.** The sweep; `cargo test -p fax`, `faxcall`, and `v29.rs`'s own gain
test at `v29.rs:1243-1284`, which must still hold the gain to within 1 % from 0
to −40 dB.

---

## Where it should stand at the end

| mode | failed cases now | failed after | what is left |
|---|---|---|---|
| V.32bis 14400T | 15 | 1 | echo (no canceller in the harness) |
| V.32 9600T | 13 | 1 | echo |
| V.32bis 12000T | 13 | 1 | echo |
| V.32bis 7200T | 12 | 1 | echo |
| V.32 9600 | 8 | 1 | echo |
| V.22 1200 | 4 | 0 | — |
| V.22bis 2400 | 6 | 0 | — |
| V.29 9600 | 4 | 1 | echo (listener, 144–288 symbols out) |
| V.22bis 1200 | 3 | 0 | — |
| V.32 4800 | 3 | 1 | echo |
| V.29 7200 | 3 | 1 | echo |
| V.27ter 2400 / 4800 | 2 each | 1 each | echo |
| Bell 103 / V.21 | 1 each | 0 | — |

Arrival phases 8/8 everywhere; SNR floors at or below 15 dB for every mode
except the two top V.32bis rates, which are constellation-limited.

---

## Risks

1. **The sweep has no start-up and no training sequence.** `lock_sweep.rs`'s
   `transmit` pushes data into `v32::Transmitter` and `v22bis::Transmitter` from
   the first symbol; only V.29 and V.27 ter get their real training, because
   `start()` emits it. So every fix that depends on a training sequence —
   V.32's S/S̄/TRN, V.22 bis's unscrambled ones, and the `ReversalDetector` seed
   of `shared-dsp.md` §4.7 — **cannot move a single figure in `before.md`**. One
   of them (the seed) is folded into `v32-carrier` with that stated plainly and
   is proved by `v32_call`/`v32_startup` instead; the rest are under "not in this
   plan". An agent that tries to make a training-seeded change show in the sweep
   will either waste a day or, worse, change the harness to make it show.
2. **The harness rewrites `before.md`.** Copy it to `before-48b9087.md` first.
   Only one package per wave may touch `lock_sweep.rs` at all, and no package in
   this plan needs to.
3. **Above 9600 a single cell is a lottery.** `before.md` says so and provides
   the instrument: the phase-count table. Acceptance for anything above 9600 is
   the count out of eight, never the phase-0 row. Every cell is the median of
   three seeds; if a package's own runs use one seed, its numbers are not
   comparable to `before.md`'s.
4. **Additive or not at all, in `crates/dsp`.** `Gardner` is used by V.22 bis,
   V.27 ter, V.29 and V.32; `Equalizer` by the same four. A change to
   `Gardner::new` or `Equalizer::new` moves the fax modes and V.32 at once and
   the sweep will not tell you which change did it. The check is mechanical: the
   existing `shaping` (18) and `equalizer` (5) unit tests must pass **unedited**.
5. **V.34 and V.90 are protected by structure, not by care.** V.34 does not
   import `dsp::Equalizer` at all and nothing here touches `rrc_taps`,
   `fir_lowpass` or `Resampler`. The one shared file a V.34 path does use is
   `tone.rs` (`ToneDetector`, `ReversalDetector`), which `v32-carrier` changes —
   so that package must run `v34_vector` and `v34_capture` as well as the V.32
   suite.
6. **Tuning is where this goes wrong.** `v32-carrier`'s per-rate bandwidths and
   `v32-equaliser`'s hand-over fraction are the two places a number could be
   chosen to make a row pass. Both are specified here as *derived* from something
   already measured — the rotation margins in `v32.md` §8.2 and
   `point_spacing_at` — and a reviewer should reject any constant whose comment
   does not show the derivation.
7. **9600 uncoded has no two-modem test.** `agreed_coding` (`v32/startup.rs:539-548`)
   returns `Trellis` for any rate above 4800 whenever both ends are V.32bis,
   which every two-`Modem` test is, so 9600 uncoded is only reachable from the
   sweep. A regression there will not show in `v32_signals`. `v32-carrier`
   changes that rate's loop gain by a factor of about 2.7 and must watch the
   sweep row specifically.
8. **A green `v22bis_handshake` is not proof of `v22bis-carrier`.** The handshake
   acquires at 1200, where the decision regions are 90° wide, and hands a
   converged integrator to the 2400 stage. The ±2.2 Hz limit is a *cold*
   acquisition limit.
9. **`v32-fractional` rewrites a `feed` loop that three other modes copy.** It is
   last for that reason, and it should leave `Equalizer` alone so the three
   copies stay on the old path until someone measures them on the new one.
10. **Nothing may be taken from another implementation.** Every mechanism here is
    either already in this tree or read from a rendered Recommendation page. If a
    package finds itself wanting a constant it cannot derive, the answer is a
    measurement, not a search.

---

## Not in this plan, and why

These are real, some are severe, and none of them has a cell in `before.md`
because the sweep drives bare receivers.

| what | where | why not here |
|---|---|---|
| V.32bis clause 8 rate renegotiation, absent entirely; the 128-symbol retrain trigger cannot see a 64-symbol preamble, and a far end that renegotiates upward costs this modem the rate it had | `v32.md` §8.5 | start-up, not lock. The single largest V.32 gap after this plan |
| The rate signal is acted on the first time it arrives; `RateDetector::agreement()` exists, is documented with the measured evidence (53 consecutive right against a wrong one that ran twice) and **has no caller** | `v32.md` §8.1 | start-up. Cheap, and probably the next thing after this plan |
| V.22 bis 6.4 retrain: the far end on `live-1788613347.wav` asks nine times in fourteen seconds and is ignored; `handshake.rs:283` does nothing once connected | `v22-and-bell.md` W1 | handshake. `v22bis-lock` gives it the loss-of-equalisation signal 6.4 needs, which is the precondition |
| The echo canceller freezes at the end of the first TRN and never re-adapts, so a 20 ms jitter slip moves the echo outside both tap windows permanently; and μ = 0.5 freezes 8 dB of gradient noise into the taps | `v32.md` §8.4, §8.11 | start-up, and invisible to a harness with no canceller |
| R2 is not filtered by R1; the CA→AC join doubles the wrong state half the time; a cleardown is a silent hang-up | `v32.md` §8.6–§8.8 | start-up conformance |
| V.22 bis 2.3's fixed compromise equaliser in the transmitter ("shall"), the guard tones of 2.1/2.2, and the 2225 Hz answer of 6.3.1.2.2 Note | `v22-and-bell.md` W8, W9, W11 | transmitter and handshake |
| Carrier thresholds are raw sample-scale constants; V.21 8.3 and V.22 bis 3.3 both specify −43/−48 dBm at the line, and nothing calibrates | `v22-and-bell.md` W19, `shared-dsp.md` §3.4 | one calibration, five call sites, across every mode at once — its own piece of work |
| V.17 has no receiver at all: 267 lines of constants with no importer | `fax-qam.md` §5, `shared-dsp.md` §2 | a new mode, not a rework. `v32-fractional`'s interpolator and `dsp::Lock` are what it would be built on |

---

## How to run any of this

```text
cargo test -p datapump --release --test lock_sweep -- --ignored --nocapture
```

`--release` is not optional: the whole sweep is 45 s in release and tens of
minutes in debug. `one_mode_clean` is the fast loop while iterating (every mode
at every arrival phase on a clean line);
`two_things_worth_a_closer_look` is the two threshold probes.
Before the first run of any package, copy `before.md` aside — the test
regenerates it.
