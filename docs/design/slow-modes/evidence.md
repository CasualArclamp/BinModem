# The slower modes: what holds them locked, and what does not

What this is: an inventory of the evidence that exists today for Bell 103, V.21,
V.22bis and V.32/V.32bis, a reading of the mechanisms that keep those receivers
locked with the actual gains and time constants, the weaknesses those numbers
predict and measurement confirms, and the design of the harness this work needs.

Everything below was read out of the source or measured. Measurements were taken
with a scratch binary built against `crates/datapump` and `crates/dsp`
unmodified; nothing in the repository was changed to obtain them. Where a number
comes from a Recommendation the clause is named and the page was rendered and
read, not taken from `docs/specs/text`.

---

## 1. What exists today

### 1.1 The tests, and what each one really proves

`crates/datapump/tests` holds 24 files. Eight of them concern the older modes.
The whole suite passes: 294 unit tests plus the integration tests, 0 failures.

**Bell 103**

- `crates/datapump/tests/bell103_loopback.rs:68,91,117,131` — four tests. Two
  modems on a shared line, each hearing the far end at `FAR` and its own
  transmit at `ECHO = 0.251` (−12 dB, `bell103_loopback.rs:20`). They prove the
  call comes up without a handshake, that characters cross, that a call to
  nobody gives up, and that a 2100 Hz answer tone is waited out rather than
  talked over. No noise, no frequency offset, no clock offset.
- `crates/datapump/tests/bell103_vector.rs:34,53,68` — the only real recording
  evidence for 300 bit/s: `tests/vectors/bell103-300.wav`, a 12.8 s cut of a
  2005 Conexant softmodem's login session. It proves the receiver reads a real
  line in both bands and that the password is not echoed. It is one recording of
  one line in one condition.

**V.22bis**

- `crates/datapump/tests/v22bis_loopback.rs` — 22 tests, the richest file in the
  set. Worth naming what is genuinely covered:
  - `:249 the_signal_may_arrive_at_any_moment` sweeps the arrival offset across
    a whole symbol. This is the test that caught the timing loop sampling
    wherever group delay left it.
  - `:269 the_two_clocks_need_not_agree` runs ±100 and ±200 ppm. See §3.6: this
    test does not model what it says it models.
  - `:343,367,381,438,447,513,526,582,592` model the modem's own transmit
    leaking into its receive band, at a swept level, with and without µ-law
    companding (`:412 ulaw_roundtrip`). This is real and valuable: it is the
    only non-linearity anywhere in the older-mode tests.
  - `:640 the_offer_of_2400_is_read_from_every_starting_phase` is the only test
    in the whole set that adds noise — `0.04 *` a xorshift, at
    `v22bis_loopback.rs:647`, a fixed level with no stated signal-to-noise
    ratio, used to check that the 2400 offer is read from every starting phase.
- `crates/datapump/tests/v22bis_handshake.rs` — 8 tests. `:210` puts the
  modem's own signal 12 dB over the far end's and proves the call still comes
  up. `:265,295` prove the rate ceiling is honoured.
- `crates/datapump/tests/v22bis_vector.rs` — 5 tests against
  `tests/vectors/v22bis-2400.wav`. The strictest ground truth the project has
  for this mode: it recovers V.42 frames whose check sequences hold, so the
  decode is right to the bit. Note `:97 the_rate_in_use_is_reported_as_1200` —
  the recording is a 1200 bit/s link.
- `crates/datapump/tests/v22bis_capture.rs` — `#[ignore]`, and points at
  `live-1788613347.wav` / `live-1788775243.wav`. **Neither file is in
  `dist/captures`.** All 31 captures there are `live-17895…` and later, which
  are V.34 and V.90. This test cannot be run.

**V.32 / V.32bis**

- `crates/datapump/tests/v32_loopback.rs` — 10 tests. Transmitter into receiver
  at 4800, plus `:157,170,195` which put the signal through a hybrid with and
  without echo cancellation, and `:245` which runs 9600 with trellis coding.
  The 9600 test uses a 256-byte lead-in and one arrival phase; see §3.1.
- `crates/datapump/tests/v32_call.rs` — 23 tests, the most thorough file in the
  project. Two whole `startup::Modem`s over a line that reflects. Impairments
  modelled: near echo at a chosen level (`:16 ECHO = 0.251`), far end at `:19
  FAR = 0.1`, a far hybrid reflection `TALKER`, and a one-way delay. `:477`
  sweeps the round trip from 40 ms to 400 ms — the VoIP case. `:496` runs the
  echo at unity (`CABLE_ECHO = 1.03`), which is a virtual cable. `:721` reaches
  9600 with trellis coding, `:781,901,1031` cover retrains. **No noise, no
  carrier frequency offset, no clock offset, no level step, no dropout.**
- `crates/datapump/tests/v32_startup.rs` — 5 tests on the start-up state
  machine, round-trip measurement, and a far end that starts over.
- `crates/datapump/tests/v32_signals.rs` (12), `v32_rate_framing.rs` (3),
  `v32_reversals.rs` (2, both `#[ignore]`) — the line signals and the rate
  sequences, checked by correlation. These test the transmitter and the tables,
  not the receiver.
- `crates/datapump/tests/v32_vector.rs` — 3 tests against
  `tests/vectors/v32bis-14400.wav`. **They never demodulate it.** The module
  comment at `v32_vector.rs:3` says "which this crate cannot yet demodulate",
  which is now stale: `crates/datapump/src/v32/trellis.rs:440` implements
  `AT_14400`. The only real V.32bis signal in the repository is checked with a
  correlator for tones and spectrum, and is never put through the receiver.
- `crates/datapump/tests/v32_replay.rs` — `#[ignore]`, points at
  `live-1788836496.wav`, which is not in `dist/captures`. Cannot be run.

**V.27 ter, V.29, V.17** — `crates/datapump/tests` contains no file for any of
them. Their only tests are in-module: 15, 14 and 8 `#[test]`s in
`crates/datapump/src/v27ter.rs`, `v29.rs`, `v17.rs`. There is no fax-mode
recording in `tests/vectors` and no fax call in `dist/captures`.

**V.21** — 6 in-module tests in `crates/datapump/src/v21.rs`. No integration
test, no recording.

**`crates/datapump/tests/levels.rs`** — 5 tests, transmit level only.

### 1.2 What the simulated lines model

Grepping `noise|delay|offset|echo|ppm` across the older-mode tests turns up
exactly three impairments, everywhere:

| impairment | where | range covered |
|---|---|---|
| own echo at a fixed level | `bell103_loopback.rs:51`, `v22bis_handshake.rs:230`, `v32_call.rs:31` | −12 dB fixed, and swept 0…+12 dB in `v22bis_loopback.rs:544` |
| round-trip delay | `v32_call.rs:455` | 40–400 ms |
| a second reflection (far hybrid) | `v32_call.rs:465` | one level |
| µ-law companding | `v22bis_loopback.rs:412` | on/off |
| additive noise | `v22bis_loopback.rs:647` | one fixed amplitude, one test |
| clock offset | `v22bis_loopback.rs:280` | ±200 ppm, and confounded (§3.6) |

### 1.3 Recordings

`tests/vectors/` holds `bell103-300.wav`, `v22bis-2400.wav`,
`v32bis-14400.wav`, all cut from one 2005 Conexant softmodem driven with
`AT+MS`. `dist/captures/` holds 31 live calls, **none of which is Bell 103,
V.21, V.22bis, V.27ter, V.29, V.17 or V.32** — the two replay harnesses written
for the older modes (`v22bis_capture.rs`, `v32_replay.rs`) have no data left to
run against.

### 1.4 What is not tested today — plainly

- **Carrier frequency offset: nothing, for any older mode.** V.22bis 2.6 and
  V.32 2.1 both require ±7 Hz. Not one test applies any.
- **Additive noise at a stated signal-to-noise ratio: nothing.** One test uses
  one unnamed noise amplitude.
- **Phase jitter: nothing, anywhere in the repository.**
- **Sampling clock offset: only V.22bis, only ±200 ppm, and confounded with a
  carrier offset (§3.6). Nothing for V.32, Bell 103, V.21, V.27ter, V.29.**
- **Level steps and dropouts: nothing.** The VoIP jitter-slip and gain-control
  work all lives in `v34_capture.rs` and `v90_call.rs`; none of it was ever
  pointed at the older modes.
- **Arrival phase: V.22bis only** (`v22bis_loopback.rs:249`). Not V.32. This is
  where the worst fault is (§3.1).
- **Rates: V.32 7200, 12 000 and 14 400 are never demodulated by any test.**
  `v32_loopback.rs` goes to 9600; `v32_call.rs:721` reaches 9600 with trellis;
  `v32_vector.rs` holds a 14 400 recording it never decodes.
- **Modes: V.27 ter, V.29 and V.17 have no integration test at all**, at any
  rate, over any line.
- **Carrier detection against its own Recommendation: nothing.** V.22bis 3.2
  gives turn-on and turn-off times and 3.3 forbids responding to the answer
  tone. Nothing checks either. Both are violated (§3.4).

---

## 2. The mechanisms, with their numbers

### 2.1 Bell 103 and V.21 — the discriminator chain

`crates/dsp/src/fsk.rs`. Band isolate, quadrature down-convert to band centre,
low-pass, phase-difference, post-filter.

- Band filter: order 8 Butterworth band-pass, half-width
  `deviation + 0.9 * baud` (`fsk.rs:63`), so ±370 Hz about the band centre at
  300 baud. Order 8 rather than 4 buys 34 dB of rejection of the other band
  instead of 17 (`fsk.rs:66`); measured in `crates/dsp/src/filter.rs:192`, the
  worst originate tone is rejected by more than 20 dB.
- Complex low-pass: order 4 at `baud * 1.3` = 390 Hz (`fsk.rs:73`).
- Post-detection low-pass: order 2 at `baud * 0.8` = 240 Hz (`fsk.rs:75`).
- Envelope: a 5 ms one-pole (`fsk.rs:80`). A 250 ms one exists
  (`fsk.rs:81`) and is computed but never read — dead state left from the
  ratio detector the comment at `fsk.rs:122` describes tearing out.
- Carrier: fixed thresholds, `CARRIER_ON = 1.0e-3`, `CARRIER_OFF = 5.62e-4`
  (`fsk.rs:49`), 5 dB apart.

Bit timing at 300 bit/s is not a loop at all: `AsyncFramer`
(`crates/datapump/src/framing.rs:34`) re-acquires on every start bit, confirms
it at the half-bit point (`framing.rs:70`) and samples bit *n* at
`sps * (1.5 + n)` (`framing.rs:84`). A clock offset of ε costs
`10.5 * sps * ε` samples by the stop bit — at 1000 ppm and 53.33 samples per
bit that is 0.56 samples out of 53, so async framing tolerates clock offsets of
several thousand ppm by construction. **This is why Bell 103 is the one older
mode with no timing weakness.**

V.21 carries synchronous HDLC, so it does need a bit clock:
`crates/datapump/src/v21.rs:183–204`. It starts half a bit after the first
transition (`v21.rs:191`) and nudges 12.5 % of the way towards the ideal at
every transition (`PULL`, `v21.rs:156`). First order, no integral. With a
transition every two bits on flag traffic, a clock offset ε leaves a standing
sampling error of about `2 * sps * ε / PULL = 16 * sps * ε` samples — at
1000 ppm that is 0.85 of 53.33 samples, 1.6 % of a bit period. Adequate.

### 2.2 V.22bis — the three loops

`crates/datapump/src/v22bis.rs`.

**Channel selection.** 401-tap linear-phase low-pass at 600 Hz, applied to the
complex baseband (`v22bis.rs:577`). 25 ms of span, 12.5 ms of group delay. The
delay sits in no feedback loop but it does delay the carrier flag (§3.4).

**Matched filter.** Root-raised-cosine, roll-off 0.75 (`v22bis.rs:26`), span 8
symbols (`v22bis.rs:28`).

**Timing.** `Gardner::new(sps, 0.1)` at `v22bis.rs:587`, `sps = 26.667`.
In `crates/dsp/src/shaping.rs`: the error is normalised by a running mean power
with a 0.02 per-symbol smoother (`shaping.rs:337`) and clamped to ±1
(`shaping.rs:338`); the integral gain is `gain / 100` = 1e-3
(`shaping.rs:276`); the integral is clamped to ±`sps/8` = ±3.33 samples
(`shaping.rs:349`) and the total correction to ±`sps/4` = ±6.67 samples
(`shaping.rs:351`). Two intervals per symbol, so the integral alone can hold a
rate correction of 2 × 3.33 = 6.67 samples per symbol — 250 000 ppm. **The
integral clamp is nowhere near being the limit on clock offset.**

