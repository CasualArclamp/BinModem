# V.34 code map for half-duplex fax (V.34 clause 12 under T.30 Annex F)

Tree: `main` at `fa5c451`, `F:\dialupmodem2`; paths are relative to it. Nothing in the
repository was changed. I checked the half-duplex bit tables quoted here (Table 22 INFOh,
Tables 23/24 MPh, 10.2.4 control-channel mapping, 10.2.4.5 PPh) against the rendered PDF
pages of `docs/specs/T-REC-V.34-199802-I.pdf` (page indexes 41-45) as well as the extracted
text, which is lossy.

An existing deep-dive on the receiver is `docs/design/slow-modes/v34-reference.md`. Its line
numbers come from an older tree (`48b9087`), so re-check them before relying on them.

---

## 0. Findings in brief

1. **V.34 here has no echo canceller.** A grep of `crates/datapump/src/v34` for echo/cancel
   finds comments only. `dsp::EchoCanceller` (`crates/dsp/src/echo.rs:631`) is used only by V.32
   (`v32/startup.rs:18, 2260, 2366`). Phase 2 separates the two directions with FDM filters.
   Phases 3 and 4 and data mode rely on a 4-wire-like line: the training test link has no echo
   path (`training.rs:1826-1844`). Half-duplex loses nothing here.
2. **The primary-channel DSP can run one way without changes.**
   - `qam::Transmitter` pulls symbols.
   - `receiver::Receiver` hunts, then trains on a known sequence, then tracks. Its state
     survives `idle()`.
   - `data::{Encoder, Decoder, Acquirer}` are built from `Params`: rate, trellis,
     non-linear, shaping, precoding and scrambler. That is exactly what an MPh exchange yields.

   V.90 already builds its own start-ups from these parts (`v90/digital.rs:543-544, 926, 953`;
   `v90/analogue.rs:1599, 3006`).
3. **The control channel's bit layer already exists.**
   - `signals::Sender::differential` and `Reader::differential` (`signals.rs:145-150, 235-245`)
     are 10.2.4's mapper exactly: 2·Q2+Q1 picks the Figure 5 label, the point turns clockwise
     by Z·90°, and Z = Z₋₁ + 2·I2 + I1. At 1200 bit/s the Q bits are 0.
   - The scrambler is clause 7's (`v32::Scrambler`, GPC or GPA by `v32::Mode`,
     `v32.rs:95-118, 347-386`).
   - E is `Sender::sequence(&[true; 20], Size::Four)`, and `mp::Finder` detects it
     (`mp.rs:217-220`).
   - **MPh has MP's lengths, sync, start bits, CRC coverage and fill.** Only the meanings of
     some fields differ (Tables 23/24 against `mp.rs:67-87`).
4. **What the control channel lacks:**
   - the 600-baud QAM modem itself (modulator and FDM demodulator);
   - PPh, ALT, AC and their detectors;
   - the procedures of 12.4, 12.6 and 12.8.
5. **INFOh is cheap to add.** `info::{frame, unframe, crc}` are generic over the information
   length (`info.rs:49-62, 133-159`). `dpsk.rs` needs one more length per side (`87-92`) and one
   more match arm (`373-381`).
6. **Phase 2 has "both ends probe each other" built into its `Stage` enum and `stage_step`**
   (`phase2.rs:191-225, 848-1048`), including ranging and INFO1c/INFO1a.
   - Its building blocks fit half-duplex as they are: tone presence over the echo of this
     end's own L2, reversal timing, DPSK tx/rx, L1/L2, the analyser.
   - The stage chains must be new: four variants, call/answer × source/recipient.
7. **The probe analysis fills INFOh almost directly.** `Reading::probed(rate, far_info0, wide)`
   gives high carrier, pre-emphasis (0-5 only) and maximum rate for each symbol rate
   (`probe.rs:372-393`).
   - Power reduction and MD are always 0 today (`phase2.rs:927-929, 1111-1113`).
   - INFOh's TRN length and TRN constellation fields need a new policy.
8. **`training.rs` (phases 3 and 4, data, renegotiation) and `startup.rs` are duplex throughout.**
   - J, J′, MP, MP′ and E all run at the primary rate.
   - `Settings` needs INFO1c and INFO1a.
   - Every sample drives both tx and rx.
   - Renegotiation is detected by watching for S in data mode.

   Half-duplex needs a new primary-channel module and a new top-level state machine. The
   private `Source` in `training.rs` (`322-515`) is the template.
9. **The receiver needs two small changes:**
   - `Reference::PpThenTrn` hard-codes 4-point TRN (`receiver.rs:528-531, 1233-1234`).
   - Nothing trains on PP alone, which 12.5's resync needs (S, S̄, PP, B1). A `Reference::Pp`
     whose solve window ends at PP's last symbol leaves `next_symbol` on B1's first symbol
     (`receiver.rs:764`).
10. **Control-channel receiver: build on `dsp::qam::Core` rather than V.22bis.** The Core needs
    an FDM channel-select pre-filter: at 600 baud its 64-tap front end cannot reject the other
    direction or the answer modem's own 1800 Hz guard tone.
11. **Control-channel transmitter: build from `dpsk.rs`'s modulator made complex.** It already
    has the exact carriers, levels, guard tone and Figure 13 pulse.
12. **The modem crate runs fax outside the `Pump` enum, so a V.34 fax cannot be a fourth
    `Carrier`** (`faxcall.rs:25-46`).
    - Fax lives in `Modem.fax: Option<FaxCall>` (`lib.rs:835, 2188-2211, 2330-2349`), and V.8
      is skipped on purpose (`lib.rs:2184-2187`).
    - The V.21 control channel is never listened to while this end talks
      (`faxcall.rs:482-524`).
    - A V.34 fax needs V.8 first, a full-duplex control channel, and the T.30 Annex F
      procedure.

---

## 1. File by file

Line counts: **total / non-test (before `#[cfg(test)]`) / code only (no comments or blanks)
/ unit tests.**

### `crates/datapump/src/v34.rs` (36): the module root and its doc comment. Nothing else.

### `constellation.rs` (115 / 53 / 26 / 4)
- **Provides:**
  - `QUARTER` = 416 (`:19`) and `Point` = (i32, i32) (`:22`);
  - `quarter(label)` (`:36`), built by 9.1's rule into a `OnceLock` (`:24-33`);
  - `clockwise` (`:41`) and `counterclockwise` (`:50`).
- **State:** the static table only.
- **Callers:** `signals.rs`, `data.rs`, `trellis.rs`, `receiver.rs`, `training.rs`,
  `v90/analogue.rs`.
- **Half-duplex:** reuse as is. The control channel's points are `quarter(0..=3)` turned by Z.

### `info.rs` (688 / 515 / 342 / 9)
- **Provides:**
  - `FILL` and `SYNC` (`:15, :19`);
  - `INFO0_BITS` 49, `INFO1C_BITS` 109, `INFO1A_BITS` 70, `INFO0D_BITS` 62 (`:22-30`);
  - `crc()` (`:49-51`) with `shift()` (`:54-62`): x¹⁶+x¹²+x⁵+1, preset to ones, not inverted;
  - `SymbolRate` (`:67-106`: `ALL`, `from_index`, `index`, `nominal`);
  - private helpers `put`/`get` (`:109-118`), the offset fields (`:122-130`) and `frame()`
    (`:133-141`);
  - `pub unframe(bits, length)` (`:149-159`);
  - `Info0` (`:163-237`), `Probed` (`:242-266`), `Info1c` (`:271-312`), `Info1a` (`:316-361`),
    `Info0d` (`:371-447`), `Info1aPcm` (`:457-501`);
  - the `enum Info` that `dpsk::Receiver` hands over (`:505-514`).
