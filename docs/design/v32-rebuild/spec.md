# V.32 / V.32bis receiver rebuild: what the Recommendations say

Research for the rebuild of the V.32/V.32bis demodulator (carrier recovery, timing, gain, equaliser, decisions).
Everything here is taken from ITU-T V.32 (03/93) and V.32bis (02/91), read from the rendered PDF pages in
`docs/specs/`. Every constellation, sign, bit mapping and table was read from rendered pages, and the four V.32bis
constellations also from the vector geometry of the PDF pages. None of it came from `docs/specs/text`. No other
modem implementation was consulted. Nothing in the source tree was changed.

**How to read the citations.** "V.32 §5.2.3 p.9" means clause 5.2.3 on printed page 9 of the V.32 (03/93) PDF;
the PDF page is printed page + 4. "bis §6.1 p.14" means V.32bis clause 6.1 on printed page 14; the PDF page is
printed page + 2. V.32bis numbers its clauses one lower than V.32 from clause 2 on (V.32 2.4 = bis 2.3,
V.32 5.4 = bis 6, V.32 5.5 = bis 7) and adds clause 8, rate renegotiation. "T" is one symbol interval, 1/2400 s.

Text marked **[derived]** is my own inference from the cited text. It is not in the Recommendations.

---

## 0. The start-up at a glance: what each segment gives a receiver

"Known" means the receiver can predict the symbols before they arrive, given only what the Recommendation fixes.

