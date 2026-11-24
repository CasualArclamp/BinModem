# V.22bis, V.22, Bell 103 and V.21: what holds, what does not

A reading of the four slowest modulations against ITU-T V.22bis (1988), V.21
(1988) and V.25, with every number measured rather than estimated. Where a
figure is quoted from a Recommendation it was read off the rendered PDF page,
not the extracted text.

The short answer to "the older modes are using old code": **V.22bis has one
loop that is genuinely too weak (the carrier loop pulls in ±2.2 Hz where the
Recommendation demands ±7 Hz) and one whole mechanism that is missing (the
retrain of clause 6.4, which the far end on our own recorded call asks for
nine times and never gets).** The FSK modes are structurally sound; their
weaknesses are a carrier detector that gives up 40 times faster than V.21
Table 2 allows, and a V.21 bit clock with no integrator.

---

## 1. Where everything is

| what | file | lines |
|---|---|---|
| FSK discriminator and carrier detect | `crates/dsp/src/fsk.rs` | 17–139 |
| Bell 103 receiver / transmitter / call setup | `crates/datapump/src/bell103.rs` | 55–119, 135–207, 261–441 |
| Start-stop framing (the Bell 103 bit clock) | `crates/datapump/src/framing.rs` | 11–120 |
| V.21 channel 2 + CNG/CED, synchronous bit clock | `crates/datapump/src/v21.rs` | 26–49, 137–233 |
| V.22bis constants, scrambler | `crates/datapump/src/v22bis.rs` | 20–130, 158–215 |
| V.22bis transmitter | `crates/datapump/src/v22bis.rs` | 251–428 |
| V.22bis receiver | `crates/datapump/src/v22bis.rs` | 484–1003 |
| Slicers | `crates/datapump/src/v22bis.rs` | 1006–1057 |
| V.22bis handshake (clause 6.3) | `crates/datapump/src/v22bis/handshake.rs` | 40–337 |
| Gardner symbol timing | `crates/dsp/src/shaping.rs` | 239–359 |
| Adaptive equaliser | `crates/dsp/src/equalizer.rs` | 20–158 |
| Butterworth/FIR primitives | `crates/dsp/src/filter.rs` | 62–139; `shaping.rs` 14–86 |

Ground truth in the tree: `tests/vectors/v22bis-2400.wav` (a real call that,
despite the name, runs at **1200** — `v22bis_vector.rs:97`), `tests/vectors/bell103-300.wav`,
and `captures/live-1788613347.wav` (a real **2400** call, replayed by the
ignored test in `crates/datapump/tests/v22bis_capture.rs`).

---

## 2. The FSK modes

### 2.1 What the discriminator does

`FskDetector` (`fsk.rs:17–139`) is a single chain, not a pair of tone filters:

1. **Band isolation** — `bandpass(8, centre-half, centre+half, fs)` at
   `fsk.rs:72`, where `half = |deviation| + 0.9·baud` (`fsk.rs:63`). For both
   V.21 and Bell 103 that is ±370 Hz about the band centre: 1380–2120 Hz for
   V.21 channel 2 (centre 1750), 1755–2495 Hz for the Bell answer band
   (centre 2125). Order 8 rather than 4, because on a 2-wire tap our own
   transmit is only 10–15 dB down; order 8 gives ~34 dB of rejection of the
   opposite band against order 4's ~17 dB.
2. **Quadrature downconversion** to band centre with an `Nco` (`fsk.rs:71,
   91–94`), then a 4th-order Butterworth at `1.3·baud` = 390 Hz on each arm
   (`fsk.rs:73–74`).
3. **Phase differencing** — `arg(z·conj(z_prev))·fs/2π` (`fsk.rs:96–99, 113`).
   This is amplitude-blind, which is the chain's single biggest strength: the
   discriminator output does not move when the line level moves.
4. **Post-detection low-pass** — 2nd-order Butterworth at `0.8·baud` = 240 Hz
   (`fsk.rs:75`), then division by the **signed** half-shift (`fsk.rs:114`).
   The sign is what makes V.21 (mark *below* space, 980 vs 1180 and 1650 vs
   1850) read the same way round as Bell 103 (mark above space). Taking the
   magnitude there inverts every V.21 bit; the test at `fsk.rs:150–173` pins it.

Measured on the real chain: a steady mark reads exactly **+1.000** and a steady
space **−1.000**; a 1010 alternation at 300 baud reads **±1.09** at the best
sampling phase, i.e. the 240 Hz post filter *overshoots* rather than closing
the eye. Intersymbol interference is not a problem here.

### 2.2 How carrier presence is decided

A 5 ms one-pole envelope of the complex baseband magnitude (`fsk.rs:80, 104`)
against two absolute thresholds with hysteresis (`fsk.rs:49–50, 106–110`):

```
CARRIER_ON  = 1.0e-3
CARRIER_OFF = 5.62e-4      // 20·log10(1e-3/5.62e-4) = 5.0 dB apart
```

The doc comment at `fsk.rs:117–134` records why the previous fast-to-slow
envelope *ratio* detector was thrown out: two envelopes of any steady signal
are equal, so it answered yes to line noise and reported CONNECT to a far end
that had said nothing.

Measured behaviour of the pair (`Bell103Rx`, answer band, 16 kHz):

| far-end level | OFF→ON | ON→OFF | characters recovered |
|---|---|---|---|
| −3 dBFS | 1.4 ms | 36.1 ms | 28/28 |
| −23 dBFS | 2.1 ms | 24.6 ms | 28/28 |
| −33.5 dBFS | — | — | 28/28 |
| −43 dBFS | 3.4 ms | 13.1 ms | 28/28 |
| −53.5 dBFS | — | — | 28/28 |
| −57 dBFS | — | — | **0/28**, framing errors 0 |

