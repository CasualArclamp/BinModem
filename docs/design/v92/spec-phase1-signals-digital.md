# V.92 short Phase 1: the digital modem's signals (clause 8.3)

Implementation digest of ITU-T V.92 (11/2000), clause 8.3, "Short Phase 1 signals and sequences
for the digital modem". It covers ANSpcm, QC1d, QC2d, QCA1d, QCA2d, QTS and QTS\\, with enough of
the surrounding text (clause 8 conventions, 8.2 and 9.2) to build and test them.

## Sources and how they were read

| Document | Rendered pages viewed (PDF page numbers) | Used for |
|---|---|---|
| V.92 (T-REC-V.92-200011-I.pdf) | **16, 17, 18, 19, 20, 21, 22, 23** (clause 8.3, printed pp. 9-16) | the subject of this digest |
| V.92 | 13 (clause 8 conventions, 8.1), 14-16 (8.2: QC1a, QC2a, QCA1a, QCA2a, TONEq) | bit-order rule, U_QTS table, the signals the digital modem receives |
| V.92 | 46, 47 (Figures 3-6), 48 (Figures 7-8, 9.2.1 to 9.2.1.2), 49, 50 (9.2.1.3 to 9.2.5) | how the signals are sequenced and timed |
| V.92 | 63 (9.8.1.1.3) | how the digital modem makes silence |
| V.90 (T-REC-V.90-199809-I.pdf) | 11, 12 (Table 1, Ucodes), 20 (8.1 Phase 1 power), 29 (8.4.4 Sd) | Ucode-to-octet mapping, levels, the Sd model QTS follows |
| V.8 (11/2000) | 9 (clause 5, Table 1 preamble), 10 (Tables 2-3), 12 (Tables 5-6) | V.8-type 10-bit framing, the `0101010101` sync, the pcm0 and prot0 octets |
| V.8 bis (11/2000) | 13, 14 (7.1 signals, 7.2 messages), 15 (format convention), 16 (preamble, flags, FCS, transparency), 19, 20 (8.2.3, 8.3, Table 3 message type) | QC2d/QCA2d framing, message type 1011, revision field |
| V.21 (1984) | 3, 4 | V.21(L)/V.21(H) frequencies and tolerances |

Every table, bit layout and number below was read from a rendered page. The extracted text in
`docs/specs/text/` was used only to find things. **Warning:** in the extracted text of V.92 Table 6,
the `scl` column is scrambled. It shows 1000 on the -9.5 dBm0 row, 667 on the -12 dBm0 row, and so
on. Use the values below, which come from the rendered page and are confirmed by §3.4.

ANSpcm Tables 7-10 were checked three ways:
1. A generator (§3.4) reproduces all 2408 octets in the extracted tables.
2. 16 cells spread across all four rendered tables match the generator. These include the cell
   printed as "8" (Table 7, k = 82, A-law), which is 0x08.
3. The first rows of every table were compared with the page images by eye.

Scripts: `scratchpad/v92/p1d_*.py`, outside the repository.

Tag legend:

| Tag | Meaning |
|---|---|
| **[SHALL]** | normative requirement in the Recommendation |
| **[MAY]** / **[SHOULD]** | permitted option / recommendation |
| **[INFO]** | descriptive statement in the Recommendation |
| **[DERIVED]** | computed or deduced by this digest's author, not stated in the text; how it was checked is given |
| **[INTERP]** | this digest's reading of text that is unclear |
| **[AMBIG]** | open question, collected again in the Implementation notes |

---

## 0. Summary card

| Signal | Sent by | When (9.2) | Line format | Length | Content |
|---|---|---|---|---|---|
| **QC1d** | digital *call* modem, V.8 case | after ANSam has been detected for 1 s | V.21(L), 300 bit/s, V.8-type 10-bit frames | 60 bits = 200 ms, sent once | sync `0101010101`, digital/QC flags, P (LAPM), LM (ANSpcm level); **CM follows immediately** |
| **QC2d** | digital *call* modem, V.8 bis case | after the first 50 ms of CRe | V.21(H), V.8 bis message (clause 7/V.8 bis) | preamble 100 ms plus at least 56 framed bits (286.7 ms or more) | identification field: type `1011`, revision, LM, P, QC, digital |
| **QCA1d** | digital *answer* modem, V.8 case | on QC1a | V.21(H), V.8-type frames | 70 bits = 233.3 ms, sent once | as QC1d but with the QCA flag, plus ten trailing ONEs |
| **QCA2d** | digital *answer* modem, V.8 bis case | on QC2a | V.21(L), V.8 bis message | as QC2d | as QC2d with the QCA flag |
| **QTS** | digital modem (either role) | after 75 ± 5 ms of silence | PCM codewords, 8000 symbols/s | 768 symbols (96 ms) | 128 × {+V, +0, +V, -V, -0, -V}, V = Ucode U_QTS |
| **QTS\\** | digital modem | right after QTS | PCM | 48 symbols (6 ms) | 8 × {-V, -0, -V, +V, +0, +V} |
| **ANSpcm** | digital modem | right after QTS\\ | PCM | until TONEq is detected (or a timeout, see §10) | fixed 301-symbol codeword period (about 2099.67 Hz), polarity reversed every 3612 symbols, one of 4 levels |

The digital modem **receives** QC1a, QCA1a, QC2a, QCA2a and TONEq. These are defined in 8.2 and
summarised in §9 because the digital modem has to parse them. U_QTS comes from them.

---

## 1. General conventions that bind these signals

- **G-1 [INFO] (clause 8).** All PCM codewords in training sequences are described by the universal
  codes (Ucodes) of V.90 Table 1.
- **G-2 [SHALL, bit order] (clause 8).** In V.92 Tables 2-5, 11-24, 27 and 30-33, unless stated
  otherwise:
  - values given as bit patterns are transmitted **leftmost bit first in time**;
  - values given as integers are transmitted **least significant bit first**.

  Tables 11-14 (this digest) are covered. So in `000LM1` bit 24 (`0`) goes out first and bit 29
  (`1`) last, and in the 2-bit field `LM`, **L is transmitted before M**.
- **G-3 [INFO] (8.2, second paragraph).** Short Phase 1 information bits are sent at **300 bit/s**
  on V.21(L), the low-band channel of V.21, or V.21(H), the high-band channel. The sentence sits in
  8.2 but applies equally to the 8.3 signals, which name their channel explicitly.
- **G-4 [INFO] V.21 (clause 3 and footnote 2), from the rendered page.**

  | Channel | Mean | Binary 1 (F_Z, mark) | Binary 0 (F_A, space) |
  |---|---|---|---|
  | No. 1 = V.21(L) | 1080 Hz | 980 Hz | 1180 Hz |
  | No. 2 = V.21(H) | 1750 Hz | 1650 Hz | 1850 Hz |

  - Modulator output within ±6 Hz of nominal.
  - The demodulator must tolerate ±12 Hz.
  - V.8 bis 7.2 tightens the tolerance to **±0.01%** for V.8 bis messages, so QC2d and QCA2d
    should meet it.