| Segment | Sender | Duration | Known in advance? | What a receiver can learn from it |
|---|---|---|---|---|
| ANS (V.25 2100 Hz) | answer | V.25; caller must hear ≥ 1 s (V.32 §5.4.1 p.13) | yes (a tone, not QAM) | nothing for the QAM receiver; can be used for a rough level |
| AA | call | open-ended, until the reversal it answers + 64±2T | **yes**: state A repeated, an 1800 Hz carrier | carrier frequency and phase, gain at 1800 Hz. No timing: the signal has no transitions |
| CC | call | NT (round trip + 64T) | **yes**: state C repeated (= −A) | same as AA; the AA→CC reversal is the answer end's round-trip stop |
| AC (first) | answer | ≥ 128T, even (§5.4.2 p.14) | **yes**: A,C,A,C… | lines at 600 and 3000 Hz. Their mean gives the carrier offset, their spacing the symbol clock, and they give timing phase. Carrier phase only to within 180° jointly with symbol parity. Gain only roughly (see §6.4) |
| CA | answer | MT (round trip + 64T), even | **yes**: C,A,C,A…; the join from AC doubles C | same as AC; the AC→CA reversal is what the caller answers |
| AC (second) | answer | from 64±2T after the caller's reversal until the answer end detects CC's drop | **yes**; the join from CA doubles A | same; the CA→AC reversal stops the caller's NT counter |
| silence | answer 16T; call silent from CC's end to R1 | 16T / ~1 round trip | — | — |
| optional EC sequence | either | ≤ 8192T (§5.4 Note 3 p.15) | **no**: not defined | nothing. The receiver must not take it for S (see §4.6) |
| S (call's first) | call | NT (§5.4.1 p.13) | **yes** | tells the answer end to stop R1; same content as S below |
| S | both | 256T (§5.2.1 p.9) | **yes**: A,B,A,B… starting on A ("ABAB..AB", Fig. 4 p.10) | symbol timing (phase and frequency), carrier frequency, carrier phase to within 180°, coarse gain. **Not the equaliser**: S has only three spectral lines |
| S̄ (S-bar) | both | 16T (§5.2.2 p.9) | **yes**: C,D,C,D… = −S | **the time reference** (§5.2.2): TRN symbol 0 is exactly 16T after the first S̄ symbol. Resolves the 180° ambiguity left by S |
| TRN, symbols 0–255 | both | 256T | **yes**: A or C from a scrambler started at zero, with ones in (§5.2.3 p.9) | equaliser taps (reference-directed), absolute carrier phase, gain, fine timing. The sender's echo canceller trains on its own echo |
| TRN, symbol 256 to end | both | ≥ 1024T; the whole of TRN is 1280–8192T | **yes**: A/B/C/D by Table 5 p.12. The end is **not** announced | the same, with a full two-dimensional reference. The echo canceller at the sender |
| R1 / R2 / R3 | answer / call / answer | R1 until the call's S arrives; R2 until R3 is detected; R3 until E | **partly**: B0–B3, B7, B11, B15 (and in bis B4, B8, B13, B14) are known. The scrambler runs on from TRN and the differential reference is TRN's last symbol | four-point decision-directed tracking; descrambler sync; the rate offer |
| E | call, then answer | 8T (one 16-bit sequence) | **partly**: B0–B3 = 1111 and the sync bits; the rate bits name one rate | where the rate changes: the next symbol is at the new rate and coding |
| B1 | both | 128T (§5.4.1 p.14, §5.4.2 p.15) | **yes once the descrambler is in sync** (the input is ones). The trellis encoder starts at zero; the Table 2 differential start is not stated | settling the loops, and filling the Viterbi decoder, on the real data constellation |
| Data | both | — | no | decision-directed tracking only |
| bis preamble AA 56T + CC 8T (call) / AC 56T + CA 8T (answer) | initiator, then responder | 64T (bis §8 p.17) | **yes** | arrives at any time during data. The reversal marks the start of R4/R5 exactly 8T later |
| bis R4 / R5 | initiator / responder | R4 ≥ 64T; R5 64T | **partly, and more than R1–R3**: the scrambler is reset to zero | rate proposal; no retraining |
| bis E + B1 | both | 8T + 24T | as for start-up E / B1 | rate change; the receive path unclamps 24T after E |

Who trains on what (from §5.4 p.13–15; §4.8 gives the reasoning):

- **Answer end.** Its echo canceller trains on its own first S, S̄, TRN while the call end is silent. Its
  receiver gets only one training signal: the call end's S, S̄, TRN, heard while the answer end is itself
  silent. After that come R2 (four points) and B1 (128T at the data rate).
- **Call end.** Its receiver trains on the answer end's first S, S̄, TRN while the call end is silent. Its echo
  canceller trains on its own S, S̄, TRN, sent after it has silenced the answer end. Its receiver then gets a
  second training signal, the answer end's second S, S̄, TRN, while it is itself sending R2, which is full duplex.

---

## 1. Line signal

| Item | Requirement | Source |
|---|---|---|
| Carrier | 1800 ± 1 Hz, both directions. "No separate pilot tones are to be provided." | V.32 §2.1 p.1; bis §2.1 p.1 |
| Receiver carrier tolerance | "The receiver must be able to operate with received frequency offsets of up to ± 7 Hz." | V.32 §2.1 p.1; bis §2.1 p.1 ("maximum received frequency offset of up to ± 7 Hz") |
| Symbol rate | 2400 baud ± 0.01 % | V.32 §2.3 p.2; bis §2.1 p.1 |
| Spectrum | With continuous binary ones into the scrambler, the energy density at 600 Hz and 3000 Hz is attenuated 4.5 ± 2.5 dB relative to the maximum energy density between 600 and 3000 Hz. V.32 says "should", V.32bis "shall". No pulse shape or roll-off is specified. | V.32 §2.2 p.2; bis §2.2 p.1 |
| Transmit level | "must conform to Recommendation V.2". For the switched network, V.2 limits output to ≤ 1 mW at any frequency (§2.1). For systems like this one, which do not send tones continuously, the 1-minute mean power shall not exceed −13 dBm0, instantaneous power stays within that of a 0 dBm0 sine (provisional), and power in any 10 Hz band stays ≤ −10 dBm0 (provisional) (§2.3). V.2 Note 2: the loss between subscribers "may be high: 30 to 40 dB". | V.32 §2.2 p.2; V.2 §2.1, §2.3, Note 2, p.2 |
| Timing | Transmit timing may come from the DTE (circuit 113). "In some applications it may be necessary to slave the transmitter timing to the receiver timing inside the modem." | V.32 §3.4 p.4; bis §3.1 p.8 |
| Equalisation | On international GSTN connections through G.235 16-channel terminal equipment "it may be necessary to employ a greater degree of equalization … than would be required for use on most national GSTN connections." | V.32 §1 Note 1 p.1; bis §1 Note 1 p.1 |

Consequences **[derived]**:

- ±7 Hz at 2400 baud is up to 1.05° per symbol, a full turn every 343 symbols. Over S (256T) that is 269°.
- The ±0.01 % symbol-rate tolerance applies at each end, so the two clocks can differ by 2×10⁻⁴. That is
  0.48 symbol of drift per second, 1.6 symbols over a maximum-length TRN (8192T), and about 29 symbols per minute
  of data.
- 600 and 3000 Hz are exactly carrier ± half the symbol rate. A root-raised-cosine pulse of any roll-off is
  3 dB down there, which falls inside 4.5 ± 2.5 dB; a raised-cosine applied entirely at the transmitter would be
  6 dB down. A far end may use any shaping that meets 2 to 7 dB, so the receiver must not assume a matched pulse.
  The equaliser has to absorb the difference.

---

## 2. Constellations, mapping, differential coding and the trellis code

### 2.1 The synchronising states A, B, C, D

The states are read from V.32 Fig. 1 p.3, Fig. 3 p.6 and Table 3 p.7, and from V.32bis Figs. 2-1 to 2-5 pp.4–7.
Every figure in which A, B, C and D are drawn puts them in the same relation:

| State | Figures 1 and 3/V.32 and Figures 2-3, 2-4 and 2-5/bis (9600, 7200, 4800) | Figures 2-1 and 2-2/bis (14 400, 12 000) |
|---|---|---|
| A | (−3, −1) | (−6, −2) |
| B | (1, −3) | (2, −6) |
| C | (3, 1) | (6, 2) |
| D | (−1, 3) | (−2, 6) |

- B = A turned +90°, C = A turned 180° (so C = −A and D = −B), and D = A turned 270°.
- In the 16-point diagram of Fig. 1/V.32 the four states are the points 0001, 0101, 1101 and 1001. In the
  7200 diagram (bis Fig. 2-4) they are the data points 0110, 1010, 0100 and 1000. In the 32-, 64- and 128-point
  diagrams they are **not** constellation points.
- Drafting fault: in bis Fig. 2-5 p.7, B is drawn at about (1.5, −2.8). This cannot be meant. Table 2/bis makes
  B the +90° turn of A, V.32 Fig. 1 puts B at (1, −3), and so does bis Fig. 2-4. Use (1, −3).

**Relative level [from the figures].** The figures are the only statement of the level of data relative to the
training states. At 4800, 7200 and 9600 (both codings) the data constellation's mean power equals the power of A
to D: both are 10 in figure units. At 12 000 the data averages 42 against A's 40, which is +0.21 dB. At 14 400 it
averages 41 against 40, which is +0.11 dB.

### 2.2 Differential quadrant coding, Table 1 (4800 and uncoded 9600)

Table 1/V.32 p.2 is repeated as Table 2/bis p.7. Q1 is the first bit in time.

| Q1 Q2 | Quadrant change |
|---|---|
| 0 0 | +90° |
| 0 1 | 0° |
| 1 0 | +180° |
| 1 1 | +270° |

The output Y1 Y2 names the quadrant: 00 = A, 01 = B, 11 = C, 10 = D. That order is anticlockwise, so +90° takes
A to B, B to C, C to D and D to A. **4800** (V.32 §2.4.2 p.3; bis §2.3.5 p.7) sends one of A, B, C, D.

### 2.3 Uncoded 9600, 16 points (V.32 only)

V.32 §2.4.1.1 p.2, Fig. 1 p.3, Table 3 p.7. The first two bits (Q1, Q2) go through Table 1. Y1 Y2 choose the
quadrant and Q3 Q4 choose the point within it:

| Y1Y2 \ Q3Q4 | 00 | 01 | 10 | 11 |
|---|---|---|---|---|
| 00 (A) | (−1,−1) | (−3,−1) | (−1,−3) | (−3,−3) |
| 01 (B) | (1,−1) | (1,−3) | (3,−1) | (3,−3) |
| 10 (D) | (−1,1) | (−1,3) | (−3,1) | (−3,3) |
| 11 (C) | (1,1) | (3,1) | (1,3) | (3,3) |

Each quadrant's pattern is the A pattern turned by that quadrant's angle. Q3 Q4 therefore read the same under a
90° ambiguity. V.32bis defines no uncoded 9600. Its 9600 is trellis-coded only (bis §1 and §2.3.3 pp.1, 5), and
V.32 §1 e) p.1 makes the 16-point alternative mandatory for interworking at 9600.

### 2.4 Trellis coding (V.32 9600; V.32bis 7200, 9600, 12 000 and 14 400)

V.32 §2.4.1.2 p.3 with Fig. 2 p.4 and Table 2 p.5; bis §2.3.1–2.3.4 pp.2–5 with Table 1 p.2 and Fig. 1 p.3.
The scrambled stream is split into groups of 3, 4, 5 or 6 bits at 7200, 9600, 12 000 and 14 400.

**Differential coding (Table 2/V.32 = Table 1/bis), indexed by Q1Q2 and the previous Y1Y2:**

| Q1Q2 \ prev Y1Y2 | 00 | 01 | 10 | 11 |
|---|---|---|---|---|
| 00 | 00 | 01 | 10 | 11 |
| 01 | 01 | 00 | 11 | 10 |
| 10 | 10 | 11 | 01 | 00 |
| 11 | 11 | 10 | 00 | 01 |

Equivalently, Y1n = Q1n ⊕ Y1n−1 and Y2n = Q2n ⊕ Y2n−1 ⊕ (Q1n·Y1n−1). This is addition mod 4 of the numbers
2·Y2 + Y1. **It is not Table 1.**

