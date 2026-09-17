# V.92 planning: the V.90 test infrastructure, and how to test V.92 with it

This digest covers the code, not the Recommendation. It maps the V.90 test harnesses and the
simulated network, then proposes how to test a V.92 call end to end when both ends are ours:
new harness pieces, what the network model is missing for PCM upstream, a V.92 test list, a
runtime budget, and how the constraints seen on live calls will hurt upstream PCM.

## 0. Scope, sources and conventions

Code read, all under `F:\dialupmodem2\crates`:

- **datapump tests:** `datapump/tests/v90_call.rs`, `v90_vector.rs`, `v90_replay.rs` and `dil_sounds.rs` (header only).
- **datapump V.90 sources:** all of `datapump/src/v90/*.rs`. `analogue.rs`, `digital.rs`, `startup.rs`, `network.rs` and `server.rs` were read in full or near-full. The other modules were outlined, with their module docs read.
- **V.34 hooks that V.90 uses:** `datapump/src/v34/info.rs` (INFO layouts), `v34/phase2.rs` (the `Pcm` role and its unit-test `Line`) and `v34/startup.rs` (`decline_pcm`, `restart_phase2`).
- **modem crate tests:** `modem/tests/v90_call.rs` and `modem/tests/call.rs` (`Pair`, `run_lossy`, the long-line test), plus the headers of `replay.rs`, `replay_answer.rs`, `soundcard_loop.rs` and `dialin_call.rs`.
- **modem wiring:** `modem/src/lib.rs`, the `Pump` enum and the V.8 and pump selection.
- **DSP pieces:** `dsp/src/resample.rs` and `dsp/src/echo.rs` (headers).
- **Elsewhere:** `at/src/lib.rs:385` (the `+MS` list) and `gui/src/engine.rs` (`Standard::V92`).
- **Project files:** `Cargo.toml`, `.cargo/config.toml`, `.github/workflows/ci.yml` and `tests/vectors/README.md`.

Test timings come from two earlier release-profile runs of the whole workspace in this
session: `scratchpad\test_all.txt` and `scratchpad\test_all2.txt`, both dated 2026-09-17. Nothing
was built or run for this digest.

V.92 facts below (clause numbers, timers, bit positions) come from the sibling digests in this
folder, which were written from rendered PDF pages: `spec-intro-transmitter.md`,
`spec-phase2-signals.md`, `spec-phase3-procedures.md`, `spec-phase4-procedures.md`,
`spec-renegotiation-fpe.md` and `spec-modem-on-hold.md`. When implementing, take exact values
from those files, not from this one.

Notation:

- "tick" is one network sample (125 µs).
- "FS" is the analogue side's rate, 16 000 Hz in every test.
- "RTT" is the round-trip delay.
- `file:line` is relative to `crates/`.

---

## 1. Map of the V.90 test infrastructure

| File | What it is | Harness types | Tests | Release runtime (whole suite, parallel) |
|---|---|---|---|---|
| `datapump/tests/v90_call.rs` | A V.90 call between our two modems over a simulated network: phases 3–4 alone, and the full start-up from the end of V.8 | `Call` (:32), `FullCall` (:142) | 20 | **7.33 s** (all passed); 7.79 s (one failed) |
| `datapump/tests/v90_vector.rs` | A real Conexant-to-server V.90 call from `tests/vectors/v90-56k.wav`: V.8 menus, INFO sequences, Ja's DIL descriptor, TRN1d, Jd, J'd, the DIL | free functions `menus`, `sequences`, `ja`, `downstream` | 6 | 0.52–0.54 s |
| `datapump/tests/v90_replay.rs` | Replays a live capture's received channel through `startup::Analogue` (ignored; `V90_CAPTURE`, `V90_START`) | none | 1 ignored | 0 |
| `datapump/tests/dil_sounds.rs` | Design tool: renders a DIL descriptor to WAV (ignored; `DIL_SPEC`, `DIL_WAV`) | none | 1 ignored | 0 |
| `datapump/src/v90/*.rs` `#[cfg(test)]` | Unit tests: see §5.1 | none | 73 in v90 (all 282 datapump unit tests: 0.86–1.12 s) | included |
| `datapump/src/v34/phase2.rs` tests | Phase 2 between two ends, including V.90 pairs (`a_v90_pair_settles_on_v90`, :1157) | `Line` (:1035), `run` (:1071) | several | included |
| `modem/tests/v90_call.rs` | The whole modem driven by AT commands: dialling a V.90 server built from parts, and two `Modem`s with a softphone model at the answering end | `Server` (:19), `Call` (:90), `Pair` (:184) | 3 | **2.75–2.80 s** |
| `modem/tests/call.rs` | Two `Modem`s on a summed 16 kHz line; V.34 at 33 600; sample loss; a 562 ms-each-way line | `Pair` (:13), `run_lossy` (:1302) | 56 (+2 ignored) | 1.55 s |
| `modem/tests/replay.rs` | Replays a capture through the whole `Modem` (ignored; `MODEM_CAPTURE`, `MODEM_COMMANDS`) | none | 2 ignored | 0 |

CI (`.github/workflows/ci.yml`) runs `cargo clippy --workspace --all-targets -D warnings` and then
`cargo test --workspace` in the **dev profile**. `Cargo.toml` has no `[profile.dev]` or
`[profile.test]` opt-level override, so CI runs these suites unoptimised. That time was not
measured; float-heavy per-sample DSP is typically one to two orders of magnitude slower
unoptimised. Every doc comment that says how to run an ignored test uses `--release`.

A release rebuild after touching `datapump` took 1 m 02 s to 1 m 10 s in those logs.

---

## 2. The harnesses in detail

### 2.1 `datapump/tests/v90_call.rs`

**Fixtures**

- `server()` (:10) is an `Info0d`: µ-law, 1664-point, 3429 upstream allowed; `nominal_power: 4` (−10 dBm0); `max_power: 23` (−12 dBm0); `power_at_codec: true`.
- `settled()` (:22) builds `analogue::Settings` and `digital::Settings` directly, as phase 2 would have left them:
  - `Info1c` with every rate probed at `max_rate: 12`;
  - `Info1aPcm { uinfo: 79, upstream: S3200, md_length: 0 }`;
  - RTT 0.02 s; wide constellation on.

**`Call` (:32–78): phases 3 and 4 only**

The fields are `net`, `analogue::Modem`, `digital::Modem`, `up: Vec<f64>` and `ticks`. One tick (`Call::tick`, :47) is:

```text
to_digital = net.up(&up)          // every analogue sample made last tick -> one network level
up.clear()
from_digital = digital.step(to_digital)   // one level back
for x in net.down(from_digital):  // 0, 1, 2 (or a burst, see 2.4) analogue samples
    up.push(analogue.step(x))
ticks += 1
```

- `run_until(seconds, done)` (:57) prints a line each time either end's `phase()` string changes, and returns early when `done` is true.
- `check_connects` (:80) runs up to 25 s and stops at `Connected` or `Failed` at either end.

**`FullCall` (:142–188): the whole start-up**

