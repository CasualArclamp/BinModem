# V.92 clause 8.4: full and short Phase 2 signals and sequences (implementation digest)

Source: ITU-T V.92 (11/2000), clause 8.4 and 8.4.1 (Tables 15 to 19). These are on PDF pages 23
to 28 (printed pages 16 to 21). All six pages were rendered and read. The clause starts part way
down page 23, after 8.3.6 QTS, and ends on page 28, where 8.5 (Phase 3 signals) begins. Every
table row, bit position, value and timing below was read from the rendered pages. The extracted
text in `docs/specs/text` scrambles exactly these tables (for example, it moves the INFO0a row
numbers), so do not check this file against it.

Other rendered pages this digest relies on:

| Document | PDF pages | What was read there |
|---|---|---|
| V.92 | 13 | Clause 8 preamble: the rule for bit order |
| V.92 | 12 | 6.4.2: precoder and prefilter equations (for Table 18 bits 12:17) |
| V.92 | 40, 41 | 8.8.3 CPd text and Table 30 (users of Ltot and Lmax) |
| V.92 | 50 to 53 | 9.3, 9.3.1, 9.4 (Figure 9), 9.5 heading and Figures 10 and 11 |
| V.92 | 54, 55, 59, 60 | 9.5.1.1.1, 9.5.1.2.1, 9.5.2.1.1 and 9.5.2.1.2, 9.6.x.2.1, 9.7 |
| V.90 (09/98) | 20 to 25 | 8.2 (power), 8.2.1 to 8.2.4, 8.2.3.1 to 8.2.3.3, Tables 7 to 11 |
| V.90 | 29 | 8.4.4 Sd (W = Ucode 16 + U_INFO) |
| V.90 | 38 to 40 | 9.2 and Figure 4 (full Phase 2 procedure) |
| V.90 | 42 | 9.3.2.x (V.90-mode Phase 3; the MD time limit) |
| V.90 | 44, 45 | 9.5 retrains |
| V.34 (02/98) | 10 to 12 | 5.2 (Table 1), 5.3 (Table 2), 5.4 (Figures 1 and 2, Tables 3 and 4) |
| V.34 | 32 to 38 | 10.1.2 to 10.1.2.4, Figure 13, Figure 14, Tables 14 to 17, 10.1.3.5 MD |
| V.8 (11/2000) | 13 | 6.5 and Table 7 (PSTN access category) |

Conventions in this file:

- **shall** marks a mandatory requirement, **[should]** a recommendation and **[may]** an option.
  Those words are the Recommendation's own. **[derived]** marks my own conclusion from the
  text. **[suggestion]** marks my own advice.
- Bit ranges are written `lo:hi`, as the tables write them ("LSB:MSB").
- Requirements are numbered `P2-nn` and cite their clause. Notes are numbered `N-nn`
  (section 16).
- "Nominal" means the modem's nominal transmit power. V.92 3.4 defines this as "the reference
  transmit power configured by the user".
- "RTD" means round-trip delay. T means one symbol interval.

---

## 1. What clause 8.4 says

Clause 8.4 has two sentences of text and five tables:

1. **8.4**: "All full and short Phase 2 signals and sequences are defined in ITU-T V.90."
   - Tone A, Tone B, INFO DPSK modulation, INFOMARKS, and L1/L2 probing all come from V.90 8.2
     unchanged.
   - V.90 8.2 in turn points to V.34 10.1.2 for Tone A, Tone B and L1/L2.
   - See section 2.
2. **8.4.1**: the INFO CRC is the generator of 10.1.2.3.2/V.34 (section 3.4).
3. **Tables 15 to 19** give the information bits:
   - INFO0d (Table 15), INFO0a (Table 16) and INFO1d (Table 17) are redefined.
   - Two INFO1a layouts are new: Table 18 for PCM upstream, and Table 19 for V.34-style
     upstream during short Phase 2.
   - Every table says "Bit 0 is transmitted first in time".

V.92 defines **no new INFO sequence names and no new lengths**. The lengths stay as in V.90:

| Sequence | Sent by | Bits | V.92 table | What changed from V.90 |
|---|---|---|---|---|
| INFO0d | digital modem | 62 | 15 | Bit 26 (was reserved, 0) now **requests short Phase 2**. Bit 27 (was reserved, 0) now shows **V.92 capability**. |
| INFO0a | analogue modem | 49 | 16 | Bit 26 (was reserved, 0) now shows **V.92 capability**. Bit 27 (was reserved, 0) now **requests short Phase 2**. **The order is the reverse of INFO0d's.** Bit 20 now reads "in V.34 mode or in V.90 mode". |
| INFO1d | digital modem | 109 | 17 | Bit 70 was the 3429 "high carrier" flag. It now says **whether the channel supports PCM upstream**. The 3429 field shrinks from 9 bits (70:78) to 8 bits (71:78); its bits keep their positions. |
| INFO1a (PCM upstream) | analogue modem | 70 | 18 | New layout. Bits 12:17 carry precoder/prefilter capabilities. The MD length is in 276-symbol (34.5 ms) steps. Both symbol-rate fields = 6. Bits 40:49 are all **ones**. |
| INFO1a (V.34 upstream, short Phase 2) | analogue modem | 70 | 19 | Same as V.90 Table 10, except that bit 33 (was reserved) now **selects the high carrier** upstream. |

Unchanged and still used (V.92 9.3): V.90 Table 9 (INFO1d), Table 10 (INFO1a, V.90 mode) and
Table 11 (INFO1a, V.34 mode). They apply whenever either modem is not V.92, and Table 10 or 11
may also apply in a V.92/V.92 full Phase 2 (section 10 and N-9).

---

## 2. Signals inherited unchanged (V.90 8.2, and through it V.34 10.1.2)

### 2.1 Transmit power in Phase 2 (8.2/V.90)

- **P2-01** (8.2/V.90, shall): every Phase 2 signal except L1 shall be sent at the nominal
  transmit power.
- **P2-02** (8.2/V.90, shall): when a recovery mechanism sends a modem back into Phase 2 from a
  later phase, its transmit level shall return to nominal, dropping any level negotiated
  earlier.
  - This differs from 10.1.2/V.34. There the level goes back to nominal only if the return
    point is before L1/L2; otherwise the negotiated level is kept.
  - V.90's rule applies to V.92.
- The digital modem's Phase 2 nominal power is the value it advertises in INFO0d bits 29:32,
  measured where INFO0d bit 38 says (section 5).

### 2.2 Tone A (8.2.1/V.90 = 10.1.2.1/V.34)

- Sent by the **analogue modem** in V.90 and V.92. In V.34 the answer modem sends it.
- A 2400 Hz tone. A-bar is the same tone after a 180° phase reversal. The change from A to
  A-bar, and from A-bar back to A, is a 180° reversal each time.
- While sending A or A-bar, the modem also sends an **1800 Hz guard tone with no phase
  reversals**.
- Levels as printed in 10.1.2.1/V.34: "Tone A is transmitted at 1 dB below the nominal
  transmit power while the guard tone is transmitted at the nominal transmit power". This
  differs from the INFO guard level of −7 dB (see N-14).
- NOTE **[should]**: do not band-limit a tone with reversals so much that round-trip delay
  measurement suffers noticeably.
- [derived] In DPSK terms, Tone A is the 2400 Hz INFO modulator fed with binary zeros. A
  reversal is a single one.

### 2.3 Tone B (8.2.2/V.90 = 10.1.2.2/V.34)

- Sent by the **digital modem** in V.90 and V.92. In V.34 the call modem sends it.
- A 1200 Hz tone. B to B-bar and B-bar to B are each 180° reversals.
- No guard tone.
- Level: nominal (P2-01).
- The same **[should]** note on bandwidth applies.

### 2.4 INFO modulation (8.2.3.1/V.90, word for word as 10.1.2.3.1/V.34)

- **P2-03**: every INFO sequence is sent as binary DPSK at **600 bit/s ± 0.01 %**.
  - A **1** turns the transmit point **180°** from the previous point.
  - A **0** turns it **0°**, so the point stays where it was.
- **P2-04**: each INFO sequence is preceded by **one point at an arbitrary carrier phase**. It
  is the reference for the first differential decision. When several INFO sequences are sent as
  a group (for example the repeated INFO0 of recovery), only the first one has this point.
- **P2-05**: the **analogue modem** sends INFO as follows:
  - carrier **2400 Hz ± 0.01 %** at **1 dB below nominal**;
  - plus a guard tone of **1800 Hz ± 0.01 %** at **7 dB below nominal**.
- **P2-06**: the **digital modem** sends INFO on a carrier of **1200 Hz ± 0.01 %** at
  **nominal** power, with no guard tone.
- **P2-07** (shall): the magnitude spectrum of the transmitted line signal shall stay inside
  Figure 13/V.34. The figure is two limit lines, symmetric about the carrier. Each is given as
  (offset from the carrier in Hz, dB relative to the carrier peak), with straight lines between
  the vertices:
  - **Upper limit:** (0 to ±125, +0.75), (±300, −2), (±400, −5), (±450, −9), (±550, −20).
  - **Lower limit:** (0 to ±125, −0.75), (±300, −4), (±400, −9), (±475, −20).
  - Nothing is specified below −20 dB.
  - A symmetric raised-cosine-like 600 bit/s pulse with about 50 to 75 % excess bandwidth fits
    inside. [derived]
- NOTE **[should]** (8.2.3.1/V.90): it is "highly desirable" for the transmit
  channel-separation and shaping filters to be linear-phase, because INFO reception has no
  adaptive equaliser training.
- Durations at 600 bit/s. Add 1.667 ms for the reference point when a sequence starts a group.

| Sequence | Bits | Duration of the bits |
|---|---|---|
| INFO0a | 49 | 81.667 ms |
| INFO0d | 62 | 103.333 ms |
| INFO1a (every layout) | 70 | 116.667 ms |
| INFO1d | 109 | 181.667 ms |

### 2.5 INFOMARKS (8.2.3.3/V.90)

- **INFOMARKSd**: the digital modem feeds continuous binary ones into its INFO DPSK modulator.
  The result is a 1200 Hz carrier that reverses every symbol, at nominal power.
- **INFOMARKSa**: the analogue modem does the same on 2400 Hz, with the guard tone, at the
  P2-05 levels.
- Only the full Phase 2 recovery steps use them (V.90 9.2.1.2.6 and 9.2.2.2.4; section 11.3).
  Short Phase 2 does not.

### 2.6 Line probing signals L1 and L2 (8.2.4/V.90 = 10.1.2.4/V.34)

These are used in **full** Phase 2 only. Short Phase 2 has no probing.

