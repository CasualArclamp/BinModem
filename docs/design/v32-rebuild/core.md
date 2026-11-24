# V.32 receiver rebuild: the V.34 receiver as its core

Research for the design agent that rebuilds the V.32/V.32bis demodulator
(carrier, timing, gain, equaliser, decisions) on the model of the V.34
receiver. Nothing in the tree was changed to write this.

- Tree: `main` at `d7b914b` (Version 1.2.1), clean.
- All `receiver.rs` references are `crates/datapump/src/v34/receiver.rs` (1676
  lines, read in full) unless another file is named.
- Baseline on this tree: `cargo test -p datapump --lib` = **324 passed, 5
  ignored**; of which `v34::` = **112 passed**. `cargo test -p dsp --lib` = **75
  passed**. `--release --test v34_vector` = 4 passed, 1 ignored;
  `--release --test v90_call` = **48 passed** (139 s).
- Probes (section 10) were run from a scratch crate outside the repository with
  path dependencies on `datapump` and `dsp`, public API only, 16 kHz, release.
  Probe IDs (E1–E6) are cited where a figure comes from one.
- The sibling documents in this folder are `spec.md` (what V.32/V.32bis say)
  and `contract.md` (what callers of `v32::Receiver` rely on). They are cited,
  not repeated.

---

## 0. What the design agent most needs to know