**V.21 Table 2/V.21** (read from the rendered page 4 of
`docs/specs/T-REC-V.21-198811-I.pdf`) requires circuit 109 to go OFF→ON in
**300–700 ms** on the general switched network and ON→OFF in **20–80 ms**.
The code is **1.4–3.4 ms** and **13–36 ms**. The ON→OFF figure is inside the
window only while the signal is more than about 35 dB above `CARRIER_OFF`
(`t = 0.005·ln(L/5.62e-4)`); at 19 dB above it is 13 ms, below the 20 ms floor.

The `slow_env` field (`fsk.rs:81`) is still fed (`fsk.rs:105`) and never read —
dead weight from the ratio detector.

### 2.3 How bit timing is recovered — two different answers

**Bell 103 uses no bit clock at all.** `AsyncFramer` (`framing.rs:11–120`) is a
UART: idle mark, a mark-to-space edge starts a character (`framing.rs:63`),
the edge is confirmed at the half-bit point (`framing.rs:70`), data bit *n* is
sampled dead-reckoned at `1.5 + n` bit times from the edge (`framing.rs:84`),
and the stop bit at `1.5 + data_bits` (`framing.rs:95`). Nothing tracks; every
character re-acquires. One sample per bit, no majority vote, no integrate-and-dump.

Tolerance follows from the geometry: the stop bit is 9.5 bit times from the
edge, so a fractional rate error ε displaces it by 9.5ε bits and the limit is
|ε| < 0.5/9.5 = **5.26 %**. Measured on the real code with an exact fractional
baud generator:

| far end | characters right | framing errors |
|---|---|---|
| ±0 to ±5.0 % | 28/28 | 0 |
| **+5.5 %** | 28/28 | 0 |
| **+6.0 %** | **0/28** | **0** |
| **−5.5 %** | 0/28 | 26 |
| −6.0 %, −7.0 % | 0/28 | 28 |

Note the asymmetry in what is *reported*: a far end that is too **slow** trips
the stop-bit check and `framing_errors` counts it; a far end that is too
**fast** slips into the stop bit, which is still mark, so **every character is
wrong and `framing_errors` stays at zero**. `login:` arrives as
`ac af a7 a9 ae ba`. The counter that `bell103.rs:420–427` calls "the count
that means something at 300 bit/s" is blind in exactly one of the two directions.

**V.21 does have a bit clock**, because HDLC has no start bits (`v21.rs:137–233`).
It is first order:

```rust
const PULL: f64 = 0.125;                                   // v21.rs:156
if transition { countdown += PULL * (sps/2 - countdown); } // v21.rs:195-198
countdown -= 1.0;
if countdown <= 0.0 { countdown += sps; emit(level > 0.0); } // v21.rs:200-208
```

Started half a bit after the first transition (`v21.rs:186–192`), reloaded by
addition so the fractional remainder survives (`v21.rs:204`), and reset to
free on carrier loss (`v21.rs:176–180`).

There is **no integrator**, so a rate error leaves a standing sampling offset.
With transition density *d*, the free drift per transition interval is
`sps·ε/d` and the pull removes `PULL·x`, so

```
x = sps·ε / (PULL·d) = 8·sps·ε/d   samples
```

For an HDLC flag stream (`01111110`, two transitions per eight bits, d = 1/4)
at 16 kHz, sps = 53.33, so **x = 1707·ε samples**. Half a symbol is 26.7
samples, reached at **ε = 1.56 %**. Measured on the real receiver, longest run
of a 448-bit burst (320 flag bits then a 16-octet identification field):

| far end | bits recovered |
|---|---|
| 0.0 %, ±0.5 %, ±1.0 % | 448/448 |
| **+1.5 %** | 324/448 |
| **−1.5 %** | 151/448 |
| ±2.0 % | 62/448 and 63/448 |
| ±3.0 % | 38/448 and 39/448 |

Prediction 1.56 %, measurement between 1.0 % and 1.5 %. The failure shape is
exactly the one the doc comment at `v21.rs:128–135` describes from a real fax
call: the flags come through and the frame after them does not.

### 2.4 A level that moves

Two consequences, and only the second is serious.

The discriminator itself does not care: `atan2` of a ratio is amplitude-blind,
so gain changes do not shift the eye at all. What cares is the carrier flag,
and what the carrier flag controls is *state*:

- `AsyncFramer::feed` on `!carrier` sets `State::Idle` (`framing.rs:52–57`) —
  the character in flight is thrown away, and the test at `framing.rs:164–177`
  pins that as correct behaviour.
- `v21::Receiver::feed` on `!carrier` sets `running = false` (`v21.rs:176–180`)
  — the bit clock restarts on the next transition, which re-phases HDLC and
  costs the frame.

With a 5 ms envelope the flag drops after `0.005·ln(L/5.62e-4)` seconds of
quiet: **11.5 ms** from 20 dB above threshold, **16 ms** from 28.5 dB
(a −30 dBFS line), 37 ms from full scale. Memory records ~20 ms concealment
inserts every few seconds on the VoIP path. **Every one of those costs a
character on Bell 103 and a frame on V.21**, and at 300 baud and 30 characters
per second a gap every few seconds is a few per cent of the screen — which,
when the character lost is the escape of an ANSI sequence, is the garbage the
comment at `bell103.rs:420–427` is about. V.21 Table 2's 20–80 ms floor on
ON→OFF exists precisely to stop this.

The second effect is the chatter band. At a transmit amplitude of 0.002
(envelope 9.8e-4, just under `CARRIER_ON`) the receiver delivered **2 of 14
characters**: the flag chattered and the framer was reset twelve times mid-character.
So between roughly −57 and −53 dBFS the modem "works" and drops most of what
arrives. The thresholds are raw sample-scale constants with no calibration
against the −43/−48 dBm of V.21 8.3 and V.22bis 3.3.

### 2.5 A tone that is a few hertz off

V.21 clause 3 (page 1 of the PDF) is explicit: characteristic frequencies must
be within ±6 Hz at the modulator, the line may drift ±6 Hz, and therefore
"**the demodulation equipment must tolerate drifts of ±12 Hz**".

A common-mode shift δ on both tones becomes a pure DC bias of `δ/|deviation|`
= δ/100 on the normalised discriminator output, because `fsk.rs:114` divides
the measured hertz by the half-shift and nothing removes a mean. Measured:

