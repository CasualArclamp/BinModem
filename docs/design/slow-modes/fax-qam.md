# The fax data modes: V.29, V.27 ter, V.17

What is in `crates/datapump/src/v29.rs`, `v27ter.rs` and `v17.rs`, how the
receivers hold on to a burst, with the gains and time constants as numbers, and
where they come apart.

Everything numeric here is either read off a rendered spec page, taken from a
named line of source, or measured by running the code. Measurements were made
with a scratch program linked against `datapump`, `fax` and `modem` through
their public interfaces; nothing in the tree was changed. The recipes are at
the end so the numbers can be re-taken.

Short version: the two working pumps are sound from the symbol clock upwards
and are let down by the one thing in front of everything else — the carrier
detector. Its fixed threshold sits 53 dB below a burst, so on any line with
audible hiss it latches on to the noise before the burst arrives, and because
the one-shot carrier measurement is armed by that same edge and never re-armed,
the burst is then decoded against a carrier measured from noise. End to end,
a fax call over a line with **53 dB of signal to noise** — better than any real
telephone circuit — already loses its top rate. V.17 is not a datapump at all;
it is a page of constants with a genuine blocker in it, and the blocker is
smaller than the file says.

---

## 1. What is there

### V.27 ter — `crates/datapump/src/v27ter.rs`, 1336 lines

Complete, both directions. 4800 bit/s as tribits on eight phases at 1600 baud
and 2400 bit/s as dibits on four phases at 1200 baud, carrier 1800 Hz
(`v27ter.rs:23`), 50 % root raised cosine split between the ends
(`v27ter.rs:28`), span six symbols (`v27ter.rs:31`).

* `Training` (`v27ter.rs:84-116`) carries both turn-on lengths of Table 3/V.27
  ter: segment 3 is 14 or 50 symbol intervals (`v27ter.rs:93-98`), segment 4 is
  58 or 1074 (`v27ter.rs:101-106`), segment 5 is 8 (`v27ter.rs:119`).
* Talker-echo protection, optional and off by default: 192.5 ms of unmodulated
  carrier then 22.5 ms of nothing (`v27ter.rs:229-232`, `v27ter.rs:329-331`),
  the middle of Table 3's 185–200 ms and 20–25 ms.
* Scrambler `1 + x^-6 + x^-7` (`v27ter.rs:157-203`) with the Appendix I training
  seed 0011110 (`v27ter.rs:145`); the guards clause 9 asks for are deliberately
  absent (`v27ter.rs:147-156`).
* Tables 1 and 2 forwards and backwards (`v27ter.rs:125-137`).
* `Transmitter` (`v27ter.rs:261-546`), `Receiver` (`v27ter.rs:624-973`).
* 15 unit tests; the turn-on lengths, Table 4's segment 4 and 5 patterns and the
  scrambler round trip are all checked against the printed tables
  (`v27ter.rs:1036-1151`).

### V.29 — `crates/datapump/src/v29.rs`, 1357 lines

Complete, both directions, all three rates. Carrier 1700 Hz (`v29.rs:27`),
2400 baud at every rate (`v29.rs:30`), root raised cosine at a roll-off of 0.25
chosen by the implementation (`v29.rs:32-39`).

* The constellation as two numbers per point, a phase in eighths and a ring
  (`v29.rs:109-148`), with the radii 3, 5, √2 and 3√2 of Table 2/V.29.
* Table 5's synchronizing signal: 48 + 128 + 384 + 48 = 608 symbol intervals
  (`v29.rs:151-162`), with A, B, C and D at `v29.rs:166-192`.
* The segment 3 conditioning generator, `1 + x^-6 + x^-7` clocked once a symbol
  from 0101010 (`v29.rs:201-233`), checked against the four register conditions
  Appendix I prints and against 8.2's "CDCDCDC" (`v29.rs:1138-1148`).
* The data scrambler is V.32's calling-end one, `1 + x^-18 + x^-23`
  (`v29.rs:243-245`, `v32.rs:385-424`).
* Phase coding borrowed wholesale from V.27 ter (`v29.rs:23`), which is what
  2.2.1 says to do.
* `Transmitter` (`v29.rs:279-526`), `Receiver` (`v29.rs:583-1017`).
* 12 unit tests, including one that proves A and B together have exactly the
  mean power of every rate's constellation (`v29.rs:1114-1128`) — the property
  the gain control leans on.

### V.17 — `crates/datapump/src/v17.rs`, 267 lines

**Not a datapump.** 161 lines of constants and tables, 106 lines of tests, no
`Transmitter`, no `Receiver`, nothing that touches a sample. What is there:
the rates and their constellations borrowed from `v32::trellis`
(`v17.rs:35-57`), the long train's four segment lengths (`v17.rs:64-76`),
Table 4's dibit-to-state map (`v17.rs:82-89`), Table 6's bridge turns
(`v17.rs:97-104`), Table 5's sixteen bridge bits (`v17.rs:111-114`), and the
four signalling states — for 7200 only (`v17.rs:139-148`).

It is unreachable from a call: `modem::faxcall`'s `Carrier::of`
(`crates/modem/src/faxcall.rs:32-42`) has no V.17 arm, so a far end whose DCS
names V.17 is heard as nothing at all and its training check is refused
(`crates/modem/src/faxcall.rs:437-444`). The GUI defaults its V.17 offer to off
(`crates/gui/src/faxwin.rs:330`).

### Where they are used

