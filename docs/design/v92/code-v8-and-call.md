# V.92 call set-up and integration: what the code does now and where V.92 goes

Scope: how a call is set up, carried and ended today, and where V.92's quick connect (short
Phase 1), modem-on-hold, the V.250 `+P` commands and a V.92 choice in the window would go.
Crates read: `crates/v8`, `crates/datapump/src/v8.rs`, `crates/modem/src`, `crates/at`,
`crates/ec` (stack and LAPM), `crates/telemetry`, `crates/gui/src` (app, live, engine, answer,
console, remembered, main). For context only, also the entry points of
`crates/datapump/src/v90/{startup,server,analogue,digital}.rs` and
`crates/datapump/src/v34/{phase2,startup}.rs`. Nothing was built or run.

Spec facts in section 5 were read from **rendered** pages:

| Document | PDF pages viewed | What for |
|---|---|---|
| V.92 (11/2000) | 14, 15, 16 | 8.2 and 8.3.1: QC1a, QC2a, QCA1a, QCA2a, TONEq, ANSpcm (Tables 2-6) |
| V.92 | 21, 22, 23 | 8.3.2-8.3.6: QC1d, QC2d, QCA1d, QCA2d, QTS (Tables 11-14), start of Table 15 |
| V.92 | 46, 47, 48, 49, 50 | 9, 9.1, 9.2 (Figures 3-8, 9.2.1-9.2.5), 9.3 opening |
| V.250 (07/2003) | 55, 56, 57 | 6.4.1 `+MS`: Table 13 (the `V92` carrier), rx-rate subparameters, test syntax |
| V.250 | 91, 93, 94, 95, 96, 97 | 6.8.1-6.8.8: Tables 31-37 (`+PCW +PMH +PMHT +PMHR +PIG +PMHF +PQC +PSS`) |

Sibling digests this one leans on, rather than repeating:
- `spec-modem-on-hold.md`: MH sequences, RT, Table 34/V.92, timers, and the MOH-* requirement IDs.
- `spec-phase2-procedures.md`: INFO0 V.92 bits and the short Phase 2 decision.
- `spec-intro-transmitter.md`: rates, interchange circuits.

No digest covers short Phase 1 yet, so section 5.1 carries what call set-up needs from it.

Line numbers are as read on 2026-09-18 at `a7678a0` plus the untracked `docs/design/v92/`.

---

## 1. Module map

| Where | Role today | What V.92 touches |
|---|---|---|
| `crates/v8/src/lib.rs` | The V.8 messages, with no samples. Holds CM/JM/CI/CJ octets, `Menu` build and parse, the joint-menu rule, the PCM availability category (Table 5), and `Decoder` (octets → `Heard`). | QC/QCA octets (a new module), a zero-modulation "cleardown" CM/JM, and QC recognition beside `Decoder` |
| `crates/v8/src/ansam.rs` | `AnswerTone`, which tells ANSam (15 Hz AM) from V.25's plain ANS | ANSpcm is an *unmodulated* ~2100 Hz tone, so it reads as `is_plain()` |
| `crates/datapump/src/v8.rs` | V.8 on a line: V.21(L) and V.21(H) through `Bell103Tx/Rx`, clause 8 timings, the calling and answering state machine, and ANSam generation | Short Phase 1 states (QC1a, QCA1d, TONEq and the wait for ANSpcm), a "held" answering variant (ANSam for T1), and a way to abort mid-octet |
| `crates/modem/src/lib.rs` | The whole modem, from AT to samples. Covers V.250 states, the `Pump` enum over every data pump, V.8 then pump then V.42 stack, CONNECT ordering, retrain following, and hang-up | `+MS=V92`, quick-connect handover, hold call control, a graceful cleardown on ATH, and the ODP/ADP bypass (9.2.5/9.3.1) |
| `crates/modem/src/faxcall.rs` | Precedent: a whole "call kind" kept beside the pump rather than inside it | The pattern for a new `hold.rs` |
| `crates/datapump/src/v90/startup.rs` | `Analogue`/`Digital`: V.34 start-up with V.90 phase 2 inside it, retrain back through phase 2, and V.34 fallback | Where the V.92 flags (INFO0 bits), short Phase 2, PCM upstream and hold transactions hang |
| `crates/datapump/src/v90/server.rs` | `Line`: the 8 kHz digital modem behind a softphone, with unity-gain resampling | QTS, QTS\ and ANSpcm must come out of here, as exact codewords with frame alignment |
| `crates/at/src/lib.rs` | V.250 interpreter: settings structs (`Modulation`, `ErrorControl`, `Compression`, `V44`), `Action` queue, `+MS/+ES/+DS/+DS44/+ER/+DR/+FCLASS/+GCAP` | A `V92` carrier, the `+P` parameters (a struct), and `+PMHR`/`+PMHF` actions |
| `crates/at/src/parse.rs` | Command-line syntax (5.2-5.4) | Nothing. `+PMHT` etc. are legal names (at most 16 characters) |
| `crates/at/src/result.rs` | `ResultCode` including `Extended(String)` | `+PMHR: <v>` is information text (via `fmt.info`), not a result code |
| `crates/ec/src/stack.rs` | V.42 endpoint: detection, XID, LAPM, V.42bis/V.44 | Keep the link alive and frozen across hold; skip detection (9.2.5, 9.3.1) |
| `crates/ec/src/lapm.rs` | LAPM state machine and timers | Nothing new is required; see 6.5 |
| `crates/telemetry/src/lib.rs` | `Frame` (latest wins) and log; `CallState`, `Leds` | `CallState::OnHold`, and V.92 rows in `distant` |
| `crates/gui/src/app.rs` | Window: carrier box, "up to" ceilings row, V.8/V.42/compress toggles, Originate/Answer/Escape/Hang up/Force hang up, Advanced `+MS` window, Retrain button, status grid, remembered settings | `V92` in `CARRIERS`, its rates and ceiling, quick/hold toggles, Hold/Resume/Flash/"Call waiting" buttons, and Retrain for V.90/V.92 |
| `crates/gui/src/live.rs` | Line thread: `Session` flags (`hang_up`, `retrain`, …), steps the modem per sample, logs phase and retrain changes, publishes `Frame` | New session flags, hold logging, and `CallState` mapping |
| `crates/gui/src/answer.rs` | Headless answering board (`binmodem --answer --carrier …`) | Accept `--carrier V92` and a hold-grant option |
| `crates/gui/src/remembered.rs` | `settings.txt` (`name value` lines) under `%APPDATA%\BinModem` | New keys, plus a quick-connect memo (a separate file is proposed) |
| `crates/gui/src/engine.rs` | Capture replay; `Standard::V92` already exists (`engine.rs:68`), labelled "no receiver" | Nothing needed |

---

## 2. How a call runs today

### 2.1 V.250 states (`modem::State`, `crates/modem/src/lib.rs:51`)

`Command` → (`ATD`/`ATA`) → `Handshaking` → (CONNECT) → `Data` ⇄ (`+++`, `ATO`) `OnlineCommand` → (`ATH`, carrier loss, `hang_up()`) → `Command`.

- **Dial or answer.** `run_actions` (`lib.rs:1995`) turns `Action::Dial` or `Action::Answer` into `place_call(role)` (`lib.rs:2038`). That function does three things:
  - it resets per-call state (`lib.rs:2069-2081`);
  - if `+MS` automode is on and `offered()` (`lib.rs:2136`) is non-empty, it builds `negotiation = v8line::Modem::new(...)`;
  - otherwise it calls `start_pump(None)`.
- **What is offered in V.8** (`lib.rs:2097-2117`):
  - LAPM (`offering_lapm`) is offered when `want_error_control`;
  - PCM is offered when `+MS` carrier is `"V90"`:
    - the calling end offers `Pcm::ANALOGUE`;
    - the answering end offers `Pcm{digital}` on `Access{digital}`.
- **`Modem::step` dispatch order** (`lib.rs:1468`). It takes the first of these that applies:
  1. fax: `carry_fax`;
  2. negotiation: `negotiate`;
  3. pump: `pump.step`, then `advance_handshake` if `Handshaking`, or `watch_for_retrain` plus `carry_data` if `Data`/`OnlineCommand`.
- **`negotiate`** (`lib.rs:2221`) acts on the V.8 status:
  - `Agreed(m)`: copy `lapm()`, `far_menu()` and `pcm_role()`, drop the negotiation, then `start_pump(Some(m))`;
  - `NoNegotiation` (plain ANS): `start_pump(None)`;
  - `Failed`: `end_call(Ended::NoAnswer)`.
- **`start_pump`** (`lib.rs:2256`) maps the agreement to a carrier string, then builds a `Pump`:
  - `V34Duplex` with `pcm_role == Analogue` becomes `"V90"`, which builds `Pump::V90(v90::startup::Analogue::new)`;
  - `V34Duplex` with `pcm_role == Digital` becomes `"V90S"`, which builds `Pump::V90Server(v90::server::Line::new(fs, ours()))`;
  - it always sets `state = Handshaking` (`lib.rs:2349`).
- **`advance_handshake`** (`lib.rs:1541`), on the pump's `Progress::Connected`:
  - records the rates and discards training bits;
  - builds a **new** `ec::Stack` (`lib.rs:1618-1693`), with T401 from rate and round trip, `declared_lapm`, `without_detection` only when `+ES` asks (`lib.rs:1660`), and compression offers;
  - sets `announce`.
- **`announce_connect`** (`lib.rs:1722`) waits for `Stack::settled()`, then:
  - enforces `+ES` and `+DS`/`+DS44` requirements;
  - sets `state = Data`;
  - emits `+ER`, then `+DR`, then `CONNECT <rate>`.
- **Retrain** (`watch_for_retrain`, `lib.rs:1796`). `Progress::Retraining` sets `retraining`. When the pump reports `Connected` again, the new rates are taken silently: no new CONNECT and no new stack. If the pump reports `Failed` while `retraining` is set, the call ends with `NO CARRIER`.
- **V.42 clocks are frozen while the pump says `Retraining`** (`Modem::tick`, `lib.rs:1526-1531`).
- **Carrier loss.** `carry_data` ends the call when `!carrier` and the pump is not `Retraining` (`lib.rs:1915-1919`).
- **Hang-up paths.** All of them reach `drop_call` (`lib.rs:2367`), which sets `pump = None` **immediately**:
  - `ATH` → `Action::HangUp` → `drop_call`;
  - `Modem::hang_up()` (`lib.rs:915`) → `end_call(LocalRequest)`;
  - carrier loss.

  `v90::startup::Analogue::clear_down()` (`startup.rs:116`) and `Digital::clear_down()` (`startup.rs:449`) exist, and so does V.34's, but **nothing in `modem` calls them**. A V.34 or V.90 call is currently ended by going silent, and the far end detects it by loss of signal (`analogue.rs:1203-1211`). V.92 9.11 says a connection is ended with the cleardown procedure.

