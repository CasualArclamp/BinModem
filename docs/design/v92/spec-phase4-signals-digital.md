# V.92 clause 8.8: Phase 4, rate renegotiation and fast parameter exchange signals sent by the digital modem

This is the implementation digest for ITU-T V.92 (11/2000), clause 8.8. It was written from the rendered PDF
pages, not from the extracted text. Everything here is paraphrased. Clause numbers are V.92 unless another
Recommendation is named.

Requirement tags:
- **[SHALL]** is normative.
- **[SHOULD]** and **[MAY]** are the Recommendation's options.
- **[DERIVED]** is a consequence I worked out from the cited text. It is not stated there.
- **[INTERP]** is my reading where the text is ambiguous. Each one is listed again in section 13.3.

## 0. Sources read (all as rendered page images)

| Document | PDF pages (1-based) | What was used |
|---|---|---|
| T-REC-V.92-200011-I.pdf | **39, 40, 41, 42, 43, 44** | All of 8.8: 8.8.1-8.8.6, Table 30 (4 pages), Table 31. Also the end of 8.7.6/8.7.7 and the start of 8.9 |
| T-REC-V.92-200011-I.pdf | 9, 10, 11, 12, 13 | 3.5 (Qa.b), 4 (RTDEd), 5, 6.1-6.4.4 (upstream framing Figure 1, modulus encoder, precoder/prefilter, trellis), 8 intro (bit-order rule) |
| T-REC-V.92-200011-I.pdf | 27 | Table 18 (INFO1a with PCM upstream: filter sections, Ltot, Lmax) |
| T-REC-V.92-200011-I.pdf | 33, 34, 35, 36, 37, 38 | 8.7.1-8.7.6: B1u, E2u (bit 29 of CPd), CPu/CPt (Table 23), CPus (Table 24), RM/RM' (Tables 25/26), SUVu (Table 27), TRN2u |
| T-REC-V.92-200011-I.pdf | 56-65, 69 | 9.6 (Figures 12-14), 9.7, 9.8 (Figures 15-18), 9.9 (Figure 19), 9.11 |
| T-REC-V.90-199809-I.pdf | 11, 12, 13, 14, 15, 16, 18, 33, 34, 35, 36, 43, 45 | Table 1 (Ucodes), 5.1-5.4.7 (encoder, Table 2, sign/shaping), 8.6 intro, 8.6.1 B1d, 8.6.2 Ed, 8.6.3 MP/Table 16, 8.6.4 R, 8.6.5 TRN2d/Table 17, 9.4.1 and 9.6.1 (the V.90 procedures, for comparison) |
| T-REC-V.34-199802-I.pdf | 16, 33 | 7 (scrambler polynomials), 10.1.2.3.2 (CRC, Figure 14) |

The page PNGs are in the session scratchpad under `v92/pages/`:
- `v92_88_p39.png` .. `v92_88_p44.png`
- the zoomed crops `v92_88_p43_rf_zoom.png` and `v92_88_p40_sync_zoom.png`
- the referenced pages `v92_88_pNN.png`, `v90_88_pNN.png` and `v34_88_pNN.png`

---

## 1. What clause 8.8 defines

8.8 has **no introductory paragraph**. The V.90 clause it replaces (8.6/V.90) did have one; see section 8.3
and Q7. 8.8 lists six items. The digital modem sends all of them downstream, as PCM codewords at 8000
symbols/s, in three situations: Phase 4 of start-up (final training), rate renegotiation (RR) and fast
parameter exchange (FPE).

| Clause | Signal(s) | Where it is defined | Status vs V.90 |
|---|---|---|---|
| 8.8.1 | B1d | 8.6.1/V.90, by reference | unchanged |
| 8.8.2 | Ed | 8.8.2 (V.92 text) | same payload as V.90. New: can also go out in data-mode modulation (FPE) |
| 8.8.3 | CPd, CPd' | 8.8.3 + Table 30 | **new**; replaces V.90's MP when the upstream is PCM |
| 8.8.4 | Rd, R̄d, Rt, R̄t | 8.6.4/V.90, by reference | unchanged |
| 8.8.4 | Rf, R̄f | 8.8.4 | **new** (FPE) |
| 8.8.5 | SUVd, SUVd' | 8.8.5 + Table 31 | **new** |
| 8.8.6 | TRN2d | 8.6.5/V.90, by reference | unchanged |

Clause 9 adds one more digital-modem signal that 8.8 does not list: **silence** in an RR with a silence request
(9.8.1.1.3). It is covered in section 9 here.

Notation:
- T is one symbol interval, 1/8000 s = 125 µs. Clause 5 of V.92 makes the digital modem's rates, symbol rate,
  scrambler and encoder the same as clause 5/V.90, and 5.2/V.90 fixes 8000 symbols/s downstream.
