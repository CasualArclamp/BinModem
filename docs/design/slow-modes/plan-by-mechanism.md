# Reworking the older modes, mechanism by mechanism

The judgement this starts from is Rory's: *the 33.6 QAM modes are locked in
place, but some of the older modes are using old code.* `v34-reference.md`
turned that into a list. `before.md` turned the list into numbers. This turns
the numbers into work.

The organising idea is **one mechanism at a time, across every mode that needs
it, with the code in `crates/dsp`**. Not one mode at a time. The reason is in
`shared-dsp.md` §3: the same problem has already been solved four times in four
files with four sets of constants, and two of the four copies still carry a
fault the other two fixed locally. Another round of per-mode fixes would make
that five. Every package below either puts a mechanism in the shared crate or
makes one mode use the shared one, and no package invents a mechanism a mode
could have borrowed.

Nothing here is taken from any other implementation. Every mechanism moved into
`crates/dsp` is moved out of `crates/datapump/src/v34/`, which was written from
the Recommendation, and every constant carries either the clause it is read
from or the measurement in `before.md` that sets it.

---

## 1. How the rules were read

Four of the constraints need a stated reading before the packages make sense.

**"V.34, V.90 and the fax modes must keep passing every existing test
unedited."** Read as a regression gate, not a no-touch rule — the fax modes are
V.29 and V.27 ter, and the by-mechanism angle cannot avoid their receivers.
Concretely:

- No package touches `crates/datapump/src/v34/**` or `crates/datapump/src/v90/**`.
  V.34's mechanisms are **copied** into `crates/dsp`, with the source cited, and
  V.34 goes on using its own. Switching V.34 over to the shared copy is a
  separate job for after this one; doing it here would put 107 V.34 tests and
  80 V.90 tests at risk for no measured gain.
- `crates/dsp` changes keep the existing constructors' behaviour bit-identical.
  Everything new is opt-in through a new constructor or setter. `Equalizer` is
  shared by four modes and `tone.rs` is shared with V.34 phase 2, so a default
  that moves is a default that moves V.34.
- The 15 V.27 ter, 14 V.29, 8 V.17 unit tests, `crates/fax`'s tests and
  `crates/datapump/tests/dil_sounds.rs` are a gate on every package that touches
  `v29.rs` or `v27ter.rs`, and none of them may be edited.

Baseline measured on `pr-1-deps` at the time of writing: `cargo test -p datapump
--lib` gives **297 tests, 0 failures** (107 v34, 80 v90, 27 v32, 17 v22bis, 15
v8, 15 v27ter, 14 v29, 8 v17, 7 framing, 6 v21, 1 bell103); `cargo test -p dsp
--lib` gives **75, 0 failures**.

**"Packages in a wave never touch the same file."** Enforced literally. Two
consequences worth stating: `crates/dsp/src/lib.rs` is touched only when a brand
new module is added, so at most one package per wave adds one — types added to
an existing `pub mod` need no export change and therefore no `lib.rs` edit. And
a mode file is owned by exactly one package per wave, which is why the waves run
**mechanism by mechanism across the modes** rather than mode by mode.

**"Each package names the figure in before.md it must move and by how much."**
Three cautions apply to every such figure, and a package that ignores them will
report a win it did not get:

1. Above 9600 the clean line only carries the payload at some arrival phases
   (V.32bis 14400T: **2/8**), so a single-phase cell cannot tell an impairment
   the receiver cannot take from the same lottery re-rolled. Every carrier and
   clock target above 9600 is stated as a **count out of eight phases**, from
   `before.md`'s "Is a carrier or clock failure the offset, or the arrival phase
   again?" table, not as one cell.
2. Every cell is the median of three seeds. A move smaller than the spread
   between seeds is not a move. Treat **a factor of three in BER, or 1.5 dB in
   slicer SNR, or one whole arrival phase** as the smallest difference worth
   claiming; anything less needs the seed count raised first.
3. `before.md` runs **no echo canceller anywhere**, so every V.32 echo row is
   the bare receiver and understates a whole V.32 modem. No package may claim an
   echo row until `proof-harness` has put the canceller in.

**"Every constant must cite its clause or its measurement."** Each package below
names its sources. Where a constant comes from V.34, the citation is the V.34
source line *and* the arithmetic in `v34-reference.md` §3 that justifies it, not
"V.34 uses this".

---

## 2. The five mechanisms, and where each mode stands

Read `v34-reference.md` §5 for the one-table version of what V.34 has. This is
the same table turned round, with `before.md`'s evidence against each cell.

| | timing | carrier | equalisation | level/AGC | loss detection |
|---|---|---|---|---|---|
| **Bell 103** | none needed — the async framer re-acquires every character (`framing.rs:63-95`) | none — amplitude-blind discriminator | none | none needed | flag drops after 11–16 ms of quiet; **dropout 20 ms → BER 0.0155, 2 slips** |
| **V.21** | first order, no integrator (`v21.rs:156`); standing error `8·sps·ε/d` | none | none | none needed | same flag; **dropout 20 ms → BER 0.0012** |
| **V.22 / V.22bis** | `Gardner` fed **raw** (`v22bis.rs:667`), normaliser starts at 1.0 | no open-loop seed; ζ = 2.59, ωn 0.93 Hz; **41.5 s to pull in 7 Hz** | T-spaced blind CMA; modulus 1.32 kept at 1200 where the truth is 1.0 | `OnePole::starting_at(10.0)`, τ = 30 symbols | none; V.22bis 6.4 retrain **absent** |
| **V.32 / V.32bis** | `Gardner` fed **raw** (`v32.rs:985`); linear interpolator, 27.3 dB ceiling | no open-loop seed; **14400 cannot pull 1 Hz in 51 s**; tracked on the **unequalised** symbol | T-spaced blind CMA; modulus 1.0 never retargeted above 4800 | `starting_at(10.0)`, τ = 120 symbols; gate counted from construction, not the carrier | none; clause 8 renegotiation absent |
| **V.29** | `Gardner` pre-scaled by 1/level ✓ | open-loop, one shot, no retry; **4800 A/B tie** | T-spaced blind CMA; modulus computed per rate ✓ | 32-symbol mean then exponential ✓ | detector latches on noise; acquisition never re-arms |
| **V.27 ter** | `Gardner` pre-scaled ✓ | open-loop, one shot, no retry | T-spaced blind CMA; modulus 1.0 correct ✓ | started at 1.0, never reset — harmless, 8-PSK | same latch; **dropout 20 ms → never locks** |
| **V.34** | data-aided, 2nd order, exactly critically damped, 200 symbols | 2nd order, ζ ≈ 1, BL 6.8 Hz, seeded by the training solve | **T/2, 31 taps, NLMS, seeded by least squares on the known sequence**, adapts in its own frame | none — the solve sets the scale | windowed error vs `settled`, rewind, two resynchronisers |

The four "none" cells in the loss-detection column, and the four T-spaced blind
equalisers, are the whole of the answer. Everything else is tuning.

---

## 3. What moves into `crates/dsp`, and from where

Seven mechanisms, each copied out of V.34 with its arithmetic, none of them a
new invention.

| new shared item | where it lives | copied from | what it is |
|---|---|---|---|
| `interp::Fractional` | `crates/dsp/src/interp.rs` (new) | `v34/receiver.rs:455-472`, `:662-678` | 64 taps × 256 phases, Kaiser β = 8, cut at `0.5·baud·(1+β_rrc) + 300 Hz` capped at `0.45·fs`, normalised per phase, over an owned complex history; answers "what is the signal at time *t*" |
| `Equalizer` T/2 + NLMS + own-frame adapt | `crates/dsp/src/equalizer.rs` | `v34/receiver.rs:909-954` (`STEP = 0.02`, `REACH = 15`) | `taps` at half-symbol spacing, step divided by row energy, error rotated back by `spin.conj()` before the update |
| `Equalizer::seed_from` | `crates/dsp/src/equalizer.rs` | `v34/receiver.rs:786-845`, `dsp::least_squares` (`complex.rs:142-163`), ridge `v34/receiver.rs:1280-1283` | one-shot least squares on a **known** training sequence, ridge `1e-3 · energy/taps`, accepted at 12 dB (`KNOWN_ENOUGH`, `v34/receiver.rs:90`) |
| `track::Carrier` | `crates/dsp/src/track.rs` (new) | `v34/receiver.rs:130-131`, `:955-958` | second order in turns, `Kp`/`Ki` chosen so ζ ≈ 1 rather than 2.59, detector `Im(z·conj(target))/max(\|target\|²,·)`, `seed_hz()`, phase carried forward by `turn` **outside** the gate |
| `track::Convergence` | `crates/dsp/src/track.rs` (new) | `v34/receiver.rs:928-945`, `Slicer::lost_*` `:180-196` | `doubtful = 0.25·min_distance²` gate, a window of recent squared errors, `settled` at α = 0.01, `lost_threshold = max(k·settled, floor)` |
| timing detector, data-aided | `crates/dsp/src/equalizer.rs` (rider on T/2) | `v34/receiver.rs:914-920`, `:959-967` | the same taps over central differences of the half samples; `late` normalised by an EMA of `\|rate\|²`; second order with `Kp² = 8·Ki` exactly |
| `ReversalDetector::drift_hz` | `crates/dsp/src/tone.rs` | `v34/probe.rs:188-243`; the measurement already exists at `tone.rs:353-360` | the offset the detector already computes and **throws away**; V.32's start-up already runs three of these (`v32/startup.rs:994-996`) |

Two V.34 mechanisms are deliberately **not** moved.

- **Rewind** (`v34/receiver.rs:971-1013`, 24 snapshots every 16 symbols).
  `shared-dsp.md` §4.5 is right: the snapshot is a struct of one receiver's own
  fields and a shared version would need every loop object `Clone` and every mode
  assembling its own snapshot anyway. Only V.32 runs long enough for it to pay.
  `v32-loss` copies the idea into `v32.rs` and nothing shared is built.
- **`resync_dense`** (`v34/receiver.rs:1027-1103`). It needs the raw history, the
  interpolator, the equaliser and the slicer at once. The shareable piece is the
  bounded raw history, which `interp::Fractional` owns; the search itself stays
  in V.34 until a second mode asks for it.

---

## 4. Where a mode genuinely needs something different

An FSK discriminator is not a QAM carrier loop, and three other places are the
same kind of mismatch. Stating them here so no package reaches for the shared
part by reflex.

**Bell 103 and V.21 get none of §3.** `arg(z·conj(z_prev))` is amplitude-blind
(`fsk.rs:96-99`), so there is no gain to control; there is no constellation, so
there is no `min_distance²` and no `Convergence`; and there is no carrier phase
to track, because the information is in the instantaneous frequency. What they
need instead is three things none of the QAM modes need:

1. **A running-mean offset removal on the discriminator output.** V.21 clause 3
   requires ±12 Hz of tolerance; a common-mode shift δ becomes a DC bias of
   δ/100 on the normalised output (`fsk.rs:114`) and the slicer is a hard zero in
   all three places that use it. At the required ±12 Hz one rail of the eye is
   12 % narrower than the other — about 1.1 dB given away that a mean would
   remove exactly. This is the FSK analogue of a carrier loop and it is a single
   pole, not a second-order loop.
2. **A minimum-duration guard on the carrier flag.** V.21 Table 2 requires
   ON→OFF in **20–80 ms** and OFF→ON in **300–700 ms** on the GSTN; measured is
   13–36 ms and 1.4–3.4 ms. The 20 ms floor exists precisely so a hole the length
   of a VoIP concealment insert does not reset state.
3. **An integrator on V.21's bit clock only.** `PULL = 0.125` with no integral
   leaves `8·sps·ε/d` samples of standing offset — 1707·ε at 16 kHz on a flag
   stream, half a symbol at ε = 1.56 %. Bell 103 needs nothing, because
   `AsyncFramer` dead-reckons from every start edge and tolerates 5.26 % by
   geometry.