### 2.2 `datapump::v8::Modem` (`crates/datapump/src/v8.rs:110-128`, `advance` at `:374`)

```
Calling:   Quiet(1 s, CALL_QUIET) → Listening ─ANSam→ Waiting(Te = 1 s) → SendingCm ─2×identical JM→ SendingCj → Handover(75 ms) → Done(Agreed|Failed)
                                   └─plain ANS held > Te → Done(NoNegotiation)
Answering: Quiet(0.2 s) → Ansam ─2×identical CM→ SendingJm ─CJ (3 zeros)→ Handover(75 ms) → Done
                                └─ANSAM (5 s) elapsed → Done(NoNegotiation)
Any state: total > PATIENCE (60 s) → Done(Failed)
```

- **CM/JM transmit.** `send_sequence` (`:359`) queues 10 ones and then the octets. Each octet is framed on demand in `transmit` (`:530`).
- **Receive.** `Bell103Rx::feed` gives framed octets to `heard` (`:312`), which calls `v8::Decoder::feed`. `settled()` (`:335`) requires `IDENTICAL = 2` identical menus (`:90`).
- **The answering side builds JM** with `joint_pcm` when it offers PCM (`:456-461`).
- **`pcm_role()`** (`:246`) returns `Pcm::pair(ours, far, calling)`, but only if V.34 was chosen.
- **ANSam** (`:548`) is generated at `ANSAM_LEVEL = 0.35` (`:567`), with a reversal every 450 ms and 20 % AM at 15 Hz.
- **`Bell103Tx`** (`bell103.rs:135`) idles at mark when transmitting. **It has no way to drop queued bits**: SendingCm → CJ clears `outgoing` but lets the queued current octet finish (`v8.rs:425-436`).

### 2.3 `Pump` → `Progress` (`lib.rs:91-440`, `Progress` at `:732`)

Every pump answers the same questions:
- `step`, `status`, `round_trip_ms`, `carrier`, `take_bits`, `send_bits`, `accepts_bits`, `pending_bits`;
- scope data;
- `standard`, `phase`, `v34`.

For `V90` and `V90Server`, `v90::startup::Status` maps straight across: `Running` → `Negotiating`, `Retraining` → `Retraining`, and `ClearedDown` or `Failed` → `Failed` (`lib.rs:159-170`). Adding a `Pump` variant means touching about 20 matches in this file (see risk R12).

### 2.4 V.90 start-up wrappers (`crates/datapump/src/v90/startup.rs`)

- **`Analogue::new`** (`:55`) wraps `v34::startup::Modem::with_phase2(phase2::Modem::v90(Pcm::Analogue))`.
  - Once phase 2 is `Done` with a PCM INFO1a, `step` (`:249-258`) builds `analogue::Modem`.
  - If V.90 fails, the next pass goes back through phase 2 (`back_to_phase2`, `:124`).
  - After `V90_RETRAINS = 2` failures, or when the DIL says the line is hopeless, it calls `decline_pcm()` (`:236-239`) and the call becomes V.34.
- **Status mapping.** Once `connected_once` is set, `Running` reports as `Retraining` (`:138`, `:141`). That flag is what lets a retrain keep the call "up" for `modem`.
- **`Digital`** (`:266`) is the same shape, and scales phase 2 by `phase2_gain` from INFO0d (`:287`).
- **`server::Line`** (`server.rs:55`) resamples between line rate and 8 kHz at unity gain. `Modem::exact_levels()` (`lib.rs:1829`) is true only for `Pump::V90Server`; `live.rs:1115` and `answer.rs:144` then bypass the drive setting.
- **Far-end retrain detection in data mode:**
  - the analogue modem watches for Tone B (`analogue.rs:1215-1219`);
  - the digital modem watches for Tone A (`digital.rs:734`).

  That watch is exactly where an incoming RT arrives. MOH-9.10.1.1-R5 says RT can be confused with a retrain.

### 2.5 V.42 stack (`crates/ec/src/stack.rs`)

- **Phases** (`Phase`, `:56`): `Detecting` → `Negotiating` (XID) → `Protocol` (LAPM), or `Transparent`.
- **`without_detection()`** (`:376`) skips ODP/ADP and cuts N400 to `UNCONFIRMED_N400 = 3`. Its doc already cites "V.92 9.3.1", but `modem` only calls it for `+ES` `<orig_rqst>` = 2.
- **`declared_lapm()`** (`:355`) marks LAPM as settled in V.8.
- **`tick(dt)`** (`:665`) drives every timer. `modem` passes `0` while the pump is retraining.
- **LAPM** (`lapm.rs`) receives RNR (`:427`) but never sends one. T402 and T403 are not implemented (`lapm.rs:8-9`).

### 2.6 Window and line thread (`crates/gui/src`)

- **Buttons type AT commands** (`app.rs:694-701`, doc comment):
  - Originate → `ATD\r`, Answer → `ATA\r`;
  - Escape → `+++`, Hang up → `ATH\r` (shown only when not online);
  - the carrier box and ceilings row → `AT+MS=…`.
- **Buttons that bypass AT** through `Session` atomics:
  - "Force hang up" (`app.rs:1086-1095`) → `Session::hang_up` (`live.rs:430`) → the loop calls `modem.hang_up()` (`live.rs:1039-1042`);
  - "Retrain" (`app.rs:3012-3027`, enabled only if `frame.modulation == "V.34"`) → `Session::retrain` (`live.rs:440`) → `modem.retrain()` (`live.rs:1043-1046`).
- **What the line thread logs:** phase changes, retrain start and end (`live.rs:1209-1231`), EC phase, and state changes.
- **What it publishes** (`live.rs:1322-1393`): `f.state` mapped from `modem::State` (`:1331-1342`), `f.modulation = modem.standard()`, and `f.distant = modem.distant()`.
- **Settings.** `settings()` (`app.rs:1759`) is `AT&F`, then `+MS`, then the protection commands. They are asserted when the line opens (`app.rs:724-726`) and remembered by `to_remember`/`from_remembered` (`app.rs:2859`, `:2887`). Carriers are stored by name, so appending one is safe.

---

## 3. Key types and functions (file:line)

### 3.1 `v8` crate
- `PREAMBLE_ONES = 10` (`lib.rs:39`), `SYNC_CI = 0x00` (`:45`), `SYNC_MENU = 0xE0` (`:51`), `CJ = [0;3]` (`:663`).
- `Modulation` (`:108`) plus `ALL` (`:127`). There is no V.92 bit: V.92 9.1 NOTE says V.8 cannot say "V.92".
- `Menu { function, modulations, protocol, access, pcm }` (`:256`):
  - `octets()` (`:404`) always emits three modulation octets, then prot0, then access, then pcm;
  - `parse()` (`:485`);
  - `joint()` (`:557`) and `joint_pcm()` (`:588`).
- `Pcm { analogue, digital, v91 }` (`:282`). Its doc already says bits b5 and b6 are "V.90 or V.92". `Pcm::ANALOGUE` (`:301`), `Pcm::pair` (`:319`).
- `PcmRole { Analogue, Digital }` (`:293`), `Access` (`:348`), `mod tag` (`:359`).
- `octet([bool;8])` (`:380`, b0 is the LSB) and `bit()` (`:387`), both private.
- `is_extension(o)` (`:616`): `!b3 && b4 && !b5`.
- `Decoder` (`:673`) and `Heard { Ci, Cm, Jm, Cj }` (`:681`). `feed` (`:695`) syncs only on `0xE0` and `0x00`, and counts three zeros as CJ.
- `AnswerTone` (`ansam.rs:64`), with constants `ANSWER_TONE = 2100.0` (`:31`), `MODULATION_RATE = 15.0` (`:34`), `NOMINAL_DEPTH = 0.2` (`:37`) and `MODULATED = 0.08` (`:45`).
- Real-capture menus: `CONEXANT_CM` and `SERVER_JM` (`lib.rs:1223-1225`). The CM is from a V.92-capable Conexant modem, and it is byte-identical to what `Menu::octets` builds (test `our_call_menu_is_the_one_a_real_modem_sends`, `:1250`).

### 3.2 `datapump::v8`
- `timing` module (`v8.rs:56-87`): `CALL_QUIET 1.0`, `ANSWER_QUIET 0.2`, `TE 1.0`, `HANDOVER 0.075`, `ANSAM 5.0`, `PATIENCE 60.0`. `IDENTICAL = 2` (`:90`).
- `Status { Negotiating, Agreed(Modulation), NoNegotiation, Failed }` (`:94`) and the private `State` (`:110`).
- `Modem::new(role, function, ours, fs)` (`:177`) and its builders: `offering_lapm` (`:219`), `offering_pcm` (`:230`), `offering_pcm_on` (`:236`).
- Queries: `pcm_role` (`:246`), `phase` (`:266`), `far_menu` (`:291`), `lapm` (`:286`).
- `LOW = (1180, 980)` and `HIGH = (1850, 1650)` (`:24`, `:27`). **980 Hz is also TONEq** (V.92 8.2.5): a `Bell103Tx` in the low channel, transmitting with an empty queue, is TONEq.

### 3.3 `modem` crate
- `Role` (`lib.rs:44`), `State` (`:51`), `Ended` (`:65`), `Pump` (`:91`), `Progress` (`:732`), `ABORT_GUARD_MS = 125` (`:756`).
- `Modem` fields (`:760-858`), in particular:
  - `negotiation: Option<v8line::Modem>` (`:796`);
  - `far_menu`, `declared_lapm`, `pcm_role` (`:831`, `:847`, `:849`);
  - `announce` (`:857`);
  - `fax: Option<FaxCall>` (`:802`), the precedent for a side object.
