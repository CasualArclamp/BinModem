# V.90: starting below the probe on a SIP line, and climbing back

What Rory asked for: *"can we make it start lower than the probe suggests just due to the state it's on a SIP
line then silently increase every 5 seconds or so"*.

The answer is yes to the first half exactly as asked, and yes to the second half with the interval an order of
magnitude longer than five seconds -- because a rate renegotiation on this line stops the data for about one and
a half seconds, and a climb of one rung wins 1333 bit/s. Five seconds apart, the climbs would cost more data
than they ever recovered. Everything below is built on that one measurement.

This is a design note, not a plan of record. Nothing in the repository has been changed by writing it.

---

## 1. What the Recommendation allows

All clause and table numbers are from ITU-T V.90 (09/98), read from rendered pages of
`docs/specs/T-REC-V.90-199809-I.pdf`.

**Up as well as down: yes, and nothing says otherwise.** Clause 9.6: "The rate renegotiation procedure can be
initiated at any time during data mode. Data signalling rate and spectral shaping parameters may change as a
result of rate renegotiation." No direction is named anywhere in 9.6 or its subclauses. The rate that changes is
the one the analogue modem writes into CP: Table 14/V.90 bits 20:24, "Selected digital modem to analogue modem
data signalling rate, an integer, drn, between 0 and 22. drn = 0 indicates cleardown. Data signalling rate =
(drn+20)\*8000/6 in CP". The only ceiling on it is the digital modem's own capability mask, Table 13/V.90 bits
18:33 and 35:46, "Bits set to 1 indicate data signalling rates supported and enabled in the transmitter of the
digital modem" -- and Jd is sent once, in phase 3, so the ceiling for every renegotiation in a call is whatever
Jd said at the last retrain. `Jd::enables` in `crates/datapump/src/v90/sequences.rs` is that mask, and
`renegotiate_within` already honours it.

**How often: no limit at all.** 9.6 says "at any time during data mode" and sets no minimum interval, no
maximum count and no cooling-off period. The only timing constraints in 9.6 are these, and none of them is an
interval:

- 9.6, third paragraph: "Rate renegotiation shall be initiated by the digital modem's transmitter only on the
  boundary of a data frame. Similarly, a digital modem's transmitter shall only respond to a rate renegotiation
  on the boundary of a data frame."
- 9.6.1: the digital modem "shall initiate a retrain according to 9.5.1.1 if it does not receive an E sequence
  within 5000 ms plus 2 round-trip delays after transmitting the Rd-to-R-bar-d transition."
- 9.6.2: the analogue modem "shall initiate a retrain according to 9.5.2.1 if it does not receive Ed within
  5000 ms plus 2 round-trip delays after sending the S-to-S-bar transition."
- 9.6.1.1.1 and 9.6.1.2.2: TRN2d is optional in a renegotiation and lasts "no more than 2000 ms".
- 9.6.2.1.2 and 9.6.2.2.3: SCR after S-bar is optional, "no more than 2000 ms". 9.6.2.1.6: SCR after Ed in the
  silent variant, "no more than 1000 ms".
- Figures 8, 9 and 10 mark the same bounds as "≤ 2 s" and "≤ 1 s".

The two five-second deadlines are the closest thing the Recommendation has to a rhythm, and they are watchdogs
that force a retrain, not spacing. So a minimum interval is entirely ours to choose, and has to be justified by
what it costs rather than by the Recommendation.

**If the digital modem will not grant the rate: there is no way for it to say so.** The procedure has no
refusal. 9.6.1.2.3: "After receiving a CP sequence, the digital modem shall send MP' sequences and proceed in
accordance with 9.4.1.4" -- and 9.4.1.5 has it send B1d "at the negotiated data signalling rate using the data
mode constellation parameters it received in CP". MP carries no downstream rate to argue with: Table 16/V.90
bits 24:27 are the "Maximum analogue modem to digital modem data signalling rate", the *upstream*. The digital
modem's three ways out are all outside the negotiation:

1. **Retrain.** 9.6.1: "The digital modem may initiate a retrain at any time during a rate renegotiation
   according to 9.5.1.1." Tone B on the line, and the call goes back through phase 2 (9.5).
2. **Cleardown.** 9.7: "Cleardown is indicated by setting drn to 0 in either CP by the analogue modem or MP by
   the digital modem."
3. **Silence.** It simply never sends Ed, and the analogue modem's own 5000 ms + 2 RTD watchdog (9.6.2) turns
   that into a retrain.

So "the far end refuses" is always, in practice, "the far end retrains". Which matters to this design, because a
retrain is where all of it resets.

**What happens to data: it stops, and the framing does not.** 9.6, second paragraph: "The digital modem's
transmitter and the analogue modem's receiver shall maintain data frame synchronization during rate
renegotiation." The DTE interface is flow-controlled off at both ends: 9.6.1.1.1 "The digital modem shall turn
OFF circuit 106"; 9.6.2.1.1 "The analogue modem shall turn OFF circuit 106"; 9.6.1.2.1 and 9.6.2.2.1 clamp
circuit 104 to binary one. Circuit 106 comes back at the end of B1 / B1d (9.4.2.5, 9.4.1.5). Circuit 109 is not
turned off anywhere in 9.5 or 9.6, so the DTE never sees carrier drop -- no `NO CARRIER`, no new `CONNECT`.

