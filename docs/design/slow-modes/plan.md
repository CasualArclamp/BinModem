# Reworking the older receivers

Rory's judgement starts this: *"the 33.6 QAM modes are locked in place, but some
of the older modes are using old code."* `v34-reference.md` turned that into a
list of mechanisms, `before.md` turned the list into numbers, and this turns the
numbers into seventeen packages over six waves.

---

## Why this shape

Two plans were written first. This one takes the skeleton of one and the
substance of the other, for three reasons that are worth stating before the
packages.

**Worst mode first, because the worst five rows are one file.** `before.md`'s
own ranking puts V.32bis 14400T (15 failures), V.32 9600T (13), V.32bis 12000T
(13), V.32bis 7200T (12) and V.32 9600 (8) at the top, and every one of them is
`crates/datapump/src/v32.rs`. So the spine of this plan is a chain of five
packages on that one file, strictly sequential because a wave may not give two
packages the same file. Everything else is scheduled into the same waves because
its files are disjoint, ordered worst-first among themselves.

**Something measurable lands in wave 1.** A mechanism-first plan that builds five
or nine shared primitives before a single `before.md` cell moves is building API
against call sites nobody has tried yet. Here, wave 1 finishes Bell 103 and V.21
outright, clears every V.32 rate's `round trip 1.1 s` row, and removes the V.22
1200 rate lottery. The shared primitives that are genuinely substantial —
`dsp::CarrierLoop`, `dsp::Lock`, `dsp::Interpolator` + `dsp::HalfSpacedEqualizer`
— still get their own package with their own unit tests, but each is scheduled
into a wave whose V.32 slot is already occupied, so it costs nothing in schedule
and its adopter in the next wave is a clean, single-cause change. A primitive
that is two additive methods on an existing type is not worth a package of its
own and is born in its first adopter.

**The harness has no training sequence, and that decides several packages.**
`lock_sweep.rs`'s `transmit` (line 522) drives `v32::Transmitter` and
`v22bis::Transmitter` with random data from the first symbol — no S, no S̄, no
TRN, no unscrambled ones, no double dibit. Only V.29 and V.27 ter get real
training, because their `start()` emits it (`lock_sweep.rs:609`, `:625`).
**Therefore no mechanism that seeds itself from a training sequence can move a
single figure in `before.md`.** That rules out, as a `before.md` target: seeding
V.32's carrier from `ReversalDetector::drift`, seeding either equaliser by least
squares on a known sequence, and V.22 bis's open-loop offset estimate on 6.3.1.1.1's
unscrambled binary 1. Two of those are in this plan anyway, because they are what
a real call gets — but they are proved by `v32_call`/`v32_startup` with an offset
applied, and that is written into the package rather than discovered by an agent
who spends a day making the sweep show it.

---

## What "locked like V.34" means

`v34-reference.md` §5, turned round, with `before.md`'s evidence against each
cell. The four "none"s in the last column and the four symbol-spaced blind
equalisers are the whole of the answer; the rest is tuning.

| | timing | carrier | equalisation | level | loss detection |
|---|---|---|---|---|---|
| **Bell 103** | none needed — `AsyncFramer` re-acquires on every start bit (`framing.rs:63-95`) | none — amplitude-blind discriminator | none | none needed | flag drops after 11–16 ms of quiet; **dropout 20 ms → BER 0.0155, 2 slips** |
| **V.21** | first order, no integrator (`v21.rs:156`); standing error `8·sps·ε/d` | none | none | none needed | same flag; **dropout 20 ms → slicer SNR 25.5 → 14.5 dB** |
| **V.22 / V.22 bis** | `Gardner` fed **raw** (`v22bis.rs:667`), normaliser starts at 1.0 | no open-loop seed; ζ = 2.59, ωn 0.93 Hz; **41.5 s to pull in 7 Hz** | T-spaced blind CMA; modulus 1.32 kept at 1200 where the truth is 1.0 | `OnePole::starting_at(10.0)`, τ = 30 symbols | none; V.22 bis 6.4 retrain absent |
| **V.32 / V.32 bis** | `Gardner` fed **raw** (`v32.rs:985`); two-point linear interpolation, 27.3 dB ceiling | no seed; **14400 cannot pull 1 Hz in 51 s**; tracked on the **unequalised** symbol | T-spaced blind CMA; modulus 1.0 never retargeted above 4800 | `starting_at(10.0)`, τ = 120 symbols; gate counted from **construction** | none; clause 8 renegotiation absent |
| **V.29** | `Gardner` pre-scaled by 1/level ✓ | open-loop, one shot, no retry; 4800 A/B tie | T-spaced blind CMA; modulus computed per rate ✓ | 32-symbol mean then exponential ✓ | detector latches on noise; acquisition never re-arms |
| **V.27 ter** | `Gardner` pre-scaled ✓ | open-loop, one shot, no retry | T-spaced blind CMA; modulus 1.0 correct ✓ | started at 1.0, never reset — harmless on 8-PSK | same latch; **dropout 20 ms → never locks** |
| **V.34** | data-aided, 2nd order, exactly critically damped, 200 symbols | 2nd order, ζ ≈ 1, BL 6.8 Hz, seeded from the training solve | **T/2, 31 taps, NLMS, seeded by least squares, adapts in its own frame** | none — the solve sets the scale | windowed error vs `settled`, rewind, two resynchronisers |

Verified in the tree while planning: **V.34 imports neither `dsp::Equalizer` nor
`dsp::Gardner`** (`grep -rn Equalizer crates/datapump/src/v34/` is empty). The
shared-crate blast radius of `equalizer.rs` and `shaping.rs` is exactly V.22 bis,
V.27 ter, V.29 and V.32. The one shared file a V.34 path does use is `tone.rs`
(`v34/phase2.rs`, `v34/training.rs`), and exactly one package below touches it.

---

## Where a mode genuinely needs something different

An FSK discriminator is not a QAM carrier loop. Four places are that kind of
mismatch, and they are stated here so no package reaches for the shared part by
reflex.

**Bell 103 and V.21 take none of the QAM mechanisms.** `arg(z·conj(z_prev))` is
amplitude-blind (`fsk.rs:96-99`), so there is no gain to control; there is no
constellation, so there is no `min_distance²` and no `Lock`; there is no carrier
phase to track, because the information is in the instantaneous frequency. What
they need instead is a **running-mean offset removal** on the discriminator
output (the FSK analogue of a carrier loop, one pole, not a second-order loop),
a **minimum-duration guard** on the carrier flag, and — for V.21 alone — an
**integrator on the bit clock**. That is `fsk-hold`, and it borrows nothing.

**V.29 and V.27 ter cannot rewind.** They are half duplex with one training
sequence at the front of a burst and no way back to it, so V.34's "put the loops
back to before they learned nonsense" has nothing to put them back *to*. The
right analogue is re-running acquisition on a later part of the same burst, and
on 8-PSK the rotation invariant is the **eighth** power, not V.34's fourth.

**V.27 ter does not need gain precision.** Every point is on the unit circle and
a phase slicer does not care about radius, so the AGC starting at 1.0 and never
being reset costs nothing in bits. Do not spend a package on it.

**V.32 is the only slow mode that shares the band with itself**, so it is the
only one where the echo canceller sits inside the tracking problem, and the only
one whose `before.md` echo row understates the real modem.

---

## Rules every package obeys

