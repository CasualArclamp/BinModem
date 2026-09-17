# V.92 Phase 2 procedures: implementation digest (clauses 9.3 and 9.4)

Source: ITU-T V.92 (11/2000), `docs/specs/T-REC-V.92-200011-I.pdf`. The referenced
texts are V.90 (09/98), V.34 (02/98), V.8 (11/2000), V.8 bis (11/2000) and V.42 (03/2002),
all in `docs/specs/`.

Everything below was read from **rendered PDF pages**. The extracted `.txt` files were used
only to find things. Clause numbers are the Recommendation's own. The requirements are
paraphrased. Bit layouts, field widths, values, timings and tolerances are exact.

Keywords, as used in this file:

- **SHALL**: a mandatory requirement.
- **SHOULD**: a recommendation.
- **MAY**: an option.
- **[derived]**: a conclusion drawn by the digest author, not text from the Recommendation.
- **[ambiguity]**: something the Recommendation leaves open. These are collected in section 12.

## 0. Pages read

| Document | PDF page(s) | Printed page(s) | Content |
|---|---|---|---|
| V.92 | 50, 51, 52, 53 | 43, 44, 45, 46 | 9.2.3.3 to 9.2.5; **9.3; 9.3.1; 9.4 with Figure 9; 9.4.1; 9.4.2**; start of 9.5 (Figures 10 and 11) |
| V.92 | 23 to 28 | 16 to 21 | 8.4, 8.4.1, Tables 15 to 19 (INFO0d, INFO0a, INFO1d, INFO1a for PCM upstream, INFO1a for V.34 upstream in short Phase 2) |
| V.92 | 46 | 39 | Clause 9 intro, 9.1 with its note, Figure 3 |
| V.92 | 56, 59, 60 | 49, 52, 53 | 9.5.2.2 (Phase 3 recovery), 9.6.x.2 (Phase 4 timers), **9.7 Retrains** |
| V.90 | 20 to 24 | 12 to 16 | 8.2 (Phase 2 signals), 8.2.3.1 modulation, Tables 7 to 10, 8.2.3.3 INFOMARKS, 8.2.4 |
| V.90 | 37 to 40 | 29 to 32 | 9.1.2/9.1.3 (75 ± 5 ms), **9.2 Phase 2 with Figure 4, 9.2.1, 9.2.2** |
| V.90 | 44 | 36 | 9.5 Retrains (V.90's re-entry points) |
| V.34 | 32, 33, 34, 36, 37 | 26, 27, 28, 30, 31 | 10.1.2 Phase 2 power, 10.1.2.1 Tone A, 10.1.2.2 Tone B, 10.1.2.3.1 modulation, Figure 13, **10.1.2.3.2 CRC with Figure 14**, Table 14 INFO0, Table 16 INFO1a, 10.1.2.3.6 INFOMARKS, 10.1.2.4 L1/L2 with Table 17 |
| V.34 | 50 | 44 | 11.2.2 recovery mechanisms (for comparison) |
| V.8 | 12, 14 | 7, 9 | Table 5 (pcm0), Table 6 (prot0: LAPM), 7.3 (the LAPM/ODP-ADP note that cites 9.3.1/V.92) |
| V.8 bis | 24 | 17 | Table 6-3a Data NPar(2): "V.42 error control" bit |
| V.42 | 17 | 9 | 7.2.1.2 and 7.2.1.3: the ODP/ADP detection phase that 9.3.1 bypasses |

---

## 1. Where Phase 2 sits and what V.92 changes

V.92's start-up has four phases (clause 9). Phases 1 and 2 each come in a **full** and a
**short** form:

- **Full Phase 2** (9.3) is V.90's Phase 2 (9.2/V.90) unchanged as a procedure: INFO0
  exchange, two rounds of ranging, L1/L2 probing in both directions, then INFO1d and
  INFO1a. What V.92 changes is **which INFO bit layouts are used**, what INFO1a may ask for
  (PCM upstream), and that later retrains use V.92's Phase 2.
- **Short Phase 2** (9.4) is new. It keeps the INFO0 exchange and a **single** ranging
  exchange started by the **digital** modem. It drops probing (no L1/L2), the second
  ranging round and INFO1d. The analogue modem sends INFO1a straight after its Tone A
  reversal.
- **ODP/ADP bypass** (9.3.1): if both modems are V.92 and both indicated LAPM in V.8 or
  V.8 bis, the V.42 detection phase is skipped.

Phase 2 always starts after the **75 ± 5 ms silence** that ends Phase 1. That silence
comes from V.90 9.1.2.1/9.1.3.2 for full Phase 1 and from V.92 9.2.x for short Phase 1.

---

## 2. Deciding which Phase 2 to run, and which INFO layouts to use

### 2.1 The capability bits (Tables 15 and 16). The positions differ between INFO0d and INFO0a

| Meaning | INFO0d (digital) bit | INFO0a (analogue) bit |
|---|---|---|
| V.92 capability (value 1) | **27** | **26** |
| Request short Phase 2 (1 = request) | **26** | **27** |
| Acknowledge the far INFO0 (error recovery) | 28 | 28 |

In V.90 (Tables 7 and 8/V.90), bits 26:27 of both INFO0d and INFO0a are "reserved for the
ITU, set to 0, not interpreted". A V.90 modem therefore sends 0 in both and ignores them.
In V.34 (Table 14/V.34), bits 26:27 of INFO0 are the transmit clock source. See pitfall P1.

### 2.2 Rules

- **R-9.3-a (SHALL).** If INFO0d bit 27 = 1 **and** INFO0a bit 26 = 1 (both V.92), the
  digital modem SHALL build INFO1d from **Table 17/V.92** (8.4.1).
- **R-9.3-b (MAY).** In that case the analogue modem MAY choose PCM upstream by sending
  INFO1a laid out as **Table 18/V.92**.
- **R-9.3-c (SHALL).** If either modem does not indicate V.92, both modems SHALL use the
  V.90 layouts of 8.2.3.2/V.90: Table 9/V.90 for INFO1d, Table 10/V.90 or Table 11/V.90
  for INFO1a.
  - [derived] INFO0d and INFO0a are sent before either side knows the other's
    capabilities. So in practice a V.92 modem always sends the Table 15/16 layout, with
    its V.92 bit = 1. Rule R-9.3-c affects only the INFO1 sequences.
- **R-9.3-d (SHALL).** If both indicate V.92, every later retrain SHALL use Phase 2 of V.92.
  - The retrain procedures of 9.7 always re-enter **full** Phase 2 (section 7). A retrain
    never uses short Phase 2 and never repeats the INFO0 exchange.
- **R-9.4-a (SHALL).** If both modems indicate V.92 (INFO0d b27, INFO0a b26) **and** both
  request short Phase 2 (INFO0d b26, INFO0a b27), both SHALL follow 9.4 (short Phase 2).
  Otherwise they follow 9.3 (full Phase 2).
- **R-9.4-b (SHALL, analogue modem).** The analogue modem SHALL set INFO0a bit 27 only if
  it intends to connect in **PCM upstream** or **V.90 data mode** (PCM downstream, V.34
  upstream).
  - [derived] So short Phase 2 always ends with an INFO1a from Table 18 or Table 19. V.34
    fallback (INFO1a with bits 37:39 in 0..5) is not reachable from short Phase 2.
- The Recommendation sets **no condition** on when the digital modem may set INFO0d bit 26.
  It also does not tie short Phase 2 to short Phase 1: the request travels in the INFO0
  bits, so it can follow full Phase 1 too. [derived]

[derived] Both ends compute the same four-bit decision, so they cannot disagree, provided
each has correctly received the other's INFO0 before choosing its tone behaviour:

- In the error-free flow, both INFO0s cross before either tone is detected.
- In the recovery flow (sections 4.3 and 6.3), a modem only moves on to the tone step after
  it has correctly received the far INFO0. A far INFO0 that arrives with bit 28 = 1 is, by
  definition, a correctly received INFO0.

---

## 3. Phase 2 signals

V.92 8.4: all full and short Phase 2 signals are those of V.90. V.90 8.2.1 and 8.2.2 refer
Tones A and B to V.34, and V.90 8.2.4 refers L1/L2 to V.34.

### 3.1 Transmit level (V.90 8.2)

- **SHALL.** Every Phase 2 signal except **L1** is sent at the **nominal transmit power**.
- **SHALL.** If a recovery mechanism sends a modem back to Phase 2 from a later phase, its
  transmit level goes back from the negotiated level to the nominal level.
  - V.34 10.1.2 differs: it keeps the negotiated level if the return point is after L1/L2.
    V.90, and therefore V.92, always reverts.
- The digital modem's nominal Phase 2 power is sent in INFO0d bits 29:32 (section 3.5).

### 3.2 Tone A and Tone B (V.34 10.1.2.1 and 10.1.2.2)

Tone A:

- **Frequency:** 2400 Hz. It is sent by the V.34 "answer" side; in V.90/V.92 that is the
  **analogue modem**, because INFO carriers are assigned by modem type (V.90 8.2.3.1).
- **Reversals:** A to Ā, and Ā to A, are 180° phase reversals of the 2400 Hz tone.
- **Guard tone:** while A or Ā is sent, the modem also sends an **1800 Hz guard tone**
  with no phase reversals.
- **Levels, as printed in V.34 10.1.2.1:** Tone A is **1 dB below** nominal power. The
  guard tone is "at the nominal transmit power". See ambiguity A1: under INFO sequences
  the guard tone is 7 dB below nominal, and the existing code uses −7 dB for both.

Tone B:

- **Frequency:** 1200 Hz. It is sent by the V.34 "call" side, which is the **digital
  modem** here.
- **Reversals:** B to B̄, and B̄ to B, are 180° phase reversals.
- **Guard tone:** none.
- **Level:** nominal power (V.90 8.2).

Note (both tones): the bandwidth of a tone with reversals should not be limited so much
that it hurts round-trip-delay accuracy.

### 3.3 INFO modulation (V.90 8.2.3.1, which repeats V.34 10.1.2.3.1)

- **SHALL.** Binary DPSK at **600 bit/s ± 0.01 %**.
  - A **1** rotates the transmit point **180°** from the previous point.
  - A **0** leaves it unrotated (0°).
- **Leading point.** Each INFO sequence is preceded by **one point at an arbitrary carrier
  phase**. When several INFO sequences are sent as a group, only the first gets that
  leading point.
- **Analogue modem transmitter:**
  - carrier **2400 Hz ± 0.01 %**, at **1 dB below** nominal power;
  - plus an **1800 Hz ± 0.01 %** guard tone at **7 dB below** nominal power.
- **Digital modem transmitter:** carrier **1200 Hz ± 0.01 %** at **nominal** power, with
  no guard tone.
- **SHALL.** The transmitted spectrum stays within the template of **Figure 13/V.34**
  (±600 Hz around the carrier; the existing `v34/dpsk.rs` already meets it).
- Note: linear-phase transmit filters are highly desirable, because the receiver does not
  train an adaptive equalizer on INFO.
- **INFOMARKS** (V.90 8.2.3.3): INFOMARKSd and INFOMARKSa are the DPSK modulator fed with
  continuous binary **ones**, on the sending modem's own carrier.

Durations (**[derived]**; a symbol is 1/600 s, about 1.667 ms):

| Sequence | Bits | Duration | With leading point |
|---|---|---|---|
| INFO0a | 49 | 81.67 ms | 83.33 ms |
| INFO0d | 62 | 103.33 ms | 105.00 ms |
| INFO1a (any layout) | 70 | 116.67 ms | 118.33 ms |
| INFO1d | 109 | 181.67 ms | 183.33 ms |

### 3.4 Common INFO frame structure and CRC

Every INFO frame has this shape, with **bit 0 sent first**. A multi-bit field "a:b" has its
LSB at bit a.

| Bits | Content |
|---|---|
| 0:3 | Fill: `1111` |
| 4:11 | Frame sync `01110010`, **left-most bit first in time**: b4=0, b5=1, b6=1, b7=1, b8=0, b9=0, b10=1, b11=0 |
| 12 … (N−21) | Information bits |
| (N−20):(N−5) | 16-bit CRC; CRC bit 0, its LSB, is sent first |
| (N−4):(N−1) | Fill: `1111` |

**CRC (V.34 10.1.2.3.2 and Figure 14/V.34; V.92 8.4.1 names it).**

- **Coverage.** The CRC runs over every information bit, meaning everything except the
  frame sync, "start bits" and fill bits. That is exactly bits 12 up to the bit before the
  CRC field:

  | Sequence | Bits covered | Count |
  |---|---|---|
  | INFO0d | 12:41 | 30 |
  | INFO0a | 12:28 | 17 |
  | INFO1d | 12:88 | 77 |
  | INFO1a | 12:49 | 38 |

- **Polynomial:** x^16 + x^12 + x^5 + 1.
- **Procedure:**
  1. Load the shift register with all ones.
  2. Shift the information bits in, in transmission order.
  3. Output the register contents starting at cell 0. Cell 0 is the CRC's LSB and is sent
     first. There is **no final inversion**.
- **Figure 14, read from the render.**
  - Sixteen cells, numbered 15 (left) to 0 (right); the register shifts right.
  - The feedback bit is fb = cell0 XOR input.
  - fb enters cell 15.
  - fb XOR cell 11 enters cell 10.
  - fb XOR cell 4 enters cell 3.
  - Every other cell i takes cell i+1.
- **Equivalent form:**
  ```
  reg = 0xFFFF
  for bit in info_bits:            # transmission order
      fb  = (reg & 1) ^ bit
      reg >>= 1
      if fb: reg ^= 0x8408         # cells 15, 10, 3
  crc = reg                        # send bit 0 first
  ```
  This is the reflected CCITT CRC-16 with init 0xFFFF and xorout 0 (the "CRC-16/MCRF4XX"
  parameter set). The existing `crates/datapump/src/v34/info.rs::crc` already implements
  exactly this.
- **Test vectors [derived].** Computed by the digest author from a literal Figure 14 model
  (script `scratchpad/v92/info_vectors.py`):
  - The ASCII string "123456789", each byte sent LSB first, gives CRC **0x6F91**.
  - Running the CRC over information bits followed by their own 16 CRC bits leaves
    **0x0000**, which gives a cheap receive-side check.
  - Whole frames, bit 0 on the left:

| Frame | Field values | CRC | Bits 0..N−1 |
|---|---|---|---|
| INFO0a | b12–b20 all 1, 21:23=5, b24=0, b25=1, b26=1 (V.92), b27=1 (short), b28=0 | 0xAF5A | `1111011100101111111111010111001011010111101011111` |
| INFO0a, same with b28=1 | | 0x2B52 | `1111011100101111111111010111101001010110101001111` |
| INFO0d | b12–b20 all 1, 21:23=5, b24=0, b25=1, b26=1 (short), b27=1 (V.92), b28=0, 29:32=3, 33:37=23, b38=0, b39=0 (µ-law), b40=1, b41=0 | 0xA8A9 | `11110111001011111111110101110110011101001010010101000101011111` |
| INFO1a, Table 18 | 12:13=3, 14:15=3, 16:17=3, 18:24=0, 25:31=77, 32:33=0, 34:36=6, 37:39=6, 40:49 all 1 | 0xF59A | `1111011100101111110000000101100100011011111111111101011001101011111111` |
| INFO1a, Table 19 | 12:17=0, 18:24=0, 25:31=77, b32=0, b33=1, 34:36=5, 37:39=6, 40:49=−512 | 0x7A52 | `1111011100100000000000000101100101101011000000000101001010010111101111` |

### 3.5 INFO0d, digital modem capabilities (Table 15/V.92), 62 bits

| Bits | Definition | Difference from V.90 Table 7 |
|---|---|---|
| 0:3 | Fill `1111` | |
| 4:11 | Frame sync `01110010` | |
| 12 | 1 = symbol rate 2743 supported in V.34 mode | |
| 13 | 1 = symbol rate 2800 supported in V.34 mode | |
| 14 | 1 = symbol rate 3429 supported in V.34 mode | |
| 15 | 1 = can transmit on the low carrier at symbol rate 3000 | |
| 16 | 1 = can transmit on the high carrier at symbol rate 3000 | |
| 17 | 1 = can transmit on the low carrier at symbol rate 3200 | |
| 18 | 1 = can transmit on the high carrier at symbol rate 3200 | |
| 19 | 0 = transmitting at symbol rate 3429 is disallowed | |
| 20 | 1 = can reduce transmit power below nominal, in V.34 mode | |
| 21:23 | Largest allowed transmit/receive symbol-rate difference in V.34 mode, in steps. Rates are numbered 0 = 2400 … 5 = 3429; value 0..5 | |
| 24 | 1 = sent by a CME modem | |
| 25 | 1 = supports constellations of up to 1664 points | |
| **26** | **1 = requests short Phase 2** | V.90: reserved, 0 |
| **27** | **V.92 capability: 1** | V.90: reserved, 0 |
| 28 | 1 = acknowledges correct reception of an INFO0a frame (error recovery only) | |
| 29:32 | Digital modem nominal transmit power for Phase 2, in −1 dBm0 steps: 0 = −6 dBm0 … 15 = −21 dBm0 | |
| 33:37 | Maximum digital modem transmit power, in −0.5 dBm0 steps: 0 = −0.5 dBm0 … 31 = −16 dBm0 | |
| 38 | 1 = digital modem power is measured at the codec output; 0 = at its terminals | |
| 39 | PCM coding in use: 0 = µ-law, 1 = A-law | |
| 40 | 1 = can operate V.90 with upstream symbol rate 3429 | |
| 41 | Reserved: sender sets 0; the analogue modem does not interpret it | |
| 42:57 | CRC | |
| 58:61 | Fill `1111` | |

- NOTE 1: bits 12, 13, 14 and 40 describe capabilities or configuration. Bits 15 to 20
  depend on regulatory requirements and apply only to the modem's own transmitter.
- NOTE 2: bit 24 **may** be combined with the V.8 PSTN access category octet to choose
  settings for the signal converters and error control in both modems and any CME in the
  path.

### 3.6 INFO0a, analogue modem capabilities (Table 16/V.92), 49 bits

| Bits | Definition | Difference from V.90 Table 8 |
|---|---|---|
| 0:3 | Fill `1111` | |
| 4:11 | Frame sync `01110010` | |
| 12 to 19 | As INFO0d bits 12 to 19 (V.34-mode rates 2743, 2800, 3429; low/high carrier at 3000 and 3200; 3429 disallowed if 0) | |
| 20 | 1 = can reduce transmit power below nominal **in V.34 mode or in V.90 mode** | V.90 says only "below the nominal setting" |
| 21:23 | Largest allowed symbol-rate difference in V.34 mode, 0..5 (as INFO0d) | |
| 24 | 1 = sent by a CME modem | |
| 25 | 1 = supports up to 1664-point constellations | |
| **26** | **V.92 capability: 1** | V.90: reserved, 0 |
| **27** | **1 = requests short Phase 2** | V.90: reserved, 0 |
| 28 | 1 = acknowledges correct reception of an INFO0d frame (error recovery only) | |
| 29:44 | CRC | |
| 45:48 | Fill `1111` | |

NOTES 1 and 2 are as for INFO0d, except that NOTE 1 lists bits 12 to 14.

### 3.7 INFO1d, digital modem probing results (Table 17/V.92), 109 bits (full Phase 2 only)

| Bits | Definition |
|---|---|
| 0:3 | Fill `1111` |
| 4:11 | Frame sync `01110010` |
| 12:14 | Minimum power reduction the analogue transmitter is to apply, 0..7 dB. SHALL be 0 if INFO0a said the analogue transmitter cannot reduce power. |
| 15:17 | Further reduction, below the bits 12:14 value, that the digital receiver can tolerate, 0..7 dB. SHALL be 0 under the same condition. |
| 18:24 | Length of the MD the **digital** modem sends in Phase 3: 0..127, in **35 ms** steps |
| 25 | 1 = upstream (analogue to digital) uses the high carrier at symbol rate 2400 |
| 26:29 | Upstream pre-emphasis filter index at symbol rate 2400, 0..10 (Tables 3 and 4/V.34) |
| 30:33 | Projected maximum data rate at symbol rate 2400, as a multiple of 2400 bit/s, 0..14; 0 = rate unusable |
| 34:42 | Probing results for final symbol rate 2743, 9 bits, coded like bits 25–33 |
| 43:51 | The same for 2800 |
| 52:60 | The same for 3000; SHALL be consistent with the INFO0a capabilities |
| 61:69 | The same for 3200; SHALL be consistent with the INFO0a capabilities |
| **70** | **0 = the channel does not support PCM upstream** |
| **71:78** | **Probing results for symbol rate 3429, 8 bits, coded like bits 26–33** (4 bits pre-emphasis, then 4 bits rate); SHALL be consistent with the INFO0a capabilities |
| 79:88 | Frequency offset of the probing tones as measured by the digital receiver: f(received) − f(transmitted) of the nominal 1050 Hz tone. Two's complement, −511..+511, in 0.02 Hz steps; **bit 88 is the sign bit**. SHALL be accurate to 0.25 Hz; if that is not achievable, send −512, meaning "ignore". |
| 89:104 | CRC |
| 105:108 | Fill `1111` |

- NOTE 1: a projected rate above 12 in bits 30:33 SHALL only be sent if the analogue modem
  supports 1664-point constellations.
- NOTE 2: the analogue modem may reach a higher downstream rate in V.90 mode if bits 15:17
  allow it to transmit at lower power.

**Difference from V.90 Table 9 / V.34 INFO1c.** There, bits **70:78** are a single 9-bit
3429 result coded like bits 25–33, so **bit 70 is the "high carrier" flag for 3429**.
V.92 reuses bit 70 as the PCM-upstream flag and shortens the 3429 field to 8 bits. Bits
71:74 (pre-emphasis) and 75:78 (rate) stay where they were. [derived] A V.90 decoder that
reads a V.92 INFO1d therefore sees only a changed 3429 carrier flag.

### 3.8 INFO1a layouts, all 70 bits long

INFO1a is sent by the analogue modem. Which layout it uses depends on the V.92 negotiation
and on which Phase 2 ran.

#### 3.8.1 Table 18/V.92: PCM upstream selected (full Phase 2 or short Phase 2)

- **SHALL NOT.** The analogue modem SHALL NOT send this INFO1a if **bit 70 of INFO1d is 0**.
- Short Phase 2 has no INFO1d. See ambiguity A3.

| Bits | Definition |
|---|---|
| 0:3 | Fill `1111` |
| 4:11 | Frame sync `01110010` |
| 12:13 | Number of filter sections in precoder and prefilter: 0 = p1(i) and z2(i); 1 = z1(i), p1(i), z2(i); 2 = p1(i), p2(i), z2(i); 3 = z1(i), p1(i), p2(i), z2(i) |
| 14:15 | Largest total coefficient count the analogue modem supports, Ltot = LZ1 + LP1 + LZ2 + LP2: 0 = 192, 1 = 256, 2 = 320, 3 = 384 |
| 16:17 | Largest coefficient count per section, Lmax = max{LZ1, LP1, LZ2, LP2}: 0 = 128, 1 = 192, 2 = 256, 3 = 320 |
| 18:24 | Length of the MD the **analogue** modem sends in Phase 3: 0..127, in steps of **276 symbols (34.5 ms)** |
| 25:31 | U_INFO: Ucode of the PCM codeword the digital modem uses for the 2-point train. Its power SHALL NOT exceed the maximum digital transmit power (INFO0d 33:37). U_INFO SHALL be **> 66**. |
| 32:33 | Reserved: sent as 0, not interpreted by the digital modem |
| 34:36 | Analogue modem uses symbol rate 8000: the integer **6** |
| 37:39 | Digital modem uses symbol rate 8000: the integer **6** |
| 40:49 | Reserved: sent as **1**, not interpreted by the digital modem. Note: they are ones so that no tone is generated. |
| 50:65 | CRC |
| 66:69 | Fill `1111` |

#### 3.8.2 Table 19/V.92: V.34 upstream selected **during short Phase 2** (V.90 data mode)

| Bits | Definition |
|---|---|
| 0:3 | Fill `1111` |
| 4:11 | Frame sync `01110010` |
| 12:17 | Reserved: sent as 0, not interpreted |
| 18:24 | Length of the analogue modem's Phase 3 MD: 0..127, in **35 ms** steps |
| 25:31 | U_INFO, with the same constraints as in Table 18 (> 66, power ≤ digital maximum) |
| 32 | Reserved: sent as 0, not interpreted |
| **33** | **1 = upstream uses the high carrier frequency** |
| 34:36 | Upstream symbol rate, 3..5: 3 = 3000 … 5 = 3429 |
| 37:39 | Digital modem uses symbol rate 8000: the integer **6** |
| 40:49 | Frequency offset of the probing tones as measured by the analogue receiver: the same definition and coding as INFO1d 79:88, **bit 49 is the sign bit**, −512 = ignore. Short Phase 2 has no probing tones; see ambiguity A4. |
| 50:65 | CRC |
| 66:69 | Fill `1111` |

#### 3.8.3 V.90 layouts still in use

**Table 10/V.90, "V.90 selected".** Used in full Phase 2 when the analogue modem wants PCM
downstream with V.34 upstream. It is the same as Table 19 except:

- bits **32:33 are both reserved (0)**, so there is no carrier flag;
- bits 34:36 SHALL be consistent with INFO1d, and carrier and pre-emphasis come from the
  INFO1d entry for that rate;
- V.90's printed text calls the frequency-offset sign bit "Bit 9", meaning bit 9 of the
  field, which is absolute bit 49.

**Table 11/V.90, "V.34 selected".** Its bits are identical to Table 16/V.34 INFO1a, with
the V.34 call modem read as the digital modem and the V.34 answer modem as the analogue
modem:

| Bits | Definition |
|---|---|
| 12:14 | Minimum power reduction for the digital (call) transmitter; 0 if INFO0d said it cannot reduce power |
| 15:17 | Additional reduction the analogue receiver can tolerate |
| 18:24 | Analogue MD length, 35 ms steps |
| 25 | High carrier for digital-to-analogue; SHALL be consistent with INFO0d |
| 26:29 | Pre-emphasis index for digital-to-analogue, 0..10 |
| 30:33 | Projected maximum rate for digital-to-analogue, 0..14. Values above 12 only if the remote modem supports 1664 points. |
| 34:36 | Analogue-to-digital symbol rate, 0..5; SHALL be consistent with INFO1d and with the asymmetry limits |
| 37:39 | Digital-to-analogue symbol rate, 0..5; SHALL be consistent with INFO0d capabilities and the asymmetry limits |
| 40:49 | Frequency offset, bit 49 is the sign bit |
| 50:65 | CRC |
| 66:69 | Fill |

**Where each layout may appear** [derived from 9.3, 9.4 and R-9.4-b]:

| Phase 2 | Both V.92? | INFO1a allowed |
|---|---|---|
| Full | no | Table 10/V.90 (V.90 mode) or Table 11/V.90 (V.34 mode) |
| Full | yes | **Table 18** (PCM upstream, only if INFO1d b70 = 1), Table 10/V.90, or Table 11/V.90 |
| Short | yes, by definition | **Table 18** or **Table 19** only |

**Telling the INFO1a layouts apart at the receiver** [derived]. All four are 70 bits long,
so the decoder must use bits 34:36 and 37:39:

| Bits 37:39 | Bits 34:36 | Layout |
|---|---|---|
| 6 | 6 | Table 18 (PCM upstream) |
| 6 | 3..5 | Table 10/V.90 in full Phase 2, or Table 19 in short Phase 2. Bit 33 is meaningful only in Table 19. |
| 0..5 | 0..5 | Table 11/V.90 (V.34); not allowed after short Phase 2 |
| anything else | | Treat as invalid |

### 3.9 L1 and L2 (full Phase 2 only; V.34 10.1.2.4)

- **L1:**
  - Repeats at **150 Hz ± 0.01 %**. It is a sum of cosines at every multiple of 150 Hz
    from 150 to 3750 Hz, **except 900, 1200, 1800 and 2400 Hz**.
  - The starting phases come from Table 17/V.34 (rendered page 37; the existing
    `v34/probe.rs` implements it).
  - Sent for **160 ms (24 periods)** at **6 dB above nominal** power.
- **L2:** the same waveform at **nominal** power, sent for **no more than 550 ms plus one
  round-trip delay**.
- Note: the tones **should** be generated accurately enough not to disturb the far end's
  distortion and noise measurements.

---

## 4. Clause 9.3: full Phase 2 (probing and ranging)

- **R-9.3-0 (SHALL).** The operating procedures and recovery procedures of full Phase 2 are
  **identical to V.90 Phase 2** (9.2/V.90). The only additions are the INFO-layout rules
  R-9.3-a to R-9.3-d.

Because the implementer must follow V.90 9.2 exactly, it is restated below as numbered
requirements with the V.92 substitutions applied. The **[V.92 map]** notes are the digest
author's mapping of V.90 references onto V.92.

- **Retrain references.** V.90 9.5.1.1, 9.5.1.2, 9.5.2.1 and 9.5.2.2 become V.92 9.7.1.1,
  9.7.1.2, 9.7.2.1 and 9.7.2.2.
- **Phase 3 references.** "Phase 3" means V.92 9.5 when INFO1a is Table 18, and V.90 9.3
  when INFO1a is Table 10/V.90.
- **Timekeeping.** Every time is measured at the **line terminals**. RTD means the
  round-trip-delay estimate: RTDEd at the digital modem, RTDEa at the analogue modem.

### 4.1 Timeline (Figure 4/V.90, read from the render)

```
Digital : [INFO0d][ B ......][B̄ 10ms][ silence ≤670 ms ][ B ....][B̄ 10ms][L1 160ms][ L2 ....][INFO1d]
                       ▲40±1 ms after Ā arrives                ▲40±1 ms after Ā arrives
Analogue: [INFO0a][A ≥50ms][Ā ......][A 10ms][L1 160][L2 ...][A 50ms][Ā 10ms][ silence ≤670 ms ][ A ...][INFO1a][silence 70±5 ms]→Phase 3
                                 ▲40±1 ms after B̄ arrives
```

- The analogue modem's first reversal (A to Ā) comes after it detects Tone B and has sent
  A for ≥ 50 ms.
- The digital modem answers with B̄ 40 ± 1 ms after that reversal arrives.
- The analogue modem answers B̄ with Ā to A 40 ± 1 ms after B̄ arrives, sends 10 ms of A,
  then L1 and L2.

### 4.2 Digital modem, error-free (V.90 9.2.1.1)

- **FD-1 (9.2.1.1.1, SHALL).**
  - During the 75 ± 5 ms silence that ends Phase 1, listen for INFO0a and Tone A.
  - After the silence, send INFO0d with **bit 28 = 0**, then **Tone B**.
- **FD-2 (9.2.1.1.2, SHALL).** Once INFO0a is received:
  - listen for Tone A;
  - keep receiving INFO0a, since repeats mean recovery (FDR-1);
  - watch for the Tone A phase reversal that follows.
- **FD-3 (9.2.1.1.3, SHALL).** On the Tone A reversal:
  - Send a Tone B reversal, timed so that **40 ± 1 ms** passes from the A reversal arriving
    at the line to the B reversal leaving at the line.
  - Keep sending B̄ for **10 ms**, then go **silent**.
  - Listen for a **second** Tone A reversal.
- **FD-4 (9.2.1.1.4, SHALL).** On the second A reversal:
  - Compute **RTDEd = t_rx(second A reversal) − t_tx(own B reversal) − 40 ms**.
  - Prepare to receive L1 and L2.
- **FD-5 (9.2.1.1.5).**
  - **SHALL** receive L1 for its full **160 ms**.
  - **MAY** then receive L2 for **≤ 500 ms**.
  - **SHALL** then send Tone B and listen for Tone A followed by an A reversal.
  - See pitfall P9 for the timing budget.
- **FD-6 (9.2.1.1.6, SHALL).** On Tone A followed by its reversal:
  - Send a B reversal **40 ± 1 ms** after the A reversal arrives.
  - Send B̄ for **10 ms** more.
  - Send **L1** then **L2**.
  - Listen for Tone A.
- **FD-7 (9.2.1.1.7, SHALL).** Send **INFO1d** once both of these hold:
  - Tone A has been detected;
  - the local echo of L2 has been received for no longer than **550 ms + RTD**.
  - L2 stops when INFO1d starts. [derived from Figure 4]
- **FD-8 (9.2.1.1.8, SHALL).** After INFO1d:
  - Go silent and receive INFO1a.
  - If INFO1a bits 37:39 = 6, go to Phase 3. [V.92 map] If bits 34:36 are also 6 (Table
    18), that is V.92 9.5.
  - If bits 37:39 are in 0..5, continue with **11.3.1.1/V.34** in the **call-modem** role.
  - Later retrains use Phase 2 of V.90, whatever mode was chosen. V.92 overrides this: they
    use Phase 2 of V.92 when both modems are V.92 (R-9.3-d).

### 4.3 Digital modem, recovery (V.90 9.2.1.2)

- **FDR-1 (9.2.1.2.1, SHALL).** Applies in FD-2 or FD-3 when either Tone A is detected
  before INFO0a has been received, or INFO0a keeps repeating.
  - Send INFO0d **repeatedly**.
  - Set **bit 28 = 1** once INFO0a has been received correctly.
  - If an INFO0a arrives with bit 28 = 1: listen for Tone A and then its reversal, **finish
    the INFO0d in progress**, then send Tone B.
  - Alternatively, if Tone A is detected and INFO0a has been received: listen for the A
    reversal, finish the INFO0d in progress, then send Tone B.
  - In both cases, continue at FD-3.
- **FDR-2 (9.2.1.2.2, SHALL).** If no A reversal arrives in FD-3, keep sending Tone B until
  one does. There is no timeout.
- **FDR-3 (9.2.1.2.3, SHALL).** If the second A reversal of FD-4 does not arrive within
  **2000 ms** of the reversal detected in FD-3:
  1. Go silent and listen for Tone A.
  2. When Tone A is detected, send Tone B.
  3. Listen for an A reversal and continue at FD-3.
- **FDR-4 (9.2.1.2.4, SHALL).** If no A reversal arrives in FD-6 within **900 ms + RTD** of
  the reversal detected in FD-4:
  1. Wait **40 ms**.
  2. Send a B reversal, then B̄ for **10 ms**.
  3. Send L1 then L2 and listen for Tone A.
  4. Continue at FD-7.
- **FDR-5 (9.2.1.2.5, SHALL).** If no Tone A is detected in FD-7 within **650 ms + RTD** of
  the **start of L2**, start a retrain. [V.92 map] That is 9.7.1.1.
- **FDR-6 (9.2.1.2.6, SHALL).** If no INFO1a arrives in FD-8 within **700 ms + RTD** of the
  **end of INFO1d**, listen for either Tone A or INFOMARKSa.
  - On INFOMARKSa, do one of two things (the choice is the implementer's):
    - start a retrain (9.7.1.1); or
    - resend INFO1d and continue at FD-8.
  - On Tone A: respond to the retrain (9.7.1.2).

### 4.4 Analogue modem, error-free (V.90 9.2.2.1)

- **FA-1 (9.2.2.1.1, SHALL).**
  - During the 75 ± 5 ms silence, listen for INFO0d and Tone B.
  - After the silence, send INFO0a with **bit 28 = 0**, then **Tone A**.
- **FA-2 (9.2.2.1.2, SHALL).** Once INFO0d is received, listen for Tone B and keep
  receiving INFO0d (see FAR-1).
- **FA-3 (9.2.2.1.3, SHALL).** When Tone B is detected **and** Tone A has been sent for
  **≥ 50 ms**:
  - send a Tone A reversal;
  - listen for a Tone B reversal.
- **FA-4 (9.2.2.1.4).** On the B reversal, compute **RTDEa = t_rx(B reversal) − t_tx(own A
  reversal) − 40 ms**.
- **FA-5 (9.2.2.1.5, SHALL).**
  - Send an A reversal **40 ± 1 ms** after the B reversal arrives.
  - Send A for **10 ms** after it.
  - Send **L1** then **L2**.
  - Listen for Tone B.
- **FA-6 (9.2.2.1.6, SHALL).** Once Tone B is detected, and the local L2 echo has been
  received for no longer than **550 ms + RTD**:
  - send Tone A for **50 ms**, then an A reversal, then **10 ms** more;
  - go silent;
  - listen for a B reversal.
- **FA-7 (9.2.2.1.7, SHALL).** On the B reversal, prepare to receive L1 and L2.
- **FA-8 (9.2.2.1.8).**
  - **SHALL** receive L1 for its **160 ms**.
  - **MAY** receive L2 for **≤ 500 ms**.
  - **SHALL** then send Tone A and receive INFO1d.
- **FA-9 (9.2.2.1.9, SHALL).** On INFO1d:
  - Send INFO1a, using bits 37:39 to choose V.90 or V.34 mode. [V.92 map] Or send Table 18
    for PCM upstream.
  - Then go to Phase 3, or to **11.3.1.2/V.34** in the **answer-modem** role.
  - Later retrains use Phase 2 of V.90 (V.92: R-9.3-d).
  - Figure 4/V.90 shows **70 ± 5 ms of silence** after INFO1a. V.92 9.5.2.1.1 requires the
    same.

### 4.5 Analogue modem, recovery (V.90 9.2.2.2)

- **FAR-1 (9.2.2.2.1, SHALL).** Applies in FA-2, FA-3 or FA-4 when either Tone B is
  detected before INFO0d has been received correctly, or INFO0d keeps repeating.
  - Send INFO0a **repeatedly**.
  - Set **bit 28 = 1** once INFO0d has been received correctly.
  - If an INFO0d arrives with bit 28 = 1: listen for Tone B, **finish the INFO0a in
    progress**, then send Tone A.
  - Alternatively, if Tone B is detected and INFO0d has been received: finish the INFO0a,
    then send Tone A.
  - In both cases, continue at FA-3.
- **FAR-2 (9.2.2.2.2, SHALL).** If the B reversal of FA-4 does not arrive within
  **2000 ms**, listen for Tone B and continue at FA-3. The start point of the 2000 ms is not
  stated; see ambiguity A8.
- **FAR-3 (9.2.2.2.3, SHALL).** If Tone B is not detected in FA-6 within **600 ms + RTD** of
  the **start of its own L2**:
  - listen for Tone B;
  - send Tone A;
  - continue at FA-3.
- **FAR-4 (9.2.2.2.4, SHALL).** If INFO1d does not arrive in FA-9 within **2000 ms + 2 × RTD**
  of the Tone B detection in FA-6, do one of two things:
  - start a retrain (9.7.2.1); or
  - send **INFOMARKSa** until INFO1d arrives or Tone B is detected.
    - On Tone B, continue at FA-3. V.34 11.2.2.2.4 says to respond to a retrain instead;
      V.90 says FA-3.
    - On INFO1d, continue at FA-9.

---

## 5. Clause 9.3.1 (and 9.2.5): bypassing ODP/ADP

- **R-9.3.1 (SHALL).** If both modems indicate V.92 (the INFO0 bits) **and** both indicated
  the **LAPM protocol in V.8 or V.8 bis**, the V.42 ODP/ADP exchange SHALL be bypassed.
- **R-9.2.5 (SHALL, short Phase 1).** If both modems indicated LAPM capability, the V.42
  ODP/ADP exchange SHALL be bypassed. Under short Phase 1, LAPM is signalled by the **P**
  bit of QC1a, QC2a, QCA1a, QCA2a, QC1d, QC2d, QCA1d or QCA2d; the V.92 8.2 and 8.3 tables
  say it "calls for LAPM … (see 9.2.5)".

How LAPM is indicated elsewhere:

- **V.8 (Table 6/V.8, the "prot0" octet).**
  - Tag b0–b3 = `0101`, then b4 = 0.
  - **b5 b6 b7 = `1 0 0`** means "calls for LAPM (V.42)".
  - `111` means "protocol given in an extension octet".
  - V.8 6.4: if CM indicates LAPM and the answerer wants LAPM, JM also carries a protocol
    octet indicating LAPM.
- **V.8 7.3.**
  - Including prot0 lets LAPM be negotiated without ODP/ADP.
  - If both DCEs show LAPM in prot0 they **may be required** to skip ODP/ADP, and V.8 cites
    9.3.1/V.92 as the example.
  - V.8 warns that **some existing implementations show LAPM in prot0 yet still need the
    ODP/ADP exchange**.
- **V.8 bis** (Table 6-3a/V.8 bis, Data NPar(2), octet 1): bit 2 is "V.42 error control".
- **V.42 7.2.1, the part being bypassed.**
  - Originator (7.2.1.2): the ODP is `0 1000 1000 1` + 8 to 16 ones + `0 1000 1001 1` + 8
    to 16 ones, which is DC1 with even parity and then DC1 with odd parity, each followed by
    ones. It is repeated until T400 expires or the ADP is seen, and two adjacent ADPs must
    be received correctly.
  - Originator: the detection phase **may be disabled**, in which case the originator goes
    straight to protocol establishment (7.2.2).
  - Answerer (7.2.1.3): sends continuous ones until the detection phase ends, the ODP is
    received (at least four DC1s of alternating parity), or the protocol phase is seen to
    start (continuous flags or an LAPM frame). On seeing the ODP it sends an ADP at least
    ten times.
  - [derived] "Bypassed" therefore means:
    - the originator does not send ODP and starts LAPM establishment at once;
    - the answerer does not wait for ODP (no T400 detection wait) and must take flags or an
      LAPM frame as the start of the protocol phase.
- **Pitfall P10.** Given the V.8 warning and a far end already seen live (it sends ADP
  "ECEC" late; see project memory), the receiver **should** still cope with an ODP/ADP that
  arrives even though it was bypassed.

---

## 6. Clause 9.4: short Phase 2 (ranging only)

### 6.1 Timeline (Figure 9/V.92, read from the render)

```
                                          |<- 40 ms ->|<- 10 ms ->|
Analogue: [INFO0a][ A ............................... ][    Ā    ][ INFO1a ]   (silence)   [Phase 3 ...
                                          ▲ B̄ arrives  ▼ Ā leaves
Digital : [INFO0d][ B  >50 ms ][ B̄ 10 ms ]  silence ...................... receive INFO1a → Phase 3
                              ▲ B̄ leaves
```

- The upward arrow in the figure runs from the start of B̄ at the digital modem to the
  analogue modem.
- The downward arrow runs from the start of Ā at the analogue modem back to the digital
  modem.
- The figure labels are "40 ms" and "10 ms" on the analogue side, and ">50 ms" and "10 ms"
  under B/B̄ on the digital side. The text gives "at least 50 ms" and 40 ± 1 ms.
- The analogue row shows INFO1a following Ā directly, then a gap (the 70 ± 5 ms silence of
  9.5.2.1.1), then the start of Phase 3.

**Compared with V.90's full Phase 2, the roles are reversed.** In full Phase 2 the
analogue modem sends the first reversal and the digital modem answers after 40 ms. In
short Phase 2 the **digital modem sends the first reversal** and the **analogue modem
answers after 40 ± 1 ms**. There is only one round:

- no second reversal pair;
- no L1/L2 in either direction;
- no INFO1d;
- no RTDEa is defined, only RTDEd.

### 6.2 Digital modem, error-free (9.4.1.1)

- **SD-1 (9.4.1.1.1, SHALL).**
  - During the 75 ± 5 ms silence that ends Phase 1, listen for INFO0a and Tone A.
  - After the silence, send **INFO0d with bit 28 = 0**, then **Tone B**.
  - This is the same as FD-1.
- **SD-2 (9.4.1.1.2, SHALL).** Once INFO0a is received:
  - listen for **Tone A**;
  - keep receiving INFO0a, since repeats mean recovery (SDR-1).
  - [derived] At this point the four capability bits decide between short and full Phase 2.
- **SD-3 (9.4.1.1.3, SHALL).** When Tone A has been **detected** and Tone B has been sent
  for **at least 50 ms**:
  - send a **Tone B phase reversal**;
  - keep sending B̄ for **another 10 ms**;
  - then go **silent**;
  - listen for a **Tone A phase reversal**.
  - Start the 2500 ms timers of SDR-2 and SDR-3 at the moment the B reversal leaves.
- **SD-4 (9.4.1.1.4, SHALL).** On the Tone A reversal:
  - Compute **RTDEd = t_rx(A reversal) − t_tx(B reversal) − 40 ms**, both times at the line
    terminals.
  - The digital modem then stays silent (it already is) and receives **INFO1a**.
- **SD-5 (9.4.1.1.5, SHALL).** When INFO1a arrives, go to the Phase 3 that INFO1a asks for:
  - Table 18 → V.92 9.5.1 (PCM upstream);
  - Table 19 → the V.90-mode Phase 3 [derived; see ambiguity A6].

### 6.3 Digital modem, recovery (9.4.1.2)

- **SDR-1 (9.4.1.2.1, SHALL).** Applies in SD-2 or SD-3 when either Tone A is detected
  before INFO0a has been received **correctly**, or INFO0a keeps repeating.
  - Send INFO0d **repeatedly**.
  - Set **bit 28 = 1** once INFO0a has been received correctly.
  - If an INFO0a arrives with **bit 28 = 1**:
    - listen for Tone A "and a subsequent Tone A phase reversal" (see ambiguity A7);
    - **finish the INFO0d in progress**;
    - then send Tone B.
  - Alternatively, if Tone A is detected **and** INFO0a has been received correctly:
    - listen for the Tone A reversal;
    - finish the INFO0d in progress;
    - then send Tone B.
  - In both cases, continue at **SD-3**. SD-3 still requires ≥ 50 ms of Tone B before the
    B reversal.
- **SDR-2 (9.4.1.2.2, SHALL).** If no Tone A reversal is detected in SD-4 within **2500 ms**
  of **sending the B reversal** in SD-3:
  1. Listen for **Tone A**.
  2. When Tone A is detected, send **Tone B** and listen for a Tone A reversal.
  3. Continue with **full Phase 2**. [V.92 map] That is FD-3 onwards, the same re-entry
     point as a V.90 retrain.
  - Unlike 9.7.1.2, this clause asks for **no 70 ms silence** and no circuit 106/104
    handling.
- **SDR-3 (9.4.1.2.3, SHALL).** If no INFO1a is received in SD-5 within **2500 ms** of
  **sending the B reversal** in SD-3:
  1. Send **Tone B** and listen for **Tone A**.
  2. When Tone A is detected, listen for the Tone A reversal.
  3. Continue with **full Phase 2** (FD-3 onwards).
  - [derived] The analogue modem, which by now is in Phase 3, hears Tone B, responds to a
    retrain (9.5.2.2 leading to 9.7.2.2), and sends Tone A. The two recoveries therefore fit
    together.

### 6.4 Analogue modem, error-free (9.4.2.1)

- **SA-1 (9.4.2.1.1, SHALL).**
  - During the 75 ± 5 ms silence, listen for INFO0d and Tone B.
  - After the silence, send **INFO0a with bit 28 = 0**, then **Tone A**.
  - This is the same as FA-1.
- **SA-2 (9.4.2.1.2, SHALL).** Once INFO0d is received:
  - listen for **Tone B**;
  - keep receiving INFO0d (see SAR-1);
  - watch for the **Tone B phase reversal** that follows.
  - The analogue modem does **not** reverse Tone A on its own initiative here, unlike FA-3.
- **SA-3 (9.4.2.1.3, SHALL).** On the Tone B reversal, send a **Tone A phase reversal**:
  - delayed so that **40 ± 1 ms** passes from the B reversal arriving at the line to the A
    reversal leaving at the line;
  - followed by **10 ms** of Ā.
- **SA-4 (9.4.2.1.4, SHALL).** Then send **INFO1a**, laid out as Table 18 or Table 19, and
  go to the Phase 3 it selects.
  - Figure 9 shows INFO1a following the 10 ms of Ā directly, with no gap.
  - V.92 9.5.2.1.1 then requires **70 ± 5 ms of silence** before Ru, for the V.92 Phase 3.

### 6.5 Analogue modem, recovery (9.4.2.2)

- **SAR-1 (9.4.2.2.1, SHALL).** Applies in SA-2 or SA-3 when either Tone B is detected
  before INFO0d has been received correctly, or INFO0d keeps repeating.
  - Send INFO0a **repeatedly**.
  - Set **bit 28 = 1** once INFO0d has been received correctly.
  - If an INFO0d arrives with bit 28 = 1:
    - listen for Tone B;
    - **finish the INFO0a in progress**;
    - then send Tone A.
  - Alternatively, if Tone B is detected **and** INFO0d has been received correctly:
    - finish the INFO0a in progress;
    - then send Tone A.
  - In both cases, continue at **SA-3**, which means waiting for the B reversal.
- **SAR-2 (9.4.2.2.2, SHALL).** If no Tone B reversal is detected in SA-3 within
  **2500 ms** of the **end of INFO0a transmission**, **start a retrain per 9.7.2.1**
  (section 7).
  - [derived] With repeated INFO0a, "end of INFO0a" should be read as the end of the last
    INFO0a sent. See ambiguity A9.

### 6.6 Derived timing budget for short Phase 2 [derived]

Definitions:

- t0 is when the digital modem's B reversal leaves its terminals.
- RTD = d_da + d_ad, the sum of the two one-way delays.

Events:

| Event | Time |
|---|---|
| B reversal arrives at the analogue modem | t0 + d_da |
| Analogue A reversal leaves | t0 + d_da + 40 ms |
| A reversal arrives at the digital modem | t0 + RTD + 40 ms, so RTDEd = RTD |
| INFO1a starts at the analogue modem | t0 + d_da + 50 ms (then 1 leading point) |
| Last INFO1a bit arrives at the digital modem | t0 + RTD + 50 + 1.67 + 116.67 ≈ **t0 + RTD + 168.3 ms** |

Limits:

- **SDR-2** fails when RTD + 40 ms + (the A-reversal detector's latency) ≥ 2500 ms. That
  caps RTD at about **2.45 s**.
- **SDR-3** fails when RTD + 168.3 ms + (INFO decode latency) ≥ 2500 ms. That caps RTD at
  about **2.33 s**.
- **SAR-2** waits from the end of INFO0a. It must cover RTD, plus the digital modem's
  Tone A detection time, plus up to 50 ms of B (if the digital's B started late), plus the
  analogue's reversal detection time. That caps RTD at about 2.3 to 2.4 s.

These are **fixed** timers, not RTD-relative like V.90's. On this project's VoIP path
(RTD measured at about 1.50 to 1.60 s) the margin is only about 0.7 to 0.9 s, before
detector latency and the roughly 20 ms concealment slips.

---

## 7. Retrains referenced by 9.3, 9.4.1.2 and 9.4.2.2 (V.92 9.7)

- **9.7.1.1, digital modem starts a retrain (SHALL).**
  1. Turn circuit 106 **OFF** and clamp circuit 104 to binary **1**.
  2. Send silence for **70 ± 5 ms**.
  3. Send **Tone B** and listen for Tone A.
  4. When Tone A is detected, listen for an A reversal and continue with **full Phase 2**.
     The V.90 equivalent (9.5.1.1/V.90) says "9.2.1.1.3", i.e. FD-3.
- **9.7.1.2, digital modem responds to a retrain (SHALL).**
  1. After Tone A has been detected for **more than 50 ms**: circuit 106 OFF, clamp 104 to
     1, silence for **70 ± 5 ms**.
  2. Send Tone B, listen for an A reversal, continue with full Phase 2 (FD-3).
- **9.7.2.1, analogue modem starts a retrain (SHALL).**
  1. Circuit 106 OFF, clamp 104 to 1, silence for **70 ± 5 ms**.
  2. Send **Tone A** and listen for Tone B.
  3. Once Tone B has been detected **and** Tone A has been sent for **≥ 50 ms**, send an A
     reversal.
  4. Listen for a B reversal and continue with full Phase 2. V.90 9.5.2.1 says
     "9.2.2.1.4", i.e. FA-4.
- **9.7.2.2, analogue modem responds to a retrain (SHALL).**
  1. After Tone B has been detected for **more than 50 ms**: circuit 106 OFF, clamp 104 to
     1, silence for **70 ± 5 ms**.
  2. Send Tone A and continue with full Phase 2. V.90 9.5.2.2 says "9.2.2.1.3", i.e. FA-3:
     reverse A after B is detected and A has been sent for ≥ 50 ms.
- A retrain never repeats INFO0. The modem keeps using the far-end capabilities from the
  first INFO0 exchange. [derived; this matches the existing `phase2::Modem::retrain` and
  `v90_retrain`]
- Phase 3 and Phase 4 timers that start at "the end of INFO1a" also apply after short
  Phase 2. See ambiguity A5.
  - V.92 9.5.1.2.1: the digital modem retrains if Ja is not detected within **4500 ms +
    RTD** of the end of INFO1a.
  - V.92 9.6.1.2.1 and 9.6.2.2.1: either modem retrains if **B1** does not arrive within
    **20 s + 6 × RTD** of the end of INFO1a.

---

## 8. Timers and tolerances

Times are at the line terminals unless noted.

| # | Value | Who | Start → stop | Clause |
|---|---|---|---|---|
| T1 | 75 ± 5 ms silence | both | end of Phase 1 → first INFO0 | V.90 9.1; V.92 9.2.x, 9.4.x.1.1 |
| T2 | 600 bit/s ± 0.01 % | both | INFO DPSK rate | V.90 8.2.3.1 |
| T3 | 2400 Hz ± 0.01 % (−1 dB), 1800 Hz ± 0.01 % (−7 dB) | analogue | INFO carrier and guard tone | V.90 8.2.3.1 |
| T4 | 1200 Hz ± 0.01 % (nominal) | digital | INFO carrier | V.90 8.2.3.1 |
| T5 | ≥ 50 ms of own tone | analogue (full) / digital (short) | tone start → first reversal (the far tone must also be detected) | V.90 9.2.2.1.3; V.92 9.4.1.1.3 |
| T6 | 40 ± 1 ms | responder | far reversal arrives → own reversal leaves | V.90 9.2.1.1.3, 9.2.1.1.6, 9.2.2.1.5; V.92 9.4.2.1.3 |
| T7 | 10 ms | sender of a reversal | tone continues after its reversal | V.90 9.2.x; V.92 9.4.1.1.3, 9.4.2.1.3 |
| T8 | RTDE = interval − 40 ms | initiator | own reversal leaves → answering reversal arrives | V.90 9.2.1.1.4, 9.2.2.1.4; V.92 9.4.1.1.4 |
| T9 | L1: 160 ms at +6 dB | both (full) | | V.34 10.1.2.4 |
| T10 | L2: ≤ 550 ms + RTD at nominal | both (full) | | V.34 10.1.2.4 |
| T11 | read L2 ≤ 500 ms (MAY) | receiver (full) | after the 160 ms of L1 | V.90 9.2.1.1.5, 9.2.2.1.8 |
| T12 | L2 echo ≤ 550 ms + RTD, then send INFO1d or A | L2 sender (full) | | V.90 9.2.1.1.7, 9.2.2.1.6 |
| T13 | 50 ms of A, reversal, then 10 ms | analogue (full) | after its L2 | V.90 9.2.2.1.6 |
| T14 | ≤ 670 ms silence (figure only) | both (full) | | Figure 4/V.90 |
| T15 | 2000 ms | digital (full) | reversal detected in FD-3 → second A reversal | V.90 9.2.1.2.3 |
| T16 | 900 ms + RTD, then wait 40 ms | digital (full) | reversal detected in FD-4 → A reversal | V.90 9.2.1.2.4 |
| T17 | 650 ms + RTD | digital (full) | start of own L2 → Tone A | V.90 9.2.1.2.5 |
| T18 | 700 ms + RTD | digital (full) | end of INFO1d → INFO1a | V.90 9.2.1.2.6 |
| T19 | 2000 ms (start not stated) | analogue (full) | → B reversal of FA-4 | V.90 9.2.2.2.2 |
| T20 | 600 ms + RTD | analogue (full) | start of own L2 → Tone B | V.90 9.2.2.2.3 |
| T21 | 2000 ms + 2 × RTD | analogue (full) | Tone B detected in FA-6 → INFO1d | V.90 9.2.2.2.4 |
| **T22** | **2500 ms** | digital (short) | **own B reversal leaves → A reversal detected** | **V.92 9.4.1.2.2** |
| **T23** | **2500 ms** | digital (short) | **own B reversal leaves → INFO1a received** | **V.92 9.4.1.2.3** |
| **T24** | **2500 ms** | analogue (short) | **end of INFO0a sent → B reversal detected** | **V.92 9.4.2.2.2** |
| T25 | 70 ± 5 ms silence | analogue | end of INFO1a → Phase 3 | Figure 4/V.90; V.92 9.5.2.1.1 |
| T26 | 70 ± 5 ms silence, then tone | either | retrain start or response | V.92 9.7 |
| T27 | far tone held > 50 ms | either | before responding to a retrain | V.92 9.7.1.2, 9.7.2.2 |
| T28 | frequency offset accurate to 0.25 Hz, else −512 | INFO1 senders | | Tables 17 and 19 |

---

## 9. Every shall, should and may in 9.3 and 9.4

| Clause | Kind | Obligation |
|---|---|---|
| 9.3 | shall | Full Phase 2 procedures and recovery follow V.90 Phase 2 exactly |
| 9.3 | shall | Both V.92 → the digital modem uses Table 17 for INFO1d |
| 9.3 | may | Both V.92 → the analogue modem may pick PCM upstream with Table 18 |
| 9.3 | shall | Either not V.92 → both use the V.90 8.2.3.2 bit definitions |
| 9.3 | shall | Both V.92 → later retrains use V.92 Phase 2 |
| 9.3.1 | shall | Both V.92 and both LAPM in V.8 or V.8 bis → bypass ODP/ADP |
| 9.4 | shall | Both V.92 and both request short Phase 2 → follow 9.4 |
| 9.4 | shall (restriction) | The analogue modem requests short Phase 2 only if it intends PCM upstream or V.90 data mode |
| 9.4.1.1.1 | shall | Listen for INFO0a and Tone A during the silence; then INFO0d (b28 = 0), then Tone B |
| 9.4.1.1.2 | shall | After INFO0a: listen for Tone A and keep receiving INFO0a |
| 9.4.1.1.3 | shall | After Tone A is detected and B has run ≥ 50 ms: B reversal, 10 ms of B̄, silence, listen for the A reversal |
| 9.4.1.1.4 | (definition) + shall | Compute RTDEd; stay silent and receive INFO1a |
| 9.4.1.1.5 | shall | Go to the Phase 3 that INFO1a selects |
| 9.4.1.2.1 | shall (×several) | Repeat INFO0d; b28 = 1 after good INFO0a; the two "then Tone B" branches; continue at 9.4.1.1.3 |
| 9.4.1.2.2 | shall | 2500 ms with no A reversal → listen for Tone A → Tone B → full Phase 2 |
| 9.4.1.2.3 | shall | 2500 ms with no INFO1a → Tone B, listen for Tone A → full Phase 2 |
| 9.4.2.1.1 | shall | Listen for INFO0d and Tone B during the silence; then INFO0a (b28 = 0), then Tone A |
| 9.4.2.1.2 | shall | After INFO0d: listen for Tone B, keep receiving INFO0d, detect the B reversal |
| 9.4.2.1.3 | shall | A reversal 40 ± 1 ms after the B reversal arrives; 10 ms of Ā |
| 9.4.2.1.4 | shall | Send INFO1a and go to the Phase 3 it selects |
| 9.4.2.2.1 | shall (×several) | Repeat INFO0a; b28 = 1 after good INFO0d; the two "then Tone A" branches; continue at 9.4.2.1.3 |
| 9.4.2.2.2 | shall | 2500 ms after the end of INFO0a with no B reversal → retrain per 9.7.2.1 |
| Table 17 | shall | Bits 12:14 and 15:17 are 0 if the analogue transmitter cannot reduce power; 3000/3200/3429 results consistent with INFO0a; offset accurate to 0.25 Hz or −512; NOTE 1 limit on values > 12 |
| Table 18 | shall / shall not | Not used if INFO1d b70 = 0; U_INFO power ≤ digital maximum; U_INFO > 66 |
| Table 19 | shall | U_INFO power ≤ digital maximum, U_INFO > 66; offset accuracy or −512 |
| Tables 15 and 16 NOTE 2 | may | Bit 24 may be combined with the V.8 PSTN access category |
| V.34 10.1.2.4 | should | Probing tones accurate enough |
| V.34 10.1.2.1 and 10.1.2.2 notes | should | Tone bandwidth must not hurt RTD accuracy |
| V.90 9.2.1.1.5, 9.2.2.1.8 | may | Read L2 for ≤ 500 ms |
| V.90 9.2.1.2.6, 9.2.2.2.4 | either/or | Retrain, or resend INFO1d / send INFOMARKSa |

---

## 10. Differences from V.34 clause 11.2 and V.90 9.2

| Aspect | V.34 11.2 | V.90 9.2 | V.92 9.3 (full) | V.92 9.4 (short) |
|---|---|---|---|---|
| Roles | call/answer by who dialled | digital = "call" side (1200 Hz, Tone B); analogue = "answer" side (2400 Hz, Tone A), whoever dialled | as V.90 | as V.90 |
| INFO0 | Table 14/V.34, 49 bits; b26:27 = transmit clock | INFO0d 62 bits (Table 7), INFO0a 49 bits (Table 8); b26:27 reserved | Tables 15 and 16: **V.92 flag and short-Phase-2 request, at swapped positions** | same as full |
| First reversal | answerer (Tone A) | analogue (Tone A) | analogue | **digital (Tone B)** |
| Answer reversal | call modem at 40 ± 1 ms | digital at 40 ± 1 ms | same | **analogue at 40 ± 1 ms** |
| Ranging rounds | 2 (RTDEc, RTDEa) | 2 (RTDEd, RTDEa) | 2 | **1 (RTDEd only)** |
| Probing (L1/L2) | both directions | both directions | both directions | **none** |
| INFO1 from the "call" side | INFO1c | INFO1d = INFO1c layout | **Table 17** (b70 = PCM-upstream flag; 3429 field 8 bits) | **not sent** |
| INFO1a | Table 16/V.34 | Table 10 (V.90) or Table 11 (V.34) | adds **Table 18** (PCM upstream) | **Table 18 or Table 19** only |
| INFO1a MD units | 35 ms | 35 ms | 35 ms; **276 symbols (34.5 ms) in Table 18** | 34.5 ms (Table 18) or 35 ms (Table 19) |
| Recovery timers | RTD-relative (2000; 900+RTD; 650+RTD; 700+RTD; 600+RTD; 2000+2RTD) | same as V.34 | same as V.90 | **fixed 2500 ms ×3** |
| Fallback on failure | retrain (11.5) | retrain (9.5) | retrain (9.7) | the digital modem **drops into full Phase 2**; the analogue modem **retrains (9.7.2.1)**, which re-enters full Phase 2 |
| Power on return to Phase 2 | keeps the negotiated level if past L1/L2 | always back to nominal | always back to nominal | always back to nominal |
| Later retrains | V.34 Phase 2 | V.90 Phase 2, even in V.34 mode | V.92 Phase 2 (if both V.92) | V.92 **full** Phase 2 |
| Analogue INFOMARKSa on missing INFO1 | Tone B → respond to retrain (11.5.2.2) | Tone B → 9.2.2.1.3 | as V.90 | not applicable |
| ODP/ADP | as V.42 | as V.42 | bypassed if both V.92 and both LAPM (9.3.1) | same |

---

## 11. Implementation notes

### 11.1 What is new compared with the current V.90 code

1. **New INFO0 fields.**
   - INFO0d gets b26 (request short) and b27 (V.92).
   - INFO0a gets b26 (V.92) and b27 (request short). **The positions are swapped.**
2. **INFO1d bit 70** becomes "channel supports PCM upstream". The 3429 result shrinks to 8
   bits.
3. **Two new INFO1a layouts:** Table 18 (PCM upstream, with both rate fields = 6 and bits
   40:49 all ones) and Table 19 (short-Phase-2 V.90 mode, with a carrier flag in b33).
4. **A new procedure: short Phase 2.**
   - The roles in the ranging exchange are reversed.
   - It has three fixed 2500 ms timers.
   - The digital modem can fall back to full Phase 2 without a retrain.
5. **ODP/ADP bypass** once V.92 and LAPM are both confirmed.
6. **Full Phase 2 as a procedure is unchanged.** Only the Phase 3 dispatch after INFO1a
   gains the Table 18 case (V.92 9.5 Phase 3).

### 11.2 Pitfalls, with the affected code

- **P1: INFO0 bits 26:27 are decoded wrongly by the current code.**
  - `v34/info.rs::Info0d::to_bits` forces info[14] and info[15], which are **bits 26 and
    27**, to 0.
  - `Info0d::from_bits` discards them (`clock: 0`).
  - `Info0::from_bits`, used for INFO0a, reads bits 26:27 into `clock`, the V.34 transmit
    clock source.
  - A V.92 INFO0a with b26 = 1 would therefore parse as `clock = 1`, "synchronized to
    receive timing".
  - V.92 needs explicit `v92` and `short_phase2` fields, at the swapped positions for each
    direction.
  - Only a PCM (V.90/V.92) Phase 2 should read them this way. A plain V.34 INFO0c/INFO0a
    still means clock source.
- **P2: INFO1d bit 70.**
  - `Info1c::from_bits` splits bits 25..78 into six 9-bit `Probed` records, so
    `probed[5].high_carrier` is bit 70.
  - For V.92 that bit must be exposed as `pcm_upstream_ok`, and the 3429 record read as
    pre-emphasis 71:74 plus rate 75:78.
  - The V.92 digital modem must set bit 70 on purpose. The existing code would send
    whatever the 3429 carrier flag held.
- **P3: The existing INFO1a decoder rejects Table 18.**
  - `Info1aPcm::from_bits` requires bits 34:36 in 3..5, so a Table 18 frame (34:36 = 6)
    comes back `None`.
  - It would also have read bits 40:49 (all ones) as a −0.02 Hz offset.
  - It ignores bit 33, which carries the carrier flag in Table 19.
  - Add a `Info1aPcmUpstream` type (Table 18) and a short-Phase-2 flavour of `Info1aPcm`
    (Table 19, b33).
  - Dispatch on (34:36, 37:39) as in section 3.8.
- **P4: MD length units.** Table 18 counts in **276 symbols at 8000 symbols/s (34.5 ms)**.
  Tables 10, 11 and 19 and INFO1d count in **35 ms**. Do not share the conversion.
- **P5: The 40 ± 1 ms turnaround is measured terminal to terminal.**
  - The responder must subtract its reversal detector's latency and add its transmit
    filter's group delay. This is the same compensation the existing `v34/phase2.rs`
    `TURN` logic already does.
  - In short Phase 2 the responder is the **analogue** (answer-role) side, which the
    existing code never had to do.
  - A 1 ms error in the responder turns straight into a 1 ms RTDEd error at the digital
    modem.
- **P6: Tails of only 10 ms.**
  - After the B reversal the digital modem sends only 10 ms of B̄ and then goes silent. The
    analogue detector must decide on "reversal" from about 12 cycles of 1200 Hz and must
    not read tone **loss** as a reversal.
  - Likewise, Ā lasts only 10 ms before INFO1a starts.
  - INFO1a opens with a leading point at an arbitrary phase and then fill bits `1111`, each
    of which is a 180° reversal. The digital modem must time-stamp the **first** reversal,
    the Ā edge, and not one of the INFO1a reversals.
  - [suggestion] The analogue transmitter can choose INFO1a's leading point to carry on the
    Ā phase, so that no extra phase step appears before the fill bits.
- **P7: Tone onset is not a reversal.** After an SAR-2 retrain (70 ms silence, then Tone
  A), the digital modem may still be waiting for the SD-4 reversal. The start of Tone A
  after silence must not register as that reversal.
- **P8: Short Phase 2 has no RTDEa.**
  - The analogue modem never learns the round-trip delay, yet V.92 Phase 3 and Phase 4
    timers use it (9.5.2.1.2: MD+TRN1u ≤ RTD + 4000 ms; 9.6.2.2.1: 20 s + 6 RTD).
  - See ambiguity A5 for the options.
- **P9: L2 read budget (full Phase 2), already hit live on V.34.**
  - The far end expects our tone within **600 ms + RTD** of the start of its own L2
    (FAR-3, and V.34 11.2.2.2.3).
  - Reading 160 ms of L1 plus the full 500 ms of L2 leaves only about 100 ms minus
    detector latency.
  - Keep the shorter L2 read the project already adopted (see memory
    "v34-phase2-tone-deadline").
  - The same rule, 650 ms + RTD (FDR-5), protects the digital side.
- **P10: ODP/ADP bypass in the real world.**
  - V.8 7.3 itself warns that some equipment shows LAPM and still needs ODP/ADP.
  - Recommendation: when bypassing, start LAPM establishment at once, but also tolerate or
    answer an ODP from the far end.
  - The bypass applies only when **both** conditions hold: both V.92 (INFO0) and both LAPM
    (V.8 prot0, V.8 bis NPar(2) bit 2, or the short-Phase-1 P bits).
- **P11: Fixed 2500 ms timers on long paths.**
  - Short Phase 2 cannot succeed if RTD is above about 2.3 s (section 6.6).
  - The fallback is automatic, but costs a full Phase 2 and possibly a retrain.
  - [suggestion] The analogue modem **should not request** short Phase 2 when it already
    knows the path is that long, for example from a previous call or from the short
    Phase 1 timing.
- **P12: Digital data-frame alignment.** With short Phase 1, V.92 8.3.6 puts the first QTS
  symbol in data frame interval 0 and requires the digital modem to keep that alignment
  from then on. The digital modem's 8 kHz frame counter must keep running through
  Phase 2, full or short.
- **P13: No Phase 2 level changes.**
  - All Phase 2 signals are at nominal power, except L1 (+6 dB) and the analogue INFO/Tone
    A offsets.
  - Returning to Phase 2 from a later phase (SDR-2, SDR-3, 9.7) SHALL restore nominal
    power.
- **P14: The fallback does not re-send INFO0.**
  - When SDR-2 or SDR-3 falls into full Phase 2, the INFO0 exchange is already done. Re-enter
    at FD-3, "wait for the A reversal", with capabilities kept.
  - The analogue side re-enters through 9.7.2.1 or 9.7.2.2, which lead to FA-4 or FA-3.
  - This matches `phase2::Modem::v90_retrain(pcm, fs, far, far_info0d)`.
- **P15: Which INFO1a layout is legal depends on the Phase 2 that ran** (section 3.8). The
  digital modem should reject a Table 11 INFO1a (V.34 mode) after short Phase 2, and a
  Table 19 frame after full Phase 2. [derived]

### 11.3 Suggested structure [suggestion]

- **Extend `v34/phase2.rs` with a `Short` flavour** rather than writing a new module. The
  INFO0 handling and recovery (SDR-1/SAR-1) are the same code as FDR-1/FAR-1; only the step
  after the INFO0 exchange branches:
  - **Digital, short:**
    `WaitToneA` → `ToneBHold(≥50 ms)` → `SendReversal + 10 ms` → `Silent/WaitARev (T22)` →
    `WaitInfo1a (T23)` → Phase 3.
    - On T22: `WaitToneA` → `ToneB` → `WaitARev`, which is full Phase 2 FD-3.
    - On T23: `ToneB` → `WaitToneA` → `WaitARev`, which is FD-3.
  - **Analogue, short:**
    `ToneA/WaitBRev (T24)` → `Turnaround(40 ms)` → `Ā 10 ms` → `INFO1a` → `silence 70 ms`
    → Phase 3.
    - On T24: retrain per 9.7.2.1.
- **Decision point:** evaluate `short = d.v92 && a.v92 && d.short_req && a.short_req` as
  soon as the far INFO0 decodes. Latch it; it only matters until the first tone event.
- **Settings:** make "request short Phase 2" a setting on each side, off by default until
  it has been tested live, and log the decision.
- **Tests to add, all in the simulated `Line` harness:**
  - both sides request short Phase 2 → RTDEd equals the simulated RTD to ±1 ms;
  - only one side requests → full Phase 2;
  - a V.90 (non-V.92) far end → bits 26/27 are ignored and V.90 layouts are used;
  - B reversal lost → SAR-2 retrain, and full Phase 2 completes;
  - A reversal lost → SDR-2 falls into full Phase 2;
  - INFO1a corrupted → SDR-3, then the analogue modem responds from Phase 3;
  - RTD sweep 0 to 2.6 s → short Phase 2 succeeds below about 2.3 s and falls back cleanly
    above;
  - INFO0 lost once in each direction → bit-28 recovery, with the short decision still
    agreed;
  - Table 18 and Table 19 round trips through the INFO codec, using the vectors in
    section 3.4.

---

## 12. Ambiguities and open questions

- **A1: Guard tone level during Tone A.**
  - V.34 10.1.2.1, as printed, puts the 1800 Hz guard tone at nominal power during A/Ā.
  - V.34 10.1.2.3.1 and V.90 8.2.3.1 put it 7 dB below nominal during INFO.
  - The existing code uses −7 dB for both.
  - The literal reading would make Tone A plus guard about 2.5 dB above nominal, which
    looks like an editorial slip in V.34. Keep −7 dB unless a real capture says otherwise.
- **A2: Conditions on the digital modem's request (INFO0d b26).** None are stated. Also
  not stated: whether a modem may request short Phase 2 after **full** Phase 1. The bits
  allow it.
- **A3: Short Phase 2 and Table 18's "not if INFO1d b70 = 0".**
  - Short Phase 2 has no INFO1d, so the analogue modem cannot check the PCM-upstream flag,
    and gets no power-reduction advice (12:17).
  - Presumably it relies on what it learned before (V.92 scope item (j): "reduced start-up
    time on **recognized** connections"). V.92 does not say how that knowledge is kept or
    matched.
  - Open: should the analogue modem only request short Phase 2 when it holds stored
    parameters for this connection?
- **A4: Frequency-offset field in Table 19.** Short Phase 2 sends no probing tones, yet
  bits 40:49 are "the frequency offset of the probing tones". [suggestion] Send −512
  ("ignore") and ignore the field on receive.
- **A5: Round-trip delay for the analogue modem after short Phase 2.** Several analogue
  Phase 3 and Phase 4 rules use RTD. Options:
  - reuse a stored RTD;
  - estimate it from the short-Phase-1 timing (TONEq start to ANSpcm stop);
  - assume 0, the strictest choice.
  - The Recommendation is silent. Also open: whether the digital modem should use RTDEd for
    its own 9.5.1.2.1 (4500 ms + RTD) timer as usual. Presumably yes.
- **A6: Which Phase 3 follows a Table 19 INFO1a.**
  - 9.4.x says only "the appropriate Phase 3 as signalled in INFO1a".
  - V.92 9.5 as drawn is the PCM-upstream Phase 3 (Ru, TRN1u, CPt).
  - [derived] Table 19, V.90 data mode, should lead to **V.90 9.3** (S, S̄, MD, PP, TRN).
    To be confirmed by whoever digests V.92 9.5 and 8.5.
- **A7: Leftover wording in 9.4.1.2.1.** "Detect Tone A and a subsequent Tone A phase
  reversal" is copied from V.90, where the analogue modem reverses first. In short Phase 2
  no A reversal can come before the digital modem's own B reversal. [suggestion] Arm only
  the Tone A detector here, and arm the A-reversal detector at SD-3.
- **A8: Start of FAR-2's 2000 ms.** V.90 9.2.2.2.2 gives no start point. V.34 11.2.2.2.2
  gives none either. Presumably it runs from the analogue modem's own A reversal (FA-3).
- **A9: Start of SAR-2's 2500 ms when INFO0a was repeated.** "From the end of INFO0a
  transmission" is taken to mean the end of the **last** INFO0a sent.
- **A10: Upstream pre-emphasis in short-Phase-2 V.90 mode.**
  - V.90 6.4 says the digital modem supplies the filter choice during Phase 2, but short
    Phase 2 has no INFO1d.
  - Table 19 carries a carrier flag (b33) but **no pre-emphasis index** and no projected
    rate.
  - Open: which index does the analogue transmitter use? A stored value, or index 0 (flat)?
- **A11: Asymmetry and 3429 checks for Table 19.** Unlike Table 10/V.90, Table 19 omits
  "consistent with INFO1d" for bits 34:36.
  - [suggestion] The analogue modem should still honour INFO0d bit 40 (V.90 upstream 3429)
    and its own INFO0a bits 15 to 19.
  - V.90 6.2 still applies: 3200 is mandatory, 3000 and 3429 are optional, and 2400, 2743
    and 2800 are forbidden upstream.
- **A12: No 70 ms silence and no circuit handling in SDR-2 and SDR-3.** Unlike 9.7.1.x,
  these clauses start Tone B without a 70 ms silence and do not mention circuits 106/104.
  During start-up the circuits are already off or clamped, so this is probably
  intentional.
- **A13: 9.3.1 versus 9.2.5.** 9.2.5 (short Phase 1) needs only "both indicated LAPM",
  while 9.3.1 also needs V.92 capability in INFO0.
  - [derived] After short Phase 1, both are V.92 by construction, so the conditions agree.
  - After full Phase 1 the bypass depends on both conditions. V.8 prot0 on its own is not
    enough, because a V.90 far end does not know about the bypass.
- **A14: Figure 9 shows ">50 ms"; the text says "at least 50 ms".** Treat 50 ms as the
  minimum.
