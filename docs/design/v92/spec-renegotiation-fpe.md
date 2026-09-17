# V.92 rate renegotiation (9.8) and fast parameter exchange (9.9): implementation digest

Source: ITU-T V.92 (11/2000), `docs/specs/T-REC-V.92-200011-I.pdf`. Clause numbers are V.92 unless another Recommendation is named. The Recommendation is paraphrased throughout; short quoted phrases are marked with quotation marks.

## 0. Sources, scope and conventions

### 0.1 Pages read

Every value below was read from pages rendered at 170 dpi. The lossy text extraction was used only to find pages. PDF page N of V.92 is printed page N - 7.

| Document | PDF pages | Content |
|---|---|---|
| V.92 | **60-65** | 9.7 (retrains, end), **9.8**, Figures 15-18, **9.9**, Figure 19, start of 9.10 |
| V.92 | 11-13 | 6.4 transmitter (Figures 1-2, 6.4.1-6.4.4), Table 1 (circuits), clause 8 preamble (bit order) |
| V.92 | 27, 29, 30, 31, 32 | Table 18 (INFO1a, Ltot/Lmax), 8.5.1-8.5.2 (CPt, E1u), 8.5.5 (Ru), Table 21 (Jd), Table 22 (Jp) |
| V.92 | 33-44 | 8.7 (B1u, E2u, CPu, CPus, RM, SUVu, TRN2u, FB1u; Tables 23-29) and 8.8 (B1d, Ed, CPd, R, SUVd, TRN2d; Tables 30-31) |
| V.92 | 56-59 | 9.6 Phase 4 (Figures 12-14, 9.6.1, 9.6.2) |
| V.92 | 69 | 9.11 Cleardown |
| V.90 (`T-REC-V.90-199809-I.pdf`) | 11 | Table 1 (Ucodes) |
| V.90 | 33-36 | 8.6 preamble (spectral shaping in Phase 4 and RR), 8.6.1 B1d, 8.6.2 Ed, 8.6.3 MP, 8.6.4 R, 8.6.5 TRN2d, Table 17 |
| V.90 | 45-47 | V.90's own 9.6 rate renegotiation and 9.7 cleardown, for comparison |
| V.34 (`T-REC-V.34-199802-I.pdf`) | 16, 33 | Clause 7 (scramblers), 10.1.2.3.2 and Figure 14 (CRC) |

### 0.2 Requirement tags

- **[SHALL]** marks a mandatory requirement ("shall").
- **[SHOULD]** marks a recommendation ("should").
- **[MAY]** marks an option ("may", "can", "is not required to").
- **[DEF]** marks a definition or a statement of fact in the text.
- **[INFERRED]** marks something that follows from the text but is not stated.
- **[OPEN]** marks something the text leaves undecided (see section 10.4).

### 0.3 Scope [INFERRED]

- 9.8 and 9.9 are written entirely in terms of PCM-upstream signals (Ru, RM, TRN2u, SUVu, CPu, E2u, FB1u).
- They therefore apply to connections in which the analogue modem selected **PCM upstream** (INFO1a per Table 18, 9.3/9.4).
- A V.92-capable pair that ends up in V.90 data mode (V.34 upstream, INFO1a per Table 19) has no PCM-upstream parameters. For such a connection, use V.90's own rate renegotiation (9.6/V.90); fast parameter exchange does not exist there.
- The text of 9.8/9.9 does not say this; see Q-20.

### 0.4 Notation

- **T** is one symbol interval: 1/8000 s = 125 us. Both directions run at 8000 symbols/s (5.2/V.90 downstream; 6.2 upstream).
- **Downstream data frame** (digital to analogue): 6 symbols, data frame intervals i = 0..5, i = 0 first in time. It carries D = K + S bits (5.4/V.90), where S + Sr = 6.
- **Upstream data frame** (analogue to digital): 12 symbols, i = 0..11 (Figure 1/V.92). It contains:
  - two constellation frames, j = 0..5 and 0..5;
  - three trellis frames, k = 0..3 each.

  In data mode it carries K bits, all of which enter the modulus encoder (6.4.1).
