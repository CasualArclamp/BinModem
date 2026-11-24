# The shared DSP crate, and what the slow modes do with it

`crates/dsp` is 4017 lines in ten modules plus `lib.rs`, no dependencies
(`crates/dsp/Cargo.toml` has no `[dependencies]` section at all), 75 unit
tests, all green (`cargo test -p dsp --lib`, 1.62 s). Its stated design rule
is at `crates/dsp/src/lib.rs:3-10`: everything is sample-at-a-time and
stateful, no public API takes or returns a block.

This digest is about the receivers below V.34 — Bell 103, V.21, V.22bis,
V.27 ter, V.29, V.17, V.32/V.32bis — and what they inherit from this crate.
Rory's reading is that the 33.6k QAM receiver holds and the older ones are
running older code. That is right, and it is more specific than it sounds:
**the shared crate contains the 1988-era half of the receiver and nothing
else.** Every mechanism V.34 added in order to hold a real line — a
fractionally spaced equaliser, a fractional-delay interpolator worth more
than two taps, a data-aided timing detector, an open-loop carrier estimate, a
loss detector, a rewind — lives inside `crates/datapump/src/v34/` and is not
reachable from V.22bis or V.32. The slow modes are not using an old version
of the shared code. They are using all of it.

Everything below that says "measured" was measured; how, and with what, is in
the last section.

---

## 1. What is in the crate

### 1.1 `nco.rs` — 80 lines, 3 tests

`Nco` (`nco.rs:11-40`) is a unit complex phasor generator. Phase is a
fraction of a turn in `[0,1)`, wrapped by subtracting its floor
(`nco.rs:34`), so the accumulator stays exact for any run length — the test
at `nco.rs:64-70` runs 8 000 000 samples and checks it. `step()`
(`nco.rs:30-37`) returns `(cos, sin)` by calling `r.cos()` and `r.sin()`
separately.

**Used by:** every mixer in the tree — `v22bis.rs:576`, `v27ter.rs:666`,
`v29.rs:624`, `v32.rs:527`/`896`, `v8.rs:14`, `v34/dpsk.rs:18`,
`v34/phase2.rs:1288`, and inside `ToneDetector` (`tone.rs:95`) and
`FskDetector` (`fsk.rs:71`). 39 references in the datapump crate.

**Dead API:** `set_frequency` (`nco.rs:22`), `adjust` (`nco.rs:26`) and
`frequency` (`nco.rs:23`) have **no callers anywhere in the workspace**
(verified by grep over `crates/**/*.rs`). `adjust` is documented as "Nudge
the frequency, as a carrier-tracking loop would" — no carrier-tracking loop
does. Every mode instead keeps its own `phase`/`frequency` pair and rotates
the symbol after the matched filter. That is the right architecture (it keeps
the select and matched filters out of the loop), but it means the doc comment
describes a design nobody chose, and it means there is no shared carrier loop
at all. See §3.1.

### 1.2 `filter.rs` — 208 lines, 5 tests

- `Biquad` (`filter.rs:15-36`), transposed direct form II. Correct: `y = b0·x
  + z1; z1 = b1·x − a1·y + z2; z2 = b2·x − a2·y`.
- `Cascade` (`filter.rs:40-55`), a `Vec<Biquad>` folded in order.
- `butterworth_zetas` (`filter.rs:62-67`): `sin(π(2k+1)/2n)` for `k < n/2`.
  Correct; even orders only, asserted.
- `butter_lowpass`/`butter_highpass` (`filter.rs:70-91`): RBJ bilinear
  cookbook with `alpha = sin(w0)·ζ` (= `sin(w0)/2Q` with `Q = 1/2ζ`).
  Correct. `fc/fs` clamped to `[1e-6, 0.4999]` (`filter.rs:71`).
- `bandpass` (`filter.rs:98-102`): a high-pass of order *n* followed by a
  low-pass of order *n*. Documented as not a textbook BP transformation.
- `OnePole` (`filter.rs:110-139`): `a = exp(−1/(τ·fs))`, τ in **seconds**.
  `starting_at` (`filter.rs:122-124`) exists because a smoothed value used as
  a divisor must not start at zero.

**Used by:** `OnePole` is the workhorse — 32 references in datapump
(`v22bis.rs:612`, `v27ter.rs:676/680`, `v29.rs:640`, `v32.rs:908/915`,
`v32/startup.rs:166/167/179/180`, `v21.rs:247`, `v8.rs:13`), plus
`ReversalDetector` (`tone.rs:229`) and `FskDetector` (`fsk.rs:80-81`).
`Biquad`, `Cascade`, `butter_lowpass`, `butter_highpass` and `bandpass` have
**zero direct references in datapump** — they are reached only through
`FskDetector`, i.e. only by Bell 103 and V.21.

**Dead API:** `OnePole::set` (`filter.rs:137`) has no callers.

### 1.3 `shaping.rs` — 751 lines, 18 tests

- `rrc_taps(sps, rolloff, span)` (`shaping.rs:14-27`): odd length, unit
  **energy** normalisation.
- `rrc_at(t, beta)` (`shaping.rs:35-52`): the closed form with both removable
  singularities handled. Public precisely because 16 kHz against 600 baud is
  26.667 samples per symbol and the transmitter must evaluate the pulse at
  arbitrary offsets.
- `fir_lowpass(cutoff, taps, fs)` (`shaping.rs:64-86`): windowed sinc,
  Hamming, DC-normalised to unit gain. ~53 dB stopband floor whatever the
  length.
- `fir_lowpass_kaiser(pass, stop, stopband_db, fs)` (`shaping.rs:121-155`):
  asks for the rejection and pays in taps. **Zero callers.** The only mention
  in the tree is the comment at `v22bis.rs:566` saying it was tried, cost two
  frames of a recorded call, and was dropped.
- `Fir` (`shaping.rs:159-203`) and `ComplexFir` (`shaping.rs:207-230`).
- `Gardner` (`shaping.rs:239-359`): the only timing recovery in the crate.

**`Gardner`'s numbers**, all from `shaping.rs:267-354`:

| thing | line | value |
|---|---|---|
| proportional gain | ctor arg | `0.1` samples per unit normalised error, in every mode |
| integral gain | `shaping.rs:276` | `gain/100` = `1.0e-3` |
| integral clamp | `shaping.rs:349` | `±sps/8` → ±25 % of clock rate |
| phase clamp | `shaping.rs:351` | `±sps/4` |
| error clamp | `shaping.rs:338` | `±1` after normalisation |
| power normaliser | `shaping.rs:337` | `mean_power += 0.02·(power − mean_power)`, τ = 50 symbols |
| power initial value | `shaping.rs:282` | **`1.0`** |
| `set_adapting` | `shaping.rs:299-301` | called only by V.32 (`v32.rs:1194`) |

**Used by:** `Gardner::new(sps, 0.1)` at `v22bis.rs:587`, `v27ter.rs:671` and
`:707`, `v29.rs:630`, `v32.rs:903`. The same gain pair in all four. V.34 does
not use it. `rrc_taps`/`rrc_at` in all four plus `v34/qam.rs:14`.
`fir_lowpass` in all four. `Fir` only in `v34/qam.rs`.

### 1.4 `equalizer.rs` — 301 lines, 5 tests

`Equalizer` (`equalizer.rs:20-157`) is a **symbol-spaced complex LMS filter**
with a blind constant-modulus stage and a decision-directed stage.

| thing | line | value |
|---|---|---|
| blind step μ | `equalizer.rs:49` | `2.0e-3` |
| tracking step μ | `equalizer.rs:50` | `4.0e-3` |
| dispersion target R₂ | ctor arg | 1.32 (V.22bis), `UNIT`=1.0 (V.27 ter), computed (V.29), **1.0** (V.32) |
| convergence smoothing | `equalizer.rs:116` | `0.01`, τ = 100 symbols |
| blind → DD threshold | `equalizer.rs:121` | mean `|y−dec|` < **0.25** |
| blow-up guard | `equalizer.rs:105` | total tap energy > `1.0e4` → full reset |
| initial state | `equalizer.rs:44-45` | centre tap `(1,0)`, rest zero |

