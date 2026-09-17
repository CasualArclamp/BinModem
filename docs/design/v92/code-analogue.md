# The V.90 analogue modem as built, and where V.92 plugs in

A code map of the analogue side of `crates/datapump/src/v90/`, written for the V.92 work.
It covers:

- the modules and what each does;
- the state machines, with file:line references;
- the upstream transmit path, which today is V.34;
- clocks, deadlines and slip handling;
- rate renegotiation, retrain and cleardown;
- the constants V.92 will touch;
- concrete extension points for:
  - PCM upstream;
  - short phase 2;
  - the new phase 3 and phase 4 sequences;
  - rate renegotiation and fast parameter exchange;
  - modem-on-hold;
- the risks;
- the house style, so new code reads like the old.

All line numbers are for the tree as read on 2026-09-18 (HEAD `a7678a0`, plus untracked `docs/design/v92/`).

**Tags used below:**

- **[code]**: what the code does now.
- **[V.92]**: a requirement, cited by clause.
  - Timings quoted from 9.5.2, 9.6.2, 9.8.2, 9.9.2 and 9.10.1 were checked against rendered pages 55, 56, 59, 63, 64, 65 and 66 of `T-REC-V.92-200011-I.pdf`.
  - Everything else V.92-specific comes from the sibling digests in this folder (`spec-*.md`), which render their own pages.
- **[proposal]**: a design suggestion, not settled.

---

## 0. What was read

| File | Lines | Read |
|---|---|---|
| `v90/mod.rs` | 258 | all |
| `v90/analogue.rs` | 1931 | all |
| `v90/pcm.rs` | 1291 | all |
| `v90/dil.rs` | 638 | all |
| `v90/encoder.rs` | 542 | all |
| `v90/modulus.rs` | 263 | all |
| `v90/sign.rs` | 772 | all |
| `v90/ucode.rs` | 258 | all |
| `v90/carrier.rs` | 169 | all |
| `v90/startup.rs` | 492 | all |
| `v90/sequences.rs` | 821 | 1-600 (all builders and parsers) |
| `v90/digital.rs` | 1122 | outline, 456-585, 1110-1122 |
| `v90/network.rs` | 414 | header and API |
| `v90/server.rs` | 133 | header |
| `v34/qam.rs` | 411 | all (the upstream `Transmitter`) |
| `v34/signals.rs` | 349 | all (`Sender`, S, S-bar, PP, TRN, `Size`) |
| `v34/data.rs` | 1269 | 1-390 (data-mode `Encoder`, `Params`) |
| `v34/mp.rs` | 309 | 1-200 |
| `v34/startup.rs` | 469 | all |
| `v34/phase2.rs` | 1269 | 1-1010 |
| `v34/info.rs` | 688 | outline, 440-514 |
| `v34/training.rs` | 2173 | 180-300 (`RetrainWatch`) |
| `datapump/tests/v90_call.rs` | | 1-200 (the test harness) |
| `modem/src/lib.rs` | | the `Pump::V90` dispatch |

---

## 1. Module map

| Module | Role | Key items (file:line) |
|---|---|---|
| `v90/mod.rs` | Shared V.90 numbers. | `POWER_LIMITS` (Table 15/V.90) :45; `power_limit` :51; `training_codeword` (UINFO) :63; `INTERVALS` = 6 :71; `SYMBOL_RATE` :74; `RATE_STEP` = (8000, 6) :83; `SLOWEST`/`FASTEST` :86-87; `K_RANGE`, `S_RANGE`, `D_RANGE` :90-94; `table_2_has` :102; `largest_k` :109; `rate_for` :117; `bits_for` :122 |
| `v90/startup.rs` | Owns V.34's start-up with V.90's phase 2 inside it, and hands over to `analogue::Modem` once phase 2 settles on V.90. Handles retrains and the fall back to V.34. | `V90_RETRAINS` = 2 :23; `Status` :27; `Analogue` :41; `Analogue::step` :216; `back_to_phase2` :124; `Digital` :266 (the test server) |
| `v90/analogue.rs` | The analogue modem from the end of INFO1a on. It covers phases 3 and 4, data mode, rate renegotiation and cleardown. Upstream is V.34 symbols; downstream is interpreted here from what `pcm` decides. | `Status` :151; `Settings` :162; `Up` :199; `Source` :217; `JdReader` :416; `RWatch` :487; `RSeen` :499; `Levels` :560; `Trust` :616; `find_place` :637; `Frames` :670; `Stage` :721; `Modem` :735; `Modem::new` :825; `Modem::step` :1199; `stage_step` :1256; `heard` :1294; `symbol` :1330; `phase4_symbol` :1647 |
| `v90/pcm.rs` | The downstream receiver. It interpolates at two samples per symbol and hunts for Sd and its reversal. It solves the equaliser by least squares on TRN1d, then tracks with NLMS, decision feedback and a timing loop. It detects slips (`Lost`/`Found`) and emits `Symbol`s. | `BAUD` :43; `REACH` :50; `FEEDBACK` :61; `Slicer` :133; `Heard` :178; `Symbol` :196; `Receiver` :310; `hunt` :429; `set_slicer` :445; `set_frame_offset` :454; `expect_from` :475; `feed` :519; `finish_training` :712; `symbol` :879; `watch` :975; `hold_centre` :1017 |
| `v90/dil.rs` | Designs the DIL this modem asks for, accumulates what arrives, and chooses CP and CPt. | `LOUDEST` :43; `SPACING` :49; `design` :71; `Route` :88; `Analysis` :100; `Route::noise_at` :178; `average_power` :239; `Choice` :277; `ROOM` :341; `fits_with_room` :344; `choose` :405; `explain` :427; `cp_for` :445 |
| `v90/sequences.rs` | Framed sequences: Jd, the DIL descriptor, CP, and MP as the digital modem sends it. Includes the 17-ones sync plus start bits plus CRC framing, and a generic `Finder<T>`. | `frame` :27; `unframe` :42; `data_rate` :76; `Jd` :93; `JD_BITS` :108; `JD_PRIME_BITS` :111; `Descriptor` :150; `Mask` :272; `Cp` :277; `Finder<T>` (private) :449; `DescriptorFinder` :531; `CpFinder` :547; `mp_bits` :570 |
| `v90/encoder.rs` | V.90 5.4 end to end. The analogue side uses only `Decoder` and `Mapping`. | `Frame` :24; `Mapping` :46; `Mapping::from_cp` :78; `Mapping::for_renegotiation` :89; `Encoder` :134; `Decoder` :236; `Decoder::could_have_sent` :256; `Decoder::frame` :267 |
| `v90/modulus.rs` | V.90 5.4.3 and 5.4.4: the modulus encoder over 6 intervals (u128), and constellation labelling with the loudest point first. | `Moduli` :29; `fits` :37; `capacity` :46; `encode` :62; `decode` :85; `Constellation` :102 |
| `v90/sign.rs` | V.90 5.4.5 and 5.4.6: sign coding, the shaper, and the analogue-side `SignDecoder`, which needs no knowledge of the chosen inversions. | `Redundancy` :29; `Differential` :80; `Shaper` :355; `SignDecoder` :474 |
| `v90/ucode.rs` | Table 1: Ucode to linear value, octet and level. | `UCODES` :22; `Law` :30; `linear` :42; `level` :58; `octet` :68; `nearest` :111 |
| `v90/carrier.rs` | Watches data mode for a far end that has gone quiet (a level drop), since PCM silence decodes as valid zeros. | `Watch` :40; `feed` :78; `gone` :100; constants :17-36 |
| `v90/digital.rs` | The digital modem (server), run at `FS` = 8000 (:42). It exists so the analogue modem has something written from the text to talk to. | `Stage` :459; `Modem` :474; `upstream_rate` :1119 |
| `v90/network.rs` | A simulated route: codec reconstruction, noise, robbed bit, pad, gain control, clock skew, and 20 ms jitter-buffer slips. | `Network` :40; `with_clock` :132; `with_slips` :176; `with_slip_at` :192; `down` :234; `up` :305 |
| `v34/qam.rs` | **The upstream transmitter V.90 uses.** Root-raised-cosine pulse (10% roll-off, SPAN 20), carrier, pre-emphasis, power reduction. Symbols are pulled lazily. | `Band` :32; `Transmitter` :173; `Transmitter::lookahead` :243 (= 20 symbols); `next_sample` :248 |
| `v34/signals.rs` | V.34 phase 3/4 symbols: S, S-bar, PP, TRN, and the differential J/MP/E sender. | `S_SYMBOLS` = 128, `S_BAR_SYMBOLS` = 16 :12-13; `PP_SYMBOLS` :17; `E_BITS` = 20 :23; `Size` :45; `Sender` :101 |
| `v34/data.rs` | V.34 data-mode encoder (shell mapper, trellis, precoder, non-linear), used for V.90's upstream. | `Params` :44; `Encoder` :266; `next_symbol` :312; `mapping_frames` :306 |
| `v34/phase2.rs` | Phase 2. V.90's part is `Pcm` :64, `v90()` :378, `v90_retrain()` :394, `again()` :403, `decline_pcm()` :415, the INFO1a choice at :685-707, and `settle_pcm` :957. | |
| `v34/startup.rs` | V.34's start-up wrapper, which the V.90 `Analogue` owns. | `with_phase2` :55; `restart_phase2` :183; `decline_pcm` :178; `step` :191 |
| `v34/training.rs` | `RetrainWatch` :234, used by V.90 for the digital modem's tone B. | |

**Upper layers.**

- `modem/src/lib.rs` wraps `v90::startup::Analogue` as `Pump::V90` (:105, created at :2283).
- It calls these methods on it:
  - `step`, `status`, `round_trip`, `carrier`, `take_bits`, `send_bits`, `accepts_bits`, `pending_bits`;
  - `v90().pair()` and `v90().points()` for the scope (:266, :349), and `is_v90()` (:381, :411);
  - `v34()`, `phase()`, `renegotiate()` (:1851), `retrain()` (:1871), `renegotiations()` and `retrains()` (:1890).
