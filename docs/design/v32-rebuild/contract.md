# V.32 / V.32bis receiver rebuild — the contract

Research for the design agent that rebuilds `v32::Receiver` (carrier, timing,
gain, equaliser, decisions) on the model of the V.34 receiver. Nothing in the
tree was changed to write this. Tree at `d7b914b` (Version 1.2.1); the V.32
source (`crates/datapump/src/v32.rs`, `v32/startup.rs`, `v32/trellis.rs`) last
changed in `cb801fc` (2026-09-16). Every figure below that is not a file:line
was measured on 2026-09-22 with release builds; section 7 says how.

Sections 1 and 2 are the contract: what callers depend on and must keep
working. Section 1.4 lists what is free to redesign. Sections 3–7 are the
supporting facts: the start-up as the receiver sees it, the tests, the
recordings, the verified state of the 2026-09-21 analysis, and new
measurements.

---

## 0. The findings that should shape the design

1. **The arrival-phase ladder is the carrier loop, not the equaliser.** The
   2026-09-21 analysis blames the symbol-spaced blind CMA equaliser (modulus
   1.0, absolute 0.25 handover) for 9600+ failing at most arrival phases. That
   is refuted (section 7, experiments A and B). A delay of N samples rotates the
   received 1800 Hz carrier by 40.5°·N. With that rotation cancelled, **all 8
   arrival phases lock at every rate with the equaliser unchanged**, and a pure
   carrier rotation of ≥ 15° at a fixed arrival kills 9600T through 14400T. The
   decision-directed loop cannot pull in more than about 10–15° on a dense
   constellation. The modulus values in the analysis are arithmetically right
   (1.320 / 1.310 / 1.381 / 1.343); they just do not decide the outcome.
2. **In a real call the start-up hides that, and hides most of `before.md`.**
   After 0.7 s at 4800 (which the start-up always provides), the bare receiver
   locks at all 8 arrival phases at every rate and holds ±7 Hz and ±200 ppm
   everywhere except 14 400 at −7 Hz (experiments C and E). Whole two-`Modem`
   calls reach 14 400 at every delay tried (D). The V.32 rows of `before.md`
   measure a cold start at the data rate, which no call performs.
3. **What does break a connected call** (experiments F and H):
   - **one dropped or repeated sample.** On a direct line with no echo, a single
     slip at 14 400 forces a retrain, and `stop_offering`
     (`startup.rs:1953-1978`) then drops 14 400 and 12 000 for the rest of the
     call. A one-sample step is a 40.5° carrier jump plus 0.15 symbol of timing,
     and nothing holds, rewinds or re-reads.
   - **a clock difference on a cable** (5, 20 or 100 ppm between what is written
     and what is read back). This keeps a cable call retraining, or not connected
     at the end. On a direct line the same ppm is harmless. The cause is the echo
     canceller being frozen after the first TRN (`startup.rs:2230-2240`), so a
     drifting full-strength echo is never followed.

   This is the likeliest reading of "unstable over a sound-card loopback". The
   ideal simulated cable (`soundcard_loop.rs`) passes; `modem-loop` counts
   dropped input samples and underruns (`modem-loop.rs:166-168`), which are
   exactly these slips.
4. **The receiver is also the start-up's rate-signal reader.** Every R1/R2/R3/E
   is read from `Receiver::take_bits()` at 2 bits a symbol
   (`startup.rs:1286-1302`). The rate switch comes from those bits
   (`startup.rs:1315-1321`), and so does the retrain decision
   (`startup.rs:1878-1881`). A new receiver that trains well but hands up bits
   late, or in a different form, breaks the start-up rather than the data.

---

## 1. The contract

### 1.1 Callers outside the `v32` module (must keep compiling and meaning the same)

**The modem crate** reaches V.32 only through `v32::startup::Modem`
(`crates/modem/src/lib.rs:96`). It is built at `lib.rs:2386-2414`, with
`fs = self.fs`, which is 16 000 everywhere it is instantiated (`gui/src/live.rs:34`,
`modem-loop.rs:33`, all tests). Every call it makes:

| modem crate | calls `startup::Modem::` | meaning it relies on |
|---|---|---|
| `Pump::step` `lib.rs:115` | `step(line) -> f64` (`startup.rs:2186`) | one sample in, one out, every sample of the call |
| `Pump::status` `lib.rs:133-138` | `status()` (`startup.rs:2324`) | `Negotiating` / `Retraining` / `Connected(rate)` / `Failed` |
| `round_trip_ms` `lib.rs:181`, `round_trip_symbols` `lib.rs:1005-1010` | `round_trip()` (`startup.rs:2359`) | symbols |
| `Pump::carrier` `lib.rs:198` | `carrier()` (`startup.rs:2355`) → `Receiver::carrier()` | **while `Connected`, false ends the call** (`carry_data`, `lib.rs:2003-2006`: `!pump.carrier() && !retraining` → `end_call(CarrierLost)`) |
| `take_bits` `lib.rs:209` (and `2038`, `1468`, discard at connect `1644`) | `take_bits()` (`startup.rs:2409`) | descrambled data bits, in order, no framing |
| `send_bits` `lib.rs:220`, `pending_bits` `lib.rs:246` | `send_bits`, `pending_bits` (`startup.rs:2414-2422`) | transmitter queue; the modem tops it up below 64 bits (`lib.rs:2055`) |
| `accepts_bits` `lib.rs:239` | — | always `true` for V.32 |
| `constellation_point` `lib.rs:261` | `constellation_point()` (`startup.rs:2424`) | last equalised symbol / √10, unit RMS |
| `residual_error` `lib.rs:307` | `residual_error()` (`startup.rs:2430`) | mean \|decision error\|, unit-RMS units |
| `reception` `lib.rs:324` | `residual_error() / point_spacing()` | fraction of the closest-point distance |
| `states` `lib.rs:339-344`, `shape` `lib.rs:358-373` | `status()`, `coding()` (`startup.rs:2315`) + `v32::constellation_size` (`v32.rs:231`) | 4 until `Connected`, then 16/32/64/128 |
| `standard` `lib.rs:404-407` | `status()` | "V.32bis" if connected above 9600 |
| `phase` `lib.rs:423` | `phase()` (`startup.rs:2328`) | start-up state names (tests match on them, see 4.2) |
| `echo_return_loss` `lib.rs:1013-1018` | `echo_return_loss()` (`startup.rs:2375`) | dB, frozen at end of training |
| `echo_return_loss_now` `lib.rs:1022-1027` | `echo_return_loss_now()` (`startup.rs:2391`) | dB, live |
| `constellation_peak` `lib.rs:1087-1095` | `status()`, `coding()` + `v32::constellation_peak` (`v32.rs:241`) | scope scale |
| `reflection` `lib.rs:1172-1177` | `reflection()` (`startup.rs:2310`) | echo-finder result |
| `ask_for_retrain` `lib.rs:1895`, `retrain` `lib.rs:1965` | `ask_for_retrain()` (`startup.rs:2333`) | a clause 7 retrain (V.32 has no renegotiation) |
| `retrains` `lib.rs:1976` | `retrains()` (`startup.rs:2338`) | count |