1. **Specs and real captures only.** Nothing is read from, ported from, built
   from or compared against any other implementation. Every mechanism here is
   either already in this tree (V.34's receiver) or read from a rendered
   Recommendation page.
2. **Every constant cites a clause or a measurement, in the code, at the
   constant.** A clause number is read from the **rendered PDF page** — PyMuPDF
   to a PNG in the scratch folder, then Read — never from `docs/specs/text`,
   which loses signs and columns. A measurement cites the row of `before.md` or
   the test that produced it. No constant chosen because it made the sweep pass;
   a reviewer rejects any constant whose comment does not show the derivation.
3. **Additive or not at all, in `crates/dsp`.** `Gardner::new` and
   `Equalizer::new` must behave bit-identically after every package here, pinned
   by a test. Everything new is reached through a new constructor, a new setter
   or a new type. `Equalizer` and `Gardner` are each shared by four modes; a
   moved default moves four modes at once and the sweep cannot say which change
   did it.
4. **No existing test is edited.** A package that cannot pass an existing test
   has found something and must say what, not change the test. The gate is
   `cargo test -p datapump --lib` = **294 passed, 3 ignored** (107 v34, 80 v90,
   27 v32, 17 v22bis, 15 v8, 15 v27ter, 14 v29, 8 v17, 7 framing, 6 v21,
   1 bell103) and `cargo test -p dsp --lib` = **75 passed**, both measured on
   `pr-1-deps` at 48b9087 while writing this.
5. **The measuring stick does not move while it is measuring.** `lock_sweep.rs`
   rewrites `before.md` on every run. `baseline-and-margins` copies it to
   `before-48b9087.md` in wave 1 and **that copy is what every figure below is
   compared against**. No package except `proof-close` may touch
   `lock_sweep.rs`, and `proof-close` may only *add columns* — the impairment
   chain, the metric definitions and the seed handling stay exactly as they are,
   and it must reproduce the existing columns row for row on an unchanged tree.
6. **Above 9600 a single row is not evidence.** The clean line only carries the
   payload at some of the eight arrival phases (14400T: 2/8), so a single-phase
   cell cannot tell an impairment the receiver cannot take from the same lottery
   re-rolled. Acceptance above 9600 is the **count out of eight** from
   `one_mode_clean` and `before.md`'s phase table, and an impairment column is
   judged **against the clean column of the same build**, not against 8/8.
7. **A move smaller than the seed spread is not a move.** Every cell is the
   median of three seeds. Treat a factor of three in BER, 1.5 dB in slicer SNR,
   or one whole arrival phase as the smallest difference worth claiming.
8. **A package that does not move its figure has not landed.** Run the sweep,
   compare against `before-48b9087.md`, and if the cell did not move, say so in
   the package's notes before moving on.
9. **`crates/datapump/src/v34/**` and `v90/**` are not touched by any package.**
   V.34's mechanisms are **copied** into `crates/dsp` with the source line cited;
   V.34 goes on using its own. Switching V.34 over is a separate job.
10. **No audio device is opened.** Nothing here needs one.

---

## Three columns nobody should chase

Written down before any agent starts, so none of them spends a day on an
impairment that is arithmetic rather than code.

1. **`echo 0.57 @ 120 ms` on the V.32 rows.** `before.md` says it plainly: there
   is no echo canceller anywhere in the harness, and V.32 is the one mode that
   puts both directions in the same band. A real V.32 modem cancels before the
   receiver sees anything (`v32/startup.rs`, exercised at `v32_loopback.rs:157`).
   These rows measure the bare receiver and understate the modem. **Not a
   target.**
2. **`echo 0.57 @ 120 ms` on the V.29 and V.27 ter rows.** These modes are half
   duplex and silent while receiving, so this is a *listener* echo: a reflection
   4.9 dB down at **144 to 288 symbol periods**. The longest equaliser in this
   tree reaches ±17 ms and a 31-tap T/2 filter reaches 15.5 symbols; the echo is
   at 120 ms. Nothing proposed here spans it. **Not a target.**
3. **FSK `OFF→ON` timing.** V.21 Table 2 asks 300–700 ms and the code answers in
   1.4–3.4 ms. Do **not** "fix" it: T.30 turn-arounds are built on the fast
   answer and `before.md` shows no failure from it. Record the deviation in the
   comment with the clause, and leave it.

And one piece of arithmetic that three of the packages below turn on. **A
dropout costs its own bits whatever the receiver does.** A 20 ms hole at 2400
baud is 48 symbols; at 6 bits a symbol that is 288 bits of a payload of a few
thousand. Several `dropout 20 ms` rows are *already* at that floor — V.32 4800's
0.0082, V.22 bis 2400's 0.0092, V.21's 0.0012 — and a target of "< 1e-3" for
them is arithmetically impossible. What those rows are actually missing is that
**nothing above the receiver is ever told**: `slips` reads 0 and `carrier lost`
reads "no" in every one of them. Where a row is at its floor, the deliverable is
the reported loss and the counted slip, not a smaller number; where it is above
its floor (V.32bis 14400T's 0.0125, V.29's 0.224, V.27 ter's 0.487) the
difference is the walk-off and that is the number to move.

---

## The waves

| wave | V.32 spine | alongside | shared primitive being built | finished at the end |
|---|---|---|---|---|
| 1 | `v32-front-end` | `v22bis-rate-latch`, `fsk-hold`, `baseline-and-margins` | — | **Bell 103**, **V.21** |
| 2 | `v32-equaliser` | `v29-detector` | `dsp-carrier-loop` | — |
| 3 | `v32-carrier` | `v27ter-hold` | `dsp-lock` | **V.27 ter** |
| 4 | `v32-lock` | `v22bis-carrier` | `dsp-fractional` | — |
| 5 | `v32-fractional` | `v22bis-lock`, `v29-lock` | — | **V.32/V.32 bis**, **V.22 bis/V.22**, **V.29** |
| 6 | `proof-close` | | | `after.md` |

File ownership, checked wave by wave — no two packages in a wave name the same
file:

| wave | files |
|---|---|
| 1 | `v32.rs`+`dsp/shaping.rs` \| `v22bis.rs` \| `dsp/fsk.rs`+`framing.rs`+`v21.rs`+`bell103.rs` \| `tests/margins.rs`+`before-48b9087.md` |
| 2 | `dsp/equalizer.rs`+`v32.rs` \| `v29.rs` \| `dsp/carrier.rs`+`dsp/lib.rs` |
| 3 | `v32.rs`+`v32/startup.rs`+`dsp/tone.rs` \| `v27ter.rs` \| `dsp/lock.rs`+`dsp/lib.rs` |
| 4 | `v32.rs` \| `v22bis.rs` \| `dsp/interp.rs`+`dsp/halfspaced.rs`+`dsp/lib.rs` |
| 5 | `v32.rs` \| `v22bis.rs` \| `v29.rs` |
| 6 | `tests/lock_sweep.rs`+`tests/margins.rs`+`after.md` |

`crates/dsp/src/lib.rs` is touched only when a new module is added, so at most
one package per wave adds one. Adding a method to an already-exported type needs
no `lib.rs` edit. `crates/datapump/src/v22bis/handshake.rs` is touched by no
package: the rate latch of `v22bis-rate-latch` lives entirely in `v22bis.rs`.

The order within the spine is the order of the signal path, and it is not
arbitrary: you cannot judge an equaliser through a front end that is sampling in
the wrong place, you cannot judge a carrier loop through an equaliser handing over
on a closed eye, you cannot judge a loss detector until the thing it is detecting
the loss of works, and the structural T/2 change goes **last** so that a failure
there does not block the four wins in front of it. `evidence.md` §5 Q1 asks
whether V.32's arrival-phase fault is the modulus, the hand-over threshold or the
symbol spacing; wave 2 runs the arrival sweep with the two-constant fix adopted
and wave 5 runs it with T/2 adopted, so the question gets answered rather than
assumed.

---

# Wave 1

## `v32-front-end` — stop learning from silence, and from your own gain

**Files:** `crates/datapump/src/v32.rs`, `crates/dsp/src/shaping.rs`.
**Size:** M. **Depends on:** nothing.

**The fault.** Every V.32 rate fails `round trip 1.1 s`, including 4800, which is
otherwise eight phases out of eight on everything else in the sweep. The probe
table pins it: the far end's signal is unchanged and the only difference is how
long the receiver listened to nothing first. At 300 ms silence, V.32 4800 goes
from 8/8 to 0/8. V.22 bis, run beside it as the control, does not move at any
silence from 0 to 1100 ms.

Three lines, all verified in the tree:

- `on_symbol` (`v32.rs:997-1004`) runs the AGC whenever `self.adapting`, and
  `adapting` defaults to `true`. The only caller of `set_adapting` is
  `v32/startup.rs:2235` — so a bare `Receiver`, which is what the sweep and any
  non-`Modem` user drives, never gets the hold at all. Over 1.1 s of digital
  silence the 50 ms pole (120 symbols at 2400 baud) drives `mean_power` to the
  `1e-9` floor and the gain to its `MAX_GAIN = 400` clamp. The carrier flag comes
  up ~20 ms after the signal arrives; the AGC needs 50 ms to come back. In
  between, the equaliser is fed points up to forty times too large.
- The equaliser's guard is `self.symbols > 64` (`v32.rs:1073`), and `symbols`
  counts from **construction**, not from the carrier — 27 ms at 2400 baud,
  shorter than the AGC's own recovery. V.22 bis does not have this fault: its
  `since_carrier` is reset on a carrier transition (`v22bis.rs:642-647`, with the
  reason written above it), and 64 symbols at 600 baud is 107 ms, longer than its
  AGC's 50 ms.
- `Gardner`'s error normaliser starts at `mean_power = 1.0` (`shaping.rs:282`)
  and moves 2 % a symbol. V.27 ter and V.29 work around it by pre-scaling the
  loop's input by 1/level (`v27ter.rs:843-845`, `v29.rs:832-839`); V.22 bis and
  V.32 feed the raw matched-filter output. Thirty decibels down that is 345
  symbols to move — and a receiver started half a symbol out of step needs
  `(sps/2)/1e-4` = 33 000 symbols to cross.

**What to do.**

1. Hold the AGC **and** `Gardner` while `!self.carrier`, the way
   `v27ter.rs:905-911` and `v29.rs:820-835` already do. `Gardner::set_adapting`
   (`shaping.rs:299`) exists and keeps producing symbols while held — use it.
   Keep `set_adapting` as the outer, modem-level hold; the carrier gate is a
   second, independent one, because a bare `Receiver` never gets the outer one.
2. Add `since_carrier`, reset on a carrier edge, and gate the equaliser on it
   instead of on `symbols`. The precedent and the number are `v22bis.rs:642-647`.
3. Add `Gardner::with_power(f64)` — additive, `Gardner::new` unchanged — and seed
   V.32's loop with the power the matched filter actually produces at nominal
   level. **Measure it, do not guess:** mean |matched|² over a full-level 4800
   burst on a clean line, printed once and written into the comment beside the
   constant. `v22bis-carrier` reuses the method in wave 4.

**Figures it must move** (`round trip 1.1 s` row of each V.32 table, and the
silence probe):

| where | now | must be |
|---|---|---|
| V.32 4800 `round trip 1.1 s` | never, BER 0.4898 | lock ≤ 40 ms, BER < 1e-3 |
| V.32 9600 `round trip 1.1 s` | lock 905 ms | lock ≤ 200 ms |
| probe, V.32 4800 at 300 ms silence | 0/8, BER 0.4913 | 8/8, BER 0 |
| probe, V.32 4800 at 400/600/1100 ms | 5/8, BER ≈ 0.490 | 8/8, BER 0 |
| 7200T / 9600T / 12000T / 14400T `round trip 1.1 s` | never, BER ≈ 0.50 | lock, and **claimed jointly with `v32-equaliser`** |

That last row is deliberate. Above 9600 the receiver must also survive the
arrival phase before the round-trip cell can clear, so `v32-front-end` claims
4800 and 9600 outright and the top four rates are accepted at `proof-close`. If
they clear here, record it; if they do not, that is expected and not a miss.

**Tests.** `lock_sweep::two_things_worth_a_closer_look` and
`every_slow_mode_against_every_impairment` (release, `--ignored`); every existing
V.32 test **unedited** — `v32_loopback`, `v32_call`, `v32_both_ends`,
`v32_startup`, `v32_signals`, `v32_rate_framing`, `v32_bits`, `v32_vector`,
`v32_who`; `cargo test -p dsp --lib` (75, unedited, `Gardner::new` bit-identical);
new `shaping::tests::an_unseeded_gardner_behaves_exactly_as_before`; new
`shaping::tests::a_seeded_normaliser_finds_the_instant_thirty_decibels_down` —
the same signal at 0 dB and −30 dB reaching the same sampling instant within one
sample in the same number of symbols; new
`v32::tests::a_silent_line_does_not_wind_up_the_gain_or_the_timing_loop`.

---

## `v22bis-rate-latch` — a rate decision that does not change its mind

**Files:** `crates/datapump/src/v22bis.rs`. **Size:** S. **Depends on:** nothing.

**The fault.** `before.md` runs two rows that put the *same waveform* on the line.
V.22 bis 2.5.2.2 nominates the one point V.22 uses "irrespective of the quadrant
concerned … This ensure compatibility with Recommendation V.22", and this project
has one receiver for both. The only difference between the "V.22 1200" and
"V.22 bis 1200" rows is that the V.22 row leaves the receiver to work the rate
out. That one difference costs it two cells: `clock +120 ppm` reads BER 0.4688
where V.22 bis 1200, told its rate, reads 0 — with ±200 ppm passing on *both*
sides of it, which is the signature of a lottery and not of a clock limit — and
7/8 arrival phases against 8/8.

Verified: `set_rate` (`v22bis.rs:920-925`) sets `self.rate` and resets three
counters, and 128 symbols later the variance test at `v22bis.rs:841-855`
(`variance < 0.16·mean²`) runs again and can overrule it. There is no latch and
no hysteresis. `v22-and-bell.md` W7 records the cost on the tree's own ground
truth: one direction of `tests/vectors/v22bis-2400.wav` read 2400 for a 1200 call
for six seconds — about 3600 symbols of sixteen-way decisions on a four-point
signal, straight into the equaliser — and its residual sat at 0.19 for the
remaining eleven seconds against 0.031 in the other direction.

**What to do.** The reasoning in the comments above the test (`v22bis.rs:773-829`)
— measure the radius not the index, before the equaliser not after, the variance
not the closeness — is sound and hard-won and is not being replaced. Two changes:

1. **Hysteresis, not a threshold.** A rate already decided needs more evidence to
   be abandoned than to be chosen: keep `0.16·mean²` for the first decision and
   require the far side of a band around it, plus N consecutive windows agreeing,
   for a change. The band's width comes from the measured variance of the two
   constellations, which the file already states at `v22bis.rs:806-809` (16-QAM's
   power variance is 32 about a mean of 10; the single ring's is 0); N and the
   band come from a sweep of the two clean waveforms, printed and written into
   the comment.
2. **`set_rate` latches.** When the handshake has negotiated a rate the variance
   test becomes a *disagreement report*, not an override. Keep computing and
   reporting it — it is what a future 6.4 retrain will trigger on — but stop
   acting on it silently. This lives entirely in `v22bis.rs`; `handshake.rs` is
   not touched.

**Figures it must move:**

| where | now | must be |
|---|---|---|
| V.22 1200 `clock +120 ppm` | BER 0.4688, lock never | BER < 1e-3, lock ≤ 500 ms |
| V.22 1200 arrival phases, clean | 7/8 | 8/8 |
| V.22 bis 1200 and 2400, every row | as printed | unchanged or better |

**Tests.** `lock_sweep::one_mode_clean` for the three V.22 rows, and the full
sweep; `cargo test -p datapump --lib v22bis::` (17, unedited); `v22bis_handshake`,
`v22bis_loopback`, `v22bis_vector` unedited — `the_receiver_works_out_which_rate_is_in_use`
and `twelve_hundred_bits_per_second_round_trips` in particular; new
`v22bis::tests::a_negotiated_rate_is_not_overruled_by_the_variance_test`.

**Note.** `v22bis_capture.rs` wants `V22_CAPTURE=captures/live-1788613347.wav`,
and that file is **not in `dist/captures`** (the tree has the 1789-series only) —
the test prints "set V22_CAPTURE to a recording; nothing to do" and passes
vacuously. Do not list it as proof of anything. Recovering that capture is
tracked in Risks.

---

## `fsk-hold` — finish Bell 103 and V.21

**Files:** `crates/dsp/src/fsk.rs`, `crates/datapump/src/framing.rs`,
`crates/datapump/src/v21.rs`, `crates/datapump/src/bell103.rs`.
**Size:** M. **Depends on:** nothing.

Scheduled in wave 1 rather than late, because it depends on nothing, it finishes
two modes outright, and `FskDetector` is used by exactly `bell103.rs` and
`v21.rs` — so it needs no shared carrier-detector abstraction and blocks nobody.

**The fault.** Both FSK modes fail exactly one case and it is the same one:
`dropout 20 ms`. The discriminator is not at fault — `atan2` of a ratio is
amplitude-blind and a steady mark still reads +1.000. What fails is the **carrier
flag**, and what the flag controls is *state*: `AsyncFramer::feed` on `!carrier`
sets `State::Idle` and throws away the character in flight (`framing.rs:52-57`),
and `v21::Receiver::feed` sets `running = false` and re-phases HDLC
(`v21.rs:176-180`). The flag drops after `0.005·ln(L/5.62e-4)` seconds of quiet —
**11.5 ms** from 20 dB above threshold, 16 ms from a −30 dBFS line. **V.21 Table 2,
rendered page 4, requires circuit 109 ON→OFF in 20 to 80 ms.** The 20 ms hole
that costs a character is one the Recommendation says should not have been
noticed at all.

**What to do.**

1. **Meet Table 2.** Lengthen the ON→OFF decision so it lands inside 20–80 ms at
   every level, rather than being a level-dependent 13–36 ms. Do it by *timing*
   the decision, not by moving the threshold — the threshold has a separate job
   (V.21 8.3, −43/−48 dBm) and a separate, uncalibrated fault which this package
   does not take on. Leave OFF→ON alone and record why in the comment.
2. **A drop must not throw away what is in flight.** Even with (1), a longer hole
   drops the flag; the framer should resume on the next start bit rather than
   lose alignment, and V.21's clock should hold its phase across a short gap
   rather than free-run.
3. **Remove the discriminator's offset.** V.21 clause 3 (Fascicle VIII.1, p. 2)
   requires the demodulator to tolerate "drifts of ± 12 Hz between the frequencies
   received and their nominal values". A common-mode shift δ becomes a DC bias of
   δ/100 on the normalised output (`fsk.rs:114`) and the slicer is a hard zero in
   all three places that use it (`fsk.rs:114`, `framing.rs:86`, `v21.rs:208`), so
   at the required ±12 Hz one rail of the eye is 12 % narrower than the other —
   about 1.1 dB given away that a running mean removes exactly, at a time constant
   long against the longest legal run of like bits. **The sign of the half-shift
   stays where it is**: it is what makes V.21's mark-below-space read the same way
   round as Bell 103's mark-above-space, and `fsk.rs:150-173` pins it.
4. **V.21's bit clock gets an integrator.** `PULL = 0.125` with no integral leaves
   `8·sps·ε/d` samples of standing offset — 1707·ε at 16 kHz on a flag stream,
   half a symbol at ε = 1.56 %, measured failing between 1.0 % and 1.5 %. No cell
   in the sweep reaches this (±200 ppm is 0.02 %), so it is proved by the
   measurement already written down, not by `before.md`. **Bell 103 needs nothing
   here**, because `AsyncFramer` dead-reckons from every start edge and tolerates
   5.26 % by geometry.
5. **The framing-error counter is blind in one direction.** A far end that is too
   *slow* trips the stop-bit check and is counted; one that is too *fast* slips
   into the stop bit, which is still mark, so every character is wrong and
   `framing_errors` stays at **zero** — `login:` arrives as `ac af a7 a9 ae ba`.
   Count a mid-character edge that arrives early as well. The one health counter
   at 300 bit/s should not be blind in half the cases.
6. While in the file: delete `slow_env` (`fsk.rs:81`, `:105`), fed every sample
   and never read, left over from the ratio detector `fsk.rs:122` describes
   removing; and correct the four citations of "V.22 bis 6.5.2" (`fsk.rs:45-48`,
   `:285-287`) — **there is no clause 6.5.2**; 6.5 is "Operation after loss of
   line signal" and the hysteresis requirement is 3.3. The same bogus citation
   sits on `CARRIER_ON`/`CARRIER_OFF` in `v32.rs:289` and is fixed by `v32-lock`.

**Figures it must move:**

| where | now | must be |
|---|---|---|
| Bell 103 `dropout 20 ms` | SER 0.0327, BER 0.0155, slips 2 | BER ≤ 0.006, slips 0 |
| V.21 `dropout 20 ms` | BER 0.0012, slicer SNR 14.5 dB | BER ≤ 0.0012, slicer SNR ≥ 22 dB |
| Bell 103 and V.21, every other row | as printed | unchanged |

Both targets are the hole's own cost and no more, which is the point. Six bits of
a ~1500-bit payload is a floor of about 0.004, so Bell 103's 0.0155 is four times
the hole plus two lost characters and the target is the floor plus one character;
a "< 1e-3" target there would be arithmetically impossible. V.21's BER is
*already* at the floor — the number that is wrong is the slicer SNR, 25.5 dB
clean against 14.5 dB after the hole, which is roughly 175 symbols' worth of unit
error for a 6-symbol hole and is the disturbance this package removes.

**Tests.** The sweep; `cargo test -p datapump --lib bell103:: v21:: framing::`
(1 + 6 + 7, unedited); `bell103_loopback`, `bell103_vector` unedited;
`cargo test -p dsp --lib` — `fsk.rs`'s tests including the sign test at
`fsk.rs:150-173`, which is what stops every V.21 bit inverting; `cargo test -p fax`
for the V.21 half of T.30; new
`v21::tests::a_twenty_millisecond_hole_costs_the_hole_and_nothing_more`; new
`fsk::tests::twelve_hertz_of_drift_leaves_both_rails_equal`; new
`v21::tests::a_thousand_ppm_leaves_no_standing_offset`; new
`framing::tests::a_fast_far_end_is_counted_as_well_as_a_slow_one`.

---

## `baseline-and-margins` — freeze the baseline, list the clauses

**Files:** `crates/datapump/tests/margins.rs` *(new)*,
`docs/design/slow-modes/before-48b9087.md` *(new)*.
**Size:** S. **Depends on:** nothing.

Two jobs, neither of which touches a receiver.

**Freeze the baseline.** `lock_sweep.rs` regenerates `before.md` on every run, so
the moment the first package runs the sweep the baseline is gone. Copy it to
`before-48b9087.md`, commit it, and never overwrite it. Every figure in this plan
is against that copy.

**List the clauses as tests.** `margins.rs` is `evidence.md` §4.6's table, one
test per requirement, each naming the clause it reads from the rendered page,
**every one `#[ignore]`d on arrival** with the reason and the current figure in
the ignore comment — so the normal suite is untouched and the list is visible
from day one.

| test | clause | today |
|---|---|---|
| `v22bis_2400_holds_seven_hertz_of_offset` | V.22 bis 2.6 | fails, holds 1.5 Hz |
| `v22bis_1200_holds_seven_hertz_of_offset` | V.22 bis 2.6 | marginal |
| `v32_holds_seven_hertz_at_every_rate` | V.32 2.1 | fails above 9600 |
| `every_v32_rate_acquires_from_every_arrival_phase` | — | fails at 7200 and above |
| `the_carrier_turns_on_between_40_and_205_ms` | V.22 bis 3.2 | fails, 22.8 ms |
| `the_carrier_turns_off_between_40_and_65_ms` | V.22 bis 3.2 | fails, 148 ms |
| `the_answer_tone_does_not_raise_circuit_109` | V.22 bis 3.3 | fails |
| `v21_turns_off_between_20_and_80_ms` | V.21 Table 2 | fails, 13–36 ms |
| `v29_turns_off_in_thirty_milliseconds_plus_or_minus_nine` | V.29 5.2.2 | fails, 17.6 ms |
| `v27ter_turns_off_between_30_and_50_ms` | V.27 ter 3.6 | fails, 17.6 ms |
| `two_clocks_two_hundred_ppm_apart_still_carry_data` | V.22 bis 2.5.1, V.32 2.3 | untested properly |
| `each_rate_carries_data_at_the_signal_to_noise_ratio_it_needs` | — | untested |
| `v27ter_and_v29_carry_a_page_over_an_impaired_line` | — | untested |

**No package before `proof-close` edits this file.** Intermediate packages *run*
their own test with `-- --ignored` and report the figure in their notes; only
`proof-close` un-ignores them, which is what turns the list into a permanent
gate. That keeps the file off every wave's ownership table.

**Tests.** Itself: `cargo test -p datapump` must report the same counts as the
baseline with the new file's tests all ignored.
**Expected:** moves nothing. It is what stops the baseline being overwritten and
what makes every clause failure visible before the work starts.

---

# Wave 2

## `v32-equaliser` — tell the equaliser which constellation it is on

**Files:** `crates/dsp/src/equalizer.rs`, `crates/datapump/src/v32.rs`.
**Size:** M. **Depends on:** `v32-front-end`.

**The fault.** `Equalizer::new(21, 1.0)` is built once in `Receiver::new`
(`v32.rs:909`) and `follow()` (`v32.rs:949-959`) — the one function that knows
the rate changed — never touches it. Two constants are then wrong for five of the
six rates.

*The constant-modulus target.* `equalizer.rs:41` documents `modulus` as
E|a|⁴/E|a|² for the constellation in use. `shared-dsp.md` §3.6 computes the true
values from `v32/trellis.rs`'s own tables: 1.000 at 4800, **1.310** at 9600 coded,
**1.381** at 12000, **1.343** at 14400. The blind stage settles where
`E|y|⁴ = R₂·E|y|²`, so a target of 1.0 against a true 1.31 hands over a
constellation `√(1/1.31)` = **12.6 % small, −1.18 dB** — and at 14400 that puts
the outer ring 0.207 out of place against a half-spacing of 0.110.

*The hand-over threshold.* `equalizer.rs:121` leaves the blind stage when the
running mean decision error falls below a bare `0.25`, on a scale that means
something different at every rate. Against the distance between neighbouring
points at unit mean power: 18 % at 4800, 56 % at 9600 coded, 81 % at 12000,
**113 % at 14400**. At 14400 the blind stage hands over to decision direction
while the mean error is still larger than the whole distance between points, and
the file's own header says what happens then: *"Starting decision-directed on a
closed eye simply reinforces whatever nonsense it first decides."* This is the
best available explanation for the clean-line arrival-phase ladder — 2/8 at
14400, 3/8 at 12000 and 9600T, 6/8 at 9600 uncoded, 8/8 at 4800 — which follows
the table rung for rung.

**What to do.**

1. Three additive methods on `Equalizer`: `set_modulus(f64)`, `set_handover(f64)`,
   `restart_blind()`. `Equalizer::new` keeps its exact present behaviour so V.22
   bis, V.27 ter, V.29 and their tests do not move. `restart_blind` is the piece
   `v22-and-bell.md` open question 3 asks for: today `equalizer.rs:121` is one-way
   and only `reset()` can undo it.
2. `V32::Receiver::follow` sets the modulus from the constellation actually in
   use, **computed at runtime from `trellis.rs:416-440`'s own table** rather than
   written as a literal, and calls `restart_blind()` when the constellation
   changes under it — which at the `Connected` handover is a jump from 45° of
   rotation margin to 5.1° in one symbol, on taps trained at four points
   (`startup.rs:1833-1848`).
3. The hand-over threshold becomes a fraction of **this** constellation's point
   spacing, using `point_spacing_at` (`v32.rs:366-376`), which already exists and
   already contradicts the comment that justifies the present behaviour. Anchor it
   at the one rate where 0.25 is defensible — 4800, where it is 18 % of the
   spacing — so the fraction is 0.25/√2 = 0.177 and every other rate inherits it
   proportionally. Put the table in the comment.

**Figures it must move** (arrival phases from `one_mode_clean`; level rows from
the sweep):

| where | now | must be |
|---|---|---|
| V.32bis 14400T arrival phases | 2/8 | ≥ 6/8 |
| V.32 9600T arrival phases | 3/8 | ≥ 6/8 |
| V.32bis 12000T arrival phases | 3/8 | ≥ 6/8 |
| V.32 9600 arrival phases | 6/8 | 8/8 |
| V.32bis 7200T arrival phases | 7/8 | 8/8 |
| V.32 9600 `level -6 dB` | BER 0.0306 | < 1e-3 |
| V.32 9600T `level -6 dB` / `+6 dB` | BER 0.0266 / 0.0033 | < 1e-3 |
| V.32bis 12000T `level -6 dB` / `+6 dB` | BER 0.0308 / 0.0095 | < 1e-3 |
| V.32bis 14400T `level -6 dB` / `+6 dB` | BER 0.0394 / 0.0944 | < 1e-3 |
| V.32bis 7200T `level -6 dB` | BER 0.0182 | < 1e-3 |
| V.32bis 14400T `clean` slicer SNR | 30.7 dB | ≥ 30.7 dB (must not fall) |

**Tests.** The sweep and `one_mode_clean`; every V.32 test unedited;
`cargo test -p dsp --lib` — `equalizer.rs`'s own tests must pass **unchanged**,
which is the proof that the additions are additive; the other three `Equalizer`
users unedited (`v22bis_loopback`, `cargo test -p fax`, `dil_sounds`); new
`equalizer::tests::the_handover_waits_for_the_eye_at_every_spacing` driving the
five spacings of `shared-dsp.md` §3.6; new
`equalizer::tests::retargeting_moves_the_blind_equilibrium` — the converged
output scale against a known R₂ within 2 % of unity after `set_modulus` and
12.6 % out without it; new `v32::tests::a_rate_change_retargets_the_equaliser`.

---

## `v29-detector` — a carrier detector that cannot latch, and an acquisition that re-arms

**Files:** `crates/datapump/src/v29.rs`. **Size:** M. **Depends on:** nothing.

**The fault.** V.29's SNR floor is **21 dB at 9600 and 24 dB at 7200** against
V.27 ter's 15 and V.32 4800's 12, and the way it fails is a cliff rather than a
slope: at 9600 the `SNR 18 dB` row locks at 1300 ms with slicer SNR 11.7 dB and
15 dB never locks at all. **And the ladder is inverted** — 7200 is worse than
9600 though its minimum distance is 4.9 dB larger, and T.30's fallback goes
9600 → 7200 entitled to assume the step is downhill.

The latch, verified line by line in `v29.rs:770-795`:

- the on-threshold is `(floor·ON_ABOVE_FLOOR).max(CARRIER_ON)` with `floor`
  starting at zero, so a fresh receiver has only the fixed `1.0e-3`, which is
  **53 dB below a burst**;
- white noise of standard deviation σ reads 0.39σ on the same meter, so any line
  whose hiss is above σ = 2.56e-3 fires it;
- the trip calls `new_burst()`, the only thing that resets `burst_symbols`, so
  the one-shot acquisition window is spent on hiss;
- the detector then **cannot fall again**, because the off-threshold is
  `loudest·0.25` and `loudest` is by then the noise itself;
- and `floor` only adapts in the `else` branch, i.e. while the detector says
  there is no carrier — so the estimate it needs in order to notice can never be
  taken.

In the sweep the lead-in is 0.5 s of line before the far end starts, and at 15 dB
the noise on that line reads about 0.08 on this meter, eighty times the
threshold. The cliff in `before.md` is this latch, measured.

**What to do.**

1. **Let the floor keep falling.** The one-way gate is what makes the latch
   permanent. Let the floor continue to fall while the detector is on *and* the
   acquisition has not been confirmed, so a detector that fired on hiss can still
   discover the hiss.
2. **Re-arm the acquisition.** `acquire` runs once, is never checked and is never
   repeated. Give it the acceptance test V.34 already uses in `resync`
   (`v34/receiver.rs:1199-1206`): a reading must be both absolutely good *and*
   better than 0.6 × the median of its own competitors — a self-calibrating
   threshold, not a constant. Segment 2's alternation of two known points
   (`v29.rs:892-935`) is the competitor set, so this is cheap here; re-run it on
   the first 64 credible alternations after any level edge of 12 dB.
3. **Put the carrier-off decision inside 5.2.2's window.** V.29 5.2.2 asks for
   **30 ± 9 ms** and notes the figure "should be suitably chosen … to ensure that
   all valid data bits have appeared on circuit 104"; measured is 17.6 ms. Read
   it again from the rendered page, put the decision inside the window, and a
   20 ms hole stops ending the burst. While there: the receiver stops emitting
   bits the instant the flag drops (`v29.rs:989-991`) while 12.5 to 21.3 ms of
   data is still inside the select filter, the matched filter and the equaliser.
4. **Bisect the 7200/9600 inversion and fix whichever it is.** Two suspects, both
   front-end, both named in `fax-qam.md` §6 Q3: segment 2 alternates |A| = 3 with
   |B| = √2, a **6.5 dB swing every symbol** into a Gardner detector that assumes
   constant modulus (`shaping.rs:321-328` records the thrashing); and
   `MIN_DECISION_POWER = 0.5` (`v29.rs:574`) clamps the inner points' 0.364 at
   7200 but only 0.148 at 9600. Fix the one the bisect names and record the other.
5. While in the file, fix the **4800 A-first/B-first tie**. `acquire` picks
   between the two hypotheses by strict `>` (`v29.rs:910`), so a tie keeps
   `first = 0`. At 4800, A is at four eighths and B at six — a quarter turn — so
   `2(θ_A − θ_B) = 180°` both ways and the wrong hypothesis adds **coherently**,
   giving exactly the same magnitude as the right one; `frequency` is then set to
   about −0.5 turns a symbol, **−1200 Hz**, and the burst is gone. Measured: the
   page was lost at 7 of 20 start offsets. Use the fourth power, which does not
   tie. No row in `before.md` covers V.29 4800 — T.30 never commands it — but it
   is a landmine, and `fax-qam.md` §5.3 says why it matters beyond V.29: at V.17
   |A| = |B| and A and B are a quarter turn apart at *every* rate, so this method
   as it stands can never be copied there.

**Figures it must move:**

| where | now | must be |
|---|---|---|
| V.29 9600 SNR floor | 21 dB | ≤ 15 dB |
| V.29 7200 SNR floor | 24 dB | ≤ 15 dB, **and never worse than 9600's** |
| V.29 9600 `SNR 18 dB` | lock 1300 ms, BER 0.0084 | lock ≤ 350 ms, BER < 1e-3 |
| V.29 7200 `SNR 24 dB` | slicer SNR 15.7 dB, lock 804 ms | ≥ 26 dB, lock ≤ 300 ms |
| V.29 7200 `SNR 21 dB` | BER 0.0012 | < 1e-3 |
| V.29 7200 / 9600 `dropout 20 ms` | carrier lost 12 / 13 ms, BER 0.2241 / 0.2238 | carrier held, BER ≤ 0.015 |

The dropout target is the hole's own cost: 48 symbols at 2400 baud is 192 bits of
a 7680-bit payload, half wrong by chance ≈ 0.0125.

**Tests.** The sweep; `cargo test -p datapump --lib v29:: v17::` (14 + 8,
unedited); `cargo test -p fax` and the whole `faxcall` suite unedited — in
particular `faxcall.rs:690-705`, the one existing noise test, which today passes
while the call silently drops a rate and which must still pass; `v29.rs`'s gain
test at `:1243-1284` and its offset sweep at `:1293`;
`cargo test -p datapump --release --lib -- --ignored v29::tests::the_carrier_is_found_wherever_it_starts_every_way`;
`dil_sounds` unedited; new
`v29::tests::a_quarter_second_of_line_before_the_burst_does_not_cost_the_page`
(32 of 32 lost today); new
`v29::tests::four_thousand_eight_hundred_is_found_from_every_start_offset` — the
existing sweep hard-codes 9600 at `v29.rs:1290-1310`, which is why the tie is not
caught by it; new
`v29::tests::seven_thousand_two_hundred_is_not_worse_than_nine_thousand_six_hundred`.

---

## `dsp-carrier-loop` — one second-order carrier loop, critically damped

**Files:** `crates/dsp/src/carrier.rs` *(new)*, `crates/dsp/src/lib.rs`.
**Size:** M. **Depends on:** nothing. **Touches no mode file.**

There is no shared carrier loop: four copies, four gain pairs, two acquisition
strategies. Every one of them keeps `phase` and `frequency` in turns while the
error arrives in radians, so the effective constants are `Kp = gain·2π` and
`Ki = gain·2π`. For V.22 bis that is `Kp = 0.0503`, `Ki = 9.425e-5`, **ζ = 2.59**
— heavily overdamped, ωn = 0.93 Hz at 600 baud. V.32 scales by `bw`/`bw²` to hold
ζ at 2.59 for every constellation, which narrows ωn to 0.92 Hz at 14400. V.34's
pair is `PHASE_GAIN = 0.04`, `FREQUENCY_GAIN = 4e-4` (`v34/receiver.rs:130-131`)
— so `Ki = (Kp/2)²` exactly, **ζ = 1.0**, ωn = 0.02 rad/symbol, one-sided noise
bandwidth 6.8 Hz at 3429 baud, settling in 58 symbols.

`dsp::CarrierLoop` is that shape with the units stated once:

- error `Im(z·conj(target))/max(|target|², floor)`, which is `sin φ` normalised
  and therefore unit gain, with V.34's proportional floor (0.1 at unit power) and
  not V.32's absolute `max(|c|², 5.0)`, which is half the mean power and
  attenuates signal and noise together — down-weighting the inner ring rather than
  protecting against it, costing 15 % of loop gain at sixteen points;
- **the constructor takes one number**, the loop's natural frequency in hertz, and
  derives `Kp` and `Ki` from it with `Ki = (Kp/2)²`, so ζ = 1 cannot be got wrong
  per mode;
- `rotation += Kp·wrong`, `turn += Ki·wrong`, and **`rotation += turn` every
  symbol before any gate** (`v34/receiver.rs:946`) — that ordering is why a held
  loop does not lose the far end's oscillator;
- `seed_hz(hz, baud)` sets the integrator outright from an open-loop measurement,
  which is how V.27 ter and V.29 already meet ±7 Hz (`v27ter.rs:892-893`,
  `v29.rs:932-933`) and how V.34 starts from `solution.turn`.

Document the closed-loop roots, the noise bandwidth and the settling time the way
`v34-reference.md` §3.3 does. Nothing in `crates/datapump` changes in this
package.

**Tests.** `cargo test -p dsp --lib` (75 unedited, plus the new module's own);
new `carrier::tests::the_loop_is_critically_damped_at_every_baud` — closed-loop
roots real, ζ within 0.1 of 1.0, at 600, 1200, 1600 and 2400 baud, asserted from
the constants and not from a lock; new
`carrier::tests::a_steady_offset_leaves_no_standing_error` — ±7 Hz at each baud,
residual rotation under 1° after settling; new
`carrier::tests::seeding_removes_the_pull_in` — settling within 100 symbols at
7 Hz seeded against thousands unseeded.
**Expected:** no `before.md` cell of its own. It is the enabling half of
`v32-carrier` and `v22bis-carrier`, and its figures are claimed there. Judged
here only on its unit tests.

---

# Wave 3

## `v32-carrier` — ±7 Hz, as 2.1 requires

**Files:** `crates/datapump/src/v32.rs`, `crates/datapump/src/v32/startup.rs`,
`crates/dsp/src/tone.rs`. **Size:** L.
**Depends on:** `v32-front-end`, `v32-equaliser`, `dsp-carrier-loop`.

**The fault.** Every V.32 rate above 4800 fails carrier offsets the
Recommendation requires it to take. **V.32 2.1, rendered from page 1: "The
carrier frequency is to be 1800 ± 1 Hz … The receiver must be able to operate
with received frequency offsets of up to ± 7 Hz."** `before.md`'s phase table is
unambiguous that this is the impairment and not the phase lottery: at 12000 and
14400 every one of the six offset columns is **0/8**.

Computed from the exact difference equations of `v32.rs:1018-1047`, the real
constellations from `v32/trellis.rs` and each rate's own `loop_bandwidth`: 9600
coded takes **85 s** to pull in 7 Hz, 12000 takes **83 s at 3 Hz and never at 7**,
and 14400 takes **51 s at one hertz**. Three things are wrong and all three are
in that block.

1. **ζ = 2.59 everywhere**, with a 9.49-symbol pole inside the loop
   (`TRACK_SMOOTHING = 0.1`, `v32.rs:285`, `:1032`). V.34 has no such filter, and
   a lag inside a second-order loop is what forces the damping up in the first
   place.
2. **9600 uncoded runs at the 4800 gain.** Verified: `loop_bandwidth`
   (`v32.rs:270-277`) only scales for the trellis codings, and
   `coding_for(9600, Uncoded)` returns `None`, so 2.4.1.1's sixteen points get
   `bw = 1.0` — the same as 4800's four. The comment justifying it (`v32.rs:274-275`,
   "both two units apart") is contradicted by `point_spacing_at` twelve lines
   below, which returns `2.0` for 9600 uncoded and `sqrt(20.0)` for 4800 — a
   factor of 2.24 in how far a symbol may move. Measured rotation margin ÷ bw:
   every trellis rate lands between 20.7 and 23.9; **9600 uncoded is the sole
   outlier at 16.9**. And no test reaches it, because `agreed_coding` returns
   `Trellis` above 4800 whenever both ends are V.32 bis, which every two-`Modem`
   test is.
3. **The loop is driven by a decision on the unequalised symbol.**
   `coarse` (`v32.rs:1013-1025`) is a full sixteen-point slice of a signal the
   equaliser has not cleaned; one misdecision — deciding (3,1) as (1,1) — injects
   **−57°**, and a systematic bias from misslicing the outer ring drives the loop
   away, which means more misslices. V.34 tracks on the equalised symbol
   (`v34/receiver.rs:921-922`) and so must this.

**What to do.**

1. Adopt `dsp::CarrierLoop`. Drop `TRACK_SMOOTHING` and the absolute
   `MIN_DECISION_POWER = 5.0`; use V.34's proportional floor.
2. **V.32 chooses its bandwidth from its own rotation margin.** One table derived
   from `point_spacing_at` and the measured margins in `v32.md` §8.2, with 9600
   uncoded in it. State the target ratio (margin ÷ bandwidth ≈ 22, which is what
   every trellis rate already is) and let the six numbers fall out of it rather
   than writing six constants.
3. Track on the **equalised** symbol.
4. **Seed the loop from the reversal detectors** — and be honest about what that
   is worth here. `ReversalDetector` already measures the offset (`drift` at
   `tone.rs:353-360`, an exponential average of the turn per history step) and
   uses it **only to refuse** (`tone.rs:399`); V.32's start-up already runs three
   of them, on the carrier and both sidebands (`v32/startup.rs:994-996`). Expose
   `drift_hz()` and seed `Receiver`'s frequency before data mode. **This moves no
   `before.md` cell, by construction** — the sweep has no reversals and no
   start-up. It is in this package because it belongs with the loop and because it
   is what a real call gets, and it is proved by `v32_call` and `v32_startup` with
   an offset applied. Assert the seed's accuracy against a known offset before
   trusting it: a systematically wrong seed is worse than none.
5. **Raise the reversal detector's own ceiling, for V.32's detectors only.**
   `f_max = π·bw/24 = 7.854 Hz` at bw = 60, so a line at the ±7 Hz the clause
   *requires* sits at 89 % of the gate — and V.32's entire start-up, including the
   round-trip measurement the echo canceller depends on, is conducted through
   these reversals. Add `with_carry(max)` defaulting to today's `MAX_CARRY = π/4`
   so `v34/phase2.rs:148` and `:338` are untouched, and opt in only V.32's three.

**Figures it must move:**

| where | now | must be |
|---|---|---|
| V.32 9600 `carrier -7/-1/+1/+3/+7 Hz` | BER 0.340 / 0.495 / 0.500 / 0.024 / 0.262 | all < 1e-3 |
| V.32bis 7200T, all six carrier rows | BER 0.018 … 0.500 | all < 1e-3 |
| V.32 9600T `carrier -7/-3/+3/+7 Hz` | BER 0.388 / 0.361 / 0.485 / 0.385 | all < 1e-3 |
| V.32bis 12000T, all six carrier rows | BER 0.455 … 0.501 | all < 1e-3 |
| V.32bis 14400T, all six carrier rows | BER 0.431 … 0.500 | all < 1e-3 |
| phase table, 12000T and 14400T `-7 … +7 Hz` | 0/8 in every column | **equal to that build's clean count**, and ≥ 6/8 |
| V.32 9600T `clock -120/-50/+50/+120 ppm` | never, BER ≈ 0.49 | lock, BER < 1e-3 |
| V.32bis 14400T `clock -200/-50/+200 ppm` | BER 0.385 / 0.496 / 0.370 | < 1e-3 |
| V.32 9600 / 9600T SNR floor | 18 dB | ≤ 15 dB |

The clock rows are in this package because a clock offset carries a proportional
carrier offset with it — ±200 ppm on an 1800 Hz carrier is ±0.36 Hz, and ±1 Hz
already fails at 14400T. **If they do not clear here they pass to
`v32-fractional`**, and the measurement that decides which is which is the ppm
figure the data-aided timing detector reports against the residual rotation the
carrier loop reports. Say which one it was.

**Tests.** The sweep and `one_mode_clean`; `cargo test -p dsp --lib`; all ten
`v32_*` integration files unedited; **`v34_vector` and `v34_capture` and
`cargo test -p datapump --lib v34::` (107) unedited**, because this package
touches `tone.rs`; `v32_reversals` and `v32_startup` for the `tone.rs` change;
`v32_call` with an offset applied, which is where item 4 is proved;
`margins.rs::v32_holds_seven_hertz_at_every_rate` run with `--ignored`; new
`tone::tests::the_drift_reads_the_offset_it_was_given` — a reversing tone at −7,
−3, 0, +3, +7 Hz read within 0.5 Hz, which brackets the 7 Hz the crate's own
tests skip; new `tone::tests::a_wider_carry_still_refuses_thirty_hertz`; new
`v32::tests::nine_thousand_six_hundred_uncoded_runs_at_its_own_loop_bandwidth`,
which is the first test in the tree to put a symbol through that receiver.

---

## `v27ter-hold` — finish V.27 ter

**Files:** `crates/datapump/src/v27ter.rs`. **Size:** M. **Depends on:** nothing.

**The fault.** V.27 ter fails exactly two cases at each rate and one of them is
the echo column nobody should chase. The other is `dropout 20 ms`, and it is
catastrophic rather than proportional: BER 0.487 at 2400 and 0.492 at 4800,
"carrier lost, 11 ms", lock **never**. The hole-length table shows the shape:
10 ms costs 0.0048 with the carrier held and 20 ms costs 0.4947 with it lost. A
carrier detector that drops ends the burst, and a half-duplex receiver has one
training sequence in front of it and no way back to it.

**What to do**, in descending order of what it is worth.

1. **The carrier-off decision is out of spec.** V.27 ter 3.6, via Tables 7 and 8,
   asks **30 to 50 ms**; measured is 17.6 ms (10·ln 4 = 13.9 ms for the level to
   fall 12 dB, plus 3.75 ms through the select filter). Read it from the rendered
   page and put the decision inside the window. On its own this should carry the
   whole 20 ms row. While there: the receiver stops emitting bits the instant the
   flag drops (`v27ter.rs:951-953`) with 12.5–21.3 ms of data still in the filters
   and the equaliser.
2. **A hole longer than the clause still ends the burst, and it need not.** The
   50 ms row needs the receiver to re-find the instant and the rotation without a
   training sequence. The mechanism exists in this tree: V.34's `resync`
   (`v34/receiver.rs:1148-1211`) re-reads a window from the raw history at sixteen
   sub-symbol shifts, takes the rotation from a power of the symbol to within a
   fraction of a turn, refines it decision-directed, and accepts only a reading
   both absolutely good and better than 0.6 × the median of its competitors. **On
   8-PSK the invariant is the eighth power, not V.34's fourth** — this is the one
   place in the plan where a V.34 mechanism has to be adapted rather than copied,
   and the comment must say so and say why.
3. **Stop slicing the carrier error over eight phases at 2400.** `nearest_eighth`
   (`v27ter.rs:928`, `:982-985`) is used at both rates and the comment justifying
   it (`:922-927`) is wrong on its own terms: every phase the 2400 end can put on
   the line is one of four — `DIBIT_TURN` is [0, 2, 6, 4] (`:134`), segment 3's
   reversal is `turn(4)` (`:445`), segment 4's is `turn(4)` or `turn(0)` (`:462`),
   the unmodulated carrier is `turn(0)` (`:429`). Slicing over eight halves the
   detector's linear range from ±45° to ±22.5° and doubles the mis-slices, each
   throwing a 45° spike into a loop whose ordinary input is a few degrees. Slice
   four at 2400, and fix the comment as well as the code.
4. Two small ones while the file is open. The **select filter stays at 1400 Hz
   when the rate drops to 2400** (`v27ter.rs:669`; `set_rate` at `:700-710`
   rebuilds the matched filter, the Gardner loop and the AGC but not this), where
   the signal only reaches 900 Hz — 1.9 dB of noise into the level meter and the
   timing loop for nothing. And **`restart()` resets neither `phase` nor
   `frequency`** (`v27ter.rs:755-760`), so a burst too short to reach the
   acquisition window inherits the last one's carrier.

**Figures it must move:**

| where | now | must be |
|---|---|---|
| V.27ter 2400 `dropout 20 ms` | lock never, BER 0.4873, carrier lost 11 ms | carrier held, lock, BER ≤ 0.02 |
| V.27ter 4800 `dropout 20 ms` | lock never, BER 0.4918, carrier lost 11 ms | carrier held, lock, BER ≤ 0.02 |
| hole table, V.27ter 4800 at 50 ms | BER 0.4769, carrier lost | BER ≤ 0.04, one slip reported |
| V.27ter 2400 SNR floor | 15 dB | ≤ 12 dB |
| V.27ter 2400 `SNR 12 dB` | BER 0.0011 | < 1e-3 |
| V.27ter 2400/4800 `clean` slicer SNR | 50.4 / 49.3 dB | must not fall |

The dropout floors are the hole's own cost: 24 symbols of 1200 baud is 48 bits,
32 symbols of 1600 baud is 96 bits, both about 0.0125 of the payload.

**Tests.** The sweep and `two_things_worth_a_closer_look`;
`cargo test -p datapump --lib v27ter:: v17::` (15 + 8, unedited);
`cargo test -p fax` and the whole `faxcall` suite unedited, including the short
turn-on paths; `dil_sounds` unedited; `v27ter.rs`'s offset sweep at `:1235` and
`cargo test -p datapump --release --lib -- --ignored v27ter::tests::the_carrier_is_found_wherever_it_starts_every_way`;
`margins.rs::v27ter_turns_off_between_30_and_50_ms` run with `--ignored`; new
`v27ter::tests::a_twenty_millisecond_hole_does_not_end_the_burst`; new
`v27ter::tests::twenty_four_hundred_slices_the_four_phases_it_is_sent`.

At the end of this wave V.27 ter has one failing cell left and it is arithmetic.

---

## `dsp-lock` — the confidence gate and the loss detector

**Files:** `crates/dsp/src/lock.rs` *(new)*, `crates/dsp/src/lib.rs`.
**Size:** M. **Depends on:** nothing. **Touches no mode file.**

The four "none"s in the loss-detection column are one missing type. Copy V.34's,
constructed from the constellation's minimum distance — the same number
`v32-equaliser` already computes — and holding:

- **the gate**, `doubtful = 0.25 · min_distance²` (`v34/receiver.rs:928`), which
  is half the distance to the decision boundary: above it a decision is more
  likely wrong than right, so nothing learns from it. One gate governs the
  equaliser, the carrier loop and the timing loop, and that single gate is why
  V.34 survives a burst of wrong decisions where the old modes learn from them;
- **`settled`**, an EMA at α = 0.01 of the *passing* symbols' error
  (`v34/receiver.rs:968`), so the threshold is always relative to what this
  particular line actually reads;
- **`recent`**, a window of the last n squared errors, n per constellation size
  (8 for four or sixteen points, 32 for a dense grid, `v34/receiver.rs:210-215`);
- **`lost_threshold = max(k · settled, min_distance²/12)`**, k = 8 for few points
  and 2 for many (`v34/receiver.rs:180-196`). The `/12` floor is deliberate and
  the reason goes into the comment: a sample landing uniformly within ±1 unit of
  some point has mean squared error `min_distance²/6`, so `/12` is 3 dB of margin
  against pure garbage.

Nothing in `crates/datapump` changes here.

**Tests.** `cargo test -p dsp --lib` (75 unedited, plus the new module's);
new `lock::tests::the_floor_is_three_decibels_below_what_uniform_garbage_reads`,
asserted numerically; new
`lock::tests::the_gate_freezes_on_a_burst_of_wrong_decisions`; new
`lock::tests::loss_is_declared_against_what_this_line_reads` — the same absolute
error is a loss on a clean line and not on a noisy one.
**Expected:** no `before.md` cell of its own; the figures are claimed by
`v32-lock`, `v22bis-lock` and `v29-lock`.

---

# Wave 4

## `v32-lock` — a confidence gate, a loss signal, a rewind and a slip count

**Files:** `crates/datapump/src/v32.rs`. **Size:** L.
**Depends on:** `v32-carrier`, `dsp-lock`.

**The fault.** `v32.rs:1031-1042` updates the carrier loop and `:1073-1081` the
equaliser on **every** symbol while connected, however improbable the decision
was. No confidence gate, no loss detector, no way back, and nothing above the
receiver is ever told: `slips` reads 0 and `carrier lost` reads "no" in every
dropout row at every hole length up to 50 ms. On Rory's rig this is not
hypothetical — a VoIP concealment insert is about 20 ms (memory:
`voip-jitter-slips`), which at 2400 baud is 48 symbols of fade and garbage into a
loop whose slow mode at 14400 is 865 ms with a 5.1° margin.

**What to do.**

1. **Adopt `dsp::Lock`** with `min_distance²` from the constellation in force,
   and gate the carrier loop, the equaliser, the timing loop and the AGC on
   `doubtful`. Report the loss signal.
2. **Rewind, in `v32.rs` and not shared.** `shared-dsp.md` §4.5 is right that the
   snapshot is a struct of one receiver's own fields and that a shared version
   would need every loop object `Clone` and every mode assembling its own snapshot
   anyway — and right that V.32 is the one slow mode that runs long enough for it
   to pay. It is the answer to the thing the gate alone cannot fix: by the time
   the error average has risen far enough to declare a loss, every loop has
   already spent the window learning from wrong decisions. Snapshot the taps,
   phase, frequency, the Gardner phase and integral, the AGC value and `settled`
   every 16 symbols, keep 24 — 384 symbols, 160 ms at 2400 baud
   (`v34/receiver.rs:112-113`, `:997-1013`) — and on a declared loss restore the
   newest snapshot at least n + 16 symbols old, carrying the phase forward by
   `frequency · elapsed`. Note V.34's own §7.5: `rewind` returns silently doing
   nothing when no snapshot is old enough, so **count the no-ops from the start**
   rather than discovering them on a live call.
3. **Count and report slips.** `slips()` in V.34 (`v34/receiver.rs:612`) is read
   by the layer above to restart a frame search (`training.rs:1251-1256`). V.32's
   consumer is the V.42 framer; at minimum expose the count so the sweep's `slips`
   column stops reading 0 when a hole has plainly cost symbols.
4. **Unfreeze the residual.** `v32.rs:1073` gates the equaliser — and therefore
   `error_average`, and therefore the whole unsatisfactory-reception retrain — on
   `self.carrier`, so a far end that goes away quietly leaves the pump in
   `Connected` reporting a healthy residual for ever. The gate subsumes this.
5. While in the file, correct the `CARRIER_ON`/`CARRIER_OFF` citation at
   `v32.rs:289`: there is no V.22 bis 6.5.2; 6.5 is "Operation after loss of line
   signal" and the hysteresis requirement is 3.3.

**Figures it must move.** Read the arithmetic note above first: four of these six
rows are already at the hole's own cost, so the deliverable there is the reported
loss and the counted slip, not a smaller BER.

| where | now | must be |
|---|---|---|
| V.32 4800 `dropout 20 ms` | BER 0.0082, `carrier lost` no, slips 0 | loss reported, slips ≥ 1, BER ≤ 0.0082 |
| V.32 9600 / 7200T / 9600T / 12000T `dropout 20 ms` | BER 0.0075 / 0.0070 / 0.0072 / 0.0081, slips 0 | loss reported, slips ≥ 1, BER not worse |
| **V.32bis 14400T `dropout 20 ms`** | **BER 0.0125** | **≤ 0.009** — the one rate above its floor, which is the walk-off |
| hole table, V.32 4800 at 50 ms | BER 0.0161, carrier held, no slip | loss reported, slip counted |
| V.32bis 12000T `clock -50/+50 ppm` | BER 0.4946 / 0.0089 | < 1e-3 |

**Tests.** The sweep, `one_mode_clean` and `two_things_worth_a_closer_look`;
`cargo test -p datapump --lib v32::` (27, unedited); all ten `v32_*` integration
files unedited; `cargo test -p dsp --lib`; new
`v32::tests::a_twenty_millisecond_hole_is_reported_and_rewound`; new
`v32::tests::the_residual_does_not_freeze_when_the_far_end_goes_quiet`; new
`v32::tests::a_rewind_that_finds_no_snapshot_is_counted`.

---

## `v22bis-carrier` — ±7 Hz, as 2.6 requires

**Files:** `crates/datapump/src/v22bis.rs`. **Size:** M.
**Depends on:** `v22bis-rate-latch`, `v32-equaliser`, `dsp-carrier-loop`.

**The fault.** **V.22 bis 2.6, rendered from Fascicle VIII.1 page 4: "The
receiver shall be able to operate with received frequency offsets of up to
± 7 Hz."** The measured cold-acquisition edge is ±2.2 Hz at 2400 and +6 Hz at
1200, and `before.md` agrees: at 2400, ±3 Hz and ±7 Hz all fail and the phase
table reads 0/8 at every offset except ±1 Hz.

The loop is the same second-order shape as V.32's with the same ζ = 2.59 and no
open-loop estimate at all (`v22bis.rs:710-731`). Before the integrator has
absorbed anything the proportional term alone leaves **11.93° per hertz** of
standing error against a 16-QAM angular margin of +20.8°/−18.4°, so 1.5 Hz eats
the whole margin on a clean line. Pull-in times from the exact equations: 2.5 s
at 1 Hz, 8.9 s at 3 Hz, **41.5 s at 7 Hz**.

**What to do.**

1. **Adopt `dsp::CarrierLoop`** at ζ = 1, with the bandwidth chosen from the
   16-point constellation's own rotation margin the same way V.32's rates are —
   one rule, two modes.
2. **Rebuild the equaliser's modulus when the rate changes.** `v22bis.rs:599`
   passes 1.32 once. At 1200 the constellation is four constant-modulus points
   (`v22bis.rs:1018-1036`, confirmed at `:1093-1100`) whose true R₂ is **1.0**;
   against 1.32 the blind stage converges 15 % large, which decision direction
   then pulls back at 4e-3 — about 125 symbols, 208 ms of wrong decisions, on
   every fallback. `Equalizer::set_modulus` and `restart_blind` arrive with
   `v32-equaliser`; `v22bis.rs:920-925` and the automatic fallback at `:847-848`
   are where they belong.
3. **Seed `Gardner`'s normaliser** with `Gardner::with_power` from
   `v32-front-end`. `v22bis.rs:667-668` feeds the raw matched-filter output, and
   at 600 baud the 1.0 start costs **575 ms** to come within a factor of two of a
   power 30 dB down — the whole of a V.22 bis handshake.
4. **An open-loop estimate before the loop is allowed to run**, the way V.29 and
   V.27 ter already do it. 6.3.1.1.1 b) unscrambled binary 1 at 1200 bit/s is a
   pure tone — every symbol the same quadrant change — so the turn per symbol *is*
   the offset; the double dibit alternates two points a quarter turn apart, so the
   fourth power removes the modulation there. Both arrive before any decision is
   needed. Feed `CarrierLoop::seed_hz`, and re-run the seed on a **carrier edge**,
   not once per receiver. **This moves no `before.md` cell by construction** — the
   sweep's V.22 bis transmitter pushes random data from the first symbol and never
   sends either pattern — so the sweep's carrier columns must be carried by items
   1–3 alone. Item 4 is proved by `v22bis_handshake` with an offset applied.

**Note on what the handshake hides.** Run these offsets through the *full*
two-modem handshake and every one connects, because everything before
`Rising2400` runs at 1200 where the decision regions are 90° wide and the
converged integrator is handed to the 2400 stage. **A green `v22bis_handshake` is
not proof of this package.** The ±2.2 Hz limit is a *cold* acquisition limit and
the sweep is what acquires cold at 2400.

**Figures it must move:**

| where | now | must be |
|---|---|---|
| V.22bis 2400 `carrier -7/-3/+3/+7 Hz` | BER 0.354 / 0.313 / 0.326 / 0.359 | all < 1e-3 |
| phase table, V.22bis 2400 `-7 … +7 Hz` | 0/8, 0/8, 3/8, 5/8, 0/8, 0/8 | ≥ 6/8 in every column |
| V.22 1200 `carrier ±7 Hz` | BER 0.1093 / 0.1084 | < 1e-3 |
| V.22bis 1200 `carrier ±7 Hz` | BER 0.0609 / 0.0651 | < 1e-3 |
| V.22bis 2400 SNR floor | 12 dB | ≤ 9 dB |
| V.22bis 2400 `clean` slicer SNR / lock | 28.6 dB / 175 ms | ≥ 28.6 dB / ≤ 175 ms |
| V.22 1200, V.22bis 1200 `level ±6 dB` slicer SNR | 18.6 dB | ≥ 34 dB (clean is 40.8) |
| V.22bis 2400 `level ±6 dB` slicer SNR | 18.5 / 19.1 dB | ≥ 26 dB (clean is 28.6) |

The level rows are in this package because the seeded `Gardner` and the rebuilt
modulus are what the AGC transient is costing; if they do not move here they pass
to `v22bis-lock`.

**Tests.** The sweep and `one_mode_clean`; `cargo test -p datapump --lib v22bis::`
(17, unedited); `v22bis_handshake`, `v22bis_loopback`, `v22bis_vector` unedited —
`the_signal_may_arrive_at_any_moment`, `the_two_clocks_need_not_agree`,
`a_carrier_that_arrives_after_a_pause_is_still_acquired` and
`the_offer_of_2400_is_read_from_every_starting_phase` in particular;
`margins.rs::v22bis_2400_holds_seven_hertz_of_offset` and
`::v22bis_1200_holds_seven_hertz_of_offset` run with `--ignored`; new
`v22bis::tests::a_cold_receiver_acquires_at_seven_hertz_of_offset`.

---

## `dsp-fractional` — a real interpolator and a T/2 equaliser

**Files:** `crates/dsp/src/interp.rs` *(new)*,
`crates/dsp/src/halfspaced.rs` *(new)*, `crates/dsp/src/lib.rs`.
**Size:** L. **Depends on:** `dsp-lock`. **Touches no mode file.**

The structural one, built here and adopted in wave 5 so that the filter and the
tuning are two separate, separately measurable changes.

*The interpolator.* All four slow modes hit the wanted instant with two-point
linear interpolation, in four identical copies (`v22bis.rs:662-666`,
`v27ter.rs:830-834`, `v29.rs:814-818`, `v32.rs:980-984`). Measured error power
against exact interpolation: −57.0 dB at V.22 bis's 26.667 samples per symbol,
exact at V.27 ter 4800's 10.000, −47.2 dB at V.27 ter 2400's 13.333, and
**−37.9 dB at V.29's and V.32's 6.667** — which for V.32 is a worst-case error of
0.0431 of full scale, an **SNR ceiling of 27.3 dB that no line quality can lift**,
and which against half the distance between neighbouring points is 6 % at 4800,
19 % at 9600 trellis, 28 % at 12000 and **39 % at 14400**.

`dsp::Interpolator`: 64 taps, 256 fractional phases, Kaiser β = 8, cutoff
`0.5·baud·(1+β_rrc) + 300 Hz` capped at `0.45·fs`, rows normalised to unit sum,
owning a bounded history and returning `None` when the filter would reach past
either end — `v34/receiver.rs:455-472` and `:662-678`. The quantisation is 1/256
of a sample; at 6.667 sps that is 1/1707 of a symbol, three orders below anything
the timing loop cares about. `resample.rs` is **not** reusable: it pushes output
at a fixed ratio rather than answering "what is the signal at time t", and its
window is Blackman. New module beside it, and the module comment says so.

*The spacing.* A symbol-spaced equaliser sees the folded channel and cannot
compensate for the sampling phase: at the worst phase the fold puts an exact null
at the band edge, which the filter must invert with unbounded gain. That is why
V.32 above 9600 settles into one of exactly two states decided by nothing but the
arrival phase, with the bad state's residual stable at 0.225–0.245 just under the
0.25 hand-over — the filter converges, to the wrong minimum, and decision
direction holds it there for ever.

`dsp::HalfSpacedEqualizer`: 31 complex taps at T/2 (15.5 symbols of line memory,
`REACH = 15` in `v34/receiver.rs:62`), **NLMS** with the step divided by the row
energy so convergence time is independent of level (`v34/receiver.rs:950`,
μ = 0.02 giving a misadjustment time constant of about 50 adapting symbols), the
error **rotated back into the equaliser's own frame** with `e · spin.conj()`
before the tap update (`:951`) so the taps stay a static channel inverse, and
adaptation gated on `dsp::Lock`'s `doubtful`. Fed twice a symbol, sampled once —
`Option<(f64, f64)>` from `feed`, the shape `Gardner::feed` already has.

And **the data-aided timing detector as a rider on it** (`v34/receiver.rs:914-920`,
`:959-967`): the same taps applied to central differences of the half samples,
`late` normalised by an EMA of |rate|², clamped to ±0.5 half symbols, second
order with `Kp² = 8·Ki` exactly — a double root at z = 0.995 and a 200-symbol
time constant — and `drift` clamped to ±0.001 = ±1000 ppm against the ±200 ppm
two conforming modems can be apart, **reported in ppm**, which no slow mode can
do today. It is strictly better than Gardner where a decision exists: unbiased
for any constellation, where Gardner assumes constant modulus and
`shaping.rs:321-328` records it thrashing on sixteen points.

`dsp::Equalizer` (symbol-spaced) is **not touched**, so V.22 bis, V.27 ter and
V.29 stay on the old path and nothing else in the tree moves.

**Tests.** `cargo test -p dsp --lib` (75 unedited, plus the new modules');
new `interp::tests::the_table_reproduces_a_band_limited_signal` — error power
against an exact pulse sum below −70 dB at every one of 256 phases, for 6.667,
10.0, 13.333 and 26.667 sps, against the linear interpolator's measured −37.9 dB
at 6.667; new `interp::tests::it_refuses_to_reach_past_the_history`; new
`interp::tests::each_phase_row_sums_to_one`; new
`halfspaced::tests::a_half_spaced_filter_is_indifferent_to_the_sampling_phase` —
the same channel at 16 sampling phases, converged residual spread under 1 dB,
against the symbol-spaced filter on the same input where it is not; new
`halfspaced::tests::the_timing_loop_is_exactly_critically_damped`; new
`halfspaced::tests::two_hundred_ppm_is_tracked_to_better_than_thirty_eight_decibels`,
mirroring `v34/receiver.rs:1420-1437`.
**Expected:** no `before.md` cell of its own; claimed by `v32-fractional`.

---

# Wave 5

## `v32-fractional` — T/2 equalisation and a real interpolator, adopted

**Files:** `crates/datapump/src/v32.rs`. **Size:** L.
**Depends on:** `v32-lock`, `dsp-fractional`.

Adopt `dsp::Interpolator` in place of the two-point interpolation at
`v32.rs:980-984`, `dsp::HalfSpacedEqualizer` in place of the 21-tap T-spaced one,
and its data-aided timing detector in place of Gardner. This rewrites the
interpolate-and-tick block: the equaliser is fed at the half-symbol instant as
well as the symbol instant, the output is sampled once a symbol, and NLMS changes
what "step" means — **a package that lands the filter and not the tuning makes
its mode worse.** Run the sweep before and after this one flip with nothing else
changing.

**Figures it must move:**

| where | now | must be |
|---|---|---|
| V.32 4800 `clean` slicer SNR | 36.5 dB | ≥ 42 dB — the 27.3 dB interpolator ceiling gone |
| V.32 9600 `clean` slicer SNR | 31.3 dB | ≥ 36 dB |
| V.32bis 14400T `clean` slicer SNR / margin | 30.7 dB / 0.22 | ≥ 36 dB / ≤ 0.15 |
| V.32bis 14400T arrival phases | 2/8 (≥ 6/8 after wave 2) | 8/8 |
| V.32bis 12000T arrival phases | 3/8 (≥ 6/8 after wave 2) | 8/8 |
| V.32 9600T arrival phases | 3/8 | 8/8 |
| V.32bis 14400T SNR floor | 24 dB (≤ 21 after wave 3) | ≤ 18 dB |
| V.32bis 12000T SNR floor | 21 dB | ≤ 15 dB |
| phase table, 14400T `-200/+200 ppm` | 0/8 | 8/8 |
| 7200T/9600T/12000T/14400T `round trip 1.1 s` | never, BER ≈ 0.50 | lock, BER < 1e-3 |

The arrival-phase column is the headline number of this whole plan and this is
the package that finishes it.

**Tests.** The whole sweep and `one_mode_clean` for all six V.32 rows;
`cargo test -p datapump --lib v32::` (27, unedited); all ten `v32_*` integration
files unedited — `v32_vector` and `v32_replay` specifically, because this
rewrites the `feed` loop; `cargo test -p dsp --lib`;
`margins.rs::every_v32_rate_acquires_from_every_arrival_phase` and
`::two_clocks_two_hundred_ppm_apart_still_carry_data` run with `--ignored`; new
`v32::tests::the_sampling_phase_no_longer_decides_whether_it_converges`.

---

## `v22bis-lock` — finish V.22 bis and V.22

**Files:** `crates/datapump/src/v22bis.rs`. **Size:** M.
**Depends on:** `v22bis-carrier`, `dsp-lock`.

**The fault.** With the rate latched and the carrier loop fixed, V.22 bis has the
dropout rows and `level -6 dB` at 2400 left, and both are the same missing
mechanism: nothing gates the loops on the decision being credible and nothing
notices that the line went away. `v22-and-bell.md` §3.8 measured the consequence
directly — **if the noise on a dropped line is loud enough to hold the carrier
flag on, about 42 dB below the signal that was there, nothing stops the equaliser
adapting on it**: one second of noise 35 dB down takes the residual from 0.049 to
0.248 and it does not come back in ten seconds, because the AGC has already
amplified the noise to mean power 10 by the time the equaliser sees it.
`since_carrier` is only reset on a carrier *edge* (`v22bis.rs:642-647`) and
`SQUELCH = 1e-7` (`:117`) is the only other gate.

**What to do.** Fit `dsp::Lock` to the V.22 bis slicers (`v22bis.rs:1006-1057`) —
four points at 1200, sixteen at 2400 — and gate the carrier loop, the equaliser
and the AGC on `doubtful`. Report the loss signal and count slips. **No rewind**:
at 600 baud V.22 bis does not run long enough between handshakes for the snapshot
machinery to earn its keep, and `shared-dsp.md` §4.5 says so. And stop pushing
descrambled bits regardless of the carrier state (`v22bis.rs:878-882`), which is
what V.22 bis 6.5 asks for.

**Figures it must move.** The dropout rows are at the hole's own cost — 12
symbols at 600 baud — so the deliverable there is the reported loss and the
counted slip:

| where | now | must be |
|---|---|---|
| V.22 1200 / V.22bis 1200 `dropout 20 ms` | BER 0.0120, `carrier lost` no, slips 0 | loss reported, slips ≥ 1, BER ≤ 0.0120 |
| V.22bis 2400 `dropout 20 ms` | BER 0.0092, no loss reported | loss reported, slips ≥ 1, BER ≤ 0.0092 |
| hole table, V.22bis 2400 at 50 ms | BER 0.0177, carrier held, no slip | loss reported, slip counted |
| **V.22bis 2400 `level -6 dB`** | **BER 0.0173** | **< 1e-3** — no hole is involved, so this one is a real failure |
| V.22bis 2400 `level +6 dB` / `-6 dB` slicer SNR | 18.5 / 19.1 dB | ≥ 26 dB if `v22bis-carrier` did not already |

**Tests.** The sweep and `two_things_worth_a_closer_look`;
`cargo test -p datapump --lib v22bis::` (17, unedited); all four V.22 bis test
files unedited; `cargo test -p dsp --lib`;
`margins.rs::the_carrier_holds_across_a_hundred_millisecond_dropout` run with
`--ignored`; new
`v22bis::tests::a_second_of_noise_thirty_five_decibels_down_does_not_poison_the_taps`.

---

## `v29-lock` — finish V.29

**Files:** `crates/datapump/src/v29.rs`. **Size:** S.
**Depends on:** `v29-detector`, `dsp-lock`.

**The fault.** After wave 2, V.29 has the two level rows and the long-hole row
left. V.29's gain control is a plain mean over the first 32 symbols of segment 2
and then a 71.5-symbol pole (`v29.rs:855-871`) — excellent for a burst that
starts at a known level, and 30 ms behind a step that happens mid-page, during
which the equaliser learns the wrong scale. The gate is what stops it.

Fit `dsp::Lock` to V.29's slicer, gate the carrier loop, the equaliser and the
AGC on `doubtful`, report the loss and count slips. Add `step_detected`: a level
change of more than 3 dB against the settled estimate re-arms the first stage,
which is what a ±6 dB step needs and what nothing has.

**Figures it must move:**

| where | now | must be |
|---|---|---|
| V.29 9600 `level +6 dB` | BER 0.0016 | < 1e-3 |
| V.29 9600 `level -6 dB` | BER 0.0143 | < 1e-3 |
| V.29 7200 `level -6 dB` | BER 0.0228 | < 1e-3 |
| hole table, V.29 9600 at 50 ms | BER 0.2176, carrier lost | ≤ 0.035, carrier held, slip reported |
| V.29 7200/9600 `clean` slicer SNR | 30.0 / 28.0 dB | must not fall |

**Tests.** The sweep; `cargo test -p datapump --lib v29:: v17::` (unedited);
`cargo test -p fax`, `faxcall` and `dil_sounds` unedited; `v29.rs`'s gain test at
`:1243-1284`, which must still hold the gain to within 1 % from 0 to −40 dB;
`margins.rs::v27ter_and_v29_carry_a_page_over_an_impaired_line` run with
`--ignored`.

---

# Wave 6

## `proof-close` — `after.md`, and the misses written down

**Files:** `crates/datapump/tests/lock_sweep.rs`,
`crates/datapump/tests/margins.rs`,
`docs/design/slow-modes/after.md` *(new)*.
**Size:** M. **Depends on:** every package above.

Three additive columns in `lock_sweep.rs` and nothing else — the impairment
chain, the metric definitions and the seed handling stay exactly as they are, and
the existing columns must reproduce `before-48b9087.md` row for row on an
unchanged tree:

1. **the receiver's own loss-of-equalisation signal**, so the four modes that
   gained one can be seen to have gained one, rather than inferred from BER;
2. **the `drift_ppm` figure** the data-aided timing detector now reports for V.32
   (`v34/receiver.rs:628`), which no slow mode could give before;
3. **more seeds where a threshold is being decided.** Three seeds set the
   smallest claimable difference; the SNR-floor and arrival-phase columns are
   thresholds and want more.

Then un-ignore what `baseline-and-margins` left ignored and what the packages
have earned, re-run everything, and write `after.md` beside
`before-48b9087.md` in the same shape so the two read column against column —
**recording every target that was not met, with the measurement that says so.** A
target missed and written down is worth more than a target quietly dropped.

**Tests.** The whole workspace: `cargo test`, `cargo test -p datapump --release
--test lock_sweep -- --ignored --nocapture`, `cargo clippy --workspace
--all-targets`. The counts must still read 294 passing + 3 ignored for
`datapump --lib` plus whatever the packages added, 75 + additions for `dsp --lib`,
with **107 v34, 80 v90, 15 v27ter, 14 v29, 8 v17 and no test file edited**.

**Expected:** `after.md` carries every figure named above, and "Where it stands,
worst first" goes from **15, 13, 13, 12, 8, 6, 4, 4, 3, 3, 3, 2, 2, 1, 1**
failures to **0 for every mode except the echo rows** — six cells (four V.32
rates' talker echo without a canceller, V.29 ×2 and V.27 ter ×2 listener echo),
all named in "Three columns nobody should chase", all expected to stay.

---

## Risks

1. **The T/2 change is a `feed`-loop rewrite, not a rename.** The caller has to
   hand the equaliser two samples per symbol instead of one, the output has to be
   sampled at the symbol rate while it is fed at twice it, and NLMS changes what
   "step" means, so V.32's tuning has to be redone. `dsp-fractional` ships the
   filter with its own tests one wave ahead, and `v32-fractional` flips exactly
   one thing with the sweep run either side of it.
2. **The sweep cannot see any training sequence for V.32 or V.22 bis.** Verified
   at `lock_sweep.rs:590-606` and `:570-589`. Three mechanisms in this plan are
   therefore unprovable by `before.md` and are labelled so in their packages: the
   `ReversalDetector` seed, V.22 bis's open-loop estimate, and any least-squares
   seed from TRN. An agent who tries to make one of them show in the sweep will
   either waste a day or, worse, change the harness to make it show — which rule 5
   forbids.
3. **`dsp::Equalizer` and `dsp::Gardner` are each shared by four modes**, two of
   which (V.27 ter, V.29) have hard fax test gates. Rule 3 plus the existing unit
   tests passing unedited is the whole defence, and it is mechanical.
4. **`tone.rs` is shared with V.34 phase 2** (`v34/phase2.rs:148`, `:338`) and
   `v34/training.rs`, and per memory `v34-phase2-tone-deadline` the DIL slip
   relocation already fails 17 of 260 sweep positions. `v32-carrier` therefore
   parameterises `MAX_CARRY` rather than changing the default, opts in only
   V.32's three detectors, and runs the 107 V.34 unit tests plus `v34_vector` and
   `v34_capture`.
5. **Every number in this plan is simulated.** `dist/captures` has 47 wav files,
   all 1789-series; the two the replay harnesses name —
   `live-1788613347.wav` for `v22bis_capture.rs` and `live-1788836496.wav` for
   `v32_replay.rs` — are **not in the tree**, and `v22bis_capture` passes
   vacuously without one. A package can pass the whole sweep and still fail on
   Rory's rig. The live-testing loop is his (memory: `dialupmodem2-test-loop`);
   recovering those two captures is worth doing before `proof-close`.
6. **`v32_call.rs` passing is not evidence.** `evidence.md` §3.1 explains why: the
   start-up converges the equaliser on the four points of TRN first, where modulus
   1.0 is correct and the eye is wide, then keeps the taps across the rate change.
   The call tests pass **because the start-up hides the fault**. Never read a
   green `v32_call` as a moved cell.
7. **A green `v22bis_handshake` is not proof of `v22bis-carrier`**, for the same
   reason one level down: the handshake acquires at 1200 where the decision
   regions are 90° wide and hands a converged integrator to the 2400 stage.
8. **The arrival-phase lottery can make a package look better than it is.** Above
   9600, moving from 3/8 to 5/8 changes several single-phase cells without fixing
   anything structural. Judge on the eight-phase count, and judge an impairment
   column against the clean column of the same build.
9. **9600 uncoded has no two-modem test.** `agreed_coding` returns `Trellis` above
   4800 whenever both ends are V.32 bis, which every two-`Modem` test is, so that
   rate is only reachable from the sweep. `v32-carrier` changes its loop gain by a
   factor of about 2.7 and must watch that sweep row specifically.
10. **The FSK carrier-hold change pulls in two directions.** V.21 Table 2's 20 ms
    floor is what stops a concealment insert costing a frame, and it is also 20 ms
    longer that a genuinely dead line keeps producing bits. `fsk.rs:117-134`
    records that the previous detector was thrown out for reporting CONNECT to a
    far end that had said nothing; do not walk back into that.
11. **Tuning is where this goes wrong.** `v32-carrier`'s per-rate bandwidths and
    `v32-equaliser`'s hand-over fraction are the two places a number could be
    chosen because it made a row pass. Both are specified as *derived* — from
    `point_spacing_at` and the measured rotation margins — and a reviewer rejects
    any constant whose comment does not show the derivation.
12. **`v17.rs` is out of scope and still blocked.** 267 lines of tables with no
    receiver and no importer. `State::label_at` returns `None` above 7200 and the
    9600 labels cannot be read off Figure 4/V.17 — four quarter-turn orbits fit
    the four circled letters about equally well (total distances 8.29, 8.02, 8.04,
    7.78) and all four pass the structural checks. It needs a **capture**, not
    another reading of the page. Do not let a package drift into it.

---

## What we are not doing

- **V.34, V.90 and everything above the pump.** No package touches
  `crates/datapump/src/v34/**` or `v90/**`, or `crates/ec`, `crates/modem`, or
  `crates/fax` above the pump. V.34's mechanisms are copied into `crates/dsp`
  with the source cited and V.34 goes on using its own; **switching V.34 onto the
  shared primitives is the obvious follow-on** and should be judged against V.34's
  own tests and a live call, not against `before.md`.
- **The echo columns.** Six cells, named above, arithmetic rather than code. If
  the V.32 talker-echo rows are ever to mean anything, the canceller goes into a
  **new** test file with its own baseline — not into `lock_sweep.rs`, which would
  change the measuring stick mid-flight. And note memory `voip-line-round-trip`:
  `EchoFinder` is O(round-trip × training) with no cap, 654 M MACs inside the
  0.85 s `search_for` spans, so a VoIP round trip will not run through it at all.
  Nothing here touches `echo.rs`.
- **The interpolator in V.22 bis, V.27 ter and V.29.** Measured linear-interpolation
  error is −57.0 dB at 26.667 sps, exact at 10.000 and −47.2 dB at 13.333 — all far
  below what limits those modes, whose clean slicer SNRs are 28–50 dB. Adopting it
  there is churn on three files with hard test gates for no decibel. V.32 is the
  one mode where 6.667 sps makes it a 27.3 dB ceiling, and V.32 is where it goes.
- **V.32 bis clause 8 rate renegotiation.** `State::Connected`
  (`startup.rs:1863-1867`) needs the opening tone to hold for more than 128
  symbols, so the 64-symbol AA/CC preamble of 8.2 is invisible; what happens
  instead is worse than nothing — `stop_offering` fires after 1.0 s and
  permanently drops the rate the call was running at and everything above it, so a
  far end that renegotiated *upward* costs this modem the rate it had. Real far
  ends do this (memory: `live-v34-far-end`, `live-v34-isp-far-end`). It is
  start-up work, it is the largest V.32 gap left after this plan, and it should be
  the next thing.
- **V.22 bis 6.4's retrain.** Grep for `retrain` in `v22bis.rs` and
  `handshake.rs`: zero hits; once connected, `handshake.rs:283` does nothing for
  ever. `v22bis-lock` delivers the loss-of-equalisation signal 6.4 needs, which is
  the precondition; the retrain itself is handshake work and needs the missing
  capture to test against.
- **The rate-signal agreement check.** `RateDetector::agreement()` exists, is
  documented with the measured evidence, and has no caller. Start-up, cheap,
  probably the second thing after this plan.
- **Calibrating the carrier thresholds.** V.22 bis 3.3, V.21 8.3 and V.17 3.6 all
  specify −43/−48 dBm at the line and nothing in the tree maps full scale to dBm;
  `1.0e-3` sits 53.5 dB below this modem's own transmitter at zero loss, 23.5 dB
  below what the clause asks. That is an engineering choice made against a real
  line's noise floor and it is not the number the Recommendation gives. One
  calibration, five call sites, every mode at once — its own piece of work.
  `fsk-hold` and `v27ter-hold` write the gap down; they do not close it.
- **Transmit-side conformance.** V.22 bis 2.3's fixed compromise equaliser
  ("shall be incorporated in the modem transmitter"), the 1800/550 Hz guard tones
  of 2.1/2.2, and the 2225 Hz answer of 6.3.1.2.2. This is a receiver plan.
- **A V.17 receiver.** See Risk 12. `dsp::Interpolator`, `dsp::HalfSpacedEqualizer`
  and `dsp::Lock` are what it would be built on when a capture exists.
- **V.27 ter's `dsp::Lock` adoption.** After `v27ter-hold` the mode has no failing
  cell but the echo row, so a gate there would move no figure — and rule 8 says a
  package needs one. Worth doing when something asks for it.

---

## How to run any of this

```text
cargo test -p datapump --release --test lock_sweep -- --ignored --nocapture
```

`--release` is not optional: the whole sweep is 45 s in release and tens of
minutes in debug. `one_mode_clean` is the fast loop while iterating (every mode
at every arrival phase on a clean line); `two_things_worth_a_closer_look` is the
two threshold probes. **Before the first run of any package, `before.md` must
already have been copied to `before-48b9087.md`** — the test regenerates it, and
that copy is what every figure in this plan is measured against.
