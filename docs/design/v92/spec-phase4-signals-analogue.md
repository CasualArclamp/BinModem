# V.92 clause 8.7: phase 4, rate renegotiation and fast parameter exchange signals sent by the analogue modem

Implementation digest of ITU-T V.92 (11/2000), clause 8.7 (printed pages 26-32, PDF pages 33-39),
for BinModem's analogue (client) modem with PCM upstream.

Every bit layout, value and duration below was read from the rendered PDF pages, not from
`docs/specs/text/T-REC-V.92-200011-I.txt`. That text file scrambles Tables 23-27; for example it
shows "21:25 Type" and "69:76 The RMS value of TRN1d", and both are wrong. The Recommendation is
paraphrased here, with clause numbers. Anything not stated in the Recommendation is marked
**[interpretation]**.

Tags used for requirements:

- **[SHALL]**: mandatory ("shall").
- **[SHOULD]**: recommended.
- **[MAY]**: optional, or permitted.
- **[DEF]**: a definition or description that has no "shall" but that the implementation must still follow exactly.

---

## 0. Scope, pages read, conventions

### 0.1 Pages read (all rendered at 170 dpi and viewed)

| Source | PDF pages | What was taken from them |
|---|---|---|
| V.92 8.7.1-8.7.7 | 33-39 | the whole of this digest's core (Tables 23-29) |
| V.92 1, 3, 6.1-6.4, 7, 8 (intro) | 8-13 | scope, Qa.b format, LU, rates, scrambler, framing, modulus encoder, precoder/prefilter, inverse map, trellis, the bit-order convention |
| V.92 8.4 (INFO1a Tables 18/19), 8.5, 8.6 | 27-32 | INFO1a filter-size fields, CPt/E1u/Ru/TRN1u, Jd/Jp (Jp bits 48/49 select the TRN2u constellation) |
| V.92 8.8.1-8.8.6 | 39-44 | CPd (Table 30), which the analogue modem consumes; SUVd (Table 31) |
| V.92 9.5.2.1.11, 9.6, 9.7, 9.8, 9.9, 9.11 | 56-65, 69 | the procedures that use these signals, Figures 12-19, the timers |
| V.90 5.4.5.6, 8.5, Tables 14/15 | 18, 30-33 | the CP layout that V.92 CPu is derived from, the spectral shape filter, the constellation power rule |
| V.34 clause 7, 10.1.2.3.2, 10.1.3.1-10.1.3.9 | 16, 33, 38-39 | the scrambler polynomials, the CRC generator (Figure 14), V.34 B1/TRN/MP conventions |

### 0.2 Symbols and units

- **T** is one upstream symbol interval: 1/8000 s, or 125 µs (6.2). "384T" therefore means 384 symbols (48 ms).
- **Data frame (upstream)**: 12 symbols, with data frame intervals i = 0..11 (6.4, Figure 1). Two other structures sit on the same 12 symbols (Figure 1/V.92):
  - A **constellation frame** is 6 symbols long. Its index is j = i mod 6, so j runs 0..5, 0..5.
  - A **trellis frame** is 4 symbols long. Its index is k = i mod 4, so k runs 0..3, 0..3, 0..3.
- **Downstream data frame** (V.90): 6 symbols, with intervals 0..5. The constellation indices carried in CPu refer to these.
- **LU** (3.8) is the level chosen so that TRN1u goes out at the desired data-mode transmit power. TRN1u is ±LU.
- **G** is the gain applied to the prefilter output v(n) (6.4.2). CPd carries 4×G (Table 30, bits 35:50).
- **Qa.b** (3.5):
  - Signed Qa.b is two's complement in a+b+1 bits, with b bits after the binary point, covering [−2^a, 2^a).
  - Unsigned Qa.b is a+b bits with b fraction bits. The Recommendation gives its range as [0, 2^(a+1)).
  - That printed range conflicts with the bit count. See ambiguity A1. Follow the explicit digit patterns in the tables, such as "xxx.xxxxxxxxxxxxx".
- **Ucode / Uchord** (V.92 3.6, which points to V.90 3):
  - A Ucode is a universal PCM code, 0..127, that describes a µ-law or A-law magnitude.
  - Uchord c (c = 1..8) holds Ucodes 16(c−1) .. 16c−1.
  - Linear values are in Table 1/V.90. `crates/datapump/src/v90/ucode.rs` already implements them.

### 0.3 Bit order (clause 8, introductory paragraph)

- **[SHALL]** Tables 2-5, 11-24, 27 and 30-33 follow two rules unless a table says otherwise:
  - A value written as a **bit pattern** goes out leftmost bit first.
  - A value written as an **integer** goes out **least significant bit first**.
  - That covers CPu/CPt (Table 23), CPus (Table 24), SUVu (Table 27) and CPd (Table 30). Every multi-bit numeric field in them, including Q-format numbers and the CRC, goes out LSB first, starting at the lower bit number.
  - The frame sync is all ones, so its order does not matter.
- Tables 25/26 (RM patterns) and 28/29 (TRN2u mappings) are **not** on that list. See ambiguity A2 for Tables 28/29.
- Each table also says "Bit 0 is transmitted first".
- All PCM codewords in training sequences are described with the universal codes of Table 1/V.90 (clause 8).

### 0.4 Framed-sequence word structure (common to CPu, CPus, SUVu, CPd, SUVd)

[DEF, derived from the tables] Every framed sequence has the same shape:

1. **Frame sync**: bits 0:16, which are 17 ones.
2. **Words**: a run of 17-bit words. Each word is a start bit (0) followed by 16 information bits. The start bits fall at bit positions 17, 34, 51, 68, ... (17·w, for w = 1, 2, ...).
3. **CRC word**: one more word whose 16 information bits are the CRC.
4. **Fill**: at least one 0 fill bit after the CRC, then more 0 fill bits up to the length boundary the sequence requires.

**CRC (V.34 10.1.2.3.2, Figure 14; required by V.92 8.7.3 and 8.7.5)** [SHALL]

- **Polynomial**: x^16 + x^12 + x^5 + 1.
- **Coverage**: every information bit of the sequence. The frame sync bits, the start bits and the fill bits are **excluded**.
- **Procedure**:
  - Load the 16-bit shift register with all ones.
  - Shift the covered bits in, in transmission order.
  - Transmit the register contents starting with register bit 0. Bit 0 is the CRC's LSB.
  - No final inversion is applied.
- **Register structure** (Figure 14): the cells are numbered 15 (left) down to 0 (right) and shift right. The incoming bit is XORed with cell 0's output. That sum feeds cell 15 and the adders in front of cells 10 and 3.
- **Existing code**: `crates/datapump/src/v34/info.rs::crc()` implements exactly this. The repo's tests prove it matches V.34/V.90 sequences from real equipment. **Reuse it.**
- **Covered range for CPu**: bits 18 .. 271+δ minus the start bits, which is 240 + 16·δ/17 bits.
- **Covered range for CPus and SUVu**: bits 18..33, which is 16 bits.

---

## 1. Signal inventory (clause 8.7)

| Signal | Clause | Content | Modulation in training (phase 4) and rate renegotiation | Modulation in fast parameter exchange | Length |
|---|---|---|---|---|---|
| **B1u** | 8.7.1 | scrambled binary ones | full data mode using the parameters in the **preceding CPd** | same (the new parameters) | 48 data frames = 576T = 72 ms |
| **E2u** | 8.7.2 | scrambled, differentially encoded zeros | TRN2u modulation (4 or 8 point) | preceding (old) data-mode modulation | 1 data frame = 12T; 13T in training if CPd bit 29 = 1 |
| **CPu** (long), **CPu'** | 8.7.3, Table 23 | data-mode parameters for the **digital** modem (downstream rate, spectral shaping, downstream constellations) | TRN2u modulation | same modulation as data mode | variable, padded to a multiple of 12 symbols |
| **CPus** (short CPu) | 8.7.3, Table 24 | rate and acknowledge only ("digital modem's modulation parameters not changed") | TRN2u modulation (rate renegotiation only) | data-mode modulation | 52 bits, padded to a multiple of 12 symbols |
| **RM**, **RM'** | 8.7.4, Tables 25/26 | fixed 12-symbol patterns of modulus-encoder outputs | not used in training or rate renegotiation | data-mode constellation parameters, precoder/prefilter, trellis coded | RM 384T, RM' 24T (from 9.9.2) |
| **SUVu**, **SUVu'** | 8.7.5, Table 27 | short information sequence: wait-for-CPu request, measured level, silence request, acknowledge | TRN2u modulation, scrambled | preceding data-mode modulation | 52 bits, padded to a multiple of 12 symbols |
| **TRN2u** | 8.7.6, Tables 28/29 | scrambled ones on a 4- or 8-level constellation | itself | not used | variable, a multiple of 12 symbols |
| **FB1u** | 8.7.7 | scrambled, differentially encoded ones | not used | preceding (old) data-mode modulation | 48 data frames = 576T = 72 ms |