- Call control: `hang_up` (`:915`), `retrain` (`:1866`), `ask_for_retrain` (`:1843`), `retraining` (`:1822`), `retrains` (`:1886`), `exact_levels` (`:1829`).
- Display: `standard()` (`:1077`), `line_phase()` (`:943`), `distant()` (`:1184`), `off_hook()` (`:1097`), `is_online()` (`:904`).
- Internal flow: `run_actions` (`:1995`), `place_call` (`:2038`), `offered` (`:2136`), `rate_range` (`:2171`), `negotiate` (`:2221`), `start_pump` (`:2256`), `end_call` (`:2353`), `drop_call` (`:2367`).
- Integration test harnesses to copy:
  - `crates/modem/tests/v90_call.rs`: `Server` (`:19`), `Call` (`:90`), `Pair` with a softphone resampler (`:184`), `when_one_end_hangs_up_the_other_notices` (`:332`);
  - `crates/modem/tests/call.rs`: V.34 retrain (`:487`), and "a retrain is not mistaken for the far end hanging up" (`:1591`).

### 3.4 `at` crate
- `Action` (`lib.rs:19`): `Dial`, `Answer`, `HangUp`, `OffHook`, `ReturnOnline`, `ResetProfile`, `FactoryDefaults`, `SelectModulation`, `SelectErrorControl`, `SelectCompression`, `SelectV44`, `SelectServiceClass`.
  - `defers_result()` (`:236`) is true for Dial, Answer and ReturnOnline. **The modem then emits the final code**; for `ATO` with nothing to return to it emits `ERROR` (`modem/lib.rs:2011-2014`).
  - `terminates_line()` (`:246`).
- Settings structs: `Modulation` (`:74`, default `V22B,1,0,0`), `ErrorControl` (`:93`), `Compression` (`:134`), `V44` (`:178`). `V44` is the closest model for a multi-field `+P` struct.
- `Interpreter` (`:331`) has public fields `regs`, `fmt`, `config`, `identity`, `modulations`, `modulation`, `error_control`, `compression`, `v44` and `service_class`. Its methods:
  - `restore_defaults` (`:425`) is what `&F` and `Z` reset;
  - `emit` (`:437`);
  - `execute_line` (`:518`) queues actions, and a failed command discards the line's actions;
  - `extended` (`:980`) dispatches by name, with the `+GCAP` string at `:992`.
- `modulations: ["V90","V34","V32B","V32","V22B","B103"]` (`:385`). This is the `+MS` whitelist.
- `modulation_select` (`:680`). `+MS=?` prints rate ranges `(300-4800)` (`:696`), which is stale. Set parsing reads only carrier, automode, min and max: **a 5th or 6th subparameter (`<min_rx_rate>`, `<max_rx_rate>`) is silently ignored** (`:703-735`).
- `v44_select` (`:893`) is the template for a many-subparameter command whose omitted values keep their current value.
- Tests (`crates/at/tests/session.rs`):
  - `a_modulation_that_is_not_offered_is_refused` (`:383`) **asserts `+MS=V92` is ERROR**;
  - `every_command_gcap_names_is_answered` (`:336`) and `the_capability_list_names_what_is_answered` (`:549`) send `AT<name>=?` for every `+GCAP` entry and require no ERROR.

### 3.5 `ec` crate
`Stack::new` (`stack.rs:311`), `declared_lapm` (`:355`), `without_detection` (`:376`), `declining` (`:388`), `over_a_round_trip` (`:412`), `settled` (`:585`), `connect`/`disconnect` (`:653`/`:660`), `tick` (`:665`), `next_bit` (`:717`), `feed_bit` (`:770`). `lapm::t401_for_line` (`lapm.rs:85`), `UNCONFIRMED_N400 = 3` (`:126`), `DEFAULT_N400 = 16` (`:115`).

### 3.6 `telemetry`
`Leds` (`lib.rs:24`), `CallState` (`:47`) with `label()` (`:60`), `Frame` (`:76`). In `Frame`: `modulation: &'static str` (`:119`), `line_phase` (`:126`), `distant: Vec<(&'static str, String)>` (`:131`, capacity 10 at `:195`), `bit_rate`/`tx_bit_rate` (`:133`/`:136`). Also `Publisher::log` (`:355`).

### 3.7 `gui`
- `app.rs`: `Modulation` (`:76`) with `rates(carrier)` (`:196`, indexed by `CARRIERS` position; `_` covers V.34 and V.90), `fit` (`:216`) and `command` (`:246`). Also:
  - `CARRIERS` (`:570`, six entries);
  - `line_controls` (`:702`): the carrier combo is at `:943-967`, the V.8 toggle at `:972-991`, and the buttons at `:1039-1095`;
  - `advanced_modulation` (`:1607`), `settings` (`:1759`), `CEILINGS` (`:2431`, ten entries, V.90 as `(0, 5, "56000")`), `call_settings` (`:2455`), `status` (`:2561`);
  - the Retrain button (`:3008-3027`);
  - tests `the_rate_lists_belong_to_the_carriers_they_are_indexed_by` (`:3098`, which asserts `CARRIERS.len() == 6`) and `v32bis_can_do_everything_v32_can` (`:3120`, which checks `CEILINGS`);
  - `perform` (`:429`) is a **second exhaustive `match` on `at::Action`**, for capture mode.
- `live.rs`: `Session` (`:224`) with atomics `hang_up` (`:235`) and `retrain` (`:238`), `run` (`:645`), the flag handling (`:1039-1046`), and `f.state` mapping (`:1331`).
- `console.rs`: in live mode it only mirrors the modem's online state (`follow`, `:167`) and prints what the modem says (`feed_screen`, `:162`). Unsolicited `+PMHR:` text needs no console change.
- `answer.rs`: `--carrier` (`:57`) is typed as `AT+MS={carrier}` (`:101-104`), and `ATA` is typed at once.

---

## 4. Constants V.92 will touch or must respect

| Constant | Where | Value | Clause | V.92 note |
|---|---|---|---|---|
| `timing::TE` | `datapump/src/v8.rs:72` | 1.0 s | V.8 8.1.1 | Same as V.92 9.2.1.1 "ANSam detected for 1 s" before QC1a. Keep it, and require ANSam to be **still** present at the end. |
| `timing::HANDOVER` | `v8.rs:76` | 0.075 s | V.8 8.1.2, 8.2.3 | Same value as V.92's 75 ± 5 ms silences in 9.2 |
| `timing::ANSAM` | `v8.rs:80` | 5.0 s | V.8 8.2.2 | Must not apply to a held modem, which sends ANSam for T1 (up to 16 min, or without limit) |
| `timing::PATIENCE` | `v8.rs:86` | 60 s | ours | Must not apply on hold |
| `timing::ANSWER_QUIET` | `v8.rs:63` | 0.2 s | V.8 8.2 | V.92 9.2.3 and 9.2.4 repeat "at least 200 ms" |
| `IDENTICAL` | `v8.rs:90` | 2 | V.8 7.3, 7.4 | QC and QCA are sent **once**, so this rule cannot be applied to them (see 6.2) |
| `ANSAM_LEVEL` | `v8.rs:567` | 0.35 | ours | ANSpcm levels are absolute (dBm0, Table 6) and exact codewords |
| `SYNC_MENU` / `SYNC_CI` | `v8/src/lib.rs:51` / `:45` | 0xE0 / 0x00 | V.8 Table 1 | QC octets can take these values (R1) |
| `V90_RETRAINS` | `v90/startup.rs:23` | 2 | ours | Also governs how often V.92 PCM is retried |
| `ANSWER_TONE` | `v8/src/ansam.rs:31` | 2100 Hz | V.25 | ANSpcm ≈ 2099.67 Hz (79/301 × 8000) |
| `L2_READ`, `RETRAIN_SILENCE` | `v34/phase2.rs:122`, `:126` | 0.300 s, 0.070 s | V.34 11.2, 11.5 | RT is Tone A or Tone B; retrain silence is 70 ± 5 ms |
| `ABORT_GUARD_MS` | `modem/src/lib.rs:756` | 125 | V.250 5.6.1 | Unchanged |
| `XID_WAIT_MS` | `ec/src/stack.rs:48` | 1000 | ours | Irrelevant when detection is bypassed and the far end sends no XID |
| `UNCONFIRMED_N400` | `ec/src/lapm.rs:126` | 3 | V.42 App. III.2 | Applied by `without_detection`. With 9.2.5 the far end *did* declare LAPM, so consider `DEFAULT_N400` there (see Q7) |
| `+MS` whitelist | `at/src/lib.rs:385` | 6 names | V.250 Table 13 | Add `"V92"` |
| `+GCAP` string | `at/src/lib.rs:992` | `+FCLASS,+MS,+ES,+ER,+DS,+DS44,+DR` | V.250 6.1.9 | Add the `+P` names (R15) |
| `CARRIERS`, `CEILINGS` | `gui/src/app.rs:570`, `:2431` | 6 and 10 entries | ours | Index-coupled to `Modulation::rates` and to tests |
| `DEFAULT_DRIVE` | `gui/src/live.rs:340` | 0.1 | ours | Bypassed through `exact_levels()`, which must also be true during QTS and ANSpcm |

---

## 5. V.92 and V.250 facts needed at this layer (from rendered pages)

### 5.1 Short Phase 1 (V.92 8.2, 8.3, 9.2)

**Signals and channels.**
- Short Phase 1 bits go at 300 bit/s on V.21(L) or V.21(H) (8.2).
- `…1` variants (QC1a, QCA1a, QC1d, QCA1d) are for V.8-initiated calls.
- `…2` variants (QC2a, QCA2a, QC2d, QCA2d) are for V.8 bis calls. The repo has no V.8 bis, so **only the `…1` variants are in scope**.

| Signal | Sent by | Channel | Sent | Table |
|---|---|---|---|---|
| QC1a | analogue modem calling | V.21(L) | once, "followed immediately by CM" | 2 |
| QCA1a | analogue modem answering | V.21(H) | once; ends with 10 extra ONEs | 4 |
| QC1d | digital modem calling | V.21(L) | once, followed immediately by CM | 11 |
| QCA1d | digital modem answering | V.21(H) | once; ends with 10 extra ONEs | 13 |
| TONEq | analogue modem | a 980 Hz tone (8.2.5) | — | — |
| ANSpcm | digital modem | PCM codewords | repetitive | 6-10 |
| QTS, QTS\ | digital modem | PCM codewords | 128 and 8 repetitions of a 6-symbol pattern | 8.3.6 |

**Bit layout, common to QC1a, QCA1a, QC1d and QCA1d** (bit 0 first in time):