**The silent period is a different, longer procedure, and we must not ask for it.** Table 14/V.90 bit 30: "Set
to 1 indicates a silent period is requested. This may be used during rate renegotiation (see 9.6)". 9.6.2.1.3:
"If the analogue modem wishes to recondition its echo canceller, it shall send CPs sequences", and 9.6, first
paragraph: "This procedure can also be used to retrain the analogue modem's echo canceller without going through
a complete retrain. Only the analogue modem can request this second procedure." Figure 10 shows what it costs:
Ed followed by silence (9.6.1.2.5, "PCM codewords with magnitudes represented by Ucode 0"), SCR for up to
1000 ms while the echo canceller reconditions, then a *second* R sequence -- Rt for 384T and R-bar-t for 24T
(9.6.1.2.6, 9.6.2.1.7) -- and a second MP round. That is a whole extra round trip plus up to a second, for
something a climb has no use for. `Cp::silence` defaults false in `sequences.rs`; it must stay false for every
renegotiation this design makes.

**And clause 8 says the training signal does not change.** 8.6: "During initial train or retrain, signals TRN2d,
MP and Ed use the spectral shaping parameters defined by CPt. During rate renegotiation, TRN2d, MP and Ed use
the spectral shaping parameters as used in the preceding data mode along with the K previously derived from CPt.
B1d and the following data mode signals shall use the spectral shaping parameters defined by CP." So a
renegotiation changes only the data mode constellation; the equaliser sees the same training constellation it
has already learned. `renegotiate_within` is right to keep `choice.training` unchanged, and this is also why
9.6.1.1.1 can make TRN2d optional at all -- see §6, WP-7.

---

## 2. What it costs

### 2.1 The pieces

Every number here is either a clause quantity or a constant in the code, at T = 1/8000 s.

| piece | length | where it is fixed |
|---|---|---|
| S | 128T = 16.0 ms | 9.6.2.1.1; `v34::signals::S_SYMBOLS` = 128 |
| S-bar | 16T = 2.0 ms | 9.6.2.1.2; `S_BAR_SYMBOLS` = 16 |
| Rd | 384T = 48.0 ms | 9.6.1.1.1; `digital::RD_SYMBOLS` = 384 |
| R-bar-d | 24T = 3.0 ms | 9.6.1.1.1; `R_BAR_FRAMES` = 4 × `INTERVALS` = 6 |
| TRN2d | 2040T = 255.0 ms | optional in 9.6.1.1.1, but `digital::TRN2D_FRAMES` = 340 always sends the 9.4.1.2 minimum |
| MP | 86 bits, or 188 with precoder coefficients, padded to whole frames of D = drn+8 bits | Table 16; `sequences::mp_bits`; 4 frames = 3.0 ms typical, 8 frames = 6.0 ms with precoding |
| CP | 275 bits with one distinct constellation, 411 with two, 547 with three | Table 14 via `Cp::to_bits`: 17 sync + (7 + 8·masks) blocks of 17 + 3 fill |
| CP on the line | 138, 206 or 274 symbols = 17.2, 25.8 or 34.2 ms | 4 points a symbol; Table 13 bit 48 clear, which every far end we have met sends, including `SERVER_JD` in `sequences.rs` |
| E | 20 bits = 10 symbols = 1.25 ms | `signals::E_BITS` = 20 |
| Ed | 2 frames = 12T = 1.5 ms | 8.6.2; `ED_FRAMES` = 2 |
| B1d | 48 frames = 288T = 36.0 ms | 8.6.1; `B1D_FRAMES` = 48 |

### 2.2 The ladder, with one-way delay *d*

Analogue-initiated, no silence requested, taking the clock from the moment the analogue modem starts S:

```
t = 0            analogue: S (16 ms), S-bar (2 ms), then CP over and over
t = d + ~2 ms    digital hears S, clamps 104, and switches Data -> Rd at the next frame
                 boundary. THE DOWNSTREAM DATA STOPS HERE.
t = d + 53 ms    Rd (48) + R-bar-d (3) done
t = d + 308 ms   TRN2d (255) done. A whole CP arrived back at d + ~52 ms, long before,
                 so the first MP it sends is already MP'
t = 2d + 53 ms   analogue sees the R-bar-d transition: `turned()`, new frames
t = 2d + 311 ms  analogue hears MP'; 9.4.2.3 completes the current CP (<= 34 ms) and
                 sends CP' (<= 34 ms)
t = 2d + 379 ms  CP' complete; 9.4.2.4 sends E (1.25 ms), then B1 at the new rate
t = 3d + 382 ms  digital hears CP', completes its MP' (<= 6 ms), sends Ed (1.5 ms)
t = 3d + 420 ms  B1d (36 ms) done. THE DOWNSTREAM DATA RESUMES HERE.
t = 4d + 420 ms  the analogue modem's decoder sees it
```

The gap in the analogue modem's decoder is (4d + 420) − (2d + 2), so:

> **a renegotiation stops the downstream for one round trip plus about 0.42 s.**

The upstream gap is within a few tens of milliseconds of the same. Numbers:

- **the simulation's VoIP line**, `with_delay(0.6, FS)` = 0.6 s each way, which `a_voip_length_round_trip_connects`
  measures as a 1.15-1.3 s round trip: **about 1.6 s**.
- **Rory's line live**, 1.1-1.6 s round trip: **1.5 to 2.0 s**.
- **a 20 ms round trip** (the other simulated line): about 0.44 s.
- **worst case against a real server**: it may choose TRN2d up to 2000 ms (9.6.1.1.1), which is its choice and
  not ours, so up to about **3.7 s** on the live line. Our own digital modem always sends 255 ms.

