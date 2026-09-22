# V.32 / V.32bis receiver rebuild — the design

The design the implementation agents build. It rests on the three research
documents in this folder, which it cites rather than repeats:
[`contract.md`](contract.md) (what callers rely on, and the experiments A–H),
[`core.md`](core.md) (the V.34 receiver, its constants and defects, probes
E1–E6), and [`spec.md`](spec.md) (V.32 and V.32bis from the rendered pages).
Tree at `d7b914b` (Version 1.2.1). Everything marked **[P1]–[P5]** was measured
for this document on 2026-09-22 (section 1); the probes live outside the tree.

"V.32 §x" is V.32 (03/93), "bis §x" is V.32bis (02/91), with page numbers as
spec.md gives them. T is one symbol, 1/2400 s. Unit power means the
constellation in use has mean power 1.

---

## 0. The design in one page

1. **A new shared QAM core, `dsp::qam`**, copied from `v34/receiver.rs` with
   its four measured defects fixed, used by V.32 only. `v34/` and `v90/` are
   not touched. Every option that differs from V.34 is a switch that defaults
   to V.34's behaviour, so moving V.34 onto the core later is mechanical
   (core.md §8).
2. **The receiver is V.34's**: a fixed mixer, 1.02 s of stored history, one
   64×256 interpolating low-pass, a 31-tap T/2 equaliser, and three loops 4×
   apart. Carrier 50 symbols (30.5 Hz), timing 200 symbols (7.5 Hz), NLMS
   about 870 symbols. To these it adds a **decision-free AGC**: 32, 128 or 256
   symbols for 4, 16 and ≥ 32 points. There is **one relative gate**, and loss
   declared from errors *or* from the gate refusing most symbols.
3. **Acquisition happens where the Recommendation puts it.** The frequency
   comes from S, the time reference from the S→S̄ reversal, and the taps,
   gain, absolute phase and turn from a least-squares solve on TRN as a known
   sequence (Table 5, with our transmitter fixed to match it). Taps are kept
   through R, E and B1 into the data constellation. The dense-constellation
   loops only ever track.
4. **Tentative decisions at trellis rates come from the Viterbi decoder's best
   path at zero delay [P1].** At each rate's working SNR, that makes 10–17×
   fewer wrong decisions than slicing the whole constellation, and 2× fewer
   than the state-based Y0 half-constellation slice, with no delay in any
   loop.
5. **Holding on**: rewind, then resync on stored samples. The resync window
   ends 4 symbols short, which closes the look-ahead hole, and every resync
   fits the gain. A one-sample slip costs tens of symbols of data. A 20 ms
   concealment slip costs only data (48 symbols, 36 carrier turns exactly). A
   retrain is asked for only after about 1 s of continuous loss, and a loss
   never takes a rate out of the offer.
6. **The echo canceller follows drift in data mode [P3].** A second-order
   fractional-delay tracker (8 Hz for 0.5 s, then 1.5 Hz) retimes the taps. An
   integer jump search restores it after a slip or underrun, and a relative
   gate holds it. NLMS stays frozen after training, as today. Frozen, the
   canceller reaches +3 dB of residual echo on a 20 ppm cable. With the
   tracker it holds −28 to −35 dB at 0–100 ppm.
7. **Outside the receiver** this job fixes the TRN dibit mapping, the 4-point
   slicer rotation and the 12 000/14 400 level (all in `v32::Transmitter`, not
   in the shared `trellis` tables), plus the wrong comments.
8. **Six packages.**
   - Wave 1, in parallel: **A** core, **C** transmitter conformance, **E**
     echo drift, **F** acceptance harness.
   - Wave 2: **D**, the V.32 receiver on the core, wired into the start-up.
   - Wave 3: **G**, verification, replays and live readiness.
   - No existing test is planned to change (section 8).

---

## 1. What was measured for this design

The probes were built in the session scratchpad as a scratch crate with path
dependencies on `datapump` and `dsp`, using the public API only, at 16 kHz in
release. They are not in the tree. Each is described well enough to rebuild.

**[P1] Tentative decisions and working SNR.** Trellis encoder into AWGN at
Es/N0. The columns:

- *whole*: nearest point of the whole constellation.
- *half*: nearest point in the Y0 half named by the best current state
  (spec.md §2.5).
- *D=0*: the decision the best survivor makes for the symbol just received.
- *D=2*: that survivor traced back 2 symbols.
- *BER*: `trellis::Decoder` at depth 24, before descrambling (multiply by
  about 3 after).

The SER columns are all measured at the working SNR in the first data column.

| rate | Es/N0 for BER 1e-5 (ideal) | SER whole | SER half | SER D=0 | SER D=2 |
|---|---|---|---|---|---|
| 7200T | 13.5 dB | 0.05 | 0.008 | 0.004 | 0.001 |
| 9600T | 17.0 dB | 0.040 | 0.005 | 0.003 | 0.000 |
| 12000T | 19.7 dB | 0.06 | 0.010 | 0.006 | 0.002 |
| 14400T | 23.5 dB | 0.035 | 0.004 | 0.002 | 0.001 |
| 4800 (uncoded) | 12.5 dB | — | — | — | — |
| 9600U (uncoded) | 19.5 dB | — | — | — | — |

*Decides §2.6: loops take the D=0 decision.*

**[P2] Decision-free power estimate.** var(|p|²) at unit power:

- 0 at 4 points;
- 0.32 at 16 and at 7200T;
- 0.31 at 9600T;
- 0.38 at 12000T;
- 0.34 at 14400T.

A one-pole power estimate with a time constant of 256 symbols has an rms gain
error of 0.055 dB at 14 400. The gate's gain tolerance there is 0.64 dB
(core.md §7.2), about 12× more. At 128 symbols the error is 0.078 dB.

The fourth-power line |E s⁴|/E|s|⁴ is:

- 1.0 at 4 points;
- 0.52 at 16;
- 0.15 at 9600T;
- 0.45 at 12000T;
- 0.135 at 14400T.

*Decides §2.4 and the blind frequency estimate in §3.8.*

**[P3] The canceller on a drifting cable.** Two `v32::Transmitter`s at 14 400
summed onto one cable at 0.45, returned after 700 samples and resampled by
the out→in clock ratio. The canceller trains 2 s with the far end silent, then
the far end starts. The figure is residual echo against the far end's power,
over 1 s windows.

| canceller in data mode | 0 ppm | 5 ppm | 20 ppm | 100 ppm |
|---|---|---|---|---|
| frozen (today) | −42 dB | −13 dB at 6 s, **+1.5 dB** at 24 s | −2 at 6 s, **+4** | **+3.6** |
| NLMS μ 0.002 | −24 | −18 | −10 | −2 |
| NLMS μ 0.01 / 0.05 | −17 / −11 | −17 / −11 | −16 / −11 | −9 / −10 |
| delay tracker 8 Hz | −24 | −24 | −24 | −24 |
| delay tracker 2 Hz | −33 | −33 | −33 | −30 (−16 in the first second) |
| **tracker 8 Hz for 0.5 s, then 1.5 Hz** | **−29 → −35** | **−29 → −35** | **−29 → −34** | **−28 → −31** |

One sample lost on the line at 12.5 s (50 ms windows after it):

- the tracker alone takes about 450 ms to get back under −20 dB;
- an integer jump search over a 30 ms window is back to −23 dB within about
  100 ms (the probe let the tracker wander while it waited for a full
  post-slip window, which the design does not), and the fractional part then
  settles with the tracker;
- a naive fractional search on the same window is noise-limited to −18 dB and
  false-triggers constantly.

*Decides §5.*

**[P4] Echo of our own signal on Rory's VoIP line.** Channel 1 (sent) was
cross-correlated with channel 0 (heard) over lags 0–2 s on
`captures/live-1788855280.wav`, `live-1788841427.wav` and
`live-1788855465.wav`. No lag stands out: the largest peak is 6× the median,
which is the noise floor. Any echo is below −37 to −46 dB relative to what
was heard. *Decides §5: the full-strength echo that has to be followed is the
cable's, and on VoIP the canceller has nothing to follow through 20 ms slips.*

**[P5] Whole two-`Modem` calls on today's code**, direct line, 45 s, offering
4800–14 400. Es/N0 as defined in §9.1.

| line | today |
|---|---|
| clean | 14 400, 0 retrains, reception 0.113 |
| Es/N0 27 dB | 14 400, **retrain, ends at 12 000** |
| Es/N0 24 dB | **retrain, 9600** |
| +7 Hz / −7 Hz | 14 400 held / **3 retrains, down to 4800, still retraining** |
| ±200 ppm | 14 400 held |
| 20 ms insert (repeat) and drop, alternating, every ~4 s | 14 400 held (48-symbol, 36-turn slips are invisible to the loops) |
| one sample dropped every ~5 s | **3 retrains, 9600, still retraining** |
| ±3 dB gain step | 14 400 held |
| 27 dB + 100 ppm + 20 ms slips | **retrain, 12 000** |
| 4800 only, 15.5 dB / slips and drops | 4800 held / held |