`crates/modem/src/faxcall.rs` owns one transmitter and one receiver of each and
feeds exactly one per sample (`faxcall.rs:356-455`). It always asks for V.27
ter's long turn-on (`faxcall.rs:385-390`), calls `set_rate` then `restart` when
the line turns round to listen (`faxcall.rs:393-403`), and stops the
transmitter as soon as the queue and the modulator are empty
(`faxcall.rs:476-478`, `faxcall.rs:489-491`). T.30's ladder never asks V.29 for
4800 — `crates/fax/src/t30.rs:254-255` lists V.29 as 9600 and 7200 only.

---

## 2. The mechanisms, with numbers

Both receivers are the same machine with different tables. Sample rate is
16 kHz throughout.

### 2.1 The receive chain and its delay

`sample -> NCO mix -> select FIR -> level meter / carrier detector`
`                  -> matched filter -> Gardner interpolator -> symbol`
`symbol -> gain -> carrier derotation -> equaliser -> slicer -> bits`

| | V.27 ter 4800 | V.27 ter 2400 | V.29 (all rates) |
|---|---|---|---|
| baud | 1600 | 1200 | 2400 |
| samples per symbol | 10 | 13.333 | 6.667 |
| select FIR | 1400 Hz, 121 taps (`v27ter.rs:669`) | the same filter, unchanged by `set_rate` | 1600 Hz, 121 taps (`v29.rs:628`) |
| select group delay | 60 samples = 3.75 ms | 3.75 ms | 3.75 ms |
| matched filter | RRC 0.5, span 6 (`v27ter.rs:670`) | same | RRC 0.25, span 6 (`v29.rs:629`) |
| matched delay | 6 symbols = 3.75 ms | 5.0 ms | 2.5 ms |
| equaliser | 31 taps (`v27ter.rs:679`) | 31 taps | 31 taps (`v29.rs:639`) |
| equaliser delay | 15 symbols = 9.38 ms | 12.5 ms | 6.25 ms |
| **total** | **16.9 ms** | **21.3 ms** | **12.5 ms** |

Measured against that: sending a run of zeros, the last wrong bit comes out
719.3 ms after the carrier is found with the long training (whose data starts
701 ms after the carrier is found) and 241.8 ms with V.29 (data at 228 ms).
That is 18 ms and 14 ms — the table's 16.9 ms and 12.5 ms plus a symbol or two.
The figures do not move by more than 1 ms between a full-level line and one
40 dB down.

### 2.2 The carrier detector — identical in both files

`v27ter.rs:786-821`, `v29.rs:770-805`. One envelope meter and two thresholds:

| | value | in dB | line |
|---|---|---|---|
| level meter | one pole, τ = 10 ms | | `v27ter.rs:680`, `v29.rs:640` |
| on | `level > max(4 × floor, 1.0e-3)` | floor + 12.0 dB, or −60 dBFS | `v27ter.rs:557,561`, `v29.rs:530,536` |
| off | `level < max(0.25 × loudest, 5.62e-4)` | burst − 12.0 dB, or −65 dBFS | `v27ter.rs:558,572`, `v29.rs:531,537` |
| loudest forgotten | 3.1e-5 / sample, τ = 2.016 s | | `v27ter.rs:578`, `v29.rs:538` |
| floor falls | 6.25e-4 / sample, τ = 100 ms | | `v27ter.rs:587`, `v29.rs:539` |
| floor rises | 1.25e-5 / sample, τ = 5.0 s | | `v27ter.rs:588`, `v29.rs:540` |
| floor after a burst | `max(floor, level/2)` | burst − 6 dB | `v27ter.rs:591,820`, `v29.rs:543,804` |

Measured references, all on the same meter:

* a V.29 9600 burst reads **0.4642**; a V.27 ter 4800 burst reads **0.4837**
* white noise of standard deviation σ reads **0.39 σ**
* so the fixed on-threshold of 1.0e-3 is **53.3 dB below a burst**
* the detector calls a carrier on pure noise for σ > 2.56e-3, and then stays on
  100 % of the time (the off-threshold is a quarter of `loudest`, and once
  `loudest` is the noise itself the level never falls that far)

Timing of the edges, measured on a clean line: V.27 ter finds a carrier 6.9 ms
after the first symbol of segment 3, V.29 25.3 ms after the start of the burst —
20 ms of that is Table 5's segment 1, so 5.3 ms, about 13 symbols, into the
alternations. Falling, the level needs 10·ln 4 = 13.9 ms to drop 12 dB, plus
3.75 ms through the select filter: **17.6 ms** from the end of a burst to the
flag dropping.

The floor only moves while the detector says there is no carrier
(`v27ter.rs:792-798`, `v29.rs:776-782`). That is the latch: once it is wrongly
on, the estimate it needs in order to notice can never be taken.

### 2.3 Gain control — the two files answer differently

**V.27 ter** (`v27ter.rs:676`, `906-911`): one pole on the symbol power,
started at 1.0, τ = 30 ms — 2.06 % a symbol at 1600 baud (48 symbols),
2.74 % at 1200 (36 symbols). It is never reset: neither `restart`
(`v27ter.rs:755-760`) nor `new_burst` (`v27ter.rs:767-780`) touches it. Starting
from 1.0 on a first burst 40 dB down it needs ln(10⁴)/0.0206 = **447 symbols,
280 ms**, to reach the right gain — longer than the whole short turn-on
(80 symbols, 50 ms) and 40 % of the long one.

This costs nothing in bits, because every V.27 ter point is on the unit circle
and a phase slicer does not care what the radius is. It does mean that
`residual_error()` (`v27ter.rs:732`), which `faxcall.rs:316` divides by the point
spacing and reports as a quality number, is measured in the wrong units for the
first few hundred symbols of a quiet burst.