**Convolutional encoder.** V.32 Fig. 2 p.4 and bis Fig. 1 p.3 draw the same encoder. It has three delays; call
their contents s1, s2 and s3. There are four exclusive-ORs and two AND gates, and the figure's own truth table
confirms which is which.

- Y0n = s3, the output of the last delay. Y0 depends only on earlier inputs.
- w = s2 ⊕ Y2n
- next s1 = s3
- next s2 = s1 ⊕ Y1n ⊕ Y2n ⊕ (s3 · w)
- next s3 = w ⊕ (Y1n · s3)

Y1, Y2 and Q3 upwards pass through unchanged. The label is Y0 Y1 Y2 Q3 … Qk, read left to right from the most
significant bit.

**Initial state.** "the initial states of the delay elements of the convolution encoder … should be set to
zero" at the start of the scrambled ones after E (V.32 §5.4.1 p.13 and §5.4.2 p.14; the latter says "Figure 3",
a typo for Figure 2). V.32bis says "shall" (bis §6.1 p.14, §6.2 p.15, §8.1 and §8.2 p.17). Neither
Recommendation states the starting value of the differential encoder's previous Y1Y2. V.32 Fig. 2 draws it in a
separate box from the convolutional encoder. A receiver should not rely on it: the first Q1Q2 after any restart
is unreliable anyway.

**Constellations**, grouped by the subset Y0Y1Y2, each entry being the uncoded bits and the point (x, y). These
were read by position from the figures (bis Figs. 2-4, 2-3, 2-2, 2-1; V.32 Fig. 3 and Table 3). They are in
figure units.

7200, 16 points (bis Fig. 2-4 p.6), label Y0Y1Y2Q3:
```
000 | 0:(3,-3)  1:(-1,1)      100 | 0:(-1,3)  1:(3,-1)
001 | 0:(-3,3)  1:(1,-1)      101 | 0:(1,-3)  1:(-3,1)
010 | 0:(3,1)   1:(-1,-3)     110 | 0:(-3,-3) 1:(1,1)
011 | 0:(-3,-1) 1:(1,3)       111 | 0:(3,3)   1:(-1,-1)
```
9600 trellis, 32 points (V.32 Table 3 p.7 = V.32 Fig. 3 p.6 = bis Fig. 2-3 p.6), label Y0Y1Y2Q3Q4:
```
000 | 00:(-4,1)  01:(0,-3)  10:(0,1)   11:(4,1)
001 | 00:(4,-1)  01:(0,3)   10:(0,-1)  11:(-4,-1)
010 | 00:(-2,3)  01:(-2,-1) 10:(2,3)   11:(2,-1)
011 | 00:(2,-3)  01:(2,1)   10:(-2,-3) 11:(-2,1)
100 | 00:(-3,-2) 01:(1,-2)  10:(-3,2)  11:(1,2)
101 | 00:(3,2)   01:(-1,2)  10:(3,-2)  11:(-1,-2)
110 | 00:(1,4)   01:(-3,0)  10:(1,0)   11:(1,-4)
111 | 00:(-1,-4) 01:(3,0)   10:(-1,0)  11:(-1,4)
```
12 000, 64 points (bis Fig. 2-2 p.5), label Y0Y1Y2Q3Q4Q5:
```
000 | 000:(7,1)  001:(3,5)   010:(7,-7)  011:(-5,5)  100:(3,-3)  101:(-1,1)  110:(-1,-7) 111:(-5,-3)
001 | 000:(-7,-1) 001:(-3,-5) 010:(-7,7) 011:(5,-5)  100:(-3,3)  101:(1,-1)  110:(1,7)   111:(5,3)
010 | 000:(-1,5) 001:(-5,1)  010:(7,5)   011:(-5,-7) 100:(3,1)   101:(-1,-3) 110:(7,-3)  111:(3,-7)
011 | 000:(1,-5) 001:(5,-1)  010:(-7,-5) 011:(5,7)   100:(-3,-1) 101:(1,3)   110:(-7,3)  111:(-3,7)
100 | 000:(-5,-1) 001:(-1,-5) 010:(-5,7) 011:(7,-5)  100:(-1,3)  101:(3,-1)  110:(3,7)   111:(7,3)
101 | 000:(5,1)  001:(1,5)   010:(5,-7)  011:(-7,5)  100:(1,-3)  101:(-3,1)  110:(-3,-7) 111:(-7,-3)
110 | 000:(1,-7) 001:(5,-3)  010:(-7,-7) 011:(5,5)   100:(-3,-3) 101:(1,1)   110:(-7,1)  111:(-3,5)
111 | 000:(-1,7) 001:(-5,3)  010:(7,7)   011:(-5,-5) 100:(3,3)   101:(-1,-1) 110:(7,-1)  111:(3,-5)
```
14 400, 128 points (bis Fig. 2-1 p.4), label Y0Y1Y2Q3Q4Q5Q6:
```
000 | 0000:(-8,-3) 0001:(8,-3) 0010:(4,-3) 0011:(4,-7) 0100:(-4,-3) 0101:(-4,-7) 0110:(0,-3) 0111:(0,-7)
      1000:(-8,1)  1001:(8,1)  1010:(4,1)  1011:(4,5)  1100:(-4,1)  1101:(-4,5)  1110:(0,1)  1111:(0,5)
001 | 0000:(8,3)   0001:(-8,3) 0010:(-4,3) 0011:(-4,7) 0100:(4,3)   0101:(4,7)   0110:(0,3)  0111:(0,7)
      1000:(8,-1)  1001:(-8,-1) 1010:(-4,-1) 1011:(-4,-5) 1100:(4,-1) 1101:(4,-5) 1110:(0,-1) 1111:(0,-5)
010 | 0000:(2,-9)  0001:(2,7)  0010:(2,3)  0011:(6,3)  0100:(2,-5)  0101:(6,-5)  0110:(2,-1) 0111:(6,-1)
      1000:(-2,-9) 1001:(-2,7) 1010:(-2,3) 1011:(-6,3) 1100:(-2,-5) 1101:(-6,-5) 1110:(-2,-1) 1111:(-6,-1)
011 | 0000:(-2,9)  0001:(-2,-7) 0010:(-2,-3) 0011:(-6,-3) 0100:(-2,5) 0101:(-6,5) 0110:(-2,1) 0111:(-6,1)
      1000:(2,9)   1001:(2,-7) 1010:(2,-3) 1011:(6,-3) 1100:(2,5)   1101:(6,5)   1110:(2,1)  1111:(6,1)
100 | 0000:(9,2)   0001:(-7,2) 0010:(-3,2) 0011:(-3,6) 0100:(5,2)   0101:(5,6)   0110:(1,2)  0111:(1,6)
      1000:(9,-2)  1001:(-7,-2) 1010:(-3,-2) 1011:(-3,-6) 1100:(5,-2) 1101:(5,-6) 1110:(1,-2) 1111:(1,-6)
101 | 0000:(-9,-2) 0001:(7,-2) 0010:(3,-2) 0011:(3,-6) 0100:(-5,-2) 0101:(-5,-6) 0110:(-1,-2) 0111:(-1,-6)
      1000:(-9,2)  1001:(7,2)  1010:(3,2)  1011:(3,6)  1100:(-5,2)  1101:(-5,6)  1110:(-1,2) 1111:(-1,6)
110 | 0000:(-3,8)  0001:(-3,-8) 0010:(-3,-4) 0011:(-7,-4) 0100:(-3,4) 0101:(-7,4) 0110:(-3,0) 0111:(-7,0)
      1000:(1,8)   1001:(1,-8) 1010:(1,-4) 1011:(5,-4) 1100:(1,4)   1101:(5,4)   1110:(1,0)  1111:(5,0)
111 | 0000:(3,-8)  0001:(3,8)  0010:(3,4)  0011:(7,4)  0100:(3,-4)  0101:(7,-4)  0110:(3,0)  0111:(7,0)
      1000:(-1,-8) 1001:(-1,8) 1010:(-1,4) 1011:(-5,4) 1100:(-1,-4) 1101:(-5,-4) 1110:(-1,0) 1111:(-5,0)
```

