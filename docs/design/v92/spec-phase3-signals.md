# V.92 Phase 3 signals (clauses 8.5 and 8.6): implementation digest

Source: ITU-T V.92 (11/2000), `docs/specs/T-REC-V.92-200011-I.pdf`.

Pages read as rendered images (PyMuPDF, 170 dpi, with 300 dpi crops of every table):

| Document | PDF pages | Printed pages | What is there |
|---|---|---|---|
| V.92 | 28-32 | 21-25 | Clauses 8.5 (8.5.1-8.5.7) and 8.6 (8.6.1-8.6.8), Tables 20, 21, 22 |
| V.92 | 33-35 | 26-28 | Table 23 (CPu **and CPt**), which 8.5.1 points to |
| V.92 | 27-28, 25-26 | 18-21 | Tables 17, 18, 19 (INFO1d, INFO1a): UINFO, MD length, PCM-upstream selection |
| V.92 | 9-12 | 2-5 | 3.5 (Qa.b), 3.8 (LU), 5, 6.1-6.4, Figures 1 and 2 |
| V.92 | 36-38 | 29-31 | Table 24 (CPus), Table 27 (SUVu), 8.7.6 TRN2u: only to cross-check shared fields and the upstream sign convention |
| V.92 | 53-56 | 46-49 | 9.5 and Figures 10/11: the Phase 3 procedure that fixes order and lengths |
| V.90 | 13, 25-35 | 5, 17-27 | 5.3, 8.3.1 with Table 12 (DIL descriptor), 8.3.2-8.3.6, 8.4.1-8.4.5 (DIL, Jd, J'd, Sd, TRN1d), Table 14 (CP), 8.6.4 (R, Ri) |
| V.34 | 16, 33, 38, 39 | 10, 27, 32, 33 | Clause 7 (scramblers), 10.1.2.3.2 (CRC, Figure 14), 10.1.3.3-10.1.3.8 (J, MD, PP, S, TRN) |

The extracted text in `docs/specs/text` was used only to find things. It is wrong in places that matter here, for example it shifts the rows of Table 22 so that bit 49 seems to be the training constellation size (the rendered table has it at bit 48). Every layout below comes from the rendered pages.

The Recommendation is paraphrased throughout. Anything marked **(derived)** was worked out for this digest and is not text from the Recommendation. Items marked **(inference)** are readings of unclear text.

Companion digests: `spec-phase3-procedures.md` covers clause 9.5 in full. `spec-phase4-signals-analogue.md` and `spec-phase4-signals-digital.md` cover CPu, CPd, TRN2u and the rest of Phase 4.

---

## 0. Conventions

| Notation | Meaning |
|---|---|
| T | One symbol interval. With PCM upstream, **both directions run at 8000 symbols/s**, so T = 125 µs. The upstream rate is taken from the digital network (6.2/V.92), and the downstream rate is 8000 (clause 5/V.92, which points to 5.2/V.90). Useful values: 12T = 1.5 ms, 24T = 3 ms, 24.5T = 3.0625 ms, 48T = 6 ms, 144T = 18 ms, 384T = 48 ms, 2040T = 255 ms, 276T = 34.5 ms. |
| X̄ | The same signal with its sign pattern inverted (R̄u, S̄u, S̄d, R̄i). The text extraction loses the bars; they were read from the pages. |
| `a:b` | A field written "LSB:MSB". Bit a is the least significant bit **and is sent first**. Every frame in this digest is sent bit 0 first. |
| Upstream data frame | **12 symbols**, data frame intervals i = 0..11 (Figure 1/V.92; 8.7.1 says so again). |
| Downstream data frame | **6 symbols**, intervals i = 0..5 (5.4/V.90). |
| LU | The upstream reference level. It is chosen so that TRN1u goes out at the wanted data-mode transmit power (3.8/V.92). |
| UINFO | A Ucode chosen by the analogue modem and sent in INFO1a bits 25:31 (Tables 18 and 19/V.92). The digital modem uses its PCM codeword for all of its 2-point Phase 3 signals. The codeword's power **shall not** exceed the digital modem's maximum transmit power, and UINFO **shall** be greater than 66. |
| Ucode, Uchord | As in clause 3/V.90 (V.92 3.6 refers there). Ucodes are 0..127. Uchord c (c = 1..8) holds Ucodes 16(c-1) to 16c-1. Table 1/V.90 lists the positive codewords; the codeword's MSB is the G.711 polarity bit. |
| Signed Qa.b | a+b+1 bits, two's complement, b fraction bits, range [-2^a, 2^a) (3.5/V.92). Signed Q1.6 is therefore 8 bits, value = code / 64, range [-2, 2). |
| Unsigned Qa.b | a+b bits with b fraction bits (3.5/V.92). Unsigned Q3.13 is 16 bits, value = code / 8192. See ambiguity A11 about its range. |
| RTD | Round-trip delay. |

---

## 1. When this Phase 3 applies

1. V.92 Phase 3 (clauses 8.5, 8.6 and 9.5) is the Phase 3 for **PCM upstream**. Upstream symbols are 2-point baseband values (±LU) at 8000 symbols/s, with no carrier, no pre-emphasis and no precoder.
2. The analogue modem asks for PCM upstream by sending the INFO1a of **Table 18/V.92**. In it, bits 34:36 = 6 and bits 37:39 = 6, meaning 8000 symbols/s in each direction.
   - It **shall not** use that INFO1a if bit 70 of INFO1d is 0 ("channel does not support PCM upstream").
   - In full Phase 2, it **may** use Table 18 only if both modems indicated V.92 capability: INFO0d bit 27 and INFO0a bit 26 (9.3/V.92).
   - Short Phase 2 (9.4) needs both V.92 bits and the short-Phase-2 bits (INFO0d bit 26, INFO0a bit 27).
3. After INFO1a, each modem goes on with "the appropriate Phase 3 as signalled in INFO1a" (9.4.1.1.5, 9.4.2.1.4).
   - With a Table 19 INFO1a (V.34 upstream, bits 34:36 = 3..5), the upstream is V.34 and the only sensible Phase 3 is V.90's (9.3/V.90). **(inference)** V.92 never names V.90's Phase 3 outright. See Q1.
4. Fields of the Table 18 INFO1a that Phase 3 depends on:
   - **Bits 18:24:** the length of the analogue modem's MD, 0..127, in units of **276 symbols (34.5 ms)**. Table 19 and V.90 Table 11 use 35 ms units instead.
   - **Bits 25:31:** UINFO.
   - **Bits 12:17:** precoder/prefilter capabilities. Phase 3 does not use them.
5. INFO1d bits 12:17 (Table 17/V.92) give the power reduction the analogue modem's transmitter must apply. That is what sets the "desired data mode transmit power" behind LU. **(inference)**

---

## 2. Phase 3 timeline (Figures 10 and 11/V.92)

```
Digital : silence .................................. |Sd 384T|S̄d 48T|TRN1d ≥2040T|Jd Jd ..|Jp ..|Jp' 12T|DIL .......(Fig.11: SCR)|Ri ....|R̄i 24T| -> Phase 4
                                        (≤500 ms after a descriptor)                    ^ 107 ON just before Jp'
Analogue: 70±5 ms|Ru 384T|R̄u 24T|[MD|Ru 384T|R̄u 24T]|TRN1u ≥2040T|Ja ....|silence ..|Su 144T|S̄u 24.5T|Su ....|S̄u (24+ε)T|TRN1u ≥2040T|CPt CPt ..|E1u| -> Phase 4
                                                                                                          ^ 107 ON on detecting Jp
```

- The bracketed MD group is present only when the INFO1a MD length is non-zero.
- In Figure 11 (the analogue modem asked for N = 0, "no DIL"), the digital modem sends **SCR** where Figure 10 has DIL. The Ri/CPt handshake then runs in a different order; see section 6.3.

---

## 3. Building blocks shared by all Phase 3 signals

### 3.1 Scramblers

Both are the self-synchronising scrambler of clause 7/V.34. The transmitter divides the data sequence by the generating polynomial, and the quotient bits are the output.

| Used by | Polynomial | Transmit recursion (x in, y out) | Where it is required |
|---|---|---|---|
| Analogue modem: TRN1u, Ja, CPt, E1u | **GPA = 1 + x^-5 + x^-23** (eq. 7-2/V.34) | y(n) = x(n) ⊕ y(n-5) ⊕ y(n-23) | 6.3/V.92 (**shall**); 8.5.7 names 6.3 for TRN1u |
| Digital modem: Jd, Jp, Jp', SCR, TRN1d | **GPC = 1 + x^-18 + x^-23** (eq. 7-1/V.34) | y(n) = x(n) ⊕ y(n-18) ⊕ y(n-23) | 8.6/V.92 (**shall**); clause 5/V.92 via 5.3/V.90 |

- **Descrambler:** x(n) = y(n) ⊕ y(n-5) ⊕ y(n-23) for GPA, or y(n) ⊕ y(n-18) ⊕ y(n-23) for GPC. It locks after 23 correct received bits.
- **The polynomial goes with the role, not with who dialled.** The analogue modem always uses GPA ("answer") and the digital modem always uses GPC ("call"). The repo's `v32::Scrambler` exposes these as `Mode::Answer` and `Mode::Call`.
- **Test vectors (derived).** Input all ones, register all zeros, first 48 outputs, first bit leftmost:
  - GPA: `111110000011111000001110011111000110000011100100`
  - GPC: `111111111111111111000001111111111111000000000011`
- **Consequences (derived):**
  - TRN1u starts with 5 symbols at -LU, because output 1 maps to negative upstream.
  - TRN1d starts with 18 positive symbols, because sign 1 maps to positive downstream.

### 3.2 Sign conventions, which are opposite in the two directions

| Direction | Bit 0 gives | Bit 1 gives | Source |
|---|---|---|---|
| Upstream: TRN1u, and Ja/CPt/E1u, which use "the same modulation as TRN1u" | **+LU** (positive voltage) | **-LU** (negative voltage) | 8.5.7 |
| Downstream: TRN1d, Jd, Jp, Jp', SCR, and the DIL sign pattern SP | **negative** voltage | **positive** voltage | 8.6.2, 8.6.3, 8.6.4, 8.6.6; 8.4.1 and 8.4.5/V.90 |

- 8.5.1 and 8.5.4 do not repeat the mapping for CPt and Ja. They only say "same modulation as TRN1u", which gives 0 → +LU.
- The Phase 4 upstream tables agree: Table 28/V.92 gives TRN2u sign bit 0 → positive.

### 3.3 Binary differential encoding

**Used by:** Ja, CPt, E1u upstream; Jd, Jp, Jp' downstream.

- **Encoder:** d(n) = s(n) ⊕ d(n-1).
  - s(n) is the **scrambler output**. The order is scramble first, then differentially encode.
  - d(n) picks the sign as in 3.2.
  - 8.5.1 and 8.5.4 describe this as "modulo 2 addition of the present bit with the previously transmitted bit".
- **Decoder:** s(n) = d(n) ⊕ d(n-1), then descramble. A polarity inversion in the channel therefore does no harm.
- **Initial memory d(-1):**

| Signal | d(-1) comes from | Clause, force |
|---|---|---|
| Ja | the final symbol of the TRN1u just before it (the first TRN1u) | 8.5.4, **shall** |
| CPt (with its 24-one preamble) | the final symbol of the TRN1u just before it (the second TRN1u) | 8.5.1, **shall** |
| E1u | no rule given; it continues from the last CPt symbol | 8.5.2 (inference) |
| Jd | the final symbol of TRN1d | 8.6.2, **shall** |
| Jp | the final symbol of Jd | 8.6.3, **shall** |
| Jp' | the final symbol of Jp | 8.6.4, **shall** |

- **How "initialised with the final symbol" is read here (inference):** d(-1) is the bit value that, under the direction's mapping in 3.2, gives that symbol's sign.
  - Upstream: d(-1) = 0 if the last symbol was +LU, 1 if it was -LU.
  - Downstream: d(-1) = 1 if the last symbol was positive, 0 if negative.
  - TRN1u and TRN1d are not differentially encoded, so for them this is simply the last scrambler output bit.
  - The repo's V.90 digital modem does exactly this (`v90/digital.rs`: `sign ^= scrambler.scramble(bit)`, carried on from TRN1d).
- **Scrambler state across the switch:** no reset is specified for Ja, CPt, Jd, Jp or Jp', so the scrambler keeps running from the preceding TRN.
- **Not differentially encoded:**
  - TRN1u, TRN1d and SCR: the sign is the scrambler output directly.
  - Ru/R̄u, Su/S̄u, Sd/S̄d, Ri/R̄i and DIL: fixed patterns, not scrambled.

### 3.4 Framing: sync, start bits, CRC, fill

Every framed Phase 3 sequence (DIL descriptor, CPt, Jd, Jp) uses the same V.34 MP-style layout:

- **Frame sync:** bits 0:16 are seventeen binary 1s.
- **Information:** 16-bit blocks, each preceded by a **start bit 0**. The first start bit is bit 17.
- **CRC:** 16 bits, after a start bit of its own.
- **Fill:** zeros, as each table says.

The start bits make sure no run of 17 ones can occur inside a frame. All of this is the bit stream **before** scrambling. A receiver descrambles (and differentially decodes where applicable) and only then looks for the sync.

**CRC (10.1.2.3.2/V.34 with Figure 14/V.34), as V.92 8.5.1, 8.5.4, 8.6.2 and 8.6.3 require:**

- **Polynomial:** x^16 + x^12 + x^5 + 1.
- **Coverage:** every information bit of the sequence, in transmission order. That excludes the frame sync, **all start bits** and **all fill bits**. Reserved bits, the SP/TP zero padding and the reserved bits of the Ucode list are information bits and **are** covered.
- **Algorithm:**
  1. Load the 16-cell register with all ones.
  2. Shift the information bits in.
  3. The register is the CRC. Its cell 0 is the CRC's LSB and is sent first. There is no final inversion.
- **Figure 14 structure:** cells 15..0, shifting toward cell 0. The feedback f = cell0 ⊕ input bit enters cell 15 and is XORed into the inputs of cells 10 and 3.
- **Equivalent code:**

```
reg = 0xFFFF
for bit in information_bits:          # transmission order
    f = (reg & 1) ^ bit
    reg >>= 1
    if f: reg ^= 0x8408               # cells 15, 10, 3
crc_field_bit[k] = (reg >> k) & 1     # k = 0 is sent first
```

The repo already has this as `crates/datapump/src/v34/info.rs::crc`. `v90/sequences.rs::frame()` builds the sync/start-bit/CRC layout, and it has been checked against a real modem's V.90 Ja.

### 3.5 Data-frame alignment

- **Downstream.** Every Phase 3 downstream signal is a whole number of 6-symbol frames: Sd 384, S̄d 48, TRN1d a multiple of 6, Jd and Jp 72 each, Jp' 12, DIL segments 6(Hc+1), SCR a multiple of 6, R period 6, R̄i 24.
  - The frame phase fixed at the first symbol of Sd, which **shall** be kept (8.4.4/V.90), therefore carries on without any extra padding **(derived)**.
- **Upstream.**
  - The digital modem **shall** keep data-frame-interval alignment from the **first symbol of the second TRN1u** (8.5.7). Clause 9.5.1.1.10 puts this as a modulo-12 count from the first symbol of that TRN1u.
  - Everything after it is a whole number of 12-symbol frames: the TRN1u segment (a multiple of 12), the 24-one CPt preamble, each CPt (padded to a multiple of 12) and E1u (12).
  - Phase 4 upstream therefore also starts on a frame boundary **(derived)**.
  - The first TRN1u, Ja and Su do **not** define the upstream frame phase.

---

## 4. Analogue modem signals (clause 8.5)

**No precoder or prefilter is used anywhere in Phase 3 upstream.**

- 8.5.5 requires Ru/R̄u to bypass the precoder and prefilter and to use the transmit structure of the 2-point TRN1u.
- The precoder and prefilter coefficients only arrive in CPd, in Phase 4.
- The 6.4 chain (modulus encoder, precoder, prefilter, gain G) first runs at B1u, where its memories are zeroed (8.7.1). **(derived from context)**

### 4.1 CPt: constellation parameters for training (8.5.1, Table 23/V.92)

**Purpose.** CPt carries the modulation parameters the digital modem uses **during training**. That means Phase 4's TRN2d, which in V.92 is still 8.6.5/V.90, the TRN2d "constellation set passed in CPt" (8.8.6/V.92).

**Requirements**

- **CPt-1 (8.5.1).** CPt is sent with **the same modulation as TRN1u**: 2-point ±LU, 1 bit per symbol, 8000 symbols/s, no precoder.
- **CPt-2 (8.5.1).** CPt is **scrambled** (GPA, 6.3) and **differentially encoded** as in 3.3.
- **CPt-3 (8.5.1, shall).** The differential encoder memory is initialised with the **final symbol of the TRN1u just before it**.
- **CPt-4 (8.5.1, shall).** **24 differentially encoded binary ones** are sent before the **first** CPt of a series. See ambiguity A4.
- **CPt-5 (8.5.1).** Fields are as in Table 23, **bit 0 first**.
- **CPt-6 (8.5.1).** CPt is variable length.
  - A constellation mask is 128 bits, one per Ucode. A 1 means the constellation includes that Ucode's PCM code.
- **CPt-7 (8.5.1, may).** A constellation that is the same in two or more data frame intervals **need only be sent once**.
  - The intervals are the **downstream** intervals 0..5, because CPt describes the digital modem's transmit constellations.
  - Sent constellations are numbered from 0 (bits 136:271) up to at most 5 (bits 816:951).
- **CPt-8 (8.5.1, shall).** If the digital modem's transmit constellations differ from those at the output to the codec's D/A converter:
  - **bit 128 shall be set**, and
  - the D/A-output constellation that corresponds to **each** transmit constellation **shall** be sent.
- **CPt-9 (8.5.1).** Two length parameters:
  - **γ = 136 × (the largest constellation index in bits 103:127)**;
  - **δ = 2γ + 136** if bit 128 is set, **δ = γ** if bit 128 is clear.
- **CPt-10 (8.5.1).** The CRC is 10.1.2.3.2/V.34 (see 3.4).
- **CPt-11 (8.5.1, shall).** When several CPt are sent as a group, **all of them shall carry identical information**.
- **CPt-12 (Table 23, shall).** ld (bits 49:50) **shall** be consistent with the digital modem's capability announced in Jd (Jd bits 49:50).
- **CPt-13 (Table 23).** Reserved bits are sent as 0 and the digital modem does not interpret them.

**Table 23/V.92 as it applies to CPt.** The same table defines CPu (8.7.3).

| Bits | Field |
|---|---|
| 0:16 | Frame sync `11111111111111111` |
| 17 | Start bit 0 |
| 18 | **CP: 0**. SUVu (Table 27/V.92) has 1 in this position, so bit 18 tells CP frames from SUV frames. |
| 19:20 | **Type** (2-bit integer): **0 = CPt**, 1 = CPu. Table 24/V.92 uses 2 for CPus. |
| 21:25 | **drn**, 0..22, the chosen downstream rate. **drn = 0 means cleardown.** Rate = **(drn + 8) × 8000/6** bit/s in CPt, and (drn + 20) × 8000/6 in CPu. In CPt, drn 1..22 covers 12 000..40 000 bit/s, the Phase 4 TRN2d rates of Table 17/V.90 (derived). |
| 26:30 | Reserved for ITU: 0 |
| 31:32 | **Sr**: number of sign bits used as redundancy for spectral shaping (5.4.1/V.90) |
| 33 | **Acknowledge**: 0 = no CPd received from the digital modem yet, 1 = CPd received. See A9. |
| 34 | Start bit 0 |
| 35 | **Codec type**: 0 = µ-law, 1 = A-law |
| 36:48 | Reserved for ITU: 0 |
| 49:50 | **ld**: number of look-ahead frames asked for in spectral shaping (**shall** be consistent with Jd) |
| 51 | Start bit 0 |
| 52:67 | RMS of TRN1d at the digital modem's transmitter output ÷ RMS of TRN1d at the output to the codec's D/A converter, **unsigned Q3.13** (value = code/8192) |
| 68 | Start bit 0 |
| 69:76 | **a1** of the spectral shaping filter, **signed Q1.6** (8 bits, value = code/64) |
| 77:84 | **a2**, signed Q1.6 |
| 85 | Start bit 0 |
| 86:93 | **b1**, signed Q1.6 |
| 94:101 | **b2**, signed Q1.6 |
| 102 | Start bit 0 |
| 103:106 | Constellation index (0..5) for downstream data frame interval 0 |
| 107:110 | Index for interval 1 |
| 111:114 | Index for interval 2 |
| 115:118 | Index for interval 3 |
| 119 | Start bit 0 |
| 120:123 | Index for interval 4 |
| 124:127 | Index for interval 5 |
| 128 | 1 = the transmitter constellations differ from those at the codec D/A output |
| 129:135 | Reserved for ITU: 0 |
| 136 | Start bit 0 |
| 137:152 | Mask, Uchord1 (bit 137 = Ucode 0 ... bit 152 = Ucode 15) |
| 153 | Start bit 0 |
| 154:169 | Mask, Uchord2 (bit 154 = Ucode 16) |
| 170 | Start bit 0 |
| 171:186 | Mask, Uchord3 (bit 171 = Ucode 32) |
| 187 | Start bit 0 |
| 188:203 | Mask, Uchord4 (bit 188 = Ucode 48) |
| 204 | Start bit 0 |
| 205:220 | Mask, Uchord5 (bit 205 = Ucode 64) |
| 221 | Start bit 0 |
| 222:237 | Mask, Uchord6 (bit 222 = Ucode 80) |
| 238 | Start bit 0 |
| 239:254 | Mask, Uchord7 (bit 239 = Ucode 96) |
| 255 | Start bit 0 |
| 256:271 | Mask, Uchord8 (bit 256 = Ucode 112 ... bit 271 = Ucode 127) |
| 272 : 271+γ | Any further transmit constellations (indices 1..max), each in the 136-bit format of bits 136:271 |
| 272+γ : 271+δ | The matching codec (D/A output) constellations, in the same format. Present only when bit 128 = 1. |
| 272+δ | Start bit 0 |
| 273+δ : 288+δ | CRC |
| 289+δ | Fill bit 0 |
| (290+δ)... | Fill zeros up to the next **multiple of 12 symbols**. The table prints this row as "289+δ:…"; see A10. |

**Addressing (derived)**

- Constellation m (m = 0..5, including the codec copies, which follow on in order) occupies bits 136+136m .. 271+136m.
- In that block, the Uchord-c mask starts at bit 137 + 136m + 17(c-1), and its j-th bit (j = 0..15) is Ucode 16(c-1) + j.
- Because CPt carries 1 bit per symbol, "12 symbols" is 12 bits.

**Lengths (derived)**, as bits up to and including the fill bit → padded length:

| Largest index | Bit 128 = 0 | Bit 128 = 1 |
|---|---|---|
| 0 | 290 → 300 | 426 → 432 |
| 1 | 426 → 432 | 698 → 708 |
| 5 | 970 → 972 | 1786 → 1788 |

A CPt group is `24 ones` followed by `CPt, CPt, ...`, with no preamble between repetitions. The fill zeros at the end of one CPt are followed directly by the next CPt's 17-one sync.

**Worked example (derived)**, pre-scrambler, bit 0 leftmost:

- Parameters: drn = 16 (32 000 bit/s training rate), Sr = 0, ack = 0, µ-law, ld = 0, RMS ratio = 1.0 (0x2000), a1 = a2 = b1 = b2 = 0, all six indices 0, bit 128 = 0, one mask with Ucodes 0..79 set.
- Result: CRC = **0xAC4D**, 300 bits in total.
- Bits 18:33 = `0000000100000000`, 35:50 = all zeros, 52:67 = `0000000000000100`.
- The tail is start bit `0`, CRC `1011001000110101`, fill `0`, then 10 padding zeros.

**Differences from V.90's CP (Table 14/V.90).** These stop `v90/sequences.rs::Cp` being reused as it stands.

| Bits | V.90 CP | V.92 CPt/CPu |
|---|---|---|
| 18 | reserved | "CP: 0" marker |
| 19 | 0 = CPt, 1 = CP | Low bit of the 2-bit Type in 19:20 |
| drn | **20:24** | **21:25** |
| 25:29 / 26:30 | reserved | reserved (26:30) |
| 30 | silence request ("CPs") | reserved. In V.92 the silence request is in SUVu bit 32. |
| 33 | acknowledge = MP received | acknowledge = CPd received |
| 36:48 | V.34 upstream rate mask (4800..33 600) | reserved. The PCM upstream rate mask is in the Ja descriptor (4.4). |
| Fill | 289+δ:291+δ = `000` | one fill bit, then zeros to a multiple of 12 symbols |
| Modulation | 10.1.3.9/V.34, 4 or 16 points (Jd bit 47), scrambler and differential encoder **reset to zero** before the first CPt | 2-point TRN1u modulation, differential encoder carried on from TRN1u, 24-one preamble |

Bits 31:32, 35, 49:271+δ and the CRC position keep their V.90 meaning.

### 4.2 E1u: end of CPt (8.5.2)

- **E1u-1.** E1u is **one data frame** of **scrambled, differentially encoded binary zeros**. An upstream data frame is 12 symbols (Figure 1, 8.7.1), so E1u is 12 zeros.
- **E1u-2.** It marks the end of the CPt series and uses the same modulation as CPt.
- **E1u-3 (inference).** The scrambler and the differential encoder simply continue from the last CPt symbol.
- **E1u-4 (context, 9.5.2.1.10/11).** The analogue modem finishes the CPt it is sending and then sends E1u.
- **E1u-5 (context, 8.7.6).** TRN2u's differential encoder memory **shall** start from the last sign bit sent in E1u. TRN2u's scrambler **shall** be reset.
- **Receiver view (derived).**
  - Every CPt ends on a 12-symbol boundary.
  - At that boundary, the start of another CPt shows as descrambled ones (the sync), while E1u shows as zeros.

### 4.3 MD: manufacturer-defined signal (8.5.3 → 10.1.3.5/V.34)

- **MD-1 (V.34 10.1.3.5).** MD is **optional** and manufacturer-defined. A modem may use it to train its echo canceller when TRN cannot serve for that in Phase 3.
- **MD-2.** Its length is announced in the sender's INFO1. A length of **0** means MD is not sent.
- **MD-3 (Table 18/V.92).** For PCM upstream the length is INFO1a bits 18:24 × **276 symbols (34.5 ms)**, 0..127. That is at most 35 052 symbols, about 4.38 s (derived). 276 = 23 × 12, so MD keeps the 12-symbol granularity (derived).
- **MD-4.** The content is not specified. The digital modem only waits it out (9.5.1.1.1).
- **MD-5 (context).** MD is sent between the first R̄u and a second Ru/R̄u pair (9.5.2.1.1).
- **Not applicable.** INFO1d bits 18:24 announce a digital-modem MD, but no digital-modem MD appears in Figure 10/11 or 9.5.1. See A14.

### 4.4 Ja and the V.92 DIL descriptor (8.5.4, Table 20/V.92, which builds on 8.3.1 and Table 12/V.90)

**Requirements**

- **Ja-1 (8.5.4).** Ja is **24 binary ones** followed by **repetitions of the DIL descriptor** of Table 20.
- **Ja-2 (8.5.4).** With **N = 0** the descriptor is **276 bits** long.
  - Check (derived): β = 34, the fill bit is bit 272, and 273 bits pad up to 276.
- **Ja-3 (8.5.4).** Ja uses **the TRN1u modulation** (2-point ±LU, 1 bit per symbol).
- **Ja-4 (8.5.4).** Ja is **scrambled** (GPA) and **differentially encoded** (3.3).
- **Ja-5 (8.5.4, shall).** At the start of Ja, the differential encoder memory is initialised with **the final symbol of the TRN1u just before it**.
- **Ja-6 (8.5.4, may).** Ja **may** stop part-way through its last descriptor.
- **Ja-7 (8.5.4, shall).** Ja **shall** be a whole number of **12-bit** units long.
  - Each descriptor is zero-padded to a 12-bit multiple (last row of Table 20), and the 24-one preamble is itself a 12-multiple.
  - The procedure ends Ja **at the next 12-bit boundary** after the Sd→S̄d transition is detected (9.5.2.1.3).
- **Ja-8 (8.5.4).** CRC per 10.1.2.3.2/V.34, over the whole descriptor's information bits. That includes the V.90 part and the new rate masks (3.4).
- **Ja-9 (8.3.1/V.90, carried along by Table 20's reference).** The variable positions use:
  - **α = ⌈L_SP/16⌉ × 17** and **β = α + ⌈L_TP/16⌉ × 17**, with ⌈x⌉ the smallest integer ≥ x;
  - **SP and TP shall be zero-padded** to the next multiple of 16 bits when their length is not one;
  - with **N = 0**, L_SP − 1 = L_TP − 1 = 0, so α = 17 and β = 34, and SP and TP carry no meaning.
- **Ja-10 (8.3.1/V.90 NOTE, advisory).** The analogue modem should preferably ask for a DIL that does not let echo-control devices in the network re-enable ("highly desirable").
  - The same NOTE's permission to send SCR during DIL is **replaced** in V.92: the analogue modem sends TRN1u during DIL (9.5.2.1.9).

Define **P = β + ⌈N/2⌉ × 17**.

**The complete V.92 DIL descriptor.** Rows marked V.90 are Table 12/V.90, which Table 20/V.92 includes as bits 0 : 187+P.

| Bits | Field | From |
|---|---|---|
| 0:16 | Frame sync, 17 ones | V.90 |
| 17 | Start bit 0 | V.90 |
| 18:25 | **N**, the number of DIL segments, 0..255 | V.90 |
| 26:33 | Reserved: 0 | V.90 |
| 34 | Start bit 0 | V.90 |
| 35:41 | **L_SP − 1** (L_SP = 1..128) | V.90 |
| 42 | Reserved: 0 | V.90 |
| 43:49 | **L_TP − 1** (L_TP = 1..128) | V.90 |
| 50 | Reserved: 0 | V.90 |
| 51 | Start bit 0 | V.90 |
| 52:67 | **SP**, first 16 bits (SP bit 0 is at bit 52) | V.90 |
| 68 | Start bit 0 | V.90 |
| ... | Any further SP in 16-bit blocks, each after a start bit 0 | V.90 |
| 51+α | Start bit 0 (the same bit as 68 when L_SP ≤ 16) | V.90 |
| 52+α : 67+α | **TP**, first 16 bits | V.90 |
| 68+α | Start bit 0 | V.90 |
| ... | Any further TP in 16-bit blocks | V.90 |
| 51+β | Start bit 0 | V.90 |
| 52+β : 58+β | **H1** (7 bits) | V.90 |
| 59+β | Reserved 0 | V.90 |
| 60+β : 66+β | **H2** | V.90 |
| 67+β | Reserved 0 | V.90 |
| 68+β | Start bit 0 | V.90 |
| 69+β : 75+β | **H3** | V.90 |
| 76+β | Reserved 0 | V.90 |
| 77+β : 83+β | **H4** | V.90 |
| 84+β | Reserved 0 | V.90 |
| 85+β | Start bit 0 | V.90 |
| 86+β : 92+β | **H5** | V.90 |
| 93+β | Reserved 0 | V.90 |
| 94+β : 100+β | **H6** | V.90 |
| 101+β | Reserved 0 | V.90 |
| 102+β | Start bit 0 | V.90 |
| 103+β : 109+β | **H7** | V.90 |
| 110+β | Reserved 0 | V.90 |
| 111+β : 117+β | **H8** | V.90 |
| 118+β | Reserved 0 | V.90 |
| 119+β | Start bit 0 | V.90 |
| 120+β : 126+β | **REF1** (Ucode) | V.90 |
| 127+β | Reserved 0 | V.90 |
| 128+β : 134+β | **REF2** | V.90 |
| 135+β | Reserved 0 | V.90 |
| 136+β | Start bit 0 | V.90 |
| 137+β : 143+β | **REF3** | V.90 |
| 144+β | Reserved 0 | V.90 |
| 145+β : 151+β | **REF4** | V.90 |
| 152+β | Reserved 0 | V.90 |
| 153+β | Start bit 0 | V.90 |
| 154+β : 160+β | **REF5** | V.90 |
| 161+β | Reserved 0 | V.90 |
| 162+β : 168+β | **REF6** | V.90 |
| 169+β | Reserved 0 | V.90 |
| 170+β | Start bit 0 | V.90 |
| 171+β : 177+β | **REF7** | V.90 |
| 178+β | Reserved 0 | V.90 |
| 179+β : 185+β | **REF8** | V.90 |
| 186+β | Reserved 0 | V.90 |
| 187+β | Start bit 0 | V.90 |
| 188+β : 194+β | Ucode of the training symbol for DIL segment 1 (present when N ≥ 1) | V.90 |
| 195+β | Reserved 0 | V.90 |
| 196+β : 202+β | Ucode for segment 2 | V.90 |
| 203+β | Reserved 0 | V.90 |
| 204+β | Start bit 0 | V.90 |
| ... | The remaining Ucodes, two per 16-bit block (7 bits and a reserved bit each), a start bit every 16 bits. When N is odd, **9 reserved bits** fill the last block. | V.90 |
| **187+P** | **Start bit 0**, the last bit taken from V.90. V.90's CRC followed straight after it. | V.90 |
| **188+P : 203+P** | **Upstream rate mask, part 1.** Bit 188+P+k is 24 000 + k×8000/6 bit/s for k = 0..15: 24 000, 25 333, 26 667, 28 000, 29 333, 30 667, 32 000, 33 333, 34 667, 36 000, 37 333, 38 667, 40 000, 41 333, 42 667, 44 000. 1 = supported and enabled in the analogue modem's transmitter. | **V.92** |
| **204+P** | **Start bit 0** | **V.92** |
| **205+P : 220+P** | **Upstream rate mask, part 2.** 205+P = 45 333, 206+P = 46 667, 207+P = 48 000. 208+P .. 220+P are reserved: sent 0, not interpreted by the digital modem. | **V.92** |
| **221+P** | **Start bit 0** | **V.92** |
| **222+P : 237+P** | **CRC** | **V.92** |
| **238+P** | **Fill bit 0** | **V.92** |
| **239+P ...** | **Fill zeros** to make the Ja length the next multiple of 12 bits | **V.92** |

- With N = 0 there are no Ucode fields: the start bit at 187+β is followed directly by the rate mask.
- There are 19 upstream rates in all, 24 000..48 000 bit/s in steps of 8000/6, matching 6.1/V.92.

**Descriptor lengths (derived):**

| N | L_SP | L_TP | Bits before padding | Padded |
|---|---|---|---|---|
| 0 | 1 | 1 | 273 | **276** (as the spec states) |
| 1 or 2 | ≤16 | ≤16 | 290 | 300 |
| 64 | 16 | 16 | 817 | 828 |
| 255 | 128 | 128 | 2687 | 2688 (the largest) |

**Compared with V.90's Ja:**

- V.90's descriptor ended with the CRC, one fill bit and a possible second fill bit to make the length even.
- V.90's Ja had no 24-one preamble.
- V.90's Ja was modulated as the V.34 J sequence (10.1.3.3/V.34): 4-point, 2 bits per symbol, at the V.34 symbol rate.

### 4.5 Ru and R̄u (8.5.5)

- **Ru-1.** **Ru** repeats the 6-symbol sequence **{+LU, +LU, +LU, −LU, −LU, −LU}**.
- **Ru-2.** **R̄u** repeats **{−LU, −LU, −LU, +LU, +LU, +LU}**.
- **Ru-3 (shall).** While sending Ru or R̄u, the analogue modem **shall bypass the precoder and prefilter**.
- **Ru-4 (shall).** It **shall** use the same structure it uses for the 2-point TRN1u.
- **Ru-5 (derived).** Neither is scrambled or differentially encoded. The digital modem looks for the **Ru→R̄u reversal** (9.5.1.1.1/2), so absolute polarity does not matter.
- **Ru-6 (context, 9.5.2.1.1).** Ru lasts **384T** (64 periods) and R̄u **24T** (4 periods). A second pair of the same lengths follows MD when MD is present.
- **Spectrum (derived).**
  - Mean power is LU².
  - Over one period the DFT magnitudes are |X1| = |X5| = 4·LU and |X3| = 2·LU, with X0 = X2 = X4 = 0.
  - That puts lines at **1333.3 Hz and 4000 Hz**.

### 4.6 Su and S̄u (8.5.6)

- **Su-1.** **Su** repeats **{+√(3/2)·LU, 0, +√(3/2)·LU, −√(3/2)·LU, 0, −√(3/2)·LU}**.
- **Su-2.** **S̄u** repeats **{−√(3/2)·LU, 0, −√(3/2)·LU, +√(3/2)·LU, 0, +√(3/2)·LU}**.
- **Su-3 (shall).** Su and S̄u **shall** each be a whole number of **12 symbols** long. The procedure contradicts this with 24.5T and (24+ε)T; see A1.
- **Spectrum (derived).**
  - Mean power is exactly LU², the same as TRN1u and Ru.
  - Lines at 1333.3 Hz (|X| = 2.449·LU) and 4000 Hz (|X| = 4.899·LU).
  - The shape copies V.90's Sd ({+W, +0, +W, −W, −0, −W}), with a true zero in the Ucode-0 positions.
- **Purpose (9.5.1.1.6/7, should).** The digital modem **should** measure the phase information on Su.
  - The CO A/D sampling instants are fixed, and the digital modem cannot move them (8.6.3).
  - Su lets it measure where those instants fall relative to the analogue modem's symbols. Jp then asks the analogue modem to move its transmit timing.
- **Durations (context, 9.5.2.1.6-8):**

| Segment | Length | Trigger |
|---|---|---|
| Su | 144T | Sent after Jd is received. The analogue modem may first wait up to 5000 ms from the silence that follows Ja. |
| S̄u | **24.5T** | directly after the 144T |
| Su | open-ended | until Jp is detected |
| S̄u | **(24 + ε)T**, 0 ≤ ε < 1, ε from Jp bits 18:33 | sent on detecting Jp; circuit 107 goes ON at the same moment |
| then TRN1u | see 4.7 | |

### 4.7 TRN1u (8.5.7)

- **TRN1u-1.** TRN1u is a sequence of **±LU** values. Its signs come from feeding **binary ones** into the scrambler of 6.3 (**GPA**).
- **TRN1u-2.** Scrambler output **0 → positive (+LU)**, **1 → negative (−LU)**.
- **TRN1u-3 (shall).** The scrambler **shall** be set to zero before TRN1u is sent. See A6 on the second segment.
- **TRN1u-4 (shall).** Every TRN1u segment **shall** be a whole number of **12 symbols**.
- **TRN1u-5 (shall, a digital-modem requirement).** The digital modem **shall** keep data-frame-interval alignment from the **first symbol of the second TRN1u**.
- **TRN1u-6 (3.8).** LU is set so that TRN1u is sent at the wanted data-mode transmit power.
- **TRN1u-7 (8.5.5, derived).** No precoder, no prefilter and no differential encoding.
- **Segments in Phase 3 (context):**
  1. **After Ru/R̄u (and MD):** at least 2040T (9.5.2.1.2, shall).
     - From the start of MD to the end of this TRN1u **shall not** take more than RTD + 4000 ms.
     - The digital modem trains its equaliser on it and starts looking for Ja after the first 2040T (9.5.1.1.3).
  2. **After the second S̄u, while the analogue modem receives DIL or SCR:** a multiple of 12 symbols (9.5.2.1.9), and **at least 2040T when a non-zero DIL was requested**.
     - CPt follows it (9.5.2.1.10/11).
     - The digital modem's modulo-12 upstream frame count starts at its first symbol.
- **Phase 4 is different.** It uses TRN2u (8.7.6): 4 or 8 levels as Jp bits 48/49 request, levels (1/√5)LU and (3/√5)LU for 4 points, and a scrambler reset at its start.

---

## 5. Digital modem signals (clause 8.6)

**General requirements (8.6):**

- **D-1 (shall).** The digital modem **shall** use **GPC** (eq. 7-1/V.34) when generating Jd, Jp, Jp', SCR and TRN1d.
- **D-2.** Nothing the digital modem sends in Phase 3 is **spectrally shaped**. That includes DIL, Sd, Ri and R̄i.
- **D-3 (derived).** Every downstream Phase 3 signal is a whole number of 6-symbol frames (3.5), so frame alignment from Sd needs no padding.

| Signal | Length in symbols |
|---|---|
| Sd | 384 |
| S̄d | 48 |
| TRN1d | multiple of 6, at least 2040 |
| Jd | 72 × k |
| Jp | 72 × k |
| Jp' | 12 |
| DIL segment | 6(Hc + 1) |
| SCR | multiple of 6 |
| Ri | multiple of 6 (period 6) |
| R̄i | 24 |

### 5.1 DIL (8.6.1 → 8.4.1/V.90)

The parameters come from the DIL descriptor in Ja (4.4). What 8.4.1/V.90 requires:

- **DIL-1.** DIL is made of **N DIL-segments**, 0 ≤ N ≤ 255.
- **DIL-2.** A segment whose training symbol belongs to Uchord c (1 ≤ c ≤ 8) is **Lc = (Hc + 1) × 6 symbols** long. Hc is 7 bits, so Lc runs from 6 to 768 symbols (derived).
- **DIL-3 (shall).** **H1 shall** set the length of Uchord1 segments, ..., **H8 shall** set the length of Uchord8 segments.
- **DIL-4 (shall).** **REFc** is the Ucode of the **reference symbol**. REF1 **shall** be used in segments whose training symbol is from Uchord1, ..., REF8 in Uchord8 segments.
- **DIL-5.** One **sign pattern SP** and one **training pattern TP** apply to the whole DIL. 1 ≤ L_SP ≤ 128 and 1 ≤ L_TP ≤ 128.
- **DIL-6 (shall).** SP bit **0 = negative, 1 = positive**.
- **DIL-7 (shall).** TP bit **0 = send REFc, 1 = send the segment's training symbol**.
- **DIL-8.** The **LSB of each pattern** applies to the **first symbol** of a segment.
- **DIL-9.** Both patterns **restart at every segment**. Inside segments longer than L_SP or L_TP, each pattern repeats independently of the other.
- **DIL-10.** The **whole sequence**, not only its last segment, repeats until the analogue modem ends it or a timeout expires.
- **DIL-11 (shall).** DIL **shall** end on a **segment boundary**.
- **DIL-12.** The N Ucodes in the descriptor give the training symbols of segments 1..N, in order.
- **DIL-13.** With **N = 0**, no DIL is sent. In V.92 the digital modem sends **SCR** in its place (9.5.1.1.11).
- **DIL-14 (derived).** DIL symbols are plain PCM codewords with the sign from SP. They are not scrambled or differentially encoded, and not shaped (D-2).
- **DIL-15 (context).** DIL starts straight after Jp' (9.5.1.1.11).
  - It ends when CPt is received: the digital modem finishes the segment in progress (DIL-11), then sends Ri (9.5.1.1.12).
  - The analogue modem starting CPt signals that it has received enough DIL (9.5.2.1.11).
- **Existing code.** `v90::sequences::Descriptor::symbols()` already produces this sequence, and `v90::dil::design` picks a descriptor. DIL itself is unchanged in V.92.

### 5.2 Jd (8.6.2, Table 21/V.92)

**Requirements**

- **Jd-1.** Jd is a **whole number of repetitions** of the 72-bit pattern below, **bit 0 first**.
- **Jd-2.** The bits are **scrambled (GPC)** and **differentially encoded**, then sent as the **sign of the PCM codeword whose Ucode is UINFO**. Sign 0 = negative voltage, sign 1 = positive voltage.
- **Jd-3 (shall).** The differential encoder is initialised with the **final symbol of TRN1d**.
- **Jd-4.** CRC per 10.1.2.3.2/V.34, covering bits 18:33 and 35:50.

| Bits | Field |
|---|---|
| 0:16 | Frame sync, 17 ones |
| 17 | Start bit 0 |
| 18:33 | **Downstream rate mask.** Bit 18+k is 28 000 + k×8000/6 bit/s for k = 0..15: bit 18 = 28 000, 19 = 29 333, 20 = 30 667, ..., 33 = 48 000. 1 = supported and enabled in the digital modem's transmitter. |
| 34 | Start bit 0 |
| 35:40 | **Rate mask, continued:** 35 = 49 333, 36 = 50 667, 37 = 52 000, 38 = 53 333, 39 = 54 667, 40 = 56 000 |
| 41:46 | Reserved for ITU: the digital modem sends 0 and the analogue modem ignores them |
| **47** | **Jd/Jp identifier: 0 = Jd** |
| **48** | **Reserved for ITU**: 0, not interpreted |
| 49:50 | The digital modem's **maximum look-ahead for spectral shaping**, 1..3 |
| 51 | Start bit 0 |
| 52:67 | CRC |
| 68:71 | Fill `0000` |

**Changes from V.90's Jd (Table 13/V.90):**

- **Bit 47** used to choose a 4- or 16-point constellation for CP, E and SCR during training. In V.92 it is the **Jd/Jp identifier** and is 0 in Jd.
- **Bit 48** used to make the same choice for rate renegotiation. In V.92 it is **reserved**.
- The constellation choice moved to **Jp bits 48/49**. It now selects **4 or 8** points, for the Phase 4 upstream signals CPu, E2u, SUVu and TRN2u.
- The rate mask, the look-ahead field, the CRC and the fill are unchanged.
- Consequence: `v90::sequences::Jd` must send its two constellation flags as 0 for V.92. A V.90-style parser will also accept a **Jp** as a Jd full of nonsense rates, so **always check bit 47**.

**Test vectors (derived)**, pre-scrambler, bit 0 leftmost:

- All 22 rates, look-ahead 1, CRC 0x776E:
  `11111111111111111 0 1111111111111111 0 1111110000000010 0 0111011011101110 0000`
- All 22 rates, look-ahead 3, CRC 0xF366:
  `11111111111111111 0 1111111111111111 0 1111110000000011 0 0110011011001111 0000`

### 5.3 Jp (8.6.3, Table 22/V.92)

**Requirements**

- **Jp-1.** Jp is a **whole number of repetitions** of the 72-bit pattern below, bit 0 first.
- **Jp-2.** Scrambled (GPC), differentially encoded, and sent as the sign of the UINFO codeword. Sign 0 = negative, 1 = positive.
- **Jp-3 (shall).** The differential encoder is initialised with the **final symbol of Jd**, so Jp carries on from Jd with no break.
- **Jp-4 (shall).** The digital modem cannot change the sampling phase of the central-office A/D converter. It **shall** therefore use Jp to ask the analogue modem to shift its **transmitter phase** by an amount in **[0, 1) symbol**, which is [0, T) seconds.
- **Jp-5.** CRC per 10.1.2.3.2/V.34, covering bits 18:33 and 35:50.

| Bits | Field |
|---|---|
| 0:16 | Frame sync, 17 ones |
| 17 | Start bit 0 |
| **18:33** | **ε**: the fraction by which the S̄u that goes with the Jp→Jp' transition has to be **lengthened**. A 16-bit unsigned integer covering [0, 1) symbol ([0, T) s). This digest reads it as ε = code / 65 536 symbols, which is about 1.9 µs per step (see A3). |
| 34 | Start bit 0 |
| 35:46 | Reserved for ITU: 0, not interpreted |
| **47** | **Jd/Jp identifier: 1 = Jp** |
| **48** | Constellation size for **CPu, E2u, SUVu and TRN2u during training** (Phase 4 of start-up or retrain): **0 = 4 points, 1 = 8 points** |
| **49** | The same, **during rate renegotiation**: 0 = 4 points, 1 = 8 points |
| 50 | Reserved for ITU: 0, not interpreted |
| 51 | Start bit 0 |
| 52:67 | CRC |
| 68:71 | Fill `0000` |

8.7.6/V.92 says the same thing from the other side: TRN2u uses 4 or 8 points as Jp bits 48 and 49 request.

**Test vectors (derived)**, pre-scrambler, bit 0 leftmost:

| ε | Bit 48 | Bit 49 | CRC | Bits |
|---|---|---|---|---|
| 0x8000 | 0 | 0 | 0x1F4C | `11111111111111111 0 0000000000000001 0 0000000000001000 0 0011001011111000 0000` |
| 0x8000 | 1 | 0 | 0x3E4E | `11111111111111111 0 0000000000000001 0 0000000000001100 0 0111001001111100 0000` (agrees with `spec-phase3-procedures.md`) |
| 0 | 1 | 1 | 0x70A6 | `11111111111111111 0 0000000000000000 0 0000000000001110 0 0110010100001110 0000` |

### 5.4 Jp' (8.6.4)

- **Jp'-1.** Jp' ends Jp. It is **12 binary zeros**, scrambled (GPC) and differentially encoded, sent as the sign of the UINFO codeword (0 = negative, 1 = positive). That is two downstream data frames.
- **Jp'-2 (shall).** The differential encoder is initialised with the **final symbol of Jp**.
- **Jp'-3 (context, 9.5.1.1.9).** When the digital modem detects the second Su→S̄u reversal:
  1. it finishes the Jp repetition in progress,
  2. asserts **circuit 107**,
  3. then sends Jp'.
- **Jp'-4 (context).** DIL (or SCR) follows Jp' directly.
  - **Derived:** DIL's first symbol is exactly 12 symbols after the end of the last Jp repetition, so a receiver that has tracked Jp's 72-bit framing knows where segment 1 begins.
- **Receiver view (derived).** After descrambling and differential decoding, the analogue modem sees Jp's `0000` fill followed by 12 more zeros where the next 17-one sync would have been.
- **J'd is gone.** V.90's J'd (8.4.3/V.90) has the same form but is **not** used in V.92 Phase 3: Jd leads into Jp, and Jp' closes the pair.

### 5.5 Ri and R̄i (8.6.5 → 8.6.4/V.90)

- **Ri-1.** **R** repeats a 6-symbol sequence of PCM codewords with the sign pattern **+ + + − − −**, leftmost sign first.
- **Ri-2.** **R̄** is **4 repetitions (24 symbols)** of the same codewords with the pattern **− − − + + +**.
- **Ri-3.** **Ri** (and R̄i) is R (and R̄) built from the **single codeword UINFO** in every data frame interval.
- **Ri-4 (V.90 NOTE).** R and R̄ are **not** differentially encoded, so the receiver must detect them **in either polarity**.
- **Ri-5 (derived).** Ri is not scrambled and, being a Phase 3 signal, not spectrally shaped (D-2). It starts on a 6-symbol frame boundary automatically (3.5).
- **Ri-6 (context).** Ri has no fixed length. It lasts until CPt is received (N ≠ 0) or until E1u is received (N = 0). R̄i is 24T and is followed by Phase 4.
- **Other R variants.** V.90's Rd and Rt, and V.92's Rf (8.8.4/V.92), are not used in Phase 3.

### 5.6 SCR, the digital modem's (8.6.6)

- **SCR-1.** SCR is the **UINFO codeword** with **signs** made by feeding **binary ones** into the scrambler (GPC). Sign 0 = negative, 1 = positive. There is no differential encoding.
- **SCR-2 (need not).** The scrambler **does not need** to be initialised at the start of SCR, so it may just continue from Jp'.
- **SCR-3 (shall).** SCR **shall** be a whole number of **6 symbols**.
- **SCR-4 (context).** SCR replaces DIL when the analogue modem asked for N = 0 (Figure 11, 9.5.1.1.11). It runs until the digital modem is "sufficiently trained", then Ri follows (9.5.1.1.13).
- **Not to be confused with V.90's SCR (8.3.5/V.90).** That one is sent by the **analogue** modem with V.34 modulation, and is not used in V.92 PCM-upstream Phase 3.

### 5.7 Sd and S̄d (8.6.7 → 8.4.4/V.90)

- **Sd-1.** **Sd** is **64 repetitions** (384T) of **{+W, +0, +W, −W, −0, −W}**.
  - **W** is the PCM codeword whose Ucode is **16 + UINFO**.
  - **0** is the codeword with **Ucode 0**, carrying the sign shown.
  - +0 and −0 are different octets (derived from Table 1/V.90 with the MSB as the polarity bit): µ-law 0xFF and 0x7F, A-law 0xD5 and 0x55.
- **Sd-2.** **S̄d** is **8 repetitions** (48T) of **{−W, −0, −W, +W, +0, +W}**.
- **Sd-3.** The first symbol of Sd is in **data frame interval 0**.
- **Sd-4 (shall).** The digital modem **shall** keep data-frame alignment from that point on.
- **Sd-5 (derived).** Ucode 16 + UINFO must exist (≤ 127), so with the INFO1a rule UINFO must be **67..111**.
- **Sd-6 (context).** The analogue modem waits for the **Sd→S̄d** reversal (9.5.2.1.3). The digital modem **may** wait up to 500 ms after a DIL descriptor before starting Sd (9.5.1.1.3).

### 5.8 TRN1d (8.6.8 → 8.4.5/V.90)

- **TRN1d-1.** TRN1d is the **UINFO codeword** with signs made by feeding binary ones into the scrambler (5.3/V.90, i.e. GPC). Sign 0 = negative, 1 = positive. There is no differential encoding.
- **TRN1d-2.** The scrambler is **set to zero** before TRN1d.
- **TRN1d-3 (shall).** TRN1d **shall** be a whole number of **6 symbols**.
- **TRN1d-4 (context, 9.5.1.1.4).** TRN1d lasts **at least 2040T**, and Jd **shall** start within 4000 ms of the start of TRN1d. The analogue modem trains its equaliser on the first 2040T and then looks for Jd (9.5.2.1.4/5).

---

## 6. Procedure context (clause 9.5; full detail in `spec-phase3-procedures.md`)

This context is repeated here because it fixes each signal's length, order and termination. D is the digital modem and A the analogue modem.

### 6.1 Digital modem (9.5.1.1)

1. **9.5.1.1.1.** D starts silent and looks for Ru and the R̄u after it.
   - If INFO1a gives MD length 0, go to step 2.
   - Otherwise, after the Ru→R̄u reversal, wait for the MD time, then look for Ru and the next Ru→R̄u reversal.
2. **9.5.1.1.2.** After Ru and the Ru→R̄u reversal, D starts training its equaliser on TRN1u.
3. **9.5.1.1.3.** After the first 2040T of TRN1u, D looks for Ja. After receiving a DIL descriptor, D **may** wait up to 500 ms, then **shall** send Sd for 384T and S̄d for 48T.
4. **9.5.1.1.4.** D sends TRN1d for at least 2040T. **Within 4000 ms** of starting TRN1d, D sends Jd and looks for Su.
5. **9.5.1.1.5.** D keeps repeating Jd.
6. **9.5.1.1.6.** On detecting Su, D looks for the Su→S̄u reversal. D **should** use Su to measure the phase.
7. **9.5.1.1.7.** On Su→S̄u, D keeps receiving Su and **should** keep measuring.
8. **9.5.1.1.8.** Once D knows the right phase adjustment, it **finishes the current Jd, then sends Jp**, and looks for the next Su→S̄u reversal.
9. **9.5.1.1.9.** On that reversal, D **finishes the current Jp, asserts circuit 107, then sends Jp'**.
10. **9.5.1.1.10.** D receives TRN1u and keeps a **modulo-12 data-frame count from its first symbol**.
11. **9.5.1.1.11.** After Jp', D sends the requested DIL and looks for CPt. If N = 0, D sends **SCR** instead and goes to step 13.
12. **9.5.1.1.12.** On receiving CPt, D sends **Ri**. On receiving the **E1u** that ends the CPts, D sends **R̄i** and enters Phase 4.
13. **9.5.1.1.13 (N = 0).** Once D is **sufficiently trained**, it sends **Ri** and looks for CPt. On receiving CPt, D sends **R̄i** and enters Phase 4.

**Recovery (9.5.1.2):**

- D **may** start a retrain at any time in Phase 3 (9.7.1.1). If it detects Tone A, it **shall** respond as 9.7.1.2 says.
- **9.5.1.2.1.** No Ja within **4500 ms + RTD from the end of INFO1a** → retrain.
- **9.5.1.2.2.** No Su within **5100 ms + RTD from the start of TRN1d** → retrain.

### 6.2 Analogue modem (9.5.2.1)

1. **9.5.2.1.1.** After INFO1a, A sends silence for **70 ± 5 ms**, then **Ru 384T** and **R̄u 24T**.
   - If the MD length is 0, go to step 2.
   - Otherwise send **MD** for the INFO1a length, then **Ru 384T** and **R̄u 24T** again.
2. **9.5.2.1.2.** A sends **TRN1u for at least 2040T**. From the start of MD to the end of TRN1u **shall not** exceed **RTD + 4000 ms**.
3. **9.5.2.1.3.** A sends **Ja** and looks for Sd and the Sd→S̄d reversal. After that reversal, A **ends Ja at the next 12-bit boundary and goes silent**.
4. **9.5.2.1.4.** A trains its equaliser on the first 2040T of TRN1d.
5. **9.5.2.1.5.** After 2040T of TRN1d, A looks for Jd.
6. **9.5.2.1.6.** After receiving Jd, A **may** wait up to **5000 ms from the start of the silence in step 3**, then **shall** send **Su for 144T**.
7. **9.5.2.1.7.** A sends **S̄u for 24.5T**, then **Su**, and looks for Jp.
8. **9.5.2.1.8.** On detecting Jp, A **asserts circuit 107** and sends **S̄u for 24T plus the 0-to-1-symbol fraction given in Jp**, then looks for Jp'.
9. **9.5.2.1.9.** On detecting Jp', A receives the DIL it asked for (or SCR if N = 0). Meanwhile A sends **TRN1u** in 12-symbol multiples, **at least 2040T when N ≠ 0**.
10. **9.5.2.1.10 (N = 0).** A waits for **Ri**, then sends CPt. On receiving **R̄i**, A finishes the current CPt, sends **E1u** and enters Phase 4.
11. **9.5.2.1.11 (N ≠ 0).** A sends at least 2040T of TRN1u and then CPt, **within 5000 ms of sending the S̄u in step 8**. This tells D that enough DIL has arrived. A keeps sending CPt until it receives **Ri**, then finishes the current CPt, sends **E1u** and enters Phase 4.

**Recovery (9.5.2.2):**

- A **may** start a retrain at any time (9.7.2.1). If it detects Tone B, it **shall** respond as 9.7.2.2 says.
- **9.5.2.2.1.** No Sd→S̄d reversal within **1500 ms from the start of Ja** → retrain.
- **9.5.2.2.2.** No Jd within **4500 ms from the end of Ja** → retrain.

### 6.3 How Phase 3 ends

| Case | Digital modem | Analogue modem |
|---|---|---|
| N ≠ 0 (Figure 10) | DIL until CPt is heard, then Ri; on E1u, R̄i → Phase 4 | TRN1u ≥ 2040T, then 24 ones + CPt, CPt, ... until Ri is heard; finishes the CPt, E1u → Phase 4 |
| N = 0 (Figure 11) | SCR until trained, then Ri; on CPt, R̄i → Phase 4 | TRN1u until Ri is heard, then 24 ones + CPt, ...; on R̄i, finishes the CPt, E1u → Phase 4 |

---

## 7. Timers, lengths and tolerances

| Item | Value | Sender | Clause |
|---|---|---|---|
| Silence after INFO1a | 70 ± 5 ms | A | 9.5.2.1.1 |
| Ru / R̄u | 384T / 24T (each pair) | A | 9.5.2.1.1 |
| MD | INFO1a bits 18:24 × 276T (34.5 ms), 0..127 | A | Table 18 |
| First TRN1u | ≥ 2040T, in 12T multiples | A | 9.5.2.1.2, 8.5.7 |
| Start of MD to end of first TRN1u | ≤ RTD + 4000 ms | A | 9.5.2.1.2 |
| Ja preamble | 24 ones | A | 8.5.4 |
| Ja length | a multiple of 12 bits; ends at the next 12-bit boundary after Sd→S̄d | A | 8.5.4, 9.5.2.1.3 |
| DIL descriptor, N = 0 | 276 bits | A | 8.5.4 |
| Wait before Sd | 0..500 ms after a descriptor (may) | D | 9.5.1.1.3 |
| Sd / S̄d | 384T / 48T | D | 9.5.1.1.3, 8.4.4/V.90 |
| TRN1d | ≥ 2040T, in 6T multiples | D | 9.5.1.1.4, 8.4.5/V.90 |
| Start of TRN1d to start of Jd | ≤ 4000 ms | D | 9.5.1.1.4 |
| Jd, Jp | 72 bits each, whole repetitions only | D | Tables 21, 22 |
| Jp' | 12 bits (12T) | D | 8.6.4 |
| Ja timeout | 4500 ms + RTD from the end of INFO1a | D | 9.5.1.2.1 |
| Su timeout | 5100 ms + RTD from the start of TRN1d | D | 9.5.1.2.2 |
| Sd→S̄d timeout | 1500 ms from the start of Ja | A | 9.5.2.2.1 |
| Jd timeout | 4500 ms from the end of Ja | A | 9.5.2.2.2 |
| Wait before Su | up to 5000 ms from the start of the silence after Ja (may) | A | 9.5.2.1.6 |
| Su, first | 144T | A | 9.5.2.1.6 |
| S̄u, first | 24.5T | A | 9.5.2.1.7 |
| Su, second | until Jp is detected | A | 9.5.2.1.7 |
| S̄u, second | (24 + ε)T, 0 ≤ ε < 1, ε = Jp[18:33] / 65 536 | A | 9.5.2.1.8, Table 22 |
| Su / S̄u granularity | a whole number of 12 symbols (8.5.6), contradicted by the two rows above | A | 8.5.6 |
| Second TRN1u | 12T multiples; ≥ 2040T if N ≠ 0 | A | 9.5.2.1.9 |
| CPt start (N ≠ 0) | ≤ 5000 ms after sending the second S̄u, and after ≥ 2040T of TRN1u | A | 9.5.2.1.11 |
| CPt preamble | 24 ones before the first CPt | A | 8.5.1 |
| CPt | padded to a multiple of 12 symbols; repeated with identical content | A | Table 23, 8.5.1 |
| E1u | 12T (one data frame) | A | 8.5.2 |
| DIL segment | (Hc + 1) × 6 symbols; ends on a segment boundary | D | 8.4.1/V.90 |
| SCR | 6T multiples | D | 8.6.6 |
| R̄i | 24T (4 × 6) | D | 8.6.4/V.90 |
| Circuit 107 | A turns it ON on detecting Jp; D turns it ON on the second Su→S̄u, before Jp' | both | 9.5.2.1.8, 9.5.1.1.9 |

---

## 8. Cross-references the implementer must follow

| Reference in V.92 | What the referenced text requires (read from the rendered pages) |
|---|---|
| 5/V.92 → 5/V.90 | The digital modem's rates, symbol rate, scrambler and encoder are V.90's. 5.3/V.90: the scrambler is clause 7/V.34 with **GPC**. |
| 6.3/V.92 → clause 7/V.34 | The analogue modem **shall** have a self-synchronising scrambler with **GPA**, eq. 7-2 (1 + x^-5 + x^-23). The transmitter divides by the polynomial. |
| 8.6/V.92 → eq. 7-1/V.34 | GPC = 1 + x^-18 + x^-23 |
| 8.5.1, 8.5.4, 8.6.2, 8.6.3 → 10.1.2.3.2/V.34 | The CRC of 3.4: x^16+x^12+x^5+1, register preset to ones, covers everything except sync, start and fill bits, register output with bit 0 = LSB sent first (Figure 14) |
| 8.5.3 → 10.1.3.5/V.34 | MD is optional and manufacturer-defined, for echo-canceller training; its length is in INFO1; 0 means absent |
| Table 20 → 8.3.1/V.90 and Table 12/V.90 | Bits 0:187+P of the descriptor; the α/β definitions; zero padding of SP/TP to 16-bit multiples (**shall**); L_SP−1 = L_TP−1 = 0 when N = 0; SP/TP meaningless when N = 0; interpretation per 8.4.1/V.90 |
| 8.6.1 → 8.4.1/V.90 | The DIL rules DIL-1..DIL-13 in 5.1 |
| 8.6.5 → 8.6.4/V.90 | R, R̄ (4 × 6), Ri with UINFO in every interval; not differentially encoded, so the receiver must accept either polarity |
| 8.6.7 → 8.4.4/V.90 | Sd and S̄d as in 5.7; the first Sd symbol is data frame interval 0; alignment **shall** be kept from there |
| 8.6.8 → 8.4.5/V.90 | TRN1d as in 5.8; the scrambler is 5.3/V.90 (GPC), zeroed first; a multiple of 6 symbols |
| 3.6/V.92 → clause 3/V.90 | Ucode and Uchord definitions; Table 1 codewords; MSB = G.711 polarity bit |
| Table 23 → Jd | ld must agree with Jd bits 49:50 |
| Table 23 fields → 5.4/V.90 | Sr (5.4.1), constellation sets per interval (5.4.1), spectral shaping filter a1, a2, b1, b2 and look-ahead ld (5.4.5) |
| 8.8.6/V.92 → 8.6.5/V.90 | TRN2d uses the constellation set passed in **CPt**. That is where CPt's content is consumed (Phase 4). |

---

## 9. Implementation notes

### 9.1 What is new compared with V.90 Phase 3 (clause 9.3 and Figures 5/6 of V.90)

1. **The whole upstream half is new.**
   - V.90 Phase 3 upstream is V.34-style QAM: S/S̄ at 128T/16T, PP, a 4-point TRN ≥ 512T, a J-modulated Ja, then S until J'd arrives, S̄ 16T, silence or SCR during DIL, and S/S̄ (128T/16T) to end DIL.
   - V.92 upstream is **baseband, 8000 symbols/s, 2-point ±LU, with no carrier and no precoder**: Ru/R̄u, MD, TRN1u, Ja, Su/S̄u, TRN1u, CPt, E1u.
2. **New signals:**
   - Ru/R̄u, the upstream counterpart of R;
   - Su/S̄u, the upstream counterpart of Sd;
   - TRN1u, which uses GPA and the opposite sign mapping to TRN1d;
   - E1u;
   - Jp and Jp', which take over J'd's role;
   - the digital modem's own SCR, used when N = 0.
   - Ri/R̄i move from the start of V.90 Phase 4 into the end of Phase 3.
3. **Sampling-phase alignment is new:** phase measurement on Su, S̄u of 24.5T, Jp's ε field, and S̄u of (24+ε)T. So is the digital modem's upstream frame count from the second TRN1u.
4. **CPt moves** from V.90 Phase 4 (where it followed Ri/R̄i) to the end of Phase 3. It has:
   - a new header (bit 18 CP marker, 2-bit Type at 19:20, drn at 21:25, no silence bit, no V.34 rate mask);
   - new fill;
   - new modulation and preamble (4.1).
5. **The DIL descriptor** gains a 19-rate upstream capability mask (24 000..48 000 bit/s) before its CRC, and is padded to 12-bit multiples. Ja gains a 24-one preamble and uses 2-point modulation.
6. **Jd bits 47/48 change meaning** (identifier / reserved). The constellation requests move to Jp bits 48/49 and now choose 4 or 8 points for the Phase 4 upstream signals.
7. **The MD length unit** is 276 symbols (34.5 ms), not 35 ms.
8. **The order of events changes.**
   - V.90: D sends Jd, J'd, then DIL; A ends DIL with S/S̄; Phase 4 opens with Ri/R̄i and CPt.
   - V.92: D sends Jd, Jp, Jp', then DIL or SCR; A ends DIL by starting CPt; D answers with Ri/R̄i, and A with E1u. **Both modems enter Phase 4 with CPt already exchanged.**
9. **Unchanged, taken by reference:** DIL (8.4.1/V.90), Sd/S̄d (8.4.4/V.90), TRN1d (8.4.5/V.90), R/Ri (8.6.4/V.90), MD (10.1.3.5/V.34), the CRC (10.1.2.3.2/V.34) and the GPA/GPC scramblers.

### 9.2 Pitfalls

- **The two directions use opposite sign conventions.** Upstream, 0 → +LU; downstream, 0 → negative.
  - A mistake inverts TRN1u or TRN1d, which breaks any receiver that correlates against the known sequence.
  - The differentially encoded frames still decode correctly, which can hide the mistake.
- **Switching from plain to differential decoding (receivers).** TRN1u and TRN1d are not differentially encoded, but the J/CP frames that follow them are.
  - A receiver has to switch decoders at a boundary it does not know exactly, because TRN is open-ended.
  - **Derived:** on the first symbol after the switch, a GPA descrambler still fed plain symbols gives 1 ⊕ (last TRN1u scrambler output). The first wrong bit can therefore be late.
  - The **24-one preamble** of Ja and CPt covers the 23-bit descrambler re-lock. If the switch lands within the preamble, the descrambler is locked again before the 17-one sync.
  - Jd has no such preamble. The repo's V.90 approach (switch on the first descrambled 0 and replay that symbol, `v90/pcm.rs::read_jd`) may lose the first Jd, but Jd repeats.
- **Runs longer than 17 ones.**
  - Before the first frame there are 24 + 17 = 41 ones (Ja, CPt), or TRN1d's descrambled ones plus 17 (the first Jd).
  - `v90/sequences.rs::Finder` only opens a candidate when a 0 follows **exactly** 17 ones, so it drops the first frame of each group.
  - For V.92, accept a 0 after **at least** 17 ones and treat the last 17 as the sync; the CRC rejects false starts. The alternative costs one repetition each time: about 37 ms for a 300-bit CPt and 9 ms for Jd.
- **Telling Jd from Jp.** The framing, length and CRC are identical. Read bit 47 **before** interpreting bits 18:33 (rate mask or ε) and 48:50.
- **Telling CPt from the other upstream frames.** Require bit 18 = 0 and Type (19:20) = 0. drn is at **21:25**, not V.90's 20:24, so `Cp::from_bits` cannot be reused unchanged. The CPt length follows from the index fields and bit 128, as V.90's `cp_length` already computes.
- **CPt/E1u framing.** The 12-symbol padding is what puts E1u and Phase 4 on an upstream frame boundary. The digital modem's modulo-12 count starts at the first symbol of the **second** TRN1u. An analogue transmitter must count its frames from that same symbol.
- **Fractional-symbol S̄u (24.5T and (24+ε)T).**
  - A fixed-rate symbol stream cannot make these by repeating symbols. In effect they are a **delay applied to the whole upstream output from that point on**.
  - Possible realisations:
    - a fractional-delay interpolator (windowed sinc or Farrow) on the transmit sample stream;
    - a transmitter that works at a higher internal rate and decimates with a movable phase.
    - If the analogue transmitter's output rate is 16 kHz, 0.5T is exactly one output sample, but ε still needs interpolation.
  - The delay must stay in force through Phase 4 and data mode, which are precoded; Phase 3 itself is not, which makes it easier there.
  - The digital-modem side needs an estimator of the CO A/D sampling phase that works on Su.
  - A loopback harness needs a "codec" whose sampling phase can be set; otherwise ε cannot be tested.
- **Our test line.** From project memory:
  - The line is a softphone/VoIP path with about 1.5 s of delay each way and ~20 ms concealment slips every few seconds.
  - The "CO A/D" may be a gateway codec, or may not exist at all if the path is G.711 end to end.
  - A slip moves both the data-frame phase and the sampling phase, so ε measured in Phase 3 can be stale by data mode.
  - The RTD-scaled timers (4500/5100 ms + RTD) matter at a ~3 s round trip. The fixed local timers (1500 ms after Ja, 4000 ms TRN1d→Jd, the 5000 ms windows) do not scale with RTD.
- **UINFO range.** Sd needs Ucode 16 + UINFO, so pick 67..111 (derived). The existing V.90 UINFO choice (`v90::dil::design(law, uinfo)`) carries over.
- **The LU level** is fixed in Phase 3 by the wanted data-mode power. TRN2u later uses fractions of LU (Tables 28/29), so keep LU the same across the Phase 3/4 boundary.
- **Building the DIL descriptor.** `Descriptor::to_bits` currently appends the CRC and even-length fill straight after bit 187+P. For V.92 it must instead:
  1. insert the two upstream rate-mask blocks, each with its start bit;
  2. then add the start bit and CRC at 221+P / 222+P;
  3. then one fill 0;
  4. then zero padding to a 12-bit multiple.
- **The `Jd` struct.** Its `sixteen_in_training` / `sixteen_in_renegotiation` fields map to bits that V.92 redefines. Send both as 0 in V.92 Jd, and add a `Jp` sibling (ε, 8-point-training, 8-point-renegotiation).
- **The digital modem's SCR vs DIL.** DIL is neither scrambled nor differentially encoded. SCR "need not" reset its scrambler, and continuing the GPC state from Jp' is simplest. The analogue receiver trains on SCR blind, since the descrambler is self-synchronising.
- **R̄i detection.** R and R̄ are not differentially encoded, so detect the R→R̄ **reversal**, not an absolute polarity. The same applies to Ru→R̄u, Su→S̄u and Sd→S̄d.
- **Circuit 107** goes ON at different points on the two sides (section 7). Only the DTE interface can see this.

### 9.3 Ambiguities and how this digest settles them

- **A1. "A whole number of 12 symbols" (8.5.6) versus S̄u of 24.5T and (24+ε)T (9.5.2.1.7/8, Figure 10).**
  - Reading: the S̄u pattern content is 24 symbols (a 12-multiple), and the extra 0.5 or ε symbol is a **timing shift of every later upstream symbol**, not extra pattern.
  - The spec does not say how the extension is produced: holding the last value, inserting zero, or shifting time.
  - Treat it as a fractional delay. **Open; confirm with a capture.**
- **A2. What ε is measured against.** The first S̄u already shifts timing by 0.5T, and the digital modem keeps measuring on the Su after it (9.5.1.1.7).
  - Reading: ε corrects the timing that holds **after** the 0.5T shift. The total shift from the timing before Su is then 0.5 + ε symbols (mod 1).
  - The spec never says why the 0.5T step exists. Perhaps it lets the digital modem resolve its phase estimate with two measurements. **Open.**
- **A3. Scaling of ε.** "16-bit unsigned integer covering [0, 1) symbol" is taken as ε = code / 2^16, with the LSB at bit 18. This is the only reading that covers [0, 1) exactly, but the text does not say it.
- **A4. Are CPt's 24 ones scrambled?** 8.5.1 calls them "differentially encoded" ones and does not mention scrambling.
  - Reading: **scrambled, then differentially encoded**, the same as Ja's 24 ones, which are part of a sequence that is explicitly scrambled.
  - Unscrambled ones would neither re-lock the digital modem's descrambler nor descramble to a run of ones.
  - A receiver that searches for the sync after descrambling works either way once the descrambler has locked, provided the preamble is long enough. **Confirm with a capture.**
- **A5. Scrambler state at Ja, CPt, Jd, Jp and Jp'.** No reset is given. Continue from the preceding signal. The receivers are self-synchronising, so this is harmless.
- **A6. Is the TRN1u scrambler reset for the second segment?**
  - 8.5.7's "initialised to zero prior to the transmission of TRN1u" is taken to apply to **each** TRN1u segment.
  - A receiver should not depend on this: descramble in self-synchronising mode or train decision-directed.
- **A7. "Initialise the differential encoder with the final symbol".** Read as in 3.3: d(−1) is the bit that gave that symbol's sign under the direction's mapping, which for TRN1u/TRN1d is the last scrambler output.
- **A8. Sign mapping for CPt, Ja and E1u.** Taken from "same modulation as TRN1u": 0 → +LU. Table 28 (TRN2u, MSB = sign, 0 → positive) is consistent with this.
- **A9. CPt acknowledge bit (33).** It means "CPd has been received", and no CPd exists yet in Phase 3.
  - Send 0 and ignore it on receipt.
  - The bit matters for CPu, which shares Table 23.
- **A10. Table 23's two fill rows both start at "289+δ"** (the single fill bit, and the "0s to extend" padding).
  - Reading: the fill bit is at 289+δ and padding starts at 290+δ. The same shape appears in Tables 24 and 27 (fill bit 51, padding from 52).
  - "12 symbols" means 12 bits for CPt.
- **A11. Unsigned Q3.13 range.** 3.5/V.92 says unsigned Qa.b uses a+b bits (16 here) with range [0, 2^(a+1)), which would be [0, 16). Sixteen bits with 13 fractional bits can only reach [0, 8).
  - Use code/8192 in [0, 8). Real ratios are near 1, so this does not matter in practice.
- **A12. γ's "maximum constellation index given in bits 103:127"** means the largest of the six 4-bit index fields. Bit 119 is a start bit, not part of any index.
- **A13. How DIL ends in V.92.** 9.5.1.1.12 says to send Ri when CPt arrives. Apply 8.4.1/V.90 and finish the current segment first, as V.90's own 9.3.1.6 did.
- **A14. INFO1d bits 18:24** (MD length for the digital modem) exist in Table 17/V.92, but no downstream MD appears in the PCM-upstream Phase 3 (Figures 10/11, 9.5.1).
  - Send 0 and do not expect a downstream MD. **Open.**
- **A15. 9.5.1.2.2** cites 9.5.1.1.9 for the Su timeout, yet Su is first awaited in 9.5.1.1.4-6. Apply the timeout to the first detection of Su.
- **A16. Using the upstream rate mask.** It lists rates "supported and enabled in the analogue modem's transmitter". The digital modem presumably uses it when choosing the upstream rate in CPd (Phase 4 digest).
- **A17. The V.34-upstream case.** Section 1, item 3: V.92 does not say in so many words that a Table 19 INFO1a leads to V.90's Phase 3.
- **A18. Ri length and alignment.** Neither is stated. Ri runs until the next event, and it starts on a 6-symbol boundary automatically (3.5).

### 9.4 Questions for Rory

1. **Q1. V.34 upstream.** When the analogue modem selects V.34 upstream (Table 19 INFO1a), should we simply run the existing V.90 Phase 3 (A17)?
2. **Q2. The fractional S̄u (A1, A2).** How should we produce it: an interpolating fractional-delay filter on the upstream output, or an oversampled transmitter?
   - Is sampling-phase alignment even meaningful on the softphone/VoIP rig? Is there a real CO A/D anywhere in the path?
3. **Q3. N = 0 or a real DIL first?** N = 0 (SCR path) is simpler. A real DIL reuses `v90::dil::design`, but adds ≥ 2040T of upstream TRN1u, which the analogue receiver must survive through its own echo, and the CPt/Ri handshake.
4. **Q4. MD.** Always send MD length 0?
5. **Q5. Loopback harness.** Should the digital-modem simulator model a settable A/D sampling phase and the codec law, so that Su phase estimation and Jp's ε can be tested offline?
6. **Q6. Captures.** Do we have, or can we record, a real V.92 server's Phase 3? Jd/Jp, and ideally our own Ja/CPt heard back, would settle A1-A4. Captures are the only allowed source besides the Recommendation.

### 9.5 Existing code this builds on (read-only survey)

| Path | Relevance |
|---|---|
| `crates/datapump/src/v34/info.rs` | `crc()`: exactly the Figure 14 CRC of 3.4 |
| `crates/datapump/src/v32.rs` | `Scrambler` with `Mode::Call` (GPC) and `Mode::Answer` (GPA); `scramble`/`descramble` match 3.1 |
| `crates/datapump/src/v90/sequences.rs` | `frame`/`unframe` (sync, start bits, CRC); `Jd` (bits 47/48 need V.92 meanings, plus a `Jp` sibling); `Descriptor` (needs the V.92 rate-mask tail and 12-bit padding); `Cp` (header differs from V.92 CPt, see 4.1); `Finder` (see the pitfall on runs longer than 17 ones) |
| `crates/datapump/src/v90/dil.rs` | DIL design and analysis on the analogue side; reusable as is |
| `crates/datapump/src/v90/digital.rs`, `pcm.rs` | Downstream Sd, TRN1d, Jd (with differential encoding carried on from TRN1d) and R generation, and the Jd reader. The V.92 order (Jd → Jp → Jp' → DIL/SCR → Ri/R̄i) and an upstream PCM receiver (Ru detection, TRN1u equaliser, Ja, Su phase estimation, CPt, E1u) are still to be written. |
