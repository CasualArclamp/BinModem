# V.34's receiver, as the reference the older modes are judged against

All paths are relative to `F:\dialupmodem2`. Line numbers are from the tree at
`48b9087` (branch `pr-1-deps`). Every V.34 unit test passes as this was written:
`cargo test -p datapump --lib v34::` — 107 passed, 0 failed.

The point of this document is not that V.34 is good. It is to write down, with
numbers, *what mechanisms exist here that the older modes do not have*, so that
"the 33.6 QAM modes are locked in place, the older ones use old code" becomes a
list of specific missing parts rather than a feeling.

The one-line summary of the difference: **V.34 does not use `dsp::Equalizer` at
all.** `crates/dsp/src/equalizer.rs` — the symbol-spaced, blind-CMA-then-decision-
directed LMS filter — is used only by `v22bis.rs`, `v27ter.rs`, `v29.rs` and
`v32.rs` (confirmed by grep: those four files and `dsp/src/lib.rs` are its only
importers). V.34 has its own front end, its own equaliser, its own carrier and
timing loops, a loss detector, a rewind buffer and two resynchronisers, all in
`crates/datapump/src/v34/receiver.rs` (1676 lines). Nothing in the older modes
has an equivalent of the last four.

---

## 1. Signal path, in order

`crates/datapump/src/v34/receiver.rs`

| Stage | Where | What |
|---|---|---|
| Down-mix | `feed` 642-651 | multiply by `2·e^{-j2πφ}`, `φ += carrier/fs` each sample (`step` set once at 478 and **never changed**) |
| History | 647-651 | `HISTORY = 16_384` complex samples (`98`) — 1.024 s at 16 kHz |
| Interpolate + low-pass | `interpolate` 662-678 | 64-tap windowed sinc (`FILTER_TAPS` 57), 256 fractional phases (`FILTER_PHASES` 58), Kaiser β = 8 (`465`), cutoff `0.5·baud·1.1 + 300` capped at `0.45·fs` (456) |
| Half-symbol sampling | `feed` 653-657 | `due += half·(1 + drift)`; `half = fs/baud/2` (483) |
| Ring of half samples | `on_half` 684-690 | `KEPT = 4096` (65) — 2048 symbols, 0.597 s at 3429 baud |
| Equalise | `symbol` 910-913 via `apply` 1271 | 31 complex taps at T/2 (`REACH = 15`, 62) |
| Derotate | 921-922 | `z = y · e^{-j·rotation}` |
| Slice | 923 | `Slicer::Points(Size)` or `Slicer::Grid{scale, limit}` (143-150) |
| Track | 947-969 | NLMS, carrier, timing — all gated on one test |

Production runs at `fs = 16_000` (`crates/gui/src/main.rs:56`).

### Numbers that fall out of the front end