**Checks passed by every constellation above.** These make a misread very unlikely:

1. Turning a point +90° never changes Q3 and above. In all four constellations the subset label changes in the
   same way: 000→111, 001→110, 010→100, 011→101, 100→011, 101→010, 110→000 and 111→001. So Y0 flips and
   2·Y2 + Y1 goes down by 1 mod 4. Table 2 adds Q to that same number, so the differences survive a turn.
2. Every point lies on one lattice. The 32- and 128-point sets use points whose coordinates sum to an odd
   number, with d²min = 2; the 16- and 64-point sets use odd,odd points, with d²min = 4. Within one Y0 value the
   squared distance is 2×d²min, and within one Y0Y1Y2 subset it is 8×d²min.
3. The mean powers are 10, 10, 42 and 41, as the relative-level note in §2.1 expects.

Distances and peaks, normalised to each constellation's own RMS **[derived]**:

| Rate | Points | dmin / RMS | Peak / RMS | Data power / A–D power (figures) |
|---|---|---|---|---|
| 4800 | 4 | 1.414 | 1.000 | 1 |
| 9600 uncoded | 16 | 0.632 | 1.342 | 1 |
| 7200 | 16 | 0.632 | 1.342 | 1 |
| 9600 trellis | 32 | 0.447 | 1.304 | 1 |
| 12 000 | 64 | 0.309 | 1.528 | 42/40 |
| 14 400 | 128 | 0.221 | 1.440 | 41/40 |

### 2.5 Tentative decisions

Neither Recommendation says anything about how the receiver decides. There is no decoding clause at all; the
trellis code is defined only at the transmitter. Three things follow from the encoder's structure **[derived]**:

- **Uncoded rates (4800, 9600/16).** The symbol-by-symbol slice is the real decision and has no delay.
- **Trellis rates.** Y0n is the contents of the last delay element, so it is fixed by the encoder state before
  symbol n arrives. A decoder that knows its best current state therefore knows which half of the constellation
  (Y0 = 0 or 1) this symbol belongs to. Slicing within that half has 2×d²min, 3 dB better than slicing the whole
  constellation, and costs no delay. Y1 and Y2 depend on the current input, so the full subset, at 8×d²min, is
  known only after Viterbi traceback.
- **The practical choices for the loops.** Nearest point over the whole constellation (no delay, the worst error
  rate: at 14 400, dmin is 0.22 RMS). Nearest point within the Y0 half given by the best survivor (no delay,
  3 dB better). Or the survivor's decision after a short traceback, which is better still but delays the loops.
  During start-up the reference is known (§0), so none of this applies until B1.

### 2.6 The code's tables against the rendered figures

I checked `crates/datapump/src/v32.rs` and `crates/datapump/src/v32/trellis.rs` at commit d7b914b.

**Agree:**

- `STATES` (v32.rs:129–134) against V.32 Fig. 1.
- `WITHIN_QUADRANT` and `rotate` (v32.rs:165–188) against Table 3, all 16 points.
- `QUADRANT_CHANGE` and `CHANGE_TO_DIBIT` (v32.rs:204, 207) and `turn` (v32.rs:766–770) against Table 1.
- The 9600-uncoded bit split (v32.rs:737–740 and 1109–1114).
- `DIFFERENTIAL` (trellis.rs:58–63) against Table 2/V.32 and Table 1/bis.
- `advance` (trellis.rs:481–488) against V.32 Fig. 2 and bis Fig. 1.
- The bit ordering of the code index (trellis.rs:519–535, 646–656).
- `POINTS_16`, `POINTS_32`, `POINTS_64` and `POINTS_128` (trellis.rs:70–335). I compared them by script against
  positions taken from the figures' vector geometry: 0 differences in 240 points. `POINTS_32` also matches V.32
  Table 3 read by eye.
- The scrambler taps and allocation (v32.rs:101–106, 401–419, 912).
- The first-256 TRN rule (v32.rs:703–706). Run from zero it reproduces both of §5.2.3's printed patterns.
- The rate-signal bits (startup.rs:416–446, 454–469, 510–522) against Table 6/7 of V.32 and Table 5/6 of bis.

**Mismatches:**

1. **v32.rs:427 (used at v32.rs:708): TRN after symbol 256 maps dibits 10 and 11 the wrong way round.**
   `TRN_STATES = [A, B, C, D]` is indexed by `first << 1 | second`, so the code sends dibit 10 as C and 11 as D.
   Table 5/V.32 p.12 and Table 4/bis p.12 both give 00→A, 01→B, **11→C, 10→D**, the same order as Table 1's
   Y1Y2. The array should be `[A, B, D, C]`. As it stands, every TRN symbol from 256 on whose first bit is 1 is
   90° from what the spec requires (C sent for D, D sent for C); that is about half of the Table-5 part.
   §5.2.3 says TRN is "intended for
   training the adaptive equalizer in the receiving modem". A far end that trains against the known TRN sequence
   is being given the wrong reference. Our own receiver has no TRN reference today, so it is unaffected, but the
   rebuilt one must use Table 5 as printed. The reference vectors in §4.3 are built with Table 5.
2. **v32.rs:298–311 `nearest_state`: the four-point decision boundaries are 4.07° off.** The point is rotated by
   22.5° (the COS and SIN constants) before the sign test. The boundaries should lie halfway between neighbouring
   states, at A's angle (198.43°) ± 45°. That needs a rotation of 26.57° (= 45° − atan(1/3), which moves the
   243.43° boundary to 270°).
   With 22.5°, a point at 245° is taken as A although it is nearer B. This is not a table error, but it biases
   every 4800 decision and every start-up decision.