**V.29** (`v29.rs:855-871`) cannot do that, because it has two radii. It skips
8 symbols while the filters fill (`v29.rs:549`), then takes a plain running mean
over the next 32 (`v29.rs:553`, `k = 1/n`, so the first sample sets it exactly),
then a one pole at 1/72 a symbol — τ = 71.5 symbols = 29.8 ms (`v29.rs:557`).
The 32 land on segment 2, and A and B together have exactly the mean power of
every rate's constellation, so the average is not an estimate but the answer.
The test at `v29.rs:1243-1284` holds it to within 1 % from 0 to −40 dB.
`MAX_GAIN` is 400 in both (`v27ter.rs:613`, `v29.rs:545`).

### 2.4 Symbol timing — `dsp::Gardner`

`Gardner::new(sps, 0.1)` in both (`v27ter.rs:671`, `v29.rs:630`);
`dsp/src/shaping.rs:239-359`. Proportional gain 0.1 samples per unit of
normalised error, integral gain a hundredth of that at 1.0e-3
(`shaping.rs:276`), the error normalised by a running mean power that moves
2 % a symbol (`shaping.rs:337`) and clamped to ±1 (`shaping.rs:338`), the
integral clamped to ±sps/8 and the interval correction to ±sps/4
(`shaping.rs:348-351`).

Both half-intervals take the same correction, so the sampling instant can slew
at most 2 × 0.1 = 0.2 samples a symbol:

| | half a symbol | symbols to slew it | time |
|---|---|---|---|
| V.29, sps 6.667 | 3.33 samples | 17 | 7.0 ms |
| V.27 ter 4800, sps 10 | 5 samples | 25 | 15.6 ms |
| V.27 ter 2400, sps 13.33 | 6.67 samples | 33 | 27.8 ms |

Both feed the loop a signal scaled by 1/level so the normalisation is right from
the first symbols of a burst, held while there is no carrier
(`v27ter.rs:836-845`, `v29.rs:820-835`). The comments there record what it was
like before: a loop normalised by its own slow power estimate was "a thousand
times too timid to move" 30 dB down and stayed half a symbol out of step for the
whole training.

Note the overlap: the timing loop needs 17 to 33 symbols to find the instant,
and the carrier measurement below starts at symbol 8. Its first dozen symbols
are taken at the wrong instant.

### 2.5 Carrier acquisition — one shot, no check, no retry

Neither receiver steers by decisions until a single measurement over the front
of the burst has set the phase and the frequency outright. Both comments record
why (`v29.rs:873-891`, `v27ter.rs:852-865`): a decision-directed loop could not
find a carrier it was not already close to, and failed 45 of 80 tries (V.29) and
40 of 40 (V.27 ter at 2400, 7 Hz off).

**V.27 ter** (`v27ter.rs:866-895`): square the 48 symbols after the first 8
(`v27ter.rs:594,602`). Everything at the front of a turn-on sequence is
two-phase — the plain carrier of segment 1, the reversals of segment 3, the
0°/180° conditioning of segment 4 — so squaring leaves twice the carrier and
none of the data. The short sequence has 14 + 58 = 72 two-phase symbols, so
8 + 48 fits inside even that.

**V.29** (`v29.rs:892-935`): segment 2 alternates two *known* points, so nothing
is squared; each arriving symbol times the conjugate of the point it must be is
the carrier alone. Window 64 symbols after 8 (`v29.rs:549,565`), inside segment
2's 128. Which of A and B arrived first is unknown, so both hypotheses are tried
and the one whose per-symbol turn sums to the larger magnitude wins
(`v29.rs:897-913`). At 9600 and 7200 the wrong hypothesis makes successive
products alternate between +270° and +90° and cancel. **At 4800 it does not** —
see §3.3.

### 2.6 Carrier tracking

Same shape in both: a one-pole average of the phase error with α = 0.20
(τ = 4.5 symbols), then a proportional term on the phase and an integral term on
the frequency (`v27ter.rs:928-939`, `v29.rs:954-978`). The error is a cross
product against a unit-scaled decision, so it arrives in radians while the loop
works in turns; converting, the loop constants are:

| | V.27 ter | V.29 |
|---|---|---|
| phase gain, turns per turn of error | 0.010 × 2π = **0.0628** | 0.008 × 2π = **0.0503** |
| frequency gain | 2.0e-5 × 2π = **1.257e-4** | 1.5e-5 × 2π = **9.42e-5** |
| natural frequency ωₙ | 1.121e-2 rad/symbol | 9.71e-3 rad/symbol |
| in hertz | 2.86 Hz at 1600 baud, 2.14 Hz at 1200 | 3.71 Hz |
| damping ζ | **2.80** | **2.59** |
| slow pole 2ζ/ωₙ | 500 symbols = **313 ms** at 4800, 417 ms at 2400 | 533 symbols = **222 ms** |
| frequency clamp ±0.02 turns/symbol | ±32 Hz at 4800, ±24 Hz at 2400 | ±48 Hz |

Both loops are heavily overdamped, which is a deliberate-looking choice and has
a consequence worth stating plainly: **the integrator cannot fix a bad frequency
estimate inside a training sequence.** 222 ms is longer than V.29's entire
synchronizing signal (253 ms) and 313 ms is six times V.27 ter's short turn-on
(50 ms). Everything rests on §2.5's single measurement.

What the proportional term leaves standing, for an uncorrected offset Δf:

* V.29: (Δf/2400)/0.0503 = **2.98° per hertz**
* V.27 ter 4800: (Δf/1600)/0.0628 = **3.58° per hertz**
* V.27 ter 2400: **4.77° per hertz**

And what a rotation costs, taken as half the distance to the nearest other point
divided by the radius, minimised over the constellation:

* V.29 9600: worst at (5, 0) — 3.606/2 over 5 = 0.361 rad = **20.7°**, so 6.9 Hz
* V.29 7200: worst at (3, 0) — 2.236/2 over 3 = 0.373 rad = **21.4°**, so 7.2 Hz
* V.27 ter 4800: eight phases, **22.5°**, so 6.3 Hz
* V.27 ter 2400: four phases are sent but eight are sliced (§3.6), so **22.5°**
  where 45° was available, 4.7 Hz instead of 9.4 Hz

So on the proportional term alone each of them tolerates between 4.7 and 7.2 Hz
of residual offset — about exactly what clause 4/V.29 and clause 3/V.27 ter ask
a receiver to accept, which is the reason the integrator is there at all.

Measured pull-in, sweeping a real carrier offset through a Hilbert shifter:
V.29 9600 carries the page at ±50 Hz; V.27 ter 4800 at ±30 Hz and loses it at
50; V.27 ter 2400 at ±20 Hz and loses it at 30. Those break points are the
frequency clamps in the table above, not the loop.

### 2.7 Equalisation — `dsp::Equalizer`

31 symbol-spaced taps, centre spike, reset at the start of every burst
(`v27ter.rs:774`, `v29.rs:744`) — right, because every burst carries a training
sequence built to teach one from nothing, and a tap set walked off by a moment's
noise would otherwise poison the retransmission meant to put it right.

* blind step 2.0e-3, tracking step 4.0e-3 (`dsp/src/equalizer.rs:50-51`)
* the blind stage is Godard's constant modulus; the hand-over to decisions
  happens when the running mean error falls below 0.25
  (`equalizer.rs:121`), and that mean starts at 1.0 and moves 1 % a symbol
  (`equalizer.rs:52,116`)
* so the hand-over **cannot happen sooner than ln(1/0.25)/0.01 = 139 symbols**
  after the first `adapt`, and `adapt` is gated until the acquisition window is
  over (`v27ter.rs:946-948`, `v29.rs:984-986`)
* earliest decision-directed symbol: 56 + 139 = 195 (V.27 ter), 72 + 139 = 211
  (V.29)
* the modulus target: V.27 ter uses 1.0 (`v27ter.rs:605,679`), which is right for
  a constant-envelope constellation; V.29 computes it per rate
  (`v29.rs:674-683`) — 1.418 at 9600, 1.405 at 7200, 1.0 at 4800
* runaway guard at a total tap energy of 1e4 (`equalizer.rs:105,143-146`)

Against V.27 ter's long turn-on (1132 symbols) and V.29's (560 after the
silence) there is room. Against V.27 ter's **short** turn-on, which is 80
symbols end to end, the page begins with an equaliser that is still adapting
blind, and stays blind for its first 115 symbols — 72 ms at 1600 baud. This end
always sends the long one (`faxcall.rs:385-390`), but the receiver has to take
whatever a real far end chooses, and Table 3/V.27 ter makes the short one the
normal choice after the first turn-around.

### 2.8 Decoding

Neither receiver ever decides where the training ended (`v27ter.rs:615-622`,
`v29.rs:576-581`). Bits are descrambled and handed up from the first symbol of
the burst, and `fax::call` finds its own place — a training check by its run of
zeros, a page by its first end-of-line code. That is a good decision for a
half-duplex line and it removes a whole class of "where did segment 5 end"
bugs.

---

## 3. Weaknesses

### 3.1 The detector latches on line noise, and the acquisition never re-arms — HIGH

**What.** The on-threshold is `max(4 × floor, 1.0e-3)` and `floor` starts at
zero (`v27ter.rs:681`, `v29.rs:641`). A fresh receiver therefore has nothing but
the fixed 1.0e-3, which §2.2 measures at 53 dB below a burst. Any line whose
hiss is above that trips the detector; the trip calls `new_burst`
(`v27ter.rs:805-807`, `v29.rs:789-791`), which is the only thing that resets
`symbols`/`burst_symbols`, so the acquisition window of §2.5 is spent on noise.
The detector then cannot fall again — the off-threshold is a quarter of
`loudest`, and `loudest` is by then the noise itself — so `new_burst` never runs
a second time and the real burst, when it arrives, is derotated by a phase and a
frequency measured from hiss.

The floor cannot rescue it, because the floor only adapts while the detector
says there is no carrier (`v27ter.rs:792-798`, `v29.rs:776-782`).

**How you know.** With a quarter of a second of line in front of the burst,
a fresh V.29 receiver fired its carrier before the burst in 32 of 32 runs and
lost the whole page in 32 of 32, at every signal-to-noise ratio from 50 dB down
to 30 dB. Five seconds of warm-up before `restart()` changed nothing, because
the floor is frozen the moment the detector latches. V.27 ter 4800 lost 30 of 32
at 40 dB and 31 of 32 at 30 dB. On pure noise with nothing else on the line the
detector is on 100 % of the time for σ ≥ 3.0e-3 and 0 % for σ ≤ 1.0e-3.

End to end, a whole fax call between two `FaxCall`s over a line with nothing
done to it but added hiss:

| hiss σ | page arrived | rate it settled at |
|---|---|---|
| 0 | yes | 9600 |
| 1.0e-3 | yes | 9600 |
| 2.0e-3 (55.2 dB SNR) | yes | 9600 |
| **2.6e-3 (52.9 dB SNR)** | yes | **7200** |
| 3.0e-3 … 8.0e-2 | yes | 7200 |