At 3429 baud the carrier is `4/7 · S = 1959.18 Hz` for **both** carrier choices
(`v34/probe.rs:248-257`, Table 2/V.34), and the cutoff is
`0.5·3428.57·1.1 + 300 = 2185.7 Hz`. Measured response of the 64-tap row at
16 kHz (recomputed from the code's own formula):

```
   0 Hz   -0.00 dB      2185.7 Hz  -6.02 dB
1500 Hz    0.00 dB      2400.0 Hz -15.34 dB
1885.7 Hz -0.80 dB      2600.0 Hz -31.39 dB
2000 Hz   -2.00 dB      3000.0 Hz -85.50 dB
```

The band edge (`0.55·S = 1885.7 Hz`) is only 0.8 dB down, which is what you want,
but see §7.1 for what that costs.

The fractional-delay table quantises the sampling instant to 1/256 of a sample =
1/1194 of a symbol at 3429/16 kHz. That is 0.0008 of a half symbol of jitter,
three orders below anything the timing loop cares about. Not a concern.

---

## 2. Acquisition: there is no blind stage, and that is the whole design

### 2.1 Finding S

`Hunt` (285-364). S alternates two points 90° apart, so it repeats every two
symbols. `Hunt::feed` correlates each half sample against the one four halves
(two symbols) earlier, sums eight such correlations, and calls it S when
`Re(Σc) > 0.7·Σp` **and** the mean power exceeds `AUDIBLE = 4e-4` (342) — 37 dB
below nominal. It has to hold for `HELD = 40` half samples = 20 symbols (301).

Then it *learns a template*: the four half samples of one S period, EMA'd at 0.2
while hunting (346) and 0.1 once armed (318). S-bar is found as the template
turned round: `ratio < -0.5` over four consecutive matches (313), reported as
`Reversal{at: index - 3}` (315).

Why a template and not more correlation: the S→S-bar change is not sharp. A
far-end pulse shaper plus this end's filter smears it over a symbol or two, and
over a VoIP call the first live far end smeared it across the whole two symbols
the correlation looks back (comment 276-283). A receiver that looks for the
reversal by correlation alone misses it or finds it two symbols late; two symbols
late is 240 samples of equaliser window misplacement at 3429/16 kHz, which the
`SEARCH = ±8` half-sample scan then has to absorb.

### 2.2 Training: least squares on a known sequence

`solve_known` (786-845). This is the mechanism the older modes have no analogue
of at all.

What follows S-bar is *known symbol for symbol*: PP is fixed by equation 10-1
(rendered from `docs/specs/T-REC-V.34-199802-I.pdf` page 33, PDF index 38 —
`PP(i) = e^{jπ(kI+4)/6}` if `k mod 3 = 1`, else `e^{jπkI/6}`; `v34/signals.rs:91-96`
reproduces it exactly), and TRN is scrambled ones from a scrambler initialised to
zero (10.1.3.8, same page), so it is equally known. The code therefore:

1. Builds `targets` for the whole window (`sequence` 1228-1238).
2. For each alignment `delta ∈ [-SEARCH, +SEARCH] = ±8` half samples (81, 809),
   collects `search_to - search_from` rows of 31 half samples each and solves
   `least_squares` (819 → `crates/dsp/src/complex.rs:142-163`, normal equations +
   Cholesky at `167-210`) with a ridge of `1e-3 · energy/31` on the diagonal
   (`ridge` 1280-1283).
3. Keeps the alignment with the lowest residual (821-824).
4. Estimates the carrier's turn per symbol from the residual phase of the early
   half of the window against the late half: `turn = arg(late · conj(early))/middle`
   (831-835).
5. Re-solves over the **full** window with the targets pre-rotated by `turn·k`
   (840-842), so the taps come out as a static filter and the rotation is left for
   the carrier loop to undo.

Windows (`windows` 1217-1225), all in symbols of the reference:

| Case | alignment search | full solve |
|---|---|---|
| Phase 3 (`PpThenTrn`) | PP 48…288 | 48…352 (288 of PP + 64 of TRN) |
| Phase 3 retry | TRN 256…512 | same |
| Phase 4 (`Trn(size)`) | 16…256 | 16…384 |
| Phase 4 retry | 320…512 | same |

`PP_SKIPPED = 48` (68) leaves the line's memory of S-bar out; `TRN_SKIPPED = 16`
(77) does the same in phase 4.

**What it solves.** A 31-tap T/2 MMSE equaliser, gain and all, in one shot from
304 symbols of known data. No convergence transient, no eye to open, no blind
criterion, no decision-directed hand-off. Compare `dsp::Equalizer` (`equalizer.rs:
109-147`): Godard CMA at `blind_step = 2e-3` until the running mean decision error
falls under 0.25 (121), then decision-directed LMS at `tracking_step = 4e-3`. A
2e-3 LMS step over 15 taps needs on the order of a thousand symbols to converge
and can converge to a rotated or delayed solution, and the CMA stage carries no
phase information at all.

**Acceptance.** `KNOWN_ENOUGH = 12.0` dB (90); `finish_training` 743 computes
`-10·log10(mse)` and requires ≥ 12 dB. Failing that, and only on the first try,
it re-arms with `second: true` (749-755), which searches `WIDE_SEARCH = ±200`
half samples (86) — wide enough for a 20 ms jitter-buffer slip (69 symbols =
138 halves) to have landed inside the first window. Because ±200 alignments × a
31×31 Cholesky is too much work, the wide search first scores each alignment by
raw correlation of samples against targets and solves only at the best ±3
(789-810). A second failure, or a solution under 6 dB (767), gives `Heard::Untrained`
and `training.rs:1232` fails the call.

### 2.3 `reacquire`: the fallback for a far end whose TRN is not zero-started

`reacquire` (857-906). Keeps the taps the last training produced (the line has not
changed), and searches only alignment (±8) and rotation. The rotation comes from
the fourth power of the outputs — `arg(Σ y⁴ ) - π)/4` (872-873), which fixes it to
within a quarter turn — and then each of the four quarters is tried by running the
decisions through `signals::Reader::trn` and counting how many descramble to ones
(877-889). Accepted only if > 95 % are ones (905). This exists because a far end's
TRN may carry its scrambler on from J rather than restart it; 10.1.3.8 says it
restarts, but the code does not bet the call on it.

---

## 3. Tracking, with the actual loop constants

Everything below is `symbol()` (909-991), called once per symbol from `on_half`
(711-724) and from `finish_training` (779-782).

### 3.1 The gate — one decision governs all three loops

```rust
let doubtful = 0.25 * self.slicer.min_distance_squared();   // 928
...
if self.lost.is_none() && squared < doubtful {              // 947
```

`doubtful` is a quarter of the squared distance to the nearest other point, i.e.
**half the distance** to the decision boundary. Above it a decision is more likely
wrong than right, so the equaliser, the carrier loop, the timing loop and
`settled` are all frozen for that symbol. This single gate is why V.34 survives a
burst of wrong decisions where the old modes learn from them. It is also the
source of two of the weaknesses in §7.

For the grid slicer, `min_distance_squared = (2/scale)²` (169), so `doubtful`
corresponds to exactly **one grid unit** of error whatever the rate.

### 3.2 Equaliser: 31 taps at T/2, normalised LMS

```rust
const STEP: f64 = 0.02;                                     // 127
let energy: f64 = row.iter().map(|x| x.norm_sqr()).sum() + 1e-9;
let back = e * spin.conj() * (STEP / energy);               // 950-951
for (tap, x) in self.taps.iter_mut().zip(row) { *tap -= back * x.conj(); }
```

- **Fractionally spaced, T/2.** 31 taps = 15.5 symbols of line memory, sampling
  twice a symbol. This is why the timing loop has almost nothing to do: a T/2
  equaliser is indifferent to where inside the symbol the sampling falls, so
  timing only has to stop drift, not find the eye (comment 31-33).
- **Normalised.** Dividing by the row energy makes the convergence time
  independent of level: μ = 0.02 gives a misadjustment time constant of about
  1/μ = 50 adapting symbols ≈ 15 ms at 3429 baud.
- **Adapts in its own frame.** `e * spin.conj()` (951) rotates the error back
  *before* the carrier is removed, so the taps stay a static channel inverse and
  do not have to chase the carrier. The old modes' equaliser adapts on the
  post-derotation error and therefore fights the carrier loop for the same degree
  of freedom.
- **Never frozen by state**, only by the `doubtful` gate and by `lost`.
- **Initial value:** `taps[REACH] = 1` (473-474), used only until the first
  training replaces the whole vector.

### 3.3 Carrier: second order, critically damped, ~17 ms

```rust
const PHASE_GAIN: f64 = 0.04;        // Kp, 130
const FREQUENCY_GAIN: f64 = 4e-4;    // Ki, 131
let power = target.norm_sqr().max(0.1);
let wrong = (z * target.conj()).im / power;   // 956
self.turn += FREQUENCY_GAIN * wrong;          // 957
self.rotation += PHASE_GAIN * wrong;          // 958
```

with `rotation += turn` every symbol at 946, *before* the gate — so the frequency
estimate keeps running the phase forward even while everything else is frozen.

The detector `Im(z·conj(target))/|target|²` is `sin(φ)` normalised, so its gain is
1 rad/rad for any target with `|target|² ≥ 0.1`.

Closed loop, per symbol: `z² − (2 − Kp − Ki)z + (1 − Kp) = 0` →
`z² − 1.9596 z + 0.96 = 0`, roots 0.98264 and 0.97697 (real, ζ ≈ 1.0).
- natural frequency `ωn = √Ki = 0.02 rad/symbol` = **10.9 Hz** at 3429 baud
- one-sided noise bandwidth ≈ `(ωn/2)(ζ + 1/4ζ)` = **6.8 Hz**
- slowest mode settles in `1/(1−0.98264)` = **58 symbols = 17 ms**

Because it is second order, **a constant carrier-frequency offset leaves no
standing phase error**. This is the concrete answer to "what does a receiver
without it suffer": the first-order part alone (Kp = 0.04) would leave a standing
error of `(2π·Δf/baud)/Kp` = **2.6° per hertz** of offset at 3429 baud. At a
typical 3–5 Hz network offset that is 8–13° of static rotation, on a 1664-point
constellation whose nearest-neighbour angle at the mean radius (24.1 grid units)
is 4.8°. A first-order carrier loop cannot run V.34 data mode at all.

A phase hit is survived by the loss detector (§4), not by the loop: 0.04 rad/rad
pulls a 90° step in about 58 symbols, and would learn garbage all the way.

Pull-in rate of the integrator: `Ki · max|wrong| · baud/2π` = **0.22 Hz per symbol**,
so a 5 Hz offset is acquired in ~25 symbols once decisions are usable — but
training already hands it `turn` (`solution.turn`, 762) measured over the window,
so the loop starts near zero error rather than pulling in.

### 3.4 Timing: second order, exactly critically damped, ~58 ms

```rust
const TIMING_GAIN: f64 = 0.01;    // Kp, 138
const DRIFT_GAIN: f64 = 1.25e-5;  // Ki, 139
let rate = rate * spin;
self.slope += 0.01 * (rate.norm_sqr() - self.slope);                 // 963
let late = ((e * rate.conj()).re / self.slope.max(1e-9)).clamp(-0.5, 0.5);  // 964
self.due   -= TIMING_GAIN * late * self.half;                        // 965
self.timed -= TIMING_GAIN * late * self.half;                        // 966
self.drift  = (self.drift - DRIFT_GAIN * late).clamp(-0.001, 0.001); // 967
```

The **error detector** is the projection of the decision error on the output's
rate of change: `rate` (916-920) is the same 31 taps applied to central
differences of the half samples, `(wide[i+2] − wide[i])/2`, i.e. d(output)/d(half
symbol). Normalising by `slope` (an EMA of `|rate|²`, 100-symbol time constant)
makes `late` a unit-gain estimate of lateness **in half symbols**, clamped to ±0.5.
This is a maximum-likelihood-style detector, not a Gardner or Mueller–Müller
detector, and it costs one extra 31-tap dot product per symbol.

Loop, in half-symbols of phase: `θ[n+1] = θ[n] − Kp·θ[n] + 2·d[n]`,
`d[n+1] = d[n] − Ki·θ[n]` (the factor 2 because `due` advances twice per symbol).
Characteristic: `z² − (2 − Kp)z + (1 − Kp + 2Ki) = 0` → `z² − 1.99 z + 0.990025 = 0`.
Discriminant `1.99² − 4·0.990025 = 0` **exactly** — `Kp² = 8·Ki` to the digit. The
loop is exactly critically damped, double root at z = 0.995:
- time constant `1/(1−0.995)` = **200 symbols = 58 ms** at 3429 baud
- `ωn = √(2Ki) = 0.005 rad/symbol` ≈ 2.7 Hz

Deliberately about 4× slower than the carrier loop and 4× *faster* than the
equaliser's 50-symbol NLMS, so the equaliser is never the thing that follows
clock drift (comment 133-139). `drift` is clamped to ±0.001 = **±1000 ppm**;
Recommendation 5.2 (page 4, PDF index 9) requires only ±0.01 % = ±100 ppm per end,
so ±200 ppm between two conforming modems. The extra order of magnitude is for a
sound card against a VoIP far end. The test at `receiver.rs:1420-1437` proves
−114, +114 and +200 ppm all track to better than 38 dB.

What a receiver without a timing *integrator* suffers: at 114 ppm the far clock
moves a third of a symbol a second. With only the proportional path, the standing
error is `2·drift/Kp` = 0.0228 half symbols — small — but there is no state to hold
it, so the equaliser absorbs it instead and walks off its own end in seconds
(comment 26-29). That is exactly what a symbol-spaced LMS equaliser with no timing
loop does.

### 3.5 Level and AGC

**There is no AGC anywhere in V.34.** Grep for `agc`/`Agc` in `v34/` and `dsp/`
returns nothing. `self.power` (`on_half` 681) is an EMA at 0.002 per half sample —
500 halves = 250 symbols = 73 ms — and it is used only as a carrier-present test
(`level()` 607, read at `training.rs:1024` and `1466` against `1e-4`).

All gain lives in the equaliser taps: set exactly by the least-squares solve
(761), tracked by NLMS (950-954), and corrected in one step by `resync_dense`
(1098-1101, `for tap in &mut self.taps { *tap = tap.scale(gain) }`).

This works because the training solve gets gain right to begin with, and NLMS
follows slow changes. It fails in exactly one way, quantified in §7.2.

### 3.6 Slicer, decisions, and the trellis

`Slicer` (143-216) has two modes:

- `Points(Size)` — four or sixteen points (`decide` 1252-1256, `signals::decide`).
  Used for S, PP, TRN, J, MP, E.
- `Grid { scale, limit }` — nearest odd integer on the grid out to `limit`
  (157-161). Used in data mode, set from `Decoder::grid_scale()`/`extent()` at
  `training.rs:1393` and `987`.

The grid slicer exists because the trellis decoder's real decisions arrive
`DEPTH = 40` 4-D symbols later (`data.rs:40, 637-639`) — far too late for a loop
that must close every symbol. So the loops run on raw nearest-grid-point
decisions while the Viterbi decoder runs behind them on the same stream. That
separation is the reason data mode can track at all.

`min_distance_squared`, `lost_level`, `lost_threshold`, `found_level` and `window`
are all slicer-dependent (166-215) — the code knows that a 1664-point grid and a
four-point constellation need different loss arithmetic, and says so at 173-184.

### 3.7 Precoding and non-linear coding

- The **decoder** replays the precoder exactly (`data.rs:664-677`, `Precoder`
  126-162, in 1/128-grid-unit integer arithmetic) and un-warps 9.7's stretch by
  fixed-point iteration (`to_grid` 437-448).
- The **tracking slicer does neither.** It sees the raw equaliser output.
- This is harmless today only because `make_mp` (`training.rs:1470-1497`) always
  sends `non_linear: false`, `expanded_shaping: false`, `precoding: None`,
  `trellis: States16`, `auxiliary: false`. `receive_params` (1421-1432) reads
  those from *our* MP, so our receive path is always linear, unshaped, Type 0.
  The far end's requests only affect `transmit_params` (1405-1420).

See §7.3 for what happens the day our MP asks for what the live ISP far end asks
of us.

---

## 4. When things go wrong

### 4.1 Loss detection

```rust
self.recent.push_back(squared);                       // 930  (unconditional)
while self.recent.len() > judged { pop_front }        // 931-933
let recent = mean(self.recent);
None if self.recent.len() == judged
     && recent > self.slicer.lost_threshold(self.settled)  // 936
```

`judged` is 8 symbols for `Points`, 32 for `Grid` (210-215).

```rust
fn lost_level(self) -> f64 {                          // 180-185
    Points => 0.25 * min_distance_squared,
    Grid   => min_distance_squared / 12.0,
}
fn lost_threshold(self, settled) -> f64 {             // 191-196
    Points => (8.0 * settled).max(lost_level),
    Grid   => (2.0 * settled).max(lost_level),
}
```

The `Grid` floor is deliberately **half** the level pure garbage reads: a sample
landing uniformly within ±1 grid unit of some point has mean squared error
`2/3` grid units² = `min_distance_squared/6`, so `/12` is 3 dB of margin. At
33 600 bit/s as this code actually negotiates it (`expanded_shaping: false`, so
L = 1408 and scale = 26.975 grid units per unit-power symbol) that floor is
`4.58e-4` — **33.4 dB** — which a locked signal on a 38 dB line clears
comfortably and garbage, at 30.4 dB, does not.

The `8×`/`2×` multipliers on `settled` are what make this work on a poor line:
`settled` is an EMA at 0.01 (968, 100 symbols) of the *passing* symbols' error, so
the threshold is always relative to what this particular line actually reads.

### 4.2 Rewind — the mechanism nothing else in this codebase has

```rust
self.lost = Some(0);
self.rewind(judged as u64 + EARLIER_EVERY);           // 940-941
```

`Loops` (440-451) is a full snapshot of taps, rotation, turn, drift, slope, timed,
settled and error. One is kept every `EARLIER_EVERY = 16` symbols, `EARLIER_KEPT = 24`
of them (112-113, 971-986) — **384 symbols = 112 ms** of history at 3429 baud.
`rewind` (997-1013) finds the newest snapshot at least `back` symbols old, restores
everything, carries the phase forward by `turn·elapsed`, and puts `due` back by
`loops.timed − self.timed`.

The problem it solves: by the time the error average has risen far enough to
declare a loss, every loop has already spent `judged` symbols learning from wrong
decisions. Without rewind the receiver resynchronises onto a corrupted equaliser.
`training.rs:984` uses the same call with a much longer reach when it discovers
that the far end went into data mode and its E was lost.

### 4.3 Resync on four or sixteen points

`resync` (1148-1211), fired from `on_half` 718-723 when `lost ≥ window/2` and
`lost % RESYNC_EVERY == 0` (`RESYNC_EVERY = 32`, 103) — so first at 32 symbols lost
for `Points`, 96 for `Grid`.

It re-reads `RESYNC_WINDOW = 48` symbols (102) straight from the raw mixed-down
history at `2·RESYNC_STEPS = 16` shifts spanning ±1 half symbol (106, 1167-1168),
runs the frozen taps over each reading, gets the rotation from the fourth power
to within a quarter turn (1174-1180) — choosing the quarter nearest the rotation
before, which the differential coding of J/MP/E does not mind — then refines it
with three decision-directed rounds (1183-1190) and scores the mean squared error.

Acceptance (1199-1206) is two tests, and the second is the clever one:

```rust
let wrong = readings[readings.len()/2];               // median of all 16 shifts
if mse > self.slicer.found_level(self.settled) || mse > 0.6 * wrong { return; }
```

The shifts that are *wrong* read as whatever the signal reads out of step. A real
find must be both absolutely good and **better than 0.6× the median of its own
competitors**. That is what stops a resync from firing on noise, and it is a
self-calibrating threshold, not a constant.

### 4.4 Resync on a dense grid

`resync_dense` (1027-1103) exists because none of the above holds on 832 points:
the fourth power of a constellation shaped round is near nought, and a reading a
sixty-fourth of a symbol out, or a degree turned, or with the gain 1 % off, reads
as noise. The comment at 1020-1026 records the live call this came from: the
equaliser trained on sixteen points was **0.9 % high** for the same line's data.

So it searches three axes:
- timing: `2·DENSE_STEPS = 32` positions across ±1 half symbol (122, 1071-1072),
  then `DENSE_FINE = 64`ths of a half symbol either side of the best (124, 1082-1090)
- phase: a quarter turn in `DENSE_DEGREES = 1.5°` steps (123, 1061-1066)
- gain **and** phase together: four rounds of complex least squares against the
  decisions (`fit` 1049-1059)

judged over `DENSE_WINDOW = 64` symbols (117), all of them after the jump. Same
two-part acceptance (1094). The fitted gain goes into the taps (1098-1101) and the
fitted phase into the carrier (1102 → `take_up` 1132).

### 4.5 Taking up again

`take_up` (1108-1136) re-reads every stored half sample from `from` onwards at the
new offset, truncates where the raw history runs out so the tail is remade when
samples arrive (1124-1128), sets the rotation, clears `lost`, clears `recent`, and
increments `slips` (1135). `training.rs:1251-1256` watches `slips()` and, if a
decoder or acquirer is running, restarts the frame search — because a slip loses
or repeats symbols and with them the place in the mapping frames.

### 4.6 Retrain and renegotiation

- `RetrainWatch` (`training.rs:234-268`) listens on the **raw line**, not the
  demodulator, for the far end's Tone A (2400 Hz, from the answerer) or Tone B
  (1200 Hz, from the caller), requiring the tone to stand `RETRAIN_TONE_HELD = 0.055 s`
  (214; 11.5.1.2/11.5.2.2 say "more than 50 ms") **and** to be `RETRAIN_TONE_CLEAR = 6×`
  above what sits 150 Hz either side (220). The side-band test is what stops data
  or a four-point renegotiation signal being read as a retrain: a pure tone puts
  everything at one frequency, a data signal fills the band.
- A renegotiation this end began that goes unanswered turns into a full retrain
  rather than a failure (`deadline_reached` 1185-1191), per 11.6.2.
- Data-mode recovery paths: decoder path cost above `STRAYED_COST = 1.0` grid
  units² for `STRAYED_SYMBOLS = 1000` 2-D symbols ≈ 0.29 s (199-203, 1285-1290)
  → `search()`; a slip → `search()`; `unlike > UNLIKE_ERROR = 0.02` for
  `UNLIKE_SYMBOLS = 150` symbols while waiting for E (209-210, 1014-1030) →
  `search()`.

---

## 5. The comparison, in one table

| Mechanism | V.34 | `dsp::Equalizer` users (V.22bis, V.27ter, V.29, V.32) |
|---|---|---|
| Equaliser spacing | T/2, 31 taps (`receiver.rs:62`) | T, symbol-spaced (`equalizer.rs:42-55`) |
| Initial convergence | one-shot least squares on the known sequence (`786-845`) | blind CMA at μ = 2e-3 then DD at 4e-3 (`equalizer.rs:49-50, 121-133`) |
| Adaptation frame | pre-derotation (`951`) | post-derotation |
| Step normalisation | NLMS, divided by row energy (`950`) | plain LMS, fixed step |
| Carrier loop | second order, Kp 0.04 / Ki 4e-4, ζ ≈ 1, BL 6.8 Hz (`130-131`) | per-mode, see each file |
| Timing loop | second order, exactly critically damped, 200-symbol (`138-139`) | per-mode |
| Loss detection | slicer-aware, relative to `settled` (`180-196`) | none |
| Rewind after bad decisions | 384 symbols of loop snapshots (`112-113, 997-1013`) | none |
| Resync after a jump | two resynchronisers, self-calibrating acceptance (`1027-1211`) | none |
| Runaway protection | ridge in the solve (`1280-1283`), `doubtful` gate (`928, 947`) | tap-energy trip at 1e4 then full reset (`equalizer.rs:105, 143-146`) |
| Slip counting | `slips()` (`612`), drives frame re-acquisition | none |

The four rows with "none" on the right are the answer to the question that
started this work.

---

## 6. What the Recommendation offers or requires that the code does not do

Read from the rendered PDF, not the text extract.

1. **Precoding (9.6.2) is never requested.** `make_mp` sends `precoding: None`
   (`training.rs:1495`), i.e. always a Type 0 MP. 10.1.3.9 (page 33, PDF index 38)
   allows either type — "Either type (Type 0 or Type 1) of MP sequence may be
   sent during start-up, retrain, or rate renegotiation" — so this is legal, but
   it means the far end never pre-compensates our channel and our single linear
   T/2 equaliser must invert it, with the noise enhancement that implies on a
   line with a sharp roll-off. The live ISP far end (memory: `live-v34-isp-far-end`)
   asks *us* for precoding and non-linear coding, and we honour it on transmit
   (`transmit_params` 1405-1420) while asking for nothing ourselves.
2. **Non-linear encoding (9.7) is never requested**, and the tracking slicer could
   not handle it if it were — see §7.3. Θ = 0.3125 and Φ = 1 + ζ/6 + ζ²/120 are
   equations 9-33 to 9-35, rendered from page 24 (PDF index 29).
3. **Only the 16-state code is offered.** `Trellis::States16` at
   `training.rs:1489`, although `code_of` (1670-1676) and `Code::States64` are
   implemented. 9.6.3.2 (page 24) says "The encoder shall be selected by the
   receiving modem during Phase 4"; we always select the weakest of the three.
   The 32- and 64-state codes are worth roughly 0.5 and 0.8 dB.
4. **Expanded shaping is never requested** (`expanded_shaping: false`, 1491),
   costing the shaping gain the shell mapper is there for.
5. **Neither a retrain nor a rate renegotiation is ever initiated from inside the
   modem.** 11.6 says the procedure "can be initiated at any time during data mode
   to change to a new data signalling rate. This procedure can also be used to
   resynchronize the receiver without going through a complete retrain."
   `start_retrain` (`training.rs:1201-1207`) and `renegotiate` (`900-911`) are
   public API only, reachable from `startup.rs:102-103, 135` and thence from the
   GUI or from V.90's analogue side (`v90/startup.rs:208, 432, 443`). A V.34 call
   whose line degrades, or whose receiver has lost its place permanently, has no
   route back that the modem takes by itself. This is the single largest gap.
6. **Auxiliary channel (5.1, 8.3)** declined (`auxiliary: false`, 1488), though
   `Framing` supports it.
7. Half-duplex (10.2, clause 12) and Annex A's modem control channel are not
   implemented at all. Out of scope for this work.

Not a gap: 6.6.1 says Circuit 109 thresholds and response times "are not
applicable in duplex mode", so the `level() > 1e-4` carrier test has no timing
requirement to meet.

---

## 7. Weaknesses

### 7.1 The down-mixer's image sits 147 Hz from the signal band, and only the equaliser removes it

`feed` (642-646) multiplies a real sample by `2·e^{-j2πφ}`. That puts the
conjugate image at `−2·f_c`, spanning `[−2f_c − 0.55S, −2f_c + 0.55S]`. The guard
between the image's upper edge and the signal's lower edge is `2f_c − 1.1S`:

| rate / carrier | guard |
|---|---|
| 3200, low (1828.6 Hz) | **137 Hz** |
| 3429, either (1959.2 Hz) | **147 Hz** |
| 2743, low (1645.7 Hz) | 274 Hz |
| 2800, low (1680 Hz) | 280 Hz |
| 3000, low (1800 Hz) | 300 Hz |
| 3200, high (1920 Hz) | 320 Hz |
| 2400, low (1600 Hz) | 560 Hz |

The interpolation low-pass is only −6.0 dB at its own cutoff (2185.7 Hz at 3429)
and −15.3 dB at 2400 Hz, so a good part of the image survives it. The T/2
equaliser *can* null it, because the image lies outside the signal band in the
T/2 Nyquist interval — but 31 taps at a 6857 Hz half-symbol rate resolve
**221 Hz**, coarser than the 147 Hz guard it has to place the null in, so it
cannot do so without disturbing the band edge.

- **Symptom:** an SNR ceiling at the two top symbol rates that does not improve
  when the line does. It is directly testable: `make_mp`'s rate formula
  (`training.rs:1475-1477`) needs 14 × 2400 = 33 600, i.e. ≥ 9.8 bits/symbol at
  3429 baud, i.e. `log2(1 + snr/10^0.6) ≥ 9.8`, i.e. **≥ 35.5 dB**. (31 200 needs
  33.4 dB.) Anything that caps the trained SNR below 35.5 dB means V.34 never
  asks for 33 600 on any line, however good, and asks for 31 200 instead — and
  `receiver.rs:1411` asserts only `> 35.0`, so a ceiling right at that boundary
  would pass the test suite unnoticed.
- **How I know:** the guard arithmetic and the filter response above are both
  computed from the code's own constants (`receiver.rs:456-472`, `probe.rs:248-257`).
  The tests only ever assert a floor (`> 35.0` at `receiver.rs:1411`), never a
  measured value, so the actual ceiling is not recorded anywhere. See §8.
- **Severity: medium.** It does not break the call; it caps it.

### 7.2 The adaptation gate is absolute, the gain error it must correct is proportional

`doubtful` is one grid unit (§3.1) whatever the point. A gain error `g` displaces
a point at radius `r` by `g·r`, so it is the **outer** points — the ones carrying
almost all the information about gain — that are excluded first. At 33 600 bit/s
as negotiated (L = 1408, mean `|z|` = 25.0 grid units, peak 42.45):

| gain error | share of symbols frozen out of all three loops |
|---|---|
| 2 % | 0 % |
| 2.36 % | the outermost point only |
| 5 % | **67 %** |
| 10 % | **91 %** |

and there is no AGC to fix it from outside (§3.5). The 10 % remaining at a 10 %
error are the smallest points, whose NLMS gradient for a gain error is the
weakest.

- **Symptom:** after a VoIP softphone's gain control moves (memory:
  `v90-live-status`, "softphone jitter cuts + gain control"), data mode does not
  recover promptly. The user sees throughput collapse for a second or more, then
  a frame re-acquisition, rather than a smooth correction. On a step of more than
  about 10 % it can stay there until `STRAYED_SYMBOLS` (0.29 s) forces a search
  that does not address gain at all.