3. **trellis.rs:414–431, 438, 440: 12 000 and 14 400 are scaled to the training states' power.** The figures put
   them 0.21 dB and 0.11 dB above it (§2.1). The effect is small, but it is the only level statement the
   Recommendation makes.
4. Comments only:
   - v32.rs:369 and v32.rs:1156 call the 16-point constellation "Figure 2/V.32". It is Figure 1; Figure 2 is the
     trellis encoder.
   - trellis.rs:569 cites "V.32 clause 8", which does not exist; V.32 specifies no decoder.
   - v32.rs:1 still describes the module as "V.32 at 4800 bit/s".
5. **Not a table, but worth knowing.** The phase of the S alternation is not pinned: `alternate` counts from the
   free-running `tick` (v32.rs:689, 696). S can therefore start on B, whereas Fig. 4's legend reads "ABAB..AB".
   S̄ still inverts S, so the time reference survives. The receiver should identify S by its rotation sense
   (even to odd symbol is +90°), not by assuming the first symbol is A.

---

## 3. Scramblers

V.32 §4 p.6 and §4.1 p.8; bis §4 and §4.1 p.9.

- The scrambler is self-synchronising, with a different one in each direction.
- The call end's generating polynomial is GPC = 1 + x⁻¹⁸ + x⁻²³. The answer end's is GPA = 1 + x⁻⁵ + x⁻²³.
- The transmitter divides by the polynomial: dₙ = mₙ ⊕ dₙ₋₁₈ ⊕ dₙ₋₂₃ for GPC, and dₙ = mₙ ⊕ dₙ₋₅ ⊕ dₙ₋₂₃ for GPA.
  The receiver multiplies: mₙ = dₙ ⊕ dₙ₋ₖ ⊕ dₙ₋₂₃, using received bits.
- On the GSTN the call end scrambles with GPC and descrambles with GPA; the answer end does the opposite. V.32
  leaves the allocation on leased lines to bilateral agreement. V.32bis keeps the GSTN allocation there too.
- The descrambler resynchronises within 23 bits of any start. It needs the 23 most recent line bits.

**What is scrambled, and from what state:**

| Signal | Scrambled? | Scrambler state at the start | Differential coding |
|---|---|---|---|
| AA, CC, AC, CA, S, S̄ | no (fixed states) | — | — |
| TRN | yes: ones in, 2 bits per symbol at 4800 bit/s | **all zeros** (§5.2.3 p.9; bis §5.2.3 p.12) | **disabled** |
| R1, R2, R3, E (start-up and retrain) | yes, 16-bit sequences repeated, B0 first, 2 bits per symbol | runs on from TRN; no reset is stated (§5.3 p.10; bis §5.3 p.12) | Table 1; "The differential encoder shall be initialized using the final symbol of the transmitted TRN segment" |
| B1 | yes: ones at the new rate and coding | runs on from E | Table 1 (uncoded) or Table 2 (trellis); the trellis delays start at zero |
| Data | yes | runs on | as B1 |
| bis R4, R5, E | yes | **all zeros** at the start of R4/R5 (bis §5.3 p.12, §8 p.17) | Table 2/bis; "initialized using the final symbol of the transmitted preamble" |

---

## 4. Start-up

V.32 §5.4 pp.13–15 and Fig. 4 p.10; bis §6 pp.14–16 and Fig. 3 p.10. The two are the same procedure, except
that bis adds Note 6 and says 3100 ms in Note 2 where V.32 says 3050 ms. Figure 4's legend defines the signals:

- AC is "signal states ACAC..AC for an even number of symbol intervals T; similarly with CA, AA and CC".
- MT and NT are the "round-trip delays observed from answer and call modems respectively, including 64T ± 2T
  modem turn round delay".
- S is "ABAB..AB"; S̄ is "CDCD..CD".
- R1, R2 and R3 are each "a repeated 16-bit rate sequence at 4800 bit/s scrambled and differentially encoded as in
  Table 1".
- E is "a single 16-bit sequence marking and following the end of a whole number of 16-bit rate sequences in R2
  and R3".
- B1 is "binary ones scrambled and encoded as for the subsequent transmission of data".

### 4.1 Call end (V.32 §5.4.1 pp.13–14; bis §6.1 p.14)

1. After receiving the answer tone for at least 1 s, connect and set up the scrambler and descrambler. Note 1
   allows proceeding on the 600/3000 Hz tones without having heard 2100 Hz.
2. Transmit **AA**, and detect either incoming tone, 600 ± 7 Hz or 3000 ± 7 Hz, and then a phase reversal in it.
   Note 2: the tones "may be preceded by a special pattern which may last up to 3050 ms", 3100 ms in bis.
3. On the first reversal, start the counter and switch to **CC**. The AA→CC transition must appear at the line
   **64 ± 2T** after the reversal reached the line terminals.
4. On the second reversal in the same tone, stop the counter (this is NT) and **cease transmitting**.
5. "When the modem detects an incoming S sequence … it shall proceed to train its receiver". Then detect R1:
   two consecutive identical 16-bit sequences.
6. On R1, transmit **S for NT**.
7. Optionally send an echo-canceller sequence (§4.6), then the conditioning signal: **S 256T, S̄ 16T,
   TRN ≥ 1280T**. TRN "may be extended in order to ensure a satisfactory level of echo cancellation".
8. Transmit **R2**, excluding anything R1 did not offer (and recommended to reflect receiver performance), until
   **R3** is detected.
9. Finish the current sequence, send **E** (8T) naming what R3 chose, then scrambled ones at that rate and
   coding, with the trellis delays at zero. If R3 calls for cleardown, disconnect.
10. "On detecting an incoming 16-bit E sequence … condition itself to receive data at the rate and with the coding
    indicated". After **128T**, turn circuit 109 ON and unclamp 104.

### 4.2 Answer end (V.32 §5.4.2 pp.14–15; bis §6.2 p.15)

1. Connect, set up the scrambler, and send the V.25 answer sequence (it may be omitted on leased lines, and in
   V.32 on national GSTN; §5.1 p.9 and bis §5.1 p.9).
2. Transmit **AC**.
3. Once AC has lasted ≥ 128T (an even number) **and** 1800 ± 7 Hz has been detected for 64T, start the counter
   and switch to **CA** (an even number of symbols). The join doubles C.
