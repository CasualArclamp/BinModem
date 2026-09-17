# V.92 phase 3: equalizer and echo canceller training, digital impairment learning

Implementation digest of ITU-T V.92 (11/2000) clause 9.5, covering the signals it uses,
everything it pulls in from V.90 and V.34, and the timers that bound it.
Implementers should be able to work from this file alone.

Source: `docs/specs/T-REC-V.92-200011-I.pdf`. Page numbers below are PDF pages. The printed
page number is the PDF page minus 7.

## Pages read

All values below were taken from rendered page images. The extracted `.txt` was used only to
find things.

| Document | PDF pages viewed as images | What is there |
|---|---|---|
| V.92 | 53, 54, 55, 56 | 9.5 in full, Figures 10 and 11, start of 9.6 |
| V.92 | 27, 28 | Table 18 (INFO1a, PCM upstream), Table 19 (INFO1a, V.34 upstream), 8.5.1 CPt |
| V.92 | 29, 30 | CPt text, 8.5.2 E1u, 8.5.3 MD, 8.5.4 Ja with Table 20 (DIL descriptor), 8.5.5 Ru, 8.5.6 Su, 8.5.7 TRN1u, 8.6 intro, 8.6.1 DIL, 8.6.2 Jd |
| V.92 | 31, 32 | Table 21 (Jd), 8.6.3 Jp with Table 22, 8.6.4 Jp', 8.6.5 Ri, 8.6.6 SCR, 8.6.7 Sd, 8.6.8 TRN1d |
| V.92 | 33, 34, 35 | Table 23 (CPu and CPt), with the gamma/delta rules |
| V.92 | 9 (zoom), 26 (bit 70), 50 (9.3), 51, 59 (9.6.1.2.1), 60 (9.7 retrains) | definitions (Qa.b, LU), INFO1d bit 70, when V.92 phase 3 applies, retrains, the overall start-up timer |
| V.90 | 25, 26, 27, 28, 29 (+ zoom), 30, 31 (CP head), 35, 42 | 8.3.1 Ja and Table 12 (DIL descriptor), 8.4.1 DIL, 8.4.2 Jd, 8.4.3 J'd, 8.4.4 Sd, 8.4.5 TRN1d, Table 14 CP (for comparison), 8.6.4 R/Ri, 9.3 phase 3 (for comparison) |
| V.34 | 16 (zoom), 33, 38 | clause 7 scrambler polynomials, 10.1.2.3.2 CRC with Figure 14, 10.1.3.5 MD |

Text only, used for context and not as a source of values: V.92 pp. 8, 10–13 (clauses 5 and 6,
the analogue transmitter), 36–39 and 43–44 (phase 4 signals), 49–52 (short phase 1 and 2),
57–58 (phase 4 procedures); V.90 pp. 13 (5.3), 36 (8.6.5 TRN2d), 40–44 (phases 2–4).

---

## 0. When this phase runs

* V.92 capability is signalled by INFO0d bit 27 (digital modem) and INFO0a bit 26 (analogue
  modem) (9.3, p.50).
  * If both modems set it, the digital modem sends the V.92 INFO1d (8.4.1). The analogue modem
    **may** then choose PCM upstream by sending an INFO1a laid out as Table 18.
  * If either modem does not set it, both use the V.90 INFO bits (8.2.3.2/V.90) and this clause
    does not apply.
* INFO1d bit 70 = 0 means "channel does not support PCM upstream". The analogue modem **shall
  not** send a Table 18 INFO1a when bit 70 is clear (p.19 text and p.26).
* After INFO1a, the digital modem goes to "the appropriate Phase 3 as signalled in INFO1a"
  (9.4.1.1.5; the analogue side is 9.4.2.1.4).
  * A Table 18 INFO1a (PCM upstream: bits 34:36 = 6 and bits 37:39 = 6) selects **this clause,
    9.5**.
  * A Table 19 INFO1a (V.34 upstream in short phase 2: bits 34:36 = 3..5) presumably selects
    V.90 phase 3 (9.3/V.90). See the open questions.
* If both modems are V.92-capable, every later retrain uses V.92 phase 2 (9.3).
* Full phase 2 is procedurally identical to V.90 phase 2 (9.3). Short phase 2 is 9.4. The
  digital modem's round-trip delay estimate (RTDEd in short phase 2, 9.4.1.1.4) is the "RTD"
  used by the timers below.

---

## 1. Conventions

### 1.1 Symbols, frames and levels

* **T** is one symbol at 8000 symbols/s in both directions: T = 125 µs (6.2; 5.2/V.90).
  * 2040T = 255 ms, 384T = 48 ms, 144T = 18 ms, 48T = 6 ms, 24T = 3 ms, 12T = 1.5 ms.
* **Downstream data frame** (digital to analogue) = 6 symbols, data frame intervals i = 0..5
  (5.4/V.90).
* **Upstream data frame** (analogue to digital) = 12 symbols, i = 0..11 (Figure 1/V.92 on
  p.11; 8.7.1).
* **LU** is the analogue modem's training amplitude. It is set so that TRN1u goes out at the
  desired data-mode transmit power (3.8). All upstream phase 3 levels are expressed in LU.
* **Ucode** is the universal PCM code 0..127 of Table 1/V.90 (3.6).
  * Uchord c (1..8) holds Ucodes 16(c−1) to 16c−1 (3.5/V.90).
  * "+X" and "−X" mean the positive or negative PCM codeword whose Ucode is X.
* **UINFO** is INFO1a bits 25:31, the Ucode used for the downstream two-point signals.
  * It **shall** be greater than 66.
  * Its power **shall not** exceed the digital modem's maximum transmit power (Table 18).
* **Bit order.** In Tables 2–5, 11–24, 27 and 30–33, bit patterns go out leftmost bit first,
  and integers go out least-significant bit first (clause 8 intro, p.13). Every sequence in
  this phase is "bit 0 first".
* **Signed Qa.b** is (a+b+1)-bit two's complement with b fraction bits, range [−2^a, 2^a)
  (3.5).
* **Unsigned Qa.b** is (a+b) bits with b fraction bits. The Recommendation gives its range as
  [0, 2^(a+1)), which contradicts the bit count (see 13.3).
  * In practice: signed Q1.6 is 8 bits, value = int8 / 64.
  * Unsigned Q3.13 is 16 bits, value = uint16 / 8192 (layout "xxx.xxxxxxxxxxxxx").

### 1.2 Scramblers

Both are self-synchronizing scramblers per clause 7/V.34 (p.16).

* **GPC** = 1 + x^−18 + x^−23 (equation 7-1/V.34).
* **GPA** = 1 + x^−5 + x^−23 (equation 7-2/V.34).
* **Transmitter** (divides by the polynomial): `out[n] = in[n] XOR out[n−a] XOR out[n−23]`,
  with a = 18 for GPC and a = 5 for GPA.
* **Receiver** (multiplies back): `data[n] = rx[n] XOR rx[n−a] XOR rx[n−23]`.
  * It needs only the last 23 received bits to synchronize.
  * This is what `crate::v32::Scrambler` already does.

In V.92 the scrambler depends on the side, not on the call/answer role:

* **Analogue modem transmitter: GPA** (6.3/V.92).
* **Digital modem transmitter: GPC** (clause 5/V.92 points to 5.3/V.90). 8.6 repeats this for
  Jd, Jp, Jp', SCR and TRN1d.
* So the digital modem descrambles with GPA, and the analogue modem descrambles with GPC.

"Initialized to zero" means the 23-bit history of past outputs is all zeros.

### 1.3 CRC (10.1.2.3.2/V.34, p.33)

* Polynomial x^16 + x^12 + x^5 + 1.
* The shift register is **loaded with all ones**. The information bits are shifted in, and the
  register is output starting with register bit 0, which is the **LSB** of the 16-bit CRC field.
  Nothing is inverted.
* Figure 14 as drawn:
  * 16 cells, numbered 15 (left) to 0 (right), shifting right.
  * feedback = cell0 XOR input bit.
  * The feedback enters cell 15, and is XORed into the inputs of cells 10 and 3.
* This is `crate::v34::info::crc` (0xFFFF start, reflected 0x8408 form).
* **Coverage:** every information bit of the sequence **except** the frame sync bits, the start
  bits and the fill bits. Reserved bits **are** covered.
* The CRC field sits LSB first in its 16 bit positions.

### 1.4 Framing shared by Jd, Jp, Ja descriptor and CPt

In order:

1. 17 ones of frame sync.
2. A `0` start bit before every 16-bit block of information.
3. A `0` start bit, then the 16-bit CRC.
4. Fill.

`crate::v90::sequences::frame()` builds exactly this.

### 1.5 Sign and bit-to-level mappings

These differ between upstream and downstream.

| Direction | Signals | Bit or sign 0 | Bit or sign 1 | Differentially encoded? |
|---|---|---|---|---|
| Upstream (analogue) | TRN1u | **+LU** | **−LU** | no (scrambled ones) |
| Upstream (analogue) | Ja, CPt, E1u, the 24 ones before CPt | +LU | −LU ("as TRN1u") | yes |
| Downstream (digital) | TRN1d, SCR | **−UINFO** (negative) | **+UINFO** (positive) | no |
| Downstream (digital) | Jd, Jp, Jp' | −UINFO | +UINFO | yes |
| Downstream (digital) | DIL | SP bit 0 = negative | SP bit 1 = positive | no, and not scrambled |