- **G-5 [INFO] Ucodes (V.92 3.6; V.90 3.6, Table 1).**
  - The μ-law and A-law columns of V.90 Table 1 are the octets handed to the digital interface.
    Every G.711 bit inversion has already been applied, and the MSB is the polarity bit.
  - [DERIVED; checked against V.90 Table 1 rows 0, 61, 87, 127 and the repo's `ucode.rs` test]
    For Ucode U:

    | | positive | negative |
    |---|---|---|
    | μ-law octet | `0xFF - U` | `0x7F - U` |
    | A-law octet | `(0x80 \| U) ^ 0x55` | `U ^ 0x55` |

  - Polarity bit 1 means positive.
  - Ucode 0 gives these octets: μ-law +0 = `FF`, -0 = `7F`; A-law +0 = `D5`, -0 = `55`.
- **G-6 [INFO] Symbol clock and frames (V.92 clause 5 → V.90 clause 5, 5.4).**
  - The digital modem runs at **8000 symbols/s**.
  - A downstream data frame is **6 symbols**. Its positions are data frame intervals i = 0..5, with
    i = 0 first in time.
- **G-7 [INTERP] Digital modem "silence".**
  - V.92 9.8.1.1.3 (and the matching V.90 rate-renegotiation text) says the digital modem makes
    silence by sending PCM codewords whose magnitude is Ucode 0, and keeps data frame alignment
    while doing so.
  - 8.3 and 9.2 do not define silence. Use the same rule for the 75 ± 5 ms silences of short
    Phase 1 and for silence after ANSpcm.
- **G-8 [INFO/INTERP] Levels of the non-PCM signals.**
  - 8.3 gives no level for QC1d, QC2d, QCA1d, QCA2d, CM, ANSam or CRe.
  - V.90 8.1 **[SHALL]**: Phase 1 signals (V.25/V.8) are sent at the **nominal transmit power**.
  - V.8 bis 7.1.4 **[SHALL]**: CRe is sent **12 to 15 dB below** the nominal permitted transmit
    power.
  - V.8 bis 7.2.1: V.21 messages follow national regulations, with V.2 as the guide.
  - V.21 clause 6: at most 1 mW, and no more than -13 dBm0 at the international circuit input.
  - [INTERP] Send QC1d, QCA1d, QC2d and QCA2d at the nominal transmit power, as for CM and JM.
    ANSpcm and QTS have levels fixed by their definitions.

---

## 2. Clause 8.3: scope

- **8.3-R1 [INFO].** QC1d and QCA1d are for connections started under **V.8**, where the answer
  modem sends ANSam. QC2d and QCA2d are for connections started under **V.8 bis**, where the answer
  modem sends CRe.
- **8.3-R2 [DERIVED] Channel choice.** The channels match the parent Recommendations:
  - V.8 case: the caller's QC1d uses V.21(L) like CM, and the answerer's QCA1d uses V.21(H) like
    JM.
  - V.8 bis case: at call establishment the *answer* modem sends CRe, an initiating signal
    (V.8 bis Table 1). That makes it the V.8 bis initiating station, and initiating stations send
    messages on V.21(L) (V.8 bis 7.2). So QCA2d, from the answerer, is on V.21(L), and QC2d, from
    the caller, which is the responding station, is on V.21(H).

---

## 3. Clause 8.3.1: ANSpcm

### 3.1 Definition

- **ANS-1 [INFO].** ANSpcm is a repeating sequence of PCM codewords that makes a tone of about
  2100 Hz.
- **ANS-2 [INFO].** The sequence **repeats every 301 symbols**, and a **phase reversal is added
  every 3612 symbols**.
- **ANS-3 [MAY].** The codeword sequence may be used to check that assumed channel characteristics
  are correct. The receiver can do this because it knows exactly which codewords were sent: the
  level is announced in LM.
- **ANS-4 [SHALL].** ANSpcm **shall** be sent at one of the four levels of Table 6.
- **ANS-5 [SHALL].** Whatever generation method is used, the output **shall equal Tables 7-10**,
  choosing the table by `scl` (that is, by level).
- **ANS-6 [INFO] NOTE.** Some network equipment is known to change the channel's characteristics in
  response to ANSpcm.
  - [INTERP] A 2100 Hz tone with a reversal about every 450 ms is the classic signal for disabling
    network echo control. G.164/G.165 are not in `docs/specs`, so this is not verified.
  - Practical consequence: the channel the analogue modem measures during and after ANSpcm may
    differ from the one before it.

### 3.2 Table 6/V.92: generation parameters (rendered p.16)

| Transmit level | LM code (Tables 11-14) | scl, μ-law | scl, A-law | θ | Octet table |
|---|---|---|---|---|---|
| -9.5 dBm0 | `00` | 1334 | 667 | 0.25 × π / 301 | Table 7 |
| -12 dBm0 | `01` | 1000 | 500 | 0.25 × π / 301 | Table 8 |
| -15 dBm0 | `10` | 708 | 354 | 0.25 × π / 301 | Table 9 |
| -18 dBm0 | `11` | 500 | 250 | 0.25 × π / 301 | Table 10 |

### 3.3 The equation in 8.3.1

- **ANS-7 [MAY].** The Recommendation calls it "the 301 symbol Ucode sequence" and says it may be
  generated by:

  `x = floor( scl × √2 × cos( 2π·k·79/301 + θ ) + 0.5 )`, for k = 0, 1, …, 300

  and then quantising x to a linear PCM value according to G.711.
- **ANS-8 [INFO] Table layout.** Tables 7-10 have, for each k = 0..300, a **μ** column and an **A**
  column of two-digit hex values.
  - [DERIVED] These are the **G.711 octets as sent**, in V.90 Table 1 form, **not** Ucodes, despite
    the wording "Ucode sequence".
  - Check: Table 7, k = 0 is μ `A1`. That is positive Ucode 94 (`0xFF - 0xA1 = 0x5E = 94`), whose
    G.711 interval starts at exactly 1887 = x(0).

### 3.4 Exact quantiser that reproduces Tables 7-10 [DERIVED, fully verified]

G.711 is not in `docs/specs`, so the rule below was pinned down by matching the tables.

1. **Scale.** x is on the native G.711 scale: μ-law magnitudes up to 8159, A-law up to 4096. V.90
   Table 1 prints μ-law linear values at 4× and A-law at 8× that scale. The 2:1 ratio between the μ
   and A `scl` values reflects the two native scales.
2. **Rounding.** Compute x with **floor(v + 0.5)**, where v is the real value inside the floor,
   using **f64**.
   - The closest any v comes to a .5 rounding boundary is 1.1 × 10⁻⁴, so f64 is safe. f32 is
     marginal at magnitudes near 1900.
   - Truncating instead of floor(v + 0.5) changes 9, 5, 10 and 12 μ-law octets in Tables 7, 8, 9
     and 10.
3. **Sign and magnitude.** m = |x|; the codeword is negative when x < 0. x is never 0: the smallest
   |x| is 5/4/3/2 for μ-law and 2/2/1/1 for A-law across Tables 7-10.
4. **Magnitude to Ucode (G.711 decision intervals, lower decision value inclusive).** A value
   exactly on a decision value goes to the **louder** interval. Closed forms, checked for every m:
   - **μ-law** (m ≤ 8158): `b = m + 33; c = floor(log2 b) - 5; s = (b >> (c + 1)) & 15; U = 16c + s`.
   - **A-law** (m ≤ 4095): if `m < 32`, `U = m >> 1`; otherwise
     `c = floor(log2 m) - 4; s = (m >> c) & 15; U = 16c + s`.
   - As lower bounds:
     - μ-law: Ucode 0 is [0, 1); in chord 0, step s ≥ 1 starts at 2s - 1; in chord c ≥ 1 the lower
       bound is (2^(c+5) - 33) + s·2^(c+1).
     - A-law: chord 0 lower bound is 2s; chord c ≥ 1 lower bound is 16·2^c + s·2^c.
5. **Ucode to octet.** Use G-5 (`ucode::octet(law, U, x < 0)` in `crates/datapump/src/v90/ucode.rs`
   does exactly this).

**How much the tie rule matters.** Rounding ties *down* instead would get these many entries wrong:

| Table | μ-law | A-law |
|---|---|---|
| 7 | 11 | 15 |
| 8 | 9 | 28 |
| 9 | 15 | 35 |
| 10 | 25 | 46 |

**Do not use a nearest-reconstruction quantiser** such as `ucode::nearest()`. It differs at segment
edges and at mid-interval ties, and would get these many entries wrong:

| Table | μ-law | A-law |
|---|---|---|
| 7 | 13 | 16 |
| 8 | 12 | 31 |
| 9 | 33 | 36 |
| 10 | 26 | 48 |

Result: with this rule the generator matches **all 301 × 2 × 4 = 2408** tabled octets. The tables
are reproduced in Appendix A. **Recommended implementation:** embed Appendix A as constant data, and
keep the generator only as a unit test.

### 3.5 Phase reversal, period and frequency

- **ANS-9 [DERIVED] Period and frequency.**
  - 79/301 is in lowest terms (301 = 7 × 43), so the period is exactly 301 symbols = **37.625 ms**.
  - Tone frequency = 8000 × 79/301 = **2099.668 Hz**, inside V.8's ANSam tolerance of 2100 ± 1 Hz
    (V.8 7.2).
- **ANS-10 [DERIVED] Reversal timing.**
  - 3612 = 12 × 301, so every reversal falls at k = 0 of a period.
  - 3612 = 602 × 6, so every reversal also falls on a downstream data frame boundary (and on a
    12-symbol upstream frame boundary).
  - The reversal interval is 3612/8000 = **451.5 ms**, inside V.8's 450 ± 25 ms.
- **ANS-11 [DERIVED] How to apply a reversal.** A phase reversal of a sampled tone negates every
  sample, which here means **flipping the polarity bit** (`octet ^ 0x80`) in both laws. A-law's
  `0x55` mask does not touch bit 7.
  - Check: for every tabled sample, quantise(-v) equals -quantise(v). No v lies on a .5 boundary.
  - Adding π inside the cosine gives the identical codeword sequence.
- **ANS-12 [INTERP] Symbol n of ANSpcm** (n = 0 is the first ANSpcm symbol) is:

  `octet(n) = T[n mod 301] XOR (0x80 if floor(n / 3612) is odd, else 0)`

  where T is the table for the chosen law and level.
  - **[AMBIG]** The text does not say whether the first reversal comes after a full 3612 symbols
    (assumed here) or at some other offset.
- **ANS-13 [DERIVED] No amplitude modulation.** Unlike ANSam (15 Hz AM, envelope 0.8 to 1.2),
  ANSpcm has a constant envelope. Spectrally it is "ANS with phase reversals", not ANSam. A
  detector that needs the 15 Hz AM will not report ANSam on ANSpcm, and should not.

### 3.6 Level selection and signalling

- **ANS-14 [SHALL, via Tables 11-14].** The digital modem announces the ANSpcm level in field LM of
  its own QC1d, QCA1d, QC2d or QCA2d: `00` = -9.5, `01` = -12, `10` = -15, `11` = -18 dBm0.
- **ANS-15 [INTERP].** The ANSpcm actually sent **shall** use the level announced in LM. The field
  is defined as "Level of ANSpcm", and the analogue modem needs it for ANS-3.
- **ANS-16 [AMBIG].** V.92 does not say how to pick the level. A reasonable policy is the level
  nearest the configured nominal transmit power, clamped to [-18, -9.5] dBm0. V.90 INFO0d allows a
  nominal power of -6 to -21 dBm0.
- **ANS-17 [DERIVED] Measured power of the tabled sequences.** Computed from G.711 reconstruction
  values. The reference is that a full-scale sine wave (peak 8159 for μ-law, 4096 for A-law) is
  +3.17 dBm0 (μ) or +3.14 dBm0 (A). That is general G.711 knowledge, not in `docs/specs`.

  | Level | μ-law | A-law |
  |---|---|---|
  | -9.5 dBm0 | -9.56 | -9.60 |
  | -12 dBm0 | -12.04 | -12.10 |
  | -15 dBm0 | -15.01 | -15.10 |
  | -18 dBm0 | -18.06 | -18.11 |

  This confirms that `scl` is the RMS value on the native G.711 scale.

### 3.7 Alignment and duration

- **ANS-18 [DERIVED] Alignment.** ANSpcm follows QTS\ directly (8.3.6 and 9.2), so it starts
  816 = 136 × 6 symbols after the first QTS symbol, on **data frame interval 0**. Its reversals stay
  on frame boundaries (ANS-10).
- **ANS-19 [INFO, 9.2] Duration.** 8.3.1 sets no length. ANSpcm is sent until TONEq is detected
  (9.2.2.3, 9.2.4.3), or until the 2 s answer-side timeout in 9.2.4.3 (§10).
  - [INTERP] ANSpcm may stop part-way through a period.
  - The silence that follows is Ucode-0 codewords, and data frame alignment is kept (G-7, QTS-6).

---

## 4. Clause 8.3.2: QC1d

### 4.1 Text

- **QC1d-1 [INFO/SHALL].** QC1d is a bit sequence sent with **V.21(L)** modulation at 300 bit/s. It
  consists of **10-bit frames with V.8-type formatting** as defined in Table 11.
- **QC1d-2 [SHALL].** QC1d is **sent once** and is **followed immediately by CM**.

### 4.2 Table 11/V.92: definition of QC1d (rendered p.21)

| Bit position | Content | Meaning |
|---|---|---|
| 0:9 | `1111111111` | ten ONEs |
| 10:19 | `0101010101` | synchronisation sequence (the V.8 Table 1 row "Defined in ITU-T V.92") |
| 20 | `0` | start bit |
| 21 | `1` | indication for **digital** modem |
| 22 | `0` | indication for **QC** |
| 23 | `P` | 1 = calls for LAPM according to V.42 (see 9.2.5) |
| 24:29 | `000LM1` | bits 24, 25, 26 = `0`; bit 27 = **L**; bit 28 = **M**; bit 29 = `1` (stop bit). LM = level of ANSpcm: `00` -9.5, `01` -12, `10` -15, `11` -18 dBm0 |
| 30:39 | `1111111111` | ten ONEs |
| 40:49 | `0101010101` | bits 10:19 repeated |
| 50:59 | `010P000LM1` | bits 20:29 repeated |

Total **60 bits = 200 ms**. Bit 0 is sent first (G-2).

### 4.3 Derived views [DERIVED]

- **V.8 octet view.** Bits 20..29 are one V.8 character: start bit, then b0..b7, then stop bit,
  with **b0 = bit 21** (V.8 clause 5, 5.1). The QC1d information octet is:

  `0x01 | (P << 2) | (L << 6) | (M << 7)`

  | P \ LM | 00 | 01 | 10 | 11 |
  |---|---|---|---|---|
  | 0 | 0x01 | 0x81 | 0x41 | 0xC1 |
  | 1 | 0x05 | 0x85 | 0x45 | 0xC5 |

  - V.8 5.1 requires b4 = 0 (bit 25) to prevent flag simulation, and QC1d meets this.
  - The octet is not a meaningful V.8 category. The `0101010101` sync is what marks the frame as V.92.
- **Complete bit strings** (bit 0 first, grouped by ten):

  ```text
  P=0 LM=00: 1111111111 0101010101 0100000001 1111111111 0101010101 0100000001
  P=0 LM=01: 1111111111 0101010101 0100000011 1111111111 0101010101 0100000011
  P=0 LM=10: 1111111111 0101010101 0100000101 1111111111 0101010101 0100000101
  P=0 LM=11: 1111111111 0101010101 0100000111 1111111111 0101010101 0100000111
  P=1 LM=00: 1111111111 0101010101 0101000001 1111111111 0101010101 0101000001
  P=1 LM=01: 1111111111 0101010101 0101000011 1111111111 0101010101 0101000011
  P=1 LM=10: 1111111111 0101010101 0101000101 1111111111 0101010101 0101000101
  P=1 LM=11: 1111111111 0101010101 0101000111 1111111111 0101010101 0101000111
  ```

### 4.4 Requirements

- **QC1d-R1 [SHALL].** Modulate V.21(L) at 300 bit/s: 1 = 980 Hz, 0 = 1180 Hz.
  - [INTERP] Use continuous-phase FSK, as for CM.
  - At 8000 samples/s one bit is 26.667 samples. Use fractional timing (for example 80 samples per
    3 bits, or a phase accumulator), not a rounded 27-sample bit.
- **QC1d-R2 [SHALL].** Send the 60 bits once, never repeated.
- **QC1d-R3 [SHALL].** CM follows immediately: the ten ONEs that start CM come directly after bit 59,
  with no gap and no extra bits.
  - CM is the ordinary V.8 call menu (V.8 7.3): ten ONEs, sync `0000001111`, the call function
    first, then modulation modes and the other categories, repeated.
- **QC1d-R4 [SHALL].** P = 1 if and only if the modem calls for V.42 LAPM. If both modems indicate
  LAPM, the V.42 ODP/ADP exchange **shall** be bypassed (9.2.5).
  - [INTERP] Keep P equal to the LAPM indication in CM's prot0 octet (V.8 Table 6: tag
    b0-b3 = `0101`, then b4 = 0, then b5 b6 b7 = `100`, which calls for LAPM).