- The live line runs at 16 kHz (`gui/src/live.rs:34`, `gui/src/main.rs:56`). The tests use `FS = 16_000.0`.

---

## 2. How phase 2 hands over

[code] The sequence, from call placement to data mode:

1. `startup::Analogue::new` (:55) builds `v34::Modem::with_phase2(phase2::Modem::v90(Pcm::Analogue, fs), fs)`.
   - The analogue modem is V.34's **answer** side in phase 2 (`Pcm::role`, phase2.rs:75). V.90 8.2.3.1 has the analogue modem send INFO at 2400 Hz.
2. Phase 2 runs inside `v34::Modem::step` (v34/startup.rs:212).
   - When INFO1c arrives in `AnswerInfo1`, phase2.rs:685-707 decides what to send.
   - If the far INFO0 was an INFO0d and PCM has not been declined, it sends `settle_pcm` (Table 10 INFO1a, :957). UINFO comes from `v90::training_codeword`; the upstream symbol rate is the best allowed of 3000/3200/3429 by INFO1d projection.
   - Otherwise it sends V.34's INFO1a.
3. Phase 2 reports `Status::Done` once INFO1a has left the modulator (phase2.rs:939-944, `!self.tx.is_sending()`).
4. `v34::Modem::step` then tries `settings()` (v34/startup.rs:234). That needs a V.34 `info1a()`, which is `None` in V.90 mode, so **V.34 training never starts**.
5. `startup::Analogue::step` (:245-258) sees `p2.status() == Done && v34.training().is_none()` with `info1a_pcm`, `far_info0d` and `info1c` all present. It then:
   - builds `analogue::Settings::new(&server, &info1d, &asked, round_trip, ours_wide = true)`;
   - sets `settings.v34_receive = p2.v34_receive_rate()` (the V.34 rate the probe projects, used later to refuse a poor V.90);
   - creates `analogue::Modem::new(settings, fs)`.
6. From the next sample on, `Analogue::step` (:217-244) drives only the V.90 modem.

`Settings` (analogue.rs:162-195) carries:

| Field | Meaning |
|---|---|
| `law` | the companding law |
| `uinfo` | UINFO |
| `server` | the INFO0d |
| `upstream: Band` | from INFO1a's symbol rate plus INFO1d's carrier choice for that rate |
| `pre_emphasis` | from INFO1d |
| `power_reduction` | INFO1d `min_power_reduction` |
| `round_trip` | seconds |
| `wide` | 1664-point both ends |
| `v34_receive` | the V.34 rate the probe projects |

**V.92 impact:**

- short phase 2 has **no INFO1d** and no probe;
- a Table 18 INFO1a (PCM upstream) has no QAM symbol rate or carrier;
- so `Settings::new` cannot be reused as is.

See 11.2.

---

## 3. `analogue::Modem`: anatomy

### 3.1 Fields, grouped (analogue.rs:735-821)

| Group | Fields |
|---|---|
| Clock and outcome | `fs`, `now` (line samples since INFO1a ended), `stage`, `status` |
| Deadlines | `deadline: Option<(u64, &'static str)>` (the one active watchdog), `start_deadline` (B1d, whole start-up), `sd_deadline` |
| Upstream | `tx: v34::qam::Transmitter`, `source: Source`, `sending_s` |
| Downstream receiver | `rx: pcm::Receiver`, `last`, `last_symbol`, `heard_any`, `lost_since` |
| Phase 3 | `descriptor` (our DIL); `jd: JdReader`; `far_jd` |
| DIL reading | `dil` (symbols); `dil_trusted`; `dil_base`; `dil_interval`; `dil_read`; `dil_left`; `dil_recent`; `dil_lost`; `dil_moved`; `before_dil`; `dil_found_late`; `jd_gone`; `analysis` |
| Choice | `route`, `choice: Option<Choice>`, `in_use: Option<Cp>` (the data CP being run) |
| Phase 4 and data | `r_watch`, `trn2d_from`, `frames: Option<Frames>`, `received`, `upstream_rate`, `downstream_rate`, `r_moved` |
| Watchers | `retrain_watch` (tone B), `wants_retrain`, `far_end: carrier::Watch`, `far_end_went` |
| Renegotiation | `rd_watch`, `renegotiating`, `initiated`, `awaiting_turn`, `clearing`, `renegotiations` |
| Margin | `least_gap`, `margin_at`, `short`, `worse` |

### 3.2 Public API

The API the start-up and the tests depend on (analogue.rs:825-1196):

- **Lifecycle:** `new`, `step(line) -> f64`, `status`, `phase() -> &'static str`.
- **Data:** `take_bits`, `send_bits`, `pending_bits`.
- **Control:**
  - `take_retrain` / `start_retrain`;
  - `renegotiate(most) -> bool`;
  - `clear_down() -> bool`;
  - `carrier` and `far_end_went`.
- **Introspection:**
  - `receiver`, `pair`, `points`, `last_symbol`;
  - `dil_start`, `descriptor`, `far_jd`, `route`, `choice`, `far_mp`;
  - `dil_found_late`, `dil_moved`, `dil_progress`, `frames_moved`;
  - `renegotiations`, `settings`.

`pair()` (:960) is the consecutive-sample scope that Rory chose for the PCM panel. Keep it in any V.92 modem.

---

## 4. The state machines

There are three machines running in lockstep:

- the modem `Stage`;
- the upstream `Source` (`Up`);
- the receiver's own `pcm::Stage`.

All three advance in `Modem::step` (:1199). The order within one sample is:

1. `now += 1`, then `rx.feed(line)` (:1200-1201).
2. The far-end carrier watch, in data mode or a renegotiation from it (:1202-1214). If the far end has gone: `far_end_went = true` and `cleared_down()`.
3. `retrain_watch.feed` in every stage except `Finished`. Tone B sets `wants_retrain` (:1217-1219).
4. Lost for more than 3 s in `Data` (not renegotiating) sets `wants_retrain` (:1223-1233).
5. Drain `rx.heard()` into `heard()`, unless the stage is `Finished` (:1234-1238).
6. Deadline check, only while `status == Running`: a renegotiation turns expiry into a retrain; otherwise `fail(why)` (:1239-1250).
7. `stage_step()` (:1256).
8. `tx.next_sample(|| source.next())`. The transmitter pulls symbols as its pulse needs them (:1252-1253).

### 4.1 `Stage` (analogue.rs:721-731)

| Stage | Entered | Leaves on | Action on leaving | Watchdog while in it | Lines |
|---|---|---|---|---|---|
| `SendTraining` | `new` | `source.up == Up::Ja` (checked in `stage_step`) | `rx.hunt(uinfo)`; `sd_deadline = SD_WAIT + 2·RTD` from now | `start_deadline` (15 s + 5·RTD) | :842, :1258-1264 |
| `AwaitSd` | above; or `Untrained` while Sd can still come | `Heard::Reversal` | `source.change(Silence)`; deadline 4.5 s + RTD "no Jd" | `sd_deadline` | :1296-1302, :1309-1316 |
| `Training` | Reversal | `Heard::Trained` goes to `AwaitJd`. `Heard::Untrained` goes back to `AwaitSd` (Ja again, re-hunt) if `now < sd_deadline`, else `fail`. | | Jd deadline | :1304-1317 |
| `AwaitJd` | Trained | a whole Jd read by `JdReader` | `far_jd`; `source.size` from Jd bit 47; `send_s()` if not already; deadline = `start_deadline` | Jd deadline | :1335-1352 |
| `AwaitJdPrime` | Jd | J'd seen (`JdReader::feed` true), giving `begin_dil(raw + 1, [])`. If Jd has stopped for `JD_GONE` symbols without a J'd: switch to the `Free` slicer and call `find_dil_start` every 64 symbols, giving `begin_dil(first, already)`. | | `start_deadline` | :1353-1376 |
| `Dil` | `begin_dil` | `dil_left == 0` in `count_dil`, then `finish_dil` | choose CP and CPt; S 128, S-bar 16, then CPt; `Free` slicer. `fail` if no choice, or if the V.90 rate is below `v34_receive`. | `start_deadline` | :1394-1416, :1537-1552, :1611-1645 |
| `Phase4` | `finish_dil` | `phase4_symbol`: B1d's 48 frames done, then `Data` | | `start_deadline` | :1647-1801 |
| `Data` | B1d done | `cleared_down`, `fail` | | none; `start_deadline` is cleared at Ed (:1772) | :1757-1762 |
| `Finished` | `fail`, `cleared_down` | never | | | :1171-1196 |

`Status::Connected` is set in `stage_step` (:1278-1284). It needs:

- `frames.data` (B1d finished);
- `source.up == Up::Data` (our E is out);
- `status == Running` and not renegotiating.

On entry it clears the deadline and arms the margin watch.

**Phase names** (`phase()`, :939-949) are the strings the GUI and the replay show:

- "V.90 phase 3: training"
- "…: Jd"
- "…: DIL"
- "V.90 phase 4"
- "V.90 rate renegotiation"
- "V.90 data"
- "V.90 finished"

### 4.2 Upstream `Source` (`Up`, analogue.rs:199-412)

This is a pull-driven symbol generator:

- `start(up)` resets `count`, `silent` and `queue`, and runs per-segment initialisation (:269-285).
- `change(up)` only records `pending` (:287). Each segment decides at which symbol it honours `pending`.