**V.27 ter does not need gain precision.** Every point is on the unit circle and
a phase slicer does not care about radius; the AGC starting at 1.0 and never
being reset costs nothing in bits (`fax-qam.md` §2.3). It affects only
`residual_error()`'s units. Do not spend a package on it.

**V.29 and V.27 ter cannot rewind.** They are half duplex with one training
sequence at the front of a burst and no way back to it, so V.34's "put the loops
back to before they learned nonsense" has nothing to put them back *to*. The
right analogue is **re-running acquisition on a later part of the same burst**,
which is what `fax-qam.md` §3.2 asks for and what the 256-symbol segments make
possible. `resync`, not `rewind`.

**V.29's known-point acquisition must not be copied to V.17.** It ties whenever
`|A| = |B|` and the two are a quarter turn apart — true at V.29 4800 (§3.3) and
true at *every* V.17 rate. The fourth power, not the second, and not a `>`
tie-break.

**V.32 is the only slow mode that shares the band with itself**, so it is the
only one where the echo canceller sits inside the tracking problem, and the only
one where `before.md`'s echo rows understate the real modem.

---

## 5. The packages

Every package below states the files it may touch, what it depends on, what it
does, the tests that prove it, and the figure in `before.md` it must move. A
package that lands without moving its figure has not landed.

### Wave 1 — shared primitives (no mode file is touched)

---

#### `dsp-timing-gardner` — S
**Files:** `crates/dsp/src/shaping.rs`
**Depends on:** —

`Gardner` divides its error by a running mean power that **starts at 1.0**
(`shaping.rs:282`) and moves 2 % a symbol (`shaping.rs:337`). Thirty decibels
down it spends 345 symbols — 144 ms at 2400 baud, **575 ms at 600** — a thousand
times too timid to move, and a receiver started half a symbol out of step stays
there: `(sps/2)/1e-4` = 33 000 symbols to cross. V.29 and V.27 ter work round it
by pre-scaling (`v29.rs:832-839`, `v27ter.rs:843-845`); V.22bis and V.32 never
got the memo.

Fix it where it cannot be got wrong again: `Gardner::with_power(sps, gain,
power)` seeding the normaliser, and `set_reference_power` so a caller that knows
its own AGC target can hand it over. `Gardner::new` keeps today's behaviour
exactly, so V.29 and V.27 ter are untouched until their own packages opt in.

**Tests:** `cargo test -p dsp --lib` (75 must stay 75 passing, none edited); new
`shaping::tests::a_seeded_normaliser_finds_the_instant_thirty_decibels_down` —
the same signal at 0 dB and −30 dB must reach the same sampling instant within
one sample in the same number of symbols; new
`shaping::tests::an_unseeded_gardner_behaves_exactly_as_before` pinning the old
path.
**Expected:** no `before.md` cell on its own — it is the enabling half of
`v22-front` and `v32-front`. Judged there.

---

#### `dsp-timing-interp` — M
**Files:** `crates/dsp/src/interp.rs` *(new)*, `crates/dsp/src/lib.rs`
**Depends on:** —

All four slow modes interpolate the matched-filter output with **two-point
linear interpolation**, in four identical copies (`v22bis.rs:662-666`,
`v27ter.rs:830-834`, `v29.rs:814-818`, `v32.rs:980-984`). Measured error against
exact interpolation: −57.0 dB at V.22bis's 26.667 sps, exact at V.27 ter 4800's
10.000, −47.2 dB at V.27 ter 2400's 13.333, and **−37.9 dB at V.29's and V.32's
6.667** — a hard SNR ceiling of 38 dB however clean the line, and about half a
decibel of the 14400 margin.

Port `v34/receiver.rs:455-472` and `:662-678` into `crates/dsp/src/interp.rs` as
`Fractional`: 64 taps, 256 phases, Kaiser β = 8, cutoff `0.5·baud·(1+β_rrc) +
300` capped at `0.45·fs`, rows normalised to unit sum, owning a bounded history
deque and returning `None` when the filter would reach past either end. The
quantisation is 1/256 of a sample; at V.32's 6.667 sps that is 1/1707 of a
symbol, three orders below anything the timing loop cares about.

`resample.rs` is **not** reusable for this: it pushes output at a fixed ratio
rather than answering "what is the signal at time *t*", and its window is
Blackman. This is a new module beside it, and that is said in the module comment.

**Tests:** new `interp::tests::the_table_reproduces_a_band_limited_signal` —
error power against an exact pulse sum below −70 dB at every one of 256 phases,
for each of 6.667, 10.0, 13.333 and 26.667 sps; new
`interp::tests::it_refuses_to_reach_past_the_history`; new
`interp::tests::each_phase_row_sums_to_one`. `cargo test -p dsp --lib`.
**Expected:** proved at adoption. The figure it is judged on is **V.32 4800
clean slicer SNR 36.5 dB → ≥ 42 dB** (`v32-front`), because 4800 is the mode
whose points are far enough apart that the interpolator is what limits it.

---

#### `dsp-eq-target` — S
**Files:** `crates/dsp/src/equalizer.rs`
**Depends on:** —

Two faults, both one number, both measured in `shared-dsp.md` §3.6.

The blind stage hands over to decision direction when the running mean error
falls below a bare `0.25` (`equalizer.rs:121`), on a scale that means something
different in every mode. Against the distance between neighbouring points at
unit mean power: 18 % at V.32 4800, 40 % at V.22bis 2400, 56 % at V.32 9600,
**81 % at 12000 and 113 % at 14400**. At 14400 the eye hands over while the mean
error is larger than the whole distance between points, and the file's own header
says what happens then — "Starting decision-directed on a closed eye simply
reinforces whatever nonsense it first decides."

And `modulus` is set once in `new` and never changed. `V32::Receiver::follow`
(`v32.rs:949-959`) recomputes the loop bandwidth on a rate change and leaves the
equaliser alone; `V22bisReceiver::set_rate` (`v22bis.rs:920-922`) does the same.
True R₂ is 1.310 at V.32 9600, 1.381 at 12000, 1.343 at 14400 against a passed
1.0, and 1.0 at V.22bis 1200 against a passed 1.32. The CMA settles where
`g²·R₂_true = R₂_target`, so 1.0 against 1.310 converges **12.6 % small,
−1.18 dB**, and 1.32 against 1.0 converges 15 % large.

Add `Equalizer::with_spacing_target(taps, modulus, half_spacing)` and
`retarget(modulus, half_spacing)`, and make the handover threshold
`0.5 · half_spacing` — half the distance to the decision boundary, the same
quantity `doubtful` is in V.34 (`v34/receiver.rs:928`) and defensible in one
line. `new` keeps 0.25 so nothing moves until a mode opts in.

**Tests:** `cargo test -p dsp --lib`; new
`equalizer::tests::the_handover_waits_for_the_eye_at_every_spacing` — drive the
five spacings of `shared-dsp.md` §3.6's table and assert the handover error is
under half the spacing in each; new
`equalizer::tests::retargeting_moves_the_blind_equilibrium` — the converged
output scale against a known R₂ is within 2 % of unity after `retarget` and 12.6 %
out without it.
**Expected:** proved at adoption in `v32-eq` and `v22-eq`. On its own it is the
two-line half of `evidence.md` §5 Q1 and must be measured *before* the T/2 change
so the sweep can say which of the two mattered.

---

#### `dsp-carrier-drift` — S
**Files:** `crates/dsp/src/tone.rs`
**Depends on:** —

`ReversalDetector` already measures the carrier offset — `drift` at
`tone.rs:353-360` is the exponentially averaged turn per history step of the
phasor — and uses it **only to refuse** (`tone.rs:399`). V.32's start-up runs
three of these on the carrier and both sidebands (`v32/startup.rs:994-996`), so
on every V.32 call the number `v32.rs`'s carrier loop needs is already being
computed and thrown away. `shared-dsp.md` §8 calls this the cheapest large win
in the tree and it is.

Expose `drift_hz()`. And make the refusal gate a per-detector parameter:
`f_max = π·bw/24 = 7.854 Hz` at bw = 60 (from `tone.rs:57`, `:211`, `:216`,
`:399`, with `fs` cancelling), while **V.32 2.1 requires the receiver to operate
at ±7 Hz** — so a conforming line sits at 89 % of the gate. `with_carry(max)`,
defaulting to today's `MAX_CARRY = π/4`, so `v34/phase2.rs:148` and `:338` are
untouched and the 107 V.34 tests cannot move.

**Tests:** `cargo test -p dsp --lib`; new
`tone::tests::the_drift_reads_the_offset_it_was_given` — a reversing tone at
−7, −3, 0, +3, +7 Hz, `drift_hz()` within 0.5 Hz after settling, which brackets
the 7 Hz the crate's own tests skip (they bracket 3 and 30); new
`tone::tests::a_wider_carry_still_refuses_thirty_hertz`.
**Expected:** enables `v32-carrier`. Judged there.

---

#### `dsp-level-core` — M
**Files:** `crates/dsp/src/filter.rs`
**Depends on:** —

Two shared pieces, both in `filter.rs` beside `OnePole` because that is what
both are made of, and both purely additive.

**`Agc`** — V.29's two-stage shape, generalised. `v29.rs:842-854` records why it
exists: an exponential average starting at 1.0 "was still four times too high
when the data began, so the equaliser learned the whole training at the wrong
gain and then had to unlearn it on the page". V.22bis and V.32 still start at
10.0 and crawl at τ = 30 and 120 symbols. `Agc::new(target, first_window, tau)`
takes a plain running mean over the first `first_window` symbols — exact on the
first sample, so no start value is needed at all — then a one pole. Plus
`step_detected()`: a level change of more than 3 dB against the settled estimate
re-arms the first stage, which is what a ±6 dB step needs and what nothing has.

**`Presence`** — the carrier detector, one shape for all five copies. Today:
`CARRIER_ON = 1.0e-3` / `CARRIER_OFF = 5.62e-4` at `fsk.rs:49-50`,
`v22bis.rs:129-130` and `v32.rs:289-290`, and an adaptive `max(4·floor, 1.0e-3)`
at `v27ter.rs:557-591` and `v29.rs:530-543`. Three of those answer the same
physical question on three incompatible scales. `Presence` carries the adaptive
floor the fax modes already have, **plus** the two things none of them have: a
minimum-duration guard so the flag cannot rise on a transient, and configurable
ON and OFF delays so a caller can meet its own clause. The windows to meet are
V.22bis 3.2 (OFF **40–65 ms**, ON **40–205 ms**), V.21 Table 2 (OFF **20–80 ms**,
ON **300–700 ms**), V.29 5.2.2 (**30 ± 9 ms**) and V.27 ter 3.6 (**30–50 ms**) —
all rendered, all different, which is exactly why the shared part is the
mechanism and the numbers stay with the caller.

Note in the module comment that the ON/OFF **levels** stay uncalibrated: V.22bis
3.3 and V.21 8.3 both give −43/−48 dBm at the line and nothing in the tree maps
full scale to dBm. `1.0e-3` sits 53.5 dB below this modem's own transmitter at
zero loss, 23.5 dB below what the clause asks for. That is an engineering choice
made against a real line's noise floor (`fsk.rs:121-131`), and it is not the
number the Recommendation gives; calibration is out of scope here and is written
down as an open question, not silently left.

**Tests:** `cargo test -p dsp --lib`; new
`filter::tests::the_gain_is_right_from_the_first_symbol_forty_decibels_down` —
within 1 % at 0, −20 and −40 dB, matching the bound `v29.rs:1243-1284` already
holds V.29 to; new `filter::tests::a_six_decibel_step_is_caught_and_re_armed`;
new `filter::tests::the_flag_honours_its_on_and_off_windows` driven at each of
the four clause windows above; new
`filter::tests::a_transient_shorter_than_the_guard_does_not_raise_the_flag`.
**Expected:** proved at adoption. The figures are the level rows of `before.md`,
claimed by `v22-front`, `v32-front` and `fax-front`.