- **State:** none; these are pure codecs.
- **Callers:** nearly every V.34 module, V.90, and the reports in `modem/src/lib.rs`.
- **Half-duplex:** INFO0 is unchanged (10.2.2). INFOh is new (~50 lines), plus an `Info::InfoH`
  variant.
  - The layout is 12:14 power reduction, 15:21 TRN length in 35 ms steps (0-127), 22 high
    carrier, 23:26 pre-emphasis, 27:29 symbol rate, 30 16-point TRN, 31:46 CRC, 47:50 fill:
    51 bits (Table 22, checked on the rendered page).
  - Adding the variant breaks two exhaustive test matches (`tests/v34_vector.rs:56-62`,
    `tests/v90_vector.rs:~100-107`).

### `dpsk.rs` (619 / 388 / 230 / 7)
- **Provides:**
  - `BAUD` 600 (`:23`), `ROLLOFF` 0.75 (`:33`, checked against Figure 13 by the test at
    `:549-603`), `GUARD_TONE` 1800 (`:40`);
  - `Side` (`:44-93`): carrier 1200/2400 (`:53`); answer carrier −1 dB (`:66-71`); guard tone
    −7 dB (`:73-78`); private `lengths()` (`:87-92`);
  - `Transmitter` (`:97-229`): `new`, `send` (`:139`; a sequence sent while the carrier is up
    continues its group), `silence` (`:146`), `pending`, `reverse` (`:164`, on the very
    sample), `stop` (`:177`), `is_sending`, `next_sample` (`:204`);
  - `Receiver` (`:264-347`): `new(side)` with a channel-select FIR of 560 Hz and fs/40|1 taps
    (`:291`) and a root-raised-cosine matched filter (`:292`); `level`; `feed` returning
    `Option<Info>` (`:314`);
  - private `decide()` (`:350-387`).
- **State:**
  - Tx: carrier and guard NCOs, symbol clock, 13-symbol history, pending bits, the current
    point ±1, and whether the carrier is on.
  - Rx: NCO, select and matched filters, eight sampling branches (`PHASES`, `:236`) each
    holding up to 109 bits, and a level.
- **Design:** no timing loop and no carrier loop. The receiver samples at eight phases in
  parallel, decides differentially, and lets the CRC pick the branch (`:231-262, :350-362`).
  That is only valid for short CRC-checked bursts.
- **Callers:** `phase2.rs` (tx `:375`, rx `:383`), `tests/v34_vector.rs`,
  `tests/v90_vector.rs`, `tests/v34_capture.rs`.
- **Half-duplex:**
  - Tones A/B, reversals, INFO0: reuse as is.
  - INFOh: add `INFOH_BITS` to both sides' `lengths()` and a `(_, INFOH_BITS)` arm in
    `decide`, because the recipient may be either modem.
    - Today every call-side length that is not INFO0 or INFO0d falls through to `Info1c`
      (`:376`).
    - The branch window (109 bits) already covers 51.
  - Control channel: the modulator's carriers, levels, guard tone and pulse are 10.2.4's
    exactly. But it is binary: the history is real and the output is `baseband * cos`
    (`:105-108, :216-227`), so it needs complex symbols. The receiver cannot be reused for
    continuous QAM.

### `probe.rs` (607 / 395 / 266 / 9)
- **Provides:**
  - `TONES` (Table 17, read off the PDF, `:22-44`), `L1_SECONDS` 0.16 (`:48`), `L2_SECONDS`
    (`:52`), `REPETITION` (`:55`);
  - `Generator::next_sample(loud)` (`:68-89`);
  - `Tone` and `Reading` (`:93-107`);
  - `Analyzer` (`:120-245`): `new`, `reset`, `windows`, `feed`, `reading`;
  - `carriers(rate)` from Table 2 (`:248-257`), `symbols_per_second` from Table 1
    (`:260-270`), `ceiling` (`:274-282`), private `floor` (`:285`), `GAP_DB` 6 (`:295`),
    private `transmits(far, rate, high)` (`:299-308`);
  - `Reading::probed` (`:372-393`) and private `pre_emphasis` (`:358-364`).
- **State:** the generator's sample count; the analyser's window buffer and per-window complex
  readings.
- **Callers:**
  - `phase2.rs` (generator `:376`, analyser `:387`, readings `:891/:1027`, `probed`
    `:923/:1103`);
  - `qam.rs` and `training.rs` (tables);
  - `v90/digital.rs`;
  - the modem crate's reports.
- **Half-duplex:** reuse as is. The source uses the `Generator`; the recipient uses `Analyzer`
  and `Reading::probed` (see Q2).

### `phase2.rs` (1511 / 1121 / 727 / 14); state machine in §2.2
- **Provides:**
  - `Role` (`:34-53`, private `side()` and `far()`), `Pcm` (`:64-81`), `Status` (`:85-93`),
    timing constants (`:97-187`);
  - private `Stage` (`:191-225`), `Speaking` (`:229-235`) and `Presence` (`:239-293`);
  - `Modem` (`:297-360`):
    - constructors: `new` (`:410-422`), `v90` (`:425-436`), `v90_retrain` (`:441-446`),
      `again` (`:450-465`), `retrain` (`:526-544`);
    - accessors: `role`, `status`, `phase`, `far_capabilities`, `round_trip`, `reading`,
      `recoveries`, `info1c`, `info1a`, `asks_for_retrain` (`:570-644`);
    - `step` (`:647-681`);
    - private logic: `timers` (`:684-708`), `speak` (`:710-716`), `heard` (`:723-786`),
      `reversal` (`:788-842`), `stage_step` (`:848-1048`), `settle_pcm` (`:1052-1077`),
      `settle` (`:1081-1119`).
- **State:**
  - role, clock, stage, status, deadline;
  - DPSK transmitter, probe generator, and what is being sent now;
  - the samples at which a reversal, silence, the probe or a tone is due;
  - DPSK receiver, `ReversalDetector`, `Presence`, analyser and its L2 window;
  - our and the far end's INFO0, round trip, reading, INFO1c/INFO1a, and the V.90 extras.
- **Callers:** `startup.rs` (`:34, :49, :55, :184, :199, :212-229, :235-240`),
  `v90/startup.rs` (`:64, :348`), `training.rs` (`Role`, `TONE_B_FLOOR`), the modem crate's
  `V34Report` (`lib.rs:498-510`).

### `signals.rs` (349 / 247 / 164 / 5)
- **Provides:**
  - `S_SYMBOLS` 128, `S_BAR_SYMBOLS` 16 (`:12-13`), `PP_PERIOD` 48, `PP_SYMBOLS` 288
    (`:16-17`), `TRN_SYMBOLS` 512 (`:20`), `E_BITS` 20 (`:23`), the J patterns (`:26-30`);
  - `Size` {Four, Sixteen} (`:45-65`);
  - `s(n)`, `s_bar(n)` (`:76, :82`) and `pp(i)` (`:91-96`);
  - `Sender` (`:101-169`): `new(mode)`, `restart`, `trn(size)`, `differential(bits)`,
    `sequence`;
  - `decide(symbol, size)` (`:180-196`);
  - `Reader` (`:173-246`): `new(mode)`, `trn`, `differential`.
- **State:** the Sender holds a scrambler and Z; the Reader holds a descrambler and Z.
- **Callers:** `training.rs`, `receiver.rs` (sequences, `decide`), V.90, tests.
- **Half-duplex:**
  - Primary channel: reuse as is (S, S̄, PP, TRN at 4 or 16 points).
  - Control channel: reuse as is as the mapper.
    - `differential` with 2 bits sets label 0 unscrambled (`:148`), which is "Q1 and Q2 set
      to 0" at 1200 bit/s.
    - Sh and Sh̄ are `s()` and `s_bar()` (10.2.3.3 defines them identically) sent at 600 baud.
  - New: PPh (10-2), AC and ALT.

### `qam.rs` (411 / 272 / 187 / 5)
- **Provides:**
  - `ROLLOFF` 0.1 (`:20`);
  - `Band` {rate, high_carrier} (`:32-65`: `baud`, `carrier`, `samples_per_symbol`);
  - `emphasis_db`, `emphasis_filter`, `response` (`:95-164`);
  - `Transmitter` (`:173-271`): `new(band, pre_emphasis, reduction, fs)` (`:197`), `band`,
    `symbols`, `lookahead()` = 20 (`:243`), and `next_sample(|| symbol)` (`:248`), which pulls
    symbols as the pulse needs them.
