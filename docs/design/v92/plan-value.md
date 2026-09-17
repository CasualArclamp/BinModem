# V.92 implementation plan: working calls early

A plan for building ITU-T V.92 (11/2000) in BinModem, ordered so that **a complete V.92
call between our own two modems, with PCM upstream, happens as early as the dependencies
allow**, and everything after that deepens a call that already works.

Both ends of every feature are ours. The analogue modem and the digital modem are built in
the same work package pair, so every feature can be proved call to call over
`v90::network::Network` without waiting for a live server.

Companion documents, all in `docs/design/v92/`:

| Digest | What it settles |
|---|---|
| `spec-intro-transmitter.md` | clauses 1-7: the upstream transmitter chain, Qa.b, CPd (Table 30), the bit-order rule |
| `spec-phase1-signals-analogue.md`, `spec-phase1-signals-digital.md` | 8.1-8.3: QC/QCA, TONEq, QTS, ANSpcm with the verified tables |
| `spec-phase1-procedures.md` | 9.1, 9.2: full and short Phase 1, the fallback web |
| `spec-phase2-signals.md`, `spec-phase2-procedures.md` | 8.4, 9.3, 9.4: INFO Tables 15-19, full and short Phase 2 |
| `spec-phase3-signals.md`, `spec-phase3-procedures.md` | 8.5, 8.6, 9.5: Ru, TRN1u, Ja, Su, CPt, E1u, Jd, Jp, Jp', DIL, SCR, Ri |
| `spec-phase4-signals-analogue.md`, `spec-phase4-signals-digital.md`, `spec-phase4-procedures.md` | 8.7, 8.8, 9.6, 9.7: B1u, E2u, CPu, CPus, SUVu, TRN2u, FB1u, CPd, SUVd, Ed, B1d, Rd/Rt/Rf, retrains |
| `spec-renegotiation-fpe.md` | 9.8, 9.9, 9.11: rate renegotiation, fast parameter exchange, cleardown |
| `spec-modem-on-hold.md` | 8.9, 9.10, 10: RT, MH sequences, hold, cleardown from hold |
| `spec-control-plane.md` | V.8, V.8 bis, V.250 `+P`, V.42/V.44 |
| `code-analogue.md`, `code-digital.md`, `code-v8-and-call.md`, `code-tests.md` | what the repository does today and where V.92 plugs in |

A sibling plan, `plan-foundations.md`, orders the same work foundations first. This plan
differs in what it builds **before** the first call: only the chain, the codecs, the
receiver and the two Phase 3/4 state machines. Short Phase 1, short Phase 2,
renegotiation, fast parameter exchange, modem-on-hold, the AT commands and the GUI are all
deliberately after the call works.

---

## 1. Overview

### 1.1 What V.92 adds

Downstream is V.90 unchanged (clause 5 of V.92 is clause 5 of V.90). Everything new is
upstream, plus four procedures:

1. **PCM upstream** (clause 6): 8000 symbols/s, 24 000 to 48 000 bit/s, 12-symbol data
   frames, a 12-modulus encoder with a differential sign step, an IIR precoder, an ARMA
   prefilter and a 4D trellis code. The **digital modem designs the analogue modem's
   transmitter** and downloads it in CPd (Table 30).
2. **A new Phase 3 and Phase 4** (8.5-8.8, 9.5, 9.6): Ru, TRN1u, Ja, Su, Jp with a
   sampling-phase correction, CPt, E1u; then TRN2u, an SUV/CP/E handshake, B1u.
3. **Quick connect**: short Phase 1 (9.2) and short Phase 2 (9.4).
4. **Modem-on-hold** (8.9, 9.10), **fast parameter exchange** (9.9) and a V.92 flavour of
   rate renegotiation (9.8) and cleardown (9.11).

### 1.2 The order, and why

The critical path to a working call is short and almost entirely new code:

```
WP-01 numbers ─┬─ WP-02 modulus ─┐
               ├─ WP-03 precoder ─┴─ WP-10 upstream chain ─┬─ WP-15 de-risk spike
               │                  ├─ WP-11 decoder ────────┤
               │                  └─ WP-12 CPd design ─────┘
               ├─ WP-13 upstream signals ─┬─ WP-16 upstream receiver ─ WP-18 epsilon
               └─ WP-14 transmitter ──────┘
WP-09 sequences ─ WP-19 exchange
WP-06 downstream extraction
                    ↓
        WP-20/21 Phase 3 to Jd  →  WP-22/23 Phase 3 to E1u  →  WP-24 Phase 3 call
                    ↓
        WP-26/27 Phase 4 and data  →  WP-29 the first complete V.92 call
                    ↓
        WP-28 Phase 2 flags  →  WP-30 hand-over, interop and the fallback ladder
```

Nothing is built before WP-29 that the first call does not need. Short Phase 1 needs
ANSpcm tables, QTS and a V.8 rework; short Phase 2 needs a new tone-reversal machine;
modem-on-hold needs a DPSK transaction machine. None of those makes the first call arrive
sooner, and all of them are easier to test once a call exists to interrupt.

### 1.3 Milestones

| Milestone | Wave | What works |
|---|---|---|
| **M0 de-risked** | 3-4 | WP-15 proves the riskiest DSP in isolation: the designed precoder and prefilter, the trellis-coded modulus chain and the digital modem's decoder carry 48 000 bit/s over a codec-like channel. WP-18 adds the real receiver and the sampling-phase loop. |
| **M1 first call** | 9 | WP-29: two of our modems complete Phase 3 and Phase 4 over `Network` and carry data both ways, PCM upstream and downstream. |
| **M2 whole start-up** | 9 | WP-30: V.8, full Phase 2, V.92 Phase 3 and 4, data, from `ATD` in the test harness; V.90 and V.34 fallbacks proved. |
| **M3 robust** | 10-11 | Echo, slips, pads, robbed bits, transcoding, long round trips, softphone paths. |
| **M4 quick connect** | 10-12 | Short Phase 1 and short Phase 2, and the recognised-connection memo. |
| **M5 renegotiation** | 11-13 | Rate renegotiation, fast parameter exchange, cleardown. |
| **M6 hold** | 14-15 | Modem-on-hold, both ends, and its call control. |
| **M7 the product** | 12-16 | `+MS=V92`, the `+P` commands, the GUI carrier and controls, the upstream PCM scope. |

### 1.4 The riskiest DSP, and how it is de-risked

Two pieces have no precedent in the repository and no reference implementation we are
allowed to look at:

- **The digital modem's PCM upstream receiver.** It sees one sample per symbol (the
  central-office A/D output), so no fractionally spaced equaliser is possible. It must
  train on TRN1u, measure the sampling phase on Su, cancel hybrid echo, and run a 4D
  Viterbi over PCM levels.
- **The precoder and prefilter the digital modem computes for the analogue modem.** The
  structure is fixed by 6.4.2 but the design method is entirely ours, and a bad design is
  invisible until the far end fails to decode.

**WP-15 is a standalone test, landed in wave 3, before any V.92 state machine exists.** It
wires the transmit chain (WP-10), the design (WP-12) and the decoder (WP-11) across a
synthetic channel that has the hard parts of a codec path: a DC null, a 4 kHz roll-off,
additive noise and a G.711 quantiser. It sweeps rate, gain and noise and reports the
achievable upstream rate. If that test cannot reach the upper half of the upstream ladder,
the plan stops there and the fallback ladder (V.34 upstream, WP-30) becomes the product.
WP-18 then repeats it through the real receiver with the sampling phase swept.

---

## 2. Architecture decisions

**A1. V.92 lives in a new `crates/datapump/src/v92/` module.** V.90 is not restructured.
It stays as the downstream encoder, the DIL design, the downstream receiver, and as the
V.34-upstream fallback that V.92 itself requires. `v92` depends on `v90`, never the
reverse. Rationale: `v90/digital.rs` and `v90/analogue.rs` are already 1000-1900 lines of
flag-heavy state machine that run on live calls; folding PCM upstream into them would
double their state and put the live path at risk (`code-digital.md` 8.1,
`code-analogue.md` 13).

**A2. One `Parameters` struct is the boundary between the wire and the chain.**
`v92::Parameters { drn, trellis, extend_e2u, gain, moduli, filters, sets, indices }`
(defined in `v92/mod.rs`) is what the transmitter, the decoder and the design all speak.
`Cpd` (Table 30) converts to and from it. So the chain can be built and tested before the
wire format exists, and the design can produce something the transmitter runs without
either knowing about bit layouts.

**A3. One transmitter model, used by both modems.** `v92::precoder` and `v92::upstream`
are the analogue modem's transmitter *and* the digital modem's model of it. The digital
modem designs against the same code it will be decoding, and verifies a candidate CPd by
running it before sending it (`code-digital.md` 7.6 step 6, 7.8).

**A4. The SUV/CP/E handshake is one reusable machine.** `v92::exchange::Exchange`
implements 9.6.x.1.2 to 9.6.x.1.4 once, with a context of `Training`,
`RateRenegotiation` or `FastExchange` that decides the modulation, whether CPd bit 29 may
be set, and whether FB1u precedes B1u. 9.8 and 9.9 both enter it (9.8.1.1.2, 9.8.2.1.3,
9.9.1.1.2, 9.9.2.1.2), so building it once is the difference between renegotiation being
a week and a day.

**A5. The two modems stay in separate files.** `v92/analogue.rs` and `v92/digital.rs`.
This is what lets Phase 3 and Phase 4 for the two sides be separate work packages in the
same wave. Both follow the house shape: `step(line) -> f64`, a pull-model symbol source
(`tx.next_sample(|| source.next())`), a `Stage` enum, `Status { Running, Connected{..},
ClearedDown, Failed(&'static str) }`, and `phase() -> &'static str`.

**A6. Every unresolved reading becomes one named constant with a test at both settings.**
The Recommendation leaves several things open (section 3). Each is a `const` in
`v92/mod.rs` with a doc comment naming the clause, the two readings and the evidence that
would settle it. Both ends are ours, so a wrong choice costs nothing until we meet a real
V.92 server; a *hidden* choice would cost a week of debugging then.

**A7. Named deadline slots, not one `deadline` field.** `v90/analogue.rs` overwrites a
single `Option<(u64, &'static str)>` in five places. V.92 has five overlapping timers in
Phase 3 alone (TR3, TR4, the Su-by and CPt-by windows, the 20 s + 6 RTD start-up
watchdog). `v92::Deadlines` holds them by name and checks them together.

**A8. The fallback ladder is explicit and early.** V.92 PCM upstream → V.92 with V.34
upstream (INFO1a Table 10 in full Phase 2, Table 19 after short Phase 2) → V.34. WP-30
builds the ladder and its tests at the same time as the first whole start-up, because on
the project's own VoIP rig the top rung may never hold (section 6, R1).

**A9. Test layering.** Cheapest first, each layer reusing the V.90 harness shape:

1. in-module unit tests;
2. `tests/v92_upstream.rs`, the synthetic-channel spike (WP-15, WP-18);
3. `tests/v92_call.rs`, `Call`: Phases 3 and 4 between our two modems over `Network`,
   started from a `settled_v92()` that stands in for Phase 2;
4. `tests/v92_startup.rs`, `FullCall`: V.8, Phase 2, Phase 3, Phase 4, data;
5. `crates/modem/tests/v92_call.rs`: the whole modem over AT, through the softphone model;
6. `tests/v92_vector.rs` and an ignored `v92_replay.rs` against the captures;
7. live, by Rory.

**A10. V.90 and V.34 keep working at every step.** The only permitted changes to V.90 code
are additive (new fields defaulting to today's behaviour) or behaviour-preserving (WP-06).
No existing test is edited; new tests are added. Every work package that touches a V.90
file states "the existing V.90 and V.34 tests pass unchanged" as an acceptance condition.

**A11. Clock and level discipline.** The upstream symbol clock is slaved to the recovered
downstream clock (6.2) through a new `pcm::Receiver::symbol_clock()` that reports the
smoothed drift, never the timing loop's phase. The transmitter carries a fractional delay
so that 24.5T and (24+ε)T are a timing shift of everything after them, not extra symbols.
LU, TRN1u and the prefilter output G·v(n) are all bounded by the same 0.3 of full scale
that `dil::LOUDEST` uses downstream, because the softphone capture path may have its own
limiter.

---

## 3. Readings fixed now

Each of these is an open question in the digests. They are fixed here so that work
packages do not each invent an answer, and each becomes a named constant (A6).

| Item | Decision | Source and why |
|---|---|---|
| Modulus encoder step 4 | `R0 = R` when `d(f-1) = 0`, else `M-1-R`, exactly as printed | `spec-intro-transmitter.md` Q-1; confirmed at 300 dpi. Both readings decode; ours is the printed one |
| `d(-1)` | 0 | 8.7.1 zeroes the modulus encoder memories before B1u |
| TRN2u symbol bit order | **LSB first in time**, sign bit last; `const TRN2U_SIGN_LAST: bool = true` | `spec-phase4-signals-analogue.md` A2: matches V.34 10.1.3.8 and clause 8's "integers LSB first". Tested at both settings |
| Only the sign bit of TRN2u is differentially encoded | yes | 8.7.6 names "the sign bit" |
| CPd constellation point "linear value" | unsigned, on the V.90 Table 1 linear scale | `spec-phase4-signals-digital.md` Q1. Both ends ours; only a real CPd capture settles interop |
| Unsigned Qa.b | from the printed digit pattern: Q0.16 is raw/65536, Q3.13 is raw/8192, so `G = raw/262144` | 3.5's printed range contradicts the field width (`spec-intro-transmitter.md` P-10) |
| ε | `code / 65536` symbols, LSB at Jp bit 18 | `spec-phase3-signals.md` A3 |
| Upstream frame origin | the first symbol of the **second** TRN1u, carried modulo 12 thereafter | 8.5.7 against 9.5.1.1.10 (`code-digital.md` 7.4); the two agree if the count is carried |
| "128 points" | transmit `2·LC <= 128`; accept `LC <= 128` | `spec-phase4-signals-digital.md` Q3 |
| Precoder point choice | minimise `abs(x(n))` symbol by symbol; ties to the smaller `abs(eta)` | the 8.8.3 NOTE tells the digital modem to assume exactly this |
| TRN2u, SUVu, CPu, E2u in training and renegotiation | bypass the precoder and prefilter, like Ru (8.5.5) | `spec-phase4-procedures.md` Q3; in training no precoder exists yet |
| Absent CPd part | keep the previous value; all three parts are required in the first CPd of a training | `spec-phase4-signals-digital.md` Q2 |
| Cleardown field | `drn = 0` in CPu/CPus bits 21:25 and CPd bits 22:26, not in SUV | 9.11's "SUVu/SUVd" is an editorial slip (E-1) |
| MD | analogue MD length 0; no downstream MD (INFO1d 18:24 = 0) | 8.5.3; nothing in 9.5 sends a downstream MD |
| DIL | always request N != 0 and reuse `v90::dil::design`; parse the N = 0 / SCR path but never ask for it | the downstream side already works this way |
| Digital modem silence | Ucode 0 codewords, frame alignment kept | 9.8.1.1.3, applied to every silence |
| Default U_QTS | `0101`, Ucode 70 | `spec-phase1-procedures.md` Q8 |
| Default ANSpcm level | `01`, -12 dBm0 | quieter than the softphone limiter seen at about 0.32 full scale |
| Tone A guard level | -7 dB, as the existing `dpsk.rs` does | V.34 10.1.2.1 prints "nominal", which interoperates badly with what already works (`spec-phase2-signals.md` N-14) |
| TR3 (1500 ms, no RTD) | relaxed to 2.0 s + 2·RTD, as `SD_WAIT` already is, and documented as a departure | it cannot hold above about 900 ms of round trip (`spec-phase3-procedures.md` 13.2 item 7) |

---

## 4. Work packages

Every work package ends with `cargo test --workspace` green and
`cargo clippy --workspace --all-targets -- -D warnings` clean. Where a package touches a
V.90 or V.34 file, "the existing tests pass unchanged" is part of its definition of done.
Sizes: S is a few hundred lines, M is one sitting, L is a long sitting.

Paths are relative to the repository root.

---

### Wave 1: the numbers, the codecs' foundations, and the V.90 openings

#### WP-01: The `v92` module, its shared numbers and `Parameters` (S)

**Clauses:** 1 (rates), 3.5 (Qa.b), 3.6 (Ucode), 3.8 (LU), 5, 6.1, 6.2, clause 8 preamble
(bit order).

**Creates:** `crates/datapump/src/v92/mod.rs`.
**Edits:** `crates/datapump/src/lib.rs` (one `pub mod v92;`).

**Does:** the constants and small shared types every later package uses.
`UP_INTERVALS = 12`, `CONSTELLATION_FRAME = 6`, `TRELLIS_FRAME = 4`; the upstream ladder
`up_rate(drn) = (drn + 17)*8000/6` for drn 1..=19 and `up_bits(drn) = 2*(drn + 17)`;
`MD_SYMBOLS = 276`; `FILTER_TOTALS = [192, 256, 320, 384]` and
`FILTER_EACH = [128, 192, 256, 320]`; the segment lengths (Ru 384T, R-bar-u 24T, Su 144T,
TRN1u minimum 2040T, B1u 48 frames, E2u 1 frame, Rf 384T with a 4-symbol sign period);
`START_UP = 20 s + 6*RTD`. Signed and unsigned `Qa.b` helpers with the section 3 reading.
`Parameters` (A2) and `Deadlines` (A7). Every reading fixed in section 3 that is a value
becomes a `const` here, with a doc comment naming the clause and, where the reading is
open, the alternative.

**Tests:**
- `the_upstream_ladder_runs_from_24000_to_48000_in_steps_of_8000_over_6` - `up_rate(1)` is
  24 000, `up_rate(19)` is 48 000, 19 rungs, each 8000/6 apart.
- `a_data_frame_carries_twice_drn_plus_seventeen_bits` - `up_bits(drn) * 8000 == up_rate(drn) * 12`
  for every drn.
- `the_q_formats_read_as_the_printed_digit_patterns` - 4G = 0x4000 gives G = 1/16; signed
  Q1.6 0x80 is -2.0; unsigned Q3.13 0x2000 is 1.0.
- `a_twelve_symbol_frame_holds_two_constellation_frames_and_three_trellis_frames`.

**Digests:** `spec-intro-transmitter.md`, `code-analogue.md` (10, 11.1), `code-digital.md` (8.1, 10).

---

#### WP-02: The 12-interval modulus encoder and decoder (S)

**Clauses:** 6.4.1.

**Creates:** `crates/datapump/src/v92/modulus.rs`.

**Does:** `encode(bits, &[u8; 12]) -> [u8; 12]` and the matching decoder, in `u128`
(M reaches 255^12, about 2^96). The five steps of 6.4.1 including the sign
`s = (2R > M-1)`, the differential `d(f) = s(f) ^ d(f-1)` and step 4's `d(f-1)` (section
3). The decoder keeps `d` from `R`, not from `R0`'s half, because those differ when M is
odd and R = (M-1)/2.