---

### Wave 2 — the rest of the shared crate, the FSK side, and the proof

---

#### `dsp-carrier-core` — M
**Files:** `crates/dsp/src/track.rs` *(new)*, `crates/dsp/src/lib.rs`
**Depends on:** `dsp-eq-target`

There is no shared carrier loop: four copies, four gain pairs, two acquisition
strategies (`shared-dsp.md` §3.1). Every one of them keeps `phase` and
`frequency` in turns while the error arrives in radians, so the effective
constants are `Kp = gain·2π` and `Ki = gain·2π`. For V.22bis that is
`Kp = 0.0503`, `Ki = 9.425e-5`, **ζ = 2.59** — heavily overdamped, ωn = 0.93 Hz
at 600 baud. V.32 scales by `bw`/`bw²` to hold ζ at 2.59 for every
constellation, which narrows ωn to 0.92 Hz at 14400.

`track::Carrier` is one loop with the units stated once: error in radians,
state in turns, `Kp` and `Ki` given as the closed-loop pair rather than as raw
gains, and a constructor that takes the wanted natural frequency and damping and
derives them. V.34's pair — `PHASE_GAIN = 0.04`, `FREQUENCY_GAIN = 4e-4`
(`v34/receiver.rs:130-131`) — gives roots 0.98264 and 0.97697, **ζ ≈ 1.0**,
ωn = √Ki = 0.02 rad/symbol, one-sided noise bandwidth 6.8 Hz at 3429 baud,
settling in 58 symbols. That is the shape to copy: critically damped, not
overdamped. Detector `Im(z·conj(target))/max(|target|², floor)`, which is
`sin φ` normalised and therefore unit gain; V.34's floor is 0.1 at unit power
against V.32's `max(|c|², 5.0)` = half the mean power, which attenuates signal
and noise together and costs 15 % of loop gain at 16 points.

Two things the shared loop must carry that no copy has:

- **`seed_hz(hz, baud)`** — set the integrator outright from an open-loop
  measurement, which is how V.27 ter and V.29 already meet ±7 Hz
  (`v27ter.rs:892-893`, `v29.rs:932-933`) and how V.34 starts from
  `solution.turn`. The comments record what it is worth: "every one of forty
  tries at plus or minus seven hertz failed" without it.
- **phase carried forward by `turn` outside the gate** (`v34/receiver.rs:946`),
  so the frequency estimate keeps running the phase forward while everything else
  is frozen.

`track::Convergence` goes in the same module because it is the thing that does
the freezing: `doubtful = 0.25 · min_distance²`, a window of recent squared
errors (V.34 uses 8 for four or sixteen points, 32 for a dense grid,
`v34/receiver.rs:210-215`), `settled` at α = 0.01 over the **passing** symbols,
and `lost_threshold = max(k·settled, floor)` so the threshold is always relative
to what this line actually reads. One gate governs the equaliser, the carrier
loop and the timing loop, and that single gate is why V.34 survives a burst of
wrong decisions where the old modes learn from them.

**Tests:** `cargo test -p dsp --lib`; new
`track::tests::a_steady_offset_leaves_no_standing_error` — ±7 Hz at 600, 1200,
1600 and 2400 baud, residual rotation under 1° after settling; new
`track::tests::the_loop_is_critically_damped_at_every_baud` — closed-loop roots
real, ζ within 0.1 of 1.0; new
`track::tests::seeding_removes_the_pull_in` — settling within 100 symbols at
7 Hz seeded against thousands unseeded; new
`track::tests::the_gate_freezes_on_a_burst_of_wrong_decisions`; new
`track::tests::loss_is_declared_against_what_this_line_reads` — the same
absolute error is a loss on a clean line and not on a noisy one.
**Expected:** proved at adoption in `v22-carrier` and `v32-carrier`.

---

#### `dsp-eq-halfspaced` — L
**Files:** `crates/dsp/src/equalizer.rs`
**Depends on:** `dsp-eq-target`, `dsp-timing-interp`

The single biggest structural difference, and the root of the worst fault in
`before.md`. A **symbol-spaced** equaliser — which is all `equalizer.rs` can be
today (`equalizer.rs:87`, one sample in per symbol) — sees the folded channel and
cannot compensate for the sampling phase: at the worst timing phase the fold puts
an exact null at the band edge, which the filter must invert with unbounded gain.
That is why V.32 above 9600 settles into one of exactly two states depending on
nothing but how many samples of silence preceded the carrier, and why the bad
state's residual sits stable at 0.225–0.245, just under the 0.25 handover — the
filter converges, to the wrong minimum, and decision direction holds it there for
ever. `evidence.md` §3.1 has the two attractors side by side; `before.md` has the
consequence as **2/8 arrival phases at V.32bis 14400T**.

Add, all opt-in:

- **T/2 spacing.** `spacing: usize` (1 or 2 samples per tap); 31 taps at T/2 is
  15.5 symbols of line memory, `REACH = 15` in `v34/receiver.rs:62`. Fed twice a
  symbol, output sampled once — `Option<(f64,f64)>` from `feed`, the shape
  `Gardner::feed` already has.
- **NLMS.** Step divided by the row energy (`v34/receiver.rs:950`), so
  convergence time is independent of level. μ = 0.02 gives a misadjustment time
  constant of about 50 adapting symbols.
- **Adapt in the equaliser's own frame.** `adapt_rotated(output, decision, spin)`
  rotates the error back by `spin.conj()` before the tap update
  (`v34/receiver.rs:951`), so the taps stay a static channel inverse. Today both
  V.22bis (`v22bis.rs:735-740`) and V.32 (`v32.rs:1049-1053`) equalise the
  already-derotated symbol and adapt against the derotated decision, so while the
  carrier loop is still pulling in the equaliser is chasing a rotating channel
  with a 125-symbol time constant — 52 ms at 2400 baud, over which 7 Hz turns
  131°. The two loops fight for one degree of freedom.
- **`seed_from(rows, targets)`** — one-shot least squares on a known sequence,
  through `dsp::least_squares` (`complex.rs:142-163`, normal equations +
  Cholesky) with the ridge rule `1e-3 · energy/taps`
  (`v34/receiver.rs:1280-1283`), alignment searched over ±8 half samples
  (`SEARCH`, `v34/receiver.rs:81`), accepted at `-10·log10(mse) ≥ 12` dB
  (`KNOWN_ENOUGH`, `:90`). This is the mechanism the older modes have no analogue
  of at all, and every one of them has a known training sequence in its
  Recommendation: V.32 5.2.3 defines TRN completely, down to the scrambler's
  all-zero initial state and differential encoding disabled; V.29 Table 2's
  segment 2 alternates two known points; V.27 ter's segments 1, 3 and 4 are all
  two-phase; V.22bis's unscrambled ones and double dibit are known patterns.
- **The data-aided timing detector** as a rider (`v34/receiver.rs:914-920`,
  `:959-967`): the same taps applied to central differences of the half samples,
  `late` normalised by an EMA of `|rate|²`, clamped to ±0.5 half symbols,
  second order with `TIMING_GAIN = 0.01` and `DRIFT_GAIN = 1.25e-5` — where
  `Kp² = 8·Ki` exactly, giving a double root at z = 0.995 and a 200-symbol time
  constant. It is strictly better than Gardner where a decision exists: unbiased
  for any constellation (Gardner assumes constant modulus, which is what
  `shaping.rs:321-328` records thrashing on sixteen points), needing no power
  normalisation, and giving a calibrated ppm figure. `drift` clamped to ±0.001 =
  ±1000 ppm, against the ±200 ppm two conforming modems can be apart.

Everything above is reachable only through the new constructor. `Equalizer::new`
must produce byte-identical taps on the same input as today, pinned by a test,
because V.29 and V.27 ter go on using it until `fax-eq`.

**Tests:** `cargo test -p dsp --lib`; new
`equalizer::tests::the_old_constructor_has_not_moved` — a fixed pseudo-random
input, taps compared to a stored vector; new
`equalizer::tests::a_half_spaced_filter_is_indifferent_to_the_sampling_phase` —
the same channel at 16 sampling phases, converged residual spread under 1 dB,
against the symbol-spaced filter on the same input where it is not; new
`equalizer::tests::a_known_sequence_trains_in_one_shot` — seeded MSE ≥ 12 dB
after 300 symbols where blind CMA has not opened the eye; new
`equalizer::tests::the_timing_loop_is_exactly_critically_damped`; new
`equalizer::tests::two_hundred_ppm_is_tracked_to_better_than_thirty_eight_decibels`
mirroring `v34/receiver.rs:1420-1437`.
**Expected:** this is the package the arrival-phase column depends on. Claimed by
`v32-eq`: **V.32 9600 6/8 → 8/8, V.32bis 7200T 7/8 → 8/8, V.32 9600T 3/8 → 8/8,
V.32bis 12000T 3/8 → 8/8, V.32bis 14400T 2/8 → 8/8.**

---

#### `fsk-hold` — L
**Files:** `crates/dsp/src/fsk.rs`, `crates/datapump/src/framing.rs`,
`crates/datapump/src/v21.rs`, `crates/datapump/src/bell103.rs`
**Depends on:** `dsp-level-core`

The whole FSK side, which needs §4's different mechanisms and none of §3's.
One package because the four files are one mechanism and no other package
touches them.

**Carrier hold.** With a 5 ms envelope the flag drops after
`0.005·ln(L/5.62e-4)` — **11.5 ms** from 20 dB above threshold, 16 ms from a
−30 dBFS line. `AsyncFramer::feed` on `!carrier` sets `State::Idle` and throws
away the character in flight (`framing.rs:52-57`); `v21::Receiver::feed` sets
`running = false` and re-phases HDLC (`v21.rs:176-180`). A 20 ms concealment
insert therefore costs a character on Bell 103 and a whole frame on V.21, every
few seconds. Adopt `Presence` with V.21 Table 2's **20–80 ms** ON→OFF window
(rendered, page 4 of `T-REC-V.21-198811-I.pdf`) and **300–700 ms** OFF→ON, which
is the clause and is also exactly what stops this.

**Offset removal.** A running mean on the discriminator output before the hard
zero slicer, at a time constant long against the longest legal run of like bits
and short against a drifting line. At ±12 Hz the bias is 0.120 of full
discriminator scale and one rail of the eye is 12 % narrower than the other;
removing it is 1.1 dB back for one pole. The sign of the half-shift stays where
it is (`fsk.rs:114`) — it is what makes V.21's mark-below-space read the same way
round as Bell 103's mark-above-space, and `fsk.rs:150-173` pins it.

**V.21's bit clock gets an integrator.** First order with `PULL = 0.125` and no
integral leaves `x = 8·sps·ε/d` samples standing — 1707·ε at 16 kHz on a flag
stream, half a symbol at ε = 1.56 %, measured failing between 1.0 % and 1.5 %
(324/448 bits at +1.5 %). The clock tolerance to meet is V.21's own: ±0.01 %
each end is not stated for V.21, so the target is the measured shape — no
standing offset at ±200 ppm and graceful past 1 %.

**The framing-error counter gets fixed.** A far end that is too *slow* trips the
stop-bit check and is counted; one that is too *fast* slips into the stop bit,
which is still mark, so every character is wrong and `framing_errors` stays at
**zero** — `login:` arrives as `ac af a7 a9 ae ba`. The one health counter at
300 bit/s is blind in one of the two directions. Count a mid-character edge that
arrives early as well.