- **How I know:** the arithmetic above, computed from `Slicer::min_distance_squared`
  (169), `data.rs::energies` (70-106) and the quarter-superconstellation
  (`constellation.rs:24-33`). The code itself records the live case at
  `receiver.rs:1025-1026`, and `resync_dense` exists only because of it — which is
  the fix *after* a declared loss, not during one.
- **Severity: high.** It is the only gain correction in the mode, and it is
  reachable only through a loss.

### 7.3 With non-linear coding on, the tracking slicer would slice the wrong points

The channel output of a non-linearly encoded transmitter is `x' = Φ·x` with
`Φ = 1 + ζ/6 + ζ²/120` and `ζ = Θ|x|²/E` (9-33…9-35, page 24). The **decoder**
un-warps it (`data.rs:437-448`); the **tracking slicer** does not (`receiver.rs:157-161`).
At 33 600 bit/s with expanded shaping and Θ = 0.3125:

| `|z|` (grid units) | Φ | displacement off the grid |
|---|---|---|
| 22.95 | 1.040 | 0.92 |
| 32.45 | 1.081 | 2.63 |
| 39.74 | 1.123 | 4.89 |
| 45.89 (peak) | 1.166 | **7.62** |

**50.9 %** of 2-D symbols would be displaced by more than one grid unit, i.e.
past `doubtful = 1.225e-3`, so half of data mode would be excluded from the
equaliser, the carrier loop and the timing loop. Worse, the two error averages
diverge, because only one of them is gated:

- `recent` (930) and `self.error` (988) are updated for **every** symbol, so both
  settle at the full mean squared displacement, 6.19 grid units² = **7.58e-3** at
  unit power — a **21.2 dB floor** on `snr_db()` (597) whatever the line is.
- `settled` (968) is updated only for symbols that pass the gate, so it settles at
  the *conditional* mean of the inner half, **2.66e-4**.

`lost_threshold` for a grid is `max(2·settled, minsq/12) = max(5.33e-4, 4.08e-4)
= 5.33e-4`, and `recent` is 7.58e-3 — fourteen times over. **The receiver declares
itself lost within the first 32 symbols of data mode and never comes back**:
`found_level` is the same 5.33e-4, and the best reading `resync_dense` can
possibly produce is the same 7.58e-3, so every resync is rejected at 1094 and
fires again 32 symbols later, for ever, with all three loops frozen.

And the 21.2 dB `snr_db()` is the figure `make_mp` (1475-1477) chooses the
receive rate from: `log2(1 + 131.8/10^0.6) = 5.09` bits, × 3428.57/2400 → the MP
would ask for **16 800 bit/s**.

- **Symptom:** if anyone ever sets `non_linear: true` in `make_mp`, the next
  connection asks for 16 800 bit/s instead of 33 600, and then goes permanently
  deaf about thirty symbols into data mode — carrier still detected, status still
  Connected, no retrain asked for (§7.4), no data.