- **State:** pulse table, 41-symbol history, carrier phase, emphasis FIR, gain, symbol count.
- **Callers:** `training.rs:836`, `receiver.rs` (`Band`), `v90/analogue.rs:1599`,
  `v90/digital.rs`, tests.
- **Half-duplex:**
  - Primary source: reuse as is, built from INFOh's symbol rate, carrier, pre-emphasis and power
    reduction.
  - Not for the control channel: `Band` is a V.34 `SymbolRate`, and the roll-off is 10%.

### `receiver.rs` (1676 / 1304 / 913 / 7); detail in Q3
- **Provides:**
  - `Slicer` (`:143-216`), `Reference` {PpThenTrn, Trn(Size)} (`:220-225`);
  - `Heard` {S, Reversal{at}, Trained{snr_db}, Untrained, Symbol} (`:229-240`), `Symbol`
    (`:244-251`);
  - `Receiver` (`:375-437`):
    - `new(band, fs)` (`:454-511`);
    - `hunt` (`:518`), `train(reference, far_mode, s_bar)` (`:525-533`), `idle` (`:536`),
      `resume(first)` (`:548-559`);
    - `set_size` (`:566`), `set_grid` (`:577`);
    - readings: `snr_db`, `last_point`, `level` (`:607`), `slips`, `is_lost`,
      `trained_snr_db`, `drift_ppm`, `halves`;
    - `heard` (`:638`), `feed` (`:642-658`), `rewind` (`:997-1013`);
  - `pub unit(size)` (`:1243`);
  - private: `Hunt` (`:285-364`), `finish_training` (`:741-783`), `solve_known` (`:786-845`),
    `reacquire` (`:857-906`), symbol tracking (`:909-991`), resyncs (`:1027-1211`),
    `windows` (`:1217-1225`), `sequence` (`:1228-1238`).
- **Callers:** `training.rs:742/:838`; `v90/digital.rs:543-544, 926, 953`; `v90/analogue.rs`;
  `tests/v34_vector.rs:124-177`; `tests/v34_capture.rs`.

### `training.rs` (2309 / 1740 / 1235 / 15); state machine in §2.3
- **Provides:**
  - `Status` (`:55-68`);
  - `Settings` (`:72-126`), built by `new(role, far, info1c, info1a, rtd, wide)`;
  - `RetrainWatch` (`:237-302`, pub(crate)) and `SWatch`/`Watched` (`:638-684`, pub(crate));
  - `Modem` (`:730-812`):
    - `new` (`:817-891`);
    - actions: `renegotiate` (`:933`), `clear_down` (`:945`), `step` (`:1177-1211`),
      `take_retrain` (`:1228`), `start_retrain` (`:1234`), `take_bits`/`send_bits`/
      `pending_bits` (`:1467-1484`);
    - readings: `rates` (`:1148`), `b1_errors`, `path_cost`, `carrier` (`:1498`), and
      scope/report getters;
  - `pub fn negotiate(call, answer)` (`:1726-1739`).
- **Private:** `Segment` (`:306-318`), `Source` (`:322-515`), `Listening` (`:528-626`), `Stage`
  (`:688-726`), `heard` (`:1242-1335`), `reversal` (`:1338-1371`), `event` (`:1373-1433`),
  `transmit_params`/`receive_params` (`:1438-1464`), `make_mp` (`:1503-1530`), `stage_step`
  (`:1551-1634`), `exchange_mp` (`:1637-1691`).
- **Callers:** `startup.rs:35, :228`; V.90 (`RetrainWatch`, `SWatch`); the modem crate's
  reports.

### `mp.rs` (309 / 245 / 182 / 4)
- **Provides:**
  - `TYPE0_BITS` 88, `TYPE1_BITS` 188 (`:13-14`), `SYNC_ONES` 17 (`:17`);
  - `Trellis` (`:21-26`), `Coefficient` (`:30`);
  - `Mp` (`:33-55`): `acknowledged`, `to_bits` (`:96-142`), `from_bits` (`:148-181`),
    `rates_up_to`;
  - `Finder` and `Found` {Mp, E} (`:191-244`).
- **Private:** `put`/`get`, `start_bits` (`:69-75`), `crc_at` (`:78-80`), `covered`
  (`:84-87`).
- **Callers:** `training.rs`, V.90 (`analogue`, `digital`, `sequences`), tests, the modem
  crate.

### `data.rs` (1269 / 965 / 729 / 10); detail in Q5
- **Provides:**
  - `THETA` (`:37`); `Params` {framing, code, nonlinear, precoding, mode} (`:44-58`);
  - `Encoder` (`:266-365`): `new(params)` (`:284`), `params`, `mapping_frames`,
    `next_symbol(source)` (`:312`), `grid_scale`;
  - `extent` (`:369`) and `peak` (`:382`);
  - `Decoder` (`:476-717`): `new(params)` (`:510`), `resume(params, m)` (`:545`),
    `grid_scale`, `peak`, `extent`, `path_cost`, `outside`, `take_bits`, `feed(symbol)`
    (`:591`);
  - `Acquirer` and `Acquired` (`:733-927`), which find the frames after a slip or a lost E.
- **Private:** precoder (`:128-162`), `inversion_at` (`:166-173`), `parse`/`unparse`
  (`:191-262`), Viterbi (`:599-717`).
- **State:**
  - Encoder: shell mapper, scrambler, trellis state, Z, precoder, frame count, ready symbols,
    energy scale.
  - Decoder: descrambler, path metrics, 40-stage traceback, precoder replay, Z, groups.
- **Callers:** `training.rs`; `v90/analogue.rs:3006`; `v90/digital.rs:1095`;
  `tests/v34_capture.rs`.

### `frame.rs` (276 / 164 / 115 / 4)
- **Provides:**
  - `Framing` (`:18-45`), built by `new(rate, primary, auxiliary, expanded)` (`:86-136`):
    J, P, N, b, r, SWP, W, AMP, K, q, M, L, all from the Recommendation's rules;
  - `high`, `auxiliary_in`, `bits_in`, `precoder_scale`, `symbols_per_data_frame`
    (`:139-162`);
  - `j_and_p` (`:48`).
- **Callers:** `data.rs`, `training.rs`, V.90, tests.

### `shell.rs` (264 / 188 / 140 / 5)
- **Provides:** `Shell` (`:21-`): `new(m)`, `rings`, `combinations`, `map` (`:69`),
  `unmap` (`:124`), `ring_shares` (`:153`).
- **Callers:** `data.rs` only.

### `trellis.rs` (436 / 163 / 100 / 9)
- **Provides:** `label` (`:33`), `convert` (Table 13, `:56`), `Code` {16, 32, 64 states}
  (`:62-147`), `modulo` (`:152`), and `inversion(j, half)` (Table 12, `:159`).
- **Callers:** `data.rs`, `training.rs`, V.90, tests.

### `startup.rs` (469 / 243 / 153 / 5); state machine in §2.1
- **Provides:**
  - `Status` (`:17-28`) and `Modem` (`:32-45`);
  - constructors `new(role, fs)` (`:48`) and `with_phase2` (`:55`);
  - readings: `retrains`, `role`, `phase2`, `training`, `status` (`:84-98`), `carrier`,
    `constellation_point`/`size`/`peak`, `phase`;
  - actions: `retrain` (`:102`), `renegotiate` (`:134`), `clear_down` (`:144`),
    `decline_pcm`, `restart_phase2` (`:183`), `step` (`:191-231`);
  - data: `take_bits`, `send_bits`, `pending_bits`, `accepts_bits` (`:107-130`).
- **Private:** `settings()` (`:234-241`).
- **Callers:** `modem/src/lib.rs:101, :2440`; `v90/startup.rs:43, :64, :326, :348`;
  `v90/server.rs`; `tests/v34_capture.rs`.

### Related modules outside `v34/`
- **`v32.rs`:** `Mode` {Call: 1+x⁻¹⁸+x⁻²³, Answer: 1+x⁻⁵+x⁻²³} (`:95-118`) and `Scrambler`
  (`:347-386`). This is V.34 clause 7's scrambler as well.