The wanted instant is reached by **linear interpolation** between two
matched-filter outputs (`v22bis.rs:662–666`). For a component at frequency *f*
sampled at *fs*, linear interpolation at the mid-point is low by
`1 − cos(π f / fs)`. The highest baseband component here is
`600/2 × (1 + 0.75)` = 525 Hz, so the error is
`1 − cos(π·525/16000)` = 0.53 %, −45.5 dB. **Harmless at V.22bis.** It is not
harmless at V.32 (§2.3).

**Gain control.** `OnePole::starting_at(10.0, 0.050, fs/sps)` at
`v22bis.rs:594` — the rate handed in is the *symbol* rate, 600, so the time
constant is 0.050 s = 30 symbols. Gain is
`sqrt(10 / mean_power)`, clamped to 400 (`v22bis.rs:706`).

**Carrier.** Decision-directed, second order, on the unequalised symbol
(`v22bis.rs:720–731`):

```
error      = Im(point · conj(decision)) / |decision|²      ≈ sin Δθ
frequency += −1.5e-5 · error        clamped ±0.02 turns/symbol
phase     += −0.008  · error + frequency
```

`phase` and `frequency` are in turns; `error` is in radians. Converting the
detector gain (2π radians of error per turn of phase error) gives, in turns:

| | value |
|---|---|
| proportional gain Kp | 0.008 × 2π = **0.0503** turns per turn |
| integral gain Ki | 1.5e-5 × 2π = **9.43e-5** turns/symbol per turn |
| natural frequency ωn = √Ki | 9.71e-3 rad/symbol → **0.93 Hz** at 600 baud |
| damping ζ = Kp / (2√Ki) | **2.59**, overdamped |
| noise bandwidth ωn(ζ + 1/4ζ)/2 | **1.24 Hz** |
| hold-in, from the ±0.02 clamp | **±12 Hz** |

The number that matters is not the hold-in but the **pull-in**, and for a
decision-directed loop that is set by how far a symbol may turn before the
slicer starts getting it wrong. The loop's proportional term alone holds a
standing offset of `0.008 · sin Δθ · 600` Hz = `4.8 · sin Δθ`.

- At 1200 bit/s the slicer offers four points 90° apart
  (`v22bis.rs:1018–1026`), so `Δθ ≤ 45°` and the pull-in is
  **4.8 × 0.707 = 3.4 Hz**.
- At 2400 bit/s the sixteen points are on a grid of 2. The tightest is the
  corner (3,3): rotate it and its real part falls below the decision boundary
  at 2 when `3(cos φ − sin φ) = 2`, i.e. φ = **16.8°**. So the pull-in is
  **4.8 × sin 16.8° = 1.39 Hz**.

Measured (§3.3): the 2400 bit/s link is perfect at 1.5 Hz and broken at 2.0 Hz.
The prediction and the measurement agree to within the sweep step.

**Equaliser.** `Equalizer::new(21, 1.32)` at `v22bis.rs:599`. 21 symbol-spaced
taps at 600 baud is ±10 symbols = **±16.7 ms**, which is generous for a
telephone line. Modulus 1.32 is right for the sixteen points and wrong for the
four that 1200 bit/s uses, whose fourth-over-second moment is 1.0 — the blind
stage therefore inflates a 1200 bit/s constellation by `√1.32` = 15 %. Harmless
for data, because the 1200 slicer decides by angle, but it biases
`residual_error()` and any display built on it.

Adaptation is gated on `since_carrier > 64` symbols (`v22bis.rs:753`), about
107 ms.

**Rate detection.** `v22bis.rs:840–855`: over 128 symbols (213 ms at 600 baud),
accumulate the mean and mean-square of the pre-equaliser power and call it
1200 if the relative variance is below 0.16. Re-decided every 128 symbols, and
gated on the carrier flag (`v22bis.rs:831`) — which matters, because the
carrier flag is wrong during the answer tone (§3.4).

### 2.3 V.32 / V.32bis — the same three loops, on a quarter of the oversampling

`crates/datapump/src/v32.rs`. Structurally the V.22bis receiver, with the
channel filter widened to a plain anti-alias low-pass (`v32.rs:901`, 121 taps at
1600 Hz) because the far end is not in a neighbouring channel, it is on top of
us.

The decisive difference is arithmetic: 2400 baud at 16 kHz is
**6.667 samples per symbol**, against V.22bis's 26.667.

**Interpolation.** Same linear interpolation between matched-filter outputs
(`v32.rs:980–984`). Highest baseband component is `2400/2 × 1.25` = 1500 Hz, so
the mid-point error is `1 − cos(π·1500/16000)` = **4.3 %, −27.3 dB**. That is a
floor on every symbol, independent of the line, and it sits directly on top of
the 26–28 dB that 14 400 needs.

**Equaliser.** `Equalizer::new(21, 1.0)` at `v32.rs:909`. Two problems:

1. **Span.** 21 symbol-spaced taps at 2400 baud is ±10 symbols = **±4.17 ms**.
   A national GSTN connection is allowed 1.5–3 ms of group-delay variation
   across the band before anything unusual has happened. V.27 ter and V.29 both
   use 31 taps (`v27ter.rs:679`, `v29.rs:639`). V.32 is the mode with the
   hardest job and the shortest equaliser in the project.
2. **Modulus.** 1.0 is the constant-modulus target for four points of equal
   radius, and is wrong for every other constellation V.32 uses. Measured from
   the code's own point tables:

   | constellation | wanted R₂ | code uses | half-spacing | peak/rms |
   |---|---|---|---|---|
   | 4800, four points | 1.000 | 1.0 | 0.7071 | 1.000 |
   | 9600 uncoded, Figure 2, sixteen | 1.320 | 1.0 | 0.3162 | 1.342 |
   | 7200 trellis, sixteen | 1.320 | 1.0 | 0.3162 | 1.342 |
   | 9600 trellis, thirty-two | 1.310 | 1.0 | 0.2236 | 1.304 |
   | 12 000, sixty-four | 1.381 | 1.0 | 0.1543 | 1.528 |
   | 14 400, one hundred and twenty-eight | 1.343 | 1.0 | 0.2209/2 = 0.1104 | 1.440 |

   A Godard update driving towards R₂ = 1.0 when the constellation's own R₂ is
   1.32 settles at a scale `g` minimising `E[(g²P − 1)²]`, which gives
   `g² = E[P]/E[P²] = 1/1.32`, so **`g` = 0.870: the constellation comes out
   13 % small**. At 14 400 the outermost point sits at 1.440 in normalised
   units, so 13 % moves it 0.187 — **nearly two decision boundaries** (half
   spacing 0.110). `crates/datapump/src/v29.rs:663–684` already computes the
   modulus per rate and rebuilds the equaliser when the rate changes; V.32
   never does, and `Receiver::follow` (`v32.rs:949`) — which is exactly where
   the rate change is handled — touches the trellis decoder and the loop
   bandwidth but not the equaliser.