- L1 is periodic with a repetition rate of **150 Hz ± 0.01 %**.
  - It is a sum of cosines 150 Hz apart, from 150 Hz to 3750 Hz, **leaving out 900, 1200, 1800
    and 2400 Hz**. That makes 21 tones.
  - Table 17/V.34 gives each tone as cos(2πft + φ):
    - φ = **180°** at 300, 1650, 2250, 2700, 3000, 3150, 3300 and 3450 Hz.
    - φ = **0°** at 150, 450, 600, 750, 1050, 1350, 1500, 1950, 2100, 2550, 2850, 3600 and
      3750 Hz.
- **L1 lasts 160 ms (24 repetitions) at 6 dB above nominal.** It is the one Phase 2 signal not
  at nominal.
- **L2** is the same signal as L1, at nominal power, sent for **no longer than 550 ms plus one
  round-trip delay**.
- NOTE **[should]**: generate the probing tones accurately enough not to disturb the far
  receiver's distortion and noise measurements.
- The INFO1d and INFO1a "frequency offset" fields are measured on the **1050 Hz** probing tone.

---

## 3. Common INFO frame structure, bit order and CRC

### 3.1 The bit-order rule (V.92 clause 8 preamble, PDF page 13)

- In V.92 Tables 2 to 5, 11 to 24, 27 and 30 to 33 (Tables 15 to 19 fall in this range),
  unless stated otherwise:
  - values given as **bit patterns** are sent **leftmost bit first in time**;
  - values given as **integers** are sent **least significant bit first in time**.
- Each INFO table also says "Bit 0 is transmitted first in time". In every multi-bit integer
  field, the lower-numbered bit is the LSB.

### 3.2 Frame layout (the same for every INFO sequence)

```
bit:   0..3        4..11                12 .. k-1           k .. k+15        k+16 .. k+19
      [1 1 1 1]   [0 1 1 1 0 0 1 0]    [information]       [CRC, LSB first]  [1 1 1 1]
       fill        frame sync           (per table)          16 bits           fill
```

- Fill bits 0:3 = the bit pattern `1111`.
- Frame sync 4:11 = the bit pattern `01110010`, **left-most bit first in time**:

  | Bit | 4 | 5 | 6 | 7 | 8 | 9 | 10 | 11 |
  |---|---|---|---|---|---|---|---|---|
  | Value | 0 | 1 | 1 | 1 | 0 | 0 | 1 | 0 |

  Read as an LSB-first integer this is 0x4E, but treat it as the fixed time pattern above.
- The information starts at bit 12 and ends just before the CRC:

  | Sequence | Information bits | Count | CRC bits | Trailing fill |
  |---|---|---|---|---|
  | INFO0d | 12:41 | 30 | 42:57 | 58:61 |
  | INFO0a | 12:28 | 17 | 29:44 | 45:48 |
  | INFO1d | 12:88 | 77 | 89:104 | 105:108 |
  | INFO1a | 12:49 | 38 | 50:65 | 66:69 |

- The 16 CRC bits are sent **least significant bit first**. Tables 15 to 19 just say "CRC"; the
  bit order comes from 10.1.2.3.2/V.34 ("Bit 0 of the CRC is the LSB", output starting with
  bit 0).

### 3.3 Reserved bits

- A bit marked "Reserved for the ITU" shall be sent with the stated value. The receiving modem
  does not interpret it.
- The value is 0 everywhere **except Table 18 bits 40:49, which are sent as 1**. The table's
  NOTE says this is "to avoid generating a tone".
  - [derived] A long run of zeros in DPSK is an unmodulated carrier, which a receiver could take
    for Tone A.

### 3.4 CRC (8.4.1/V.92 → 10.1.2.3.2/V.34, Figure 14/V.34)

- **P2-08**: the CRC covers every information bit of the sequence. The frame sync bits, start
  bits (INFO sequences have none) and fill bits are left out.
  - [derived] So it covers bits 12 through the bit just before the CRC, in transmission order.
- Polynomial: **x^16 + x^12 + x^5 + 1**.
- **P2-09** procedure (10.1.2.3.2/V.34):
  1. Load the shift register with **all ones**.
  2. Shift in the information bits.
  3. Output the register "starting with bit 0". **Bit 0 of the CRC is its LSB.** No final
     inversion is described.
- Figure 14/V.34 as drawn:
  - Cells run from 15 (left) to 0 (right), and data moves right.
  - The incoming information bit is XORed with the output of cell 0. That sum is the
    feedback f.
  - f feeds cell 15, the adder between cells 11 and 10, and the adder between cells 4 and 3.
  - Equivalent code:
    ```
    reg = 0xFFFF
    for bit in info_bits:            # bit 12 first
        f   = (reg & 1) ^ bit
        reg = reg >> 1
        if f: reg ^= 0x8408          # (1<<15) | (1<<10) | (1<<3)
    crc = reg                        # send crc bit 0 first ... bit 15 last
    ```
- In catalogue terms: reflected 0x1021, init 0xFFFF, no output XOR ("CRC-16/MCRF4XX"). The code
  above gives the check value **0x6F91** for ASCII "123456789" fed LSB-first per byte. I
  re-derived this in this session.
- Receiver check [derived]: run the same register over the information bits and then the 16
  received CRC bits. The result is **0x0000**. This was checked on random frames.
- The repo already implements exactly this: `crates/datapump/src/v34/info.rs`, `crc()`.
- The MH sequences (8.9.2/V.92) and CPd/CPt/CPu (8.5.1, 8.7.3, 8.8.3) point to the same CRC
  generator. MH also uses the INFO modulation and framing.

---

## 4. Carriers, symbol rates and pre-emphasis (V.34 5.2 to 5.4, for decoding INFO1)

Symbol rate S = (a/c) × 2400 ± 0.01 % (Table 1/V.34). Carrier = (d/e) × S (Table 2/V.34).

| Rate index (INFO1a 34:36 / 37:39) | S (symbols/s) | a/c | Low carrier d/e → Hz | High carrier d/e → Hz |
|---|---|---|---|---|
| 0 | 2400 | 1/1 | 2/3 → 1600 | 3/4 → 1800 |
| 1 | 2743 | 8/7 | 3/5 → 1646 (1645.7) | 2/3 → 1829 (1828.6) |
| 2 | 2800 | 7/6 | 3/5 → 1680 | 2/3 → 1867 (1866.7) |
| 3 | 3000 | 5/4 | 3/5 → 1800 | 2/3 → 2000 |
| 4 | 3200 | 4/3 | 4/7 → 1829 (1828.6) | 3/5 → 1920 |
| 5 | 3429 | 10/7 | 4/7 → 1959 (1959.2) | 4/7 → 1959 (1959.2) |
| 6 | 8000 (PCM; V.90 and V.92 only) | n/a | n/a | n/a |

- At 3429 both carriers are the same frequency (d/e = 4/7). So the old 3429 "high carrier" bit
  (INFO1d bit 70) never carried information, which is why V.92 could reuse it.
- In V.90 mode the upstream rate may only be 3000, 3200 or 3429 (indices 3 to 5, per Tables
  10/V.90 and 19/V.92).

Pre-emphasis filter index (4-bit fields, 0 to 10; 5.4/V.34):

- Indices **0 to 5** (Table 3, Figure 1): a straight-line tilt in dB, from 0 dB at f/S = 0 to
  **α** at f/S = 1.0. α = 0, 2, 4, 6, 8, 10 dB for indices 0 to 5.
- Indices **6 to 10** (Table 4, Figure 2):
  - 0 dB for f/S from 0 to 0.4;
  - a straight line from **β** at f/S = 0.8 to **β + γ** at f/S = 1.2;
  - nothing specified between 0.4 and 0.8.
  - (β, γ) = (0.5, 1.0), (1.0, 2.0), (1.5, 3.0), (2.0, 4.0), (2.5, 5.0) dB for indices 6
    to 10.
- Tolerance ±1 dB. The spectrum is checked for f/S from (d/e − 0.45) to (d/e + 0.45), into a
  600 Ω resistive load.

Projected maximum data rate (4-bit fields): n × 2400 bit/s, with n from 0 to 14. **n = 0 means
the symbol rate cannot be used.**

---

## 5. INFO0d: Table 15/V.92 (digital modem capabilities), 62 bits

The digital modem sends this on 1200 Hz at nominal power. V.92 uses this layout in every
Phase 2 that sends INFO0 (9.3, 9.4).

| Bits | Width | Meaning (V.92) | Values |
|---|---|---|---|
| 0:3 | 4 | Fill | `1111` |
| 4:11 | 8 | Frame sync | `01110010`, left-most bit first |
| 12 | 1 | Symbol rate 2743 supported in V.34 mode | 1 = yes |
| 13 | 1 | Symbol rate 2800 supported in V.34 mode | 1 = yes |
| 14 | 1 | Symbol rate 3429 supported in V.34 mode | 1 = yes |
| 15 | 1 | Can transmit on the **low** carrier at 3000 | 1 = yes |
| 16 | 1 | Can transmit on the **high** carrier at 3000 | 1 = yes |
| 17 | 1 | Can transmit on the **low** carrier at 3200 | 1 = yes |
| 18 | 1 | Can transmit on the **high** carrier at 3200 | 1 = yes |
| 19 | 1 | Transmission at 3429 | **0 = disallowed** (so 1 = allowed) |
| 20 | 1 | Can reduce transmit power below nominal **in V.34 mode** | 1 = yes |
| 21:23 | 3 | Largest allowed difference between transmit and receive symbol rates in V.34 mode, counted in rate steps (rates in increasing order, 0 = 2400 ... 5 = 3429) | integer 0 to 5 |
| 24 | 1 | This INFO0d comes from a CME modem | 1 = CME |
| 25 | 1 | Supports signal constellations of up to 1664 points | 1 = yes |
| **26** | 1 | **"Set to 1 requests short Phase 2 to be used"** (new) | 1 = request |
| **27** | 1 | **"V.92 capability: 1"** (new) | a V.92 digital modem sends **1** |
| 28 | 1 | Acknowledges correct reception of an INFO0a during error recovery | 1 = acknowledged |
| 29:32 | 4 | Digital modem nominal transmit power for Phase 2, in −1 dBm0 steps | n → **−(6 + n) dBm0**: 0 = −6, 15 = −21 |
| 33:37 | 5 | Maximum digital modem transmit power, in −0.5 dBm0 steps | n → **−0.5 × (n + 1) dBm0**: 0 = −0.5, 31 = −16 |
| 38 | 1 | Where the digital modem's power "shall be measured" | 1 = at the output of the codec; 0 = at the modem's terminals |
| 39 | 1 | PCM coding the digital modem uses | **0 = µ-law, 1 = A-law** |
| 40 | 1 | Can operate V.90 with an **upstream** symbol rate of 3429 | 1 = yes |
| 41 | 1 | Reserved for the ITU | shall be 0; the analogue modem does not interpret it |
| 42:57 | 16 | CRC over bits 12:41 | LSB (bit 42) first |
| 58:61 | 4 | Fill | `1111` |

Notes to Table 15:

- NOTE 1:
  - Bits 12, 13, 14 and 40 describe the modem's capabilities and/or configuration.
  - Bits 15 to 20 depend on regulatory requirements and **apply only to the modem's own
    transmitter**.
- NOTE 2 **[may]**: bit 24 can be used together with the V.8 PSTN access category octet to pick
  the best settings for the signal converters and error control in both modems and any CME in
  between.
- Table 7/V.8 ("access0") defines that octet. Its fields, in V.8 async framing:
  - start bit 0;
  - tag b0 to b3 = `1011`;
  - b4 = 0, marking a category octet;
  - b5 = 1 if the call DCE is on a cellular connection;
  - b6 = 1 if the answer DCE is on a cellular connection;
  - b7 = 1 if the DCE is on a digital network connection, 0 if on an analogue one;
  - stop bit 1.
- V.8's own notes on that octet:
  - A missing octet says nothing about access.
  - An analogue V.90/V.92 modem may sit on a digital network connection.

Differences from V.90 Table 7:

- Only bits 26 and 27 changed. In V.90 both were "Reserved for the ITU: set to 0 by the digital
  modem and not interpreted by the analogue modem".
- Everything else is word for word the same, including bit 20's "in V.34 mode" only.

Differences from V.34 Table 14 (INFO0):

- INFO0 is 49 bits and ends at bit 28.
- V.34 uses bits 26:27 as the transmit clock source: 0 internal, 1 synchronised to receive
  timing, 2 external, 3 reserved.
- Bits 29:41 exist only in INFO0d.

---

## 6. INFO0a: Table 16/V.92 (analogue modem capabilities), 49 bits

The analogue modem sends this on 2400 Hz at −1 dB, with the 1800 Hz guard tone at −7 dB.

| Bits | Width | Meaning (V.92) | Values |
|---|---|---|---|
| 0:3 | 4 | Fill | `1111` |
| 4:11 | 8 | Frame sync | `01110010`, left-most bit first |
| 12 | 1 | Symbol rate 2743 supported in V.34 mode | 1 = yes |
| 13 | 1 | Symbol rate 2800 supported in V.34 mode | 1 = yes |
| 14 | 1 | Symbol rate 3429 supported in V.34 mode | 1 = yes |
| 15 | 1 | Can transmit on the low carrier at 3000 | 1 = yes |
| 16 | 1 | Can transmit on the high carrier at 3000 | 1 = yes |
| 17 | 1 | Can transmit on the low carrier at 3200 | 1 = yes |
| 18 | 1 | Can transmit on the high carrier at 3200 | 1 = yes |
| 19 | 1 | Transmission at 3429 | 0 = disallowed |
| 20 | 1 | Can reduce transmit power below nominal **"in V.34 mode or in V.90 mode"** | 1 = yes |
| 21:23 | 3 | Largest allowed transmit/receive symbol-rate difference in V.34 mode | integer 0 to 5 steps |
| 24 | 1 | This INFO0a comes from a CME modem | 1 = CME |
| 25 | 1 | Supports signal constellations of up to 1664 points | 1 = yes |
| **26** | 1 | **"V.92 capability: 1"** (new) | a V.92 analogue modem sends **1** |
| **27** | 1 | **"Set to 1 requests short Phase 2 to be used"** (new) | 1 = request |
| 28 | 1 | Acknowledges correct reception of an INFO0d during error recovery | 1 = acknowledged |
| 29:44 | 16 | CRC over bits 12:28 | LSB (bit 29) first |
| 45:48 | 4 | Fill | `1111` |

Notes to Table 16:

- NOTE 1: bits 12 to 14 describe capabilities and/or configuration. Bits 15 to 20 depend on
  regulation and apply only to the modem's own transmitter. Bit 40 is not listed, because
  INFO0a has no bit 40.
- NOTE 2 **[may]**: the same as for INFO0d (bit 24 with the V.8 access octet).

Differences from V.90 Table 8:

- Bits 26 and 27 changed. In V.90 both were "Reserved for the ITU: set to 0 by the analogue
  modem".
- Bit 20 gained "in V.34 mode or in V.90 mode". V.90 said only "lower than the nominal
  setting".

**Watch the bit order.**

- In INFO0a, **bit 26 = V.92** and **bit 27 = short Phase 2 request**. INFO0d has them the
  other way round.
- The procedure text agrees:
  - 9.3: "bit 27 of INFO0d and bit 26 of INFO0a" for V.92 capability;
  - 9.4: "bit 26 of INFO0d and bit 27 of INFO0a" for the short Phase 2 request.

Difference from V.34 Table 14: bits 26:27 are the V.34 transmit-clock field there (N-3).

---

## 7. INFO1d: Table 17/V.92 (digital modem's probing results), 109 bits

- The digital modem sends this in **full** Phase 2 only (after L1/L2), on 1200 Hz at nominal
  power.
- **P2-10** (9.3, shall): the digital modem shall use this layout **when both modems have shown
  V.92 capability**. Otherwise V.90 Table 9 applies (section 10).

| Bits | Width | Meaning | Values |
|---|---|---|---|
| 0:3 | 4 | Fill | `1111` |
| 4:11 | 8 | Frame sync | `01110010` |
| 12:14 | 3 | Minimum power reduction the analogue modem's transmitter is to make | integer 0 to 7 = recommended reduction in dB. **Shall be 0** if INFO0a said the analogue transmitter cannot reduce power (INFO0a bit 20 = 0). |
| 15:17 | 3 | Additional power reduction, beyond 12:14, that the digital modem's receiver can tolerate | integer 0 to 7 dB. **Shall be 0** if INFO0a bit 20 = 0. |
| 18:24 | 7 | Length of the MD the **digital** modem sends in Phase 3 | integer 0 to 127, in **35 ms** steps |
| 25 | 1 | 2400: use the high carrier from analogue to digital | 1 = high (1800 Hz); 0 = low (1600 Hz) |
| 26:29 | 4 | 2400: pre-emphasis filter index, analogue to digital | integer 0 to 10 (Tables 3 and 4/V.34) |
| 30:33 | 4 | 2400: projected maximum data rate | integer 0 to 14 × 2400 bit/s; **0 = this rate cannot be used** |
| 34:42 | 9 | Probing results for a final rate of 2743 | coded like 25:33: 34 carrier, 35:38 pre-emphasis, 39:42 rate |
| 43:51 | 9 | Probing results for 2800 | coded like 25:33: 43, 44:47, 48:51 |
| 52:60 | 9 | Probing results for 3000 | coded like 25:33: 52, 53:56, 57:60. **Shall be consistent** with the analogue capabilities in INFO0a. |
| 61:69 | 9 | Probing results for 3200 | coded like 25:33: 61, 62:65, 66:69. **Shall be consistent** with INFO0a. |
| **70** | 1 | **"Set to 0 indicates that the channel does not support PCM upstream"** (new) | 0 = no PCM upstream; 1 = channel supports it |
| **71:78** | **8** | Probing results for 3429 | "coded like bits **26-33**": **71:74 pre-emphasis, 75:78 projected rate**. **Shall be consistent** with INFO0a. |
| 79:88 | 10 | Frequency offset of the probing tones, as measured by the **digital** modem's receiver | see below |
| 89:104 | 16 | CRC over bits 12:88 | LSB (bit 89) first |
| 105:108 | 4 | Fill | `1111` |

Frequency offset field (79:88):

- It shall be f(received) − f(transmitted) for the nominal **1050 Hz** line probing tone.
- It is a two's-complement integer from −511 to +511, in steps of **0.02 Hz**. **Bit 88 is the
  sign bit.**
- The measurement shall be accurate to **0.25 Hz**.
- If that accuracy cannot be reached, the field shall be **−512**, meaning "ignore this field".
  In time order, bits 79 to 88 are then 0,0,0,0,0,0,0,0,0,1.

Notes to Table 17:

- NOTE 1 (shall): a projected maximum data rate above 12 (above 28 800 bit/s) in bits 30:33
  shall only be indicated when the analogue modem supports 1664-point constellations (INFO0a
  bit 25).
  - [derived] The same limit applies to the matching rate field of every other symbol rate.
- NOTE 2 **[may]**: the analogue modem may get a higher downstream rate in V.90 mode if the
  digital modem lets it transmit at lower power through bits 15:17.

Differences from V.90 Table 9 (which is "identical to INFO1c" of V.34 Table 15):

- Only bits 70:78 differ. V.90 has a 9-bit 3429 field there: 70 carrier, 71:74 pre-emphasis,
  75:78 rate.
- V.92 turns bit 70 into the PCM-upstream flag and leaves the other 8 bits in place.
- **The field positions are unchanged, so an INFO1c/Table 9 parser reads a Table 17 INFO1d
  correctly. Only the meaning of bit 70 changes.**
- V.34 Table 15 says "call/answer modem" and "remote modem" where V.90 and V.92 say
  "digital/analogue modem".

How the digital modem decides bit 70 is not specified. It is a digital-modem implementation
matter.

---

## 8. INFO1a when PCM upstream is selected: Table 18/V.92, 70 bits

- **P2-11** (8.4.1): the analogue modem uses this layout to show that it wants PCM upstream.
  **It shall not use it if bit 70 of INFO1d is clear.**
- **P2-12** (9.3, **[may]**): in full Phase 2, the analogue modem may select PCM upstream with
  Table 18 only when both modems have shown V.92 capability (INFO0d bit 27 = 1 and INFO0a bit
  26 = 1).
- The analogue modem sends it on 2400 Hz at −1 dB, with the guard tone at −7 dB.

| Bits | Width | Meaning | Values |
|---|---|---|---|
| 0:3 | 4 | Fill | `1111` |
| 4:11 | 8 | Frame sync | `01110010` |
| 12:13 | 2 | Number of filter sections in the precoder and prefilter | 0 = p1(i) and z2(i) supported; 1 = z1(i), p1(i), z2(i); 2 = p1(i), p2(i), z2(i); 3 = z1(i), p1(i), p2(i), z2(i) |
| 14:15 | 2 | Maximum number of coefficients the analogue modem supports, in multiples of 64 starting at 192: L_tot = LZ1 + LP1 + LZ2 + LP2 | 0 = 192, 1 = 256, 2 = 320, 3 = 384 |
| 16:17 | 2 | Maximum number of coefficients per filter section, in multiples of 64 starting at 128: L_max = max{LZ1, LP1, LZ2, LP2} | 0 = 128, 1 = 192, 2 = 256, 3 = 320 |
| 18:24 | 7 | Length of the MD the **analogue** modem sends in Phase 3 | integer 0 to 127, in **276-symbol (34.5 ms)** steps |
| 25:31 | 7 | U_INFO: Ucode of the PCM codeword the digital modem uses for the 2-point train | Its power **shall not exceed** the maximum digital modem transmit power (INFO0d 33:37). **U_INFO shall be greater than 66.** |
| 32:33 | 2 | Reserved for the ITU | set to 0; not interpreted by the digital modem |
| 34:36 | 3 | "Symbol rate of 8000 to be used by the **analogue** modem" | **the integer 6** |
| 37:39 | 3 | "Symbol rate of 8000 to be used by the **digital** modem" | **the integer 6** |
| 40:49 | 10 | Reserved for the ITU | **set to 1** (all ones); not interpreted by the digital modem. NOTE: ones "to avoid generating a tone". |
| 50:65 | 16 | CRC over bits 12:49 | LSB (bit 50) first |
| 66:69 | 4 | Fill | `1111` |