- **`v8.rs`** (1300 / 712 / 21 tests): `Modem::new(role, CallFunction, Modulations, fs)`
  (`:245`), `status` (`:332`), `chosen` (`:355`), `step` (`:370`).
  - The `v8` crate already has `CallFunction::TransmitFax/ReceiveFax`
    (`crates/v8/src/lib.rs:63-65`) and `Modulation::V34HalfDuplex` (`:110`, octet 0 bit 7 at
    `:151`).
  - Nothing in the modem crate offers either.

---

## 2. The state machines

### 2.1 `startup.rs`, both roles
`startup::Modem` wraps `phase2::Modem` and `Option<training::Modem>` (`:32-45`):

```
phase2 (Running) --Done--> training = training::Modem::new(settings())      :227-229
   ^  asks_for_retrain && phase2_retrains < 2 && far INFO0 known -> phase2.again()   :216-226
   |
training.step(): take_retrain() -> phase2 = phase2.again(); training = None;
                 retraining = true                                           :194-204
status = phase2 Failed | training {Failed, Done, Connected, Retraining, ClearedDown}
         | Retraining while phase 2 re-runs | Running                         :84-98
```

`settings()` (`:234-241`) needs the far INFO0, INFO1c, INFO1a and the round trip, and those
exist only in duplex. The two roles differ only inside `phase2` and `training`.

### 2.2 `phase2.rs` stage chains (duplex, 11.2)
**Call modem:**
```
CallInfo0 (INFO0c, tone B)            --far INFO0 heard & tx drained-->            :851-857
CallFirstReversal (tone B)            --answer's reversal: reverse +40 ms-->       :792-797
CallRanging                           --answer's 2nd reversal: RTD; set L2 window--> :799-809
CallReadProbe (read answer's L2)      --window over: reading; tone B-->            :889-898
CallAwaitTone                         --answer's reversal (or 900 ms+RTD): reverse +40 ms,
                                        then L1/L2-->                              :812-816, :899-908
CallSendProbe (L1 160 ms, L2)         --tone A over L2 echo: INFO1c from reading-->  :909-946
CallInfo1                             --INFO1a heard--> Finished/Done              :771-775, :947-961
```

**Answer modem:**
```
AnswerInfo0 (INFO0a, tone A)          --far INFO0-->                               :963-967
AnswerAwaitTone                       --tone B held 20 ms, tone A 50 ms: reverse-->  :968-989
AnswerRanging                         --call's answering reversal: RTD; reverse +40 ms,
                                        then L1/L2-->                              :818-831
AnswerSendProbe                       --tone B over L2 echo: tone A 50 ms, reverse-->  :1002-1023
AnswerProbeReversal                   --call's reversal: L2 window-->              :833-839
AnswerReadProbe                       --window over: reading; tone A-->            :1025-1033
AnswerInfo1                           --INFO1c heard: settle() -> INFO1a, silence;
                                        done when drained-->                       :748-770, :1034-1045
```

**Recovery:** each deadline either falls back to an earlier stage or calls
`fail_for_a_retrain` (`:634-637`). `startup.rs` then re-runs phase 2 (`:216-226`). There is a
20 s cap (`:187`, `:667-668`).

### 2.3 `training.rs` stage chains (duplex phases 3/4 and data, 11.3-11.7)
**Answer modem:**
```
AnswerSendTraining: 70 ms silence, S, S̄, PP, TRN (1 s), J…                  :881-887, :1560-1570
AnswerAwaitS (hunt) --call's S̄: silence after J; train(PpThenTrn)-->         :1340-1354
AnswerTraining --Trained--> AnswerAwaitJ --J(size)--> S, S̄, TRN(size)         :1252-1258, :1387-1397
AnswerPhase4 --J′ + ≥512 TRN heard--> MP --> AnswerMp                           :1398-1403, :1595-1606
exchange_mp: MP′×8, E, B1 --E heard, B1 counted--> Data                        :1637-1691
```

**Call modem:**
```
CallAwaitS (hunt) --answer's S̄: train(PpThenTrn)--> CallTraining --Trained--> CallAwaitJ
CallAwaitJ --J(size)--> S, S̄, PP, TRN, J…  (CallSendTraining)                  :1375-1386, :1571-1581
CallAwaitS4 --answer's S̄: train(Trn(ask)); J′ then TRN-->                    :1356-1368
CallTraining4 --≥512 TRN sent and trained--> MP --> CallMp --> exchange_mp --> Data   :1582-1594
```

**Both roles in data:**
- `SWatch` sees S → `begin_renegotiation` → `Renegotiation` → back to Data or ClearedDown
  (`:1267-1281, :967-1002`).
- The far end's retrain tone, or an unanswered renegotiation, sets `wants_retrain`
  (`:1185-1187, :1218-1224`).

### 2.4 What half-duplex needs instead (12.2-12.6), against these parts
```
Phase 2 (12.2): INFO0 exchange; recipient reverses its tone; source answers 40 ms later
   (no second exchange, so no RTD), then 10 ms of tone, L1 160 ms, L2 until it hears the
   recipient's tone; recipient reads ≤500 ms of L2, sends its tone, then after hearing the
   source's tone keeps its own 25 ms and sends INFOh.
Phase 3 (12.3): source waits 70 ms, then S 128T, S̄ 16T, PP, TRN (size and length from INFOh);
   recipient is silent and trains.
Control channel start-up (12.4): 70 ms silence, PPh, ALT ≥16T, MPh … E; then control data.
Page (12.5): control-channel turn-off (4T of ones, 12.6.3), 70 ms silence, S, S̄, PP, B1,
   data at the MPh rate, 35 ms of scrambled ones.
Resync (12.6): 70 ms silence, Sh 24T, Sh̄ 8T, ALT, E; or PPh/MPh for a rate change;
   AC for a control-channel retrain (12.8).
```

**Mapping onto existing parts:**
- phase 2 blocks → `phase2.rs` machinery;
- phase 3 and page bursts → `qam::Transmitter`, `signals`, `receiver::Receiver`, `data`;
- control channel → new modem plus `signals`, `mp` and `v32::Scrambler`.

---

## 3. Answers

### Q1. INFO sequences: coding, carrying, and whether the codec is generic
- **Encoding:** each struct's `to_bits` lays fields out least-significant bit first with
  `put()` (`info.rs:109-113`), then `frame()` wraps them (`:133-141`): fill 1111, sync
  01110010, information, `crc(info)` LSB first, fill 1111.
- **Decoding:** `from_bits` calls `unframe(bits, LENGTH)` (`:149-159`), which checks the
  leading fill and sync and compares the CRC. The trailing fill is not required.
- **CRC:** `crc()` (`:49-51`) is Figure 14's register, generic over any bit slice. MP reuses it
  (`mp.rs:10, :138`).
- **Carrying:** `dpsk::Transmitter::send` queues bits; the modulator carries straight on into a
  tone and a following sequence (`dpsk.rs:133-142, :188-202`).
  - `dpsk::Receiver::feed` decides every symbol at eight sampling phases.
  - At each symbol it tries each length the far side can send, most recent bits first, and
    hands back the first sequence whose CRC checks (`:314-346, :350-387`).
  - The sender side decides the carrier: call 1200 Hz, answer 2400 Hz plus the guard tone
    (`:49-78`). `phase2` builds the receiver for `role.far()` (`phase2.rs:383`).
- **Generic?** Yes. `frame`, `unframe`, `crc`, `put` and `get` do not depend on the layout, as
  INFO0d and INFO1aPcm show (`info.rs:402-446, 469-500`). INFOh costs:
  - about 50 lines in `info.rs`;
  - `INFOH_BITS` (51) in both entries of `Side::lengths()` (`dpsk.rs:87-92`);
  - an explicit arm in `decide` (`:373-381`);
  - two exhaustive test matches.

  Info0 (49) and INFOh (51) cannot be confused by length. The window of 109 bits (`:300, :359`)
  is enough.