| shift | mark reads | space reads | bias |
|---|---|---|---|
| 0 Hz | +1.0000 | −1.0000 | 0.0000 |
| +6 Hz | +0.9400 | −1.0600 | −0.060 |
| +12 Hz | +0.8800 | −1.1200 | −0.120 |
| −12 Hz | +1.1200 | −0.8800 | +0.120 |

The slicer is a hard zero in all three places that use it (`framing.rs:86`,
`v21.rs:208`, `bell103.rs:313`). So at the ±12 Hz the Recommendation requires,
**one rail of the eye is 12 % narrower than the other** — about 1.1 dB of noise
margin given away for nothing, since a running mean of the discriminator would
remove it exactly. End to end it still works well past the requirement:
Bell 103 recovers 28/28 characters at ±70 Hz; V.21 recovers 448/448 bits at
±30 Hz and fails at ±60 Hz (bias 0.6, so a mark reads only +0.4).

Two band-edge notes. 2100 Hz sits inside the Bell answer band (1755–2495) and
reads as a strong space; `bell103.rs:228–235` waits out a V.25 answer tone by
requiring 200 ms of *idle mark* rather than mere carrier, which is the right
fix. The same 2100 Hz sits at the very edge of V.21 channel 2's band
(1380–2120) and there is no equivalent guard in `v21::Receiver`.

---

## 3. V.22bis and V.22

### 3.1 Is the encoding right?

Yes, exactly, and it was checked against rendered pages rather than the
extracted text (which loses Table 1's arrows and Figure 2's coordinates).

**Table 1/V.22 bis** (page 3 of the PDF, rendered): first two bits 00 → 90°,
01 → 0°, 11 → 270°, 10 → 180°, with quadrants numbered 1→2→3→4 anticlockwise.
`QUADRANT_CHANGE = [1, 0, 2, 3]` indexed by `Q1<<1|Q2` (`v22bis.rs:91`) is that
table, and `v22bis.rs:1073–1083` pins it.

**Figure 2/V.22 bis** (page 4, rendered): quadrant 1 carries `00` at (1,1),
`01` at (3,1), `10` at (1,3), `11` at (3,3). `QUADRANT_POINTS`
(`v22bis.rs:103`) is that list in that order, and `rotate()` (`v22bis.rs:133–140`)
reproduces the labels in all four quadrants — I checked all sixteen against the
rendered figure.

Clause 2.5.2.2: at 1200 the point `01` is sent "irrespective of the quadrant
concerned"; `V22_POINT = 0b01` (`v22bis.rs:62`, used at `v22bis.rs:369–372`).
Its magnitude √10 is the RMS of the whole set, so both rates carry the same
power — pinned at `v22bis.rs:1093–1110`.

Clause 5.1/5.2: polynomial `1 + x⁻¹⁴ + x⁻¹⁷`, 64 consecutive ones at the
scrambler output inverting the *next input* and resetting the counter; the
descrambler watching the same stream and inverting the *next output*.
`Scrambler` (`v22bis.rs:170–209`) does both, and gets the ordering right —
the counter is reset before the current bit is counted, at both ends
(`v22bis.rs:190–199`).

### 3.2 Is there an equaliser at all?

Two answers, one for each end.

**In the receiver, yes**: `Equalizer::new(21, 1.32)` at `v22bis.rs:599` —
21 **symbol-spaced** complex taps, centre spike, 10 symbols (16.7 ms) of delay
(`equalizer.rs:42–55, 72–74`). Two stages:

- **Blind**, Godard constant-modulus, step `2.0e-3` (`equalizer.rs:49, 125–130`),
  with modulus 1.32 (the fourth-over-second moment of unit-power 16-QAM, pinned
  at `equalizer.rs:196–208`).
- **Decision-directed**, step `4.0e-3` (`equalizer.rs:50, 131–132`). Entered
  when a 100-symbol running mean of the decision error (`equalizer.rs:116`)
  falls below 0.25 (`equalizer.rs:121`), and **never left again** — nothing
  puts `blind` back except `reset()`, which fires only on a NaN or on tap
  energy over 1e4 (`equalizer.rs:105, 110–113, 144–146`).

Adaptation is gated on `since_carrier > 64 && mean_power > SQUELCH`
(`v22bis.rs:753`), i.e. 64 symbols (107 ms) after a carrier *edge*, and a raw
symbol power above 1e-7 (`v22bis.rs:117`).

Symbol-spaced, not fractionally spaced: the filter cannot correct a timing
phase error, so it is only as good as the Gardner loop's sampling instant.

**In the transmitter, no.** V.22bis **2.3** is one sentence: *"Fixed compromise
equalization shall be incorporated in the modem transmitter."* The transmitter
(`v22bis.rs:411–418`) applies the root-raised cosine of 2.4 and nothing else.

### 3.3 How the carrier is tracked

A second-order decision-directed loop on the **unequalised** symbol
(`v22bis.rs:710–731`), deliberately ahead of the equaliser so the equaliser's
ten symbols of delay are not inside the loop:

```rust
let coarse = nearest_point(point, self.rate);                       // :720
let error  = (point.1*coarse.0 - point.0*coarse.1) / (|coarse|² + 1e-9); // :723
self.frequency += -1.5e-5 * error;                                  // :728
self.frequency  = self.frequency.clamp(-0.02, 0.02);                // :729
self.phase     += -0.008 * error + self.frequency;                  // :730
```

`error` is `sin θ` with θ the residual angle in radians (the algebra collapses
to `(c²+s²)sin θ / (c²+s²)`). `phase` and `frequency` are in **turns**, so in
consistent units the gains are

- proportional **Kp = 0.008·2π = 0.05027 rad per rad, per symbol**
- integral **Ki = 1.5e-5·2π = 9.425e-5 rad/symbol per rad**
- frequency clamp ±0.02 turn/symbol = **±12 Hz** at 600 baud

