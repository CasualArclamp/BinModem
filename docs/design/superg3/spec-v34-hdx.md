# V.34 half-duplex fax ("Super G3", 33.6k): what the Recommendations say

Digest for BinModem. Sources: ITU-T V.34 (02/98), T.30 (09/2005), V.8 (11/2000), T.4 (07/2003).
Everything here is either a transcription (with the PDF page it was read from) or a paraphrase
with a clause reference. Anything I computed rather than read is marked **(derived)**.
Prose is paraphrased, not quoted. Check the cited clause when the exact wording matters.

---

## 0. Sources, page offsets and how the pages were read

| Document | File (F:\dialupmodem2\docs\specs\) | PDF page = printed page + | PDF pages used |
|---|---|---|---|
| V.34 (02/98) | T-REC-V.34-199802-I.pdf (79 pp) | **+6** (PDF 7 = printed 1) | 10-12 (Tables 1-4), 13-16 (cl. 6, 7, 8, Fig 3), 17-18 (Tables 7-9), 20 (Fig 5), 26-28 (cl. 9.5-9.6), 31-41 (cl. 10.1), 42-46 (cl. 10.2), 46-49 (cl. 11.1, 11.2), 59-70 (cl. 12, Figs 23-28), 72-78 (Annex A) |
| T.30 (09/2005) | T-REC-T.30-200509-I.pdf (322 pp) | **+10** (PDF 90 = printed 80) | 16, 22-24 (3.1, Figs 6a/6b, 4.1), 55 (5.3.2), 62-79 (Table 2 and notes), 88-92 (5.3.7, 5.4, cl. 6, Table 4, Fig 11), 93-100 (Annex A), 118 (Annex C title), 174-176 (Annex F text), 177-192 (Figs F.5-1 to F.5-14), 317-318 (App. VII) |
| V.8 (11/2000) | T-REC-V.8-200011-I.pdf (19 pp) | **+5** (PDF 6 = printed 1) | 8-11 (cl. 3-6, Tables 1-4), 14-15 (cl. 7), 16-18 (cl. 8, Fig 1) |
| T.4 (07/2003) | T-REC-T.4-200307-I.pdf (78 pp) | **+8** (PDF 26 = printed 18) | 26-29 (Annex A, Figs A.1/A.2) |

How the pages were read: the Read tool's PDF mode needs `pdftoppm`, which is not installed on this
machine, so every page cited was rasterised with PyMuPDF 1.28 into
`scratchpad\render\*.png` (110-170 dpi whole pages, 250-800 dpi crops for figures, equations and
tables) and read as an image. The helper is `scratchpad\render.py`; per-page text dumps are in
`scratchpad\pages\*.paged.txt`. The extracted `docs\specs\text\*.txt` was used only to find
things. Section F lists every place where it disagrees with the rendered page.

### Notation used below

- **S̄, S̄h, Ā, B̄** = S-bar, Sh-bar, A-bar, B-bar. The overbars are real (confirmed at 250 dpi) and
  are lost in the extracted text. ASCII names if you need them: Sbar, Shbar, Abar, Bbar.