4. On a reversal in the incoming tone (the caller's AA→CC), stop the counter (this is MT). "After transmitting a
   state A", revert to **AC**; the join doubles A. The CA→AC transition must appear at the line **64 ± 2T** after
   the reversal was received.
5. On an **amplitude drop** in the incoming tone (the caller stopping CC), cease transmitting for **16T**.
   Optionally send the EC sequence, then **S 256T, S̄ 16T, TRN ≥ 1280T** (extendable), then **R1**.
6. "On detection of an incoming S sequence, the modem shall cease transmitting."
7. Wait **MT**. Then, "if an incoming S sequence persists, or when an S sequence reappears", train the receiver.
   Then detect **R2**.
8. On R2, send the second conditioning signal, **S, S̄, TRN**, then **R3**. R3 must lie within R2 and is
   recommended to reflect receiver performance. bis Note 6: if R3 calls for cleardown, repeat it for ≥ 64T first.
9. On the caller's **E**, condition the receiver to the indicated rate and coding.
10. Finish the current R3 sequence, send **E**, then scrambled ones for **128T** (the trellis delays at zero).
    Then enable 106, turn 109 ON and unclamp 104.

Notes to §5.4 (V.32 p.15; bis Notes to §6 p.16):

- Note 4: "a period of 650 ms is needed for training any network echo cancellers conforming to Recommendation
  G.165".
- Note 5: the answer end may disconnect if 1800 Hz is not detected after AC, "it shall not disconnect for at least
  3 seconds".

### 4.3 The conditioning signal, segment by segment (V.32 §5.2 p.9; bis §5.2 pp.10, 12)

- **S (segment 1), 256T.** It alternates A and B. The receiver knows every symbol **[derived]**: going from an
  even symbol to an odd one is +90° (A→B), and going back is −90°. That fixes the symbol parity, and the carrier
  phase to within 180°. The line signal is a line at 1800 Hz with amplitude |A+B|/2 = √5, plus lines at 600 and
  3000 Hz with |A−B|/2 = √5, all scaled by the far transmitter's filter. Only three frequencies are excited, so S
  cannot identify a channel for the equaliser. §5.2.3 names TRN as the equaliser's segment.
- **S̄ (segment 2), 16T.** It alternates C and D, which is S negated. §5.2.2 (V.32 p.9, normative text; a Note in
  bis p.10): "The transition from segment 1 to segment 2 provides a well-defined event in the signal that may be
  used for generating a time reference in the receiver." **[derived]** Every line in S reverses at once. TRN symbol
  0 is the 17th symbol after the transition. Because S (256T) is much longer than S̄ (16T), there is no 180°
  ambiguity left.
- **TRN (segment 3), 1280 to 8192T.** The 8192 maximum is "for further study". TRN is ones scrambled at 4800 bit/s
  by the sender's own polynomial, from an all-zero state, with the differential encoding disabled.
  - Symbols 0–255 use only the first bit of each dibit: 0 sends A, 1 sends C.
  - From symbol 256 on, the whole dibit is mapped by Table 5/V.32 (Table 4/bis): 00 A, 01 B, 11 C, 10 D.
  - "Segment 3 is intended for training the adaptive equalizer in the receiving modem and the echo canceller in
    the transmitting modem."
  - **The end of TRN is not signalled.** The receiver sees it only when the symbols stop matching, or when a rate
    sequence appears.

**TRN reference vectors.** I generated these from the rule above. The first 15 dibits reproduce the patterns
printed in §5.2.3, which are "11 11 11 11 11 11 11 11 11 00 00 01 11 11 11" for GPC and
"11 11 10 00 00 11 11 10 00 00 11 10 01 11 11" for GPA.

```
Call end (GPC), symbols 0-255 (A/C):
  0 CCCCCCCCCAAACCCCCCAAAAACCCCAAACCAAACAAAAAAAAACAACCCCCCACCCACCCCA
 64 CCAACAACACCCAACCCCCCAAAAAACCAAACCCCAACAAAAACAACCCCCAACACAACCAACC
128 AACAAAACACCCCAACACACAAACCCCAAACCACACCAACAACAACAACCCACCCCAAAAACAA
192 CAAACCACCCCAAAAACCACACACCAACCCAAAAAACACCAACCAACCCCAAACACAACAACAC
256-287 (Table 5): ACCBCAADBBCDCCBACDCACCCCADCBABDB

Answer end (GPA), symbols 0-255 (A/C):
  0 CCCAACCCAACCACCAACAACCAAACACAACCAAAACACCAACCCCCACACACCAACAAACCAC
 64 CCCACCACCCAACAAACACAAAACAACACCACAAAAACCACAAACAACCCCACACACACAACCC
128 CCACAACACACCCCCCACAACACAAAAACACACCACACCCCCACACACACAAAAAAACACAACA
192 CCAAAACCACCAACAAACCCCAACCACCACAACAACAACCCAACAAAAAAACACCAACCACACC
256-287 (Table 5): CBABAACDDCBBCCADBCABABCACCCACCDD
```

A receiver hears the other end's TRN, so it uses the far end's polynomial: the call end expects the GPA vector.
With the code's `TRN_STATES` as it is today, our transmitter sends the C↔D-swapped version of the part after
symbol 256.

### 4.4 Rate signals R and E (V.32 §5.3 p.10 and Tables 6–7 p.12; bis §5.3 pp.12–13 and Tables 5–6 p.13)

- A rate signal is a whole number of repeated 16-bit sequences, B0 first, at two bits per symbol. The first two
  bits form the first symbol, so B0 always leads a dibit and each sequence is exactly 8 symbols.
- **Detection (§5.3.1):** "the receipt of two consecutive identical 16-bit sequences each with bits B0-3, B7, 11
  and 15 conforming".
- **Ending (§5.3.2):** "In order to mark the end of transmission of any rate signal other than R1 … first complete
  the transmission of the current 16-bit rate sequence, and then transmit one 16-bit sequence E."

V.32 Table 6, the rate sequence:

| Bit | Meaning |
|---|---|
| B0–B3 | 0000 |
| B4 | 2400 (for further study) |
| B5 | 4800 |
| B6 | 9600 |
| B4–B6 = 000 | cleardown |
| B7 | 1 |
| B8 | trellis at the highest rate in B4–B6 |
| B9–B14 | 001000 = no special modes (B11 = 1 doubles as a sync bit) |
| B15 | 1 |

"B4 equal one and B8 equal one indicates V.32 bis operation" (Note 1).

V.32 Table 7, E: B0–B3 = **1111**; B7, B11 and B15 = 1; B4–B14 as in Table 6, but naming only the rate and
coding of the scrambled ones that follow E.

bis Table 5, the rate sequence:

| Bit | Meaning |
|---|---|
| B0–B3 | 0000 |
| B4 | 1 |
| B5 | 4800 |
| B6 | 9600 |
| B7 | 1 |
| B8 | 1 |
| B9 | 7200 |
| B10 | 12 000 |
| B11 | 1 |
| B12 | 14 400 |
| B13, B14 | 0 (ignored on reception) |
| B15 | 1 |

Note 1: B4 or B8 = 0 means V.32 interworking only. Note 3: B4–B6, B9–B10 and B12 all zero calls for cleardown.

bis Table 6, E: 1111 1--1 1--1 -001, where "-" marks a rate bit.

**What the receiver knows [derived]:**

- 7 of 16 bits per V.32 sequence (B0–B3, B7, B11, B15). For bis, 11 (adding B4, B8, B13, B14).
- The scrambler continues from TRN. If TRN's length is known, the transmit scrambler's state is too; in any case
  the descrambler is in sync by then.
- The differential reference for the first R symbol is TRN's last symbol, which is known absolutely.
- There are only 8 possible framing phases, one per symbol.
- B0–B3 arrive in a sequence's first two symbols. By then the receiver knows whether the sequence is an R or an
  E. The last rate bit (B12) arrives in symbol 7. So the rate change can be made exactly at the symbol after E's
  eighth.

### 4.5 Round-trip delay (MT, NT) and the 64 ± 2T turnaround

- **MT (answer end):** measured from its AC→CA transition to the detection of the caller's AA→CC reversal. It is
  one round trip plus the caller's 64 ± 2T turnaround.
- **NT (call end):** it runs while CC is sent, which is one round trip plus the answer end's turnaround (Fig. 4
  legend p.10).