### Q2. Phase 2: tones, reversals, L1/L2, analysis, role threading
- **Tones A and B:** the DPSK carrier with nothing queued, which is a string of zeros
  (`dpsk.rs:5-9, :201`), started by `start_tone()` (`phase2.rs:718-721`).
- **Reversals:**
  - Sent by `tx.reverse()` on the exact sample (`dpsk.rs:164-169`), scheduled by
    `reverse_at`, and followed 10 ms later by silence or L1 (`phase2.rs:692-707`).
  - Detected by `dsp::ReversalDetector` (`crates/dsp/src/tone.rs:147`) at 60 Hz bandwidth.
    Detections are backdated by its latency (`phase2.rs:659-661`), and the detector restarts
    while probing is heard (`:656-658`).
- **Presence of the far tone over the echo of this end's own L2:** `Presence` compares the tone
  bin against ±150 Hz bins with `OVER_LEAKAGE` 0.05 (`:150-165, :239-293`). The duplex tests
  exercise it with echo (`:1130-1180`).
- **L1/L2:** `probe::Generator` (`probe.rs:68-89`). L1 is 160 ms at +6 dB, and L2 runs until
  stopped (`phase2.rs:702-715`).
- **Analysis:** `Analyzer` takes 20 ms windows of three repetitions each (`probe.rs:111-166`).
  - `reading()` gives per-tone gain and SNR (median of window-to-window differences, robust to
    VoIP slips) and the 1050 Hz frequency offset (`:170-244`).
  - `Reading::probed(rate, far_info0, wide)` gives the carrier with the best Shannon-gap bits
    (only carriers the far transmitter's INFO0 allows), pre-emphasis 0-5 from the tilt (the
    6-10 shelves are never chosen, `:351-364`), and `max_rate` capped by Table 8 and by 12
    without 1664-point support (`:372-393`).
  - The symbol rate is chosen in `settle()` (`phase2.rs:1081-1119`): best `max_rate`, lower
    symbol rate on ties, within the asymmetry limit.
  - Power reduction and MD are never computed; they are always 0 (`:927-929, :1111-1113`).
- **Can a recipient fill INFOh from it?** Yes.
  - Run `Analyzer` over the source's L2, then `reading.probed(r, &source_info0, wide)` for each
    `SymbolRate::ALL`, and pick with `settle`'s `best` rule (ignore asymmetry).
  - That gives INFOh bits 22 (high carrier), 23:26 (pre-emphasis) and 27:29 (symbol rate).
  - Bits 12:14 (power reduction, 0 as today), 15:21 (TRN length) and 30 (16-point TRN) are new
    policy.
  - `probed` already restricts to what the source can transmit (`transmits`, `:299-308`). Also
    AND in this end's own receive capabilities, as `settle` does with `wide`.
- **Role threading:** only `Role::{Call, Answer}` exists (`:34-53`). It picks the transmit and
  receive carriers (`:375, :383-385`) and the start stage (`:412-415`). `Pcm` remaps it for
  V.90 (`:74-81`).
  - "Both ends probe each other" is built into the stage chains in §2.2 (`CallReadProbe` →
    `CallSendProbe`, `AnswerSendProbe` → `AnswerReadProbe`), the two ranging stages, and the
    INFO1c/INFO1a generation (`:916-941, :748-770`).
  - Half-duplex has no ranging (12.2.1.1.3: one reversal pair, then L1/L2 straight away). Only
    the source probes, and INFOh replaces both INFO1 sequences.
  - So half-duplex needs a source/recipient axis and four new stage chains. The rest of the
    struct (`blank`, `timers`, `speak`, `Presence`, INFO0 handling in `heard`) carries over. It
    fits either as a mode on `phase2::Modem` or as a sibling module, but `Presence`, `Stage`,
    `Speaking`, `timers` and `speak` are private today.
  - Half-duplex timeouts are fixed times rather than round trips: 2700 ms and 2000 ms
    (12.2.1.3.3-4, 12.2.2.4.3-4).

### Q3. Phase 3: generation, training, echo canceller, state across bursts
- **Generation:**
  - Symbols come from `signals::{s, s_bar, pp}` and `Sender::trn` (`signals.rs:76-96, :128-140`).
  - `training.rs`'s private `Source::next` sequences them: silence with a hold, S, S̄, then
    PP or TRN; PP is followed by TRN with the scrambler reset (`training.rs:408-455, :379-392`).
  - It hard-codes 4-point TRN after PP (`:453`) and has no segment for "after PP go to B1".
  - `qam::Transmitter::next_sample` pulls one symbol at a time (`qam.rs:248-270`).
- **Training:**
  1. `Receiver::hunt()` finds S by two-symbol self-correlation, learns a template, and reports
     `Reversal{at}` at the start of S̄ ±1 half-symbol (`receiver.rs:276-364, :693-699`).
  2. `train(Reference, far_mode, at)` puts the sequence 16 symbols after S̄ (`:525-533`).
     Once enough half-samples have arrived, `solve_known` runs:
     - a least-squares 31-tap T/2 equaliser at each of ±8 half-symbol alignments;
     - an estimate of the carrier's turn per symbol;
     - a final solve over the window (`:786-845`).
  3. If nothing fits, a second, wide ±200 search runs further into TRN, for slips
     (`:749-755, :1217-1225`).
  4. Tracking uses NLMS taps, a second-order carrier loop and a timing loop on the output's
     slope (`:909-991`).
  5. Losses are rewound and resynced (`:997-1211`).
- **Echo canceller or duplex timing?** Neither.
  - The receiver takes raw line samples and nothing else.
  - The only coupling is `far_mode`, which chooses which scrambler generates the TRN targets
    (`:1228-1238`).
  - All deadlines live in `training.rs`.
  - A recipient can train with no canceller, and in half-duplex it is silent while the source
    sends phase 3 (12.3.2.1).
  - Precedent: the Conexant recording's answer-modem phase 3 (S, S̄, PP, TRN, J) trains this
    receiver to over 15 dB while the call modem is quiet (`tests/v34_vector.rs:179-196`).
- **Where the state lives:** all of it is in `Receiver` (`receiver.rs:375-437`):
  - fixed mixer phase (`:377-379`);
  - timing: `due`, `half`, `drift`, `timed` (`:386-390, :435`);
  - equaliser `taps` (`:403`);
  - carrier: `rotation`, `turn` (`:410-411`);
  - `slope`, `error`, `settled`;
  - 24 loop snapshots (`earlier`, `:436`).
- **Keeping it across bursts:**
  - `idle()` only changes the mode (`:536-538`). `feed()` keeps interpolating half-samples,
    advancing timing by the last drift estimate (`:653-657`).
  - `resume(first)` restarts tracking with the old taps and advances the carrier by turn ×
    symbols gone (`:548-559`). V.90's digital modem uses it to read CPt after S̄ with phase 3's
    taps (`v90/digital.rs:951-957`).
  - `train()` always re-solves from scratch. The only exception is the `Trn` fallback
    `reacquire`, which uses the previous taps with any alignment or quarter turn
    (`receiver.rs:746, :857-906`).
  - For 12.5's S, S̄, PP, B1 after seconds of control channel, carrier phase and ±½-symbol
    timing will have wandered. Two routes:
    - **Robust:** a new `Reference::Pp`. Align and solve over PP symbols 48..288, which is
      today's first-try alignment window (`:1220`). Skip the second try (no TRN follows).
      `next_symbol` then lands on B1 (`:764`). This is about 30 lines in `windows`,
      `sequence`, `train` and `finish_training`.
    - **Cheaper, riskier:** `resume(at + 2·(16+288))` with a hold during PP. There is no
      `hold()` in the V.34 receiver; the dsp Core has one (`qam/mod.rs:583-585`).
  - 16-point TRN after PP (INFOh bit 30) needs `PpThenTrn` to take a `Size` (`:220-225,
    :528-531, :1233-1234`), and the transmit side needs `training.rs:453`'s equivalent.

