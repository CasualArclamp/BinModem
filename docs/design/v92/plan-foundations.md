# V.92 implementation plan: foundations first

This plan breaks V.92 (T-REC-V.92-200011, "Enhancements to Recommendation V.90") into work packages (WPs)
for BinModem. Each WP is sized for one engineer-agent in one sitting and ends with passing tests. The plan
is ordered "foundations first":

1. shared data structures (every new frame and sequence codec, with its CRC, unit-tested against the digests);
2. the PCM-upstream transmitter and receiver DSP;
3. Phases 3 and 4;
4. short Phase 1 and short Phase 2;
5. rate renegotiation and fast parameter exchange;
6. modem-on-hold;
7. AT commands and the GUI.

Both ends of every feature are ours: the analogue modem (client) and the digital modem (server). So every feature is
proved call-to-call over the simulated network before it is offered to a live line. V.90 and V.34 keep working at
every step. V.92 falls back to V.90 and V.34 exactly where the Recommendation says it must.

The plan paraphrases the Recommendation and cites clause numbers. Exact bit layouts, field widths, timings, CRC
vectors and constants are **not** repeated here. They live in the digests listed under each WP, which were written
from rendered PDF pages. Where a digest and this plan differ, the digest wins. Where a digest itself is in doubt,
render the PDF page (see section 4).

---

## 1. Sources