The modem crate also relies on two things without calling anything for them:

- **At `Connected` it throws away everything the pump holds** (`pump.take_bits()`
  at `lib.rs:1644`). Bits handed up during training are expected and discarded.
- **`Retraining` must last until the start-up gives up or reconnects.**
  `watch_for_retrain` (`lib.rs:1845-1869`) maps `Failed` during a retrain to
  NO CARRIER. The receiver's `carrier()` is ignored while `Retraining`.

**The GUI** reads only the modem crate's accessors, once per audio block:

- `gui/src/live.rs:1206-1216`: `constellation_point()` is read **every sample**
  and kept **only when it changes**, so it must change exactly once per symbol.
  `lock_sweep.rs:770-783` uses the same change detection.
- `live.rs:1450`: `reception`. `live.rs:1471`: `snr_db = −20·log10(residual_error)`.
- `live.rs:1449`: `echo_return_loss_now`. `live.rs:1467-1470`: `reflection`,
  `states`, `constellation_peak`, `shape`.
- `live.rs:1485`: the CD LED is `carrier()`.
- `gui/src/scopes.rs:36-58` only draws tone markers. `scopes.rs:329-331` shrinks
  by `constellation_peak`.
- `gui/src/engine.rs:65-115` labels V.32bis files "no receiver". Capture
  playback does not use the V.32 receiver.

**Other pumps share V.32 items. These must not change at all:**

- `v32::Scrambler` and `v32::Mode` (`v32.rs:92-115`, `385-424`): V.29
  (`v29.rs:243-244`, `283`, `611`), V.34 (`v34/data.rs:34`, `v34/signals.rs:9`,
  `v34/receiver.rs:53`, `v34/training.rs:51`), V.90 (`v90/analogue.rs:27`,
  `v90/digital.rs:24`, `v90/pcm.rs:37`, `v90/dil.rs:115`), and V.17's doc.
  Tests: `v34_capture.rs:36`, `v34_vector.rs:125`, `v90_vector.rs:139,209`.
- `v32::trellis::{Coded, AT_7200, AT_9600, AT_12000, AT_14400}`, and
  `Coded::point`/`size`: V.17 (`v17.rs:20`, `49-55`, `152`, `182`).
- The `datapump` crate root re-exports `V32Mode`, `V32Rx`, `V32Tx`
  (`datapump/src/lib.rs:22`). Nothing in the workspace uses the aliases.
- `dsp::Equalizer` and `dsp::Gardner` are shared with V.22bis, V.27ter and V.29
  (and fax). V.34 imports neither. Change them only additively (plan.md rule 3).
  A V.32 rebuild is better off with its own types, copied from V.34's as
  plan.md rule 9 requires.

### 1.2 Integration tests that drive `v32::Receiver` directly (the bare-receiver contract)

These construct a `Receiver` with no `Startup`, no training and no hold:

| test | uses | requires |
|---|---|---|
| `v32_loopback.rs:14-30` `loopback` (5 tests) | `Receiver::new(mode.peer(), FS)`, `feed`, `take_bytes` | blind acquisition at 4800 from 128 bytes of `0x55`, both directions |
| `v32_loopback.rs:91` `the_signal_may_arrive_at_any_moment` | same | **4800 locks at every one of 8 arrival phases, blind** |
| `v32_loopback.rs:195-235` `through_a_hybrid` (2 tests) | `feed` on a `dsp::EchoCanceller` residual | 4800 through an echo 8 dB above the far end, cancelled, blind |
| `v32_loopback.rs:245` `nine_thousand_six_hundred_with_trellis_coding` | `set_data_rate(9600)`, `set_coding(Trellis)` before any signal | **9600T acquired blind from data**, arrival phase 0, 256-byte lead-in |
| `v32_bits.rs:35-45`, `v32_both_ends.rs:68-78,101-107` (ignored, capture) | `new`, `feed`, `take_bits` | a 4800 bit stream off a recording, never held |
| `v32_startup.rs:13-65` (5 tests) | `endpoints(role, FS)` (`startup.rs:330`) + `Startup::step(line, &mut tx, &mut rx)` | the start-up with **no `Modem`**: no echo canceller, and `set_adapting` is never called, so the receiver adapts on everything |
| `lock_sweep.rs:734-738`, `805-808`, `753-773` | `new(Mode::Call)`, `set_data_rate`, `set_coding`, `feed`, `take_bits`, `carrier`, `constellation_point` | the measurement harness (section 6.3) |

So the new receiver still has to acquire **blind, from scrambled data, with no
training sequence**: 4800 at any arrival phase, and 9600T at arrival phase 0.
A design that only trains on TRN needs a blind path as well, or these tests
fail. plan.md rule 4 forbids editing them.

### 1.3 The seam between `Startup`/`Modem` and `Receiver` (inside `v32`; may change, but this is what it needs today)