`FullCall` does the same thing with `startup::Analogue::new(FS)` and `startup::Digital::new(server)`, so it covers V.90 phase 2 (inside V.34's), phases 3 and 4, retrains and the V.34 fallback.

- `run(seconds)` (:161) returns true when both ends are `Connected`, and false on the first `Failed` or on timeout.
- The tick body is **copied four times**: in `run`, `run_until_seconds` (:215), `carries_data` (:228) and `notices_silence` (:580). The last one silences one direction.

**Helpers**

| Helper | Line | What it does |
|---|---|---|
| `pattern(n, seed)` | :107 | xorshift bit stream |
| `contains(got, sent)` | :120 | whole-window match. Tolerates bits lost before or after, not inside. |
| `connects(net, server, seconds)` | :205 | asserts a connection and prints `last_failure()` |
| `carries_data(seconds)` | :228 | sends 30 000 bits down and 15 000 up; returns `(down_ok, up_ok)` |
| `comes_back_up(seconds)` | :348 | true once both ends have left `Connected` and returned to it (retrain or renegotiation) |
| `rates()` | :364 | `(receive, transmit)` from the analogue end |
| `notices_silence(server_stops, seconds)` | :580 | zeroes one direction; returns how long the other end's `carrier()` took to drop; asserts `far_end_went()` |

**What the tests assert through**

- Status: `status()`, `phase()` (the exact strings, e.g. `"V.90 phase 3: Jd"` at :560), `is_v90()`, `retrains()`, `renegotiations()`, `last_failure()`, `round_trip()` and `carrier()`.
- Through `analogue.v90()`: `dil_moved()`, `frames_moved()`, `receiver().drift_ppm()`, `receiver().trained_snr_db()`, `route().readings/levels` and `choice()`.
- On the digital side: `digital.v90().renegotiations()` and `phase3_snr()`.
- The network's own count, `net.slips()`.
- The live-server behaviour via `Digital::with_habits(Habits::LIVE_SERVER)`.

### 2.2 `datapump/tests/v90_vector.rs`

This reads `tests/vectors/v90-56k.wav` (16 kHz, both directions summed on one tap, the analogue modem nearest the tap).

**Phase 1 and 2**

- `menus` reads V.8 with `Bell103Rx::with_tones` and `v8::Decoder`.
- `sequences` reads the INFO sequences with `v34::dpsk::Receiver` for each `Side`.
- The tests check:
  - the CM and JM PCM bits;
  - INFO0d: µ-law, power at the codec, −12 and −10 dBm0;
  - the INFO order `[INFO0d, INFO1d, INFO1a (V.90)]`, with UINFO 78, S3200 and MD 20;
  - INFO1d's probed rates `[5, 6, 6, 7, 8, 9]`.

**Phase 3**

- `ja()` (:138) reads the upstream Ja with the V.34 QAM `Receiver`, from 10 s to 12.6 s of the file.
- `downstream()` (:208) runs `pcm::Receiver` from 12 s to 19 s. It descrambles Jd, finds J'd and aligns the DIL.
- The tests check:
  - the DIL is 147 segments and 17 334 symbols;
  - Jd ends at symbol 33 816;
  - the DIL starts at `end + 12`;
  - the DIL fits the descriptor to better than 14 dB, and does not fit shifted by 1 or 6 symbols.

**V.92 relevance.** `tests/vectors/v92-56k.wav` (103.2–126.5 s of the same source, "V.92 56k, V.34-style startup") is in the repository and **no test reads it**. The GUI labels it "V.92 (no receiver)" (`gui/src/engine.rs:105`). It is the first place to look for V.92 INFO bits (§9.5).

### 2.3 `datapump/tests/v90_replay.rs`

This feeds channel 0 of a capture (what arrived) into `startup::Analogue`, starting at sample `V90_START`, where the modem-level replay shows "V.90 phase 2". Each time the state changes it prints:

- the phase;
- DIL progress (`dil_progress()`), `is_lost()` and the moved count;
- SNR, trained SNR, drift and frame offset;
- whether Jd was read and whether the DIL was found without J'd;
- the RTT.

Channel 1 (what we sent) is never used. For V.92 it should be, because the upstream is where the new failures will be (§9.5).

### 2.4 `datapump/src/v90/network.rs`: the simulated route

`Network` (:41) is the only model of the PCM path. It is built with `Network::new(law, fs)` (:93) and configured with builder methods.

**Downstream** (digital to analogue), `down(level) -> Vec<f64>` (:234), one call per tick:

1. `carry()` (:220): quantise to the nearest G.711 codeword, and set the LSB if this is the robbed octet (every sixth, at `robbed` phase).
2. Apply the digital `pad` as scale-and-requantise (not a G.711 pad table).
3. The jitter buffer, at `now % every == 0` (periodic) or `now == at` (once):
   - **inserted**: the last `SLIP = 160` codewords again, linearly faded (:245);
   - **dropped**: the next 160 codewords discarded.
   - The counter is `slip_count`.
4. On the first call, the whole one-way delay is emitted **up front** as `delay·fs` zeros (:263).
5. The codec reconstruction: a windowed sinc with cutoff 3800 Hz at 8 kHz, reach `DOWN_REACH = 20` codewords (:282), evaluated at `down_next`, which advances by `(1+skew)·8000/fs`.
6. Optional gain control (:285): `gain = ceiling/|sum|` whenever the output would exceed `ceiling`; recovery `gain += (1−gain)/(release·fs)`.
7. Add Gaussian noise (`noise` is the RMS with full scale = 1).

**Upstream** (analogue to digital), `up(&samples) -> f64` (:305), one level per tick:

1. On the first call, `delay·fs` zeros are **prepended** (:306).
2. Each analogue sample is pushed as `up_gain·x + noise`. `up_gain` defaults to **0.25** (:113), "a telephone line delivers a modem's −9 to −12 dBm to the codec well inside its range".
3. The codec samples at `t = (up_next − lag)·per`, where:
   - `per = fs/((1+skew)·8000)`;
   - `lag = DOWN_REACH + 2 + UP_REACH·8000 = 86` ticks (:321).
   - With zero skew and fs = 16 000, **t is always an even integer**: the codec samples exactly on every other analogue sample.
4. The anti-alias filter is a windowed sinc with cutoff 3700 Hz at fs, reach `UP_REACH = 8 ms` (±128 samples at 16 kHz), **recomputed with sin/cos for every tap on every call** (:327–334).
5. Quantise to a G.711 codeword, unless `unquantised()` was set.

**Builders**

| Builder | Line | Direction | Used by tests? |
|---|---|---|---|
| `with_delay(seconds, _fs)` | :125 | **both** (the `_fs` argument is ignored) | yes, 23 times |
| `with_clock(ppm)` | :132 | **both** (the skew applies to `down` and to `up`) | once (`…120_ppm…`) |
| `with_noise` / `set_noise` | :138 / :145 | both | yes |
| `with_robbed_bit(phase)` | :150 | **downstream only** | once |
| `with_pad(db)` | :156 | **downstream only** | **never** |
| `with_upstream_gain(g)` | :162 | upstream | only its own unit test |
| `unquantised()` | :169 | upstream | **never** |
| `with_slips(seconds, inserted)` | :176 | **downstream only**, always 160 codewords | yes |
| `with_gain_control(ceiling, release)` | :186 | **downstream only** | twice, with (0.8, 0.3) |
| `with_slip_at(seconds, inserted)` | :192 | **downstream only**, 160 codewords | once |
| `slips()` | :198 | counts downstream slips | yes |

**Quirks to know before writing a V.92 test**

- **The analogue modem runs ahead of the ticks.** Because the downstream delay is emitted up front, the analogue modem hears network sample *n* at tick *n*. "Seconds" given to `with_slip_at` and `with_slips` are network time, counted from the start of phase 2.
- **"Ten millisecond" cuts are modelled as 20 ms.** Two test comments describe 10 ms cuts ("a jitter buffer cutting ten milliseconds out", `v90_call.rs:503`, :550), but `SLIP` is fixed at 160 codewords. The 80-symbol cuts seen live cannot be expressed today.
- **There is no echo path.** The digital modem never hears its downstream reflected, and the analogue modem never hears its own upstream. No V.90 code uses an echo canceller; `dsp::echo::EchoCanceller` (`dsp/src/echo.rs:112`) and `EchoFinder` (:320) are used only by V.32.
- **There is one law for both directions**, and no transcoder.
- Unit tests (:360–413) cover:
  - a codeword round trip at `up_gain = 1`;
  - 500 ppm gives 80 extra samples in 10 s;
  - a slip moves everything by ±320 samples;
  - a robbed bit moves codewords only in its own octet of six.

### 2.5 `modem/tests/v90_call.rs`

**`Server` (:19–88)** is a V.90 server built from parts, stepped at 8 kHz:

- `v8line::Modem` answering with `offering_lapm()` and `offering_pcm_on(digital)`, output ×0.3;
- then `startup::Digital::new(info0d())`;
- then an `ec::stack::Stack` (answerer, `t401_for_line(.., 60)`, `over_a_round_trip(60)`), fed bit by bit and topped up to 256 pending bits.

**`Call` (:90–131)** puts `Modem::new(FS)` (the caller, driven by AT) and `Server` over `Network::new(Mu).with_delay(0.015).with_noise(1e-5)`. The tick is the same as in §2.1, plus `take_dte()`.

**`Pair` (:184–266)** is two `Modem`s, with a **softphone model at the answering end only**:

- Upstream: `net.up` produces a µ-law level, `decoded` resamples 8 k to 16 k, and the host steps.
- Downstream, host to encoder, one of two paths:
  - `straight: Some(phase)` takes every other 16 kHz sample, chosen by `count % 2 == phase`;
  - otherwise the sample goes 16 k to 48 k (`raised`), through a `delay` queue, then 48 k to 8 k (`lowered`).
- The caller side has no softphone model; it uses the network's loop model (§2.4).

The host is `Pump::V90Server(server::Line)` (`modem/src/lib.rs:108`, `v90/server.rs:55`). `server::Line` resamples 16 k to 8 k into `Digital` and 8 k back to 16 k, with `SLACK = 8` (:37).

**Tests**

| Line | Test | What it checks |
|---|---|---|
| :134 | `dialling_a_v90_server_connects_at_pcm_rates_and_carries_text` | CONNECT at ≥ 48 000, text both ways over V.42, a server retrain, then text again |
| :276 | `two_of_these_connect_at_pcm_rates_when_the_codewords_reach_the_encoder` | Phase 0 gives V.90; phase 1 gives V.34 at 33 600. Also `distant()` rows and `constellation_peak()`. |
| :332 | `when_one_end_hangs_up_the_other_notices` | NO CARRIER within 4 s |

### 2.6 `modem/tests/call.rs` and `v34/phase2.rs` tests

- **`call.rs` `Pair` (:13).** Two modems at 16 kHz, each hearing the other's previous sample. There is no network.
  - `run_lossy(every, run)` (:1302) steps both ends `run` extra times on stale input, to model a jitter buffer running dry.
  - `error_control_is_asked_for_once_over_a_line_with_a_long_round_trip` (:560) uses hand-rolled `VecDeque` delays (562.5 ms each way) and counts SABMEs and UAs from `take_frame_log()`.
- **`phase2.rs` `Line` (:1035).** A delay, loss, echo and noise line used by the phase-2 unit tests. It is the only simulated line in the tree with an echo term.

### 2.7 Replay, vectors and the live rig

- Captures are `dist/captures/live-<stamp>.wav`: channel 0 is what arrived, channel 1 is what was sent. Only channel 0 is replayed.
- Live testing is Rory's job (memory: dialupmodem2-test-loop): MicroSIP on VB-Cable A and B, and `dialupmodem2.exe --answer` for our server.

---

## 3. The code under test: modules, roles, key items

| Module | Role | Key items |
|---|---|---|
| `datapump/src/v90/mod.rs` | Rate ladder and Table 2/15 arithmetic | `POWER_LIMITS` :45, `power_limit` :51, `training_codeword` :63, `INTERVALS = 6` :71, `SYMBOL_RATE` :74, `RATE_STEP = (8000, 6)` :83, `SLOWEST/FASTEST` :86–87, `K_RANGE/S_RANGE/D_RANGE` :90–94, `table_2_has` :102, `rate_for` :117, `bits_for` :122 |
| `v90/startup.rs` | V.34's start-up with V.90's phase 2 inside it; hands over to the V.90 phases 3 and 4; retrains; V.34 fallback | `V90_RETRAINS = 2` :23, `Status` :27, `Analogue` :41 (`new` :55, `step` :216), `Digital` :266 (`new` :284, `with_habits` :301, `step` :457) |
| `v90/analogue.rs` | The analogue modem from phase 3: V.34 QAM up, PCM down, DIL analysis, CP, renegotiation, cleardown, slip recovery, margin watch | constants :48–147, `Status` :151, `Settings` :162/:181, `Up` :199, `Source` :217, `Stage` :721, `Modem` :735, `new` :825, `phase` :939, `step` :1199, `stage_step` :1256, `heard` :1294, `symbol` :1330, `begin_dil` :1394, `find_dil_start` :1420, `fit_dil` :1449, `finish_dil` :1611, `phase4_symbol` :1647, `watch_margin` :1805, `prepare_upstream` :1848 |
| `v90/digital.rs` | The digital modem from phase 3, at 8 kHz; its upstream receiver is V.34's QAM `Receiver` | `FS` :42, constants :45–72, `Status` :76, `Habits` :87 (`PROMPT` :96, `LIVE_SERVER` :100), `Settings` :111/:129, `Out` :148, `Source` :169, `Stage` :459, `Modem` :474, `new` :523, `step` :718, `begin_renegotiation` :767, `stage_step` :826, `reversal` :897, `symbol` :944, `heard_ja` :1010, `heard_cp` :1031, `heard_e` :1065, `make_mp` :1089, `upstream_rate` :1119 |
| `v90/pcm.rs` | The analogue modem's downstream PCM receiver: interpolator, least-squares training, NLMS equaliser with decision feedback, timing and drift loops, slip detection | `BAUD` :43, `REACH = 31` :50, `FEEDBACK = 24` :61, `TRAIN_*` :81–88, `LOST_AT/FOUND_AT` :119–120, `Slicer` :133, `Heard` :178, `Symbol` :196, `Receiver` :310 (`new` :366, `feed` :519) |
| `v90/network.rs` | The simulated route (§2.4) | `SLIP = 160` :37, `Network` :41 |
| `v90/server.rs` | The digital modem at a line rate (16 kHz), for a VoIP host | `SLACK` :37, `ours()` :42, `Line` :55, `step` :86 |
| `v90/dil.rs` | DIL design and analysis; constellation choice | `LOUDEST = 0.3` :43, `SPACING` :49, `design` :71, `Route`, `Analysis`, `choose` :405 |
| `v90/encoder.rs` | 5.4 encoder and decoder: frame, mapping, CP to mapping | `Mapping::from_cp`, `for_renegotiation`, `Encoder`, `Decoder::could_have_sent` |
| `v90/modulus.rs` | 5.4.3 mixed-radix modulus encoder and mapper | `Moduli`, `encode`, `decode`, `Constellation` |
| `v90/sign.rs` | 5.4.5–5.4.6 sign coding, spectral shaping trellis | `Differential`, `Shaper`, `ShapeFilter`, `SignDecoder` |
| `v90/sequences.rs` | Jd, the DIL descriptor, CP, the V.90 MP framing; finders | `JD_BITS = 72`, `JD_PRIME_BITS = 12`, `Jd`, `Descriptor`, `Cp`, `CpFinder`, `DescriptorFinder`, `mp_bits` |
| `v90/ucode.rs` | Table 1 derived from G.711 | `Law`, `level`, `linear`, `octet`, `from_octet`, `nearest` |
| `v90/carrier.rs` | Level-based far-end-gone watch in data mode | `FAST` :17, `SLOW` :20, `WARM_UP` :23, `QUIET` :28, `GONE = 2.0` :36, `Watch` :40 |
| `v34/info.rs` | INFO layouts | `INFO1A_BITS` :24, `INFO0D_BITS` :30, `PCM_SYMBOL_RATE = 6` :35, `Info1c` :271, `Info0d` :371 (`to_bits` clears bits 26:27 at :407–408), `Info1aPcm` :457 (`from_bits` rejects upstream index 6 at :490–493), `Info` :505 |
| `v34/phase2.rs` | Phase 2, with V.90's role | `Pcm` :64, `Modem::v90` :378, `v90_retrain` :394, `decline_pcm` :415, `far_info0d` :433, `info1a_pcm` :438 |
| `modem/src/lib.rs` | Pump selection | `Pump::V90` / `V90Server` :105–108; V.8 PCM offer :2109–2117; `offered()` "V90" gives V34Duplex :2147; `start_pump` "V90"/"V90S" :2261–2262, :2283–2284 |
| `at/src/lib.rs:385` | The `+MS` modulation list | `["V90", "V34", "V32B", "V32", "V22B", "B103"]`; no "V92" |

---

## 4. State machines

### 4.1 `startup::Analogue` and `startup::Digital`

```text
            +------------- V.34 start-up (phase 2 = V.90's) --------------+
 new() ---> | phase2 Done && INFO1a(PCM) && INFO0d && INFO1d               |--- phase2 Done with V.34 INFO1a ---> V.34 training -> V.34 data
            +---------------------------------+---------------------------+
                                              v
                                   v90 = analogue::Modem::new(settings)
                                              |
       take_retrain() --------------------->  back_to_phase2()  (v34.restart_phase2)
       Failed(why) ------------------------>  back_to_phase2(); failed_starts += 1
                                              if route shown hopeless || failed_starts > V90_RETRAINS:
                                                  v34.decline_pcm()   // 9.2.2.1.9: V.34 INFO1a next time
```

- **`Status`:** `Running`, then `Connected { transmit, receive }`. After the first connection, `Running` is reported as `Retraining`. There are also `ClearedDown` and `Failed(&'static str)`.
- **The digital side** (`startup.rs:457`) mirrors this. It retrains on `take_retrain()`, or on `Failed` while `failed_starts < V90_RETRAINS`, and multiplies every non-V.90 output by `phase2_gain`, the nominal power relative to a full-scale sine, which is +3.17 dBm0.

### 4.2 `analogue::Modem` (V.90, phase 3 on)

`Stage` (:721): `SendTraining → AwaitSd → Training → AwaitJd → AwaitJdPrime → Dil → Phase4 → Data → Finished`

| Transition | Where |
|---|---|
| `SendTraining → AwaitSd` when `Up::Ja` starts; `rx.hunt(uinfo)`; Sd deadline `SD_WAIT + 2·RTT` | :1258 |
| `AwaitSd → Training` on `Heard::Reversal`; `Up::Silence`; Jd deadline 4.5 s + RTT | :1297 |
| `Training → AwaitSd` again on `Untrained` before the Sd deadline (a false hunt) | :1309 |
| `Training → AwaitJd` on `Heard::Trained` | :1305 |
| `AwaitJd`: S is sent early at `JD_LATEST + S_AFTER_JD − RTT`; a CRC-checked Jd gives `far_jd` and `AwaitJdPrime` | :1335–1352 |
| `AwaitJdPrime → Dil` on J'd, or on `find_dil_start` when J'd was missed (`JD_GONE`) | :1356–1376 |
| `Dil → Phase4` on `finish_dil`, which chooses the CP or fails with "the route cannot carry V.90's slowest rate" or "V.34 carries more than V.90 on this route" | :1611 |
| `Phase4`: `RWatch` on R; R-bar gives TRN2d framing; MP read; CP then CP'; E after an MP' or Ed is heard | :1647, :1270 |
| `Data`: `Connected` once B1d is done and `Up::Data` is going out; margin watch every 0.25 s after 2 s | :1279–1288 |
| `Data`: renegotiation (`begin_renegotiation`), cleardown (`clearing`), far end gone (`carrier::Watch`), lost for more than 3 s gives a retrain | :1086, :1202–1233 |

- **Upstream signal enum `Up`** (:199): `Silence, S, SBar, Pp, Trn, Ja, Cp, E, Data`. These are all V.34 QAM symbols, through `v34::qam::Transmitter` (`tx.next_sample(|| source.next())`, :1253).
- **Deadlines** are `Option<(u64 sample, &'static str)>`. `start_deadline` is **15 s + 5·RTT** (:900; V.90 9.4.2).

### 4.3 `digital::Modem` (V.90, phase 3 on)

`Stage` (:459): `AwaitS → Training → ReadJa → SendJd → AwaitFirstReversal → AwaitSecondReversal → Phase4Cpt → Phase4Cp → Data → Finished`

- **Downstream signal enum `Out`** (:148): `Silence, Sd, SdBar, Trn1d, Jd, JdPrime, Dil, Ri, RiBar, Rd, RdBar, Trn2d, Mp, Ed, B1d, Data`.
- **Upstream reception** uses `v34::receiver::Receiver`. Its reports (`Heard::S`, `Reversal`, `Trained`, `Symbol`) drive `heard` (:877), `reversal` (:897) and `symbol` (:944).
- **Deadlines** are 15 s + 5·RTT for B1 (:566) and the S wait (:829–844). A missed deadline sets `wants_retrain`; it does not fail.

### 4.4 `pcm::Receiver`

- `Stage` (:222): `Idle → Hunting(Hunt) → Collecting { start, base, tries } → Trained`.
- **Reports (`Heard`, :178):** `Reversal { at }`, `Trained { snr_db, inverted }`, `Untrained`, `Symbol`, `Lost`, `Found`.
- **Slicer (:133):** `Binary`, `Known { first, levels }` (the DIL; NaN means "don't learn"), `Levels` (phase 4 and data) or `Free`.
- **Hunting:** the hunt looks for a period-6 pattern that inverts. That is Sd {+W,0,+W,−W,0,−W}. **V.92's Su {+a,0,+a,−a,0,−a} and Ru {+L,+L,+L,−L,−L,−L} have the same property** (from spec-phase3-procedures.md §3.1), so the hunt is a reuse candidate for the digital modem's upstream receiver.

### 4.5 `carrier::Watch`

Once data mode has been heard for `WARM_UP`, the watch compares a fast level (50 ms) with a slow reference (2 s). The far end is gone after `GONE = 2 s` below `QUIET` relative to that reference.

For V.92 this must not fire during modem-on-hold. MOH itself leaves hold on "silence detected for 2 s" (spec-modem-on-hold.md, MOH-9.10.2.1-R6).

---

## 5. What the current tests exercise

### 5.1 Unit tests in `v90/`

| Module | Tests | Covers |
|---|---|---|
| `mod.rs` | 6 | ladder, Table 2, Table 15, UINFO |
| `ucode.rs` | 5 | Table 1 from G.711 (512 values), ordering, nearest |
| `modulus.rs` | 6 | mixed radix both ways |
| `sign.rs` | 12 | sign coding, shaping trellis |
| `encoder.rs` | 9 | frame to codewords to bits, robbed-bit cost |
| `sequences.rs` | 11 | real Jd, real 1736-bit DIL descriptor, CP round trips, finders, MP framing |
| `dil.rs` | 9 | design, analysis, robbed-bit ladder, choice versus noise and ceiling |
| `pcm.rs` | 4 | trains and reads Jd, including an inverted line with drift and noise; scale; no training on silence or noise |
| `analogue.rs` | 2 | J'd after whole Jds, and after a slip-cut Jd |
| `carrier.rs` | 4 | gone after 2 s; quieter far end; comfort noise; nothing judged before data |
| `network.rs` | 4 | see §2.4 |
| `server.rs` | 1 | at 2× rate every other sample is the level |
| `digital.rs`, `startup.rs` | 0 | covered only by the integration tests |

### 5.2 Integration tests in `datapump/tests/v90_call.rs`

| Line | Test | Network | Simulated time |
|---|---|---|---|
| :99 | `phases_3_and_4_connect_over_a_clean_network` | 10 ms, 1e-5 | ≤ 25 s, stops at connect |
| :125 | `data_crosses_both_ways` | same | connect + 3 s |
| :191 | `a_whole_v90_start_up_from_phase_2_connects` (RTT 0.03–0.08) | 20 ms | ≤ 30 s |
| :253 | `a_robbed_bit_route_connects_and_carries_data` | robbed phase 2 | connect + 4 s |
| :277 | `an_a_law_network_connects` | A-law | ≤ 30 s |
| :284 | `a_voip_length_round_trip_connects` (RTT 1.15–1.3) | 0.6 s each way | ≤ 40 s |
| :292 | `a_noisy_loop_connects_slower` (< 50 000) | 3e-3 | ≤ 30 s |
| :302 | `a_slip_in_data_mode_is_followed_and_data_after_it_arrives` | slip at 9 s, ×2 | 2 × (connect + to 10 s + 4 s) |
| :319 | `a_retrain_from_either_end_comes_back_up` | | 2 × (connect + ≤ 30 s + 3 s) |
| :376 | `a_rate_renegotiation_from_either_end_settles_the_rates_asked_for` | 0.02 and 0.6 s | 4 × (connect + 2 × (≤ 10 s + 3 s)) |
| :417 | `a_line_gone_noisy_is_renegotiated_down` | `set_noise(1e-3)` | connect + ≤ 17 s |
| :434 | `a_cleardown_from_either_end_ends_the_call_at_both` | no noise | 2 × (connect + ≤ 3 s) |
| :453 | `a_sound_card_clock_120_ppm_off_is_followed_through_ten_seconds_of_data` | 120 ppm | connect + 10 s |
| :466 | `slips_during_the_start_up_are_followed` | 0.6 s; slips every 5.9/2.9/4.3/3.1/2.3 s | 5 × ≤ 25 s |
| :492 | `a_line_that_will_not_carry_pcm_comes_up_as_v34` (exactly 1 retrain) | 2e-2 | ≤ 60 s |
| :509 | `a_softphone_with_a_gain_control_and_a_hasty_jitter_buffer_is_followed` | 0.6 s, GC (0.8, 0.3), slips | 3 × ≤ 40 s |
| :536 | `a_server_that_sends_jd_at_the_last_moment_hears_s_in_time` | 0.3 and 0.6 s, `LIVE_SERVER` | 2 × ≤ 40 s |
| :554 | `a_cut_in_the_training_stretch_is_trained_past` | 0.3 s, one slip | 1 probe + 2 × ≤ 40 s |
| :618 | `a_far_end_that_stops_sending_is_noticed_at_either_end` (< 3.5 s) | 0.6 s | 2 × (connect + 2 s + ≤ 6 s) |
| :636 | `a_softphone_line_keeps_its_carrier_through_data_and_renegotiations` | 0.6 s, GC, slips 2.9 s | connect + 24 s, checked every tick |

"Connect" is roughly 7–15 s of line depending on the delay. This is an estimate from the sequence lengths, not a measurement. The suite simulates several hundred seconds of line in about 7.3 s of wall-clock time, in parallel on this machine. The slowest test bounds the wall-clock time, so every single test runs well over 15× faster than real time in release.

**Cost hot spot.** `Network::up` evaluates a 257-tap windowed sinc with `sin`/`cos` for every network sample (:327–334). With zero skew the sample instant is always an integer, so the kernel is identical every time and could be tabulated. Do that before V.92 adds more upstream processing.

---

## 6. Constants V.92 would touch

| Constant | Where | Now | V.92 note |
|---|---|---|---|
| `INTERVALS = 6` | `v90/mod.rs:71` | downstream data frame | Upstream PCM frames are **12** symbols, with constellation frame j = i mod 6 and trellis frame k = i mod 4. Add a separate `UP_INTERVALS = 12`; don't overload this one. |
| `SLOWEST/FASTEST`, `RATE_STEP` | `mod.rs:86–87`, `:83` | 28 000–56 000, 8000/6 | Upstream **24 000–48 000** in the same 8000/6 steps (clause 1 e). The Ja rate mask has 16 bits for 24 000–44 000 plus 3 bits for 45 333, 46 666 and 48 000 (spec-intro-transmitter.md §6.1). |
| `K_RANGE/S_RANGE/D_RANGE` | `mod.rs:90–94` | Table 2/V.90 | The upstream modulus-encoder bounds come from the V.92 tables. |
| `V90_RETRAINS = 2` | `startup.rs:23` | failed PCM starts before V.34 | Needs a V.92 counterpart: failed PCM-upstream starts before choosing V.34 upstream (Table 19 INFO1a) while keeping PCM downstream. |
| `start_deadline` 15 s + 5·RTT | `analogue.rs:900`, `digital.rs:566` | V.90 9.4.x | V.92 uses **20 s + 6·RTD** to B1u/B1d (TR5/TR6, 9.6.x.2.1). |
| `SD_WAIT = 2.0` (+ 2 RTT) | `analogue.rs:57` | relaxed from 1500 ms | V.92 TR3 is still 1500 ms with no RTD term (9.5.2.2.1), so the same relaxation applies. |
| Jd deadline 4.5 s + RTT | `analogue.rs:1301` | | V.92 TR4 is 4500 ms with **no** RTD (9.5.2.2.2); keep the RTT allowance. |
| `JD_LATEST`, `S_AFTER_JD` | `analogue.rs:67–68` | S before Jd | V.92 replaces "S after Jd" with **Su** (144T), then S̄u (24.5T), then Su until Jp. The live-server early-send trick must be re-derived for Su (T11: Su no later than 5000 ms after the silence starts). |
| `SILENCE_BEFORE_S = 0.070` | `analogue.rs:51` | 70 ± 5 ms | The same silence comes before Ru (T1). |
| `PHASE3_TRN = 1.0` | `analogue.rs:48` | V.34 TRN | Replaced by **TRN1u ≥ 2040T, a multiple of 12** (T4, T15). |
| `B1D_FRAMES = 48`, `ED_FRAMES = 2` | `analogue.rs:123`, `digital.rs:57–58` | | Unchanged downstream. B1u is 48 upstream data frames. |
| `RD_SYMBOLS`, `RENEGOTIATION_E(D)`, `CLEARDOWN_*` | `digital.rs:61–68`, `analogue.rs:127–130` | V.90 9.6/9.7 | V.92 9.8 (RR), 9.9 (FPE) and 9.11 (cleardown) replace these; see spec-renegotiation-fpe.md. |
| `S_WITHIN = 5.1` | `digital.rs:45` | | Becomes the Su watchdog, 5100 ms + RTD (TR2). |
| `RI_SYMBOLS`, `TRN2D_FRAMES`, `SD_FRAMES`, `SD_BAR_FRAMES` | `digital.rs:48–72` | | Unchanged, but `Out` gains Jp, Jp' and SCR, and the digital modem gains TRN2u, CPu and SUVu reception. |
| `Habits::LIVE_SERVER` | `digital.rs:100` | 4.05 s TRN1d, no RTT in the S wait | Add a V.92 habit: a long TRN1d, and no RTT in the Su wait. |
| `DIL_TRUSTED = 0.3`, `dil::LOUDEST = 0.3` | `analogue.rs:95`, `dil.rs:43` | softphone headroom | The same ceiling must bound **upstream** peaks: LU, TRN1u, and the prefilter output G·v(n). |
| `MARGIN*` | `analogue.rs:135–141` | downstream margin watch | The digital modem needs an upstream equivalent to trigger 9.8 RR. |
| `PLACE_*`, `R_HEARD/R_MOVED` | `analogue.rs:72–73`, :145–147 | downstream re-framing after slips | The digital modem needs the same for 12-symbol upstream frames. A 160-codeword slip moves the frame by 160 mod 12 = **4** symbols; an 80-codeword cut by **8**. |
| `SLIP = 160`, `DOWN_REACH`, `UP_REACH`, `up_gain = 0.25`, cutoffs 3800 and 3700 Hz | `network.rs:37`, :30, :34, :113, :282, :325 | | See §7. |
| `SLACK = 8`, `ours()` | `server.rs:37`, :42 | | `ours()` needs INFO0d bit 27 (V.92). The 16 k to 8 k down-conversion is not codeword-exact (§7, item 11). |
| `INFO0D_BITS`, `INFO1A_BITS`, `PCM_SYMBOL_RATE = 6` | `info.rs:30`, :24, :35 | | `Info0d::to_bits` forces bits 26:27 to 0 (:407–408). `Info1aPcm::from_bits` rejects index 6 in bits 34:36 (:490–493), which is exactly Table 18. `Info0.clock` holds INFO0a bits 26:27, which V.92 uses for V.92 capability and the short-phase-2 request, in **the opposite order from INFO0d** (spec-phase2-signals.md N-3). `Info1c.probed[5].high_carrier` is bit 70, the PCM-upstream flag under V.92 only (N-4). |
| `carrier::GONE = 2.0` | `carrier.rs:36` | | Must be suspended while on hold (9.10). |

---

## 7. What the network model lacks for PCM upstream

V.92's upstream only works if the codec at the far end samples our transmitted waveform and gets
the codewords we meant. The digital modem measures the analogue path and designs the precoder
and prefilter; the analogue modem only follows them (spec-intro-transmitter.md 6.4.2, 8.8.3).
Today's `Network::up` is a loop model. It is adequate for V.34 upstream and missing everything
else that matters here.

1. **Upstream robbed bits.** A T1 robs bits in both directions. `carry()` is downstream only. Upstream, a robbed octet every 6 hits intervals *i* and *i*+6 of each 12-symbol frame.
   - *Proposal:* `with_robbed_bits(down: Option<usize>, up: Option<usize>)`, keeping `with_robbed_bit` as the downstream shorthand. Use independent phases.
2. **Upstream digital pad.** `pad` is downstream only, and no test uses even that.
   - *Proposal:* `with_pads(down_db, up_db)`, and a test for each direction.
3. **Upstream slips.** The far gateway's jitter buffer (or, with both ends ours, the host softphone's) slips what we send. We can never observe that live.
   - *Proposal:* direction-tagged `with_slips(Direction, seconds, inserted)` and `with_slip_at(Direction, ..)`, plus `slips_down()` and `slips_up()`, keeping `slips()` as downstream.