### 2.3 What that means, and what the user sees

At 50 666 bit/s a 1.62 s gap is 82 kbit of downstream that never happened. One rung of drn is 8000/6 = 1333
bit/s. So:

> **one rung of climb pays back the renegotiation that won it after about 62 seconds. Three rungs pay back
> after about 21 seconds.**

Climbing a rung every five seconds would spend a third of the line on the climbs and would be behind on
throughput for the first minute of every one of them. That is the whole reason §4.4 climbs in one step rather
than four, and §5 defaults the interval to thirty seconds rather than five.

The user sees and hears:

- **At the DTE:** CTS drops for the length of the gap and comes back. DCD never drops, there is no result code
  and no `CONNECT` line. A terminal session stalls for a second and a half and carries on.
- **Under V.42:** `ec::lapm::t401_for_line(50666, 1200)` is `max(1000, 1800) + 27` ≈ 1.83 s. The gap is 1.62 s.
  Every climb therefore sits just inside T401, and any jitter on top of it costs a retransmission and turns the
  1.6 s stall into a 3 s one. This is the sharpest practical argument against climbing often.
- **On the line, if monitored:** Rd is the loudest data-mode codeword in the pattern "+ + + − − −" at 8000
  symbols a second, which is a 1333 Hz tone for 48 ms; then TRN2d, scrambled ones through the shaper, which is a
  quarter-second of hiss; then the brief MP/Ed/B1d. Upstream it is S and S-bar (a 2400-baud four-point warble)
  and then CP. It sounds like a short chirp and a hiss -- nothing like a retrain, which adds 70 ms of silence,
  Tone A and Tone B, and the whole of phases 2 to 4.

---

## 3. The machinery this is built on

From `fix/v90-fallback` (13 commits, `main..fix/v90-fallback`), in
`crates/datapump/src/v90/analogue.rs` unless said otherwise:

- **`Decisions`** watches data mode symbol by symbol. `symbol()` measures each decision's distance to the nearer
  of the two levels either side and calls it a miss if it went more than `MISSED` = 0.8 of the way to the
  boundary. **`Look`** is a quarter-second of that (`MARGIN_EVERY` = 0.25): symbols, misses, summed error power,
  and `worst`, the mean error power over the worst 32 ms block (`BLOCK` = 256).
- **The storm rule** (`Decisions::storm`) throws away looks that were a jitter buffer's doing rather than the
  line's: a stretch of misses bounded by `STORM_GAP` = 128 clean decisions, with at least `STORM_MISSES` = 6
  misses, whose missed decisions carried more than `STORM_GARBLED` = 0.03 of the sound under the stretch, and
  which lasted less than `STORM_SHORT` = 512 symbols, is made-up audio. `Decisions::line` does the same for a
  hole -- `HOLE` = 16 symbols of line under `HOLE_LEVEL` = 1e-3 of its own level. Either spoils the look, the
  look before it and `HOLD_AFTER` = 1 look after it.
- **The evidence** (`Decisions::weigh`) adds `(misses − MISSES_ALLOWED).clamp(0, LOOK_AT_MOST)` per standing
  look, decayed with a time constant `REMEMBERED` = 15 s, and fires at `ENOUGH` = 12.5. `recent` keeps the last
  `REMEMBERED / MARGIN_EVERY` = 60 standing looks.
- **`worst()`** is the third-worst (`WORST_OF` = 3) of those looks' worst blocks. **`rms()`** is the error RMS
  over all of them.
- **`worse`** is a field on `Modem`, a ratchet: `self.worse = self.worse.max(worst().max(receiver) / expected)`.
  It is *the factor by which the line is worse than the DIL said*, and the only thing the fall-back scales the
  route by.
- **`renegotiate_within(most, least)`** re-chooses from the stored `Route` with that factor applied --
  `shaping::scaled(route, left * worse * worse)` multiplies every codeword's spread by `sqrt(left) * worse` --
  then `dil::choose_shaped(&scaled, law, limit, slow_enough, shaping)`, retrying with `worse` divided by
  `BELIEVED_LESS` = 1.0594 until the step is inside `MOST_DROPPED` = 8 bits a frame.
- **`dil::choose_shaped`** picks the largest K whose constellations stand `dil::SPACING` = 10 noises apart inside
  Table 15's power limit, with `widest()` then taking the most room the power allows for that K.
- **Every entry to data mode wipes the watch**: `self.decisions = Decisions::new(&frames.levels)` and
  `margin_at = samples(MARGIN_SETTLE = 2.0)` in `stage_step`. `worse` survives; the window does not.

---

## 4. The design

### 4.1 Starting lower is a cap in rungs, not a factor on the noise

Two ways to express "lower than the probe says" are available, and they are not equivalent.

A **factor on the route's noise** is the code's existing currency: `worse`, and `shaping::scaled`. But it is a
bad unit for this, because on a mu-law ladder the rate is not proportional to the log of the noise. Modelling
`ladder`, `quietest` and `widest` against Table 1's mu-law levels and a 12724 power limit (a rough model of
`average_power`, with uniform weights instead of the modulus encoder's; **an estimate, not a measurement of the
code** -- WP-1 pins the real numbers):

| the route's noise scaled by | the drn that comes out |
|---|---|
| ×1.00 | 20 |
| ×1.41 | 20 |
| ×1.78 | 19 |
| ×2.50 | 19 |
| ×3.17 | 18 |
| ×4.00 | 17 |