| call | where, when | what it touches |
|---|---|---|
| `Receiver::new(mode, fs)` (`v32.rs:893-926`) | `endpoints` `startup.rs:330-333`, once per `Modem` | `mode` is **this** end; the descrambler takes `mode.peer()` (`v32.rs:912`) |
| `rx.feed(line)` (`v32.rs:961-988`) | `Startup::step` `startup.rs:1253`, **every sample of the whole call**, start-up and data alike | the whole chain |
| `rx.take_bits()` (`v32.rs:1121-1123`) | `startup.rs:1290`, every sample while not `Connected`/`Failed`. Bits go to `RateDetector::feed` until `heard_end` (`startup.rs:1286-1302`) | the start-up reads R1/R2/R3/E from these; bits are dropped once E is heard |
| `rx.set_data_rate(r)`, `rx.set_coding(c)` (`v32.rs:935-945`, `follow` `949-959`) | (a) on an incoming E while `expecting_end` (`startup.rs:1315-1321`); (b) again at the end of `Settling` (`startup.rs:1843-1844`); (c) back to 4800/Uncoded in `start_again` (`startup.rs:2023-2024`) | `carried`, `coded`, carrier-loop `bandwidth`; resets the Viterbi only when the bit count changes. **Nothing else is reset: equaliser, carrier phase and frequency, timing and AGC all carry across** |
| `rx.set_adapting(b)` (`v32.rs:1192-1195`) | `Modem::step` `startup.rs:2235`, **every sample**, `b = !far_end_quiet()` | AGC, carrier phase and frequency updates, equaliser, `Gardner::set_adapting`. The frequency still advances the phase while held (`v32.rs:1046`) |
| `rx.residual_error()`, `rx.point_spacing()` (`v32.rs:1141-1163`) | every symbol in `Connected` (`startup.rs:1878-1881`); `stop_offering(rate, rx.residual_error())` (`startup.rs:1892`) | unsatisfactory reception: `residual > 0.25 × spacing` for 2400 symbols (`UNSATISFACTORY_GAP` `startup.rs:768`, `timing::UNSATISFACTORY` `:799`); the rate choice uses `v32::point_spacing_at(r, coding)` (`startup.rs:1961`, `v32.rs:366`) |
| `rx.carrier()` (`v32.rs:1175`) | `Modem::carrier` `startup.rs:2355` | 20 ms envelope with fixed `CARRIER_ON` 1e-3 / `CARRIER_OFF` 5.62e-4 (`v32.rs:289-290`, `915`, `964-971`) |
| `rx.constellation_point()` (`v32.rs:1134`) | `Modem::constellation_point` `startup.rs:2424` | `last_symbol / √10` |
| `rx.take_bytes()` (`v32.rs:1126-1132`) | `Modem::take_bytes` `startup.rs:2400` | **shares the bit buffer with `take_bits`** (v32_call.rs:363-365 relies on it) |
| `rx.equalizer_blind()` (`v32.rs:1171`) | `Modem::equalizer_blind` `startup.rs:2435`, which has no caller | unused |

**What the receiver is fed.** `Modem::step` (`startup.rs:2186-2255`) computes
`cleaned = echo.process(tx.last_sample(), line)` (`:2190`, `:2200`) and passes
`cleaned` to `Startup::step`. That hands the same sample to the `Listener`,
the three reversal detectors **and** `rx.feed` (`:1252-1253`). So the receiver
always sees the canceller's residual. The canceller:

- 128 near taps (8 ms) with NLMS step 0.5 (`startup.rs:2174`).
- A second run of 64 taps (4 ms) placed by `EchoFinder` in the first half of
  the first TRN, if the reflection scores ≥ 0.15 (`:2138-2164`, `:2272-2299`).
- Adapts **only** while `training_echo()` (this end sending its first TRN,
  `startup.rs:1099-1101`, gate `:2230-2240`). Frozen through the whole of data
  mode.
- Adapts again only after a retrain, because `start_again` clears `trained`
  (`:2016`).
- `EchoCanceller::reset` has no caller outside dsp's own tests.

Until the first TRN has trained it, the residual is the raw line, own echo
included. On a cable that echo is as loud as the far end.