4. **Slip length.** `SLIP` is fixed at 160. The live 10 ms (80-symbol) cuts cannot be modelled.
   - *Proposal:* `with_slip_length(codewords)`, defaulting to 160. Also fix the two test comments that say "ten milliseconds".
5. **Upstream gain control.** A softphone's capture path can have its own AGC or limiter.
   - *Proposal:* `with_upstream_gain_control(ceiling, release)`, applied to the analogue waveform before the codec, with the same law as the downstream one.
6. **Fractional sampling phase.** With zero skew the codec samples exactly on our even samples (§2.4). Jp's ε (a 16-bit fraction of T, bits 18:33; S̄u extended by (24+ε)T) is therefore never exercised.
   - *Proposal:* `with_upstream_phase(fraction_of_T)`, which adds `fraction·per` to `t`, and make `lag` fractional-capable.
7. **Echo.** There is no CO hybrid echo of the downstream D/A output in the upstream A/D input. That is what the V.92 digital modem's canceller is for (clause 1 b, "channel separation by echo cancellation"). There is also no near echo of our upstream in our downstream, and no far echo at a VoIP delay (the live V.34 far end echoed us only ~25 dB down).
   - *Proposal:* `with_echo(hybrid_db, taps)`, which adds a short FIR of recent downstream levels to the upstream sum before quantising and does the reverse for the analogue side, plus `with_far_echo(db, delay)`.