- **How I know:** computed from `data.rs::projection` (109-111), `energies`
  (70-106) and `Framing::new` (86-136) for 33 600/3429 expanded; Θ and Φ read from
  the rendered page 24.
- **Severity: medium** — latent, not live, because `make_mp` hard-codes
  `non_linear: false` (1490). It is a trap laid for whoever turns shaping and
  non-linear coding on, which is the obvious next rate improvement.

### 7.4 A data-mode receiver can stay `lost` for ever

Nothing in `Stage::Data` reads `is_lost()`. `training.rs:1023` and `1026` consult
it only while waiting for E; `v90/analogue.rs:1236, 1827` has its own watchdog,
V.34 has none. If `resync_dense` never accepts a reading, `lost` counts up without
limit (943) and `resync` is retried every 32 symbols for ever. The only escape is
the decoder's `path_cost > 1.0` for 1000 symbols → `search()` (1285-1290), which
re-runs frame acquisition but does not touch timing, carrier or gain; and when the
acquirer fails `SEARCHES = 4` times in data mode the handler is
`Acquired::Nothing => self.search()` (1279) — it starts over, for ever.

- **Symptom:** a call that goes silent and stays silent. `carrier()` still returns
  true (`1465-1467`) because the level is fine, the status still says Connected,
  no retrain is asked for, and the user waits.