| `Up` | Produces | Ends when | Honours `pending` | Notes |
|---|---|---|---|---|
| `Silence` | 0 | `silent >= hold` **and** `pending` is set | after `hold` symbols | `hold` is the 70 ms of 9.3.2.1 less `Transmitter::lookahead()` (:828-829) |
| `S` | `signals::s(n)`, 4-point grid | `s_length = Some(n)`: after n symbols, then `SBar`. `None`: when `pending` is set **and** `count` is even, then `pending` (normally `SBar`). | at a pair boundary only | the "S until J'd" mode (`send_s`, :1385) |
| `SBar` | `s_bar(n)` | after 16 symbols, then `after_s_bar` (`Pp`, `Silence` or `Cp`) | no | |
| `Pp` | `signals::pp(n)` | after 288 symbols, then `Trn` | no | |
| `Trn` | `sender.trn(Four)` | after `trn_length` (1.0 s × baud), then `Ja` | no | `start(Trn)` restarts the scrambler |
| `Ja` | the DIL descriptor bits, differential, 2 bits a symbol | never on its own | **immediately**; V.90 8.3.1 allows Ja to stop mid-descriptor | repeats the descriptor |
| `Cp` | CP bits at `size` (4 or 16 points) | at sequence end: count `cps` and `acknowledged`; then `pending`, else swap in `next_cp`, else repeat | at a sequence boundary only | the first `start(Cp)` restarts the scrambler (8.5.2), guarded by `restarted` |
| `E` | 20 ones (`E_BITS`) | queue empty, then `Data` if `encoder` is set, else `Silence` | no | |
| `Data` | `UpstreamEncoder::next_symbol` | never | immediately | B1 is scrambled ones while `mapping_frames() < framing.p` (one V.34 data frame) |

- The scrambler is `Sender::new(Mode::Answer)`, which is GPA (:246).
- `Source.size` is set from Jd bit 47 (:1346), or from Jd bit 48 in a renegotiation (:1114-1124).

**Phase 3 upstream as sent** (module header, :4-7):

- `Silence(70 ms) S(128) S̄(16) PP(288) TRN(1 s) Ja…`
- then silence;
- then `S…` (from 4.1 s − RTD into TRN1d, or at Jd), then `S̄(16)` at J'd;
- then silence during the DIL;
- then `S(128) S̄(16) CPt… CP… CP'… E B1 data`.

### 4.3 The receiver's own stages (`pcm::Stage`, pcm.rs:222-230)

| Stage | What happens | Lines |
|---|---|---|
| `Idle` | Makes nothing. Entered from `new`, `idle()`, or a failed training. | |
| `Hunting(Hunt)` | Correlates half-symbol samples with those 12 half-symbols earlier. After 8 Sd periods above 0.7 it arms; a fall through zero then below −0.5 emits `Reversal{at}`. | :234-296 |
| `Collecting{start, base, tries}` | Waits until the TRN1d stretch has arrived: `start + 2·base + reach + SEARCH + 2·TRAIN_TO + REACH + 1` half-samples. `start` is the reversal plus 2·48 (S-bar-d). | :572-581 |
| `finish_training` | Coarse re-search of ±320 symbols on retries, alignment search of ±12 half-symbols, then up to 8 passes of drift estimate and resample (below 0.5 ppm), then a least-squares solve with a second pass for robbed-bit neighbours. Success needs SNR ≥ 9 dB (`KNOWN_ENOUGH`); then `Trained`, `Heard::Trained`, and the `Binary(UINFO)` slicer. On failure it retries the next 1600-symbol stretch, up to 3 tries, then `Heard::Untrained`. | :712-767 |
| `Trained` | `symbols()` emits `Heard::Symbol` for every symbol whose samples are in. | :582, :587-597 |

In `Trained`, each `symbol()` (:879-957):

1. equalises, with feedback taking decided codewords off;
2. decides with the current `Slicer`, using `interval = (index + frame_offset) % 6`;
3. on a `Known` slicer, corrects decisions the route moved onto a neighbour codeword;
4. runs `watch()` for slip detection, which may emit `Lost`/`Found`;
5. while not lost, runs the NLMS update, the timing loop and the error average;
6. calls `hold_centre()` every 16 decided symbols.

`Symbol.index` includes the frame offset; `Symbol.raw` does not.

---

## 5. The upstream transmit path

[code] The whole path:

```
analogue::Modem::step
  └─ tx.next_sample(|| source.next())          v34/qam.rs:248
        per line sample: RRC pulse over 41 symbols (SPAN 20 each side), carrier mix,
        optional pre-emphasis FIR (63 taps), gain 10^(-reduction/20)
        whenever frac passes p: history.push(next())    ← symbol pulled here
  Source::next                                  analogue.rs:297
    S / S̄ / PP / TRN / Ja / CP / E via v34::signals::Sender (GPA, differential)
    Data via v34::data::Encoder (from prepare_upstream, analogue.rs:1848)
```

- **Level:** a unit-power symbol leaves at 0.707 RMS, less the power reduction (qam.rs:167-171). `grid()` (analogue.rs:211) scales V.34 grid points by `receiver::unit(size)`.
- **Latency:** `Transmitter::lookahead()` is 20 symbols (qam.rs:243). A symbol is decided about 20 symbols (5.8 to 6.3 ms) before it reaches the line. `new` subtracts this from the 70 ms silence (:829). Nothing else compensates for it.
- **Data-mode parameters** (`prepare_upstream`, :1848-1868):
  - `digital::upstream_rate(&choice.data, &mp)` (digital.rs:1119) picks the highest rate enabled in both CP's mask (shifted by one) and MP's mask, capped by MP's answer-to-call rate;
  - `Framing::new(upstream.rate, rate, false, mp.expanded_shaping)`;
  - trellis and precoding from MP, non-linear from MP;
  - `Mode::Answer`.
- **CP's upstream mask:** `finish_cp` (:1835) sets `upstream_rates` to 0x1fff if `wide`, else 0x07ff. It sets `lookahead` to 0 and the law bit.
- **Rate the start-up reports:** `Status::Connected { downstream, upstream }`. `startup::Status::Connected { transmit: upstream, receive: downstream }` (:133-135).

**What V.92 PCM upstream replaces:**

- all of `Source`;
- `Transmitter` (QAM at 2400·a/c Bd with a carrier);
- `Sender`;
- `UpstreamEncoder`;
- `prepare_upstream`.

The replacement is an 8000 Bd baseband PAM transmitter slaved to the network clock (V.92 6.2). The V.90 path must survive unchanged for V.92's "V.34 upstream" mode, which V.92 runs through V.90's phases 3 and 4. See 11.3.

---

## 6. The downstream receive path, as the analogue modem uses it

- **Jd and J'd** (`JdReader`, :416-476):
  - It descrambles signs with GPC (`Mode::Call`).
  - It switches to differential decoding at the first zero after TRN1d's ones, re-decoding that same symbol.
  - It keeps the last 84 bits (`JD_BITS + JD_PRIME_BITS`) and records every CRC-valid Jd as `last = (index + 1, jd)`.
  - It reports J'd when the last 12 bits are zero **and** the 24 bits before them equal the tail of the last whole Jd. That survives a slip cut into the final Jd (test :1924).
  - **It does not check Jd bit 47.** See section 13.
- **DIL design** (`dil::design`, :71):
  - Every codeword up to 0.3 of full scale except UINFO.
  - Order: three sweeps with step 3 (0,3,6,…; 1,4,…; 2,5,…).
  - Each codeword gets a 36-symbol segment: one frame of UINFO references, then five frames of the codeword.
  - Signs are GPA-scrambled ones.
  - Under μ-law that is 98 segments, 3528 symbols, 0.44 s.
- **DIL reading** (:1394-1608):
  - `begin_dil` sets `dil_base`, puts interval 0 at the first DIL symbol (J'd ends on a frame boundary) and primes `rx.expect_from(next, 2 passes of levels)`.
  - Symbols go through a 96-symbol delay (`DIL_DELAY`) before `count_dil`, which feeds `Analysis` unless the symbol is `Trust::Spoiled`.
  - Each DIL position is counted once, so a slip-spoiled symbol is simply read on the next pass.
- **Choice** (`dil::choose`, :405):
  - Data mode: the fastest `drn` that Jd enables whose constellations stand `SPACING` (10) noise spreads apart within Table 15's power, then widened as far as the power allows.
  - The moduli must carry K with `ROOM` = 5/4 spare. That spare is what makes misplaced frames detectable.
  - CPt: K from 6 to 24 at twice the spacing, at least half data mode's power (8.5.2's 3 dB).
  - S is always 6: no shaping is ever requested.
- **R, R-bar, Rd** (`RWatch`, :487-556):
  - Per frame, it tests all 6 rotations of `+ + + − − −` against levels within ×0.5 to ×1.5.
  - 8 frames at rotation 0 set `heard`.
  - Rotation 3 after `heard` means R-bar: `Turned(frame_start + 24)`, the TRN2d start.
  - Any other rotation for 6 frames means `Moved(m)`, which realigns the frames (`move_frames`, :1151).
- **Frames** (`Frames`, :670-717, used in `phase4_symbol`):
  - Nearest-level decisions per interval.
  - `Decoder::frame`, then GPC descramble.
  - Before MP: an `mp::Finder` looks for MP and MP'.
  - Ed is two all-zero frames after an MP. Then comes B1d at data mode's mapping (48 frames).
  - Then data goes to `received`. Frames that looked like Rd are held back in `held`, and dropped if they turn out to be Rd.

---

## 7. Clocks, timing and deadlines

### 7.1 Clocks

[code] There are three clocks:

1. **`Modem::now`**: line samples since the modem was created (the end of INFO1a). `samples(seconds) = now + round(seconds·fs)` (:905).
2. **Receiver symbol count** (`Symbol.raw`): symbols since TRN1d's first symbol. It is locked to the digital modem's clock and used as a network-time clock. The early-S rule (:1337-1340) compares `raw` with `(JD_LATEST + S_AFTER_JD − RTD)·8000`, so that S reaches the digital modem 4.1 s after its TRN1d began.
3. **Transmitter symbol count** (`Transmitter::symbols`): on the sound-card clock. It is not tied to the downstream clock at all. For V.90 that is fine: the upstream is V.34 with the digital modem's own timing recovery.

### 7.2 Deadlines

All deadlines are `(sample, &'static str)`. There is one active slot plus two stored values.