**The blind-to-decision-directed handover.** `crates/dsp/src/equalizer.rs:121`:

```rust
if self.blind && self.error_average < 0.25 { self.blind = false; }
```

`error_average` is the running mean distance from a decision
(`equalizer.rs:116`, 0.01 per symbol, so a 100-symbol window), in the same
normalised units as the table above. **0.25 is an absolute constant, and the
distance at which a decision stops being trustworthy is half the point
spacing.** Reading the table: the handover fires at 0.25 while the boundary is
at 0.2236 (9600 trellis), 0.1543 (12 000) and 0.1104 (14 400). At those three
rates the equaliser abandons the blind criterion *while the eye is still shut*
and then reinforces whatever it is deciding. That is the trap the doc comment
at `equalizer.rs:18` warns about, wired in as a constant.

**Carrier loop.** `v32.rs:1028–1047`, the same shape as V.22bis with two
additions: the error is smoothed by `TRACK_SMOOTHING = 0.1` per symbol
(`v32.rs:285`), a 10-symbol lag, and both gains are scaled by a bandwidth
factor (`v32.rs:270`) that shrinks as the constellation crowds:

| rate | bw | ωn | pull-in |
|---|---|---|---|
| 4800 | 1.000 | 3.71 Hz | 19.2 × sin 45° = **13.6 Hz** |
| 7200 | 0.707 | 2.62 Hz | 13.6 × sin(0.3162/1.342) = **3.2 Hz** |
| 9600 uncoded | 1.000 | 3.71 Hz | 19.2 × sin 16.8° = **5.6 Hz** |
| 9600 trellis | 0.500 | 1.86 Hz | 9.6 × sin(0.2236/1.304) = **1.6 Hz** |
| 12 000 | 0.345 | 1.28 Hz | 6.6 × sin(0.1543/1.528) = **0.67 Hz** |
| 14 400 | 0.247 | 0.92 Hz | 4.7 × sin(0.1104/1.440) = **0.36 Hz** |

Damping is 2.59 at every rate; the clamp is ±0.02 turns/symbol = ±48 Hz.
**At 14 400 the loop can pull in 0.36 Hz on its own.** The ±7 Hz of V.32 2.1
can only be met by the offset already being known when the rate changes — that
is, by having been acquired during the four-point training and carried across.
Nothing acquires it: there is no open-loop estimator anywhere in `v32.rs` or
`v32/startup.rs`.

### 2.4 What the fax modes already do that these do not

`crates/datapump/src/v27ter.rs` and `v29.rs` were written later and contain
three mechanisms the older modes lack. They are the template.

1. **Open-loop carrier acquisition from the two-phase front of the burst** —
   `v27ter.rs:866 fn acquire`, `v29.rs:892`. Square the symbols (which removes
   the two-phase data and leaves twice the carrier), take the sum of each
   square times the conjugate of the one before for the frequency, de-rotate
   and sum for the phase, halve both. Measured over `ACQUIRING = 48` symbols
   after `SETTLING = 8` (`v27ter.rs:594,602`). The comment at `v27ter.rs:861`
   records what the decision-directed loop did without it: "every one of forty
   tries at plus or minus seven hertz failed."
2. **A carrier detector referred to the line, not to full scale** —
   `v27ter.rs:792–821`. On requires 12 dB over a measured noise floor
   (`ON_ABOVE_FLOOR = 4.0`, `v27ter.rs:561`); off requires 12 dB below the
   burst's own loudest (`OFF_BELOW_LOUDEST = 0.25`, `:572`). The floor falls in
   a tenth of a second and rises over five (`FLOOR_FALL = 6.25e-4`,
   `FLOOR_RISE = 1.25e-5`, `:587`), and only moves while there is no carrier.
   The fixed levels survive only as a floor under the ratio (`:557`).
3. **The timing loop's input scaled to unit level** — `v27ter.rs:842`,
   `v29.rs:833`. Gardner's error is normalised by a running power estimate that
   starts at 1 and moves at 0.02 per symbol; a short burst 30 dB down is over
   before that estimate has come down. Scaling the input fixes it. V.22bis and
   V.32 feed the raw symbol.

And `v29.rs:663–684` computes the constant-modulus target from the
constellation in use, rather than assuming one.

---

## 3. The weaknesses

All measurements below: `fs` 16 kHz, transmitter into receiver, no echo, seeded
noise, carrier offset applied as a true single-sideband shift (255-tap Hilbert)
so the symbol rate is untouched. "Match" is the best bit agreement with the sent
payload at any bit offset; 0.5 is chance.

### 3.1 V.32 at 9600 and above fails at most arrival phases, on a perfect line

**Severity: high.** This is the worst thing found.

With 600 bytes of lead-in (0.33–1.0 s of training depending on rate), no noise,
no offset, no echo, and the only variable being how many samples of silence
precede the carrier:

```
rate            phases that carried the payload, out of 20
4800  uncoded   20/20   [....................]
7200  trellis   11/20   [.X.X.X.XX.......XXXX]
9600  uncoded   10/20   [.X..XX..XX..X...XXXX]
9600  trellis    6/20   [XX.X.X.XXXX.X.X.XXXX]
12000 trellis    6/20   [.X.X.XXXXXX.X.X.XXXX]
14400 trellis    4/20   [XX.X.XXXXXX.X.XXXXXX]
```

The failures are not marginal and not random — the receiver settles into one of
exactly two states, and which one is decided by the arrival phase:

```
9600 uncoded  q=0: match 1.000  residual 0.0240   half-spacing 0.3162
9600 uncoded  q=1: match 0.545  residual 0.2397
9600 trellis  q=2: match 1.000  residual 0.0241  half-spacing 0.2236
9600 trellis  q=3: match 0.552  residual 0.1706
14400         q=2: match 1.000  residual 0.0257  half-spacing 0.1104
14400         q=3: match 0.545  residual 0.0904
```

`equalizer_blind()` is `false` in every case, good and bad. The bad state's
residual for 9600 uncoded is 0.225–0.245 — sitting just under the 0.25
handover threshold at `equalizer.rs:121`. The equaliser reaches 0.25, hands over
to decision direction, and decision direction then holds it there for ever.