giving natural frequency `ωn = √Ki = 9.71e-3 rad/symbol` = **0.93 Hz**, damping
`ζ = Kp/(2√Ki) = 2.59` (overdamped), and a slow pole at `Ki/Kp = 1.875e-3` per
symbol — **a 533-symbol, 0.89 s time constant on absorbing a frequency offset**.

Before the integrator has absorbed anything, the proportional term alone holds
a frequency offset Δf at a standing phase error of

```
θ = (2π·Δf/600) / 0.05027 = 0.2083·Δf rad = 11.93° per hertz
```

The 16-QAM angular margin is about **+20.8° / −18.4°** (the point (3,1) at
radius √10 leaves its decision square when √10·sin(18.435°+δ) exceeds 2, and
when √10·sin(18.435°−δ) falls below 0). So **1.5 Hz of unabsorbed offset eats
the entire margin with no noise on the line at all**, and everything past that
depends on the integrator catching up before the decisions go wrong.

#### Measured pull-in

A far end whose whole clock is off by δ at its 2400 Hz carrier, feeding the
receiver directly at 2400 bit/s, five seconds to converge, then 100 zero
bytes. "Clean run" is the longest unbroken stretch of the 800-bit payload:

| offset | clean | loop settles at | equaliser residual |
|---|---|---|---|
| +2.0 Hz | 800/800 | −1.975 Hz | 0.0238 |
| **+2.2 Hz** | **800/800** | −2.150 Hz | 0.0262 |
| **+2.4 Hz** | **437/800** | −2.315 Hz | 0.0306 |
| +2.6 Hz | 228/800 | −2.415 Hz | 0.0421 |
| +2.8 Hz | 109/800 | −1.464 Hz | **0.1995** |
| +3.0 Hz | 110/800 | −1.397 Hz | 0.1845 |
| −2.0 Hz | 800/800 | +1.975 Hz | 0.0453 |
| −2.4 Hz | 280/800 | +2.203 Hz | 0.2402 |
| +7.0 Hz | 24/800 | −0.414 Hz | 0.2324 |

**V.22bis 2.6** (read from the rendered page 4): *"The receiver shall be able
to operate with received frequency offsets of up to ± 7 Hz."* The pull-in edge
is **±2.2 to ±2.4 Hz at 2400** and **+6 Hz at 1200** (clean at +6.0, 77/800 at
+7.0). Both fall short, the fast rate by a factor of three.

Acquisition time at a 2 Hz offset, with the payload sent 2.5 s before the end
of each run:

| carrier on for | clean | loop reached |
|---|---|---|
| 3.0 s (payload at 0.5 s) | 158/800 | −1.698 Hz |
| 4.0 s (payload at 1.5 s) | 587/800 | −1.922 Hz |
| 5.0 s (payload at 2.5 s) | 800/800 | −1.975 Hz |

So **about 2.5 seconds of carrier before data is reliable at a 2 Hz offset** —
against the 600 ms + 200 ms the handshake allows between the rate switch and
data (`handshake.rs:52–54`).

#### Why the handshake hides this

Run the same offsets through the *full* two-modem handshake and every one of
them connects at 2400 and carries a 2000-bit payload perfectly, out to 7 Hz.
The reason is that everything before `Rising2400` runs at 1200
(`handshake.rs:206, 243, 254`), where the slicer offers four points
(`v22bis.rs:1018–1026`) whose decision regions are 90° wide, so the loop pulls
in easily and hands a converged integrator to the 2400 stage. **The ±2.2 Hz
limit is a cold-acquisition limit at 2400**, and it bites in exactly one
situation: anything that forces reacquisition while the far end is already at
2400 — a dropout, or the far end's retrain. Which brings us to §3.7.

### 3.4 Symbol timing

`Gardner::new(sps, 0.1)` at `v22bis.rs:587`, driven from an interpolated
matched-filter output (`v22bis.rs:656–669`, linear interpolation because
16 kHz / 600 baud is 26.667 samples per symbol, never a whole number).

The detector is the classic `midpoint · (this − previous)` correlation
(`shaping.rs:320–321`), normalised against a **running** mean power smoothed at
0.02 per symbol — a 50-symbol time constant (`shaping.rs:337`) — and clamped to
±1 (`shaping.rs:338`). Normalising against the *instantaneous* power would turn
16-QAM's 3:1 magnitude spread into huge apparent timing errors.

Two terms:

```rust
integral = (integral - gain/100 * error).clamp(-sps/8, sps/8);  // :276, :348-349
phase    = (-gain*error + integral).clamp(-sps/4, sps/4);       // :350-351
interval() = sps/2 + phase                                      // :288
```

`interval()` is read once per half-symbol instant (`v22bis.rs:667`), so the
proportional correction is applied **twice per symbol**: up to 2 × 0.1 = **0.2
samples of slew per symbol**, crossing half a symbol (13.33 samples) in at
least **67 symbols = 111 ms**. The integral is clamped at ±sps/8 = ±3.33
samples on a half-symbol interval, which is ±25 % of rate — enormous next to
the ±0.01 % V.22bis 2.5.1 allows, so a symbol-rate error is never the problem.

**The known false lock.** The double dibit alternates two points a quarter turn
apart, so its *midpoints* are all the same point: sampled there, every symbol
reads identical, the Gardner error is zero, and the loop is at a perfectly
stable wrong place. Nothing before that signal can reveal it, because
unscrambled binary 1 is a pure tone that reads the same turn wherever it is
sampled. The escape is `half_symbol_out` (`v22bis.rs:945–954`, eight symbols of
zero turn) and `shift_half_symbol` (`v22bis.rs:956–973`, a *kick* of half a
symbol rather than an exact relabel, because an exact move between two stable
points is still a move between two stable points). It is armed only in
`OfferingDoubleDibit` and `Scrambled1200` (`handshake.rs:189–193`). The
`V22_SKIP` sweep quoted in `v22bis_capture.rs:17–31` is the evidence it works.

### 3.5 Gain control

