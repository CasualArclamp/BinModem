# V.92 planning: the V.90 digital modem as the code stands

This file maps the digital modem (the server side) in `crates/datapump/src/v90/`, the parts of
`crates/datapump/src/v34/` it runs on, and the simulated network the tests use. It then sets out
what V.92's PCM upstream needs from the digital side.

- **Code:** read on 2026-09-18, commit `a7678a0` plus the working tree. `docs/design/v92/` was
  untracked at the time. File and line references are `path:line`, relative to
  `crates/datapump/src/` unless another crate is named.
- **Spec:** T-REC-V.92-200011. Every bit position below was read from rendered PDF pages, never from
  the extracted text. Pages viewed: 11, 12, 23, 24, 25, 27, 28, 29, 30, 31, 32, 33, 34, 37, 38, 40,
  53, 54, 55 (renders are under the session scratchpad, `v92/pages/pNNN.png`). Other values are
  taken from the sibling digests in this folder and cited to them:
  - `spec-intro-transmitter.md`
  - `spec-phase3-procedures.md`
  - `spec-phase3-signals.md`
  - `spec-phase4-procedures.md`
  - `spec-phase4-signals-digital.md`
  - `spec-renegotiation-fpe.md`
- **Markers:** "[interpretation]" marks this digest's own reading, where the Recommendation does not
  say it.

---

## 1. Module map

### 1.1 The digital modem and what it stands on

| File | Lines | Role | Key items |
|---|---|---|---|
| `v90/mod.rs` | 258 | Constants and the downstream rate ladder | `POWER_LIMITS` :45 (Table 15/V.90), `power_limit` :51, `training_codeword` :63 (UINFO from INFO0d), `INTERVALS = 6` :71, `SYMBOL_RATE = 8000` :74, `RATE_STEP = (8000, 6)` :83, `SLOWEST`/`FASTEST` :86-87, `K_RANGE`/`S_RANGE`/`D_RANGE` :90-94, `table_2_has` :102, `largest_k` :109, `rate_for` :117, `bits_for` :122 |
| `v90/server.rs` | 133 | The digital modem behind a sound card and softphone: resamples line rate to and from 8 kHz | `SLACK` :37, `ours()` :42 (our INFO0d), `Line` :55, `Line::step` :86 |
| `v90/startup.rs` | 492 | Wrapper from the end of V.8: V.34 start-up with V.90 phase 2 inside, then V.90 phases 3 and 4, or V.34 | `V90_RETRAINS = 2` :23, `Status` :27, `Analogue` :41, **`Digital` :266**, `Digital::new` :284, `with_habits` :301, `Digital::status` :375, `Digital::step` :457 |
| `v90/digital.rs` | 1122 | **The digital modem, phase 3 onwards**: transmitter (`Source`) and receiver/control (`Modem`) | constants :41-72, `Status` :76, `Habits` :87, `Settings` :111, `Out` :148, `Source` :169, `Stage` :459, `Modem` :474, `upstream_rate` :1119 |
| `v90/sequences.rs` | 821 | Framed sequences: Jd, the DIL descriptor, CP/CPt, MP as V.90 pads it, and stream finders | `frame` :27, `unframe` :42, `put`/`get` :60/:66, `Jd` :93, `Descriptor` :150, `Cp` :277, `Finder<T>` :449, `DescriptorFinder` :531, `CpFinder` :547, `mp_bits` :570 |
| `v90/encoder.rs` | 542 | Downstream encoder (5.4/V.90) and its decoder | `Frame` :24, `Mapping` :46 (`best` :68, `from_cp` :78, `for_renegotiation` :89), `Encoder` :134 (`next_frame` :195), `Decoder` :236 |
| `v90/modulus.rs` | 263 | 6-interval modulus encoder and mapper | `Moduli = [u16; 6]` :29, `fits` :37, `capacity` :46, `encode` :62, `decode` :85, `Constellation` :102 |
| `v90/sign.rs` | 772 | Sign coding and spectral shaping (5.4.5, 5.4.6) | `Redundancy` :29, `Differential`, `Shaper`, `SignDecoder` |
| `v90/ucode.rs` | 258 | Table 1 derived from G.711 | `UCODES` :22, `Law` :30, `linear` :42, `level` :58, `octet` :68, `from_octet` :87, `amplitude` :102, `nearest` :111 |
| `v90/carrier.rs` | 169 | "Is the far end still sending" level watch, shared by both modems | `Watch` :40 (`feed` :78, `gone` :100) |
| `v90/network.rs` | 414 | **Simulated route** used by the V.90 tests | `Network` :41, `down` :234, `up` :305, `kernel` :345 |
| `v90/pcm.rs` | 1291 | The *analogue* modem's PCM receiver: a template for any V.92 PCM receiver | `Slicer` :133, `Heard` :178, `Receiver` :310 |
| `v90/dil.rs` | 638 | The analogue modem's DIL design and analysis: a template for "choose constellations from measurements" | `design` :71, `Analysis` :100, `choose` :405 |
| `v34/phase2.rs` | 1269 | Phase 2, with V.90's part in it (`Pcm`) | `Role` :34, `Pcm` :64, `Modem` :256, `v90` :378, `again` :403, `info0_bits` :420, `heard` :660, `settle_pcm` :957 |
| `v34/info.rs` | 688 | INFO sequences, including INFO0d and V.90's INFO1a | `crc` :49, `Info0` :163, `Info1c` :271, `Info1a` :316, `Info0d` :371, `Info1aPcm` :457, `Info` :505 |
| `v34/dpsk.rs` | 619 | Phase 2 DPSK; decides which INFO a frame is | `Side::lengths` :87, frame classification :373-381 |
| `v34/startup.rs` | 469 | V.34 start-up holder; the digital modem's phase 2 lives in it | `Modem` :32, `with_phase2` :55, `decline_pcm` :178, `restart_phase2` :183, `step` :191 |
| `v34/receiver.rs` | 1676 | V.34 QAM receiver; **the digital modem's whole upstream front end today** | `Slicer` :143, `Reference` :220, `Heard` :229, `Receiver` :375 |
| `v34/data.rs` | 1269 | V.34 data mode; `Decoder` is the digital modem's upstream data decoder | `Params` :44, `Decoder` :476 |
| `v34/training.rs` | 2173 | V.34 phases 3 and 4; the digital modem borrows two watchers from it | `RetrainWatch` :234, `SWatch` :605, `Watched` :615 |
| `v34/signals.rs` | 349 | V.34 S/PP/TRN/J, and `Reader` (descrambler plus differential decoder) | `Reader` :173, `E_BITS = 20` :23 |
| `v32.rs` | | Scrambler | `Scrambler` :385; `Mode::taps` :101: `Call` = (18, 23) = GPC, `Answer` = (5, 23) = GPA |
| `dsp/src/echo.rs` | 715 | Two-segment NLMS echo canceller. **Only V.32 uses it**; V.34 and V.90 have none | `EchoCanceller` :112, `watch_far_echo` :161, `set_adapting` :186, `process` :199, `EchoFinder` :320 |
| `dsp/src/complex.rs` | 267 | Least squares, used for training on known sequences | `least_squares` :142, `solve_hermitian` :167 |
| `dsp/src/resample.rs` | 259 | Windowed-sinc resampler, used by `server::Line` | `Resampler` :32 |

### 1.2 Who drives it

- **`crates/modem/src/lib.rs`**
  - `Pump::V90Server(Box<v90::server::Line>)` is declared at :108.
  - `start_pump` (:2256) picks `"V90S"` when V.8 agreed V.34 duplex and `pcm_role == Some(PcmRole::Digital)` (:2262).
  - It builds `v90::server::Line::new(self.fs, v90::server::ours())` at :2284.
- **`crates/datapump/tests/v90_call.rs`** drives the digital modem directly at 8 kHz over `Network`:
  - one tick (:47-55) is `net.up(&up)`, then `digital.step()`, then `net.down()`, then `analogue.step()` for each line sample;
  - `FullCall` (:142) does the same through `startup::Digital` and `startup::Analogue`.

```text
modem crate --(line samples, fs)--> server::Line --Resampler fs->8k--> startup::Digital
                                                                        |
                          +---------------------------------------------+
                          | v34::startup::Modem  (phase2::Modem::v90(Pcm::Digital(info0d), 8000))
                          |   phase 2 as V.34's CALL modem: tone B, INFO0d/INFO1d at 1200 Hz
                          |   -> Status::Done with an Info1aPcm   ==>  digital::Modem (phases 3, 4, data)
                          |   -> Status::Done with a V.34 Info1a  ==>  V.34 training::Modem (fallback)
                          +--> output x phase2_gain (phase 2 and V.34 only; V.90 codewords are not scaled)
```

---

## 2. `startup::Digital` (`v90/startup.rs:266-492`)

**Fields**
- `law`
- `v34: v34::Modem`, which holds phase 2 and any V.34 fallback
- `v90: Option<digital::Modem>`
- `phase2_gain`
- `failed_starts`
- `renegotiations`
- `habits`
- `connected_once`

**Construction (`new`, :284)**
- The law is taken from `info0d.a_law`.
- `phase2_gain = 10^((nominal_dbm0 - 3.17)/20)` (:287). The comment says a full-scale sine is +3.17 dBm0.
- Phase 2 is `phase2::Modem::v90(Pcm::Digital(info0d), digital::FS)` (:290), so it runs at 8000 Hz.

**`step` (:457)**
- **While `v90` is `Some`:**
  - Step it and forward its output unscaled (:458-472).
  - A retrain request (`take_retrain`), or a failure while `failed_starts < V90_RETRAINS`, drops `v90` and calls `v34.restart_phase2()`. That runs phase 2 again as a retrain (tone B, no INFO0).
- **Otherwise:**
  - Step V.34 (:474).
  - Once phase 2 reports `Done` with `info1a_pcm()` and `info1c()`, and training has not started, build `digital::Settings::new(law, &info1d, &asked, rtt, wide)` and `digital::Modem::new` (:478-487).
  - `wide` is the far INFO0a's `constellation_1664` (:483).
- Status mapping is at :375. `Running` after a first connection is reported as `Retraining` (:386, :393).

**Where V.92 plugs in**
- This is where the V.92 decision has to be made. The inputs are: the INFO0a V.92 bit, INFO1a Table 18 against Table 10 against Table 19, and short against full phase 2.
- `Settings::new` takes an `Info1aPcm` (V.90 Table 10). A V.92 PCM-upstream INFO1a (Table 18) cannot be turned into one (see 6.3).
- Proposal: `startup::Digital` builds an `Upstream` choice:
  - `Upstream::V34(v90 settings)`
  - `Upstream::Pcm(v92 settings)`
  - `v90: Option<Phase3>`, where `Phase3` is an enum over `digital::Modem` and a new `v92::digital::Modem`.

