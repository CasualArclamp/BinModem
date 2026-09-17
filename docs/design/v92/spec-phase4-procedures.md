# V.92 Phase 4 (final training) and retrains: implementation digest

Source: ITU-T V.92 (11/2000), clauses **9.6** (Phase 4, final training) and **9.7** (retrains),
for both the digital and the analogue modem. The source is `docs/specs/T-REC-V.92-200011-I.pdf`,
PDF pages 56 to 60 (printed pages 49 to 53).

Every table, bit layout, figure and number below was read from rendered PDF pages, not from
the extracted text. The extracted text is visibly wrong in several places here:

- Table 23's "Type" and "drn" rows are shifted.
- Tables 28 and 29 have their rows displaced by one.

To make this file stand alone, it also covers the Phase 4 signals that 9.6 uses (V.92 8.7
and 8.8), and the V.90 and V.34 text that 8.7, 8.8, 9.6 and 9.7 point to.

Pages rendered and read:

| Document | PDF pages read | What they hold |
|---|---|---|
| V.92 | 9 to 13 | Definitions, clauses 5 and 6, clause 8 preamble |
| V.92 | 26 to 44 | INFO1a, Phase 3 signals, 8.7, 8.8 |
| V.92 | 50 to 58 | 9.3 to 9.6 |
| V.92 | 59 to 60 | 9.7 |
| V.92 | 61 to 66 and 69 | 9.8 to 9.11, for cross-references only |
| V.90 | 13, 14, 20, 30 to 36, 38 to 40, 43 to 45 | Referenced V.90 text |
| V.34 | 16, 32, 33 | Scrambler, tones A and B, CRC |

Markers used below:

- **[SHALL]**: a mandatory requirement.
- **[SHOULD]**: a recommendation.
- **[MAY]**: an option.
- **[INFERRED]**: this digest's own reading. It is not stated in the text.
- **[AMBIGUOUS]**: the text is unclear or inconsistent. Each one is also listed in section 10.3.

---

## 1. Notation and units

- **T** is one symbol interval. Both directions run at 8000 symbols/s in V.92 PCM-upstream mode:
  - downstream per V.90 5.2;
  - upstream per V.92 6.2, "derived from the digital network".
  - So 1T = 125 µs.
  - Useful conversions: 24T = 3 ms, 384T = 48 ms, 2040T = 255 ms, 12000T = 1.5 s.
- **Downstream data frame** (digital modem to analogue modem): 6 symbols, intervals i = 0..5 (V.90 5.4).
- **Upstream data frame** (analogue modem to digital modem): 12 symbols, intervals i = 0..11 (V.92 6.4, Figure 1).
  - Figure 1 also shows a constellation frame of 6 symbols (j = 0..5, twice per data frame).
  - It also shows a trellis frame of 4 symbols (k = 0..3, three per data frame).
- **RTD** means round-trip delay.
  - The digital modem's estimate is **RTDEd**. It is measured in Phase 2: in V.92 9.4.1.1.4 for short Phase 2, or in V.90 9.2.1.1.4 for full Phase 2.
    - It is the time from the Tone B phase reversal leaving the digital modem's line terminals to the received Tone A phase reversal, minus 40 ms.
  - The analogue modem's estimate is **RTDEa**, defined only in full Phase 2 (V.90 9.2.2.1.4).
    - It is the time from sending the Tone A phase reversal to receiving the Tone B phase reversal, minus 40 ms.
    - See section 10, question Q1: after a short Phase 2 the analogue modem has no RTD estimate.
- **Bit numbering in sequences** follows V.92 clause 8:
  - Bit 0 is transmitted first.
  - In Tables 2 to 5, 11 to 24, 27 and 30 to 33, a value written as a bit pattern goes leftmost bit first.
  - A value written as an integer goes least-significant bit first.
  - Tables 28 and 29 (TRN2u mapping) are **not** in that list. See section 10, question Q2.
- **Qa.b format** (V.92 3.5):
  - **Signed Qa.b** is (a+b+1)-bit two's complement with b fraction bits, range [-2^a, 2^a).
  - **Unsigned Qa.b** is (a+b) bits with b fraction bits. The text gives its range as [0, 2^(a+1)), which does not match an (a+b)-bit field; see Q9.
- **L_U** (V.92 3.8) is chosen so that TRN1u goes out at the desired data-mode transmit power.
- **U_INFO** is the Ucode that INFO1a carries in bits 25:31 (it shall be > 66).
  - Ucodes and Uchords are as in V.90 clause 3 and V.90 Table 1.
- The subscripts follow the text:
  - **d** marks a digital-modem signal;
  - **u** marks an analogue-modem (upstream) signal;
  - a prime (for example CPd′ or SUVu′) marks a sequence with its acknowledge bit set to 1.

---

## 2. Where Phase 4 sits

### 2.1 Entry into Phase 4 (end of V.92 Phase 3, 9.5)

**Digital modem**

- 9.5.1.1.12 (non-zero DIL): when it receives CPt, the digital modem sends Ri. When it receives the E1u that ends the CPt sequences, it sends R̄i and goes to Phase 4.
- 9.5.1.1.13 (zero-length DIL): the digital modem sends Ri. On receiving CPt it sends R̄i and goes to Phase 4.
- Signal definitions (V.90 8.6.4, reused by V.92 8.6.5):
  - Ri repeats a 6-symbol PCM sequence with signs `+ + + - - -`, leftmost sign first. Every symbol uses the one codeword whose Ucode is U_INFO.
  - R̄i is exactly 4 repetitions (24T) with signs `- - - + + +`.
  - Neither signal is differentially encoded, so a receiver has to detect them in either polarity.

**Analogue modem**

- 9.5.2.1.10 (zero-length DIL) and 9.5.2.1.11 (non-zero DIL): on receiving R̄i (or Ri, as those clauses say), the analogue modem finishes the current CPt, sends **E1u**, and goes to Phase 4.
- E1u (V.92 8.5.2) is one data frame of scrambled, differentially encoded zeros, sent with the same modulation as CPt.
  - That modulation is the 2-point ±L_U modulation of TRN1u.
- By this point the digital modem already holds the **CPt** parameters: the constellations, drn, Sr, ld and spectral-shaping filter used for its Phase 4 transmissions (V.90 8.6.5).
- The upstream data-frame count modulo 12 runs from the first symbol of TRN1u (V.92 9.5.1.1.10, 8.5.7).

### 2.2 Exit from Phase 4

Both modems are in data mode:

- Circuit 106 follows circuit 105.
- Circuit 104 is unclamped.
- Circuit 109 is ON.

### 2.3 Other users of the 9.6 exchange

The core of 9.6, from 9.6.x.1.2 onwards, is **reused**:

- **9.8 Rate renegotiation**:
  - 9.8.1.1.2, 9.8.1.1.4 and 9.8.1.1.5 continue at 9.6.1.1.2;
  - 9.8.2.1.3, 9.8.2.1.5 and 9.8.2.1.6 continue at 9.6.2.1.2.
- **9.9 Fast parameter exchange (FPX)**:
  - 9.9.1.1.2 and 9.9.1.2.3 continue at 9.6.1.1.2;
  - 9.9.2.1.2 and 9.9.2.2.3 continue at 9.6.2.1.2.

So build the SUV/CP/E exchange as one reusable state machine that takes a **context** of Training, RateRenegotiation or FPX. The context decides:

- the modulation (TRN2 modulation, or the preceding data-mode modulation for FPX);
- whether CPd bit 29 may be set (only in Training);
- whether FB1u comes before B1u (only in FPX).

---

## 3. Common framing rules for SUV, CP and E sequences

These apply to CPd, CPu, CPt, CPus, SUVd and SUVu.

### 3.1 Layout

1. **Frame sync**: bits 0:16 are seventeen 1s, `11111111111111111`.
2. **Start bits** are 0. They sit at every bit position that is a multiple of 17: 17, 34, 51, 68, and so on.
   - This holds in every table below, including the variable-length parts, because each variable word is 17 bits (1 start bit and 16 data bits) and α, β, γ and δ are all multiples of 17.
   - In SUVd, SUVu and CPus, position 51 holds a **fill bit (0)** rather than a start bit, because the CRC finishes at bit 50.
3. **Bit 18 tells the sequence type apart:** 0 means a CP sequence (CPd, CPu, CPt, CPus), 1 means an SUV sequence.
4. **Acknowledge bit** [SHALL]:
   - Bit 33 in CPu, CPt, CPus, CPd, SUVu and SUVd.
   - In CPd and SUVd it means "received a CPu from the analogue modem".
   - In CPu, CPus and SUVu it means "received a CPd from the digital modem".
5. **CRC**: 16 bits after the last start bit, followed by **one fill bit 0**. Then more fill 0s extend the sequence to a symbol multiple:
   - upstream sequences (CPu, CPt, CPus, SUVu): the next multiple of **12 symbols**;
   - downstream sequences (CPd, SUVd): the next multiple of **6 symbols**.
   - See 4.2 and 4.10 for what that means in bits.
6. **Grouping** [SHALL]:
   - When several CPd and CPd′ go out as a group, they carry identical information. The same holds for CPu/CPu′, SUVd/SUVd′ and SUVu/SUVu′ (8.7.3, 8.7.5, 8.8.3, 8.8.5).
   - [INFERRED] Only the acknowledge bit (and so the CRC) may differ inside a group, since the prime notation exists to mark it.