- **QC1d-R5 [SHALL, ANS-15].** LM = the level at which this modem will send ANSpcm.
- **QC1d-R6 [INFO, V.8 references for the CM that follows].**
  - A V.90/V.92 digital modem shows PCM capability with the **pcm0** octet (V.8 Table 5): tag
    b0-b3 = `1110`, b4 = 0, **b5 = V.90/V.92 analogue**, **b6 = V.90/V.92 digital**, b7 = V.91.
  - If b5 or b6 is set, the modulation category **shall** be present with the V.34 bit set.
  - When pcm0 is present, the PSTN access category **shall** be present.
  - When pcm0 and the modulation category are both present, b5 of the first modulation octet
    **shall** be 1 (V.8 6.3, 7.3).
  - The V.8 access0 octet has b7 = 1 for "DCE on a digital network connection".
- **QC1d-R7 [SHALL, 9.2.2.1].** If QCA1a is detected, stop CM **without completing the current
  octet**. This differs from V.8, where the caller finishes the octet and then sends CJ.

---

## 5. Clause 8.3.3: QC2d

### 5.1 Text

- **QC2d-1 [SHALL].** QC2d is a bit sequence sent with **V.21(H)** modulation.
- **QC2d-2 [SHALL].** It uses the **signal structure of clause 7/V.8 bis** and the **information
  field structure of clause 8/V.8 bis**.
- **QC2d-3 [SHALL].** The digital modem **shall** encode the identification field as in Table 12.

### 5.2 Table 12/V.92: identification field of QC2d (rendered p.21)

| Bit position | Content | Meaning |
|---|---|---|
| 0:3 | `1011` | message type |
| 4:7 | `VVVV` | V.8 bis revision number (Note) |
| 8:9 | `LM` | level of ANSpcm, coded as in Table 11 (bit 8 = L, bit 9 = M) |
| 10:12 | `000` | reserved for the ITU |
| 13 | `P` | 1 = calls for LAPM (V.42), see 9.2.5 |
| 14 | `0` | **QC** identifier |
| 15 | `1` | **digital** modem |

NOTE (in the table):
- **[INFO]** At the time of publication the V.8 bis revision number is `0100`.
- **[SHALL]** The receiving modem **shall ignore** the revision field.

### 5.3 How the 16 bits map onto V.8 bis octets [DERIVED]

- **V.8 bis conventions (7.2.2, rendered p.14-15).** Octets go out in ascending order. Within an
  octet, **bit 1 is sent first**. For a single-octet field, the lowest-numbered bit is the LSB.
- **Mapping.** With G-2 (leftmost first), V.92 bit n is octet 1, bit n+1 for n = 0..7, and octet 2,
  bit n-7 for n = 8..15.
- **Message type.** V.8 bis Table 3 (rendered p.20) lists "Defined in ITU-T V.92" as bits
  4 3 2 1 = `1 1 0 1`. In transmission order (bit 1 first) that is `1 0 1 1`, which matches Table 12.
  As a value, the low nibble of octet 1 is **0xD**.
- **Revision.** V.8 bis Table 4 gives Revision 2 as bits 8 7 6 5 = `0 0 1 0`. In transmission order
  (bit 5 first) that is `0 1 0 0`, the `VVVV = 0100` of the Note. The high nibble of octet 1 is
  **0x2**.
  - The receiver must ignore this nibble.
  - [INTERP] The transmitter sends `0100`, the value the Note gives.
- **Octet values** (bit 1 = LSB):
  - octet 1 = **0x2D**
  - QC2d octet 2 = `0x80 | L | (M << 1) | (P << 5)`
  - QCA2d octet 2 = `0xC0 | L | (M << 1) | (P << 5)`

  | P \ LM | 00 | 01 | 10 | 11 |
  |---|---|---|---|---|
  | QC2d, P=0 | 2D 80 | 2D 82 | 2D 81 | 2D 83 |
  | QC2d, P=1 | 2D A0 | 2D A2 | 2D A1 | 2D A3 |
  | QCA2d, P=0 | 2D C0 | 2D C2 | 2D C1 | 2D C3 |
  | QCA2d, P=1 | 2D E0 | 2D E2 | 2D E1 | 2D E3 |

### 5.4 Complete message on the wire (V.8 bis clause 7 applied)

1. **Preamble [SHALL, V.8 bis 7.2.4].** 100 ms ± 2% of continuous V.21 **marking** frequency: 1650 Hz
   on V.21(H), which is 30 ONE bits at 300 bit/s.
2. **Opening flags [SHALL, 7.2.5].** At least 2 and at most 5 HDLC flags, `01111110` (0x7E).
3. **Information field [SHALL, 7.2.6, clause 8].** A whole number of octets: octet 1, then octet 2
   (§5.3), each LSB first.
   - **[AMBIG]** V.8 bis 8.1 says an information field is an I field, a standard information field
     (S) and an optional NS field. V.92 defines only the 16-bit I field.
   - Bits 8:15 are a V.92-specific parameter octet and do not follow the V.8 bis 8.2 tree
     delimiting: bit 15 = 0 in the analogue modem's QC2a would mean "more octets follow".
   - [INTERP] Send only the two I-field octets. On receive, accept and ignore any further octets.
4. **FCS [SHALL, 7.2.7].** 16 bits, ISO/IEC 3309 (HDLC) CRC:
   - generator x¹⁶ + x¹² + x⁵ + 1, register preset to all ONEs, one's complement transmitted;
   - computed over the information field without the inserted zeros;
   - the x¹⁵ coefficient is sent first (V.8 bis Figure 3);
   - in the usual reflected implementation (poly 0x8408, init 0xFFFF, final XOR 0xFFFF), send the
     low octet first, LSB first.
   - Receiver check: the remainder is `0001110100001111` (x¹⁵..x⁰), which is 0xF0B8 in the
     reflected register.
   - [DERIVED] Worked values (octet values, bit 1 = LSB, in transmission order):

     | Signal | Information | FCS |
     |---|---|---|
     | QC2d, P=0, LM=00 | 2D 80 | 04 18 |
     | QC2d, P=1, LM=01 | 2D A2 | 14 1A |
     | QCA2d, P=1, LM=00 | 2D E0 | 02 7B |
     | QCA2d, P=0, LM=11 | 2D C3 | 9B 68 |

     Each verified to give the good-FCS residue.
5. **Transparency [SHALL, 7.2.8].** Insert a ZERO after any five consecutive ONEs within the
   information and FCS fields. The receiver removes them.
6. **Closing flags [SHALL, 7.2.5].** At least 1 and at most 3 flags.
7. **Invalid frames (V.8 bis 7.2.9, receive side).** A frame is invalid if it:
   - is not bounded by flags;
   - has fewer than 3 octets between flags;
   - is not a whole number of octets;
   - has a bad FCS.

   A valid QC2x or QCA2x frame has 4 octets between flags.
8. **Length [DERIVED].**
   - Shortest: 30 preamble + 16 flag + 16 information + 16 FCS + 8 flag = 86 bits = **286.7 ms**,
     plus any stuffed zeros.
   - Longest: 30 + 40 + 32 + 24 = 126 bits = 420 ms, plus stuffing.

### 5.5 Requirements

- **QC2d-R1 [SHALL].** V.21(H): 1 = 1650 Hz, 0 = 1850 Hz, 300 bit/s, ±0.01% (V.8 bis 7.2).
- **QC2d-R2 [SHALL].** Build the message as in §5.4, with the identification field of Table 12:
  bit 14 = 0 (QC), bit 15 = 1 (digital), bits 10:12 = 000.