Filter sections (6.4.2/V.92):

- Precoder output: x(n) = u(n) + Σ_{k=1..LZ1} u(n−k)·z1(k) + Σ_{k=1..LP1} x(n−k)·p1(k).
- Prefilter output: v(n) = Σ_{k=0..LZ2−1} x(n−k)·z2(k) + Σ_{k=1..LP2} v(n−k)·p2(k). This is
  then multiplied by the gain G.
- p1 and z2 are always supported. [derived] Read as a 2-bit value, **bit 12 = z1 supported**
  and **bit 13 = p2 supported**.

Users of bits 12:17 in CPd (8.8.3, Table 30/V.92):

- LZ1 (bits 154:162), LP1 (171:179), LZ2 (188:196) and LP2 (205:213) are each "up to L_max
  given in bits 16:17 of INFO1a".
- (8.8.3, shall) LZ1 + LP1 + LZ2 + LP2 shall not exceed L_tot from bits 14:15 of INFO1a.
- [derived] If bits 12:13 show that z1 or p2 is unsupported, the digital modem should send
  LZ1 = 0 or LP2 = 0 respectively. Table 30 does not say this in so many words.
- (8.8.3, shall) the digital modem shall design the parameters on the assumption that the
  analogue modem transmits at the desired power when the prefilter output times G has a
  mean-square value of 1.
- Table 18 has no power-reduction fields. Upstream PCM level is controlled through CPd and G,
  and through L_U (V.92 3.8).

Why the MD step is 276 symbols [derived]:

- The PCM upstream symbol rate is 8000/s, and data frames are 6 symbols (12 for the modulus
  encoder).
- 276 = 23 × 12, so the analogue modem's MD is always a whole number of 12-symbol frames. 35 ms
  (280 symbols) would not be.
- 127 × 34.5 = 4381.5 ms.

Differences from V.90 Table 10 (the V.90-mode INFO1a):

| Bits | V.90 Table 10 | V.92 Table 18 |
|---|---|---|
| 12:17 | Reserved, 0 | 12:13 sections, 14:15 L_tot, 16:17 L_max |
| 18:24 | MD length × 35 ms | MD length × **276 symbols (34.5 ms)** |
| 25:31 | U_INFO | U_INFO (the same wording) |
| 32:33 | Reserved, 0 | Reserved, 0 |
| 34:36 | Upstream symbol rate 3 to 5 (3000/3200/3429), consistent with INFO1d | **6** (8000, analogue modem) |
| 37:39 | 6 (8000, digital modem) | 6 (the same) |
| 40:49 | Frequency offset measured by the analogue modem | **Reserved, all ones** (no frequency offset) |

---

## 9. INFO1a when V.34 upstream is selected during short Phase 2: Table 19/V.92, 70 bits

- **P2-13** (8.4.1): the analogue modem uses this layout **during short Phase 2** to show that
  it wants V.34-style upstream. That means V.90 data mode: PCM downstream with V.34-modulated
  upstream (9.4, "V.90 data mode").

| Bits | Width | Meaning | Values |
|---|---|---|---|
| 0:3 | 4 | Fill | `1111` |
| 4:11 | 8 | Frame sync | `01110010` |
| 12:17 | 6 | Reserved for the ITU | set to 0; not interpreted by the digital modem |
| 18:24 | 7 | Length of the MD the **analogue** modem sends in Phase 3 | integer 0 to 127, in **35 ms** steps |
| 25:31 | 7 | U_INFO: Ucode of the PCM codeword the digital modem uses for the 2-point train | power **shall not exceed** the maximum digital transmit power; **U_INFO shall be > 66** |
| 32 | 1 | Reserved for the ITU | set to 0; not interpreted |
| **33** | 1 | **"Set to 1 indicates that the high carrier frequency is to be used in transmitting from the analogue modem to the digital modem"** (new relative to V.90) | 1 = high carrier; 0 = low carrier, for the rate in 34:36 (section 4) |
| 34:36 | 3 | Symbol rate from analogue to digital | integer 3 to 5: **3 = 3000, 4 = 3200, 5 = 3429** |
| 37:39 | 3 | "Symbol rate of 8000 to be used by the digital modem" | **the integer 6** |
| 40:49 | 10 | Frequency offset of the probing tones as measured by the **analogue** modem's receiver | f(rx) − f(tx) of the 1050 Hz tone; two's complement −511 to +511 in 0.02 Hz steps; **bit 49 is the sign bit**; accurate to 0.25 Hz, otherwise **−512 = ignore** |
| 50:65 | 16 | CRC over bits 12:49 | LSB (bit 50) first |
| 66:69 | 4 | Fill | `1111` |

Differences from V.90 Table 10:

- **Bit 33** now selects the upstream carrier. In V.90, bits 32:33 are both reserved.
- V.90's 34:36 text says the rate "shall be consistent with information in INFO1d" and that
  "the carrier frequency and pre-emphasis filter to be used are those already indicated for
  this symbol rate in INFO1d".
  - Table 19 drops that sentence, because short Phase 2 has no INFO1d.
  - So the carrier comes from bit 33, and **the pre-emphasis filter is not signalled at all**
    (N-10).
- V.90 Table 10 prints "Bit 9 is the sign bit", a misprint. V.92 Table 19 (and V.34 Table 16)
  correctly say bit 49.

---

## 10. V.90 layouts V.92 still uses, and how to tell the INFO1a layouts apart

### 10.1 When each layout applies (9.3/V.92)

- **P2-14** (9.3, shall): if **either** modem does not show V.92 capability (INFO0d bit 27 = 0
  or INFO0a bit 26 = 0), both modems shall use the information bits of 8.2.3.2/V.90. That
  means:
  - INFO1d = V.90 Table 9, where bit 70 is the 3429 high-carrier bit;
  - INFO1a = V.90 Table 10 or Table 11.
  - INFO0d and INFO0a are still sent in the Table 15/16 layout. A V.92 modem sets its own V.92
    bit, and a V.90 peer sends 0 there (reserved in V.90).
- **P2-15** (9.3, shall): if both show V.92 capability, the digital modem shall use the Table 17
  INFO1d. The analogue modem **[may]** then select PCM upstream with Table 18.
- 9.3 does not say which INFO1a the analogue modem sends in a V.92/V.92 **full** Phase 2 when
  it does **not** pick PCM upstream. See N-9.

| Phase 2 | Both V.92? | INFO1d | INFO1a the analogue modem may send |
|---|---|---|---|
| Full | no | V.90 Table 9 | V.90 Table 10 (V.90 mode) or V.90 Table 11 (V.34 mode) |
| Full | yes | V.92 Table 17 | Table 18 (only if INFO1d bit 70 = 1); otherwise Table 10 or Table 11 (N-9) |
| Short | yes (required) | none sent | Table 18 or Table 19. The short Phase 2 request is only allowed when the analogue modem means to connect in PCM upstream or V.90 data mode (P2-19), so Table 11 cannot be used here. |

### 10.2 V.90 Table 10: INFO1a when V.90 is selected (70 bits)