7. **CRC generator** (V.34 10.1.2.3.2; V.92 8.7.3, 8.7.5, 8.8.3 and 8.8.5 all point to it):
   - The polynomial is x^16 + x^12 + x^5 + 1.
   - **Input**: every information bit of the sequence in transmission order. Frame-sync bits, start bits and fill bits are **excluded**; reserved bits **are** included.
     - With the layouts here, that is every bit from 18 up to the bit before the CRC's own start bit, except positions that are multiples of 17.
   - **Register**: 16 cells, numbered 15 on the left to 0 on the right, shifting right (V.34 Figure 14).
     - It is loaded with all ones before the first bit.
     - For each input bit: `f = cell0 XOR in`. Every cell shifts right. `f` enters cell 15 and is also XORed into cells 10 and 3.
     - Equivalent code: `reg = (reg >> 1) ^ (f ? 0x8408 : 0)`, where `f = (reg & 1) ^ bit`.
   - **Output**: the register as it stands, starting with cell 0, which is the LSB. Nothing is inverted.
   - This is already implemented as `crc()` in `crates/datapump/src/v34/info.rs`.

---

## 4. Signals used in Phase 4

### 4.1 Scramblers

- **Analogue modem, GPA** (V.92 6.3; V.34 7-2): 1 + x^-5 + x^-23.
  - It is self-synchronising: `out = in XOR out[-5] XOR out[-23]`.
  - The transmitter divides by the polynomial (V.34 clause 7).
- **Digital modem, GPC** (V.92 clause 5, which points to V.90 5.3; V.34 7-1): 1 + x^-18 + x^-23.

Which sequences reset the scrambler, all [SHALL]:

| Signal | Scrambler | Other state reset before the signal |
|---|---|---|
| TRN2u | Reset at its start (8.7.6) | Differential encoder memory is **initialised to the last sign bit of the preceding E1u**, not to zero (8.7.6) |
| SUVu, CPu, E2u | Continue, no reset (8.7.2, 8.7.3, 8.7.5) | Differential encoder carries on from the preceding sequence's last sign bit |
| B1u | Reset to zero (8.7.1) | Modulus encoder, convolutional encoder, precoder and prefilter memories reset to zero |
| FB1u | Not stated; [INFERRED] continues the data-mode scrambler | — |
| TRN2d | Reset to zero (V.90 8.6.5) | Differential encoder and spectral-shape filter memory reset to zero |
| SUVd, CPd, Ed | Continue (8.8.2, 8.8.3, 8.8.5) | Differential encoder carries on from the preceding sequence's last sign bit |
| B1d | Reset to zero (V.90 8.6.1) | Differential encoder and spectral-shape filter memory reset to zero |

### 4.2 Upstream modulation of TRN2u, SUVu, CPu and E2u (V.92 8.7.2 to 8.7.6)

**Constellation size and bits per symbol**

- TRN2u uses a **4-point or 8-point** constellation, chosen by the digital modem in **Jp** (Table 22):
  - bit 48 sets the size during training: 0 means 4-point, 1 means 8-point;
  - bit 49 sets the size during rate renegotiation, with the same coding.
- Each symbol therefore carries 2 or 3 scrambled bits.
- Each symbol is a direct linear value. The precoder and prefilter are not involved, as far as the text says.
  - [AMBIGUOUS] The text never says whether TRN2u goes through the precoder or prefilter. For Ru it says explicitly that they are bypassed (8.5.5).

**Table 28/V.92, 4-point TRN2u** (group written MSB:LSB):

| MSB:LSB | Linear value |
|---|---|
| 00 | +(1/√5) × L_U |
| 01 | +(3/√5) × L_U |
| 10 | −(1/√5) × L_U |
| 11 | −(3/√5) × L_U |

**Table 29/V.92, 8-point TRN2u** (group written MSB:LSB):

| MSB:LSB | Linear value |
|---|---|
| 000 | +(1/√21) × L_U |
| 001 | +(3/√21) × L_U |
| 010 | +(5/√21) × L_U |
| 011 | +(7/√21) × L_U |
| 100 | −(1/√21) × L_U |
| 101 | −(3/√21) × L_U |
| 110 | −(5/√21) × L_U |
| 111 | −(7/√21) × L_U |

Both constellations have a mean square of exactly L_U² (the average of 1 and 9 over 5 is 1; the average of 1, 9, 25 and 49 over 21 is 1).

**Sign handling**

- The MSB is the sign: 0 is positive, 1 is negative.
- It is differentially encoded [SHALL]: the transmitted sign is the present scrambled sign bit XOR the previously transmitted sign bit.
- The remaining bits pick the magnitude directly.
- [AMBIGUOUS, Q2] The text never says which of the 2 or 3 bits of a symbol comes first in time. The mapping tables label the group MSB:LSB, but they are not in clause 8's list of LSB-first tables.

**Lengths**

- Sequence length is always a multiple of 12 symbols. TRN2u "shall be an integer multiple of 12 symbols".
- For SUV, CP and E, 12 symbols means 24 bits with 4 points, or 36 bits with 8 points.
- SUVu and CPus: the 52 fixed bits pad to **72 bits** under either size. That is 36 symbols with 4 points, 24 symbols with 8 points.
- CPu (long): 290 + δ bits (bits 0 to 289+δ), padded up to the next multiple of 24 or 36.

**Other points**

- During FPX, SUVu, CPu (long and short) and E2u use the **preceding data-mode modulation** instead of the TRN2u modulation (8.7.2, 8.7.3, 8.7.5).
- TRN2u "may be used to estimate the analogue channel for upstream" [MAY]. This is the digital modem's source for designing the CPd precoder, prefilter and constellations.

### 4.3 TRN2u (8.7.6)

- Scrambled binary ones, mapped with Table 28 or 29.
- The scrambler is reset at its start.
- The sign differential encoder is initialised from the last sign bit of E1u.
- Length is a multiple of 12 symbols.
- In initial Phase 4 it lasts **at least 12000T** unless SUVd arrives first (9.6.2.1.1).
- In rate renegotiation it lasts at most 16008T, and may stop after 2400T or when SUVd arrives (9.8.2.1.2). This is noted only for the shared state machine.

### 4.4 SUVu: Table 27/V.92, short information sequence (8.7.5)

| Bits (LSB:MSB) | Meaning |
|---|---|
| 0:16 | Frame sync, 17 ones |
| 17 | Start bit 0 |
| 18 | SUVu identifier = **1** |
| 19:25 | Reserved for the ITU: the analogue modem sets them to 0; the digital modem does not interpret them |
| 26 | 1 = the analogue modem asks the digital modem to **wait for a CPu before sending a CPd**. The digital modem does not have to comply [MAY] |
| 27:31 | 20·log10(L), where L is the measured RMS level of the prefilter output multiplied by G. Signed Q2.2 (5 bits, `sxx.xx`), range −4.00 to +3.75. **The value 16 (bit pattern 10000, −4.00) means no measurement was taken.** |
| 32 | 1 = a silent period is requested. It may be used during rate renegotiation (9.8.2.1) |
| 33 | Acknowledge: 0 = no CPd received from the digital modem yet; 1 = a CPd has been received. SUVu with this bit set is written **SUVu′** |
| 34 | Start bit 0 |
| 35:50 | CRC |
| 51 | Fill bit 0 |
| 52:… | Fill 0s up to the next multiple of 12 symbols |

- Scrambled and sent with the TRN2u modulation during training and rate renegotiation. During FPX it uses the preceding data-mode modulation.
- The differential encoder continues from the preceding sequence.
- [INFERRED] In initial Phase 4:
  - the analogue modem has no active prefilter or G yet, so bits 27:31 should normally be 16;
  - bit 32 should be 0. 9.9 says explicitly that FPX uses bit 32 clear.

### 4.5 CPu (long) and CPt: Table 23/V.92 (8.7.3; CPt in 8.5.1)

A long CPu carries the downstream (digital modem) data-mode parameters. It shares the layout of CPt.