**Tests:**
- `every_frame_round_trips_at_every_rate` - random R for K = 36..=72, even and odd M,
  including R = (M-1)/2, R = 0 and R = M-1.
- `an_inverted_line_still_decodes` - mapping every Ki to Mi-1-Ki and starting the decoder's
  d inverted recovers the same bits, which is the reason the differential step exists.
- `the_encoder_refuses_a_rate_the_moduli_cannot_carry` - `2^K <= product(M)` is checked.
- `the_memory_starts_at_zero` - the first frame after a reset matches a hand-worked value.

**Digests:** `spec-intro-transmitter.md` (6.4.1, P-1, P-11, 9.5).

---

#### WP-03: The precoder, prefilter, inverse map and 4T trellis (M)

**Clauses:** 6.4.2, 6.4.3, 6.4.4.

**Creates:** `crates/datapump/src/v92/precoder.rs`.

**Does:** the clause 6.4 chain below the modulus encoder, as one `Chain` that both modems
use (A3).

- Constellations: `a(eta) = P[eta]` for `eta >= 0` and `-P[-eta-1]` for `eta < 0`,
  `N = 2*LC`, indices in level order.
- Equivalence classes: `eta = Ki + z*Mi` for k = 0, 1, 2 and
  `eta = 2*Ki + 2*z*Mi + ((eta0+eta1+eta2+Y0) mod 2)` for k = 3, with a non-negative mod.
- Point selection: minimise `abs(x(n))` (section 3), by binary search around `-c(n)`.
- Precoder `x(n) = u(n) + sum z1(k)*u(n-k) + sum p1(k)*x(n-k)`, prefilter
  `v(n) = sum_{k=0..LZ2-1} z2(k)*x(n-k) + sum p2(k)*v(n-k)`, output `G*v(n)`, all in f64,
  never saturating the state.
- Inverse map through `v34::trellis::FIGURE_9` and `TABLE_13`, fed points `(2*eta+1, 2*eta'+1)`.
- `v34::trellis::Code` clocked once per trellis frame (4 symbols), 16/32/64 state chosen by
  `Parameters::trellis`.

**Tests:**
- `with_no_filters_the_output_is_the_chosen_level` - z1 = p1 = [], z2 = [1], p2 = [],
  G = 1: the output equals `a(eta)`.
- `the_four_indices_of_a_trellis_frame_have_the_parity_the_encoder_asked_for` -
  `(eta0+eta1+eta2+eta3) mod 2 == Y0` over 5000 random frames.
- `a_class_is_never_empty_when_n_is_big_enough` - for `N >= Mi` (k < 3) and `N >= 2*Mi`
  (k = 3) every Ki has a member in range; below that the chain reports the parameters as
  unusable rather than panicking.
- `the_precoder_output_stays_bounded_over_a_long_run` - with a feedback section from a real
  design, `max abs(x)` over 100 000 symbols is under a stated multiple of the top level.
- `the_prefilter_feed_forward_starts_at_zero_and_the_precoder_s_at_one` - a one-tap
  difference between z1 and z2 indexing is caught (pitfall P-5).
- `the_memories_are_zero_before_b1u` - the first 12 outputs after a reset are a fixed vector.

**Digests:** `spec-intro-transmitter.md` (6.4.2-6.4.4, P-2..P-5, P-12), `code-digital.md` (7.8).

---

#### WP-04: V.90 framing opened up, the Jd/Jp guard and a tolerant finder (S)

**Clauses:** 8.6.2 (Jd bit 47), 10.1.2.3.2/V.34, the framing shared by every V.92 framed
sequence.

**Edits:** `crates/datapump/src/v90/sequences.rs`.

**Does:** three small changes so `v92::sequences` can build on framing already proved
against live servers.

1. Make `frame`, `unframe`, `put` and `get` `pub(crate)`.
2. `Jd::from_bits` refuses a frame with bit 47 set. Today it would read a V.92 Jp as a Jd
   full of nonsense rates with `sixteen_in_training = true`.
3. `Finder` accepts a zero after **at least** 17 ones, not exactly 17, and takes the last
   17 as the sync. Today the first Ja descriptor and the first CPt after their 24-one
   preambles are dropped, costing a repetition on every group.

**Tests:**
- `a_jd_with_bit_47_set_is_not_a_jd` - the Jp vector from `spec-phase3-signals.md` 5.3 is
  rejected.
- `a_frame_after_a_long_run_of_ones_is_still_found` - 41 ones, then a sync, then a valid
  CRC is found; today's exactly-17 rule is the regression this pins.
- The existing `sequences.rs` tests, including the real-server Jd with CRC 0x776E, pass
  unchanged.

**Digests:** `spec-phase3-signals.md` (3.4, 5.2, 9.2), `code-digital.md` (4.2, 4.5).

---

#### WP-05: The V.92 INFO layouts and the MH frame (M)

**Clauses:** 8.4, 8.4.1 (Tables 15, 16, 17, 18, 19), 8.9.2 (Tables 32, 33).

**Edits:** `crates/datapump/src/v34/info.rs`, `crates/datapump/src/v34/dpsk.rs`.

**Does:** the bits Phase 2 and modem-on-hold need, without changing V.90 behaviour.

- `Info0d` gains `v92` (bit 27) and `short_phase2` (bit 26). Today `to_bits` forces both to
  zero and `from_bits` drops them.
- INFO0a gains a V.92 view with the bits **the other way round** (bit 26 = V.92, bit 27 =
  short Phase 2), kept off `Info0.clock`, which is V.34's transmit-clock field.
- `Info1c` gains `pcm_upstream` for bit 70, meaningful only when both ends are V.92, and
  reads the 3429 result as 8 bits (71:74 pre-emphasis, 75:78 rate).
- New `Info1aPcmUp` for Table 18: bits 34:36 = 6 and 37:39 = 6, the filter fields at 12:17,
  the MD length in 276-symbol units, bits 40:49 all ones. Today `Info1aPcm::from_bits`
  rejects exactly this frame.
- `Info1aPcm` gains `high_carrier` (Table 19 bit 33).
- `dpsk.rs`: dispatch on (34:36, 37:39) per `spec-phase2-signals.md` 10.4, and add the
  40-bit MH length to both sides' accepted lengths with an `Mh` variant.
- `Mh { indication, information }` on Table 32 with the Table 33 T1 codes, its four-bit
  fields sent leftmost first.

**Tests:**
- `a_v92_info0d_says_so_in_bit_27_and_asks_for_short_phase_2_in_bit_26` and
  `a_v92_info0a_has_the_two_bits_the_other_way_round` - against the vectors in
  `spec-phase2-signals.md` 15 (CRC 0xDB49 and 0xAF5A).
- `a_v90_peer_reads_a_v92_info0_as_v90` - a V.90 decoder is unaffected.
- `a_table_18_info1a_round_trips_and_is_not_thrown_away` - CRC 0xA858; bits 40:49 are ones
  and are never read as a frequency offset.
- `a_table_19_info1a_keeps_the_carrier_bit` - CRC 0x7A52.
- `info1d_bit_70_is_the_pcm_upstream_flag_only_between_v92_modems`.
- `every_mh_sequence_round_trips` - the ten vectors in `spec-modem-on-hold.md` 1.2.5, each
  leaving CRC residue 0.
- `v90_vector.rs` passes unchanged.

**Digests:** `spec-phase2-signals.md` (5-10, 15, 16.2), `spec-modem-on-hold.md` (1.2),
`code-digital.md` (6).

---

#### WP-06: The shared downstream reading moves to `v90/downstream.rs` (M)

**Clauses:** none new. This is behaviour-preserving.

**Creates:** `crates/datapump/src/v90/downstream.rs`.
**Edits:** `crates/datapump/src/v90/analogue.rs`.

**Does:** moves the parts of the V.90 analogue modem that read the downstream and have
nothing to do with V.34 upstream, so the V.92 analogue modem uses them rather than copying
them: `Levels`, `levels_for`, `least_gap`, `nearest`, `slicer_for`; `Trust` and
`trusted_symbols`; `find_place`; and a `DilReader` owning `dil_*`, `before_dil` and
`analysis`, with `begin_dil`, `find_dil_start`, `fit_dil`, `dil_levels`, `dil_symbol`,
`count_dil` and `find_dil`. Each takes `&mut pcm::Receiver` instead of reaching into
`self.rx`. `RWatch` and `RSeen` move too, generalised to take the pattern period, 6 for
R/Ri/Rd/Rt and 4 for Rf, so V.92 needs no second copy.

This is the one package that touches slip-sensitive code live calls depend on. It is a
pure move, no behaviour may change, and it is committed on its own.

**Tests:** the acceptance condition is that every existing test passes untouched, in
particular the slip tests in `v90_call.rs` (`a_slip_moves_everything_after_it_by_160_codewords`,
the DIL relocation tests, and the sweep harness the `v34-phase2-tone-deadline` memory
describes), plus one new test:
- `an_r_watch_finds_a_four_symbol_pattern_too` - the period generalisation, on a synthetic
  `++--` sequence in both polarities.

**Digests:** `code-analogue.md` (6, 8, 11.1), `code-tests.md` (4.2).

---

#### WP-07: Network, part A: the A/D sampling phase and per-direction impairments (M)

**Clauses:** none. Test infrastructure.

**Edits:** `crates/datapump/src/v90/network.rs`.

**Does:** the upstream model changes the de-risk spike and the first call need.

- `with_upstream_phase(fraction_of_t)`: a fractional offset on the A/D sampling instant,
  and a `lag` that can be fractional. Today, at fs = 16 000 with no skew, every A/D instant
  lands on an analogue sample, so epsilon would always be zero and Su and Jp would never be
  exercised.
- `with_delays(down, up)` beside `with_delay`.
- `with_upstream_noise(level)` beside the single noise level.
- `up_codeword() -> (u8, bool)` beside `up()`, so upstream tests are exact in Ucodes.
- A configurable upstream anti-alias cutoff and `up_gain`.

**Tests:**
- `a_fractional_upstream_phase_samples_between_our_samples` - a known ramp read at phases
  0, 0.25, 0.5 and 0.75 gives the interpolated values.
- `unequal_legs_arrive_when_they_should`.
- `the_upstream_codeword_is_the_level_we_meant` - at phase 0 with no noise, every codeword
  out equals the codeword in.
- The four existing network tests pass unchanged.

**Digests:** `code-tests.md` (7, items 6, 12, 13), `code-digital.md` (9.2, N1, N7, N8, N10).

---

#### WP-08: The downstream symbol clock, for slaving the upstream (S)

**Clauses:** 6.2.

**Edits:** `crates/datapump/src/v90/pcm.rs`.

**Does:** `pub fn symbol_clock(&self) -> SymbolClock`, giving the line samples one far-end
symbol takes, smoothed from `drift` rather than from `due`. `due` steps by up to a quarter
half-symbol in `hold_centre` and on every timing-loop update; an upstream transmitter that
followed it would jitter at the far A/D. Nothing else in `pcm.rs` changes.

**Tests:**
- `the_symbol_clock_follows_a_network_120_ppm_fast` - over 10 s of
  `Network::with_clock(120)`, the reported period is within 1 ppm of the truth and moves
  smoothly, not in jumps.
- `the_symbol_clock_does_not_jump_when_the_timing_loop_steps` - the sample-to-sample change
  stays under a stated bound while `due` steps.

**Digests:** `code-analogue.md` (11.3 "Clock slaving", 13.2).

---

### Wave 2: the chain, the codecs and the design

#### WP-09: The V.92 framed sequences (M)

**Clauses:** 8.5.1 (CPt, Table 23), 8.5.4 (Ja, Table 20), 8.6.2 (Jd, Table 21), 8.6.3 (Jp,
Table 22), 8.6.4 (Jp'), 8.7.3 (CPu and CPus, Tables 23 and 24), 8.7.4 (RM, Tables 25 and
26), 8.7.5 (SUVu, Table 27), 8.8.3 (CPd, Table 30), 8.8.5 (SUVd, Table 31).

**Creates:** `crates/datapump/src/v92/sequences.rs`.
**Depends on:** WP-01, WP-04.

**Does:** the wire formats, built on `v90::sequences::{frame, unframe, put, get}` and
`v34::info::crc`.

- `Jd` (bit 47 = 0, bit 48 reserved) and `Jp` (bit 47 = 1, epsilon in 18:33, the 4/8-point
  choices in 48 and 49), and `Jp'` as 12 zeros.
- `Descriptor`: the V.90 Table 12 body, then the two new upstream rate-mask blocks before
  the CRC, one fill bit, then zeros to a multiple of 12 bits. With N = 0 the descriptor is
  276 bits, which is the check the Recommendation itself states.