While in the file, delete `slow_env` (`fsk.rs:81`, `:105`) — fed every sample,
never read, left over from the ratio detector `fsk.rs:122` describes removing —
and correct the four citations of "V.22bis 6.5.2" (`fsk.rs:45-48`, `:285-287`).
**There is no clause 6.5.2**; 6.5 is "Operation after loss of line signal" and
the hysteresis requirement is 3.3.

**Tests:** `cargo test -p datapump --lib bell103:: v21:: framing::` (1 + 6 + 7
today, none edited); `cargo test -p datapump --test bell103_loopback --test
bell103_vector`; `cargo test -p dsp --lib`; new
`v21::tests::a_twenty_millisecond_hole_costs_the_hole_and_nothing_more`; new
`framing::tests::a_fast_far_end_is_counted_as_well_as_a_slow_one`; new
`fsk::tests::twelve_hertz_of_drift_leaves_both_rails_equal`; new
`v21::tests::a_thousand_ppm_leaves_no_standing_offset`.
**Expected:** **Bell 103 dropout 20 ms: BER 0.0155 and 2 slips → BER ≤ 0.006,
0 slips.** The hole itself is 6 bits of a 1500-bit payload — a floor of 0.004 —
so 0.0155 is four times the hole's own cost plus two lost characters, and the
target is the floor plus one character. **V.21 dropout 20 ms: BER 0.0012 →
0 with the carrier held** (20 ms is inside Table 2's 20–80 ms window, so the flag
should not drop at all). Both modes' remaining rows must not move: Bell 103 and
V.21 are 8/8 phases, 6 dB SNR floor and clean everywhere else, and V.21's echo
row (slicer SNR 8.9 dB, BER 0) must stay passing.

---

#### `proof-harness` — M
**Files:** `crates/datapump/tests/lock_sweep.rs`,
`crates/datapump/tests/margins.rs` *(new)*
**Depends on:** —

`lock_sweep.rs` is already the measuring instrument and needs three additions
before any later package can claim a number.

1. **The echo canceller in the V.32 rows.** Today there is none anywhere, so
   `V.32 9600 echo 0.57 @ 120 ms → BER 0.5008` is the bare receiver and means
   nothing about a whole V.32 modem, which cancels before the receiver
   (`v32/startup.rs`, exercised at `v32_loopback.rs:157`). Add it, keep the
   canceller-free rows beside it, and label both.
2. **Report the loss signal.** The `carrier lost` column reads "no" for V.22bis
   and V.32 at **every** hole length up to 50 ms. That is correct behaviour under
   V.22bis 3.2 — a 20 ms hole is shorter than the 40 ms OFF window — and it is
   also why nothing above is ever told. Add a column for the receiver's own
   loss-of-equalisation signal, so `v22-loss` and `v32-loss` have something to
   move.
3. **Raise the seed count where a threshold is being decided.** Three seeds set
   the smallest claimable difference; the SNR-floor and arrival-phase columns are
   thresholds and want more.

`margins.rs` is `evidence.md` §4.6's table, one test per requirement, each
naming its clause, every one of them `#[ignore]`d on arrival with the reason
and the current figure in the ignore comment, so the normal suite is untouched
and the list is visible:

| test | clause | today |
|---|---|---|
| `v22bis_2400_holds_seven_hertz_of_offset` | V.22bis 2.6 | fails, holds 1.5 Hz |
| `v22bis_1200_holds_seven_hertz_of_offset` | V.22bis 2.6 | marginal |
| `v32_holds_seven_hertz_at_every_rate` | V.32 2.1 | fails above 9600 |
| `every_v32_rate_acquires_from_every_arrival_phase` | — | fails at 7200 and above |
| `the_carrier_turns_on_between_40_and_205_ms` | V.22bis 3.2 | fails, 22.8 ms |
| `the_carrier_turns_off_between_40_and_65_ms` | V.22bis 3.2 | fails, 148 ms |
| `the_answer_tone_does_not_raise_circuit_109` | V.22bis 3.3 | fails |
| `the_carrier_holds_across_a_hundred_millisecond_dropout` | — | passes silently |
| `two_clocks_two_hundred_ppm_apart_still_carry_data` | V.22bis 2.5.1, V.32 2.3 | untested properly |
| `each_rate_carries_data_at_the_signal_to_noise_ratio_it_needs` | — | untested |
| `v27ter_and_v29_carry_a_page_over_an_impaired_line` | — | untested |

**Tests:** itself. `cargo test -p datapump` must report the same pass count as
the baseline with the new file's tests ignored; `cargo test -p datapump --release
--test lock_sweep -- --ignored --nocapture` must still reproduce `before.md`
row for row on the unchanged tree, which is the check that the harness changes
did not move the measuring stick.
**Expected:** moves nothing. It makes the V.32 echo column mean something and
gives `v22-loss`, `v32-loss` and `fax-loss` a column to move.

---

### Wave 3 — the front end: sampling instant and level

---

#### `v22-front` — M
**Files:** `crates/datapump/src/v22bis.rs`
**Depends on:** `dsp-timing-gardner`, `dsp-timing-interp`, `dsp-level-core`

Three front-end changes, all adoption.

`Gardner` seeded with the AGC's own target instead of 1.0 (`v22bis.rs:667-668`
feeds the raw matched-filter output). At 600 baud the unseeded normaliser takes
**575 ms** to come within a factor of two of a power 30 dB down — the whole of a
V.22bis handshake.

`interp::Fractional` in place of the two-point interpolation at
`v22bis.rs:662-666`. At 26.667 sps the linear error is only −57.0 dB, so this is
the smallest of the four interpolator wins and it is taken for uniformity, not
for the decibels.

`Agc` in place of `OnePole::starting_at(10.0, 0.050, 600.0)` at `v22bis.rs:594`.
Thirty symbols is fast, and 16-QAM's power variance about its mean of 10 is 32
(the file says so at `v22bis.rs:806-809`), so a 30-symbol EWMA has a standard
deviation of 7.3 % in power — **3.6 % RMS in amplitude**, about 11 % of the outer
points' decision margin, spent on the estimator. The first-window mean removes
the start transient and the step detector re-arms it; keep the `MAX_GAIN = 400`
clamp and the `mean_power ≥ 1e-9` floor.

**Tests:** `cargo test -p datapump --lib v22bis::` (17, none edited);
`cargo test -p datapump --test v22bis_loopback --test v22bis_vector --test
v22bis_handshake`, in particular `the_signal_may_arrive_at_any_moment:249`,
`the_two_clocks_need_not_agree:269`, `a_carrier_that_arrives_after_a_pause_is_still_acquired:343`
and `the_offer_of_2400_is_read_from_every_starting_phase:640`, all unedited;
`cargo test -p datapump --release --test lock_sweep -- --ignored one_mode_clean`.
**Expected:** **V.22bis 2400 level −6 dB: BER 0.0173 → < 1e-3, lock 401 ms →
≤ 200 ms; level +6 dB slicer SNR 18.5 dB → ≥ 26 dB and −6 dB 19.1 → ≥ 26 dB**
(clean is 28.6, so the 10 dB drop is the AGC transient averaged into the run).
**V.22 1200 and V.22bis 1200 level ±6 dB slicer SNR 18.6 dB → ≥ 34 dB** against a
clean 40.8. Arrival phases and the 6 dB SNR floor must not move.

---

#### `v32-front` — L
**Files:** `crates/datapump/src/v32.rs`, `crates/datapump/src/v32/startup.rs`
**Depends on:** `dsp-timing-gardner`, `dsp-timing-interp`, `dsp-level-core`

The same three adoptions, where they are worth far more, plus the one gate bug.

The interpolator is the sharp edge here: at 6.667 samples per symbol the
two-point interpolation of `v32.rs:980-984` has a worst-case error of 0.0431 of
full scale — an **SNR ceiling of 27.3 dB** that no line quality can lift, which
against half the distance between neighbouring points is 6 % at 4800, 19 % at
9600 trellis, 28 % at 12000 and **39 % at 14400**.

`Gardner` seeded (`v32.rs:985-986` feeds raw). `Agc` in place of
`starting_at(10.0, 0.050, 2400.0)` at `v32.rs:908` — τ = 50 ms is **120 symbols**
here. Every V.32 and V.32bis constellation has mean power exactly 10 after
scaling (`trellis.rs:416-440`, checked at `v32.rs:1420-1432`), so the AGC still
needs no telling about the rate; that much is right and stays right.

**And the gate.** `v32.rs:1073` gates the equaliser on `self.symbols > 64`,
counted from **construction**, not from the carrier. V.22bis fixed exactly this
bug — `v22bis.rs:642-647` resets `since_carrier` on a carrier transition, with a
long comment at `:502` about why — and V.32 still has it. This is the whole of
`before.md`'s "What does a silence before the carrier do to V.32?": with 1.1 s of
silence in front, the gate has long expired, the AGC has been amplifying nothing,
and the equaliser adapts straight into the acquisition transient. V.22bis, the
structurally identical receiver, holds **8/8 at every silence from 0 to 1100 ms**
and V.32 4800 collapses to **0/8 at 300 ms** — the only impairment in the whole
sweep that breaks V.32 4800, which is otherwise eight phases out of eight on
everything. Reset the gate, the AGC and the Gardner loop on the carrier edge, and
hold all three while the far end is quiet.

**Tests:** `cargo test -p datapump --lib v32::` (27, none edited); `cargo test -p
datapump --test v32_loopback --test v32_startup --test v32_call --test v32_bits
--test v32_both_ends --test v32_rate_framing --test v32_reversals --test
v32_signals --test v32_vector --test v32_who`, all unedited — `v32_call.rs`
especially, because it is the test that has been hiding the equaliser fault and
must go on passing while the fault is fixed elsewhere; `cargo test -p datapump
--release --test lock_sweep -- --ignored two_things_worth_a_closer_look`.
**Expected:** **V.32 4800 round trip 1.1 s: never → lock ≤ 30 ms, BER 0.4898 →
< 1e-3**, and the silence table **300 ms 0/8 → 8/8 at every silence from 0 to
1100 ms**. **V.32 4800 clean slicer SNR 36.5 dB → ≥ 42 dB** — the interpolator's
own figure, and the one number that says the 27.3 dB ceiling is gone. **V.32 9600
round trip 1.1 s lock 905 ms → ≤ 200 ms.** **Level −6 dB: V.32 9600 BER 0.0306 →
< 1e-3, 9600T 0.0266 → < 1e-3, 12000T 0.0308 → < 1e-3, 14400T 0.0394 → < 1e-3;
level +6 dB 14400T 0.0944 → < 1e-3.** The carrier and clock columns above 9600
are **not** this package's to claim.

---

#### `fax-front` — M
**Files:** `crates/datapump/src/v29.rs`, `crates/datapump/src/v27ter.rs`
**Depends on:** `dsp-timing-interp`, `dsp-level-core`

These two already pre-scale the Gardner input (`v29.rs:832-839`,
`v27ter.rs:843-845`) and V.29's two-stage AGC is the shape the others are
copying, so the front-end work here is narrower.

`interp::Fractional` in both. It buys −37.9 dB → below −70 dB for V.29 at 6.667
sps and −47.2 dB for V.27 ter at 2400's 13.333 sps, and **nothing at all** for
V.27 ter 4800, whose 10.000 samples per symbol make linear interpolation exact —
adopt it there anyway for one code path, and say in the comment that it is free.

`V.27 ter's select filter` stays at 1400 Hz when the rate drops to 2400
(`v27ter.rs:669`; `set_rate` at `:700-710` rebuilds the matched filter, the
Gardner loop and the AGC but not this), where the signal only reaches 900 Hz.
1.9 dB of noise into the level meter and the timing loop for nothing.