A prime (') means the acknowledge bit (bit 33) is set.

The subscript M in RM distinguishes it from Ru (8.5.5), which is the analogue modem's rate renegotiation and phase 3 signal and **bypasses** the precoder.

---

## 2. Shared transmitter machinery

### 2.1 Scrambler (6.3, V.34 clause 7)

- **[SHALL]** The analogue modem uses the self-synchronising scrambler of V.34 clause 7 with **GPA = 1 + x^−5 + x^−23** (equation 7-2/V.34). This applies whichever end placed the call.
  - The digital modem uses GPC = 1 + x^−18 + x^−23 (V.90 5.3 and V.92 8.6).
  - This is a fixed role assignment, **not** V.34's call/answer rule. `crates/datapump/src/v90/analogue.rs` already scrambles with GPA.
- The scrambler "divides" the data by the polynomial. In other words, out = in XOR out(−5) XOR out(−23) (V.34 clause 7).
- **Reset points** within these signals:
  - **[SHALL]** Reset (to zero) at the beginning of **every** TRN2u (8.7.6). **[interpretation]** "Reset" is read as the all-zero state, as for TRN1u (8.5.7) and V.34 TRN (10.1.3.8).
  - **[SHALL]** Initialise to zero before B1u (8.7.1).
  - **[SHALL]** In fast parameter exchange, initialise to zero after RM' and before the first SUVu (9.9.2.1.2, 9.9.2.2.3).
  - Everywhere else the scrambler **runs on continuously**, from TRN2u through SUVu, CPu, SUVu' and E2u, and from B1u into data. **[interpretation]** No other reset is stated.

### 2.2 "TRN2u modulation" (8.7.6 with Jp bits 48/49)

This is the modulation CPu, CPus, SUVu and E2u use in training and rate renegotiation.

- **Constellation size** comes from the digital modem's Jp (Table 22/V.92):
  - Jp bit 48 sets the size **during training**: 0 = 4-point, 1 = 8-point.
  - Jp bit 49 sets the size **during rate renegotiation**: 0 = 4-point, 1 = 8-point.
  - **[SHALL]** Store both bits from phase 3. Bit 49 is still needed long after training.
- **Bits per symbol**: b = 2 for 4-point and b = 3 for 8-point. Each symbol takes b scrambled bits.
- **Mapping** (Tables 28 and 29; bit groups are written MSB:LSB) **[SHALL]**:

Table 28 (4-point):

| MSB:LSB | value |
|---|---|
| 00 | +(1/√5)·LU |
| 01 | +(3/√5)·LU |
| 10 | −(1/√5)·LU |
| 11 | −(3/√5)·LU |

Table 29 (8-point):

| MSB:LSB | value |
|---|---|
| 000 | +(1/√21)·LU |
| 001 | +(3/√21)·LU |
| 010 | +(5/√21)·LU |
| 011 | +(7/√21)·LU |
| 100 | −(1/√21)·LU |
| 101 | −(3/√21)·LU |
| 110 | −(5/√21)·LU |
| 111 | −(7/√21)·LU |

  - The MSB is the **sign bit**: 0 is positive and 1 is negative, the same polarity as TRN1u (8.5.7).
  - The remaining bits, read as an unsigned integer m, select the magnitude (2m+1)/√5 or (2m+1)/√21.
  - Both constellations have a mean square of exactly LU². 4-point: (1+9)/2/5 = 1. 8-point: (1+9+25+49)/4/21 = 1. TRN2u therefore has the same power as TRN1u, which is the desired data-mode power.
- **Differential encoding of the sign bit** [SHALL]:
  - The transmitted sign is the present (scrambled) sign bit XOR the previously **transmitted** sign bit.
  - The magnitude bits are **not** differentially encoded **[interpretation]**. Only "the sign bit" is named.
  - Initialisation, as each clause states it:
    - TRN2u (8.7.6): from the last transmitted sign bit of the **preceding E1u** (phase 3's last sequence, which uses TRN1u modulation). A symbol of +LU has sign bit 0.
    - CPu, CPus and SUVu in training and rate renegotiation (8.7.3, 8.7.5): from the last transmitted sign bit of the preceding sequence. In practice the differential encoder runs on continuously from TRN2u.
    - In rate renegotiation TRN2u follows R̄u, not E1u. See ambiguity A4.
- **Time order of the b bits within a symbol**: **not stated**. See ambiguity A2, which is the most important open item for interoperability.
- **Length** [SHALL]: TRN2u is an integer multiple of 12 symbols (8.7.6). Every other sequence in this group is too, so upstream data-frame alignment is kept throughout.
- **[MAY]** The digital modem may use TRN2u to estimate the upstream analogue channel (8.7.6), for example to design the precoder and prefilter it returns in CPd.

### 2.3 "Data mode modulation" (6.4)

FPE sends E2u, CPu, CPus, SUVu and FB1u with the old data-mode parameters. B1u and RM use the data-mode parameters too. The data-mode chain is summarised below; a separate digest covers clause 6.

1. **Modulus encoder (6.4.1)**
   - Each data frame takes K scrambled bits b0..bK−1, with b0 first in time.
   - R = Σ b_i·2^i.
   - The sign is s = 0 if R ≤ (M−1)/2 and 1 otherwise, where M = ∏ M_i over i = 0..11.
   - The sign is differentially encoded: d(f) = s(f) XOR d(f−1).
   - R0 = R if d(f−1) = 0, and R0 = M−1−R if d(f−1) = 1. The printed text uses **d(f−1)** here, not d(f).
   - For i = 0..11: K_i = R_i mod M_i and R_{i+1} = (R_i − K_i)/M_i.
   - **[SHALL]** 2^K ≤ M.
   - **Bits per frame**: the upstream data signalling rate is (drn+17)·8000/6 (CPd bits 22:26), so K = 2·(drn+17). That is 36 bits at 24 kbit/s and 72 bits at 48 kbit/s. **[interpretation]** This follows from 12 symbols = 1.5 ms.
2. **Precoder (6.4.2)**
   - K_i maps to the equivalence class E(K_i) of points a(η) of the N = 2·LC point constellation (CPd). η runs −N/2 .. N/2−1 in level order.
   - For k = 0, 1, 2: η = K_i + z·M_i.
   - For k = 3: η = 2K_i + 2z·M_i + ((η0+η1+η2+Y0) mod 2).
   - The precoder picks u(n) from the class and writes its index to y(n). NOTE in 8.8.3: the digital modem assumes the analogue modem picks the point that minimises the precoder output power, symbol by symbol.
   - Precoder filter: x(n) = u(n) + Σ_{κ=1..LZ1} u(n−κ)·z1(κ) + Σ_{κ=1..LP1} x(n−κ)·p1(κ).
3. **Prefilter (6.4.2)**
   - v(n) = Σ_{κ=0..LZ2−1} x(n−κ)·z2(κ) + Σ_{κ=1..LP2} v(n−κ)·p2(κ).
   - Output = G·v(n).
4. **Inverse map (6.4.3)**: per trellis frame, (y(0),y(1)) and (y(2),y(3)) produce Y1..Y4, as in V.34 9.6.3.1 with odd coordinates 2·y(k)+1.
5. **Convolutional encoder (6.4.4)**: the V.34 9.6.3.2 encoders (16/32/64 state, chosen by CPd bits 27:28), with every 2T delay replaced by a 4T delay. It produces Y0. `crates/datapump/src/v34/trellis.rs` is the starting point.

**[interpretation]** When an information sequence (CPu, CPus, SUVu, E2u, FB1u) is sent "using data mode modulation", its bits feed the scrambler and then the modulus encoder at K bits per data frame, exactly as user data does. "Differentially encoded" then refers to the modulus encoder's d(f), and "a multiple of 12 symbols" means a whole number of data frames, which is a multiple of K bits. This matches how V.90 sends MP downstream in data frames (`v90/sequences.rs::mp_bits`).

### 2.4 Padding to 12 symbols (worked lengths)

**[SHALL]** CPu, CPus and SUVu are extended with 0 fill bits to the next multiple of 12 symbols (Tables 23, 24, 27). The table gives the resulting sizes.

| Sequence | Minimum bits L | 4-point TRN2u (24 bits per 12 symbols) | 8-point TRN2u (36 bits per 12 symbols) | FPE (K bits per 12 symbols) |
|---|---|---|---|---|
| CPus, SUVu | 52 | 72 bits = 36 symbols | 72 bits = 24 symbols | ⌈52/K⌉·K |
| CPu, max index 0, no codec masks (δ = 0) | 290 | 312 bits = 156 symbols | 324 bits = 108 symbols | ⌈290/K⌉·K |
| CPu, max index 0, codec masks (δ = 136) | 426 | 432 = 216 symbols | 432 = 144 symbols | ⌈426/K⌉·K |
| CPu, max index 5, no codec masks (δ = 680) | 970 | 984 = 492 symbols | 972 = 324 symbols | ⌈970/K⌉·K |
| CPu, max index 5, codec masks (δ = 1496) | 1786 | 1800 = 900 symbols | 1800 = 600 symbols | ⌈1786/K⌉·K |

For comparison, CPd and SUVd are padded to a multiple of **6** symbols (Tables 30, 31), because the downstream frame is 6 symbols long.

---

## 3. The signals

### 3.1 B1u (8.7.1)

- **[DEF]** B1u is 48 upstream data frames (576 symbols, 72 ms) of scrambled binary ones.
- **Parameters**: B1u uses the data-mode constellation parameters from the **preceding CPd**: rate, trellis choice, modulus parameters M0..M11, precoder/prefilter coefficients, G, and the constellation sets. It is the first signal to use them.
- **[SHALL]** The first symbol of B1u begins **data frame interval 0**.
- **[DEF]** The first transmitter output symbol of B1u's first data frame is time **n = 0** in the precoder and prefilter equations of 6.4.2.
- **[DEF/SHALL]** Before B1u, these memories are all initialised to **zero**:
  - the scrambler;
  - the modulus encoder (its differential memory d(f−1));
  - the convolutional encoder;
  - the precoder;
  - the prefilter.
  - Consequence: every x(n−κ), u(n−κ) and v(n−κ) with n−κ < 0 is 0, and B1u is a deterministic, known sequence.
- **Where it is used**:
  - In training and rate renegotiation, B1u follows E2u (9.6.2.1.5).
  - In FPE, B1u follows FB1u (9.6.2.1.5, Figure 19).
- **After B1u** [SHALL]: enable circuit 106 to follow circuit 105, and transmit data using 6.4 (9.6.2.1.5). The scrambler and every other memory run on from B1u into data **[interpretation]**.
- **Difference from V.90/V.34**: V.90's analogue B1 was V.34's B1 (10.1.3.1/V.34), which is **one** V.34 data frame with superframe bit inversions. V.92 B1u is 48 PCM-upstream data frames and has no superframe inversions.

### 3.2 E2u (8.7.2)

- **[DEF]** E2u is **one data frame** of scrambled, differentially encoded **zeros**.
- **Modulation**:
  - In training and rate renegotiation: TRN2u modulation, sized by Jp bit 48 (training) or bit 49 (rate renegotiation). That makes 12 symbols carrying 24 or 36 zero bits.
  - In FPE: the preceding data-mode modulation, which is K zero bits in one data frame.
- **[SHALL]** Extend E2u by **one symbol** (13 symbols) if **CPd bit 29** is set.
  - **[SHALL, for the digital modem]** CPd bit 29 is 0 during rate renegotiation and FPE (Table 30). The extension therefore only happens in phase 4 training.
  - **[interpretation]** The extra symbol continues the same scrambled, differentially encoded zero stream for one more symbol, carrying b more bits.
  - **[interpretation]** The extension shifts the upstream data-frame grid by one symbol. B1u then starts "data frame interval 0" (8.7.1) on the grid the digital modem wants.
- **Role**: E2u marks the end of the CPu/SUVu exchange (9.6.2.1.4).
  - The digital modem may treat E2u as acknowledgement (9.6.1.1.4) and switches its receiver to B1u, or to FB1u then B1u (9.6.1.1.6).
  - In the rate renegotiation silence path, E2u is followed by more TRN2u (9.8.2.1.4).
- **Receiver view**: after descrambling, E2u is a 12-symbol run of zeros that follows a sequence's zero fill. The next framed sequence would have begun with 17 ones instead.

### 3.3 CPu, the long form (8.7.3, Table 23)

**Purpose (8.7.3)**: CPu carries the modulation parameters the **digital modem** will use in data mode: the downstream rate, spectral shaping and the downstream PCM constellations. A CPu with bit 33 set is **CPu'**.

**Modulation**:
- **[DEF]** In training and rate renegotiation, CPu uses TRN2u modulation (§2.2). The differential encoder carries on from the last transmitted sign bit of the preceding sequence.
- **[DEF]** In FPE, CPu uses the same modulation as data mode (§2.3).
- **[SHALL]** Bit 0 goes out first.

**Table 23** (shared by CPu and CPt; CPt is the phase-3 signal of 8.5.1):

| Bits (LSB:MSB) | Width | Field | Values / meaning |
|---|---|---|---|
| 0:16 | 17 | Frame sync | 1111 1111 1111 1111 1 (17 ones) |
| 17 | 1 | Start bit | 0 |
| 18 | 1 | "CP" | 0. Tells CP-family sequences (0) from SUV sequences (1). |
| 19:20 | 2 | Type | 0 = CPt, 1 = CPu. CPus uses 2 (Table 24). 3 is unassigned. LSB (bit 19) first: CPu is bit19 = 1, bit20 = 0. |
| 21:25 | 5 | drn | Selected **downstream** (digital to analogue) data signalling rate, an integer 0..22. **drn = 0 means cleardown.** Rate = (drn+20)·8000/6 in CPu, so drn 1..22 gives 28 000 .. 56 000 bit/s. Rate = (drn+8)·8000/6 in CPt. |
| 26:30 | 5 | Reserved (ITU) | [SHALL] The analogue modem sends 0. The digital modem ignores these bits. |
| 31:32 | 2 | Sr | Number of sign bits per downstream data frame used as spectral-shaping redundancy, 0..3. 0 disables shaping (V.90 5.4.5). |
| 33 | 1 | Acknowledge | 0 = CPd not yet received from the digital modem. 1 = CPd received. |
| 34 | 1 | Start bit | 0 |
| 35 | 1 | Codec type | 0 = µ-law, 1 = A-law |
| 36:48 | 13 | Reserved (ITU) | [SHALL] Send 0. The digital modem ignores these bits. (V.90 put the upstream rate mask here; V.92 moved it to Ja.) |
| 49:50 | 2 | ld | Look-ahead frames requested for spectral shaping, 0..3. [SHALL] It must be consistent with the digital modem's capability in **Jd bits 49:50** (maximum look-ahead 1..3). V.90 5.4.5.5 makes ld 0 and 1 mandatory in the digital modem and 2 and 3 optional. |
| 51 | 1 | Start bit | 0 |
| 52:67 | 16 | TRN1d ratio | RMS of TRN1d at the digital modem's transmitter output divided by RMS of TRN1d at the codec's D/A output. **Unsigned Q3.13** (xxx.xxxxxxxxxxxxx), so 1.0 = 0x2000. |
| 68 | 1 | Start bit | 0 |
| 69:76 | 8 | a1 | Spectral shaping filter parameter, **signed Q1.6** (sx.xxxxxx) |
| 77:84 | 8 | a2 | same format |
| 85 | 1 | Start bit | 0 |
| 86:93 | 8 | b1 | same format |
| 94:101 | 8 | b2 | same format |
| 102 | 1 | Start bit | 0 |
| 103:106 | 4 | Index for downstream interval 0 | integer 0..5 |
| 107:110 | 4 | Index for interval 1 | 0..5 |
| 111:114 | 4 | Index for interval 2 | 0..5 |
| 115:118 | 4 | Index for interval 3 | 0..5 |
| 119 | 1 | Start bit | 0 |
| 120:123 | 4 | Index for interval 4 | 0..5 |
| 124:127 | 4 | Index for interval 5 | 0..5 |
| 128 | 1 | Codec constellations flag | 1 if the constellations at the digital modem's transmitter differ from those at the codec's D/A output |
| 129:135 | 7 | Reserved (ITU) | [SHALL] Send 0. The digital modem ignores these bits. |
| 136 | 1 | Start bit | 0 |
| 137:152 | 16 | Mask, Uchord1, constellation 0 | bit 137 = Ucode 0 ... bit 152 = Ucode 15 |
| 153 | 1 | Start bit | 0 |
| 154:169 | 16 | Mask, Uchord2 | bit 154 = Ucode 16 ... |
| 170 | 1 | Start bit | 0 |
| 171:186 | 16 | Mask, Uchord3 | bit 171 = Ucode 32 |
| 187 | 1 | Start bit | 0 |
| 188:203 | 16 | Mask, Uchord4 | bit 188 = Ucode 48 |
| 204 | 1 | Start bit | 0 |
| 205:220 | 16 | Mask, Uchord5 | bit 205 = Ucode 64 |
| 221 | 1 | Start bit | 0 |
| 222:237 | 16 | Mask, Uchord6 | bit 222 = Ucode 80 |
| 238 | 1 | Start bit | 0 |
| 239:254 | 16 | Mask, Uchord7 | bit 239 = Ucode 96 |
| 255 | 1 | Start bit | 0 |
| 256:271 | 16 | Mask, Uchord8 | bit 256 = Ucode 112 ... bit 271 = Ucode 127 |
| 272 : 271+γ | γ | More constellations | constellations 1..max, each in the 136-bit format of bits 136:271 |
| 272+γ : 271+δ | δ−γ | Codec constellations | present only if bit 128 = 1: one per transmit constellation (0..max), same 136-bit format |
| 272+δ | 1 | Start bit | 0 |
| 273+δ : 288+δ | 16 | CRC | V.34 10.1.2.3.2, LSB first |
| 289+δ | 1 | Fill bit | 0 |
| 289+δ : ... | ≥0 | Fill bits | 0s up to the next multiple of 12 symbols. The table prints the start as 289+δ; it means 290+δ (erratum E3). |

**Derived parameters** (8.7.3; identical to 8.5.1 and V.90 8.5.2):
- Let **max** be the largest constellation index in bits 103:127, from 0 to 5.
- **γ = 136 · max**.
- **δ = 2γ + 136** if bit 128 = 1, and **δ = γ** if bit 128 = 0.
- Constellations 0..max go out in bits 136 .. 135+136·(max+1). Index 0 sits at 136:271 and index 5 at 816:951.
- If bit 128 = 1, max+1 codec constellations follow, one per transmit constellation, in the same order.
- Total length before padding: **L = 290 + δ** bits.

**Mask semantics** [DEF]:
- A constellation mask is 128 bits. A bit set to 1 means the constellation contains the PCM code with that Ucode.
- Mask bit u (0..127) sits at bit position 137 + 17·⌊u/16⌋ + (u mod 16), sent Ucode-ascending within each Uchord.
- **[MAY]** A constellation used in two or more intervals need only be sent once. Indices point into the sent list.
- **[SHALL]** If the transmit constellations differ from those seen at the codec's D/A (bit 128 = 1), the codec-side constellation matching each transmit constellation **shall** be sent.

**Group rule**: **[SHALL]** CPu and CPu' sequences sent as a group all carry identical information. **[interpretation]** The acknowledge bit and the CRC are the only permitted differences.

**Meaning of the shaping fields** (from V.90 5.4.5.6, which V.92 5. applies to the digital modem):
- The analogue modem chooses T(z) = (1 − a1·z^−1)(1 − a2·z^−1) / ((1 − b1·z^−1)(1 − b2·z^−1)), with |a1|, |a2|, |b1|, |b2| ≤ 1.
- The digital modem's shaper minimises a running metric w[n] computed through F(z) = 1/T(z):
  - y[n] = x[n] − b1·x[n−1] + a1·y[n−1]
  - v[n] = y[n] − b2·y[n−1] + a2·v[n−1]
  - w[n] = v[n]² + w[n−1]
  - Here x[n] is the linear value of the transmitted PCM code (Table 1/V.90).
- The Q1.6 format covers [−2, 2), but V.90 restricts these parameters to magnitude ≤ 1.

**Constraints tying the downstream fields together** (V.90 5.4.1, 5.4.2, Table 2; V.92 clause 5 says the digital modem's encoder is V.90's):
- A data-mode downstream frame carries D = drn + 20 bits.
- Of those, S = 6 − Sr are sign bits and K = D − S go to the modulus encoder.
- The constellations named in CPu must give ∏_{i=0..5} M_i ≥ 2^K, where M_i is the size of interval i's constellation.
- The valid (K, S) combinations are in Table 2/V.90. `v90/sequences.rs::data_bits` and `v90/modulus.rs::fits` already encode this.

**Power rule (V.90 8.5.2, Table 15/V.90)**: V.92 8.7.3 does **not** restate it (open question Q3). V.90 requires two things:
- **[SHALL in V.90]** The average power of the constellation set must not exceed the Table 15 limit for the digital modem's maximum transmit power (INFO0d). The average is Σ_i Σ_j p_ij·n_ij / (6·2^K).
- **[SHALL in V.90]** Data-mode constellations must be no more than 3 dB above the phase-4 (CPt) constellations.

**Differences from V.90 CP (Table 14/V.90)**. The existing `v90::sequences::Cp` **cannot be reused as-is**:

| Field | V.90 CP | V.92 CPu |
|---|---|---|
| bit 18 | reserved 0 | "CP" identifier, 0 (SUV sequences carry 1) |
| type | bit 19 only (0 = CPt, 1 = CP) | bits 19:20 (0 = CPt, 1 = CPu, 2 = CPus) |
| drn | bits 20:24 | bits **21:25** |
| reserved | 25:29 | 26:30 |
| silence request | bit 30 (CPs) | removed; now SUVu bit 32 |
| upstream rate mask | bits 36:48 | reserved 0; the mask moved to the Ja DIL descriptor (Table 20/V.92) |
| acknowledge meaning | "received MP" | "received CPd" |
| fill | exactly 3 zeros (289+δ:291+δ) | 1 zero, then zeros to a multiple of 12 symbols |
| modulation | V.34 4/16-point 2D (10.1.3.9/V.34), size from Jd bits 47/48 | PCM-upstream TRN2u 4/8-level, size from **Jp** bits 48/49; data-mode modulation in FPE |
| everything from bit 31 onward except 36:48 | identical positions and semantics | identical |

### 3.4 CPus, the short CPu (8.7.3, Table 24)

- **[DEF]** CPus is used in rate renegotiation and FPE **when the digital modem's modulation parameters are not changed**.
- **[interpretation]** It carries only drn and the acknowledge bit. It can therefore change the downstream rate, for example lower it or clear down with drn = 0, while keeping the constellations and shaping from the last long CPu. It is not used in initial training, where the digital modem has no previous parameters.
- **Modulation** [DEF]:
  - In rate renegotiation: the same parameters as TRN2u (Jp bit 49). The differential encoder carries on from the last transmitted sign bit of the preceding sequence.
  - In FPE: data-mode modulation.
- **[SHALL]** Bit 0 goes out first. The CRC is that of 10.1.2.3.2/V.34.

| Bits (LSB:MSB) | Width | Field | Values |
|---|---|---|---|
| 0:16 | 17 | Frame sync | 17 ones |
| 17 | 1 | Start bit | 0 |
| 18 | 1 | "CP" | 0 |
| 19:20 | 2 | Type | **2** (bit 19 = 0, bit 20 = 1) |
| 21:25 | 5 | drn | downstream rate 0..22, 0 = cleardown, rate = (drn+20)·8000/6 |
| 26:32 | 7 | Reserved (ITU) | [SHALL] send 0 |
| 33 | 1 | Acknowledge | 0 = CPd not received, 1 = CPd received |
| 34 | 1 | Start bit | 0 |
| 35:50 | 16 | CRC | over bits 18..33 |
| 51 | 1 | Fill bit | 0 |
| 52:... | ≥0 | Fill bits | 0s to the next multiple of 12 symbols |

- CPus has no Sr field (bits 31:32 are reserved here). **[interpretation]** The digital modem keeps its previous Sr.
- The group rule is only stated for "CPu and CPu'". **[interpretation]** Apply it to CPus too.

### 3.5 RM and RM' (8.7.4, Tables 25 and 26)

- **[DEF]** RM repeats a 12-symbol pattern of **modulus encoder outputs** K_i. RM' repeats the complementary pattern.

| Interval i | RM (Table 25) | RM' (Table 26) |
|---|---|---|
| 0 | K0 = M0 − 1 | K0 = 0 |
| 1 | K1 = M1 − 1 | K1 = 0 |
| 2 | K2 = 0 | K2 = M2 − 1 |
| 3 | K3 = 0 | K3 = M3 − 1 |
| 4 | K4 = M4 − 1 | K4 = 0 |
| 5 | K5 = M5 − 1 | K5 = 0 |
| 6 | K6 = 0 | K6 = M6 − 1 |
| 7 | K7 = 0 | K7 = M7 − 1 |
| 8 | K8 = M8 − 1 | K8 = 0 |
| 9 | K9 = M9 − 1 | K9 = 0 |
| 10 | K10 = 0 | K10 = M10 − 1 |
| 11 | "u11 = 0" (read as K11 = 0; erratum E1) | "k11 = M11 − 1" (read as K11 = M11 − 1) |

- The pattern alternates two intervals at M_i − 1 with two at 0, a 4-symbol period. RM' is RM shifted by two symbols, which amounts to a polarity reversal of the pattern.
- **Constellation parameters**: **[DEF]** RM and RM' use the constellation parameters of data mode: M_i, the constellation sets and G.
- **Precoder and prefilter**: **[SHALL]** Use the same precoder and prefilter structure as the latest data mode. The coefficients are **not** bypassed, unlike Ru (8.5.5).
- **Trellis**: **[SHALL]** RM and RM' are trellis encoded. The K_i above go straight into the precoder's equivalence-class selection (§2.3 step 2), and Y0 from the convolutional encoder drives the k = 3 class in intervals 3, 7 and 11. **[interpretation]** The bit-to-K_i steps of the modulus encoder (R, sign, d(f)) are skipped.
- **Memories**: **[interpretation]** The scrambler is irrelevant to RM, because no bits are involved. The precoder, prefilter and trellis memories carry on from data mode, because nothing says to reset them. After RM', the scrambler and differential encoder are reset (9.9.2.1.2 and 9.9.2.2.3); the precoder, prefilter and trellis memories are not mentioned.
- **Durations**, from the procedures (9.9.2.1.1, 9.9.2.2.2): **[SHALL]** RM for **384T** (32 patterns), then RM' for **24T** (2 patterns). **[SHALL]** RM starts on a data-frame boundary.
- **Use**: RM/RM' is the analogue modem's FPE initiation and response signal. It is the FPE counterpart of Ru/R̄u in rate renegotiation.

### 3.6 SUVu and SUVu' (8.7.5, Table 27)

- **[DEF]** SUVu is a short information sequence. It is **scrambled**.
- **Modulation** [DEF]:
  - In training and rate renegotiation: TRN2u modulation. The differential encoder carries on from the last transmitted sign bit of the preceding sequence.
  - In FPE: the preceding data-mode modulation.
- An SUVu with bit 33 set is **SUVu'**.
- **[SHALL]** Bit 0 goes out first. The CRC is V.34 10.1.2.3.2.
- **[SHALL]** SUVu and SUVu' sequences sent as a group all carry identical information.

| Bits (LSB:MSB) | Width | Field | Values / meaning |
|---|---|---|---|
| 0:16 | 17 | Frame sync | 17 ones |
| 17 | 1 | Start bit | 0 |
| 18 | 1 | "SUVu" identifier | **1**. CP-family sequences carry 0 here. |
| 19:25 | 7 | Reserved (ITU) | [SHALL] send 0 |
| 26 | 1 | Wait-for-CPu request | 1 = the analogue modem asks the digital modem to wait for a CPu before sending CPd. [MAY] The digital modem need not comply. |
| 27:31 | 5 | Measured level | 20·log10(L), where L is the measured RMS of the **prefilter output multiplied by G**, in **signed Q2.2** (sxx.xx), LSB (bit 27) first, bit 31 the sign. Range −4.00 .. +3.75 dB in 0.25 dB steps. **Value 16 (binary 10000, −4.00) means no measurement has been taken.** |
| 32 | 1 | Silence request | 1 = a silent period is requested. [MAY] Used during rate renegotiation (9.8.2.1). [SHALL] It is 0 in FPE (9.9.2.1.2, 9.9.2.2.3). |
| 33 | 1 | Acknowledge | 0 = CPd not received, 1 = CPd received. The rate-renegotiation silence path also uses it as a handshake; see A6. |
| 34 | 1 | Start bit | 0 |
| 35:50 | 16 | CRC | over bits 18..33 |
| 51 | 1 | Fill bit | 0 |
| 52:... | ≥0 | Fill bits | 0s to the next multiple of 12 symbols |

**Measured level (bits 27:31)**
- CPd states the scaling it refers to (8.8.3): the digital modem designs its parameters on the assumption that when G·v(n) has a mean square of 1, the analogue modem transmits at the desired power.
- 0 dB therefore means "exactly as designed", and a non-zero value reports how far off the real precoder/prefilter output power is.
- **[interpretation]** Send 16 in initial training, where there is no data-mode measurement yet.
- **[interpretation]** In rate renegotiation and FPE, report the level measured over the preceding data mode, clamped to −3.75 .. +3.75.

**Why SUVu matters**
- SUVu is the "I'm ready and listening" sequence of the phase-4 and rate-renegotiation handshake.
- In training, the digital modem sends a single CPd only after receiving an SUVu (9.6.1.1.2).
- In rate renegotiation, the silence handshake is done entirely with SUVu and SUVd.

### 3.7 TRN2u (8.7.6, Tables 28 and 29)

- **[DEF]** TRN2u is a 4- or 8-point signal, as the digital modem requested in Jp bits 48 (training) and 49 (rate renegotiation).
- **[DEF]** It carries scrambled binary ones.
- **[SHALL]** The mapping follows Tables 28/29 (§2.2).
- **[SHALL]** The scrambler is reset at the beginning of TRN2u.
- **[DEF/SHALL]** The sign bit is differentially encoded (modulo-2 sum with the previously transmitted sign). **[SHALL]** Its memory is initialised with the last transmitted sign bit of the preceding E1u.
- **[SHALL]** TRN2u is an integer multiple of 12 symbols.
- **[MAY]** The digital modem may use TRN2u to estimate the upstream analogue channel.
- **Level**: mean square LU², the same as TRN1u (derived in §2.2).
- **Precoder**: **[interpretation]** TRN2u is not precoded or prefiltered. It is a fresh signal for the far end's channel estimate, and the phase-3 Ru/TRN1u path already bypasses the precoder (8.5.5). V.92 does not say so explicitly (open question Q5).
- **Lengths** set by the procedures:

| Context | Clause | Length rule |
|---|---|---|
| Phase 4 training | 9.6.2.1.1 | Send TRN2u **until** the modem is ready for CPd **and** (TRN2u has lasted **≥ 12000T** (1.5 s) **or** an SUVd has been received). Then start SUVu. |
| Rate renegotiation, first TRN2u (initiator or responder) | 9.8.2.1.2, 9.8.2.2.3 | **[SHALL]** Up to **16008T** (2.001 s). **[MAY]** Stop after **2400T** (300 ms) or when SUVd is received. |
| Rate renegotiation, after E2u, analogue modem requested silence (SUVd bit 32 clear) | 9.8.2.1.5 | **[SHALL]** Up to **8004T** (1.0005 s), then SUVu with bit 32 clear. |
| Rate renegotiation, after E2u, digital modem requested silence (SUVd bit 32 set) | 9.8.2.1.6 | Until **Rt is received** or **8004T** has been sent, then SUVu with bit 32 clear. |
| Figures 16/17 (informative) | — | Figure 16 labels the post-E2u TRN2u "≥4008T" (silence held to the maximum); Figure 17 labels it "≥2400T" (silence ended early). See A7. |

All of these are multiples of 12: 12000 = 12·1000, 16008 = 12·1334, 2400 = 12·200, 8004 = 12·667, 4008 = 12·334.

### 3.8 FB1u (8.7.7)

- **[DEF]** FB1u is 48 data frames (576T, 72 ms) of scrambled, differentially encoded binary ones, sent with the **preceding (old) data-mode modulation**.
- **Use**: FPE only. After E2u the analogue modem sends FB1u and then B1u (9.6.2.1.5). The digital modem's receiver expects FB1u followed by B1u (9.6.1.1.6).
- **[interpretation]**
  - "Differentially encoded" is the modulus encoder's d(f).
  - The scrambler and memories carry on from the FPE sequences; the reset happens at B1u.
  - FB1u gives the digital modem 72 ms of known-structure signal in the old parameters before the switch to the new ones.

### 3.9 Telling the sequences apart at a receiver

For both directions, after descrambling:

| bit 18 | bits 19:20 | Sequence |
|---|---|---|
| 0 | 0 | CPt (phase 3) |
| 0 | 1 | CPu (long) |
| 0 | 2 | CPus |
| 0 | n/a | CPd. Its bits 19:21 are part-present flags, not a type; direction tells CPd from CPu. |
| 1 | — | SUVu / SUVd |

A sequence starts at the first 0 after a run of **at least** 17 ones. See pitfall P6.

---

## 4. Related signals the analogue modem needs, defined outside 8.7

### 4.1 Ru and R̄u (8.5.5), used for rate renegotiation

- Ru repeats {+LU, +LU, +LU, −LU, −LU, −LU}. R̄u repeats {−LU, −LU, −LU, +LU, +LU, +LU}.
- **[SHALL]** Both **bypass the precoder and prefilter** and use the same structure as the 2-point TRN1u.
- Durations in rate renegotiation: **[SHALL]** Ru for 384T, then R̄u for 24T, with **[SHALL]** Ru starting on a data-frame boundary (9.8.2.1.1, 9.8.2.2.2).

### 4.2 E1u (8.5.2) and CPt (8.5.1)

- E1u is one data frame of scrambled, differentially encoded zeros in TRN1u modulation. It ends the CPt train, and TRN2u's differential encoder is seeded from its last sign.
- CPt uses Table 23 with Type = 0 and rate = (drn+8)·8000/6.
- CPt is sent in TRN1u modulation: 1 bit per symbol, each bit differentially encoded. 24 differentially encoded ones go before the first CPt.

### 4.3 Jp bits 48/49 (Table 22, 8.6.3)

- Bit 48: the constellation for CPu, E2u, SUVu and TRN2u in **training**. 0 = 4-point, 1 = 8-point.
- Bit 49: the same choice for **rate renegotiation**.

### 4.4 INFO1a bits the analogue modem advertises (Table 18; they bound CPd)

- **Bits 12:13, filter sections supported**:
  - 0 = p1 and z2
  - 1 = z1, p1 and z2
  - 2 = p1, p2 and z2
  - 3 = z1, p1, p2 and z2
- **Bits 14:15, Ltot** = maximum of LZ1+LP1+LZ2+LP2: 0 = 192, 1 = 256, 2 = 320, 3 = 384.
- **Bits 16:17, Lmax** = maximum of any single one of LZ1, LP1, LZ2, LP2: 0 = 128, 1 = 192, 2 = 256, 3 = 320.

### 4.5 CPd (8.8.3, Table 30): the upstream parameters the analogue modem receives

This is a summary so that the precoder/prefilter and upstream constellation fields are in one place. 8.8.3 is the authoritative clause, and it probably has its own digest.

**General rules (8.8.3)**
- CPd has **four parts**:
  - Part 1 is bits 0..50 and is always present.
  - The other three are optional. Their presence is flagged by **bits 19, 20 and 21**: modulus encoder parameters, precoder/prefilter coefficients, and constellation sets.
- **[DEF]** An absent part's bits are removed completely. The bit positions in Table 30 assume all parts are present.
- Every CPd ends with a CRC and at least one fill bit, padded to a multiple of **6 symbols**.
- CPd is scrambled and uses TRN2d modulation in training and rate renegotiation, or the preceding data-mode modulation in FPE.
- Bit 0 goes out first. CPd' is CPd with the acknowledge bit set.
- **[SHALL]** CPd/CPd' sequences sent as a group carry identical information.
- **[SHALL]** Constellations contain no zero point. Non-empty constellation sets come first. A set has at most 128 points.
- **[SHALL]** LZ1+LP1+LZ2+LP2 ≤ Ltot (INFO1a bits 14:15). **[SHALL]** Each count is at most Lmax (INFO1a bits 16:17).
- **[SHALL]** The digital modem designs its parameters assuming G·v(n) with a mean square of 1 gives the desired transmit power.
- **[SHOULD]** (NOTE) The digital modem designs the precoder assuming the analogue modem minimises the precoder output power symbol by symbol.
- **Variable-length parameters**:
  - **α = 17·(LZ1+LP1+LZ2+LP2)**
  - **β = 17·(LC1+…+LC6)**
  - The modulus part is 6 words, the coefficient part is 4 + ΣL words, and the constellation part is 5 + ΣLC words. A word is 17 bits.

**Part 1 (always present)**

| Bits | Field |
|---|---|
| 0:16 | sync, 17 ones |
| 17 | start 0 |
| 18 | "CPd": 0 |
| 19 | 1 = modulus encoder parameters present |
| 20 | 1 = prefilter and precoder coefficients present |
| 21 | 1 = constellation sets present |
| 22:26 | **upstream** drn, 0..19. [SHALL] 0 = cleardown. Rate = (drn+17)·8000/6, so 24 000 .. 48 000. |
| 27:28 | upstream trellis encoder: 0 = 16-state, 1 = 32-state, 2 = 64-state, 3 = reserved. [SHALL] The analogue transmitter uses it. |
| 29 | extend E2u by 1 symbol (1) or not (0). [SHALL] 0 during rate renegotiation and FPE. |
| 30:32 | reserved (0) |
| 33 | acknowledge: CPu received |
| 34 | start 0 |
| 35:50 | **4·G** (> 0), unsigned Q0.16 (.xxxxxxxxxxxxxxxx) |

**Modulus encoder parameters** (if bit 19 = 1): start bits at 51, 68, 85, 102, 119, 136.

| Bits | Field |
|---|---|
| 52:59 | M0 (8 bits) |
| 60:67 | M1 |
| 69:76 | M2 |
| 77:84 | M3 |
| 86:93 | M4 |
| 94:101 | M5 |
| 103:110 | M6 |
| 111:118 | M7 |
| 120:127 | M8 |
| 128:135 | M9 |
| 137:144 | M10 |
| 145:152 | M11 |

**Precoder and prefilter coefficients** (if bit 20 = 1). The bit positions assume the modulus part is present.

| Bits | Field |
|---|---|
| 153 | start 0 |
| 154:162 | **LZ1** (9 bits): precoder feed-forward taps, up to Lmax |
| 163:169 | reserved 0 |
| 170 | start 0 |
| 171:179 | **LP1** (9 bits): precoder feedback taps, up to Lmax |
| 180:186 | reserved 0 |
| 187 | start 0 |
| 188:196 | **LZ2** (9 bits): prefilter feed-forward taps, up to Lmax |
| 197:203 | reserved 0 |
| 204 | start 0 |
| 205:213 | **LP2** (9 bits): prefilter feedback taps, up to Lmax |
| 214:220 | reserved 0 |
| 221 | start 0 |
| 222:237, then one word per coefficient | **z1(1) .. z1(LZ1)**, signed **Q0.15** (s.xxxxxxxxxxxxxxx), present if LZ1 > 0 |
| start bit at 221+17·LZ1, then 222+17·LZ1 : 237+17·LZ1 ... | **p1(1) .. p1(LP1)**, signed **Q1.14** (sx.xxxxxxxxxxxxxx) |
| start bit at 221+17·(LZ1+LP1), then ... | **z2(0) .. z2(LZ2−1)**, signed **Q0.15**. The index starts at **0**. |
| start bit at 221+17·(LZ1+LP1+LZ2), then ... | **p2(1) .. p2(LP2)**, signed **Q1.14**, present if LP2 > 0 |

Each coefficient word is a start bit followed by 16 bits. The last coefficient ends at bit 220+α.

**Constellation sets** (if bit 21 = 1)

| Bits | Field |
|---|---|
| 221+α | start 0 |
| 222+α : 225+α | constellation index (0..5) for upstream intervals **0 and 6** |
| 226+α : 229+α | intervals 1 and 7 |
| 230+α : 233+α | intervals 2 and 8 |
| 234+α : 237+α | intervals 3 and 9 |
| 238+α | start 0 |
| 239+α : 242+α | intervals 4 and 10 |
| 243+α : 246+α | intervals 5 and 11 |
| 247+α : 254+α | reserved 0 |
| 255+α | start 0 |
| 256+α : 263+α | **LC1**: number of positive points in set 1 |
| 264+α : 271+α | **LC2** (possibly 0) |
| 272+α | start 0 |
| 273+α : 280+α | LC3 |
| 281+α : 288+α | LC4 |
| 289+α | start 0 |
| 290+α : 297+α | LC5 |
| 298+α : 305+α | LC6 |
| 306+α | start 0 |
| 307+α : 322+α | linear value of the 1st (smallest-magnitude) point of set 1 |
| 323+α | start 0 |
| ... | one word per point, ascending, up to the largest point of set 1 |
| 306+α+17·LC1 | start 0 |
| ... | further non-empty sets in the same format |

**End of CPd**

| Bits | Field |
|---|---|
| 306+α+β | start 0 |
| 307+α+β : 322+α+β | CRC |
| 323+α+β | fill 0 |
| 324+α+β ... | 0s to a multiple of 6 symbols |

**Constellation geometry** (6.4.2): set n has N = 2·LCn points. The LCn sent magnitudes are the non-negative-index points a(0) .. a(LCn−1), and **[interpretation]** the negative points are their mirror images: a(−η−1) = −a(η). The format of the 16-bit linear value is not stated (open question Q1).

### 4.6 SUVd (Table 31), received by the analogue modem

| Bits | Field |
|---|---|
| 0:16 | sync |
| 17 | start |
| 18 | 1 |
| 19:31 | reserved |
| 32 | silence requested (see 9.8.1.1) |
| 33 | acknowledge: CPu received |
| 34 | start |
| 35:50 | CRC |
| 51 | fill 0 |
| then | fill to a multiple of **6** symbols |

- **[SHALL]** SUVd and SUVd' sequences sent as a group carry identical information.
- Table 31's reserved text says "set to 0 by the analogue modem", which is a copy error (E4).

### 4.7 Digital-modem signals whose detection drives these procedures

- **Ed** (8.8.2): 2 downstream data frames of scrambled zeros.
- **B1d**: see 8.6.1/V.90.
- **Rd/R̄d and Rt/R̄t**: see 8.6.4/V.90.
- **Rf/R̄f** (8.8.4): Rf repeats 12 PCM codewords with sign pattern ++−−++−−++−−. R̄f is 2 repetitions of −−++−−++−−++. The codewords are the highest-power code of each interval's data-mode constellation from CPu.

---

## 5. Procedures that use these signals (analogue modem side)

Numbered as in the Recommendation. "RTD" means round-trip delay.

### 5.1 Phase 4, final training (9.6.2; Figures 12-14)

**Error-free procedure**

1. **9.6.2.1.1 [SHALL]** Set the receiver to receive SUVd and transmit TRN2u, sized by Jp bit 48.
   - When the modem is ready to receive CPd **and** either TRN2u has lasted ≥ 12000T **or** an SUVd has arrived, transmit SUVu sequences.
2. **9.6.2.1.2 [SHALL]** On receiving an SUVd, transmit **one** CPu, then more SUVu.
   - On receiving a CPd, set the acknowledge bit in every subsequent CPu and SUVu (making them CPu' and SUVu').
3. **9.6.2.1.3 [SHALL]** Watch the acknowledge bit in the received sequences.
   - Look at every CPd and SUVd received up to and including the entire sequence that arrives **after 100 ms + RTD from the end of this modem's CPu**.
   - If none of them has the acknowledge bit set, send **repeated CPu** sequences.
   - Figure 14 shows three CPu' in a row in this case.
4. **9.6.2.1.4 [SHALL]** Finish and send E2u once both of these hold:
   - this modem has sent a CPu or SUVu with the acknowledge bit set, **and**
   - it has received a CPd or SUVd with the acknowledge bit set, **or** Ed.
   - Then finish the current CPu (the text says CPu even where the current sequence is an SUVu; see A5) and transmit **E2u**, extended by 1 symbol if CPd bit 29 = 1.
5. **9.6.2.1.5 [SHALL]** After E2u, send **B1u**. In FPE, send **FB1u then B1u** instead.
   - Then enable circuit 106 to follow circuit 105, and transmit data per 6.4.
6. **9.6.2.1.6 [SHALL]** On receiving Ed, set the receiver for B1d.
   - After B1d, unclamp circuit 104, turn circuit 109 on, and demodulate data.

**Recovery (9.6.2.2)**

- **[MAY]** Retrain at any time in phase 4 (9.7.2.1).
- **[SHALL]** If Tone B is detected, respond to the retrain (9.7.2.2).
- **9.6.2.2.1 [SHALL]** If B1d has not arrived within **20 s + 6·RTD** of the end of sending INFO1a, initiate a retrain (9.7.2.1).

The digital modem's mirror image is 9.6.1:
- TRN2d ≥ 2040T, then SUVd.
- One CPd on receiving SUVu.
- Acknowledgement after CPu.
- Repeated CPd if there is no acknowledgement within 100 ms + RTD.
- Ed, then B1d with the CPu parameters.
- On E2u, it expects B1u, or FB1u then B1u.
- 9.6.1.2.1: B1u must arrive within 20 s + 6·RTD of the end of INFO1a.

### 5.2 Rate renegotiation (9.8.2; Figures 15-18)

**General requirements (9.8)**
- **[MAY]** Either modem can start one at any time in data mode.
- **[MAY]** It can change the rate and other parameters, or retrain the analogue modem's echo canceller or its precoder/prefilter without a full retrain.
- **[SHALL]** Both modems keep data-frame synchronisation throughout.
- **[SHALL]** A rate renegotiation is initiated, and responded to, only on a data-frame boundary.

**Initiating (9.8.2.1)**

1. **9.8.2.1.1 [SHALL]** Turn circuit 106 OFF and transmit **Ru for 384T**, then **R̄u for 24T**. Ru starts on a data-frame boundary.
2. **9.8.2.1.2 [SHALL]** Set the receiver for SUVd and transmit TRN2u, sized by **Jp bit 49**, for up to 16008T. **[MAY]** Stop after 2400T or when SUVd arrives.
3. **9.8.2.1.3 [SHALL]** Transmit SUVu sequences.
   - **[MAY]** Set bit 32 to request silence (Table 27).
   - Once an SUVu has been sent **and** an SUVd received, continue with 9.6.2.1.2 (the CPu/CPd exchange, then E2u and B1u) **unless bit 32 is set in either the SUVu or the SUVd**.
4. **9.8.2.1.4 [SHALL]** (silence path) Transmit SUVu with **bit 33** set.
   - On receiving an SUVd with bit 33 set, or Ed, finish the current SUVu, then transmit **E2u followed by TRN2u**.
   - Meanwhile the digital modem sends Ed and then silence: Ucode-0 codewords, with frame alignment kept (9.8.1.1.3).
5. **9.8.2.1.5 [SHALL]** If SUVd bit 32 was **clear** (only the analogue modem wanted silence), send TRN2u for **up to 8004T**, then SUVu with bit 32 clear, then continue with 9.6.2.1.2.
   - The digital modem, having seen SUVu bit 32 clear, sends Rt (384T), R̄t (24T), then SUVd (9.8.1.1.4).
6. **9.8.2.1.6 [SHALL]** If SUVd bit 32 was **set** (the digital modem wanted silence), set the receiver for **Rt**.
   - When Rt arrives **or** 8004T of TRN2u have been sent, transmit SUVu with bit 32 clear and wait for an SUVd.
   - Then continue with 9.6.2.1.2.

**Responding (9.8.2.2)**

1. **9.8.2.2.1 [SHALL]** On receiving Rd, clamp circuit 104 to binary one and set the receiver to detect the Rd-to-R̄d transition.
2. **9.8.2.2.2 [SHALL]** After that transition, transmit Ru for 384T and R̄u for 24T. Ru starts on a data-frame boundary.
3. **9.8.2.2.3 [SHALL]** Set the receiver for SUVd and transmit TRN2u for up to 16008T. **[MAY]** Stop after 2400T or when SUVd arrives. Then continue per 9.8.2.1.3.

**Figure 15** (digital modem initiates, no silence), analogue side:

DATA, Ru (384T), R̄u (24T), TRN2u (≥2400T), SUVu, SUVu, CPu, SUVu, SUVu', SUVu', E2u, B1u, DATA

The analogue modem starts Ru after R̄d; the figure's 24T mark there is R̄d's length.

### 5.3 Fast parameter exchange (9.9.2; Figure 19)

**General requirements (9.9)**
- **[MAY]** Either modem can start one at any time in data mode.
- **[MAY]** The rate and other parameters can change.
- **[SHALL]** Both modems keep data-frame synchronisation.
- **[SHALL]** An FPE is initiated and responded to only on data-frame boundaries.
- There is no TRN2u in FPE. Every sequence uses the preceding data-mode modulation.

**Initiating (9.9.2.1)**

1. **9.9.2.1.1 [SHALL]** Turn circuit 106 OFF, set the receiver to detect Rf, R̄f and SUVd, and transmit **RM for 384T** followed by **RM' for 24T**. RM starts on a data-frame boundary.
2. **9.9.2.1.2 [SHALL]** Initialise the **scrambler and differential encoder to zero** and transmit SUVu with **bit 32 clear**.
   - After detecting Rf and R̄f, set the receiver for SUVd and continue with 9.6.2.1.2: CPu (long or short), SUVu', E2u, **FB1u**, B1u.
   - **[SHALL]** If **Rd** is detected instead (the far end started a rate renegotiation), follow 9.8.2.2.1. Rate renegotiation takes precedence.

**Responding (9.9.2.2)**

1. **9.9.2.2.1 [SHALL]** On detecting Rf, clamp circuit 104 to binary one and set the receiver to detect the Rf-to-R̄f transition.
2. **9.9.2.2.2 [SHALL]** After that transition, transmit RM for 384T, then RM' for 24T. RM starts on a data-frame boundary.
3. **9.9.2.2.3 [SHALL]** Initialise the scrambler and differential encoder to zero, transmit SUVu with bit 32 clear, and continue with 9.6.2.1.2.

**The digital modem's side (9.9.1)**
- It sends Rf (384T) and R̄f (24T).
- It then zeroes its scrambler, differential encoder and **spectral shaping filter memory**, and sends SUVd with bit 32 clear.
- It detects RM and RM'.

**Figure 19** (analogue modem initiates), analogue side:

DATA, RM (384T), RM' (24T), SUVu, SUVu, SUVu, CPu, SUVu, SUVu', SUVu', E2u, FB1u, B1u, DATA

### 5.4 Cleardown (9.11)

- **[SHALL]** The cleardown procedure is used to end a connection. It is signalled by **drn = 0** in a rate sequence.
- The text says "in SUVu", but SUVu has **no drn field** (erratum E2). **[interpretation]** Use CPu or CPus bits 21:25 = 0.
- **[MAY]** Cleardown can be signalled any time a rate sequence is sent.
- **[SHALL]** To clear down from data mode, initiate a rate renegotiation **or** an FPE to get a chance to send drn = 0.
- The digital modem signals cleardown with CPd bits 22:26 = 0.

---

## 6. Timers, durations and tolerances (all from V.92 unless noted)

| Item | Value | Clause | Kind |
|---|---|---|---|
| Upstream symbol | T = 125 µs (8000 symbol/s) | 6.2 | SHALL |
| Upstream data frame | 12T = 1.5 ms | 8.7.1, 6.4 | DEF |
| B1u | 48 frames = 576T = 72 ms | 8.7.1 | DEF |
| FB1u | 48 frames = 576T = 72 ms | 8.7.7 | DEF |
| E2u | 12T, or 13T if CPd bit 29 = 1 | 8.7.2 | SHALL |
| TRN2u, phase 4 | ≥ 12000T (1.5 s) before SUVu, unless SUVd has arrived | 9.6.2.1.1 | SHALL |
| TRN2u, first in rate renegotiation | ≤ 16008T; may stop at ≥ 2400T or on SUVd | 9.8.2.1.2, 9.8.2.2.3 | SHALL / MAY |
| TRN2u, after E2u (silence path) | ≤ 8004T, or until Rt | 9.8.2.1.5-6 | SHALL |
| Every sequence length | multiple of 12 symbols (E2u extension excepted) | 8.7.3, 8.7.5, 8.7.6 | SHALL |
| Ru / R̄u | 384T / 24T (48 ms / 3 ms) | 9.8.2.1.1, 9.8.2.2.2 | SHALL |
| RM / RM' | 384T / 24T | 9.9.2.1.1, 9.9.2.2.2 | SHALL |
| CPu acknowledgement window | 100 ms + RTD after the end of own CPu (includes the whole sequence straddling that time) | 9.6.2.1.3 | SHALL |
| B1d timeout (phase 4) | 20 s + 6·RTD from the end of sending INFO1a, then retrain | 9.6.2.2.1 | SHALL |
| Retrain silence | 70 ± 5 ms, then Tone A | 9.7.2.1 | SHALL |
| Tone B detection before responding | > 50 ms | 9.7.2.2 | SHALL |
| Digital modem TRN2d (for context) | ≥ 2040T in phase 4; ≤ 16008T in rate renegotiation | 9.6.1.1.1, 9.8.1.1.2 | SHALL |
| Rd/Rt/Rf / their complements (for context) | 384T / 24T | 9.8.1, 9.9.1 | SHALL |

---

## 7. What the referenced Recommendations require

| Reference in V.92 | Where | Requirement |
|---|---|---|
| V.34 clause 7, equation 7-2 | 6.3 | Self-synchronising scrambler. GPA = 1 + x^−5 + x^−23 (the answer-mode polynomial; the V.92 analogue modem always uses it). The transmitter divides the data by the polynomial; the quotient coefficients, in descending order, form the output. |
| V.34 10.1.2.3.2 | 8.7.3, 8.7.5 (and 8.5.1, 8.6.2, 8.8.3, 8.8.5) | CRC-16 with x^16+x^12+x^5+1, preset to all ones. Covers all information bits except sync, start and fill. Output from register bit 0 (the LSB) first, not inverted. Figure 14 fixes the register. |
| V.34 9.6.3.1, 9.6.3.2 | 6.4.3, 6.4.4 (used by B1u, RM, FB1u, FPE sequences) | The symbol-to-bit converter and the 16/32/64-state convolutional encoders. V.92 changes the coordinates to 2y(k)+1 and the 2T delays to 4T. The repo's `v34/trellis.rs` implements V.34's. |
| V.90 clause 3, Table 1 | 3.6, clause 8 | Ucode 0..127 and the µ-law/A-law linear values. Uchord c = Ucodes 16(c−1)..16c−1. Implemented in `v90/ucode.rs`. |
| V.90 5.4.1, 5.4.2, Table 2 | clause 5 (the digital modem's encoder is V.90's) | Downstream frame of 6 symbols carrying D = drn+20 bits, split into K modulus bits and S = 6−Sr sign bits. Valid (K, S) pairs are listed. CPu's drn, Sr and constellations must be consistent with them. |
| V.90 5.4.5.5, 5.4.5.6 | CPu fields ld, a1, a2, b1, b2 | ld is 0..3 (0 and 1 mandatory in the digital modem). T(z) and the w[n] recursion are in §3.3. The parameters have magnitude ≤ 1 and use 8-bit two's complement with 6 fraction bits. |
| V.90 8.5.2 and Table 15 | not re-cited by V.92 8.7.3 (Q3) | Constellation average-power ceiling as a function of the digital modem's maximum power. Data-mode constellations no more than 3 dB above the phase-4 ones. |
| V.90 8.6.1 (B1d), 8.6.4 (Rd, Rt), 8.6.5 (TRN2d), 8.2 (Tones A/B) | 8.8.1, 8.8.4, 8.8.6, 8.9.1 | Downstream signals the analogue modem must detect during these procedures. Covered by the V.90 digests and code. |

---

## 8. Implementation notes

### 8.1 What is new compared with V.90 (for our analogue modem)

1. **The whole upstream phase 4 is new.**
   - V.90's analogue modem ran V.34 phase 4 upstream: V.34 TRN, CP in V.34 MP modulation, V.34 E, and V.34 B1.
   - V.92 replaces all of that with PCM-upstream signals: 4- or 8-level TRN2u at 8000 baud, CPu/SUVu on that constellation, E2u, and B1u in the precoded, trellis-coded PCM-upstream data mode.
2. **SUV handshake.**
   - V.90 had no SUV sequences. V.92 wraps the CP exchange in SUVu/SUVd "ready" sequences.
   - The silence request moved from CP bit 30 into SUV bit 32.
   - SUVu adds the wait-for-CPu request (bit 26) and the transmit-level report (bits 27:31).
3. **CPu layout changed** (§3.3 comparison table).
   - The type field is now 2 bits, and drn has moved up by one bit.
   - The upstream rate mask is gone from CP.
   - The fill rule changed.
4. **CPus** (short CPu) is new. It is used when downstream modulation parameters are unchanged.
5. **Fast parameter exchange** is entirely new: RM/RM', FB1u, and every sequence sent in the old data-mode modulation with no TRN.
6. **E2u extension bit** (CPd bit 29) and **B1u's 48 frames** with a full memory reset are new.
7. **Constellation choice comes from Jp** (bits 48/49), not Jd (bits 47/48 in V.90).
8. **Downstream side unchanged**: CPu's downstream semantics (Sr, ld, the shaping filter, masks, the codec-constellation flag, the TRN1d ratio) are V.90's, so the existing V.90 constellation-design code applies directly.

### 8.2 Pitfalls

- **P1 Scrambler polynomial.**
  - Upstream is always GPA and downstream always GPC, whoever placed the call.
  - The V.34 code picks by call/answer role. Do not reuse its selector.
- **P2 Resets.**
  - TRN2u resets the scrambler. B1u resets the scrambler and all transmit memories. FPE resets the scrambler and the differential encoder after RM'.
  - **Nothing else resets.** In particular, CPu, SUVu and E2u in training carry on the TRN2u scrambler and differential encoder states. A per-sequence reset, as a naive "build frame, send frame" design would do, breaks the far end's descrambling.
  - Build a single continuous upstream bit pump for the whole phase-4 run.
- **P3 Two different "bits per symbol" regimes.**
  - With TRN2u modulation, b = 2 or 3, and padding is to 24 or 36 bits per 12 symbols.
  - With data-mode modulation (FPE), padding is to multiples of K.
  - Also, rate renegotiation uses **Jp bit 49**, not bit 48.
- **P4 E2u's odd length.**
  - With CPd bit 29 set, E2u is 13 symbols, so B1u starts one symbol "late" relative to the phase-3 frame grid.
  - The transmitter must redefine data-frame interval 0 at B1u's first symbol (8.7.1). The upstream framer must accept this shift rather than assert on a mid-frame boundary.
- **P5 RM is not Ru.**
  - Ru/R̄u (rate renegotiation) are raw ±LU symbols that **bypass** the precoder and prefilter.
  - RM/RM' (FPE) go **through** the precoder, prefilter and trellis with fixed K_i values.
  - Mixing these up makes the far end miss the FPE entirely.
- **P6 Frame sync after TRN.**
  - TRN2u and TRN2d are scrambled **ones**, so after descrambling the first SUV sequence follows an unbounded run of ones.
  - `v90/sequences.rs::Finder` only accepts a zero after exactly 17 ones ("not eighteen"). That rule would miss the first SUVd/CPd after TRN2d.
  - A V.92 finder must accept the first 0 after a run of ≥ 17 ones as a start, then check the CRC. This matters for our receiver of SUVd and CPd.
- **P7 Fill and E2u are both zeros.**
  - At the receiver, E2u (or Ed) can only be told from fill by counting past the known sequence end.
  - For the transmitter: never pad by more than the 12-symbol rule. Extra zeros could look like E2u.
- **P8 Acknowledge timing.**
  - The 100 ms + RTD window includes the whole sequence that is in flight at the deadline.
  - VoIP paths add about 1.5 s of round trip (see the project's memory notes on the VoIP line round trip and jitter slips). Size RTD from phase 2's measurement plus margin, or CPu repeats will start spuriously.
  - Repeats are harmless, though; the far end handles CPu' storms (Figure 14).
- **P9 Group identity.**
  - Every CPu/CPu' in one exchange must carry the same parameters. Only the acknowledge bit (and so the CRC) may differ.
  - Do not re-run constellation design between repeats.
- **P10 LSB-first Q formats.**
  - a1..b2, the TRN1d ratio, the SUVu level and the CRC all go out LSB first.
  - The SUVu level's sign bit is bit 31. "No measurement" is 16 decimal, which is bit 31 = 1 and bits 27..30 = 0.
- **P11 Frame alignment across rate renegotiation and FPE.**
  - Ru/RM and every following sequence must start on data-frame boundaries, and all are multiples of 12 symbols.
  - Keep one upstream symbol counter modulo 12 from data mode through the procedure. The only permitted shift is the E2u extension.
- **P12 FB1u and B1u parameter switch.**
  - E2u and FB1u use the **old** parameters. B1u uses the **new** ones, from the CPd just exchanged.
  - If CPd omitted a part (bits 19-21 = 0), keep the previous values of that part (Q2).
- **P13 Level report.**
  - L is measured after G, at the prefilter output. It is not the line level after the D/A.
  - Report 16 whenever there is no data-mode history.

### 8.3 Ambiguities and errata found in the rendered text

- **A1 Unsigned Qa.b range.**
  - Clause 3.5 prints the range as [0, 2^(a+1)) for an (a+b)-bit number. That is inconsistent: unsigned Q3.13 in 16 bits can only reach [0, 8).
  - Use the digit patterns in the tables: "xxx.xxxxxxxxxxxxx" for CPu bits 52:67 and ".xxxxxxxxxxxxxxxx" for CPd bits 35:50, which means 4·G lies in [0, 1).
- **A2 Time order of the b bits in a TRN2u symbol (Tables 28/29).**
  - The tables label the bit groups "MSB:LSB", and the MSB is plainly the sign. Nothing says which scrambler output bit is sent first. Clause 8's convention paragraph does not list Tables 28/29.
  - Two readings are possible:
    - (a) The first bit in time is the LSB, and the last bit of each symbol is the sign. This matches V.34 10.1.3.8, where I1n is first and In = 2·I2n + I1n, and clause 8's "integers LSB first".
    - (b) The first bit in time is the MSB, the sign, as the table layout reads.
  - **Recommended default: (a)**, marked UNVERIFIED.
  - This must be checked against a real V.92 server. If the server never answers our SUVu with CPd, flip the order. Make the order a single switch in the code.
  - The same question applies to the magnitude bits of the 8-point constellation: bit 1 versus bit 0 order.
- **A3 Table 23's last row** starts the extra fill at "289 + δ", the same position as the single fill bit. The second row presumably means 290+δ.
- **A4 TRN2u differential-encoder seed in rate renegotiation.**
  - 8.7.6 says the seed is the last sign of the preceding **E1u**, but in rate renegotiation TRN2u follows R̄u. In the silence path, TRN2u follows E2u.
  - **[interpretation]** Seed with the last transmitted sign of whatever immediately precedes TRN2u:
    - E1u in phase 4;
    - R̄u in rate renegotiation, whose last symbol is +LU, giving sign 0;
    - E2u in the silence path.
- **A5 9.6.2.1.4** says "complete sending the current CPu sequence", but the sequence in flight may be an SUVu (Figures 12-14 show SUVu' then E2u). **[interpretation]** Complete whichever sequence is in flight.
- **A6 Bit 33 in the rate-renegotiation silence path** (9.8.2.1.4, 9.8.1.1.3).
  - SUVu/SUVd are sent "with bit 33 set" to handshake entering silence, although no CP has been exchanged.
  - **[interpretation]** In that path bit 33 means "I received your SUV with bit 32" and is not literally "CPd received".
- **A7 Figures versus text for the post-E2u TRN2u length.**
  - Figure 16 shows "≥ 4008T" and Figure 17 shows "≥ 2400T". The text (9.8.2.1.5-6) gives an **upper** bound of 8004T, or "until Rt".
  - **[interpretation]** Follow the text: send until Rt arrives or 8004T has elapsed. When the analogue modem itself asked for silence, choose any length up to 8004T. The echo canceller needs enough; the figures suggest ≥ 2400T.
- **E1 Table 25, interval 11**, reads "u11 = 0", and **Table 26, interval 11**, reads "k11 = M11 − 1". Both are lowercase typos for K11, confirmed from the PDF font spans. By the 2-high/2-low pattern, K11 = 0 in RM and K11 = M11 − 1 in RM'.
- **E2 9.11** puts drn "in SUVu/SUVd", which have no drn field. **[interpretation]** It means CPu/CPus (bits 21:25) and CPd (bits 22:26).
- **E3** Same as A3.
- **E4 Table 31** (SUVd) says its reserved bits are "set to 0 by the analogue modem". Treat that as the digital modem, and ignore those bits on receipt.
- **A8 SUVu level resolution.** There is no code for "below −4 dB", because −4.00 means "no measurement". **[interpretation]** Clamp to −3.75.
- **A9 CPus Sr and ld.** CPus carries neither. **[interpretation]** The previous values stay in force.

### 8.4 Open questions

- **Q1 CPd constellation-point format.** The 16-bit "linear value" in Table 30 has no stated sign or Q format.
  - Its scale relative to G (4·G in Q0.16) and to the precoder coefficients is also undefined.
  - It needs a real CPd capture from a V.92 server. The Crazytel/NetZero-type servers in the project's test notes are candidates if any offer V.92 PCM upstream.
- **Q2 CPd omitting parts.** If a CPd omits the modulus, coefficient or constellation part, are the previous values kept?
  - The text implies it, but only states that the bits are removed.
  - In initial training all three parts must presumably be present. What should the analogue modem do if they are not: retrain?
- **Q3 V.90 constellation power rule.** Do the V.90 8.5.2 rules (the Table 15 ceiling, and data mode ≤ 3 dB above CPt) still bind a V.92 CPu? V.92 does not restate them.
  - Recommendation: comply anyway, since it costs nothing and the existing V.90 design code already does.
- **Q4 E2u extension contents.** What exactly goes into the 13th symbol? The assumption here is one more symbol of scrambled, differentially encoded zeros.
- **Q5 Precoder bypass for TRN2u.** Is TRN2u (and CPu/SUVu/E2u in training and rate renegotiation) sent without the precoder and prefilter? V.92 only states the bypass for Ru (8.5.5).
  - In phase 4 training no precoder exists yet, so a bypass is implied there. In rate renegotiation the same structure should logically be bypassed as well.
- **Q6 Timeouts during rate renegotiation and FPE.** No timeout is specified for the analogue modem in 9.8.2 or 9.9.2.
  - Suggestion: reuse the phase-4 style limit (20 s + 6·RTD) or a shorter local guard, and fall back to a retrain (9.7.2.1).
- **Q7 FPE memory state for RM.** Do RM/RM' continue the precoder, prefilter and trellis memories from data mode (assumed here), or restart them?
- **Q8 RTD for the analogue modem under short phase 2.** V.92 9.4 only defines RTDEd for the digital modem. The analogue modem needs its own RTD for the 100 ms + RTD rule.
  - Full phase 2 (V.90/V.34) gives RTDEa. With short phase 2 it is unclear where it comes from. It may be estimable from the Tone B/Tone A reversal timing in 9.4.2.1.3.

### 8.5 Suggested code shape (for the implementer; nothing here is normative)

- **New module**, `crates/datapump/src/v92/phase4_up.rs` or similar.
- **`UpBitPump`**: owns the GPA scrambler, the sign differential memory and the symbol-to-12 counter.
  - `trn2u_symbol()` resets the scrambler at the start of TRN2u.
  - `bits_to_symbols(bits, b)` implements Tables 28/29, with the A2 order switch.
- **`Cpu { kind: Cpt | Cpu | Cpus, drn, sr, ack, a_law, ld, trn1d_ratio, shaping: [i8; 4], intervals: [u8; 6], constellations: Vec<u128>, codec: Option<Vec<u128>> }`**
  - `to_bits(pad_unit_bits)` and `from_bits()`.
  - Reuse the `put`/`get` helpers and `v34::info::crc`. Copy the mask loop from `v90::sequences::Cp`, but move drn and type per §3.3.
- **`Suvu { wait_for_cpu, level_q2_2: i8, silence, ack }`**
  - `to_bits(pad_unit_bits)`, with level 16 meaning none.
- **`Cpd` parser**: parts gated by bits 19-21, α/β offsets, and coefficient words.
  - Unit-test it with a synthetic CPd containing every part.
  - Unit-test the "part absent" case too, checking that the CRC position moves.
- **`RmPattern`**: yields K_i for RM/RM' (Table 25/26) into the data-mode precoder, with the trellis running.
- **Tests worth writing** (none need external implementations):
  - CRC round trip on CPu, CPus and SUVu. Every length in §2.4.
  - Mask bit positions: u = 0 at bit 137, u = 15 at 152, u = 16 at 154, u = 127 at 271.
  - Type and drn positions: CPu with drn = 22 gives bits 21..25 = 0,1,1,0,1.
  - The E2u extension shifts B1u's interval 0 by one symbol.
  - TRN2u power is LU² for both constellations.
  - After TRN2u the descrambled bits are all ones (loopback through our own descrambler), and SUVu parses from the first 0 after the run.