| Bits | Content |
|---|---|
| 0:9 | ten ONEs |
| 10:19 | `0101010101` (sync) |
| 20 | start bit `0` |
| 21 | 0 = analogue modem, 1 = digital modem |
| 22 | 0 = QC, 1 = QCA |
| 23 | `P`: 1 calls for LAPM (see 9.2.5) |
| 24:29 | analogue variants: `W 0 X Y Z 1`; digital variants: `0 0 0 L M 1` |
| 30:39 | ten ONEs |
| 40:49 | bits 10:19 repeated |
| 50:59 | bits 20:29 repeated |
| 60:69 | QCA1a and QCA1d only: ten ONEs |

**[derived] As V.8-type async characters** (start, b0..b7 with b0 the LSB, stop, which is the convention of `v8::octet`):
- The sync is the octet **`0x55`**.
- The information octet is `q = b0 | b1<<1 | P<<2 | b3<<3 | b4<<4 | b5<<5 | b6<<6 | b7<<7`, where:
  - analogue variants: `b0=0`, `b1` = QC/QCA, `b3=W`, `b4=0`, `b5=X`, `b6=Y`, `b7=Z`;
  - digital variants: `b0=1`, `b1` = QC/QCA, `b3=0`, `b4=0`, `b5=0`, `b6=L`, `b7=M`.
- Bit 29 is the stop bit.
- The table rows repeat exactly this: QC1a 50:59 = `000PW0XYZ1`, QCA1a = `001PW0XYZ1`, QC1d = `010P000LM1`, QCA1d = `011P000LM1`.
- So QC1a is ONEs, `[0x55, q]`, ONEs, `[0x55, q]`, and then CM with its own 10 ONEs and `0xE0`.
- Worked values:

| Signal | Fields | `q` |
|---|---|---|
| QC1a | P=1, WXYZ=`0001` | `0x84` |
| QC1a | P=1, WXYZ=`1111` | `0xEC` |
| QC1a | P=0, WXYZ=`0111` | **`0xE0`** |
| QC1a | P=0, WXYZ=`0000` | **`0x00`** |
| QCA1a | P=1, WXYZ=`0000` | `0x06` |
| QC1d | P=1, LM=`01` | `0x85` |
| QCA1d | P=1, LM=`00` | `0x07` |

**WXYZ is U_QTS** (Table 2), a bit pattern sent W first. It is the Ucode of the codeword the digital modem uses for QTS:

| WXYZ | 0000 | 0001 | 0010 | 0011 | 0100 | 0101 | 0110 | 0111 | 1000 | 1001 | 1010 | 1011 | 1100 | 1101 | 1110 | 1111 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| U_QTS | 61 | 62 | 63 | 66 | 67 | 70 | 71 | 74 | 75 | 78 | 79 | 82 | 83 | 86 | 87 | **cleardown from on-hold state** |

**LM is the ANSpcm level** (Tables 11 and 13), L sent first: `00` = −9.5 dBm0, `01` = −12, `10` = −15, `11` = −18.

**ANSpcm** (8.3.1, Table 6):
- A 301-symbol codeword loop, x_k = ⌊scl·√2·cos(2πk·79/301 + ϑ) + 0.5⌋ for k = 0..300, then G.711-quantised.
- ϑ = 0.25π/301.
- scl depends on law and level:

| Level | µ-law scl | A-law scl |
|---|---|---|
| −9.5 dBm0 | 1334 | 667 |
| −12 dBm0 | 1000 | 500 |
| −15 dBm0 | 708 | 354 |
| −18 dBm0 | 500 | 250 |

- The result must equal Tables 7-10.
- A phase reversal is added every **3612** symbols (451.5 ms).
- A NOTE warns that some network equipment changes the channel in response to ANSpcm.
- [derived] Frequency 2099.67 Hz, with **no amplitude modulation**.

**QTS** (8.3.6):
- 128 × {+V, +0, +V, −V, −0, −V}, where V is the codeword with Ucode U_QTS and 0 is the codeword with Ucode 0.
- QTS\ is 8 × {−V, −0, −V, +V, +0, +V}.
- The first QTS symbol is in **data frame interval 0**, and the digital modem keeps that frame alignment from then on.
- The figures give 768T and 48T, which is 96 ms and 6 ms at T = 1/8000 s.

**Procedure for the two roles this project has.**

Analogue caller (9.2.1.1 and 9.2.1.3):
1. Listen for ANSam.
2. After **1 s** of ANSam, send QC1a, then CM (repeated as in V.8), and listen for QCA1d, QCA1a and JM.
3. On **QCA1d**: stop CM **without completing the current octet**, send silence, and wait for QTS and QTS\, then ANSpcm.
4. After ANSpcm has been detected for 1 s, send TONEq for at least 50 ms. **Because ANSam was already detected for 1 s, TONEq may start as soon as ANSpcm is detected.**
5. When ANSpcm stops, stop TONEq, stay silent for 75 ± 5 ms, and go to Phase 2.

The other outcomes after step 2:
- **QCA1a** (the far end is an analogue modem): stop CM mid-octet, stay silent until ANSam, send TONEq, and when ANSam stops, 75 ± 5 ms of silence, then **V.34** Phase 2 (9.2.1.4).
- **JM**: continue by V.8.

Digital answerer (9.2.4, 9.2.4.1 and 9.2.4.3):
1. Stay silent for at least 200 ms, then send ANSam.
2. Listen for QC1a, QC1d and CM.
3. On **QC1a**: send QCA1d, 75 ± 5 ms of silence, QTS, QTS\, then ANSpcm, and listen for TONEq.
4. On TONEq: 75 ± 5 ms of silence, then Phase 2.
5. **If no TONEq arrives within 2 s after QCA1d: send ANSam and continue by V.8.**

The other outcomes after step 2:
- **QC1d**: it *may* take the analogue role; not needed here, so treat it as ignorable and wait for CM.
- **CM**: normal V.8.

The analogue answerer (9.2.3.1, 9.2.3.3, 9.2.3.4) and digital caller (9.2.2.1) mirror these. They are not needed for BinModem's roles today.

**9.2.5.** If both modems have indicated LAPM, V.42 ODP/ADP **shall** be bypassed. **9.3** carries the V.92 INFO0 capability bits (see `spec-phase2-procedures.md` §2).

**Figure 3 timings (V.92 p.46):**
- 1 s from ANSam start to QC1a;
- 75 ± 5 ms of silence after QCA1d;
- QTS 768T, then QTS\ 48T, then ANSpcm;
- the TONEq arrow is labelled "≤1 s".

### 5.2 V.250 (07/2003) 6.8: PCM DCE commands

Every one of these is "mandatory if ITU-T Rec. V.92 is implemented in the DCE".

| Command | Kind | Values (rendered tables) | Default |
|---|---|---|---|
| `+PCW=[<call waiting>]` (6.8.1, Table 31) | parameter | 0 = toggle V.24 circuit 125 and collect Caller ID if `+VCID` enables it; 1 = hang up; 2 = ignore V.92 call waiting | 0 |
| `+PMH=[<value>]` (6.8.2, Table 32) | parameter | 0 = MOH **enabled**; 1 = disabled | 0 |
| `+PMHT` (6.8.3, Table 33) | parameter | 0 = deny MOH requests; 1…12 = grant with 10 s, 20 s, 30 s, 40 s, 1, 2, 3, 4, 6, 8, 12, 16 min; 13 = grant, indefinite | **not stated**; the example reads `+PMHT: 0` |
| `+PMHR` (6.8.4, Table 34) | action | reply `+PMHR: <v>`: 0 = denied or not available (may retry later); 1…12 = granted with the `+PMHT` timeouts; 13 = granted, indefinite; 14 = denied, and later requests this session will be denied too | — |
| `+PIG=[<value>]` (6.8.5, Table 35) | parameter | 0 = PCM upstream enabled; 1 = disabled | 0 |
| `+PMHF` (6.8.6) | action | go on-hook for about 0.5 s (national rules may differ) and then back (the text says "on-hook", surely a slip) | — |
| `+PQC=<value>` (6.8.7, Table 36) | parameter | 0 = short Phase 1 and short Phase 2 enabled; 1 = short Phase 1 only; 2 = short Phase 2 only; 3 = both disabled | 0 |
| `+PSS=<value>` (6.8.8, Table 37) | parameter, calling DCE | 0 = the DCEs decide (short only where `+PQC` allows); 1 = force short on this and later connections where `+PQC` allows; 2 = force full regardless of `+PQC` | 0 |

Behaviour notes:
- `+PMHR` returns **ERROR** if MOH is not enabled or the DCE is idle, and its response "may be delayed".
- `+PMHF` returns **ERROR** if the modem is not on hold.
- Each parameter has read syntax `+X?` → `+X: <v>` and test syntax `+X=?` → `+X: (list)`. The listed examples are `(0,1,2)`, `(0,1)`, `(0,1,…,13)`, `(0,1)`, `(0,1,2,3)` and `(0,1,2)`.
- The `+PMHT` values **are** the V.92 Table 33 T1 codes read as integers:
  - rendered V.92 p.46 gives `1011` = 12 min, `1100` = 16 min, `1101` = no limit;
  - so a received MHack T1 code *c* maps to `+PMHR: c`.
- Not V.92 facts, but related (see `spec-modem-on-hold.md` §7): V.250 6.3.6 NOTE says `H` may end a V.92 hold without going on-hook, and 6.3.7 says `O` also reconnects after a hold transaction.

`+MS` (6.4.1, rendered pp.55-57):
- Table 13 lists **`V92`** ("ITU-T Rec. V.92") next to `V90` and `V91`. Proprietary carrier strings must not begin with "V", and the repo's internal `"V90S"` never reaches `+MS`.
- The full form is `+MS=<carrier>,<automode>,<min_rate>,<max_rate>,<min_rx_rate>,<max_rx_rate>`. The `_rx_` pair limits the receive direction separately, which is natural for V.90/V.92 downstream. Their recommended default is 0, "if implemented".

---

## 6. Extension points and concrete proposals

### 6.1 A `V92` carrier end to end