A factor of two is sometimes worth a whole rung and sometimes worth none: the quiet rungs of the ladder go
first, the loud ones stay, and the binding constraint at the top is the power limit rather than the spacing.
"Start three rungs lower" cannot be said in that unit.

A **cap on the first CP's drn** can, exactly, and the code makes it a one-line change, because
`dil::choose_shaped` already takes `enabled: impl Fn(u8) -> bool` and `renegotiate_within` already builds one of
those from a rate ceiling. So:

> **`finish_dil` keeps its uncapped `shaping::choose` exactly as it is, records `ceiling = asked.choice.data.drn`
> and the `(shaping, left)` it picked, and -- only if the margin is non-zero -- calls
> `dil::choose_shaped(&shaping::scaled(&route, left), law, limit, |drn| jd.enables(drn) && drn + margin <=
> ceiling, shaping)` for the CP it actually sends.**

Three details that are easy to get wrong:

1. **The shaping comes from the uncapped choice and is then held fixed.** `shaping::choose` picks a shaping by
   which one carries the most; at a capped rate it may well pick `Shaping::NONE`, and then the climb could never
   reach a ceiling that needed shaping to get to. Taking the shaping from the uncapped answer and only
   re-choosing the rate is what `renegotiate_within` already does, and it is also what 8.6 requires of every
   later renegotiation.
2. **The V.34 comparison stays uncapped.** `finish_dil` bails to V.34 when `rate < self.settings.v34_receive`.
   That must keep testing the *uncapped* rate; otherwise asking for a margin would silently hand marginal calls
   to V.34.
3. **The cap saturates rather than failing.** `drn + margin <= ceiling` with a ceiling near the bottom leaves
   nothing enabled; fall back to the slowest rate Jd enables rather than returning `None` and failing the call.

### 4.2 What the climb waits for

The climb is a fourth branch of `watch_margin`, reached only when the fall-back branch did not fire. All five
conditions must hold:

1. **In data mode, settled, and nothing else pending.** `in_data_mode()`, `!wants_retrain`, `unreadable == 0`.
2. **The window is full of looks that stood.** `decisions.recent.len() >= CLIMB_LOOKS` (proposed 40, two thirds
   of the 60 slots, so ten seconds of judged line). Looks the storm rule or the hole rule threw away simply do
   not count, so a line that is a third garbage takes fifteen seconds to fill the window rather than ten, and a
   line that is mostly garbage never fills it at all. The climb waits on evidence it has, never on evidence it
   wishes it had.
3. **The evidence is near zero.** `decisions.evidence < CLIMB_QUIET` (proposed 0.25, against `ENOUGH` = 12.5).
4. **The window's *worst* 32 ms block would still be read cleanly at the higher rung.** Not the third-worst:
   see §4.3.
5. **Long enough since the last renegotiation of any kind.** `now >= last_renegotiation + climb_every`
   (default 30 s -- §2.3, and §5).

Condition 4 is the one that decides. It is asked in exactly the currency the fall-back uses, by exactly the same
function:

```
peak     = max over decisions.recent of look.worst            // a new Decisions::peak()
factor   = peak.sqrt().max(receiver) / expected               // expected as watch_margin computes it today
candidate = dil::choose_shaped(&shaping::scaled(route, left * factor * factor),
                               law, limit,
                               |drn| jd.enables(drn) && drn <= climb_ceiling,
                               shaping)
```

and the climb happens if and only if `candidate.data.drn > in_use.drn`. That is: *the climb asks the question
the DIL asked, with the noise the last fifteen seconds actually delivered instead of the noise a few hundred
milliseconds of DIL delivered, and moves only if the answer has changed.* It is not "nothing bad happened
lately"; it is a measurement that has to clear `SPACING` = 10 noises at the higher rung with the power limit
respected, the same bar the first choice had to clear.

### 4.3 Why a quiet patch cannot fool it

This is the part that has to be right, because Rory's line is quiet most of the time and bad for tens of
milliseconds at a stretch.

**The peak, not the third-worst.** `Decisions::worst()` deliberately discards the two worst looks in the window
so that no single 32 ms window can drag the call's rate down -- the comment on `WORST_OF` says so. For the climb
that is exactly backwards: a burst every ten seconds lands in at most one or two looks out of sixty, so
`worst()` would never see it and would wave the climb through onto a rung the bursts will break. The climb
therefore takes the **maximum** of `look.worst` across the window. One bad 32 ms block anywhere in the last
fifteen seconds of judged line, and the higher rung does not clear the bar. The two rules read the same
measurements and take opposite tails: **the fall-back is deaf to one bad window, the climb is deaf to nothing.**
That asymmetry is the anti-hunting device, and it is free.

**The decay, not a run length.** "No misses for five seconds" is precisely the rule a VoIP line defeats, because
the gaps between its disturbances are seconds long. The evidence accumulator was built for this: it adds every
standing look's excess misses and decays them with a 15 s time constant, so bursts that are seconds apart add
up instead of cancelling. Read the other way round, it is a memory of the last disturbance, and
`CLIMB_QUIET` = 0.25 is chosen so the memory is long: a look with three misses adds 1.0 and takes
15·ln(4) ≈ 21 s to decay under the threshold; a look with five misses adds 3.0 and takes 15·ln(12) ≈ 37 s. A
disturbance recurring up to twenty seconds apart therefore keeps the climb shut indefinitely, and the quiet
patch between two of them never opens it.