Per-symbol mean *power* (not magnitude — they differ by 5 % on this
constellation), one pole at 50 ms **at the symbol rate**
(`OnePole::starting_at(10.0, 0.050, fs/sps)`, `v22bis.rs:594`; `fs/sps` is 600,
so τ = 30 symbols). Gain is `√(10/mean_power)` floored at `mean_power ≥ 1e-9`
and clamped to `MAX_GAIN = 400` (`v22bis.rs:113, 706–708`).

Thirty symbols is fast. The power variance of 16-QAM about its mean of 10 is
32 (the code says so itself at `v22bis.rs:806–809`), so a 30-symbol EWMA
estimate has a standard deviation of √(32/60) = 0.73, i.e. 7.3 % in power and
**3.6 % RMS in amplitude** — a gain jitter of 3.6 % fed straight into a slicer
whose outer points have a ±1-unit-in-3 margin, so ~11 % of the margin spent on
the gain estimate alone.

It also runs in series with the equaliser's own gain freedom: the CMA drives
the *output* to constant modulus while the AGC drives the *input* to constant
mean power, two loops chasing the same degree of freedom at different speeds.

### 3.6 Rate decision

Judged from the **power variance** of the pre-equaliser point, over 128-symbol
windows (`v22bis.rs:830–855`):

```rust
self.rate = if variance < 0.16 * mean * mean { Bps1200 } else { Bps2400 };
```

The reasoning in the comments (`v22bis.rs:773–829`) is sound and hard-won:
measure the radius not the decided index (a ring at √10 can be rotated onto
itself, so the point index does not survive a wrong lock but the radius does);
measure before the equaliser (a collapsed equaliser reports small radii, reads
2400, offers sixteen points to a four-point signal, and stays collapsed);
measure variance rather than closeness to √10 (a real line moves individual
symbols off their ring while the shape of the whole is still plain).

It is nonetheless **not latched**. `set_rate` (`v22bis.rs:920–925`) is what the
handshake calls when it has *negotiated* the rate (`handshake.rs:271, 334`),
but it only resets the counters — 128 symbols (213 ms) later the variance test
runs again and can overrule the negotiation.

On the repository's own ground-truth vector, that costs seconds. Feeding
`tests/vectors/v22bis-2400.wav` (a real call that runs at 1200) to both
directions of the receiver:

| | calling direction | answering direction |
|---|---|---|
| reads 1200 from | 2.0 s | **6.0 s** |
| residual at 16 s | **0.031** | **0.194** |
| V.42 frames passing FCS | 5 | 5 |

The answering direction spent six seconds decoding a four-point signal with a
sixteen-point slicer — about 3600 symbols of wrong decisions feeding the
equaliser — and **never recovered**: its residual error sits at 0.19 for the
remaining eleven seconds, six times the clean-channel figure and within a
factor of 1.6 of the decision boundary (half the point spacing is 0.316). It
escaped at all only because the carrier flag dropped at 5 s and reset the
window. Nothing in the code would have got it out otherwise.

Against noise the threshold is generous: a real 1200 signal is still read as
1200 down to 0 dB of *wideband* SNR and flips to 2400 at −3 dB (the receiver's
noise bandwidth is ~1050 Hz out of 8000, so that is roughly +9 dB at the
symbol). Low risk on its own; it is the combination with the collapsed
equaliser above that matters.

### 3.7 The handshake, and the mechanism that is missing

`Handshake` (`handshake.rs:85–337`) walks clause 6.3.1.1 with the middles of
the stated ranges (`handshake.rs:40–59`): 3.3 s answer tone, 155 ms of
unscrambled ones heard, 456 ms of silence, 100 ms of double dibit, 270 ms of
scrambled ones heard, 600 ms to 2400, 200 ms to settle, 60 s of patience.
Patterns are recognised from the **turns** rather than the waveform
(`v22bis.rs:864–876, 927–943`), which is the only way to tell scrambled ones
from unscrambled ones — both descramble to ones.

Three deviations from the Recommendation, in increasing order of cost.

**a) The 600 ms is measured from the wrong event.** 6.3.1.1.1 c/d and
6.3.1.1.2 b/c both run the clock from *circuit 112 turning ON*, which is "at
the end of receipt of" the far end's double dibit. The code runs it from the
end of **its own** dibit (`handshake.rs:248–250` then `259–260` then `266–274`),
because `offered_2400` is latched as soon as the pattern is recognised —
five symbols, 8 ms into the far end's 100 ms. The answering modem therefore
starts 2400 about **100 ms late** and the calling modem about 8 ms early, a
window of roughly 65 symbols in which one end transmits sixteen points and the
other decides among four, with the equaliser adapting on the results.

**b) The receiver switches to 16-way decisions 150 ms later than allowed.**
6.3.1.1.1 d) says the transmitter begins 2400 at 600 ± 10 ms *and* "450 ± 10 ms
after circuit 112 has been turned ON the receiver may begin making 16-way
decisions". `handshake.rs:270–272` does both at 600 ms. The 450 ms allowance
exists to cover the far end's own ±10 ms.

**c) There is no retrain.** 6.4 is unambiguous: a retrain "shall be initiated
either by detection of loss of equalization **or by detection of unscrambled
repetitive double dibit 00 and 11 at 1200 bit/s from the distant modem**", and
a modem that sent one and got none back "shall return to the beginning of the
retrain signal ... and repeat the procedure until unscrambled repetitive double
dibit 00 and 11 is received from the remote modem". In this code,
`State::Connected(_) | State::Failed => {}` (`handshake.rs:283`) — once
connected, `step` does nothing at all, for ever.

**This is what a real call does to it.** Replaying
`captures/live-1788613347.wav` (a real 2400 call, both directions on one tap)
through `crates/datapump/tests/v22bis_capture.rs`:

```
  20.81s  connected          Negotiating -> Connected(Bps2400)
  21.00s  err 0.277  point (  0.02,  -0.35)
  21.89s  connected          hears DoubleDibit      <-- far end asks for a retrain
  22.20s  connected          hears DoubleDibit
  22.99s  connected          hears UnscrambledOnes
  ...
  26.26s  connected          hears DoubleDibit
  30.09s  connected          hears DoubleDibit
  32.63s  connected          hears DoubleDibit
  34.73s  connected          hears DoubleDibit
  31.00s  err 0.462  point (  0.48,   0.15)
  33.00s  err 0.519  point (  0.46,   0.28)
  37.00s  err 0.388  point (  0.00,   0.00)   <-- constellation collapsed to the origin
  39.50s  err 0.440  point (  0.00,   0.00)

final: Connected(Bps2400), 4648 bytes recovered  (72% "printable", all garbage)
```

Nine `DoubleDibit` and nineteen `UnscrambledOnes` detections **after** connect.
The receiver sees every one of them — `Pattern::DoubleDibit` is reported
correctly — and the handshake state machine is not looking. The far end is
doing exactly what 6.4 tells it to: retraining, getting nothing back,
retraining again, for the remaining fourteen seconds of the call. Meanwhile
this end's residual error runs 0.24 → 0.46 → 0.52 and the equaliser collapses
the constellation to the origin.

The status line says `Connected(Bps2400)` throughout.

**d) 6.5, operation after loss of line signal, is also absent**: no clamp on
recovered data (`v22bis.rs:878–882` pushes descrambled bits whatever the
carrier state), and no 100 ms window in which a retrain would be looked for.
**6.6, optional rate signalling** (S1, the R1/R2 dibits of Table 3), is
likewise absent; 6.6.1 is optional but 6.6.2, *responding* to a far end that
instigates one, shares the same blind spot as 6.4.

### 3.8 What a dropout does to the equaliser

A converged 2400 call, one second of line noise, one second to settle, then
100 zero bytes:

| noise level | in-band level | clean | residual before → after |
|---|---|---|---|
| −30 dB | 3.22e-3 | 13/800 | 0.049 → 0.246 |
| −35 dB | 1.81e-3 | 10/800 | 0.049 → 0.248 |
| **−40 dB** | **1.02e-3** | **16/800** | 0.049 → **0.252** |
| −45 dB | 5.72e-4 | 800/800 | 0.049 → 0.021 |
| −50 dB and quieter | ≤3.2e-4 | 800/800 | 0.049 → 0.021 |

The boundary is `CARRIER_ON`/`CARRIER_OFF` to within a hair. **If the noise on
a dropped line is loud enough to hold the carrier flag on — about 42 dB below
the signal that was there — nothing stops the equaliser adapting on it.**
`since_carrier` is only reset on a carrier *edge* (`v22bis.rs:642–647`), and
`SQUELCH = 1e-7` (`v22bis.rs:117`) is the only other gate; the AGC has already
amplified the noise to mean power 10 by the time the equaliser sees it
(`v22bis.rs:706–708`, gain up to 400). With ten seconds to recover, −40 dB
comes back and −35 dB does not.

The carrier loop itself does *not* wander during a dropout, because on exact
digital silence `nearest_point((0,0))` gives (1,1) and the error is exactly
zero; measured drift over a 15 s gap of −55 dB noise was 0.027 Hz. Nothing
resets `phase` or `frequency` on a carrier edge either way (`v22bis.rs:642–647`
resets only `since_carrier`), which is fine today and is one clamp away from
not being fine.

---

## 4. Weaknesses, with the symptom a user sees