1. **`at`** (`lib.rs:385`): make the whitelist `["V92","V90","V34","V32B","V32","V22B","B103"]`.
   - Fix the `+MS=?` rate text (`:696`) while there. Since V.92 has asymmetric rates, parse `<min_rx_rate>`/`<max_rx_rate>` into `Modulation { min_rx_rate, max_rx_rate }`, rejecting a 7th subparameter as `+DS44` does. That gives the analogue modem a downstream ceiling to feed `v90::analogue::Modem::renegotiate(most)` and `dil::choose`.
   - Update `session.rs:383` so `V92` moves to the offered list; `V21` stays refused.
2. **`modem::offered`** (`lib.rs:2147`): `"V34" | "V90" | "V92"` → `V34Duplex`.
3. **`modem::place_call`** (`lib.rs:2109`): `matches!(carrier, "V90" | "V92")`, with the same PCM offers.
   - V.8 cannot say V.92 (9.1 NOTE), so the menus are identical to V.90's.
   - Record `self.v92_wanted = carrier == "V92"` and `self.pcm_upstream = at.v92.pcm_upstream` (`+PIG`).
4. **`modem::start_pump`** (`lib.rs:2261-2262`): pass the V.92 options into the existing pumps rather than adding `Pump` variants (R12). Proposal:
   ```rust
   /// What this end says in INFO0 beyond V.90 (Tables 15 and 16/V.92), and what
   /// it will do with it.
   #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
   pub struct V92 {
       /// INFO0d bit 27 / INFO0a bit 26.
       pub capable: bool,
       /// INFO0d bit 26 / INFO0a bit 27: ask for short phase 2 (9.4).
       pub short_phase2: bool,
       /// Whether the analogue modem may pick PCM upstream in INFO1a (Table 18),
       /// which +PIG turns off.
       pub pcm_upstream: bool,
       /// Answer MHreq with MHack and this T1 code (Table 33), or MHnack if None.
       pub grant_hold: Option<u8>,
   }
   ```
   - Construct with `v90::startup::Analogue::with_v92(fs, v92)` and `v90::server::Line::with_v92(fs, ours(), v92)`, threaded down to `phase2::Modem::v90(Pcm::…)`. The phase 2 digest owns that part.
   - The fallback line (`lib.rs:2278`) becomes `"V34" | "V90" | "V92" => "V32B"`.
5. **`Pump::standard()`** (`lib.rs:411-414`): add `m.is_v92()` arms returning `"V.92"`. **`Modem::standard()`** idle arm (`lib.rs:1086-1092`): add `"V92" => "V.92"` and also the missing `"V90" => "V.90"` (today an idle V.90 selection reads "V.22bis").
6. **`Pump::shape()`/`states()`/`constellation_point()`** need nothing new for PCM downstream. PCM upstream on the **server** side needs a different upstream scope; `pairs: frame.symbol_label == "PCM"` (`app.rs:2843`, `:2991`) is the hook, and the PCM display choice is recorded in memory `v90-pcm-display-choice`.

### 6.2 Quick connect (short Phase 1)

**Layering.** The work splits across four layers:
- messages go in `v8`;
- FSK signalling and timing go in `datapump::v8`;
- PCM signals (QTS and ANSpcm) go in the digital pump, because they must be exact codewords at 8 kHz with frame alignment;
- the decision and the memo go in `modem`.

**6.2.1 New module `crates/v8/src/quick.rs`** (`pub mod quick;` next to `pub mod ansam;` at `lib.rs:30`):
```rust
//! V.92's short phase 1: QC and QCA, in V.8's own framing (8.2 and 8.3/V.92).

/// The synchronisation both copies of a QC or QCA open with: `0101010101`
/// (Tables 2, 4, 11 and 13/V.92), which framed the way V.8 frames an octet
/// is 0x55.
pub const SYNC_QC: u8 = 0x55;

/// Which of the four sequences (V.8-initiated only; the V.8 bis ones are not here).
pub enum Kind { Qc1a, Qca1a, Qc1d, Qca1d }

/// U_QTS (Table 2/V.92): a Ucode for QTS, or the cleardown code 1111.
pub enum Uqts { Ucode(u8), Cleardown }   // from_pattern(u8) / pattern() / ucode()

/// The level ANSpcm goes at (Table 11/V.92).
pub enum AnspcmLevel { Minus9_5, Minus12, Minus15, Minus18 } // dbm0(), scl(Law)

pub struct Qc { pub kind: Kind, pub lapm: bool, pub uqts: Option<Uqts>, pub level: Option<AnspcmLevel> }
impl Qc {
    pub fn octet(&self) -> u8;                 // b0..b7 as in section 5.1
    pub fn from_octet(o: u8) -> Option<Self>;  // rejects b4 = 1; digital variants also need b3 = b5 = 0
    pub fn bits(&self) -> Vec<bool>;           // all 60 or 70 bits, ONEs included, for a line to send as-is
}
/// Watches framed octets for `[SYNC_QC, q, SYNC_QC, q]` with both copies equal.
pub struct Watcher { /* last two octets */ }
```
- Add `Heard::Qc(Qc)` to `v8::Heard` (`lib.rs:681`) only if the `Decoder` owns the watcher. Keeping `Watcher` separate is safer, because the decoder's sync logic mis-reads some QC octets (R1).
- **Detection rule.** QC and QCA are sent once, so accept one sequence whose two copies agree. That redundancy is built into the signal, and `IDENTICAL` does not apply.
- Tests to write:
  - one test per table row;
  - that `0x55` is also `is_extension`;
  - that a CM stream containing `0x55` as an extension octet is **not** read as QC;
  - that QC1a with `q == 0xE0` or `q == 0x00` is still recognised.

**6.2.2 `crates/datapump/src/v8.rs`.**
- **Builders:**
  - `Modem::offering_quick(self, qc: v8::quick::Qc) -> Self`, calling side;
  - `Modem::answering_quick(self, level: AnspcmLevel) -> Self`, digital answering side.
- **New `State` variants:**
  - calling: `QuickWait`, which follows QCA1d and is silent while the pump listens for QTS and ANSpcm (see below); also `QuickAnalogue` (after QCA1a, silent until ANSam, then TONEq);
  - answering: `SentQca` (QCA1d queued).
- **`Waiting` (`:413`)** must, when `self.quick.is_some()` and `self.answer.is_ansam()` still hold, queue `qc.bits()` directly, then `send_sequence(cm)`. This needs a raw-bits queue next to `outgoing`, since QC1a has ONEs between two framed octets.
- **`SendingCm` (`:422`)**, on a detected QCA1d, must **abandon the current octet** (V.92 9.2.1.1). Add `Bell103Tx::abandon()` (`bell103.rs:135`), which clears `pending`, sets `current = true` and zeroes `countdown`. Then:
  ```rust
  Status::Quick { role: PcmRole::Analogue, lapm: bool, level: AnspcmLevel }
  ```
  `lapm` is `qc.lapm && qca.lapm`.
- **`Ansam` (`:451`)**, on QC1a with our PCM digital offer: send `Qca1d.bits()` and go to `Done(Status::Quick { role: PcmRole::Digital, lapm, uqts })`. **Do not wait for CJ**, because no JM was sent.
- **A new `Status::Quick`.** `modem::negotiate` (`lib.rs:2224`) gains an arm that sets `declared_lapm`, sets `pcm_role`, and calls `start_pump_quick(...)`.
- **ANSpcm is plain 2100 Hz.** While `QuickWait` (or the pump that replaces it) waits, `AnswerTone::is_plain()` means **ANSpcm**, not V.25 ANS (R8). ANSam coming back means the digital modem gave up after 2 s (9.2.4.3), so the calling side must go back to `Waiting` and send CM (V.8). This reverse path does not exist today (R5).
- **TONEq** is `tx.set_transmitting(true)` with nothing queued: `Bell103Tx` in the LOW channel idles at 980 Hz mark (`bell103.rs:193-206`).

**6.2.3 Where QTS, QTS\ and ANSpcm live.**
- **Digital side.** `v90::server::Line`/`startup::Digital` gets `Digital::after_quick_connect(info0d, uqts, level, v92)`. Its first stage runs at 8 kHz:
  - 75 ms of silence, QTS for 768 symbols, QTS\ for 48 symbols, then ANSpcm (301-symbol loop, reversal every 3612);
  - a 980 Hz detector (a `dsp::ToneDetector`) listens for TONEq;
  - on TONEq: 75 ms of silence, then phase 2;
  - with no TONEq within 2 s after QCA1d: report `Status::BackToV8` (new).

  The frame counter starts at the first QTS symbol and must carry on into phase 2 (8.3.6; P12 in the phase 2 digest). Because this pump exists from QTS onward, `exact_levels()` is already true, so the drive is bypassed (R4).