**And what is thrown away is not counted as good.** Concealed packets, slips and holes spoil their looks. Those
looks are absent from `recent` entirely, which means they cannot lower the peak, cannot lower the evidence and
cannot fill the window. A line that is half jitter buffer takes twice as long to earn a climb and, correctly,
is never argued *up* by the very disturbances a lower rate cannot cure.

### 4.4 One informed step, not four blind ones

Because `candidate` is the measurement's own answer rather than "the next rung up", a good line climbs from the
capped rate back to the ceiling in a **single renegotiation**. Given §2.3's arithmetic that is not a nicety: four
one-rung climbs cost 6.5 s of dead air and, at a rung apiece, are behind on throughput for a minute each. One
climb of three rungs costs 1.6 s and is ahead after twenty seconds.

A guard mirroring `MOST_DROPPED` -- call it `MOST_CLIMBED`, 8 bits a frame -- keeps a wild measurement from
asking for the top of the ladder in one go. With a start margin of two to four rungs it never binds, which is
the point: it is there for the case nobody has thought of.

### 4.5 Not oscillating

Four devices, in order of how often they act:

**The dead band.** The fall-back fires at `evidence >= 12.5`; the climb requires `evidence < 0.25`. Between them
is a band of fifty to one in which neither does anything at all. Two rules reading one accumulator cannot
alternate across a gap that wide.

**The ceiling.** `climb_ceiling` starts as the drn the uncapped `shaping::choose` picked at `finish_dil`. The
climb never asks for more. So "climbs back to where the DIL said" is literally true, and there is no mechanism
by which the climb can ever exceed the probe -- which also means this feature can never make a call faster than
today's code would have made it, only less damaged on the way there.

**The burn.** `climbed_at` records when a climb landed. If, within `CLIMB_PROOF` (proposed 20 s, one evidence
window and a little) of landing, the fall-back fires, or `wants_retrain` is set, or the frame place is lost,
then the rung that was climbed onto is **burned**: `climb_ceiling = drn_climbed_to − 1`, and `climb_failures +=
1`. A rung that has failed once is never asked for again in this call. A fall-back that happens *outside* that
window is ordinary weather and burns nothing -- it lowers the rate, the climb may in time earn it back, and the
evidence test is what stops it earning back a rate on a line that is genuinely bad.

**The stop.** After `CLIMB_FAILURES` = 2 burns, the climb stops for the rest of the call. Two is enough: with
the ceiling falling a rung per failure, a start margin of three rungs can only fail three times anyway, and a
line that fails twice is telling us the measurement is wrong rather than the rung.

**And what resets it.** A **retrain** resets everything, and does so for free: `startup.rs` builds a fresh
`analogue::Modem` after every retrain, so the ceiling, the burns, the failure count, `worse` and the window all
go, and the start-low cap is applied afresh to the new DIL. That is right -- a retrain is a new measurement of a
line that has changed. A **renegotiation initiated by the far end** clears the window (data mode is re-entered,
so `Decisions::new` runs) and restarts the interval, but keeps the burns: the far end sending Rd tells us
nothing about why our rung failed, and our CP still carries our own choice of rate.

One more thing must be relaxed on a successful climb. `worse` is a ratchet that only grows; if it is left alone,
one burst in the first ten seconds of a call pins every later fall-back to it forever. On a climb that lands,
set `self.worse = factor.max(1.0)` from the measurement the climb was made on. The fall-back's own `.max()`
takes it from there.

### 4.6 Where the two watches meet, and how they cannot fight

They are one watch. The climb is a branch inside `watch_margin`, taken only in the arm where
`decisions.weigh(look)` returned false -- so a single look can produce a fall-back or a climb or neither, never
both. Both go out through one function; `renegotiate_within` should be split so both share it:

```
fn renegotiate_using(&mut self, most: u32, least: u32, worse: f64) -> bool    // today's body, `worse` a parameter
pub fn renegotiate(&mut self, most: u32) -> bool                              // -> renegotiate_using(most, 0, self.worse)
fn climb(&mut self) -> bool                                                   // -> renegotiate_using(ceiling_rate, current_rate + 1 rung, peak factor)
```

`in_data_mode()` is false while `renegotiating`, so neither can start while the other is in flight, and
`MARGIN_SETTLE` gives both two seconds of quiet after every one. The whole feature is then: one extra call in
`finish_dil`, one method on `Decisions`, one branch in `watch_margin`, and a parameter on a function that
already exists.

### 4.7 What the far end does about it

- **It accepts** -- the usual case, because there is nothing for it to accept or refuse (§1): it sends MP', Ed,
  B1d at the constellation our CP named.
- **It retrains.** `RetrainWatch` and the existing `wants_retrain` path handle it; everything resets, we start
  low again against a fresh DIL, and nothing here needs new code.
- **It goes quiet.** The 5000 ms + 2 RTD deadline that `begin_renegotiation` already arms (`RENEGOTIATION_ED` +
  2·round\_trip + 0.1, "no Ed in the rate renegotiation") turns it into a retrain.
- **It comes back with a lower rate.** It cannot, downstream. MP's drn is the upstream cap (Table 16 bits
  24:27), and `prepare_upstream` already honours it. A far end that wants us slower downstream has to retrain or
  clear down.

---

## 5. What the user can set, and where

### 5.1 There is no standard command for this