**Differential encoding** (8.5.1, 8.5.4, 8.6.2–8.6.4):

* The bit is first scrambled: s[n] = scrambler(data[n]).
* It is then encoded as d[n] = s[n] XOR d[n−1]. d[n] is the transmitted sign or level bit.
* d[−1] is "the final symbol of the preceding signal", read back as a bit through that
  signal's own mapping.
* The receiver takes s[n] = d[n] XOR d[n−1] and then descrambles. This makes the receiver
  indifferent to line polarity.
* The existing V.90 digital modem already does this for Jd and J'd:
  `sign ^= scrambler.scramble(bit)` in `crates/datapump/src/v90/digital.rs`, confirmed against
  live servers.

---

## 2. Time line (Figures 10 and 11, p.53)

```
Analogue  INFO1a |sil 70±5ms|Ru 384T|R̄u 24T|[MD k·276T|Ru 384T|R̄u 24T]|TRN1u ≥2040T|Ja ........|silence ...
Digital   (silent) ...........................................................  ≤500ms |Sd 384T|S̄d 48T|TRN1d ≥2040T|Jd Jd ...

Analogue  ... silence |Su 144T|S̄u 24.5T|Su ...........|S̄u (24+ε)T|TRN1u (≥2040T if DIL) ...|1×24|CPt|CPt|..|E1u| → phase 4 (TRN2u)
Digital   ... Jd Jd Jd ........... |Jp Jp ... |Jp' 12T|DIL (or SCR) ....................|Ri ..............|R̄i 24T| → phase 4 (TRN2d)
                                          107 ON ^ (digital: before Jp')     107 ON ^ (analogue: on detecting Jp)
```

Causal arrows in Figure 10 (a non-zero DIL was requested):

1. A received Ja descriptor leads to digital Sd, after at most 500 ms.
2. Received S̄d makes the analogue modem end Ja and go silent.
3. Received Jd leads to analogue Su (after an optional wait).
4. The Su after S̄u lets the digital modem finish its phase measurement, and it sends Jp.
5. Received Jp leads to the analogue S̄u of (24+ε)T.
6. That received S̄u makes the digital modem finish Jp and send Jp'.
7. Received CPt leads to digital Ri.
8. Received Ri makes the analogue modem finish its CPt and send E1u.
9. Received E1u leads to digital R̄i.

Figure 11 (zero DIL) is the same drawing with SCR in place of DIL. The text orders the zero-DIL
case differently (see 7 and 13.3).

---

## 3. Signals sent by the analogue modem

### 3.1 Ru and R̄u (8.5.5, p.30)

* **Ru** repeats the 6-symbol pattern {+LU, +LU, +LU, −LU, −LU, −LU}.
* **R̄u** repeats {−LU, −LU, −LU, +LU, +LU, +LU}.
* Durations in phase 3: Ru 384T (64 periods), R̄u 24T (4 periods) (9.5.2.1.1).
* **Shall** bypass the precoder and prefilter while sending Ru and R̄u, and **shall** use "the
  same structure used while transmitting 2 point TRN1u".
  * Read this as: the plain ±LU path, with no precoder or prefilter filtering.
* The receiver finds the Ru-to-R̄u reversal. It is a timing mark only; data frame alignment
  does not start here.

### 3.2 MD (8.5.3, pointing to 10.1.3.5/V.34, p.38)

* An **optional**, manufacturer-defined signal that the sending modem uses to train its echo
  canceller if TRN cannot do it.
* Its length is given in the sender's INFO1 and is 0 if MD is absent. Its content is
  unspecified; the digital modem only has to skip its duration.
* **Length unit in V.92 PCM-upstream INFO1a (Table 18 bits 18:24):** an integer 0..127 in steps
  of **276 symbols (34.5 ms)**.
  * V.90's and V.34's INFO1a, and V.92 Table 19, use steps of 35 ms.
* Maximum MD = 127 × 276T = 35 052T ≈ 4.38 s.
* An MD block (MD, then Ru 384T, then R̄u 24T) is present only when the length is non-zero
  (9.5.2.1.1).

### 3.3 TRN1u (8.5.7, p.30)

* A sequence of ±LU. The signs come from feeding **binary ones** into the GPA scrambler:
  scrambler output 0 = +LU, 1 = −LU.
* The scrambler **shall** be reset to zero before TRN1u.
  * Nothing in the text limits this to the first TRN1u, so apply it to the second TRN1u (sent
    during DIL or SCR) as well. See 13.3.
* Every TRN1u segment **shall** be a whole multiple of 12 symbols.
* **First TRN1u:** at least 2040T (9.5.2.1.2).
* **Second TRN1u** (during DIL or SCR): a multiple of 12 symbols, and at least 2040T when a
  non-zero DIL was requested (9.5.2.1.9 and 9.5.2.1.11).
* **Upstream data-frame alignment:** the digital modem **shall** hold data frame interval
  alignment from the **first symbol of the second TRN1u** (8.5.7). Also 9.5.1.1.10: "modulo 12
  data frame interval count from the first symbol of TRN1u".
  * Every later upstream sequence (the 24 ones, CPt, E1u, TRN2u and so on) is a multiple of 12
    symbols, so upstream interval 0 carries through into phase 4.
  * 8.7.1: B1u starts data frame interval 0.
* First 48 signs of TRN1u with a zero start (derived with GPA, 1 = −LU):
  `111110000011111000001110011111000110000011100100`

### 3.4 Ja and the V.92 DIL descriptor (8.5.4 and Table 20, p.29)

**Content:**

* 24 binary ones, then the DIL descriptor repeated.
* The **last descriptor may be cut short**.
* The total Ja length (the 24 ones included) **shall** be a whole multiple of 12 bits.
* Ja is sent one bit per symbol with TRN1u's modulation (two-point ±LU, 0 = +LU).

**Encoding:**

* Scrambled (GPA) and differentially encoded.
* The differential encoder starts from **the final symbol of the preceding TRN1u**.
* The scrambler is not reset. It runs on from TRN1u; the 24 leading ones let the far
  descrambler lock.
* The analogue modem ends Ja at the **next 12-bit boundary** after it detects the Sd-to-S̄d
  reversal (9.5.2.1.3).

**Bit positions.** α = ⌈LSP/16⌉·17 and β = α + ⌈LTP/16⌉·17 (8.3.1/V.90, with ⌈⌉ = ceiling;
p.25 of V.90). Let **X = β + ⌈N/2⌉·17**.

* Bits 0 to 187+X are exactly Table 12/V.90, up to and including its start bit at 187+X.
* V.92 replaces V.90's CRC and fill from bit 188+X onwards.

| Bits (LSB:MSB) | Field |
|---|---|
| 0:16 | frame sync, 17 ones |
| 17 | start bit 0 |
| 18:25 | **N**, the number of DIL segments, 0..255 |
| 26:33 | reserved, sent as 0 |
| 34 | start bit 0 |
| 35:41 | **LSP − 1** (LSP = 1..128) |
| 42 | reserved 0 |
| 43:49 | **LTP − 1** (LTP = 1..128) |
| 50 | reserved 0 |
| 51 | start bit 0 |
| 52:67 | **SP** bits 1..16. SP bit 0 (LSB, bit 52) is for the first symbol of a segment. |
| 68, … | if LSP > 16: a start bit 0, then the next 16 SP bits, and so on. SP is zero-padded to a multiple of 16. |
| 51+α | start bit 0 |
| 52+α:67+α | **TP** first 16 bits (then continued like SP; zero-padded to a multiple of 16) |
| 51+β | start bit 0 |
| 52+β:58+β | **H1** (7 bits) |
| 59+β | reserved 0 |
| 60+β:66+β | **H2** |
| 67+β | reserved 0 |
| 68+β | start 0 |
| 69+β:75+β | H3 |
| 76+β | rsv 0 |
| 77+β:83+β | H4 |
| 84+β | rsv 0 |
| 85+β | start 0 |
| 86+β:92+β | H5 |
| 93+β | rsv 0 |
| 94+β:100+β | H6 |
| 101+β | rsv 0 |
| 102+β | start 0 |
| 103+β:109+β | H7 |
| 110+β | rsv 0 |
| 111+β:117+β | H8 |
| 118+β | rsv 0 |
| 119+β | start 0 |
| 120+β:126+β | **REF1** (Ucode) |
| 127+β | rsv 0 |
| 128+β:134+β | REF2 |
| 135+β | rsv 0 |
| 136+β | start 0 |
| 137+β:143+β | REF3 |
| 144+β | rsv 0 |
| 145+β:151+β | REF4 |
| 152+β | rsv 0 |
| 153+β | start 0 |
| 154+β:160+β | REF5 |
| 161+β | rsv 0 |
| 162+β:168+β | REF6 |
| 169+β | rsv 0 |
| 170+β | start 0 |
| 171+β:177+β | REF7 |
| 178+β | rsv 0 |
| 179+β:185+β | REF8 |
| 186+β | rsv 0 |
| 187+β+17k | start 0, for pair k = 0 .. ⌈N/2⌉−1 |
| 188+β+17k : 194+β+17k | Ucode of the training symbol of segment 2k+1 |
| 195+β+17k | rsv 0 |
| 196+β+17k : 202+β+17k | Ucode of the training symbol of segment 2k+2 |
| 203+β+17k | rsv 0. If N is odd, the last pair's bits 195..203+β+17k are **9 reserved zero bits**. |
| **187+X** | start bit 0 (the last V.90-defined bit) |
| **188+X : 203+X** | **Upstream rate capability mask, part 1.** Bit 188+X+k set means 24 000 + k·8000/6 bit/s is supported and enabled in the analogue modem's transmitter, for k = 0..15: 24 000, 25 333, … 44 000. |
| **204+X** | start bit 0 |
| **205+X : 220+X** | **Mask, part 2.** 205+X = 45 333, 206+X = 46 666, 207+X = 48 000. Bits 208+X..220+X are reserved and sent as 0. |
| **221+X** | start bit 0 |
| **222+X : 237+X** | **CRC** over all information bits (N, reserved, LSP−1, LTP−1, SP and TP with padding, H, REF, the Ucodes with their reserved bits, both mask blocks) |
| **238+X** | fill bit 0 |
| **239+X …** | fill zeros up to the next multiple of 12 bits of Ja length |

