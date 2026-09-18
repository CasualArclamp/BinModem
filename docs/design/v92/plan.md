# V.92 implementation plan

ITU-T V.92 (11/2000), "Enhancements to Recommendation V.90", built in BinModem from the Recommendation and
real captures only. 62 work packages in 18 waves. Both ends of every feature are ours, so every feature is
proved call-to-call over `v90::network::Network` before it is ever offered to a live line.

---

## 0. Why this shape

Two independent plans were written first: `plan-foundations.md` (foundations first, 57 WPs) and
`plan-value.md` (a working call as early as possible, 46 WPs). This plan takes the foundations skeleton and
grafts the best of the other onto it, for these reasons.

**The foundations skeleton wins on coverage, ordering and parallelism.** Counting waves rather than work
packages, it reaches Phase 3 call-to-call at its wave 5 and a complete PCM-both-ways call at its wave 7,
against the value plan's waves 7 and 9, *and* it keeps seven to ten packages running abreast in the early
waves, where the value plan narrows to two or three from its wave 4 onward. It also covers things the other
plan simply has not got: hold on a V.92 pair that fell back to V.34 upstream, a graceful `ATH`, the memo as
a module with a text round trip, the `+P` action commands reaching `modem::run_actions` *and*
`gui::app::perform` (the value plan's `+P` package adds `at::Action` variants but does not list the two
files whose exhaustive matches would then fail to compile), and `v90/server.rs` in the hand-over package.

**Five things are grafted from the value plan, and they matter.**

1. **A de-risk spike before any state machine** (V92-26). The digital modem's PCM-upstream receiver and the
   precoder/prefilter design it downloads have no precedent in this repository and no reference we are
   allowed to look at. The spike joins the transmit chain, the design and the decoder across a synthetic
   codec channel with an explicit go/no-go: if it cannot reach the upper half of the upstream ladder, the
   plan stops and V.34 upstream becomes the product. The foundations plan had no such gate.
2. **A `Parameters` struct as the boundary between the wire and the chain** (AD-3). The chain, the decoder
   and the design then depend only on the numbers package, not on the CPd bit layout, which lets three
   packages run in wave 3 that would otherwise have queued behind `v92/sequences.rs`.
3. **Phase 3 split at Jd** (V92-32/33, then V92-36/37). The foundations plan had two L-sized Phase 3
   packages; four M-sized ones fit the "one engineer-agent in one sitting" rule and give two two-wide waves.
4. **`pcm::Receiver::symbol_clock()` as its own S package** (V92-06), instead of being smuggled into the
   transmitter package and dragging `v90/pcm.rs` into a wave that owns `v92/transmit.rs`.
5. **Two concrete findings about `v90/sequences.rs`**: `Jd::from_bits` will happily read a V.92 Jp as a Jd
   full of nonsense rates, and `Finder`'s exactly-17-ones rule drops the first Ja descriptor and the first
   CPt behind their 24-one preambles. Both are folded into V92-03, but as *additive* V.92 paths, so V.90's
   own reading is untouched.

**What is deliberately kept from foundations.** The long "readings fixed now" table (section 4), which is
the difference between one wrong reading being a one-line change and being a week of debugging; AD-4 and
AD-5 (no new `Pump` variant, no new `Status` variants), which keep about twenty exhaustive matches in
`crates/modem` and `crates/gui` from being edited in every second package; the transmit-timing fork
(AD-8), which the project's softphone rig makes unavoidable; the split of the integration tests by topic,
so the late waves do not all collide on one test file; and the split of modem-on-hold into a transaction
machine (wave 3) and its pump integration (wave 15).

**What the grafts cost.** Splitting Phase 3 at Jd and landing the spike push the first complete call from
the foundations plan's wave 7 to wave 9 here, and take the package count from 57 to 62 (the extra one is
V92-62, the numeric upstream choice split out of V92-30 so the M0 gate can depend on it; section 14). That is the trade
made deliberately: a fortnight of state-machine work is not worth starting until the spike says the
upstream can carry data at all, and four M packages are safer to hand out than two L ones. Wave count is
not the same as elapsed time; waves 5 to 9 are two packages wide either way.

**Where the plan is honest about serialisation.** `v92/analogue.rs` and `v92/digital.rs` are single files
that Phase 4, rate renegotiation, fast parameter exchange, cleardown and hold all have to enter. From wave
8 they serialise, one feature per wave. Every such wave is paired with a package in another crate so the
wave is still two wide.

---

## 1. Sources