| Bits | Meaning |
|---|---|
| 0:3, 4:11 | fill, sync |
| 12:17 | reserved, 0 |
| 18:24 | analogue modem MD length × 35 ms |
| 25:31 | U_INFO (> 66; power ≤ the digital modem's maximum) |
| 32:33 | reserved, 0 |
| 34:36 | upstream symbol rate 3 to 5. It shall be consistent with INFO1d. The carrier and pre-emphasis are the ones INFO1d gave for that rate. |
| 37:39 | 6 |
| 40:49 | frequency offset measured by the analogue modem; sign bit 49 (misprinted as "bit 9"); −512 = ignore |
| 50:65, 66:69 | CRC, fill |

Bits 37:39 = 6 send the digital modem to Phase 3 (V.90 9.2.1.1.8).

### 10.3 V.90 Table 11: INFO1a when V.34 is selected (70 bits, = V.34 Table 16)

| Bits | Meaning |
|---|---|
| 12:14 | Minimum power reduction for the **digital** modem's transmitter, 0 to 7 dB. Shall be 0 if INFO0d bit 20 says the digital modem cannot reduce power. |
| 15:17 | Additional reduction the analogue modem's receiver tolerates, 0 to 7 dB. The same "shall be 0" rule applies. |
| 18:24 | Analogue modem MD length × 35 ms |
| 25 | High carrier from digital to analogue. Shall be consistent with the digital modem's capabilities in INFO0d. |
| 26:29 | Pre-emphasis index (0 to 10), digital to analogue |
| 30:33 | Projected maximum rate, digital to analogue, × 2400 bit/s (0 to 14) |
| 34:36 | Symbol rate analogue→digital, 0 to 5. Shall be consistent with INFO1d and with the asymmetry allowed by INFO0a/INFO0d. Carrier and pre-emphasis are as INFO1d gave for that rate. |
| 37:39 | Symbol rate digital→analogue, 0 to 5. Shall be consistent with the INFO0a capabilities and the allowed asymmetry. |
| 40:49 | Frequency offset measured by the analogue modem (sign bit 49) |
| 50:65, 66:69 | CRC, fill |

- NOTE: a rate above 12 in 30:33 is allowed only if the **digital** modem supports
  1664-point constellations.
- With 37:39 in 0 to 5, both modems continue as V.34 (9.2.1.1.8 and 9.2.2.1.9/V.90):
  - the digital modem follows 11.3.1.1/V.34 as the call modem;
  - the analogue modem follows 11.3.1.2/V.34 as the answer modem.

### 10.4 Classifying a received 70-bit INFO1a (digital-modem receiver)

| 37:39 | 34:36 | Layout | Next |
|---|---|---|---|
| 0 to 5 | 0 to 5 | V.90 Table 11 (V.34 mode) | V.34 Phase 3 (11.3/V.34), digital modem as call modem |
| 6 | 3 to 5 | V.90 Table 10 in full Phase 2, or V.92 Table 19 in short Phase 2 | V.90-mode Phase 3: PCM downstream, V.34 upstream (N-18) |
| 6 | **6** | **V.92 Table 18** (PCM upstream) | V.92 Phase 3 (9.5/V.92) |
| 6 | 0, 1, 2 or 7 | none: invalid | treat as not received [suggestion] |
| 7 | any | none: invalid | treat as not received [suggestion] |

- In a Table 18 frame, bits 40:49 are all ones. Read as a frequency offset they would give
  −0.02 Hz. They are reserved and must not be read as an offset.
- [derived] The receiver should also check the context:
  - A Table 18 frame is valid only if both modems are V.92 and, in full Phase 2, the sent
    INFO1d had bit 70 = 1.
  - A Table 19 frame (bit 33 meaningful) exists only in short Phase 2.

---

## 11. How the sequences are used (procedure hooks)

This section is for the INFO codec and the Phase 2 state machine. The full procedure digest is
`spec-phase2-procedures.md`. Clause numbers are kept so the two can be checked against each
other.

### 11.1 Choosing full or short Phase 2 (9.3, 9.3.1, 9.4/V.92)

- **P2-16** (9.4, shall): the modems shall run short Phase 2 only if **all four** of these
  hold:
  - INFO0d bit 27 = 1 (the digital modem is V.92);
  - INFO0d bit 26 = 1 (the digital modem wants short Phase 2);
  - INFO0a bit 26 = 1 (the analogue modem is V.92);
  - INFO0a bit 27 = 1 (the analogue modem wants short Phase 2).

  Otherwise both run full Phase 2.
- **P2-17** (9.3, shall): if both modems show V.92 capability, **any later retrain shall use
  V.92's Phase 2**.
  - V.90 9.2.1.1.8 and 9.2.2.1.9 say the matching thing for V.90: later retrains use V.90
    Phase 2 whatever mode was chosen.
- **P2-18** (9.3.1, shall): if both show V.92 capability **and** both indicated LAPM in V.8 or
  V.8 bis, the V.42 ODP/ADP exchange shall be bypassed.
  - 9.2.5 has a similar rule for short Phase 1: bypass whenever both indicated LAPM.
- **P2-19** (9.4, shall): the analogue modem shall set INFO0a bit 27 only if it intends to
  connect in **PCM upstream or V.90 data mode**. It must not set it if it may fall back to V.34.
- [derived] Each modem must know which Phase 2 it is in **before** the first reversal, because
  the two variants reverse in opposite orders:
  - In full Phase 2 the analogue modem reverses A first.
  - In short Phase 2 the digital modem reverses B first, and the analogue modem must not
    reverse A until the B reversal arrives.
  - Both decisions therefore rest on a correctly received far-end INFO0. The bit-28 recovery
    (below) is what guarantees one.

### 11.2 Short Phase 2 (9.4/V.92, Figure 9)

Figure 9, as drawn:

```
Analogue: INFO0a | A ...................... | A-bar 10 ms | INFO1a | (silence) | [Phase 3]
Digital : INFO0d | B (>50 ms) | B-bar 10 ms | silence ...
                              ^ B reversal         ^ A reversal sent 40 ms after the B reversal arrives
```

- The figure labels the B segment ">50 ms". The text says "at least 50 ms".
- INFO1a follows the 10 ms of A-bar with no gap in the drawing.

Digital modem:

- **9.4.1.1.1** (shall):
  - During the **75 ± 5 ms** silence that ends Phase 1, it conditions its receiver for INFO0a
    and Tone A.
  - After that silence it sends **INFO0d with bit 28 = 0**, then Tone B.
- **9.4.1.1.2** (shall): after receiving INFO0a, it conditions its receiver to detect Tone A and
  to receive INFO0a again (recovery).
- **9.4.1.1.3** (shall):
  - Once Tone A is detected **and** B has been sent for **≥ 50 ms**, it sends a **B phase
    reversal**.
  - It keeps B for **10 ms** after the reversal.
  - Then it goes silent and waits for the A reversal.
- **9.4.1.1.4** (shall):
  - **RTDEd** = the time from the B reversal appearing at its own line terminals to the A
    reversal arriving at them, **minus 40 ms**.
  - It then stays silent and conditions its receiver for INFO1a.
- **9.4.1.1.5** (shall): on INFO1a, it proceeds to "the appropriate Phase 3 as signalled in
  INFO1a" (section 10.4).
- **9.4.1.2.1** (recovery, shall): this applies if it detects Tone A before correctly
  receiving INFO0a, or if it receives repeated INFO0a.
  - It **repeatedly sends INFO0d**, with **bit 28 = 1** once INFO0a has been received
    correctly.
  - If it receives INFO0a with bit 28 = 1, it conditions its receiver for Tone A and the
    following A reversal, **finishes the current INFO0d**, then sends Tone B.
  - Alternatively, once it has detected Tone A and correctly received INFO0a, it conditions its
    receiver for the A reversal, finishes the current INFO0d, then sends Tone B.
  - In both cases it then continues at 9.4.1.1.3.
- **9.4.1.2.2** (shall): if **no A reversal arrives within 2500 ms of sending the B reversal**:
  - it conditions its receiver for Tone A;
  - on Tone A, it sends Tone B and conditions its receiver for the A reversal;
  - it then continues with **full Phase 2**.
- **9.4.1.2.3** (shall): if **no INFO1a arrives within 2500 ms of sending the B reversal**:
  - it sends Tone B and conditions its receiver for Tone A;
  - on Tone A, it conditions its receiver for the A reversal and continues with **full
    Phase 2**.

Analogue modem:

- **9.4.2.1.1** (shall):
  - During the 75 ± 5 ms silence it conditions its receiver for INFO0d and Tone B.
  - Then it sends **INFO0a with bit 28 = 0**, then Tone A.
- **9.4.2.1.2** (shall): after INFO0d, it conditions its receiver to detect Tone B, to receive
  INFO0d again (recovery), and to detect the B reversal that follows.
- **9.4.2.1.3** (shall):
  - On the B reversal it sends an **A reversal**, delayed so that the time from the B reversal
    arriving at its line terminals to the A reversal appearing there is **40 ± 1 ms**.
  - Tone A continues for **10 ms** after the reversal.
- **9.4.2.1.4** (shall): "Then" it sends **INFO1a** (Table 18 or Table 19) and proceeds to the
  Phase 3 that INFO1a names.
  - 9.5.2.1.1 (PCM upstream) requires **70 ± 5 ms** of silence after INFO1a, then Ru for 384T.
  - V.90 9.3.2.1 has the same 70 ± 5 ms before S for V.90 mode.
- **9.4.2.2.1** (recovery, shall): the mirror image of 9.4.1.2.1.
  - It repeats INFO0a, with bit 28 = 1 once INFO0d has been received correctly.
  - If INFO0d arrives with bit 28 = 1, it conditions its receiver for Tone B, finishes the
    current INFO0a, then sends Tone A.
  - Alternatively, once it has detected Tone B and correctly received INFO0d, it finishes the
    current INFO0a and sends Tone A.
  - It then continues at 9.4.2.1.3.
- **9.4.2.2.2** (shall): if **no B reversal arrives within 2500 ms of the end of its INFO0a**,
  it initiates a retrain per **9.7.2.1**, which leads into full Phase 2.

What short Phase 2 does **not** have:

- no L1/L2;
- no INFO1d;
- no INFOMARKS;
- no second pair of reversals, so **no RTDEa** (N-11).

### 11.3 Full Phase 2 (9.3/V.92 = V.90 9.2, Figure 4/V.90)

9.3 says the operating and recovery procedures are "identical to those for Phase 2 of ITU-T
V.90". The only difference is which information bits are used (P2-14, P2-15).

Figure 4/V.90, read from the render:

```
Digital : INFO0d | B ........ | B-bar 10 ms | silence (<= 670 ms) | B ....... | B-bar 10 ms | L1 160 ms | L2 | INFO1d | silence
Analogue: INFO0a | A (>= 50 ms) | A-bar ............ | A 10 ms | L1 160 ms | L2 | A 50 ms | A-bar 10 ms | silence (<= 670 ms) | A ... | INFO1a | silence 70 +/- 5 ms | [Phase 3]
```

- On the analogue side, **A-bar continues** after the first A reversal. It is not silence.
  - The tone reverses back to A 40 ± 1 ms after the B reversal arrives.
  - A is kept for 10 ms, then L1 starts.
- Every answering reversal is marked 40 ± 1 ms after the reversal it answers.
- INFO1d arrives while the analogue modem is sending Tone A. INFO1a follows that Tone A.

Digital modem (9.2.1/V.90):

- **9.2.1.1.1**: 75 ± 5 ms silence, receiver set for INFO0a and Tone A. Then **INFO0d with bit
  28 = 0**, then Tone B.
- **9.2.1.1.2**: after INFO0a, the receiver is set for Tone A, a repeated INFO0a (recovery),
  and the A reversal.
- **9.2.1.1.3**:
  - On the A reversal, send a B reversal delayed to **40 ± 1 ms** after it (at the line
    terminals).
  - Keep B for **10 ms**, then go silent.
  - Wait for the **second** A reversal.
- **9.2.1.1.4**:
  - **RTDEd** = the time from the B reversal appearing at its terminals to the second A
    reversal arriving, **minus 40 ms**.
  - Then condition the receiver for L1/L2.
- **9.2.1.1.5**:
  - Receive L1 for its **160 ms**.
  - **[may]** receive L2 for **no more than 500 ms**.
  - Then send Tone B and condition the receiver for Tone A and its reversal.
- **9.2.1.1.6**:
  - On the A reversal, send a B reversal at **40 ± 1 ms** and keep B for **10 ms**.
  - Then send **L1, then L2**, and look for Tone A.
- **9.2.1.1.7**: once Tone A is detected and the local echo of L2 has been received for no more
  than **550 ms + RTD**, **send INFO1d**, laid out per P2-14 and P2-15.
- **9.2.1.1.8**:
  - After INFO1d, go silent and wait for INFO1a.
  - If 37:39 = 6, go to Phase 3.
  - If 37:39 is 0 to 5, continue per 11.3.1.1/V.34 as the call modem.
  - Later retrains use this Recommendation's Phase 2 (P2-17).
- **9.2.1.2.1** (recovery): if Tone A arrives before INFO0a, or INFO0a repeats, run the INFO0d
  repeat and bit-28 logic of 11.2, then continue at 9.2.1.1.3.
- **9.2.1.2.2**: if the A reversal of 9.2.1.1.3 never comes, **keep sending Tone B** until it
  does.
- **9.2.1.2.3**: if the second A reversal does not come **within 2000 ms** of the first
  (9.2.1.1.3):
  - go silent and look for Tone A;
  - on Tone A, send Tone B, look for the A reversal, and continue at 9.2.1.1.3.
- **9.2.1.2.4**: if the A reversal of 9.2.1.1.6 does not come **within 900 ms + RTD** of the
  reversal detected in 9.2.1.1.4:
  - wait **40 ms**, send a B reversal, and keep B for **10 ms**;
  - then send L1 and L2, look for Tone A, and continue at 9.2.1.1.7.
- **9.2.1.2.5**: if Tone A is not detected **within 650 ms + RTD of the start of L2**, initiate
  a retrain. V.90 cites 9.5.1.1; in V.92 that is **9.7.1.1** (N-19).
- **9.2.1.2.6**: if INFO1a does not arrive **within 700 ms + RTD of the end of INFO1d**, look
  for Tone A or INFOMARKSa.
  - On INFOMARKSa, **either** initiate a retrain **or resend INFO1d** and continue at
    9.2.1.1.8.
  - On Tone A, respond to a retrain (V.90 9.5.1.2, which is V.92 9.7.1.2).

Analogue modem (9.2.2/V.90):

- **9.2.2.1.1**: 75 ± 5 ms silence, receiver set for INFO0d and Tone B. Then **INFO0a with bit
  28 = 0**, then Tone A.
- **9.2.2.1.2**: after INFO0d, the receiver is set for Tone B and a repeated INFO0d
  (recovery).
- **9.2.2.1.3**: once Tone B is detected **and** Tone A has been sent for **≥ 50 ms**, send an
  A reversal and look for the B reversal.
- **9.2.2.1.4**: **RTDEa** = the time from its own A reversal at its terminals to the B
  reversal arriving there, **minus 40 ms**.
- **9.2.2.1.5**:
  - Send an A reversal delayed to **40 ± 1 ms** after the B reversal arrives, and keep A for
    **10 ms**.
  - Then send **L1, then L2**, and look for Tone B.
- **9.2.2.1.6**:
  - Once Tone B is detected and the L2 echo has been received for no more than **550 ms +
    RTD**, send Tone A for **50 ms**, then an A reversal, then **10 ms** more of tone.
  - Then go silent and look for the B reversal.
- **9.2.2.1.7**: on the B reversal, condition the receiver for L1/L2.
- **9.2.2.1.8**:
  - Receive L1 (**160 ms**) and **[may]** L2 (**no more than 500 ms**).
  - Then send Tone A and condition the receiver for INFO1d.
- **9.2.2.1.9**:
  - After INFO1d, send INFO1a, choosing the mode with 37:39. In V.92, a value of 6 in 34:36 as
    well selects PCM upstream.
  - Then go to Phase 3, or to 11.3.1.2/V.34 as the answer modem.
- **9.2.2.2.1** (recovery): if Tone B arrives before INFO0d (in 9.2.2.1.2, .3 or .4), or INFO0d
  repeats, run the INFO0a repeat and bit-28 logic, then continue at 9.2.2.1.3.
- **9.2.2.2.2**: if the B reversal of 9.2.2.1.4 does not come **within 2000 ms**, look for
  Tone B and continue at 9.2.2.1.3.
- **9.2.2.2.3**: if Tone B does not come **within 600 ms + RTD of the start of L2** (in
  9.2.2.1.6):
  - look for Tone B and send Tone A;
  - continue at 9.2.2.1.3.
- **9.2.2.2.4**: if INFO1d does not arrive **within 2000 ms + 2 RTD** of detecting Tone B in
  9.2.2.1.6, do **one of two things**:
  - initiate a retrain; **or**
  - send **INFOMARKSa** until INFO1d arrives (then continue at 9.2.2.1.9) or Tone B is
    detected (then continue at 9.2.2.1.3).

### 11.4 Retrains land in full Phase 2 without INFO0 (9.7/V.92)

- **9.7.1.1** (digital modem initiates):
  - circuit 106 OFF, circuit 104 clamped to binary 1, **70 ± 5 ms** of silence;
  - then Tone B, and look for Tone A;
  - on Tone A, look for the A reversal and continue with full Phase 2.
- **9.7.1.2** (digital modem responds):
  - after Tone A has been detected for **> 50 ms**, the same circuit actions and 70 ± 5 ms of
    silence;
  - then Tone B, look for the A reversal, and continue with full Phase 2.
- **9.7.2.1** (analogue modem initiates):
  - the same circuit actions and 70 ± 5 ms of silence;
  - then Tone A, and look for Tone B;
  - once Tone B is detected and A has been sent for **≥ 50 ms**, send an A reversal, look for
    the B reversal, and continue with full Phase 2.
- **9.7.2.2** (analogue modem responds):
  - after Tone B has been detected for **> 50 ms**, the same circuit actions and silence;
  - then Tone A and full Phase 2.
- V.90's matching text (9.5/V.90) names the entry points:
  - 9.5.1.1 and 9.5.1.2 enter at 9.2.1.1.3;
  - 9.5.2.1 enters at 9.2.2.1.4;
  - 9.5.2.2 enters at 9.2.2.1.3.
- So [derived] **INFO0d and INFO0a are not sent again on a retrain**. Every INFO0-derived fact
  must be kept from the first exchange for the whole call:
  - V.92 on both sides;
  - power levels and measurement point;
  - µ-law or A-law;
  - V.90 upstream at 3429;
  - CME;
  - 1664-point support;
  - V.34 capabilities.
- A retrain always does full Phase 2, with the V.92 INFO1d/INFO1a layouts when both sides are
  V.92 (P2-17).

### 11.5 Phase 3/4 readers of INFO1a (V.92 9.5/9.6; V.90 9.3)

- **9.5.1.1.1** (digital modem):
  - If the MD length in INFO1a is zero, it proceeds directly.
  - Otherwise, after the Ru-to-R̄u transition, it waits out the MD duration given in INFO1a,
    then conditions its receiver for Ru and the next Ru-to-R̄u transition.
- **9.5.1.2.1**: the digital modem retrains (9.7.1.1) if Ja is not detected **within 4500 ms +
  RTD of the end of INFO1a**.
- **9.5.2.1.1** (analogue modem):
  - After INFO1a: silence **70 ± 5 ms**, then Ru for 384T, then R̄u for 24T.
  - If its MD length is non-zero, it then sends MD for that length, then Ru 384T and R̄u 24T
    again.
- **9.5.2.1.2** (shall): TRN1u lasts at least 2040T, and **the time from the start of MD to the
  end of TRN1u shall not exceed one RTD + 4000 ms**.
  - [derived] This caps the Table 18 MD length. After MD come Ru + R̄u (408T = 51 ms) and at
    least 2040T (255 ms) of TRN1u.
  - So n × 34.5 ms + 306 ms ≤ RTD + 4000 ms, which gives **n ≤ 107 with RTD taken as 0**.
- **V.90 9.3.2.1 and 9.3.2.3** (Table 19 or Table 10 selected): MD is followed by S, S̄, PP,
  and TRN of at least 512T. The time from the start of MD to the end of TRN shall not exceed
  RTD + 4000 ms.
- **9.6.1.2.1 and 9.6.2.2.1**: B1u (at the digital modem) and B1d (at the analogue modem) must
  arrive **within 20 s + 6 RTD of the end of INFO1a**, or that modem retrains.
- **8.6.x and 8.4.x/V.90**:
  - U_INFO sets the codeword for TRN1d, Jd, Jp, Jp′, SCR and Ri.
  - **Sd uses W = the codeword with Ucode 16 + U_INFO** (8.4.4/V.90, used by 8.6.7/V.92).
  - Ucodes stop at 127, so [derived] **U_INFO must be 67 to 111**.
- MD itself (8.5.3/V.92 → 10.1.3.5/V.34) is an optional manufacturer-defined echo-canceller
  training signal. "If the signal is not present, the MD length indication will be 0."

---

## 12. Timers and tolerances in one place

| Item | Value | Clause |
|---|---|---|
| INFO bit rate | 600 bit/s ± 0.01 % | 8.2.3.1/V.90 |
| INFO carriers | analogue 2400 Hz ± 0.01 %; digital 1200 Hz ± 0.01 % | 8.2.3.1/V.90 |
| INFO levels | analogue: carrier nominal −1 dB, guard 1800 Hz ± 0.01 % at nominal −7 dB; digital: nominal | 8.2.3.1/V.90 |
| INFO spectrum | Figure 13/V.34 mask (section 2.4) | 8.2.3.1/V.90 |
| Tone A / Tone B | 2400 Hz with 1800 Hz guard / 1200 Hz | 10.1.2.1–2/V.34 |
| Tone A levels | tone at nominal −1 dB; guard "at the nominal" (as printed) | 10.1.2.1/V.34 |
| L1 | 150 Hz ± 0.01 % repetition, 160 ms (24 periods), nominal +6 dB | 10.1.2.4/V.34 |
| L2 (sent) | nominal, ≤ 550 ms + RTD | 10.1.2.4/V.34 |
| L2 (received) | **[may]** ≤ 500 ms | 9.2.1.1.5, 9.2.2.1.8/V.90 |
| L2 echo before the next step | ≤ 550 ms + RTD | 9.2.1.1.7, 9.2.2.1.6/V.90 |
| Silence ending Phase 1 | 75 ± 5 ms | 9.4.x.1.1/V.92; 9.2.x.1.1/V.90 |
| Reversal turnaround | 40 ± 1 ms, line terminal to line terminal | 9.4.2.1.3/V.92; 9.2.1.1.3, 9.2.1.1.6, 9.2.2.1.5/V.90 |
| Tone kept after a reversal | 10 ms | same clauses; 9.2.2.1.6/V.90 |
| Tone before the first reversal | ≥ 50 ms (B in short Phase 2; A in full Phase 2 and in 9.7.2.1) | 9.4.1.1.3/V.92; 9.2.2.1.3/V.90 |
| Tone A before the second reversal (analogue) | 50 ms | 9.2.2.1.6/V.90 |
| RTDE | measured interval − 40 ms | 9.4.1.1.4/V.92; 9.2.1.1.4, 9.2.2.1.4/V.90 |
| Short Phase 2: A reversal deadline (digital) | 2500 ms from sending the B reversal | 9.4.1.2.2 |
| Short Phase 2: INFO1a deadline (digital) | 2500 ms from sending the B reversal | 9.4.1.2.3 |
| Short Phase 2: B reversal deadline (analogue) | 2500 ms from the end of INFO0a | 9.4.2.2.2 |
| Full Phase 2: second A reversal (digital) | 2000 ms from the first | 9.2.1.2.3/V.90 |
| Full Phase 2: third A reversal (digital) | 900 ms + RTD, then wait 40 ms and reverse anyway | 9.2.1.2.4/V.90 |
| Full Phase 2: Tone A after L2 (digital) | 650 ms + RTD from the start of L2 | 9.2.1.2.5/V.90 |
| Full Phase 2: INFO1a (digital) | 700 ms + RTD from the end of INFO1d | 9.2.1.2.6/V.90 |
| Full Phase 2: B reversal (analogue) | 2000 ms | 9.2.2.2.2/V.90 |
| Full Phase 2: Tone B after L2 (analogue) | 600 ms + RTD from the start of L2 | 9.2.2.2.3/V.90 |
| Full Phase 2: INFO1d (analogue) | 2000 ms + 2 RTD from detecting Tone B | 9.2.2.2.4/V.90 |
| Silences in Figure 4 | ≤ 670 ms (figure only) | Figure 4/V.90 |
| Silence after INFO1a | 70 ± 5 ms | Figure 4/V.90; 9.5.2.1.1/V.92; 9.3.2.1/V.90 |
| Retrain silence | 70 ± 5 ms | 9.7/V.92 |
| Tone detection before responding to a retrain | > 50 ms | 9.7.1.2, 9.7.2.2/V.92 |
| Frequency offset field | 0.02 Hz steps, ±511, accuracy 0.25 Hz, −512 = ignore | Tables 17, 19 (and V.90 Tables 9 to 11) |
| MD length steps | 35 ms (Table 17, Table 19, V.90 Tables 9 to 11); 276 symbols = 34.5 ms (Table 18) | Tables 17 to 19 |
| MD start to end of training | ≤ RTD + 4000 ms | 9.5.2.1.2/V.92; 9.3.2.3/V.90 |
| INFO0d power fields | 29:32: −(6 + n) dBm0; 33:37: −0.5 × (n + 1) dBm0 | Table 15 |
| Filter coefficient limits | L_tot 192 + 64n; L_max 128 + 64n | Table 18 |

---

## 13. Every shall, should and may in clause 8.4 (and in the signal text it imports)

| Where | Word | Requirement |
|---|---|---|
| 8.4.1 / Table 18 | shall not | Do not use Table 18 if INFO1d bit 70 is clear. |
| Table 15 bit 41 | (set to 0) | Reserved bit sent as 0; the analogue modem does not interpret it. |
| Table 15 bit 38 | shall | The power is measured at the codec output (bit 38 = 1) or at the digital modem's terminals (bit 38 = 0). |
| Tables 15/16 NOTE 2 | may | Bit 24 may be combined with the V.8 PSTN access octet. |
| Table 17 12:14, 15:17 | shall | These fields are 0 if INFO0a says the analogue transmitter cannot reduce power. |
| Table 17 52:60, 61:69, 71:78 | shall | These fields are consistent with the INFO0a capabilities. |
| Table 17 79:88; Table 19 40:49 | shall | Offset = f(rx) − f(tx); accurate to 0.25 Hz; otherwise −512. |
| Table 17 NOTE 1 | shall only | A rate above 12 only if the analogue modem supports 1664 points. |
| Table 17 NOTE 2 | may | A higher downstream rate may follow from allowing lower analogue power. |
| Tables 18/19 25:31 | shall not / shall | U_INFO power ≤ the digital maximum; U_INFO > 66. |
| Tables 18/19 reserved bits | (set to 0 or 1) | Sent as stated; the digital modem does not interpret them. |
| 8.2/V.90 | shall | Phase 2 at nominal power except L1; recovery returns to nominal. |
| 8.2.3.1/V.90 | shall | INFO spectrum inside Figure 13/V.34. |
| 8.2.3.1/V.90 NOTE | (highly desirable) | Linear-phase filters. |
| 10.1.2.1–2/V.34 NOTE | should | Do not restrict tone bandwidth so much that RTD accuracy suffers. |
| 10.1.2.4/V.34 NOTE | should | Probing tones accurate enough. |
| 9.3 | shall / may | V.92 INFO1d when both are V.92; the analogue modem may choose Table 18; otherwise V.90 bits; later retrains use V.92 Phase 2. |
| 9.3.1 | shall | Bypass ODP/ADP if both are V.92 and both indicated LAPM. |
| 9.4 | shall / shall only | Short Phase 2 only on all four bits; the analogue modem requests it only for PCM upstream or V.90 data mode. |

---

## 14. Cross-reference list (what each outside reference requires)

| Reference | What it requires | Section here |
|---|---|---|
| "defined in ITU-T V.90" (8.4) | V.90 8.2: power rule, A, B, INFO modulation, INFOMARKS, L1/L2 | 2 |
| 10.1.2.1 / 10.1.2.2 / 10.1.2.4 of V.34 (via 8.2.1, 8.2.2, 8.2.4/V.90) | Tone A with guard, Tone B, L1/L2 with Table 17/V.34 phases | 2.2, 2.3, 2.6 |
| Figure 13/V.34 (via 8.2.3.1/V.90) | INFO transmit spectrum mask | 2.4 |
| 10.1.2.3.2/V.34, Figure 14 (8.4.1) | CRC x^16+x^12+x^5+1, init all ones, output from cell 0 as LSB, information bits only | 3.4 |
| Tables 3/V.34 and 4/V.34 (Table 17) | Pre-emphasis indices | 4 |
| PSTN access octet of V.8 (Tables 15/16 NOTE 2) | Table 7/V.8 "access0" | 5 |
| 8.2.3.2/V.90 (9.3) | V.90 Tables 7 to 11, used when either side is not V.92 | 10 |
| Phase 2 of V.90 (9.3) | V.90 9.2 procedures and Figure 4 | 11.3 |
| 11.3.1.1 / 11.3.1.2 of V.34 (via V.90 9.2.1.1.8, 9.2.2.1.9) | V.34 Phase 3 when INFO1a 37:39 = 0 to 5 | 10.3 |
| 9.5/V.90 (retrain references inside V.90 9.2) | Read as V.92 9.7 | 11.4, N-19 |
| 6.4.2 and 8.8.3/V.92 (Table 18) | Precoder/prefilter structure; LZ/LP limits; G normalisation | 8 |
| 8.4.4/V.90 (via 8.6.7/V.92) | Sd uses Ucode 16 + U_INFO | 11.5 |
| 10.1.3.5/V.34 (via 8.5.3/V.92) | MD is optional and manufacturer-defined; length 0 if absent | 11.5 |
| 8.2.3.1/V.90 (from 8.9.2/V.92, MH) | Same DPSK modulator and CRC used for modem-on-hold | 3.4 |

---

## 15. Test vectors (computed with the Figure 14 algorithm; not from the Recommendation)

- I computed these in this session with an independent script:
  `C:\Users\Gaming\AppData\Local\Temp\claude\F--dialupmodem2\88010855-0d3a-43e6-94b3-c299dc9e4360\scratchpad\v92\phase2sig\check_vectors.py`.
  The script is scratch and not in the repo.
- The same code gives 0x6F91 for "123456789" and a zero residue on random frames.
- Bit strings are in transmission order (bit 0 first), in groups of 10. The CRC is shown as the
  register value; its bit 0 is sent first.
- They match the vectors an earlier pass wrote, and the Table 19 vector in
  `spec-phase2-procedures.md` (0x7A52), which I re-derived as well.

```
INFO0a (Table 16): 12..20 = 1, 21:23 = 5, 24 = 0, 25 = 1, 26 (V.92) = 1, 27 (short P2) = 1, 28 = 0
  49 bits, CRC = 0xAF5A
  1111011100 1011111111 1101011100 1011010111 101011111

INFO0d (Table 15): 12..19 = 1, 20 = 0, 21:23 = 5, 24 = 0, 25 = 1, 26 (short P2) = 1, 27 (V.92) = 1,
  28 = 0, 29:32 = 3 (-9 dBm0), 33:37 = 23 (-12 dBm0), 38 = 1, 39 = 0 (mu-law), 40 = 1, 41 = 0
  62 bits, CRC = 0xDB49
  1111011100 1011111111 0101011101 1001110110 1010010010 1101101111 11

INFO1d (Table 17): 12:14 = 2, 15:17 = 1, 18:24 = 0,
  2400: carrier 1, pre 3, rate 10;  2743: 0, 2, 11;  2800: 0, 2, 11;
  3000: 1, 4, 13;  3200: 1, 5, 14;  70 (PCM upstream) = 1;  3429: pre 6, rate 13;
  79:88 = -3 (-0.06 Hz)
  109 bits, CRC = 0x086C
  1111011100 1001010000 0000011100 0101001001 1010010011 0110010101 1110100111 1011010111
  0111111110 0110110000 100001111

INFO1a (Table 18): 12:13 = 3, 14:15 = 1 (Ltot 256), 16:17 = 0 (Lmax 128), 18:24 = 4 (138 ms),
  25:31 = 90, 32:33 = 0, 34:36 = 6, 37:39 = 6, 40:49 = all ones
  70 bits, CRC = 0xA858
  1111011100 1011100000 1000001011 0100011011 1111111111 0001101000 0101011111

INFO1a (Table 19): 12:17 = 0, 18:24 = 0, 25:31 = 90, 32 = 0, 33 = 1 (high carrier),
  34:36 = 4 (3200), 37:39 = 6, 40:49 = -512
  70 bits, CRC = 0x6742
  1111011100 1000000000 0000001011 0101001011 0000000001 0100001011 1001101111

INFO1a (Table 19), the vector in spec-phase2-procedures.md: 25:31 = 77, 33 = 1, 34:36 = 5 (3429),
  40:49 = -512, all else 0
  70 bits, CRC = 0x7A52
  1111011100 1000000000 0000001011 0010110101 1000000000 1010010100 1011110111
```

---

## 16. Implementation notes

### 16.1 What is new relative to V.90

1. **Two new INFO0 flags in each direction:** V.92 capability, and a request for short
   Phase 2. **INFO0d and INFO0a swap the order of these two bits.**
2. **Short Phase 2** (9.4):
   - no probing and no INFO1d;
   - the digital modem reverses first, and the analogue modem answers at 40 ± 1 ms and sends
     INFO1a straight after;
   - every deadline is a flat 2500 ms, with no RTD term.
3. **INFO1d bit 70** now tells whether the channel can carry PCM upstream. The 3429 field is
   8 bits, and its bits have not moved.
4. **Table 18 INFO1a (PCM upstream)**, recognised by **34:36 = 6**. It carries
   precoder/prefilter limits (12:17) and an MD length in 276-symbol steps. Bits 40:49 are
   ones.
5. **Table 19 INFO1a** (short Phase 2, V.90 data mode) carries the upstream carrier in bit 33.
6. After a V.92/V.92 start-up, **every retrain uses V.92's Phase 2**. That means the Table 17
   INFO1d and the Table 18 option.
7. When both modems are V.92 and both indicated LAPM in V.8 or V.8 bis, the **ODP/ADP exchange
   is skipped** (9.3.1).

### 16.2 Pitfalls, including ones specific to this codebase

- **N-1: the INFO0d encoder zeroes the new bits.**
  - `crates/datapump/src/v34/info.rs`, `Info0d::to_bits`, sets `info[14]` and `info[15]`
    (bits 26 and 27) to false on purpose, because V.90 reserves them.
  - For V.92, bit 26 must carry the short Phase 2 request and bit 27 the V.92 capability.
- **N-2: the INFO0d decoder drops them.**
  - `Info0d::from_bits` sets `clock: 0` and discards bits 26 and 27.
  - Expose them, for example as `short_phase2_request` and `v92`.
- **N-3: INFO0a shares the V.34 `Info0` struct.**
  - `Info0` stores bits 26:27 as the V.34 `clock` field.
  - A V.92 INFO0a therefore decodes as clock = 1, or 3 with the short Phase 2 request.
  - In V.90/V.92 Phase 2, read INFO0a bits 26 and 27 as flags, never as a clock source.
  - Our own INFO0a is built in `crates/datapump/src/v34/phase2.rs` (around line 502,
    `clock: 0`), which today means "not V.92, no short Phase 2".
  - Give V.92 its own view of these bits rather than overloading `clock`.
- **N-4: reusing INFO1c as INFO1d keeps the bits but not the meaning.**
  - `Info1c` doubles as INFO1d. `probed[5].high_carrier` is bit 70, which is the PCM upstream
    flag in V.92.
  - Treat bit 70 as "PCM upstream OK" **only** when INFO0d bit 27 = 1 **and** our INFO0a bit
    26 = 1.
  - A V.90 digital modem may send any value there, because for it bit 70 is a 3429 carrier
    flag that carries no information.
  - The V.92 digital-modem side (`crates/datapump/src/v90/server.rs` is our test server) must
    write bit 70 from its own PCM-upstream decision when both sides are V.92, and a carrier
    flag otherwise.
- **N-5: a Table 18 INFO1a is discarded today.**
  - `Info1a::from_bits` rejects 37:39 = 6.
  - `Info1aPcm::from_bits` rejects 34:36 outside 3 to 5.
  - So `dpsk.rs::decide` silently drops a valid Table 18 frame with a good CRC.
  - Add a third variant, chosen by the table in section 10.4.
  - `Info1aPcm::to_bits` writes 32:33 = 0. Table 19 needs bit 33, and a Table 19 frame parsed
    by `Info1aPcm::from_bits` loses the carrier choice.
- **N-6: bit 70 = 1 permits PCM upstream; it does not require it.** The analogue modem
  **[may]** still choose Table 10 (V.90 upstream) or Table 11 (V.34).
- **N-7: there are two MD-length units.**
  - Table 18: n × 276 symbols (34.5 ms).
  - Everything else: n × 35 ms.
  - Maxima: 127 × 34.5 = 4381.5 ms; 127 × 35 = 4445 ms.
  - Section 11.5 limits the useful Table 18 value to n ≤ 107 when RTD is taken as 0.
- **N-8: do not read Table 18 bits 40:49 as an offset.** They are all ones, which a generic
  offset parser (`offset_from`) would turn into −0.02 Hz.
- **N-12: short Phase 2's 2500 ms deadlines have no RTD term.** On Rory's VoIP path (round
  trip about 1.5 to 1.6 s; memory note "VoIP line round trip") the margins are thin:
  - **A reversal.** It reaches the digital modem about RTD + 40 ms ≈ 1.6 s after the B
    reversal, leaving about 0.9 s.
  - **INFO1a.** It ends about RTD + 40 + 10 + 117 ms ≈ 1.77 s after the B reversal, leaving
    about 0.73 s.
  - **Analogue side.** The B reversal arrives about RTD + (the far end's Tone A detection) +
    (≥ 50 ms of B) + (INFO0 recovery, if any) after our INFO0a ends. That may use most of the
    2500 ms.
  - Keep detectors fast, and measure real start-ups against these deadlines.
  - Do not trim the ≥ 50 ms tone or the 10 ms tails. A period one end waits out and the other
    must beat is a comparison both sides make.
- **N-13: retrains do not resend INFO0.**
  - Keep the V.92 flags from the first exchange across retrains (P2-17).
  - `phase2::Modem::v90_retrain(pcm, fs, far: Info0, far_info0d: Option<Info0d>)` already
    receives the far INFO0s. Make sure they carry the V.92 bits, and that our own flags are
    kept too.
- **N-14: Tone A level versus INFO level.**
  - The repo's DPSK modulator (`dpsk.rs`, `guard_amplitude` = −7 dB) produces Tone A with the
    INFO levels: carrier −1 dB, guard −7 dB.
  - 10.1.2.1/V.34, as printed, puts Tone A's guard **at nominal**.
  - This is not new in V.92, and the existing V.34/V.90 code works with live servers.
  - Record the discrepancy, and check it against a real capture before changing anything.
  - A continuous Tone A followed by INFO1a (short Phase 2) would make the guard step from
    nominal to −7 dB if the printed text were followed literally.
- **N-15: the frame sync is not LSB-first.** It is the time pattern 0,1,1,1,0,0,1,0 (clause 8
  preamble: bit patterns go leftmost first). Integer fields are LSB-first.
- **N-16: existing receivers tell sequences apart by length.** INFO0 is 49 bits, INFO0d 62,
  INFO1c/INFO1d 109, and every INFO1a 70. Keep that design (`dpsk.rs::Side::lengths`) and add
  the 34:36 = 6 branch plus the Table 19 bit 33.
- **N-23: reversal timing is finer than a DPSK symbol.**
  - 40 ± 1 ms is tighter than one 600 bit/s symbol (1.667 ms). So the answering reversal (and
    the INFO1a that follows it in short Phase 2) must be placed with sub-symbol accuracy.
  - Restart or align the symbol clock at the reversal rather than waiting for the next symbol
    boundary.
  - 10 ms is exactly 6 symbols, so INFO1a can start on the symbol clock of the reversal. The
    last A-bar symbol then serves as the arbitrary-phase reference point (P2-04).
- **N-24: U_INFO range.**
  - > 66 is required by the tables. Sd needs Ucode 16 + U_INFO ≤ 127, so use **67 to 111**.
  - The power check against INFO0d 33:37 is done today by `v90::training_codeword`, which uses
    the Table 15/V.90 power ceilings. V.92 Tables 18/19 give only the qualitative rule, so
    reuse that function.
- **N-25: the CME and access bits change no behaviour.** Bit 24 and the V.8 access octet are
  **[may]**. Log them for diagnostics.
- **N-26: invalid flag combinations.** A modem sending the short Phase 2 request with its V.92
  bit clear is not conforming. P2-16 still gives the right answer (full Phase 2), so do not
  special-case it.

### 16.3 Ambiguities and open questions

- **N-9: INFO1a layout in a V.92/V.92 full Phase 2 when PCM upstream is not chosen.**
  - 9.3 mentions only Table 18. Table 19 is labelled "during short Phase 2". 9.3 says the
    procedures are those of V.90, so V.90 Table 10 (V.90 mode) and Table 11 (V.34 mode) remain
    available.
  - [suggestion] As the analogue modem, send Table 10 with bit 33 = 0.
    - A V.90 reader ignores bits 32:33 anyway.
    - Setting bit 33 to INFO1d's carrier choice would also be harmless, but it is reserved in
      Table 10, so send 0 as Table 10 requires.
  - [suggestion] As the digital modem in full Phase 2, take the carrier and pre-emphasis from
    INFO1d, per Table 10, and ignore bit 33.
  - Question: acceptable?
- **N-10: short Phase 2 has no INFO1d, so the analogue modem is never told:**
  - (a) whether the channel supports PCM upstream. Table 18 is forbidden "if bit 70 of INFO1d
    is clear", and no INFO1d is sent;
  - (b) which upstream pre-emphasis filter to use, which Table 19 does not carry;
  - (c) which upstream rate and carrier suit the line.

  The Recommendation is silent on all three.
  - Scope item j) says "reduced start-up time on recognized connections". So the likely intent
    is that short Phase 2 is used on repeat calls (after short Phase 1, whose QTS carries
    U_QTS), where the analogue modem remembers the line from an earlier full start-up.
  - [suggestion] Use values remembered from the last full start-up with the same server:
    bit 70, the chosen rate, carrier and pre-emphasis. Failing that, use pre-emphasis index 0,
    3200 symbols/s, and the high carrier only if INFO0d bits 17/18 permit it. Let the
    Phase 3/4 retrain path correct a bad guess.
  - Question: in short Phase 2, should V.90-mode upstream use pre-emphasis index 0? Or does
    the digital modem not care, because its equaliser trains in Phase 3?
- **N-11: short Phase 2 gives the analogue modem no RTD estimate**, because the digital modem
  reverses first and there is no second exchange.
  - Several analogue-side Phase 3/4 deadlines contain RTD, for example 9.6.2.2.1 (20 s +
    6 RTD) and 9.5.2.1.2 (MD start to TRN1u end ≤ RTD + 4000 ms).
  - [suggestion] Use an RTD remembered from an earlier full Phase 2 with the same server, or
    else a conservative default: 0 for "must not exceed" limits, and a large value such as
    1.6 s on this rig for "wait at least" timeouts.
  - Question: does anything in V.92 Phase 3 give the analogue modem an RTD?
- **N-17: frequency offset in Table 19.**
  - Bits 40:49 are defined on "the probing tones", and short Phase 2 has none.
  - [suggestion] Send **−512** (ignore), unless the offset is known from somewhere else.
    Tones A and B are short and phase-reversed, so meeting the 0.25 Hz accuracy on them is
    unrealistic.
  - As a receiver, honour −512.
- **N-18: "proceed with the full Phase 2 procedure" (9.4.1.2.2 and 9.4.1.2.3) and "the
  appropriate Phase 3" (9.4.x.1.x) name no entry points.**
  - The actions in 9.4.1.2.2 and 9.4.1.2.3 (look for Tone A, send Tone B, look for the A
    reversal) match V.90 9.5.1.2, which enters at **9.2.1.1.3**.
  - [suggestion] Enter the full Phase 2 state machine there on the digital side. On the
    analogue side, enter per the 9.7.2.x retrain it runs.
  - For Table 19 the "appropriate Phase 3" is presumably V.90 9.3 (S/S̄, MD, PP, TRN, Ja). V.92
    does not say so explicitly.
  - Question: confirm.
- **N-19: V.90's retrain references inside 9.2** (9.2.1.2.5, 9.2.1.2.6 and 9.2.2.2.4 cite V.90
  9.5.x) should be read as V.92 9.7.x when running V.92. The wording is identical, and so are
  the entry points.
- **N-20: INFO0d bit 20 still says "in V.34 mode" only, while INFO0a bit 20 now covers V.90
  mode as well.**
  - The likely reason is that the digital modem's PCM power is set elsewhere (INFO0d 29:37,
    CPu).
  - [suggestion] Treat INFO0d bit 20 as governing only the V.34-mode fields (Table 11 bits
    12:17).
- **N-21: should a short Phase 2 request depend on having run a short Phase 1?** 9.4 does not
  tie the two together.
  - [suggestion] As the analogue modem, request short Phase 2 only when we have remembered
    line data (N-10).
  - As the digital modem, honour the request whenever all four bits are set.
- **N-22: U_INFO ≤ 66 or > 111 received.** The digital modem's behaviour is not specified.
  - [suggestion] Treat the INFO1a as invalid (not received). That leads into the 9.2.1.2.6 or
    9.4.1.2.3 recovery.