* When N = 0:
  * LSP − 1 = LTP − 1 = 0, so α = 17, β = 34 and X = 34.
  * SP and TP carry no meaning.
  * The descriptor is **276 bits** (the Recommendation states this). That is 273 bits through
    the fill bit, plus 3 zeros, so that 24 + 276 = 300 = 25·12.
  * Our field-by-field layout reproduces this.
* In general the unpadded length is 239 + X bits. It is padded so that each descriptor is a
  multiple of 12 bits, since the 24 leading ones already are.
  * Examples (derived): N=1, LSP=LTP=1 gives 290 → 300. N=125, LSP=LTP=6 gives 1344. N=255,
    LSP=LTP=128 gives 2687 → 2688.

**What the fields mean** (8.4.1/V.90). See 4.6.

### 3.5 Su and S̄u (8.5.6, p.30)

* Let a = √(3/2)·LU.
* **Su** repeats {+a, 0, +a, −a, 0, −a}.
* **S̄u** repeats {−a, 0, −a, +a, 0, +a}.
* Average power equals LU² (4 × 1.5 / 6 = 1).
* Su and S̄u "shall be an integer multiple of 12 symbols in length". Phase 3 nonetheless uses
  S̄u of **24.5T** and **(24+ε)T** (see 13.3).
* Durations and use:
  * Su 144T.
  * Then S̄u 24.5T.
  * Then Su, running until Jp is detected.
  * Then S̄u (24 + ε)T, with ε from Jp.
  * Then the second TRN1u.
  * Precoder and prefilter treatment is not stated for Su. Use the same bypassed path as for Ru.
* Derived, not from the spec: the pattern has x[n+3] = −x[n], so it contains only odd
  harmonics of 8000/6 Hz.
  * About 1/3 of the power is at 1333.3 Hz and 2/3 at 4000 Hz.
  * The 4000 Hz part is what makes the sampled result sensitive to the A/D sampling phase. It is
    also what the codec's anti-alias filter removes most. Keep this in mind when designing the
    phase estimator.

### 3.6 CPt (8.5.1 on pp.28–29; Table 23 on pp.33–35)

**Purpose.** Modulation parameters the digital modem uses **during phase 4 training**: the
TRN2d rate, constellations and spectral shaping.

**Modulation and encoding:**

* Same modulation as TRN1u: 1 bit per symbol, ±LU, 0 = +LU.
* Scrambled (GPA) and differentially encoded.
* The differential encoder starts from the final symbol of the preceding TRN1u.
* **24 differentially encoded binary ones shall be sent before the first CPt** of a series. They
  are scrambled too, since they are "differentially encoded ones" on the CPt path.
* When CPt is sent repeatedly, every copy **shall** carry identical information.
* CRC per 10.1.2.3.2/V.34.

**Length.** Variable.

* γ = 136 × (the largest of the six constellation indices in bits 103:127).
* δ = 2γ + 136 if bit 128 is set, otherwise δ = γ.
* Constellation j (0..5) occupies bits 136+136j .. 271+136j. Index 5 is at bits 816:951.
* A constellation shared by several data frame intervals is sent once.
* If bit 128 is set, the codec-output (D/A) version of every transmit constellation follows,
  in the same format.

Bit layout (Table 23; "CPt" column meanings):