- **X-bar** (Rd-bar, Rt-bar, Rf-bar, Ru-bar) is the sign-inverted companion that terminates X. The Recommendation draws it with an overbar.
- **Prime (')** on SUVd', SUVu', CPd', CPu' means the sequence is sent with its acknowledge bit (bit 33) = 1.
  - RM' is **not** such a prime. It is a separate pattern (Table 26) that terminates RM.
  - 9.9.2.1.2 also writes "Rf'" where Rf-bar is meant (E-6).
- **RTD** is the round-trip delay. The digital modem's estimate is RTDEd (clause 4).
- **Circuits (Table 1/V.92):**
  - 104 received data;
  - 105 request to send;
  - 106 ready for sending;
  - 109 received line signal detector. Note 1 of Table 1: thresholds and response times do not apply to 109.

### 0.5 Bit order (clause 8 preamble) [SHALL]

- In Tables 2-5, 11-24, 27 and 30-33 (unless a table says otherwise):
  - a value written as a **bit pattern** is sent **leftmost bit first**;
  - a value written as an **integer** is sent **least-significant bit first**.
- This covers every sequence table used here: Tables 23, 24, 27, 30 and 31. Each of 8.7.3, 8.7.5, 8.8.3 and 8.8.5 also says "Bit 0 is transmitted first".
- So:
  - the 17-bit frame sync is 17 ones;
  - a field "bits a:b" holds an integer with bit a as its LSB;
  - the CRC field holds CRC register bit 0 at its lowest-numbered position (2.14).
- Tables 28/29 (TRN2u mapping) and 25/26 (RM) are **not** in that list. See Q-6.

### 0.6 Duration conversions (every duration used here is a whole number of frames)

| Duration | ms | Downstream frames (6T) | Upstream frames (12T) | Used for |
|---|---|---|---|---|
| 24T | 3 | 4 | 2 | Rd-bar, Rt-bar, Ru-bar, Rf-bar, RM' |
| 288T | 36 | 48 | n/a | B1d |
| 384T | 48 | 64 | 32 | Rd, Rt, Ru, Rf, RM |
| 576T | 72 | n/a | 48 | B1u, FB1u |
| 2040T | 255 | 340 | 170 | Phase 4 minimum TRN2d (not RR) |
| 2400T | 300 | 400 | 200 | earliest optional end of RR TRN2u |
| 4008T | 501 | 668 | 334 | Figure 16 label only (E-3) |
| 8004T | 1000.5 | 1334 | 667 | cap on post-silence TRN2u |
| 12000T | 1500 | 2000 | 1000 | Phase 4 minimum TRN2u (not RR) |
| 16008T | 2001 | 2668 | 1334 | cap on RR TRN2d and first RR TRN2u |

---

## 1. What the two procedures are

### 1.1 Rate renegotiation, RR (9.8, page 60)

- **[MAY] When:** either modem may start RR at any time in data mode (Figures 15-18).
- **[DEF] What it can change:** the data signalling rate and "other parameters".
- **[DEF] Other uses:** RR can also retrain the analogue modem's echo canceller, or its precoder and prefilter, without a full retrain. The optional **silent period** (bit 32 of SUVd/SUVu) serves this.
- **[DEF] Content:** a short retrain. Both modems send:
  1. an R-type initiating signal;
  2. TRN2 in training modulation;
  3. SUV sequences;
  4. optionally a silent period, ended by Rt;
  5. then the Phase 4 CP exchange of 9.6 from 9.6.x.1.2 onwards;
  6. finally E, B1 and data.

### 1.2 Fast parameter exchange, FPE (9.9, page 64)

- **[MAY] When:** either modem may start FPE at any time in data mode (Figure 19).
- **[DEF] What it can change:** the data signalling rate and other parameters.
- **[DEF] What it leaves out:** there is no TRN2 and no silence.
  - SUV, CP and E sequences are sent with the **preceding (old) data-mode modulation**, after a scrambler and differential-encoder reset.
  - The analogue modem inserts **FB1u** (48 frames, old modulation) between E2u and B1u.

### 1.3 Framing rules common to both

Stated separately in 9.8 and 9.9 with the same wording:

- **R-COM-1 [SHALL]:** both modems keep data frame synchronisation throughout the procedure.
  - This is new against V.90, whose 9.6 bound only the digital transmitter and the analogue receiver.
- **R-COM-2 [SHALL]:** a modem initiates RR or FPE only on a data frame boundary.
- **R-COM-3 [SHALL]:** a modem responds to RR or FPE only on a data frame boundary.
- **R-COM-4 [SHALL]:** each initiating or responding R signal (Rd, Rt, Ru, Rf, RM) begins on a data frame boundary. Each step clause repeats this.
  - For the digital modem this is a 6-symbol (downstream) boundary.
  - For the analogue modem it is a 12-symbol (upstream) boundary.
- **[INFERRED]** Every other sequence in both procedures is a whole number of that direction's frames:
  - TRN2d and TRN2u (8.6.5/V.90, 8.7.6);
  - SUV/CP fill (Tables 23, 24, 27, 30, 31);
  - Ed (2 frames) and E2u (1 frame; never extended in RR or FPE, CPd bit 29);
  - B1 and FB1u (48 frames).

  Frame alignment therefore survives without special handling, provided the silent period is also a whole number of frames (2.13).

### 1.4 Telling the initiators apart (informative)

| Signal | Sender | Period / signature | Level |
|---|---|---|---|
| Rd | digital | signs `+ + + - - -`, 6-symbol period: 1333.3 Hz fundamental plus harmonics | highest-power data-mode codeword per interval |
| Rt | digital | same as Rd | highest-power **training** (CPt) codeword per interval |
| Rf | digital | signs `+ + - -` repeating, 4-symbol sign period inside a 12-symbol block: 2000 Hz dominant | highest-power data-mode codeword per interval |
| Ru | analogue | `+LU +LU +LU -LU -LU -LU`: 1333.3 Hz | +/-LU (desired data-mode power), precoder and prefilter bypassed |
| RM | analogue | modulus-encoder output K = `M-1, M-1, 0, 0` repeating (4-symbol period in the **K domain**) | full data-mode chain (precoder, prefilter, trellis, G); see 2.5 |

- **[DEF] Precedence:**
  - A digital FPE initiator that detects Ru becomes the RR responder (9.9.1.1.2).
  - An analogue FPE initiator that detects Rd becomes the RR responder (9.9.2.1.2).
  - RR therefore overrides FPE.
- **[INFERRED]** A modem in data mode must watch for:
  - digital modem: Ru and RM;
  - analogue modem: Rd and Rf;
  - both: Tone A or Tone B (retrain, 9.7) and the modem-on-hold sequences (9.10, outside this digest).

---

## 2. Signals and sequences

### 2.1 Rd and Rd-bar (digital): 8.8.4/V.92, which refers to 8.6.4/V.90 (V.90 PDF p.35)

- **[DEF] Rd:** repeat a 6-symbol block of PCM codewords with sign pattern `+ + + - - -`, leftmost sign first.
- **[DEF] Rd-bar:** **exactly 4 repetitions** (24T) of the same 6 codewords with signs `- - - + + +`, leftmost first.
- **[DEF] Codeword in position p (0..5):** the **highest-power PCM codeword of the data-mode constellation of data frame interval i**, as passed in CP. In V.92 the downstream data-mode constellations come in **CPu** (Table 23, type 1).
  - Rd starts on a frame boundary (R-COM-4), so p = i.
  - "Data mode constellation" means the one currently in use: the last long CPu, since CPus carries no constellations. See Q-16 and Q-24.
- **[DEF] Polarity:**
  - "+" is the positive-voltage codeword: the octet of Table 1/V.90, whose MSB is the G.711 polarity bit (3.6/V.90).
  - "-" is the same octet with the MSB cleared. Examples: μ-law 0xFF/0x7F; A-law 0xD5/0x55 for Ucode 0.
  - V.90 5.4.6 uses the same convention for data: sign bit 1 is positive, 0 is negative.
- **[DEF] Note to 8.6.4/V.90:**
  - Neither R nor R-bar is differentially encoded.
  - The receiver must therefore detect them **regardless of polarity**.
- **Durations in 9.8:** Rd 384T (64 blocks), then Rd-bar 24T (9.8.1.1.1 and 9.8.1.2.2; both [SHALL]).

### 2.2 Rt and Rt-bar (digital): 8.8.4 and 8.6.4/V.90

- **[DEF]** Rt is signal R (sign patterns as in 2.1) using the **highest-power PCM codeword of the training constellation** of each data frame interval, as passed in **CPt**.
- In V.92, CPt is the analogue modem's Phase 3 sequence (8.5.1, Table 23 with type 0).
  - It gives the constellation used for TRN2d.
  - No new CPt is exchanged in RR, so the CPt of the most recent Phase 3 applies.
- **Durations:** Rt 384T, then Rt-bar 24T (9.8.1.1.4 and 9.8.1.1.5).
- **Use:** Rt ends the digital modem's silent period in RR. It is followed immediately by SUVd.

### 2.3 Rf and Rf-bar (digital): 8.8.4/V.92 (page 43)

- **[DEF] Rf:** repeat a **12-symbol** block of PCM codewords with signs `+ + - - + + - - + + - -`, leftmost first. The printed text says "transmitted by 0 repeating"; the stray "0" is a typo (E-5).
- **[DEF] Rf-bar:** **exactly 2 repetitions** (24T) of the same 12 codewords with signs `- - + + - - + + - - + +`.
- **[DEF] Codewords:** the highest-power PCM codeword of the **data-mode** constellation of each data frame interval, as passed in CPu.
  - The block spans two 6-symbol frames.
  - Rf starts on a frame boundary, so position p (0..11) uses interval p mod 6.
- **Durations in 9.9:** Rf 384T (32 blocks), then Rf-bar 24T (9.9.1.1.1 and 9.9.1.2.2; both [SHALL]).
- **[INFERRED]** 8.8.4 does not say whether Rf is differentially encoded.
  - It is built exactly like R and is sent after the data-mode encoder has stopped.
  - Treat it as not encoded and detect it polarity-blind (Q-17).

### 2.4 Ru and Ru-bar (analogue): 8.5.5/V.92 (page 30)

- **[DEF] Ru:** repeat the 6-symbol sequence `{+LU, +LU, +LU, -LU, -LU, -LU}`.
- **[DEF] Ru-bar:** repeat `{-LU, -LU, -LU, +LU, +LU, +LU}`.
- **[SHALL]** Bypass the precoder and prefilter while sending Ru and Ru-bar. Use the same transmit structure as for the 2-point TRN1u.
- **[DEF] LU (3.8):** the level at which TRN1u is sent at the desired data-mode transmit power. Ru therefore has the data-mode power.
- **Durations in 9.8:**
  - Ru is 384T: 64 blocks, which is 32 upstream frames.
  - Ru-bar is 24T: 4 blocks, which is 2 upstream frames.
  - Ru starts on an upstream frame boundary, so the 6-symbol pattern has phase 0 at i = 0 and at i = 6.
  - Clauses: 9.8.2.1.1 and 9.8.2.2.2.

### 2.5 RM and RM' (analogue): 8.7.4/V.92, Tables 25 and 26 (pages 36-37)

- **[DEF]** RM (and RM') is a repeated 12-symbol pattern.
- **[DEF]** The pattern gives the **modulus encoder output K_i** for each upstream data frame interval i, **not line values**.
- **[DEF]** M_i are the data-mode modulus parameters currently in force (the last CPd that carried them).

| i | RM (Table 25) | RM' (Table 26) |
|---|---|---|
| 0 | M0 - 1 | 0 |
| 1 | M1 - 1 | 0 |
| 2 | 0 | M2 - 1 |
| 3 | 0 | M3 - 1 |
| 4 | M4 - 1 | 0 |
| 5 | M5 - 1 | 0 |
| 6 | 0 | M6 - 1 |
| 7 | 0 | M7 - 1 |
| 8 | M8 - 1 | 0 |
| 9 | M9 - 1 | 0 |
| 10 | 0 | M10 - 1 |
| 11 | 0 (printed "u11 = 0", E-4) | M11 - 1 (printed "k11") |

**Transmission rules (8.7.4):**

- **[DEF]** Send RM and RM' with the data-mode constellation parameters.
- **[SHALL]** Use the same precoder and prefilter structure as the latest data mode.
- **[SHALL]** Trellis-encode RM and RM'.
  - K_i goes into the 6.4.2 precoder, which picks u(n) from the equivalence class E(K_i).
  - For k = 3 the class also depends on Y0 from the 6.4.4 convolutional encoder, as in data mode.
- **Durations in 9.9:** RM 384T (32 frames), then RM' 24T (2 frames) (9.9.2.1.1 and 9.9.2.2.2). RM begins at interval 0 (R-COM-4).
- **[INFERRED]** Nothing enters the modulus encoder while RM is sent: K_i is forced. Its sign and differential state d(f) is therefore not clocked.
  - FPE resets the "differential encoder" right after RM' anyway (9.9.2.1.2, 9.9.2.2.3).

**What the digital modem sees (informative, derived from 6.4.2; this corrects an earlier draft):**

- **Classes.** E(K) = {a(eta) : eta = K + z*M} for k = 0, 1, 2. For k = 3 it is {a(eta) : eta = 2K + 2zM + ((eta0 + eta1 + eta2 + Y0) mod 2)}.
- **The line signal is not a clean tone.**
  - The precoder may pick **any** member of E(K_i). The 8.8.3 NOTE only says the digital modem should design for a symbol-by-symbol minimum-power choice.
  - The precoder output x(n) contains the feedback terms (z1, p1), and the prefilter reshapes it further.
  - So with non-trivial coefficients the line signal is noise-like, like Tomlinson-Harashima output.
  - A "2000 Hz tone" detector is therefore **not** reliable for RM.
- **Detect RM in the K domain.** The digital modem's normal data-mode receiver already recovers K_i from each decided point index:
  - K_i = eta mod M_i for k = 0..2;
  - K_i = ((eta - parity) / 2) mod M_i for k = 3.

  During RM the recovered sequence is `M-1, M-1, 0, 0, M-1, M-1, 0, 0, ...`, which random data essentially never produces for 32 frames. During RM' it is shifted by two symbols.
- **Transition signature.** RM ends `..., M-1, M-1, 0, 0` (i = 8..11) and RM' starts `0, 0, M-1, M-1` (i = 0..3), so the RM-to-RM' transition shows up as **four consecutive K = 0**.
- **Pitfall.** If some M_i = 1, then M_i - 1 = 0 and that interval carries no information. The detector must mask such intervals (Q-19).

### 2.6 TRN2d (digital): 8.8.6/V.92, which refers to 8.6.5/V.90 (V.90 PDF p.36)

- **[DEF] Content:** scrambled binary ones applied to the encoder of 5.4/V.90. The constellation set is the one passed in **CPt**.
- **[SHALL] Reset:** before TRN2d, initialise the scrambler, the differential encoder and the spectral-shaping filter memory to zero.
- **[SHALL] First frame:** the first data frame's magnitudes are those produced by mapping the first D scrambled ones after the scrambler reset. They must be identical for every value of ld.
- **[DEF] K and S:** permitted combinations are in Table 17/V.90: K = 6..24, S = 3..6, giving 12 to 40 kbit/s.
  - CPt signals the rate as (drn + 8) x 8000/6, so D = K + S = drn_t + 8.
- **[SHALL] Length:** a whole number of 6-symbol frames.
- **[SHALL] In RR:** "for up to 16008T" (9.8.1.1.2). No minimum is given (Q-3).
- **[OPEN] Spectral shaping in RR (Q-18).** V.90's 8.6 preamble (PDF p.33) says:
  - during an initial train or retrain, TRN2d, MP and Ed use the shaping parameters defined by CPt;
  - **during rate renegotiation they use the shaping parameters of the preceding data mode, together with the K previously derived from CPt**;
  - B1d and later use the parameters of the new CP.

  V.92's 8.8 has **no** preamble and 8.8.6 imports only 8.6.5/V.90, so it is unclear whether that rule carries over. It matters because it changes D, and with it the fill length of SUVd, CPd and Ed in RR (2.17).

### 2.7 TRN2u (analogue): 8.7.6/V.92, Tables 28 and 29 (pages 38-39)

- **[DEF] Constellation size:** 4-point or 8-point, as the digital modem requested in **Jp** (Table 22, page 32).
  - **Jp bit 48:** size for CPu, E2u, SUVu and TRN2u during **training** (Phase 4).
  - **Jp bit 49:** size for the same sequences during **rate renegotiation**.
  - Both bits: 0 = 4-point, 1 = 8-point.
  - Jp bit 47 is the Jd/Jp identifier (1 = Jp), bit 50 is reserved and bit 51 is a start bit. The extracted text file mislabels these rows; the rendered table is authoritative.
  - CPus is not named in Jp, but 8.7.3 sends it with the TRN2u parameters in RR, so the bit 49 size applies to it too.
- **[DEF] Content:** scrambled binary ones.
- **[SHALL] Mapping:** scrambler output to symbols per Tables 28/29.
- **[SHALL] Scrambler:** reset at the start of TRN2u (GPA, 6.3). This applies to the first RR TRN2u and to the post-silence TRN2u.
- **[SHALL] Sign bit:** differentially encoded. The transmitted sign is the present sign bit XOR the previously transmitted sign bit.
- **[SHALL] Differential encoder memory:** initialised with the last transmitted sign bit of "the preceding E1u". That is true only in Phase 4; in RR the preceding sequence is Ru-bar or E2u (E-10, Q-5).
- **[SHALL] Length:** a whole number of 12-symbol frames.
- **[MAY]** The digital modem may use TRN2u to estimate the upstream analogue channel.

**Table 28 (4-point).** The column is headed MSB:LSB. The MSB is the sign (0 = positive) and the LSB selects the magnitude.

| MSB:LSB | value |
|---|---|
| 00 | +(1/sqrt5) LU |
| 01 | +(3/sqrt5) LU |
| 10 | -(1/sqrt5) LU |
| 11 | -(3/sqrt5) LU |

**Table 29 (8-point).** The MSB is the sign; the two LSBs select magnitude 1, 3, 5 or 7.

| MSB:LSB | value |
|---|---|
| 000 | +(1/sqrt21) LU |
| 001 | +(3/sqrt21) LU |
| 010 | +(5/sqrt21) LU |
| 011 | +(7/sqrt21) LU |
| 100 | -(1/sqrt21) LU |
| 101 | -(3/sqrt21) LU |
| 110 | -(5/sqrt21) LU |
| 111 | -(7/sqrt21) LU |

- **[INFERRED] Power:** both constellations have mean square LU^2: (1 + 9)/2/5 = 1 and (1 + 9 + 25 + 49)/4/21 = 1.
- **[INFERRED] Bits per symbol:** 2 (4-point) or 3 (8-point), so one 12-symbol frame holds 24 or 36 bits.
- **[OPEN] Bit order and path:** the time order of the 2 or 3 bits within a symbol is not stated (Q-6). Neither is whether the magnitude bits are differentially encoded (only "the sign bit" is named) nor whether TRN2u passes through the precoder and prefilter (Q-8).

### 2.8 SUVd (digital): 8.8.5/V.92, Table 31 (pages 43-44)

- **[DEF]** A short information sequence, scrambled (GPC).
- **[DEF] Modulation:**
  - in **training and RR**: the corresponding **TRN2d modulation** (V.90 encoder, CPt constellation; see 2.6);
  - in **FPE**: the **preceding data-mode modulation**.
- **[DEF] Differential encoder:** initialised from the last transmitted sign bit of the preceding sequence.
  - After Rt-bar, the last transmitted sign is **+** (sign bit 1), because Rt-bar ends `+ + +` (see Q-11).
- **[DEF] Naming:** an SUVd with the acknowledge bit set is SUVd'.
- **[DEF] Bit order:** bit 0 first.
- **[DEF] CRC:** 10.1.2.3.2/V.34 (2.14).
- **[SHALL]** SUVd and SUVd' sequences sent as a group all carry identical information. Only bit 33 may differ between the plain and primed forms (see 3.5 for the silence handshake).

| Bits (LSB:MSB) | Width | Field |
|---|---|---|
| 0:16 | 17 | Frame sync `11111111111111111` |
| 17 | 1 | Start bit 0 |
| 18 | 1 | Sequence identifier: **1** = SUV (a CP has 0 here) |
| 19:31 | 13 | Reserved. The table says "set to 0 by the analogue modem, not interpreted by the digital modem", copied from Table 27 (E-2). Send 0 and ignore on receipt. |
| 32 | 1 | 1 = **a silent period is requested**. [MAY] be used during RR (9.8.1.1). |
| 33 | 1 | **Acknowledge:** 0 = CPu not yet received from the analogue modem; 1 = CPu received |
| 34 | 1 | Start bit 0 |
| 35:50 | 16 | CRC over bits 18..33 |
| 51 | 1 | Fill bit 0 |
| 52:... | | Fill 0s up to the next multiple of **6 symbols** |

- **[INFERRED] Length.** "A multiple of 6 symbols" means a whole number of downstream data frames. Each frame carries D bits of the modulation in use:
  - RR: D from the TRN2d parameters (2.6, Q-18);
  - FPE: the data-mode D = drn_u + 20 of the preceding CPu.

  The sequence is 52 bits padded up to a multiple of D. This is the same rule V.90's MP uses (`mp_bits`).

### 2.9 SUVu (analogue): 8.7.5/V.92, Table 27 (pages 37-38)

- **[DEF]** A short information sequence, scrambled (GPA).
- **[DEF] Modulation:**
  - in **training and RR**: the corresponding **TRN2u modulation** (4- or 8-point, differential sign);
  - in **FPE**: the **preceding data-mode modulation**.
- **[DEF] Differential encoder:** initialised from the last transmitted sign bit of the preceding sequence.
- **[DEF] Naming:** SUVu with bit 33 set is SUVu'.
- **[DEF] Bit order:** bit 0 first.
- **[DEF] CRC:** 10.1.2.3.2/V.34.
- **[SHALL]** Sequences sent as a group carry identical information.

| Bits | Width | Field |
|---|---|---|
| 0:16 | 17 | Frame sync, 17 ones |
| 17 | 1 | Start bit 0 |
| 18 | 1 | Identifier: **1** = SUV (CPu and CPus have 0) |
| 19:25 | 7 | Reserved: the analogue modem sends 0, the digital modem ignores them |
| 26 | 1 | 1 = the analogue modem asks the digital modem to **wait for a CPu before sending CPd**. [MAY] The digital modem need not comply. |
| 27:31 | 5 | 20 x log10(L), where L is the measured RMS level of (prefilter output x G). Signed Q2.2 ("sxx.xx"): 5-bit two's complement with 2 fraction bits, range -4.00 .. +3.75 dB, integer LSB (bit 27) first. The value **16** (bit 31 = 1, bits 27..30 = 0, which is -4.00) means **no measurement taken**. |
| 32 | 1 | 1 = **a silent period is requested**. [MAY] be used during RR (9.8.2.1). |
| 33 | 1 | **Acknowledge:** 0 = CPd not yet received; 1 = CPd received |
| 34 | 1 | Start bit 0 |
| 35:50 | 16 | CRC over bits 18..33 |
| 51 | 1 | Fill bit 0 |
| 52:... | | Fill 0s up to the next multiple of **12 symbols** |

- **[INFERRED] Length.**
  - RR, 4-point (24 bits per frame): 72 bits = 3 frames = 36 symbols.
  - RR, 8-point (36 bits per frame): 72 bits = 2 frames = 24 symbols.
  - FPE (K = 2(drn_d + 17) bits per frame, 36..72): 2 frames if K < 52, 1 frame if K >= 52 (Q-7).
- **[INFERRED] Meaning of L.** 8.8.3 has the digital modem design its parameters so that G*v(n) with mean square 1 gives the desired power. So 0 dB here means "on target".

### 2.10 CPd (digital): 8.8.3/V.92, Table 30 (pages 39-43)

**Structure [DEF]:**

- CPd carries the **analogue modem's** data-mode parameters in four parts:
  - part 1, bits 0..50: always sent;
  - modulus-encoder parameters: present if bit 19 = 1;
  - prefilter and precoder coefficients: present if bit 20 = 1;
  - constellation sets: present if bit 21 = 1.
- **Absent parts are removed entirely**, so everything after them moves up. The bit positions in the table assume **all parts are present**.
- Every CPd ends with the CRC and at least one fill bit.

**Transmission:**

- **[DEF] Scrambling and modulation:** scrambled (GPC); TRN2d modulation in training and RR; preceding data-mode modulation in FPE.
- **[DEF] Differential encoder (training and RR):** initialised from the last transmitted sign bit of the preceding sequence.
- **[DEF] Naming and order:** CPd' has bit 33 set. Bit 0 is sent first.
- **[DEF] CRC:** 10.1.2.3.2/V.34.
- **[SHALL]** CPd and CPd' sent as a group carry identical information.

**Variable lengths [DEF]:**

- alpha = 17 x (LZ1 + LP1 + LZ2 + LP2)
- beta = 17 x (LC1 + LC2 + LC3 + LC4 + LC5 + LC6)
- Every word is 17 bits: a start bit 0 followed by 16 bits.
- The modulus-encoder part is 6 words (bits 51..152, 102 bits).
- The precoder/prefilter part is 4 + LZ1 + LP1 + LZ2 + LP2 words (bits 153..220 + alpha).
- The constellation part is 5 + LC1 + ... + LC6 words (bits 221 + alpha .. 305 + alpha + beta).

**Constraints:**

- **[SHALL]** LZ1 + LP1 + LZ2 + LP2 must not exceed **Ltot**, INFO1a bits 14:15 (Table 18): 0 = 192, 1 = 256, 2 = 320, 3 = 384.
- **[DEF]** Each of LZ1, LP1, LZ2 and LP2 is "up to" **Lmax**, INFO1a bits 16:17: 0 = 128, 1 = 192, 2 = 256, 3 = 320.
- **[INFERRED]** INFO1a bits 12:13 say which filter sections the analogue modem supports:
  - 0 = p1, z2;
  - 1 = z1, p1, z2;
  - 2 = p1, p2, z2;
  - 3 = z1, p1, p2, z2.

  Sections it does not support must have length 0.
- **[SHALL]** Constellations must not contain the zero point.
- **[SHALL]** All non-zero-size constellation sets are listed first.
- **[SHALL]** A set has at most 128 points. Whether that bounds N or LC is unclear.
- **[SHALL]** The digital modem designs the parameters assuming that the analogue modem transmits at the desired power when (prefilter output x G) has mean square 1.
- **[SHOULD] (NOTE)** The digital modem designs the precoder coefficients assuming the analogue modem minimises the precoder output power symbol by symbol.

| Bits (all parts present) | Width | Field |
|---|---|---|
| 0:16 | 17 | Frame sync, 17 ones |
| 17 | 1 | Start 0 |
| 18 | 1 | Identifier: **0** = CP |
| 19 | 1 | 1 = modulus encoder parameters present |
| 20 | 1 | 1 = prefilter and precoder coefficients present |
| 21 | 1 | 1 = constellation sets present |
| 22:26 | 5 | drn, 0..19: selected **analogue-to-digital (upstream)** rate = (drn + 17) x 8000/6 (24 000 .. 48 000 bit/s). **[SHALL] drn = 0 indicates cleardown.** |
| 27:28 | 2 | Upstream trellis encoder: 0 = 16-state, 1 = 32-state, 2 = 64-state, 3 = reserved. The digital receiver requires the analogue transmitter to use it. |
| 29 | 1 | Extend E2u by one symbol (0 = no, 1 = yes). **[SHALL] 0 during RR and FPE.** |
| 30:32 | 3 | Reserved (digital sends 0; analogue ignores) |
| 33 | 1 | Acknowledge: 0 = CPu not yet received; 1 = CPu received |
| 34 | 1 | Start 0 |
| 35:50 | 16 | 4 x G (> 0), where G is the gain at the prefilter output; unsigned Q0.16 |
| *modulus encoder part* | | |
| 51 | 1 | Start 0 |
| 52:59, 60:67 | 8 + 8 | M0, M1 |
| 68 | 1 | Start 0 |
| 69:76, 77:84 | 8 + 8 | M2, M3 |
| 85 | 1 | Start 0 |
| 86:93, 94:101 | 8 + 8 | M4, M5 |
| 102 | 1 | Start 0 |
| 103:110, 111:118 | 8 + 8 | M6, M7 |
| 119 | 1 | Start 0 |
| 120:127, 128:135 | 8 + 8 | M8, M9 |
| 136 | 1 | Start 0 |
| 137:144, 145:152 | 8 + 8 | M10, M11 |
| *precoder / prefilter part* | | |
| 153 | 1 | Start 0 |
| 154:162 | 9 | LZ1: taps in the precoder feed-forward section |
| 163:169 | 7 | Reserved (0 / ignored) |
| 170 | 1 | Start 0 |
| 171:179 | 9 | LP1: taps in the precoder feedback section |
| 180:186 | 7 | Reserved |
| 187 | 1 | Start 0 |
| 188:196 | 9 | LZ2: taps in the prefilter feed-forward section |
| 197:203 | 7 | Reserved |
| 204 | 1 | Start 0 |
| 205:213 | 9 | LP2: taps in the prefilter feedback section |
| 214:220 | 7 | Reserved |
| 221 | 1 | Start 0 (of the first coefficient word) |
| 222:237 | 16 | z1(1), signed Q0.15 (present if LZ1 > 0) |
| ... | | z1(2) .. z1(LZ1). Word k (1-based) has its start bit at 221 + 17(k-1) and its value at 222 + 17(k-1) .. 237 + 17(k-1). |
| 221 + 17 LZ1 | 1 | Start 0 |
| 222 + 17 LZ1 : 237 + 17 LZ1 | 16 | p1(1), signed Q1.14; then p1(2) .. p1(LP1) |
| 221 + 17 (LZ1 + LP1) | 1 | Start 0 |
| ... | | z2(0) .. z2(LZ2 - 1), signed Q0.15 (note: index starts at **0**) |
| 221 + 17 (LZ1 + LP1 + LZ2) | 1 | Start 0 |
| ... | | p2(1) .. p2(LP2), signed Q1.14 (present if LP2 > 0) |
| *constellation part* | | |
| 221 + a | 1 | Start 0 |
| 222 + a : 225 + a | 4 | Constellation index (0..5) for intervals **0 and 6** |
| 226 + a : 229 + a | 4 | Index for intervals 1 and 7 |
| 230 + a : 233 + a | 4 | Index for intervals 2 and 8 |
| 234 + a : 237 + a | 4 | Index for intervals 3 and 9 |
| 238 + a | 1 | Start 0 |
| 239 + a : 242 + a | 4 | Index for intervals 4 and 10 |
| 243 + a : 246 + a | 4 | Index for intervals 5 and 11 |
| 247 + a : 254 + a | 8 | Reserved |
| 255 + a | 1 | Start 0 |
| 256 + a : 263 + a | 8 | LC1: number of positive points in set 1 |
| 264 + a : 271 + a | 8 | LC2 (possibly 0) |
| 272 + a | 1 | Start 0 |
| 273 + a : 280 + a | 8 | LC3 (possibly 0) |
| 281 + a : 288 + a | 8 | LC4 (possibly 0) |
| 289 + a | 1 | Start 0 |
| 290 + a : 297 + a | 8 | LC5 (possibly 0) |
| 298 + a : 305 + a | 8 | LC6 (possibly 0) |
| 306 + a | 1 | Start 0 |
| 307 + a : 322 + a | 16 | "Linear value" of the 1st (smallest-magnitude) point of set 1 (format not stated, Q-12) |
| 323 + a | 1 | Start 0 |
| ... | | One 17-bit word per point, in increasing magnitude, up to the largest point of set 1 |
| 306 + a + 17 LC1 | 1 | Start 0; then the next non-empty set in the same format |
| 306 + a + b | 1 | Start 0 (end of CPd) |
| 307 + a + b : 322 + a + b | 16 | CRC |
| 323 + a + b | 1 | Fill bit 0 |
| 324 + a + b : ... | | Fill 0s up to the next multiple of **6 symbols** |

In this table a = alpha and b = beta.

**[DEF] Point layout (6.4.2):**

- Set n has N = 2 x LCn points, a(eta) with -N/2 <= eta < N/2, indices in level order.
- Negative indices are the negative points.
- **[INFERRED]** Only the LC positive magnitudes are sent. The negative points mirror them, a(-eta-1) = -a(eta); this is implied, not stated.
- **[INFERRED]** "Index 0..5" refers to the 1st..6th set.

**[INFERRED] CRC coverage:**

- Every bit that is not frame sync, a start bit or fill.
- With all parts present that is bits 18..33, 35..50 and every 16-bit value after that, in transmission order.
- Absent parts contribute nothing.

### 2.11 CPu (long) and CPus (short) (analogue): 8.7.3/V.92, Tables 23 and 24 (pages 33-36)

**CPu:**

- **[DEF] Content:** the **digital modem's** data-mode parameters.
- **[DEF] Modulation:** TRN2u modulation in training and RR; "the same modulation parameters as data mode" in FPE.
- **[DEF] Differential encoder (training and RR):** initialised from the last transmitted sign bit of the preceding sequence.
- **[DEF] Naming and order:** CPu' has bit 33 set. Bit 0 is sent first.
- **[DEF] CRC:** 10.1.2.3.2/V.34.
- **[SHALL]** CPu and CPu' sent as a group carry identical information.
- **[DEF]** CPt (Phase 3) uses the same Table 23 with type 0.

| Bits | Width | Field |
|---|---|---|
| 0:16 | 17 | Frame sync, 17 ones |
| 17 | 1 | Start 0 |
| 18 | 1 | Identifier: **0** = CP |
| 19:20 | 2 | Type (integer, LSB first): **0 = CPt, 1 = CPu** (**2 = CPus**, Table 24; 3 unused) |
| 21:25 | 5 | drn, 0..22: selected **digital-to-analogue (downstream)** rate = (drn + 20) x 8000/6 in CPu (28 000 .. 56 000 bit/s), or (drn + 8) x 8000/6 in CPt. drn = 0 means cleardown. |
| 26:30 | 5 | Reserved (analogue sends 0; digital ignores) |
| 31:32 | 2 | Sr: sign bits used as spectral-shaping redundancy (0..3; S = 6 - Sr) |
| 33 | 1 | Acknowledge: 0 = CPd not yet received; 1 = CPd received |
| 34 | 1 | Start 0 |
| 35 | 1 | Codec type: 0 = μ-law, 1 = A-law |
| 36:48 | 13 | Reserved |
| 49:50 | 2 | ld: look-ahead frames requested for spectral shaping. **[SHALL]** Consistent with the digital modem's capability in Jd bits 49:50 (1..3). |
| 51 | 1 | Start 0 |
| 52:67 | 16 | RMS of TRN1d at the transmitter output / RMS of TRN1d at the codec D/A output; unsigned Q3.13 |
| 68 | 1 | Start 0 |
| 69:76, 77:84 | 8 + 8 | a1, a2: spectral-shaping filter parameters, signed Q1.6 |
| 85 | 1 | Start 0 |
| 86:93, 94:101 | 8 + 8 | b1, b2: signed Q1.6 |
| 102 | 1 | Start 0 |
| 103:106, 107:110, 111:114, 115:118 | 4 x 4 | Constellation index (0..5) for downstream intervals 0, 1, 2, 3 |
| 119 | 1 | Start 0 |
| 120:123, 124:127 | 4 + 4 | Index for intervals 4 and 5 |
| 128 | 1 | 1 = the transmitter constellations differ from those at the codec D/A output |
| 129:135 | 7 | Reserved |
| 136 | 1 | Start 0 |
| 137:152 | 16 | Mask of constellation 0, Uchord1 (bit 137 = Ucode 0 ... bit 152 = Ucode 15) |
| 153 / 154:169 | 1 / 16 | Start 0 / Uchord2 (bit 154 = Ucode 16) |
| 170 / 171:186 | 1 / 16 | Start 0 / Uchord3 (bit 171 = Ucode 32) |
| 187 / 188:203 | 1 / 16 | Start 0 / Uchord4 (bit 188 = Ucode 48) |
| 204 / 205:220 | 1 / 16 | Start 0 / Uchord5 (bit 205 = Ucode 64) |
| 221 / 222:237 | 1 / 16 | Start 0 / Uchord6 (bit 222 = Ucode 80) |
| 238 / 239:254 | 1 / 16 | Start 0 / Uchord7 (bit 239 = Ucode 96) |
| 255 / 256:271 | 1 / 16 | Start 0 / Uchord8 (bit 256 = Ucode 112) |
| 272 : 271 + gamma | | Further constellations (indices 1..), each in the 136-bit format of bits 136:271 |
| 272 + gamma : 271 + delta | | The corresponding codec (D/A-side) constellations, same format; present only if bit 128 = 1 |
| 272 + delta | 1 | Start 0 |
| 273 + delta : 288 + delta | 16 | CRC |
| 289 + delta | 1 | Fill 0 |
| 289 + delta : ... | | Fill 0s up to the next multiple of **12 symbols** (the table repeats 289 + delta; the run really starts at 290 + delta, E-9) |

**Mask and variable-length rules [DEF]:**

- A mask is 128 bits; bit = 1 means that Ucode is in the constellation.
- Mask bit for Ucode u in constellation n: 137 + 136n + 17*floor(u/16) + (u mod 16).
- Constellations identical across intervals are sent once, indexed 0 (bits 136:271) up to at most 5 (bits 816:951).
- gamma = 136 x (the largest index in bits 103:127).
- delta = 2 x gamma + 136 if bit 128 = 1, otherwise delta = gamma.
- **[SHALL]** When the transmitter and D/A constellations differ, bit 128 is set and one D/A constellation is sent per transmit constellation.

**CPus (Table 24, page 36):**

- **[DEF] Use:** a short CPu for RR and FPE when **the digital modem's modulation parameters are not changed**.
- **[DEF] Modulation:** the TRN2u parameters in RR; the data-mode modulation in FPE.
- **[DEF] Differential encoder (RR):** initialised from the last transmitted sign bit of the preceding sequence.
- **[DEF] Order and CRC:** bit 0 first; CRC per 10.1.2.3.2/V.34.

| Bits | Width | Field |
|---|---|---|
| 0:16 | 17 | Frame sync, 17 ones |
| 17 | 1 | Start 0 |
| 18 | 1 | Identifier: 0 = CP |
| 19:20 | 2 | **2** (integer, LSB first, so bit 19 = 0 and bit 20 = 1) |
| 21:25 | 5 | drn, 0..22: downstream rate = (drn + 20) x 8000/6; 0 = cleardown |
| 26:32 | 7 | Reserved (analogue sends 0; digital ignores) |
| 33 | 1 | Acknowledge: 1 = CPd received |
| 34 | 1 | Start 0 |
| 35:50 | 16 | CRC over bits 18..33 |
| 51 | 1 | Fill 0 |
| 52:... | | Fill 0s up to the next multiple of 12 symbols |

- **[INFERRED]** The group-identity rule is stated for CPu/CPu'. Apply it to CPus as well.
- **[INFERRED]** For ack purposes (9.6.x.1.2-.4), a CPus counts as a CPu.

### 2.12 Ed, E2u, B1d, B1u, FB1u

**Ed (8.8.2/V.92):**

- **[DEF]** 2 downstream data frames of scrambled binary **zeros**.
- **[DEF] Modulation:** TRN2d in training and RR; preceding data-mode modulation in FPE.
- V.90 8.6.2 used Ed to end MP. In V.92 it ends the SUVd/CPd exchange. In the RR silence branch it is followed by silence.

**E2u (8.7.2/V.92):**

- **[DEF]** One upstream data frame (12 symbols) of scrambled, differentially encoded **zeros**.
- **[DEF] Modulation:** TRN2u in training and RR; preceding data-mode modulation in FPE.
- **[SHALL]** Extended by one symbol if CPd bit 29 = 1. That bit is always 0 in RR and FPE, so **E2u is always exactly 12 symbols here**.

**B1d (8.8.1/V.92, defined in 8.6.1/V.90):**

- **[DEF]** 48 downstream data frames (288T) of scrambled ones, using the newly selected data-mode constellation parameters.
- **[SHALL]** Scrambler, differential encoder and spectral-shaping filter memory are zeroed before B1d.
- **[SHALL]** The first frame's magnitudes come from mapping the first D scrambled ones after the reset. They are identical for every ld.
- **[DEF]** K and S are as in Table 2/V.90.
- **[DEF]** B1d and everything after it use the shaping parameters of the new CPu (V.90 8.6 preamble).

**B1u (8.7.1/V.92):**

- **[DEF]** 48 upstream data frames (576T) of scrambled ones, using the data-mode constellation parameters of the preceding CPd.
- **[DEF]** Its first output symbol is n = 0 in the 6.4.2 precoder and prefilter equations.
- **[SHALL]** The first symbol of B1u starts data frame interval 0.
- **[DEF]** The scrambler, modulus encoder, convolutional encoder, precoder and prefilter memories are zeroed before B1u.

**FB1u (8.7.7/V.92):**

- **[DEF]** 48 upstream data frames (576T) of scrambled, differentially encoded **ones**, using the **preceding (old)** data-mode modulation.
- Sent only in FPE, between E2u and B1u (9.6.2.1.5).
- No reset is specified before FB1u.
- **[INFERRED]** B1u therefore starts exactly 49 frames after E2u began (1 + 48). The digital receiver can switch to the new parameters at that frame boundary without detecting anything.

### 2.13 Silence (digital, RR only): 9.8.1.1.3

- **[SHALL]** Generate silence by sending PCM codewords whose magnitude is **Ucode 0**:
  - μ-law 0xFF or 0x7F (linear 0);
  - A-law 0xD5 or 0x55 (linear +8 or -8).
- The sign is not specified.
- **[SHALL]** Keep data frame alignment during the silence.
- **[INFERRED]** End the silence on a frame boundary, since Rt must begin on one (R-COM-4). The silence is then a whole number of 6-symbol frames.
- **[INFERRED]** On A-law, use a constant sign. Alternating +/-8 would put a small 4 kHz tone on the line.

### 2.14 CRC: 10.1.2.3.2/V.34 and Figure 14/V.34 (V.34 PDF p.33)

- **[DEF] Polynomial:** x^16 + x^12 + x^5 + 1.
- **[DEF] Input:** every information bit of the sequence **except** frame-sync, start and fill bits, in transmission order. For SUVd, SUVu and CPus that is bits 18..33 (16 bits).
- **[DEF] Procedure:**
  1. Load the register with all ones.
  2. Shift the bits in.
  3. Output the register contents starting with register bit 0, which is the CRC LSB.
- **[DEF]** There is no final inversion.
- **[DEF] Figure 14 wiring:**
  - cells 15 ... 0, shifting from 15 towards 0;
  - feedback f = cell0 XOR input bit;
  - f enters cell 15 and is XORed into the inputs of cells 10 and 3.
- Equivalent code:

```
reg = 0xFFFF
for bit in info_bits:              # transmission order
    f = (reg & 1) ^ bit
    reg >>= 1
    if f: reg ^= 0x8408            # bits 15, 10, 3
crc_field_bit[k] = (reg >> k) & 1  # k = 0 is sent first
```

- **Repo:** `crates/datapump/src/v34/info.rs::crc` implements exactly this (read, not modified).

### 2.15 Scramblers: clause 7/V.34 (V.34 PDF p.16)

- **[DEF] Digital modem (5.3/V.90):** GPC = 1 + x^-18 + x^-23 (7-1).
- **[DEF] Analogue modem (6.3/V.92):** GPA = 1 + x^-5 + x^-23 (7-2).
- **[DEF] Operation:** both are self-synchronising. The transmitter "divides" the data by the polynomial, which gives:
  - GPC: out(n) = in(n) XOR out(n-18) XOR out(n-23);
  - GPA: out(n) = in(n) XOR out(n-5) XOR out(n-23).

  The descrambler applies the same taps to the received stream.
- **Resets that matter here:**

| Reset | Where specified | Resets |
|---|---|---|
| TRN2d (RR) | 8.6.5/V.90 | GPC scrambler, differential encoder, shaping filter |
| TRN2u (RR, both segments) | 8.7.6 | GPA scrambler |
| SUVd in FPE | 9.9.1.1.2, 9.9.1.2.3 | GPC scrambler, differential encoder, shaping filter, right after Rf-bar |
| SUVu in FPE | 9.9.2.1.2, 9.9.2.2.3 | GPA scrambler and differential encoder (6.4.1 d(f)), right after RM' |
| B1d | 8.6.1/V.90 | scrambler, differential encoder, shaping filter |
| B1u | 8.7.1 | scrambler, modulus encoder, convolutional encoder, precoder, prefilter |

- **[INFERRED] Why the FPE resets exist.** A self-synchronising descrambler outputs garbage for 23 bits after any discontinuity. With a known zero state at a known frame boundary (the end of Rf-bar or RM'), the far receiver can zero its descrambler at the same boundary. The 17-bit frame sync of the first SUV then decodes correctly.

### 2.16 Which modulation each sequence uses

| Sequence | Phase 4 | RR | FPE |
|---|---|---|---|
| Initiator | n/a | Rd (PCM, data-mode peaks) / Ru (+/-LU, bypass) | Rf (PCM, data-mode peaks) / RM (full data-mode chain, trellis coded) |
| TRN2d | CPt constellation, at least 2040T | CPt constellation, at most 16008T (shaping per Q-18) | not sent |
| TRN2u | 4/8-point per **Jp bit 48**, at least 12000T before SUVu | 4/8-point per **Jp bit 49**, at most 16008T (may stop after 2400T or on SUVd); post-silence at most 8004T | not sent |
| SUVd, CPd, Ed | TRN2d modulation | TRN2d modulation | old data-mode modulation (SUVd after the 9.9.1 reset) |
| SUVu, CPu, CPus, E2u | TRN2u modulation (bit 48 size) | TRN2u modulation (bit 49 size) | old data-mode modulation (SUVu after the 9.9.2 reset) |
| silence, Rt | n/a | Ucode 0; Rt uses CPt peaks | n/a |
| FB1u | not sent | not sent | old data-mode modulation, 48 frames |
| B1d / B1u | new data mode | new data mode | new data mode |

### 2.17 Bits per frame (for fill lengths and E detection) [INFERRED]

| Context | Downstream bits per 6-symbol frame | Upstream bits per 12-symbol frame |
|---|---|---|
| RR (training modulation) | D_trn = K_t + S. If V.90's 8.6 rule applies (Q-18): K_t from CPt (K_t = drn_t + 8 - (6 - Sr_t)) and S = 6 - Sr of the preceding data mode. Otherwise D_trn = drn_t + 8. | 24 (4-point) or 36 (8-point), per Jp bit 49 |
| FPE and data mode | D = drn_u + 20 (21..42) of the preceding CPu | K = 2 x (drn_d + 17) (36..72) of the preceding CPd |

Resulting minimum lengths:

- **SUVd:** ceil(52/D) frames.
- **SUVu:** ceil(52/bits) frames.
- **CPus:** same as SUVu.
- **CPd:** ceil((324 + parts)/D) frames, where "parts" is the length change from omitted or variable parts.
- **CPu:** ceil((290 + delta)/bits) frames.

### 2.18 Transition signatures for the detectors (informative)

A responder acts on the **X to X-bar transition** (9.8.1.2.1, 9.8.2.2.1, 9.9.1.2.1, 9.9.2.2.1). X-bar lasts only 24T and is followed by an unrelated signal, so the detector has at most 24 symbols to confirm it.

| Transition | What it looks like | Length of X-bar |
|---|---|---|
| Rd to Rd-bar (and Rt to Rt-bar) | ... `+ + + - - -` then `- - - + + +`: **six consecutive "-"** across the boundary, a half-period slip of the 6-symbol pattern | 4 periods |
| Ru to Ru-bar | same shape with +/-LU | 4 periods (2 upstream frames) |
| Rf to Rf-bar | ... `+ + - -` then `- - + +`: **four consecutive "-"**, a 180 degree reversal of the 2000 Hz component | 6 periods of the 4-symbol sign cycle |
| RM to RM' | K domain: ... `M-1, M-1, 0, 0` then `0, 0, M-1, M-1`: **four consecutive K = 0** (2.5) | 6 periods (2 upstream frames) |

- Rd and Rf are not differentially encoded and the channel polarity is unknown, so detect the **slip or reversal**, not an absolute sign.
- Rf's sign cycle (4) and the frame (6) are not commensurate. Relative to the frame boundary where Rf started, the pattern phase is 0 in even frames and 2 in odd frames. Rf and Rf-bar are therefore indistinguishable without history; only the reversal identifies the transition.
- The text gives **no** detection time limit and no tolerance. The responder starts its own R on its next own frame boundary after detection. The figures show the response starting while the initiator's X-bar is still running.

---

## 3. Procedure 9.8: rate renegotiation, step by step (pages 62-64)

### 3.1 Digital modem initiates (9.8.1.1)

- **RR-DI-1 (9.8.1.1.1) [SHALL]:**
  - Turn circuit 106 OFF.
  - Condition the receiver to detect **Ru, Ru-bar and SUVu**.
  - Transmit **Rd for 384T**, then **Rd-bar for 24T**.
  - Rd begins on a downstream data frame boundary.
- **RR-DI-2 (9.8.1.1.2) [SHALL]:**
  - Then transmit **TRN2d for up to 16008T**, followed by **SUVd** sequences.
    - Bit 32 of these SUVd = 1 if the digital modem wants a silent period [MAY].
    - Bit 33 = 0.
  - On **receiving an SUVu**, continue per **9.6.1.1.2** (section 5.1): send a single CPd, and so on.
  - **Exception:** if bit 32 is set in **either** the SUVd being sent **or** the SUVu received, go to RR-DI-3.
  - Figure 18 shows that the SUVu may arrive **during TRN2d**. The digital modem then sends SUVd' straight after TRN2d, with no plain SUVd at all.
- **RR-DI-3 (9.8.1.1.3) [SHALL], silence branch:**
  - Transmit SUVd sequences **with bit 33 set** (SUVd').
  - After receiving **an SUVu with bit 33 set, or E2u**, finish the current SUVd, transmit **Ed**, then **silence**.
  - [SHALL] The silence is Ucode-0 magnitudes, with data frame alignment kept (2.13).
- **RR-DI-4 (9.8.1.1.4) [SHALL], if bit 32 of the received SUVu was set:**
  - Wait for an **SUVu with bit 32 clear**.
  - Then transmit **Rt for 384T**, **Rt-bar for 24T**, and **SUVd**.
  - Continue per **9.6.1.1.2**.
- **RR-DI-5 (9.8.1.1.5), if bit 32 of the received SUVu was clear** (only the digital modem asked for silence):
  - **[MAY]** either transmit Rt 384T, Rt-bar 24T and then SUVd sequences on its own initiative (Figure 17);
  - **[MAY]** or wait for another SUVu first (Figure 16).
  - **[SHALL]** Then continue per 9.6.1.1.2.

### 3.2 Digital modem responds (9.8.1.2)

- **RR-DR-1 (9.8.1.2.1) [SHALL]:** after detecting **Ru**:
  - clamp circuit 104 to binary one;
  - condition the receiver to detect the **Ru-to-Ru-bar transition**.
- **RR-DR-2 (9.8.1.2.2) [SHALL]:** after detecting that transition:
  - transmit **Rd for 384T**, then **Rd-bar for 24T**;
  - Rd begins on a data frame boundary.
- **RR-DR-3 (9.8.1.2.3) [SHALL]:** continue from **9.8.1.1.2** (RR-DI-2 onward): TRN2d up to 16008T, SUVd, the bit-32 test, and so on.
- The responder is not told to turn 106 OFF; the initiator is not told to clamp 104 (Q-9).

### 3.3 Analogue modem initiates (9.8.2.1)

- **RR-AI-1 (9.8.2.1.1) [SHALL]:**
  - Turn circuit 106 OFF.
  - Transmit **Ru for 384T**, then **Ru-bar for 24T**.
  - Ru begins on an upstream data frame boundary.
  - Unlike RR-DI-1, no receiver conditioning is named here: the analogue initiator is not told to watch for Rd.
- **RR-AI-2 (9.8.2.1.2):**
  - **[SHALL]** Condition the receiver to receive an **SUVd**.
  - **[SHALL]** Transmit **TRN2u for up to 16008T**.
  - **[MAY]** End TRN2u **once it has run 2400T**, or **as soon as an SUVd is received**.
- **RR-AI-3 (9.8.2.1.3) [SHALL]:**
  - Then transmit **SUVu** sequences.
    - Bit 32 = 1 if the analogue modem wants silence [MAY].
    - Bit 33 = 0.
    - Bits 26 and 27:31 as desired.
  - After **having transmitted an SUVu and received an SUVd**, continue per **9.6.2.1.2**.
  - **Exception:** if bit 32 is set in **either** the SUVu or the SUVd, go to RR-AI-4.
- **RR-AI-4 (9.8.2.1.4) [SHALL], silence branch:**
  - Transmit SUVu sequences **with bit 33 set** (SUVu').
  - After receiving **an SUVd with bit 33 set, or Ed**, finish the current SUVu, then transmit **E2u** followed by **TRN2u** (scrambler reset, 2.7).
  - Figure 18: an SUVd' may already have arrived before the first SUVu' goes out. The figure still shows one complete SUVu' before E2u (Q-21).
- **RR-AI-5 (9.8.2.1.5) [SHALL], if bit 32 of the received SUVd was clear** (only the analogue modem asked for silence):
  - Transmit TRN2u **for up to 8004T**, then **SUVu with bit 32 clear**.
  - Continue per **9.6.2.1.2**.
- **RR-AI-6 (9.8.2.1.6) [SHALL], if bit 32 of the received SUVd was set:**
  - Condition the receiver to receive **Rt**.
  - On **receiving Rt**, or **after transmitting 8004T of TRN2u**, transmit SUVu sequences **with bit 32 clear** and wait for an **SUVd**.
  - Continue per **9.6.2.1.2**.

### 3.4 Analogue modem responds (9.8.2.2, page 64)

- **RR-AR-1 (9.8.2.2.1) [SHALL]:** after receiving **Rd**:
  - clamp circuit 104 to binary one;
  - condition the receiver to detect the **Rd-to-Rd-bar transition**.
- **RR-AR-2 (9.8.2.2.2):** after receiving that transition:
  - transmit **Ru for 384T** and **Ru-bar for 24T**. The sentence lacks "shall" (E-7); treat it as mandatory.
  - **[SHALL]** Ru begins on a data frame boundary.
- **RR-AR-3 (9.8.2.2.3):**
  - **[SHALL]** Condition the receiver to receive an SUVd.
  - **[SHALL]** Transmit **TRN2u for up to 16008T**.
  - **[MAY]** End TRN2u after 2400T or when an SUVd is received.
  - **[SHALL]** Then continue from **9.8.2.1.3** (RR-AI-3 onward).

### 3.5 The silent-period sub-procedure as a decision table

B32d is bit 32 of the SUVd the digital modem sends. B32u is bit 32 of the SUVu the analogue modem sends.

| B32d | B32u | Digital modem | Analogue modem | Figure |
|---|---|---|---|---|
| 0 | 0 | No silence. On SUVu: single CPd (9.6.1.1.2). | No silence. After its own SUVu plus a received SUVd: single CPu (9.6.2.1.2). | 15 |
| 1 | 0 | SUVd' until SUVu' or E2u, then Ed and silence. Then (RR-DI-5): Rt on its own initiative, or after another SUVu. Then Rt-bar, SUVd, 9.6.1.1.2. | SUVu' until SUVd' or Ed, then E2u and TRN2u. Listen for Rt. On Rt, or at 8004T, send SUVu (B32 = 0) and wait for SUVd (RR-AI-6). | 16 (full length; Rt after SUVu), 17 (Rt ends it early) |
| 0 | 1 | SUVd' until SUVu' or E2u, then Ed and silence. Wait for SUVu with B32 = 0, then Rt, Rt-bar, SUVd (RR-DI-4). | SUVu' until SUVd' or Ed, then E2u and TRN2u for up to 8004T (own choice), then SUVu with B32 = 0 (RR-AI-5). | 18 |
| 1 | 1 | RR-DI-4 applies (the received SUVu has bit 32 set): wait for SUVu with B32 = 0. | RR-AI-6 applies (the received SUVd has bit 32 set): TRN2u until Rt or 8004T. The digital modem will not send Rt first, so this is 8004T in practice. | none (Q-15) |

**Notes:**

- **[INFERRED]** Inside the silence branch, **bit 33 is the handshake that agrees the silence**. No CP has been exchanged at that point, so it cannot mean "CP received".
  - After the silence, both modems send plain SUV again (all figures show SUVd and SUVu without prime after Rt / after the second TRN2u). Bit 33 then regains its 9.6 meaning.
  - Reset the "CP received" state when leaving the silence.
- **[INFERRED]** The post-silence SUVu has bit 32 = 0 (RR-AI-5 and RR-AI-6 say so). The post-Rt SUVd should also have bit 32 = 0 (Q-10).
- **[INFERRED]** During the silence the analogue modem sends TRN2u against a silent far end:
  - its echo canceller retrains on its own echo;
  - the digital modem sees the upstream channel without its own echo-free-but-present downstream signal, which helps redesign the precoder and prefilter.
- **[INFERRED]** "Wait for another SUVu" (RR-DI-5) and "SUVu with bit 32 clear" (RR-DI-4) refer to the SUVu the analogue modem sends **after** its post-silence TRN2u. Any SUVu' still arriving from before the silence must not trigger Rt.
  - Those carry bit 33 = 1 and precede the E2u, so ignore every SUVu until the E2u, or until at least one TRN2u frame has been seen.

### 3.6 Figures 15-18 as sequence listings (read from the rendered figures)

Arrow notation: "A -> B" means the event at A (usually the end of a sequence) is received by the far modem at point B.

**Figure 15 (page 61): RR with no silence, initiated by the digital modem.**

```
Digital : DATA | Rd 384T | Rd-bar 24T | TRN2d <=16008T | SUVd | SUVd | CPd | SUVd' | SUVd' | Ed | B1d | DATA
Analogue: DATA          | Ru 384T | Ru-bar 24T | TRN2u >=2400T | SUVu | SUVu | CPu | SUVu | SUVu' | SUVu' | E2u | B1u | DATA
```

- Rd to Rd-bar transition -> the analogue modem starts Ru; Ru begins while Rd-bar is still running.
- The first SUVd and first SUVu cross. Each modem hears the other's first SUV during its own second SUV, then sends its CP.
- End of CPu -> heard by the digital modem during its CPd -> every later SUVd is SUVd'.
- End of CPd -> heard by the analogue modem at the end of its first post-CPu SUVu -> every later SUVu is SUVu'.
- The first SUVd' and first SUVu' cross. Each modem hears the other's during its own second primed SUV, finishes it, and sends Ed or E2u.
- TRN2d and TRN2u end at about the same time in the drawing.

**Figure 16 (page 61): RR, silence requested by the digital modem and held for the maximum length.**

```
Digital : DATA Rd Rd-bar TRN2d(<=16008T) SUVd SUVd SUVd' Ed ----silence---- Rt(384T) Rt-bar(24T) SUVd CPd SUVd' SUVd' Ed B1d DATA
Analogue: DATA Ru Ru-bar TRN2u(>=2400T) SUVu SUVu' SUVu' E2u TRN2u(label >=4008T) SUVu SUVu SUVu SUVu CPu SUVu SUVu' SUVu' E2u B1u DATA
```

- End of the first SUVd (B32 = 1) -> heard by the analogue modem near the end of its first SUVu -> it continues with SUVu'.
- End of the first SUVu -> heard by the digital modem during its second SUVd -> its next sequence is SUVd'.
- End of SUVd' -> heard by the analogue modem during its second SUVu' -> it finishes that, sends E2u, then TRN2u.
- The analogue modem's SUVu' reaching the digital modem is not drawn. The digital modem sends one SUVd', then Ed, then silence.
- End of the first post-TRN2u SUVu (B32 = 0) -> the digital modem starts Rt (the "wait for another SUVu" option of RR-DI-5). The analogue modem ran TRN2u to its cap (RR-AI-6).
- End of the post-Rt SUVd -> heard by the analogue modem at the end of its 4th SUVu -> CPu.
- End of CPu -> heard by the digital modem during CPd -> SUVd'.
- End of CPd -> heard by the analogue modem at the end of the next SUVu -> SUVu'.
- The primed SUVs cross -> Ed and E2u.
- The ">=4008T" label contradicts the 8004T cap in the text (E-3).

**Figure 17 (page 62): RR, silence requested by the digital modem and ended before the maximum.**

```
Digital : DATA Rd Rd-bar TRN2d(<=16008T) SUVd SUVd SUVd' Ed ----silence---- Rt(384T) Rt-bar(24T) SUVd CPd SUVd' SUVd' Ed B1d DATA
Analogue: DATA Ru Ru-bar TRN2u(>=2400T) SUVu SUVu' SUVu' E2u TRN2u(>=2400T) SUVu SUVu SUVu CPu SUVu SUVu' SUVu' E2u B1u DATA
```

- First SUVu and SUVd exchanged as in Figure 16 -> SUVd' and SUVu'.
- End of SUVd' -> heard by the analogue modem at the end of its second SUVu' -> E2u, then TRN2u.
- **Start of Rt** -> heard by the analogue modem -> it ends TRN2u there, after at least 2400T in the drawing, and sends SUVu (B32 = 0). The digital modem used the "send Rt on its own" option of RR-DI-5.
- End of the first post-TRN2u SUVu -> heard by the digital modem during its SUVd -> CPd.
- End of SUVd -> heard by the analogue modem at the end of its second SUVu -> one more SUVu, then CPu.
- End of CPu -> heard near the end of CPd -> SUVd'.
- End of CPd -> SUVu'.
- The primed SUVs cross -> Ed and E2u.

**Figure 18 (page 62): RR, silence requested by the analogue modem (the analogue modem initiates).**

```
Analogue: DATA Ru(384T) Ru-bar(24T) TRN2u(>=2400T) SUVu SUVu SUVu SUVu SUVu' E2u TRN2u(<=8004T) SUVu SUVu SUVu SUVu CPu SUVu SUVu' SUVu' E2u B1u DATA
Digital : DATA Rd(384T) Rd-bar(24T) TRN2d(<=16008T) SUVd' SUVd' SUVd' Ed ----silence---- Rt(384T) Rt-bar(24T) SUVd CPd SUVd' SUVd' Ed B1d DATA
```

- Ru to Ru-bar transition -> the digital modem starts Rd.
- End of the first SUVu (B32 = 1) -> heard by the digital modem **during TRN2d** -> its first SUV after TRN2d is already SUVd'.
- The analogue modem sends four plain SUVu while the long TRN2d is still running and no SUVd has arrived.
- End of the first SUVd' -> heard by the analogue modem during its 4th SUVu -> it sends one SUVu', then E2u, then TRN2u.
- End of SUVu' -> heard by the digital modem during its third SUVd' -> Ed, then silence.
- End of the first post-TRN2u SUVu (B32 = 0), after at most 8004T of TRN2u -> the digital modem starts Rt (RR-DI-4).
- The rest is as in Figures 16 and 17: SUVd -> CPu; CPu -> SUVd'; CPd -> SUVu'; crossing -> Ed and E2u.

---

## 4. Procedure 9.9: fast parameter exchange, step by step (pages 64-65)

### 4.1 Digital modem initiates (9.9.1.1)

- **FPE-DI-1 (9.9.1.1.1) [SHALL]:**
  - Turn circuit 106 OFF.
  - Condition the receiver to detect **RM, RM' and SUVu**.
  - Transmit **Rf for 384T**, then **Rf-bar for 24T**.
  - Rf begins on a data frame boundary.
- **FPE-DI-2 (9.9.1.1.2) [SHALL]:**
  - Then **initialise the scrambler, differential encoder and spectral-shaping filter memory to zero**.
  - Transmit **SUVd sequences with bit 32 clear**, in the preceding data-mode modulation.
  - **After detecting RM and RM'**, condition the receiver to receive an SUVu and continue per **9.6.1.1.2**.
  - **If Ru is detected** instead, continue per **9.8.1.2.1**: become the RR responder, abandoning FPE.

### 4.2 Digital modem responds (9.9.1.2)

- **FPE-DR-1 (9.9.1.2.1) [SHALL]:** after detecting **RM**:
  - clamp circuit 104 to binary one;
  - condition the receiver to detect the **RM-to-RM' transition**.
- **FPE-DR-2 (9.9.1.2.2) [SHALL]:** after detecting that transition:
  - transmit **Rf for 384T**, then **Rf-bar for 24T**;
  - Rf begins on a data frame boundary.
- **FPE-DR-3 (9.9.1.2.3) [SHALL]:**
  - Then initialise the scrambler, differential encoder and spectral-shaping filter memory to zero.
  - Transmit **SUVd with bit 32 clear**.
  - Continue per **9.6.1.1.2**.

### 4.3 Analogue modem initiates (9.9.2.1)

- **FPE-AI-1 (9.9.2.1.1) [SHALL]:**
  - Turn circuit 106 OFF.
  - Condition the receiver to detect **Rf, Rf-bar and SUVd**.
  - Transmit **RM for 384T**, then **RM' for 24T**.
  - RM begins on a data frame boundary.
- **FPE-AI-2 (9.9.2.1.2) [SHALL]:**
  - Then **initialise the scrambler and differential encoder to zero**. The precoder, prefilter and trellis state are **not** reset.
  - Transmit **SUVu with bit 32 clear**, in the preceding data-mode modulation.
  - After detecting **Rf and Rf-bar** (printed "Rf, Rf'", E-6), condition the receiver to receive an SUVd and continue per **9.6.2.1.2**.
  - **If Rd is detected**, continue per **9.8.2.2.1**: become the RR responder.

### 4.4 Analogue modem responds (9.9.2.2)

- **FPE-AR-1 (9.9.2.2.1) [SHALL]:** after detecting **Rf**:
  - clamp circuit 104 to binary one;
  - condition the receiver to detect the **Rf-to-Rf-bar transition**.
- **FPE-AR-2 (9.9.2.2.2) [SHALL]:** after detecting that transition:
  - transmit **RM for 384T**, then **RM' for 24T**;
  - RM begins on a data frame boundary.
- **FPE-AR-3 (9.9.2.2.3) [SHALL]:**
  - Then initialise the scrambler and differential encoder to zero.
  - Transmit **SUVu with bit 32 clear**.
  - Continue per **9.6.2.1.2**.

### 4.5 FPE end game (from 9.6, section 5)

- **Digital modem:** after E2u, the receiver expects **FB1u (48 old-modulation frames) and then B1u** (9.6.1.1.6).
- **Analogue modem:** after E2u, it transmits **FB1u and then B1u** (9.6.2.1.5).
- The digital modem sends **no** FB1d: it goes straight from Ed to B1d.
- Neither side sends TRN2 or silence.
- **[INFERRED]** Bit 32 is always 0 in FPE, so the silence branch can never be entered.

### 4.6 Figure 19 (page 64): FPE initiated by the analogue modem

```
Analogue: DATA | RM 384T | RM' 24T | SUVu | SUVu | SUVu | CPu | SUVu | SUVu' | SUVu' | E2u | FB1u | B1u | DATA
Digital : DATA           | Rf 384T | Rf-bar 24T | SUVd | SUVd | CPd | SUVd' | SUVd' | Ed | B1d | DATA
```

- RM to RM' transition -> the digital modem starts Rf, while RM' is still running.
- The first exchange crosses:
  - the analogue modem's SUVu reaches the digital modem during its second SUVd, and CPd follows;
  - the digital modem's first SUVd reaches the analogue modem during its third SUVu, and CPu follows.
- End of CPu -> heard during CPd -> SUVd'.
- End of CPd -> heard at the end of the analogue modem's post-CPu SUVu -> SUVu'.
- First SUVd' -> heard during the analogue modem's second SUVu' -> **E2u, FB1u, B1u**.
- First SUVu' -> heard during the digital modem's second SUVd' -> **Ed, B1d**.

---

## 5. Phase 4 steps that RR and FPE jump into (9.6, pages 58-59)

Every RR and FPE path ends with "proceed according to 9.6.1.1.2" (digital) or "9.6.2.1.2" (analogue). The following steps then apply unchanged.

### 5.1 Digital modem (9.6.1.1)

- **9.6.1.1.1** (Phase 4 entry only; **not** used by RR or FPE) [SHALL]:
  - Send TRN2d for at least 2040T.
  - When ready to receive CPu, condition the receiver for SUVu and send SUVd.
- **P4-D2 (9.6.1.1.2) [SHALL]:**
  - After **receiving an SUVu**, send **one CPd**, followed by more SUVd.
  - After **receiving a CPu** (CPus included, [INFERRED]), send every later CPd and SUVd with the **acknowledge bit set**.
  - **[MAY]** If the SUVu carries bit 26 = 1, the digital modem may hold CPd back until it has a CPu (Table 27). Figure 13 shows CPd' sent after CPu arrived.
- **P4-D3 (9.6.1.1.3) [SHALL]:**
  - Consider every CPu and SUVu received up to and including the **complete** one that arrives **after 100 ms + one RTD from the end of the digital modem's CPd**.
  - If none of them has the acknowledge bit set, send **repeated CPd** sequences.
  - Figure 14 marks this "RTD + 100 ms" window.
- **P4-D4 (9.6.1.1.4) [SHALL]:**
  - Once the digital modem has **sent** a CPd or SUVd with the ack bit set **and** has **received** a CPu or SUVu with the ack bit set, **or E2u**:
  - finish the current CPd or SUVd, then send **Ed**.
- **P4-D5 (9.6.1.1.5) [SHALL]:**
  - After Ed, send **B1d** at the negotiated rate, using the data-mode constellation parameters received in CPu.
  - Then let circuit 106 follow circuit 105, and start data transmission (clause 5).
- **P4-D6 (9.6.1.1.6) [SHALL]:**
  - After **receiving E2u**, condition the receiver for **B1u**. In **FPE**, condition it for **FB1u followed by B1u**.
  - After receiving B1u, **unclamp 104**, turn **109 ON**, and start demodulating.
- **Recovery (9.6.1.2):**
  - **[MAY]** Retrain at any time in Phase 4 (9.7.1.1).
  - **[SHALL]** On Tone A, respond per 9.7.1.2.
  - **9.6.1.2.1 [SHALL]:** retrain if B1u has not arrived within **20 s + 6 RTD from the end of INFO1a**. This is a start-up timer with no defined meaning in RR or FPE (Q-1).

### 5.2 Analogue modem (9.6.2.1)

- **9.6.2.1.1** (Phase 4 entry only) [SHALL]:
  - Condition the receiver for SUVd and send TRN2u.
  - When ready for CPd **and** (TRN2u has run at least 12000T **or** an SUVd has arrived), send SUVu.
- **P4-A2 (9.6.2.1.2) [SHALL]:**
  - After **receiving an SUVd**, send **one CPu** (long or short), followed by more SUVu.
  - After **receiving a CPd**, send every later CPu and SUVu with the ack bit set.
- **P4-A3 (9.6.2.1.3) [SHALL]:**
  - Consider every CPd and SUVd received up to and including the complete one that arrives after **100 ms + one RTD from the end of the analogue modem's CPu**.
  - If none has the ack bit set, send **repeated CPu**.
- **P4-A4 (9.6.2.1.4) [SHALL]:**
  - Once the analogue modem has **sent** a CPu or SUVu with the ack bit set **and** has **received** a CPd or SUVd with the ack bit set, **or Ed**:
  - finish "the current CPu sequence" (read: the current CPu **or SUVu**, E-8), then send **E2u**.
- **P4-A5 (9.6.2.1.5) [SHALL]:**
  - After E2u, send **B1u**. In **FPE**, send **FB1u then B1u**.
  - Then let 106 follow 105, and start data transmission (6.4).
- **P4-A6 (9.6.2.1.6) [SHALL]:**
  - After **receiving Ed**, condition the receiver for **B1d**.
  - After B1d, **unclamp 104**, turn **109 ON**, and start demodulating.
- **Recovery (9.6.2.2):**
  - **[MAY]** Retrain at any time (9.7.2.1).
  - **[SHALL]** On Tone B, respond per 9.7.2.2.
  - **9.6.2.2.1:** retrain if B1d has not arrived within 20 s + 6 RTD of the end of sending INFO1a. This is a start-up timer (Q-1).

---

## 6. Related procedures

### 6.1 Retrain (9.7, page 60): the escape from a stuck RR or FPE

- **Digital modem initiates (9.7.1.1) [SHALL]:**
  1. Turn 106 OFF, clamp 104, and send silence for **70 +/- 5 ms**.
  2. Send Tone B and detect Tone A.
  3. On Tone A, detect the Tone A phase reversal and run the full Phase 2.
- **Digital modem responds (9.7.1.2) [SHALL]:**
  1. After hearing Tone A for **more than 50 ms**, turn 106 OFF, clamp 104, and send 70 +/- 5 ms of silence.
  2. Send Tone B, detect the Tone A reversal, and run full Phase 2.
- **Analogue modem initiates (9.7.2.1) [SHALL]:**
  1. Turn 106 OFF, clamp 104, and send 70 +/- 5 ms of silence.
  2. Send Tone A and detect Tone B.
  3. Once Tone B has been heard **and** Tone A has run **at least 50 ms**, send the Tone A phase reversal, detect the Tone B reversal, and run full Phase 2.
- **Analogue modem responds (9.7.2.2) [SHALL]:**
  1. After hearing Tone B for more than 50 ms, turn 106 OFF, clamp 104, and send 70 +/- 5 ms of silence.
  2. Send Tone A and run full Phase 2.

### 6.2 Cleardown (9.11, page 69)

- **[SHALL]** A connection is ended with the cleardown procedure.
- **[DEF]** Cleardown is signalled by **drn = 0** in a rate sequence.
  - The text says "in SUVu" and "in SUVd", but neither has a drn field (E-1).
  - The intended fields are CPu/CPus bits 21:25 (analogue modem) and CPd bits 22:26 (digital modem).
- **[MAY]** Cleardown may be signalled whenever a modem sends a rate sequence.
- **[SHALL]** To clear down from data mode, a modem **initiates either an RR or an FPE** in order to send drn = 0.
  - **[INFERRED]** FPE is the cheaper way: 408T (51 ms) of R signalling, then the SUV/CP exchange (a few round trips), with no training and no silence.
- **[INFERRED] Receiving side.** V.92 does not say what to do after receiving drn = 0.
  - V.90 9.7 has a NOTE: the digital modem **should** ignore the constellation fields of a CP with drn = 0.
  - Complete the acknowledged exchange (so the far end knows it was received), then drop the line instead of entering data mode.
  - The disconnect itself belongs to V.250/V.25 handling.

---

## 7. Timers, durations and tolerances

| Item | Value | Kind | Clause |
|---|---|---|---|
| Rd; Rd-bar | 384T (64 x 6); 24T (exactly 4 x 6) | SHALL | 9.8.1.1.1, 9.8.1.2.2; 8.6.4/V.90 |
| Rt; Rt-bar | 384T; 24T | SHALL | 9.8.1.1.4 (SHALL), 9.8.1.1.5 (MAY send) |
| Ru; Ru-bar | 384T (32 frames); 24T (2 frames) | SHALL | 9.8.2.1.1, 9.8.2.2.2 |
| Rf; Rf-bar | 384T (32 x 12); 24T (exactly 2 x 12) | SHALL | 9.9.1.1.1, 9.9.1.2.2; 8.8.4 |
| RM; RM' | 384T (32 frames); 24T (2 frames) | SHALL | 9.9.2.1.1, 9.9.2.2.2 |
| TRN2d in RR | at most 16008T (2.001 s); whole 6T frames; **no minimum stated** | SHALL (upper bound) | 9.8.1.1.2; 8.6.5/V.90 |
| First TRN2u in RR | at most 16008T. MAY end once it has run 2400T, or when an SUVd is received. Whole 12T frames. | SHALL / MAY | 9.8.2.1.2, 9.8.2.2.3 |
| Post-silence TRN2u, analogue modem asked | at most 8004T (1.0005 s), then SUVu with B32 = 0 | SHALL | 9.8.2.1.5 |
| Post-silence TRN2u, digital modem asked | until Rt is received, or 8004T sent | SHALL | 9.8.2.1.6 |
| Figure labels on that TRN2u | Fig. 16 ">=4008T", Fig. 17 ">=2400T", Fig. 18 "<=8004T" | informative (E-3) | Figures 16-18 |
| Silence (digital) | open-ended; Ucode 0; frame alignment kept; ended by Rt | SHALL | 9.8.1.1.3 |
| CP repeat window | repeat CP if no ack seen up to and including the first complete sequence received after 100 ms + RTD from the end of own CP | SHALL | 9.6.1.1.3, 9.6.2.1.3 |
| Ed | 2 downstream frames = 12T | DEF | 8.8.2 |
| E2u | 1 upstream frame = 12T (CPd bit 29 SHALL be 0 in RR and FPE) | DEF / SHALL | 8.7.2; Table 30 |
| B1d | 48 frames = 288T (36 ms) | DEF | 8.6.1/V.90 |
| B1u | 48 frames = 576T (72 ms) | DEF | 8.7.1 |
| FB1u (FPE only) | 48 frames = 576T | DEF | 8.7.7 |
| Retrain silence | 70 +/- 5 ms | SHALL | 9.7 |
| Retrain tone qualification | more than 50 ms (responder); at least 50 ms of Tone A before the reversal (analogue initiator) | SHALL | 9.7 |
| Overall RR or FPE timeout | **none specified**. V.90 had 5000 ms + 2 RTD from the R or S transition to E or Ed. | none | V.90 9.6.1, 9.6.2 |
| Detection time or tolerance for Rd, Rt, Rf, Ru, RM | **none specified** | none | n/a |
| Response delay after the X-to-X-bar transition | **none specified**; only "on a data frame boundary" | none | 9.8.x.2.2, 9.9.x.2.2 |

---

## 8. Cross-references the implementer must follow

| Reference | Used by | What the referenced text requires (read from the rendered page) |
|---|---|---|
| 8.6.4/V.90 (V.90 p.35) | Rd, Rt (8.8.4) | R = 6-symbol block with signs `+++---`, R-bar = exactly 4 blocks of `---+++`, leftmost first. Neither is differentially encoded, so the receiver must detect them regardless of polarity. Rd uses the highest-power data-mode codeword per interval (from CP, i.e. CPu); Rt the highest-power training codeword per interval (from CPt); Ri uses UINFO throughout. |
| 8.6.5/V.90 (V.90 p.36) | TRN2d (8.8.6) | Scrambled ones through the 5.4/V.90 encoder, CPt constellation. Scrambler, differential encoder and shaping memory reset to zero first. First frame's magnitudes = mapping of the first D scrambled ones, identical for all ld. K and S per Table 17 (K 6..24, S 3..6, 12..40 kbit/s). Length a multiple of 6 symbols. |
| 8.6 preamble /V.90 (V.90 p.33) | TRN2d, Ed in RR (not imported by V.92 in so many words) | Phase 4 signals may be shaped. Initial train or retrain: TRN2d, MP and Ed use CPt's shaping. **RR: the preceding data mode's shaping with the K derived from CPt.** B1d onward: the new CP's shaping. See Q-18. |
| 8.6.1/V.90 (V.90 p.34) | B1d (8.8.1) | 48 frames of scrambled ones, new data-mode parameters. Scrambler, differential encoder and shaping memory reset first. First-frame magnitudes rule as above. K and S per Table 2/V.90. |
| 5.3, 5.4/V.90 | all digital modem sequences | GPC scrambler. Encoder: D = K + S bits per 6-symbol frame, S + Sr = 6. Modulus encoder, mapper, shaping, then sign assignment (sign bit 1 = positive, 0 = negative; 5.4.6). |
| Table 1/V.90 (V.90 p.11), 3.6/V.90 | Rd, Rt, Rf, silence | Ucode to μ-law and A-law octets and linear values. The octet MSB is the G.711 polarity bit. Ucode 0 = μ-law FF (linear 0), A-law D5 (linear 8). |
| 9.6/V.90 (V.90 pp.45-47) | comparison only | V.90 RR: S/S-bar/SCR upstream, MP downstream, CPs bit 30 silence (analogue modem only), 5000 ms + 2 RTD timeouts. Replaced in V.92 for PCM upstream. |
| 9.7/V.90 NOTE (V.90 p.47) | cleardown | The digital modem should ignore the constellation fields of a CP with drn = 0. |
| clause 7/V.34 (V.34 p.16) | all scramblers | GPC 1 + x^-18 + x^-23, GPA 1 + x^-5 + x^-23; self-synchronising; the transmitter divides by the polynomial. |
| 10.1.2.3.2/V.34, Figure 14 (V.34 p.33) | SUVd, SUVu, CPd, CPu, CPus | CRC x^16 + x^12 + x^5 + 1 over all bits except sync, start and fill; register preset to all ones; output from register bit 0 (LSB) first (2.14). |
| 9.6.3.1-9.6.3.2/V.34 (via 6.4.3-6.4.4) | RM, FPE-mode upstream sequences, FB1u | Inverse map (odd coordinates 2y + 1) and 16/32/64-state convolutional encoders, with 2T delays replaced by 4T. |
| 6.4.1-6.4.2/V.92 (pp.11-12) | RM, FPE upstream | Modulus encoder: R = sum b_i 2^i; s = [R > (M-1)/2]; d(f) = s(f) XOR d(f-1); R0 = R or M-1-R depending on d(f-1); K_i = R_i mod M_i. Precoder and prefilter equations, and the equivalence classes of 2.5. |
| Tables 18, 21, 22/V.92 | CPd limits, ld, TRN2u size | INFO1a 12:13 (sections), 14:15 (Ltot), 16:17 (Lmax). Jd 49:50 (max look-ahead 1..3); Jd 18:40 (downstream rates enabled, 28 000..56 000). Table 20 (DIL descriptor) upstream mask (24 000..48 000). Jp 48/49 (4 or 8 points in training / RR). **[INFERRED]** drn values chosen in RR or FPE should lie within the enabled masks. |
| V.8 / V.8 bis | none | 9.8 and 9.9 make no V.8 reference. |

---

## 9. Suggested state machines (informative, derived from sections 3-5)

### 9.1 Digital modem

```
DATA:
  local RR request      -> 106 OFF; clamp 104 (Q-9); rx {Ru, Ru-bar, SUVu, Tone A}
                           tx Rd x64 blocks, Rd-bar x4 (start on frame boundary)      -> D_TRN2D
  heard Ru              -> clamp 104; wait Ru->Ru-bar; tx Rd, Rd-bar (next frame bdy)  -> D_TRN2D
  local FPE request     -> 106 OFF; clamp 104; rx {RM, RM', Ru, SUVu}
                           tx Rf x32, Rf-bar x2; reset scr/diff/shaper; SUVd(b32=0, data mode) -> D_FPE_WAIT_RM
  heard RM              -> clamp 104; wait RM->RM'; tx Rf, Rf-bar; reset; SUVd(b32=0) -> P4 (fpe=true)
D_FPE_WAIT_RM:
  heard RM+RM'          -> rx SUVu                                                     -> P4 (fpe=true)
  heard Ru              -> RR responder (wait Ru->Ru-bar, ...)                         -> D_TRN2D
D_TRN2D: reset encoder; TRN2d (CPt constellation) up to 16008T, whole frames
         (an SUVu may arrive already: latch its b32)                                    -> D_SUV
D_SUV:   tx SUVd(b32 = want_silence, b33 = 0)
         on SUVu: if want_silence or suvu.b32 -> D_SIL_HS else -> P4 (at 9.6.1.1.2)
D_SIL_HS: tx SUVd'(b33 = 1) (at least one complete)
         on SUVu' or E2u: finish current SUVd, tx Ed                                    -> D_SILENT
D_SILENT: tx Ucode-0 frames; ignore pre-E2u SUVu'
         if suvu.b32 was 1: on post-TRN2u SUVu with b32 = 0 -> D_RT
         else: own choice any time, or on the next post-TRN2u SUVu                       -> D_RT
D_RT:    tx Rt x64 (CPt peaks), Rt-bar x4; tx SUVd(b32 = 0, b33 = 0); clear ack state   -> P4
P4 (9.6.1.1.2..6):
         on SUVu: tx one CPd (b33 = got_cpu); then SUVd (b33 = got_cpu)
         on CPu/CPus: got_cpu = true
         CP-repeat rule (100 ms + RTD); on (sent ack) and (rx ack or E2u): finish, tx Ed, B1d (reset), data;
         106 follows 105
         rx: after E2u -> [fpe: FB1u 48 frames (old params)] -> B1u (new params); unclamp 104; 109 ON
ANY:     Tone A for more than 50 ms -> 9.7.1.2; stuck -> 9.7.1.1 (timer per Q-1)
```

### 9.2 Analogue modem

```
DATA:
  local RR request      -> 106 OFF; clamp 104 (Q-9); tx Ru x64 (x32 frames), Ru-bar x4 -> A_TRN2U
  heard Rd              -> clamp 104; wait Rd->Rd-bar; tx Ru, Ru-bar (next frame bdy)  -> A_TRN2U
  local FPE request     -> 106 OFF; clamp 104; rx {Rf, Rf-bar, Rd, SUVd}
                           tx RM x32 frames, RM' x2 (data-mode chain, trellis);
                           reset scr + diff; SUVu(b32 = 0, data mode)                   -> A_FPE_WAIT_RF
  heard Rf              -> clamp 104; wait Rf->Rf-bar; tx RM, RM'; reset; SUVu(b32 = 0) -> P4 (fpe=true)
A_FPE_WAIT_RF:
  heard Rf+Rf-bar       -> rx SUVd                                                      -> P4 (fpe=true)
  heard Rd              -> RR responder                                                 -> (wait Rd->Rd-bar) ...
A_TRN2U: rx SUVd; TRN2u (Jp bit 49 size, GPA reset) up to 16008T;
         MAY stop once 2400T sent, or when an SUVd arrives                              -> A_SUV
A_SUV:   tx SUVu(b32 = want_silence, b33 = 0, b26, level)
         when (>= 1 SUVu sent) and (SUVd received):
           if want_silence or suvd.b32 -> A_SIL_HS else -> P4 (at 9.6.2.1.2)
A_SIL_HS: tx SUVu'(b33 = 1) (at least one complete)
         on SUVd' or Ed: finish current SUVu; tx E2u; TRN2u (GPA reset)                   -> A_TRN2U_2
A_TRN2U_2:
         if suvd.b32 == 0: TRN2u up to 8004T (own choice)       -> tx SUVu(b32 = 0); clear ack state -> P4
         if suvd.b32 == 1: rx Rt; on Rt or at 8004T              -> tx SUVu(b32 = 0); wait SUVd -> P4
P4 (9.6.2.1.2..6):
         on SUVd: tx one CPu or CPus (b33 = got_cpd); then SUVu (b33 = got_cpd)
         on CPd: got_cpd = true
         CP-repeat rule; on (sent ack) and (rx ack or Ed): finish, tx E2u (12 symbols);
         [fpe: FB1u 48 frames old params]; B1u (all memories reset, interval 0); data; 106 follows 105
         rx: after Ed -> B1d; unclamp 104; 109 ON
ANY:     Tone B for more than 50 ms -> 9.7.2.2; stuck -> 9.7.2.1 (timer per Q-1)
```

---

## 10. Implementation notes

### 10.1 What is new compared with V.90's rate renegotiation (9.6/V.90)

- **Upstream signalling is PCM-style.**
  - V.90 used V.34 signals upstream: S (128T), S-bar (16T), optional SCR up to 2000 ms, CP/CPs and V.34's E. The digital modem answered with MP/MP'.
  - V.92 uses Ru and Ru-bar (384T and 24T), TRN2u (4/8-point), SUVu and CPu/CPus, and **E2u** (one frame of zeros). The digital modem answers with **SUVd and CPd**.
- **The digital modem also sends TRN2d up to 16008T.** V.90 had "optionally no more than 2000 ms".
- **Both modems can ask for silence (bit 32).**
  - In V.90 only the analogue modem could (CPs bit 30).
  - The silent period is agreed with bit 33 (SUV') and always ends with **Rt, Rt-bar, SUVd**. In V.90 it ended with Rt, Rt-bar, MP.
  - V.92 names two purposes: retraining the echo canceller, and retraining the precoder and prefilter.
- **Frame synchronisation is kept in both directions**, and both directions are 8000 symbols/s. In V.90 this was required only downstream.
- **The explicit RR timeouts are gone.** V.90 had E within 5000 ms + 2 RTD, and Ed within 5000 ms + 2 RTD. V.92 relies on the Phase 4 CP-repeat rule and the general retrain options.
- **FPE is new:**
  - Rf and RM initiators;
  - no training and no silence;
  - old data-mode modulation with scrambler and differential resets;
  - FB1u before B1u;
  - RR takes precedence over FPE.
- **CPus is new:** a short CPu used when the downstream parameters are unchanged.
- **CPd has optional parts** (bits 19-21), so an RR or FPE can resend just the rate, or just the rate and the constellations.
- **Cleardown** can use RR or FPE. In V.90 it used RR only.

### 10.2 Pitfalls

1. **Detectors in data mode run beside the data receiver.**
   - Digital side: Ru (6-symbol, +/-LU) and RM (K domain, 2.5), plus Tone A.
   - Analogue side: Rd (6-symbol) and Rf (4-symbol sign cycle), both polarity-blind, plus Tone B.
   - Each responder acts on the **transition** (2.18), then starts its own R on its **own** next frame boundary.
   - The initiator's X-bar lasts 3 ms, so the transition detector must be sharp and must not need X-bar to last longer.
2. **RM uses the full data-mode chain:** precoder with feedback, prefilter, trellis-dependent class at k = 3, and G.
   - Precoder, prefilter and convolutional encoder states carry over from data mode.
   - Build RM by overriding the modulus encoder output, not by writing line values.
   - **Do not** detect it as a 2000 Hz tone (2.5). Detect it on the decoded K sequence.
3. **Ru bypasses the precoder and prefilter; RM does not.** Do not share one "R generator" between them.
4. **Rd and Rf use the data-mode constellation** (from the last long CPu), interval by interval. **Rt uses the training constellation** (CPt from Phase 3). Keep both tables alive for the whole connection.
5. **Encoder resets differ by procedure** (2.15).
   - FPE resets the digital modem's scrambler, differential encoder and shaper before SUVd, and the analogue modem's scrambler and differential encoder (not precoder or trellis) before SUVu.
   - RR resets the digital modem's encoder at TRN2d and the analogue modem's scrambler at each TRN2u.
   - B1d and B1u reset everything again.
   - FB1u runs on the old chain with no reset.
   - Receivers must mirror each reset **at the same frame boundary**. Descramblers self-synchronise, but not fast enough to catch a 17-bit frame sync right after a discontinuity.
6. **Sequence lengths depend on the modulation** (2.17).
   - SUVd and CPd pad to whole downstream frames of D bits; SUVu, CPu, CPus and E2u to whole upstream frames.
   - Bits per frame differ between RR (training modulation) and FPE (data mode).
   - Reuse the "pad to frame_bits" approach of V.90's `mp_bits` (`crates/datapump/src/v90/sequences.rs`).
7. **Finding E2u and Ed.**
   - The SUV and CP fill is zeros, and E2u and Ed are zeros too.
   - Sequences are repeated back to back on frame boundaries.
   - So at each frame boundary after a sequence, a new frame starting with ones (frame sync) means another sequence; a frame of zeros means E.
   - For Ed (2 frames) the receiver can confirm across two frames. For E2u there is only one frame; in FPE, B1u follows exactly 48 frames later regardless.
8. **Bit 33 has two jobs** (3.5): the silence handshake inside the RR silence branch, and CP acknowledgement everywhere else. Clear the ack state when the silence ends. Ignore stale SUVu' arriving before E2u.
9. **Round-trip delay on the test line.**
   - Rory's VoIP path measures about **1.5-1.6 s round trip** (about 750 ms each way; project memory).
   - The 100 ms + RTD CP-repeat rule needs a sane RTD. Use the Phase 2 estimate (RTDEd on the digital side, and the analogue modem's own estimate).
   - "Stop TRN2u after 2400T or when SUVd arrives" is legal but can starve the far equaliser on a long path. Prefer running TRN2u until the SUVd actually arrives.
   - The 8004T (1.0 s) silence cap is shorter than one RTD on this line.
     - An Rt the digital modem sends on its own (Figure 17) can reach the analogue modem after the analogue modem has already ended the silence on its 8004T timer.
     - Both paths converge (SUVu with B32 = 0, then SUVd), so each side must accept the other's post-silence sequence arriving before, during or after its own.
   - In particular the digital modem must accept a B32 = 0 SUVu that arrives while it is still sending Rt, and must not send Rt twice.
10. **Jitter slips.**
    - VoIP concealment inserts about 20 ms every few seconds (project memory).
    - A slip during RR or FPE breaks the 6- and 12-symbol alignment that Rd, Rf, RM and the frame-sync search rely on, although the Recommendation assumes alignment is simply kept.
    - Detectors should re-find the pattern phase, and receivers should re-find frame sync, rather than trust a frame counter carried across the procedure.
11. **Collisions.** Test all of these:
    - **RR against RR:**
      - The digital initiator listens for Ru, Ru-bar and SUVu.
      - The analogue initiator listens only for SUVd.
      - Both proceed through TRN2 to SUV and converge.
      - The analogue initiator may see Rd while in TRN2u; it must not treat that as a new RR.
    - **FPE against RR:** the FPE initiator becomes the RR responder when it detects Rd or Ru (9.9.x.1.2).
      - Meanwhile the RR initiator may receive the FPE initiator's data-mode SUV while expecting a training-mode SUV. That SUV fails its CRC, so this is harmless as long as the RR initiator keeps sending TRN2 or SUV until a valid SUV arrives.
    - **FPE against FPE:** both send R and then SUV, and they converge.
12. **Circuit 104.**
    - The clamp starts only when the far R is **detected**, so symbols of R demodulated before detection reach the DTE as garbage.
    - Hold back the last few frames of received data (or accept that V.42 discards them), as the V.90 implementation does.
13. **Nothing in 9.8 or 9.9 bounds how long a modem may sit in the procedure.** Implement Q-1's watchdog, or a lost sequence on a noisy line can hang the call.

### 10.3 Errata and apparent editorial errors (read from the rendered pages)

- **E-1 (9.11, page 69):** says drn = 0 is sent "in SUVu" or "in SUVd". Neither has a drn field. Use CPu/CPus bits 21:25 and CPd bits 22:26. V.90 9.7 says "CP" and "MP", so this is a copy-edit slip.
- **E-2 (Table 31, SUVd bits 19:31):** "set to 0 by the analogue modem and not interpreted by the digital modem" is copied from Table 27. It should read: the digital modem sets them to 0 and the analogue modem ignores them.
- **E-3 (Figure 16):** labels the post-E2u TRN2u ">=4008T". The text (9.8.2.1.5, 9.8.2.1.6) caps it at 8004T, and Figure 18 shows "<=8004T". Follow the text.
- **E-4 (Table 25, row 11):** "u11 = 0" should be K11 = 0. Table 26 row 11 uses a lower-case "k11".
- **E-5 (8.8.4):** "Rf is transmitted by 0 repeating ..." contains a stray "0".
- **E-6 (9.9.2.1.2):** "after detecting Rf, Rf'" should read Rf and Rf-bar.
- **E-7 (9.8.2.2.2):** "the analogue modem transmit Ru" is missing "shall".
- **E-8 (9.6.2.1.4):** "complete sending the current CPu sequence" should say CPu **or SUVu**, as 9.6.1.1.4 does for the digital side.
- **E-9 (Table 23):** "289 + delta" appears twice, for the fill bit and for the start of the fill run. The run starts at 290 + delta.
- **E-10 (8.7.6):** initialises TRN2u's differential encoder from "the preceding E1u". That holds only in Phase 4; in RR the preceding sequence is Ru-bar or E2u (Q-5).
- **E-11 (extracted text of Table 22):** the text file shows Jp bits 48-51 in the wrong rows. The rendered table: 47 is the Jd/Jp identifier, 48 the training size, 49 the RR size, 50 reserved, 51 a start bit.

### 10.4 Open questions and ambiguities (decisions needed)

- **Q-1. Timeouts.** 9.8 and 9.9 give no timeout for a stalled procedure. Proposal:
  - Initiator: retrain (9.7.x.1) if B1 has not been received within 5 s + 2 RTD of sending the X-to-X-bar transition (V.90's value). For RR, add the silence time (at most 8004T + 1 RTD).
  - Responder: the same, counted from the transition it detected.
  - Needs a decision.
- **Q-2. Tone A or Tone B during RR or FPE.** 9.8 and 9.9 are silent. 9.6.x.2 ("during Phase 4") arguably applies once the modems "proceed according to 9.6.x.1.2". Proposal: treat Tone A or Tone B as a retrain response at every point, as V.90 9.6 said explicitly.
- **Q-3. Minimum TRN2d in RR.** "Up to 16008T" allows zero. The analogue modem needs some TRN2d to re-converge. Proposal: at least 2040T (the Phase 4 minimum) unless only the rate changes.
- **Q-4. Minimum post-silence TRN2u.** Figure 17 shows ">=2400T"; the text gives none. Proposal: when the analogue modem controls the length (RR-AI-5), send at least 2400T and at most 8004T.
- **Q-5. TRN2u differential seed in RR.** Proposal: "the last transmitted sign bit of the preceding sequence", i.e. the last Ru-bar symbol (+LU, sign bit 0) for the first TRN2u and the last E2u symbol for the post-silence TRN2u. A sign-differential receiver does not care about absolute polarity. Same issue as the Phase 4 digest's A4.
- **Q-6. Bit order in TRN2u-modulated sequences** (SUVu, CPu, CPus and E2u in RR, and TRN2u itself).
  - Tables 28 and 29 list MSB:LSB, but the time order of the 2 or 3 scrambled bits in a symbol is not stated.
  - Also unstated: whether only the MSB (sign) is differentially encoded.
  - This is the same issue as the Phase 4 digest's A2. Decide once, preferably from a real V.92 capture.
- **Q-7. Data-mode modulation of information sequences in FPE.**
  - Likely reading, upstream: sequence bits are the modulus encoder's input b0..bK-1 after GPA scrambling, padded to a multiple of K. E2u is K zeros. "Differentially encoded" is 6.4.1's d(f).
  - Likely reading, downstream: the bits are the D data bits of a V.90 data frame (K modulus bits then S sign bits, 5.4.2/V.90).
  - Consistent with the Phase 4 digests; confirm.
- **Q-8. Precoder and prefilter for TRN2u-modulated sequences.** The text is explicit only for Ru (bypass) and RM (use). Proposal: TRN2u, SUVu, CPu, CPus and E2u in RR bypass them like TRN1u, since the digital modem estimates the raw channel from TRN2u and may be about to send new coefficients.
- **Q-9. Circuits 104 and 106.** Responders are not told to turn 106 OFF, and initiators are not told to clamp 104; V.90 had the same gaps. Both are released in 9.6.x.1.5/6, which implies they were set. Proposal: every participant turns 106 OFF and clamps 104 on entry.
- **Q-10. Bit 32 of the post-Rt SUVd.** Not specified. Proposal: 0. Otherwise the "unless bit 32 is set" test of 9.8.1.1.2 would suggest looping into another silence.
- **Q-11. Encoder state for the post-Rt SUVd.**
  - The differential encoder seed is given by 8.8.5: the last transmitted sign bit of the preceding sequence, which is Rt-bar's final "+".
  - Scrambler state and shaper memory are not given.
  - Proposal: freeze the GPC scrambler (and shaper) at the end of Ed and resume them for SUVd. A self-synchronising descrambler holding the last 23 bits of Ed then decodes SUVd from its first bit.
  - The receiver should in any case survive losing the first SUVd; the protocol recovers because SUVd' follows CPd.
- **Q-12. CPd constellation point "linear value" (16 bits).** Signedness and scale are not stated in Table 30. Settle together with the Phase 4 and intro digests (their Q1, Q-4 and Q5).
- **Q-13. CPus semantics.** CPus is for "digital modem's modulation parameters not changed" but carries drn. Proposal:
  - The analogue modem sends CPus only when the constellations, Sr, ld, the shaping filter and the codec fields are unchanged.
  - drn may change only if the new K still satisfies 5.4/V.90 with the unchanged constellations; otherwise send a long CPu.
  - The digital modem keeps the previous long-CPu parameters on receiving CPus.
- **Q-14. Omitted CPd parts.** When bits 19, 20 or 21 are 0, the analogue modem keeps the values last received for those parts (not stated).
  - In FPE (no new channel estimate) the natural choice is to omit the precoder and prefilter unless they changed.
  - Can the first CPd of a connection omit parts? No: there is nothing to keep.
- **Q-15. Both modems request silence** (B32d = B32u = 1). The text makes the analogue modem run TRN2u to 8004T (RR-AI-6) while the digital modem waits for its SUVu (RR-DI-4), a fixed 1 s. Proposal: accept this; it is legal and bounded.
- **Q-16. Rd and Rf codewords when CPu bit 128 = 1** (transmit constellation differs from the D/A constellation). Proposal: use the **transmit** constellation's highest-power Ucode, since that is what the digital modem puts on the wire.
- **Q-17. Rf polarity.** Not stated. Rf is not differentially encoded, and the V.90 note for R applies by analogy. Implement a polarity-blind Rf detector.
- **Q-18. Spectral shaping and K for TRN2d, SUVd, CPd and Ed in RR.**
  - V.90's 8.6 preamble uses the preceding data-mode shaping with K from CPt; V.92 does not repeat it (2.6).
  - This changes D and therefore the SUVd and CPd frame counts, so the two ends must agree.
  - Proposal: follow V.90's rule (V.92 builds on V.90 and imports its TRN2d), and make it a single constant that is easy to flip if a capture disagrees.
- **Q-19. RM when M_i = 1.** Then M_i - 1 = 0 and the interval carries no RM information; the detector must mask it. What the modulus encoder's d(f) memory does across RM is irrelevant, since FPE resets it after RM', but must be decided for the RR fallback: proposal, reset at B1u as specified.
- **Q-20. Scope.** V.92 does not say that 9.8 and 9.9 apply only to PCM-upstream connections. Proposal:
  - V.34-upstream (V.90 data mode) connections use V.90's 9.6 RR and have no FPE.
  - A V.92 modem in that mode must not send Rf or RM, and should treat them as unknown.
- **Q-21. Minimum number of SUV' in the silence handshake.** The text says "transmit SUVd/SUVu sequences with bit 33 set" and then, on the other side's SUV' or E, finish the current one. Proposal: always send at least one complete SUV', even if the other side's SUV' or E is already in hand, as Figure 18 shows.
- **Q-22. Which SUVu triggers Rt** (RR-DI-4 and RR-DI-5). Proposal: only an SUVu received after the analogue modem's E2u, i.e. after at least one TRN2u frame (3.5).
- **Q-23. Response latency.** No maximum delay between detecting X to X-bar and starting one's own R is given. Proposal: start at the first own frame boundary after detection, and at most 2 frames later.
- **Q-24. "Data mode constellation" after a CPus-only exchange.** Proposal: the constellation of the last long CPu remains in force for Rd and Rf (Q-13).

### 10.5 Reuse in this repository (read, not modified)

- `crates/datapump/src/v34/info.rs::crc`: the 10.1.2.3.2/V.34 CRC (2.14). Use it for SUVd, SUVu, CPd, CPu and CPus.
- `crates/datapump/src/v90/digital.rs`:
  - V.90 R and R-bar generation (Ri, Rd; `RD_SYMBOLS = 384`, `R_BAR_FRAMES = 4`) and "highest-power codeword per interval";
  - TRN2d, silence, and the V.90 RR state.
  - Extend it with Rt (CPt peaks) and Rf (12-symbol `++--` with 2-block Rf-bar), and replace MP with SUVd and CPd.
- `crates/datapump/src/v90/sequences.rs`: V.90 CP and MP builders and finders, including `mp_bits` (fill to whole frames of D bits). SUVd and CPd builders, and the SUVu, CPu and CPus finders, should follow the same pattern.
- `crates/datapump/src/v90/modulus.rs`, `sign.rs`, `ucode.rs`: the downstream encoder pieces needed for TRN2d, SUVd, CPd and Ed in both training and data modulation, and the Ucode table (silence, R codewords).
- `crates/datapump/src/v34/trellis.rs`: V.34 convolutional encoders. V.92 needs the 4T-delay variant (6.4.4) for RM and the upstream data mode.
- Not present yet: the V.92 upstream PCM transmitter (6.4 modulus encoder, precoder, prefilter, inverse map) and the digital modem's matching receiver (K recovery). RM, FB1u, FPE-mode SUVu/CPu and RM detection all depend on them.