| # | where | what | symptom | severity |
|---|---|---|---|---|
| W1 | `handshake.rs:283` | No retrain in either direction (V.22bis 6.4) | On `captures/live-1788613347.wav` the far end asks nine times in fourteen seconds and is ignored; residual 0.24 → 0.52, constellation at the origin, 4648 bytes of garbage, status still `Connected(Bps2400)` | **high** |
| W2 | `v22bis.rs:642–647, 753` | Equaliser adapts through a dropout whenever residual noise holds the carrier flag | One second of noise 35 dB below the signal takes residual from 0.049 to 0.25 and it does not recover in ten seconds — a burst of crosstalk kills a 2400 call permanently | **high** |
| W3 | `v22bis.rs:728–730` | Carrier loop pulls in only ±2.2 Hz at 2400 (spec 2.6: ±7 Hz), +6 Hz at 1200, and needs ~2.5 s to settle 2 Hz | Any reacquisition at 2400 — after a dropout or a far-end retrain — fails or takes seconds; the handshake hides it by acquiring at 1200 first | **high** |
| W4 | `fsk.rs:80, 106–110`; `framing.rs:52–57`; `v21.rs:176–180` | Carrier flag drops after 11–16 ms of quiet at ordinary levels; loss resets the framer / the bit clock | A 20 ms VoIP concealment gap costs one character on Bell 103 and a whole frame on V.21, every few seconds. V.21 Table 2 requires 20–80 ms ON→OFF and 300–700 ms OFF→ON; measured 13–36 ms and 1.4–3.4 ms | **high** |
| W5 | `v21.rs:156, 195–198` | V.21 bit clock is first order, no integrator: standing offset 8·sps·ε/d = 1707·ε samples on a flag stream | A fax burst whose 320 flag bits arrive and whose identification frame does not: 324/448 bits at +1.5 %, 62/448 at +2 % | medium |
| W6 | `framing.rs:95–100` | A too-fast far end slips into the stop bit, which is still mark | At +6 % every character is wrong and `framing_errors` reads **zero**; `login:` arrives as `ac af a7 a9 ae ba`. The one health counter is blind in that direction | medium |
| W7 | `v22bis.rs:840–855, 920–925` | Rate is re-decided from power variance every 128 symbols and overrides the negotiated rate | On `tests/vectors/v22bis-2400.wav` one direction read 2400 for a 1200 call for six seconds and the equaliser never recovered — residual stuck at 0.19 against 0.031 in the other direction | medium |
| W8 | `v22bis.rs:411–418` | No fixed compromise equaliser in the transmitter (V.22bis **2.3**, "shall") | Our outgoing signal is harder for the far end than it needs to be — plausibly part of why the live far end keeps asking to retrain | medium |
| W9 | `v22bis.rs:379–423` | No 1800 Hz (or 550 Hz) guard tone in the high channel, and no 1 dB level offset between channels (V.22bis **2.1/2.2**) | Answering into a network that expects one. Receiving one is harmless: measured, the select filter is −6.03 dB at 600 Hz and the matched RRC −49.9 dB, so a guard tone 6 dB below the data arrives 62 dB down and aliases to DC at the symbol rate | medium |
| W10 | `v22bis.rs:612, 637–641` | Carrier ON→OFF delay is level-dependent: `0.020·ln(L/5.62e-4)` | 46 ms only when the received level is 20 dB above threshold; 23 ms at 10 dB, 92 ms at 40 dB. V.22bis **3.2** specifies 40–65 ms flat | medium |
| W11 | `handshake.rs:222` | Only `Pattern::UnscrambledOnes` ends `Listening` | V.22bis 6.3.1.2.2 Note warns that modems in some countries answer with **2225 Hz** instead of unscrambled binary 1. 2225 Hz is −175 Hz from the 2400 carrier, which is not a legal quadrant change, so the run never builds and the call fails after 60 s | medium |
| W12 | `handshake.rs:248–250, 259–260, 266–274` | The 600 ms runs from the end of *our own* dibit, not the end of receipt of the far end's; and the receiver switches to 16-way at 600 ms where 6.3.1.1.1 d) allows 450 ms | ~100 ms in which the answering end decides among four points while sixteen arrive, with the equaliser learning from it; the first bytes of a 2400 connection are corrupt | medium |
| W13 | `v22bis.rs:878–882` | No output clamp on loss of line signal (V.22bis **6.5**) and no 100 ms retrain window | Garbage bits keep flowing into V.42 after the carrier goes, which is how a frame-check failure becomes a disconnect | medium |
| W14 | `v22bis.rs:594` | AGC time constant is 30 symbols; 16-QAM's own power variance is 32 about a mean of 10 | 3.6 % RMS gain jitter, about 11 % of the outer points' decision margin, spent on the estimator | low |
| W15 | `fsk.rs:114`, `framing.rs:86`, `v21.rs:208` | Hard-zero slicer, no running-mean offset removal | At the ±12 Hz V.21 clause 3 requires, one rail of the eye is 12 % narrower than the other — about 1.1 dB given away for nothing | low |
| W16 | `fsk.rs:81, 105` | `slow_env` is fed and never read | Dead weight left over from the ratio detector that was removed | low |
| W17 | `bell103.rs:87` | `self.symbol = framer.take_sampled()` overwrites with `None` on most samples | Only a caller that reads after every single sample sees anything; the GUI does (`crates/gui/src/engine.rs:545`), so it works today by call-pattern rather than by construction | low |
| W18 | `v22bis.rs:381–392` | `Signal::Silent` is honoured only while the queue is empty | Anything that calls `send_bits` before the handshake finishes makes the calling modem transmit during the silence 6.3.1.1.1 a) requires. The one caller throttles to 64 pending bits (`crates/modem/src/lib.rs:1967`), so it is latent | low |
| W19 | `v22bis.rs:129–130`; `fsk.rs:49–50` | Carrier thresholds are raw sample-scale constants | V.22bis 3.3 and V.21 8.3 both specify −43/−48 dBm at the line. Nothing calibrates; a live line at a different gain sits in the chatter band, where Bell 103 delivered 2 of 14 characters | low |

---

## 5. What the Recommendations require and the code does not do

Read from rendered pages of `docs/specs/T-REC-V.22bis-198811-I.pdf` (Table 1 on
page 3, Figure 2 and clauses 2.6/3.2 on page 4) and
`docs/specs/T-REC-V.21-198811-I.pdf` (clause 3 on page 1, Table 2 and 8.3 on
page 4).

| clause | requirement | status |
|---|---|---|
| V.22bis 2.1 | 1800 ± 20 Hz guard tone in the high channel (national option), or 550 ± 20 Hz | **not transmitted** (W9) |
| V.22bis 2.2 | Guard tone 6 ± 1 dB (or 3 ± 1 dB) below the data power; high-channel data ~1 dB below low-channel | **not done** (W9) |
| V.22bis 2.3 | "Fixed compromise equalization **shall** be incorporated in the modem transmitter" | **absent** (W8) |
| V.22bis 2.4 | √raised cosine, 75 % roll-off; group delay within ±150 µs over 900–1500 / 2100–2700 Hz | done (`v22bis.rs:411–418`, `shaping.rs:14–52`); the group-delay limit has never been measured |
| V.22bis 2.5.1 | 600 baud ± 0.01 %, 2400/1200 bit/s ± 0.01 % | exact (`v22bis.rs:394`) |
| V.22bis 2.5.2.1/2.5.2.2, Table 1, Figure 2 | quadrant change and point map | **exact**, checked against the rendered figure in all four quadrants |
| V.22bis 2.6 | receiver operates with ± 7 Hz offset | **±2.2 Hz at 2400, +6 Hz at 1200** (W3) |
| V.22bis 3.2 | circuit 109 OFF 40–65 ms after the level falls below threshold; ON 40–205 ms after it exceeds it following a dropout | level-dependent, 23–92 ms (W10) |
| V.22bis 3.3 | −43 dBm ON / −48 dBm OFF, ≥ 2 dB hysteresis; **must not respond** to the guard tones or the 2100 Hz answer tone during the handshake | hysteresis 5 dB ✓; levels uncalibrated (W19); **responds to the answer tone** — 2100 Hz is only 300 Hz from the 2400 carrier, inside the ±525 Hz passband. Measured harmless, because 300 Hz is exactly half the symbol rate so the decision-directed error cancels symbol to symbol; move the tone by V.25's ±15 Hz and it is still harmless, but it releases the equaliser's settling guard 3.3 s early |
| V.22bis 5.1/5.2 | 1 + x⁻¹⁴ + x⁻¹⁷, 64-ones detection at both ends | **exact**, including the reset ordering (`v22bis.rs:170–209`) |
| V.22bis 6.3.1.1.1 b/c | 155 ± 10, 456 ± 10, 100 ± 3, 270 ± 40 ms | done (`handshake.rs:44–50`) |
| V.22bis 6.3.1.1.1 d | 2400 at 600 ± 10 ms after 112 ON; 16-way decisions allowed from 450 ± 10 ms | wrong anchor, and 150 ms late on the receiver (W12) |
| V.22bis 6.3.1.1.1 f | ready to receive when **32 consecutive bits** of scrambled binary 1 at 2400 have been detected | fixed 200 ms instead, with no check on what is arriving (`handshake.rs:54, 276–281`) |
| V.22bis 6.3.1.2.2 Note | far ends that answer with 2225 Hz instead of unscrambled binary 1 | **not recognised** (W11) |
| V.22bis 6.4 | retrain on loss of equalisation **or on detecting the far end's double dibit**; repeat until answered | **absent** (W1) — and the far end on our own capture repeats, as 6.4 tells it to |
| V.22bis 6.5 | on loss of line signal: 109 OFF, clamp 104; on return, 100 ms retrain window then unclamp | **absent** (W13) |
| V.22bis 6.6 | optional rate signalling, S1 and the R1/R2 dibits of Table 3 | absent; 6.6.2 (responding) shares W1's blind spot |
| V.21 clause 3 | ±12 Hz of received drift must be tolerated | met to ±30 Hz, with a 12 % eye asymmetry (W15) |
| V.21 clause 3 fn 2 | ch.1 FA 1180 / FZ 980; ch.2 FA 1850 / FZ 1650; higher frequency is binary 0 | exact (`v21.rs:26`, pinned at `v21.rs:267–271`) |
| V.21 Table 2 | circuit 109: OFF→ON 300–700 ms (GSTN), ON→OFF 20–80 ms | **1.4–3.4 ms and 13–36 ms** (W4) |
| V.21 8.3 | −43 dBm ON / −48 dBm OFF, ≥ 2 dB hysteresis | hysteresis ✓ (5 dB), levels uncalibrated (W19) |