- `Cp { kind: Cpt | Cpu | Cpus, .. }` on Tables 23 and 24: bit 18 = 0, the 2-bit type at
  19:20, drn at **21:25** (not V.90's 20:24), no silence bit, no upstream rate mask, fill
  to a multiple of 12 symbols.
- `Suvu` (Table 27) and `Suvd` (Table 31), 52 bits before padding.
- `Cpd` (Table 30) with the part walker of `spec-phase4-signals-digital.md` 5.3, never an
  absolute offset beyond bit 50, plus `From<&Parameters>` and `TryInto<Parameters>`.
- `Rm` and `RmPrime` as the Ki patterns of Tables 25 and 26, row 11 read as K11 (erratum E1).
- Finders for each, dispatching on bit 18 (SUV = 1, CP = 0) and bits 19:20.

**Tests:** one round-trip test per table, plus:
- `a_jp_carries_epsilon_in_bits_18_to_33_and_says_jp_in_bit_47` - CRC 0x3E4E for
  epsilon = 0x8000 with bit 48 = 1.
- `a_jd_with_every_rate_and_look_ahead_three_is_the_vector_we_read` - CRC 0xF366.
- `a_descriptor_with_no_dil_is_two_hundred_and_seventy_six_bits` - and CRC 0xB71C with all
  19 upstream rates enabled.
- `a_cpt_is_padded_to_a_multiple_of_twelve_symbols` - the lengths 300, 432, 972 and 1788.
- `a_cpd_without_its_optional_parts_puts_the_crc_where_the_walker_says` - the 69-bit
  minimal CPd with CRC 0x570B, and the same with bit 33 set giving 0x5BE7.
- `a_cpd_with_every_part_round_trips_through_parameters` - moduli, 384 coefficients, six
  sets, back to the same `Parameters`.
- `a_suvd_says_suv_in_bit_18` - the four vectors in `spec-phase4-signals-digital.md` 2.4.
- `the_mask_bit_for_a_ucode_is_where_the_table_puts_it` - Ucode 0 at bit 137, 15 at 152,
  16 at 154, 127 at 271.
- `a_v90_cp_is_not_read_as_a_v92_cp` - drn and type do not line up and the parser refuses.

**Digests:** `spec-phase3-signals.md` (3.4, 4.1, 4.4, 5.2, 5.3),
`spec-phase4-signals-analogue.md` (3.3-3.6), `spec-phase4-signals-digital.md` (5, 7),
`code-digital.md` (4).

---

#### WP-10: The upstream data-mode chain (M)

**Clauses:** 6.3 (GPA), 6.4, 8.7.1 (B1u), 8.7.2 (E2u), 8.7.4 (RM), 8.7.7 (FB1u).

**Creates:** `crates/datapump/src/v92/upstream.rs`.
**Depends on:** WP-01, WP-02, WP-03.

**Does:** `UpEncoder::new(&Parameters)` with
`next_level(&mut impl FnMut() -> bool) -> f64`: pull K scrambled bits at each data-frame
start, run the modulus encoder, then the chain, and emit `LU*G*v(n)`. GPA comes from
`v32::Scrambler::new(Mode::Answer)`; the polynomial goes with the role, never with who
dialled. Plus the sequences that ride the chain:

- `b1u()`: 48 data frames of scrambled ones, every memory zeroed first, the first symbol
  being data frame interval 0 and n = 0;
- `fb1u()`: 48 frames of scrambled, differentially encoded ones on the **old** parameters,
  with no reset;
- `e2u(extend)`: one frame of scrambled, differentially encoded zeros, 13 symbols when CPd
  bit 29 is set;
- `rm()` and `rm_prime()`: the Tables 25 and 26 Ki forced into the chain, trellis coded,
  with precoder, prefilter and convolutional state carried over from data mode;
- an information-sequence mode that feeds a framed sequence's bits through the scrambler
  and the modulus encoder at K bits per frame, for fast parameter exchange.

**Tests:**
- `b1u_is_the_same_forty_eight_frames_every_time` - a pinned vector for one `Parameters`,
  so a later refactor cannot move it.
- `rm_forces_the_k_pattern_of_table_25` - the decoded Ki per interval match, and RM' is the
  complement.
- `e2u_grows_by_one_symbol_when_the_digital_modem_asks` - and B1u's interval 0 moves with it.
- `every_sequence_boundary_lands_on_a_twelve_symbol_frame` - a debug assertion holds across
  TRN2u, SUVu, CPu and E2u.
- `the_transmit_power_is_lu_squared_when_g_is_designed_for_it` - the measured RMS of
  G*v over 100 000 symbols is 1 within a stated tolerance.

**Digests:** `spec-intro-transmitter.md` (6.3, 6.4, "Use of the chain"),
`spec-phase4-signals-analogue.md` (2, 3.1, 3.2, 3.5, 3.8).

---

#### WP-11: The upstream decoder and the RM watch (M)

**Clauses:** 6.4.1-6.4.4 read backwards.

**Creates:** `crates/datapump/src/v92/decoder.rs`.
**Depends on:** WP-01, WP-02, WP-03.

**Does:** the digital modem's data-mode decoder, given equalised samples and `Parameters`:

- a generic `Viterbi4d` over the V.34 16/32/64-state codes clocked once per 4 symbols,
  taking a closure for the per-dimension metrics, because `v34::data::Decoder` is welded to
  V.34's grid;
- the 2D subset label from Figure 9/V.34 on `(2*eta_a+1, 2*eta_b+1)`, which depends only on
  `(eta_a mod 4, eta_b mod 4)`;
- `Ki` from the surviving `eta`: `eta mod Mi` for k < 3, `((eta - p)/2) mod Mi` for k = 3;
- the 12-interval modulus decode and the GPA descramble;
- `RmWatch`, recognising the Tables 25 and 26 patterns in the decoded Ki, and a
  polarity-blind `RuWatch` on raw samples (period 6);
- no V.34 modulo encoder and no superframe inversion: 6.4.4 cites only 9.6.3.2.

**Tests:**
- `every_bit_comes_back_over_a_perfect_channel` - the WP-10 encoder into this decoder,
  100 000 bits at drn 19, zero errors, at 16, 32 and 64 states.
- `the_trellis_earns_its_keep` - at a noise level where a symbol-by-symbol slicer errs, the
  Viterbi's error rate is at least a stated factor lower.
- `rm_is_recognised_from_the_decoded_k_and_not_from_a_tone` - RM through the full chain,
  spread by a real precoder, is still detected.
- `a_forty_frame_survivor_depth_is_enough` - the error rate does not improve past the
  chosen depth.

**Digests:** `spec-intro-transmitter.md` (6.4.3, 6.4.4, Q-15), `code-digital.md` (7.7).

---

#### WP-12: The upstream channel estimate and the CPd design (M)

**Clauses:** 6.4.2, 8.8.3 (the design rules and its NOTE), Table 18 bits 12:17.

**Creates:** `crates/datapump/src/v92/design.rs`.
**Depends on:** WP-01, WP-03.

**Does:** the second half of the riskiest DSP. Given a T-spaced channel estimate `h` from a
known training sequence, the noise floor, the analogue modem's filter capabilities
(sections, Ltot, Lmax) and its enabled rate mask, produce a `Parameters`:

1. `Channel::estimate(known_levels, received)` by least squares, with a real-valued solver;
   `dsp::least_squares` takes `Complex`, so either feed zero imaginary parts or add a real
   variant.
2. An MMSE-DFE on `h`: the feed-forward section becomes z2 (and p2 where allowed), the
   feedback becomes p1 (and z1 where allowed), within Lmax each and Ltot in total.
3. Constellations per constellation-frame index j, chosen from the codec's own levels at a
   spacing the measured noise allows, the way `dil::choose` already does downstream; no
   zero point, non-empty sets first, `2*LC <= 128`.
4. Moduli and rate: the highest drn in the Ja mask with `2^K <= product(Mi)` and margin,
   subject to `N >= Mi` and `N >= 2*Mi` at k = 3.
5. `G` from a simulation of the actual transmitter (A3), so that `E[(G*v)^2] = 1`,
   quantised to unsigned Q0.16.
6. Verification: run the transmitter model through `h` and check the decisions before the
   parameters are ever sent.

**Tests:**
- `the_designed_precoder_leaves_little_residual_isi` - on a synthetic codec channel with a
  DC null and a 4 kHz roll-off, the residual at the A/D is below a stated fraction of the
  level spacing.
- `the_coefficients_fit_inside_ltot_and_lmax` - for all four capability codes, including
  the minimum p1-and-z2-only modem.
- `the_class_rules_hold_for_every_interval` - `N >= Mi`, and `N >= 2*Mi` at intervals 3, 7
  and 11.
- `the_gain_lands_inside_q0_16` - `0 < 4G < 1` for every design in the sweep.
- `a_noisier_channel_is_designed_slower` - the chosen drn falls monotonically as noise rises.
- `the_design_verifies_itself_before_it_is_sent` - a deliberately broken coefficient set is
  caught by the self-check.

**Digests:** `code-digital.md` (7.6), `spec-phase4-signals-digital.md` (5.1, 5.4).

---

#### WP-13: The two-level and TRN2u upstream signals (M)

**Clauses:** 8.5.2 (E1u), 8.5.4 (Ja modulation), 8.5.5 (Ru), 8.5.6 (Su), 8.5.7 (TRN1u),
8.7.6 (TRN2u, Tables 28 and 29), 6.3.

**Creates:** `crates/datapump/src/v92/up_signals.rs`.
**Depends on:** WP-01.

**Does:** every upstream signal that bypasses the precoder, as sources and as readers.

- `Trn1u`: GPA fed ones, the scrambler zeroed first, output **0 maps to +LU**, the opposite
  of the downstream convention.
- `Ru` and `RuBar`: `{+L,+L,+L,-L,-L,-L}` and its inverse.
- `Su` and `SuBar`: `{+a, 0, +a, -a, 0, -a}` with `a = sqrt(3/2)*LU`.
- `TwoPoint`: the scrambled, differentially encoded one-bit-per-symbol sender that Ja, CPt
  and E1u use, with the differential memory seeded from the last TRN1u symbol and the
  24-one preamble before the first Ja descriptor and the first CPt.
- `Trn2u`: 4 or 8 point per Tables 28 and 29, the scrambler reset at its start, only the
  sign bit differential, seeded from E1u's last sign, and the bit order behind the named
  constant of section 3.
- Matching readers for the digital modem: differential decode, then GPA descramble.

**Tests:**
- `trn1u_starts_with_the_signs_the_scrambler_gives` - the first 48 signs equal
  `111110000011111000001110011111000110000011100100`.
- `ru_and_su_have_the_power_and_lines_we_expect` - mean power LU squared, lines at
  1333.3 Hz and 4000 Hz.
- `both_trn2u_constellations_have_mean_square_lu_squared`.
- `a_two_point_sequence_round_trips_through_its_reader` - Ja, CPt and E1u, including the
  preamble and the differential seed.
- `the_trn2u_bit_order_switch_is_symmetric` - sender and reader agree at both settings of
  `TRN2U_SIGN_LAST`, so flipping it after a capture is a one-line change.
- `e1u_is_twelve_zeros_and_is_told_from_another_cpt` - at a 12-symbol boundary, zeros mean
  E1u and ones mean another sync.

**Digests:** `spec-phase3-signals.md` (3.1-3.3, 4.2, 4.5-4.7),
`spec-phase4-signals-analogue.md` (2.2, 3.7).

---

#### WP-14: The PCM upstream transmitter (M)

**Clauses:** 6.2, 9.5.2.1.7, 9.5.2.1.8.

**Creates:** `crates/datapump/src/v92/transmit.rs`.
**Depends on:** WP-01, WP-08.

**Does:** `PcmTransmitter`, the counterpart of `v34::qam::Transmitter`: it turns 8000
symbols/s levels into line samples at `fs`, pulling symbols as its interpolator needs them.

- A windowed-sinc reconstruction just under 4 kHz, built as `pcm::Receiver` builds its own.
- `lookahead()` in symbols, so that mid-segment cuts are computed on the symbol stream and
  never on line time.
- `delay(fraction_of_t)`: a one-off phase shift, used twice, +0.5T at the first S-bar-u
  (24.5T) and +epsilon*T at the second (24+epsilon)T, and never re-stepped afterwards.
- Clock slaving: free-run at a nominal 8000 symbols/s until the downstream receiver has
  trained, then follow `symbol_clock()`. The switch happens during the silence after Ja,
  where a phase step costs nothing and the digital modem has not yet measured Su.
- A `symbol_at(n) -> line sample` mapping, so the 40 +/- 1 ms turnarounds and the
  100 ms + RTD acknowledgement window can be measured at the line terminals.

**Tests:**
- `at_twice_the_rate_every_other_sample_is_the_level` - at fs = 16 000, phase 0.
- `a_half_symbol_delay_moves_the_stream_by_half_a_symbol` - the correlation against an
  undelayed copy peaks at 0.5T within a stated error.
- `epsilon_resolves_to_a_sixty_five_thousandth_of_a_symbol` - a sweep of eight epsilon
  values, each measured back through `Network::with_upstream_phase`.
- `the_transmitter_follows_a_network_120_ppm_fast` - over 10 s, the symbol instants at the
  far A/D stay within a stated fraction of T.

**Digests:** `code-analogue.md` (11.3), `spec-phase3-procedures.md` (7.1 A7 and A8, 13.2
item 5).

---
### Wave 3: de-risking the DSP

#### WP-15: The PCM upstream link on a synthetic channel (M) - the de-risk spike

**Clauses:** 6.4, 8.8.3.

**Creates:** `crates/datapump/tests/v92_upstream.rs`.
**Depends on:** WP-10, WP-11, WP-12.

**Does:** the most valuable early test in the plan. It joins the transmit chain, the CPd
design and the decoder across a channel that has the hard parts of a codec path, with no
protocol and no state machine in the way, so that the answer to "can PCM upstream work at
all" arrives before anything is built on top of it.

The harness lives in the test file: a first-order high-pass near 200 Hz for the codec's DC
null, a raised-cosine roll-off from 3.4 to 4 kHz, a flat-to-tilted loop response, additive
noise, a settable A/D sampling phase and a G.711 quantiser. The flow per case is: estimate
the channel from a known TRN1u/TRN2u run, design a `Parameters`, run random data through
`UpEncoder`, quantise at the A/D, decode, and count bit errors.

**Tests:**
- `the_designed_precoder_carries_the_top_of_the_ladder_over_a_clean_codec_channel` - at
  about 40 dB of signal to quantising noise, the design picks drn in the top third and the
  bit error rate is zero over 200 000 bits. This is the go/no-go for PCM upstream.
- `the_rate_falls_a_rung_at_a_time_as_noise_rises` - a sweep of five noise levels; the
  chosen drn falls monotonically and the error rate stays at zero.
- `a_channel_the_filters_cannot_equalise_is_refused_not_mis_designed` - with a deep notch
  inside the band, the design either drops to the bottom rung or reports that it cannot
  serve the analogue modem, and never returns coefficients whose decisions are wrong.
- `the_design_obeys_every_limit_the_analogue_modem_announced` - Ltot, Lmax, the allowed
  sections, the class rules and `2^K <= product(Mi)`, over the whole sweep.
- `a_gain_change_at_the_codec_is_absorbed_by_g` - up_gain 0.25, 0.5 and 1.0 all connect at
  the same rate.

If the first test cannot be made to pass, stop and report: the rest of the plan then runs
with V.34 upstream as the product (WP-30 already builds that ladder), and PCM upstream
becomes a research branch.

**Digests:** `code-digital.md` (7.6, 7.7, 11), `spec-intro-transmitter.md` (6.4),
`code-tests.md` (7).

---

#### WP-16: The digital modem's upstream PCM receiver (M)

**Clauses:** 8.5.5, 8.5.6, 8.5.7, 9.5.1.1.1, 9.5.1.1.2, 9.5.1.1.10.

**Creates:** `crates/datapump/src/v92/receiver.rs`.
**Depends on:** WP-01, WP-07, WP-11, WP-13.

**Does:** the front end. One sample per symbol arrives from the A/D, so the equaliser is
T-spaced; there is no interpolation to do at this end because the analogue modem is
required to put its symbols on the A/D instants.

- A period-6 hunt for Ru and Su and their reversals, in either polarity, in the shape of
  `pcm::Hunt` but at T spacing.
- Least-squares training on TRN1u, which is known from its first symbol (GPA from a zero
  register fed ones), with a coarse alignment search and retries on a later stretch, in the
  shape of `pcm.rs`'s `TRAIN_TRIES`.
- A feed-forward section plus decision feedback for the Phase 3 and Phase 4 decisions,
  which are 2, 4 or 8 level.
- `Heard { Ru, RuReversal { at }, Trained { snr_db }, Su, SuReversal { at }, Symbol(..),
  Lost, Found }`, the event-queue shape the rest of the code base uses.
- A `symbol: u64` count with `interval() = symbol % 12`, started at the first symbol of the
  second TRN1u and carried modulo 12 thereafter (section 3).
- The TRN1u-modulation and TRN2u-modulation bit readers from WP-13, plumbed to the
  sequence finders.

Slip re-framing and echo cancellation are deliberately **not** here; they are WP-31, once
the network can produce slips and echo.

**Tests:**
- `ru_and_its_reversal_are_found_whatever_the_polarity` - through `Network::up` at four A/D
  phases and both polarities.
- `trn1u_trains_the_receiver_to_within_a_stated_snr` - on the WP-15 harness channel, after
  2040T the residual is below a stated level.
- `training_retries_on_a_later_stretch_when_the_first_fails` - a burst at the start of
  TRN1u costs a retry, not a failure.
- `the_twelve_symbol_count_starts_at_the_second_trn1u` - fed a scripted Ru, TRN1u, Ja, Su,
  S-bar-u, TRN1u stream from WP-13 and WP-14, `interval()` is 0 at the right symbol.
- `su_and_its_reversal_are_found_through_the_zeros` - Su's zero positions do not defeat the
  period-6 hunt.

**Digests:** `code-digital.md` (7.1, 7.2, 7.4), `spec-phase3-signals.md` (4.5-4.7).

---

#### WP-17: What the V.92 capture actually holds (S)

**Clauses:** 8.4 (reading INFO off a capture).

**Creates:** `crates/datapump/tests/v92_vector.rs`.
**Depends on:** WP-05.

**Does:** reads `tests/vectors/v92-56k.wav` the way `v90_vector.rs` reads its file, and
reports what the Conexant modem and its server actually negotiated. Its README says
"V.34-style startup", so this may find no PCM upstream at all; either answer is worth
knowing before the Phase 3 state machines are written, because a capture is the only thing
that can settle the open readings of section 3.

**Tests:**
- `the_v92_menus_offer_pcm` - the V.8 CM and JM octets.
- `the_info_sequences_say_whether_v92_and_pcm_upstream_were_used` - INFO0d bit 27, INFO0a
  bit 26 where readable, INFO1d bit 70, and INFO1a bits 34:36 (6 means Table 18, 3 to 5
  means Table 19).
- `if_pcm_upstream_was_used_ru_and_trn1u_are_on_the_tap` - a period-6 hunt on the upstream
  half of the tap after INFO1a; the test prints what it finds and is `#[ignore]`d if the
  capture turns out not to contain it.

**Digests:** `code-tests.md` (8.4), `spec-phase2-signals.md` (10.4).

---

### Wave 4: the sampling phase, and the handshake machine

#### WP-18: The sampling phase, and the spike through the real receiver (M)

**Clauses:** 9.5.1.1.6, 9.5.1.1.7, 9.5.1.1.8, 8.6.3 (Jp bits 18:33), 9.5.2.1.7, 9.5.2.1.8.

**Creates:** `crates/datapump/src/v92/phase.rs`.
**Edits:** `crates/datapump/src/v92/receiver.rs`, `crates/datapump/tests/v92_upstream.rs`.
**Depends on:** WP-15, WP-16.

**Does:** the piece that has no specified algorithm at all: the digital modem must work out
where the A/D instants fall inside the analogue modem's symbol and report it as epsilon.

The estimator fits the known Su pattern `{+a, 0, +a, -a, 0, -a}` through a short estimated
channel at each trial phase, over the Su before the first S-bar-u and the Su after it. The
first S-bar-u is 24.5T, so the second Su is half a symbol away from the first, which gives
two views a half symbol apart. Epsilon corrects the timing that holds **after** that half
symbol, so the total shift from the original grid is 0.5 + epsilon (section 3, and
`spec-phase3-signals.md` A2). The result is quantised to Jp's 16-bit fraction.

The spike then runs end to end through the real receiver, with the A/D phase swept, which
is the second half of the de-risk.

**Tests:**
- `su_gives_the_sampling_phase_to_within_a_sixty_fourth_of_a_symbol` - eight A/D phases
  through `Network::with_upstream_phase`, reported epsilon against the truth.
- `the_estimate_survives_the_noise_the_channel_has` - at the WP-15 noise levels, the error
  stays inside the same bound.
- `the_link_carries_data_at_every_sampling_phase` - the WP-15 flow with the real receiver
  and the measured epsilon applied by the WP-14 transmitter; zero bit errors at all eight
  phases.
- `an_uncorrected_phase_costs_what_we_expect` - the same run with epsilon forced to zero,
  printing the rate loss, so the value of the mechanism is on record.

**Digests:** `code-digital.md` (7.3), `spec-phase3-procedures.md` (11 item 3, 13.3 items 1
and 2).

---

#### WP-19: The SUV, CP and E exchange (M)

**Clauses:** 9.6.1.1.2, 9.6.1.1.3, 9.6.1.1.4, 9.6.2.1.2, 9.6.2.1.3, 9.6.2.1.4, and the
grouping rules of 8.7.3, 8.7.5, 8.8.3 and 8.8.5.

**Creates:** `crates/datapump/src/v92/exchange.rs`.
**Depends on:** WP-09.

**Does:** the handshake both modems run, once, with a context (A4). State: `sent_ack`,
`got_peer_cp`, `peer_acked`, `my_cp_end`, `repeat_cp`, `need_single_cp`, and the
round-trip delay. Events: `peer_suv`, `peer_cp`, `peer_e`, `sequence_started(kind) -> ack
bit`, `sequence_ended(kind, at)`. Query: `next() -> Trn2 | Suv | Cp | E`.

The rules it owns: one CP only, sent after the first peer SUV; the acknowledge bit set on
every later sequence once a peer CP has arrived; repeated CPs only when no acknowledge has
been seen in any sequence whose reception completes by own-CP-end + 100 ms + RTD; E only
once an acknowledged sequence has been sent and an acknowledged sequence or the peer's E
has been received; and every sequence in a group identical apart from bit 33.

**Tests:**
- `the_two_ends_cross_as_figure_12_draws_them` - a scripted event trace reproduces
  SUVd SUVd CPd SUVd' SUVd' Ed against SUVu SUVu CPu SUVu SUVu' SUVu' E2u.
- `a_cp_that_arrives_first_is_acknowledged_in_the_single_cp` - Figure 13.
- `a_lost_cp_is_repeated_after_a_hundred_milliseconds_and_a_round_trip` - Figure 14,
  including that the sequence straddling the deadline still counts, and that nothing is
  repeated when an acknowledge arrives inside it.
- `a_long_round_trip_does_not_cause_a_spurious_repeat` - at RTD 1.5 s.
- `a_group_differs_only_in_the_acknowledge_bit` - the parameters are not recomputed between
  repeats.

**Digests:** `spec-phase4-procedures.md` (6.5), `spec-phase4-signals-analogue.md` (5.1).

---

### Wave 5: Phase 3, as far as Jd

#### WP-20: The analogue modem, Phase 3 to Jd (M)

**Clauses:** 9.5.2.1.1, 9.5.2.1.2, 9.5.2.1.3, 9.5.2.1.4, 9.5.2.1.5, 9.5.2.2.1, 9.5.2.2.2,
8.5.4, 8.5.5, 8.5.7.

**Creates:** `crates/datapump/src/v92/analogue.rs`.
**Depends on:** WP-01, WP-06, WP-09, WP-13, WP-14.

**Does:** the first half of the V.92 analogue modem: `Settings` built from a Table 18
INFO1a and an INFO0d; the `Stage`, `Up` and `Deadlines` skeleton; and the sequence

silence 70 +/- 5 ms, Ru 384T, R-bar-u 24T, (no MD), TRN1u at least 2040T in whole 12-symbol
frames, then Ja: 24 ones then the descriptor repeated, cut **at the next 12-bit boundary**
after the Sd-to-S-bar-d reversal is detected, then silence.

While silent it trains `pcm::Receiver` on the first 2040T of TRN1d and then reads Jd with a
V.92 reader that checks bit 47 before anything else. Deadlines: TR3 (relaxed, section 3)
from the start of Ja, TR4 4500 ms from the end of Ja, and the 20 s + 6 RTD start-up
watchdog from the end of INFO1a.

**Tests:** driven by a scripted downstream built from `v90::ucode`, `v90::sequences` and
`v92::sequences`.
- `the_upstream_opens_with_seventy_milliseconds_of_silence_then_ru_and_trn1u` - the segment
  lengths measured at the line, Ru 384T and R-bar-u 24T exactly.
- `ja_is_cut_at_the_next_twelve_bit_boundary_after_the_reversal` - across a sweep of
  reversal instants, the cut is always on a 12-bit boundary and never more than 11 bits
  late.
- `trn1u_is_a_whole_number_of_twelve_symbol_frames_and_at_least_2040t`.
- `a_jp_is_not_read_as_a_jd` - the same frame with bit 47 set is ignored.
- `no_sd_reversal_retrains` and `no_jd_retrains` - TR3 and TR4 fire with the right reason
  strings.
- `the_start_of_md_to_the_end_of_trn1u_fits_inside_rtd_plus_four_seconds`.

**Digests:** `spec-phase3-procedures.md` (7.1 A1-A5, 7.2, 8), `spec-phase3-signals.md`
(3.1, 3.3, 3.4), `code-analogue.md` (4, 7, 11.4).

---

#### WP-21: The digital modem, Phase 3 to Jd (M)

**Clauses:** 9.5.1.1.1, 9.5.1.1.2, 9.5.1.1.3, 9.5.1.1.4, 9.5.1.1.5, 9.5.1.2.1, 8.6.2,
8.6.7, 8.6.8.

**Creates:** `crates/datapump/src/v92/digital.rs`.
**Depends on:** WP-01, WP-04, WP-09, WP-16.

**Does:** the first half of the V.92 digital modem: `Settings` from the Table 18 INFO1a
(UINFO, the filter capabilities, the MD length) and its own INFO0d; the `Stage` and `Out`
skeleton; and

silence while it hunts Ru and the Ru-to-R-bar-u reversal (waiting out the MD when INFO1a
asks for one), training on TRN1u, reading Ja and its DIL descriptor with the upstream rate
mask, then, after at most 500 ms, Sd 384T, S-bar-d 48T, TRN1d at least 2040T, and Jd within
4000 ms of the start of TRN1d, repeated.

Downstream generation reuses V.90: Sd and S-bar-d from 8.4.4/V.90, TRN1d from 8.4.5/V.90,
and the Jd framing from WP-09 with bit 47 = 0 and bit 48 reserved.

**Tests:** driven by the real analogue upstream from WP-13 and WP-14 through
`Network::up`, so the two halves meet without either state machine being finished.
- `ru_then_trn1u_then_ja_is_read_off_the_line` - the descriptor decodes, with the upstream
  rate mask it was given.
- `the_downstream_runs_sd_then_s_bar_d_then_trn1d_then_jd` - the lengths, and that Jd
  starts within 4000 ms of TRN1d.
- `the_optional_wait_before_sd_is_at_most_five_hundred_milliseconds`.
- `no_ja_within_four_and_a_half_seconds_and_a_round_trip_retrains`.
- `a_ja_descriptor_names_the_dil_we_would_have_designed` - round trip against
  `v90::dil::design`.

**Digests:** `spec-phase3-procedures.md` (6.1 D1-D5, 6.2, 8), `spec-phase3-signals.md`
(5.1, 5.2, 5.7, 5.8), `code-digital.md` (8.2, 8.3).

---

### Wave 6: Phase 3, from Su to the end

#### WP-22: The analogue modem, from Su to E1u (M)

**Clauses:** 9.5.2.1.6, 9.5.2.1.7, 9.5.2.1.8, 9.5.2.1.9, 9.5.2.1.10, 9.5.2.1.11, 8.5.1,
8.5.2, 8.5.6.

**Edits:** `crates/datapump/src/v92/analogue.rs`.
**Depends on:** WP-18, WP-20, WP-06.

**Does:** the rest of the analogue modem's Phase 3.

After Jd, and no later than 5000 ms from the start of the silence: Su 144T, S-bar-u 24.5T
(a half-symbol transmitter delay, not extra symbols), then Su until Jp arrives. On a
CRC-valid Jp with bit 47 = 1: circuit 107 ON, S-bar-u for 24T plus the epsilon fraction
from bits 18:33, and the 4- and 8-point choices from bits 48 and 49 stored for Phase 4 and
for renegotiation. Then TRN1u, whose first symbol is upstream interval 0, while the DIL
arrives; the DIL is read through `v90::downstream::DilReader`. After at least 2040T of
TRN1u, and within 5000 ms of that S-bar-u, 24 ones then CPt repeated, built from
`dil::choose`. On Ri: finish the CPt, send E1u, and enter Phase 4.

**Tests:**
- `su_is_one_hundred_and_forty_four_symbols_then_a_half_symbol_shift` - measured at the
  line through `Network::with_upstream_phase`.
- `epsilon_from_jp_moves_everything_after_it` - a sweep of eight epsilon values; the far
  A/D sees the symbols land where Jp asked.
- `circuit_107_comes_on_when_jp_is_detected`.
- `the_second_trn1u_starts_upstream_interval_zero` - the frame count at CPt and E1u is a
  multiple of 12.
- `cpt_starts_within_five_seconds_of_the_second_s_bar_u_and_after_2040t`.
- `on_ri_the_current_cpt_is_finished_and_then_e1u_goes` - never mid-sequence.
- `the_cpt_we_send_is_one_the_digital_modem_can_send` - drn, Sr, ld against the Jd we read.

**Digests:** `spec-phase3-procedures.md` (7.1 A6-A11), `spec-phase3-signals.md` (4.1, 4.2,
4.6), `code-analogue.md` (11.4).

---

#### WP-23: The digital modem, from Su to R-bar-i (M)

**Clauses:** 9.5.1.1.6 to 9.5.1.1.13, 9.5.1.2.2, 8.6.3, 8.6.4, 8.6.1, 8.6.5, 8.6.6.

**Edits:** `crates/datapump/src/v92/digital.rs`.
**Depends on:** WP-18, WP-21, WP-12.

**Does:** the rest of the digital modem's Phase 3.

On detecting Su, measure the sampling phase across Su, the 24.5T S-bar-u and the Su after
it. Once epsilon is known, finish the current Jd and send Jp repeatedly. On the second
Su-to-S-bar-u reversal: finish the current Jp, assert circuit 107, send Jp' (12 zeros).
Then send the DIL the descriptor asked for while receiving the second TRN1u and starting
the modulo-12 count. On CPt: finish the DIL segment, send Ri. On E1u: send R-bar-i (24T)
and enter Phase 4. The N = 0 path sends SCR and the Ri/CPt order of 9.5.1.1.13; it is
parsed and implemented but never requested (section 3), and the modem accepts both orders
of Figure 11 and the text.

Also the Su watchdog: no Su within 5100 ms + RTD of the start of TRN1d retrains.

**Tests:** driven by the finished WP-22 analogue modem's upstream through `Network`.
- `jp_follows_jd_only_once_the_phase_is_known` - and always at a Jd boundary.
- `jp_prime_follows_the_second_reversal_and_107_comes_on_first`.
- `the_dil_is_the_one_the_descriptor_asked_for` - segment lengths, reference symbols, sign
  and training patterns, ended on a segment boundary.
- `ri_follows_cpt_and_r_bar_i_follows_e1u`.
- `no_su_within_five_point_one_seconds_and_a_round_trip_retrains`.
- `the_scr_path_is_accepted_in_either_order` - a scripted N = 0 exchange.

**Digests:** `spec-phase3-procedures.md` (6.1 D6-D13, 6.2, 13.3 item 4),
`spec-phase3-signals.md` (5.3-5.6), `code-digital.md` (8.3).

---

### Wave 7: Phase 3, call to call

#### WP-24: Phase 3 between our two modems (M)

**Clauses:** 9.5 as a whole.

**Creates:** `crates/datapump/tests/v92_call.rs`.
**Depends on:** WP-22, WP-23, WP-07.

**Does:** the first V.92 thing that is a call. `settled_v92()` returns the analogue and
digital `Settings` as Phase 2 would leave them (INFO0d bit 27 set, INFO1d bit 70 set, a
Table 18 INFO1a with both rate fields 6, UINFO 79, the filter capability codes, MD length
0), and `Call` runs the two modems over `Network` one tick at a time, in the shape
`v90_call.rs` already uses.

**Tests:**
- `phase_3_completes_with_pcm_upstream_over_a_clean_network` - both ends reach Phase 4
  entry; the CPt the digital modem holds is the one the analogue modem sent; the DIL was
  read; the whole phase takes less than a stated wall time.
- `every_sampling_phase_is_found_and_corrected` - eight A/D phases, epsilon within a
  sixty-fourth of a symbol of the truth and applied.
- `a_voip_length_round_trip_still_completes_phase_3` - 0.6 s each way; the relaxed TR3 and
  the RTD-bearing watchdogs all hold.
- `a_sound_card_clock_a_hundred_and_twenty_ppm_off_is_followed_upstream` - ten seconds
  through Phase 3 with the transmitter slaved.
- `an_a_law_network_completes_phase_3`.

**Digests:** `code-tests.md` (2.1, 8.1), `spec-phase3-procedures.md` (2).

---

#### WP-25: Network, part B: echo, transcoding, gain control, slips and softphone paths (M)

**Clauses:** none. Test infrastructure.

**Edits:** `crates/datapump/src/v90/network.rs`.
**Depends on:** WP-07.

**Does:** the impairments the robustness milestone needs, all in the builder style already
there, with the per-direction settings held in one private `Impairments` struct used twice.

- `with_echo(hybrid_db, taps)` and `with_far_echo(db, delay)`: the downstream waveform
  added into the upstream sum **before** the A/D quantiser, and the reverse at the analogue
  side.
- `with_transcoder(to: Law, low_pass)`: decode, low-pass, re-encode, as the Crazytel path
  does, with its measured -3 dB at 3.75 kHz and -18 dB at 4 kHz.
- `with_upstream_gain_control(ceiling, release)`, mirroring the downstream one.
- Direction-tagged slips and a settable slip length, so both the 20 ms (160-codeword)
  inserts and the 10 ms (80-codeword) cuts can be placed in either direction.
- `with_robbed_bits(down, up)` and `with_pads(down_db, up_db)`.
- `UpPath { Loop, Straight { phase }, Resampled { delay } }`, so the both-ends-ours
  softphone case, where our samples are forwarded verbatim, can be told from a real
  analogue loop; on the straight path a clock offset becomes packet slips, not skew.

**Tests:** one per feature, in the style of the four that exist:
- `an_upstream_robbed_bit_moves_every_other_codeword_in_one_octet_of_six`.
- `an_upstream_slip_moves_everything_after_it_by_the_slip_s_length`, and
  `a_ten_millisecond_cut_moves_everything_by_eighty_codewords`.
- `the_transcoder_is_three_db_down_at_3750_and_eighteen_db_down_at_4000`.
- `the_hybrid_echo_is_where_it_was_put` - a measured impulse at the stated delay and level.
- `a_straight_softphone_path_hands_the_encoder_our_samples_exactly`.
- `a_clock_off_on_a_softphone_path_becomes_slips` - one 20 ms slip every 175 s at 114 ppm.

**Digests:** `code-tests.md` (7), `code-digital.md` (9.2).

---
### Wave 8: Phase 4, and the Phase 2 flags

#### WP-26: The analogue modem, Phase 4 and data (M)

**Clauses:** 9.6.2.1.1 to 9.6.2.1.6, 9.6.2.2.1, 8.7.1, 8.7.2, 8.7.3, 8.7.5, 8.7.6, 6.4.

**Edits:** `crates/datapump/src/v92/analogue.rs`.
**Depends on:** WP-10, WP-19, WP-24.

**Does:** TRN2u at the size Jp bit 48 asked for, the scrambler reset at its start and the
differential sign seeded from E1u's last sign, for at least 12000T or until an SUVd
arrives and the modem is ready to receive a CPd. Then SUVu repeated, driven by
`Exchange`: one CPu after the first SUVd, the acknowledge bit once a CPd has arrived,
repeats only when the 100 ms + RTD window passes with no acknowledgement, then E2u
(13 symbols when CPd bit 29 is set), then B1u through `UpEncoder` built from the CPd's
`Parameters`, then data with circuit 106 following 105.

Downstream in Phase 4: SUVd and CPd are read out of the V.90 data frames with the
`Mapping` the CPt defined, then Ed, then B1d, then data; circuit 104 is unclamped and 109
turned on at the end of B1d. The 20 s + 6 RTD watchdog from the end of sending INFO1a
retrains if B1d never comes.

The CPu the analogue modem sends carries the downstream choice `dil::choose` already
makes, in the V.92 header, with **no upstream rate mask** (that moved to Ja).

**Tests:**
- `trn2u_runs_for_twelve_thousand_symbols_or_until_an_suvd_arrives`.
- `one_cpu_goes_after_the_first_suvd_and_is_repeated_only_when_unacknowledged`.
- `e2u_grows_by_a_symbol_when_the_digital_modem_asked_and_b1u_still_starts_interval_zero`.
- `b1u_is_the_frames_the_cpd_parameters_predict` - the digital modem's own model of B1u,
  computed independently, matches sample for sample.
- `no_b1d_within_twenty_seconds_and_six_round_trips_retrains`.
- `the_cpu_asks_for_a_downstream_rate_the_jd_enabled`.

**Digests:** `spec-phase4-procedures.md` (6.3, 6.4), `spec-phase4-signals-analogue.md`
(3.1-3.6, 5.1), `code-analogue.md` (11.5).

---

#### WP-27: The digital modem, Phase 4 and data (M)

**Clauses:** 9.6.1.1.1 to 9.6.1.1.6, 9.6.1.2.1, 8.8.1 to 8.8.6.

**Edits:** `crates/datapump/src/v92/digital.rs`.
**Depends on:** WP-12, WP-19, WP-24.

**Does:** TRN2d for at least 2040T with the CPt constellations, the shaping and K that CPt
defined. While it runs, estimate the upstream channel from TRN2u and **design the CPd**
(WP-12), verifying it against our own model of the transmitter before sending. Once ready
to receive a CPu, send SUVd repeatedly and drive `Exchange`: one CPd after the first SUVu,
the acknowledge bit once a CPu has arrived, repeats only on the 100 ms + RTD rule, then Ed,
then B1d at the negotiated rate with the CPu's data-mode parameters, then data.

Upstream in Phase 4: read SUVu, CPu and CPus from the TRN2u-modulated bits; on E2u
(allowing its optional extra symbol) condition for B1u and check it against the known
sequence the CPd predicts; then unclamp 104, turn 109 on, and run the WP-11 decoder.

SUVu bit 26 ("wait for my CPu") is honoured when it costs nothing and ignored otherwise,
which the Recommendation allows. SUVu bits 27:31 (the analogue modem's measured level) are
checked against the design's power assumption and logged.

**Tests:**
- `trn2d_runs_at_least_2040t_and_uses_the_cpt_constellations`.
- `the_designed_cpd_verifies_before_it_is_sent` - the self-check of WP-12 runs on the real
  channel estimate.
- `one_cpd_goes_after_the_first_suvu_and_is_repeated_only_when_unacknowledged`.
- `b1u_is_recognised_against_the_sequence_our_own_cpd_predicts`.
- `ed_only_goes_after_the_current_sequence_is_complete`.
- `no_b1u_within_twenty_seconds_and_six_round_trips_retrains`.

**Digests:** `spec-phase4-procedures.md` (6.1, 6.2), `spec-phase4-signals-digital.md`
(3-8, 10.1), `code-digital.md` (8.3).

---

#### WP-28: Phase 2: the V.92 capability flags and the Table 18 choice (M)

**Clauses:** 9.3, 9.3.1, and the INFO layout rules of 8.4.1.

**Edits:** `crates/datapump/src/v34/phase2.rs`.
**Depends on:** WP-05.

**Does:** full Phase 2 learns V.92, with the procedure itself unchanged (9.3 says it is
V.90's).

- `Pcm::Analogue` and `Pcm::Digital` carry what this end offers: V.92 capability, whether
  it will request short Phase 2 later, and whether PCM upstream is allowed (`+PIG`).
- `info0_bits` writes INFO0d bit 27 or INFO0a bit 26, and the short-Phase-2 request in the
  other position.
- The far INFO0's V.92 bits are recorded and survive a retrain, because a retrain never
  repeats INFO0.
- INFO1d is laid out as Table 17 when both ends are V.92, with bit 70 set from the digital
  modem's own verdict on the channel.
- The INFO1a choice gains `settle_pcm92` (Table 18) beside today's `settle_pcm` (Table 10),
  taken only when both are V.92, INFO1d bit 70 is set and PCM upstream is allowed.
- `9.3.1`: when both are V.92 and both indicated LAPM in V.8, the pump exposes
  `bypass_odp_adp()` for the error-control layer (WP-34 consumes it).

**Tests:**
- `two_v92_ends_settle_on_a_table_18_info1a` - and both report PCM upstream.
- `a_v90_far_end_gets_the_v90_layouts` - INFO1d Table 9 and INFO1a Table 10, exactly as
  today; every existing V.90 Phase 2 test passes unchanged.
- `bit_70_clear_forbids_table_18` - the analogue modem sends Table 10 instead.
- `pig_off_forbids_table_18`.
- `a_retrain_keeps_the_v92_flags_without_resending_info0`.
- `the_lapm_bypass_needs_both_v92_and_both_prot0`.

**Digests:** `spec-phase2-procedures.md` (2, 4, 5, 11.1), `spec-phase2-signals.md` (10,
16.2), `code-analogue.md` (11.2).

---

### Wave 9: the first complete V.92 call

#### WP-29: A V.92 call that carries data, PCM both ways (M) - milestone M1

**Clauses:** 9.5 and 9.6 end to end.

**Edits:** `crates/datapump/tests/v92_call.rs`.
**Depends on:** WP-26, WP-27.

**Does:** extends WP-24's harness through Phase 4 and into data. This is the milestone the
whole plan is ordered around: two of our modems, over the simulated network, PCM in both
directions, carrying bytes.

**Tests:**
- `a_v92_call_connects_with_pcm_both_ways_and_carries_data` - `Status::Connected` on both
  sides; the downstream rate at or above 48 000; the upstream rate on the PCM ladder and at
  or above a floor set from what WP-15 measured; `carries_data` true in both directions; no
  retrain.
- `the_rates_are_the_ones_the_two_ends_asked_for` - the CPu drn and the CPd drn match what
  each side chose.
- `a_voip_length_round_trip_connects` - 0.6 s each way; the whole start-up fits inside
  20 s + 6 RTD with margin, and the CP repeat rule does not fire spuriously.
- `an_a_law_network_connects_with_pcm_both_ways`.
- `a_far_end_that_stops_sending_is_noticed_at_either_end` - within 3.5 s, on PCM upstream
  levels as well as downstream.
- `the_call_is_no_slower_than_v90_plus_a_half` - a wall-clock budget, so the suite stays
  near today's runtime.

**Digests:** `code-tests.md` (8.1, 9.2, 9.6), `spec-phase4-procedures.md` (5).

---

#### WP-30: Start-up hand-over, V.90 interop and the fallback ladder (M) - milestone M2

**Clauses:** 9.3 (which Phase 3 follows which INFO1a), 9.7 (retrains land in full
Phase 2), 1 g (V.34 in either direction).

**Creates:** `crates/datapump/tests/v92_startup.rs`.
**Edits:** `crates/datapump/src/v90/startup.rs`.
**Depends on:** WP-26, WP-27, WP-28.

**Does:** joins Phase 2 to the V.92 modems, and builds the ladder (A8).

- `Analogue` holds `enum Pcm { V90(v90::analogue::Modem), V92(v92::analogue::Modem) }` and
  implements `status`, `phase`, `take_bits`, `send_bits`, `pending_bits`, `carrier`,
  `retrain`, `renegotiate` and `clear_down` once over it, with `pair()` and `points()`
  passed through so `crates/modem` need not reach inside.
- The hand-over branch chooses by which INFO1a went: Table 18 builds the V.92 modem;
  Table 10 or Table 19 builds today's V.90 modem. `v90::analogue::Settings::new` learns to
  take `Option<&Info1c>`, because short Phase 2 leaves no probe; without one it uses the
  INFO1a symbol rate, the low carrier and pre-emphasis 0.
- Per-version failure counters: after `V92_RETRAINS` failed PCM-upstream start-ups the
  analogue modem asks for V.34 upstream while keeping PCM downstream; after
  `V90_RETRAINS` more it declines PCM entirely and the call continues as V.34.
- `Digital` mirrors it, choosing the V.92 or V.90 modem from Phase 2's outcome.
- `Status` gains the reasons the new failures need.

**Tests:** in `v92_startup.rs`, with a `FullCall` in the shape of `v90_call.rs`'s:
- `a_whole_v92_start_up_connects_with_pcm_both_ways` - V.8, full Phase 2, Phase 3, Phase 4,
  data; `is_v92()` and `upstream_pcm()` true.
- `a_v92_analogue_modem_meets_a_v90_server_on_v90` - INFO0d bit 27 clear; the call is V.90
  with V.34 upstream, and every existing V.90 test passes unchanged.
- `a_v90_server_that_happens_to_set_bit_70_does_not_get_pcm_upstream` - bit 70 means the
  3429 carrier to a V.90 modem.
- `a_server_that_says_the_channel_will_not_carry_pcm_upstream_gets_v34_upstream` - bit 70
  clear; the downstream is still PCM.
- `an_upstream_that_cannot_carry_the_slowest_rate_falls_back_one_rung_at_a_time` - upstream
  noise only; one step to V.34 upstream, then data crosses; no retrain loop.
- `a_retrain_from_either_end_comes_back_up_as_v92` - always through full Phase 2.
- `a_call_that_fails_twice_on_pcm_upstream_ends_as_v34_upstream_and_then_as_v34`.

**Digests:** `code-analogue.md` (11.2, 11.9), `spec-phase2-procedures.md` (7, 11.4),
`code-tests.md` (8.2).

---

### Wave 10: robustness, and the parts quick connect needs

#### WP-31: Upstream robustness: echo cancellation and slip re-framing (M)

**Clauses:** 1 b (channel separation by echo cancellation); 8.5.7 and 9.5.1.1.10 (the
frame count the slips break).

**Edits:** `crates/datapump/src/v92/receiver.rs`, `crates/datapump/src/v92/decoder.rs`.
**Depends on:** WP-25, WP-29.

**Does:** the two things a real upstream needs and a clean simulation does not.

- **Echo.** `dsp::EchoCanceller` at 8 kHz, fed our own downstream codeword as the
  transmitted signal and the A/D sample as the received one: a near run of 32 to 64 taps
  for the codec, filters and hybrid, and a far run placed by `EchoFinder` inside the
  measured round trip for a VoIP-length reflection. It adapts in the window where the
  analogue modem is silent (from the end of Ja to Su) and then holds, adapting slowly on
  the decision-directed residual in data mode. The residual must stay well under half the
  upstream level spacing, because the echo is added **before** quantising and cannot be
  undone afterwards.
- **Slips.** A 160-codeword slip moves the 12-symbol frame by 4 and an 80-codeword cut by
  8. The receiver gains the upstream equivalent of `find_place`: frames the modulus encoder
  could not have produced, three in the last 24, trigger a search over the 12 shifts, and
  the winner re-bases the interval count, the constellation index and the trellis phase.
  Estimates use medians, not means.

**Tests:**
- `the_hybrid_echo_of_the_downstream_is_cancelled_from_the_upstream` - at 15 dB and 25 dB,
  the residual is below the stated fraction of the spacing.
- `a_far_echo_a_round_trip_late_is_found_and_cancelled`.
- `an_upstream_slip_is_found_and_the_twelve_symbol_frame_moved_by_four` - and by eight for
  the 80-codeword cut.
- `a_slip_in_data_mode_costs_a_burst_and_not_the_call` - data after the slip arrives.

**Digests:** `code-digital.md` (7.5, 7.4), `code-tests.md` (10).

---

#### WP-32: Short Phase 1's PCM signals: ANSpcm, QTS and TONEq (M)

**Clauses:** 8.3.1 (ANSpcm, Tables 6 to 10), 8.3.6 (QTS and QTS-backslash), 8.2.5 (TONEq).

**Creates:** `crates/datapump/src/v92/short1.rs`.
**Depends on:** WP-01.

**Does:** the signals the digital modem must generate exactly, and the analogue modem must
recognise, before Phase 2 of a quick connect.

- The four ANSpcm tables as constant data (301 octets each, in both laws), with the
  generator of 8.3.1 kept as a unit test rather than as the source: `floor(scl*sqrt(2)*
  cos(2*pi*k*79/301 + theta) + 0.5)` quantised with the exact rule that reproduces all
  2408 tabled octets, ties going to the louder interval.
- The polarity reversal as `octet ^ 0x80` every 3612 symbols, which falls on a period and a
  frame boundary.
- QTS as 128 repeats of `{+V, +0, +V, -V, -0, -V}` and QTS-backslash as 8 inverted repeats,
  with the first QTS symbol in data frame interval 0 and the frame count carried from there
  for the rest of the call.
- A 980 Hz TONEq generator and a detector that will not fire on V.21(L) marking, which
  needs about 60 ms of steady 980 Hz with no 1180 Hz energy.
- An ANSpcm detector that distinguishes it from ANSam by the absence of the 15 Hz AM, and
  from V.25 ANS by the QTS burst that precedes it.

**Tests:**
- `the_anspcm_tables_are_the_ones_the_recommendation_prints` - the generator reproduces all
  2408 octets, including Table 7 k = 82 A-law, which the page prints as "8" and means 08.
- `a_reversal_is_a_polarity_flip_and_lands_on_a_frame_boundary`.
- `qts_puts_its_first_symbol_in_interval_zero_and_reverses_at_symbol_768`.
- `the_qts_reversal_is_the_timing_mark_the_analogue_modem_needs` - found within a symbol
  through `Network`.
- `toneq_is_not_reported_for_v21_marking` - the 14-bit mark run inside a QC1a with
  WXYZ = 1111 does not trigger it.
- `anspcm_is_told_from_ansam_and_from_ans`.

**Digests:** `spec-phase1-signals-digital.md` (3, 8, Appendix A),
`spec-phase1-signals-analogue.md` (7, 10.3, 10.4, 11).

---

#### WP-33: The V.8 quick-connect sequence codec (S)

**Clauses:** 8.2.1 (QC1a, Table 2), 8.2.3 (QCA1a, Table 4), 8.3.2 (QC1d, Table 11), 8.3.4
(QCA1d, Table 13), and the clause 8 bit-order rule.

**Creates:** `crates/v8/src/quick.rs`.
**Edits:** `crates/v8/src/lib.rs` (one `pub mod quick;`).

**Does:** the four V.8-framed quick-connect sequences as data, with no line behaviour.
`SYNC_QC = 0x55`; `Kind { Qc1a, Qca1a, Qc1d, Qca1d }`; `Uqts` covering Ucodes 61 to 87 and
the cleardown code `1111`; `AnspcmLevel`; `Qc { kind, lapm, uqts, level }` with `octet()`,
`from_octet()` and `bits()` (60 bits for QC, 70 for QCA, the ten-ONE runs included). A
`Watcher` that accepts `[0x55, q, 0x55, q]` with both copies equal, kept separate from
`v8::Decoder` because the decoder's sync logic would misread some QC octets.

The V.8 bis sequences (QC2a, QC2d, QCA2a, QCA2d) are out of scope; see section 7.

**Tests:**
- one test per table row, against the worked octets in the digest: QC1a P=1 WXYZ=0101 is
  0xA4, QCA1a is 0xA6, QC1d P=1 LM=01 is 0x85, QCA1d is 0x87;
- `a_qc_body_octet_that_looks_like_a_v8_sync_is_still_read_correctly` - the 0xE0 and 0x00
  cases;
- `the_cleardown_code_is_not_a_ucode` - WXYZ = 1111 round trips as `Uqts::Cleardown`;
- `both_copies_must_agree` - one corrupted copy is refused.

**Digests:** `spec-phase1-signals-analogue.md` (5, 12.3), `spec-phase1-signals-digital.md`
(4, 6), `code-v8-and-call.md` (6.2.1, 8 R1).

---

#### WP-34: V.42: bypassing ODP/ADP at both ends, and suspend and resume (S)

**Clauses:** 9.2.5, 9.3.1; V.42 7.2.1, 7.10, 7.11.

**Edits:** `crates/ec/src/stack.rs`, `crates/ec/src/detect.rs`.

**Does:** two small things V.92 needs from the error-control layer.

- An **answerer-side** bypass beside today's originator-side `without_detection()`: do not
  wait for an ODP, do not send an ADP, and take flags or an LAPM frame as the start of the
  protocol phase. V.8 itself warns that some equipment declares LAPM and still needs
  ODP/ADP, so a bypassing modem must still tolerate an ODP that arrives anyway.
- `suspend()` and `resume()`, freezing T400, or T401 and T402/T403, so a modem-on-hold
  transaction or a long retrain does not tear the link down. The link is not released, so
  sequence numbers, windows and the V.42 bis and V.44 dictionaries survive.

**Tests:**
- `an_answerer_told_to_skip_detection_starts_lapm_on_the_first_flag`.
- `an_odp_that_arrives_anyway_is_answered` - the bypass does not make the stack rude.
- `a_suspended_stack_does_not_time_out` - T401 does not fire across a 60 s freeze, and the
  link carries data again after `resume()`.
- The existing `ec` loopback tests pass unchanged.

**Digests:** `spec-control-plane.md` (6.2, 6.3), `spec-phase1-procedures.md` (5.5),
`code-v8-and-call.md` (6.5).

---
### Wave 11: impairments, short Phase 2, rate renegotiation

#### WP-35: The impairment suite for the V.92 call (M)

**Clauses:** none new. This is where the Recommendation meets the rig.

**Edits:** `crates/datapump/tests/v92_call.rs`.
**Depends on:** WP-25, WP-29, WP-31.

**Does:** turns everything WP-25 and WP-31 can do into a table of connect-or-fall-back
expectations, so that a later change that quietly loses upstream PCM is caught.

**Tests:**
- `robbed_bits_both_ways_connect_and_carry_data` - robbed downstream phase 2, upstream
  phase 5; the per-interval constellations absorb it.
- `a_digital_pad_either_way_connects` - (3, 0), (0, 3) and (6, 6) dB.
- `the_hybrid_echo_is_cancelled_both_ways` - 15 dB and 25 dB; the upstream rate stays
  within one rung of the echo-free rate.
- `a_transcoding_gateway_leaves_pcm_upstream_out` - mu to A with the Crazytel response; the
  call settles on V.34 upstream with about 32 000 downstream and no retrain loop.
- `downstream_slips_during_the_start_up_are_followed` and
  `upstream_slips_during_the_start_up_are_followed` - the five period-and-length pairs the
  V.90 tests already use, in each direction.
- `an_upstream_slip_in_data_mode_is_followed_and_data_after_it_arrives`.
- `a_softphone_gain_control_on_what_we_send_is_stayed_under` - ceiling 0.3; LU and the
  prefilter peaks are bounded; and at ceiling 0.1 the call falls back once, not in a loop.
- `a_softphone_that_passes_our_samples_straight_through_gives_pcm_upstream` - both 1-in-2
  phases, one of them needing epsilon = 0.5 T.
- `a_softphone_that_resamples_what_we_send_gives_v34_upstream`.
- `a_slip_anywhere_in_trn1u_is_found` - `#[ignore]`d sweep, reporting the failure positions
  the way the V.90 DIL sweep does.

**Digests:** `code-tests.md` (9.3, 10), `code-analogue.md` (13).

---

#### WP-36: Short Phase 2, both ends (M)

**Clauses:** 9.4, 9.4.1.1.1 to 9.4.1.1.5, 9.4.1.2.1 to 9.4.1.2.3, 9.4.2.1.1 to 9.4.2.1.4,
9.4.2.2.1, 9.4.2.2.2.

**Edits:** `crates/datapump/src/v34/phase2.rs`.
**Depends on:** WP-28.

**Does:** the ranging-only Phase 2, as a `Short` flavour of the existing machine rather
than a new module: the INFO0 exchange and its bit-28 recovery are the same code.

The roles reverse. The **digital** modem reverses Tone B first, after Tone A has been
detected and B has run for at least 50 ms, keeps B-bar for 10 ms and goes silent; the
**analogue** modem answers with a Tone A reversal timed so that 40 +/- 1 ms passes at the
line terminals, keeps A-bar for 10 ms and sends INFO1a straight after. There is no
probing, no INFO1d and no second reversal, so only RTDEd exists.

The three recovery timers are flat 2500 ms with no RTD term, which caps the round trip at
about 2.3 s; above that the modems fall into full Phase 2 automatically (the digital side
re-enters at FD-3, the analogue side through the 9.7.2.1 retrain). The analogue modem only
sets its request bit when it means to connect in PCM upstream or V.90 data mode, and, as a
local policy, only when it holds remembered line data for this connection.

**Tests:**
- `both_ends_asking_for_it_run_short_phase_2` - all four bits; RTDEd equals the simulated
  round trip within a millisecond.
- `only_one_end_asking_gives_full_phase_2`.
- `the_analogue_modem_answers_forty_milliseconds_after_the_b_reversal` - measured at the
  line terminals, within the 1 ms tolerance.
- `info1a_follows_the_ten_milliseconds_of_a_bar_without_a_gap` - and the digital modem
  time-stamps the A-bar edge, not one of INFO1a's fill reversals.
- `a_lost_b_reversal_retrains_the_analogue_modem` and
  `a_lost_a_reversal_drops_the_digital_modem_into_full_phase_2`.
- `a_round_trip_sweep_falls_back_cleanly_above_two_point_three_seconds` - 0 to 2.6 s.
- Every existing full-Phase-2 test passes unchanged.

**Digests:** `spec-phase2-procedures.md` (6, 8, 11.2, 11.3), `spec-phase2-signals.md` (11.2).

---

#### WP-37: Rate renegotiation, both ends, with its call tests (M)

**Clauses:** 9.8, 9.8.1.1.1 to 9.8.1.1.5, 9.8.1.2.1 to 9.8.1.2.3, 9.8.2.1.1 to 9.8.2.1.6,
9.8.2.2.1 to 9.8.2.2.3, 8.8.4 (Rd, Rt), 8.5.5 (Ru).

**Creates:** `crates/datapump/tests/v92_reneg.rs`.
**Edits:** `crates/datapump/src/v92/analogue.rs`, `crates/datapump/src/v92/digital.rs`.
**Depends on:** WP-19, WP-29.

**Does:** the V.92 flavour of what V.90 already does, reusing `Exchange`.

Either end may start one, on a data-frame boundary. The initiator turns circuit 106 off and
sends its R signal, Rd or Ru, for 384T then its bar for 24T; the responder clamps 104,
waits for the transition, and answers with its own R on **its** next frame boundary. Then
TRN2d up to 16008T and TRN2u up to 16008T (stopping early on an SUVd, though on a long line
it is better to keep going), then SUVu and SUVd and the `Exchange`.

The silent period is the new part: either end may ask with SUV bit 32; the two agree with
bit 33; then Ed, then Ucode-0 silence with frame alignment kept; then Rt, R-bar-t and SUVd,
and the exchange again. Both directions keep data-frame synchronisation throughout.

Since 9.8 gives no timeout at all, a local watchdog of 5 s + 2 RTD plus the silence
allowance retrains, as V.90's `RENEGOTIATION_ED` already does.

**Tests:** in `v92_reneg.rs`:
- `a_rate_renegotiation_from_either_end_settles_the_rates_asked_for` - at 0.02 s and 0.6 s
  each way; data before and after; no retrain.
- `the_frame_count_is_kept_across_the_whole_procedure` - both directions, every sequence a
  multiple of its frame.
- `a_silent_period_asked_for_by_either_end_is_granted_and_ended` - Figures 16, 17 and 18.
- `both_ends_asking_for_silence_at_once_converges`.
- `an_rt_that_arrives_after_the_analogue_modem_has_already_ended_the_silence_is_accepted` -
  the long-line case where 8004T is shorter than one round trip.
- `two_renegotiations_started_at_once_converge`.
- `a_stalled_renegotiation_retrains_rather_than_hanging`.

**Digests:** `spec-renegotiation-fpe.md` (2, 3, 7, 9, 10.2), `spec-phase4-procedures.md`
(2.3).

---

### Wave 12: quick connect on the line, fast parameter exchange, the AT commands

#### WP-38: Short Phase 1 in the V.8 pump and the PCM start-up wrappers (M)

**Clauses:** 9.2, 9.2.1.1, 9.2.1.3, 9.2.2.1, 9.2.2.3, 9.2.3.1, 9.2.3.3, 9.2.4.1, 9.2.4.3,
9.2.5; 8.2, 8.3.

**Creates:** `crates/datapump/tests/v92_quick.rs`.
**Edits:** `crates/datapump/src/v8.rs`, `crates/datapump/src/v90/startup.rs`,
`crates/datapump/src/v90/server.rs`.
**Depends on:** WP-30, WP-32, WP-33.

**Does:** the ANSam route of short Phase 1, end to end, in the four roles.

- `datapump::v8::Modem` gains `offering_quick(qc)` and `answering_quick(level)`, a raw-bit
  queue for the QC frames (QC1a has ten ONEs between two framed octets), and the ability to
  **abandon the current octet** when a QCA arrives, which V.8 never needed
  (`Bell103Tx::abandon`).
- New states: the caller goes silent after QCA1d and waits for QTS, QTS-backslash and
  ANSpcm; the answerer sends QCA1d and then hands over to the digital pump.
- QTS, QTS-backslash and ANSpcm are generated **inside the digital pump** (WP-32), not in
  `datapump::v8`, because they must be exact codewords at 8 kHz with the frame count
  starting at the first QTS symbol and carrying into Phase 2.
- `startup::Analogue::after_quick_connect` and `startup::Digital::after_quick_connect`, and
  a `Progress::BackToV8` path for every place 9.2 falls back: no TONEq within 2 s, ANSam
  returning, JM arriving.
- The analogue caller uses the permitted early TONEq (as soon as ANSpcm is detected, given
  ANSam was already detected for a second), because on a 1.5 s round trip the full second
  of ANSpcm would miss the far end's 2 s deadline.
- 9.2.5: both P bits set is surfaced so the error-control layer can bypass ODP/ADP (WP-34).

**Tests:** in `v92_quick.rs`:
- `an_analogue_caller_and_a_digital_answerer_quick_connect_within_four_seconds` - and
  reach Phase 2 with the right U_QTS and LAPM flags.
- `a_digital_caller_and_an_analogue_answerer_quick_connect`.
- `cm_is_cut_mid_octet_when_a_qca_arrives_and_no_cj_is_sent`.
- `a_quick_connect_that_is_not_answered_falls_back_to_v8_and_still_connects` - the 2 s
  TONEq timeout at the answerer, ANSam returning, and the caller completing V.8.
- `the_caller_does_not_read_anspcm_as_v25_ans`.
- `a_slip_that_wrecks_the_only_qca1d_still_ends_in_a_v8_call`.
- `a_quick_connect_is_at_least_a_second_faster_than_the_full_v8_exchange`.

**Digests:** `spec-phase1-procedures.md` (5, 6, 10), `spec-phase1-signals-digital.md` (10),
`code-v8-and-call.md` (6.2, 8 R2 to R8).

---

#### WP-39: Fast parameter exchange, both ends, with its call tests (M)

**Clauses:** 9.9, 9.9.1.1.1, 9.9.1.1.2, 9.9.1.2.1 to 9.9.1.2.3, 9.9.2.1.1, 9.9.2.1.2,
9.9.2.2.1 to 9.9.2.2.3, 8.7.4 (RM), 8.7.7 (FB1u), 8.8.4 (Rf).

**Edits:** `crates/datapump/src/v92/analogue.rs`, `crates/datapump/src/v92/digital.rs`,
`crates/datapump/tests/v92_reneg.rs`.
**Depends on:** WP-37.

**Does:** the renegotiation with no training in it. The digital modem sends Rf for 384T and
R-bar-f for 24T (a 12-symbol pattern with a 4-symbol sign period, told from Rd's 6-symbol
one in either polarity); the analogue modem sends RM and RM', which go **through** the
precoder, prefilter and trellis with their Ki forced, unlike Ru which bypasses them. Both
then zero the scrambler and the differential encoder (and the digital modem its shaper) and
send SUV in the **old data-mode modulation**, straight into `Exchange`. The analogue modem
answers with E2u, then FB1u on the old parameters, then B1u on the new ones.

Rate renegotiation takes precedence: an FPE initiator that hears Ru or Rd becomes the
renegotiation responder.

**Tests:**
- `a_fast_parameter_exchange_from_either_end_changes_the_rate_without_losing_the_frames` -
  at 0.02 s and 0.6 s each way; data before and after; frame synchronisation kept.
- `rm_goes_through_the_precoder_and_ru_does_not` - the line signals differ as expected, and
  each is detected by its own watcher and not the other's.
- `rf_is_told_from_rd_in_either_polarity`.
- `a_renegotiation_wins_over_a_fast_exchange_begun_at_the_same_time`.
- `fb1u_runs_on_the_old_parameters_and_b1u_on_the_new`.
- `an_exchange_during_a_file_transfer_costs_no_data` - through the V.42 stack.

**Digests:** `spec-renegotiation-fpe.md` (2.3, 2.5, 4, 9, 10.2),
`spec-phase4-signals-analogue.md` (3.5, 3.8, 5.3).

---

#### WP-40: The V.250 `+P` commands (M)

**Clauses:** V.250 6.8.1 to 6.8.8 (Tables 31 to 37), 6.4.1 (`+MS`), 6.4.3 (`+MR`),
6.5.1 (`+ES`).

**Edits:** `crates/at/src/lib.rs`, `crates/at/src/parse.rs`, `crates/at/tests/session.rs`.

**Does:** the eight commands V.250 makes mandatory for a V.92 DCE, following the `+DS44`
pattern already in the file. A `V92` settings struct on `Interpreter` with `+PCW`
(0 toggle circuit 125, 1 hang up, 2 ignore), `+PMH` (0 **enables** hold, note the inverted
sense), `+PMHT` (0 denies, 1 to 13 grant with the Table 33 timeouts), `+PIG` (0 enables PCM
upstream), `+PQC` (0 both short phases, 3 neither) and `+PSS` (0 the DCEs decide, 1 force
short, 2 force full); plus the two action commands `+PMHR` and `+PMHF`, which defer their
result because the modem decides between OK, ERROR and a late `+PMHR: <n>`. New
`Action::{SelectV92, RequestHold, HookFlash}`, handled in both exhaustive matches
(`modem::run_actions` and `gui::app::perform`).

While here: `+MS` accepts `V92`, and its test reply stops advertising `(300-4800)`, which
cannot describe V.34, V.90 or V.92; the `<min_rx_rate>` and `<max_rx_rate>` subparameters
are parsed rather than dropped, because V.92 is asymmetric. `+PMHT`'s default is 0 (deny)
until hold is implemented, and `+GCAP` lists the `+P` commands only once the modem acts on
them.

**Tests:** in `session.rs`, in the file's style:
- `the_p_commands_read_back_what_they_were_set_to`.
- `values_outside_the_v250_tables_are_refused` - and every accepted value is one that `=?`
  advertises.
- `pmhr_and_pmhf_parse_as_actions_and_answer_a_test`.
- `ms_v92_is_accepted_and_moves_to_the_offered_list`.
- `ms_test_advertises_rates_v92_can_actually_reach`.
- `and_f_puts_the_p_settings_back`.

**Digests:** `spec-control-plane.md` (5.1, 5.2, 5.3), `code-v8-and-call.md` (6.4, 8 R13,
R15, R21).

---

### Wave 13: cleardown, and V.92 from the AT command line

#### WP-41: Cleardown, both ends (S)

**Clauses:** 9.11.

**Edits:** `crates/datapump/src/v92/analogue.rs`, `crates/datapump/src/v92/digital.rs`,
`crates/datapump/tests/v92_reneg.rs`.
**Depends on:** WP-39.

**Does:** a connection is ended by starting a rate renegotiation or a fast parameter
exchange and sending a rate sequence with `drn = 0`: CPu or CPus bits 21:25 from the
analogue modem, CPd bits 22:26 from the digital modem. 9.11 says "SUVu or SUVd", which have
no drn field; that is an editorial slip (section 3). The fast exchange is the quicker route
and needs no training. On receiving `drn = 0` a modem completes the sequence in progress,
ignores the constellation fields and drops the line.

**Tests:**
- `a_cleardown_from_either_end_ends_the_call_at_both` - through both a renegotiation and a
  fast exchange.
- `the_constellation_fields_of_a_cleardown_rate_sequence_are_ignored`.
- `a_cleardown_is_reported_as_a_cleardown_and_not_as_a_failure`.

**Digests:** `spec-modem-on-hold.md` (3), `spec-renegotiation-fpe.md` (6.2).

---

#### WP-42: `+MS=V92` from the command line to CONNECT (M)

**Clauses:** V.250 6.4.1, 6.4.3; V.92 1 j (recognised connections), 9.2, 9.4.

**Creates:** `crates/modem/tests/v92_call.rs`.
**Edits:** `crates/modem/src/lib.rs`.
**Depends on:** WP-30, WP-36, WP-38, WP-40.

**Does:** wires V.92 through the call-control layer without adding a `Pump` variant, which
would touch about twenty exhaustive matches. `Pump::V90` and `Pump::V90Server` carry the
V.92 options; `standard()` reports "V.92"; `offered()` and `place_call()` accept `"V92"`;
the fallback chain becomes `"V34" | "V90" | "V92" => "V32B"`; `transmit_rate()` reports the
upstream PCM rate; `+MCR: V92` and `+MRR: <tx>,<rx>` are emitted and re-emitted after a
renegotiation.

It also holds the **recognised-connection memo** that quick connect spends: U_QTS, the
round-trip delay, the law, and the Phase 2 and Phase 3 facts short Phase 2 needs, stored by
dial string where there is one and in a single last-call slot where there is not. `+PQC`
and `+PSS` decide whether the memo is used; `+PSS = 1` forces a short start-up where `+PQC`
allows it, `+PSS = 2` forces a full one.

**Tests:** in `modem/tests/v92_call.rs`, copying `v90_call.rs`'s `Pair` with a caller-side
softphone model:
- `dialling_a_v92_server_connects_with_pcm_both_ways_and_carries_text` - `+MS=V92`;
  CONNECT; text both ways over V.42; a server retrain; text again.
- `two_of_these_connect_when_both_softphones_pass_codewords` - the 2 x 2 phase matrix; the
  host phase that resamples still ends on V.34, as the V.90 test shows.
- `a_second_call_with_the_memo_connects_faster` - fewer V.21 seconds and fewer Phase 2
  seconds than the first.
- `ms_v92_is_offered_and_reported` - `+MS?`, the CONNECT text, the `distant()` rows.
- `when_one_end_hangs_up_the_other_notices` - gracefully, through the cleardown of WP-41.

**Digests:** `code-v8-and-call.md` (6.1, 6.2.4, 6.6), `spec-control-plane.md` (5.3).

---

### Wave 14: modem-on-hold on the line, and the GUI carrier

#### WP-43: Modem-on-hold in the data pumps (L)

**Clauses:** 8.9.1 (Tone RT), 8.9.2 (MH sequences, Tables 32 and 33), 9.10, 9.10.1.1,
9.10.1.2 (Table 34), 9.10.2.1, 9.10.2.2, 9.10.2.3.

**Creates:** `crates/datapump/src/v92/hold.rs`, `crates/datapump/tests/v92_hold.rs`.
**Edits:** `crates/datapump/src/v92/analogue.rs`, `crates/datapump/src/v92/digital.rs`.
**Depends on:** WP-05, WP-29.

**Does:** the transaction, over the Phase 2 DPSK modem that already exists: RT is Tone A
for the analogue modem and Tone B for the digital one, and MH sequences are 40-bit INFO
frames on the same carriers.

Responding is **mandatory for every V.92 modem** even if it never initiates: MHreq gets
MHack or MHnack; MHnack gets MHcda or MHfrr; MHclrd gets MHcda; MHfrr gets ANSam.
Initiating needs circuit 107 asserted and either the far RT or an MH response detected.
The held modem sends ANSam for T1 and watches for Phase 1 signals, and disconnects on a QC
with U_QTS = 1111 or on a zero-modulation CM. Every sequence that is started is completed
before anything else goes out, which the 80 ms MHack-to-ANSam window must allow for.

The hard part is that a hold transaction and a retrain start identically, so the tone
detectors must run an INFO framer beside the reversal detector and let a validated MH frame
win: an MH sequence opens with four ONEs, which in DPSK is four phase reversals in a row.

**Tests:** in `v92_hold.rs`, two of our modems over `Network`:
- `mhreq_is_answered_with_mhack_and_its_hold_time` - and MHnack when `+PMHT` denies.
- `the_held_modem_sends_ansam_within_eighty_milliseconds_of_the_last_mhack`.
- `a_call_put_on_hold_is_held_and_resumed` - at 0.6 s each way; the carrier watch does not
  report the far end gone.
- `a_hold_that_runs_out_ends_the_call` - timed from the end of the first MHack.
- `a_retrain_is_still_a_retrain` - a single reversal then steady tone is not read as MH.
- `a_hold_request_wins_over_a_retrain_response` - the race of 9.10.1.1 R5.
- `mhclrd_is_answered_with_mhcda_and_both_ends_disconnect`.
- `mhfrr_brings_ansam_back_and_phase_1_starts_again`.
- `a_slip_that_loses_one_mh_frame_is_not_two_hundred_milliseconds_of_absence`.

**Digests:** `spec-modem-on-hold.md` (1, 2, 5, 8, 9.3), `spec-phase4-procedures.md` (7.6).

---

#### WP-44: The GUI: the V.92 carrier, call settings and status (M)

**Clauses:** none. Presentation of V.250 6.8 and V.92 9.2/9.4.

**Edits:** `crates/gui/src/app.rs`.
**Depends on:** WP-42.

**Does:** appends `("V92", ...)` to `CARRIERS` at index 6 (appending, never inserting,
because `CEILINGS` and `rates` are index-based and pinned by tests), adds the V.92 rate
list and ceiling, and adds two checkboxes to the call settings that type `AT+PQC=` and
`AT+PMH=`, in keeping with the file's rule that GUI controls type AT commands. The status
grid gains a start-up row (quick or full, short Phase 2 or not) and a hold row. The Retrain
button is enabled for V.92 as well as V.34 and V.90, which it should already have been.

**Tests:** in the file's existing style:
- `the_rate_lists_belong_to_the_carriers_they_are_indexed_by` - updated for the new length.
- `v92_can_do_everything_v90_can`.
- `what_the_last_run_was_set_to_comes_back` - the new remembered keys round trip.

**Digests:** `code-v8-and-call.md` (6.7, 8 R14), `spec-control-plane.md` (5.1).

---

### Wave 15: hold, from the command line

#### WP-45: Hold call control in the modem crate (L)

**Clauses:** V.250 6.8.2, 6.8.3, 6.8.4, 6.8.6, 6.3.6 (`H`), 6.3.7 (`O`); V.92 9.10.2.1,
9.10.2.3.

**Creates:** `crates/modem/src/hold.rs`, `crates/modem/tests/v92_hold.rs`.
**Edits:** `crates/modem/src/lib.rs`.
**Depends on:** WP-34, WP-40, WP-43.

**Does:** the DTE's view. A `Hold { Asking, Away, Holding }` state beside the V.250 states;
`+PMHR` initiates and answers late with `+PMHR: <n>`; `+PMHT` decides what an incoming
MHreq gets; `ATO` resumes as a **retrain**, not as a new call, so the terminal sees no
second CONNECT and the V.42 stack, its sequence numbers and its dictionaries survive;
`ATH` while held ends the call without going on-hook.

Three existing guards have to learn about hold or a granted hold ends the call at once:
carrier loss in `carry_data`, the V.42 clocks in `tick` (frozen through WP-34's
`suspend()`), and V.8's 5 s ANSam and 60 s patience limits. `+PMHF` cannot really flash the
hook, because the softphone owns the line and there is no DAA; it answers OK while on hold,
logs, and says so in its help text.

**Tests:** in `modem/tests/v92_hold.rs`, two whole modems:
- `at_pmhr_asks_and_the_answer_comes_back_as_a_timer` - `+PMHR: 5` for a one-minute grant.
- `pmht_zero_gives_pmhr_zero`.
- `ato_resumes_and_text_still_crosses_on_the_same_stack` - the V.42 frame counters continue,
  and no second CONNECT is emitted.
- `a_held_call_does_not_time_out_while_it_is_held` - T401 frozen; no NO CARRIER.
- `ath_on_hold_gives_the_held_side_no_carrier_within_two_seconds`.
- `a_hold_across_a_one_and_a_half_second_round_trip_still_resumes`.

**Digests:** `spec-modem-on-hold.md` (2, 7, 9.3), `code-v8-and-call.md` (6.3, 8 R6, R9, R18).

---

### Wave 16: the scope and the hold controls

#### WP-46: The upstream PCM scope, and the GUI hold controls (M)

**Clauses:** none. Presentation.

**Edits:** `crates/modem/src/lib.rs`, `crates/gui/src/app.rs`, `crates/gui/src/live.rs`,
`crates/telemetry/src/lib.rs`.
**Depends on:** WP-43, WP-44, WP-45.

**Does:** the display for PCM upstream at the digital modem, using the consecutive-sample
pair plot Rory chose for the PCM panel: the digital modem publishes the A/D codeword pairs
the way the analogue modem already publishes the downstream ones, labelled so the panel can
tell the two apart. `CallState::OnHold` joins the telemetry states with the label "on
hold", `Leds.oh` staying true. The GUI gains Hold, Resume, Flash and a simulated
call-waiting button, each typing its AT command (`AT+PMHR`, `ATO`, `AT+PMHF`), and logs
hold transitions the way it logs retrains. The headless answering server gains
`--grant-hold <0..13>`, which types `AT+PMHT=<n>`.

**Tests:**
- `the_upstream_pair_plot_holds_the_codewords_the_receiver_decided` - a unit test on the
  published frame.
- `on_hold_is_a_call_state_with_its_own_label`.
- `the_hold_buttons_are_enabled_only_where_they_can_work` - the GUI state test.
- `the_answering_server_grants_a_hold_when_asked_to`.

**Digests:** `code-v8-and-call.md` (6.7), memory note `v90-pcm-display-choice`.

---
## 5. Waves

Work packages in a wave never edit the same file, so each can be taken in its own git
worktree and merged without conflict. A wave ends when all of its packages are merged and
the workspace is green.

| Wave | Work packages | What the wave delivers |
|---|---|---|
| 1 | WP-01, WP-02, WP-03, WP-04, WP-05, WP-06, WP-07, WP-08 | The numbers, the modulus encoder, the precoder chain, the V.90 openings, the INFO layouts, the shared downstream reading, the network's A/D phase, the symbol clock |
| 2 | WP-09, WP-10, WP-11, WP-12, WP-13, WP-14 | Every codec and signal generator the upstream needs, and the CPd design |
| 3 | WP-15, WP-16, WP-17 | **The de-risk spike**, the upstream receiver, and what the capture holds |
| 4 | WP-18, WP-19 | The sampling phase (and the spike through the real receiver), and the SUV/CP/E exchange |
| 5 | WP-20, WP-21 | Phase 3 to Jd, both ends |
| 6 | WP-22, WP-23 | Phase 3 to E1u and R-bar-i, both ends |
| 7 | WP-24, WP-25 | **Phase 3 call to call**, and the network's impairments |
| 8 | WP-26, WP-27, WP-28 | Phase 4 and data, both ends, and V.92 in Phase 2 |
| 9 | WP-29, WP-30 | **The first complete V.92 call (M1)** and the whole start-up with its fallback ladder (M2) |
| 10 | WP-31, WP-32, WP-33, WP-34 | Echo and slips upstream; ANSpcm, QTS and TONEq; the QC codec; the V.42 bypass and freeze |
| 11 | WP-35, WP-36, WP-37 | The impairment suite, short Phase 2, rate renegotiation |
| 12 | WP-38, WP-39, WP-40 | Short Phase 1 on the line, fast parameter exchange, the `+P` commands |
| 13 | WP-41, WP-42 | Cleardown, and `+MS=V92` from the command line |
| 14 | WP-43, WP-44 | Modem-on-hold in the pumps, and the GUI carrier |
| 15 | WP-45 | Hold call control |
| 16 | WP-46 | The upstream PCM scope and the hold controls |

Waves 5, 6, 8, 11, 12, 13 and 14 each hold one analogue-side and one digital-side package
that edit different files, which is the pattern that keeps "both ends are ours" from
serialising the work.

### Where the parallelism actually is

- **Wave 1** is eight independent packages; it is the widest point in the plan.
- **Wave 2** is six, and is the other wide point.
- Waves 5 to 9 are narrow by design: each is one step of a call, and the step after it
  cannot be written until the step before it works.
- From wave 10 on the plan widens again: the four remaining features (quick connect,
  renegotiation, hold, the control plane) touch different crates and proceed in parallel.

---

## 6. Keeping V.90 and V.34 working

Four rules, checked in every work package that touches a V.90 or V.34 file:

1. **No existing test is edited.** New tests are added. `v90_call.rs`, `v90_vector.rs`,
   `v90_replay.rs`, `v34_capture.rs`, `v34_vector.rs`, `modem/tests/v90_call.rs` and
   `modem/tests/call.rs` all pass byte for byte.
2. **Additive fields default to today's behaviour.** `Info0d::v92` defaults to false, so a
   V.90 modem still sends bits 26 and 27 as zero. `Pcm::Analogue` without V.92 still sends
   a Table 10 INFO1a.
3. **WP-06 is the only behaviour-preserving move, and it is committed on its own**, with
   the slip tests and the DIL sweep green before and after.
4. **The V.90 modems are the fallback, not dead code.** A V.92 call that cannot hold PCM
   upstream runs `v90::analogue::Modem` for real, so every V.90 test is also a V.92 test of
   the middle rung of the ladder.

The V.92-to-V.90 interop tests are in WP-30, and two of them are just the existing V.90
tests run against a V.92-capable caller.

---

## 7. The fallbacks the Recommendation requires, and where each is tested

| Fallback | Clause | Where |
|---|---|---|
| Either end not V.92: both use V.90's INFO bits | 9.3 | WP-28, WP-30 |
| INFO1d bit 70 clear: no PCM upstream | 8.4.1, Table 18 | WP-28, WP-30 |
| Not all four short-Phase-2 bits: full Phase 2 | 9.4 | WP-36 |
| Short Phase 2 with no A reversal or no INFO1a: full Phase 2 | 9.4.1.2.2, 9.4.1.2.3 | WP-36 |
| Short Phase 2 with no B reversal: retrain into full Phase 2 | 9.4.2.2.2 | WP-36 |
| Short Phase 1 with no QCA: continue with V.8 | 9.2.1.1, 9.2.2.1 | WP-38 |
| Short Phase 1 with no ANSpcm or no TONEq in 2 s: ANSam and V.8 | 9.2.3.3, 9.2.4.3 | WP-38 |
| A V.34-mode INFO1a: V.34 Phase 3 as call or answer modem | 9.2.2.1.9/V.90 | existing V.90 path, WP-30 |
| Every retrain lands in full Phase 2 | 9.7 | WP-30, WP-36 |
| Cleardown by `drn = 0` | 9.11 | WP-41 |
| Modem-on-hold refused with MHnack | 9.10.2.1 | WP-43 |
| A held modem cleared down by QC with U_QTS = 1111, or by a zero CM | 9.10.2.1 | WP-43 |

---

## 8. Out of scope, and why

| Item | Clause | Why |
|---|---|---|
| **The V.8 bis route of short Phase 1**: CRe, QC2a, QC2d, QCA2a, QCA2d, and V.8 bis transactions | 8.2.2, 8.2.4, 8.3.3, 8.3.5, 9.2.1.2, 9.2.2.2, 9.2.3.2, 9.2.4.2 | The route needs a QCA within 1 s of QC2x. On the project's line the round trip alone is 1.1 to 1.6 s, so it can never succeed there, and stretching the timer would not be conforming. No observed server has sent CRe. The ANSam route (WP-33, WP-38) reaches the same place. Revisit only if a live far end is seen sending CRe. |
| **Both modems analogue in short Phase 1**, ending in V.34 Phase 2 | 9.2.1.4, 9.2.3.4, Figures 7 and 8 | It is a "may" for the answerer, and it produces a V.34 call, which the repository already does through ordinary V.8. It costs two extra states in every Phase 1 role for no new capability. |
| **A digital answerer taking the analogue role on QC1d or QC2d** | 9.2.4.1, 9.2.4.2 | A "may". V.8's own rule (the caller becomes the analogue modem when both ends could be either) reaches the same outcome one exchange later. |
| **The digital modem's MD** | Table 17 bits 18:24 | No downstream MD appears anywhere in 9.5 or Figures 10 and 11. We send length 0 and never expect one. |
| **Requesting a zero-length DIL (the SCR path)** | 9.5.1.1.11, 9.5.1.1.13, 8.6.6 | The downstream choice needs a DIL, and `v90::dil::design` already produces a good one. The SCR path is parsed and answered (WP-23) so a far end may use it, but we never ask. |
| **V.80 synchronous access, and V.43 circuit 133** | 7.2, Table 1 note 2 | Neither Recommendation is in `docs/specs`, so neither can be implemented from the text. V.14 and V.42 cover 7.2. |
| **V.59 managed objects behind `+TMO`** | V.250 6.9 | V.59 is not in `docs/specs`. |
| **`+VCID` Caller ID collection for `+PCW=0`** | V.250 6.8.1 | `+VCID` is V.253, which is not in `docs/specs`. `+PCW=0` toggles circuit 125 and logs; the Caller ID half is left out. |
| **A real hook flash for `+PMHF`** | V.250 6.8.6 | The softphone owns the line and there is no DAA. The command answers OK while on hold and logs; anything else would be a lie. |
| **Network call-waiting detection (CAS or SAS tones)** | 9.10 in general | Neither V.92 nor V.250 specifies it, it is network-specific, and the softphone takes the waiting call itself, so nothing reaches the modem. A GUI button stands in for the indication so that the `+PCW` paths can still be exercised. |
| **V.54-style loopback testing** | V.92 clause 10 | Clause 10 says the testing facilities of other V-series Recommendations cannot be used with V.92, and that V.92's own are for further study. There is nothing normative to build. |
| **A 2-wire echo canceller in front of the analogue modem's receiver** | 1 b | The rig is 4-wire (softphone to softphone), and V.90 has run without one. The digital modem's canceller is in scope (WP-31) because it is what makes upstream PCM decisions possible at all. |
| **V.44 changes** | 7.2 | V.92 does not name V.44, and V.44 does not name V.92. The existing XID negotiation is unaffected. |

---

## 9. Risks

**R1. Upstream PCM may never work on the project's own line.** The evidence is already in
the memory notes: about 1.5 s round trip, 20 ms jitter-buffer concealment inserts every few
seconds, a softphone gain ceiling near 0.32 of full scale, and a transcoding path
(Crazytel) that decodes, low-passes and re-encodes. Upstream PCM needs our samples to reach
the far codec unchanged, and every one of those breaks that. *Mitigation:* the whole plan
is proved between our own two modems over a simulated network first (A9); WP-30 builds the
fallback ladder at the same time as the first whole start-up, not later; WP-35 asserts the
transcoding and gain-control cases fall back rather than loop.

**R2. The de-risk spike may say no.** WP-15 could show that a precoder we can design does
not reach a useful upstream rate over a codec channel. *Mitigation:* it is wave 3, before
any state machine exists, and its failure mode is explicit: the plan continues with V.34
upstream as the product and PCM upstream as a branch. Roughly waves 5 to 9 would shrink to
the Phase 2 and hand-over work.

**R3. Readings that only a real V.92 server can settle.** The TRN2u symbol bit order, the
CPd constellation "linear value" scale, whether TRN2u and the SUV/CP sequences are
precoded, and the frame-count origin. Both ends being ours hides all four until we meet
someone else's modem. *Mitigation:* section 3 fixes each as one named constant with both
readings documented and a test at both settings (WP-13), so flipping one after a capture is
a one-line change; WP-17 looks at the capture we already hold.

**R4. WP-06 touches slip-sensitive code that live calls depend on.** The DIL relocation
logic has a known 36-symbol alias weakness and was tuned against real calls.
*Mitigation:* it is a pure move, committed on its own, with the slip tests and the DIL
sweep green before and after. If it goes wrong it is a single revert.

**R5. Timers that the Recommendation wrote for short lines.** TR3 (1500 ms from the start
of Ja, with no RTD term) cannot hold above about 900 ms of round trip. Short Phase 2's
three flat 2500 ms timers cap the round trip at about 2.3 s. The short-Phase-1 2 s TONEq
window is marginal at 1.5 s even with the permitted early TONEq. *Mitigation:* the V.90
`SD_WAIT` relaxation carries over and is documented as a departure (section 3); short
Phase 2 is only requested when the remembered round trip is small enough (WP-36); WP-38
uses the early TONEq; every RTD-bearing deadline is derived from the measured estimate and
tested at 0.6 and 0.75 s each way.

**R6. The analogue modem has no round-trip estimate after a short Phase 2.** Only RTDEd is
defined there, yet the CP repeat rule (100 ms + RTD) and the 20 s + 6 RTD watchdog are the
analogue modem's too. *Mitigation:* use the estimate remembered from the last full
Phase 2, or derive one from the reversal it answered; failing both, use 0 for "must not
exceed" limits and a large value for "wait at least" ones, and log which was used.

**R7. Silent misparses between V.90 and V.92 layouts.** A V.92 Jp reads as a Jd full of
nonsense rates; a V.92 CPt read by `v90::sequences::Cp` gives a shifted drn and can still
produce a plausible `Mapping`; a V.92 INFO0a lands in `Info0.clock`. *Mitigation:* WP-04
adds the bit-47 guard; WP-09's `a_v90_cp_is_not_read_as_a_v92_cp` and WP-05's
`a_v90_peer_reads_a_v92_info0_as_v90` pin the rest; every shared parser takes a version.

**R8. The `+P` commands and `at::Action` are matched exhaustively in two places.** Every
new variant breaks `modem::run_actions` and `gui::app::perform` until handled.
*Mitigation:* WP-40 changes both in the same package.

**R9. CI runs `clippy -D warnings`.** Scaffolding that nothing calls yet fails the build.
*Mitigation:* every package wires its code in or exercises it from `#[cfg(test)]`; no
package lands a module that nothing references.

**R10. Test runtime.** A V.92 start-up is longer than a V.90 one and adds a precoder design
of up to 384 coefficients. *Mitigation:* the budget is in WP-29 (about 1.5 times V.90 per
simulated second); every start-up test stops at its connection; sweeps are `#[ignore]`d;
WP-25 tabulates the network kernels. If the dev profile becomes the bottleneck, propose
`opt-level = 2` for the `datapump` package to the maintainer rather than trimming tests.

**R11. `crates/modem` reaches through `startup::Analogue::v90()` for the scope.** Changing
the field breaks it. *Mitigation:* WP-30 adds `pair()` and `points()` passthroughs in the
same package that changes the field.

**R12. Modem-on-hold and a retrain start identically.** The existing tone watches fire
first, and an MH sequence opens with four phase reversals. *Mitigation:* WP-43 runs the
INFO framer beside the reversal detector and gives a validated MH frame priority, with a
test that a genuine retrain still works.

---

## 10. What to ask of the live rig, batched

These are for Rory, and they are cheap to batch because most need one call each. None
blocks the plan before wave 9.

1. **A transparency check in both directions**, both ends ours, with the second SIP leg the
   test-loop memory says is missing. Does what we send arrive at the far host sample for
   sample, at one phase, with no gain change? That single answer decides whether upstream
   PCM is possible on this rig at all (WP-15's assumptions, R1).
2. **INFO1d bit 70 from a real V.92 server.** It is the server's own verdict on whether our
   path carries PCM upstream. Capture it on the first call to the NetZero or GlobalPOPs
   pool, before anything else (WP-28).
3. **One decoded CPd from a real server.** It settles the constellation "linear value"
   scale, what an absent part means, and the 128-point reading (R3).
4. **One captured TRN2u and the SUVu that follows it.** It settles the symbol bit order
   (R3), because a wrong order means the server never answers our SUVu with a CPd.
5. **`AT+PMHR` against a real V.92 server**, with a recording running. The only live V.92
   far ends available are the ISP pools (WP-43, WP-45).
6. **MicroSIP with AEC, AGC, noise suppression, VAD and comfort noise all off**, confirmed
   on a capture. A softphone AEC on the capture path would subtract a filtered copy of the
   downstream from our upstream, which destroys PCM upstream outright.
7. **Does the V.92 capture we already hold contain a QC1a?** WP-17 answers this from the
   file; if it does, it pins our octet mapping against a real modem.