**And the V.29 7200 anomaly.** `before.md` has V.29 7200's SNR floor at **24 dB**
and V.29 9600's at **21 dB** — the lower rate is worse, though its minimum
distance is 4.9 dB larger, and T.30's ladder goes 9600 → 7200 entitled to assume
the step is downhill. `fax-qam.md` §6 Q3 names two suspects and this package must
bisect them: segment 2 alternates |A| = 3 with |B| = √2, a **6.5 dB swing every
symbol** into a Gardner detector that assumes constant modulus
(`shaping.rs:321-328` records the thrashing), and `MIN_DECISION_POWER = 0.5`
(`v29.rs:574`) clamps the inner points' 0.364 at 7200 but only 0.148 at 9600.
Both are front-end. Fix whichever the bisect names, and record the other.

**Tests:** `cargo test -p datapump --lib v29:: v27ter:: v17::` (14 + 15 + 8, none
edited); `cargo test -p datapump --release --lib -- --ignored
v27ter::tests::the_carrier_is_found_wherever_it_starts_every_way
v29::tests::the_carrier_is_found_wherever_it_starts_every_way` — both pass in
5.7 s today and must keep passing; `cargo test -p fax`; `cargo test -p datapump
--test dil_sounds`; new
`v29::tests::seven_thousand_two_hundred_is_not_worse_than_nine_thousand_six_hundred`.
**Expected:** **V.29 7200 SNR floor 24 dB → ≤ 21 dB** (it must be at least as
good as 9600's, not worse). **V.29 9600 clean slicer SNR 28.0 dB → ≥ 32 dB** and
**V.29 7200 clean 30.0 → ≥ 34 dB**, the interpolator's share. **V.29 7200 level
−6 dB BER 0.0228 → < 1e-3; V.29 9600 level +6 dB 0.0016 → < 1e-3 and −6 dB
0.0143 → < 1e-3.** V.27 ter's clean figures (50.4 and 49.3 dB) and its 15 dB
floors must not move; its level rows already pass and must stay passing.

---

### Wave 4 — carrier

---

#### `v22-carrier` — M
**Files:** `crates/datapump/src/v22bis.rs`
**Depends on:** `dsp-carrier-core`, `v22-front`

V.22bis starts at `phase = 0, frequency = 0` and pulls in from decisions
(`v22bis.rs:728-731`). Measured time to a clean eye with the exact difference
equations against a unit-power sixteen-point constellation: 2.5 s at +1 Hz,
8.9 s at +3 Hz, **41.5 s at +7 Hz**. V.22bis 2.6, rendered from Fascicle VIII.1
page 4, is not optional: "The receiver shall be able to operate with received
frequency offsets of up to ± 7 Hz."

Two changes:

**An open-loop estimate before the loop is allowed to run**, the way V.29 and
V.27 ter already do it. V.22bis hands the receiver a known two-phase signal to
measure on: 6.3.1.1.1 b) unscrambled binary 1 at 1200 bit/s is a pure tone —
every symbol the same quadrant change — so the turn per symbol *is* the offset,
with nothing to remove. The double dibit alternates two points a quarter turn
apart, so the fourth power removes the modulation there. Both arrive before any
decision is needed. Feed `Carrier::seed_hz`.

**Adopt `track::Carrier`** for the loop itself, at ζ ≈ 1 instead of 2.59.

`v22bis.rs:642-647` resets only `since_carrier` on a carrier edge and nothing
resets `phase` or `frequency`; measured drift over a 15 s gap of −55 dB noise is
0.027 Hz, so that is fine today and stays as it is — but the seed must re-run on
a carrier edge, not once per receiver.

**Tests:** `cargo test -p datapump --lib v22bis::`; `cargo test -p datapump
--test v22bis_loopback --test v22bis_handshake --test v22bis_vector --test
v22bis_capture`, unedited; `margins.rs::v22bis_2400_holds_seven_hertz_of_offset`
and `::v22bis_1200_holds_seven_hertz_of_offset` un-ignored; `cargo test -p
datapump --release --test lock_sweep -- --ignored`.
**Expected:** **V.22bis 2400 carrier −7 Hz 0/8 → 8/8 phases and BER 0.3540 →
< 1e-3; −3 Hz 0/8 → 8/8, BER 0.3133 → < 1e-3; −1 Hz 3/8 → 8/8; +1 Hz 5/8 → 8/8;
+3 Hz 0/8 → 8/8, BER 0.3258 → < 1e-3; +7 Hz 0/8 → 8/8, BER 0.3588 → < 1e-3.**
**V.22 1200 carrier ±7 Hz 0/8 → 8/8, BER 0.1093 and 0.1084 → < 1e-3; V.22bis 1200
±7 Hz BER 0.0609 and 0.0651 → < 1e-3.** Lock times at ±1 and ±3 Hz (65–274 ms)
must not get worse than the clean 175 ms by more than 50 %.

---

#### `v32-carrier` — L
**Files:** `crates/datapump/src/v32.rs`, `crates/datapump/src/v32/startup.rs`
**Depends on:** `dsp-carrier-core`, `dsp-carrier-drift`, `v32-front`

The worst carrier failure in the tree. With the exact equations of
`v32.rs:1018-1047`, the real constellations from `v32/trellis.rs` and each rate's
own `loop_bandwidth`: 9600 coded takes **85 s** at 7 Hz, 12000 takes **83 s at
3 Hz and never at 7**, and 14400 takes **51 s at one hertz** and never at three.
V.32 2.1, rendered: "The receiver must be able to operate with received frequency
offsets of up to ± 7 Hz." `before.md` shows it as **0/8 arrival phases at every
offset from −7 to +7 Hz** for both 12000T and 14400T.

Four changes.

**Seed from the reversal detectors.** V.32's start-up already runs three
`ReversalDetector`s on the carrier and both sidebands (`v32/startup.rs:994-996`)
and each one already computes the offset. Read `drift_hz()` and seed
`Receiver`'s `frequency` before data mode. This is ~10 lines in `startup.rs` and
it is the single change with the most `before.md` cells behind it.

**Raise the reversal detector's own ceiling** for V.32's three detectors, using
`with_carry` from `dsp-carrier-drift`: `f_max = π·bw/24 = 7.854 Hz` at bw = 60,
so a line at the ±7 Hz the clause *requires* sits at 89 % of the gate, and V.32's
entire start-up — including the round-trip measurement the echo canceller depends
on — is conducted through these reversals. V.34's detectors keep today's value.

**Track on the equalised symbol.** `coarse`, the decision the carrier loop is
driven by, is taken on the **unequalised** symbol (`v32.rs:1013-1025`). At four
points that is a quadrant test and robust; at sixteen it is a full slice on a
signal the equaliser has not cleaned, and one misdecision — deciding (3,1) as
(1,1) — injects **−57°**. `TRACK_SMOOTHING` turns one of those into 5.7° of
`track`, but a systematic bias from misslicing the outer ring drives the loop
away, and further away means more misslices. V.34 tracks on the equalised symbol
(`v34/receiver.rs:921-922`) and so must this.

**Fix `loop_bandwidth` for 9600 uncoded.** `coding_for(9600, Uncoded)` returns
`None` (`v32.rs:353-358`), so 2.4.1.1's sixteen points get `bw = 1.0`, the same
as 4800's four. Measured rotation margin ÷ bw: every trellis rate lands between
20.7 and 23.9; **9600 uncoded is the sole outlier at 16.9** — 2.7× hotter than
the rate the gain was tuned on. 7200 has *identical* point spacing and gets
0.7071. The comment justifying it (`v32.rs:274-275`, "both two units apart") is
contradicted by `point_spacing_at` in the same file (`v32.rs:369-373`,
`sqrt(20.0)` against `2.0`). And no test reaches it, because `agreed_coding`
returns `Trellis` above 4800 whenever both ends are V.32bis, which every
two-`Modem` test is.

While in `v32.rs:224`, note `MIN_DECISION_POWER = 5.0` — half the mean power.
The signal part of `raw` is already normalised (`raw = sin θ` whatever the
radius); what the clamp attenuates is signal and noise together, so it
down-weights the inner ring rather than protecting against it, costing 15 % of
loop gain at sixteen points and 10 % at thirty-two, and making the effective gain
depend on which symbols happen to arrive. V.34's floor is ten times gentler.
Bring it in line and measure.

**Tests:** `cargo test -p datapump --lib v32::`; the ten `v32_*` integration
files unedited; `margins.rs::v32_holds_seven_hertz_at_every_rate` un-ignored; new
`v32::tests::nine_thousand_six_hundred_uncoded_runs_at_its_own_loop_bandwidth`,
which is the first test in the tree to put a symbol through the 9600 uncoded
receiver; `cargo test -p datapump --release --test lock_sweep -- --ignored`.
**Expected:** **every carrier column from −7 to +7 Hz reaches 8/8 phases at every
V.32 rate.** Specifically: V.32 9600 −7 Hz 0/8 → 8/8 (BER 0.3404 → < 1e-3), ±1 Hz
4/8 → 8/8 (BER 0.4946 and 0.4998 → < 1e-3), +7 Hz 0/8 → 8/8 (BER 0.2617 →
< 1e-3); V.32bis 7200T ±7 and ±3 all 0/8 → 8/8; V.32 9600T ±3 and ±7 all 0/8 →
8/8; **V.32bis 12000T every offset 0/8 → 8/8; V.32bis 14400T every offset 0/8 →
8/8.** And the clock columns, which carry a proportional carrier offset with
them: **V.32bis 14400T ±200 ppm 0/8 → 8/8, BER 0.3853 and 0.3696 → < 1e-3;
V.32 9600T clock −120, −50, +50, +120 ppm all never → lock with BER < 1e-3;
V.32bis 12000T clock −50 ppm never → lock.**

---

#### `fax-carrier` — S
**Files:** `crates/datapump/src/v29.rs`, `crates/datapump/src/v27ter.rs`
**Depends on:** `dsp-carrier-core`, `fax-front`

These two already meet ±7 Hz and have tests for it — `before.md` shows both at
8/8 phases at every offset and every clock. Two narrow things, neither of which
is a `before.md` cell today.

**V.29 at 4800: the A-first/B-first tie.** `acquire` picks between the two
hypotheses by strict `>` (`v29.rs:910`), so a tie keeps `first = 0`. At 9600 and
7200 there is no tie, because `2(θ_A − θ_B) = ±270°` and the wrong hypothesis
cancels. At 4800, A is at four eighths and B at six — a quarter turn — so
`2(θ_A − θ_B) = 180°` both ways and the wrong hypothesis adds **coherently**,
giving exactly the same magnitude as the right one. When B in fact arrived first,
`frequency` is set to about −0.5 turns a symbol, **−1200 Hz**, and the burst is
gone. Which arrived "first" depends only on the parity of the symbol the detector
fired on: sweeping the start offset 0 to 19 samples lost the page at 2, 3, 4, 9,
10, 16 and 17 — seven of twenty. Use the fourth power, which does not tie, or
break the tie on the equaliser's residual after a few dozen symbols. `fax-qam.md`
§5.3 says why this matters beyond V.29 4800: at V.17 `|A| = |B|` and A and B are a
quarter turn apart at *every* rate, so this method as it stands can never be
copied there.

**V.27 ter at 2400 slices the carrier error over eight phases.** `nearest_eighth`
(`v27ter.rs:928`, `:982-985`) is used at both rates, and the comment at `:922-927`
justifies it with an argument that is wrong: every phase the 2400 end can put on
the line is one of four — `DIBIT_TURN` is [0, 2, 6, 4] (`:134`), segment 3's
reversal is `turn(4)` (`:445`), segment 4's is `turn(4)` or `turn(0)` (`:462`),
the unmodulated carrier is `turn(0)` (`:429`). Slicing over eight halves the
phase detector's linear range from ±45° to ±22.5° and doubles the number of
mis-slices, each throwing a 45° spike into a loop whose ordinary input is a few
degrees. Slice four at 2400. Fix the comment as well as the code.

Also: `restart()` resets neither `phase` nor `frequency` in either receiver
(`v27ter.rs:755-760`, `v29.rs:730-735`), so a burst too short to reach the
acquisition window inherits the last one's carrier.