**How I know it is the equaliser and not the timing loop or the carrier loop:**
the same receiver at 4800, with the same timing loop, the same carrier loop and
the same arrival phases, is perfect at all 20 — and 4800 is the one
constellation whose constant-modulus target of 1.0 is correct. The residual in
the bad state is stable, not drifting, which is a converged filter and not a
loop that never acquired. And a longer lead-in does not rescue it.

**Why the arrival phase decides it:** the equaliser is symbol-spaced
(`equalizer.rs:87`, one sample in per symbol). A symbol-spaced equaliser cannot
compensate for the sampling phase — the phase decides which aliased version of
the channel it sees, and for some phases the constant-modulus cost surface has
a local minimum the filter falls into. A T/2-spaced equaliser has no such
dependence. This is the structural fix, and it is why the same receiver is fine
at 4800 where the points are seven times further apart than the local minimum's
residual.

**Symptom a user would see:** a V.32bis call that connects at 14 400 and
carries nothing — V.42 never gets a good frame, the terminal stays blank, the
modem sits there with a carrier up and a constellation display that looks like a
smear. Retry the call and it works. Retry again and it does not. It is roughly a
coin toss at 9600 and worse above.

**Why the test suite does not see it:** `v32_loopback.rs:245` uses one arrival
phase. `v32_call.rs:721` reaches 9600 through the full start-up, where the
equaliser converges on the four points of TRN first — at which point modulus 1.0
is correct and the eye is wide — and then keeps its taps across the rate change.
The call tests pass because the start-up hides the fault, and they would stop
hiding it the moment anything caused an equaliser reset at the higher rate.

### 3.2 V.32 does not use the training sequence the Recommendation provides

**Severity: high.**

V.32 5.2.3 (page 9 of the Recommendation, rendered and read) defines segment 3,
TRN, completely: scrambled binary ones at 4800 bit/s, scrambler initial state
all zeros, a binary one applied for the duration, **differential quadrant
encoding disabled**, the first 256 states given bit by bit (the text prints the
first fifteen for each mode) and the rest by Table 5. Its closing sentence:
"Segment 3 is intended for training the adaptive equalizer in the receiving
modem and the echo canceller in the transmitting modem." Duration at least 1280
and not more than 8192 symbol intervals. 5.2.2 adds that the segment 1 to
segment 2 transition "provides a well-defined event in the signal that may be
used for generating a time reference in the receiver."

The receiver knows every symbol of TRN in advance and uses none of it.
`crates/dsp/src/equalizer.rs` has no reference-trained path at all: `adapt` is
only ever called with the slicer's own output as the decision (`v32.rs:1074`).
`crates/datapump/src/v32/startup.rs` knows exactly when TRN is arriving —
`training_echo` at `:1099`, the adaptation gates at `:1143–1161`,
`SEGMENT_TRN = 1280` at `:815` — and uses that knowledge only to turn adaptation
on and off. The time reference of 5.2.2 is not used for timing either.

Every fault in §3.1 and every number in §2.3's pull-in table would be a
different problem if the equaliser and the carrier loop were trained against a
known sequence instead of against themselves.

### 3.3 V.22bis at 2400 bit/s fails at 2 Hz of carrier offset; 2.6 requires 7

**Severity: high.**

```
2400 bit/s   +0.0:1.00  +1.0:1.00  +1.5:1.00  +2.0:0.73  +3.0:0.69  +5.0:0.65  +7.0:0.63  −7.0:0.62
1200 bit/s   +0.0:1.00  +3.0:1.00  +7.0:0.97  −7.0:0.97  +10.0:0.92  +14.0:0.86  +20.0:0.86
```

V.22bis 2.6, read from the rendered page: "The receiver shall be able to operate
with received frequency offsets of up to ± 7 Hz." The 2400 bit/s receiver
manages 1.5. The 1200 bit/s receiver reaches 7 Hz but is already losing 3 % of
its bits there.

This is exactly the 1.39 Hz predicted in §2.2 from `Kp = 0.008` and the 16.8°
that a corner point of the sixteen may turn through. The frequency integrator
does not rescue it: at `1.5e-5` per unit error it gains 2.6 mHz per symbol even
if the error stayed saturated, and once the corner points start slicing wrong
the error stops being a measure of anything.

**Symptom a user would see:** a V.22bis call that connects, reports 2400, and
delivers garbage — or, with the handshake involved, falls back to 1200 on a line
that could have carried 2400. A real analogue trunk with a frequency-translating
carrier system routinely offsets by a few hertz, and that is precisely why the
clause is there.

### 3.4 The carrier detectors are fixed-threshold, mistimed, and answer the
answer tone

**Severity: medium to high.** Three separate defects in one mechanism.

**(a) Timing.** V.22bis 3.2 (rendered page, Fascicle VIII.1 p. 4): circuit 109
"shall turn OFF 40 to 65 ms after the level … falls below the relevant
threshold", and following a dropout "shall turn ON 40 to 205 ms after the
level … exceeds the relevant threshold". Measured:

```
V.22bis   ON after  22.8 ms,  OFF after 148.2 ms   (settled level 0.5305)
V.32      ON after   6.1 ms,  OFF after 138.7 ms   (settled level 0.4790)
```

Both ends of the window are missed. ON is early because the only delay is the
select filter's group delay (12.5 ms for V.22bis's 401 taps, 3.75 ms for V.32's
121) plus a 20 ms one-pole ramping through a threshold 54 dB below the signal —
there is no guard timer anywhere. OFF is late because the same 20 ms one-pole
(`v22bis.rs:612`, `v32.rs:915`) has to decay from 0.53 to 5.62e-4, which is
`0.020 × ln(944)` = **137 ms**.

Consequences of each: a 30 ms noise burst raises the carrier, which resets
`since_carrier` (`v22bis.rs:646`), throws away the part-gathered rate window
(`v22bis.rs:835`) and re-arms the 64-symbol equaliser gate. And when the far end
goes away the receiver keeps producing bits for 137 ms — at 2400 bit/s that is
330 bits of pure noise handed up to V.42 after the call is over.

**(b) Fixed thresholds.** `CARRIER_ON = 1.0e-3`, `CARRIER_OFF = 5.62e-4` in
three places: `v22bis.rs:129`, `v32.rs:289`, `fsk.rs:49`. Measured, band noise
alone raises the flag:

```
noise amplitude 0.0010:  V.22bis false  V.32 false  Bell103 false
noise amplitude 0.0030:  V.22bis false  V.32 true   Bell103 false
noise amplitude 0.0060:  V.22bis true   V.32 true   Bell103 true
```