The step is at exactly the σ where 0.39 σ crosses 1.0e-3. The call loses one
rung and no more, because by the second high-speed listen `floor` has inherited
half the previous burst's level (`v27ter.rs:820`, `v29.rs:804`) and holds the
detector off for the ~550 ms it takes to decay, which is longer than a T.30
turn-around.

**Symptom a user sees.** A fax that negotiates perfectly over V.21 and then
never manages 9600 — the training check at the top rate is always refused and
the call settles one rung down, on a line that is objectively excellent. On a
noisier line, or when the far end is slow enough that the listen lasts more than
about half a second before its carrier starts, the same thing happens again at
the next rate and the call walks the ladder to the bottom.

**Why the tree's own tests pass.** In a loopback the line is silent between
bursts and the fixed threshold is never reached. The one existing noise test
(`faxcall.rs:690-705`) uses uniform noise of ±0.01 — σ = 5.8e-3, comfortably over
the threshold — and it passes anyway, because the call drops a rate and the test
asserts only that the page arrives, not what it arrived at.

### 3.2 Acquisition is one-shot, unchecked and unrepeatable — HIGH

Once `acquire` has run there is nothing that notices it was wrong and nothing
that runs it again. The equaliser's residual error (`equalizer.rs:82`) is
computed and reported but never consulted. `burst_symbols` only restarts on a
detector edge. The result is that V.29 fails all-or-nothing: at 40 dB
signal-to-noise (with no pre-burst line, so only the noise during segment 1's
own 20 ms of silence can trip the detector early), 4 of 32 bursts lost the page
and **all four lost the whole page** — never a fragment. At 50 dB, 0 of 32.
Above 50 dB nothing fails; below it, nothing degrades gracefully.

This is the same mechanism as §3.1 seen from the other side, and it is why §3.1
is a page and not a few lines: a receiver that re-armed on the first 64 clean
alternations it saw would shrug the false carrier off.

### 3.3 V.29 at 4800: the A-first/B-first tie breaks the frequency estimate — MEDIUM

`acquire` picks between the two hypotheses by strict `>` (`v29.rs:910`), so a
tie keeps `first = 0`. At 9600 and 7200 there is no tie: A is a half turn from
the axis and B is at 7 eighths, so 2(θ_A − θ_B) = ±270° and the wrong
hypothesis's products cancel. At 4800, A is at 4 eighths and B at 6 — a quarter
turn — so 2(θ_A − θ_B) = 180° both ways and the wrong hypothesis's products add
coherently at 180°, giving **exactly** the same magnitude as the right one. When
B in fact arrived first, `per_symbol` comes out as the true offset plus π and
`frequency` is set to about −0.5 turns a symbol, −1200 Hz, with an arbitrary
phase (`v29.rs:928-933`). The clamp at `v29.rs:974` pulls it back to ±0.02 on the
next symbol but the burst is already gone. Which of A and B is "first" depends
only on the parity of the symbol the detector happened to fire on.

**How you know.** Sweeping the receiver's start offset from 0 to 19 samples
(a symbol is 6.667 samples), V.29 at 4800 lost the page at offsets
2, 3, 4, 9, 10, 16 and 17 — seven of twenty, with a period of about one symbol.
9600 and 7200 lost none.

**Symptom.** None today: T.30 never commands V.29 at 4800
(`crates/fax/src/t30.rs:254-255`). It is a landmine for anyone using the pump as
a general V.29 modem, which its own Recommendation provides for, and it is also
a warning for V.17 (§5).

### 3.4 Talker-echo protection raises a phantom burst — MEDIUM

With `set_echo_protection(true)` the measured carrier flag goes on at 6.875 ms,
off at 214.25 ms, on again at 229.375 ms and off at the end. The first 207 ms is
the unmodulated carrier of Table 3's segment 1. V.27 ter 5.2.1 is explicit:
circuit 109 "is prevented from turning ON during reception of unmodulated
carrier when the optional protection against talker echo is used". There is no
such interlock in the receiver, and the detector has no way to tell an
unmodulated carrier from a modulated one.

**Symptom.** `fax::call` is told a page carrier arrived and then went away,
before the real one. `v27ter.rs:325-328` records that a real public fax service
sends this in front of every training check, so this is not a hypothetical.
It works today only because the acquisition happens to survive: the 20 ms gap
drops the detector, which re-arms `new_burst`, so the second edge lands on the
reversals. That is luck, not design — and on a line noisy enough for §3.1 the
gap will not drop the detector at all.

### 3.5 V.29's blind equaliser gives up where V.27 ter does not — MEDIUM

A page through a channel that is the signal plus one delayed copy:

| echo | V.29 9600 | V.27 ter 4800 long | V.27 ter 4800 short |
|---|---|---|---|
| 0.2 at 0.44 ms | whole page | whole page | whole page |
| 0.35 at 0.44 ms | **all eight eighths lost** | whole page | **all lost** |
| 0.5 at 0.44 ms | **all lost** | whole page | **all lost** |
| 0.2 at 0.88 ms | whole page | whole page | whole page |
| 0.35 at 0.88 ms | whole page | three eighths lost | **all lost** |
| 0.5 at 0.88 ms | **all lost** | whole page | **all lost** |
| 0.2 at 1.3 ms | whole page | whole page | whole page |