8. **Transcoding.** There is one `law` and no gateway. Crazytel decodes µ-law, low-passes (−3 dB at 3.75 kHz, −18 dB at 4 kHz) and re-encodes as A-law (memory: crazytel-pcm-path).
   - *Proposal:* `with_transcoder(to: Law, low_pass: Option<(f64, f64)>)`, applied per direction in the codeword domain, with a unit test of the response at 3.75 and 4 kHz.
9. **Softphone upstream path.** The upstream is always an analogue loop: 3700 Hz anti-alias, loop noise, and sampling on the network clock. With both ends ours over VoIP there is **no analogue loop at all**. Our 16 kHz samples go through MicroSIP's decimation (or a straight 1-in-2) directly into its G.711 encoder, and then packets carry them verbatim.
   - *Proposal:* `enum UpPath { Loop, Straight { phase: usize }, Resampled { delay: usize } }` with `with_up_path`. This moves the modem-crate `Pair`'s host-side softphone model (§2.5) into the network, so both directions and both crates can use it.
10. **Clock model split.** `with_clock` skews both directions as a continuous resampling. That is right for a real loop, where the far codec samples our waveform on the network clock. For `UpPath::Straight`, the clock offset instead turns into **packet slips** at the far jitter buffer: one 20 ms slip every 20 ms / |δ| (175 s at 114 ppm), plus whatever adaptive slips it adds.
    - *Proposal:* in the softphone paths, `with_clock` produces upstream slips at that rate, not skew.