- The **64 ± 2T** is measured at the line terminals in both directions (§5.4.1 p.13, §5.4.2 p.14). The whole
  chain inside one modem must therefore be known to ±2T (±0.83 ms): the receive filter and detector latency plus
  the transmit filter delay.
- Uses: the call end holds S for NT; the answer end waits MT before training. Both are how the echo canceller
  finds the far echo **[derived]**: the far echo arrives at about MT − 64T at the answer end and NT − 64T at the
  call end.
- **Neither Recommendation sets a maximum round trip.** For project context, not from the spec: this project's
  VoIP line measured about 1.5 s round trip, 750 ms each way (memory note `voip-line-round-trip`).

### 4.6 Echo-canceller training, and the optional EC sequence

- "The procedure includes the estimating of round-trip delay from each modem, the training of echo cancellers and
  receivers initially with half-duplex transmissions" (§5.4 p.13; bis §6 p.14). Channel separation is "by echo
  cancellation techniques" (§1 b) p.1).
- TRN serves both the far equaliser and the near echo canceller (§5.2.3), and "may be extended".
- **Note 3 (V.32 p.15; bis p.16).** A sender may instead precede the conditioning signal with a sequence
  specifically for training the echo canceller:
  - It "need not be defined in detail".
  - It must keep network echo control disabled.
  - "The sum of its power in the three 200 Hz bands centred at 600 Hz, 1800 Hz and 3000 Hz is at least 1 dB less
    than its power in the remaining bandwidth", averaged over any 6 ms.
  - It is ≤ 8192T.
  - Fig. 4 puts it at the gap before S 256T: after the caller's S (NT), and after the answer end's 16T silence.
  - **[derived]** This is the Recommendation's own S/S̄ discriminator. S and S̄ put essentially all their power
    in those three bands. A receiver waiting for S may first hear up to 3.4 s of an unknown broadband signal, and
    must neither train on it nor take it for S.
- Circuit 109 (§3.7 p.5; bis §3.1 p.8): "Thresholds and response times are inapplicable because a line signal
  detector cannot be expected to distinguish wanted received signals from unwanted talker echoes." Energy alone
  cannot declare the far carrier present.

### 4.7 B1 and the switch to data

- B1 is scrambled binary ones at the chosen rate and coding, 128T at start-up and retrain (24T in bis §8). The
  trellis delays start at zero.
- **[derived]** Once the descrambler is in sync, the receiver knows every B1 bit, since the input is ones and the
  descrambler register holds the transmit scrambler's state. For uncoded rates B1 is fully predictable, using the
  Table 1 reference from E's last symbol. For trellis rates it is predictable apart from the unstated initial
  Y1Y2 of the Table 2 encoder, which leaves four hypotheses.

### 4.8 Who trains what, and when [derived from §5.4]

- **The answer end's first S, S̄, TRN.** The caller has stopped transmitting after CC. So this signal is at once
  the answer end's echo-canceller training on a quiet line and the call end's receiver training.
- **The call end's S for NT.** It silences the answer end: R1 stops arriving at the call end about one round trip
  plus the answer end's detection time after the S starts, and NT exceeds the round trip by 64T. Its own
  conditioning signal then trains its echo canceller on a quiet line, and trains the answer end's receiver after
  the answer end's MT wait.
- **The answer end's second S, S̄, TRN** arrives while the call end is sending R2, so it is full duplex. The call
  end's echo canceller must already be converged.
- **The answer end's receiver** has only the call end's single conditioning signal (at least 1552T = 647 ms of
  known reference), then R2 (four-point, partly known) and 128T of B1 before data. It must reach 14 400-grade SNR
  from that.

---

## 5. Retrain and rate renegotiation

### 5.1 Retrain (V.32 §5.5 p.15 and Fig. 5 p.11; bis §7 p.16 and Fig. 4 p.11)

A retrain may be started by "either modem [that] incorporates a means of detecting unsatisfactory signal
reception". How that is detected is not specified.