V.250 (07/2003) is close and then stops. 6.4.8, "Seamless rate change enable (+MSC)", is the only rate-change
control it has, and its NOTE 1 says in as many words: "The addition of other subparameters to control other
aspects of Seamless Rate Change Operation, and control of V.90 SRC Operation, is for further study." The ITU
reserved the space and never filled it, and 5.4.1 forbids us filling it for them: "All other + leadin character
sequences are reserved for future standardization by the ITU-T", and 5.8.1 item 1 repeats that "the '+' prefix
is reserved for future use in this and other standards and should not be used for non-standard purposes."

The same clause names where a non-standard knob *does* belong. 5.8.1 item 4: "S-parameter numbers". So:

**Two manufacturer S-parameters, in `crates/at/src/registers.rs`, with `clause: "extension"`** -- the convention
the file already uses for S2 and S12:

| | default | range | meaning |
|---|---|---|---|
| **S90** | **0** | 0-6 | rungs of drn below the DIL's own choice that the first CP asks for. 0 is today's behaviour exactly. |
| **S91** | **30** | 0-255 | seconds between climbs. 0 turns the climb off, so S90 alone gives a plain fixed-margin call. |

`+MS`'s `<max_rate>` is *not* the right place: it already means "never exceed this", which is a ceiling on the
whole call, where S90 means "start here and come back up". Conflating them would make the climb look like a
violation of `+MS`. They compose correctly as they stand: `+MS` bounds `climb_ceiling`, S90 offsets the start
below it.

### 5.2 Getting them to the data pump

`Analogue::new(fs)` has one caller, `crates/modem/src/lib.rs:2283`. Follow `digital::Habits`, which is already
the project's word for "how this end behaves":

```
pub struct Habits { pub start_low: u8, pub climb_every: Option<f64> }
impl Habits { pub const POTS: Self = ...; pub const VOIP: Self = ...; }
pub fn Analogue::with_habits(fs: f64, habits: Habits) -> Self
```

carried into `analogue::Settings` where `Settings::new` builds it, alongside `round_trip` and `v34_receive`,
which are the same kind of thing. `Analogue::new(fs)` keeps today's behaviour, so nothing that does not ask
changes.

### 5.3 In the window

The GUI's settings are asserted onto the modem as AT lines (`app.rs::settings` → `assert_settings`) and
remembered by name (`remembered.rs`, `to_remember` / `from_remembered`). So:

- One checkbox, **"this line is VoIP"**, in the same group as the carrier and rate boxes, with a hover
  explaining what it does: *"Start below what the line probe says and work back up. Use it on a SIP or
  softphone line, where the probe measures a quiet moment and the call meets the jitter buffer later."*
- Checked, `settings()` emits `ATS90=3S91=30`; unchecked it emits `ATS90=0S91=0`, so the window and the modem
  cannot disagree -- the property `remembered.rs` exists to protect.
- Remembered as `voip_line` (a bool), not as the two numbers, so the meaning survives a change of default.
- A spinner for the margin belongs behind an "advanced" disclosure at most. Two numbers on the main window is
  two numbers nobody will set correctly.

### 5.4 Should it be on by default for everyone? No.

On an ordinary line the DIL is a good measurement and the fall-back handles what it misses: the tests
`a_clean_line_and_a_drifting_clock_are_left_at_their_rates` and
`a_softphone_s_slips_are_left_at_their_rates_and_its_gain_control_never_engages` show a clean call coming up at
54 666 and staying there with no renegotiations at all. Defaulting S90 to 3 would throw away 4000 bit/s on every
such call and then spend 1.6 s of dead air getting it back. That is a worse modem for everyone in order to be a
better one for Rory.

**Off by default; on when the user says the line is VoIP.** Deciding it automatically is attractive -- the
route's own spread already tells a softphone's sample-rate conversion apart (`ladder`'s comment on codewords
"where a softphone's sample rate conversion has run out of headroom"), and `Analogue::holes()` counts
concealment -- but that is a second design, wants its own evidence, and should not hold this one up. Note it as
future work and leave the box to the user.

**Why 3 rungs.** Not a round number: it is what this line's own disturbances have measured.
`a_burst_shorter_than_the_block_is_still_held_against_the_rate` records that "ten milliseconds of it takes
54 666 to 50 666 over a 0.6 s one" -- three rungs, for the mildest disturbance the fall-back will act on over
this round trip. Starting three rungs low starts the call where the mildest VoIP disturbance would have taken it
anyway, and the climb recovers the rest when the line turns out to be better than that.

---

## 6. Work packages

Rules as elsewhere in this project: one package is one sitting; files are disjoint within a wave; every package
names its tests; a constant that encodes a measurement carries the measurement in its doc comment.

### Wave 1

#### WP-1: The start-low cap, and what a rung is worth (M)

- **Depends on:** `fix/v90-fallback` merged.
- **Clauses:** 9.3.2.10; Table 14 bits 20:24; Table 13 bits 18:33, 35:46; 8.6.
- **Files:** `crates/datapump/src/v90/analogue.rs` (`Settings`, `finish_dil`, new `START_LOW` doc constant);
  `crates/datapump/src/v90/startup.rs` (`Habits`, `Analogue::with_habits`).
- **Description:** `Settings` gains `start_low: u8`. `finish_dil` keeps its existing uncapped `shaping::choose`,
  records `climb_ceiling` and `(shaping, left)` from it, compares the **uncapped** rate against `v34_receive` as
  today, and then -- only when `start_low > 0` -- re-chooses the data CP with
  `dil::choose_shaped(&shaping::scaled(&route, left), ..., |drn| jd.enables(drn) && drn + start_low <= ceiling,
  shaping)`, saturating to the slowest rate Jd enables rather than failing.