- **How I know:** read directly off the control flow above; there is no path from
  `lost` or from repeated acquisition failure to `wants_retrain`.
- **Severity: high.** This is the same hole as §6.5 seen from the receiver's end.

### 7.5 `rewind` fails silently when asked to reach too far

`self.earlier` holds 24 snapshots at 16 symbols = 384 symbols. `rewind` (997-999)
does `rposition(|l| now − l.symbol >= back)` and **returns doing nothing** if no
snapshot is old enough. `training.rs:984` calls `rewind(self.off_run + 32)`, and
`off_run` (1026) counts every symbol whose error is above a *tenth* of
`UNLIKE_ERROR` — a much easier trip than `unlike_run`, so it can reach the
thousands while `unlike_run` stays under 150. Past `off_run = 352` the rewind is a
no-op and `search()` proceeds from loops that were taught by the wrong
constellation.

- **Symptom:** the "E that never arrived" recovery (`1010-1030`) succeeds less
  often than it should on a slightly noisy line — `found_again`
  (`training.rs:893`) stays zero and the call fails on the E deadline instead.
- **Severity: medium.**

### 7.6 `resync_dense` leaves the rotation five symbols stale

`resync_dense` judges its window ending at `last = next_symbol − 10`, i.e. five
symbols before the next symbol to be produced (1032), but `take_up` sets
`rotation = turned + turn·(window/2)` (1132) — the extrapolation to the window's
midpoint plus 32 symbols, when the next symbol is 37 symbols past the midpoint.
The plain `resync` has the same shape but is off by half a symbol (`last =
next_symbol − 2`), which does not matter.