- A prime (') means the acknowledge bit is set.
- An overbar (R̄) means the sign-inverted version of an R pattern.
- A "data frame" downstream is 6 symbols (5.4/V.90). Its intervals are i = 0..5, and i = 0 goes first.
- "Word" means one start bit (0) followed by 16 information bits.

---

## 2. Rules shared by the 8.8 signals

### 2.1 Bit order

- **GEN-1 [SHALL] (clause 8 intro, p.13).**
  - PCM codewords in training sequences are described by the universal codes (Ucodes) of Table 1/V.90.
  - The rule covers Tables 2-5, 11-24, 27 and **30-33**, so Tables 30 and 31 are included. Unless a table
    says otherwise, a value written as a bit pattern goes out leftmost bit first, and a value written as an
    integer goes out least-significant bit first.
- **GEN-2 [SHALL] (8.8.3, 8.8.5).** "Bit 0 is transmitted first." The tables give each field as `LSB:MSB`,
  so the lowest-numbered bit of a field is its LSB and goes out first.
  - Example: drn in bits 22:26 has its LSB in bit 22.
  - The frame sync pattern is all ones, so its order does not matter.

### 2.2 Number formats (3.5, p.9)

- **Signed Qa.b:** (a + b + 1)-bit two's complement with b bits after the binary point. Values lie in
  [-2^a, 2^a).
- **Unsigned Qa.b:** (a + b) bits with b bits after the binary point. The range is printed as [0, 2^(a+1)).
  That is inconsistent with an (a + b)-bit field, whose range is [0, 2^a). The typo does not affect Q0.16.

The formats used in 8.8:

| Field | Format | Width | Decode | Range |
|---|---|---|---|---|
| 4·G (CPd 35:50) | unsigned Q0.16 `.xxxxxxxxxxxxxxxx` | 16 | 4G = N/65536, so **G = N/262144** | 0 < 4G < 1, i.e. 0 < G < 0.25 |
| z1(k), z2(k) | signed Q0.15 `s.xxxxxxxxxxxxxxx` | 16 | N (int16) / 32768 | [-1, 1) |
| p1(k), p2(k) | signed Q1.14 `sx.xxxxxxxxxxxxxx` | 16 | N (int16) / 16384 | [-2, 2) |

- These are integers on the wire, so they go out LSB first and the sign bit goes last (GEN-1).
- **GEN-3 (5.4.6/V.90, p.18):** a PCM sign bit of **1 means a positive voltage** and 0 means negative.
  - The "+" and "-" in the R patterns are these voltages.
  - The octets in Table 1/V.90 are the positive codewords, with the MSB (the G.711 polarity bit) set: µ-law
    Ucode 0 = `FF`, A-law Ucode 0 = `D5`.
  - The negative codeword is the same octet with the MSB cleared.

### 2.3 Framing of the information sequences (CPd, SUVd)

CPd and SUVd use the V.34/V.90 information-sequence frame. The layout below is taken from the rendered
Tables 30 and 31:

```
bit:  0 ...... 16 | 17 | 18 ...... 33 | 34 | 35 ...... 50 | ... | s | s+1 .. s+16 | s+17 | s+18 ...
      1 x 17      | 0  | word 0 info  | 0  | word 1 info  | ... | 0 |  CRC (16)   |  0   | 0 ... 0
      frame sync   start               start                      start              fill   fill to frame boundary
```

- **Frame sync:** bits 0:16, 17 ones. Counted in the rendered text; the pattern has 17 ones.
- **Words.** W information words follow the sync.
  - Word w (counted from 0) has its start bit (0) at `17 + 17·w` and its 16 information bits at
    `18 + 17·w .. 33 + 17·w`.
- **CRC.** After the last word comes one more start bit (0) at `s = 17 + 17·W`. The 16-bit CRC follows at
  `s+1 .. s+16`, LSB first.
- **Fill.**
  - **Fill bit 0** is at `s+17`.
  - More 0 fill bits follow "to extend the sequence length to the next multiple of 6 symbols" (last row of
    Tables 30 and 31).
  - The length before this extra fill is **17·W + 35** bits.
- **Unit of the padding [DERIVED].** One 6-symbol downstream data frame carries D = K + S bits (5.4.2/V.90).
  "A multiple of 6 symbols" therefore means the total bit count is a multiple of D, where D belongs to the
  modulation carrying the sequence (section 2.6). The repo's `v90::sequences::mp_bits` already pads this
  way.
- **No false sync [DERIVED].** 17 consecutive ones cannot occur after the sync. A zero start bit comes at
  least every 17 bits, and the bit after the CRC is a zero fill bit.
- **Frame alignment [DERIVED].** Each sequence is padded to a whole number of data frames. Every CPd/SUVd,
  and the Ed after it, therefore **starts on a downstream data-frame boundary**, provided the first one does.

### 2.4 CRC (10.1.2.3.2/V.34, cited by 8.8.3 and 8.8.5)

- **GEN-4 [SHALL].** The CRC covers **all information bits of the sequence** and excludes the frame-sync
  bits, the start bits and the fill bits. For CPd and SUVd that means the 16 information bits of every word,
  in transmission order. Reserved bits, flags, the type bit and the acknowledge bit are all included.
- Polynomial: x^16 + x^12 + x^5 + 1.
- Procedure:
  1. load the register with all ones;
  2. shift the information bits in;
  3. output the register contents starting with bit 0 of Figure 14. **Bit 0 of the CRC is its LSB** and goes
     out first.
- Figure 14/V.34 (rendered p.33):
  - It has cells 15 (left) to 0 (right), shifting right.
  - The input bit is XORed with the output of cell 0.
  - The sum feeds cell 15 and the adders in front of cells 10 and 3.
  - There is no final inversion.

```
reg = 0xFFFF
for b in information_bits:            # in transmission order
    fb  = (reg & 1) ^ b
    reg >>= 1
    if fb: reg ^= 0x8408               # cells 15, 10 and 3
crc = reg                              # sent LSB first
```

- Repo:
  - `crates/datapump/src/v34/info.rs::crc()` implements exactly this.
  - `crates/datapump/src/v90/sequences.rs::{frame, unframe, put, get}` build and check the sync / start-bit
    / CRC frame.
  - I re-checked the algorithm against the real server Jd in that file's tests: CRC 0x776E matches.

**Test vectors (computed with the algorithm above; not from the Recommendation).** The spaces separate sync
| start | word 0 | start | CRC | fill bit.

| Sequence | Word 0 as u16 (bit 18 = bit 0) | CRC | Bits 0..51 |
|---|---|---|---|
| SUVd | 0x0001 | 0xE960 | `11111111111111111 0 1000000000000000 0 0000011010010111 0` |
| SUVd' | 0x8001 | 0x6D68 | `11111111111111111 0 1000000000000001 0 0001011010110110 0` |
| SUVd, bit 32 set | 0x4001 | 0xAB64 | `11111111111111111 0 1000000000000010 0 0010011011010101 0` |
| SUVd', bit 32 set | 0xC001 | 0x2F6C | `11111111111111111 0 1000000000000011 0 0011011011110100 0` |

Minimal CPd: no optional parts, drn = 19, 16-state trellis, bit 29 = 0, 4G = 0x4000 (G = 1/16).
- Word 0 = 0x0130, word 1 = 0x4000, CRC = 0x570B. The length before padding is 69 bits:
  `11111111111111111 0 0000110010000000 0 0000000000000010 0 1101000011101010 0`
- The same CPd with bit 33 set (CPd') has word 0 = 0x8130 and CRC = 0x5BE7.

### 2.5 Scrambler

- **GEN-5 [SHALL] (clause 5 → 5.3/V.90 → 7/V.34).** The digital modem uses the self-synchronising scrambler
  of clause 7/V.34 with the **call-mode polynomial GPC = 1 + x^-18 + x^-23** (equation 7-1/V.34).
  - Transmitter: `out(n) = in(n) ^ out(n-18) ^ out(n-23)`.
  - Descrambler: `in(n) = rx(n) ^ rx(n-18) ^ rx(n-23)`.
  - The analogue modem uses GPA = 1 + x^-5 + x^-23 (6.3), so the two directions differ.
- **GEN-6 [SHALL].** What goes through the scrambler:
  - "CPd is scrambled" (8.8.3) and "SUVd is scrambled" (8.8.5). The whole sequence passes through the
    scrambler: sync, start bits, words, CRC and fill.
  - Ed is scrambled binary **zeros** (8.8.2).
  - TRN2d and B1d are scrambled binary **ones** (8.6.5/V.90, 8.6.1/V.90).
- **GEN-7: scrambler state across sequences.**

  | Point | Rule | Source |
  |---|---|---|
  | Before TRN2d | scrambler, differential encoder and spectral-shape filter memory initialised to **zero** [SHALL] | 8.6.5/V.90 |
  | Before B1d | the same three initialised to zero | 8.6.1/V.90 |
  | FPE, after R̄f, before the first SUVd | the same three initialised to zero [SHALL] | 9.9.1.1.2, 9.9.1.2.3 |
  | Anywhere else (TRN2d→SUVd→CPd→SUVd→Ed; Ed→silence→Rt/R̄t→SUVd) | no reset is stated, so the state carries on [INTERP] | -- |

- **Receiver implication [DERIVED].** A self-synchronising descrambler emits wrong bits for 23 bits after the
  far transmitter resets, unless it resets at the same instant. The resets above fall at known frame
  boundaries, so a receiver can reset in step.
  - Otherwise the first SUVd after an FPE reset may lose its sync. That is harmless, because SUVd is sent
    repeatedly.

### 2.6 The modulation carrying each signal

| Signal | Initial training / retrain (Phase 4) | Rate renegotiation | Fast parameter exchange |
|---|---|---|---|
| TRN2d | TRN2d modulation | TRN2d modulation | not sent |
| SUVd | "corresponding TRN2d modulation" | same | "preceding data mode modulation" |
| CPd | "corresponding TRN2d modulation" | same | "preceding data mode modulation" |
| Ed | "corresponding TRN2d modulation" | same | "preceding data mode modulation" |
| B1d | new data-mode parameters from the CPu just received, at the negotiated rate (9.6.1.1.5) | same | same |
| Rd / R̄d | -- | fixed codewords, not scrambled or encoded | -- |
| Rt / R̄t | -- | fixed codewords (only after a silence request) | -- |
| Rf / R̄f | -- | -- | fixed codewords, not scrambled or encoded |
| silence | -- | Ucode 0 magnitudes (only after a silence request) | -- |

What the modulations mean, as the encoder of 5.4/V.90 sees them. The parameter set is {Ci, Mi, K, Sr/S, ld,
a1, a2, b1, b2}.

- **TRN2d modulation in initial training / retrain.** Every parameter comes from **CPt** (Table 23 with type
  bits 19:20 = 0, sent by the analogue modem in Phase 3):
  - constellations: per-interval index bits 103:127 and masks from bit 136;
  - the rate: D_t = drn_CPt + 8, because the rate is (drn + 8)·8000/6;
  - S = 6 - Sr with Sr from bits 31:32, and K = D_t - S;
  - ld from bits 49:50 and a1, a2, b1, b2 from bits 69:101.

  That CPt defines the spectral shaping is the V.90 8.6 intro rule; see 8.3 below.
- **TRN2d modulation in rate renegotiation.** V.90 8.6 intro [INTERP: still applies in V.92, Q7]:
  - the constellations and **K** stay as derived from CPt;
  - **Sr, ld and a1..b2 are those of the preceding data mode**, i.e. from the CPu in force;
  - so D_rr = K_CPt + (6 - Sr_data), which can differ from the Phase 4 D.
  - The repo already models this: `v90::encoder::Mapping::for_renegotiation(cpt, data_mode)`.
- **Preceding data mode modulation (FPE).** The mapping currently used for data: the Ci and Mi of the CPu in
  force, D = drn_CPu + 20, S = 6 - Sr, K = D - S, and that CPu's ld and shaping.
- Permitted K and S:

  | Modulation | Table | K | S |
  |---|---|---|---|
  | TRN2d | Table 17/V.90 | 6..24 | 3..6 |
  | data mode | Table 2/V.90 | 15..39 | constrained per K (see section 8) |

### 2.7 Differential (sign) encoding

- The V.90 encoder's sign path (5.4.5/V.90, pp.15-16):

  | Sr | S | Sign coding |
  |---|---|---|
  | 0 | 6 | `$0 = s0 ^ $5(previous frame)`, `$i = s_i ^ $(i-1)` for i = 1..5 |
  | 1, 2, 3 | 5, 4, 3 | parse into shaping frames (Table 3/V.90), odd-bit differential coding (Table 4/V.90), a second differential coding `t_j(k) = p'_j(k) ^ t_(j-1)(k)`, then the spectral shaper (5.4.5.5) |

- **GEN-8 (8.8.3) [SHALL].** For training and rate renegotiation, CPd's differential encoder "is initialized
  using the last transmitted sign bit of the preceding sequence".
- **GEN-9 (8.8.5) [SHALL].** SUVd has the same sentence, but **without** the "training and rate
  renegotiation" qualifier.
- **GEN-10 (9.9.1.1.2, 9.9.1.2.3) [SHALL].** In FPE, the scrambler, differential encoder and spectral-shaping
  filter memory are set to **zero** before SUVd. This overrides GEN-9 for FPE.
  - The sequence before that point is R̄f, which is not differentially encoded anyway.
- **In practice, with Sr = 0 [DERIVED].** "Initialised with the last transmitted sign bit" means the
  `$5(previous frame)` input is the actual sign of the last symbol sent. In other words, the encoder state
  simply continues from TRN2d → SUVd → CPd → SUVd → Ed.
- **Special case [DERIVED]: SUVd straight after R̄t** (RR with silence, 9.8.1.1.4/5).
  - The preceding sequence is R̄t, whose pattern `- - - + + +` ends on "+".
  - With Sr = 0, the first SUVd's differential reference is therefore sign 1, **not** whatever the encoder
    held before the silence.
  - The silence and Rt/R̄t codewords do not come from the encoder, so the encoder state must be overwritten
    here.
- **With Sr > 0,** "the last transmitted sign bit" does not define the shaper's state (t, p', Q_j and the
  filter memory). See Q6.

### 2.8 Grouping rule

- **GEN-11 [SHALL] (8.8.3).** When several CPd and CPd' are sent as a group, they all carry identical
  information.
- **GEN-12 [SHALL] (8.8.5).** The same applies to SUVd and SUVd'.
- **[DERIVED]** Within a group, only bit 33 (acknowledge) may change, and with it the CRC. A change in bit 33
  is exactly what turns CPd into CPd'.

---

## 3. B1d (8.8.1 → 8.6.1/V.90, rendered V.90 p.34)

8.8.1 says only "As defined in 8.6.1/V.90". 8.6.1/V.90 requires:

- **B1-1 [SHALL].** B1d is **48 data frames** of **scrambled ones**, sent at the end of start-up with the
  **selected data mode constellation parameters**.
  - That is 48 × 6 = 288 symbols = 288T = 36 ms.
  - In V.92 these are "the data mode constellation parameters it received in CPu", at the negotiated rate
    (9.6.1.1.5), in all three procedures.
- **B1-2 [SHALL].** The scrambler, differential encoder and spectral-shape filter memory are initialised to
  zero before B1d.
- **B1-3 [SHALL].** The symbols of B1d's first data frame have the magnitudes that come from mapping the
  **first D scrambled ones after the scrambler is zeroed**. They are identical for every ld value.
  - **[DERIVED]** A look-ahead shaper (ld > 0) must not delay or reorder the magnitudes. Only signs may
    depend on look-ahead, so fill the look-ahead window before the first frame goes out.
- **B1-4.** Permitted K and S are those of **Table 2/V.90** (rendered V.90 p.14). Rows (K: S from..to →
  rate in kbit/s):

  | K | S | Rate (kbit/s) |
  |---|---|---|
  | 15 | 6..6 | 28 |
  | 16 | 5..6 | 28 - 29 1/3 |
  | 17 | 4..6 | 28 - 30 2/3 |
  | 18..36 | 3..6 | K = 18: 28 - 32, rising by 1 1/3 per K, up to K = 36: 52 - 56 |
  | 37 | 3..5 | 53 1/3 - 56 |
  | 38 | 3..4 | 54 2/3 - 56 |
  | 39 | 3..3 | 56 |

  The rate is always (K + S)·8000/6, from 28 000 to 56 000 bit/s (5.1/V.90).
- **B1-5 (8.6 intro/V.90) [SHALL].** B1d and the data after it use the spectral-shaping parameters defined by
  CP. In V.92 that CP is **CPu**: Sr in bits 31:32, ld in bits 49:50, and a1, a2, b1, b2 (signed Q1.6) in
  bits 69:101 (Table 23).
- Sequencing (9.6.1.1.5):
  1. Ed;
  2. B1d;
  3. circuit 106 is enabled to follow circuit 105;
  4. data under clause 5.

---

## 4. Ed (8.8.2, p.39)

- **ED-1 [SHALL].** Ed is **2 data frames** of **scrambled binary zeros**: 12 symbols = 12T = 1.5 ms, and 2·D
  input bits.
- **ED-2 [SHALL].** In training and RR, Ed uses the corresponding TRN2d modulation. In FPE it uses the
  preceding data-mode modulation.
- The scrambler and differential encoder carry on from the preceding CPd/SUVd (GEN-7, GEN-8).
  - The zeros go through the scrambler, so Ed is **not** a constant signal on the line.
- V.90 wording (8.6.2/V.90): "2 data frames of scrambled binary zeroes used to signal the end of MP", mapped
  with the TRN2d constellation parameters. V.92 keeps the payload and adds the FPE case.
- Role:
  - Ed ends the digital modem's SUVd/CPd exchange (9.6.1.1.4, 9.8.1.1.3).
  - The analogue modem treats Ed as an acknowledgement (9.6.2.1.4, 9.8.2.1.4) and switches its receiver to
    B1d (9.6.2.1.6).
- **ED-3 [SHALL] (9.6.1.1.4, 9.8.1.1.3).** Ed is sent only after the current CPd or SUVd has been
  **completed**, including its fill.
  - **[DERIVED]** Ed therefore begins on a data-frame boundary, where a new sequence's 17-one sync would
    otherwise start.
  - An analogue receiver can spot Ed as "the frame after a complete sequence descrambles to zeros instead of
    ones".
  - At the lowest TRN2d rate (D = 9), Ed is only 18 bits long, so detection has to be quick.

---

## 5. CPd and CPd' (8.8.3 and Table 30, pp.39-43)

### 5.1 Narrative requirements (8.8.3)

- **CP-1.** CPd carries the modulation parameters that the **analogue modem uses in data mode**, i.e. the
  upstream PCM transmitter of 6.4.
- **CP-2.** A CPd has four parts:
  1. **Mandatory:** bits 0 to 50 (sync + 2 words). Always sent.
  2. **Modulus encoder parameters:** 6 words. Present if bit 19 = 1.
  3. **Prefilter and precoder coefficients:** 4 + LZ1 + LP1 + LZ2 + LP2 words. Present if bit 20 = 1.
  4. **Constellation sets:** 5 + LC1 + LC2 + LC3 + LC4 + LC5 + LC6 words. Present if bit 21 = 1.
- **CP-3 [SHALL].** When a part is flagged absent, **all its bits are removed** from the sequence.
  - The absolute bit positions in Table 30 assume that all parts are present. The positions actually sent
    depend on which parts are present.
- **CP-4 [SHALL].** Every CPd ends with the CRC field and at least one fill bit (then fill to 6 symbols;
  2.3).
- **CP-5 [SHALL].** CPd is scrambled. It uses the corresponding TRN2d modulation in training and RR, and the
  preceding data-mode modulation in FPE.
- **CP-6 [SHALL].** In training and RR, the differential encoder is initialised from the last transmitted
  sign bit of the preceding sequence (2.7).
- **CP-7.** A CPd with the acknowledge bit (bit 33) set is written CPd'.
- **CP-8 [SHALL].** Bit 0 goes first (2.1).
- **CP-9.** α = 17 × (LZ1 + LP1 + LZ2 + LP2) and β = 17 × (LC1 + … + LC6). These are the bit lengths of the
  coefficient words and the constellation-point words.
- **CP-10 [SHALL].** LZ1 + LP1 + LZ2 + LP2 **shall not exceed Ltot**, which INFO1a gives in bits 14:15
  (Table 18, p.27).

  | Code | 0 | 1 | 2 | 3 |
  |---|---|---|---|---|
  | Ltot | 192 | 256 | 320 | 384 |

- **CP-11 [SHALL, from Table 30].** LZ1, LP1, LZ2 and LP2 are each "up to **Lmax** given in bits 16:17 of
  INFO1a". Table 18 defines Lmax = max{LZ1, LP1, LZ2, LP2}.

  | Code | 0 | 1 | 2 | 3 |
  |---|---|---|---|---|
  | Lmax | 128 | 192 | 256 | 320 |

- **CP-12 [SHALL].** A constellation shall not contain the zero point.
- **CP-13 [SHALL].** All constellation sets of non-zero size are listed first. LC1..LCn are non-zero and
  LC(n+1)..LC6 are zero.
- **CP-14 [SHALL].** The number of points in a constellation set shall not exceed 128. For what "points"
  means, see Q3; the recommended practice is to send 2·LC ≤ 128.
- **CP-15 [SHALL].** The digital modem shall design the modulation parameters on the assumption that the
  analogue modem transmits at the **desired power** when (prefilter output × G) has a **mean-square value of
  1**.
- **CP-16 [SHALL].** The CRC generator is the one in 10.1.2.3.2/V.34 (2.4).
- **CP-17 [SHALL].** CPd and CPd' sent as a group carry identical information (2.8).
- **CP-18 [SHOULD] (NOTE to 8.8.3).** The digital modem should design the precoder coefficients assuming
  that the analogue modem minimises the power at the precoder output symbol by symbol.
  - In 6.4.2 terms, the analogue modem picks the member u(n) of the equivalence class E(Ki) that minimises
    |x(n)|.

### 5.2 Table 30: the bit layout with every part present

**Part 1: always present (bits 0:50)**

| Bits (LSB:MSB) | Width | Content |
|---|---|---|
| 0:16 | 17 | Frame sync `11111111111111111` |
| 17 | 1 | Start bit 0 |
| 18 | 1 | Sequence type: **CPd = 0** (SUVd has 1) |
| 19 | 1 | 1 = modulus encoder parameters present |
| 20 | 1 | 1 = prefilter and precoder coefficients present |
| 21 | 1 | 1 = constellation sets present |
| 22:26 | 5 | **drn**: the selected analogue-to-digital (upstream) data signalling rate, an integer **0..19**. **[SHALL] drn = 0 indicates cleardown.** Rate = (drn + 17) × 8000/6, so drn 1 = 24 000 and drn 19 = 48 000 bit/s (the 6.1 range). Codes 20..31 are not defined. |
| 27:28 | 2 | Upstream trellis encoder: 0 = 16-state, 1 = 32-state, 2 = 64-state, 3 = reserved for ITU. "The digital modem receiver requires the analogue modem transmitter to use the selected trellis encoder." The encoders are those of V.34 with 2T delays replaced by 4T (6.4.4). |
| 29 | 1 | Extend E2u: 0 = don't extend, 1 = extend by 1 symbol. **[SHALL] 0 during rate renegotiation and fast parameter exchange.** Its effect is in 8.7.2: E2u "shall be extended by a single symbol if bit 29 of CPd is set". |
| 30:32 | 3 | Reserved for ITU: the digital modem sets them to 0 and the analogue modem does not interpret them |
| 33 | 1 | Acknowledge: 0 = this modem has not received CPu from the analogue modem, 1 = it has. Set → CPd'. |
| 34 | 1 | Start bit 0 |
| 35:50 | 16 | **4 × G, > 0**: four times the gain applied at the prefilter output, as unsigned Q0.16. G = N/262144. |

Word 0 as a u16, with bit 18 as bit 0 [DERIVED]:
`type | flags<<1 | drn<<4 | trellis<<9 | extend<<11 | reserved<<12 | ack<<15`

**Part 2: modulus encoder parameters (6 words; present if bit 19 = 1)**

| Bits | Width | Content | | Bits | Width | Content |
|---|---|---|---|---|---|---|
| 51 | 1 | start 0 | | 102 | 1 | start 0 |
| 52:59 | 8 | M0 | | 103:110 | 8 | M6 |
| 60:67 | 8 | M1 | | 111:118 | 8 | M7 |
| 68 | 1 | start 0 | | 119 | 1 | start 0 |
| 69:76 | 8 | M2 | | 120:127 | 8 | M8 |
| 77:84 | 8 | M3 | | 128:135 | 8 | M9 |
| 85 | 1 | start 0 | | 136 | 1 | start 0 |
| 86:93 | 8 | M4 | | 137:144 | 8 | M10 |
| 94:101 | 8 | M5 | | 145:152 | 8 | M11 |

- Mi is the modulus for upstream data-frame interval i, where i = 0..11 across the 12-symbol upstream data
  frame (Figure 1, 6.4.1).
- Each Mi is an unsigned 8-bit integer sent LSB first. Each word packs two moduli: the lower-numbered one in
  the low byte.

**Part 3: precoder and prefilter coefficients (4 + LZ1 + LP1 + LZ2 + LP2 words; present if bit 20 = 1)**

| Bits | Width | Content |
|---|---|---|
| 153 | 1 | Start bit 0 |
| 154:162 | 9 | **LZ1**: taps in the precoder feed-forward section, up to Lmax |
| 163:169 | 7 | Reserved for ITU (0) |
| 170 | 1 | Start bit 0 |
| 171:179 | 9 | **LP1**: taps in the precoder feedback section, up to Lmax |
| 180:186 | 7 | Reserved for ITU (0) |
| 187 | 1 | Start bit 0 |
| 188:196 | 9 | **LZ2**: taps in the prefilter feed-forward section, up to Lmax |
| 197:203 | 7 | Reserved for ITU (0) |
| 204 | 1 | Start bit 0 |
| 205:213 | 9 | **LP2**: taps in the prefilter feedback section, up to Lmax |
| 214:220 | 7 | Reserved for ITU (0) |
| 221 | 1 | Start bit 0 (the first coefficient word) |
| 222:237 | 16 | **z1(1)**, signed Q0.15 "(if LZ1 > 0)" |
| … | | z1(2) .. z1(LZ1), each behind a start bit |
| 221 + 17·LZ1 | 1 | Start bit 0 |
| 222 + 17·LZ1 : 237 + 17·LZ1 | 16 | **p1(1)**, signed Q1.14 |
| … | | p1(2) .. p1(LP1) |
| 221 + 17·(LZ1 + LP1) | 1 | Start bit 0 |
| … | | **z2(0) .. z2(LZ2 − 1)**, signed Q0.15. **Indexed from 0.** |
| 221 + 17·(LZ1 + LP1 + LZ2) | 1 | Start bit 0 |
| … | | **p2(1) .. p2(LP2)**, signed Q1.14 "(if LP2 > 0)" |

- The order on the wire is always z1, p1, z2, p2. Coefficient word k (counted from 0) has its start bit at
  `221 + 17·k`.
- The next part starts at bit 221 + α.
- Only z1 and p2 carry the "(if … > 0)" qualifier. That matches Table 18, where p1 and z2 are supported in
  every filter-section option and z1 and p2 are optional.
  - [INTERP] LP1 ≥ 1 and LZ2 ≥ 1 are expected in any CPd that carries part 3; see Q13.

The filters these coefficients define (6.4.2, p.12). The analogue modem transmits G·v(n).

```
x(n) = u(n) + Σ_{κ=1..LZ1} u(n−κ)·z1(κ) + Σ_{κ=1..LP1} x(n−κ)·p1(κ)        precoder
v(n) =        Σ_{κ=0..LZ2−1} x(n−κ)·z2(κ) + Σ_{κ=1..LP2} v(n−κ)·p2(κ)      prefilter
```

**Part 4: constellation sets (5 + ΣLC words; present if bit 21 = 1)**

| Bits | Width | Content |
|---|---|---|
| 221+α | 1 | Start bit 0 |
| 222+α : 225+α | 4 | Index (0..5) of the constellation for upstream intervals **0 and 6** |
| 226+α : 229+α | 4 | Index for intervals **1 and 7** |
| 230+α : 233+α | 4 | Index for intervals **2 and 8** |
| 234+α : 237+α | 4 | Index for intervals **3 and 9** |
| 238+α | 1 | Start bit 0 |
| 239+α : 242+α | 4 | Index for intervals **4 and 10** |
| 243+α : 246+α | 4 | Index for intervals **5 and 11** |
| 247+α : 254+α | 8 | Reserved for ITU (0) |
| 255+α | 1 | Start bit 0 |
| 256+α : 263+α | 8 | **LC1**: number of positive points in the 1st set |
| 264+α : 271+α | 8 | **LC2** (possibly zero) |
| 272+α | 1 | Start bit 0 |
| 273+α : 280+α | 8 | **LC3** (possibly zero) |
| 281+α : 288+α | 8 | **LC4** (possibly zero) |
| 289+α | 1 | Start bit 0 |
| 290+α : 297+α | 8 | **LC5** (possibly zero) |
| 298+α : 305+α | 8 | **LC6** (possibly zero) |
| 306+α | 1 | Start bit 0 |
| 307+α : 322+α | 16 | "Linear value" of the 1st (**smallest magnitude**) point of the 1st set (format: Q1) |
| 323+α | 1 | Start bit 0 |
| … | | the remaining points of set 1 in increasing magnitude, ending with the largest |
| 306+α+17·LC1 | 1 | Start bit 0 of set 2's first point. Then "possibly more constellations in the same format", one for each non-zero set. |

- The upstream **constellation frame** is 6 symbols (Figure 1, p.11; j = 0..5 twice). That is why intervals i
  and i+6 share one index, while the 12 moduli are all separate.
- The **trellis frame** is 4 symbols (k = i mod 4).
- Index v refers to "the (v+1)-th constellation set", i.e. the set whose size is LC(v+1) [INTERP, Q4].

**End of CPd**

| Bits | Width | Content |
|---|---|---|
| 306+α+β | 1 | Start bit 0 |
| 307+α+β : 322+α+β | 16 | CRC (2.4) |
| 323+α+β | 1 | Fill bit 0 |
| 324+α+β : … | | Fill 0s to the next multiple of 6 symbols (of D bits) |

- Consistency check: with every part present, W = 2 + 6 + (4 + α/17) + (5 + β/17).
  - The CRC start bit is then at 17 + 17·W = 306 + α + β, which matches the table.

### 5.3 Position calculator for any combination of parts

```
W = 2                                        # word 0 (flags/drn/...), word 1 (4G)
if bit19: W_mod  = W; W += 6                 # M0..M11, two per word
if bit20: W_len  = W; W += 4                 # LZ1, LP1, LZ2, LP2 (low 9 bits of each word)
          W_coef = W; W += LZ1+LP1+LZ2+LP2   # z1(1..), p1(1..), z2(0..), p2(1..)
if bit21: W_cs   = W; W += 5                 # 6 indices + reserved byte, then LC1..LC6
          W_pts  = W; W += LC1+...+LC6       # set 1 ascending, set 2 ascending, ...
start_bit(w) = 17 + 17*w ; info(w) = start_bit(w)+1 .. start_bit(w)+16
crc_start    = 17 + 17*W ; crc = crc_start+1 .. crc_start+16 ; first fill = crc_start+17
unpadded     = 17*W + 35
total        = ceil(unpadded / D) * D        # D of the carrying modulation (2.6)
```

Sizes:
- **Smallest CPd:** W = 2, so 69 bits before padding.
- **Largest CPd.** Ltot = 384, and 6 sets.

  | Reading of the 128-point limit (Q3) | W | Bits |
  |---|---|---|
  | 2·LC ≤ 128 | 2 + 6 + 4 + 384 + 5 + 384 = 785 | 13 380 |
  | LC ≤ 128 | 1169 | 19 908 |

- **On-air time:**

  | Rate | 13 380 bits | 19 908 bits |
  |---|---|---|
  | lowest TRN2d rate, 12 kbit/s (D = 9) | about 1.1 s | about 1.7 s |
  | top TRN2d rate, 40 kbit/s | about 0.33 s | -- |

  This matters for the 9.6.1.1.3 window (section 10.1).

### 5.4 Design constraints on CPd content (from other clauses)

These are not stated in 8.8, but a CPd that breaks them cannot be used.

- **DC-1 [SHALL, 6.4.1].** 2^K ≤ M = ∏_{i=0..11} Mi, where K is the number of bits per upstream data frame.
  - **[DERIVED]** From the 12-symbol frame at 8000 symbols/s: K = rate·12/8000 = **2·(drn + 17)**.
  - So K runs from 36 (24 000 bit/s) to 72 (48 000 bit/s) in steps of 2.
  - The upstream modulus encoder takes all K bits. There are no separate sign bits: the sign is folded into
    the modulus encoding, 6.4.1 steps 2-4.
- **DC-2 [DERIVED from 6.4.2].** The equivalence class E(Ki) must be non-empty for every Ki. The indices run
  −N/2 ≤ η < N/2, with N = 2·LC of the set assigned to interval i.

  | Intervals | Trellis position | Class | Requirement |
  |---|---|---|---|
  | i mod 4 ∈ {0, 1, 2} | k = 0, 1, 2 | η = Ki + z·Mi | N ≥ Mi |
  | i = 3, 7, 11 | k = 3 | η = 2Ki + 2z·Mi + parity | **N ≥ 2·Mi** |

  Useful precoding needs more than one member per class, so N should be larger in practice.
- **DC-3 [DERIVED, Table 18 bits 12:13].** INFO1a says which filter sections the analogue modem supports:

  | Code | Sections supported |
  |---|---|
  | 0 | p1, z2 |
  | 1 | z1, p1, z2 |
  | 2 | p1, p2, z2 |
  | 3 | z1, p1, p2, z2 |

  - Send LZ1 = 0 unless z1 is supported.
  - Send LP2 = 0 unless p2 is supported.
- **DC-4 [DERIVED].** Every index in 222+α..246+α must point to a non-empty set, i.e. be less than the number
  of non-zero LCs.
- **DC-5 [DERIVED].** The points of each set are positive, non-zero (CP-12) and strictly increasing. By the
  6.4.2 convention (negative indices for negative points), the full set is the mirror image
  a(−η−1) = −a(η) [INTERP].
- **DC-6 [DERIVED].** 4G ≠ 0. G is limited to (0, 0.25). The point scale (Q1) must be chosen so that G
  combined with the CP-15 normalisation lands in that range.
- **DC-7 [DERIVED, 9.11].** A CPd with drn = 0 is a cleardown. Such a CPd needs no optional parts.

### 5.5 Parsing CPd (loopback tests, or an analogue-side receiver)

1. Hunt for 17 ones followed by a 0.
2. Read word 0 (bits 18:33).
   - If bit 18 = 1, this is an SUVd (section 7). A receiver can dispatch on word 0 alone.
3. Read word 1 (4G).
4. If bit 19 is set, read 6 words (M0..M11).
5. If bit 20 is set:
   1. read 4 words and take the low 9 bits of each as LZ1, LP1, LZ2 and LP2 (ignore the 7 reserved bits);
   2. check each against Lmax and the sum against Ltot;
   3. read LZ1 + LP1 + LZ2 + LP2 coefficient words.
6. If bit 21 is set, read 5 header words (6 indices, the reserved byte, LC1..LC6), then ΣLC point words.
7. Check that the next start bit is 0. Read the 16 CRC bits LSB first, and check them against the CRC of every
   word's 16 information bits.
8. Skip fill up to the frame boundary. Treat any start bit that is not 0 as a framing error.
9. Repo: `v90::sequences::unframe(bits, blocks)` needs the word count before it runs. A CPd parser has to
   read the header words first, or take the word count as a callback (the `Finder::new(header_blocks,
   length_fn, …)` pattern already used for CP).

---

## 6. The R signals (8.8.4, p.43)

### 6.1 Rd and Rt, by reference to 8.6.4/V.90 (rendered V.90 p.35)

- **R-1.** Signal R is the **6-symbol** sequence of PCM codewords with sign pattern `+ + + − − −`,
  repeated. The leftmost sign goes first.
- **R-2.** R̄ is **4 repetitions** of the same 6 codewords with sign pattern `− − − + + +`, leftmost first.
  That is 24 symbols = 24T.
- **R-3 (NOTE, 8.6.4/V.90).** Neither R nor R̄ is differentially encoded, so a receiver must detect them
  whatever their polarity.
  - **[DERIVED]** Neither is scrambled either. They are fixed codewords, not encoder output.
- **R-4.** **Rd** uses, in each data-frame interval i = 0..5, the **highest-power PCM codeword of that
  interval's data-mode constellation "as passed in CP"**.
  - In V.92 that CP is **CPu**: bits 103:127 give each interval's constellation index, and the masks from bit
    136 give the member Ucodes.
  - Ucodes increase with linear magnitude (Table 1/V.90), so the highest-power codeword is the **largest
    Ucode** in the set.
- **R-5.** **Rt** uses, in each interval, the highest-power PCM codeword of the **training** constellation
  passed in **CPt**.
- Ri, the single codeword U_INFO in every interval, is a Phase 3 signal. It is not part of 8.8.
- **Sign pattern [DERIVED].** Rd starts on a data-frame boundary (9.8.1.1.1, 9.8.1.2.2). The sign of symbol n
  (n counted from the start of Rd, which equals the interval number n mod 6) is therefore
  `+ if (n mod 6) < 3 else −`, inverted for R̄.
  - The repo's V.90 digital pump does exactly this: `(interval < 3) ^ bar` in `v90/digital.rs`.
- **Transition shape [DERIVED].** 384T is 64 whole patterns, so the Rd→R̄d boundary reads
  `… + + + − − − | − − − + + + …`. The run of six "−" is the reversal a receiver detects.
- **Durations (clause 9):**

  | Signal | Duration | Repetitions |
  |---|---|---|
  | Rd | 384T = 48 ms | 64 × 6 |
  | R̄d | 24T = 3 ms | 4 × 6 |
  | Rt | 384T | 64 × 6 |
  | R̄t | 24T | 4 × 6 |

  - Rd shall begin on a data-frame boundary (9.8.1.1.1, 9.8.1.2.2).
  - Rt is sent only after a silence request (9.8.1.1.4, 9.8.1.1.5).

### 6.2 Rf and R̄f (8.8.4, new in V.92)

The rendered text was checked in the zoomed crop `v92_88_p43_rf_zoom.png`.

- **RF-1.** Rf is sent by repeating the **12-symbol** sequence of PCM codewords with sign pattern
  **`+ + − − + + − − + + − −`**, leftmost sign first.
  - The printed sentence reads "Rf is transmitted by 0 repeating …". The stray "0" is a typo (Q9).
- **RF-2.** R̄f is **2 repetitions** of the 12-symbol sequence, with the same codewords and sign pattern
  **`− − + + − − + + − − + +`**, leftmost first. That is 24 symbols = 24T.
- **RF-3.** The codewords are "the highest power PCM codeword from the data mode constellation of each data
  frame interval as passed in **CPu**". This is the same codeword choice as Rd.
  - In an FPE, Rf goes out **before** any new CPu is exchanged, so these are the codewords of the data mode
    currently running.
- **RF-4 [INTERP, inherited from the V.90 R definition, not restated in 8.8.4].** Rf and R̄f are not scrambled
  and not differentially encoded. A receiver must detect them in either polarity.
- **Durations (9.9.1.1.1, 9.9.1.2.2) [SHALL]:**
  - **Rf lasts 384T** (32 repetitions of the 12-symbol pattern).
  - **R̄f lasts 24T** (2 repetitions).
  - **Rf begins on a data-frame boundary.**
- **Generator [DERIVED].** Count n from the first symbol of Rf. n does **not** come from a global symbol
  counter taken mod 12, because the boundary is only guaranteed to be a 6-symbol one.

  ```
  ucode(n)  = Umax[n mod 6]      # largest Ucode in the CPu data-mode constellation of interval (n mod 6)
  sign(n)   = + if (n mod 4) in {0, 1} else −      # Rf,  n = 0 .. 383
  sign(m)   = − if (m mod 4) in {0, 1} else +      # R̄f, m = 0 .. 23, counted from the start of R̄f
  ```

- **Transition shape [DERIVED].** 384 = 32 × 12, so the Rf→R̄f boundary reads `… + + − − | − − + + …`. The
  run of four "−" is the reversal to detect.
  - The last sign of R̄f is "+".
  - Right after R̄f, the FPE resets the encoder state to zero anyway (GEN-10).
- **Telling Rd from Rf [DERIVED].** Over 12 symbols, Rd is `+++−−−+++−−−` (sign period 6) and Rf is
  `++−−++−−++−−` (sign period 4). They stay distinguishable in either polarity.
  - The analogue modem must make this distinction. While it is initiating an FPE, a received **Rd** means an
    RR and takes priority (9.9.2.1.2).
- **Receive-side counterparts (what the digital modem listens for):**
  - Ru, the upstream R of 8.5.5;
  - **RM** and **RM'**, 12-symbol patterns of modulus-encoder outputs (Tables 25/26, p.36-37):
    - RM: K = M−1, M−1, 0, 0, M−1, M−1, 0, 0, M−1, M−1, 0, and u11 = 0.
    - RM': K = 0, 0, M−1, M−1, 0, 0, M−1, M−1, 0, 0, M−1, M−1.
    - Both are sent with the data-mode parameters and are trellis encoded.

---

## 7. SUVd and SUVd' (8.8.5 and Table 31, pp.43-44)

### 7.1 Narrative requirements

- **SUV-1 [SHALL].** SUVd is a short information sequence.
  - It is scrambled.
  - It uses the corresponding TRN2d modulation in training and RR, and the preceding data-mode modulation in
    FPE.
- **SUV-2 [SHALL].** The differential encoder is initialised from the last transmitted sign bit of the
  preceding sequence (GEN-9). FPE overrides this by resetting to zero first (GEN-10).
- **SUV-3.** An SUVd with the acknowledge bit set is written SUVd'.
- **SUV-4 [SHALL].** Bit 0 goes first.
- **SUV-5 [SHALL].** The CRC is the one in 10.1.2.3.2/V.34.
- **SUV-6 [SHALL].** SUVd and SUVd' sent as a group carry identical information.

### 7.2 Table 31 layout (p.44)

| Bits (LSB:MSB) | Width | Content |
|---|---|---|
| 0:16 | 17 | Frame sync `11111111111111111` |
| 17 | 1 | Start bit 0 |
| 18 | 1 | Sequence type: **SUVd = 1** (CPd has 0) |
| 19:31 | 13 | Reserved for ITU. The table says "set to 0 by the analogue modem and not interpreted by the digital modem", a copy of Table 27 (Q11). **Implement as: the digital modem sends 0, and receivers ignore these bits.** |
| 32 | 1 | 1 = **a silent period is requested**. "This may be used during rate renegotiation (see 9.8.1.1)" [MAY]. |
| 33 | 1 | Acknowledge: 0 = this modem has not received CPu from the analogue modem, 1 = it has. Set → SUVd'. |
| 34 | 1 | Start bit 0 |
| 35:50 | 16 | CRC over bits 18:33 |
| 51 | 1 | Fill bit 0 |
| 52:… | | Fill 0s to the next multiple of 6 symbols |

- SUVd is **52 bits** before padding (W = 1). After padding it is ⌈52/D⌉ data frames:

  | D | Frames | Symbols |
  |---|---|---|
  | 9 (12 kbit/s) | 6 | 36 |
  | 30 (40 kbit/s) | 2 | 12 |
  | 42 (56 kbit/s, FPE) | 2 | 12 |

- Word 0 as a u16, bit 18 = bit 0 [DERIVED]: `1 | silence<<14 | ack<<15`. Test vectors are in 2.4.
- Word 0 has the same dispatch fields as CPd's word 0: bit 18 is the type and bit 33 the acknowledge.
- In CPd, bit 32 is reserved (0). **Only SUVd carries the silence request.**
- Table 31 has **no drn field**, even though 9.11 talks of drn = 0 in SUVd (Q17).
- For comparison, SUVu (Table 27, pp.37-38) also has:
  - bit 26 ("please wait for my CPu before sending CPd"; **the digital modem is not required to comply**);
  - bits 27:31, the analogue modem's measured RMS of G × prefilter output as 20·log10(L), in signed Q2.2
    (5 bits; the value 16 = −4.00 means "not measured").

  The digital modem uses bits 27:31 to check the CP-15 power assumption.

### 7.3 How bits 32 and 33 are used (clause 9)

- **Phase 4 and RR without silence (9.6.1.1.2).** Once a CPu has been received, every later SUVd **and** CPd
  goes out with bit 33 = 1.
- **RR with silence** (bit 32 set in either SUVd or SUVu; 9.8.1.1.2-9.8.1.1.5):
  - The digital modem sends SUVd with **bit 33 set** (9.8.1.1.3), although no CPu has been exchanged yet.
  - Figure 16 shows SUVd SUVd SUVd' Ed when the digital modem asked for silence. Figure 18 shows SUVd'
    SUVd' SUVd' Ed when the analogue modem asked.
  - Here bit 33 acknowledges the SUV exchange, not a CPu. Table 31's "received CPu" wording is narrower than
    this usage (Q10).
- **FPE (9.9.1.1.2, 9.9.1.2.3).** SUVd goes out with **bit 32 clear**.
- **After the silence** (9.8.1.1.4/5), the digital modem sends Rt, R̄t, then SUVd, and continues with
  9.6.1.1.2. [INTERP] That SUVd has bit 32 clear, and bit 33 clear until the new CPu arrives (Figures 16-18
  show a plain "SUVd", then CPd, then SUVd').

---

## 8. TRN2d (8.8.6 → 8.6.5/V.90, rendered V.90 p.36)

8.8.6 says only "As defined in 8.6.5/V.90".

### 8.1 What 8.6.5/V.90 requires

- **TRN-1 [SHALL].** TRN2d is scrambled binary ones fed to the encoder of 5.4/V.90.
- **TRN-2 [SHALL].** The constellation set is the one passed in **CPt**. In V.92 that is Table 23 with type
  bits 19:20 = 0, drn giving the rate (drn + 8) × 8000/6, and the interval indices and masks as in CPu.
- **TRN-3 [SHALL].** The scrambler, differential encoder and spectral-shape filter memory are initialised
  to zero before TRN2d.
- **TRN-4 [SHALL].** The symbols of the first data frame have the magnitudes that come from mapping the first
  D scrambled ones after the scrambler is zeroed. They are identical for every ld.
- **TRN-5 [SHALL].** Permitted K and S are those of **Table 17/V.90**: K = 6..24, and S = 3..6 for every K.
  - Rate = (K + S)·8000/6.
  - K = 6: 12 to 16 kbit/s. Each step of K adds 1 1/3 kbit/s. K = 24: 36 to 40 kbit/s.
  - CPt drn 1..22 covers 12 000 .. 40 000 bit/s.
- **TRN-6 [SHALL].** TRN2d is an integer multiple of 6 symbols long.

### 8.2 Durations in V.92

| Where | Duration | Source |
|---|---|---|
| Phase 4 | **at least 2040T** (255 ms, 340 frames) | 9.6.1.1.1 [SHALL] |
| RR | **up to 16008T** (2.001 s, 2668 frames), followed by SUVd sequences | 9.8.1.1.2 [SHALL] |

- V.90's RR TRN2d was optional and at most 2000 ms (9.6.1.1.1/V.90). **In V.92 the digital modem "shall then
  transmit TRN2d for up to 16008T"** [INTERP: mandatory but of any length up to 16008T, including very
  short].
- 2040, 16008 and 384 are all multiples of 6.

### 8.3 The V.90 8.6 intro (rendered V.90 p.33; not repeated in V.92)

- The digital modem's Phase 4 signals "may be spectrally shaped".
- In initial train or retrain, TRN2d, MP and Ed use the shaping parameters defined by **CPt**.
- In RR, TRN2d, MP and Ed use the shaping parameters **of the preceding data mode**, together with the **K
  previously derived from CPt**.
- **[SHALL]** B1d and the data after it use the shaping parameters defined by **CP** (V.92: CPu).
- **[INTERP, Q7]** In V.92, "corresponding TRN2d modulation" extends this rule to SUVd and CPd, which take the
  place of MP.

---

## 9. Silence (9.8.1.1.3) and circuit handling

- **SIL-1 [SHALL].** In an RR with silence, the digital modem generates silence by sending **PCM codewords
  whose magnitude is Ucode 0**.
  - Linear value: µ-law 0, A-law 8 (Table 1/V.90).
  - The sign is not specified (Q12).
- **SIL-2 [SHALL].** The digital modem **keeps data-frame alignment** during the silence, so the Rt that
  follows starts on a frame boundary.
- Duration:
  - The silence runs from the end of Ed until the digital modem starts Rt (9.8.1.1.4/5). There is no fixed
    length.
  - The analogue modem sends TRN2u for up to 8004T in this period (9.8.2.1.5/6).
- Circuits, digital side:

  | When | Action | Source |
  |---|---|---|
  | initiating RR or FPE | circuit 106 OFF | 9.8.1.1.1, 9.9.1.1.1 |
  | responding to Ru or RM | clamp 104 to binary one | 9.8.1.2.1, 9.9.1.2.1 |
  | after Ed and B1d | 106 follows 105 | 9.6.1.1.5 |
  | after B1u | unclamp 104, 109 ON | 9.6.1.1.6 |
  | initiating a retrain | 106 OFF, 104 clamped | 9.7.1.1 |

---

## 10. Where clause 9 uses these signals (digital-modem side)

This is a cross-reference so that the 8.8 signals can be placed in order. The clause 9 digest
(`spec-renegotiation-fpe.md`, `spec-phase4-procedures.md`) is the authority on procedure. Everything here comes
from the rendered pp.56-65.

### 10.1 Phase 4 (9.6.1; Figures 12, 13, 14)

1. **9.6.1.1.1 [SHALL].**
   - Send TRN2d for at least 2040T.
   - When ready to receive CPu, condition the receiver for SUVu and send SUVd sequences.
2. **9.6.1.1.2 [SHALL].**
   - After receiving an SUVu, send **a single CPd**, then more SUVd.
   - After receiving a CPu, send every later CPd and SUVd with the acknowledge bit set.
3. **9.6.1.1.3 [SHALL].**
   - Consider every CPu and SUVu received up to and including the **whole** CPu/SUVu that is received after
     (end of our CPd + 100 ms + round-trip delay). Figure 14 labels this window "RTD + 100 ms".
   - If none of them has the acknowledge bit set, send **repeated CPd** sequences.
   - The round-trip delay is the digital modem's own estimate, RTDEd (clause 4 abbreviation).
4. **9.6.1.1.4 [SHALL].** Once the digital modem has **sent** a CPd or SUVd with the acknowledge bit set
   **and received** either a CPu/SUVu with the acknowledge bit set or E2u, it completes the current CPd/SUVd
   and sends Ed.
5. **9.6.1.1.5 [SHALL].**
   1. After Ed, send B1d at the negotiated rate, using the data-mode constellation parameters received in
      CPu.
   2. Enable circuit 106 to follow circuit 105.
   3. Start data transmission under clause 5.
6. **9.6.1.1.6 [SHALL].**
   - After E2u, condition the receiver for B1u. In an FPE, expect **FB1u** (48 frames, 8.7.7) and then B1u.
   - After B1u, unclamp 104, turn 109 on and demodulate.
7. **9.6.1.2 [MAY]/[SHALL].**
   - The digital modem may retrain at any time in Phase 4 (9.7.1.1).
   - If it detects Tone A, it shall respond per 9.7.1.2.
8. **9.6.1.2.1 [SHALL].** If B1u has not arrived within **20 s + 6 round-trip delays** of the end of INFO1a,
   retrain per 9.7.1.1. (V.90 allowed 15 s + 5 RTD.)

Analogue-side facts that shape the digital side:
- The analogue modem sends TRN2u for at least 12000T, or until it receives an SUVd, before it sends SUVu
  (9.6.2.1.1).
- After receiving an SUVd, it sends a single CPu (9.6.2.1.2).
- It may ask the digital modem to wait for its CPu (SUVu bit 26). **The digital modem is not required to
  comply** [MAY].
- Figure 13 shows the case where CPu comes first: SUVd ×4, SUVd', CPd', SUVd' ×2, Ed.
- Figure 14 shows a lost CPu:
  1. The digital modem sends CPd, then plain SUVd, until a CPu' arrives.
  2. The analogue modem, seeing no acknowledgement, repeats CPu'.
  3. The digital modem then sends SUVd' and Ed.

### 10.2 Rate renegotiation (9.8, 9.8.1; Figures 15-18)

General requirements:
- **[MAY]** RR can start at any time in data mode, and the rate and other parameters may change.
- RR can also retrain the analogue modem's echo canceller, or its precoder and prefilter.
- **[SHALL]** Both modems keep data-frame synchronisation throughout RR.
- **[SHALL]** An RR is initiated only on a data-frame boundary.
- **[SHALL]** A modem responds to an RR only on a data-frame boundary.

Initiating (9.8.1.1):
1. **9.8.1.1.1 [SHALL].**
   - Turn 106 OFF.
   - Condition the receiver for Ru, R̄u and SUVu.
   - Send **Rd for 384T**, then **R̄d for 24T**. Rd starts on a data-frame boundary.
2. **9.8.1.1.2 [SHALL].**
   - Send **TRN2d for up to 16008T**, then SUVd sequences.
   - On receiving an SUVu, continue with 9.6.1.1.2, **unless bit 32 is set in either the SUVd or the SUVu**.
3. **9.8.1.1.3 [SHALL]** (the silence branch).
   - Send SUVd with **bit 33 set**.
   - On receiving an SUVu with bit 33 set, or E2u, complete the current SUVd, then send **Ed followed by
     silence** (section 9).
4. **9.8.1.1.4 [SHALL].** If SUVu bit 32 was set:
   1. wait for an SUVu with bit 32 clear;
   2. send **Rt for 384T**, **R̄t for 24T**, then SUVd;
   3. continue with 9.6.1.1.2.
5. **9.8.1.1.5 [MAY].** If SUVu bit 32 was clear (only the digital modem asked for silence), the digital
   modem may either:
   - send Rt 384T + R̄t 24T + SUVd straight away, or
   - wait for another SUVu.

   It then continues with 9.6.1.1.2.

Responding (9.8.1.2):
1. **9.8.1.2.1 [SHALL].** After detecting Ru, clamp 104 to binary one and watch for the Ru→R̄u transition.
2. **9.8.1.2.2 [SHALL].** After that transition, send **Rd 384T** then **R̄d 24T**, starting on a
   data-frame boundary.
3. **9.8.1.2.3 [SHALL].** Continue with 9.8.1.1.2.

Analogue-side durations:
- TRN2u lasts up to 16008T, and may stop after 2400T or when SUVd arrives (9.8.2.1.2, 9.8.2.2.3). Figures
  15-18 print "≥2400T".
- After the silence exchange, the analogue modem sends E2u, then TRN2u (9.8.2.1.4):
  - If SUVd bit 32 was clear: TRN2u for up to 8004T, then SUVu with bit 32 clear (9.8.2.1.5).
  - If SUVd bit 32 was set: SUVu with bit 32 clear once it receives Rt or has sent 8004T of TRN2u
    (9.8.2.1.6).
- Figures 16, 17 and 18 label this TRN2u segment "≥4008T", "≥2400T" and "≤8004T" respectively (Q14).

### 10.3 Fast parameter exchange (9.9, 9.9.1; Figure 19)

General requirements:
- **[MAY]** An FPE can start at any time in data mode.
- **[SHALL]** Both modems keep data-frame synchronisation.
- **[SHALL]** An FPE is initiated only on a data-frame boundary.
- **[SHALL]** A modem responds to one only on a data-frame boundary.

Initiating (9.9.1.1):
1. **9.9.1.1.1 [SHALL].**
   - Turn 106 OFF.
   - Condition the receiver for RM, RM' and SUVu.
   - Send **Rf for 384T**, then **R̄f for 24T**. Rf starts on a data-frame boundary.
2. **9.9.1.1.2 [SHALL].**
   1. Initialise the scrambler, differential encoder and spectral-shaping filter memory to **zero**.
   2. Send SUVd with **bit 32 clear**, in data-mode modulation, **without waiting** for RM.
   3. After detecting RM and RM', condition the receiver for SUVu and continue with 9.6.1.1.2: a single
      CPd, the acknowledge handshake, Ed, B1d.
   4. **If Ru is detected** (the far end started an RR), continue with 9.8.1.2.1. **RR takes priority.**

Responding (9.9.1.2):
1. **9.9.1.2.1 [SHALL].** After detecting RM, clamp 104 to binary one and watch for the RM→RM'
   transition.
2. **9.9.1.2.2 [SHALL].** After that transition, send **Rf 384T** then **R̄f 24T**, starting on a
   data-frame boundary.
3. **9.9.1.2.3 [SHALL].** Zero the scrambler, differential encoder and shaping memory. Send SUVd with bit 32
   clear, and continue with 9.6.1.1.2.

Also:
- In FPE, CPd, SUVd and Ed use **data-mode modulation**, and CPd bit 29 **shall be 0**.
- The analogue modem answers with E2u, **FB1u** and B1u (9.6.2.1.5).
- Figure 19 (initiated by the analogue modem) reads:
  - analogue modem: RM, RM', SUVu, SUVu, SUVu, CPu, SUVu, SUVu', SUVu', E2u, FB1u, B1u;
  - digital modem: Rf, R̄f, SUVd, SUVd, CPd, SUVd', SUVd', Ed, B1d.

### 10.4 Cleardown (9.11, p.69)

- **[SHALL]** A connection is ended with the cleardown procedure.
- Cleardown is signalled by drn = 0 "in a rate sequence". The text says "in either SUVu … or SUVd", but SUV
  sequences carry no drn. For the digital modem, the field is **CPd bits 22:26** (Q17).
- **[MAY]** Cleardown may be signalled whenever a rate sequence is sent.
- **[SHALL]** To clear down from data mode, a modem starts either an RR or an FPE and sends a rate sequence
  with drn = 0.

---

## 11. Timers, durations and tolerances

| Item | Value | Clause | Notes |
|---|---|---|---|
| T | 1/8000 s | 5/V.92 → 5.2/V.90 | downstream; upstream is also 8000 (6.2) |
| Data frame (downstream) | 6T | 5.4/V.90 | D = K + S bits |
| B1d | 48 frames = 288T (36 ms) | 8.6.1/V.90 | scrambler etc. zeroed first |
| Ed | 2 frames = 12T (1.5 ms) | 8.8.2 | |
| CPd, SUVd | padded to a whole number of frames | Tables 30, 31 | SUVd = 52 bits before padding; CPd = 17·W + 35 |
| TRN2d, Phase 4 | ≥ 2040T (255 ms) | 9.6.1.1.1 | multiple of 6T (8.6.5/V.90) |
| TRN2d, RR | ≤ 16008T (2.001 s) | 9.8.1.1.2 | |
| Rd, Rt | 384T (48 ms) = 64 × 6 | 9.8.1.1.1, 9.8.1.1.4, 9.8.1.2.2 | Rd starts on a frame boundary |
| R̄d, R̄t | 24T (3 ms) = 4 × 6 | 8.6.4/V.90, 9.8.1.x | |
| Rf | 384T = 32 × 12 | 9.9.1.1.1, 9.9.1.2.2 | starts on a frame boundary |
| R̄f | 24T = 2 × 12 | 8.8.4, 9.9.1.x | |
| CPd acknowledgement window | end of our CPd + 100 ms + RTD, through the next whole CPu/SUVu | 9.6.1.1.3 | no ack seen → repeat CPd |
| B1u deadline | 20 s + 6 RTD after the end of INFO1a | 9.6.1.2.1 | else retrain (9.7.1.1) |
| Silence (RR) | Ucode 0 magnitudes, frame alignment kept, length open | 9.8.1.1.3 | |
| Analogue TRN2u, Phase 4 | ≥ 12000T before SUVu, unless SUVd arrives first | 9.6.2.1.1 | |
| Analogue TRN2u, RR | ≤ 16008T; may stop after 2400T or on SUVd | 9.8.2.1.2 | |
| Analogue TRN2u after silence | ≤ 8004T | 9.8.2.1.5/6 | the figures print ≥4008T / ≥2400T / ≤8004T |
| Ru, RM (analogue) | 384T; R̄u, RM' 24T | 9.8.2.1.1, 9.9.2.1.1 | |
| Retrain silence | 70 ± 5 ms | 9.7.1.1, 9.7.1.2 | Tone A must be heard for > 50 ms before responding |

---

## 12. Requirement checklist

### 12.1 SHALL

1. (8.8.1 → 8.6.1/V.90) B1d:
   - 48 frames of scrambled ones in the selected data-mode constellation;
   - scrambler, differential encoder and shaping memory zeroed first;
   - first-frame magnitudes = the first D scrambled ones, the same for any ld;
   - K and S from Table 2/V.90;
   - data-mode shaping (from CPu).
2. (8.8.2) Ed is 2 frames of scrambled zeros: TRN2d modulation in training and RR, data-mode modulation in
   FPE.
3. (8.8.3) CPd:
   - The mandatory part is bits 0:50. Optional parts are flagged by bits 19, 20 and 21, and the bits of an
     absent part are removed.
   - It ends with the CRC, at least one fill bit, and fill to a multiple of 6 symbols.
   - It is scrambled: TRN2d modulation in training and RR, data-mode modulation in FPE.
   - In training and RR, the differential encoder starts from the last transmitted sign bit.
   - Bit 0 goes first.
4. (8.8.3) Limits:
   - LZ1 + LP1 + LZ2 + LP2 ≤ Ltot (INFO1a 14:15);
   - each of them ≤ Lmax (INFO1a 16:17);
   - no zero point in any constellation;
   - non-empty sets listed first;
   - ≤ 128 points per set.
5. (8.8.3) Design the parameters so that mean-square(G × prefilter output) = 1 gives the desired transmit
   power.
6. (8.8.3) CRC per 10.1.2.3.2/V.34. A group of CPd/CPd' carries identical information.
7. (Table 30)
   - drn = 0 indicates cleardown.
   - The analogue transmitter must use the trellis encoder selected in bits 27:28.
   - Bit 29 = 0 in RR and FPE.
   - 4G > 0.
   - Reserved bits are 0.
8. (8.8.4 → 8.6.4/V.90) Rd and Rt as defined in V.90. Neither R nor R̄ is differentially encoded, so the
   receiver must be polarity-agnostic.
9. (8.8.4) Rf and R̄f:
   - Rf is the 12-symbol `++−−++−−++−−` pattern, repeated.
   - R̄f is 2 × the `−−++−−++−−++` pattern.
   - Both use the highest-power codeword of each interval's data-mode constellation from CPu.
10. (8.8.5) SUVd:
    - It is scrambled: TRN2d modulation in training and RR, data-mode modulation in FPE.
    - The differential encoder starts from the last transmitted sign bit.
    - Bit 0 goes first.
    - CRC per 10.1.2.3.2/V.34.
    - A group carries identical information.
    - Reserved bits are 0.
11. (8.8.6 → 8.6.5/V.90) TRN2d:
    - scrambled ones in the CPt constellation;
    - scrambler, differential encoder and shaping memory zeroed first;
    - first-frame magnitudes fixed;
    - K and S from Table 17/V.90;
    - length a multiple of 6 symbols.
12. (clause 5 → 5.3/V.90) The scrambler uses GPC.
13. (8.6 intro/V.90) B1d and later data use CP(u) shaping.
14. Clause 9, digital side:
    - TRN2d ≥ 2040T (Phase 4) or ≤ 16008T (RR);
    - Rd/Rf 384T starting on a frame boundary, R̄d/R̄f 24T;
    - silence = Ucode 0 with frame alignment kept;
    - FPE resets before SUVd;
    - the SUVd bit 32/33 rules;
    - a single CPd, repeated only without an acknowledgement;
    - Ed only after completing the current sequence;
    - the retrain deadline.

### 12.2 SHOULD and MAY

- **[SHOULD]** (8.8.3 NOTE) Design the precoder assuming the analogue modem minimises the precoder output
  power symbol by symbol.
- **[MAY]** (Table 31 bit 32; 9.8.1.1) Request a silent period in RR with SUVd bit 32.
- **[MAY]** (Table 27 bit 26) Ignore an analogue request to wait for CPu before sending CPd.
- **[MAY]** (9.8.1.1.5) After a digital-only silence request, send Rt/R̄t/SUVd at once or wait for another
  SUVu.
- **[MAY]** (9.6.1.2, 9.8, 9.9) Retrain at any time in Phase 4. Start an RR or FPE at any time in data mode.
- **[MAY]** (9.11) Signal cleardown in any rate sequence.
- **[MAY]** (8.6 intro/V.90) Phase 4 signals may be spectrally shaped.

---

## 13. Implementation notes

### 13.1 What is new compared with V.90 (digital modem, Phase 4 / RR)

- **CPd replaces MP** (Table 16/V.90) whenever the upstream is PCM.
  - MP carried V.34 QAM upstream parameters: a rate mask, trellis, non-linear (Θ), shaping and 3 complex
    precoder taps.
  - CPd carries instead:
    - the upstream PCM rate (drn, 24-48 kbit/s);
    - the trellis choice and the E2u extension bit;
    - the gain G;
    - 12 moduli;
    - up to Ltot (≤ 384) real-valued precoder and prefilter taps;
    - up to 6 upstream constellations given as linear point values.
  - CPd has **variable length with presence flags**. MP had exactly two fixed layouts.
  - The framing is unchanged: 17-one sync, 16-bit words behind start bits, V.34 CRC, fill to 6 symbols. That
    makes `crates/datapump/src/v90/sequences.rs` (`frame`, `unframe`, `put`, `get`, `mp_bits`) the natural
    base.
- **SUVd is new.** It is a 1-word handshake sequence (bit 18 = 1). It agrees when CP is exchanged, carries the
  acknowledgement, and requests silence in RR. V.90 had no equivalent, because repeating MP/MP' carried the
  handshake.
- **The Phase 4 handshake changed.**

  | Step | V.90 (9.4.1.3-9.4.1.4) | V.92 |
  |---|---|---|
  | Start | MP within 2000 ms of TRN2d | SUVd once ready for CPu |
  | Parameters | MP repeated until CP arrives, then MP' | **one** CPd after the first SUVu, then SUVd again |
  | Repetition | MP is always repeated | CPd repeated only if no acknowledgement arrives within 100 ms + RTD (9.6.1.1.3) |
  | Deadline | 15 s + 5 RTD | 20 s + 6 RTD |

- **RR:**
  - TRN2d is now mandatory, up to 16008T. It used to be optional, ≤ 2000 ms.
  - The digital modem listens for Ru/R̄u. V.90's upstream was V.34-style (S/S̄).
  - There is a new silence branch: SUV bit 32, then Ed, Ucode-0 silence, Rt/R̄t, SUVd.
- **Rf/R̄f and the whole FPE procedure are new:**
  - a 12-symbol R with sign period 4;
  - CPd/SUVd/Ed carried in the **current data-mode modulation**;
  - scrambler, differential encoder and shaper reset to zero before SUVd;
  - FB1u expected upstream before B1u.
- **Unchanged:** B1d, TRN2d and Rd/Rt, all by reference to V.90. Ed keeps its V.90 payload.

### 13.2 Pitfalls

1. **Bit order.** Every integer goes out **LSB first**: every coefficient, Mi, LC, index, drn, 4G and the
   CRC. Signed Q values go out as two's-complement integers with the sign bit last.
2. **z2 is indexed from 0; z1, p1 and p2 from 1.** z2(0) is transmitted, not implied to be 1.
3. **9-bit length fields.** LZ1, LP1, LZ2 and LP2 each share a 16-bit word with 7 reserved zeros. Mask them
   with `& 0x1FF`.
4. **Constellation indexing.**
   - 6 indices for 12 intervals (i and i+6 share one), but 12 moduli.
   - The points are **positive magnitudes in increasing order**, with the smallest first. That is the
     **opposite** of V.90's mapper labelling (5.4.4/V.90, where label 0 is the largest code).
   - The N used by 6.4.2 is 2·LC.
5. **Absent parts shift everything after them.** Never hard-code a Table 30 bit number beyond 50; use the
   5.3 calculator.
6. **CRC scope.** The CRC covers the word payloads only. It excludes sync, start bits and fill, is seeded
   with 0xFFFF and is not inverted.
7. **Groups.** CPd' differs from CPd only in bit 33 and the CRC. Everything else in a group must be
   bit-identical.
8. **Padding unit.** Pad to whole data frames of D bits, not to 6 bits.
   - In FPE, D is the data-mode D.
   - In training, D is the Table 17 D from CPt.
   - In RR, D is K_CPt plus the data-mode S (2.6, Q7).
9. **Frame boundaries.**
   - Rd, Rt and Rf must start on a data-frame boundary.
   - Keep one 6-symbol frame counter running through data, R, R̄, TRN2d, the sequences, Ed, silence and
     B1d.
   - Count Rf's period-4 signs from Rf's first symbol, **not** from a global symbol count mod 12.
10. **R polarity.** R signals are neither scrambled nor encoded. Detect them in either polarity, and look for
    the R→R̄ transition (a run of 6 equal signs for R, 4 for Rf), not for an absolute sign.
11. **Priority.**
    - An FPE initiator that detects **Ru** must switch to the RR response path (9.9.1.1.2).
    - An RR initiator should simply keep waiting for Ru if it sees RM. The analogue modem will switch to RR
      when it sees Rd (9.9.2.1.2) [DERIVED].
12. **Long CPd versus the 9.6.1.1.3 window.** The window starts at the **end** of our CPd. The whole CPu or
    SUVu that straddles the deadline still counts. A maximal CPd at 12 kbit/s can be longer than a second.
13. **The encoder state after R̄t.** The first SUVd after R̄t takes its differential reference from R̄t's last
    sign ("+"), not from the encoder memory before the silence (2.7).
14. **The descrambler after the FPE reset.** Either reset the receive descrambler in step, or accept that the
    first SUVd after an FPE reset may be lost (2.5).
15. **Silence and DIL-slip sensitivity** (from the project notes on VoIP jitter slips and the DIL
    slip-relocation weakness).
    - A long Ucode-0 silence followed by an Rt that must land on a frame boundary is exactly where the
      softphone path can lose frame alignment.
    - Plan a frame-alignment check when Rt/R̄t is received, and prefer the RR without silence unless the far
      end asks for it.
16. **VoIP round trip (~1.5 s each way on the test line).** The 9.6.1.1.3 window depends on RTD. With a
    large RTDEd, the digital modem will be sending SUVd for a long time before it may decide to repeat CPd.
    Size buffers and timers for that.

### 13.3 Ambiguities and open questions

- **Q1. Format of the constellation "linear value" (Table 30).**
  - The field is 16 bits, but V.92 (11/2000) gives no signedness or scale.
  - The points are positive magnitudes, so unsigned is the likely reading.
  - A plausible scale is the linear column of Table 1/V.90: µ-law up to 32124, A-law up to 32256, which
    fits in 16 bits.
  - G (4G in Q0.16, so G < 0.25) and CP-15 then fix the transmit power. Only the ratio matters to the
    analogue modem, so any consistent scale works, but both ends must agree.
  - **Settle this against a decoded CPd from a real V.92 server capture before fixing the scale.**
- **Q2. What an absent part means.**
  - V.92 does not say whether an absent part means "keep the previous values".
  - It also does not say that the first Phase 4 CPd must carry all parts. It must in practice, because
    there are no previous values.
  - Proposed rule:
    - In initial training or after a retrain, send all three parts.
    - In RR and FPE, omit a part only when it is unchanged.
    - On reception, treat a missing part as "unchanged".
- **Q3. "The number of points in a constellation set shall not exceed 128."**
  - 6.4.2 calls the set size N = 2·LC, which supports 2·LC ≤ 128 (LC ≤ 64). The other V.92 digests read it
    this way.
  - The 8-bit LC field allows the reading LC ≤ 128.
  - **Send 2·LC ≤ 128, which satisfies both readings, and accept LC up to 128 on reception.**
- **Q4. Index-to-set mapping.** Index v presumably selects the (v+1)-th listed set (LC(v+1)). That matches
  CPu/CPt, where "the constellations that are sent are indexed from 0", but Table 30 does not say it.
- **Q5. Purpose of bit 29 (extend E2u by one symbol).**
  - It is not explained. It presumably lets the digital modem move the upstream 12-symbol frame phase by 1T
    at the end of initial training.
  - The digital receiver must expect B1u, and the upstream frame boundary, one symbol later when it sets
    this bit.
  - The bit is forbidden in RR and FPE, where frame synchronisation must be kept.
- **Q6. Differential encoder initialisation with shaping (Sr > 0).**
  - "The last transmitted sign bit" only defines the state for Sr = 0.
  - Proposed rule: keep the whole encoder and shaper state running across TRN2d → SUVd → CPd → Ed. Reset it
    only where 8.6.5/V.90, 8.6.1/V.90 or 9.9.1.x require.
  - After R̄t, set the Sr = 0 reference to "+". For Sr > 0, set the t/p' memory as if the last sent sign were
    "+", and zero the shaper filter.
  - Confirm against a capture.
- **Q7. Does the V.90 8.6 intro carry over?**
  - V.92 8.8 has no intro. Does V.90's rule still apply, and to SUVd and CPd as well as TRN2d and Ed?
    - Training: CPt shaping.
    - RR: data-mode shaping with the CPt K.
  - Assume yes, because TRN2d is imported unchanged and "corresponding TRN2d modulation" points at it.
  - This decides D in RR, so confirm it with a capture of an RR.
- **Q8. Which codewords Rd/Rf use when CPu bit 128 is set.** If the transmit constellations differ from the
  codec-output constellations, assume the **transmit** set, because Rd/Rf are defined as PCM codewords the
  digital modem sends.
- **Q9. Rf repetition typo.** "transmitted by 0 repeating" should read "by repeating". The count comes from
  clause 9: 384T = 32 repetitions.
- **Q10. Scope of SUVd bit 33.**
  - Table 31 ties bit 33 to receiving CPu, but 9.8.1.1.3 sets it in the silence handshake before any CPu.
  - Implement bit 33 as "I have received your SUV/CP in the current exchange". Clear it at the start of each
    new exchange: Phase 4, after R̄d, after R̄t, after R̄f.
- **Q11. SUVd reserved-bit wording.** Table 31 bits 19:31 say "set to 0 by the analogue modem", a copy of
  Table 27. The digital modem sends 0s.
- **Q12. Sign of the silence codewords.**
  - 9.8.1.1.3 fixes only the magnitude (Ucode 0).
  - In A-law, Ucode 0 is ±8 linear, so the sign is not entirely irrelevant.
  - Proposed: a constant positive sign (µ-law `FF`, A-law `D5`), or keep the encoder's sign pattern. Check a
    capture.
- **Q13. LP1 = 0 or LZ2 = 0.**
  - LZ2 = 0 makes v(n) ≡ 0, and the Table 30 rows for p1 and z2 have no "(if > 0)" qualifier.
  - Treat LP1 ≥ 1 and LZ2 ≥ 1 as required in practice. LP1 = 0 may be harmless, but avoid it.
- **Q14. Analogue TRN2u after a silence request.** The text says "up to 8004T" (9.8.2.1.5/6). Figures 16, 17
  and 18 print ≥4008T, ≥2400T and ≤8004T. Follow the text; the digital modem's wait timers depend on it.
- **Q15. K per upstream rate.** K = 2·(drn + 17) is derived from 6.1, 6.2 and Figure 1, not stated in 8.8.
  The digital modem must choose Mi so that ∏Mi ≥ 2^K (6.4.1), and satisfy DC-2.
- **Q16. Scope of 8.8.** 8.8 presumably applies only when INFO1a selects PCM upstream (Table 18). When short
  Phase 2 selects V.34 upstream (Table 19), the digital modem presumably uses the V.90 Phase 4 signals (MP
  and so on). Confirm with the Phase 2/3 digests.
- **Q17. Cleardown field.**
  - 9.11 says drn = 0 is signalled "in SUVu … or SUVd", but SUV sequences have no drn. The field is CPd
    bits 22:26.
  - The digital modem must be able to send a CPd with drn = 0, and must recognise drn = 0 in CPu or CPus
    (bits 21:25).
- **Q18. How long RR TRN2d may be.** "Up to 16008T" has no lower bound. Very short TRN2d gives the analogue
  equaliser little to train on. Choose a practical minimum (for example ≥ 2040T, as in Phase 4) and tune it
  on the live rig.
- **Q19. Rd/Rf codewords when the constellation of an interval changes in the RR/FPE being started.** Rf/Rd
  go out before the new CPu, so use the constellations in force. Section 6 states this; it is listed here
  because it is easy to get wrong.

### 13.4 Suggested module shape (non-binding)

- `v92::sequences::Cpd` with the fields:
  - `drn: u8`, `trellis: Trellis`, `extend_e2u: bool`, `ack: bool`, `gain4_q16: u16`;
  - `moduli: Option<[u8; 12]>`;
  - `filters: Option<Filters { z1: Vec<i16>, p1: Vec<i16>, z2: Vec<i16>, p2: Vec<i16> }>`;
  - `constellations: Option<Constellations { index: [u8; 6], sets: Vec<Vec<u16>> }>`.

  Give it `to_bits(frame_bits)` and `from_bits()`, built on `v90::sequences::{frame, put, get}` and
  `v34::info::crc`. Generalise `unframe` to a word count that is only known after the header words.
- `v92::sequences::Suvd { silence: bool, ack: bool }`: 52 bits before padding. The 2.4 vectors are its unit
  tests.
- An Rf source in the digital pump next to the existing `Out::Rd`/`Out::RdBar`:
  - the same per-interval `r_codes`;
  - sign `((n % 4) < 2) ^ bar`, with n counted from the start of the signal;
  - 32 × 12 symbols of Rf, then 2 × 12 of R̄f.
- A `Mapping` selector implementing 2.6: `for_training(cpt)`, `for_renegotiation(cpt, data)` (it exists in
  V.90), and `data_mode(cpu)` for FPE.
- A silence source: Ucode 0 on the frame clock.
- Test vectors to add once a V.92 server capture exists:
  - a decoded CPd, to settle Q1-Q4 and Q13;
  - an SUVd/SUVd' pair, to confirm the CRC over bits 18:33 and the vectors above;
  - an FPE, to confirm the Rf pattern, the reset to zero before SUVd, and the data-mode D;
  - an RR, to confirm D_rr (Q7) and the TRN2d length.