11. **Host-side exactness.** The V.90 digital modem reads upstream QAM through `server::Line`'s 16 k to 8 k `Resampler`. A V.92 digital modem needs the exact codeword. Up-sampling 8 k to 16 k is exact on even samples (`server.rs` test :120). Down-sampling 16 k to 8 k with the stretched windowed sinc (`dsp/src/resample.rs`, `HALF = 16`) is **not** exact near 4 kHz: its kernel is −6 dB at the new Nyquist.
    - *Proposal:* a `server::Line` mode that takes one sample in two, with the phase found from TRN1u, and a unit test that measures the error of today's down-conversion.
12. **Octets upstream.** `up()` returns a level. A V.92 digital modem thinks in Ucodes and signs.
    - *Proposal:* `up_codeword() -> (u8, bool)`, keeping `up()`. Neither is lossy, but the octet form makes robbed-bit and pad tests exact.
13. **Asymmetric delay.** `with_delay` sets both directions.
    - *Proposal:* `with_delays(down, up)`, to test RTD-sensitive procedures (Jp/ε, SUV acknowledgement windows of 100 ms + RTD) with unequal legs.
14. **Upstream level definition.** `up_gain = 0.25` exists to fit V.34's 0.707-RMS output inside the codec. For PCM upstream the level at the codec *is* the constellation. The digital modem measures it (TRN1u at LU) and sends G in CPd (bits 35:50, 4·G, Q0.16).
    - *Proposal:* keep `up_gain` as part of the channel, and sweep it in tests (0.25, 0.5, 1.0) so the G handling is exercised.

---

## 8. How a V.92 call (both ends ours) would be tested end to end

Five layers, cheapest first. Each layer reuses the V.90 harness shape, so V.92 tests read like the
V.90 ones.

### 8.1 Phases 3 and 4 alone (datapump, `tests/v92_call.rs`, `Call`)

`settled_v92()` returns the V.92 `analogue::Settings` and `digital::Settings` as phase 2 would leave them:

- **INFO0d:** bit 27 = 1 (V.92).
- **INFO1d:** bit 70 = 1 (PCM upstream supported).
- **INFO1a in Table 18 form:** bits 34:36 = 6, bits 37:39 = 6, UINFO 79, Ltot and Lmax codes, MD length 0.

The tick is unchanged (§2.1): the digital modem still takes one level and gives one level per tick. Everything V.92-specific is inside the two modems and the network's upstream.

Use this layer for:

- sweeps: ε phase, up_gain, robbed and pad phases, and slip positions;
- the precoder design check: the digital modem's residual ISI after CPd, read through an accessor.

### 8.2 The whole start-up (datapump, `FullCall`)

`FullCall::new(net, server_v92())`, where `server_v92()` is `server()` with `v92: true`.

Recommendation: make `startup::Analogue::new(FS)` V.92-capable by default, the way a real modem is. Existing V.90 tests keep `server()` with bit 27 = 0 and must keep passing untouched; that is itself the V.92-to-V.90 interop test. Add `Analogue::without_pcm_upstream()` to force Table 19 INFO1a, and `Digital::v90_only()` for the reverse pairing.

Add new accessors, following the existing names:

- `is_v92()`, `upstream_pcm()`;
- `epsilon()` (ε from Jp, on both sides);
- `precoder()` (a CPd summary);
- `up_frames_moved()` (on the digital side).

Refactor while copying: give `FullCall` a single `tick()` (as `Call::tick` has) and build `run`, `run_until_seconds`, `carries_data`, `comes_back_up` and `notices_silence` on it.

### 8.3 The whole modem over AT (modem crate, `tests/v92_call.rs`)

- **`Server`:** as in §2.5, but with `Digital::new(info0d_v92())`.
- **`Pair`:** gains a caller-side softphone. Use `Network` with `UpPath::Straight { phase }` (§7, item 9) and the existing host-side `straight` phase.
- **The expected matrix:** 2 × 2 (caller phase × host phase).
  - The caller can fix a half-sample offset by delaying its output one 16 kHz sample, which is ε = 0.5 T. So both caller phases should reach PCM upstream.
  - Host phase 1 should still end on V.34, as the V.90 test shows.
- **AT and modem wiring:**
  - add `+MS=V92` to `at/src/lib.rs:385`;
  - map `"V92"` in `offered()` and `start_pump()` (`modem/src/lib.rs:2147`, :2261–2284);
  - `standard()` should report "V.92";
  - `transmit_rate()` should be in 24 000..=48 000;
  - `distant()` rows should say "V.92".

### 8.4 Golden vector and replay

- **`datapump/tests/v92_vector.rs`**, copied from `v90_vector.rs`. It reads the V.8 menus and INFO sequences off `tests/vectors/v92-56k.wav`, then asserts the V.92 bits:
  - INFO0d bit 27;
  - INFO0a bit 26 (if INFO0a is readable; in the V.90 file it was masked by the JM);
  - INFO1d bit 70;
  - INFO1a bits 34:36 (6 means Table 18; 3–5 means Table 19).

  What the capture contains is unknown until this is run. `info.rs` must first stop dropping Table 18 (§6).

  If the call did use PCM upstream, the loud upstream at the tap can be read directly. Ru, TRN1u and Ja are ±LU without the precoder (spec-phase3-procedures.md §3.1), so a `pcm::Receiver`-style hunt on Ru's period-6 inversion can find them.
- **`datapump/tests/v92_replay.rs`** (ignored), copied from `v90_replay.rs` and with two additions:
  - print ε, CPd and Jp contents;
  - read **channel 1** (what we sent) and check that the levels we meant to send are the samples that left the sound card, taken 1 in 2 at the right phase. That is the only upstream-transparency check available without the far end's cooperation.

### 8.5 Live (Rory)

- **Against a real V.92 server.** INFO1d bit 70 is the server's own verdict on whether our path carries PCM upstream. Capture it on the first call, before anything else.
- **Both ends ours, live.** This needs two SIP legs: our answering server on cable B behind a second softphone account. With it, the host-side channel 1 shows exactly what arrived upstream, so upstream slips, AGC and transcoding become observable. Flag this rig requirement before proposing the milestone (memory: dialupmodem2-test-loop).

---

## 9. Proposed V.92 test list

Names follow the project's sentence style (§12). "(sweep)" marks a test that should be `#[ignore]` by default.

### 9.1 Unit tests (in-module `#[cfg(test)]`)

**INFO layouts**

- `info.rs`:
  - `a_v92_info0d_says_so_in_bit_27_and_asks_for_short_phase_2_in_bit_26`
  - `a_v92_info0a_has_the_two_bits_the_other_way_round`
  - `a_v90_peer_reads_a_v92_info0_as_v90`
- `info.rs`:
  - `info1d_bit_70_means_pcm_upstream_only_between_v92_modems`
  - `a_table_18_info1a_round_trips_and_is_not_thrown_away`: 34:36 = 6, 37:39 = 6, 40:49 all ones; Ltot 192/256/320/384 and Lmax 128/192/256/320 codes.
- `dpsk.rs`: `a_table_18_info1a_is_handed_over`.

**New sequences** (`v92/sequences.rs`)

- `jp_carries_epsilon_in_bits_18_to_33_and_says_jp_in_bit_47`
- `jd_says_jd_in_bit_47`
- `jp_prime_is_twelve_zeros_differential_from_jp_s_last_symbol`
- One round-trip test per table: CPt/CPu (Table 23), CPus (24), RM/RM' (25/26), SUVu (27), CPd (30), SUVd (31), MH sequences (32/33).
- Each should include a CRC check against V.34 10.1.2.3.2 and the leftmost-first versus LSB-first rule from clause 8 (spec-renegotiation-fpe.md §0).

**Precoder and transmitter** (`v92/precoder.rs`)

- `with_no_filters_the_output_is_the_chosen_level`: z1 = p1 = 0, z2 = [1], LP2 = 0, G = 1/rms.
- `the_precoder_output_stays_bounded_when_n_is_at_least_twice_m`
- `an_inverse_channel_gives_back_every_k_as_eta_mod_m`
- `the_memories_are_zero_before_b1u`
- `the_prefilter_s_feed_forward_starts_at_kappa_0_and_the_precoder_s_at_1` (sibling P-5)
- `the_state_is_never_saturated_only_the_output` (P-12)
- `four_g_reads_as_q0_16`

**Upstream encoder** (`v92/upstream.rs`)

- `the_upstream_ladder_runs_from_24000_to_48000_in_steps_of_8000_over_6`
- `a_12_symbol_frame_holds_two_constellation_frames_and_three_trellis_frames`
- `the_4d_trellis_is_clocked_once_every_four_symbols`
- `any_path_through_the_upstream_trellis_decodes`

**Transmit timing**

- `at_twice_the_rate_every_other_upstream_sample_is_the_level` (as `server.rs:120`)
- `s_bar_u_is_extended_by_epsilon_to_a_65536th_of_a_symbol`

**Digital modem's upstream receiver** (`v92/receiver.rs`)

- `ru_and_its_reversal_are_found_whatever_the_line`
- `su_to_s_bar_u_gives_the_phase_to_a_small_fraction_of_t`: against `with_upstream_phase(0.0, 0.25, 0.5, 0.75)`.
- `trn1u_trains_the_receiver`
- `a_slip_upstream_is_found_and_the_twelve_symbol_frame_moved_by_four`

**Digital modem's precoder design** (`v92/design.rs`)

- `the_designed_precoder_leaves_little_isi_on_the_loop_model`
- `the_coefficients_fit_inside_ltot_and_lmax`

**Echo** (`v92/receiver.rs`): `the_hybrid_s_echo_of_the_downstream_is_cancelled_from_the_upstream`

**Network** (`v90/network.rs`), mirroring the existing four tests:

- `an_upstream_robbed_bit_moves_every_other_codeword_in_one_octet_of_six`
- `an_upstream_pad_scales_every_level`
- `an_upstream_slip_moves_everything_after_it_by_the_slip_s_length`
- `a_ten_millisecond_cut_moves_everything_by_80_codewords`
- `a_fractional_upstream_phase_samples_between_our_samples`
- `the_transcoder_is_3_db_down_at_3750_and_18_db_down_at_4000`
- `a_straight_softphone_path_hands_the_encoder_our_samples_exactly`
- `a_clock_off_on_a_softphone_path_becomes_slips`
- `the_hybrid_echo_is_where_it_was_put`

**`server.rs`**

- `one_sample_in_two_of_the_line_is_the_codeword_the_far_end_meant`
- `the_resampled_down_path_is_not_exact_and_by_how_much`

**`carrier.rs`**: `a_call_on_hold_is_not_a_far_end_that_has_gone`

**Modem-on-hold and quick connect** (`v92/moh.rs`, `v92/qc.rs`)

- `mhreq_is_answered_with_mhack_and_its_hold_time`
- `ansam_for_a_second_after_mhfrr_starts_phase_1_as_the_caller`
- `qc1a_says_uqts_1111`

### 9.2 Both ends, datapump (`tests/v92_call.rs`)

**Basic connection and data**

| Test | Network | Asserts |
|---|---|---|
| `phases_3_and_4_connect_with_pcm_both_ways_over_a_clean_network` | 10 ms, 1e-5 (`Call`) | down ≥ 48 000; up on the PCM ladder, ≥ 24 000, target ≥ 40 000 (tune once measured) |
| `data_crosses_both_ways_at_pcm_rates` | same | `carries_data` both true |
| `a_whole_v92_start_up_from_phase_2_connects` | 20 ms (`FullCall`) | `is_v92()`, `upstream_pcm()`, RTT 0.03–0.08 |

**Interop and fallback**

| Test | Network | Asserts |
|---|---|---|
| `a_v92_analogue_modem_meets_a_v90_server_on_v90` | 20 ms, `server()` bit 27 = 0 | V.90 phase 3, V.34 upstream; all existing V.90 tests unchanged |
| `a_v90_server_that_sets_bit_70_does_not_get_pcm_upstream` | bit 27 = 0, bit 70 = 1 | no Table 18 INFO1a (sibling N-4) |
| `a_server_that_says_the_channel_will_not_carry_pcm_upstream_gets_v34_upstream` | bit 70 = 0 | Table 19 INFO1a, V.92 with QAM up, down still PCM |
| `an_upstream_that_will_not_carry_24000_comes_up_with_v34_upstream` | upstream noise only | one fallback step, data crosses; the downstream still PCM |

**Timing and gain**

| Test | Network | Asserts |
|---|---|---|
| `every_upstream_sampling_phase_is_found_and_corrected` (sweep: 8 phases) | `with_upstream_phase` | connects; `epsilon()` within ±1/64 T of the network's phase |
| `the_upstream_gain_at_the_codec_is_measured_and_used` | `with_upstream_gain` 0.25, 0.5, 1.0 | connects; the up rate does not collapse |
| `a_sound_card_clock_120_ppm_off_is_followed_upstream_too` | `with_clock(120)`, loop path | 10 s of data both ways; ε stays valid |

**Renegotiation, parameter exchange, retrain, cleardown, hold**

| Test | Network | Asserts |
|---|---|---|
| `a_rate_renegotiation_from_either_end_settles_the_rates_asked_for` | 0.02 and 0.6 s | 9.8; no retrain; data after |
| `a_fast_parameter_exchange_from_either_end_changes_the_rate_without_losing_the_frames` | 0.02 and 0.6 s | 9.9; data before and after; R-COM-1 frame sync kept |
| `a_renegotiation_wins_over_a_fast_parameter_exchange_begun_at_the_same_time` | 0.02 s | 9.9.1.1.2 precedence |
| `a_retrain_from_either_end_comes_back_up_as_v92` | 0.02 s | 9.7 |
| `a_cleardown_from_either_end_ends_the_call_at_both` | | 9.11 |
| `a_call_put_on_hold_is_held_and_resumed` | 0.6 s | 9.10 MHreq, MHack, silence, resume; `carrier()` does not drop |
| `a_hold_that_runs_out_ends_the_call` | | the MHack T1 time |
| `a_far_end_that_stops_sending_is_noticed_at_either_end` | 0.6 s | < 3.5 s; the digital modem's watch works on PCM upstream levels |
| `a_line_gone_noisy_upstream_is_renegotiated_down_by_the_digital_modem` | `set_noise` upstream only | RR from the digital side; no retrain |

### 9.3 Impairments, datapump (`tests/v92_call.rs`)

**Digital impairments**

| Test | Network | Asserts |
|---|---|---|
| `robbed_bits_both_ways_connect_and_carry_data` | robbed down phase 2, up phase 5 | connects; the upstream constellation copes with intervals i and i+6; data both ways |
| `a_digital_pad_either_way_connects` | pads (3, 0), (0, 3), (6, 6) dB | connects V.92 (or a documented fallback) |
| `an_a_law_network_connects_with_pcm_both_ways` | `Law::A`, server A-law | V.92 |
| `a_transcoding_gateway_leaves_pcm_upstream_out` | µ to A with (3750 Hz, −3 dB) and (4000 Hz, −18 dB) both ways | bit 70 = 0, or V.34 upstream chosen; down ≈ 32 000; no retrain loop |

**Delay**

| Test | Network | Asserts |
|---|---|---|
| `a_voip_length_round_trip_connects_with_pcm_upstream` | 0.6 s each way | RTT 1.15–1.3; no retrain; 20 s + 6·RTD never fires |
| `a_one_and_a_half_second_round_trip_connects` (sweep) | 0.75 s each way | as above |
| `unequal_legs_do_not_upset_the_acknowledgement_windows` | `with_delays(0.2, 0.9)` | SUV/CP acks within 100 ms + RTD |

**Slips**

| Test | Network | Asserts |
|---|---|---|
| `downstream_slips_during_the_v92_start_up_are_followed` | 0.6 s, the five (period, inserted) pairs from :468 | connects first time; `dil_moved` > 0 and `frames_moved` > 0 over the set |
| `upstream_slips_during_the_start_up_are_followed` | the same pairs, upstream | no retrain; the digital modem's `up_frames_moved` > 0 |
| `an_upstream_slip_between_jp_and_s_bar_u_is_survived` | `with_slip_at` placed from `phase()` | connects, perhaps via one retrain (document which) |
| `an_upstream_slip_in_data_mode_is_followed_and_data_after_it_arrives` | slip up at 9 s, inserted and dropped | `carries_data` true, true |
| `ten_millisecond_cuts_either_way_are_followed` | `with_slip_length(80)` | as above |

**Softphone behaviour**

| Test | Network | Asserts |
|---|---|---|
| `a_softphone_gain_control_on_what_we_send_is_kept_out_of` | upstream GC at ceiling 0.3 | our LU and prefilter peaks stay under it; connects V.92 |
| `a_gain_control_below_our_quietest_useful_level_drops_pcm_upstream_once` | upstream GC at ceiling 0.1 | one fallback, not a retrain loop |
| `a_softphone_that_passes_our_samples_straight_through_gives_pcm_upstream` | `UpPath::Straight`, phases 0 and 1 | both phases give PCM up (ε = 0 or 0.5 T) |
| `a_softphone_that_resamples_what_we_send_gives_v34_upstream` | `UpPath::Resampled` | V.34 upstream, PCM downstream |
| `a_clock_off_through_a_softphone_is_slips_not_drift` | `Straight` + `with_clock(114)` | data continues across the implied slips |

**Echo and live-server behaviour**

| Test | Network | Asserts |
|---|---|---|
| `the_hybrid_s_echo_is_cancelled_both_ways` | `with_echo(15 dB)` and `with_echo(25 dB)` | connects; the up rate is within one rung of the echo-free rate |
| `a_far_echo_a_round_trip_late_is_cancelled` | `with_far_echo(25 dB, 1.2 s)` | connects |
| `a_v92_server_with_live_habits_hears_su_in_time` | `LIVE_SERVER`-like, 0.3 and 0.6 s | no retrain |

**Sweeps (ignored)**

| Test | Network | Asserts |
|---|---|---|
| `a_slip_anywhere_in_a_dil_pass_is_found` | one slip every 5 ms across a DIL pass, inserted and dropped (memory: v34-phase2-tone-deadline) | reports the failures (17/260 today for V.90) |
| `a_slip_anywhere_in_trn1u_is_found` | the same, upstream | reports |

### 9.4 Whole modem (modem crate, `tests/v92_call.rs`)

- `dialling_a_v92_server_connects_with_pcm_both_ways_and_carries_text`: `+MS=V92`; CONNECT; text both ways over V.42; a server retrain; text again.
- `two_of_these_connect_with_pcm_both_ways_when_both_softphones_pass_codewords`: a 2 × 2 phase matrix (§8.3).
- `when_one_end_hangs_up_the_other_notices`
- `a_v92_call_put_on_hold_says_nothing_to_the_terminal_and_resumes`
- `ms_v92_is_offered_and_reported`: `+MS?`, CONNECT text, `distant()` rows.

### 9.5 Vectors and replay

- `v92_vector.rs`:
  - `the_v92_menus_offer_pcm`
  - `the_info_sequences_say_whether_v92_and_pcm_upstream_were_used`
  - if PCM upstream was used, `ru_trn1u_and_ja_are_read_off_the_tap` and `the_ja_descriptor_s_upstream_rate_mask_reads`