**Other figures used below:**

- `v32_call::a_rate_that_cannot_be_read_is_given_up` today: up at 14 400,
  ends at 9600 after 1 retrain, at 0.139 of the gap.
- `a_call_completes_over_a_hybrid`: echo return loss 30.2 and 31.8 dB.
- lock_sweep today:

| lock_sweep V.32 row | 4800 | 9600U | 7200T | 9600T | 12000T | 14400T |
|---|---|---|---|---|---|---|
| `one_mode_clean`: phases locked | 8/8 | 6/8 | 7/8 | 3/8 | 3/8 | 2/8 |
| clean-line slicer SNR | 37.2 | 29.3 | 30.7 | 31.4 | 29.2 | 31.5 dB |
| offset +1 / +3 / ±7 Hz | — | 4/3/0 | — | 7/0/0 | 0/0/0 | 0/0/0 (of 8) |
| silence 0 / 300 / 1100 ms before the carrier | 8/0/5 | 6/7/6 | — | — | — | 2/0/0 |

- Contract experiments F and H (cable faults, direct-line faults) are the
  before figures for slips and drift.

`lock_sweep.rs` is **tracked**, having been committed in `ea1c3fc`, and was
not untracked as the brief said. Its V.32 rows run as contract.md §6.3 says.

---

## 2. The receiver

### 2.1 Signal path

The front end is copied from `v34/receiver.rs` (core.md §1).

| # | stage | V.32 configuration |
|---|---|---|
| 1 | Fixed mixer | 1800 Hz (V.32 §2.1), `2·x·e^{−j2πf t}`, never corrected. The carrier offset is taken out after the equaliser. |
| 2 | History | 16 384 mixed samples (1.02 s). |
| 3 | Interpolating low-pass | 64 taps × 256 phases, Kaiser β = 8. The cutoff is **1620 Hz**, V.34's `0.5·baud·1.1 + 300`. The −3 dB this leaves at V.32's 25 % band edge is re-flattened by the equaliser, and the 2f_c image edge is ≥ 39 dB down (core.md §1). The cutoff is a `Band` field, not a formula, so V.34 keeps its own. |
| 4 | Half-symbol sampling | `due += half·(1+drift)`, half = 3.333 samples. 4096 halves (0.85 s) kept, each with its time. |
| 5 | Equaliser | 31 complex taps at T/2 (`REACH` 15), 15.5 symbols = 6.46 ms. Taps come from training, or from the blind start (§3.8). No blind equaliser stage. |
| 6 | Gain | **new**: `z = g·(w·x)·e^{−jθ}`, with `g` from the AGC of §2.4. |
| 7 | Derotation | θ = `rotation`, advanced by `turn` every symbol, including when the gate is shut (a flywheel). |
| 8 | Decision | **supplied by the V.32 driver** (§2.6); the core also slices to the nearest point for loss, resync and SNR. |
| 9 | Gate and loops | §2.3 and §2.5. |

The core is pull-style so that the driver can supply the decision. `feed()`
stores samples and makes halves. `next()` returns the next equalised,
derotated point when its samples are in. `settle(target)` then applies loss
detection, the gate, the loops, snapshots and any resync for that symbol. The
core knows nothing about trellis codes.

### 2.2 The three loops and the AGC

At 2400 baud, per symbol:

| loop | law | gains | response | in Hz | clamp |
|---|---|---|---|---|---|
| carrier | 2nd order; `rotation += turn` every symbol, then `turn += Kf·ε`, `rotation += Kp·ε` when the gate is open | Kp 0.04, Kf 4e-4 | double root 0.98, time constant 50 symbols (20.8 ms) | B_L = 30.5 Hz | \|turn\| ≤ 20 Hz (new: 2π·20/2400 rad/symbol) |
| timing | data-aided (`Re(e·conj(rate))/slope`), as V.34 `:959-967` | 0.01, 1.25e-5 | double root 0.995, 200 symbols (83 ms) | B_L = 7.5 Hz | ±0.5 half per symbol; drift ±1000 ppm (V.32 needs ±200: 2.3 × 2 ends) |
| equaliser | NLMS in its own frame (error rotated back by `conj(spin)`) | step 0.02 | about 870 symbols (0.36 s) | — | — |
| **AGC (new)** | 1st order on `|z|²` towards 1 (the table's mean power); **no decisions** | α = 1/32, 1/128, 1/256 for 4, 16, ≥ 32 points | 13 / 53 / 107 ms | — | g within ±20 dB of its trained value; frozen while lost |

**The ratios decide stability, not the numbers.** Carrier 50 : timing 200 :
NLMS 870 is copied from V.34 (core.md §3.4). The AGC shares one degree of
freedom (gain) with the NLMS and is at least 3.4× faster, so it takes the
gain before the taps can.

The phase detector for table slicers is **`ε = Im(z·t̄)`, unnormalised**. The
loop gain is then the constellation's mean power, 1, so inner points are not
over-weighted. V.34's `/max(|t|², 0.1)` gave inner points of the 128-cross a
detector gain of 0.24 (core.md §9, 7.9); V.34's `Grid` slicer keeps its own
form.

- **Phase jitter at 14 400**: B_L·T = 0.0127 at 27 dB gives under 0.4° rms.
  That moves the outer point by at most 0.01 against σ = 0.045.
- **±7 Hz** is 1.05° a symbol. A type-2 loop holds it with no standing error
  once `turn` starts near it (§3.3).

### 2.3 The single gate

A symbol updates every loop (NLMS, carrier, timing, `settled`) only if all
three hold:

- the receiver is not lost;
- it is not held;
- `e² < min(d²/4, max(9·settled, d²/400))`.

The terms:

- `e = z − target` is the tracking decision's error.
- `d²` is the constellation's d²_min at unit power.
- `settled` is the EMA (0.01) of accepted `e²`.

**The gate is relative.** A Gaussian error passes 9·settled with probability
1 − e⁻⁹. The decision boundary `d²/4` only caps it. It is an absolute level
only in constellation units, which the decision-free AGC makes true whatever
the line's level. The `d²/400` floor stops a 50 dB line refusing symbols
over nothing.

**Loss is declared on either of two conditions.** Each is judged over the
density class's window (§4.1):

- the mean of the recent `e_near²` passes the threshold;
- **the gate has refused at least 75 % of the window.**

The second closes a deadlock the relative gate would otherwise open. Take a
one-sample slip at 4800: it rotates the constellation 40.5°, which gives
`e² = 0.48`. That is under V.34's Points floor (`d²/4 = 0.5`), so the loss
detector alone never fires, while every symbol fails `9·settled`.

**AGC and the AGC-gain decision:**

- Every symbol updates the AGC, and every symbol updates the `error` EMA
  behind `snr_db()`. Neither waits for the gate.
- The AGC freezes only while lost, because silence would otherwise wind the
  gain up.

### 2.4 Gain

- **At training**, the least-squares solution carries the absolute gain.
  `g = 1`, and the AGC's power estimate starts at 1.
- **In tracking**, the AGC holds `E|z|²` at the table's mean power (1).
  - Noise biases it by σ²/2, which is 0.01 dB at 27 dB.
  - The level difference between TRN and data at 12 000 and 14 400 (spec.md
    §2.1: +0.21 and +0.11 dB) disappears into it, so the receiver never
    needs to know the far end's level convention.
- **At a slicer change** (E, retrain), the AGC's time constant changes, and
  `g` and `settled` carry over (core.md §7.5, §9 7.7).
- **At resync**, the least-squares gain/phase fit (`resync_dense`'s `fit`,
  V.34 `:1049-1059`) runs in *both* resyncs. Its gain replaces `g`, and the
  AGC's power estimate resets to 1. The fit writes `g`, not the taps; the
  V.34-compatibility mode, with the AGC off, writes the taps as V.34 does.
- **Snapshots** include `g` and the AGC state.
- **Ramps**: a type-1 AGC lags a ramp by slope × τ.
  - The softphone limiter of `v90::network` (instant attack, then an
    exponential release of about 1 s) ramps about 0.0025 dB a symbol at
    most. That is a 0.08 dB lag at τ = 32 during the start-up, and 0.64 dB
    at τ = 256 in the rare case it is still releasing in data mode.
  - Anything faster is a loss, and then a resync with a gain fit, which
    recovers it in one step.

### 2.5 Timing and the taps' centroid

The data-aided timing detector and the T/2 taps share the delay degree of
freedom, and nothing anchors their sum (core.md §9, N6). Nothing has shown it
drifting: E5 held for 120 s. Hour-long file transfers are the use case,
though, so the core gets an anchor.

- Every 1024 symbols, compute the taps' energy centroid.
- If it is more than one half from the centre, shift the taps one half the
  other way and move `due` by one half.
- It is off in V.34-compatibility mode.

### 2.6 Tentative decisions, by rate

| constellation in use | tracking decision (drives gate and loops) | bits |
|---|---|---|
| 4 points, A–D (start-up, 4800) | nearest of A–D. The table slice has exact Voronoi boundaries, so the 22.5° mistake is impossible here. | label → Table 1 inverse (`CHANGE_TO_DIBIT`) → descramble |
| 16 uncoded (9600U, V.32 §2.4.1.1) | nearest of 16 | quadrant change → Q1Q2, within-quadrant → Q3Q4 (Table 3), as `v32.rs:1096-1118` |
| 16, 32, 64, 128 trellis | **the data decoder's best survivor at zero delay.** Call `Decoder::decode(z·√10)`, then the new additive `Decoder::tentative()`: the code index on the transition into the best state (y0 from the from-state, y1 y2 q from the step). For the first 16 symbols after any rate change or resync, the nearest point instead, while the survivors are still meaningless. | `Decoder` groups, as today (depth 24) |

**Rejected:**

- *The whole-constellation slice* (V.34's `Grid`): 3.5–6 % wrong at the
  working SNR [P1].
- *The Y0 half-constellation slice*: 0.4–1 % wrong, about twice D=0, for the
  same cost.
- *Delayed traceback D ≥ 1*: another 2× better, but it puts D symbols of delay
  inside a 50-symbol carrier loop and into snapshot/rewind bookkeeping, for a
  gain the gate already mostly delivers.

Loss detection, resync and `residual_error` use the nearest-point error
`e_near`, so that their thresholds (V.34's, and today's
`UNSATISFACTORY_GAP`) keep their meaning.

**Constellation tables** are `dsp::qam::Constellation` values: unit-power
points indexed by label, with `d²_min`, the garbage MSE and a density class.
V.32's are built in `v32/receiver.rs`:

- `STATES / √10` for the 4 points;
- `signal_point(state, within) / √10` for the 16;
- `trellis::Coded::point(code) / √10` for the coded rates.

The nearest-point search is by lattice rounding, (x, y) for the square sets
and (x+y, x−y) for the crosses, with a precomputed map for points outside the
constellation. A unit test checks it is identical to exhaustive search.
128-point exhaustive search inside `resync_dense`'s 32 × 60 × 64 trials would
cost about 16 M distance evaluations per resync.

### 2.7 What reaches the scope and the modem crate

| accessor | new meaning | contract (contract.md §1.1) |
|---|---|---|
| `constellation_point()` | last `z`, unit power; changes exactly once per produced symbol, unchanged while idle | "last equalised symbol / √10, unit RMS", once a symbol ✓ |
| `residual_error()` | EMA (0.01) of `\|z − nearest\|` over every produced symbol, gated or not; frozen when no symbols are produced | "mean \|decision error\|, unit-RMS" ✓; now also rises while lost, which the retrain rule needs (§4.6) |
| `point_spacing()` | `v32::point_spacing_at(rate, coding)`, same numbers | ✓ |
| `carrier()` | today's detector unchanged in line terms: a 20 ms envelope of the in-band amplitude with the same on and off levels (1e-3 / 5.62e-4 at today's mixer scale; the core mixes ×2, so ×2 there), measured on the core's half-symbol samples | ✓ NO CARRIER timing unchanged (a hole drops it after about 100 ms at normal levels) |
| `take_bits()` / `take_bytes()` | one shared buffer; 2 bits a symbol at 4 points whenever symbols are produced, including during TRN, where the bits are garbage and are discarded; none while idle or hunting | ✓ (§3.1) |
| new diagnostics | `snr_db()`, `trained_snr_db()`, `slips()`, `is_lost()`, `lost_for()`, `drift_ppm()`, `offset_hz()`, `gain_db()`, `stage()`, `is_tracking()` | for transcripts, replays and Rory's live checks |

The GUI's `snr_db = −20·log10(residual_error)` and
`reception = residual / spacing` keep their meaning.

---

## 3. Acquisition in the real start-up

### 3.1 What each segment is used for

The receiver has one stage machine: **Idle / Hunting / Training / Tracking /
Blind.** The `Startup` sets the stage with *cues* at state entries (§3.2). It
no longer steers the receiver through `set_adapting`.

| far end sends | receiver stage | used for | clause |
|---|---|---|---|
| ANS, AA/CC (heard by the answerer), AC/CA/AC (heard by the caller), gaps | Idle (history keeps filling) | nothing in the receiver; the `Listener` and reversal detectors run the start-up as today. AA and AC would give frequency, but S does it better and is always right before TRN, so the tones are not used (rejected alternative) | V.32 §5.4.1-2 |
| optional EC sequence (≤ 8192T, broadband) | Hunting | refused by the discriminator | §5.4 Note 3 |
| **S** (256T; the caller's is NT + 256T) | Hunting | S found by V.34's hunt plus a discriminator that refuses AA/CC and AC/CA; carrier frequency coarse and fine (§3.3) | §5.2.1 |
| **S̄** (16T) | Hunting → Reversal | **time reference**: TRN symbol 0 is centred on half `at + 32`, searched ±8 halves; resolves S's 180° | §5.2.2 |
| **TRN 0–255** (A/C) | Training (collecting) | LS alignment window, symbols 16–256 | §5.2.3 |
| **TRN 256+** (Table 5) | Training → Tracking | LS solve over 16–512, or the retry over 640–1152 searched ±200 halves: taps, gain, absolute phase, turn. Then decision-directed at 4 points to the end of TRN | §5.2.3 |
| R1/R2/R3 | Tracking (4 points) | Table 1 decoding from TRN's last symbol, descrambler (far polynomial), bits to `RateDetector` | §5.3, Table 6/bis Table 5 |
| E | Tracking (4 points) | read by the `Startup`, which calls `set_data_rate`/`set_coding` as today (`startup.rs:1315-1321`) | §5.3.2 |
| B1 (128T) | Tracking (data table) | slicer and AGC time constant switched with taps kept (§3.6); Viterbi warm-up | §5.4.1, §5.4.2 |
| data | Tracking | — | — |
| our own ANS, AC/CA, AA/CC, S, S̄, TRN (first) | Idle | not our far end; the canceller trains in our TRN (§5) | §5.2.3, §4.8 of spec.md |

### 3.2 The cues (`Startup`, on every state entry, including the first step)

| state entered | calling end | answering end |
|---|---|---|
| Listening, Aa, AaToCc, Cc / AnswerTone, RetrainAc, Ac, Ca, CaToAc, AcAgain, Gap | `rx.idle()` | `rx.idle()` |
| AwaitingR1 | `rx.hunt()`: the answerer's first S S̄ TRN R1 | — |
| PreRoll, SendS, SendSBar, SendTrn (first, while `!trained`) | `rx.idle()`: our own uncancelled S is on the line, and the far end stops R1 | `rx.idle()` |
| SendRate, first time (R1 from the answerer, R2 from the caller) | `rx.hunt()`: the answerer's **second** S S̄ TRN, then R3 (full duplex with our R2) | `rx.hunt()`: the caller's S (NT + 256T) S̄ TRN, then R2 |
| AfterR1, AwaitingR2, the second conditioning, SendEnd, Settling, Connected | — (keeps whatever stage it is in) | — |
| `start_again` (retrain) | `set_data_rate(4800)`, `set_coding(Uncoded)` as today, then `rx.idle()` | same |

**What the receiver does on its own:**

- *Untrained*: it hunts again.
- *A hunt that lapses after S was armed* (no reversal found, for instance a
  slip across the join): it trains anyway. The start is taken where S ended
  plus 32 halves, searched ±200 halves with V.34's correlation pre-score
  (`:789-807`).
- *A training that fails after an earlier one in the same call* succeeded:
  it keeps the old taps and runs a resync search on the newest window
  (core.md §2.3's idea, on V.32 terms). If found, it tracks at 4 points.

**What changes in the `Startup` and `Modem`:**

- `Modem` no longer calls `set_adapting` (`startup.rs:2235`). `far_end_quiet`
  and `far_end_finished_training` stay as diagnostics.
- `set_adapting(false)` survives as a public "hold every loop" that keeps
  symbols flowing.
- `v32_startup.rs`'s bare `Startup` gets the same cues, because the cues live
  in `Startup` and not in `Modem`.

### 3.3 Frequency from S

- **Coarse**: `arg(Σ h_n·conj(h_{n−4}))/2` rad/symbol over the armed S (the
  hunt's lag-4-halves correlation). It is unambiguous to ±600 Hz and needs no
  equaliser.
- **Fine**: keep the template's average over consecutive 128-symbol blocks of
  S. At the reversal, `arg(B_last·conj(B_prev))/128`, unwrapped around the
  coarse value. For a heard S of fewer than 256 symbols after arming, use its
  two halves.
  - Precision about 0.02 Hz at 20 dB.
  - The unwrap is safe while the coarse error is under 9 Hz. The coarse
    estimate is good to about 0.7 Hz.
- The turn is reported in `Reversal { at, turn }` and passed as
  `Training::turn`.
- The solve pre-rotates the targets by it. V.34's early/late fit (`:829-835`)
  then measures only the residual δ, and the carrier loop starts at
  `turn + δ`.
- This removes E2's collapse (15 dB at ±7 Hz, aliasing at 10 Hz).

### 3.4 The S discriminator

The discriminator runs over the last 16 halves.

- **Components**: a DC component `d = mean(h)` and the two ±1200 Hz
  components `u, v = mean(h·e^{∓jπn/2})`.
- **Rule**: a half is S-like only if V.34's hunt condition holds *and* the DC
  share `|d|²/(|d|²+|u|²+|v|²)` lies in [0.25, 0.92].
  - AA and CC have a share of about 1: a bare carrier.
  - AC and CA have a share of about 0: the carrier is suppressed.
  - S has 0.44–0.71 for the 2–7 dB band-edge loss V.32 §2.2 allows: the
    DC line against two sideband lines each 2–7 dB down, `1/(1+2·10^(−L/10))`.
- **Why a ratio**: the far end's pulse shape is unknown, so the rule is not
  "lag-1 correlation near zero".
- **The same test as the tones**: this is `Listener::classify`'s
  carrier-and-sidebands test in the symbol domain. The hunt is still armed
  only when S is due (§3.2), because E6 found V.34's hunt firing on AA→CC and
  AC→CA.
- **S may start on B**, because our transmitter alternates by tick parity
  (spec.md §2.6.5). The template hunt does not care. The targets begin at
  TRN, which S̄ fixes.

### 3.5 Least squares on TRN

- **Targets**: `v32::TrnSequence` (package C). It is the far end's
  polynomial (`Mode::peer()`), zero-started (§5.2.3), with ones in. Symbols
  0–255 are A or C by the dibit's first bit; from 256 the dibit maps by
  Table 5 as printed: **00 A, 01 B, 11 C, 10 D**. Points are at unit power.
- **One generator**: the same sequence drives our transmitter, and our
  transmitter is fixed to match (§6).
- **The table is checked twice**: against spec.md §4.3's printed vectors, and
  against a real modem's TRN in `tests/vectors/v32bis-14400.wav`, where the
  Table 5 targets must fit and the swapped ones must not.
- **The two-point part (0–255) is a sound LS input**: a linear equaliser's
  normal equations use only `E[x xᴴ]` and `E[x s*]` (core.md §7.3). The
  targets are complex and absolute, so the phase is absolute too.

| window (symbols of TRN) | first try | retry |
|---|---|---|
| alignment search | 16–256, ±8 halves | 640–1152, ±200 halves pre-scored by correlation, best ±3 solved |
| solve | 16–512 | 640–1152 |
| done by | about symbol 530 | about symbol 1170 (< 1280, the shortest TRN) |

- **Accept**: ≥ 12 dB, else retry. Under 6 dB is Untrained.
- **Ridge**: `1e-3·energy/31` (V.34).
- **After training**, symbols from the window's end are produced at once, and
  the driver drains them in the same `feed`.

### 3.6 From 4 points to the data constellation

At E, `set_data_rate`/`set_coding` do the following:

- **Swap the slicer**: `core.set_slicer(table)`. Taps, rotation, turn, timing,
  `g` and `settled` are kept. `lost` and `recent` are cleared (core.md §7.5).
- **Change the AGC's time constant.**
- **Reset the trellis decoder** only when the coding changes, as today
  (`v32.rs:949-959`).
- **Use nearest-point decisions for 16 symbols.**
- **Tolerate a late switch.** The `Startup` switches at its next symbol tick
  after reading E, so 0–1 B1 symbols may be decided on 4 points. Their large
  errors are refused by the gate, and B1 carries no data (§5.4.1). No
  symbol-exact switch is needed.

### 3.7 Retrain

- `start_again` puts the rate back to 4800 and the receiver to Idle.
- The procedure then runs exactly as in §3.2: the hunt is re-armed at
  AwaitingR1 or SendRate, and the next TRN's solve replaces the taps.
- The old taps survive only as the fallback of §3.2.

### 3.8 Blind start (bare receivers, for tests)

`Receiver::new` starts in **Blind**, at whatever rate and coding are set.
`Startup`'s first cue replaces it, so no call ever uses it. It exists for
`v32_loopback.rs` (4800 at 8 arrival phases, 9600T at phase 0, 4800 through a
cancelled hybrid) and for lock_sweep.

1. Set the taps to the RRC(0.25) matched response sampled at T/2, normalised,
   so that our own transmitter gives a Nyquist overall pulse.
2. Once 64 symbols of signal have arrived above the level floor, run the
   resync search (§4.3, sparse or dense by the table) on the newest window
   every 32 symbols.
   - Try the turn at 0, and at the z⁴ estimate
     (`arg Σ z_k⁴·conj(z_{k−1}⁴)/4` over the centre-tap outputs) once 256
     symbols are in.
   - The z⁴ line is 0.14 on the crosses [P2], so the estimate needs those 256.
   - Accept on the median test alone (`≤ 0.6 × median`), plus `≤ ½ garbage`.
3. When the search is found, `settled` is the found MSE and the stage is
   Tracking.

Measure this early: core.md §8 lists it as a risk. Package A proves it on its
own modulator; D proves it on V.32's transmitter before anything else.

---

## 4. Holding on

### 4.1 Loss, by density class

| class | constellations | judged over | lost when mean recent `e_near²` > | or gate refused | first resync | then |
|---|---|---|---|---|---|---|
| sparse | 4, 16 | 8 symbols | `max(8·settled, d²/4)` | ≥ 6 of 8 | 24 symbols after loss | every 32 |
| dense | 32, 64, 128 | 32 symbols | `max(2·settled, d²/12)` | ≥ 24 of 32 | 96 symbols after loss | every 32 |

Garbage on the dense sets reads about d²/6, *under* the sparse floor d²/4
(core.md N5). That is why density, and not the kind of slicer, picks the
arithmetic.

- On loss: `rewind(judged + 16)`.
- A resync is attempted only if the window's power is at least ¼ of the power
  before the loss. A trained receiver facing silence then waits instead of
  searching on nothing (core.md N4).

### 4.2 Rewind

- Snapshots are taken every 16 symbols while not lost, 24 of them (384
  symbols, 160 ms). They hold the taps, rotation, turn, drift, slope, timed,
  settled, error, `g` and the AGC state.
- `rewind` returns `bool`. When no snapshot is old enough it restores the
  oldest and returns `false`, so the driver can count it; V.34 silently did
  nothing (`:999`).

### 4.3 Resync

**Sparse**, copied from `resync` (`:1148-1211`):

- A 48-symbol window **ending 4 symbols short of the newest symbol** (the fix
  for core.md §4.3's look-ahead hole). The same 4-symbol margin as
  `resync_dense`.
- 16 shifts across ±1 half, in 1/8 steps.
- Fourth-power phase nearest the previous one, then 3 decision-directed
  refinements.
- **Then a 4-round gain and phase least-squares fit (new).**
- Accept when `≤ max(4·settled, d²/16)` and `≤ 0.6 × median`.

**Dense**, copied from `resync_dense` (`:1027-1103`):

- A 64-symbol window ending 5 short.
- 32 shifts; 60 × 1.5° coarse phase; the gain/phase fit; fine steps of
  1–3/64 half.
- Accept when `≤ max(2·settled, 0.4·d²/6)` and `≤ 0.6 × median`.
- **The rotation after take-up is extrapolated to `next_symbol`, not to the
  window's middle** (core.md §9 7.6, 4.5 symbols of turn).

**Both:**

- `take_up` as V.34 (`:1108-1136`): every stored half re-read on the moved
  grid, `due` moved, `slips += 1`.
- Timing is resolved modulo one symbol and phase modulo 90°. Neither matters:
  Table 1 is differential, and the trellis code is invariant to 90° (spec.md
  §2.4 check 1).
- The descrambler self-synchronises in 23 bits. The Viterbi decoder allows
  every state.

### 4.4 Slips

| event | what the loops see | what happens |
|---|---|---|
| one sample dropped or repeated at 16 kHz (sound-card drop or underrun of one sample) | 40.5° carrier step plus ±0.15 symbol (±0.3 half) | loss (the gate refuses, §2.3), then rewind, then a resync at 24 or 96 symbols. The +0.3 half case was the look-ahead hole's edge (E3h: 1.1 s lost on 4 points); the 4-symbol margin puts it inside the search. Cost: tens of symbols; no retrain, no rate change |
| underrun gap of k samples (silence) | timing k/6.67 symbols mod 1, carrier 40.5°·k | the same, after the gap's garbage |
| 20 ms concealment slip (repeat, comfort noise or silence, insert or drop) | nothing: 48 symbols and 36 turns of 1800 Hz exactly (core.md §4.7); the concealed stretch is garbage or a valid-looking repeat | garbage gives loss, then rewind, then a resync that finds shift 0 at the same phase; a repeat gives no loss. Cost: 48 symbols of data (96–288 bits); V.42 retransmits |
| 20 ms slip inside TRN | the fit window straddles it | the retry window (640–1152, ±200 halves = ±42 ms) covers it (V.34's own design, `:749-755`) |

### 4.5 Gain steps

- **Small steps**: up to 1 dB at 16 points, or 0.3 dB at 128. The AGC
  follows them without loss.
- **Larger steps**: loss, then rewind (which restores the pre-step `g`), then
  a resync whose gain fit finds the new `g` in one step.
- **The −6 dB ramp**: E4b's false lock at 12 dB cannot recur. The AGC removes
  the systematic inward decisions that caused it, and the relative gate stops
  `settled` rising to meet them.

### 4.6 When to give up and ask for a retrain

The retrain rule stays as it is: residual > `UNSATISFACTORY_GAP` × spacing for
2400 symbols (`startup.rs:1878-1894`).

- **Why a loss reaches it**: `residual_error` now rises while lost (§2.7).
  About 1 s of continuous loss therefore ends in a clause 7 retrain, as
  V.32 §5.5 allows, without any new watchdog.
- **The one change**: at the moment of the retrain decision, if
  `rx.lost_for() ≥ 1200` symbols, the `Startup` retrains **without
  `stop_offering`**. The line did not prove a rate unreadable; it lost the
  signal.
- **Reading an unreadable rate**: while tracking with too much error,
  `stop_offering(rate, rx.residual_error())` runs as today.
- **Hang-ups**: a far end that hangs up drops `carrier()` about 100 ms later,
  and the modem crate ends the call before any retrain (unchanged).

---

## 5. The echo canceller

**Decision.** NLMS stays as it is: it adapts only in this end's first TRN,
at μ 0.5 (`startup.rs:2226-2240`). The canceller gains a **drift-following
mode** that is switched on once that training ends and runs through the
whole of data mode.

**Why:**

- NLMS in data mode cannot do the job [P3]. The far end, as loud as the echo
  on a cable, is noise to it. At μ = 0.002 its misadjustment alone is −24 dB,
  and it still does not follow 20 ppm (−10 dB).
- A single delay parameter has about 190× less adaptation noise for the same
  speed.
- The only full-strength echo on Rory's rigs is the cable's [P4].

### 5.1 The drift-following mode (`dsp::echo`, additive)

- **Retiming.** Every tap at lag ≥ 17 samples reads a retimed reference
  `x(n − L − τ)`, from a 32-tap Kaiser-windowed sinc with 1024 phases.
  - Lags 0–16 (hybrid-like echoes inside 1 ms) are left unretimed, because
    the interpolator needs 16 samples of look-ahead. Nothing that drifts can
    arrive inside 1 ms of a sound card's buffering.
  - When |τ| passes 0.5, the retimed taps shift by one and τ wraps.
- **The delay loop.**
  - The error is `ε = −e·ŷ′/P`, where ŷ′ is the retimed taps applied to the
    reference's derivative (the central difference at ±½ sample) and P is an
    EMA of ŷ′².
  - It is second order and critically damped:
    - B_L 8 Hz for the first 0.5 s after switching on: Kp 1.6e-3,
      Ki 6.4e-7 per sample;
    - then 1.5 Hz: Kp 3.0e-4, Ki 2.25e-8 per sample.
  - That follows 100 ppm (1.6 samples/s) with no standing error.
  - It updates only while we are sending (reference energy ≥ ¼ of its mean),
    while the echo estimate is ≥ −40 dB of what is heard (a four-wire or VoIP
    line has nothing to track), and while not held.
- **The relative gate.**
  - `short` is a 5 ms EMA of e², and `settled` a 125 ms EMA updated only
    while not held.
  - It holds when `short > 1.25·settled`.
- **The integer jump search**, after 10 ms held:
  1. Compute the echo estimate once over a 30 ms window ±480 samples.
  2. Score every integer shift s ∈ [−480, 480] by `Σ(r − ŷ(n−s))²`. This is
     under 1 M multiply-adds, about 1 ms: safe inside `modem-loop`'s
     real-time loop.
  3. Accept when `score(s) < 0.8·score(0)`. Then τ = τ from before the hold,
     plus s, and the loop goes back to 8 Hz for 200 ms.
  4. A search that keeps 0 re-baselines `settled` and releases, so a change
     in the far end's level does not hold it for ever.
- **Off by default**: with `follow_drift` off, the canceller is bit-identical
  to today.

### 5.2 Wiring (`Modem`)

- `follow_drift(true)` goes on when `trained` first becomes true. The tracker
  is paused while NLMS is adapting (a retrain's TRN) and keeps its rate
  across it.
- The receiver and the canceller have **independent** gates.
  - Holding the receiver whenever the canceller holds was rejected: a far-end
    level change would hold both until the canceller re-baselines.
  - The receiver's own loss detection already refuses the uncancelled stretch
    after a slip.
  - The resync re-reads stored residual samples, and succeeds on the first
    window after the canceller has realigned. That is within about 100 ms
    of a one-sample slip on the cable.
- **V.32 §5.2.3** ("TRN … the echo canceller in the transmitting modem"):
  - Each end's TRN is the other end's quiet line. The receiver is Idle
    through our own TRN, which is when the canceller learns.
  - The far end's TRN reaches a canceller that is not adapting. The tracker
    sees ŷ′ ≈ 0 while we are silent and does not update.
  - In the full-duplex second conditioning (the caller's R2 over the
    answerer's second S S̄ TRN), the canceller's NLMS is frozen, and the
    tracker's noise is set by the far end, as in data mode.
- **Diagnostics**: `Modem::echo_drift_ppm()` and `echo_jumps()`.

---

## 6. Fixes outside the receiver (all in this job)

| fix | where | why now |
|---|---|---|
| TRN after symbol 256: `TRN_STATES = [A, B, D, C]` (Table 5: 00 A, 01 B, **11 C, 10 D**) | `v32.rs:427`, used at `:708`; replaced by `TrnSequence` | the LS trainer needs it on our own loopback; a real far end that trains on the known TRN has been getting the wrong half of it (spec.md §2.6.1) |
| 4-point slicer rotated 26.57° (45° − atan(1/3)), not 22.5° | `v32.rs:298-312` `nearest_state` | biases every 4800 and start-up decision (spec.md §2.6.2). The new table slicer is exact anyway; the fixed function stays for the unit tests |
| 12 000 and 14 400 data at ×√(42/40) and ×√(41/40) (+0.21 / +0.11 dB) against the training states | `v32::Transmitter::next_symbol`, after `coded.point(code)` (`v32.rs:788`); **not** `trellis.rs`, whose tables V.17 shares | the only level statement the Recommendation makes (spec.md §2.1); a far end whose gain comes from TRN expects it. Our receiver does not care (§2.4) |
| comments: `v32.rs:1` ("at 4800"), `:369` ("Figure 2" is Figure 1), `trellis.rs:569` ("V.32 clause 8") | C for v32.rs, D for trellis.rs | wrong |
| S may start on B | not changed | S̄ is still exactly −S, and pinning S to A would move the 64 ± 2T joins (spec.md §2.6.5) |

---

## 7. The contract, old against new

| today (contract.md) | after |
|---|---|
| `Receiver::new(mode, fs)`, `feed`, `take_bits`, `take_bytes`, `set_data_rate`, `set_coding`, `carrier`, `constellation_point`, `residual_error`, `point_spacing` | same signatures and meanings (§2.7) |
| `set_adapting(b)` freezes the AGC, carrier and equaliser, while the frequency still advances | holds every loop (gate shut) and keeps symbols flowing; no longer called by `Modem` |
| `level()`, `point_spacing_at()`, `equalizer_blind()` on `Receiver` (no callers) | removed |
| `Modem::equalizer_blind()`, `receiver_adapting()` (no callers) | kept; now "not yet tracking" and "tracking" |
| `Scrambler`, `Mode`, `trellis::{Coded, AT_*}`, `Coded::point/size/closest/peak/nearest` | untouched |
| `trellis::Decoder` | plus the additive `tentative()` |
| `dsp::Equalizer`, `dsp::Gardner`, `dsp::EchoCanceller` | untouched; the canceller gets additive methods only |
| `startup` public surface (`phase()` strings, `UNSATISFACTORY_GAP`, `Listener`, `Heard`, `RateDetector`, `is_rate_signal`, …) | untouched |

---

## 8. Tests that would change

**None is planned to change.** The blind path keeps every bare-receiver
contract of contract.md §1.2:

- 4800 at 8 phases;
- 9600T at phase 0;
- 4800 through a cancelled hybrid;
- lock_sweep's API.

These are watched, and none is to be edited without Rory's say:

1. **`v32_call::a_rate_that_cannot_be_read_is_given_up`** assumes the hybrid
   line (echo 0.251, far 0.1) cannot carry 14 400.
   - With the echo return loss measured at 30–32 dB, the residual echo sits
     22–24 dB under the far end. That is below the 24.1 dB at which the
     unchanged retrain rule fires at 14 400, so it should still pass.
   - If package E ever raised that line's SNR past 24.1 dB, the test would
     fail because the modem got *better*. The remedy (making the test's line
     explicitly unreadable) needs Rory's approval.
2. **`v32.rs` unit tests** use the private `STATES`, `rotate`,
   `nearest_state`, `nearest_point`, `signal_point` and `WITHIN_4800`. These
   stay in `v32.rs`. `nearest_state`'s correction keeps
   `slicing_returns_each_state_from_its_own_neighbourhood` passing (a 20 %
   nudge stays inside the true Voronoi cell).
3. **lock_sweep `every_slow_mode_against_every_impairment`** overwrites
   `docs/design/slow-modes/before.md`. Package G copies it to
   `before-d7b914b.md` before running it (plan.md rule 5).
4. **Semantics no test pins**, listed so nobody is surprised:
   - `set_adapting`;
   - `residual_error` now updating while lost;
   - the three removed `Receiver` accessors;
   - E's canceller output differing from today's after training;
   - C's TRN and 12 000/14 400 level (no test pins TRN symbols or the
     level at those rates; `every_modulation_goes_out_at_the_same_level`
     runs V.32 at 9600).

---

## 9. Measurement and acceptance

### 9.1 Definitions

- **Es/N0.** White Gaussian noise at 16 kHz of variance
  `σ² = (fs/2)/baud · P · 10^(−SNR/10)`, where P is the far signal's power as
  received. It is the ideal slicer SNR.
- **Working SNR.** Per rate, 3 dB above the higher of two figures:
  - the ideal decoder's 1e-5 point [P1];
  - where today's unchanged retrain rule fires (reception 0.25 of the gap,
    Gaussian).

| rate | ideal decoder, BER 1e-5 [P1] | where the retrain rule fires | **working SNR** |
|---|---|---|---|
| 4800 | 12.5 dB | 8.0 dB | **15.5 dB** |
| 9600U | 19.5 | 15.0 | **22.5** |
| 7200T | 13.5 | 15.0 | **18.0** |
| 9600T | 17.0 | 18.0 | **21.0** |
| 12000T | 19.7 | 21.2 | **24.0** |
| 14400T | 23.5 | 24.1 | **27.0** |

The rule fires 0.6–1.5 dB *above* the ideal 1e-5 point at 7200T, 12000T and
14400T. Changing it is out of scope, because a pinned test and
`stop_offering` depend on it, but it is noted as a follow-up.

- **Held.** The call reaches the offered rate and stays `Connected` at it for
  the whole run, with `retrains() == 0` at both ends.
- **BER.** Seeded pseudo-random bytes sent both ways from 1 s after connect.
  Measured over 100 ms blocks, each block re-aligned by searching ±2048 bits,
  so that slips count as errored blocks and not as a whole broken stream.

### 9.2 Whole calls: `crates/datapump/tests/v32_acceptance.rs` (package F)

These are two `startup::Modem`s using only today's public API.

- **Line model**, per direction:
  - carrier shift (lock_sweep's 255-tap Hilbert `Shifter`);
  - clock (`dsp::Resampler`);
  - gain schedule;
  - delay;
  - slip schedule: single-sample drop or repeat; 20 ms insert filled by
    faded repeat (V.34's `slip`, `receiver.rs:1572`), comfort noise or
    silence; 20 ms drop;
  - AWGN at Es/N0.
- **Echo** is either a hybrid (echo 0.251, far 0.1, as `v32_call`) or none.
- **The cable variant** sums both modems at 0.45 over a 700-sample crossing,
  with out→in drift and slips on the cable.
- **Slip moments** are drawn from a fixed seed, so they are random but
  reproducible.

| test | line | pass (every rate listed) |
|---|---|---|
| `every_rate_holds_at_its_working_snr` | direct, 1-sample delay, working SNR, 30 s of data; each rate forced by offering only it (9600U by the V.32 table without trellis) | held; BER ≤ 1e-5 |
| `seven_hertz_either_way` | ±7 Hz both directions, working SNR; 4800, 9600T, 14400T | held; BER ≤ 1e-5 |
| `two_hundred_ppm_either_way` | +200 ppm one way, −200 the other, working SNR; 4800, 14400T | held; BER ≤ 1e-5 |
| `single_sample_slips` | 10 drops and repeats per direction at random moments over 60 s, working SNR + 3 dB; 4800, 9600T, 14400T | held; each slip costs ≤ 200 ms of errored blocks; ≥ 97 % blocks clean |
| `concealment_slips` | a 20 ms slip every 3–5 s (alternating insert and drop, each fill kind), 60 s; 14400T, 9600T | held; ≥ 97 % blocks clean |
| `gain_steps_and_ramps` | ±3 dB steps, ±6 dB over 0.5 s, and the softphone limiter (instant attack, 1 s release) triggered in the start-up; 14400T, 9600T | held; ≥ 99 % blocks clean |
| `cable_with_drift` | cable at 0, 5, 20, 100 ppm, 60 s, offering 4800–14 400 | 14 400 held; BER ≤ 1e-5 |
| `cable_with_slips` | cable at 20 ppm with 5 one-sample drops, 5 repeats and two 160-sample underrun gaps, 60 s | 14 400 held; ≥ 97 % blocks clean |
| `rorys_voip_line` | 0.7 s each way, ±100 ppm, the softphone limiter, a 20 ms slip every 3–5 s, working SNR; 14400T and 9600T | held for 60 s; ≥ 95 % blocks clean |
| `hybrid_with_everything` | hybrid plus far reflection at 20 ms, +3 Hz, 200 ppm, working SNR; 9600T | held |
| `a_real_loss_still_retrains` | 1.5 s of loud noise replaces the far end mid-call at 14 400 | exactly one retrain, begun ≤ 1.6 s after the noise starts; reconnects at **14 400** (a loss does not stop the offer) |

- **`crates/modem/tests/v32_cable_faults.rs`** (F): through the modem crate,
  like `soundcard_loop.rs`, at 20 ppm with one-sample drops and a 160-sample
  gap. It must reach `CONNECT` at 14 400, get the greeting through, have
  `retrains() == 0`, and never report NO CARRIER.
- **Runtime budget**: the new suites take ≤ 120 s in release together.
- **Before any receiver change**, F records its before table. Tests that fail
  today carry `#[ignore = "V.32 rebuild: enabled by package D"]`. D removes
  the attributes, and changes nothing else in F's files.

### 9.3 Receiver level: `crates/datapump/tests/v32_receiver.rs` (package D)

- Our own S, S̄ and a 1280T TRN into a bare receiver:
  - trained ≥ 45 dB on a clean line;
  - ≥ 34 dB at 35 dB Es/N0 with ±7 Hz and ±200 ppm;
  - the S frequency estimate within 0.1 Hz;
  - R1 bits read.
- **The real modem.** `tests/vectors/v32bis-14400.wav` (mono), on the
  answering modem's first S S̄ TRN (4.25–7.00 s) and the calling modem's
  (7.75–10.50 s), run twice: once as the calling end's receiver, once as the
  answering end's.
  - Trained ≥ 25 dB on each.
  - The frequency from S reads 1800.0 and 1798.1 Hz, ±0.3 Hz (contract.md
    §5).
  - The R1 that follows is read, sync bits and all.
  - This is V.32's counterpart of V.34's real-modem TRN check
    (`v34/receiver.rs:13-21`).
- Every existing V.32 test, bare and whole-call, passes unchanged.

### 9.4 lock_sweep (bare receiver, cold and blind at the data rate)

Run as contract.md §6.3 says. G adds a V.32-only ignored test that does not
touch `before.md`.

| row | before | after |
|---|---|---|
| `one_mode_clean` phases | 8 / 6 / 7 / 3 / 3 / 2 | **8/8 at every rate** |
| clean slicer SNR | 37 / 29 / 31 / 31 / 29 / 32 dB | **≥ 45 dB at every rate** (V.34's core reaches 54, E1) |
| silence 0 / 300 / 1100 ms | 4800 8/0/5; 14400T 2/0/0 | 8/8 in every cell |
| offset ±1 and ±3 Hz | 0–7 of 8 | 8/8 at every rate |
| offset ±7 Hz | 0/8 above 4800 | ≥ 7/8 at 4800, 9600U and 7200T; informative for the crosses (a call never cold-starts at the data rate) |
| holes 2–50 ms at 4800 | BER 0.0013–0.016, carrier held | carrier held; BER no worse |

### 9.5 Captures (package G; `crates/datapump/tests/v32_capture_check.rs`, ignored, env-var paths)

**`captures/live-1788855280.wav`** (`V32_FROM=14.4 V32_OFFER=9600`):

- Before: `Connected(9600)` at 24.22 s, then the rotation walks 0 → −20° in
  0.8 s, SNR goes from 17 to 4 dB, and it retrains 1 s in.
- After: from `Connected` to the far end's next retrain tone or the end,
  - no receiver-initiated retrain;
  - rotation drift under 2° a second;
  - median SNR per second within 3 dB of the first second's.
- Also report the trained SNR, the S frequency, drift, gain and slips.
- The replay's canceller references the replayed transmitter. [P4] found no
  echo on this line, so that limitation does not bite here.

**`captures/live-1788841427.wav`** (`V32_FROM=10.6 V32_OFFER=4800`):

- Before: connects at 20.44 s, decodes 77 225 octets, residual 0.03–0.05,
  error bursts at 21.2, 21.7 and 22.0 s.
- After: connects, decodes ≥ 77 225 octets, residual ≤ 0.05, bursts no worse.

**`tests/vectors/v32bis-14400.wav`**: as §9.3, plus the least-squares fit on
the swapped Table 5 targets, which must be at least 6 dB worse on symbols
≥ 256 (package C does the first version of this with a hand-built front end).

### 9.6 Always

On every package:

- `cargo test --workspace --release` passes;
- `cargo clippy --all-targets -- -D warnings` passes;
- `git diff --stat main -- crates/datapump/src/v34 crates/datapump/src/v90`
  is empty.

---

## 10. Work packages

Each package runs in its own worktree. Files are listed by owner, and no two
packages in the same wave share a file.

| wave | package | owns (create or edit) | needs |
|---|---|---|---|
| 1 | **A** core | `crates/dsp/src/qam/` (new: `mod.rs`, `front.rs`, `slicer.rs`, `hunt.rs`, `train.rs`, `track.rs`, `resync.rs`, `blind.rs`), `crates/dsp/src/lib.rs` (one `pub mod`), `crates/dsp/tests/qam_core.rs` (new) | — |
| 1 | **C** transmitter conformance | `crates/datapump/src/v32.rs` (transmitter side, `nearest_state`, comments, new `TrnSequence`), `crates/datapump/tests/v32_trn.rs` (new) | — |
| 1 | **E** echo drift | `crates/dsp/src/echo.rs` (additive), `crates/dsp/tests/echo_drift.rs` (new), `crates/datapump/src/v32/startup.rs` (**`Modem` struct and `impl Modem` only**), `crates/datapump/tests/v32_cable.rs` (new) | — |
| 1 | **F** acceptance harness | `crates/datapump/tests/v32_acceptance.rs` (new), `crates/modem/tests/v32_cable_faults.rs` (new) | — |
| 2 | **D** V.32 receiver on the core | `crates/datapump/src/v32/receiver.rs` (new), `crates/datapump/src/v32.rs` (old receiver out; `mod receiver; pub use receiver::Receiver`), `crates/datapump/src/v32/startup.rs` (cues, retrain rule; `Modem` drops `set_adapting`), `crates/datapump/src/v32/trellis.rs` (additive `Decoder::tentative`, comment), `crates/datapump/tests/v32_receiver.rs` (new); **only `#[ignore]` removals** in F's two files | A, C, E, F merged |
| 3 | **G** verification | `crates/datapump/tests/v32_capture_check.rs` (new), `crates/datapump/tests/lock_sweep.rs` (additive V.32-only test), `crates/modem/src/bin/modem-loop.rs` (print retrains, slips, drift, echo jumps, rate, reception), `docs/design/v32-rebuild/results.md` (new), constant tuning in D's files | D merged |

**Renegotiation (bis §8), not built.** What it would need:

- **Receiver (D's file)**: a data-mode watcher for AA/CC or AC/CA runs of
  equalised points, 56T then a reversal (A is not a point of the 9600T,
  12 000 or 14 400 sets). Then `set_data_rate(4800)` mid-call with taps kept,
  which D's design already supports. R4/R5 are read through the
  self-synchronising descrambler from the preamble's last symbol, and E
  switches as at start-up.
- **Startup**: states for §8.1 and §8.2 (56T + 8T, 24T B1, a reversal at 56T
  telling a preamble from a > 128T retrain tone).
- **Transmitter**: the scrambler reset at R4/R5, and the 24T B1.

### Briefs

**A — the shared QAM core (`dsp::qam`).**

Read `docs/design/v32-rebuild/design.md` §2–§4 and core.md in full, then
`crates/datapump/src/v34/receiver.rs`.

Build `dsp::qam::Core` by copying V.34's generic machinery (core.md §5, right
column), citing source lines. Do **not** edit `v34/` or `v90/`. Fix the
defects in the copy:

- the resync window ends 4 symbols short;
- a gain fit in both resyncs;
- the decision-free AGC;
- the relative gate, with loss declared from gate refusals;
- `rewind` returns `bool`;
- `resync_dense`'s rotation is extrapolated;
- turn from S, with the discriminator;
- the unweighted phase detector for tables;
- level-gated resync;
- the tap-centroid anchor.

Each option that differs from V.34 is a field of an options struct, with
V.34's behaviour as the default.

The API: `Band{fs, baud, carrier, cutoff}`; `Constellation` (unit power,
labels, `d2min`, density, fast nearest); `Slicer::{Table, Grid}`; `Training`;
`Heard`; and pull-style `feed`, `next`, `settle(target)`. Add `idle`, `hunt`,
`train`, `acquire_blind`, `set_slicer`, `hold`, `rewind`, and the reporting
accessors of §2.7. `dsp` must not depend on `datapump`.

Prove it in `crates/dsp/tests/qam_core.rs` with your own RRC modulator (any
roll-off), constellation tables (4, 16, the 32- and 128-crosses) and PRBS
targets, all at 2400/1800 at 16 kHz:

- clean: trained and tracked ≥ 50 dB;
- 30 dB: ≥ 28.5 trained, within 1 dB of 30 tracked;
- ±3 and ±7 Hz at 35 dB: ≥ 33 dB, with the turn from S within 0.1 Hz;
- ±200 and ±1000 ppm;
- insert and drop of every length from 1 to 340 samples on 16 points and on
  the 128-cross: all recovered within 150 symbols (E3c's band gone);
- pure timing jumps +0.25 to +0.35 symbol: recovered;
- ±3 and ±6 dB steps, and ramps over 0.1–0.5 s: recovered within 0.5 s, no
  false lock;
- AA→CC, AC→CA and a broadband EC-like sequence give no S; the S→S̄ reversal
  comes within ±1 half;
- silence after training causes no resync storm;
- blind: 4 points at 8 arrival phases, 16 points, and the 32-cross at phase 0,
  within 200 symbols;
- 10 minutes at 200 ppm: SNR within 0.5 dB throughout, taps' centroid within
  ±2 halves.

Workspace tests and clippy pass.

**C — V.32 transmitter conformance.**

Read design.md §3.5 and §6, and spec.md §2.1, §2.6 and §4.3. In
`crates/datapump/src/v32.rs`, add `pub struct TrnSequence` (`new(mode)`,
`next() -> state`): the zero-started scrambler of that mode fed ones, with
symbols 0–255 as A/C by the first bit and Table 5 from 256 (00 A, 01 B, 11 C,
10 D). Make `Signal::Trn` use it; this fixes `v32.rs:427/708`.

Also:

- correct `nearest_state` to rotate by 26.57° (45° − atan(1/3));
- scale transmitted data points at 12 000 by √(42/40) and at 14 400 by
  √(41/40), in the transmitter only, not in `trellis.rs`;
- fix the comments at `:1` and `:369`.

Do not touch the `Receiver` beyond `nearest_state`.

`crates/datapump/tests/v32_trn.rs` proves:

- the first 288 symbols of both polynomials equal spec.md §4.3's vectors;
- the transmitter's TRN symbols equal `TrnSequence`;
- the Conexant check, with a small front end of your own in the test file
  (mixer, `dsp::fir_lowpass`, T/2 sampling, S→S̄ found by correlation, turn
  estimated, `dsp::least_squares`): on each of the two TRN segments of
  `tests/vectors/v32bis-14400.wav` (4.25–7.00 s and 7.75–10.50 s; contract.md
  §5), the Table 5 targets fit ≥ 25 dB, and the swapped ones are at least
  6 dB worse on symbols ≥ 256.

Every existing test passes.

**E — the echo canceller follows drift.**

Read design.md §5 and contract.md §1.3 and §7 F/H. In `dsp::echo`, add a
drift-following mode to `EchoCanceller`, reached by path (`dsp::echo::…`, not
through `lib.rs`), with exactly the constants of §5.1: `follow_drift`,
`drift_ppm`, `jumps`, `is_holding`. With it off, the output is bit-identical
to today; prove it with a digest test.

In `crates/datapump/src/v32/startup.rs`, edit **only** `Modem`: switch it on
when the first own TRN ends, keep NLMS as today, and add
`echo_drift_ppm`/`echo_jumps`.

`crates/dsp/tests/echo_drift.rs` uses a band-limited random reference plus an
equal-power far signal on a 700-sample crossing. It requires:

- residual echo ≤ −28 dB relative to the far end at 0, 5, 20 and 100 ppm from
  3 s after switching on, and throughout 60 s;
- after a one-sample drop, a one-sample repeat or a 160-sample gap, ≤ −23 dB
  within 100 ms;
- no accepted jump on a clean line in 60 s.

`crates/datapump/tests/v32_cable.rs` runs two `Modem`s on a cable (sum ×0.45,
700 samples) with today's receiver:

- 14 400 held with 0 retrains for 60 s at 5, 20 and 100 ppm (contract.md §7 F
  fails all three today);
- slips are the receiver's problem and are not asserted here.

Every existing test passes.

**F — the acceptance harness and the before numbers.**

Read design.md §9 and contract.md §4. Write
`crates/datapump/tests/v32_acceptance.rs` using only the public API of
`v32::startup::Modem`:

- the line model and the BER and block measurements of §9.1–9.2;
- every test in §9.2 with its exact pass criterion.

Write `crates/modem/tests/v32_cable_faults.rs` the same way. Copy lock_sweep's
`Shifter` and V.34's slip helper; they are this project's own code.

Also:

- run everything on today's code;
- mark each test that fails with `#[ignore = "V.32 rebuild: enabled by package D"]`;
- add one ignored `before_and_after_table` test that runs the §9.2 lines and
  prints rate, retrains, block-error share and reception per row.

Report the before table in your final message. Keep the suites under 120 s in
release. No source files outside your two tests.

**D — the V.32 receiver on the core.**

Read design.md in full, contract.md in full, and core.md §5 and §7.

Write `crates/datapump/src/v32/receiver.rs`, the driver (§2.6, §2.7, §3):

- the stages and the cue API (`idle`, `hunt`), the TRN `Training` from
  `TrnSequence`, the fallbacks of §3.2;
- V.32's constellation tables and tentative decisions (add
  `trellis::Decoder::tentative()` additively);
- bits by Table 1, Table 3 and the decoder;
- `residual_error`, the carrier detector, the blind start, and the
  diagnostics.

Replace the old receiver in `v32.rs`. Keep `Scrambler`, `Mode`, the
transmitter and the private helpers the unit tests use.

In `startup.rs`:

- add the cues of §3.2;
- make the retrain rule skip `stop_offering` when `lost_for ≥ 1200` (§4.6);
- stop `Modem` calling `set_adapting`.

Order of work:

1. The bare receiver: `v32_loopback` passes, including 9600T blind. Measure
   this first; it is the top risk.
2. Training on our own TRN, and the Conexant vector (§9.3).
3. The whole start-up: `v32_startup`, `v32_call`, the modem `call` and
   `soundcard_loop` tests.
4. Remove the ignores from F's tests and make them pass.

Do not edit `v34/`, `v90/`, `dsp::Equalizer` or `dsp::Gardner`.

**G — verification and live readiness.**

Read design.md §9–§13. First copy `docs/design/slow-modes/before.md` to
`before-d7b914b.md`.

Add `crates/datapump/tests/v32_capture_check.rs` (ignored, env vars as
`v32_replay`). For each second, print stage, rate, rotation, SNR, slips,
drift, gain, S frequency and trained SNR. Run it on the two live captures and
the vector, against §9.5.

Add a V.32-only lock_sweep test and fill in §9.4. Extend `modem-loop` to print
retrains, receiver slips, drift, echo jumps, the rate and the reception.

Tune constants only where a measurement says to, and record every change.

Write `docs/design/v32-rebuild/results.md` with the before and after columns
of §1, §9.2, §9.4 and §9.5, and the list for Rory's first live call (§13).

---

## 11. Rejected alternatives

| alternative | why not |
|---|---|
| Retune today's receiver (the modulus, handover, loop bandwidth) | contract.md §7 A/B: the arrival ladder is carrier pull-in; a slip has no rewind; the 27 dB interpolation ceiling stays |
| Instantiate V.34's `Receiver` for V.32 | needs edits to `v34/receiver.rs`, which is live-proven and off limits (core.md §8 c) |
| A separate V.32 copy sharing nothing | two copies of about 1000 lines to fix twice (core.md §8 b); the core is shared but V.34 is not moved yet |
| CMA or another blind equaliser in calls | the start-up always delivers S, S̄ and a known TRN; blind stays for bare tests only |
| Frequency from AA/AC tones | earlier than TRN and separated from it by silence, and harder to measure through our own echo; S is right before TRN in every start-up and retrain |
| Symbol-exact rate switch at E | B1 carries no data and the gate refuses the 0–1 mis-sliced symbols |
| Changing the retrain threshold to per-rate SNR | pinned by `a_rate_that_cannot_be_read_is_given_up` and `stop_offering`'s tuning; a follow-up for Rory |
| Canceller NLMS in data mode | [P3]: −24 dB at best and does not follow 20 ppm |
| Retiming the reference by the receiver's timing drift | right only on the cable, where both directions share clocks; the canceller measures its own |
| Holding the receiver while the canceller holds | a far-end level change would hold both; independent gates suffice |
| A faithful replay using channel 1 as the canceller's reference | [P4]: no echo on the VoIP captures to cancel; not needed now |

## 12. Risks

1. **Blind 9600T** (the bare contract test) on the resync-as-acquisition path.
   A and D prove it first. The fallback is a longer search window and the z⁴
   turn, never a CMA stage in calls.
2. **Zero-delay Viterbi decisions** can come in bursts after resyncs. They
   are mitigated by 16 symbols of nearest-point decisions and by the gate. If
   bias shows in the carrier loop, fall back to the Y0 half, which costs 2×
   in SER.
3. **The discriminator on real pulses.** Its thresholds come from V.32 §2.2's
   2–7 dB. The Conexant vector is the only real S in the tree.
4. **The canceller's jump search** could lock onto a wrong shift on a line
   with several echoes. The 0.8 threshold and the re-baselining limit that,
   and it is watched with `echo_jumps` live.
5. **CPU in `modem-loop`'s real-time loop**: resync bursts of a few ms, and
   about 1 ms for the jump search. G checks for underruns caused by our own
   processing.
6. **The retrain rule** gives away 0.6–1.5 dB at 7200T, 12 000 and 14 400.
   Calls step down a little earlier than they must.
7. **V.34 and the core diverge** until V.34 is moved. That is bounded by the
   default-off options; the later move needs a bit-exact golden test first
   (core.md §8).
8. **N6** (tap walk over hours) is unproven either way. The anchor is
   insurance, and the 10-minute test is the check.

## 13. What cannot be verified without Rory, and what to check on the first live V.32bis call

Nothing here reaches a real far end except Rory's live calls. The simulation
cannot say:

- how a real V.32bis far end behaves given our corrected TRN and level;
- the real softphone's concealment and gain behaviour during V.32's start-up;
- the cable's real drop and underrun pattern.

**First, the cable.** Run `modem-loop --carrier V32 --seconds 120` on the
cable before any live call.

- **Expect**: CONNECT 14 400, the greeting through, `retrains = 0`.
- **Report**: `samples lost coming in` and `underruns`, alongside receiver
  slips, echo jumps and drift. Every counted drop should appear as one slip
  and one echo jump, with no retrain.

**Then the first live call** (`AT+MS=V32B`, to a V.32bis far end), with the
capture kept. From the capture and G's `v32_capture_check`:

1. The pump phases through the start-up. On any stall, check the pump phase
   first (memory: the V.32 retrain stall was the start-up).
2. Both trainings on the calling end:
   - the trained SNR, which should be well above 20 dB on VoIP;
   - the S frequency, which should be under 0.5 Hz on VoIP;
   - whether the first try fitted or the retry was needed. A retry points to
     a slip inside TRN.
3. Whether the far end's R3 offers 14 400. That is the far end's own view of
   our signal, now with a correct TRN.
4. Whether the call holds its rate for 60 s with 0 retrains. Check these
   medians per second over the call (memory: stats must be medians):
   - SNR;
   - rotation drift (the 9600T capture walked 20° in 0.8 s);
   - drift in ppm (expect tens);
   - `gain_db`, which moves if the softphone's gain control is working.
5. Every receiver slip lined up against the softphone's jitter slips (a
   20.0 ms cadence every few seconds). Each should cost only data. Count the
   V.42 good and bad frames around them.
6. Whether any retrain happened, and which kind:
   - a loss (no `stop_offering`);
   - unreadable reception;
   - the far end's tone.
7. `echo_return_loss_now` and `echo_jumps` on VoIP, expected about 0 by [P4].
   A jump on VoIP would mean the far network does return an echo.