- **QC2d-R3 [SHALL].** LM = the ANSpcm level this modem will use. P = the LAPM request.
- **QC2d-R4 [INFO, 9.2.2.2].** QC2d is followed by **silence**, not by CM.
- **QC2d-R5 [INTERP].** 8.3.3 does not say how many times QC2d is sent. 9.2.2.2 ("transmit
  signal QC2d followed by silence") implies once.

---

## 6. Clause 8.3.4: QCA1d

### 6.1 Text

- **QCA1d-1 [INFO/SHALL].** QCA1d is a bit sequence sent with **V.21(H)** modulation. It consists
  of 10-bit frames with V.8-type formatting as defined in Table 13.
- **QCA1d-2 [SHALL].** QCA1d is **sent once**.

### 6.2 Table 13/V.92: definition of QCA1d (rendered p.22)

| Bit position | Content | Meaning |
|---|---|---|
| 0:9 | `1111111111` | ten ONEs |
| 10:19 | `0101010101` | synchronisation sequence |
| 20 | `0` | start bit |
| 21 | `1` | indication for **digital** modem |
| 22 | `1` | indication for **QCA** |
| 23 | `P` | 1 = calls for LAPM (V.42), see 9.2.5 |
| 24:29 | `000LM1` | 24-26 = `0`; 27 = L; 28 = M; 29 = `1` (stop). LM = ANSpcm level (`00` -9.5, `01` -12, `10` -15, `11` -18 dBm0) |
| 30:39 | `1111111111` | ten ONEs |
| 40:49 | `0101010101` | bits 10:19 repeated |
| 50:59 | `011P000LM1` | bits 20:29 repeated |
| 60:69 | `1111111111` | ten ONEs |

Total **70 bits = 233.3 ms**.

### 6.3 Derived views [DERIVED]

- **Information octet** (b0 = bit 21): `0x03 | (P << 2) | (L << 6) | (M << 7)`.

  | P \ LM | 00 | 01 | 10 | 11 |
  |---|---|---|---|---|
  | 0 | 0x03 | 0x83 | 0x43 | 0xC3 |
  | 1 | 0x07 | 0x87 | 0x47 | 0xC7 |

- **Complete bit strings** (bit 0 first):

  ```text
  P=0 LM=00: 1111111111 0101010101 0110000001 1111111111 0101010101 0110000001 1111111111
  P=0 LM=01: 1111111111 0101010101 0110000011 1111111111 0101010101 0110000011 1111111111
  P=0 LM=10: 1111111111 0101010101 0110000101 1111111111 0101010101 0110000101 1111111111
  P=0 LM=11: 1111111111 0101010101 0110000111 1111111111 0101010101 0110000111 1111111111
  P=1 LM=00: 1111111111 0101010101 0111000001 1111111111 0101010101 0111000001 1111111111
  P=1 LM=01: 1111111111 0101010101 0111000011 1111111111 0101010101 0111000011 1111111111
  P=1 LM=10: 1111111111 0101010101 0111000101 1111111111 0101010101 0111000101 1111111111
  P=1 LM=11: 1111111111 0101010101 0111000111 1111111111 0101010101 0111000111 1111111111
  ```

- **Why the trailing ONEs.** [INTERP] QC1x is followed by CM, whose own ten ONEs close the second
  copy. QCA1x is followed by silence, so it carries its own ten trailing ONEs. They give the
  receiver a clean end marker, and a V.21 demodulator time to settle, after the last information bit.

### 6.4 Requirements

- **QCA1d-R1 [SHALL].** V.21(H) at 300 bit/s (1 = 1650 Hz, 0 = 1850 Hz). Send once.
- **QCA1d-R2 [SHALL, 9.2.4.1].** Follow it with silence of **75 ± 5 ms** (600 ± 40 samples), then
  QTS, QTS\ and ANSpcm.
- **QCA1d-R3 [SHALL].** LM = the ANSpcm level about to be used. P = the LAPM request.
- **QCA1d-R4 [INFO].** The 2 s TONEq timeout of 9.2.4.3 runs from the **end** of QCA1d (§10).

---

## 7. Clause 8.3.5: QCA2d

### 7.1 Text

- **QCA2d-1 [SHALL].** QCA2d is a bit sequence sent with **V.21(L)** modulation.
- **QCA2d-2 [SHALL].** It uses the signal structure of clause 7/V.8 bis and the information field
  structure of clause 8/V.8 bis.
- **QCA2d-3 [SHALL].** "The *analogue* modem shall encode the identification field as defined in
  Table 14."
  - **[INTERP, erratum]** QCA2d is a digital modem signal, and Table 14 sets bit 15 = 1, "Digital
    modem". Read "analogue" as "digital". The sentence was evidently copied from 8.2.4.

### 7.2 Table 14/V.92: identification field of QCA2d (rendered p.22)

| Bit position | Content | Meaning |
|---|---|---|
| 0:3 | `1011` | message type |
| 4:7 | `VVVV` | V.8 bis revision number (Note: `0100` at publication; the receiver **shall** ignore it) |
| 8:9 | `LM` | level of ANSpcm, coded as in Table 11 |
| 10:12 | `000` | reserved for the ITU |
| 13 | `P` | 1 = calls for LAPM (V.42), see 9.2.5 |
| 14 | `1` | **QCA** identifier |
| 15 | `1` | **digital** modem |

Octets (§5.3): `2D`, then `0xC0 | L | (M << 1) | (P << 5)`. The message is built as in §5.4, but
on **V.21(L)**: mark 980 Hz, so the 100 ms preamble is 980 Hz; space 1180 Hz.

### 7.3 Requirements

- **QCA2d-R1 [SHALL].** V.21(L), V.8 bis message structure (§5.4), Table 14 contents.
- **QCA2d-R2 [SHALL, 9.2.4.2].** Send it after terminating CRe. Then send silence of
  **75 ± 5 ms**, then QTS, QTS\ and ANSpcm.
- **QCA2d-R3 [INFO].** The 2 s TONEq timeout of 9.2.4.3 runs from the end of QCA2d.
- **QCA2d-R4 [DERIVED].** The preamble mark is 980 Hz, the same frequency as TONEq (8.2.5) and as
  V.8 bis ESi segment 2. The digital modem's TONEq detector should be enabled only after its own
  QCA2d ends. Echo of its own preamble must not count as TONEq.

---

## 8. Clause 8.3.6: QTS and QTS\\

### 8.1 Text

- **QTS-1 [INFO/SHALL].** QTS is **128 repetitions** of the sequence **{+V, +0, +V, -V, -0, -V}**.
  - V is the PCM codeword whose Ucode is **U_QTS**.
  - 0 is the PCM codeword whose Ucode is **0**.
- **QTS-2 [INFO/SHALL].** QTS\ is **8 repetitions** of **{-V, -0, -V, +V, +0, +V}**.
- **QTS-3 [SHALL].** The first QTS symbol is sent in **data frame interval 0**.
- **QTS-4 [SHALL].** The digital modem **shall keep data frame alignment from this point on**.

### 8.2 Construction details

- **QTS-5 [DERIVED from Figures 3-6] Lengths.** QTS = 128 × 6 = **768 symbols (96 ms)**, labelled
  "768T". QTS\ = 8 × 6 = **48 symbols (6 ms)**, labelled "48T". QTS\ follows QTS with no gap, and
  ANSpcm follows QTS\ with no gap.
- **QTS-6 [DERIVED] Frame positions.** In every 6-symbol frame:

  | Interval | 0 | 1 | 2 | 3 | 4 | 5 |
  |---|---|---|---|---|---|---|
  | QTS | +V | +0 | +V | -V | -0 | -V |
  | QTS\ | -V | -0 | -V | +V | +0 | +V |

  - The QTS-to-QTS\ change is a sign inversion at symbol 768, a frame boundary. [INTERP] The
    analogue modem uses it as the timing mark, exactly as V.90 uses Sd to Sd\\.
  - **Frame counter origin.** The digital modem's downstream frame counter is 0 at the first QTS
    sample and must keep counting through QTS\\, ANSpcm, the silence, all of Phase 2 and beyond.
  - [INTERP] Every later "starts at data frame interval 0" requirement (for example Sd in Phase 3,
    V.92 8.6.7 = V.90 8.4.4) must land on this same grid.
- **QTS-7 [DERIVED] The codewords.**
  - "+0" and "-0" are the positive and negative Ucode-0 codewords, and they are **different
    octets**: μ-law `FF` / `7F`, A-law `D5` / `55`. Do not merge them into one "zero".
  - ±V is Ucode U_QTS with the given polarity (G-5).
- **QTS-8 [INFO, 8.2.1 Table 2 (rendered p.14)] U_QTS.** The analogue modem chooses U_QTS and sends
  it as WXYZ in QC1a, QCA1a, QC2a or QCA2a. The digital modem uses the value from the QC or QCA it
  received: QC1a or QC2a when it answers, QCA1a or QCA2a when it calls.

  | WXYZ | U_QTS | +V μ | -V μ | +V A | -V A | μ linear (V.90) | A linear (V.90) | QTS power μ / A (dBm0) [DERIVED] |
  |---|---|---|---|---|---|---|---|---|
  | 0000 | 61 | C2 | 42 | E8 | 68 | 1756 | 1888 | -20.96 / -20.40 |
  | 0001 | 62 | C1 | 41 | EB | 6B | 1820 | 1952 | -20.65 / -20.11 |
  | 0010 | 63 | C0 | 40 | EA | 6A | 1884 | 2016 | -20.35 / -19.83 |
  | 0011 | 66 | BD | 3D | 97 | 17 | 2236 | 2368 | -18.87 / -18.43 |
  | 0100 | 67 | BC | 3C | 96 | 16 | 2364 | 2496 | -18.38 / -17.97 |
  | 0101 | 70 | B9 | 39 | 93 | 13 | 2748 | 2880 | -17.07 / -16.73 |
  | 0110 | 71 | B8 | 38 | 92 | 12 | 2876 | 3008 | -16.68 / -16.35 |
  | 0111 | 74 | B5 | 35 | 9F | 1F | 3260 | 3392 | -15.59 / -15.31 |
  | 1000 | 75 | B4 | 34 | 9E | 1E | 3388 | 3520 | -15.26 / -14.99 |
  | 1001 | 78 | B1 | 31 | 9B | 1B | 3772 | 3904 | -14.32 / -14.09 |
  | 1010 | 79 | B0 | 30 | 9A | 1A | 3900 | 4032 | -14.03 / -13.81 |
  | 1011 | 82 | AD | 2D | 87 | 07 | 4604 | 4736 | -12.59 / -12.41 |
  | 1100 | 83 | AC | 2C | 86 | 06 | 4860 | 4992 | -12.12 / -11.95 |
  | 1101 | 86 | A9 | 29 | 83 | 03 | 5628 | 5760 | -10.85 / -10.71 |
  | 1110 | 87 | A8 | 28 | 82 | 02 | 5884 | 6016 | -10.46 / -10.33 |
  | 1111 | — | — | — | — | — | — | — | "Cleardown from on-hold state": not a QTS code |

  - The octets and linear values follow from G-5 and V.90 Table 1; rows 61-63 and 87 were checked
    on the rendered Table 1.
  - The power column is 4/6 of the frame at ±V and 2/6 at ±0, against the same 0 dBm0 reference as
    ANS-17.
  - WXYZ order: W is bit 24 of QC1a/QCA1a (bit 25 is a fixed 0), X is bit 26, Y is bit 27, Z is
    bit 28. In QC2a/QCA2a it is bits 8, 9, 10, 11.
- **QTS-9 [DERIVED] Spectrum.** With 0 taken as zero, the 6-symbol pattern has components in
  0-4 kHz only at **1333.3 Hz** (|X| = 2V, one-sided bin) and **4000 Hz** (|X| = 4V). There is no
  DC and nothing at 2666.7 Hz.
  [INTERP] The 4 kHz part is at the codec's Nyquist frequency and is largely removed by the
  decoder's reconstruction filter, so the analogue modem mainly sees a 1333 Hz tone, reversed at
  the QTS\ boundary.
- **QTS-10 [INFO/DERIVED] Relation to V.90 Sd (8.4.4, rendered V.90 p.29).**
  - V.90 Sd: 64 × {+W, +0, +W, -W, -0, -W} with W = Ucode 16 + U_INFO; Sd\ = 8 inverted repeats;
    Sd starts data frame interval 0 and alignment is kept from then on.
  - QTS is the same construction with **128** repeats and V = **U_QTS**.
  - The V.92 Phase 3 Sd (8.6.7) is still the V.90 one.
- **QTS-11 [AMBIG] WXYZ = 1111 outside modem-on-hold.** It has no Ucode. Inside the on-hold state,
  9.10.2.1 says the modem **shall disconnect** on a QC carrying 1111. For a fresh call, the
  behaviour is undefined. Suggest treating it as "no valid QC" and continuing with V.8 or V.8 bis.

---

## 9. What the digital modem receives (8.2, summarised for parsing)

These are not 8.3 signals, but the digital modem has to decode them. Layouts are from the rendered
pp.14-16.

### 9.1 QC1a (V.21(L)) and QCA1a (V.21(H)): V.8-type, sync `0101010101`

| Bits | QC1a | QCA1a |
|---|---|---|
| 0:9 | ten ONEs | ten ONEs |
| 10:19 | `0101010101` | `0101010101` |
| 20 | `0` start | `0` start |
| 21 | `0` analogue | `0` analogue |
| 22 | `0` QC | `1` QCA |
| 23 | P | P |
| 24:29 | `W0XYZ1` (U_QTS) | `W0XYZ1` |
| 30:39 | ten ONEs | ten ONEs |
| 40:49 | `0101010101` | `0101010101` |
| 50:59 | `000PW0XYZ1` | `001PW0XYZ1` |
| 60:69 | (CM follows) | ten ONEs |

QC1a is sent once and followed immediately by CM. QCA1a is sent once.

### 9.2 QC2a (V.21(H)) and QCA2a (V.21(L)): V.8 bis message, identification field

| Bits | Content |
|---|---|
| 0:3 | `1011` |
| 4:7 | VVVV (ignore) |
| 8:11 | WXYZ = U_QTS (Table 2) |
| 12 | `0` reserved |
| 13 | P |
| 14 | `0` QC / `1` QCA |
| 15 | `0` analogue |

### 9.3 TONEq (8.2.5)

A **980 Hz** tone.
- Duration (9.2.1.3, 9.2.3.3): at least 50 ms, and on until ANSpcm is no longer detected.
- Level and frequency tolerance: not specified.

### 9.4 How to tell the frames apart [DERIVED]

| Frame type | Field | Values |
|---|---|---|
| V.8-type (sync `0101010101`, never CM's `0000001111` or CI's `0000000001`) | bit 21 | 0 = analogue, 1 = digital |
| | bit 22 | 0 = QC, 1 = QCA |
| | bits 24:29 | `W0XYZ1` from an analogue modem, `000LM1` from a digital one |
| V.8 bis message type `1011` | bit 14 | 0 = QC, 1 = QCA |
| | bit 15 | 0 = analogue, 1 = digital |

- A V.8 CM/JM parser that accepts any sync after the ten ONEs would misread QC frames. The
  preamble sync must pick the parser.
- **[AMBIG]** No acceptance rule is given, for example whether both copies of bits 20:29 (and
  50:59) must be received and agree. [INTERP] Require both copies to be valid, with start bit 0 and
  stop bit 1, and to agree. This follows V.8's habit of requiring two identical sequences.

### 9.5 What the digital modem listens for

| Role | Case | Listen for |
|---|---|---|
| Answer | V.8 (9.2.4.1) | QC1a (V.21(L)), QC1d (V.21(L), another digital modem), CM |
| Call | V.8 (9.2.2.1) | QCA1a and JM (V.21(H)), ANSam |
| Answer | V.8 bis (9.2.4.2) | QC2a and QC2d (V.21(H)), other V.8 bis signals |
| Call | V.8 bis (9.2.2.2) | QCA2a (V.21(L)), ANSam, ANS |
| Either, after ANSpcm starts | 9.2.2.3, 9.2.4.3 | TONEq (980 Hz) |

---

## 10. How 9.2 uses these signals (digital modem side)

This clause belongs to another digest; it is summarised because 8.3 cannot be implemented without
it. Rendered pages 46-50 were read.

### 10.1 Figures 3-6 (timing annotations)

| Figure | Case | Sequence and annotations |
|---|---|---|
| Fig. 3 | analogue calls, answerer sends ANSam | Analogue: 1 s after ANSam starts → QC1a + CM. Digital: ANSam → QCA1d → **75 ± 5 ms** → QTS (768T) → QTS\ (48T) → ANSpcm. Analogue TONEq starts **≤ 1 s** after ANSpcm arrives; ANSpcm ends after TONEq is detected. |
| Fig. 4 | digital calls, answerer sends ANSam | Digital: 1 s after ANSam arrives → QC1d + CM. Analogue: ANSam → QCA1a. Digital: CM cut → 75 ± 5 ms → QTS 768T → QTS\ 48T → ANSpcm. TONEq ≤ 1 s later. |
| Fig. 5 | analogue calls, digital answerer sends CRe | Analogue: ≥ 50 ms of CRe → QC2a. Digital: CRe, then QCA2d → 75 ± 5 ms → QTS → QTS\ → ANSpcm. TONEq after 1 s. |
| Fig. 6 | digital calls, analogue answerer sends CRe | Digital: ≥ 50 ms of CRe → QC2d. Analogue: CRe → QCA2a. Digital: 75 ± 5 ms after QCA2a → QTS → QTS\ → ANSpcm. TONEq after 1 s. |

### 10.2 Digital modem as call modem (9.2.2)

- **9.2.2-R0 [SHALL].** Start by conditioning the receiver for ANSam (V.8) and, **optionally**, CRe
  (V.8 bis).
- **9.2.2.1-R1 [SHALL].** When ANSam has been detected for **1 s**, send **QC1d followed by CM**, and
  listen for QCA1a, JM and ANSam.
- **9.2.2.1-R2 [SHALL].** On **QCA1a**:
  1. stop CM **without completing the current octet**;
  2. send silence for **75 ± 5 ms**;
  3. send **QTS**, then **QTS\\**, then **ANSpcm** (using U_QTS from QCA1a);
  4. go to 9.2.2.3.
- **9.2.2.1-R3 [SHALL].** If ANSam is detected for 1 s after QC1d was sent, or if **JM** is
  detected, proceed per **V.8**: carry on with CM, JM and CJ.
- **9.2.2.2-R1 [SHALL].** If the **first 50 ms of CRe** are detected, send **QC2d followed by
  silence**, and listen for QCA2a, ANSam and ANS.
- **9.2.2.2-R2 [SHALL].** On **QCA2a**: silence **75 ± 5 ms**, then QTS, QTS\ and ANSpcm (U_QTS from
  QCA2a), then 9.2.2.3.
- **9.2.2.2-R3 [SHALL].** On **ANSam**: go to 9.2.2.1.
- **9.2.2.2-R4 [SHALL].** If **ANS** is detected **for 3 s** after QC2d was sent, proceed per
  **V.8**.
- **9.2.2.2-R5 [SHALL].** If QCA2a has not been detected **1 s** after sending "QC2a", proceed per
  **V.8 bis**.
  - [INTERP, erratum] Read "QC2a" as **QC2d**, the signal this modem sent.
- **9.2.2.3-R1 [SHALL].** While ANSpcm is being sent, listen for **TONEq**. When TONEq is detected,
  send silence for **75 ± 5 ms** and go to **Phase 2**.
  - **[AMBIG]** 9.2.2.3 sets no timeout for the calling digital modem (compare 9.2.4.3).

### 10.3 Digital modem as answer modem (9.2.4)

- **9.2.4-R0 [SHALL].** On connecting to the line, stay silent for **at least 200 ms**. Then send
  **ANSam** (per V.8) or **CRe** (per V.8 bis).
- **9.2.4.1-R1 [SHALL].** If ANSam is being sent (even after an earlier V.8 bis session timed out),
  listen for QC1a, QC1d and CM.
- **9.2.4.1-R2 [SHALL].** On **QC1a**:
  1. send **QCA1d**;
  2. send silence for **75 ± 5 ms**;
  3. send **QTS, QTS\ and ANSpcm**;
  4. go to 9.2.4.3.
- **9.2.4.1-R3 [MAY].** On **QC1d**, the modem may take the role of the analogue modem and follow
  9.2.3.1 (send QCA1a and listen for QTS, QTS\ and ANSpcm).
- **9.2.4.1-R4 [SHALL].** On **CM**, follow normal V.8.
- **9.2.4.2-R1 [SHALL].** If CRe is being sent, listen for QC2a, QC2d and V.8 bis signals.
- **9.2.4.2-R2 [SHALL].** On **QC2a**:
  1. **terminate CRe**;
  2. send **QCA2d**;
  3. send silence for **75 ± 5 ms**;
  4. send QTS, QTS\ and ANSpcm;
  5. go to 9.2.4.3.
- **9.2.4.2-R3 [MAY].** On **QC2d**, the modem may take the analogue role and follow 9.2.3.2.
- **9.2.4.2-R4 [SHALL].** On any other V.8 bis signal, follow normal V.8 bis.
- **9.2.4.2-R5 [SHALL].** If no V.8 bis signal, QC2a or QC2d is detected **3 s** after CRe was sent,
  send **ANSam** and go to 9.2.4.1.
- **9.2.4.3-R1 [SHALL].** While ANSpcm is being sent, listen for TONEq. On TONEq: silence
  **75 ± 5 ms**, then Phase 2.
- **9.2.4.3-R2 [SHALL].** If TONEq is not detected **within 2 s after QCA1d was sent**, send
  **ANSam** and proceed per **V.8**.
- **9.2.4.3-R3 [SHALL].** If TONEq is not detected **within 2 s after QCA2d was sent**, send ANSam
  and go to **9.2.4.1**.

### 10.4 Both roles

- **9.2.5-R1 [SHALL].** If both modems indicated LAPM (the P bits), the V.42 ODP/ADP exchange **shall
  be bypassed**.

### 10.5 What the analogue side does (constrains the digital modem's timing)

- **9.2.1.3 / 9.2.3.3.** The analogue modem sends TONEq for **at least 50 ms** once ANSpcm has been
  detected for **1 s**.
  - **[MAY]** It may send TONEq as soon as ANSpcm is detected, if it had already detected ANSam for
    1 s (calling) or had sent ANSam (answering).
  - It stops TONEq when ANSpcm stops, then sends silence for 75 ± 5 ms and enters Phase 2.
- **9.2.3.3.** An analogue answerer that does not detect ANSpcm within **2 s** of sending QCA1a (or
  QCA2a) sends ANSam and falls back to V.8 (or 9.2.3.1).
- **[DERIVED] The handshake.**
  - The digital modem ends ANSpcm when it detects TONEq.
  - The analogue modem ends TONEq when ANSpcm disappears.
  - Each then waits 75 ± 5 ms.
  - The digital modem's TONEq detection delay therefore directly lengthens TONEq, but has no other
    effect.

---

## 11. Timers and tolerances

| Item | Value | Where |
|---|---|---|
| Short Phase 1 bit rate | 300 bit/s (26.667 samples per bit at 8 kHz) | 8.2 |
| V.21 frequency accuracy | ±6 Hz (V.21); ±0.01% for V.8 bis messages | V.21 §3; V.8 bis 7.2 |
| QC1d length | 60 bits = 200 ms, sent once | 8.3.2 |
| QCA1d length | 70 bits = 233.3 ms, sent once | 8.3.4 |
| V.8 bis message preamble | 100 ms ± 2% of mark | V.8 bis 7.2.4 |
| V.8 bis flags | 2-5 opening, 1-3 closing | V.8 bis 7.2.5 |
| ANSam detection before QC1d | 1 s | 9.2.2.1 |
| CRe detection before QC2d | first 50 ms (figures: ≥ 50 ms) | 9.2.2.2 |
| QCA2a wait after QC2d | 1 s, then V.8 bis | 9.2.2.2 |
| ANS after QC2d | 3 s of ANS, then V.8 | 9.2.2.2 |
| Answer: initial silence | ≥ 200 ms | 9.2.4 |
| Answer: CRe with no response | 3 s, then ANSam | 9.2.4.2 |
| Silence before QTS | **75 ± 5 ms** (600 ± 40 samples) | 9.2.2.1, 9.2.2.2, 9.2.4.1, 9.2.4.2 |
| QTS | 768 symbols = 96 ms | 8.3.6, Figs 3-6 |
| QTS\ | 48 symbols = 6 ms | 8.3.6, Figs 3-6 |
| ANSpcm period | 301 symbols = 37.625 ms | 8.3.1 |
| ANSpcm reversal interval | 3612 symbols = 451.5 ms | 8.3.1 |
| ANSpcm tone | 8000 × 79/301 = 2099.668 Hz | 8.3.1 [DERIVED] |
| ANSpcm levels | -9.5 / -12 / -15 / -18 dBm0 | Table 6 |
| TONEq timeout (answer) | 2 s from the end of QCA1d / QCA2d | 9.2.4.3 |
| Silence after TONEq detection | 75 ± 5 ms, then Phase 2 | 9.2.2.3, 9.2.4.3 |
| TONEq (from the analogue modem) | 980 Hz, ≥ 50 ms, after 1 s of ANSpcm (or earlier, see §10.5) | 8.2.5, 9.2.1.3, 9.2.3.3 |
| CRe (sent by the digital answerer) | segment 1: 1375 + 2002 Hz, 400 ms nominal (MAY be 285 ms); segment 2: 400 Hz, 100 ms nominal; ±250 ppm; durations ±2%; 12-15 dB below nominal power | V.8 bis 7.1.1 to 7.1.4 |
| ANSam (sent by the digital answerer) | 2100 ± 1 Hz, reversals every 450 ± 25 ms, 15 ± 0.1 Hz AM with envelope 0.8-1.2 (±0.01) | V.8 7.2 |

---

## 12. What the referenced Recommendations require

| Reference | Requirement brought in |
|---|---|
| V.90 3.6 and Table 1 (via V.92 3.6 and clause 8) | Ucode ↔ octet ↔ linear value (G-5). Octets go to the interface exactly as tabled. |
| V.90 clause 5, 5.4 (via V.92 clause 5) | 8000 symbols/s; 6-symbol downstream data frame; interval 0 first. |
| V.90 8.1 | Phase 1 V.8/V.25 signals at the nominal transmit power. |
| V.90 8.4.4 (V.92 8.6.7) | Sd/Sd\ pattern and frame alignment. QTS copies the idea. |
| V.8 clause 5, 5.1, Table 1 | 10 ONEs + 10 sync bits + characters (start 0, b0..b7, stop 1); b4 = 0 in category octets; `0101010101` is reserved for V.92; CM/JM sync is `0000001111`, CI sync is `0000000001`. |
| V.8 7.2 | ANSam definition; a caller shall not send CM until ANSam has been detected. |
| V.8 7.3, 7.4, 8.1.2 | CM and JM content rules (call function first; pcm0 → access0 present; V.34 bit set; first modulation octet b5 = 1). JM after 2 identical CMs; caller sends CJ after 2 identical JMs. Short Phase 1 overrides "complete the current octet" with "stop without completing" on QCA1x. |
| V.8 Tables 5, 6, 7 | pcm0 (b5 analogue, b6 digital, b7 V.91); prot0 (`100` = LAPM); access0 (b7 = digital network connection). |
| V.8 bis 7.1 | CRe tones, durations, tolerances and level (§11). |
| V.8 bis 7.2, 7.2.2-7.2.9 | Message channel by role, bit and octet order, preamble, flags, FCS, transparency, invalid frames (§5.4). |
| V.8 bis 8.3, Tables 3, 4 | Message type `1101` (bits 4..1), which V.8 bis assigns to V.92; revision field bits 5-8 (Revision 2 = `0010`, bits 8..5). |
| V.21 | Channel frequencies and tolerances (G-4); at most 1 mW; -13 dBm0 at the international circuit input. |
| G.711 (not in `docs/specs`) | Quantising x (§3.4). The exact rule was confirmed by reproducing Tables 7-10. |
| V.42 (via P) | LAPM. With both P bits set, ODP/ADP is bypassed (9.2.5). |

---

## Appendix A: ANSpcm Tables 7-10 as octets (verified)

- Each block lists T[k] for k = 0..300, 20 per row. Row `k=040` holds k = 40..59.
- Values are the octets sent on the digital interface (V.90 Table 1 form), before any phase
  reversal.
- The values were generated by §3.4 and compared equal, entry by entry, with the extracted Tables
  7-10, and by spot checks against the rendered pp. 17-20.

Table 7/V.92, -9.5 dBm0 (LM = 00), scl = 1334 (mu) / 667 (A), mu-law octets, k = 0..300:
```text
k=000: A1 58 22 C2 A3 38 25 B0 A7 2C 2A A9 AE 26 34 A4 BC 22 4B A2
k=020: FC 22 CB A2 3C 24 B4 A6 2E 28 AA AC 27 2F A5 B8 23 41 A2 D7
k=040: 21 DB A2 43 23 B8 A5 30 27 AC AA 29 2D A6 B3 24 3C A2 CA 22
k=060: 72 A2 4D 22 BD A4 34 26 AE A8 2A 2B A7 AF 25 37 A3 C0 22 55
k=080: A2 5D 22 C4 A3 39 24 B1 A7 2C 2A A9 AD 26 33 A4 BB 22 48 A2
k=100: EC 22 CE A2 3E 23 B5 A5 2E 28 AB AB 28 2F A5 B6 23 3F A2 D2
k=120: 22 E0 A2 45 23 BA A4 31 27 AD A9 29 2D A6 B2 24 3A A3 C7 22
k=140: 67 A2 4F 22 BE A3 36 25 AF A8 2B 2B A8 AF 25 36 A3 BF 22 50
k=160: A2 65 22 C6 A3 3A 24 B2 A6 2D 29 A9 AD 27 31 A4 BA 23 46 A2
k=180: E2 22 D1 A2 3F 23 B6 A5 2F 28 AB AB 28 2E A5 B5 23 3E A2 CE
k=200: 22 EA A2 48 23 BB A4 32 26 AD A9 2A 2C A7 B1 24 39 A3 C5 22
k=220: 5E A2 53 22 BF A3 37 25 AF A8 2B 2B A8 AE 26 35 A4 BD 22 4D
k=240: A2 6F 22 C9 A2 3B 24 B3 A6 2D 29 AA AC 27 30 A5 B9 23 43 A2
k=260: DC 22 D6 A2 40 23 B7 A5 2F 27 AC AA 28 2E A6 B4 24 3D A2 CC
k=280: 22 F7 A2 4A 22 BC A4 33 26 AE A9 2A 2C A7 B0 25 38 A3 C2 22
k=300: 59
```

Table 7/V.92, -9.5 dBm0 (LM = 00), scl = 1334 (mu) / 667 (A), A-law octets, k = 0..300:
```text
k=000: 88 76 08 EE 89 13 0F 9B 82 06 01 83 84 0C 1F 8E 97 09 67 88
k=020: D4 08 E7 89 17 0E 9F 8C 04 03 81 86 02 1A 8F 93 0E 69 88 F1
k=040: 08 F5 88 6F 09 93 8F 1B 0D 87 80 03 04 8D 9E 0E 17 89 E6 08
k=060: 53 88 65 09 94 8E 1F 0C 85 83 01 06 82 9A 0F 12 8E E8 08 73
k=080: 88 49 08 EC 89 10 0F 98 8D 07 00 83 84 0D 1E 8F 96 09 60 88
k=100: DE 08 FA 89 15 0E 9C 8C 05 03 81 86 02 05 8C 9D 0E 6B 89 FC
k=120: 08 C2 88 6D 09 91 8F 18 0D 87 80 00 07 8D 99 0F 11 89 E3 08
k=140: 45 88 79 09 95 8E 1D 0C 85 82 01 06 82 85 0C 1D 8E EA 09 7E
k=160: 88 47 08 E3 89 11 0F 99 8D 07 00 80 87 0D 18 8F 91 09 62 88
k=180: C0 08 FF 89 6A 0E 9D 8C 05 02 86 81 02 05 8C 9C 0E 15 89 FB
k=200: 08 D8 88 60 09 96 8F 19 0D 84 80 00 07 8D 98 0F 10 89 ED 08
k=220: 4F 88 7D 08 EB 8E 12 0C 9A 82 06 01 83 85 0C 1C 8E 94 09 65
k=240: 88 5D 08 E1 89 16 0E 9E 8D 04 03 80 87 0D 1B 8F 90 09 6C 88
k=260: CB 08 F0 88 69 0E 92 8F 1A 02 86 81 03 04 8C 9F 0E 14 89 E4
k=280: 08 D6 88 66 09 97 8E 1E 0C 84 83 01 06 82 9B 0F 13 89 EE 08
k=300: 74
```

Table 8/V.92, -12 dBm0 (LM = 01), scl = 1000 (mu) / 500 (A), mu-law octets, k = 0..300:
```text
k=000: A9 5D 29 C9 AA 3D 2B B7 AD 32 2F AE B4 2C 3A AB C2 29 4F A9
k=020: FD 29 D0 A9 42 2B BB AC 35 2E AF B2 2D 37 AB BD 2A 48 A9 DC
k=040: 29 DF A9 4A 2A BE AB 38 2D B2 AF 2E 34 AC BA 2B 41 AA CE 29
k=060: 76 A9 52 29 C3 AA 3B 2C B5 AE 30 31 AD B7 2B 3D AA C7 29 5A
k=080: A9 62 29 CA AA 3E 2B B8 AD 32 2F AE B4 2C 3A AB C0 2A 4E A9
k=100: EF 29 D4 A9 44 2A BB AC 35 2E B0 B1 2D 36 AC BC 2A 46 A9 D8
k=120: 29 E6 A9 4B 2A BF AB 39 2D B3 AF 2F 33 AD B9 2B 3F AA CD 29
k=140: 6B A9 56 29 C5 AA 3C 2C B6 AE 30 31 AE B6 2C 3C AA C5 29 57
k=160: A9 69 29 CC AA 3F 2B B9 AD 33 2F AF B3 2D 39 AB BF 2A 4C A9
k=180: E7 29 D8 A9 46 2A BC AC 36 2E B1 B0 2E 36 AC BC 2A 45 A9 D5
k=200: 29 ED A9 4D 2A C0 AB 39 2C B4 AF 2F 33 AD B8 2B 3F AA CB 29
k=220: 64 A9 59 29 C7 AA 3D 2C B7 AD 31 30 AE B5 2C 3B AA C4 29 53
k=240: A9 72 29 CE AA 41 2B BA AC 34 2E AF B2 2D 38 AB BE 2A 4A A9
k=260: E0 29 DB A9 48 2A BD AB 37 2D B1 B0 2E 35 AC BB 2A 43 A9 D1
k=280: 29 F9 A9 4F 2A C2 AB 3A 2C B4 AE 2F 32 AD B8 2B 3E AA C9 29
k=300: 5E
```

Table 8/V.92, -12 dBm0 (LM = 01), scl = 1000 (mu) / 500 (A), A-law octets, k = 0..300:
```text
k=000: 83 49 00 E1 80 15 06 92 84 19 1A 85 9F 07 11 81 EE 00 79 83
k=020: D4 03 FE 80 6E 01 96 87 1C 05 9A 99 04 12 86 94 01 60 80 CB
k=040: 03 CC 80 66 00 95 86 13 07 99 9A 05 1F 87 91 01 69 80 F8 03
k=060: 51 83 7C 00 EF 81 16 06 9C 84 1B 18 84 92 06 14 81 E3 00 74
k=080: 83 40 00 E6 80 15 06 93 87 19 1A 85 9F 07 11 81 E8 00 7A 80
k=100: DD 03 F2 80 6C 01 96 86 1C 04 9B 98 04 1D 86 97 01 62 80 F6
k=120: 03 C4 80 67 00 EA 86 10 07 9E 85 05 1E 87 90 01 6B 80 E5 00
k=140: 59 83 70 00 ED 81 17 06 9D 84 1B 18 84 9D 06 17 81 E2 00 71
k=160: 83 5B 00 E4 80 6B 01 90 87 1E 05 85 9E 07 10 81 EB 00 64 80
k=180: DA 03 F6 80 62 01 97 86 1D 04 98 9B 04 1D 86 97 01 6D 80 F3
k=200: 03 DF 80 65 00 E8 81 10 07 9F 85 05 1E 87 93 06 6A 80 E7 00
k=220: 46 83 77 00 E3 81 14 06 92 84 18 1B 84 9C 06 16 81 EC 00 7D
k=240: 83 53 03 FB 80 69 01 91 87 1F 05 9A 99 07 13 86 95 00 66 80
k=260: C2 03 F5 80 60 01 94 86 12 04 98 9B 05 1C 87 96 01 6F 80 FF
k=280: 03 D6 83 78 00 EE 81 11 07 9F 85 1A 19 84 93 06 15 80 E1 00
k=300: 4F
```

Table 9/V.92, -15 dBm0 (LM = 10), scl = 708 (mu) / 354 (A), mu-law octets, k = 0..300:
```text
k=000: AF 63 30 CF B1 45 33 BE B5 3A 38 B7 BC 34 41 B2 CA 30 57 AF
k=020: FD 2F D8 B0 4A 31 C1 B4 3C 37 B8 BA 35 3E B3 C5 31 4E B0 E2
k=040: 2F E6 B0 4F 31 C6 B2 3E 35 BA B8 37 3C B4 C0 32 49 B0 D6 2F
k=060: 78 AF 59 30 CB B1 42 34 BC B6 39 3A B5 BE 33 44 B1 CE 30 5F
k=080: AF 68 2F D0 B1 47 32 BF B5 3B 38 B7 BC 34 40 B2 C9 30 55 AF
k=100: F3 2F DB B0 4C 31 C2 B3 3D 36 B9 BA 36 3D B3 C4 31 4D B0 DE
k=120: 2F EB AF 52 30 C7 B2 3F 35 BB B8 37 3B B4 BF 32 48 B0 D4 2F
k=140: 6F AF 5C 30 CC B1 43 33 BD B6 39 39 B6 BD 33 43 B1 CC 30 5D
k=160: AF 6D 2F D3 B0 48 32 BF B4 3B 37 B8 BB 34 3F B2 C8 30 52 AF
k=180: EC 2F DD B0 4D 31 C4 B3 3D 36 B9 B9 36 3D B3 C3 31 4C B0 DB
k=200: 2F F0 AF 54 30 C8 B2 3F 34 BB B7 38 3B B5 BF 32 47 B0 D1 2F
k=220: 69 AF 5F 30 CD B1 44 33 BE B6 3A 39 B6 BD 33 42 B1 CB 30 5A
k=240: AF 76 2F D6 B0 49 32 C0 B4 3C 37 B8 BB 35 3F B2 C6 31 50 AF
k=260: E7 2F E0 B0 4E 31 C5 B3 3E 35 BA B9 36 3C B4 C2 31 4B B0 D9
k=280: 2F FB AF 57 30 CA B2 41 34 BC B7 38 3A B5 BE 33 46 B1 CF 30
k=300: 64
```

Table 9/V.92, -15 dBm0 (LM = 10), scl = 708 (mu) / 354 (A), A-law octets, k = 0..300:
```text
k=000: 9A 41 1B F8 98 6D 1E 95 9C 11 13 92 97 1F 69 99 E6 1B 76 9A
k=020: D5 1A F6 9B 66 19 E9 9F 17 12 93 91 1C 15 9E ED 18 7B 9B C0
k=040: 1A C4 9B 79 18 E2 99 15 1C 91 93 12 17 9F E8 19 61 9B F0 1A
k=060: 56 9A 77 1B E7 98 6E 1F 97 9D 10 11 9C 95 1E 6D 98 FA 1B 4D
k=080: 9A 5A 1A FE 98 63 19 EA 9C 16 13 92 97 1F 68 99 E1 1B 73 9A
k=100: D3 1A F5 9B 64 18 EE 9E 14 1D 90 91 1D 14 9E EC 18 65 9B CF
k=120: 1A D9 9A 7C 1B E3 99 6A 1C 96 93 12 16 9F EB 19 60 9B F2 1A
k=140: 5D 9A 4B 1B E4 98 6F 1E 94 9D 10 10 9D 94 1E 6F 98 E5 1B 48
k=160: 9A 5F 1A FD 9B 60 19 EB 9F 16 12 93 96 1C 6B 99 E0 1B 7C 9A
k=180: DE 1A C9 9B 65 18 EC 9E 14 1D 90 90 1D 14 9E EF 18 64 9B F5
k=200: 1A D2 9A 72 1B E0 99 68 1F 96 92 13 16 9C EA 19 63 9B FF 1A
k=220: 58 9A 4C 1B E5 98 6C 1E 95 9D 11 10 9D 94 1E 6E 98 E7 1B 74
k=240: 9A 51 1A F0 9B 61 19 E8 9F 17 12 93 96 1C 6A 99 E2 18 7E 9B
k=260: C5 1A C2 9B 7B 18 ED 9E 15 1C 91 90 1D 17 9F EE 18 67 9B F7
k=280: 1A D7 9A 71 1B E6 99 69 1F 97 92 13 11 9C 95 1E 62 98 F9 1B
k=300: 46
```

Table 10/V.92, -18 dBm0 (LM = 11), scl = 500 (mu) / 250 (A), mu-law octets, k = 0..300:
```text
k=000: B8 69 39 D7 B9 4C 3B C6 BD 41 3F BE C3 3C 49 BA D0 39 5D B8
k=020: FE 38 DE B9 50 3A CA BC 44 3E BF C1 3D 46 BB CC 39 56 B9 E8
k=040: 38 EB B9 57 39 CD BB 47 3C C1 BF 3E 43 BC C9 3A 4F B9 DC 38
k=060: 7A B8 5F 39 D1 BA 4A 3B C4 BD 3F 40 BD C6 3B 4C BA D5 39 66
k=080: B8 6D 39 D8 B9 4D 3B C7 BC 41 3F BE C3 3C 48 BA CF 39 5B B8
k=100: F6 38 E0 B9 52 3A CA BB 44 3D BF C0 3D 45 BB CB 3A 54 B9 E4
k=120: 38 EE B9 59 39 CE BA 47 3C C2 BE 3E 42 BC C8 3A 4E B9 DB 38
k=140: 73 B8 61 39 D3 BA 4B 3B C5 BD 3F 3F BD C5 3B 4B BA D3 39 62
k=160: B8 71 39 DA B9 4E 3A C8 BC 42 3E BE C2 3C 48 BA CE 39 5A B9
k=180: EF 38 E3 B9 54 3A CB BB 45 3D C0 BF 3D 45 BB CB 3A 53 B9 E1
k=200: 38 F5 B8 5B 39 CF BA 48 3C C2 BE 3E 42 BC C7 3B 4E B9 D9 39
k=220: 6D B8 65 39 D5 BA 4C 3B C6 BD 40 3F BD C4 3B 4A BA D2 39 5F
k=240: B8 78 38 DC B9 4F 3A C9 BC 43 3E BF C1 3C 47 BB CD 39 58 B9
k=260: EC 38 E7 B9 56 3A CC BB 46 3D C0 BF 3E 44 BC CA 3A 51 B9 DE
k=280: 38 FC B8 5D 39 CF BA 49 3C C3 BE 3F 41 BD C6 3B 4D B9 D7 39
k=300: 6A
```

Table 10/V.92, -18 dBm0 (LM = 11), scl = 500 (mu) / 250 (A), A-law octets, k = 0..300:
```text
k=000: 93 5B 10 F1 90 64 16 E2 94 69 6A 95 EF 17 61 91 FE 10 49 93
k=020: D5 13 CE 90 7E 11 E6 97 6C 15 EA E9 14 62 96 E4 10 70 90 DA
k=040: 13 D9 90 71 10 E5 96 63 17 E9 EA 15 6F 97 E1 11 79 90 CB 13
k=060: 57 93 4C 10 FF 91 66 16 EC 94 6B 68 94 E2 16 64 91 F3 10 44
k=080: 93 5F 10 F6 90 65 16 E3 97 69 6A 95 EF 17 61 91 F8 10 4A 93
k=100: D1 13 C2 90 7C 11 E6 96 6C 14 EB E8 14 6D 96 E7 11 72 90 C6
k=120: 13 DC 90 77 10 FA 91 60 17 EE 95 15 6E 97 E0 11 7B 90 F5 10
k=140: 53 93 40 10 FD 91 67 16 ED 94 6B 68 94 ED 16 67 91 FD 10 41
k=160: 93 52 10 F4 90 7B 11 E0 97 6E 15 95 EE 17 60 91 FA 10 74 90
k=180: DD 13 C1 90 72 11 E7 96 6D 14 E8 EB 14 6D 96 E7 11 7D 90 C3
k=200: 13 D0 93 75 10 F8 91 60 17 EF 95 15 6E 97 E3 16 7A 90 F7 10
k=220: 5C 93 47 10 F3 91 64 16 E2 94 68 6B 94 EC 16 66 91 FC 10 4D
k=240: 93 56 13 CB 90 79 11 E1 97 6F 15 EA E9 17 63 96 E5 10 76 90
k=260: DE 13 C5 90 70 11 E4 96 62 14 E8 EB 15 6C 97 E6 11 7F 90 CF
k=280: 13 D4 93 48 10 F9 91 61 17 EF 95 6A 69 94 E3 16 65 90 F1 10
k=300: 58
```


---

## Implementation notes

### What is new compared with V.90

1. **A Phase 1 shortcut.** V.90's digital modem does plain V.8 or V.8 bis. V.92 adds QC/QCA frames
   inside the V.8 framing (new sync `0101010101`) and as a new V.8 bis message type (`1011`
   transmitted, V.8 bis "Defined in ITU-T V.92").
2. **The first PCM-domain signals come in Phase 1.** QTS/QTS\ and ANSpcm are raw codeword
   sequences. In V.90 the first such signal is Sd in Phase 3.
3. **Frame alignment starts in Phase 1.** It must survive the whole of Phase 2, where the digital
   modem sends V.34-style tones and DPSK INFO as PCM samples. The frame counter must therefore run
   from the first QTS sample, with no resets at phase boundaries.
4. **A new answer tone.** ANSpcm replaces ANSam once the short path is agreed. The analogue modem
   answers with TONEq (980 Hz). The digital modem must add a TONEq detector.
5. **LAPM is agreed in Phase 1** through P, and ODP/ADP is bypassed (9.2.5).
6. **The digital modem announces its ANSpcm level (LM).** The analogue modem chooses the QTS
   codeword (U_QTS).

### Pitfalls

1. **Table 6 in the extracted text is wrong** (the scl column is shuffled). Use §3.2.
2. **The tables are octets, not Ucodes**, whatever the text says.
3. **The quantiser tie rule matters.** Use decision intervals with the lower bound inclusive
   (§3.4). The existing `ucode::nearest()` is a nearest-reconstruction decoder-side function and
   gets 12-48 entries per table wrong. Best practice: ship Appendix A as `const` arrays, and test
   the generator against them if one is kept.
4. **Use floor(v + 0.5), not truncation or round-half-even**, and use f64.
5. **Phase reversal = XOR 0x80 on every octet**, starting from the ANSpcm origin (ANS-12). Do not
   re-quantise.
6. **+0 and -0 are different octets in QTS.** μ-law `FF`/`7F`, A-law `D5`/`55`.
7. **Bit order.**
   - Tables 11-14 are leftmost-first, so L goes out before M and WXYZ goes out W first.
   - V.8 characters are LSB first, with b0 = the bit after the start bit.
   - V.8 bis octets are bit 1 first. Keep the three conventions straight (§4.3, §5.3).
8. **V.21 at 8 kHz** has 26.667 samples per bit. Accumulate phase and bit time fractionally. A
   27-sample bit drifts 23 samples (almost a whole bit) over a 70-bit frame.
9. **CM must be cut mid-octet** on QCA1a (9.2.2.1). A V.8 CM transmitter written to finish the
   octet and send CJ needs an "abort now" path.
10. **Receive-side parsing.** Select the parser by the preamble sync: `0101010101` for QC,
    `0000001111` for CM/JM. A digital answerer must accept both QC1a and QC1d, and a caller must
    accept QCA1a while it is still sending CM, which is full-duplex V.21(L) out and V.21(H) in.
11. **Two errata:**
    - 8.3.5 says "analogue modem" where it means the digital modem.
    - 9.2.2.2 says "QC2a" where it means QC2d.
12. **980 Hz confusion.** TONEq, the V.21(L) mark and V.8 bis ESi segment 2 all sit at 980 Hz. The
    digital modem's own QCA2d preamble and marking bits are 980 Hz too. Arm the TONEq detector only
    after QCA2d ends, and only once ANSpcm is on the line. Require a steady 980 Hz with no FSK
    movement; the analogue modem sends at least 50 ms.
13. **Round-trip delay against the 2 s windows** [DERIVED].
    - **Digital answerer (9.2.4.3).**
      - After QCA1d/QCA2d ends, ANSpcm starts 75 + 96 + 6 = **177 ms** later.
      - An analogue caller normally needs **1 s** of ANSpcm before sending TONEq.
      - So one full round trip plus both detectors must fit in about 2000 - 177 - 1000 ≈ 820 ms.
    - **On Rory's VoIP rig** (about 1.5 s round trip, 750 ms each way; see project memory):
      - In the V.8 bis case the analogue caller cannot use the early-TONEq option: 9.2.1.3 allows it
        only after ANSam was detected in 9.2.1.1. TONEq then reaches the digital modem at about
        177 + 750 + 1000 + 750 ≈ 2.7 s, so the answer side **always times out**.
      - In the V.8 case it works only if the caller takes the early option: about 177 + 1500 ms plus
        both detection times, just under 2 s, which is marginal.
    - **Digital caller.** The analogue answerer's own 2 s window (9.2.3.3) is also marginal. QCA1a
      reaches the digital modem after about 750 ms plus detection, and the digital modem adds
      177 ms. ANSpcm therefore reaches the analogue modem after about 1.68 s, plus its detector's
      time.
    - Expect fallbacks to ANSam/V.8 on that path, and do not mistake them for bugs.
14. **The calling digital modem has no TONEq timeout** (9.2.2.3). Add a defensive one, and decide
    what it falls back to (Question 4).
15. **ANSpcm is not ANSam.** Our own analogue-side detector, if we build one, must not require the
    15 Hz AM to find ANSpcm. It must still tell ANSam (fallback path) from ANSpcm: the sign pattern
    and exact codewords give this away in the PCM domain; on the analogue side use the AM.
16. **Silence is Ucode-0 codewords**, with frame alignment kept (G-7). "Stop transmitting" in a PCM
    modem must still clock out octets.
17. **The network may change the channel when ANSpcm arrives** (8.3.1 NOTE). Measurements the
    analogue side makes before ANSpcm may be stale.

### Ambiguities and questions for Rory

1. **Q1: ANSpcm level policy.** V.92 leaves the choice open. Proposal: the level nearest the
   configured nominal transmit power, limited to -9.5..-18 dBm0, default -12 dBm0 (`01`). Always
   send the level that LM announced.
2. **Q2: Where the first reversal falls.** Proposal: after the first full 3612 symbols, counting
   from the first ANSpcm symbol (ANS-12). A capture from a real V.92 server would settle it.
3. **Q3: V.8 bis information field contents.** Is it only the 16-bit I field, or should an S field
   follow? Proposal: send I only, and on receive accept and ignore anything extra (§5.4 step 3).
4. **Q4: TONEq timeout for a calling digital modem** (9.2.2.3 gives none). Proposal: mirror
   9.2.4.3 with 2 s after the end of the received QCA1a/QCA2a plus a round-trip allowance, then fall
   back to V.8 (resume CM) or V.8 bis. That fallback is itself undefined.
5. **Q5: Rule for accepting QC and QCA frames.** Proposal: both copies present, start and stop bits
   correct, copies equal. Accept after the second copy's stop bit (bit 59), without waiting for
   QCA's trailing ONEs.
6. **Q6: Do we implement the MAY in 9.2.4.1 and 9.2.4.2?** That is, a digital answerer that
   receives QC1d/QC2d from another digital modem takes the analogue role. It needs the analogue-side
   short Phase 1 (QTS/ANSpcm detection) inside the digital modem.
7. **Q7: WXYZ = 1111 in a fresh call.** Proposal: ignore the QC and continue with V.8/V.8 bis.
   Inside modem-on-hold, disconnect (9.10.2.1).
8. **Q8: The P bit and CM's prot0.** Proposal: P = 1 exactly when CM carries the LAPM prot0 octet
   and V.42 is enabled.
9. **Q9: Level of the QC/QCA V.21 signals.** Not stated. Proposal: nominal transmit power, as for
   CM/JM (V.90 8.1).
10. **Q10: Ending ANSpcm mid-period.** Allowed? Proposal: stop at the next symbol after TONEq is
    detected. Nothing requires stopping on a period or frame boundary, and the frame counter keeps
    running anyway.
11. **Q11: The U_QTS range and our power limits.** QTS power is -21 to -10.3 dBm0 (§8.2), within
    V.90's usual digital-modem limits. No clamp is needed, but log it.