### Q4. MP, MPh, and the E and scrambled-ones detection
- **MP encoding:** 17 ones, start bit, type (`mp.rs:98-100`); fields (`:102-119`); start bits at
  17/34/51/68 (Type 1 also 85…170); precoding (`:121-135`); `crc()` over bits 18 to the CRC
  minus the start bits (`:67-87, :137-139`); fill 000 or 0 (`:140`).
- **MP decoding:** `from_bits` checks the sync, the start bits and the CRC (`:148-181`).
- **Frame sync:** `Finder::feed` keeps a rolling buffer of descrambled bits. At each bit it tests
  both lengths ending now, with the fill not awaited, requiring exactly 17 ones (the bit before
  must not be 1) and the type bit to match the length (`:209-243`).
- **How close MPh is** (Tables 23/24, checked on the rendered pages):
  - Identical: lengths 88/188, sync, start bit positions, CRC coverage and position (69/171),
    fill, trellis (29:30), non-linear (31), shaping (32), rate mask (35:49), and Type 1
    precoding positions.
  - Different:
    - 20:23 is the maximum primary data rate (one direction);
    - 24:26 are reserved and 27 is the control-channel rate for the remote transmitter
      (0 = 1200, 1 = 2400), where MP has 24:27 as answer-to-call;
    - 28 is reserved (MP: auxiliary channel);
    - 33 is reserved (MP: acknowledge), so there is no MP′;
    - 50 enables asymmetric control-channel rates.
  - An `Mph` struct can share `covered`/`start_bits`/`crc_at` (make them pub(crate) or put MPh
    in `mp.rs`), about 60-80 lines. `Finder` needs a parse hook, or a pure MPh can be parsed as
    `Mp` and reinterpreted.
  - The exchange logic differs:
    - MPh ends with a single E once one MPh is received (12.4.1.3/12.4.2.4), not MP′×8
      (`training.rs:1637-1691`);
    - the rate is "maximum enabled ≤ both modems' MPh rates" (12.4.1.3), which is
      `negotiate` (`training.rs:1726-1739`) restricted to one direction.
- **E detector:** yes. `Finder` returns `Found::E` on 20 consecutive descrambled ones
  (`mp.rs:210, :217-220`). `training.rs` accepts it only after an MP has been read, because
  TRN's tail is also ones (`:603-607`).
- **Scrambled-ones detection elsewhere:**
  - TRN is recognised as "descrambles to all ones" through `Reader::trn` (`training.rs:579-592`).
  - B1 is counted as `framing.n` decoded bits after E, with non-ones counted in `b1_errors`
    (`:1427, :1033-1037`).
  - Half-duplex B1 follows PP directly with no E, so the decoder starts at the symbol after
    PP (Q3).
  - The control channel's E after ALT is found by `Finder` unchanged: descrambled ALT is
    0101…, which never holds 20 ones.

### Q5. Data mode one way, B1, the superframe and the auxiliary channel, start and stop
- **Encoder API:** `Encoder::new(Params)` resets scrambler, trellis, differential encoder and
  precoder to zero "from the start of B1" (`data.rs:281-299`). `next_symbol(&mut || bit)` gives
  unit-power 2D symbols and pulls a mapping frame's bits when one starts (`:310-359`).
- **Decoder API:** `Decoder::new(Params)` (`:510-538`). Call `feed(unit_symbol)` (`:591`), then
  `take_bits()`. `path_cost()` exposes decoder health (`:576`). `resume(params, m)` and
  `Acquirer` restart after a slip (`:545-552, :773-927`). The receiver must be told the grid:
  `rx.set_grid(decoder.grid_scale(), decoder.extent())`, as at `training.rs:1426`.