With a settled signal level of 0.53, that puts the whole usable dynamic range at
about 39 dB, with nothing adaptive underneath it. `v27ter.rs:561` and
`v29.rs:536` already require 12 dB over a *measured* floor instead, and the
comment at `v27ter.rs:553` states the reason: "a fixed level cannot be both low
enough for a quiet line and high enough to ignore the noise on a noisy one."

**(c) The answer tone.** V.22bis 3.3, same rendered page: "Circuit 109 shall not
respond to the 1800 Hz or 550 Hz guard tones, or the 2100 Hz (nominal) answer
tone during the handshake sequence." Measured: feeding the answering modem's own
`Signal::AnswerTone` to a calling-side receiver raises `carrier()` — **true**.
It has to: 2100 Hz is 300 Hz from the 2400 Hz carrier, in the middle of the
401-tap select filter's 600 Hz passband, and nothing downstream asks what shape
the thing is.

The V.22bis handshake itself survives this, because it keys on `Pattern` rather
than on the carrier flag (`v22bis/handshake.rs:208–261`). What does not survive
it: `Modem::carrier()` (`handshake.rs:398`) reports a carrier during the answer
tone, and the rate detector's accumulator is gated on the same flag
(`v22bis.rs:831`), so 128 symbols of a pure tone are measured as a
zero-variance constellation and the rate is decided as 1200 before the real
carrier has arrived. It takes another 128 symbols — 213 ms — to correct.

The code's own citation for the thresholds, "V.22bis 6.5.2"
(`v22bis.rs:125`, `v32.rs:288`, `fsk.rs:47`), is wrong: the thresholds are in
3.3 and the timing in 3.2. 6.5.2 is part of the handshake.

Also on the same clause: the hysteresis required is "at least 2 dB". The code
uses 5, which is fine, but the −43/−48 dBm the clause actually gives are
absolute levels on the line, and nothing in the project calibrates a line level
to dBm. That is the real reason the thresholds cannot be fixed numbers.

### 3.5 A level step costs V.22bis a V.42 frame; a dropout is invisible

**Severity: medium.**

Level step applied at the first symbol of the payload, then held:

```
 −3 dB: payload 1.000     −6 dB: 0.938     −12 dB: 0.890
−20 dB: payload 0.786    −30 dB: 0.696     +6 dB: 1.000   +12 dB: 0.992
```

A 150-byte payload is 1200 bits, so −12 dB costs about 130 bits and −20 dB about
260. The mechanism is the gain control's 30-symbol (50 ms) time constant at
`v22bis.rs:594`: a −20 dB step needs the estimate to fall by a factor of ten,
which takes 2.3 time constants = 115 ms = 69 symbols, and during that time the
constellation is oversized and the slicer decides the outer ring. 260 bits is
65 symbols. The arithmetic and the measurement agree.

Dropout of silence in the middle of the payload:

```
   5 ms: payload 0.988      10 ms: 0.983       20 ms: 0.972
  50 ms: payload 0.943     100 ms: 0.870      200 ms: 0.818
```

and in every case the carrier flag was **still up at the end**, because a
dropout shorter than 137 ms cannot bring it down (§3.4a) and a longer one is
followed immediately by the carrier returning. So the layers above are never
told. A 100 ms dropout costs about 150 bits and nothing reports anything.

**Symptom a user would see:** on a VoIP line, exactly what
`memory/voip-jitter-slips.md` describes for V.32 — a stall, V.42 retransmitting,
and no indication of why. The modem's own status says the carrier never went
away.

### 3.6 The one clock-offset test does not test a clock offset

**Severity: medium.** This one is about the evidence, not the receiver.

`crates/datapump/tests/v22bis_loopback.rs:269–285` builds the receiver with
`FS * (1.0 + ppm / 1e6)`. `Receiver::new` derives *both* the samples-per-symbol
and the down-converting NCO's step from that one number
(`v22bis.rs:576–578`). The NCO is advanced once per sample fed, and samples
arrive at the true 16 kHz, so scaling `fs` by (1 + ε) puts the receive carrier
at `2400/(1+ε)` — **a carrier offset of −ε × 2400 Hz riding on top of the clock
offset.** At the ±200 ppm the test uses that is ∓0.48 Hz, which is inside the
1.39 Hz the loop can take (§2.2), so the test passes; it simply is not measuring
what it says.

My own sweeps using the same trick are contaminated the same way and I am not
reporting them as clock-offset results. The harness must resample
(`dsp::Resampler`, `crates/dsp/src/resample.rs:52`) so the two impairments can
be applied one at a time.

### 3.7 Smaller things, with line numbers

- `crates/dsp/src/fsk.rs:81` — `slow_env` is constructed and fed
  (`fsk.rs:105`) and never read. Dead state from the ratio detector that
  `fsk.rs:122` describes removing.
- `crates/datapump/src/v22bis.rs:599` — modulus 1.32 is wrong for the four
  points 1200 bit/s uses (want 1.0), inflating the blind constellation by 15 %.
  Cosmetic for data, wrong for `residual_error()`.
- `crates/datapump/src/v32.rs:949 fn follow` — handles a rate change but does
  not reset or re-target the equaliser, so a receiver that goes from 4800 to
  14 400 keeps taps converged on a four-point constellation and a modulus that
  was right for it.
- `crates/datapump/src/v32.rs:1073` — the equaliser gate is
  `self.symbols > 64`, counted from construction, not from the carrier. V.22bis
  fixed exactly this bug (`v22bis.rs:646` resets `since_carrier` on a carrier
  transition, with a long comment at `:502` about why); V.32 still has it.
- `crates/datapump/tests/v32_vector.rs:3` — the module comment says the crate
  cannot demodulate 14 400. It can, since `v32/trellis.rs:440`. The only real
  V.32bis recording in the repository is never put through the receiver.
- `crates/datapump/tests/v22bis_capture.rs` and `v32_replay.rs` both name
  captures that are no longer in `dist/captures`.

### 3.8 What was measured and found sound

Worth stating, so the revamp does not go looking here.

- **Bell 103** holds a 75 Hz carrier offset (the shift is only ±100 Hz, so
  failing at 100 Hz is arithmetic, not a fault) and reads 37 of 37 characters
  down to about 6 dB of in-band signal-to-noise, 34 of 37 at 6 dB, 29 at 4 dB.
  Async framing means there is no timing loop to lose. The only weakness at
  300 bit/s is the carrier detector it shares with everything else (§3.4).
- **V.21**'s transition-pulled bit clock (§2.1) leaves under 2 % of a bit period
  of standing error at 1000 ppm.