| Watchdog | Value | Armed at | On expiry | Clause and origin |
|---|---|---|---|---|
| `start_deadline` | 15 s + 5·RTD | `new` (:900), re-armed at Jd (:1351) | `fail("no B1d from the digital modem")` | V.90 9.4.2; V.92 changes it to **20 s + 6·RTD** (9.6.2.2.1) |
| `sd_deadline` | `SD_WAIT` (2.0 s) + 2·RTD | Ja starts (:1262) | `fail("no Sd …")` | V.90 9.3.2.4 says 1500 ms. Relaxed for VoIP (live server Sd came 2 s after Ja). V.92's TR3 is 1500 ms from the start of Ja, with no RTD (9.5.2.2.1). |
| Jd | 4.5 s + RTD | Sd reversal (:1301) | `fail("no Jd …")` | V.92 TR4 is 4500 ms from the **end of Ja** (9.5.2.2.2) |
| Renegotiation | `RENEGOTIATION_ED` (5.0) + 2·RTD + 0.1 s | `begin_renegotiation` (:1099) | **retrain**, not fail (:1243-1246) | V.90 9.6.2. V.92 9.8 has no explicit timer (see `spec-renegotiation-fpe.md` Q-1). |
| Lost too long | 3 s of `rx.is_lost()` | first lost symbol | retrain (data mode only) | V.90 9.5.2.1, "may initiate a retrain" |
| Margin | every 0.25 s after a 2 s settle; 4 short looks | Connected | renegotiate one rung down, or retrain | local policy |
| Far end gone | 2 s at 20 dB under data mode's level | `carrier::Watch` in data mode | `cleared_down`, `far_end_went` | local policy |
| Tone B | 55 ms of a clear 1200 Hz tone | always | retrain | V.90 9.3.2, 9.4.2, 9.6.2 |

**Behaviour notes:**