With the input held at unit mean power (which every caller does), the LMS
mode time constant is `1/(2μ)` symbols: **250 symbols blind, 125 tracking**.
That is 417 ms / 208 ms at 600 baud, 104 ms / 52 ms at 2400 baud. The
stability bound `μ < 2/(N·σ²)` is 0.095 for 21 taps and 0.065 for 31, so
4.0e-3 sits at 4-6 % of the bound: very safe, and very slow.

**Used by:** `v22bis.rs:599` (21 taps, R₂ = 1.32), `v27ter.rs:679` (31,
R₂ = 1.0), `v29.rs:639` and rebuilt per rate at `v29.rs:683` (31, R₂ computed
from the constellation), `v32.rs:909` (21, R₂ = 1.0). V.34 does not use it.

**Dead API:** `with_steps` (`equalizer.rs:57-61`) has no callers; every mode
runs the default 2e-3/4e-3.

### 1.5 `tone.rs` — 830 lines, 14 tests

The most carefully written file in the crate, and the one with the tightest
margin against the Recommendation (§5.2).

`ToneDetector` (`tone.rs:60-131`): NCO down-conversion into two cascaded
`OnePole`s per axis. Each pole is widened by `CASCADE_CORNER = 0.6436`
(`tone.rs:29`) so the pair is half power at the requested bandwidth, and the
skirt then falls at 12 dB/octave instead of 6. `amplitude()` returns twice
the phasor magnitude, which is right for a real cosine.

At the 60 Hz bandwidth V.32 and V.34 use, at 16 kHz: each pole is 93.23 Hz
wide, τ = 1.707 ms = 27.3 samples.

`ReversalDetector` (`tone.rs:146-459`) watches one tone for the half-turn
V.32 5.4 uses as a timing mark. At bandwidth 60 Hz and fs 16 kHz:

| thing | line | value |
|---|---|---|
| `tau` (single-pole equivalent) | `tone.rs:211` | 42.44 **samples** |
| comparison depth | `tone.rs:216` | `ceil(6τ)` = 255 samples = 15.93 ms |
| opposition must hold | `tone.rs:219` | `ceil(τ)` = 43 samples = 2.69 ms |
| refractory | `tone.rs:226` | 255 samples → at most 62.8 reversals/s |
| reported latency | `tone.rs:248` | `(1.678·0.6436 + 1)·τ` = 88 samples = 5.5 ms |
| presence envelope | `tone.rs:229` | `OnePole::new(0.100, fs)` |
| drift average | `tone.rs:235` | 0.5 s, run as a plain mean until `settled ≥ 255` (`tone.rs:349-359`) |
| "opposed" | `tone.rs:404` | dot < −0.7, i.e. more than 134.4° apart |
| collapse allowance | `tone.rs:350-352` | `2·|z| ≤ 0.5·envelope` for at most `2·confirm` = 86 samples |
| **refusal gate** | `tone.rs:399`, `MAX_CARRY` `tone.rs:57` | `|drift|·255 > π/4` |

That last row is the important one. `drift` is radians per sample, so the
gate refuses to judge when the tone is more than

    f > MAX_CARRY / (6·τ) · fs/2π = (π/4)·bw/6/(2π)·2π = π·bw/24

Hz off frequency. **At bw = 60 Hz that is 7.854 Hz**, independent of the
sample rate. See §5.2.

**Used by:** `ToneDetector` 30 references — `v32/startup.rs:163/168/169/170/173`
(eight detectors per direction), `v21.rs:246`, `v8/ansam.rs:28`,
`v34/training.rs:250-252`, `v34/phase2.rs:230-232`. `ReversalDetector` 13 —
`v32/startup.rs:994-996` and `:1997-2001`, `v34/phase2.rs:338`.

### 1.6 `echo.rs` — 715 lines, 11 tests

`EchoCanceller` (`echo.rs:111-283`): normalised LMS over **two** runs of taps
with a hole between them — the hybrid's immediate return and the network's
late one — sharing one division by the energy under both (`echo.rs:234-235`,
and the reason is written out at `echo.rs:230-233`). `Segment::shift`
(`echo.rs:73-78`) keeps each run's energy exactly by adding what entered and
subtracting what left, rather than averaging it; the comment at
`echo.rs:201-208` records that a running average of the reference power made
the canceller diverge to −200 dB of return loss on band-limited noise.

`EchoFinder` (`echo.rs:319-376`): correlate what arrives against every
candidate delay at once, over a stretch where the far end is silent, and
normalise by `sqrt(reference·arriving)`.

**Numbers as V.32 uses them:** near run 128 taps = 8 ms at 16 kHz
(`v32/startup.rs:2138`, `:2174`), step **μ = 0.5**, far run 64 taps = 4 ms
(`v32/startup.rs:2145`), accepted only at correlation ≥ 0.15
(`v32/startup.rs:2164`), return-loss meters at `POWER_TRACK = 0.01`
(`echo.rs:37`), τ = 100 samples = 6.25 ms. Adaptation is on only during
training (`v32/startup.rs:2240`).

`echo.rs:137-138` says "Something around a tenth converges in a few thousand
samples and leaves the taps quiet once it has." V.32 passes 0.5. NLMS
misadjustment is `μ/(2−μ)`: **33 % excess mean-square error at μ = 0.5
against 5.3 % at μ = 0.1**, a 8.0 dB difference in the floor the taps settle
at — and since `set_adapting(false)` freezes them where they happen to be,
that misadjustment is frozen into the call. Nothing anywhere steps μ down as
training ends.

**Used by:** V.32 only (`v32/startup.rs:18`). Nothing else in the tree needs
it: V.22bis separates the directions by band, V.27 ter/V.29/V.17 are
half-duplex, and V.34/V.90 have their own arrangements.

### 1.7 `resample.rs` — 259 lines, 5 tests

`Resampler` (`resample.rs:32-117`): arbitrary-ratio windowed-sinc
interpolation, 16 zero crossings either side (`resample.rs:28`), Blackman
window (`resample.rs:128-135`), kernel stretched to the output period when
coming down in rate so the same kernel is the anti-alias filter
(`resample.rs:54-59`). Output normalised by the weight actually used
(`resample.rs:111-115`) to hold the gain at unity whatever fraction of a
sample the output lands at. `span = ceil(2·reach) + 2` (`resample.rs:61`) is
exactly enough: I checked that `behind` at output time is always in
`[reach, reach+1)`, so the kernel never hangs off either end.

Real-only. No complex version, no way to ask for the signal at one arbitrary
time.

**Used by:** `line/duplex.rs:26` (sound card ↔ modem), `v90/server.rs:28`,
and V.34's *tests* only (`v34/receiver.rs:1339`, `:1447`,
`v34/training.rs:1748-1775` — clock-offset and 8 kHz-path simulations). **No
slow-mode receiver uses it.**

### 1.8 `complex.rs` — 267 lines, 3 tests

`Complex` with operators (`complex.rs:11-130`), `least_squares(rows, targets,
ridge)` (`complex.rs:142-163`) building the normal equations with a ridge on
the diagonal, and `solve_hermitian` (`complex.rs:167-213`) by Cholesky with a
rank-deficiency guard at `complex.rs:182`.

The header (`complex.rs:1-7`) is explicit that this exists for V.34: "a
receiver that solves for its equaliser from a training sequence does a great
deal". The ridge doc (`complex.rs:136-140`) is explicit that it exists to
keep a **T/2-spaced** equaliser solvable.

**Used by:** `v34/data.rs:27`, `v34/qam.rs:14`, `v34/receiver.rs:48`,
`v34/training.rs:38`, `v90/analogue.rs:25`. 125 references, all V.34/V.90.
**No slow mode uses `Complex` at all** — they are all `(f64, f64)` tuples.
`solve_hermitian` has no caller outside `least_squares`.

### 1.9 `fft.rs` — 270 lines, 6 tests

`Fft` (`fft.rs:14-84`), radix-2, precomputed twiddles and bit reversal.
`Spectrum` (`fft.rs:91-169`), periodic Hann, coherent-gain corrected, reports
**amplitude** dBFS (a full-scale sine reads 0 dB).

**Used by:** `gui/engine.rs:23`, `gui/live.rs:20`, one V.34 test
(`v34/dpsk.rs:558`), one V.22bis test (`tests/v22bis_loopback.rs:137`). No
receiver uses it in the signal path.