All digests are in `F:\dialupmodem2\docs\design\v92\`. Short names used below:

| Short name | File | Covers |
|---|---|---|
| INTRO | `spec-intro-transmitter.md` | clauses 1-7: scope, Qa.b, digital modem = V.90 clause 5, PCM-upstream transmitter (6.1-6.4), circuits |
| P1A | `spec-phase1-signals-analogue.md` | 8.1, 8.2: QC1a, QC2a, QCA1a, QCA2a, TONEq; V.8/V.8 bis/V.21 framing |
| P1D | `spec-phase1-signals-digital.md` | 8.3: ANSpcm (Tables 6-10, Appendix A octets), QC1d, QC2d, QCA1d, QCA2d, QTS |
| P1P | `spec-phase1-procedures.md` | 9.1, 9.2 (Figures 3-8), state machines, 28 timers |
| P2S | `spec-phase2-signals.md` | 8.4, 8.4.1, Tables 15-19, INFO CRC vectors, layout classification |
| P2P | `spec-phase2-procedures.md` | 9.3, 9.3.1, 9.4 (Figure 9), V.90 9.2 restated, retrain entry points |
| P3S | `spec-phase3-signals.md` | 8.5, 8.6, Tables 20-23, scramblers, sign conventions, Jd/Jp/CPt vectors |
| P3P | `spec-phase3-procedures.md` | 9.5 (Figures 10, 11), timers TR1-TR6, handshake table |
| P4A | `spec-phase4-signals-analogue.md` | 8.7: B1u, E2u, CPu, CPus, RM, SUVu, TRN2u (Tables 23-29), FB1u |
| P4D | `spec-phase4-signals-digital.md` | 8.8: B1d, Ed, CPd (Table 30), Rd/Rt/Rf, SUVd (Table 31), TRN2d, vectors |
| P4P | `spec-phase4-procedures.md` | 9.6 (Figures 12-14), 9.7, the SUV/CP/E state machine |
| RRF | `spec-renegotiation-fpe.md` | 9.8 (Figures 15-18), 9.9 (Figure 19), 9.11, detectors, errata E-1..E-11 |
| MOH | `spec-modem-on-hold.md` | 8.9 (Tables 32, 33), 9.10 (Table 34, Figures 20-24), 9.11, clause 10, MH vectors |
| CTL | `spec-control-plane.md` | V.8, V.8 bis, V.250 6.8 (`+P` commands), `+MS`, V.42/V.44 ties |
| CA | `code-analogue.md` | the V.90 analogue modem as built, extension points, house style |
| CD | `code-digital.md` | the V.90 digital modem, `sequences.rs`, `network.rs`, what a V.92 receiver needs |
| CV | `code-v8-and-call.md` | V.8, modem crate, AT, GUI, telemetry, hold and quick-connect integration |
| CT | `code-tests.md` | harnesses, network gaps, proposed V.92 tests, runtime budget, live constraints |

Also read `docs/design/architecture.md` (the streaming rule: no block APIs in `dsp` or `datapump`).

**Provenance.** This plan was written from those eighteen digests and then re-checked against all of them, clause by
clause: the frame layouts and CRC vectors of WP-02, WP-03 and WP-09; the segment lengths, timers and watchdogs of
WP-21, WP-22, WP-26, WP-27, WP-34, WP-35, WP-43, WP-45 and WP-47; the padding arithmetic of section 5's CP and SUV
tests; and the wave-by-wave file lists in section 6, which are disjoint within every wave. No PDF page was rendered
for this pass, because nothing in the digests was in doubt. Where a WP and a digest disagree, the digest wins.

---

## 2. Overview

### 2.1 What V.92 adds to what the repo has

The repo already has a complete V.90 pair: `datapump::v90::{analogue, digital, startup, server, network, ...}`.
V.92's downstream is V.90 clause 5 unchanged (INTRO section 5), so the downstream encoder, sign coding, DIL design and
analysis, and the downstream PCM receiver are all reused. What is new:

| Area | New in V.92 | Clauses |
|---|---|---|
| Upstream | PCM at 8000 symbols/s, 24 000-48 000 bit/s. 12-symbol data frames; 12-modulus encoder with a differential sign step; precoder and prefilter designed by the digital modem; 4D trellis on PCM levels | 1, 6.1-6.4 |
| Phase 1 | short Phase 1: QC/QCA in V.8 or V.8 bis framing, QTS/QTS-bar, ANSpcm, TONEq; LAPM agreed through the P bit | 8.2, 8.3, 9.2 |
| Phase 2 | INFO0 V.92 and short-Phase-2 bits; INFO1d bit 70; two new INFO1a layouts (Tables 18, 19); short Phase 2 (ranging only) | 8.4, 9.3, 9.4 |
| Phase 3 | a new upstream half: Ru, MD, TRN1u, Ja with a 24-ones preamble and an upstream rate mask, Su/Su-bar with a fractional extension, Jp/Jp', CPt, E1u; the digital modem's SCR | 8.5, 8.6, 9.5 |
| Phase 4 | TRN2u (4 or 8 levels), the SUV/CP/E acknowledge exchange, CPd (precoder, prefilter, gain, 12 moduli, constellations), E2u (optional 13th symbol), B1u | 8.7, 8.8, 9.6 |
| Rate renegotiation | Ru/Rd, TRN2 both ways, an optional silent period ended by Rt, CPus | 9.8 |
| Fast parameter exchange | RM/RM' through the data-mode chain, Rf/Rf-bar, encoder resets, FB1u | 9.9 |
| Cleardown | drn = 0 in a CP sequence, reached through RR or FPE | 9.11 |
| Modem-on-hold | RT tones and 40-bit MH sequences on the INFO modem; hold, refuse, cleardown, fast reconnect | 8.9, 9.10 |
| DTE | V.250 `+PCW +PMH +PMHT +PMHR +PIG +PMHF +PQC +PSS`, `+MS=V92` | V.250 6.8, 6.4.1 |

### 2.2 Shape of the plan

57 WPs in 19 waves. WPs in the same wave touch disjoint files, so they can run in parallel worktrees and merge cleanly.
The early waves are wide (7, 10, 7, 5 and 4 WPs). The later waves are narrow, because the state machines and the
modem crate are shared files.

| Milestone | Reached after | What works |
|---|---|---|
| M1: codecs and blocks | wave 3 | every V.92 frame and sequence has an encoder, a decoder and a CRC test; the upstream chain, receiver, ε estimator, sources, short Phase 2 and the hold transaction exist and pass unit tests |
| M2: Phase 3 call-to-call | wave 5 | our analogue and digital V.92 modems complete Phase 3 over `Network` |
| M3: PCM both ways | wave 7 | phases 3 and 4 connect with PCM both ways; data crosses |
| M4: a whole V.92 start-up | wave 8 | from the end of V.8, through V.92 Phase 2, with V.90 interop and the fallback ladder |
| M5: short phases, robustness | waves 9-11 | short Phase 2 with a memo; echo canceller; slips; short Phase 1 on the line and in the pumps; RR |
| M6: modem crate | waves 12-14 | `+MS=V92` from AT to CONNECT; quick connect with the ODP/ADP bypass; FPE; cleardown |
| M7: hold, AT, GUI | waves 15-19 | modem-on-hold end to end; the `+P` commands; the GUI choices; the upstream PCM scope |

### 2.3 Rules every WP follows

- **Specs and real captures only.** Do not consult, port, build or run spandsp or any other modem implementation.
  Do not search the web for implementations.
- **Values come from the digests.** When a digest is in doubt, render the PDF page with PyMuPDF and read it as an
  image. The extracted text in `docs/specs/text` is lossy: use it only to find things.
- **Definition of done**, for every WP:
  - `cargo clippy --workspace --all-targets -- -D warnings` is clean. That includes `missing_debug_implementations`.
    Every public type derives `Debug`, and the modems are `Clone`.
  - `cargo test --workspace --release` passes, including every existing V.34 and V.90 test, unchanged unless the WP
    says otherwise.
  - The WP's own named tests exist and pass.
- **House style** (CA section 14, CD section 12, CV section 7, CT section 12):
  - module docs tell the story first;
  - every constant quotes its clause;
  - failure reasons are lowercase sentences;
  - tests are named as sentences, with a doc comment citing the clause or capture;
  - streaming `step`/`feed`/`heard`, no block APIs;
  - deadlines are `Option<(u64, &'static str)>` in samples;
  - British spelling.
- **Deliberate departures** from a Recommendation timer (for the 1.5 s VoIP round trip) are named constants whose doc
  gives the clause, the printed value and the live evidence.
- **Ambiguous readings** are named constants in the owning module (section 4), so one edit flips a reading if a
  capture disagrees.

---

## 3. Architecture decisions

**AD-1: a sibling `datapump::v92` module.**
- It reuses `v90` for everything V.92 inherits unchanged:
  - `encoder`, `sign`, `modulus` (6 intervals), `ucode`, `dil`, `pcm::Receiver`, `carrier::Watch`;
  - the framing helpers in `sequences`.
- Folding PCM upstream into `v90/analogue.rs` (1931 lines) or `v90/digital.rs` (1122 lines) would double their state
  (CA 11.1, CD 8.1).
- The V.90 modems stay as they are, and serve as V.92's "V.34 upstream" (V.90 data mode) path.

**AD-2: the start-up wrappers own V.92.**
- `v90::startup::Analogue` and `Digital` already own V.34's start-up, V.90's Phase 2, retrains and the V.34 fallback.
  They gain `enum Pcm { V90(..), V92(..) }`, chosen by which INFO1a layout Phase 2 settled on.
- The modem crate keeps `Pump::V90` and `Pump::V90Server`, and the start-up decides V.90 or V.92. This avoids editing
  about 20 exhaustive matches in `modem/src/lib.rs` (CV R12).
- `new()` stays V.90-only. `with_v92(..)` is opt-in and is used only for `+MS=V92`. That way `+MS=V90` behaves exactly
  as today, even against a V.92 server, since V.8 cannot say "V.92" (9.1 NOTE).

**AD-3: new events are accessors, not enum variants, wherever an enum is matched exhaustively elsewhere.**
- `v8line::Status` and `v90::startup::Status` are matched exhaustively in `modem/src/lib.rs`. So quick connect is
  reported through `v8line::Modem::quick()`, and the return to V.8 and the hold events through `take_back_to_v8()` and
  `hold_event()`.
- New `Info` variants do force edits in `v34/phase2.rs`, `tests/v90_vector.rs` and `tests/v34_vector.rs`. WP-02 makes
  them.

**AD-4: one shared model of the analogue transmitter.**
- `v92::{modulus, precoder, upstream}` is used by the analogue modem to transmit.
- The digital modem uses the same code to design CPd, to verify CPd before sending it, to predict B1u, and in tests.
- x(n) and v(n) are kept in f64 and never saturated (INTRO P-12).

**AD-5: separate sources and state machines.**
- Each side has a pull-driven signal source, like the existing `Source`/`Up`/`Out`:
  - `v92::up_source`, levels in LU units;
  - `v92::down_source`, G.711 octets.
- Each side has a state machine that consumes receiver events: `v92::analogue`, `v92::digital`.
- A source can be scripted by time in a unit test. So each state machine is tested against a scripted peer before the
  two meet in `tests/v92_call.rs`.

**AD-6: the SUV/CP/E acknowledge exchange is one side-agnostic machine** (`v92::exchange`). Phase 4, RR and FPE all use
it, with a context of Training, Renegotiation or FastExchange (P4P 2.3, 6.5).

**AD-7: PCM transmit timing.**
- `v92::transmit::PcmTransmitter` turns 8 kHz levels into line samples. It has two modes:
  - `Interpolated`: windowed-sinc reconstruction under 4 kHz, with an arbitrary fractional delay;
  - `Straight`: at fs = 16 000, the level on the codec's phase and a band-limited midpoint between.
- It has one-off shifts for Su-bar: +0.5 T, then +εT with ε = Jp bits 18:33 / 65 536.
- Its clock is slaved to a smoothed `pcm::Receiver::symbol_clock()` (drift, not the jittery phase) from the A3 silence
  on. It free-runs before that, and is never re-stepped after ε is applied (CA 11.3; INTRO 6.2).
- Which mode a live line needs is an open, evidence-driven question (CT 13.2). Both are built, and `Network` models
  both paths.

**AD-8: timers.**
- `v92::Deadlines` holds named slots, checked together. This replaces the single overwritten `deadline` slot (CA 11.4).
- RTD comes from Phase 2: RTDEd on the digital side, RTDEa after a full Phase 2 on the analogue side.
- After a short Phase 2 the analogue modem uses the RTD remembered in the memo. Failing that, it uses 0 for
  "shall not exceed" limits and 1.6 s for "wait at least" timeouts (P2S N-11).
- Departures:
  - TR3, 1500 ms from the start of Ja: relaxed as `SD_WAIT` is today (2.0 s + 2 RTD);
  - TR4 gets an RTD allowance;
  - short Phase 1 and short Phase 2 windows are kept as printed.

**AD-9: the fallback ladder.**
- The spec's own branches come first:
  - either side not V.92: V.90 layouts (9.3);
  - bit 70 clear: no Table 18 (8.4.1);
  - short Phase 2 failure: full Phase 2 (9.4.1.2.x, 9.4.2.2.2);
  - short Phase 1 with no answer: V.8 (9.2).
- Then local policy, in `v90::startup`:
  1. after `V92_RETRAINS` failed PCM-upstream start-ups, decline PCM upstream, so the next INFO1a is Table 10 (V.90
     data mode);
  2. after `V90_RETRAINS` more, decline PCM, so the next INFO1a is Table 11 (V.34).
- `+PIG=1` starts at step 1.

**AD-10: short Phase 1 is the V.8 path only.**
- Implemented: QC1a, QCA1a, QC1d and QCA1d, in every role, including the both-analogue path of Figure 7.
- Not implemented: the V.8 bis path (CRe, QC2x, QCA2x). See section 10.

**AD-11: a "recognised connection" memo.**
- `v92::memo::Recognized` holds what a full start-up taught: RTD, law, INFO1d bit 70, the V.34-upstream rate, carrier
  and pre-emphasis, PCM-upstream capability, U_QTS and LM.
- It gates the short-Phase-2 request (P2P A3, N-21) and the sending of QC1a (CTL 5.3).
- The modem crate holds one, keyed by dial string, else a single last-call slot. The GUI stores it.

**AD-12: modem-on-hold.**
- The transaction (`v92::hold`) runs on the existing V.34 INFO DPSK modem (`v34::dpsk`, 40-bit MH frames opt-in) and
  the existing tone A/B generators.
- MH detection is added beside the retrain response in `v34::phase2`, because a hold request can begin exactly like a
  retrain (9.10.1.1).
- Call control (held side, requester away, resume by short Phase 1) lives in `modem/src/hold.rs`, beside `faxcall.rs`.

**AD-13: the ODP/ADP bypass.**
- It applies when both P bits are set (9.2.5), or when both modems are V.92 and both indicated LAPM in V.8 (9.3.1).
- It uses the existing originator `Stack::without_detection()` and a new answerer-side `bypassing_detection()`.
- An ODP that arrives anyway is still answered (V.8 7.3 warning).

**AD-14: integration tests are split by topic,** so later waves do not collide:
- `v92_call.rs`, `v92_impairments.rs`, `v92_renegotiation.rs`, `v92_hold.rs`, `v92_quick.rs`, `v92_vector.rs` and
  `v92_replay.rs` in `crates/datapump/tests/`;
- `crates/modem/tests/v92_call.rs`.

---

## 4. Readings fixed now as named constants

These are the digests' open questions that block code. Each gets a documented default, owned by the named module, and
is checked against a real capture when one exists (live requests in section 12).

| Reading | Default | Owner | Digest |
|---|---|---|---|
| 6.4.1 step 4 uses d(f-1) | as printed: d(f-1); d(-1) = 0 | `v92::modulus` | INTRO Q-1, Q-2 |
| Point selection in a class | minimum \|x(n)\|; ties go to the smaller \|eta\| | `v92::precoder` | INTRO Q-3 |
| CPd point "linear value" | unsigned magnitude on the V.90 Table 1 linear scale; negatives mirror a(-eta-1) = -a(eta) | `v92::sequences` | P4D Q1, INTRO Q-4 |
| "128 points" in a set | send 2·LC ≤ 128; accept LC ≤ 128 | `v92::sequences` | P4D Q3 |
| CPd with an absent part | keep the previous values; the first CPd of a training must carry every part, else retrain | `v92::sequences`, `v92::upstream` | P4D Q2 |
| Unsigned Qa.b | raw / 2^b in [0, 2^a); G = raw / 262 144 | `v92::sequences` | INTRO P-10 |
| Time order of TRN2u symbol bits | LSB first (V.34 10.1.3.8 style); one switch | `v92::up_signals` | P4A A2, RRF Q-6 |
| TRN2u, SUVu, CPu and E2u in training/RR | not precoded or prefiltered | `v92::up_source` | P4A Q5, RRF Q-8 |
| TRN2u differential seed in RR | last transmitted sign of whatever precedes it (Ru-bar or E2u) | `v92::up_source` | P4A A4, RRF Q-5 |
| E2u's 13th symbol | one more scrambled, differentially encoded zero symbol | `v92::up_source` | P4A Q4 |
| RM memories | precoder, prefilter and trellis carry on from data mode; the modulus sign state is not clocked | `v92::upstream` | P4A Q7 |
| Upstream frame origin | the first symbol of the second TRN1u (8.5.7, 9.5.1.1.10), after the ε shift, at both ends | `v92::up_source`, `v92::receiver` | P3P D10, CD 7.4 |
| Su-bar 24.5T and (24+ε)T | a lasting delay of every later upstream symbol; ε measured after the 0.5 T shift | `v92::transmit`, `v92::epsilon` | P3S A1-A3 |
| CPt's 24 ones | scrambled, then differentially encoded | `v92::up_signals` | P3S A4 |
| TRN1u scrambler reset | before each TRN1u segment | `v92::up_signals` | P3S A6 |
| RR TRN2d/SUVd/CPd/Ed shaping and D | V.90 8.6 intro: data-mode shaping with K from CPt | `v92::down_source` | RRF Q-18, P4D Q7 |
| Silence codeword sign | constant positive Ucode 0 | `v92::anspcm`, `v92::down_source` | P4D Q12 |
| ANSpcm first reversal | after 3612 symbols, table polarity first | `v92::anspcm` | P1D Q2 |
| QC/QCA acceptance | both copies valid and equal; accept at bit 59 | `v8::quick` | P1D Q5, P1A Q3 |
| Timers "after sending X" | run from the end of X | `datapump::v8`, `v90::startup` | P1P I2 |
| U_QTS and LM when no memo | U_QTS `0101` (Ucode 70); LM `01` (-12 dBm0) | `modem` | P1P Q8, CV Q3 |
| Digital caller TONEq timeout | 2 s + RTD after the end of the received QCA1a, then back to V.8 | `v90::startup` | P1P Q6 |
| RR/FPE watchdog | 5 s + 2 RTD from the X-to-X-bar transition (+ 8004T + RTD for a silent period), then retrain | `v92::analogue`, `v92::digital` | RRF Q-1 |
| Minimum RR TRN2d | 2040T | `v92::digital` | RRF Q-3 |
| Post-silence TRN2u when ours to choose | 2400T to 8004T | `v92::analogue` | RRF Q-4 |
| CP-repeat window | the first CP/SUV whose reception completes after end-of-own-CP + 100 ms + RTD is included | `v92::exchange` | P4P Q12 |
| SUV bit 33 in the silence path | "your SUV with bit 32 was received"; ack state cleared when the silence ends | `v92::exchange` | RRF 3.5, P4D Q10 |
| 9.11 "drn in SUV" | drn = 0 in CPu/CPus (bits 21:25) or CPd (bits 22:26) | `v92::analogue`, `v92::digital` | RRF E-1 |
| MHack held until | remote RT for 100 ms or 2 s silence, bounded by T1 | `v92::hold` | MOH Q2 |
| MHnack's own timeout | starts once MHreq is no longer heard | `v92::hold` | MOH Q3 |
| `+PMHT` default | 0 (deny) | `at` | CTL 5.1 |
| Tone A guard level | keep -7 dB, as today | unchanged | P2P A1 |

---

## 5. Work packages

Every WP ends with the definition of done in 2.3. "Files" lists every file the WP may edit or create; files not
listed must not be touched. Paths are relative to `F:\dialupmodem2\`. Sizes: S ≈ up to 300 lines, M ≈ 300-800,
L ≈ 800-1300 (including tests).

### Wave 1: independent foundations

#### WP-01: Scaffold the `v92` module and its shared numbers (S)

- **Depends on:** none.
- **Clauses:** 1 (items c, e); 6.1; 6.2; 6.4 (Figure 1); Table 18 bits 14:17 and 18:24; 9.6.1.2.1; 9.6.2.2.1.
- **Files:**
  - `crates/datapump/src/lib.rs`
  - `crates/datapump/src/v92/mod.rs` (new)
  - new stub files `crates/datapump/src/v92/{sequences, modulus, precoder, upstream, up_signals, transmit, anspcm,
    exchange, receiver, epsilon, decoder, design, upchoice, up_source, down_source, analogue, digital, hold, memo}.rs`
- **Digests:** INTRO; CA (11.1, 14); CD (8.1, 12); CT (11.1, 12).
- **What to build:**
  - `pub mod v92;`.
  - The stubs contain only a `//!` paragraph naming the module's clause and the WP that fills it, so later waves edit
    disjoint files.
  - `mod.rs` holds:
    - the frame constants: `UP_INTERVALS = 12`, `CONSTELLATION_FRAME = 6`, `TRELLIS_FRAME = 4`;
    - the upstream ladder, drn 1..=19: rate (drn+17)·8000/6, in the same convention as `v90::rate_for`;
    - `up_bits(drn) = 2·(drn+17)`;
    - the 19-bit Ja mask helpers (bit k ↔ drn k+1);
    - `MD_STEP_SYMBOLS = 276`;
    - `FILTER_TOTALS = [192, 256, 320, 384]` and `FILTER_EACH = [128, 192, 256, 320]`;
    - `FilterSections` from INFO1a bits 12:13 (z1 allowed, p2 allowed);
    - the start-up watchdog (20 s + 6 RTD);
    - `Deadlines`: named `Option<(u64, &'static str)>` slots with `arm`, `clear` and `expired(now)`, which reports the
      earliest slot that has passed.
- **Tests:**
  - `the_upstream_ladder_runs_from_24000_to_48000_in_steps_of_8000_over_6`: 19 rates; drn 1 is 24 000, drn 19 is 48 000.
  - `a_data_frame_holds_2_drn_plus_34_bits`: K runs 36..=72, even.
  - `a_12_symbol_frame_holds_two_constellation_frames_and_three_trellis_frames`: j = i mod 6, k = i mod 4.
  - `md_length_counts_in_276_symbols`: 127 → 35 052 symbols; 276 is a multiple of 12.
  - `the_filter_limits_decode_from_info1a`: codes 0..3 → Ltot and Lmax as tabled.
  - `deadlines_report_the_earliest_that_has_passed`.

#### WP-02: V.92 INFO sequences and the MH frame (M)

- **Depends on:** none.
- **Clauses:**
  - 8.4, 8.4.1 (Tables 15-19); 9.3; 9.4;
  - 8.9.2 (Tables 32, 33);
  - V.34 10.1.2.3.1-10.1.2.3.2; V.90 8.2.3.2 (Tables 7-11).
- **Files:**
  - `crates/datapump/src/v34/info.rs`
  - `crates/datapump/src/v34/dpsk.rs`
  - `crates/datapump/src/v34/phase2.rs` (only arms for new `Info` variants)
  - `crates/datapump/tests/v90_vector.rs` and `crates/datapump/tests/v34_vector.rs` (only arms for new variants)
- **Digests:** P2S; P2P (sections 2-3); MOH (1.2); CD (6); CT (6, 9.1).
- **What to build:**
  - **INFO0d.** `Info0d` gains `v92` (bit 27) and `short_phase2` (bit 26). With both false, the encoding is
    bit-identical to today's.
  - **INFO0a.** A V.92 view of INFO0a (`Info0::pcm_flags()`: bit 26 = V.92, bit 27 = short request) that does not
    change V.34's clock-source meaning. Only PCM roles read it (P2S N-3).
  - **INFO1d.** `Info1c::pcm_upstream()` (bit 70) and an 8-bit 3429 view. A way to write bit 70 on purpose (N-4).
  - **Table 18.** New `Info1aPcmUp { sections, ltot_code, lmax_code, md_length, uinfo }`:
    - bits 34:36 = 6 and 37:39 = 6;
    - sends 32:33 = 0 and 40:49 = all ones;
    - UINFO must be 67..111 (N-24);
    - the receiver ignores 32:33 and 40:49.
  - **Table 19.** `Info1aPcm.high_carrier` (bit 33), written only for Table 19; Table 10 writes 0.
  - **Classification.** The answer-side 70-bit INFO1a is classified by (37:39, 34:36), as in P2S 10.4. Invalid
    combinations are dropped.
  - **MH.** New `Mh` (Table 32 indication and information nibbles as leftmost-first patterns; Table 33 T1 codes; MHclrd
    reasons) in the 40-bit INFO framing, with the CRC over bits 12..19. Undefined indications give `None`. A reserved
    cleardown reason is kept as "unknown".
  - **dpsk.** `Receiver::with_mh()` opts in to 40-bit frames on either side. The default stays unchanged.
- **Tests:**
  - `a_v92_info0d_says_so_in_bit_27_and_asks_for_short_phase_2_in_bit_26`: the P2S vector (CRC 0xDB49) and the P2P
    vector (0xA8A9) round-trip bit-exact.
  - `a_v92_info0a_has_the_two_bits_the_other_way_round`: CRC 0xAF5A; 0x2B52 with bit 28 set.
  - `a_v90_info0d_still_sends_zeros_in_bits_26_and_27`: identical to the pre-change encoding.
  - `info1d_bit_70_reads_as_pcm_upstream_and_the_3429_field_keeps_its_place`: CRC 0x086C vector.
  - `a_table_18_info1a_round_trips_and_is_not_thrown_away`: CRC 0xA858 and 0xF59A; every Ltot and Lmax code.
  - `a_table_19_info1a_carries_the_high_carrier_in_bit_33`: CRC 0x6742 and 0x7A52.
  - `the_receiver_tells_the_info1a_layouts_apart_by_bits_34_to_39`: Tables 10, 11 and 18 decoded; 6 with 0/1/2/7, and
    7 with anything, dropped.
  - `a_table_18_frame_is_never_read_as_a_frequency_offset`.
  - `every_mh_sequence_matches_its_worked_vector`: the 10 MOH 1.2.5 vectors and the 16 MHack CRCs; residue 0.
  - `an_undefined_mh_indication_is_ignored_and_a_reserved_cleardown_reason_is_kept`.
  - `mh_frames_are_heard_back_to_back_only_when_asked_for`.
  - All existing `info.rs`/`dpsk.rs` tests, `v90_vector` and `v34_vector` pass.

#### WP-03: V.92 layouts for CP and the DIL descriptor in `v90::sequences` (M)

- **Depends on:** none.
- **Clauses:** 8.5.1 (Table 23); 8.7.3; 8.5.4 (Table 20); V.90 8.3.1 (Table 12) and Table 14; V.34 10.1.2.3.2;
  clause 8 bit order.
- **Files:** `crates/datapump/src/v90/sequences.rs`.
- **Digests:** P3S (3.4, 4.1, 4.4); P3P (3.4, 3.6); P4A (3.3); CD (4); CA (13).
- **What to build:**
  - **Shared helpers.** `frame`, `unframe`, `put`, `get` and the sync/block constants become `pub(crate)`.
  - **CP layouts.**
    - `Layout { V90, V92 }`, with `Cp::to_bits_in(layout, pad_unit_bits)` and `Cp::from_bits_in(layout, bits)`.
    - The V.92 layout:
      - bit 18 = 0, and bit 18 = 1 is rejected;
      - the 2-bit type at 19:20: 0 = CPt, 1 = CPu (2 is CPus, handled in WP-09);
      - drn at 21:25; bits 26:30 reserved;
      - no silence bit, and 36:48 reserved, so `silence` and `upstream_rates` must be zero;
      - one fill bit, then zeros to the pad unit;
      - bits 31 onward as V.90.
    - `cp_length` and the mask code are shared. V.90 behaviour is unchanged.
  - **Descriptor.** `Descriptor.upstream_rates: Option<u32>` (19 bits). `Some` means the V.92 layout:
    - two mask words;
    - a start bit, the CRC and one fill bit at 221+P..238+P;
    - zeros to a multiple of 12 bits;
    - `descriptor_length` + 2 blocks.
  - **Finders.** `DescriptorFinder::v92()` and `CpFinder::v92(pad_unit)` accept a 0 after at least 17 ones (the 24-ones
    preambles; P3S 9.2). The V.90 finders are unchanged.
- **Tests:**
  - `a_v92_cpt_has_its_type_at_19_and_drn_at_21`: drn 16, masks with Ucodes 0..79, CRC 0xAC4D, 300 bits, tail as
    printed in P3S 4.1.
  - `a_v92_cpu_with_drn_22_puts_0_1_1_0_1_in_bits_21_to_25`.
  - `mask_bits_sit_where_table_23_puts_them`: u 0 → 137, 15 → 152, 16 → 154, 127 → 271.
  - `cp_lengths_follow_gamma_and_delta_for_each_pad_unit`: 290/426/970/1786 unpadded; the 12-, 24- and 36-bit pad
    tables of P4A 2.4 and P3S 4.1.
  - `a_v92_cpt_is_not_misread_through_the_v90_layout_or_back`: documents the doubled-drn hazard of CD 4.4; the
    cross-layout read is rejected.
  - `a_v92_descriptor_with_no_dil_is_276_bits`: CRC 0xB71C (P3P 12).
  - `v92_descriptor_lengths_pad_to_12_bits`: N=1 → 300; N=64 → 828; N=255 with L=128 → 2688.
  - `the_first_descriptor_behind_41_ones_is_found_by_the_v92_finder`: the V.90 finder still skips it.
  - The existing tests pass: real Jd CRC 0x776E, real 1736-bit descriptor, CP round trips.

#### WP-04: Network, part A: upstream impairments and a faster upstream kernel (M)

- **Depends on:** none.
- **Clauses:** 6.2 (network clock); 8.6.3 (the A/D phase is fixed); 8.5.7 (upstream framing). The rest is a model.
- **Files:** `crates/datapump/src/v90/network.rs`.
- **Digests:** CD (9); CT (2.4, 5.2, 7).
- **What to build:** every default stays unchanged, so V.90 tests are bit-identical.
  - Tabulate the upstream windowed-sinc kernel per fractional phase, instead of recomputing sin/cos per tap per tick.
  - `Direction { Down, Up }`.
  - Slips: `with_slips_in(dir, seconds, inserted)`, `with_slip_at_in(dir, ..)`, `with_slip_length(codewords)`
    (default 160), `slips_down()`, `slips_up()`. `slips()` stays downstream-only.
  - `with_upstream_robbed_bit(phase)`, with its phase independent of the downstream one.
  - `with_pads(down_db, up_db)`.
  - `with_delays(down, up)`.
  - `with_upstream_noise(level)`.
  - `with_upstream_phase(fraction_of_t)`: a fractional A/D sampling instant.
  - `up_code() -> (ucode, positive)` for the last upstream sample.
- **Tests:**
  - `the_tabulated_upstream_kernel_gives_the_same_levels_as_before`: exact, against a copy of the old computation on
    random input.
  - `an_upstream_robbed_bit_moves_codewords_only_in_its_own_octet_of_six`.
  - `an_upstream_pad_scales_every_level`.
  - `an_upstream_slip_moves_everything_after_it_by_the_slip_length`.
  - `a_ten_millisecond_cut_moves_everything_by_80_codewords`.
  - `a_fractional_upstream_phase_samples_between_our_samples`.
  - `unequal_legs_delay_each_direction_by_its_own_amount`.
  - `up_code_reports_the_ucode_and_sign_of_what_was_quantised`.
  - The four existing network tests and all of `v90_call.rs` pass unchanged.

#### WP-05: Move the V.90 downstream reading into `v90/downstream.rs` (M)

- **Depends on:** none.
- **Clauses:** V.90 8.4.1-8.4.3 and 8.6.4 (by reference); V.92 8.6.1 and 8.6.5 (DIL and Ri unchanged).
- **Files:**
  - `crates/datapump/src/v90/analogue.rs`
  - `crates/datapump/src/v90/downstream.rs` (new)
  - `crates/datapump/src/v90/mod.rs`
- **Digests:** CA (6, 8, 11.1); CT (5).
- **What to build:** a behaviour-preserving extraction (CA 11.1), committed on its own. `pub(crate) mod downstream;`.
  No constant or timing changes. What moves:
  - `JdReader`, which gains an acceptance predicate, so V.92 can require bit 47 = 0 for Jd and = 1 for Jp;
  - `RWatch` and `RSeen`, with levels supplied by the caller;
  - `Levels`, `levels_for`, `least_gap`, `nearest`, `slicer_for`;
  - `Trust` and `trusted_symbols`;
  - `find_place`;
  - `Frames`, with a pluggable sequence finder;
  - a `DilReader`, which owns the `dil*` state and the functions `begin_dil`, `find_dil_start`, `fit_dil`,
    `dil_levels`, `dil_symbol`, `count_dil` and `find_dil`, each taking `&mut pcm::Receiver`.
- **Tests:**
  - Every existing V.90 unit test, `v90_call.rs` (including every slip test) and `v90_vector.rs` pass unchanged.
  - The two J'd tests move into `downstream.rs`.
  - New: `a_jd_reader_with_a_predicate_ignores_frames_it_rejects` and `an_r_watch_takes_its_levels_from_the_caller`.
  - Run the ignored DIL slip sweep before and after, and record the same failure count (17/260) in the commit message.

#### WP-06: V.8 quick-connect sequences (S)

- **Depends on:** none.
- **Clauses:**
  - 8.2 (V.21 channels); 8.2.1 (Table 2); 8.2.3 (Table 4); 8.3.2 (Table 11); 8.3.4 (Table 13);
  - clause 8 bit order; 9.10.2.1 (U_QTS `1111`);
  - V.8 clause 5 and Table 1.
- **Files:** `crates/v8/src/quick.rs` (new); `crates/v8/src/lib.rs` (the `pub mod quick;` line only).
- **Digests:** P1A (5); P1D (4, 6, 9); P1P (3.2, 3.4, 3.5); CTL (2.2, 3.1); CV (5.1, 6.2.1).
- **What to build:**
  - `SYNC_QC = 0x55`.
  - `Kind { Qc1a, Qca1a, Qc1d, Qca1d }`, with the V.21 channel each uses.
  - `Uqts`: 15 Ucodes, or `Cleardown`, to and from the WXYZ pattern (W first).
  - `AnspcmLevel`: LM (L first), dBm0 and scl.
  - `Qc { kind, lapm, field }`:
    - `octet()` and `from_octet()`: b4 = 0; the digital variants also need b3 = b5 = 0;
    - `bits()`: the whole 60 or 70 bits.
  - `BitWatcher::feed(bit) -> Option<Qc>`. It needs:
    - at least ten ones, then `0101010101`;
    - a frame with start 0 and stop 1;
    - ones, the sync again, and an equal second copy.
    - It accepts at bit 59 (P1D Q5).
- **Tests:**
  - `every_table_2_code_names_its_ucode_and_1111_is_cleardown`.
  - `lm_codes_name_the_four_anspcm_levels`.
  - `qc1a_with_p_and_wxyz_0101_is_the_printed_60_bits_and_octet_0xa4`.
  - `qca1a_is_70_bits_ending_in_ten_ones_and_octet_0xa6`.
  - `qc1d_and_qca1d_carry_lm_where_tables_11_and_13_put_it`: all 16 printed strings; 0x05, 0x85, 0x87, 0xC3.
  - `a_cleardown_qc1a_is_octet_0xec`.
  - `a_qc_body_equal_to_a_v8_sync_is_still_a_qc`: body 0x00 and 0xE0.
  - `a_cm_carrying_0x55_as_an_extension_octet_is_not_a_qc`.
  - `a_qc_whose_two_copies_differ_is_not_accepted`.

#### WP-07: V.42, skipping the detection phase at both ends, and suspend/resume (S)

- **Depends on:** none.
- **Clauses:** V.92 9.2.5, 9.3.1; V.42 7.2.1.2, 7.2.1.3, 7.10, 7.11, Appendix VI.2; V.8 7.3/7.4 NOTE.
- **Files:** `crates/ec/src/stack.rs`; `crates/ec/src/lapm.rs`.
- **Digests:** CTL (6.2, 6.3, 7.4); P2P (5, P10); P1P (5.5); CV (6.5).
- **What to build:**
  - **`Stack::bypassing_detection()`**, for the answerer:
    - no ODP wait, no ADP;
    - wait for flags or XID;
    - an ODP that does arrive is still answered with an ADP.
  - Document `without_detection()` as the originator form.
  - **`suspend()`/`resume()`:**
    - freeze T400 during detection, and T401 once connected (T402/T403 are not implemented);
    - keep sequence numbers and the V.42 bis/V.44 dictionaries.
- **Tests:** add them inline in `stack.rs`.
  - `two_ends_that_both_skip_detection_reach_lapm_with_no_odp_or_adp_on_the_wire`.
  - `an_answerer_that_skips_detection_still_answers_an_odp`.
  - `a_suspended_link_keeps_its_timers_still_through_thirty_seconds_of_nothing`.
  - `after_resume_the_link_carries_on_with_the_same_sequence_numbers_and_dictionary`.
  - `ec/tests` and the modem-crate call tests pass.

### Wave 2: codecs and signal blocks

#### WP-08: Probe the V.92 capture (S)

- **Depends on:** WP-02, WP-06.
- **Clauses:** 8.2.1; 9.1; 9.3; 8.4.1 (Tables 15-19).
- **Files:** `crates/datapump/tests/v92_vector.rs` (new).
- **Digests:** CT (2.2, 8.4, 9.5); CV (Q8); P2S (15).
- **What to build:**
  - Copy the helpers of `v90_vector.rs`.
  - On `tests/vectors/v92-56k.wav`:
    - read the V.8 menus;
    - run `BitWatcher` on V.21(L) before the CM, looking for a QC1a;
    - read the INFO sequences with their V.92 bits;
    - classify the INFO1a.
  - Pin whatever the file shows as assertions, and record the findings in the test doc. This is evidence: it settles
    the P1A/CTL octet mapping and whether the capture used PCM upstream.
  - Do not change code to fit the capture. Report any contradiction instead.
- **Tests:**
  - `the_v92_menus_offer_pcm`.
  - `a_qc1a_before_the_cm_is_looked_for`: asserts presence or absence, as found.
  - `the_info_sequences_say_whether_v92_and_pcm_upstream_were_used`: INFO0d bit 27, INFO1d bit 70 and the INFO1a
    layout, as found.

#### WP-09: V.92 framed sequences (M)

- **Depends on:** WP-01, WP-03.
- **Clauses:**
  - 8.6.2-8.6.4 (Tables 21, 22);
  - 8.7.3 (Tables 23, 24); 8.7.4 (Tables 25, 26); 8.7.5 (Table 27);
  - 8.8.3 (Table 30); 8.8.5 (Table 31);
  - 9.11; V.34 10.1.2.3.2.
- **Files:** `crates/datapump/src/v92/sequences.rs`.
- **Digests:** P3S (5.2-5.4); P4A (0.4, 3); P4D (2, 5, 7); RRF (2.5, 2.8-2.11); P4P (3, 4); INTRO (3.5).
- **What to build:**
  - **`J`** is either:
    - `Jd`: the V.90 rate mask and look-ahead, with bits 47 and 48 = 0; or
    - `Jp`: ε (u16, bits 18:33), `eight_in_training` (bit 48), `eight_in_renegotiation` (bit 49).

    It dispatches on bit 47 before anything else. `JP_PRIME_BITS = 12`.
  - **`Suvd { silence, ack }`.**
  - **`Suvu { wait_for_cpu, level, silence, ack }`**: `level` is signed Q2.2, where 16 means "not measured"; it is
    clamped to ±3.75.
  - **`Cpus { drn, ack }`.**
  - **`Cpd`**: `drn`, `trellis`, `extend_e2u`, `ack`, `gain4` (nonzero), and three optional parts:
    - `moduli: Option<[u8; 12]>`;
    - `filters: Option<{z1, p1, z2, p2: Vec<i16>}>`;
    - `sets: Option<{index: [u8; 6], points: Vec<Vec<u16>>}>`.

    It has sequential `to_bits(pad_unit)` and `from_bits` (the word calculator of P4D 5.3). It also has:
    - `check(limits, up_bits)`: DC-1..DC-7 and the 8.8.3 SHALLs (no zero point, non-empty sets first, class
      feasibility, section, Ltot and Lmax limits);
    - `merged_over(previous)`, for absent parts.
  - **`CpFamily`** dispatch on bit 18 and the type field: CPt/CPu through the WP-03 V.92 layout, CPus, and the SUVs.
  - **A stream finder** for SUV and CP, tolerant of long runs of ones, with a length callback: CPd header words, then
    counts; `cp_length` for CPu.
  - **Pad helpers** for 24/36/K bits (upstream) and D bits (downstream).
  - **`rm_k(i, m, prime)`**: Tables 25 and 26, with the K11 errata readings.
  - **Q-format helpers**: Q0.15, Q1.14, Q0.16 (G = raw/262 144), Q3.13, Q1.6, Q2.2.
- **Tests:**
  - `jd_vectors_check_and_say_jd_in_bit_47`: CRC 0x776E (look-ahead 1) and 0xF366 (look-ahead 3).
  - `jp_carries_epsilon_in_bits_18_to_33_and_the_trn2u_sizes_in_48_and_49`: 0x1F4C, 0x3E4E, 0x70A6.
  - `a_jp_is_never_read_as_a_jd_and_back`.
  - `suvd_vectors_match`: 0xE960, 0x6D68, 0xAB64, 0x2F6C.
  - `the_smallest_cpd_is_69_bits`: CRC 0x570B; the CPd' form 0x5BE7.
  - `a_cpd_with_every_part_round_trips_and_keeps_start_bits_on_multiples_of_17`.
  - `an_absent_part_moves_everything_after_it`.
  - `cpd_limits_are_checked`: Ltot, Lmax, unsupported sections, a zero point, an empty set before a full one, an index
    past the sets, N < M or N < 2M at k = 3, 2^K > M.
  - `suvu_level_is_signed_q2_2_and_16_means_none`.
  - `cpus_is_type_2_with_its_crc_over_bits_18_to_33`.
  - `sequences_pad_to_24_36_k_or_d_bits`: 52 → 72 at both TRN2u sizes.
  - `the_cp_family_is_told_apart_by_bit_18_and_type`.
  - `rm_and_rm_prime_follow_tables_25_and_26`.
  - `a_cleardown_cpd_needs_no_parts`.

#### WP-10: The 12-interval modulus encoder (S)

- **Depends on:** WP-01.
- **Clauses:** 6.4.1.
- **Files:** `crates/datapump/src/v92/modulus.rs`.
- **Digests:** INTRO (6.4.1, 9.3-9.5); CD (7.7).
- **What to build:**
  - `Moduli12`, with its product in u128 and `fits(k)`, which checks 2^K ≤ M.
  - `Encoder { d_prev }`: steps 1-6, with the d(f-1) reading.
  - `Decoder { d_prev }`: rebuilds R0 → R → s → d, keeping d from R.
  - `reset()` on both.
- **Tests:**
  - `random_frames_round_trip_for_even_and_odd_products`: 2000 frames each, K up to 72.
  - `the_middle_value_of_an_odd_product_keeps_the_sign_chain`: R = (M-1)/2.
  - `an_inverted_channel_decodes_with_the_decoder_started_inverted`.
  - `a_frame_that_does_not_fit_is_refused`.
  - `seventy_two_bits_need_u128`: M = 255^12.

#### WP-11: Precoder, prefilter, constellations and inverse map (M)

- **Depends on:** WP-01.
- **Clauses:** 6.4.2; 6.4.3; 6.4.4 (Y0 use); 8.8.3 NOTE; V.34 9.6.3.1 (Figure 9, Table 13).
- **Files:** `crates/datapump/src/v92/precoder.rs`.
- **Digests:** INTRO (6.4.2-6.4.4, P-2..P-5, P-12); P4D (5.2, 5.4); CD (7.6, 7.8).
- **What to build:**
  - **`Constellation`**: the ascending positive magnitudes, mirrored to N = 2·LC points a(eta).
  - **Class members:**
    - for k < 3: eta = K + z·M;
    - for k = 3: eta = 2K + 2zM + parity, where the parity is (eta0 + eta1 + eta2 + Y0) mod 2, taken non-negative;
    - feasibility checks N ≥ M and N ≥ 2M.
  - **`Coefficients`**, built from `Cpd`'s integers: z1(1..), p1(1..), z2(0..), p2(1..), gain.
  - **`Precoder::choose(k, kind, y0, etas) -> (u, x, eta)`**, minimising |x|.
  - **`Prefilter`**: v(n); the output is G·v.
  - **`inverse_map(etas) -> [Y1..Y4]`** through `v34::trellis` on (2·eta+1) coordinates.
  - `reset()`. f64 state, never saturated.
- **Tests:**
  - `with_no_filters_the_output_is_the_chosen_level`.
  - `the_prefilter_s_feed_forward_starts_at_kappa_0_and_the_precoder_s_at_1`.
  - `the_precoder_output_stays_bounded_when_n_is_at_least_twice_m`: with a feedback p1.
  - `an_inverse_channel_gives_back_every_k_as_eta_mod_m`.
  - `the_fourth_symbol_s_parity_makes_the_frame_sum_equal_y0`.
  - `a_label_s_low_bit_is_the_parity_of_the_index_sum`: over ±130.
  - `the_state_is_never_saturated_only_the_output`.
  - `a_class_with_no_member_is_reported`.
  - `four_g_reads_as_q0_16`.

#### WP-12: Two-level and TRN2u upstream signals, senders and readers (M)

- **Depends on:** WP-01.
- **Clauses:** 8.5.1, 8.5.2, 8.5.4-8.5.7; 8.7.2; 8.7.6 (Tables 28, 29); 6.3; 3.8; V.34 clause 7.
- **Files:** `crates/datapump/src/v92/up_signals.rs`.
- **Digests:** P3S (3.1-3.3, 4.5-4.7); P3P (1.5, 3.1-3.3); P4A (2.1, 2.2); P4P (4.2); RRF (2.7).
- **What to build:**
  - **Pattern generators**, in LU units: `ru(n, bar)`, and `su(n, bar)` with a = √1.5.
  - **`Trn1u`**: GPA, reset first; output 0 → +1, 1 → -1.
  - **`TwoPointSender`**: GPA, then differential; seeded from the last symbol; the 24-ones preamble; a bit queue.
  - **`TwoPointReader`**: the polarity-blind differential decode, then descramble, with a plain-to-differential switch
    that replays the symbol it switched on.
  - **`Trn2uSender { size, first_bit_is_lsb }`**:
    - scrambler reset at the start;
    - the sign is differential, from a caller seed;
    - magnitudes (2m+1)/√5 or (2m+1)/√21.
  - **`Trn2uReader`**, the matching reader.
  - The segment-length constants (384, 24, 144, 2040), and `UP_SEQUENCE_UNIT = 12`.
- **Tests:**
  - `trn1u_starts_with_the_gpa_signs_the_digest_lists`: the 48-bit vector.
  - `ru_and_its_bar_are_the_printed_patterns`.
  - `su_has_the_same_power_as_trn1u`.
  - `a_two_point_sequence_decodes_whatever_the_line_polarity`.
  - `twenty_four_ones_let_the_far_descrambler_lock_before_the_sync`.
  - `trn2u_has_mean_square_lu_squared_at_both_sizes`.
  - `trn2u_descrambles_to_ones_and_a_following_sequence_parses_from_its_first_zero`.
  - `the_trn2u_bit_order_is_one_switch`: each reading round-trips with itself, and the two differ.

#### WP-13: ANSpcm, QTS and TONEq (M)

- **Depends on:** WP-01.
- **Clauses:**
  - 8.3.1 (Tables 6-10); 8.3.6; 8.2.5;
  - 9.2.1.3, 9.2.2.3, 9.2.3.3, 9.2.4.3;
  - 9.8.1.1.3 (silence); V.90 Table 1.
- **Files:** `crates/datapump/src/v92/anspcm.rs`.
- **Digests:** P1D (3, 8, Appendix A); P1A (7, 10.3, 10.4, 11); P1P (3.6-3.9).
- **What to build:**
  - The Appendix A octet tables as consts, 4 levels × 2 laws. The generator (floor(v + 0.5), f64, G.711 decision
    intervals) is kept only as a test.
  - `anspcm_octet(n)`: XOR 0x80 in every other 3612-symbol block.
  - `qts_octet(n, uqts)`: 768 + 48 symbols; +0 and -0 are distinct octets.
  - `silence_octet(law)`.
  - `ToneqGenerator`: exactly 980 Hz.
  - `ToneqDetector`: a steady 980 Hz for at least 60 ms with no 1180 Hz; it rejects V.21(L) data.
  - An `AnspcmWatch` for the analogue side: 2100 Hz without the 15 Hz AM (using `v8::ansam`), plus the QTS 1333 Hz
    burst and its reversal time.
- **Tests:**
  - `the_generator_reproduces_all_2408_tabled_octets`.
  - `table_7_k82_a_law_is_08`.
  - `a_reversal_flips_only_the_polarity_bit_every_3612_symbols`.
  - `anspcm_power_is_the_level_lm_names`: within ±0.2 dB of the P1D ANS-17 table.
  - `qts_is_128_patterns_then_8_inverted_ones`.
  - `toneq_is_heard_after_60_ms_and_cm_is_not_taken_for_it`.
  - `anspcm_is_told_from_ansam_by_the_missing_15_hz`.
  - `the_qts_reversal_is_found_to_within_a_symbol`: through `Network::down`.

#### WP-14: The SUV/CP/E exchange (M)

- **Depends on:** WP-01.
- **Clauses:** 9.6.1.1.1-9.6.1.1.6; 9.6.2.1.1-9.6.2.1.6; Figures 12-14; 9.8.1.1.2 and 9.8.2.1.3 (entry); 9.9 (context);
  Table 30 bit 29.
- **Files:** `crates/datapump/src/v92/exchange.rs`.
- **Digests:** P4P (3, 5, 6.5); RRF (5); P4D (10.1); P4A (5.1).
- **What to build:** the side-agnostic machine of P4P 6.5.
  - **Context:** Training, Renegotiation or FastExchange.
  - **Inputs:**
    - the peer's SUV (ack, silence, wait-for-CPu), CP (a CPus counts as a CP) and E;
    - own sequence start and end times;
    - `now` and RTD.
  - **Outputs:** the next sequence to send: `Suv { ack, silence }`, `Cp { ack }` or `E`.
  - **Rules:**
    - one CP, sent after the first peer SUV;
    - the repeat rule with the completion reading (section 4);
    - termination once an ack has been sent and an ack or E has been received, after finishing whatever is in flight
      (E-8 reading);
    - a wait-for-CPu policy flag;
    - the ack bit changes only at sequence boundaries.
  - Silence requests are reported to the owner.
- **Tests:**
  - `figure_12_crossing_cps_end_in_ed_and_e2u`.
  - `figure_13_a_cpu_heard_first_makes_the_single_cpd_a_cpd_prime`.
  - `figure_14_a_lost_cpu_is_repeated_after_100_ms_and_a_round_trip`.
  - `the_sequence_that_completes_after_the_deadline_still_counts`.
  - `no_second_cp_is_sent_while_acks_arrive`.
  - `an_e_counts_as_an_acknowledgement`.
  - `a_cpus_counts_as_a_cpu`.
  - `the_ack_bit_changes_only_at_a_sequence_boundary`.
  - `with_a_1_5_s_round_trip_no_early_repeat_happens`.

#### WP-15: The PCM transmitter (M)

- **Depends on:** WP-01, WP-04.
- **Clauses:** 6.2; 8.5.6 with 9.5.2.1.7-9.5.2.1.8; 8.6.3; 3.8.
- **Files:** `crates/datapump/src/v92/transmit.rs`; `crates/datapump/src/v90/pcm.rs`.
- **Digests:** CA (5, 7, 11.3, 13); CT (7, 10); INTRO (6.2, 6.4 output level); P3S (9.2).
- **What to build:**
  - **`PcmTransmitter::new(fs, Interpolated | Straight)`**, with `next_sample(clock, next_level)` in the pull shape of
    `qam::Transmitter`.
  - **`lookahead()`.**
  - **`delay(fraction_of_t)`**, a one-off shift. `Straight` is exact only for multiples of 0.5 T; document that.
  - **A symbol-to-line-sample map.**
  - **LU scaling**, with peaks bounded to 0.3 of full scale (the `dil::LOUDEST` ceiling).
  - **`pcm::Receiver::symbol_clock()`**: the smoothed samples per symbol plus a phase that does not follow the
    `hold_centre` steps.
- **Tests:**
  - `at_twice_the_rate_every_other_sample_is_the_level`.
  - `through_the_network_the_codec_reads_back_the_levels_sent`: unquantised, gain 1, error below -40 dB.
  - `a_half_symbol_delay_moves_the_codec_samples_by_half_a_symbol`.
  - `s_bar_u_is_extended_by_epsilon_to_a_65536th_of_a_symbol`: ε = 0x4000 moves the waveform by T/4 ± 1 %.
  - `a_clock_slaved_to_the_receiver_stays_on_the_network_clock_at_120_ppm`.
  - `the_receiver_s_symbol_clock_does_not_jump_when_it_holds_centre`.
  - The existing `pcm.rs` tests pass.

#### WP-16: Phase 2, V.92 flags and the Table 18 choice in full Phase 2 (M)

- **Depends on:** WP-02.
- **Clauses:** 9.3; 9.3.1; 8.4.1; Tables 17, 18; 9.7; V.90 9.2.1.1.8 and 9.2.2.1.9.
- **Files:** `crates/datapump/src/v34/phase2.rs`; `crates/datapump/src/v34/startup.rs`.
- **Digests:** P2P (2, 4, 7, 11); P2S (10, 11, 16); CA (2, 11.2); CD (2, 5.1, 8.4).
- **What to build:**
  - **`V92Wish { capable, short_phase2, pcm_upstream, up_caps }`**, carried by the `Pcm` roles. The existing
    constructors mean "not V.92".
  - **INFO0.** Write the INFO0 bits at the swapped positions. Record the far flags only in PCM roles. Add `both_v92()`.
  - **Digital INFO1d.** Bit 70 = PCM upstream allowed when both modems are V.92; otherwise the V.90 carrier flag,
    unchanged.
  - **Analogue INFO1a.** Table 18 when all of these hold: both V.92, bit 70 set, `pcm_upstream` wanted, and PCM
    upstream not declined. Otherwise today's Table 10 or 11.
  - **`decline_pcm_upstream()`**, with a pass-through in `v34::startup`.
  - **`info1a_pcm_up()`** accessor.
  - **Digital acceptance.** The digital modem accepts Table 18 only if it set bit 70 and both are V.92; otherwise the
    frame is "not received" (P2P P15).
  - **Retrains.** `again()` keeps the flags and always runs full Phase 2.
  - **`lapm_bypass_allowed()`** means both modems are V.92.
- **Tests** (the `phase2.rs` `Line` harness):
  - `a_v92_pair_settles_on_pcm_upstream`.
  - `a_v92_analogue_modem_meets_a_v90_digital_modem_with_table_10`.
  - `a_v90_analogue_modem_meets_a_v92_digital_modem_with_table_10`.
  - `bit_70_clear_means_no_table_18`.
  - `declining_pcm_upstream_gives_table_10_next_time`.
  - `a_retrain_between_v92_modems_keeps_the_flags_and_runs_full_phase_2`.
  - `a_v34_info0_with_a_clock_of_1_is_not_taken_for_v92`.
  - `a_v90_pair_settles_on_v90` passes unchanged.

#### WP-17: Network, part B: echo, transcoder, gain control and softphone paths (M)

- **Depends on:** WP-04.
- **Clauses:** 1 b (channel separation by echo cancellation); 9.8 (the purpose of the silent period). The rest is a
  model.
- **Files:** `crates/datapump/src/v90/network.rs`.
- **Digests:** CD (9.2 N2-N11); CT (7, 10).
- **What to build:** defaults unchanged.
  - `with_echo(hybrid_db, taps)`: a short FIR of the reconstructed downstream, added to the upstream sum before the
    quantiser.
  - `with_far_echo(db, delay)`.
  - `with_transcoder(to_law, low_pass)`, per direction.
  - `with_upstream_gain_control(ceiling, release)`.
  - `UpPath { Loop, Straight { phase }, Resampled { delay } }` and `with_up_path`. On a `Straight` path, a clock offset
    becomes upstream slips at 20 ms / |δ|.
- **Tests:**
  - `the_hybrid_echo_is_where_it_was_put`.
  - `the_transcoder_is_3_db_down_at_3750_and_18_db_down_at_4000`.
  - `a_straight_softphone_path_hands_the_encoder_our_samples_exactly`.
  - `a_resampled_path_does_not`.
  - `a_clock_off_on_a_softphone_path_becomes_slips`.
  - `an_upstream_gain_control_squashes_loud_samples_and_the_ones_after`.
  - `v90_call.rs` passes unchanged.

### Wave 3: the upstream chain, receiver and sources

#### WP-18: The upstream data-mode encoder chain (M)

- **Depends on:** WP-09, WP-10, WP-11.
- **Clauses:**
  - 6.3; 6.4.1-6.4.4;
  - 8.7.1 (resets, n = 0, interval 0); 8.7.4 (RM through the chain, trellis coded); 8.7.7 (FB1u on the old chain);
  - 9.9.2.1.2 (scrambler and differential reset);
  - Table 30 bits 27:28.
- **Files:** `crates/datapump/src/v92/upstream.rs`.
- **Digests:** INTRO (6.3, 6.4, "Complete per-data-frame procedure"); P4A (2.3, 3.1, 3.5, 3.8); RRF (2.5); CD (7.8).
- **What to build:**
  - **`UpParams::from_cpd(cpd, previous, limits)`.**
  - **`UpEncoder::new(params)`**, with every memory zero.
  - **`next_level(bit_source)`**, per symbol:
    - at a frame start, take K bits and scramble them with GPA;
    - run the modulus encoder;
    - choose the point with the precoder;
    - at k = 3, run the inverse map and clock the 16/32/64-state `v34::trellis::Code`, with its inputs masked to the
      code;
    - run the prefilter and apply G.
  - **`force_k(pattern)`**, for RM and RM': no bits are consumed, and the modulus sign state is not clocked.
  - **`reset_scrambler_and_differential()`**, for FPE.
  - **Introspection:** `frame_symbol()`, `sent_etas()`, and an RMS tracker for the SUVu level field.
- **Tests:**
  - `every_trellis_frame_s_index_sum_has_the_parity_of_y0`.
  - `a_separate_encoder_fed_table_13_regenerates_the_same_y0`.
  - `b1u_is_a_fixed_sequence_for_a_given_cpd`.
  - `rm_puts_the_table_25_pattern_into_the_classes`.
  - `rm_prime_is_rm_shifted_by_two`.
  - `the_32_and_64_state_codes_are_selected_by_cpd_bits_27_and_28`.
  - `a_cpd_without_a_modulus_part_keeps_the_previous_moduli`.
  - `a_first_cpd_missing_a_part_is_refused`.
  - `the_output_power_with_a_flat_prefilter_is_one_when_g_is_one_over_rms`.

#### WP-19: The digital modem's upstream PCM receiver (M)

- **Depends on:** WP-04, WP-12, WP-15.
- **Clauses:** 9.5.1.1.1-9.5.1.1.3; 9.5.1.1.6-9.5.1.1.7; 9.5.1.1.10; 8.5.5-8.5.7; 8.7.6; Table 1 note 1.
- **Files:** `crates/datapump/src/v92/receiver.rs`.
- **Digests:** CD (7.1, 7.2, 7.4); P3P (6.1, 11); CT (4.4); CA (4.3 as a template).
- **What to build:** a T-spaced receiver.
  - **Hunts.** `feed(level)`, and `hunt()` for period-6 patterns and their inversions: Ru `+++---` and Su `+0+-0-`.
    They report `Heard::RuReversal { at }` and `Heard::SuReversal { at }`.
  - **`idle_for(symbols)`**, for MD.
  - **`train_on_trn1u(from)`**: least squares against the known GPA sequence, with an alignment search, giving a
    feed-forward plus decision-feedback equaliser. Rows are `Complex` with im = 0, through `dsp::least_squares`.
  - **NLMS tracking.**
  - **Slicers:** two-point, TRN2u (4 or 8), and per-j level sets.
  - **Framing:** `set_frame_origin(at)` and `interval()`.
  - **`Lost`/`Found`** on an error jump.
  - **A sample history** for the ε estimator and the channel estimate.
  - **`Symbol` events.**
- **Tests:**
  - `ru_and_its_reversal_are_found_in_either_polarity`.
  - `su_is_found_and_its_reversal_timed_to_the_symbol`.
  - `trn1u_trains_the_receiver_over_a_loop_with_noise`: SNR ≥ 25 dB at noise 1e-4.
  - `two_point_decisions_after_training_are_error_free_at_30_db`.
  - `trn2u_levels_are_decided_at_both_sizes`.
  - `silence_and_noise_do_not_train`.
  - `the_frame_count_starts_where_it_is_told`.

#### WP-20: The sampling-phase estimator (M)

- **Depends on:** WP-04, WP-12, WP-15.
- **Clauses:** 8.6.3 (Jp bits 18:33); 9.5.1.1.6-9.5.1.1.8; 9.5.2.1.7-9.5.2.1.8.
- **Files:** `crates/datapump/src/v92/epsilon.rs`.
- **Digests:** P3S (4.6, 5.3, 9.2, 9.3); P3P (3.5, 11.3); CD (7.3).
- **What to build:** `PhaseEstimator`.
  - It collects the A/D samples over Su, the 24.5T Su-bar and the Su after it. Those give two views half a symbol
    apart.
  - It fits the known pattern through a short channel at trial phases.
  - It returns `epsilon: u16`: the extra delay, after the 0.5 T shift, that puts the transmitter's symbols on the A/D
    instants. It also returns a confidence.
  - Statistics use medians over windows (the jitter-slip memory note).
- **Tests:**
  - `every_upstream_sampling_phase_is_found_to_a_64th_of_a_symbol`: phases 0, 0.25, 0.5 and 0.75, through the WP-15
    transmitter and the loop network.
  - `the_estimate_holds_with_noise`.
  - `applying_epsilon_puts_the_symbols_on_the_samples`: residual below -30 dB.
  - `a_slip_during_su_is_reported_not_believed`.

#### WP-21: The analogue upstream source, Phase 3 (M)

- **Depends on:** WP-03, WP-09, WP-12, WP-15.
- **Clauses:** 9.5.2.1.1-9.5.2.1.11; 8.5.1-8.5.7; Table 20; Table 1 (circuit 107).
- **Files:** `crates/datapump/src/v92/up_source.rs`.
- **Digests:** P3S (4, 6.2); P3P (2, 3, 7); CA (4.2, 11.3).
- **What to build:**
  - **`Up`**, the Phase 3 subset: Silence, Ru, RuBar, Md (zeros), Trn1u, Ja, Su, SuBar{extension}, Cpt, E1u.
  - **`UpSource::next() -> f64`**, pull-driven.
  - **`change(up)`**, with boundary rules per segment:
    - Ja stops at the next 12-bit boundary;
    - Su and Su-bar have fixed lengths;
    - CPt changes at a sequence end, and E1u follows the CPt in flight.
  - It drives `PcmTransmitter::delay` for the 0.5 T and εT extensions.
  - The frame counter resets at the first symbol of the second TRN1u. From then on, a debug assertion checks that
    every sequence is whole frames.
  - `circuit_107()` and counters.
- **Tests:**
  - `phase_3_upstream_is_ru_ru_bar_trn1u_and_ja_in_that_order`: 384, 24, then at least 2040 in multiples of 12.
  - `ja_is_24_ones_then_whole_v92_descriptors_and_stops_on_a_12_bit_boundary`: decoded with WP-12's reader and WP-03's
    V.92 finder, including the 19-bit mask.
  - `the_first_s_bar_u_shifts_everything_after_it_by_half_a_symbol`.
  - `the_second_s_bar_u_adds_epsilon`.
  - `the_second_trn1u_starts_interval_0_and_every_later_sequence_is_whole_frames`.
  - `cpt_repeats_identically_after_its_24_ones_and_e1u_follows_the_one_in_flight`.
  - `circuit_107_turns_on_with_the_second_s_bar_u`.

#### WP-22: The digital downstream source, Phases 1 and 3 (M)

- **Depends on:** WP-03, WP-09, WP-13.
- **Clauses:**
  - 8.3.1; 8.3.6;
  - 8.6.1-8.6.8, with V.90 8.4.1, 8.4.4, 8.4.5 and 8.6.4;
  - 9.5.1.1.3-9.5.1.1.13; clause 5 (frame alignment); 9.8.1.1.3.
- **Files:** `crates/datapump/src/v92/down_source.rs`.
- **Digests:** P3S (5, 6.1); P3P (4, 6); P1D (3, 8); CD (3.2, 8.2).
- **What to build:**
  - **`Out`**, the Phase 1 and 3 subset: Quiet, Qts, QtsBar, Anspcm, Sd, SdBar, Trn1d, Jd, Jp, JpPrime, Dil, Scr, Ri,
    RiBar.
  - **Octets per symbol**, by law.
  - **The frame counter.** Its origin is the first QTS or Sd symbol, or a grid passed in `frame_origin`. Sd waits for
    interval 0.
  - **One differential chain:** Jd starts from TRN1d's last symbol, Jp from Jd's, Jp' from Jp's. Sign 0 is negative.
  - **SCR** continues the scrambler.
  - **DIL** comes from `Descriptor::symbols` and ends on a segment boundary.
  - **Ri** runs until pending, then Ri-bar for 24 symbols.
  - **Boundaries:** Jp waits for the Jd in progress; Jp' for the Jp in progress.
- **Tests:**
  - `qts_qts_bar_and_anspcm_follow_with_no_gap_and_qts_starts_interval_0`.
  - `sd_lands_on_interval_0_of_a_grid_begun_at_qts`.
  - `trn1d_signs_are_the_gpc_vector`.
  - `jd_then_jp_then_jp_prime_decode_with_one_differential_chain`.
  - `jp_waits_for_the_jd_in_progress_to_finish`.
  - `the_dil_stops_on_a_segment_boundary_and_ri_follows`.
  - `scr_is_whole_frames_of_scrambled_ones`.
  - `ri_bar_is_exactly_24_symbols`.

#### WP-23: Short Phase 2, the error-free procedure (M)

- **Depends on:** WP-16.
- **Clauses:** 9.4; 9.4.1.1.1-9.4.1.1.5; 9.4.2.1.1-9.4.2.1.4; Figure 9; Table 19; 9.5.2.1.1.
- **Files:** `crates/datapump/src/v34/phase2.rs`.
- **Digests:** P2P (2, 6, 8, 11); P2S (11.2, 16).
- **What to build:**
  - **The decision.** Latch the four-bit decision as soon as the far INFO0 decodes.
  - **The digital stages:**
    1. wait for Tone A, having sent B for at least 50 ms;
    2. B reversal, then 10 ms of B-bar;
    3. silence;
    4. on the A reversal, compute RTDEd;
    5. INFO1a;
    6. done, with the layout.
  - **The analogue stages:**
    1. wait for the B reversal;
    2. A reversal at 40 ± 1 ms at the line terminals, scheduled below a symbol with detector-latency and filter-delay
       compensation (P2P P5);
    3. 10 ms of A-bar;
    4. INFO1a at once, with its leading point continuing A-bar's phase;
    5. done.
  - **`ShortPlan`**, supplied by the analogue owner:
    - Table 18 fields, or Table 19's rate and carrier;
    - no plan means no request (9.4 restriction, N-21);
    - the plan's remembered RTD;
    - Table 19's frequency offset is sent as -512.
- **Tests:**
  - `both_asking_for_short_phase_2_get_it_and_rtded_is_the_line_s_round_trip`: 0.03, 0.6 and 1.5 s, ±1 ms.
  - `one_side_not_asking_means_full_phase_2`.
  - `a_v90_far_end_s_zeros_mean_full_phase_2`.
  - `the_answering_a_reversal_leaves_40_ms_after_the_b_reversal_arrives`: measured on the line, ±1 ms.
  - `info1a_follows_ten_ms_of_a_bar_with_no_gap`.
  - `short_phase_2_hands_over_the_planned_table_18_or_table_19_info1a`.
  - `an_analogue_modem_with_no_plan_does_not_ask`.

#### WP-24: Modem-on-hold transactions (M)

- **Depends on:** WP-02.
- **Clauses:** 8.9.1; 8.9.2 (Tables 32, 33); 9.10; 9.10.1; 9.10.1.1; 9.10.1.2 (Table 34); 9.10.2.1-9.10.2.3;
  Figures 20-24; V.8 7.2; V.90 8.2.3.1.
- **Files:** `crates/datapump/src/v92/hold.rs`; `crates/datapump/src/v8.rs` (only to make the ANSam generator
  `pub(crate)`).
- **Digests:** MOH (all); CA (11.8); CV (6.3.2).
- **What to build:** `Transaction` for a role.
  - **Tones.** The analogue modem's RT is Tone A and it listens for Tone B; the digital modem's is Tone B.
  - It runs its own `dpsk` transmitter and an MH-enabled receiver.
  - **Initiator** (MHreq, MHclrd{reason} or MHfrr):
    - optional RT: at least 50 ms, or at least 20 ms after an MH sequence;
    - sequences back to back until the response;
    - 2 s + RTD, then finish the sequence in flight, giving `GaveUp`.
  - **Responder**, per Table 34, with `Policy { grant: Option<t1>, after_refusal }`:
    - responses stop on ANSam, on silence, or when the initiating sequence has been absent for 200 ms by the clock;
    - a requester that is refused must choose within 10 s.
  - **Held side:** on the remote's RT for 100 ms, or 2 s of silence, finish MHack, then ANSam within 80 ms, giving
    `Holding { t1 }`.
  - **Fast reconnect:** MHfrr leads to at most 80 ms of silence, then ANSam (`FastReconnectAnswer`). The MHfrr sender,
    after 1 s of ANSam, gives `FastReconnectCall`.
  - **Cleardown:** MHclrd/MHcda give `ClearedDown { reason }`.
  - **The retrain race:** a responder watching for both reports `RetrainInstead` when a reversal arrives with no MH.
- **Tests** (two transactions over a delay line):
  - `figure_20_a_request_granted_leaves_the_held_modem_sending_ansam`.
  - `figure_21_a_refusal_then_cleardown_ends_both`.
  - `figure_22_a_refusal_then_fast_reconnect_ends_in_ansam_heard_for_a_second`.
  - `figure_23_a_cleardown_request_is_acknowledged`.
  - `figure_24_a_fast_reconnect_request_is_answered_with_ansam`.
  - `an_unanswered_request_gives_up_after_two_seconds_and_a_round_trip_at_a_sequence_boundary`.
  - `a_response_stops_200_ms_after_the_request_does`.
  - `mhack_carries_the_t1_code_granted`.
  - `the_digital_initiator_s_mhreq_wins_over_the_analogue_responder_s_reversal`.
  - `a_slip_that_breaks_one_frame_does_not_count_as_200_ms_absent`.

### Wave 4: decoder, Phase 3 state machines, short Phase 2 recovery, design

#### WP-25: The upstream decoder and the RM watch (M)

- **Depends on:** WP-18.
- **Clauses:** 6.4.1-6.4.4 (inverted); 8.7.4; 9.9.1.1.2; 9.9.1.2.1.
- **Files:** `crates/datapump/src/v92/decoder.rs`.
- **Digests:** CD (7.7); RRF (2.5, 2.18); INTRO (6.4.3, Q-15).
- **What to build:**
  - **`Viterbi4d`:** the 16/32/64-state codes, clocked every 4 symbols, with per-residue subset metrics. It has no C0
    and no V0.
  - **`UpDecoder`:**
    - K from eta, per k;
    - the 12-modulus decode with d(f-1);
    - GPA descramble;
    - resets for B1u and FPE.
  - **`RmWatch`:**
    - works in the K domain;
    - ignores intervals with M = 1;
    - reports RM after 8 frames, and the RM-to-RM' turn (four zeros) to the symbol.
  - **`could_have_sent`**, for re-framing.
- **Tests:**
  - `any_path_through_the_upstream_trellis_decodes`: all three codes, with and without noise.
  - `an_inverted_line_still_decodes`.
  - `a_whole_chain_round_trips_through_a_precoder_on_an_ideal_channel`.
  - `rm_is_seen_within_eight_frames_and_its_turn_to_rm_prime_to_the_symbol`.
  - `random_data_never_looks_like_rm_in_100000_frames`.
  - `an_interval_with_a_modulus_of_one_is_ignored_by_the_rm_watch`.

#### WP-26: The analogue modem, Phase 3 (L)

- **Depends on:** WP-05, WP-09, WP-15, WP-21, WP-22 (test script).
- **Clauses:** 9.5; 9.5.2.1.1-9.5.2.1.11; 9.5.2.2.1-9.5.2.2.2; Figures 10, 11; 9.6.2.2.1; 9.7.2.1-9.7.2.2; 6.2;
  8.6.x (reception).
- **Files:** `crates/datapump/src/v92/analogue.rs`.
- **Digests:** P3P (6-9, 13); P3S (6, 9); CA (3, 4, 7, 10, 11.2-11.4, 13, 14); CT (12).
- **What to build:**
  - **Settings.** `Settings::new(server: &Info0d, asked: &Info1aPcmUp, probe: Option<&Info1c>, rtd: Option<f64>, fs)`,
    with a test-only `no_dil` option. The optional probe and RTD cover short Phase 2 later.
  - **Stages:** SendTraining → AwaitSd → Training → AwaitJd → AwaitJp → AwaitJpPrime → Dil → Cpt → `Phase3Done`. The
    last is a placeholder for WP-34.
  - **Downstream**, through `pcm::Receiver` and `v90::downstream`:
    - `JdReader` with a bit-47 predicate for Jd and for Jp;
    - `DilReader`;
    - `RWatch` at UINFO for Ri and Ri-bar.
  - **Upstream**, through `UpSource` and `PcmTransmitter`: free-running until the A3 silence, slaved after it, and
    never re-stepped after ε.
  - **DIL and CPt.** The DIL comes from `dil::design`, with the 19-bit mask of rates `UpEncoder` supports. CPt comes
    from `dil::choose` with the V.92 header.
  - **Deadlines:**
    - TR3, relaxed like `SD_WAIT`, with the departure documented;
    - TR4, from the Ja cut, with an RTD allowance;
    - Su no later than 5 s;
    - CPt no later than 5 s after Su-bar;
    - start-up 20 s + 6 RTD.
  - **Tone B** requests a retrain.
  - **Reporting:** `phase()` strings ("V.92 phase 3: ..."), `circuit_107()`, and accessors (`epsilon`, `jp`,
    `far_jd`, `descriptor`).
- **Tests** (a scripted digital modem, from `DownSource` over `Network`, driven by time and heard events):
  - `the_analogue_modem_sends_phase_3_in_order_and_hears_a_scripted_digital_modem`: Ru, TRN1u, Ja (with its descriptor
    and mask decoded), Su and Su-bar, the second TRN1u, CPt and E1u, all read back from our own upstream.
  - `epsilon_from_jp_is_applied_to_the_second_s_bar_u`.
  - `a_jp_is_never_taken_for_a_jd`.
  - `cpt_starts_after_2040t_of_trn1u_and_within_5_s_of_s_bar_u`.
  - `e1u_follows_ri_after_the_cpt_in_flight`.
  - `no_sd_gives_the_documented_failure_after_the_relaxed_wait`.
  - `tone_b_during_phase_3_asks_for_a_retrain`.
  - `with_no_dil_asked_ri_comes_before_cpt_and_e1u_follows_ri_bar`.

#### WP-27: The digital modem, Phase 3 (L)

- **Depends on:** WP-09, WP-19, WP-20, WP-21 (test script), WP-22.
- **Clauses:** 9.5; 9.5.1.1.1-9.5.1.1.13; 9.5.1.2.1-9.5.1.2.2; Figures 10, 11; 8.6; 9.6.1.2.1; 9.7.1.1-9.7.1.2.
- **Files:** `crates/datapump/src/v92/digital.rs`.
- **Digests:** P3P (6, 8, 9, 11, 13); P3S (5, 6); CD (3, 7, 8.2, 8.3, 11, 12).
- **What to build:**
  - **Settings.** `Settings::new(law, info0d, asked: &Info1aPcmUp, rtd, habits, frame_origin: Option<u64>)`.
    `frame_origin` lets short Phase 1 hand over a grid begun at QTS.
  - **Jd and Jp.** Jd advertises all rates with look-ahead 1. The Jp size policy is 8-point in training when the
    TRN1u SNR allows, and 4-point in RR.
  - **Stages:**
    1. AwaitRu; then WaitMd if the MD length is nonzero;
    2. TrainTrn1u;
    3. ReadJa;
    4. within 500 ms, Sd and Sd-bar, then TRN1d for at least 2040T, with Jd no later than 4 s after TRN1d starts;
    5. SendJd, hunting Su (DR2: 5.1 s + RTD);
    6. MeasurePhase;
    7. SendJp;
    8. Jp' and circuit 107 at the second Su→Su-bar;
    9. DilOrScr: the frame origin is the first symbol of the second TRN1u, and the receiver is refined;
    10. AwaitCpt or AwaitE1u, accepting both orders (P3P 13.3);
    11. Ri and Ri-bar;
    12. `Phase3Done`, a placeholder.
  - **Deadlines:** DR1 (4.5 s + RTD from the end of INFO1a) and the start-up watchdog.
  - **Tone A** requests a retrain.
  - **`Habits::LIVE_V92`:** a long TRN1d, and no RTD in the Su wait.
- **Tests** (a scripted analogue modem, from `UpSource` over `Network`):
  - `the_digital_modem_answers_a_scripted_phase_3_in_order`: Sd within 500 ms of the descriptor; Jd within 4 s; Jp's ε
    matches the network phase; Jp' after the second Su-bar; the DIL as described; Ri on CPt; Ri-bar on E1u.
  - `with_no_dil_asked_scr_runs_until_trained_then_ri_then_ri_bar_on_cpt`.
  - `no_ja_within_4_5_s_and_a_round_trip_asks_for_a_retrain`.
  - `no_su_within_5_1_s_and_a_round_trip_asks_for_a_retrain`.
  - `a_non_zero_md_length_is_waited_out_before_ru_is_hunted_again`.
  - `the_upstream_frame_count_starts_at_the_second_trn1u`.
  - `sd_lands_on_interval_0_of_a_grid_it_was_given`.

#### WP-28: Short Phase 2 recovery (M)

- **Depends on:** WP-23.
- **Clauses:** 9.4.1.2.1-9.4.1.2.3; 9.4.2.2.1-9.4.2.2.2; 9.7.1.x; 9.7.2.1; V.90 9.2.1.1.3 (entry point).
- **Files:** `crates/datapump/src/v34/phase2.rs`.
- **Digests:** P2P (6.3, 6.5, 6.6, 7, 11.2, 12); P4P (7).
- **What to build:**
  - The INFO0 repeat and bit-28 recovery on the short path.
  - SDR-2 and SDR-3: into full Phase 2 at FD-3, without INFO0.
  - SAR-2: a 9.7.2.1 retrain.
  - The A-reversal detector is armed only at SD-3 (P2P A7).
  - A tone onset is not a reversal (P7).
  - The 2500 ms timer runs from the last INFO0a (P2P A9).
- **Tests:**
  - `a_lost_b_reversal_makes_the_analogue_modem_retrain_into_full_phase_2`.
  - `a_lost_a_reversal_drops_the_digital_modem_into_full_phase_2`.
  - `a_corrupted_info1a_leads_both_into_full_phase_2`.
  - `an_info0_lost_each_way_is_recovered_and_short_phase_2_still_agreed`.
  - `round_trips_up_to_2_3_s_use_short_phase_2_and_longer_ones_fall_back_cleanly`: three points by default; the full
    0-2.6 s sweep is ignored.
  - `tone_a_after_retrain_silence_is_not_taken_for_the_reversal`.

#### WP-29: Upstream channel estimate and filter design (M)

- **Depends on:** WP-11, WP-12, WP-18.
- **Clauses:** 8.8.3 (limits, the design rule, the NOTE); 8.7.6 (TRN2u may be used); Table 18 bits 12:17; 6.4.2.
- **Files:** `crates/datapump/src/v92/design.rs`.
- **Digests:** CD (7.6, 7.8); P4D (5.1, 5.4, 13.3); INTRO (6.4.2).
- **What to build:**
  - **`Channel::estimate`**: T-spaced least squares from known TRN2u (reset scrambler, known seed), or from TRN1u,
    with a noise floor.
  - **`design(channel, noise, limits) -> Filters`**, as an MMSE-DFE:
    - the feed-forward goes to z2, and to p2 if allowed;
    - the feedback goes to p1, and to z1 if allowed;
    - coefficients are quantised to Q0.15 and Q1.14, with a range check;
    - the design respects the sections, Ltot and Lmax;
    - it reports the predicted residual ISI.
  - **Verification** through the WP-11 model.
- **Tests:**
  - `an_ideal_channel_gets_a_trivial_design`.
  - `the_designed_filters_leave_little_isi_on_a_loop_model`: a test-local loop with a DC high-pass and a 3.4 kHz
    roll-off; residual ISI below -25 dB.
  - `the_coefficients_fit_inside_ltot_and_lmax_for_every_info1a_code`.
  - `z1_and_p2_are_left_out_when_info1a_says_so`.
  - `the_channel_estimate_from_trn2u_matches_the_true_channel`.

### Wave 5: Phase 3 call-to-call, and the Phase 4 building blocks

#### WP-30: Phase 3 between our two modems (M)

- **Depends on:** WP-26, WP-27.
- **Clauses:** 9.5; Figures 10, 11.
- **Files:**
  - `crates/datapump/tests/v92_call.rs` (new)
  - `crates/datapump/src/v92/analogue.rs`
  - `crates/datapump/src/v92/digital.rs`
- **Digests:** CT (2.1, 8.1, 9.2); P3P (9).
- **What to build:**
  - A `settled_v92()` fixture.
  - A `Call` harness with a single `tick()`, run until both modems reach `Phase3Done`.
  - Fix whatever the meeting shows.
- **Tests:**
  - `phase_3_completes_over_a_clean_network`.
  - `phase_3_completes_with_no_dil_asked`.
  - `phase_3_completes_over_a_voip_length_round_trip`: 0.6 s each way.
  - `the_digital_modem_s_epsilon_matches_the_network_s_sampling_phase`: phases 0.25 and 0.5.
  - `phase_3_completes_on_an_a_law_network_and_with_a_robbed_bit_downstream`.

#### WP-31: Upstream constellations, moduli, rate and gain, assembled into CPd (M)

- **Depends on:** WP-09, WP-18, WP-25, WP-29.
- **Clauses:** 8.8.3 (Table 30); 6.4.1 (2^K ≤ M); 6.4.2 (N ≥ M, 2M); Table 20 (mask); Table 18 (limits); 9.11.
- **Files:** `crates/datapump/src/v92/upchoice.rs`.
- **Digests:** CD (7.6); P4D (5, 13.3); INTRO (6.1, 6.4).
- **What to build:** `choose(channel, noise, filters, ja_mask, limits, law, robbed_j) -> Option<Cpd>`.
  - Per-j sets come from the codec's linear levels, at the spacing the noise allows (the `dil` approach). Point values
    use the section 4 reading.
  - Moduli have room and meet the class constraints.
  - drn is the highest rate the Ja mask enables that meets 2^K ≤ ΠM with room.
  - G comes from the simulated mean square: 4G in Q0.16, never 0.
  - Trellis is 16-state by default, and configurable.
  - Every part is present.
  - Before it is returned, the CPd must decode N frames error-free (`UpEncoder` → channel → `UpDecoder`).
  - `cleardown()` gives a CPd with drn 0.
- **Tests:**
  - `a_chosen_cpd_meets_every_rule_of_8_8_3`.
  - `more_noise_means_a_lower_upstream_rate`.
  - `a_robbed_bit_interval_gets_a_sparser_set`.
  - `the_rate_is_one_the_ja_mask_enables`.
  - `a_chosen_cpd_decodes_a_thousand_frames_through_its_channel`.
  - `g_is_quantised_to_q0_16_and_never_zero`.

#### WP-32: The upstream source for Phase 4, RR and FPE (M)

- **Depends on:** WP-09, WP-12, WP-18, WP-21.
- **Clauses:** 8.7.1-8.7.7; 9.6.2.1.1-9.6.2.1.5; 9.8.2.1.1-9.8.2.1.6; 9.8.2.2.2-9.8.2.2.3; 9.9.2.1.1-9.9.2.1.2;
  9.9.2.2.2-9.9.2.2.3; 8.5.5.
- **Files:** `crates/datapump/src/v92/up_source.rs`.
- **Digests:** P4A (2, 3); RRF (2.4, 2.5, 2.7, 2.9, 2.11, 2.12, 2.16, 2.17); P4P (4).
- **What to build:** add these segments.
  - **Trn2u:** the size comes from Jp bit 48 or 49, by context; the scrambler is reset; the sign seed follows the
    section 4 reading.
  - **Suv, Cp and Cpus**, in either of two modulations:
    - TRN2u modulation, padded to 24 or 36 bits;
    - data-mode modulation for FPE, padded to K through `UpEncoder`.
  - **E2u**, with its optional 13th symbol.
  - **B1u:** a new `UpEncoder`, and the frame counter reset.
  - **Data.**
  - **Fb1u:** the old encoder, 48 frames.
  - **Rm and RmPrime:** the old encoder with `force_k`, then the FPE reset.
  - **Ru and RuBar for RR**, bypassing the chain.
  - **`change_at_frame_boundary`.**
- **Tests:**
  - `trn2u_after_e1u_descrambles_to_ones_and_has_lu_squared_power`.
  - `suvu_in_trn2u_modulation_is_72_bits_at_either_size`.
  - `an_extended_e2u_moves_b1u_s_interval_0_by_one_symbol`.
  - `b1u_starts_interval_0_with_every_memory_zero`.
  - `rm_and_rm_prime_decode_to_their_k_patterns_through_the_old_chain`.
  - `an_fpe_suvu_decodes_after_the_reset_that_follows_rm_prime`.
  - `fb1u_uses_the_old_parameters_and_b1u_the_new_49_frames_after_e2u_began`.
  - `ru_for_a_renegotiation_starts_on_a_frame_boundary`.

#### WP-33: The downstream source for Phase 4, RR and FPE (M)

- **Depends on:** WP-03, WP-09, WP-22.
- **Clauses:**
  - 8.8.1-8.8.6;
  - V.90 8.6 (intro), 8.6.1, 8.6.2, 8.6.4, 8.6.5, and V.90 Table 17;
  - 9.6.1.1.1-9.6.1.1.5; 9.8.1.1.1-9.8.1.1.5; 9.8.1.2.2; 9.9.1.1.1-9.9.1.1.2; 9.9.1.2.2-9.9.1.2.3.
- **Files:** `crates/datapump/src/v92/down_source.rs`.
- **Digests:** P4D (2-4, 6, 8, 9, 13); RRF (2.1-2.3, 2.6, 2.13, 2.17); CD (3.2, 8.2).
- **What to build:** add these segments.
  - **Trn2d:** the V.90 `Encoder`, with a `Mapping` built from the V.92 CPt, after the resets.
  - **Suvd and Cpd**, padded to D bits, where D is:
    - in training, from CPt;
    - in RR, K from CPt plus the data-mode S (section 4 reading);
    - in FPE, the data-mode D.
  - **Ed:** two frames of scrambled zeros.
  - **B1d:** the CPu mapping, after the resets. Its first frame is the same for every look-ahead.
  - **Data.**
  - **Rd and Rd-bar:** the loudest Ucode per interval of the CPu transmit constellation.
  - **Rt and Rt-bar:** the CPt peaks.
  - **Rf and Rf-bar:** `++--` over 12 symbols, counted from Rf's start; 384 and 24 symbols.
  - **Quiet.**
  - **The FPE reset before SUVd.**
  - **After Rt-bar,** SUVd's differential reference is "+".
- **Tests:**
  - `trn2d_uses_the_cpt_constellations_and_starts_from_zero_state`.
  - `an_ed_follows_a_complete_sequence_as_two_frames_of_scrambled_zeros`.
  - `suvd_and_cpd_pad_to_whole_frames_of_d_bits`.
  - `rd_uses_the_loudest_code_of_each_interval_s_data_constellation`.
  - `rf_is_plus_plus_minus_minus_for_384_symbols_then_24_inverted`.
  - `the_first_suvd_after_rt_bar_takes_plus_as_its_reference`.
  - `b1d_s_first_frame_is_the_same_for_every_look_ahead`.
  - `quiet_keeps_the_frame_count`.

### Wave 6: Phase 4 state machines

#### WP-34: The analogue modem, Phase 4 and data mode (L)

- **Depends on:** WP-09, WP-14, WP-30, WP-32.
- **Clauses:** 9.6.2.1.1-9.6.2.1.6; 9.6.2.2; 9.6.2.2.1; 8.7; 8.8 (reception); 9.11; Table 1 (circuits 104, 106, 109).
- **Files:** `crates/datapump/src/v92/analogue.rs`.
- **Digests:** P4P (5, 6.3-6.5, 9); P4A (5.1, 8); CA (6, 11.5); P4D (2.6).
- **What to build:**
  - **Upstream sequence:**
    - TRN2u for at least 12000T, or until an SUVd has arrived and we are ready;
    - the Exchange, in Training context;
    - CPu: the V.92 header on the `dil::choose` result, sent once and repeated per the rule;
    - E2u, honouring CPd bit 29;
    - B1u.
  - **CPd:** the first must carry every part. It is parsed into `UpParams`.
  - **Downstream frames:**
    - a `Mapping` built from the V.92 CPt;
    - the SUVd/CPd finder;
    - Ed, then B1d with the CPu mapping, then data.
  - **Circuits**, and `Connected { downstream, upstream }`.
  - **A far drn of 0** clears the call down.
  - **Data:** `take_bits` and `send_bits`.
  - **SUVu's level field** is 16 in training.
  - **Phase names:** "V.92 phase 4" and "V.92 data".
- **Tests** (a scripted digital Phase 4, from `DownSource`, with a CPd from WP-31 for an ideal channel):
  - `the_analogue_modem_connects_against_a_scripted_phase_4`.
  - `a_lost_cpd_acknowledgement_makes_the_cpu_repeat`.
  - `an_extension_asked_for_in_cpd_bit_29_is_sent`.
  - `a_cpd_with_drn_0_clears_the_call_down`.
  - `a_first_cpd_missing_a_part_asks_for_a_retrain`.
  - `no_b1d_within_20_s_and_six_round_trips_asks_for_a_retrain`.

#### WP-35: The digital modem, Phase 4 and data mode (L)

- **Depends on:** WP-14, WP-19, WP-25, WP-29, WP-30, WP-31, WP-33.
- **Clauses:** 9.6.1.1.1-9.6.1.1.6; 9.6.1.2; 9.6.1.2.1; 8.8; 8.7 (reception); 9.11.
- **Files:** `crates/datapump/src/v92/digital.rs`.
- **Digests:** P4P (5, 6.1, 6.2, 6.5); P4D (10.1, 13); CD (7, 8.3).
- **What to build:**
  - **Before the exchange:**
    - TRN2d for at least 2040T;
    - read TRN2u at the Jp bit 48 size;
    - build the channel estimate, run the design and the choice;
    - once ready, send SUVd.
  - **The exchange,** with the wait-for-CPu policy: one CPd, repeats per the rule, then Ed.
  - **B1d**, with the CPu mapping.
  - **E2u**, with or without the extra symbol. Our policy is off; a test turns it on.
  - **B1u**, checked against the `UpEncoder` model.
  - **Data:** `UpDecoder` feeds `take_bits`.
  - **`Connected`**, and the start-up watchdog.
  - **A far CPu drn of 0** clears the call down.
  - **`sent_cpd()`**, for tests.
- **Tests** (a reactive test double, built from `UpSource`, that reads `sent_cpd()`):
  - `the_digital_modem_connects_against_a_scripted_phase_4`.
  - `a_lost_cpu_is_answered_with_repeated_cpd_prime_as_in_figure_14`.
  - `asking_for_an_e2u_extension_moves_b1u_by_a_symbol`.
  - `b1u_is_checked_against_the_cpd_that_was_sent`.
  - `a_cpu_with_drn_0_clears_the_call_down`.
  - `no_b1u_within_20_s_and_six_round_trips_asks_for_a_retrain`.

### Wave 7

#### WP-36: PCM both ways between our two modems (M)

- **Depends on:** WP-34, WP-35.
- **Clauses:** 1 (c-e); 6; 9.5; 9.6.
- **Files:**
  - `crates/datapump/tests/v92_call.rs`
  - `crates/datapump/src/v92/analogue.rs`
  - `crates/datapump/src/v92/digital.rs`
- **Digests:** CT (8.1, 9.2, 9.6); CD (9.2).
- **Tests:**
  - `phases_3_and_4_connect_with_pcm_both_ways_over_a_clean_network`: down ≥ 48 000; up on the ladder and ≥ 24 000;
    the reached rate is printed and recorded.
  - `data_crosses_both_ways_at_pcm_rates`.
  - `a_voip_length_round_trip_connects_with_pcm_upstream`: 0.6 s each way; the start-up watchdog never fires.
  - `an_a_law_network_connects_with_pcm_both_ways`.
  - `every_upstream_sampling_phase_is_found_and_corrected`: 4 phases; the 8-phase sweep is ignored.
  - `a_sound_card_clock_120_ppm_off_is_followed_upstream_too`.
  - `the_upstream_gain_at_the_codec_is_measured_and_used`: gains 0.25, 0.5 and 1.0.
  - `the_digital_modem_can_ask_for_the_32_and_64_state_codes`.
  - All V.90 tests still pass.

### Wave 8

#### WP-37: Start-up hand-over, V.90 interop and the fallback ladder (M)

- **Depends on:** WP-16, WP-36.
- **Clauses:** 9.3; 9.3.1; 9.7; 8.4.1; Table 18; V.90 9.2.1.1.8 and 9.2.2.1.9; 9.6.1.2.1; 9.6.2.2.1.
- **Files:**
  - `crates/datapump/src/v90/startup.rs`
  - `crates/datapump/src/v90/server.rs`
  - `crates/datapump/tests/v92_call.rs`
- **Digests:** CA (2, 9.2, 11.2, 11.9); CD (2); CT (4.1, 8.2); P2P (7).
- **What to build:**
  - **`V92Options`**, and the constructors `Analogue::with_v92(fs, opts)`, `Digital::with_v92(info0d, opts)` and
    `server::ours_v92()`. `new()` stays V.90-only (AD-2).
  - **`enum Pcm { V90, V92 }`** inside the wrappers, handed over by INFO1a table.
  - **The ladder:**
    1. `V92_RETRAINS` failed PCM-upstream start-ups, then `decline_pcm_upstream()`;
    2. `V90_RETRAINS` more, then `decline_pcm()`.
  - **Pass-throughs:** `is_v92()`, `upstream_pcm()`, `pair()`, `points()`, `lapm_bypass_allowed()`, and the
    `last_failure()` strings.
- **Tests** (`FullCall`):
  - `a_whole_v92_start_up_from_phase_2_connects`.
  - `a_v92_analogue_modem_meets_a_v90_server_on_v90`.
  - `a_v90_analogue_modem_meets_a_v92_server_on_v90`.
  - `a_v92_server_that_says_no_pcm_upstream_gets_v90_mode`.
  - `an_upstream_that_will_not_carry_24000_steps_down_to_v34_upstream_once`.
  - `a_retrain_from_either_end_comes_back_up_as_v92`.
  - `a_v92_modem_told_not_to_use_pcm_upstream_asks_for_v90_mode`.
  - Every `FullCall` test in `v90_call.rs` passes unchanged.

### Wave 9

#### WP-38: Short Phase 2 in a whole start-up, with the recognised-connection memo (M)

- **Depends on:** WP-28, WP-37.
- **Clauses:** 1 j; 9.4; Table 19; 9.4.1.1.5 and 9.4.2.1.4 ("the appropriate Phase 3"); V.90 9.3; V.90 6.2;
  9.5.2.1.2; 9.6.2.2.1.
- **Files:**
  - `crates/datapump/src/v92/memo.rs`
  - `crates/datapump/src/v90/startup.rs`
  - `crates/datapump/src/v90/analogue.rs`
  - `crates/datapump/src/v90/digital.rs`
  - `crates/datapump/tests/v92_call.rs`
- **Digests:** P2P (6, 11, 12 A3/A5/A6/A10/A11); P2S (16.3); CA (11.2, 12); CD (6.3); CV (6.2.4).
- **What to build:**
  - **`Recognized`,** with a text round trip for the GUI file later, and `Analogue::learned()`.
  - **The request.** `with_v92(.., memo)` builds a `ShortPlan` only when a memo exists:
    - Table 18 when the memo says PCM upstream works;
    - otherwise Table 19.
  - **The V.90 analogue `Settings::new`** accepts an optional INFO1d. Without one it uses the INFO1a symbol rate, the
    Table 19 carrier and pre-emphasis 0, and sets `v34_receive` to 0.
  - **The V.90 digital `Settings`** uses INFO1a bit 33 when there is no INFO1d.
  - **RTD after a short Phase 2** comes from the memo (AD-8).
- **Tests:**
  - `a_second_call_with_what_the_first_taught_uses_short_phase_2_and_connects_v92`.
  - `short_phase_2_with_a_table_19_info1a_connects_in_v90_mode`.
  - `without_a_memo_the_analogue_modem_does_not_ask_for_short_phase_2`.
  - `a_v92_server_that_does_not_ask_gets_full_phase_2_even_with_a_memo`.
  - `short_phase_2_over_a_1_5_s_round_trip_connects`.
  - `a_memo_round_trips_through_its_text_form`.

#### WP-39: The digital modem's echo canceller (M)

- **Depends on:** WP-17, WP-36.
- **Clauses:** 1 b; 9.5 (its title names echo canceller training); 9.8 (the silent period).
- **Files:**
  - `crates/datapump/src/v92/receiver.rs`
  - `crates/datapump/src/v92/digital.rs`
  - `crates/datapump/tests/v92_impairments.rs` (new)
- **Digests:** CD (5.4, 7.5); CT (7, 10); CA (13).
- **What to build:** `dsp::EchoCanceller` at 8 kHz on the A/D samples.
  - The reference is our downstream levels.
  - The near run has 32 to 64 taps.
  - It adapts while the analogue modem is silent (Sd to Jd) and then holds.
  - It adapts slowly, decision-directed, in data mode.
  - An optional far run is placed within RTD by `EchoFinder`.
- **Tests:**
  - `the_hybrid_s_echo_is_cancelled`: 15 dB and 25 dB; the upstream rate is within one rung of the echo-free rate.
  - `a_far_echo_a_round_trip_late_is_cancelled`.
  - `without_echo_the_canceller_changes_nothing`.

#### WP-40: V.92 replay probe (S)

- **Depends on:** WP-37.
- **Clauses:** none new.
- **Files:** `crates/datapump/tests/v92_replay.rs` (new).
- **Digests:** CT (2.3, 8.4).
- **What to build:** ignored probes, with their commands in the module doc.
  - `probe_replay_v92`: channel 0 through `Analogue::with_v92`, printing the phases, ε, Jp and CPd.
  - `what_we_sent_is_what_we_meant`: channel 1 against the transmitter's intended 8 kHz levels.
- **Tests:** both are `#[ignore]`d and compile under clippy.

### Wave 10

#### WP-41: Upstream robustness: slips and digital impairments (M)

- **Depends on:** WP-39.
- **Clauses:** 8.5.7; 9.8 (R-COM-1); 6.4.1.
- **Files:**
  - `crates/datapump/src/v92/receiver.rs`
  - `crates/datapump/src/v92/decoder.rs`
  - `crates/datapump/src/v92/digital.rs`
  - `crates/datapump/tests/v92_impairments.rs`
- **Digests:** CT (7, 9.3, 10); CD (9.2, 11); CA (8).
- **What to build:**
  - **Re-framing after upstream slips:**
    - count impossible frames, using the moduli's room;
    - search ±4 and ±8 symbols, then every shift, over recent symbols;
    - resynchronise the decoder, or give up to a retrain.
  - **Lost/Found hold**, and medians in every estimator.
  - **The digital start-up fails quickly, with a reason,** when upstream PCM never trains, so the ladder steps down.
- **Tests:**
  - `upstream_slips_during_the_start_up_are_followed`: the five period/insert pairs of `v90_call.rs`.
  - `an_upstream_slip_in_data_mode_is_followed_and_data_after_it_arrives`.
  - `ten_millisecond_cuts_either_way_are_followed`.
  - `robbed_bits_both_ways_connect_and_carry_data`.
  - `a_digital_pad_either_way_connects`.
  - `a_transcoding_gateway_leaves_pcm_upstream_out_without_a_retrain_loop`.
  - `a_gain_control_below_our_quietest_useful_level_drops_pcm_upstream_once`.
  - `a_softphone_that_passes_our_samples_straight_through_gives_pcm_upstream`.
  - `a_clock_off_through_a_softphone_is_slips_not_drift`.
  - Ignored sweep: `a_slip_anywhere_in_trn1u_is_found`.

#### WP-42: Short Phase 1, the line signalling (M)

- **Depends on:** WP-06.
- **Clauses:** 9.2; 9.2.1.1; 9.2.1.4; 9.2.2.1; 9.2.3.1; 9.2.3.4; 9.2.5; Figures 3, 4, 7; V.8 8.1, 8.2; V.21.
- **Files:** `crates/datapump/src/v8.rs`; `crates/datapump/src/bell103.rs`.
- **Digests:** P1P (5, 6, 10); P1A (9, 11); CV (2.2, 6.2.2); CTL (3.4).
- **What to build:**
  - **Builders:** `offering_quick(qc)` for the caller, analogue or digital, and `answering_quick(role, field)`.
  - **Caller:**
    - after 1 s of ANSam, queue the QC bits and then CM, and run `BitWatcher` on V.21(H) beside JM;
    - on a QCA of the other role: `Bell103Tx::abandon()` mid-octet, silence, then `Done(Agreed(V34Duplex))`, with
      `quick()` returning the role, both P bits ANDed, the U_QTS and the LM;
    - on JM: V.8;
    - a digital caller that still hears ANSam 1 s after QC1d goes to V.8 and ignores a late QCA1a.
  - **Caller after QCA1a** (both analogue):
    1. silence until ANSam;
    2. TONEq, as Bell103Tx idle mark at 980 Hz;
    3. when ANSam is lost, 75 ms of silence;
    4. `quick()` = V.34, call role.
  - **Answerer:**
    - while sending ANSam, watch V.21(L) for QC1a and QC1d, and hold CM parsing until decided;
    - then QCA, silence, and `quick()`.
  - **Analogue answerer on QC1a** (implements the MAY of 9.2.3.1):
    1. QCA1a;
    2. 75 ms of silence;
    3. ANSam, listening for TONEq and CM;
    4. TONEq, then 75 ms, then `quick()` = V.34, answer role.
  - **For the return to V.8:** `resuming_answer()` starts straight in ANSam; `resuming_call()` starts in Waiting.
  - **No new `Status` variants** (AD-3).
- **Tests** (two V.8 modems over a delay line):
  - `an_analogue_caller_and_a_digital_answerer_both_report_quick_connect`.
  - `the_lapm_outcome_is_set_only_when_both_p_bits_are`.
  - `cm_is_cut_mid_octet_when_qca1d_arrives`.
  - `a_digital_answerer_that_does_not_do_quick_connect_ignores_qc1a_and_v8_completes`.
  - `a_caller_that_hears_jm_after_qc1a_finishes_v8`.
  - `a_digital_caller_and_an_analogue_answerer_both_report_quick_connect`.
  - `two_analogue_modems_go_from_qc1a_through_toneq_to_v34`.
  - `a_resumed_answerer_starts_in_ansam`.
  - The existing V.8 tests pass.

### Wave 11

#### WP-43: Rate renegotiation without a silent period (M)

- **Depends on:** WP-14, WP-25, WP-32, WP-33, WP-41.
- **Clauses:** 9.8; 9.8.1.1.1-9.8.1.1.2; 9.8.1.2.1-9.8.1.2.3; 9.8.2.1.1-9.8.2.1.3; 9.8.2.2.1-9.8.2.2.3; Figure 15;
  8.8.4; 8.5.5; 8.7.3 (CPus); R-COM-1..4.
- **Files:**
  - `crates/datapump/src/v92/analogue.rs`
  - `crates/datapump/src/v92/digital.rs`
  - `crates/datapump/tests/v92_renegotiation.rs` (new)
- **Digests:** RRF (1-3, 5, 7, 9, 10); P4P (2.3); CA (9.1, 11.6); CD (3.3).
- **What to build:**
  - **Data-mode watches:**
    - analogue: Rd (`RWatch` at the CPu peaks) and Tone B;
    - digital: an Ru hunt on the A/D samples, and Tone A.
  - **Initiators and responders,** each starting on its own frame boundary, with 106 OFF and 104 clamped.
  - **TRN2d** for 2040T to 16008T.
  - **TRN2u** at the Jp bit 49 size, running until SUVd or 16008T. It does not stop early on a long RTD.
  - **The Exchange,** in Renegotiation context: CPus when the downstream parameters are unchanged; D in RR per the
    section 4 reading.
  - **B1 with the new parameters.**
  - **API:** `renegotiate(..)` on both modems.
  - **The local watchdog,** which leads to a retrain.
- **Tests:**
  - `a_rate_renegotiation_from_either_end_settles_the_rates_asked_for`: 0.02 s and 0.6 s each way.
  - `two_renegotiations_begun_together_converge`.
  - `a_renegotiation_that_changes_only_the_rate_sends_cpus`.
  - `a_line_gone_noisy_upstream_is_renegotiated_down_by_the_digital_modem`.
  - `a_stalled_renegotiation_turns_into_a_retrain`.
  - `data_crosses_after_a_renegotiation_without_a_retrain`.

#### WP-44: Short Phase 1, the PCM side in the start-up wrappers (M)

- **Depends on:** WP-13, WP-22, WP-38, WP-42.
- **Clauses:** 8.3.1; 8.3.6; 8.2.5; 9.2.1.3; 9.2.2.3; 9.2.3.3; 9.2.4.3; Figures 3-6; 9.2.5.
- **Files:**
  - `crates/datapump/src/v90/startup.rs`
  - `crates/datapump/src/v90/server.rs`
  - `crates/datapump/tests/v92_quick.rs` (new)
- **Digests:** P1P (4, 5, 6, 10.1 P2-P5, P14, P15); P1D (10, pitfalls 12-16); CV (6.2.3, R4, R22).
- **What to build:**
  - **`Digital::after_quick_connect(info0d, uqts, lm, opts)`:**
    1. 75 ms of Ucode 0;
    2. QTS, QTS-bar and ANSpcm, from `DownSource`. The frame origin is the first QTS symbol, counted in the 8 kHz domain
       across Phase 2 and passed to `v92::digital` as `frame_origin`. `server::Line` keeps the 8 kHz count, not line
       samples;
    3. the TONEq detector is armed once ANSpcm has started;
    4. on TONEq: stop ANSpcm, 75 ms of silence, then Phase 2.
    - With no TONEq: 2 s after QCA1d (answering), or 2 s + RTD (calling, per section 4), `take_back_to_v8()`.
  - **`Analogue::after_quick_connect(fs, uqts, opts, memo, ansam_seen)`:**
    1. hunt QTS and its reversal; detect ANSpcm;
    2. TONEq at once where the MAY allows it, else after 1 s; at least 50 ms;
    3. ANSpcm lost (detected within 30 ms): TONEq off, 75 ms of silence, then Phase 2.
    - If ANSam (with its AM) returns: `take_back_to_v8()`.
    - An analogue answerer with no ANSpcm 2 s after QCA1a also goes back to V.8.
  - **Status is unchanged** (AD-3).
- **Tests** (V.8 modems hand over to the pumps over `Network`):
  - `quick_connect_reaches_phase_2_and_connects_v92`.
  - `the_frame_grid_begun_at_qts_carries_to_sd`.
  - `a_digital_answerer_that_never_hears_toneq_asks_to_go_back_to_v8_within_2_s`.
  - `an_analogue_caller_that_hears_ansam_come_back_asks_to_go_back_to_v8`.
  - `with_0_75_s_each_way_immediate_toneq_beats_the_2_s_window`.
  - `toneq_stops_within_30_ms_of_anspcm_so_info0d_is_heard`.

### Wave 12

#### WP-45: Rate renegotiation with a silent period (M)

- **Depends on:** WP-43.
- **Clauses:** 9.8.1.1.2-9.8.1.1.5; 9.8.2.1.3-9.8.2.1.6; Figures 16-18; Tables 27 and 31 (bits 32, 33); 8.8.4 (Rt);
  9.8.1.1.3.
- **Files:**
  - `crates/datapump/src/v92/analogue.rs`
  - `crates/datapump/src/v92/digital.rs`
  - `crates/datapump/tests/v92_renegotiation.rs`
- **Digests:** RRF (2.2, 2.13, 3.5, 3.6, 10.2 items 8-9, 10.4); P4D (7.3, 9).
- **Tests:**
  - `a_digital_modem_s_silent_period_held_to_the_cap_as_in_figure_16`.
  - `a_digital_modem_s_silent_period_ended_early_by_rt_as_in_figure_17`.
  - `an_analogue_modem_s_silent_period_as_in_figure_18`.
  - `both_asking_for_silence_settles_after_8004t`.
  - `stale_primed_suvu_does_not_start_rt`.
  - `rt_and_the_8004t_timer_crossing_on_a_1_5_s_line_do_not_send_rt_twice`.

#### WP-46: `+MS=V92` from AT command to CONNECT (M)

- **Depends on:** WP-37.
- **Clauses:** V.250 6.4.1 (Table 13) and 6.4.3; V.92 9.1 NOTE; V.8 Tables 4, 5, 7.
- **Files:**
  - `crates/at/src/lib.rs`
  - `crates/at/tests/session.rs`
  - `crates/modem/src/lib.rs`
  - `crates/modem/tests/v92_call.rs` (new)
- **Digests:** CV (2, 3, 6.1, 8); CTL (5.2); CT (8.3, 9.4).
- **What to build:**
  - **AT:**
    - add `V92` to the `+MS` whitelist;
    - parse and keep the 5th and 6th `+MS` subparameters; a 7th is an error;
    - fix the `+MS=?` text.
  - **Modem crate:**
    - `offered()`, `place_call()` and `start_pump()` map V92 to the `with_v92` pumps, with V.90's menus;
    - `standard()` says "V.92", and the idle arms cover "V90" and "V92";
    - `transmit_rate()`;
    - `distant()` rows: PCM upstream and ε;
    - the fallback carrier is V32B.
- **Tests:**
  - In `session.rs`:
    - `ms_v92_is_accepted_and_read_back`;
    - `the_receive_rate_subparameters_are_kept`;
    - `a_seventh_ms_subparameter_is_an_error`;
    - update `a_modulation_that_is_not_offered_is_refused`.
  - In `modem/tests/v92_call.rs`:
    - `dialling_a_v92_server_connects_with_pcm_both_ways_and_carries_text`: with a server retrain;
    - `two_of_these_connect_with_pcm_both_ways_when_both_softphones_pass_codewords`: the 2 × 2 phase matrix, with
      outcomes recorded;
    - `ms_v90_still_gets_v90_from_a_v92_server`;
    - `ms_v92_is_reported_in_the_standard_and_rates`.

### Wave 13

#### WP-47: Fast parameter exchange (M)

- **Depends on:** WP-25, WP-45.
- **Clauses:** 9.9; 9.9.1.1-9.9.2.2; Figure 19; 8.7.4; 8.7.7; 8.8.4 (Rf); 9.6.1.1.6; 9.6.2.1.5.
- **Files:**
  - `crates/datapump/src/v92/analogue.rs`
  - `crates/datapump/src/v92/digital.rs`
  - `crates/datapump/tests/v92_renegotiation.rs`
- **Digests:** RRF (1.2, 1.4, 2.3, 2.5, 2.15, 2.18, 4, 9, 10); P4A (3.5, 3.8, 5.3); P4D (6.2, 10.3).
- **What to build:**
  - An `RfWatch` on the analogue side: polarity-blind, on the 4-symbol sign cycle.
  - The `RmWatch` wired in on the digital side.
  - FPE initiators and responders, with their resets.
  - Data-mode modulation for SUV, CP and E in both directions.
  - FB1u.
  - Precedence: RR wins over FPE.
  - The `fast_exchange()` API. The margin watch prefers FPE for rate-only changes.
- **Tests:**
  - `a_fast_parameter_exchange_from_either_end_changes_the_rate_without_losing_the_frames`.
  - `a_renegotiation_wins_over_a_fast_parameter_exchange_begun_at_the_same_time`: both orders.
  - `two_fast_exchanges_begun_together_converge`.
  - `the_first_suv_after_the_reset_is_read`.
  - `fb1u_then_b1u_switch_parameters_on_the_right_frame`.

#### WP-48: Quick connect and the ODP/ADP bypass in the modem crate (M)

- **Depends on:** WP-07, WP-42, WP-44, WP-46.
- **Clauses:** 9.2; 9.2.5; 9.3.1; 1 j; V.42 7.2.1.
- **Files:** `crates/modem/src/lib.rs`; `crates/modem/tests/v92_call.rs`.
- **Digests:** CV (6.2.3, 6.2.4, R5, R7, R16); CTL (5.3, 6.2); P1P (5.5).
- **What to build:**
  - **Start.** `negotiate` sees `quick()` and calls `start_pump_quick`, which starts:
    - the `after_quick_connect` pumps; or
    - a V.34 pump in the right role, after the both-analogue path.
  - **The return to V.8.** An internal `Progress::BackToV8`, raised by `take_back_to_v8()`, rebuilds the negotiation
    with the resuming constructors.
  - **The memo:** `quick_memo` and `learned()`. QC is sent only with `+MS=V92` and a memo. The U_QTS and LM defaults
    are in section 4.
  - **The bypass:**
    - after quick connect with both P bits set; or
    - with both prot0 LAPM and `lapm_bypass_allowed()`.

    The originator uses `without_detection`, the answerer `bypassing_detection`.
- **Tests:**
  - `a_second_call_with_the_memo_connects_by_quick_connect_and_faster`.
  - `no_odp_or_adp_crosses_when_both_asked_for_lapm`.
  - `a_v90_answerer_ignores_qc1a_and_the_call_still_connects`.
  - `a_server_that_never_hears_toneq_is_still_reached_by_v8`.
  - `quick_connect_over_a_1_5_s_round_trip`.

### Wave 14

#### WP-49: Cleardown (S)

- **Depends on:** WP-47.
- **Clauses:** 9.11; Tables 23, 24 and 30 (drn); V.90 9.7 NOTE.
- **Files:**
  - `crates/datapump/src/v92/analogue.rs`
  - `crates/datapump/src/v92/digital.rs`
  - `crates/datapump/src/v90/startup.rs`
  - `crates/datapump/tests/v92_renegotiation.rs`
- **Digests:** RRF (6.2, E-1); MOH (3); P4D (10.4).
- **What to build:**
  - `clear_down()`: drn = 0 by FPE (preferred) or RR.
  - On receiving drn = 0: finish the exchange, ignore that CP's constellations, and report `ClearedDown`.
  - The wrapper passes `clear_down()` through for V.92.
- **Tests:**
  - `a_cleardown_from_either_end_ends_the_call_at_both`: via FPE and via RR.
  - `a_cleardown_in_phase_4_is_honoured`.
  - `the_constellations_of_a_drn_0_cp_are_ignored`.
  - `clear_down_is_passed_through_the_start_up_wrapper_for_v92`.

#### WP-50: GUI, the V.92 carrier (S)

- **Depends on:** WP-46.
- **Clauses:** V.250 6.4.1.
- **Files:** `crates/gui/src/app.rs`; `crates/gui/src/answer.rs`.
- **Digests:** CV (3.7, 6.7, R14, Q11).
- **What to build:**
  - Append `V92` to `CARRIERS` at index 6.
  - Its `rates` arm: the V.34 list plus the PCM-upstream rates, rounded down to whole bit/s.
  - A `CEILINGS` entry.
  - Retrain enabled for V.90 and V.92.
  - `answer.rs` help mentions `--carrier V92`.
- **Tests:**
  - `the_rate_lists_belong_to_the_carriers_they_are_indexed_by`, updated to 7 carriers.
  - `v92_is_offered_after_v90_and_remembered_by_name`.
  - `v32bis_can_do_everything_v32_can` still passes.

### Wave 15

#### WP-51: Modem-on-hold in the data pumps, PCM-upstream connections (L)

- **Depends on:** WP-24, WP-49.
- **Clauses:** 9.10; 9.10.1.1 (the circuit-107 condition and the retrain race); 8.9.1; 9.7; Table 1.
- **Files:**
  - `crates/datapump/src/v34/phase2.rs`
  - `crates/datapump/src/v34/startup.rs`
  - `crates/datapump/src/v90/startup.rs`
  - `crates/datapump/src/v92/analogue.rs`
  - `crates/datapump/src/v92/digital.rs`
  - `crates/datapump/src/v92/hold.rs`
  - `crates/datapump/tests/v92_hold.rs` (new)
- **Digests:** MOH (2, 9); CA (11.8); CV (6.3.2); P4P (7.6).
- **What to build:**
  - **Requesting.** `request_hold(kind)` on the V.92 modems works in data mode, after circuit 107, at a frame boundary,
    with 106 OFF. It starts the transaction.
  - **Responding.** A tone watch in data mode leads to Phase 2's retrain response. That response listens for MH
    (dpsk `with_mh`) in its tone-awaiting and ranging stages, then `phase2.take_hold()`, and the wrapper runs the
    transaction as responder.
  - **`hold_event()`:** `Granted(t1)`, `Refused`, `Holding(t1)`, `Away`, `Cleared(reason)`, `FastReconnect{..}`,
    `GaveUp`.
  - **During hold,** status reads as retraining, and the carrier watch and deadlines are suspended.
  - **The V.90-mode arm** returns false until WP-53.
- **Tests:**
  - `a_call_put_on_hold_is_granted_and_the_held_modem_sends_ansam`: from either end.
  - `a_refused_hold_then_cleardown_ends_both_pumps`.
  - `a_refused_hold_then_fast_reconnect_ends_in_the_call_side_hearing_ansam`.
  - `a_cleardown_request_from_data_mode_ends_the_call`.
  - `a_hold_request_is_not_mistaken_for_a_retrain_and_a_retrain_is_not_mistaken_for_a_hold`.
  - `an_unanswered_hold_request_turns_into_a_retrain`.
  - `hold_is_refused_before_circuit_107`.

#### WP-52: Graceful hang-up in the modem crate (S)

- **Depends on:** WP-48, WP-49.
- **Clauses:** 9.11; V.90 9.7; V.250 6.3.6.
- **Files:**
  - `crates/modem/src/lib.rs`
  - `crates/modem/tests/v92_call.rs`
  - `crates/modem/tests/v90_call.rs`
- **Digests:** CV (2.1, 6.6, R11).
- **What to build:**
  - `Pump::clear_down` for V.34, V.90 and the V.90 server.
  - An internal `Progress::ClearedDown`, distinct from `Failed`.
  - `ATH` becomes graceful, with a bounded wait of 3 s + 2 RTD.
  - "Force hang up" is unchanged.
  - A far-end cleardown ends the call with `NO CARRIER`.
- **Tests:**
  - `when_one_end_hangs_up_the_other_sees_a_cleardown`: V.92 and V.90.
  - `ath_answers_ok_within_the_bounded_wait`.
  - `force_hang_up_still_drops_at_once`.
  - The existing hang-up tests pass.

### Wave 16

#### WP-53: Modem-on-hold on V.90-mode connections between V.92 modems (M)

- **Depends on:** WP-51.
- **Clauses:** 9.10 (a V.92 connection running V.34 upstream); 9.10.1.1 (circuit 107 in V.90 Phase 3).
- **Files:**
  - `crates/datapump/src/v90/analogue.rs`
  - `crates/datapump/src/v90/digital.rs`
  - `crates/datapump/src/v90/startup.rs`
  - `crates/datapump/tests/v92_hold.rs`
- **Digests:** MOH (2.3); CV (Q9); CA (9.2).
- **Tests:**
  - `a_v90_mode_call_between_v92_modems_can_be_held`.
  - `a_call_with_a_v90_only_modem_will_not_start_a_hold`.

#### WP-54: Hold call control in the modem crate (L)

- **Depends on:** WP-48, WP-51, WP-52.
- **Clauses:** 9.10.2.1-9.10.2.3; 9.2 (the return by short Phase 1); V.8 8.1.2 and 8.2.3 (the zero CM); V.250 6.3.6
  and 6.3.7; V.42 7.10 and 7.11.
- **Files:**
  - `crates/modem/src/lib.rs`
  - `crates/modem/src/hold.rs` (new)
  - `crates/datapump/src/v8.rs`
  - `crates/v8/src/lib.rs`
  - `crates/telemetry/src/lib.rs`
  - `crates/modem/tests/v92_call.rs`
- **Digests:** CV (6.3.3, 6.5, R6, R9, R10); MOH (2.5-2.8, 6.3-6.5); CTL (5.2-5.4).
- **What to build:** per CV 6.3.3.
  - **Modem API:** `request_hold()`, `hold_state()`, and a `call_waiting()` hook.
  - **The held side:** the pump ends, and `v8line::Modem::held(t1, ..)` takes over:
    - ANSam for T1, with no 5 s or 60 s limits;
    - QC or CM starts an answering Phase 1 (quick when allowed);
    - a QC with U_QTS `1111` is a cleardown;
    - a zero-modulation CM gets a zero JM, and the call clears on CJ.
  - **The requester is away:**
    - `ATO` builds a resuming calling V.8 (quick with the memo);
    - the result is `CONNECT` via `pending_connect`;
    - the `Stack` is kept and suspended/resumed (WP-07).
  - **`ATH` on hold** sends QC1a `1111` (analogue) or a zero CM, then drops.
  - **During hold,** timers are frozen and the carrier guard is off.
  - **`Ended::ClearedDown` and `Ended::HoldExpired`** report `NO CARRIER`.
  - **`CallState::OnHold`**, and the `distant` rows.
- **Tests** (`Pair`):
  - `a_v92_call_put_on_hold_says_nothing_to_the_terminal_and_resumes_with_the_same_link`.
  - `a_hold_that_runs_out_ends_the_call_with_no_carrier`.
  - `ath_on_hold_gives_the_held_side_no_carrier_within_2_s`.
  - `a_cleardown_cm_ends_the_held_call_with_no_carrier`.
  - `a_refused_hold_with_fast_reconnect_comes_back_up`.
  - `text_sent_before_the_hold_arrives_after_it`.

### Wave 17

#### WP-55: The V.250 `+P` commands (M)

- **Depends on:** WP-46, WP-50, WP-54.
- **Clauses:** V.250 6.8.1-6.8.8 (Tables 31-37), 6.1.9, 5.4.2, 6.3.6, 6.3.7; V.92 Table 33 (T1 codes).
- **Files:**
  - `crates/at/src/lib.rs`
  - `crates/at/tests/session.rs`
  - `crates/modem/src/lib.rs`
  - `crates/gui/src/app.rs`
- **Digests:** CTL (5); CV (6.4, R13, R15, Q4, Q5).
- **What to build:**
  - **A `V92` settings struct** holding `+PCW`, `+PMH`, `+PMHT`, `+PIG`, `+PQC` and `+PSS`, each with read, test and
    set.
  - **`+PMHR` and `+PMHF` actions,** with deferred results:
    - `+PMHR` gives a delayed `+PMHR: n`, or ERROR when hold is disabled or the modem is idle;
    - `+PMHF` gives OK on hold (and a log note), ERROR otherwise.
  - **`Action` variants,** handled in `modem::run_actions` and in `gui::app::perform`.
  - **The settings reach the pumps:**
    - `+PIG` sets `pcm_upstream`;
    - `+PQC` and `+PSS` set the quick and short-Phase-2 flags;
    - `+PMH` and `+PMHT` set the hold grant.
  - **`+PCW`**, through `call_waiting()`.
  - **`+GCAP`** lists the new names once they act. `&F` and `Z` reset them. Add `Interpreter::info()`.
- **Tests:**
  - In `session.rs`:
    - `the_p_parameters_read_back_their_defaults`;
    - `values_outside_tables_31_to_37_are_refused`;
    - `the_p_test_replies_list_what_is_accepted`;
    - `every_command_gcap_names_is_answered`, updated;
    - `pmhr_and_pmhf_are_actions`.
  - In the modem crate:
    - `at_pmhr_is_answered_with_the_granted_t1_then_ok`;
    - `pmht_0_at_the_far_end_gives_pmhr_0`;
    - `pmh_1_makes_pmhr_an_error`;
    - `pmhf_is_ok_on_hold_and_an_error_otherwise`;
    - `pig_1_gives_v92_with_v34_upstream`;
    - `pqc_3_puts_no_qc1a_on_the_line`;
    - `pss_2_forces_a_full_start_up`;
    - `pcw_1_hangs_up_on_a_call_waiting`;
    - `and_f_restores_the_p_defaults`.

### Wave 18

#### WP-56: GUI, quick connect, hold controls, status and the memo (M)

- **Depends on:** WP-55.
- **Clauses:** V.250 6.8; V.92 1 j.
- **Files:**
  - `crates/gui/src/app.rs`
  - `crates/gui/src/live.rs`
  - `crates/gui/src/remembered.rs`
  - `crates/gui/src/answer.rs`
- **Digests:** CV (6.7, 2.6, R16, R19); CTL (5.4).
- **What to build:**
  - **Settings:** "quick" and "hold" checkboxes (`+PQC`, `+PMH`) in a `Pcm` settings struct, which is remembered.
  - **Buttons:**
    - Hold (`AT+PMHR`), Resume (`ATO`) and Flash (`AT+PMHF`);
    - a simulated Call waiting button, through a `Session` atomic.
  - **State:** `CallState::OnHold` in the frame, hold transitions logged, and "hold" and "start-up" status rows.
  - **The memo** is saved and loaded, keyed by dial string or a last-call slot.
  - **`answer.rs --grant-hold <n>`.**
- **Tests:**
  - `the_quick_and_hold_toggles_type_their_commands`.
  - `the_p_settings_are_remembered`.
  - `the_quick_connect_memo_round_trips_through_its_file`.
  - `on_hold_is_shown_as_its_own_call_state`.
  - `answer_grant_hold_types_pmht`.

### Wave 19

#### WP-57: Scope, the upstream PCM pair plot at the server (S)

- **Depends on:** WP-37, WP-56.
- **Clauses:** none. This follows Rory's display choice: the consecutive-sample pair plot.
- **Files:**
  - `crates/modem/src/lib.rs`
  - `crates/datapump/src/v90/startup.rs`
  - `crates/datapump/src/v92/digital.rs`
  - `crates/gui/src/app.rs`
- **Digests:** CV (6.1 item 6); CA (3.2); CT (8.3).
- **What to build:**
  - `v92::digital` publishes (sample n, sample n+1) pairs of the received upstream A/D levels. The wrapper passes them
    through, and the modem crate offers them for `V90Server` with V.92 upstream.
  - The existing "PCM" pair panel draws them, with permanent "sample n / sample n+1" axis labels and the existing
    footer style.
- **Tests:**
  - `a_v92_server_s_scope_shows_consecutive_upstream_sample_pairs`.
  - `the_analogue_side_still_shows_the_downstream_pairs`.
  - `the_pcm_label_selects_the_pair_plot_for_either_direction`.

---

## 6. Waves

WPs in a wave touch disjoint files, so they can run in parallel worktrees. Paths are abbreviated: `dp` =
`crates/datapump/src`, `dpt` = `crates/datapump/tests`.

| Wave | WPs | Files touched (disjoint within the wave) |
|---|---|---|
| 1 | WP-01, WP-02, WP-03, WP-04, WP-05, WP-06, WP-07 | `dp/lib.rs` + `dp/v92/*` (new stubs) · `dp/v34/{info,dpsk,phase2}.rs` + `dpt/{v90,v34}_vector.rs` · `dp/v90/sequences.rs` · `dp/v90/network.rs` · `dp/v90/{analogue,downstream,mod}.rs` · `v8/src/{quick,lib}.rs` · `ec/src/{stack,lapm}.rs` |
| 2 | WP-08 .. WP-17 | `dpt/v92_vector.rs` · `v92/sequences.rs` · `v92/modulus.rs` · `v92/precoder.rs` · `v92/up_signals.rs` · `v92/anspcm.rs` · `v92/exchange.rs` · `v92/transmit.rs` + `v90/pcm.rs` · `v34/{phase2,startup}.rs` · `v90/network.rs` |
| 3 | WP-18 .. WP-24 | `v92/upstream.rs` · `v92/receiver.rs` · `v92/epsilon.rs` · `v92/up_source.rs` · `v92/down_source.rs` · `v34/phase2.rs` · `v92/hold.rs` + `dp/v8.rs` |
| 4 | WP-25 .. WP-29 | `v92/decoder.rs` · `v92/analogue.rs` · `v92/digital.rs` · `v34/phase2.rs` · `v92/design.rs` |
| 5 | WP-30 .. WP-33 | `dpt/v92_call.rs` + `v92/{analogue,digital}.rs` · `v92/upchoice.rs` · `v92/up_source.rs` · `v92/down_source.rs` |
| 6 | WP-34, WP-35 | `v92/analogue.rs` · `v92/digital.rs` |
| 7 | WP-36 | `dpt/v92_call.rs`, `v92/{analogue,digital}.rs` |
| 8 | WP-37 | `v90/{startup,server}.rs`, `dpt/v92_call.rs` |
| 9 | WP-38, WP-39, WP-40 | `v92/memo.rs` + `v90/{startup,analogue,digital}.rs` + `dpt/v92_call.rs` · `v92/{receiver,digital}.rs` + `dpt/v92_impairments.rs` · `dpt/v92_replay.rs` |
| 10 | WP-41, WP-42 | `v92/{receiver,decoder,digital}.rs` + `dpt/v92_impairments.rs` · `dp/{v8,bell103}.rs` |
| 11 | WP-43, WP-44 | `v92/{analogue,digital}.rs` + `dpt/v92_renegotiation.rs` · `v90/{startup,server}.rs` + `dpt/v92_quick.rs` |
| 12 | WP-45, WP-46 | `v92/{analogue,digital}.rs` + `dpt/v92_renegotiation.rs` · `at/src/lib.rs` + `at/tests/session.rs` + `modem/src/lib.rs` + `modem/tests/v92_call.rs` |
| 13 | WP-47, WP-48 | `v92/{analogue,digital}.rs` + `dpt/v92_renegotiation.rs` · `modem/src/lib.rs` + `modem/tests/v92_call.rs` |
| 14 | WP-49, WP-50 | `v92/{analogue,digital}.rs` + `v90/startup.rs` + `dpt/v92_renegotiation.rs` · `gui/src/{app,answer}.rs` |
| 15 | WP-51, WP-52 | `v34/{phase2,startup}.rs` + `v90/startup.rs` + `v92/{analogue,digital,hold}.rs` + `dpt/v92_hold.rs` · `modem/src/lib.rs` + `modem/tests/{v92_call,v90_call}.rs` |
| 16 | WP-53, WP-54 | `v90/{analogue,digital,startup}.rs` + `dpt/v92_hold.rs` · `modem/src/{lib,hold}.rs` + `dp/v8.rs` + `v8/src/lib.rs` + `telemetry/src/lib.rs` + `modem/tests/v92_call.rs` |
| 17 | WP-55 | `at/src/lib.rs`, `at/tests/session.rs`, `modem/src/lib.rs`, `gui/src/app.rs` |
| 18 | WP-56 | `gui/src/{app,live,remembered,answer}.rs` |
| 19 | WP-57 | `modem/src/lib.rs`, `v90/startup.rs`, `v92/digital.rs`, `gui/src/app.rs` |

A WP that finds it must touch a file outside its list stops and reports. It does not edit a file another WP in the
same wave owns.

---

## 7. Dependency outline

```text
WP-01 ─┬─ WP-09 (+WP-03) ─┬─ WP-18 (+WP-10, WP-11) ─┬─ WP-25 ─┬─ WP-31 ─┐
       │                  │                         ├─ WP-29 ─┘         │
       │                  ├─ WP-21 (+WP-12, WP-15) ─┼─ WP-32 ─┐         │
       │                  └─ WP-22 (+WP-13) ────────┼─ WP-33 ─┤         │
       ├─ WP-12 ─┬─ WP-19 (+WP-15) ─────────────────┤         │         │
       │         └─ WP-20 (+WP-15)                  │         │         │
       ├─ WP-14 ────────────────────────────────────┼─────────┤         │
WP-05 ─────────── WP-26 (+WP-21, WP-22) ──┐         │         │         │
                  WP-27 (+WP-19, WP-20) ──┴─ WP-30 ─┴─ WP-34, WP-35 ────┴─ WP-36 ─ WP-37 ─┬─ WP-38 ─ WP-44 ─ WP-48
WP-02 ─ WP-16 ─ WP-23 ─ WP-28 ─────────────────────────────────────────────────────────────┘                    │
WP-06 ─ WP-42 ──────────────────────────────────────────────────────────────────────────────────── WP-44 ────────┤
WP-02 ─ WP-24 ────────────────────────────────────────────────────── WP-51 ─ WP-53, WP-54 ─ WP-55 ─ WP-56 ─ WP-57
WP-36 ─ WP-39 ─ WP-41 ─ WP-43 ─ WP-45 ─ WP-47 ─ WP-49 ─ WP-51
WP-37 ─ WP-46 ─ WP-48 ─ WP-52 ─ WP-54
```

The critical path is WP-01 → WP-09 → WP-18 → WP-25/WP-29 → WP-31 → WP-35 → WP-36 → WP-37. The first live-worthy
V.92 PCM-upstream build is WP-46 (wave 12). Its prerequisites are M3 and M4.

---

## 8. Keeping V.90 and V.34 working

- `startup::Analogue::new` and `startup::Digital::new` stay V.90-only (AD-2). V.92 is reached only through
  `with_v92`, which is chosen only by `+MS=V92`.
- Every shared codec change in WP-02 and WP-03 has a "the V.90 encoding is bit-identical" test.
- The existing real-capture tests (`v90_vector`, `v34_vector`, the real Jd CRC, the real 1736-bit descriptor) are
  never changed except to add arms.
- The behaviour-preserving refactor (WP-05) is its own WP, gated on the slip suite and the recorded DIL sweep count.
- Every `Network` addition keeps today's defaults, and WP-04 proves the tabulated kernel equals the old one.
- WP-37 re-runs every V.90 `FullCall` test and adds V.92↔V.90 pairings in both directions.
- The V.34-upstream fallback carries real calls on transcoding paths (the Crazytel memory note). WP-37, WP-38 and
  WP-41 test it explicitly. Any known V.34 fallback bug found on the way is flagged as a separate task, not fixed
  inside a V.92 WP.

## 9. Fallbacks the Recommendation requires, and where each is tested

| Situation | Required behaviour | Clause | Test WP |
|---|---|---|---|
| Either modem not V.92 | V.90 INFO layouts (Tables 9/10/11); V.90 phases 3 and 4 | 9.3 | WP-16, WP-37 |
| INFO1d bit 70 clear | Table 18 must not be sent; Table 10 or 11 | 8.4.1 | WP-16, WP-37 |
| Short Phase 2 not requested by both | full Phase 2 | 9.4 | WP-23 |
| Analogue modem not intending PCM up or V.90 mode | must not request short Phase 2 | 9.4 | WP-23, WP-38 |
| No A reversal / no INFO1a (short) | digital modem into full Phase 2 | 9.4.1.2.2-3 | WP-28 |
| No B reversal (short) | analogue modem retrains (9.7.2.1) | 9.4.2.2.2 | WP-28 |
| Retrain of a V.92 pair | always V.92 full Phase 2, no INFO0 | 9.3, 9.7 | WP-16, WP-37 |
| Table 19 INFO1a | V.90 data mode (V.90 Phase 3) | 9.4.x.1.x (interpretation) | WP-38 |
| No QCA after QC1a | V.8 continues (CM already sent) | 9.2.1.1, 9.2.2.1 | WP-42, WP-48 |
| Non-V.92 answerer | ignores QC1a; V.8 | V.8 clause 10 | WP-42, WP-48 |
| No TONEq within 2 s (digital answerer) | ANSam, V.8 | 9.2.4.3 | WP-44, WP-48 |
| No ANSpcm within 2 s (analogue answerer) | ANSam, V.8 | 9.2.3.3 | WP-44 |
| Phase 3 or 4 watchdogs | retrain | 9.5.x.2, 9.6.x.2 | WP-26, WP-27, WP-34, WP-35 |
| RR detected during FPE | RR takes precedence | 9.9.1.1.2, 9.9.2.1.2 | WP-47 |
| MH initiator unanswered | retrain or disconnect | 9.10.1.1 | WP-24, WP-51 |
| PCM upstream will not train (local policy) | V.34 upstream, then V.34 | AD-9 | WP-37, WP-41 |

## 10. Out of scope, and why

1. **The V.8 bis path of short Phase 1:** CRe detection and generation, QC2a, QC2d, QCA2a, QCA2d, and the V.8 bis
   fallback transactions (9.2.1.2, 9.2.2.2, 9.2.3.2, 9.2.4.2).
   - The repo has no V.8 bis. V.92 makes CRe detection optional for callers (9.2.1, 9.2.2), and answerers may choose
     ANSam.
   - On the VoIP rig, the 1 s window after QC2x can never be met with a 1.5 s round trip (P1P P14).
   - No capture shows a server sending CRe.
   - A digital answerer that sent CRe would cost a caller 3 s. Ours never sends CRe.
   - The QC2 codec could be added later from P1A 6 and CTL 3.2 if a capture ever shows CRe.
2. **The digital answerer taking the analogue role on QC1d or QC2d** (the MAYs of 9.2.4.1 and 9.2.4.2). BinModem's
   digital modem only ever answers as a server. A two-digital pair still ends up with the caller as the analogue
   modem through V.8 (V.90 9.1.1).
3. **V.59 managed objects (`+TMO V92`).** V.59 is not in `docs/specs`.
4. **A real hook flash, call-waiting tone (CAS/SAS) detection, and Caller ID (`+VCID`, V.253).**
   - There is no DAA, the softphone owns the line, and V.253 is not in `docs/specs`.
   - `+PMHF` and `+PCW` get their command semantics and a simulated call-waiting trigger only (WP-55, WP-56).
5. **Testing facilities (clause 10).** They are "for further study", and V.54 loopbacks must not be used. There is
   nothing to implement.
6. **Circuit 133 per V.43 4.2.1.1** (Table 1 Note 2). V.43 is not in `docs/specs`. The existing DTE flow control
   stays.
7. **V.80 as the asynchronous converter** (7.2). V.80 is not in `docs/specs`. V.14 and V.42 (both present) meet the
   clause.
8. **Sending a manufacturer-defined MD, in either direction.** MD is optional and its content is undefined (8.5.3,
   V.34 10.1.3.5). Our analogue modem sends MD length 0 in INFO1a, and our digital modem sends MD length 0 in
   INFO1d: Figures 10 and 11 show no downstream MD at all in a PCM-upstream Phase 3 (P3S A14). Receiving a nonzero
   analogue MD length and waiting it out is in scope (WP-27).
9. **Asking for codec-side constellations in CPt or CPu** (Table 23 bit 128 = 1, and the γ/δ doubling that follows).
   Our analogue modem's transmit constellations are the codec's own Ucode levels, so bit 128 is always 0 in what we
   send. Parsing an incoming bit 128 does not arise: only the analogue modem sends Table 23. The length arithmetic
   for δ = 2γ + 136 is still implemented and tested in WP-03, so turning it on later is a one-line change.
10. **An analogue-side echo canceller** (for 2-wire lines). The rig is 4-wire, and V.90 has none either. The digital
    modem's canceller (WP-39) is in scope, because the simulated server faces hybrid echo before its A/D.
11. **Spectral shaping requested by our analogue modem** (Sr > 0 in CPu). `dil::choose` never asks for it today, and
    V.92 does not change that. The digital modem still honours any Sr through the V.90 encoder.
12. **Changing the CI build profile** (`[profile.dev.package.datapump] opt-level`). This is the maintainer's decision
    (CT 9.6). It is raised as a risk, not done in a WP.

## 11. Risks

1. **Upstream PCM on the live rig.**
   - The softphone path may resample, apply AGC, AEC or noise suppression, slip 20 ms or 10 ms, or transcode (the
     Crazytel memory note).
   - Any of these can make PCM upstream impossible while every simulation passes (CT 13.1).
   - The V.34-upstream fallback must stay solid, and the ladder must step down quickly.
2. **The transmit-timing design forks** (AD-7). Interpolating to the network clock suits a real loop; passing samples
   straight through suits a softphone. The wrong choice kills PCM upstream on the other path. Only captures (channel 1,
   ε behaviour) can decide.
3. **Our two ends share one reading of the text.** Call-to-call tests cannot catch a shared misreading of these:
   - the TRN2u bit order;
   - the CPd point scale and sign;
   - d(f-1);
   - absent CPd parts;
   - the frame origin;
   - the Su-bar extension.

   They are isolated as named constants (section 4), but only a real V.92 capture settles them.
4. **Timers against a 1.1-1.6 s round trip:**
   - short Phase 1's 2 s windows (no RTD term);
   - short Phase 2's fixed 2500 ms timers;
   - TR3's 1500 ms;
   - the 100 ms + RTD repeat rule;
   - the analogue modem having no RTD after a short Phase 2;
   - a retrain that mis-measured RTD (the Crazytel note: 0.31 s measured against 1.1 s real).

   Documented departures and memo RTDs mitigate this. Some windows cannot be met live whatever we do.
5. **Silent misparses between versions:**
   - a Jp read as a Jd;
   - a V.92 CPt read through the V.90 CP layout (doubled drn);
   - a V.92 INFO0a read as a V.34 clock field;
   - a Table 18 INFO1a dropped;
   - bit 70 read from a V.90 server.

   Cross-version tests exist in WP-02, WP-03, WP-09 and WP-16. A miss would silently give V.90, or worse, PCM upstream
   against a V.90 server.
6. **The digital modem's design problem is ours to invent:** the precoder, prefilter, G, constellations and moduli
   (CD 7.6). A poor design shows up as a low upstream rate rather than a failure. WP-29 and WP-31 need honest channel
   models (WP-17) to mean anything.
7. **Echo and upstream slips at a real server are invisible to us.** Recovery there is the server's business, and
   unknown. Expect V.42 retransmission bursts or retrains that the simulation does not predict.
8. **The DIL's 36-symbol alias weakness** (17 of 260 slip positions fail) carries into V.92 unchanged, now after Jp'.
   WP-05 must not change it. Improving it is a separate task.
9. **The refactor in WP-05** touches the slip-sensitive DIL code that live V.90 calls depend on.
10. **State-machine size.** `v92/analogue.rs` and `v92/digital.rs` gain Phase 3, Phase 4, RR, FPE, cleardown and
    hold over eight waves. Named deadline slots and the shared exchange keep them reviewable. Several L-sized WPs
    remain (WP-26, WP-27, WP-34, WP-35, WP-51, WP-54).
11. **Coupling in the modem crate and the GUI:**
    - the exhaustive matches on `Pump`, `v90::startup::Status`, `v8line::Status` and `at::Action`;
    - the index-coupled `CARRIERS`/`rates`/`CEILINGS`.

    AD-2 and AD-3 limit the damage. Late WPs still serialise on `modem/src/lib.rs` and `gui/src/app.rs`.
12. **CI runtime.** CI runs the dev profile. V.92 roughly doubles the both-ends matrix, and the precoder design and
    Viterbi add cost. WP-04 tabulates the network kernel. Sweeps are `#[ignore]`d. The opt-level change is for the
    maintainer.
13. **Clippy with `-D warnings`** on scaffolding. Stub modules must contain no unused private items. Every new public
    type needs `Debug`.
14. **Round-trip-delay and timing claims** in several digests come from project memory rather than the Recommendation.
    They are planning inputs, not requirements.

## 12. Live-test requests to batch for Rory

Live testing is Rory's job, so these are gathered into one request, to make once WP-46 is merged:

1. **A capture of a real V.92 server call with `+MS=V92`,** both channels. It settles:
   - INFO1d bit 70 (the server's verdict on our path);
   - the INFO1a layout;
   - whether the server grants short Phase 2;
   - Jp's ε;
   - a CPd to decode, which settles the point scale, absent parts, the 128-point limit and the TRN2u bit order.
2. **A both-ends-ours live call,** our server on cable B behind a second SIP account. The host's channel 1 shows
   exactly what arrived upstream: slips, AGC, the decimation phase.
3. **MicroSIP settings:** AGC, AEC, noise suppression, VAD/CNG and PLC off; PCMU only if possible.
4. **`AT+PMHR` against the NetZero/GlobalPOPs pool**, with a recording running, to see whether V.92 hold is granted.
5. **A second call with the memo, to the same server**, to see whether quick connect and short Phase 2 are honoured
   live.