Two things in that table. V.29 is much more fragile than V.27 ter, which is
expected for sixteen points on two radii but is made worse by the constant
modulus criterion having to open a two-radius eye. And the short turn-on
sequence fails everywhere the long one succeeds: 58 symbols of conditioning is
not enough to converge 31 taps at 2.0e-3 a step, and §2.7's 139-symbol floor on
the hand-over means the page starts blind. Note also the non-monotonicity —
0.5 at 0.88 ms puts a 6 dB notch at 1705 Hz, right on V.29's carrier — so these
are individual channels rather than a curve.

### 3.6 V.27 ter slices the carrier error over eight phases at 2400 — LOW

`nearest_eighth` (`v27ter.rs:928,982-985`) is used at both rates, and the comment
at `v27ter.rs:922-927` justifies it by saying a four-point decision "would read a
reversal as a quarter turn of error". That is not right. At 2400 the transmitter
starts at eighth 0 and every turn it can make is even — `DIBIT_TURN` is
[0, 2, 6, 4] (`v27ter.rs:134`), segment 3's reversal is `turn(4)`
(`v27ter.rs:445`), segment 4's is `turn(4)` or `turn(0)` (`v27ter.rs:462`), the
unmodulated carrier is `turn(0)` (`v27ter.rs:429`). Every phase the 2400 end can
put on the line, training included, is one of the four at 0°, 90°, 180° and
270°, and a reversal lands on one of them. Slicing over eight halves the phase
detector's linear range from ±45° to ±22.5° and doubles the number of symbols
mis-sliced at a given noise level, each one throwing a 45° spike into a loop
whose ordinary input is a few degrees.

**Symptom.** Nothing visible today — 2400 is the most robust configuration in
the tree, carrying 2 whole pages of 8 at a signal-to-noise ratio of 12 dB where
4800 carries none, and 8 of 8 at 14 dB. It is margin thrown away, and the
comment that explains it away is wrong, which is worse than the code.

### 3.7 The carrier flag drops 17.6 ms after a burst — LOW

§2.2 measures it. V.29 5.2.2 asks for 30 ± 9 ms and says in a note that the
figure "should be suitably chosen … to ensure that all valid data bits have
appeared on circuit 104". V.27 ter 3.6 (via Table 7/8) asks 30 to 50 ms. Both
receivers stop emitting bits the instant the flag drops
(`v27ter.rs:951-953`, `v29.rs:989-991`), and at that moment 12.5 to 21.3 ms of
data is still inside the select filter, the matched filter and the equaliser
(§2.1). The turn-off sequences cover most of it — 24 symbols, 10 ms, for V.29
(`v29.rs:270`) and 10 symbols, 6.3–8.3 ms, for V.27 ter (`v27ter.rs:244`) — but
not all of it at 2400, where the pipeline is 21.3 ms and the turn-off is 8.3 ms.

**Symptom.** The last byte or two of a burst is lost. Under error correction
mode that is a retransmitted frame rather than a lost page, which is why it has
not been noticed.

### 3.8 Smaller things

* V.27 ter's `select` filter stays at 1400 Hz when the rate drops to 2400
  (`v27ter.rs:669`; `set_rate` at `v27ter.rs:700-710` rebuilds the matched filter,
  the Gardner loop and the AGC but not this), where the signal only reaches
  900 Hz. 1.9 dB of noise into the level meter and the timing loop for nothing.
* V.27 ter's turn-off has no segment B: `Stage::TurnOff` goes straight to
  `Silent` (`v27ter.rs:480-485`). Table 5/V.27 ter is 5–10 ms of scrambled ONEs
  *then 20 ms of no transmitted energy*, total 25–30 ms. Harmless because the
  layer above goes quiet anyway, but `is_transmitting()` goes false about 20 ms
  early.
* `restart()` does not reset `phase` or `frequency` in either receiver
  (`v27ter.rs:755-760`, `v29.rs:730-735`). A burst too short to reach the
  acquisition window inherits the last one's carrier.
* The scrambler guards of clause 9/V.27 ter are omitted (`v27ter.rs:147-156`).
  The worst real input is error correction mode's synchronisation fill, 200 ms
  of HDLC flags at the high speed (`crates/fax/src/ecm.rs:38-40,143-144`;
  240 flags at 9600, 120 at 4800). Measured: 240 flags through the scrambler come
  out with a period of 8 × 127 = 1016 bits, the page behind them reads back
  intact, and the residual error does not move (0.057 of a gap with no flags,
  0.060 with 240). Worth recording as a spec deviation, not worth fixing on the
  evidence here.

---

## 4. Things that hold up (so nobody re-tests them)

* **Level.** Both pumps leave at the root mean square 0.707 the tree uses
  everywhere (measured 0.7073 for V.27 ter, 0.7147 for V.29). V.29's gain
  control is within 1 % of correct from 0 to −40 dB; V.27 ter is insensitive to
  level by construction.
* **Timing slips.** A VoIP concealment insert or drop in the middle of a page —
  samples duplicated or removed — costs only the symbols inside the slip. At
  ±1, ±5, ±10 and ±20 ms, every eighth of a 1200-byte page came back on both
  pumps. The Gardner loop's ±sps/4 correction and ±sps/8 integral re-find the
  instant and the differential decoder needs no help.
* **A quiet line.** Both pumps carry a page unchanged from 0 to −40 dB; the
  carrier is found within 1 to 3 ms of the same place.
* **Frequency offset.** Clause 4/V.29 and clause 3/V.27 ter ask for ±7 Hz. The
  measured limits are ±50 Hz (V.29 9600), ±30 Hz (V.27 ter 4800) and ±20 Hz
  (V.27 ter 2400), set by the ±0.02 turns/symbol clamp rather than by the loop.