### 1.10 `fsk.rs` — 302 lines, 5 tests

`FskDetector` (`fsk.rs:17-139`): band isolation → quadrature down-conversion
→ complex low-pass → phase differencing → post-detection low-pass, plus
carrier detection.

At 300 baud, fs 16 kHz: band `bandpass(8, centre−370, centre+370)`
(`fsk.rs:63`, `:72`) — order **8**, not 4, because the dominant interferer is
our own transmitter through the hybrid (`fsk.rs:64-69`); complex low-pass
`butter_lowpass(4, 390)` (`fsk.rs:73-74`); post-detection `butter_lowpass(2,
240)` (`fsk.rs:75`); `fast_env` τ = 5 ms, `slow_env` τ = 250 ms
(`fsk.rs:80-81`).

`deviation` is **signed** (`fsk.rs:26-36`) — the comment records that taking
its magnitude inverted every bit of V.21, whose mark is below its space in
both channels, while Bell 103 never noticed.

Carrier: absolute thresholds with hysteresis, `CARRIER_ON = 1.0e-3`,
`CARRIER_OFF = 5.62e-4` (`fsk.rs:49-50`), 5.00 dB apart.

**Dead field:** `slow_env` is written at `fsk.rs:105` and **never read**.
`level()` returns `fast_env` (`fsk.rs:136-138`). It is the leftover of the
ratio detector whose removal is described at `fsk.rs:121-131`.

**Used by:** `bell103.rs:73`, `v21.rs:162`. Nothing else.

---

## 2. Who uses what — the map

| shared item | Bell 103 | V.21 | V.22bis | V.27 ter | V.29 | V.17 | V.32 | V.34 | V.90 |
|---|---|---|---|---|---|---|---|---|---|
| `Nco` | via FSK | via FSK | ✓ | ✓ | ✓ | – | ✓ | ✓ (dpsk) | – |
| `FskDetector` | ✓ | ✓ | – | – | – | – | – | – | – |
| `ComplexFir` + `fir_lowpass` | – | – | ✓ | ✓ | ✓ | – | ✓ | ✓ (dpsk) | – |
| `rrc_taps`/`rrc_at` | – | – | ✓ | ✓ | ✓ | – | ✓ | ✓ (qam) | – |
| `Gardner` | – | – | ✓ | ✓ | ✓ | – | ✓ | – | – |
| `Equalizer` | – | – | ✓ | ✓ | ✓ | – | ✓ | – | – |
| `OnePole` | via FSK | ✓ | ✓ | ✓ | ✓ | – | ✓ | – | – |
| `ToneDetector` | – | ✓ | – | – | – | – | ✓ | ✓ | – |
| `ReversalDetector` | – | – | – | – | – | – | ✓ | ✓ | – |
| `EchoCanceller`/`EchoFinder` | – | – | – | – | – | – | ✓ | – | – |
| `Complex`/`least_squares` | – | – | – | – | – | – | – | ✓ | ✓ |
| `Resampler` | – | – | – | – | – | – | – | tests | server |
| `Fft`/`Spectrum` | – | – | test | – | – | – | – | test | – |

**V.17 has no receiver.** `crates/datapump/src/v17.rs` is 267 lines of
constants and tables (`v17.rs:20-140`) that reuse `v32::trellis`; there is no
`Nco`, no `Gardner`, no `Equalizer`, no `Receiver` type. The only consumers
are `datapump/src/lib.rs` and `gui/src/faxwin.rs`. Whatever "V.17 works" means
in this tree, it does not mean a V.17 demodulator exists.

---

## 3. Where two modes solve the same problem differently

### 3.1 The carrier loop: four copies, four gain pairs, two acquisition strategies

There is no shared carrier loop. Each mode keeps `phase` and `frequency` as
fractions of a turn and runs its own second-order update:

| mode | line | phase gain | frequency gain | clamp | error smoothing |
|---|---|---|---|---|---|
| V.22bis | `v22bis.rs:728-731` | `0.008` | `1.5e-5` | `±0.02` turns/symbol = ±12 Hz | none |
| V.27 ter | `v27ter.rs:935-936` | `0.010` | `2.0e-5` | `±0.02` = ±32 Hz at 1600 baud, ±24 Hz at 1200 | `track += 0.20·(raw−track)` (`v27ter.rs:931`) |
| V.29 | `v29.rs:974-975` | `0.008` | `1.5e-5` | `±0.02` = ±48 Hz | `TRACK` inline |
| V.32 | `v32.rs:1039-1041` | `0.008·bw` | `1.5e-5·bw²` | `±0.02` = ±48 Hz | `TRACK_SMOOTHING = 0.1` (`v32.rs:285`) |
| V.34 | `receiver.rs:957-958` | `PHASE_GAIN = 0.04` rad | `FREQUENCY_GAIN = 4e-4` rad | none | none |

The error term is in **radians** while `phase` is in **turns**, so the
effective loop constants are `Kp = gain·2π` and `Ki = gain·2π`. For V.22bis:
`Kp = 0.0503`, `Ki = 9.425e-5`, `ωn = √Ki = 9.708e-3` rad/symbol,
**ζ = Kp/(2√Ki) = 2.59** — heavily overdamped, ωn = 0.93 Hz at 600 baud, loop
noise bandwidth 1.25 Hz.

V.32's `bw`/`bw²` scaling (`v32.rs:270-277`, `loop_bandwidth`) keeps ζ at
2.59 for every constellation while narrowing ωn as the points crowd: bw = 1.0
uncoded, 0.707 at 7200, **0.500 at 9600**, 0.345 at 12000, **0.247 at 14400**.

**Acquisition is where they diverge.** V.27 ter (`v27ter.rs:866-895`) and
V.29 (`v29.rs:892-935`) both added an open-loop estimate before the tracking
loop is allowed to run at all: V.27 ter squares the two-phase training and
takes the mean turn between consecutive squares; V.29 uses the known A/B
alternation of segment 2 directly, trying both phasings and keeping the
stronger. Both then set `frequency` and `phase` outright (`v27ter.rs:892-893`,
`v29.rs:932-933`) and only then start tracking. The comment at
`v27ter.rs:860-865` is explicit about why: "every one of forty tries at plus
or minus seven hertz failed" without it; `v29.rs:876-881` says "forty-five of
eighty tries".

**V.22bis and V.32 have no such thing.** They start at `phase = 0,
frequency = 0` and pull in from the decisions. Measured — see §7 for the
method — with the exact difference equations of `v22bis.rs:720-731` against a
unit-power 16-point constellation, time to a clean eye:

| offset | V.22bis time to lock (600 baud) |
|---|---|
| +1 Hz | 1 527 symbols = **2.5 s** |
| +2 Hz | 2 600 = 4.3 s |
| +3 Hz | 5 322 = 8.9 s |
| +4 Hz | 7 986 = 13.3 s |
| +5 Hz | 14 599 = 24.3 s |
| +6 Hz | 19 223 = 32.0 s |
| **+7 Hz** | 24 892 = **41.5 s** |

And with the exact equations of `v32.rs:1018-1047`, the real constellations
read out of `v32/trellis.rs`, and each rate's own `loop_bandwidth`:

| V.32/V.32bis rate | bw | +1 Hz | +3 Hz | +7 Hz |
|---|---|---|---|---|
| 4800 (4 pt) | 1.000 | 0.44 s | 0.67 s | 0.85 s |
| 9600 coded (32 pt) | 0.500 | 0.97 s | 6.7 s | **85 s** |
| 12000 coded (64 pt) | 0.345 | 3.3 s | **83 s** | **never** (250 s) |
| 14400 coded (128 pt) | 0.247 | **51 s** | **never** | **never** |

V.22bis 2.6 and V.32 2.1 both require the receiver to operate with received
frequency offsets of up to ±7 Hz (both rendered, §5.1). V.32bis at 14 400
cannot pull in **one** hertz inside a minute.

There is also no test. `v27ter.rs:1235` and `v29.rs:1293` both build a
transmitter at `CARRIER + hz` and sweep the offset. Grep for the same in
`v22bis.rs`, `v32.rs`, `tests/v22bis_*.rs` and `tests/v32_*.rs` returns
nothing.

### 3.2 Normalising the Gardner detector: three answers, and one mode that never got the memo