- **Tests** (unit, in `analogue.rs`; call-to-call in `v90_call.rs`):
  - `a_cap_of_three_rungs_asks_for_three_rungs_less_than_the_dil_chose`
  - `the_cap_never_asks_for_a_rate_the_far_end_did_not_enable` - a Jd with holes in its mask.
  - `a_cap_deeper_than_the_ladder_asks_for_the_slowest_rate_the_far_end_enables`
  - `the_cap_does_not_change_the_shaping_the_dil_chose` - a band-edge route where shaping wins, capped: the
    `Shaping` in CP and CPt is the uncapped one.
  - `a_capped_route_that_beats_v34_uncapped_is_still_v90` - the `v34_receive` comparison stays uncapped.
  - `a_rung_is_not_a_factor_on_the_noise` - the measurement §4.1 estimates, taken from the real
    `dil::choose_shaped`: tabulate the drn that comes out of one stored route scaled by 1, 1.41, 2, 2.83 and 4,
    and assert the table, so the next person does not have to guess.
  - `a_voip_line_capped_three_rungs_low_connects_and_carries_data_there`

#### WP-2: The climb's evidence (M)

- **Depends on:** none (parallel with WP-1; different functions in the same file -- schedule them in one
  sitting if that file's churn is a worry).
- **Files:** `crates/datapump/src/v90/analogue.rs` (`Decisions::peak`, `CLIMB_QUIET`, `CLIMB_LOOKS` and their
  doc comments).
- **Description:** `Decisions::peak()` returning the maximum `look.worst` over `recent`, and the two constants,
  each documented with the arithmetic in §4.3 (a three-miss look is 21 s of silence; a five-miss look is 37 s).
- **Tests** (unit):
  - `the_peak_is_the_worst_block_in_the_window_where_the_fall_back_takes_the_third_worst` - feed six looks, one
    much worse, and assert `peak()` sees it and `worst()` does not.
  - `a_quiet_patch_between_disturbances_is_not_a_quiet_line` - the headline test: looks with one bad one every
    forty looks (ten seconds), run for two minutes; `evidence` never falls below `CLIMB_QUIET` and `peak()`
    stays high, while `worst()` drops to the clean value.
  - `one_bad_look_keeps_the_climb_shut_for_twenty_seconds` - the decay, to within a look.
  - `looks_a_jitter_buffer_spoiled_do_not_fill_the_window` - a storm and a hole every other look: `recent` grows
    at half speed and `peak()` never takes the garbage in.

### Wave 2

#### WP-3: The climb (M)

- **Depends on:** WP-1, WP-2.
- **Clauses:** 9.6; 9.6.2.1.1-9.6.2.1.4; Table 14 bit 30 (must stay clear).
- **Files:** `crates/datapump/src/v90/analogue.rs` (`renegotiate_using`, `climb`, `watch_margin`, the
  `climb_ceiling` / `climbed_at` / `climb_failures` fields, `CLIMB_PROOF`, `CLIMB_FAILURES`, `MOST_CLIMBED`).
- **Description:** §4.2 and §4.5 as written. `renegotiate_within` becomes `renegotiate_using(most, least,
  worse)`; `climb()` calls it with the peak factor; `watch_margin` gains the branch in the arm where `weigh`
  returned false; the burn is applied where the fall-back, `wants_retrain` and the lost-place path already are;
  `self.worse` is relaxed to the climb's own factor when a climb lands.
- **Tests** (unit):
  - `a_climb_asks_for_what_the_measurement_supports_and_never_more_than_the_dil_s_best`
  - `a_climb_asks_with_bit_thirty_clear` - no silent period, ever.
  - `a_look_produces_a_fall_back_or_a_climb_and_never_both`
  - `a_burned_rung_is_never_asked_for_again`
  - `a_landed_climb_relaxes_the_ratchet_it_was_measured_against`

#### WP-4: What it costs, measured (S)

- **Depends on:** none.
- **Files:** `crates/datapump/tests/v90_renegotiation_cost.rs` (new).
- **Description:** drive `renegotiate` by hand over `v90::network::Network` at several delays with known data
  going down, and measure the gap in arriving bits directly. This is the file that will tell us if §2.2 is
  wrong, and it wants to exist before anything is tuned to the number.
- **Tests:**
  - `a_renegotiation_stops_the_downstream_for_one_round_trip_and_four_hundred_milliseconds` - at 20 ms and
    0.6 s each way; assert the *difference* between the two gaps is one round trip to within 50 ms, and that
    the fixed part is 0.35-0.50 s.
  - `the_upstream_stops_for_about_as_long_as_the_downstream`
  - `a_renegotiation_is_a_fraction_of_a_retrain` - the same measurement for `start_retrain`, for the record.

### Wave 3

#### WP-5: The call, end to end (L)

- **Depends on:** WP-3, WP-4.
- **Files:** `crates/datapump/tests/v90_call.rs`.
- **Description:** the tests Rory will actually judge this by. They use `Habits` with a short `climb_every` so
  they are affordable, plus one test at the shipped default.
- **Tests:**
  - `a_voip_line_that_starts_low_climbs_back_to_what_the_dil_chose` - `voip_line()`, S90 = 3, climb every 10 s:
    connects three rungs low, climbs, and ends at the rate an uncapped call over the same network came up at;
    `renegotiations() == 1`, `retrains() == 0`, and no errored blocks after the climb.
  - `a_line_that_goes_on_being_disturbed_never_climbs` - `voip_line().with_bursts(.., 1.5, 0.01, 1e-3)` from the
    start of data mode: 60 s, `renegotiations() == 0`, and the rate is still the capped one.
  - `a_line_disturbed_every_ten_seconds_never_climbs` - the quiet-patch case over the network rather than in the
    unit test, which is the one that would embarrass us live.
  - `a_climb_onto_a_rung_that_errors_is_backed_off_once_and_not_tried_again` - a network clean for the first
    forty seconds and disturbed after: climb, fall back, then 60 s more with no further climb;
    `renegotiations() == 2`.
  - `the_climb_stops_for_good_after_two_failures`
  - `a_retrain_forgets_the_burned_rungs_and_starts_low_again`
  - `a_climb_costs_less_than_one_twentieth_of_the_call` - S90 = 3 with the default 30 s interval over a
    90 s clean VoIP call: total blocks lost to renegotiation, against blocks carried.
  - `a_line_that_is_not_declared_voip_behaves_exactly_as_it_does_today` - the regression that matters: S90 = 0
    reproduces the existing rate and renegotiation counts on every network in the file.

#### WP-6: The settings (M)

- **Depends on:** WP-1 (for `Habits`).
- **Files:** `crates/at/src/registers.rs`; `crates/at/src/lib.rs`; `crates/modem/src/lib.rs`;
  `crates/gui/src/app.rs`; `crates/gui/src/remembered.rs`.
- **Description:** S90 and S91 as in §5.1; `modem::start_pump` reads them into `Habits` at the one call site;
  the checkbox, its AT line and its remembered name.
- **Tests:**
  - `s90_and_s91_are_extensions_with_their_own_ranges` (registers)
  - `s90_and_s91_read_back_and_are_restored_by_ampersand_f` (at)
  - `a_voip_line_starts_the_v90_pump_with_a_margin` (modem)
  - `the_voip_box_is_remembered_by_meaning_and_not_by_number` (gui)
  - `an_unchecked_box_asserts_zero_rather_than_saying_nothing` (gui) -- the `remembered.rs` property.

### Later, and optional

#### WP-7: Shorten TRN2d in a renegotiation (S)

- **Clauses:** 9.6.1.1.1 ("shall optionally transmit TRN2d for no more than 2000 ms"); 8.6 (the training signal
  keeps the preceding data mode's shaping and CPt's K).
- **Files:** `crates/datapump/src/v90/digital.rs`.
- **Description:** 255 ms of every renegotiation is TRN2d that a renegotiation does not need, because 8.6 says
  the equaliser is looking at the constellation it already has. Cutting it takes about 15% off the cost. **It
  only helps BinModem talking to BinModem**: against a real ISP the digital modem is theirs, and its TRN2d is
  its own business. Worth doing for the tests and for a BinModem-to-BinModem call, worth nothing live, and
  therefore last.
- **Tests:**
  - `a_renegotiation_may_send_no_trn2d_and_the_far_end_still_follows_it`
  - `phase_four_still_sends_the_full_two_thousand_and_forty_symbols`
  - the WP-4 cost test, re-run, showing where the 255 ms went.

---

## 7. Risks, and what has not been measured

1. **§4.1's rung table is a model, not a measurement.** It comes from a small Python reconstruction of `ladder`,
   `quietest` and `widest` with uniform weights instead of `average_power`'s modulus weighting, on a synthetic
   clean mu-law route with no shaping. It reproduces the 54 666 a clean line comes up at, which is reassuring
   and not a proof. WP-1's `a_rung_is_not_a_factor_on_the_noise` replaces it with the real numbers. **If it
   turns out that the factor and the rungs are proportional after all, the cap is still the right design** --
   it is exact and the factor is not -- but the paragraph arguing for it should be rewritten rather than left
   standing as false.
2. **§2.2's ladder is arithmetic, not a measurement.** WP-4 exists to check it before anything is tuned to it.
   The part most likely to be wrong is how long the analogue modem takes to complete a CP and get CP' out, which
   depends on how many distinct constellations the route needs and therefore varies by up to 34 ms at each of
   two points.
3. **The live worst case is not ours.** A real server may send up to 2000 ms of TRN2d in every renegotiation
   (9.6.1.1.1), which would make each climb 3.7 s rather than 1.6 s and would move the break-even from a minute
   to two and a half. Nothing in the design detects this. It could: measure the gap the first renegotiation
   actually took and refuse to climb again if it exceeded a threshold. Worth adding only once WP-4 exists to
   measure gaps at all.
4. **`CLIMB_QUIET` = 0.25 has not been run against a real capture.** The memory note on this line says errors
   come about half a minute apart at 54 666 and that a look has three misses at worst; three misses is exactly
   the amount that parks the climb for twenty-one seconds. On a line at the *capped* rate misses should be
   rarer, so the threshold should be comfortable -- but if it turns out that the climb never fires on Rory's
   line, this is the first constant to look at, and the symptom will be a call that stays three rungs low
   forever.
5. **The climb can only ever reach what the probe said.** If the SIP line's real problem is that the DIL is too
   *optimistic* at the moment it runs, this feature helps. If the DIL is systematically pessimistic on that
   line, this feature can do nothing about it, and the honest answer would be a longer or repeated DIL, which is
   a different note.
6. **Starting low is nearly free; climbing is not.** If the live evidence ends up showing that the climbs rarely
   pay for themselves, the right retreat is S91 = 0: keep the margin, drop the climb, and let a retrain be the
   thing that re-measures the line. That configuration should keep working on its own and is worth keeping a
   test for.