Bell 103 has no Recommendation; V.21 is its ITU counterpart and the code shares
the receiver, so the V.21 rows above apply to it wherever the tone pair is the
only difference.

---

## 6. How the numbers were got

All measurements were made against the unmodified crates from a scratch
package outside the tree (`datapump`, `dsp`, `line`, `ec` by path), plus one
run of the repository's own ignored replay test. Nothing in the tree was
changed.

- **Filter responses** (select at 600 Hz: −6.03 dB; matched RRC at 600 Hz:
  −49.86 dB; the pair: −55.9 dB) by evaluating the exact tap formulas of
  `fir_lowpass` (`shaping.rs:64–86`) and `rrc_taps` (`shaping.rs:14–27`).
- **FSK eye, bias and clock tolerance** by driving `FskDetector`,
  `Bell103Rx`, `AsyncFramer` and `v21::Receiver` with continuous-phase FSK at
  an exact fractional baud. (An earlier sweep that truncated samples-per-bit to
  an integer produced a spurious asymmetry — every truncation makes the far end
  *faster* than asked. The tables above use fractional timing.)
- **V.22bis carrier pull-in** by building a real `Transmitter` at a modified
  sample rate, so both its carrier and its baud scale — a far end with a
  crystal error. A Hilbert-based pure-carrier shift gave the same edge to
  within a few tenths of a hertz.
- **Dropout behaviour** by converging a real `Receiver` for five seconds, then
  substituting noise at a stated level relative to the measured signal power.
- **Real-call evidence** from `tests/vectors/v22bis-2400.wav` and
  `captures/live-1788613347.wav` via
  `V22_CAPTURE="F:/dialupmodem2/captures/live-1788613347.wav" cargo test -p datapump --test v22bis_capture -- --ignored --nocapture`
  (the path must be absolute; the test runs from the crate directory).

All 34 existing slow-mode tests pass on `pr-1-deps` at 48b9087
(`v22bis_handshake` 8, `v22bis_loopback` 22, `bell103_loopback` 4).

---

## 7. Open questions

1. **Why does no existing test see W1, W2 or W3?** Because no loopback test
   introduces a carrier frequency offset, a mid-call dropout, or a far end that
   retrains. Three tests would close the gap: an offset sweep to ±7 Hz at both
   rates, a noise-burst-in-the-middle test asserting that residual error
   returns below 0.05, and a far end that sends the 6.4 double dibit after
   connect and asserts that this end answers within the 1.2 s two-way
   propagation allowance 6.4 recommends.
2. **Is ±2.2 Hz a loop-gain problem or a decision problem?** Raising Kp widens
   pull-in and raises jitter. The cheaper fix may be to stop guessing: the
   handshake's unscrambled binary 1 is a pure tone at carrier − 150 Hz whose
   frequency can be measured outright and loaded into `self.frequency` before
   any decision is made, which is what the equivalent stage of V.34 does.
   Worth trying against the live capture before touching the gains.
3. **Why is the answering direction of `v22bis-2400.wav` stuck at residual
   0.19?** The six seconds of sixteen-point decisions on a four-point signal is
   the obvious cause, but the equaliser has no path back to blind adaptation
   (`equalizer.rs:121` is one-way), so it cannot be distinguished from ordinary
   line distortion without a re-run that forces `Bps1200` from the first symbol.
4. **Is the far end on `live-1788613347.wav` retraining because of our
   receiver or because of our transmitter?** It asks for a retrain, which per
   6.4 means it detected loss of equalisation *on our signal*. Our transmit had
   RMS 0.085 on that call and carries no compromise equaliser (W8). Decoding
   the second channel of that capture — which holds what we sent — with the
   receiver would separate the two.
5. **Does the guard tone matter on any real far end we can reach?** No capture
   in the tree shows one. Worth checking the spectrum of the high channel of
   `live-1788613347.wav` around 1800 Hz before building a transmitter for it.
6. **Should the Bell 103 framer oversample?** One sample per bit at the bit
   centre means one noise spike is one wrong character. Three samples and a
   vote costs nothing and would be measurable against
   `tests/vectors/bell103-300.wav` with added noise.