- **Bit ranges `a:b`** are LSB:MSB, and bit 0 is sent first (V.34 INFO/MP convention).
- **T** = one symbol interval. Clause 12 uses T for two different clocks and never says so
  (see §F-V34 #1). **(derived)** Primary-channel signals (S, S̄, PP, TRN, B1) run at the primary symbol
  rate (2400-3429 Bd). Control-channel signals (PPh, ALT, MPh, E, Sh, S̄h, AC, the 4T of scrambled
  ones) run at 600 Bd, so T = 1/600 s = 1.667 ms there. PPh is defined as 32 control-channel symbols and
  every figure labels it 32T.
- **Roles.**
  - *call / answer* is fixed by who dialled. It decides the control-channel carrier (call 1200 Hz,
    answer 2400 Hz), the scrambler polynomial (call GPC, answer GPA), INFO0c vs INFO0a, and Tone B vs
    Tone A.
  - *source / recipient* is fixed by who sends primary-channel (image) data. V.34 3.14 defines the
    source and 3.11 the recipient. The role decides who sends L1/L2, S/S̄/PP/TRN and B1 (the source),
    and who sends INFOh (the recipient).
  - All four combinations occur. The T.30 V.8 call function picks the direction (D.1), and
    turnaround polling swaps it mid-call (D.3).

---

## 1. The whole call on one page

Summarised from V.34 cl. 12 and T.30 Annex F / Fig F.5-1 (PDF p177), for a call terminal that sends.
Silent gaps are shown as `~x~`.

1. **V.8 (Phase 1, V.34 12.1 = 11.1).**
   - Answer: at least 200 ms of silence, then ANSam.
   - Call: CNG (or CI), then silence Te, then CM repeated (call function "transmit fax", modn0 bit
     b7 = V.34 half-duplex).
   - Answer: JM repeated. Call: CJ.
   - Both then go silent for 75 ± 5 ms.
2. **Phase 2, probing (12.2).**
   - Both: INFO0.
   - Tones B and A with one phase-reversal handshake.
   - The **source only** sends L1 (160 ms) and L2.
   - The **recipient only** sends INFOh: the parameters the source must use (symbol rate, carrier,
     pre-emphasis, power cut, TRN length and constellation).
3. **Phase 3, primary-channel equaliser training (12.3).**
   - Source: `~70±5 ms~` S 128T, S̄ 16T, PP 288T, then TRN (length and constellation from INFOh).
   - The recipient stays silent.
4. **Control-channel start-up (12.4).** 600 Bd full-duplex, FDM on 1200/2400 Hz.
   - Source: `~70±5 ms~` PPh, then ALT.
   - Recipient: answers with PPh, then ALT.
   - Both: MPh ... MPh, then E.
   - Now each side knows the primary data rate and the control-channel rate (1200 or 2400 bit/s).
5. **T.30 phase B over the control channel (T.30 F.3.1.4, F.3.2).**
   - Each side sends at least 2 HDLC flags first.
   - Answer: NSF CSI DIS. Call: TSI DCS. Answer: CFR.
   - There is **no TCF and no FTT**.
6. **"40 ones" turnaround (F.3.2.2-3).**
   - The recipient sends flags until it has seen at least 40 consecutive 1s, then goes silent.
   - The source sends 1s until it hears silence and has sent at least 40, then sends 4T of scrambled
     ones (control-channel turn-off, 12.6.3).
7. **Primary-channel resynchronisation (12.5.1) and message (F.3.2.3).**
   - Source: `~70±5 ms~` S 128T, S̄ 16T, PP, B1.
   - Then T.4 A.3.1 synchronisation flags (nominal 200 ms, tolerance +100 ms).
   - Then ECM FCD frames and 3 × RCP, all on the primary channel.
8. **Primary-channel turn-off (12.5.3.1).** Source: 35 ms of scrambled ones.
9. **Back to the control channel.** Source: `~70±5 ms~`, then one of:
   - Sh 24T, S̄h 8T, ALT, E: resynchronisation, 12.6.
   - PPh ...: start-up, 12.4, which lets the rate be renegotiated.

   The recipient answers in kind. Then come the PPS-xxx / EOR-xxx post-message command and the
   MCF / PPR / RNR / ERR response.
10. Repeat steps 6-9 per partial page and page. The call ends with DCN, optionally followed by 1s
    (Fig F.5-3).

---

## A. V.34 10.2: signals used only in half-duplex (PDF p42-46)

### A.0 What 10.2 inherits (PDF p42)

- **10.2.1, Phase 1.** Every signal is at nominal transmit power and identical to 10.1.1: ANS,
  ANSam, CI, CJ, CM and JM, all defined by reference to V.8.
- **10.2.2, Phase 2.** Every signal except L1 is at nominal power. The signals are identical to
  10.1.2, **except that INFOh replaces INFO1a and INFO1c**. So INFO0, Tones A/B, L1/L2 and
  INFOMARKS come from duplex (section B).
- **10.2.3, Phase 3.** All signals use the selected symbol rate, carrier, pre-emphasis filter and
  power level. NOTE: the transmitter should compensate for non-linear encoding and precoding so that
  the average power is kept in B1 and in data mode.
  - PP is as in 10.1.3.6.
  - S is as in 10.1.3.7.

### A.1 INFOh: Table 22 (read from PDF p42 = printed p36)

INFOh is 51 bits (0..50), sent **only by the recipient**, with the INFO DPSK modulation of B.2.

| INFOh bits (LSB:MSB) | Definition |
|---|---|
| 0:3 | Fill bits: 1111 |
| 4:11 | Frame sync: 01110010, left-most bit first in time (so bit 4 = 0, bit 5 = 1, bit 6 = 1, bit 7 = 1, bit 8 = 0, bit 9 = 0, bit 10 = 1, bit 11 = 0) |
| 12:14 | Power reduction the **recipient's receiver** asks for: integer 0..7 = dB. Must be 0 if the **source's** INFO0 said its transmitter cannot reduce power (INFO0 bit 20 = 0). |
| 15:21 | Length of TRN the **source** sends in Phase 3: integer 0..127, in units of **35 ms** |
| 22 | 1 = use the **high** carrier frequency in data mode. Must agree with the capabilities in the source's INFO0. |
| 23:26 | Pre-emphasis filter index for source-to-recipient transmission: integer 0..10 (Tables 3 and 4, see B.10) |
| 27:29 | Symbol rate for data: integer 0..5, where 0 = 2400 and 5 = 3429. **(derived)** The steps follow Table 1: 0 = 2400, 1 = 2743, 2 = 2800, 3 = 3000, 4 = 3200, 5 = 3429. |
| 30 | 1 = TRN uses the 16-point constellation; 0 = 4-point |
| 31:46 | CRC (the table labels it the code CRC; generator in B.3) |
| 47:50 | Fill bits: 1111 |

- **(derived)** The CRC covers bits 12..30 (19 bits): everything except frame sync and fill (B.3).
- **(derived)** Airtime at 600 bit/s DPSK: 51 bits = 85.0 ms, plus the one reference symbol that
  starts a group (B.2), about 86.7 ms in all.
- **(derived)** TRN in symbols is `n × 35 ms × S`. One 35 ms unit is 84 / 96 / 98 / 105 / 112 / 120
  symbols at 2400 / 2743 / 2800 / 3000 / 3200 / 3429 Bd.
  The maximum is 127 × 35 ms = 4.445 s. Figs 23/24 carry a correction note that removes a "≤ 2000 ms"
  limit on PP+TRN (C.9).

### A.2 Control-channel modulation, 10.2.4 (PDF p43)

- **Rate.** QAM at **600 Bd ± 0.01%**, carrying **1200 or 2400 bit/s**. Control-channel training
  and synchronisation signals go at 1200 bit/s.
  - ALT, E and MPh are stated explicitly at 1200 bit/s.
  - PPh, Sh, S̄h and AC are defined as fixed point sequences on the point-0 diagonal set.
- **Scrambling.** Control-channel data is scrambled with the clause 7 scrambler (B.7). The
  polynomial goes by call/answer role: call GPC, answer GPA.
- **Answer-modem transmitter.** Carrier **2400 Hz ± 0.01%** at **1 dB below** nominal power, plus a
  **1800 Hz ± 0.01% guard tone at 7 dB below** nominal.
- **Call-modem transmitter.** Carrier **1200 Hz ± 0.01%** at **nominal** power, with no guard tone.
- **(derived)** The two directions sit in separate bands (1200 vs 2400 Hz), so the control channel
  is full-duplex by frequency division. The source and recipient transmit it at the same time.
- **Spectrum.** Must lie inside Figure 13, the INFO template (PDF p33). Transcribed from the
  250-dpi render; the mask is symmetric about the carrier and the offsets are from the carrier.

  | offset from carrier (Hz) | upper limit (dB) | lower limit (dB) |
  |---|---|---|
  | 0 to ±125 | +0.75 (flat) | −0.75 (flat) |
  | ±300 | −2 | −4 |
  | ±400 | −5 | −9 |
  | ±450 | −9 | (on the line from −9 @400 to −20 @475) |
  | ±475 | (on the line from −9 @450 to −20 @550) | −20 (lower line ends here) |
  | ±550 | −20 (upper line ends here) | none |

  - Each limit runs straight between its breakpoints (linear Hz, linear dB). The figure has a grey
    band behind the lines; the two thin lines are the limits. The axis runs −600..+600 Hz and
    +5..−25 dB.
  - The Figure 13 NOTE (PDF p33) says linear-phase transmit filters are highly desirable, because
    the receiver has no equaliser training for INFO.
  - **(derived)** A root-raised-cosine at 600 Bd fits the mask for roll-off 0.7-0.8, and 0.75 fits
    comfortably. Checked at 5 Hz steps: 0.5, 0.6, 0.9 and 1.0 each break a limit somewhere.
- **Bits per symbol.** 1200 bit/s sends 2 bits a symbol; 2400 bit/s sends 4. They are labelled
  **I1, I2, Q1, Q2**: I1 first in time, Q2 last. With only 2 bits, **Q1 = Q2 = 0**. Transmission
  is uncoded, meaning no trellis.
- **Mapping.**
  - `2·Q2 + Q1` selects a point of the Figure 5 quarter-superconstellation.
  - That point is rotated **clockwise** by `Zn·90°`, where `Zn = (2·I2n + I1n + Zn−1) mod 4`.
  - If differential encoding is not enabled, `Zn = 2·I2n + I1n`.
  - ALT, E and MPh are explicitly sent with the differential encoder on. For user data, see §F-V34 #4.

**Figure 5 points (read from the 300-dpi render of PDF p20).** Coordinates are (x, y) = (real,
imaginary) on the odd-integer grid. Column labels −43..45 run across the bottom and row labels
45..−43 down the left.

| label | coordinates | \|v\|² | used by |
|---|---|---|---|
| **0** | **(1, 1)** | 2 | 1200 and 2400 bit/s control channel; AC, Sh/S̄h, S/S̄, J/4-point TRN (duplex) |
| 1 | (−3, 1) | 10 | 2400 bit/s control channel (Q2Q1 = 01); 16-point TRN/MP |
| 2 | (1, −3) | 10 | 2400 bit/s control channel (Q2Q1 = 10); 16-point TRN/MP |
| 3 | (−3, −3) | 18 | 2400 bit/s control channel (Q2Q1 = 11); 16-point TRN/MP |
| 4, 5, 6, 7 | (1, 5), (5, 1), (−3, 5), (5, −3) | 26, 26, 34, 34 | not used here. Listed to confirm the tie-break rule: at equal magnitude, the larger imaginary part gets the lower label. |

**Constellation after rotation (derived).** Clockwise 90° maps (x, y) to (y, −x).

| Z (cw quarter turns) | Q=0 (pt 0) | Q=1 (pt 1) | Q=2 (pt 2) | Q=3 (pt 3) |
|---|---|---|---|---|
| 0 | (1, 1) | (−3, 1) | (1, −3) | (−3, −3) |
| 1 | (1, −1) | (1, 3) | (−3, −1) | (−3, 3) |
| 2 | (−1, −1) | (3, −1) | (−1, 3) | (3, 3) |
| 3 | (−1, 1) | (−1, −3) | (3, 1) | (3, −3) |

- At 1200 bit/s only the Q=0 column is used: differential QPSK on the diagonals.
- At 2400 bit/s the 16 points are {±1, ±3}². The mean energy is 10, against 2 at 1200 bit/s.
  10.2.4 gives levels but does not say how to rescale between the two modes (§F-V34 #5).

### A.3 AC, ALT and E (10.2.4.1-10.2.4.3, PDF p43)

- **AC.** Alternates point 0 and point 0 rotated 180°: (1,1), (−1,−1), (1,1), ...
  - Used only to start a control-channel retrain (12.8).
  - The responder must detect it for more than 100 ms.
  - **(derived)** Its spectrum is two lines at carrier ± 300 Hz.
- **ALT.** Control-channel modulation with the **differential encoder on**, carrying alternating
  0/1 data bits through the scrambler at 1200 bit/s. The scrambler starts in the **all-zero state**.
  The text does not say whether the first bit is 0 or 1, nor what the initial Zn−1 is (§F-V34 #3).
- **E.** A **20-bit sequence of scrambled binary ones**. It marks the start of control-channel user
  data and is sent at 1200 bit/s with the differential encoder on.
  - **(derived)** 10 symbols = 16.7 ms.
  - Contrast with duplex E (10.1.3.2): that one marks the end of MP and uses the 4- or 16-point
    Phase 4 constellation.

### A.4 PPh, equation 10-2 (PDF p46, read at 400 and 800 dpi)

PPh is four periods of an 8-symbol sequence, 32 symbols in all (= 32T at 600 Bd, 53.3 ms). It is
used for control-channel receiver initialisation and resynchronisation, and 12.4 trains the
control-channel equaliser on it.

    Set i = 2k + I,  k = 0, 1, ..., 15,  I = 0, 1 for each k
    PPh(i) = e^( jπ · [ (2k(k−1) + 1) / 4 ] )          (10-2)
    PPh(0) is transmitted first.

- The exponent has **no I in it**. At 800 dpi both 1s have the flagged digit-one glyph, the same as
  the digit 1 in the line defining I. They are not the serif capital I used for the variable.
- So each value is sent twice, and the phase depends only on `k mod 4`. With k = 0, 1, 2, 3 the
  numerator 2k(k−1)+1 is 1, 1, 5, 13 ≡ 1, 1, 5, 5 (mod 8).
- **The 32 points as printed (derived, checked by script).** p = point 0 direction
  `e^{jπ/4} = (1+j)/√2`; n = its negative `e^{j5π/4} = −(1+j)/√2` (point 0 rotated 180°).

| i | 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | 11 | 12 | 13 | 14 | 15 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| k | 0 | 0 | 1 | 1 | 2 | 2 | 3 | 3 | 4 | 4 | 5 | 5 | 6 | 6 | 7 | 7 |
| 2k(k−1)+1 | 1 | 1 | 1 | 1 | 5 | 5 | 13 | 13 | 25 | 25 | 41 | 41 | 61 | 61 | 85 | 85 |
| phase | π/4 | π/4 | π/4 | π/4 | 5π/4 | 5π/4 | 5π/4 | 5π/4 | π/4 | π/4 | π/4 | π/4 | 5π/4 | 5π/4 | 5π/4 | 5π/4 |
| point | p | p | p | p | n | n | n | n | p | p | p | p | n | n | n | n |

| i | 16 | 17 | 18 | 19 | 20 | 21 | 22 | 23 | 24 | 25 | 26 | 27 | 28 | 29 | 30 | 31 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| k | 8 | 8 | 9 | 9 | 10 | 10 | 11 | 11 | 12 | 12 | 13 | 13 | 14 | 14 | 15 | 15 |
| 2k(k−1)+1 | 113 | 113 | 145 | 145 | 181 | 181 | 221 | 221 | 265 | 265 | 313 | 313 | 365 | 365 | 421 | 421 |
| phase | π/4 | π/4 | π/4 | π/4 | 5π/4 | 5π/4 | 5π/4 | 5π/4 | π/4 | π/4 | π/4 | π/4 | 5π/4 | 5π/4 | 5π/4 | 5π/4 |
| point | p | p | p | p | n | n | n | n | p | p | p | p | n | n | n | n |

- The period is `p p p p n n n n`, which agrees with the prose of 10.2.4.5 (4 periods × 8 symbols).
- **(derived)** This is a diagonal BPSK square wave. Its spectrum is carrier ± 75 Hz and ± 225 Hz,
  easy to tell apart from AC (± 300 Hz).
- It is a poor equaliser-training sequence: its autocorrelation is (8, 4, 0, −4, −8, ...), not
  impulse-like. Treat it as printed-but-suspect and confirm it against a real capture (§F-V34 #2).
- PPh(i) is unit magnitude. Scale it like point 0 of the constellation; the text does not say
  (§F-V34 #5).

### A.5 Sh and S̄h (10.2.3.3, PDF p43, overbars checked at 250 dpi)

Both use the **control-channel modulation** (600 Bd, 1200/2400 Hz), not the primary channel.

- **Sh** alternates point 0 and point 0 rotated **counter-clockwise** 90°: (1,1), (−1,1), ...
  It **ends** on (−1,1).
- **S̄h** alternates point 0 rotated 180° and point 0 rotated counter-clockwise 270°:
  (−1,−1), (1,−1), ... It **begins** on (−1,−1).
- Durations come from clause 12, not 10.2: **Sh 24T, S̄h 8T** (12.6.1.1, 12.6.2.2).
  - **(derived)** That is 40 ms + 13.3 ms. An even 24 symbols ending on (−1,1) means Sh starts on
    (1,1). S̄h is (−1,−1), (1,−1) four times, ending on (1,−1).
- Primary S/S̄ (B.6) are the same point patterns at the primary symbol rate and carrier.

### A.6 MPh, Tables 23 and 24 (PDF p44 and p45)

- **How they are sent.** MPh uses the control-channel modulation at **1200 bit/s**, with the
  **differential encoder and scrambler on**. MPh is exchanged during control-channel start-up and
  resynchronisation.
- **Types.** Type 0 or Type 1 may be sent. Type 1 adds the precoding coefficients.
- **Precoding coefficients.**
  - They are set to 0 before the first MPh is received in a control-channel start-up.
  - A Type 0 MPh leaves them unchanged.
  - They are 16-bit two's complement with 14 fractional bits, in (−2, 2). The format is from 9.6.2
    (PDF p27), not from 10.2.

**Table 23, MPh Type 0 (PDF p44)**

| bits | Definition |
|---|---|
| 0:16 | Frame sync: 11111111111111111 (17 ones) |
| 17 | Start bit: 0 |
| 18 | Type: 0 |
| 19 | Reserved for ITU-T: sent as 0, not interpreted |
| 20:23 | Maximum data signalling rate: N × 2400, N = 1..14 (4-bit integer) |
| 24:26 | Reserved for ITU-T: sent as 0, not interpreted |
| 27 | Control-channel data rate selected **for the remote transmitter**: 0 = 1200 bit/s, 1 = 2400 bit/s (see bit 50) |
| 28 | Reserved for ITU-T: sent as 0. In duplex MP this bit is the auxiliary-channel enable, so half-duplex has **no aux channel** (B.8). |
| 29:30 | Trellis encoder select: 0 = 16-state, 1 = 32-state, 2 = 64-state, 3 = reserved. The receiver requires the remote transmitter to use it. |
| 31 | Non-linear encoder parameter for the remote transmitter: 0 → Θ = 0, 1 → Θ = 0.3125 |
| 32 | Constellation shaping for the remote transmitter: 0 = minimum, 1 = expanded (Table 10) |
| 33 | Reserved for ITU-T: sent as 0. In duplex MP this bit is the acknowledge bit; **MPh has no acknowledge bit.** |
| 34 | Start bit: 0 |
| 35:49 | Data signalling rate capability mask: bit 35 = 2400, 36 = 4800, 37 = 7200, ..., 46 = 28 800, 47 = 31 200, 48 = 33 600. Bit 49 is reserved (sent 0, not interpreted). A 1 means the rate is supported **and enabled in both transmitter and receiver**. |
| 50 | Asymmetric **control-channel** rates: 0 = not allowed, 1 = allowed. Asymmetric is used only if **both** set bit 50. If the chosen rates differ in symmetric mode, **both transmit at the lower rate**. |
| 51 | Start bit: 0 |
| 52:67 | Reserved for ITU-T: sent as 0, not interpreted |
| 68 | Start bit: 0 |
| 69:84 | CRC |
| 85:87 | Fill bits: 000 |

- NOTE 1: rates above 12 in bits 20:23 (that is, above 28 800) only if the remote supports the
  1664-point constellation (INFO0 bit 25).
- NOTE 2: the **source does not use bits 29-32 and should send them as 0**. Only the recipient's
  trellis, Θ and shaping requests matter, because only the source transmits primary data.

**Table 24, MPh Type 1 (PDF p45).** Identical to Type 0 for bits 0:51 except **bit 18 = 1**. Then:

| bits | Definition |
|---|---|
| 52:67 | Precoding coefficient h(1), real part |
| 68 | Start bit: 0 |
| 69:84 | h(1) imaginary |
| 85 | Start bit: 0 |
| 86:101 | h(2) real |
| 102 | Start bit: 0 |
| 103:118 | h(2) imaginary |
| 119 | Start bit: 0 |
| 120:135 | h(3) real |
| 136 | Start bit: 0 |
| 137:152 | h(3) imaginary |
| 153 | Start bit: 0 |
| 154:169 | Reserved for ITU-T: sent as 0, not interpreted |
| 170 | Start bit: 0 |
| 171:186 | CRC |
| 187 | Fill bit: 0 |

Type 1 carries the same NOTE 1 and NOTE 2 as Type 0.

**Derived facts for implementation.**

- **CRC coverage.** The CRC is the B.3 generator over all bits except frame sync, start bits, fill
  and the CRC itself.
  - Type 0: bits 18-33, 35-50, 52-67.
  - Type 1: bits 18-33, 35-50, 52-67, 69-84, 86-101, 103-118, 120-135, 137-152, 154-169.
- **Length.**
  - Type 0: 88 bits = 44 symbols = 73.3 ms at 1200 bit/s.
  - Type 1: 188 bits = 94 symbols = 156.7 ms.
- **Framing.**
  - Consecutive MPh: `...fill, 17×1, 0, ...`.
  - MPh then E: `...fill, 20×1, then user data (flags)`.
  - So a receiver can take ≥ 18 ones not followed by a start bit as E.
- **Who sends what.**
  - The recipient normally sends Type 1, because it owns the equaliser that computes h(p).
  - The source can send Type 0 with bits 29-32 = 0.
  - The source's bit 27 sets the recipient's control-channel transmit rate, and the other way round
    (12.4.1.4, 12.4.2.5).
  - T.30 F.3.1.4 NOTE leaves asymmetric rates (bit 50) for further study, so send bit 50 = 0.

### A.7 TRN, PP and S in half-duplex Phase 3 (10.2.3, PDF p42-43)

- **TRN (10.2.3.4).** TRN uses the 4- or 16-point 2D constellation chosen by **INFOh bit 30**,
  generated as in 10.1.3.8 (B.6):
  - scrambled ones with the scrambler preset to zero before TRN;
  - clockwise rotation by `In·90°`, with **no** differential encoding;
  - for 16 points, `2·Q2+Q1` picks points 0..3.
- The length comes from INFOh bits 15:21.
- PP (288T) and S/S̄ (128T/16T) are the duplex signals (B.6) at the primary symbol rate and carrier.

---

## B. Duplex definitions that half-duplex refers to

### B.1 INFO0: Table 14 (PDF p34 = printed p28)

INFO0 is 49 bits (0..48). The call modem sends INFO0c and the answer modem INFO0a.

| bits | Definition |
|---|---|
| 0:3 | Fill bits: 1111 |
| 4:11 | Frame sync: 01110010, left-most first |
| 12 | 1 = symbol rate 2743 supported |
| 13 | 1 = 2800 supported |
| 14 | 1 = 3429 supported |
| 15 | 1 = can transmit at the **low** carrier at 3000 Bd |
| 16 | 1 = can transmit at the **high** carrier at 3000 Bd |
| 17 | 1 = can transmit at the low carrier at 3200 Bd |
| 18 | 1 = can transmit at the high carrier at 3200 Bd |
| 19 | **0** = transmitting at 3429 Bd is **disallowed** |
| 20 | 1 = can reduce transmit power below nominal |
| 21:23 | Maximum allowed difference between transmit and receive symbol rates, in steps (0 = 2400 ... 5 = 3429), 0..5 |
| 24 | 1 in an INFO0 sent by a CME modem |
| 25 | 1 = supports up to 1664-point constellations |
| 26:27 | Transmit clock source: 0 = internal, 1 = synchronised to receive timing, 2 = external, 3 = reserved |
| 28 | 1 = acknowledges correct reception of an INFO0 frame **during error recovery** |
| 29:44 | CRC |
| 45:48 | Fill bits: 1111 |

- NOTE 1: bits 12-14 give capability and/or configuration. Bits 15-20 depend on regulation and
  apply only to the modem's **transmitter**.
- NOTE 2: bit 24 may be used with the V.8 GSTN-access octet.
- **(derived)** The CRC covers bits 12..28. Airtime at 600 bit/s is 81.7 ms, plus the reference
  symbol.

**What distinguishes a half-duplex call?** Nothing in INFO0. Half-duplex is chosen in V.8, by CM/JM
modn0 bit b7 (section E). In half-duplex:

- The **source's** INFO0 bits 12-20 and 25 bound what the recipient may ask for in INFOh:
  - symbol rate, from bits 12-14 and 19;
  - carrier, from bits 15-18 against INFOh bit 22;
  - power reduction, from bit 20 against INFOh 12:14;
  - rates above 28 800 in MPh, from bit 25.
- Bits 21:23 (asymmetric symbol rates) have no use, because only one direction carries primary
  data (my reading; nothing says so).
- Bit 28 is used exactly as in duplex:
  - Each side sends its first INFO0 with bit 28 = 0 (12.2.x.x.1).
  - It sets bit 28 = 1 after correctly receiving the other side's INFO0 (NOTEs to 12.2.1.3, 12.2.1.4,
    12.2.2.3, 12.2.2.4).
  - Receiving INFO0 with bit 28 = 1 shortens the recovery loop (C.2).

### B.2 INFO modulation, 10.1.2.3.1 (PDF p32)

- **Modulation.** Binary DPSK at **600 bit/s ± 0.01%**. A 1 rotates the point **180°** from the
  previous one; a 0 rotates it 0°.
- **Reference point.** Each INFO sequence is preceded by one point at an arbitrary carrier phase.
  When several INFO sequences go as a group, only the first gets it.
- **Answer transmitter.** 2400 Hz ± 0.01% at 1 dB below nominal, plus an 1800 Hz ± 0.01% guard tone
  at 7 dB below nominal.
- **Call transmitter.** 1200 Hz ± 0.01% at nominal.
- **Spectrum.** Figure 13 (the mask in A.2).
- **INFOMARKS (10.1.2.3.6, PDF p37).** Binary ones into the same DPSK modulator. They are used in
  duplex error recovery; half-duplex clause 12 never uses them.

### B.3 CRC, 10.1.2.3.2 and Figure 14 (PDF p33)

- **Polynomial** x¹⁶ + x¹² + x⁵ + 1.
- **Procedure.** Load the register with all ones. Shift in the sequence: every information bit
  except frame-sync, start and fill bits. Output the register **starting with bit 0**; bit 0 of the
  CRC is its LSB.
- **Figure 14 (rendered).**
  - Register cells 15 14 13 12 11 → ⊕ → 10 9 8 7 6 5 4 → ⊕ → 3 2 1 0 → ⊕.
  - At the last ⊕ the information bit comes in from below.
  - That ⊕ output feeds back into cell 15 and into the two inner ⊕s: before cell 10 and before
    cell 3.
- **(derived)** This is the reflected CRC-CCITT:
  `fb = (reg ^ bit) & 1; reg >>= 1; if fb { reg ^= 0x8408 }`, with reg preset to 0xFFFF.
  There is **no** final inversion (unlike the HDLC FCS). The field is sent LSB first (the lowest
  field bit number = register bit 0).
- The same generator serves INFO0, INFOh, MP and MPh. 10.1.3.9 points MP at 10.1.2.3.2; the MPh
  tables just say CRC.

### B.4 Tones A and B, 10.1.2.1-10.1.2.2 (PDF p32)

- **Tone A.** 2400 Hz, sent by the answer modem.
  - A↔Ā and Ā↔A transitions are **180° phase reversals**.
  - While A/Ā is sent, the answer modem also sends an **1800 Hz guard tone with no reversals**.
  - **Tone A is sent 1 dB below nominal, and the guard tone at nominal power.** That is as printed,
    and it differs from INFO and the control channel, where the guard is at −7 dB (§F-V34 #6).
- **Tone B.** 1200 Hz, sent by the call modem. B↔B̄ transitions are 180° reversals. No guard tone
  is specified. Level: nominal (10.1.2 / 10.2.2).
- NOTE, for both tones: do not band-limit a reversing tone so hard that round-trip measurement
  suffers. Half-duplex makes no round-trip measurement, but the reversal is still timed.

### B.5 Line probing L1 and L2, 10.1.2.4 and Table 17 (PDF p37)

- **L1.** Periodic at **150 Hz ± 0.01%**: cosines every 150 Hz from 150 to 3750 Hz, **omitting 900,
  1200, 1800 and 2400 Hz** (21 tones). Sent for **160 ms (24 repetitions) at 6 dB above nominal**.
- **L2.** The same signal at **nominal** power, for **no longer than 550 ms plus a round-trip delay**
  (see §F-V34 #7 for half-duplex).
- **Table 17** (read from PDF p37), cos(2πft + φ):

| f (Hz) | 150 | 300 | 450 | 600 | 750 | 1050 | 1350 | 1500 | 1650 | 1950 | 2100 | 2250 | 2550 | 2700 | 2850 | 3000 | 3150 | 3300 | 3450 | 3600 | 3750 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| φ (deg) | 0 | 180 | 0 | 0 | 0 | 0 | 0 | 0 | 180 | 0 | 0 | 180 | 0 | 180 | 0 | 180 | 180 | 180 | 180 | 0 | 0 |

- In half-duplex **only the source sends L1/L2**, and only the recipient analyses them.

### B.6 Primary-channel training signals, 10.1.3 (PDF p38-39)

- **S / S̄ (10.1.3.7).** Same point patterns as Sh/S̄h (A.5), but at the primary symbol rate and
  carrier.
  - S alternates point 0 and point 0 rotated counter-clockwise 90°, and **ends** on (−1,1).
  - S̄ alternates point 0 rotated 180° and rotated counter-clockwise 270°, and **begins** on
    (−1,−1).
  - Half-duplex durations: **S 128T, S̄ 16T** (12.3.1.1, 12.5.1).
- **PP (10.1.3.6, eq. 10-1, rendered).** Six periods of a 48-symbol sequence, i = 0..287.

      i = 4k + I,  k = 0..71,  I = 0..3
      PP(i) = e^( jπ(kI + 4)/6 )   if k mod 3 = 1
            = e^( jπkI/6 )         otherwise
      PP(0) first.

  PP is **288T** (Figs 23/24).
- **TRN (10.1.3.8).**
  - Binary ones into the clause 7 scrambler, which is **initialised to zero before TRN**.
  - **4-point:** I1n (first) and I2n per symbol. The point is point 0 rotated **clockwise** by
    `In·90°`, `In = 2·I2n + I1n`. No differential encoding.
  - **16-point:** I1n, I2n, Q1n, Q2n per symbol. `2·Q2n + Q1n` selects the point, then it is
    rotated clockwise by `In·90°`.
- **B1 (10.1.3.1).** One data frame of scrambled ones, in the selected data-mode parameters.
  - Superframe bit inversions are inserted as if this were the **last** data frame of a superframe.
  - Before B1, the scrambler, trellis encoder, differential encoder and precoder delay line are all
    **initialised to zero**.
  - Mapping frame i = 0 is the first mapping frame of B1 (8.1).
  - In half-duplex B1 follows PP in every primary-channel resync (12.5.1).

### B.7 Scrambler, clause 7 (PDF p16)

- **Self-synchronising.** It scrambles primary-channel data; in half-duplex it also scrambles
  control-channel data (10.2.4). Auxiliary-channel data is not scrambled.
- **One polynomial per direction:**
  - call-modem transmitter: **GPC = 1 + x⁻¹⁸ + x⁻²³**;
  - answer-modem transmitter: **GPA = 1 + x⁻⁵ + x⁻²³**.
- The transmitter divides the data by the polynomial.
- **(derived)** Scrambler: `out[n] = in[n] ⊕ out[n−18] ⊕ out[n−23]` (call), or
  `⊕ out[n−5] ⊕ out[n−23]` (answer).
- **(derived)** Descrambler: `d[n] = r[n] ⊕ r[n−18] ⊕ r[n−23]` (or with n−5). A half-duplex receiver
  descrambles with the **far end's** call/answer polynomial, whether it is receiving the primary or
  the control channel.
- **Resets in half-duplex:**
  - before TRN (10.1.3.8);
  - at ALT start, all zeros (10.2.4.2);
  - before B1 (10.1.3.1).

### B.8 Data-mode framing and encoder in half-duplex (cl. 5, 8, 9)

Half-duplex primary data mode uses the duplex data-mode encoder of clauses 8-9 unchanged. There is
no separate half-duplex data mode.

- **Symbol rates (Table 1, PDF p10).**
  - S = (a/c) · 2400 ± 0.01%, with (a, c) = 2400 (1, 1), 2743 (8, 7), 2800 (7, 6), 3000 (5, 4),
    3200 (4, 3), 3429 (10, 7).
  - 2400, 3000 and 3200 are mandatory.
- **Carriers (Table 2, PDF p11).** Carrier = (d/e) · S.

| S | low carrier Hz (d/e) | high carrier Hz (d/e) |
|---|---|---|
| 2400 | 1600 (2/3) | 1800 (3/4) |
| 2743 | 1646 (3/5) | 1829 (2/3) |
| 2800 | 1680 (3/5) | 1867 (2/3) |
| 3000 | 1800 (3/5) | 2000 (2/3) |
| 3200 | 1829 (4/7) | 1920 (3/5) |
| 3429 | 1959 (4/7) | 1959 (4/7) |

- **Framing (8.1, Fig 3, Table 7; PDF p16-17).**
  - Superframe = **280 ms** = J data frames: J = 7 at 2400, 2800, 3000 and 3200, and J = 8 at 2743
    and 3429.
  - Data frame = P mapping frames = 40 ms (J=7) or 35 ms (J=8).
  - Mapping frame = four 4D symbols; a 4D symbol = two 2D symbols.
  - Superframe sync uses V0 bit inversions (9.6.3, Table 12 on PDF p28): J=8 pattern
    `01 11 01 11 11 11 10 10`, J=7 pattern `01 11 01 11 11 11 10`.

| S | 2400 | 2743 | 2800 | 3000 | 3200 | 3429 |
|---|---|---|---|---|---|---|
| J | 7 | 8 | 7 | 7 | 7 | 8 |
| P | 12 | 12 | 14 | 15 | 16 | 15 |

- **Bits per data frame (8.2).** N = R · 0.28 / J, where R = primary + aux rate.
  - b = ⌈N/P⌉ and r = N − (b−1)P.
  - SWP comes from Table 8 (PDF p18), or from the counter algorithm in 8.2.
- **Mapping parameters (9.2, eq. 9-1 and 9-2; PDF p20-21; Table 10 on PDF p21-23).**
  - K = 0 if b ≤ 12. Otherwise K = b − 12 − 8q, where q is the smallest non-negative integer that
    makes K < 32 (q = 0 when K = 0).
  - M minimum = the smallest integer ≥ 2^(K/8).
  - M expanded = the nearest integer to 1.25 · 2^(K/8), and not less than the minimum.
  - L = 4M · 2^q.
  - Table 10 tabulates K, M (minimum and expanded) and L per rate and symbol rate, the same as in
    duplex. Use its rows at primary-only rates.
- **Auxiliary channel: half-duplex has none.**
  - The aux channel (8.3, Table 9) is used only when both modems declare it (5.1), and that
    declaration is MP bit 28. In MPh bit 28 is reserved and sent as 0 (A.6).
  - So in half-duplex **R = the primary rate**. Use the Table 8 rows at multiples of 2400
    (2400 ... 33 600), not the "+200" rows (2600, 5000, ..., 33 800).
  - AMP and W are unused: every I1i,0 carries a primary bit.
  - Annex A (modem-control channel over aux, MP bits 52-55 / 154-157) is duplex-only; MPh bits
    52:67 are reserved or precoder.
- **Encoder (cl. 9).** Shell mapping, the 9.5 differential encoder, the 9.6.2 precoder, the trellis
  and the 9.7 non-linear encoder are as in duplex. The per-link choices come from the
  **recipient's** MPh:
  - trellis states, bits 29:30;
  - Θ, bit 31;
  - shaping, bit 32 (Table 10: minimum or expanded M);
  - h(1..3), Type 1 bits 52:152.
- **Pre-emphasis index (INFOh 23:26).** Tables 3/4 (PDF p12). Index 0-5: α = 0, 2, 4, 6, 8, 10 dB
  (Fig 1 shapes). Index 6-10: (β, γ) = (0.5, 1.0), (1.0, 2.0), (1.5, 3.0), (2.0, 4.0),
  (2.5, 5.0) dB (Fig 2 shapes).

### B.9 Phase 1: 11.1, which half-duplex adopts wholesale (12.1; PDF p46-47)

**Call modem (11.1.1).**

- Watch for ANS or ANSam. Send CI, CT, CNG or nothing.
- On ANSam: silence Te (per V.8), then send CM with the modulation bits set for V.34, and listen for
  JM.
- After at least **two identical JM**: finish the current CM octet and send CJ.
- Then silence **75 ± 5 ms**, and Phase 2.
- Branch on JM (11.1.1.2):
  - duplex → 11.2;
  - **half-duplex → 12.2**;
  - no V.34 → V.8 (for fax: T.30 cl. 5, see E).
- On ANS rather than ANSam (11.1.1.3): V.32 bis Annex A, T.30, etc.

**Answer modem (11.1.2).**

- Stay silent for **at least 200 ms**, then send ANSam per V.8.
  - Duplex: ANSam **must** carry the phase reversals.
  - **Half-duplex: the reversals are optional** (11.1.2.1).
- After at least 2 identical CM showing V.34: send JM and watch for CJ.
- After **all 3 octets of CJ**: silence **75 ± 5 ms**, then Phase 2 (11.1.2.2).
- Branch on JM exactly as the call modem does (11.1.2.3).
- No CM for the whole allowed ANSam time: silence 75 ± 5 ms, then V.32 bis Annex A, T.30, etc.
  (11.1.2.5).

**Figure 15 (PDF p46).**

- Call: [CI, CI, ... dashed, optional] ... **Te** ... CM, CM, ... | CJ | ~75 ± 5 ms~ | INFO0c.
- Answer: ~≥ 200 ms~ | ANSam | JM, JM, ... | ~75 ± 5 ms~ | INFO0a.
- Arrows (read at 300 dpi):
  - start of ANSam → the call ends CI, and Te starts there;
  - start of CM → the answer detects it near the end of ANSam, then starts JM;
  - start of JM → the call detects it inside CM, which leads to CJ;
  - end of CJ → the answer ends JM.

### B.10 Interface circuits in half-duplex (6.3, 6.6.2; PDF p15-16)

- **6.3.** In half-duplex the primary and control channels share the Table 5 primary interchange
  circuits (103/104/105/106/109). How data is steered to one channel or the other is outside V.34.
  - **(derived)** For BinModem: a single "channel mode" (control or primary) plus the
    105 / 106 / 104 / 109 events that clause 12 uses.
- **6.6.2, half-duplex Circuit 109 (carrier detect).**
  - ON above −43 dBm; OFF below −48 dBm.
  - 109 turns OFF **20 to 25 ms** after the level falls below threshold.
  - Hysteresis: the OFF→ON threshold is at least 2 dB above the ON→OFF threshold.
- **6.1.** After start-up and retrain, 106 follows 105 within 2 ms.
- **Clause 12 circuit behaviour:**
  - 104 is clamped to binary 1 whenever the receiver is not delivering valid data.
  - After receiving E (control) or B1 (primary): unclamp 104 and turn 109 ON.
  - 106 turns ON only after the modem's own E (control) or B1 (primary).
  - 12.5.3.2 and 12.6.3.x also turn 109 OFF / clamp 104 if the level falls below the 6.6.2 threshold
    mid-mode, and restore both when it returns.

---

## C. V.34 clause 12: half-duplex procedures (PDF p59-70)

Clause 12 (PDF p59) defines half-duplex as primary data flowing one way, source to recipient, while
control-channel data flows both ways at the same time. It is written per role. Below, each role is
a numbered procedure. `→` means "go to". Timeouts are given inline, and every recovery path is
listed after the main path.

### C.0 Every timing in clause 12

| Where | What | Value |
|---|---|---|
| 11.1 / 12.1 | answer silence after going off-hook, before ANSam | ≥ 200 ms |
| 11.1 / 12.1 | silence after CJ (call) / after JM (answer), before INFO0 | 75 ± 5 ms |
| 12.2 (recipient role) | own tone sent before own reversal, and far tone detected | ≥ 50 ms |
| 12.2 (source role) | detect far reversal, then send own reversal | 40 ± 10 ms (duplex 11.2 is 40 ± 1 ms) |
| 12.2 | tone continues after a reversal | 10 ms |
| 12.2 | L1 (source) | 160 ms at +6 dB |
| 12.2 | recipient may receive L2 for | ≤ 500 ms |
| Figs 23/24 | recipient: far reversal detected to own tone start | ≤ 670 ms |
| 12.2 | recipient: far tone detected to INFOh start | 25 ms of continued tone |
| 12.2.1.3.3, 12.2.2.4.3 | source: no far tone after own reversal | 2700 ms, then re-probe |
| 12.2.1.3.4, 12.2.2.4.4 | source: no INFOh after starting own tone | 2000 ms, then keep tone and wait for far tone |
| 12.2.1.4.2, 12.2.2.3.2 | recipient: no far reversal after own reversal | 2000 ms, then back to tone |
| 12.2.1.4.3, 12.2.2.3.3 | recipient: no far tone after starting own tone | 2000 ms, then send INFOh anyway |
| 12.3.1.1 | source: INFOh received to S start | 70 ± 5 ms silence |
| 12.3.1.1, 12.5.1 | S, S̄, PP | 128T, 16T, PP (Figs: 288T), primary T |
| INFOh 15:21 | TRN | n × 35 ms, n = 0..127 |
| 12.3.3 | recipient: no S | 2000 ms, then back to Phase 2 tone |
| 12.4.1.1 | source: silence before PPh | 70 ± 5 ms |
| 10.2.4.5, Figs 25-28 | PPh | 32T (600 Bd) |
| 12.4.1.1 | source ALT after PPh | ≥ 16T |
| 12.4.1.2 | source: PPh received to MPh start | < 120T |
| 12.4.2.3 | recipient ALT | ≥ 16T and ≤ 120T |
| 10.2.4.3 | E | 20 bits = 10T |
| 12.4.3.x, 12.4.4.x, 12.6.1.5-6, 12.6.2.4-5 | missing PPh, MPh, E or Sh/S̄h | 3 s, then control-channel retrain (12.8.1) |
| 12.5.1 | source: silence before S (resync) | 70 ± 5 ms |
| 12.5.3.1 | source primary turn-off | 35 ms of scrambled ones |
| 12.6.1.1 | source: silence before Sh | 70 ± 5 ms |
| 12.6.1.1, 12.6.2.2 | Sh, S̄h | 24T, 8T (600 Bd) |
| 12.6.1.4, 12.6.2.2 | ALT before E (resync) | ≥ 16T and ≤ 120T |
| 12.6.3.1-2 | control-channel turn-off | 4T of scrambled ones |
| 12.7 | retrain: silence before tone | 70 ± 5 ms |
| 12.7 | retrain: respond after far tone lasts | > 50 ms |
| 12.8.2 | respond to AC after it lasts | > 100 ms |
| 6.6.2 | 109 OFF after level drop | 20-25 ms |

**(derived)** Primary-channel durations of S 128T / S̄ 16T / PP 288T:

| S (Bd) | 2400 | 2743 | 2800 | 3000 | 3200 | 3429 |
|---|---|---|---|---|---|---|
| S | 53.3 ms | 46.7 | 45.7 | 42.7 | 40.0 | 37.3 |
| S̄ | 6.7 ms | 5.8 | 5.7 | 5.3 | 5.0 | 4.7 |
| PP | 120 ms | 105 | 102.9 | 96 | 90 | 84 |

**(derived)** Control channel at 600 Bd:

| Signal | Length |
|---|---|
| PPh | 53.3 ms |
| Sh | 40 ms |
| S̄h | 13.3 ms |
| ALT 16T / 120T | 26.7 ms / 200 ms |
| E | 16.7 ms |
| MPh Type 0 | 73.3 ms |
| MPh Type 1 | 156.7 ms |
| 4T | 6.7 ms |

### C.1 Phase 1 (12.1)

Identical to 11.1 (section B.9).

### C.2 Phase 2, probing (12.2; text PDF p59-63, Figs 23/24 PDF p60/p62)

INFO sequences: INFO0 (10.1.2.3) and INFOh (10.2.2.1). Only the source probes. There is no
round-trip measurement and no second reversal pair (contrast 11.2).

#### C.2.1 Call modem = source (12.2.1.1; recovery 12.2.1.3)

1. **(12.2.1.1.1)** During the 75 ± 5 ms silence that ends Phase 1, prepare to receive INFO0a and
   detect Tone A. After the silence, send **INFO0c with bit 28 = 0**, then **Tone B**.
2. **(12.2.1.1.2)** After INFO0a is received, listen for Tone A and then its reversal.
3. **(12.2.1.1.3)** On the Tone A reversal:
   - wait **40 ± 10 ms**, then send the **Tone B reversal**;
   - keep B̄ for **10 ms**;
   - send **L1 for 160 ms**;
   - send **L2** and listen for Tone A.
4. **(12.2.1.1.4)** On Tone A: send **Tone B** and prepare for INFOh. On INFOh → 12.3.1
   (Phase 3, source).

Recovery:

- **R1 (12.2.1.3.1).** In step 2 or 3: if Tone A is detected before INFO0a is correctly received,
  or INFO0a keeps repeating → send INFO0c repeatedly.
  - If INFO0a arrives **with bit 28 = 1**: listen for Tone A and then its reversal, finish the
    current INFO0c, send Tone B.
  - If instead Tone A is detected *after* INFO0a was correctly received: listen for the reversal,
    finish the current INFO0c, send Tone B.
  - Either way → step 3.
- **R2 (12.2.1.3.2).** In step 3, no A reversal → keep sending Tone B and wait for another reversal.
  No timeout is given.
- **R3 (12.2.1.3.3).** In step 4, Tone A not detected within **2700 ms** of sending the B reversal
  → stop L2, send Tone B, listen for Tone A and then an A reversal → step 3. This probes again.
- **R4 (12.2.1.3.4).** In step 4, INFOh not detected within **2000 ms** of sending Tone B → keep
  sending Tone B, listen for Tone A. On Tone A → step 4.
- NOTE: set INFO0c bit 28 = 1 once INFO0a has been correctly received.

#### C.2.2 Answer modem = recipient (12.2.1.2; recovery 12.2.1.4)

1. **(12.2.1.2.1)** During the 75 ± 5 ms silence, prepare to receive INFO0c and detect Tone B.
   After the silence, send **INFO0a with bit 28 = 0**, then **Tone A**.
2. **(12.2.1.2.2)** After INFO0c is received, listen for Tone B, and receive INFO0c again.
3. **(12.2.1.2.3)** When Tone B **has been detected and Tone A has been sent for at least 50 ms**:
   - send the **Tone A reversal**;
   - keep Ā for **10 ms**;
   - go **silent**;
   - listen for the Tone B reversal.
4. **(12.2.1.2.4)** On the B reversal, prepare to receive L1 and L2.
5. **(12.2.1.2.5)** Receive L1 for its 160 ms. Optionally receive L2 for **≤ 500 ms**. Then send
   **Tone A** and listen for Tone B. This last clause is printed in bold on PDF p59, probably marking
   a 1998 revision (my inference).
6. **(12.2.1.2.6)** On Tone B: keep Tone A for **25 ms**, then send **INFOh**. After INFOh →
   12.3.2 (Phase 3, recipient).

Recovery:

- **R1 (12.2.1.4.1).** In step 2 or 3: if Tone B is detected before INFO0c is correctly received,
  or INFO0c keeps repeating → send INFO0a repeatedly.
  - If INFO0c arrives **with bit 28 = 1**: listen for Tone B, finish the current INFO0a, send
    Tone A.
  - If instead Tone B is detected after INFO0c was correctly received: finish the current INFO0a,
    send Tone A.
  - Either way → step 3.
- **R2 (12.2.1.4.2).** In step 4, B reversal not detected within **2000 ms** of sending the A
  reversal → listen for Tone B. On Tone B → send Tone A → step 3.
- **R3 (12.2.1.4.3).** In step 6, Tone B not detected within **2000 ms** of starting Tone A in
  step 5 → **send INFOh anyway** and go to Phase 3.
- NOTE: set INFO0a bit 28 = 1 once INFO0c has been correctly received.

#### C.2.3 Call modem = recipient (12.2.2.1; recovery 12.2.2.3)

1. **(12.2.2.1.1)** During the 75 ± 5 ms silence, prepare for INFO0a and Tone A. Then send
   **INFO0c (bit 28 = 0)**, then **Tone B**.
2. **(12.2.2.1.2)** After INFO0a is received, listen for Tone A, and receive INFO0a again.
3. **(12.2.2.1.3)** When Tone A **has been detected and Tone B has been sent for at least 50 ms**:
   - send the **Tone B reversal**;
   - keep B̄ for **10 ms**;
   - go **silent**;
   - listen for the Tone A reversal.
4. **(12.2.2.1.4)** On the A reversal, prepare for L1 and L2.
5. **(12.2.2.1.5)** Receive L1 for 160 ms, then L2 for **≤ 500 ms**. Then send **Tone B** and listen
   for Tone A.
6. **(12.2.2.1.6)** On Tone A: keep Tone B for **25 ms**, then **INFOh**. After INFOh → 12.3.2.

Recovery:

- **R1 (12.2.2.3.1).** In step 2 or 3: if Tone A is detected before INFO0a is correctly received,
  or INFO0a keeps repeating → send INFO0c repeatedly.
  - If INFO0a arrives with bit 28 = 1: listen for Tone A and send Tone B.
  - If instead Tone A is detected after INFO0a was correctly received: send Tone B.
  - Either way → step 3.
  - Unlike 12.2.1.3.1, this clause does not mention completing the current INFO0c.
- **R2 (12.2.2.3.2).** In step 4, A reversal not detected within **2000 ms** of sending the B
  reversal → listen for Tone A. On Tone A → send Tone B → step 3.
- **R3 (12.2.2.3.3).** In step 6, Tone A not detected within **2000 ms** of sending Tone B in step 5
  → **send INFOh anyway** → 12.3.2.
- NOTE: INFO0c bit 28, as in C.2.1.

#### C.2.4 Answer modem = source (12.2.2.2; recovery 12.2.2.4)

1. **(12.2.2.2.1)** During the 75 ± 5 ms silence, prepare for INFO0c and Tone B. Then send
   **INFO0a (bit 28 = 0)**, then **Tone A**.
2. **(12.2.2.2.2)** After INFO0c is received, listen for Tone B and then its reversal.
3. **(12.2.2.2.3)** On the B reversal:
   - wait **40 ± 10 ms**, then send the **Tone A reversal**;
   - keep Ā for **10 ms**;
   - send **L1 for 160 ms**;
   - send **L2** and listen for Tone B.
4. **(12.2.2.2.4)** On Tone B: send **Tone A** and prepare for INFOh. On INFOh → 12.3.1.

Recovery:

- **R1 (12.2.2.4.1).** In step 2 or 3: if Tone B is detected before INFO0c is correctly received,
  or INFO0c keeps repeating → send INFO0a repeatedly.
  - If INFO0c arrives with bit 28 = 1: listen for Tone B and then its reversal, send Tone A.
  - If instead Tone B is detected after INFO0c was correctly received: listen for the B reversal,
    send Tone A.
  - Either way → step 3.
- **R2 (12.2.2.4.2).** In step 3, no B reversal → keep sending Tone A and wait for another
  reversal. No timeout is given.
- **R3 (12.2.2.4.3).** In step 4, Tone B not detected within **2700 ms** of the A reversal → send
  Tone A, listen for Tone B and then a reversal → step 3.
- **R4 (12.2.2.4.4).** In step 4, INFOh not received within **2000 ms** of sending Tone A → keep
  sending Tone A, listen for Tone B. On Tone B → step 4.
- NOTE: INFO0a bit 28, as in C.2.2.

### C.3 Phase 3, primary-channel equaliser training (12.3, PDF p63)

**Source (12.3.1).**

1. After INFOh is received: **silence 70 ± 5 ms**, then **S for 128T**, then **S̄ for 16T**, then
   **PP** (288T per Figs 23/24).
2. After PP, send **TRN**. Its constellation and duration are set by the received INFOh (bit 30 and
   bits 15:21).
3. After TRN → control channel (12.4). By 12.4.1.1 that starts with another 70 ± 5 ms of silence,
   then PPh.

**Recipient (12.3.2).**

1. After sending INFOh: stay silent and listen for **S followed by S̄**.
2. On S→S̄: train the main equaliser on PP. It may refine the equaliser on TRN.
3. After TRN has lasted the INFOh duration → control channel (12.4). The end of TRN is found **by
   counting**, not by detecting a marker.

**Recipient recovery (12.3.3).** In step 2, if **S is not detected within 2000 ms** or TRN was not
received well:

- if the recipient is the **answer** modem: listen for Tone B, send Tone A, carry on at 12.2.1.2.6
  (on Tone B: 25 ms more of A, then a new INFOh);
- if it is the **call** modem: listen for Tone A, send Tone B, carry on at 12.2.2.1.6.

The source is by then waiting for PPh (12.4.1.1). It sees the tone instead and follows 12.4.3.1
(C.4), which repeats Phase 3.

### C.4 Control-channel start-up (12.4, PDF p63-66; Figs 25/26)

The control channel carries information before and between primary transmissions. Fig 25 covers
the first training, and a restart requested by the **source**. Fig 26 covers a restart requested
by the **recipient** (reached through 12.6.2.3).

**Source (12.4.1).**

1. **(12.4.1.1)** Listen for PPh. After **70 ± 5 ms of silence**, send **PPh** then **ALT for at
   least 16T**. When PPh is detected: train the control-channel equaliser on it and prepare for MPh.
2. **(12.4.1.2)** After PPh is received, send **MPh within 120T**.
3. **(12.4.1.3)** Once at least one MPh has been received while sending MPh:
   - finish the current MPh;
   - send **one** 20-bit **E**;
   - set the **transmit rate** to the highest enabled rate that is ≤ the rates in **both** MPh.
4. **(12.4.1.4)** After sending E:
   - let 106 follow 105;
   - send user control data at the rate given in the **recipient's** MPh (its bit 27).

   After receiving E: unclamp 104, turn 109 ON, and receive at the rate given in the **source's own**
   MPh.

**Recipient (12.4.2).**

1. **(12.4.2.1)** Listen for PPh. On PPh:
   - send **PPh**;
   - train the equaliser on the received PPh;
   - prepare for MPh.
2. **(12.4.2.2)** After its own PPh, send **ALT**.
3. **(12.4.2.3)** After ALT for **at least 16T and at most 120T**, send **MPh**.
4. **(12.4.2.4)** Once at least one MPh has been received while sending MPh:
   - finish the current MPh;
   - send **one E**;
   - set the **receive rate** to the highest enabled rate that is ≤ the rates in both MPh.
5. **(12.4.2.5)** After E:
   - let 106 follow 105;
   - send at the rate given in the **source's** MPh.

   After receiving E: unclamp 104, turn 109 ON, and receive at the rate given in its **own** MPh.

**Source recovery (12.4.3).**

- **12.4.3.1.** In step 1, if the source is the **call** modem and detects **Tone A** instead of
  PPh: send Tone B. On INFOh → 12.3.1 (Phase 3 again).
  - If it is the **answer** modem and detects **Tone B**: send Tone A. On INFOh → 12.3.1.
  - NOTE: this applies only to the control-channel start-up that follows Phase 3.
- **12.4.3.2.** In step 1, no PPh within **3 s** of sending PPh → control-channel retrain (12.8.1).
- **12.4.3.3.** In step 3, no MPh within **3 s** of receiving PPh → retrain.
- **12.4.3.4.** In step 4, no E within **3 s** of receiving MPh → retrain.

**Recipient recovery (12.4.4).**

- **12.4.4.1.** In step 1, no PPh within **3 s** of the end of TRN or of primary data → retrain.
- **12.4.4.2.** In step 4, no MPh within **3 s** of sending PPh → retrain.
- **12.4.4.3.** In step 5, no E within **3 s** of receiving MPh → retrain.

Notes:

- MPh carries no acknowledge bit. Receiving at least one MPh while still sending MPh is the whole
  handshake, and E terminates it.
- If E is lost, the 3 s timers lead to a retrain.

### C.5 Primary-channel resynchronisation and turn-off (12.5, PDF p66)

- **Source (12.5.1).**
  1. Silence **70 ± 5 ms**.
  2. **S 128T**, **S̄ 16T**, **PP**, **B1**.
  3. Let 106 follow 105, then send user data.
  - T.30 inserts the T.4 A.3.1 flags here (D.7).
- **Recipient (12.5.2).**
  1. Listen for S and S̄.
  2. Resynchronise on PP. There is no TRN; the Phase 3 equaliser is reused.
  3. After **B1**: unclamp 104, turn 109 ON, and receive data.
- **Turn-off, source (12.5.3.1).** In primary mode, when 105 goes **ON→OFF**:
  1. turn 106 OFF;
  2. send **35 ms of scrambled ones**;
  3. → 12.6.1.1.
- **Turn-off, recipient (12.5.3.2).** In primary mode, when 105 goes **OFF→ON** (the recipient's DTE
  wants to talk): turn 109 OFF and clamp 104, then → 12.6.2.
  - If the received level falls below the 6.6.2 turn-off threshold: 109 OFF and clamp 104.
  - If it comes back above the turn-on threshold: 109 ON and unclamp.

### C.6 Control-channel resynchronisation and turn-off (12.6, PDF p67-69; Fig 27)

This is the normal between-pages path.

**Source (12.6.1).**

1. **(12.6.1.1)** If new modulation parameters are wanted → 12.4.1.1 (start-up, PPh). Otherwise:
   silence **70 ± 5 ms**, then **Sh 24T**, then **S̄h 8T**.
2. **(12.6.1.2)** Listen for **PPh** or **Sh followed by S̄h**, and send **ALT**.
3. **(12.6.1.3)** If PPh is detected:
   - send **PPh**, then ALT for **at least 16T**;
   - prepare for MPh;
   - → 12.4.1.2 (MPh within 120T of receiving PPh).
4. **(12.6.1.4)** If Sh→S̄h is detected:
   - listen for E;
   - send ALT for **16T to 120T**, then **E**;
   - let 106 follow 105;
   - send control data at the **previous** control-channel rate.

   After receiving E: unclamp 104, turn 109 ON.
5. **(12.6.1.5)** In step 2, neither PPh nor Sh→S̄h within **3 s** of sending Sh→S̄h → retrain
   (12.8.1).
6. **(12.6.1.6)** In step 4, no E within **3 s** of sending Sh→S̄h → retrain.

**Recipient (12.6.2).**

1. **(12.6.2.1)** Listen for **PPh** or **Sh→S̄h**. On PPh:
   - send **PPh**;
   - prepare for MPh;
   - → 12.4.2.2 (ALT, then MPh).
2. **(12.6.2.2)** On Sh→S̄h with **no** change wanted:
   - send **Sh 24T**, **S̄h 8T**;
   - send **ALT for 16T to 120T**, then **E**;
   - let 106 follow 105;
   - send at the **previous** control-channel rate.

   After receiving E: unclamp 104, turn 109 ON.
3. **(12.6.2.3)** On Sh→S̄h **with** a change wanted:
   - send **PPh** then **ALT**;
   - listen for PPh;
   - on PPh → 12.4.2.3.

   This is Fig 26: the source switches from ALT to PPh by 12.6.1.3.
4. **(12.6.2.4)** In step 1, neither PPh nor Sh→S̄h within **3 s** of the end of primary data →
   retrain.
5. **(12.6.2.5)** In step 2, no E within **3 s** of sending Sh→S̄h → retrain.

**Control-channel turn-off (12.6.3).**

- **Source (12.6.3.1).** In control mode, when 105 goes **ON→OFF**:
  1. turn 106 OFF;
  2. send **4T of scrambled ones**;
  3. → 12.5.1, which begins with 70 ± 5 ms of silence.
  - Same 6.6.2 level rule for 109/104.
- **Recipient (12.6.3.2).** In control mode, when 105 goes **ON→OFF**:
  1. turn 106 OFF;
  2. send **4T of scrambled ones**;
  3. go **silent**;
  4. → 12.5.2.
  - Same level rule.

### C.7 Primary-channel retrains (12.7, PDF p69)

These use the Phase 2 tones: no INFO0, and the INFO0 values from the first exchange stand. T.30
F.3.3 makes their use during phase C a matter for further study.

- **Call modem, initiating (12.7.1.1).** Source or recipient:
  1. turn 106 OFF (if ON), clamp 104 to 1, and stay silent **70 ± 5 ms**;
  2. send **Tone B** and listen for Tone A;
  3. on Tone A, listen for the **A reversal**;
  4. → **12.2.1.1.3** (as source) or **12.2.2.1.3** (as recipient).
- **Call modem, responding (12.7.1.2).** After Tone A has been detected for **more than 50 ms**:
  1. turn 106 OFF, clamp 104, stay silent **70 ± 5 ms**;
  2. send **Tone B**;
  3. listen for the **A reversal**;
  4. → 12.2.1.1.3 / 12.2.2.1.3.
- **Answer modem, initiating (12.7.2.1).**
  1. turn 106 OFF, clamp 104, stay silent **70 ± 5 ms**;
  2. send **Tone A** and listen for Tone B;
  3. → **12.2.2.2.3** (as source) or **12.2.1.2.3** (as recipient).
- **Answer modem, responding (12.7.2.2).** After Tone B has been detected for **more than 50 ms**:
  1. turn 106 OFF, clamp 104, stay silent **70 ± 5 ms**;
  2. send **Tone A**;
  3. → 12.2.2.2.3 / 12.2.1.2.3.
- See §F-V34 #11 for the inconsistency in the call-modem recipient case.

### C.8 Control-channel retrains (12.8, PDF p69-70; Fig 28)

**Initiating (12.8.1).**

1. Turn 106 OFF, send **AC**, and listen for PPh.
2. When PPh is detected:
   - clamp 104 to 1;
   - prepare for MPh;
   - send **PPh**, then **ALT for 16T to 120T**.
3. Then:
   - if the initiator is the **recipient** → 12.4.2.3;
   - if it is the **source** → send MPh, then → 12.4.1.3.
4. If **AC arrives from the far end while this side is sending AC**, this side becomes the
   responder (12.8.2). This is the collision rule.

**Responding (12.8.2).**

1. After detecting AC for **more than 100 ms**:
   - turn 106 OFF;
   - clamp 104 to 1;
   - send **PPh**, then ALT for **at least 16T** (the text says "should" here).
2. When the initiator's PPh arrives, the responder may train its equaliser on it.
3. Then:
   - if the responder is the **source**: prepare for MPh → 12.4.1.2;
   - if it is the **recipient**: after receiving PPh, send **MPh within 120T** (of ALT) → 12.4.2.4.

No timeout is specified for an initiator that never receives PPh (§F-V34 #12).

### C.9 Figures 23-28 transcribed as timelines

Each row is what one modem transmits, in order. `~x~` is silence. Arrows are the figure's
cause→effect lines between the rows.

**Figure 23: Phases 2 and 3, call modem is source (PDF p60, read at 300 dpi).**

    Call   : CM | CJ | ~75±5 ms~ | INFO0c | B ........ | B̄ 10 ms | L1 160 ms | L2 .... | B ....... | ~70±5 ms~ | S 128T | S̄ 16T | PP 288T | TRN
    Answer : JM | ~75±5 ms~ | INFO0a | A (50 ms) | Ā 10 ms | ~≤670 ms~ | A ...(25 ms)... | INFOh | ~silent~

Arrows:

1. End of CJ → the answer ends JM.
2. Start of the call's B (after INFO0c) → the answer detects B inside its A. The "50 ms" dimension
   runs from the start of A to the start of Ā.
3. Start of Ā → the call detects it inside B. "40 ± 10 ms" later the call starts B̄.
4. Start of B̄ → the answer detects it. The "≤ 670 ms" runs from that detection to the answer's
   next A. **(derived)** 10 + 160 + ≤ 500 = 670.
5. Start of the answer's A → the call stops L2 and starts B.
6. Start of that B → the answer detects it. "25 ms" later INFOh starts.
7. End of INFOh → the call stops B, then 70 ± 5 ms, then S.

Dimensions: 128T over S, 16T under S̄, 288T over PP, and a "≤ 2000 ms" bracket from PP start to TRN
end.

**Inset "Current ⇒ Correct".** Current shows S(128T) S̄(16T) PP(288T) TRN with the ≤ 2000 ms bracket
over PP+TRN. Correct is the same **without the ≤ 2000 ms bracket**. The PP+TRN limit is withdrawn,
which fits TRN lengths up to 4445 ms.

**Figure 24: Phases 2 and 3, answer modem is source (PDF p62, 300 dpi).**

    Call   : CM | CJ | ~75±5 ms~ | INFO0c | B (50 ms) | B̄ 10 ms | ~≤670 ms~ | B ...(25 ms)... | INFOh | ~silent~
    Answer : JM | ~75±5 ms~ | INFO0a | A ........ | Ā 10 ms | L1 160 ms | L2 .... | A ....... | ~70±5 ms~ | S 128T | S̄ 16T | PP 288T | TRN

Arrows:

1. End of CJ → end of JM.
2. Start of the answer's A → the call detects it inside B. The "50 ms" runs from **that detection**
   to B̄. Figure 23 draws it from the start of A, so the two figures differ (§F-V34 #17).
3. Start of B̄ → the answer detects it. "40 ± 10 ms" later, Ā.
4. Start of Ā → the call detects it. "≤ 670 ms" later the call starts B.
5. Start of the call's B → the answer stops L2 and starts A.
6. Start of that A → the call detects it. "25 ms" later, INFOh.
7. End of INFOh → the answer stops A, 70 ± 5 ms, S.

Dimensions and the "Current ⇒ Correct" inset are as in Fig 23: the ≤ 2000 ms bracket over PP+TRN
is removed.

**Figure 25: control-channel initial training, and restart requested by the source (PDF p64,
300 dpi).**

    Source    : Primary channel data | ~70±5 ms~ | PPh 32T | ALT ≥16T | MPh | MPh | E | Control channel data
    Recipient : ~silent~ ............... | PPh 32T | ALT ≥16T <120T | MPh | MPh | E | Control channel data

Circuits:

- Source: 106 OFF when primary data ends; 106 ON at the end of its own E; 104 and 109 change a
  little later, on receiving the far E.
- Recipient: 104 and 109 change (clamp/OFF) when the source's primary data ends; 106 ON at the end
  of its own E; 104 and 109 change again on receiving the source's E.

Arrows:

1. End of primary data → the recipient's 104 and 109.
2. Start of the source's PPh → the recipient detects it and sends its own PPh.
3. End of the recipient's PPh → the source has PPh. The "< 120T" runs from there to the source's
   first MPh.
4. The two middle MPh boxes cross: each side receives an MPh while sending MPh, then sends E.
5. The two Es cross: each side receives the other's E.

**Figure 26: control-channel restart requested by the recipient (PDF p65, 330 dpi).**

    Source    : Primary channel data | ~70±5 ms~ | Sh 24T | S̄h 8T | ALT | PPh 32T | ALT ≥16T <120T | MPh | MPh | E | Control channel data
    Recipient : ~silent~ ......................... | PPh 32T | ALT ≥16T ..................... | MPh | MPh | E | (control data)

Arrows:

1. End of primary data → the recipient's 104 and 109.
2. Trigger for the recipient's PPh:
   - **Current:** the start of Sh.
   - **Correct (inset):** a point inside S̄h, meaning after Sh followed by S̄h has been detected,
     which matches 12.6.2.3.
3. End of the recipient's PPh → the source abandons ALT and sends PPh (12.6.1.3).
4. End of the source's PPh → the recipient. The "< 120T" runs from there to the recipient's MPh.
5. MPh cross, then E cross.

Circuits as in Figure 25.

**Figure 27: control-channel resynchronisation, normal between pages (PDF p68, 330 dpi).**

    Source    : Primary channel data | ~70±5 ms~ | Sh 24T | S̄h 8T | ALT ≥16T | E | Control channel data
    Recipient : ~silent~ .......................... | Sh 24T | S̄h 8T | ALT ≥16T <120T | E | Control channel data

Arrows:

1. End of primary data → the recipient's 104 and 109.
2. Source's S̄h (Sh→S̄h detected) → the recipient starts Sh.
3. Recipient's S̄h → the source detects Sh→S̄h. The "< 120T" runs from there to the source's E.
4. Order of the two Es:
   - **Current:** the source's E starts the recipient's E, and the recipient's E then leads into the
     source's control data, one after the other.
   - **Correct (inset):** the two Es run side by side and cross at their ends.
   - In the corrected version each side turns 106 ON after its own E, and changes 104/109 after
     receiving the far E.

**Figure 28: control-channel retrains (PDF p70, 330 dpi).**

    Initiating : Control channel data | AC ......... | PPh 32T | ALT ≥16T <120T | MPh | MPh | E | Control channel data
    Responding : Control channel data | ~≥100 ms of AC detection~ | PPh 32T | ALT ≥16T | MPh | MPh | E | Control channel data

Circuits:

- Initiator: 106 OFF at the start of AC; 104 clamped when the responder's PPh is detected; 106 ON
  after its E; 104 on receiving the responder's E.
- Responder: 104 clamped and 106 OFF at the start of its PPh, after at least 100 ms of AC; 106 ON
  after its E; 104 after the far E.

Arrows:

1. Start of AC → the responder, which sends PPh once AC has lasted ≥ 100 ms.
2. Start of the responder's PPh → the initiator's 104.
3. End of the responder's PPh → the initiator sends PPh.
4. End of the initiator's PPh → the responder. The "< 120T" runs from there to the responder's MPh.
5. MPh cross, then E cross.

---

## D. T.30 (09/2005): clause 6, Annex F, Table 2 and the ECM pieces

### D.1 Clause 6: using V.34 (PDF p90-92 = printed p80-82)

- **6.1.**
  - **ECM is mandatory** for every fax message sent over V.34, half-duplex or duplex.
  - Follow Annex A (ECM) except where Annex C (duplex) or Annex F (half-duplex) says otherwise.
  - A terminal that supports **duplex must also support half-duplex**.
  - V.8 start-up is common to both modes; follow V.8 except as clause 6 says.
- **6.1.1.** An answering V.34 fax sends **ANSam until a valid CM arrives, or until an ANSam time-out
  of 2.6 to 4.0 s**. Plain V.8 (8.2.2) would run ANSam for 5 ± 1 s.
- **6.1.2.** A calling V.34 terminal answers ANSam with CM. **The call terminal picks the direction**
  of fax transfer, using the V.8 call-function codes of Table 4.
- **Table 4/T.30, the call-function category (read from PDF p91).**

| Start | b0 | b1 | b2 | b3 | b4 | b5 | b6 | b7 | Stop | Octet "callf0" |
|---|---|---|---|---|---|---|---|---|---|---|
| 0 | 1 | 0 | 0 | 0 | 0 | 0 | 0 | 1 | 1 | Transmit facsimile from call terminal |
| 0 | 1 | 0 | 0 | 0 | 0 | 1 | 0 | 1 | 1 | Receive facsimile at call terminal |

  - NOTE: duplex and half-duplex use the same codepoints.
  - **(derived)** With b0 as LSB, the octets are **0x81** (transmit) and **0xA1** (receive). They
    match V.8 Table 3 (section E).
- **6.1.3.** After a valid CM, follow V.8. If the **ANSam time-out expires**, the answer terminal
  switches to clause 5 binary signalling at 300 bit/s (V.21), with **DIS bit 6 = 1**.
- **6.1.4.** If a call terminal in 300 bit/s mode receives **DIS with bit 6 = 1**, it **may restart
  V.8 by sending CI**. An answer terminal that is waiting for a reply to its DIS and detects CI must
  re-enter V.8 by **sending ANSam again**. See Figs F.5-8 and F.5-9 (D.3) and T.30 Fig 6b.
- **6.1.5.** If CM/JM shows V.34 on both sides: **Annex C** for duplex, **Annex F** for half-duplex.
- **6.1.6.** If CM/JM does **not** show V.34 on both sides: **clause 5** (ordinary V.21 T.30). See
  Fig F.5-10 and section E.
- **6.1.7, manual mode after a telephone conversation.**
  - The terminal that **sends** the document takes the V.8/V.34 **call** role; the one that
    receives takes the **answer** role. This holds for the whole fax session, whoever dialled.
  - The sender listens for ANSam and sends CM. The receiver starts V.8 by sending ANSam.
  - See Fig F.5-14.
- **6.2 and Figure 11 (PDF p92).** The mode-selection flow:
  1. Supports V.8? No → clause 4. Yes → start V.8.
  2. V.34 modulation? No → clause 5.
  3. Duplex? **No → Annex F.** Yes → Annex C, then "Channel established?", looping back until yes.
- **6.2.1.** Codepoints for Extended Negotiations via V.8 exist, but the procedure is for further
  study.
- **Related (PDF p24, 4.1).**
  - 4.1.1: a non-V.8 answerer sends CED (2100 Hz ± 15 Hz, 2.6-4.0 s) after at least 0.2 s of
    silence, then waits 75 ± 20 ms.
  - 4.1.2: a V.8-capable answerer sends ANSam and follows clause 6.
- **Figs 6a/6b (PDF p22-23), operating methods 4 bis a / 4 bis b.**
  - Calling side: transmit CNG. On ANSam → transmit CM. Then JM duplex indication? **No → Annex F**;
    Yes → Annex C.
    - With no ANSam but DIS or V.21 detected: 4 bis a → phase B node T. 4 bis b → "V.8 capability
      in DIS?" Yes → transmit CI (and back to watching for ANSam); No → continue phase B node T.
  - Called side: answer, optional recorded announcement, send ANSam, CM detected? Yes → Annex F or C.
    No → a second decision:
    - In **6b** it is "CI detected?": Yes → send ANSam again; No → phase B node R.
    - In **6a** that diamond is labelled "CM detected?" (see §F-T30 #14).

### D.2 Annex F: V.34 half-duplex procedures (text PDF p174-176 = printed p164-166)

- **F.1.** Scope: optional use of V.34 half-duplex by G3 terminals under T.4 Annex A and T.30
  Annex A.
- **F.2.** References V.8 (2000) and V.34 (1998).
- **F.3.** ECM is mandatory. Follow Annex A except as below.

**F.3.1, General.**

- **F.3.1.1.** Use the V.8 start-up and **V.34 clause 12**, except where clause 6 or this annex
  says otherwise.
- **F.3.1.2.** Once ANSam has been received, the **source must transmit continuously**, to keep
  network echo suppressors disabled. The only gaps allowed are:
  - the silent periods of V.8 and V.34 during start-up;
  - the gaps between control-channel and primary-channel transmissions.

  After control-channel start-up, the **recipient may be silent only while it is receiving primary
  training or data**. In practice both sides send HDLC flags whenever they have nothing else to send.
- **F.3.1.3.** Where each kind of data goes:
  - binary-coded T.30 procedure frames go on the **control channel**;
  - **message data and the RCP command** go on the **primary channel**.
- **F.3.1.4.** After control-channel start-up (12.4), each terminal listens for HDLC frames and sends
  HDLC flags at the agreed control-channel rate.
  - **At least two flags** must precede the first frame after **any** start-up, resynchronisation
    or retrain.
  - The MPh exchange sets the control-channel rate.
  - NOTE: asymmetric rates (MPh bit 50) are for further study.
- **F.3.1.5.** If a terminal decides, by any means, that its receiver has lost sync with the far
  control-channel transmitter, it starts a **control-channel retrain (12.8)**.

**F.3.2, pre-message (phase B).**

- **F.3.2.1.**
  - **TCF is not used.** After DCS, the source sends control-channel flags while it waits for a
    valid response.
  - The recipient answers DCS with **CFR**. CFR means the whole pre-message procedure is complete
    and message transmission can start.
  - **FTT is not used.**
- **F.3.2.2.** After CFR, the recipient's modem sends **flags until it detects at least 40
  consecutive 1s**, then goes **silent**. While silent it is ready for the primary-channel resync
  signal and then data at the MPh-agreed rate.
- **F.3.2.3.** After receiving CFR, the source:
  1. sends **consecutive 1s until it detects silence, or the absence of flags, from the recipient
     AND it has sent at least 40 1s**;
  2. stays silent **70 ± 5 ms**;
  3. sends the **primary-channel resync signal** (V.34 12.5.1: S, S̄, PP, B1);
  4. sends the **T.4 A.3.1 synchronisation signal** (D.7);
  5. sends message data at the MPh-agreed rate.
- F.3.2.3 NOTE 1: machines **may restart T1 when V.8 completes**, to match Annex D.
- F.3.2.3 NOTE 2: **T2 is reset at the start of each new frame, not on flag detection.**
  **(derived)** Flag detection cannot be used because both sides send flags continuously.
- Fig F.5-1 NOTE (PDF p177): the string of 1s is **followed by the 4T of scrambled ones of
  V.34 12.6.3**, the control-channel turn-off.

**F.3.3, phase C.** Primary-channel retrain (12.7) during the message is for further study.

**F.3.4, post-message (phase D).**

- **F.3.4.1.** After message data and RCP, the source:
  1. runs the **primary-channel turn-off** (12.5.3);
  2. starts either **control-channel resync** (12.6, Sh/S̄h) or, **if it wants a new data rate**,
     **control-channel start-up** (12.4, PPh).

  Its receiver then expects:
  - after a resync: a resync response or a start-up response;
  - after a start-up: a start-up response.

  The start-up procedure is what allows the rate to be renegotiated, through MPh.
- **F.3.4.2.** After receiving the message and RCP, the recipient's modem listens for the
  control-channel resync signal.
  - To a **resync signal** it replies with a resync response, or with a start-up response if **it**
    wants a new rate (Fig 26).
  - To a **start-up signal** it replies with a start-up response.
- **F.3.4.3.** Once the control channel is back, the source sends the **post-message command**:
  PPS-xxx, or EOR-xxx after a fourth PPR, or DCN. The recipient answers with the **post-message
  response** (MCF / PPR / RNR / ERR ...).
- **F.3.4.4.** After the **last** post-message response between messages, the recipient's modem
  sends flags until it has seen **at least 40 consecutive 1s**, then goes silent and waits for the
  resync signal and data.
- **F.3.4.5.** After receiving that last response, the source:
  1. sends **1s** until silence or no flags is detected **and** at least forty 1s have been sent;
  2. stays silent **70 ± 5 ms**;
  3. sends the primary-channel resync signal;
  4. sends the T.4 A.3.1 sync;
  5. sends data.
- **F.3.4.5 NOTE 1.**
  - A data-rate change is possible at every start of the control channel (F.3.4.1-2).
  - **CTC/CTR frames are not used** in the V.34 ECM protocol.
  - The note's own wording for what replaces them: "EOR/ERR or DCN signals are used to transit".
- **F.3.4.5 NOTE 2.** Terminals may drop the line straight after DCN without sending 1s.
- **F.3.4.5 NOTE 3.** PIP, PIN and PRI-Q are for further study.

**F.4, F.5.** The half-duplex V.34/V.8 operating procedures are those of V.8 and V.34. F.5 gives
the example sequences in Figs F.5-1 to F.5-14.

### D.3 Figures F.5-1 to F.5-14 as sequences (PDF p177-192, 170-300 dpi)

Each line is what that terminal sends, in order. `~x~` = silence. `→` = the other side's reaction
shown by the figure's connecting lines.

- **Fig F.5-1, typical V.34 fax start-up (PDF p177).** The call terminal sends.
  - Call: CNG | CM | CJ | ~75±5~ | INFO0c | B | B̄ | L1 | L2 | B | ~70±5~ | S | S̄ | PP | TRN | ~70±5~ |
    PPh | ALT | MPh | MPh | E | Flags | TSI | DCS | Flags | "1" (Note) | ~70±5~ | S | S̄ | PP | B1 |
    Image Data
  - Answer: ANSam | JM | ~75±5~ | INFO0a | A | Ā | ~silent~ | A | INFOh | ~silent~ | PPh | ALT |
    MPh | MPh | E | NSF | CSI | DIS | Flags | CFR | Flags | ~silent~
  - Phase labels along the bottom: Network Interaction (to INFO0) | Line Probing (to INFOh) |
    Primary Channel Equalizer Training (S..TRN) | Modem Parameter Exchange (PPh..E) | T.30 Fax
    Handshaking (flags..the 1s) | Primary Channel Resync.
  - Links (300-dpi crop):
    - the answer's PPh starts once it detects the call's PPh;
    - the call detects the answer's PPh inside its ALT;
    - MPh cross;
    - DIS → TSI/DCS;
    - DCS → CFR;
    - CFR → the 1s;
    - the 1s ↔ the end of the answer's flags;
    - the call's S and B1 → detected by the answer.
  - NOTE: the string of 1s is followed by the 4T of scrambled ones of 12.6.3.
- **Fig F.5-2, between pages (PDF p178).**
  - Source: Image Data | ~ | Sh | S̄h | ALT | E | PPS-MPS | Flags | "1" | ~ | S | S̄ | PP | B1 |
    Image Data
  - Recipient: ~silent~ | Sh | S̄h | ALT | E | Flags | MCF | Flags | ~silent~
  - Links: Sh/S̄h → recipient Sh; recipient S̄h → source ALT; PPS-MPS → MCF; MCF → "1";
    "1" ↔ end of the recipient's flags.
- **Fig F.5-3, end of communication (PDF p179).**
  - Source: Image Data | ~ | Sh | S̄h | ALT | E | PPS-EOP | Flags | DCN | "1" (Note) | ~ | Line
    Disconnect
  - Recipient: Sh | S̄h | ALT | E | Flags | MCF | Flags | ~ | Line Disconnect
  - NOTE: some terminals drop the line straight after DCN without the 1s.
- **Fig F.5-4, mode change without a rate change (PDF p180).**
  - Source: Image Data | ~ | Sh | S̄h | ALT | E | PPS-EOM | Flags ........ | TSI | DCS | Flags | "1" |
    ~ | S | S̄ | PP | B1 | Image Data
  - Recipient: Sh | S̄h | ALT | E | Flags | MCF | Flags (**"T2 elapsed"**, from the end of MCF to
    NSF) | NSF | CSI | DIS | Flags | CFR | Flags
- **Fig F.5-5, mode change with a rate change started by the source (PDF p181).**
  - Source: Image Data | ~ | **PPh | ALT | MPh | MPh | E** | PPS-EOM | Flags ... | TSI | DCS | Flags |
    "1" | ~ | S | S̄ | PP | B1 | Image Data
  - Recipient: PPh | ALT | MPh | MPh | E | Flags | MCF | Flags (**"T2 elapsed"**) | NSF | CSI | DIS |
    Flags | CFR | Flags
- **Fig F.5-6, rate change between partial pages (PDF p182).** The recipient asks for it (V.34
  Fig 26).
  - Source: Image Data | ~ | Sh | S̄h | ALT | **PPh | ALT | MPh | MPh | E** | PPS-NULL | Flags | "1" |
    ~ | S | S̄ | PP | B1 | Image Data
  - Recipient: **PPh | ALT** (long) | MPh | MPh | E | Flags | **PPR** | Flags
  - Links:
    - source Sh/S̄h → recipient PPh;
    - recipient PPh end → source PPh;
    - source PPh end → recipient MPh;
    - MPh cross;
    - PPS-NULL → PPR;
    - PPR → "1".
  - After the PPR the source resyncs and sends image data again (the retransmission).
- **Fig F.5-7, command retransmission (PDF p183).**
  - Source: Image Data | ~ | Sh | S̄h | ALT | E | PPS-MPS | Flags (**"T4 elapsed"**) | PPS-MPS | Flags |
    "1" | ~ | S | S̄ | PP | B1 | Image Data
  - Recipient: Sh | S̄h | ALT | E | Flags | MCF (**lost, drawn with an X**) | Flags | MCF | Flags
- **Fig F.5-8, manual sending (PDF p184).**
  - Answer: ANSam | ~**75 ± 20 ms**~ | DIS (Note) | ~ | DIS (Note) | ~ | ANSam | JM | → Line Probing
  - Call (idle until an operator "Start"): CNG | ~ | CI | ~ | CM | CJ | → Line Probing
  - NOTE: DIS bit 6 = 1.
  - Links:
    - the start of CI lines up with the end of the second DIS;
    - the end of CI leads to the answer's new ANSam;
    - CM is detected inside ANSam, and JM follows;
    - JM is detected inside CM, and CJ follows;
    - the end of CJ is the end of JM.
- **Fig F.5-9, manual receiving (PDF p185).**
  - Call: CNG | ~ | CNG | ~ | CNG | ~ | CI | ~ | CM | CJ | → Line Probing
  - Answer (idle until an operator "Start"): ANSam (**dotted box: optional**) | ~**75 ± 20 ms**~ |
    DIS (Note) | ~ | ANSam | JM | → Line Probing
  - NOTE: DIS bit 6 = 1.
- **Fig F.5-10, normal T.30 after V.8, JM without V.34 (PDF p186).**
  - Call: CNG | CM | CJ | ~ | (TSI) DCS (Note) | Training and TCF | ~ | Image Data (e.g. V.17)
  - Answer: ANSam | JM | ~**75 ± 5 ms**~ | (NSF) (CSI) DIS (Note) | ~ | CFR (Note) | ~
  - NOTE: V.21 modulation.
  - Spans: "Rec. V.8" = ANSam..JM; "Normal T.30 procedure" starts 75 ± 5 ms after JM ends.
- **Fig F.5-11a, turnaround polling: the call terminal sends, then receives (PDF p187).**
  - Call (source): Image Data | ~70±5~ | Sh | S̄h | ALT | E | PPS-EOM | Flags ... | CIG | DTC | Flags |
    ~**70±5 ms**~ | **CM (RX FAX)** | CJ | → F.5-11b
  - Answer (recipient): Sh | S̄h | ALT | E | Flags | MCF | Flags (**"T2 elapsed"**) | NSF | CSI | DIS |
    Flags | **"1"** | ~ | **JM** | → F.5-11b
  - The answer, the future source, sends the 1s. The call's flags stop when it sees them.
  - There is **no ANSam before this CM**.
- **Fig F.5-11b, continued (PDF p188).** The answer is now the source.
  - Call: ~75±5~ | INFO0c | B | B̄ | ~ | B | INFOh | ~ | PPh | ALT | MPh | MPh | E | Flags | CFR |
    Flags | ~
  - Answer: ~75±5~ | INFO0a | A | Ā | L1 | L2 | A | ~70±5~ | S | S̄ | PP | TRN | ~70±5~ | PPh | ALT |
    MPh | MPh | E | **TSI | DCS** | Flags | "1" | ~70±5~ | S | S̄ | PP | B1 | Image Data
  - No DIS here: the call terminal's DTC has already been sent.
- **Fig F.5-12a, turnaround polling: the call terminal receives, then sends (PDF p189).**
  - Answer (source): Image Data | ~70±5~ | Sh | S̄h | ALT | E | PPS-EOM | Flags ... | CIG | DTC |
    Flags | ~ | JM | → F.5-12b
  - Call (recipient): Sh | S̄h | ALT | E | Flags | MCF | Flags (**"T2 elapsed"**) | NSF | CSI | DIS |
    Flags | "1" | ~70±5~ | **CM (TX FAX)** | CJ | → F.5-12b
- **Fig F.5-12b, continued (PDF p190).** The call is now the source.
  - Call: ~75±5~ | INFO0c | B | B̄ | L1 | L2 | B | ~70±5~ | S | S̄ | PP | TRN | ~70±5~ | PPh | ALT |
    MPh | MPh | E | TSI | DCS | Flags | "1" | ~70±5~ | S | S̄ | PP | B1 | Image Data
  - Answer: ~75±5~ | INFO0a | A | Ā | ~ | A | INFOh | ~ | PPh | ALT | MPh | MPh | E | Flags | CFR |
    Flags | ~
- **Fig F.5-13, polling sequence (PDF p191).** The call terminal polls with CM set to RX FAX
  (Note); the answer is the source.
  - Call: CNG | CM (Note) | CJ | ~75±5~ | INFO0c | [box without a label, where B belongs] | B̄ | ~ |
    B | INFOh | ~ | PPh | ALT | MPh | MPh | E | **"MPh Flags"** (sic) | CIG | DTC | Flags | CFR |
    Flags | ~
  - Answer: ANSam | JM | ~75±5~ | INFO0a | A | Ā | L1 | L2 | A | ~70±5~ | S | S̄ | PP | TRN |
    ~70±5~ | PPh | ALT | MPh | MPh | E | NSF | CSI | DIS | Flags | TSI | DCS | Flags | "1" | ~70±5~ |
    S | S̄ | PP | B1 | Image Data
- **Fig F.5-14, manual communication after telephony (PDF p192).**
  - Receiving terminal (made the answer terminal; hooked up and dialled): Telephony (dashed) | ANSam |
    JM | ~75±5~ | INFO0a | A | Ā | ~ | A | INFOh | ~ | PPh | ALT | MPh | MPh | E | NSF | CSI | DIS |
    Flags | CFR | Flags | ~
  - Sending terminal (made the call terminal): Telephony (dashed) | CM | CJ | ~75±5~ | INFO0c | B |
    B̄ | L1 | L2 | B | ~70±5~ | S | S̄ | PP | TRN | ~70±5~ | PPh | ALT | MPh | MPh | E | Flags | TSI |
    DCS | Flags | "1" | ~70±5~ | S | S̄ | PP | B1 | Image Data

What the figures leave implicit **(derived)**:

- The F.3.1.4 minimum of two flags before a frame is drawn only as a Flags box, or not at all
  ("E | PPS-MPS").
- The 12.5.3.1 35 ms of scrambled ones and the 12.6.3 4T of ones are not drawn.
- RCP is inside "Image Data".
- The T.4 A.3.1 flags after B1 are inside "Image Data".

### D.4 Table 2/T.30: DIS/DTC/DCS bits that matter for V.34 (read from PDF p62-64, p66; notes p69-71, p74)

| Bit | DIS/DTC | Note | DCS | Note |
|---|---|---|---|---|
| 6 | **V.8 capabilities** | 23 | Invalid | 24 |
| 7 | **0 = 256 octets preferred, 1 = 64 octets preferred** | 23, 42 | Invalid | 24 |
| 9 | Ready to transmit a facsimile document (polling) | 18 | Set to 0 | none |
| 10 | Receiver fax operation | 19 | Receiver fax operation | 20 |
| 11-14 | Data signalling rate (table below) | none | Data signalling rate (table below) | **33** on the first row |
| 21-23 | Minimum scan line time capability at the receiver | 4, 8, 23 | Minimum scan line time | 8, 24 |
| 27 | **Error correction mode** | 17 | **Error correction mode** | 17 |
| 28 | Set to 0 | none | **Frame size: 0 = 256 octets, 1 = 64 octets** | **7, 24** |
| 31 | T.6 coding capability | 9, 17 | T.6 coding enabled | 9, 17 |
| 67 | Duplex and half-duplex capabilities: **0 = half-duplex operation only**, 1 = duplex and half-duplex | none | 0 = half-duplex operation only, 1 = duplex operation | none |

**Bits 11-14** (DIS/DTC meaning | DCS meaning):

| 11 12 13 14 | DIS/DTC | Note | DCS | Note |
|---|---|---|---|---|
| 0 0 0 0 | V.27 ter fall-back mode | none | 2400 bit/s, V.27 ter | 33 |
| 0 1 0 0 | V.27 ter | 3 | 4800 bit/s, V.27 ter | none |
| 1 0 0 0 | V.29 | none | 9600 bit/s, V.29 | none |
| 1 1 0 0 | V.27 ter and V.29 | none | 7200 bit/s, V.29 | none |
| 0 0 1 0 | Not used | none | Invalid | 31 |
| 0 1 1 0 | Reserved | none | Invalid | 31 |
| 1 0 1 0 | Not used | none | Reserved | none |
| 1 1 1 0 | Invalid | 32 | Reserved | none |
| 0 0 0 1 | Not used | none | 14 400 bit/s, V.17 | none |
| 0 1 0 1 | Reserved | none | 12 000 bit/s, V.17 | none |
| 1 0 0 1 | Not used | none | 9600 bit/s, V.17 | none |
| 1 1 0 1 | V.27 ter, V.29 and V.17 | 31 | 7200 bit/s, V.17 | none |
| 0 0 1 1 | Not used | none | Reserved | none |
| 0 1 1 1 | Reserved | none | Reserved | none |
| 1 0 1 1 | Not used | none | Reserved | none |
| 1 1 1 1 | Reserved | none | Reserved | none |

Notes (paraphrased; the numbers are Table 2 note numbers):

- **Note 33.** When **V.34 is used**, or DCS bit 123 (Internet-aware fax) is 1, **DCS bits 11-14 are
  invalid and should be 0**. Nothing is said about DIS/DTC bits 11-14 under V.34.
- **Note 23 (Annex C only).** DIS/DTC bits 6 and 7 = 0; bits 21-23 and 27 = 1.
- **Note 24 (Annex C only).** DCS bits 6, 7 and 28 = 0; bits 21-23 and 27 = 1.
- Annex C is the ISDN / **duplex-GSTN** annex (title on PDF p118). **No note fixes bits 6, 7 or 28
  for Annex F** (§F-T30 #1).
- **Note 7.** DCS bit 28 is valid only when bit 27 invokes ECM.
- **Note 8.** ECM needs 0 ms minimum scan-line time. With ECM the sender sends DCS bits 21-23 =
  1, 1, 1 (0 ms). DIS/DTC 21-23 report the receiver's real capability anyway.
- **Note 17.** If any of bits 31, 36, 38, 51, 53, 54, 55, 57, 59, 60, 62, 65, 68, 78, 79, 115, 116 or
  127 is 1, or bits 92-94 are non-zero, then **bit 27 must be 1**.
- **Note 42.** For backward compatibility a transmitter may ignore a request for 64-octet frames, so
  a receiver must cope with 256-octet frames.
- **Note 5.** The standard FIF is 24 bits. Each "extend field" bit (24, 32, 40, ...) set to 1 adds 8
  bits.
- **Bit 67** has no note. It reads as the Annex C duplex/half-duplex selector. A V.34 half-duplex
  terminal would naturally send 0; nothing in Annex F says so.

### D.5 ECM under V.34 half-duplex: frame size, RCP, PPS/PPR/EOR, CTC

- **Frame size (T.30 A.1, A.3; PDF p93).**
  - The transmitter picks **256 or 64 octets** in DCS (bit 28). The receiver must accept both and
    may state a preference in DIS/DTC (bit 7).
  - The sizes exclude the FCF and frame-number octets, so the HDLC information field is 258 or
    66 octets.
  - Block = up to 256 frames. The last block of a page may be short.
  - The frame size may change only at a page boundary, signalled by PPS-EOM or EOR-EOM.
  - **Nothing V.34-specific** is added. Annex C's note 24 forces 256 octets for duplex only.
- **FCD and RCP frames (T.4 Annex A; PDF p27-29 = printed p19-21).**
  - Address 1111 1111. Control **1100 X000 with X = 0** for both FCD and RCP.
  - FCF: **FCD = 0110 0000**, **RCP = 0110 0001**. Bits are sent MSB-first as printed, except the
    frame number, which is LSB-first.
  - FCD FIF = 8-bit frame number + a 256- or 64-octet data field, so 257 or 65 octets.
  - **RCP has no information field.**
  - A partial page ends with **three consecutive RCP frames**. The flag sequence after the last RCP
    must be **shorter than 50 ms** (A.3.8 NOTE).
  - Figs A.1/A.2 (PDF p27-28) show the layout: Synchronization | FCD frames 0..n−1 | RCP | RCP | RCP |
    ≤ 50 ms of flags | end of phase C.
- **RCP in V.34.**
  - T.30 5.3.2.1 (PDF p55): with V.27ter/V.29/V.17, delineation is RTC plus RCP. **With V.34, the
    delineation is as defined in Annex F.**
  - Annex F puts **message data and RCP on the primary channel** (F.3.1.3), then the primary
    turn-off (F.3.4.1).
  - 5.3.2.1 NOTE: a receiver that decodes at least one RCP correctly may start post-message command
    reception.
  - 5.3.2.1 also says that in duplex mode RCP is not used and the FCF delineates instead. That is
    Annex C.
- **PPS / PPR / EOR / ERR / RR / RNR (T.30 A.4; PDF p94-100).**
  - These are unchanged; they are sent on the control channel. PPR carries its 256-bit map.
  - Fig F.5-6 shows PPS-NULL answered by PPR, followed by the retransmission.
- **CTC/CTR.**
  - Normally, after the **4th PPR for a block**, the sender sends **EOR** (stop correcting) or **CTC**
    (keep correcting, possibly at a lower speed) (A.1).
  - **In V.34 mode CTC/CTR are not used.** Per the F.3.4.5 NOTE 1 wording quoted in D.2, EOR/ERR or
    DCN are used instead.
  - The speed-change job CTC did is taken over by the MPh renegotiation available at every
    control-channel start (F.3.4.1-2, Figs F.5-5/6).
  - Whether a sender may simply keep retransmitting after the 4th PPR, with no CTC, is unclear
    (§F-T30 #5).
- **PIP / PIN / PRI-Q.** For further study under V.34 (F.3.4.5 NOTE 3).

### D.6 Timers

- **T0.** 60 ± 5 s (up to 120 s) for the answer. Not V.34-specific.
- **T1 (5.4.3.1, PDF p90).** 35 ± 5 s. Starts on entering phase B and is reset on a valid signal.
  - Operating methods 3/4: start on receiving V.21. Method 4 bis a: start on starting V.21
    transmission.
  - **Annex F NOTE 1:** optionally restart T1 when V.8 completes.
- **T2.** 6 ± 1 s. Starts when a command search begins; normally reset by a received HDLC flag.
  - **Annex F NOTE 2: in V.34 half-duplex, T2 is reset at the start of each new frame instead.**
  - T2 also appears in Figs F.5-4, 5, 11a and 12a as the wait between MCF (for EOM) and the
    recipient's new NSF/CSI/DIS.
- **T3.** 10 ± 5 s. Unchanged.
- **T4.** 3.0 s ± 15% for automatic units; 3.0 or 4.5 s ± 15% for manual units (flow-diagram note,
  5.2). This is the command-repeat timer: Fig F.5-7 repeats PPS-MPS after "T4 elapsed".
  - 5.4.2: after 3 failed attempts, send DCN.
- **T5.** 60 ± 5 s. Unchanged (RNR busy).
- **Response time.** 5.4.2 says a terminal must respond within **1.5 s** of a signal received in
  binary-coded (V.21) or V.27ter/V.29/V.17 modulation. V.34 is not named (§F-T30 #6).
- **Frame-length limit (5.4.2 NOTE 1).** 3 s ± 15%: transmit no frame over 2.55 s; discard any
  received frame over 3.45 s. Not V.34-specific.

### D.7 "The synchronization signal defined in A.3.1/T.4" (T.4 PDF p27 = printed p19)

- **A.3.1.** Whenever a new transmission begins, a synchronisation sequence precedes all
  binary-coded information. It is **a training sequence and a series of flags for nominal 200 ms,
  tolerance +100 ms**. The NOTE shows continuous flags as ...0111 1110 0111 1110...
- **A.3.2.** Use this synchronisation before the first frame. Later frames need at least one flag
  between them, and a frame's leading flag may be the previous frame's trailing flag.
- **In V.34 half-duplex (F.3.2.3, F.3.4.5)** the source sends, in order:
  1. 70 ± 5 ms of silence;
  2. the V.34 primary resync signal (S 128T, S̄ 16T, PP, B1);
  3. the A.3.1 synchronisation;
  4. the FCD frames.
- **(derived, reading)** The V.34 resync already is the training. The A.3.1 part that adds
  anything is **200 ms (up to 300 ms) of HDLC flags at the primary data rate** after B1. For
  example, 6720 flag bits at 33.6 kbit/s. See §F-T30 #4.

---

## E. V.8 as used by a V.34 fax

**Coding (V.8 cl. 5, Table 1; PDF p9 = printed p4).**

- CI, CM and JM share one format: a repeating sequence of **10 ones**, then **10 sync bits**, then
  octets. Each octet is start bit (0), b0..b7, stop bit (1).

| Preamble part | Bits (in transmission order) |
|---|---|
| Ten ones before each sequence | 1 1 1 1 1 1 1 1 1 1 |
| CI sync | 0 0 0 0 0 0 0 0 0 1 |
| **CM and JM sync** | **0 0 0 0 0 0 1 1 1 1** |
| V.92 use | 0 1 0 1 0 1 0 1 0 1 |

- **Category octet:** b0-b3 = tag (b0 is the LSB), **b4 = 0**, b5-b7 = options.
- **Extension octet:** b0-b2 options, **b3 = 0, b4 = 1, b5 = 0**, b6-b7 options. This coding stops
  JM from containing an HDLC flag.
- **CJ** (3.5) = **three octets of all zeros**, each with its start and stop bits, sent in V.21(L).
- **Modulation.** CI and CM use V.21(L), the low channel, at 300 bit/s. JM uses V.21(H), the high
  channel.

**Table 2/V.8 category tags (PDF p10).** Transcribed as b0 b1 b2 b3:

| b0 b1 b2 b3 | Category |
|---|---|
| 1 0 0 0 | Call function |
| 1 0 1 0 | Modulation modes |
| 0 1 0 1 | Protocols |
| 1 0 1 1 | PSTN access |
| 1 1 1 1 | Non-standard facilities |
| 1 1 1 0 | PCM modem availability |
| 0 1 1 1 | Defined in T.66 |

**Table 3/V.8 call functions (PDF p10).** Tag 1000, then b5 b6 b7:

| b5 b6 b7 | Call function |
|---|---|
| 0 0 0 | To be determined by ITU-T |
| 1 0 0 | H.324 |
| 0 1 0 | V.18 textphone |
| 1 1 0 | T.101 videotext |
| **0 0 1** | **Transmit facsimile from call terminal (T.30)** |
| **1 0 1** | **Receive facsimile at call terminal (T.30)** |
| 0 1 1 | Data |
| 1 1 1 | In an extension octet |

This agrees with T.30 Table 4.

**Table 4/V.8 modulation modes (PDF p11).** The extracted text misaligns the Item column.

| Octet | Bit | Meaning | Item |
|---|---|---|---|
| **modn0** (category: tag b0-b3 = 1 0 1 0, b4 = 0) | b5 | PCM availability category present | 0 |
| | **b6** | **V.34 duplex** | 1 |
| | **b7** | **V.34 half-duplex** | 2 |
| **modn1** (extension: b3 b4 b5 = 0 1 0) | b0 | V.32 bis/V.32 | 3 |
| | b1 | V.22 bis/V.22 | 4 |
| | b2 | V.17 | 5 |
| | b6 | V.29 half-duplex | 6 |
| | b7 | V.27 ter | 7 |
| **modn2** (extension: b3 b4 b5 = 0 1 0) | b0 | V.26 ter | 8 |
| | b1 | V.26 bis | 9 |
| | b2 | V.23 duplex | 10 |
| | b6 | V.23 half-duplex | 11 |
| | b7 | V.21 | 12 |

**(derived) Example octets, b0 = LSB.**

| Octet | Value | Contents |
|---|---|---|
| callf0 | 0x81 | fax transmit |
| callf0 | 0xA1 | fax receive |
| modn0 | **0x85** | V.34 half-duplex only |
| modn0 | 0xC5 | V.34 duplex + half-duplex |
| modn1 | 0xD4 | V.17 + V.29 + V.27ter |
| modn2 | 0x90 | V.21 only |

**CI (7.1, PDF p14).**

- CI carries the call function.
- ON periods: **at least 3 CI sequences and at most 2 s**. OFF periods: **0.4 to 2 s**.
- CI is optional. Fax usually sends **CNG** instead: Figs F.5-1/13 show CNG, and 6.1.4 uses CI only
  to restart V.8 after DIS bit 6 = 1.

**ANSam (7.2, PDF p14).**

- **2100 ± 1 Hz**, amplitude-modulated by **15 ± 0.1 Hz** between (0.8 ± 0.01) and (1.2 ± 0.01) times
  the mean.
- **Phase reversals every 450 ± 25 ms**. They are left out when network echo-canceller disabling is
  not needed. V.34 11.1.2.1 makes them **optional for half-duplex**. NOTE 2 warns that some echo
  cancellers may fail to connect without them.
- Power per V.2. Out-of-band power at least 24 dB below the in-band power (2100 ± 200 Hz).
- A call DCE must not send CM unless it has detected ANSam.

**Call DCE sequence (8.1, Fig 1; PDF p16-17).**

1. 1 s of silence, then CI, CT, CNG or nothing.
2. On ANS or ANSam, stop the call signal. The call DCE may make sure CI ran for at least 3 full
   sequences.
3. After ANSam: silence **Te ≥ 0.5 s**, or **≥ 1 s** if V.25-style echo-canceller disabling is
   wanted.
   - Te starts when the call signal ends, or at ANSam detection if there was no call signal.
4. Send CM repeatedly and listen for JM.
5. After **2 identical JM**: finish the current octet (with its start and stop bits), send **CJ**,
   stay silent **75 ± 5 ms**, then sigC (for V.34 that is INFO0c).
6. If JM shows no common modulation and no PCM, the call DCE may disconnect after CJ.

**Answer DCE sequence (8.2; PDF p17-18).**

1. At least **0.2 s** of silence after going off-hook.
2. ANSam. Plain V.8 runs it **5 ± 1 s** if not ended early; **T.30 6.1.1 cuts this to 2.6-4.0 s**.
3. After **2 identical CM**: send JM. Keep sending until CJ is detected, **all 3 octets**. Other
   cut-offs are allowed if CJ is missed (sigC detected, or CM absent for long enough).
4. JM stops **without finishing the current sequence**.
5. Silence **75 ± 5 ms**, then sigA (INFO0a).
6. If neither CM nor sigC arrives during ANSam: silence 75 ± 5 ms, then V.32bis Annex A, T.30,
   etc. For fax that means V.21 DIS with bit 6 = 1 (T.30 6.1.3).

**What JM contains (7.4, 8.2.3; PDF p15, p17).**

- The first category is the call function.
  - It must be the **same** as in CM if the answerer supports it.
  - Otherwise JM may name a different call function. In that case JM carries the same number of
    modulation octets as CM, all zero.
- JM lists **every modulation mode that appears in CM and that the answer DCE can use with that call
  function**. It may also include other modulation octets from CM.
- If there is no common mode: the same number of modulation octets as CM, all zero.
- The PCM category appears only if it was in CM (never, for fax).
- **Selection rule:** use the listed mode with the **lowest item number**. V.34 duplex (item 1) beats
  V.34 half-duplex (item 2), which beats V.17 (item 5), and so on.
  - **(derived)** So a fax that wants half-duplex must not advertise V.34 duplex in its CM/JM, or
    must accept duplex. T.30 Fig 6a branches the call side on "JM duplex indication?".

**JM without V.34 (T.30 6.1.6, App. VII, Fig F.5-10).**

- Use clause 5.
- **App. VII.2.3 (PDF p318).** If the agreed call function is fax and the best common modulation is
  V.17, V.29 or V.27ter:
  - after V.8, the **answer** modem prepares its **transmitter for V.21 channel 2** and the **call**
    modem its receiver for the same;
  - then comes clause 5 (DIS ...). The modulation bits must **not** be taken literally as a V.17
    sigA.
- Fig F.5-10 puts (NSF)(CSI)DIS in V.21 **75 ± 5 ms** after JM ends.

**DIS bit 6 = 1 (T.30 6.1.3-6.1.4).**

- An answerer that timed out on ANSam sends V.21 DIS with bit 6 = 1.
- A V.8-capable caller in 300 bit/s mode may answer it with **CI**. The answerer, on detecting CI,
  **sends ANSam again** and V.8 restarts (Figs F.5-8, F.5-9; Fig 6b).
- Figs F.5-8/9 show **75 ± 20 ms** between ANSam and the DIS, the T.30 4.1.1 CED rule. V.8 and V.34
  use 75 ± 5 ms after ANSam when no CM arrives (§F-T30 #12).

---

## F. Open questions, ambiguities, and where the text and the rendered page disagree

### F-V34. V.34: undefined or ambiguous

1. **T means two different things in clause 12.**
   - Primary-channel T: S 128T, S̄ 16T, PP 288T.
   - Control-channel T at 600 Bd: PPh 32T, ALT 16T..120T, MPh within 120T, Sh 24T, S̄h 8T, the 4T of
     ones.
   - The text never says which is which. The figures and the PPh definition (32 symbols = 32T) settle
     it.
2. **PPh (eq. 10-2) does not depend on I.** As printed, the sequence is `p p p p n n n n` × 4, a
   diagonal BPSK square wave (A.4). That is a poor equaliser-training sequence, although 12.4 uses
   PPh to train the control-channel equaliser.
   - The glyphs at 800 dpi are digit 1s, not capital I. So the printed formula is what it says, and
     no alternative reading is available from this PDF.
   - The square brackets could be read as integer-part. That gives the same pattern rotated 45° onto
     the real axis: phases 0, 0, π, π per k.
   - **Confirm against a real fax capture before trusting it.** Implement it as printed, but be ready
     to swap the table.
3. **ALT has two unstated details.** Which bit comes first, 0 or 1, is not given. Neither is the
   initial Zn−1 for the control-channel differential encoder. Duplex J/MP initialise from the last
   TRN symbol, but that rule is duplex-only.
   - **(derived)** A differential receiver loses at most the first symbol, and the self-synchronising
     descrambler loses the first 23 bits. So this matters for bit-exact TX tests, not for
     interworking.
4. **Differential encoding of control-channel user data (the HDLC flags and frames after E) is never
   stated.** It is stated only for ALT, E and MPh. 10.2.4 gives a formula for when differential
   encoding is off, which implies a switch. Continuing with it enabled is the natural reading.
5. **Relative scaling is not given for:**
   - the 1200 bit/s (|point|² = 2) and 2400 bit/s (mean 10) control-channel constellations;
   - AC, Sh and S̄h (point 0);
   - PPh (unit magnitude).

   The spec gives only the answer/call output levels: −1 dB carrier plus −7 dB guard, or nominal.
6. **Tone A's guard tone is at nominal power, with Tone A at −1 dB (10.1.2.1).** INFO and the
   control channel put the guard at −7 dB. This is inherited from duplex, so reuse whatever the
   duplex implementation already does.
7. **L2 length.** 10.1.2.4 caps L2 at 550 ms plus a round-trip delay, but a half-duplex source
   measures no RTD.
   - The recipient takes at most 500 ms of L2 (12.2.x.x.5) and must answer within 670 ms of the
     reversal (Figs 23/24).
   - The source stops L2 when it detects the far tone, or after the 2700 ms time-out
     (12.2.1.3.3 / 12.2.2.4.3).
8. **The reversal-response tolerance is looser in half-duplex:** 40 ± 10 ms against 40 ± 1 ms in
   duplex 11.2. There is no round-trip estimate in half-duplex.
9. **Some Phase 2 waits have no timeout.**
   - A missing reversal (12.2.1.3.2, 12.2.2.4.2) → keep sending the tone forever.
   - The recipient's 2000 ms "no far tone" rule (12.2.1.4.3, 12.2.2.3.3) sends INFOh **blind**.
10. **12.3.3's 2000 ms limit for detecting S** gives no start point. The natural one is the end of
    INFOh. What counts as TRN being received well enough is left to the implementer.
11. **12.7.1.1 / 12.7.1.2, call modem retraining as recipient.** The text says to listen for a Tone
    A **reversal** and then continue at 12.2.2.1.3. But 12.2.2.1.3 has the call modem send B̄ after
    detecting plain Tone A plus ≥ 50 ms of B, and only then wait for the A reversal. So the "detect A
    reversal first" instruction fits only the source case.
12. **12.8.1 has no timeout** for "sent AC, never got PPh".
13. **12.5.3.2 trigger.** The recipient leaves primary mode on **105 OFF→ON** (its DTE asks to
    send), or on the level-drop rule. How the recipient's DTE knows the message has ended is left to
    T.30: after RCP (F.3.4.2).
14. **12.5.3.1's 35 ms of scrambled ones** equals a data frame only at J = 8 (2743 / 3429 Bd); at
    J = 7 a data frame is 40 ms. The text does not say whether to finish the current mapping frame or
    data frame first.
15. **Fig 26 against the text.**
    - The source's post-PPh ALT is labelled "≥ 16T < 120T". 12.4.1.2 measures its 120T from
      *receiving* PPh.
    - The recipient's ALT is drawn "≥ 16T" with no upper bound. 12.4.2.3 caps it at 120T, but by
      12.6.2.3 the recipient's ALT must also wait for the source's PPh.
    - Treat the 120T as "from receiving the far PPh to own MPh".
16. **MPh rate selection** takes the highest enabled rate at or below both MPh maxima. It does not
    say what happens if the two capability masks share no such rate. Nor does MPh have MP's rule that
    repeated sequences in a group should be identical; ideally they should be.
17. **Figs 23/24 "50 ms".** Fig 23 draws it from the start of the tone; Fig 24 draws it from the
    far-tone detection. The text needs both: the far tone detected **and** own tone ≥ 50 ms.
18. **INFOh bit 22 at 3429 Bd** makes no difference: low and high carriers are both 1959 Hz
    (Table 2).
19. **Precoder coefficients** are meaningful only in the recipient's MPh. The source should send
    Type 0. A control-channel *resync* (12.6) exchanges no MPh, so the rate, precoder and data-mode
    parameters carry over. Only a start-up (12.4) or retrain (12.8) can change them.

### F-T30. T.30, V.8 and T.4: undefined or ambiguous

1. **DIS bit 6, DIS bit 7 and DCS bit 28 in an Annex F call.** Notes 23 and 24 fix them only for
   Annex C (duplex).
   - For half-duplex, bit 7 (the 256/64 preference) and DCS bit 28 (the frame size) work normally.
   - Whether DIS bit 6 should be 1 in a DIS sent *over the V.34 control channel* is not stated. It
     is meaningful only in the V.21 fallback of 6.1.3.
2. **DIS/DTC bits 11-14 under V.34** are not addressed. Note 33 covers **DCS** only: set to 0.
   Presumably they still advertise V.27ter/V.29/V.17 for fallback.
3. **Bit 67 (duplex/half-duplex)** is not mentioned by Annex F. Send 0.
4. **The T.4 A.3.1 synchronisation signal.** A.3.1 describes it as training plus flags, nominally
   200 ms, tolerance +100 ms. Annex F puts it **after** the V.34 resync signal. The sensible reading
   is that the resync is the training and the flags for 200 to 300 ms are what remains. Not stated.
5. **CTC/CTR are banned** (F.3.4.5 NOTE 1). The note's awkward wording for the replacement is quoted
   in D.2. It is unclear whether a sender may make a **5th and later** retransmission without CTC.
   - Presumably yes, with an optional rate change through the control-channel start-up.
   - Fig F.5-6 shows a recipient-driven rate change around PPR.
6. **T.30 5.4.2's 1.5 s response rule** names V.21 and V.27ter/V.29/V.17 modulation, but not V.34.
7. **T.30 5.3.2.2** requires 75 ± 20 ms after RTC/RCP before binary-coded signalling. Under V.34,
   5.3.2.1 defers to Annex F, where the gap is the 35 ms of ones plus 70 ± 5 ms of silence
   (12.5.3.1, 12.6.1.1). The conflict is not addressed explicitly.
8. **The "40 ones" rule.**
   - The recipient detects 40 consecutive 1s, presumably in the descrambled control-channel bits.
     HDLC data cannot contain more than six 1s in a row, so a run of 40 is unambiguous.
   - The source stops on silence **or** on absence of flags. Those are two different detectors, and
     both are allowed.
   - With a round trip of about 1.5 s (a VoIP line), the source's 1s will run for 1.5 s or more.
9. **F.3.1.2 wording.** It is written for a source that has received ANSam. When the **answer**
   modem is the source (polling, F.5-11/13), it never receives ANSam. Apply the rule from V.8
   onwards.
10. **Turnaround polling (Figs F.5-11a and F.5-12a) sends CM with no ANSam first.** V.8 7.2 forbids
    a call DCE from sending CM unless ANSam has been detected. The T.30 figures show otherwise: after
    the 1s and 70 ± 5 ms of silence, the call terminal sends CM straight away and the answer replies
    with JM.
11. **ANSam duration.** V.8 8.2.2 gives 5 ± 1 s; T.30 6.1.1 gives 2.6-4.0 s. For fax, T.30 wins.
12. **Delay from ANSam time-out to DIS.** 75 ± 20 ms in Figs F.5-8/9 (the CED rule, T.30 4.1.1)
    against 75 ± 5 ms in V.8 8.2.2 and V.34 11.1.2.5.
13. **V.8 lowest-item rule.** A JM listing both V.34 duplex (item 1) and half-duplex (item 2) selects
    **duplex**. A half-duplex-only fax must offer only modn0 b7.
14. **Figure drafting oddities in T.30.**
    - Fig F.5-13: the call terminal's box after E reads "MPh Flags" (should be Flags). Its box after
      INFO0c has no label (should be B).
    - Fig 6a: the called-side second diamond reads "CM detected?", where Fig 6b has "CI detected?".
    - The F.5 figures omit the ≥ 2 flags of F.3.1.4, the 35 ms and 4T turn-off ones, RCP and the
      A.3.1 flags. These sit inside the "Image Data" boxes or are simply not drawn.

### F-TXT. Where the extracted text (`docs\specs\text\*.txt`) disagrees with the rendered page

1. **V.34 Table 22, INFOh (render PDF p42).** From 0:3 downwards the .txt shifts the bit-range column
   one row against the definitions:
   - 4:11 sits against the fill bits;
   - 12:14 against frame sync;
   - 15:21 against power reduction;
   - 22 against TRN length;
   - 23:26 against the high carrier;
   - 27:29 against pre-emphasis;
   - 30 against the symbol rate;
   - 31:46 and 47:50 against the 16-point TRN bit, the CRC and the fill.

   The render (A.1) is authoritative.
2. **V.34 Tables 23 and 24, MPh (PDF p44-45).** The .txt ranges drift further and further down the
   table.
   - Table 23: bit 18 appears against the start bit, 20:23 against Type, 27 against a reserved bit,
     31 against the maximum rate, 35:49 against the control-channel rate, and 68 against the
     trellis select.
   - Table 24: 86:101 appears against a start bit, 103:118 against the capability mask, and 171:186
     against h(1) real.
   - Θ is lost in both.

   The render (A.6) is authoritative.
3. **V.34 eq. 10-2 (PDF p46).** The .txt shows `PPh(i) = ej 4` with `2k(k-1)+1` on the line above.
   **π and the brackets are lost**, so it can be misread as e^{j(2k(k−1)+1)/4}. The render has
   e^{jπ[(2k(k−1)+1)/4]}.
4. **Overbars are lost everywhere in the .txt.**
   - 10.1.2.1 prints both A and Ā as plain A; 10.1.2.2 does the same with B and B̄.
   - 10.2.3.3's second sentence (180°/270° points) names plain Sh where the render has **S̄h**.
   - 12.3.1.1, 12.3.2.1, 12.5.1 and 12.5.2 print S̄ as S.
   - 12.6.1.1 and 12.6.2.2 print S̄h as Sh.
   - Figure labels B̄ / Ā / S̄ / S̄h become plain letters.
5. **Symbols.** "±" becomes "�" throughout the V.34 .txt ("75 � 5 ms", "600 � 0.01%"). The "·"
   multiplication dots are dropped ("2  Q2 + Q1", "Zn  90 degrees"). In T.30 Table 2, "×" and "±"
   become "�".
6. **V.34 Figure 5 (PDF p20).** The .txt left-justifies each row, so rows with fewer than 23 entries
   lose their x position. For example, the y = 45 row `408 396 394 400 414` really sits at
   x = −7, −3, 1, 5, 9, and the y = −43 row `411 ... 393` at x = −15 ... 13.
   - The full-width rows (y = 13 ... −15) align by luck, so points 0-3 read the same either way.
   - Checked on the render with the magnitude/tie-break rule. For example 408 = (−7, 45) and
     411 = (−15, −43) both have |v|² = 2074, and the larger-imaginary rule puts 408 first.
7. **V.34 Figure 13 (PDF p33).** The .txt has only axis tick labels. The breakpoints in A.2 exist
   only in the render.
8. **V.34 Figures 15-18 and 23-28.** The .txt is a jumble of segment labels. The order of segments,
   what each dimension measures, the causal arrows and the "Current ⇒ Correct" insets (23, 24, 26,
   27) can only be read from the render.
9. **T.30 Table 2 (PDF p62-64).** The .txt interleaves rows.
   - Bit 6's "V.8 capabilities" lands below bit 8.
   - Bit 5's line shows "Real-time Internet fax" (really bit 3).
   - In bits 11-14 the code column is one row off from the meanings: code 0100 sits against the
     V.27 ter fall-back row.
   - DCS notes 31, 33 and 10/11/13/25/34 sit against the wrong rows.
   - The render (D.4) is authoritative.
10. **V.8 Tables 3 and 4 (PDF p10-11).** In the .txt the Reference column of Table 3 (H.324, V.18,
    T.101, T.30 ...) and the Item column of Table 4 (0-12) are displaced from their rows. The
    b5/b6/b7 "x" positions happen to survive.
11. **T.30 Figs F.5-1 to F.5-14.** The .txt keeps the labels, some of them as rotated text, but not
    their order across the two terminals or the causal lines.
12. **V.34 Table 17, probing tones (render PDF p37).** In the .txt the phase column slips **one row**
    from 450 Hz onwards: 450 is blank, and 1650 → 0, 1950 → 180, 2250 → 0, 2550 → 180, 2700 → 0,
    2850 → 180, 3000 → 0, 3600 → 180, with a stray trailing 0. Also "cos (2ft + )" has lost π and φ.
    - The correct values are in B.5.
    - `crates\datapump\src\v34\probe.rs` (`TONES`) already carries the rendered values, and its
      comment records this exact slip.
13. **V.34 Tables 2, 7 and 9 (render PDF p11, p17, p18).** The .txt drops the 2743 / 3000 / 3429 rows'
    values into the rows below.
    - Table 2 reads 2800 → 3/5 and 2/3, which is right, but also 3200 → 3/5 and 2/3, which is wrong:
      the true value is 4/7 and 3/5.
    - Table 7 reads 2800 → J 8, P 12. The true value is J 7, P 14.
    - Table 9 reads 2800 → W 7, P 12, AMP 56B. The true value is W 8, P 14, AMP 15AB.
    - The renders (B.8) are authoritative.
14. **No disagreement found** between the .txt and the render in:
    - T.30 Table 4;
    - T.30 Annex F prose;
    - the T.4 A.3 prose;
    - V.8 Table 1;
    - V.34 Table 14 (INFO0).

    V.34 Tables 1, 3, 4 and 8 were checked only against the fitz text, which matches the render. Their
    `docs\specs\text` copies were not compared.

---

## G. Minimum parameter set an implementation has to carry (derived checklist)

- **Per call:**
  - call/answer role;
  - source/recipient role, which can swap in turnaround polling, where V.8 CM/JM is repeated
    without ANSam;
  - both INFO0s, keeping the far INFO0 for INFOh consistency.
- **From INFOh:**
  - power cut (dB);
  - TRN n × 35 ms and 4/16 points;
  - high carrier;
  - pre-emphasis index;
  - symbol-rate index.
- **From the MPh exchange:**
  - primary rate = the highest rate enabled in both MPh (bits 35-48) that is ≤ both maxima (bits
    20:23);
  - trellis, Θ, shaping (recipient's bits 29-32);
  - h(1..3) (recipient's Type 1);
  - control-channel TX rate = the far side's bit 27, or the lower of the two if either has bit 50 = 0.
- **Control-channel state that persists across resyncs:**
  - the control-channel rate, re-used by 12.6;
  - the primary parameters, which only 12.4 or 12.8 can change.
- **Timers:**
  - 3 s control-channel watchdogs (12.4.3/12.4.4/12.6), leading to AC retrain;
  - 2000 / 2700 ms Phase 2;
  - 2000 ms Phase 3;
  - T.30 T1, T2 (reset per frame), T4 and T5.
- **Detectors needed:**
  - Tone A / Tone B and their reversals;
  - INFO0 / INFOh DPSK with CRC;
  - L1/L2 analysis (recipient);
  - S→S̄ (primary);
  - PPh;
  - Sh→S̄h;
  - AC (≥ 100 ms);
  - MPh with CRC;
  - E (≥ 18-20 ones);
  - ≥ 40 consecutive ones;
  - carrier drop (109 per 6.6.2: −43 / −48 dBm, 20-25 ms).