1. **The V.34 receiver already runs V.32's exact band.** `Band::new(S2400,
   true)` is 2400 baud on 1800 Hz (`probe.rs:251`, `:262`). It trains to
   **54.1 dB** on a clean line there (E1). It is unit-tested at 2400/1600 and
   2400/1800 (`receiver.rs:1664-1675`). On the live call of 2026-09-22
   (`live-1790041800.wav`), channel 0, what arrived (`gui/src/live.rs:1622`),
   is centred on **1600 Hz**, and channel 1, what we sent, on **1800 Hz**. So
   the receive path was live at 2400/1600, not 2400/1800 (section 6).
2. **Its stability comes from seven specific choices** (section 1–4):
   - a fixed mixer, with everything else after it re-readable from stored
     samples;
   - one interpolating filter that both rejects the image and places each
     sample;
   - a T/2 equaliser;
   - an equaliser solved by least squares on the known sequence, with no
     blind stage;
   - three loops 4× apart in speed: carrier 50 symbols, timing 200, equaliser
     about 870;
   - one gate that freezes every loop together;
   - loss detection with rewind and re-reading of the raw samples.
   All of it is generic QAM machinery except the parts listed in section 5.
3. **Four measured defects must not be copied** (sections 4.3, 9):
   - **the plain resync cannot reach a band of timing jumps.** Its window ends
     at the newest sample, so later shifts are skipped
     (`receiver.rs:1157`, `:1169-1170`). On 16 points, a jump of +0.25 to +0.35
     symbol (2400 baud) is never recovered, whatever the phase (E3e/E3f).
     That is about 35 of the 339 insertion lengths from 2 to 340 samples
     (E3c). It is latent in live V.34, because a 20 ms slip at 3429 baud
     falls outside the band.
   - **there is no gain control anywhere, and the plain resync fits no gain.**
     On 16 points, a 3 dB step either way is lost for good, and a −6 dB ramp
     false-locks at 12 dB without ever declaring loss (E4/E4b). V.32's
     tolerance falls to **0.64 dB** at 14 400 (section 7.2).
   - **frequency acquisition is weak.** The trained SNR is 23 dB at 2 Hz of
     offset and 15 dB at ±7 Hz. At 7 Hz tracking is still about 5 dB down 20 s
     later. The trainer's turn estimate aliases at 10 Hz (E2/E2b). V.32 2.1
     requires ±7 Hz.
   - **V.34's hunt takes V.32's AA→CC and AC→CA for S and S-bar** (E6).
4. **Recommendation (section 8): option (a) as the destination, built the way
   plan.md rule 9 already prescribes.** Copy the machinery into a shared core
   (V.32 first, with the four defects fixed) and leave `v34/` and `v90/`
   untouched. Move V.34 onto the core later, as its own job, gated by a
   bit-exact golden test that does not exist yet. Every current V.34/V.90 test
   asserts a threshold, so a 1–2 dB regression would pass them all.
5. **Two contract facts constrain the design** (from `contract.md`):
   - the bare-receiver tests need **blind** acquisition (4800 at any arrival
     phase, 9600T at phase 0), and V.34's receiver has no blind stage
     (section 7.8);
   - our transmitter's TRN after symbol 256 swaps dibits 10 and 11 against
     Table 5/V.32 (`v32.rs:427`, `:708`). `spec.md` §2.6 found this, and I
     confirmed it on the rendered page (PDF page index 15). A known-sequence
     trainer built from Table 5 will not train on our own transmitter until
     that is fixed.

---

## 1. The signal path, in order

| # | Stage | Where | Constants | Why it holds up |
|---|---|---|---|---|
| 1 | Down-mix | `feed` `:642-646` | `mixed = 2·x·e^{-j2πφ}`, `φ += carrier/fs`; `step` is set once at `:478` and **never corrected** | The equaliser's input is a fixed linear function of the line. So stored samples can be re-read at any offset later (resync, `take_up`), and least squares can be solved on stored rows. A carrier offset appears only as a slow rotation, taken out after the equaliser (row 6). No loop sits upstream of anything that is stored. |
| 2 | History | `:647-651` | `HISTORY = 16_384` mixed samples (`:98`): 1.024 s at 16 kHz, 2.05 s at 8 kHz (V.90 digital) | The raw material for every re-read after a slip. |
| 3 | Interpolating low-pass | `interpolate` `:662-678`; table built in `new` `:456-472` | 64 taps (`:57`) × 256 fractional phases (`:58`). Windowed sinc, Kaiser β = 8 (`:465`), each row normalised to unit DC gain (`:468-471`). The nearest row is taken, with no interpolation between rows (`:664`): 1/256-sample resolution. Cutoff `0.5·baud·(1+ROLLOFF)+300` Hz, capped at `0.45·fs` (`:456`), `ROLLOFF = 0.1` (`qam.rs:20`) | One filter rejects the 2f_c image *and* places the sample exactly where the timing loop asks. There is no separate anti-alias FIR, matched filter and two-point interpolator to disagree with each other. **There is no matched filter at all**: the equaliser is the matched filter. |
| 4 | Half-symbol sampling | `feed` `:653-657`; store `on_half` `:680-690` | `due += half·(1+drift)`, `half = fs/baud/2` (`:483`), i.e. 3.333 samples at 2400/16 kHz. First `due = 64` (`:482`). Each half is kept with the time it was taken (`halves`/`times`). `KEPT = 4096` halves (`:65`) = 2048 symbols = 0.853 s at 2400 | The times make exact re-reading possible. The ring is long enough for a training window plus its retry. |
| 5 | Equaliser | `symbol` `:910-913`, `apply` `:1271` | 31 complex taps at T/2 (`REACH = 15`, `:62`), `y = Σ wᵢxᵢ`. Symbols centred on even halves (`next_symbol += 2`, `:911`). 15.5 symbols = 6.46 ms at 2400. The initial centre tap of 1 (`:473-474`) is replaced wholesale by training | Fractional spacing makes it indifferent to where in the symbol the sampling falls (comment `:31-33`), so timing only has to stop drift. |
| 6 | Derotation | `:921-922` | `z = y·e^{-j·rotation}` | The carrier is removed after the equaliser, which learns in its own frame (section 3.2). |
| 7 | Slicer | `:923`, `Slicer::decide` `:154-163` | `Points(Size)` → V.34's 4 or 16 points via `signals::decide` (`:1252-1256`). `Grid{scale,limit}` → the nearest odd integer on each axis, clamped | The tentative decision that drives every loop (section 3.6). |
| 8 | Error, gate, loops | `:924-969` | `e = z − target`; see section 3 | |

**The front end at V.32's band.** These are computed from the code's own
formula (`receiver.rs:456-472`) at 2400 baud, cutoff 1620 Hz:

| frequency from carrier | 16 kHz | 8 kHz |
|---|---|---|
| 900 Hz | 0.00 dB | 0.00 |
| 1200 Hz (±baud/2) | −0.22 | −0.00 |
| 1320 Hz (band edge at 10% roll-off) | −0.80 | −0.01 |
| 1500 Hz (band edge at 25%, V.32's usual) | −3.09 | −1.34 |
| 1620 Hz (cutoff) | −6.02 | −6.02 |
| 2000 Hz | −27.9 | −90.0 |
| 2100 Hz (nearest image edge at 25% roll-off, 1800 Hz carrier) | −39.4 | −85.8 |
| 2280 Hz (nearest image edge at 10% roll-off, 1800 Hz carrier) | −95.0 | −96.9 |

At 2400/1800 the mixer's image is 960 Hz clear of a 10% band and 600 Hz clear
of a 25% band. It is at least 39 dB down before the equaliser, whose
resolution at 31 taps × 4800 Hz is 155 Hz. The front end needs no change for
V.32. The cutoff formula should take the roll-off as a parameter anyway,
keeping 0.1 for V.34. The −3 dB at a 25% band edge is re-flattened by the
equaliser: E2 trained to 40.4 dB at 25% roll-off with 35 dB of noise.

---

## 2. Acquisition

### 2.1 Finding S and S-bar: `Hunt` (`:285-364`)

- Each half-sample `h_n` is correlated with `h_{n−4}`, two symbols back:
  `c = h_n·conj(h_{n−4})`, `p = (|h_n|²+|h_{n−4}|²)/2` (`:333-336`). The last
  8 are summed (`:337-341`).
- It is S when `Re Σc > 0.7·Σp` and `Σp/8 > AUDIBLE = 4e-4` (`:342`, `:94`).
  After `HELD = 40` halves in a row (20 symbols; `:301`) it reports `Heard::S`.
- A four-phase template of S (one per half-sample position mod 4) is kept by
  EMA: 0.2 while hunting (`:346`), 0.1 once armed (`:318`).
- Once armed: S-bar is found when the last four halves against the template
  give a ratio below −0.5. It is reported as `Reversal{at: index−3}`
  (`:313-316`), the half-sample S-bar starts on, ±1. 24 halves in a row that
  do not match (ratio ≤ 0.5) reset the hunt (`:321-324`).
- **Why it holds up:** it needs no timing, carrier phase, gain or equaliser. The
  template exists because the S→S-bar change arrives smeared over two symbols
  through a VoIP path (comment `:276-283`).
- **V.32's S and S-bar have V.34's structure exactly.**
  - V.32 5.2.1/5.2.2: S = A,B alternating, B = jA; S-bar = C,D = −S.
  - V.34 S is 128T and S-bar 16T (`signals.rs:12-13`). V.32 S is 256T (NT
    longer for the caller, 5.4.1) and S-bar 16T.
  - E6: fed V.32's S/S-bar through my modulator at 25% roll-off, the hunt
    reported the reversal at 0.192 s against a true 0.19 s.
- **But it cannot tell V.32's preamble tones from S** (E6).
  - AA→CC (call modem, 5.4.1) and AC→CA (answer modem, 5.4.2) are also
    two-symbol periodic.
  - It reported `S` 0.095 s into each, and `Reversal` at the AA→CC and AC→CA
    joins (0.586 s).
  - A V.32 driver must arm `hunt()` only when S is due, or add a
    discriminator:
    - S has energy at the carrier and at both ±1200 Hz sidebands (a DC and a
      Nyquist component in the T-spaced sum and difference);
    - AA has only the carrier;
    - AC has only the sidebands.

### 2.2 Training by least squares on the known sequence

`train` (`:525-533`) sets `start = s_bar + 2·S_BAR_SYMBOLS` halves (`:526`).
The sequence's first symbol is 16 symbols after S-bar begins, which is also
true of V.32 (TRN follows a 16T S-bar). `finish_training` (`:741-783`) runs
once `made > start + search + 2·end + REACH` (`:705-707`). `solve_known`
(`:786-845`) then does the following.

1. **Targets.** `sequence(reference, far, to)` (`:1228-1238`) generates them.
   PP comes from `signals::pp`. TRN comes from `signals::Sender::trn`, starting
   from a zero-state scrambler of the far end's polynomial, at unit mean power
   (`unit` `:1243-1248`).
2. **Alignment.** Each `delta` in ±`SEARCH` = ±8 halves (±4 symbols; `:81`,
   `:809`):
   - forms rows of 31 halves centred at `origin + 2k` (`:815`);
   - solves `least_squares` with ridge `1e-3·energy/31` (`:819`, `:1280-1283`;
     normal equations and Cholesky, `dsp/src/complex.rs:142-163`);
   - keeps the smallest residual (`:820-824`).
3. **Frequency.** From the rough (static) fit, `early`/`late` are the halves of
   the window summed as `Σ y·conj(d)`. Then
   `turn = arg(late·conj(early))/middle` (`:829-835`).
   - `middle` is 120 symbols in both first-try windows, so the estimate is
     unambiguous only for `|Δf| < baud/(2·middle)`: **10 Hz at 2400 baud**,
     14.3 Hz at 3429.
4. **Final solve.** Over the full window, against targets pre-rotated by
   `e^{j·turn·k}` (`:838-843`). The taps come out static, and the turn is
   handed to the carrier loop, with `rotation = turn·to`.
5. **Acceptance.** At least `KNOWN_ENOUGH = 12` dB (`:90`, `:743`). Otherwise
   it tries once more, further into the sequence (`:749-755`), with
   `WIDE_SEARCH = ±200` halves (`:86`). That wide search pre-scores every
   alignment by raw correlation and solves only at the best ±3 (`:789-807`).
   Under 6 dB it gives `Heard::Untrained` (`:767-771`), which fails the call
   (`training.rs:1265`). Symbols from the window's end are produced at once
   (`:779-782`).

Windows (`:1217-1225`), in symbols of the reference:

| reference | alignment | solve | retry (both) |
|---|---|---|---|
| `PpThenTrn` | 48–288 (PP) | 48–352 | 544–800 |
| `Trn(size)` | 16–256 | 16–384 | 320–512 |

**What it solves for:** a 31-tap T/2 MMSE equaliser, including gain and the
carrier phase at the window's end, plus the carrier's turn per symbol.

**What it needs in advance:**
- the sequence, from its first symbol;
- where it starts, to within ±4 symbols (from S-bar);
- the far end's scrambler polynomial.

It needs no timing phase, carrier phase, gain, channel or convergence.
At 2400 baud, a `Trn` training completes about 396 symbols (165 ms) after TRN
starts. V.32's TRN is at least 1280T (533 ms; 5.2.3), so the first try *and*
the retry fit inside one TRN with room to spare.

Measured (E1, E2, E2b):
- clean line: 54.1 dB;
- 35 dB of noise and 114 ppm: 34.1 dB (40.4 dB at 0 ppm);
- carrier offset, `Trn(Four)`: 28.9 dB at 1 Hz, 22.8 at 2 Hz, 14.6 at 4 Hz,
  15.8 at +7, 14.4 at −7, 7.5 at 10 Hz;
- carrier offset, `PpThenTrn`: 21.3 dB at 4 Hz, 13.7 at 7 Hz, **untrained at
  10 Hz**;
- tracking after the offset training: back to full SNR within 2 s at 4 Hz;
  at 7 Hz it is stuck about 5 dB low, creeping from 31.6 to 35.4 dB over 20 s.

On VoIP the only frequency offset is the clock: 114 ppm of 1800 Hz = 0.2 Hz.
So this matters for the Recommendation (V.32 2.1: ±7 Hz), not for Rory's line.

**For V.32, estimate the frequency before solving.** S gives it for free,
because S is periodic.
- **Coarse.** The hunt's lag-4-halves correlation `Σc` (`:337-341`) turns by
  exactly two symbols of carrier offset. Averaged over S it is unambiguous to
  ±baud/4 = ±600 Hz, and it is not upset by the static-fit problem, since no
  equaliser is involved.
- **Fine.** The 256T of V.32's S also allow the template's average over the
  first half of S to be compared with the second: a lag of about 128 symbols,
  unwrapped by the coarse figure.
- **Then solve.** Pre-rotate the targets by that turn (the `turn` field of
  7.3), and keep the existing early/late step as a final refinement.

Whether this closes E2's 7 Hz gap is to be measured.

### 2.3 The fallback, `reacquire` (`:857-906`)

- **Scope:** `Reference::Trn` only, when a previous training exists and the
  known fit came in under 12 dB (`:746`).
- **Method:** it keeps the old taps and tries ±8 halves × 4 quarter turns, the
  base turn coming from the fourth power (`:872-873`). Each is scored by how
  many decisions descramble to ones (`signals::Reader::trn`, `:877-889`), and
  more than 95% is required (`:905`).
- **Purpose:** it exists for a far end whose TRN scrambler does not restart.
  V.32 5.2.3 requires a zero start ("The initial state of the scrambler shall
  be all zeros"), so a V.32 equivalent is insurance only. It could use the
  4-point part of TRN after symbol 256; the first 256 carry one bit a symbol.

### 2.4 `resume` (`:548-559`)

`resume` goes on with the last taps, carrying the carrier on by
`turn·(symbols gone)`. V.90's digital modem uses it for CPt straight after
S-bar (`v90/digital.rs:953`). V.32 has a use for it too: the call modem hears
the answer modem's second S S-bar TRN R3 on the same line it trained on. The
call modem may `resume` or train again; both are cheap.

---

## 3. Tracking

### 3.1 The single gate (`symbol` `:909-991`)

```rust
let doubtful = 0.25 * self.slicer.min_distance_squared();   // :928
if self.lost.is_none() && squared < doubtful {               // :947
    /* NLMS, carrier, timing, settled */                     // :950-968
}
```

- `doubtful` accepts an error inside the circle inscribed in the decision cell
  (`|e| < d_min/2`). That is the boundary itself, not half of it: the
  reference's §3.1 wording is wrong.
- Inside the gate: all three loops and `settled` (the EMA of passing errors,
  0.01, `:968`).
- Outside the gate, every symbol:
  - `rotation += turn` (`:946`), a flywheel that keeps the offset running
    while everything else is held;
  - the `recent` loss window (`:930-934`);
  - the `error` EMA behind `snr_db()` (`:988`, `:597`);
  - loop snapshots, but only while not lost (`:971-986`).

### 3.2 T/2 NLMS equaliser (`:950-954`)

`w −= (STEP/‖x‖²)·(e·conj(spin))·conj(x)`, `STEP = 0.02` (`:127`), where
`‖x‖²` is the row energy + 1e-9.

- **Normalised,** so the adaptation speed does not depend on the level.
- **Learns in its own frame:** the error is rotated back by `conj(spin)`
  before it is applied, so the taps model a static channel and do not chase
  the carrier.
- **Speed, measured** from the gain mode after a 3 dB step on 4 points (E4b):
  the error halves about every 600 symbols, a **time constant of about 870
  symbols (0.36 s at 2400)**. That fits `N/(μ(2−μ)) ≈ 783` and the code's own
  comment, "the best part of a thousand symbols" (`:135-137`). The reference's
  "1/μ = 50 symbols" is wrong.

### 3.3 Carrier loop (`:955-958`)

```rust
let wrong = (z * target.conj()).im / target.norm_sqr().max(0.1);
self.turn += FREQUENCY_GAIN * wrong;   // 4e-4, :131
self.rotation += PHASE_GAIN * wrong;   // 0.04, :130
```

- **Order.** `rotation += turn` runs first, every symbol (`:946`). The
  characteristic is `(z−1)² + Kp(z−1) + Ki`, and `Kp² = 4·Ki` exactly.
- **Response.** A **double root at 0.98**: exactly critically damped, 50-symbol
  time constant (20.8 ms at 2400, 14.6 ms at 3429).
- **Bandwidth.** One-sided noise bandwidth `B_L·T = 0.0127`: **30.5 Hz at 2400
  baud**, 43.6 Hz at 3429. This was integrated numerically from the closed
  loop. The reference gives roots 0.983/0.977 and 6.8 Hz; both are wrong.
- **Offset.** Type 2, so a constant offset leaves no standing phase error. It
  starts from training's `turn`.
- **No clamp** on `turn` or `rotation`. The detector's gain is reduced for
  targets with `|t|² < 0.1` (section 9, §7.9).

### 3.4 Timing loop (`:910`, `:916-920`, `:959-967`)

- **Error detector.** `rate` is the same 31 taps applied to central differences
  of the halves, `(x_{i+1} − x_{i−1})/2`, i.e. the output's derivative in
  half-symbols. `slope` is an EMA (0.01) of `|rate|²` (`:963`). Then
  `late = Re(e·conj(rate·spin))/slope`, clamped to **±0.5 half-symbol**
  (`:964`). It is data-aided, not Gardner's.
- **Correction.** `due −= 0.01·late·half` (`:965`), and
  `drift = (drift − 1.25e-5·late)` clamped to **±0.001 = ±1000 ppm** (`:967`).
  `timed` accumulates the corrections for `rewind` (`:966`).
- **Characteristic.** Two halves a symbol give `(z−1)² + Kp(z−1) + 2Ki`, and
  `Kp² = 8·Ki` exactly: a **double root at 0.995**. That is a 200-symbol time
  constant (83 ms at 2400), `ζ = 1`, and `B_L·T = 0.00314` (**7.5 Hz at 2400**,
  10.8 Hz at 3429).
- **Latency.** A correction lands on `due`, about `REACH+2` = 17 halves (8.5
  symbols) ahead of the symbol that measured it. With 8–9 symbols of delay the
  largest root moves only from 0.9950 to 0.9958.
- **Measured.** 114 ppm, and 120 s at 114 ppm and 35 dB, holds 39.8–39.9 dB in
  every 10 s block, with drift reading 114.0 ppm (E5). There is no sign of the
  taps and the timing wandering apart in two minutes (section 9, N6).

**Why the loops do not fight.** Carrier 50, timing 200, equaliser about 870
symbols, each about 4× the last. The equaliser shares a degree of freedom with
each loop: phase with the carrier loop, delay with the timing loop. Each time,
the loop is faster and takes it before the equaliser can, which is what the
comment at `:133-139` intends. Copy the ratio, not only the numbers.

### 3.5 Level and gain

- **There is no AGC.** `power` is an EMA at 0.002 per half (`:681`): 500
  halves, 104 ms at 2400. It is used only as a carrier-present test (`level()`
  `:607`; `training.rs:1057`, `:1499`, against 1e-4).
- **All gain lives in the taps.**
  - set by the least-squares solve;
  - followed by NLMS at about 870 symbols;
  - re-fitted in one step by `resync_dense` only (`:1098-1101`);
  - **the plain `resync` fits no gain.**
- The consequences for V.32 are in section 7.2 and in section 9 (§7.2).

### 3.6 Tracking decisions ahead of the trellis decoder

- **Data mode switches the slicer.** V.34 data mode sets `Slicer::Grid` from
  the decoder's `grid_scale()`/`extent()` (`training.rs:1426`, `:1020`;
  `v90/digital.rs:1096`). That slices to the nearest odd integer on each axis,
  clamped to ±`limit` (`:157-161`). It ignores the trellis, shaping,
  precoding and non-linear coding.
- **The decoder runs behind.** The Viterbi decoder (`data.rs`, `DEPTH` = 40 4D
  symbols = 80 2D symbols, `data.rs:40`) is fed `symbol.point`, the equalised
  and derotated sample (`training.rs:1317`). The loops never wait for it.
- **Loss arithmetic follows the density.** The grid has its own thresholds
  (`:180-215`), because garbage on a dense grid reads only
  `d_min²/6` (comment `:173-179`).
- **V.32 today does the same thing** with `Coded::nearest` for the equaliser
  and `trellis::Decoder` for the data (`v32.rs:1061-1091`). Its decoder's
  depth is 24 symbols (`v32/trellis.rs:558`).

---

## 4. When things go wrong

### 4.1 Loss detection (`:930-945`, thresholds `:180-215`)

The loss window `recent` holds the last `judged` squared errors: 8 on
`Points`, 32 on `Grid`. Loss is declared when their mean passes
`lost_threshold(settled)` (`:936`):

| slicer | judged over | `lost_threshold` | `found_level` (resync acceptance) | first resync | resync reads |
|---|---|---|---|---|---|
| `Points` | 8 symbols | `max(8·settled, 0.25·d²)` | `max(4·settled, d²/16)` | 32 symbols after loss | 48 symbols (`resync`) |
| `Grid` | 32 symbols | `max(2·settled, d²/12)` | `max(2·settled, 0.4·d²/6)` | 96 symbols after loss (`window().0/2` of 192) | 64 symbols (`resync_dense`) |

Then `lost = Some(0)` and `rewind(judged + 16)` (`:940-941`).

**The `Points` floor is high.** `0.25·d²` is 0.1 on 16 points: an SNR of
10 dB. A disturbance that leaves decisions at 12–20 dB is never declared
lost, and every loop keeps learning from it. E4b's −6 dB ramp ends in a
permanent false lock at 12 dB (all points decided inward), with `lost` never
set. `settled` rose to meet it, because those errors pass the gate
(`0.05 < 0.1`).

### 4.2 Rewind (`:971-986`, `:997-1013`)

- **Snapshots.** `Loops` (`:441-451`) is taps, rotation, turn, drift, slope,
  timed, settled and error. One is taken every `EARLIER_EVERY = 16` symbols
  while not lost, and 24 are kept (`:112-113`): 384 symbols, 160 ms at 2400.
- **Restoring.** `rewind(back)` restores the newest snapshot at least `back`
  symbols old. It carries the rotation on by `turn·elapsed` and moves `due` by
  `loops.timed − timed`.
- **Failure is silent.** With no snapshot old enough it does nothing
  (`:999`).
- **Other caller.** `training.rs:1017` rewinds by `off_run + 32` when it finds
  data where E should have been.

### 4.3 Resync on four or sixteen points (`resync` `:1148-1211`)

- **When.** Called from `on_half` (`:718-723`) when `lost ≥ window/2` and
  `lost % 32 == 0`. On `Points` that is 32, 64, 96… symbols after loss (13 ms
  apart at 2400); on `Grid`, 96, 128….
- **Reading.**
  - The newest 48 symbols (`:102`) are re-read from the raw history at 16
    shifts, `(step−8)/8` of a half-symbol: −1 to +0.875 (`:1167-1169`).
  - Each reading goes through the frozen taps. The turn comes from the fourth
    power, taking the quarter nearest the previous rotation (`:1174-1180`),
    then three decision-directed refinements (`:1183-1190`).
- **Acceptance** (`:1202-1206`): `mse ≤ found_level`, and
  `mse ≤ 0.6 × the median over all shifts`. The second test calibrates itself.
- **Ambiguities.** It resolves timing modulo one symbol and phase modulo a
  quarter turn. Neither ambiguity matters to differentially coded data
  (comment `:1142-1147`), which holds for V.32 as well: Table 1 differential
  quadrants, and a trellis code that is invariant to 90° turns (spec.md §2.4).
- **No gain fit.**
- **Defect (measured).** The window ends at `next_symbol − 2` (`:1157`). Its
  last half-symbol is the newest one made, so any shift later than the one
  sample or so of slack needs line samples not yet taken. `interpolate`
  returns `None` (`:672`) and the whole shift is skipped (`:1170`).
  - Positive shifts beyond about +0.3 half-symbol at 2400/16 kHz (about +0.43
    at 3429) are never evaluated, and −1 half is too far to substitute.
  - E3e: a pure timing jump of **+0.25 to +0.35 symbol fails at every phase**,
    and everything else recovers with one resync. E3f: the band moves to
    +0.30 to +0.375 symbol at 3429 baud, as the slack predicts.
  - E3c: at 2400/1800, 32–35 of the 339 insertion lengths from 2 to 340
    samples are never recovered, depending on the seed. The core of the set,
    lengths ≡ 2 or 9 (mod 20), is the same for all three noise seeds; a few
    lengths at its edges vary. 20 samples is 3 symbols and 2¼ carrier turns,
    so the pattern repeats every 20 on a 90°-symmetric constellation.
  - 4 points mostly ride through (E3f), but a one-sample repeat at the edge
    of the band stayed lost for 1.1 s (E3h).
  - `resync_dense` stops 4 symbols short (`:1032`, "so that reading them
    later still has samples to read") and is not affected.
  - V.34 has never hit it live, because a 20 ms slip is 0.57 symbol at 3429
    baud.
  - **Fix in the copy:** end the window 4 symbols short, as the dense one
    does. Report it separately for V.34 as a latent bug.

### 4.4 Resync on a dense grid (`resync_dense` `:1027-1103`)

- **Search.** A 64-symbol window (`:117`) ending 5 symbols short (`:1032`),
  searched over:
  - 32 timing shifts across ±1 half (`:122`, `:1071-1072`);
  - a coarse quarter turn in 60 steps of 1.5° (`:123`, `:1061-1066`);
  - 4 rounds of complex least squares for gain *and* phase against the
    decisions (`fit` `:1049-1059`);
  - fine steps of ±1–3/64 half around the best (`:124`, `:1082-1091`).
- **Acceptance** as above (`:1094`). The fitted gain goes into the taps
  (`:1098-1101`) and the phase into the carrier.
- **Why it exists.** The fourth power is near nought on a shaped
  constellation, and 832 points read as noise a degree or a percent off
  (`:1020-1026`).

### 4.5 Taking up again (`take_up` `:1108-1136`)

- Every stored half from `from` onwards is re-read at `times + moved`.
- Where the history runs out the halves are truncated, to be made again when
  samples arrive (`:1124-1128`); otherwise `due += moved`.
- The rotation is set to `turned + turn·window/2` (`:1132`), `lost` and
  `recent` are cleared, and `slips += 1`.
- The V.34 driver watches `slips()` and restarts its frame search
  (`training.rs:1284-1289`). V.32 has no framing to find again.

### 4.6 Retrain and renegotiation, as V.34 does them

- **Renegotiation keeps the receiver.**
  - `SWatch` (`training.rs:638-684`) spots S in data mode from the *equalised*
    points: 24 symbols of S (`:167`) → `clamp` → `set_size(Four)`
    (`:1005-1008`).
  - After S-bar, TRN is tracked decision-directed at four points
    (`:1066-1072`), with no least-squares retraining.
  - E → `set_grid` for the new rate (`:1426`).
  - This is exactly what V.32bis clause 8 needs. Its preamble is AA 56T + CC
    8T (call) or AC 56T + CA 8T (answer), then R4/R5 and E at four points, then
    data, and **no TRN** (spec.md §5.2).
- **Retrain rebuilds the receiver.**
  - `RetrainWatch` (`training.rs:237-302`) hears the far end's tone on the
    **raw line**. It needs 55 ms, at least 6× above ±150 Hz either side, and
    above 0.008.
  - That sets `wants_retrain`, and `startup.rs:228` builds a new
    `training::Modem`, with a new `Receiver`, after phase 2.
  - A renegotiation this end began that goes unanswered becomes a retrain
    (`training.rs:1218-1224`).
- **A lasting loss in data mode leads to neither.** Nothing reaches
  `wants_retrain`, and the modem crate's door is used only by tests
  (`modem/src/lib.rs:1885-1888`: "Nothing on a real call calls it").

### 4.7 VoIP slips and the round trip, at 2400 baud (E3, E3b, E3c, E3g, E3h)

- **20 ms slips.** 20 ms is exactly 48 symbols, and 36 turns of 1800 Hz (32 of
  1600). The loops see no jump at all: 1 bad symbol, no resync, 42.6 dB after.
  10 ms (24 symbols) and 12.5 ms (30 symbols; a half turn, invisible on
  90°-symmetric constellations) behave the same. Rory's measured slips are
  +20.0 ms (memory `voip-jitter-slips`). **On V.32 those slips cost only data,
  never lock.**
- **Other lengths from 0.5 to 21 ms** recover after one resync, with 2–56 bad
  symbols (E3b), except the band in 4.3.
- **One sample repeated or dropped** (a sound-card underrun):
  - on 16 points, resynced at all 16 positions tried, about 23 bad symbols
    each;
  - on 4 points, ridden through at most positions, but one repeat stayed lost
    for 1.1 s before a resync took (E3h), at the band's edge.
  - `contract.md` §7 F/H finds that this very fault forces today's V.32
    receiver into a retrain, and then a permanent `stop_offering`.
- **What a slip costs in data.** 48 symbols dropped or repeated is 96–288 bits.
  The descrambler resynchronises itself in 23 bits, Table 1 coding is
  differential, and the Viterbi decoder allows every state
  (`trellis.rs:592-600`), so the stream recovers. V.42 retransmits the frame.
- **The 1.2–1.5 s round trip** never reaches the core: its buffers are at most
  1.02 s and it never waits for the far end. It does decide how long the V.32
  start-up keeps the receiver idle or hunting. A trained receiver facing
  silence, as in V.32's half-duplex gaps, goes lost and resyncs on nothing
  every 32 symbols. The driver must `idle()` or `hunt()` it, as V.34's does
  (`training.rs:1383`, `:1566`).

---

## 5. What is V.34-specific, and what is generic

| V.34-specific (stays with V.34, or gets a V.32 counterpart) | Generic QAM machinery (the core) |
|---|---|
| `Reference` and `sequence()` (`:220-225`, `:1228-1238`): PP by 10-1, V.34 TRN mapping via `signals::Sender` | Fixed mixer, history, interpolating filter (`:456-472`, `:642-678`) |
| `windows()` (`:1217-1225`): tied to `PP_SYMBOLS` = 288 and TRN of 512T | Half-sample ring with sample times (`:680-690`) |
| `reacquire` (`:857-906`), which reads V.34 TRN through `signals::Reader` | Least squares on stored rows: alignment search, turn estimate, pre-rotated solve, wide retry (`:786-845`, given targets and windows) |
| `Slicer::Points(Size)` and `unit()` (`:1243-1263`): V.34's (±1,±1) four points and its 16 | NLMS in its own frame, carrier loop, timing loop, gate, `settled`/`error` bookkeeping (`:909-991`) |
| `Slicer::Grid`'s extent with precoder room (`data.rs:369-378`) | Loss detection parameterised by the slicer (`:173-215`) |
| `Band` and Table 1/2 carriers (`qam.rs:31-65`, `probe.rs:248-270`); `ROLLOFF = 0.1` in the cutoff | Snapshots and `rewind` (`:441-451`, `:971-1013`) |
| `data.rs`: shell mapping, precoding, non-linear coding, the 4D 16/32/64-state trellis, superframe `Acquirer` (all outside the receiver) | `resync` and `resync_dense`, `take_up` (`:1027-1211`) |
| `training.rs`: `SWatch` (S in data), `RetrainWatch` on tones A/B at 2400/1200, `Listening` (J, J′, MP, E), MP rate choice | `resume` (`:548-559`) |
| — | `Hunt` (`:285-364`): generic for both, since both S signals are two points a quarter apart and S-bar is −S. It needs V.32's arming or discriminator (2.1) |
| `Symbol.decided: Point`, V.34 grid units | `Heard` events (`:229-240`) |

`v34/receiver.rs:53`, `data.rs:34`, `signals.rs:9`, `training.rs:51`,
`v90/digital.rs:24`, `v90/pcm.rs:37` and `v90/dil.rs:115` import
`crate::v32::{Mode, Scrambler}`. A V.32 rewrite must keep both exactly
(contract.md §1.1).

---

## 6. Does it already run at 2400 baud on 1800 Hz?

- **In the code, yes.**
  - `Band::new(SymbolRate::S2400, true)` gives `baud()` = 2400 (`probe.rs:260-270`)
    and `carrier()` = 2400·3/4 = **1800 Hz** (`probe.rs:251`); the low carrier
    is 1600 Hz.
  - `Receiver::new` (`:454-511`) then has `half` = 3.333 samples at 16 kHz,
    cutoff 1620 Hz, 31 taps = 6.46 ms.
  - Everything else is in symbols, so the time constants scale: history
    1.024 s, ring 0.853 s, snapshots 160 ms, carrier 20.8 ms, timing 83 ms,
    NLMS about 0.36 s, resync every 13 ms while lost.
- **Tests.** `every_symbol_rate_and_carrier_trains` (`:1664-1675`) trains both
  2400 carriers at 50 ppm, 10 dB loss and 40 dB noise, asserting more than
  28 dB and J read. E1 measures 54.1 dB (trained) and 54.5 dB (tracked) on a
  clean line at 2400/1800. There is no ceiling below 53 dB at any V.34 rate
  either (section 9, §7.1).
- **Live, 2026-09-22** (`dist/captures/live-1790041800.wav`, 46 s, 16 kHz,
  stereo; channel 0 = heard, channel 1 = sent, `gui/src/live.rs:1622`):
  - spectral centroid in 2 s windows at 30, 34 and 38 s:
    - channel 0 at 1594/1584/1625 Hz, −10 dB span about 300–2900 Hz: **2400
      baud on the low carrier, 1600 Hz**;
    - channel 1 at 1784/1783/1801 Hz, span 540–3050 Hz: **our 2400 baud on
      1800 Hz**, with the −70 dB skirts of our transmitter.
  - The `.frames.txt` shows V.42 frames decoded from the far end from 38.1 s:
    XID replies, UA, I-frames.
  - So **the receiver's 2400-baud path is live-proven at 1600 Hz**. At 1800 Hz
    it differs only in the mixer step (`:478`), with more image guard (960
    against 560 Hz). It has not yet received live at 1800.
- **Also at 8 kHz:** V.90's digital modem runs the same receiver on its
  upstream (`v90/digital.rs:543`, `FS = 8000`).

---

## 7. The API a shared core needs to serve V.32

A sketch. Names are placeholders; semantics are what matter. Rule 9 of
`slow-modes/plan.md` puts copied V.34 mechanisms in `crates/dsp`, which
already has `Complex` and `least_squares`.

```rust
pub struct Band { pub fs: f64, pub baud: f64, pub carrier: f64, pub rolloff: f64 }