- **V.32 at 4800** is solid: 20/20 arrival phases, carrier offset clean to
  10 Hz and 0.94 at 20 Hz against a predicted 13.6 Hz pull-in, and no clock
  sensitivity found.
- **V.22bis timing acquisition** is right. `v22bis_loopback.rs:249` and
  `:640` were hard-won — the `Gardner` gain of 0.1 (`v22bis.rs:587`) and
  `shift_half_symbol` (`v22bis.rs:970`) both exist because of real failures —
  and 12 of 12 arrival phases carried the payload in my sweep.

---

## 4. The harness this work needs

### 4.1 Where it lives

**A new dev-only workspace member, `crates/margin`.** Not in `crates/dsp`,
which ships and should not carry a channel simulator. Not in `crates/line`,
which depends on `cpal` and must stay nowhere near this work. Not in
`crates/datapump/tests/common`, because the same code has to be usable from a
sweep binary and from the GUI's diagnostics, and a `tests/` module cannot be
depended on.

```
crates/margin/Cargo.toml        dependencies: dsp, datapump   (dev-only; nothing ships depends on it)
crates/margin/src/lib.rs
crates/margin/src/line.rs       the impairment chain
crates/margin/src/mode.rs       one uniform driver per modulation
crates/margin/src/metrics.rs    the numbers
crates/margin/src/sweep.rs      find the largest impairment a receiver still holds at
crates/margin/src/main.rs       the exploration binary
crates/datapump/tests/margins.rs   the assertions, with `margin` as a dev-dependency
```

### 4.2 The line

One chain, sample in, samples out, deterministic from a seed. Order matters and
should be fixed and documented: clock offset first (it is the far end's
oscillator), then the carrier offset and phase jitter (the network's frequency
translation), then the echo of our own transmit (the hybrid, which is local),
then the level step and dropout (the path), then noise (the receiver's own
front end and the trunk).

```rust
#[derive(Clone, Copy, Debug, Default)]
pub struct Impairments {
    /// Carrier frequency offset in hertz, applied as a single-sideband shift so
    /// the symbol rate is untouched. V.22bis 2.6 and V.32 2.1 both ask for ±7.
    pub carrier_hz: f64,
    /// The far end's sampling clock against ours, in parts per million, applied
    /// by resampling. Each end is allowed 0.01% (V.22bis 2.5.1, V.32 2.3), so
    /// ±200 is the worst two conforming modems can be apart.
    pub clock_ppm: f64,
    /// Sinusoidal phase jitter: peak degrees, and its frequency in hertz.
    /// Mains hum at 50 or 60 Hz and ringing-current jitter at 20 Hz are the
    /// two that happen.
    pub jitter_degrees: f64,
    pub jitter_hz: f64,
    /// Signal to noise ratio in decibels, measured in a 3.1 kHz band. None is a
    /// noiseless line. The generator is Gaussian and seeded.
    pub snr_db: Option<f64>,
    /// An echo of what this end transmitted.
    pub echo: Option<Echo>,     // { delay_s: f64, level_db: f64 }
    /// A level change: when, and by how much.
    pub step: Option<Step>,     // { at_s: f64, db: f64 }
    /// A stretch of silence: when, and for how long.
    pub dropout: Option<Dropout>, // { at_s: f64, length_s: f64 }
}

pub struct Line { /* … */ }

impl Line {
    pub fn new(fs: f64, impairments: Impairments, seed: u64) -> Self;

    /// Feed one sample of what the far end sent and one of what this end is
    /// transmitting at the same instant, and append what arrives at this end's
    /// receiver. A slice, not a sample, because a clock offset makes the count
    /// vary: this is the whole reason the offset is resampled rather than faked
    /// by lying to the receiver about `fs` (see §3.6).
    pub fn step(&mut self, far: f64, own: f64, out: &mut Vec<f64>);

    /// Line time in seconds, which is what `step` and `dropout` are scheduled
    /// against.
    pub fn elapsed(&self) -> f64;
}
```

`clock_ppm` uses `dsp::Resampler::new(fs, fs * (1.0 + ppm / 1e6))`
(`crates/dsp/src/resample.rs:52`), whose kernel is already the anti-alias
filter. `carrier_hz` uses a Hilbert transformer and a complex rotation, so it
shifts the carrier and leaves the baud rate alone; a 255-tap Hamming-windowed
Hilbert is flat to better than 0.1 dB from 200 Hz to 3.6 kHz at 16 kHz and is
what the measurements above used.

### 4.3 The modes, uniformly

```rust
#[derive(Clone, Copy, Debug)]
pub enum Mode {
    Bell103,
    V21,
    V22bis(v22bis::Rate),
    V32 { bps: u32, coding: v32::Coding },
    V27ter(v27ter::Rate),
    V29(v29::Rate),
    V17(v17::Rate),
}

impl Mode {
    pub fn baud(self) -> f64;
    pub fn bits_per_symbol(self) -> u32;
    /// Half the distance between the two closest points, in the units the
    /// receiver's residual error is measured in. Every judgement about whether
    /// a receiver is holding has to be divided by this first: the same residual
    /// is a comfortable lock at 4800 and noise at 14 400.
    pub fn half_spacing(self) -> f64;
    /// Whether the mode has a constellation at all. FSK does not, and its
    /// slicer margin is the discriminator level instead.
    pub fn is_qam(self) -> bool;
}

/// A transmitter and a receiver of one mode, with what the harness needs
/// exposed the same way for all of them.
pub trait Pump {
    fn send(&mut self, bytes: &[u8]);
    fn next_sample(&mut self) -> f64;
    fn feed(&mut self, sample: f64);
    fn take_bits(&mut self) -> Vec<bool>;
    fn carrier(&self) -> bool;
    /// What the slicer last saw and what it decided, once per symbol. For FSK,
    /// the discriminator level and ±1.
    fn last_decision(&mut self) -> Option<(Complex, Complex)>;
}
```

**The one change this needs in the receivers.** Nothing today exposes the
decision. `v22bis::Receiver::constellation_point` (`v22bis.rs:900`) and
`v32::Receiver::constellation_point` (`v32.rs:1134`) give the equalised symbol
but not what it was sliced to, and `residual_error()` is a 100-symbol average
inside the equaliser, which is too slow to show a receiver coming apart. Each
receiver needs one accessor returning `(received, decided)` once per symbol,
cleared by reading, exactly as `take_symbol` already does for the FSK level
(`bell103.rs:95`). That is a handful of lines per mode and it is the only
production change the harness requires.

### 4.4 The numbers