- **One way from MPh parameters?** Yes. The objects are single-direction.
  - `Params` {framing = `Framing::new(symbol_rate, bps, false, expanded)`, code, nonlinear,
    precoding [h1..h3], mode = the transmitter's scrambler} (`:44-52`).
  - `training.rs:1438-1464` shows the mapping from MP. V.90 already runs `Encoder` alone
    upstream and `Decoder` alone (`v90/analogue.rs:3006`; `v90/digital.rs:1095-1096`).
  - The precoder and non-linear encoding are implemented and tested both ways
    (`data.rs:128-162, :1158-1164, :1234-1268`).
  - Computing precoding coefficients from this end's equaliser is **not found**:
    `make_mp` always sends Type 0 (`training.rs:1527-1528`). A recipient can ask for no
    precoding.
- **B1:**
  - `data.rs` does not know B1 exists; `inversion_at` counts the superframe from B1 as the last
    data frame (`:164-173`).
  - B1 is produced by the caller: `training.rs::Source` feeds ones for the first P mapping frames
    (`:504-510`).
  - The receive side skips `framing.n` bits after E (`:1427, :1033-1037`).
  - The same rule serves half-duplex B1 (10.1.3.1).
- **Auxiliary channel:** `Framing` computes W/AMP (`frame.rs:36-38, :143-146`), but
  `Encoder`/`Decoder` never multiplex auxiliary bits (`auxiliary_in` has no caller), and
  `training.rs` always passes `false` (`:1442, :1458`). Half-duplex has no auxiliary channel, so
  this matches.
- **Start and stop:**
  - Start is clean: a new `Encoder` and `Decoder` at B1.
  - Stop has no API. Turn-off is "keep feeding ones": idle data is ones (`training.rs:510`), and
    the encoder scrambles its input (`data.rs:324`). Feed ones for 35 ms, then zero symbols
    until the 20-symbol pulse lookahead has flushed (`qam.rs:243`; `training.rs:1683`).
  - The recipient has no 6.6.2 turn-off/turn-on detector. It only has `rx.level()` (a slow
    power average) and `rx.is_lost()`; `training::Modem::carrier()` is `level() > 1e-4`
    (`training.rs:1497-1500`). A page-end detector is new work.

### Q6. Scrambler, differential encoding, and the control-channel base
- **Scrambler:** `v32::Scrambler` with `Mode::{Call, Answer}` (`v32.rs:95-118, :347-386`) is
  clause 7's GPC/GPA, as in `signals.rs:112` and `data.rs:289, :518`.
- **Differential encoding:**
  - Z for J, MP, E and control-channel style symbols: `signals.rs:145-150, :235-245`.
  - Data mode's I2/I3 Z (9.5): `data.rs:331`.
  - 10.2.4's rule is exactly `Sender::differential`: `bits[0]`=I1, `bits[1]`=I2, label =
    Q1+2·Q2, clockwise by Z. "If differential encoding is not enabled, Zₙ = 2·I2+I1" is the
    `trn()` path (`signals.rs:128-140`).
- **Can the control channel be built from existing parts?** Bits, mapping, scrambling, E and
  MPh framing: yes. PPh, AC, ALT, the 600-baud modem and the procedures: new.

**The three candidates for the 600-baud FDM modem:**

| | `v34/dpsk.rs` | `v22bis.rs` (+`v22bis/handshake.rs`) | `dsp::qam::Core` |
|---|---|---|---|
| Baud, carriers | 600; 1200/2400 (`:23, :53`) | 600; 1200/2400 (`:20-24`) | any (`Band::new`, `qam/mod.rs:112-132`) |
| Guard tone, levels | 1800 Hz −7 dB, answer carrier −1 dB (`:40, :66-78`), 10.2.4's own | **none** (no "guard" or 1800 in the file) | n/a (receiver only) |
| Pulse | RRC 0.75, Figure 13 tested (`:25-33, :549-603`), the same mask 10.2.4 cites | RRC 0.75 (`:25`) | no matched filter; the T/2 equaliser does it |
| Symbols | binary DPSK only (`:105-108, :216-227`) | V.22bis 16 points (1,1)…(3,3), counterclockwise, Table 1 quadrant code (`:88-100, :334-376`): **not** V.34's labels or rotation | any table (`Slicer::table`) |
| Scrambler | none | 1+x⁻¹⁴+x⁻¹⁷ (`:152-215`), not clause 7 | the driver's (`v32::Scrambler`) |
| Timing, carrier | none: 8 parallel phases, CRC-checked bursts only | Gardner gain 0.1 (`:587`) and a PLL; documented pull-in of ±2.2 Hz against the ±7 Hz required (`docs/design/slow-modes/v22-and-bell.md:8-12`) | T/2 31-tap equaliser, carrier and timing loops, AGC, S-derived frequency and drift |
| Training | none | blind CMA to DD, 21 taps (`:599`) | least squares on any known sequence (`Window`/`Training`, `qam/train.rs:48-101`; `Core::train` at `:166`); PPh+ALT can be the targets |
| FDM selectivity | 401-tap 560 Hz complex LPF (`:291`); also rejects the own guard 600 Hz off | 401-tap 600 Hz (`:577`), 67 dB measured (`:553-571`); the guard would sit on its edge | 64-tap Kaiser only, cutoff 0.5·baud·1.1+300 = 630 Hz (`qam/front.rs:18-21, :69-88`) |
| Slips, loss | n/a | none | loss, rewind, two resyncs (`qam/resync.rs`); used by V.17, V.29, V.27ter, V.32 |
| Sh/Sh̄, AC hunt | no | only V.22bis handshake patterns (`:442-470, :866-885`) | S→S̄ hunt with discriminator (`qam/hunt.rs`) |

**Recommendation:**
- **Transmitter:** make `dpsk::Transmitter`'s modulator complex, or write a sibling that
  copies its clock, pulse, carriers, levels and guard tone. Feed it from `signals::Sender`,
  plus new PPh/AC/ALT generators. About 150-250 lines.
- **Receiver:** `dsp::qam::Core` behind a sharp channel-select pre-filter. It is pull-style
  (`next()`/`settle()`, `qam/track.rs:172, :226`), so the driver does the 4/16-point decisions,
  `signals::Reader`-style differential decoding and descrambling, and `mp::Finder` for MPh and
  E.
- **Why not the Core's own filter:** at 600 baud its filter is still only −15 dB at 845 Hz and
  −31 dB at about 1045 Hz from the carrier. That is scaled from the response measured for the
  same 64-tap design in `v34-reference.md:44-52`. The other direction occupies 675-1725 Hz at
  baseband, and the answer modem's own guard tone sits at +600 Hz, inside the passband.
- **The pre-filter:** a real band-pass FIR ahead of `Core::feed`, which mixes internally, built
  like `dpsk.rs:291` or `v22bis.rs:577` (`dsp::fir_lowpass`, `shaping.rs:64`, modulated to the
  far carrier).
- **Why not V.22bis:** its constellation labels, rotation sense, differential code, scrambler
  and handshake are all V.22bis's. It has no guard tone. Its receiver is the pre-rebuild design
  (blind equaliser, a weak carrier loop, no slip handling) that V.17, V.29, V.27ter and V.32
  were moved off. Its one transferable asset, the FDM select filter, `dpsk.rs` already has.
- **Caveats for the Core:**
  - PPh is only 32 symbols (period 8; formula 10-2, not in code). Least squares with 31 taps
    needs ALT's known symbols as well, or a smaller solve with the ridge; `REACH` is a const
    (`qam/mod.rs:94`).
  - The Core's hunt only knows S→S̄. Detecting PPh and classifying tone, AC, PPh and Sh is new.

### Q7. The echo canceller
- **Where:** `dsp::EchoCanceller` and `EchoFinder` (`crates/dsp/src/echo.rs:631`; 1624 lines,
  13 tests; `dsp/tests/echo_drift.rs`).
- **Who uses it:** only `v32/startup.rs` (`:18, :2260, :2366`) and the `v32_loopback` test.
- **How much V.34 relies on it:** not at all.
  - Phase 2 survives the echo of its own L2 with `Presence`'s leakage test and restarts of the
    reversal detector (`phase2.rs:150-165, :651-658`), and separates directions by FDM
    (`dpsk.rs:11-14, :286-292`).
  - Phases 3 and 4 and data run both directions in one band with no canceller: the answer
    modem's J overlaps the call modem's S (`training.rs:9-12`). That works on a 4-wire VoIP
    path; the test link has no echo (`training.rs:1826-1844`).
- **For half-duplex:** no canceller is needed. The primary channel is one way, the control
  channel is FDM, and the phase 2 behaviour is shared with duplex.

### Q8. How the modem crate drives V.34, and how fax is driven
- **The V.34 path:**
  - `Pump::V34(Box<v34::startup::Modem>)` (`lib.rs:91-109`) is built in `start_pump` when V.8
    agrees `Modulation::V34Duplex` (`:2402-2441`, V.34 at `:2435-2441`).
  - `Pump` delegates `step`, `carrier`, `take_bits`/`send_bits`, `accepts_bits` (false during a
    retrain, `:234-241`), `pending_bits`, and the scope data (`constellation_point`, `states`,
    `shape`, `standard`, `phase`: `:258-429`).
  - `status()` maps Running → Negotiating; Connected{receive, transmit} → Connected (separate
    rates each way); Retraining → Retraining; Done, ClearedDown and Failed → Failed
    (`:147-158`).
  - `round_trip_ms` comes from phase 2 (`:182`).
  - Reports: `V34Report` and `V34Training` (`:449-728`) are built from
    `startup.phase2()`/`training()`.
  - `advance_handshake` turns Connected into CONNECT and V.42 detection, with rate updates
    (`:1640-1700`).
  - `watch_for_retrain` follows Retraining → Connected and changes the rate (`:1895-1918`).
  - `ask_for_retrain` → `renegotiate` two steps down (`:1942-1948`); `retrain()` → the full
    11.5 retrain (`:2004-2008`); `retrains()` counts renegotiations plus retrains (`:2027`).
- **The fax path:**
  - `Modem.fax: Option<FaxCall>` (`:830-835`) is created in `place_call` when
    `+FCLASS=1`. It skips V.8 and the pump (`:2182-2211`).
  - Each sample goes to `carry_fax` (`:1571-1573, :2330-2349`).
  - `FaxCall` (`faxcall.rs:59-76`) owns a V.21 control Sender/Receiver, a Transmitter and
    Receiver for each of V.27ter, V.29 and V.17, CNG and CED.
  - `step()` (`:400-407`) asks `fax::call::Call::line()` what should be on the line
    (`fax/src/call.rs:52-67, :628`), then:
    - `follow()` starts, stops or restarts pumps on a change (`:410-473`), including V.17's
      long train against resync with taps kept (`:443-450, :462-467`);
    - `listen()` feeds the control receiver in Quiet, Listen, CallingTone and FastListen, and
      the page receiver only in FastListen (`:482-524`);
    - `talk()` pulls bits from `call.next_control_bit()`/`next_fast_bit()` into the
      transmitter and reports idle (`:527-595`).
  - `Carrier::of(Speed)` maps `fax::t30::Modulation` {V27ter, V29, V17} and a rate to a pump
    (`faxcall.rs:25-46`; `fax/src/t30.rs:233-240`).
- **Where a V.34 half-duplex pump fits:**
  - It cannot be a fourth `Carrier`. The call starts with V.8, which fax skips today. The
    control channel is V.34's full-duplex 600-baud QAM rather than V.21, and it must be
    received while transmitting, which `listen()` never does for `Line::Control`.
  - There is no TCF or DCS rate ladder; the rate comes from MPh. The page channel's start-up
    and resyncs happen below T.30.
  - Shape it as a V.34 fax object that `FaxCall` drives on a separate branch: control bits
    in and out while the control channel is up, page bits out or in during primary bursts, and
    events (control up, primary ready or ended, rate, retrain).
  - `fax::call` will need an Annex F mode. The scope getters (`faxcall.rs:278-397`) extend
    naturally.

### Q9. Tests
- **Unit tests:** 112 across `v34/*.rs` (per-file counts in Q11). The harnesses worth reusing:
  - `phase2.rs` test `Line`: delay, loss, echo, noise (`:1130-1180`); `run` for any two ends
    (`:1166-1180`); `time_to_spare` checks the L2-read deadline (`:1285-1338`). Use these for
    half-duplex phase 2 against itself, including echo over L2.
  - `startup.rs` `run` (`:253-282`) for a whole start-up.
  - `training.rs` `Link`: delay, loss, noise, a ppm clock offset through two resamplers, VoIP
    slips (`:1773-1888`). Use it for the primary burst and control-channel loopbacks.
  - `receiver.rs` `phase3()`, `line()` (resample by ppm, loss, noise), `slip()` and
    `phase4_with_mps` (`:1315-1355, :1547-1587`). Use them for recipient training on S, S̄,
    PP, TRN(16) and for a PP-only resync after a gap.
  - `data.rs` `loopback()` and `signal()` (`:983-1013, :1065-1095`) for every Params
    combination.
  - `dpsk.rs` `through()` (`:427-457`) and the duplex-both-ways test (`:505-533`) for INFOh on
    either side.
  - `signals.rs:292-340` for Sender/Reader round trips, which carry over to the control
    channel.
- **Integration tests:**
  - `datapump/tests/v34_vector.rs` (211 lines): INFO sequences read off
    `tests/vectors/v34-33600.wav` with CRCs checked, and the real answer modem's phase 3
    training this receiver (`:120-196`).
  - `datapump/tests/v34_capture.rs` (473 lines): three ignored tests over live captures set by
    environment variables.
  - `modem/tests/call.rs`: V.34 connect at 33600 (`:394`), a full retrain (`:487`), fallback to
    V.32bis (`:534`).
  - `modem/tests/v17_fax.rs`, `v27ter_fax.rs`, and `faxcall.rs`'s `between`/`through`
    (`:623-661`) for whole fax calls.
- **Vectors:** `tests/vectors/v34-33600.wav` is a duplex data call (README: V.8 and line
  probing). No half-duplex or V.34 fax recording was found in `tests/vectors`, and nothing in
  `captures/` is indexed as one: the frame logs sampled there are V.42 XID.
  - The duplex vector still exercises INFO0, tones A/B, DPSK and phase 3 training.
  - Half-duplex will need loopback tests only, until a real Super G3 capture exists.

### Q10. What is too duplex for a flag
- **`training.rs` as a whole:**
  - the stage chains (§2.3);
  - the J/J′ constellation request;
  - primary-rate MP/MP′/E with `MP_PRIME_REPEATS`;
  - `Settings` from INFO1c+INFO1a with both a transmit and a receive `Band` (`:72-126`);
  - tx and rx every sample (`:1177-1211`);
  - renegotiation by `SWatch` in data (`:1267-1281`);
  - round-trip-based deadlines (`:879, :1257, :1366, :1395, :999`).

  This needs a new primary-channel module. Reuse `Source`'s pattern: make a pub(crate) copy
  with segments Silence, S, S̄, PP, then TRN(size, length) or B1/Data, then ones and silence.
- **`startup.rs`:** `settings()` needs INFO1c and INFO1a (`:234-241`), and the status wants
  both-direction rates. It needs a new half-duplex top level: phase 2 HDX → phase 3 → control
  start-up → (page ↔ control resync)*, plus 12.7 and 12.8 retrains.
- **`phase2.rs` stage chains:** extend with a mode and new stages, or write a sibling module on
  the same private blocks (Q2). Borderline between a flag and new code.
- **The control channel:** entirely new. Modem, PPh, AC, ALT, detectors, 12.4, 12.6 and 12.8.
- **The modem crate's fax join:** new branch (Q8).
- **Only parameters or small splits:** `dpsk` lengths, `info` (INFOh), `mp` (MPh),
  `receiver::Reference`, TRN size after PP.

### Q11. Sizes (lines)

| file | total | non-test | code only | unit tests |
|---|---:|---:|---:|---:|
| v34.rs | 36 | 36 | ~15 | 0 |
| constellation.rs | 115 | 53 | 26 | 4 |
| info.rs | 688 | 515 | 342 | 9 |
| dpsk.rs | 619 | 388 | 230 | 7 |
| probe.rs | 607 | 395 | 266 | 9 |
| phase2.rs | 1511 | 1121 | 727 | 14 |
| signals.rs | 349 | 247 | 164 | 5 |
| qam.rs | 411 | 272 | 187 | 5 |
| receiver.rs | 1676 | 1304 | 913 | 7 |
| training.rs | 2309 | 1740 | 1235 | 15 |
| mp.rs | 309 | 245 | 182 | 4 |
| data.rs | 1269 | 965 | 729 | 10 |
| frame.rs | 276 | 164 | 115 | 4 |
| shell.rs | 264 | 188 | 140 | 5 |
| trellis.rs | 436 | 163 | 100 | 9 |
| startup.rs | 469 | 243 | 153 | 5 |
| **V.34 total** | **11,344** | **8,039** | **~5,520** | **112** |

About 40% of each non-test file is doc comment.

**Related modules:**
- `datapump/src/v8.rs`: 1300 lines (712 non-test).
- `v22bis.rs`: 1337 (1059 non-test); `v22bis/handshake.rs`: 433.
- `dsp/src/qam/*`: 3083 (mod 740, front 240, hunt 275, train 382, track 442, resync 414,
  slicer 449, blind 141), with 1049 lines of tests in `dsp/tests/qam_core.rs`.
- `dsp/src/echo.rs`: 1624.
- `modem/src/lib.rs`: 2604; `faxcall.rs`: 1249 (598 non-test).
- `fax/src/call.rs`: 2743.
- V.17 for comparison: transmitter 321, receiver 783, `v17.rs` 460, all non-test.

---

## 4. Seams for work packages

The sizes are my rough estimates of new or changed lines, scaled from the comparable existing
code above; they are not measured.

| WP | What | Reuse as is | Change or split | New | Est. code + tests |
|---|---|---|---|---|---|
| A | INFOh | `info::{frame, unframe, crc, SymbolRate}` | `dpsk::Side::lengths`, `decide` arm; two test matches | `InfoH` struct | ~60 + 60 |
| B | MPh | `mp` CRC, start-bit and coverage helpers, `Finder`'s E rule | make the helpers pub(crate); give `Finder` a parse hook | `Mph` struct; one-direction `negotiate` | ~80 + 60 |
| C | Receiver references | `Receiver` whole | `PpThenTrn(Size)`; new `Reference::Pp` with no retry | none | ~40 + 100 |
| D | HDX phase 2 | dpsk tx/rx, `Presence`, reversal and timer machinery, probe `Generator`/`Analyzer`/`probed` | mode or sibling module; private blocks made pub(crate) | 4 stage chains (call/answer × source/recipient); INFOh fill policy (rate, carrier, pre-emphasis, TRN length and size); fixed 2000/2700 ms timeouts | ~350-500 + 300 |
| E | Primary channel | `qam::Transmitter`, `signals`, `receiver`, `data::{Encoder, Decoder, Acquirer}`, `frame`, `trellis`, `shell`, `RetrainWatch` | a `Source` template taken from `training.rs` | phase 3 (source and recipient), 12.5 bursts with B1, 35 ms turn-off, page-end carrier detector (6.6.2), MPh → Params | ~500-700 + 400 |
| F | Control-channel modem | `signals::{Sender, Reader, s, s_bar}`, `v32::Scrambler`, `mp::Finder`, `dsp::qam::Core`, `dsp::fir_lowpass` | `dpsk` modulator made complex (or copied) | FDM pre-filter; PPh, AC, ALT generators; PPh/AC/Sh/tone detection; 4/16-point driver | ~700-1000 + 500 |
| G | Control procedures and HDX top level | `SWatch`-style Sh/Sh̄ watch on equalised symbols; `RetrainWatch` | none | 12.4 start-up, 12.6 resync, 12.8 control retrain, 12.7 primary retrain, 3 s timers, status and rates API | ~600-900 + 400 |
| H | Modem and fax join (outside this map) | `v8` codec (TransmitFax/ReceiveFax, V34HalfDuplex), `datapump::v8::Modem` | the fax path in `place_call` must run V.8 | a V.34 branch in `FaxCall`; Annex F in `fax::call` | not estimated |

**Suggested order:** A, B and C are independent and small. D and F can proceed in parallel.
E depends on C. G depends on B, E and F. H sits on top of G.