| Bits (LSB:MSB) | Meaning |
|---|---|
| 0:16 | Frame sync, 17 ones |
| 17 | Start bit 0 |
| 18 | CP identifier = **0** |
| 19:20 | Type: **0 = CPt, 1 = CPu**. The value 2 means CPus (Table 24) |
| 21:25 | drn, the selected downstream (digital to analogue) data signalling rate, an integer 0 to 22. **drn = 0 means cleardown.** Rate = (drn+20)×8000/6 in CPu (drn 1 to 22 gives 28 000 to 56 000 bit/s); rate = (drn+8)×8000/6 in CPt (12 000 to 40 000 bit/s) |
| 26:30 | Reserved: the analogue modem sets them to 0; the digital modem ignores them |
| 31:32 | Sr, the number of sign bits used as redundancy for spectral shaping |
| 33 | Acknowledge: 0 = no CPd received yet; 1 = a CPd has been received. CPu with this bit set is written **CPu′** |
| 34 | Start bit 0 |
| 35 | Codec type: 0 = µ-law, 1 = A-law |
| 36:48 | Reserved: the analogue modem sets them to 0; the digital modem ignores them. *(In V.90 these bits held the upstream rate mask; V.92 moved it into the Ja DIL descriptor, Table 20.)* |
| 49:50 | ld, the number of look-ahead frames requested for spectral shaping. It shall be consistent with the digital modem's capability announced in **Jd** bits 49:50 (a value 1 to 3) |
| 51 | Start bit 0 |
| 52:67 | RMS of TRN1d at the transmitter output divided by RMS of TRN1d at the codec D/A output. Unsigned Q3.13 |
| 68 | Start bit 0 |
| 69:76 | a1 of the spectral-shaping filter, signed Q1.6 (8 bits) |
| 77:84 | a2, signed Q1.6 |
| 85 | Start bit 0 |
| 86:93 | b1, signed Q1.6 |
| 94:101 | b2, signed Q1.6 |
| 102 | Start bit 0 |
| 103:106 | Index (0 to 5) of the constellation used in downstream data-frame interval 0 |
| 107:110 | The same for interval 1 |
| 111:114 | The same for interval 2 |
| 115:118 | The same for interval 3 |
| 119 | Start bit 0 |
| 120:123 | The same for interval 4 |
| 124:127 | The same for interval 5 |
| 128 | 1 = the constellations at the transmitter differ from those at the codec D/A output |
| 129:135 | Reserved: the analogue modem sets them to 0; the digital modem ignores them |
| 136 | Start bit 0 |
| 137:152 | Constellation mask for Uchord1 (bit 137 is Ucode 0) |
| 153 | Start bit 0 |
| 154:169 | Mask for Uchord2 (bit 154 is Ucode 16) |
| 170 | Start bit 0 |
| 171:186 | Mask for Uchord3 (bit 171 is Ucode 32) |
| 187 | Start bit 0 |
| 188:203 | Mask for Uchord4 (bit 188 is Ucode 48) |
| 204 | Start bit 0 |
| 205:220 | Mask for Uchord5 (bit 205 is Ucode 64) |
| 221 | Start bit 0 |
| 222:237 | Mask for Uchord6 (bit 222 is Ucode 80) |
| 238 | Start bit 0 |
| 239:254 | Mask for Uchord7 (bit 239 is Ucode 96) |
| 255 | Start bit 0 |
| 256:271 | Mask for Uchord8 (bit 256 is Ucode 112) |
| 272 : 271+γ | Further constellations, each in the same 136-bit format as bits 136:271 |
| 272+γ : 271+δ | The matching codec (D/A-output) constellations in the same format. Present only if bit 128 = 1 |
| 272+δ | Start bit 0 |
| 273+δ : 288+δ | CRC |
| 289+δ | Fill bit 0 |
| 289+δ : … | Fill 0s up to the next multiple of 12 symbols (the table's own row numbering; see Q7) |

Rules for the variable part (8.7.3):

- A constellation mask is 128 bits. A 1 bit means the constellation contains the PCM code of the corresponding Ucode.
- A constellation used in several intervals is sent only once.
- Sent constellations are indexed from 0 (at bits 136:271) up to 5 (at bits 816:951).
- If the transmitter constellations differ from the D/A-output ones, bit 128 **shall** be 1, and the D/A-output constellation for every transmit constellation **shall** be sent.
- γ = 136 × (the largest index found in bits 103:127).
- δ = 2γ + 136 when bit 128 = 1, and δ = γ when bit 128 = 0.
- Modulation: during training and rate renegotiation, the TRN2u modulation, with the differential encoder continuing from the last sign bit of the preceding sequence. During FPX, the data-mode modulation.
- Constraints on constellation power that V.90 imposes on its CP (V.90 8.5.2 and Table 15) are **not repeated** in V.92 8.7.3. See section 10.3, Q14.

**CPt** uses the same layout with bits 19:20 = 0. CPt belongs to Phase 3, not Phase 4:

- It is sent with the TRN1u (2-point) modulation.
- It is preceded by 24 differentially encoded ones (8.5.1).

What CPt matters for here: its drn, Sr, ld, a/b coefficients and constellations set how the digital modem modulates **TRN2d, SUVd, CPd and Ed** (see 4.10).

### 4.6 CPus: Table 24/V.92, short CPu (8.7.3)

CPus is used only in rate renegotiation and FPX, when the digital modem's modulation parameters do not change. It is listed here because the shared 9.6 state machine has to accept it wherever it accepts a CPu.

| Bits | Meaning |
|---|---|
| 0:16 | Frame sync, 17 ones |
| 17 | Start bit 0 |
| 18 | CP identifier = 0 |
| 19:20 | Type = **2** (CPus) |
| 21:25 | drn, 0 to 22, where 0 means cleardown. Rate = (drn+20)×8000/6 |
| 26:32 | Reserved: the analogue modem sets them to 0; the digital modem ignores them |
| 33 | Acknowledge (received a CPd) |
| 34 | Start bit 0 |
| 35:50 | CRC |
| 51 | Fill bit 0 |
| 52:… | Fill 0s up to the next multiple of 12 symbols |

Modulation: the TRN2u modulation during rate renegotiation, with the differential encoder continuing from the preceding sequence. The data-mode modulation during FPX.

### 4.7 E2u (8.7.2)

- One upstream **data frame** (12 symbols) of scrambled, differentially encoded **zeros**.
- It uses the TRN2u modulation in training and rate renegotiation, and the preceding data-mode modulation in FPX.
- **E2u shall be one symbol longer (13 symbols) if CPd bit 29 = 1** [SHALL].
- CPd bit 29 shall be 0 in rate renegotiation and FPX, so the extension happens only in initial training (and after retrains).
- [INFERRED] The extension lets the digital modem move the upstream frame boundary by one symbol, so that B1u starts on its data-frame interval 0 (8.7.1). The text gives no reason for it.
- [INFERRED] How a receiver tells E2u apart: after a complete SUVu or CPu (whose length is known from its contents), a run of zeros appears where the next frame sync (17 ones) was expected.

### 4.8 B1u (8.7.1)

- **48 data frames** (48 × 12 = 576 symbols) of scrambled binary **ones**.
- It uses the data-mode constellation parameters from the preceding CPd.
- The first transmitter output symbol of the first frame is n = 0 in the precoder and prefilter equations of 6.4.2.
- **The first symbol of B1u shall begin data-frame interval 0** [SHALL].
- Before B1u, the scrambler, modulus encoder, convolutional encoder, precoder and prefilter memories are reset to zero.
  - [INFERRED] The modulus-encoder memory is the differential sign state d(f−1) of 6.4.1.
- [INFERRED] The bits per upstream data frame are K = rate × 12 / 8000. Since the upstream rate is (drn_d + 17) × 8000/6, K = 2 × (drn_d + 17), which runs from 36 to 72 bits (6.1, 6.4.1).

### 4.9 FB1u (8.7.7)

- 48 data frames of scrambled, differentially encoded binary ones, sent with the **preceding** data-mode modulation.
- It is used only in FPX, between E2u and B1u (9.6.1.1.6, 9.6.2.1.5, Figure 19).

### 4.10 Downstream modulation of TRN2d, SUVd, CPd and Ed

**Encoder**

- TRN2d is defined in V.90 8.6.5, which V.92 8.8.6 adopts.
- TRN2d is made by feeding **scrambled binary ones into the V.90 5.4 encoder**:
  - modulus encoder;
  - M_i-point map;
  - sign assignment;
  - spectral shaper with differential encoder;
  - using the **constellation set passed in CPt**.
- **Symbols in the first data frame** of TRN2d [SHALL]: their magnitudes are those produced by mapping the first D scrambled ones after the scrambler reset, and they are identical for every value of ld.
- **Permitted K and S** come from **Table 17/V.90**:
  - K runs from 6 to 24;
  - S runs from 3 to 6 for each K;
  - so K + S runs from 9 to 30, which is 12 to 40 kbit/s.
- **Length** [SHALL]: TRN2d is an integer multiple of 6 symbols.

**Bits per frame**

- With CPt drn = drn_t, the Phase 4 downstream rate is (drn_t + 8) × 8000/6.
- So **D = K + S = drn_t + 8 bits per 6-symbol frame**, with S = 6 − Sr (Sr from CPt; V.90 5.4.1) and K = D − S.
- V.90 5.4.2 splits each frame's D bits: the first S bits are sign bits, the remaining K bits go to the modulus encoder.

**Spectral shaping**

- V.90 8.6 says the Phase 4 TRN2d, MP and Ed in initial training and retrain use the CPt spectral-shaping parameters.
- In rate renegotiation they use the preceding data-mode shaping with the K previously derived from CPt.
- V.92 8.6 only says that *Phase 3* digital signals are unshaped.
- [INFERRED] Apply the V.90 8.6 rule to TRN2d, SUVd, CPd and Ed. See section 10.3, Q17.

**Sequence lengths in bits**

- SUVd and CPd are padded to a multiple of 6 symbols, which is a multiple of **D bits**.
- Ed is exactly 2 frames, which is **2D bits**.

**Sequences as encoder input** [INFERRED]

- SUVd, CPd and Ed are "scrambled and transmitted using the corresponding TRN2d modulation". That is, their bits replace the ones fed into the same encoder, scrambler state carries on, and the differential encoder continues (8.8.3, 8.8.5).
- During FPX they use the preceding data-mode modulation instead.

### 4.11 TRN2d (V.92 8.8.6, pointing to V.90 8.6.5)

- See 4.10 for how it is built.
- In initial Phase 4 it lasts **at least 2040T** (9.6.1.1.1), with no stated maximum.
- V.90 had an MP deadline of 2000 ms after the start of TRN2d. V.92 has nothing equivalent.
- For comparison, rate renegotiation allows up to 16008T (9.8.1.1.2).

### 4.12 SUVd: Table 31/V.92 (8.8.5)

| Bits | Meaning |
|---|---|
| 0:16 | Frame sync, 17 ones |
| 17 | Start bit 0 |
| 18 | SUVd identifier = **1** |
| 19:31 | Reserved. The table literally says "set to 0 by the analogue modem and not interpreted by the digital modem", which is an editorial slip. Read it as: **the digital modem sets them to 0 and the analogue modem ignores them** |
| 32 | 1 = a silent period is requested. It may be used during rate renegotiation (9.8.1.1) |
| 33 | Acknowledge: 0 = no CPu received from the analogue modem yet; 1 = a CPu has been received. SUVd with this bit set is written **SUVd′** |
| 34 | Start bit 0 |
| 35:50 | CRC |
| 51 | Fill bit 0 |
| 52:… | Fill 0s up to the next multiple of **6 symbols** (a multiple of D bits) |

[INFERRED] In initial Phase 4, set bit 32 to 0.

### 4.13 CPd: Table 30/V.92 (8.8.3)

**What CPd carries.** CPd carries the upstream (analogue modem) data-mode parameters in four parts:

| Part | Bits (all parts present) | Presence flag |
|---|---|---|
| P0, fixed, always sent | 0 to 50 | — |
| P1, modulus encoder parameters | 51 to 152 (6 words) | CPd bit 19 |
| P2, precoder and prefilter coefficients | 153 to 220+α (4 + LZ1 + LP1 + LZ2 + LP2 words) | CPd bit 20 |
| P3, constellation sets | 221+α to 305+α+β (5 + LC1 + … + LC6 words) | CPd bit 21 |

- A **word** is 17 bits: one start bit (0) followed by 16 data bits.
- **All bits of an absent part are removed** [SHALL]. Table 30's bit numbers assume every part is present, so real positions shift.
- **Every CPd ends with**: start bit 0, a 16-bit CRC, at least one fill bit 0, then fill 0s up to a multiple of 6 symbols.
- **Definitions**:
  - α = 17 × (LZ1 + LP1 + LZ2 + LP2);
  - β = 17 × (LC1 + LC2 + LC3 + LC4 + LC5 + LC6).

**P0, bits 0 to 50 (always sent)**

| Bits | Meaning |
|---|---|
| 0:16 | Frame sync, 17 ones |
| 17 | Start bit 0 |
| 18 | CPd identifier = **0** |
| 19 | 1 = modulus-encoder parameters (P1) present |
| 20 | 1 = precoder and prefilter coefficients (P2) present |
| 21 | 1 = constellation sets (P3) present |
| 22:26 | drn, the selected upstream (analogue to digital) data signalling rate, 0 to 19. **drn = 0 shall indicate cleardown.** Rate = (drn+17) × 8000/6, so drn 1 to 19 gives 24 000 to 48 000 bit/s |
| 27:28 | Upstream trellis encoder: 0 = 16-state, 1 = 32-state, 2 = 64-state, 3 = reserved. The digital modem's receiver requires the analogue modem to use the selected encoder |
| 29 | Extend E2u: 0 = no; 1 = extend by 1 symbol. **Shall be 0 during rate renegotiation and FPX** |
| 30:32 | Reserved: the digital modem sets them to 0; the analogue modem ignores them |
| 33 | Acknowledge: 0 = no CPu received from the analogue modem; 1 = a CPu has been received. CPd with this bit set is written **CPd′** |
| 34 | Start bit 0 |
| 35:50 | 4 × G (> 0), four times the gain applied at the prefilter output. Unsigned Q0.16 (`.xxxxxxxxxxxxxxxx`) |

**P1, modulus encoder parameters (6 words, bits 51 to 152 when present)**

| Bits | Meaning |
|---|---|
| 51 | Start bit |
| 52:59 | M0 |
| 60:67 | M1 |
| 68 | Start bit |
| 69:76 | M2 |
| 77:84 | M3 |
| 85 | Start bit |
| 86:93 | M4 |
| 94:101 | M5 |
| 102 | Start bit |
| 103:110 | M6 |
| 111:118 | M7 |
| 119 | Start bit |
| 120:127 | M8 |
| 128:135 | M9 |
| 136 | Start bit |
| 137:144 | M10 |
| 145:152 | M11 |

Each M_i is an 8-bit unsigned integer, sent LSB first. It is the modulus for upstream data-frame interval i (V.92 6.4.1).

**P2, precoder and prefilter coefficients (bits 153 to 220+α when present)**

| Bits | Meaning |
|---|---|
| 153 | Start bit |
| 154:162 | **LZ1** (9 bits): taps in the precoder feed-forward section, at most L_max (INFO1a bits 16:17) |
| 163:169 | Reserved: the digital modem sets them to 0 |
| 170 | Start bit |
| 171:179 | **LP1** (9 bits): taps in the precoder feedback section, at most L_max |
| 180:186 | Reserved (0) |
| 187 | Start bit |
| 188:196 | **LZ2** (9 bits): taps in the prefilter feed-forward section, at most L_max |
| 197:203 | Reserved (0) |
| 204 | Start bit |
| 205:213 | **LP2** (9 bits): taps in the prefilter feedback section, at most L_max |
| 214:220 | Reserved (0) |
| 221 | Start bit |
| 222:237 | z1(1), the first precoder feed-forward coefficient, signed Q0.15 (16 bits). Present only if LZ1 > 0 |
| … | z1(2) … z1(LZ1), each a 17-bit word (start bit and 16 bits) |
| 221 + 17·LZ1 | Start bit |
| 222 + 17·LZ1 : 237 + 17·LZ1 | p1(1), the first precoder feedback coefficient, signed Q1.14 |
| … | p1(2) … p1(LP1) |
| 221 + 17·(LZ1+LP1) | Start bit |
| … | Prefilter feed-forward coefficients **z2(0) … z2(LZ2−1)**, signed Q0.15. Note that indexing starts at 0 |
| 221 + 17·(LZ1+LP1+LZ2) | Start bit |
| … | Prefilter feedback coefficients p2(1) … p2(LP2), signed Q1.14. Present only if LP2 > 0 |

- The coefficient word for coefficient number k (counting from 1 across all four lists) starts at bit 221 + 17·(k−1).
- [SHALL] LZ1 + LP1 + LZ2 + LP2 ≤ L_tot, from INFO1a bits 14:15 (0 = 192, 1 = 256, 2 = 320, 3 = 384).
- [SHALL] Each of LZ1, LP1, LZ2 and LP2 ≤ L_max, from INFO1a bits 16:17 (0 = 128, 1 = 192, 2 = 256, 3 = 320).
- [INFERRED] The digital modem must also respect INFO1a bits 12:13, which list the filter sections the analogue modem supports:
  - 0 = p1 and z2 only;
  - 1 = z1, p1 and z2;
  - 2 = p1, p2 and z2;
  - 3 = z1, p1, p2 and z2.
  - So LZ1 = 0 unless the value is 1 or 3, and LP2 = 0 unless the value is 2 or 3.
- The filters (V.92 6.4.2):
  - x(n) = u(n) + Σ_{κ=1..LZ1} u(n−κ)·z1(κ) + Σ_{κ=1..LP1} x(n−κ)·p1(κ)
  - v(n) = Σ_{κ=0..LZ2−1} x(n−κ)·z2(κ) + Σ_{κ=1..LP2} v(n−κ)·p2(κ)
  - The transmitted value is G·v(n).
- [INFERRED] LZ2 must be at least 1, or v(n) is identically zero.

**P3, constellation sets (bits 221+α to 305+α+β when present)**

| Bits | Meaning |
|---|---|
| 221+α | Start bit |
| 222+α : 225+α | Index (0 to 5) of the constellation for upstream intervals **0 and 6** |
| 226+α : 229+α | The same for intervals 1 and 7 |
| 230+α : 233+α | The same for intervals 2 and 8 |
| 234+α : 237+α | The same for intervals 3 and 9 |
| 238+α | Start bit |
| 239+α : 242+α | The same for intervals 4 and 10 |
| 243+α : 246+α | The same for intervals 5 and 11 |
| 247+α : 254+α | Reserved: the digital modem sets them to 0 |
| 255+α | Start bit |
| 256+α : 263+α | LC1, the number of **positive** points in constellation set 1 (8 bits) |
| 264+α : 271+α | LC2, possibly zero |
| 272+α | Start bit |
| 273+α : 280+α | LC3, possibly zero |
| 281+α : 288+α | LC4, possibly zero |
| 289+α | Start bit |
| 290+α : 297+α | LC5, possibly zero |
| 298+α : 305+α | LC6, possibly zero |
| 306+α | Start bit |
| 307+α : 322+α | Linear value of the 1st (smallest-magnitude) point of set 1 (16 bits) |
| 323+α | Start bit |
| … | The rest of set 1, ascending in magnitude, up to the largest point |
| 306+α + 17·LC1 | Start bit |
| … | Further sets in the same format, one for each set of non-zero size |

Rules for P3 (8.8.3 and 6.4.2):

- Constellations **shall not contain the zero point** [SHALL].
- **All sets of non-zero size shall be listed first** [SHALL].
- **A set shall not have more than 128 points** [SHALL]. [INFERRED] "Points" means N = 2·LC, the symmetric constellation, so LC ≤ 64 (see Q6).
- Only the LC positive points are sent. The full constellation is taken as symmetric:
  - there are N = 2·LC_k points a(η), with −N/2 ≤ η < N/2, in the same order as the levels;
  - negative indices are negative points (6.4.2).
- [AMBIGUOUS, Q5] The number format and scale of the 16-bit "linear value" are not given.
- [INFERRED] Set k (k = 1 to 6) has index k−1 in the interval-index fields.

**End of CPd**

| Bits (all parts present) | Meaning |
|---|---|
| 306+α+β | Start bit 0 |
| 307+α+β : 322+α+β | CRC |
| 323+α+β | Fill bit 0 |
| 324+α+β : … | Fill 0s up to the next multiple of 6 symbols |

**Other CPd requirements**

- Design rule [SHALL]: the digital modem designs the parameters on the assumption that the analogue modem transmits at the desired power when the prefilter output times G has a mean square of 1.
- Note [SHOULD]: the digital modem should design the precoder coefficients on the assumption that the analogue modem minimises precoder output power symbol by symbol.
- Modulation: the TRN2d modulation (4.10) in training and rate renegotiation, with the differential encoder continuing from the last sign bit of the preceding sequence. The preceding data-mode modulation in FPX.

**Building or parsing CPd without absolute offsets.** Walk the parts in order:

1. P0: 51 bits.
2. If bit 19 = 1: P1, 102 bits.
3. If bit 20 = 1: P2, 68 + α bits. α comes from LZ1, LP1, LZ2 and LP2 read inside P2.
4. If bit 21 = 1: P3, 85 + β bits. β comes from LC1 to LC6 read inside P3.
5. Start bit, 16-bit CRC, fill bit, then pad to a multiple of D.

This keeps start bits on multiples of 17 whatever parts are present, because each part is a whole number of 17-bit words.

[AMBIGUOUS, Q4] In initial training all three parts are presumably required, since the analogue modem has no earlier values to fall back on. The text does not say so.

### 4.14 Ed (8.8.2)

- **2 downstream data frames** (12 symbols, 2D bits) of scrambled binary **zeros**.
- It uses the TRN2d modulation in training and rate renegotiation, and the preceding data-mode modulation in FPX.
- Its job is to mark the end of SUVd and CPd.

### 4.15 B1d (V.92 8.8.1, pointing to V.90 8.6.1)

- **48 downstream data frames** (288 symbols) of scrambled ones.
- It uses the **selected data-mode constellation parameters from CPu** and the negotiated rate.
- Before B1d, the scrambler, differential encoder and spectral-shape filter memory are reset to zero.
- The symbols in B1d's first data frame shall have the magnitudes produced by mapping the first D scrambled ones after the scrambler reset, and shall be the same for every ld [SHALL].
- K and S are from Table 2/V.90 (K from 15 to 39; 28 to 56 kbit/s).

### 4.16 Tone A and Tone B (for 9.7)

- V.90 8.2.1 and 8.2.2 point to V.34 10.1.2.1 and 10.1.2.2. In V.90 and V.92, **the analogue modem always sends Tone A and the digital modem always sends Tone B**, whichever side placed the call.
- **Tone A**: 2400 Hz.
  - Transitions between A and Ā (and back) are 180° phase reversals.
  - While A or Ā is on the line, an **1800 Hz guard tone** is sent with no phase reversals.
  - V.34 10.1.2.1, as rendered, says Tone A is 1 dB below nominal transmit power and the guard tone is *at* nominal power.
  - For INFO, by contrast, V.34 10.1.2.3.1 and V.90 8.2.3.1 put the guard tone 7 dB below nominal. See section 10.3, Q13.
- **Tone B**: 1200 Hz. Transitions between B and B̄ are 180° phase reversals.
- **Level**: V.90 8.2 says every Phase 2 signal except L1 goes at the nominal transmit power. If recovery returns a modem to Phase 2 from a later phase, the transmit level shall go back to the nominal transmit power.
- V.34 NOTE [SHOULD]: the transmit filter should not narrow the tone so much that the phase reversal timing becomes inaccurate, since round-trip delay is measured from it.

### 4.17 Fields from earlier sequences that Phase 4 depends on

| Field | Where it is defined | How Phase 4 uses it |
|---|---|---|
| Jp bit 48 | Table 22 | TRN2u, SUVu, CPu and E2u constellation size during training (0 = 4-point, 1 = 8-point) |
| Jp bit 49 | Table 22 | The same, during rate renegotiation |
| Jd bits 18:33 and 35:40 | Table 21 | Downstream rates the digital modem supports. Bit 18 is 28 000 and each bit adds 8000/6 up to bit 33 (48 000); bit 35 is 49 333 up to bit 40 (56 000). Constrains the drn in CPu |
| Jd bits 49:50 | Table 21 | The digital modem's maximum look-ahead (1 to 3). Constrains ld in CPu |
| DIL descriptor rate mask (in Ja) | Table 20 | Upstream rates the analogue modem supports. The first mask bit is 24 000, each bit adds 8000/6, bit 203+β+⌈N/2⌉·17 is 44 000, and the following bits are 45 333, 46 666 and 48 000. Constrains the drn in CPd |
| INFO1a bits 12:13 | Table 18 | Filter sections the analogue modem supports |
| INFO1a bits 14:15 | Table 18 | L_tot |
| INFO1a bits 16:17 | Table 18 | L_max |
| INFO1a bits 25:31 | Table 18 | U_INFO, used by Ri |
| CPt | Table 23 | Modulation parameters for TRN2d, SUVd, CPd and Ed |

[INFERRED] Each modem should choose its drn only from rates the other enabled. The text says only that the masks mean "supported and enabled".

---

## 5. Phase 4 sequence diagrams (Figures 12, 13 and 14, transcribed)

Arrows mark which received sequence triggers which change.

**Figure 12/V.92: both CP sequences cross at about the same time**

```
Digital : TRN2d(>=2040T) SUVd SUVd  CPd      SUVd' SUVd' Ed  B1d  DATA
Analogue: TRN2u(>=12000T) SUVu SUVu CPu  SUVu SUVu' SUVu' E2u B1u DATA
```

- The first SUVd and SUVu cross.
- CPu arrives at the digital modem while it is sending CPd, so the digital modem's following SUVd carry the ack (SUVd′).
- CPd arrives at the analogue modem after its CPu, so the analogue modem sends one plain SUVu and then SUVu′.
- The last SUVd′ and SUVu′ cross, and then Ed and E2u are sent.
- Circuit timing:
  - digital modem: 106 enabled at the start of DATA; 109 ON and 104 unclamped at the end of received B1u;
  - analogue modem: the same, the other way round.

**Figure 13/V.92: CPu sent earlier than CPd**

```
Digital : TRN2d SUVd SUVd SUVd SUVd SUVd' CPd'  SUVd' SUVd' Ed B1d DATA
Analogue: TRN2u SUVu SUVu CPu  SUVu SUVu SUVu SUVu SUVu' E2u B1u DATA
```

- The digital modem receives CPu before it sends its single CPd, so that CPd is already CPd′ and the SUVd before it is already SUVd′.
- The analogue modem receives CPd′ and switches to SUVu′.
- Ed and E2u follow.

**Figure 14/V.92: the first CPu is lost at the digital modem**

```
Digital : TRN2d SUVd SUVd SUVd SUVd SUVd CPd SUVd SUVd SUVd SUVd' Ed B1d DATA
Analogue: TRN2u SUVu SUVu CPu SUVu SUVu SUVu SUVu SUVu' CPu' CPu' CPu' E2u B1u DATA
                             |<--- RTD + 100 ms --->|
```

- The analogue modem's CPu is lost, so the digital modem's CPd goes out without the ack.
- The analogue modem receives the CPd and sends SUVu′.
- The analogue modem gets **no ack** in any SUVd received up to and including the one that completes after (end of its CPu + 100 ms + RTD). It therefore repeats CPu, as CPu′ because a CPd has now been received.
- The digital modem receives CPu′ and sends SUVd′.
- The digital modem has now sent SUVd′ and received CPu′ (acked), so it sends Ed.
- The analogue modem has sent CPu′ and received SUVd′ or Ed, so it finishes the current CPu′ and sends E2u.
- The "RTD + 100 ms" bracket is drawn under the analogue modem's line. It starts at the end of that modem's CPu and ends part-way through its fourth SUVu after CPu. The first SUVd whose reception completes after that point carries no ack, and CPu′ repeats follow.
- The figure shows one more plain SUVu and then an SUVu′ after the CPd arrives, rather than switching at the next boundary. [INFERRED] Treat this as drawing slack or processing delay, not as a rule. 9.6.2.1.2 only says "subsequent" sequences carry the ack.
- Circuit marks (Figures 12 to 14 alike): on each modem, "106 enable" is at the start of its DATA, just after its own B1. "109" and "unclamp 104" are drawn close to that point, and the figures place them only roughly. Their timing comes from the text: they switch when the B1 the modem receives ends (9.6.1.1.6, 9.6.2.1.6).

---

## 6. Clause 9.6: Phase 4 operating procedures

### 6.1 Digital modem, error-free procedures (9.6.1.1)

**D4-1 (9.6.1.1.1)** [SHALL]

- Transmit TRN2d for **at least 2040T**.
- When ready to receive a CPu:
  - condition the receiver to receive **SUVu**;
  - transmit **SUVd** sequences, repeatedly.
- [INFERRED] "Ready" includes having trained on enough TRN2u to design CPd. The analogue modem may cut TRN2u short as soon as SUVd arrives (see A4-1).

**D4-2 (9.6.1.1.2)** [SHALL]

- After receiving an SUVu, transmit **one** CPd, then more SUVd.
- After receiving a CPu (or, in rate renegotiation or FPX, a CPus), set the acknowledge bit in every later CPd and SUVd (CPd′, SUVd′).
- Options and notes:
  - [MAY] If the received SUVu has bit 26 = 1, the digital modem may wait for a CPu before sending CPd. It is not required to.
  - The text gives no deadline between receiving SUVu and sending CPd.

**D4-3 (9.6.1.1.3), repeat rule** [SHALL]

- Let t_end be the end of the digital modem's (single) CPd.
- Look at every CPu and SUVu received **up to and including the whole CPu or SUVu that is received after t_end + 100 ms + RTD**.
- If none of them has the acknowledge bit set, send **repeated** CPd sequences.
  - These are CPd′ if a CPu has been received by then.
  - Keep sending them until D4-4 is met.

**D4-4 (9.6.1.1.4), termination** [SHALL]

- Condition: the digital modem **has sent** a CPd or SUVd with the ack bit set, **and has received** either a CPu or SUVu with the ack bit set, or an E2u.
- Then: finish the current CPd or SUVd and transmit **Ed**.

**D4-5 (9.6.1.1.5)** [SHALL]

- After Ed, transmit **B1d** at the negotiated rate with the data-mode constellation parameters received in CPu.
- Then **enable circuit 106 to follow circuit 105**.
- Begin data transmission using the modulation of clause 5 (V.92 clause 5, which is V.90 clause 5).

**D4-6 (9.6.1.1.6)** [SHALL]

- After receiving **E2u**, condition the receiver for **B1u**. In FPX, condition it for **FB1u followed by B1u** instead.
  - [INFERRED] Allow for the optional 1-symbol extension of E2u (CPd bit 29), which only the digital modem can have asked for.
- After receiving B1u:
  - **unclamp circuit 104**;
  - **turn circuit 109 ON**;
  - begin demodulating data.

### 6.2 Digital modem, recovery (9.6.1.2)

- **D4-R0**:
  - [MAY] The digital modem may start a retrain at any time during Phase 4 (9.7.1.1).
  - [SHALL] If it detects **Tone A** during Phase 4, it shall respond to the retrain (9.7.1.2).
- **D4-R1 (9.6.1.2.1)** [SHALL]: if B1u has not been received within **20 s + 6 × RTD from the end of INFO1a**, start a retrain (9.7.1.1).
  - "The end of INFO1a" means the end of the INFO1a *received* in Phase 2.
  - V.90 used 15 s + 5 RTD, measured from receiving INFO1a.

### 6.3 Analogue modem, error-free procedures (9.6.2.1)

**A4-1 (9.6.2.1.1)** [SHALL]

- Condition the receiver to receive **SUVd**, and transmit **TRN2u**.
- Start sending SUVu sequences, repeatedly, once both of these hold:
  - the analogue modem is ready to receive a CPd;
  - **either** it has sent at least **12000T** of TRN2u, **or** it has received an SUVd.

**A4-2 (9.6.2.1.2)** [SHALL]

- After receiving an SUVd, transmit **one** CPu, then more SUVu.
- After receiving a CPd, set the acknowledge bit in every later CPu and SUVu (CPu′, SUVu′).

**A4-3 (9.6.2.1.3), repeat rule** [SHALL]

- This mirrors D4-3, measured from the end of the analogue modem's own CPu.
- If no CPd or SUVd received up to and including the whole one that is received after t_end + 100 ms + RTD has the ack bit set, send repeated CPu sequences.

**A4-4 (9.6.2.1.4), termination** [SHALL]

- Condition: the analogue modem has sent a CPu or SUVu with the ack bit set, **and** has received a CPd or SUVd with the ack bit set, or an Ed.
- Then: finish the current CPu and transmit **E2u**.
- [AMBIGUOUS, Q8] The text says only "the current CPu". The digital-side text says "CPd or SUVd", and Figures 12 and 13 show the analogue modem going from SUVu′ straight to E2u. Treat it as "the current CPu or SUVu".

**A4-5 (9.6.2.1.5)** [SHALL]

- After E2u, send **B1u**. In FPX, send **FB1u then B1u**.
- Then **enable circuit 106 to follow circuit 105**.
- Begin data transmission using 6.4.

**A4-6 (9.6.2.1.6)** [SHALL]

- After receiving **Ed**, condition the receiver for **B1d**.
- After receiving B1d:
  - **unclamp 104**;
  - **turn 109 ON**;
  - begin demodulating data.

### 6.4 Analogue modem, recovery (9.6.2.2)

- **A4-R0**:
  - [MAY] The analogue modem may start a retrain at any time during Phase 4 (9.7.2.1).
  - [SHALL] If it detects **Tone B** during Phase 4, it shall respond to the retrain (9.7.2.2).
- **A4-R1 (9.6.2.2.1)** [SHALL]: if B1d has not been received within **20 s + 6 × RTD from the end of sending INFO1a**, start a retrain (9.7.2.1).

### 6.5 The exchange as a state machine [INFERRED]

The exchange is the same on both sides. "Own" means this modem; "peer" means the far modem.

```
state:
  sent_ack       := false   // we have transmitted a CP/SUV with bit33 = 1
  got_peer_cp    := false   // we have received a CP (long or short) from the peer
  peer_acked     := false   // we have received a CP/SUV with bit33 = 1, or E
  my_cp_sent_end := None
  repeat_cp      := false

transmit loop (sequence by sequence, each padded to its symbol multiple):
  if not started_suv: send TRN2 (see D4-1 / A4-1 conditions)
  elif need_single_cp and received_peer_suv and not my_cp_sent_end:
        send CP(ack = got_peer_cp); my_cp_sent_end = now
  elif repeat_cp: send CP(ack = got_peer_cp)
  else: send SUV(ack = got_peer_cp)
  if the sequence just sent had ack = 1: sent_ack = true
  if sent_ack and peer_acked: send E (E2u, extended if CPd.bit29 on the analogue side,
                                      or Ed) and leave

receive:
  on CP (CRC good):  got_peer_cp = true; store parameters; if bit33: peer_acked = true
  on SUV (CRC good): if bit33: peer_acked = true
  on E:              peer_acked = true
  repeat check:      after my_cp_sent_end, at the first complete CP/SUV received whose
                     reception ends after my_cp_sent_end + 100 ms + RTD:
                     if no ack seen so far -> repeat_cp = true
```

- The ack bit in CP and SUV always reflects `got_peer_cp` at the moment the sequence starts.
- Sequences in a group have to be identical apart from that bit. So if a CP arrives in the middle of a sequence, the change of ack takes effect at the next sequence boundary.
- Sequences with a bad CRC should be ignored. [INFERRED] The text has no rule for CRC failures other than the repeat mechanism.

---

## 7. Clause 9.7: retrains

### 7.1 Digital modem

**RD-I, initiating a retrain (9.7.1.1)** [SHALL], in order:

1. Turn circuit **106 OFF**.
2. **Clamp circuit 104** to binary one.
3. Transmit **silence for 70 ± 5 ms**.
   - [INFERRED] Following 9.8.1.1.3, silence from the digital modem means PCM codewords with Ucode 0.
4. Transmit **Tone B** and condition the receiver to **detect Tone A**.
5. After detecting Tone A, condition the receiver to detect a **Tone A phase reversal**.
6. Continue with the **full Phase 2** start-up procedure (see 7.3).

**RD-R, responding to a retrain (9.7.1.2)** [SHALL]

1. Trigger: **Tone A detected for more than 50 ms**.
2. Turn **106 OFF**, **clamp 104** to one, and send **silence for 70 ± 5 ms**.
3. Transmit **Tone B** and condition the receiver to detect a **Tone A phase reversal**.
4. Continue with the full Phase 2 procedure.

### 7.2 Analogue modem

**RA-I, initiating a retrain (9.7.2.1)** [SHALL]

1. Turn **106 OFF**, **clamp 104** to one, and send **silence for 70 ± 5 ms**.
2. Transmit **Tone A** and condition the receiver to **detect Tone B**.
3. Once **Tone B has been detected and Tone A has been sent for at least 50 ms**:
   - transmit a **Tone A phase reversal**;
   - condition the receiver to detect a **Tone B phase reversal**.
4. Continue with the full Phase 2 procedure.

**RA-R, responding to a retrain (9.7.2.2)** [SHALL]

1. Trigger: **Tone B detected for more than 50 ms**.
2. Turn **106 OFF**, **clamp 104** to one, and send **silence for 70 ± 5 ms**.
3. Transmit **Tone A**.
4. Continue with the full Phase 2 procedure.

### 7.3 Where "full Phase 2" continues

**Which Phase 2 applies**

- V.92 9.3: full Phase 2 procedures and recovery are **identical to V.90 Phase 2** (V.90 9.2). The difference is that when both modems announced V.92 (INFO0d bit 27, INFO0a bit 26), the INFO information bits are the V.92 ones (V.92 8.4.1, and INFO1a Table 18 for PCM upstream).
- [SHALL] "If both … indicate V.92 capability, **any subsequent retrains shall use Phase 2 of ITU-T V.92**."
- A retrain always uses **full** Phase 2. 9.7 never mentions short Phase 2 (9.4).

**Where in V.90 9.2 to rejoin.** V.92 9.7 just says "the full Phase 2 start-up procedure". The retrain text in V.90 9.5, which is otherwise word-for-word the same, names the exact entry points:

| Retrain case | V.90 entry point | What that step and the following ones require (V.90 9.2) |
|---|---|---|
| RD-I and RD-R (digital) | **9.2.1.1.3** | See the digital column below |
| RA-I (analogue initiator; it has already sent its Tone A reversal) | **9.2.2.1.4** | See the analogue column below |
| RA-R (analogue responder) | **9.2.2.1.3** | See the analogue column below |

What the digital modem does from V.90 9.2.1.1.3:

1. On detecting the Tone A reversal, transmit a Tone B reversal timed so that it **appears at the line terminals 40 ± 1 ms after the Tone A reversal was received**.
2. Send Tone B for 10 ms more, then silence, and wait for a second Tone A reversal.
3. 9.2.1.1.4: RTDEd = (the moment its B reversal appeared at the line) to (the moment the second A reversal was received), minus 40 ms.
4. Receive L1 (160 ms) and, optionally, L2 (at most 500 ms).
5. Send Tone B; after Tone A and its reversal, send a B reversal again 40 ± 1 ms after, then 10 ms more of B, then L1 and L2.
6. After detecting Tone A and receiving the echo of L2 for at most 550 ms + RTD, send INFO1d.
7. Wait for INFO1a.

What the analogue modem does from V.90 9.2.2.1.3:

1. Once Tone B is detected and Tone A has run for at least 50 ms, send the Tone A reversal and wait for the Tone B reversal.
2. 9.2.2.1.4: RTDEa = (Tone A reversal sent) to (Tone B reversal received), minus 40 ms.
3. 9.2.2.1.5: send another A reversal appearing 40 ± 1 ms after the B reversal was received, then 10 ms more of A, then L1 and L2, and wait for Tone B.
4. 9.2.2.1.6: send A for 50 ms, then a reversal and 10 ms more, then silence.
5. Receive L1 and L2, send Tone A, receive INFO1d, send INFO1a.

V.90 9.2 recovery timers that then apply:

- 9.2.1.2.2: the digital modem keeps sending Tone B until it sees the A reversal.
- 9.2.1.2.3: no second A reversal within 2000 ms. The digital modem goes silent, waits for Tone A, and restarts at 9.2.1.1.3.
- 9.2.1.2.4: 900 ms + RTD.
- 9.2.1.2.5: no Tone A within 650 ms + RTD of the start of L2. **Retrain.**
- 9.2.1.2.6: no INFO1a within 700 ms + RTD. The digital modem looks for Tone A or INFOMARKSa.
- 9.2.2.2.2: no B reversal within 2000 ms. The analogue modem waits for Tone B and repeats 9.2.2.1.3.
- 9.2.2.2.3: 600 ms + RTD.
- 9.2.2.2.4: no INFO1d within 2000 ms + 2 RTD. The analogue modem retrains or sends INFOMARKSa.

**Other points on the retrain path**

- After a retrain, Phase 2 **does not repeat the INFO0 exchange**; it starts at the tones. [INFERRED] Keep the INFO0 capabilities from the first Phase 2.
- Transmit level: V.90 8.2 says that when recovery goes back to Phase 2 from a later phase, the level goes back to nominal transmit power. (V.34 10.1.2 has a different rule, which V.90 and V.92 do not use.)
- After Phase 2 ends, the modems go through Phase 3 (9.5) and Phase 4 (9.6) again, with the new INFO1a as the time reference for the 20 s + 6 RTD timer.

### 7.4 Collision and convergence [INFERRED]

The four procedures are built to converge.

- **Both modems start a retrain at once.** The digital modem sends B and waits for A; the analogue modem sends A and waits for B. Each sees the other's tone, so the analogue modem sends its reversal (RA-I step 3) and the digital modem is already waiting for it (RD-I step 5).
- **One modem starts and the other responds.** A responder only sends its tone and continues. The analogue responder's first reversal (V.90 9.2.2.1.3) is what the digital modem, as initiator, waits for.
- **Echo.** A modem's own tone echo is 1200 Hz away from the tone it is detecting (2400 against 1200), so echo does not confuse detection. Detectors still have to reject the 1800 Hz guard tone.

### 7.5 What triggers a retrain in V.92

Among the pages reviewed, these V.92 clauses start a retrain:

| Clause | Modem | Trigger |
|---|---|---|
| 9.4.2.2.2 | Analogue | No Tone B reversal within 2500 ms of the end of INFO0a (short Phase 2) |
| 9.5.1.2 | Digital | [MAY] at any time |
| 9.5.1.2.1 | Digital | Ja not received within 4500 ms + RTD of the end of INFO1a |
| 9.5.1.2.2 | Digital | Su not detected within 5100 ms + RTD of the start of TRN1d |
| 9.5.2.2 | Analogue | [MAY] at any time |
| 9.5.2.2.1 | Analogue | Sd-to-S̄d not detected within 1500 ms of the start of Ja |
| 9.5.2.2.2 | Analogue | Jd not received within 4500 ms of the end of Ja |
| 9.6.1.2, 9.6.1.2.1 | Digital | As in 6.2 |
| 9.6.2.2, 9.6.2.2.1 | Analogue | As in 6.4 |
| 9.10.1.1 | Either | MH initiating sequence gets no answer within 2 s + RTD; the modem retrains or disconnects |

In every phase, detecting the peer's retrain tone (Tone A at the digital modem, Tone B at the analogue modem) means the modem **shall** respond (9.5.x.2, 9.6.x.2).

### 7.6 Interaction with modem-on-hold (9.10.1.1, 8.9.1)

- Tone **RT** is the tone a modem sends in retrains: Tone B for the digital modem, Tone A for the analogue modem. A modem detects the other tone.
- A modem-on-hold transaction may start **exactly like a retrain**:
  - The initiator sends RT (≥ 50 ms, or ≥ 20 ms if an MH sequence came before), followed by MH sequences. MH sequences use INFO DPSK modulation and the V.34 CRC.
  - A responder that has started RD-R or RA-R **may** answer with a phase reversal (a retrain). The initiator will normally ignore that and carry on with modem-on-hold.
  - [SHALL] So a responding modem has to be listening for **both** the peer's phase reversal **and** an initiating MH sequence.
- For example, when the digital modem starts modem-on-hold with Tone B, the analogue modem (in RA-R) must keep an INFO-style DPSK receiver at 1200 Hz active alongside the Tone B reversal detector.

---

## 8. Timers and tolerances

| Item | Value | Clause | Kind |
|---|---|---|---|
| TRN2d minimum | ≥ 2040T (255 ms) | 9.6.1.1.1 | SHALL |
| TRN2d maximum (initial Phase 4) | None stated | — | — |
| TRN2u minimum before SUVu, unless SUVd was received | ≥ 12000T (1.5 s) | 9.6.2.1.1 | SHALL |
| Window before repeating CPd or CPu | 100 ms + RTD from the end of own CP, including the whole sequence received after that point | 9.6.1.1.3, 9.6.2.1.3 | SHALL |
| B1u receive deadline (digital) | 20 s + 6·RTD from the end of INFO1a, else retrain | 9.6.1.2.1 | SHALL |
| B1d receive deadline (analogue) | 20 s + 6·RTD from the end of sending INFO1a, else retrain | 9.6.2.2.1 | SHALL |
| Ed length | 2 downstream data frames (12 symbols) | 8.8.2 | — |
| E2u length | 1 upstream data frame (12 symbols), +1 symbol if CPd bit 29 = 1 | 8.7.2 | SHALL |
| B1d, B1u, FB1u | 48 data frames each (288, 576 and 576 symbols) | 8.8.1, 8.7.1, 8.7.7 | — |
| SUVd and CPd length | Multiple of 6 symbols | Tables 30, 31 | SHALL |
| SUVu, CPu, CPus and TRN2u length | Multiple of 12 symbols | Tables 23, 24, 27; 8.7.6 | SHALL |
| TRN2d length | Multiple of 6 symbols | V.90 8.6.5 | SHALL |
| Retrain silence | 70 ± 5 ms | 9.7.x | SHALL |
| Retrain-response tone detection | > 50 ms of Tone A (digital) or Tone B (analogue) | 9.7.1.2, 9.7.2.2 | SHALL |
| Tone A before the analogue initiator's reversal | ≥ 50 ms, and Tone B detected | 9.7.2.1 | SHALL |
| Phase 2 reversal timing after a retrain | 40 ± 1 ms at the line terminals; 10 ms of tone after a reversal | V.90 9.2 | SHALL |

---

## 9. Interchange circuits

| Event | Digital modem | Analogue modem |
|---|---|---|
| Throughout start-up and Phase 4 | 104 clamped to 1, 106 OFF, 109 OFF [INFERRED from the figures and the retrain text] | Same |
| After sending Ed and then B1d, or E2u and then B1u (or FB1u + B1u) | **Enable 106 to follow 105**; begin data transmission (clause 5) | **Enable 106 to follow 105**; begin data transmission (6.4) |
| After receiving B1u or B1d | **Unclamp 104, 109 ON**, demodulate | **Unclamp 104, 109 ON**, demodulate |
| Retrain start (initiating or responding) | **106 OFF, 104 clamped to 1** | **106 OFF, 104 clamped to 1** |

Circuit 107 is asserted in Phase 3 (9.5.1.1.9, 9.5.2.1.8). Neither 9.6 nor 9.7 changes it.

V.92 Table 1, note 1: circuit 109 has no threshold or response-time requirements, because a signal detector cannot tell received signals from talker echo.

---

## 10. Implementation notes

### 10.1 What is new compared with V.90 Phase 4 (V.90 9.4)

| Aspect | V.90 | V.92 (PCM upstream) |
|---|---|---|
| CPt and Ri/R̄i | Part of Phase 4 | Moved into Phase 3 (9.5). Phase 4 starts straight after R̄i or E1u |
| Upstream training in Phase 4 | Optional SCR for at most 4 s | **TRN2u**, a 4- or 8-point PCM-level signal, for at least 12000T unless SUVd arrives. The digital modem uses it to estimate the upstream channel |
| Handshake before parameters | None: MP within 2000 ms of TRN2d; CP straight away | **SUV** short sequences are exchanged first. Each side sends **one** CP only after hearing the other's SUV, and repeats it only if no ack arrives within 100 ms + RTD |
| Parameters for the analogue modem | MP (rate, trellis, Θ, shaping, optional 3-tap precoder) | **CPd**: rate, trellis, G, 12 moduli, full precoder and prefilter (up to 384 taps), up to 6 PCM-level constellations, and the E2u extension flag |
| Parameters for the digital modem | CP: bit 19 type, drn at 20:24, silence bit 30, rate mask 36:48 | **CPu**: bit 18 = 0, type at **19:20**, drn at **21:25**, no silence bit (silence moved to SUV bit 32), no rate mask (moved to the Ja DIL descriptor). CPus is the short form |
| End marker from the analogue modem | 20-bit E (V.34 10.1.3.2) | **E2u**: one 12-symbol frame, optionally 13 |
| Final upstream frames | B1 (V.34) | **B1u**, 48 × 12 symbols of PCM-upstream data mode, starting at interval 0. In FPX, **FB1u** comes first |
| Timeout | 15 s + 5 RTD after INFO1a | **20 s + 6 RTD** after INFO1a |
| Fill bits | CP: fixed `000`. MP: pad to 6 symbols | Pad to 12 symbols (upstream) or 6 symbols (downstream) |
| Retrain text | Names exact V.90 9.2 entry steps | Says "full Phase 2 start-up procedure", and requires V.92 Phase 2 once both modems are V.92 |

### 10.2 Pitfalls

1. **The extracted text is wrong for these tables.**
   - Table 23 in the .txt puts "Type" at 21:25 and drn at 26:30.
   - The TRN2u tables (28 and 29) are shifted by one row.
   - Use only the layouts in this file.
2. **CPu and CPt are not V.90 CP.**
   - The bit positions differ from bit 18 onwards (type moved from 19 to 19:20, drn from 20:24 to 21:25, the silence bit dropped).
   - `crates/datapump/src/v90/sequences.rs::Cp` will need a V.92 variant, not reuse as it stands.
   - The constellation part (136:271+δ, γ and δ) and the CRC placement are the same as V.90.
3. **Start bits sit at multiples of 17 everywhere.**
   - Use that as a self-check when building or parsing.
   - The CRC skips positions 0 to 16 and every multiple of 17, and stops before the fill.
   - In CPd with parts absent, positions shift by whole 17-bit words, so the rule still holds.
4. **Fill to "multiple of N symbols" means different bit counts in each direction.**
   - Downstream: a multiple of D = drn_t + 8 bits, taking drn from **CPt**, not CPu.
   - Upstream: a multiple of 24 or 36 bits.
   - A receiver has to work out each sequence's length from its contents (the P1/P2/P3 flags, LZ/LP/LC, and bit 128 with the maximum index) to know where the next sequence starts.
5. **The scrambler keeps running from TRN2 through SUV, CP and E.** Only TRN2u/TRN2d and B1u/B1d reset it.
   - The TRN2u *differential* encoder starts from **E1u's last sign bit, not zero**.
   - TRN2d's differential encoder starts from **zero** (V.90 8.6.5).
6. **Receivers must tell E apart from fill.** Both are zeros.
   - Upstream fill after a CRC is at most 35 zero bits (8-point) and is followed by 17 ones.
   - E2u is at least 24 zero bits.
   - [INFERRED] Decide at the point where the next frame sync should start: a sync that never comes, and zeros that continue past the fill boundary, mean E.
   - Ed is 2D zeros (18 to 60 bits).
7. **The repeat rule depends on RTD.**
   - On VoIP paths the round trip can be about 1.5 s each way (project memory "VoIP line round trip").
   - The RTD measured in Phase 2 is what should be used. Do not replace it with a fixed guess, or CP repeats will start too early. Early repeats are harmless but lengthen training.
   - The 20 s + 6 RTD timeout can reach about 29 s on such lines.
8. **Timers depend on "the end of INFO1a".** Keep one timestamp per training attempt and reset it on each retrain (the new INFO1a).
9. **The analogue modem must count 12000T of TRN2u OR stop at SUVd.** The digital modem should not send SUVd before it has what it needs to compute CPd (see D4-1).
10. **E2u extension.** Only the analogue modem transmits it, and only when CPd bit 29 = 1. The digital receiver that asked for it has to expect the extra symbol before B1u interval 0.
11. **FPX changes the modulation** of SUV, CP and E to the current data mode, and adds FB1u. The shared state machine needs that context flag (section 2.3).
12. **A retrain looks like the start of modem-on-hold** (7.6). Keep the MH detector active while responding to a retrain.
13. **Reuse from the existing code.** These were only looked at, not changed:
    - the V.34 CRC: `crates/datapump/src/v34/info.rs::crc`;
    - the V.34 scramblers: `crates/datapump/src/v34/signals.rs`;
    - Tone A, Tone B and INFO DPSK: `crates/datapump/src/v34/dpsk.rs`;
    - the V.90 5.4 encoder for TRN2d, SUVd, CPd, Ed and B1d: `crates/datapump/src/v90/encoder.rs`;
    - V.90 Phase 2 for the post-retrain path: `crates/datapump/src/v90/startup.rs`, `digital.rs`, `analogue.rs`.
    - These are V.90 implementations, so check each against the differences above before reusing it.
14. **This whole clause applies only to PCM-upstream operation.**
    - [INFERRED] V.92 9.4.1.1.5 and 9.4.2.1.4 say to go on with "the appropriate Phase 3 as signalled in INFO1a".
    - If INFO1a selects V.90 mode (Table 18/19 and V.90 INFO1a bits 37:39), Phases 3 and 4 follow V.90 9.3 and 9.4.
    - If it selects V.34 upstream (Table 19), the V.34 procedures apply.

### 10.3 Ambiguities, editorial issues and open questions

- **Q1. RTD for the analogue modem after a short Phase 2.**
  - 9.6.2.1.3 uses "a round-trip delay", but short Phase 2 (9.4.2) never measures RTDEa. The analogue modem only sends a reversal 40 ± 1 ms after receiving one.
  - Options: estimate RTD some other way, or use a conservative constant.
  - Needs a decision, or capture evidence from a V.92 server.
- **Q2. Bit order inside TRN2u, SUVu, CPu and E2u symbols** (Tables 28 and 29, labelled MSB:LSB).
  - The text does not say whether the first bit in time is the sign (MSB) or the magnitude.
  - Clause 8's rule that integers go LSB first covers Tables 2–5, 11–24, 27 and 30–33, **not** 28 and 29.
  - Resolve from a real V.92 capture before settling it. Record the choice as a named constant.
- **Q3. Do TRN2u and the SUV/CP/E sequences go through the precoder and prefilter or G?**
  - The text says only "4- or 8-point constellation signal", with values given in L_U units. For Ru it says explicitly that they are bypassed (8.5.5).
  - Leaning: bypass them, as for TRN1u and Ru, because the precoder is not designed yet in initial training. In rate renegotiation this is less clear.
- **Q4. Which CPd parts are mandatory in initial training?**
  - Presumably all three, since the analogue modem has nothing to fall back on. Not stated.
  - When a part is absent in rate renegotiation or FPX, presumably the previous values stay in force. Not stated.
- **Q5. Format and scale of the 16-bit constellation "linear value" in CPd** (Table 30).
  - No Q-format is given. It is also unclear whether the scale is before or after G.
  - Needs a capture or a decision tied to the "G·v has mean square 1 means desired power" rule.
- **Q6. "Shall not exceed 128 points".**
  - It is not clear whether this means N = 2·LC ≤ 128 (so LC ≤ 64) or LC ≤ 128.
  - 6.4.2 calls 2·LC the number of points N. This digest reads it as N ≤ 128.
- **Q7. Table 23's last two rows both start at 289+δ.**
  - One row is "Fill bit 0" and the other is "Fill bits 0s to extend…".
  - Read this as: one fill bit at 289+δ, then further fill 0s from 290+δ. That matches Tables 24, 27, 30 and 31, whose extra fill starts one bit after the single fill bit.
- **Q8. 9.6.2.1.4 says "complete sending the current CPu".**
  - Figures 12 and 13 show SUVu′ as the last sequence before E2u. Treat it as "the current CPu or SUVu".
- **Q9. The unsigned Qa.b range in 3.5 is written as [0, 2^(a+1)).**
  - That does not match an (a+b)-bit field, which would give [0, 2^a).
  - Use the field widths: Q3.13 is 16 bits, value = raw/2^13; Q0.16 is 16 bits, value = raw/2^16.
  - So G = raw/2^18 from CPd bits 35:50.
- **Q10. Table 31 (SUVd) reserved bits 19:31 name the wrong modem** as the one that sets them. Read it as: the digital modem sets them to 0.
- **Q11. 9.11 says cleardown is drn = 0 "in SUVu or SUVd"**, but SUV sequences have no drn field. drn exists only in CPu, CPus and CPd.
  - Implement cleardown as drn = 0 in the CP sequence that is sent.
- **Q12. What "received after 100 ms + RTD" means in 9.6.x.1.3.**
  - It could refer to when reception of that sequence starts or when it ends.
  - This digest uses the first sequence whose reception **completes** after the deadline, and includes it in the check.
- **Q13. The Tone A guard-tone level.**
  - V.34 10.1.2.1 as rendered says the guard tone is at nominal power, with Tone A 1 dB below.
  - For INFO, the guard tone is 7 dB below.
  - `crates/datapump/src/v34/dpsk.rs` currently uses −7 dB for both. Keep it consistent with whatever V.90 already interoperates with, and confirm with a capture.
- **Q14. The V.90 constellation power rules (V.90 8.5.2 and Table 15) are not repeated for V.92 CPu.**
  - V.90's rules: the data-mode average power is at most 3 dB above the Phase 4 average, and the Table 15 limit depends on INFO0d's maximum power.
  - [INFERRED] Keep enforcing them on CPu. V.92 describes itself as enhancements to V.90, and the digital modem's power limits still apply.
- **Q15. SUVu bit 26 and SUVd/SUVu bit 32 in initial Phase 4.**
  - Bit 32 is described only for rate renegotiation. Send 0 in initial training.
  - If a peer sends bit 32 = 1 during initial training, ignore it. [INFERRED]
- **Q16. "Silence" from the digital modem during the retrain's 70 ± 5 ms.**
  - Only 9.8.1.1.3 defines it (PCM codewords with Ucode 0, keeping frame alignment). Use the same here.
- **Q17. Spectral shaping of TRN2d, SUVd, CPd and Ed in V.92 Phase 4.**
  - V.92 8.8.6 adopts TRN2d from V.90 8.6.5, and V.90 8.6 says the Phase 4 downstream signals use the CPt shaping parameters in initial training and after a retrain.
  - V.92 8.6 only says that the *Phase 3* digital-modem signals are unshaped, and V.92 never states the Phase 4 rule itself.
  - This digest applies the V.90 rule: shape with the CPt a1, a2, b1, b2, Sr and ld in training, and with the preceding data-mode shaping in rate renegotiation. Confirm with a capture if one ever shows TRN2d.