**When the receiver is held** (`far_end_quiet`, `startup.rs:1148-1172`; the
doc comment for `far_end_finished_training` sits in the middle of
`far_end_quiet`'s, `:1103-1142`):

- this end's own first PreRoll/S/S-bar/TRN (`:1158-1164`);
- `AwaitingR1`/`AwaitingR2` once a sequence with rate-signal sync bits has been
  seen (`:1143-1146`, set at `:1332-1334`, cleared by `enter` `:2087`);
- whenever the `Listener` classifies the line as `Nothing` (`:1171`);
- never while `Connected` (`:1152-1157`).

Everything else adapts, including this end's own echo in the half-duplex
opening (section 3).

**Other `Startup` surface that tests pin** (keep, whatever the receiver
becomes): `phase()` strings (`startup.rs:1191-1217`; v32_call matches "S
pre-roll", "S bar", "awaiting R2", "rate signal", "AA", "AA to CC", "CA to AC",
"listening"); `counted()`, `round_trip()`, `reflection()`, `echo_return_loss()`,
`retrains()`, `ask_for_retrain()`; `UNSATISFACTORY_GAP` (imported by
`v32_call.rs:10`); `Listener`, `Heard`, `RateDetector`, `is_rate_signal`,
`is_end_signal`, `offered_rate` (imported by `v32_both_ends.rs:19`);
`describe_sequence` (v32_replay.rs:152).

### 1.4 Free to redesign

Everything inside `Receiver` (`v32.rs:850-1200`):

- the fixed NCO, the 121-tap 1600 Hz FIR, the RRC matched filter;
- the two-point interpolation (`:980-984`) and `Gardner` (`:903`);
- the AGC (`:908`, `:997-1004`) and the carrier loop (`:1018-1047`,
  `TRACK_SMOOTHING` `:285`, `MIN_DECISION_POWER` `:224`, `loop_bandwidth`
  `:261-277`);
- the 21-tap T-spaced `dsp::Equalizer` (`:909`, `:1049-1081`) and the slicers;
- the private helpers `nearest_state`, `nearest_point`, `signal_point`,
  `rotate`, and `bits_per_symbol`.

These accessors have no caller and may go: `Receiver::level()` (`:1197`),
`Receiver::point_spacing_at` (`:1167`), `equalizer_blind`, and
`Modem::receiver_adapting` (`startup.rs:2320`).

**Keep** `trellis::Decoder` and `Encoder`, or change only their `v32`-internal
use. The `Coded` tables and their tests are the constellation ground truth, and
`nearest`/`closest`/`peak`/`point` are public.

Unit tests that reach receiver internals, and must keep passing or move with
the code (27 run, 1 ignored, `cargo test -p datapump --lib v32`):

- `v32.rs:1206-1445` (14 tests). They use private `STATES`, `rotate`,
  `nearest_state`, `nearest_point`, `signal_point`, `WITHIN_4800`, and
  `Scrambler.register` (`:1283`).
- `trellis.rs:713-987` (8 tests).
- `startup.rs:2494-2550` (4 run, `lines_of_each_signal` ignored).

The shared `v17::tests::the_scrambler_is_the_one_v32_gives_its_calling_end`
also matches the filter.

---

## 2. The receiver today, precisely

### 2.1 Signal path (`Receiver::feed` `v32.rs:961-988`, `on_symbol` `:990-1119`)

1. Mix by a fixed 1800 Hz `Nco` (`:896`, `:962`). All carrier correction is a
   later rotation at symbol rate (`:1006-1011`).
2. `ComplexFir(fir_lowpass(1600, 121, fs))` (`:901`): 60 samples = 9 symbols
   of delay.
3. The `level` envelope (`OnePole` 20 ms, `:915`, `:964-966`) sets the
   `carrier` flag with hysteresis (`:967-971`).
4. The RRC matched filter, 0.25 roll-off over ±6 symbols (`:902`): 6 symbols
   of delay.
5. Linear interpolation between two consecutive matched-filter outputs at the
   instant `Gardner` asks for (`:974-985`). `Gardner::new(sps, 0.1)`: integral
   gain = gain/100, error normalised by a running power with α 0.02, correction
   clamped to ±sps/4 (`shaping.rs:267-354`).
6. AGC `OnePole::starting_at(10, 0.050 s, 2400)` on symbol power;
   `gain = √(10/mean)` clamped to 400 (`:908`, `:997-1004`).
7. Derotate by `phase` (turns), then a **coarse decision on the unequalised
   point** (`:1018-1025`) over the constellation in use:
   `raw = Im(p·conj(c))/max(|c|², 5)`, `track += 0.1·(raw − track)`,
   `frequency −= 1.5e-5·bw²·track` clamped ±0.02 turns/symbol (±48 Hz),
   `phase −= 0.008·bw·track`, `phase += frequency` always (`:1028-1047`).
   `bw` is 1.0 uncoded (including 9600U), otherwise `0.5·closest/closest₉₆₀₀`
   (`:270-277`).
8. Normalise by √10 and equalise: 21 T-spaced taps, CMA with modulus 1.0 and
   step 2e-3 until `error_average < 0.25`, then decision-directed with step
   4e-3 (`equalizer.rs:42-55`, `109-147`). Adapt is gated on
   `symbols > 64 && carrier && adapting` (`v32.rs:1073`), where `symbols`
   counts from construction.
9. The decision for the equaliser is the nearest point of the constellation in
   use (`:1061-1070`). `last_symbol` is the equalised, rescaled point
   (`:1082`).
10. Bits (`:1084-1118`):
    - trellis rates: `trellis::Decoder::decode` (8 states, depth 24 symbols,
      `trellis.rs:558`, `623-703`), which undoes Table 2's differential coding
      internally (`:544-551`), then `descramble` each of the `coded.bits` bits;
    - uncoded: nearest quadrant (and point, at 9600U), differential decode by
      Table 1 inverse (`CHANGE_TO_DIBIT` `:207`, `:1101-1106`), Q1 Q2 (Q3 Q4),
      descramble.

    The descrambler is the far end's polynomial (`:912`), continuous and never
    reset. The quadrant tracker is not updated in trellis mode (`:1091`).
11. Reported: `constellation_point` = `last_symbol/√10` (`:1134-1139`);
    `residual_error` = `Equalizer::error()`, the running mean \|DD error\| with
    α 0.01, only updated when `adapt` runs, so it **freezes while held or
    without carrier** (`equalizer.rs:116`); `point_spacing` from
    `carried`/`coded` (`:1153-1163`).

### 2.2 The transmitter facts the receiver meets (`v32.rs:479-841`, not to be changed)

- `Signal::Silent` and `AnswerTone` return before the carrier `Nco` **and** the
  symbol clock advance (`:808-817`). Our own far end therefore resumes after
  every silence with an arbitrary carrier phase and symbol timing. A receiver
  must not assume continuity across the far end's silences. A real far end
  free-runs instead.
- TRN resets the scrambler to zero (`:630-633`). The first 256 symbols are A or
  C by the first bit of each dibit, then Table 5 by both bits, and there is no
  differential coding (`:698-710`).
- The rate signal continues the same scrambler and is differentially encoded
  from TRN's last state (`:711-730`, `:743`).
- The data coding changes as E ends (`startup.rs:1820-1831`); trellis
  encoders start at zero (`v32.rs:570-586`).

---

## 3. The start-up as the receiver sees it

`rx` rate is 4800/Uncoded (4 points, 2 bits) from construction or
`start_again` until E. "Adapts" means `set_adapting(true)` under `Modem`.
Under `v32_startup.rs` it always adapts. S and S-bar are period-2 patterns; TRN
is fully known in advance; R and E are known only in their sync bits.

### 3.1 Calling modem (5.4.1 / Figure 4; states `startup.rs:1414-1582`, `1700-1849`)

| state (tx) | arrives at rx | rx today | known in advance |
|---|---|---|---|
| `Listening` (silent) | far answer tone, 2100 Hz (V.25, or V.8 ANSam) | adapts (Heard ≠ Nothing) on a 300 Hz baseband tone; the carrier flag comes up | — |
| `Aa` (sends A) | far AC alternation plus own AA echo | adapts on the mixture | AC = ±A alternating |
| `AaToCc` / `Cc` (sends C) | far AC, then CA, then AC; own CC echo | adapts | as above |
| `AwaitingR1` (silent) | far AC tail, far `Gap` (16T silent), **far S (256T), S-bar (16T), TRN (1280 or 4096T), R1** | adapts on S, S-bar and TRN: **this is its training window**. Held from the first sync-bit sequence until R1 is read (two identical sixteens) | S = A,B alternating; S-bar = C,D; TRN = zero-started scrambler of ones through Table 5 (the far end's polynomial); R1 sync bits |
| `PreRoll`, `SendS`, `SendSBar`, `SendTrn` (first; S for NT, then S, S-bar, TRN) | own echo only (the far end is silent after its R1) | **held**. The canceller trains in TRN | — |
| `SendRate` (R2) | silence, then far **second** S, S-bar, TRN, then R3 | adapts (`trained` is now true) through all of it, R3 included, with own R2 echo under a frozen canceller | as above |
| `SendEnd` (E, 8T) | far R3 continuing, then far E | adapts; **on far E: `set_data_rate`/`set_coding`** (`startup.rs:1315-1321`, `expecting_end` `:1363`) | E: B0-B3 = 1111 |
| `Settling` (B1, 128T at the data rate) | far B1: scrambled ones at the **new** constellation | adapts at the new rate; at 128T sets the rate again (`:1843`) and goes `Connected` | ones before scrambling |
| `Connected` | data; a retrain tone (AC > 128T) or reception > 0.25 gap for 2400T → `begin_retrain` | adapts every symbol, no gate | — |

### 3.2 Answering modem (5.4.2; states `startup.rs:1585-1697`, `1700-1849`)

| state (tx) | arrives at rx | rx today | known in advance |
|---|---|---|---|
| `AnswerTone` (7920T) | own 2100 Hz echo; the caller is silent | adapts on own echo if there is any (Heard = AnswerTone); held on a clean four-wire line | — |
| `Ac` / `Ca` / `CaToAc` / `AcAgain` | far AA, then CC, then AA, then silence; own AC/CA echo | adapts on the mixture | AA = A repeated, CC = C repeated |
| `Gap` (16T silent) | the caller silent | held (Nothing) | — |
| `SendS`, `SendSBar`, `SendTrn` (first) | own echo | **held**. The canceller trains | — |
| `SendRate` (R1, agreed = 0) | silence, then far **S** (the caller's pre-roll) | adapts once S is heard; leaves after 64T of Conditioning | S |
| `AfterR1` (silent, MT) | far S (NT + 256T), S-bar, TRN | adapts: **its training window** | S, S-bar, TRN (the caller's polynomial) |
| `AwaitingR2` (silent) | far TRN tail, R2 | adapts until the first sync-bit sequence, then held until R2 is read | R2 sync bits |
| `SendS`, `SendSBar`, `SendTrn` (second), `SendRate` (R3) | far R2 continuing (4800 data-like) under own second conditioning | adapts on R2 | — |
| R3 until far E; `SendEnd`; `Settling` | far E → rate switch (`expecting_end` `:1366-1369`); far B1 at the new rate | as for the caller | — |

### 3.3 Timing constants (`startup.rs:771-847`)

Answer tone 7920T; answer tone heard 2400T; response 64T; alternation ≥ 128T;
1800 Hz heard 64T; gap 16T; S 256T; S-bar 16T; TRN 1280T, or 4096T once the
round trip exceeds 8 ms (`training_symbols` `:1180-1187`, **both** conditioning
sequences); minimum rate signal 32T; E 8T (`:1820`); B1 128T; retrain tone
> 128T; unsatisfactory 2400T; R3 at the latest 8464T + NT; patience 60 s.

### 3.4 Retrain and renegotiation

- **Retrain (clause 7):** `begin_retrain` then `start_again`
  (`startup.rs:1907-2035`). It rebuilds every detector, puts tx and rx back to
  4800/Uncoded, and sends the caller to `Aa` and the answerer to `RetrainAc`
  (128T of AC, then `Ac`). **The receiver is not reset**: taps, phase,
  frequency, timing and AGC all survive (`start_again` touches only rate and
  coding).
- **Unsatisfactory reception** (`startup.rs:1878-1894`) first calls
  `stop_offering(rate, residual)`. That permanently drops that rate and above,
  and any rate whose `0.25 × point_spacing_at` the error does not fit.
- **Rate renegotiation (V.32bis clause 8)** is not implemented. The preamble is
  AA 56T + CC 8T (calling) or AC 56T + CA 8T (answering), then R4/R5 with the
  scrambler reset to zero and the differential encoder taken from the
  preamble's last symbol. E follows, then B1 24T at the new rate; no retrain.
  (Read in `docs/specs/text/T-REC-V.32bis-199102-I.txt:983-1040`; durations
  only, no tables.)

  `Connected` needs more than 128T of tone (`startup.rs:1867`), so a 64T
  preamble is never noticed. If it is ever added, the receiver will have to go
  data constellation → 4 points → new data constellation **without losing
  lock**, and without the canceller being retrained.

### 3.5 What the start-up leaves the receiver with at `Connected`

Taps are decision-directed from 4-point decisions (`blind` long since false).
Frequency and phase were learned at bw = 1.0 and the rate change applies the
new `bw` immediately. Gardner and AGC are as they were. The canceller is frozen
since the first TRN. Measured on perfect lines (section 7), that is enough to
lock every rate at every arrival phase.

---

## 4. Tests that exercise the V.32 receiver

All pass at `d7b914b` (release, 2026-09-22): v32_loopback 10, v32_startup 5
(+1 ignored), v32_signals 12, v32_rate_framing 3, v32_vector 3, v32_call 23
(+2 ignored), lib `v32` 27 (+1 ignored); modem `call` 56 (+2),
`soundcard_loop` 5 (+2), `file_transfer` 2.

### 4.1 `crates/datapump/tests`

Run with `cargo test -p datapump --release --test <file>`.

| file | what it actually pins | start-up? | line / phases |
|---|---|---|---|
| `v32_loopback.rs` | 4800 tx into rx both ways, 800 bytes, constants, all-ones (`:46-88`); **8 arrival phases at 4800** (`:91`); tx spectrum (`:118`); cancellation premise and 4800 through a cancelled hybrid (`:157-235`); 9600T blind (`:245`) | no (bare) | 4800: 8 phases; the rest 1 phase; one channel |
| `v32_startup.rs` | reaching `Connected(4800)` on a clean line (`:74`), who speaks first (`:93`), the round trip within ±4 symbols of 80 behind a hybrid (`:139`), following a far end that restarts (`:209`), giving up (`:248`) | yes, `Startup` without `Modem` (no canceller, never held) | fixed delays; 4800 only; no data checked |
| `v32_call.rs` | two `Modem`s. 4800 over a hybrid (echo 0.251, far 0.1) (`:44`); canceller > 15 dB (`:69`); data both ways (`:87`); no echo (`:129`); a 20 ms line with the far hybrid found and > 20 dB (`:249-308`); **9600U data** (`:399`); 4800 fallback (`:424`); round trips of 40-400 ms (`:477`); cable echo at unity (`:496`, `:531`); late far end (`:594`); ANSam (`:667`); silence (`:702`); **9600T data** (`:721`); retrain followed and data after, 4800-9600 (`:781`, `:901`, `:1031`); S margin (`:1094`); R2 timeout (`:1131`); **14 400 given up on the hybrid line within ≤ 3 retrains** (`:1195`); NT = MT + 64 (`:1263`) | yes | no noise, no carrier or clock offset, no slips. **Every delay used (320, 1000, 1600, 2400, 3200 samples) is a whole number of symbols**, and all but 1000 are whole carrier cycles (1000 is a half cycle): effectively one arrival phase |
| `v32_signals.rs`, `v32_rate_framing.rs` | transmitter waveforms and rate tables | — | no receiver |
| `v32_vector.rs` | tones and spectrum of `tests/vectors/v32bis-14400.wav` by correlation | — | never demodulates (its module comment "cannot yet demodulate" is stale) |
| `v32_replay.rs`, `v32_both_ends.rs`, `v32_bits.rs`, `v32_reversals.rs`, `v32_who.rs` | capture probes, all `#[ignore]` | replay: yes | section 5 |
| `lock_sweep.rs` | measurement, all `#[ignore]` | no | section 6.3 |

### 4.2 `crates/modem/tests`

Run with `cargo test -p modem --release --test <file>`. All go through the whole
start-up. `Pair` is a direct line with a one-sample delay each way, no echo
and no attenuation (`call.rs:44-48`).

- `call.rs`:
  - `ms_chooses_which_modulation_the_call_uses` (`:340`): V32 → 9600;
  - `the_two_v32_carriers_are_two_ceilings` (`:377`): V32B → 14 400, 128 states;
  - `a_v34_caller_meets_a_v32bis_modem_on_v32bis` (`:534`);
  - `a_v32_call_carries_data_both_ways` (`:653`);
  - `a_scope_can_see_what_the_modem_is_doing` (`:785`): 4 states before
    `Connected`, 128TCM after, the point's radius in (0.1, 2.0),
    **residual < 0.3 on a clean line**;
  - `every_modulation_goes_out_at_the_same_level` (`:898`);
  - rate ceilings (`:1123`, `:1145`);
  - `error_control_comes_up_on_every_pump_that_can_carry_it` (`:1436`);
  - `a_retrain_is_not_mistaken_for_the_far_end_hanging_up` (`:1616`): no NO
    CARRIER during a V32B retrain.
- `soundcard_loop.rs`: two modems summed onto one simulated cable at 0.45,
  crossing 700 samples, exact samples (no clock offset, no slips).
  `a_v32_call_goes_through_a_sound_card_loopback` (`:94`) asserts **14 400** and
  a greeting; `:140` places the reflection within 16 samples of the crossing;
  `:174` covers a 64-sample crossing; `sweep_the_crossing` (`:244`) is ignored.
- `file_transfer.rs` (V32 at 9600), `ppp_call.rs:203` (ping over V32),
  `crates/at/tests/session.rs:371-409,658` (AT only).

---

## 5. Real recordings with V.32 traffic

Classified by running `v32_both_ends` over every WAV: a `Listener` plus a
4800 `Receiver` on each channel, with a `RateDetector`. Channel 0 is what
arrived, channel 1 is what we sent; all are 2 channels at 16 kHz. Channel 0
reads nothing in most calls, because our own echo on it is uncancelled.

Test binaries run with the crate directory as cwd, so **`V32_CAPTURE` must be
an absolute path**.

| file | when | what | use |
|---|---|---|---|
| `captures/live-1788841427.wav` (152 s) | 2026-09-08 14:23 | **V.32 4800, the good real call** (dialup.world). We offered 4800 (R 0000010100010001 ×584). Frames: 879 rx good, 95 bad | replay connects at 20.44 s and decodes 77 225 octets; residual 0.03-0.05, error bursts at 21.2, 21.7, 22.0 s. `V32_FROM=10.6 V32_OFFER=4800` |
| `captures/live-1788855280.wav` (74 s) | 09-08 18:14 | **V.32 9600 trellis on a real line.** We offered 9600 + B8 (V.32 table). Frames: 107 good, **1672 bad** | replay: `Connected(9600)` at 24.22 s, then the turn walks 0 → −20° in 0.8 s, SNR 17 → 4 dB, retrain 1 s in. `V32_FROM=14.4 V32_OFFER=9600` |
| `captures/live-1788855465.wav`, `live-1788856805.wav` | 09-08 18:17, 18:40 | the same 9600T offer; frames 0 good / 214 and 242 bad | same |
| `captures/live-1788841688.wav` | 09-08 14:28 | we offered 9600 uncoded; the far end stopped transmitting after connecting (commit ef6981b); frames 0 good / 30 bad | 9600U against a V.32bis far end |
| `captures/live-1788832249.wav`, `live-1788832373.wav` | 09-08 11:50, 11:52 | early V.32 attempts (offers 4800+9600, 4800) | start-up |
| `dist/captures/live-1789618687.wav` (287 s) | 09-17 14:18 | **V.32bis** from 152 s: far R1 is the full V.32bis offer, we send S/TRN/R2 (0000111111111001 ×1498) | replay reads R1, never reads R3, restarts, `Failed` at 212 s. `V32_FROM=152 V32_OFFER=all` |
| `dist/captures/live-1790031662.wav` (40 s) | **today 09:01** | **V.32bis**: far R1 (full offer) at 13.9 s, then the far end returns to AC for 7.9 s and starts again (twice) | replay reads R1, sends R2, no R3 is coming, restarts at 20.69 and 32.3 s. A start-up (NT/MT) failure against a real far end, not a receiver one. `V32_FROM=9.0 V32_OFFER=all` |
| `tests/vectors/v32bis-14400.wav` (18 s, **mono**, two modems summed, Conexant ~2005) | — | a full V.32bis call. `v32_who.rs` attributes the **answering modem's first S/S-bar/TRN/R1 to 4.25-7.00 s and the calling modem's to 7.75-10.50 s**, each alone on the line; carriers 1800.000 and 1798.125 Hz | **the only real-modem TRN in the tree**: a TRN-trained acquisition can be checked against it, as V.34 checked its TRN against two real modems (`v34/receiver.rs:13-21`). Not yet tried. Data from 10.75 s is two modems summed and cannot be separated |
| `WAV/ALL Old Modem Sounds (300 baud to 56K).wav` | — | 44.1 kHz stereo compilation; the source of the vectors (`tests/vectors/README.md`) | — |

Everything else in `dist/captures` (Sep 16-22) is V.34/V.90/V.22bis. A few
show stray lone "E" readings of runs of ones; none has a V.32 rate signal. The
earlier docs' `live-1788836496`, `…758849`, `…758957` and `…760125` are not in
the tree.

**Replay limitation (matters for any receiver work on captures).**
`v32_replay` (`:118-176`) feeds channel 0 to a `Modem` whose echo canceller
references the *replayed* transmitter, not what was really sent. The recorded
echo in channel 0 is therefore never cancelled: echo return loss is 0.2-5 dB in
every replay above. Results after the first TRN are indicative only where the
real echo was small. A faithful replay has to use channel 1 as the canceller's
reference, which `Modem::step(line)` cannot be given today.

Env vars:

- `v32_replay`: `V32_CAPTURE`, `V32_OFFER` (`4800` | `9600` | `all`,
  default 4800+9600), `V32_CHANNEL` (0 = replay as the calling end, 1 = as the
  answering end), `V32_FROM` (seconds; start after V.8, see the comment at
  `:87-103`).
- `v32_both_ends`: `V32_CAPTURE`.
- `v32_bits`: `V32_CAPTURE`, `V32_CHANNEL`, `V32_BITS` (output path).
- `v32_who` / `v32_reversals`: `V32_CAPTURE`, `V32_CHANNEL`.

Example:

```text
V32_CAPTURE=F:/dialupmodem2/captures/live-1788855280.wav V32_FROM=14.4 V32_OFFER=9600 \
  cargo test -p datapump --release --test v32_replay -- --ignored --nocapture
```

---

## 6. The 2026-09-21 analysis, checked against the code

All in `docs/design/slow-modes/`, committed in `ea1c3fc`. The V.32 source is
unchanged since, so its line numbers still match `v32.rs` / `startup.rs`
exactly. The references into `v34/receiver.rs` have drifted by a few lines:
confidence gate `:928`, `:947`; loss `:936`; `rewind` `:997`; gains `:130-131`.

### 6.1 Claims that hold

- **v32.md §1-§4 (structure, numbers, timing table, round-trip overheads).**
  Loop constants match `v32.rs:901-924`, `1018-1047`, `equalizer.rs:49-50`
  (the doc says 50-51), `:116`, `:121`, `shaping.rs:276-351`. The 166-symbol
  overhead is 128 + 2·88/6.67 + 12, with the 88-sample reversal latency from
  `tone.rs:211-245`.
- **§2 silent transmitter.** `v32.rs:808-817` stops the carrier *and* the
  symbol clock.
- **§3.3 carrier-loop analysis.** The loop is as written; experiment A confirms
  the pull-in is tiny at dense constellations.
- **§4 "left behind" and §6 "what survives a retrain".**
- **§5.** `RateDetector::agreement()` has **no caller** (grep).
- **§7.** Canceller: μ = 0.5 (`startup.rs:2174`) against `echo.rs:136-138`'s
  "around a tenth"; frozen after the first TRN.
- **§8.2.** 9600U runs at bw = 1.0 (`v32.rs:270-277`, `353-358`), and the
  comment at `:274-275` is contradicted by `:369-373`.
- **§8.3.** No confidence gate, loss detector or rewind (`v32.rs:1031-1042`,
  `1073-1081`).
- **§8.4.** The canceller never re-adapts. Measured: fatal on a drifting cable
  (experiment F).
- **§8.5.** No clause 8. `Connected` needs > 128T of tone (`startup.rs:1867`).
  One correction: the modem crate's `ask_for_retrain` (now `lib.rs:1882-1906`)
  documents a clause 7 retrain for V.32 and a renegotiation for V.34/V.90.
- **§8.6-§8.12** (R2 not filtered by R1, `startup.rs:1717`; CA→AC parity; no
  cleardown signal; absolute thresholds; equaliser adapting through S/S-bar;
  4096T second TRN; one-sided receive delay).
- **evidence §2.3**, all numbers:
  - interpolation mid-point error 1 − cos(π·1500/16000) = 4.3 %;
  - 21 taps = ±4.17 ms;
  - modulus table recomputed exactly from `trellis.rs`: 1.320 / 1.320 /
    1.310 / 1.381 / 1.343, CMA scale 0.870 / 0.874 / 0.851 / 0.863;
  - 0.25 handover against half-spacings 0.224 / 0.154 / 0.110;
  - `v29.rs` computes the modulus per rate.
- **evidence §3.2.** No reference-trained path exists; `adapt` only ever gets
  the slicer's decision (`v32.rs:1074`); the 5.2.2 S→S-bar time reference is
  unused.
- **before.md.** Reproduced: `one_mode_clean` gives exactly 8/8, 6/8, 7/8,
  3/8, 3/8, 2/8 for 4800, 9600, 7200T, 9600T, 12000T, 14400T.
- **plan.md `v32-front-end`.** The silence failure is the receiver adapting
  through silence: held with `set_adapting(false)` it is 0 errors at 300 ms,
  adapting it is BER 0.5 (experiment G). The only `set_adapting` caller is
  `startup.rs:2235`.

### 6.2 Claims that do not hold, or need qualifying

| claim | where | verdict |
|---|---|---|
| "9600 and above fails at most arrival phases" is caused by the symbol-spaced CMA equaliser (modulus 1.0, 0.25 handover); "How I know it is the equaliser and not … the carrier loop: 4800 is perfect" | evidence §3.1; plan `v32-equaliser` ("best available explanation … follows the table rung for rung"); plan wave 5 | **Refuted.** An arrival delay of N samples is also a 40.5°·N carrier rotation. The 4800 control does not separate the two, because 4800 has 45° of rotation margin. With the rotation cancelled, 8/8 at every rate with the equaliser untouched; with rotation alone, failure beyond 10-15° (section 7 A, B). Retargeting the modulus and handover should not be expected to lift the ladder |
| before.md's V.32 carrier and clock rows (e.g. 14400T fails ±1 Hz and ±50 ppm), and plan `v32-carrier`'s "85 s / 51 s to pull in" | before.md, plan | **Cold-start artefacts.** The harness starts the receiver blind at the data rate. After a 0.7 s 4800 pre-roll: ±7 Hz and ±200 ppm pass at every rate except 14400T at −7 Hz (section 7 E) |
| "round trip 1.1 s" failures | before.md | an artefact of the bare harness never holding the receiver; `Modem` holds it on `Heard::Nothing` (G) |
| "Arrival phase: V.22bis only. Not V.32." | evidence §1.4 | wrong: `v32_loopback.rs:91` sweeps 8 phases at 4800 |
| "dist/captures holds 31 live calls, none V.32"; "v32_replay … cannot be run" | evidence §1.1, §1.3; v32.md Q3 | out of date and incomplete: top-level `captures/` holds seven Sep 8 V.32 calls, and `dist/captures` now holds two V.32bis calls (section 5). The harness runs, with absolute paths |
| "the start-up hides the fault … would stop hiding it the moment anything caused an equaliser reset" | evidence §3.1 | half right: the start-up hides it (C, D), but the mechanism is carrier phase acquired at 4 points, not taps |
| "V.32 is the mode with … the shortest equaliser" | evidence §2.3 | true, but on the lines tested span is not what fails |

### 6.3 What `lock_sweep` measures for V.32, and how to run only V.32

A bare `v32::Transmitter` (`Mode::Answer`) at the data rate **from the first
symbol**, with random bits and no start-up, TRN or reversals
(`lock_sweep.rs:590-604`), feeds a bare `v32::Receiver` (`Mode::Call`) with
the rate preset, never held and with no echo canceller (`:734-738`). In
between is the impairment chain: clock by resampling, carrier by a
Hilbert-transform shift, echo, level step and dropout, and AWGN in 3.1 kHz
(`:862-1024`). It runs 0.70 s of lead-in and 0.80 s of payload (`:380`).

It reports:

- lock time;
- slicer SNR against the transmitted alphabet;
- margin;
- SER / BER after alignment;
- first error;
- carrier lost;
- slips.

Each cell is the median of 3 seeds, over 8 arrival phases (`:1336-1341`). It
is a **cold-start blind-acquisition** benchmark at the data rate. A call never
does that; a V.32 retrain or renegotiation would not either.

There is no V.32-only filter:

- `every_slow_mode_against_every_impairment` (`:1402`) runs all 15 modes and
  **overwrites `docs/design/slow-modes/before.md`** (`:1716-1718`); plan rule
  5's `before-48b9087.md` copy does not exist.
- For V.32 alone without editing `MODES` (`:1234-1250`), use the two probes,
  which only print:

```text
cargo test -p datapump --release --test lock_sweep one_mode_clean -- --ignored --nocapture 2>&1 | grep "V\.32"
cargo test -p datapump --release --test lock_sweep two_things_worth_a_closer_look -- --ignored --nocapture
```

The first takes 1.6 s. The second runs the V.32 silence, hole, phase and
offset tables.

---

## 7. Measurements made for this document

A scratch crate outside the repository, with path dependencies on `datapump`
and `dsp` and only the public API, 16 kHz, release. Scoring: find a 256-bit
window of the sent stream in the received one (≤ 40 errors), then count errors
over the last half; "ok" is BER < 1e-3. The carrier rotation is a 255-tap
Hamming Hilbert transformer (as in `lock_sweep`'s `Shifter`), always applied,
with its own 127-sample delay netted out of the phase.

**A. Arrival 0, carrier rotated by θ, bare receiver, cold start at the rate.**

| rate | 0° | 5° | 10° | 15° | 20° | 25° | 30° | 35° | 40° | 45° |
|---|---|---|---|---|---|---|---|---|---|---|
| 4800 | ok | ok | ok | ok | ok | ok | ok | ok | ok | ok |
| 9600U | ok | ok | ok | ok | – | – | – | – | – | – |
| 7200T | ok | ok | ok | ok | – | – | ok | ok | ok | ok |
| 9600T | ok | ok | ok | – | – | – | – | – | – | – |
| 12000T | ok | ok | ok | – | ok | – | – | – | – | – |
| 14400T | ok | ok | ok | – | – | – | – | – | – | – |

**B. Arrival delay N = 0..7 samples, raw vs with the 40.5°·N carrier rotation
cancelled.**

| rate | raw | cancelled |
|---|---|---|
| 4800 | oooooooo | oooooooo |
| 9600U | ooooooXo | oooooooo |
| 7200T | oooXXoXo | oooooooo |
| 9600T | oXoXXoXo | oooooooo |
| 12000T | oXoXXoXX | oooooooo |
| 14400T | oXoXXXXX | oooooooo |

**C. 0.7 s at 4800 first (tx and rx switched together), then the rate, raw
arrival.** 8/8 at every rate. Reception (residual / spacing) on a perfect line:
0.01 (4800), 0.03 (9600U, 7200T), 0.05 (9600T), 0.07 (12000T), **0.12
(14400T)**. That is half of the 0.25 retrain threshold before any impairment,
which is the 27 dB interpolation ceiling showing.

**D. Whole `startup::Modem` calls offering 4800-14 400, 30 s.** Every call
connected at 14 400 with 0 retrains.

| line | delays (samples) | reception | payload BER |
|---|---|---|---|
| direct | 320-327 | 0.10-0.15 | 0 |
| hybrid (echo 0.251, far 0.1) | 320-327 | **0.20-0.22** | 0 in 6 of 8; 0.0054 and 0.0017 in two |
| cable (sum at 0.45) | 700-707 | 0.10-0.14 | 0 |

**E. After the 0.7 s pre-roll, arrival 0.** ±7 / ±3 / ±1 Hz and ±200 / ±50 ppm
are all ok at every rate except **14400T at −7 Hz**.

**F. Cable call (whole modems, 40 s, offering up to 14 400), fault on the wire
3 s into data.**

| fault | outcome |
|---|---|
| clean | 14 400, BER 0 |
| **one sample dropped** | **2 retrains, ends at 4800**, ERL now 3 dB |
| one sample repeated | same as dropped |
| wire clock 5 ppm | 4 retrains, not connected at the end |
| 20 ppm | not connected at the end |
| 100 ppm | not connected at the end, ERL now −5 dB |

**H. The same faults on a direct line with no echo at all.**

| fault | outcome |
|---|---|
| 20 ppm | 14 400 throughout, 0 retrains |
| 100 ppm | 14 400 throughout, 0 retrains |
| **one sample dropped or repeated** | **retrain; 9600 for the rest of the call** |

So ppm on a cable is the frozen canceller, and a slip is the receiver (plus the
canceller, on a cable) and then `stop_offering`.

**G. 4800 cold start after silence.**

| silence | adapting through it | held (`set_adapting(false)` until the carrier) |
|---|---|---|
| 0 ms | BER 0 | BER 0 |
| 150 ms | BER 0 | BER 0 |
| 300 ms | **BER 0.5** | BER 0 |
| 600 ms | BER 0 | BER 0 |

The adapting column is phase-dependent, as before.md says.

**M. CMA constants from `trellis.rs`.** R₂ = 1.320 (7200T, 9600U), 1.310
(9600T), 1.381 (12000T), 1.343 (14400T); peak/rms 1.342, 1.304, 1.528, 1.440.

The crux of the probe, for reproduction:

```rust
// rotate the passband by phi: y = x(n-127)·cos(phi) − H{x}(n)·sin(phi)
// net carrier rotation of an N-sample arrival delay: −2π·1800·(N+127)/16000 + phi
let phi = w * (127.0 + n as f64); // w = 2π·1800/16000: cancels the arrival rotation
rx.feed(rot.process(if i >= n { far[i - n] } else { 0.0 }));
```

---

## 8. What follows for the design (facts, not a design)

- **Acquire carrier phase and frequency, timing, gain and taps on the
  4-point part of the start-up.** S and S-bar (with the 5.2.2 reversal as a
  time reference) and the fully known TRN are there in every start-up and every
  retrain, and they are what currently saves the call. The dense-constellation
  loop only has to *track*. Checking TRN against
  `tests/vectors/v32bis-14400.wav` is the real-modem check.
- **Keep a blind path** that works at 4800 from any arrival phase and at 9600T
  at arrival phase 0 (section 1.2 tests).
- **Survive a one-sample slip and a 20 ms hole without a retrain.** V.34's
  model: gate on `squared < 0.25·d²_min`, windowed loss, rewind, re-read at
  fractional offsets (`v34/receiver.rs:36-44`, `928-1010`; API `:454-642`).
  The alternative today is a retrain plus a permanent `stop_offering`.
- **The canceller is outside `Receiver` but inside the problem.** On the
  user's cable any drift between the output and input clocks defeats the
  frozen canceller whatever the receiver does. Tracking in data mode is a
  `Modem`-level change (`startup.rs:2226-2240`), and a replay that references
  channel 1 is needed to measure it on captures.
- **Keep the semantics of `take_bits` during the start-up,
  `set_data_rate`/`set_coding` at E, `residual_error` and `point_spacing`
  (retrain and `stop_offering` are tuned to them), `carrier()` (NO CARRIER in
  data), and `constellation_point` changing once a symbol.** Or change them
  together with `Startup`, keeping every test in section 4 unedited.