---

## 3. `digital::Modem`, the V.90 digital modem (`v90/digital.rs`)

### 3.1 Configuration

**`Settings` (:111), built by `Settings::new` (:129)**

| Field | Source |
|---|---|
| `law` | from INFO0d |
| `uinfo` | INFO1a bits 25:31 |
| `upstream: Band` | INFO1a symbol rate, plus the high-carrier bit INFO1d probed for that rate |
| `far_md` | INFO1a MD, in 35 ms steps |
| `round_trip` | phase 2 |
| `jd` | always `Jd::ALL_RATES`, 4-point CP/E/SCR, look-ahead 1 (:139) |
| `wide` | 1664-point upstream allowed |
| `habits` | see below |

**`Habits` (:87)**
- `trn1d` seconds and `s_wait_counts_round_trip`.
- `PROMPT` is 0.3 s and counts the round trip.
- `LIVE_SERVER` is 4.05 s and does not count it; it models a real server.

**`Status` (:76)**
- `Running`
- `Connected { downstream, upstream }`
- `ClearedDown`
- `Failed(&'static str)`

### 3.2 Transmitter: `Source` and the `Out` state machine (:148-455)

`Source::next_level` (:286) is one loop over `Out`.

Transitions happen in three ways:
- **automatically**, when a length is reached;
- **through `pending`**, set by `change()` (:274) and taken at the next legal boundary;
- **through `after_jd_prime`** (:179), which says where J'd goes next.

`symbol` counts symbols since Sd began, so `symbol % 6` is the downstream data frame interval (:174, :270).