* **The tables.** Every printed table that has been transcribed is right. I
  re-read Table 2 and Table 5 and Figure 4/V.29, Tables 3 and 4/V.27 ter and
  Tables 3, 5, 6 and 7 and Figures 4 and 5/V.17 off rendered pages; the code
  agrees with all of them, including the radii √2 and 3√2 that the extracted
  text turns into "2" and "32".

---

## 5. V.17: what is missing, and how much of the blocker is real

### 5.1 What the Recommendation asks for that is not here at all

| clause | what | in the tree |
|---|---|---|
| 2.1 | carrier 1800 ± 1 Hz; receiver to accept ± 7 Hz | constant only (`v17.rs:25`) |
| 2.2 | 2400 ± 0.01 % symbols a second | constant only (`v17.rs:23`) |
| 2.4 | spectrum 4.5 ± 2.5 dB down at 600 and 3000 Hz | nothing |
| Table 3, resync row | **256 + 38 + 48 = 342 symbol intervals, 142 ms** | nothing; `train` has only `LONG` (`v17.rs:64-76`) |
| 5.1.2 | the scrambler state that makes Table 4's output 00 01 00 01 … 10 01 10 01 | nothing |
| 5.1.3 | differential encoder initialised from the final symbol of the previous segment | nothing |
| 5.1.4 | initialised from the first symbol of segment 3 (long) or the last of segment 2 (resync); convolutional encoder started at zero | nothing |
| Table 7 | turn-off: 32 SI of scrambled ONEs then 48 SI of nothing, 80 SI, 33 ms | nothing |
| 5.3 | talker echo protection: 185–200 ms unmodulated carrier, then 20–25 ms silence | nothing |
| 3.6, 3.7 | circuit 109 thresholds −43 dBm on / −48 dBm off, ≥ 2 dB hysteresis; off 30–50 ms after the level falls, on 40–205 ms after it rises | nothing |

**A warning about Table 3.** The extracted text at
`docs/specs/text/T-REC-V.17-199102-I.txt` gives the resync row as
256 / 2938 / 64 / 48 / 3342 / 1142. The rendered page (PDF page 10) prints
256 / 38 / 64 / 48 / **342** / **142**, and only the printed row is consistent:
342/2400 = 142.5 ms, and 256 + 38 + 48 = 342 with the bridge signal left out —
which 5.1.3 requires, since the bridge is "used only during an initial long
train", and 5.1.4 confirms it by initialising the resync differential encoder
from *segment 2*. So the 64 printed in the resync row's bridge column is a
merged-cell artefact and the sequence is ABAB 256, equaliser 38, scrambled ONEs
48. This is worse than the usual lossy-table problem: the text layer here
carries digits the page does not draw.

### 5.2 The real blocker: A, B, C and D at 9600, 12 000 and 14 400

`State::label_at` returns `None` for everything but 7200 (`v17.rs:139-148`), so
`State::point` is `None` and no training signal can be built at the three rates
above it — including 14 400, which is the rate T.30 picks first when V.17 is
offered (`crates/fax/src/t30.rs:786-791`: `best_shared` over the three
modulations answers `(V17, 14_400)`).

I measured the drawing rather than eyeballing it: PyMuPDF gives the vector
geometry of the figures, so the dots, the circled letters and the axis ticks can
be read as coordinates instead of guessed at.

**Figure 5/V.17, 7200** (grid step 4, the code's coordinates are half these):
the four dots the circles belong to are A (−6, −2), B (2, −6), C (6, 2),
D (−2, 6), matching `v17.rs:193-198` exactly. The circles themselves sit at
(−5.90, −3.51), (2.01, −5.20), (6.00, 2.91) and (−1.91, 6.92) — that is, 1.51
grid units *below* A and 0.80 to 0.92 units *above* B, C and D. There is no
consistent offset to transfer to another figure.

**Figure 4/V.17, 9600** (grid step 2, 32 points on the odd-sum lattice): the
circles are at (−5.99, −1.74), (2.00, −5.73), (5.99, 2.39) and (−2.13, 6.25).
Four quarter-turn orbits fit them about equally well:

| orbit (figure units) | labels Q4 Q3 Y2 Y1 Y0 | total distance from the four circles |
|---|---|---|
| (−6,0) (0,−6) (6,0) (0,6) | 10011 10000 10111 10100 | 8.29 |
| (−4,−2) (2,−4) (4,2) (−2,4) | 10010 10001 10110 10101 | 8.02 |
| (−8,−2) (2,−8) (8,2) (−2,8) | 11100 11011 11000 11111 | 8.04 |
| (−6,−4) (4,−6) (6,4) (−4,6) | 00001 00110 00101 00010 | 7.78 |

Every one of them passes the structural checks as well: the two uncoded bits are
constant across the orbit, Y2 Y1 takes all four values, and Y0 alternates. So
the comment at `v17.rs:134-138` is right that the figure alone does not settle
it.

**One avenue that does not work, so nobody spends a day on it.** The
convolutional code cannot be used as a filter. With the *known* 7200 labels,
ABAB is not a valid path through the V.32bis encoder
(`crates/datapump/src/v32/trellis.rs:480-488`): A carries Y0 = 0 and B carries
Y0 = 1, and following `advance` through A, B, A the third symbol is forced to
Y0 = 0 and the fourth to Y0 = 0 where B needs 1. That is consistent with 5.1.4
starting the encoder at zero only at segment 4 — during segments 1 to 3 the
encoder is not running and Y0 is just part of the point's printed name.

**What would settle it: a real capture.** Segment 1 is 256 symbol intervals —
107 ms — of a two-point alternation in which |A| = |B| and the two are a quarter
turn apart (Table 6: dibit 00 is +90° and the pair is A/B). Any recording of a
real V.17 sender at 9600, 12 000 or 14 400 puts those two points on the screen
directly; the rest of the orbit follows from the quarter turns. `dist/captures`
has no V.17 in it today.

### 5.3 Notes for whoever writes the receiver

* Segment 1 is a **quarter-turn** alternation, not V.27 ter's 180° reversal.
  Squaring does not remove that modulation; the fourth power does.
* And do not copy V.29's known-point method as it stands. §3.3 fails precisely
  because |A| = |B| and A and B are a quarter turn apart — which at V.17 is true
  at *every* rate, not just one. The A-first/B-first hypotheses will always tie.
  The fourth power, or a tie broken by something other than `>` (the equaliser's
  residual after a few dozen symbols, say), is needed from the start.
* 256 symbols at 2400 baud is a much longer acquisition window than V.29's 128,
  so there is room to measure and then check.
* The scrambler is the one V.32's calling end already has (`v17.rs:32`,
  `v32.rs:385-424`). Segment 2's seed is a 23-bit unknown with a 32-bit
  constraint from Table 4 — solvable by linear algebra over GF(2) or by a search
  of 8.4 million states, and worth a test that reproduces Table 4's
  "C D C D C D C D C D C D B D B D" the way `v27ter.rs:1097-1109` reproduces
  Table 4/V.27 ter.