- `v92_replay.rs` (ignored): `probe_replay_v92` (channel 0) and `what_we_sent_is_what_we_meant` (channel 1 against the transmitter's intended 8 kHz levels).

### 9.6 Runtime budget

- **Start-up length.** A V.92 start-up is no shorter than V.90's, because phase 3 adds Ru, TRN1u, Su, Jp and a second TRN1u. The digital modem also adds a PCM receiver, an echo canceller and a one-off precoder design of up to Ltot = 384 coefficients. Plan for about 1.5× V.90's cost per simulated second.
- **Suite targets.** Keep the default `datapump/tests/v92_call.rs` near today's 7–8 s release wall-clock time:
  - stop every start-up at its connection (`run()`);
  - keep loops to 5 points or fewer;
  - mark anything that sweeps as `#[ignore = "sweep; see the module comment"]`.

  The modem-crate V.92 file should stay at 3–5 tests (about 3–5 s).
- **CI.** CI runs the dev profile, so the budget there is minutes. Before V.92 lands, consider `[profile.dev.package.datapump] opt-level = 2` (or `[profile.test]`) in `Cargo.toml`. That is a proposal for the maintainer, not something done here.
- **Network kernel.** Tabulate `Network::up`'s kernel first (§5.2).

---

## 10. Live-call constraints, and what each does to V.92 upstream PCM

Upstream PCM through a softphone means **our samples must reach the far codec (or the far
digital modem) unchanged**: sample for sample, at the right phase, with no gain change, no
resampling and no re-quantising. Every item below breaks one of those conditions.

| Constraint (evidence) | What it does to V.92 upstream PCM | What to do / test |
|---|---|---|
| **Round trip 1.1–1.5 s.** 0.92, 1.10, 1.13 and 1.125 s on SIP-trunk calls; 1.50–1.58 s measured 2026-09-07; Crazytel 1.531 s (memory: voip-line-round-trip, crazytel-pcm-path). | Phase 3 adds round-trip-paced steps: Su, then Jp, then S̄u(24+ε), then Jp', then DIL. Phase 4, RR and FPE add SUV/CP acknowledgement windows of **100 ms + RTD**. ε is measured at the digital end and applied a round trip later, so our transmit timing relative to the far codec must hold still for ≥ 1.5 s: at 114 ppm uncorrected, the phase moves 171 µs = **1.37 T** in 1.5 s. The spec's watchdogs TR3 (1500 ms) and TR4 (4500 ms) have no RTD term, just as V.90's did (`SD_WAIT` exists because of that). A retrain that mis-measures the round trip shortens every deadline (V.34 measured 0.3075 s against 1.1 s). After a short phase 2 the analogue modem has no RTD estimate at all (spec-phase4-procedures.md Q1). | Keep the `SD_WAIT`-style allowances. Derive every V.92 wait from RTD and test at 0.6 and 0.75 s each way. Store the full-phase-2 RTD for short-phase-2 use. Lock the transmit clock before Su on the loop path. |
| **Jitter-buffer slips.** +20 ms concealment inserts every few seconds (19.87, 20.22 and 21.61 s in one call); 10 ms (80-symbol) cuts at the most self-similar spot, including inside the DIL and TRN1d (memory: voip-jitter-slips, v90-live-test-pending). | **Upstream slips happen in a buffer we cannot see**: the far gateway's, or the host softphone's when both ends are ours. The digital modem's 12-symbol frame moves by 4 (160) or 8 (80) symbols, and its trellis and constellation frame indices move with it. A slip between the Su phase measurement and the S̄u extension makes ε wrong for the rest of the call. Concealment inserts non-codeword levels, which means a burst of errors and V.42 retransmissions. Against a real server, recovery is the server's business and unknown. Downstream slips also still hit our DIL, which has a known 36-symbol alias weakness (memory: v34-phase2-tone-deadline) that can drop V.92 to V.34 just as it drops V.90. | Direction-tagged slips and variable slip length in `Network` (§7, items 3–4). Upstream re-framing at the digital modem (a `PLACE_*` equivalent). Slip-anywhere sweeps. Median, not mean, statistics in every upstream estimator. |
| **Softphone gain control above ~0.3 of full scale.** Codewords up to ~0.32 FS arrived exact; louder ones were held to ~0.6 and read low for ~0.3 s (memory: v90-live-test-pending; modelled as `with_gain_control(0.8, 0.3)` downstream). | If our capture path has the same (unknown), any upstream sample above the threshold is squashed, and the samples after it read low. That corrupts TRN1u (so the gain G and the precoder design are wrong) and data. Precoding raises peak-to-average: the feedback section can make x(n) grow (P-12), so the prefilter output can exceed LU's scale even when the constellation does not. | Bound LU, TRN1u and G·v(n) peaks by the same 0.3 FS as `DIL_TRUSTED`/`dil::LOUDEST`. Add an upstream gain-control model and tests. Ask Rory to turn off AGC, AEC, noise suppression, VAD and CNG in MicroSIP, and to confirm on a capture. |
| **Crazytel transcoding.** µ-law decoded, low-passed (−3 dB at 3.75 kHz, −18 dB at 4 kHz) and re-encoded as A-law; PCM training capped at ~37 dB; V.90 would still carry ~32 000 (memory: crazytel-pcm-path). | If the upstream is treated the same way (only the downstream was measured), our codewords are re-quantised to the nearest A-law level of a filtered signal. That is signal-dependent noise at about the 37 dB level, applied **after** any equalisation our prefilter could do. The 4 kHz edge that 8000-symbol PCM uses is 18 dB down, so the prefilter would have to boost it and would hit the gain-control ceiling. A second A-to-µ conversion toward a US server adds more. Upstream PCM is effectively unavailable on this path. The V.34-upstream fallback must be solid, and the known V.34 fallback bugs (undetected slips at d²/8, the phase-4 MP rate, answer-role TRN, the retrain RTT) are the ones to fix first. | Transcoder model and test (§7, item 8). Assert a clean fallback: bit 70 = 0 or Table 19, down ≈ 32 000, no retrain loop. Live: offer PCMU only in MicroSIP (untested idea). |
| **Clock offsets.** A steady ~114 ppm between our sound card and the far clock; +69.9 ppm on another far end; 73 ppm on the recording (memory, `pcm.rs` doc). | V.92 6.2 says the upstream symbol clock is the network's. **On a real loop** the analogue modem must slave its transmit to the downstream clock and trim its phase with ε (a fractional delay at 1/65536 T resolution). **On a softphone path** our samples are forwarded verbatim, and the clock difference turns into packet slips at the far buffer (one per 175 s at 114 ppm). Resampling our transmit there would *create* non-codeword values. Which path we are on decides the transmit-timing design. A steady 114 ppm drift in what *arrives* suggests something resamples on at least the downstream path; if the upstream path resamples too, PCM upstream is impossible there. | Model both (§7, items 9–10). Choose the timing mode from what TRN1u and ε show. Live: measure whether channel 1 at the host arrives sample-exact (a both-ends-ours live call). |
| **Softphone decimation and phase.** 16 kHz to 8 kHz inside MicroSIP (or 16 kHz to 48 kHz in our `line` crate and 48 kHz to 8 kHz in MicroSIP). The V.90 `Pair` test shows that only one of the two 1-in-2 phases carries codewords (`modem/tests/v90_call.rs:276`). | The decimation filter is a linear channel the digital modem can learn from TRN1u and the prefilter can invert (Ltot ≤ 384). The sample phase is fixed per call, so ε matters. A 48 kHz leg may give a fractional phase. | ε sweeps. `UpPath::Straight` and `UpPath::Resampled` tests. The V.92 `Pair` 2 × 2 matrix. |
| **Echo.** A live V.34 far end echoed our signal back only ~25 dB down (memory: v34-phase2-tone-deadline). | Upstream PCM decisions have no margin for 25 dB of uncancelled echo. The digital modem needs a canceller (1 b) for the CO hybrid (short) and possibly far echo at VoIP delay (long; use the two-run `EchoCanceller` with `EchoFinder`, as V.32 does). Any softphone AEC on our capture path would *subtract* a filtered copy of the downstream from our upstream, which destroys it. | `with_echo` and `with_far_echo` tests. MicroSIP AEC off. |
| **Buffer lengths.** Receivers keep history in seconds: `HISTORY_SECONDS = 1.0`, `KEPT = 8192`, `DIL_KEPT_LOST = 2048`, `PLACE_KEPT = 48·6`. Every modem is `Clone`. | Upstream estimators that look back across a round trip need ≥ 1.5 s of history at 8 kHz (12 000 symbols). A far-echo canceller spanning 1.2 s as one tap run would need about 9600 taps, so use a placed second run. Precoder design with Ltot = 384 is a 384 × 384 solve, run once and cheap. | Size buffers from the measured RTD, not from constants. Keep the network kernels tabulated. |
| **Server habits.** A live V.90 server sent 4 s of TRN1d, sent Jd at the last moment, and gave up on S 1 s later regardless of RTT (memory: v90-live-test-pending). V.8 took 7 s because half the JMs arrived with bit errors. | Expect V.92 servers to wait for Su with no RTT in the wait (TR2 is 5100 ms + RTD in the spec, but the habit may not be). Expect the long TRN1d to push Jp later. | A V.92 `Habits` preset and its test (§9.3). |
| **LAPM over long RTT.** T401 of 1.03 s against 1.19–1.20 s SABME/UA round trips (memory: voip-line-round-trip). | Not PCM-specific, but a V.92 FPE or RR mid-transfer triggers V.42 retransmissions, and T401 must allow for the RTT. | Keep the V.42 long-line test. Add FPE-during-transfer to the modem-crate V.92 test. |

---

## 11. Extension points and concrete proposals

These are proposals. The sibling spec digests own the exact bit layouts and procedures.

### 11.1 Where V.92 lives

**New modules.** Add a new `crates/datapump/src/v92/` (and `pub mod v92;` in `datapump/src/lib.rs`), and keep reusing `v90` for everything V.92 inherits unchanged: downstream encoder, DIL, `pcm::Receiver`, ucode, sign coding.

| Module | Contents |
|---|---|
| `v92/mod.rs` | upstream ladder constants (`UP_INTERVALS = 12`, `UP_SLOWEST = 24_000`, `UP_FASTEST = 48_000`), `up_rate_for(bits)` |
| `v92/precoder.rs` | `Precoder`, `Prefilter`, `Coefficients { z1, p1, z2, p2, g }`, following 6.4.2, with f64 state |
| `v92/upstream.rs` | the analogue modem's PCM transmitter: `Encoder` (modulus encoder over 12 symbols, `v34::trellis` codes clocked every 4 symbols, inverse map) and `Transmitter` (8 kHz levels to fs, with ε fractional delay; mirrors `server::Line`'s up-conversion) |
| `v92/receiver.rs` | the digital modem's upstream PCM receiver: Ru/Su hunt borrowed from `pcm::Hunt`; phase measurement for Jp's ε; TRN1u training; echo canceller from `dsp::echo`; Viterbi; 12-symbol re-framing after slips |
| `v92/design.rs` | the digital modem's precoder and prefilter design from TRN1u, within Ltot and Lmax (the CPd NOTE: assume a power-minimising point choice) |
| `v92/sequences.rs` | `Jp`, `Cpt`, `Cpu`, `Cpus`, `Cpd`, `SuvU`, `SuvD`, `Rm`, and the Table 20 `Descriptor` extension (the upstream rate mask), with finders like `CpFinder` |
| `v92/moh.rs` | 9.10 modem-on-hold sequences and state |
| `v92/qc.rs` | quick connect (short phases 1 and 2) |

### 11.2 Changes to existing types

- **`v34::info`:**
  - `Info0d { v92: bool, short_phase2: bool }` (bits 27 and 26);
  - an INFO0a V.92 view that does not reuse `Info0.clock`;
  - `Info1c::pcm_upstream(v92_pair: bool)`;
  - `Info1aPcm` taking Table 18, e.g. `upstream: Upstream` where `enum Upstream { Qam(SymbolRate), Pcm { total_taps: u16, most_taps: u16 } }`, with the MD length in 276T units for Table 18 (sibling N-7).