pub struct Core { /* everything in receiver.rs's Receiver except Mode3::Collecting's V.34 bits */ }

impl Core {
    pub fn new(band: Band) -> Self;                 // cutoff 0.5·baud·(1+rolloff)+300, ≤ 0.45 fs
    pub fn feed(&mut self, sample: f64);            // raw line OR echo-cancelled residual
    pub fn heard(&mut self) -> Option<Heard>;       // S{turn}, Reversal{at}, Trained{snr_db}, Untrained, Symbol(Symbol)

    // acquisition
    pub fn hunt(&mut self);                         // S/S-bar template, as Hunt
    pub fn train(&mut self, t: Training);           // caller-supplied known sequence
    pub fn acquire_blind(&mut self, slicer: Slicer);// section 7.8
    pub fn resume(&mut self, first_half: u64) -> bool;
    pub fn idle(&mut self);

    // mid-call changes, taps kept
    pub fn set_slicer(&mut self, slicer: Slicer);   // today's private set_slicer :590-594
    pub fn rewind(&mut self, back: u64) -> bool;    // true if it found a snapshot

    // reporting
    pub fn snr_db(&self) -> f64;                    // EMA of every symbol (:597)
    pub fn settled_snr_db(&self) -> f64;            // EMA of gated symbols (new)
    pub fn trained_snr_db(&self) -> f64;
    pub fn last_point(&self) -> Option<Complex>;    // unit mean power
    pub fn level(&self) -> f64;
    pub fn gain(&self) -> f64;                      // new, from the AGC of 7.2
    pub fn slips(&self) -> u32;
    pub fn is_lost(&self) -> bool;
    pub fn lost_for(&self) -> Option<usize>;        // symbols; for a retrain watchdog (new)
    pub fn drift_ppm(&self) -> f64;
    pub fn halves(&self) -> u64;
}