`Gardner` divides its error by a running mean power that **starts at 1.0**
(`shaping.rs:282`) and moves 2 % a symbol (`shaping.rs:337`). The comment at
`v29.rs:820-826` spells out the consequence:

> It divides its error by its own running estimate of the power, which starts
> at one and moves two per cent a symbol, so thirty decibels down it spends
> the whole of segment 2 a thousand times too timid to move — and a receiver
> started half a symbol out of step stays there.

The arithmetic: to fall from 1.0 to within a factor of two of a true power
30 dB down takes `50·ln(1000) = 345` symbols. At 2400 baud that is 144 ms; at
600 baud, **575 ms** — the whole of a V.22bis handshake. During it the phase
correction is `0.1 × error/1000 = 1e-4` samples per symbol, so crossing the
half symbol that separates the worst starting phase from the right one takes
`(sps/2)/1e-4` = **33 000 symbols ≈ 14 s** at 2400 baud.

- **V.29** pre-scales by `1/level` and unscales the output
  (`v29.rs:832-839`).
- **V.27 ter** does exactly the same (`v27ter.rs:843-845`).
- **V.22bis** feeds the raw matched-filter output (`v22bis.rs:667-668`).
- **V.32** feeds the raw matched-filter output (`v32.rs:985-986`).

The fix belongs in the shared crate — `Gardner::new` taking a starting power,
or the loop normalising against a `OnePole::starting_at` the caller can seed
— and it never went there, so two of the four modes still have the fault the
other two fixed locally.

### 3.3 The fractional-sample instant: two taps against 64

All four slow modes interpolate the matched-filter output to the wanted
instant with **two-point linear interpolation**, in four identical copies:

- `v22bis.rs:662-666`
- `v27ter.rs:830-834`
- `v29.rs:814-818`
- `v32.rs:980-984`

V.34 built a 64-tap, 256-phase, Kaiser(β = 8) polyphase table
(`v34/receiver.rs:457-472`) and interpolates the **complex mixed-down**
signal at an arbitrary time (`v34/receiver.rs:662-678`).

Measured error power of the linear interpolator against exact interpolation,
for each mode's own `sps`, roll-off and pulse:

| mode | samples/symbol at 16 kHz | β | error | EVM |
|---|---|---|---|---|
| V.22bis, 600 baud | 26.667 | 0.75 | −57.0 dB | 0.14 % |
| V.27 ter, 4800 (1600 baud) | **10.000 exactly** | 0.50 | none | 0 % |
| V.27 ter, 2400 (1200 baud) | 13.333 | 0.50 | −47.2 dB | 0.44 % |
| V.29, 2400 baud | 6.667 | 0.25 | **−37.9 dB** | **1.28 %** |
| V.32, 2400 baud | 6.667 | 0.25 | **−37.9 dB** | **1.28 %** |

So the V.32 and V.29 receivers carry a hard SNR ceiling of **38 dB** from the
interpolator alone, however clean the line. V.32bis at 14 400 wants about
27 dB for a usable error rate, so this is about 0.5 dB of the margin rather
than a wall — but it is a wall for anything faster, and it is exactly why
V.34, at 4.67 samples per symbol, could not reuse this.

### 3.4 Carrier detection: five thresholds on five scales

| where | line | measured quantity | ON | OFF | gap |
|---|---|---|---|---|---|
| Bell 103, V.21 | `fsk.rs:49-50`, `:104-110` | 5 ms envelope of complex baseband | `1.0e-3` | `5.62e-4` | 5.00 dB |
| V.22bis | `v22bis.rs:129-130`, `:633-641` | 20 ms envelope of `|selected|` | `1.0e-3` | `5.62e-4` | 5.00 dB |
| V.32 | `v32.rs:289-290`, `:964-971` | 20 ms envelope of `|selected|` | `1.0e-3` | `5.62e-4` | 5.00 dB |
| V.27 ter | `v27ter.rs:557-591`, and V.29 `v29.rs:530-543`, `:776-805` | 10 ms envelope, **adaptive floor** | `max(4·floor, 1.0e-3)` | `max(0.25·loudest, 5.62e-4)` | 12 dB each side |
| V.32 start-up | `v32/startup.rs:36` | 100 ms envelope of `ToneDetector::amplitude` | `AUDIBLE = 0.008` | — | — |
| V.34 phase 2 | `v34/phase2.rs:144` | same | `AUDIBLE = 0.008` | — | — |
| V.34 receiver | `v34/receiver.rs:94` | half-symbol sample power | `AUDIBLE = 4e-4` | — | — |

Three of those are the same physical question — is the far end talking —
answered on three incompatible scales, and none of them is calibrated to the
dBm the Recommendation states. See §5.3.

### 3.5 Automatic gain control: four shapes

| mode | line | shape | start | τ |
|---|---|---|---|---|
| V.22bis | `v22bis.rs:594`, `:678-708` | `OnePole` on mean **power**, gain = √(10/P), clamped to 400 | `starting_at(10.0)` | 50 ms = 30 symbols |
| V.27 ter | `v27ter.rs:676`, `:905-911` | same shape, held while no carrier | `starting_at(1.0)` | 30 ms |
| V.29 | `v29.rs:855-871` | **plain mean over the first 32 symbols of segment 2, then exponential** at 1/72 | – | 30 ms after |
| V.32 | `v32.rs:908`, `:997-1004` | `OnePole`, held while `!adapting` | `starting_at(10.0)` | 50 ms = 120 symbols |
| V.34 | — | none: the least-squares solve sets the scale | – | – |

V.29's two-stage form (`v29.rs:842-854`) exists because an exponential
average starting at 1.0 "was still four times too high when the data began,
so the equaliser learned the whole training at the wrong gain and then had to
unlearn it on the page". The same argument applies word for word to V.22bis
and V.32, which still start at 10.0 and crawl.

### 3.6 The equaliser's target and the equaliser's handover

`Equalizer::new(taps, modulus)` is told the constant-modulus dispersion
target R₂ = E|a|⁴/E|a|² and nothing else about the constellation. Computed
from the actual tables in `v32/trellis.rs`, normalised as each receiver
normalises them:

| constellation | true R₂ | what is passed | line |
|---|---|---|---|
| V.22bis 2400, 16 pt | 1.320 | 1.32 ✓ | `v22bis.rs:599` |
| V.22bis 1200, 4 pt (the `01` point only) | **1.000** | 1.32 ✗ | never rebuilt (`v22bis.rs:920-922`) |
| V.27 ter, 8-PSK | 1.000 | `UNIT` = 1.0 ✓ | `v27ter.rs:679` |
| V.29, per rate | computed | computed ✓ | `v29.rs:677-683` |
| V.32 4800, 4 pt | 1.000 | 1.0 ✓ | `v32.rs:909` |
| V.32 9600, 32 pt | **1.310** | 1.0 ✗ | never rebuilt (`v32.rs:949-959`) |
| V.32bis 12000, 64 pt | **1.381** | 1.0 ✗ | " |
| V.32bis 14400, 128 pt | **1.343** | 1.0 ✗ | " |

The constant-modulus stage settles where `E|y|⁴ = R₂·E|y|²`, so a target of
1.0 against a true 1.31 converges to a scale of `√(1/1.31) = 0.874` — the
blind stage hands over a constellation **12.6 % small, −1.18 dB**. V.29
rebuilds the equaliser with the right target on every rate change
(`v29.rs:683`); `V32::Receiver::follow` (`v32.rs:949-959`) recomputes
`bandwidth` on the same path and leaves the equaliser alone.

The handover threshold is worse, because it is a bare `0.25`
(`equalizer.rs:121`) on a scale that means something different in each mode.
Against the distance between neighbouring points, at unit mean power:

| constellation | point spacing | 0.25 as a fraction of it |
|---|---|---|
| V.32 4800, 4 pt | 1.414 | 18 % |
| V.22bis 2400, 16 pt | 0.632 | 40 % |
| V.32 9600, 32 pt | 0.447 | 56 % |
| V.32bis 12000, 64 pt | 0.309 | **81 %** |
| V.32bis 14400, 128 pt | 0.221 | **113 %** |