- A deadline only fires while `status == Running`, so in `Connected` there is none.
- `Heard::Untrained` before `sd_deadline` sends the modem back to Ja and a new hunt (:1309-1316). This absorbs a false Sd (phase 2's tone is near 1333 Hz).
- Responses are not scheduled to the millisecond. Timing comes from:
  - segment lengths in symbols (S 128, S-bar 16, …);
  - `pending` taking effect at segment-defined boundaries;
  - the 20-symbol transmitter look-ahead.
- Nothing measures when a symbol leaves at the line. V.92 has several such requirements: 40 ± 1 ms reversal turnarounds in short phase 2, the ε extension, and the 100 ms + RTD CP-repeat rule. The V.92 transmitter needs a "symbol n leaves at line sample t" mapping (11.3).

---

## 8. Slip handling (VoIP jitter buffer, ±20 ms = ±160 symbols)

[code] Every mechanism:

1. **Receiver hold** (pcm.rs:975-1006).
   - The mean squared error over 32 symbols rising above 4× its settled level emits `Heard::Lost` and freezes NLMS and timing.
   - Falling below 2× emits `Found`.
   - After 4000 held symbols (0.5 s) the new error level is accepted as the line.
   - While lost, feedback uses the nearest codeword, not a possibly-moved known level (:916-924).
2. **Training retries** (pcm.rs:712-767): a later 1600-symbol TRN1d stretch, found by a ±320-symbol coarse correlation, up to 3 tries.
3. **J'd from the Jd tail** (analogue.rs:467-475).
4. **DIL found without J'd** (`find_dil_start`, :1420-1442).
   - It keeps 1200 symbols from `AwaitJdPrime` on (`DIL_START_KEPT`).
   - Every 64 symbols after Jd has stopped for 216 symbols, it fits 480-symbol windows at every start.
   - It accepts a start only if the fit error is at most 0.01 of signal power, at least 90% of signs agree, and the start is 4× better than any start more than 1 symbol away.
5. **DIL relocation** (`dil_symbol`/`find_dil`, :1514-1608).
   - On `Lost` in `Dil`, it gathers symbols. From the 288th, every 32, it tests moves of ±400 symbols on the last 128 (`fit_dil` with the trust map).
   - It accepts move 0 or a clear winner.
   - If the window is too quiet to judge move 0, it judges move 0 over everything since the loss (8cc78c3).
   - A move shifts `dil_base`, moves `rx.frame_offset`, re-primes `expect_from`, and recounts.
6. **Trust map** (`trusted_symbols`, :589-612).
   - Codewords above 0.3 of full scale are only counted (a softphone gain-control ceiling).
   - The first 12 symbols of a segment after a louder one are ignored.
7. **R realignment** (`RWatch::Moved`, :538-547; `move_frames`, :1151).
8. **Data-frame place** (:1725-1741, `find_place` :637).
   - Frames the modulus encoder could not have made (`could_have_sent`), 3 in the last 24, trigger a search.
   - The search tries the 6 shifts over the last 288 symbols and picks the one with the fewest impossible frames.
   - It relies on `ROOM`.
9. **Rd-lookalike data held back** (`held`, :1743-1751).

**Known weaknesses** (project memory `v34-phase2-tone-deadline`):

- The DIL repeats a 36-symbol pattern. When the true move lands on untrusted loud codewords, a move 36 or 72 symbols off can win.
  - A one-slip sweep failed at 17 of 260 positions.
  - Slips 5 to 35 ms before the DIL (inside J'd) fail.
- There is **no upstream slip handling at all.** The digital modem (ours or a real one) owns that. For V.92 PCM upstream this becomes fatal; see section 13.

---

## 9. Rate renegotiation, retrain and cleardown, as built

### 9.1 Rate renegotiation (V.90 9.6)

**This end initiates.** `renegotiate(most)` (:1046):

1. Only in data mode.
2. Rechooses from `route`, with every spread scaled by `worse` and only `drn`s that Jd enables at or below `most`.
3. Keeps the old CPt and runs `finish_cp`.
4. Calls `begin_renegotiation(true)` (:1086), which:
   - increments `renegotiations`;
   - sets `renegotiating`, `initiated` and `awaiting_turn`;
   - sets `status = Running`;
   - resets `rd_watch`;
   - calls `send_s_then_cp()` (:1112): S 128, S-bar 16, then CP, with `size` from Jd bit 48 and the scrambler restarted at the first CP;
   - arms the Ed deadline.
5. The digital modem's Rd is seen by `rd_watch` (levels = the loudest codeword of the in-use CP per interval, `rd_levels` :1163). On `heard` the modem calls `clamp()`, which stops delivering data and drops `held`.
6. On R-bar-d, `turned(from)` (:1131) builds `Frames` with `Mapping::for_renegotiation(CPt, in_use)`. That is CPt's constellations with the old data-mode shaping (V.90 8.6).
7. The phase 4 path then runs as before: MP leads to CP', CP' sent plus MP'/Ed heard leads to `prepare_upstream` and E; Ed leads to B1d; B1d done leads to `renegotiating = false` and `Connected` again.

**The far end initiates.** `rd_watch.heard` while not renegotiating calls `begin_renegotiation(false)`, which clamps. On R-bar-d, `turned` then sends S and CP (:1143-1146).

`watching()` (:927) keeps the far-end watch running through a renegotiation but stops learning from it.

### 9.2 Retrain (V.90 9.5)

- **Triggers:**
  - tone B;
  - lost for 3 s;
  - margin collapse (least gap below 2·rms);
  - renegotiation timeout;
  - no slower rate available;
  - `start_retrain()` from above.
- **Mechanism:** each trigger sets `wants_retrain`. `startup::Analogue::step` (:219-224) sees `take_retrain()` and calls `back_to_phase2()`:
  1. It banks the renegotiation count.
  2. It calls `v34.restart_phase2()`, which calls `phase2.again()`, which calls `v90_retrain(Pcm::Analogue, …)`, which calls `retrain(Role::Answer)`.
  3. That retrain: 70 ms of silence, then tone A, then `AnswerAwaitTone`, with INFO0 skipped.
- **Failure:** `Status::Failed` counts `failed_starts` and goes back to phase 2 as well (:230-240).
  - If the DIL produced a route but no choice (`hopeless`), or failures exceed `V90_RETRAINS` (2), it calls `decline_pcm()`. The next INFO1a is then V.34's and the call continues as V.34 (V.90 9.2.2.1.9).

### 9.3 Cleardown (V.90 9.7)

- **This end:** `clear_down()` (:1070) sets `choice.data.drn = 0` and `clearing`, then runs the renegotiation initiator path. In `stage_step`, the `Data if clearing` arm (:1265-1269) waits for `CLEARDOWN_CPS` (4) whole CPs, then `cleared_down()`.
- **Far end:** an MP with `answer_to_call == 0` calls `cleared_down()` (:1785-1789).
- **Far end gone:** the carrier watch calls `cleared_down()` (:1205-1211).
- **Reporting:** `ClearedDown` is reported upward. `modem::Pump` maps it to `Progress::Failed`.

---

## 10. Constants V.92 touches

| Constant | Where | Now | V.92 counterpart | Action |
|---|---|---|---|---|
| `SILENCE_BEFORE_S` | analogue.rs:51 | 70 ms, before S | 70 ± 5 ms before **Ru** (9.5.2.1.1) | keep the value; rename in the V.92 module (`SILENCE_BEFORE_RU`) |
| `PHASE3_TRN` | :48 | 1.0 s of 4-point TRN | TRN1u ≥ 2040T, a multiple of 12 symbols; MD start to TRN1u end ≤ RTD + 4000 ms (9.5.2.1.2) | new `TRN1U_SYMBOLS` (for example 2040 rounded up to 12, or longer within the 4 s + RTD cap) |
| `SD_WAIT` | :57 | 2.0 s + 2·RTD from Ja | TR3: 1500 ms from the start of Ja (9.5.2.2.1) | keep the relaxation, documented as a deliberate departure (as now) |
| Jd deadline (inline 4.5 + RTD) | :1301 | from the Sd reversal | TR4: 4500 ms from the end of Ja (9.5.2.2.2) | name it (`JD_WAIT`) and measure from the Ja cut |
| `JD_LATEST`, `S_AFTER_JD` | :67-68 | S before Jd, for servers that ignore RTD | V.92 Su comes **after** Jd, within 5000 ms of the A3 silence (9.5.2.1.6). The digital modem's own Su watchdog includes RTD (9.5.1.2.2). | do **not** port; see open question Q3 |
| `R_HEARD`, `R_MOVED` | :72-73 | 8 and 6 frames | Ri and R-bar-i are unchanged in form (8.6.5 points to V.90 8.6.4). Rd and Rt are 384T/24T; Rf is 4-periodic. | reuse for Ri, Rd and Rt; new watch for Rf |
| `DIL_*`, `JD_GONE`, `DIL_START_*` | :78-120 | V.90 DIL reading | DIL is unchanged (8.6.1 points to V.90 8.4.1), but now follows **Jp'** | reuse. `JD_GONE` becomes "Jp stopped". |
| `JD_TAIL` | :428 | 24 | Jp' follows Jp (same 72-bit framing) | reuse with Jp |
| `B1D_FRAMES` | :123 | 48 | the same (8.8.1 points to V.90 8.6.1) | reuse |
| `RENEGOTIATION_ED` | :127 | 5 s | no explicit timer in 9.8/9.9 | keep as local policy (Q1 of the RR digest) |
| `CLEARDOWN_CPS` | :130 | 4 CPs | `drn = 0` in CPu/CPus (9.11). CPd has `drn = 0` too. | reuse the idea with CPu |
| `MARGIN*` | :135-141 | policy | adds a choice between RR and FPE | extend |
| `PLACE_*` | :145-147 | downstream place | unchanged downstream | reuse |
| `start_deadline` (inline 15 + 5·RTD) | :900 | V.90 9.4.2 | **20 s + 6·RTD** from the end of sending INFO1a (9.6.2.2.1) | name it `START_UP_*` per version |
| `signals::S_SYMBOLS`/`S_BAR_SYMBOLS` | v34/signals.rs:12-13 | 128/16 | Ru 384/R-bar-u 24; Su 144, S-bar-u 24.5 then 24 + ε | new PCM constants |
| `signals::E_BITS` | :23 | 20 | E1u 12T (8.5.2); E2u one upstream data frame (8.7.2), +1 symbol if CPd bit 29 | new |
| `signals::Size` | :45 | 4 or 16 | TRN2u, SUVu, CPu and E2u use 4 or **8** points (Jp bits 48 and 49) | new `Trn2uSize` |
| `Jd::sixteen_in_training`/`…_renegotiation` | sequences.rs:98-100 | bits 47/48 | bit 47 is the Jd/Jp identifier; 48 is reserved (Table 21) | new V.92 `Jd` type |
| `Cp` layout | sequences.rs:277-440 | Table 14/V.90 | CPt/CPu (Table 23), CPus (Table 24), CPd (Table 30) | new types; do not reuse `Cp::from_bits` |
| `Descriptor::to_bits` fill | sequences.rs:237-241 | pad to even | V.92 Table 20 adds an upstream rate mask; pad to 12 bits; Ja has a 24-ones preamble | new or extended type |
| `data_rate`/`training_bits` | sequences.rs:76-89 | drn + 20 / drn + 8 | downstream is the same; upstream is `(drn + 17)·8000/6` and K = 2·(drn + 17) | new `upstream_rate(drn)` |
| `V90_RETRAINS` | startup.rs:23 | 2 | fallback ladder V.92 PCM up, then V.90 (V.34 up), then V.34 | per-version counter (11.9) |
| `POWER_LIMITS` | mod.rs:45 | Table 15/V.90 | still bounds the downstream constellation | reuse |
| `dil::ROOM` | dil.rs:341 | 5/4 | downstream is unchanged | reuse |
| `RetrainWatch` constants | v34/training.rs:214-220 | 55 ms, 6× | tone B means retrain **or** modem-on-hold RT (8.9.1, 9.10.1.1) | reuse; hold detection is added after it (11.8) |

---

## 11. Extension points and proposals

### 11.0 What V.92 changes for this modem

This is a pointer list; the sibling digests have the detail.

- **Upstream becomes PCM** (clause 6):
  - 8000 Bd; 12-symbol data frames, 6-symbol constellation frames, 4-symbol trellis frames;
  - 24 000 to 48 000 bit/s;
  - a 12-modulus encoder, precoder, prefilter, inverse map and 4D trellis, all parameterised by CPd.
  - See `spec-intro-transmitter.md`.
- **Short phase 2** (9.4): ranging only. The digital modem reverses first and the analogue modem answers after 40 ± 1 ms. There is no INFO1d. See `spec-phase2-procedures.md` section 6.
- **New phase 3** (9.5), in this order:
  1. Ru 384T, R-bar-u 24T, optional MD, Ru, R-bar-u;
  2. TRN1u ≥ 2040T;
  3. Ja (2-point, with a 24-ones preamble), cut at a 12-bit boundary on the Sd reversal;
  4. silence;
  5. Jd;
  6. Su 144T, S-bar-u 24.5T, Su…;
  7. Jp, then S-bar-u (24 + ε)T and circuit 107 ON;
  8. Jp', then DIL (or SCR) while TRN1u is sent;
  9. CPt… until Ri;
  10. E1u.
- **New phase 4** (9.6):
  - the analogue modem sends TRN2u (≥ 12000T or until SUVd), SUVu, CPu, SUVu', E2u, B1u;
  - the digital modem sends TRN2d, SUVd, CPd, SUVd', Ed, B1d;
  - an acknowledge and repeat rule applies (100 ms + RTD).
- **Rate renegotiation** (9.8): Ru 384T and R-bar-u 24T start on a data-frame boundary, then TRN2u (≤ 16008T; may stop after 2400T or on SUVd), then SUV, CP and E. There is an optional silent period.
- **Fast parameter exchange** (9.9): RM 384T and RM' 24T through the **data-mode chain**; then scrambler and differential reset; SUV and CP in data-mode modulation; FB1u before B1u.
- **Modem-on-hold** (8.9, 9.10): RT is tone A for the analogue modem; MH sequences use INFO modulation.
- **Retrains** (9.7) always land in full phase 2.

### 11.1 Module layout [proposal]

```
crates/datapump/src/v92/
  mod.rs          constants: UP_INTERVALS = 12, CONSTELLATION_FRAME = 6,
                  TRELLIS_FRAME = 4, UPSTREAM_RATES = 19, upstream_rate(drn),
                  up_bits(drn) = 2·(drn+17), START_UP (20 s, 6 RTD)
  sequences.rs    Jd (Table 21, bit 47 = 0), Jp (Table 22: epsilon, trn2u sizes),
                  Descriptor (Table 20: V.90 descriptor + upstream mask, 12-bit pad),
                  Cpt/Cpu (Table 23), Cpus (Table 24), Cpd (Table 30, parsed
                  sequentially), Suvu (Table 27), Suvd (Table 31), Rm tables
                  (Tables 25/26), Mh (Table 32); finders
  upstream.rs     the clause 6 chain: ModulusEncoder (12 moduli, u128, d(f)),
                  Precoder, Prefilter, InverseMap, trellis via v34::trellis::Code;
                  UpEncoder::next_level(bit source) -> f64
  up_signals.rs   Ru/R̄u, Su/S̄u, TRN1u (2-point), two-point differential sender
                  (Ja/CPt/E1u), Trn2uSender (4/8 point, Tables 28/29)
  transmit.rs     PcmTransmitter: 8 kHz levels -> line samples, clock slaved to
                  the downstream receiver, fractional symbol shifts (0.5T, εT)
  exchange.rs     the SUV/CP/E acknowledge-and-repeat machine shared by phase 4,
                  RR and FPE (spec-phase4-procedures.md 6.5)
  analogue.rs     V.92 PCM-upstream analogue modem, phase 3 on
  hold.rs         modem-on-hold (RT + MH over the V.34 DPSK INFO modem)
```

**Refactor to make first** [proposal]: move the downstream interpretation out of `v90/analogue.rs` into `v90/downstream.rs` as `pub(crate)` pieces. Both V.90 and V.92 analogue modems need it. Candidates:

- `JdReader`, parameterised with a "what counts as the sequence" predicate so V.92 can require bit 47;
- `RWatch` and `RSeen`;
- `Levels`, `levels_for`, `least_gap`, `nearest`, `slicer_for`;
- `Trust` and `trusted_symbols`;
- `find_place` and `Frames`, with the MP finder made pluggable;
- a `DilReader` struct owning `dil*`, `before_dil`, `analysis` and the functions:
  - `begin_dil`, `find_dil_start`, `fit_dil`, `dil_levels`, `dil_symbol`, `count_dil`, `find_dil`.

Each of those functions takes `&mut pcm::Receiver` as an argument instead of reaching into `self.rx`.

This is a behaviour-preserving move. It must be done with the slip tests green (`v90_call.rs`, and the sweep harness described in memory `v34-phase2-tone-deadline`), and committed on its own.

### 11.2 Start-up and hand-over

- **`v90/startup.rs::Analogue`** [proposal]:
  - Replace `v90: Option<analogue::Modem>` (:44) with `pcm: Option<Pcm>`, where `enum Pcm { V90(analogue::Modem), V92(v92::analogue::Modem) }`.
  - Implement `status`, `phase`, `take_bits`, `send_bits`, `pending_bits`, `carrier`, `retrain`, `renegotiate` and `clear_down` once on `Pcm`.
  - Keep `is_v90()` meaning "a PCM mode is running", or add `is_pcm()`. `modem/src/lib.rs:266, 349, 381, 411` call `v90()` for the scope, so provide a `pair()` and `points()` passthrough on `Analogue` and move the modem crate to those.
- **Hand-over branch** (:249-258): choose by which INFO1a went.
  - Table 18 (PCM upstream) builds `v92::analogue::Modem::new(v92::analogue::Settings::new(&server, info1d: Option<&Info1c>, &asked92, rtd), fs)`.
  - Table 10 (V.90), or Table 19 (V.34 upstream during short phase 2), builds `analogue::Modem`.
  - For Table 19 there is no INFO1d, so `analogue::Settings::new` must accept `Option<&Info1c>`. Without probe data, fall back to the INFO1a symbol rate with the low carrier and pre-emphasis 0.
  - `v34_receive` is unknown after short phase 2. Set it to 0 so the "V.34 carries more" refusal (:1622-1628) never fires.
- **`v34/phase2.rs`:**
  - `Pcm::Analogue` (:67) becomes `Analogue(V92Wish)`, recording whether this end offers V.92, short phase 2 and PCM upstream.
  - `info0_bits` (:420) sets the Table 16 capability bits.
  - `heard(Info0d)` (:664) records the far end's V.92 bits.
  - The INFO1a choice (:689-702) gains `settle_pcm92` (Table 18) beside `settle_pcm`.
  - `again()` (:403) must keep the V.92 flags but force **full** phase 2 (9.7).
  - Short phase 2 as new `Stage`s: follow `spec-phase2-procedures.md` 11.3.
- **`v34/info.rs`:**
  - add `Info1aPcm92` (Table 18) and `Info1aV34Short` (Table 19);
  - add V.92 fields to `Info0d` and `Info0`;
  - add `Info::Info1aPcm92` and `Info::Mh` variants (:505).
  - `Info1aPcm::from_bits` already rejects bits 34:36 = 6 (:490-493), so Table 18 cannot be misread as Table 10. Keep a test for that.
- **`startup::Status`** (:27): add `OnHold` (and possibly `Reconnecting`) for 11.8. Map them in `modem/src/lib.rs:159`.

### 11.3 PCM upstream transmission [proposal]

**Source.** Mirror `Source`/`Up` with a PCM version whose `next()` returns a level (`f64`, in units of LU) instead of a `Complex`:

```rust
/// What goes up, at the network's 8000 symbols a second.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Up {
    Silence,
    Ru, RuBar,            // 8.5.5, 384T and 24T
    Md,                   // 8.5.3 -> 10.1.3.5/V.34
    Trn1u,                // 8.5.7, scrambler reset
    Ja,                   // 8.5.4, 24 ones then descriptors
    Su, SuBar,            // 8.5.6; S̄u carries a fractional extension
    Cpt, E1u,             // 8.5.1, 8.5.2
    Trn2u, Suv, Cp, E2u,  // 8.7.2 to 8.7.6
    Rm, RmPrime,          // 8.7.4, through the data chain
    Fb1u, B1u, Data,      // 8.7.7, 8.7.1, 6.4
}
```

**Framing counter.**

- `Source` keeps `frame_symbol: u64`, reset to 0 at the first symbol of the **second** TRN1u (8.5.7, 9.5.2.1.9) and again at B1u (8.7.1).
- `change()` takes effect only where the segment allows:
  - Ja at a 12-bit boundary (9.5.2.1.3). Ja is 1 bit a symbol, so that is a 12-symbol boundary.
  - Ru and RM for RR and FPE only at `frame_symbol % 12 == 0` (9.8.2.1.1, 9.9.2.1.1, and the responders).
  - SUV, CP and E only at a sequence end, each padded to 12 symbols.
- Assert (`debug_assert!`) that every sequence between the second TRN1u and B1u has a length that is a multiple of 12, so B1u lands on interval 0 without special cases. The exception is E2u's optional extra symbol (CPd bit 29), which restarts the count at B1u.

**Encoder.**

- `v92::upstream::UpEncoder::new(params: CpdParams)` is the analogue of `v34::data::Encoder::new(Params)` (:284).
- `next_level(&mut dyn FnMut() -> bool) -> f64` pulls K bits at each data-frame start and returns G·v(n).
- Built afresh at B1u; kept (state carried) through RM and FB1u (spec-renegotiation-fpe 9.2 pitfalls 2 and 5).
- `prepare_upstream` becomes `prepare_upstream(&Cpd)`. `upstream_rate = rate_for_up(cpd.drn)` comes directly from CPd; no mask intersection is needed beyond checking our Ja mask.

**Transmitter** (`v92::transmit::PcmTransmitter`):

- **Interface:** `next_sample(&mut self, clock: SymbolClock, next: impl FnMut() -> f64) -> f64`, the same pull shape as `qam::Transmitter::next_sample`.
- **Interpolation:** a windowed-sinc table, as `pcm::Receiver::new` builds one (pcm.rs:367-383). A reconstruction lowpass just under 4 kHz.
- **Latency:** `lookahead()` in symbols, like `Transmitter::lookahead`.
- **Level:** LU chosen for the data-mode power (Q-14 in `spec-intro-transmitter.md`).
- **Fractional shift:** `delay(fraction_of_t: f64)` adds a one-off phase shift. It is used twice: +0.5T at the first S-bar-u (24.5T, 9.5.2.1.7) and +εT at the second (ε = Jp bits 18:33 / 65536; 9.5.2.1.8).

**Clock slaving** (V.92 6.2; `spec-intro-transmitter.md` 6.2). The upstream symbol period must follow the network clock that the downstream receiver has locked to. Add to `pcm::Receiver`:

```rust
/// Line samples a symbol takes on the far clock, and where the next
/// downstream symbol falls on this end's line, smoothed: what the upstream
/// transmitter times itself by (6.2/V.92).
pub fn symbol_clock(&self) -> SymbolClock { .. }   // from half, drift, due/timed
```

Use the smoothed `drift` (the rate) rather than `due` (the phase). `due` jumps by up to a quarter half-symbol in `hold_centre` (pcm.rs:1022-1025) and on every timing-loop step. A transmit phase that followed it would jitter at the digital modem's A/D.

**Before the receiver is trained**, the transmitter must free-run at nominal 8000 Bd: Ru, TRN1u and Ja go out before the digital modem has sent anything. Switch to the slaved clock during the A3 silence (after the Sd reversal, before Su), when a phase step costs nothing. The digital modem measures the phase on Su anyway (9.5.1.1.6-7). After Jp's ε is applied, the phase must not be re-stepped.

**Two-point sender** (Ja, CPt, E1u; `spec-phase3-signals.md` 2.1-2.3):

- the GPA scrambler (`v32::Scrambler::new(Mode::Answer)`, as `dil::design` already uses);
- 0 maps to +LU and 1 to −LU (the opposite of the downstream convention);
- the differential seed is the last TRN1u symbol.

Keep this separate from `v34::signals::Sender`, which is quadrant-differential.

**Precoder and prefilter bypass.** Ru, TRN1u, Su and (by interpretation) TRN2u, SUVu, CPu and E2u in training bypass the precoder and prefilter. RM goes through them. Model this as two `Up` families: `Plain(f64)` from `up_signals`, and `Chain` from `UpEncoder`.

### 11.4 The V.92 phase 3 state machine [proposal]

`v92::analogue::Stage`, mapping each 9.5.2.1 step to what exists:

| Stage | Upstream | Leave on | Reuse |
|---|---|---|---|
| `SendTraining` | Silence 70 ms, Ru 384, R-bar-u 24, [MD, Ru, R-bar-u], TRN1u ≥ 2040, then Ja | source reaches `Ja`: `rx.hunt(uinfo)`; TR3 armed | `Stage::SendTraining` pattern (:1258) |
| `AwaitSd` | Ja | `Heard::Reversal`: cut Ja at the next 12-bit boundary, then silence; TR4 armed from the cut | `heard` (:1296); `Untrained` fallback (:1309) |
| `Training` | silence | `Heard::Trained` | As now. The first try trains on symbols 64 to 1600, inside the first 2040T of TRN1d (9.5.2.1.4). Retries after a slip use later 1600-symbol stretches, which go past 2040T. V.92 guarantees at least 2040T of TRN1d and a Jd within 4000 ms of TRN1d's start, so a retry can collide with Jd. Keep `TRAIN_TRIES` but check `base + TRAIN_TO` stays before the earliest Jd. |
| `AwaitJd` | silence | a V.92 Jd (bit 47 = 0): after an optional wait, Su 144, S-bar-u 24.5, then Su… | `JdReader` with the bit-47 predicate |
| `AwaitJp` | Su… | a Jp (bit 47 = 1): circuit 107 ON (a new `Modem::circuit_107()` getter); S-bar-u 24 + ε; store TRN2u sizes (bits 48/49) | `JdReader` |
| `AwaitJpPrime` | S-bar-u, then TRN1u (frame counter reset) | Jp' gives `begin_dil`; Jp stopped gives `find_dil_start` | as `AwaitJdPrime` (:1353-1376). Jd, Jp and Jp' are 72, 72 and 12 bits, all multiples of 6, so "Jp' ends on a frame boundary" still holds. |
| `Dil` | TRN1u | DIL pass read **and** ≥ 2040T of TRN1u sent: CPt (24 ones, then CPt repeated). Must start within 5000 ms of S-bar-u (9.5.2.1.11). | `DilReader`; `finish_dil` without the S/S-bar |
| `Cpt` | CPt… | Ri (an `RWatch` at UINFO, as :1650-1652): finish CPt, E1u, phase 4 | `RWatch`, `Heard` |
| `Phase4` | TRN2u… (see 11.5) | B1d done: `Data` | `Frames` with a SUV/CPd finder |
| `Data`, `Finished` | | | as now |

**Order constraint.** In V.90, R-bar marks TRN2d. In V.92, R-bar-i comes after our E1u (9.5.1.1.12), and TRN2d follows R-bar-i. So `RSeen::Turned(from)` still gives `trn2d_from`, but the sequence is:

1. Ri heard: stop CPt at its end, then send E1u.
2. R-bar-i: TRN2d frames begin.

**N = 0 (SCR in place of DIL).** Never requested today, since `dil::design` always asks for a DIL. Keep it that way at first, but parse Ri and R-bar-i in the order of 9.5.2.1.10 if it is ever used.

**Deadlines to name:**

- `SD_WAIT` (TR3, with the existing relaxation);
- `JD_WAIT` (TR4);
- `SU_BY` (5000 ms after the A3 silence starts; 9.5.2.1.6);
- `CPT_BY` (5000 ms after the second S-bar-u; 9.5.2.1.11);
- `START_UP` (20 s + 6·RTD).

Replace the single `deadline` slot plus `start_deadline`/`sd_deadline` with a small struct of named `Option<(u64, &'static str)>` slots checked together. Today's single slot is overwritten in several places (:1263, :1301, :1315, :1351, :1100), which is easy to get wrong once there are five overlapping timers.

### 11.5 Phase 4, and the shared exchange [proposal]

**`v92::exchange::Exchange`** implements `spec-phase4-procedures.md` 6.5.

- **State:** `sent_ack`, `got_peer_cp`, `peer_acked`, `my_cp_end`, `repeat_cp`, `need_single_cp`, plus the RTD.
- **Event methods:**
  - `peer_suv(suv)`, `peer_cp(cpd)`, `peer_e()`;
  - `sequence_started(kind) -> ack bit`;
  - `sequence_ended(kind, at)`.
- **Query:** `next(&self) -> Up` returns `Suv`, `Cp`, `E2u` or `Trn2u`.
- **Clock:** it runs on `Modem::now`. The "received after my CPu end + 100 ms + RTD" rule (9.6.2.1.3) needs the receive-completion time of each SUVd/CPd, which `phase4_symbol` knows in symbols. Convert through the receiver's `symbol_clock`, or keep a `raw -> now` map.

**Downstream side** (replacing the MP path in `phase4_symbol`, :1783-1800):

- `Frames` feeds descrambled bits into a `SuvdFinder` and a `CpdFinder` (sequential Table 30 parse; `spec-intro-transmitter.md` P-8).
- Ed (two zero frames) and B1d are unchanged.
- `Frames::new(from, Mapping::from_cp(&cpt_as_v90_mapping), …)`. V.92 CPt carries the downstream training constellations in V.90's mask form but with a different header, so `Mapping` needs a constructor from the V.92 CPt.

**Upstream side:**

- TRN2u (reset scrambler; differential seed from E1u) for ≥ 12000T, or until an SUVd arrives **and** we are ready.
- Then SUVu repeated, a single CPu on the first SUVd, acks per `Exchange`, E2u, then B1u through `UpEncoder` built from CPd.
- `Connected` is set when B1d is done and `source.up == Data`, as now.

**CPu contents.** `dil::choose` still produces the downstream constellations. Add:

- the V.92 CPu header fields;
- `drn` from the downstream ladder (unchanged);
- **no upstream mask**: in V.92 it is in Ja's descriptor, so Ja must carry the 19-bit mask of upstream rates `UpEncoder` supports.

### 11.6 Rate renegotiation and fast parameter exchange [proposal]

These are extensions of `renegotiate()`, `begin_renegotiation()`, `turned()` and `phase4_symbol`'s data branch (:1672-1701).

- **Detectors in data mode.** Run all of these at once:
  - `RWatch` for Rd (6-symbol period, data-constellation top level, as `rd_levels` :1163);
  - a new `RfWatch` (4-symbol period `+ + − −`, polarity-blind);
  - `RWatch` at CPt levels for Rt (after an RR silence);
  - `RetrainWatch` (tone B);
  - a silence detector (Ucode 0 frames) for the digital modem's RR silent period.
- **Responder rule.** Act on the X to X-bar transition, then start our Ru or RM at **our** next 12-symbol boundary (9.8.2.2.2, 9.9.2.2.2). `RSeen::Turned` already gives the transition; add `Source::change_at_frame_boundary`.
- **RR initiator** (9.8.2.1):
  1. 106 OFF.
  2. Ru 384 and R-bar-u 24 at a frame boundary.
  3. TRN2u (Jp bit 49 size) for up to 16008T, stopping early after 2400T or on SUVd. Prefer the long end on a VoIP line (`spec-renegotiation-fpe.md` 9.2 pitfall 8).
  4. SUVu with bit 32 = silence wanted.
  5. The `Exchange`.
  6. If either side set bit 32: SUVu' until SUVd'/Ed, then E2u, then TRN2u for up to 8004T or until Rt, then SUVu with bit 32 = 0, then the `Exchange` again.
- **FPE initiator** (9.9.2.1):
  1. 106 OFF.
  2. RM 384 and RM' 24 at a frame boundary, through `UpEncoder` with the modulus output overridden (Tables 25 and 26).
  3. Reset the scrambler and differential encoder.
  4. SUVu in **data-mode** modulation.
  5. The `Exchange`.
  6. E2u, FB1u (old chain), B1u (new chain).
  7. On hearing Rd instead: become the RR responder (precedence).
- **Downstream during FPE.** SUVd, CPd and Ed arrive in the *preceding data-mode* modulation, so `Frames` keeps the data mapping instead of `Mapping::for_renegotiation`.
- **API** [proposal]:
  - `renegotiate(most)` stays;
  - add `fast_exchange(most) -> bool`;
  - let `watch_margin` (:1805) prefer FPE when only the rate changes and RR when the route needs retraining (the `worse` factor has grown a lot).
- **Timeout.** Keep a local one, like `RENEGOTIATION_ED` (5 s + 2·RTD + silence allowance), whose expiry retrains (`spec-renegotiation-fpe.md` Q-1).

### 11.7 Cleardown [proposal]

- **Ours:** `clear_down()` sends `drn = 0` in a CPu or CPus, by RR or FPE (9.11). FPE is faster and needs no training. The `CLEARDOWN_CPS` idea carries over.
- **Theirs:** a CPd with `drn = 0` calls `cleared_down()`, as the MP path does (:1785).
- **Via modem-on-hold:** MHclrd and MHcda (11.8).

### 11.8 Modem-on-hold [proposal]

`v92::hold::Modem` is a small machine that owns:

- a `v34::dpsk::Transmitter` (analogue side: 2400 Hz, as phase 2's answer role);
- a `dpsk::Receiver` (1200 Hz);
- tone A generation;
- tone B and silence detection;
- `Mh` framing, a 40-bit back-to-back cycle (`spec-modem-on-hold.md` 1.2, 2.2).

**Entry points:**

1. **Local request from data mode.** This needs circuit 107 to have been asserted; in V.92 PCM mode that happens at Jp (9.10.1.1).
   - `Analogue::request_hold(Mh::Req | Mh::Clrd | Mh::Frr)` causes `pcm` to stop at a frame boundary.
   - `hold::Modem` then sends RT (tone A, ≥ 50 ms) and listens for tone B or an MH response; then sends the initiating MH repeatedly.
   - Timeout: 2 s + RTD, then retrain or disconnect (9.10.1.1, checked on page 66).
2. **Remote RT (tone B) in data mode.** Today this is `retrain_watch`, then `back_to_phase2`, then `phase2::retrain(Answer)`: 70 ms silence, tone A, then waiting for tone B and its reversal. That is exactly the responder behaviour 9.10.1.1 allows, provided phase 2's `AnswerAwaitTone` and `AnswerRanging` also **listen for an initiating MH sequence**.
   - Proposal: in `v34/phase2.rs` `heard()` (:660), accept `Info::Mh(_)` in those stages when `pcm` is V.92.
   - Surface it with a new `phase2::Status::Hold(Mh)`.
   - `startup::Analogue` then swaps to `hold::Modem`, which answers per Table 34: MHack or MHnack; MHcda; ANSam.
3. **Exits.**
   - `Status::OnHold` (ANSam for T1; Phase 1 again as answerer on QC or CM). That belongs to `crates/modem` and `crates/v8`, not datapump.
   - Fast reconnect: after 1 s of ANSam, Phase 1 as caller.
   - Disconnect (`ClearedDown`).
   - Retrain: back to phase 2.

**Why the hold machine sits beside phase 2 rather than inside `v92::analogue`:** once RT starts, nothing of the PCM data pump is used. The DPSK INFO modem and the tone detectors are phase 2's.

### 11.9 Retrains and the fallback ladder [proposal]

- Every V.92 retrain is full phase 2 (9.7). `phase2.again()` already skips INFO0; add "not short".
- Per-version failure counters in `startup::Analogue`:
  1. After `V92_RETRAINS` failed PCM-upstream start-ups, ask for V.34 upstream (Table 10, or Table 19 if short).
  2. After `V90_RETRAINS` more, `decline_pcm()` and use V.34.
- A route with no upstream PCM (for example, a transcoding path such as Crazytel) should step down quickly. Upstream PCM cannot survive a transcoder, and the digital modem will say so only by failing.

### 11.10 Tests to add [proposal]

- **Harness.** Reuse `datapump/tests/v90_call.rs`. `Call` (:32) and `FullCall` (:142) run the modems over `network::Network`. They need a V.92 digital partner (`v92::digital`) that:
  - reads PCM upstream from `Network::up` (already an 8 kHz quantised codeword per tick, network.rs:305);
  - designs CPd.
- **Unit tests,** in the existing style:
  - `UpEncoder` round trip against a decoder;
  - ε shift measured at the network side;
  - frame counter at B1u = 0;
  - Ja cut at a 12-bit boundary;
  - `JdReader` rejecting Jp as Jd;
  - `Exchange` against Figures 12, 13 and 14;
  - RR/FPE collisions (all four);
  - MH 40-bit cycle and Table 34 responses.
- **Network stresses:**
  - `with_clock(±100 ppm)`: upstream must stay locked;
  - `with_slips`: expect a retrain, not a hang;
  - `with_delay(0.75)`: RTD 1.5 s.

---

## 12. What stays as is

- All of `pcm.rs`, except a new clock accessor.
- `dil.rs`.
- `encoder::Decoder` and `Mapping`.
- `sign.rs`, `modulus.rs`, `ucode.rs`, `carrier.rs`.
- The V.90 analogue modem in full, as V.92's "V.34 upstream" mode.
- Tone B retrain detection.
- The far-end-gone watch.

---

## 13. Risks

1. **Jd/Jp confusion** [code fact].
   - `sequences::Jd::from_bits` (:131) accepts any CRC-valid 72-bit frame, and `JdReader` (:461-466) takes the latest.
   - A V.92 Jp would be read as a Jd with garbage rates and `sixteen_in_training = true` (bit 47 = 1). That would set `source.size = Sixteen` (:1346).
   - The V.92 reader must check bit 47 before anything else.
2. **Upstream clock and phase.**
   - PCM upstream needs the transmit clock at the network rate (6.2) and a phase held to a fraction of a symbol after Jp's ε.
   - Our receiver's timing loop and `hold_centre` step `due` continually, and the VoIP path adds a steady offset of about 70 to 115 ppm between sound card and network (memories `v90-live-test-pending`, `voip-jitter-slips`).
   - A transmitter that follows `due` jitters; one that ignores `drift` walks off.
   - Needs a smoothed clock and a test with `Network::with_clock`.
3. **Upstream slips are invisible and unrecoverable.**
   - A 20 ms jitter-buffer slip on the upstream moves every later symbol by 160 at the digital modem's A/D. That breaks 12-symbol framing and the digital modem's precoder model.
   - V.92 has no re-alignment short of a retrain. We cannot observe it from our side.
   - Expect PCM upstream to be far more fragile than V.90's downstream on Rory's rig. The fallback ladder (11.9) must be quick and sure.
4. **Transcoded paths.** Crazytel decodes and re-encodes G.711 (memory `crazytel-pcm-path`). Upstream PCM through that is hopeless; only V.34 upstream will work.
5. **Echo.**
   - There is no echo canceller anywhere in the V.90 analogue path (pcm.rs has none; grep finds none in v90 or v34).
   - It works because the VoIP rig is 4-wire.
   - V.92's PCM upstream shares the whole 0 to 4 kHz band with the downstream, and V.92 builds EC retraining into RR (bit 32). On a real 2-wire line both V.90 and V.92 would need a canceller in front of `pcm::Receiver`.
   - `dsp::EchoCanceller` exists (dsp/src/echo.rs:112) but is only used by V.32.
6. **Timers with RTD of 1.5 s or more.**
   - V.92's TR3 (1500 ms from Ja, no RTD) cannot be met on a 1.1 to 1.5 s RTD line (`spec-phase3-procedures.md` notes it holds only for RTD below about 900 ms). The current `SD_WAIT` relaxation must carry over.
   - The CP repeat rule (100 ms + RTD) depends on an RTD estimate. Short phase 2 gives only the digital modem's RTDEd; the analogue modem must derive its own from the reversal it answers.
   - A retrain once measured 0.31 s against a real 1.1 s (memory `crazytel-pcm-path`). A wrong RTD shortens every V.92 timer.
7. **The single `deadline` slot.** It is overwritten in five places today. V.92 adds overlapping timers (Su-by, CPt-by, TR4, start-up, the CP repeat rule). Move to named slots (11.4).
8. **The 20-symbol transmit look-ahead.**
   - Symbol decisions happen 20 symbols (V.34) or `lookahead` symbols (PCM) before the line. Mid-segment cuts ("next 12-bit boundary") must be computed on the symbol stream, not on line time.
   - Short phase 2's 40 ± 1 ms turnaround lives in phase 2's DPSK modulator. That modulator already schedules by sample (`reverse_at`, phase2.rs:629-634), so keep it there.
9. **The DIL 36-symbol alias** (8). This is unchanged in V.92, and the DIL is now preceded by Jp' instead of J'd. Slips during Jp' fail the same way that slips in J'd do today.
10. **Precedence and collisions in RR/FPE.** Four initiator signals plus tone B must be watched in parallel with data decoding. `RWatch` needs 8 frames to believe R, but R-bar lasts only 4 frames (24T). A late R lock would miss the transition. The current code handles R-bar by rotation 3 in the frame after `heard`; FPE's Rf needs the same.
11. **Modem crate coupling.** `modem/src/lib.rs` reaches through `v90()` for the scope. Changing `startup::Analogue`'s fields breaks it unless passthroughs are added first.
12. **`Settings` without INFO1d.** Short phase 2 leaves no probe. Guard every `info1d.probed[...]` access (analogue.rs:182, digital.rs `Settings::new`).
13. **Cargo CI** runs `cargo clippy --workspace --all-targets -- -D warnings` (.github/workflows/ci.yml:39). Unused V.92 scaffolding will fail CI unless it is wired in or `#[cfg(test)]`-used.

---

## 14. House style (for code that must read like this code)

**Module docs** (`//!`):

- Open with a plain-English account of the idea. Examples:
  - pcm.rs:1-33 ("Three jobs, one after another…");
  - dil.rs:1-15.
- Then cite clauses in parentheses, bare within the module's own Recommendation: "(9.3.2.4)", "(8.4.1)".
- Cross-Recommendation citations use "10.1.3.1/V.34" or "Table 15/V.90".
- Timelines are drawn in ```` ```text ```` blocks (analogue.rs:4-7).

**Constants:**

- Each has a `///` doc giving its source. Either:
  - a clause plus a quoted phrase: `/// B1d: "48 data frames" (8.6.1).`, `/// "70 +- 5 ms" of silence after INFO1a (9.3.2.1).`; or
  - live-call evidence: `SD_WAIT` (:53-57), `L2_READ` (phase2.rs:106-122).
- Related constants are grouped under one doc comment (analogue.rs:82-90).
- Types follow units:
  - seconds as `f64` (`const SD_WAIT: f64 = 2.0;`), converted with `self.samples(seconds)`;
  - symbol and frame counts as `usize`, often as expressions (`2 * INTERVALS`, `48 * INTERVALS`, `3 * JD_BITS as u64`);
  - levels as fractions of full scale (`f64`);
  - power ratios as plain ratios, with the dB value in the doc.
- Values that depart from the Recommendation say so and why, in the doc.

**Quoting:**

- Short quotes in straight double quotes, ASCII only: "+-", "S-bar", "J'd", "R-bar-d".
- μ and − appear only in a few doc comments (dil.rs:59, server.rs:39-41).

**Comments:**

- Explain *why*, often with live evidence ("A live server sent Jd at the last moment…").
- Clause-anchored action comments look like `// 9.4.2.3: "complete sending the current CP sequence, and then send CP' sequences".`
- About 17 to 27% of lines are comments (analogue.rs 334/1931; pcm.rs 279/1291).
- British spelling in prose: analogue, equaliser, normalised, behaviour. Quotes keep the ITU spelling ("initialized").
- No emojis. No TODO litter.

**Naming:**

- Plain English and spec names. Types: `Stage`, `Status`, `Settings`, `Source`, `Up`, `Frames`, `RWatch`, `RSeen`, `Trust`, `Choice`, `Route`.
- Variants: `AwaitJdPrime`, `SBar`, `Turned(u64)`, `Moved(usize)`.
- Functions name events or acts: `heard`, `symbol`, `turned`, `clamp`, `cleared_down`, `send_s_then_cp`, `begin_dil`, `finish_dil`, `watch_margin`, `prepare_upstream`.
- Booleans read as facts: `heard`, `looked`, `clearing`, `renegotiating`, `awaiting_turn`, `far_end_went`.
- Failure reasons are lowercase sentences: `"no Sd from the digital modem"`, `"the route cannot carry V.90's slowest rate"`.

**Idiom:**

- Rust 2024 (`edition = "2024"`): `let … else`, `if let … && let …` chains, `is_some_and`, `is_none_or`, `std::array::from_fn`, `std::mem::take`, `repeat_n`, `is_multiple_of`.
- `#[derive(Debug, Clone)]` on every state struct (the modems are `Clone`); `Copy` on small enums.
- `VecDeque` for queues and history.
- `Option<(u64, &'static str)>` for deadlines.
- `Status::Failed(&'static str)`.
- Pull-model symbol sources: `tx.next_sample(|| source.next())`.
- `pub(crate)` for cross-module internals (`carrier::Watch`, `RetrainWatch`).
- One-shot flags read and cleared by `take_*()` (`take_retrain`, `take_bits`).
- Public getters are small, with one-line docs.
- Long single-line struct literals and conditions are normal. Lines run to about 136 columns. There is no rustfmt.toml and CI checks clippy only.

**Tests:**

- In-file `#[cfg(test)] mod tests`, and integration tests in `crates/datapump/tests/`.
- Names are sentences (`j_prime_is_read_after_a_jd_a_slip_cut_into`, `a_robbed_bit_halves_one_interval_s_ladder`).
- A doc comment on each test cites the clause or the capture.
- Inline xorshift PRNGs; no external crates.
- `println!` of measured values next to asserts.
- Real captures are referenced by file name (`tests/vectors/v90-56k.wav`).

---

## 15. Open questions

1. **Refactor first?** Should the downstream refactor (11.1, `v90/downstream.rs`) be done and committed before any V.92 code? It touches the slip-sensitive DIL code that live calls depend on.
2. **Fallback ordering.** How many PCM-upstream failures before asking for V.34 upstream (`V92_RETRAINS`)? On Rory's rig upstream PCM may never survive; should V.34 upstream be the default until a live V.92 server proves otherwise?
3. **Early Su.** V.90's live fix sends S before Jd arrives, because a server ignored the RTD. V.92 orders Su after Jd, and its digital-side watchdog includes RTD (9.5.1.2.2). Keep strict V.92 order, or send Su early when RTD is large? An early Su would arrive before the digital modem is listening for it; that was harmless in V.90, but V.92's Su also carries the phase measurement.
4. **Clock slaving.** Is a smoothed `drift` alone enough, or does the digital modem need phase stability tighter than the receiver can give after ε is applied? This needs a V.92 server capture.
5. **Where modem-on-hold lives.** Should the MH receive path live in `v34/phase2.rs` (beside the retrain it is indistinguishable from), or in `v92/hold.rs` with phase 2 only signalling "tone B heard"? The proposal is the former for detection and the latter for the transaction.
6. **FPE and data-mode timing.** FPE needs RM through the full data chain at a frame boundary. Should `watch_margin` choose FPE automatically, or only on request?
7. **Echo cancellation.** Is one worth adding in front of `pcm::Receiver` now, while it is only needed for 2-wire lines? V.92's RR silence exists for it.