**Tests:** `cargo test -p datapump --lib v29:: v27ter::`, unedited; the two
`#[ignore]`d `the_carrier_is_found_wherever_it_starts_every_way` sweeps; new
`v29::tests::four_thousand_eight_hundred_is_found_from_every_start_offset` —
the V.29 sweep hard-codes 9600 at `v29.rs:1290-1310`, which is why §3.3 is not
caught by it; new
`v27ter::tests::twenty_four_hundred_slices_the_four_phases_it_is_sent`;
`cargo test -p fax`.
**Expected:** no `before.md` cell moves — both modes are already 8/8 at every
carrier and clock column and must stay there. The figures this package is judged
on are the two new sweeps: **V.29 4800 loses the page at 7 of 20 start offsets →
0 of 20**, and **V.27 ter 2400's SNR floor 15 dB → ≤ 12 dB** from the recovered
phase-detector range (it already carries 2 pages of 8 at 12 dB where 4800 carries
none).

---

### Wave 5 — equalisation

---

#### `v22-eq` — L
**Files:** `crates/datapump/src/v22bis.rs`,
`crates/datapump/src/v22bis/handshake.rs`
**Depends on:** `dsp-eq-halfspaced`, `v22-carrier`

**T/2 and NLMS**, fed from `interp::Fractional` at the half-symbol instant as
well as the symbol instant. **Adapt in the equaliser's own frame** —
`v22bis.rs:735-740` equalises the derotated symbol and adapts on the derotated
decision, so the equaliser and the carrier loop chase the same degree of freedom.
**Seed from the known handshake segments**: 6.3.1.1.1's unscrambled binary 1 and
the double dibit are both known symbol for symbol, which is what `seed_from`
wants, and they arrive before any data.

**Retarget on every rate change.** `v22bis.rs:599` passes 1.32 once. At 1200 the
constellation is four constant-modulus points (`v22bis.rs:1018-1036`, the `01`
point of each quadrant, confirmed constant-modulus at `:1093-1100`) whose true R₂
is **1.0**. Against 1.32 the blind stage converges 15 % large, which decision
direction then pulls back at 4e-3 — about 125 symbols, 208 ms of wrong decisions,
on every fallback. `set_rate` (`:920-922`) and the automatic fallback at
`:847-848` both set `self.rate` and nothing else; both must call `retarget`.