| `Out` | Output, per the code | Ends |
|---|---|---|
| `Silence` | 0.0 | when `pending` is taken (:290-296) |
| `Sd` | pattern `[(w,+),(0,+),(w,+),(w,-),(0,-),(w,-)]` with `w = 16 + uinfo` (:287, :305). `start(Sd)` sets `symbol = 0` (:249) | after `SD_FRAMES * 6` = 384T, goes to `SdBar` (:299-301) |
| `SdBar` | the same pattern, signs inverted | after `SD_BAR_FRAMES * 6` = 48T, goes to `Trn1d` |
| `Trn1d` | UINFO, sign = `scrambler.scramble(true)`, scrambler reset at start (:252, :319) | after `trn1d_symbols`, at a frame boundary, goes to `Jd` (:311) |
| `Jd` | UINFO, `sign ^= scramble(bit)` (:336); bits from `Jd::to_bits` repeated (:333) | at a Jd boundary, takes `pending` (J'd) (:329) |
| `JdPrime` | 12 zeros the same way (:253) | goes to `after_jd_prime` (`Dil` or `Ri`) (:324-327) |
| `Dil` | `(u, sign)` from `descriptor.symbols()` (:354) | takes `pending` at a segment boundary (`dil_ends`, :347); an empty DIL goes straight to `Ri` |
| `Ri` / `RiBar` | UINFO, signs `+++---` by interval, inverted for bar (:380-383) | `Ri`: at least `RI_SYMBOLS` = 192, then `pending`. `RiBar`: 4 frames, then `Trn2d` (:360-374) |
| `Rd` / `RdBar` | per-interval largest code of the last data-mode CP, `r_codes` (:381, :776) | `Rd`: at least 384T, then `RdBar`; then 4 frames, then `Trn2d` |
| `Trn2d` | encoder frames of ones; scrambler and encoder reset at start (:254-260) | after `TRN2D_FRAMES` = 340 frames, goes to `Mp` (:402) |
| `Mp` | encoder frames carrying `mp_bits(mp, D)`. MP' once `mp_ack` is set. Counts `mps_sent` and `acknowledged` (:403-420) | at an MP boundary, takes `pending` (`Ed`) |
| `Ed` | 2 frames of zeros | goes to `B1d` (:421) |
| `B1d` | 48 frames of ones; scrambler reset, and a new encoder from `data_mode` (:261-264) | goes to `Data` (:422) |
| `Data` | encoder frames of user data, padded with ones (:446) | takes `pending` (`Rd`) at a frame boundary (:423) |

- Frames come from `make_frame` (:436): D bits in, `Encoder::next_frame` with the GPC scrambler, amplitudes scaled to /32768.
- **V.92 downstream reuse.** V.92 clause 5 is V.90 clause 5, so `Encoder`, `Mapping`, `sign` and `modulus` (6 intervals) carry over unchanged for downstream data. The start-up sequence around them changes (section 8).

### 3.3 Receiver and control: `Modem` and the `Stage` state machine (:459-1111)

`Modem::step(input)` (:718) runs these in order:
1. `rx.feed(input)`
2. the carrier `Watch`, in data mode and in renegotiations (:721-733)
3. `RetrainWatch` for tone A (:735)
4. drain `rx.heard()` into `heard()` (:738)
5. the MD wait timer (:743)
6. the overall deadline, which sets `wants_retrain` (:749)
7. `stage_step()`
8. `source.next()`

| `Stage` | Entered | Receiver doing | Leaves on |
|---|---|---|---|
| `AwaitS` | `new` (:529); `rx.hunt()` (:525) | hunting V.34 S and S-bar | `Heard::Reversal`. If `far_md > 0`, first wait `0.035 * far_md` s with `rx.idle()`, then hunt again (:900-906). Then `rx.train(PpThenTrn, Mode::Answer, at)` and go to `Training` |
| `Training` | | least-squares solve on PP+TRN | `Heard::Trained{snr}`: store `phase3_snr`, go to `ReadJa` (:884-891). `Heard::Untrained` means fail |
| `ReadJa` | | `reader.trn(Size::Four)` until the first bit that is not a one after 24 symbols (:947-957), then `reader.differential` into `DescriptorFinder` | a descriptor arrives: `heard_ja` (:1010). DIL symbols are built, `change(Sd)`, `rx.hunt()`, go to `SendJd` |
| `SendJd` | | hunting S | `Heard::S` sets `s_heard` (:881). When `Jd` is going out: `after_jd_prime = Dil or Ri`, `change(JdPrime)`, go to `AwaitFirstReversal` (:833-839). No S within `S_WITHIN` (+RTD, per habit) means retrain (:840-844) |
| `AwaitFirstReversal` | | hunting S-bar | with a DIL: go to `AwaitSecondReversal`, `rx.hunt()`. With no DIL: `begin_phase4` (:910-919) |
| `AwaitSecondReversal` | | hunting S-bar again | `change(Ri)`, `begin_phase4` (:920-925) |
| `Phase4Cpt` | `begin_phase4` (:932): `rx.resume(s_bar + 32 halves)`, 4 or 16 points | `reader.differential` into `CpFinder` | CPt (`!data_mode`): `Mapping::from_cp`, make MP, `change(RiBar)`, go to `Phase4Cp` (:1033-1044) |
| `Phase4Cp` | | CP, E (20 ones after a CP, :997), S/S-bar in renegotiations | CP (`data_mode`): `data_mode` mapping, `mp_ack = true` (:1053-1062). Once an MP' has been sent and CP' or E heard: `change(Ed)` (:852-856). **E**: `heard_e` (:1065) builds the upstream decoder, goes to `Data` |
| `Data` | | `UpstreamDecoder`, plus `SWatch` for the far S and S-bar | `Connected` once `Out::Data`, B1 skipped and the decoder present (:859-868). S from the far end: `begin_renegotiation(false)` (:970) |
| `Finished` | `fail` / `cleared_down` | | nothing more |

**Renegotiation and cleardown**
- **`begin_renegotiation`** (:767) handles both initiating and responding:
  - `Mapping::for_renegotiation(cpt, cp)`;
  - `r_codes` from the last CP;
  - `change(Rd)`;
  - a deadline of `RENEGOTIATION_E + 2*RTD + Rd length` (:794-796).
- **`renegotiate(upstream)`** (:678) caps the analogue modem's upstream rate through `upstream_cap`.
- **`clear_down`** (:689) sends an MP with `answer_to_call = 0` (:1100). Once `CLEARDOWN_MPS` = 2 have been sent, the modem goes to `ClearedDown` (:846-849).
- **`clamp`** (:800) drops the decoder when the far S arrives during a renegotiation.

**The MP this end sends (`make_mp`, :1089)**
- Upstream rate = `floor(log2(1 + SNR/10^0.6) * baud/2400)`.
- It is clamped to `[2, probe::ceiling(rate)]`, to 12 if not `wide`, and to `upstream_cap` if set.
- Trellis is 16-state, `non_linear = false`, `precoding = None`, rate mask up to 14 with bit 0 cleared.

**`upstream_rate`** (:1119) is the CP upstream mask shifted left by one, ANDed with the MP mask, then the highest rate at or below `answer_to_call`.

**Retrain triggers**
- tone A (`RetrainWatch::new(Role::Answer)`, 2400 Hz, held 55 ms; `training.rs:212-268`);
- the overall deadline: 15 s + 5 RTD from `new` (:566), or the renegotiation deadline;
- the S deadline in `SendJd`.

`startup::Digital` consumes the request through `take_retrain` (:660).

### 3.4 Constants V.92 touches (digital.rs)

| Const | Line | V.90 value | V.92 |
|---|---|---|---|
| `FS` | :42 | 8000.0 | Unchanged. Upstream is also 8000 symbols/s (6.2) |
| `S_WITHIN` | :45 | 5.1 s | Still 5100 ms + RTD from the start of TRN1d, now for **Su** (9.5.1.2.2, p.55) |
| `RI_SYMBOLS` | :48 | 192 | V.92 sends Ri until CPt or E1u arrives (9.5.1.1.12-13, p.54). The minimum is harmless, but V.92 does not cite it |
| `R_BAR_FRAMES` | :51 | 4 | Unchanged (8.6.5/V.92 points to 8.6.4/V.90) |
| `TRN2D_FRAMES` | :54 | 340 (2040T) | Unchanged minimum. V.92 has the digital modem send SUVd once it is ready to receive CPu (9.6.1.1.1; see `spec-phase4-procedures.md` D4-1) |
| `B1D_FRAMES`, `ED_FRAMES` | :57-58 | 48, 2 | B1d unchanged. Ed is per `spec-phase4-signals-digital.md` section 4 |
| `RD_SYMBOLS` | :61 | 384 | Rd unchanged. V.92 adds Rt and Rf (Rf: 384T; `spec-renegotiation-fpe.md` 2.3) |
| `RENEGOTIATION_E` | :65 | 5.0 s | Different V.92 timers (see `spec-renegotiation-fpe.md`) |
| `CLEARDOWN_MPS` | :68 | 2 | Becomes CPd with drn = 0 (Table 30 bits 22:26, p.40) |
| `SD_FRAMES`, `SD_BAR_FRAMES` | :71-72 | 64, 8 | Unchanged: Figure 10 shows 384T and 48T (p.53) |
| 15 s + 5 RTD | :566 | from INFO1a | V.92: B1u within **20 s + 6 RTD from the end of INFO1a** (9.6.1.2.1, per `spec-phase4-procedures.md` D4-R1) |
| MD step 0.035 | :903 | 35 ms | INFO1a Table 18 bits 18:24: MD in **276-symbol (34.5 ms)** steps (p.27). Table 19 (short phase 2, V.34 upstream) keeps 35 ms (p.28) |

---

## 4. `sequences.rs`: layouts and what V.92 changes

### 4.1 Shared framing (:19-68)

**Frame layout**
- `SYNC_ONES = 17`, `BLOCK = 16`.
- `frame(information)` writes: 17 ones, then for each 16-bit block a 0 start bit and the block, then a 0 start bit and the 16-bit CRC.
- The CRC is `v34::info::crc`, over the information bits only.
- `unframe(bits, blocks)` checks the sync, every start bit and the CRC.
- `put`/`get` are LSB-first.

**CRC (`v34/info.rs:49`)**
- Register preset to 0xFFFF.
- Taps as in `shift` (:54): feedback into cells 15, 10 and 3; polynomial x^16 + x^12 + x^5 + 1.
- No inversion on output.

All of the V.92 framed sequences use this same structure, so it can be reused as it is: Jd, Jp, the Ja descriptor, CPt, CPu, CPus, SUVu, SUVd and CPd. It is Table 30's word format, a start bit plus 16 bits.

### 4.2 `Jd` (:93-145)

**Layout today** (72 bits, V.90 Table 13)

| Bits | Content |
|---|---|
| 0:16 | sync |
| 17 | start |
| 18:33 | rates 28 000 to 48 000 |
| 34 | start |
| 35:40 | rates 49 333 to 56 000 |
| 41:46 | reserved |
| 47 | `sixteen_in_training` |
| 48 | `sixteen_in_renegotiation` |
| 49:50 | look-ahead |
| 51 | start |
| 52:67 | CRC |
| 68:71 | fill `0000` |

`JD_BITS = 72` (:108) and `JD_PRIME_BITS = 12` (:111).

**Changes in V.92**

- **V.92 Jd (Table 21, p.31)** has the same layout, except:
  - bit 47 is the **Jd/Jp identifier** (0 = Jd);
  - bit 48 is **reserved**, 0.

  Our `to_bits` already writes 0 in both, because `Settings` sets both booleans false (:139). But `from_bits` reads information bit 28 (absolute bit 47) as `sixteen_in_training` (:135). **A V.92 Jp parsed as a Jd therefore reads as "16-point" with garbage rates.** The analogue side has to check bit 47 first.
- **Jp (Table 22, p.32)**, also 72 bits:

  | Bits | Content |
  |---|---|
  | 0:16 | sync |
  | 17 | start |
  | 18:33 | **ε**, a 16-bit unsigned fraction of T, [0, T) |
  | 34 | start |
  | 35:46 | reserved 0 |
  | 47 | = 1 |
  | 48 | TRN2u/CPu/E2u/SUVu constellation in **training**: 0 = 4-point, 1 = 8-point |
  | 49 | the same during **rate renegotiation** |
  | 50 | reserved 0 |
  | 51 | start |
  | 52:67 | CRC |
  | 68:71 | fill `0000` |

- **Signalling.** Jp and Jp' are sent like Jd: scrambled with GPC, differentially encoded, sent as the sign of UINFO, sign bit 0 = negative.
  - The differential encoder for Jp starts from the last Jd symbol (8.6.3).
  - For Jp' it starts from the last Jp symbol (8.6.4).
  - Jp' is 12 zeros.
- **Proposal:** a `Jp { epsilon: u16, eight_in_training: bool, eight_in_renegotiation: bool }` with `to_bits`/`from_bits`, sharing a private `j_frame(information)` helper with `Jd`. `Jd::from_bits` should return `None` when bit 47 is 1, or a `J` enum should dispatch on it.

### 4.3 `Descriptor`, the DIL descriptor that Ja carries (:150-268)

**Today (V.90 Table 12)**

| Information field | Content |
|---|---|
| block 0 | N (8 bits), reserved (8) |
| block 1 | LSP-1 (7), rsv, LTP-1 (7), rsv |
| then | SP, then TP, each padded to 16 bits |
| then | H1..H8, REF1..REF8, then the N training Ucodes, each as 7 bits plus a reserved bit, two per block, padded |

After that come the CRC, a fill bit 0, and one more fill bit if needed to make the length even (:237-241).

`descriptor_length` (:505) gives `SYNC + (blocks + 1) * 17` with `blocks = 2 + ceil(LSP/16) + ceil(LTP/16) + 8 + ceil(N/2)`.

**V.92 Table 20 (p.29)**
- Everything up to and including the start bit at 187 + β + ⌈N/2⌉·17 is as V.90.
- Then **two more words** follow:
  - bits 188..203 (+offset): a 16-bit **upstream rate mask**, bit 0 = 24 000 up to bit 15 = 44 000;
  - a start bit at 204;
  - bits 205..220: bit 0 = 45 333, bit 1 = 46 666, bit 2 = 48 000; the remaining 13 bits are reserved 0.
- Then a start bit at 221, the CRC at 222..237, fill 0 at 238, and **zeros up to a multiple of 12 bits** (not "even").
- **Ja** (8.5.4, p.29) is **24 ones followed by repeated descriptors**.
  - It uses TRN1u modulation: scrambled with GPA, differentially encoded, with differential memory from the last TRN1u symbol.
  - It is a whole number of 12-bit units long, and may stop in the middle of a descriptor.
- When N = 0 the descriptor is 276 bits.

**Proposal:** give `Descriptor` an `upstream_rates: Option<u32>` field (19 bits). `to_bits`/`from_bits` take a `Version` (V90/V92), and `descriptor_length` adds 2 blocks for V92. The CRC then covers the mask words.

### 4.4 `Cp`, CP and CPt as V.90 lays them out (:277-440)

**V.90 Table 14 as coded**

| Bits | Field |
|---|---|
| 18 | reserved |
| 19 | `data_mode` |
| 20:24 | `drn` |
| 25:29 | reserved |
| 30 | `silence` |
| 31:32 | Sr |
| 33 | ack |
| 34 | start |
| 35 | law |
| 36:48 | `upstream_rates` |
| 49:50 | ld |
| 52:67 | `trn1d_ratio` (Q3.13) |
| 69:76, 77:84, 86:93, 94:101 | a1, a2, b1, b2 (Q1.6) |
| 103:127 | six 4-bit interval indices |
| 128 | codec flag |
| 129:135 | reserved |
| 136 onwards | 8 blocks per mask |

`CP_HEADER_BLOCKS = 7` (:334) and `MASK_BLOCKS = 8` (:337). `to_bits` appends "000" fill (:392).

**V.92 Table 23 (CPt and CPu, pp.33-34)** differs **only in bits 18..48**. Bits 49 and up are the same as V.90.

| Bits | V.92 content |
|---|---|
| 18 | CP identifier = 0 (SUVu has 1 here, Table 27 p.37) |
| 19:20 | **type**: 0 = CPt, 1 = CPu |
| 21:25 | drn, 0..22. CPu rate = (drn+20)·8000/6; CPt rate = (drn+8)·8000/6 |
| 26:30 | reserved |
| 31:32 | Sr |
| 33 | ack (**CPd** received) |
| 34 | start |
| 35 | law |
| 36:48 | **reserved** (the upstream mask moved into Ja) |

- There is no silence bit; in V.92 it lives in SUV.
- CPt is preceded by 24 differentially encoded ones (8.5.1, p.28).
- **Danger:** `Cp::from_bits` run on a V.92 CPt does not fail. It reads `data_mode` from bit 19 and `drn` from bits 20:24, which is the V.92 type bit and drn shifted one place. For CPt (type 0, bit 20 = 0) that gives `drn_v90 = 2·drn_v92 mod 32`. `Mapping::from_cp` can still accept the result. **This is a silent wrong-rate bug waiting to happen.**
- **Proposal:** `Cp::from_bits_v92` and `Cp::to_bits_v92`, or a `Layout` parameter, mapping into the same `Cp` struct:
  - `data_mode = (type == 1)`
  - `silence = false`
  - `upstream_rates = 0`

  `cp_length` and the mask code are unchanged. `Mapping::from_cp` (`encoder.rs:78`) then works as it is for V.92 downstream.

### 4.5 `Finder<T>` (:449-502)

- A start is recorded where a 0 follows **exactly** 17 ones: `if !bit && self.ones == SYNC_ONES` (:466).
- **V.92 hazard.** Ja and the first CPt are each preceded by 24 ones, so the first sequence arrives behind 41 ones and is **skipped**. The test at :788 documents the same effect for CP.
  - Ja repeats, so the cost is one descriptor.
  - For CPt the cost is one CPt of delay before Ri.
- **Proposal:** `Finder::after_preamble()`, which accepts a run of 17 or more ones once, for the first sequence.

### 4.6 `mp_bits` (:570)

- V.34 MP, with V.90's reserved fields cleared, cut after the fill bit (86 bits for type 0, 188 for type 1), and padded to whole D-bit frames.
- **Not used when PCM upstream is selected.** CPd (Table 30) and SUVd (Table 31) replace MP.
- Still needed for V.92 short phase 2 with V.34 upstream (Table 19). [interpretation, per `spec-phase4-signals-digital.md` Q16]

### 4.7 New framed types the V.92 digital modem needs

Details are in `spec-phase4-signals-digital.md` and `spec-renegotiation-fpe.md`.

| Type | Direction | Notes |
|---|---|---|
| `Jp` | send | 4.2 |
| `Cpd` | send | Table 30. Variable length with presence flags 19/20/21; parse sequentially; 4G is unsigned Q0.16 in bits 35:50; pad to whole 6-symbol frames of D |
| `Suvd` | send | Table 31, one word; bit 18 = 1 |
| V.92 `Cp` (CPt/CPu) | receive | 4.4 |
| `Cpus` | receive | Table 24, short CPu |
| `Suvu` | receive | Table 27 (p.37-38): 18 = 1; 19:25 reserved; 26 = wait for CPu before CPd; 27:31 = 20·log10(RMS of G·v), signed Q2.2, where 16 means not measured; 32 = silence request; 33 = ack; 34 start; 35:50 CRC; 51 fill 0; fill to a multiple of 12 symbols |
| V.92 `Descriptor` | receive | 4.3 |

---

## 5. What the digital modem receives upstream today (V.34)

### 5.1 Phase 2 (`v34/phase2.rs`), at 8 kHz

**Role and signals**
- `Pcm::Digital(_)` makes the digital modem V.34's **call** modem (:75-80): tone B and INFO at 1200 Hz.
- It listens on 2400 Hz (`dpsk::Receiver::new(role.far())`, :337) for INFO0a, tone A and INFO1a.

**Sequence**
1. It sends INFO0d (`info0_bits`, :420). Bits 12 to 28 come from `capabilities()` (:488).
2. It measures the round trip from the reversals (:736-746).
3. It reads the analogue modem's L2 with `probe::Analyzer`, for about 300 ms (`L2_READ`, :122).
4. It sends L1/L2.
5. It sends INFO1d, which is INFO1c (:839-850), with `md_length: 0` and the probe results for all six symbol rates.
6. It finishes on `Info::Info1aPcm` (:716-720), or on a V.34 `Info1a` (:708).

**Which INFO a frame is** is decided in `dpsk.rs:373-381` by length:
- call side: 49 → Info0, 62 → Info0d, 109 → Info1c;
- answer side: 49 → Info0, 70 → `Info1a`, else `Info1aPcm`.

### 5.2 Phase 3 onwards: `v34::receiver::Receiver` at FS = 8000 (`digital.rs:524`)

**Front end**
- Mix down at the V.34 carrier.
- A 64-tap by 256-phase interpolating low-pass (:57-58); cutoff `min(0.5·baud·(1+rolloff)+300, 0.45·fs)` (:456).
- Half-symbol samples at the V.34 baud (3000, 3200 or 3429), timed by a second-order loop (`TIMING_GAIN` :138, `DRIFT_GAIN` :139).

**Hunt**
- S is detected by comparing each sample with the one two symbols back.
- S-bar is detected by a template (`Hunt`, :285).

**Training**
- Least squares (`dsp::least_squares`) over PP+TRN or TRN, with the known sequence generated from a zero-started scrambler of the far mode (`sequence`, :1228).
- The alignment search is ±8 half-symbols, widened to ±200 on a second try (:81, :86).
- 31 T/2 taps (`REACH = 15`, :62). Ridge is 1e-3 of the signal energy (:1280).

**Tracking**
- NLMS equaliser with `STEP = 0.02` (:127).
- Second-order carrier PLL.
- Timing from the error projected on the output slope.

**Slips**
- Detected by an error jump: `lost_threshold`.
- Handled by rewinding the loops (`rewind`, :997) and re-reading raw samples at fractional shifts (`resync` :1148, `resync_dense` :1027).

**Decisions**
- `Slicer::Points(4|16)` during training.
- `Slicer::Grid` in data mode, fed by `set_grid(decoder.grid_scale(), decoder.extent())` (`digital.rs:1077`).

### 5.3 How the digital modem reads bits from it

**Phase 3 and 4 sequences**
- **Ja:** `signals::Reader::new(Mode::Answer)` (GPA descrambler). `trn()` skips TRN, then `differential(decided, Size::Four)` (`digital.rs:947-963`).
- **CPt and CP:** the same differential reader at 4 or 16 points, per our Jd bits 47/48 (`cp_size` :940, `renegotiation_size` :813).
- **E:** 20 consecutive ones (`signals::E_BITS`), counted only after a CP (:997).

**Data mode**
- `v34::data::Decoder` (:1076) with:
  - `Framing::new(symbol_rate, rate, false, expanded_shaping)`
  - `Code::States16`
  - `nonlinear` per our MP
  - **precoding coefficients all zero**
  - `Mode::Answer`
- The first `framing.n` bits (B1) are discarded (:1078, :985-989).

**Watchers**
- `SWatch` (:968): the far S and S-bar, on the equalised points.
- `RetrainWatch` (:735): tone A, on raw samples.
- `carrier::Watch` (:723): level.

### 5.4 Limits of today's upstream

**Rate and coding**
- Upstream is at most 33 600 bit/s (14 × 2400), or 28 800 without `wide`.
- No precoding: our MP asks for none, so ISI is left to the receiver's linear equaliser.

**No echo canceller**
- The digital modem transmits downstream codewords at the same time as it listens. There is no canceller anywhere in `digital.rs`, and `network.rs` has no echo path to make one necessary. Real CO line cards reflect part of the downstream back into the A/D.
- A V.34 upstream at 16 or 32 points survives modest echo. PCM upstream will not (section 7.5).

**The sound-card path (`server::Line`)**
- The server's input is `Resampler(fs → 8000)` of whatever the softphone decoded (`server.rs:69`).
- For V.34 upstream the sampling phase does not matter: the receiver interpolates and has its own timing loop.

---

## 6. INFO sequences for V.92 (`v34/info.rs`)

### 6.1 INFO0d: `Info0d` (:371-447)

**Layout** (62 bits): bits 12:28 as INFO0a's, then:

| Bits | Content |
|---|---|
| 29:32 | nominal power |
| 33:37 | maximum power |
| 38 | power at codec |
| 39 | law |
| 40 | V.90 upstream at 3429 |
| 41 | reserved |
| 42:57 | CRC |
| 58:61 | fill |

**How the code handles bits 26:27**
- `to_bits` forces information bits 14 and 15 (absolute bits 26 and 27) to 0 (:407-408).
- `from_bits` sets `clock: 0` (:435).

**V.92 Table 15 (p.23-24)** fills those bits in. The rest of the table is unchanged (29:41, CRC 42:57, fill 58:61):
- **bit 26**: 1 requests **short phase 2**;
- **bit 27**: **V.92 capability**, = 1.

**Proposal:** add `short_phase2: bool` and `v92: bool` to `Info0d`. Write and read them at information bits 14 and 15. Stop forcing them to 0.

### 6.2 INFO0a (V.92 Table 16, p.24-25)

- Same 49-bit shape as `Info0`.
- **bit 26 = V.92 capability** and **bit 27 = short phase 2 request**. This is the **opposite order to INFO0d**.
- Today these land in `Info0.clock` (:188) as `clock = bit26 | bit27 << 1`.
- **Proposal:** a `V92Flags` view, `Info0::v92_flags(&self)`, used only where `Pcm::Digital` reads the analogue modem's INFO0a.
- Risk: a V.34-only INFO0 with `clock = 1` would read as V.92-capable. Gate on V.8's PCM negotiation.

### 6.3 INFO1a (V.92 Tables 18 and 19)

**Table 18, PCM upstream selected (p.27)**, 70 bits:

| Bits | Content |
|---|---|
| 12:13 | filter sections: 0 = p1, z2; 1 = z1, p1, z2; 2 = p1, p2, z2; 3 = z1, p1, p2, z2 |
| 14:15 | Ltot: 0 = 192, 1 = 256, 2 = 320, 3 = 384 |
| 16:17 | Lmax: 0 = 128, 1 = 192, 2 = 256, 3 = 320 |
| 18:24 | analogue MD, in **276-symbol (34.5 ms)** steps |
| 25:31 | UINFO (> 66; its power must not exceed the digital modem's maximum) |
| 32:33 | reserved 0 |
| **34:36** | **= 6** (analogue modem at 8000) |
| 37:39 | = 6 |
| **40:49** | **all ones** (reserved, set to 1 so no tone is generated) |
| 50:65 | CRC |
| 66:69 | fill |

- **Today's parsers reject it.**
  - `Info1aPcm::from_bits` requires bits 34:36 to be in 3..=5 (:490-493).
  - `Info1a::from_bits` fails on `SymbolRate::from_index(6)` (:356).
  - So `dpsk.rs:378-380` returns `None`, and the digital modem times out in `CallInfo1`.
- **Proposal:**
  - a new `Info1aPcmUp { sections: u8, ltot: u8, lmax: u8, md_length: u8, uinfo: u8 }` whose `from_bits` requires bits 34:36 = 6 and bits 37:39 = 6;
  - a new `Info::Info1aPcmUp` variant, tried in `dpsk.rs` after the other two;
  - a `phase2::Modem::info1a_pcm_up()` accessor, with a `heard` arm like :716.

**Table 19, short phase 2 with V.34 upstream (p.28)**
- Bits 12:17 reserved, 18:24 MD (35 ms), 25:31 UINFO, 32 reserved, **33 = high carrier**, 34:36 = symbol rate 3..5, 37:39 = 6, 40:49 frequency offset.
- `Info1aPcm::from_bits` parses it except bit 33.
- **Proposal:** add `high_carrier: Option<bool>` to `Info1aPcm`, meaningful only after a short phase 2. Full phase 2 takes the high-carrier choice from INFO1d probing (`digital.rs:130`); short phase 2 has no probing.

### 6.4 INFO1d (Table 17)

- Same as INFO1c (`Info1c`, :271). Page 25 shows 12:14, 15:17, 18:24 (the **digital** modem's MD, 35 ms steps) and 25 (high carrier at 2400) as before.
- Nothing new for the code, beyond the fact that it is only sent in full phase 2.

---

## 7. What a V.92 PCM upstream receiver needs

### 7.1 What it has to hear, phase by phase

Sources: Figures 10 and 11 (p.53), 9.5.1 (p.54-55), 8.5 (p.28-30), and the phase 4 digests.

| Phase | Upstream signal | Form at the CO A/D | Digital modem action |
|---|---|---|---|
| 3 | Ru 384T, R̄u 24T, (MD), Ru, R̄u | {+L,+L,+L,−L,−L,−L} repeated, R̄u inverted, precoder and prefilter bypassed (8.5.5) | detect Ru and the Ru-to-R̄u transition. MD from INFO1a: wait, then detect again (9.5.1.1.1) |
| 3 | TRN1u ≥ 2040T | ±LU; sign from the GPA scrambler fed ones, reset to 0 first; output 0 = **+LU** (8.5.7) | train the equaliser (9.5.1.1.2). After 2040T, read Ja (9.5.1.1.3) |
| 3 | Ja | 24 ones, then descriptors, TRN1u modulation, GPA, differential | DIL descriptor plus upstream rate mask. Ja deadline: 4500 ms + RTD from the end of INFO1a (9.5.1.2.1) |
| 3 | (silence) | none | **echo canceller window**: we send Sd, S̄d, TRN1d, Jd [interpretation] |
| 3 | Su 144T, S̄u 24.5T, Su | {+a, 0, +a, −a, 0, −a}, a = √(3/2)·LU; S̄u inverted; multiples of 12 symbols (8.5.6) | detect Su (deadline 5100 ms + RTD from the TRN1d start, 9.5.1.2.2). **Measure the sampling phase** on Su before and after the first S̄u (9.5.1.1.6-7). Then send Jp (9.5.1.1.8) |
| 3 | S̄u (24+ε)T, then TRN1u | S̄u lengthened by ε | on Su-to-S̄u: finish the current Jp, then send Jp' (9.5.1.1.9). **Keep a modulo-12 frame count** (9.5.1.1.10; 8.5.7 says from the first symbol of the *second* TRN1u) |
| 3 | TRN1u (≥ 2040T with a DIL), CPt ×n, E1u | TRN1u modulation; CPt preceded by 24 ones; E1u is one data frame of scrambled, differentially encoded zeros (8.5.1-8.5.2) | refine at the new phase; parse CPt, send Ri; on E1u send R̄i and go to phase 4 (9.5.1.1.12). With no DIL: SCR, then Ri once trained (9.5.1.1.13) |
| 4 | TRN2u (≥ 12000T unless SUVd arrives) | 4 or 8 points per Jp bit 48; scrambler reset; sign bit differential from E1u's last sign; multiple of 12 (8.7.6, p.38) | **estimate the upstream channel**; design CPd |
| 4 | SUVu, CPu, CPus, E2u | TRN2u modulation (data-mode modulation in FPE). E2u is 1 frame, plus 1 symbol if CPd bit 29 is set | SUV/CP/E exchange (D4-1..D4-6) |
| 4 | B1u, 48 frames | **full chain** with our CPd, all memories zero, first symbol is interval 0 and n = 0 (8.7.1, p.33) | condition for B1u, unclamp, data |
| data | data, RM/RM', Ru, tone A | data: full chain. RM: forced Ki patterns through the precoder, trellis coded (8.7.4, p.37). Ru: bypassed | decode. Watch for RR and FPE starts; retrain on tone A |

### 7.2 Front end

**What the input is**
- The input is **one sample per symbol**, the CO A/D's output.
- In a real server it arrives as octets. In our simulation it is a quantised level from `Network::up` (`network.rs:339`).
- There is no carrier to remove and no interpolation to do at this end. The analogue modem is required to put its symbols on the A/D instants (6.2 and Jp).

**Consequences** [interpretation]
- The equaliser is **T-spaced**. A fractionally spaced equaliser, as in `v34::receiver` and `v90::pcm`, is not possible, because the A/D sampled once per T.
- This is why Su and ε exist: the digital modem cannot move the sampling phase, so it asks the transmitter to move.

**Templates to reuse**
- **Hunt:** Ru and Su both have period 6. `pcm::Hunt` (`pcm.rs:234-296`) already detects a period-6 pattern and its inversion on half-symbol samples. At T-spacing, `PERIOD` becomes 6, not 12.
  - Su has zeros in positions 1 and 4, so it matches Sd's shape.
  - Ru is `+++---`.
- **Training:** TRN1u is known from its first symbol. It is the GPA sequence from a zero register fed ones; `spec-phase3-procedures.md` section 12 gives its first 48 signs.
  - Reuse the `solve_known` pattern (`v34/receiver.rs:786`): least squares at each alignment near the R̄u reversal, keep the best, and on a poor fit try a later stretch with a wide search. The wide search handles slips, as `pcm.rs` does with `TRAIN_TRIES`/`COARSE_SYMBOLS`.
  - The problem is real-valued. `dsp::least_squares` takes `Complex`; either pass real rows (im = 0) or add a real `least_squares_real` to `dsp`.
- **Equaliser:** a feed-forward section plus decision feedback, as in `pcm::Receiver` (`FEEDBACK = 24`, `pcm.rs:61`). This covers phase 3 and 4 decisions, which are 2, 4 or 8 levels. In data mode the transmitter's precoder and prefilter take over most of this (7.6).
- **Bit readers:** a `UpReader` (GPA descrambler plus differential sign):
  - TRN1u modulation: d(n) = (r(n) < 0); s = d(n) ⊕ d(n−1); bit = descramble(s).
  - The differential memory starts from the last TRN1u symbol.
  - TRN2u modulation: Tables 28 and 29 (4-point: 00 → +1/√5, 01 → +3/√5, 10 → −1/√5, 11 → −3/√5, times LU, p.38); only the sign bit is differential.
  - Bit order within a TRN2u symbol is open (`spec-phase4-procedures.md` Q2).
- **Finders:** `DescriptorFinder` with the V.92 length and the preamble fix (4.3, 4.5). `CpFinder` with the V.92 CP layout. A new `SuvFinder`: bit 18 separates SUV (1) from CP (0).
- **E detection:** E1u and E2u are zeros, not V.90's 20 ones.
  - A CPt mask word can be 17 zeros (start bit plus an empty Uchord mask), so a plain run of zeros is not enough.
  - [interpretation] Look for E only at a sequence boundary: after a CPt or CPu has been parsed, the next unit is E if it is not the 17-ones sync.

### 7.3 Sampling phase (ε) and clock

**The measurement**
- On Su, the digital modem estimates where the A/D samples fall within the analogue modem's symbol.
- It reports ε as a 16-bit fraction of T in Jp bits 18:33 (p.32).
- The analogue modem lengthens S̄u by εT (9.5.2.1.8, p.55).
- The first S̄u is 24.5T (9.5.2.1.7), so the Su after it is offset by half a symbol. That gives the estimator two views, T/2 apart. [interpretation]
- **The estimator is not specified.**
  - Proposal: fit the known Su pattern {+a, 0, +a, −a, 0, −a} through a short estimated channel at each trial phase.
  - Alternative: use the T-spaced equaliser trained on TRN1u. Its centre-tap asymmetry gives the phase, much as `pcm.rs` uses `centre` and `CENTRE_EVERY` to steer its clock (:128-129).
  - Choose ε to put the main tap's peak on a sample.

**Drift**
- The network clock is the only clock. The analogue modem must slave its transmit clock to what it recovers downstream (`pcm::Receiver::drift_ppm`, `pcm.rs:506`).
- The digital modem can only *measure* drift, as the slow walk of its equaliser's centre. It has no in-band way to correct it except FPE or retrain (9.9, 9.7). [interpretation]
- Proposal: an `UpClock` monitor that tracks the centre walk. When it exceeds a threshold, the modem runs FPE with a redesigned CPd, and after that a retrain.

### 7.4 Frame alignment

- The upstream data frame is 12 symbols, with constellation frame j = i mod 6 and trellis frame k = i mod 4 (Figure 1, p.11).
- The count is set at the second TRN1u (8.5.7, p.30), while 9.5.1.1.10 (p.54) says "from the first symbol of TRN1u". The two agree only if the count is carried modulo 12 across Su and S̄u. [interpretation: count from the first TRN1u and carry it]
- `spec-phase3-procedures.md` flags this.
- B1u starts at i = 0 (8.7.1). If CPd bit 29 was set, E2u is one symbol longer, so B1u moves by 1T.
- **Proposal:** a `symbol: u64` counter in the receiver, like `Source::symbol` (`digital.rs:174`), with `interval() = symbol % 12`. Add a known-sequence re-check at B1u, since B1u is scrambled ones with every memory at zero, so it is fully predictable from CPd.

### 7.5 Echo canceller

**The problem**
- Real CO line cards send part of the downstream D/A output back into the A/D through the hybrid.
- The analogue modem's hybrid and any loop discontinuities add later reflections.
- The upstream A/D sees u(n) + echo + noise, and then quantises it.

**The design** [interpretation]
- Use `dsp::EchoCanceller` (`echo.rs:112`) at 8 kHz, with `transmitted` = our downstream level (exact: we chose the codeword) and `received` = the A/D sample.
  - The **near** run spans the codec, filters and hybrid, a few ms: 32 to 64 taps.
  - A **far** run through `watch_far_echo` covers the loop reflection, placed by `EchoFinder` within the RTD measured in phase 2.
- **Training** (9.5's title names echo canceller training; the text gives no procedure): the analogue modem is silent from the end of Ja to Su (Figure 10). We send Sd, S̄d, TRN1d and Jd in that window. Adapt there with `set_adapting(true)`, then hold.
- **Data mode:** adapt slowly on the decision-directed residual, (received − ê) − decided level.
- **Limitation:** echo is added **before** quantisation, so cancelling it afterwards cannot restore the exact codeword. Keep the residual well under half the upstream level spacing.
- **Server on a softphone:** the "echo" is the far CO hybrid seen through two jitter buffers, about 1.1 to 1.5 s later (project memory note "VoIP line round trip": about 1.5 s each way round). It can slip. `EchoFinder` has to search that far: at 8 kHz that is about 12 000 samples of history.

### 7.6 Precoder and prefilter design, which becomes CPd

The digital modem **designs** the analogue modem's transmitter. The structure is fixed; the design method is not.

**The transmitter being designed** (6.4.2, rendered p.12; Figure 2, p.11)
- x(n) = u(n) + Σ_{κ=1..LZ1} u(n−κ)·z1(κ) + Σ_{κ=1..LP1} x(n−κ)·p1(κ)
- v(n) = Σ_{κ=0..LZ2−1} x(n−κ)·z2(κ) + Σ_{κ=1..LP2} v(n−κ)·p2(κ)
- The output is G·v(n).
- u(n) is chosen from an equivalence class:
  - k = 0, 1, 2: η = Ki + z·Mi;
  - k = 3: η = 2Ki + 2z·Mi + ((η0 + η1 + η2 + Y0) mod 2).
- Points a(η) are indexed −N/2 ≤ η < N/2 in level order, N = 2·LC of the interval's set.

**Constraints** (INFO1a Table 18, p.27; CPd Table 30, p.40)
- Allowed sections: bits 12:13. LZ1 and LP2 must be 0 unless allowed.
- LZ1 + LP1 + LZ2 + LP2 ≤ Ltot; each ≤ Lmax.
- 2^K ≤ ΠMi with K = 2·(drn+17) (`spec-intro-transmitter.md` 6.1).
- Sets of at most 128 points, with no zero point, and non-empty sets listed first.
- When G·v has mean square 1, the analogue modem transmits at the desired power (p.40).
- NOTE (p.40): design assuming the analogue modem minimises |x(n)| symbol by symbol.

**Proposed design pipeline** [interpretation throughout]

1. **Channel estimate `h`**, T-spaced, from the transmit symbol to the A/D sample. Taken from TRN2u: 4 or 8 known levels, a scrambler reset at its start, and a sign memory from E1u, so it is fully known. Least squares against the received samples, after echo cancellation. TRN1u gives a first estimate. The noise floor comes from the residual.
2. **Target.** Choose a monic causal response `t(D) = (1 − P1(D))/(1 + Z1(D))`, and a prefilter `P2(D) = Z2(D)/(1 − P2fb(D))`, so that `G·LU·h(D)·P2(D) ≈ t(D)`. The A/D then sees u(n) directly. This is transmit-side equalisation.
   - Everything in front of the A/D is equalised before quantisation. The digital modem's decisions become decisions about **which codeword** arrived, with no receive filter to colour the A/D quantisation noise. That appears to be the point of the structure.
   - The problem is ill-conditioned at DC (codec high-pass, line transformer) and near 4 kHz (codec anti-alias filter). The precoder's feedback section (p1) takes the spectral nulls, and the class choice keeps x bounded.
   - Standard route: MMSE-DFE on `h`. Its feed-forward goes to z2 (and p2 if allowed). Its feedback `B(D)` becomes `p1(κ) = −b_κ`, or an ARMA fit into z1/p1 if the sections allow.
3. **Constellations.** Choose, per constellation frame index j (6 sets, used for i and i+6), the set of positive magnitudes a(η).
   - Natural choice: **the codec's own levels** (`ucode::linear`) at a spacing the measured noise allows, as `dil::Analysis::constellation` (`dil.rs:206`) and `dil::choose` (`dil.rs:405`) do downstream.
   - With robbed-bit signalling upstream, the robbed j gets a sparser set. This is what per-j sets are for.
   - The "linear value" scale is not defined (`spec-phase4-signals-digital.md` Q1). Calibrate against a capture.
4. **Moduli and rate.** Mi ≤ N (and ≤ N/2 at k = 3) (`spec-intro-transmitter.md` 6.4.2). Pick drn as the highest rate that is both in the Ja mask and satisfies 2^K ≤ ΠMi with margin. `dil::fits_with_room` (`dil.rs:344`) is the downstream analogue.
5. **Gain G.** Predict E[v²] for the precoder's output statistics. By simulation, run the actual transmitter model (7.8) on random data. Then set G so that E[(G·v)²] = 1, and quantise 4G to unsigned Q0.16 (G = raw/262144, `spec-intro-transmitter.md` P-10).
6. **Verify** by running our own model of the analogue transmitter (7.8) through `h` and checking decisions before sending CPd. This is cheap and catches Q-format and sign mistakes.

**Cost**
- Up to 384 taps; a dense solve is O(n³), about 6·10⁷ flops, once per training. Acceptable.
- Keep the design off the per-sample path: compute it when TRN2u has been read, before the first SUVd.

### 7.7 Data-mode decoder (new)

For each symbol n, with interval i = n mod 12, j = i mod 6, k = i mod 4:

1. r(n) = A/D sample − ê(n). A small residual FFE is optional.
2. **4D Viterbi** over V.34's 16/32/64-state codes (`v34::trellis::Code`, clocked once per 4 symbols, with the "2T" delays becoming 4T; 6.4.4).
   - Subset label of a 2D pair = Figure 9/V.34 on (2·η_a+1, 2·η_b+1), which depends only on (η_a mod 4, η_b mod 4) (`spec-intro-transmitter.md` 6.4.3).
   - Per dimension and residue r ∈ {0..3}: the nearest level of set j with η ≡ r (mod 4).
   - Label metric: the minimum over the label's two residue pairs.
   - The Y0 parity constraint ties the four indices.
   - **No** V.34 modulo encoder or superframe inversion (`spec-intro-transmitter.md` Q-15).
   - `v34::data::Decoder` (`data.rs:476`, `DEPTH = 40`) is tied to V.34's grid. Write a generic `Viterbi4d` that takes a closure for the metrics.
3. **Ki from η**:
   - k < 3: Ki = η mod Mi (non-negative);
   - k = 3: Ki = ((η − p)/2) mod Mi, with p = η mod 2.
4. **12-interval modulus decode (u128)**:
   - R0 = Σ Ki·Π_{j<i} Mj;
   - R = R0 if d_prev = 0, else M−1−R0;
   - s = (2R > M−1); d = s ⊕ d_prev;
   - output K bits LSB-first.
   - Step 4 uses d(f−1) (rendered p.12). `v90::modulus::decode` handles 6 × u16 only; add `modulus::decode12` or a const-generic version.
5. GPA descramble (`Scrambler::new(Mode::Answer)`, `v32.rs:392`).

**Watchers in data mode**
- **RM and RM'.** Forced Ki patterns (Tables 25 and 26; RM' on p.37: K0 = 0, K1 = 0, K2 = M2−1, K3 = M3−1, and so on, ending K11 = M11−1). These are data-mode symbols, so detect them on decoded Ki per frame.
  - This replaces V.90's `SWatch` (`digital.rs:508`, :967-981) with an `RmWatch`.
- **Ru.** Bypassed, so it looks like ±LU through the raw channel: a period-6 hunt on raw samples.
- **Tone A.** `RetrainWatch` works unchanged at 8 kHz.
- **Carrier.** `carrier::Watch` works unchanged.

### 7.8 A shared transmitter model

- The digital modem needs a bit-exact model of the analogue transmitter's precoder and prefilter, for design (7.6), for verification, and for the network tests.
- The analogue side needs the same code to transmit.
- **Proposal:** put it in one module, `v92::precoder` (`Precoder`, `Prefilter`, `Chain`), used by both sides.
  - Keep x and v in f64, as `spec-intro-transmitter.md` P-12 says.
  - The point selector is a parameter, defaulting to minimum |x|.

---

## 8. Proposed V.92 digital modem: states, types and modules

### 8.1 Placement

- V.92's downstream is V.90's. Its start-up, upstream and phase 4 are new.
- **Proposal:** a sibling module `crates/datapump/src/v92/` that `use`s `v90::{encoder, modulus, sign, ucode, dil, sequences::{frame…}}`. Keep `digital.rs` as the V.90 (and V.92-with-V.34-upstream) modem.
- `digital.rs` is already 1100 lines of booleans (`s_heard`, `far_e`, `renegotiating`, `clearing`, `far_s_bar`, …). Folding PCM upstream into it would double its states.

| Module | Contents |
|---|---|
| `v92/mod.rs` | `UP_INTERVALS = 12`, `CONSTELLATION_FRAME = 6`, `TRELLIS_FRAME = 4`; upstream ladder `UP_SLOWEST = 24_000`, `UP_FASTEST = 48_000`, `up_rate(drn) = (drn+17)·8000/6` for drn 1..=19, `up_bits(drn) = 2·(drn+17)`; `MD_SYMBOLS = 276`; `FILTER_TOTALS = [192, 256, 320, 384]`, `FILTER_EACH = [128, 192, 256, 320]` |
| `v92/sequences.rs` | `Jp`, V.92 `Descriptor` extension, V.92 `Cp` layout, `Cpd`, `Suvd`, `Suvu`, `Cpus`, `SuvFinder` |
| `v92/upstream.rs` | the PAM receiver: `Receiver`, `Heard { Ru, RuReversal{at}, Trained{snr_db}, Su, SuReversal{at}, Phase{epsilon}, Symbol(Symbol), Lost, Found }`, `UpReader` |
| `v92/echo.rs` or direct use of `dsp::EchoCanceller` | canceller wiring and training window |
| `v92/precoder.rs` | shared transmitter model (7.8) |
| `v92/design.rs` | `Channel` estimate, `design(...) -> Option<Cpd>` (7.6) |
| `v92/modulus.rs` | 12-interval u128 encode and decode with the differential step |
| `v92/decoder.rs` | `Viterbi4d`, `UpDecoder` (7.7), `RmWatch` |
| `v92/digital.rs` | the modem (8.2, 8.3) |
| `v92/startup.rs`, or extend `v90/startup.rs` | full and short phase 2 choice; V.34 upstream fallback |
| `v92/hold.rs` (later) | MH sequences (see `spec-modem-on-hold.md`) |

### 8.2 `Out` for the V.92 digital modem

- **Keep:** `Silence`, `Sd`, `SdBar`, `Trn1d`, `Jd`, `Dil`, `Ri`, `RiBar`, `Rd`, `RdBar`, `Trn2d`, `Ed`, `B1d`, `Data`.
- **Add:**
  - `Jp` (repeated, carrying ε; from `pending` at a Jd boundary);
  - `JpPrime` (12 zeros; then `after_jp_prime`);
  - `Scr` (UINFO signed by GPC-scrambled ones; no scrambler reset; whole 6-symbol units; 8.6.6, p.32);
  - `Suvd` and `Cpd` (framed, in TRN2d modulation, or data-mode modulation in FPE; padded to D-bit frames);
  - `Rt`/`RtBar` and `Rf`/`RfBar` (`spec-renegotiation-fpe.md` 2.2-2.3);
  - `Quiet` (Ucode-0 frames with frame count kept, for RR silence; 9.8.1.1.3).
- **Remove from the V.92 path:** `Mp`, with its `mps_sent` and `acknowledged` bookkeeping. The SUV/CP exchange replaces it.

### 8.3 `Stage` for the V.92 digital modem

| Stage | Receiver | Exit |
|---|---|---|
| `AwaitRu` | silent; hunt Ru and the transition (9.5.1.1.1) | transition; MD > 0 → `WaitMd` |
| `WaitMd` | idle for MD × 276 symbols | → hunt again, then `TrainTrn1u` |
| `TrainTrn1u` | least squares on TRN1u (9.5.1.1.2) | trained → `ReadJa` after 2040T (9.5.1.1.3) |
| `ReadJa` | `UpReader` → V.92 `DescriptorFinder` | descriptor → (≤ 500 ms) `change(Sd)` → `SendJd`. Ja deadline: 4500 ms + RTD from the end of INFO1a |
| `SendJd` | **echo canceller adapting**; hunt Su | Su → `MeasurePhase`. No Su within 5100 ms + RTD from TRN1d → retrain |
| `MeasurePhase` | estimate ε across Su, S̄u (24.5T), Su (9.5.1.1.6-7) | ε ready → `change(Jp)` at a Jd boundary → `SendJp` |
| `SendJp` | hunt Su to S̄u | transition → finish Jp, `change(JpPrime)`, assert 107 (9.5.1.1.9) → `DilOrScr` |
| `DilOrScr` | second TRN1u: start the mod-12 count, refine the equaliser | DIL: parse CPt → `change(Ri)` → `AwaitE1u`. No DIL: trained → `change(Ri)` → `AwaitCpt` |
| `AwaitCpt` / `AwaitE1u` | `UpReader` → V.92 `CpFinder`, E1u | CPt (no DIL) or E1u (DIL) → `change(RiBar)` → `Phase4Trn`. Keep CPt as the downstream training `Mapping` |
| `Phase4Trn` | read TRN2u (4 or 8 points per our Jp); build `Channel`; **design CPd** | ready → `change(Suvd)` (D4-1) → `Phase4Exchange` |
| `Phase4Exchange` | `SuvFinder` / `CpFinder` (CPu, CPus); E2u | the D4-2 to D4-4 exchange (`spec-phase4-procedures.md` 6.5): one CPd after the first SUVu; repeat if no ack by end-of-CPd + 100 ms + RTD; Ed once we have sent an ack and received one or E2u |
| `AwaitB1u` | E2u heard (with the optional extra symbol) → B1u known-sequence check | B1u → unclamp → `Data`. B1u deadline: 20 s + 6 RTD from the end of INFO1a |
| `Data` | `UpDecoder`, `RmWatch`, Ru hunt, tone A, carrier | RR, FPE, cleardown or retrain |
| `Renegotiation`, `FastExchange`, `Hold` | per `spec-renegotiation-fpe.md` and `spec-modem-on-hold.md` | |
| `Finished` | | |

**Settings**
- `v92::digital::Settings { law, uinfo, filters: FilterCaps { sections, ltot, lmax }, far_md_symbols, round_trip, jd: Jd, jp_sizes: (bool, bool), habits }`
- Built from `Info1aPcmUp` and `Info0d` (with `v92 = true`) plus INFO0a's V.92 flag.

### 8.4 Changes to existing code (small, all backward-compatible)

1. `v34/info.rs`
   - `Info0d` gets `short_phase2` and `v92` (6.1).
   - `Info0::v92_flags()` (6.2).
   - `Info1aPcmUp` and `Info::Info1aPcmUp` (6.3).
   - `Info1aPcm.high_carrier` (Table 19).
2. `v34/dpsk.rs:378-380`: also try `Info1aPcmUp`.
3. `v34/phase2.rs`
   - `Pcm::Digital` carries whether this end offers V.92.
   - A `heard` arm for `Info1aPcmUp`.
   - `info0_bits` writes the V.92 bits.
   - **Short phase 2** is a separate stage set; see `spec-phase2-procedures.md` section 6.
4. `v90/sequences.rs`
   - `Jd::from_bits` refuses bit 47 = 1.
   - `Finder` preamble tolerance.
   - A V.92 layout for `Cp` and `Descriptor`.
   - Make `frame`, `unframe`, `put` and `get` `pub(crate)` so `v92::sequences` can use them.
5. `v90/digital.rs`: none required for V.92 PCM upstream. If V.92 with V.34 upstream is supported (short phase 2, Table 19), `Settings::new` needs the INFO1a `high_carrier` in place of `info1d.probed`.
6. `v90/startup.rs::Digital::step` (:478-487): choose V.90, V.92 PCM or V.34 from phase 2's outcome.
7. `v90/modulus.rs`: a 12-interval u128 variant with the differential step, or const generics.
8. `dsp`: optional `least_squares_real`.
9. `v90/server.rs`: a codeword-phase tracker for PCM upstream through a sound card (9.2).

---

## 9. `network.rs` today, and what V.92 upstream needs from it

### 9.1 What it models today (`v90/network.rs`)

**Configuration: `Network::new(law, fs)`** (:93) plus builders

| Builder | Line | Effect |
|---|---|---|
| `with_delay(seconds, _fs)` | :125 | the same delay both ways; `_fs` is ignored |
| `with_clock(ppm)` | :132 | `skew = −ppm·1e-6` |
| `with_noise(level)`, `set_noise` | :138, :145 | one noise level, used for **both** directions |
| `with_robbed_bit(phase)` | :150 | **downstream only** (`carry`, :223) |
| `with_pad(db)` | :156 | **downstream only**; requantised (:229) |
| `with_upstream_gain(g)` | :162 | default 0.25 (:113) |
| `unquantised()` | :169 | |
| `with_slips(seconds, inserted)`, `with_slip_at` | :176, :192 | **downstream only** jitter-buffer slips of `SLIP = 160` codewords (:37); inserted audio is the last 20 ms, faded (:243-248) |
| `with_gain_control(ceiling, release)` | :186 | downstream only; models a softphone's playback limiter |

**`down(level)`** (:234)
- `carry`: nearest codeword → octet → robbed bit → back → pad.
- Then the jitter buffer.
- Then the D/A reconstruction: a windowed sinc at 3800 Hz, reaching ±`DOWN_REACH = 20` codewords (:30, :282), sampled at the analogue clock (`step = (1+skew)·8000/fs`, :270).
- Then the gain control, then additive noise.
- Output: 0..n analogue samples.

**`up(samples)`** (:305)
- The analogue samples are scaled by `up_gain`, with noise added at the analogue rate (:311-314).
- One network sample is taken at t = (up_next − lag)·per, where per = fs/((1+skew)·8000) and lag = DOWN_REACH + 2 + UP_REACH·8000 (:319-322).
- Anti-alias: a windowed sinc at **3700 Hz** reaching ±`UP_REACH = 8 ms` (:34, :325-334).
- Then `quantise` to the nearest codeword of the **same law** (:213, :339).
- It returns a level (f64), not an octet.

**Tests** (:355-414): a codeword round trip, clock skew, slips, robbed bit.

### 9.2 Gaps for PCM upstream, with proposals

Each gap either makes the simulation easier than a real line, or leaves a V.92 mechanism untested.

| # | Gap | Why it matters for V.92 | Proposal |
|---|---|---|---|
| N1 | **A/D sampling phase is fixed.** At fs = 16 000 with no skew, `per = 2` and `lag` is whole, so every A/D instant lands on an analogue sample. | ε would always be about 0, and Su/Jp would never be exercised. | `with_adc_phase(fraction)`: a fractional offset added to `t`. Also `with_delay` in fractional samples. Test at fs = 44 100 and 48 000 |
| N2 | **No DC high-pass or G.712 band edges** on the A/D side (or the D/A side); the only filtering is a 3700 Hz sinc. | Precoder and prefilter design is hard because of the DC null and the 4 kHz roll-off. Without them the design problem is trivial. | `with_codec_filters()`: a high-pass (a few hundred Hz corner, e.g. `dsp::butter_highpass`) plus the existing low-pass, both directions. Make the up cutoff configurable |
| N3 | **No loop response.** The line is flat. | The channel `h` the digital modem has to equalise is only the codec filter. | `with_loop(biquads)`: attenuation and tilt on `up_samples` at fs, and on the down waveform |
| N4 | **No echo.** Downstream never reaches the A/D. | The echo canceller (7.5) is untestable, and a real server's A/D input is corrupted. | `with_hybrid_echo(db, delay_ms)`: add the reconstructed downstream waveform, scaled and delayed, into `up_samples` **before** the A/D quantiser. `with_far_echo(db, delay)` at the analogue side. The one-tick ordering (up before down) gives one sample of latency, which is fine |
| N5 | **Upstream digital impairments absent.** No robbed bit, no pad, no law conversion upstream. | RBS affects both directions of a T1, and V.92's per-j sets exist for it. A pad requantises the A/D codewords. A transcoder (the Crazytel path: mu-law decoded, low-passed and re-encoded as A-law, per the project memory note on it) moves every codeword. V.92 has **no upstream DIL** to discover any of this. | `with_upstream_robbed_bit(phase)` (independent phase), `with_upstream_pad(db)`, `with_transcoding(Law)` (both ways, decode, low-pass, re-encode, as observed) |
| N6 | **Upstream slips absent.** | A jitter buffer at a VoIP server side slips the upstream. For PCM upstream that loses the 12-symbol frame and the precoder state. | `with_upstream_slips(seconds, inserted)`, mirroring `down` |
| N7 | **One noise level for both directions**, added at the analogue rate on the up path. | Upstream and downstream margins are different questions. | `with_upstream_noise(level)`. Keep the current one as the default for both |
| N8 | **Returns levels, not octets.** | Lossless (`quantise` maps to exact codeword levels), but a V.92 decoder deciding on codewords is simpler and faster on `(Ucode, sign)`, and A-law has no zero. | `up_octet()` or `up_code() -> (u8, bool)` alongside `up()` |
| N9 | **Overload.** `quantise` saturates at Ucode 127, with `up_gain` 0.25. | The V.92 transmit level is set by LU and G. Precoder excursions can overload the A/D, which is real. | Keep saturation. Add a test that measures the overload rate at the designed G |
| N10 | **Same delay both ways.** | RTD asymmetry changes where the far echo sits relative to the round trip. | `with_delays(down, up)` |
| N11 | **Skew applies to both directions** through `per` (good). The analogue modem has to compensate. | This already exercises the analogue transmitter's clock slaving. | Add a test: V.92 upstream at ±120 ppm must hold frame alignment and SNR for 10 s, like `a_sound_card_clock_120_ppm_off_is_followed_through_ten_seconds_of_data` (`tests/v90_call.rs:453`) |

**`server::Line` (sound-card server).**
- For V.34 upstream the phase of `Resampler(fs → 8000)` does not matter. For PCM upstream the server has to sample at the **codeword instants** of the far softphone's decoded audio, or its "A/D output" is an interpolation between codewords.
- Proposal: a `CodewordPhase` tracker in `server.rs`. It searches the fractional delay at which samples land on the law's levels, and holds it with a slow loop, as `pcm.rs` does. It is also a useful check on whether the path is transparent at all (compare the memory note on the NetZero path arriving bit-exact at 82 dB).

---

## 10. Constants V.92 would touch, outside `digital.rs`

| Where | Const | Now | V.92 |
|---|---|---|---|
| `v90/mod.rs:71` | `INTERVALS` | 6 | Downstream unchanged. Add `UP_INTERVALS = 12` in `v92` |
| `v90/mod.rs:86-87` | `SLOWEST`, `FASTEST` | 28 000, 56 000 | Downstream unchanged. Upstream 24 000 to 48 000 |
| `v90/mod.rs:45` | `POWER_LIMITS` | Table 15/V.90 | Downstream power still applies (Table 18 bits 25:31 keep the UINFO power rule, p.27) |
| `v90/sequences.rs:72` | `DOWNSTREAM_RATES` | 22 | Unchanged (Jd, CPu drn 0..22) |
| `v90/sequences.rs:108, :111` | `JD_BITS`, `JD_PRIME_BITS` | 72, 12 | Same for Jp and Jp' |
| `v90/sequences.rs:334, :337` | `CP_HEADER_BLOCKS`, `MASK_BLOCKS` | 7, 8 | Same for the V.92 CP layout |
| `v90/sequences.rs:574` | MP fill ends 86 and 188 | | Not used with PCM upstream |
| `v34/info.rs:30` | `INFO0D_BITS` | 62 | Unchanged |
| `v34/info.rs:35` | `PCM_SYMBOL_RATE` | 6 | Now also in bits 34:36 (Table 18) |
| `v34/signals.rs:23` | `E_BITS` | 20 ones | E1u and E2u are zeros, one data frame |
| `v34/phase2.rs:97-168` | phase 2 timers | V.34/V.90 | Full phase 2 unchanged. Short phase 2 has its own (`spec-phase2-procedures.md` 6.6, 8) |
| `v90/network.rs:25, :30, :34, :37` | `NETWORK_FS`, `DOWN_REACH`, `UP_REACH`, `SLIP` | 8000, 20, 8 ms, 160 | Keep. Make the up cutoff (3700 Hz at :325) and `up_gain` (0.25 at :113) configurable |
| `v90/pcm.rs:50, :61` | `REACH`, `FEEDBACK` | 31 half symbols, 24 | Templates only. The upstream receiver is T-spaced |
| `v34/receiver.rs:57-139` | QAM receiver constants | | Not used by the V.92 PCM receiver. Still used if V.34 upstream is selected |

---

## 11. Risks

1. **Silent misparses.**
   - A V.92 CPt read by `Cp::from_bits` gives a doubled or shifted drn and can still produce a valid `Mapping` (4.4).
   - A V.92 Jp read by `Jd::from_bits` reads as 16-point (4.2).
   - A V.92 INFO0a is read into `Info0.clock` (6.2).

   Put a version or layout parameter on every shared parser, and test each against the other version's bits.
2. **Rejected INFO1a.** Table 18 fails both INFO1a parsers, so phase 2 times out with "no INFO1a" (6.3). This is the first thing a V.92 analogue modem calling our server will hit.
3. **Preamble ones hide the first Ja and CPt** (4.5). This costs a descriptor or a CPt of latency on a line with a 1.5 s round trip.
4. **E detection** changes from 20 ones to a frame of zeros that can look like mask words (7.2).
5. **The digital modem's design problem is unspecified.** Precoder, prefilter, G, constellations and moduli are all our invention (7.6). Several things are ambiguous:
   - the "linear value" scale and signedness;
   - the unsigned Q-range wording;
   - what an absent CPd part means;
   - the 128-point limit;
   - the E2u extension bit.

   See `spec-phase4-signals-digital.md` 12.3 and `spec-intro-transmitter.md` 9.4. Only a real V.92 CPd settles these.
6. **T-spaced reception.** The digital modem cannot move its sampling, and the ε estimator is unspecified. A wrong ε costs SNR the prefilter then has to recover.
7. **Clock and slips on the VoIP rig** (project memory notes on VoIP jitter slips and the VoIP round trip).
   - PCM upstream needs a sample-exact, clock-locked path from the analogue modem's D/A to the A/D, and from the A/D to our server. Every 20 ms slip breaks the 12-symbol frame and the precoder's state at the receiver.
   - V.34 upstream survives slips (`receiver.rs` resync); PCM upstream has no in-band resync short of FPE or retrain.
   - Expect PCM upstream to be fragile over MicroSIP, and hopeless through a transcoder.
   - **The V.34-upstream fallback (short phase 2, Table 19, and full phase 2 with Table 10) must stay solid**, including the known V.34 fallback bugs in the project memory note on the Crazytel PCM path.
8. **Server over a sound card** (9.2): the upstream codeword phase is unknown after the softphone's decode and resample. The existing `server.rs` doc already says downstream exactness depends on the softphone.
9. **Echo.** It is not modelled and not cancelled today (5.4, N4). A real server sees hybrid echo before its A/D quantises. On the softphone server path the echo is a whole VoIP round trip late and can slip.
10. **Frame count wording conflict:** 8.5.7 against 9.5.1.1.10 (7.4). Decide it, test it, and check it against a capture.
11. **Arithmetic width.** 12 moduli need u128 (M up to about 2^96). `v90::modulus` is 6 × u16.
12. **Timers differ from V.90.**
    - 20 s + 6 RTD from the end of INFO1a, against 15 s + 5 RTD from receiving it.
    - Ja within 4500 ms + RTD.
    - MD steps of 34.5 ms.

    Copying V.90 constants into the V.92 modem is an easy mistake.
13. **State growth.** Folding V.92 into `digital.rs` would make an already flag-heavy state machine unreviewable (8.1).
14. **Simulation that is too kind** (9.2: N1, N2, N4, N5). A V.92 upstream that passes today's `Network` proves little. Add the impairments before tuning.
15. **Vector coverage.** `tests/vectors/v92-56k.wav` is a line-side tap of a Conexant V.92 softmodem, both directions summed, resampled from 44.1 to 16 kHz. Its README says "V.34-style startup", so it may not contain PCM upstream at all. It is not a CO A/D view, and it cannot validate the digital receiver sample-exactly. At best it validates the shapes of the analogue modem's signals (Ru, TRN1u, Ja, CPt, SUVu) and our parsers.

---

## 12. House style, so new code reads like this code

**Module docs (`//!`)**
- Open with one sentence naming the thing and its clause, e.g. "The framed sequences of V.90's phases 3 and 4: Jd (8.4.2), …" (`sequences.rs:1`).
- Then prose paragraphs that explain the idea and the why, often with a ` ```text ` timeline of both modems' signals (`digital.rs:10-20`, `training.rs:9-12`).
- Real-world evidence is cited in prose, with dates and capture names (`phase2.rs:106-122`, `network.rs:11-16`).

**Constants**
- Every one has a `///` doc that paraphrases or quotes the Recommendation in double quotes, with the clause in parentheses. Example: `/// Rd in a rate renegotiation: "384T" (9.6.1.1.1).` (`digital.rs:60`).
- A bare clause number means the module's own Recommendation. Other Recommendations are written `10.1.2.3.2/V.34` or `Table 16/V.90`.
- Durations are kept in the spec's unit and converted at the point of use: `S_WITHIN: f64 = 5.1` seconds, `RD_SYMBOLS: usize = 384`, `TRN2D_FRAMES = 340` with a comment saying 2040T.

**Comment density**
- High. In state-machine code, about one comment line for every two or three code lines.
- Comments inside match arms cite the clause the arm implements, e.g. `// 9.3.1.5: "complete the current Jd sequence and then transmit J'd"`.
- Comments explain why an unusual choice was made, and name the live call or test that forced it.
- Trivial getters have no doc; everything else public has one.

**Spelling and voice**
- British: analogue, equaliser, normalised, behaviour.
- Plain declarative sentences; no "we".
- Failure strings are lowercase phrases that read as a reason: `"no B1 from the analogue modem"`, `"the analogue modem's CPt is not one this end can send"`.

**Naming**
- Full words, named for what is on the line or what happened: `Out::SdBar`, `Stage::AwaitSecondReversal`, `heard_ja`, `begin_phase4`, `far_md`, `round_trip`, `s_heard`, `far_e`.
- Spec symbols keep their letters where the spec uses them (`uinfo`, `drn`, `k`, `jd`).
- Status enums follow `Running` / `Connected{..}` / `ClearedDown` / `Failed(&'static str)`.
- `phase()` returns strings such as `"V.90 phase 3: Jd"`.

**Streaming shape** (`docs/design/architecture.md`)
- `step(sample) -> sample` for modems.
- `feed(sample)` plus a `heard() -> Option<Heard>` event queue for receivers.
- No block APIs.
- Time is a `now: u64` sample counter, with `samples(seconds)` helpers and deadlines as `Option<(u64, &'static str)>`.

**Bits**
- `Vec<bool>`, LSB first, through small private `put`/`get` helpers; `to_bits(&self) -> Vec<bool>` and `from_bits(&[bool]) -> Option<Self>`.
- Tables are derived and then checked by tests against values read from the rendered page (`ucode.rs:8-12` states the rule).

**Rust idiom (edition 2024)**
- `let … else`, `if let … && let …` chains, `is_some_and`, `is_none_or`, `.then_some`, `std::array::from_fn`, `std::mem::take`, `matches!`.
- u128 for mixed radix.
- `#[derive(Debug, Clone)]` everywhere (`missing_debug_implementations` is a workspace warning); `Copy, PartialEq, Eq` on small enums.
- `pub(crate)` for internals shared between modules (`RetrainWatch`, `SWatch`, `carrier::Watch`).
- No `unsafe` (the workspace denies it).
- Builder methods `with_*(mut self, …) -> Self`.
- Lines run to about 135 columns.

**Tests**
- Names are sentences: `a_real_jd_checks_and_enables_every_rate`, `a_slip_moves_everything_after_it_by_160_codewords`.
- A `///` doc on the test cites the clause or capture.
- Vectors come from real captures, with time stamps (`sequences.rs:589-591`).
- Assertions carry messages with the values (`assert!(ok, "slips every {period} s: …")`).
- Integration tests live in `crates/datapump/tests/`, drive `Network`, and print phase transitions with `println!` (`v90_call.rs:57-72`).

---

## 13. Open questions

1. Should V.92 be a new `v92` module that reuses `v90` (proposed), or an extension of `v90`?
2. Which echo levels and delays should the simulation model, both for a CO server and for the softphone server? Is an echo canceller wanted before the first V.92 simulation passes, or after?
3. What scale and signedness do CPd constellation "linear values" have? This can only be settled from a real V.92 server capture: decode one CPd.
4. Frame count origin: first or second TRN1u (8.5.7 against 9.5.1.1.10)?
5. Can the sound-card server (MicroSIP) path carry PCM upstream at all? A live `line-check`-style transparency test of codeword exactness in both directions would answer it before any V.92 work depends on it.
6. Scope of the first milestone: full phase 2 with PCM upstream only, or short phase 2, MOH and FPE as well?
7. Does `tests/vectors/v92-56k.wav` contain PCM upstream (Ru, TRN1u) or V.90-style V.34 upstream? A quick look for the 8 kHz two-level TRN1u after INFO1a would tell.
8. Should the digital modem equalise mostly at the transmitter (prefilter plus precoder, bare slicer at the receiver) or split the work with a receive FFE? The first seems to be the design intent, but that is not stated. It changes whether the echo residual or the A/D quantisation dominates.