---

## 6. Open questions

1. Is there a real capture with high-speed fax carrier in it, at a known level,
   that the §3.1 threshold can be set against? `dist/captures/*.wav` has V.21
   frames read out of several calls, and `crates/fax/tests/replay.rs:113-190`
   exists to print exactly the level trace needed, but the number that decides
   §3.1 is the *quiet-line* level between bursts on a real circuit, and I have
   not seen one measured.
2. Should the fix to §3.1 be a detector that re-arms (re-run `acquire` whenever
   the level rises 12 dB above whatever it had settled at) or a detector that
   refuses to latch (make the floor adapt while the carrier is on as long as the
   level is not rising)? The first is more work and fixes §3.2 as well.
3. V.29 at 7200 is measurably *worse* in noise than at 9600 once the noise is
   what limits it — 1 whole page of 8 at 22 dB against 9600's 4, and 0 of 8 at
   20 dB against 2 — even though 7200's minimum distance is 4.9 dB larger.
   (Higher up, where §3.2's acquisition failures dominate, it is the other way
   round: 2 bursts of 32 lost at 40 dB against 9600's 4.) I did not find the
   cause. Suspects: segment 2 alternates
   |A| = 3 with |B| = √2, a 6.5 dB swing every symbol into a timing detector that
   assumes a constant modulus, and the `MIN_DECISION_POWER` floor of 0.5
   (`v29.rs:574`) which at 7200 clamps the inner points' 0.364 but at 9600
   clamps 0.148. Worth a bisect, because T.30's ladder goes 9600 → 7200 and is
   entitled to assume the step is downhill.
4. Does the short V.27 ter turn-on ever arrive in practice? T.30 leaves the
   choice to the sender and this end always sends the long one. If real machines
   send the short one after the first turn-around, §3.5's short-training column
   is a live failure rather than a latent one.

---

## 7. How to re-take the measurements

All of it was run from a scratch crate outside the tree with
`datapump`, `fax` and `modem` as path dependencies, using only their public
interfaces — `Transmitter::{start, push_bytes, next_sample, stop, trained}`,
`Receiver::{set_rate, feed, take_bits, carrier, level, residual_error,
point_spacing, restart}`, and `modem::FaxCall::{originate, answer, step,
received, rate}`.

* **Carrier detector against noise.** Feed a fresh `Receiver` white noise of
  standard deviation σ for ten seconds and report the mean `level()` and the
  fraction of samples with `carrier()` true. Gives 0.39 σ and the 2.56e-3 knee.
* **Burst level.** Feed a loopback burst and average `level()` over the data.
  Gives 0.4642 (V.29 9600) and 0.4837 (V.27 ter 4800).
* **Page survival.** Send a 300 to 2000 byte payload, add the channel, and look
  for each eighth of the payload in the returned bit stream. Eighths rather than
  the whole thing, or a slip in the middle looks like total loss.
* **Frequency offset.** A 127-tap Hamming-windowed Hilbert transformer with the
  in-phase path delayed by the same 63 samples, multiplied by e^{jωt}: check it
  with a 1800 Hz tone first — one sample of misalignment leaves the wrong
  sideband 8.6 dB *above* the wanted one, which looks exactly like a broken
  receiver.
* **Spec pages.** `fitz.open(pdf)[N-1].get_pixmap(dpi=150).save(png)`, and for
  the V.17 figures `page.get_drawings()` — the dots are 2 to 5 point circles,
  the letter circles 14 to 21 points, and the grid spacing from the sorted
  unique dot coordinates gives the transform into the figure's own units.
* **The ignored sweeps in the tree.**
  `cargo test -p datapump --release --lib -- --ignored
  v27ter::tests::the_carrier_is_found_wherever_it_starts_every_way
  v29::tests::the_carrier_is_found_wherever_it_starts_every_way` — both pass in
  5.7 s. Note that V.29's sweep only covers 9600 (`v29.rs:1290-1310` hard-codes
  the rate), which is why §3.3 is not caught by it.