- **Analogue side.** `startup::Analogue::after_quick_connect(fs, uqts, v92)`. Its first stage:
  - hunts QTS/QTS\ (known levels; timing and gain reference for the pump's receiver);
  - detects ANSpcm and starts TONEq as soon as it is detected (allowed, since ANSam was detected for 1 s);
  - on ANSpcm off: stop TONEq, 75 ms of silence, phase 2.

  TONEq can be generated with a local 980 Hz NCO; the pump must not depend on `datapump::v8`.
- **`modem::start_pump_quick`**, next to `start_pump` (`lib.rs:2256`), builds these. It also needs a `Progress`/`Pump` way to say "go back to V.8" (R5):
  ```rust
  /// The pump has handed the line back: short phase 1 was not answered, and
  /// V.8 goes on from ANSam (9.2.4.3/V.92).
  Progress::BackToV8
  ```
  `advance_handshake` handles it by rebuilding `negotiation` in the right state: the answering side goes straight to `Ansam`, and the calling side goes to `Waiting` or `SendingCm`.

**6.2.4 Deciding whether to send QC1a (`modem`).** Send QC1a only when all of these hold:
- `+MS=V92`;
- `at.v92.quick` allows short Phase 1 (`+PQC` ∈ {0, 1});
- `+PSS` ≠ 2;
- a **memo** exists for this connection.

With `+PSS=1`, a memo is still needed for U_QTS. Proposal:
```rust
/// What a previous call to the same place taught this end (V.92 1 j, "reduced
/// start-up time on recognized connections"), which quick connect spends.
pub struct Recognized { pub uqts: v8::quick::Uqts, pub round_trip_ms: u32, pub law: Law, /* + phase 2/3 facts the other digests ask for */ }
```
- `Modem` gets `pub quick_memo: Option<Recognized>`, set by the host like the `fax_*` fields (`lib.rs:811-829`), and `pub fn learned(&self) -> Option<Recognized>` after a V.92 call.
- **Keying.** In the GUI, Originate types a bare `ATD`, so there is no number to key on (R16). Key by the dial string when present, otherwise a single "last call" slot. Store it in `remembered.rs` under `qc_*` keys, or in a separate `quick.txt`, so a bad memo can be deleted on its own.
- **Choosing U_QTS** is open (Q1). It must be one of the 15 Ucodes in Table 2.
- **9.2.5.** When `Status::Quick { lapm: true }`, and after 9.3 when both INFO0s said V.92 with prot0 LAPM, call `Stack::without_detection()` in `advance_handshake` (`lib.rs:1657-1662`) as well as when `+ES` asks. The pump must expose `both_v92()`; the phase 2 digest owns that.

### 6.3 Modem-on-hold

There is no call waiting to detect: the softphone takes it and the modem never hears it. So every trigger has to come from something this modem can see.

**6.3.1 Triggers available to BinModem**

| Trigger | Where | What it does |
|---|---|---|
| `AT+PMHR` in online command state | `at` → `Action::RequestHold` → `modem` | Initiate: RT, then MHreq. Reply `+PMHR: n` once MHack or MHnack arrives. This is the V.250 route, and the one to test live against a V.92 ISP server (NetZero pool, see memory `live-v34-isp-far-end`). |
| GUI "Hold" button | `app.rs` line_controls, next to Hang up (`:1074`) | Types `AT+PMHR\r`. Like Hang up, it is available only after Escape, which follows the file's "buttons type commands" rule (`app.rs:694-701`). |
| GUI "Call waiting" button (simulated) | `Session::call_waiting` atomic → `modem.call_waiting()` | Stands in for a network indication, so the `+PCW` paths can be exercised: 0 → emit `RING` (the V.250 text for circuit 125) and a log note; 1 → `hang_up`; 2 → nothing. It does **not** start MHreq; the DTE decides, as in V.250. |
| Far end sends RT + MHreq | V.90/V.92 pump, from the retrain watch (`analogue.rs:1215`, `digital.rs:734`) | Respond by `+PMH` and `+PMHT`: MHack(T1), then hold as the answering side; or MHnack. This is mandatory for every V.92 modem (MOH-9.10-R2). Testable in `modem/tests` with two `Modem`s. |
| GUI "Resume" | types `ATO\r` | Requester side: Phase 1 as the calling modem (short Phase 1 if allowed), then Phase 2 onward. The link and the V.42 stack are kept. |
| GUI "Flash" | types `AT+PMHF\r` | No DAA here (R18): log it, answer OK while on hold, ERROR otherwise. |
| Hang up while on hold | `ATH` | Requester: send QC1a with U_QTS `1111` (the analogue modem), or a zero CM (9.10.2.1 R10, R11), then drop the call. |
| `answer.rs --grant-hold <0..13>` | headless server | Types `AT+PMHT=<n>` so a live dial-in can be held. |

**6.3.2 Line level (datapump)**
- New `crates/datapump/src/v92/hold.rs`, or `v90/hold.rs` if V.92 stays inside `v90/`, holds a `Transaction`:
  - RT (Tone A or Tone B from the existing phase 2 generators);
  - MH send and receive through `v34::dpsk` with a new 40-bit length;
  - Table 34 responses, the 200 ms stop rule, and the 2 s + RTD initiator timeout.

  All of these are in `spec-modem-on-hold.md` §1-2 and §9.2.
- It is owned by `analogue::Modem`/`digital::Modem`, because it must pre-empt data mode and share the retrain watch (MOH-9.10.1.1-R5). They expose:
  ```rust
  pub fn request_hold(&mut self) -> bool;         // MHreq, false outside data mode
  pub fn hold_status(&self) -> Option<HoldEvent>; // Granted(t1) | Refused | Requested | Cleared(reason) | FastReconnect
  ```
- `startup::Analogue`/`Digital` pass these through the way `clear_down` and `renegotiate` pass through (`startup.rs:108-121`).

**6.3.3 Call control (`crates/modem/src/hold.rs`, beside `faxcall.rs`)**
```rust
/// Where a modem-on-hold transaction has got to (9.10/V.92).
pub enum Hold {
    /// MHreq is going; the DTE is waiting for +PMHR.
    Asking { since_ms: u32 },
    /// Granted by the far end: the line is this end's to use elsewhere.
    Away { t1: u8 },
    /// This end granted it: ANSam for T1, listening for QC or CM.
    Holding { t1: u8, since_ms: u32 },
}
```
- **`Modem` gets `hold: Option<Hold>`.** `State` keeps V.250's four values: hold happens in `OnlineCommand` (after `+++`) or `Data`. `is_online()` stays true, and `off_hook()` becomes `|| self.hold.is_some()`.
- **Held side.** The pump ends, `pump = None`, and `negotiation = Some(v8line::Modem::held(t1, ours, fs))`. That is a new constructor: it starts in `Ansam` after at most 80 ms, has no 5 s limit and no 60 s patience, and ends when:
  - QC1a/CM arrives: `Agreed` or `Quick`;
  - a zero-modulation CM arrives: zero JM, then `Failed` after CJ;
  - QC1a with U_QTS `1111` arrives: `ClearedDown`;
  - T1 expires: `Failed`.
- **`Modem::negotiate` (`lib.rs:2221`) must not reset `state` to `Handshaking` when `hold` is set** (R6). Resuming is a retrain as far as the terminal and V.42 are concerned:
  - `start_pump` takes a `resuming: bool` and builds the pump so that `Running` reports `Retraining` (`Analogue::resuming(...)`, like `connected_once`, `startup.rs:138`);
  - `watch_for_retrain` then takes the new rates silently;
  - the V.42 `Stack` is **kept**, because `advance_handshake` is not reached while `state == Data`.
- **Requester side, `ATO` while `Hold::Away`.** This is a new branch in `Action::ReturnOnline` (`lib.rs:2006`):
  - build a calling `v8line::Modem` with `resuming = true` (quick if memo and `+PQC` allow);
  - emit `CONNECT` when data resumes, as V.250 `O` requires. Emit it from `watch_for_retrain`'s Connected arm when a `pending_connect` flag is set, since `ReturnOnline` defers its result.
- **Timers during hold.** Extend the `retraining` predicate in `tick` (`lib.rs:1526`) and in `carry_data` (`lib.rs:1915`) to `|| self.hold.is_some()`, so that:
  - LAPM T401, T400 and the XID wait stay frozen for as long as T1 allows;
  - carrier absence is not read as `NO CARRIER`.

  `carry_data` must also skip `pump` I/O when `pump` is None; it already returns early (`lib.rs:1897`).
- **`+PMHR` reply.** `+PMHR: <c>` goes through `self.at.fmt.info(...)`, then `ResultCode::Ok`:
  - `Granted(t1)` → `+PMHR: <t1>`;
  - `Refused` → `+PMHR: 0`, or 14 after a policy decision.

  Add `Interpreter::info(&mut self, text: &str)` so `modem` does not reach into `fmt` and `regs`.
- **New `Ended` variants:** `ClearedDown` (MHclrd, cleardown from hold, drn = 0 from the far end) and `HoldExpired`. Both map to `NO CARRIER` in `end_call` (`lib.rs:2353`).
- **`distant()` rows:** `"V.92 hold"` → "granted 4 min", "refused", "held 37 s of 4 min".

### 6.4 AT `+P` commands (`crates/at`)

Follow the `V44` pattern (`lib.rs:178-219`, `v44_select` `:893`):
```rust
/// What the +P commands asked for (V.250 6.8, Tables 31 to 37).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct V92 {
    /// +PCW <call waiting>: 0 toggle 125 and collect Caller ID, 1 hang up, 2 ignore.
    pub call_waiting: u8,
    /// +PMH: whether modem-on-hold is enabled. Note the sense: 0 enables it.
    pub hold: bool,
    /// +PMHT: 0 denies a far end's request, 1 to 13 grant it with that T1 code.
    pub hold_timer: u8,
    /// +PIG: whether PCM upstream may be used. 0 enables it.
    pub pcm_upstream: bool,
    /// +PQC: 0 both short phases, 1 short phase 1 only, 2 short phase 2 only, 3 neither.
    pub quick: u8,
    /// +PSS: 0 the DCEs decide, 1 force short, 2 force full.
    pub short_sequence: u8,
}
```
- **Defaults:** `call_waiting 0`, `hold true`, `pcm_upstream true`, `quick 0`, `short_sequence 0`. `hold_timer` has no stated default (Q4); proposal `0` (deny), matching the only example.
- **Interpreter.** Add `pub v92: V92` to `Interpreter` (`:331`) and `self.v92 = V92::default()` to `restore_defaults` (`:425`). Whether `&F` should reset these is not shown in the pages read; follow the `+DS44` precedent.
- **Dispatch.** In `extended` (`:980`):
  - `"PCW" | "PMH" | "PMHT" | "PIG" | "PQC" | "PSS"` → one `p_parameter(op, name, &mut field, &[allowed])` helper, shaped like `reporting()` (`:781`);
  - read → `+NAME: v`;
  - test → the list, as in section 5.2;
  - set → range check → `Ok(Some(Action::SelectV92(self.v92)))`. The empty value is legal only for `+PCW`, `+PMH` and `+PIG` (`=[<v>]`), and means the default.
- **Action commands:**
  - `"PMHR"` → `Execute` → `Ok(Some(Action::RequestHold))`;
  - `"PMHF"` → `Execute` → `Ok(Some(Action::HookFlash))`;
  - add both to `defers_result()` (`:236`), because the modem decides between OK, ERROR and the delayed `+PMHR:`;
  - `Test` should answer OK so the `+GCAP` tests pass (R15).
- **`Action` additions** (`:19`): `SelectV92(V92)`, `RequestHold`, `HookFlash`.
  - They must be handled in **both** `modem::run_actions` (`lib.rs:1996`) and `gui::app::ScopeApp::perform` (`app.rs:430`).
  - `SelectV92` is a no-op in both, like `SelectV44`.
  - In capture mode, `RequestHold` and `HookFlash` emit ERROR or do nothing.
- **`+GCAP`** (`:992`): append `+PCW,+PMH,+PMHT,+PMHR,+PIG,+PMHF,+PQC,+PSS` **only once the modem acts on them**, per the comment at `:988-991`.
- **Tests** (`crates/at/tests/session.rs`), in the file's style: one "all of them" read-back test, one "values outside Table N are refused" test, a `+GCAP` update, and `+MS=V92` accepted.

### 6.5 What V.42 needs for modem-on-hold

- Nothing structural:
  - frames wait while `pump.accepts_bits()` is false (`lib.rs:1967`), and a held or away pump is `None`, so nothing is sent;
  - timers freeze via `tick(0)` (6.3.3);
  - the dictionaries survive because the `Stack` survives.
- Optional: `Stack::hold(&mut self, on: bool)`, which documents the freeze in the stack itself rather than relying on the caller passing 0.
- After resume, T401 recovery (an RR poll, `lapm.rs:396-408`) re-synchronises if the far end moved on. No re-establishment is needed; V.92 does not ask for one.
- If a far end re-establishes anyway, `Event::Reset` already re-initialises compression (`stack.rs:1017-1021`).
- LAPM local busy (sending RNR) is not needed. T403 (idle keepalive) is not implemented, and a far end that uses it will poll into the hold; that is the far end's clock, not ours.

### 6.6 Cleardown (9.11) and the two hang-up buttons

- **Make `ATH` graceful.** `Action::HangUp` (`lib.rs:2000`) becomes: if `pump.clear_down()` returns true, set `hanging_up = true` and wait for `Progress::Failed` with a `ClearedDown` cause, then `end_call(LocalRequest)`, bounded by a timeout (about 3 s plus 2 RTD); otherwise `drop_call()` as now.
  - Add `Pump::clear_down()` for V34, V90 and V90Server, all of which already have the method.
  - Distinguish `ClearedDown` from `Failed` in `Progress` (today both become `Failed`, `lib.rs:163`), so a far-end cleardown can end the call with a reason.
- **Keep "Force hang up"** (`app.rs:1086`, `Session::hang_up` → `Modem::hang_up` → `drop_call`) as the immediate path. Its hover text already promises "nothing more sent to the far end".
- **The GUI "Hang up" button stays `ATH`.** It is shown only when not online, so after Escape. The existing V.90 test `when_one_end_hangs_up_the_other_notices` (`v90_call.rs:332`) should then gain a graceful variant that asserts the far end saw a cleardown, not a signal loss.

### 6.7 The window (`crates/gui/src`)

- **`CARRIERS` (`app.rs:570`).** Append `("V92", "V.92 - up to 56000 down and 48000 up with PCM upstream, quick connect and modem-on-hold. A far end without V.92 gets V.90, then V.34")` at index 6.
  - Append, don't insert: `from_remembered` stores names, but `CEILINGS` and `rates` are index-based.
- **`Modulation::rates` (`app.rs:196`).**
  - Add arm `6` for the V.92 upstream (V.250 `+MS` rates are the sending direction, per the doc at `:2428-2430`): V.34's list plus the PCM-upstream rates 24 000 … 48 000 in steps of 8000/6 (`spec-intro-transmitter.md` §6.1). Rounding needs a decision: 25 333 or 25 334? V.250 wants whole bit/s.
  - Or keep `_` if PCM upstream is not rate-capped from here.
  - Update `the_rate_lists_belong_to_the_carriers_they_are_indexed_by` (`:3098`, `len() == 6`).
- **`CEILINGS` (`app.rs:2431`).** Add `(0, 6, "V.92")` (array length 11) after `(0, 5, "56000")`. `v32bis_can_do_everything_v32_can` (`:3129`) still passes, since `rate == 0`.
- **`call_settings` (`app.rs:2455`).** After the V.8/V.42/compress toggles, add two checkboxes enabled only when `self.carrier == 6`, each typing its command:
  - "quick" → `AT+PQC=0` or `AT+PQC=3`, with hover text "V.92 9.2 and 9.4: short phase 1 and 2 on a connection this modem has seen before";
  - "hold" → `AT+PMH=0` or `AT+PMH=1`.

  Put the fields into a new `Pcm` struct next to `Protection` (`:92`), give it `commands()`, and add it to `settings()` (`:1759`) and to `to_remember`/`from_remembered` (`:2859`, `:2887`). The keys would be `pcw`, `pmh`, `pmht`, `pig`, `pqc`, `pss`.
- **`line_controls` buttons (`app.rs:1039-1095`).** Beside Escape and Hang up:
  - "Hold" (`AT+PMHR\r`): enabled when `frame.state == OffHook` (online command) and the modulation is V.92;
  - "Resume" (`ATO\r`): enabled when `frame.state == OnHold`;
  - "Flash" (`AT+PMHF\r`): enabled when `OnHold`;
  - "Call waiting" (simulated): `session.call_waiting()`, with hover text saying it stands in for a network indication the softphone never passes on.
- **Retrain button (`app.rs:3014`).** Enable it for `"V.34" | "V.90" | "V.92"`. `Modem::retrain` already handles V.90 (`lib.rs:1871-1876`), and the button is disabled there today for no reason.
- **Status grid (`app.rs:2561`).** Add a "hold" row (state and T1 left) and a "start-up" row (quick or full, short phase 2 yes or no).
- **`live.rs`:**
  - `Session` (`:224`) gets `call_waiting: AtomicBool`, handled next to `take_retrain` (`:1043`);
  - hold transitions are logged like retrains (`:1209-1231`);
  - `f.state` mapping (`:1331`) gains `_ if modem.on_hold() => CallState::OnHold`, checked first;
  - `learned()` is saved after a V.92 call, and `quick_memo` is set every block like the `fax_*` fields (`:1008-1013`).
- **`telemetry`:** `CallState::OnHold` with label `"on hold"` (`lib.rs:47-71`). `Leds.oh` stays true. The `distant` capacity comment (`:193-195`) is already wrong (R17).
- **`answer.rs`:** add `V92` to the help text (`:62`), and add `--grant-hold <n>`, which types `AT+PMHT=<n>` after `+MS`.
- **`engine.rs`:** nothing. `Standard::V92` already exists for capture labelling.

### 6.8 Tests to add (with the existing harnesses)

- `crates/v8`, inline `quick_tests`: the table vectors in 5.1, the collision cases (`0xE0`, `0x00`, `0x55`), and a round trip through `Qc::bits()` → `AsyncBits` → `Watcher`.
- `crates/datapump/src/v8.rs` tests:
  - an analogue caller with a memo plus a digital answerer: `Status::Quick` on both sides within about 4 s;
  - a digital answerer that never hears TONEq: ANSam comes back, and V.8 completes;
  - a caller hearing plain 2100 Hz after QCA1d does **not** report `NoNegotiation`.
- `crates/modem/tests/v92_call.rs`, copying `v90_call.rs`'s `Pair`:
  - two `+MS=V92` modems connect, with `standard() == "V.92"`;
  - a second call with the memo connects faster, with fewer V.21 seconds;
  - `AT+PMHR` then `+PMHR: <t1>`; `ATO` resumes, and text still crosses with the same `Stack` (`damaged_frames` continuity);
  - `+PMHT=0` gives `+PMHR: 0`;
  - ATH on hold gives the held side `NO CARRIER` within 2 s;
  - a round-trip sweep 0 to 1.5 s (memory `voip-line-round-trip`).
- `crates/at/tests/session.rs`: section 6.4.
- `gui` unit tests: `CARRIERS`/`rates`/`CEILINGS` coupling, and remembered-key round trip (see `what_the_last_run_was_set_to_comes_back`, `app.rs:3232`).

---

## 7. House style to match

- **Module docs (`//!`) tell the story first.** Say why the thing exists and what went wrong without it, then what is here and what is not. Example: `crates/v8/src/lib.rs:1-26`.
- **Every public item and most fields carry doc comments**, often several paragraphs. Comment density is very high; comments explain reasons and live-call history, not mechanics. Examples: `lib.rs:1506-1525`, `v8.rs:344-358`.
- **Clause citations are bare and precise**, and quotes are short:
  - bare: `8.1.1`, `7.4`, `Table 5/V.8`, `9.1.1/V.90`, `V.250 6.4.1`, `V.42 Appendix IV`;
  - quoted, e.g. `// 8.2.2: "upon receiving a minimum of 2 identical CM sequences, the DCE shall transmit JM".`
  - Constants say whose number it is: "Not a figure from the Recommendation, which leaves this to the modem" (`v8.rs:82-86`), "no ITU default, Hayes value used" (`registers.rs`).
- **Live evidence is cited by capture name** (`live-1789546478`, `lib.rs:1551-1553`) or date (`phase2.rs:115`).
- **British spelling:** synchronisation, analogue, behaviour, recognised, organised.
- **Names are plain English**, not abbreviations: `offering_lapm`, `declared_lapm`, `without_detection`, `take_retrain`, `far_end_went`, `accepts_bits`.
  - Spec signal names stay as the spec writes them (`Info0d`, `Info1a`, `Mp`, `Cp`), CamelCased.
  - Timing modules are named `timing` and constants are SCREAMING_SNAKE.
- **Idioms:**
  - builder methods `fn x(mut self) -> Self` (`offering_*`, `with_*`, `without_*`, `declining`);
  - `Status` enums with `Failed(&'static str)` reasons;
  - a private `State` with `enter()` that resets `elapsed`;
  - `phase() -> &'static str` for scopes and transcripts;
  - `take_*` for read-once flags and queues (`take_retrain`, `take_bits`, `take_dte`);
  - boxed large pumps in enums;
  - `let … else`, let-chains (`if let … && let …`), `is_some_and`, `is_none_or`;
  - time is `f64` seconds in `datapump`, `u32` ms in `modem`/`ec`, and `u64` sample counters in pumps.
- **Bit fields:** `octet([bool;8])` with inline `// b0..b3: the … tag, 1010` comments (`v8/src/lib.rs:406-410`). The table's printed order is kept in comments, and any mirror-image trap is written down (`lib.rs:360-365`).
- **Tests are named as sentences** (`a_joint_menu_carries_pcm_only_for_a_pair`), with a comment quoting the rule under test and, often, the bug that prompted it. Real-capture byte arrays are pinned as constants.
- **Layering rule, stated in `app.rs:694-701`:** GUI controls type AT commands. The only bypasses are the Session atomics for things a terminal cannot do in time ("Force hang up", "Retrain"). New buttons should prefer AT.
- `#![forbid(unsafe_code)]` and workspace lints apply. Lines run to about 120 columns in newer code.
- In `at`, every accepted value must be one `=?` advertises (V.250 5.4.2 is cited at `lib.rs:706-710`), and `+GCAP` lists only what is answered.

---

## 8. Risks and pitfalls

- **R1: QC octets collide with V.8 sync octets.**
  - QC1a with P=0 and WXYZ=`0111` is `0xE0`, which equals `SYNC_MENU`.
  - With P=0 and WXYZ=`0000` it is `0x00`, which equals `SYNC_CI`. A run of those also feeds the CJ zero counter (`v8/src/lib.rs:698`).
  - The QC sync `0x55` is itself a legal modulation extension octet (`is_extension`: b3=0, b4=1, b5=0).
  - The existing decoder mis-reads these harmlessly: a body-less "menu" parses to nothing, and a bogus CI is ignored. A QC detector must still match the whole `[0x55,q,0x55,q]` pattern and must not reuse `Decoder`'s sync state.
- **R2: `Bell103Tx` cannot abandon a character.** V.92 9.2.1.1 and 9.2.2.1 require stopping CM "without completing the current octet". Today `SendingCm` → `SendingCj` lets the queued bits run (`v8.rs:425-436`).
- **R3: QC and QCA are sent once.**
  - `IDENTICAL = 2` cannot apply; use the in-sequence repeat.
  - One slip (memory `voip-jitter-slips`: about 20 ms is 6 bits at 300 bit/s) can wreck the only QCA1d. The fallbacks then have to work: the digital side times out in 2 s and returns to ANSam and V.8, and the analogue side must keep sending CM and accept JM.
- **R4: Exact codewords.** QTS, QTS\ and ANSpcm are PCM and must leave at unity gain with frame alignment. V.8 today runs at the line rate through `drive` (`live.rs:1106-1116`), and `exact_levels()` is true only for `Pump::V90Server`. So these signals must be generated inside the digital pump, not in `datapump::v8`.
- **R5: No path from a pump back to V.8.** 9.2.4.3 (no TONEq), 9.2.1.1 (JM after QC1a), the held modem's return to Phase 1 and MHfrr all need one. `Modem::step`'s dispatch (`lib.rs:1477-1487`) and `negotiate`/`start_pump` assume V.8 comes first and once.
- **R6: `start_pump` always sets `state = Handshaking`** (`lib.rs:2349`), and `advance_handshake` always builds a new `Stack` (`lib.rs:1647`). A resume after hold must bypass both, or the terminal sees a second CONNECT and V.42 restarts (losing unacknowledged data, 8.2.4.3).
- **R7: Round trip on the rig is 1.1-1.5 s.**
  - The digital side's 2 s TONEq timer (9.2.4.3) counts from the end of QCA1d and **is not RTD-compensated**. At about 0.75 s each way, TONEq reaches the server about 1.8 s after QCA1d, and only if the caller starts TONEq **on first detection** of ANSpcm (allowed by 9.2.1.3).
  - Waiting the full 1 s of ANSpcm would miss the deadline, so quick connect over Crazytel is marginal.
  - For our own server, consider adding the measured RTD from the last call's memo; that is a local deviation, and should be documented.
  - MH timers (2 s + RTD, 200 ms response stop) have the same exposure (see `spec-modem-on-hold.md` §9.3).
- **R8: ANSpcm is an unmodulated 2100 Hz tone.** `AnswerTone::is_plain()` reads it as V.25 ANS. The calling V.8 machine's `Listening` state turns a held plain tone into `NoNegotiation` (`v8.rs:402-410`). While waiting for ANSpcm, "plain" must mean ANSpcm. ANSpcm also has 451.5 ms reversals, which `AnswerTone` is designed to ignore.
- **R9: Hold defeats three existing guards.**
  - Carrier loss: `carry_data`, `lib.rs:1916`.
  - V.42 clocks: `tick`, `lib.rs:1526`.
  - V.8's `ANSAM` 5 s and `PATIENCE` 60 s: `v8.rs:80`, `:86`, `:376`, `:470`.

  Each must learn about hold, or a granted hold ends the call at once, or after 5 s or 60 s.
- **R10: A zero-modulation CM (cleardown from hold) ends with the wrong result code.** It already produces a zero JM and `Status::Failed` (`v8.rs:494-501`), but `negotiate` maps that to `end_call(NoAnswer)` and `NO ANSWER`. A held call being cleared should report `NO CARRIER`.
- **R11: `ATH` never sends a cleardown.** `clear_down()` is unused in `modem`, and V.92 9.11 says a connection is ended with the cleardown procedure. Changing `ATH` to be graceful changes the timing of `OK` after `ATH`, which V.250 6.3.6 leaves to the DCE. Check the `call.rs` hang-up tests (`:263`) and dial-in behaviour (`live.rs:890-894`, `:914-917`, which use `hang_up()` and are unaffected).
- **R12: Cost of new `Pump` variants.** A variant touches about 20 exhaustive matches in `lib.rs:111-440` plus `constellation_peak`, `retrain`, `ask_for_retrain`, `retrains`, `echo_*` and `reflection`. Prefer V.92 flags inside `Pump::V90`/`V90Server`, with `standard()` deciding the name.
- **R13: `at::Action` is matched exhaustively twice** (`modem/src/lib.rs:1996`, `gui/src/app.rs:430`). Every new variant breaks both until handled.
- **R14: Index coupling in the GUI.** `CARRIERS` ↔ `Modulation::rates(usize)` ↔ `CEILINGS`, pinned by tests (`app.rs:3098-3135`). Append V.92 at index 6; do not insert.
- **R15: The `+GCAP` tests send `AT<name>=?` to every listed command.** `+PMHR` and `+PMHF` are actions with no defined test syntax, so they must still answer that without ERROR, or stay unlisted.
- **R16: The quick-connect memo has no reliable key.** The GUI's Originate is a bare `ATD` (the softphone dials), so a memo from one server can be spent on another. The protocol recovers (no QCA1d, or a wrong U_QTS gives a poor QTS and ANSpcm check), but only after seconds. Consider a "number/label" field next to Originate, typed as `ATD<label>`.
- **R17: `telemetry` says publishing never allocates** (`lib.rs:3-4`, `:193-195`), but `distant` already holds more than 10 owned `String` rows (V.8, V.42 and V.34 rows). V.92 rows add to it. This is harmless on the line thread today, but the comment is false.
- **R18: `+PMHF` (hook flash) cannot be performed.** The softphone owns the line and there is no DAA. V.250 still makes the command mandatory for a V.92 DCE. Proposal: OK and a log note while on hold, ERROR otherwise, and say so in the hover text.
- **R19: Call waiting is invisible.** `+PCW` can only be exercised with the simulated button. Nothing on the line can be relied on: a SAS/CAS tone would have to be passed through by the softphone, and MicroSIP does not.
- **R20: A retrain can look like a hold request** (MOH-9.10.1.1-R5 and Q8 in the MOH digest). The existing Tone A/B retrain watches (`analogue.rs:1217`, `digital.rs:734`) fire first. Both watches must hand a validated MH frame priority over a reversal.
- **R21: Existing inconsistencies V.92 work will trip over.**
  - `+MS=?` advertises `(300-4800)` (`at/src/lib.rs:696`).
  - Idle `Modem::standard()` has no `V90` arm (`lib.rs:1086-1092`).
  - The Retrain button is V.34-only (`app.rs:3014`).
  - The doc comment for `distant()` sits above `v34_report()` (`lib.rs:1170-1180`).
  - `+MS` silently drops `<min_rx_rate>`/`<max_rx_rate>`.
- **R22: Frame alignment across phases.** The digital modem must keep the 8 kHz data-frame count from the first QTS symbol through Phase 2 and beyond (8.3.6). `server::Line` introduces resampler delay (`server.rs:86-101`, `SLACK = 8`), so alignment must be counted in the 8 kHz domain inside `Digital`, not in line samples.

---

## 9. Open questions

- **Q1.** How does the analogue modem choose U_QTS, and what exactly must a "recognized connection" memo hold for short Phase 2, 3 and 4? V.92 says nothing on this; see `spec-phase2-procedures.md` A3, A5, A10.
  - Proposal: the highest Table 2 Ucode that the last call's DIL showed clean, plus RTD, law, upstream pre-emphasis and rates.
- **Q2.** What should the memo be keyed on when `ATD` carries no number (R16)?
- **Q3.** Which ANSpcm level (LM) should our digital modem use?
  - Candidates: −12 dBm0, or tie it to INFO0d's nominal power (`server::ours()`: `nominal_power = 4`).
  - Also: is −9.5 dBm0 too hot for the softphone's gain control (memory `v90-live-test-pending`: codewords above about 0.32 of full scale were held back)?
- **Q4.** What should the `+PMHT` default be (V.250 gives none)? And should the server (`answer.rs`) grant by default?
- **Q5.** DTE view of hold:
  - should the modem stay in `OnlineCommand` or return to `Data` after `+PMHR: n`?
  - which result does the **held** side's DTE see when the far end returns: nothing (retrain-like) or a new `CONNECT`?
  - V.250 only says `O` reconnects.
- **Q6.** Should `+MS=V92` replace `V90` as the GUI default for 56k? A V.92 modem runs V.90 against a V.90 server once the INFO0 bits say so.
- **Q7.** With 9.2.5 or 9.3.1 bypassing detection, is `UNCONFIRMED_N400 = 3` right? The far end has declared LAPM twice (P bit and V.8 prot0), so `DEFAULT_N400` may be the better fit. Phase 2 digest P10 also says to tolerate an ODP anyway.
- **Q8.** Does `tests/vectors/v92-56k.wav` contain a QC1a?
  - It is a Conexant V.92 modem dialling the same server straight after the V.90 capture, so it may have had quick-connect data.
  - Check: decode V.21(L) octets before the CM and look for `0x55 q 0x55 q`. A real QC1a would pin the octet mapping in 5.1 against a real modem. Replay only; no build was done here.
- **Q9.** Is modem-on-hold allowed when a V.92 pair fell back to V.34 data mode? V.92 9.10 speaks of V.92 operation, while RT/MH use V.90 phase 2 tones in either case.
- **Q10.** Do the NetZero/GlobalPOPs servers grant MHreq? They are the only live V.92 far ends available (memory `live-v34-isp-far-end`). The first live hold test should be `AT+PMHR` against them, with a recording running.
- **Q11.** Should PCM-upstream rates appear in the GUI rate boxes at all? `+MS` rates are the transmit direction, and V.92's 8000/6 steps are not whole numbers.