Five symbols of `turn` is 0.5° at a 1 Hz carrier offset, 2.6° at 5 Hz — larger
than `DENSE_DEGREES = 1.5°`, the resolution the dense search just spent 32×60
evaluations achieving.

- **Symptom:** a dense resync that lands and then immediately reads a few symbols
  badly while the carrier loop pulls the residual out; on a marginal line it can
  re-trip the loss detector and loop.
- **Severity: low** — the carrier loop removes it in a handful of symbols.

### 7.7 `settled` survives a change of slicer

`set_slicer` (590-594) clears `lost` and `recent` but not `settled`, which feeds
both `lost_threshold` (191) and `found_level` (199). Going from `Points(Sixteen)`
(settled ~1e-3) to `Grid` at the E event (`training.rs:1393`), the grid's loss
threshold starts at `2·1e-3 = 2e-3` against a floor of 4.1e-4 — nearly 5× too lax
for the first ~100 symbols while the 0.01 EMA catches up.

- **Symptom:** a slip landing in the first 30 ms of data mode is not detected as a
  loss and is only found later, by the decoder's strayed counter, 0.29 s in.
- **Severity: low.**

### 7.8 The grid slicer's limit is wider than the constellation

`Decoder::extent` (`data.rs:570-572` → `369-378`) adds `2·precoder_scale` to the
largest coordinate — 4 grid units at 33 600 bit/s, giving `limit = 49` against a
real peak of 45. That room is correct for a precoded signal; with `precoding: None`
it just lets a noisy outer symbol be decided to a point (47 or 49) that the
constellation does not contain, producing a large error that then trips
`doubtful` and freezes the loops.