At 14 400 the blind stage hands over to decision direction when the mean
error is still larger than the whole distance between points. The header of
the file (`equalizer.rs:15-18`) says exactly why that is fatal: "Starting
decision-directed on a closed eye simply reinforces whatever nonsense it
first decides."

### 3.7 Where the equaliser sits relative to the carrier loop

Both V.22bis (`v22bis.rs:735-740`) and V.32 (`v32.rs:1049-1053`) equalise the
**already de-rotated** symbol and adapt against the de-rotated decision. So
while the carrier loop is still pulling in, the equaliser is chasing a
rotating channel with a 125-symbol time constant — 52 ms at 2400 baud, over
which a 7 Hz offset turns 131°. The equaliser cannot follow that, so it
averages towards nothing useful and then has to unwind when the loop
eventually locks. This compounds directly with §3.1.

V.34 does it the other way round (`v34/receiver.rs:947-954`): the error is
rotated **back** into the equaliser's own frame with `e * spin.conj()` before
the tap update, so the equaliser never sees the carrier at all. The comment
at `v34/receiver.rs:948-949` states it as a rule: "The equaliser learns in
its own frame, before the carrier is taken out."

---

## 4. What the V.34 receiver has that the shared crate does not

Everything in this section is in `crates/datapump/src/v34/` and reachable
from nowhere else. For each: what it is, what moving it would cost, and what
the slow modes would get.

### 4.1 A fractional-delay interpolator (`v34/receiver.rs:457-472`, `:662-678`)

64 taps, 256 phases, Kaiser β = 8, cut at `0.5·baud·(1+β_rrc) + 300 Hz`,
normalised per phase. Interpolates **complex** samples at an arbitrary time
from a history deque, returning `None` if the filter would reach past either
end.

*Cost to share:* small and clean. It needs a constructor taking (cutoff, fs,
taps, phases) and a `at(history, time)` method. The awkward part is that it
reads from the caller's own history (`v34/receiver.rs:672-677` checks
`history_first` and `taken`), so a shared version either owns the history or
takes a slice plus an index base. Owning it is the better shape; that is a
~120-line module. `resample.rs` already has the windowed-sinc arithmetic but
in the wrong shape (it pushes output at a fixed ratio rather than answering
"what is the signal at time t"), and its window is Blackman rather than
Kaiser, so there is no reuse — this is a new module beside it.

*What the slow modes would get:* the 38 dB interpolator ceiling in §3.3 goes
away, and the same code becomes available to a V.17 receiver, which at 2400
baud would inherit the same ceiling.

### 4.2 A T/2 fractionally spaced NLMS equaliser (`v34/receiver.rs:909-954`)

31 taps at **half-symbol** spacing (`REACH = 15`, `v34/receiver.rs:62`), step
normalised by the row energy (`STEP = 0.02 / Σ|x|²`,
`v34/receiver.rs:950-951`), error rotated into the equaliser's frame, and
adaptation gated on the decision being credible (`squared < doubtful`, a
quarter of the squared distance to the nearest other point,
`v34/receiver.rs:928`, `:947`).

This is the single biggest difference. A **symbol-spaced** equaliser — which
is all `equalizer.rs` can be — cannot compensate for the sampling phase: it
sees the folded channel, and at the worst timing phase the fold puts an exact
null at the band edge (the two RRC skirts, equal at `baud/2`, arrive π out of
phase), which the equaliser must invert with unbounded gain. That is why the
slow modes depend utterly on `Gardner` having found the right instant — and
`Gardner` is the thing that is 1000× too timid at low level in V.22bis and
V.32 (§3.2). The two faults are the same fault.

*Cost to share:* moderate. The filter itself is a straightforward
generalisation of `Equalizer` — take a `spacing` (1 or 2 samples per tap) and
an NLMS step. Four things make it more than a rename:

1. Every caller's `feed` loop has to hand the equaliser **two** samples per
   symbol instead of one, which means the interpolate-and-tick block at
   `v22bis.rs:648-669` (and its three copies) has to be rewritten so the
   symbol instant and the half-symbol instant are both produced.
2. The equaliser's output must be sampled at the symbol rate while it is fed
   at twice it — a `Option<Complex>` return, like `Gardner::feed`.
3. NLMS needs the input energy, which is one running sum, but it changes what
   "step" means, so every mode's tuning has to be redone.
4. `least_squares` (`complex.rs:142`) plus the ridge rule
   (`v34/receiver.rs:1280-1284`) would come with it, to seed the taps from a
   known training sequence rather than crawl there blind — which is what
   makes V.34 train in 384 symbols where V.22bis takes 250 symbols blind plus
   125 tracking *after* the carrier has locked.

The prize is large enough to justify it: a T/2 equaliser plus a
least-squares seed removes the blind stage, the 0.25 handover threshold, the
R₂ table and the whole of §3.6.

### 4.3 A data-aided, derivative-based timing detector (`v34/receiver.rs:914-920`, `:959-967`)

The equaliser's own taps are applied to the **central differences** of the
half-symbol samples to get the output's rate of change; the error's component
along that rate of change is how late the sample was. Second order:
`TIMING_GAIN = 0.01` on the instant, `DRIFT_GAIN = 1.25e-5` on the rate,
drift clamped to `±0.001` = **±1000 ppm** (`v34/receiver.rs:967`), with the
slope tracked at α = 0.01 (`v34/receiver.rs:963`).

This is strictly better than Gardner where a decision is available: it is
unbiased for any constellation (Gardner assumes constant modulus — the
comment at `shaping.rs:321-328` records the 16-point thrashing), it needs no
power normalisation (the slope estimate does that job), and it gives a
calibrated ppm figure (`drift_ppm`, `v34/receiver.rs:628`) which no slow mode
has.

*Cost to share:* small, if 4.2 has already happened, because it needs the
equaliser's taps and the row of samples. On its own it is not shareable —
there is nothing to take the derivative of until the equaliser exists. So it
is a rider on 4.2, not a separate move.

### 4.4 Loss detection and hold (`v34/receiver.rs:930-945`, `Slicer::lost_threshold` `:191-198`)

A window of recent squared errors (length per slicer, `v34/receiver.rs:210`),
a `settled` reference tracked at α = 0.01 (`v34/receiver.rs:968`), and a
threshold on the ratio. When it trips, **every** loop is held and the
receiver rewinds.

The slow modes have nothing comparable. `Equalizer::error()` is the nearest
thing and it is a mean magnitude with no reference to compare against; it is
read only for display (`v22bis.rs:980`, `v27ter.rs:733`, `v29.rs:715`,
`v32.rs:1141`). V.22bis 6.4 requires a retrain "initiated by detection of
loss of equalization" and there is no `retrain` anywhere in `v22bis.rs` or
`v22bis/handshake.rs` (grep: zero hits).

*Cost to share:* small and worth doing first. It is about 40 lines: a
`Convergence` type holding the recent-error window, the settled reference and
the two thresholds, constructed from the constellation's minimum distance.
That same minimum distance is what §3.6 needs, so one change serves both. It
would give every slow mode a loss-of-equalisation signal, which is the
precondition for a V.22bis retrain.

### 4.5 Rewind (`v34/receiver.rs:971-986`, `:997-1026`)

Snapshots of every loop every 16 symbols, 24 kept (`EARLIER_EVERY`,
`EARLIER_KEPT`, `v34/receiver.rs:112-113`), so the receiver can put itself
back to before the symbols that taught it nonsense.

*Cost to share:* high, and probably not worth it. The snapshot is a struct of
this receiver's own fields (`Loops`, `v34/receiver.rs:441-451`) and a shared
version would need each loop object to be `Clone` and each mode to assemble
its own snapshot anyway. What *is* worth sharing is nothing; what is worth
copying is the idea, and only into V.32, which is the only slow mode that
runs long enough for it to matter.

### 4.6 Resync on a dense grid (`v34/receiver.rs:1027-1107`, `:1148-1216`)

Re-read the last 48 symbols from the raw mixed-down history at 16 fractions
of a half symbol and 1.5°-steps of phase, then 64 finer steps around the
best. This is the answer to the VoIP jitter slips the memory records
(~20 ms concealment inserts every few seconds).