All digests are in `F:\dialupmodem2\docs\design\v92\`. Short names used throughout:

| Short | File | Covers |
|---|---|---|
| INTRO | `spec-intro-transmitter.md` | clauses 1-7: scope, Qa.b, digital modem = V.90 clause 5, the PCM-upstream transmitter (6.1-6.4), circuits |
| P1A | `spec-phase1-signals-analogue.md` | 8.1, 8.2: QC1a, QC2a, QCA1a, QCA2a, TONEq; V.8/V.8 bis/V.21 framing |
| P1D | `spec-phase1-signals-digital.md` | 8.3: ANSpcm (Tables 6-10 and the verified octets), QC1d, QC2d, QCA1d, QCA2d, QTS |
| P1P | `spec-phase1-procedures.md` | 9.1, 9.2 (Figures 3-8), the four role state machines, 28 timers |
| P2S | `spec-phase2-signals.md` | 8.4, 8.4.1, Tables 15-19, INFO CRC vectors, layout classification |
| P2P | `spec-phase2-procedures.md` | 9.3, 9.3.1, 9.4 (Figure 9), V.90 9.2 restated, retrain entry points |
| P3S | `spec-phase3-signals.md` | 8.5, 8.6, Tables 20-23, scramblers, sign conventions, Jd/Jp/CPt vectors |
| P3P | `spec-phase3-procedures.md` | 9.5 (Figures 10, 11), steps D1-D13 and A1-A11, recovery |
| P4A | `spec-phase4-signals-analogue.md` | 8.7: B1u, E2u, CPu, CPus, RM/RM', SUVu, TRN2u (Tables 23-29), FB1u |
| P4D | `spec-phase4-signals-digital.md` | 8.8: B1d, Ed, CPd (Table 30), Rd/Rt/Rf, SUVd (Table 31), TRN2d |
| P4P | `spec-phase4-procedures.md` | 9.6 (Figures 12-14), 9.7, the SUV/CP/E state machine |
| RRF | `spec-renegotiation-fpe.md` | 9.8 (Figures 15-18), 9.9 (Figure 19), 9.11, detectors, errata |
| MOH | `spec-modem-on-hold.md` | 8.9 (Tables 32, 33), 9.10 (Table 34, Figures 20-24), clause 10, MH vectors |
| CTL | `spec-control-plane.md` | V.8, V.8 bis, V.250 6.8 (`+P`), `+MS`, V.42/V.44 ties |
| CA | `code-analogue.md` | the V.90 analogue modem as built, extension points, house style |
| CD | `code-digital.md` | the V.90 digital modem, `sequences.rs`, `network.rs`, what a V.92 receiver needs |
| CV | `code-v8-and-call.md` | V.8, the modem crate, AT, GUI, telemetry, hold and quick-connect integration |
| CT | `code-tests.md` | harnesses, network gaps, proposed V.92 tests, runtime budget, live constraints |

Also read `docs/design/architecture.md` (the streaming rule: no block APIs in `dsp` or `datapump`).

Where this plan and a digest disagree, **the digest wins**. Where a digest is itself in doubt, render the
PDF page with PyMuPDF at 170 dpi (300 dpi for a table) into
`C:\Users\Gaming\AppData\Local\Temp\claude\F--dialupmodem2\<session>\scratchpad\v92\pages` and read the PNG
with the Read tool. The extracted text in `docs/specs/text` is lossy; use it only to find things.

---

## 2. Rules every work package follows

- **Specs and real captures only.** Never consult, port from, build or run spandsp or any other modem
  implementation. Do not search the web for implementations.
- **Definition of done**, for every package:
  - `cargo clippy --workspace --all-targets -- -D warnings` is clean, including
    `missing_debug_implementations`: every public type derives `Debug`, and the modems are `Clone`.
  - `cargo test --workspace --release` passes, **including every existing V.34 and V.90 test, unedited**.
  - The package's own named tests exist and pass.
  - The package edited no file outside its own list. A package that finds it must stops and reports.
- **House style** (CA 14, CD 12, CV 7, CT 12): module docs tell the story first; every constant quotes its
  clause; failure reasons are lowercase sentences; tests are named as sentences with a doc comment citing
  the clause or the capture; streaming `step`/`feed`/`heard`, never block APIs; deadlines are
  `Option<(u64, &'static str)>` in samples; British spelling.
- **Deliberate departures** from a printed timer (for the project's 1.1-1.6 s VoIP round trip) are named
  constants whose doc gives the clause, the printed value and the live evidence.
- **Ambiguous readings** are named constants in the owning module (section 4), so that one edit flips a
  reading if a capture ever disagrees.

Sizes: **S** up to about 300 lines including tests, **M** 300-800, **L** 800-1300.

---

## 3. Architecture decisions

**AD-1: a sibling `datapump::v92` module.** It reuses `v90` for everything V.92 inherits unchanged:
`encoder`, `sign`, `modulus` (6 intervals), `ucode`, `dil`, `pcm::Receiver`, `carrier::Watch`, and the
framing helpers in `sequences`. Folding PCM upstream into `v90/analogue.rs` (1931 lines) or
`v90/digital.rs` (1122 lines) would double their state and put the live V.90 path at risk (CA 11.1,
CD 8.1). `v92` depends on `v90`, never the reverse.

**AD-2: the V.90 modems are the fallback, not dead code.** A V.92 call that cannot hold PCM upstream runs
`v90::analogue::Modem` for real, so every V.90 test is also a test of the middle rung of the V.92 ladder.

**AD-3: `Parameters` is the boundary between the wire and the chain.**
`v92::Parameters { drn, trellis, extend_e2u, gain, moduli: [u8; 12], filters: { z1, p1, z2, p2 }, sets, indices }`
lives in `v92/mod.rs`. `Cpd` (Table 30) converts to and from it. The transmitter, the decoder and the design
all speak `Parameters` and know nothing about bit layouts, so they can be built and tested before the wire
format exists.

**AD-4: the start-up wrappers own V.92; there is no new `Pump` variant.** `v90::startup::Analogue` and
`Digital` already own V.34's start-up, V.90's Phase 2, retrains and the V.34 fallback. They gain
`enum Pcm { V90(..), V92(..) }`, chosen by which INFO1a layout Phase 2 settled on. `crates/modem` keeps
`Pump::V90` and `Pump::V90Server`, which avoids editing about twenty exhaustive matches (CV R12).
`new()` stays V.90-only; `with_v92(..)` is opt-in and reached only from `+MS=V92`, so `+MS=V90` behaves
exactly as today even against a V.92 server (V.8 cannot say "V.92"; 9.1 NOTE).

**AD-5: new events are accessors, not enum variants,** wherever an enum is matched exhaustively elsewhere.
`v8line::Status` and `v90::startup::Status` are matched exhaustively in `crates/modem`. Quick connect is
therefore reported through `v8line::Modem::quick()`, the return to V.8 through `take_back_to_v8()`, and
hold through `hold_event()`. New `Info` variants *do* force edits in `v34/phase2.rs`,
`tests/v90_vector.rs` and `tests/v34_vector.rs`; V92-02 makes them, once.

**AD-6: one shared model of the analogue transmitter.** `v92::{modulus, precoder, upstream}` is the
analogue modem's transmitter *and* the digital modem's model of it. The digital modem designs against the
same code it will decode, verifies a candidate CPd by running it before sending it, predicts B1u with it,
and uses it in tests (CD 7.6 step 6, 7.8). x(n) and v(n) stay in f64 and are never saturated (INTRO P-12).

**AD-7: separate sources and state machines.** Each side has a pull-driven signal source in the shape of
the existing `Source`/`Up`/`Out`: `v92::up_source` in LU units, `v92::down_source` in G.711 octets. Each
side has a state machine consuming receiver events: `v92::analogue`, `v92::digital`. A source can be
scripted by time, so each state machine is tested against a scripted peer before the two meet.

**AD-8: PCM transmit timing has two modes, and which one a live line needs is an open question.**
`v92::transmit::PcmTransmitter` turns 8 kHz levels into line samples as `Interpolated` (windowed-sinc
reconstruction under 4 kHz with an arbitrary fractional delay) or `Straight` (at fs = 16 000, the level on
the codec's phase and a band-limited midpoint between). It has one-off shifts for S-bar-u: +0.5 T, then
+epsilon*T with epsilon = Jp bits 18:33 / 65 536. Its clock free-runs until the downstream receiver has
trained, is then slaved to a smoothed `pcm::Receiver::symbol_clock()` (drift, not the jittery timing-loop
phase) from the silence after Ja, and is never re-stepped after epsilon is applied (CA 11.3, INTRO 6.2).
Both modes are built and `Network` models both paths; only captures decide (CT 13.2).

**AD-9: the SUV/CP/E acknowledge exchange is one side-agnostic machine** (`v92::exchange`), driven by plain
flag structs rather than by the wire types, so it has no dependency on `v92/sequences.rs`. Phase 4, rate
renegotiation and fast parameter exchange all enter it with a context of `Training`, `Renegotiation` or
`FastExchange` (P4P 2.3, 6.5).

**AD-10: named deadline slots.** `v90/analogue.rs` overwrites a single `Option<(u64, &'static str)>` in
five places. V.92 has five overlapping timers in Phase 3 alone. `v92::Deadlines` holds them by name,
checks them together, and reports the earliest that has passed. RTD comes from Phase 2: RTDEd on the
digital side, RTDEa after a full Phase 2 on the analogue side. After a short Phase 2 the analogue modem
uses the RTD remembered in the memo; failing that, 0 for "shall not exceed" limits and 1.6 s for
"wait at least" timeouts (P2S N-11).

**AD-11: the fallback ladder.** The Recommendation's own branches come first: either side not V.92 means
V.90 layouts (9.3); bit 70 clear means no Table 18 (8.4.1); a short-Phase-2 failure means full Phase 2
(9.4.1.2.x, 9.4.2.2.2); short Phase 1 with no answer means V.8 (9.2). Then local policy, in
`v90::startup`: after `V92_RETRAINS` failed PCM-upstream start-ups, decline PCM upstream so the next
INFO1a is Table 10 (V.90 data mode); after `V90_RETRAINS` more, decline PCM so the next INFO1a is Table 11
(V.34). `+PIG=1` starts at step 1.

**AD-12: a "recognised connection" memo.** `v92::memo::Recognized` holds what a full start-up taught: RTD,
law, INFO1d bit 70, the V.34-upstream rate, carrier and pre-emphasis, PCM-upstream capability, U_QTS and
LM. It gates the short-Phase-2 request (P2P A3, N-21) and the sending of QC1a (CTL 5.3). The modem crate
holds one keyed by dial string, else a single last-call slot; the GUI stores it.

**AD-13: modem-on-hold.** The transaction (`v92::hold`) runs on the existing V.34 INFO DPSK modem
(`v34::dpsk`, 40-bit MH frames opt-in) and the existing tone A/B generators. MH detection is added beside
the retrain response in `v34::phase2`, because a hold request begins exactly like a retrain (9.10.1.1).
Call control (held side, requester away, resume by short Phase 1) lives in `modem/src/hold.rs`, beside
`faxcall.rs`.

**AD-14: the ODP/ADP bypass** applies when both P bits are set (9.2.5), or when both modems are V.92 and
both indicated LAPM in V.8 (9.3.1). The originator uses the existing `Stack::without_detection()`, the
answerer a new `bypassing_detection()`. An ODP that arrives anyway is still answered (V.8 7.3 warning).

**AD-15: integration tests are split by topic,** so late waves do not collide:
`v92_upstream.rs`, `v92_call.rs`, `v92_impairments.rs`, `v92_renegotiation.rs`, `v92_hold.rs`,
`v92_quick.rs`, `v92_vector.rs` and `v92_replay.rs` in `crates/datapump/tests/`, plus
`crates/modem/tests/v92_call.rs` and `crates/modem/tests/v92_hold.rs`.

---

## 4. Readings fixed now as named constants

These are the digests' open questions that block code. Each gets a documented default, owned by the named
module, and is checked against a real capture when one exists (section 13).

| Reading | Default | Owner | Digest |
|---|---|---|---|
| 6.4.1 step 4 uses d(f-1) | as printed: `R0 = R` when d(f-1) = 0, else `M-1-R`; d(-1) = 0 | `v92::modulus` | INTRO Q-1, Q-2 |
| Point selection in a class | minimum \|x(n)\|; ties to the smaller \|eta\| | `v92::precoder` | INTRO Q-3 (the 8.8.3 NOTE assumes exactly this) |
| CPd point "linear value" | unsigned magnitude on the V.90 Table 1 linear scale; negatives mirror a(-eta-1) = -a(eta) | `v92::sequences` | P4D Q1, INTRO Q-4 |
| "128 points" in a set | send 2·LC <= 128; accept LC <= 128 | `v92::sequences` | P4D Q3 |
| CPd with an absent part | keep the previous value; the first CPd of a training must carry every part, else retrain | `v92::sequences`, `v92::upstream` | P4D Q2 |
| Unsigned Qa.b | raw / 2^b; G = raw / 262 144 (4G is Q0.16) | `v92::mod` | INTRO P-10, 3.5 |
| epsilon | code / 65 536 symbols, LSB at Jp bit 18 | `v92::epsilon` | P3S A3 |
| Time order of TRN2u symbol bits | LSB first in time, sign bit last; `TRN2U_SIGN_LAST: bool = true`; one switch, both settings tested | `v92::up_signals` | P4A A2, RRF Q-6 |
| Only the sign bit of TRN2u is differential | yes | `v92::up_signals` | 8.7.6 |
| TRN2u, SUVu, CPu, E2u in training and RR | not precoded or prefiltered, like Ru | `v92::up_source` | P4A Q5, RRF Q-8, P4P Q3 |
| TRN2u differential seed in RR | the last transmitted sign of whatever precedes it (R-bar-u or E2u) | `v92::up_source` | P4A A4, RRF Q-5 |
| E2u's 13th symbol | one more scrambled, differentially encoded zero symbol | `v92::up_source` | P4A Q4 |
| RM memories | precoder, prefilter and trellis carry on from data mode; the modulus sign state is not clocked | `v92::upstream` | P4A Q7 |
| Upstream frame origin | the first symbol of the **second** TRN1u (8.5.7 with 9.5.1.1.10), after the epsilon shift, carried modulo 12, at both ends | `v92::up_source`, `v92::receiver` | P3P D10, CD 7.4 |
| S-bar-u 24.5T and (24+eps)T | a lasting delay of every later upstream symbol, not extra symbols; epsilon is measured after the 0.5 T shift | `v92::transmit`, `v92::epsilon` | P3S A1-A3 |
| CPt's 24 ones | scrambled, then differentially encoded | `v92::up_signals` | P3S A4 |
| TRN1u scrambler reset | before each TRN1u segment | `v92::up_signals` | P3S A6 |
| RR TRN2d/SUVd/CPd/Ed shaping and D | V.90 8.6 intro: data-mode shaping, with K from CPt | `v92::down_source` | RRF Q-18, P4D Q7 |
| Silence codeword | constant positive Ucode 0, frame alignment kept | `v92::anspcm`, `v92::down_source` | P4D Q12, 9.8.1.1.3 |
| ANSpcm first reversal | after 3612 symbols, table polarity first | `v92::anspcm` | P1D Q2 |
| QC/QCA acceptance | both copies valid and equal; accept at bit 59 | `v8::quick` | P1D Q5, P1A Q3 |
| Timers "after sending X" | run from the end of X | `datapump::v8`, `v90::startup` | P1P I2 |
| Default U_QTS and LM when no memo | U_QTS `0101` (Ucode 70); LM `01` (-12 dBm0, quieter than the observed 0.32-full-scale softphone limiter) | `crates/modem` | P1P Q8, CV Q3 |
| Digital caller TONEq timeout | 2 s + RTD after the end of the received QCA1a, then back to V.8 | `v90::startup` | P1P Q6 |
| RR/FPE watchdog | 5 s + 2 RTD from the X-to-X-bar transition (plus 8004T + RTD when a silent period was granted), then retrain | `v92::analogue`, `v92::digital` | RRF Q-1 |
| Minimum RR TRN2d | 2040T | `v92::digital` | RRF Q-3 |
| Post-silence TRN2u when ours to choose | 2400T to 8004T | `v92::analogue` | RRF Q-4 |
| CP-repeat window | the first CP or SUV whose *reception completes* after end-of-own-CP + 100 ms + RTD is included | `v92::exchange` | P4P Q12 |
| SUV bit 33 in the silence path | means "your SUV with bit 32 was received"; the ack state is cleared when the silence ends | `v92::exchange` | RRF 3.5, P4D Q10 |
| 9.11 "drn in SUV" | an editorial slip: drn = 0 in CPu/CPus bits 21:25 or CPd bits 22:26 | `v92::analogue`, `v92::digital` | RRF E-1 |
| MD | our analogue modem sends MD length 0; our digital modem sends INFO1d 18:24 = 0; a nonzero far MD length is waited out | `v92::up_source`, `v92::digital` | P3S A14, 8.5.3 |
| DIL | always request N != 0 and reuse `v90::dil::design`; the N = 0 / SCR path is parsed and answered but never asked for | `v92::analogue`, `v92::digital` | P3P 13.3 |
| MHack held until | the remote RT for 100 ms, or 2 s of silence, bounded by T1 | `v92::hold` | MOH Q2 |
| MHnack's own timeout | starts once MHreq is no longer heard | `v92::hold` | MOH Q3 |
| `+PMHT` default | 0 (deny) until hold is wired through | `crates/at` | CTL 5.1 |
| TR3 (1500 ms, no RTD term) | relaxed to 2.0 s + 2 RTD, as `SD_WAIT` already is, documented as a departure | `v92::analogue` | P3P 13.2 item 7 |
| SUVu bit 26 ("wait for my CPu before sending CPd") | our analogue modem always sends 0, because its CPu is ready as soon as TRN2u has run; a received 1 is honoured only while it costs nothing, which 9.6.1.1.2 makes a [MAY] | `v92::analogue`, `v92::digital` | P4A 3.6, P4D 7.2, 12.2 |
| SUVu bits 27:31 outside initial training | the level actually measured over the preceding data mode - 20*log10 of the RMS of G x the prefilter output - in signed Q2.2 clamped to -3.75..+3.75 dB; 16 ("not measured") is sent only in initial Phase 4, or if the tracker has nothing | `v92::analogue` | P4A 3.6, RRF 3.3 RR-AI-3, P4P 4.4 |
| SUV bit 32 (silence request) outside rate renegotiation | sent 0 in the `Training` and `FastExchange` contexts, and a received 1 is swallowed and never reported, because 9.8 is the only clause that defines a silent period | `v92::exchange` | P4P 10.3 Q15 |
| A reserved T1 (`0000`, `1110`, `1111`) in a received MHack | `MH_RESERVED_T1_IS_NO_GRANT = true`: not a grant, so the requester treats it as a refusal and takes the MHcda or MHfrr path | `v92::hold` | MOH Q11, 1.2.3 |
| Where the 2 s + RTD MH initiator timer starts | at the first bit of our first initiating sequence, which R1 already gates on the far Tone RT or an MH response; the wait for that gate has its own bound, `MH_RT_WAIT = 2 s + RTD` | `v92::hold` | MOH Q4, 2.3 R1 |
| MH glare (both ends initiate, or MHclrd crosses MHreq) | `MH_GLARE_YIELD_DIGITAL = true`: answer the received initiating sequence per Table 34 while our own continues, and at 2 s + RTD the digital modem abandons its own transaction | `v92::hold` | MOH Q9 |
| Does `+PMHT` alone grant a hold? | yes: a grant needs no DTE confirmation, so a hold works with no DTE attached. `+PMHR` issued while an incoming request is pending confirms it, `+PMHR=0` denies it, and the result `+PMHR: 14` means "denied, and denied for the rest of this session" | `crates/at` | CTL 5.1, 5.3, Q5, Q6 |
| Tone A guard level | keep -7 dB, as `dpsk.rs` does today | unchanged | P2S N-14, P2P A1 |

**One reading, one home.** Every reading above that is a value is a `pub const` in **exactly** the module
named in its Owner column, with its doc giving the clause and, where the reading is open, the alternative.
V92-01 creates only those whose owner is `v92::mod` (plus the generic signed and unsigned `Qa.b` helpers);
each of the others is created by the package that first needs it, in the module named here - for example
`TRN2U_SIGN_LAST` by V92-12 in `v92::up_signals`, `MH_RESERVED_T1_IS_NO_GRANT` by V92-25 in `v92::hold`.
No package defines a second copy of a helper or a constant that another package owns, because a duplicate
and an item nothing references are both risk R12 under `-D warnings`.

---

## 5. Work packages

Paths are relative to `F:\dialupmodem2\`. "Files" lists every file the package may create or edit; a file
not listed must not be touched.

### Wave 1: independent foundations

#### V92-01: Scaffold the `v92` module, its shared numbers, `Parameters` and `Deadlines` (L)

- **Depends on:** none.
- **Clauses:** 1 (items c, e); 3.5; 3.6; 3.8; 5; 6.1; 6.2; 6.4 (Figure 1); clause 8 preamble (bit order);
  Table 18 bits 12:17 and 18:24; 9.6.1.2.1; 9.6.2.2.1.
- **Files:** `crates/datapump/src/lib.rs`; `crates/datapump/src/v92/mod.rs` (new); and new stub files
  `crates/datapump/src/v92/{sequences, modulus, precoder, upstream, decoder, design, upchoice, up_signals,
  transmit, epsilon, receiver, anspcm, exchange, up_source, down_source, analogue, digital, hold, memo}.rs`.
- **Digests:** INTRO; P3P (8, timers TR5/TR6); P4P (6.2 D4-R1, 6.4 A4-R1, 8); CA (10, 11.1, 14);
  CD (8.1, 10, 12); CT (11.1, 12). (P3P 8 and P4P 8 are where the 20 s + 6 RTD start-up watchdog's value
  actually lives; INTRO stops at clause 8.1 and carries no clause-9 timer.)
- **Description:** add `pub mod v92;`. Each stub contains only a `//!` paragraph naming its clause and the
  package that will fill it, so that later waves edit disjoint files and clippy sees no unused private
  items. `mod.rs` holds:
  - frame constants `UP_INTERVALS = 12`, `CONSTELLATION_FRAME = 6`, `TRELLIS_FRAME = 4`;
  - the upstream ladder for drn 1..=19: `up_rate(drn) = (drn + 17) * 8000 / 6` (24 000 to 48 000 bit/s) in
    the same convention as `v90::rate_for`, and `up_bits(drn) = 2 * (drn + 17)`;
  - the 19-bit Ja mask helpers (mask bit k corresponds to drn k+1);
  - `MD_STEP_SYMBOLS = 276`;
  - `FILTER_TOTALS = [192, 256, 320, 384]`, `FILTER_EACH = [128, 192, 256, 320]`, and `FilterSections`
    decoded from INFO1a bits 12:13 (whether z1 and p2 are allowed);
  - segment lengths: Ru 384T, R-bar-u 24T, Su 144T, TRN1u minimum 2040T, B1u 48 frames, E2u 1 frame,
    Rf 384T with a 4-symbol sign period, R-bar-f 24T;
  - the start-up watchdog, 20 s + 6 RTD;
  - signed and unsigned `Qa.b` helpers with the section 4 reading;
  - `Parameters` (AD-3), with `Debug`, `Clone` and a `fits()` self-check;
  - `Deadlines`: slots of `pub type Deadline = (u64, &'static str)` held by name, with `arm`, `clear`,
    `clear_all`, `armed` and `expired(now)`, which reports the earliest slot that has passed. The name is
    a `&'static str`, not an enum variant, because a later package must be able to arm a timer this module
    has never heard of without editing `mod.rs`, which its own file list forbids - the same reason
    `PeerSuv`, `PeerCp` and `SequenceKind` are declared here. The cost is that a name is checked at run
    time, so a package that arms `"TR3"` and clears `"Tr3"` leaves the timer running; whoever writes the
    second of a pair asserts with `armed()`;
  - the plain flag structs and the sequence-kind enum that the side-agnostic `Exchange` of V92-14 is driven
    by, declared **here** because `exchange.rs` may not edit `mod.rs` and the plan's own rule forbids
    touching a file outside a package's list: `PeerSuv { ack, silence, wait_for_cp }`, `PeerCp { ack }` and
    `SequenceKind { Trn2, Suv, Cp, Cpus, E, B1 }`, all `pub`, with `Debug` and `Clone`;
  - the section 4 readings **whose Owner column names `v92::mod`**, as `pub const`s whose doc gives the
    clause and, where the reading is open, the alternative. The other readings belong to the modules
    section 4 names and are written by the packages that first need them, so no reading has two homes.
- **Tests:**
  - `the_upstream_ladder_runs_from_24000_to_48000_in_steps_of_8000_over_6` - 19 rungs, drn 1 is 24 000,
    drn 19 is 48 000.
  - `a_data_frame_holds_2_drn_plus_34_bits` - K runs 36..=72, always even, and
    `up_bits(drn) * 8000 / 12 == up_rate(drn)` for every drn, plus the exact
    `up_bits(drn) * 8000 == up_rate(drn) * 12` at the seven rungs where 3 divides drn + 17. The exact form
    does not hold at the other twelve, because `up_rate` floors as `v90::rate_for` does: at drn 2,
    38 x 8000 is 304 000 and 25 333 x 12 is 303 996.
  - `a_twelve_symbol_frame_holds_two_constellation_frames_and_three_trellis_frames` - j = i mod 6,
    k = i mod 4.
  - `the_q_formats_read_as_the_printed_digit_patterns` - 4G = 0x4000 gives G = 1/16; signed Q1.6 0x80 is
    -2.0; unsigned Q3.13 0x2000 is 1.0.
  - `md_length_counts_in_276_symbols` - 127 gives 35 052 symbols, and 276 is a multiple of 12.
  - `the_filter_limits_decode_from_info1a` - codes 0..3 give the tabled Ltot and Lmax.
  - `deadlines_report_the_earliest_that_has_passed`.

#### V92-02: The V.92 INFO sequences and the MH frame (M)

- **Depends on:** none.
- **Clauses:** 8.4, 8.4.1 (Tables 15-19); 8.9.2 (Tables 32, 33); 9.3; 9.4; V.34 10.1.2.3.1-10.1.2.3.2;
  V.90 8.2.3.2 (Tables 7-11).
- **Files:** `crates/datapump/src/v34/info.rs`; `crates/datapump/src/v34/dpsk.rs`;
  `crates/datapump/src/v34/phase2.rs` (new match arms only); `crates/datapump/tests/v90_vector.rs` and
  `crates/datapump/tests/v34_vector.rs` (new match arms only).
- **Digests:** P2S (5-10, 15, 16.2); P2P (2, 3); MOH (1.2); CD (6); CT (6, 9.1).
- **Description:**
  - **INFO0d** gains `v92` (bit 27) and `short_phase2` (bit 26). With both false the encoding is
    bit-identical to today's, which is the regression test.
  - **INFO0a** gains a V.92 view with the two bits **the other way round** (bit 26 = V.92, bit 27 = short
    request), exposed as `Info0::pcm_flags()` and kept off `Info0.clock`, which is V.34's transmit-clock
    field. Only PCM roles read it (P2S N-3).
  - **INFO1d**: `Info1c::pcm_upstream()` for bit 70, a way to write it on purpose, and the 3429 result read
    as 8 bits (71:74 pre-emphasis, 75:78 rate) (N-4).
  - **Table 18**: new `Info1aPcmUp { sections, ltot_code, lmax_code, md_length, uinfo }` with bits
    34:36 = 6 and 37:39 = 6, sending bits 32:33 = 0 and 40:49 all ones; UINFO must be 67..111 (N-24); the
    receiver ignores 32:33 and 40:49. Today `Info1aPcm::from_bits` rejects exactly this frame.
  - **Table 19**: `Info1aPcm.high_carrier` (bit 33), written only for Table 19; Table 10 writes 0.
  - **Classification**: the 70-bit INFO1a is classified by (37:39, 34:36) as in P2S 10.4; invalid
    combinations are dropped rather than guessed.
  - **MH**: new `Mh { indication, information }` on Table 32 (indication and information nibbles as
    leftmost-first patterns) with the Table 33 T1 codes and the MHclrd reasons, in the 40-bit INFO framing
    with the CRC over bits 12..19. An undefined indication gives `None`; a reserved cleardown reason is
    kept as "unknown"; and **a reserved T1 in an MHack - `0000`, `1110` or `1111` (Table 33) - decodes as
    `T1::Reserved(code)`, never as a duration**, so nothing downstream can mistake it for a grant. The
    Recommendation says nothing about reserved T1 codes (MOH Q11); section 4 fixes the reading and V92-25
    acts on it by treating such an MHack as a refusal.
  - **dpsk**: `Receiver::with_mh()` opts either side in to 40-bit frames; the default is unchanged.
- **Tests:**
  - `a_v92_info0d_says_so_in_bit_27_and_asks_for_short_phase_2_in_bit_26` - the P2S vector (CRC 0xDB49) and
    the P2P vector (0xA8A9) round-trip bit-exact.
  - `a_v92_info0a_has_the_two_bits_the_other_way_round` - CRC 0xAF5A, and 0x2B52 with bit 28 set.
  - `a_v90_info0d_still_sends_zeros_in_bits_26_and_27` - byte-identical to the pre-change encoding.
  - `a_v90_peer_reads_a_v92_info0_as_v90` - a V.90 decoder is unaffected.
  - `info1d_bit_70_reads_as_pcm_upstream_and_the_3429_field_keeps_its_place` - CRC 0x086C.
  - `a_table_18_info1a_round_trips_and_is_not_thrown_away` - CRC 0xA858 and 0xF59A, every Ltot and Lmax code.
  - `a_table_19_info1a_carries_the_high_carrier_in_bit_33` - CRC 0x6742 and 0x7A52.
  - `the_receiver_tells_the_info1a_layouts_apart_by_bits_34_to_39` - Tables 10, 11 and 18 decode; 6 with
    0/1/2/7, and 7 with anything, are dropped.
  - `a_table_18_frame_is_never_read_as_a_frequency_offset`.
  - `every_mh_sequence_matches_its_worked_vector` - the ten MOH 1.2.5 vectors and the 16 MHack CRCs, each
    leaving residue 0.
  - `an_undefined_mh_indication_is_ignored_and_a_reserved_cleardown_reason_is_kept`.
  - `a_reserved_t1_code_in_an_mhack_is_not_taken_as_a_grant` - `0000`, `1110` and `1111` decode as
    `T1::Reserved`, the thirteen defined codes decode as their printed durations (10 s through "no limit"),
    and no reserved code yields a duration or an unbounded hold.
  - `mh_frames_are_heard_back_to_back_only_when_asked_for`.
  - Existing `info.rs` and `dpsk.rs` tests, `v90_vector` and `v34_vector` pass.

#### V92-03: V.92 CP and descriptor layouts, and the V.92 finders, in `v90/sequences.rs` (M)

- **Depends on:** none.
- **Clauses:** 8.5.1 (Table 23); 8.5.4 (Table 20); 8.6.2 (Jd bit 47); 8.7.3; V.90 8.3.1 (Table 12) and
  Table 14; V.34 10.1.2.3.2; clause 8 bit order.
- **Files:** `crates/datapump/src/v90/sequences.rs`.
- **Digests:** P3S (3.4, 4.1, 4.4, 5.2, 9.2); P3P (3.4, 3.6); P4A (3.3); CD (4).
- **Description:** all additions are additive; V.90's own reading of every sequence is unchanged.
  - Make `frame`, `unframe`, `put`, `get` and the sync and block constants `pub(crate)` so `v92::sequences`
    builds on framing already proved against live servers.
  - **CP layouts.** `Layout { V90, V92 }`, with `Cp::to_bits_in(layout, pad_unit_bits)` and
    `Cp::from_bits_in(layout, bits)`. The V.92 layout: bit 18 = 0 (bit 18 = 1 is rejected); a 2-bit type at
    19:20 (0 = CPt, 1 = CPu, 2 = CPus, handled in V92-11); drn at **21:25** (not V.90's 20:24); bits 26:30
    reserved; no silence bit and no upstream rate mask, so 36:48 are reserved too. **The V.92 layout
    writes zeros into bits 26:30 and 36:48 on transmission and ignores them on reception**, which is what
    Table 23 asks for ("set to 0", not interpreted) - so `silence` reads back as false and
    `upstream_rates` as `None` whatever arrives, and a far end using a future ITU extension, or leaving
    stale bits set, still connects. Only bit 18 and the type field at 19:20 are dispatch fields and may be
    refused. Bits 31 onward as V.90; one fill bit, then zeros to the pad unit.
    `cp_length` and the mask code are shared.
  - **Descriptor.** `Descriptor.upstream_rates: Option<u32>` (19 bits); `Some` selects the V.92 layout:
    two mask words, then a start bit, the CRC and one fill bit at 221+P..238+P, then zeros to a multiple of
    12 bits, giving `descriptor_length` + 2 blocks.
  - **Guard.** A V.92 reader must dispatch on bit 47 before anything else, or a Jp reads as a Jd full of
    nonsense rates with `sixteen_in_training` set. Add an acceptance predicate parameter so `v92` can
    require bit 47 = 0 for Jd and 1 for Jp. V.90's own `Jd::from_bits` keeps today's behaviour.
  - **Finders.** `DescriptorFinder::v92()` and `CpFinder::v92(pad_unit)` accept a 0 after **at least** 17
    ones and take the last 17 as the sync, because Ja's first descriptor and the first CPt sit behind
    24-one preambles and today's exactly-17 rule drops them, costing a repetition on every group (P3S 9.2).
    The V.90 finders are untouched.
- **Tests:**
  - `a_v92_cpt_has_its_type_at_19_and_drn_at_21` - drn 16, masks with Ucodes 0..79, CRC 0xAC4D, 300 bits,
    the tail as printed in P3S 4.1.
  - `a_v92_cpu_with_drn_22_puts_0_1_1_0_1_in_bits_21_to_25`.
  - `mask_bits_sit_where_table_23_puts_them` - u 0 at 137, 15 at 152, 16 at 154, 127 at 271.
  - `cp_lengths_follow_gamma_and_delta_for_each_pad_unit` - 290/426/970/1786 unpadded, and the 12-, 24- and
    36-bit pad tables of P4A 2.4 and P3S 4.1.
  - `a_v92_cpt_is_not_misread_through_the_v90_layout_or_back` - pins the doubled-drn hazard of CD 4.4.
  - `reserved_bits_set_by_a_far_end_are_ignored_not_rejected` - the reserved runs of Table 23 (26:30,
    36:48, 129:135) swept with all ones, and the same for the descriptor's reserved bits: every sequence
    still parses, and every interpreted field is identical to the all-zeros case. Nothing is rejected for a
    reserved bit.
  - `a_v92_descriptor_with_no_dil_is_276_bits` - CRC 0xB71C with all 19 upstream rates enabled.
  - `v92_descriptor_lengths_pad_to_12_bits` - N = 1 gives 300; N = 64 gives 828; N = 255 with L = 128 gives
    2688.
  - `a_frame_behind_forty_one_ones_is_found_by_the_v92_finder` - and the V.90 finder still skips it.
  - `a_jd_with_bit_47_set_is_refused_by_the_v92_predicate` - the Jp vector of P3S 5.3.
  - Existing tests pass: the real-server Jd CRC 0x776E, the real 1736-bit descriptor, the CP round trips.

#### V92-04: Network, part A: the upstream A/D phase, per-direction impairments, a tabulated kernel (M)

- **Depends on:** none.
- **Clauses:** 6.2 (the network clock); 8.6.3 (the A/D phase is fixed); 8.5.7. The rest is a model.
- **Files:** `crates/datapump/src/v90/network.rs`.
- **Digests:** CD (9, 9.2 N1-N11); CT (2.4, 5.2, 7).
- **Description:** every default stays exactly as it is, so the V.90 tests are bit-identical.
  - Tabulate the upstream windowed-sinc kernel per fractional phase instead of recomputing sin/cos per tap
    per tick. This is the CI-runtime lever.
  - `with_upstream_phase(fraction_of_t)`: a fractional A/D sampling instant, with a fractional lag. Today,
    at fs = 16 000 with no skew, every A/D instant lands on an analogue sample, so epsilon would always be
    zero and Su and Jp would never be exercised.
  - `Direction { Down, Up }`; `with_slips_in(dir, seconds, inserted)`, `with_slip_at_in(dir, ..)`,
    `with_slip_length(codewords)` (default 160); `slips_down()`, `slips_up()`; `slips()` stays
    downstream-only.
  - `with_upstream_robbed_bit(phase)`, its phase independent of the downstream one.
  - `with_pads(down_db, up_db)`, `with_delays(down, up)`, `with_upstream_noise(level)`, a configurable
    upstream anti-alias cutoff and `up_gain`.
  - `up_code() -> (ucode, positive)` for the last upstream sample, so upstream tests are exact in Ucodes.
- **Tests:**
  - `the_tabulated_upstream_kernel_gives_the_same_levels_as_before` - exact, against a copy of the old
    computation on random input.
  - `a_fractional_upstream_phase_samples_between_our_samples` - a known ramp at phases 0, 0.25, 0.5, 0.75.
  - `the_upstream_codeword_is_the_level_we_meant` - at phase 0 with no noise, every codeword out equals the
    codeword in.
  - `an_upstream_robbed_bit_moves_codewords_only_in_its_own_octet_of_six`.
  - `an_upstream_pad_scales_every_level`.
  - `an_upstream_slip_moves_everything_after_it_by_the_slip_length`, and
    `a_ten_millisecond_cut_moves_everything_by_80_codewords`.
  - `unequal_legs_delay_each_direction_by_its_own_amount`.
  - The four existing network tests and all of `v90_call.rs` pass unchanged.

#### V92-05: Move the V.90 downstream reading into `v90/downstream.rs`, with `RWatch` generalised (M)

- **Depends on:** none.
- **Clauses:** V.90 8.4.1-8.4.3 and 8.6.4 by reference; V.92 8.6.1 and 8.6.5 (DIL and Ri unchanged).
- **Files:** `crates/datapump/src/v90/analogue.rs`; `crates/datapump/src/v90/downstream.rs` (new);
  `crates/datapump/src/v90/mod.rs`.
- **Digests:** CA (6, 8, 11.1); CT (4.2, 5).
- **Description:** a behaviour-preserving extraction, committed on its own, so the V.92 analogue modem uses
  this code rather than copying it. No constant and no timing changes. `pub(crate) mod downstream;`, and in
  `v90/mod.rs` **`mod carrier;` becomes `pub(crate) mod carrier;`** so that `datapump::v92` can name
  `carrier::Watch` at all (AD-1 assumes it can; today the module is private, so V92-41, V92-42 and V92-55
  could not reach it). Nothing inside `carrier.rs` changes. What moves: `JdReader`, which gains a
  **caller-supplied acceptance closure over the raw framed bits**, `accept: impl Fn(&[bool]) -> bool`,
  applied *before* `Jd::from_bits` is called at all - V.90 passes `|_| true`, and V.92 will pass
  `|bits| !bits[47]`, so a Jp is never decoded as a Jd full of nonsense rates. The closure needs nothing
  from V92-03, which is what keeps this package a pure move with no dependency inside its own wave; the
  bit-47 knowledge stays wholly in V92-03 and V92-11. `RWatch` and `RSeen`, with the levels supplied by the caller **and the pattern period made a
  parameter** (6 for R, Ri, Rd and Rt; 4 for Rf), so V.92 needs no second copy; `Levels`, `levels_for`,
  `least_gap`, `nearest`, `slicer_for`; `Trust` and `trusted_symbols`; `find_place`; `Frames`, with a
  pluggable sequence finder; and a `DilReader` owning the `dil*` state, `before_dil` and `analysis`, with
  `begin_dil`, `find_dil_start`, `fit_dil`, `dil_levels`, `dil_symbol`, `count_dil` and `find_dil`, each
  taking `&mut pcm::Receiver` instead of reaching into `self.rx`.

  This is the one package that touches slip-sensitive code live calls depend on. It is a pure move; if it
  goes wrong it is a single revert.
- **Tests:**
  - Every existing V.90 unit test, all of `v90_call.rs` (including every slip and DIL-relocation test) and
    `v90_vector.rs` pass unchanged; the two J-prime-d tests move into `downstream.rs`.
  - `an_r_watch_finds_a_four_symbol_pattern_too` - a synthetic plus-plus-minus-minus sequence in both
    polarities.
  - `a_jd_reader_with_a_callers_acceptance_closure_ignores_frames_it_rejects` - a frame with bit 47 set is
    refused by a `|bits| !bits[47]` closure and accepted by `|_| true`, and the refused one never reaches
    `Jd::from_bits`.
  - `the_carrier_watch_can_be_named_from_outside_v90` - a compile-level check that
    `crate::v90::carrier::Watch` resolves from a sibling module.
  - `an_r_watch_takes_its_levels_from_the_caller`.
  - Run the ignored DIL slip sweep before and after and record the same failure count (17 of 260) in the
    commit message.

#### V92-06: `pcm::Receiver::symbol_clock()`, for slaving the upstream (S)

- **Depends on:** none.
- **Clauses:** 6.2.
- **Files:** `crates/datapump/src/v90/pcm.rs`.
- **Digests:** CA (11.3 "Clock slaving", 13.2).
- **Description:** `pub fn symbol_clock(&self) -> SymbolClock`, giving the line samples one far-end symbol
  takes, smoothed from `drift` rather than from `due`. `due` steps by up to a quarter half-symbol in
  `hold_centre` and on every timing-loop update; an upstream transmitter that followed it would jitter at
  the far A/D. Nothing else in `pcm.rs` changes.
- **Tests:**
  - `the_symbol_clock_follows_a_network_120_ppm_fast` - over 10 s of `Network::with_clock(120)` the
    reported period is within 1 ppm of the truth.
  - `the_symbol_clock_does_not_jump_when_the_timing_loop_steps` - the sample-to-sample change stays under a
    stated bound while `due` steps.
  - The existing `pcm.rs` tests pass.

#### V92-07: The V.8 quick-connect sequence codec (S)

- **Depends on:** none.
- **Clauses:** 8.2 (V.21 channels); 8.2.1 (Table 2); 8.2.3 (Table 4); 8.3.2 (Table 11); 8.3.4 (Table 13);
  clause 8 bit order; 9.10.2.1 (U_QTS `1111`); V.8 clause 5 and Table 1.
- **Files:** `crates/v8/src/quick.rs` (new); `crates/v8/src/lib.rs` (the `pub mod quick;` line only).
- **Digests:** P1A (5, 12.3); P1D (4, 6, 9); P1P (3.2, 3.4, 3.5); CTL (2.2, 3.1); CV (5.1, 6.2.1, 8 R1).
- **Description:** the four V.8-framed quick-connect sequences as data, with no line behaviour.
  - `SYNC_QC = 0x55` (the V.8 Table 1 sync reserved for V.92).
  - `Kind { Qc1a, Qca1a, Qc1d, Qca1d }`, with the V.21 channel each uses.
  - `Uqts`: the 15 U_QTS codes with their Ucodes, or `Cleardown` for `1111`, to and from the WXYZ pattern
    (W first).
  - `AnspcmLevel`: the LM pattern (L first), its dBm0 and its scl.
  - `Qc { kind, lapm, field }` with `octet()` and `from_octet()` (b4 = 0; the digital variants also need
    b3 = b5 = 0) and `bits()` for the whole 60 or 70 bits, ten-ONE runs included.
  - `BitWatcher::feed(bit) -> Option<Qc>`: at least ten ones, then `0101010101`, then a framed octet with
    start 0 and stop 1, then ones, the sync again and an equal second copy. It accepts at bit 59. It is
    kept separate from `v8::Decoder`, whose sync logic would misread some QC octets.
- **Tests:**
  - `every_table_2_code_names_its_ucode_and_1111_is_cleardown`.
  - `lm_codes_name_the_four_anspcm_levels`.
  - `qc1a_with_p_and_wxyz_0101_is_the_printed_60_bits_and_octet_0xa4`.
  - `qca1a_is_70_bits_ending_in_ten_ones_and_octet_0xa6`.
  - `qc1d_and_qca1d_carry_lm_where_tables_11_and_13_put_it` - all 16 printed strings; 0x05, 0x85, 0x87, 0xC3.
  - `a_cleardown_qc1a_is_octet_0xec`.
  - `a_qc_body_equal_to_a_v8_sync_is_still_a_qc` - bodies 0x00 and 0xE0.
  - `a_cm_carrying_0x55_as_an_extension_octet_is_not_a_qc`.
  - `a_qc_whose_two_copies_differ_is_not_accepted`.

#### V92-08: V.42: skipping the detection phase at both ends, and suspend and resume (S)

- **Depends on:** none.
- **Clauses:** V.92 9.2.5, 9.3.1; V.42 7.2.1.2, 7.2.1.3, 7.10, 7.11, Appendix VI.2; V.8 7.3/7.4 NOTE.
- **Files:** `crates/ec/src/stack.rs`; `crates/ec/src/detect.rs`; `crates/ec/src/lapm.rs`.
- **Digests:** CTL (6.2, 6.3, 7.4); P2P (5, P10); P1P (5.5); CV (6.5).
- **Description:**
  - `Stack::bypassing_detection()` for the answerer: no ODP wait, no ADP, take flags or an LAPM frame as
    the start of the protocol phase; an ODP that does arrive anyway is still answered with an ADP, because
    V.8 itself warns that some equipment declares LAPM and still needs the detection phase. Document
    `without_detection()` as the originator form.
  - `suspend()` and `resume()`: freeze T400 during detection and T401 once connected (T402 and T403 are not
    implemented), without releasing the link, so sequence numbers, windows and the V.42 bis and V.44
    dictionaries survive a hold or a long retrain.
- **Tests** (inline in `stack.rs`):
  - `two_ends_that_both_skip_detection_reach_lapm_with_no_odp_or_adp_on_the_wire`.
  - `an_answerer_that_skips_detection_still_answers_an_odp`.
  - `a_suspended_link_keeps_its_timers_still_through_sixty_seconds_of_nothing`.
  - `after_resume_the_link_carries_on_with_the_same_sequence_numbers_and_dictionary`.
  - `ec/tests/loopback.rs` and the modem-crate call tests pass unchanged.

### Wave 2: codecs, signal blocks and the Phase 2 flags

#### V92-09: The 12-interval modulus encoder and decoder (S)

- **Depends on:** V92-01.
- **Clauses:** 6.4.1.
- **Files:** `crates/datapump/src/v92/modulus.rs`.
- **Digests:** INTRO (6.4.1, P-1, P-11, 9.3-9.5); CD (7.7).
- **Description:** `Moduli12` with its product in `u128` (M reaches 255^12, about 2^96) and `fits(k)`
  checking 2^K <= M. `Encoder { d_prev }` runs the five steps of 6.4.1 including the sign `s = (2R > M-1)`,
  the differential `d(f) = s(f) ^ d(f-1)`, and step 4's `d(f-1)` reading from section 4. `Decoder { d_prev }`
  rebuilds R0, R, s and d, keeping d from **R**, not from R0's half, because those differ when M is odd and
  R = (M-1)/2. Both have `reset()`, which zeroes d, because 8.7.1 zeroes the memories before B1u.
- **Tests:**
  - `random_frames_round_trip_for_even_and_odd_products` - 2000 frames each, K = 36..=72, including
    R = 0, R = M-1 and R = (M-1)/2.
  - `the_middle_value_of_an_odd_product_keeps_the_sign_chain`.
  - `an_inverted_channel_decodes_with_the_decoder_started_inverted` - mapping every Ki to Mi-1-Ki and
    inverting the decoder's d recovers the same bits, which is why the differential step exists.
  - `a_frame_that_does_not_fit_is_refused`.
  - `the_memory_starts_at_zero` - the first frame after a reset matches a hand-worked value.
  - `seventy_two_bits_need_u128` - M = 255^12.

#### V92-10: Precoder, prefilter, constellations, the inverse map and the 4T trellis (M)

- **Depends on:** V92-01.
- **Clauses:** 6.4.2; 6.4.3; 6.4.4; 8.8.3 NOTE; V.34 9.6.3.1 (Figure 9, Table 13).
- **Files:** `crates/datapump/src/v92/precoder.rs`.
- **Digests:** INTRO (6.4.2-6.4.4, P-2..P-5, P-12); P4D (5.2, 5.4); CD (7.6, 7.8).
- **Description:** the clause 6.4 chain below the modulus encoder, as one `Chain` that both modems use
  (AD-6), built from `Parameters` and never from `Cpd`.
  - `Constellation`: ascending positive magnitudes, mirrored to N = 2·LC points, `a(eta) = P[eta]` for
    eta >= 0 and `-P[-eta-1]` for eta < 0.
  - Equivalence classes: for k < 3, eta = Ki + z·Mi; for k = 3,
    eta = 2·Ki + 2·z·Mi + ((eta0 + eta1 + eta2 + Y0) mod 2) with a non-negative mod. Feasibility is
    N >= Mi and, at k = 3, N >= 2·Mi; an infeasible parameter set is **reported**, never panicked on.
  - Point selection: minimise |x(n)| by binary search around -c(n); ties to the smaller |eta| (section 4).
  - Precoder `x(n) = u(n) + sum z1(k)·u(n-k) + sum p1(k)·x(n-k)`; prefilter
    `v(n) = sum_{k=0..LZ2-1} z2(k)·x(n-k) + sum p2(k)·v(n-k)`; output G·v(n). All state is f64 and is never
    saturated; only the output is bounded.
  - Inverse map through `v34::trellis` on the coordinates (2·eta + 1), giving Y1..Y4, and
    `v34::trellis::Code` clocked once per 4-symbol trellis frame with its inputs masked to the code, 16, 32
    or 64 state chosen by `Parameters::trellis`.
  - `reset()` zeroes every memory.
- **Tests:**
  - `with_no_filters_the_output_is_the_chosen_level` - z1 = p1 = [], z2 = [1], p2 = [], G = 1.
  - `the_prefilter_feed_forward_starts_at_kappa_0_and_the_precoder_s_at_1` - the one-tap indexing
    difference of pitfall P-5.
  - `the_four_indices_of_a_trellis_frame_have_the_parity_the_encoder_asked_for` - over 5000 random frames.
  - `a_label_s_low_bit_is_the_parity_of_the_index_sum` - over eta in -130..=130.
  - `the_precoder_output_stays_bounded_over_a_long_run` - with a real feedback section, max |x| over
    100 000 symbols is under a stated multiple of the top level.
  - `a_class_with_no_member_is_reported` - N < Mi, and N < 2·Mi at k = 3.
  - `an_inverse_channel_gives_back_every_k_as_eta_mod_m`.
  - `the_memories_are_zero_before_b1u` - the first 12 outputs after a reset are a fixed vector.
  - `the_state_is_never_saturated_only_the_output`.

#### V92-11: The V.92 framed sequences (M)

- **Depends on:** V92-01, V92-03.
- **Clauses:** 8.5.4 (Table 20); 8.6.2-8.6.4 (Tables 21, 22, and Jp-prime); 8.7.3 (Tables 23, 24);
  8.7.4 (Tables 25, 26); 8.7.5 (Table 27); 8.8.3 (Table 30); 8.8.5 (Table 31); 9.11;
  V.34 10.1.2.3.2.
- **Files:** `crates/datapump/src/v92/sequences.rs`.
- **Digests:** P3S (5.2-5.4); P4A (0.4, 3, **5.4 cleardown**); P4D (2, 5, 7); RRF (2.5, 2.8-2.11);
  P4P (3, 4); **MOH (3, clause 9.11)**; INTRO (3.5).
- **Description:** every V.92 wire format, on `v90::sequences::{frame, unframe, put, get}` and
  `v34::info::crc`.
  - **`J`** dispatches on bit 47 *before anything else*: `Jd` (the V.90 rate mask and look-ahead, bits 47
    and 48 = 0) or `Jp` (epsilon as u16 in bits 18:33, `eight_in_training` bit 48, `eight_in_renegotiation`
    bit 49). `JP_PRIME_BITS = 12` zeros.
  - **`Suvd { silence, ack }`** (Table 31) and
    **`Suvu { wait_for_cpu, level, silence, ack }`** (Table 27), 52 bits before padding; `level` is signed
    Q2.2 clamped to +/-3.75, where 16 means "not measured".
  - **`Cpus { drn, ack }`**: type 2, CRC over bits 18:33.
  - **`Cpd`**: drn, trellis, `extend_e2u` (bit 29), ack, `gain4` (never zero), and three optional parts -
    `moduli: Option<[u8; 12]>`, `filters: Option<{ z1, p1, z2, p2 }>` and
    `sets: Option<{ index: [u8; 6], points: Vec<Vec<u16>> }>`. Sequential `to_bits(pad_unit)` and
    `from_bits` through the part walker of P4D 5.3, never an absolute offset beyond bit 50, with start bits
    landing on multiples of 17. Plus `check(limits, up_bits)` for DC-1..DC-7 and the 8.8.3 SHALLs (no zero
    point, non-empty sets first, class feasibility, sections, Ltot and Lmax, N >= M and N >= 2M at k = 3,
    2^K <= product M), and `merged_over(previous)` for absent parts.
  - **`From<&Parameters> for Cpd`** and **`TryFrom<&Cpd> for Parameters`** (AD-3).
  - **`CpFamily`** dispatch on bit 18 (SUV = 1, CP = 0) and the type at 19:20; CPt and CPu go through the
    V92-03 V.92 layout.
  - **A stream finder** for SUV and CP, tolerant of long runs of ones, with a length callback: CPd header
    words then counts, `cp_length` for CPu.
  - **Pad helpers** for 24, 36 and K bits upstream and D bits downstream.
  - **`rm_k(i, m, prime)`**: Tables 25 and 26 with the interval-11 errata reading (row 11 is K11).
  - **Q formats**: V92-01's signed and unsigned `Qa.b` helpers are used as they stand, at Q0.15, Q1.14,
    Q0.16 (G = raw / 262 144), Q3.13, Q1.6 and Q2.2. This package defines no Q helper of its own, so the
    reading has one home and `v92/mod.rs` is not touched in wave 2.
- **Tests:**
  - `jd_vectors_check_and_say_jd_in_bit_47` - CRC 0x776E (look-ahead 1) and 0xF366 (look-ahead 3).
  - `jp_carries_epsilon_in_bits_18_to_33_and_the_trn2u_sizes_in_48_and_49` - 0x1F4C, 0x3E4E, 0x70A6.
  - `a_jp_is_never_read_as_a_jd_and_back`.
  - `suvd_vectors_match` - 0xE960, 0x6D68, 0xAB64, 0x2F6C.
  - `the_smallest_cpd_is_69_bits` - CRC 0x570B, and the CPd-prime form 0x5BE7.
  - `a_cpd_with_every_part_round_trips_through_parameters_and_keeps_start_bits_on_multiples_of_17` -
    moduli, up to 384 coefficients, six sets, back to the same `Parameters`.
  - `an_absent_part_moves_everything_after_it`.
  - `cpd_limits_are_checked` - Ltot, Lmax, an unsupported section, a zero point, an empty set before a full
    one, an index past the sets, N < M or N < 2M at k = 3, 2^K > M.
  - `suvu_level_is_signed_q2_2_and_16_means_none`.
  - `cpus_is_type_2_with_its_crc_over_bits_18_to_33`.
  - `sequences_pad_to_24_36_k_or_d_bits` - 52 bits becomes 72 at both TRN2u sizes.
  - `the_cp_family_is_told_apart_by_bit_18_and_type`, and `a_v90_cp_is_not_read_as_a_v92_cp`.
  - `rm_and_rm_prime_follow_tables_25_and_26`.
  - `a_cleardown_cpd_needs_no_parts`, and the drn = 0 forms of CPu/CPus bits 21:25 and of CPd bits 22:26
    (9.11, with the P4A 5.4 erratum that the printed text says "SUVu", which has no drn field).
  - `reserved_bits_set_by_a_far_end_are_ignored_not_rejected` - the reserved runs of Table 21 (41:46, 48),
    Table 22 (35:46, 50), Table 23 (26:30, 36:48, 129:135), Table 27 (19:25), Table 30 (30:32) and
    Table 31 (19:31), each swept with all ones: every sequence still parses to the same interpreted fields
    as the all-zeros case, and none is refused.

#### V92-12: Two-level and TRN2u upstream signals, senders and readers (M)

- **Depends on:** V92-01.
- **Clauses:** 8.5.1, 8.5.2, 8.5.4-8.5.7; 8.7.2; 8.7.6 (Tables 28, 29); 6.3; 3.8; V.34 clause 7.
- **Files:** `crates/datapump/src/v92/up_signals.rs`.
- **Digests:** P3S (3.1-3.3, 4.2, 4.5-4.7); P3P (1.5, 3.1-3.3); P4A (2.1, 2.2, 3.7); P4P (4.2); RRF (2.7).
- **Description:** every upstream signal that bypasses the precoder, as sources and as readers, in LU units.
  - `Trn1u`: GPA (`v32::Scrambler`, the polynomial chosen by role, never by who dialled) fed ones, the
    register zeroed **before each TRN1u segment**, output 0 mapping to +LU, the opposite of the downstream
    convention.
  - `ru(n, bar)`: `{+L, +L, +L, -L, -L, -L}` and its inverse; `su(n, bar)`: `{+a, 0, +a, -a, 0, -a}` with
    a = sqrt(1.5)·LU, so Su carries the same power as TRN1u.
  - `TwoPointSender`: GPA then differential, seeded from the last symbol sent, with the 24-one preamble and
    a bit queue; used by Ja, CPt and E1u. `TwoPointReader` decodes polarity-blind differentially, then
    descrambles, with a plain-to-differential switch that replays the symbol it switched on.
  - `Trn2uSender { size, first_bit_is_lsb }`: scrambler reset at the start, only the **sign** bit
    differential from a caller seed, magnitudes (2m+1)/sqrt(5) for 4 points or (2m+1)/sqrt(21) for 8, and
    the bit order behind the named constant of section 4. `Trn2uReader` matches it.
  - Segment-length constants 384, 24, 144, 2040, and `UP_SEQUENCE_UNIT = 12`.
- **Tests:**
  - `trn1u_starts_with_the_gpa_signs_the_digest_lists` - the first 48 signs equal the P3S vector.
  - `ru_and_its_bar_are_the_printed_patterns`, and `su_has_the_same_power_as_trn1u` with lines at
    1333.3 Hz and 4000 Hz.
  - `a_two_point_sequence_decodes_whatever_the_line_polarity`, round-tripping Ja, CPt and E1u including the
    preamble and the differential seed.
  - `twenty_four_ones_let_the_far_descrambler_lock_before_the_sync`.
  - `trn2u_has_mean_square_lu_squared_at_both_sizes`.
  - `trn2u_descrambles_to_ones_and_a_following_sequence_parses_from_its_first_zero`.
  - `the_trn2u_bit_order_is_one_switch` - sender and reader agree at both settings of `TRN2U_SIGN_LAST`,
    and the two settings differ, so flipping it after a capture is a one-line change.
  - `e1u_is_twelve_zeros_and_is_told_from_another_cpt`.

#### V92-13: ANSpcm, QTS and TONEq (M)

- **Depends on:** V92-01.
- **Clauses:** 8.3.1 (Tables 6-10); 8.3.6; 8.2.5; 9.2.1.3, 9.2.2.3, 9.2.3.3, 9.2.4.3; 9.8.1.1.3;
  V.90 Table 1.
- **Files:** `crates/datapump/src/v92/anspcm.rs`.
- **Digests:** P1D (3, 8, Appendix A); P1A (7, 10.3, 10.4, 11); P1P (3.6-3.9).
- **Description:**
  - The Appendix A octet tables as consts, four levels by two laws, 301 octets each. The generator
    (`floor(scl·sqrt(2)·cos(2·pi·k·79/301 + theta) + 0.5)` in f64, with the exact G.711 decision intervals)
    is kept only as a test, because it reproduces all 2408 tabled octets.
  - `anspcm_octet(n)`: XOR 0x80 in every other 3612-symbol block; the reversal lands on a period and a
    frame boundary, table polarity first (section 4).
  - `qts_octet(n, uqts)`: 128 repeats of the six-symbol pattern then 8 inverted ones, 768 + 48 symbols,
    with +0 and -0 distinct octets, and the first QTS symbol in data-frame interval 0.
  - `silence_octet(law)`: constant positive Ucode 0.
  - `ToneqGenerator`, exactly 980 Hz, and `ToneqDetector`: a steady 980 Hz for at least 60 ms with no
    1180 Hz energy, so V.21(L) marking cannot trigger it.
  - `AnspcmWatch` for the analogue side: 2100 Hz **without** the 15 Hz AM (using `v8::ansam`), told from
    V.25 ANS by the QTS burst that precedes it, and reporting the QTS reversal time.
- **Tests:**
  - `the_generator_reproduces_all_2408_tabled_octets`, and `table_7_k82_a_law_is_08` (the page prints "8").
  - `a_reversal_flips_only_the_polarity_bit_every_3612_symbols_and_lands_on_a_frame_boundary`.
  - `anspcm_power_is_the_level_lm_names` - within +/-0.2 dB of the P1D ANS-17 table.
  - `qts_puts_its_first_symbol_in_interval_zero_and_reverses_at_symbol_768`.
  - `the_qts_reversal_is_found_to_within_a_symbol` - through `Network::down`.
  - `toneq_is_heard_after_60_ms_and_v21_marking_is_not_taken_for_it` - including the 14-bit mark run inside
    a QC1a with WXYZ = 1111.
  - `anspcm_is_told_from_ansam_by_the_missing_15_hz_and_from_ans_by_the_qts_burst`.

#### V92-14: The SUV/CP/E acknowledge exchange (M)

- **Depends on:** V92-01.
- **Clauses:** 9.6.1.1.1-9.6.1.1.6; 9.6.2.1.1-9.6.2.1.6; Figures 12-14; 9.8.1.1.2 and 9.8.2.1.3 (entry);
  9.9.1.1.2 and 9.9.2.1.2 (entry); Table 30 bit 29; the grouping rules of 8.7.3, 8.7.5, 8.8.3 and 8.8.5.
- **Files:** `crates/datapump/src/v92/exchange.rs` only. `PeerSuv`, `PeerCp` and `SequenceKind` are
  declared by **V92-01** in `v92/mod.rs` (wave 1), so this package writes no file outside its list.
- **Digests:** P4P (3, 5, 6.5); RRF (5); P4D (10.1, 12.2); P4A (3.6, 5.1).
- **Description:** the side-agnostic machine of P4P 6.5 (AD-9). It is driven by the plain flag structs and
  the sequence-kind enum that V92-01 declares in `v92/mod.rs` (`PeerSuv { ack, silence, wait_for_cp }`,
  `PeerCp { ack }`, `SequenceKind`), so it has **no dependency on the wire codec** and touches no file but
  its own.
  - **Context:** `Training`, `Renegotiation` or `FastExchange`, which decides the modulation the owner will
    use, whether CPd bit 29 may be set, whether FB1u precedes B1u, and what happens to the silence bits.
    **Only `Renegotiation` may send SUV bit 32 or act on a received one**: in `Training` and
    `FastExchange` the machine sends bit 32 = 0 and swallows a received 1 without reporting it to the
    owner, because 9.8 is the only clause that defines a silent period and the owners in those contexts
    (V92-39, V92-40) have no silence path at all until V92-51 (section 4).
  - **State:** `sent_ack`, `got_peer_cp`, `peer_acked`, `my_cp_end`, `repeat_cp`, `need_single_cp`, RTD.
  - **Events:** `peer_suv`, `peer_cp` (a CPus counts as a CP), `peer_e`, `sequence_started(kind) -> ack`,
    `sequence_ended(kind, at)`, `now`.
  - **Rules:** one CP, sent after the first peer SUV; the ack bit set on every later sequence once a peer
    CP has arrived, and changed only at a sequence boundary; a CP repeated only when no acknowledgement has
    been seen in any sequence whose *reception completes* by own-CP-end + 100 ms + RTD (section 4); E only
    once an acknowledged sequence has been sent and an acknowledged sequence or the peer's E has been
    received, after finishing whatever is in flight; every sequence in a group identical apart from bit 33.
  - In `Renegotiation`, silence requests (SUV bit 32) and grants (bit 33) are reported to the owner, with
    the ack state cleared when the silence ends.
  - A peer SUV with `wait_for_cp` set (Table 27 bit 26) holds our own CP back until the peer's CP has
    arrived **or** the CP repeat window expires, whichever comes first, and never holds back an SUV, an E
    or a B1. The bound is what makes complying free, which is the condition 9.6.1.1.2 attaches to its
    [MAY].
- **Tests:**
  - `figure_12_crossing_cps_end_in_ed_and_e2u` - a scripted trace reproduces SUVd SUVd CPd SUVd' SUVd' Ed
    against SUVu SUVu CPu SUVu SUVu' SUVu' E2u.
  - `figure_13_a_cpu_heard_first_makes_the_single_cpd_a_cpd_prime`.
  - `figure_14_a_lost_cpu_is_repeated_after_100_ms_and_a_round_trip`.
  - `the_sequence_that_completes_after_the_deadline_still_counts`.
  - `no_second_cp_is_sent_while_acks_arrive`, and `an_e_counts_as_an_acknowledgement`.
  - `a_cpus_counts_as_a_cpu`.
  - `the_ack_bit_changes_only_at_a_sequence_boundary`, and a group differs only in that bit.
  - `with_a_1_5_s_round_trip_no_early_repeat_happens`.
  - `a_silence_request_in_training_is_ignored` - a scripted peer SUV with bit 32 set, in `Training` and in
    `FastExchange` context: nothing is reported to the owner, our own bit 32 stays 0, and the trace is
    identical to the run where the bit was clear.
  - `a_peer_that_asks_us_to_wait_for_its_cp_delays_our_cp` - side-agnostic and scripted: with
    `wait_for_cp` set no CP goes before the peer's CP arrives; with it clear the CP goes after the first
    peer SUV; and a peer that asks and then never sends a CP does not stall us past the repeat window.

#### V92-15: The PCM upstream transmitter (M)

- **Depends on:** V92-01, V92-04, V92-06.
- **Clauses:** 6.2; 8.5.6 with 9.5.2.1.7-9.5.2.1.8; 8.6.3; 3.8.
- **Files:** `crates/datapump/src/v92/transmit.rs`.
- **Digests:** CA (5, 7, 11.3, 13); CT (7, 10); INTRO (6.2, the 6.4 output level); P3S (9.2);
  P3P (7.1 A7-A8, 13.2 item 5).
- **Description:** `PcmTransmitter::new(fs, Mode)` in the pull shape of `v34::qam::Transmitter`, with
  `next_sample(clock, next_level)`, turning 8000 symbols/s levels into line samples (AD-8).
  - `Mode::Interpolated`: windowed-sinc reconstruction just under 4 kHz, as `pcm::Receiver` builds its own,
    with an arbitrary fractional delay.
  - `Mode::Straight`: at fs = 16 000, the level on the codec's phase and a band-limited midpoint between;
    exact only for multiples of 0.5 T, which the module doc states.
  - `lookahead()` in symbols, so mid-segment cuts are computed on the symbol stream, never on line time.
  - `delay(fraction_of_t)`: a one-off shift, used twice only - +0.5 T at the first S-bar-u (24.5T) and
    +epsilon·T at the second ((24+epsilon)T) - and never re-stepped afterwards.
  - `symbol_at(n) -> line sample`, so the 40 +/- 1 ms turnarounds and the 100 ms + RTD window can be
    measured at the line terminals.
  - Clock slaving: free-run at a nominal 8000 symbols/s until the downstream receiver has trained, then
    follow `symbol_clock()`; the switch happens in the silence after Ja, where a phase step costs nothing.
  - LU scaling with peaks bounded to 0.3 of full scale, the `dil::LOUDEST` ceiling, because the softphone
    capture path has its own limiter.
- **Tests:**
  - `at_twice_the_rate_every_other_sample_is_the_level` - fs = 16 000, phase 0, both modes.
  - `through_the_network_the_codec_reads_back_the_levels_sent` - unquantised, gain 1, error below -40 dB.
  - `a_half_symbol_delay_moves_the_codec_samples_by_half_a_symbol`.
  - `epsilon_resolves_to_a_sixty_five_thousandth_of_a_symbol` - eight epsilon values measured back through
    `Network::with_upstream_phase`; epsilon = 0x4000 moves the waveform by T/4 within 1 %.
  - `the_transmitter_follows_a_network_120_ppm_fast` - over 10 s the symbol instants at the far A/D stay
    within a stated fraction of T.
  - `the_transmitter_is_never_re_stepped_after_epsilon`.

#### V92-16: Phase 2: the V.92 flags and the Table 18 choice in full Phase 2 (M)

- **Depends on:** V92-02.
- **Clauses:** 9.3; 9.3.1; 8.4.1 (Tables 17, 18); 9.7; V.90 9.2.1.1.8 and 9.2.2.1.9.
- **Files:** `crates/datapump/src/v34/phase2.rs`; `crates/datapump/src/v34/startup.rs`;
  `crates/datapump/src/v90/mod.rs`.
- **Digests:** P2P (2, 4, 5, 7, 11); P2S (10, 11, 16); P3S (5.7 Sd-5); CA (2, 11.2); CD (2, 5.1, 8.4).
- **Description:** full Phase 2 learns V.92; the procedure itself is unchanged, because 9.3 says it is
  V.90's.
  - `V92Wish { capable, short_phase2, pcm_upstream, up_caps }`, carried by the `Pcm` roles. The existing
    constructors mean "not V.92", so a V.90 pair is byte-identical to today.
  - INFO0 is written with the bits at the swapped positions per role; the far flags are recorded **only**
    in PCM roles and survive a retrain, because a retrain never repeats INFO0. Add `both_v92()`.
  - Digital INFO1d: bit 70 is "PCM upstream allowed", set from the digital modem's own verdict, when both
    modems are V.92; otherwise it is the V.90 carrier flag, unchanged.
  - Analogue INFO1a: Table 18 when all of - both V.92, bit 70 set, `pcm_upstream` wanted, PCM upstream not
    declined; otherwise today's Table 10 or Table 11.
  - **UINFO is capped for V.92.** `v90::training_codeword` (`v90/mod.rs:63`) searches Ucodes 67..127
    downwards against the INFO0d power limit and can return a Ucode above 111. Table 18 cannot carry one
    (bits 25:31), and Sd could not use one anyway, because Sd's codeword is Ucode 16 + U_INFO and must stay
    <= 127 (8.6.7 with V.90 8.4.4; P3S 5.7 Sd-5). Against a digital modem with a high maximum transmit
    power a conforming Table 18 INFO1a could therefore not be built at all. Add `V92_MAX_UINFO: u8 = 111`
    in `v90/mod.rs`, whose doc quotes Sd-5, and a `training_codeword_v92(far)` that starts the same search
    at 111. V.90's own `training_codeword` is left exactly as it is, so no V.90 behaviour changes.
  - `decline_pcm_upstream()`, with a pass-through in `v34::startup`, and an `info1a_pcm_up()` accessor.
  - Digital acceptance: a Table 18 INFO1a is accepted only if this modem set bit 70 and both are V.92;
    otherwise the frame counts as not received (P2P P15).
  - `again()` keeps the flags and always runs full Phase 2.
  - `lapm_bypass_allowed()` means both modems are V.92 (9.3.1); the error-control layer consumes it later.
- **Tests** (the `phase2.rs` `Line` harness):
  - `a_v92_pair_settles_on_pcm_upstream` - a Table 18 INFO1a, both ends reporting PCM upstream.
  - `uinfo_never_exceeds_111_so_sd_s_codeword_exists` - against a digital modem whose INFO0d maximum
    transmit power would otherwise admit Ucode 120: the Table 18 INFO1a is still built, its UINFO is
    67..=111, and Ucode 16 + UINFO is <= 127. The V.90 `training_codeword` for the same INFO0d still
    returns its old value, and the existing `v90/mod.rs` tests pass.
  - `a_v92_analogue_modem_meets_a_v90_digital_modem_with_table_10`, and the mirror image.
  - `bit_70_clear_means_no_table_18`, and `pig_off_means_no_table_18`.
  - `declining_pcm_upstream_gives_table_10_next_time`.
  - `a_retrain_between_v92_modems_keeps_the_flags_and_runs_full_phase_2`.
  - `a_v34_info0_with_a_clock_of_1_is_not_taken_for_v92`.
  - `the_lapm_bypass_needs_both_v92_and_both_prot0`.
  - `a_v90_pair_settles_on_v90` passes unchanged, as does every existing Phase 2 test.

#### V92-17: Network, part B: echo, transcoder, gain control and softphone paths (M)

- **Depends on:** V92-04.
- **Clauses:** 1 b (channel separation by echo cancellation); 9.8 (why a silent period exists). The rest is
  a model.
- **Files:** `crates/datapump/src/v90/network.rs`.
- **Digests:** CD (9.2 N2-N11); CT (7, 10).
- **Description:** defaults unchanged. Per-direction settings live in one private `Impairments` struct used
  twice.
  - `with_echo(hybrid_db, taps)`: a short FIR of the reconstructed downstream added into the upstream sum
    **before** the quantiser, and the reverse at the analogue side; `with_far_echo(db, delay)`.
  - `with_transcoder(to_law, low_pass)` per direction, with the measured Crazytel response: -3 dB at
    3750 Hz, -18 dB at 4000 Hz.
  - `with_upstream_gain_control(ceiling, release)`, mirroring the downstream one.
  - `UpPath { Loop, Straight { phase }, Resampled { delay } }` and `with_up_path`. On a `Straight` path a
    clock offset becomes upstream slips at 20 ms / |delta|, not skew.
- **Tests:**
  - `the_hybrid_echo_is_where_it_was_put` - a measured impulse at the stated delay and level.
  - `the_transcoder_is_3_db_down_at_3750_and_18_db_down_at_4000`.
  - `a_straight_softphone_path_hands_the_encoder_our_samples_exactly`, and `a_resampled_path_does_not`.
  - `a_clock_off_on_a_softphone_path_becomes_slips` - one 20 ms slip every 175 s at 114 ppm.
  - `an_upstream_gain_control_squashes_loud_samples_and_the_ones_after`.
  - `v90_call.rs` passes unchanged.

#### V92-18: Probe the V.92 capture (S)

- **Depends on:** V92-02, V92-07.
- **Clauses:** 8.2.1; 8.4.1 (Tables 15-19); 9.1; 9.3.
- **Files:** `crates/datapump/tests/v92_vector.rs` (new).
- **Digests:** CT (2.2, 8.4, 9.5); CV (Q8); P2S (10.4, 15).
- **Description:** read `tests/vectors/v92-56k.wav` the way `v90_vector.rs` reads its file, and pin what
  the 2005 Conexant softmodem and its server actually negotiated. The vectors README calls this file
  "V.34-style startup", so the honest expectation is that it may contain no PCM upstream and no QC1a at
  all; either answer is worth having before the Phase 3 machines are written, because a capture is the only
  thing that can settle the open readings of section 4. **Do not change code to fit the capture**; report
  any contradiction instead.
  - Copy the helpers of `v90_vector.rs`.
  - Read the V.8 menus; run `v8::quick::BitWatcher` on V.21(L) before the CM looking for a QC1a; read the
    INFO sequences with their V.92 bits; classify the INFO1a.
- **Tests:**
  - `the_v92_menus_offer_pcm` - the CM and JM octets.
  - `a_qc1a_before_the_cm_is_looked_for` - asserts presence or absence, as found, and records which.
  - `the_info_sequences_say_whether_v92_and_pcm_upstream_were_used` - INFO0d bit 27, INFO0a bit 26 where
    readable, INFO1d bit 70, and the INFO1a layout from bits 34:39.
  - `if_pcm_upstream_was_used_ru_and_trn1u_are_on_the_tap` - a period-6 hunt after INFO1a; `#[ignore]`d
    with its finding recorded if the capture turns out not to contain it.

### Wave 3: the chain, the decoder, the design, the choice, the receiver

#### V92-19: The upstream data-mode encoder chain (M)

- **Depends on:** V92-01, V92-09, V92-10, V92-11 (`rm_k`, Tables 25 and 26, for `force_k`).
- **Clauses:** 6.3; 6.4.1-6.4.4; 8.7.1 (resets, n = 0, interval 0); 8.7.2 (E2u); 8.7.4 (RM through the
  chain, trellis coded); 8.7.7 (FB1u on the old chain); 9.9.2.1.2 (the scrambler and differential reset);
  Table 30 bits 27:28.
- **Files:** `crates/datapump/src/v92/upstream.rs`.
- **Digests:** INTRO (6.3, 6.4, "Complete per-data-frame procedure"); P4A (2, 3.1, 3.2, 3.5, 3.8);
  RRF (2.5); CD (7.8).
- **Description:** `UpEncoder::new(&Parameters)` (AD-3: it never sees a `Cpd`), with every memory zero, and
  `next_level(&mut impl FnMut() -> bool) -> f64` per symbol:
  - at a frame start, pull K bits and scramble them with GPA (`v32::Scrambler`, polynomial by role);
  - run the 12-interval modulus encoder;
  - choose the point with the precoder;
  - at k = 3, run the inverse map and clock the 16/32/64-state `v34::trellis::Code` with its inputs masked
    to the code;
  - run the prefilter and apply G, emitting LU·G·v(n).

  Plus the sequences that ride the chain: `b1u()` (48 data frames of scrambled ones, every memory zeroed
  first, the first symbol being interval 0 with n = 0); `fb1u()` (48 frames of scrambled, differentially
  encoded ones on the **old** parameters, no reset); `e2u(extend)` (one frame of scrambled, differentially
  encoded zeros, 13 symbols when CPd bit 29 is set); `force_k(pattern)` for RM and RM' (no bits consumed,
  the modulus sign state not clocked, precoder/prefilter/trellis carried over from data mode);
  `reset_scrambler_and_differential()` for FPE; and an information-sequence mode that feeds a framed
  sequence's bits through the scrambler and the modulus encoder at K bits per frame, for FPE.
  Introspection: `frame_symbol()`, `sent_etas()`, and an RMS tracker for the SUVu level field.
- **Tests:**
  - `every_trellis_frame_s_index_sum_has_the_parity_of_y0`.
  - `a_separate_encoder_fed_table_13_regenerates_the_same_y0`.
  - `b1u_is_the_same_forty_eight_frames_every_time` - a pinned vector for one `Parameters`.
  - `rm_puts_the_table_25_pattern_into_the_classes`, and `rm_prime_is_rm_shifted_by_two`.
  - `e2u_grows_by_one_symbol_when_the_digital_modem_asks_and_b1u_s_interval_0_moves_with_it`.
  - `the_32_and_64_state_codes_are_selected_by_cpd_bits_27_and_28`.
  - `every_sequence_boundary_lands_on_a_twelve_symbol_frame` - a debug assertion across TRN2u, SUVu, CPu
    and E2u.
  - `the_output_power_with_a_flat_prefilter_is_one_when_g_is_one_over_rms` - measured RMS of G·v over
    100 000 symbols.

#### V92-20: The upstream decoder and the RM watch (M)

- **Depends on:** V92-01, V92-09, V92-10, V92-11 (`rm_k`, Tables 25 and 26, for `RmWatch`).
- **Clauses:** 6.4.1-6.4.4 read backwards; 8.7.4; 9.9.1.1.2; 9.9.1.2.1.
- **Files:** `crates/datapump/src/v92/decoder.rs`.
- **Digests:** INTRO (6.4.3, 6.4.4, Q-15); CD (7.7); RRF (2.5, 2.18).
- **Description:** the digital modem's data-mode decoder, given equalised samples and `Parameters`.
  - `Viterbi4d`: a generic decoder over the V.34 16/32/64-state codes, clocked once per 4 symbols, taking a
    closure for the per-dimension metrics, because `v34::data::Decoder` is welded to V.34's grid. There is
    no V.34 modulo encoder and no superframe inversion: 6.4.4 cites only 9.6.3.2.
  - The 2D subset label on `(2·eta_a + 1, 2·eta_b + 1)`, which depends only on `(eta_a mod 4, eta_b mod 4)`.
  - Ki from the surviving eta: `eta mod Mi` for k < 3, `((eta - p) / 2) mod Mi` for k = 3.
  - The 12-interval modulus decode with d(f-1), then the GPA descramble; resets for B1u and for FPE.
  - `RmWatch`: works in the K domain, ignores intervals with M = 1, reports RM after 8 frames and the
    RM-to-RM' turn (four zeros) to the symbol.
  - `could_have_sent(frame)`, for the re-framing search of V92-46.
- **Tests:**
  - `every_bit_comes_back_over_a_perfect_channel` - the V92-19 encoder into this decoder, 100 000 bits at
    drn 19, zero errors, at 16, 32 and 64 states.
  - `the_trellis_earns_its_keep` - at a noise level where a symbol-by-symbol slicer errs, the Viterbi's
    error rate is at least a stated factor lower.
  - `a_forty_frame_survivor_depth_is_enough` - the error rate does not improve past the chosen depth.
  - `an_inverted_line_still_decodes`.
  - `rm_is_recognised_from_the_decoded_k_and_not_from_a_tone` - RM through the full chain, spread by a real
    precoder, is still detected within eight frames, and its turn to RM' is found to the symbol.
  - `random_data_never_looks_like_rm_in_100000_frames`.
  - `an_interval_with_a_modulus_of_one_is_ignored_by_the_rm_watch`.

#### V92-21: The upstream channel estimate and the CPd design (M)

- **Depends on:** V92-01, V92-10.
- **Clauses:** 6.4.2; 8.8.3 (the design rules and its NOTE); 8.7.6 (TRN2u may be used); Table 18 bits 12:17.
- **Files:** `crates/datapump/src/v92/design.rs`.
- **Digests:** CD (7.6, 7.8); P4D (5.1, 5.4, 13.3); INTRO (6.4.2).
- **Description:** the half of the riskiest DSP that is entirely ours to invent. It produces `Parameters`,
  not a `Cpd`.
  - `Channel::estimate(known_levels, received)`: T-spaced least squares from a known TRN1u or TRN2u run,
    with a noise-floor estimate. `dsp::least_squares` takes `Complex`, so feed zero imaginary parts or add
    a real variant.
  - `design(channel, noise, limits) -> Filters`, an MMSE-DFE: the feed-forward section becomes z2 (and p2
    where the INFO1a sections allow), the feedback becomes p1 (and z1 where allowed), each within Lmax and
    all within Ltot; coefficients quantised to Q0.15 and Q1.14 with a range check; it reports the predicted
    residual ISI, which V92-62 adds to the measured noise before it chooses moduli, sets and a rate.
  - Verification through the V92-10 model before anything is returned (AD-6).
- **Tests:**
  - `an_ideal_channel_gets_a_trivial_design`.
  - `the_designed_filters_leave_little_isi_on_a_loop_model` - a test-local loop with a DC high-pass near
    200 Hz and a 3.4-4 kHz roll-off; residual ISI below -25 dB and below a stated fraction of the level
    spacing.
  - `the_coefficients_fit_inside_ltot_and_lmax_for_every_info1a_code` - including the minimum
    p1-and-z2-only modem.
  - `z1_and_p2_are_left_out_when_info1a_says_so`.
  - `the_channel_estimate_from_trn2u_matches_the_true_channel`.
  - `a_deliberately_broken_coefficient_set_is_caught_by_the_self_check`.

#### V92-22: The digital modem's upstream PCM receiver (M)

- **Depends on:** V92-01, V92-03, V92-04, V92-11, V92-12, V92-15. V92-03 and V92-11 are what the last
  bullet below means by "the sequence finders": `DescriptorFinder::v92`, `CpFinder::v92` and the SUV/CP
  stream finder. A worktree cut from the old list (V92-04, V92-12, V92-15) would not compile.
- **Clauses:** 8.5.5-8.5.7; 8.7.6; 9.5.1.1.1-9.5.1.1.3; 9.5.1.1.6-9.5.1.1.7; 9.5.1.1.10; **Table 1/V.90**,
  the Ucode table, reached through V.92 3.6 - *not* Table 1/V.92, which is the interchange-circuit list and
  has nothing a receiver needs.
- **Files:** `crates/datapump/src/v92/receiver.rs`.
- **Digests:** INTRO (3.6, Ucodes and Table 1/V.90); P1D (3); CD (7.1, 7.2, 7.4); P3P (6.1, 11);
  P3S (4.5-4.7); CT (4.4); CA (4.3 as a template).
- **Description:** a T-spaced receiver: one sample per symbol arrives from the central-office A/D, so no
  fractionally spaced equaliser is possible and there is nothing to interpolate at this end, because the
  analogue modem is required to put its symbols on the A/D instants.
  - `feed(level)` and `hunt()` for period-6 patterns and their inversions, in either polarity, in the shape
    of `pcm::Hunt` but at T spacing: Ru `+++---` and Su `+0+-0-`, whose zeros must not defeat the hunt.
  - `idle_for(symbols)`, for waiting out a nonzero MD length.
  - `train_on_trn1u(from)`: least squares against the known GPA sequence (known from its first symbol,
    because the register is zeroed and fed ones), with a coarse alignment search and retries on a later
    stretch in the shape of `pcm.rs`'s `TRAIN_TRIES`, giving a feed-forward plus decision-feedback
    equaliser. Rows are `Complex` with im = 0 through `dsp::least_squares`. NLMS tracking after that.
  - Slicers: two-point, TRN2u at 4 and 8 points, and per-j level sets.
  - Framing: `set_frame_origin(at)` and `interval() = symbol % 12`, started at the first symbol of the
    second TRN1u and carried modulo 12 (section 4).
  - `Heard { Ru, RuReversal { at }, Trained { snr_db }, Su, SuReversal { at }, Symbol(..), Lost, Found }`,
    the event-queue shape the rest of the code base uses, with `Lost`/`Found` on an error jump.
  - A sample history for the epsilon estimator and the channel estimate.
  - The TRN1u- and TRN2u-modulation bit readers of V92-12, plumbed to the sequence finders.

  Slip re-framing and echo cancellation are deliberately **not** here; they are V92-43 and V92-46, once the
  network can produce slips and echo.
- **Tests:**
  - `ru_and_its_reversal_are_found_whatever_the_polarity` - through `Network::up` at four A/D phases.
  - `su_is_found_through_its_zeros_and_its_reversal_timed_to_the_symbol`.
  - `trn1u_trains_the_receiver_over_a_loop_with_noise` - SNR at least 25 dB at noise 1e-4.
  - `training_retries_on_a_later_stretch_when_the_first_fails` - a burst at the start of TRN1u costs a
    retry, not a failure.
  - `two_point_decisions_after_training_are_error_free_at_30_db`.
  - `trn2u_levels_are_decided_at_both_sizes`.
  - `silence_and_noise_do_not_train`.
  - `the_twelve_symbol_count_starts_where_it_is_told`.

#### V92-23: The sampling-phase estimator (M)

- **Depends on:** V92-04, V92-12, V92-15.
- **Clauses:** 8.6.3 (Jp bits 18:33); 9.5.1.1.6-9.5.1.1.8; 9.5.2.1.7-9.5.2.1.8.
- **Files:** `crates/datapump/src/v92/epsilon.rs`.
- **Digests:** P3S (4.6, 5.3, 9.2, 9.3); P3P (3.5, 11 item 3, 11.3, 13.3 items 1-2); CD (7.3).
- **Description:** the piece with no specified algorithm at all: the digital modem must work out where the
  A/D instants fall inside the analogue modem's symbol and report it as epsilon.
  - `PhaseEstimator` collects the A/D samples over the Su before the first S-bar-u, the 24.5T S-bar-u, and
    the Su after it. Because the first S-bar-u is 24.5T, the second Su is half a symbol from the first,
    which gives two views half a symbol apart.
  - It fits the known `{+a, 0, +a, -a, 0, -a}` pattern through a short estimated channel at trial phases.
  - It returns `epsilon: u16` - the extra delay, **after** the 0.5 T shift, that puts the transmitter's
    symbols on the A/D instants - quantised to Jp's 16-bit fraction, with a confidence.
  - Statistics use medians over windows, never means, because of the VoIP jitter-concealment inserts.
- **Tests:**
  - `every_upstream_sampling_phase_is_found_to_a_sixty_fourth_of_a_symbol` - eight phases through the
    V92-15 transmitter and `Network::with_upstream_phase`.
  - `the_estimate_holds_with_noise`.
  - `applying_epsilon_puts_the_symbols_on_the_samples` - residual below -30 dB.
  - `a_slip_during_su_is_reported_not_believed`.

#### V92-24: Short Phase 2, the error-free procedure (M)

- **Depends on:** V92-16.
- **Clauses:** 9.4; 9.4.1.1.1-9.4.1.1.5; 9.4.2.1.1-9.4.2.1.4; Figure 9; Table 19; 9.5.2.1.1.
- **Files:** `crates/datapump/src/v34/phase2.rs`.
- **Digests:** P2P (2, 6, 8, 11); P2S (11.2, 16).
- **Description:** the ranging-only Phase 2, as a `Short` flavour of the existing machine rather than a new
  module, so the INFO0 exchange and its bit-28 recovery are the same code. The roles reverse.
  - Latch the four-bit decision as soon as the far INFO0 decodes.
  - **Digital stages:** wait for Tone A having sent B for at least 50 ms; reverse B, then 10 ms of B-bar;
    silence; on the A reversal compute RTDEd; INFO1a; done, with the layout. There is no probing, no
    INFO1d and no second reversal, so only RTDEd exists.
  - **Analogue stages:** wait for the B reversal; reverse A so that 40 +/- 1 ms passes **at the line
    terminals**, scheduled below a symbol with detector-latency and filter-delay compensation (P2P P5);
    10 ms of A-bar; INFO1a at once, its leading point continuing A-bar's phase; done.
  - `ShortPlan`, supplied by the analogue owner: the Table 18 fields, or Table 19's rate and carrier; no
    plan means no request (the 9.4 restriction, N-21); the plan carries the remembered RTD; Table 19's
    frequency offset is sent as -512.
  - **Which INFO1a layouts are legal is latched with the short/full decision** (9.4.1.1.5 with 8.4.1;
    P2P 11.2 P15, with the layout classification of P2P 3.8). After a **short** Phase 2 only a Table 18 or
    a Table 19 frame may arrive; after a **full** Phase 2 only a Table 10, a Table 11 or a Table 18 frame.
    A Table 19 frame after full Phase 2, or a Table 11 (V.34) frame after short Phase 2, counts as **not
    received**, with a reason string naming the layout, and the pair goes into the recovery of V92-29.
    V92-16 already implements the Table 18 half of P15 ("accepted only if this modem set bit 70 and both
    are V.92"); this is the other half, and it belongs here because this is where the decision is latched.
- **Tests:**
  - `both_asking_for_short_phase_2_get_it_and_rtded_is_the_line_s_round_trip` - 0.03, 0.6 and 1.5 s,
    within 1 ms.
  - `one_side_not_asking_means_full_phase_2`, and `a_v90_far_end_s_zeros_mean_full_phase_2`.
  - `the_answering_a_reversal_leaves_40_ms_after_the_b_reversal_arrives` - measured on the line, +/-1 ms.
  - `info1a_follows_ten_ms_of_a_bar_with_no_gap` - and the digital modem time-stamps the A-bar edge, not
    one of INFO1a's fill reversals.
  - `short_phase_2_hands_over_the_planned_table_18_or_table_19_info1a`.
  - `a_table_19_info1a_after_full_phase_2_is_refused` - the frame counts as not received, the reason names
    the layout, and recovery runs.
  - `a_table_11_info1a_after_short_phase_2_is_refused` - the mirror case, with a Table 18 and a Table 19
    frame accepted on the same path in the same test, so the check is shown not to be a blanket refusal.
  - `an_analogue_modem_with_no_plan_does_not_ask`.
  - Every existing full-Phase-2 test passes unchanged.

#### V92-25: Modem-on-hold transactions (M)

- **Depends on:** V92-01 (the `hold.rs` stub and its `pub mod hold;` line, without which this file does not
  compile), V92-02.
- **Clauses:** 8.9.1; 8.9.2 (Tables 32, 33); 9.10; 9.10.1; 9.10.1.1; 9.10.1.2 (Table 34);
  9.10.2.1-9.10.2.3; Figures 20-24; V.8 7.2; V.90 8.2.3.1.
- **Files:** `crates/datapump/src/v92/hold.rs`; `crates/datapump/src/v8.rs` (only to make the ANSam
  generator `pub(crate)`).
- **Digests:** MOH (1, 2, 5, 8, 9.3); CA (11.8); CV (6.3.2).
- **Description:** `Transaction` for a role, running its own `dpsk` transmitter and an MH-enabled receiver.
  Responding is mandatory for every V.92 modem even if it never initiates.
  - **Tones.** The analogue modem's RT is Tone A and it listens for Tone B; the digital modem's is Tone B.
  - **Initiator** (MHreq, MHclrd{reason} or MHfrr). An initiating sequence may go only once circuit 107 is
    asserted **and** either the remote's Tone RT has been detected **or** an MH response sequence has been
    detected (9.10.1.1 R1; MOH 2.3, 2.5 - Figures 20 to 24 all show the initiator waiting for the far end's
    RT before its first MH sequence). Our own optional RT is at least 50 ms, or at least 20 ms after an MH
    sequence. The wait for that gate has its own bound, `MH_RT_WAIT = 2 s + RTD` (section 4), because a far
    end that never sends RT would otherwise hang the transaction for ever; expiry gives `GaveUp` with its
    own reason. The 2 s + RTD response timer starts at the first bit of our first initiating sequence
    (section 4, MOH Q4). Sequences then go back to back until the response; on expiry, finish the sequence
    in flight and give `GaveUp`.
  - **Glare.** No clause covers both ends starting a transaction at once, or MHclrd crossing MHreq
    (MOH Q9). The policy is the named constant `MH_GLARE_YIELD_DIGITAL` of section 4: answer the received
    initiating sequence per Table 34 while our own continues, and if both are still running at 2 s + RTD
    the digital modem abandons its own transaction and the analogue modem's stands.
  - **Responder**, per Table 34, with `Policy { grant: Option<t1>, after_refusal }`: MHreq gets MHack or
    MHnack; MHnack gets MHcda or MHfrr; MHclrd gets MHcda; MHfrr gets ANSam. Responses stop on ANSam, on
    silence, or when the initiating sequence has been absent for 200 ms by the clock. A refused requester
    must choose within 10 s.
  - **Held side:** on the remote's RT for 100 ms, or 2 s of silence, finish MHack, then ANSam within 80 ms,
    giving `Holding { t1 }`. Every sequence that is started is completed before anything else goes out,
    which the 80 ms window must allow for.
  - **Fast reconnect:** MHfrr leads to at most 80 ms of silence, then ANSam (`FastReconnectAnswer`); the
    MHfrr sender, after 1 s of ANSam, gives `FastReconnectCall`.
  - **Cleardown:** MHclrd/MHcda give `ClearedDown { reason }`.
  - **A reserved T1.** An MHack whose T1 field is `0000`, `1110` or `1111` (Table 33) is **not a grant**
    (section 4, MOH Q11): the transaction reports `Refused`, so the requester takes the refusal path -
    MHcda or MHfrr - exactly as it would for an MHnack, rather than holding for an unknown time.
  - **The retrain race:** a responder watching for both reports `RetrainInstead` when a reversal arrives
    with no MH. An MH sequence opens with four ONEs, which in DPSK is four phase reversals in a row, so a
    validated MH frame must win over the reversal detector.
- **Tests** (two transactions over a delay line):
  - `figure_20_a_request_granted_leaves_the_held_modem_sending_ansam`.
  - `figure_21_a_refusal_then_cleardown_ends_both`.
  - `figure_22_a_refusal_then_fast_reconnect_ends_in_ansam_heard_for_a_second`.
  - `figure_23_a_cleardown_request_is_acknowledged`.
  - `figure_24_a_fast_reconnect_request_is_answered_with_ansam`.
  - `the_held_modem_sends_ansam_within_eighty_milliseconds_of_the_last_mhack`.
  - `an_unanswered_request_gives_up_after_two_seconds_and_a_round_trip_at_a_sequence_boundary`.
  - `a_response_stops_200_ms_after_the_request_does`.
  - `mhack_carries_the_t1_code_granted`, and MHnack when the policy denies.
  - `the_digital_initiator_s_mhreq_wins_over_the_analogue_responder_s_reversal`.
  - `a_slip_that_breaks_one_frame_does_not_count_as_200_ms_absent`.
  - `an_initiator_waits_for_the_far_rt_before_its_first_mh_sequence` - with the responder's RT delayed, not
    one MH bit goes out before it arrives; with an MH response arriving instead of an RT, the initiator
    proceeds; with neither, `MH_RT_WAIT` gives `GaveUp` rather than a silent hang.
  - `a_grant_with_a_reserved_t1_ends_as_a_refusal` - MHack with T1 `0000`, `1110` and `1111`: `Refused`,
    never `Granted`, and the MHcda/MHfrr choice is offered.
  - `two_transactions_begun_together_converge` - both ends send MHreq at the same instant, and separately
    MHclrd crossing MHreq: each answers the other per Table 34, and the pair settles on one outcome within
    2 s + RTD with the digital modem yielding. This mirrors the `two_renegotiations_begun_together_converge`
    test of V92-48.

#### V92-62: Upstream constellations, moduli, rate and gain - the numeric choice (M)

- **Depends on:** V92-01, V92-09, V92-10.
- **Clauses:** 6.1 (the upstream ladder); 6.4.1 (2^K <= M); 6.4.2 (N >= M, and N >= 2M at k = 3);
  8.8.3 (the design rules and its NOTE); Table 18 (the limits the analogue modem announced).
- **Files:** `crates/datapump/src/v92/upchoice.rs`.
- **Digests:** CD (7.6, 7.8); P4D (5, 5.1, 5.4, 13.3); INTRO (6.1, 6.4); CA (4.3, `dil::choose` as the
  template).
- **Description:** the numeric half of the digital modem's design problem, **split out of V92-30 so that it
  lands a wave earlier and the M0 gate (V92-26) can depend on it** - four of V92-26's eight tests assert on
  the chosen drn, the moduli and the class rules, and V92-30 sits in V92-26's own wave (section 14 records
  why the split was preferred to the two fixes the review suggested). It produces `Parameters`, never a
  `Cpd`, so it needs neither the wire codec nor `UpEncoder`:
  `choose(channel, noise, filters, limits, law, robbed_j) -> Option<Parameters>`, where `filters` are
  V92-21's and `noise` already includes that design's predicted residual ISI.
  - Per-j sets come from the codec's own linear levels at the spacing the measured noise allows, the way
    `dil::choose` already works downstream; point values use the section 4 reading; no zero point,
    non-empty sets first, 2·LC <= 128.
  - Moduli have margin and meet the class constraints: N >= Mi, and N >= 2·Mi at k = 3.
  - drn is the highest rung of the ladder that meets 2^K <= product(Mi) with room. The **Ja mask is not
    applied here** - that is V92-30's job, so that this package stays free of the wire and of Table 20.
  - G comes from a simulation of the V92-10 chain driven by random Ki (AD-6), so that E[(G·v)^2] = 1,
    quantised to unsigned Q0.16 as 4G and never zero. It deliberately does **not** use `UpEncoder`, which
    is V92-19 in this same wave; the modulus encoder and the precoder/prefilter chain are all that set the
    output power, and both are wave 2.
  - Trellis is 16-state by default and configurable.
- **Tests:**
  - `more_noise_means_a_lower_upstream_rate` - five noise levels, monotonic.
  - `a_robbed_bit_interval_gets_a_sparser_set`.
  - `the_moduli_and_the_sets_satisfy_every_class_rule` - N >= Mi, N >= 2·Mi at k = 3 and
    2^K <= product(Mi), over a sweep of channels and noise levels.
  - `g_is_quantised_to_q0_16_and_never_zero`, and the simulated E[(G·v)^2] is 1 within a stated tolerance.
  - `a_channel_that_cannot_carry_the_bottom_rung_is_refused` - `None`, never a set whose decisions would be
    wrong.
  - `the_choice_obeys_every_limit_the_analogue_modem_announced` - Ltot, Lmax, the allowed filter sections
    and 2·LC <= 128, for every Table 18 code.

### Wave 4: the de-risk spike, the sources, the CPd assembly, short Phase 1 on the line

#### V92-26: The PCM upstream link on a synthetic channel - the de-risk spike (M) - milestone M0

- **Depends on:** V92-17 (the echo model), V92-19, V92-20, V92-21, V92-22, V92-23, **V92-62** (the rate,
  moduli, constellation and G choice: four of the tests below assert on the chosen drn and on the class
  rules, and that choice is V92-62's, not V92-21's).
- **Clauses:** 6.4; 8.8.3.
- **Files:** `crates/datapump/tests/v92_upstream.rs` (new).
- **Digests:** CD (7.6, 7.7, 11); INTRO (6.4); CT (7); P4D (5.1, 5.4).
- **Description:** the most valuable early test in the plan, and the plan's one explicit go/no-go. It joins
  the transmit chain, the CPd design and the decoder across a channel with the hard parts of a codec path,
  with no protocol and no state machine in the way, so that the answer to "can PCM upstream work at all"
  arrives before anything is built on top of it.

  The harness lives in the test file: a first-order high-pass near 200 Hz for the codec's DC null, a
  raised-cosine roll-off from 3.4 to 4 kHz, a flat-to-tilted loop response, additive noise, a settable A/D
  sampling phase, **hybrid echo added before the quantiser** and a G.711 quantiser. The echo is part of the
  gate on purpose: section 11 argues that the digital modem's canceller is what makes upstream PCM
  decisions possible at all, because echo is added before the quantiser and cannot be undone afterwards, so
  a go/no-go decided on an echo-free channel would be decided on a channel the real digital modem never
  sees. V92-43's canceller is six waves away, so the spike carries a **test-local least-squares echo fit**
  against our own known downstream: enough to answer whether the rate survives, and its result is the
  target V92-43 must later meet with a real adaptive canceller. Per case: estimate the channel from a known TRN1u or TRN2u run,
  design `Parameters`, run random data through `UpEncoder`, quantise at the A/D, decode, count bit errors.
  The second half runs the same flow through the **real** V92-22 receiver with the V92-23 epsilon applied
  by the V92-15 transmitter, with the A/D phase swept, which is the other half of the de-risk.

  **If the first test cannot be made to pass, stop and report.** The plan then runs with V.34 upstream as
  the product (V92-42 already builds that ladder) and PCM upstream becomes a research branch; waves 5 to 9
  shrink to the Phase 2 and hand-over work.
- **Tests:**
  - `the_designed_precoder_carries_the_top_of_the_ladder_over_a_clean_codec_channel` - at about 40 dB of
    signal to quantising noise, the design picks drn in the top third and the bit error rate is zero over
    200 000 bits. **This is the go/no-go.**
  - `the_go_no_go_holds_with_hybrid_echo` - the same case through `Network::with_echo` at 15 dB and 25 dB
    with the test-local canceller: the residual stays under the stated fraction of the upstream level
    spacing and drn stays within one rung of the echo-free result. If this fails while the echo-free case
    passes, that is a requirement handed to V92-43, not a stop.
  - `the_rate_falls_a_rung_at_a_time_as_noise_rises` - five noise levels; drn falls monotonically and the
    error rate stays at zero.
  - `a_channel_the_filters_cannot_equalise_is_refused_not_mis_designed` - with a deep in-band notch the
    design drops to the bottom rung or reports that it cannot serve the analogue modem, and never returns
    coefficients whose decisions are wrong.
  - `the_design_obeys_every_limit_the_analogue_modem_announced` - Ltot, Lmax, the allowed sections, the
    class rules and 2^K <= product(Mi), over the whole sweep.
  - `a_gain_change_at_the_codec_is_absorbed_by_g` - up_gain 0.25, 0.5 and 1.0 all connect at the same rate.
  - `su_gives_the_sampling_phase_to_within_a_sixty_fourth_of_a_symbol` - eight A/D phases through the real
    receiver.
  - `the_link_carries_data_at_every_sampling_phase` - zero bit errors at all eight phases with the measured
    epsilon applied.
  - `an_uncorrected_phase_costs_what_we_expect` - the same run with epsilon forced to zero, printing the
    rate loss, so the value of the mechanism is on record.

#### V92-27: The analogue upstream source, Phase 3 (M)

- **Depends on:** V92-03, V92-11, V92-12, V92-15.
- **Clauses:** 9.5.2.1.1-9.5.2.1.11; 8.5.1-8.5.7; Table 20; Table 1 (circuit 107).
- **Files:** `crates/datapump/src/v92/up_source.rs`.
- **Digests:** P3S (4, 6.2); P3P (2, 3, 7); CA (4.2, 11.3).
- **Description:** `UpSource`, behind a pull-driven `UpSource::next() -> f64` and a `change(up)`.
  **The whole `Up` enum is declared here, the Phase 4, rate-renegotiation and FPE variants included** -
  `Trn2u`, `Suv`, `Cp`, `Cpus`, `E2u`, `B1u`, `Data`, `Fb1u`, `Rm`, `RmPrime`, `Ru`/`RuBar` for
  renegotiation - with `unimplemented!()` bodies and their `change()` boundary rules written down but not
  yet enforced. That is what lets V92-34 merely fill them in, so that `v92/analogue.rs` (V92-32), which
  `match`es on `Up`, keeps compiling when V92-34 lands in the same wave and neither package has to edit the
  other's file (section 6, wave 5). The variants this package implements are the Phase 3 subset - Silence,
  Ru, RuBar, Md (zeros), Trn1u, Ja, Su, SuBar{extension}, Cpt, E1u - with boundary rules per segment: Ja stops at the next **12-bit** boundary; Su and S-bar-u have fixed lengths;
  CPt changes only at a sequence end, and E1u follows the CPt in flight. It drives
  `PcmTransmitter::delay` for the 0.5 T and epsilon·T extensions. The frame counter resets at the first
  symbol of the second TRN1u; from then on a debug assertion checks that every sequence is whole frames.
  `circuit_107()` and segment counters.
- **Tests:**
  - `phase_3_upstream_is_ru_ru_bar_trn1u_and_ja_in_that_order` - 384T, 24T, then at least 2040T in
    multiples of 12.
  - `ja_is_24_ones_then_whole_v92_descriptors_and_stops_on_a_12_bit_boundary` - decoded with the V92-12
    reader and the V92-03 V.92 finder, including the 19-bit mask, across a sweep of cut instants, never
    more than 11 bits late.
  - `the_first_s_bar_u_shifts_everything_after_it_by_half_a_symbol`, and
    `the_second_s_bar_u_adds_epsilon`.
  - `the_second_trn1u_starts_interval_0_and_every_later_sequence_is_whole_frames`.
  - `cpt_repeats_identically_after_its_24_ones_and_e1u_follows_the_one_in_flight`.
  - `circuit_107_turns_on_with_the_second_s_bar_u`.

#### V92-28: The digital downstream source, Phases 1 and 3 (M)

- **Depends on:** V92-03, V92-11, V92-13.
- **Clauses:** 8.3.1; 8.3.6; 8.6.1-8.6.8 with V.90 8.4.1, 8.4.4, 8.4.5 and 8.6.4;
  9.5.1.1.3-9.5.1.1.13; clause 5 (frame alignment); 9.8.1.1.3.
- **Files:** `crates/datapump/src/v92/down_source.rs`.
- **Digests:** P3S (5, 6.1); P3P (4, 6); P1D (3, 8); CD (3.2, 8.2).
- **Description:** `DownSource`, emitting octets per symbol by law. **The whole `Out` enum is declared
  here, the Phase 4, rate-renegotiation and FPE variants included** - `Trn2d`, `Suvd`, `Cpd`, `Ed`, `B1d`,
  `Data`, `Rd`/`RdBar`, `Rt`/`RtBar`, `Rf`/`RfBar` - with `unimplemented!()` bodies and their boundary
  rules written down, so that V92-35 merely fills them in and `v92/digital.rs` (V92-33) keeps compiling
  through the wave-5 merge (section 6, wave 5). The variants this package implements are the Phase 1 and
  Phase 3 subset - Quiet, Qts, QtsBar, Anspcm, Sd, SdBar, Trn1d, Jd, Jp, JpPrime, Dil, Scr, Ri, RiBar.
  - The frame counter's origin is the first QTS or Sd symbol, or a grid passed in `frame_origin`; Sd waits
    for interval 0.
  - **One differential chain:** Jd starts from TRN1d's last symbol, Jp from Jd's, Jp-prime from Jp's; sign
    0 is negative.
  - SCR continues the scrambler, in whole frames of scrambled ones.
  - The DIL comes from `Descriptor::symbols` and ends on a segment boundary.
  - Ri runs until pending, then R-bar-i for exactly 24 symbols.
  - Boundaries: Jp waits for the Jd in progress; Jp-prime for the Jp in progress.
  - Quiet is constant positive Ucode 0 with the frame count kept.
- **Tests:**
  - `qts_qts_bar_and_anspcm_follow_with_no_gap_and_qts_starts_interval_0`.
  - `sd_lands_on_interval_0_of_a_grid_begun_at_qts`.
  - `trn1d_signs_are_the_gpc_vector`.
  - `jd_then_jp_then_jp_prime_decode_with_one_differential_chain`.
  - `jp_waits_for_the_jd_in_progress_to_finish`.
  - `the_dil_stops_on_a_segment_boundary_and_ri_follows`.
  - `scr_is_whole_frames_of_scrambled_ones`, and `ri_bar_is_exactly_24_symbols`.
  - `quiet_keeps_the_frame_count`.

#### V92-29: Short Phase 2 recovery (M)

- **Depends on:** V92-24.
- **Clauses:** 9.4.1.2.1-9.4.1.2.3; 9.4.2.2.1-9.4.2.2.2; 9.7.1.x; 9.7.2.1; V.90 9.2.1.1.3 (the entry point).
- **Files:** `crates/datapump/src/v34/phase2.rs`.
- **Digests:** P2P (6.3, 6.5, 6.6, 7, 11.2, 11.3, 12); P4P (7).
- **Description:** the INFO0 repeat and bit-28 recovery on the short path; SDR-2 and SDR-3 take the digital
  modem into full Phase 2 at FD-3 without INFO0; SAR-2 is a 9.7.2.1 retrain. The A-reversal detector is
  armed only at SD-3 (P2P A7); a tone onset is not a reversal (P7); the 2500 ms timer runs from the last
  INFO0a (P2P A9). The three recovery timers are flat 2500 ms with no RTD term, which caps the round trip
  at about 2.3 s; above that the modems fall into full Phase 2 automatically.
- **Tests:**
  - `a_lost_b_reversal_makes_the_analogue_modem_retrain_into_full_phase_2`.
  - `a_lost_a_reversal_drops_the_digital_modem_into_full_phase_2`.
  - `a_corrupted_info1a_leads_both_into_full_phase_2`.
  - `an_info0_lost_each_way_is_recovered_and_short_phase_2_still_agreed`.
  - `round_trips_up_to_2_3_s_use_short_phase_2_and_longer_ones_fall_back_cleanly` - three points by
    default; the full 0 to 2.6 s sweep is `#[ignore]`d.
  - `tone_a_after_retrain_silence_is_not_taken_for_the_reversal`.

#### V92-30: The CPd assembled from the chosen parameters, and verified end to end (S)

- **Depends on:** V92-11, V92-19, V92-20, V92-21, V92-62.
- **Clauses:** 8.8.3 (Table 30); Table 20 (the Ja mask); Table 18 (the limits); 9.11.
- **Files:** `crates/datapump/src/v92/upchoice.rs`.
- **Digests:** CD (7.6); P4D (5, 13.3); INTRO (6.1, 6.4).
- **Description:** the wire half of the choice. V92-62 (wave 3) already turns a channel estimate into
  `Parameters`; this package turns `Parameters` into a `Cpd` the analogue modem will accept, and proves it
  by running it.
  - `assemble(params, ja_mask, limits, law) -> Option<Cpd>`: apply the Ja mask (drn must be a rate the mask
    enables; otherwise step down to the highest rung it does enable, or refuse), the Table 18 limits and
    `Cpd::check`'s DC-1..DC-7, then build the `Cpd` through `From<&Parameters>` (AD-3). Every part is
    present in the first CPd of a training.
  - Before it is returned, the candidate must decode N frames error-free through `UpEncoder` -> channel ->
    `UpDecoder` (AD-6).
  - `cleardown()` gives a CPd with drn 0 and no parts.
- **Tests:**
  - `a_chosen_cpd_meets_every_rule_of_8_8_3`.
  - `the_rate_is_one_the_ja_mask_enables` - a mask that forbids the chosen drn steps it down, or refuses.
  - `a_chosen_cpd_decodes_a_thousand_frames_through_its_channel`.
  - `a_cpd_that_breaks_a_table_18_limit_is_refused_not_trimmed`.
  - `a_cleardown_cpd_has_drn_0_and_no_parts`.

#### V92-31: Short Phase 1, the line signalling (M)

- **Depends on:** V92-07.
- **Clauses:** 9.2; 9.2.1.1; 9.2.2.1; 9.2.3.1; 9.2.4.1; 9.2.5; Figures 3, 4; V.8 8.1, 8.2; V.21.
- **Files:** `crates/datapump/src/v8.rs`; `crates/datapump/src/bell103.rs`.
- **Digests:** P1P (5, 6, 10); P1A (9, 11); CV (2.2, 6.2.2, 8 R2-R8); CTL (3.4).
- **Description:** the ANSam route of short Phase 1 on the V.21 channels, in the roles we implement
  (section 11 lists which roles are out of scope).
  - Builders `offering_quick(qc)` for the caller, analogue or digital, and `answering_quick(role, field)`,
    with a raw-bit queue for the QC frames, since QC1a carries ten ONEs between two framed octets, and
    `Bell103Tx::abandon()` to drop the current octet mid-way, which V.8 never needed.
  - **Caller:** after 1 s of ANSam, queue the QC bits and then CM, and run `v8::quick::BitWatcher` on
    V.21(H) beside JM. On a QCA of the other role, abandon CM mid-octet, go silent, and report
    `Done(Agreed(V34Duplex))` with `quick()` giving the role, both P bits ANDed, the U_QTS and the LM. On
    JM, carry on with V.8. A digital caller that still hears ANSam 1 s after QC1d goes to V.8 and ignores a
    late QCA1a.
  - **Answerer:** while sending ANSam, watch V.21(L) for QC1a and QC1d and hold CM parsing until decided;
    then send QCA, go silent, and report `quick()`.
  - **Returning to V.8:** `resuming_answer()` starts straight in ANSam and `resuming_call()` starts in
    Waiting, for the hold resume of V92-58.
  - **No new `Status` variants** (AD-5).
- **Tests** (two V.8 modems over a delay line):
  - `an_analogue_caller_and_a_digital_answerer_both_report_quick_connect`.
  - `a_digital_caller_and_an_analogue_answerer_both_report_quick_connect`.
  - `the_lapm_outcome_is_set_only_when_both_p_bits_are`.
  - `cm_is_cut_mid_octet_when_qca1d_arrives_and_no_cj_is_sent`.
  - `a_digital_answerer_that_does_not_do_quick_connect_ignores_qc1a_and_v8_completes`.
  - `a_caller_that_hears_jm_after_qc1a_finishes_v8`.
  - `a_slip_that_wrecks_the_only_qca1d_still_ends_in_a_v8_call`.
  - `a_resumed_answerer_starts_in_ansam`.
  - The existing V.8 tests pass.

### Wave 5: Phase 3 as far as Jd, and the Phase 4 sources

#### V92-32: The analogue modem, Phase 3 to Jd (M)

- **Depends on:** V92-02 (`Info0d`, `Info1aPcmUp` and `Info1c`, which `Settings::new` takes by
  reference), V92-05, V92-11, V92-12, V92-15, V92-27, V92-28 (as a test script).
- **Clauses:** 9.5.2.1.1-9.5.2.1.5; 9.5.2.2.1-9.5.2.2.2; 8.5.4, 8.5.5, 8.5.7; 6.2; Figures 10, 11.
- **Files:** `crates/datapump/src/v92/analogue.rs`.
- **Digests:** P3P (7.1 A1-A5, 7.2, 8, 13); P3S (3.1, 3.3, 3.4); CA (3, 4, 7, 11.2-11.4, 13, 14); CT (12).
- **Description:** the first half of the V.92 analogue modem.
  - `Settings::new(server: &Info0d, asked: &Info1aPcmUp, probe: Option<&Info1c>, rtd: Option<f64>, fs)`,
    with a test-only `no_dil` option; the optional probe and RTD cover short Phase 2 later.
  - The `Stage`, `Up` and `Deadlines` skeleton, `status()`, `phase()` strings ("V.92 phase 3: ..."), and
    `circuit_107()`.
  - Upstream: 70 +/- 5 ms of silence, Ru 384T, R-bar-u 24T, no MD, TRN1u at least 2040T in whole 12-symbol
    frames, then Ja - 24 ones then the descriptor repeated - cut at the next **12-bit** boundary after the
    Sd-to-S-bar-d reversal is detected, then silence. The descriptor comes from `dil::design` with the
    19-bit mask of the rates `UpEncoder` supports.
  - Downstream, through `pcm::Receiver` and `v90::downstream`: train on the first 2040T of TRN1d, then read
    Jd with a reader whose predicate requires bit 47 = 0.
  - Deadlines: TR3 relaxed as `SD_WAIT` is today (2.0 s + 2 RTD), with the departure documented; TR4 from
    the Ja cut with an RTD allowance; the start-up watchdog 20 s + 6 RTD from the end of INFO1a; and the
    **transmit-side limit of 9.5.2.1.2** - the interval from the start of MD to the end of TRN1u shall not
    exceed RTD + 4000 ms, measured from the end of the first R-bar-u when there is no MD (P3P 7.1 A2,
    timer T5, ambiguity 13.3 item 8; P2P P8). It bites hardest after a short Phase 2, where AD-10 gives the
    analogue modem RTD = 0 for "shall not exceed" limits, so TRN1u is then cut at 4000 ms whether or not Sd
    has been seen - a cut, not a failure, because TRN1u has already run its 2040T minimum by then.
  - Tone B requests a retrain.
- **Tests** (a scripted digital modem built from `DownSource` over `Network`, driven by time and events):
  - `the_upstream_opens_with_seventy_milliseconds_of_silence_then_ru_and_trn1u` - segment lengths measured
    at the line, Ru 384T and R-bar-u 24T exactly.
  - `ja_is_cut_at_the_next_twelve_bit_boundary_after_the_reversal` - over a sweep of reversal instants.
  - `trn1u_is_a_whole_number_of_twelve_symbol_frames_and_at_least_2040t`.
  - `md_and_trn1u_fit_inside_rtd_plus_four_seconds` - measured at the line terminals, swept over MD lengths
    (0 and a nonzero one) and over RTD = 0, 0.6 s and 1.5 s. With RTD = 0 the limit is 4000 ms from the end
    of R-bar-u and TRN1u is cut there, still on a 12-symbol boundary.
  - `a_jp_is_not_read_as_a_jd`.
  - `no_sd_gives_the_documented_failure_after_the_relaxed_wait`, and `no_jd_retrains` - both with their
    reason strings.
  - `tone_b_during_phase_3_asks_for_a_retrain`.
  - `the_ja_descriptor_decodes_with_the_upstream_rate_mask_we_meant`.

#### V92-33: The digital modem, Phase 3 to Jd (M)

- **Depends on:** V92-02 (`Info0d` and `Info1aPcmUp`, which `Settings::new` takes by reference), V92-11,
  V92-22, V92-27 (as a test script), V92-28.
- **Clauses:** 9.5.1.1.1-9.5.1.1.5; 9.5.1.2.1; 8.6.2, 8.6.7, 8.6.8; 9.7.1.1-9.7.1.2.
- **Files:** `crates/datapump/src/v92/digital.rs`.
- **Digests:** P3P (6.1 D1-D5, 6.2, 8, 13); P3S (5.1, 5.2, 5.7, 5.8); CD (3, 7, 8.2, 8.3, 11, 12).
- **Description:** the first half of the V.92 digital modem.
  - `Settings::new(law, info0d, asked: &Info1aPcmUp, rtd, habits, frame_origin: Option<u64>)`, where
    `frame_origin` lets short Phase 1 hand over a grid begun at QTS (V92-49).
  - Silence while it hunts Ru and the Ru-to-R-bar-u reversal, waiting out the MD when INFO1a asks for one;
    train on TRN1u; read Ja and its DIL descriptor with the 19-bit upstream rate mask.
  - Then, within 500 ms, Sd 384T and S-bar-d 48T from V.90 8.4.4, TRN1d at least 2040T from V.90 8.4.5, and
    Jd no later than 4000 ms after TRN1d starts, repeated. Jd advertises all rates with look-ahead 1, bit
    47 = 0 and bit 48 reserved.
  - Deadlines: DR1, 4.5 s + RTD from the end of INFO1a, and the start-up watchdog. Tone A requests a
    retrain. `Habits::LIVE_V92` gives a long TRN1d and no RTD in the Su wait.
- **Tests** (the real analogue upstream of V92-27 through `Network::up`, so the two halves meet without
  either state machine being finished):
  - `ru_then_trn1u_then_ja_is_read_off_the_line` - the descriptor decodes with the mask it was given.
  - `the_downstream_runs_sd_then_s_bar_d_then_trn1d_then_jd` - the lengths, and Jd within 4000 ms.
  - `the_optional_wait_before_sd_is_at_most_five_hundred_milliseconds`.
  - `no_ja_within_4_5_s_and_a_round_trip_asks_for_a_retrain`.
  - `a_ja_descriptor_names_the_dil_we_would_have_designed` - a round trip against `v90::dil::design`.
  - `a_non_zero_md_length_is_waited_out_before_ru_is_hunted_again`.
  - `sd_lands_on_interval_0_of_a_grid_it_was_given`.

#### V92-34: The upstream source for Phase 4, rate renegotiation and FPE (M)

- **Depends on:** V92-11, V92-12, V92-19, V92-27.
- **Clauses:** 8.7.1-8.7.7; 8.5.5; 9.6.2.1.1-9.6.2.1.5; 9.8.2.1.1-9.8.2.1.6; 9.8.2.2.2-9.8.2.2.3;
  9.9.2.1.1-9.9.2.1.2; 9.9.2.2.2-9.9.2.2.3.
- **Files:** `crates/datapump/src/v92/up_source.rs`.
- **Digests:** P4A (2, 3); RRF (2.4, 2.5, 2.7, 2.9, 2.11, 2.12, 2.16, 2.17); P4P (4).
- **Description:** fill in the `Up` variants **V92-27 already declared** with `unimplemented!()` bodies;
  the enum itself does not change, so `v92/analogue.rs` (V92-32, same wave) keeps compiling through the
  merge and neither package edits the other's file (section 6, wave 5).
  - **Trn2u:** the size comes from Jp bit 48 (training) or bit 49 (renegotiation) by context; the scrambler
    is reset at its start; the sign seed follows the section 4 reading.
  - **Suv, Cp and Cpus** in either of two modulations: TRN2u modulation padded to 24 or 36 bits, or
    data-mode modulation for FPE, padded to K through `UpEncoder`.
  - **E2u** with its optional 13th symbol; **B1u** on a new `UpEncoder` with the frame counter reset;
    **Data**; **Fb1u** on the old encoder, 48 frames; **Rm** and **RmPrime** on the old encoder with
    `force_k`, then the FPE reset; **Ru** and **RuBar** for renegotiation, bypassing the chain.
  - `change_at_frame_boundary`.
- **Tests:**
  - `trn2u_after_e1u_descrambles_to_ones_and_has_lu_squared_power`.
  - `suvu_in_trn2u_modulation_is_72_bits_at_either_size`.
  - `an_extended_e2u_moves_b1u_s_interval_0_by_one_symbol`.
  - `b1u_starts_interval_0_with_every_memory_zero`.
  - `rm_and_rm_prime_decode_to_their_k_patterns_through_the_old_chain`.
  - `an_fpe_suvu_decodes_after_the_reset_that_follows_rm_prime`.
  - `fb1u_uses_the_old_parameters_and_b1u_the_new_forty_nine_frames_after_e2u_began`.
  - `ru_for_a_renegotiation_starts_on_a_frame_boundary`.

#### V92-35: The downstream source for Phase 4, rate renegotiation and FPE (M)

- **Depends on:** V92-03, V92-11, V92-28.
- **Clauses:** 8.8.1-8.8.6; V.90 8.6 intro, 8.6.1, 8.6.2, 8.6.4, 8.6.5 and Table 17;
  9.6.1.1.1-9.6.1.1.5; 9.8.1.1.1-9.8.1.1.5; 9.8.1.2.2; 9.9.1.1.1-9.9.1.1.2; 9.9.1.2.2-9.9.1.2.3.
- **Files:** `crates/datapump/src/v92/down_source.rs`.
- **Digests:** P4D (2-4, 6, 8, 9, 13); RRF (2.1-2.3, 2.6, 2.13, 2.17); CD (3.2, 8.2).
- **Description:** fill in the `Out` variants **V92-28 already declared** with `unimplemented!()` bodies;
  the enum itself does not change, so `v92/digital.rs` (V92-33, same wave) keeps compiling through the
  merge (section 6, wave 5).
  - **Trn2d:** the V.90 `Encoder` with a `Mapping` built from the V.92 CPt, after the resets, starting from
    the zero state.
  - **Suvd and Cpd** padded to D bits, where D is: in training, from CPt; in renegotiation, K from CPt plus
    the data-mode S (section 4); in FPE, the data-mode D.
  - **Ed:** two frames of scrambled zeros, only after the sequence in progress is complete.
  - **B1d:** the CPu mapping after the resets; its first frame is the same for every look-ahead.
  - **Data**; **Rd and R-bar-d** at the loudest Ucode per interval of the CPu transmit constellation;
    **Rt and R-bar-t** at the CPt peaks; **Rf and R-bar-f** at the **same codewords as Rd** - the loudest
    Ucode of each interval of the data-mode constellation of the **CPu currently running** (8.8.4, P4D 6.2
    RF-3 and 12.1 item 9) - differing from Rd only in the sign pattern, `++--` on a 4-symbol period counted
    from Rf's start, 384T and 24T; **Quiet**.
  - The FPE reset before SUVd, and, after R-bar-t, SUVd's differential reference is "+".
- **Tests:**
  - `trn2d_uses_the_cpt_constellations_and_starts_from_zero_state`.
  - `an_ed_follows_a_complete_sequence_as_two_frames_of_scrambled_zeros`.
  - `suvd_and_cpd_pad_to_whole_frames_of_d_bits`.
  - `rd_uses_the_loudest_code_of_each_interval_s_data_constellation`.
  - `rf_is_plus_plus_minus_minus_for_384_symbols_then_24_inverted`, and it is told from Rd's six-symbol
    period in either polarity.
  - `rf_uses_the_same_codewords_as_rd_and_differs_only_in_the_sign_period` - Rd and Rf generated from the
    same running CPu and compared symbol by symbol: the magnitudes match interval for interval and only the
    signs differ, so an Rf built at the CPt levels or at a training level fails even though it would pass
    both sign-pattern tests.
  - `the_first_suvd_after_rt_bar_takes_plus_as_its_reference`.
  - `b1d_s_first_frame_is_the_same_for_every_look_ahead`.

### Wave 6: Phase 3 to the end

#### V92-36: The analogue modem, Su to E1u (M)

- **Depends on:** V92-23, V92-27, V92-32.
- **Clauses:** 9.5.2.1.6-9.5.2.1.11; 8.5.1, 8.5.2, 8.5.6; 9.6.2.2.1; 9.7.2.1-9.7.2.2.
- **Files:** `crates/datapump/src/v92/analogue.rs`.
- **Digests:** P3P (7.1 A6-A11, 9, 13); P3S (4.1, 4.2, 4.6, 6, 9); CA (11.4).
- **Description:** the rest of the analogue modem's Phase 3. After Jd, and no later than 5000 ms from the
  start of the silence: Su 144T, S-bar-u 24.5T (a half-symbol transmitter delay, not extra symbols), then
  Su until Jp arrives. On a CRC-valid Jp with bit 47 = 1: circuit 107 ON; S-bar-u for 24T plus the epsilon
  fraction from bits 18:33; the 4- and 8-point choices from bits 48 and 49 stored for Phase 4 and for
  renegotiation. Then TRN1u, whose first symbol is upstream interval 0, while the DIL arrives, read through
  `v90::downstream::DilReader`, with `RWatch` at UINFO for Ri and R-bar-i. After at least 2040T of TRN1u,
  and within 5000 ms of that S-bar-u, 24 ones then CPt repeated, built from `dil::choose` with the V.92
  header. On Ri: finish the CPt in flight, send E1u, and enter `Phase3Done` (a placeholder for V92-39).
  Accessors: `epsilon`, `jp`, `far_jd`, `descriptor`.
- **Tests:**
  - `su_is_one_hundred_and_forty_four_symbols_then_a_half_symbol_shift` - measured at the line through
    `Network::with_upstream_phase`.
  - `epsilon_from_jp_is_applied_to_the_second_s_bar_u` - eight epsilon values; the far A/D sees the symbols
    land where Jp asked.
  - `circuit_107_comes_on_when_jp_is_detected`.
  - `the_second_trn1u_starts_upstream_interval_zero` - the frame count at CPt and E1u is a multiple of 12.
  - `cpt_starts_after_2040t_of_trn1u_and_within_5_s_of_s_bar_u`.
  - `on_ri_the_current_cpt_is_finished_and_then_e1u_goes` - never mid-sequence.
  - `the_cpt_we_send_is_one_the_digital_modem_can_send` - drn, Sr and ld against the Jd we read.
  - `with_no_dil_asked_ri_comes_before_cpt_and_e1u_follows_ri_bar`.

#### V92-37: The digital modem, Su to R-bar-i (M)

- **Depends on:** V92-23, V92-28, V92-30, V92-33.
- **Clauses:** 9.5.1.1.6-9.5.1.1.13; 9.5.1.2.2; 8.6.1, 8.6.3-8.6.6; 9.6.1.2.1.
- **Files:** `crates/datapump/src/v92/digital.rs`.
- **Digests:** P3P (6.1 D6-D13, 6.2, 13.3 item 4); P3S (5.3-5.6); CD (8.3).
- **Description:** the rest of the digital modem's Phase 3. On detecting Su, measure the sampling phase
  across Su, the 24.5T S-bar-u and the Su after it (V92-23). Once epsilon is known, finish the current Jd
  and send Jp repeatedly; the size policy is 8-point in training when the TRN1u SNR allows and 4-point in
  renegotiation. On the second Su-to-S-bar-u reversal: finish the current Jp, assert circuit 107, send
  Jp-prime (12 zeros). Then send the DIL the descriptor asked for while receiving the second TRN1u, whose
  first symbol sets the frame origin and refines the receiver. On CPt: finish the DIL segment, send Ri. On
  E1u: send R-bar-i for 24T and enter `Phase3Done`. Both orders of Figure 11 and the text are accepted
  (P3P 13.3). The N = 0 / SCR path is implemented and parsed but never requested (section 4). Deadline
  DR2: no Su within 5100 ms + RTD of the start of TRN1d asks for a retrain.
- **Tests:**
  - `jp_follows_jd_only_once_the_phase_is_known` - and always at a Jd boundary; its epsilon matches the
    network phase.
  - `jp_prime_follows_the_second_reversal_and_107_comes_on_first`.
  - `the_dil_is_the_one_the_descriptor_asked_for` - segment lengths, reference symbols, sign and training
    patterns, ended on a segment boundary.
  - `ri_follows_cpt_and_r_bar_i_follows_e1u`.
  - `no_su_within_five_point_one_seconds_and_a_round_trip_asks_for_a_retrain`.
  - `the_scr_path_is_accepted_in_either_order` - a scripted N = 0 exchange.
  - `the_upstream_frame_count_starts_at_the_second_trn1u`.

### Wave 7: Phase 3 call to call

#### V92-38: Phase 3 between our two modems (M) - milestone M1

- **Depends on:** V92-04, V92-36, V92-37.
- **Clauses:** 9.5 as a whole; Figures 10, 11.
- **Files:** `crates/datapump/tests/v92_call.rs` (new); `crates/datapump/src/v92/analogue.rs`;
  `crates/datapump/src/v92/digital.rs`.
- **Digests:** CT (2.1, 8.1, 9.2); P3P (2, 9).
- **Description:** the first V.92 thing that is a call. `settled_v92()` returns the analogue and digital
  `Settings` as Phase 2 would leave them (INFO0d bit 27 set, INFO1d bit 70 set, a Table 18 INFO1a with both
  rate fields 6, UINFO 79, the filter capability codes, MD length 0). `Call` runs the two modems over
  `Network` one `tick()` at a time, in the shape `v90_call.rs` already uses, until both reach `Phase3Done`.
  Fix whatever the meeting shows, in `analogue.rs` and `digital.rs` only.
- **Tests:**
  - `phase_3_completes_over_a_clean_network` - both ends reach Phase 4 entry; the CPt the digital modem
    holds is the one the analogue modem sent; the DIL was read; the phase takes less than a stated wall
    time.
  - `phase_3_completes_with_no_dil_asked`.
  - `phase_3_completes_over_a_voip_length_round_trip` - 0.6 s each way; the relaxed TR3 and every
    RTD-bearing watchdog hold.
  - `the_digital_modem_s_epsilon_matches_the_network_s_sampling_phase` - phases 0.25 and 0.5, within a
    sixty-fourth of a symbol, and applied.
  - `a_sound_card_clock_a_hundred_and_twenty_ppm_off_is_followed_upstream` - ten seconds of Phase 3.
  - `phase_3_completes_on_an_a_law_network_and_with_a_robbed_bit_downstream`.

### Wave 8: Phase 4 and data mode

#### V92-39: The analogue modem, Phase 4 and data mode (L)

- **Depends on:** V92-11, V92-14, V92-19, V92-34, **V92-35** (the scripted digital Phase 4 its tests are
  driven from is built out of `DownSource`'s Phase 4 segments, which are V92-35's and are not in the
  transitive closure of the rest of this list), V92-38.
- **Clauses:** 9.6.2.1.1-9.6.2.1.6; 9.6.2.2; 9.6.2.2.1; 8.7 in full; 8.8 (reception); 9.11 (reception);
  Table 1 (circuits 104, 106, 109).
- **Files:** `crates/datapump/src/v92/analogue.rs`.
- **Digests:** P4P (5, 6.3-6.5, 9); P4A (3.1-3.6, 5.1, 8); P4D (2.6); CA (6, 11.5).
- **Description:** the natural seam, should this need two sittings, is at B1u.
  - **Upstream:** TRN2u at the size Jp bit 48 asked for, the scrambler reset at its start and the
    differential sign seeded from E1u's last sign, for at least 12000T or until an SUVd has arrived and we
    are ready to receive a CPd; then the `Exchange` in `Training` context - SUVu repeated, one CPu after
    the first SUVd (the V.92 header on the `dil::choose` result, **no upstream rate mask**, that moved to
    Ja), the ack bit once a CPd has arrived, repeats only on the 100 ms + RTD rule; then E2u honouring CPd
    bit 29; then B1u through an `UpEncoder` built from the CPd's `Parameters`; then data.
  - **CPd:** the first must carry every part, else retrain; later ones merge over the previous.
  - **SUVu's own fields in initial training** (Table 27, with section 4): the level, bits 27:31, is 16,
    "not measured", because nothing has yet been received through the data-mode chain; bit 32, the silence
    request, is 0, because only rate renegotiation defines a silent period; and **bit 26, "wait for my CPu
    before sending CPd", is always 0**, because our CPu is ready as soon as TRN2u has run, so asking the
    digital modem to wait would only lengthen the start-up for nothing. 9.6.1.1.2 makes complying a [MAY]
    in any case. V92-48 and V92-53 are where bits 27:31 stop being 16.
  - **Downstream:** a `Mapping` built from the V.92 CPt; the SUVd/CPd finder; Ed, then B1d with the CPu
    mapping, then data. Circuit 104 is unclamped and 109 turned on at the end of B1d; 106 follows 105.
  - **The peer's retrain tone, in Phase 4 and in data mode.** Tone B for more than 50 ms, in **any** state
    from the start of Phase 4 onwards, means the digital modem is retraining, and 9.6.2.2 with 9.7.2.2
    makes the response mandatory: 106 OFF, clamp 104, 70 +/- 5 ms of silence, our own Tone A, then full
    V.92 Phase 2 (P4P 6.4 A4-R0, 7.2, 7.5; RRF 9.2 "ANY: Tone B for more than 50 ms"). The watch is armed
    **here**, in the wave that first has a V.92 data mode and already owns `analogue.rs`, not in V92-48
    three waves later: V92-42's `a_retrain_from_either_end_comes_back_up_as_v92` runs in wave 10 and needs
    it, and rate renegotiation must not be the thing that first makes a data-mode retrain work. V92-48 and
    V92-55 reuse this watch rather than introducing one.
  - A far CPd with drn 0 clears the call down. `Connected { downstream, upstream }`, `take_bits`,
    `send_bits`, and the phase names "V.92 phase 4" and "V.92 data". The 20 s + 6 RTD watchdog from the end
    of sending INFO1a retrains if B1d never comes.
- **Tests** (a scripted digital Phase 4 from `DownSource`, with a CPd from V92-30 for an ideal channel):
  - `the_analogue_modem_connects_against_a_scripted_phase_4`.
  - `trn2u_runs_for_twelve_thousand_symbols_or_until_an_suvd_arrives`.
  - `one_cpu_goes_after_the_first_suvd_and_is_repeated_only_when_unacknowledged`.
  - `an_extension_asked_for_in_cpd_bit_29_is_sent_and_b1u_still_starts_interval_zero`.
  - `b1u_is_the_frames_the_cpd_parameters_predict` - an independently computed model matches symbol for
    symbol.
  - `the_cpu_asks_for_a_downstream_rate_the_jd_enabled`.
  - `a_cpd_with_drn_0_clears_the_call_down`.
  - `a_first_cpd_missing_a_part_asks_for_a_retrain`.
  - `no_b1d_within_20_s_and_six_round_trips_asks_for_a_retrain`.
  - `tone_b_during_phase_4_asks_for_a_retrain` - Tone B injected during TRN2u, during the exchange and
    during B1d: each gives 106 OFF, 104 clamped, 70 +/- 5 ms of silence measured at the line terminals,
    then Tone A, then Phase 2.
  - `tone_b_in_data_mode_asks_for_a_retrain` - the same after `Connected`, with data already crossing; the
    reason string says the far end asked.

#### V92-40: The digital modem, Phase 4 and data mode (L)

- **Depends on:** V92-14, V92-20, V92-21, V92-22, V92-30, **V92-34** (the reactive test double is built
  out of `UpSource`'s Phase 4 segments, which are V92-34's and are not in the transitive closure of the
  rest of this list), V92-35, V92-38.
- **Clauses:** 9.6.1.1.1-9.6.1.1.6; 9.6.1.2; 9.6.1.2.1; 8.8 in full; 8.7 (reception); 9.11 (reception).
- **Files:** `crates/datapump/src/v92/digital.rs`.
- **Digests:** P4P (5, 6.1, 6.2, 6.5); P4D (3-8, 10.1, 13); CD (7, 8.3).
- **Description:** the natural seam, should this need two sittings, is at B1d.
  - **Before the exchange:** TRN2d for at least 2040T with the CPt constellations, shaping and K. While it
    runs, read TRN2u at the Jp bit 48 size, build the channel estimate and design the filters (V92-21),
    choose the moduli, sets, rate and G (V92-62), assemble the CPd (V92-30), and verify the candidate
    against our own model of the transmitter before sending it. Once ready to receive a CPu, send SUVd
    repeatedly.
  - **The exchange** in `Training` context, honouring SUVu bit 26 ("wait for my CPu") while it costs
    nothing - that is, while our own CPd is not yet ready, or the CP repeat window has not expired - and
    ignoring it otherwise, which 9.6.1.1.2 allows [MAY]: one CPd, repeats on the 100 ms + RTD rule, then
    Ed. SUVu bits 27:31, the analogue modem's measured level, are read as signed Q2.2 and checked against
    the 8.8.3 design assumption that E[(G·v)^2] = 1: 16 means "no measurement" and is what initial training
    carries, while any other value is a real number and a mismatch beyond a stated margin is logged as
    something the next renegotiation's design should act on.
  - **Interchange circuits** (9.6.1.1.5, 9.6.1.1.6, Table 1/V.92; P4P 6.1 D4-5, D4-6 and section 9): after
    Ed and B1d, 106 follows 105; on receiving B1u, 104 is unclamped and 109 goes ON. They are modelled the
    way `v90/{analogue,digital}.rs` already model circuits - as data holdback - so the DTE side sees no
    data before B1u has arrived. V92-39 states the analogue mirror of this; without it here the digital
    modem's 104, 106 and 109 would be the only ones in the plan that are not modelled.
  - **The peer's retrain tone, in Phase 4 and in data mode.** Tone A for more than 50 ms, in any state from
    the start of Phase 4 onwards, means the analogue modem is retraining, and 9.6.1.2 with 9.7.1.2 makes
    the response mandatory: 106 OFF, clamp 104, 70 +/- 5 ms of silence, our own Tone B, then full V.92
    Phase 2 (P4P 6.2 D4-R0, 7.1, 7.5; RRF 9.1). Armed here, in the wave that first has a V.92 data mode and
    already owns `digital.rs`; V92-48 and V92-55 reuse it.
  - **B1d** with the CPu mapping; **E2u** accepted with or without the extra symbol (our policy is off, a
    test turns it on); **B1u** checked against the `UpEncoder` model built from the CPd we sent; then
    `UpDecoder` feeds `take_bits`.
  - `Connected`, `sent_cpd()` for tests, the start-up watchdog, and a far CPu with drn 0 clearing the call
    down.
- **Tests** (a reactive test double built from `UpSource` that reads `sent_cpd()`):
  - `the_digital_modem_connects_against_a_scripted_phase_4`.
  - `trn2d_runs_at_least_2040t_and_uses_the_cpt_constellations`.
  - `the_designed_cpd_verifies_before_it_is_sent` - the self-check runs on the real channel estimate.
  - `a_lost_cpu_is_answered_with_repeated_cpd_prime_as_in_figure_14`.
  - `ed_only_goes_after_the_current_sequence_is_complete`.
  - `asking_for_an_e2u_extension_moves_b1u_by_a_symbol`.
  - `b1u_is_checked_against_the_cpd_that_was_sent`.
  - `a_cpu_with_drn_0_clears_the_call_down`.
  - `no_b1u_within_20_s_and_six_round_trips_asks_for_a_retrain`.
  - `an_suvu_asking_us_to_wait_is_honoured_when_it_costs_nothing` - the double sends SUVu with bit 26 set:
    no CPd goes before the CPu while our CPd is not yet ready, and when the CP repeat window expires first
    the CPd goes anyway, which is the [MAY] half.
  - `the_measured_level_in_an_suvu_is_read_and_checked` - the double reports 16, then a real signed Q2.2
    level of +1.0 dB, then -2.0 dB: each is read back exactly, 16 is treated as "no measurement", and the
    two real ones are checked against the design assumption, so the V92-19 RMS tracker has a reader.
  - `the_dte_sees_no_data_before_b1u` - 104 stays clamped and 109 OFF until B1u has arrived, then both
    release, and 106 follows 105 from Ed onwards.
  - `tone_a_during_phase_4_asks_for_a_retrain`, and `tone_a_in_data_mode_asks_for_a_retrain` - 106 OFF, 104
    clamped, 70 +/- 5 ms of silence, then Tone B.

### Wave 9: the first complete V.92 call

#### V92-41: PCM both ways between our two modems (M) - milestone M2

- **Depends on:** V92-39, V92-40.
- **Clauses:** 1 (c-e); 6; 9.5; 9.6.
- **Files:** `crates/datapump/tests/v92_call.rs`; `crates/datapump/src/v92/analogue.rs`;
  `crates/datapump/src/v92/digital.rs`.
- **Digests:** CT (8.1, 9.2, 9.6); CD (9.2); P4P (5).
- **Description:** extend the V92-38 harness through Phase 4 and into data. This is the milestone the plan
  is ordered around: two of our modems, over the simulated network, PCM in both directions, carrying bytes.
  Fixes go in `analogue.rs` and `digital.rs` only.
- **Tests:**
  - `a_v92_call_connects_with_pcm_both_ways_and_carries_data` - `Status::Connected` at both ends; the
    downstream at or above 48 000; the upstream on the PCM ladder, at or above the floor V92-26 measured;
    data crosses both ways; no retrain. The reached rate is printed and recorded.
  - `the_rates_are_the_ones_the_two_ends_asked_for` - the CPu drn and the CPd drn match each side's choice.
  - `a_voip_length_round_trip_connects_with_pcm_upstream` - 0.6 s each way; the whole start-up fits inside
    20 s + 6 RTD with margin and the CP repeat rule never fires spuriously.
  - `an_a_law_network_connects_with_pcm_both_ways`.
  - `every_upstream_sampling_phase_is_found_and_corrected` - four phases; the eight-phase sweep is
    `#[ignore]`d.
  - `a_sound_card_clock_120_ppm_off_is_followed_upstream_too`.
  - `the_upstream_gain_at_the_codec_is_measured_and_used` - gains 0.25, 0.5 and 1.0.
  - `the_digital_modem_can_ask_for_the_32_and_64_state_codes`.
  - `a_far_end_that_stops_sending_is_noticed_at_either_end` - within 3.5 s, on upstream PCM levels as well
    as downstream.
  - `the_call_is_no_slower_than_v90_plus_a_half` - a wall-clock budget, so the suite stays near today's
    runtime.
  - All V.90 and V.34 tests still pass.

### Wave 10: the whole start-up, and the echo canceller

#### V92-42: Start-up hand-over, V.90 interop and the fallback ladder (M) - milestone M3

- **Depends on:** V92-16, V92-41.
- **Clauses:** 9.3; 9.3.1; 9.7; 8.4.1 (Table 18); 1 g; V.90 9.2.1.1.8 and 9.2.2.1.9; 9.6.1.2.1; 9.6.2.2.1.
- **Files:** `crates/datapump/src/v90/startup.rs`; `crates/datapump/src/v90/server.rs`;
  `crates/datapump/tests/v92_call.rs`.
- **Digests:** CA (2, 9.2, 11.2, 11.9); CD (2); CT (4.1, 8.2); P2P (7, 11.4).
- **Description:** joins Phase 2 to the V.92 modems and builds the ladder (AD-11), without adding a `Pump`
  variant (AD-4).
  - `V92Options`, and the constructors `Analogue::with_v92(fs, opts)`, `Digital::with_v92(info0d, opts)`
    and `server::ours_v92()`. `new()` stays V.90-only.
  - `enum Pcm { V90(v90::analogue::Modem), V92(v92::analogue::Modem) }` inside each wrapper, with `status`,
    `phase`, `take_bits`, `send_bits`, `pending_bits`, `carrier`, `retrain`, `renegotiate` and `clear_down`
    implemented once over it, and `pair()`, `points()`, `is_v92()`, `upstream_pcm()`,
    `lapm_bypass_allowed()` and the `last_failure()` strings passed through so `crates/modem` need not
    reach inside (this is also what keeps the scope working; CV R11).
  - The hand-over branch chooses by which INFO1a went: Table 18 builds the V.92 modem, Table 10 or Table 19
    builds today's V.90 modem.
  - Per-version failure counters: after `V92_RETRAINS` failed PCM-upstream start-ups call
    `decline_pcm_upstream()`, so the next INFO1a is Table 10 and the call keeps PCM downstream with V.34
    upstream; after `V90_RETRAINS` more call `decline_pcm()`, so the next INFO1a is Table 11 and the call
    continues as V.34. `Status` gains only the new failure reasons, not new variants.
- **Tests** (a `FullCall` in the shape of `v90_call.rs`'s):
  - `a_whole_v92_start_up_from_phase_2_connects` - V.8, full Phase 2, Phase 3, Phase 4, data;
    `is_v92()` and `upstream_pcm()` true.
  - `a_v92_analogue_modem_meets_a_v90_server_on_v90`, and `a_v90_analogue_modem_meets_a_v92_server_on_v90`.
  - `a_v90_server_that_happens_to_set_bit_70_does_not_get_pcm_upstream` - bit 70 means the 3429 carrier to
    a V.90 modem.
  - `a_v92_server_that_says_no_pcm_upstream_gets_v90_mode` - the downstream is still PCM.
  - `an_upstream_that_will_not_carry_24000_steps_down_to_v34_upstream_once` - upstream noise only; data
    crosses afterwards; no retrain loop.
  - `a_call_that_fails_twice_on_pcm_upstream_ends_as_v34_upstream_and_then_as_v34`.
  - `a_retrain_from_either_end_comes_back_up_as_v92` - always through full Phase 2.
  - `a_v92_modem_told_not_to_use_pcm_upstream_asks_for_v90_mode`.
  - Every `FullCall` test in `v90_call.rs` passes unchanged.

#### V92-43: The digital modem's echo canceller (M)

- **Depends on:** V92-17, V92-41.
- **Clauses:** 1 b; 9.5 (its title names echo canceller training); 9.8 (the silent period).
- **Files:** `crates/datapump/src/v92/receiver.rs`; `crates/datapump/src/v92/digital.rs`;
  `crates/datapump/tests/v92_impairments.rs` (new).
- **Digests:** CD (5.4, 7.5); CT (7, 10); CA (13).
- **Description:** `dsp::EchoCanceller` at 8 kHz on the A/D samples, the reference being our own downstream
  codeword. A near run of 32 to 64 taps covers the codec, the filters and the hybrid; an optional far run
  is placed by `dsp::EchoFinder` inside the measured round trip for a VoIP-length reflection. It adapts in
  the window where the analogue modem is silent (from the end of Ja to Su), then holds, then adapts slowly
  on the decision-directed residual in data mode. The residual must stay well under half the upstream level
  spacing, because the echo is added **before** quantising and cannot be undone afterwards.
- **Tests:**
  - `the_hybrid_s_echo_is_cancelled` - at 15 dB and 25 dB; the residual is below the stated fraction of the
    spacing and the upstream rate is within one rung of the echo-free rate.
  - `a_far_echo_a_round_trip_late_is_found_and_cancelled`.
  - `without_echo_the_canceller_changes_nothing`.

### Wave 11: robustness, the memo, and V.92 from the command line

#### V92-44: The V.92 replay probe (S)

- **Depends on:** V92-42.
- **Clauses:** none new.
- **Files:** `crates/datapump/tests/v92_replay.rs` (new).
- **Digests:** CT (2.3, 8.4).
- **Description:** `#[ignore]`d probes with their commands in the module doc, in the shape of
  `v90_replay.rs`: `probe_replay_v92` feeds channel 0 of a recording through `Analogue::with_v92` and
  prints the phases, epsilon, Jp and any CPd; `what_we_sent_is_what_we_meant` compares channel 1 against
  the transmitter's intended 8 kHz levels, which is the only way to see what the softphone did to our
  upstream.
- **Tests:** both are `#[ignore]`d and compile clean under clippy.

#### V92-45: Short Phase 2 in a whole start-up, with the recognised-connection memo (M)

- **Depends on:** V92-29, V92-42.
- **Clauses:** 1 j; 9.4; Table 19; 9.4.1.1.5 and 9.4.2.1.4 ("the appropriate Phase 3"); 9.5.2.1.2;
  9.6.2.2.1; V.90 6.2 and 9.3.
- **Files:** `crates/datapump/src/v92/memo.rs`; `crates/datapump/src/v90/startup.rs`;
  `crates/datapump/src/v90/analogue.rs`; `crates/datapump/src/v90/digital.rs`;
  `crates/datapump/tests/v92_call.rs`.
- **Digests:** P2P (6, 11, 12 A3/A5/A6/A10/A11); P2S (16.3); CA (11.2, 12); CD (6.3); CV (6.2.4).
- **Description:**
  - `Recognized` (AD-12) with a text round trip for the GUI file, and `Analogue::learned()`.
  - `with_v92(.., memo)` builds a `ShortPlan` only when a memo exists: Table 18 when the memo says PCM
    upstream works, otherwise Table 19. Without a memo the analogue modem does not request short Phase 2.
  - **New constructors, not a changed signature.** `v90::analogue::Settings::without_probe(..)` and
    `v90::digital::Settings::from_info1a(..)` are added **beside** the existing `new`, whose signature is
    left exactly as it is. `crates/datapump/tests/v90_call.rs` calls `analogue::Settings::new(&server(),
    &info1d, &asked, 0.02, true)` and `digital::Settings::new(Law::Mu, &info1d, &asked, 0.02, true)` at
    lines 27-28, and section 9.1 promises that file passes as it stands; making `new` take an
    `Option<&Info1c>` would stop the workspace compiling and force an undeclared edit to a protected test.
    The new constructors are for short Phase 2, which leaves no probe: with no `Info1c` they use the INFO1a
    symbol rate, the Table 19 carrier and pre-emphasis 0, and set `v34_receive` to 0; the digital one uses
    INFO1a bit 33 when there is no INFO1d.
  - **The Table 19 rate must still be one the INFO0s allowed.** Table 19 omits the "consistent with INFO1d"
    wording and carries no pre-emphasis index (P2P 12 A10, A11), but INFO0d bit 40 (3429 upstream) and our
    own INFO0a bits 15:19 still bind, and V.90 6.2 forbids 2400, 2743 and 2800 upstream. The constructor
    checks the INFO1a symbol rate against both and refuses with a named reason rather than building a
    `Settings` the far end cannot match.
  - RTD after a short Phase 2 comes from the memo (AD-10).
- **Tests:**
  - `a_second_call_with_what_the_first_taught_uses_short_phase_2_and_connects_v92`.
  - `short_phase_2_with_a_table_19_info1a_connects_in_v90_mode`.
  - `a_table_19_rate_the_info0s_did_not_allow_is_refused` - 3429 with INFO0d bit 40 clear, and each of
    2400, 2743 and 2800 upstream: refused with a named reason, not guessed at.
  - `settings_new_is_unchanged` - a compile-level check that the two `Settings::new` calls of
    `crates/datapump/tests/v90_call.rs` still type-check with the same arguments in the same order.
  - `without_a_memo_the_analogue_modem_does_not_ask_for_short_phase_2`.
  - `a_v92_server_that_does_not_ask_gets_full_phase_2_even_with_a_memo`.
  - `short_phase_2_over_a_1_5_s_round_trip_connects`.
  - `a_memo_round_trips_through_its_text_form`.

#### V92-46: Upstream robustness: slips, re-framing and the digital impairments (M)

- **Depends on:** **V92-42** (the fallback ladder that its transcoding and gain-control tests assert steps
  down), V92-43.
- **Clauses:** 8.5.7; 9.5.1.1.10; 6.4.1; 9.8 (R-COM-1).
- **Files:** `crates/datapump/src/v92/receiver.rs`; `crates/datapump/src/v92/decoder.rs`;
  `crates/datapump/src/v92/digital.rs`; `crates/datapump/tests/v92_impairments.rs`.
- **Digests:** CT (7, 9.3, 10); CD (9.2, 11); CA (8).
- **Description:**
  - **Re-framing after an upstream slip.** A 160-codeword slip moves the 12-symbol frame by 4 and an
    80-codeword cut by 8. Count frames the modulus encoder could not have produced (`could_have_sent`,
    using the moduli's room); three in the last 24 trigger a search over +/-4 and +/-8 symbols and then
    every shift, over recent symbols; the winner re-bases the interval count, the constellation index and
    the trellis phase; failing that, give up to a retrain.
  - **Lost/Found hold**, and medians rather than means in every estimator (the jitter-slip memory note).
  - **The digital start-up fails quickly, with a reason,** when upstream PCM never trains, so the V92-42
    ladder steps down instead of looping.
- **Tests:**
  - `upstream_slips_during_the_start_up_are_followed` - the five period-and-insert pairs `v90_call.rs`
    already uses, and the same downstream.
  - `an_upstream_slip_in_data_mode_is_followed_and_data_after_it_arrives`, with the frame moved by 4 and,
    for the cut, by 8.
  - `ten_millisecond_cuts_either_way_are_followed`.
  - `robbed_bits_both_ways_connect_and_carry_data` - downstream phase 2, upstream phase 5.
  - `a_digital_pad_either_way_connects` - (3, 0), (0, 3) and (6, 6) dB.
  - `a_transcoding_gateway_leaves_pcm_upstream_out_without_a_retrain_loop` - mu to A with the Crazytel
    response; the call settles on V.34 upstream.
  - `a_gain_control_below_our_quietest_useful_level_drops_pcm_upstream_once` - ceiling 0.3 stays under,
    ceiling 0.1 falls back once and not in a loop.
  - `a_softphone_that_passes_our_samples_straight_through_gives_pcm_upstream` - both 1-in-2 phases, one of
    them needing epsilon = 0.5 T; `a_softphone_that_resamples_what_we_send_gives_v34_upstream`.
  - `a_clock_off_through_a_softphone_is_slips_not_drift`.
  - `a_slip_anywhere_in_trn1u_is_found` - an `#[ignore]`d sweep reporting failure positions the way the
    V.90 DIL sweep does.

#### V92-47: `+MS=V92` from the AT command line to CONNECT (M)

- **Depends on:** V92-42.
- **Clauses:** V.250 6.4.1 (Table 13), 6.4.3; V.92 9.1 NOTE; V.8 Tables 4, 5, 7.
- **Files:** `crates/at/src/lib.rs`; `crates/at/tests/session.rs`; `crates/modem/src/lib.rs`;
  `crates/modem/tests/v92_call.rs` (new).
- **Digests:** CV (2, 3, 6.1, 6.6, 8); CTL (5.2, 5.3); CT (8.3, 9.4).
- **Description:**
  - **AT:** add `V92` to the `+MS` whitelist; parse and keep the 5th and 6th subparameters
    (`<min_rx_rate>`, `<max_rx_rate>`), because V.92 is asymmetric, and make a 7th an error; fix the
    `+MS=?` text, which advertises `(300-4800)` and cannot describe V.34, V.90 or V.92.
  - **Modem crate:** `offered()`, `place_call()` and `start_pump()` map V92 to the `with_v92` pumps with
    V.90's V.8 menus; `standard()` says "V.92" and the idle arms cover "V90" and "V92"; `transmit_rate()`
    reports the upstream PCM rate; the `distant()` rows gain PCM upstream and epsilon; `+MCR: V92` and
    `+MRR: <tx>,<rx>` are emitted and re-emitted after a renegotiation; the fallback carrier for
    `"V34" | "V90" | "V92"` is `"V32B"`.
- **Tests:**
  - In `session.rs`: `ms_v92_is_accepted_and_read_back`; `the_receive_rate_subparameters_are_kept`;
    `a_seventh_ms_subparameter_is_an_error`; `ms_test_advertises_rates_v92_can_actually_reach`; and
    `a_modulation_that_is_not_offered_is_refused` updated.
  - In `modem/tests/v92_call.rs`, copying `v90_call.rs`'s `Pair` with a caller-side softphone model:
    `dialling_a_v92_server_connects_with_pcm_both_ways_and_carries_text` (with a server retrain and text
    again afterwards); `two_of_these_connect_with_pcm_both_ways_when_both_softphones_pass_codewords` (the
    2 x 2 phase matrix, with the resampling host phase still ending on V.34, as the V.90 test shows);
    `ms_v90_still_gets_v90_from_a_v92_server`; `ms_v92_is_reported_in_the_standard_and_rates`.

### Wave 12: rate renegotiation, quick connect in the pumps, the GUI carrier

#### V92-48: Rate renegotiation without a silent period (M)

- **Depends on:** V92-14, V92-20, V92-34, V92-35, V92-46.
- **Clauses:** 9.8; 9.8.1.1.1-9.8.1.1.2; 9.8.1.2.1-9.8.1.2.3; 9.8.2.1.1-9.8.2.1.3; 9.8.2.2.1-9.8.2.2.3;
  Figure 15; 8.8.4; 8.5.5; 8.7.3 (CPus); R-COM-1..4.
- **Files:** `crates/datapump/src/v92/analogue.rs`; `crates/datapump/src/v92/digital.rs`;
  `crates/datapump/tests/v92_renegotiation.rs` (new).
- **Digests:** RRF (1-3, 5, 7, 9, 10.2); P4P (2.3); CA (9.1, 11.6); CD (3.3).
- **Description:** the V.92 flavour of what V.90 already does, reusing `Exchange` in `Renegotiation`
  context.
  - **Data-mode watches:** analogue - Rd through the generalised `RWatch` at the CPu peaks; digital - an Ru
    hunt on the A/D samples. The Tone A and Tone B watches and their 9.7.x.2 responses are **not**
    introduced here: V92-39 and V92-40 arm them in wave 8, when data mode first exists, and this package
    only reuses them.
  - Either end may start one, on **its own** data-frame boundary: 106 OFF, its R signal for 384T then its
    bar for 24T; the responder clamps 104, waits for the transition, and answers with its own R on its next
    frame boundary.
  - TRN2d for 2040T to 16008T; TRN2u at the **Jp bit 49** size - the *renegotiation* size, which may
    differ from the training size carried in bit 48 - running until SUVd or 16008T and **not** stopping
    early on a long RTD. SUVu and CPu follow that same size. Bit 49 is the one field whose only effect is
    in a rate renegotiation, so it is exercised here or nowhere (P3S 5.3; P4A 2.2, 4.3).
  - **SUVu's level field is a real measurement from here on.** Bits 27:31 carry 20*log10 of the RMS of
    G x the prefilter output, taken from the V92-19 tracker over the **preceding data mode**, quantised to
    signed Q2.2 and clamped to -3.75..+3.75 dB; 16 ("not measured") goes only if the tracker has nothing.
    This is the digital modem's only feedback about the received upstream level (P4A 3.6; RRF 2.9, 3.3
    RR-AI-3; P4P 4.4), and it is why the tracker exists.
  - The `Exchange`, with CPus when the downstream parameters are unchanged, and D in renegotiation per the
    section 4 reading; then B1 with the new parameters, both directions keeping data-frame synchronisation
    throughout.
  - `renegotiate(..)` on both modems, and the local watchdog (section 4) leading to a retrain, since 9.8
    gives no timeout at all.
- **Tests:**
  - `a_rate_renegotiation_from_either_end_settles_the_rates_asked_for` - 0.02 s and 0.6 s each way; data
    before and after; no retrain.
  - `the_frame_count_is_kept_across_the_whole_procedure` - both directions, every sequence a multiple of
    its frame.
  - `two_renegotiations_begun_together_converge`.
  - `a_renegotiation_that_changes_only_the_rate_sends_cpus`.
  - `a_line_gone_noisy_upstream_is_renegotiated_down_by_the_digital_modem`.
  - `a_stalled_renegotiation_turns_into_a_retrain`.
  - `data_crosses_after_a_renegotiation_without_a_retrain`.
  - `the_suvu_level_reports_the_measured_prefilter_power` - renegotiate with the transmitter gain
    deliberately 2 dB high, then 2 dB low: the field the digital modem receives is the measured value
    within one 0.25 dB step, is never 16, and moves in the right direction.
  - `a_renegotiation_uses_the_trn2u_size_jp_bit_49_asked_for` - settle Phase 3 with Jp bit 48 = 0 and
    bit 49 = 1, then renegotiate: the upstream TRN2u, SUVu and CPu are 8-point although the training ones
    were 4-point. Plus the mirror case (bit 48 = 1, bit 49 = 0), so a reader that read bit 48 twice fails.
  - `a_retrain_tone_during_a_renegotiation_is_answered` - the V92-39/V92-40 watch still fires mid-procedure
    and the 9.7.x.2 response runs.

#### V92-49: Short Phase 1, the PCM side in the start-up wrappers (M)

- **Depends on:** V92-13, V92-28, V92-31, V92-45.
- **Clauses:** 8.2.5; 8.3.1; 8.3.6; 9.2.1.3; 9.2.2.3; 9.2.3.3; 9.2.4.3; 9.2.5; Figures 3-6.
- **Files:** `crates/datapump/src/v90/startup.rs`; `crates/datapump/src/v90/server.rs`;
  `crates/datapump/tests/v92_quick.rs` (new).
- **Digests:** P1P (4, 5, 6, 10.1 P2-P5, P14, P15); P1D (10, pitfalls 12-16); CV (6.2.3, R4, R22).
- **Description:** QTS, QTS-bar and ANSpcm are generated **inside the digital pump**, not in
  `datapump::v8`, because they must be exact codewords at 8 kHz with the frame count starting at the first
  QTS symbol and carrying into Phase 2.
  - `Digital::after_quick_connect(info0d, uqts, lm, opts)`: 75 ms of Ucode 0; then QTS, QTS-bar and ANSpcm
    from `DownSource`, the frame origin being the first QTS symbol counted in the 8 kHz domain across
    Phase 2 and passed to `v92::digital` as `frame_origin` (`server::Line` keeps the 8 kHz count, not line
    samples); the TONEq detector armed once ANSpcm has started; on TONEq, stop ANSpcm, 75 ms of silence,
    then Phase 2. With no TONEq: 2 s after QCA1d when answering, or 2 s + RTD when calling (section 4),
    then `take_back_to_v8()`.
  - `Analogue::after_quick_connect(fs, uqts, opts, memo, ansam_seen)`: hunt QTS and its reversal and detect
    ANSpcm; send TONEq at once where the 9.2.3.3 MAY allows it (because on a 1.5 s round trip a full second
    of ANSpcm would miss the far end's 2 s deadline), else after 1 s, for at least 50 ms; when ANSpcm is
    lost - detected within 30 ms, so INFO0d is still heard - drop TONEq, 75 ms of silence, then Phase 2. If
    ANSam with its AM returns, `take_back_to_v8()`. An analogue answerer with no ANSpcm 2 s after QCA1a
    also goes back to V.8.
  - `Status` is unchanged (AD-5); 9.2.5's "both P bits set" is surfaced for the error-control layer.
- **Tests** (V.8 modems handing over to the pumps over `Network`):
  - `quick_connect_reaches_phase_2_and_connects_v92`.
  - `the_frame_grid_begun_at_qts_carries_to_sd`.
  - `a_digital_answerer_that_never_hears_toneq_asks_to_go_back_to_v8_within_2_s`.
  - `an_analogue_answerer_with_no_anspcm_two_seconds_after_qca1a_goes_back_to_v8` - the 9.2.3.3 mirror
    (P1P 5.3 R13, timer T16, state `A_ANS_WAIT_PCM`): ANSam goes out and V.8 continues. On this rig the
    window is marginal (P1P P14), so this test is the only thing that will show the rule working.
  - `an_analogue_caller_that_hears_ansam_come_back_asks_to_go_back_to_v8`.
  - `with_0_75_s_each_way_immediate_toneq_beats_the_2_s_window`.
  - `toneq_stops_within_30_ms_of_anspcm_so_info0d_is_heard`.
  - `the_caller_does_not_read_anspcm_as_v25_ans`.
  - `a_quick_connect_is_at_least_a_second_faster_than_the_full_v8_exchange`.

#### V92-50: GUI, the V.92 carrier (S)

- **Depends on:** V92-47.
- **Clauses:** V.250 6.4.1.
- **Files:** `crates/gui/src/app.rs`; `crates/gui/src/answer.rs`.
- **Digests:** CV (3.7, 6.7, R14, Q11).
- **Description:** **append** `("V92", ..)` to `CARRIERS` at index 6, never insert, because `CEILINGS` and
  the `rates` arms are index-based and pinned by tests. Its `rates` arm is the V.34 list plus the PCM
  upstream rates rounded down to whole bit/s; add a `CEILINGS` entry; enable the Retrain button for V.92 as
  well as V.34 and V.90; `answer.rs` help mentions `--carrier V92`.
- **Tests:**
  - `the_rate_lists_belong_to_the_carriers_they_are_indexed_by`, updated to seven carriers.
  - `v92_is_offered_after_v90_and_remembered_by_name`, and `v92_can_do_everything_v90_can`.
  - `v32bis_can_do_everything_v32_can` still passes.

### Wave 13: the silent period, and quick connect from the command line

#### V92-51: Rate renegotiation with a silent period (M)

- **Depends on:** V92-48.
- **Clauses:** 9.8.1.1.2-9.8.1.1.5; 9.8.1.1.3; 9.8.2.1.3-9.8.2.1.6; Figures 16-18; Table 27 and Table 31
  (bits 32, 33); 8.8.4 (Rt).
- **Files:** `crates/datapump/src/v92/analogue.rs`; `crates/datapump/src/v92/digital.rs`;
  `crates/datapump/tests/v92_renegotiation.rs`.
- **Digests:** RRF (2.2, 2.13, 3.5, 3.6, 10.2 items 8-9, 10.4); P4D (7.3, 9).
- **Description:** either end may ask with SUV bit 32; the two agree with bit 33; then Ed, then Ucode-0
  silence with the frame alignment kept; then Rt, R-bar-t and SUVd, and the exchange again. The
  post-silence TRN2u length when it is ours to choose is the section 4 reading. The ack state is cleared
  when the silence ends. On a line where 8004T is shorter than one round trip, an Rt arriving after the
  analogue modem has already ended its silence is still accepted, and Rt is never sent twice.
- **Tests:**
  - `a_digital_modem_s_silent_period_held_to_the_cap_as_in_figure_16`.
  - `a_digital_modem_s_silent_period_ended_early_by_rt_as_in_figure_17`.
  - `an_analogue_modem_s_silent_period_as_in_figure_18`.
  - `both_asking_for_silence_settles_after_8004t`.
  - `stale_primed_suvu_does_not_start_rt`.
  - `rt_and_the_8004t_timer_crossing_on_a_1_5_s_line_do_not_send_rt_twice`.
  - `a_silent_period_keeps_the_frame_count_in_both_directions`.

#### V92-52: Quick connect and the ODP/ADP bypass in the modem crate (M)

- **Depends on:** V92-08, V92-31, V92-47, V92-49.
- **Clauses:** 9.2; 9.2.5; 9.3.1; 1 j; V.42 7.2.1.
- **Files:** `crates/modem/src/lib.rs`; `crates/modem/tests/v92_call.rs`.
- **Digests:** CV (6.2.3, 6.2.4, R5, R7, R16); CTL (5.3, 6.2); P1P (5.5).
- **Description:**
  - **Start.** `negotiate` reads `v8line::Modem::quick()` and calls `start_pump_quick`, which starts the
    `after_quick_connect` pumps, or a V.34 pump in the right role where the outcome was V.34.
  - **The return to V.8.** An internal `Progress::BackToV8`, raised by `take_back_to_v8()`, rebuilds the
    negotiation with `resuming_call()` / `resuming_answer()`.
  - **The memo.** `quick_memo` and `learned()`, keyed by dial string with a single last-call slot
    otherwise. A QC is sent only with `+MS=V92` and a memo; the U_QTS and LM defaults are in section 4.
  - **The bypass.** After quick connect with both P bits set, or with both prot0 LAPM and
    `lapm_bypass_allowed()`: the originator uses `without_detection()`, the answerer `bypassing_detection()`.
- **Tests:**
  - `a_second_call_with_the_memo_connects_by_quick_connect_and_faster` - fewer V.21 seconds than the first.
  - `no_odp_or_adp_crosses_when_both_asked_for_lapm`.
  - `a_v90_answerer_ignores_qc1a_and_the_call_still_connects`.
  - `a_server_that_never_hears_toneq_is_still_reached_by_v8`.
  - `quick_connect_over_a_1_5_s_round_trip`.

### Wave 14: fast parameter exchange, cleardown, and the `+P` command surface

#### V92-53: Fast parameter exchange, and cleardown (L)

- **Depends on:** V92-20, V92-51.
- **Clauses:** 9.9; 9.9.1.1-9.9.2.2; Figure 19; 8.7.4 (RM); 8.7.7 (FB1u); 8.8.4 (Rf); 9.6.1.1.6;
  9.6.2.1.5; 9.11; Tables 23, 24, 30 (drn); V.90 9.7 NOTE.
- **Files:** `crates/datapump/src/v92/analogue.rs`; `crates/datapump/src/v92/digital.rs`;
  `crates/datapump/src/v90/startup.rs`; `crates/datapump/tests/v92_renegotiation.rs`.
- **Digests:** RRF (1.2, 1.4, 2.3, 2.5, 2.15, 2.18, 4, 6.2, 9, 10); P4A (3.5, 3.8, 5.3); P4D (6.2, 10.3,
  10.4); MOH (3).
- **Description:** the renegotiation with no training in it, plus the cleardown that rides on it. The seam,
  should this need two sittings, is between FPE and cleardown.
  - **FPE.** The digital modem sends Rf for 384T and R-bar-f for 24T - the same codewords as Rd, the
    loudest Ucode of each interval of the running CPu's data-mode constellation (V92-35), with a 4-symbol
    sign period instead of Rd's 6-symbol one, so the two are told apart in either polarity by an `RfWatch`
    built on the generalised `RWatch`. The analogue modem sends RM and RM', which go **through** the precoder, prefilter and trellis
    with their Ki forced, unlike Ru which bypasses them, and are therefore detected in the K domain by the
    V92-20 `RmWatch`, never as a tone. Both ends then zero the scrambler and the differential encoder (and
    the digital modem its shaper) and send SUV in the **old data-mode modulation**, straight into
    `Exchange` in `FastExchange` context. The analogue modem answers with E2u, then FB1u on the old
    parameters, then B1u on the new ones. Rate renegotiation takes precedence: an FPE initiator that hears
    Ru or Rd becomes the renegotiation responder. API: `fast_exchange()`, with the margin watch preferring
    FPE for rate-only changes.
  - **Interchange circuits, exactly as in a rate renegotiation.** The initiator turns 106 OFF and clamps
    104 before its first Rf or RM (9.9.1.1.1, 9.9.2.1.1); the responder clamps 104 on detecting Rf or RM
    (9.9.1.2.1, 9.9.2.2.1); both unclamp 104 and drive 109 at the end of the B1 they receive, through
    9.6.1.1.5-1.6 and 9.6.2.1.5-1.6, which the exchange rejoins (P4D 9; RRF 5 and Q-9; P4P 9). Clause 9.9
    says nothing else about circuits, so this mirrors whatever V92-48 asserts.
  - **SUVu's level field** carries the measurement over the preceding data mode, exactly as in a rate
    renegotiation (V92-48 and section 4): signed Q2.2 clamped to -3.75..+3.75 dB, and 16 only if the
    V92-19 tracker has nothing. 9.9.2.1.3 leaves bits 26 and 27:31 "as desired", and this is what we
    desire; bit 26 stays 0 for the reason V92-39 gives.
  - **CPus is the normal outcome of a rate-only fast exchange** - the case CPus exists for, and the likely
    result of a margin-driven rate change: type 2, CRC over bits 18:33, sent in the data-mode modulation
    and therefore padded to a whole number of **K-bit data frames**, not to 24 or 36 bits (Table 24,
    9.9.2.1.2; P4A 3.4; RRF 2.11).
  - **Cleardown.** `clear_down()` starts an FPE (preferred, because it needs no training) or a rate
    renegotiation and sends a rate sequence with drn = 0: CPu or CPus bits 21:25 from the analogue modem,
    CPd bits 22:26 from the digital modem. 9.11's "SUVu or SUVd" is an editorial slip (section 4). On
    receiving drn = 0 a modem completes the sequence in progress, **ignores that CP's constellation
    fields**, and reports `ClearedDown`, which is not a failure. The start-up wrapper passes `clear_down()`
    through for V.92.
- **Tests:**
  - `a_fast_parameter_exchange_from_either_end_changes_the_rate_without_losing_the_frames` - 0.02 s and
    0.6 s each way; data before and after; frame synchronisation kept.
  - `rm_goes_through_the_precoder_and_ru_does_not` - the line signals differ as expected and each is
    detected by its own watcher and not the other's.
  - `rf_is_told_from_rd_in_either_polarity`.
  - `a_renegotiation_wins_over_a_fast_parameter_exchange_begun_at_the_same_time` - both orders.
  - `two_fast_exchanges_begun_together_converge`.
  - `the_first_suv_after_the_reset_is_read`.
  - `a_fast_exchange_that_changes_only_the_downstream_rate_sends_cpus` - the sequence type is 2, it is
    padded to a whole number of K-bit data frames rather than to 24 or 36 bits, and the digital modem keeps
    its previous constellations and Sr.
  - `a_fast_exchange_clamps_104_and_drops_106_at_both_ends_and_releases_them_after_b1` - at 0.02 s and
    0.6 s each way, mirroring what V92-48 asserts for rate renegotiation.
  - `a_fast_exchange_suvu_reports_the_level_actually_measured` - as in V92-48, and never 16 once data mode
    has run.
  - `fb1u_then_b1u_switch_parameters_on_the_right_frame`.
  - `an_exchange_during_a_file_transfer_costs_no_data` - through the V.42 stack.
  - `a_cleardown_from_either_end_ends_the_call_at_both` - by FPE and by rate renegotiation.
  - `a_cleardown_in_phase_4_is_honoured`.
  - `the_constellation_fields_of_a_drn_0_cp_are_ignored`.
  - `a_cleardown_is_reported_as_a_cleardown_and_not_as_a_failure`, and
    `clear_down_is_passed_through_the_start_up_wrapper_for_v92`.

#### V92-54: The V.250 `+P` command surface (M)

- **Depends on:** V92-47, V92-50.
- **Clauses:** V.250 6.8.1-6.8.8 (Tables 31-37), 6.1.9, 6.8.4 with Table 34; V.92 Table 33 (the T1 codes).
  V.250 **5.4.2 is deliberately not cited**: CTL never rendered that page, so the plan would be quoting a
  clause no digest carries. The `+P` test replies instead follow the `+DS44` pattern already in
  `crates/at/src/lib.rs`, which is what CV 6.4 documents and what the test below actually checks.
- **Files:** `crates/at/src/lib.rs`; `crates/at/tests/session.rs`; `crates/modem/src/lib.rs`;
  `crates/gui/src/app.rs`.
- **Digests:** CTL (5.1, 5.2, 5.3); CV (6.4, R13, R15, R21, Q4, Q5).
- **Description:** the eight commands V.250 makes mandatory for a V.92 DCE, following the `+DS44` pattern
  already in the file. **The `at::Action` enum is matched exhaustively in `modem::run_actions` and
  `gui::app::perform`, so every new variant must be handled in the same package or the workspace will not
  compile** - that is why all four files are in this list.
  - A `V92` settings struct on `Interpreter` holding `+PCW` (0 toggle circuit 125, 1 hang up, 2 ignore),
    `+PMH` (0 **enables** hold - note the inverted sense), `+PMHT` (0 denies, 1-13 grant with the Table 33
    timeouts), `+PIG` (0 enables PCM upstream), `+PQC` (0 both short phases, 3 neither) and `+PSS` (0 the
    DCEs decide, 1 force short, 2 force full), each with read, test and set.
  - `Action::{SelectV92, RequestHold, HookFlash}`, plus the deferred results the action commands need:
    `+PMHR` answers late with `+PMHR: <n>`, or ERROR when hold is disabled or the modem is idle; `+PMHF`
    answers OK on hold and ERROR otherwise.
  - **`+PMHR` has two jobs** (V.250 6.8.4 with Table 34; CTL 5.1, 5.3, Q5, Q6). As an *initiator* it asks
    for a hold and reports the granted T1. As a *responder* it is the DTE's way to answer an incoming
    request. Section 4 fixes the reading: **`+PMHT` alone grants**, so a hold still works with no DTE
    watching, and `+PMHR` is a confirmation or an override - `+PMHR=0` denies the pending request, and the
    result **`+PMHR: 14`** means "denied, and further requests this session will be denied too", which is
    the local policy this package stores and `+PMHT` does not express. The behaviour tests are in V92-59,
    where hold really exists; here it is the parse, the result codes and the stored policy. Until V92-59 wires hold through, hold is unavailable, so
    `+PMHT` defaults to 0 and `+PMHR` honestly answers ERROR - which is the conforming answer for a
    disabled feature, not a stub.
  - The settings reach the pumps now: `+PIG` sets `pcm_upstream`; `+PQC` and `+PSS` set the quick-connect
    and short-Phase-2 flags on `V92Options`; `+PCW` goes through a `call_waiting()` hook. `+GCAP` lists the
    new names; `&F` and `Z` reset them; add `Interpreter::info()`.
- **Tests:**
  - In `session.rs`: `the_p_parameters_read_back_their_defaults`;
    `values_outside_tables_31_to_37_are_refused` and every accepted value is one that `=?` advertises;
    `the_p_test_replies_list_what_is_accepted` (each reply advertises exactly the set its setter accepts,
    in the `+DS44` style of the file); `pmhr_and_pmhf_parse_as_actions`;
    `pmhr_0_and_pmhr_14_parse_and_read_back`;
    `and_f_restores_the_p_defaults`; `every_command_gcap_names_is_answered`, updated.
  - In the modem crate: `pig_1_gives_v92_with_v34_upstream`; `pqc_3_puts_no_qc1a_on_the_line`;
    `pss_2_forces_a_full_start_up`; `pcw_1_hangs_up_on_a_call_waiting`;
    `pmhr_is_an_error_while_hold_is_disabled`.

### Wave 15: modem-on-hold in the pumps, and a graceful hang-up

#### V92-55: Modem-on-hold in the data pumps, on PCM-upstream connections (L)

- **Depends on:** V92-25, V92-41 (a working V.92 data mode). It does **not** depend on V92-53: hold needs
  neither fast parameter exchange nor cleardown. Wave 15 still follows wave 14 only because both packages
  enter `v92/analogue.rs` and `v92/digital.rs`, which is a merge gate rather than a logical prerequisite;
  section 14 records why the two were not swapped outright.
- **Clauses:** 9.10; 9.10.1.1 (the circuit-107 condition and the retrain race); 8.9.1; 9.7; Table 1.
- **Files:** `crates/datapump/src/v34/phase2.rs`; `crates/datapump/src/v34/startup.rs`;
  `crates/datapump/src/v90/startup.rs`; `crates/datapump/src/v92/analogue.rs`;
  `crates/datapump/src/v92/digital.rs`; `crates/datapump/src/v92/hold.rs`;
  `crates/datapump/tests/v92_hold.rs` (new).
- **Digests:** MOH (2, 9); CA (11.8); CV (6.3.2); P4P (7.6).
- **Description:**
  - **Requesting.** `request_hold(kind)` on the V.92 modems works in data mode only, after circuit 107, at
    a frame boundary, with 106 OFF, and starts the V92-25 transaction.
  - **Responding.** A tone watch in data mode leads to Phase 2's retrain response; that response listens
    for MH (`dpsk::Receiver::with_mh`) in its tone-awaiting and ranging stages, then `phase2.take_hold()`,
    and the wrapper runs the transaction as responder. A hold request and a retrain start identically, so
    the INFO framer runs beside the reversal detector and a validated MH frame wins.
  - **`hold_event()`** (an accessor, not a `Status` variant; AD-5): `Granted(t1)`, `Refused`,
    `Holding(t1)`, `Away`, `Cleared(reason)`, `FastReconnect{..}`, `GaveUp`.
  - **During hold** the status reads as retraining, and the carrier watch and every deadline are suspended.
  - The V.90-mode arm returns false until V92-57.
- **Tests** (two of our modems over `Network`):
  - `a_call_put_on_hold_is_granted_and_the_held_modem_sends_ansam` - from either end, at 0.6 s each way,
    with the carrier watch not reporting the far end gone.
  - `mhreq_is_answered_with_mhack_and_its_hold_time`, and MHnack when the policy denies.
  - `a_refused_hold_then_cleardown_ends_both_pumps`.
  - `a_refused_hold_then_fast_reconnect_ends_in_the_call_side_hearing_ansam`.
  - `a_cleardown_request_from_data_mode_ends_the_call`.
  - `a_hold_request_is_not_mistaken_for_a_retrain_and_a_retrain_is_not_mistaken_for_a_hold`.
  - `a_hold_request_wins_over_a_retrain_response` - the 9.10.1.1 R5 race.
  - `an_unanswered_hold_request_turns_into_a_retrain`.
  - `hold_is_refused_before_circuit_107`.
  - `a_hold_that_runs_out_ends_the_call` - timed from the end of the first MHack.

#### V92-56: Graceful hang-up in the modem crate (S)

- **Depends on:** V92-52, V92-53.
- **Clauses:** 9.11; V.90 9.7; V.250 6.3.6.
- **Files:** `crates/modem/src/lib.rs`; `crates/modem/tests/v92_call.rs`;
  `crates/modem/tests/v90_call.rs`.
- **Digests:** CV (2.1, 6.6, R11).
- **Description:** `Pump::clear_down` for V.34, V.90 and the V.90 server; an internal
  `Progress::ClearedDown` distinct from `Failed`; `ATH` becomes graceful with a bounded wait of 3 s + 2 RTD;
  "Force hang up" is unchanged; a far-end cleardown ends the call with `NO CARRIER`.
- **Tests:**
  - `when_one_end_hangs_up_the_other_sees_a_cleardown` - V.92 and V.90.
  - `ath_answers_ok_within_the_bounded_wait`.
  - `force_hang_up_still_drops_at_once`.
  - The existing hang-up tests pass.

### Wave 16: hold on the V.90-mode rung, and hold call control

#### V92-57: Modem-on-hold on V.90-mode connections between V.92 modems (M)

- **Depends on:** V92-55.
- **Clauses:** 9.10 (a V.92 connection running V.34 upstream); 9.10.1.1 (circuit 107 in V.90 Phase 3).
- **Files:** `crates/datapump/src/v90/analogue.rs`; `crates/datapump/src/v90/digital.rs`;
  `crates/datapump/src/v90/startup.rs`; `crates/datapump/tests/v92_hold.rs`.
- **Digests:** MOH (2.3); CA (9.2); CV (Q9).
- **Description:** hold is a property of the *connection*, not of the upstream modulation, so a V.92 pair
  that fell to the middle rung of the ladder must still be holdable. Wire the same data-mode tone watch and
  `request_hold` into the V.90 modems, gated on both ends being V.92 (from Phase 2's INFO0 flags), and make
  the V.90-mode arm of the wrapper return the real answer.
- **Tests:**
  - `a_v90_mode_call_between_v92_modems_can_be_held`.
  - `a_call_with_a_v90_only_modem_will_not_start_a_hold`.

#### V92-58: Hold call control in the modem crate (L)

- **Depends on:** V92-08, V92-52, V92-55, V92-56.
- **Clauses:** 9.10.2.1-9.10.2.3; 9.2 (the return by short Phase 1); V.8 8.1.2 and 8.2.3 (the zero CM);
  V.250 6.3.6, 6.3.7, 6.8.2-6.8.4, 6.8.6; V.42 7.10, 7.11.
- **Files:** `crates/modem/src/lib.rs`; `crates/modem/src/hold.rs` (new);
  `crates/datapump/src/v8.rs`; `crates/v8/src/lib.rs`; `crates/telemetry/src/lib.rs`;
  `crates/modem/tests/v92_call.rs`.
- **Digests:** CV (6.3.3, 6.5, R6, R9, R10, R18); MOH (2.5-2.8, 6.3-6.5, 7, 9.3); CTL (5.2-5.4).
- **Description:** per CV 6.3.3, a `Hold { Asking, Away, Holding }` state beside the V.250 states.
  - **API:** `request_hold()`, `hold_state()`, and a `call_waiting()` hook.
  - **The held side:** the pump ends and `v8line::Modem::held(t1, ..)` takes over - ANSam for T1 with no
    5 s or 60 s limits; a QC or a CM starts an answering Phase 1 (quick where allowed); a QC with U_QTS
    `1111` is a cleardown; a zero-modulation CM gets a zero JM and the call clears on CJ.
    **Everything the pre-hold Phase 1 taught is discarded** (9.10.2.1 R9; MOH 2.5, 9.3 pitfall 10): the
    resumed call re-reads the CM/JM menus, the LAPM indication, U_QTS and LM from the *new* Phase 1 and
    reuses none of the old ones. That matters most for the ODP/ADP bypass of V92-52, which is derived from
    the P bits or prot0 of the call in progress: a stale "both LAPM" would skip the detection phase on a
    link that never renegotiated it.
  - **The requester is away:** `ATO` resumes as a **retrain**, not as a new call, so the terminal sees no
    second CONNECT; it builds a resuming calling V.8 (quick with the memo) and the result is `CONNECT` via
    `pending_connect`; the `Stack` is kept, suspended and resumed (V92-08), so the V.42 sequence numbers
    and the V.42 bis / V.44 dictionaries survive.
  - **`ATH` on hold** sends QC1a with U_QTS `1111` (analogue) or a zero CM, then drops.
  - **Three existing guards must learn about hold** or a granted hold ends the call at once: carrier loss
    in `carry_data`, the V.42 clocks in `tick`, and V.8's 5 s ANSam and 60 s patience limits.
  - `Ended::ClearedDown` and `Ended::HoldExpired` report `NO CARRIER`; `CallState::OnHold` joins the
    telemetry states with the label "on hold" and `Leds.oh` staying true; the `distant()` rows gain hold.
- **Tests** (`Pair`, two whole modems):
  - `a_v92_call_put_on_hold_says_nothing_to_the_terminal_and_resumes_with_the_same_link` - the V.42 frame
    counters continue and no second CONNECT is emitted.
  - `a_hold_that_runs_out_ends_the_call_with_no_carrier`.
  - `ath_on_hold_gives_the_held_side_no_carrier_within_2_s`.
  - `a_cleardown_cm_ends_the_held_call_with_no_carrier`.
  - `a_refused_hold_with_fast_reconnect_comes_back_up`.
  - `a_held_call_does_not_time_out_while_it_is_held` - T401 frozen; no `NO CARRIER`.
  - `text_sent_before_the_hold_arrives_after_it`.
  - `a_resumed_call_renegotiates_its_menus_and_bypass_rather_than_reusing_the_old_ones` - the far end
    offers LAPM before the hold and not after: the resumed call runs the detection phase; and the mirror
    case, no LAPM before and LAPM after, bypasses it.
  - `a_hold_across_a_one_and_a_half_second_round_trip_still_resumes`.

### Wave 17: the hold commands, and the GUI

#### V92-59: The hold AT actions wired up (S)

- **Depends on:** V92-54, V92-58.
- **Clauses:** V.250 6.8.2-6.8.6; V.92 Table 33.
- **Files:** `crates/at/src/lib.rs`; `crates/at/tests/session.rs`; `crates/modem/src/lib.rs`.
- **Digests:** CTL (5.1); MOH (7); CV (6.4).
- **Description:** now that hold exists, `+PMH` and `+PMHT` really set the grant policy the pumps use,
  `+PMHR` initiates and answers late with `+PMHR: <n>` carrying the granted T1 code, and `+PMHF` answers OK
  while on hold and logs (it cannot really flash the hook; see section 9). `+PMHT`'s default moves from 0
  to the value the GUI or `answer.rs` sets.
- **Tests:**
  - `at_pmhr_is_answered_with_the_granted_t1_then_ok` - `+PMHR: 5` for a one-minute grant.
  - `pmht_0_at_the_far_end_gives_pmhr_0`.
  - `pmh_1_makes_pmhr_an_error`.
  - `pmhf_is_ok_on_hold_and_an_error_otherwise`.
  - `pmhr_confirms_an_incoming_request` - `+PMHT` has already granted and the DTE is told; `+PMHR` from the
    DTE confirms it, and `+PMHR=0` denies the pending request instead, which sends MHnack.
  - `pmhr_14_denies_the_rest_of_the_session` - once the local deny-for-this-session policy is set the
    result is `+PMHR: 14`, and a second incoming request is refused with MHnack without reaching the DTE.

#### V92-60: GUI: quick connect, hold controls, status and the memo (M)

- **Depends on:** V92-45, V92-54, V92-58.
- **Clauses:** V.250 6.8; V.92 1 j.
- **Files:** `crates/gui/src/app.rs`; `crates/gui/src/live.rs`; `crates/gui/src/remembered.rs`;
  `crates/gui/src/answer.rs`.
- **Digests:** CV (2.6, 6.7, R16, R19); CTL (5.4).
- **Description:** in keeping with the file's rule that GUI controls type AT commands.
  - Settings: "quick" and "hold" checkboxes (`AT+PQC=`, `AT+PMH=`) in a `Pcm` settings struct that is
    remembered.
  - Buttons: Hold (`AT+PMHR`), Resume (`ATO`), Flash (`AT+PMHF`), and a simulated Call waiting button
    through a `Session` atomic.
  - State: `CallState::OnHold` in the frame, hold transitions logged the way retrains are, and a start-up
    row (quick or full, short Phase 2 or not) and a hold row in the status grid.
  - The recognised-connection memo is saved and loaded, keyed by dial string or a last-call slot.
  - `answer.rs --grant-hold <0..13>`, which types `AT+PMHT=<n>`.
- **Tests:**
  - `the_quick_and_hold_toggles_type_their_commands`.
  - `the_p_settings_are_remembered`, and `what_the_last_run_was_set_to_comes_back`.
  - `the_quick_connect_memo_round_trips_through_its_file`.
  - `on_hold_is_shown_as_its_own_call_state_with_its_own_label`.
  - `the_hold_buttons_are_enabled_only_where_they_can_work`.
  - `answer_grant_hold_types_pmht`.

### Wave 18: the scope

#### V92-61: Scope, the upstream PCM pair plot at the server (S)

- **Depends on:** V92-42, V92-60.
- **Clauses:** none. This follows Rory's display choice: the consecutive-sample pair plot.
- **Files:** `crates/modem/src/lib.rs`; `crates/datapump/src/v90/startup.rs`;
  `crates/datapump/src/v92/digital.rs`; `crates/gui/src/app.rs`.
- **Digests:** CV (6.1 item 6, 6.7); CA (3.2); CT (8.3); and the `v90-pcm-display-choice` memory note.
- **Description:** `v92::digital` publishes (sample n, sample n+1) pairs of the received upstream A/D
  levels, the way the analogue modem already publishes the downstream ones, labelled so the panel can tell
  the two directions apart. The wrapper passes them through and the modem crate offers them for
  `V90Server` with V.92 upstream. `gui::scopes` already draws pair plots (`Constellation { pairs }`), so
  only the feed and the labelling are new: permanent "sample n / sample n+1" axis labels in the existing
  footer style.
- **Tests:**
  - `a_v92_server_s_scope_shows_consecutive_upstream_sample_pairs` - a unit test on the published frame.
  - `the_analogue_side_still_shows_the_downstream_pairs`.
  - `the_pcm_label_selects_the_pair_plot_for_either_direction`.

---

## 6. Waves

Work packages in a wave never edit the same file, so each can be taken in its own git worktree and merged
without conflict. A wave ends when all of its packages are merged and the workspace is green. Paths are
abbreviated: `dp` = `crates/datapump/src`, `dpt` = `crates/datapump/tests`.

| Wave | Packages | Files touched (disjoint within the wave) |
|---|---|---|
| 1 | V92-01 .. V92-08 | `dp/lib.rs` + `dp/v92/*` (new stubs) · `dp/v34/{info,dpsk,phase2}.rs` + `dpt/{v90,v34}_vector.rs` · `dp/v90/sequences.rs` · `dp/v90/network.rs` · `dp/v90/{analogue,downstream,mod}.rs` · `dp/v90/pcm.rs` · `v8/src/{quick,lib}.rs` · `ec/src/{stack,detect,lapm}.rs` |
| 2 | V92-09 .. V92-18 | `v92/modulus.rs` · `v92/precoder.rs` · `v92/sequences.rs` · `v92/up_signals.rs` · `v92/anspcm.rs` · `v92/exchange.rs` · `v92/transmit.rs` · `dp/v34/{phase2,startup}.rs` + `dp/v90/mod.rs` (V92-16) · `dp/v90/network.rs` · `dpt/v92_vector.rs` |
| 3 | V92-19 .. V92-25, V92-62 | `v92/upstream.rs` · `v92/decoder.rs` · `v92/design.rs` · `v92/receiver.rs` · `v92/epsilon.rs` · `dp/v34/phase2.rs` · `v92/hold.rs` + `dp/v8.rs` · `v92/upchoice.rs` (V92-62) |
| 4 | V92-26 .. V92-31 | `dpt/v92_upstream.rs` · `v92/up_source.rs` · `v92/down_source.rs` · `dp/v34/phase2.rs` · `v92/upchoice.rs` (V92-30, which V92-62 wrote in wave 3) · `dp/{v8,bell103}.rs` |
| 5 | V92-32 .. V92-35 | `v92/analogue.rs` · `v92/digital.rs` · `v92/up_source.rs` · `v92/down_source.rs` — **shared merge gate:** V92-32 and V92-33 `match` on the `Up` and `Out` enums that V92-34 and V92-35 fill in. V92-27 and V92-28 declare every variant in wave 4 with `unimplemented!()` bodies so the four compile independently, and the wave still does not close until V92-32 and V92-33 have been re-run against the merged sources. |
| 6 | V92-36, V92-37 | `v92/analogue.rs` · `v92/digital.rs` |
| 7 | V92-38 | `dpt/v92_call.rs` + `v92/{analogue,digital}.rs` |
| 8 | V92-39, V92-40 | `v92/analogue.rs` · `v92/digital.rs` |
| 9 | V92-41 | `dpt/v92_call.rs` + `v92/{analogue,digital}.rs` |
| 10 | V92-42, V92-43 | `dp/v90/{startup,server}.rs` + `dpt/v92_call.rs` · `v92/{receiver,digital}.rs` + `dpt/v92_impairments.rs` |
| 11 | V92-44 .. V92-47 | `dpt/v92_replay.rs` · `v92/memo.rs` + `dp/v90/{startup,analogue,digital}.rs` + `dpt/v92_call.rs` · `v92/{receiver,decoder,digital}.rs` + `dpt/v92_impairments.rs` · `at/src/lib.rs` + `at/tests/session.rs` + `modem/src/lib.rs` + `modem/tests/v92_call.rs` |
| 12 | V92-48, V92-49, V92-50 | `v92/{analogue,digital}.rs` + `dpt/v92_renegotiation.rs` · `dp/v90/{startup,server}.rs` + `dpt/v92_quick.rs` · `gui/src/{app,answer}.rs` |
| 13 | V92-51, V92-52 | `v92/{analogue,digital}.rs` + `dpt/v92_renegotiation.rs` · `modem/src/lib.rs` + `modem/tests/v92_call.rs` |
| 14 | V92-53, V92-54 | `v92/{analogue,digital}.rs` + `dp/v90/startup.rs` + `dpt/v92_renegotiation.rs` · `at/src/lib.rs` + `at/tests/session.rs` + `modem/src/lib.rs` + `gui/src/app.rs` |
| 15 | V92-55, V92-56 | `dp/v34/{phase2,startup}.rs` + `dp/v90/startup.rs` + `v92/{analogue,digital,hold}.rs` + `dpt/v92_hold.rs` · `modem/src/lib.rs` + `crates/modem/tests/{v92_call,v90_call}.rs` |
| 16 | V92-57, V92-58 | `dp/v90/{analogue,digital,startup}.rs` + `dpt/v92_hold.rs` · `modem/src/{lib,hold}.rs` + `dp/v8.rs` + `v8/src/lib.rs` + `telemetry/src/lib.rs` + `modem/tests/v92_call.rs` |
| 17 | V92-59, V92-60 | `at/src/lib.rs` + `at/tests/session.rs` + `modem/src/lib.rs` · `gui/src/{app,live,remembered,answer}.rs` |
| 18 | V92-61 | `modem/src/lib.rs`, `dp/v90/startup.rs`, `v92/digital.rs`, `gui/src/app.rs` |

**Two different files are abbreviated `v90_call.rs`.** `dpt/v90_call.rs` is
`crates/datapump/tests/v90_call.rs`, the V.90 pump call harness; `crates/modem/tests/v90_call.rs` is the
whole-modem one that V92-56 edits in wave 15. No package edits the first: V92-45 adds
`Settings::without_probe` and `Settings::from_info1a` beside the existing `new` rather than changing a
signature that file calls (section 9.1, section 14). A worktree scheduler must not merge or split the two
rows by name.

Where the parallelism is: waves 1 to 4 are six to ten packages wide, which is where most of the new code
is. Waves 5 to 9 are narrow by design, because each is one step of a call and the step after it cannot be
written until the step before it works. From wave 10 the plan widens again, because the remaining features
live in different crates - except for `v92/analogue.rs` and `v92/digital.rs`, which serialise one feature
per wave and are always paired with a package in another crate.

## 7. Milestones

| Milestone | Wave | What works |
|---|---|---|
| **M0 de-risked** | 4 | V92-26: the designed precoder, the trellis-coded modulus chain and the decoder carry the upper half of the upstream ladder over a codec-like channel, through the real receiver, at every sampling phase. The explicit go/no-go for PCM upstream. |
| **M1 Phase 3 call to call** | 7 | V92-38: our two V.92 modems complete Phase 3 over `Network`, including epsilon. |
| **M2 the first complete call** | 9 | V92-41: Phases 3 and 4 connect with PCM both ways and carry data. |
| **M3 a whole start-up** | 10 | V92-42: V.8, full Phase 2, V.92 Phases 3 and 4, data, with the V.90 interop pairings and the fallback ladder. |
| **M4 robustness and quick connect** | 11-13 | Echo, slips, pads, robbed bits, transcoding, long round trips, softphone paths; short Phase 2 with the memo; short Phase 1 on the line and in the pumps. |
| **M5 renegotiation** | 12-14 | Rate renegotiation with and without a silent period, fast parameter exchange, cleardown. |
| **M6 hold** | 15-17 | Modem-on-hold in the pumps on both rungs of the ladder, and its call control. |
| **M7 the product** | 11-18 | `+MS=V92`, the `+P` commands, the GUI carrier and controls, the upstream PCM scope. |

## 8. Dependency outline

```text
V92-01 ─┬─ V92-09 ─┬─ V92-19 ─┬─ V92-26 (M0, +V92-17, V92-20..23, V92-62)
        │          ├─ V92-20 ─┤
        │          └─ V92-62 ─┴─ V92-30 (+V92-11, V92-19, V92-20, V92-21)
        ├─ V92-10 ─┬─ V92-21 ──┘
        │          └─ V92-62
        ├─ V92-11 (+V92-03) ─┬─ V92-27 ─┬─ V92-32 ─ V92-36 ─┐
        ├─ V92-12 ─┬─ V92-22 ┤          └─ V92-34 ─────────┐│
        │          └─ V92-23 ┤                             ││
        ├─ V92-13 ───────────┴─ V92-28 ─┬─ V92-33 ─ V92-37 ─┴┼─ V92-38 ─┬─ V92-39 ─┬─ V92-41 ─ V92-42 ─┐
        ├─ V92-14 ──────────────────────┴─ V92-35 ──────────┴─ V92-40 ─┘          │                   │
        └─ V92-15 (+V92-04, V92-06)                                                                    │
V92-02 ─┬─ V92-16 ─ V92-24 ─ V92-29 ──────────────────────────────────────────────────── V92-45 ───────┤
        ├─ V92-25 (+V92-01) ──────────────────────── V92-55 (+V92-41) ─┬─ V92-57                       │
        ├─ V92-32, V92-33                                              └─ V92-58 ─┬─ V92-59            │
V92-03 ─ (V92-05 excepted: see below) ─ (V92-11, V92-22, V92-27, V92-28, V92-35)   └─ V92-60 ─ V92-61
V92-05 ─ V92-32
V92-07 ─ V92-31 ─┬─ V92-49 ─ V92-52 ─┬─ V92-56 ─ V92-58
V92-08 ──────────┴─ V92-52           │
V92-17 ─┬─ V92-26                    │
        └─ V92-43 ─ V92-46 (+V92-42) ─ V92-48 ─ V92-51 ─ V92-53 ─┘
V92-42 ─┬─ V92-44
        ├─ V92-45 ─ V92-49
        ├─ V92-46
        └─ V92-47 ─┬─ V92-50 ─ V92-54 ─ V92-59
                   └─ V92-52
```

Edges the diagram cannot draw, each of them real and each now declared in the package itself:

- **V92-11 → V92-19, V92-20** (`rm_k`, Tables 25 and 26, with the interval-11 errata).
- **V92-03, V92-11 → V92-22** (the Ja descriptor, CPt and SUV/CP finders the receiver is plumbed to).
- **V92-01 → V92-25** (`hold.rs` and its `pub mod hold;` line) and **V92-01 → V92-14** (`PeerSuv`,
  `PeerCp`, `SequenceKind`, which live in `v92/mod.rs` and which V92-14 may not write itself).
- **V92-02 → V92-32, V92-33** (`Info0d`, `Info1aPcmUp`, `Info1c` in both `Settings::new`s).
- **V92-35 → V92-39** and **V92-34 → V92-40** (each one's tests are driven by the other side's source).
- **V92-62 → V92-26 and → V92-30**; **V92-17 → V92-26** (the echoed cases in the M0 sweep).
- **V92-42 → V92-46** (the fallback ladder its last four tests assert steps down).
- **V92-05 does *not* depend on V92-03.** The moved `JdReader` takes a caller-supplied acceptance closure
  over the raw framed bits, so the bit-47 knowledge stays in V92-03 and V92-11 and wave 1 really is
  parallel. V92-05 also makes `v90::carrier` `pub(crate)`, which V92-41, V92-42 and V92-55 need.
- **V92-55 does *not* depend on V92-53.** Wave 15 follows wave 14 as a merge gate on `v92/analogue.rs` and
  `v92/digital.rs`, not as a prerequisite (section 14).

The critical path is V92-01 → V92-09/10 → V92-19/20/21/62 → V92-26 → V92-30 → V92-40 → V92-41 → V92-42.
The first live-worthy V.92 build is V92-47 (wave 11); its prerequisites are M2 and M3.

## 9. Keeping V.90 and V.34 working

1. **No existing test is edited.** New tests are added; new `Info` variants add match arms only.
   `crates/datapump/tests/v90_call.rs`, `v90_vector.rs`, `v90_replay.rs`, `v34_capture.rs`,
   `v34_vector.rs`, `crates/modem/tests/v90_call.rs` and `crates/modem/tests/call.rs` all pass as they
   stand. That is why V92-45 adds `Settings::without_probe` and `Settings::from_info1a` **beside** the
   existing `new` instead of widening `new`, which `crates/datapump/tests/v90_call.rs:27-28` calls.
2. **`startup::Analogue::new` and `startup::Digital::new` stay V.90-only** (AD-4). V.92 is reached only
   through `with_v92`, which is chosen only by `+MS=V92`. So `+MS=V90` against a V.92 server behaves
   exactly as today.
3. **Additive fields default to today's behaviour.** `Info0d::v92` defaults to false; the existing `Pcm`
   constructors mean "not V.92" and still produce a Table 10 INFO1a. Every shared codec change in V92-02
   and V92-03 carries a "the V.90 encoding is bit-identical" test.
4. **V92-05 is the only behaviour-preserving move, and it is committed on its own**, with the slip tests
   and the ignored DIL sweep counted before and after and the count recorded in the commit message.
5. **Every `Network` addition keeps today's defaults**, and V92-04 proves the tabulated kernel equals the
   old computation exactly.
6. **The V.90 modems are the fallback, not dead code** (AD-2). V92-42 re-runs every V.90 `FullCall` test
   and adds V.92-to-V.90 pairings in both directions; V92-46 exercises the V.34-upstream rung on
   transcoding paths, which is what carries real calls today (the Crazytel memory note). Any V.34 fallback
   bug found on the way is flagged as a separate task, never fixed inside a V.92 package.

## 10. Fallbacks the Recommendation requires, and where each is tested

| Situation | Required behaviour | Clause | Tested in |
|---|---|---|---|
| Either modem not V.92 | V.90 INFO layouts (Tables 9/10/11), V.90 Phases 3 and 4 | 9.3 | V92-16, V92-42 |
| INFO1d bit 70 clear | Table 18 must not be sent; Table 10 or 11 instead | 8.4.1 | V92-16, V92-42 |
| A V.90 server that sets bit 70 | bit 70 means the 3429 carrier, not PCM upstream | 8.4.1 | V92-42 |
| Short Phase 2 not requested by both | full Phase 2 | 9.4 | V92-24 |
| Analogue modem not intending PCM upstream or V.90 mode | must not request short Phase 2 | 9.4 | V92-24, V92-45 |
| No A reversal or no INFO1a (short) | digital modem into full Phase 2 at FD-3 | 9.4.1.2.2-3 | V92-29 |
| No B reversal (short) | analogue modem retrains into full Phase 2 | 9.4.2.2.2 | V92-29 |
| Retrain of a V.92 pair | always V.92 full Phase 2, no INFO0 | 9.3, 9.7 | V92-16, V92-42 |
| Table 19 INFO1a | V.90 data mode, i.e. V.90 Phase 3 | 9.4.x.1.x | V92-45 |
| No QCA after QC1a | V.8 continues; the CM was already sent | 9.2.1.1, 9.2.2.1 | V92-31, V92-52 |
| Non-V.92 answerer | ignores QC1a; ordinary V.8 | V.8 clause 10 | V92-31, V92-52 |
| No TONEq within 2 s (digital answerer) | ANSam, back to V.8 | 9.2.4.3 | V92-49, V92-52 |
| No ANSpcm within 2 s (analogue answerer) | ANSam, back to V.8 | 9.2.3.3 | V92-49 |
| A Phase 3 or Phase 4 watchdog expires | retrain | 9.5.x.2, 9.6.x.2 | V92-32, V92-33, V92-36, V92-37, V92-39, V92-40 |
| The peer's retrain tone, in any phase and in data mode | 106 OFF, clamp 104, 70 +/- 5 ms of silence, our own tone, then full Phase 2 | 9.7.1.2, 9.7.2.2 | V92-32, V92-33 (Phase 3); **V92-39, V92-40 (Phase 4 and data mode)**; reused by V92-48, V92-51, V92-53, V92-55 |
| MD plus TRN1u longer than RTD + 4000 ms | the analogue modem must not let it happen: TRN1u is cut | 9.5.2.1.2 | V92-32 |
| An INFO1a layout the Phase 2 kind does not allow | count the frame as not received and recover | 9.4.1.1.5, 8.4.1 | V92-16 (Table 18), V92-24 (Table 11 after short, Table 19 after full) |
| Rate renegotiation detected during FPE | rate renegotiation takes precedence | 9.9.1.1.2, 9.9.2.1.2 | V92-53 |
| Cleardown by drn = 0 | finish the sequence, ignore the constellations, drop | 9.11 | V92-53 |
| MH initiator unanswered | retrain or disconnect | 9.10.1.1 | V92-25, V92-55 |
| An MH initiating sequence before the far RT or an MH response | must not be sent at all | 9.10.1.1 R1 | V92-25 |
| An MHack with a reserved T1 | not a grant: treat as a refusal | 8.9.2, Table 33 (section 4) | V92-02, V92-25 |
| A resumed call after a hold | discard what the pre-hold Phase 1 taught; renegotiate the menus and the bypass | 9.10.2.1 R9 | V92-58 |
| MH refused | MHnack, then MHcda or MHfrr | 9.10.2.1 | V92-25, V92-55 |
| A held modem cleared down by QC with U_QTS 1111, or a zero CM | disconnect | 9.10.2.1 | V92-58 |
| PCM upstream will not train (local policy) | V.34 upstream, then V.34 | AD-11 | V92-42, V92-46 |

## 11. Out of scope, and why

| Item | Clauses | Why |
|---|---|---|
| **The V.8 bis route of short Phase 1**: CRe detection and generation, QC2a, QC2d, QCA2a, QCA2d, and the V.8 bis fallback transactions | 8.2.2, 8.2.4, 8.3.3, 8.3.5, 9.2.1.2, 9.2.2.2, 9.2.3.2, 9.2.4.2 | The repository has no V.8 bis at all. The route needs a QCA within 1 s of QC2x, and on this project's line the round trip alone is 1.1-1.6 s, so it can never succeed there, and stretching the timer would not be conforming (P1P P14). V.92 makes CRe detection optional for callers (9.2.1, 9.2.2) and lets answerers choose ANSam, which is the route this plan builds. No observed server has sent CRe, and a digital answerer that did would cost a caller 3 s; ours never sends it. The QC2 codec could be added later from P1A 6 and CTL 3.2 if a capture ever shows CRe. |
| **Both modems analogue in short Phase 1**, ending in V.34 Phase 2 | 9.2.1.4, 9.2.3.4, Figures 7, 8 | It is a "may" for the answerer and it produces a V.34 call, which the repository already reaches through ordinary V.8. It would cost two extra states in every Phase 1 role for no new capability. If it is ever wanted, it is an S-sized addition to V92-31 plus a TONEq exchange from V92-13. |
| **A digital answerer taking the analogue role on QC1d or QC2d** | 9.2.4.1, 9.2.4.2 | A "may". BinModem's digital modem only ever answers as a server, and V.8's own rule (the caller becomes the analogue modem when both ends could be either; V.90 9.1.1) reaches the same outcome one exchange later. |
| **Sending a manufacturer-defined MD, in either direction** | 8.5.3, Table 17 bits 18:24, V.34 10.1.3.5 | MD is optional and its content is undefined. Figures 10 and 11 show no downstream MD at all in a PCM-upstream Phase 3 (P3S A14). We send length 0 both ways. *Receiving* a nonzero analogue MD length and waiting it out is in scope (V92-33). |
| **Requesting a zero-length DIL (the SCR path)** | 9.5.1.1.11, 9.5.1.1.13, 8.6.6 | The downstream choice needs a DIL and `v90::dil::design` already produces a good one. The SCR path is parsed and answered (V92-37) so a far end may use it, but we never ask for it. |
| **Asking for codec-side constellations in CPt or CPu** (Table 23 bit 128 = 1, and the delta = 2·gamma + 136 doubling that follows) | 8.5.1, 8.7.3 | Our analogue modem's transmit constellations are the codec's own Ucode levels, so bit 128 is always 0 in what we send, and only the analogue modem sends Table 23, so parsing an incoming 1 does not arise. The length arithmetic is still implemented and tested in V92-03, so turning it on later is a one-line change. |
| **Spectral shaping requested by our analogue modem** (Sr > 0 in CPu) | V.90 5.4.5.6 | `dil::choose` never asks for it today and V.92 does not change that. Our digital modem still honours any Sr through the V.90 encoder. |
| **An analogue-side echo canceller** (for a 2-wire line) | 1 b | The rig is 4-wire, softphone to softphone, and V.90 has run without one. The digital modem's canceller (V92-43) *is* in scope, because the simulated server faces hybrid echo before its A/D and that is what makes upstream PCM decisions possible at all. |
| **V.80 synchronous access, and V.43 circuit 133** | 7.2, Table 1 note 2 | Neither Recommendation is in `docs/specs`, so neither can be implemented from the text. V.14 and V.42, both present, meet clause 7.2. The existing DTE flow control stays. |
| **V.59 managed objects behind `+TMO`** | V.250 6.9 | V.59 is not in `docs/specs`. |
| **`+VCID` Caller ID collection for `+PCW=0`** | V.250 6.8.1 | `+VCID` is V.253, which is not in `docs/specs`. `+PCW=0` toggles circuit 125 and logs; the Caller ID half is left out. |
| **A real hook flash for `+PMHF`** | V.250 6.8.6 | The softphone owns the line and there is no DAA. The command answers OK while on hold and logs, and says so in its help text; anything else would be a lie. |
| **Network call-waiting detection (CAS or SAS tones)** | 9.10 in general | Neither V.92 nor V.250 specifies it, it is network-specific, and the softphone takes the waiting call itself, so nothing reaches the modem. A GUI button stands in for the indication so the `+PCW` paths can still be exercised (V92-60). |
| **V.54-style loopback testing** | V.92 clause 10 | Clause 10 says the testing facilities of other V-series Recommendations cannot be used with V.92 and that V.92's own are for further study. There is nothing normative to build. |
| **V.44 changes** | 7.2 | V.92 does not name V.44 and V.44 does not name V.92. The existing XID negotiation is unaffected. |
| **Changing the CI build profile** (`[profile.dev.package.datapump] opt-level`) | none | This is the maintainer's decision (CT 9.6). It is raised as risk R9, not done in a package. |

## 12. Risks

**R1. Upstream PCM may never work on the project's own line.** The evidence is already in the memory
notes: about 1.5 s round trip, 20 ms jitter-buffer concealment inserts every few seconds, a softphone gain
ceiling near 0.32 of full scale, and a transcoding path (Crazytel) that decodes, low-passes and re-encodes.
Upstream PCM needs our samples to reach the far codec unchanged, and every one of those breaks that.
*Mitigation:* everything is proved between our own two modems over a simulated network first (AD-15);
V92-42 builds the fallback ladder at the same time as the first whole start-up, not later; V92-46 asserts
that the transcoding and gain-control cases fall back rather than loop.

**R2. The de-risk spike may say no.** V92-26 could show that a precoder we can design does not reach a
useful upstream rate over a codec channel. *Mitigation:* it is wave 4, before any V.92 state machine
exists, and its failure mode is explicit - the plan continues with V.34 upstream as the product and PCM
upstream as a research branch, and waves 5 to 9 shrink to the Phase 2 and hand-over work.

**R3. Our two ends share one reading of the text.** Call-to-call tests cannot catch a misreading both ends
make: the TRN2u symbol bit order, the CPd point scale and sign, d(f-1), what an absent CPd part means, the
frame origin, the S-bar-u extension, and whether TRN2u and the SUV/CP sequences are precoded. *Mitigation:*
section 4 fixes each as one named constant with both readings documented, and V92-12 tests the bit order at
both settings, so flipping one after a capture is a one-line change. Only a real V.92 capture settles them
(section 13).

**R4. Timers written for short lines, against a 1.1-1.6 s round trip.** TR3 (1500 ms from the start of Ja,
no RTD term) cannot hold above about 900 ms of round trip; short Phase 2's three flat 2500 ms timers cap
the round trip at about 2.3 s; short Phase 1's 2 s windows have no RTD term; the 100 ms + RTD repeat rule
and the 20 s + 6 RTD watchdog are the analogue modem's too, and after a short Phase 2 it has no RTD of its
own; and a retrain that mis-measured RTD makes all of them wrong (the Crazytel note: 0.31 s measured
against 1.1 s real). *Mitigation:* documented departures (section 4), memo RTDs (V92-45), the permitted
early TONEq (V92-49), and tests at 0.6 and 0.75 s each way. Some windows cannot be met live whatever we do.

**R5. Silent misparses between the V.90 and V.92 layouts.** A Jp read as a Jd full of nonsense rates; a
V.92 CPt read through the V.90 CP layout, giving a shifted drn that can still produce a plausible
`Mapping`; a V.92 INFO0a landing in `Info0.clock`; a Table 18 INFO1a dropped; bit 70 read from a V.90
server. A miss gives V.90 silently, or worse, PCM upstream against a V.90 server. *Mitigation:*
cross-version tests in V92-02, V92-03, V92-11 and V92-16; every shared parser takes a version.

**R6. The digital modem's design problem is ours to invent.** The precoder, prefilter, G, constellations
and moduli have no specified method (CD 7.6). A poor design shows up as a low upstream rate rather than a
failure, so it can hide. *Mitigation:* V92-26 measures it against a stated target before anything is built
on top - and with hybrid echo in the sweep, since echo before the quantiser is what the real digital modem
faces; V92-21, V92-62 and V92-30 verify every candidate through the transmitter model before it is sent;
the channel models in V92-17 are what make those numbers mean anything.

**R7. Echo and upstream slips at a real server are invisible to us.** Recovery there is the far server's
business and is unknown. Expect V.42 retransmission bursts or retrains that the simulation does not
predict.

**R8. Slip-sensitive V.90 code.** V92-05 moves the DIL relocation logic that live V.90 calls depend on, and
which has a known 36-symbol alias weakness (17 of 260 sweep positions fail). *Mitigation:* it is a pure
move committed on its own, with the sweep counted before and after. Improving the weakness is a separate
task; V.92 inherits it unchanged, now sitting after Jp-prime.

**R9. CI runtime.** CI runs the dev profile. V.92 roughly doubles the both-ends matrix and adds a precoder
design of up to 384 coefficients and a 4D Viterbi. *Mitigation:* V92-04 tabulates the network kernel;
every start-up test stops at its connection; sweeps are `#[ignore]`d; V92-41 carries an explicit wall-clock
budget of about 1.5 times V.90 per simulated second. If the dev profile becomes the bottleneck, propose
`opt-level = 2` for the `datapump` package to the maintainer rather than trimming tests.

**R10. State-machine size.** `v92/analogue.rs` and `v92/digital.rs` gain Phase 3, Phase 4, rate
renegotiation, FPE, cleardown and hold over ten waves. Named deadline slots and the shared `Exchange` keep
them reviewable, and Phase 3 is split at Jd, but V92-39, V92-40, V92-53, V92-55 and V92-58 are still L, and
each names its seam in case it needs two sittings.

**R11. Coupling in the modem crate and the GUI.** The exhaustive matches on `Pump`,
`v90::startup::Status`, `v8line::Status` and `at::Action`, and the index-coupled `CARRIERS`, `rates` and
`CEILINGS`. AD-4 and AD-5 limit the damage, and V92-54 changes `at::Action` and both of its matches in one
package, but the late waves still serialise on `modem/src/lib.rs` and `gui/src/app.rs`.

**R12. Clippy with `-D warnings` on scaffolding.** V92-01's stub modules must contain no unused private
items, and every new public type needs `Debug`. No package may land a module that nothing references.

**R13. Modem-on-hold and a retrain start identically.** The existing tone watches fire first, and an MH
sequence opens with four ONEs, which in DPSK is four phase reversals in a row. *Mitigation:* V92-25 and
V92-55 run the INFO framer beside the reversal detector and give a validated MH frame priority, with tests
that a genuine retrain still works and that a hold request is not read as one.

**R14. Round-trip-delay and timing claims in several digests come from project memory**, not from the
Recommendation. They are planning inputs, not requirements, and every one of them is written as a named
constant with its evidence.

## 13. Live-test requests, batched for Rory

Live testing is Rory's job, so these are gathered into one request. None blocks the plan before wave 9;
the first four are best made once V92-47 is merged, which is the first build that can be pointed at a real
server.

1. **A transparency check in both directions, both ends ours**, with the second SIP leg the test-loop
   memory says is missing. Does what we send arrive at the far host sample for sample, at one phase, with
   no gain change? That one answer decides whether upstream PCM is possible on this rig at all (R1, and
   V92-26's assumptions).
2. **A capture of a real V.92 server call with `+MS=V92`, both channels.** It settles INFO1d bit 70 (the
   server's own verdict on our path), the INFO1a layout, whether short Phase 2 is granted, Jp's epsilon,
   and - if a CPd arrives - the constellation point scale, what an absent part means and the 128-point
   reading (R3).
3. **One captured TRN2u and the SUVu that follows it.** It settles the symbol bit order (R3), because a
   wrong order means the server never answers our SUVu with a CPd.
4. **`AT+PMHR` against the NetZero or GlobalPOPs pool, with a recording running**, to see whether V.92 hold
   is granted at all; those pools are the only live V.92 far ends available.
5. **A second call with the memo to the same server**, to see whether quick connect and short Phase 2 are
   honoured live.
6. **MicroSIP with AEC, AGC, noise suppression, VAD and comfort noise all off**, confirmed on a capture,
   and PCMU only if possible. A softphone AEC on the capture path would subtract a filtered copy of the
   downstream from our upstream, which destroys PCM upstream outright.

## 14. Review notes

The review of the first complete plan is resolved above. Every coverage gap, every ordering problem and
every file conflict it raised has been amended in place, ids kept stable and one id appended (V92-62).
Six points were resolved differently from the way the review suggested, or judged wrong; each is set out
here with its reason.

**1. The V92-26/V92-30 ordering break was fixed by splitting V92-30, not by either option offered.** The
review was right that this is the one hard break and that it sits on the M0 gate: four of V92-26's tests
assert on the drn, the moduli, the per-j sets and G, and the plan put all of that in V92-30, in V92-26's
own wave. Neither offered fix works as stated. Moving V92-30 into wave 3 is impossible, because V92-30
depends on V92-19, V92-20 and V92-21, which *are* wave 3. Moving the choice into V92-21 makes V92-21 L and
still needs `UpEncoder` (V92-19, same wave) for G. So the numeric choice became **V92-62** in wave 3,
depending only on V92-01, V92-09 and V92-10: it returns `Parameters`, takes the filters as an input, and
computes G by simulating the V92-10 chain rather than `UpEncoder`. V92-30 keeps its file and its wave and
shrinks to CPd assembly, the Ja mask, the Table 18 limits, the end-to-end verification and `cleardown()`;
it is now S. M0 stays in wave 4, before any state machine, and the wave count stays at 18.

**2. V92-05 takes the closure, not the dependency.** The review offered either declaring V92-05 dependent
on V92-03 and sequencing wave 1, or making the acceptance test a caller-supplied closure. The closure was
taken, because the same change also settles the digest gap the review raised separately (V92-05 citing no
digest that carries the Jd/Jp bit layout): with a closure over the raw framed bits, V92-05 needs no bit-47
knowledge and therefore no P3S citation, and stays the pure behaviour-preserving move that section 9.4 and
risk R8 promise. Wave 1 is genuinely parallel again. V92-05 also now carries the `pub(crate) mod carrier;`
change the review asked for.

**3. V92-45 gains constructors; `Settings::new` is untouched.** The review offered either adding
`without_probe`/`from_info1a` or adding `crates/datapump/tests/v90_call.rs` to V92-45's file list and
amending section 9.1. The first was taken: section 9.1's promise that every existing test passes unedited
is the plan's main defence of V.90, and it is worth more than the tidiness of one signature.

**4. V92-22's "Table 1 note 1" is Table 1/V.90, the Ucode table.** The review was right that the citation
was ambiguous. It is resolved in favour of the Ucode table, because that is the table a PCM receiver needs
and because Table 1/V.92 is the interchange-circuit list, which has nothing in it for a receiver. INTRO
(3.6) and P1D (3) were added, and the clause line now says which table is *not* meant.

**5. V.250 5.4.2 was dropped from V92-54 rather than rendered.** The `+P` test replies are checked against
the `+DS44` pattern already in `crates/at/src/lib.rs`, which CV 6.4 documents and which the digests
actually cover. Citing a clause no digest carries would be worse than citing none.

**6. V92-53 and V92-55 were *not* swapped in the wave order.** The review's point that V92-55's dependency
on V92-53 is artificial is accepted, and that dependency has been removed: V92-55 now depends on V92-25 and
V92-41, and wave 15 follows wave 14 only as a merge gate on `v92/analogue.rs` and `v92/digital.rs`. The
swap itself is declined, because it does not do what it was proposed for. What the live NetZero/GlobalPOPs
test in section 13 needs is `AT+PMHR` reaching a real far end, and that is V92-55 → V92-56 → V92-58 →
V92-59, not V92-55 alone. Swapping V92-53 and V92-55 moves V92-55 one wave earlier but pushes V92-56 (which
really does need V92-53's cleardown, and which shares `modem/src/lib.rs` with V92-58) from wave 15 to 16,
V92-58 from 16 to 17 and V92-59 from 17 to 18, and adds a nineteenth wave. Hold call control would land
**later**, not earlier. If the maintainer wants hold sooner regardless, the cheapest real lever is to cut
V92-53's cleardown half - the seam the package already names - into its own package, which would free
V92-56 and let hold run a wave ahead of FPE; that is a change to make deliberately, not a reordering.

Two further judgements worth recording, both accepted from the review but resolved with a choice:

- **Reserved codes are treated as "unknown", never as a licence.** A reserved T1 in an MHack is no grant
  (section 4), so a far end cannot obtain an unbounded hold by sending `1111`. Reserved *bits* in a
  received sequence go the other way and are ignored rather than rejected (V92-03, V92-11), because there
  the risk is refusing a conforming far end that uses a future extension. The two rules point in opposite
  directions on purpose: refuse to act on what we cannot interpret, but do not refuse to talk to it.
- **The digital modem's 104, 106 and 109 are modelled** (V92-40), rather than recorded as out of scope.
  The analogue mirror was already in V92-39, the existing V.90 modems already carry the data-holdback
  semantics, and a Phase 4 in which only one side's circuits exist would make the FPE circuit tests of
  V92-53 untestable on the digital side.

**V92-01, after implementation review.** Three lines of its entry were corrected against what the package
had to be. The sizing was wrong: `v92/mod.rs` came to 979 lines, and 1165 once the review's findings were
answered, a third of it the clause quotations the house rule asks for. So the package is **L** and not
**S**; the description lists thirteen groups of content and seven tests and was never an S. The `Deadlines` slots are keyed by `&'static str` rather than
by an enum variant, because a package that has to arm a new timer cannot add a variant without editing
`mod.rs`, which its own file list forbids - the entry now says so, and says what it costs. And
the named test's `up_bits(drn) * 8000 == up_rate(drn) * 12` is false at twelve of the nineteen rungs,
because `up_rate` floors 8000/6 exactly as `v90::rate_for` does; the entry now asks for the floor-
consistent identity everywhere and the exact one at the seven rungs where it holds, so that a later reader
does not restore a broken assertion.