- **Severity: low.** It only ever loses adaptation opportunities, never mis-tracks.

### 7.9 The phase detector loses gain on the innermost points

`power = target.norm_sqr().max(0.1)` (955) makes the detector gain
`|target|²/0.1` for any point closer in than `√0.1` at unit power = 8.5 grid units.
At 33 600 bit/s that is **5.7 %** of symbols, and the mean detector gain over the
shaped constellation is **0.969** — so the carrier loop's effective bandwidth is
about 3 % below the 6.8 Hz of §3.3.

- **Severity: low**, and the clamp is right: without it a point near the origin
  would divide by near-zero.

---

## 8. Open questions

1. What is the actual trained SNR ceiling at 3429 baud on a noiseless loopback?
   Every test asserts a floor (`> 35.0`, `> 28.0`) and none records the value, so
   §7.1's prediction of an image-limited ceiling is untested. A test that trains
   on a clean line at each of the six symbol rates and both carriers, and prints
   `trained_snr_db()`, would settle it in one run — and the 3200-low/3429 pair
   should read worse than 2400-low if the image is what is limiting.
2. Is the residual after §7.1 better at the high carrier? At 3429 both carriers
   are 1959.18 Hz so there is no choice, but at 3200 the high carrier gives 320 Hz
   of guard against the low carrier's 137 Hz. If measurement confirms the image is
   the limit, phase 2 should prefer the high carrier at 3200.
3. Does any real far end send a Type 1 MP asking us to precode *and* expect us to
   apply it to our receive path? `receive_params` (1425-1432) reads precoding from
   *our* MP, which is always `None`. If a far end precodes unasked, our decoder's
   `Precoder` would be zeroed while the signal is not — worth checking against the
   captures in `dist/captures/`.
4. How often does `rewind` actually no-op (§7.5)? A counter on the `return` at
   999 would answer it from one live call.
5. `reacquire` (857-906) has never, as far as the comments record, fired on a real
   far end. Is it dead code kept for safety, or did a live far end need it? The
   memory note `live-v34-far-end` says dialup.world "renegotiates, retrains if
   unanswered" but does not say its TRN scrambler state.

---

## 9. Reading list for the older modes

The modes with a real receiver to judge are `v22bis.rs` (+ `v22bis/handshake.rs`),
`v27ter.rs`, `v29.rs` and `v32.rs` (+ `v32/{startup,trellis}.rs`) — the four
importers of `dsp::Equalizer`. `bell103.rs` and `v21.rs` are FSK and use
`dsp/src/fsk.rs` instead, so most of this does not apply to them. `v17.rs` is
267 lines of signal and constellation definitions with no receiver at all and no
importer anywhere in the tree (`grep -rn "v17::"` outside the file returns
nothing) — it is unwired, which is worth confirming with Rory before any work on
it.

Questions, in order of how much they matter on a real line:

1. Is there a known training sequence, and is the equaliser solved from it, or
   does it converge blind? (V.22bis, V.27ter, V.29, V.32 all have a defined
   training sequence in their Recommendations; all four currently converge blind.)
2. Is the equaliser fractionally spaced? All four are symbol-spaced, so they need
   a timing loop that finds the eye, not just one that stops drift.
3. Is the carrier loop second order? A first-order loop leaves 2.6° per hertz at
   3429 baud and proportionally more at lower rates — fatal for V.32's 32-point
   constellation, survivable for V.22bis's four.
4. Is adaptation gated on a `doubtful` test, or does it learn from every decision?
5. Is there any loss detection, rewind or resync at all? (No.)
6. Is there any slip handling? A VoIP jitter slip is 20 ms — 69 symbols at 3429,
   but 48 symbols at 2400 and 28 at 1200 baud, and the older modes' differential
   coding means the same trick V.34 uses (§4.3, "where the symbols fall in whole
   half symbols does not matter") applies to them too.