| Bits | Field |
|---|---|
| 0:16 | frame sync, 17 ones |
| 17 | start 0 |
| 18 | "CP": **0** (SUV sequences have 1 here) |
| 19:20 | **Type: 0 = CPt** (1 = CPu, 2 = CPus) |
| 21:25 | **drn**, 0..22. drn = 0 means cleardown. **In CPt, rate = (drn + 8)·8000/6** (so drn 1..22 is 12 000..40 000 bit/s). This is the TRN2d signalling rate for phase 4 (Table 17/V.90). |
| 26:30 | reserved 0 |
| 31:32 | **Sr**: sign bits of redundancy for spectral shaping |
| 33 | acknowledge bit: 1 = the modem has received CPd. In phase 3 no CPd exists yet, so send **0** (interpretation). |
| 34 | start 0 |
| 35 | codec type: 0 = µ-law, 1 = A-law |
| 36:48 | reserved 0 (V.90's CP had the upstream rate mask here; see 13.1) |
| 49:50 | **ld**: look-ahead frames requested for spectral shaping. **Shall** fit the digital modem's capability in Jd bits 49:50. |
| 51 | start 0 |
| 52:67 | RMS of TRN1d at the transmitter output ÷ RMS of TRN1d at the codec D/A output, **unsigned Q3.13** (xxx.xxxxxxxxxxxxx) |
| 68 | start 0 |
| 69:76 | spectral shaping filter a1, signed Q1.6 (sx.xxxxxx) |
| 77:84 | a2, signed Q1.6 |
| 85 | start 0 |
| 86:93 | b1, signed Q1.6 |
| 94:101 | b2, signed Q1.6 |
| 102 | start 0 |
| 103:106 | constellation index (0..5) for downstream data frame interval 0 |
| 107:110 | index for interval 1 |
| 111:114 | index for interval 2 |
| 115:118 | index for interval 3 |
| 119 | start 0 |
| 120:123 | index for interval 4 |
| 124:127 | index for interval 5 |
| 128 | 1 = the transmitter constellations differ from those at the codec D/A output |
| 129:135 | reserved 0 |
| 136 | start 0 |
| 137:152 | mask for Uchord1. Bit 137 = Ucode 0, …, bit 152 = Ucode 15. |
| 153 | start 0 |
| 154:169 | mask for Uchord2 (bit 154 = Ucode 16) |
| 170 | start 0 |
| 171:186 | mask for Uchord3 (bit 171 = Ucode 32) |
| 187 | start 0 |
| 188:203 | mask for Uchord4 (bit 188 = Ucode 48) |
| 204 | start 0 |
| 205:220 | mask for Uchord5 (bit 205 = Ucode 64) |
| 221 | start 0 |
| 222:237 | mask for Uchord6 (bit 222 = Ucode 80) |
| 238 | start 0 |
| 239:254 | mask for Uchord7 (bit 239 = Ucode 96) |
| 255 | start 0 |
| 256:271 | mask for Uchord8 (bit 256 = Ucode 112) |
| 272 : 271+γ | further constellations (index 1..max), same 136-bit format |
| 272+γ : 271+δ | codec-output constellations, same format (only when bit 128 = 1) |
| 272+δ | start 0 |
| 273+δ : 288+δ | CRC |
| 289+δ | fill bit 0 |
| 289+δ … | fill zeros up to the next multiple of 12 symbols. The table repeats 289+δ as the start of this row too; treat it as "from 290+δ". |

* A constellation mask bit set to 1 means the constellation includes the PCM code of that Ucode.
* Inside one 136-bit constellation block, the start bits sit at block offsets 0, 17, 34, …, 119.
* Lengths (derived; the unpadded length is 290 + δ):

  | Largest index | Bit 128 | Unpadded | Padded |
  |---|---|---|---|
  | 0 | 0 | 290 | 300 |
  | 0 | 1 | 426 | 432 |
  | 1 | 0 | 426 | 432 |
  | 5 | 0 | 970 | 972 |
  | 5 | 1 | 1786 | 1788 |

* V.90 8.5.2 requires the analogue modem's data-mode constellations to average no more than
  3 dB above its phase 4 constellations. The CP power formula and the Table 15/V.90 limits also
  exist. V.92 8.5.1 restates neither (see 13.4).

### 3.7 E1u (8.5.2, p.29)

* **One upstream data frame (12 symbols)** of scrambled, differentially encoded **zeros**, with
  the same modulation as CPt. It marks the end of CPt.
* Sent once, right after the last CPt is complete (9.5.2.1.10 and 9.5.2.1.11).
* After descrambling and differential decoding, the receiver sees 12 zeros where a new CPt would
  start with 17 ones.
* Every CPt ends on a 12-symbol boundary with at least one zero of fill. So one way to spot E1u
  is: at a CPt boundary, if the next 12 decoded bits are all 0, it is E1u.
* Phase 4 TRN2u starts its differential encoder from "the last transmitted sign bit of the
  preceding E1u" (8.7.6).

---

## 4. Signals sent by the digital modem

General (8.6, p.30):

* The **GPC** scrambler is used for Jd, Jp, Jp', SCR and TRN1d.
* **Nothing the digital modem sends in phase 3 is spectrally shaped.**
* Every downstream phase 3 signal is a whole number of 6-symbol frames, so downstream data frame
  alignment, fixed at Sd, carries straight into TRN2d.

### 4.1 Sd and S̄d (8.6.7, pointing to 8.4.4/V.90, V.90 p.29)

* W is the PCM codeword with Ucode **16 + UINFO**. "0" is the codeword with Ucode 0, signed.
* **Sd** = 64 repetitions of {+W, +0, +W, −W, −0, −W}, which is **384T**.
* **S̄d** = 8 repetitions of {−W, −0, −W, +W, +0, +W}, which is **48T**.
* **The first symbol of Sd is downstream data frame interval 0.** The digital modem keeps data
  frame alignment from there.
  * The V.90 page draws only one overbar in this paragraph, over the S̄d of the 8-repetition
    sentence, so the anchor is the start of plain Sd.
* Derived constraint: 16 + UINFO must be a valid Ucode (≤ 127), so UINFO is 67..111 in practice.

### 4.2 TRN1d (8.6.8, pointing to 8.4.5/V.90, V.90 p.30)

* The codeword UINFO, with signs from feeding binary ones into the GPC scrambler: 0 = negative,
  1 = positive.
* The scrambler is reset to zero before TRN1d.
* **A whole multiple of 6 symbols.**
* Phase 3 minimum: 2040T (9.5.1.1.4).
* First 48 signs with a zero start (derived with GPC, 1 = +):
  `111111111111111111000001111111111111000000000011`

### 4.3 Jd (8.6.2 and Table 21, pp.30–31)

**Structure and encoding:**

* A whole number of repetitions of the 72-bit pattern below, bit 0 first.
* Each bit is GPC-scrambled, differentially encoded, and sent as the **sign** of UINFO
  (0 = negative, 1 = positive).
* The differential encoder starts from **the final symbol of TRN1d**. The scrambler carries on
  from TRN1d.
* CRC per 10.1.2.3.2/V.34.

| Bits | Field |
|---|---|
| 0:16 | frame sync, 17 ones |
| 17 | start 0 |
| 18:33 | **downstream rate mask, part 1.** Bit 18+k = 28 000 + k·8000/6 for k = 0..15: 28 000, 29 333, 30 666, … bit 33 = 48 000. A set bit means supported and enabled in the digital modem's transmitter. |
| 34 | start 0 |
| 35:40 | **mask, part 2.** k = 16..21: bit 35 = 49 333, 36 = 50 666, …, 39 = 54 666, 40 = 56 000. |
| 41:46 | reserved 0 |
| 47 | **Jd/Jp identifier: 0 = Jd** |
| 48 | reserved 0 |
| 49:50 | digital modem's maximum spectral-shaping look-ahead, 1..3 |
| 51 | start 0 |
| 52:67 | CRC |
| 68:71 | fill 0000 |

Derived test vector: all 22 rates enabled, look-ahead 3. The CRC is 0xF366, sent LSB first as
`0110011011001111`. The 72 bits before scrambling:

```
0:16=11111111111111111 17=0 18:33=1111111111111111 34=0 35:46=111111000000
47=0 48=0 49:50=11 51=0 52:67=0110011011001111 68:71=0000
```

### 4.4 Jp (8.6.3 and Table 22, pp.31–32)

**Structure and encoding:**

* A whole number of repetitions of 72 bits, encoded exactly like Jd (GPC, differential, sign of
  UINFO).
* The differential encoder starts from **the final symbol of the transmitted Jd**.
* The digital modem cannot move the central office A/D's sampling phase. Instead it **shall**
  use Jp to ask the analogue modem to shift its transmitter phase by an amount in [0, 1) symbol,
  that is [0, T).
* CRC per 10.1.2.3.2/V.34.

| Bits | Field |
|---|---|
| 0:16 | frame sync, 17 ones |
| 17 | start 0 |
| 18:33 | **ε**: how much the S̄u at the Jp-to-Jp' change must be lengthened. 16-bit unsigned covering [0, 1) symbol, so ε = value / 65536 symbol, or value · T/65536 (about 1.9 µs per step). |
| 34 | start 0 |
| 35:46 | reserved 0 |
| 47 | **Jd/Jp identifier: 1 = Jp** |
| 48 | size of the constellation for **CPu, E2u, SUVu and TRN2u in training**: 0 = 4-point, 1 = 8-point |
| 49 | the same **in rate renegotiation**: 0 = 4-point, 1 = 8-point |
| 50 | reserved 0 |
| 51 | start 0 |
| 52:67 | CRC |
| 68:71 | fill 0000 |

TRN2u's 4- and 8-point PAM mappings are Tables 28 and 29 (8.7.6), in phase 4.

Derived test vector: ε = 0x8000 (half a symbol), bit 48 = 1, bit 49 = 0. The CRC is 0x3E4E,
sent as `0111001001111100`.

```
0:16=11111111111111111 17=0 18:33=0000000000000001 34=0 35:46=000000000000
47=1 48=1 49:50=00 51=0 52:67=0111001001111100 68:71=0000
```

### 4.5 Jp' (8.6.4, p.32)

* Ends Jp. It is **12 binary zeros**, GPC-scrambled, differentially encoded, and sent as the sign
  of UINFO.
* The differential encoder starts from **the final symbol of Jp**.
* It lasts 12T (2 downstream frames).
* Same construction as J'd in V.90 (8.4.3/V.90).

### 4.6 DIL (8.6.1, pointing to 8.4.1/V.90, V.90 pp.28–29)

**Structure:**

* N DIL segments, 0 ≤ N ≤ 255.
* A segment whose training Ucode u lies in Uchord c is **Lc = (Hc + 1)·6 symbols** long,
  1 ≤ c ≤ 8.
* Segment s (1..N) uses the s-th Ucode from the descriptor as its training symbol.
* REFc is the Ucode of the reference symbol used in segments whose training symbol is in
  Uchord c.

**Symbol n of a segment** (n = 0 is the first; the patterns restart at every segment and repeat
independently inside long segments):

* sign = SP[n mod LSP], where **0 = negative and 1 = positive**.
* codeword = TP[n mod LTP] ? training Ucode : REFc, where **TP 0 = REFc and 1 = training
  symbol**.
* The LSB of each pattern applies to the first symbol of the segment.

**Other rules:**

* Not scrambled and not differentially encoded. The signs are absolute.
* **The whole sequence** (all N segments, not just the last one) is repeated until the analogue
  modem ends it or a timeout occurs. It **shall** end on a segment boundary.
  * In V.92 the analogue modem ends it by sending CPt (9.5.1.1.11–12).
  * No timeout value is given (see 13.4).
* Each segment is a multiple of 6 symbols, so downstream frame alignment is kept.
* When N = 0, no DIL is sent. **In V.92 the digital modem sends SCR instead** (9.5.1.1.11).
* V.90's note (p.28) says it is highly desirable that the analogue modem asks for a DIL that
  does not let echo control devices in the network re-enable.
  * In V.92 the analogue modem sends TRN1u throughout DIL, so there is energy on the line in
    both directions.

`crate::v90::sequences::Descriptor::symbols()` already builds this.

### 4.7 SCR (8.6.6, p.32)

* The codeword UINFO, with signs from GPC-scrambled binary ones: 0 = negative, 1 = positive.
* **The scrambler need not be reset** at the start of SCR.
* Not differentially encoded (none is stated).
* A whole multiple of **6** symbols.
* Sent after Jp' in place of DIL when N = 0.

### 4.8 Ri and R̄i (8.6.5, pointing to 8.6.4/V.90, V.90 p.35)

* **R** repeats a 6-symbol block with signs + + + − − − (leftmost first).
* **R̄** is **4 repetitions (24T)** of the same codewords with signs − − − + + +.
* **Ri** uses the single codeword UINFO in every data frame interval.
* Neither R nor R̄ is differentially encoded, so the receiver **must** find them whatever the
  line polarity (V.90 note).
* Keep the 6-symbol block on downstream frame boundaries, starting with the first "+" at
  interval 0.
  * This is implied by "each data frame interval" for Rd and Rt, and it is what the V.90 code
    does.
* In V.92 phase 3, Ri runs until a trigger arrives: CPt (zero DIL) or E1u (non-zero DIL). Then
  R̄i (24T) is sent, and phase 4 begins with TRN2d.
* V.90's phase 4 rule of "Ri for at least 192T" is not restated in V.92.

---

## 5. INFO1a (Table 18) fields that phase 3 uses (p.27)

Bit 0 is sent first; integers go LSB first.

| Bits | Field | Phase 3 use |
|---|---|---|
| 0:3 | fill 1111 | — |
| 4:11 | frame sync 01110010, leftmost first | — |
| 12:13 | precoder and prefilter filter sections: 0 = p1, z2; 1 = z1, p1, z2; 2 = p1, p2, z2; 3 = z1, p1, p2, z2 | phase 4 (CPd) |
| 14:15 | maximum total coefficients Ltot = LZ1+LP1+LZ2+LP2: 0 = 192, 1 = 256, 2 = 320, 3 = 384 | phase 4 |
| 16:17 | maximum coefficients per section Lmax: 0 = 128, 1 = 192, 2 = 256, 3 = 320 | phase 4 |
| 18:24 | **MD length**, 0..127, in steps of 276 symbols (34.5 ms) | 9.5.1.1.1 and 9.5.2.1.1 |
| 25:31 | **UINFO** (> 66, power ≤ the digital modem's maximum) | TRN1d, Jd, Jp, Jp', SCR, Ri, and W = 16 + UINFO in Sd |
| 32:33 | reserved 0 | — |
| 34:36 | analogue modem symbol rate: the integer **6** (8000) | selects PCM upstream |
| 37:39 | digital modem symbol rate: the integer **6** | — |
| 40:49 | reserved, **set to 1** so that no tone is generated | — |
| 50:65 | CRC | — |
| 66:69 | fill 1111 | — |

---

## 6. Digital modem procedure (9.5.1, pp.54–55)

### 6.1 Error-free procedure (9.5.1.1)

* **D1 (9.5.1.1.1).**
  * Start **silent**. **Shall** listen for Ru and then R̄u.
  * If INFO1a's MD length is 0, go to D2.
  * Otherwise, after the Ru-to-R̄u reversal, **shall** wait the MD duration (length × 276T) and
    then listen again for Ru and its Ru-to-R̄u reversal.
    * Whether the 24T R̄u counts toward the wait is not stated. Allow for it.
* **D2 (9.5.1.1.2).** After Ru and the Ru-to-R̄u reversal, **shall** start training the
  equalizer on TRN1u.
  * TRN1u begins 24T after the reversal and starts with a zero-state GPA sequence (3.3). This
    known reference can be used for least-squares training.
* **D3 (9.5.1.1.3).**
  * After the **first 2040T** of TRN1u, **shall** get ready to receive Ja.
  * After receiving a DIL descriptor from Ja (a CRC-valid one), **may** wait **up to 500 ms**.
  * It **shall** then send **Sd for 384T and S̄d for 48T**.
* **D4 (9.5.1.1.4).**
  * **Shall** then send TRN1d for **at least 2040T**.
  * **Within 4000 ms of starting TRN1d, shall** send Jd, and listen for Su.
* **D5 (9.5.1.1.5).** **Shall** keep repeating Jd.
* **D6 (9.5.1.1.6).**
  * On detecting Su, **shall** listen for the Su-to-S̄u reversal.
  * **Should** use Su to measure phase.
* **D7 (9.5.1.1.7).**
  * On the Su-to-S̄u reversal (the 24.5T S̄u), **shall** carry on receiving Su.
  * **Should** keep measuring phase.
* **D8 (9.5.1.1.8).**
  * Once it has worked out the right phase adjustment (ε), **shall** finish the current Jd and
    then send **Jp**, repeated.
  * **Shall** listen for the next Su-to-S̄u reversal.
* **D9 (9.5.1.1.9).**
  * On that reversal (the (24+ε)T S̄u), **shall** finish the current Jp.
  * **Shall** turn **circuit 107 ON**.
  * **Shall** then send **Jp'** (12T).
* **D10 (9.5.1.1.10).**
  * **Shall** then receive TRN1u (the second one).
  * **Shall** keep a **modulo-12 data frame interval count from its first symbol**, which is
    upstream interval 0 (8.5.7).
  * The second TRN1u starts 24T+ε after the reversal and again starts from a zero-state GPA
    sequence (interpretation, 3.3).
* **D11 (9.5.1.1.11).**
  * After Jp', **shall** send the DIL the analogue modem asked for, and listen for CPt.
  * **If N = 0, shall send SCR instead** and go to D13.
* **D12 (9.5.1.1.12).** Non-zero DIL.
  * On receiving CPt, **shall** send **Ri**.
    * Finish the current DIL segment first (8.4.1/V.90: DIL ends on a segment boundary).
  * On receiving the **E1u** that ends the CPt run, **shall** send **R̄i** (24T) and go to
    **phase 4** (9.6.1: TRN2d for at least 2040T).
* **D13 (9.5.1.1.13).** Zero DIL.
  * **When sufficiently trained**, meaning its receiver on the second TRN1u, **shall** send
    **Ri** and listen for CPt.
  * On receiving CPt, **shall** send **R̄i** and go to **phase 4**.
  * Unlike D12, it does not wait for E1u here.

### 6.2 Recovery (9.5.1.2)

* **May** start a retrain at any point in phase 3 (9.7.1.1).
* **Shall** answer a retrain (9.7.1.2) if Tone A is detected in phase 3.
* **DR1 (9.5.1.2.1).** If Ja is **not detected within 4500 ms + RTD from the end of INFO1a**,
  **shall** start a retrain (9.7.1.1).
* **DR2 (9.5.1.2.2).** If Su is **not detected within 5100 ms + RTD from the start of TRN1d**,
  **shall** start a retrain (9.7.1.1).
  * The clause says "in 9.5.1.1.9", but Su detection is D4/D6. Treat that as a wrong
    cross-reference.
* Timer covering all of phase 3 (9.6.1.2.1): if B1u has not arrived **within 20 s + 6·RTD of
  the end of INFO1a**, **shall** retrain (9.7.1.1).

---

## 7. Analogue modem procedure (9.5.2, pp.55–56)

### 7.1 Error-free procedure (9.5.2.1)

* **A1 (9.5.2.1.1).** After INFO1a, **shall** send, in order:
  * **silence for 70 ± 5 ms**;
  * **Ru for 384T**;
  * **R̄u for 24T**.

  If the MD length in INFO1a is 0, go to A2. Otherwise **shall** also send **MD** for the
  stated time, then **Ru for 384T**, then **R̄u for 24T**.
* **A2 (9.5.2.1.2).**
  * **Shall** then send **TRN1u for at least 2040T** (scrambler reset; a multiple of 12 symbols).
  * **From the start of MD to the end of TRN1u must not exceed RTD + 4000 ms.**
    * With no MD, "start of MD" is undefined. Measure from where MD would have started, that is
      the end of the first R̄u (interpretation).
* **A3 (9.5.2.1.3).**
  * After TRN1u, **shall** send **Ja** (24 ones, then descriptors) and listen for Sd and the
    Sd-to-S̄d reversal.
  * On that reversal, **shall** end Ja **at the next 12-bit boundary** and **send silence**.
  * The analogue modem stays silent until A6. This gives the digital modem a quiet line while it
    sends Sd, TRN1d and Jd.
* **A4 (9.5.2.1.4).** **Shall** start equalizer training on the **first 2040T of TRN1d**.
  * Sd's reversal marks the start of TRN1d exactly, and downstream interval 0 is at the start of
    Sd.
* **A5 (9.5.2.1.5).** After 2040T of TRN1d, **shall** listen for Jd.
* **A6 (9.5.2.1.6).**
  * After receiving Jd, **may** wait **up to 5000 ms, counted from when it started the A3
    silence**.
  * It **shall** then send **Su for 144T**.
* **A7 (9.5.2.1.7).**
  * After 144T of Su, **shall** send **S̄u for 24.5T**, then **Su** again, running on.
  * **Shall** listen for **Jp**.
* **A8 (9.5.2.1.8).**
  * On detecting Jp (CRC-valid, bit 47 = 1), **shall** turn **circuit 107 ON**.
  * **Shall** send **S̄u for 24T plus ε**, where ε is 0 to 1 symbol from Jp bits 18:33.
  * **Shall** listen for **Jp'**.
  * Applying ε means every later upstream symbol goes out ε·T later than before. The transmitter
    therefore needs sub-symbol delay: an interpolating output or a shifted D/A clock phase.
* **A9 (9.5.2.1.9).**
  * On detecting Jp', **shall** receive the DIL it asked for in Ja, or **SCR** if it asked for
    N = 0.
  * **Throughout that reception, shall send TRN1u.**
  * This TRN1u **shall** be a whole number of 12-symbol frames, and **at least 2040T if a
    non-zero DIL was asked for**.
  * Its first symbol is upstream data frame interval 0.
* **A10 (9.5.2.1.10).** Zero DIL.
  * **Shall** keep sending TRN1u **until it receives Ri**.
  * It **shall** then send CPt, repeated: first the 24 differentially encoded ones, then CPt,
    CPt, …
  * On receiving **R̄i**, **shall** finish the current CPt, send **E1u**, and go to **phase 4**.
* **A11 (9.5.2.1.11).** Non-zero DIL.
  * **Shall** send **at least 2040T of TRN1u, then CPt, starting within 5000 ms of sending the
    S̄u of A8**.
    * Measure from the start of that S̄u, since the text says "of transmitting".
  * Starting CPt tells the digital modem that the analogue modem has captured enough DIL.
  * **Shall** keep sending CPt until it receives **Ri**.
  * On receiving **Ri**, **shall** finish the current CPt, send **E1u**, and go to **phase 4**
    (9.6.2: TRN2u, with SUVu once ready).

### 7.2 Recovery (9.5.2.2)

* **May** start a retrain at any point in phase 3 (9.7.2.1).
* **Shall** answer a retrain (9.7.2.2) if Tone B is detected in phase 3.
* **AR1 (9.5.2.2.1).** If the Sd-to-S̄d reversal is **not detected within 1500 ms of the start
  of Ja**, **shall** retrain (9.7.2.1).
  * This timer has no RTD term. See 13.2.
* **AR2 (9.5.2.2.2).** If Jd is **not received within 4500 ms of the end of Ja**, **shall**
  retrain (9.7.2.1).
* Timer covering all of phase 3 (9.6.2.2.1): if B1d has not arrived **within 20 s + 6·RTD of
  the end of sending INFO1a**, **shall** retrain.

### 7.3 Retrain procedures referenced (9.7, p.60)

* **9.7.1.1, digital modem starts a retrain.**
  * Circuit 106 OFF. Circuit 104 clamped to binary 1.
  * **Silence for 70 ± 5 ms**, then Tone B, listening for Tone A.
  * Once Tone A is found, listen for a Tone A phase reversal and continue with the **full**
    phase 2.
* **9.7.1.2, digital modem answers.**
  * After **Tone A has been detected for more than 50 ms**: 106 OFF, 104 clamped to 1, silence
    for 70 ± 5 ms.
  * Then Tone B, listening for the Tone A reversal, and on into full phase 2.
* **9.7.2.1, analogue modem starts a retrain.**
  * 106 OFF, 104 clamped to 1, silence for 70 ± 5 ms, then Tone A, listening for Tone B.
  * Once Tone B is detected **and Tone A has been sent for at least 50 ms**: send a Tone A phase
    reversal, listen for the Tone B reversal, and continue with full phase 2.
* **9.7.2.2, analogue modem answers.**
  * After **Tone B has been detected for more than 50 ms**: 106 OFF, 104 clamped to 1, silence
    for 70 ± 5 ms.
  * Then Tone A, and on into full phase 2.
* Tones A and B are the V.34 tones (2400 Hz with an 1800 Hz guard; 1200 Hz), per
  10.1.2.1–10.1.2.2/V.34 through 8.2/V.90.

---

## 8. Timers and tolerances

| ID | Who | Quantity | Value | Measured from, to | Clause |
|---|---|---|---|---|---|
| T1 | A | silence after INFO1a | 70 ± 5 ms | end of INFO1a | 9.5.2.1.1 |
| T2 | A | Ru / R̄u | 384T / 24T (each time sent) | — | 9.5.2.1.1 |
| T3 | A | MD length | INFO1a[18:24] × 276T (0..35 052T) | — | Table 18, 9.5.2.1.1 |
| T4 | A | first TRN1u | ≥ 2040T, a multiple of 12 | — | 9.5.2.1.2, 8.5.7 |
| T5 | A | start of MD to end of TRN1u | ≤ RTD + 4000 ms | — | 9.5.2.1.2 |
| T6 | D | Ja receive to Sd | optional wait ≤ 500 ms | end of a received descriptor | 9.5.1.1.3 |
| T7 | D | Sd / S̄d | 384T / 48T | — | 9.5.1.1.3 |
| T8 | D | TRN1d | ≥ 2040T, a multiple of 6 | — | 9.5.1.1.4, 8.4.5/V.90 |
| T9 | D | start of Jd | ≤ 4000 ms after TRN1d starts | — | 9.5.1.1.4 |
| T10 | A | end of Ja | next 12-bit boundary after the Sd-to-S̄d reversal is detected | — | 9.5.2.1.3 |
| T11 | A | Jd to Su | optional wait, Su no later than 5000 ms after the silence starts | start of A3 silence | 9.5.2.1.6 |
| T12 | A | Su, S̄u | 144T, then 24.5T | — | 9.5.2.1.6–7 |
| T13 | A | second S̄u | 24T + ε, with ε = Jp[18:33]/65536 T, in [0, T) | — | 9.5.2.1.8 |
| T14 | D | Jp' | 12T | — | 8.6.4 |
| T15 | A | second TRN1u | a multiple of 12; ≥ 2040T if N ≠ 0 | — | 9.5.2.1.9 |
| T16 | A | start of CPt (N ≠ 0) | ≤ 5000 ms after the A8 S̄u is sent, and after ≥ 2040T of TRN1u | — | 9.5.2.1.11 |
| T17 | A | 24 ones before the first CPt | 24T | — | 8.5.1 |
| T18 | A | E1u | 12T | — | 8.5.2 |
| T19 | D | R̄i | 24T | — | 8.6.4/V.90 |
| TR1 | D | Ja watchdog | 4500 ms + RTD | end of INFO1a | 9.5.1.2.1 |
| TR2 | D | Su watchdog | 5100 ms + RTD | start of TRN1d | 9.5.1.2.2 |
| TR3 | A | Sd-to-S̄d watchdog | 1500 ms (no RTD) | start of Ja | 9.5.2.2.1 |
| TR4 | A | Jd watchdog | 4500 ms (no RTD) | end of Ja | 9.5.2.2.2 |
| TR5 | D | start-up watchdog | 20 s + 6·RTD until B1u | end of INFO1a | 9.6.1.2.1 |
| TR6 | A | start-up watchdog | 20 s + 6·RTD until B1d | end of sending INFO1a | 9.6.2.2.1 |
| TR7 | both | retrain silence | 70 ± 5 ms | — | 9.7 |
| TR8 | both | tone qualification | > 50 ms (responding); ≥ 50 ms of own Tone A before the reversal (analogue initiating) | — | 9.7 |

Missing timers:

* No stated limit on the digital modem waiting for CPt after Jp' when N ≠ 0.
* No stated limit on how long SCR and Ri may last when N = 0.
* No DIL "timeout" value (8.4.1/V.90).

Only TR5 and TR6 cover these.

Do the Recommendation's timers fit together?

* **TR2 against T11.** The analogue modem may start Su 5000 ms after it heard S̄d, and the
  digital modem started TRN1d 6 ms after S̄d began. So Su arrives at most about
  5000 + 6 ms + RTD after the start of TRN1d, inside 5100 ms + RTD, with roughly 94 ms to spare.
* **TR4 against T9.** Jd starts at most 4000 ms after TRN1d, and one Jd lasts 9 ms. That fits in
  4500 ms, counted from when the analogue modem ended Ja.
* **TR3 against T6.** A descriptor takes at least 300 bits (37.5 ms) to arrive. Add the digital
  modem's optional 500 ms, 48 ms of Sd, and the round trip. TR3 holds only if
  **RTD ≲ 900 ms** (see 13.2).

---

## 9. Handshake table: event and required reaction

| Receiver | Event detected | Reaction | Clause |
|---|---|---|---|
| D | Ru, then the Ru-to-R̄u reversal (after MD, if any) | train the equalizer on TRN1u | 9.5.1.1.1–2 |
| D | 2040T of TRN1u done | hunt for the Ja descriptor | 9.5.1.1.3 |
| D | valid DIL descriptor | (≤ 500 ms) Sd 384T, S̄d 48T, TRN1d ≥ 2040T, Jd ≤ 4000 ms after TRN1d starts | 9.5.1.1.3–4 |
| A | Sd-to-S̄d reversal | finish Ja at a 12-bit boundary, go silent, train on TRN1d | 9.5.2.1.3–4 |
| A | Jd (CRC ok, bit 47 = 0) | (≤ 5000 ms from the silence) Su 144T, S̄u 24.5T, then Su | 9.5.2.1.6–7 |
| D | Su, then the Su-to-S̄u reversal | measure phase; once ε is known, finish Jd, send Jp repeatedly | 9.5.1.1.6–8 |
| A | Jp (CRC ok, bit 47 = 1) | 107 ON, S̄u (24+ε)T, second TRN1u | 9.5.2.1.8–9 |
| D | second Su-to-S̄u reversal | finish Jp, 107 ON, Jp', then DIL (or SCR); frame count from the first TRN1u symbol | 9.5.1.1.9–11 |
| A | Jp' | receive DIL/SCR while sending TRN1u | 9.5.2.1.9 |
| D (N ≠ 0) | CPt | finish the DIL segment, send Ri | 9.5.1.1.12 |
| A (N ≠ 0) | Ri | finish the current CPt, send E1u, go to phase 4 | 9.5.2.1.11 |
| D (N ≠ 0) | E1u | send R̄i, go to phase 4 | 9.5.1.1.12 |
| D (N = 0) | its own training is good enough | Ri | 9.5.1.1.13 |
| A (N = 0) | Ri | 24 ones, CPt, CPt, … | 9.5.2.1.10 |
| D (N = 0) | CPt | R̄i, go to phase 4 | 9.5.1.1.13 |
| A (N = 0) | R̄i | finish the current CPt, send E1u, go to phase 4 | 9.5.2.1.10 |
| either | Tone A (D) / Tone B (A) | answer the retrain (9.7.x.2) | 9.5.1.2 / 9.5.2.2 |

---

## 10. What the referenced texts require (summary)

| Reference | Requirement |
|---|---|
| 5.3/V.90 (through clause 5/V.92) | digital modem scrambler: self-synchronizing, clause 7/V.34, GPC |
| 6.3/V.92 | analogue modem scrambler: clause 7/V.34, GPA |
| clause 7/V.34 | GPC = 1 + x^−18 + x^−23; GPA = 1 + x^−5 + x^−23; the transmitter divides by the polynomial |
| 10.1.2.3.2/V.34 | CRC-16 x^16+x^12+x^5+1, register starts at all ones, covers everything but sync, start and fill bits, register bit 0 out first (LSB) |
| 10.1.3.5/V.34 | MD: optional, manufacturer-defined echo-canceller training; its length is given in INFO1, 0 = absent |
| 8.3.1/V.90 | DIL descriptor bits 0..187+X (Table 12), α and β with ceilings, SP and TP zero-padded to 16 bits, LSP−1 = LTP−1 = 0 when N = 0 |
| 8.4.1/V.90 | DIL construction: segment length (Hc+1)·6; REFc; SP (0 neg / 1 pos); TP (0 REF / 1 training); patterns restart per segment; the whole DIL repeats until stopped or timed out; ends on a segment boundary; N = 0 means no DIL |
| 8.4.4/V.90 | Sd = 64 × {+W,+0,+W,−W,−0,−W}; S̄d = 8 × the inverse; W = Ucode 16+UINFO; the first Sd symbol is downstream interval 0 |
| 8.4.5/V.90 | TRN1d: UINFO with GPC-scrambled-ones signs (0 neg), scrambler reset to zero, a multiple of 6 symbols |
| 8.6.4/V.90 | R: +++−−− repeated; R̄: 4 × −−−+++; not differentially encoded; Ri uses UINFO in every interval |
| 8.6.5/V.90 (TRN2d, phase 4) | uses the constellation set passed in CPt; scrambler, differential encoder and spectral shaper start at zero; a multiple of 6 symbols |
| 9.6/V.92 (phase 4) | the digital modem starts with TRN2d ≥ 2040T; the analogue modem starts with TRN2u (≥ 12000T before SUVu unless SUVd is received); TRN2u's differential encoder starts from the last sign of E1u |
| 9.7/V.92 | retrains (7.3 above) |

---

## 11. How PCM upstream is trained in phase 3, and what the digital modem works out and sends back

Phase 3 does not set upstream data-mode parameters. Those come from phase 4 (TRN2u, then CPd per
Table 30, which carries the modulus encoder parameters, the precoder and prefilter coefficients,
and the upstream constellations). Phase 3 prepares for that in the following steps.

1. **Upstream receiver acquisition.**
   * The analogue modem sends two-point, 8 kHz, ±LU signals (Ru and TRN1u) with no precoder or
     prefilter.
   * The digital modem trains its equalizer on the first TRN1u, reading the analogue loop as the
     CO codec's A/D samples it.
   * TRN1u is a zero-start GPA sequence, so the receiver knows it in advance.
2. **Echo cancellers.**
   * Only the analogue modem transmits (Ru, MD, TRN1u, Ja) while the digital modem is silent, so
     the analogue modem can train its echo canceller.
   * The analogue modem then goes silent while the digital modem sends Sd, TRN1d and Jd. The
     digital modem can use that stretch for any echo canceller it keeps against the CO hybrid's
     echo of its own signal (interpretation; the clause title names echo canceller training but
     gives no procedure).
   * From Jp' onwards both directions run at once: DIL or SCR against the second TRN1u.
3. **Sampling-phase alignment (new in V.92).**
   * The CO A/D samples the upstream signal at a phase the digital modem cannot change (8.6.3).
     So the digital modem measures that phase on **Su** (it **should** do so, 9.5.1.1.6–7).
   * The measurement covers the Su before and after the first S̄u; that S̄u is 24.5T, so the
     second Su is shifted by half a symbol.
   * The digital modem returns **ε**, a 16-bit fraction of T, in **Jp** bits 18:33.
   * The analogue modem applies ε by lengthening the next S̄u to (24+ε)T. From then on its
     symbols line up with the A/D sampling instants.
   * **The estimator is not specified.** Only the signal and the result format are fixed.
4. **Upstream frame alignment.**
   * The first symbol of the second TRN1u is upstream data frame interval 0 (12-symbol frames).
   * The digital modem counts modulo 12 from there (9.5.1.1.10, 8.5.7).
   * The second TRN1u also lets the digital modem refine its equalizer at the new phase. In the
     zero-DIL case, this TRN1u is what "sufficiently trained" refers to (9.5.1.1.13).
5. **Phase 4 upstream set-up that the digital modem chooses now.** Jp bit 48 (training) and
   bit 49 (rate renegotiation) pick a 4- or 8-point constellation for CPu, E2u, SUVu and TRN2u.
6. **What the digital modem learns from the analogue modem in phase 3:**
   * from **Ja**: the DIL parameters, and the **upstream rate mask** (24 000..48 000 bit/s)
     enabled in the analogue modem's transmitter. It needs the mask for its phase 4 upstream rate
     choice.
   * from **CPt**: the phase 4 training set-up for TRN2d, SUVd and CPd. That is the rate
     (drn+8)·8000/6, Sr, ld, codec law, the RMS ratio, a1, a2, b1, b2, the per-interval
     constellation indices and masks, and any codec-output constellations.

**What the digital modem sends back in phase 3:**

* **Jd**: its downstream rate mask (28 000..56 000) and its maximum look-ahead (1..3). Bit 47 = 0.
* **Jp**: ε, the TRN2u, CPu, E2u and SUVu constellation sizes, and bit 47 = 1.
* **Jp'**: the end of Jp.
* **DIL**, built exactly as the descriptor asks, or SCR when N = 0.
* **Ri / R̄i**: the handshake around CPt and E1u.

**What the analogue modem works out:**

* its equalizer from TRN1d;
* DIL analysis, which becomes CPt now and, in phase 4, CPu (the data-mode downstream set-up);
* its own echo canceller;
* when it has "enough DIL", which it signals by starting CPt.

---

## 12. Test vectors (derived by the digest author, not in the Recommendation)

All use the repo's conventions (`v34::info::crc`, `v32::Scrambler`). They are values before
scrambling unless noted.

* GPA, ones in, zero start, first 48 outputs (the TRN1u signs; 1 = −LU):
  `111110000011111000001110011111000110000011100100`
* GPC, ones in, zero start, first 48 outputs (the TRN1d signs; 1 = +UINFO):
  `111111111111111111000001111111111111000000000011`
* Jd with all 22 rates, look-ahead 3: CRC = 0xF366 (see 4.3 for all 72 bits).
* Jp with ε = 0x8000, bit 48 = 1, bit 49 = 0: CRC = 0x3E4E (see 4.4).
* Ja descriptor with N = 0, all 19 upstream rates enabled, H and REF all zero:
  * 273 bits through the fill bit, padded to 276;
  * CRC = 0xB71C, sent LSB first as `0011100011101101`;
  * the information bits run N, reserved, LSP−1, rsv, LTP−1, rsv, 16 zero SP, 16 zero TP,
    16 × (7-bit value + rsv), mask 0x FFFF, mask 0x0007.
* CPt lengths: see 3.6.

---

## 13. Implementation notes

### 13.1 What is new compared with V.90 phase 3 (9.3/V.90)

* **Upstream start-up signals are 8 kHz PAM, not V.34 QAM.**

  | V.90 | V.92 |
  |---|---|
  | S / S̄ (128T / 16T) | Ru / R̄u (384T / 24T) |
  | PP | dropped |
  | TRN, 4-point QAM ≥ 512T | TRN1u, 2-point ±LU ≥ 2040T |
  | Ja, J-style 4-point QAM (10.1.3.3/V.34) | Ja, 2-point, GPA-scrambled, differential, with a **24-ones preamble** |

* **MD length unit:** 34.5 ms (276 symbols) instead of 35 ms.
* **DIL descriptor:**
  * adds a 19-rate **upstream** mask in two new 16-bit blocks before the CRC;
  * pads to **a multiple of 12 bits** instead of V.90's "even length";
  * Ja is cut at a 12-bit boundary rather than at once.
* **Jd:**
  * bit 47 becomes the **Jd/Jp identifier**, and bit 48 is reserved;
  * in V.90 both bits chose 4 or 16 points for CP, E and SCR;
  * a V.90 Jd parser reused unchanged would read every Jp as "16-point".
* **New signals:** Jp (sampling-phase ε and TRN2u sizes), Su/S̄u (phase-measurement signal),
  E1u, and SCR from the **digital** modem (N = 0).
* **Jp' replaces J'd.** The construction is the same: 12 zeros, initialized from the preceding
  Jp.
* **The analogue modem must send TRN1u during DIL.** In V.90 it could send silence or SCR.
* **DIL is stopped by CPt,** not by a second S-to-S̄ reversal. When N = 0 the digital modem sends
  SCR and waits until it is trained. In V.90 it went straight to phase 4.
* **CPt moves into phase 3** and is sent with TRN1u modulation. The Ri/R̄i exchange around it
  also moves into phase 3, and E1u ends it.
  * In V.90, phase 4 began with Ri (≥ 192T), then CPt, then the "optional SCR ≤ 4000 ms".
* **The CPt layout is not V.90's CP.**
  * V.92 bit 18 = "CP" = 0 (reserved in V.90).
  * **Type is 2 bits at 19:20** (V.90: 1 bit at 19).
  * **drn moves to bits 21:25** (V.90: 20:24).
  * Reserved bits are 26:30. There is **no silence-request bit** (V.90 bit 30); in V.92 it lives
    in SUV.
  * The ack bit refers to **CPd**, not MP.
  * Bits 36:48 are **reserved** (V.90: upstream rate mask; in V.92 that mask is in Ja).
  * Fill runs to **a multiple of 12 symbols** (V.90: exactly 3 fill bits "000").
  * **`v90::sequences::Cp` must not parse CPt unchanged.**
* **New digital-side Ja watchdog** DR1 (4500 ms + RTD from INFO1a).
* **Circuit 107** comes ON at Jp detection on the analogue side, and just before Jp' on the
  digital side.
* **Upstream frame alignment** is set in phase 3, at the second TRN1u. There are 12-symbol
  frames upstream against 6 downstream.

### 13.2 Pitfalls

1. **Opposite sign conventions.**
   * Upstream: scrambler output 0 = +LU.
   * Downstream: sign bit 0 = negative.
   * Differential encoders start from "the final symbol". Turn that symbol back into a bit with
     **the preceding signal's** mapping.
2. **Scrambler by side, not role.** The analogue modem is always GPA and the digital modem
   always GPC, even when the analogue modem placed the call.
3. **Ceilings in α, β and ⌈N/2⌉.** The `.txt` loses them. Odd N leaves 9 reserved bits. With
   N = 0 the H and REF blocks are still present.
4. **Frame hunting.**
   * Ja descriptors, CPt and Jd/Jp are found by their 17-ones sync after descrambling and
     differential decoding.
   * Fill of up to 11 zeros can sit between repeats.
   * A CPt run ends in E1u (12 zeros). Tell the two apart only at a CPt boundary.
   * Always check the CRC and the start bits. Jd and Jp share the sync and are told apart **only
     by bit 47**.
5. **Fractional S̄u.**
   * 24.5T and (24+ε)T are not whole symbols. The upstream transmitter's output timing must be
     adjustable in steps finer than a symbol, down to T/65536 as coded.
   * After the change, the second TRN1u and everything later must stay on the new grid.
   * The digital modem's receiver must follow a timing step of 0.5T and then ε.
6. **UINFO range.** Sd needs Ucode 16 + UINFO ≤ 127, so choose UINFO in 67..111. Its power is
   also capped by the digital modem's maximum.
7. **Long round-trip VoIP lines.** Project memory: about 1.5 s each way.
   * TR3 (1500 ms from the start of Ja, with no RTD) **cannot hold** once RTD exceeds about
     0.9 s. The digital modem may also wait 500 ms.
   * The V.90 analogue code already stretches the same V.90 timer to 2 s + 2·RTD
     (`SD_WAIT` in `v90/analogue.rs`). Do the same here and document the departure.
8. **Live servers bend the V.90 timers** (`v90/digital.rs` `Habits::LIVE_SERVER`, and
   `JD_LATEST` / `S_AFTER_JD` in `v90/analogue.rs`).
   * Observed: about 4 s of TRN1d, then a wait for S that ignores RTD.
   * The V.90 fix was to start S before Jd could arrive. **That fix does not carry over to V.92**:
     * Su is exactly 144T before its S̄u reversal;
     * the digital modem must **see** that reversal (D6) before it will send Jp;
     * an early reversal it missed would leave both ends stuck until TR5/TR6 fire.
   * Treat this as a live-test question (13.4).
9. **VoIP jitter slips** (about 20 ms inserted every few seconds, per project memory).
   * A slip breaks the digital modem's mod-12 upstream count, the downstream 6-symbol alignment
     at the analogue modem, and DIL segment alignment.
   * A slip during Su corrupts ε.
   * Receivers must be able to resynchronize: re-hunt for R, Jd/Jp sync and CPt sync. Estimates
     should use medians.
10. **The analogue modem's RTD.**
    * T5 needs it, and full phase 2 (V.90/V.34) provides it.
    * Short phase 2 defines only the digital modem's RTDEd (9.4.1.1.4). The analogue side needs
      its own estimate; if it has none, use 0, which is the strictest.
11. **The ack bit of CPt.** Send 0: CPd does not exist yet in phase 3.
12. **Ri timing.**
    * R/R̄ are not differentially encoded, so detect them in either polarity.
    * The analogue modem must detect Ri while it is sending TRN1u (N ≠ 0), or while sending CPt
      and waiting for R̄i (N = 0). Its echo canceller must already be trained.
13. **Code that can be reused:**
    * `v34::info::crc`
    * `v32::Scrambler`
    * `v90::sequences::{frame, unframe, Descriptor}`. Extend the descriptor with the two mask
      blocks and the 12-bit fill, and keep a V.90 variant.
    * `v90::dil` (DIL design and analysis)
    * `v90::digital::Source` (Sd, TRN1d, Jd-style differential signing, DIL, Ri/R̄i). Needs new
      Jp, Jp' and SCR states, and a Jd with bit 47 = 0 and bit 48 reserved.
    * `v90::pcm` (the downstream receiver; Sd hunting is unchanged).

    Genuinely new work:
    * an 8 kHz upstream PAM transmitter with a fractional-delay output;
    * the digital modem's upstream PAM receiver and Su phase estimator;
    * a V.92 CPt codec;
    * the phase 3 state machines above.

### 13.3 Ambiguities in the text

1. **Su length.** 8.5.6 says Su and S̄u are whole multiples of 12 symbols, but phase 3 uses 144T
   (fine), 24.5T and (24+ε)T. Read the multiple-of-12 rule as applying to the nominal
   symbol-aligned segments, with the half-symbol and ε as deliberate timing shifts.
   * Open point: whether the "Su" run between the two S̄u segments must hold a whole number of
     12-symbol periods is not stated.
2. **Is the analogue transmitter's half-symbol shift from the 24.5T S̄u permanent?** It is, if
   the output keeps going continuously. ε is then measured by the digital modem after that
   shift, since it keeps measuring on the second Su (D7). So the total shift from the original
   grid is 0.5T + εT, with ε chosen for the grid after the shift. Worth confirming against a
   capture.
3. **Resetting the scrambler before the second TRN1u.** 8.5.7 applies to "TRN1u" without saying
   which one. Resetting makes the second TRN1u a known training reference, and the descrambler
   locks either way.
4. **Figure 11 against the text for N = 0.**
   * The text: SCR, then Ri from the digital modem when it is trained, then CPt from the analogue
     modem, then R̄i from the digital modem, then E1u.
   * Figure 11 copies Figure 10's arrows: CPt leads to Ri, Ri leads to E1u, E1u leads to R̄i.
   * **Follow the text.** A robust digital modem accepts both orders: after sending Ri, if CPt
     arrives, send R̄i at once.
5. **DR2's reference.** "In 9.5.1.1.9" should be read as the Su detection of 9.5.1.1.4/6.
6. **Table 23 fill.** The fill-bit rows both start at 289+δ. Take one fill zero at 289+δ, then
   zeros from 290+δ up to a multiple of 12.
7. **Qa.b unsigned range.** 3.5 says [0, 2^(a+1)), but the width is a+b bits. The CPt pattern
   "xxx.xxxxxxxxxxxxx" (3 integer bits) fixes Q3.13 = uint16/8192, range [0, 8).
8. **"Beginning of MD" when there is no MD** (T5). Measure from the end of the first R̄u.
9. **MD wait in D1.** The text does not say whether the wait includes the 24T R̄u that follows
   the reversal. Hunt for Ru over a window that covers both readings.
10. **"Within 5000 ms of transmitting S̄u"** (T16). The start of that S̄u is the stricter reading.
11. **A digital modem that has not found its phase.** D8 depends on it having worked out ε.
    No limit is given apart from TR5 and the analogue modem's own timers; the analogue modem has
    no Jp watchdog.

### 13.4 Open questions (for Rory or for live testing)

1. With a Table 19 INFO1a (V.34 upstream after short phase 2), is phase 3 exactly V.90's 9.3,
   with V.90 Jd semantics (bits 47/48 = 16-point flags)? Or does the V.92 Jd layout apply? 9.5
   seems to assume PCM upstream (TRN1u, Ja with an upstream PCM rate mask).
2. Does V.90's CP power limit (Table 15/V.90, and the 3 dB data-mode-over-phase-4 rule of
   8.5.2/V.90) still bind CPt and CPu in V.92? V.92 does not restate it. The digital modem still
   has to respect its maximum transmit power.
3. How long may a digital modem keep sending DIL while waiting for CPt, and SCR/Ri in the N = 0
   case, before giving up? What do real V.92 servers do?
4. Do real V.92 servers wait for Su with RTD included (TR2), or not, as the V.90 server did? Is
   there a safe way to make Su arrive in time on a long-delay line, given the fixed 144T
   reversal?
5. How do real servers pick ε (the phase estimator), and how precisely? Does a VoIP path, with
   its resampling and jitter buffer, even keep a stable sampling phase for PCM upstream? That
   decides whether PCM upstream is worth attempting on Rory's lines at all.
6. Should our analogue modem ask for N = 0 (SCR) when the downstream is already known, for
   example on a retrain? Would servers then wait "until sufficiently trained" for long?
7. Is the 24.5T S̄u meant to leave a lasting half-symbol offset in the transmitter (13.3 item 2)?
   A capture of a real V.92 client would settle it.