- **`v34::phase2::Pcm`:** `Analogue` becomes `Analogue { v92: bool }`, or add a `v92` flag on `phase2::Modem`. `decline_pcm` needs a sibling, `decline_pcm_upstream`.
- **`v90::startup`:**
  - `Analogue::new` V.92-capable by default; `without_pcm_upstream()`; `Digital::v90_only()`;
  - `is_v92()`, `upstream_pcm()`;
  - `last_failure()` strings for the new failures, e.g. "the digital modem's precoder does not fit Ltot", "no Jp from the digital modem".
- **`v90::analogue::Modem`:**
  - `Settings` gains `upstream: UpstreamMode { Qam(Band), Pcm { ltot, lmax } }`;
  - `tx: Transmitter` becomes an enum of the V.34 QAM transmitter or `v92::upstream::Transmitter`;
  - `Up` gains `Ru, RuBar, Md, Trn1u, Su, SuBar, SuBarEps, Cpt, E1u, Trn2u, Suv, Cpu, E2u, B1u, Rm, RmPrime`, plus the RR, FPE and MOH signals;
  - `Stage` gains `SendRu`, `SendTrn1u` (Ja follows, as now), `AwaitJd` (then Su), `AwaitJp`, `SendTrn1uAgain` (S̄u(24+ε) then TRN1u), and `SendCpt` (inside `Dil`, since CPt overlaps the DIL); `Phase4` gains the SUV/CP exchange; `OnHold` is new.
  - `phase()` strings in the existing style: "V.92 phase 3: training", "V.92 phase 3: Jp", "V.92 phase 3: DIL", "V.92 phase 4", "V.92 data", "V.92 rate renegotiation", "V.92 parameter exchange", "V.92 on hold".
- **`v90::digital::Modem`:**
  - `rx` becomes an enum of V.34 `Receiver` or `v92::receiver::Receiver`;
  - `Out` gains `Jp, JpPrime, Scr, Suvd, Cpd, Rf`;
  - `Stage` gains `AwaitRu`, `TrainingUp`, `ReadJa`, `SendJd` (then measure the phase on Su), `SendJp`, `AwaitEpsilonReversal`, `Dil`/`Scr` against the second TRN1u, `AwaitCpt`, `Phase4` (SUV/CP), `OnHold`;
  - `Habits` gains the V.92 live-server fields.
- **`v90::server::Line`:** an exact 1-in-2 down path for V.92 (§7, item 11). `ours()` sets `v92: true`.
- **`v90::network::Network`:** everything in §7, with the builder style kept. Put per-direction impairments in a private `struct Impairments` held twice.
- **`modem`:**
  - `Pump::V90` and `Pump::V90Server` keep carrying V.92 (the start-up decides);
  - `standard()`, `transmit_rate()`, `constellation_point()` (a pair plot for the PCM upstream at the server; memory: v90-pcm-display-choice) and `line_phase()` learn V.92;
  - `+MS=V92` goes in `at/src/lib.rs:385` and `modem/src/lib.rs` `offered()` and `start_pump()`.
- **`gui/src/engine.rs:105`:** "V.92 (no receiver)" can become a receiver once `v92_vector.rs` reads the file.

---

## 12. Style observations (write V.92 code to match)

**Module docs.** `//!` opens with one line naming the thing and its clause, e.g. "The analogue modem from phase 3 on (9.3.2, 9.4.2): V.34 going up, PCM coming down." Prose paragraphs follow that explain *why*, often with the live call or recording that motivated a choice. Sequence diagrams go in ```` ```text ```` blocks with the two modems on aligned lines (`analogue.rs:4–7`, `digital.rs:10–20`).

**Constants.** Every constant has a `///` doc that quotes the Recommendation in double quotes and gives the clause in parentheses:

```rust
/// B1d: "48 data frames" (8.6.1).
const B1D_FRAMES: usize = 48;
/// TRN2d: "a minimum of 2040T" (9.4.1.2), in whole frames.
const TRN2D_FRAMES: usize = 340;
```

- Units are in the name or the type: seconds as `f64` (`SD_WAIT: f64 = 2.0`), symbols or frames as `usize` (`RD_SYMBOLS`, `SD_FRAMES`).
- Related constants are grouped under one doc comment (`DIL_FIRST_LOOK`/`DIL_LOOK_EVERY`/`DIL_KEPT_LOST`/`DIL_FIT`).
- A deliberate departure from the text is explained with the live evidence (`SD_WAIT`, `JD_LATEST`).
- Tables are derived, not copied, where possible (`ucode.rs`, `table_2_has`), with a test that reproduces the printed table.

**Inline comments** start with the clause: `// 9.3.2.4: "terminate Ja and transmit silence".` They explain intent, not mechanics. Comment density is about 15–21% of lines in `analogue.rs`, `digital.rs` and `network.rs`, and every `pub fn` has a doc comment.

**Naming.** Plain English with spec signal names kept: `sd_deadline`, `far_jd`, `r_watch`, `dil_moved`, `send_s_then_cp`, `heard_ja`, `begin_phase4`.

- Stages are named for what is happening (`AwaitSd`, `SendTraining`).
- Signal enums are `Up` and `Out`, with variants named after the signals (`SdBar`, `JdPrime`, `RiBar`).
- Bars are written `Bar` and primes `Prime`.

**Status and failure.** `Status { Running, Connected { .. }, ClearedDown, Failed(&'static str) }`. Failure reasons are lower-case English sentences ("no B1d from the digital modem"). Deadlines are `Option<(u64, &'static str)>` in samples, from a `samples(seconds)` helper.

**Idioms**

- `take_*` for read-once buffers and flags (`take_bits`, `take_retrain`).
- `with_*` builders (`Network`, `Digital::with_habits`); `Habits` presets as associated consts.
- Edition 2024 let-chains (`if let … && let …`), `let … else { return }`, `is_some_and`, `std::array::from_fn`, `std::mem::take`.
- Digit separators (`48_000`, `16_000.0`).
- Every pub type derives `Debug`, because the workspace lint `missing_debug_implementations = "warn"` combines with clippy `-D warnings` in CI. Modems also derive `Clone`. `unsafe_code` is denied.
- There is no `rustfmt.toml` and CI does not check formatting. Lines run to about 136 characters; 56 lines in `analogue.rs` and `digital.rs` exceed 100.

**Tests**

- Names are full-sentence claims in snake_case (`a_slip_in_data_mode_is_followed_and_data_after_it_arrives`).
- A doc comment above each test says what real thing it models and why.
- Parameter tuples are looped (`for (period, inserted) in [...]`).
- Tests `println!` a diagnostic line and then assert with a message naming the parameter.
- Ignored tests use `#[ignore = "reason"]` and a module doc giving the exact command and environment variables (`V90_CAPTURE`, `V90_START`).
- Each test file is self-contained, with helpers duplicated rather than shared.
- Integration tests reach only public API: `phase()` strings, counters and accessors. The modem-crate tests reach only the DTE bytes and line samples (`call.rs:1–6`).

---

## 13. Risks

1. **The model can pass while the rig fails.** The network's upstream is a loop model: filter, noise, network-clock sampling. Our real upstream is a softphone: verbatim samples, packet slips, and unknown AGC, AEC and noise suppression. A V.92 that passes on today's `Network` can still fail on every live call. Model both paths before trusting a green suite.
2. **The transmit-timing design forks.** Resample the transmit to the network clock (loop), or never touch the samples (softphone). Choosing wrongly makes PCM upstream impossible on one of the two. This must be decided from evidence (ε and TRN1u behaviour, channel-1 captures), not assumed.
3. **Softphone processing on the capture path** (AGC, AEC, NS, VAD, CNG, PLC) can destroy upstream PCM without any of it being visible to us. MicroSIP's settings need checking before any live V.92 attempt.
4. **Transcoding paths** (Crazytel) make upstream PCM unusable. The V.34-upstream fallback, and the V.34 fallback in general with its known bugs, then carry the call. V.92 work must not regress it.
5. **Upstream slips are unobservable against a real server.** The server's recovery behaviour is unknown, so expect V.42 retransmission bursts or retrains that the simulation cannot predict.
6. **INFO parsing hazards.**
   - `Info0d::to_bits` clears bits 26:27.
   - `Info0.clock` swallows INFO0a's V.92 bits, in the opposite order to INFO0d.
   - `Info1aPcm::from_bits` drops Table 18.
   - `Info1c` bit 70 is meaningless from a V.90 server.
   - Getting any of these wrong silently turns V.92 into V.90, or worse, makes us choose PCM upstream against a V.90 server. Tests must cover both pairings.
7. **The DIL's 36-symbol alias weakness** (17 of 260 slip positions fail) carries straight into V.92, whose downstream DIL is unchanged.
8. **Harness quirks bite new tests.**
   - The delay is emitted up front, so the analogue modem runs ahead of the ticks.
   - `with_delay` and `with_clock` act on both directions.
   - `slips()` is downstream only.
   - `SLIP` is 160 even where comments say 10 ms.
   - `with_pad` has never been exercised.
9. **Runtime.** V.92 roughly doubles the both-ends matrix. Unoptimised CI will be slow, and the `Network::up` kernel is recomputed on every tick.
10. **Modem-on-hold against the carrier watch.** The 2 s silence rules could clear down a held call unless `carrier::Watch` is suspended in `OnHold`.
11. **Test doubling.** Our digital modem is built from the same reading of the spec as our analogue modem. A both-ends-ours test cannot catch a shared misreading, so the V.92 vector and a real-server capture remain the only independent checks.

---

## 14. Open questions

1. What does `tests/vectors/v92-56k.wav` contain? V.92 with PCM upstream (INFO1a bits 34:36 = 6) or V.34 upstream (Table 19)? What are INFO0d bit 27 and INFO1d bit 70? Nothing reads the file yet.
2. Does Rory's upstream path deliver our samples verbatim to the far network (1-in-2 of our 16 kHz output, no AGC or AEC), and at which phase? The steady 114 ppm drift seen downstream suggests continuous resampling somewhere on at least that direction. Is the upstream resampled too?
3. Does Crazytel transcode and low-pass the upstream as it does the downstream?
4. Which MicroSIP (PJSIP) processing can be turned off: AGC, AEC, noise suppression, VAD/CNG, PLC? Does it expose a PCMU-only codec setting that avoids the transcoder?
5. Can the rig place a call between two of our own SIP endpoints, so that our V.92 server can answer our V.92 client live and both ends' captures can be compared?
6. Should `startup::Analogue::new` become V.92-capable by default (recommended), or should V.92 be opt-in through `+MS=V92` only?
7. Should the network's softphone paths replace the modem-crate `Pair`'s ad hoc resamplers, so both crates share one model?
8. What upstream rate should the clean-network test demand? It is unknown until the precoder design exists. Start at "on the ladder and ≥ 24 000", and tighten once measured.
9. Is the `Network::up` sin/cos-per-tap cost worth fixing now, or only when V.92 tests land?
10. CI runs these suites unoptimised. Should the workspace set a dev or test opt-level for `datapump`?