**Latch the rate.** This is the one `before.md` cell that separates V.22 1200
from V.22bis 1200, which are the same waveform (V.22bis 2.5.2.2 nominates the one
point V.22 uses "irrespective of the quadrant concerned … This ensure
compatibility with Recommendation V.22") — the only difference is whether the
receiver was told. The rate is re-decided from the pre-equaliser power variance
every 128 symbols (`v22bis.rs:830-855`) and can overrule a negotiated rate 213 ms
later. On the tree's own ground-truth vector `tests/vectors/v22bis-2400.wav`, a
real call that runs at 1200, the answering direction read 2400 for **six
seconds** — about 3600 symbols of sixteen-way decisions on a four-point signal
feeding the equaliser — and **never recovered**: residual stuck at 0.194 against
0.031 in the other direction, within a factor of 1.6 of the decision boundary. It
escaped at all only because the carrier flag happened to drop at 5 s. The
reasoning behind measuring the radius rather than the index, before the equaliser
rather than after, and the variance rather than the closeness (`v22bis.rs:773-829`)
is sound and stays; what changes is that a negotiated rate latches, and an
unnegotiated one needs agreement across consecutive windows before it moves.

**Tests:** `cargo test -p datapump --lib v22bis::`; `cargo test -p datapump
--test v22bis_loopback --test v22bis_handshake --test v22bis_vector --test
v22bis_capture`, unedited — `the_receiver_works_out_which_rate_is_in_use:219` and
`twelve_hundred_bits_per_second_round_trips:209` in particular; new
`v22bis::tests::a_negotiated_rate_is_not_overruled_by_the_variance_test`; new
`v22bis::tests::the_ground_truth_vector_reads_twelve_hundred_in_both_directions`
asserting under 2.0 s in both directions and a residual under 0.05;
`margins.rs::each_rate_carries_data_at_the_signal_to_noise_ratio_it_needs`
un-ignored for the V.22 rows.
**Expected:** **V.22 1200 arrival phases 7/8 → 8/8** (V.22bis 1200 is 8/8 on the
identical waveform, so the missing phase is the rate decision, not the receiver)
and **V.22 1200 clock +120 ppm BER 0.4688 → < 1e-3** — the lone clock failure in
that row, with ±200 ppm passing on both sides of it, which is the same lottery.
**V.22bis 2400 SNR floor 12 dB → ≤ 9 dB** and **clean slicer SNR 28.6 dB →
≥ 33 dB**.

---

#### `v32-eq` — L
**Files:** `crates/datapump/src/v32.rs`, `crates/datapump/src/v32/startup.rs`
**Depends on:** `dsp-eq-halfspaced`, `v32-carrier`

The largest single package, and the one that fixes the worst fault in
`before.md`.

**T/2 and NLMS**, fed at the half-symbol instant from `interp::Fractional`, and
**adapting in its own frame** (`v32.rs:1049-1053` derotates first today). A
symbol-spaced equaliser cannot compensate for the sampling phase, and that is why
the receiver settles into one of exactly two states decided by nothing but the
arrival phase, with the bad state's residual stable at 0.225–0.245 just under the
0.25 handover. 21 taps at T becomes 31 at T/2.

**Seed from TRN.** V.32 5.2.3, rendered from page 9, defines segment 3
completely: scrambled binary ones at 4800 bit/s, scrambler initial state all
zeros, a binary one applied throughout, **differential quadrant encoding
disabled**, the first 256 states given bit by bit and the rest by Table 5,
duration 1280 to 8192 symbol intervals. Its closing sentence names the purpose:
"Segment 3 is intended for training the adaptive equalizer in the receiving modem
and the echo canceller in the transmitting modem." The receiver knows every
symbol of it in advance and uses **none** of it: `adapt` is only ever called with
the slicer's own output (`v32.rs:1074`), and `startup.rs` knows exactly when TRN
is arriving (`:1099`, `:1143-1161`, `SEGMENT_TRN = 1280` at `:815`) and uses that
knowledge only to turn adaptation on and off. `seed_from` over the first 304
symbols of TRN replaces the blind stage entirely.

**Stop adapting through segments 1 and 2.** 5.2.3 says segment 3 is the training
segment; `far_end_quiet()` opens the receiver as soon as the listener hears
anything, so the equaliser adapts right through S (A/B alternating) and S̄ (C/D).
`startup.rs:1122-1125` states the objection precisely — "a filter learned from a
periodic reference is one of the many that explain that period and almost
certainly not the one the line is" — and then applies it only to the echo
canceller. An LMS filter driven by a two-point signal repeating every two symbols
is under-determined in exactly that way.

**Retarget on every rate change.** `v32.rs:909` passes 1.0 once; `follow`
(`:949-959`) recomputes the loop bandwidth and leaves the equaliser alone. True
R₂ is 1.310 at 9600, 1.381 at 12000, 1.343 at 14400. At 14400 the 12.6 % shortfall
puts the outer ring 0.207 out of place against a half-spacing of 0.110. And note
what the start-up hands over (`startup.rs:1833-1848`): taps trained on **four
points**, and the constellation then changes underneath in one step, from 45° of
rotation margin to 5.1°.

**And use 5.2.2's free time reference.** "The transition from segment 1 to
segment 2 provides a well-defined event in the signal that may be used for
generating a time reference in the receiver." Permissive, and it is free timing
the receiver is declining.

**Tests:** `cargo test -p datapump --lib v32::`; all ten `v32_*` integration
files unedited; `margins.rs::every_v32_rate_acquires_from_every_arrival_phase`
un-ignored; new `v32::tests::the_equaliser_is_solved_from_trn_not_from_itself`
asserting ≥ 12 dB after 304 symbols; new
`v32::tests::a_rate_change_retargets_the_equaliser`; `cargo test -p datapump
--release --test lock_sweep -- --ignored`.
**Expected:** **the arrival-phase column, which is the headline number of this
whole plan: V.32 9600 6/8 → 8/8, V.32bis 7200T 7/8 → 8/8, V.32 9600T 3/8 → 8/8,
V.32bis 12000T 3/8 → 8/8, V.32bis 14400T 2/8 → 8/8.** And with it: **V.32bis
14400T SNR floor 24 dB → ≤ 21 dB, 12000T 21 → ≤ 18 dB, 9600T 18 → ≤ 15 dB,
V.32 9600 18 → ≤ 15 dB.** **V.32bis 14400T clean slicer SNR 30.7 dB → ≥ 34 dB**
and margin 0.22 → ≤ 0.15. The round-trip rows for 7200T/9600T/12000T/14400T,
all "never" with BER ≈ 0.50, must lock with BER < 1e-3 — `v32-front` fixed the
gate, but above 9600 the equaliser has to survive the arrival phase as well
before those cells move.

---

#### `fax-eq` — M
**Files:** `crates/datapump/src/v29.rs`, `crates/datapump/src/v27ter.rs`
**Depends on:** `dsp-eq-halfspaced`, `fax-carrier`

**T/2, NLMS, own-frame adaptation**, and **seed from the known segments** — V.29's
segment 2 alternates two known points for 128 symbol intervals and V.27 ter's
segments 1, 3 and 4 are all two-phase, so both have exactly what `seed_from`
wants, and both currently throw it away on a blind CMA stage that cannot hand
over sooner than `ln(1/0.25)/0.01` = **139 symbols** after the first `adapt`.
Earliest decision-directed symbol today: 195 for V.27 ter, 211 for V.29.

That floor is what makes V.27 ter's **short** turn-on fail: 80 symbols end to end
against a 115-symbol blind stage, so the page begins with an equaliser still
adapting blind. This end always sends the long one (`faxcall.rs:385-390`), but
Table 3/V.27 ter makes the short one the normal choice after the first
turn-around and the receiver has to take what a real far end sends. Measured on a
channel of signal plus one delayed copy, the short turn-on loses the whole page at
0.35 and 0.5 echo where the long one carries it. A seeded equaliser needs no
blind stage at all and the 58 symbols of conditioning become sufficient.

V.29's blind stage is also much more fragile than V.27 ter's — expected for
sixteen points on two radii, and made worse by a constant-modulus criterion
having to open a two-radius eye. `v29.rs:677-683` already recomputes R₂ per rate
and that is right; keep it and add `half_spacing` to it.

**Tests:** `cargo test -p datapump --lib v29:: v27ter:: v17::`, unedited;
`cargo test -p fax`; `cargo test -p datapump --test dil_sounds`; both `#[ignore]`d
carrier sweeps; new
`v27ter::tests::the_short_turn_on_carries_a_page_through_the_same_echo_as_the_long_one`
against the 0.35-at-0.44 ms and 0.5-at-0.44 ms channels the long one already
survives; new `v29::tests::the_equaliser_is_solved_from_segment_two`;
`margins.rs::v27ter_and_v29_carry_a_page_over_an_impaired_line` un-ignored.
**Expected:** **V.29 9600 SNR floor 21 dB → ≤ 18 dB; V.29 7200 24 dB → ≤ 18 dB**
(with `fax-front`'s bisect, 7200 must end up at least as good as 9600).
**V.27 ter 2400 and 4800 SNR floor 15 dB → ≤ 12 dB.** **V.29 9600 clean slicer
SNR 28.0 dB → ≥ 34 dB.** V.27 ter's clean 50.4/49.3 dB and its 8/8 phases must
not move. The echo rows are **not** this package's to claim — see the note under
`fax-loss`.

---

### Wave 6 — loss detection, hold and recovery

---

#### `v22-loss` — L
**Files:** `crates/datapump/src/v22bis.rs`,
`crates/datapump/src/v22bis/handshake.rs`
**Depends on:** `dsp-carrier-core`, `dsp-level-core`, `v22-eq`, `proof-harness`

V.22bis 6.4 is unambiguous, and read from the rendered page: a retrain "shall be
initiated either by detection of loss of equalization **or by detection of
unscrambled repetitive double dibit 00 and 11 at 1200 bit/s from the distant
modem**", and a modem that sent one and got none back "shall return to the
beginning of the retrain signal … and repeat the procedure until unscrambled
repetitive double dibit 00 and 11 is received from the remote modem". Grep for
`retrain` in `v22bis.rs` and `handshake.rs`: **zero hits**. Once connected,
`State::Connected(_) | State::Failed => {}` (`handshake.rs:283`) and `step` does
nothing at all, for ever.

What that costs is on the tree's own capture. Replaying
`captures/live-1788613347.wav`, a real 2400 call: at 21.89 s the far end sends a
double dibit — it is asking for a retrain, exactly as 6.4 tells it to — and sends
**nine** of them plus nineteen unscrambled-ones detections over the next fourteen
seconds. The receiver reports every one correctly as `Pattern::DoubleDibit`. The
handshake is not looking. Residual runs 0.277 → 0.462 → 0.519, the constellation
collapses to the origin, 4648 bytes of garbage come out, and the status line says
`Connected(Bps2400)` throughout.

Four things:

1. **`track::Convergence` on the data-mode symbol**, with `min_distance²` from the
   rate in force, giving the loss-of-equalisation signal 6.4 needs. The `doubtful`
   gate freezes the equaliser, the carrier loop and the timing loop on an
   improbable decision instead of learning from it.
2. **Retrain on either trigger**, and repeat until answered, per 6.4.
3. **6.5, operation after loss of line signal.** Today there is no clamp on
   recovered data (`v22bis.rs:878-882` pushes descrambled bits whatever the
   carrier state) and no 100 ms window in which a retrain would be looked for.
   Both are in the clause.
4. **Stop the equaliser adapting through a dropout.** Measured: one second of
   line noise 35 dB below the signal takes the residual from 0.049 to 0.248 and it
   does not recover in ten seconds; the boundary is `CARRIER_ON`/`CARRIER_OFF` to
   within a hair, because the AGC has already amplified the noise to mean power 10
   by the time the equaliser sees it. If the noise on a dropped line is loud enough
   to hold the flag — about 42 dB below the signal that was there — nothing stops
   it. `Presence`'s guard plus `Convergence`'s gate close this together.

**Tests:** `cargo test -p datapump --lib v22bis::`; the four `v22bis_*`
integration files unedited; `margins.rs::the_carrier_turns_on_between_40_and_205_ms`,
`::the_carrier_turns_off_between_40_and_65_ms`,
`::the_answer_tone_does_not_raise_circuit_109` and
`::the_carrier_holds_across_a_hundred_millisecond_dropout` un-ignored; new
`v22bis::tests::a_far_end_asking_for_a_retrain_is_answered` driven by the
`DoubleDibit` pattern; new
`v22bis::tests::a_second_of_noise_thirty_five_decibels_down_does_not_poison_the_taps`.
**Expected:** **V.22bis 2400 dropout 20 ms: `carrier lost` "no" → a
loss-of-equalisation event reported, with BER held at the hole's own cost
(0.0092 today, floor ≈ 0.010 for 12 symbols of a 2400-bit payload) and slips
counted rather than silent.** The 50 ms row (BER 0.0177, carrier held) must
likewise report. **V.22 1200 and V.22bis 1200 dropout 20 ms BER 0.0120 → ≤ 0.012
with the loss reported.** The three V.22bis 3.2/3.3 margin tests move from
failing (ON 22.8 ms against 40–205, OFF 148.2 ms against 40–65, answer tone
raising 109) to passing. No other V.22bis cell may move.

---

#### `v32-loss` — L
**Files:** `crates/datapump/src/v32.rs`, `crates/datapump/src/v32/startup.rs`
**Depends on:** `dsp-carrier-core`, `v32-eq`, `proof-harness`

`v32.rs:1031-1042` updates the carrier loop and `:1073-1081` the equaliser on
**every** symbol while connected, however improbable the decision was. No
confidence gate, no loss detector, no way back. On Rory's rig a ~20 ms
concealment insert feeds the loop about 48 symbols of fade and garbage; at 14400
the slow mode is **865 ms** and the margin **5.1°**, so recovery is a second of
errors at best and a walk-off at worst, and the V.42 layer sees a burst of
retransmissions with no indication from the pump that anything happened.

Three things, in order of how much they are worth:

1. **`track::Convergence`**, with `min_distance²` from the constellation in force
   — the one gate governing all three loops, the windowed error against `settled`,
   and a reported loss signal.
2. **Rewind.** V.32 is the only slow mode that runs long enough for it to pay
   (`shared-dsp.md` §4.5), and it is the answer to the thing `Convergence` alone
   cannot fix: by the time the error average has risen far enough to declare a
   loss, every loop has already spent the window learning from wrong decisions.
   Copy the shape, not shared code: a snapshot of taps, rotation, frequency, timing
   phase and `settled` every 16 symbols, 24 kept — 384 symbols, 160 ms at 2400 baud
   (`v34/receiver.rs:112-113`, `:997-1013`). Note V.34's own §7.5: `rewind` returns
   silently doing nothing when no snapshot is old enough, so count the no-ops from
   the start rather than discovering them on a live call.
3. **Clause 8 — rate renegotiation — at least well enough not to be destroyed by
   one.** V.32bis 8.2: "A modem shall be conditioned to detect an incoming preamble
   at any time while receiving data." The preamble is **AA for 56T then CC for 8T**
   from the calling modem, AC/CA from the answering one; `State::Connected`
   (`startup.rs:1863-1867`) requires the opening tone to hold for **more than
   128** symbols, so a 64-symbol preamble is invisible. What happens instead is
   worse than nothing: the far end changes rate after its E, this receiver does
   not, `residual_error` climbs, and after 1.0 s `stop_offering(running_at, …)`
   fires and **permanently drops the rate the call was running at and everything
   above it**, then does a full 10–15 s retrain. A far end that renegotiated
   *upward* has just cost this modem the rate it had. Real far ends do this —
   memory: `live-v34-far-end` "renegotiates, retrains if unanswered";
   `live-v34-isp-far-end` "renegotiates often". Detecting the preamble and
   declining to `stop_offering` on a renegotiation is the minimum; implementing
   R4/R5 is a follow-on.

Two more, cheap, while the file is open. The echo canceller freezes at the end of
the first TRN and is only released by a full retrain (`startup.rs:2230-2240`); a
20 ms slip in the return path moves the echo clean outside both the 8 ms near and
4 ms far windows, and the canceller then adds a second uncorrelated copy of our
own signal, which is 8–12 dB above the far end's. And `v32.rs:1073` gates the
equaliser — and therefore `error_average`, and therefore the whole unsatisfactory-
reception retrain — on `self.carrier`, so a far end that goes away quietly leaves
the pump in `Connected` reporting a healthy residual for ever.

**Tests:** `cargo test -p datapump --lib v32::`; all ten `v32_*` integration files
unedited; new `v32::tests::a_twenty_millisecond_hole_is_reported_and_rewound`; new
`v32::tests::a_renegotiation_preamble_does_not_cost_the_rate`; new
`v32::tests::the_residual_does_not_freeze_when_the_far_end_goes_quiet`;
`cargo test -p datapump --release --test lock_sweep -- --ignored`.
**Expected:** **dropout 20 ms at every V.32 rate: `carrier lost` "no" → loss
reported, with BER held at the hole's cost** (0.0082 at 4800, 0.0075 at 9600,
0.0070 at 7200T, 0.0072 at 9600T, 0.0081 at 12000T, **0.0125 at 14400T → ≤ 0.009**
— 14400T is the one rate whose dropout cost is above the hole's own floor, which
is the walk-off). The 50 ms row (BER 0.0161 at 4800) likewise. **With the
canceller in the harness (`proof-harness`), echo 0.57 @ 120 ms: V.32 9600 BER
0.5008 → < 1e-3, 7200T 0.4969 → < 1e-3, 9600T 0.5011 → < 1e-3, 12000T 0.4964 →
< 1e-3, 14400T 0.5001 → < 1e-3, and V.32 4800 0.0112 → < 1e-3.**

---

#### `fax-loss` — M
**Files:** `crates/datapump/src/v29.rs`, `crates/datapump/src/v27ter.rs`
**Depends on:** `dsp-level-core`, `fax-eq`, `proof-harness`

The fax modes' dropout failure is different in kind from V.22bis's and V.32's,
and this is the package that names the difference. At 20 ms **V.29 goes to BER
0.224 and V.27 ter never locks at all (BER 0.487)**, against V.22bis's and V.32's
0.008, because the fax detectors *correctly* drop the carrier — and dropping it
is what ends the burst, since a half-duplex receiver has one training sequence in
front of it and no way back to it.

Two mechanisms, and neither is `rewind`, because there is nothing to rewind to.

**Do not end the burst on a hole.** V.29 5.2.2 asks for **30 ± 9 ms** from the
end of a burst to circuit 109 going OFF and V.27 ter 3.6 (via Tables 7 and 8) for
**30 to 50 ms**; measured is **17.6 ms** (10·ln 4 = 13.9 ms for the level to fall
12 dB, plus 3.75 ms through the select filter). A 20 ms hole is inside every one
of those windows and must not drop the flag. `Presence` with V.29's and V.27 ter's
own numbers does it, and it also fixes the separate complaint that both receivers
stop emitting bits the instant the flag drops (`v27ter.rs:951-953`,
`v29.rs:989-991`) while 12.5 to 21.3 ms of data is still inside the select filter,
the matched filter and the equaliser.

**Make acquisition re-armable and checked.** Today it is one shot: once `acquire`
has run nothing notices it was wrong and nothing runs it again;
`Equalizer::error()` is computed and reported but never consulted; `burst_symbols`
only restarts on a detector edge. So V.29 fails all-or-nothing — at 40 dB SNR, 4
of 32 bursts lost the page and **all four lost the whole page**, never a fragment.
The same one-shot design is what makes `fax-qam.md` §3.1 a page and not a line:
the on-threshold is `max(4·floor, 1.0e-3)` with `floor` starting at **zero**
(`v27ter.rs:681`, `v29.rs:641`), so a fresh receiver has only the fixed 1.0e-3,
which is 53 dB below a burst; any line whose hiss is above that trips the
detector, the trip calls `new_burst`, the acquisition window is spent on noise,
and the detector then **cannot fall again**, because the off-threshold is a
quarter of `loudest` and `loudest` is by then the noise itself. The floor cannot
rescue it because the floor only adapts while the detector says there is no
carrier. Measured end to end: a fresh V.29 receiver fired its carrier before the
burst in **32 of 32** runs and lost the page in 32 of 32, at every SNR from 50 dB
down to 30 dB; a whole fax call over a line with nothing but added hiss drops from
9600 to 7200 at exactly σ = 2.6e-3, the point where 0.39σ crosses 1.0e-3.

`fax-qam.md` §6 Q2 asks which fix to take. Take the re-arming one: re-run
`acquire` whenever the level rises 12 dB above whatever it had settled at, and
check the result against the equaliser's residual over the next few dozen symbols.
It is more work and it fixes §3.2 as well, and §3.2 is what the dropout row is.

Also here, cheaply: V.27 ter 5.2.1 says circuit 109 "is prevented from turning ON
during reception of unmodulated carrier when the optional protection against
talker echo is used", and there is no such interlock — with echo protection on,
the flag goes on at 6.9 ms, off at 214 ms and on again at 229 ms, a phantom burst
in front of the real one. `v27ter.rs:325-328` records that a real public fax
service sends this before every training check.

**Tests:** `cargo test -p datapump --lib v29:: v27ter:: v17::`, unedited; `cargo
test -p fax` including `faxcall.rs:690-705`, which passes today only because it
asserts the page arrives and not what rate it arrived at; `cargo test -p datapump
--test dil_sounds`; new
`v29::tests::a_quarter_second_of_line_before_the_burst_does_not_cost_the_page`
(32 of 32 lost today); new
`v27ter::tests::a_twenty_millisecond_hole_does_not_end_the_burst`; new
`v29::tests::a_call_over_hiss_still_settles_at_nine_thousand_six_hundred` at
σ = 2.6e-3 and above; `margins.rs::v27ter_and_v29_carry_a_page_over_an_impaired_line`
fully un-ignored.
**Expected:** **V.29 7200 dropout 20 ms BER 0.2241 → ≤ 0.015 with the carrier
held; V.29 9600 0.2238 → ≤ 0.015; V.27 ter 2400 "never locks", BER 0.4873 → lock
and BER ≤ 0.02; V.27 ter 4800 0.4918 → ≤ 0.02.** (The floors are the hole's own
cost: 48 symbols of 2400 baud is 192 bits of a 7680-bit V.29 payload, half of them
wrong by chance ≈ 0.0125; 24 symbols of 1200 baud is 48 bits of 1920 ≈ 0.0125.)
And from the hole-length table: **V.27 ter 4800 at 50 ms BER 0.4769 → ≤ 0.04,
carrier held; V.29 9600 at 50 ms 0.2176 → ≤ 0.035, carrier held.**

**What this package must *not* claim.** The fax echo rows —
V.29 7200 BER 0.4912, V.29 9600 0.4970, V.27 ter 2400 0.4867, V.27 ter 4800
0.4983 — are a **listener** echo, because these modes are half duplex and silent
while receiving, so the only echo there can be is the far signal off the far
hybrid: a reflection 4.9 dB down at **144 to 288 symbol periods**. A 31-tap T/2
equaliser at 2400 baud reaches ±15 symbols = ±6.25 ms; the echo is at 120 ms.
No equaliser in this plan spans it and none is proposed. The row is brutal by
construction and stays where it is, and that is written into the package rather
than discovered later.

---

### Wave 7 — close

---

#### `proof-close` — M
**Files:** `crates/datapump/tests/lock_sweep.rs`,
`crates/datapump/tests/margins.rs`,
`docs/design/slow-modes/after.md` *(new)*
**Depends on:** every package above

Un-ignore what is left in `margins.rs`, re-run the sweep, write `after.md` beside
`before.md` in the same shape so the two can be read column against column, and
record every target that was **not** met with the measurement that says so. A
target missed and written down is worth more than a target quietly dropped.

Then add the four columns `before.md` could not have: the loss signal, the
canceller-in and canceller-out echo pair for V.32, the slip count, and the
`drift_ppm` figure the data-aided timing detector gives (`v34/receiver.rs:628`),
which no slow mode has today.

**Tests:** the whole suite — `cargo test` across the workspace, `cargo test -p
datapump --release --test lock_sweep -- --ignored --nocapture`, `cargo clippy
--workspace --all-targets`. The V.34 count must still read 107, V.90 80, V.27 ter
15, V.29 14, V.17 8, with no test file edited.
**Expected:** `after.md` carries every figure named above, and the summary table
"Where it stands, worst first" goes from **15, 13, 13, 12, 8, 6, 4, 4, 3, 3, 3,
2, 2, 1, 1 failures** to **0 for every mode except the echo rows the half-duplex
modes cannot reach** — four cells, named in `fax-loss`, expected to stay.

---

## 6. The waves

| wave | packages | what lands |
|---|---|---|
| 1 | `dsp-timing-gardner`, `dsp-timing-interp`, `dsp-eq-target`, `dsp-carrier-drift`, `dsp-level-core` | the cheap shared primitives; nothing in `crates/datapump` moves and no `before.md` cell changes |
| 2 | `dsp-carrier-core`, `dsp-eq-halfspaced`, `fsk-hold`, `proof-harness` | the structural shared primitives, the whole FSK side, and the measuring stick |
| 3 | `v22-front`, `v32-front`, `fax-front` | sampling instant and level: the interpolator ceiling, the AGC transient, V.32's carrier-edge gate |
| 4 | `v22-carrier`, `v32-carrier`, `fax-carrier` | ±7 Hz, which is a clause in four Recommendations and met in two |
| 5 | `v22-eq`, `v32-eq`, `fax-eq` | T/2 and the least-squares seed: the arrival-phase lottery |
| 6 | `v22-loss`, `v32-loss`, `fax-loss` | the four "none" cells in §2's last column |
| 7 | `proof-close` | `after.md` |

Waves 1 and 2 build capability and move nothing measurable, which is deliberate:
`evidence.md` §5 Q1 asks whether V.32's arrival-phase fault is the modulus, the
handover threshold or the symbol spacing, and says it "should be settled by
running §4.5's arrival sweep after each". Wave 3 runs the sweep with
`dsp-eq-target`'s two-line fix adopted and wave 5 runs it with T/2 adopted, so the
question gets answered rather than assumed.

The order within waves 3 → 6 is not arbitrary either. It is the order of the
signal path: you cannot judge a carrier loop through a timing loop that is
sampling in the wrong place, you cannot judge an equaliser through a carrier loop
that is still pulling in, and you cannot judge a loss detector until the thing it
is detecting the loss of works.

---

## 7. Rules every package obeys

1. **Specs and real captures only.** No implementation is consulted, ported from,
   built or run. Every mechanism moved into `crates/dsp` comes out of
   `crates/datapump/src/v34/`, with the source line cited in the module comment.
2. **Every constant cites a clause or a measurement**, in the code, at the
   constant. A clause number is read from the **rendered PDF page**, never from
   `docs/specs/text`, which loses signs and columns — `fax-qam.md` §5.1 has a case
   where the text layer carries digits the page does not draw. A measurement cites
   the row of `before.md` or the test that produced it.
3. **No default moves.** Every addition to `crates/dsp` is opt-in through a new
   constructor or setter, and a test pins the old path.
4. **No existing test is edited.** A package that cannot pass an existing test has
   found something and must say what, not change the test.
5. **A package that does not move its figure has not landed.** Run the sweep,
   compare against `before.md`, and if the cell did not move by more than the seed
   spread, say so in the package's own notes before moving on.
6. **`crates/datapump/src/v34/**` and `v90/**` are not touched by any package
   here.**
7. **No audio device is opened** by anything in this work.

---

## 8. Risks

1. **The T/2 change is four `feed` loops, not a rename.** Each caller has to hand
   the equaliser two samples per symbol instead of one, which means rewriting the
   interpolate-and-tick block at `v22bis.rs:648-669` and its three copies; the
   output has to be sampled at the symbol rate while it is fed at twice it; and
   NLMS changes what "step" means, so every mode's tuning has to be redone. A
   package that lands the filter and not the tuning makes its mode **worse**.
   Mitigation: `dsp-eq-halfspaced` ships the T/2 path behind a constructor, each
   mode's own package flips it, and the sweep runs before and after that one flip
   with nothing else changing.
2. **`dsp::Equalizer` is shared by four modes**, so a wave-1 or wave-2 change to it
   moves numbers for all four at once, including the two whose existing tests are a
   hard gate. Mitigation: rule 3, plus
   `equalizer::tests::the_old_constructor_has_not_moved` comparing taps against a
   stored vector.
3. **`tone.rs` is shared with V.34 phase 2** (`v34/phase2.rs:148`, `:338`), and
   raising `MAX_CARRY` globally would move a detector 107 V.34 tests depend on —
   and, per `v34-phase2-tone-deadline` in memory, one whose DIL slip relocation
   already fails 17 of 260 sweep positions. `dsp-carrier-drift` therefore
   parameterises rather than changes the default, and `v32-carrier` opts in only
   V.32's three detectors.
4. **Every number in this plan is simulated.** There is no capture of V.22bis, V.32
   or high-speed fax carrier in `dist/captures` that any of this is measured
   against — two replay harnesses (`v22bis_capture.rs`, `v32_replay.rs`) name files
   that are no longer in the tree. A package can pass the whole sweep and still
   fail on Rory's rig. The live-testing loop is his (memory:
   `dialupmodem2-test-loop`), and `v22-loss` in particular is arguing from a
   capture whose file may need recovering before the test can be written.