*Cost to share:* high. It needs the raw history, the interpolator, the
equaliser and the slicer all at once, and it is inseparable from V.34's
`Heard`/`Mode3` structure. The shareable piece is the **raw mixed-down
history** — nothing below V.34 keeps one, so nothing below V.34 can read
anything twice. Giving `Resampler`-adjacent code a bounded history deque is
the enabling change; the search itself should stay where it is until a second
mode needs it.

### 4.7 An open-loop frequency measurement (`v34/probe.rs:188-243`)

Turn between consecutive FFT windows of the 1050 Hz probing tone, divided by
the window length, accepted when `|offset| < 10 Hz` (`v34/probe.rs:242`).
This is the mechanism §3.1 says V.22bis and V.32 are missing, in a form that
needs no decisions at all.

*Cost to share:* the arithmetic is six lines (`v34/probe.rs:188-196`) and is
already half-duplicated inside `ReversalDetector` as `drift`
(`tone.rs:353-360`), which measures the same quantity and then only uses it
to refuse. Exposing `ReversalDetector::drift_hz()` and letting V.32's
start-up seed `Receiver::phase`/`frequency` from it would be a ~20-line
change to the shared crate and a ~10-line change to `v32/startup.rs` — and
V.32's start-up already runs three `ReversalDetector`s on the carrier and
both sidebands (`v32/startup.rs:994-996`), so the measurement is already
being made and thrown away. **This is the cheapest large win in the tree.**

---

## 5. What the Recommendations require that the code does not do

### 5.1 ±7 Hz of received frequency offset

Rendered: V.22 bis PDF page 6 (document page 4), clause 2.6 — "The receiver
shall be able to operate with received frequency offsets of up to ± 7 Hz."
V.32 PDF page 5 (document page 1), clause 2.1 — "The carrier frequency is to
be 1800 ± 1 Hz. No separate pilot tones are to be provided. The receiver must
be able to operate with received frequency offsets of up to ± 7 Hz."
V.27 ter PDF page 7 (document page 5), clause 3 and V.29 PDF page 6 (document
page 4), clause 4 both say "at least ± 7 Hz", from ±1 Hz of transmitter
tolerance plus ±6 Hz of network drift.

V.27 ter and V.29 meet it by open-loop acquisition and have tests for it.
V.22bis and V.32 do not meet it (§3.1) and have no test. V.32bis at 14 400
misses it by a factor of seven.

### 5.2 The reversal detector's ceiling is 7.85 Hz

`tone.rs:399` refuses to judge a phasor turning faster than
`MAX_CARRY/delay`. With `MAX_CARRY = π/4` (`tone.rs:57`) and
`delay = ceil(6·fs/(2π·bw))` (`tone.rs:211`, `:216`), that is

    f_max = π·bw/24 = 7.854 Hz at bw = 60 Hz

independent of the sample rate. V.32 uses bw = 60 for all three reversal
detectors (`v32/startup.rs:994-996`) and V.34 phase 2 uses the same
(`v34/phase2.rs:148`, `:338`). So a line at the ±7 Hz that V.32 2.1 *requires*
sits at 89 % of the gate, with the drift estimate being a half-second
exponential average of a noisy phasor — and V.32's entire start-up, including
the round-trip measurement the echo canceller depends on, is conducted through
these reversals.

The crate's own tests bracket 3 Hz (pass, `tone.rs:549-551`) and 30 Hz
(refuse, `tone.rs:539-541`) and never touch 7.

Worse for V.34: INFOc bits 79:88 carry the measured 1050 Hz offset as a
signed integer from −511 to 511 in 0.02 Hz steps
(`docs/specs/text/T-REC-V.34-199802-I.txt:1977-1982`) — **±10.22 Hz** — and
the code implements that range (`v34/info.rs:120-128`) while
`v34/probe.rs:242` accepts up to 10 Hz. So V.34 contemplates offsets 30 %
beyond what its own phase-2 reversal detector will look at.

### 5.3 Circuit 109 thresholds, in dBm, and its response times

Rendered: V.22 bis PDF page 7 (document page 5), clause 3.3 —

> High channel threshold: greater than −43 dBm circuit 109 ON; less than
> −48 dBm circuit 109 OFF. [same for the low channel] The condition of
> circuit 109 between the ON and OFF levels is not specified except that the
> signal detector shall exhibit a hysteresis action, such that the level at
> which the OFF to ON transition occurs shall be at least 2 dB greater than
> that for the ON to OFF transition. Circuit 109 thresholds are specified at
> the input to the modem when receiving scrambled binary 1. … Circuit 109
> shall not respond to the 1800 Hz or the 550 Hz guard tones, or the 2100 Hz
> (nominal) answer tone during the handshake sequence.

Four things follow, and the code does none of them.

**(a) The thresholds are not calibrated.** `CARRIER_ON = 1.0e-3` is in units
of full scale, and nothing in the tree maps full scale to dBm. Every
transmitter here leaves at RMS 1/√2 (`tests/levels.rs:25`). A passband signal
of power 0.5 has a complex envelope of mean square 1.0, and the mixer's
low-pass keeps half of it, so `|selected|` has RMS 0.5 and — for the 16-point
set, whose mean magnitude is 0.947 of its RMS — a mean of **0.474**. The ON
threshold therefore sits `20·log10(1e-3/0.474)` = **−53.5 dB below what this
modem's own transmitter delivers at zero loss.**

V.2 1.2 (rendered, PDF page 3, document page 1) puts the maximum transmit
power at the zero relative level point at −13 dBm0, so V.22 bis's −43 dBm ON
threshold is 30 dB below a full-strength signal. The code turns on **23.5 dB
lower than that**. Concretely: a signal at exactly −48 dBm, which 3.3 says
shall read OFF, arrives as a `level` of 8.4e-3 — fifteen times the code's OFF
threshold of 5.62e-4, so it reads ON and keeps reading ON. The threshold was
chosen to sit above a real line's noise floor (`fsk.rs:121-131` records the
measurement, and the test at `fsk.rs:248-281` pins it at −46 dBFS of noise),
which is a reasonable engineering choice and is not the number the
Recommendation asks for; nothing in the tree records which of the two was
meant.

**(b) The hysteresis is 5.00 dB.** That satisfies "at least 2 dB". Both
`fsk.rs:45-48` and `v32.rs:288` claim it is "as V.22bis 6.5.2 asks for", and
`v22bis.rs:975` documents `carrier()` as "(V.22bis 6.5.2)". **There is no
clause 6.5.2.** V.22 bis 6.5 is "Operation after loss of line signal"; the
hysteresis requirement is 3.3. Three wrong citations plus a fourth in the
test at `fsk.rs:285-287`.

**(c) The response times are both outside the window.** V.22 bis 3.2 (PDF
page 6): circuit 109 "shall turn OFF 40 to 65 ms after the level … falls
below the relevant threshold" and "shall turn ON 40 to 205 ms after the level
… exceeds" it. With the 20 ms `OnePole` at `v22bis.rs:612`, falling from 0.47
to 5.62e-4 takes `20·ln(0.47/5.62e-4)` = **134 ms** (too slow), and rising
past 1e-3 from silence takes `20·ln(1/(1−1e-3/0.47))` ≈ **0.04 ms** (too
fast, by three orders of magnitude). There is no minimum-duration guard
anywhere.

**(d) The answer tone is in the passband.** `fir_lowpass(600.0, 401, fs)`
(`v22bis.rs:577`) measured at the offsets that matter:

| baseband offset | what it is | gain |
|---|---|---|
| 0 Hz | carrier | 0.00 dB |
| **300 Hz** | **2100 Hz answer tone in the 2400 Hz high channel** | **−0.01 dB** |
| 600 Hz | 1800 Hz guard tone in either channel | −6.03 dB |
| 675 Hz | the other channel's nearest edge | −54.84 dB |
| 1200 Hz | the other carrier | −69.09 dB |

So the calling modem's receiver, listening on the high channel, sees the
answering modem's 2100 Hz answer tone at full strength and asserts
`carrier()`. `v22bis.rs:642-647` then restarts `since_carrier`, and by the
time the real carrier arrives the gate at `v22bis.rs:753` is long open, so
the equaliser adapts straight into the acquisition transient — the exact
failure the comment at `v22bis.rs:690-693` says "was the whole of the trouble
a call starting after any real pause was having". The AGC half of it was
fixed; the detector half was not.