pub struct Training {
    pub targets: Vec<Complex>,     // unit power, from the sequence's first symbol
    pub start: u64,                // half-symbol targets[0] is centred on, ± search
    pub search: i64,               // ±halves (V.34: 8)
    pub align: (usize, usize),     // symbols the alignment is chosen over
    pub solve: (usize, usize),     // symbols the taps are solved over
    pub retry: Option<(usize, usize, i64)>, // window and wide search (V.34: ±200)
    pub turn: Option<f64>,         // rad/symbol, from S (2.2); None = V.34's behaviour
    pub accept_db: f64,            // 12
}

pub struct Symbol { pub point: Complex, pub target: Complex, pub label: u16, pub error: f64 }
```

### 7.1 The slicer

It must take V.32's constellations. All figures below are at unit mean power
(computed from `v32/trellis.rs` and `v32.rs:129-134`; 7.2 has the tolerances).

| use | points | lattice | d²_min | peak | smallest \|p\|² |
|---|---|---|---|---|---|
| TRN symbols 0–255 | 2 (A, C) | — | 4.0 | 1.0 | 1.0 |
| S, S-bar, TRN 256+, R, E, 4800 data | 4 (A–D = (−3,−1),(1,−3),(3,1),(−1,3)/√10) | subset of the odd grid | 2.0 | 1.0 | 1.0 |
| 9600 uncoded, 7200 TCM | 16 | odd grid ±1, ±3 | 0.40 | 1.342 | 0.200 |
| 9600 TCM | 32 cross | x+y odd | 0.20 | 1.304 | 0.100 |
| 12 000 TCM | 64 | odd grid ±1…±7 | 0.095 | 1.528 | 0.048 |
| 14 400 TCM | 128 cross | x+y odd | 0.049 | 1.440 | 0.024 |

- **V.34's `Grid` cannot slice the crosses.** It slices only the odd square
  grid. In `(u,v) = (x+y, x−y)` the crosses *are* the odd grid, so `Grid`
  would serve with a 45° pre-turn. A table (up to 128 points, as
  `Coded::nearest` already does) is simpler and clamps to the constellation.
- **V.34's `Points(Four)` is (±1,±1)**, not V.32's A–D.
- **Proposed enum.** `Slicer::{Points(Size), Grid{scale,limit}}` stay
  **exactly** as they are, for V.34, and `Slicer::Table(&'static
  Constellation)` is added. A `Constellation` holds its unit-power points with
  labels, and precomputes `d²_min`, `garbage` (the MSE of a signal of equal
  power against it) and a density class.
- **Loss thresholds by density, not by variant.** The class picks the
  arithmetic:
  - ≤ 16 points use the `Points` rules;
  - ≥ 32 use the `Grid` rules (`d²/12` floor, `2·settled`, 32-symbol
    judging, `resync_dense`).
  - The reason: on the 32/64/128 sets, garbage (≈ d²/6: 0.033, 0.016, 0.008)
    reads *below* the `Points` floor of `0.25·d²` (0.05, 0.024, 0.012), so a
    `Points`-style detector would never declare loss.
- **Tentative decisions** are the nearest table point. V.32's
  `trellis::Decoder` is fed `point·√10` (its tables are at mean power 10) and
  runs 24 symbols behind (`v32/trellis.rs:558`), as V.34's does. The `label`
  lets the uncoded 4800/9600 path read Table 1 quadrants straight from the
  decision.

### 7.2 Gain: what V.32 needs that V.34 does not have

**The gate's gain tolerance.** Solving `g·peak·d = d/2` gives the gain error
that takes the outermost point out of the gate:

| constellation | tolerance |
|---|---|
| 4 points | 4.65 dB |
| 16 | **1.84 dB** |
| 32 | 1.37 dB |
| 64 | 0.84 dB |
| 128 | **0.64 dB** |

E4 and E4b confirm the 16-point row:
- 2 dB steps are recovered by NLMS in about 1.5 s;
- 3 dB steps (and 3 dB over 20 ms) are lost for ever, because the plain
  resync has no gain fit;
- 3 dB ramps over 0.1–0.5 s are recovered;
- +6 dB over 0.5 s is lost for ever;
- −6 dB over 0.5 s false-locks at 12 dB.

On 4 points everything recovers, but slowly (about 1.5 s at the NLMS rate).
MicroSIP has a gain control (memory `v90-live-test-pending`; the test
`a_softphone_with_a_gain_control_and_a_hasty_jitter_buffer_is_followed`). So
V.32 above 4800 needs:

1. **A decision-free AGC.** Hold the mean `|z|²` to the constellation's known
   mean power (1 at unit power), taking every symbol whatever the gate says. Put
   its speed between the carrier and timing loops, about 100–200 symbols, and
   scale the taps (as `resync_dense` does) or a scalar in front of the slicer.
   - Noise biases it by σ²: 0.04 dB at 20 dB of SNR.
   - On the 128-cross, the power variance calls for averaging over at least
     500 symbols, or for acting only once the gate has frozen a large share of
     symbols.
2. **A gain fit in every resync.** Take `resync_dense`'s `fit` (`:1049-1059`)
   into the plain one too.
3. **A gate relative to `settled`**, e.g. `squared < min(doubtful, k·settled)`,
   so that `settled` cannot climb into a false lock (the −6 dB case). This
   needs (1) to be safe.

TRN (4 points) and data (up to 128) may arrive at slightly different powers.
The spec's figures normalise to the same mean power, but V.34's live far end
was 0.9% off (`:1025-1026`). V.32's B1, 128T of scrambled ones at the new
rate (5.4.1/5.4.2), is known once the descrambler is synchronised, so it is a
free gain reference at the rate change.

### 7.3 A trainer driven by a known symbol sequence

- **The caller supplies the targets.**
- **For V.32**, the targets are the far end's scrambler (`Mode::peer()`,
  started at zero) fed ones. Symbols 0–255 are A or C by the first bit of each
  dibit; from 256 the dibit maps by Table 5 as printed: **00 A, 01 B, 11 C,
  10 D** (spec.md §4.3 gives reference vectors).
- **Our transmitter currently disagrees.** `TRN_STATES[first<<1|second]`
  (`v32.rs:427`, `:708`) sends 10 as C and 11 as D. No test pins it
  (`v32_signals.rs:165-186` checks only the spectrum). Fix the transmitter in
  the same change, or loopback training fails. Confirm on
  `tests/vectors/v32bis-14400.wav` (contract.md §5) that a real far end follows
  Table 5: the right targets fit above 25 dB, the swapped ones cannot.
- **Least squares on the two-point segment is sound.** For a linear channel
  the normal equations use only `E[x xᴴ]`, which is the same for two-point and
  four-point input. The targets are complex points, so the phase is still
  absolute.
- **Windows.** At 2400 baud, with TRN of at least 1280T:
  - align over 16–256 (clear of S-bar's memory);
  - solve over 16–512;
  - retry over 640–1152 with ±200 halves (±42 ms, room for two 20 ms slips);
  - all of it fits the 4096-half ring.
- **Frequency** from S (2.2), before the solve.
- **`train()` becomes a V.34 wrapper**: `Training { targets: sequence(..),
  windows(..) .. }`, bit-identical to today.

### 7.4 Fed an echo-cancelled residual

- **`feed(f64)` accepts either.** The core never needs the raw line.
- **V.32 already hands its receiver the residual:**
  `cleaned = echo.process(sent, line)` (`v32/startup.rs:2200`) goes to
  `Startup::step` (`:2253`) and on to `rx.feed` (`:1253`).
- **History re-reads the residual as it was computed.** A canceller that goes
  on adapting in data mode changes nothing that has already been stored.
- **The canceller's own timing.** It is frozen after the first TRN
  (`startup.rs:2230-2240`), which contract.md §7 F shows fails under drift on
  a cable. That is a `Modem`-level change, outside the core.
- **Retrain tones** (V.32bis 7.1/7.2: 600/3000 Hz from the answer end, 1800 Hz
  from the call end, "for more than 128 symbol intervals") stay on the raw or
  residual line, as V.34's `RetrainWatch` does. They are not a core job.

### 7.5 Rate or constellation change mid-call, taps kept

- **`set_slicer(slicer)`**, made public. It keeps taps and loops and clears
  `lost` and `recent`, which is right (comment `:581-589`).
- **Carry `settled` across.** Noise at unit power does not depend on the
  constellation, so keeping it, as today, is right; section 9, §7.7, explains
  why that reference claim does not hold.
- **At V.32's E**, switch to the data table at the symbol 5.4 names. At a
  V.32bis §8 preamble (AA/CC or AC/CA, detected from equalised points like
  `SWatch`), switch back to four points. No retrain either way.

### 7.6 Reporting for a scope and the modem crate

- `Heard::Symbol` gives every symbol, and `last_point()` the newest.
  contract.md §1.1: the GUI keeps `constellation_point()` only when it
  changes, so it must change exactly once per symbol, which a
  one-point-per-symbol core does naturally.
- **The modem crate reads `residual_error()` and `point_spacing()`**
  (`reception`, the retrain rule `startup.rs:1878-1881`, `stop_offering`).
  - Provide `sqrt(error EMA)` and the table's `d_min`, both at unit power.
  - Keep in mind that `snr_db()` today is the EMA of *every* symbol, gated or
    not (`:988`).
- `settled_snr_db()`, `slips()`, `is_lost()`, `lost_for()` and `gain()` are
  what a transcript, or Rory, needs to tell a slip from a gain change from a
  line going bad.

### 7.7 Driver responsibilities that stay in `v32`

These do not belong in the core:
- arming `hunt()` only when S is due (2.1);
- idling through the half-duplex gaps (4.7);
- the TRN target generator (7.3);
- R/E reading (Table 1 differential dibits);
- detecting the V.32bis §8 preamble;
- a **watchdog**: lost for more than about 1–2 s, or unsatisfactory, starts a
  clause 7 retrain. V.32bis 7 allows it "if either modem incorporates a means
  of detecting unsatisfactory signal reception". V.34 has none (section 9,
  §7.4).

### 7.8 Blind acquisition (required by the contract, absent in V.34)

The bare-receiver tests (`contract.md` §1.2; plan.md rule 4 forbids editing
them) acquire with no training sequence:
- 4800 from `0x55` data at any of 8 arrival phases;
- 9600T from data at arrival phase 0.

The V.34 machinery has the parts for it:
- centre-tap taps (`:473-474`);
- gain from the AGC of 7.2;
- then **the resync search itself as acquisition**: 16 or 32 timing shifts ×
  fourth-power or grid phase, on a 48–64-symbol window, accepted by the same
  median test;
- then decision-directed tracking.

On the tests' flat loopback lines this is likely enough. It must be
measured, not assumed. The trained path remains the one real calls use,
because the start-up always provides S/S-bar/TRN.

---

## 8. The three options

**(a) Extract a shared QAM core that both V.34 and V.32 use.**
- For: one copy of the machinery; fixes land once; V.32 inherits a tree-proven
  design, and later modes (V.29, V.17) can use it too.
- Against: moving V.34 and V.90, both live-proven, onto a refactored core puts
  them at risk. The current tests would not see a subtle regression: every
  V.34/V.90 assertion is a floor (for example trained > 28 dB, or J found, or
  an MP within 0.1 s of a slip), and none pins the output.

**(b) A new V.32 receiver modelled closely on V.34's, sharing only small dsp
pieces** (`least_squares`, the filter design).
- For: zero risk to V.34/V.90; V.32 can diverge freely.
- Against: about 1000 lines duplicated and two copies to fix. The look-ahead
  defect of 4.3 would need fixing twice, and the copies drift apart.

**(c) Instantiate the V.34 `Receiver` directly for V.32.**
- `Receiver::new(Band::new(S2400, true), fs)` does give 2400/1800 today, and
  `feed` would take the residual.
- But `Reference` knows only PP and V.34's TRN (`:220-225`, `:1228-1238`), and
  `Slicer` is private and V.34-only: (±1,±1) four points, the odd grid, no
  crosses.
- `reacquire` reads V.34 TRN, the hunt fires on AA/AC, and there is no gain
  fit, AGC or blind path.
- Every one of those needs an edit to `v34/receiver.rs`: (a)'s risk with none
  of (a)'s structure, and a breach of plan.md rule 9. **Not viable.**

**Recommendation: (a) as the architecture, reached the way plan.md rule 9
already prescribes.** The steps:

1. **Copy, don't move.** Copy `receiver.rs`'s generic parts (section 5, right
   column) into a new core in `crates/dsp` (additive, rule 3), citing the
   source lines, with the API of section 7. `v34/` and `v90/` stay
   byte-identical (rule 9).
2. **Build V.32 on the core**, with the four defect fixes (look-ahead resync,
   AGC and gain fit, frequency from S, hunt arming or discriminator), the
   Table 5 transmitter fix, the blind path and the watchdog.
3. **Keep the core's V.34-equivalent paths numerically identical.** Same
   operations, same order, `Points`/`Grid` thresholds untouched, V.34's new
   options (`turn`, AGC, gain fit, relative gate) default to off. Then a later
   switch of V.34 is a mechanical change.
4. **Switching V.34 is a separate job, gated by a new golden test written
   first.** Record the current receiver's full `Heard` stream as exact f64
   bits or a digest. Sources: every receiver unit-test signal,
   `tests/vectors/v34-33600.wav`, and one or two `dist/captures` files. Assert
   bit-equality after the switch. The V.34 look-ahead fix is a one-line change
   to live code, to be made (or not) with Rory's say, with a test built from
   E3f.

**Risks of the recommendation:**
- the two receivers diverge until V.34 is switched: bounded by step 3 and the
  golden test;
- V.32-driven changes to the core leak into V.34's defaults: bounded by the
  default-off rule;
- the blind path may not meet the bare-receiver tests at 9600T: measure early
  (7.8);
- the AGC and the NLMS gain mode interact: the AGC must be the faster, and
  decision-free.

**What guards the V.34 receiver today** (all passing on this tree):

| where | count | what it exercises |
|---|---|---|
| `receiver.rs:1405-1675` | 7 | clean train and J; ±114/200 ppm; 8 kHz VoIP; phase 4 with and without scrambler restart (`reacquire`); two 20 ms slips at 3429 with MP′ back within 0.1 s; slip inside training; every rate and carrier |
| `training.rs` tests | 15 | phases 3–4 end to end; VoIP round trip plus 114 ppm; data both ways; renegotiation from either end, over VoIP, unanswered; slip in data mode; lost E found from data; cleardown; S heard in data; silent far end; tone B floor |
| `v34/startup.rs` tests | 5 | INFO0 to E; data at 33 600; full retrain through phase 2; lost INFO1a; 70 ms silence |
| `tests/v34_vector.rs` | 4 (+1 ignored) | Conexant 33 600 recording: S-bar at 6.09–6.12 s, trained > 15 dB, > 5000 TRN symbols, J (16 points) at 8.0–8.2 s |
| `tests/v34_capture.rs` | 3 ignored | live captures in `dist/captures` via env vars |
| `tests/v90_call.rs` | 48 | the digital modem's upstream *is* this receiver at 8 kHz: slips, softphone gain control, 120 ppm, holes, retrain storms, renegotiation |
| `tests/v90_vector.rs` | 6 | V.90 sequences |
| `modem/tests/call.rs` | V.34 subset | `two_modems_asked_for_v34_connect_at_33600_and_carry_data`, `a_v34_call_retrains_the_whole_way_and_comes_back`, `a_v34_caller_meets_a_v32bis_modem_on_v32bis` |
| `modem/tests/v90_call.rs` | 4 | V.90 through the modem crate |

The acceptance bar for the new V.32 receiver is `contract.md` §4 (the V.32
tests) and `lock_sweep` as plan.md rules 5–8 describe.

---

## 9. The weaknesses of `v34-reference.md` §7, checked against this tree

| § | claim | verdict | for the V.32 port |
|---|---|---|---|
| 7.1 | The mixer's image caps the SNR below 35.5 dB at the top rates | **Not real.** E1: clean-line training gives 53.4–54.1 dB at every rate and both carriers, 3429 and 3200-low included | Irrelevant anyway: the guard at 2400/1800 is 600–960 Hz |
| 7.2 | The gate is absolute while gain error is proportional; there is no AGC | **Real, and worse than stated**: on 16 points a 3 dB step is lost for ever, −6 dB false-locks, and there is no gain fit outside `resync_dense` (E4/E4b) | **Must fix**: AGC, gain fit in every resync, relative gate (7.2). Tolerance 0.64 dB at 14 400 |
| 7.3 | With non-linear coding the tracking slicer slices the wrong points | Real in principle, latent (our MP never asks, `training.rs:1523`; `v90/digital.rs:1122`) | Not applicable: V.32 has no non-linear coding |
| 7.4 | A data-mode receiver can stay lost for ever; nothing asks for a retrain | **Real** (no path from `is_lost` or repeated search to `wants_retrain`; `modem/src/lib.rs:1885-1888`) | Add the watchdog (7.7) |
| 7.5 | `rewind` does nothing, silently, when asked to reach too far | **Real** (`:999`) | Return `bool`; fall back to the oldest snapshot, or count it |
| 7.6 | `resync_dense` leaves the rotation stale | **Real, 4.5 symbols of turn** (the window's middle is 36.5 symbols before `next_symbol`; `:1132` adds 32), not 5 | Extrapolate to `next_symbol`: `turn·(window/2 + gap)` |
| 7.7 | `settled` survives a change of slicer, making the grid's threshold too lax | **Mostly not real.** Noise at unit power is the same whatever the constellation, so a `settled` carried over is the right reference; the "5× too lax" compares it with a floor that sits below the noise | Keep carrying it; reset only if the old stream was not being tracked |
| 7.8 | The grid's limit is wider than the constellation | Real, V.34-only (precoder room with precoding off) | Table slicers clamp to the constellation |
| 7.9 | The phase detector loses gain on inner points (`max(0.1)`, `:955`) | Real; costs V.34 about 3% of bandwidth | **Worse for V.32**: inner \|p\|² is 0.024 at 14 400 (detector gain 0.24) and 0.048 at 12 000. Clamp near 0.02, or use the unnormalised `Im(z·conj t)` weighted by \|t\|² |

**Other corrections to `v34-reference.md`:**
- §3.1: `doubtful` is the decision boundary itself (`|e| < d/2`), not half of
  it.
- §3.2: NLMS takes about 870 symbols, not 50.
- §3.3: the carrier loop has a double root at 0.98, with B_L = 30.5 Hz (2400)
  or 43.6 Hz (3429), not 6.8 Hz.
- Its `training.rs` line numbers are about 30 lines stale (for example
  `Untrained` is now at `:1265`, not `:1232`).
- Its test count was 107; it is now 112.

**New, from the probes** (not in the reference):

- **N1 (high for V.32; latent in V.34):** the plain resync's look-ahead band
  (4.3).
- **N2 (spec, not VoIP):** carrier offset. Training degrades from 2 Hz, and at
  7 Hz tracking is stuck about 5 dB low for over 20 s. The turn estimate
  aliases at 10 Hz (2400) or 14.3 Hz (3429) (2.2).
- **N3:** the hunt takes AA/CC and AC/CA for S and S-bar (2.1).
- **N4:** a trained receiver facing silence keeps resyncing on nothing; the
  driver must idle it (4.7).
- **N5:** `Points`-style loss thresholds never fire on V.32's 32/64/128 sets,
  whose garbage sits below `0.25·d²` (7.1).
- **N6 (unproven):** nothing anchors the taps' centroid. Timing and taps share
  the delay degree of freedom, and adaptation noise could walk it over a long
  call. E5 shows no drift in 120 s. Track the centroid in a long simulated
  call before relying on hour-long sessions.

---

## 10. The probes, for reproduction

A scratch crate outside the repository:

```toml
[dependencies]
datapump = { path = "F:/dialupmodem2/crates/datapump" }
dsp      = { path = "F:/dialupmodem2/crates/dsp" }
[workspace]
```

It uses the public `v34::receiver::{Receiver, Heard, Reference, unit}`,
`v34::qam::{Band, Transmitter}`, `v34::signals`, `dsp::Resampler` and
`dsp::rrc_at`. The line is `receiver.rs`'s own test line (`:1338-1355`):
resampled for ppm, then loss, then uniform noise. Slips use its `slip`
(`:1572-1587`: repeat the previous n samples with a 40-sample fade, or drop
n). Signals are the silence/S/S-bar/PP/TRN sequences of its tests, sent
either through `qam::Transmitter` (E1) or through my own RRC modulator at any
roll-off and carrier. For pure jumps, the symbol clock and the carrier are
moved independently from one sample on:

```rust
// from sample `at`: symbols `delay` symbols later, carrier turned by `turn` revolutions
let (d, p) = if n >= at { (delay, turn) } else { (0.0, 0.0) };
let t = n as f64 * baud / FS - d;                              // time in symbols
let b: Complex = (k0 - 16..=k0 + 16).map(|k| sym[k] * rrc_at(t - k as f64, rolloff) * hann).sum();
let angle = TAU * (fc * n as f64 / FS + p);
out.push(b.re * angle.cos() - b.im * angle.sin());
```

| probe | set-up | result |
|---|---|---|
| E1 | V.34 Transmitter, phase 3 + 3000 TRN, 10 dB loss, no noise, 0 ppm, all 12 bands | trained 53.3–54.1 dB, tracked 53.7–54.5 dB |
| E2 | 2400/1800 + Δf, 25% roll-off, `Trn(Four)`, 114 ppm, 35 dB | trained 34.1 / 28.9 / 22.8 / 14.6 / 15.8 / 7.5 dB at 0/1/2/4/7/10 Hz; tracked 39.6 → 24.5 dB |
| E2b | same, 20 s, per-second SNR | 4 Hz back to 39.6 dB within 2 s; 7 Hz from 31.6 to 35.4 dB over 20 s |
| E3 | 16 points, insert at 2 s, drop at 4.5 s, 114 ppm, 38 dB | 20/10/12.5 ms: 0–2 bad symbols, no resync; 7 ms: resynced; 3.1 ms insert: never recovered |
| E3b | insert lengths 0.5–21 ms in 0.5 ms steps, 16 and 4 points | all recovered |
| E3c | insert lengths 2–340 samples, 16 points, 3 seeds | 32–35 lengths never recovered per seed; the core (≡ 2 or 9 mod 20) is common to all three |
| E3e | pure timing × phase jumps, 16 points | fails at +0.25/0.30/0.35 symbol at every phase; all else one resync or none |
| E3f | pure timing jumps in 0.025-symbol steps | 2400 16 pts: fail 0.25–0.35; 2400 4 pts: none; 3429 16 pts: fail 0.30–0.375 |
| E3g/E3h | one sample repeated/dropped, 16 positions | 16 pts: all resynced (about 23 bad symbols each); 4 pts: one lost for 1.1 s |
| E4/E4b | gain steps and ramps, 16 and 4 points | see 7.2 |
| E5 | 120 s, 16 points, 114 ppm, 35 dB | 39.8–39.9 dB throughout, drift 114.0 ppm |
| E6 | AA→CC, AC→CA and V.32 S/S-bar into `hunt()` | `S` + `Reversal` on all three; the true S-bar at 0.192 s |

The loop figures (section 3) come from the difference equations as coded,
with noise bandwidth integrated over the closed loop. The filter figures
(section 1) come from rebuilding `receiver.rs:456-472`. The capture figures
(section 6) are 4096-point Welch spectra of each channel.