- **Call end (§5.5.1).** It retrains on unsatisfactory reception, or on detecting 600 ± 7 or 3000 ± 7 Hz (the
  answer end's AC) for **more than 128T**. It turns 106 OFF, clamps 104 to ones and sends **AA**, then follows
  §5.4.1 from its third paragraph: detect the tone and its reversals, then CC, NT and the rest.
- **Answer end (§5.5.2).** It retrains on unsatisfactory reception, or on detecting 1800 ± 7 Hz (the caller's AA)
  for **more than 128T**. It turns 106 OFF, clamps 104 and sends **AC** "for an even number of symbol intervals
  not less than 128", then follows §5.4.2 from its third paragraph.
- Everything after that is the start-up without the answer tone: the same AC/CA/AC, the MT/NT measurement, S, S̄,
  TRN, R1/R2/R3, E and B1. Circuit 107 stays ON (§5.5.2 Note; bis §7.3).
- **Figure against text.** Fig. 5a (V.32 p.11, and bis Fig. 4a p.11) labels the answer end's AC in a
  caller-initiated retrain "≥ 64T", after "≥ 128T" of detection, whereas §5.5.2 says not less than 128. A call-end
  receiver should cope with 64T.
- §5.5.3 (bis §7.3): circuit 109 may go OFF if AA (at the call end) or the first AC (at the answer end) lasts more
  than 45 s. 109 goes back ON when 104 unclamps.
- V.32 also says "The need for a shorter duplex retrain procedure … is for further study" (p.15).
- When V.25 is omitted (on leased lines, or where national rules permit), the answer end starts as in a retrain
  (V.32 §5.1 p.9; bis §5.1 p.9).

### 5.2 Rate renegotiation (V.32bis §8 p.17, Fig. 5 p.18, Notes p.19)

"… to enable modems to change their data signalling rate without retraining." Either modem may start it, at any
time during data.

- **Preamble.** The call end sends AA for 56T then CC for 8T. The answer end sends AC for 56T then CA for 8T.
- **The rate signal** is as in §5.3, with the scrambler at all zeros and the differential encoder referenced to
  the preamble's last symbol, which is C for the call end and A for the answer end **[derived from the 8T
  segment]**.
- **Initiator (§8.1).**
  1. Turn 106 OFF and send the preamble, then **R4**, which names the desired rate and all lower enabled rates.
  2. On detecting the other end's preamble (this "might occur during the transmission of a preamble if both
     modems initiate … almost simultaneously"), clamp 104 and look for R5.
  3. On R5, look for E. Once R4 has run ≥ 64T, finish the current sequence, send **E** naming the highest rate
     common to R4 and R5, then **24T** of scrambled ones with the trellis delays at zero, then enable 106.
  4. On the incoming E, switch the receiver and unclamp 104 **24T** later.
- **Responder (§8.2).** "A modem shall be conditioned to detect an incoming preamble at any time while receiving
  data."
  1. On a preamble, clamp 104 and look for R4.
  2. On R4, turn 106 OFF and send its own preamble, then **R5 for 64T**. R5 names its desired rate and all lower
     enabled rates, "irrespective of the rates indicated in R4".
  3. Send **E** (the highest common rate), then **24T** of ones, then enable 106.
  4. On the incoming E, switch the receiver and unclamp 104 after 24T.
- **Notes to §8.**
  - Note 1: R5's highest rate may be lower because of the line or because the rate is disabled.
  - Note 2: on cleardown or no common rate, "repeating the transmission of sequence E for not less than 64T
    before clearing".
  - Note 3: a V.32 far end "uses B4 for other purposes".
- **Receiver requirements [derived]:**
  - The data-mode receiver must hold lock through 64T of AA/CC or AC/CA. These are four-point signals on the A/C
    axis at the training states' level.
  - At 9600 trellis, 12 000 and 14 400, A is not a data point, so a run of A-position symbols is unmistakable. At
    7200 and 4800 it is a data point.
  - A retrain request (AA or AC for more than 128T) and a preamble (the same signal for 56T, then a reversal) are
    told apart only by the reversal at 56T. The receiver has to watch for it rather than wait out 128T.

---

## 6. What the Recommendations require of, or leave to, the receiver

### 6.1 Required or recommended

- Operate with ±7 Hz carrier offset (V.32 §2.1 p.1; bis §2.1 p.1).
- Accept 2400 baud ± 0.01 % (V.32 §2.3 p.2; bis §2.1 p.1). **[derived]** That allows up to 2×10⁻⁴ between the
  two ends' clocks.
- Tone detectors:
  - 600 ± 7 Hz or 3000 ± 7 Hz, with phase reversals, at the call end (§5.4.1).
  - 1800 ± 7 Hz for 64T, a reversal in it, and an amplitude drop, at the answer end (§5.4.2).
  - The same tones for more than 128T in data, as retrain requests (§5.5.1, §5.5.2).
- Turnaround 64 ± 2T, reversal received to reversal sent, measured at the line (§5.4.1, §5.4.2).
- Detect S (§5.4.1, §5.4.2) and keep it apart from the optional EC sequence (§5.4 Note 3).
- Train the receiver within the far end's conditioning signal. That is S 256T + S̄ 16T + TRN ≥ 1280T, so the
  guaranteed minimum is 1552T, 646.7 ms (§5.2).
- Detect two consecutive identical rate sequences (§5.3.1), and E (§5.3.2).
- Change the receive rate and coding on detecting E (§5.4.1, §5.4.2; bis §8.1, §8.2).
- In V.32bis data mode, detect a renegotiation preamble at any time (bis §8.2).
- Echo cancellation in each modem, trained during the half-duplex part of the start-up (§1 b), §5.2.3, §5.4).
  TRN is extendable for it, and G.165 network echo cancellers need 650 ms (Note 4).
- The answer end must not disconnect within 3 s of AC for want of 1800 Hz (Note 5).
- The caller must tolerate up to 3050 ms (V.32) or 3100 ms (bis) of "special pattern" before the tones
  (Note 2).
- More equalisation may be needed on G.235 connections (§1 Note 1).

### 6.2 Not specified

The receiver designer is free on all of these:

- the pulse shape or roll-off (only 2 to 7 dB at 600/3000 Hz is fixed);
- the receive level range;
- the maximum round trip;
- any echo-canceller performance or return-loss figure;
- the equaliser type and length;
- the decision method, and the trellis decoder and its depth (§2.5);
- what counts as "unsatisfactory signal reception";
- any time limit on waiting for R3 (§5.4.1 says only "until an incoming rate signal R3 is detected");
- the initial Y1Y2 of the Table 2 differential encoder at B1;
- the level of data relative to training, except as the figures imply (§2.1).

### 6.3 Durations in milliseconds, at 2400 baud

| Item | Symbols | Duration |
|---|---|---|
| Turnaround | 64 ± 2T | 26.7 ± 0.8 ms |
| S | 256T | 106.7 ms |
| S̄ | 16T | 6.7 ms |
| TRN | 1280–8192T | 533 ms – 3.41 s |
| One rate sequence, or E | 8T | 3.3 ms |
| B1 | 128T | 53.3 ms |
| Retrain tone threshold | > 128T | 53.3 ms |
| 1800 Hz detection | 64T | 26.7 ms |
| bis preamble | 56T + 8T | 23.3 + 3.3 ms |
| R4 / R5 | ≥ 64T / 64T | 26.7 ms |
| bis B1 and unclamp | 24T | 10 ms |
| Retrain AA/AC before 109 may go OFF | — | 45 s |

### 6.4 Level of the start-up signals [derived from §2.2 and the state positions]

- **AA and CC** sit at the passband centre and carry the full state power, |A|² = 10.
- **AC and CA** put all their power at 600 and 3000 Hz, where the far transmitter is 2 to 7 dB down (§2.2). They
  arrive 2 to 7 dB below a broadband signal of the same constellation power.
- **S and S̄** put half their symbol power at 1800 Hz and half at the band edges.

So neither AC nor S gives an accurate gain. AA does (at 1800 Hz only), and TRN does over the whole band. The
answer end hears only AA before the caller's S, which gives carrier frequency and phase but no timing. The call
end hears AC, CA and AC for at least 128T + MT + 64T, which gives timing and carrier frequency long before its
first S.

---

## 7. How this document was produced

- Every page of both PDFs was rendered with PyMuPDF and read as an image. The tables, figures and signs were read
  from the images.
- For the four V.32bis constellations I also extracted each page's vector geometry: the dot centres, the
  label-text positions and the axis ticks. I set the scale from the ticks and the origin from each
  constellation's symmetry, then paired each label with its dot. Dot-to-grid residuals were ≤ 0.15 unit. For
  every label the paired dot was at least 3.5 times nearer than the next dot. No point was used twice.
- As a cross-check, I spot-read the outer points of each figure by eye against the image.
- The TRN vectors come from a direct implementation of §4 and §5.2.3, checked against §5.2.3's printed patterns.
- V.34 was not needed and was not consulted.