V.27 ter 5.2.1 (PDF page 7) adds the same shape of requirement for fax:
"Circuit 109 must turn ON after synchronizing is completed and prior to user
data appearing on circuit 104." The V.27 ter receiver turns it on from a
level threshold (`v27ter.rs:557-591`), which is before synchronising, not
after.

### 5.4 The fixed compromise equaliser

Rendered: V.22 bis PDF page 4 (document page 2), clause 2.3 — "Fixed
compromise equalization **shall** be incorporated in the modem transmitter."

Grep for "compromise" over `crates/**/*.rs` returns one hit, in an unrelated
comment (`gui/src/answer.rs:7`). There is no compromise equaliser, in the
shared crate or anywhere else. Clause 2.4 on the same page defines the
transmitted spectrum "excluding the characteristics of the fixed compromise
equalizer", so the mask in Figure 1/V.22 bis is measured before it — the
shaping in `rrc_taps` is the right pulse, and the compromise equaliser would
sit after it.

### 5.5 Retrain on loss of equalisation

V.22 bis 6.4 (`docs/specs/text/T-REC-V.22bis-198811-I.txt:556-593`): "A
retrain may be initiated during data transmission between two V.22 bis modems
if either modem incorporates a means of detecting loss of equalization." And
6.5 (`:598-606`): after a dropout, "If at any time after turning ON circuit
109 following a drop out the modem detects loss of equalization, it shall
proceed according to § 6.4 above", and circuit 104 stays clamped to binary 1
for 100 ms while the modem looks for a retrain sequence.

Grep for `retrain` in `v22bis.rs` and `v22bis/handshake.rs`: zero hits. The
one thing that would detect loss of equalisation — `Equalizer::reset` firing
on the `MAX_TAP_ENERGY` guard (`equalizer.rs:144-146`) — does so **silently**;
the only way out is `is_blind()`, read for display and nothing else.

---

## 6. Weaknesses

Ordered by what a user would notice.

### 6.1 V.32bis at 12 000 and 14 400 cannot acquire a real line's frequency offset — high

*Where:* `v32.rs:1039-1041`, gains `0.008·bw` / `1.5e-5·bw²` with `bw` from
`v32.rs:270-277`; no open-loop acquisition anywhere in `v32.rs` or
`v32/startup.rs`.

*Symptom:* the modem trains, reports CONNECT 14400, and then produces
garbage, or falls back to 9600 and then to 4800 for no visible reason, on
every line that is not frequency-exact. On a VoIP path it is worse, because
the transcoding chain adds offset. The constellation display shows a slowly
rotating cloud.

*How I know:* simulated the exact difference equations against the real
constellation tables, §3.1 and §7. At bw = 0.247 (14 400) even 1 Hz takes
51 s and 3 Hz never converges in 250 s of clean signal. V.32 2.1 requires
±7 Hz. There is no test in the tree that applies any offset to V.32.

### 6.2 V.22bis takes 41 s to pull in the offset the Recommendation requires — high

*Where:* `v22bis.rs:728-731`.

*Symptom:* a V.22bis call to a real far end connects and then sits producing
nothing readable for tens of seconds before suddenly coming good; or the far
end gives up first and the call reads as "connected, no data". On a loopback
it is invisible, because both ends share one oscillator.

*How I know:* §3.1 table; 1 527 symbols at 1 Hz, 24 892 at 7 Hz. V.22bis 2.6
requires ±7 Hz (rendered).

### 6.3 `Gardner` starts its normaliser at 1.0, and V.22bis and V.32 never compensate — high

*Where:* `shaping.rs:282` and `:337`; used raw at `v22bis.rs:667-668` and
`v32.rs:985-986`; worked around at `v27ter.rs:843-845` and `v29.rs:832-839`.

*Symptom:* on a quiet line (30 dB down is ordinary after a long haul), the
receiver samples wherever the filters' group delay happened to leave it and
stays there. Because the equaliser is symbol-spaced (§4.2), a half-symbol
error puts a null at the band edge of the folded channel and the eye never
opens. The user sees a call that connects on a good line and refuses to on a
quiet one, with no difference in the reported level.

*How I know:* the fault is described in the tree's own words at
`v29.rs:820-826`, with the fix applied to two modes out of four. The
arithmetic: 345 symbols to come within 2× of the true power, and
`(sps/2)/1e-4` = 33 000 symbols to cross half a symbol while the error is
1000× shrunk.

### 6.4 The blind/decision-directed handover threshold is a bare 0.25 — high

*Where:* `equalizer.rs:121`.

*Symptom:* V.32bis at 12 000 and 14 400 hands the equaliser over to decision
direction on a closed eye and then reinforces its own wrong decisions. The
constellation display shows points collapsing onto the wrong grid and staying
there. Since there is no loss-of-equalisation detector (§4.4) and no retrain
(§5.5), nothing recovers it.

*How I know:* computed the minimum distance of each constellation from the
tables in `v32/trellis.rs`, normalised as each receiver normalises them.
0.25 is 113 % of the point spacing at 14 400 and 81 % at 12 000 (§3.6 table).
The file's own header (`equalizer.rs:15-18`) says what happens then.

### 6.5 `EchoFinder` is O(round-trip × training) and a VoIP round trip is 3 s — high

*Where:* `echo.rs:351-359` (one multiply-accumulate per candidate delay per
sample), sized at `v32/startup.rs:2272-2282` from `round_trip_samples() +
far_taps()` with **no cap**.

*Symptom:* on a VoIP line the V.32 start-up stalls at the training segment,
audio underruns, the call drops. On an ordinary line nothing happens, so it
will never show up in testing here.

*How I know:* arithmetic. Ordinary line: round trip ~80 symbols → 533
samples, plus 64 far taps, less the 128 the near run already covers → 470
candidates × 13 653 samples of `search_for` (`v32/startup.rs:2280-2281`,
`SEGMENT_TRN_LONG = 4096` at `v32/startup.rs:825`) = **6.4 M** MACs over
0.85 s, trivial. VoIP at 1.5 s each way (the figure in
`memory/voip-line-round-trip.md`): round trip 3 s = 48 000 samples → 47 937
candidates × 13 653 = **654 M** MACs, to be done inside the 0.85 s that
`search_for` spans — **767 M MAC/s**, each one a bounds-checked `VecDeque`
index. That will not run. The `scores` and `history` vectors are 383 KB each,
so it is memory-bandwidth bound as well as arithmetic bound.

### 6.6 V.32 never tells the equaliser which constellation it is on — medium

*Where:* `v32.rs:909` passes `1.0` once; `v32.rs:949-959` (`follow`)
recomputes `bandwidth` on every rate change and leaves the equaliser alone.
V.29 does the right thing at `v29.rs:677-683`.

*Symptom:* the blind stage settles 12.6 % small (−1.18 dB) at every rate
above 4800. At 14 400 that puts the outer ring 0.207 out of place against a
half-spacing of 0.110, so the first few hundred symbols after handover are
all wrong on the outer points. Visible as a constellation whose corners are
smeared inwards while the middle is clean.

*How I know:* computed R₂ for all four tables (1.320, 1.310, 1.381, 1.343)
and solved the CMA equilibrium `g²·R₂_true = R₂_target`.

### 6.7 V.22bis never tells the equaliser it fell back to 1200 — medium

*Where:* `v22bis.rs:599` passes 1.32 once; `Receiver::set_rate`
(`v22bis.rs:920-922`) and the automatic fallback at `v22bis.rs:847-848` both
set `self.rate` and nothing else.

*Symptom:* at 1200 bit/s the constellation is four constant-modulus points
(`v22bis.rs:1018-1036`, the `01` point of each quadrant) whose true R₂ is
1.0. Against a target of 1.32 the blind stage converges 15 % **large**, which
the decision-directed stage then has to pull back at 4e-3 — about 125 symbols
= 208 ms of wrong decisions on every fallback.

*How I know:* same calculation as 6.6, and `v22bis.rs:1093-1100` confirms the
1200 bit/s point carries the RMS of the whole constellation, so the set is
constant-modulus.

### 6.8 The carrier detector asserts on the 2100 Hz answer tone — medium

*Where:* `v22bis.rs:577` (the select filter), `:633-641` (the only carrier
test). Measured gain at that offset: −0.01 dB.

