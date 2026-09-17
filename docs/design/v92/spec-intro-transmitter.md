# V.92 digest, clauses 1-7: scope, definitions, digital modem, the analogue modem's PCM upstream transmitter, interchange circuits

Source: ITU-T V.92 (11/2000), `docs/specs/T-REC-V.92-200011-I.pdf`, PDF pages 8-13 (printed pages 1-6).
Every one of those pages was rendered and read as an image. The extracted text was used only to find things.

The references were also read from rendered pages:

- **V.92:** PDF 27 (Table 18, INFO1a), 29 (Ja rate mask), 30 (Ru/Su/TRN1u), 32 (Table 22, Jp), 33 (B1u, E2u, CPu), 36-38 (RM, RM', SUVu, TRN2u), 39-43 (CPd, Table 30).
- **V.90:** PDF 10-11 (clause 3, Table 1), 13-20 (clauses 5, 6 and 7, Table 6).
- **V.34:** PDF 16 (clause 7, the scrambler), 28-31 (9.6.3, Figure 9, Table 13, Figures 10-12), 33 (10.1.2.3.2, the CRC).

Where a figure or table matters, it was also zoomed to 250-320 dpi.

Conventions in this file:

- **SHALL** marks a mandatory requirement, **MAY** an option, and **SHOULD** a recommendation. **[derived]** marks my own deduction from the text. **[interpretation]** marks a reading the text does not state outright.
- T = 1/8000 s, one PCM symbol.
- "a:b" is a bit field written LSB:MSB. Bit a is first in time (the clause 8 convention, restated in section 8 below).
- `^` is XOR (modulo-2 addition).
- `eta` is a signed constellation-point index (the spec's Greek eta).
- Requirement IDs are `R-<clause>-<n>`.

The clauses I was given contain no timers. The only timing-related items are the 8000 symbol/s clock of 6.2 and the V.14 tolerances quoted under 7.2.

---

## 1 Scope (clause 1)

V.92 specifies a pair of different modems: a **digital modem** and an **analogue modem** (both defined in clause 3). It covers coding, start-up signals and sequences, operating procedures and the DTE-DCE interface. Two things are national matters and are not specified: the digital modem's network interface, and the rate at which the digital modem connects locally to the digital network.

Principal characteristics (clause 1, items a-k):

| Item | Characteristic |
|---|---|
| a | Duplex operation on the PSTN. |
| b | Channel separation by echo cancellation. |
| c | **PCM modulation in both directions at 8000 symbols/s.** New: V.90's upstream was V.34 QAM. |
| d | Downstream synchronous rates 28 000 to 56 000 bit/s in steps of 8000/6 bit/s (same as V.90). |
| e | **Upstream synchronous rates 24 000 to 48 000 bit/s in steps of 8000/6 bit/s.** New. |
| f | Adaptive techniques that get close to the channel's maximum rate. |
| g | V.34 modulation is negotiated upstream (or downstream) if the connection will not support PCM in that direction. |
| h | Rate sequences are exchanged during start-up to set the rate. |
| i | V.8 is used, and optionally V.8 bis, for start-up and mode selection. |
| j | Reduced start-up time on recognized connections (short Phase 1 and short Phase 2; clauses 8 and 9, not in this digest). |
| k | Modem-on-hold in response to call-waiting events or outgoing call requests (8.9, 9.10). |

- R-1-1 [interpretation]: item g lets each direction fall back to V.34 on its own. That gives up to four upstream/downstream combinations:
  - PCM/PCM;
  - V.34/PCM (V.90-like);
  - PCM/V.34 (item g allows it on its face; whether clause 9 really permits it belongs to the clause 9 digest);
  - V.34/V.34 (full V.34 fallback).

  INFO1a Table 18 (PCM upstream) and Table 19 (V.34 upstream) are how the analogue modem chooses the upstream mode in short Phase 2.

## 2 References (clause 2)

| Recommendation (edition cited) | In `docs/specs`? | Used by (in V.92) |
|---|---|---|
| G.711 (1988), PCM | no (Ucode table in V.90 Table 1 is used instead) | definitions 3.1/3.2, ANSpcm quantizing |
| V.8 (2000) | yes, `T-REC-V.8-200011-I.pdf` | Phase 1 |
| V.8 bis (2000) | yes, `T-REC-V.8bis-200011-I.pdf` | Phase 1 (optional), QC2a/QCA2a |
| V.14 (1993) | yes | 7.2 async conversion |
| V.21 (1988) | yes | short Phase 1 V.21(L)/V.21(H) |
| V.24 (2000) | yes | 7.1 circuit definitions |
| V.25 (1996) | yes | Phase 1 answer tone etc. |
| V.34 (1998) | yes | scrambler (6.3), inverse map (6.4.3), trellis (6.4.4), CRC, MD, V.34 fallback |
| V.42 (1996) | yes, but the 2002 edition (`T-REC-V.42-200203-I.pdf`) | 7.2, LAPM (P bit of QC signals) |
| V.43 (1998) | **no** | circuit 133 (Table 1, Note 2) |
| V.80 (1996) | **no** | 7.2 |
| V.90 (1998) | yes | clause 5 (whole digital modem encoder), Ucode, DIL, Ri, Sd, TRN1d, B1d, Rd/Rt |

V.44 is **not** among V.92's references, although 7.2 allows "data compression".

## 3 Definitions (clause 3)

- **3.1 Analogue modem.** In data mode, the modem that receives G.711 signals after they have passed through a G.711 decoder. Typically on the PSTN.
  - Changed from V.90 3.1: V.90's definition also said the analogue modem *generates V.34 signals*. V.92 drops that, because upstream can be PCM.
- **3.2 Digital modem.** In data mode, the modem that generates G.711 signals. It is connected to a digital switched network through a digital interface (e.g. BRI, PRI).
  - Changed from V.90: V.90 also said it *receives V.34 signals passed through a G.711 encoder*. That is dropped.
- **3.3 Downstream.** Digital modem to analogue modem.
- **3.4 Nominal transmit power.** The user-configured reference transmit power.
- **3.5 Qa.b format.**
  - *Signed* Qa.b is an (a+b+1)-bit two's-complement number with b bits after the binary point, in the range [-2^a, 2^a).
  - *Unsigned* Qa.b is an (a+b)-bit number with b fractional bits. The printed range is **[0, 2^(a+1))**. The superscript "a + 1" was checked again at 320 dpi; the extracted text mangles it to "2a + 1".
  - See pitfall P-10: the bit patterns used elsewhere show an unsigned range of [0, 2^a). For example, CPu's "unsigned Q3.13 (xxx.xxxxxxxxxxxxx)" is 16 bits wide with 3 integer bits.

  Formats used by the transmitter parameters:

  | Format | Width | Pattern as printed | Value = raw / | Range |
  |---|---|---|---|---|
  | signed Q0.15 (z1, z2) | 16 | s.xxxxxxxxxxxxxxx | 32768 | [-1, 1) |
  | signed Q1.14 (p1, p2) | 16 | sx.xxxxxxxxxxxxxx | 16384 | [-2, 2) |
  | unsigned Q0.16 (4G) | 16 | .xxxxxxxxxxxxxxxx | 65536 | [0, 1), so G is in [0, 0.25) |
  | signed Q1.6 (a1, a2, b1, b2 in CPu) | 8 | sx.xxxxxx | 64 | [-2, 2) (V.90 limits the magnitudes to at most 1) |
  | signed Q2.2 (SUVu bits 27:31) | 5 | sxx.xx | 4 | [-4, 4) |
  | unsigned Q3.13 (CPu bits 52:67) | 16 | xxx.xxxxxxxxxxxxx | 8192 | [0, 8) |

- **3.6 Ucode.** As in V.90 clause 3. V.90 3.6 (rendered) says:
  - A Ucode is a universal code, 0..127, that names both a mu-law and an A-law PCM codeword. Table 1/V.90 lists each Ucode's mu-law octet, A-law octet (hex, all G.711 modifications already applied) and a linear value. The MSB of the octet is the G.711 polarity bit.
  - Uchords (V.90 3.5): Uchord1 is Ucodes 0-15, Uchord2 is 16-31, ..., Uchord8 is 112-127. V.92 does not redefine Uchord but uses it in CPt/CPu.
  - Sample rows (rendered): Ucode 0 is mu FF, linear 0 / A D5, linear 8. Ucode 66 is mu BD, 2236 / A 97, 2368. Ucode 103 is mu 98, 11900 / A B2, 12032.
  - The repo already carries this table: `crates/datapump/src/v90/ucode.rs` (`linear`, `octet`, `from_octet`, `nearest`).
- **3.7 Upstream.** Analogue modem to digital modem.
- **3.8 LU.** Set so that TRN1u is transmitted at the desired data-mode transmit power. TRN1u is a sequence of +/-LU (8.5.7), so LU^2 is the desired data-mode power.

## 4 Abbreviations (clause 4)

- BRI: Basic Rate Interface
- DCE: Data Circuit-terminating Equipment
- DIL: Digital Impairment Learning sequence
- DTE: Data Terminal Equipment
- PRI: Primary Rate Interface
- PSTN: Public Switched Telephone Network
- RTDEd: Round-Trip Delay Estimate, digital modem. Defined procedurally in 9.4.1.1.4: the interval between the Tone B phase reversal appearing at the digital modem's line terminals and the received Tone A phase reversal, minus 40 ms.

Signal names (CPd, TRN1u, B1u, ...) are not abbreviations here. They are defined in clause 8.

---

## 5 Digital modem (clause 5), which is V.90 clause 5

- **R-5-1 SHALL:** the digital modem's data signalling rates, symbol rate, scrambler and encoder are those of V.90 clause 5.

So V.92's downstream is V.90's downstream, unchanged. The repo already implements this: `crates/datapump/src/v90/{encoder.rs, modulus.rs, sign.rs, ucode.rs, digital.rs}`. Below is what V.90 clause 5 requires, read from the rendered V.90 pages 13-18.

### 5.1/V.90 Data signalling rates

- SHALL support synchronous rates 28 000 to 56 000 bit/s in steps of 8000/6.
- The rate is set in Phase 4 (V.90 9.4; in V.92 the procedure is 9.6).
- V.92 carries the chosen downstream rate in CPu bits 21:25 as `drn`, 0..22, with rate = (drn + 20) x 8000/6. drn = 0 means cleardown, drn = 1 means 28 000 and drn = 22 means 56 000.

### 5.2/V.90 Symbol rate

- The downstream symbol rate SHALL be 8000, timed from the digital network interface.
- In V.90, the digital modem SHALL support upstream (V.34) symbol rates 3000 and 3200 and MAY support 3429.
- [interpretation] In V.92 that upstream clause matters only when V.34 upstream is chosen (INFO1a Table 19 bits 34:36 carry 3..5, meaning 3000..3429).

### 5.3/V.90 Scrambler

- SHALL be the self-synchronizing scrambler of V.34 clause 7 with **GPC = 1 + x^-18 + x^-23** (equation 7-1/V.34), whatever the call direction. V.92 8.6 repeats this for Jd, Jp, Jp', SCR and TRN1d.
- Contrast: the analogue modem always uses GPA (6.3 below).

### 5.4/V.90 Encoder

**Data frame.** 6 symbols, with intervals i = 0..5 and i = 0 first. Frame sync is set up during training.

**Mapping parameters (5.4.1).** They are set during training or rate renegotiation:

- six PCM code sets C0..C5, where set Ci has Mi members;
- K, the number of modulus-encoder bits per frame;
- Sr, the number of sign bits per frame spent on spectral-shaping redundancy;
- S, the number of sign bits per frame that carry data, with **S + Sr = 6**.

**Table 2/V.90 (rendered), written as a rule.** The valid data-mode (K, S) pairs are all pairs with 15 <= K <= 39, 3 <= S <= 6 and 21 <= K + S <= 42. The rate is (K + S) x 8000/6. Checked row by row:

- K=15 allows only S=6.
- K=16 allows S 5..6; K=17 allows S 4..6.
- K=18..36 allow S 3..6.
- K=37 allows S 3..5; K=38 allows S 3..4; K=39 allows only S=3.

V.90 Table 17 lists the pairs valid during Phase 4 and rate renegotiation; that table was not read for this digest.

**Input bit parsing (5.4.2).** D = S + K serial data bits d0..dD-1, d0 first:

- d0..dS-1 become the sign-input bits s0..sS-1;
- dS..dD-1 become the modulus bits b0..bK-1.

**Modulus encoder (5.4.3).**

- Mi is the number of *positive* levels in interval i's constellation, as signalled by the analogue modem in CP (V.92: CPu, Table 23).
- SHALL satisfy 2^K <= product of M0..M5.
- Algorithm:
  - R0 = b0 + b1·2 + ... + bK-1·2^(K-1);
  - Ki = Ri mod Mi (0 <= Ki < Mi) and Ri+1 = (Ri - Ki)/Mi, for i = 0..5;
  - K0 goes to interval 0.
- NOTE: other implementations are allowed, but the mapping must be identical.

**Mapper (5.4.4).**

- Ci holds interval i's Mi positive PCM codes. "Largest" and "smallest" PCM code mean largest and smallest Ucode.
- Labels run in **descending** order: label 0 is the largest code in Ci and label Mi-1 is the smallest.
- Ui is the code labelled Ki.

**Spectral shaping (5.4.5).**

- SHALL be applied if enabled. It changes only sign bits: in each 6-symbol frame, Sr sign bits are redundancy and S carry data.
- Sr is chosen by the analogue modem, and Sr = 0 disables shaping. In V.92, Sr is carried in CPu bits 31:32.
- NOTE: the shaper's initial state is left to the implementer.

*5.4.5.1, Sr = 0, S = 6.*

```
$0 = s0 ^ ($5 of the previous data frame)
$i = si ^ $(i-1),  i = 1..5
```

*5.4.5.2-5.4.5.4, Sr = 1, 2, 3.* Sign bits are parsed into shaping frames (Table 3), the odd bits are differentially encoded (Table 4), and then a second differential encoding produces t:

- Sr=1 (one 6-bit frame j): `tj(k) = p'j(k) ^ tj-1(k)`. Then tj(k) becomes $k.
- Sr=2 (two 3-bit frames j, j+1): `tj(k) = p'j(k) ^ tj-1(k)` and `tj+1(k) = p'j+1(k) ^ tj(k)`. Then tj(k) becomes $k and tj+1(k) becomes $(k+3).
- Sr=3 (three 2-bit frames): `tj(k)=p'j(k)^tj-1(k)`, `tj+1(k)=p'j+1(k)^tj(k)` and `tj+2(k)=p'j+2(k)^tj+1(k)`. Then these become $k, $(k+2) and $(k+4).

Table 3/V.90, parsing sign bits into shaping frames (rendered):

| interval | Sr=1, S=5 | Sr=2, S=4 | Sr=3, S=3 |
|---|---|---|---|
| 0 | pj(0)=0 | pj(0)=0 | pj(0)=0 |
| 1 | pj(1)=s0 | pj(1)=s0 | pj(1)=s0 |
| 2 | pj(2)=s1 | pj(2)=s1 | pj+1(0)=0 |
| 3 | pj(3)=s2 | pj+1(0)=0 | pj+1(1)=s1 |
| 4 | pj(4)=s3 | pj+1(1)=s2 | pj+2(0)=0 |
| 5 | pj(5)=s4 | pj+1(2)=s3 | pj+2(1)=s2 |

Table 4/V.90, odd-bit differential coding (rendered):

| interval | Sr=1 | Sr=2 | Sr=3 |
|---|---|---|---|
| 0 | p'j(0)=0 | p'j(0)=0 | p'j(0)=0 |
| 1 | p'j(1)=pj(1)^p'j-1(5) | p'j(1)=pj(1)^p'j-1(1) | p'j(1)=pj(1)^p'j-1(1) |
| 2 | p'j(2)=pj(2) | p'j(2)=pj(2) | p'j+1(0)=0 |
| 3 | p'j(3)=pj(3)^p'j(1) | p'j+1(0)=0 | p'j+1(1)=pj+1(1)^p'j(1) |
| 4 | p'j(4)=pj(4) | p'j+1(1)=pj+1(1)^p'j(1) | p'j+2(0)=0 |
| 5 | p'j(5)=pj(5)^p'j(3) | p'j+1(2)=pj+1(2) | p'j+2(1)=pj+2(1)^p'j+1(1) |

Table 5/V.90, shaping-frame bit to PCM sign (rendered):

| interval | Sr=1 | Sr=2 | Sr=3 | sign |
|---|---|---|---|---|
| 0 | tj(0) | tj(0) | tj(0) | $0 |
| 1 | tj(1) | tj(1) | tj(1) | $1 |
| 2 | tj(2) | tj(2) | tj+1(0) | $2 |
| 3 | tj(3) | tj+1(0) | tj+1(1) | $3 |
| 4 | tj(4) | tj+1(1) | tj+2(0) | $4 |
| 5 | tj(5) | tj+1(2) | tj+2(1) | $5 |

*5.4.5.5 Spectral shaper.*

- Figure 2/V.90 is a 2-state trellis:
  - from Q=0: rule A leads to Q=0 and rule B to Q=1;
  - from Q=1: rule C leads to Q=0 and rule D to Q=1.
- Rules:
  - A: no change;
  - B: invert every sign in the shaping frame;
  - C: invert the even-numbered bits tj(0), tj(2), ...;
  - D: invert the odd-numbered bits tj(1), tj(3), ....
- The look-ahead ld is 0..3 and is chosen by the analogue modem (V.92: CPu bits 49:50). ld 0 and 1 are mandatory in the digital modem; 2 and 3 are optional. In V.92, Jd (Table 21) tells the analogue modem the digital modem's maximum look-ahead (1..3).
- For frame j, the shaper SHALL do the following:
  - evaluate every allowed rule sequence for frames j..j+ld, starting from Qj and using the mapper magnitudes of those frames;
  - pick the rule for frame j that minimizes w[n] up to and including the last symbol of frame j+ld;
  - update Q and set the signs.

*5.4.5.6 Spectral shape filter.*

- T(z) = (1 - a1 z^-1)(1 - a2 z^-1) / ((1 - b1 z^-1)(1 - b2 z^-1)), and F(z) = 1/T(z).
- The parameters satisfy |a1|, |a2|, |b1|, |b2| <= 1. Each is 8-bit two's complement with 6 fractional bits.
- x[n] is signed and proportional to the linear value (V.90 Table 1) of the PCM code sent.
- The metric:

```
y[n] = x[n] - b1·x[n-1] + a1·y[n-1]
v[n] = y[n] - b2·y[n-1] + a2·v[n-1]
w[n] = v[n]^2 + w[n-1]
```

**Sign assignment (5.4.6).** Sign bit 0 means a **negative** voltage; 1 means positive.

**Mux (5.4.7).** PCM0 is sent first.

---

## 6 Analogue modem (clause 6), the PCM upstream transmitter

Clause 6 describes the analogue modem when PCM upstream is in use. [interpretation] When V.34 upstream is negotiated instead, V.90 clause 6 describes the analogue modem (see section 6.6 below).

### 6.1 Data signalling rates

- **R-6.1-1 SHALL:** transmit synchronously at 24 000 to 48 000 bit/s in steps of 8000/6 bit/s.
- **R-6.1-2 SHALL:** the rate is set during Phase 4 by the procedure of 9.6.
- [derived] That gives 19 rates. CPd bits 22:26 carry `drn`, 0..19 (drn = 0 means cleardown), and rate = (drn + 17) x 8000/6.
- An upstream data frame is 12 symbols, so the bits per frame are **K = 12·rate/8000 = 2·(drn + 17)**:

  | drn | rate (bit/s) | K | | drn | rate | K |
  |---|---|---|---|---|---|---|
  | 1 | 24 000 | 36 | | 11 | 37 333 | 56 |
  | 2 | 25 333 | 38 | | 12 | 38 666 | 58 |
  | 3 | 26 666 | 40 | | 13 | 40 000 | 60 |
  | 4 | 28 000 | 42 | | 14 | 41 333 | 62 |
  | 5 | 29 333 | 44 | | 15 | 42 666 | 64 |
  | 6 | 30 666 | 46 | | 16 | 44 000 | 66 |
  | 7 | 32 000 | 48 | | 17 | 45 333 | 68 |
  | 8 | 33 333 | 50 | | 18 | 46 666 | 70 |
  | 9 | 34 666 | 52 | | 19 | 48 000 | 72 |
  | 10 | 36 000 | 54 | | | | |

- The analogue modem announces the upstream rates it supports in the Ja DIL descriptor (Table 20, rendered). It is a 16-bit mask for 24 000 .. 44 000 plus 3 bits for 45 333, 46 666 and 48 000; the remaining 13 bits are reserved (0).

### 6.2 Symbol rate and transmit timing

- **R-6.2-1 SHALL:** the upstream symbol rate is **8000 symbols/s derived from the digital network**. Clause 6 gives no frequency tolerance.
- [derived] The only view the analogue modem has of the network clock is the downstream PCM signal. So its transmit D/A clock (or its transmit resampler) must be slaved to the recovered downstream symbol clock. The upstream samples arrive at the central-office A/D, which samples on the network clock; any residual frequency offset appears there as a drifting sampling phase.
- Related requirements elsewhere (not in clauses 1-7, listed so the transmitter is built to meet them):
  - 8.6.3: the digital modem cannot change the CO A/D sampling phase. It uses **Jp bits 18:33** (16-bit unsigned, covering [0, 1) symbol, i.e. [0, T)) to tell the analogue modem how much to extend S̄u at the Jp-to-Jp' transition. **The analogue transmitter therefore needs a fractional-symbol delay adjustment with 1/65536 T resolution.** The procedure is in 9.5.
  - 8.5.7: TRN1u segments are multiples of 12 symbols. The digital modem keeps data-frame-interval alignment from the first symbol of the second TRN1u.
  - 8.7.1: the first symbol of B1u begins data frame interval 0 and is n = 0 in the 6.4.2 equations.
  - 8.7.2: E2u is one symbol longer if CPd bit 29 is set (never during rate renegotiation or fast parameter exchange).

### 6.3 Scrambler

- **R-6.3-1 SHALL:** the self-synchronizing scrambler of V.34 clause 7 with **GPA = 1 + x^-5 + x^-23** (equation 7-2/V.34). The analogue modem uses GPA **regardless of which modem placed the call**; in V.34 itself, GPA is the answer modem's polynomial.
- What V.34 clause 7 says (rendered): the transmitter divides the data sequence by the generating polynomial, and the quotient coefficients in descending order are the scrambler output. So, per bit:

  ```
  out(n) = in(n) ^ out(n-5) ^ out(n-23)        // scrambler (divide)
  in(n)  = rx(n) ^ rx(n-5) ^ rx(n-23)          // descrambler (multiply), digital modem side
  ```

  Repo: `v32::Scrambler::new(Mode::Answer)` (taps 5 and 23; see `crates/datapump/src/v32.rs` around line 385). `scramble` divides and `descramble` multiplies.
- 6.4.1 says the modulus encoder's input bits b0..bK-1 are *scrambled* bits.
- Initialization rules (from clauses 8 and 9):
  - SHALL be zero before TRN1u (8.5.7);
  - SHALL be reset at the start of TRN2u (8.7.6);
  - zeroed before B1u, together with every other transmitter memory (8.7.1);
  - zeroed at a fast parameter exchange before SUVu (9.9.2.1.2, 9.9.2.2.3);
  - [interpretation] otherwise free-running.
- Polarity conventions differ between the two modems (pitfall P-6):
  - TRN1u: scrambler output 0 is **+LU** and 1 is **-LU** (8.5.7).
  - Digital-modem sign bits: 0 is **negative** (V.90 5.4.6; V.92 8.6.2).

### 6.4 Transmitter

- **R-6.4-1 SHALL:** the transmitter's framing is based on Figure 1.

#### Figure 1/V.92, framing (rendered)

| data frame interval i | 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | 11 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| constellation frame index j | 0 | 1 | 2 | 3 | 4 | 5 | 0 | 1 | 2 | 3 | 4 | 5 |
| trellis frame index k | 0 | 1 | 2 | 3 | 0 | 1 | 2 | 3 | 0 | 1 | 2 | 3 |

- **Data frame:** 12 symbols (1.5 ms). It carries K bits and has 12 moduli M0..M11.
- **Constellation frame:** 6 symbols. The constellation set in use depends on j = i mod 6 (CPd gives one set index per pair (i, i+6)).
- **Trellis frame:** 4 symbols, one 4D symbol. There are 3 per data frame.
- [derived] With n counting symbols from the first symbol of B1u (n = 0), i = n mod 12, j = n mod 6 and k = n mod 4.

#### Figure 2/V.92, block diagram (rendered)

```
 b0:bK-1 --> [Modulus encoder] --Ki--> [precoder] --x(n)--> [prefilter] --> G·v(n)
                    ^                    ^    |
                    Mi                   Y0   y(n)
                                         |    v
                               [Conv. encoder] <--Y1:Y4-- [Inverse map]
```

The precoder takes Ki and Y0. It outputs x(n) to the prefilter and the chosen point index y(n) to the inverse map. The inverse map turns y(0..3) of a trellis frame into Y1..Y4. The convolutional encoder turns those into the Y0 used by the next trellis frames.

#### Transmitter parameters and where they come from

All of these come from **CPd** (8.8.3, Table 30), except the capability limits, which the analogue modem itself announced in **INFO1a** (Table 18). Integers and Q values are sent LSB first (clause 8 preamble).

Table 30/V.92, CPd (rendered). Positions assume all parts are present.

- **Coefficient words:** α = 17·(LZ1 + LP1 + LZ2 + LP2).
- **Constellation-point words:** β = 17·(LC1 + ... + LC6).
- **Word format:** every field below the header is a 17-bit word: a start bit (0) followed by 16 bits.

| bits | field |
|---|---|
| 0:16 | frame sync, 17 ones |
| 17 | start bit 0 |
| 18 | CPd: 0 |
| 19 | 1 = modulus-encoder parameters present |
| 20 | 1 = prefilter and precoder coefficients present |
| 21 | 1 = constellation sets present |
| 22:26 | drn, 0..19 (0 = cleardown SHALL); upstream rate = (drn+17)·8000/6 |
| 27:28 | trellis encoder: 0 = 16-state, 1 = 32-state, 2 = 64-state, 3 = reserved. The digital modem *requires* the analogue transmitter to use it. |
| 29 | extend E2u by 1 symbol (1) or not (0); SHALL be 0 in rate renegotiation and fast parameter exchange |
| 30:32 | reserved, 0 |
| 33 | acknowledge (CPu received) |
| 34 | start bit 0 |
| 35:50 | **4·G > 0**, unsigned Q0.16 (G is the gain at the prefilter output) |
| *modulus-encoder part* | |
| 51 | start 0 |
| 52:59, 60:67 | M0, M1 (8 bits each) |
| 68 | start 0 |
| 69:76, 77:84 | M2, M3 |
| 85 | start 0 |
| 86:93, 94:101 | M4, M5 |
| 102 | start 0 |
| 103:110, 111:118 | M6, M7 |
| 119 | start 0 |
| 120:127, 128:135 | M8, M9 |
| 136 | start 0 |
| 137:144, 145:152 | M10, M11 |
| *coefficient part* | |
| 153 | start 0 |
| 154:162 | LZ1 (9 bits), taps in the precoder feed-forward section, <= Lmax |
| 163:169 | reserved 0 |
| 170 | start 0 |
| 171:179 | LP1 (9 bits), taps in the precoder feedback section, <= Lmax |
| 180:186 | reserved 0 |
| 187 | start 0 |
| 188:196 | LZ2 (9 bits), taps in the prefilter feed-forward section, <= Lmax |
| 197:203 | reserved 0 |
| 204 | start 0 |
| 205:213 | LP2 (9 bits), taps in the prefilter feedback section, <= Lmax |
| 214:220 | reserved 0 |
| 221 / 222:237 | start / z1(1), signed Q0.15 (only if LZ1 > 0); then z1(2)..z1(LZ1), one word each |
| 221+17·LZ1 / 222+17·LZ1 : 237+17·LZ1 | start / p1(1), signed Q1.14; then p1(2)..p1(LP1) |
| 221+17·(LZ1+LP1) / ... | start / z2(0)..z2(LZ2-1), signed Q0.15 |
| 221+17·(LZ1+LP1+LZ2) / ... | start / p2(1)..p2(LP2), signed Q1.14 (only if LP2 > 0) |
| *constellation part* | |
| 221+α | start 0 |
| 222+α : 225+α | constellation set index (0..5) for intervals 0 and 6 |
| 226+α : 229+α | index for intervals 1 and 7 |
| 230+α : 233+α | index for intervals 2 and 8 |
| 234+α : 237+α | index for intervals 3 and 9 |
| 238+α | start 0 |
| 239+α : 242+α | index for intervals 4 and 10 |
| 243+α : 246+α | index for intervals 5 and 11 |
| 247+α : 254+α | reserved 0 |
| 255+α | start 0 |
| 256+α : 263+α | LC1, number of **positive** points in set 1 |
| 264+α : 271+α | LC2 (may be 0) |
| 272+α | start 0 |
| 273+α : 280+α / 281+α : 288+α | LC3 / LC4 (may be 0) |
| 289+α | start 0 |
| 290+α : 297+α / 298+α : 305+α | LC5 / LC6 (may be 0) |
| 306+α / 307+α : 322+α | start / linear value of set 1's **smallest-magnitude** point (16 bits) |
| ... | one word per point, ascending magnitude, up to the largest point of set 1; then the other non-empty sets in the same format |
| 306+α+β | start 0 |
| 307+α+β : 322+α+β | CRC (V.34 10.1.2.3.2) |
| 323+α+β | fill 0 |
| 324+α+β : ... | fill 0s up to the next multiple of 6 symbols |

CPd rules from 8.8.3:

- Bits 0..50 are always sent. When a part is absent, its bits are **removed**, so later fields move up and **the frame must be parsed sequentially**.
- The modulus part is 6 words (102 bits), the coefficient part is 4 + ΣL words, and the constellation part is 5 + ΣLC words.
- SHALL: LZ1 + LP1 + LZ2 + LP2 <= Ltot (INFO1a bits 14:15).
- SHALL: constellations do not contain the zero point.
- SHALL: all non-empty sets are listed first.
- SHALL: a set has at most 128 points (see open question Q-5 on what "points" means).
- SHALL (digital modem design): the digital modem assumes that when G·v(n) has a mean square of 1, the analogue modem transmits at the desired power.
- SHALL: CPd sequences sent as a group carry identical information.
- NOTE (SHOULD, for the digital modem): design the precoder coefficients assuming the analogue modem **minimizes the power at the precoder output symbol by symbol**.
- CRC (V.34 10.1.2.3.2, rendered):
  - Generator x^16 + x^12 + x^5 + 1, register preset to all ones.
  - It covers every information bit except frame sync, start bits and fill bits.
  - The register is output starting from its bit 0; CRC bit 0 is the LSB.
  - Repo: the V.34 INFO/MP code already implements it (`crates/datapump/src/v34/info.rs`, `mp.rs`).

INFO1a capability fields sent by the analogue modem (Table 18, rendered). The transmitter must be able to do what it announces:

| INFO1a bits | meaning |
|---|---|
| 12:13 | filter sections supported: 0 = p1, z2; 1 = z1, p1, z2; 2 = p1, p2, z2; 3 = z1, p1, p2, z2 |
| 14:15 | Ltot = max LZ1+LP1+LZ2+LP2: 0 = 192, 1 = 256, 2 = 320, 3 = 384 |
| 16:17 | Lmax = max per section: 0 = 128, 1 = 192, 2 = 256, 3 = 320 |
| 34:36, 37:39 | the integer 6 (8000 symbols/s) for the analogue modem and the digital modem |

[derived] The minimum structure every PCM-upstream analogue modem supports is **p1 plus z2** (at least 192 coefficients in total, 128 per section). [derived] If the modem announced 0, a conforming CPd has LZ1 = 0 and LP2 = 0.

#### 6.4.1 Modulus encoder

- **R-6.4.1-1:** in each data frame, K scrambled bits b0..bK-1 enter the encoder, b0 first in time, along with M0..M11.
- **R-6.4.1-2 SHALL:** 2^K <= M, where M = M0·M1·...·M11.
- **R-6.4.1-3:** the encoder turns the K bits into K0..K11. NOTE: any implementation is allowed, provided the mapping is identical to the algorithm below.

With f the data-frame index:

1. R = b0 + b1·2^1 + ... + bK-1·2^(K-1). The first bit in time is the LSB.
2. Sign of R: **s(f) = 0 if R <= (M-1)/2; s(f) = 1 if R > (M-1)/2.** In integer form, s = (2R > M-1).
3. Differential encoding: **d(f) = s(f) ^ d(f-1).**
4. **R0 = R if d(f-1) = 0; R0 = M - 1 - R if d(f-1) = 1.** The rendered page and a 300 dpi zoom both show **d(f-1)** here, not d(f); see Q-1.
5. Ki = Ri mod Mi (0 <= Ki < Mi) and Ri+1 = (Ri - Ki)/Mi, for i = 0..11. Dividing R0 by M0 gives K0 as the remainder and R1 as the quotient, and so on for the other eleven intervals.
6. K0..K11 are the outputs; Ki belongs to data frame interval i.

Notes:

- [derived] M can reach 255^12 (about 2^96) and K can reach 72, so **use u128**. The repo's V.90 `modulus::encode` works on 6 `u16` moduli and needs a 12-interval u128 variant.
- [interpretation] d(-1) = 0: 8.7.1 zeroes the "modulus encoder memories" before B1u, and 9.9.2 zeroes "the scrambler and differential encoder" at a fast parameter exchange.
- [derived] Decoder (for our own digital modem):
  - rebuild R0 from the Ki as R0 = Σ Ki·Π(j<i) Mj;
  - then `R = (d_prev == 0) ? R0 : M-1-R0`, `s = (2R > M-1)`, `d = s ^ d_prev`;
  - keep d from R, not from R0's half: they differ when M is odd and R = (M-1)/2.
  - A simulation of 2000 random frames, for M even and M odd, decoded every frame correctly.
- [derived] Why the step exists: inverting the whole received sequence (every point index eta becomes -eta-1) maps every Ki to Mi-1-Ki, hence R0 to M-1-R0. The differential "sign" then makes decoding immune to that inversion (checked numerically, with the decoder's d started inverted). The trellis parity rule of 6.4.2 is also consistent with it (see 6.4.3).

#### 6.4.2 Precoder and prefilter

- **R-6.4.2-1:** the precoder takes Ki from the modulus encoder and Y0 from the convolutional encoder. For each Ki it forms the equivalence class E(Ki), **selects one point u(n) from E(Ki)**, and reports that point's index as **y(n)**.

**Constellation for the current interval i** (from CPd):

- Let c = set index for (i mod 6), LC = LC(c+1) and **N = 2·LC**. N is "one of 2·LC1 .. 2·LC6".
- The N points are a(eta) with -N/2 <= eta < N/2, **indexed in level order**: negative points have negative indices, positive points have non-negative indices.
- [interpretation] Only the LC positive magnitudes P[0] < P[1] < ... < P[LC-1] are sent (smallest first), so:

  ```
  a(eta) =  P[eta]        for 0 <= eta < LC
  a(eta) = -P[-eta-1]     for -LC <= eta < 0      // symmetric; a(-1) = -P[0]
  ```

  In the equations, u(n) is the level a(eta) and y(n) is eta.

**Equivalence classes** (rendered and zoomed; Mi is the modulus of the current interval i, and k = i mod 4):

```
k = 0, 1, 2 :  E(Ki) = { a(eta_k) : eta_k = Ki + z·Mi,                                  z integer }
k = 3       :  E(Ki) = { a(eta_k) : eta_k = 2·Ki + 2·z·Mi + ((eta_0 + eta_1 + eta_2 + Y0) mod 2), z integer }
```

- Here eta_0, eta_1 and eta_2 are the indices already chosen for k = 0, 1, 2 of **the same trellis frame**, and Y0 is the convolutional encoder output for that frame. Use a non-negative mod 2; indices can be negative.
- Only indices inside [-N/2, N/2) exist.
- [derived] eta_3 ≡ (eta_0 + eta_1 + eta_2 + Y0) (mod 2), so **eta_0 + eta_1 + eta_2 + eta_3 ≡ Y0 (mod 2)**. At k = 3 the class has period 2·Mi in the index.
- [derived] A class is non-empty for every Ki only if N >= Mi (k = 0..2) and **N >= 2·Mi (k = 3, i.e. i = 3, 7, 11)**. The spec states neither constraint; a transmitter should check both when CPd arrives.

**Precoder filter** (rendered and zoomed):

```
x(n) = u(n) + Σ_{κ=1..LZ1} u(n-κ)·z1(κ) + Σ_{κ=1..LP1} x(n-κ)·p1(κ)
```

**Prefilter** (rendered and zoomed). Note z2 starts at κ = 0 and ends at LZ2-1, while p2 starts at κ = 1:

```
v(n) = Σ_{κ=0..LZ2-1} x(n-κ)·z2(κ) + Σ_{κ=1..LP2} v(n-κ)·p2(κ)
```

- **R-6.4.2-2:** the transmitter's output is **G·v(n)**. G comes from CPd bits 35:50 as G = raw / 65536 / 4.

**Point selection.** The rule is the implementer's; 6.4.2 only says "selects".

- The CPd NOTE (8.8.3) says the digital modem designs its coefficients assuming the analogue modem minimizes the precoder output power symbol by symbol. Implement exactly that:

  ```
  c(n) = Σ_{κ=1..LZ1} u(n-κ)·z1(κ) + Σ_{κ=1..LP1} x(n-κ)·p1(κ)
  choose eta in the class (and in [-N/2, N/2)) minimizing |a(eta) + c(n)|
  u(n) = a(eta);  x(n) = u(n) + c(n);  y(n) = eta
  ```

- [derived] The levels rise with the index, so the best class member is one of the two members on either side of -c(n). Binary-search -c(n) among the levels, then step to the nearest class indices below and above.
- Tie-breaking is unspecified.
- There is no modulo operation. The only freedom is the choice within the class (N > Mi gives more than one candidate). When N = Mi and k != 3, the class has one member and x(n) is not controlled.

**Initialization.** The precoder memories u(n-κ) and x(n-κ) and the prefilter memories x(n-κ) and v(n-κ) are zero before B1u (8.7.1).

**Where it is used and bypassed:**

- SHALL bypass for Ru and R̄u (8.5.5), which use "the same structure used while transmitting the 2-point TRN1u signal".
- SHALL use the latest data-mode precoder and prefilter for RM and RM' (8.7.4).
- See also the table in "Use of the chain by other sequences" below.

#### 6.4.3 Inverse map

- **R-6.4.3-1:** in each trellis frame, the inverse map takes the pairs (y(0), y(1)) and (y(2), y(3)) and produces Y1, Y2, Y3 and Y4.
- **R-6.4.3-2:** it is identical to the symbol-to-bit converter of V.34 9.6.3.1. The odd-integer coordinates that V.34 uses are **2·y(k) + 1**.

So:

- 2D point m0 = (2·y(0)+1, 2·y(1)+1), which is V.34's y(2m);
- 2D point m1 = (2·y(2)+1, 2·y(3)+1), which is y(2m+1);
- their subset labels are s(2m) and s(2m+1) from Figure 9/V.34;
- **[Y4 Y3 Y2 Y1]** comes from Table 13/V.34.

**Figure 9/V.34, 3-bit subset label of odd-coordinate points (rendered at 300 dpi).** The labelling repeats under shifts of (4, 4) and (4, -4), and so under (8, 0) and (0, 8). These 16 points therefore define every point: reduce each coordinate modulo 8 into {-3, -1, 1, 3}.

| y \ x | -3 | -1 | 1 | 3 |
|---|---|---|---|---|
| 3 | 001 | 110 | 101 | 010 |
| 1 | 100 | 011 | 000 | 111 |
| -1 | 101 | 010 | 001 | 110 |
| -3 | 000 | 111 | 100 | 011 |

[derived] The same table indexed by point indices (x = 2·eta_a + 1, y = 2·eta_b + 1), where the label depends only on (eta_a mod 4, eta_b mod 4):

| eta_b mod 4 \ eta_a mod 4 | 0 | 1 | 2 | 3 |
|---|---|---|---|---|
| 0 | 000 | 111 | 100 | 011 |
| 1 | 101 | 010 | 001 | 110 |
| 2 | 100 | 011 | 000 | 111 |
| 3 | 001 | 110 | 101 | 010 |

[derived, checked numerically over ±130] A label's low bit equals (eta_a + eta_b) mod 2. So the k = 3 parity rule makes the low bits of s(2m) and s(2m+1) XOR to Y0, the same role U0 plays in V.34.

**Table 13/V.34, [Y4 Y3 Y2 Y1]** (rendered at 250 dpi). Rows are s(2m); columns are s(2m+1).

| s(2m) \ s(2m+1) | 000 | 001 | 010 | 011 | 100 | 101 | 110 | 111 |
|---|---|---|---|---|---|---|---|---|
| 000 | 0000 | 0000 | 0001 | 0001 | 1000 | 1000 | 1001 | 1001 |
| 001 | 0011 | 0010 | 0010 | 0011 | 1011 | 1010 | 1010 | 1011 |
| 010 | 0101 | 0101 | 0100 | 0100 | 1101 | 1101 | 1100 | 1100 |
| 011 | 0110 | 0111 | 0111 | 0110 | 1110 | 1111 | 1111 | 1110 |
| 100 | 1000 | 1000 | 1001 | 1001 | 0000 | 0000 | 0001 | 0001 |
| 101 | 1011 | 1010 | 1010 | 1011 | 0011 | 0010 | 0010 | 0011 |
| 110 | 1101 | 1101 | 1100 | 1100 | 0101 | 0101 | 0100 | 0100 |
| 111 | 1110 | 1111 | 1111 | 1110 | 0110 | 0111 | 0111 | 0110 |

Both tables match `crates/datapump/src/v34/trellis.rs` (`FIGURE_9`, `TABLE_13`, `label()`, `convert()`). Those can be reused as they are, fed with points (2·eta+1, 2·eta'+1).

#### 6.4.4 Convolutional encoder

- **R-6.4.4-1 SHALL:** use the V.34 convolutional encoders. The encoder takes Y1..Y4 from the inverse map and produces Y0 as in V.34 9.6.3.2, **with the 2T delays replaced by 4T delays**.
- **R-6.4.4-2:** the code is chosen by CPd bits 27:28 (0/1/2 = 16/32/64 states). V.34 9.6.3.2 says the receiving modem selects it.

What V.34 9.6.3.2 requires (rendered):

- Y1..Y4 go into one of the systematic encoders of Figures 10, 11 and 12, which are 16-state rate 2/3, 32-state rate 3/4 and 64-state rate 4/5.
- The 32-state code ignores Y3. The 16-state code ignores Y3 and Y4.
- There is an inherent delay of one 4D interval, so **Y0(m) does not depend on the current frame's inputs**.

[derived] In V.92 one 4D interval is one trellis frame (4T). So:

- the encoder is clocked once per trellis frame, after its four points are chosen;
- the Y0 used at k = 3 of trellis frame m is output(state), where the state holds the inputs of frames up to m-1;
- the state is zero before B1u (8.7.1), so the first trellis frame has Y0 = 0.

Next-state equations as read from the zoomed Figures 10-12. Let dq be the output of the q-th delay from the left, and dq' the value it latches.

```
16-state (Fig. 10; inputs as printed: Y2, Y2, Y1):
  d1' = d4
  d2' = d1 ^ Y2 ^ d4          // first adder also takes the fed-back output
  d3' = d2 ^ Y2
  d4' = d3 ^ Y1
  Y0  = d4

32-state (Fig. 11; inputs Y2, Y1, Y4, Y2):
  d1' = d5
  d2' = d1 ^ Y2
  d3' = d2 ^ Y1
  d4' = d3 ^ Y4
  d5' = d4 ^ Y2
  Y0  = d5

64-state (Fig. 12, traced wire by wire):
  a   = d1 ^ d2
  b   = d2 ^ Y1
  d1' = Y4 ^ a ^ (d3 & b)
  d2' = a ^ Y3 ^ (Y2 & d3) ^ d4
  d3' = b ^ d3
  d4' = d3
  d5' = d6
  d6' = d5 ^ Y2 ^ d3
  Y0  = d6
```

These are identical to `v34::trellis::Code::next/output`. That code has tests for free distance (16, 16, 20) and quarter-turn invariance, and it runs on live V.34 calls.

Parts of V.34 9.6.3 that V.92 does **not** bring in [interpretation]:

- the modulo encoder C0 (9.6.3.3);
- the superframe bit-inversion V0 (Table 12/V.34);
- the sum U0 = Y0 ^ C0 ^ V0 (equation 9-32).

6.4.4 cites only 9.6.3.2, and the V.92 precoder uses **Y0** directly. There is no superframe upstream.

#### Complete per-data-frame procedure (pseudocode)

```
state: scr (GPA), d_prev, conv_state, u_hist[LZ1], x_hist[max(LP1, LZ2-1)], v_hist[LP2], n
params: K, M[0..11], G, z1, p1, z2, p2, set_index[0..5], sets (P arrays), code
(all state zero, n = 0 at the first symbol of B1u)

for each data frame f:
    bits  = next K data bits, each scrambled with GPA        // b0 first
    R     = Σ bits[t] << t                                    // u128
    Mtot  = Π M[i]
    s     = (2R > Mtot-1)
    R0    = (d_prev == 0) ? R : Mtot-1-R
    d_prev = s ^ d_prev
    for i in 0..12: K[i] = R0 % M[i]; R0 /= M[i]
    for i in 0..12:                        // n = symbol count; i == n mod 12
        k = i % 4; P = sets[set_index[i % 6]]; N = 2·len(P)
        if k == 0: Y0 = conv_output(conv_state); etas = []
        c = Σ z1(κ)·u(n-κ) + Σ p1(κ)·x(n-κ)
        if k < 3: cand = { eta ≡ K[i] (mod M[i]) }
        else:     p = (etas[0]+etas[1]+etas[2]+Y0) & 1
                  cand = { eta ≡ 2·K[i] + p (mod 2·M[i]) }
        eta = argmin_{eta in cand, -N/2 <= eta < N/2} |a(eta) + c|
        u = a(eta); x = u + c
        v = Σ_{κ=0..LZ2-1} z2(κ)·x(n-κ) + Σ_{κ=1..LP2} p2(κ)·v(n-κ)
        emit LU · G · v                     // see "Output level"
        etas.push(eta); push histories; n += 1
        if k == 3:
            s0 = label(2·etas[0]+1, 2·etas[1]+1)
            s1 = label(2·etas[2]+1, 2·etas[3]+1)
            conv_state = conv_next(conv_state, TABLE_13[s0][s1] & code.inputs())
```

#### Use of the chain by other sequences (clauses 8 and 9, for context)

| Sequence (clause) | Chain used | Notes |
|---|---|---|
| TRN1u (8.5.7) | none: ±LU, sign from GPA output (0 is +, 1 is -) | scrambler SHALL be zero first; lengths are multiples of 12 symbols |
| CPt, E1u, Ja (8.5.1, 8.5.2, 8.5.4) | TRN1u modulation, bits scrambled then differentially encoded | differential memory starts from the last TRN1u symbol; Ja is a multiple of 12 bits |
| Ru / R̄u (8.5.5) | **SHALL bypass the precoder and prefilter**; {+L,+L,+L,-L,-L,-L} / its inverse (L = LU) | "same structure as 2-point TRN1u" |
| Su / S̄u (8.5.6) | {+a, 0, +a, -a, 0, -a} / its inverse, a = √(3/2)·LU | SHALL be a multiple of 12 symbols; precoder use not stated ([interpretation] bypassed, like Ru) |
| MD (8.5.3) | V.34 10.1.3.5 | |
| TRN2u (8.7.6) | 4-point {00: +1, 01: +3, 10: -1, 11: -3}·LU/√5, or 8-point {000..011: +1,+3,+5,+7; 100..111: -1,-3,-5,-7}·LU/√21 (rendered Tables 28 and 29; MSB is the sign) | chosen by Jp bit 48 (training) or 49 (rate renegotiation); scrambled ones; scrambler reset at start; sign bit differentially encoded (memory starts from E1u's last sign); multiple of 12 symbols; "may be used to estimate the analogue channel" ([interpretation] not precoded) |
| CPu, SUVu, E2u (8.7.2, 8.7.3, 8.7.5) | TRN2u modulation in training and rate renegotiation; **data-mode modulation in a fast parameter exchange** | E2u is one data frame of scrambled, differentially encoded zeros, plus 1 symbol if CPd bit 29 is set |
| **B1u** (8.7.1) | **full chain** with the latest CPd parameters | 48 data frames of scrambled ones. Scrambler, modulus encoder, convolutional encoder, precoder and prefilter memories are zeroed first. The first symbol is n = 0 and i = 0. |
| FB1u (8.7.7) | data-mode modulation | 48 frames of scrambled, differentially encoded ones |
| RM / RM' (8.7.4, 9.9.2) | Ki forced by Tables 25/26; data-mode constellations; **SHALL use the latest data-mode precoder and prefilter**; **SHALL be trellis encoded** | RM is 384T and RM' is 24T; RM starts on a data-frame boundary. RM: K0,K1 = M-1; K2,K3 = 0; K4,K5 = M-1; K6,K7 = 0; K8,K9 = M-1; K10 = 0; row 11 printed "u11 = 0" (Q-8). RM': the complement pattern, ending K10 = M10-1, "k11" = M11-1. |
| DATA (9.6.2.1.5) | full chain (6.4), continuing from B1u | after B1u (or FB1u then B1u), circuit 106 follows 105 |

#### Output level

- [derived] Definition 3.8 makes LU^2 the desired data-mode power (TRN1u is ±LU). 8.8.3 makes a mean square of 1 in G·v(n) equal to that same power.
- So, in the same units as TRN1u: **tx(n) = LU·G·v(n)**. The spec never writes this product; it is the only reading consistent with 3.8 and 8.8.3.
- SUVu bits 27:31 report 20·log10(RMS of G·v(n)) in signed Q2.2. The raw value 16 (-4.00) means "not measured". The transmitter should therefore track that RMS.

### 6.5 Items clause 6 does not specify

- No transmit spectrum mask, pre-emphasis or D/A filter is specified for PCM upstream.
- [interpretation] Anything after G·v(n) must be transparent at 8000 samples/s, because the precoder and prefilter already equalize the analogue path up to the CO A/D.

### 6.6 V.34-upstream fallback: what V.90 clause 6 required (rendered V.90 page 19)

[interpretation] This applies when V.92 selects V.34 upstream through INFO1a Table 19.

- **6.1/V.90:** SHALL support 4800 to 28 800 bit/s in 2400 steps; 31 200 and 33 600 are optional. No 200 bit/s auxiliary channel. The rate is set in Phase 4.
- **6.2/V.90:** SHALL support symbol rate 3200; MAY support 3000 and the optional 3429. SHALL NOT support 2400, 2743 or 2800. The analogue modem picks the rate in Phase 2.
- **6.3/V.90:** carriers per 5.3/V.34, set in Phase 2.
- **6.4/V.90:** pre-emphasis per 5.4/V.34, selected by the digital modem in Phase 2.
- **6.5/V.90:** GPA scrambler.
- **6.6/V.90:** V.34 primary-channel framing (clause 8/V.34).
- **6.7/V.90:** V.34 primary-channel encoder (clause 9/V.34).
- V.90 clause 6 preamble: after a fallback to V.34 mode, the analogue modem has V.34 characteristics.

---

## 7 Interchange circuits (clause 7)

- **R-7-1:** clause 7 applies to both modems.

### 7.1 List of interchange circuits

- **R-7.1-1:** V.24 circuit numbers mean the *functional equivalent* of the circuit, not a physical implementation. The spec's example is that "circuit 103" should be read as the functional equivalent of circuit 103.

Table 1/V.92 (rendered; identical to Table 6/V.90). Definitions are paraphrased from V.24 (2000) clause 3.

| No. | Description | Notes | V.24 meaning (paraphrase) |
|---|---|---|---|
| 102 | Signal ground or common return | | |
| 103 | Transmitted data | | DTE to DCE data |
| 104 | Received data | | DCE to DTE data. V.92 clause 9 clamps it to binary one during rate renegotiation and fast parameter exchange and unclamps it after B1d/B1u. |
| 105 | Request to send | | |
| 106 | Ready for sending | | turned OFF at the start of rate renegotiation and fast parameter exchange; after B1u, it follows 105 again (9.6.2.1.5) |
| 107 | Data set ready | | asserted by the analogue modem after detecting Jp (9.5.2.1.8) |
| 108/1 or 108/2 | Connect data set to line / Data terminal ready | | 108/1: OFF to ON connects the modem to the line, OFF disconnects once pending data has gone. 108/2: ON readies the DCE to connect; OFF makes the DCE drop the line after pending data. |
| 109 | Data channel received line signal detector | **1** | turned ON after B1d/B1u is received (clause 9) |
| 125 | Calling indicator | | ON while a calling signal is being received |
| 133 | Ready for receiving | **2** | DTE to DCE. ON means the DTE can accept data and the DCE may pass received data on 104. OFF means the DCE (or an intermediate function such as error control) holds the data. |

- **Note 1:** 109 thresholds and response times do not apply, because a line signal detector cannot tell the received signal from talker echo. [derived] So 109 is driven by the procedure, not by a level detector.
- **Note 2 (SHALL):** circuit 133 operates per **4.2.1.1/V.43**. V.43 is **not in `docs/specs`**, so its content could not be checked here. The V.24 definition above is the best available description.

### 7.2 Asynchronous character-mode interfacing

- **R-7.2-1 MAY:** include an asynchronous-to-synchronous converter that talks to the DTE in asynchronous (start-stop) mode.
- **R-7.2-2 SHALL:** the conversion protocol is **V.14, V.42 or V.80**.
- **R-7.2-3 MAY:** use data compression. No compression Recommendation is named. The repo has V.42bis and V.44, and AT+DS44 exists.

What V.14 requires (read from `T-REC-V.14-199303-I.txt`, clauses 1-7):

- It converts start-stop characters over a synchronous bearer "up to 19 200 bit/s" (see Q-11).
- The synchronous rate tolerance is ±0.01%.
- DTE rate ranges: basic +1%/-2.5% (preferred) or extended +2.3%/-2.5%, fixed at installation.
- Character formats: 1 start bit plus 7, 8 or 9 data bits plus 1 stop bit (9, 10 or 11 bits). An 8-bit format (6 data bits) is optional.
- Rate differences are absorbed by deleting stop elements at the transmitter and reinserting them at the receiver.
- Break signals follow V.14 7.3.

V.42 is LAPM; the QC signals' P bit asks for it (9.2.5, not in this digest). V.80 is not in `docs/specs`.

---

## 8 The clause 8 preamble and 8.1 (also on PDF page 13)

- All PCM codewords in training sequences are described with the Ucodes of Table 1/V.90.
- **In Tables 2-5, 11-24, 27 and 30-33, unless stated otherwise:**
  - values given as **bit patterns are sent leftmost bit first**;
  - values given as **integers are sent least-significant bit first**.
  - This covers Table 30 (CPd), so Mi, LZ/LP/LC, the Q-format coefficients and the constellation linear values are all LSB first.
- 8.1: all full Phase 1 signals and sequences are those of V.25, V.8 or V.8 bis.

---

## 9 Implementation notes

### 9.1 What is new compared with V.90 (clauses 1-7)

1. **Upstream is PCM** at 8000 symbols/s, 24 000 to 48 000 bit/s, with 12-symbol data frames, 6-symbol constellation frames and 4-symbol trellis frames. V.90's upstream was V.34 QAM with its own symbol clock.
2. **The analogue modem's transmit clock must follow the network clock** (6.2), and its symbol phase must be adjustable through Jp.
3. **New modulus encoder:**
   - 12 moduli carried *explicitly* in CPd (not "number of positive levels");
   - a new differential "sign" step (steps 2-4);
   - arithmetic up to 2^96.
4. **Precoding and prefiltering:**
   - an IIR/FIR precoder whose point choice is free within an equivalence class;
   - an ARMA prefilter;
   - a gain G;
   - all designed by the *digital* modem and downloaded in CPd.
5. **4D trellis coding on PCM levels.** The V.34 16-, 32- and 64-state codes are reused, clocked once per 4 symbols. The redundancy lives in the index parity of the 4th symbol, not in a modulo encoder.
6. **Scope additions:** short Phase 1 (QC, ANSpcm), a faster Phase 2, modem-on-hold, and V.34 fallback in either direction (clauses 8 and 9).
7. **Unchanged:** the digital modem's downstream encoder (V.90 clause 5 in full), the interchange circuits (Table 1 = Table 6/V.90), and 7.2 async conversion (identical wording).

### 9.2 Reuse map (repo)

| Need | Existing code | Change needed |
|---|---|---|
| GPA scrambler | `datapump::v32::Scrambler` (answer taps 5 and 23) | none |
| Figure 9, Table 13, 16/32/64-state codes | `datapump::v34::trellis` | none. Feed points (2·eta+1, 2·eta'+1) and clock once per trellis frame. Do **not** use `modulo()`/`inversion()`. |
| V.34 CRC-16 | `v34::info` / `v34::mp` | none |
| Ucode table | `v90::ucode` | none |
| Modulus encoder | `v90::modulus::encode` (6 × u16) | 12-interval u128 version plus the d(f) step |
| Downstream encoder and shaper | `v90::encoder`, `v90::sign` | none for V.92 downstream. CPu has a new drn offset (+20). |
| Upstream decoder (our digital modem) | none | new: Viterbi over 4D subsets plus the modulus decoder of 6.4.1 |

### 9.3 Pitfalls

- **P-1:** step 4 of 6.4.1 uses **d(f-1)**, not d(f). Mirror it exactly in the decoder.
- **P-2:** V.90's mapper labels count **down** from the largest code (label 0 is the largest). V.92 upstream indices are **signed and rise with level**. Do not share the V.90 label logic.
- **P-3:** V.90's Mi is "the number of positive levels". V.92 upstream's Mi is an explicit CPd field and generally differs from LC (N >= Mi, and N >= 2·Mi at k = 3).
- **P-4:** the k = 3 parity uses the indices **actually chosen** for k = 0..2 of the *same* trellis frame, and a non-negative mod 2 of possibly negative sums.
- **P-5:** the prefilter's feed-forward starts at κ = 0 (it uses the current x(n)) and runs to LZ2-1. The precoder's feed-forward starts at κ = 1 (u(n) enters with weight 1). Coefficient counts and indexing differ: z1(1..LZ1) versus z2(0..LZ2-1).
- **P-6:** sign conventions:
  - TRN1u: scrambler 0 is **+LU**.
  - Digital-modem sign bits (Jd, Jp, SCR, V.90 data): 0 is **negative**.
  - TRN2u tables: MSB 1 is negative.
- **P-7:** the scrambler polynomial is fixed by *role*, not by call direction: analogue uses GPA, digital uses GPC. Code copied from V.34 picks by call/answer.
- **P-8:** CPd parts are removed when absent. Parse sequentially and never by absolute bit position; Table 30's positions assume all parts are present.
- **P-9:** CPd fill runs to a multiple of **6** symbols, while CPu, SUVu and TRN segments use multiples of **12**.
- **P-10:** the unsigned Qa.b range in 3.5 reads [0, 2^(a+1)). The patterns (Q0.16 has no integer bit; Q3.13 has three) give [0, 2^a). Decode 4G as raw/65536, so G = raw/262144.
- **P-11:** the modulus arithmetic overflows u64 (M can reach 255^12, about 2^96). Use u128.
- **P-12:** the precoder is not a modulo precoder. With a one-member class and a feedback section, x(n) can grow. Keep x and v in f64 (or wide fixed point) and saturate the final D/A value, but never the filter state (that would desynchronize from the digital modem's model).
- **P-13 (our VoIP rig):** upstream PCM needs sample-exact, clock-locked delivery to the CO A/D. The ~20 ms concealment inserts and slips already seen on the softphone path (memory: VoIP jitter slips) will break data-frame alignment and the digital modem's view of the precoder. Expect upstream PCM to be much more fragile than downstream on that rig. A transcoding path such as Crazytel's makes upstream PCM hopeless; V.34 upstream fallback must stay solid.

### 9.4 Ambiguities and open questions

- **Q-1:** d(f-1) versus d(f) in step 4. It is printed as d(f-1) in the rendered PDF, confirmed at 300 dpi. Both readings decode; d(f-1) is implemented as printed. A real V.92 server capture would settle it: B1u is known data (scrambled ones), so a mismatch shows up immediately.
- **Q-2:** the initial d(-1) is not stated in clause 6. Taken as 0 from 8.7.1 ("modulus encoder memories initialized to zero") and 9.9.2 ("differential encoder" zeroed).
- **Q-3:** the point-selection rule is not normative. The 8.8.3 NOTE implies symbol-by-symbol minimum |x(n)|. Tie-breaking is unspecified; the lower |eta| (or lower eta) is proposed.
- **Q-4:** the constellation "linear value" (16 bits): signedness and scale are not stated. Taken as an unsigned magnitude, presumably on the V.90 Table 1 linear scale. G normalizes the power anyway, so the transmitter does not depend on the scale. Symmetry (a(-eta-1) = -a(eta)) is also implied, not stated.
- **Q-5:** "The number of points in a constellation set shall not exceed 128": does this mean N (LC <= 64) or LC (N <= 256)? The analogue side should accept LC up to 128 (N up to 256) and size its buffers for that.
- **Q-6:** a CPd without the modulus, coefficient or constellation part is presumably "keep the current values" (for rate renegotiation or fast parameter exchange that only changes the rate), but no clause says so. Neither 8.8.3 nor clause 9 was found to state it. Nothing says what an analogue modem should do if the first CPd of a training lacks a part. Proposed: treat a missing part with no previous value as a protocol error and retrain.
- **Q-7:** there is no procedure for an inconsistent CPd: 2^K > ΠM, N < Mi (or < 2Mi at k = 3), lengths above the INFO1a limits, a set index pointing at an empty set, or LZ2 = 0 (which makes v ≡ 0). Proposed: retrain (9.7.2.1) or cleardown.
- **Q-8:** Table 25 row 11 reads "u11 = 0", but constellations contain no zero point. It is almost certainly K11 = 0 (the pattern pairs K10, K11). Table 26 row 11 has a lowercase "k11 = M11 - 1".
- **Q-9:** 8.5.5's "the same structure used while transmitting 2 point TRN1u signal" is not defined anywhere. Presumably the plain, unprecoded path with no prefilter. Whether Su, TRN2u, CPu and SUVu are precoded is not stated; taken as not precoded.
- **Q-10:** Figure 10/V.34 labels two adders "Y2(m)". It is kept as printed, because the repo's implementation uses exactly that, passes free-distance and rotation-invariance tests, and works on live V.34 calls.
- **Q-11:** V.14's own scope stops at 19 200 bit/s, yet V.92 7.2 names it for 24-56 kbit/s links. Presumably its method is applied unchanged.
- **Q-12:** Note 2 of Table 1 points at V.43 4.2.1.1, which is not available.
- **Q-13:** there is no transmit clock tolerance or phase-adjustment procedure in clauses 1-7; see 9.5 (Jp/Jp'/S̄u) for the procedure. The exact meaning of "extend S̄u by a fraction of T" belongs to the 9.5 digest.
- **Q-14:** tx = LU·G·v(n) is inferred, not written. If the digital modem's power expectations (INFO1d bits 15:17, the "lower power" request) change LU, then G·v(n) follows the new LU.
- **Q-15:** [interpretation] V.34's modulo encoder (C0) and superframe bit inversion (V0) are not applied. 6.4.4 cites only 9.6.3.2 and there is no upstream superframe. A capture would confirm this: the parity of the four indices in each 4D symbol should equal the 16/32/64-state Y0 with no periodic inversions.

### 9.5 Test ideas

- **Modulus encoder round trip:** 12 moduli, K up to 72, random R. Include odd products M and the middle value R = (M-1)/2. Check d(f) tracking over many frames, and inverted-channel decoding with the decoder's d started inverted.
- **Trellis:** random data through the transmitter. Check that every trellis frame satisfies Σeta ≡ Y0 (mod 2) and that Table 13 inputs regenerate the same Y0 sequence in a separate encoder instance.
- **Precoder:** with z1 = p1 = 0, z2 = [1], LP2 = 0 and G = 1/rms, the output must equal the chosen levels. With a known p1, check |x(n)| stays bounded when N >= 2Mi, and that a toy receiver applying the inverse channel recovers each Ki as eta mod Mi.
- **B1u:** from zeroed state, B1u is a deterministic function of CPd. Generate it and compare against a capture when one exists (none yet: specs and real captures only).