5. **`v32_call.rs` passing is not evidence.** `evidence.md` §3.1 explains why: the
   start-up converges the equaliser on the four points of TRN first, where modulus
   1.0 is correct and the eye is wide, then keeps the taps across the rate change.
   The call tests pass **because the start-up hides the fault**, and they would stop
   hiding it the moment anything caused an equaliser reset at the higher rate.
   Never read a green `v32_call` as a moved cell.
6. **The FSK carrier-hold change pulls in two directions.** V.21 Table 2's 20–80 ms
   ON→OFF floor is what stops a 20 ms concealment insert costing a frame, and it is
   also 20 ms longer that a genuinely dead line keeps producing bits. `fsk.rs:117-134`
   records that the previous detector was thrown out for reporting CONNECT to a far
   end that had said nothing; do not walk back into that.
7. **Seeding V.32's carrier from `ReversalDetector::drift` measures the tone, not
   the data carrier.** V.32's reversals are at 1800 Hz and the 600/3000 Hz
   sidebands, and the drift is an exponential average of a noisy phasor over about
   half a second. It is a *seed*, good to a hertz or so; the loop still has to
   close. If the seed is systematically wrong the loop will be worse than
   unseeded, so `v32-carrier` must assert the seed's accuracy against a known
   offset before trusting it.
8. **The arrival-phase lottery can make a package look better than it is.** Above
   9600, moving from 3/8 to 5/8 phases changes several single-phase cells without
   fixing anything structural. Judge on the eight-phase count.
9. **VoIP round trip is about 1.5 s each way** (memory: `voip-line-round-trip`) and
   `EchoFinder` is O(round-trip × training) with no cap — 654 M MACs inside the
   0.85 s `search_for` spans, which will not run. Nothing in this plan touches
   `echo.rs`, so `proof-harness` putting the canceller into the V.32 rows must use
   an ordinary line's round trip, and the VoIP case stays a known hole.
10. **`v17.rs` is out of scope and still blocked.** 267 lines of tables with no
    receiver and no importer anywhere in the tree. The blocker is real:
    `State::label_at` returns `None` for everything above 7200, and the 9600
    labels cannot be read off Figure 4/V.17 — four quarter-turn orbits fit the
    four circled letters about equally well (total distances 8.29, 8.02, 8.04,
    7.78) and all four pass the structural checks. It needs a **capture**, not
    another reading of the page. Do not let a package drift into it.

---

## 9. What this plan does not do

- It does not touch V.34, V.90 or `crates/ec`, `crates/modem`, `crates/fax`
  above the pump.
- It does not calibrate the carrier thresholds to the −43/−48 dBm that V.22bis
  3.3, V.21 8.3 and V.17 3.6 all specify. Nothing in the tree maps full scale to
  dBm, and inventing the mapping is a separate piece of work with a separate
  measurement behind it. `dsp-level-core` writes the gap down; it does not close
  it.
- It does not implement V.32bis clause 8's R4/R5 exchange, only the preamble
  detection that stops a renegotiation destroying the call.
- It does not add the fixed compromise equaliser that V.22bis 2.3 says "shall be
  incorporated in the modem transmitter", or the 1800/550 Hz guard tones of 2.1.
  Both are transmit-side and this is a receiver plan.
- It does not write a V.17 receiver.
- It does not switch V.34 onto the shared primitives. That is the obvious
  follow-on, and it should be judged against V.34's own tests and a live call,
  not against `before.md`.