```rust
pub struct Metrics {
    /// Seconds from the first sample of carrier on the line to the first bit
    /// that matched the payload, once alignment held for 200 bits.
    pub time_to_lock: Option<f64>,
    /// Seconds from lock to the first wrong bit.
    pub time_to_first_error: Option<f64>,
    /// Signal to noise ratio at the slicer, in decibels: mean |decision|² over
    /// mean |received − decision|². The number that decides whether a receiver
    /// holds, and directly comparable across rates.
    pub slicer_snr_db: f64,
    /// The same in 100 ms windows, so a receiver that locks and then walks off
    /// is visible where a single average hides it.
    pub slicer_snr_over_time: Vec<(f64, f64)>,
    /// Residual over half the point spacing. One means the average symbol is
    /// sitting on the decision boundary.
    pub margin: f64,
    /// Symbol error rate over the whole run, and in 100 ms windows.
    pub symbol_error_rate: f64,
    pub symbol_errors_over_time: Vec<(f64, f64)>,
    /// Bit error rate against what was sent, after alignment.
    pub bit_error_rate: f64,
    /// Whether the carrier was ever declared lost after being found, and for
    /// how long in total.
    pub carrier_lost: bool,
    pub carrier_down_seconds: f64,
    /// What the receiver believed the rate was at the end, where it decides.
    pub rate: Option<u32>,
}
```

Because of the differential coding and the self-synchronising descrambler the
recovered stream is offset by an unknown number of bits. Align once — the offset
that maximises agreement over the first 200 bits after the carrier appears — and
count from there, so a receiver that never locks scores a bit error rate near
0.5 rather than an accidental match. `slicer_snr_db` in windows is what
separates the two V.32 attractors in §3.1 at a glance: 0.024 residual against a
0.110 boundary is 13 dB of margin, 0.090 against the same boundary is 1.8 dB.

### 4.5 Running one

```rust
pub struct Plan {
    /// Seconds of filler before the payload, so the loops can settle.
    pub lead_in: f64,
    pub payload: f64,
    /// Samples of silence before the carrier starts, which is what selects the
    /// arrival phase. Sweeping this is what found §3.1, and nothing else would
    /// have.
    pub arrival: usize,
    pub seed: u64,
}

/// One transmitter into one receiver through one line.
pub fn run(mode: Mode, imp: Impairments, plan: Plan) -> Metrics;

/// Two whole modems, each hearing the far end through its own `Line` and its
/// own transmit through the echo path. What `v32_call.rs:446 custom_call` does
/// today, with the full impairment set instead of three of them.
pub fn call(mode: Mode, imp: Impairments, plan: Plan) -> (Metrics, Metrics);

/// The largest value of one impairment at which the receiver still held.
pub fn margin_of(
    mode: Mode,
    plan: Plan,
    base: Impairments,
    vary: impl Fn(&mut Impairments, f64),
    values: &[f64],
    holds: impl Fn(&Metrics) -> bool,
) -> f64;
```

and a binary for exploration, which is how the tables in §3 were produced:

```
cargo run -p margin --release -- --mode v32:14400t --sweep arrival:0..20
cargo run -p margin --release -- --mode v22bis:2400 --sweep carrier-hz:-8..8:0.5
cargo run -p margin --release -- --mode v27ter:4800 --sweep snr-db:30..8:-1
```

### 4.6 The tests it should carry

`crates/datapump/tests/margins.rs`, one test per requirement, each naming its
clause. Marked below with what happens today.

| test | clause | today |
|---|---|---|
| `v22bis_2400_holds_seven_hertz_of_offset` | V.22bis 2.6 | **fails** (holds 1.5) |
| `v22bis_1200_holds_seven_hertz_of_offset` | V.22bis 2.6 | marginal (0.97) |
| `v32_holds_seven_hertz_at_every_rate` | V.32 2.1 | **fails** above 9600 |
| `every_v32_rate_acquires_from_every_arrival_phase` | — | **fails** at 7200 and above |
| `the_carrier_turns_on_between_40_and_205_ms` | V.22bis 3.2 | **fails** (22.8 ms) |
| `the_carrier_turns_off_between_40_and_65_ms` | V.22bis 3.2 | **fails** (148 ms) |
| `the_answer_tone_does_not_raise_circuit_109` | V.22bis 3.3 | **fails** |
| `the_carrier_holds_across_a_hundred_millisecond_dropout` | — | passes, but silently (§3.5) |
| `two_clocks_two_hundred_ppm_apart_still_carry_data` | V.22bis 2.5.1, V.32 2.3 | untested properly (§3.6) |
| `each_rate_carries_data_at_the_signal_to_noise_ratio_it_needs` | — | untested |
| `v27ter_and_v29_carry_a_page_over_an_impaired_line` | — | untested, no test file exists |

The first seven are the revamp's definition of done.

---

## 5. Open questions

1. **Is the V.32 arrival-phase fault the constant-modulus target, the handover
   threshold, or the symbol-spaced equaliser?** The evidence points at all
   three contributing and at the symbol spacing being the root. Fixing the
   modulus and the threshold is two lines; going fractionally spaced is a real
   change to `dsp::Equalizer`. Which to do first should be settled by running
   §4.5's arrival sweep after each.
2. **Does the fault reach a whole call?** `v32_call.rs:721` passes at 9600,
   which suggests the start-up hides it by converging on four points first. A
   `margin::call` run at 14 400 with a retrain forced mid-call would say for
   certain, and that is the case a user actually hits.
3. **What should replace the fixed carrier thresholds?** `v27ter.rs:561` and
   `v29.rs:536` have a working answer for a burst-mode receiver. A duplex mode
   never sees the line quiet once a call is up, so the floor estimate needs a
   different trigger — probably the pre-handshake quiet, measured once.
4. **What is the real frequency offset on Rory's lines?** `v32_reversals.rs:110
   probe_carrier_offset` exists to measure it from a capture and has no capture
   to measure. Until there is one, ±7 Hz from the Recommendation is the only
   target available, and it may be generous or mean.
5. **Should the older modes resample to a higher rate?** V.32's 6.667 samples
   per symbol is what makes the linear interpolator a −27 dB error floor
   (§2.3). Running the receiver at 24 kHz would give 10 samples per symbol and
   −12 dB better, or a cubic interpolator would do it without changing the rate.
   Unmeasured either way.
6. **Are there captures of the older modes anywhere?** Two replay harnesses were
   written for files that are gone. Recovering them, or making a fresh V.22bis
   and V.32 call, would turn §3's simulated evidence into measured evidence.