*Symptom:* CD comes up during the answer tone, so `since_carrier` runs out
before the real carrier arrives and the equaliser adapts into the acquisition
transient; and upstream, `modem/src/lib.rs:1916` sees a carrier where the
Recommendation says there is none.

*How I know:* computed the filter's response (§5.3(d)); V.22 bis 3.3
rendered; no answer-tone exclusion exists anywhere in `v22bis.rs` or
`v22bis/handshake.rs`.

### 6.9 `ReversalDetector` refuses at 7.85 Hz, and V.32 requires 7 — medium

*Where:* `tone.rs:57`, `:399`.

*Symptom:* intermittent — on a line near the allowed limit, V.32's start-up
misses the AA-to-CC transition or the round-trip mark, and the modem sits in
a start-up state until it gives up. `v32/startup.rs:2074` subtracts the
detector's latency from a measurement that was never made.

*How I know:* `f_max = π·bw/24 = 7.854 Hz` derived from `tone.rs:211`,
`:216`, `:399`; V.32 2.1 rendered; the tests bracket 3 and 30.

### 6.10 The linear interpolator caps the 2400-baud modes at 38 dB — low

*Where:* `v22bis.rs:662-666`, `v27ter.rs:830-834`, `v29.rs:814-818`,
`v32.rs:980-984` — four identical copies.

*Symptom:* the SNR a clean V.32 or V.29 call reports never exceeds about
38 dB, and about half a decibel of margin is gone at 14 400. Not visible as a
failure; visible as a number that will not go up.

*How I know:* measured, §3.3 table.

### 6.11 Echo canceller step is five times the crate's own guidance, and never reduced — low

*Where:* `echo.rs:137-138` recommends ~0.1; `v32/startup.rs:2174` passes 0.5;
`v32/startup.rs:2240` freezes the taps at the end of training without
reducing it.

*Symptom:* the achieved echo return loss is a few decibels worse than it
could be, for the whole call. On a line where the echo is close to the far
end's level, that is the difference between 9600 and 4800.

*How I know:* NLMS misadjustment `μ/(2−μ)`: 0.333 at 0.5, 0.053 at 0.1 —
8.0 dB.

### 6.12 Dead API and stale citations — low

- `Nco::set_frequency`, `Nco::adjust`, `Nco::frequency` (`nco.rs:22-26`) —
  no callers anywhere.
- `OnePole::set` (`filter.rs:137`) — no callers.
- `Equalizer::with_steps` (`equalizer.rs:57-61`) — no callers; every mode
  runs the defaults.
- `fir_lowpass_kaiser` (`shaping.rs:121`) — no callers; only the comment at
  `v22bis.rs:566`.
- `solve_hermitian` (`complex.rs:167`) — only called by `least_squares`.
- `FskDetector::slow_env` (`fsk.rs:38`, `:81`, `:105`) — written every
  sample, never read.
- "V.22bis 6.5.2" at `fsk.rs:45-48`, `v32.rs:288`, `v22bis.rs:975`,
  `fsk.rs:285-287` — the clause does not exist; the requirement is 3.3.

### 6.13 `Fir::process` does two modulos per tap — low

*Where:* `shaping.rs:184-197`. V.22bis's select filter is 401 taps through
`ComplexFir`, so 401 × 2 × 16 000 = **12.8 M** iterations a second each doing
`idx = (idx + 1) % n`, plus 213 more taps for the matched filter — about
20 M modulo operations a second per receiver, doubled when the GUI runs both
ends. `Nco::step` (`nco.rs:35-36`) calls `cos` and `sin` separately rather
than `f64::sin_cos`; V.32's start-up runs eight `ToneDetector`s
(`v32/startup.rs:163-178`), so 16 transcendental calls a sample = 256 k/s per
direction.

*Symptom:* CPU, not correctness. Relevant because 6.5 is already on the edge.

### 6.14 No tap leakage, no re-centring, no way to seed — low, unproven

*Where:* `equalizer.rs:109-147`. The only guard is the blow-up reset at
`equalizer.rs:144-146`.

*Symptom (predicted, not observed):* on a long call the tap energy can walk
away from the centre, because the timing loop and the equaliser both control
delay and nothing couples them. The failure mode would be a constellation
that degrades over minutes and never recovers, with no retrain to rescue it.
I have not demonstrated this; I flag it because nothing in the tree measures
the tap centroid, so it would not be noticed if it happened.

---

## 7. How the numbers were got

Everything here was computed from the code in this tree or read off a
rendered PDF page. Nothing was consulted outside `docs/specs` and
`F:\dialupmodem2`.

- **Carrier loop pull-in (§3.1):** the exact difference equations transcribed
  from `v22bis.rs:720-731` and `v32.rs:1018-1047`, driven by a random
  constellation sequence with a pure frequency offset applied, no noise and
  no channel. The V.32 constellations were parsed out of
  `v32/trellis.rs:70-440` (`POINTS_16/32/64/128`), scaled by the `scaled()`
  factors at `v32/trellis.rs:424-428` and divided by `CONSTELLATION_RMS`, and
  the per-rate `bw` was recomputed from each set's own minimum distance
  exactly as `loop_bandwidth` (`v32.rs:270-277`) does. "Locked" means a
  0.01-smoothed mean `|z − decision|` below 0.02 held for 400 symbols. These
  are optimistic numbers: with noise and a real channel they get worse, and
  with the equaliser fighting the loop (§3.7) worse again.
- **R₂ and point spacing (§3.6):** the same parsed tables, mean power
  confirmed to be 1.000 for all four after scaling.
- **Interpolator error (§3.3):** a raised-cosine baseband built from
  `shaping.rs:35-52`'s own `rrc_at` at each mode's `sps`, `ROLLOFF` and
  `SPAN`; the linear interpolation of `v32.rs:980-984` compared against the
  exact pulse-sum value at the same instant, over 3000 symbols.
- **Filter responses (§5.3(d)):** `fir_lowpass` transcribed from
  `shaping.rs:64-86` and evaluated by direct DFT of the taps.
- **The 7.854 Hz gate (§5.2):** algebra from `tone.rs:211`, `:216`, `:399`
  and `MAX_CARRY` at `tone.rs:57`; the `fs` cancels.
- **Rendered pages:** V.22 bis PDF pages 4, 6, 7 (clauses 2.1-2.5, 2.6, 3.2,
  3.3, Figure 1, Table 2); V.32 PDF page 5 (clause 1, 2.1); V.27 ter PDF page
  7 (Table 5, clauses 3, 4, 5.2.1); V.29 PDF page 6 (Table 3, Figure 3,
  clauses 3, 4); V.2 PDF page 3 (clauses 1.1, 1.2). Every figure and every
  numeric threshold quoted above was read off the rendered page, not the
  extracted text; the extracted text was used only to find the clause.
- **Test run:** `cargo test -p dsp --lib` — 75 passed, 0 failed, 1.62 s;
  3 complex, 11 echo, 5 equalizer, 6 fft, 5 filter, 5 fsk, 3 nco, 5 resample,
  18 shaping, 14 tone.

---

## 8. If only three things are done

1. **Seed the carrier loop.** `ReversalDetector` already measures the offset
   (`tone.rs:353-360`) and throws it away; V.32's start-up already runs three
   of them (`v32/startup.rs:994-996`). Expose `drift_hz()`, seed
   `v32.rs`'s `frequency` from it, and give V.22bis a V.29-style open-loop
   estimate off its own unscrambled-ones segment. §6.1, §6.2, §5.1.
2. **Give `Equalizer` the constellation, not just R₂.** One extra
   constructor argument — the minimum distance — fixes the 0.25 handover
   (§6.4), and rebuilding it in `V32::Receiver::follow` (`v32.rs:949-959`)
   and `V22bisReceiver::set_rate` (`v22bis.rs:920-922`) fixes §6.6 and §6.7.
   The same number is what a loss-of-equalisation detector (§4.4) needs.
3. **Scale the Gardner input in V.22bis and V.32,** the way V.27 ter and V.29
   already do, or better, fix it in `Gardner` itself so no mode can get it
   wrong again. §6.3.

Everything else in §4 — the T/2 equaliser, the polyphase interpolator, the
data-aided timing — is a larger rewrite of the four `feed` loops, and should
wait until these three have been measured on a real line.
